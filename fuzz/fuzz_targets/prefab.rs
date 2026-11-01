#![no_main]
use hytale_unpack::bridge::block_transform;
use hytale_unpack::prefab::{self, decode_rotation, PrefabBlock, Visit};
use libfuzzer_sys::fuzz_target;

struct Walk(u64);
impl Visit for Walk {
    fn block(&mut self, b: &PrefabBlock) {
        let _ = decode_rotation(b.rotation.unwrap_or(0));
        let _ = block_transform([b.x, b.y, b.z], [i32::MIN, 0, i32::MAX], b.rotation.unwrap_or(0));
        let _ = block_transform([b.x, b.y, b.z], [0, 0, 0], b.rotation.unwrap_or(0));
        self.0 += 1;
    }
    fn wants_entities(&self) -> bool {
        self.0 % 2 == 0
    }
}

fuzz_target!(|data: &[u8]| {
    let _ = prefab::parse(data);
    let _ = prefab::stream(data, &mut Walk(0));
    let _ = prefab::stream(data, &mut Walk(1));
});
