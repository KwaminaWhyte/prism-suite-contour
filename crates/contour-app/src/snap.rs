//! Snapping — pure geometry for grid / guide / object snapping.
//!
//! Illustrator pulls a dragged object toward nearby reference coordinates: the
//! document grid, user-placed ruler guides, and the edges/centres of other
//! objects ("smart guides"). All three reduce to the same primitive: a set of
//! candidate **target coordinates** on each axis, and a search for the candidate
//! nearest a moving point — within a pixel tolerance — so the drag *snaps* to it.
//!
//! This module owns that primitive as UI-free `f32` arithmetic so the spacing /
//! nearest-target maths is unit-testable without egui. The app gathers the
//! candidate coordinates (from the grid spacing, the document's guides, and the
//! other shapes' bounding boxes) and the moving point's *snap features* (the
//! point itself plus the corners/centre of its bounding box), then calls
//! [`snap_delta`] to get the `(dx, dy)` adjustment to add to a raw drag delta.
//!
//! Tolerance is supplied in **document units** (the app divides a fixed pixel
//! tolerance by the zoom), so snapping feels the same at every zoom level.

/// Which snapping sources are active. Mirrors the View-menu toggles; passed into
/// candidate-gathering so a disabled source contributes nothing.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SnapConfig {
    pub to_grid: bool,
    pub to_guides: bool,
    pub to_objects: bool,
    /// Grid spacing in document units (only consulted when `to_grid`).
    pub grid: f32,
}

impl Default for SnapConfig {
    fn default() -> Self {
        Self {
            to_grid: false,
            to_guides: false,
            to_objects: true,
            grid: 20.0,
        }
    }
}

impl SnapConfig {
    /// Whether any snapping source is enabled at all.
    pub fn any(&self) -> bool {
        self.to_grid || self.to_guides || self.to_objects
    }
}

/// The result of a snap query: the translation to add to the raw drag plus the
/// document-space coordinates of the snap lines that fired (for drawing the
/// magenta "smart guide" overlay). A line is `None` when that axis did not snap.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct SnapResult {
    /// Horizontal adjustment (added to the raw `dx`).
    pub dx: f32,
    /// Vertical adjustment (added to the raw `dy`).
    pub dy: f32,
    /// `x` coordinate of the vertical snap line that fired, if any.
    pub line_x: Option<f32>,
    /// `y` coordinate of the horizontal snap line that fired, if any.
    pub line_y: Option<f32>,
}

/// Candidate target coordinates to snap to, gathered per axis. `xs` are vertical
/// lines (constant `x`); `ys` are horizontal lines (constant `y`).
#[derive(Clone, Debug, Default)]
pub struct SnapTargets {
    pub xs: Vec<f32>,
    pub ys: Vec<f32>,
}

impl SnapTargets {
    pub fn is_empty(&self) -> bool {
        self.xs.is_empty() && self.ys.is_empty()
    }
}

/// Snap features of the thing being moved: the document-space coordinates that
/// should try to land on a target. For a box drag these are the box's left /
/// centre / right (as `xs`) and top / middle / bottom (as `ys`); for a single
/// point both lists hold just that coordinate.
#[derive(Clone, Debug, Default)]
pub struct SnapFeatures {
    pub xs: Vec<f32>,
    pub ys: Vec<f32>,
}

impl SnapFeatures {
    /// Features of a single point (snap the point itself on both axes).
    pub fn point(x: f32, y: f32) -> Self {
        Self {
            xs: vec![x],
            ys: vec![y],
        }
    }

    /// Features of an axis-aligned box `[x, y, w, h]`: the three vertical
    /// reference lines (left, centre, right) and three horizontal ones (top,
    /// middle, bottom), matching Illustrator's box snapping.
    pub fn bbox(b: &[f32; 4]) -> Self {
        Self {
            xs: vec![b[0], b[0] + b[2] * 0.5, b[0] + b[2]],
            ys: vec![b[1], b[1] + b[3] * 0.5, b[1] + b[3]],
        }
    }
}

/// The nearest target to `value` within `tol`, returning `(target, distance)`.
fn nearest(value: f32, targets: &[f32], tol: f32) -> Option<(f32, f32)> {
    let mut best: Option<(f32, f32)> = None;
    for &t in targets {
        let d = (t - value).abs();
        if d <= tol && best.is_none_or(|(_, bd)| d < bd) {
            best = Some((t, d));
        }
    }
    best
}

/// Compute the snap adjustment for moving `features` (already offset by the raw
/// drag delta) toward the candidate `targets`, within `tol` document units.
///
/// On each axis independently it finds the (feature, target) pair with the
/// smallest distance and returns the delta that moves that feature exactly onto
/// the target — so the *closest* of the box's edges/centre wins, exactly like
/// Illustrator. When grid snapping is the only source the targets are the grid
/// lines straddling each feature; see [`grid_targets_near`].
pub fn snap_delta(features: &SnapFeatures, targets: &SnapTargets, tol: f32) -> SnapResult {
    let mut res = SnapResult::default();

    // X axis: pick the feature/target pair with the smallest absolute gap.
    let mut best_x: Option<(f32, f32, f32)> = None; // (delta, line, dist)
    for &fx in &features.xs {
        if let Some((target, d)) = nearest(fx, &targets.xs, tol) {
            if best_x.is_none_or(|(_, _, bd)| d < bd) {
                best_x = Some((target - fx, target, d));
            }
        }
    }
    if let Some((delta, line, _)) = best_x {
        res.dx = delta;
        res.line_x = Some(line);
    }

    // Y axis.
    let mut best_y: Option<(f32, f32, f32)> = None;
    for &fy in &features.ys {
        if let Some((target, d)) = nearest(fy, &targets.ys, tol) {
            if best_y.is_none_or(|(_, _, bd)| d < bd) {
                best_y = Some((target - fy, target, d));
            }
        }
    }
    if let Some((delta, line, _)) = best_y {
        res.dy = delta;
        res.line_y = Some(line);
    }

    res
}

/// The two grid lines straddling `value` for a grid of spacing `spacing`
/// (`spacing > 0`). Used to feed grid coordinates into [`SnapTargets`] without
/// enumerating the whole (infinite) grid: only the nearest lines can ever win.
pub fn grid_targets_near(value: f32, spacing: f32) -> [f32; 2] {
    if spacing <= 0.0 {
        return [value, value];
    }
    let k = (value / spacing).floor();
    [k * spacing, (k + 1.0) * spacing]
}

/// Snap a single document point to the grid (round to the nearest grid line on
/// each axis) when within `tol`. Returns the adjusted point. Used by shape
/// creation so a fresh rectangle's corner lands on the grid.
pub fn snap_point_to_grid(x: f32, y: f32, spacing: f32, tol: f32) -> (f32, f32) {
    let snap1 = |v: f32| {
        let cands = grid_targets_near(v, spacing);
        match nearest(v, &cands, tol) {
            Some((t, _)) => t,
            None => v,
        }
    };
    (snap1(x), snap1(y))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nearest_within_tolerance() {
        assert_eq!(nearest(9.5, &[0.0, 10.0, 20.0], 1.0), Some((10.0, 0.5)));
        // Outside tolerance: no snap.
        assert_eq!(nearest(5.0, &[0.0, 10.0], 1.0), None);
        // Ties pick the first encountered equally-near target deterministically.
        assert_eq!(nearest(5.0, &[4.0, 6.0], 2.0), Some((4.0, 1.0)));
    }

    #[test]
    fn snap_delta_point_to_guide() {
        // A point at x=98 with a vertical guide at x=100, tol=4 -> +2 on x.
        let f = SnapFeatures::point(98.0, 50.0);
        let t = SnapTargets {
            xs: vec![100.0],
            ys: vec![],
        };
        let r = snap_delta(&f, &t, 4.0);
        assert_eq!(r.dx, 2.0);
        assert_eq!(r.line_x, Some(100.0));
        assert_eq!(r.dy, 0.0);
        assert_eq!(r.line_y, None);
    }

    #[test]
    fn snap_delta_no_target_in_range() {
        let f = SnapFeatures::point(0.0, 0.0);
        let t = SnapTargets {
            xs: vec![100.0],
            ys: vec![100.0],
        };
        let r = snap_delta(&f, &t, 4.0);
        assert_eq!(r, SnapResult::default());
        assert_eq!(r.line_x, None);
        assert_eq!(r.line_y, None);
    }

    #[test]
    fn snap_delta_box_closest_edge_wins() {
        // Box [10,10,40,40]: left=10, centerX=30, right=50. A vertical target at
        // 52 is 2 from the right edge (closest) -> dx = +2, line at 52.
        let f = SnapFeatures::bbox(&[10.0, 10.0, 40.0, 40.0]);
        let t = SnapTargets {
            xs: vec![52.0],
            ys: vec![],
        };
        let r = snap_delta(&f, &t, 5.0);
        assert_eq!(r.dx, 2.0);
        assert_eq!(r.line_x, Some(52.0));
    }

    #[test]
    fn snap_delta_box_center_can_win() {
        // Target at 31 is 1 from the centre (30) but 19 from left/right -> centre
        // wins, dx = +1.
        let f = SnapFeatures::bbox(&[10.0, 10.0, 40.0, 40.0]);
        let t = SnapTargets {
            xs: vec![31.0],
            ys: vec![],
        };
        let r = snap_delta(&f, &t, 5.0);
        assert!((r.dx - 1.0).abs() < 1e-6);
        assert_eq!(r.line_x, Some(31.0));
    }

    #[test]
    fn snap_delta_picks_nearer_of_two_targets() {
        // Left edge at 10; targets at 8 (d=2) and 13 (d=3) -> the nearer (8) wins.
        let f = SnapFeatures {
            xs: vec![10.0],
            ys: vec![],
        };
        let t = SnapTargets {
            xs: vec![13.0, 8.0],
            ys: vec![],
        };
        let r = snap_delta(&f, &t, 5.0);
        assert_eq!(r.dx, -2.0);
        assert_eq!(r.line_x, Some(8.0));
    }

    #[test]
    fn grid_targets_straddle_value() {
        assert_eq!(grid_targets_near(23.0, 20.0), [20.0, 40.0]);
        assert_eq!(grid_targets_near(-1.0, 20.0), [-20.0, 0.0]);
        // On a line: that line and the next.
        assert_eq!(grid_targets_near(20.0, 20.0), [20.0, 40.0]);
        // Degenerate spacing: no movement.
        assert_eq!(grid_targets_near(7.0, 0.0), [7.0, 7.0]);
    }

    #[test]
    fn snap_point_to_grid_rounds_when_close() {
        // 23 -> nearest grid line 20 (d=3) within tol=4.
        assert_eq!(snap_point_to_grid(23.0, 38.0, 20.0, 4.0), (20.0, 40.0));
        // 30 is 10 from both 20 and 40: outside tol -> unchanged.
        assert_eq!(snap_point_to_grid(30.0, 30.0, 20.0, 4.0), (30.0, 30.0));
    }

    #[test]
    fn config_any_reflects_sources() {
        let mut c = SnapConfig {
            to_grid: false,
            to_guides: false,
            to_objects: false,
            grid: 20.0,
        };
        assert!(!c.any());
        c.to_objects = true;
        assert!(c.any());
    }
}
