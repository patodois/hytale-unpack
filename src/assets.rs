//! Access to Hytale's `Assets.zip`.
//!
//! `Assets.zip` (about 3.9 GB, 69,000 entries) is an ordinary zip file. In the install checked while
//! writing this crate every entry is *stored* (compression method 0), and the entries are flat
//! paths such as `Common/Characters/Player.blockymodel` or `Server/Models/Human/Player.json`.
//!
//! [`Assets::open`] reads the central directory once (a few megabytes at the end of the file) and
//! builds a name index. After that every read seeks to one entry and returns just its bytes, so
//! the archive is never loaded whole. Reads are serialised through a mutex because a zip reader
//! has one file cursor.
//!
//! Top-level folders, as found in the install:
//!
//! | folder | holds |
//! |--------|-------|
//! | `Common/` | what the client needs: `Blocks`, `Characters`, `Cosmetics`, `Items`, `NPC`, `Icons`, `Sounds`, textures, particles |
//! | `Server/` | game logic as JSON: `Models` (entity model definitions), `Item`, `NPC`, `Prefabs`, `World`, `Particles`, `Audio`, ... |
//! | `Cosmetics/` | the character creator's data |
//!
//! Paths inside JSON files (`"Model": "Characters/Player.blockymodel"`, `"Texture": ...`) are
//! relative to `Common/`; [`Assets::common`] joins them.

use std::collections::HashMap;
use std::fs::File;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use anyhow::{anyhow, bail, Context, Result};
use zip::{CompressionMethod, ZipArchive};

/// Where the launcher puts the game on macOS, relative to the home directory.
const MAC_INSTALL: &str =
    "Library/Application Support/Hytale/install/release/package/game/latest/Assets.zip";

/// No read hands out more than this many bytes of one entry: [`Assets::read`] refuses a bigger
/// entry, and the reader of [`Assets::with_reader`] fails once more has come out. The biggest
/// entry in the install is 24 MB, a prefab, so the limit is ten times what the game ships.
const READ_LIMIT: u64 = 256 << 20;

pub struct Assets {
    path: PathBuf,
    zip: Mutex<ZipArchive<File>>,
    /// Length of the archive file: no entry can store more bytes than the file holds
    file_len: u64,
    names: Vec<String>,
    sizes: Vec<u64>,
    index: HashMap<String, usize>,
    /// Lower-case name -> index, for references whose case differs from the file
    folded: HashMap<String, usize>,
}

impl Assets {
    /// The default `Assets.zip` location: `$HYTALE_ASSETS` if set, else the macOS launcher path.
    pub fn default_path() -> PathBuf {
        if let Some(p) = std::env::var_os("HYTALE_ASSETS") {
            return PathBuf::from(p);
        }
        let home = std::env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_default();
        home.join(MAC_INSTALL)
    }

    /// Opens the archive and reads its central directory.
    pub fn open(path: &Path) -> Result<Self> {
        let file = File::open(path).with_context(|| {
            format!(
                "opening {} (set HYTALE_ASSETS or pass --assets <path to Assets.zip>)",
                path.display()
            )
        })?;
        let file_len = file.metadata()?.len();
        let mut zip = ZipArchive::new(file)
            .with_context(|| format!("reading the central directory of {}", path.display()))?;
        let n = zip.len();
        let mut names = Vec::with_capacity(n);
        let mut sizes = Vec::with_capacity(n);
        let mut index = HashMap::with_capacity(n);
        let mut folded = HashMap::with_capacity(n);
        for i in 0..n {
            let f = zip.by_index_raw(i)?;
            let name = f.name().to_string();
            sizes.push(f.size());
            if !f.is_dir() {
                index.insert(name.clone(), i);
                folded.entry(name.to_ascii_lowercase()).or_insert(i);
            }
            names.push(name);
        }
        Ok(Assets {
            path: path.to_path_buf(),
            zip: Mutex::new(zip),
            file_len,
            names,
            sizes,
            index,
            folded,
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Every file entry name, in archive order.
    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.names
            .iter()
            .enumerate()
            .filter(|(i, n)| self.index.get(*n) == Some(i))
            .map(|(_, n)| n.as_str())
    }

    /// File entries under a folder prefix such as `"Common/Blocks/"`.
    pub fn names_under<'a>(&'a self, prefix: &'a str) -> impl Iterator<Item = &'a str> {
        self.names().filter(move |n| n.starts_with(prefix))
    }

    pub fn contains(&self, name: &str) -> bool {
        self.lookup(name).is_some()
    }

    /// The real entry name for `name`, matching exactly or else ignoring ASCII case.
    pub fn resolve(&self, name: &str) -> Option<&str> {
        self.lookup(name).map(|i| self.names[i].as_str())
    }

    /// Uncompressed size of an entry.
    pub fn size(&self, name: &str) -> Option<u64> {
        self.lookup(name).map(|i| self.sizes[i])
    }

    /// `Common/` + a path as written inside Hytale's JSON files.
    pub fn common(path: &str) -> String {
        format!("Common/{}", path.trim_start_matches('/'))
    }

    fn lookup(&self, name: &str) -> Option<usize> {
        self.index
            .get(name)
            .copied()
            .or_else(|| self.folded.get(&name.to_ascii_lowercase()).copied())
    }

    /// The bytes of one entry.
    pub fn read(&self, name: &str) -> Result<Vec<u8>> {
        self.read_limited(name, READ_LIMIT)
    }

    fn read_limited(&self, name: &str, limit: u64) -> Result<Vec<u8>> {
        let i = self
            .lookup(name)
            .ok_or_else(|| anyhow!("{name}: not in Assets.zip"))?;
        if self.sizes[i] > limit {
            bail!("{name}: {} bytes is too big to read at once", self.sizes[i]);
        }
        let mut zip = self.zip.lock().unwrap();
        let f = zip.by_index(i).with_context(|| format!("opening {name}"))?;
        // The sizes in the headers are only claims. The stored bytes of an entry cannot be more
        // than the file holds after the start of its data, and a DEFLATE stream expands by at
        // most 1032 times (see `image::decodable_from`), a stored one not at all: reserve no more
        // than that, and let the vector grow if the entry really is that big
        let stored = self.file_len.saturating_sub(f.data_start().unwrap_or(0));
        let ratio = if f.compression() == CompressionMethod::Stored {
            1
        } else {
            1032
        };
        let reserve = self.sizes[i].min(f.compressed_size().min(stored).saturating_mul(ratio));
        let mut buf = Vec::with_capacity(reserve as usize);
        read_bounded(f, limit, &mut buf).with_context(|| format!("reading {name}"))?;
        if buf.len() as u64 > limit {
            bail!("{name}: entry expands beyond {limit} bytes");
        }
        Ok(buf)
    }

    /// Reads and parses one JSON entry.
    pub fn read_json<T: serde::de::DeserializeOwned>(&self, name: &str) -> Result<T> {
        let bytes = self.read(name)?;
        serde_json::from_slice(&bytes).with_context(|| format!("parsing {name}"))
    }

    /// Runs `f` on a streaming reader of one entry, without buffering the entry. The archive stays
    /// locked while `f` runs, so `f` must not call back into `self`.
    ///
    /// The reader is bounded like [`Assets::read`]: an entry that claims more than 256 MiB is
    /// refused before `f` runs, and a read fails as soon as more than that has actually come
    /// out, whatever the archive claims (a compressed entry can expand far beyond its claim).
    pub fn with_reader<T>(
        &self,
        name: &str,
        f: impl FnOnce(&mut dyn Read) -> Result<T>,
    ) -> Result<T> {
        self.with_reader_limited(name, READ_LIMIT, f)
    }

    fn with_reader_limited<T>(
        &self,
        name: &str,
        limit: u64,
        f: impl FnOnce(&mut dyn Read) -> Result<T>,
    ) -> Result<T> {
        let i = self
            .lookup(name)
            .ok_or_else(|| anyhow!("{name}: not in Assets.zip"))?;
        if self.sizes[i] > limit {
            bail!("{name}: {} bytes is too big to read", self.sizes[i]);
        }
        let mut zip = self.zip.lock().unwrap();
        let file = zip.by_index(i).with_context(|| format!("opening {name}"))?;
        let mut bounded = Bounded {
            inner: file,
            left: limit,
            limit,
            name,
            over: false,
        };
        f(&mut bounded)
    }
}

/// A reader that hands out at most `limit` bytes of `inner`. One byte more is an error, not an end
/// of file: a cut entry must not look like a whole one to the parser that reads it.
struct Bounded<'a, R> {
    inner: R,
    /// Bytes that may still be handed out
    left: u64,
    limit: u64,
    name: &'a str,
    over: bool,
}

impl<R: Read> Read for Bounded<'_, R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        if !self.over {
            // Ask for one byte more than is left, so that an entry of exactly the limit ends in
            // an ordinary end of file and only a longer one is refused
            let ask = usize::try_from(self.left.saturating_add(1))
                .unwrap_or(usize::MAX)
                .min(buf.len());
            let n = self.inner.read(&mut buf[..ask])?;
            if n as u64 <= self.left {
                self.left -= n as u64;
                return Ok(n);
            }
            // The entry is longer than the limit: hand over what is still allowed, then fail
            self.over = true;
            let allowed = std::mem::take(&mut self.left) as usize;
            if allowed > 0 {
                return Ok(allowed);
            }
        }
        Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("{}: entry expands beyond {} bytes", self.name, self.limit),
        ))
    }
}

#[cfg(test)]
mod read_limit_regression {
    use super::*;
    #[test]
    fn actual_zip_output_obeys_the_read_limit() {
        use std::io::{Cursor, Write};
        let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
        zip.start_file(
            "Common/test",
            zip::write::SimpleFileOptions::default()
                .compression_method(CompressionMethod::Deflated),
        )
        .unwrap();
        zip.write_all(&[b'A'; 8192]).unwrap();
        let mut bytes = zip.finish().unwrap().into_inner();
        let central = bytes.windows(4).position(|s| s == b"PK\x01\x02").unwrap();
        bytes[central + 24..central + 28].copy_from_slice(&1u32.to_le_bytes());
        bytes[22..26].copy_from_slice(&1u32.to_le_bytes());
        let path = std::env::temp_dir().join(format!("hytale-limit-{}.zip", std::process::id()));
        std::fs::write(&path, bytes).unwrap();
        let a = Assets::open(&path).unwrap();
        assert_eq!(a.size("Common/test"), Some(1));
        assert!(a.read_limited("Common/test", 1024).is_err());
        drop(a);
        std::fs::remove_file(path).unwrap();
    }
}

fn read_bounded(mut reader: impl Read, limit: u64, buf: &mut Vec<u8>) -> std::io::Result<usize> {
    reader.by_ref().take(limit + 1).read_to_end(buf)
}
#[cfg(test)]
mod bounded_read_consumption {
    use super::*;
    #[test]
    fn read_limit_stops_consumption_before_the_post_read_check() {
        let mut source = std::io::Cursor::new([42; 8192]);
        let mut out = Vec::new();
        read_bounded(&mut source, 1024, &mut out).unwrap();
        assert_eq!(source.position(), 1025);
        assert_eq!(out.len(), 1025);
    }
}

#[cfg(test)]
mod streaming_read_limit {
    use super::*;
    use std::io::{Cursor, Write};

    /// An archive of one DEFLATE entry `Server/Prefabs/a.prefab.json` holding `content`, whose
    /// headers claim `claimed` bytes for it
    fn archive(content: &[u8], claimed: u32, tag: &str) -> (Assets, PathBuf) {
        let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
        zip.start_file(
            "Server/Prefabs/a.prefab.json",
            zip::write::SimpleFileOptions::default()
                .compression_method(CompressionMethod::Deflated),
        )
        .unwrap();
        zip.write_all(content).unwrap();
        let mut bytes = zip.finish().unwrap().into_inner();
        let local = bytes.windows(4).position(|s| s == b"PK\x03\x04").unwrap();
        let central = bytes.windows(4).position(|s| s == b"PK\x01\x02").unwrap();
        bytes[local + 22..local + 26].copy_from_slice(&claimed.to_le_bytes());
        bytes[central + 24..central + 28].copy_from_slice(&claimed.to_le_bytes());
        let path = std::env::temp_dir().join(format!(
            "hytale-stream-limit-{tag}-{}.zip",
            std::process::id()
        ));
        std::fs::write(&path, bytes).unwrap();
        (Assets::open(&path).unwrap(), path)
    }

    /// A prefab of `n` bytes: the padding is white space, which a JSON parser skips
    fn prefab(n: usize) -> Vec<u8> {
        let mut doc = br#"{"version":8,"blocks":[{"x":0,"y":0,"z":0,"name":"A"}]"#.to_vec();
        doc.resize(n - 1, b' ');
        doc.push(b'}');
        doc
    }

    #[test]
    fn the_streaming_reader_stops_at_the_limit_whatever_the_entry_claims() {
        // 8192 bytes come out of an entry that claims to hold one
        let (a, path) = archive(&prefab(8192), 1, "over");
        assert_eq!(a.size("Server/Prefabs/a.prefab.json"), Some(1));
        let mut seen = 0u64;
        let err = a
            .with_reader_limited("Server/Prefabs/a.prefab.json", 1024, |r| {
                let mut buf = [0u8; 300];
                loop {
                    match r.read(&mut buf) {
                        Ok(0) => return Ok(()),
                        Ok(n) => seen += n as u64,
                        Err(e) => return Err(e.into()),
                    }
                }
            })
            .unwrap_err();
        assert!(
            format!("{err:#}").contains("expands beyond 1024 bytes"),
            "{err:#}"
        );
        // nothing past the limit was handed over, and the reader stays failed
        assert_eq!(seen, 1024);
        // the prefab parser gets the same error instead of a document cut short
        struct Nop;
        impl crate::prefab::Visit for Nop {}
        let err = a
            .with_reader_limited("Server/Prefabs/a.prefab.json", 1024, |r| {
                crate::prefab::stream(r, &mut Nop)
            })
            .unwrap_err();
        assert!(format!("{err:#}").contains("expands beyond"), "{err:#}");
        drop(a);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn an_entry_of_exactly_the_limit_streams_whole() {
        let doc = prefab(8192);
        let (a, path) = archive(&doc, 8192, "exact");
        let name = "Server/Prefabs/a.prefab.json";
        let read_all = |limit| {
            a.with_reader_limited(name, limit, |r| {
                let mut out = Vec::new();
                r.read_to_end(&mut out)?;
                Ok(out)
            })
        };
        assert_eq!(read_all(8192).unwrap(), doc);
        assert_eq!(read_all(READ_LIMIT).unwrap(), doc);
        assert!(read_all(8191).is_err());
        // and it parses through the public entry point as before
        struct Count(u64);
        impl crate::prefab::Visit for Count {
            fn block(&mut self, _b: &crate::prefab::PrefabBlock) {
                self.0 += 1;
            }
        }
        let mut c = Count(0);
        a.with_reader(name, |r| crate::prefab::stream(r, &mut c))
            .unwrap();
        assert_eq!(c.0, 1);
        drop(a);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn an_entry_that_claims_more_than_the_limit_is_refused_before_it_is_read() {
        let (a, path) = archive(&prefab(64), 2000, "claim");
        let mut called = false;
        let err = a
            .with_reader_limited("Server/Prefabs/a.prefab.json", 1024, |_| {
                called = true;
                Ok(())
            })
            .unwrap_err();
        assert!(!called);
        assert!(format!("{err:#}").contains("too big to read"), "{err:#}");
        drop(a);
        std::fs::remove_file(path).unwrap();
    }
}
