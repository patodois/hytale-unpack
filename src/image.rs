//! PNG decoding to 8-bit RGBA and encoding back, for textures.
//!
//! Hytale's textures are PNGs: 1,361 of 1,506 sampled are 8-bit RGBA, the rest 8-bit RGB or
//! palette. Sizes are multiples of 32 px (the modelling guide's rule) and are read from the file,
//! because UVs are in pixels of the texture the model is drawn with.

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

/// The most a decoded texture may take (RGBA, 8 bits): 128 MiB, a 5792 x 5792 image. The biggest
/// textures of the game are far below it.
const MAX_DECODED: usize = 128 << 20;

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

pub fn decode_png(bytes: &[u8]) -> Result<Rgba> {
    let mut dec = png::Decoder::new(Cursor::new(bytes));
    dec.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut reader = dec.read_info()?;
    // The header alone says how big the output is; do not allocate it before it is believable
    let size = reader
        .output_buffer_size()
        .ok_or_else(|| anyhow::anyhow!("PNG too large"))?;
    if size > MAX_DECODED || size > decodable_from(bytes.len()) {
        bail!(
            "PNG header declares {} x {} pixels, more than a {} byte file can hold",
            reader.info().width,
            reader.info().height,
            bytes.len()
        );
    }
    let mut buf = vec![0; size];
    let info = reader.next_frame(&mut buf)?;
    buf.truncate(info.buffer_size());
    let (w, h) = (info.width, info.height);
    let px = w as usize * h as usize;
    let data = match info.color_type {
        png::ColorType::Rgba => buf,
        png::ColorType::Rgb => {
            let mut out = Vec::with_capacity(px * 4);
            for p in buf.as_chunks::<3>().0 {
                out.extend_from_slice(&[p[0], p[1], p[2], 255]);
            }
            out
        }
        png::ColorType::Grayscale => {
            let mut out = Vec::with_capacity(px * 4);
            for &g in &buf {
                out.extend_from_slice(&[g, g, g, 255]);
            }
            out
        }
        png::ColorType::GrayscaleAlpha => {
            let mut out = Vec::with_capacity(px * 4);
            for p in buf.as_chunks::<2>().0 {
                out.extend_from_slice(&[p[0], p[0], p[0], p[1]]);
            }
            out
        }
        png::ColorType::Indexed => bail!("palette PNG was not expanded"),
    };
    if data.len() != px * 4 {
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
pub fn tint(texture: &Rgba, gradient: &Rgba) -> Rgba {
    let mut out = texture.clone();
    if gradient.width == 0 || gradient.height == 0 {
        return out;
    }
    let last = (gradient.width - 1) as usize;
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
        // 16384 x 16384 RGBA is 1 GiB: past the cap, though a legal PNG header
        assert!(decode_png(&declaring(16384, 16384)).is_err());
    }

    #[test]
    fn a_big_solid_texture_still_decodes() {
        // 2048 x 2048 of one colour compresses to a few kilobytes
        let px = vec![7u8; 2048 * 2048 * 4];
        let png = encode_png(2048, 2048, &px).unwrap();
        assert!(png.len() < 100_000);
        assert_eq!(decode_png(&png).unwrap().data.len(), px.len());
    }

    #[test]
    fn garbage_is_an_error() {
        assert!(decode_png(b"not a png").is_err());
    }
}
