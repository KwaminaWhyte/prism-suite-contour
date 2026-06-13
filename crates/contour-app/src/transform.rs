//! Affine transforms — pure 2-D matrix math for the on-canvas transform box.
//!
//! Illustrator's free-transform / rotate / scale / reflect tools all reduce to a
//! single 2×3 affine matrix applied to every coordinate of the selection. This
//! module owns that matrix and the closed-form constructors (translate, scale
//! about a pivot, rotate about a pivot, reflect across an axis), plus a handful
//! of helpers the app layer needs to drive the on-canvas handles:
//!
//! - [`Affine::scale_about`] / [`Affine::rotate_about`] build a transform that
//!   keeps a chosen *pivot* point fixed (the opposite handle, or the box centre).
//! - [`scale_factors_for_handle`] turns a corner/edge handle drag into the
//!   `(sx, sy)` a free-transform scale should use, given the box and the pivot.
//!
//! Everything here is UI-free `f32` arithmetic so it can be unit-tested without
//! egui. The app maps a handle drag to an [`Affine`], then calls
//! [`crate::document::Shape::apply_affine`] on each selected shape as one undo
//! step. Handles are stored as *offsets* (direction + length only), so they are
//! transformed by the matrix's **linear** part (no translation) — see
//! [`Affine::apply_vector`].

/// A 2-D affine transform stored as the six coefficients of the matrix
///
/// ```text
/// | a c e |   | x |   | a·x + c·y + e |
/// | b d f | · | y | = | b·x + d·y + f |
/// | 0 0 1 |   | 1 |   |       1       |
/// ```
///
/// `(a, b, c, d)` is the linear part (rotation/scale/shear/reflect); `(e, f)` is
/// the translation. This matches the SVG / `kurbo` / PostScript `matrix(a b c d
/// e f)` convention, so it maps cleanly onto export later.
///
/// Serializable so a **symbol instance** can persist its placement matrix in the
/// `.contour` file.
#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Affine {
    pub a: f32,
    pub b: f32,
    pub c: f32,
    pub d: f32,
    pub e: f32,
    pub f: f32,
}

impl Default for Affine {
    fn default() -> Self {
        Self::IDENTITY
    }
}

impl Affine {
    /// The identity transform (leaves points unchanged).
    pub const IDENTITY: Affine = Affine {
        a: 1.0,
        b: 0.0,
        c: 0.0,
        d: 1.0,
        e: 0.0,
        f: 0.0,
    };

    /// Pure translation by `(tx, ty)`.
    pub fn translate(tx: f32, ty: f32) -> Affine {
        Affine {
            a: 1.0,
            b: 0.0,
            c: 0.0,
            d: 1.0,
            e: tx,
            f: ty,
        }
    }

    /// Pure scale about the origin by `(sx, sy)`.
    pub fn scale(sx: f32, sy: f32) -> Affine {
        Affine {
            a: sx,
            b: 0.0,
            c: 0.0,
            d: sy,
            e: 0.0,
            f: 0.0,
        }
    }

    /// Pure rotation about the origin by `radians` (positive = clockwise in
    /// screen/document space, where +y points down).
    pub fn rotate(radians: f32) -> Affine {
        let (s, c) = radians.sin_cos();
        Affine {
            a: c,
            b: s,
            c: -s,
            d: c,
            e: 0.0,
            f: 0.0,
        }
    }

    /// Pure shear about the origin: `shx` slides x by `shx·y` (horizontal shear),
    /// `shy` slides y by `shy·x` (vertical shear). The two factors are tangents of
    /// the shear angles, so `shear(tan θ, 0.0)` slants verticals by `θ`.
    pub fn shear(shx: f32, shy: f32) -> Affine {
        Affine {
            a: 1.0,
            b: shy,
            c: shx,
            d: 1.0,
            e: 0.0,
            f: 0.0,
        }
    }

    /// Compose: `self * rhs`, i.e. apply `rhs` first, then `self`.
    pub fn then(self, parent: Affine) -> Affine {
        // parent ∘ self  (matrix product parent · self)
        let m = parent;
        let s = self;
        Affine {
            a: m.a * s.a + m.c * s.b,
            b: m.b * s.a + m.d * s.b,
            c: m.a * s.c + m.c * s.d,
            d: m.b * s.c + m.d * s.d,
            e: m.a * s.e + m.c * s.f + m.e,
            f: m.b * s.e + m.d * s.f + m.f,
        }
    }

    /// Scale by `(sx, sy)` while keeping the pivot `(px, py)` fixed.
    ///
    /// This is `T(p) · S(sx,sy) · T(-p)`: move the pivot to the origin, scale,
    /// move it back.
    pub fn scale_about(sx: f32, sy: f32, px: f32, py: f32) -> Affine {
        Affine::translate(px, py)
            .then_apply(Affine::scale(sx, sy))
            .then_apply(Affine::translate(-px, -py))
    }

    /// Rotate by `radians` while keeping the pivot `(px, py)` fixed.
    pub fn rotate_about(radians: f32, px: f32, py: f32) -> Affine {
        Affine::translate(px, py)
            .then_apply(Affine::rotate(radians))
            .then_apply(Affine::translate(-px, -py))
    }

    /// Shear by `(shx, shy)` while keeping the pivot `(px, py)` fixed.
    pub fn shear_about(shx: f32, shy: f32, px: f32, py: f32) -> Affine {
        Affine::translate(px, py)
            .then_apply(Affine::shear(shx, shy))
            .then_apply(Affine::translate(-px, -py))
    }

    /// Reflect across the line through `(px, py)` at `radians` from the +x axis,
    /// keeping that line fixed. `angle = 0` is a vertical flip (mirror across the
    /// horizontal axis); `angle = π/2` is a horizontal flip. This is the general
    /// Reflect tool: `R(θ)·diag(1,-1)·R(-θ)` recentred on the pivot.
    pub fn reflect_about(radians: f32, px: f32, py: f32) -> Affine {
        Affine::translate(px, py)
            .then_apply(Affine::rotate(radians))
            .then_apply(Affine::scale(1.0, -1.0))
            .then_apply(Affine::rotate(-radians))
            .then_apply(Affine::translate(-px, -py))
    }

    /// Right-multiply: `self · rhs` (apply `rhs` first, then `self`). The dual of
    /// [`then`](Affine::then) written the other way round for readable chains.
    fn then_apply(self, rhs: Affine) -> Affine {
        rhs.then(self)
    }

    /// Map a *point* (subject to translation).
    pub fn apply_point(&self, x: f32, y: f32) -> (f32, f32) {
        (
            self.a * x + self.c * y + self.e,
            self.b * x + self.d * y + self.f,
        )
    }

    /// Map a *vector / offset* (the linear part only — translation is dropped).
    /// Used for the per-anchor handle offsets, which describe a direction and
    /// length relative to their anchor rather than an absolute position.
    pub fn apply_vector(&self, x: f32, y: f32) -> (f32, f32) {
        (self.a * x + self.c * y, self.b * x + self.d * y)
    }

    /// Whether this is (numerically) the identity — used to drop no-op edits.
    pub fn is_identity(&self) -> bool {
        let eq = |x: f32, y: f32| (x - y).abs() < 1e-6;
        eq(self.a, 1.0)
            && eq(self.b, 0.0)
            && eq(self.c, 0.0)
            && eq(self.d, 1.0)
            && eq(self.e, 0.0)
            && eq(self.f, 0.0)
    }
}

/// The eight transform-box handles: four corners and four edge midpoints. Corner
/// handles scale both axes; edge handles scale a single axis.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Handle {
    TopLeft,
    TopMid,
    TopRight,
    MidRight,
    BottomRight,
    BottomMid,
    BottomLeft,
    MidLeft,
}

impl Handle {
    /// All eight handles, clockwise from the top-left corner.
    pub const ALL: [Handle; 8] = [
        Handle::TopLeft,
        Handle::TopMid,
        Handle::TopRight,
        Handle::MidRight,
        Handle::BottomRight,
        Handle::BottomMid,
        Handle::BottomLeft,
        Handle::MidLeft,
    ];

    /// Whether this handle is a corner (scales both axes).
    pub fn is_corner(self) -> bool {
        matches!(
            self,
            Handle::TopLeft | Handle::TopRight | Handle::BottomRight | Handle::BottomLeft
        )
    }

    /// Position of this handle on a box, as `(fx, fy)` in `0..=1` of the box
    /// extents (`0` = left/top, `0.5` = centre, `1` = right/bottom).
    pub fn unit_pos(self) -> (f32, f32) {
        match self {
            Handle::TopLeft => (0.0, 0.0),
            Handle::TopMid => (0.5, 0.0),
            Handle::TopRight => (1.0, 0.0),
            Handle::MidRight => (1.0, 0.5),
            Handle::BottomRight => (1.0, 1.0),
            Handle::BottomMid => (0.5, 1.0),
            Handle::BottomLeft => (0.0, 1.0),
            Handle::MidLeft => (0.0, 0.5),
        }
    }

    /// The opposite handle — the pivot a free-transform scale keeps fixed while
    /// dragging this one.
    pub fn opposite(self) -> Handle {
        match self {
            Handle::TopLeft => Handle::BottomRight,
            Handle::TopMid => Handle::BottomMid,
            Handle::TopRight => Handle::BottomLeft,
            Handle::MidRight => Handle::MidLeft,
            Handle::BottomRight => Handle::TopLeft,
            Handle::BottomMid => Handle::TopMid,
            Handle::BottomLeft => Handle::TopRight,
            Handle::MidLeft => Handle::MidRight,
        }
    }
}

/// Compute the per-axis scale factors for dragging `handle` of a box of size
/// `(bw, bh)` to a cursor that is `(cur_dx, cur_dy)` from the **pivot** (the
/// opposite handle), where the handle started `(orig_dx, orig_dy)` from the
/// pivot.
///
/// Edge handles scale only their axis (the other factor is `1.0`). Corner
/// handles scale both; when `uniform` is set, both axes take the larger absolute
/// factor (shift-drag, preserving aspect ratio). Degenerate (zero-extent) axes
/// are left unscaled to avoid division by zero / collapse.
pub fn scale_factors_for_handle(
    handle: Handle,
    orig_dx: f32,
    orig_dy: f32,
    cur_dx: f32,
    cur_dy: f32,
    uniform: bool,
) -> (f32, f32) {
    let (fx, fy) = handle.unit_pos();
    // Which axes this handle controls: a midpoint at 0.5 doesn't move on that
    // axis, so its factor stays 1.
    let scales_x = (fx - 0.5).abs() > 0.0;
    let scales_y = (fy - 0.5).abs() > 0.0;

    let sx = if scales_x && orig_dx.abs() > 1e-6 {
        cur_dx / orig_dx
    } else {
        1.0
    };
    let sy = if scales_y && orig_dy.abs() > 1e-6 {
        cur_dy / orig_dy
    } else {
        1.0
    };

    if uniform && handle.is_corner() {
        // Lock aspect ratio: use the factor with the larger magnitude on both
        // axes, preserving each axis's sign so a drag past the pivot still flips.
        let mag = sx.abs().max(sy.abs());
        let sx2 = if sx < 0.0 { -mag } else { mag };
        let sy2 = if sy < 0.0 { -mag } else { mag };
        (sx2, sy2)
    } else {
        (sx, sy)
    }
}

/// Compute the shear factors for Cmd/Ctrl-dragging an *edge* `handle` of a box
/// of size `(bw, bh)`, given the cursor's `(dx, dy)` displacement from where the
/// handle started. A top/bottom edge shears horizontally (`shx`) by sliding
/// along x in proportion to the box height; a left/right edge shears vertically
/// (`shy`) by sliding along y in proportion to the box width. Corner handles do
/// not shear (returns `(0, 0)`). The pivot is the opposite edge, so the factor
/// is the drag distance divided by the *full* extent perpendicular to the edge.
///
/// The sign convention keeps the dragged edge following the cursor: dragging the
/// top edge right shears the top of the box to the right (positive `shx`).
pub fn shear_factors_for_handle(handle: Handle, bw: f32, bh: f32, dx: f32, dy: f32) -> (f32, f32) {
    match handle {
        // Horizontal edges slide along x; the pivot is the far edge a full height
        // away, so the shear factor is dx / height. Dragging the bottom edge
        // (which is below the top pivot, +y) needs an inverted sign so it tracks.
        Handle::TopMid => {
            if bh.abs() > 1e-6 {
                (-dx / bh, 0.0)
            } else {
                (0.0, 0.0)
            }
        }
        Handle::BottomMid => {
            if bh.abs() > 1e-6 {
                (dx / bh, 0.0)
            } else {
                (0.0, 0.0)
            }
        }
        // Vertical edges slide along y; the pivot is a full width away.
        Handle::MidLeft => {
            if bw.abs() > 1e-6 {
                (0.0, -dy / bw)
            } else {
                (0.0, 0.0)
            }
        }
        Handle::MidRight => {
            if bw.abs() > 1e-6 {
                (0.0, dy / bw)
            } else {
                (0.0, 0.0)
            }
        }
        // Corners don't shear.
        _ => (0.0, 0.0),
    }
}

/// The pivot a shear-drag of `handle` keeps fixed: the opposite edge's midpoint
/// on a box `[x, y, w, h]`. Returns `None` for corner handles (no shear).
pub fn shear_pivot(handle: Handle, bbox: &[f32; 4]) -> Option<(f32, f32)> {
    if handle.is_corner() {
        return None;
    }
    let opp = handle.opposite();
    let (fx, fy) = opp.unit_pos();
    Some((bbox[0] + bbox[2] * fx, bbox[1] + bbox[3] * fy))
}

/// A numeric transform request: translate, then scale, then rotate, then shear,
/// each about the selection centre (translate is absolute). Mirrors the
/// Transform-panel order Illustrator composes its numeric dialog in.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct NumericTransform {
    /// Document-space translation (applied first).
    pub move_x: f32,
    pub move_y: f32,
    /// Uniform-or-per-axis scale factors (1.0 = unchanged).
    pub scale_x: f32,
    pub scale_y: f32,
    /// Rotation about the pivot, radians (positive = clockwise, +y down).
    pub rotate: f32,
    /// Shear angles about the pivot, radians (x then y).
    pub shear_x: f32,
    pub shear_y: f32,
}

impl Default for NumericTransform {
    fn default() -> Self {
        Self {
            move_x: 0.0,
            move_y: 0.0,
            scale_x: 1.0,
            scale_y: 1.0,
            rotate: 0.0,
            shear_x: 0.0,
            shear_y: 0.0,
        }
    }
}

impl NumericTransform {
    /// Compose this request into a single affine about pivot `(px, py)`.
    ///
    /// Order (right-to-left, i.e. applied to a point in this sequence): translate
    /// → scale → rotate → shear, the rotation/scale/shear each pivot-anchored so
    /// the selection stays put except for the explicit move.
    pub fn to_affine(self, px: f32, py: f32) -> Affine {
        let mut m = Affine::translate(self.move_x, self.move_y);
        m = Affine::scale_about(self.scale_x, self.scale_y, px, py).then(m);
        m = Affine::rotate_about(self.rotate, px, py).then(m);
        m = Affine::shear_about(self.shear_x.tan(), self.shear_y.tan(), px, py).then(m);
        m
    }
}

/// The clockwise angle (radians) from the pivot→reference baseline to the
/// pivot→cursor direction — used to map a rotate-handle drag to a rotation.
/// Returns `0` when either vector is degenerate.
pub fn angle_between(from: (f32, f32), to: (f32, f32), pivot: (f32, f32)) -> f32 {
    let v0 = (from.0 - pivot.0, from.1 - pivot.1);
    let v1 = (to.0 - pivot.0, to.1 - pivot.1);
    if (v0.0.hypot(v0.1) < 1e-6) || (v1.0.hypot(v1.1) < 1e-6) {
        return 0.0;
    }
    let a0 = v0.1.atan2(v0.0);
    let a1 = v1.1.atan2(v1.0);
    a1 - a0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn approx(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-4
    }

    fn approx_pt(p: (f32, f32), q: (f32, f32)) -> bool {
        approx(p.0, q.0) && approx(p.1, q.1)
    }

    #[test]
    fn identity_leaves_points() {
        let m = Affine::IDENTITY;
        assert!(approx_pt(m.apply_point(3.0, -7.0), (3.0, -7.0)));
        assert!(m.is_identity());
    }

    #[test]
    fn translate_moves_points_not_vectors() {
        let m = Affine::translate(10.0, 5.0);
        assert!(approx_pt(m.apply_point(1.0, 2.0), (11.0, 7.0)));
        // A vector/offset ignores translation.
        assert!(approx_pt(m.apply_vector(1.0, 2.0), (1.0, 2.0)));
    }

    #[test]
    fn scale_about_keeps_pivot_fixed() {
        let m = Affine::scale_about(2.0, 3.0, 100.0, 50.0);
        // Pivot is unmoved.
        assert!(approx_pt(m.apply_point(100.0, 50.0), (100.0, 50.0)));
        // A point 10 right / 10 down of the pivot scales away from it.
        assert!(approx_pt(m.apply_point(110.0, 60.0), (120.0, 80.0)));
    }

    #[test]
    fn rotate_about_quarter_turn() {
        // 90° clockwise about the origin sends (1,0) -> (0,1) in +y-down space.
        let m = Affine::rotate_about(std::f32::consts::FRAC_PI_2, 0.0, 0.0);
        assert!(approx_pt(m.apply_point(1.0, 0.0), (0.0, 1.0)));
        assert!(approx_pt(m.apply_point(0.0, 1.0), (-1.0, 0.0)));
    }

    #[test]
    fn rotate_about_pivot_fixed() {
        let m = Affine::rotate_about(0.7, 42.0, -13.0);
        assert!(approx_pt(m.apply_point(42.0, -13.0), (42.0, -13.0)));
    }

    #[test]
    fn rotation_preserves_lengths() {
        let m = Affine::rotate(1.234);
        let v = m.apply_vector(3.0, 4.0);
        assert!(approx(v.0.hypot(v.1), 5.0));
    }

    #[test]
    fn compose_is_apply_rhs_then_self() {
        // First scale ×2 about origin, then translate +10.
        let m = Affine::translate(10.0, 0.0).then(Affine::scale(2.0, 2.0));
        // wait: then(parent) applies self first; build explicitly instead.
        let scale_then_translate = Affine::scale(2.0, 2.0).then(Affine::translate(10.0, 0.0));
        // Scale first: (3,0)->(6,0); then translate: (16,0).
        assert!(approx_pt(
            scale_then_translate.apply_point(3.0, 0.0),
            (16.0, 0.0)
        ));
        // The other order (`m`) translates first then scales: (3,0)->(13,0)->(26,0).
        assert!(approx_pt(m.apply_point(3.0, 0.0), (26.0, 0.0)));
    }

    #[test]
    fn corner_handle_scales_both_axes() {
        // Box 100×100, dragging the bottom-right corner. Pivot = top-left.
        // Original handle is (100,100) from the pivot; drag it to (200,50).
        let (sx, sy) =
            scale_factors_for_handle(Handle::BottomRight, 100.0, 100.0, 200.0, 50.0, false);
        assert!(approx(sx, 2.0));
        assert!(approx(sy, 0.5));
    }

    #[test]
    fn edge_handle_scales_one_axis() {
        // Right-mid handle scales only x.
        let (sx, sy) = scale_factors_for_handle(Handle::MidRight, 100.0, 0.0, 150.0, 999.0, false);
        assert!(approx(sx, 1.5));
        assert!(approx(sy, 1.0)); // y untouched despite the cursor's y delta
    }

    #[test]
    fn uniform_corner_locks_aspect_to_larger_factor() {
        let (sx, sy) =
            scale_factors_for_handle(Handle::BottomRight, 100.0, 100.0, 200.0, 50.0, true);
        // Larger magnitude (2.0) wins on both axes.
        assert!(approx(sx, 2.0));
        assert!(approx(sy, 2.0));
    }

    #[test]
    fn uniform_keeps_axis_sign_for_flip() {
        // Drag past the pivot on x so sx is negative; uniform should keep that
        // axis flipped while matching the larger magnitude.
        let (sx, sy) =
            scale_factors_for_handle(Handle::BottomRight, 100.0, 100.0, -300.0, 100.0, true);
        // |sx|=3 dominates; sx stays negative, sy positive.
        assert!(approx(sx, -3.0));
        assert!(approx(sy, 3.0));
    }

    #[test]
    fn opposite_handles_pair_up() {
        assert_eq!(Handle::TopLeft.opposite(), Handle::BottomRight);
        assert_eq!(Handle::MidLeft.opposite(), Handle::MidRight);
        assert_eq!(Handle::TopMid.opposite(), Handle::BottomMid);
        // Opposite is an involution.
        for h in Handle::ALL {
            assert_eq!(h.opposite().opposite(), h);
        }
    }

    #[test]
    fn shear_slants_one_axis() {
        // Horizontal shear by 1: x slides by 1·y; verticals lean over.
        let m = Affine::shear(1.0, 0.0);
        assert!(approx_pt(m.apply_point(0.0, 0.0), (0.0, 0.0)));
        assert!(approx_pt(m.apply_point(0.0, 10.0), (10.0, 10.0)));
        // A vertical shear leaves x, slides y by shy·x.
        let mv = Affine::shear(0.0, 0.5);
        assert!(approx_pt(mv.apply_point(10.0, 0.0), (10.0, 5.0)));
    }

    #[test]
    fn shear_about_keeps_pivot_fixed() {
        let m = Affine::shear_about(0.5, 0.0, 100.0, 50.0);
        assert!(approx_pt(m.apply_point(100.0, 50.0), (100.0, 50.0)));
        // A point 10 below the pivot slides right by 0.5·10 = 5.
        assert!(approx_pt(m.apply_point(100.0, 60.0), (105.0, 60.0)));
    }

    #[test]
    fn reflect_about_zero_is_vertical_flip() {
        // angle 0 mirrors across the horizontal line y = py (a vertical flip).
        let m = Affine::reflect_about(0.0, 0.0, 50.0);
        assert!(approx_pt(m.apply_point(7.0, 50.0), (7.0, 50.0)));
        assert!(approx_pt(m.apply_point(7.0, 70.0), (7.0, 30.0)));
    }

    #[test]
    fn reflect_about_half_pi_is_horizontal_flip() {
        // angle π/2 mirrors across the vertical line x = px (a horizontal flip).
        let m = Affine::reflect_about(std::f32::consts::FRAC_PI_2, 100.0, 0.0);
        assert!(approx_pt(m.apply_point(100.0, 7.0), (100.0, 7.0)));
        assert!(approx_pt(m.apply_point(120.0, 7.0), (80.0, 7.0)));
    }

    #[test]
    fn reflect_is_an_involution() {
        // Reflecting twice across the same line is the identity.
        let m = Affine::reflect_about(0.6, 12.0, -8.0);
        let twice = m.then(m);
        assert!(twice.is_identity());
    }

    #[test]
    fn top_edge_shears_horizontally() {
        // Box 200 wide, 100 tall. Drag the top edge 50px right: shx = -dx/h so the
        // top (above the bottom pivot) leans right.
        let (shx, shy) = shear_factors_for_handle(Handle::TopMid, 200.0, 100.0, 50.0, 999.0);
        assert!(approx(shx, -0.5));
        assert!(approx(shy, 0.0)); // y ignored on a horizontal edge
    }

    #[test]
    fn side_edge_shears_vertically() {
        let (shx, shy) = shear_factors_for_handle(Handle::MidRight, 200.0, 100.0, 999.0, 40.0);
        assert!(approx(shx, 0.0));
        assert!(approx(shy, 0.2)); // dy / width = 40/200
    }

    #[test]
    fn corner_handle_does_not_shear() {
        let (shx, shy) = shear_factors_for_handle(Handle::TopRight, 200.0, 100.0, 50.0, 50.0);
        assert!(approx(shx, 0.0) && approx(shy, 0.0));
        assert_eq!(shear_pivot(Handle::TopRight, &[0.0, 0.0, 200.0, 100.0]), None);
    }

    #[test]
    fn shear_pivot_is_opposite_edge_midpoint() {
        let bbox = [10.0, 20.0, 200.0, 100.0];
        // Dragging the top edge pivots about the bottom edge midpoint.
        assert_eq!(shear_pivot(Handle::TopMid, &bbox), Some((110.0, 120.0)));
        // Dragging the right edge pivots about the left edge midpoint.
        assert_eq!(shear_pivot(Handle::MidRight, &bbox), Some((10.0, 70.0)));
    }

    #[test]
    fn numeric_identity_is_noop() {
        let m = NumericTransform::default().to_affine(33.0, 44.0);
        assert!(m.is_identity());
    }

    #[test]
    fn numeric_translate_only_moves() {
        let nt = NumericTransform {
            move_x: 10.0,
            move_y: -5.0,
            ..Default::default()
        };
        let m = nt.to_affine(0.0, 0.0);
        assert!(approx_pt(m.apply_point(1.0, 1.0), (11.0, -4.0)));
    }

    #[test]
    fn numeric_scale_about_pivot() {
        let nt = NumericTransform {
            scale_x: 2.0,
            scale_y: 3.0,
            ..Default::default()
        };
        let m = nt.to_affine(100.0, 50.0);
        assert!(approx_pt(m.apply_point(100.0, 50.0), (100.0, 50.0)));
        assert!(approx_pt(m.apply_point(110.0, 60.0), (120.0, 80.0)));
    }

    #[test]
    fn numeric_compose_scale_then_rotate() {
        // Scale ×2 then rotate 90° CW about the origin. (2,0) -> (4,0) -> (0,4).
        let nt = NumericTransform {
            scale_x: 2.0,
            scale_y: 2.0,
            rotate: std::f32::consts::FRAC_PI_2,
            ..Default::default()
        };
        let m = nt.to_affine(0.0, 0.0);
        assert!(approx_pt(m.apply_point(2.0, 0.0), (0.0, 4.0)));
    }

    #[test]
    fn numeric_shear_uses_angle_tangent() {
        // 45° x-shear has tan = 1: a point 10 below the pivot slides right by 10.
        let nt = NumericTransform {
            shear_x: std::f32::consts::FRAC_PI_4,
            ..Default::default()
        };
        let m = nt.to_affine(0.0, 0.0);
        assert!(approx_pt(m.apply_point(0.0, 10.0), (10.0, 10.0)));
    }

    #[test]
    fn angle_between_quarter_turn() {
        // From the +x direction to the +y direction about the origin is +90°.
        let a = angle_between((10.0, 0.0), (0.0, 10.0), (0.0, 0.0));
        assert!(approx(a, std::f32::consts::FRAC_PI_2));
    }

    #[test]
    fn angle_between_degenerate_is_zero() {
        assert!(approx(
            angle_between((0.0, 0.0), (1.0, 1.0), (0.0, 0.0)),
            0.0
        ));
    }
}
