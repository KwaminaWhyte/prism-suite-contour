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

use crate::appearance::Appearance;
use crate::artboard::{self, Artboard};
use crate::gradient::Gradient;
use crate::swatches::{self, Swatches};
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
        /// Optional stacked [`Appearance`] (multiple fills/strokes) that, when
        /// `Some`, overrides the single `fill`/`stroke` fields on every render
        /// surface. Additive (`#[serde(default)]` → `None`), so older files load
        /// with their single fill/stroke and render unchanged.
        #[serde(default)]
        appearance: Option<Appearance>,
        #[serde(default = "default_true")]
        visible: bool,
        /// Group membership: shapes sharing a `Some(id)` form one group and are
        /// selected / moved / transformed as a unit. Additive
        /// (`#[serde(default)]` → `None`), so older files load ungrouped.
        #[serde(default)]
        group: Option<u64>,
        /// Clip-set membership: shapes sharing a `Some(id)` form one clipping
        /// mask, one of them flagged [`mask`](Self). Additive (`#[serde(default)]`
        /// → `None`), so older files load unclipped.
        #[serde(default)]
        clip: Option<u64>,
        /// Whether this shape is the *masking path* of its clip set. Additive
        /// (`#[serde(default)]` → `false`).
        #[serde(default)]
        mask: bool,
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
        #[serde(default)]
        appearance: Option<Appearance>,
        #[serde(default = "default_true")]
        visible: bool,
        #[serde(default)]
        group: Option<u64>,
        #[serde(default)]
        clip: Option<u64>,
        #[serde(default)]
        mask: bool,
    },
    Line {
        p0: (f32, f32),
        p1: (f32, f32),
        stroke: [f32; 4],
        stroke_w: f32,
        #[serde(default)]
        stroke_style: StrokeStyle,
        #[serde(default)]
        appearance: Option<Appearance>,
        #[serde(default = "default_true")]
        visible: bool,
        #[serde(default)]
        group: Option<u64>,
        #[serde(default)]
        clip: Option<u64>,
        #[serde(default)]
        mask: bool,
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
        #[serde(default)]
        appearance: Option<Appearance>,
        #[serde(default = "default_true")]
        visible: bool,
        #[serde(default)]
        group: Option<u64>,
        #[serde(default)]
        clip: Option<u64>,
        #[serde(default)]
        mask: bool,
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

    /// The shape's group id, if it belongs to one. Shapes sharing an id form a
    /// group that selects / moves / transforms as a unit.
    pub fn group(&self) -> Option<u64> {
        match self {
            Shape::Rect { group, .. }
            | Shape::Ellipse { group, .. }
            | Shape::Line { group, .. }
            | Shape::Path { group, .. } => *group,
        }
    }

    /// Set (or clear, with `None`) the shape's group membership.
    pub fn set_group(&mut self, g: Option<u64>) {
        match self {
            Shape::Rect { group, .. }
            | Shape::Ellipse { group, .. }
            | Shape::Line { group, .. }
            | Shape::Path { group, .. } => *group = g,
        }
    }

    /// The shape's clip-set id, if it belongs to a clipping mask. Shapes sharing
    /// an id form one clip set; the one with [`is_mask`](Self::is_mask) confines
    /// the rest.
    pub fn clip(&self) -> Option<u64> {
        match self {
            Shape::Rect { clip, .. }
            | Shape::Ellipse { clip, .. }
            | Shape::Line { clip, .. }
            | Shape::Path { clip, .. } => *clip,
        }
    }

    /// Set (or clear, with `None`) the shape's clip-set membership.
    pub fn set_clip(&mut self, c: Option<u64>) {
        match self {
            Shape::Rect { clip, .. }
            | Shape::Ellipse { clip, .. }
            | Shape::Line { clip, .. }
            | Shape::Path { clip, .. } => *clip = c,
        }
    }

    /// Whether this shape is the masking path of its clip set.
    pub fn is_mask(&self) -> bool {
        match self {
            Shape::Rect { mask, .. }
            | Shape::Ellipse { mask, .. }
            | Shape::Line { mask, .. }
            | Shape::Path { mask, .. } => *mask,
        }
    }

    /// Flag (or unflag) this shape as the masking path of its clip set.
    pub fn set_mask(&mut self, m: bool) {
        match self {
            Shape::Rect { mask, .. }
            | Shape::Ellipse { mask, .. }
            | Shape::Line { mask, .. }
            | Shape::Path { mask, .. } => *mask = m,
        }
    }

    /// Clear both clip-set tags (id + mask flag), releasing the shape from any
    /// clipping mask. Used by `Object → Clipping Mask → Release`.
    pub fn clear_clip(&mut self) {
        self.set_clip(None);
        self.set_mask(false);
    }

    /// This shape's clip tag pair, for the pure [`clip`](crate::clip) helpers.
    pub fn clip_tag(&self) -> crate::clip::ClipTag {
        crate::clip::ClipTag::new(self.clip(), self.is_mask())
    }

    /// The shape's filled outline as a single closed document-space polygon (the
    /// input both boolean ops and clipping masks consume). `Rect`/`Ellipse`
    /// sample their outline; a closed `Path` flattens its (possibly bezier)
    /// outline; an open `Path` or a `Line` has no fillable region and returns
    /// `None`.
    pub fn outline_polygon(&self) -> Option<Vec<(f32, f32)>> {
        let pts: Vec<(f32, f32)> = match self {
            Shape::Rect { rect, .. } => {
                let (x, y, w, h) = (rect[0], rect[1], rect[2], rect[3]);
                vec![(x, y), (x + w, y), (x + w, y + h), (x, y + h)]
            }
            Shape::Ellipse { rect, .. } => {
                let cx = rect[0] + rect[2] * 0.5;
                let cy = rect[1] + rect[3] * 0.5;
                let rx = rect[2] * 0.5;
                let ry = rect[3] * 0.5;
                (0..64)
                    .map(|i| {
                        let t = i as f32 / 64.0 * std::f32::consts::TAU;
                        (cx + rx * t.cos(), cy + ry * t.sin())
                    })
                    .collect()
            }
            Shape::Path {
                points,
                closed,
                handles,
                ..
            } => {
                if !*closed {
                    return None;
                }
                path::flatten(points, handles, true)
            }
            Shape::Line { .. } => return None,
        };
        (pts.len() >= 3).then_some(pts)
    }

    /// The shape's stroke colour (straight sRGB RGBA). Every variant has a
    /// stroke, so this is always `Some` — the `Option` keeps the accessor shaped
    /// like [`fill_color`](Self::fill_color) for the appearance helpers.
    pub fn stroke_color(&self) -> Option<[f32; 4]> {
        match self {
            Shape::Rect { stroke, .. }
            | Shape::Ellipse { stroke, .. }
            | Shape::Line { stroke, .. }
            | Shape::Path { stroke, .. } => Some(*stroke),
        }
    }

    /// Set the shape's stroke colour.
    pub fn set_stroke_color(&mut self, c: [f32; 4]) {
        match self {
            Shape::Rect { stroke, .. }
            | Shape::Ellipse { stroke, .. }
            | Shape::Line { stroke, .. }
            | Shape::Path { stroke, .. } => *stroke = c,
        }
    }

    /// The shape's stroke width in document units.
    pub fn stroke_width(&self) -> f32 {
        match self {
            Shape::Rect { stroke_w, .. }
            | Shape::Ellipse { stroke_w, .. }
            | Shape::Line { stroke_w, .. }
            | Shape::Path { stroke_w, .. } => *stroke_w,
        }
    }

    /// Set the shape's stroke width (document units).
    pub fn set_stroke_width(&mut self, w: f32) {
        match self {
            Shape::Rect { stroke_w, .. }
            | Shape::Ellipse { stroke_w, .. }
            | Shape::Line { stroke_w, .. }
            | Shape::Path { stroke_w, .. } => *stroke_w = w,
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

    /// The shape's stacked [`Appearance`], if one has been attached. When `Some`
    /// it overrides the single `fill`/`stroke` fields on every render surface;
    /// when `None` the shape paints from its legacy single fields. Every variant
    /// can carry one (a `Line`'s stack just holds strokes).
    pub fn appearance(&self) -> Option<&Appearance> {
        match self {
            Shape::Rect { appearance, .. }
            | Shape::Ellipse { appearance, .. }
            | Shape::Line { appearance, .. }
            | Shape::Path { appearance, .. } => appearance.as_ref(),
        }
    }

    /// Mutable access to the shape's `appearance` slot (set/clear the stack).
    pub fn appearance_mut(&mut self) -> &mut Option<Appearance> {
        match self {
            Shape::Rect { appearance, .. }
            | Shape::Ellipse { appearance, .. }
            | Shape::Line { appearance, .. }
            | Shape::Path { appearance, .. } => appearance,
        }
    }

    /// Set (or clear, with `None`) the shape's stacked appearance.
    pub fn set_appearance(&mut self, a: Option<Appearance>) {
        *self.appearance_mut() = a;
    }

    /// The appearance the renderers should walk: the attached stack if there is
    /// one, otherwise a freshly-migrated one-fill/one-stroke stack built from the
    /// shape's legacy single fields ([`Appearance::from_legacy`]). This is the
    /// single source of truth for the canvas painter and the SVG / PNG exporters,
    /// so a shape renders identically whether or not it has an explicit stack.
    pub fn effective_appearance(&self) -> Appearance {
        match self.appearance() {
            Some(a) => a.clone(),
            None => Appearance::from_legacy(
                self.fill_color(),
                self.fill_gradient(),
                self.stroke_color().unwrap_or([0.0, 0.0, 0.0, 0.0]),
                self.stroke_width(),
                self.stroke_style(),
            ),
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

    /// Replace every occurrence of the colour `old` with `new` across this
    /// shape's paint — its solid fill, its stroke, and any gradient-stop colours.
    /// Colours are compared with the same picker-rounding tolerance as the
    /// swatch model. Returns `true` if anything changed.
    ///
    /// This is the per-shape half of a **global swatch** recolour: when a global
    /// swatch's colour is edited, the document walks every shape and remaps the
    /// old colour to the new one, so artwork painted with that swatch follows the
    /// edit (Illustrator's global-colour behaviour).
    pub fn remap_color(&mut self, old: [f32; 4], new: [f32; 4]) -> bool {
        let mut changed = false;
        if let Some(c) = self.fill_color() {
            if swatches::colors_eq(c, old) {
                self.set_fill_color(new);
                changed = true;
            }
        }
        if let Some(c) = self.stroke_color() {
            if swatches::colors_eq(c, old) {
                self.set_stroke_color(new);
                changed = true;
            }
        }
        // Remap colours inside an attached appearance stack (solid paints and
        // gradient stops on every fill / stroke) so a global swatch edit follows
        // stacked artwork too.
        if let Some(ap) = self.appearance_mut() {
            use crate::appearance::Paint;
            let mut remap_paint = |p: &mut Paint| match p {
                Paint::Solid(c) => {
                    if swatches::colors_eq(*c, old) {
                        *c = new;
                        changed = true;
                    }
                }
                Paint::Gradient(g) => {
                    for stop in g.stops.iter_mut() {
                        if swatches::colors_eq(stop.color, old) {
                            stop.color = new;
                            changed = true;
                        }
                    }
                }
            };
            for f in ap.fills.iter_mut() {
                remap_paint(&mut f.paint);
            }
            for s in ap.strokes.iter_mut() {
                remap_paint(&mut s.paint);
            }
        }
        let grad_changed = match self {
            Shape::Rect { fill_gradient, .. }
            | Shape::Ellipse { fill_gradient, .. }
            | Shape::Path { fill_gradient, .. } => fill_gradient
                .as_mut()
                .map(|g| {
                    let mut any = false;
                    for stop in g.stops.iter_mut() {
                        if swatches::colors_eq(stop.color, old) {
                            stop.color = new;
                            any = true;
                        }
                    }
                    any
                })
                .unwrap_or(false),
            Shape::Line { .. } => false,
        };
        changed || grad_changed
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

    /// A closed corner [`Shape::Path`] tracing `ring`, inheriting this shape's
    /// paint style (fill, gradient, stroke colour / width / dashes) and group
    /// tag, but **never** a clip/mask tag (the result is already clipped, so it
    /// renders as a plain shape). Used by clip-mask resolution to turn a content
    /// outline cropped to the mask into a drawable path. All anchors are corners.
    pub fn with_outline(&self, ring: Vec<(f32, f32)>) -> Shape {
        let n = ring.len();
        Shape::Path {
            points: ring,
            closed: true,
            fill: self.fill_color().unwrap_or([0.0, 0.0, 0.0, 0.0]),
            fill_gradient: self.fill_gradient().cloned(),
            stroke: self.stroke_color().unwrap_or([0.0, 0.0, 0.0, 0.0]),
            stroke_w: self.stroke_width(),
            stroke_style: self.stroke_style().clone(),
            // Carry the stacked appearance through so a clipped multi-fill shape
            // keeps its full paint stack after clipping.
            appearance: self.appearance().cloned(),
            handles: vec![(0.0, 0.0); n],
            visible: self.visible(),
            group: self.group(),
            clip: None,
            mask: false,
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
                appearance,
                visible,
                group,
                clip,
                mask,
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
                    appearance: appearance.clone(),
                    handles,
                    visible: *visible,
                    group: *group,
                    clip: *clip,
                    mask: *mask,
                }
            }
            Shape::Ellipse {
                rect,
                fill,
                fill_gradient,
                stroke,
                stroke_w,
                stroke_style,
                appearance,
                visible,
                group,
                clip,
                mask,
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
                    appearance: appearance.clone(),
                    handles,
                    visible: *visible,
                    group: *group,
                    clip: *clip,
                    mask: *mask,
                }
            }
            Shape::Line {
                p0,
                p1,
                stroke,
                stroke_w,
                stroke_style,
                appearance,
                visible,
                group,
                clip,
                mask,
            } => Shape::Path {
                points: vec![*p0, *p1],
                closed: false,
                fill: [0.0, 0.0, 0.0, 0.0],
                fill_gradient: None,
                stroke: *stroke,
                stroke_w: *stroke_w,
                stroke_style: stroke_style.clone(),
                appearance: appearance.clone(),
                handles: vec![(0.0, 0.0); 2],
                visible: *visible,
                group: *group,
                clip: *clip,
                mask: *mask,
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

/// Default artboard size (document units) for a fresh document and for the
/// single board a pre-artboards `.contour` file loads with — matching the
/// app's former single-`Size` artboard.
pub const DEFAULT_ARTBOARD: [f32; 2] = [1000.0, 700.0];

/// One default artboard for documents that predate the `artboards` field. A
/// pre-artboards `.contour` always rendered one 1000×700 board at the origin, so
/// `#[serde(default)]` reconstructs exactly that.
fn default_artboards() -> Vec<Artboard> {
    vec![Artboard::new(
        artboard::default_name(0),
        [0.0, 0.0, DEFAULT_ARTBOARD[0], DEFAULT_ARTBOARD[1]],
    )]
}

/// The whole vector document: an ordered list of shapes, any ruler guides, and
/// the artboard stack.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Document {
    pub shapes: Vec<Shape>,
    /// User-placed ruler guides. Additive (`#[serde(default)]`) so pre-existing
    /// `.contour` files — which have no `guides` key — load with none.
    #[serde(default)]
    pub guides: Vec<Guide>,
    /// The artboards. Always at least one (kept non-empty by the editor). A
    /// pre-artboards `.contour` file loads with a single default board, so older
    /// documents round-trip with no visible change. Additive
    /// (`#[serde(default = "default_artboards")]`).
    #[serde(default = "default_artboards")]
    pub artboards: Vec<Artboard>,
    /// Index of the active artboard (frames export / align-to / new-artboard
    /// placement). Clamped into range by the editor. Additive.
    #[serde(default)]
    pub active_artboard: usize,
    /// The document colour palette (the Swatches panel). Additive
    /// (`#[serde(default)]`), so a pre-swatches `.contour` file loads with the
    /// default starter palette; saved palettes round-trip through serde.
    #[serde(default)]
    pub swatches: Swatches,
}

impl Default for Document {
    fn default() -> Self {
        Self {
            shapes: Vec::new(),
            guides: Vec::new(),
            artboards: default_artboards(),
            active_artboard: 0,
            swatches: Swatches::default(),
        }
    }
}

impl Document {
    pub fn new() -> Self {
        Self::default()
    }

    /// The active artboard, clamped so an out-of-range / empty stack still
    /// returns a board. Always `Some` for a well-formed document (the editor
    /// keeps ≥1 artboard); falls back to the first board if the active index is
    /// stale.
    pub fn active_artboard(&self) -> Option<&Artboard> {
        if self.artboards.is_empty() {
            return None;
        }
        let i = self.active_artboard.min(self.artboards.len() - 1);
        self.artboards.get(i)
    }

    /// Repair an opened/legacy document: ensure at least one artboard exists and
    /// the active index is in range. Called after deserialize so a hand-edited
    /// or corrupt file can't leave the editor with zero artboards.
    pub fn normalize_artboards(&mut self) {
        if self.artboards.is_empty() {
            self.artboards = default_artboards();
        }
        if self.active_artboard >= self.artboards.len() {
            self.active_artboard = self.artboards.len() - 1;
        }
    }

    /// Remap the colour `old` to `new` across every shape's paint (fill, stroke,
    /// gradient stops). Returns the number of shapes that changed. Drives a
    /// **global swatch** recolour: editing a global swatch hands back its
    /// `(old, new)` pair, and this walks the artwork so every shape painted with
    /// that swatch follows the edit.
    pub fn remap_color(&mut self, old: [f32; 4], new: [f32; 4]) -> usize {
        if swatches::colors_eq(old, new) {
            return 0;
        }
        let mut n = 0;
        for s in self.shapes.iter_mut() {
            if s.remap_color(old, new) {
                n += 1;
            }
        }
        n
    }

    /// The shapes to *render*, with clipping masks resolved, paired with the
    /// originating shape's index (so the canvas keeps its selection highlight
    /// mapping). Paint / export iterate this rather than `shapes` directly.
    ///
    /// For each shape:
    /// - **mask path** of a clip set → omitted (an Illustrator clipping path
    ///   paints no fill or stroke once it becomes a mask),
    /// - **clipped content** (a non-mask member of a clip set) → replaced by its
    ///   outline intersected against the mask, as a styled closed `Path`. If the
    ///   content falls entirely outside the mask the shape is omitted; if the mask
    ///   geometry is unusable the original shape is kept unclipped (graceful
    ///   degradation),
    /// - everything else → kept as-is.
    ///
    /// Hidden shapes are still skipped by the caller; this method does not filter
    /// on visibility so callers keep their existing `visible()` checks.
    pub fn render_shapes(&self) -> Vec<(usize, Shape)> {
        let tags: Vec<crate::clip::ClipTag> = self.shapes.iter().map(|s| s.clip_tag()).collect();
        let mut out: Vec<(usize, Shape)> = Vec::with_capacity(self.shapes.len());
        for (i, shape) in self.shapes.iter().enumerate() {
            match shape.clip() {
                None => out.push((i, shape.clone())),
                Some(_) if shape.is_mask() => { /* mask paints nothing */ }
                Some(_) => {
                    // Clip this content shape against its set's mask outline.
                    let clipped = crate::clip::mask_of(&tags, i)
                        .and_then(|m| self.shapes[m].outline_polygon())
                        .and_then(|mask_poly| {
                            shape
                                .outline_polygon()
                                .and_then(|subj| crate::clip::clip_polygon(&subj, &mask_poly))
                        });
                    match clipped {
                        Some(ring) => out.push((i, shape.clone().with_outline(ring))),
                        // No usable mask geometry: keep the content unclipped.
                        // (An empty intersection drops the shape entirely.)
                        None if crate::clip::mask_of(&tags, i)
                            .map(|m| self.shapes[m].outline_polygon().is_none())
                            .unwrap_or(true) =>
                        {
                            out.push((i, shape.clone()))
                        }
                        None => { /* clipped to nothing — omit */ }
                    }
                }
            }
        }
        out
    }
}
