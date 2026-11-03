#![no_main]
use std::io::Write;

use hytale_unpack::assets::Assets;
use hytale_unpack::defs::Defs;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    // RAM disk where there is one (Linux), the temp directory elsewhere (macOS)
    let base = if std::path::Path::new("/dev/shm").is_dir() {
        std::path::PathBuf::from("/dev/shm")
    } else {
        std::env::temp_dir()
    };
    let path = base.join(format!("hytale-fuzz-zip-{}.zip", std::process::id()));
    std::fs::File::create(&path).unwrap().write_all(data).unwrap();
    if let Ok(a) = Assets::open(&path) {
        let names: Vec<String> = a.names().map(String::from).collect();
        for n in names.iter().take(64) {
            let _ = a.size(n);
            let _ = a.resolve(n);
            let _ = a.read(n);
            let _ = a.with_reader(n, |r| {
                let mut buf = [0u8; 4096];
                std::io::Read::read(r, &mut buf)?;
                Ok(())
            });
        }
        let _ = a.names_under("Common/").count();
        let defs = Defs::new(&a);
        for p in defs.model_paths().map(String::from).collect::<Vec<_>>().iter().take(16) {
            let _ = defs.model_def(&a, p);
        }
        let _ = defs.gradient_sets(&a);
        let _ = defs.block(&a, "x");
        let _ = hytale_unpack::check::run(&a, |_, _, _| {});
    }
    let _ = std::fs::remove_file(&path);
});
