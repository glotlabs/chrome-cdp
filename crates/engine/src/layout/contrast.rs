//! `layout`'s contrast on pixels: the colour of a text against what is
//! drawn behind it, not against declared background colours (which miss
//! text on images and gradients).
//!
//! Two screenshots of the viewport in CSS pixels: `before`, as is, and
//! `after`, with all text made transparent, which holds only background.
//! In a text's box, the background is the 10th and 90th percentile of
//! `after`'s luminance; the text's computed colour (with its opacity) is
//! blended over each, and the worse of the two ratios counts. Where the
//! two shots do not differ, the text was not drawn there (something
//! covers it): it is skipped, unless its colour is the background's.

use crate::expect::pixels::Image;

/// Contrast WCAG asks for (1.4.3): 4.5:1, or 3:1 for large text.
pub(super) const NORMAL: f64 = 4.5;
pub(super) const LARGE: f64 = 3.0;

/// How much a channel may differ between the shots and still count as
/// the same pixel.
const SAME: i32 = 16;

/// What a text box measured.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct Measured {
    /// The worse of the two ratios.
    pub(super) ratio: f64,
    /// The background at the 10th and 90th percentile of luminance.
    pub(super) low: [u8; 3],
    pub(super) high: [u8; 3],
}

/// Measures the text of colour `colour` (`[r, g, b, a]`, alpha from 0
/// to 1) in `area` (`[x, y, width, height]` in the shots' pixels).
/// `None` when the area is off the shots, or the text was not drawn
/// there.
pub(super) fn measure(
    before: &Image,
    after: &Image,
    area: [f64; 4],
    colour: [f64; 4],
) -> Option<Measured> {
    if (before.width, before.height) != (after.width, after.height) {
        return None;
    }
    let [left_x, top_y, width, height] = area;
    let clamp = |v: f64, max: u32| {
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let v = v.max(0.0).min(f64::from(max)) as u32;
        v
    };
    let (left, top) = (
        clamp(left_x.floor(), after.width),
        clamp(top_y.floor(), after.height),
    );
    let (right, bottom) = (
        clamp((left_x + width).ceil(), after.width),
        clamp((top_y + height).ceil(), after.height),
    );
    if right <= left || bottom <= top {
        return None;
    }
    let mut background: Vec<(f64, [u8; 3])> = Vec::new();
    let mut drawn = 0usize;
    for py in top..bottom {
        for px in left..right {
            let [r, g, b, _] = after.at(px, py);
            let [r0, g0, b0, _] = before.at(px, py);
            let differs = [(r, r0), (g, g0), (b, b0)]
                .iter()
                .any(|&(a, b)| (i32::from(a) - i32::from(b)).abs() > SAME);
            if differs {
                drawn += 1;
            }
            let rgb = [r, g, b];
            background.push((luminance(rgb.map(f64::from)), rgb));
        }
    }
    background.sort_by(|a, b| a.0.total_cmp(&b.0));
    let at = |fraction: f64| {
        #[allow(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            clippy::cast_precision_loss
        )]
        let i = ((background.len() - 1) as f64 * fraction).round() as usize;
        background[i].1
    };
    let (low, high) = (at(0.1), at(0.9));
    let ratio = [low, high]
        .into_iter()
        .map(|bg| {
            let bg = bg.map(f64::from);
            let alpha = colour[3].clamp(0.0, 1.0);
            let text = [0, 1, 2].map(|i| colour[i] * alpha + bg[i] * (1.0 - alpha));
            ratio(luminance(text), luminance(bg))
        })
        .fold(f64::INFINITY, f64::min);
    // Not drawn here (covered), unless it is drawn in the background's
    // own colour.
    if drawn == 0 && ratio >= 1.1 {
        return None;
    }
    Some(Measured { ratio, low, high })
}

/// WCAG's relative luminance of an sRGB colour (0 to 255 a channel).
fn luminance(rgb: [f64; 3]) -> f64 {
    let [r, g, b] = rgb.map(|c| {
        let c = c / 255.0;
        if c <= 0.039_28 {
            c / 12.92
        } else {
            ((c + 0.055) / 1.055).powf(2.4)
        }
    });
    0.2126 * r + 0.7152 * g + 0.0722 * b
}

/// The contrast ratio of two luminances (1 to 21).
fn ratio(a: f64, b: f64) -> f64 {
    (a.max(b) + 0.05) / (a.min(b) + 0.05)
}

/// `#7fb1ff`.
pub(super) fn hex(rgb: [u8; 3]) -> String {
    format!("#{:02x}{:02x}{:02x}", rgb[0], rgb[1], rgb[2])
}

/// A ratio as the output shows it: one decimal, rounded down, so a ratio
/// just under what is needed does not read as enough.
pub(super) fn shown(ratio: f64) -> String {
    format!("{:.1}", (ratio * 10.0).floor() / 10.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A `width` x `height` image, each pixel from `paint(x, y)`.
    fn image(width: u32, height: u32, paint: impl Fn(u32, u32) -> [u8; 3]) -> Image {
        let mut rgba = Vec::new();
        for y in 0..height {
            for x in 0..width {
                let [r, g, b] = paint(x, y);
                rgba.extend([r, g, b, 255]);
            }
        }
        Image::from_rgba(width, height, rgba)
    }

    #[test]
    fn black_on_white_is_the_most() {
        let after = image(10, 10, |_, _| [255, 255, 255]);
        let before = image(
            10,
            10,
            |x, _| {
                if x == 5 { [0, 0, 0] } else { [255, 255, 255] }
            },
        );
        let m = measure(
            &before,
            &after,
            [0.0, 0.0, 10.0, 10.0],
            [0.0, 0.0, 0.0, 1.0],
        )
        .unwrap();
        assert!((m.ratio - 21.0).abs() < 0.01, "{}", m.ratio);
        assert_eq!(hex(m.low), "#ffffff");
    }

    #[test]
    fn the_worse_end_of_a_gradient_counts() {
        // White text on a gradient from dark blue to light blue.
        let after = image(100, 10, |x, _| {
            #[allow(clippy::cast_possible_truncation)]
            let v = (x * 2) as u8;
            [v, v, 255]
        });
        let before = image(100, 10, |x, y| {
            if y == 5 {
                [255, 255, 255]
            } else {
                #[allow(clippy::cast_possible_truncation)]
                let v = (x * 2) as u8;
                [v, v, 255]
            }
        });
        let m = measure(
            &before,
            &after,
            [0.0, 0.0, 100.0, 10.0],
            [255.0, 255.0, 255.0, 1.0],
        )
        .unwrap();
        // Against the light end (about #b4b4ff) white is poor.
        assert!(m.ratio < 2.0, "{}", m.ratio);
        assert!(m.low[0] < m.high[0]);
    }

    #[test]
    fn text_not_drawn_is_skipped_unless_it_matches_the_background() {
        let white = image(10, 10, |_, _| [255, 255, 255]);
        assert_eq!(
            measure(&white, &white, [0.0, 0.0, 10.0, 10.0], [0.0, 0.0, 0.0, 1.0]),
            None
        );
        let m = measure(
            &white,
            &white,
            [0.0, 0.0, 10.0, 10.0],
            [255.0, 255.0, 255.0, 1.0],
        )
        .unwrap();
        assert!((m.ratio - 1.0).abs() < 0.01);
    }

    #[test]
    fn transparent_text_blends_with_the_background() {
        let after = image(4, 4, |_, _| [255, 255, 255]);
        let before = image(4, 4, |_, _| [128, 128, 128]);
        let full = measure(&before, &after, [0.0, 0.0, 4.0, 4.0], [0.0, 0.0, 0.0, 1.0]).unwrap();
        let half = measure(&before, &after, [0.0, 0.0, 4.0, 4.0], [0.0, 0.0, 0.0, 0.5]).unwrap();
        assert!(half.ratio < full.ratio);
    }

    #[test]
    fn ratios_are_shown_rounded_down() {
        assert_eq!(shown(4.49), "4.4");
        assert_eq!(shown(4.5), "4.5");
        assert_eq!(shown(21.0), "21.0");
    }
}
