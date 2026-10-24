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

pub fn decode_png(bytes: &[u8]) -> Result<Rgba> {
    let mut dec = png::Decoder::new(Cursor::new(bytes));
    dec.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut reader = dec.read_info()?;
    let mut buf = vec![
        0;
        reader
            .output_buffer_size()
            .ok_or_else(|| anyhow::anyhow!("PNG too large"))?
    ];
    let info = reader.next_frame(&mut buf)?;
    buf.truncate(info.buffer_size());
    let (w, h) = (info.width, info.height);
    let px = (w * h) as usize;
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

    #[test]
    fn garbage_is_an_error() {
        assert!(decode_png(b"not a png").is_err());
    }
}
