//! Tests against a real install. They run when `HYTALE_ASSETS` points at `Assets.zip` (or the
//! default macOS install exists) and are skipped otherwise:
//!
//! ```sh
//! HYTALE_ASSETS=".../Assets.zip" cargo test --release
//! ```

#![cfg(feature = "bridge")]

use std::sync::OnceLock;

use hytale_unpack::bridge::{block_rotation, Hytale};
use hytale_unpack::prefab::{self, PrefabBlock, Visit};
use hytale_unpack::Assets;
use rewrite_bridge::{Interpolation, Provider, Target};

fn hy() -> Option<&'static Hytale> {
    static HY: OnceLock<Option<Hytale>> = OnceLock::new();
    HY.get_or_init(|| {
        let path = Assets::default_path();
        if !path.exists() {
            eprintln!("skipping: {} does not exist", path.display());
            return None;
        }
        Some(Hytale::open(&path).expect("open Assets.zip"))
    })
    .as_ref()
}

macro_rules! need {
    () => {
        match hy() {
            Some(h) => h,
            None => return,
        }
    };
}

fn height(m: &rewrite_bridge::Model) -> (f32, f32) {
    let world = hytale_unpack::gltf::world_matrices(m);
    let (mut lo, mut hi) = (f32::MAX, f32::MIN);
    for mesh in &m.meshes {
        let w = &world[mesh.node];
        for p in &mesh.positions {
            let y = w[1] * p[0] + w[5] * p[1] + w[9] * p[2] + w[13];
            lo = lo.min(y);
            hi = hi.max(y);
        }
    }
    (lo, hi)
}

#[test]
fn archive_index_has_the_documented_counts() {
    let hy = need!();
    let a = hy.assets();
    let count = |ext: &str| a.names().filter(|n| n.ends_with(ext)).count();
    assert_eq!(count(".blockymodel"), 2830);
    assert_eq!(count(".blockyanim"), 6736);
    assert_eq!(count(".png"), 10901);
    assert_eq!(count(".ogg"), 4490);
    assert_eq!(
        a.names_under("Server/Prefabs/")
            .filter(|n| n.ends_with(".prefab.json"))
            .count(),
        7828
    );
}

#[test]
fn the_player_is_human_sized_and_textured() {
    let hy = need!();
    let (am, report) = hy
        .load_animated_report("Server/Models/Human/Player.json")
        .unwrap();
    assert_eq!(
        am.model.nodes.iter().filter(|n| n.name == "Pelvis").count(),
        1
    );
    assert_eq!(
        report.texture.as_deref(),
        Some("Common/Characters/Player_Textures/Player_Greyscale.png")
    );
    let (lo, hi) = height(&am.model);
    // Player.json's hitbox is 1.85 high; the model's head top is a little above that
    assert!(lo.abs() < 0.02, "feet at {lo}");
    assert!((1.8..1.95).contains(&hi), "head at {hi}");
    // every UV is inside the texture
    for m in &am.model.meshes {
        assert!(m
            .uvs
            .iter()
            .all(|uv| (0.0..=1.0).contains(&uv[0]) && (0.0..=1.0).contains(&uv[1])));
    }
    am.validate().unwrap();
}

#[test]
fn clips_come_from_the_animation_sets_with_parent_inheritance() {
    let hy = need!();
    let player = hy.load_animated("Server/Models/Human/Player.json").unwrap();
    for name in ["Idle", "Walk", "Run", "Jump"] {
        assert!(player.animation(name).is_some(), "player lacks {name}");
    }
    let walk = player.animation("Walk").unwrap();
    assert!((walk.duration_s - 1.0).abs() < 1e-3);
    // The Trork's definition has "Parent": "Player" and overrides some sets
    let trork = hy
        .load_animated("Server/Models/Intelligent/Trork/Trork.json")
        .unwrap();
    assert!(trork.animation("Walk").is_some());
    assert!(trork.animations.len() > 100);
    // position channels with smooth keys are cubic, rotations are always spherical-linear
    let cubic = walk
        .channels
        .iter()
        .filter(|c| c.interpolation == Interpolation::CubicSpline)
        .count();
    let rot = walk
        .channels
        .iter()
        .filter(|c| c.target == Target::Rotation)
        .all(|c| c.interpolation == Interpolation::Linear);
    assert!(
        cubic > 0
            || walk
                .channels
                .iter()
                .all(|c| c.target != Target::Translation)
    );
    assert!(rot);
}

#[test]
fn bare_models_find_their_texture_and_folder_clips() {
    let hy = need!();
    let id = "Common/NPC/Beast/Bear_Grizzly/Models/Model.blockymodel";
    if !hy.assets().contains(id) {
        return;
    }
    let (am, report) = hy.load_animated_report(id).unwrap();
    assert!(report.texture.is_some());
    assert!(!am.animations.is_empty(), "no clips from ../Animations/");
}

#[test]
fn blocks_are_cubes_or_models_standing_on_their_cell() {
    let hy = need!();
    let stone = hy.block_model("Rock_Stone").unwrap().unwrap();
    let (lo, hi) = stone.bounds().unwrap();
    assert_eq!((lo, hi), ([-0.5, 0.0, -0.5], [0.5, 1.0, 0.5]));
    let bench = hy
        .load("Server/Item/Items/Bench/Bench_WorkBench.json")
        .unwrap();
    assert!(!bench.meshes.is_empty());
    let (blo, _) = height(&bench);
    assert!(blo.abs() < 0.05, "bench floats at {blo}");
    // an empty block draws nothing
    assert!(hy.block_model("Empty").unwrap().is_none());
}

#[test]
fn state_names_resolve_to_their_state_models() {
    let hy = need!();
    let plain = hy.block_model("Rail").unwrap().unwrap();
    let corner = hy
        .block_model("*Rail_State_Definitions_Corner_Left")
        .unwrap()
        .unwrap();
    assert_ne!(plain.meshes[0].positions, corner.meshes[0].positions);
}

/// The yaw convention is read off placed roofs: `rotation` 0..3 of a roof stair is found climbing
/// towards -Z, -X, +Z, +X in a real prefab, and `block_rotation` must turn -Z the same way.
#[test]
fn yaw_matches_the_way_roofs_climb_in_a_real_prefab() {
    let hy = need!();
    let id = "Server/Prefabs/Monuments/Encounter/Zone4/Tier4/Village/Forest/Townhall/Encounter_Forest_Townhall_001.prefab.json";
    if !hy.assets().contains(id) {
        return;
    }
    struct Roofs(Vec<PrefabBlock>);
    impl Visit for Roofs {
        fn block(&mut self, b: &PrefabBlock) {
            if b.name == "Wood_Deadwood_Roof" {
                self.0.push(b.clone());
            }
        }
    }
    let mut r = Roofs(vec![]);
    prefab::stream(&hy.assets().read(id).unwrap()[..], &mut r).unwrap();
    let mut climbs = std::collections::HashMap::<u32, [u32; 4]>::new();
    for a in &r.0 {
        for b in &r.0 {
            if b.y == a.y + 1 && b.rotation == a.rotation {
                let (dx, dz) = (b.x - a.x, b.z - a.z);
                let k = match (dx, dz) {
                    (0, -1) => 0, // -Z
                    (-1, 0) => 1, // -X
                    (0, 1) => 2,  // +Z
                    (1, 0) => 3,  // +X
                    _ => continue,
                };
                climbs.entry(a.rotation.unwrap_or(0)).or_default()[k] += 1;
            }
        }
    }
    let dirs = [
        [0.0, 0.0, -1.0],
        [-1.0, 0.0, 0.0],
        [0.0, 0.0, 1.0],
        [1.0, 0.0, 0.0],
    ];
    for (rot, counts) in climbs {
        if rot > 3 {
            continue;
        }
        let best = (0..4).max_by_key(|&k| counts[k]).unwrap();
        assert!(counts[best] > 0);
        // -Z turned by this rotation
        let q = block_rotation(rot);
        let v = rotate(q, [0.0, 0.0, -1.0]);
        let want = dirs[best];
        assert!(
            (0..3).all(|k| (v[k] - want[k]).abs() < 1e-4),
            "rotation {rot}: {v:?} vs {want:?} ({counts:?})"
        );
    }
}

fn rotate(q: [f32; 4], v: [f32; 3]) -> [f32; 3] {
    let qi = [-q[0], -q[1], -q[2], q[3]];
    let m = |a: [f32; 4], b: [f32; 4]| {
        [
            a[3] * b[0] + a[0] * b[3] + a[1] * b[2] - a[2] * b[1],
            a[3] * b[1] - a[0] * b[2] + a[1] * b[3] + a[2] * b[0],
            a[3] * b[2] + a[0] * b[1] - a[1] * b[0] + a[2] * b[3],
            a[3] * b[3] - a[0] * b[0] - a[1] * b[1] - a[2] * b[2],
        ]
    };
    let r = m(m(q, [v[0], v[1], v[2], 0.0]), qi);
    [r[0], r[1], r[2]]
}

#[test]
fn a_prefab_loads_as_a_scene_of_placed_blocks() {
    let hy = need!();
    let id = "Server/Prefabs/Npc/Kweebec/Autumn/Houses_Small/Kweebec_Autumn_Houses_Small_001.prefab.json";
    if !hy.assets().contains(id) {
        return;
    }
    let (scene, report) = hy.load_scene_report(id).unwrap();
    assert!(report.unresolved.is_empty(), "{:?}", report.unresolved);
    assert_eq!(report.placed as usize, scene.instances.len());
    assert_eq!(report.blocks, report.placed + report.empty + report.fillers);
    assert!(scene.instances.len() > 1000);
    assert!(
        scene.models.len() < 40,
        "models are shared between instances"
    );
    scene.validate().unwrap();
    // load_scene is the same scene
    assert_eq!(
        hy.load_scene(id).unwrap().instances.len(),
        scene.instances.len()
    );
}

#[test]
fn list_filters_by_substring_and_tags_kinds() {
    let hy = need!();
    let l = hy.list("Player.json").unwrap();
    assert!(l.iter().any(|a| a.id == "Server/Models/Human/Player.json"
        && a.kind == rewrite_bridge::AssetKind::Character));
    let l = hy.list("Townhall_001").unwrap();
    assert!(l.iter().all(|a| a.kind == rewrite_bridge::AssetKind::Level));
    assert!(hy.list("zzzz-no-such-thing").unwrap().is_empty());
}

#[test]
fn list_offers_only_what_draws() {
    let hy = need!();
    // editor tools whose BlockType only names a block list or a prefab list, an invisible block, and
    // entity definitions that are only particles
    for id in [
        "Filter_TreeWoodAndLeaves",
        "Filter_Rock",
        "ExamplePrefabPlacementBlock_Tree",
        "Barrier",
        "Showcase_Mannequin_Inv_Portal",
    ] {
        let l = hy.list(id).unwrap();
        assert!(
            l.is_empty(),
            "{id} is listed: {:?}",
            l.first().map(|a| &a.id)
        );
    }
    assert!(!hy.list("Rock_Stone").unwrap().is_empty());
}

#[test]
fn everything_listed_that_is_not_a_prefab_loads() {
    use rewrite_bridge::AssetKind;
    let hy = need!();
    let mut failures = Vec::new();
    for (n, a) in hy.list("").unwrap().iter().enumerate() {
        // blocks and entity models; the bare models are loaded by the conformance sweep
        if a.kind == AssetKind::Level || !a.id.ends_with(".json") {
            continue;
        }
        if n % 50 == 0 {
            hy.clear_cache();
        }
        if let Err(e) = hy.load_animated(&a.id) {
            failures.push(format!("{}: {e}", a.id));
        }
    }
    assert!(failures.is_empty(), "{failures:#?}");
}

#[test]
fn clip_names_are_unique() {
    let hy = need!();
    // The bat's Fly lists one file at three speeds; Pterodactyl does the same
    for (id, slow, fast) in [
        (
            "Server/Models/Flying_Critter/Bat_Ice.json",
            "Fly/Fly@1.25",
            "Fly/Fly@1.5",
        ),
        (
            "Server/Models/Flying_Critter/Bat.json",
            "Fly/Fly@1.25",
            "Fly/Fly@1.5",
        ),
        (
            "Server/Models/Flying_Beast/Pterodactyl.json",
            "Fly/Fly@1.2",
            "Fly/Fly_Glide",
        ),
    ] {
        let am = hy.load_animated(id).unwrap();
        let mut names: Vec<&str> = am.animations.iter().map(|a| a.name.as_str()).collect();
        let total = names.len();
        names.sort();
        names.dedup();
        assert_eq!(names.len(), total, "{id} repeats a clip name");
        for name in ["Fly/Fly", slow, fast] {
            assert!(am.animation(name).is_some(), "{id} has no clip {name}");
        }
    }
}

#[test]
fn a_sample_conforms_to_the_bridge() {
    use rewrite_bridge::conformance::check_provider;
    let hy = need!();
    let ids = [
        "Server/Models/Human/Player.json",
        "Server/Models/Flying_Critter/Bat_Ice.json",
        "Server/Item/Items/Bench/Bench_WorkBench.json",
        "Server/Prefabs/Npc/Kweebec/Autumn/Houses_Small/Kweebec_Autumn_Houses_Small_001.prefab.json",
    ];
    for (id, report) in check_provider(hy, &ids, &[]) {
        assert!(report.passed(), "{id}: {report}");
        assert!(!report.has("provider.unlisted_id"), "{id} is not listed");
        assert!(!report.has("animation.duplicate_name"), "{id}: {report}");
    }
}

#[test]
fn every_prefab_in_a_sample_streams() {
    let hy = need!();
    let a = hy.assets();
    let mut n = 0;
    for name in a
        .names_under("Server/Prefabs/")
        .filter(|n| n.ends_with(".prefab.json"))
        .step_by(97)
    {
        struct Nop;
        impl Visit for Nop {}
        let (h, _) = a
            .with_reader(name, |r| prefab::stream(r, &mut Nop))
            .unwrap();
        assert_eq!((h.version, h.block_id_version), (Some(8), Some(11)));
        n += 1;
    }
    assert!(n > 50);
}

#[test]
fn a_block_with_a_texture_per_side_exports() {
    // Soil_Grass shows grass on top, dirt below and a third texture on the sides: three meshes on
    // the block's one node, which the glTF writer once refused ("node 0 carries more than one mesh")
    let hy = need!();
    let am = hy
        .load_animated("Server/Item/Items/Soil/Grass/Soil_Grass.json")
        .unwrap();
    let meshes_on_the_node = am.model.meshes.iter().filter(|m| m.node == 0).count();
    assert_eq!(
        meshes_on_the_node,
        3,
        "{:?}",
        am.model.meshes.iter().map(|m| &m.name).collect::<Vec<_>>()
    );
    let dir = std::env::temp_dir().join(format!("hytale-unpack-soil-{}", std::process::id()));
    let path = hytale_unpack::gltf::write_animated(&am, "Soil_Grass", &dir).unwrap();
    let doc: serde_json::Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    std::fs::remove_dir_all(&dir).unwrap();
    let mesh = doc["nodes"][0]["mesh"].as_u64().unwrap() as usize;
    assert_eq!(
        doc["meshes"][mesh]["primitives"].as_array().unwrap().len(),
        3
    );
}
