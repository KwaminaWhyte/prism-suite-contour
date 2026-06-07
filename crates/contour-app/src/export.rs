//! Export the document to external formats: SVG (vector) and PNG (raster).
//!
//! Both exporters iterate the document in paint order (bottom-up) and skip
//! hidden shapes. SVG emits standard `rect`/`ellipse`/`line`/`path` elements;
//! PNG rasterizes via `tiny-skia` into a `Pixmap` sized to the artboard.

use crate::appearance::{Appearance, Paint};
// NOTE: `Paint` here is the Appearance paint enum; tiny-skia's `Paint` is used
// fully-qualified as `tiny_skia::Paint` in the rasterizer to avoid the clash.
use crate::document::{self, Document, LineCap, LineJoin, Shape, StrokeStyle};
use crate::gradient::{Gradient, GradientKind, SpreadMode};
use tiny_skia::{
    Color as TsColor, FillRule as TsFillRule, GradientStop as TsStop, LineCap as TsCap,
    LineJoin as TsJoin, LinearGradient, Paint as TsPaint, PathBuilder, Pixmap, Point as TsPoint,
    RadialGradient, Rect as TsRect, Shader, SpreadMode as TsSpread, Stroke, StrokeDash, Transform,
};

// --- SVG ---------------------------------------------------------------------

/// Serialize the whole document to a standalone SVG string cropped to one
/// artboard `[ox, oy, w, h]` in document units: the viewBox is `0 0 w h` and the
/// artwork is translated by `(-ox, -oy)` so the chosen artboard's content lands
/// at the SVG origin (matching Illustrator's per-artboard SVG export). Gradient
/// fills are emitted as `<linearGradient>` / `<radialGradient>` defs (one per
/// gradient-filled shape, in user space mapped to the shape's bounding box) and
/// referenced via `fill="url(#…)"`.
pub fn to_svg_artboard(doc: &Document, ab: [f32; 4]) -> String {
    let (ox, oy, w, h) = (ab[0], ab[1], ab[2], ab[3]);
    let body_inner = svg_body(doc);
    let mut s = String::new();
    s.push_str(&format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{w}\" height=\"{h}\" \
         viewBox=\"0 0 {w} {h}\">\n"
    ));
    let translate = ox != 0.0 || oy != 0.0;
    if translate {
        s.push_str(&format!("  <g transform=\"translate({},{})\">\n", -ox, -oy));
    }
    s.push_str(&body_inner);
    if translate {
        s.push_str("  </g>\n");
    }
    s.push_str("</svg>\n");
    s
}

/// Serialize the whole document to a standalone SVG string sized to `(w, h)` in
/// document units, anchored at the document origin (the artboard-at-origin
/// case). A thin wrapper over [`svg_body`] used by the export tests;
/// [`to_svg_artboard`] is the path the editor takes (it adds the crop offset).
#[cfg(test)]
pub fn to_svg(doc: &Document, w: f32, h: f32) -> String {
    let mut s = String::new();
    s.push_str(&format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{w}\" height=\"{h}\" \
         viewBox=\"0 0 {w} {h}\">\n"
    ));
    s.push_str(&svg_body(doc));
    s.push_str("</svg>\n");
    s
}

/// Build the inner SVG body (`<defs>` for gradients + the shape elements) shared
/// by [`to_svg`] and [`to_svg_artboard`]. Does not emit the `<svg>` wrapper.
fn svg_body(doc: &Document) -> String {
    // Build the <defs> for every gradient (one per gradient-painted fill/stroke
    // layer) and the layered shape elements. Clipping masks are resolved before
    // emission: a clip mask path drops out, and clipped content is emitted
    // already cropped to the mask outline (so the SVG matches the canvas without
    // needing `<clipPath>` plumbing).
    //
    // Each shape's *effective* Appearance is walked bottom-to-top: a separate SVG
    // element is emitted for every visible fill (filled, no stroke) then every
    // visible stroke (fill=none), so a stacked object becomes a stack of paint
    // layers and a legacy single-fill/stroke object emits one fill + one stroke.
    let mut defs = String::new();
    let mut body = String::new();
    for (i, shape) in doc.render_shapes() {
        if !shape.visible() {
            continue;
        }
        let bbox = shape.bounds().map(|b| [b.x, b.y, b.w, b.h]).unwrap_or([0.0; 4]);
        let appearance = shape.effective_appearance();
        let geom = svg_geom(&shape);
        let mut grad_n = 0usize;

        // Fills, bottom-to-top (only on a fillable geometry).
        if geom.fillable {
            for fill in &appearance.fills {
                if !fill.visible || fill.opacity <= 0.0 {
                    continue;
                }
                let (fill_attr, grad) = paint_to_svg_fill(&fill.paint, fill.opacity);
                let grad_id = grad.map(|g| {
                    let id = format!("grad{i}_{grad_n}");
                    grad_n += 1;
                    defs.push_str("    ");
                    defs.push_str(&gradient_def(&id, &g, &bbox));
                    defs.push('\n');
                    id
                });
                let attrs = match grad_id {
                    Some(id) => format!(" fill=\"url(#{id})\""),
                    None => fill_attr,
                };
                body.push_str("  ");
                body.push_str(&geom.element(&format!("{attrs} stroke=\"none\"")));
                body.push('\n');
            }
        }
        // Strokes, bottom-to-top.
        for stroke in &appearance.strokes {
            if !stroke.visible || stroke.opacity <= 0.0 || stroke.width <= 0.0 {
                continue;
            }
            let (stroke_attr, grad) = paint_to_svg_stroke(&stroke.paint, stroke.opacity);
            let stroke_paint = match grad {
                Some(g) => {
                    let id = format!("grad{i}_{grad_n}");
                    grad_n += 1;
                    defs.push_str("    ");
                    defs.push_str(&gradient_def(&id, &g, &bbox));
                    defs.push('\n');
                    format!(" stroke=\"url(#{id})\"")
                }
                None => stroke_attr,
            };
            let mut attrs = String::from(" fill=\"none\"");
            attrs.push_str(&stroke_paint);
            attrs.push_str(&stroke_geom_attrs(stroke.width, &stroke.style));
            body.push_str("  ");
            body.push_str(&geom.element(&attrs));
            body.push('\n');
        }
    }

    let mut s = String::new();
    if !defs.is_empty() {
        s.push_str("  <defs>\n");
        s.push_str(&defs);
        s.push_str("  </defs>\n");
    }
    s.push_str(&body);
    s
}

/// An SVG geometry token: emits the shape's element (`<rect>`, `<ellipse>`,
/// `<line>`, or `<path>`) with arbitrary paint attributes spliced in, so the
/// Appearance walker can emit the same outline once per fill / stroke layer.
struct SvgGeom {
    /// The element kind + geometry attributes (everything but paint).
    head: String,
    /// Whether the geometry has a fillable region (closed).
    fillable: bool,
}

impl SvgGeom {
    /// `<tag geom… {paint} />`.
    fn element(&self, paint: &str) -> String {
        format!("{}{} />", self.head, paint)
    }
}

/// Build the geometry token for a shape (no paint).
fn svg_geom(shape: &Shape) -> SvgGeom {
    match shape {
        Shape::Rect { rect, .. } => {
            let (x, y, w, h) = norm_rect(rect);
            SvgGeom {
                head: format!("<rect x=\"{x}\" y=\"{y}\" width=\"{w}\" height=\"{h}\""),
                fillable: true,
            }
        }
        Shape::Ellipse { rect, .. } => {
            let (x, y, w, h) = norm_rect(rect);
            let (cx, cy) = (x + w * 0.5, y + h * 0.5);
            let (rx, ry) = (w * 0.5, h * 0.5);
            SvgGeom {
                head: format!("<ellipse cx=\"{cx}\" cy=\"{cy}\" rx=\"{rx}\" ry=\"{ry}\""),
                fillable: true,
            }
        }
        Shape::Line { p0, p1, .. } => SvgGeom {
            head: format!(
                "<line x1=\"{}\" y1=\"{}\" x2=\"{}\" y2=\"{}\"",
                p0.0, p0.1, p1.0, p1.1
            ),
            fillable: false,
        },
        Shape::Path {
            points,
            handles,
            closed,
            ..
        } => {
            let d = path_d(points, handles, *closed);
            SvgGeom {
                head: format!("<path d=\"{d}\""),
                fillable: *closed,
            }
        }
    }
}

/// SVG `fill` attribute for a paint layer (with opacity folded into the colour
/// alpha / `fill-opacity`). Returns the attribute string and, for a gradient,
/// the (opacity-scaled) gradient so the caller can emit a def.
fn paint_to_svg_fill(paint: &Paint, opacity: f32) -> (String, Option<Gradient>) {
    match paint {
        Paint::Solid(c) => {
            let mut a = format!(" fill=\"{}\"", hex(*c));
            let eff = c[3] * opacity;
            if eff < 1.0 {
                a.push_str(&format!(" fill-opacity=\"{:.3}\"", eff.clamp(0.0, 1.0)));
            }
            (a, None)
        }
        Paint::Gradient(g) => (String::new(), Some(scale_grad(g, opacity))),
    }
}

/// SVG `stroke` colour attribute (sans width/dash) for a stroke layer.
fn paint_to_svg_stroke(paint: &Paint, opacity: f32) -> (String, Option<Gradient>) {
    match paint {
        Paint::Solid(c) => {
            let mut a = format!(" stroke=\"{}\"", hex(*c));
            let eff = c[3] * opacity;
            if eff < 1.0 {
                a.push_str(&format!(" stroke-opacity=\"{:.3}\"", eff.clamp(0.0, 1.0)));
            }
            (a, None)
        }
        Paint::Gradient(g) => (String::new(), Some(scale_grad(g, opacity))),
    }
}

/// Width + caps/joins/dashes attributes for a stroke layer (mirrors the legacy
/// [`paint_attrs`] stroke half, minus the colour).
fn stroke_geom_attrs(width: f32, style: &StrokeStyle) -> String {
    let mut a = format!(" stroke-width=\"{width}\"");
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
    a
}

/// Clone a gradient with every stop alpha scaled by `opacity`.
fn scale_grad(g: &Gradient, opacity: f32) -> Gradient {
    let mut g = g.clone();
    for s in g.stops.iter_mut() {
        s.color[3] = (s.color[3] * opacity).clamp(0.0, 1.0);
    }
    g
}

/// Emit a `<linearGradient>` / `<radialGradient>` def for `g` mapped onto the
/// bounding box `bbox`, in user-space coordinates so the geometry is exact.
fn gradient_def(id: &str, g: &Gradient, bbox: &[f32; 4]) -> String {
    let stops: String = g
        .sorted_stops()
        .iter()
        .map(|st| {
            let mut s = format!(
                "<stop offset=\"{:.4}\" stop-color=\"{}\"",
                st.offset,
                hex(st.color)
            );
            if st.color[3] < 1.0 {
                s.push_str(&format!(" stop-opacity=\"{:.3}\"", st.color[3]));
            }
            s.push_str(" />");
            s
        })
        .collect::<Vec<_>>()
        .join("");
    let spread = if g.spread == SpreadMode::Pad {
        String::new()
    } else {
        format!(" spreadMethod=\"{}\"", g.spread.svg())
    };
    match g.kind {
        GradientKind::Linear => {
            let (a, b) = crate::gradient::linear_endpoints(bbox, g.angle);
            format!(
                "<linearGradient id=\"{id}\" gradientUnits=\"userSpaceOnUse\" \
                 x1=\"{}\" y1=\"{}\" x2=\"{}\" y2=\"{}\"{spread}>{stops}</linearGradient>",
                a.0, a.1, b.0, b.1
            )
        }
        GradientKind::Radial => {
            let ((cx, cy), r) = crate::gradient::radial_params(bbox);
            format!(
                "<radialGradient id=\"{id}\" gradientUnits=\"userSpaceOnUse\" \
                 cx=\"{cx}\" cy=\"{cy}\" r=\"{r}\"{spread}>{stops}</radialGradient>"
            )
        }
    }
}

/// `[f32;4]` straight sRGB -> `#rrggbb`.
fn hex(c: [f32; 4]) -> String {
    let b = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
    format!("#{:02x}{:02x}{:02x}", b(c[0]), b(c[1]), b(c[2]))
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

/// Rasterize the document to PNG bytes at size `(w, h)` (document units ==
/// output pixels), anchored at the document origin. A thin wrapper over
/// [`to_png_artboard`] used by the export tests; the editor calls
/// [`to_png_artboard`] directly with the active artboard's rectangle.
#[cfg(test)]
pub fn to_png(doc: &Document, w: f32, h: f32) -> Option<Vec<u8>> {
    to_png_artboard(doc, [0.0, 0.0, w, h])
}

/// Rasterize the document cropped to one artboard `[ox, oy, w, h]` (document
/// units == output pixels): the canvas is `w × h` and the artwork is translated
/// by `(-ox, -oy)`, so the chosen artboard's content fills the image. Returns
/// `None` on degenerate sizes / encode error.
pub fn to_png_artboard(doc: &Document, ab: [f32; 4]) -> Option<Vec<u8>> {
    let (ox, oy, w, h) = (ab[0], ab[1], ab[2], ab[3]);
    let pw = w.round().max(1.0) as u32;
    let ph = h.round().max(1.0) as u32;
    let mut pixmap = Pixmap::new(pw, ph)?;
    pixmap.fill(TsColor::WHITE);

    let base = Transform::from_translate(-ox, -oy);
    // Clipping masks resolved: mask paths drop out, clipped content is cropped.
    for (_, shape) in doc.render_shapes() {
        if !shape.visible() {
            continue;
        }
        draw_shape_skia(&mut pixmap, &shape, base);
    }

    pixmap.encode_png().ok()
}

fn draw_shape_skia(pixmap: &mut Pixmap, shape: &Shape, id: Transform) {
    // Gradient geometry maps onto the shape's document-space bounding box.
    let bbox = shape
        .bounds()
        .map(|b| [b.x, b.y, b.w, b.h])
        .unwrap_or([0.0; 4]);
    // Build the shape's tiny-skia path once + whether it has a fillable region,
    // then walk its effective Appearance stack over that path.
    let (path, fillable) = match shape {
        Shape::Rect { rect, .. } => {
            let (x, y, w, h) = norm_rect(rect);
            match TsRect::from_xywh(x, y, w.max(0.01), h.max(0.01)) {
                Some(r) => (Some(PathBuilder::from_rect(r)), true),
                None => (None, true),
            }
        }
        Shape::Ellipse { rect, .. } => {
            let (x, y, w, h) = norm_rect(rect);
            let path = TsRect::from_xywh(x, y, w.max(0.01), h.max(0.01)).and_then(|r| {
                let mut pb = PathBuilder::new();
                pb.push_oval(r);
                pb.finish()
            });
            (path, true)
        }
        Shape::Line { p0, p1, .. } => {
            let mut pb = PathBuilder::new();
            pb.move_to(p0.0, p0.1);
            pb.line_to(p1.0, p1.1);
            (pb.finish(), false)
        }
        Shape::Path {
            points,
            closed,
            handles,
            ..
        } => (build_skia_path(points, handles, *closed), *closed),
    };
    let Some(path) = path else {
        return;
    };
    let appearance = shape.effective_appearance();
    paint_appearance_skia(pixmap, &path, fillable, &bbox, &appearance, id);
}

/// Rasterize an [`Appearance`] stack onto `path`: fills bottom-to-top (only when
/// `fillable`), then strokes bottom-to-top, each scaled by its per-item opacity.
/// Only [`BlendMode::Normal`] is composited (tiny-skia source-over); other modes
/// rasterize as Normal.
fn paint_appearance_skia(
    pixmap: &mut Pixmap,
    path: &tiny_skia::Path,
    fillable: bool,
    bbox: &[f32; 4],
    appearance: &Appearance,
    transform: Transform,
) {
    if fillable {
        for fill in &appearance.fills {
            if !fill.visible || fill.opacity <= 0.0 {
                continue;
            }
            let mut paint = TsPaint::default();
            match &fill.paint {
                Paint::Solid(c) => {
                    let c = scale_alpha(*c, fill.opacity);
                    if c[3] <= 0.0 {
                        continue;
                    }
                    paint.set_color(ts_color(c));
                }
                Paint::Gradient(g) => {
                    let g = scale_grad(g, fill.opacity);
                    match gradient_shader(&g, bbox) {
                        Some(s) => paint.shader = s,
                        None => continue,
                    }
                }
            }
            paint.anti_alias = true;
            pixmap.fill_path(path, &paint, TsFillRule::Winding, transform, None);
        }
    }
    for stroke in &appearance.strokes {
        if !stroke.visible || stroke.opacity <= 0.0 || stroke.width <= 0.0 {
            continue;
        }
        let mut paint = TsPaint::default();
        match &stroke.paint {
            Paint::Solid(c) => {
                let c = scale_alpha(*c, stroke.opacity);
                if c[3] <= 0.0 {
                    continue;
                }
                paint.set_color(ts_color(c));
            }
            Paint::Gradient(g) => {
                let g = scale_grad(g, stroke.opacity);
                match gradient_shader(&g, bbox) {
                    Some(s) => paint.shader = s,
                    None => continue,
                }
            }
        }
        paint.anti_alias = true;
        let s = Stroke {
            width: stroke.width.max(0.01),
            miter_limit: stroke.style.miter_limit.max(1.0),
            line_cap: ts_cap(stroke.style.cap),
            line_join: ts_join(stroke.style.join),
            dash: stroke
                .style
                .normalized_dash()
                .and_then(|runs| StrokeDash::new(runs, stroke.style.dash_offset)),
        };
        pixmap.stroke_path(path, &paint, &s, transform, None);
    }
}

/// Multiply a straight-sRGB RGBA colour's alpha by `opacity`.
fn scale_alpha(mut c: [f32; 4], opacity: f32) -> [f32; 4] {
    c[3] = (c[3] * opacity).clamp(0.0, 1.0);
    c
}

/// Map our gradient [`SpreadMode`] to tiny-skia's.
fn ts_spread(mode: SpreadMode) -> TsSpread {
    match mode {
        SpreadMode::Pad => TsSpread::Pad,
        SpreadMode::Repeat => TsSpread::Repeat,
        SpreadMode::Reflect => TsSpread::Reflect,
    }
}

/// Build a tiny-skia gradient [`Shader`] for `g` over the bounding box `bbox`.
/// Returns `None` if the gradient is degenerate (tiny-skia falls back to the
/// solid fill in that case).
fn gradient_shader(g: &Gradient, bbox: &[f32; 4]) -> Option<Shader<'static>> {
    let stops: Vec<TsStop> = g
        .sorted_stops()
        .iter()
        .map(|s| TsStop::new(s.offset, ts_color(s.color)))
        .collect();
    if stops.is_empty() {
        return None;
    }
    let mode = ts_spread(g.spread);
    match g.kind {
        GradientKind::Linear => {
            let (a, b) = crate::gradient::linear_endpoints(bbox, g.angle);
            LinearGradient::new(
                TsPoint::from_xy(a.0, a.1),
                TsPoint::from_xy(b.0, b.1),
                stops,
                mode,
                Transform::identity(),
            )
        }
        GradientKind::Radial => {
            let ((cx, cy), r) = crate::gradient::radial_params(bbox);
            RadialGradient::new(
                TsPoint::from_xy(cx, cy),
                TsPoint::from_xy(cx, cy),
                r,
                stops,
                mode,
                Transform::identity(),
            )
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
                    fill_gradient: None,
                    stroke: [0.0, 0.0, 0.0, 1.0],
                    stroke_w: 2.0,
                    stroke_style: StrokeStyle::default(),
                    appearance: None,
                    visible: true,
                    group: None,
                    clip: None,
                    mask: false,
                },
                Shape::Path {
                    points: vec![(60.0, 60.0), (90.0, 60.0), (90.0, 90.0)],
                    closed: true,
                    fill: [0.0, 0.0, 1.0, 1.0],
                    fill_gradient: None,
                    stroke: [0.0, 0.0, 0.0, 1.0],
                    stroke_w: 1.0,
                    stroke_style: StrokeStyle::default(),
                    appearance: None,
                    handles: vec![(10.0, 0.0), (0.0, 0.0), (0.0, 0.0)],
                    visible: true,
                    group: None,
                    clip: None,
                    mask: false,
                },
            ],
            ..Default::default()
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

    /// A non-origin artboard crop offsets the SVG body with a `translate(...)`
    /// group and sizes the viewBox to the artboard, not the document origin.
    #[test]
    fn svg_artboard_crop_offsets_body() {
        let svg = to_svg_artboard(&sample_doc(), [120.0, 40.0, 200.0, 150.0]);
        assert!(
            svg.contains("viewBox=\"0 0 200 150\""),
            "viewBox sized to the artboard: {svg}"
        );
        assert!(
            svg.contains("translate(-120,-40)"),
            "artwork translated to the artboard origin: {svg}"
        );
        // The origin case adds no translate group.
        let at_origin = to_svg_artboard(&sample_doc(), [0.0, 0.0, 200.0, 150.0]);
        assert!(!at_origin.contains("translate"), "no offset at origin");
    }

    /// A cropped PNG still encodes to a valid image at the artboard pixel size.
    #[test]
    fn png_artboard_crop_encodes() {
        let bytes =
            to_png_artboard(&sample_doc(), [50.0, 25.0, 64.0, 48.0]).expect("png should encode");
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
                fill_gradient: None,
                stroke: [0.0, 0.0, 0.0, 1.0],
                stroke_w: 4.0,
                stroke_style: StrokeStyle {
                    cap: LineCap::Round,
                    join: LineJoin::Round,
                    miter_limit: 4.0,
                    dash: vec![12.0, 6.0],
                    dash_offset: 3.0,
                },
                appearance: None,
                visible: true,
                group: None,
                clip: None,
                mask: false,
            }],
            ..Default::default()
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

    fn gradient_doc(kind: GradientKind) -> Document {
        Document {
            shapes: vec![Shape::Rect {
                rect: [0.0, 0.0, 100.0, 100.0],
                fill: [0.5, 0.5, 0.5, 1.0],
                fill_gradient: Some(Gradient::two_stop(
                    kind,
                    [1.0, 0.0, 0.0, 1.0],
                    [0.0, 0.0, 1.0, 1.0],
                )),
                stroke: [0.0, 0.0, 0.0, 0.0],
                stroke_w: 0.0,
                stroke_style: StrokeStyle::default(),
                appearance: None,
                visible: true,
                group: None,
                clip: None,
                mask: false,
            }],
            ..Default::default()
        }
    }

    #[test]
    fn svg_emits_linear_gradient_def_and_ref() {
        let svg = to_svg(&gradient_doc(GradientKind::Linear), 100.0, 100.0);
        assert!(svg.contains("<defs>"), "svg: {svg}");
        // Gradient defs are now named per-layer: grad{shape}_{layer}.
        assert!(svg.contains("<linearGradient id=\"grad0_0\""), "svg: {svg}");
        assert!(
            svg.contains("gradientUnits=\"userSpaceOnUse\""),
            "svg: {svg}"
        );
        assert!(svg.contains("<stop offset="), "svg: {svg}");
        // The shape's fill layer references the def rather than a solid colour.
        assert!(svg.contains("fill=\"url(#grad0_0)\""), "svg: {svg}");
        assert!(
            !svg.contains("fill=\"#808080\""),
            "svg should not use solid"
        );
    }

    #[test]
    fn svg_emits_radial_gradient_def() {
        let svg = to_svg(&gradient_doc(GradientKind::Radial), 100.0, 100.0);
        assert!(svg.contains("<radialGradient id=\"grad0_0\""), "svg: {svg}");
        assert!(svg.contains("fill=\"url(#grad0_0)\""), "svg: {svg}");
    }

    #[test]
    fn png_renders_gradient_fill() {
        // A linear red→blue gradient should leave the left edge reddish and the
        // right edge bluish in the rasterized output.
        let doc = gradient_doc(GradientKind::Linear);
        let bytes = to_png(&doc, 100.0, 100.0).expect("png should encode");
        assert!(bytes.len() > 8);

        // Re-rasterize to a pixmap directly so we can sample pixels.
        let mut pixmap = Pixmap::new(100, 100).unwrap();
        pixmap.fill(TsColor::WHITE);
        draw_shape_skia(&mut pixmap, &doc.shapes[0], Transform::identity());
        let px = |x: u32, y: u32| {
            let p = pixmap.pixel(x, y).unwrap();
            (p.red(), p.green(), p.blue())
        };
        let (lr, _, lb) = px(2, 50);
        let (rr, _, rb) = px(97, 50);
        // Left is more red than blue; right is more blue than red.
        assert!(lr > lb, "left should be reddish: {lr},{lb}");
        assert!(rb > rr, "right should be bluish: {rr},{rb}");
    }

    /// A shape with two stacked fills + two strokes emits a paint layer per item
    /// in the SVG (bottom-to-top), so the stack survives export.
    #[test]
    fn svg_emits_stacked_paint_layers() {
        use crate::appearance::{Appearance, Fill, Stroke as AppStroke};
        let mut s = Shape::Rect {
            rect: [0.0, 0.0, 50.0, 50.0],
            fill: [1.0, 0.0, 0.0, 1.0],
            fill_gradient: None,
            stroke: [0.0, 0.0, 0.0, 1.0],
            stroke_w: 1.0,
            stroke_style: StrokeStyle::default(),
            appearance: None,
            visible: true,
            group: None,
            clip: None,
            mask: false,
        };
        s.set_appearance(Some(Appearance {
            fills: vec![
                Fill::solid([1.0, 0.0, 0.0, 1.0]),
                Fill::solid([0.0, 1.0, 0.0, 1.0]),
            ],
            strokes: vec![
                AppStroke::solid([0.0, 0.0, 1.0, 1.0], 2.0),
                AppStroke::solid([1.0, 1.0, 1.0, 1.0], 6.0),
            ],
        }));
        let doc = Document {
            shapes: vec![s],
            ..Default::default()
        };
        let svg = to_svg(&doc, 100.0, 100.0);
        // Two fill colours + two stroke colours present as separate elements.
        assert!(svg.matches("<rect").count() == 4, "4 paint layers: {svg}");
        assert!(svg.contains("fill=\"#ff0000\""), "bottom fill: {svg}");
        assert!(svg.contains("fill=\"#00ff00\""), "top fill: {svg}");
        assert!(svg.contains("stroke=\"#0000ff\""), "bottom stroke: {svg}");
        assert!(svg.contains("stroke=\"#ffffff\""), "top stroke: {svg}");
    }

    /// A stacked PNG paints the top fill over the bottom one (last-on-top), so the
    /// centre samples the topmost opaque fill's colour.
    #[test]
    fn png_renders_top_of_fill_stack() {
        use crate::appearance::{Appearance, Fill};
        let mut s = Shape::Rect {
            rect: [0.0, 0.0, 100.0, 100.0],
            fill: [1.0, 0.0, 0.0, 1.0],
            fill_gradient: None,
            stroke: [0.0, 0.0, 0.0, 0.0],
            stroke_w: 0.0,
            stroke_style: StrokeStyle::default(),
            appearance: None,
            visible: true,
            group: None,
            clip: None,
            mask: false,
        };
        // Bottom red, top opaque green → centre reads green.
        s.set_appearance(Some(Appearance {
            fills: vec![
                Fill::solid([1.0, 0.0, 0.0, 1.0]),
                Fill::solid([0.0, 1.0, 0.0, 1.0]),
            ],
            strokes: vec![],
        }));
        let mut pixmap = Pixmap::new(100, 100).unwrap();
        pixmap.fill(TsColor::WHITE);
        draw_shape_skia(&mut pixmap, &s, Transform::identity());
        let p = pixmap.pixel(50, 50).unwrap();
        assert!(
            p.green() > p.red() && p.green() > p.blue(),
            "top green fill wins: {},{},{}",
            p.red(),
            p.green(),
            p.blue()
        );
    }
}
