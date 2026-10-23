//! Access to Hytale's `Assets.zip`.
//!
//! `Assets.zip` (about 3.5 GB, 60,000 entries) is an ordinary zip file. In the install checked while
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
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use anyhow::{anyhow, bail, Context, Result};
use zip::ZipArchive;

/// Where the launcher puts the game on macOS, relative to the home directory.
const MAC_INSTALL: &str =
    "Library/Application Support/Hytale/install/release/package/game/latest/Assets.zip";

/// Entries bigger than this are refused by [`Assets::read`] (the biggest in the install is 23 MB,
/// a prefab); use [`Assets::with_reader`] to stream anything larger.
const READ_LIMIT: u64 = 256 << 20;

pub struct Assets {
    path: PathBuf,
    zip: Mutex<ZipArchive<File>>,
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
        let i = self
            .lookup(name)
            .ok_or_else(|| anyhow!("{name}: not in Assets.zip"))?;
        if self.sizes[i] > READ_LIMIT {
            bail!("{name}: {} bytes is too big to read at once", self.sizes[i]);
        }
        let mut zip = self.zip.lock().unwrap();
        let mut f = zip.by_index(i).with_context(|| format!("opening {name}"))?;
        let mut buf = Vec::with_capacity(self.sizes[i] as usize);
        f.read_to_end(&mut buf)
            .with_context(|| format!("reading {name}"))?;
        Ok(buf)
    }

    /// Reads and parses one JSON entry.
    pub fn read_json<T: serde::de::DeserializeOwned>(&self, name: &str) -> Result<T> {
        let bytes = self.read(name)?;
        serde_json::from_slice(&bytes).with_context(|| format!("parsing {name}"))
    }

    /// Runs `f` on a streaming reader of one entry, without buffering the entry. The archive stays
    /// locked while `f` runs, so `f` must not call back into `self`.
    pub fn with_reader<T>(
        &self,
        name: &str,
        f: impl FnOnce(&mut dyn Read) -> Result<T>,
    ) -> Result<T> {
        let i = self
            .lookup(name)
            .ok_or_else(|| anyhow!("{name}: not in Assets.zip"))?;
        let mut zip = self.zip.lock().unwrap();
        let mut file = zip.by_index(i).with_context(|| format!("opening {name}"))?;
        f(&mut file)
    }
}
