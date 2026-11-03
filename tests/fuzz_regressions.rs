//! Regression tests for what fuzzing (`fuzz/`) found: each one is a minimised input that made a
//! reader panic, overflow, hang or allocate far more than the input justifies. Synthetic data only.

use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};

use hytale_unpack::assets::Assets;

/// A global allocator that remembers the biggest single request made by the thread that asked
/// for it, so a test can see what a call reserved even when the call then fails
mod counting {
    use std::alloc::{GlobalAlloc, Layout, System};
    use std::cell::Cell;

    struct Counting;

    thread_local! {
        static LARGEST: Cell<Option<usize>> = const { Cell::new(None) };
    }

    fn note(size: usize) {
        LARGEST.with(|l| {
            if let Some(m) = l.get() {
                l.set(Some(m.max(size)));
            }
        });
    }

    unsafe impl GlobalAlloc for Counting {
        unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
            note(layout.size());
            System.alloc(layout)
        }
        unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
            note(layout.size());
            System.alloc_zeroed(layout)
        }
        unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
            System.dealloc(ptr, layout)
        }
        unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
            note(new_size);
            System.realloc(ptr, layout, new_size)
        }
    }

    #[global_allocator]
    static ALLOC: Counting = Counting;

    /// The biggest single allocation `f` made on this thread
    pub fn largest_allocation(f: impl FnOnce()) -> usize {
        LARGEST.with(|l| l.set(Some(0)));
        f();
        LARGEST.with(|l| l.replace(None)).unwrap_or(0)
    }
}

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

/// A zip of one entry of `content`, compressed with `method`, whose central directory and local
/// header both claim `claimed` bytes for the content and `claimed_compressed` (if given) for the
/// stored bytes
fn zip_claiming(
    name: &str,
    content: &[u8],
    method: zip::CompressionMethod,
    claimed: u32,
    claimed_compressed: Option<u32>,
) -> Vec<u8> {
    let mut buf = Vec::new();
    {
        let mut zip = zip::ZipWriter::new(std::io::Cursor::new(&mut buf));
        let opts = zip::write::SimpleFileOptions::default().compression_method(method);
        zip.start_file(name, opts).unwrap();
        zip.write_all(content).unwrap();
        zip.finish().unwrap();
    }
    let find = |sig: &[u8; 4]| buf.windows(4).position(|w| w == sig).unwrap();
    let (local, central) = (find(b"PK\x03\x04"), find(b"PK\x01\x02"));
    // compressed size: offset 18 of a local header, offset 20 of a central directory entry;
    // uncompressed size: offset 22 and 24
    if let Some(c) = claimed_compressed {
        buf[local + 18..local + 22].copy_from_slice(&c.to_le_bytes());
        buf[central + 20..central + 24].copy_from_slice(&c.to_le_bytes());
    }
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
        zip::CompressionMethod::Deflated,
        250 << 20,
        None,
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
    let zip = Temp::new(&zip_claiming(
        "Common/B.blockymodel",
        &content,
        zip::CompressionMethod::Deflated,
        5000,
        None,
    ));
    let assets = Assets::open(&zip.0).unwrap();
    let bytes = assets.read("Common/B.blockymodel").unwrap();
    assert_eq!(bytes, content);
    // the true size was reserved at once: the vector never had to grow past it
    assert_eq!(bytes.capacity(), 5000);
}

/// Found by review: `Assets.zip` stores its entries, and for a stored entry the compressed size
/// is as untrusted as the uncompressed one. An attacker claims the same big number for both; the
/// bytes after the start of the data are all the entry can have.
#[test]
fn a_stored_entry_that_claims_a_huge_size_for_both_sizes_does_not_reserve_it() {
    let zip = Temp::new(&zip_claiming(
        "Common/C.blockymodel",
        b"{}{}{}{}",
        zip::CompressionMethod::Stored,
        250 << 20,
        Some(250 << 20),
    ));
    let assets = Assets::open(&zip.0).unwrap();
    // the read fails (the checksum does not match the claimed content), what matters is the
    // memory taken on the way
    let peak = counting::largest_allocation(|| {
        let _ = assets.read("Common/C.blockymodel");
    });
    assert!(peak < 1 << 20, "reserved {peak} bytes for a 8 byte entry");
}

/// The same claim on a DEFLATE entry: it is bounded by the file too, and a stored entry gets no
/// expansion factor at all
#[test]
fn a_stored_entry_reserves_no_more_than_it_stores() {
    let zip = Temp::new(&zip_claiming(
        "Common/D.blockymodel",
        &vec![b'x'; 100],
        zip::CompressionMethod::Stored,
        1 << 20,
        Some(1 << 20),
    ));
    let assets = Assets::open(&zip.0).unwrap();
    let peak = counting::largest_allocation(|| {
        let _ = assets.read("Common/D.blockymodel");
    });
    // the file is about 300 bytes; with the DEFLATE factor the claim would be reserved up to 1032
    // times that
    assert!(peak < 1 << 12, "reserved {peak} bytes for a stored entry");
}
