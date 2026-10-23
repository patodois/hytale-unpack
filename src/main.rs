use std::collections::HashMap;
use std::path::PathBuf;

use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand};

use hytale_unpack::blockyanim::{BlockyAnim, FPS};
use hytale_unpack::prefab::{self, PrefabBlock, Visit};
use hytale_unpack::{check, Assets};

#[derive(Parser)]
#[command(
    name = "hytale-unpack",
    version,
    about = "Reader for Hytale's Assets.zip (models, animations, prefabs)"
)]
struct Cli {
    /// Path to Assets.zip (default: $HYTALE_ASSETS, else the macOS launcher's install)
    #[arg(long, global = true)]
    assets: Option<PathBuf>,
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// List models, entity model definitions, block items and prefabs whose id contains FILTER
    List {
        filter: Option<String>,
        /// List raw archive entries instead of provider assets
        #[arg(long)]
        raw: bool,
    },
    /// Print a model (or entity model definition, or block item), or export it to glTF with -o
    Model {
        id: String,
        #[arg(short, long)]
        out: Option<PathBuf>,
        /// Leave the animation clips out of the glTF
        #[arg(long)]
        no_anim: bool,
        /// Only add clips whose name contains this text
        #[arg(long)]
        clips: Option<String>,
    },
    /// Print an animation clip
    Anim {
        path: String,
        /// Also list the keyframes
        #[arg(long)]
        keys: bool,
    },
    /// Print a prefab's blocks and entities, or export it to glTF with -o
    Prefab {
        path: String,
        #[arg(short, long)]
        out: Option<PathBuf>,
        /// How many entities to print
        #[arg(long, default_value_t = 20)]
        entities: usize,
        /// How many block names to print
        #[arg(long, default_value_t = 25)]
        top: usize,
    },
    /// Parse every model, clip and prefab, streaming, and report counts and peak memory
    Check,
    /// Load every entity model, model and block through the provider and validate it; with
    /// --scenes N also every prefab below N MB
    CheckBridge {
        #[arg(long)]
        scenes: Option<f64>,
    },
    /// Check what the provider returns against the rewrite-bridge conventions (scale, facing,
    /// winding, skins, clips ...); try `check-conformance --help`
    #[command(disable_help_flag = true)]
    CheckConformance {
        /// Options of the checker, passed through (`--id`, `--filter`, `--kind`, `--limit`,
        /// `--strict`, `-v`, `--help`)
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
}

fn find(assets: &Assets, id: &str, prefixes: &[&str], suffixes: &[&str]) -> Result<String> {
    for p in prefixes {
        for s in suffixes {
            let cand = format!("{p}{id}{s}");
            if let Some(real) = assets.resolve(&cand) {
                return Ok(real.to_string());
            }
        }
    }
    bail!(
        "{id}: not found in Assets.zip (tried {} prefix and suffix combinations)",
        prefixes.len() * suffixes.len()
    )
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let path = cli.assets.clone().unwrap_or_else(Assets::default_path);
    match cli.cmd {
        Cmd::Check => cmd_check(&path),
        #[cfg(not(feature = "bridge"))]
        Cmd::CheckBridge { .. } | Cmd::CheckConformance { .. } => {
            bail!("this command needs the `bridge` feature")
        }
        Cmd::Anim { path: p, keys } => {
            let assets = Assets::open(&path)?;
            let id = find(
                &assets,
                &p,
                &["", "Common/", "Common/Characters/Animations/"],
                &["", ".blockyanim"],
            )?;
            cmd_anim(&assets, &id, keys)
        }
        cmd => {
            #[cfg(feature = "bridge")]
            {
                run_bridge(cmd, &path)
            }
            #[cfg(not(feature = "bridge"))]
            {
                match cmd {
                    Cmd::List { filter, .. } => {
                        let assets = Assets::open(&path)?;
                        let f = filter.unwrap_or_default();
                        for n in assets.names().filter(|n| n.contains(&f)) {
                            println!("{n}");
                        }
                        Ok(())
                    }
                    Cmd::Prefab {
                        path: p,
                        entities,
                        top,
                        ..
                    } => {
                        let assets = Assets::open(&path)?;
                        let id = find(
                            &assets,
                            &p,
                            &["", "Server/Prefabs/"],
                            &["", ".prefab.json", ".json"],
                        )?;
                        cmd_prefab(&assets, &id, entities, top)
                    }
                    _ => bail!("this command needs the `bridge` feature"),
                }
            }
        }
    }
}

fn cmd_check(path: &std::path::Path) -> Result<()> {
    let assets = Assets::open(path)?;
    let report = check::run(&assets, |name, done| eprintln!("  {done:>6} {name}"))?;
    let t = |name: &str, t: &check::Tally| {
        println!(
            "{name:<10} ok {:>6}  failed {:>4}  with notes {:>4}",
            t.ok, t.failed, t.with_problems
        )
    };
    t("models", &report.models);
    t("anims", &report.anims);
    t("prefabs", &report.prefabs);
    println!(
        "{} nodes, {} keyframes, {} blocks ({} filler cells), {} fluids, {} entities",
        report.nodes,
        report.keyframes,
        report.blocks,
        report.fillers,
        report.fluids,
        report.entities
    );
    println!("block rotation values: {:?}", report.rotations);
    println!(
        "prefab version/blockIdVersion: {:?}",
        report.prefab_versions
    );
    for f in &report.failures {
        println!("FAILED {f}");
    }
    for n in &report.notes {
        println!("note {n}");
    }
    println!(
        "peak RSS {:.1} MB, {:.1} s",
        report.peak_rss as f64 / 1e6,
        report.seconds
    );
    let failed = report.models.failed + report.anims.failed + report.prefabs.failed;
    if failed > 0 {
        bail!("{failed} entries failed");
    }
    Ok(())
}

fn cmd_anim(assets: &Assets, id: &str, keys: bool) -> Result<()> {
    let a = BlockyAnim::parse(&assets.read(id)?).with_context(|| id.to_string())?;
    println!(
        "{id}: {} frames = {:.3} s at {FPS} fps, {}, {} nodes, {} keyframes",
        a.duration,
        a.duration_s(),
        if a.hold_last_keyframe {
            "holds its last keyframe"
        } else {
            "loops"
        },
        a.node_animations.len(),
        a.key_count()
    );
    for (name, n) in &a.node_animations {
        let smooth =
            |v: &[bool]| format!("{}/{} smooth", v.iter().filter(|s| **s).count(), v.len());
        let mut parts = Vec::new();
        if !n.position.is_empty() {
            parts.push(format!(
                "position {} ({})",
                n.position.len(),
                smooth(&n.position.iter().map(|k| k.smooth()).collect::<Vec<_>>())
            ));
        }
        if !n.orientation.is_empty() {
            parts.push(format!(
                "orientation {} ({})",
                n.orientation.len(),
                smooth(&n.orientation.iter().map(|k| k.smooth()).collect::<Vec<_>>())
            ));
        }
        if !n.shape_stretch.is_empty() {
            parts.push(format!("shapeStretch {}", n.shape_stretch.len()));
        }
        if !n.shape_visible.is_empty() {
            parts.push(format!("shapeVisible {}", n.shape_visible.len()));
        }
        if !n.shape_uv_offset.is_empty() {
            parts.push(format!("shapeUvOffset {}", n.shape_uv_offset.len()));
        }
        println!("  {name}: {}", parts.join(", "));
        if keys {
            for k in &n.position {
                println!(
                    "    position    t={:<6} ({:.4}, {:.4}, {:.4}) {}",
                    k.time,
                    k.delta.x,
                    k.delta.y,
                    k.delta.z,
                    k.interpolation_type.as_deref().unwrap_or("linear")
                );
            }
            for k in &n.orientation {
                println!(
                    "    orientation t={:<6} ({:.4}, {:.4}, {:.4}, {:.4}) {}",
                    k.time,
                    k.delta.x,
                    k.delta.y,
                    k.delta.z,
                    k.delta.w,
                    k.interpolation_type.as_deref().unwrap_or("linear")
                );
            }
            for k in &n.shape_stretch {
                println!(
                    "    stretch     t={:<6} ({:.4}, {:.4}, {:.4}) {}",
                    k.time,
                    k.delta.x,
                    k.delta.y,
                    k.delta.z,
                    k.interpolation_type.as_deref().unwrap_or("linear")
                );
            }
            for k in &n.shape_visible {
                println!("    visible     t={:<6} {}", k.time, k.delta);
            }
            for k in &n.shape_uv_offset {
                println!(
                    "    uvOffset    t={:<6} ({}, {})",
                    k.time, k.delta.x, k.delta.y
                );
            }
        }
    }
    Ok(())
}

fn cmd_prefab(assets: &Assets, id: &str, entities: usize, top: usize) -> Result<()> {
    struct Stats {
        names: HashMap<String, u64>,
        lo: [i32; 3],
        hi: [i32; 3],
        rotated: u64,
        fillers: u64,
        fluids: HashMap<String, u64>,
        ents: Vec<serde_json::Value>,
        want: usize,
    }
    impl Visit for Stats {
        fn block(&mut self, b: &PrefabBlock) {
            *self.names.entry(b.name.clone()).or_default() += 1;
            for (k, v) in [b.x, b.y, b.z].into_iter().enumerate() {
                self.lo[k] = self.lo[k].min(v);
                self.hi[k] = self.hi[k].max(v);
            }
            self.rotated += b.rotation.is_some() as u64;
            self.fillers += b.filler.is_some_and(|f| f != 0) as u64;
        }
        fn fluid(&mut self, f: &prefab::PrefabFluid) {
            *self.fluids.entry(f.name.clone()).or_default() += 1;
        }
        fn entity(&mut self, e: &serde_json::Value) {
            if self.ents.len() < self.want {
                self.ents.push(e.clone());
            }
        }
        fn wants_entities(&self) -> bool {
            self.want > 0
        }
    }
    let mut s = Stats {
        names: HashMap::new(),
        lo: [i32::MAX; 3],
        hi: [i32::MIN; 3],
        rotated: 0,
        fillers: 0,
        fluids: HashMap::new(),
        ents: vec![],
        want: entities,
    };
    let (header, counts) = assets.with_reader(id, |r| prefab::stream(r, &mut s))?;
    println!("{id}");
    println!(
        "version {:?}, blockIdVersion {:?}, anchor {:?}",
        header.version, header.block_id_version, header.anchor
    );
    println!(
        "{} blocks ({} rotated, {} filler cells, {} distinct names), {} fluid cells, {} entities",
        counts.blocks,
        s.rotated,
        s.fillers,
        s.names.len(),
        counts.fluids,
        counts.entities
    );
    if counts.blocks > 0 {
        println!("cells from {:?} to {:?}", s.lo, s.hi);
    }
    let mut names: Vec<_> = s.names.iter().collect();
    names.sort_by(|a, b| b.1.cmp(a.1).then(a.0.cmp(b.0)));
    for (n, c) in names.iter().take(top) {
        println!("  {c:>7}  {n}");
    }
    if names.len() > top {
        println!("  ... and {} more names", names.len() - top);
    }
    for (n, c) in &s.fluids {
        println!("  fluid {n}: {c} cells");
    }
    for e in &s.ents {
        let c = e.get("Components").unwrap_or(e);
        let model = c
            .pointer("/Model/Model/Id")
            .and_then(|v| v.as_str())
            .unwrap_or("-");
        let nameplate = c
            .pointer("/Nameplate/Text")
            .and_then(|v| v.as_str())
            .unwrap_or("-");
        let kinds: Vec<&str> = c
            .as_object()
            .map(|o| o.keys().map(|k| k.as_str()).collect())
            .unwrap_or_default();
        let pos = c.pointer("/Transform/Position").map(|p| {
            format!(
                "{:.2},{:.2},{:.2}",
                p["X"].as_f64().unwrap_or(0.0),
                p["Y"].as_f64().unwrap_or(0.0),
                p["Z"].as_f64().unwrap_or(0.0)
            )
        });
        println!(
            "  entity model {model} name {nameplate} at {} [{}]",
            pos.unwrap_or_else(|| "-".into()),
            kinds.join(" ")
        );
    }
    Ok(())
}

#[cfg(feature = "bridge")]
fn run_bridge(cmd: Cmd, path: &std::path::Path) -> Result<()> {
    use hytale_unpack::bridge::Hytale;
    use hytale_unpack::gltf;
    use rewrite_bridge::Provider;

    let hy = Hytale::open(path)?;
    match cmd {
        Cmd::List { filter, raw } => {
            let f = filter.unwrap_or_default();
            if raw {
                for n in hy.assets().names().filter(|n| n.contains(&f)) {
                    println!("{n}");
                }
            } else {
                for a in Provider::list(&hy, &f).map_err(|e| anyhow::anyhow!("{e}"))? {
                    println!("{:<10} {}", format!("{:?}", a.kind), a.id);
                }
            }
            Ok(())
        }
        Cmd::Model {
            id,
            out,
            no_anim,
            clips,
        } => {
            let id = match hy
                .defs()
                .item_path(&id)
                .or_else(|| hy.defs().model_path(&id))
            {
                Some(p) => p.to_string(),
                None => find(
                    hy.assets(),
                    &id,
                    &["", "Server/Models/", "Common/", "Server/Item/Items/"],
                    &["", ".json", ".blockymodel"],
                )?,
            };
            let (mut am, report) = hy.load_animated_report(&id)?;
            if let Some(c) = &clips {
                am.animations.retain(|a| a.name.contains(c.as_str()));
            }
            if no_anim {
                am.animations.clear();
            }
            let m = &am.model;
            let tris: usize = m.meshes.iter().map(|x| x.indices.len() / 3).sum();
            println!("{id}");
            println!(
                "  {} nodes, {} meshes ({} triangles), {} materials",
                m.nodes.len(),
                m.meshes.len(),
                tris,
                m.materials.len()
            );
            {
                // bounds of the rest pose, in model space
                let world = gltf::world_matrices(m);
                let (mut lo, mut hi) = ([f32::MAX; 3], [f32::MIN; 3]);
                for mesh in &m.meshes {
                    let w = &world[mesh.node];
                    for p in &mesh.positions {
                        for k in 0..3 {
                            let v = w[k] * p[0] + w[4 + k] * p[1] + w[8 + k] * p[2] + w[12 + k];
                            lo[k] = lo[k].min(v);
                            hi[k] = hi[k].max(v);
                        }
                    }
                }
                if lo[0] <= hi[0] {
                    println!("  rest-pose bounds {lo:.3?} to {hi:.3?} m");
                }
            }
            println!(
                "  texture: {}",
                report.texture.as_deref().unwrap_or("none found")
            );
            println!(
                "  {} clips{}",
                am.animations.len(),
                if no_anim { " (left out)" } else { "" }
            );
            for a in am.animations.iter().take(12) {
                println!(
                    "    {:<28} {:.3} s, {} channels",
                    a.name,
                    a.duration_s,
                    a.channels.len()
                );
            }
            if am.animations.len() > 12 {
                println!("    ... and {} more", am.animations.len() - 12);
            }
            if report.dropped_visible_keys + report.dropped_uv_keys > 0 {
                println!(
                    "  left out of clips: {} shapeVisible keys, {} shapeUvOffset keys",
                    report.dropped_visible_keys, report.dropped_uv_keys
                );
            }
            if !report.missing_nodes.is_empty() {
                println!(
                    "  clip nodes the model lacks: {}",
                    report.missing_nodes.join(", ")
                );
            }
            if !report.attachments_not_composed.is_empty() {
                println!(
                    "  default attachments not composed: {}",
                    report.attachments_not_composed.join(", ")
                );
            }
            if let Some(dir) = out {
                let name = id
                    .rsplit('/')
                    .next()
                    .unwrap()
                    .split('.')
                    .next()
                    .unwrap()
                    .to_string();
                let gltf_path = gltf::write_animated(&am, &name, &dir)?;
                println!("  wrote {}", gltf_path.display());
            }
            Ok(())
        }
        Cmd::Prefab {
            path: p,
            out,
            entities,
            top,
        } => {
            let id = find(
                hy.assets(),
                &p,
                &["", "Server/Prefabs/"],
                &["", ".prefab.json", ".json"],
            )?;
            cmd_prefab(hy.assets(), &id, entities, top)?;
            if let Some(dir) = out {
                let (scene, report) = hy.load_scene_report(&id)?;
                println!(
                    "scene: {} models, {} instances ({} empty cells and {} filler cells skipped)",
                    scene.models.len(),
                    scene.instances.len(),
                    report.empty,
                    report.fillers
                );
                if !report.unresolved.is_empty() {
                    let n: u64 = report.unresolved.values().sum();
                    println!(
                        "  {} cells of {} names matched no block:",
                        n,
                        report.unresolved.len()
                    );
                    for (name, c) in report.unresolved.iter().take(10) {
                        println!("    {c:>6}  {name}");
                    }
                }
                let gltf_path = gltf::write_scene(&scene, &scene.name, &dir)?;
                println!("wrote {}", gltf_path.display());
            }
            Ok(())
        }
        Cmd::CheckBridge { scenes } => cmd_check_bridge(&hy, scenes),
        Cmd::CheckConformance { args } => {
            // exits with the checker's status: 0, 1 (an asset failed) or 2 (usage)
            std::process::exit(rewrite_bridge::conformance::cli::run(&hy, args))
        }
        Cmd::Anim { .. } | Cmd::Check => unreachable!(),
    }
}

#[cfg(feature = "bridge")]
fn cmd_check_bridge(hy: &hytale_unpack::bridge::Hytale, scenes: Option<f64>) -> Result<()> {
    use rewrite_bridge::Provider;
    let start = std::time::Instant::now();
    let mut ok = [0u64; 4];
    let mut failed = [0u64; 4];
    let mut fails: Vec<String> = Vec::new();
    let mut clips = 0usize;
    let mut instances = 0u64;
    let mut unresolved = std::collections::BTreeMap::<String, u64>::new();
    let names: Vec<String> = hy.assets().names().map(String::from).collect();
    let mut n = 0;
    for id in &names {
        let kind = if id.starts_with("Server/Models/") && id.ends_with(".json") {
            // definitions without a Model (3D-less showcase entries) have nothing to load
            if hy
                .defs()
                .model_def(hy.assets(), id)
                .map(|d| d.model.is_none())
                .unwrap_or(false)
            {
                continue;
            }
            0
        } else if id.starts_with("Common/") && id.ends_with(".blockymodel") {
            1
        } else if id.starts_with("Server/Item/Items/") && id.ends_with(".json") {
            if !hy.is_block_item(id) {
                continue;
            }
            2
        } else if id.starts_with("Server/Prefabs/") && id.ends_with(".prefab.json") {
            match scenes {
                Some(mb) if (hy.assets().size(id).unwrap_or(0) as f64) < mb * 1e6 => 3,
                _ => continue,
            }
        } else {
            continue;
        };
        n += 1;
        if n % 40 == 0 {
            hy.clear_cache();
        }
        let res: Result<()> = if kind == 3 {
            hy.load_scene_report(id).map(|(s, r)| {
                instances += s.instances.len() as u64;
                for (k, v) in r.unresolved {
                    *unresolved.entry(k).or_default() += v;
                }
            })
        } else if kind == 2 {
            // a block that draws nothing (DrawType Empty and the like) is not a failure
            match hy.block_model(id) {
                Ok(_) => Ok(()),
                Err(e) => Err(e),
            }
        } else {
            hy.load_animated_report(id).and_then(|(am, r)| {
                clips += r.clips;
                Ok(am.validate()?)
            })
        };
        match res {
            Ok(()) => ok[kind] += 1,
            Err(e) => {
                failed[kind] += 1;
                if fails.len() < 40 {
                    fails.push(format!("{id}: {e:#}"));
                }
            }
        }
    }
    for (i, label) in [
        "model definitions",
        "bare models",
        "block items",
        "prefab scenes",
    ]
    .iter()
    .enumerate()
    {
        println!("{label:<18} ok {:>6}  failed {:>5}", ok[i], failed[i]);
    }
    println!("{clips} clips built, {instances} scene instances");
    if !unresolved.is_empty() {
        let total: u64 = unresolved.values().sum();
        println!(
            "{} block names ({total} cells) matched no drawable block; most common:",
            unresolved.len()
        );
        let mut v: Vec<_> = unresolved.iter().collect();
        v.sort_by(|a, b| b.1.cmp(a.1));
        for (k, c) in v.iter().take(12) {
            println!("  {c:>8}  {k}");
        }
    }
    for f in &fails {
        println!("FAILED {f}");
    }
    let _ = Provider::game(hy);
    println!(
        "peak RSS {:.1} MB, {:.1} s",
        check::peak_rss() as f64 / 1e6,
        start.elapsed().as_secs_f64()
    );
    Ok(())
}
