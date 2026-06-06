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

/// Draw the rubber-band marquee selection box: a translucent accent fill with a
/// dashed accent outline, given the box as document-space `[x, y, w, h]`.
pub fn paint_marquee(painter: &egui::Painter, view: &View, bbox: &[f32; 4]) {
    let rect = doc_rect(view, bbox);
    let accent = theme::accent();
    let fill = Color32::from_rgba_unmultiplied(accent.r(), accent.g(), accent.b(), 28);
    painter.rect_filled(rect, 0.0, fill);
    let stroke = Stroke::new(1.0, accent);
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

/// Width of the ruler strips (top and left), in screen pixels.
pub const RULER_PX: f32 = 18.0;

/// Document-space coordinate of a guide, classified by orientation, for drawing.
use crate::document::Guide;

/// Paint the document grid across `clip`, every `spacing` document units. Skips
/// drawing when the on-screen spacing would be too dense to read.
pub fn paint_grid(painter: &egui::Painter, view: &View, clip: Rect, spacing: f32) {
    if spacing <= 0.0 {
        return;
    }
    let step_px = spacing * view.zoom;
    if step_px < 4.0 {
        return; // too dense — would just be a grey wash
    }
    let minor = Stroke::new(1.0, Color32::from_rgba_unmultiplied(0x4a, 0x52, 0x5c, 90));
    let major = Stroke::new(1.0, Color32::from_rgba_unmultiplied(0x5a, 0x64, 0x70, 130));

    // Document-space extent currently visible.
    let (dx0, dy0) = view.screen_to_doc(clip.left_top());
    let (dx1, dy1) = view.screen_to_doc(clip.right_bottom());

    let start_i = (dx0 / spacing).floor() as i64;
    let end_i = (dx1 / spacing).ceil() as i64;
    for i in start_i..=end_i {
        let x = i as f32 * spacing;
        let sx = view.doc_to_screen((x, 0.0)).x;
        let stroke = if i % 5 == 0 { major } else { minor };
        painter.line_segment(
            [Pos2::new(sx, clip.top()), Pos2::new(sx, clip.bottom())],
            stroke,
        );
    }
    let start_j = (dy0 / spacing).floor() as i64;
    let end_j = (dy1 / spacing).ceil() as i64;
    for j in start_j..=end_j {
        let y = j as f32 * spacing;
        let sy = view.doc_to_screen((0.0, y)).y;
        let stroke = if j % 5 == 0 { major } else { minor };
        painter.line_segment(
            [Pos2::new(clip.left(), sy), Pos2::new(clip.right(), sy)],
            stroke,
        );
    }
}

/// Paint the document's ruler guides (full-canvas cyan lines) clipped to `clip`.
pub fn paint_guides(painter: &egui::Painter, view: &View, clip: Rect, guides: &[Guide]) {
    let stroke = Stroke::new(1.0, Color32::from_rgb(0x2d, 0xc6, 0xd6));
    for g in guides {
        match *g {
            Guide::Vertical(x) => {
                let sx = view.doc_to_screen((x, 0.0)).x;
                if sx >= clip.left() && sx <= clip.right() {
                    painter.line_segment(
                        [Pos2::new(sx, clip.top()), Pos2::new(sx, clip.bottom())],
                        stroke,
                    );
                }
            }
            Guide::Horizontal(y) => {
                let sy = view.doc_to_screen((0.0, y)).y;
                if sy >= clip.top() && sy <= clip.bottom() {
                    painter.line_segment(
                        [Pos2::new(clip.left(), sy), Pos2::new(clip.right(), sy)],
                        stroke,
                    );
                }
            }
        }
    }
}

/// Paint a transient snap line (magenta, full-canvas) where the active drag is
/// snapping, like Illustrator's smart guides.
pub fn paint_snap_lines(
    painter: &egui::Painter,
    clip: Rect,
    line_x: Option<f32>,
    line_y: Option<f32>,
    view: &View,
) {
    let stroke = Stroke::new(1.0, Color32::from_rgb(0xff, 0x3d, 0xb5));
    if let Some(x) = line_x {
        let sx = view.doc_to_screen((x, 0.0)).x;
        painter.line_segment(
            [Pos2::new(sx, clip.top()), Pos2::new(sx, clip.bottom())],
            stroke,
        );
    }
    if let Some(y) = line_y {
        let sy = view.doc_to_screen((0.0, y)).y;
        painter.line_segment(
            [Pos2::new(clip.left(), sy), Pos2::new(clip.right(), sy)],
            stroke,
        );
    }
}

/// Paint the top + left ruler strips with document-unit tick labels, plus a
/// position read-out tracking `cursor` (when present). The caller derives the
/// inner content rectangle (the canvas minus the [`RULER_PX`] strips) itself.
pub fn paint_rulers(painter: &egui::Painter, view: &View, full: Rect, cursor: Option<Pos2>) {
    let bg = Color32::from_rgb(0x20, 0x23, 0x28);
    let tick = Color32::from_rgb(0x6a, 0x72, 0x7c);
    let text_col = Color32::from_rgb(0x9a, 0xa1, 0xab);
    let font = egui::FontId::monospace(9.0);

    let top = Rect::from_min_max(
        full.left_top(),
        Pos2::new(full.right(), full.top() + RULER_PX),
    );
    let left = Rect::from_min_max(
        full.left_top(),
        Pos2::new(full.left() + RULER_PX, full.bottom()),
    );
    painter.rect_filled(top, 0.0, bg);
    painter.rect_filled(left, 0.0, bg);

    // Choose a "nice" label step so labels never overlap (~50px apart minimum).
    let step = nice_ruler_step(view.zoom);
    let content = Rect::from_min_max(
        Pos2::new(full.left() + RULER_PX, full.top() + RULER_PX),
        full.right_bottom(),
    );

    // Top ruler ticks (vertical lines + x labels).
    let (dx0, _) = view.screen_to_doc(content.left_top());
    let (dx1, _) = view.screen_to_doc(content.right_bottom());
    let mut i = (dx0 / step).floor() as i64;
    let end_i = (dx1 / step).ceil() as i64;
    while i <= end_i {
        let x = i as f32 * step;
        let sx = view.doc_to_screen((x, 0.0)).x;
        if sx >= content.left() {
            painter.line_segment(
                [
                    Pos2::new(sx, top.bottom() - 5.0),
                    Pos2::new(sx, top.bottom()),
                ],
                Stroke::new(1.0, tick),
            );
            painter.text(
                Pos2::new(sx + 2.0, top.top() + 1.0),
                egui::Align2::LEFT_TOP,
                format!("{}", x as i64),
                font.clone(),
                text_col,
            );
        }
        i += 1;
    }

    // Left ruler ticks (horizontal lines + y labels, rotated read as plain text).
    let (_, dy0) = view.screen_to_doc(content.left_top());
    let (_, dy1) = view.screen_to_doc(content.right_bottom());
    let mut j = (dy0 / step).floor() as i64;
    let end_j = (dy1 / step).ceil() as i64;
    while j <= end_j {
        let y = j as f32 * step;
        let sy = view.doc_to_screen((0.0, y)).y;
        if sy >= content.top() {
            painter.line_segment(
                [
                    Pos2::new(left.right() - 5.0, sy),
                    Pos2::new(left.right(), sy),
                ],
                Stroke::new(1.0, tick),
            );
            painter.text(
                Pos2::new(left.left() + 1.0, sy + 1.0),
                egui::Align2::LEFT_TOP,
                format!("{}", y as i64),
                font.clone(),
                text_col,
            );
        }
        j += 1;
    }

    // Cursor position markers on each ruler.
    if let Some(c) = cursor {
        if content.contains(c) {
            let marker = Stroke::new(1.0, theme::accent());
            painter.line_segment(
                [Pos2::new(c.x, top.top()), Pos2::new(c.x, top.bottom())],
                marker,
            );
            painter.line_segment(
                [Pos2::new(left.left(), c.y), Pos2::new(left.right(), c.y)],
                marker,
            );
        }
    }

    // Corner square.
    painter.rect_filled(
        Rect::from_min_max(
            full.left_top(),
            Pos2::new(full.left() + RULER_PX, full.top() + RULER_PX),
        ),
        0.0,
        Color32::from_rgb(0x2a, 0x2e, 0x34),
    );
}

/// Pick a human-friendly ruler label step (1/2/5 × 10^k document units) so that
/// labels are at least ~50 screen pixels apart at the current zoom.
pub fn nice_ruler_step(zoom: f32) -> f32 {
    let target_px = 50.0;
    let raw = target_px / zoom.max(1e-3); // document units per ~50px
    let mag = 10f32.powf(raw.max(1.0).log10().floor());
    let norm = raw / mag;
    let mult = if norm <= 1.0 {
        1.0
    } else if norm <= 2.0 {
        2.0
    } else if norm <= 5.0 {
        5.0
    } else {
        10.0
    };
    (mult * mag).max(1.0)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ruler_step_is_nice_and_scales_with_zoom() {
        // At 1× zoom, ~50px target ≈ 50 doc units → snaps to 50.
        assert_eq!(nice_ruler_step(1.0), 50.0);
        // Zoomed in (4×), each doc unit is wider, so fewer units per label.
        assert!(nice_ruler_step(4.0) < nice_ruler_step(1.0));
        // Zoomed way out, the step grows.
        assert!(nice_ruler_step(0.1) > nice_ruler_step(1.0));
        // Step is always a 1/2/5 × 10^k value (mantissa in {1,2,5,10}).
        for z in [0.05_f32, 0.3, 1.0, 2.5, 9.0, 30.0] {
            let s = nice_ruler_step(z);
            let mag = 10f32.powf(s.log10().floor());
            let m = (s / mag).round();
            assert!(
                [1.0, 2.0, 5.0, 10.0].contains(&m),
                "step {s} (mantissa {m}) at zoom {z} is not 1/2/5"
            );
        }
    }
}
