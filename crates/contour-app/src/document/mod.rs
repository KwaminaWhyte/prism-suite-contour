//! Contour's vector document model.
//!
//! A document is an ordered `Vec<Shape>` (paint order: index 0 painted first,
//! last index on top). All coordinates are in *document space*; the canvas maps
//! them to screen via pan/zoom. Colors are straight sRGB RGBA in `[f32; 4]`
//! (matching egui's `Rgba`/`Color32` channel order) so they round-trip cleanly
//! through the color pickers and JSON.

mod path;
mod style;
#[cfg(test)]
mod tests;

pub use path::{flatten, handle_at, nearest_segment, rects_intersect};
pub use style::{LineCap, LineJoin, StrokeStyle};

use crate::gradient::Gradient;
use crate::transform::Affine;
use kurbo::Shape as KurboShape;
use prism_core::geometry::Rect as CoreRect;
use serde::{Deserialize, Serialize};

/// Default for the additive `visible` field so pre-existing `.contour` files
/// (which lack it) deserialize as visible.
fn default_true() -> bool {
    true
}

/// One drawable vector primitive.
///
/// Every variant carries an additive `visible` flag (`#[serde(default)]`) so
/// older documents keep loading. The `Path` variant additionally carries an
/// additive `handles` list describing per-anchor cubic-bezier tangents.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum Shape {
    Rect {
        rect: [f32; 4],
        fill: [f32; 4],
        /// Optional multi-stop gradient that overrides `fill` when present.
        /// Additive (`#[serde(default)]`), so older files load as a solid fill.
        #[serde(default)]
        fill_gradient: Option<Gradient>,
        stroke: [f32; 4],
        stroke_w: f32,
        #[serde(default)]
        stroke_style: StrokeStyle,
        #[serde(default = "default_true")]
        visible: bool,
    },
    Ellipse {
        rect: [f32; 4],
        fill: [f32; 4],
        #[serde(default)]
        fill_gradient: Option<Gradient>,
        stroke: [f32; 4],
        stroke_w: f32,
        #[serde(default)]
        stroke_style: StrokeStyle,
        #[serde(default = "default_true")]
        visible: bool,
    },
    Line {
        p0: (f32, f32),
        p1: (f32, f32),
        stroke: [f32; 4],
        stroke_w: f32,
        #[serde(default)]
        stroke_style: StrokeStyle,
        #[serde(default = "default_true")]
        visible: bool,
    },
    Path {
        points: Vec<(f32, f32)>,
        closed: bool,
        fill: [f32; 4],
        #[serde(default)]
        fill_gradient: Option<Gradient>,
        stroke: [f32; 4],
        stroke_w: f32,
        #[serde(default)]
        stroke_style: StrokeStyle,
        /// Per-anchor *out-tangent* handle, stored as an offset (delta) from the
        /// anchor in document space. The in-tangent is the mirror (`-offset`),
        /// giving a smooth symmetric handle. `(0.0, 0.0)` means a corner anchor
        /// (the adjacent segments are straight lines).
        ///
        /// Additive: defaults to empty, in which case the path is a polyline and
        /// loads identically to the v0 model.
        #[serde(default)]
        handles: Vec<(f32, f32)>,
        #[serde(default = "default_true")]
        visible: bool,
    },
}

impl Shape {
    /// Short human label for the layer list.
    pub fn label(&self) -> &'static str {
        match self {
            Shape::Rect { .. } => "Rectangle",
            Shape::Ellipse { .. } => "Ellipse",
            Shape::Line { .. } => "Line",
            Shape::Path { .. } => "Path",
        }
    }

    /// Whether the shape is drawn / exported. Hidden shapes are skipped.
    pub fn visible(&self) -> bool {
        match self {
            Shape::Rect { visible, .. }
            | Shape::Ellipse { visible, .. }
            | Shape::Line { visible, .. }
            | Shape::Path { visible, .. } => *visible,
        }
    }

    /// Flip the visibility flag.
    pub fn toggle_visible(&mut self) {
        match self {
            Shape::Rect { visible, .. }
            | Shape::Ellipse { visible, .. }
            | Shape::Line { visible, .. }
            | Shape::Path { visible, .. } => *visible = !*visible,
        }
    }

    /// The shape's stroke attributes (caps/joins/dashes).
    pub fn stroke_style(&self) -> &StrokeStyle {
        match self {
            Shape::Rect { stroke_style, .. }
            | Shape::Ellipse { stroke_style, .. }
            | Shape::Line { stroke_style, .. }
            | Shape::Path { stroke_style, .. } => stroke_style,
        }
    }

    /// Mutable access to the shape's stroke attributes.
    pub fn stroke_style_mut(&mut self) -> &mut StrokeStyle {
        match self {
            Shape::Rect { stroke_style, .. }
            | Shape::Ellipse { stroke_style, .. }
            | Shape::Line { stroke_style, .. }
            | Shape::Path { stroke_style, .. } => stroke_style,
        }
    }

    /// The shape's gradient fill, if it has one (`Line` never does). When
    /// present this overrides the solid `fill` colour on every render surface.
    pub fn fill_gradient(&self) -> Option<&Gradient> {
        match self {
            Shape::Rect { fill_gradient, .. }
            | Shape::Ellipse { fill_gradient, .. }
            | Shape::Path { fill_gradient, .. } => fill_gradient.as_ref(),
            Shape::Line { .. } => None,
        }
    }

    /// Set (or clear, with `None`) the shape's gradient fill. No-op on `Line`,
    /// which has no fill region.
    pub fn set_fill_gradient(&mut self, g: Option<Gradient>) {
        match self {
            Shape::Rect { fill_gradient, .. }
            | Shape::Ellipse { fill_gradient, .. }
            | Shape::Path { fill_gradient, .. } => *fill_gradient = g,
            Shape::Line { .. } => {}
        }
    }

    /// The shape's solid fill colour, if it has a fill region (`Line` returns
    /// `None`). This is the colour used when there is no gradient, and the
    /// gradient's fallback.
    pub fn fill_color(&self) -> Option<[f32; 4]> {
        match self {
            Shape::Rect { fill, .. } | Shape::Ellipse { fill, .. } | Shape::Path { fill, .. } => {
                Some(*fill)
            }
            Shape::Line { .. } => None,
        }
    }

    /// Set the shape's solid fill colour. No-op on `Line`.
    pub fn set_fill_color(&mut self, c: [f32; 4]) {
        match self {
            Shape::Rect { fill, .. } | Shape::Ellipse { fill, .. } | Shape::Path { fill, .. } => {
                *fill = c
            }
            Shape::Line { .. } => {}
        }
    }

    /// Axis-aligned bounding box in document space.
    ///
    /// Returns a `prism_core::geometry::Rect` to exercise the shared suite
    /// primitive. Returns `None` for empty paths.
    pub fn bounds(&self) -> Option<CoreRect> {
        let bbox = |pts: &[(f32, f32)]| -> Option<CoreRect> {
            let mut it = pts.iter();
            let &(x0, y0) = it.next()?;
            let (mut min_x, mut min_y, mut max_x, mut max_y) = (x0, y0, x0, y0);
            for &(x, y) in it {
                min_x = min_x.min(x);
                min_y = min_y.min(y);
                max_x = max_x.max(x);
                max_y = max_y.max(y);
            }
            Some(CoreRect::new(min_x, min_y, max_x - min_x, max_y - min_y))
        };
        match self {
            Shape::Rect { rect, .. } | Shape::Ellipse { rect, .. } => {
                Some(CoreRect::new(rect[0], rect[1], rect[2], rect[3]))
            }
            Shape::Line { p0, p1, .. } => bbox(&[*p0, *p1]),
            Shape::Path {
                points,
                closed,
                handles,
                ..
            } => {
                if points.is_empty() {
                    return None;
                }
                // Build the kurbo BezPath (honoring any bezier handles) and let
                // kurbo compute the tight bounding box.
                let bp = path::bez_path(points, handles, *closed);
                let r = bp.bounding_box();
                Some(CoreRect::new(
                    r.x0 as f32,
                    r.y0 as f32,
                    r.width() as f32,
                    r.height() as f32,
                ))
            }
        }
    }

    /// Translate every coordinate by `(dx, dy)` in document space. Handles are
    /// offsets, so they are unaffected by translation.
    pub fn translate(&mut self, dx: f32, dy: f32) {
        match self {
            Shape::Rect { rect, .. } | Shape::Ellipse { rect, .. } => {
                rect[0] += dx;
                rect[1] += dy;
            }
            Shape::Line { p0, p1, .. } => {
                p0.0 += dx;
                p0.1 += dy;
                p1.0 += dx;
                p1.1 += dy;
            }
            Shape::Path { points, .. } => {
                for p in points.iter_mut() {
                    p.0 += dx;
                    p.1 += dy;
                }
            }
        }
    }

    /// Apply an affine transform (rotate / scale / reflect / shear) to this
    /// shape, in document space.
    ///
    /// Axis-aligned shapes (`Rect`, `Ellipse`) stay their own variant only while
    /// the transform keeps their bounding box axis-aligned (pure
    /// translate/scale/flip). Under any rotation or shear they are rasterised
    /// into a [`Shape::Path`] that traces the transformed outline, exactly the
    /// way Illustrator turns a rotated rectangle into an editable path under the
    /// hood. `Line`/`Path` always transform in place (handles transform by the
    /// matrix's *linear* part, since they are offsets).
    pub fn apply_affine(&mut self, m: &Affine) {
        // A transform is "axis-aligned" if it has no rotation or shear, i.e. the
        // off-diagonal coefficients are (numerically) zero.
        let axis_aligned = m.b.abs() < 1e-6 && m.c.abs() < 1e-6;
        match self {
            Shape::Rect { rect, .. } | Shape::Ellipse { rect, .. } if axis_aligned => {
                let (x0, y0) = m.apply_point(rect[0], rect[1]);
                let (x1, y1) = m.apply_point(rect[0] + rect[2], rect[1] + rect[3]);
                // Re-normalise so width/height stay non-negative after a flip.
                rect[0] = x0.min(x1);
                rect[1] = y0.min(y1);
                rect[2] = (x1 - x0).abs();
                rect[3] = (y1 - y0).abs();
            }
            Shape::Rect { .. } | Shape::Ellipse { .. } => {
                // Rotation/shear: convert to a path tracing the outline, then
                // transform that path.
                *self = self.to_path();
                self.apply_affine(m);
            }
            Shape::Line { p0, p1, .. } => {
                *p0 = m.apply_point(p0.0, p0.1);
                *p1 = m.apply_point(p1.0, p1.1);
            }
            Shape::Path {
                points, handles, ..
            } => {
                for p in points.iter_mut() {
                    *p = m.apply_point(p.0, p.1);
                }
                for h in handles.iter_mut() {
                    *h = m.apply_vector(h.0, h.1);
                }
            }
        }
    }

    /// Convert this shape into an equivalent [`Shape::Path`], preserving paint
    /// style. `Rect` becomes a four-corner closed corner-path; `Ellipse` becomes
    /// a four-anchor closed cubic approximation; `Line` becomes a two-point open
    /// path; an existing `Path` is returned unchanged.
    pub fn to_path(&self) -> Shape {
        match self {
            Shape::Path { .. } => self.clone(),
            Shape::Rect {
                rect,
                fill,
                fill_gradient,
                stroke,
                stroke_w,
                stroke_style,
                visible,
            } => {
                let pts = vec![
                    (rect[0], rect[1]),
                    (rect[0] + rect[2], rect[1]),
                    (rect[0] + rect[2], rect[1] + rect[3]),
                    (rect[0], rect[1] + rect[3]),
                ];
                let handles = vec![(0.0, 0.0); 4];
                Shape::Path {
                    points: pts,
                    closed: true,
                    fill: *fill,
                    fill_gradient: fill_gradient.clone(),
                    stroke: *stroke,
                    stroke_w: *stroke_w,
                    stroke_style: stroke_style.clone(),
                    handles,
                    visible: *visible,
                }
            }
            Shape::Ellipse {
                rect,
                fill,
                fill_gradient,
                stroke,
                stroke_w,
                stroke_style,
                visible,
            } => {
                // Four anchors at the extrema with the classic 0.5523 cubic
                // tangent so the path traces a smooth ellipse.
                let cx = rect[0] + rect[2] * 0.5;
                let cy = rect[1] + rect[3] * 0.5;
                let rx = rect[2] * 0.5;
                let ry = rect[3] * 0.5;
                const K: f32 = 0.552_284_8; // (4/3)·(√2−1)
                                            // Anchors clockwise from the rightmost point. Out-tangent offsets
                                            // are tangent to the ellipse, scaled by K·radius.
                let points = vec![
                    (cx + rx, cy), // right
                    (cx, cy + ry), // bottom
                    (cx - rx, cy), // left
                    (cx, cy - ry), // top
                ];
                let handles = vec![
                    (0.0, K * ry),  // right anchor: tangent down
                    (-K * rx, 0.0), // bottom anchor: tangent left
                    (0.0, -K * ry), // left anchor: tangent up
                    (K * rx, 0.0),  // top anchor: tangent right
                ];
                Shape::Path {
                    points,
                    closed: true,
                    fill: *fill,
                    fill_gradient: fill_gradient.clone(),
                    stroke: *stroke,
                    stroke_w: *stroke_w,
                    stroke_style: stroke_style.clone(),
                    handles,
                    visible: *visible,
                }
            }
            Shape::Line {
                p0,
                p1,
                stroke,
                stroke_w,
                stroke_style,
                visible,
            } => Shape::Path {
                points: vec![*p0, *p1],
                closed: false,
                fill: [0.0, 0.0, 0.0, 0.0],
                fill_gradient: None,
                stroke: *stroke,
                stroke_w: *stroke_w,
                stroke_style: stroke_style.clone(),
                handles: vec![(0.0, 0.0); 2],
                visible: *visible,
            },
        }
    }

    /// Insert an anchor into this path at segment `seg`, parameter `t`,
    /// preserving shape. No-op (returns `None`) on non-`Path` shapes.
    pub fn insert_anchor(&mut self, seg: usize, t: f32) -> Option<usize> {
        if let Shape::Path {
            points,
            closed,
            handles,
            ..
        } = self
        {
            path::insert_anchor(points, handles, *closed, seg, t)
        } else {
            None
        }
    }

    /// Delete anchor `i` from this path (keeps ≥2 points). No-op on non-`Path`.
    pub fn delete_anchor(&mut self, i: usize) -> bool {
        if let Shape::Path {
            points, handles, ..
        } = self
        {
            path::delete_anchor(points, handles, i)
        } else {
            false
        }
    }

    /// Toggle anchor `i` smooth↔corner on this path. Returns the new smooth
    /// state (`true` = now smooth). No-op (returns `false`) on non-`Path`.
    pub fn toggle_anchor_smooth(&mut self, i: usize) -> bool {
        if let Shape::Path {
            points,
            closed,
            handles,
            ..
        } = self
        {
            path::toggle_anchor_smooth(points, handles, *closed, i)
        } else {
            false
        }
    }

    /// Hit-test a document-space point. Tolerance is in document units (used to
    /// give lines/open paths a clickable thickness).
    pub fn hit(&self, x: f32, y: f32, tol: f32) -> bool {
        match self {
            Shape::Rect { rect, .. } => path::point_in_rect(x, y, rect, tol),
            Shape::Ellipse { rect, .. } => {
                let cx = rect[0] + rect[2] * 0.5;
                let cy = rect[1] + rect[3] * 0.5;
                let rx = (rect[2] * 0.5).max(1e-3);
                let ry = (rect[3] * 0.5).max(1e-3);
                let nx = (x - cx) / (rx + tol);
                let ny = (y - cy) / (ry + tol);
                nx * nx + ny * ny <= 1.0
            }
            Shape::Line { p0, p1, .. } => path::dist_to_segment(x, y, *p0, *p1) <= tol.max(2.0),
            Shape::Path {
                points,
                closed,
                handles,
                ..
            } => {
                // Hit-test against the flattened polyline so curves are clickable.
                let flat = path::flatten(points, handles, *closed);
                if *closed && flat.len() >= 3 && path::point_in_polygon(x, y, &flat) {
                    return true;
                }
                let n = flat.len();
                if n < 2 {
                    return n == 1 && (x - flat[0].0).hypot(y - flat[0].1) <= tol.max(2.0);
                }
                let last = if *closed { n } else { n - 1 };
                for i in 0..last {
                    let a = flat[i];
                    let b = flat[(i + 1) % n];
                    if path::dist_to_segment(x, y, a, b) <= tol.max(2.0) {
                        return true;
                    }
                }
                false
            }
        }
    }
}

/// A shape's index in the document paired with its bounding box. Used by the
/// align/distribute layer, which needs to map per-box translation deltas back to
/// the originating shape.
#[derive(Clone, Copy, Debug)]
pub struct ShapeBounds {
    pub index: usize,
    pub rect: CoreRect,
}

/// A ruler guide: an infinite straight line at a fixed document coordinate,
/// either vertical (constant `x`) or horizontal (constant `y`). Dragged out of
/// the rulers and used as a snap target. Stored on the [`Document`] so guides
/// persist in `.contour` files.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum Guide {
    /// A vertical line at this `x` (in document units).
    Vertical(f32),
    /// A horizontal line at this `y` (in document units).
    Horizontal(f32),
}

/// The whole vector document: an ordered list of shapes plus any ruler guides.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Document {
    pub shapes: Vec<Shape>,
    /// User-placed ruler guides. Additive (`#[serde(default)]`) so pre-existing
    /// `.contour` files — which have no `guides` key — load with none.
    #[serde(default)]
    pub guides: Vec<Guide>,
}

impl Document {
    pub fn new() -> Self {
        Self::default()
    }
}
