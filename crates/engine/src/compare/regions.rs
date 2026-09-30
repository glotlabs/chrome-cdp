//! `compare --visual`'s regions: changed pixels grouped into boxes an
//! agent can name. The image is cut into 8×8 px cells; cells with any
//! change that touch (sides or corners) are one group, boxed around its
//! changed pixels; then boxes within 16 px of each other are merged until
//! none are.

/// The side of a cell, in pixels.
const CELL: u32 = 8;

/// Boxes closer than this (in pixels, on both axes) are merged.
const NEAR: u32 = 16;

/// A box of changed pixels: `x, y, width, height` and how many changed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Region {
    pub(crate) x: u32,
    pub(crate) y: u32,
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) changed: u64,
}

impl Region {
    fn right(&self) -> u32 {
        self.x + self.width
    }

    fn bottom(&self) -> u32 {
        self.y + self.height
    }

    /// Whether `other` is within [`NEAR`] of it.
    fn near(&self, other: &Self) -> bool {
        let apart = |a0: u32, a1: u32, b0: u32, b1: u32| {
            if b0 >= a1 {
                b0 - a1
            } else {
                a0.saturating_sub(b1)
            }
        };
        apart(self.x, self.right(), other.x, other.right()) <= NEAR
            && apart(self.y, self.bottom(), other.y, other.bottom()) <= NEAR
    }

    fn merge(&self, other: &Self) -> Self {
        let x = self.x.min(other.x);
        let y = self.y.min(other.y);
        Self {
            x,
            y,
            width: self.right().max(other.right()) - x,
            height: self.bottom().max(other.bottom()) - y,
            changed: self.changed + other.changed,
        }
    }

    /// Its area, in pixels.
    pub(crate) fn area(&self) -> u64 {
        u64::from(self.width) * u64::from(self.height)
    }

    /// Its centre.
    pub(crate) fn centre(&self) -> (f64, f64) {
        (
            f64::from(self.x) + f64::from(self.width) / 2.0,
            f64::from(self.y) + f64::from(self.height) / 2.0,
        )
    }
}

/// The regions of `changed` (`width` pixels a row), largest first.
pub(crate) fn regions(changed: &[bool], width: u32) -> Vec<Region> {
    if width == 0 {
        return Vec::new();
    }
    let height = u32::try_from(changed.len() / width as usize).unwrap_or(u32::MAX);
    let (cols, rows) = (width.div_ceil(CELL), height.div_ceil(CELL));
    // Each cell's box of changed pixels, if it has any.
    let mut cells: Vec<Option<Region>> = vec![None; cols as usize * rows as usize];
    for y in 0..height {
        let row = y as usize * width as usize;
        for x in 0..width {
            if !changed[row + x as usize] {
                continue;
            }
            let at = Region {
                x,
                y,
                width: 1,
                height: 1,
                changed: 1,
            };
            let cell = &mut cells[(y / CELL * cols + x / CELL) as usize];
            *cell = Some(cell.map_or(at, |c| c.merge(&at)));
        }
    }
    // Groups of touching cells.
    let mut found = Vec::new();
    let mut seen = vec![false; cells.len()];
    for start in 0..cells.len() {
        if seen[start] || cells[start].is_none() {
            continue;
        }
        seen[start] = true;
        let mut group: Option<Region> = None;
        let mut stack = vec![start];
        while let Some(i) = stack.pop() {
            let Some(cell) = cells[i] else { continue };
            group = Some(group.map_or(cell, |g| g.merge(&cell)));
            let i32 = u32::try_from(i).unwrap_or(u32::MAX);
            let (cx, cy) = (i32 % cols, i32 / cols);
            for dy in -1i64..=1 {
                for dx in -1i64..=1 {
                    let (nx, ny) = (i64::from(cx) + dx, i64::from(cy) + dy);
                    if nx < 0 || ny < 0 || nx >= i64::from(cols) || ny >= i64::from(rows) {
                        continue;
                    }
                    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                    let n = (ny as u32 * cols + nx as u32) as usize;
                    if !seen[n] && cells[n].is_some() {
                        seen[n] = true;
                        stack.push(n);
                    }
                }
            }
        }
        found.extend(group);
    }
    // Merge near boxes until none are.
    loop {
        let mut merged = false;
        let mut out: Vec<Region> = Vec::with_capacity(found.len());
        for region in found {
            match out.iter_mut().find(|r| r.near(&region)) {
                Some(r) => {
                    *r = r.merge(&region);
                    merged = true;
                }
                None => out.push(region),
            }
        }
        found = out;
        if !merged {
            break;
        }
    }
    found.sort_by(|a, b| b.area().cmp(&a.area()).then((a.y, a.x).cmp(&(b.y, b.x))));
    found
}

#[cfg(test)]
mod tests {
    use super::{Region, regions};

    /// A `width` x `height` mask with the boxes set.
    fn mask(width: u32, height: u32, boxes: &[[u32; 4]]) -> Vec<bool> {
        let mut out = vec![false; (width * height) as usize];
        for &[x, y, w, h] in boxes {
            for py in y..y + h {
                for px in x..x + w {
                    out[(py * width + px) as usize] = true;
                }
            }
        }
        out
    }

    fn boxes(found: &[Region]) -> Vec<[u32; 4]> {
        found
            .iter()
            .map(|r| [r.x, r.y, r.width, r.height])
            .collect()
    }

    #[test]
    fn far_changes_are_apart_and_near_ones_merge() {
        // Two blocks 40 px apart, and one 10 px right of the second.
        let changed = mask(200, 100, &[[0, 0, 20, 10], [60, 0, 10, 10], [80, 5, 5, 5]]);
        let found = regions(&changed, 200);
        assert_eq!(boxes(&found), [[60, 0, 25, 10], [0, 0, 20, 10]]);
        assert_eq!(found[1].changed, 200);
        assert_eq!(found[0].centre(), (72.5, 5.0));
    }

    #[test]
    fn touching_cells_are_one_region() {
        // A diagonal line crosses cells that touch only at corners.
        let line: Vec<[u32; 4]> = (0..40).map(|i| [i, i, 1, 1]).collect();
        let found = regions(&mask(64, 64, &line), 64);
        assert_eq!(boxes(&found), [[0, 0, 40, 40]]);
        assert!(regions(&mask(64, 64, &[]), 64).is_empty());
    }
}
