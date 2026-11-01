#![no_main]
use hytale_unpack::defs::{merge, AttachmentDef, BlockDesc, GradientRef, GradientSets, ModelDef};
use libfuzzer_sys::fuzz_target;
use serde_json::Value;

fuzz_target!(|data: &[u8]| {
    let Ok(v) = serde_json::from_slice::<Value>(data) else {
        return;
    };
    let _ = merge(&v, &v);
    let _ = merge(&Value::Null, &v);
    let sets = GradientSets::from_json(&v);
    let _ = sets.len();
    let def = ModelDef::from_json(&v);
    let _ = def.gradient_paired();
    for a in def.attachments.iter() {
        let _ = (a.gradient(), a.half_gradient());
    }
    if let Some(g) = &def.gradient {
        let _ = sets.texture(g);
    }
    let _ = GradientRef::from_json(&v);
    let _ = AttachmentDef::from_json(&v);
    let b = BlockDesc::from_json(&v);
    let _ = b.draws();
});
