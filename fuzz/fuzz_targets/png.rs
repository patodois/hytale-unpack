#![no_main]
use hytale_unpack::image::{decode_png, encode_png, tint};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Ok(img) = decode_png(data) else {
        return;
    };
    let _ = img.has_alpha();
    let t = tint(&img, &img);
    let _ = t.has_alpha();
    let _ = encode_png(img.width, img.height, &img.data);
});
