//! Where the pipeline aims: frames' local roots, and the point in an
//! element's quads.

use cdp::domains::dom;

use super::Viewport;
use crate::snapshot::FrameSource;

/// The local root of frame `frame`: the frame itself or its nearest
/// ancestor whose parent runs in another session (another process), or
/// the main frame. Quads and hit tests of `frame` are in its viewport.
pub(super) fn local_root(frames: &[FrameSource], frame: &str) -> String {
    let session = |id: &str| {
        frames
            .iter()
            .find(|f| f.info.id == id)
            .map(|f| f.session.id().map(str::to_owned))
    };
    let mut current = frame.to_owned();
    while let Some(parent) = frames
        .iter()
        .find(|f| f.info.id == current)
        .and_then(|f| f.info.parent.clone())
    {
        if session(&parent) != session(&current) {
            break;
        }
        current = parent;
    }
    current
}

/// Whether frame `frame` is inside frame `ancestor` (at any depth).
pub(super) fn is_below(frames: &[FrameSource], frame: &str, ancestor: &str) -> bool {
    let mut current = frame.to_owned();
    while let Some(parent) = frames
        .iter()
        .find(|f| f.info.id == current)
        .and_then(|f| f.info.parent.clone())
    {
        if parent == ancestor {
            return true;
        }
        current = parent;
    }
    false
}

/// The centre of the first quad that is in the viewport, clipped to it
/// (Playwright's rule: a clipped area of at least one square pixel).
pub(super) fn pick(quads: &[dom::Quad], viewport: Viewport) -> Option<(f64, f64)> {
    quads.iter().filter(|q| q.len() == 8).find_map(|quad| {
        let clipped: Vec<f64> = quad
            .chunks(2)
            .flat_map(|p| {
                [
                    p[0].clamp(0.0, viewport.width),
                    p[1].clamp(0.0, viewport.height),
                ]
            })
            .collect();
        (area(&clipped) > 0.99).then(|| centre(&clipped))
    })
}

/// The area of a quad (four points, clockwise or not).
fn area(quad: &[f64]) -> f64 {
    let mut sum = 0.0;
    for i in 0..4 {
        let (x1, y1) = (quad[2 * i], quad[2 * i + 1]);
        let (x2, y2) = (quad[(2 * i + 2) % 8], quad[(2 * i + 3) % 8]);
        sum += x1 * y2 - x2 * y1;
    }
    (sum / 2.0).abs()
}

/// A quad's bounding box: `[left, top, right, bottom]`.
pub(super) fn bounds(quad: &[f64]) -> [f64; 4] {
    let xs = quad.iter().step_by(2).copied();
    let ys = quad.iter().skip(1).step_by(2).copied();
    [
        xs.clone().fold(f64::INFINITY, f64::min),
        ys.clone().fold(f64::INFINITY, f64::min),
        xs.fold(f64::NEG_INFINITY, f64::max),
        ys.fold(f64::NEG_INFINITY, f64::max),
    ]
}

/// A quad's first point (its top left, for a box model's quads).
pub(super) fn origin(quad: &[f64]) -> (f64, f64) {
    (
        quad.first().copied().unwrap_or_default(),
        quad.get(1).copied().unwrap_or_default(),
    )
}

pub(super) fn centre(quad: &[f64]) -> (f64, f64) {
    let n = (quad.len() / 2).max(1);
    #[allow(clippy::cast_precision_loss)]
    let n = n as f64;
    let x: f64 = quad.iter().step_by(2).sum();
    let y: f64 = quad.iter().skip(1).step_by(2).sum();
    (x / n, y / n)
}

/// Why no quad is in the viewport, for the error.
pub(super) fn out_of_view(quads: &[dom::Quad], viewport: Viewport) -> String {
    let Some(quad) = quads.iter().find(|q| q.len() == 8) else {
        return "it has no box".to_owned();
    };
    let (x, y) = centre(quad);
    format!(
        "its box is at x={} y={}, the viewport is {}x{}",
        x.round(),
        y.round(),
        viewport.width.round(),
        viewport.height.round()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const VIEWPORT: Viewport = Viewport {
        width: 800.0,
        height: 600.0,
        scroll_x: 0.0,
        scroll_y: 1000.0,
    };

    #[test]
    fn aims_at_the_centre_of_the_visible_part() {
        let quad = vec![10.0, 20.0, 110.0, 20.0, 110.0, 60.0, 10.0, 60.0];
        assert_eq!(pick(&[quad], VIEWPORT), Some((60.0, 40.0)));
        // Half below the fold: the centre of the part in view.
        let quad = vec![0.0, 500.0, 100.0, 500.0, 100.0, 700.0, 0.0, 700.0];
        assert_eq!(pick(&[quad], VIEWPORT), Some((50.0, 550.0)));
    }

    #[test]
    fn skips_quads_outside_the_viewport() {
        let below = vec![0.0, 900.0, 100.0, 900.0, 100.0, 950.0, 0.0, 950.0];
        let inside = vec![0.0, 0.0, 10.0, 0.0, 10.0, 10.0, 0.0, 10.0];
        assert_eq!(pick(std::slice::from_ref(&below), VIEWPORT), None);
        assert_eq!(pick(&[below.clone(), inside], VIEWPORT), Some((5.0, 5.0)));
        assert_eq!(
            out_of_view(&[below], VIEWPORT),
            "its box is at x=50 y=925, the viewport is 800x600"
        );
        assert_eq!(out_of_view(&[], VIEWPORT), "it has no box");
    }

    #[test]
    fn a_sliver_under_a_pixel_does_not_count() {
        let edge = vec![0.0, 599.5, 1.0, 599.5, 1.0, 650.0, 0.0, 650.0];
        assert_eq!(pick(&[edge], VIEWPORT), None);
    }
}
