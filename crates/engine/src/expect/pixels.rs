//! Pixel comparison for `expect-screenshot`: pixelmatch's perceptual
//! colour distance (YIQ, pixels blended over white by their alpha), without
//! its anti-aliasing detection. A baseline is taken on the same platform
//! with the same Chrome, where rendering is deterministic, so a pixel
//! either changed or did not.

use std::io::Cursor;

/// An 8-bit RGBA image.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Image {
    pub(crate) width: u32,
    pub(crate) height: u32,
    rgba: Vec<u8>,
}

/// The largest YIQ distance two colours can have (black and white).
const MAX_DELTA: f64 = 35215.0;

impl Image {
    /// Decodes a PNG of any colour type and depth.
    pub(crate) fn decode(bytes: &[u8]) -> Result<Self, String> {
        let mut decoder = png::Decoder::new(Cursor::new(bytes));
        decoder.set_transformations(png::Transformations::normalize_to_color8());
        let mut reader = decoder.read_info().map_err(|e| e.to_string())?;
        let size = reader
            .output_buffer_size()
            .ok_or_else(|| "the image is too large".to_owned())?;
        let mut buf = vec![0; size];
        let info = reader.next_frame(&mut buf).map_err(|e| e.to_string())?;
        buf.truncate(info.buffer_size());
        let rgba = match info.color_type {
            png::ColorType::Rgba => buf,
            png::ColorType::Rgb => buf
                .as_chunks::<3>()
                .0
                .iter()
                .flat_map(|&[r, g, b]| [r, g, b, 255])
                .collect(),
            png::ColorType::Grayscale => buf.iter().flat_map(|&g| [g, g, g, 255]).collect(),
            png::ColorType::GrayscaleAlpha => buf
                .as_chunks::<2>()
                .0
                .iter()
                .flat_map(|&[g, a]| [g, g, g, a])
                .collect(),
            png::ColorType::Indexed => return Err("an indexed PNG was not expanded".to_owned()),
        };
        Ok(Self {
            width: info.width,
            height: info.height,
            rgba,
        })
    }

    /// Encodes the image as a PNG.
    pub(crate) fn encode(&self) -> Result<Vec<u8>, String> {
        let mut out = Vec::new();
        let mut encoder = png::Encoder::new(&mut out, self.width, self.height);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header().map_err(|e| e.to_string())?;
        writer
            .write_image_data(&self.rgba)
            .map_err(|e| e.to_string())?;
        writer.finish().map_err(|e| e.to_string())?;
        Ok(out)
    }

    fn pixels(&self) -> impl Iterator<Item = [u8; 4]> + '_ {
        self.rgba.as_chunks::<4>().0.iter().copied()
    }
}

/// How many pixels of `a` and `b` differ by more than `threshold` (0 to 1;
/// pixelmatch's default is 0.1). `None` when their sizes differ.
pub(crate) fn count_changed(a: &Image, b: &Image, threshold: f64) -> Option<u64> {
    if (a.width, a.height) != (b.width, b.height) {
        return None;
    }
    let max = MAX_DELTA * threshold * threshold;
    Some(
        a.pixels()
            .zip(b.pixels())
            .filter(|(p, q)| delta(*p, *q) > max)
            .count() as u64,
    )
}

/// An image of what changed from `a` to `b` (the same size): `b` faded to
/// light grey, with each pixel that changed by more than `threshold` in
/// red.
pub(crate) fn diff_image(a: &Image, b: &Image, threshold: f64) -> Image {
    let max = MAX_DELTA * threshold * threshold;
    let rgba = a
        .pixels()
        .zip(b.pixels())
        .flat_map(|(p, q)| {
            if delta(p, q) > max {
                [255, 0, 0, 255]
            } else {
                let [r, g, b] = blend(q);
                // pixelmatch's `alpha = 0.1`: a faint copy for context.
                let grey = (255.0 + (luma(r, g, b) - 255.0) * 0.1).clamp(0.0, 255.0);
                #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                let grey = grey.round() as u8;
                [grey, grey, grey, 255]
            }
        })
        .collect();
    Image {
        width: b.width,
        height: b.height,
        rgba,
    }
}

/// A pixel blended over white by its alpha.
fn blend([r, g, b, a]: [u8; 4]) -> [f64; 3] {
    let a = f64::from(a) / 255.0;
    [r, g, b].map(|c| 255.0 + (f64::from(c) - 255.0) * a)
}

fn luma(r: f64, g: f64, b: f64) -> f64 {
    r * 0.298_895_31 + g * 0.586_622_47 + b * 0.114_482_23
}

/// pixelmatch's squared YIQ distance between two pixels.
fn delta(p: [u8; 4], q: [u8; 4]) -> f64 {
    if p == q {
        return 0.0;
    }
    let [r1, g1, b1] = blend(p);
    let [r2, g2, b2] = blend(q);
    let y = luma(r1, g1, b1) - luma(r2, g2, b2);
    let i = (r1 * 0.595_977_99 - g1 * 0.274_176_10 - b1 * 0.321_801_89)
        - (r2 * 0.595_977_99 - g2 * 0.274_176_10 - b2 * 0.321_801_89);
    let q = (r1 * 0.211_470_17 - g1 * 0.522_617_11 + b1 * 0.311_146_94)
        - (r2 * 0.211_470_17 - g2 * 0.522_617_11 + b2 * 0.311_146_94);
    0.5053 * y * y + 0.299 * i * i + 0.1957 * q * q
}

#[cfg(test)]
mod tests {
    use super::{Image, count_changed, diff_image};

    fn image(width: u32, height: u32, pixels: &[[u8; 4]]) -> Image {
        Image {
            width,
            height,
            rgba: pixels.concat(),
        }
    }

    const WHITE: [u8; 4] = [255, 255, 255, 255];
    const BLACK: [u8; 4] = [0, 0, 0, 255];

    #[test]
    fn counts_pixels_past_the_threshold() {
        let a = image(2, 2, &[WHITE, WHITE, WHITE, BLACK]);
        assert_eq!(count_changed(&a, &a, 0.1), Some(0));
        // A barely different white is under the threshold; black is not.
        let b = image(2, 2, &[[250, 250, 250, 255], WHITE, BLACK, BLACK]);
        assert_eq!(count_changed(&a, &b, 0.1), Some(1));
        assert_eq!(count_changed(&a, &b, 0.0), Some(2));
        // Black and white are as far apart as colours get.
        assert_eq!(count_changed(&a, &b, 1.0), Some(0));
        assert_eq!(count_changed(&a, &image(1, 4, &[WHITE; 4]), 0.1), None);
    }

    #[test]
    fn transparent_is_white() {
        let a = image(1, 1, &[[0, 0, 0, 0]]);
        let b = image(1, 1, &[WHITE]);
        assert_eq!(count_changed(&a, &b, 0.01), Some(0));
    }

    #[test]
    fn the_diff_marks_changes_in_red_over_a_faint_copy() {
        let a = image(2, 1, &[WHITE, WHITE]);
        let b = image(2, 1, &[BLACK, WHITE]);
        let diff = diff_image(&a, &b, 0.1);
        assert_eq!(diff.rgba, [255, 0, 0, 255, 255, 255, 255, 255]);
    }

    #[test]
    fn png_round_trip() {
        let a = image(2, 1, &[BLACK, [10, 20, 30, 40]]);
        let bytes = a.encode().unwrap();
        assert_eq!(Image::decode(&bytes).unwrap(), a);
        assert!(Image::decode(b"not a png").is_err());
    }
}
