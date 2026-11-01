//! Regression tests for what fuzzing (`fuzz/`) found: each one is a minimised input that made a
//! reader panic, overflow, hang or allocate far more than the input justifies. Synthetic data only.

use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};

use hytale_unpack::assets::Assets;

/// A file in the temp directory, removed on drop
struct Temp(PathBuf);

impl Temp {
    fn new(bytes: &[u8]) -> Temp {
        static N: AtomicUsize = AtomicUsize::new(0);
        let path = std::env::temp_dir().join(format!(
            "hytale-unpack-fuzzreg-{}-{}.bin",
            std::process::id(),
            N.fetch_add(1, Ordering::SeqCst)
        ));
        std::fs::write(&path, bytes).unwrap();
        Temp(path)
    }
}

impl Drop for Temp {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

/// A zip of one DEFLATE-compressed entry whose central directory and local header both claim
/// `claimed` bytes of content
fn zip_claiming(name: &str, content: &[u8], claimed: u32) -> Vec<u8> {
    let mut buf = Vec::new();
    {
        let mut zip = zip::ZipWriter::new(std::io::Cursor::new(&mut buf));
        let opts = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated);
        zip.start_file(name, opts).unwrap();
        zip.write_all(content).unwrap();
        zip.finish().unwrap();
    }
    let find = |sig: &[u8; 4]| buf.windows(4).position(|w| w == sig).unwrap();
    let (local, central) = (find(b"PK\x03\x04"), find(b"PK\x01\x02"));
    // uncompressed size: offset 22 of a local header, offset 24 of a central directory entry
    buf[local + 22..local + 26].copy_from_slice(&claimed.to_le_bytes());
    buf[central + 24..central + 28].copy_from_slice(&claimed.to_le_bytes());
    buf
}

/// Found by fuzzing (`fuzz/crashes/assets_zip/big-claim`): a 170-byte archive whose entry claims
/// 250 MiB made `Assets::read` reserve 250 MiB before reading a byte.
#[test]
fn an_entry_that_claims_a_huge_size_does_not_reserve_it() {
    let zip = Temp::new(&zip_claiming(
        "Common/A.blockymodel",
        b"{}{}{}{}",
        250 << 20,
    ));
    let assets = Assets::open(&zip.0).unwrap();
    let bytes = assets.read("Common/A.blockymodel").unwrap();
    assert!(bytes.len() <= 8);
    assert!(
        bytes.capacity() < 1 << 20,
        "reserved {} bytes for a {} byte entry",
        bytes.capacity(),
        bytes.len()
    );
}

#[test]
fn a_true_size_is_still_reserved_in_one_go() {
    let content = vec![b'x'; 5000];
    let zip = Temp::new(&zip_claiming("Common/B.blockymodel", &content, 5000));
    let assets = Assets::open(&zip.0).unwrap();
    assert_eq!(assets.read("Common/B.blockymodel").unwrap(), content);
}
