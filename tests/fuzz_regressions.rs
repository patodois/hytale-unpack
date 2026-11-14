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
/// stored bytes. `pad` bytes of a second, stored entry follow the first one, so that the file
/// is longer than the entry (the length of the file is a bound the reader uses).
fn zip_claiming(
    name: &str,
    content: &[u8],
    method: zip::CompressionMethod,
    claimed: u32,
    claimed_compressed: Option<u32>,
    pad: usize,
) -> Vec<u8> {
    let mut buf = Vec::new();
    {
        let mut zip = zip::ZipWriter::new(std::io::Cursor::new(&mut buf));
        let opts = zip::write::SimpleFileOptions::default().compression_method(method);
        zip.start_file(name, opts).unwrap();
        zip.write_all(content).unwrap();
        if pad > 0 {
            let stored = zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Stored);
            zip.start_file("Common/Pad.bin", stored).unwrap();
            zip.write_all(&vec![0u8; pad]).unwrap();
        }
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
/// 250 MiB made `Assets::read` reserve 250 MiB before reading a byte. Found by review: the
/// entry's own compressed size bounds it too, in a file that is longer than the entry (here a
/// 1 MiB stored entry follows), where the length of the file alone would allow it.
#[test]
fn an_entry_that_claims_a_huge_size_does_not_reserve_it() {
    let zip = Temp::new(&zip_claiming(
        "Common/A.blockymodel",
        b"{}{}{}{}",
        zip::CompressionMethod::Deflated,
        250 << 20,
        None,
        1 << 20,
    ));
    let assets = Assets::open(&zip.0).unwrap();
    let mut bytes = Vec::new();
    let largest = counting::largest_allocation(|| {
        bytes = assets.read("Common/A.blockymodel").unwrap();
    });
    assert!(bytes.len() <= 8);
    assert!(
        largest < 1 << 20,
        "allocated {largest} bytes at once for a {} byte entry",
        bytes.len()
    );
}

/// Bytes that DEFLATE cannot shrink: a linear congruential sequence
fn incompressible(n: usize) -> Vec<u8> {
    let mut x = 12345u32;
    (0..n)
        .map(|_| {
            x = x.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            (x >> 24) as u8
        })
        .collect()
}

/// A DEFLATE stream expands by 1032 at most: an entry whose stored bytes are `c` long reserves
/// `c * 1032` however much it claims, and not a byte more
#[test]
fn a_deflate_entry_reserves_at_most_1032_times_what_it_stores() {
    let zip_bytes = zip_claiming(
        "Common/E.blockymodel",
        &incompressible(4100),
        zip::CompressionMethod::Deflated,
        250 << 20,
        None,
        0,
    );
    let central = zip_bytes
        .windows(4)
        .position(|w| w == b"PK\x01\x02")
        .unwrap();
    let compressed =
        u32::from_le_bytes(zip_bytes[central + 20..central + 24].try_into().unwrap()) as usize;
    let zip = Temp::new(&zip_bytes);
    let assets = Assets::open(&zip.0).unwrap();
    let largest = counting::largest_allocation(|| {
        let _ = assets.read("Common/E.blockymodel");
    });
    assert_eq!(largest, compressed * 1032);
}

/// The stored bytes of an entry are the ones after the start of its data, not the whole file:
/// the header before it does not count
#[test]
fn the_bytes_before_the_data_do_not_count_as_stored() {
    let content = vec![b'z'; 200 << 10];
    let zip_bytes = zip_claiming(
        "Common/F.blockymodel",
        &content,
        zip::CompressionMethod::Stored,
        (300 << 10) as u32,
        Some((300 << 10) as u32),
        0,
    );
    let local = zip_bytes
        .windows(4)
        .position(|w| w == b"PK\x03\x04")
        .unwrap();
    let name_len = u16::from_le_bytes([zip_bytes[local + 26], zip_bytes[local + 27]]) as usize;
    let extra_len = u16::from_le_bytes([zip_bytes[local + 28], zip_bytes[local + 29]]) as usize;
    let data_start = local + 30 + name_len + extra_len;
    let file_len = zip_bytes.len();
    let zip = Temp::new(&zip_bytes);
    let assets = Assets::open(&zip.0).unwrap();
    let largest = counting::largest_allocation(|| {
        let _ = assets.read("Common/F.blockymodel");
    });
    assert_eq!(largest, file_len - data_start);
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
        0,
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
        0,
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
        &[b'x'; 100],
        zip::CompressionMethod::Stored,
        1 << 20,
        Some(1 << 20),
        0,
    ));
    let assets = Assets::open(&zip.0).unwrap();
    let peak = counting::largest_allocation(|| {
        let _ = assets.read("Common/D.blockymodel");
    });
    // the file is about 300 bytes; with the DEFLATE factor the claim would be reserved up to 1032
    // times that
    assert!(peak < 1 << 12, "reserved {peak} bytes for a stored entry");
}

/// Found by review: the streaming reader of `Assets::with_reader` (what `check` and `prefab` read
/// prefabs through) had no bound at all, so a prefab of a few hundred kilobytes that inflates to
/// gigabytes cost unbounded time, and unbounded memory in a parser that keeps what it reads. The
/// entry here holds 1 GiB of zeros in about 1 MB and claims 16 bytes; the reader must fail after
/// the 256 MiB every other read is held to, not hand all of it over.
#[test]
fn a_small_entry_that_inflates_to_a_gigabyte_is_refused_by_the_streaming_reader() {
    const LIMIT: u64 = 256 << 20;
    let mut buf = Vec::new();
    {
        let mut zip = zip::ZipWriter::new(std::io::Cursor::new(&mut buf));
        let opts = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated);
        zip.start_file("Server/Prefabs/Bomb.prefab.json", opts)
            .unwrap();
        let chunk = vec![0u8; 1 << 20];
        for _ in 0..1024 {
            zip.write_all(&chunk).unwrap();
        }
        zip.finish().unwrap();
    }
    assert!(buf.len() < 2 << 20, "{} bytes", buf.len());
    let find = |sig: &[u8; 4]| buf.windows(4).position(|w| w == sig).unwrap();
    let (local, central) = (find(b"PK\x03\x04"), find(b"PK\x01\x02"));
    buf[local + 22..local + 26].copy_from_slice(&16u32.to_le_bytes());
    buf[central + 24..central + 28].copy_from_slice(&16u32.to_le_bytes());
    let zip = Temp::new(&buf);
    let assets = Assets::open(&zip.0).unwrap();
    assert_eq!(assets.size("Server/Prefabs/Bomb.prefab.json"), Some(16));
    let mut handed_over = 0u64;
    let result = assets.with_reader("Server/Prefabs/Bomb.prefab.json", |r| {
        let mut chunk = vec![0u8; 1 << 16];
        loop {
            match r.read(&mut chunk) {
                Ok(0) => return Ok(()),
                Ok(n) => handed_over += n as u64,
                Err(e) => return Err(e.into()),
            }
        }
    });
    assert_eq!(handed_over, LIMIT, "bytes the reader handed over");
    let err = result.unwrap_err();
    assert!(format!("{err:#}").contains("expands beyond"), "{err:#}");
}

use hytale_unpack::image::{decode_png, encode_png};

/// Found by review: a valid 6000 x 6000 PNG (152 KB, 144 MB decoded) was refused by a cap of
/// 128 MiB that nothing in the format justifies
#[test]
fn a_big_valid_png_decodes() {
    let px = vec![7u8; 6000 * 6000 * 4];
    let png = encode_png(6000, 6000, &px).unwrap();
    let img = decode_png(&png).unwrap();
    assert_eq!((img.width, img.height), (6000, 6000));
    assert!(img.data == px);
}

/// A header can pass the ratio check (a file of this length *could* hold the image) and still
/// lie. The output must then grow with the rows that exist, not be reserved from the header.
#[test]
fn a_png_cut_short_does_not_reserve_what_its_header_claims() {
    let px = vec![7u8; 4096 * 4096 * 4];
    let png = encode_png(4096, 4096, &px).unwrap();
    // the image data starts after the signature (8), IHDR (25) and the chunk header (8); keep
    // 2200 bytes of it: with those 41 the file is long enough for the ratio check (64 MiB /
    // (1032 * 32) = 2033 bytes), and far shorter than the whole image
    let cut = &png[..8 + 25 + 8 + 2200];
    let mut result = None;
    let largest = counting::largest_allocation(|| result = Some(decode_png(cut)));
    assert!(result.unwrap().is_err());
    let claimed = 4096 * 4096 * 4;
    assert!(
        largest < claimed / 4,
        "allocated {largest} bytes at once for a header that claims {claimed}"
    );
}

/// CRC-32 of the PNG specification (section 5.5), bit by bit
fn crc32(data: &[u8]) -> u32 {
    let mut c = 0xffff_ffffu32;
    for &b in data {
        c ^= b as u32;
        for _ in 0..8 {
            c = if c & 1 != 0 {
                0xedb8_8320 ^ (c >> 1)
            } else {
                c >> 1
            };
        }
    }
    !c
}

fn png_chunk(kind: &[u8; 4], body: &[u8]) -> Vec<u8> {
    let mut out = (body.len() as u32).to_be_bytes().to_vec();
    let mut crc_in = kind.to_vec();
    crc_in.extend_from_slice(body);
    out.extend_from_slice(&crc_in);
    out.extend_from_slice(&crc32(&crc_in).to_be_bytes());
    out
}

/// Found by review: for an interlaced image the whole frame its header declares was reserved and
/// filled with zeros before a single row was decoded, so a file of 2 MB that declares 60,000 x
/// 60,000 pixels took 14 GB on its way to a decode error. Here: 8192 x 8192 RGBA, interlaced (a
/// 256 MiB frame), in a file of 57 KB, long enough for the ratio check, whose data is the first
/// 14 rows of the first pass and nothing more. The memory taken must follow the rows that exist.
#[test]
fn an_interlaced_png_does_not_reserve_the_frame_its_header_claims() {
    let (w, h) = (8192u32, 8192u32);
    let mut ihdr = w.to_be_bytes().to_vec();
    ihdr.extend_from_slice(&h.to_be_bytes());
    // 8-bit RGBA, interlace method 1 (Adam7)
    ihdr.extend_from_slice(&[8, 6, 0, 0, 1]);
    // a row of the first pass is a filter byte and every eighth texel of the row
    let rows = vec![0u8; 14 * (1 + 4 * 1024)];
    // one stored DEFLATE block in a zlib stream (RFC 1950, RFC 1951 section 3.2.4); the
    // Adler-32 of zeros is 1 in the low half and their count in the high half
    let mut zlib = vec![0x78, 0x01, 1];
    zlib.extend_from_slice(&(rows.len() as u16).to_le_bytes());
    zlib.extend_from_slice(&(!(rows.len() as u16)).to_le_bytes());
    zlib.extend_from_slice(&rows);
    zlib.extend_from_slice(&(((rows.len() as u32 % 65521) << 16) | 1).to_be_bytes());
    let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
    png.extend(png_chunk(b"IHDR", &ihdr));
    png.extend(png_chunk(b"IDAT", &zlib));
    png.extend(png_chunk(b"IEND", &[]));
    let claimed = w as usize * h as usize * 4;
    assert!(png.len() * 1032 * 32 >= claimed, "the ratio check passes");
    let mut result = None;
    let largest = counting::largest_allocation(|| result = Some(decode_png(&png)));
    let err = result.unwrap().unwrap_err();
    assert!(!format!("{err:#}").contains("more than"), "{err:#}");
    assert!(
        largest < claimed / 64,
        "allocated {largest} bytes at once for a header that claims {claimed}"
    );
}
