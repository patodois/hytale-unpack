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
        self.data.chunks_exact(4).any(|p| p[3] != 255)
    }
}

pub fn decode_png(bytes: &[u8]) -> Result<Rgba> {
    let mut dec = png::Decoder::new(Cursor::new(bytes));
    dec.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut reader = dec.read_info()?;
    let mut buf = vec![0; reader.output_buffer_size().ok_or_else(|| anyhow::anyhow!("PNG too large"))?];
    let info = reader.next_frame(&mut buf)?;
    buf.truncate(info.buffer_size());
    let (w, h) = (info.width, info.height);
    let px = (w * h) as usize;
    let data = match info.color_type {
        png::ColorType::Rgba => buf,
        png::ColorType::Rgb => {
            let mut out = Vec::with_capacity(px * 4);
            for p in buf.chunks_exact(3) {
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
            for p in buf.chunks_exact(2) {
                out.extend_from_slice(&[p[0], p[0], p[0], p[1]]);
            }
            out
        }
        png::ColorType::Indexed => bail!("palette PNG was not expanded"),
    };
    if data.len() != px * 4 {
        bail!("PNG decoded to {} bytes for {w}x{h}", data.len());
    }
    Ok(Rgba { width: w, height: h, data })
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

    #[test]
    fn garbage_is_an_error() {
        assert!(decode_png(b"not a png").is_err());
    }
}
