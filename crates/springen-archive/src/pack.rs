//! The blueprint tree and the three ways to ship it.
//!
//! A folder named `something.sdd` is loaded directly by the engine, which is
//! how you test without re-archiving. `.sd7` (7-Zip) and `.sdz` (zip) are the
//! distributable forms.
//!
//! 7-Zip settings follow the mapping guides: LZMA2, and **not solid**. Solid
//! archives break the engine's random access into the archive, which is the one
//! packing mistake that produces a map that loads but stutters.

use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use springen_core::png::crc32;

/// Where an entry's bytes come from. Large layers stay on disk so a 32×32 map
/// never has to be held in memory to be packed.
pub enum Source {
    Bytes(Vec<u8>),
    File(PathBuf),
}

pub struct Entry {
    /// Archive-relative path, always with forward slashes.
    pub path: String,
    pub source: Source,
}

#[derive(Default)]
pub struct Blueprint {
    pub entries: Vec<Entry>,
}

impl Blueprint {
    pub fn new() -> Blueprint {
        Blueprint::default()
    }

    pub fn add_bytes(&mut self, path: impl Into<String>, bytes: Vec<u8>) {
        self.entries.push(Entry {
            path: path.into(),
            source: Source::Bytes(bytes),
        });
    }

    pub fn add_text(&mut self, path: impl Into<String>, text: &str) {
        self.add_bytes(path, text.as_bytes().to_vec());
    }

    pub fn add_file(&mut self, path: impl Into<String>, file: impl Into<PathBuf>) {
        self.entries.push(Entry {
            path: path.into(),
            source: Source::File(file.into()),
        });
    }

    /// Total uncompressed size, for the "50.4 MB uncompressed" style readout.
    pub fn uncompressed_len(&self) -> io::Result<u64> {
        let mut n = 0u64;
        for e in &self.entries {
            n += match &e.source {
                Source::Bytes(b) => b.len() as u64,
                Source::File(p) => fs::metadata(p)?.len(),
            };
        }
        Ok(n)
    }

    fn read(&self, e: &Entry) -> io::Result<Vec<u8>> {
        match &e.source {
            Source::Bytes(b) => Ok(b.clone()),
            Source::File(p) => fs::read(p),
        }
    }

    /// Write the tree as a loadable `.sdd` folder.
    pub fn write_sdd(&self, dir: &Path) -> io::Result<()> {
        for e in &self.entries {
            let out = dir.join(&e.path);
            if let Some(parent) = out.parent() {
                fs::create_dir_all(parent)?;
            }
            match &e.source {
                Source::Bytes(b) => fs::write(&out, b)?,
                Source::File(p) => {
                    if p.canonicalize().ok() != out.canonicalize().ok() {
                        fs::copy(p, &out)?;
                    }
                }
            }
        }
        Ok(())
    }

    /// Pack as `.sd7`. Entries are compressed independently — non-solid.
    pub fn write_sd7(&self, path: &Path) -> io::Result<()> {
        use sevenz_rust2::encoder_options::Lzma2Options;
        use sevenz_rust2::{ArchiveEntry, ArchiveWriter};

        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let mut w = ArchiveWriter::create(path).map_err(to_io)?;
        // Level 9 with a 32 MB dictionary, matching what the guides specify.
        let mut opts = Lzma2Options::from_level(9);
        opts.set_dictionary_size(32 * 1024 * 1024);
        w.set_content_methods(vec![opts.into()]);
        // Keep the header plain so every 7z reader can open it.
        w.set_encrypt_header(false);

        for e in &self.entries {
            let entry = ArchiveEntry::new_file(&e.path);
            match &e.source {
                Source::Bytes(b) => {
                    w.push_archive_entry(entry, Some(io::Cursor::new(b.clone())))
                        .map_err(to_io)?;
                }
                Source::File(p) => {
                    let f = fs::File::open(p)?;
                    w.push_archive_entry(entry, Some(f)).map_err(to_io)?;
                }
            };
        }
        w.finish()?;
        Ok(())
    }

    /// Pack as `.sdz`, a plain zip. Every entry is deflated independently, so
    /// random access into the archive still works.
    pub fn write_sdz(&self, path: &Path) -> io::Result<()> {
        use flate2::write::DeflateEncoder;

        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let mut out: Vec<u8> = Vec::new();
        let mut central: Vec<u8> = Vec::new();
        let mut count = 0u16;

        for e in &self.entries {
            let data = self.read(e)?;
            let crc = crc32(&data);
            let mut enc = DeflateEncoder::new(Vec::new(), flate2::Compression::best());
            enc.write_all(&data)?;
            let comp = enc.finish()?;
            // Storing is only worth it when deflate made things worse.
            let (method, payload): (u16, &[u8]) = if comp.len() < data.len() {
                (8, &comp)
            } else {
                (0, &data)
            };
            let offset = out.len() as u32;
            let name = e.path.as_bytes();

            out.extend_from_slice(&0x0403_4b50u32.to_le_bytes());
            out.extend_from_slice(&20u16.to_le_bytes());
            out.extend_from_slice(&0u16.to_le_bytes());
            out.extend_from_slice(&method.to_le_bytes());
            // A fixed 1980-01-01 timestamp keeps the archive reproducible.
            out.extend_from_slice(&0u16.to_le_bytes());
            out.extend_from_slice(&0x0021u16.to_le_bytes());
            out.extend_from_slice(&crc.to_le_bytes());
            out.extend_from_slice(&(payload.len() as u32).to_le_bytes());
            out.extend_from_slice(&(data.len() as u32).to_le_bytes());
            out.extend_from_slice(&(name.len() as u16).to_le_bytes());
            out.extend_from_slice(&0u16.to_le_bytes());
            out.extend_from_slice(name);
            out.extend_from_slice(payload);

            central.extend_from_slice(&0x0201_4b50u32.to_le_bytes());
            central.extend_from_slice(&20u16.to_le_bytes());
            central.extend_from_slice(&20u16.to_le_bytes());
            central.extend_from_slice(&0u16.to_le_bytes());
            central.extend_from_slice(&method.to_le_bytes());
            central.extend_from_slice(&0u16.to_le_bytes());
            central.extend_from_slice(&0x0021u16.to_le_bytes());
            central.extend_from_slice(&crc.to_le_bytes());
            central.extend_from_slice(&(payload.len() as u32).to_le_bytes());
            central.extend_from_slice(&(data.len() as u32).to_le_bytes());
            central.extend_from_slice(&(name.len() as u16).to_le_bytes());
            central.extend_from_slice(&0u16.to_le_bytes()); // extra
            central.extend_from_slice(&0u16.to_le_bytes()); // comment
            central.extend_from_slice(&0u16.to_le_bytes()); // disk
            central.extend_from_slice(&0u16.to_le_bytes()); // internal attrs
            central.extend_from_slice(&0u32.to_le_bytes()); // external attrs
            central.extend_from_slice(&offset.to_le_bytes());
            central.extend_from_slice(name);
            count += 1;
        }

        let cd_offset = out.len() as u32;
        out.extend_from_slice(&central);
        out.extend_from_slice(&0x0605_4b50u32.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(&count.to_le_bytes());
        out.extend_from_slice(&count.to_le_bytes());
        out.extend_from_slice(&(central.len() as u32).to_le_bytes());
        out.extend_from_slice(&cd_offset.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        fs::write(path, out)
    }
}

fn to_io(e: sevenz_rust2::Error) -> io::Error {
    io::Error::other(e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Blueprint {
        let mut b = Blueprint::new();
        b.add_text("mapinfo.lua", "return {}");
        b.add_bytes("maps/test.smf", vec![7u8; 4096]);
        b.add_text("mapconfig/map_metal_layout.lua", "return { spots = {} }");
        b
    }

    #[test]
    fn sdd_writes_the_tree_at_the_archive_root() {
        let dir = std::env::temp_dir().join("springen-test-sdd");
        let _ = fs::remove_dir_all(&dir);
        sample().write_sdd(&dir).unwrap();
        assert!(dir.join("mapinfo.lua").is_file());
        assert!(dir.join("maps/test.smf").is_file());
        assert!(dir.join("mapconfig/map_metal_layout.lua").is_file());
        assert_eq!(fs::read(dir.join("maps/test.smf")).unwrap().len(), 4096);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn sdz_is_a_readable_zip() {
        let path = std::env::temp_dir().join("springen-test.sdz");
        sample().write_sdz(&path).unwrap();
        let bytes = fs::read(&path).unwrap();
        assert_eq!(&bytes[..4], &0x0403_4b50u32.to_le_bytes());
        // End of central directory, with the entry count.
        let eocd = bytes.len() - 22;
        assert_eq!(&bytes[eocd..eocd + 4], &0x0605_4b50u32.to_le_bytes());
        assert_eq!(
            u16::from_le_bytes(bytes[eocd + 8..eocd + 10].try_into().unwrap()),
            3
        );
        fs::remove_file(&path).unwrap();
    }

    #[test]
    fn sdz_is_reproducible() {
        let a = std::env::temp_dir().join("springen-repro-a.sdz");
        let b = std::env::temp_dir().join("springen-repro-b.sdz");
        sample().write_sdz(&a).unwrap();
        sample().write_sdz(&b).unwrap();
        assert_eq!(fs::read(&a).unwrap(), fs::read(&b).unwrap());
        fs::remove_file(&a).unwrap();
        fs::remove_file(&b).unwrap();
    }

    #[test]
    fn sd7_has_the_seven_zip_signature_and_round_trips() {
        let path = std::env::temp_dir().join("springen-test.sd7");
        sample().write_sd7(&path).unwrap();
        let bytes = fs::read(&path).unwrap();
        assert_eq!(&bytes[..6], &[b'7', b'z', 0xBC, 0xAF, 0x27, 0x1C]);

        // Read it back with the same library and check the tree survived.
        let mut reader = sevenz_rust2::ArchiveReader::open(&path, "".into()).unwrap();
        let names: Vec<String> = reader
            .archive()
            .files
            .iter()
            .map(|f| f.name.clone())
            .collect();
        assert!(names.contains(&"mapinfo.lua".to_string()));
        assert!(names.contains(&"maps/test.smf".to_string()));
        let mut found = Vec::new();
        reader
            .for_each_entries(|entry, rd| {
                let mut buf = Vec::new();
                std::io::copy(rd, &mut buf)?;
                found.push((entry.name.clone(), buf.len()));
                Ok(true)
            })
            .unwrap();
        let smf = found.iter().find(|(n, _)| n == "maps/test.smf").unwrap();
        assert_eq!(smf.1, 4096);
        fs::remove_file(&path).unwrap();
    }
}
