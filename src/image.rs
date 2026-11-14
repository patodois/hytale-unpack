//! PNG decoding to 8-bit RGBA and encoding back, for textures.
//!
//! Hytale's textures are PNGs: 1,361 of 1,506 sampled are 8-bit RGBA, the rest 8-bit RGB or
//! palette. Sizes are multiples of 32 px (the modelling guide's rule) and are read from the file,
//! because UVs are in pixels of the texture the model is drawn with.
//!
//! [`decode_png`] does not trust the header for memory: the output a file may declare is bounded
//! by what a file of its length can decode to and by an absolute ceiling of 1 GiB of RGBA, and
//! the output grows with the rows that are really decoded, for interlaced images too.

use std::io::Cursor;

use anyhow::{bail, Result};

/// 8-bit RGBA, rows top to bottom.
#[derive(Clone, PartialEq, Eq)]
pub struct Rgba {
    pub width: u32,
    pub height: u32,
    pub data: Vec<u8>,
}

impl std::fmt::Debug for Rgba {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Rgba({}x{})", self.width, self.height)
    }
}

impl Rgba {
    /// Whether any texel is not fully opaque
    pub fn has_alpha(&self) -> bool {
        self.data.as_chunks::<4>().0.iter().any(|p| p[3] != 255)
    }
}

/// How many bytes a PNG of `n` bytes can decode to at most, whatever its header says.
///
/// The image data is zlib-compressed (PNG specification, section 10.3), i.e. DEFLATE, in which a
/// match costs at least two bits (a length/literal code and a distance code of one bit each, RFC
/// 1951 section 3.2.5) and copies at most 258 bytes: 1032 bytes of filtered scanlines per input
/// byte. Expanding to 8-bit RGBA multiplies that by at most 32 (a 1-bit palette index becomes
/// four bytes).
fn decodable_from(n: usize) -> usize {
    n.saturating_mul(1032 * 32)
}

/// Appends the texels of one decoded row (or of a whole frame) as RGBA
fn push_rgba(color: png::ColorType, raw: &[u8], out: &mut Vec<u8>) -> Result<()> {
    match color {
        png::ColorType::Rgba => out.extend_from_slice(raw),
        png::ColorType::Rgb => {
            for p in raw.as_chunks::<3>().0 {
                out.extend_from_slice(&[p[0], p[1], p[2], 255]);
            }
        }
        png::ColorType::Grayscale => {
            for &g in raw {
                out.extend_from_slice(&[g, g, g, 255]);
            }
        }
        png::ColorType::GrayscaleAlpha => {
            for p in raw.as_chunks::<2>().0 {
                out.extend_from_slice(&[p[0], p[0], p[0], p[1]]);
            }
        }
        png::ColorType::Indexed => bail!("palette PNG was not expanded"),
    }
    Ok(())
}

/// `size` zero bytes, or an error when they cannot be had (a failed allocation would abort)
fn zeroed(size: usize) -> Result<Vec<u8>> {
    let mut buf = Vec::new();
    buf.try_reserve_exact(size)
        .map_err(|_| anyhow::anyhow!("PNG too large: {size} bytes of output"))?;
    buf.resize(size, 0);
    Ok(buf)
}

/// The most RGBA output [`decode_png`] produces, in bytes: 1 GiB, a 16384 x 16384 image.
///
/// The ratio of [`decodable_from`] alone lets a file of a few megabytes decode to many gigabytes
/// (a single-colour image compresses that well), so there is an absolute ceiling too. It is set
/// from what the game ships: a run that loads every model, block and prefab the provider lists,
/// and so decodes every texture they use, peaks at 490 MB of memory for the whole process (see
/// "Numbers" and "Checking the provider" in the README), and the ceiling is more than twice
/// that for one image.
const MAX_DECODED_BYTES: u64 = 1 << 30;

/// The seven passes of Adam7 interlacing (PNG specification, section 8.2): the first column and
/// the first row of each pass, and its step in x and in y
const ADAM7: [(u32, u32, u32, u32); 7] = [
    (0, 0, 8, 8),
    (4, 0, 8, 8),
    (0, 4, 4, 8),
    (2, 0, 4, 4),
    (0, 2, 2, 4),
    (1, 0, 2, 2),
    (0, 1, 1, 2),
];

/// How many of `len` columns (or rows) a pass holds that starts at `first` and takes every
/// `step`-th
fn pass_len(len: u32, first: u32, step: u32) -> u32 {
    if len > first {
        (len - first).div_ceil(step)
    } else {
        0
    }
}

/// One texel of a decoded row as RGBA
type ToRgba = fn(&[u8]) -> [u8; 4];

/// Bytes per texel of a decoded row, and the texel as RGBA
fn texel(color: png::ColorType) -> Result<(usize, ToRgba)> {
    Ok(match color {
        png::ColorType::Rgba => (4, |p| [p[0], p[1], p[2], p[3]]),
        png::ColorType::Rgb => (3, |p| [p[0], p[1], p[2], 255]),
        png::ColorType::Grayscale => (1, |p| [p[0], p[0], p[0], 255]),
        png::ColorType::GrayscaleAlpha => (2, |p| [p[0], p[0], p[0], p[1]]),
        png::ColorType::Indexed => bail!("palette PNG was not expanded"),
    })
}

/// The RGBA frame of an interlaced image of `w` x `h` texels.
///
/// The passes fill the frame in turn, so the frame has to exist before the last pass is read.
/// It is not taken from the header, though: the rows are kept as they are decoded, pass after
/// pass, so the memory used grows with rows that really exist, and the frame is made once every
/// row has been read. A file that declares a big frame and then has no data for it costs what
/// its data decodes to, not the frame.
fn read_interlaced<R: std::io::BufRead + std::io::Seek>(
    reader: &mut png::Reader<R>,
    w: u32,
    h: u32,
    color: png::ColorType,
) -> Result<Vec<u8>> {
    let (n, rgba) = texel(color)?;
    let mut passes: Vec<u8> = Vec::new();
    for (x0, y0, dx, dy) in ADAM7 {
        let (pw, ph) = (pass_len(w, x0, dx), pass_len(h, y0, dy));
        if pw == 0 {
            continue;
        }
        for _ in 0..ph {
            let row = reader
                .next_row()?
                .ok_or_else(|| anyhow::anyhow!("interlaced PNG ends before its last row"))?;
            if row.data().len() != pw as usize * n {
                bail!(
                    "interlaced PNG row of {} bytes where a pass of {pw} texels has {}",
                    row.data().len(),
                    pw as usize * n
                );
            }
            passes.extend_from_slice(row.data());
        }
    }
    let width = w as usize;
    let mut data = zeroed(width * h as usize * 4)?;
    let mut texels = passes.chunks_exact(n);
    for (x0, y0, dx, dy) in ADAM7 {
        let (pw, ph) = (pass_len(w, x0, dx), pass_len(h, y0, dy));
        if pw == 0 {
            continue;
        }
        for j in 0..ph {
            let line = (y0 + j * dy) as usize * width;
            for (i, p) in texels.by_ref().take(pw as usize).enumerate() {
                let at = (line + x0 as usize + i * dx as usize) * 4;
                data[at..at + 4].copy_from_slice(&rgba(p));
            }
        }
    }
    Ok(data)
}

pub fn decode_png(bytes: &[u8]) -> Result<Rgba> {
    let mut dec = png::Decoder::new(Cursor::new(bytes));
    dec.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut reader = dec.read_info()?;
    // The header alone says how big the output is: believe it only as far as a file of this
    // length can decode
    let size = reader
        .output_buffer_size()
        .ok_or_else(|| anyhow::anyhow!("PNG too large"))?;
    let (w, h) = (reader.info().width, reader.info().height);
    if size > decodable_from(bytes.len()) {
        bail!(
            "PNG header declares {w} x {h} pixels, more than a {} byte file can hold",
            bytes.len()
        );
    }
    // ... and never beyond the ceiling, however long the file is. Counted in 64 bits: the
    // product of two 32-bit sizes does not fit a 32-bit `usize`
    let rgba = u64::from(w)
        .checked_mul(u64::from(h))
        .and_then(|px| px.checked_mul(4))
        .filter(|n| *n <= MAX_DECODED_BYTES)
        .ok_or_else(|| {
            anyhow::anyhow!(
                "PNG of {w} x {h} pixels is more than the {MAX_DECODED_BYTES} bytes of RGBA \
                 this reader decodes"
            )
        })? as usize;
    let (color, _) = reader.output_color_type();
    let data = if reader.info().interlaced {
        read_interlaced(&mut reader, w, h, color)?
    } else {
        // Rows are appended as they are decoded, so the output costs nothing beyond the rows that
        // exist. The `png` crate itself holds a row of the header's width (zero-filled, a few
        // times the row's bytes), which the checks above bound like the whole frame.
        let mut data = Vec::new();
        while let Some(row) = reader.next_row()? {
            push_rgba(color, row.data(), &mut data)?;
        }
        data
    };
    if data.len() != rgba {
        bail!("PNG decoded to {} bytes for {w}x{h}", data.len());
    }
    Ok(Rgba {
        width: w,
        height: h,
        data,
    })
}

/// Tints a greyscale texture with a gradient (`GradientSet` / `GradientId` in the definitions).
///
/// The gradients are 256 texels wide ramps from dark to light, and the tinted textures are
/// greyscale, so a grey texel of value `v` takes the gradient's colour at column `v` (scaled when
/// the gradient is not 256 wide), keeping its own alpha. Texels that are not grey (red, green and
/// blue differing) are painted colours and stay as they are: the textures mix both (the player's
/// skin is grey and its underwear blue). The gradient files are 13 to 16 rows high and the rows are not identical (163 of the 212 differ
/// from row 0, by up to 12 levels): reading row 0 is this crate's choice, nothing says which row
/// the game uses, and the rows are not averaged. A texel counts as grey only when red, green and
/// blue are exactly equal; a texel a level off stays as painted.
///
/// The data does not describe this mapping. It is inferred from the files (a ramp to index by
/// brightness) and was checked by eye against three of the game's own generated icons; see the
/// README.
///
/// A gradient with no texels, or whose `data` is shorter than the row its `width` declares (the
/// fields are public, so the two can disagree), tints nothing.
pub fn tint(texture: &Rgba, gradient: &Rgba) -> Rgba {
    let mut out = texture.clone();
    if gradient.width == 0 || gradient.height == 0 {
        return out;
    }
    let last = (gradient.width - 1) as usize;
    if gradient.data.len() / 4 <= last {
        return out;
    }
    for px in out.data.as_chunks_mut::<4>().0 {
        if px[0] != px[1] || px[1] != px[2] {
            continue;
        }
        let col = (px[0] as usize * last + 127) / 255;
        px[..3].copy_from_slice(&gradient.data[col * 4..col * 4 + 3]);
    }
    out
}

pub fn encode_png(width: u32, height: u32, rgba: &[u8]) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    {
        let mut enc = png::Encoder::new(&mut out, width, height);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        let mut w = enc.write_header()?;
        w.write_image_data(rgba)?;
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rgba_round_trips() {
        let px = vec![1, 2, 3, 255, 4, 5, 6, 0];
        let png = encode_png(2, 1, &px).unwrap();
        let img = decode_png(&png).unwrap();
        assert_eq!((img.width, img.height), (2, 1));
        assert_eq!(img.data, px);
        assert!(img.has_alpha());
    }

    fn ramp() -> Rgba {
        // 256 x 1: column x is (x, 0, 255 - x)
        let mut data = Vec::new();
        for x in 0..=255u8 {
            data.extend_from_slice(&[x, 0, 255 - x, 255]);
        }
        Rgba {
            width: 256,
            height: 1,
            data,
        }
    }

    #[test]
    fn tint_maps_grey_texels_through_the_gradient_and_keeps_colours_and_alpha() {
        let tex = Rgba {
            width: 3,
            height: 1,
            // grey 10 with alpha 200, grey 255, a painted colour
            data: vec![10, 10, 10, 200, 255, 255, 255, 255, 200, 100, 50, 255],
        };
        let t = tint(&tex, &ramp());
        assert_eq!(&t.data[0..4], &[10, 0, 245, 200]);
        assert_eq!(&t.data[4..8], &[255, 0, 0, 255]);
        assert_eq!(&t.data[8..12], &[200, 100, 50, 255]);
    }

    #[test]
    fn tint_scales_the_column_for_a_narrower_gradient() {
        let grad = Rgba {
            width: 2,
            height: 1,
            data: vec![1, 2, 3, 255, 9, 8, 7, 255],
        };
        let tex = Rgba {
            width: 2,
            height: 1,
            data: vec![0, 0, 0, 255, 255, 255, 255, 255],
        };
        let t = tint(&tex, &grad);
        assert_eq!(&t.data[0..3], &[1, 2, 3]);
        assert_eq!(&t.data[4..7], &[9, 8, 7]);
    }

    /// Found by review: `tint` indexed the gradient by its declared width and panicked when the
    /// data was shorter. `Rgba`'s fields are public, so a caller can build such a value.
    #[test]
    fn a_gradient_with_less_data_than_its_width_declares_tints_nothing() {
        let tex = Rgba {
            width: 2,
            height: 1,
            data: vec![0, 0, 0, 255, 255, 255, 255, 255],
        };
        let short = Rgba {
            width: 256,
            height: 1,
            data: vec![9, 9, 9, 255],
        };
        assert_eq!(tint(&tex, &short), tex);
        let empty = Rgba {
            width: 256,
            height: 16,
            data: vec![],
        };
        assert_eq!(tint(&tex, &empty), tex);
        // a whole first row is enough, as it always was: only row 0 is read
        let mut row = ramp();
        row.height = 16;
        assert_eq!(&tint(&tex, &row).data[4..7], &[255, 0, 0]);
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

    fn chunk(kind: &[u8; 4], body: &[u8]) -> Vec<u8> {
        let mut out = (body.len() as u32).to_be_bytes().to_vec();
        let mut crc_in = kind.to_vec();
        crc_in.extend_from_slice(body);
        out.extend_from_slice(&crc_in);
        out.extend_from_slice(&crc32(&crc_in).to_be_bytes());
        out
    }

    /// A PNG whose header declares `w` x `h` 8-bit RGBA pixels and whose data is a few bytes
    fn declaring(w: u32, h: u32) -> Vec<u8> {
        let mut ihdr = w.to_be_bytes().to_vec();
        ihdr.extend_from_slice(&h.to_be_bytes());
        ihdr.extend_from_slice(&[8, 6, 0, 0, 0]);
        let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
        png.extend(chunk(b"IHDR", &ihdr));
        png.extend(chunk(b"IDAT", &[0x78, 0x9c, 0x62, 0x00, 0x00]));
        png.extend(chunk(b"IEND", &[]));
        png
    }

    /// Found by fuzzing (`fuzz/crashes/png/huge-dimensions-alloc`: a header of 63234 x
    /// 4244635652 pixels in 46 bytes): `decode_png` asked for the whole output buffer, about a
    /// petabyte, before reading any data. The fuzz build of the `png` crate skips the CRC check,
    /// so the test uses a file with valid CRCs.
    #[test]
    fn a_header_that_declares_more_than_the_file_can_hold_is_an_error() {
        let err = decode_png(&declaring(63234, 4_244_635_652)).unwrap_err();
        assert!(format!("{err:#}").contains("more than a"), "{err:#}");
        // 16384 x 16384 RGBA is 1 GiB, a legal header, but not for a file of this length
        assert!(decode_png(&declaring(16384, 16384)).is_err());
    }

    /// Below the ceiling the bound is the ratio alone: a header one byte over
    /// `decodable_from(len)` is refused, one exactly at it is believed (and fails later, on its
    /// data)
    #[test]
    fn the_ratio_is_refused_just_above_and_believed_at_the_bound() {
        let len = declaring(1, 1).len();
        // one row of `w` RGBA pixels: 4 * w bytes
        let at = 33024 * len / 4;
        assert_eq!(at * 4, 33024 * len);
        let above = decode_png(&declaring(at as u32 + 1, 1)).unwrap_err();
        assert!(format!("{above:#}").contains("more than a"), "{above:#}");
        let on = decode_png(&declaring(at as u32, 1)).unwrap_err();
        assert!(!format!("{on:#}").contains("more than a"), "{on:#}");
    }

    #[test]
    fn a_frame_that_cannot_be_allocated_is_an_error_not_an_abort() {
        assert!(zeroed(usize::MAX / 2).is_err());
        assert_eq!(zeroed(3).unwrap(), [0, 0, 0]);
    }

    /// A non-interlaced image of `color` and `depth`, `raw` being its rows, through the
    /// encoder of the `png` crate
    fn encoded(
        color: png::ColorType,
        depth: png::BitDepth,
        w: u32,
        h: u32,
        raw: &[u8],
        palette: Option<Vec<u8>>,
    ) -> Vec<u8> {
        let mut out = Vec::new();
        {
            let mut enc = png::Encoder::new(&mut out, w, h);
            enc.set_color(color);
            enc.set_depth(depth);
            if let Some(p) = palette {
                enc.set_palette(p);
            }
            let mut writer = enc.write_header().unwrap();
            writer.write_image_data(raw).unwrap();
        }
        out
    }

    #[test]
    fn rgb_grey_grey_alpha_and_palette_images_become_rgba() {
        use png::{BitDepth::*, ColorType::*};
        // RGB: alpha is opaque
        let png = encoded(
            Rgb,
            Eight,
            2,
            2,
            &[1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12],
            None,
        );
        let img = decode_png(&png).unwrap();
        assert_eq!(
            img.data,
            [1, 2, 3, 255, 4, 5, 6, 255, 7, 8, 9, 255, 10, 11, 12, 255]
        );
        // Grey: the grey is repeated, alpha is opaque
        let png = encoded(Grayscale, Eight, 2, 2, &[10, 20, 30, 40], None);
        assert_eq!(
            decode_png(&png).unwrap().data,
            [10, 10, 10, 255, 20, 20, 20, 255, 30, 30, 30, 255, 40, 40, 40, 255]
        );
        // Grey and alpha: the alpha is the file's
        let png = encoded(GrayscaleAlpha, Eight, 2, 1, &[10, 100, 20, 200], None);
        assert_eq!(
            decode_png(&png).unwrap().data,
            [10, 10, 10, 100, 20, 20, 20, 200]
        );
        // 16 bits keep the high byte
        let png = encoded(Grayscale, Sixteen, 2, 1, &[0xab, 0xcd, 0x12, 0x34], None);
        assert_eq!(
            decode_png(&png).unwrap().data,
            [0xab, 0xab, 0xab, 255, 0x12, 0x12, 0x12, 255]
        );
        // A palette without transparency is RGB, with a 1-bit index
        let png = encoded(
            Indexed,
            One,
            2,
            1,
            &[0b0100_0000],
            Some(vec![9, 8, 7, 1, 2, 3]),
        );
        assert_eq!(decode_png(&png).unwrap().data, [9, 8, 7, 255, 1, 2, 3, 255]);
    }

    /// 4096 x 4096 RGBA is 64 MiB, which a file of a few dozen bytes cannot expand to (it would
    /// need about a million output bytes per input byte, DEFLATE and the RGBA expansion give at
    /// most 1032 x 32): the header is refused by the ratio, before any decoding
    #[test]
    fn the_ratio_of_the_format_refuses_a_header_no_file_that_short_can_fill() {
        let png = declaring(4096, 4096);
        assert!(png.len() * 1032 * 32 < 4096 * 4096 * 4);
        let err = decode_png(&png).unwrap_err();
        assert!(format!("{err:#}").contains("more than a"), "{err:#}");
    }

    #[test]
    fn a_big_solid_texture_still_decodes() {
        // 2048 x 2048 of one colour compresses to a few kilobytes
        let px = vec![7u8; 2048 * 2048 * 4];
        let png = encode_png(2048, 2048, &px).unwrap();
        assert!(png.len() < 100_000);
        assert_eq!(decode_png(&png).unwrap().data.len(), px.len());
    }

    /// Adler-32 of the zlib format (RFC 1950 section 8.2)
    fn adler32(data: &[u8]) -> u32 {
        let (mut a, mut b) = (1u32, 0u32);
        for &x in data {
            a = (a + x as u32) % 65521;
            b = (b + a) % 65521;
        }
        (b << 16) | a
    }

    /// A zlib stream of stored blocks (RFC 1950, RFC 1951 section 3.2.4), each of 65535 bytes at
    /// most
    fn zlib_stored(data: &[u8]) -> Vec<u8> {
        let mut out = vec![0x78, 0x01];
        let blocks: Vec<&[u8]> = if data.is_empty() {
            vec![data]
        } else {
            data.chunks(65535).collect()
        };
        for (i, block) in blocks.iter().enumerate() {
            out.push((i + 1 == blocks.len()) as u8); // final block or not, stored
            out.extend_from_slice(&(block.len() as u16).to_le_bytes());
            out.extend_from_slice(&(!(block.len() as u16)).to_le_bytes());
            out.extend_from_slice(block);
        }
        out.extend_from_slice(&adler32(data).to_be_bytes());
        out
    }

    /// DEFLATE bits, least significant bit first (RFC 1951 section 3.1.1)
    #[derive(Default)]
    struct Bits {
        out: Vec<u8>,
        acc: u8,
        n: u32,
    }

    impl Bits {
        fn bit(&mut self, b: u32) {
            self.acc |= (b as u8 & 1) << self.n;
            self.n += 1;
            if self.n == 8 {
                self.out.push(self.acc);
                (self.acc, self.n) = (0, 0);
            }
        }
        /// A value that is not a Huffman code: low bit first
        fn value(&mut self, v: u32, count: u32) {
            for i in 0..count {
                self.bit(v >> i);
            }
        }
        /// A Huffman code: most significant bit first
        fn code(&mut self, v: u32, len: u32) {
            for i in (0..len).rev() {
                self.bit(v >> i);
            }
        }
        fn finish(mut self) -> Vec<u8> {
            if self.n > 0 {
                self.out.push(self.acc);
            }
            self.out
        }
    }

    /// A zlib stream of `1 + 258 * k` zero bytes at the best ratio DEFLATE has: one dynamic
    /// Huffman block in which the literal 0 costs 2 bits, and each length-258 match at distance 1
    /// costs one bit for the length code and one for the distance code (RFC 1951 section 3.2.5),
    /// i.e. 1032 output bytes per input byte
    fn zlib_best_ratio_zeros(k: usize) -> Vec<u8> {
        let mut b = Bits::default();
        b.value(1, 1); // last block
        b.value(2, 2); // dynamic Huffman codes
        b.value(286 - 257, 5); // HLIT: 286 literal/length codes
        b.value(0, 5); // HDIST: 1 distance code
        b.value(14, 4); // HCLEN: 18 code length codes
                        // Lengths of the code length codes in the order 16 17 18 0 8 7 9 6 10 5 11 4 12 3 13 2 14
                        // 1: symbol 18 has 1 bit, symbols 2 and 1 have 2 bits
        for len in [0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 2, 0, 2] {
            b.value(len, 3);
        }
        // Canonical codes: 18 = 0, 1 = 10, 2 = 11
        // literal/length lengths: symbol 0 -> 2, 1..=255 -> 0, 256 -> 2, 257..=284 -> 0, 285 -> 1
        b.code(0b11, 2);
        b.code(0, 1);
        b.value(138 - 11, 7); // 138 zeros
        b.code(0, 1);
        b.value(117 - 11, 7); // 117 zeros
        b.code(0b11, 2);
        b.code(0, 1);
        b.value(28 - 11, 7); // 28 zeros
        b.code(0b10, 2);
        // distance lengths: symbol 0 -> 1
        b.code(0b10, 2);
        // Data. Literal/length codes: 285 = 0, 0 = 10, 256 = 11; distance code 0 = 0
        b.code(0b10, 2); // the first zero byte
        for _ in 0..k {
            b.code(0, 1); // length 258
            b.code(0, 1); // distance 1
        }
        b.code(0b11, 2); // end of block
        let mut out = vec![0x78, 0xda];
        out.extend(b.finish());
        out.extend_from_slice(&adler32(&vec![0; 1 + 258 * k]).to_be_bytes());
        out
    }

    /// A 1-bit palette image with a `tRNS` chunk expands to 32 output bytes per input pixel byte
    /// (one bit becomes four bytes), and the best DEFLATE stream gives 1032 pixel bytes per input
    /// byte: the widest expansion `decodable_from` allows for. It must pass, and the file must be
    /// so short that a factor of 28 would already refuse it (the file has fixed chunks that
    /// keep it from reaching 32 exactly).
    #[test]
    fn a_one_bit_palette_image_at_the_widest_expansion_decodes() {
        // 2064 pixels are 258 bytes, a row is 259 bytes with its filter byte; 5161 rows are
        // 5161 * 259 = 1 + 258 * 5181 bytes
        let (w, h, k) = (2064u32, 5161u32, 5181usize);
        let mut ihdr = w.to_be_bytes().to_vec();
        ihdr.extend_from_slice(&h.to_be_bytes());
        ihdr.extend_from_slice(&[1, 3, 0, 0, 0]); // 1-bit palette
        let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
        png.extend(chunk(b"IHDR", &ihdr));
        png.extend(chunk(b"PLTE", &[0, 0, 0, 255, 255, 255]));
        png.extend(chunk(b"tRNS", &[0, 255]));
        png.extend(chunk(b"IDAT", &zlib_best_ratio_zeros(k)));
        png.extend(chunk(b"IEND", &[]));
        let decoded = w as usize * h as usize * 4;
        assert!(
            png.len() * 1032 * 28 < decoded,
            "{} bytes is too long to need the factor of 32",
            png.len()
        );
        let img = decode_png(&png).unwrap();
        assert_eq!((img.width, img.height), (w, h));
        assert_eq!(img.data.len(), decoded);
        assert!(img.data.chunks(4).all(|p| p == [0, 0, 0, 0]));
    }

    /// An interlaced image is decoded pass by pass into a frame (Adam7, PNG specification
    /// section 8.2): a 3 x 3 grey image whose texels are 1 to 9 in reading order
    #[test]
    fn an_interlaced_image_is_put_back_in_reading_order() {
        // pass 1: (0,0); pass 4: (2,0); pass 5: (0,2) (2,2); pass 6: (1,0), (1,2) as two rows;
        // pass 7: row 1; passes 2 and 3 are empty. Each row starts with filter type 0.
        let raw = [
            0, 1, // pass 1
            0, 3, // pass 4
            0, 7, 9, // pass 5
            0, 2, 0, 8, // pass 6
            0, 4, 5, 6, // pass 7
        ];
        let mut ihdr = 3u32.to_be_bytes().to_vec();
        ihdr.extend_from_slice(&3u32.to_be_bytes());
        ihdr.extend_from_slice(&[8, 0, 0, 0, 1]); // 8-bit grey, interlace method 1
        let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
        png.extend(chunk(b"IHDR", &ihdr));
        png.extend(chunk(b"IDAT", &zlib_stored(&raw)));
        png.extend(chunk(b"IEND", &[]));
        let img = decode_png(&png).unwrap();
        let grey: Vec<u8> = img.data.chunks(4).map(|p| p[0]).collect();
        assert_eq!(grey, (1..=9).collect::<Vec<u8>>());
        assert!(img.data.chunks(4).all(|p| p[3] == 255));
    }

    #[test]
    fn garbage_is_an_error() {
        assert!(decode_png(b"not a png").is_err());
    }

    /// The interlaced path as it was before the passes were read row by row: the whole frame at
    /// once, through the `png` crate's own de-interlacing. The reference the row-wise path is
    /// compared with.
    fn decode_by_frame(bytes: &[u8]) -> Result<Rgba> {
        let mut dec = png::Decoder::new(Cursor::new(bytes));
        dec.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
        let mut reader = dec.read_info()?;
        let size = reader.output_buffer_size().unwrap();
        let mut buf = zeroed(size)?;
        let info = reader.next_frame(&mut buf)?;
        buf.truncate(info.buffer_size());
        let mut data = Vec::new();
        push_rgba(info.color_type, &buf, &mut data)?;
        if data.len() != info.width as usize * info.height as usize * 4 {
            bail!("PNG decoded to {} bytes", data.len());
        }
        Ok(Rgba {
            width: info.width,
            height: info.height,
            data,
        })
    }

    /// Samples per pixel of a PNG colour type code (PNG specification, section 11.2.2)
    fn samples(color: u8) -> u32 {
        match color {
            0 | 3 => 1,
            4 => 2,
            2 => 3,
            _ => 4,
        }
    }

    /// The pass rows of an Adam7 image of `w` x `h` as the specification stores them (section
    /// 8.2): each row a filter byte 0, then its pixels packed from the most significant bit.
    /// `px(x, y)` gives the `bits` bits of a pixel.
    fn adam7_rows(w: u32, h: u32, bits: u32, px: impl Fn(u32, u32) -> u64) -> Vec<u8> {
        let mut raw = Vec::new();
        for (x0, y0, dx, dy) in ADAM7 {
            let (pw, ph) = (pass_len(w, x0, dx), pass_len(h, y0, dy));
            if pw == 0 {
                continue;
            }
            for j in 0..ph {
                raw.push(0);
                let (mut acc, mut n) = (0u8, 0);
                for i in 0..pw {
                    let v = px(x0 + i * dx, y0 + j * dy);
                    for b in (0..bits).rev() {
                        acc = (acc << 1) | ((v >> b) & 1) as u8;
                        n += 1;
                        if n == 8 {
                            raw.push(acc);
                            (acc, n) = (0, 0);
                        }
                    }
                }
                if n > 0 {
                    raw.push(acc << (8 - n));
                }
            }
        }
        raw
    }

    /// An interlaced PNG around `raw` pass rows; `chunks` go between `IHDR` and `IDAT`
    fn adam7_png(w: u32, h: u32, depth: u8, color: u8, chunks: &[Vec<u8>], raw: &[u8]) -> Vec<u8> {
        let mut ihdr = w.to_be_bytes().to_vec();
        ihdr.extend_from_slice(&h.to_be_bytes());
        ihdr.extend_from_slice(&[depth, color, 0, 0, 1]);
        let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
        png.extend(chunk(b"IHDR", &ihdr));
        for c in chunks {
            png.extend_from_slice(c);
        }
        png.extend(chunk(b"IDAT", &zlib_stored(raw)));
        png.extend(chunk(b"IEND", &[]));
        png
    }

    /// Pixel values that differ from texel to texel: a 64-bit mix of the coordinates
    fn noise(x: u32, y: u32, bits: u32) -> u64 {
        let mut v = (u64::from(x) + 1).wrapping_mul(0x9e37_79b9_7f4a_7c15)
            ^ (u64::from(y) + 1).wrapping_mul(0xc2b2_ae3d_27d4_eb4f);
        v ^= v >> 29;
        v.wrapping_mul(0xbf58_476d_1ce4_e5b9) >> (64 - bits)
    }

    /// Every colour type and bit depth an interlaced PNG can have, with the chunks it needs
    fn interlaced_kinds() -> Vec<(u8, u8, Vec<Vec<u8>>)> {
        let palette = |depth: u8, transparent: usize| {
            let n = 1usize << depth;
            let rgb: Vec<u8> = (0..n * 3).map(|i| (i * 37 + 11) as u8).collect();
            let mut chunks = vec![chunk(b"PLTE", &rgb)];
            if transparent > 0 {
                let alpha: Vec<u8> = (0..transparent.min(n)).map(|i| (i * 90) as u8).collect();
                chunks.push(chunk(b"tRNS", &alpha));
            }
            chunks
        };
        let mut kinds: Vec<(u8, u8, Vec<Vec<u8>>)> = Vec::new();
        for depth in [1, 2, 4, 8, 16] {
            kinds.push((0, depth, vec![]));
        }
        // a grey and an RGB image with one transparent colour
        kinds.push((0, 8, vec![chunk(b"tRNS", &[0, 7])]));
        kinds.push((0, 2, vec![chunk(b"tRNS", &[0, 1])]));
        kinds.push((2, 8, vec![chunk(b"tRNS", &[0, 1, 0, 2, 0, 3])]));
        for depth in [8, 16] {
            kinds.push((2, depth, vec![]));
            kinds.push((4, depth, vec![]));
            kinds.push((6, depth, vec![]));
        }
        for depth in [1, 2, 4, 8] {
            kinds.push((3, depth, palette(depth, 0)));
            kinds.push((3, depth, palette(depth, 3)));
        }
        kinds
    }

    /// The row-wise path gives exactly what the whole-frame path gave, for every colour type,
    /// bit depth and size: sizes 1 to 17 cover every combination of empty and partial passes
    #[test]
    fn an_interlaced_image_decodes_row_by_row_to_the_same_texels_as_frame_by_frame() {
        let mut sizes: Vec<(u32, u32)> = (1..=17)
            .flat_map(|w| (1..=17).map(move |h| (w, h)))
            .collect();
        sizes.extend([(64, 48), (33, 1), (1, 33), (100, 7), (7, 100)]);
        let mut compared = 0;
        for (color, depth, chunks) in interlaced_kinds() {
            let bits = u32::from(depth) * samples(color);
            for &(w, h) in &sizes {
                let raw = adam7_rows(w, h, bits, |x, y| noise(x, y, bits));
                let png = adam7_png(w, h, depth, color, &chunks, &raw);
                let want = decode_by_frame(&png)
                    .unwrap_or_else(|e| panic!("colour {color} depth {depth} {w}x{h}: {e:#}"));
                let got = decode_png(&png)
                    .unwrap_or_else(|e| panic!("colour {color} depth {depth} {w}x{h}: {e:#}"));
                assert_eq!((got.width, got.height), (w, h));
                assert!(
                    got == want,
                    "colour {color} depth {depth} {w}x{h} differs from the whole-frame decode"
                );
                compared += 1;
            }
        }
        assert_eq!(compared, 22 * (17 * 17 + 5));
    }

    /// The texels land where the file's own coordinates put them, not only where the reference
    /// decoder puts them: 8-bit RGBA holds the texel as it is written
    #[test]
    fn an_interlaced_rgba_image_holds_each_texel_at_its_own_place() {
        let (w, h) = (13u32, 9u32);
        let raw = adam7_rows(w, h, 32, |x, y| noise(x, y, 32));
        let img = decode_png(&adam7_png(w, h, 8, 6, &[], &raw)).unwrap();
        for y in 0..h {
            for x in 0..w {
                let at = ((y * w + x) * 4) as usize;
                assert_eq!(
                    img.data[at..at + 4],
                    (noise(x, y, 32) as u32).to_be_bytes(),
                    "texel {x},{y}"
                );
            }
        }
    }

    /// An interlaced image whose data stops early is an error in both paths, wherever it stops
    #[test]
    fn an_interlaced_image_cut_short_is_an_error() {
        let (w, h) = (9u32, 9u32);
        let raw = adam7_rows(w, h, 8, |x, y| noise(x, y, 8));
        for keep in [0, 1, 2, 5, raw.len() / 2, raw.len() - 1] {
            let png = adam7_png(w, h, 8, 0, &[], &raw[..keep]);
            assert!(decode_by_frame(&png).is_err(), "{keep} bytes");
            assert!(decode_png(&png).is_err(), "{keep} bytes");
        }
        assert!(decode_png(&adam7_png(w, h, 8, 0, &[], &raw)).is_ok());
    }

    /// A PNG whose header declares `w` x `h` 8-bit RGBA pixels and whose data is `rows` bytes of
    /// zeros: the first rows of the image, and nothing after them
    fn declaring_with_rows(w: u32, h: u32, interlaced: bool, rows: usize) -> Vec<u8> {
        let mut ihdr = w.to_be_bytes().to_vec();
        ihdr.extend_from_slice(&h.to_be_bytes());
        ihdr.extend_from_slice(&[8, 6, 0, 0, interlaced as u8]);
        let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
        png.extend(chunk(b"IHDR", &ihdr));
        png.extend(chunk(b"IDAT", &zlib_stored(&vec![0; rows])));
        png.extend(chunk(b"IEND", &[]));
        png
    }

    /// Found by review: the ratio was the only bound, so a file of a few megabytes could decode
    /// to many gigabytes. A header over the ceiling is refused before any row is decoded, however
    /// long the file is; one exactly at the ceiling is believed (and fails later, on its data).
    #[test]
    fn a_header_over_the_ceiling_is_refused_whatever_the_length_of_the_file() {
        // 16384 x 16384 RGBA is the ceiling, 1 GiB; 40 KB of data are enough for the ratio
        for interlaced in [false, true] {
            let over = declaring_with_rows(16385, 16384, interlaced, 40_000);
            assert!(16385 * 16384 * 4 <= decodable_from(over.len()));
            let err = decode_png(&over).unwrap_err();
            assert!(
                format!("{err:#}").contains("this reader decodes"),
                "{err:#}"
            );
            let at = declaring_with_rows(16384, 16384, interlaced, 40_000);
            let err = decode_png(&at).unwrap_err();
            assert!(
                !format!("{err:#}").contains("this reader decodes"),
                "{err:#}"
            );
        }
        // sizes whose product does not fit 64 bits are refused too, by the ratio first
        assert!(decode_png(&declaring(u32::MAX >> 1, u32::MAX >> 1)).is_err());
    }
}
