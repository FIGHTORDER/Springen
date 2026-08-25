//! Hand-rolled PNG encoder.
//!
//! The prototype's encoder was validated against Pillow byte for byte, and the
//! golden files were written with its stored-deflate path, so [`Compression::Stored`]
//! must stay exactly as it is. [`Compression::Deflate`] exists because a real
//! archive does not want a 200 MB uncompressed splat texture.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PngColor {
    Gray,
    Rgb,
    Rgba,
}

impl PngColor {
    pub fn channels(self) -> usize {
        match self {
            PngColor::Gray => 1,
            PngColor::Rgb => 3,
            PngColor::Rgba => 4,
        }
    }
    fn code(self) -> u8 {
        match self {
            PngColor::Gray => 0,
            PngColor::Rgb => 2,
            PngColor::Rgba => 6,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Compression {
    /// Valid zlib made of stored blocks. Byte-identical to the prototype.
    Stored,
    /// Real DEFLATE. Smaller, and what archive output uses.
    #[default]
    Deflate,
}

const CRC_TABLE: [u32; 256] = build_crc_table();

const fn build_crc_table() -> [u32; 256] {
    let mut t = [0u32; 256];
    let mut n = 0;
    while n < 256 {
        let mut c = n as u32;
        let mut k = 0;
        while k < 8 {
            c = if c & 1 != 0 {
                0xedb8_8320 ^ (c >> 1)
            } else {
                c >> 1
            };
            k += 1;
        }
        t[n] = c;
        n += 1;
    }
    t
}

pub fn crc32(bytes: &[u8]) -> u32 {
    let mut c = 0xffff_ffffu32;
    for b in bytes {
        c = CRC_TABLE[((c ^ u32::from(*b)) & 0xff) as usize] ^ (c >> 8);
    }
    c ^ 0xffff_ffff
}

pub fn adler32(bytes: &[u8]) -> u32 {
    let mut a = 1u32;
    let mut b = 0u32;
    for v in bytes {
        a = (a + u32::from(*v)) % 65521;
        b = (b + a) % 65521;
    }
    (b << 16) | a
}

/// zlib stream made of stored (uncompressed) deflate blocks.
pub fn zlib_store(data: &[u8]) -> Vec<u8> {
    let blocks = data.len().div_ceil(65535).max(1);
    let mut out = Vec::with_capacity(2 + blocks * 5 + data.len() + 4);
    out.push(0x78);
    out.push(0x01);
    let mut pos = 0usize;
    for i in 0..blocks {
        let len = (data.len() - pos).min(65535);
        let last = u8::from(i == blocks - 1);
        out.push(last);
        out.push((len & 0xff) as u8);
        out.push(((len >> 8) & 0xff) as u8);
        let n = !(len as u16);
        out.push((n & 0xff) as u8);
        out.push(((n >> 8) & 0xff) as u8);
        out.extend_from_slice(&data[pos..pos + len]);
        pos += len;
    }
    out.extend_from_slice(&adler32(data).to_be_bytes());
    out
}

fn chunk(kind: &[u8; 4], data: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(12 + data.len());
    out.extend_from_slice(&(data.len() as u32).to_be_bytes());
    out.extend_from_slice(kind);
    out.extend_from_slice(data);
    let c = crc32(&out[4..]);
    out.extend_from_slice(&c.to_be_bytes());
    out
}

/// Scanlines with filter type 0 (none) — what the prototype writes.
pub fn filtered_rows(
    width: usize,
    height: usize,
    color: PngColor,
    bit_depth: u8,
    samples: &[u16],
) -> Vec<u8> {
    let channels = color.channels();
    let bpp = channels * usize::from(bit_depth) / 8;
    let stride = width * bpp;
    let mut raw = vec![0u8; (stride + 1) * height];
    for y in 0..height {
        let ro = y * (stride + 1);
        raw[ro] = 0;
        for x in 0..width * channels {
            let v = samples[y * width * channels + x];
            if bit_depth == 16 {
                raw[ro + 1 + x * 2] = (v >> 8) as u8;
                raw[ro + 2 + x * 2] = (v & 255) as u8;
            } else {
                raw[ro + 1 + x] = (v & 255) as u8;
            }
        }
    }
    raw
}

pub fn assemble(
    width: usize,
    height: usize,
    color: PngColor,
    bit_depth: u8,
    zlib_stream: &[u8],
) -> Vec<u8> {
    let mut ihdr = Vec::with_capacity(13);
    ihdr.extend_from_slice(&(width as u32).to_be_bytes());
    ihdr.extend_from_slice(&(height as u32).to_be_bytes());
    ihdr.push(bit_depth);
    ihdr.push(color.code());
    ihdr.extend_from_slice(&[0, 0, 0]);
    let mut out = Vec::new();
    out.extend_from_slice(&[137, 80, 78, 71, 13, 10, 26, 10]);
    out.extend_from_slice(&chunk(b"IHDR", &ihdr));
    out.extend_from_slice(&chunk(b"IDAT", zlib_stream));
    out.extend_from_slice(&chunk(b"IEND", &[]));
    out
}

pub fn encode(
    width: usize,
    height: usize,
    color: PngColor,
    bit_depth: u8,
    samples: &[u16],
    compression: Compression,
) -> Vec<u8> {
    let raw = filtered_rows(width, height, color, bit_depth, samples);
    let stream = match compression {
        Compression::Stored => zlib_store(&raw),
        Compression::Deflate => {
            use flate2::write::ZlibEncoder;
            use std::io::Write;
            let mut e = ZlibEncoder::new(Vec::new(), flate2::Compression::new(6));
            e.write_all(&raw).expect("in-memory write cannot fail");
            e.finish().expect("in-memory finish cannot fail")
        }
    };
    assemble(width, height, color, bit_depth, &stream)
}

/* --------------------------------------------------------------- reading */

/// A decoded image, as 16-bit samples whatever the file's own depth was.
#[derive(Clone, Debug)]
pub struct Decoded {
    pub width: usize,
    pub height: usize,
    pub color: PngColor,
    /// `width · height · channels`, scaled to the full 16-bit range so an
    /// 8-bit file and a 16-bit file mean the same thing to a caller.
    pub samples: Vec<u16>,
}

impl Decoded {
    /// One channel of one pixel as a 0..1 field value.
    pub fn value(&self, x: usize, y: usize, channel: usize) -> f64 {
        let ch = self.color.channels();
        let i = (y.min(self.height - 1) * self.width + x.min(self.width - 1)) * ch
            + channel.min(ch - 1);
        f64::from(self.samples[i]) / 65535.0
    }
}

/// Decode a PNG.
///
/// Deliberately narrow: no interlacing, no palettes, no ancillary chunks. It
/// exists to read back what [`encode`] wrote — a project's own rasters — and a
/// decoder that silently half-understands a file it was not designed for is
/// worse than one that says so. Every filter type is handled, because image
/// editors use them even when we do not, and a hand-edited heightmap coming
/// back from another tool is the whole point of storing rasters as PNG.
pub fn decode(bytes: &[u8]) -> std::io::Result<Decoded> {
    use std::io::{Error, ErrorKind};
    let bad = |m: String| Error::new(ErrorKind::InvalidData, m);
    if bytes.len() < 8 || bytes[..8] != [137, 80, 78, 71, 13, 10, 26, 10] {
        return Err(bad("not a PNG".into()));
    }
    let (mut w, mut h, mut depth, mut ct) = (0usize, 0usize, 0u8, 0u8);
    let mut idat: Vec<u8> = Vec::new();
    let mut pos = 8;
    while pos + 8 <= bytes.len() {
        let len = u32::from_be_bytes(bytes[pos..pos + 4].try_into().unwrap()) as usize;
        let kind = &bytes[pos + 4..pos + 8];
        let end = pos + 8 + len;
        if end + 4 > bytes.len() {
            return Err(bad("a chunk runs past the end of the file".into()));
        }
        let data = &bytes[pos + 8..end];
        match kind {
            b"IHDR" => {
                if data.len() < 13 {
                    return Err(bad("truncated IHDR".into()));
                }
                w = u32::from_be_bytes(data[0..4].try_into().unwrap()) as usize;
                h = u32::from_be_bytes(data[4..8].try_into().unwrap()) as usize;
                depth = data[8];
                ct = data[9];
                if data[12] != 0 {
                    return Err(bad("interlaced PNGs are not supported".into()));
                }
            }
            b"IDAT" => idat.extend_from_slice(data),
            b"IEND" => break,
            _ => {}
        }
        pos = end + 4;
    }
    if w == 0 || h == 0 {
        return Err(bad("the image has no size".into()));
    }
    let color = match ct {
        0 => PngColor::Gray,
        2 => PngColor::Rgb,
        6 => PngColor::Rgba,
        _ => {
            return Err(bad(format!(
                "colour type {ct} is not supported; use greyscale, RGB or RGBA"
            )))
        }
    };
    if depth != 8 && depth != 16 {
        return Err(bad(format!(
            "bit depth {depth} is not supported; use 8 or 16"
        )));
    }

    let raw = {
        use flate2::read::ZlibDecoder;
        use std::io::Read;
        let mut d = ZlibDecoder::new(idat.as_slice());
        let mut out = Vec::new();
        d.read_to_end(&mut out)?;
        out
    };

    let ch = color.channels();
    let bpp = ch * usize::from(depth) / 8;
    let stride = w * bpp;
    if raw.len() < (stride + 1) * h {
        return Err(bad(format!(
            "the pixel data is {} bytes; {} × {} at depth {depth} needs {}",
            raw.len(),
            w,
            h,
            (stride + 1) * h
        )));
    }
    let mut lines: Vec<u8> = vec![0; stride * h];
    for y in 0..h {
        let filter = raw[y * (stride + 1)];
        let src = &raw[y * (stride + 1) + 1..y * (stride + 1) + 1 + stride];
        for x in 0..stride {
            let a = if x >= bpp {
                lines[y * stride + x - bpp]
            } else {
                0
            };
            let b = if y > 0 {
                lines[(y - 1) * stride + x]
            } else {
                0
            };
            let c = if x >= bpp && y > 0 {
                lines[(y - 1) * stride + x - bpp]
            } else {
                0
            };
            let v = src[x];
            lines[y * stride + x] = match filter {
                0 => v,
                1 => v.wrapping_add(a),
                2 => v.wrapping_add(b),
                3 => v.wrapping_add(((u16::from(a) + u16::from(b)) / 2) as u8),
                4 => {
                    let p = i16::from(a) + i16::from(b) - i16::from(c);
                    let (pa, pb, pc) = (
                        (p - i16::from(a)).abs(),
                        (p - i16::from(b)).abs(),
                        (p - i16::from(c)).abs(),
                    );
                    let pred = if pa <= pb && pa <= pc {
                        a
                    } else if pb <= pc {
                        b
                    } else {
                        c
                    };
                    v.wrapping_add(pred)
                }
                other => return Err(bad(format!("unknown row filter {other}"))),
            };
        }
    }

    let mut samples = Vec::with_capacity(w * h * ch);
    for y in 0..h {
        for x in 0..w * ch {
            let o = y * stride;
            samples.push(if depth == 16 {
                u16::from_be_bytes([lines[o + x * 2], lines[o + x * 2 + 1]])
            } else {
                // 8-bit widened so callers never have to ask about depth.
                u16::from(lines[o + x]) * 257
            });
        }
    }
    Ok(Decoded {
        width: w,
        height: h,
        color,
        samples,
    })
}

#[cfg(test)]
mod decode_tests {
    use super::*;

    fn round_trip(color: PngColor, depth: u8, w: usize, h: usize) {
        let ch = color.channels();
        let samples: Vec<u16> = (0..w * h * ch)
            .map(|i| {
                if depth == 16 {
                    (i * 7919 % 65536) as u16
                } else {
                    ((i * 37 % 256) as u16) * 257
                }
            })
            .collect();
        // The encoder takes samples in the file's own depth.
        let enc: Vec<u16> = if depth == 16 {
            samples.clone()
        } else {
            samples.iter().map(|v| v / 257).collect()
        };
        let png = encode(w, h, color, depth, &enc, Compression::Deflate);
        let d = decode(&png).unwrap();
        assert_eq!((d.width, d.height), (w, h));
        assert_eq!(d.color, color);
        assert_eq!(d.samples, samples, "{color:?} at depth {depth}");
    }

    #[test]
    fn everything_the_encoder_writes_reads_back_exactly() {
        // A heightmap raster is 16-bit greyscale, and losing a bit of it would
        // lose a bit of somebody's terrain.
        round_trip(PngColor::Gray, 16, 9, 5);
        round_trip(PngColor::Gray, 8, 9, 5);
        round_trip(PngColor::Rgb, 8, 7, 4);
        round_trip(PngColor::Rgba, 8, 4, 4);
        round_trip(PngColor::Rgb, 16, 3, 3);
    }

    #[test]
    fn an_eight_bit_file_and_a_sixteen_bit_file_mean_the_same_thing() {
        let g8 = encode(2, 1, PngColor::Gray, 8, &[0, 255], Compression::Deflate);
        let g16 = encode(2, 1, PngColor::Gray, 16, &[0, 65535], Compression::Deflate);
        let (a, b) = (decode(&g8).unwrap(), decode(&g16).unwrap());
        assert_eq!(a.value(0, 0, 0), 0.0);
        assert_eq!(a.value(1, 0, 0), 1.0);
        assert_eq!(a.value(1, 0, 0), b.value(1, 0, 0));
    }

    #[test]
    fn every_row_filter_decodes() {
        // We only write filter 0, but an image editor will not.
        let (w, h) = (6usize, 4usize);
        let pixels: Vec<u8> = (0..w * h * 3).map(|i| (i * 13 % 256) as u8).collect();
        for filter in 0..=4u8 {
            let stride = w * 3;
            let mut raw = Vec::with_capacity((stride + 1) * h);
            let mut prev = vec![0u8; stride];
            for y in 0..h {
                raw.push(filter);
                let line = &pixels[y * stride..(y + 1) * stride];
                for x in 0..stride {
                    let a = if x >= 3 { line[x - 3] } else { 0 };
                    let b = prev[x];
                    let c = if x >= 3 { prev[x - 3] } else { 0 };
                    let v = line[x];
                    raw.push(match filter {
                        0 => v,
                        1 => v.wrapping_sub(a),
                        2 => v.wrapping_sub(b),
                        3 => v.wrapping_sub(((u16::from(a) + u16::from(b)) / 2) as u8),
                        _ => {
                            let p = i16::from(a) + i16::from(b) - i16::from(c);
                            let (pa, pb, pc) = (
                                (p - i16::from(a)).abs(),
                                (p - i16::from(b)).abs(),
                                (p - i16::from(c)).abs(),
                            );
                            let pred = if pa <= pb && pa <= pc {
                                a
                            } else if pb <= pc {
                                b
                            } else {
                                c
                            };
                            v.wrapping_sub(pred)
                        }
                    });
                }
                prev = line.to_vec();
            }
            let stream = {
                use flate2::write::ZlibEncoder;
                use std::io::Write;
                let mut e = ZlibEncoder::new(Vec::new(), flate2::Compression::new(6));
                e.write_all(&raw).unwrap();
                e.finish().unwrap()
            };
            let png = assemble(w, h, PngColor::Rgb, 8, &stream);
            let d = decode(&png).unwrap();
            let got: Vec<u8> = d.samples.iter().map(|v| (v / 257) as u8).collect();
            assert_eq!(got, pixels, "filter {filter} did not decode");
        }
    }

    #[test]
    fn things_that_are_not_our_pngs_are_refused_with_a_reason() {
        assert!(decode(b"not a png at all").is_err());
        let truncated = &encode(4, 4, PngColor::Gray, 8, &[0; 16], Compression::Deflate)[..20];
        assert!(decode(truncated).is_err());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crc_and_adler_match_known_vectors() {
        assert_eq!(crc32(b"IEND"), 0xae42_6082);
        assert_eq!(adler32(b"abc"), 0x024d_0127);
    }

    #[test]
    fn both_compressions_produce_a_valid_png_header() {
        let samples: Vec<u16> = (0..16).collect();
        for c in [Compression::Stored, Compression::Deflate] {
            let png = encode(4, 4, PngColor::Gray, 8, &samples, c);
            assert_eq!(&png[..8], &[137, 80, 78, 71, 13, 10, 26, 10]);
            assert_eq!(&png[12..16], b"IHDR");
            assert_eq!(&png[png.len() - 8..png.len() - 4], b"IEND");
        }
        // Deflate must actually be smaller on compressible data.
        let flat = vec![7u16; 4096];
        let s = encode(64, 64, PngColor::Gray, 8, &flat, Compression::Stored);
        let d = encode(64, 64, PngColor::Gray, 8, &flat, Compression::Deflate);
        assert!(d.len() < s.len());
    }
}
