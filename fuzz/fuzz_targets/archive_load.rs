//! Builds a small archive from the input and loads everything in it through the `Hytale` provider.
//!
//! The input is split on `\n@@\n` into these slots (missing ones are left out of the archive):
//! 0,1 models (`Common/A`, `Common/B.blockymodel`), 2 clip (`Common/C.blockyanim`),
//! 3,4 entity definitions (`Server/Models/E`, `Server/Models/P.json`), 5 block item
//! (`Server/Item/Items/Blk.json`), 6 prefab (`Server/Prefabs/F.prefab.json`), 7,8 textures
//! (`Common/T.png`, `Common/G.png`), 9 gradient sets (`Cosmetics/CharacterCreator/GradientSets.json`).
#![no_main]
use std::io::Write;

use hytale_unpack::bridge::Hytale;
use libfuzzer_sys::fuzz_target;
use rewrite_bridge::Provider;

const SLOTS: [&str; 10] = [
    "Common/A.blockymodel",
    "Common/B.blockymodel",
    "Common/C.blockyanim",
    "Server/Models/E.json",
    "Server/Models/P.json",
    "Server/Item/Items/Blk.json",
    "Server/Prefabs/F.prefab.json",
    "Common/T.png",
    "Common/G.png",
    "Cosmetics/CharacterCreator/GradientSets.json",
];

fn split(data: &[u8]) -> Vec<&[u8]> {
    let sep = b"\n@@\n";
    let mut out = Vec::new();
    let mut start = 0;
    let mut i = 0;
    while i + sep.len() <= data.len() && out.len() < SLOTS.len() - 1 {
        if &data[i..i + sep.len()] == sep {
            out.push(&data[start..i]);
            i += sep.len();
            start = i;
        } else {
            i += 1;
        }
    }
    out.push(&data[start..]);
    out
}

fuzz_target!(|data: &[u8]| {
    let dir = std::path::PathBuf::from(format!("/dev/shm/hytale-fuzz-{}", std::process::id()));
    let _ = std::fs::create_dir_all(&dir);
    let path = dir.join("a.zip");
    {
        let mut zip = zip::ZipWriter::new(std::fs::File::create(&path).unwrap());
        let opts = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored);
        for (name, bytes) in SLOTS.iter().zip(split(data)) {
            if bytes.is_empty() {
                continue;
            }
            zip.start_file(*name, opts).unwrap();
            zip.write_all(bytes).unwrap();
        }
        zip.finish().unwrap();
    }
    if let Ok(hy) = Hytale::open(&path) {
        let _ = hy.list("");
        let ids: Vec<String> = hy.assets().names().map(String::from).collect();
        for id in &ids {
            if id.ends_with("Items/Blk.json") {
                let _ = hy.is_drawable_block(id);
            }
            for att in [true, false] {
                if let Ok((am, _)) = hy.load_animated_with(id, att) {
                    let _ = am.validate();
                    let _ = hytale_unpack::gltf::write_animated(&am, "m", &dir);
                }
            }
            if let Ok((scene, _)) = hy.load_scene_report(id) {
                let _ = hytale_unpack::gltf::write_scene(&scene, "s", &dir);
            }
        }
        let _ = hy.block_model("Blk");
        hy.clear_cache();
    }
    let _ = std::fs::remove_dir_all(&dir);
});
