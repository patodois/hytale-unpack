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
