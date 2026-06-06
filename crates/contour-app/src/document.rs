//! Contour's vector document model.
//!
//! A document is an ordered `Vec<Shape>` (paint order: index 0 painted first,
//! last index on top). All coordinates are in *document space*; the canvas maps
//! them to screen via pan/zoom. Colors are straight sRGB RGBA in `[f32; 4]`
//! (matching egui's `Rgba`/`Color32` channel order) so they round-trip cleanly
//! through the color pickers and JSON.

use kurbo::{BezPath, PathEl, Point, Shape as KurboShape};
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
        stroke: [f32; 4],
        stroke_w: f32,
        #[serde(default = "default_true")]
        visible: bool,
    },
    Ellipse {
        rect: [f32; 4],
        fill: [f32; 4],
        stroke: [f32; 4],
        stroke_w: f32,
        #[serde(default = "default_true")]
        visible: bool,
    },
    Line {
        p0: (f32, f32),
        p1: (f32, f32),
        stroke: [f32; 4],
        stroke_w: f32,
        #[serde(default = "default_true")]
        visible: bool,
    },
    Path {
        points: Vec<(f32, f32)>,
        closed: bool,
        fill: [f32; 4],
        stroke: [f32; 4],
        stroke_w: f32,
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
                let bp = bez_path(points, handles, *closed);
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

    /// Hit-test a document-space point. Tolerance is in document units (used to
    /// give lines/open paths a clickable thickness).
    pub fn hit(&self, x: f32, y: f32, tol: f32) -> bool {
        match self {
            Shape::Rect { rect, .. } => point_in_rect(x, y, rect, tol),
            Shape::Ellipse { rect, .. } => {
                let cx = rect[0] + rect[2] * 0.5;
                let cy = rect[1] + rect[3] * 0.5;
                let rx = (rect[2] * 0.5).max(1e-3);
                let ry = (rect[3] * 0.5).max(1e-3);
                let nx = (x - cx) / (rx + tol);
                let ny = (y - cy) / (ry + tol);
                nx * nx + ny * ny <= 1.0
            }
            Shape::Line { p0, p1, .. } => dist_to_segment(x, y, *p0, *p1) <= tol.max(2.0),
            Shape::Path {
                points,
                closed,
                handles,
                ..
            } => {
                // Hit-test against the flattened polyline so curves are clickable.
                let flat = flatten(points, handles, *closed);
                if *closed && flat.len() >= 3 && point_in_polygon(x, y, &flat) {
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
                    if dist_to_segment(x, y, a, b) <= tol.max(2.0) {
                        return true;
                    }
                }
                false
            }
        }
    }
}

/// The out-tangent handle offset for anchor `i`, or `(0, 0)` (a corner) when no
/// handle is stored.
pub fn handle_at(handles: &[(f32, f32)], i: usize) -> (f32, f32) {
    handles.get(i).copied().unwrap_or((0.0, 0.0))
}

/// Build a kurbo [`BezPath`] from anchor points and per-anchor out-tangent
/// handle offsets. A segment between anchors `a` and `b` is a `CurveTo` when
/// either endpoint carries a non-zero handle, otherwise a straight `LineTo`.
///
/// The out-handle of `a` is `a + handle[a]`; the in-handle of `b` is
/// `b - handle[b]` (mirror), producing smooth symmetric tangents.
pub fn bez_path(points: &[(f32, f32)], handles: &[(f32, f32)], closed: bool) -> BezPath {
    let mut els: Vec<PathEl> = Vec::with_capacity(points.len() + 2);
    let n = points.len();
    if n == 0 {
        return BezPath::from_vec(els);
    }
    let pt = |p: (f32, f32)| Point::new(p.0 as f64, p.1 as f64);
    els.push(PathEl::MoveTo(pt(points[0])));

    let seg_count = if closed { n } else { n - 1 };
    for i in 0..seg_count {
        let a = points[i];
        let b = points[(i + 1) % n];
        let ha = handle_at(handles, i);
        let hb = handle_at(handles, (i + 1) % n);
        let a_corner = ha.0 == 0.0 && ha.1 == 0.0;
        let b_corner = hb.0 == 0.0 && hb.1 == 0.0;
        if a_corner && b_corner {
            els.push(PathEl::LineTo(pt(b)));
        } else {
            let c1 = (a.0 + ha.0, a.1 + ha.1); // out-handle of a
            let c2 = (b.0 - hb.0, b.1 - hb.1); // in-handle of b (mirror)
            els.push(PathEl::CurveTo(pt(c1), pt(c2), pt(b)));
        }
    }
    if closed {
        els.push(PathEl::ClosePath);
    }
    BezPath::from_vec(els)
}

/// Flatten a (possibly bezier) path into a polyline of document-space points.
/// Used for hit-testing, polygon fills, and boolean-op input.
pub fn flatten(points: &[(f32, f32)], handles: &[(f32, f32)], closed: bool) -> Vec<(f32, f32)> {
    let any_curve = handles.iter().any(|&(hx, hy)| hx != 0.0 || hy != 0.0);
    if !any_curve {
        return points.to_vec();
    }
    let bp = bez_path(points, handles, closed);
    let mut out: Vec<(f32, f32)> = Vec::new();
    // kurbo flattens to line segments at the given tolerance (document units).
    bp.flatten(0.25, |el| match el {
        PathEl::MoveTo(p) => out.push((p.x as f32, p.y as f32)),
        PathEl::LineTo(p) => out.push((p.x as f32, p.y as f32)),
        PathEl::ClosePath => {}
        // flatten only emits MoveTo/LineTo/ClosePath.
        _ => {}
    });
    out
}

fn point_in_rect(x: f32, y: f32, rect: &[f32; 4], tol: f32) -> bool {
    let (x0, y0) = (
        rect[0].min(rect[0] + rect[2]),
        rect[1].min(rect[1] + rect[3]),
    );
    let (x1, y1) = (
        rect[0].max(rect[0] + rect[2]),
        rect[1].max(rect[1] + rect[3]),
    );
    x >= x0 - tol && x <= x1 + tol && y >= y0 - tol && y <= y1 + tol
}

fn dist_to_segment(px: f32, py: f32, a: (f32, f32), b: (f32, f32)) -> f32 {
    let (ax, ay) = a;
    let (bx, by) = b;
    let dx = bx - ax;
    let dy = by - ay;
    let len2 = dx * dx + dy * dy;
    if len2 <= 1e-6 {
        return (px - ax).hypot(py - ay);
    }
    let t = (((px - ax) * dx + (py - ay) * dy) / len2).clamp(0.0, 1.0);
    let cx = ax + t * dx;
    let cy = ay + t * dy;
    (px - cx).hypot(py - cy)
}

fn point_in_polygon(px: f32, py: f32, pts: &[(f32, f32)]) -> bool {
    let mut inside = false;
    let n = pts.len();
    let mut j = n - 1;
    for i in 0..n {
        let (xi, yi) = pts[i];
        let (xj, yj) = pts[j];
        if (yi > py) != (yj > py) {
            let x_int = (xj - xi) * (py - yi) / (yj - yi) + xi;
            if px < x_int {
                inside = !inside;
            }
        }
        j = i;
    }
    inside
}

/// The whole vector document: an ordered list of shapes.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Document {
    pub shapes: Vec<Shape>,
}

impl Document {
    pub fn new() -> Self {
        Self::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Old `.contour` JSON (pre-`visible`/`handles`) must still deserialize,
    /// defaulting `visible = true` and `handles = []`.
    #[test]
    fn loads_legacy_document() {
        let json = r#"{"shapes":[
            {"Rect":{"rect":[0,0,10,10],"fill":[1,0,0,1],"stroke":[0,0,0,1],"stroke_w":2}},
            {"Path":{"points":[[0,0],[10,0],[10,10]],"closed":true,"fill":[0,1,0,1],"stroke":[0,0,0,1],"stroke_w":1}}
        ]}"#;
        let doc: Document = serde_json::from_str(json).unwrap();
        assert_eq!(doc.shapes.len(), 2);
        assert!(doc.shapes[0].visible());
        assert!(doc.shapes[1].visible());
        if let Shape::Path { handles, .. } = &doc.shapes[1] {
            assert!(handles.is_empty());
        } else {
            panic!("expected Path");
        }
    }

    /// A path with no handles flattens to its raw points (polyline).
    #[test]
    fn flatten_polyline_is_identity() {
        let pts = vec![(0.0, 0.0), (10.0, 0.0), (10.0, 10.0)];
        let out = flatten(&pts, &[], false);
        assert_eq!(out, pts);
    }

    /// A path with a non-zero handle flattens into more segments (a curve).
    #[test]
    fn flatten_curve_subdivides() {
        let pts = vec![(0.0, 0.0), (100.0, 0.0)];
        let handles = vec![(0.0, 50.0), (0.0, 50.0)];
        let out = flatten(&pts, &handles, false);
        assert!(out.len() > 2, "curve should subdivide, got {}", out.len());
    }
}
