//! The drawing surface: pan/zoom transform, per-frame shape painting, and tool
//! interaction (create / select / move / pen).

use crate::document::{self, Shape, StrokeStyle};
use crate::theme;
use crate::transform::Handle;
use egui::epaint::CubicBezierShape;
use egui::{Color32, Pos2, Rect, Stroke, Vec2};

use prism_core::color::{linear_to_srgb, srgb_to_linear};

/// Maps document space <-> screen space. `pan` is the screen position of the
/// document origin; `zoom` is screen-pixels per document-unit.
#[derive(Clone, Copy)]
pub struct View {
    pub pan: Vec2,
    pub zoom: f32,
}

impl Default for View {
    fn default() -> Self {
        Self {
            pan: Vec2::new(80.0, 80.0),
            zoom: 1.0,
        }
    }
}

impl View {
    pub fn doc_to_screen(&self, p: (f32, f32)) -> Pos2 {
        Pos2::new(p.0 * self.zoom + self.pan.x, p.1 * self.zoom + self.pan.y)
    }

    pub fn screen_to_doc(&self, p: Pos2) -> (f32, f32) {
        (
            (p.x - self.pan.x) / self.zoom,
            (p.y - self.pan.y) / self.zoom,
        )
    }
}

/// Convert a straight sRGB `[f32;4]` (0..=1) into an egui color.
///
/// egui's `Color32::from_rgba_unmultiplied` expects sRGB bytes, so we just
/// scale. The `srgb_to_linear`/`linear_to_srgb` round-trip below is a no-op in
/// value but demonstrates the shared color helpers from `prism-core` at the
/// app boundary (and keeps the import meaningful).
pub fn to_color32(c: [f32; 4]) -> Color32 {
    let enc = |v: f32| (linear_to_srgb(srgb_to_linear(v.clamp(0.0, 1.0))) * 255.0).round() as u8;
    Color32::from_rgba_unmultiplied(
        enc(c[0]),
        enc(c[1]),
        enc(c[2]),
        (c[3].clamp(0.0, 1.0) * 255.0).round() as u8,
    )
}

/// Paint one shape using the painter, transforming document coords to screen.
pub fn paint_shape(painter: &egui::Painter, view: &View, shape: &Shape, selected: bool) {
    let style = shape.stroke_style();
    match shape {
        Shape::Rect {
            rect,
            fill,
            stroke,
            stroke_w,
            ..
        } => {
            let r = doc_rect(view, rect);
            painter.rect_filled(r, 0.0, to_color32(*fill));
            if *stroke_w > 0.0 {
                // A rect's outline is its 4 corners as a closed polyline so we
                // can honor a dash pattern; solid strokes use the fast path.
                let ring = [
                    (rect[0], rect[1]),
                    (rect[0] + rect[2], rect[1]),
                    (rect[0] + rect[2], rect[1] + rect[3]),
                    (rect[0], rect[1] + rect[3]),
                ];
                stroke_polyline(painter, view, &ring, true, *stroke, *stroke_w, style);
            }
        }
        Shape::Ellipse {
            rect,
            fill,
            stroke,
            stroke_w,
            ..
        } => {
            let pts = ellipse_points(view, rect, 48);
            painter.add(egui::Shape::convex_polygon(
                pts.clone(),
                to_color32(*fill),
                Stroke::NONE,
            ));
            if *stroke_w > 0.0 {
                let ring: Vec<(f32, f32)> = ellipse_doc_points(rect, 48);
                stroke_polyline(painter, view, &ring, true, *stroke, *stroke_w, style);
            }
        }
        Shape::Line {
            p0,
            p1,
            stroke,
            stroke_w,
            ..
        } => {
            stroke_polyline(
                painter,
                view,
                &[*p0, *p1],
                false,
                *stroke,
                stroke_w.max(0.5),
                style,
            );
        }
        Shape::Path {
            points,
            closed,
            fill,
            stroke,
            stroke_w,
            handles,
            ..
        } => {
            paint_path(
                painter, view, points, handles, *closed, *fill, *stroke, *stroke_w, style,
            );
        }
    }

    if selected {
        if let Some(b) = shape.bounds() {
            let r = doc_rect(view, &[b.x, b.y, b.w, b.h]);
            painter.rect_stroke(
                r.expand(2.0),
                0.0,
                Stroke::new(1.5, theme::accent()),
                egui::StrokeKind::Outside,
            );
        }
    }
}

/// Paint a (possibly cubic-bezier) path. Straight segments use a polyline /
/// polygon; curved segments are drawn with egui's [`CubicBezierShape`]. A
/// dashed [`StrokeStyle`] forces flattening so the dash pattern can be applied
/// uniformly along the (curved) outline.
#[allow(clippy::too_many_arguments)]
pub fn paint_path(
    painter: &egui::Painter,
    view: &View,
    points: &[(f32, f32)],
    handles: &[(f32, f32)],
    closed: bool,
    fill: [f32; 4],
    stroke: [f32; 4],
    stroke_w: f32,
    style: &StrokeStyle,
) {
    let n = points.len();
    if n < 2 {
        return;
    }
    let stroke32 = Stroke::new(stroke_w.max(0.5) * view.zoom, to_color32(stroke));
    let any_curve = handles.iter().any(|&(hx, hy)| hx != 0.0 || hy != 0.0);

    // Fill: flatten to a polygon (closed paths only).
    if closed {
        let flat: Vec<Pos2> = document::flatten(points, handles, true)
            .iter()
            .map(|&p| view.doc_to_screen(p))
            .collect();
        if flat.len() >= 3 {
            painter.add(egui::Shape::convex_polygon(
                flat,
                to_color32(fill),
                Stroke::NONE,
            ));
        }
    }

    // Dashed curves can't use the per-segment bezier fast path: flatten the
    // whole outline so the dash phase carries across segment boundaries.
    if style.is_dashed() {
        let flat = document::flatten(points, handles, closed);
        stroke_polyline(painter, view, &flat, closed, stroke, stroke_w, style);
        return;
    }

    if !any_curve {
        // Pure solid polyline / polygon outline.
        let pts: Vec<Pos2> = points.iter().map(|&p| view.doc_to_screen(p)).collect();
        if closed {
            let mut ring = pts.clone();
            ring.push(pts[0]);
            painter.add(egui::Shape::line(ring, stroke32));
        } else {
            painter.add(egui::Shape::line(pts, stroke32));
        }
        return;
    }

    // Solid curve: stroke each segment, choosing line vs. cubic per segment.
    let seg_count = if closed { n } else { n - 1 };
    for i in 0..seg_count {
        let a = points[i];
        let b = points[(i + 1) % n];
        let ha = document::handle_at(handles, i);
        let hb = document::handle_at(handles, (i + 1) % n);
        let a_corner = ha.0 == 0.0 && ha.1 == 0.0;
        let b_corner = hb.0 == 0.0 && hb.1 == 0.0;
        if a_corner && b_corner {
            painter.line_segment([view.doc_to_screen(a), view.doc_to_screen(b)], stroke32);
        } else {
            let c1 = view.doc_to_screen((a.0 + ha.0, a.1 + ha.1));
            let c2 = view.doc_to_screen((b.0 - hb.0, b.1 - hb.1));
            let bez = CubicBezierShape::from_points_stroke(
                [view.doc_to_screen(a), c1, c2, view.doc_to_screen(b)],
                false,
                Color32::TRANSPARENT,
                stroke32,
            );
            painter.add(bez);
        }
    }
}

/// Stroke a polyline (a flat list of document-space points) honoring the
/// dash pattern in `style`. `closed` repeats the first point to close the ring.
/// Solid strokes take egui's fast line path; dashed strokes are emitted with
/// [`egui::Shape::dashed_line_many_with_offset`], scaling the document-unit dash
/// runs by the current zoom so dashes keep their on-screen length stable.
fn stroke_polyline(
    painter: &egui::Painter,
    view: &View,
    points: &[(f32, f32)],
    closed: bool,
    stroke: [f32; 4],
    stroke_w: f32,
    style: &StrokeStyle,
) {
    if points.len() < 2 {
        return;
    }
    let stroke32 = Stroke::new(stroke_w.max(0.5) * view.zoom, to_color32(stroke));
    let mut screen: Vec<Pos2> = points.iter().map(|&p| view.doc_to_screen(p)).collect();
    if closed {
        screen.push(screen[0]);
    }

    match style.normalized_dash() {
        None => {
            painter.add(egui::Shape::line(screen, stroke32));
        }
        Some(runs) => {
            // `runs` alternates on,off,on,off… Split into the dash (on) and gap
            // (off) arrays egui expects, scaling each run to screen pixels.
            let z = view.zoom;
            let dashes: Vec<f32> = runs.iter().step_by(2).map(|&d| (d * z).max(0.01)).collect();
            let gaps: Vec<f32> = runs
                .iter()
                .skip(1)
                .step_by(2)
                .map(|&d| (d * z).max(0.0))
                .collect();
            let mut shapes = Vec::new();
            egui::Shape::dashed_line_many_with_offset(
                &screen,
                stroke32,
                &dashes,
                &gaps,
                style.dash_offset * z,
                &mut shapes,
            );
            painter.extend(shapes);
        }
    }
}

/// Ellipse outline as document-space points (mirror of [`ellipse_points`] but
/// untransformed, for dashed stroking).
fn ellipse_doc_points(rect: &[f32; 4], segments: usize) -> Vec<(f32, f32)> {
    let cx = rect[0] + rect[2] * 0.5;
    let cy = rect[1] + rect[3] * 0.5;
    let rx = rect[2] * 0.5;
    let ry = rect[3] * 0.5;
    (0..segments)
        .map(|i| {
            let t = i as f32 / segments as f32 * std::f32::consts::TAU;
            (cx + rx * t.cos(), cy + ry * t.sin())
        })
        .collect()
}

/// Draw editable anchor dots and tangent handles for a selected path. Returns
/// nothing; pure overlay. Anchor radius ~4px, handle dots ~3px.
pub fn paint_path_handles(
    painter: &egui::Painter,
    view: &View,
    points: &[(f32, f32)],
    handles: &[(f32, f32)],
) {
    for (i, &p) in points.iter().enumerate() {
        let anchor = view.doc_to_screen(p);
        let h = document::handle_at(handles, i);
        let smooth = h.0 != 0.0 || h.1 != 0.0;
        if smooth {
            let out = view.doc_to_screen((p.0 + h.0, p.1 + h.1));
            let inp = view.doc_to_screen((p.0 - h.0, p.1 - h.1));
            let line = Stroke::new(1.0, theme::accent());
            painter.line_segment([anchor, out], line);
            painter.line_segment([anchor, inp], line);
            painter.circle_filled(out, 3.5, theme::accent());
            painter.circle_filled(inp, 3.5, theme::accent());
        }
        // Anchor glyph: smooth anchors are round, corner anchors are square,
        // matching the Illustrator convention. Both are white with an accent ring.
        let ring = Stroke::new(1.5, theme::accent());
        if smooth {
            painter.circle_filled(anchor, 4.0, Color32::WHITE);
            painter.circle_stroke(anchor, 4.0, ring);
        } else {
            let sq = Rect::from_center_size(anchor, Vec2::splat(7.0));
            painter.rect_filled(sq, 0.0, Color32::WHITE);
            painter.rect_stroke(sq, 0.0, ring, egui::StrokeKind::Outside);
        }
    }
}

/// Screen-space position of transform `handle` on the document-space box
/// `[x, y, w, h]`.
pub fn handle_screen_pos(view: &View, bbox: &[f32; 4], handle: Handle) -> Pos2 {
    let (fx, fy) = handle.unit_pos();
    view.doc_to_screen((bbox[0] + bbox[2] * fx, bbox[1] + bbox[3] * fy))
}

/// On-screen size of a transform handle marker (square side, in pixels).
pub const HANDLE_PX: f32 = 8.0;
/// Pick radius (pixels) for grabbing a handle.
pub const HANDLE_PICK_PX: f32 = 7.0;
/// How far diagonally outside a corner the rotate ring begins (pixels).
pub const ROTATE_PICK_PX: f32 = 18.0;

/// Draw the free-transform box: a dashed outline around the selection bounds
/// plus the eight scale handles (white squares with an accent ring). Drawn only
/// for the Select tool when there is no per-path anchor edit in progress.
pub fn paint_transform_box(painter: &egui::Painter, view: &View, bbox: &[f32; 4]) {
    let tl = view.doc_to_screen((bbox[0], bbox[1]));
    let br = view.doc_to_screen((bbox[0] + bbox[2], bbox[1] + bbox[3]));
    let rect = Rect::from_two_pos(tl, br);

    // Dashed bounding outline so it reads distinctly from a plain selection ring.
    let stroke = Stroke::new(1.0, theme::accent());
    let mut dashed = Vec::new();
    let ring = [
        rect.left_top(),
        rect.right_top(),
        rect.right_bottom(),
        rect.left_bottom(),
        rect.left_top(),
    ];
    egui::Shape::dashed_line_many_with_offset(&ring, stroke, &[4.0], &[3.0], 0.0, &mut dashed);
    painter.extend(dashed);

    // Eight handle markers.
    let ringv = Stroke::new(1.25, theme::accent());
    for h in Handle::ALL {
        let p = handle_screen_pos(view, bbox, h);
        let sq = Rect::from_center_size(p, Vec2::splat(HANDLE_PX));
        painter.rect_filled(sq, 0.0, Color32::WHITE);
        painter.rect_stroke(sq, 0.0, ringv, egui::StrokeKind::Outside);
    }
}

fn doc_rect(view: &View, rect: &[f32; 4]) -> Rect {
    let a = view.doc_to_screen((rect[0], rect[1]));
    let b = view.doc_to_screen((rect[0] + rect[2], rect[1] + rect[3]));
    Rect::from_two_pos(a, b)
}

fn ellipse_points(view: &View, rect: &[f32; 4], segments: usize) -> Vec<Pos2> {
    let cx = rect[0] + rect[2] * 0.5;
    let cy = rect[1] + rect[3] * 0.5;
    let rx = rect[2] * 0.5;
    let ry = rect[3] * 0.5;
    (0..segments)
        .map(|i| {
            let t = i as f32 / segments as f32 * std::f32::consts::TAU;
            view.doc_to_screen((cx + rx * t.cos(), cy + ry * t.sin()))
        })
        .collect()
}

/// Paint the artboard background frame for a document of the given size.
pub fn paint_artboard(painter: &egui::Painter, view: &View, w: f32, h: f32) {
    let r = doc_rect(view, &[0.0, 0.0, w, h]);
    painter.rect_filled(r, 0.0, Color32::from_rgb(0xf4, 0xf5, 0xf7));
    painter.rect_stroke(
        r,
        0.0,
        Stroke::new(1.0, Color32::from_rgb(0x50, 0x57, 0x60)),
        egui::StrokeKind::Outside,
    );
}

/// Handle scroll-to-zoom anchored at the cursor. Mutates `view`.
pub fn handle_zoom(view: &mut View, response: &egui::Response, ctx: &egui::Context) {
    let scroll = ctx.input(|i| i.smooth_scroll_delta.y);
    if scroll.abs() < f32::EPSILON {
        return;
    }
    let Some(cursor) = response.hover_pos() else {
        return;
    };
    let before = view.screen_to_doc(cursor);
    let factor = (scroll * 0.0015).exp();
    view.zoom = (view.zoom * factor).clamp(0.05, 64.0);
    // Re-anchor so the doc point under the cursor stays put.
    view.pan.x = cursor.x - before.0 * view.zoom;
    view.pan.y = cursor.y - before.1 * view.zoom;
}
