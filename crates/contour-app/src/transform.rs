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
#[derive(Clone, Copy, Debug, PartialEq)]
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

    /// Reflect across the vertical line `x = px` (a horizontal flip), keeping
    /// that line fixed.
    pub fn flip_h_about(px: f32) -> Affine {
        Affine::scale_about(-1.0, 1.0, px, 0.0)
    }

    /// Reflect across the horizontal line `y = py` (a vertical flip).
    pub fn flip_v_about(py: f32) -> Affine {
        Affine::scale_about(1.0, -1.0, 0.0, py)
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
    fn flip_h_mirrors_across_line() {
        let m = Affine::flip_h_about(100.0);
        assert!(approx_pt(m.apply_point(100.0, 7.0), (100.0, 7.0))); // on the line
        assert!(approx_pt(m.apply_point(120.0, 7.0), (80.0, 7.0))); // mirrored
    }

    #[test]
    fn flip_v_mirrors_across_line() {
        let m = Affine::flip_v_about(50.0);
        assert!(approx_pt(m.apply_point(7.0, 50.0), (7.0, 50.0)));
        assert!(approx_pt(m.apply_point(7.0, 70.0), (7.0, 30.0)));
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
