#![no_main]
use hytale_unpack::blockymodel::{face_uvs, shape_faces, BlockyModel};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Ok(m) = BlockyModel::parse(data) else {
        return;
    };
    for f in m.flatten() {
        let _ = shape_faces(&f.node.shape);
        for l in f.node.shape.texture_layout.values() {
            let _ = face_uvs(l, 3.0, 5.0);
        }
    }
    let _ = m.problems();
    for p in ["", "Common/Blocks/x.blockymodel", "Common/NPC/x.blockymodel"] {
        let _ = m.pixels_per_unit(p);
    }
});
