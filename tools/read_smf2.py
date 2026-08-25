import struct
d = open("ref/maps/geyservalley.smf",'rb').read()
w=l=640
hdr = struct.unpack_from('<16s2i5i2f7i', d, 0)
hPtr,tPtr,tiPtr,mmPtr,mePtr,fPtr,nExtra = hdr[10:]
print("=== physical block order (by offset), NOT header field order ===")
blocks = sorted([("grassmap",92),("heightmap",hPtr),("typemap",tPtr),("minimap",mmPtr),
                 ("metalmap",mePtr),("tileindex",tiPtr),("features",fPtr)], key=lambda x:x[1])
prev=None
for name,off in blocks:
    if prev: print(f"  {prev[0]:<10} {prev[1]:>8} .. {off:>8}  = {off-prev[1]:>8} bytes")
    prev=(name,off)
print(f"  {prev[0]:<10} {prev[1]:>8} .. {len(d):>8}  = {len(d)-prev[1]:>8} bytes")
exp = {"grassmap":(w//4)*(l//4), "heightmap":(w+1)*(l+1)*2, "typemap":(w//2)*(l//2),
       "minimap":699048, "metalmap":(w//2)*(l//2)}
print("\n=== sizes vs spec (comparing to the NEXT block by offset) ===")
for i,(name,off) in enumerate(blocks[:-1]):
    actual = blocks[i+1][1]-off
    if name in exp:
        good = actual==exp[name]
        print(("  OK   " if good else "  FAIL ")+f"{name}: {actual} vs spec {exp[name]}")

print("\n=== features block: wiki says numFeatures then numFeatureTypes ===")
o=fPtr
a=struct.unpack_from('<i',d,o)[0]; b=struct.unpack_from('<i',d,o+4)[0]
print(f"  first int = {a}, second int = {b}, block is {len(d)-fPtr} bytes")
print(f"  if {a} features of 24 bytes -> {a*24} bytes needed (have {len(d)-fPtr-8})")
o=fPtr+8
names=[]
try:
    for i in range(b):
        e=d.index(b'\0',o); names.append(d[o:e].decode('latin1')); o=e+1
    print(f"  reading {b} names: {names}")
    print(f"  names consumed {o-(fPtr+8)} bytes, {len(d)-o} left = {(len(d)-o)/24:g} features of 24B")
except Exception as ex:
    print("  name read failed:", ex)
# try the other interpretation
o=fPtr+8; names2=[]
try:
    for i in range(a):
        e=d.index(b'\0',o); names2.append(d[o:e].decode('latin1')); o=e+1
    print(f"  ALT: reading {a} names: {names2[:20]}")
    print(f"  ALT names consumed {o-(fPtr+8)}B, {len(d)-o} left = {(len(d)-o)/24:g} features")
except Exception as ex:
    print("  ALT failed:", ex)

print("\n=== heightmap statistics ===")
n=(w+1)*(l+1)
hm=struct.unpack_from('<%dH'%n, d, hPtr)
print(f"  {len(hm)} samples, min {min(hm)}, max {max(hm)}")
minh,maxh=-60.0,440.0
print(f"  -> elmo range {minh + min(hm)/65535*(maxh-minh):.1f} .. {minh + max(hm)/65535*(maxh-minh):.1f}")
water = -minh/(maxh-minh)
print(f"  waterline at normalised {water:.4f} = 16-bit value {round(water*65535)}")
below=sum(1 for v in hm if v < water*65535)
print(f"  {100*below/len(hm):.1f}% of vertices are under water")

print("\n=== typemap ===")
tm=d[tPtr:tPtr+(w//2)*(l//2)]
print(f"  distinct terrain type indices used: {sorted(set(tm))}")

print("\n=== SMT header ===")
s=open("ref/maps/geyservalley.smt",'rb').read()
magic=s[0:16]; ver,nT,tsz,comp = struct.unpack_from('<4i', s, 16)
print(f"  magic {magic}")
print(f"  version {ver}, numTiles {nT}, tileSize {tsz}, compression {comp} ({'DXT1' if comp==1 else '?'})")
print(f"  header 32 bytes + {nT} tiles x 680 = {32+nT*680} ; file is {len(s)}")
print(("  OK   " if 32+nT*680==len(s) else "  FAIL ")+"SMT size == 32 + numTiles*680")
