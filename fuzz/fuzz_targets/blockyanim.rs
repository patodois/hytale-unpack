#![no_main]
use hytale_unpack::blockyanim::BlockyAnim;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Ok(a) = BlockyAnim::parse(data) else {
        return;
    };
    let _ = a.problems();
    let _ = a.key_count();
    let _ = a.duration_s();
});
