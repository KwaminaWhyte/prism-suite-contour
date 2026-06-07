//! Export the document to external formats: SVG (vector) and PNG (raster).
//!
//! Both exporters iterate the document in paint order (bottom-up) and skip
//! hidden shapes. SVG emits standard `rect`/`ellipse`/`line`/`path` elements;
//! PNG rasterizes via `tiny-skia` into a `Pixmap` sized to the artboard.

use crate::appearance::{Appearance, BlendMode, Effect, Paint};
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
        // Accumulate this shape's paint elements separately so an effect filter
        // can wrap the whole stack in one `<g filter="url(#…)">`.
        let mut shape_body = String::new();

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
                let mut attrs = match grad_id {
                    Some(id) => format!(" fill=\"url(#{id})\""),
                    None => fill_attr,
                };
                attrs.push_str(" stroke=\"none\"");
                attrs.push_str(&blend_style_attr(fill.blend));
                shape_body.push_str("  ");
                shape_body.push_str(&geom.element(&attrs));
                shape_body.push('\n');
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
            attrs.push_str(&blend_style_attr(stroke.blend));
            shape_body.push_str("  ");
            shape_body.push_str(&geom.element(&attrs));
            shape_body.push('\n');
        }

        // Opacity mask: emit a luminance `<mask>` def (the mask shape painted in
        // greyscale; white reveals, black hides) and reference it on the content's
        // group, so the SVG masks natively. Inverted masks add an `invert` filter.
        let mask_attr = doc.opacity_mask_of(i).map(|(mask_shape, invert)| {
            let mid = format!("om{i}");
            defs.push_str("    ");
            defs.push_str(&opacity_mask_def(&mid, &mask_shape, invert));
            defs.push('\n');
            format!(" mask=\"url(#{mid})\"")
        });

        // Live effects: emit a standard SVG `<filter>` (feGaussianBlur /
        // feDropShadow) and wrap the shape's paint stack in a group referencing
        // it, so the exported SVG renders the effect natively in any viewer.
        let filter_attr = if appearance.has_active_effects() {
            let fid = format!("fx{i}");
            defs.push_str("    ");
            defs.push_str(&effect_filter_def(&fid, &appearance.effects));
            defs.push('\n');
            Some(format!(" filter=\"url(#{fid})\""))
        } else {
            None
        };

        if filter_attr.is_some() || mask_attr.is_some() {
            // One group carries both the filter and the mask (mask outside the
            // filter so the effect spill is masked too, matching the raster path).
            let attrs = format!(
                "{}{}",
                filter_attr.unwrap_or_default(),
                mask_attr.unwrap_or_default()
            );
            body.push_str(&format!("  <g{attrs}>\n"));
            body.push_str(&shape_body);
            body.push_str("  </g>\n");
        } else {
            body.push_str(&shape_body);
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

/// Emit a standard SVG `<filter>` for a live-effect stack, chaining one filter
/// primitive per active [`Effect`] (bottom-to-top, matching the canvas / PNG
/// order). Drop Shadow → `feDropShadow`; Gaussian Blur → `feGaussianBlur`. Each
/// primitive consumes the previous one's `result`, and the filter region is
/// widened (`x/y/width/height`) so soft edges aren't clipped. SVG's Gaussian
/// `stdDeviation` ≈ our box-blur radius, so the visual reach matches closely.
fn effect_filter_def(id: &str, effects: &[Effect]) -> String {
    let active: Vec<&Effect> = effects.iter().filter(|e| e.is_active()).collect();
    // Region margin large enough for the widest effect's spill (as a fraction of
    // the filtered object's bbox — SVG filter regions are in objectBoundingBox
    // units by default, so a flat 50% padding is a safe generous default).
    let mut prims = String::new();
    let mut prev: Option<String> = None;
    for (n, e) in active.iter().enumerate() {
        let result = format!("e{n}");
        let in_attr = match &prev {
            Some(p) => format!(" in=\"{p}\""),
            None => " in=\"SourceGraphic\"".to_string(),
        };
        match e {
            Effect::GaussianBlur { radius } => {
                prims.push_str(&format!(
                    "<feGaussianBlur{in_attr} stdDeviation=\"{radius}\" result=\"{result}\" />"
                ));
            }
            Effect::DropShadow {
                dx,
                dy,
                blur,
                color,
                opacity,
            } => {
                let flood = (color[3] * opacity).clamp(0.0, 1.0);
                prims.push_str(&format!(
                    "<feDropShadow{in_attr} dx=\"{dx}\" dy=\"{dy}\" stdDeviation=\"{blur}\" \
                     flood-color=\"{}\" flood-opacity=\"{:.3}\" result=\"{result}\" />",
                    hex(*color),
                    flood
                ));
            }
        }
        prev = Some(result);
    }
    format!(
        "<filter id=\"{id}\" x=\"-50%\" y=\"-50%\" width=\"200%\" height=\"200%\">{prims}</filter>"
    )
}

/// Emit a luminance `<mask>` def for an opacity mask: the mask shape is painted
/// (its effective fill swatch as the greyscale luminance source) inside a
/// `mask-type="luminance"` element, so any SVG viewer multiplies the masked
/// content's alpha by the mask's luminance — white reveals, black hides. An
/// inverted mask wraps the painted geometry so `1 − luminance` drives the alpha
/// (achieved by flooding the mask region white and subtracting the shape).
fn opacity_mask_def(id: &str, mask_shape: &Shape, invert: bool) -> String {
    let geom = svg_geom(mask_shape);
    let ap = mask_shape.effective_appearance();
    // Representative greyscale colour: the top visible fill's swatch (or white so
    // a stroke-only mask still reveals where it paints).
    let swatch = ap
        .fills
        .iter()
        .rev()
        .find(|f| f.visible && f.opacity > 0.0)
        .map(|f| f.paint.swatch())
        .unwrap_or([1.0, 1.0, 1.0, 1.0]);
    let body = if invert {
        // White backdrop minus the mask shape painted black → inverted luminance.
        format!(
            "<rect x=\"-100%\" y=\"-100%\" width=\"300%\" height=\"300%\" fill=\"#ffffff\" />\
             {}",
            geom.element(" fill=\"#000000\" stroke=\"none\"")
        )
    } else {
        geom.element(&format!(" fill=\"{}\" stroke=\"none\"", hex(swatch)))
    };
    format!("<mask id=\"{id}\" mask-type=\"luminance\">{body}</mask>")
}

/// A `style="mix-blend-mode:…"` attribute for a non-`Normal` paint layer (empty
/// for `Normal`), so an exported fill / stroke composites in any SVG viewer the
/// same way the canvas and PNG do. `Normal` emits nothing (the default).
fn blend_style_attr(blend: BlendMode) -> String {
    if blend.is_separable_blend() {
        format!(" style=\"mix-blend-mode:{}\"", blend.css())
    } else {
        String::new()
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
    // Opacity masks resolved: the mask path drops out and its luminance is applied
    // to its content shape's alpha (via `render_shapes` / `opacity_mask_of`).
    for (i, shape) in doc.render_shapes() {
        if !shape.visible() {
            continue;
        }
        let mask = doc.opacity_mask_of(i);
        draw_shape_skia(&mut pixmap, &shape, base, mask.as_ref());
    }

    pixmap.encode_png().ok()
}

/// Build a shape's tiny-skia [`Path`](tiny_skia::Path) (document space) and
/// whether it has a fillable region. `None` for a degenerate shape. Shared with
/// the live canvas so its effect raster matches the PNG exporter exactly.
pub(crate) fn skia_path_of(shape: &Shape) -> Option<(tiny_skia::Path, bool)> {
    match shape {
        Shape::Rect { rect, .. } => {
            let (x, y, w, h) = norm_rect(rect);
            TsRect::from_xywh(x, y, w.max(0.01), h.max(0.01))
                .map(|r| (PathBuilder::from_rect(r), true))
        }
        Shape::Ellipse { rect, .. } => {
            let (x, y, w, h) = norm_rect(rect);
            TsRect::from_xywh(x, y, w.max(0.01), h.max(0.01))
                .and_then(|r| {
                    let mut pb = PathBuilder::new();
                    pb.push_oval(r);
                    pb.finish()
                })
                .map(|p| (p, true))
        }
        Shape::Line { p0, p1, .. } => {
            let mut pb = PathBuilder::new();
            pb.move_to(p0.0, p0.1);
            pb.line_to(p1.0, p1.1);
            pb.finish().map(|p| (p, false))
        }
        Shape::Path {
            points,
            closed,
            handles,
            ..
        } => build_skia_path(points, handles, *closed).map(|p| (p, *closed)),
    }
}

fn draw_shape_skia(pixmap: &mut Pixmap, shape: &Shape, id: Transform, omask: Option<&(Shape, bool)>) {
    // Gradient geometry maps onto the shape's document-space bounding box.
    let bbox = shape
        .bounds()
        .map(|b| [b.x, b.y, b.w, b.h])
        .unwrap_or([0.0; 4]);
    let Some((path, fillable)) = skia_path_of(shape) else {
        return;
    };
    let appearance = shape.effective_appearance();
    let mask = omask.and_then(OpacityMaskInput::of);

    // Fast path: no live effects, no opacity mask → paint the stack straight onto
    // the page (blend layers still composite, handled inside paint_appearance_skia).
    if !appearance.has_active_effects() && mask.is_none() {
        paint_appearance_skia(pixmap, &path, fillable, &bbox, &appearance, id);
        return;
    }

    // Effects and/or an opacity mask present: rasterize the fill/stroke stack into
    // a padded scratch pixmap (at the page's pixel scale, here 1 px/doc-unit
    // because the page `id` transform is a pure translate), apply the effect stack
    // and the mask, then draw the processed raster back onto the page at the right
    // offset. `id` is a pure `translate(-ox, -oy)` so its translation gives the
    // artboard crop offset.
    if let Some(layer) =
        render_shape_layer_masked(&path, fillable, &bbox, &appearance, 1.0, mask.as_ref())
    {
        let tx = id.tx; // = -ox (artboard crop)
        let ty = id.ty;
        let dst_x = (layer.doc_origin.0 + tx).round() as i32;
        let dst_y = (layer.doc_origin.1 + ty).round() as i32;
        pixmap.draw_pixmap(
            dst_x,
            dst_y,
            layer.pixmap.as_ref(),
            &tiny_skia::PixmapPaint::default(),
            Transform::identity(),
            None,
        );
    }
}

/// A rasterized shape layer + where to place it: the processed `pixmap` and the
/// **document-space** coordinate of its top-left pixel (`doc_origin`). Callers
/// map `doc_origin` to their own surface (page pixels for PNG, screen pixels for
/// the canvas) at the same `scale` they passed in.
pub(crate) struct ShapeLayer {
    pub pixmap: Pixmap,
    pub doc_origin: (f32, f32),
}

/// A resolved opacity-mask input for the rasterizer: the mask shape's tiny-skia
/// `path`, whether it has a fillable region, its document-space `bbox` (for any
/// gradient), its effective `appearance` (the luminance source), and whether the
/// mask is inverted. The mask is rasterized into the same scratch as the artwork
/// and multiplied into its alpha by luminance. Owns its appearance so callers can
/// build it from a transient [`Shape::effective_appearance`].
pub(crate) struct OpacityMaskInput {
    pub path: tiny_skia::Path,
    pub fillable: bool,
    pub bbox: [f32; 4],
    pub appearance: Appearance,
    pub invert: bool,
}

impl OpacityMaskInput {
    /// Build the mask input for a resolved `(mask_shape, invert)` pair, or `None`
    /// if the mask shape is degenerate. Shared by PNG export and the canvas.
    pub(crate) fn of(mask: &(Shape, bool)) -> Option<Self> {
        let (mask_shape, invert) = mask;
        let (path, fillable) = skia_path_of(mask_shape)?;
        let bbox = mask_shape
            .bounds()
            .map(|b| [b.x, b.y, b.w, b.h])
            .unwrap_or([0.0; 4]);
        Some(Self {
            path,
            fillable,
            bbox,
            appearance: mask_shape.effective_appearance(),
            invert: *invert,
        })
    }
}

/// Rasterize a shape's effective appearance (fills + strokes) into a padded
/// scratch pixmap at `scale` px/doc-unit, then apply its live effect stack.
/// A thin no-mask wrapper over [`render_shape_layer_masked`], retained for the
/// export tests.
#[cfg(test)]
pub(crate) fn render_shape_layer(
    path: &tiny_skia::Path,
    fillable: bool,
    bbox: &[f32; 4],
    appearance: &Appearance,
    scale: f32,
) -> Option<ShapeLayer> {
    render_shape_layer_masked(path, fillable, bbox, appearance, scale, None)
}

/// Rasterize a shape's effective appearance into a padded scratch pixmap, then
/// apply its live effect stack and, last, any opacity mask. Returns the processed
/// layer + its document-space placement, or `None` for a degenerate size. Shared
/// by PNG export and the live canvas so the two surfaces composite effects,
/// blends and masks identically.
pub(crate) fn render_shape_layer_masked(
    path: &tiny_skia::Path,
    fillable: bool,
    bbox: &[f32; 4],
    appearance: &Appearance,
    scale: f32,
    mask: Option<&OpacityMaskInput>,
) -> Option<ShapeLayer> {
    let pad = appearance.effect_pad();
    // Padded document-space rect covering the artwork + the effects' spill.
    let dx = bbox[0] - pad;
    let dy = bbox[1] - pad;
    let dw = bbox[2] + 2.0 * pad;
    let dh = bbox[3] + 2.0 * pad;
    let pw = (dw * scale).ceil().max(1.0) as u32;
    let ph = (dh * scale).ceil().max(1.0) as u32;
    // Guard against absurd allocations (e.g. a pathological zoom).
    if pw > 8192 || ph > 8192 {
        return None;
    }
    let mut layer = crate::effects::transparent_pixmap(pw, ph);
    // Map document space into the scratch pixmap: translate the padded origin to
    // (0,0), then scale to pixels.
    let t = Transform::from_scale(scale, scale).post_translate(-dx * scale, -dy * scale);
    paint_appearance_skia(&mut layer, path, fillable, bbox, appearance, t);
    crate::effects::apply_effects(&mut layer, &appearance.effects, scale);
    // Opacity mask: rasterize the mask shape's luminance into a same-size scratch
    // (same transform, so it registers pixel-for-pixel with the artwork), then
    // multiply it into the artwork's alpha. Applied last so it masks the final
    // composited result (artwork + effects), as Illustrator does.
    if let Some(m) = mask {
        let mut mask_pm = crate::effects::transparent_pixmap(pw, ph);
        paint_appearance_skia(&mut mask_pm, &m.path, m.fillable, &m.bbox, &m.appearance, t);
        crate::effects::apply_luminance_mask(&mut layer, &mask_pm, m.invert);
    }
    Some(ShapeLayer {
        pixmap: layer,
        doc_origin: (dx, dy),
    })
}

/// Rasterize an [`Appearance`] stack onto `path`: fills bottom-to-top (only when
/// `fillable`), then strokes bottom-to-top, each scaled by its per-item opacity.
///
/// **Blend modes really composite now.** A `Normal` layer is drawn straight onto
/// `pixmap` with `tiny-skia` source-over (the fast path). A non-`Normal` layer is
/// rasterized alone into a transparent scratch pixmap (same size as `pixmap`,
/// same transform) and then composited onto `pixmap` with the separable
/// Porter-Duff blend math in [`crate::effects::composite_blended`], so it blends
/// against everything painted beneath it — closing the long-standing "stored but
/// not composited" Appearance gap.
fn paint_appearance_skia(
    pixmap: &mut Pixmap,
    path: &tiny_skia::Path,
    fillable: bool,
    bbox: &[f32; 4],
    appearance: &Appearance,
    transform: Transform,
) {
    let (w, h) = (pixmap.width(), pixmap.height());
    // Paint one layer's `paint`+`draw` either straight (Normal) or via a blended
    // scratch composite. `draw` rasterizes onto whichever pixmap it is handed.
    let paint_layer = |pixmap: &mut Pixmap,
                       blend: crate::appearance::BlendMode,
                       draw: &dyn Fn(&mut Pixmap)| {
        if !blend.is_separable_blend() {
            draw(pixmap);
            return;
        }
        // Non-Normal: isolate this layer on a transparent scratch, then blend it
        // over the accumulated backdrop.
        let mut scratch = crate::effects::transparent_pixmap(w, h);
        draw(&mut scratch);
        crate::effects::composite_blended(pixmap, &scratch, blend);
    };

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
            let draw = |pm: &mut Pixmap| {
                pm.fill_path(path, &paint, TsFillRule::Winding, transform, None);
            };
            paint_layer(pixmap, fill.blend, &draw);
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
        let draw = |pm: &mut Pixmap| {
            pm.stroke_path(path, &paint, &s, transform, None);
        };
        paint_layer(pixmap, stroke.blend, &draw);
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
                    omask: None,
                    omask_path: false,
                    omask_invert: false,
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
                    omask: None,
                    omask_path: false,
                    omask_invert: false,
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
                omask: None,
                omask_path: false,
                omask_invert: false,
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
                omask: None,
                omask_path: false,
                omask_invert: false,
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
        draw_shape_skia(&mut pixmap, &doc.shapes[0], Transform::identity(), None);
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
            omask: None,
            omask_path: false,
            omask_invert: false,
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
            effects: vec![],
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
            omask: None,
            omask_path: false,
            omask_invert: false,
        };
        // Bottom red, top opaque green → centre reads green.
        s.set_appearance(Some(Appearance {
            fills: vec![
                Fill::solid([1.0, 0.0, 0.0, 1.0]),
                Fill::solid([0.0, 1.0, 0.0, 1.0]),
            ],
            strokes: vec![],
            effects: vec![],
        }));
        let mut pixmap = Pixmap::new(100, 100).unwrap();
        pixmap.fill(TsColor::WHITE);
        draw_shape_skia(&mut pixmap, &s, Transform::identity(), None);
        let p = pixmap.pixel(50, 50).unwrap();
        assert!(
            p.green() > p.red() && p.green() > p.blue(),
            "top green fill wins: {},{},{}",
            p.red(),
            p.green(),
            p.blue()
        );
    }

    // --- Blend-mode compositing ---------------------------------------------

    /// A stacked PNG with a Multiply top fill must darken where it overlaps the
    /// bottom fill (Multiply composites against the backdrop, not source-over).
    #[test]
    fn png_blend_multiply_darkens_against_backdrop() {
        use crate::appearance::{Appearance, BlendMode, Fill};
        let mut s = Shape::Rect {
            rect: [0.0, 0.0, 100.0, 100.0],
            fill: [1.0, 1.0, 1.0, 1.0],
            fill_gradient: None,
            stroke: [0.0, 0.0, 0.0, 0.0],
            stroke_w: 0.0,
            stroke_style: StrokeStyle::default(),
            appearance: None,
            visible: true,
            group: None,
            clip: None,
            mask: false,
            omask: None,
            omask_path: false,
            omask_invert: false,
        };
        // Bottom 60% grey, top 60% grey Multiply → 0.36 grey (much darker than
        // either layer alone, which a source-over top fill could never produce).
        s.set_appearance(Some(Appearance {
            fills: vec![
                Fill::solid([0.6, 0.6, 0.6, 1.0]),
                Fill {
                    paint: Paint::Solid([0.6, 0.6, 0.6, 1.0]),
                    opacity: 1.0,
                    blend: BlendMode::Multiply,
                    visible: true,
                },
            ],
            strokes: vec![],
            effects: vec![],
        }));
        let mut pixmap = Pixmap::new(100, 100).unwrap();
        pixmap.fill(TsColor::WHITE);
        draw_shape_skia(&mut pixmap, &s, Transform::identity(), None);
        let p = pixmap.pixel(50, 50).unwrap();
        let expected = (0.36_f32 * 255.0).round() as i32;
        assert!(
            (p.red() as i32 - expected).abs() <= 6,
            "multiply should darken to ~{expected}, got {}",
            p.red()
        );
    }

    /// SVG export tags a non-Normal paint layer with `mix-blend-mode` so it
    /// composites in any viewer; a Normal layer emits no blend style.
    #[test]
    fn svg_emits_mix_blend_mode_for_non_normal() {
        use crate::appearance::{Appearance, BlendMode, Fill};
        let mut s = Shape::Rect {
            rect: [0.0, 0.0, 50.0, 50.0],
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
            omask: None,
            omask_path: false,
            omask_invert: false,
        };
        s.set_appearance(Some(Appearance {
            fills: vec![
                Fill::solid([1.0, 0.0, 0.0, 1.0]), // Normal: no style
                Fill {
                    paint: Paint::Solid([0.0, 0.0, 1.0, 1.0]),
                    opacity: 1.0,
                    blend: BlendMode::Screen,
                    visible: true,
                },
            ],
            strokes: vec![],
            effects: vec![],
        }));
        let doc = Document {
            shapes: vec![s],
            ..Default::default()
        };
        let svg = to_svg(&doc, 100.0, 100.0);
        assert!(
            svg.contains("mix-blend-mode:screen"),
            "non-normal layer gets a blend style: {svg}"
        );
        // Exactly one blend style (the Normal layer emits none).
        assert_eq!(svg.matches("mix-blend-mode").count(), 1, "svg: {svg}");
    }

    // --- Opacity masks ------------------------------------------------------

    fn omask_rect(rect: [f32; 4], fill: [f32; 4], omask: Option<u64>, mask: bool) -> Shape {
        Shape::Rect {
            rect,
            fill,
            fill_gradient: None,
            stroke: [0.0, 0.0, 0.0, 0.0],
            stroke_w: 0.0,
            stroke_style: StrokeStyle::default(),
            appearance: None,
            visible: true,
            group: None,
            clip: None,
            mask: false,
            omask,
            omask_path: mask,
            omask_invert: false,
        }
    }

    /// PNG: a content shape masked by a half-covering white rect is opaque under
    /// the mask and erased outside it (luminance·coverage drives alpha).
    #[test]
    fn png_opacity_mask_reveals_under_mask_hides_outside() {
        let mut doc = Document::new();
        doc.shapes.clear();
        // Red content fills the left 100×100; a white mask covers only its left
        // half (0..50). Under the mask → red shows; right of it → erased to white.
        doc.shapes
            .push(omask_rect([0.0, 0.0, 100.0, 100.0], [1.0, 0.0, 0.0, 1.0], Some(0), false));
        doc.shapes
            .push(omask_rect([0.0, 0.0, 50.0, 100.0], [1.0, 1.0, 1.0, 1.0], Some(0), true));

        let mut pixmap = Pixmap::new(100, 100).unwrap();
        pixmap.fill(TsColor::WHITE);
        for (i, shape) in doc.render_shapes() {
            let mask = doc.opacity_mask_of(i);
            draw_shape_skia(&mut pixmap, &shape, Transform::identity(), mask.as_ref());
        }
        // Under the white mask (x=25): red shows through.
        let under = pixmap.pixel(25, 50).unwrap();
        assert!(
            under.red() > 200 && under.green() < 80 && under.blue() < 80,
            "under mask should be red: {},{},{}",
            under.red(),
            under.green(),
            under.blue()
        );
        // Outside the mask (x=75): content hidden → page white shows.
        let outside = pixmap.pixel(75, 50).unwrap();
        assert!(
            outside.red() > 240 && outside.green() > 240 && outside.blue() > 240,
            "outside mask should be white page: {},{},{}",
            outside.red(),
            outside.green(),
            outside.blue()
        );
    }

    /// SVG: an opacity-masked shape emits a luminance `<mask>` def and references
    /// it on the content's group.
    #[test]
    fn svg_emits_opacity_mask_def_and_ref() {
        let mut doc = Document::new();
        doc.shapes.clear();
        doc.shapes
            .push(omask_rect([0.0, 0.0, 100.0, 100.0], [1.0, 0.0, 0.0, 1.0], Some(0), false));
        doc.shapes
            .push(omask_rect([0.0, 0.0, 50.0, 100.0], [1.0, 1.0, 1.0, 1.0], Some(0), true));
        let svg = to_svg(&doc, 100.0, 100.0);
        assert!(svg.contains("<mask id=\"om0\""), "mask def: {svg}");
        assert!(svg.contains("mask-type=\"luminance\""), "luminance mask: {svg}");
        assert!(svg.contains("mask=\"url(#om0)\""), "masked group: {svg}");
        // The mask path itself is not emitted as a normal painted shape (only one
        // <rect> for the content's fill, inside the masked group).
    }

    // --- Live effects -------------------------------------------------------

    fn effect_shape(effects: Vec<Effect>) -> Shape {
        use crate::appearance::{Appearance, Fill};
        let mut s = Shape::Rect {
            rect: [40.0, 40.0, 40.0, 40.0],
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
            omask: None,
            omask_path: false,
            omask_invert: false,
        };
        s.set_appearance(Some(Appearance {
            fills: vec![Fill::solid([1.0, 0.0, 0.0, 1.0])],
            strokes: vec![],
            effects,
        }));
        s
    }

    /// A shape with a drop shadow + blur emits an SVG `<filter>` with the
    /// matching primitives and wraps the paint stack in a filtered group.
    #[test]
    fn svg_emits_effect_filter() {
        let doc = Document {
            shapes: vec![effect_shape(vec![
                Effect::drop_shadow(),
                Effect::GaussianBlur { radius: 5.0 },
            ])],
            ..Default::default()
        };
        let svg = to_svg(&doc, 200.0, 200.0);
        assert!(svg.contains("<filter id=\"fx0\""), "filter def: {svg}");
        assert!(svg.contains("<feDropShadow"), "drop-shadow primitive: {svg}");
        assert!(svg.contains("<feGaussianBlur"), "blur primitive: {svg}");
        assert!(svg.contains("filter=\"url(#fx0)\""), "filtered group: {svg}");
    }

    /// A shape with no active effect emits no filter (back-compat: plain output).
    #[test]
    fn svg_no_filter_without_effects() {
        let doc = Document {
            shapes: vec![effect_shape(vec![Effect::GaussianBlur { radius: 0.0 }])],
            ..Default::default()
        };
        let svg = to_svg(&doc, 200.0, 200.0);
        assert!(!svg.contains("<filter"), "no filter for inactive fx: {svg}");
    }

    /// A drop-shadow PNG still encodes, and the shadow paints pixels *outside*
    /// the shape's tight bounds (down-right of it), proving the effect raster is
    /// composited onto the page.
    #[test]
    fn png_drop_shadow_paints_outside_bounds() {
        let doc = Document {
            shapes: vec![effect_shape(vec![Effect::DropShadow {
                dx: 8.0,
                dy: 8.0,
                blur: 3.0,
                color: [0.0, 0.0, 0.0, 1.0],
                opacity: 1.0,
            }])],
            ..Default::default()
        };
        let bytes = to_png(&doc, 200.0, 200.0).expect("png should encode");
        assert_eq!(
            &bytes[0..8],
            &[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a]
        );
        // Re-rasterize to sample. Shape spans doc (40,40)-(80,80) on a white page;
        // a point just past the bottom-right corner should be darkened by the
        // shadow (not pure white).
        let mut pixmap = Pixmap::new(200, 200).unwrap();
        pixmap.fill(TsColor::WHITE);
        draw_shape_skia(&mut pixmap, &doc.shapes[0], Transform::identity(), None);
        let p = pixmap.pixel(85, 85).unwrap();
        assert!(
            p.red() < 250 && p.green() < 250 && p.blue() < 250,
            "shadow should darken just outside the shape: {},{},{}",
            p.red(),
            p.green(),
            p.blue()
        );
    }

    /// An effect layer's placement: the rasterized layer's `doc_origin` sits at
    /// the padded top-left of the shape (bbox minus the effect padding).
    #[test]
    fn render_shape_layer_pads_bounds() {
        let s = effect_shape(vec![Effect::GaussianBlur { radius: 4.0 }]);
        let (path, fillable) = skia_path_of(&s).unwrap();
        let bbox = s.bounds().map(|b| [b.x, b.y, b.w, b.h]).unwrap();
        let ap = s.effective_appearance();
        let layer = render_shape_layer(&path, fillable, &bbox, &ap, 1.0).unwrap();
        // pad = 3·radius = 12, so the origin is shifted up-left by 12 from (40,40).
        assert!((layer.doc_origin.0 - 28.0).abs() < 1.0, "origin x");
        assert!((layer.doc_origin.1 - 28.0).abs() < 1.0, "origin y");
        // The layer is the padded shape (40 + 2·12 = 64 units) → ~64 px at 1×.
        assert!(layer.pixmap.width() >= 64);
    }
}
