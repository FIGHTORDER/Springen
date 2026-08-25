import struct, sys, os, hashlib
from collections import Counter

path = sys.argv[1]
d = open(path,'rb').read()
o = 0
def I(): 
    global o
    v = struct.unpack_from('<i', d, o)[0]; o += 4; return v
def F():
    global o
    v = struct.unpack_from('<f', d, o)[0]; o += 4; return v

magic = d[0:16]; o = 16
print("magic          ", magic)
assert magic == b"spring map file\0", magic
ver = I(); mapid = I()
width = I(); length = I()
sqsize = I(); texelspersq = I(); tilesize = I()
minh = F(); maxh = F()
hPtr=I(); tPtr=I(); tiPtr=I(); mmPtr=I(); mePtr=I(); fPtr=I()
nExtra = I()
print(f"version        {ver}\nmapid          {mapid}")
print(f"width x length {width} x {length}   (smu {width/64:g} x {length/64:g})")
print(f"squareSize     {sqsize}\ntexelsPerSquare{texelspersq}\ntileSize       {tilesize}")
print(f"minHeight      {minh}\nmaxHeight      {maxh}")
print(f"offsets        height={hPtr} type={tPtr} tileindex={tiPtr} minimap={mmPtr} metal={mePtr} features={fPtr}")
print(f"numExtraHeader {nExtra}   (header ends at {o})")

print("\n--- spec conformance ---")
checks = [
 ("width % 128 == 0", width % 128 == 0, width % 128),
 ("length % 128 == 0", length % 128 == 0, length % 128),
 ("smu even", (width//64) % 2 == 0, width//64),
 ("squareSize == 8", sqsize == 8, sqsize),
 ("texelsPerSquare == 8", texelspersq == 8, texelspersq),
 ("tileSize == 32", tilesize == 32, tilesize),
 ("header is 80 bytes", o == 80, o),
]
for n,c,v in checks: print(("  OK   " if c else "  FAIL ")+n, "" if c else f"-> {v}")

# extra headers
for i in range(nExtra):
    size = I(); typ = I()
    body = d[o:o+size-8]; o += size-8
    extra = struct.unpack_from('<i', body, 0)[0] if len(body)>=4 else None
    print(f"  extra header {i}: size={size} type={typ} ptr={extra}"
          f"  {'(grass map)' if typ==1 else ''}")
    if typ == 1: grassPtr = extra

print("\n--- derived block sizes vs actual offsets ---")
hm_bytes = (width+1)*(length+1)*2
tm_bytes = (width//2)*(length//2)
mm_bytes = 699048
me_bytes = (width//2)*(length//2)
gr_bytes = (width//4)*(length//4)
ti_count = (width//4)*(length//4)

# The physical block order is NOT the header field order, and a writer may
# pick any order it likes -- so sort by offset and measure each block against
# whatever actually follows it. Assuming adjacency here misreports a
# perfectly good file: on the measured reference map the minimap is followed
# by the metalmap, not by the tile index.
blocks = [("heightmap", hPtr), ("typemap", tPtr), ("minimap", mmPtr),
          ("metalmap", mePtr), ("tileindex", tiPtr), ("features", fPtr)]
if grassPtr is not None: blocks.append(("grassmap", grassPtr))
blocks.sort(key=lambda b: b[1])
spans = {}
print("  physical order, by offset:")
for i,(name,off) in enumerate(blocks):
    end = blocks[i+1][1] if i+1 < len(blocks) else len(d)
    spans[name] = end-off
    print(f"    {name:<10} {off:>10} .. {end:<10} = {end-off}")
for n,c,v in [
  ("heightmap block size matches (width+1)(length+1)*2", spans["heightmap"] == hm_bytes, spans["heightmap"]),
  ("typemap block size matches (width/2)(length/2)", spans["typemap"] == tm_bytes, spans["typemap"]),
  ("minimap block is exactly 699048", spans["minimap"] == mm_bytes, spans["minimap"]),
  ("metalmap block size matches (width/2)(length/2)", spans["metalmap"] == me_bytes, spans["metalmap"]),
] + ([("grass block size matches (width/4)(length/4)", spans["grassmap"] == gr_bytes, spans["grassmap"])]
     if grassPtr is not None else []):
    print(("  OK   " if c else "  FAIL ")+n, "" if c else f"-> {v}")

# tile index header
o = tiPtr
nTileFiles = I(); nTilesTotal = I()
print(f"\ntile index: {nTileFiles} smt file(s), {nTilesTotal} tiles total")
for i in range(nTileFiles):
    nt = I()
    end = d.index(b'\0', o)
    fn = d[o:end].decode(); o = end+1
    print(f"  smt[{i}] {nt} tiles  '{fn}'")
idxStart = o
print(f"  index array starts {idxStart}, expect {ti_count} ints = {ti_count*4} bytes")
idx = struct.unpack_from('<%di'%ti_count, d, idxStart)
idxEnd = idxStart + ti_count*4
print(f"  index array ends {idxEnd}; tileindex block spans {spans['tileindex']} bytes")
for n,c,v in [
  ("tile index count == (width/4)(length/4)", ti_count == len(idx), (ti_count,len(idx))),
  ("index array fills the tileindex block", idxEnd == tiPtr + spans["tileindex"], idxEnd - (tiPtr + spans["tileindex"])),
  ("all tile indices within range", max(idx) < nTilesTotal and min(idx) >= 0, (min(idx),max(idx))),
]: print(("  OK   " if c else "  FAIL ")+n, "" if c else f"-> {v}")

# dedup reality on a real hand-made map
uniq = len(set(idx))
print(f"\n  tile index: {len(idx)} slots, {uniq} distinct tiles referenced")
print(f"  tiles stored in smt: {nTilesTotal}")
print(f"  dedup ratio: {100*(1-nTilesTotal/len(idx)):.1f}% fewer tiles stored than slots")
c = Counter(idx).most_common(3)
print(f"  most reused tiles: {c}")

# metalmap + features
o = mePtr + me_bytes
print(f"\nmetalmap {me_bytes} bytes at {mePtr}; block spans {spans['metalmap']}")
metal = d[mePtr:mePtr+me_bytes]
nz = sum(1 for b in metal if b)
print(f"  metalmap {width//2}x{length//2}: {nz} non-zero px ({100*nz/len(metal):.2f}%), max {max(metal)}")
o = fPtr
nFeat = I(); nTypes = I()
print(f"features: {nFeat} features, {nTypes} types")
names=[]
for i in range(nTypes):
    end = d.index(b'\0', o); names.append(d[o:end].decode()); o = end+1
print(f"  types: {names[:8]}{' ...' if len(names)>8 else ''}")
