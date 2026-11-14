//! Tests on a small archive built here, so that they run without a Hytale install: the inputs the
//! real data does not (or no longer) contain, such as a `Parent` cycle, a half gradient pair or
//! an attachment so large that it overflows.

#![cfg(feature = "bridge")]

use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};

use hytale_unpack::bridge::Hytale;
use hytale_unpack::image::encode_png;

/// A `Hytale` over an archive of the given entries, and the file it lives in (removed on drop).
struct Fake {
    hy: Hytale,
    path: PathBuf,
}

impl Drop for Fake {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

fn archive(files: &[(String, Vec<u8>)]) -> Fake {
    static N: AtomicUsize = AtomicUsize::new(0);
    let path = std::env::temp_dir().join(format!(
        "hytale-unpack-test-{}-{}.zip",
        std::process::id(),
        N.fetch_add(1, Ordering::SeqCst)
    ));
    let mut zip = zip::ZipWriter::new(std::fs::File::create(&path).unwrap());
    let opts =
        zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    for (name, bytes) in files {
        zip.start_file(name, opts).unwrap();
        zip.write_all(bytes).unwrap();
    }
    zip.finish().unwrap();
    let hy = Hytale::open(&path).unwrap();
    Fake { hy, path }
}

fn grey_png(w: u32, h: u32, v: u8) -> Vec<u8> {
    let rgba: Vec<u8> = (0..w * h).flat_map(|_| [v, v, v, 255]).collect();
    encode_png(w, h, &rgba).unwrap()
}

fn ramp_png() -> Vec<u8> {
    let rgba: Vec<u8> = (0..=255u8).flat_map(|x| [x, 0, 255 - x, 255]).collect();
    encode_png(256, 1, &rgba).unwrap()
}

const FACE: &str = r#""textureLayout":{"front":{"offset":{"x":0,"y":0},"mirror":{"x":false,"y":false},"angle":0}}"#;

fn body() -> String {
    format!(
        r#"{{"nodes":[{{"id":"1","name":"Root","position":{{"x":0,"y":0,"z":0}},"shape":{{"type":"none"}},
        "children":[{{"id":"2","name":"Head","position":{{"x":0,"y":20,"z":0}},
          "shape":{{"type":"box","offset":{{"x":0,"y":5,"z":0}},"settings":{{"size":{{"x":10,"y":10,"z":10}}}},{FACE}}}}}]}}]}}"#
    )
}

/// A hat: a piece named `Head` with a box child
fn hat() -> String {
    format!(
        r#"{{"nodes":[{{"id":"1","name":"Head","position":{{"x":0,"y":0,"z":0}},
        "shape":{{"type":"none","settings":{{"isPiece":true}}}},
        "children":[{{"id":"2","name":"Hat","position":{{"x":0,"y":6,"z":0}},
          "shape":{{"type":"box","settings":{{"size":{{"x":12,"y":4,"z":12}}}},{FACE}}}}}]}}]}}"#
    )
}

/// A node whose child overflows f32 once the parent's shape offset is added
fn huge() -> String {
    r#"{"nodes":[{"id":"1","name":"Far","position":{"x":0,"y":0,"z":0},
      "shape":{"type":"none","offset":{"x":3e38,"y":0,"z":0}},
      "children":[{"id":"2","name":"Child","position":{"x":3e38,"y":0,"z":0},"shape":{"type":"none"}}]}]}"#
        .to_string()
}

fn base_files() -> Vec<(String, Vec<u8>)> {
    let f = |n: &str, b: Vec<u8>| (n.to_string(), b);
    vec![
        f("Common/Body.blockymodel", body().into_bytes()),
        f("Common/Hat.blockymodel", hat().into_bytes()),
        f("Common/Huge.blockymodel", huge().into_bytes()),
        f("Common/Body.png", grey_png(4, 4, 100)),
        f("Common/Hat.png", grey_png(8, 8, 200)),
        f("Common/TintGradients/Hair/Brown.png", ramp_png()),
        f(
            "Cosmetics/CharacterCreator/GradientSets.json",
            br#"[{"Id":"Hair","Gradients":{"Brown":{"Texture":"TintGradients/Hair/Brown.png"}}}]"#
                .to_vec(),
        ),
    ]
}

fn model(files: &mut Vec<(String, Vec<u8>)>, name: &str, json: &str) {
    files.push((
        format!("Server/Models/{name}.json"),
        json.as_bytes().to_vec(),
    ));
}

fn path(name: &str) -> String {
    format!("Server/Models/{name}.json")
}

#[test]
fn half_a_gradient_pair_is_worn_untinted_and_reported_never_completed() {
    let mut files = base_files();
    model(
        &mut files,
        "Half",
        r#"{"Model":"Body.blockymodel","Texture":"Body.png","GradientId":"Brown",
            "DefaultAttachments":[
              {"Model":"Hat.blockymodel","Texture":"Hat.png","GradientId":"Brown"},
              {"Model":"Hat.blockymodel","Texture":"Hat.png","GradientSet":"Hair"},
              {"Model":"Hat.blockymodel","Texture":"Hat.png","GradientSet":"Hair","GradientId":"Brown"}]}"#,
    );
    let fake = archive(&files);
    let (am, report) = fake.hy.load_animated_report(&path("Half")).unwrap();
    am.validate().unwrap();
    // the entity has a GradientId from its own file and no GradientSet
    assert_eq!(report.tint, None);
    assert_eq!(
        report.tint_not_applied.as_deref(),
        Some("GradientId from Server/Models/Half.json without a GradientSet")
    );
    assert_eq!(report.attachments.len(), 3);
    let a = &report.attachments;
    assert_eq!(a[0].tint, None);
    assert!(a[0]
        .tint_not_applied
        .as_ref()
        .unwrap()
        .contains("GradientId Brown without a GradientSet"));
    assert!(a[1]
        .tint_not_applied
        .as_ref()
        .unwrap()
        .contains("GradientSet Hair without a GradientId"));
    // a whole pair is applied
    assert_eq!(a[2].tint.as_deref(), Some("Hair/Brown"));
    assert_eq!(a[2].tint_not_applied, None);
    // the half-paired hat keeps its own grey, the whole one is tinted
    let grey = am.model.materials[1].base_color.as_ref().unwrap().rgba[0];
    let tinted = am.model.materials[3].base_color.as_ref().unwrap().rgba[0];
    assert_eq!(grey, 200);
    assert_eq!(tinted, 200); // ramp column 200: red 200
    let blue = am.model.materials[3].base_color.as_ref().unwrap().rgba[2];
    assert_eq!(blue, 55);
    assert_eq!(
        am.model.materials[1].base_color.as_ref().unwrap().rgba[2],
        200
    );
}

#[test]
fn a_gradient_that_is_not_in_the_sets_is_reported_not_swallowed() {
    let mut files = base_files();
    model(
        &mut files,
        "Lost",
        r#"{"Model":"Body.blockymodel","Texture":"Body.png","GradientSet":"Hair","GradientId":"Nope",
            "DefaultAttachments":[
              {"Model":"Hat.blockymodel","Texture":"Hat.png","GradientSet":"Hair","GradientId":"Nope"}]}"#,
    );
    let fake = archive(&files);
    let (am, report) = fake.hy.load_animated_report(&path("Lost")).unwrap();
    am.validate().unwrap();
    assert_eq!(report.tint, None);
    let why = report.tint_not_applied.unwrap();
    assert!(
        why.starts_with("Hair/Nope: gradient Hair/Nope is not in"),
        "{why}"
    );
    // the attachment is worn as it is, with the reason
    assert_eq!(report.attachments.len(), 1);
    assert!(report.attachments[0]
        .tint_not_applied
        .as_ref()
        .unwrap()
        .contains("not in"));
    assert!(report.attachments_left_out.is_empty());
    assert_eq!(
        am.model.materials[1].base_color.as_ref().unwrap().rgba[2],
        200
    );
}

#[test]
fn an_attachment_that_overflows_is_left_out_and_the_entity_still_loads() {
    let mut files = base_files();
    model(
        &mut files,
        "Big",
        r#"{"Model":"Body.blockymodel","Texture":"Body.png",
            "DefaultAttachments":[{"Model":"Huge.blockymodel"},{"Model":"Hat.blockymodel","Texture":"Hat.png"}]}"#,
    );
    let fake = archive(&files);
    let (am, report) = fake.hy.load_animated_report(&path("Big")).unwrap();
    am.validate().unwrap();
    assert_eq!(report.attachments_left_out.len(), 1);
    assert!(report.attachments_left_out[0].1.contains("not finite"));
    // the hat after it is still worn
    assert_eq!(report.attachments.len(), 1);
    assert!(report.attachments[0].model.ends_with("Hat.blockymodel"));
}

#[test]
fn a_weight_that_is_not_a_number_still_leaves_the_entry_out() {
    let mut files = base_files();
    model(
        &mut files,
        "Weighed",
        r#"{"Model":"Body.blockymodel","Texture":"Body.png",
            "DefaultAttachments":[{"Model":"Hat.blockymodel","Texture":"Hat.png","Weight":"heavy"}]}"#,
    );
    let fake = archive(&files);
    let (_, report) = fake.hy.load_animated_report(&path("Weighed")).unwrap();
    assert!(report.attachments.is_empty());
    assert!(report.attachments_left_out[0]
        .1
        .contains("Weight (not a number)"));
}

#[test]
fn a_parent_cycle_is_an_error_not_a_hang() {
    let mut files = base_files();
    model(
        &mut files,
        "CycA",
        r#"{"Parent":"CycB","Model":"Body.blockymodel"}"#,
    );
    model(&mut files, "CycB", r#"{"Parent":"CycA"}"#);
    let fake = archive(&files);
    let err = fake
        .hy
        .defs()
        .model_def(fake.hy.assets(), &path("CycA"))
        .unwrap_err();
    assert!(
        format!("{err:#}").contains("Parent chain too deep"),
        "{err:#}"
    );
    assert!(fake.hy.load_animated_report(&path("CycA")).is_err());
}

/// `n` definitions `Chain0` .. `Chain{n-1}`, each the child of the next; the last one holds
/// `Texture`, `GradientSet` and `GradientId`.
fn chain(n: usize) -> Fake {
    let mut files = base_files();
    for i in 0..n {
        let json = if i + 1 < n {
            format!(r#"{{"Parent":"Chain{}"}}"#, i + 1)
        } else {
            r#"{"Model":"Body.blockymodel","Texture":"Body.png","GradientSet":"Hair","GradientId":"Brown"}"#
                .to_string()
        };
        model(&mut files, &format!("Chain{i}"), &json);
    }
    archive(&files)
}

#[test]
fn the_deepest_allowed_parent_chain_still_finds_where_each_key_comes_from() {
    // 33 files is the most `Defs` resolves; the keys are in the last one, all three in one file
    let fake = chain(33);
    let def = fake
        .hy
        .defs()
        .model_def(fake.hy.assets(), &path("Chain0"))
        .unwrap();
    let last = path("Chain32");
    assert_eq!(def.texture_from.as_deref(), Some(last.as_str()));
    assert_eq!(def.gradient_from.as_deref(), Some(last.as_str()));
    assert!(def.gradient_paired());
    let (_, report) = fake.hy.load_animated_report(&path("Chain0")).unwrap();
    assert_eq!(report.tint.as_deref(), Some("Hair/Brown"));
}

#[test]
fn a_chain_one_file_too_long_reports_the_depth_error() {
    let fake = chain(34);
    let err = fake
        .hy
        .defs()
        .model_def(fake.hy.assets(), &path("Chain0"))
        .unwrap_err();
    assert!(
        format!("{err:#}").contains("Parent chain too deep"),
        "{err:#}"
    );
}

fn anim(nodes: &str) -> Vec<u8> {
    format!(r#"{{"duration":120,"nodeAnimations":{{{nodes}}}}}"#).into_bytes()
}

#[test]
fn keys_before_zero_cost_a_channel_at_most_never_the_clip() {
    let mut files = base_files();
    let rot = |t: i32, z: f32| format!(r#"{{"time":{t},"delta":{{"x":0,"y":0,"z":{z},"w":1}}}}"#);
    // `Head` orientation: a key at -49, one at 0, one at 30 (trimmed); `Root` position: a key at
    // -49 and none at 0 (that channel goes); `Head` position: a clean channel that stays
    files.push((
        "Common/Anim/Early.blockyanim".into(),
        anim(&format!(
            r#""Head":{{"orientation":[{},{},{}],"position":[{{"time":0,"delta":{{"x":0,"y":0,"z":0}}}},{{"time":30,"delta":{{"x":1,"y":0,"z":0}}}}]}},
               "Root":{{"position":[{{"time":-49,"delta":{{"x":0,"y":0,"z":0}}}},{{"time":30,"delta":{{"x":1,"y":0,"z":0}}}}]}}"#,
            rot(-49, 0.2),
            rot(0, 0.0),
            rot(30, 0.2)
        )),
    ));
    model(
        &mut files,
        "Moves",
        r#"{"Model":"Body.blockymodel","Texture":"Body.png",
            "AnimationSets":{"RestUp":{"Animations":[{"Animation":"Anim/Early.blockyanim"}]}}}"#,
    );
    let fake = archive(&files);
    let (am, report) = fake.hy.load_animated_report(&path("Moves")).unwrap();
    am.validate().unwrap();
    assert_eq!(report.clips, 1);
    assert!(
        report.skipped_clips.is_empty(),
        "{:?}",
        report.skipped_clips
    );
    assert_eq!(
        report.trimmed_channels,
        [("RestUp".to_string(), "Head orientation: 1 keys".to_string())]
    );
    assert_eq!(report.dropped_channels.len(), 1);
    assert!(report.dropped_channels[0]
        .1
        .starts_with("Root position: 1 keys before time 0 and none at 0"));
    // Head orientation (trimmed) and Head position are in the clip; Root position is not
    assert_eq!(am.animations[0].channels.len(), 2);
    assert!(am.animations[0].channels.iter().all(|c| c.times[0] == 0.0));
}

#[test]
fn referenced_clip_read_and_parse_failures_are_reported() {
    for content in [None, Some(b"{".to_vec())] {
        let mut files = base_files();
        model(
            &mut files,
            "BrokenClip",
            r#"{"Model":"Body.blockymodel","Texture":"Body.png","AnimationSets":{"Idle":{"Animations":[{"Animation":"Broken.blockyanim"}]}}}"#,
        );
        if let Some(b) = content {
            files.push(("Common/Broken.blockyanim".into(), b));
        }
        let fake = archive(&files);
        let (am, report) = fake.hy.load_animated_report(&path("BrokenClip")).unwrap();
        assert!(am.animations.is_empty());
        assert_eq!(report.skipped_clips.len(), 1);
        assert_eq!(report.skipped_clips[0].0, "Idle");
        assert!(report.skipped_clips[0].1.contains("Broken.blockyanim"));
    }
}

#[test]
fn prefab_special_component_does_not_export_to_parent() {
    let fake = archive(&[(
        "Server/Prefabs/...prefab.json".into(),
        br#"{"version":8,"blocks":[]}"#.to_vec(),
    )]);
    let dir = std::env::temp_dir().join(format!("hytale-prefab-path-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let result = std::process::Command::new(env!("CARGO_BIN_EXE_hytale-unpack"))
        .arg("--assets")
        .arg(&fake.path)
        .arg("prefab")
        .arg("Server/Prefabs/...prefab.json")
        .arg("-o")
        .arg(dir.join("output"))
        .output()
        .unwrap();
    assert!(
        !dir.join("...gltf").exists(),
        "{}",
        String::from_utf8_lossy(&result.stdout)
    );
    assert!(!dir.join("...bin").exists());
    assert!(!result.status.success());
    std::fs::remove_dir_all(&dir).unwrap();
}

// ---- blocks and scenes ----

const ROCK: &str =
    r#"{"BlockType":{"DrawType":"Cube","Textures":[{"All":"BlockTextures/Rock.png"}]}}"#;

/// A cube block `Rock_Stone`, a `Rail` with one state, `Barrier` (a block that draws nothing)
/// and a texture, plus the given prefab cells in `Server/Prefabs/Test/Mixed.prefab.json`
fn block_files(extra: &[(&str, &str)], cells: &[&str]) -> Vec<(String, Vec<u8>)> {
    let f = |n: &str, b: Vec<u8>| (n.to_string(), b);
    let mut files = vec![
        f("Common/BlockTextures/Rock.png", grey_png(4, 4, 90)),
        f("Server/Item/Items/Rock/Rock_Stone.json", ROCK.into()),
        f(
            "Server/Item/Items/Rail/Rail.json",
            br#"{"BlockType":{"DrawType":"Cube","Textures":[{"All":"BlockTextures/Rock.png"}],
                "State":{"Definitions":{"Corner":{"Textures":[{"Sides":"BlockTextures/Rock.png"}]}}}}}"#
                .to_vec(),
        ),
        f(
            "Server/Item/Items/Barrier.json",
            br#"{"BlockType":{"DrawType":"Empty"}}"#.to_vec(),
        ),
    ];
    for (name, json) in extra {
        files.push(f(name, json.as_bytes().to_vec()));
    }
    let blocks: Vec<String> = cells
        .iter()
        .enumerate()
        .map(|(i, name)| format!(r#"{{"x":{i},"y":0,"z":0,"name":"{name}"}}"#))
        .collect();
    files.push(f(
        "Server/Prefabs/Test/Mixed.prefab.json",
        format!(
            r#"{{"version":8,"blockIdVersion":11,"anchorX":0,"anchorY":0,"anchorZ":0,"blocks":[{}]}}"#,
            blocks.join(",")
        )
        .into_bytes(),
    ));
    files
}

const MIXED: &str = "Server/Prefabs/Test/Mixed.prefab.json";

/// Found by review: a block that failed to load was counted with the names that match no block,
/// without its error, and the scene looked complete.
#[test]
fn a_block_that_fails_to_load_is_reported_as_failed_not_as_matching_no_block() {
    let fake = archive(&block_files(
        &[
            (
                "Server/Item/Items/Broken_Texture.json",
                r#"{"BlockType":{"DrawType":"Cube","Textures":[{"All":"BlockTextures/Missing.png"}]}}"#,
            ),
            (
                "Server/Item/Items/Broken_Model.json",
                r#"{"BlockType":{"DrawType":"Model","CustomModel":"Blocks/Broken.blockymodel"}}"#,
            ),
            ("Common/Blocks/Broken.blockymodel", "{"),
        ],
        &[
            "Rock_Stone",
            "*Rail_State_Definitions_Corner",
            "*Rail_State_Definitions_Nope",
            "Broken_Texture",
            "Broken_Model",
            "*Rail_State_Definitions_Nope",
            "Barrier",
            "Not_An_Item",
            "Empty",
            "Rock_Stone",
            "Broken_Model",
        ],
    ));
    let (scene, report) = fake.hy.load_scene_report(MIXED).unwrap();
    scene.validate().unwrap();
    // what loads is placed as before
    assert_eq!(scene.models.len(), 2);
    assert_eq!(scene.instances.len(), 3);
    assert_eq!((report.blocks, report.placed, report.empty), (11, 3, 1));
    // a block the game does not draw and a name with no item behind it match no block
    let unresolved: Vec<(&str, u64)> = report
        .unresolved
        .iter()
        .map(|(k, v)| (k.as_str(), *v))
        .collect();
    assert_eq!(unresolved, [("Barrier", 1), ("Not_An_Item", 1)]);
    // the three that exist and did not load are failures, each with its cells and its error
    let failed: Vec<(&str, u64)> = report
        .failed
        .iter()
        .map(|(k, v)| (k.as_str(), v.0))
        .collect();
    assert_eq!(
        failed,
        [
            ("*Rail_State_Definitions_Nope", 2),
            ("Broken_Model", 2),
            ("Broken_Texture", 1)
        ]
    );
    assert!(
        report.failed["*Rail_State_Definitions_Nope"]
            .1
            .contains("no state Nope"),
        "{:?}",
        report.failed
    );
    assert!(report.failed["Broken_Texture"].1.contains("Missing.png"));
    assert!(report.failed["Broken_Model"]
        .1
        .contains("Broken.blockymodel"));
    // every cell is accounted for
    let skipped: u64 =
        report.unresolved.values().sum::<u64>() + report.failed.values().map(|f| f.0).sum::<u64>();
    assert_eq!(
        report.blocks,
        report.placed + report.empty + report.fillers + skipped
    );
}

fn check_bridge(fake: &Fake) -> (std::process::ExitStatus, String) {
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_hytale-unpack"))
        .arg("--assets")
        .arg(&fake.path)
        .args(["check-bridge", "--scenes", "30"])
        .output()
        .unwrap();
    (
        out.status,
        String::from_utf8_lossy(&out.stdout).into_owned(),
    )
}

/// Found by review: `check-bridge` printed its failures and exited with 0, and a block state that
/// only a prefab names (nothing else loads it) did not even count as one.
#[test]
fn check_bridge_exits_with_1_when_a_block_of_a_scene_fails_to_load() {
    let fake = archive(&block_files(
        &[],
        &["Rock_Stone", "*Rail_State_Definitions_Nope", "Barrier"],
    ));
    let (status, stdout) = check_bridge(&fake);
    assert_eq!(status.code(), Some(1), "{stdout}");
    assert!(
        stdout.contains(&format!("FAILED {MIXED}: 1 block names failed to load")),
        "{stdout}"
    );
    assert!(stdout.contains("*Rail_State_Definitions_Nope (1 cells)"));
    assert!(stdout.contains("no state Nope"), "{stdout}");
    // the block that draws nothing is still only a name that matches no drawable block
    assert!(
        stdout.contains("1 block names (1 cells) matched no drawable block"),
        "{stdout}"
    );
}

#[test]
fn check_bridge_exits_with_1_when_a_model_fails_and_with_0_when_all_load() {
    let good = archive(&block_files(
        &[],
        &["Rock_Stone", "*Rail_State_Definitions_Corner", "Barrier"],
    ));
    let (status, stdout) = check_bridge(&good);
    assert_eq!(status.code(), Some(0), "{stdout}");
    assert!(!stdout.contains("FAILED"), "{stdout}");

    let mut files = block_files(&[], &["Rock_Stone"]);
    files.push(("Common/Broken.blockymodel".into(), b"{".to_vec()));
    let bad = archive(&files);
    let (status, stdout) = check_bridge(&bad);
    assert_eq!(status.code(), Some(1), "{stdout}");
    assert!(
        stdout.contains("FAILED Common/Broken.blockymodel"),
        "{stdout}"
    );
}

// ---- smaller findings of the review ----

/// Found by review: a `GradientSets.json` that cannot be read or parsed was swallowed, and every
/// tint then failed with "gradient ... is not in GradientSets.json", which is not what happened.
#[test]
fn a_broken_gradient_sets_file_is_named_as_the_reason_a_tint_is_not_applied() {
    let mut files: Vec<(String, Vec<u8>)> = base_files()
        .into_iter()
        .filter(|(n, _)| !n.ends_with("GradientSets.json"))
        .collect();
    files.push((
        "Cosmetics/CharacterCreator/GradientSets.json".into(),
        br#"[{"Id":"Hair","Gradients":{"Brown":"#.to_vec(),
    ));
    model(
        &mut files,
        "Tinted",
        r#"{"Model":"Body.blockymodel","Texture":"Body.png","GradientSet":"Hair","GradientId":"Brown",
            "DefaultAttachments":[
              {"Model":"Hat.blockymodel","Texture":"Hat.png","GradientSet":"Hair","GradientId":"Brown"}]}"#,
    );
    let fake = archive(&files);
    // the model still loads, untinted, and both reports say why
    let (am, report) = fake.hy.load_animated_report(&path("Tinted")).unwrap();
    am.validate().unwrap();
    assert_eq!(report.tint, None);
    let why = report.tint_not_applied.unwrap();
    assert!(
        why.starts_with(
            "Hair/Brown: Cosmetics/CharacterCreator/GradientSets.json could not be read: "
        ),
        "{why}"
    );
    assert!(why.contains("EOF while parsing"), "{why}");
    assert!(!why.contains("is not in"), "{why}");
    let hat = report.attachments[0].tint_not_applied.as_ref().unwrap();
    assert!(hat.contains("could not be read"), "{hat}");
    // the sets are empty, as they are for an archive without the file, and the error is there
    // to ask for
    let defs = fake.hy.defs();
    assert!(defs.gradient_sets(fake.hy.assets()).is_empty());
    assert!(defs.gradient_sets_error(fake.hy.assets()).is_some());
    // a file that reads has no error
    let good = archive(&base_files());
    assert!(good
        .hy
        .defs()
        .gradient_sets_error(good.hy.assets())
        .is_none());
    assert_eq!(good.hy.defs().gradient_sets(good.hy.assets()).len(), 1);
}

/// Found by review: `Defs::model_paths` handed out the values of a hash map, in an order that
/// differs from run to run (the `assets_zip` fuzz target tests the first 16).
#[test]
fn model_paths_come_in_a_fixed_order() {
    let mut files = base_files();
    for i in 0..40 {
        model(
            &mut files,
            &format!("Folder{}/Model_{:02}", i % 3, (i * 7) % 40),
            r#"{"Model":"Body.blockymodel"}"#,
        );
    }
    let fake = archive(&files);
    let paths: Vec<&str> = fake.hy.defs().model_paths().collect();
    assert_eq!(paths.len(), 40);
    let mut sorted = paths.clone();
    sorted.sort();
    assert_eq!(paths, sorted);
}

/// Found by review: `prefab` printed its fluid lines in the order of a hash map, different from
/// run to run. They come like the block names: most cells first, then by name.
#[test]
fn prefab_prints_its_fluids_in_a_fixed_order() {
    let names = [
        ("Water_Source", 3),
        ("Lava", 1),
        ("Water", 3),
        ("Mud", 2),
        ("Acid", 1),
        ("Tar", 5),
        ("Slime", 1),
        ("Poison", 2),
    ];
    let mut fluids = Vec::new();
    for (name, cells) in names {
        for i in 0..cells {
            fluids.push(format!(
                r#"{{"x":{i},"y":0,"z":0,"name":"{name}","level":1}}"#
            ));
        }
    }
    let fake = archive(&[(
        "Server/Prefabs/Pond.prefab.json".into(),
        format!(
            r#"{{"version":8,"blocks":[],"fluids":[{}]}}"#,
            fluids.join(",")
        )
        .into_bytes(),
    )]);
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_hytale-unpack"))
        .arg("--assets")
        .arg(&fake.path)
        .args(["prefab", "Pond"])
        .output()
        .unwrap();
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    let lines: Vec<&str> = stdout
        .lines()
        .filter(|l| l.trim_start().starts_with("fluid "))
        .map(str::trim)
        .collect();
    assert_eq!(
        lines,
        [
            "fluid Tar: 5 cells",
            "fluid Water: 3 cells",
            "fluid Water_Source: 3 cells",
            "fluid Mud: 2 cells",
            "fluid Poison: 2 cells",
            "fluid Acid: 1 cells",
            "fluid Lava: 1 cells",
            "fluid Slime: 1 cells",
        ]
    );
}
