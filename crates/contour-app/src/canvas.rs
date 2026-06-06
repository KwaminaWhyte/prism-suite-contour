//! The drawing surface: pan/zoom transform, per-frame shape painting, and tool
//! interaction (create / select / move / pen).

use crate::document::Shape;
use crate::theme;
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
    match shape {
        Shape::Rect {
            rect,
            fill,
            stroke,
            stroke_w,
        } => {
            let r = doc_rect(view, rect);
            painter.rect_filled(r, 0.0, to_color32(*fill));
            if *stroke_w > 0.0 {
                painter.rect_stroke(
                    r,
                    0.0,
                    Stroke::new(stroke_w * view.zoom, to_color32(*stroke)),
                    egui::StrokeKind::Middle,
                );
            }
        }
        Shape::Ellipse {
            rect,
            fill,
            stroke,
            stroke_w,
        } => {
            let pts = ellipse_points(view, rect, 48);
            painter.add(egui::Shape::convex_polygon(
                pts.clone(),
                to_color32(*fill),
                if *stroke_w > 0.0 {
                    Stroke::new(stroke_w * view.zoom, to_color32(*stroke))
                } else {
                    Stroke::NONE
                },
            ));
        }
        Shape::Line {
            p0,
            p1,
            stroke,
            stroke_w,
        } => {
            painter.line_segment(
                [view.doc_to_screen(*p0), view.doc_to_screen(*p1)],
                Stroke::new(stroke_w.max(0.5) * view.zoom, to_color32(*stroke)),
            );
        }
        Shape::Path {
            points,
            closed,
            fill,
            stroke,
            stroke_w,
        } => {
            let pts: Vec<Pos2> = points.iter().map(|&p| view.doc_to_screen(p)).collect();
            if pts.len() >= 2 {
                let stroke32 = Stroke::new(stroke_w.max(0.5) * view.zoom, to_color32(*stroke));
                if *closed {
                    painter.add(egui::Shape::convex_polygon(
                        pts.clone(),
                        to_color32(*fill),
                        stroke32,
                    ));
                } else {
                    painter.add(egui::Shape::line(pts.clone(), stroke32));
                }
            }
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
