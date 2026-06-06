//! Export the document to external formats: SVG (vector) and PNG (raster).
//!
//! Both exporters iterate the document in paint order (bottom-up) and skip
//! hidden shapes. SVG emits standard `rect`/`ellipse`/`line`/`path` elements;
//! PNG rasterizes via `tiny-skia` into a `Pixmap` sized to the artboard.

use crate::document::{self, Document, LineCap, LineJoin, Shape, StrokeStyle};
use tiny_skia::{
    Color as TsColor, FillRule as TsFillRule, LineCap as TsCap, LineJoin as TsJoin, Paint,
    PathBuilder, Pixmap, Rect as TsRect, Stroke, StrokeDash, Transform,
};

// --- SVG ---------------------------------------------------------------------

/// Serialize the whole document to a standalone SVG string sized to the
/// artboard `(w, h)` in document units.
pub fn to_svg(doc: &Document, w: f32, h: f32) -> String {
    let mut s = String::new();
    s.push_str(&format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{w}\" height=\"{h}\" \
         viewBox=\"0 0 {w} {h}\">\n"
    ));
    for shape in &doc.shapes {
        if !shape.visible() {
            continue;
        }
        s.push_str("  ");
        s.push_str(&shape_to_svg(shape));
        s.push('\n');
    }
    s.push_str("</svg>\n");
    s
}

/// `[f32;4]` straight sRGB -> `#rrggbb`.
fn hex(c: [f32; 4]) -> String {
    let b = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
    format!("#{:02x}{:02x}{:02x}", b(c[0]), b(c[1]), b(c[2]))
}

/// Stroke/fill paint attributes shared by all elements, including the stroke
/// style's caps/joins/miter/dashes.
fn paint_attrs(
    fill: Option<[f32; 4]>,
    stroke: [f32; 4],
    stroke_w: f32,
    style: &StrokeStyle,
) -> String {
    let mut a = String::new();
    match fill {
        Some(f) => {
            a.push_str(&format!(" fill=\"{}\"", hex(f)));
            if f[3] < 1.0 {
                a.push_str(&format!(" fill-opacity=\"{:.3}\"", f[3]));
            }
        }
        None => a.push_str(" fill=\"none\""),
    }
    if stroke_w > 0.0 {
        a.push_str(&format!(
            " stroke=\"{}\" stroke-width=\"{}\"",
            hex(stroke),
            stroke_w
        ));
        if stroke[3] < 1.0 {
            a.push_str(&format!(" stroke-opacity=\"{:.3}\"", stroke[3]));
        }
        // Only emit non-default cap/join keywords to keep the SVG compact.
        if style.cap != LineCap::Butt {
            a.push_str(&format!(" stroke-linecap=\"{}\"", style.cap.svg()));
        }
        if style.join != LineJoin::Miter {
            a.push_str(&format!(" stroke-linejoin=\"{}\"", style.join.svg()));
        } else if (style.miter_limit - 4.0).abs() > 1e-3 {
            a.push_str(&format!(" stroke-miterlimit=\"{}\"", style.miter_limit));
        }
        if let Some(runs) = style.normalized_dash() {
            let list = runs
                .iter()
                .map(|v| format!("{v}"))
                .collect::<Vec<_>>()
                .join(",");
            a.push_str(&format!(" stroke-dasharray=\"{list}\""));
            if style.dash_offset != 0.0 {
                a.push_str(&format!(" stroke-dashoffset=\"{}\"", style.dash_offset));
            }
        }
    }
    a
}

fn shape_to_svg(shape: &Shape) -> String {
    let style = shape.stroke_style();
    match shape {
        Shape::Rect {
            rect,
            fill,
            stroke,
            stroke_w,
            ..
        } => {
            // Normalize negative width/height (drags can produce them).
            let (x, y, w, h) = norm_rect(rect);
            format!(
                "<rect x=\"{x}\" y=\"{y}\" width=\"{w}\" height=\"{h}\"{} />",
                paint_attrs(Some(*fill), *stroke, *stroke_w, style)
            )
        }
        Shape::Ellipse {
            rect,
            fill,
            stroke,
            stroke_w,
            ..
        } => {
            let (x, y, w, h) = norm_rect(rect);
            let (cx, cy) = (x + w * 0.5, y + h * 0.5);
            let (rx, ry) = (w * 0.5, h * 0.5);
            format!(
                "<ellipse cx=\"{cx}\" cy=\"{cy}\" rx=\"{rx}\" ry=\"{ry}\"{} />",
                paint_attrs(Some(*fill), *stroke, *stroke_w, style)
            )
        }
        Shape::Line {
            p0,
            p1,
            stroke,
            stroke_w,
            ..
        } => format!(
            "<line x1=\"{}\" y1=\"{}\" x2=\"{}\" y2=\"{}\"{} />",
            p0.0,
            p0.1,
            p1.0,
            p1.1,
            paint_attrs(None, *stroke, *stroke_w, style)
        ),
        Shape::Path {
            points,
            closed,
            fill,
            stroke,
            stroke_w,
            handles,
            ..
        } => {
            let d = path_d(points, handles, *closed);
            // Only closed paths take a fill; open paths are stroke-only.
            let fill = if *closed { Some(*fill) } else { None };
            format!(
                "<path d=\"{d}\"{} />",
                paint_attrs(fill, *stroke, *stroke_w, style)
            )
        }
    }
}

fn norm_rect(rect: &[f32; 4]) -> (f32, f32, f32, f32) {
    let x = rect[0].min(rect[0] + rect[2]);
    let y = rect[1].min(rect[1] + rect[3]);
    (x, y, rect[2].abs(), rect[3].abs())
}

/// Build the SVG `d` attribute for a path, emitting `C` (cubic) commands for
/// curved segments and `L` for straight ones.
fn path_d(points: &[(f32, f32)], handles: &[(f32, f32)], closed: bool) -> String {
    let n = points.len();
    if n == 0 {
        return String::new();
    }
    let mut d = format!("M {} {}", points[0].0, points[0].1);
    let seg_count = if closed { n } else { n - 1 };
    for i in 0..seg_count {
        let a = points[i];
        let b = points[(i + 1) % n];
        let ha = document::handle_at(handles, i);
        let hb = document::handle_at(handles, (i + 1) % n);
        let a_corner = ha.0 == 0.0 && ha.1 == 0.0;
        let b_corner = hb.0 == 0.0 && hb.1 == 0.0;
        if a_corner && b_corner {
            d.push_str(&format!(" L {} {}", b.0, b.1));
        } else {
            let c1 = (a.0 + ha.0, a.1 + ha.1);
            let c2 = (b.0 - hb.0, b.1 - hb.1);
            d.push_str(&format!(
                " C {} {} {} {} {} {}",
                c1.0, c1.1, c2.0, c2.1, b.0, b.1
            ));
        }
    }
    if closed {
        d.push_str(" Z");
    }
    d
}

// --- PNG ---------------------------------------------------------------------

fn ts_color(c: [f32; 4]) -> TsColor {
    TsColor::from_rgba(
        c[0].clamp(0.0, 1.0),
        c[1].clamp(0.0, 1.0),
        c[2].clamp(0.0, 1.0),
        c[3].clamp(0.0, 1.0),
    )
    .unwrap_or(TsColor::BLACK)
}

/// Rasterize the document to PNG bytes at the artboard size `(w, h)` (document
/// units == output pixels). Returns `None` on degenerate sizes / encode error.
pub fn to_png(doc: &Document, w: f32, h: f32) -> Option<Vec<u8>> {
    let pw = w.round().max(1.0) as u32;
    let ph = h.round().max(1.0) as u32;
    let mut pixmap = Pixmap::new(pw, ph)?;
    pixmap.fill(TsColor::WHITE);

    for shape in &doc.shapes {
        if !shape.visible() {
            continue;
        }
        draw_shape_skia(&mut pixmap, shape);
    }

    pixmap.encode_png().ok()
}

fn draw_shape_skia(pixmap: &mut Pixmap, shape: &Shape) {
    let id = Transform::identity();
    let style = shape.stroke_style();
    match shape {
        Shape::Rect {
            rect,
            fill,
            stroke,
            stroke_w,
            ..
        } => {
            let (x, y, w, h) = norm_rect(rect);
            if let Some(r) = TsRect::from_xywh(x, y, w.max(0.01), h.max(0.01)) {
                let path = PathBuilder::from_rect(r);
                fill_then_stroke(pixmap, &path, Some(*fill), *stroke, *stroke_w, style, id);
            }
        }
        Shape::Ellipse {
            rect,
            fill,
            stroke,
            stroke_w,
            ..
        } => {
            let (x, y, w, h) = norm_rect(rect);
            if let Some(r) = TsRect::from_xywh(x, y, w.max(0.01), h.max(0.01)) {
                let mut pb = PathBuilder::new();
                pb.push_oval(r);
                if let Some(path) = pb.finish() {
                    fill_then_stroke(pixmap, &path, Some(*fill), *stroke, *stroke_w, style, id);
                }
            }
        }
        Shape::Line {
            p0,
            p1,
            stroke,
            stroke_w,
            ..
        } => {
            let mut pb = PathBuilder::new();
            pb.move_to(p0.0, p0.1);
            pb.line_to(p1.0, p1.1);
            if let Some(path) = pb.finish() {
                fill_then_stroke(
                    pixmap,
                    &path,
                    None,
                    *stroke,
                    (*stroke_w).max(1.0),
                    style,
                    id,
                );
            }
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
            if let Some(path) = build_skia_path(points, handles, *closed) {
                let fill = if *closed { Some(*fill) } else { None };
                fill_then_stroke(pixmap, &path, fill, *stroke, *stroke_w, style, id);
            }
        }
    }
}

/// Map our document [`LineCap`] to tiny-skia's.
fn ts_cap(cap: LineCap) -> TsCap {
    match cap {
        LineCap::Butt => TsCap::Butt,
        LineCap::Round => TsCap::Round,
        LineCap::Square => TsCap::Square,
    }
}

/// Map our document [`LineJoin`] to tiny-skia's.
fn ts_join(join: LineJoin) -> TsJoin {
    match join {
        LineJoin::Miter => TsJoin::Miter,
        LineJoin::Round => TsJoin::Round,
        LineJoin::Bevel => TsJoin::Bevel,
    }
}

fn build_skia_path(
    points: &[(f32, f32)],
    handles: &[(f32, f32)],
    closed: bool,
) -> Option<tiny_skia::Path> {
    let n = points.len();
    if n < 2 {
        return None;
    }
    let mut pb = PathBuilder::new();
    pb.move_to(points[0].0, points[0].1);
    let seg_count = if closed { n } else { n - 1 };
    for i in 0..seg_count {
        let a = points[i];
        let b = points[(i + 1) % n];
        let ha = document::handle_at(handles, i);
        let hb = document::handle_at(handles, (i + 1) % n);
        let a_corner = ha.0 == 0.0 && ha.1 == 0.0;
        let b_corner = hb.0 == 0.0 && hb.1 == 0.0;
        if a_corner && b_corner {
            pb.line_to(b.0, b.1);
        } else {
            pb.cubic_to(a.0 + ha.0, a.1 + ha.1, b.0 - hb.0, b.1 - hb.1, b.0, b.1);
        }
    }
    if closed {
        pb.close();
    }
    pb.finish()
}

#[allow(clippy::too_many_arguments)]
fn fill_then_stroke(
    pixmap: &mut Pixmap,
    path: &tiny_skia::Path,
    fill: Option<[f32; 4]>,
    stroke: [f32; 4],
    stroke_w: f32,
    style: &StrokeStyle,
    transform: Transform,
) {
    if let Some(f) = fill {
        if f[3] > 0.0 {
            let mut paint = Paint::default();
            paint.set_color(ts_color(f));
            paint.anti_alias = true;
            pixmap.fill_path(path, &paint, TsFillRule::Winding, transform, None);
        }
    }
    if stroke_w > 0.0 && stroke[3] > 0.0 {
        let mut paint = Paint::default();
        paint.set_color(ts_color(stroke));
        paint.anti_alias = true;
        let s = Stroke {
            width: stroke_w,
            miter_limit: style.miter_limit.max(1.0),
            line_cap: ts_cap(style.cap),
            line_join: ts_join(style.join),
            dash: style
                .normalized_dash()
                .and_then(|runs| StrokeDash::new(runs, style.dash_offset)),
        };
        pixmap.stroke_path(path, &paint, &s, transform, None);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::Shape;

    fn sample_doc() -> Document {
        Document {
            shapes: vec![
                Shape::Rect {
                    rect: [10.0, 10.0, 40.0, 30.0],
                    fill: [1.0, 0.0, 0.0, 1.0],
                    stroke: [0.0, 0.0, 0.0, 1.0],
                    stroke_w: 2.0,
                    stroke_style: StrokeStyle::default(),
                    visible: true,
                },
                Shape::Path {
                    points: vec![(60.0, 60.0), (90.0, 60.0), (90.0, 90.0)],
                    closed: true,
                    fill: [0.0, 0.0, 1.0, 1.0],
                    stroke: [0.0, 0.0, 0.0, 1.0],
                    stroke_w: 1.0,
                    stroke_style: StrokeStyle::default(),
                    handles: vec![(10.0, 0.0), (0.0, 0.0), (0.0, 0.0)],
                    visible: true,
                },
            ],
        }
    }

    #[test]
    fn svg_contains_elements_and_curve() {
        let svg = to_svg(&sample_doc(), 200.0, 200.0);
        assert!(svg.contains("<svg"));
        assert!(svg.contains("<rect"));
        assert!(svg.contains("<path"));
        assert!(svg.contains(" C "), "curved segment should emit a cubic");
        assert!(svg.trim_end().ends_with("</svg>"));
    }

    #[test]
    fn svg_skips_hidden_shapes() {
        let mut doc = sample_doc();
        if let Shape::Rect { visible, .. } = &mut doc.shapes[0] {
            *visible = false;
        }
        let svg = to_svg(&doc, 200.0, 200.0);
        assert!(!svg.contains("<rect"));
        assert!(svg.contains("<path"));
    }

    #[test]
    fn png_encodes_nonempty() {
        let bytes = to_png(&sample_doc(), 200.0, 200.0).expect("png should encode");
        assert!(bytes.len() > 8);
        // PNG magic signature.
        assert_eq!(
            &bytes[0..8],
            &[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a]
        );
    }

    fn dashed_rect() -> Document {
        Document {
            shapes: vec![Shape::Rect {
                rect: [10.0, 10.0, 80.0, 60.0],
                fill: [0.0, 0.0, 0.0, 0.0],
                stroke: [0.0, 0.0, 0.0, 1.0],
                stroke_w: 4.0,
                stroke_style: StrokeStyle {
                    cap: LineCap::Round,
                    join: LineJoin::Round,
                    miter_limit: 4.0,
                    dash: vec![12.0, 6.0],
                    dash_offset: 3.0,
                },
                visible: true,
            }],
        }
    }

    #[test]
    fn svg_emits_dash_and_cap_attrs() {
        let svg = to_svg(&dashed_rect(), 200.0, 200.0);
        assert!(svg.contains("stroke-dasharray=\"12,6\""), "svg: {svg}");
        assert!(svg.contains("stroke-dashoffset=\"3\""), "svg: {svg}");
        assert!(svg.contains("stroke-linecap=\"round\""), "svg: {svg}");
        assert!(svg.contains("stroke-linejoin=\"round\""), "svg: {svg}");
    }

    #[test]
    fn svg_omits_default_stroke_attrs() {
        // A solid butt/miter stroke must not emit cap/join/dash attributes.
        let svg = to_svg(&sample_doc(), 200.0, 200.0);
        assert!(!svg.contains("stroke-linecap"));
        assert!(!svg.contains("stroke-linejoin"));
        assert!(!svg.contains("stroke-dasharray"));
    }

    #[test]
    fn png_encodes_dashed_stroke() {
        // Dashed/round-cap stroking must not crash the rasterizer.
        let bytes = to_png(&dashed_rect(), 120.0, 100.0).expect("png should encode");
        assert!(bytes.len() > 8);
    }
}
