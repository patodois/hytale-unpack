//! `check`: parse every `.blockymodel`, `.blockyanim` and prefab in the archive, one entry at a
//! time, and count what parses.
//!
//! Models and clips are read into a buffer (the biggest is under 1 MB). Prefabs are streamed from
//! the archive through [`crate::prefab::stream`] without buffering them, so the memory used does
//! not depend on the 23 MB prefabs. Entries are never kept after they are checked.

use std::collections::BTreeMap;
use std::time::Instant;

use anyhow::Result;

use crate::assets::Assets;
use crate::blockyanim::BlockyAnim;
use crate::blockymodel::BlockyModel;
use crate::prefab::{self, PrefabBlock, Visit};

#[derive(Clone, Debug, Default)]
pub struct Tally {
    pub ok: u64,
    pub failed: u64,
    /// Parsed, but with something suspicious (see `notes`)
    pub with_problems: u64,
}

#[derive(Clone, Debug, Default)]
pub struct Report {
    pub models: Tally,
    pub anims: Tally,
    pub prefabs: Tally,
    /// First failures, `path: error`
    pub failures: Vec<String>,
    pub notes: Vec<String>,
    pub nodes: u64,
    pub keyframes: u64,
    pub blocks: u64,
    pub fluids: u64,
    pub entities: u64,
    pub fillers: u64,
    pub rotations: BTreeMap<u32, u64>,
    pub prefab_versions: BTreeMap<String, u64>,
    pub peak_rss: u64,
    pub seconds: f64,
}

const MAX_LISTED: usize = 40;

impl Report {
    fn fail(&mut self, path: &str, e: impl std::fmt::Display) {
        if self.failures.len() < MAX_LISTED {
            self.failures.push(format!("{path}: {e}"));
        }
    }
    fn note(&mut self, path: &str, e: impl std::fmt::Display) {
        if self.notes.len() < MAX_LISTED {
            self.notes.push(format!("{path}: {e}"));
        }
    }
}

/// Peak resident set size of this process in bytes
pub fn peak_rss() -> u64 {
    #[cfg(unix)]
    {
        let mut ru: libc::rusage = unsafe { std::mem::zeroed() };
        if unsafe { libc::getrusage(libc::RUSAGE_SELF, &mut ru) } == 0 {
            let v = ru.ru_maxrss as u64;
            // macOS reports bytes, Linux kilobytes
            return if cfg!(target_os = "macos") {
                v
            } else {
                v * 1024
            };
        }
    }
    0
}

struct Counter<'a> {
    report: &'a mut Report,
}

impl Visit for Counter<'_> {
    fn block(&mut self, b: &PrefabBlock) {
        if let Some(r) = b.rotation {
            *self.report.rotations.entry(r).or_default() += 1;
        }
        if b.filler.unwrap_or(0) != 0 {
            self.report.fillers += 1;
        }
    }
}

/// Runs the check. `progress` is called with (category, done so far) every few hundred entries.
pub fn run(assets: &Assets, mut progress: impl FnMut(&str, u64)) -> Result<Report> {
    let start = Instant::now();
    let mut r = Report::default();
    let mut names: Vec<&str> = assets
        .names()
        .filter(|n| {
            n.ends_with(".blockymodel")
                || n.ends_with(".blockyanim")
                || (n.starts_with("Server/Prefabs/") && n.ends_with(".prefab.json"))
        })
        .collect();
    names.sort();
    for (i, name) in names.iter().enumerate() {
        if i % 500 == 0 {
            progress(name, i as u64);
        }
        if name.ends_with(".blockymodel") {
            match assets.read(name).and_then(|b| BlockyModel::parse(&b)) {
                Ok(m) => {
                    r.nodes += m.flatten().len() as u64;
                    r.models.ok += 1;
                    let p = m.problems();
                    if !p.is_empty() {
                        r.models.with_problems += 1;
                        r.note(name, p.join("; "));
                    }
                }
                Err(e) => {
                    r.models.failed += 1;
                    r.fail(name, format!("{e:#}"));
                }
            }
        } else if name.ends_with(".blockyanim") {
            match assets.read(name).and_then(|b| BlockyAnim::parse(&b)) {
                Ok(a) => {
                    r.keyframes += a.key_count() as u64;
                    r.anims.ok += 1;
                    let p = a.problems();
                    if !p.is_empty() {
                        r.anims.with_problems += 1;
                        r.note(name, p.join("; "));
                    }
                }
                Err(e) => {
                    r.anims.failed += 1;
                    r.fail(name, format!("{e:#}"));
                }
            }
        } else {
            let mut sub = Report::default();
            let res = assets.with_reader(name, |rd| {
                let mut c = Counter { report: &mut sub };
                prefab::stream(rd, &mut c)
            });
            match res {
                Ok((header, counts)) => {
                    r.prefabs.ok += 1;
                    r.blocks += counts.blocks;
                    r.fluids += counts.fluids;
                    r.entities += counts.entities;
                    r.fillers += sub.fillers;
                    for (k, v) in sub.rotations {
                        *r.rotations.entry(k).or_default() += v;
                    }
                    *r.prefab_versions
                        .entry(format!(
                            "{:?}/{:?}",
                            header.version, header.block_id_version
                        ))
                        .or_default() += 1;
                }
                Err(e) => {
                    r.prefabs.failed += 1;
                    r.fail(name, format!("{e:#}"));
                }
            }
        }
    }
    r.peak_rss = peak_rss();
    r.seconds = start.elapsed().as_secs_f64();
    Ok(r)
}
