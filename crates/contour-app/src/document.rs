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

/// One drawable vector primitive.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum Shape {
    Rect {
        rect: [f32; 4],
        fill: [f32; 4],
        stroke: [f32; 4],
        stroke_w: f32,
    },
    Ellipse {
        rect: [f32; 4],
        fill: [f32; 4],
        stroke: [f32; 4],
        stroke_w: f32,
    },
    Line {
        p0: (f32, f32),
        p1: (f32, f32),
        stroke: [f32; 4],
        stroke_w: f32,
    },
    Path {
        points: Vec<(f32, f32)>,
        closed: bool,
        fill: [f32; 4],
        stroke: [f32; 4],
        stroke_w: f32,
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
            Shape::Path { points, closed, .. } => {
                if points.is_empty() {
                    return None;
                }
                // Build a kurbo BezPath (the pen-tool path model) and let kurbo
                // compute the tight bounding box. Lays groundwork for real bezier
                // segments later.
                let bp = bez_path(points, *closed);
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

    /// Translate every coordinate by `(dx, dy)` in document space.
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
            Shape::Path { points, closed, .. } => {
                if *closed && points.len() >= 3 && point_in_polygon(x, y, points) {
                    return true;
                }
                let n = points.len();
                if n < 2 {
                    return n == 1 && (x - points[0].0).hypot(y - points[0].1) <= tol.max(2.0);
                }
                let last = if *closed { n } else { n - 1 };
                for i in 0..last {
                    let a = points[i];
                    let b = points[(i + 1) % n];
                    if dist_to_segment(x, y, a, b) <= tol.max(2.0) {
                        return true;
                    }
                }
                false
            }
        }
    }
}

/// Build a kurbo [`BezPath`] from polyline points (the v0 pen model uses
/// straight segments; this is the seam where bezier control points slot in).
fn bez_path(points: &[(f32, f32)], closed: bool) -> BezPath {
    let mut els: Vec<PathEl> = Vec::with_capacity(points.len() + 2);
    let mut it = points.iter();
    if let Some(&(x, y)) = it.next() {
        els.push(PathEl::MoveTo(Point::new(x as f64, y as f64)));
        for &(x, y) in it {
            els.push(PathEl::LineTo(Point::new(x as f64, y as f64)));
        }
        if closed {
            els.push(PathEl::ClosePath);
        }
    }
    BezPath::from_vec(els)
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
