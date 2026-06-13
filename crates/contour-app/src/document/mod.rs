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

pub use path::{
    anchors_in_rect, bez_path, flatten, handle_at, handle_endpoints, nearest_segment,
    point_in_rings, rects_intersect, FillRule, SubPath,
};
pub use style::{Arrowhead, LineCap, LineJoin, StrokeAlign, StrokeStyle};

use crate::text::TextParams;

use crate::appearance::Appearance;
use crate::artboard::{self, Artboard};
use crate::gradient::Gradient;
use crate::graphic_styles::GraphicStyles;
use crate::liveshape::LiveShape;
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

/// A read-only view of one editable sub-contour: its anchor `points`, per-anchor
/// out-tangent `handles`, and whether it is `closed`. Returned by
/// [`Shape::contour`] so the Direct-Select tool treats a `Path` and each
/// sub-path of a `Compound` uniformly.
pub type ContourRef<'a> = (&'a [(f32, f32)], &'a [(f32, f32)], bool);

/// A mutable view of one editable sub-contour (anchor points, out-tangent
/// handles, `closed`). Returned by [`Shape::contour_mut`].
pub type ContourMut<'a> = (&'a mut Vec<(f32, f32)>, &'a mut Vec<(f32, f32)>, bool);

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
        /// Opacity-mask set membership: shapes sharing a `Some(id)` form one
        /// opacity-mask group, one of them flagged [`omask_path`](Self) as the
        /// luminance mask. Additive (`#[serde(default)]` → `None`), so older
        /// files load unmasked.
        #[serde(default)]
        omask: Option<u64>,
        /// Whether this shape is the *luminance mask* of its opacity-mask set.
        /// Additive (`#[serde(default)]` → `false`).
        #[serde(default)]
        omask_path: bool,
        /// Invert the opacity mask (black reveals, white hides) for the masked
        /// content of this set. Additive (`#[serde(default)]` → `false`).
        #[serde(default)]
        omask_invert: bool,
        /// Blend-set membership: shapes sharing a `Some(id)` form one blend run
        /// (the two ends plus the generated steps). Additive (`#[serde(default)]`
        /// → `None`), so older files load un-blended.
        #[serde(default)]
        blend: Option<u64>,
        /// Whether this shape is a *generated* intermediate step of its blend set
        /// (vs. one of the two original ends). Release deletes the steps and keeps
        /// the ends. Additive (`#[serde(default)]` → `false`).
        #[serde(default)]
        blend_step: bool,
        /// Optional Layers-panel display name. Additive (`#[serde(default)]` →
        /// `None`), so older files load with the generic type label.
        #[serde(default)]
        name: Option<String>,
        /// Whether the shape is **locked**: it renders normally but cannot be
        /// selected, hit-tested, or edited. Additive (`#[serde(default)]` →
        /// `false`), so older files load unlocked.
        #[serde(default)]
        locked: bool,
        /// Optional Layers-panel colour swatch (the row tint Illustrator gives a
        /// layer). Additive (`#[serde(default)]` → `None`).
        #[serde(default)]
        layer_color: Option<[f32; 4]>,
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
        #[serde(default)]
        omask: Option<u64>,
        #[serde(default)]
        omask_path: bool,
        #[serde(default)]
        omask_invert: bool,
        #[serde(default)]
        blend: Option<u64>,
        #[serde(default)]
        blend_step: bool,
        /// Optional Layers-panel display name. Additive (`#[serde(default)]` →
        /// `None`), so older files load with the generic type label.
        #[serde(default)]
        name: Option<String>,
        /// Whether the shape is **locked**: it renders normally but cannot be
        /// selected, hit-tested, or edited. Additive (`#[serde(default)]` →
        /// `false`), so older files load unlocked.
        #[serde(default)]
        locked: bool,
        /// Optional Layers-panel colour swatch (the row tint Illustrator gives a
        /// layer). Additive (`#[serde(default)]` → `None`).
        #[serde(default)]
        layer_color: Option<[f32; 4]>,
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
        #[serde(default)]
        omask: Option<u64>,
        #[serde(default)]
        omask_path: bool,
        #[serde(default)]
        omask_invert: bool,
        #[serde(default)]
        blend: Option<u64>,
        #[serde(default)]
        blend_step: bool,
        /// Optional Layers-panel display name. Additive (`#[serde(default)]` →
        /// `None`), so older files load with the generic type label.
        #[serde(default)]
        name: Option<String>,
        /// Whether the shape is **locked**: it renders normally but cannot be
        /// selected, hit-tested, or edited. Additive (`#[serde(default)]` →
        /// `false`), so older files load unlocked.
        #[serde(default)]
        locked: bool,
        /// Optional Layers-panel colour swatch (the row tint Illustrator gives a
        /// layer). Additive (`#[serde(default)]` → `None`).
        #[serde(default)]
        layer_color: Option<[f32; 4]>,
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
        /// Optional **live-shape** parameters (polygon / star). When `Some`, the
        /// `points` / `handles` above are *generated* from these parameters
        /// (about the points' bounding-box centre) and the inspector edits the
        /// count / radius / inner-ratio to regenerate them live, like text type's
        /// `params` → `glyphs`. Additive (`#[serde(default)]` → `None`), so a
        /// hand-drawn path and every older file load as a plain (non-live) path.
        #[serde(default)]
        live: Option<LiveShape>,
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
        #[serde(default)]
        omask: Option<u64>,
        #[serde(default)]
        omask_path: bool,
        #[serde(default)]
        omask_invert: bool,
        #[serde(default)]
        blend: Option<u64>,
        #[serde(default)]
        blend_step: bool,
        /// Optional Layers-panel display name. Additive (`#[serde(default)]` →
        /// `None`), so older files load with the generic type label.
        #[serde(default)]
        name: Option<String>,
        /// Whether the shape is **locked**: it renders normally but cannot be
        /// selected, hit-tested, or edited. Additive (`#[serde(default)]` →
        /// `false`), so older files load unlocked.
        #[serde(default)]
        locked: bool,
        /// Optional Layers-panel colour swatch (the row tint Illustrator gives a
        /// layer). Additive (`#[serde(default)]` → `None`).
        #[serde(default)]
        layer_color: Option<[f32; 4]>,
    },
    /// A **compound path**: one object that keeps several sub-contours (an outer
    /// ring plus inner holes, or several disjoint regions) together, filled as a
    /// unit under a [`FillRule`] (even-odd carves holes, non-zero absorbs same-
    /// wound nesting). This is the document model's real answer to a Pathfinder
    /// result that has holes — instead of expanding it into separate ring shapes,
    /// the holes stay sub-contours of one path. Renders / hit-tests / serializes
    /// as one object, and an `appearance` stack (when present) paints over the
    /// whole compound outline.
    Compound {
        subpaths: Vec<SubPath>,
        #[serde(default)]
        fill_rule: FillRule,
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
        #[serde(default)]
        omask: Option<u64>,
        #[serde(default)]
        omask_path: bool,
        #[serde(default)]
        omask_invert: bool,
        #[serde(default)]
        blend: Option<u64>,
        #[serde(default)]
        blend_step: bool,
        /// Optional Layers-panel display name. Additive (`#[serde(default)]` →
        /// `None`), so older files load with the generic type label.
        #[serde(default)]
        name: Option<String>,
        /// Whether the shape is **locked**: it renders normally but cannot be
        /// selected, hit-tested, or edited. Additive (`#[serde(default)]` →
        /// `false`), so older files load unlocked.
        #[serde(default)]
        locked: bool,
        /// Optional Layers-panel colour swatch (the row tint Illustrator gives a
        /// layer). Additive (`#[serde(default)]` → `None`).
        #[serde(default)]
        layer_color: Option<[f32; 4]>,
    },
    /// A **point-type** object: an editable string plus its font parameters,
    /// rendered as real glyph outlines. The editable model is `params` (text +
    /// size + alignment) anchored at `origin`; the laid-out glyph contours are
    /// cached in `glyphs` (one closed [`SubPath`] per glyph contour, in document
    /// space) so every render surface, geometry query, and the boolean / clip
    /// pipeline treat a text object exactly like a [`Compound`](Self::Compound)
    /// path. Editing any of `params` / `origin` re-runs [`crate::text::layout`]
    /// to refresh `glyphs` (see [`Shape::text_relayout`]). `Object ▸ Type ▸
    /// Convert to Outlines` lifts `glyphs` into a real `Compound`.
    Text {
        /// The editable text + font parameters. Re-laying out on edit refreshes
        /// the `glyphs` cache.
        params: TextParams,
        /// Document-space anchor: the top-left of the first line's em box (where
        /// the Type tool's click lands).
        origin: (f32, f32),
        /// Cached laid-out glyph outlines (one closed contour each). Derived from
        /// `params` + `origin`; rebuilt on edit and re-derivable on load, so it is
        /// serialized for forward-compat but never trusted over a relayout.
        #[serde(default)]
        glyphs: Vec<SubPath>,
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
        #[serde(default)]
        omask: Option<u64>,
        #[serde(default)]
        omask_path: bool,
        #[serde(default)]
        omask_invert: bool,
        #[serde(default)]
        blend: Option<u64>,
        #[serde(default)]
        blend_step: bool,
        /// Optional Layers-panel display name. Additive (`#[serde(default)]` →
        /// `None`), so a text object falls back to its string / the type label.
        #[serde(default)]
        name: Option<String>,
        #[serde(default)]
        locked: bool,
        #[serde(default)]
        layer_color: Option<[f32; 4]>,
    },
}

impl Shape {
    /// Short human label for the layer list.
    pub fn label(&self) -> &'static str {
        match self {
            Shape::Rect { .. } => "Rectangle",
            Shape::Ellipse { .. } => "Ellipse",
            Shape::Line { .. } => "Line",
            // A live polygon / star labels itself so the layer row reads
            // "Polygon" / "Star" rather than the generic "Path".
            Shape::Path { live: Some(ls), .. } => ls.label(),
            Shape::Path { .. } => "Path",
            Shape::Compound { .. } => "Compound Path",
            Shape::Text { .. } => "Type",
        }
    }

    /// Whether the shape is drawn / exported. Hidden shapes are skipped.
    pub fn visible(&self) -> bool {
        match self {
            Shape::Rect { visible, .. }
            | Shape::Ellipse { visible, .. }
            | Shape::Line { visible, .. }
            | Shape::Path { visible, .. }
            | Shape::Compound { visible, .. }
            | Shape::Text { visible, .. } => *visible,
        }
    }

    /// Flip the visibility flag.
    pub fn toggle_visible(&mut self) {
        match self {
            Shape::Rect { visible, .. }
            | Shape::Ellipse { visible, .. }
            | Shape::Line { visible, .. }
            | Shape::Path { visible, .. }
            | Shape::Compound { visible, .. }
            | Shape::Text { visible, .. } => *visible = !*visible,
        }
    }

    /// Whether the shape is **locked**. A locked shape still renders, but it is
    /// excluded from selection, hit-testing, and editing (Illustrator's lock).
    pub fn locked(&self) -> bool {
        match self {
            Shape::Rect { locked, .. }
            | Shape::Ellipse { locked, .. }
            | Shape::Line { locked, .. }
            | Shape::Path { locked, .. }
            | Shape::Compound { locked, .. }
            | Shape::Text { locked, .. } => *locked,
        }
    }

    /// Set the shape's locked flag.
    pub fn set_locked(&mut self, v: bool) {
        match self {
            Shape::Rect { locked, .. }
            | Shape::Ellipse { locked, .. }
            | Shape::Line { locked, .. }
            | Shape::Path { locked, .. }
            | Shape::Compound { locked, .. }
            | Shape::Text { locked, .. } => *locked = v,
        }
    }

    /// Flip the locked flag.
    pub fn toggle_locked(&mut self) {
        let v = self.locked();
        self.set_locked(!v);
    }

    /// Whether the shape can take part in selection / hit-testing / editing: it
    /// must be both **visible** and **unlocked**. The single predicate the canvas
    /// pick paths and the Layers panel share so the two gates never drift apart.
    pub fn selectable(&self) -> bool {
        self.visible() && !self.locked()
    }

    /// The shape's user-set Layers-panel name, if it has one (`None` falls back to
    /// the generic [`label`](Self::label)).
    pub fn name(&self) -> Option<&str> {
        match self {
            Shape::Rect { name, .. }
            | Shape::Ellipse { name, .. }
            | Shape::Line { name, .. }
            | Shape::Path { name, .. }
            | Shape::Compound { name, .. }
            | Shape::Text { name, .. } => name.as_deref(),
        }
    }

    /// Set (or clear, with an empty string) the shape's Layers-panel name. A blank
    /// name is stored as `None` so the row falls back to the type label.
    pub fn set_name(&mut self, n: &str) {
        let value = {
            let t = n.trim();
            (!t.is_empty()).then(|| t.to_string())
        };
        match self {
            Shape::Rect { name, .. }
            | Shape::Ellipse { name, .. }
            | Shape::Line { name, .. }
            | Shape::Path { name, .. }
            | Shape::Compound { name, .. }
            | Shape::Text { name, .. } => *name = value,
        }
    }

    /// The name to show in the Layers panel: the user-set name when present, else
    /// a text object's (first-line) string, else the generic type label.
    pub fn display_name(&self) -> String {
        if let Some(n) = self.name() {
            return n.to_string();
        }
        if let Shape::Text { params, .. } = self {
            let first = params.text.lines().next().unwrap_or("").trim();
            if !first.is_empty() {
                let truncated: String = first.chars().take(24).collect();
                return truncated;
            }
        }
        self.label().to_string()
    }

    /// The shape's Layers-panel colour swatch, if one has been set.
    pub fn layer_color(&self) -> Option<[f32; 4]> {
        match self {
            Shape::Rect { layer_color, .. }
            | Shape::Ellipse { layer_color, .. }
            | Shape::Line { layer_color, .. }
            | Shape::Path { layer_color, .. }
            | Shape::Compound { layer_color, .. }
            | Shape::Text { layer_color, .. } => *layer_color,
        }
    }

    /// Set (or clear, with `None`) the shape's Layers-panel colour swatch.
    pub fn set_layer_color(&mut self, c: Option<[f32; 4]>) {
        match self {
            Shape::Rect { layer_color, .. }
            | Shape::Ellipse { layer_color, .. }
            | Shape::Line { layer_color, .. }
            | Shape::Path { layer_color, .. }
            | Shape::Compound { layer_color, .. }
            | Shape::Text { layer_color, .. } => *layer_color = c,
        }
    }

    /// The shape's group id, if it belongs to one. Shapes sharing an id form a
    /// group that selects / moves / transforms as a unit.
    pub fn group(&self) -> Option<u64> {
        match self {
            Shape::Rect { group, .. }
            | Shape::Ellipse { group, .. }
            | Shape::Line { group, .. }
            | Shape::Path { group, .. }
            | Shape::Compound { group, .. }
            | Shape::Text { group, .. } => *group,
        }
    }

    /// Set (or clear, with `None`) the shape's group membership.
    pub fn set_group(&mut self, g: Option<u64>) {
        match self {
            Shape::Rect { group, .. }
            | Shape::Ellipse { group, .. }
            | Shape::Line { group, .. }
            | Shape::Path { group, .. }
            | Shape::Compound { group, .. }
            | Shape::Text { group, .. } => *group = g,
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
            | Shape::Path { clip, .. }
            | Shape::Compound { clip, .. }
            | Shape::Text { clip, .. } => *clip,
        }
    }

    /// Set (or clear, with `None`) the shape's clip-set membership.
    pub fn set_clip(&mut self, c: Option<u64>) {
        match self {
            Shape::Rect { clip, .. }
            | Shape::Ellipse { clip, .. }
            | Shape::Line { clip, .. }
            | Shape::Path { clip, .. }
            | Shape::Compound { clip, .. }
            | Shape::Text { clip, .. } => *clip = c,
        }
    }

    /// Whether this shape is the masking path of its clip set.
    pub fn is_mask(&self) -> bool {
        match self {
            Shape::Rect { mask, .. }
            | Shape::Ellipse { mask, .. }
            | Shape::Line { mask, .. }
            | Shape::Path { mask, .. }
            | Shape::Compound { mask, .. }
            | Shape::Text { mask, .. } => *mask,
        }
    }

    /// Flag (or unflag) this shape as the masking path of its clip set.
    pub fn set_mask(&mut self, m: bool) {
        match self {
            Shape::Rect { mask, .. }
            | Shape::Ellipse { mask, .. }
            | Shape::Line { mask, .. }
            | Shape::Path { mask, .. }
            | Shape::Compound { mask, .. }
            | Shape::Text { mask, .. } => *mask = m,
        }
    }

    /// Clear both clip-set tags (id + mask flag), releasing the shape from any
    /// clipping mask. Used by `Object → Clipping Mask → Release`.
    pub fn clear_clip(&mut self) {
        self.set_clip(None);
        self.set_mask(false);
    }

    /// The shape's opacity-mask set id, if it belongs to one. Shapes sharing an
    /// id form one opacity-mask set; the one with [`is_omask`](Self::is_omask)
    /// supplies the luminance that drives the others' alpha.
    pub fn omask(&self) -> Option<u64> {
        match self {
            Shape::Rect { omask, .. }
            | Shape::Ellipse { omask, .. }
            | Shape::Line { omask, .. }
            | Shape::Path { omask, .. }
            | Shape::Compound { omask, .. }
            | Shape::Text { omask, .. } => *omask,
        }
    }

    /// Set (or clear, with `None`) the shape's opacity-mask set membership.
    pub fn set_omask(&mut self, m: Option<u64>) {
        match self {
            Shape::Rect { omask, .. }
            | Shape::Ellipse { omask, .. }
            | Shape::Line { omask, .. }
            | Shape::Path { omask, .. }
            | Shape::Compound { omask, .. }
            | Shape::Text { omask, .. } => *omask = m,
        }
    }

    /// Whether this shape is the luminance mask of its opacity-mask set.
    pub fn is_omask(&self) -> bool {
        match self {
            Shape::Rect { omask_path, .. }
            | Shape::Ellipse { omask_path, .. }
            | Shape::Line { omask_path, .. }
            | Shape::Path { omask_path, .. }
            | Shape::Compound { omask_path, .. }
            | Shape::Text { omask_path, .. } => *omask_path,
        }
    }

    /// Flag (or unflag) this shape as the luminance mask of its opacity-mask set.
    pub fn set_omask_path(&mut self, m: bool) {
        match self {
            Shape::Rect { omask_path, .. }
            | Shape::Ellipse { omask_path, .. }
            | Shape::Line { omask_path, .. }
            | Shape::Path { omask_path, .. }
            | Shape::Compound { omask_path, .. }
            | Shape::Text { omask_path, .. } => *omask_path = m,
        }
    }

    /// Whether this shape's opacity mask is inverted (black reveals, white hides).
    pub fn omask_invert(&self) -> bool {
        match self {
            Shape::Rect { omask_invert, .. }
            | Shape::Ellipse { omask_invert, .. }
            | Shape::Line { omask_invert, .. }
            | Shape::Path { omask_invert, .. }
            | Shape::Compound { omask_invert, .. }
            | Shape::Text { omask_invert, .. } => *omask_invert,
        }
    }

    /// Set whether this shape's opacity mask is inverted.
    pub fn set_omask_invert(&mut self, v: bool) {
        match self {
            Shape::Rect { omask_invert, .. }
            | Shape::Ellipse { omask_invert, .. }
            | Shape::Line { omask_invert, .. }
            | Shape::Path { omask_invert, .. }
            | Shape::Compound { omask_invert, .. }
            | Shape::Text { omask_invert, .. } => *omask_invert = v,
        }
    }

    /// Clear all opacity-mask tags (id + mask flag + invert), releasing the shape
    /// from any opacity mask. Used by `Object ▸ Opacity Mask ▸ Release`.
    pub fn clear_omask(&mut self) {
        self.set_omask(None);
        self.set_omask_path(false);
        self.set_omask_invert(false);
    }

    /// The shape's blend-set id, if it belongs to one. Shapes sharing an id form
    /// one blend run (the two ends plus generated intermediate steps).
    pub fn blend(&self) -> Option<u64> {
        match self {
            Shape::Rect { blend, .. }
            | Shape::Ellipse { blend, .. }
            | Shape::Line { blend, .. }
            | Shape::Path { blend, .. }
            | Shape::Compound { blend, .. }
            | Shape::Text { blend, .. } => *blend,
        }
    }

    /// Set (or clear, with `None`) the shape's blend-set membership.
    pub fn set_blend(&mut self, b: Option<u64>) {
        match self {
            Shape::Rect { blend, .. }
            | Shape::Ellipse { blend, .. }
            | Shape::Line { blend, .. }
            | Shape::Path { blend, .. }
            | Shape::Compound { blend, .. }
            | Shape::Text { blend, .. } => *blend = b,
        }
    }

    /// Whether this shape is a generated intermediate *step* of its blend set (as
    /// opposed to one of the two original ends). Release deletes steps, keeps ends.
    pub fn is_blend_step(&self) -> bool {
        match self {
            Shape::Rect { blend_step, .. }
            | Shape::Ellipse { blend_step, .. }
            | Shape::Line { blend_step, .. }
            | Shape::Path { blend_step, .. }
            | Shape::Compound { blend_step, .. }
            | Shape::Text { blend_step, .. } => *blend_step,
        }
    }

    /// Flag (or unflag) this shape as a generated blend step.
    pub fn set_blend_step(&mut self, s: bool) {
        match self {
            Shape::Rect { blend_step, .. }
            | Shape::Ellipse { blend_step, .. }
            | Shape::Line { blend_step, .. }
            | Shape::Path { blend_step, .. }
            | Shape::Compound { blend_step, .. }
            | Shape::Text { blend_step, .. } => *blend_step = s,
        }
    }

    /// Clear both blend tags (id + step flag), releasing the shape from any blend
    /// set. Used by `Object ▸ Blend ▸ Release` on the surviving ends.
    pub fn clear_blend(&mut self) {
        self.set_blend(None);
        self.set_blend_step(false);
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
            Shape::Compound { subpaths, .. } => {
                // The outline polygon is the compound's *outer* ring — the
                // largest-area closed sub-contour — so boolean ops / clip masks
                // that consume a single ring treat the compound by its outer
                // boundary. (Hit-testing / rendering use every sub-contour under
                // the fill rule where holes matter.)
                let outer = subpaths
                    .iter()
                    .filter(|s| s.closed)
                    .map(|s| (s.signed_area().abs(), s))
                    .max_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal))
                    .map(|(_, s)| s)?;
                outer.flatten()
            }
            Shape::Text { glyphs, .. } => {
                // Like a compound: the outer ring is the largest-area glyph
                // contour, so a boolean / clip op against text uses its overall
                // silhouette's biggest piece.
                let outer = glyphs
                    .iter()
                    .filter(|s| s.closed)
                    .map(|s| (s.signed_area().abs(), s))
                    .max_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal))
                    .map(|(_, s)| s)?;
                outer.flatten()
            }
            Shape::Line { .. } => return None,
        };
        (pts.len() >= 3).then_some(pts)
    }

    /// The editable text parameters, if this is a text object.
    pub fn text_params(&self) -> Option<&TextParams> {
        match self {
            Shape::Text { params, .. } => Some(params),
            _ => None,
        }
    }

    /// Replace this text object's parameters and re-lay-out its glyph cache from
    /// `params` + `origin`. No-op (returns `false`) on a non-text shape. Editing
    /// the string / size / alignment routes through here so the cached outlines
    /// always match the editable model.
    pub fn set_text_params(&mut self, new: TextParams) -> bool {
        if let Shape::Text {
            params,
            origin,
            glyphs,
            ..
        } = self
        {
            *params = new;
            *glyphs = crate::text::layout(params, *origin).0;
            true
        } else {
            false
        }
    }

    /// Re-run text layout into the glyph cache (after an origin change, or to
    /// repair a loaded document whose cache may be stale / absent). No-op on a
    /// non-text shape.
    pub fn text_relayout(&mut self) {
        if let Shape::Text {
            params,
            origin,
            glyphs,
            ..
        } = self
        {
            *glyphs = crate::text::layout(params, *origin).0;
        }
    }

    /// The live-shape parameters (polygon / star), if this path is a live shape.
    pub fn live_shape(&self) -> Option<LiveShape> {
        match self {
            Shape::Path { live, .. } => *live,
            _ => None,
        }
    }

    /// The centre about which a live shape regenerates: the centroid of its
    /// current anchor points (stable under translation, so a moved polygon stays
    /// put when an edit re-generates it). `None` if not a live path / no points.
    fn live_center(&self) -> Option<(f32, f32)> {
        if let Shape::Path {
            points,
            live: Some(_),
            ..
        } = self
        {
            if points.is_empty() {
                return None;
            }
            let n = points.len() as f32;
            let (sx, sy) = points
                .iter()
                .fold((0.0f32, 0.0f32), |(ax, ay), &(x, y)| (ax + x, ay + y));
            Some((sx / n, sy / n))
        } else {
            None
        }
    }

    /// Replace this path's live-shape parameters and regenerate its outline about
    /// the current centre. No-op (returns `false`) on a non-live path. Editing a
    /// count / radius / inner-ratio routes through here so the cached geometry
    /// always matches the editable parameters (mirrors [`set_text_params`]).
    ///
    /// [`set_text_params`]: Self::set_text_params
    pub fn set_live_shape(&mut self, new: LiveShape) -> bool {
        let Some(center) = self.live_center() else {
            return false;
        };
        if let Shape::Path {
            points,
            handles,
            closed,
            live,
            ..
        } = self
        {
            let (pts, hs) = new.outline(center);
            *points = pts;
            *handles = hs;
            *closed = true;
            *live = Some(new);
            true
        } else {
            false
        }
    }

    /// Drop the live-shape parameters (if any), demoting a polygon / star to a
    /// plain editable path. Called whenever an anchor / handle is edited directly,
    /// since the hand-edited geometry no longer matches the parameters (this is
    /// Illustrator's behaviour: reshaping a live shape's points expands it).
    fn drop_live(&mut self) {
        if let Shape::Path { live, .. } = self {
            *live = None;
        }
    }

    /// Lift a text object's cached glyph outlines into a real editable
    /// [`Shape::Compound`] (Illustrator's *Convert to Outlines*), inheriting the
    /// text's paint / style / membership and filling under the even-odd rule so
    /// glyph counters stay as holes. Returns the original shape unchanged if it is
    /// not a text object.
    pub fn text_to_outlines(&self) -> Shape {
        if let Shape::Text {
            params,
            origin,
            glyphs,
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
            omask,
            omask_path,
            omask_invert,
            blend,
            blend_step,
            name,
            locked,
            layer_color,
        } = self
        {
            // Prefer the live cache; fall back to a fresh layout if it is empty
            // (e.g. a hand-edited file) so convert never yields nothing for
            // non-empty text.
            let subpaths = if glyphs.is_empty() {
                crate::text::layout(params, *origin).0
            } else {
                glyphs.clone()
            };
            Shape::Compound {
                subpaths,
                fill_rule: FillRule::EvenOdd,
                fill: *fill,
                fill_gradient: fill_gradient.clone(),
                stroke: *stroke,
                stroke_w: *stroke_w,
                stroke_style: stroke_style.clone(),
                appearance: appearance.clone(),
                visible: *visible,
                group: *group,
                clip: *clip,
                mask: *mask,
                omask: *omask,
                omask_path: *omask_path,
                omask_invert: *omask_invert,
                blend: *blend,
                blend_step: *blend_step,
                name: name.clone(),
                locked: *locked,
                layer_color: *layer_color,
            }
        } else {
            self.clone()
        }
    }

    /// The compound path's [`FillRule`], if this is a compound (or text) shape.
    /// Text fills under **even-odd** so glyph counters (the hole in an "o") render
    /// as holes, matching the glyph-outline → compound conversion.
    pub fn fill_rule(&self) -> Option<FillRule> {
        match self {
            Shape::Compound { fill_rule, .. } => Some(*fill_rule),
            Shape::Text { .. } => Some(FillRule::EvenOdd),
            _ => None,
        }
    }

    /// The shape's stroke colour (straight sRGB RGBA). Every variant has a
    /// stroke, so this is always `Some` — the `Option` keeps the accessor shaped
    /// like [`fill_color`](Self::fill_color) for the appearance helpers.
    pub fn stroke_color(&self) -> Option<[f32; 4]> {
        match self {
            Shape::Rect { stroke, .. }
            | Shape::Ellipse { stroke, .. }
            | Shape::Line { stroke, .. }
            | Shape::Path { stroke, .. }
            | Shape::Compound { stroke, .. }
            | Shape::Text { stroke, .. } => Some(*stroke),
        }
    }

    /// Set the shape's stroke colour.
    pub fn set_stroke_color(&mut self, c: [f32; 4]) {
        match self {
            Shape::Rect { stroke, .. }
            | Shape::Ellipse { stroke, .. }
            | Shape::Line { stroke, .. }
            | Shape::Path { stroke, .. }
            | Shape::Compound { stroke, .. }
            | Shape::Text { stroke, .. } => *stroke = c,
        }
    }

    /// The shape's stroke width in document units.
    pub fn stroke_width(&self) -> f32 {
        match self {
            Shape::Rect { stroke_w, .. }
            | Shape::Ellipse { stroke_w, .. }
            | Shape::Line { stroke_w, .. }
            | Shape::Path { stroke_w, .. }
            | Shape::Compound { stroke_w, .. }
            | Shape::Text { stroke_w, .. } => *stroke_w,
        }
    }

    /// Set the shape's stroke width (document units).
    pub fn set_stroke_width(&mut self, w: f32) {
        match self {
            Shape::Rect { stroke_w, .. }
            | Shape::Ellipse { stroke_w, .. }
            | Shape::Line { stroke_w, .. }
            | Shape::Path { stroke_w, .. }
            | Shape::Compound { stroke_w, .. }
            | Shape::Text { stroke_w, .. } => *stroke_w = w,
        }
    }

    /// The shape's stroke attributes (caps/joins/dashes).
    pub fn stroke_style(&self) -> &StrokeStyle {
        match self {
            Shape::Rect { stroke_style, .. }
            | Shape::Ellipse { stroke_style, .. }
            | Shape::Line { stroke_style, .. }
            | Shape::Path { stroke_style, .. }
            | Shape::Compound { stroke_style, .. }
            | Shape::Text { stroke_style, .. } => stroke_style,
        }
    }

    /// Mutable access to the shape's stroke attributes.
    pub fn stroke_style_mut(&mut self) -> &mut StrokeStyle {
        match self {
            Shape::Rect { stroke_style, .. }
            | Shape::Ellipse { stroke_style, .. }
            | Shape::Line { stroke_style, .. }
            | Shape::Path { stroke_style, .. }
            | Shape::Compound { stroke_style, .. }
            | Shape::Text { stroke_style, .. } => stroke_style,
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
            | Shape::Path { appearance, .. }
            | Shape::Compound { appearance, .. }
            | Shape::Text { appearance, .. } => appearance.as_ref(),
        }
    }

    /// Mutable access to the shape's `appearance` slot (set/clear the stack).
    pub fn appearance_mut(&mut self) -> &mut Option<Appearance> {
        match self {
            Shape::Rect { appearance, .. }
            | Shape::Ellipse { appearance, .. }
            | Shape::Line { appearance, .. }
            | Shape::Path { appearance, .. }
            | Shape::Compound { appearance, .. }
            | Shape::Text { appearance, .. } => appearance,
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
            | Shape::Path { fill_gradient, .. }
            | Shape::Compound { fill_gradient, .. }
            | Shape::Text { fill_gradient, .. } => fill_gradient.as_ref(),
            Shape::Line { .. } => None,
        }
    }

    /// Set (or clear, with `None`) the shape's gradient fill. No-op on `Line`,
    /// which has no fill region.
    pub fn set_fill_gradient(&mut self, g: Option<Gradient>) {
        match self {
            Shape::Rect { fill_gradient, .. }
            | Shape::Ellipse { fill_gradient, .. }
            | Shape::Path { fill_gradient, .. }
            | Shape::Compound { fill_gradient, .. }
            | Shape::Text { fill_gradient, .. } => *fill_gradient = g,
            Shape::Line { .. } => {}
        }
    }

    /// The shape's solid fill colour, if it has a fill region (`Line` returns
    /// `None`). This is the colour used when there is no gradient, and the
    /// gradient's fallback.
    pub fn fill_color(&self) -> Option<[f32; 4]> {
        match self {
            Shape::Rect { fill, .. }
            | Shape::Ellipse { fill, .. }
            | Shape::Path { fill, .. }
            | Shape::Compound { fill, .. }
            | Shape::Text { fill, .. } => Some(*fill),
            Shape::Line { .. } => None,
        }
    }

    /// Set the shape's solid fill colour. No-op on `Line`.
    pub fn set_fill_color(&mut self, c: [f32; 4]) {
        match self {
            Shape::Rect { fill, .. }
            | Shape::Ellipse { fill, .. }
            | Shape::Path { fill, .. }
            | Shape::Compound { fill, .. }
            | Shape::Text { fill, .. } => *fill = c,
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
            | Shape::Path { fill_gradient, .. }
            | Shape::Compound { fill_gradient, .. }
            | Shape::Text { fill_gradient, .. } => fill_gradient
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
            Shape::Compound { subpaths, .. } | Shape::Text { glyphs: subpaths, .. } => {
                // Union of every sub-contour's tight (bezier-aware) bounds. Text
                // shares this: its glyph cache is just a list of sub-contours.
                let mut union: Option<kurbo::Rect> = None;
                for sp in subpaths {
                    if sp.points.is_empty() {
                        continue;
                    }
                    let r = path::bez_path(&sp.points, &sp.handles, sp.closed).bounding_box();
                    union = Some(match union {
                        Some(u) => u.union(r),
                        None => r,
                    });
                }
                union.map(|r| {
                    CoreRect::new(
                        r.x0 as f32,
                        r.y0 as f32,
                        r.width() as f32,
                        r.height() as f32,
                    )
                })
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
            Shape::Compound { subpaths, .. } => {
                for sp in subpaths.iter_mut() {
                    for p in sp.points.iter_mut() {
                        p.0 += dx;
                        p.1 += dy;
                    }
                }
            }
            Shape::Text {
                origin, glyphs, ..
            } => {
                // Move the editable anchor *and* the cached glyph outlines so the
                // text stays live (a later relayout keeps producing it in place).
                origin.0 += dx;
                origin.1 += dy;
                for sp in glyphs.iter_mut() {
                    for p in sp.points.iter_mut() {
                        p.0 += dx;
                        p.1 += dy;
                    }
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
            Shape::Compound { subpaths, .. } => {
                // Transform every sub-contour in place (anchors by the full
                // matrix, handle offsets by its linear part). A compound path has
                // no axis-aligned fast path — it is already an editable path.
                for sp in subpaths.iter_mut() {
                    for p in sp.points.iter_mut() {
                        *p = m.apply_point(p.0, p.1);
                    }
                    for h in sp.handles.iter_mut() {
                        *h = m.apply_vector(h.0, h.1);
                    }
                }
            }
            Shape::Text { .. } => {
                // A general affine (scale / rotate / shear / reflect) on live text
                // would desync the editable params from the transformed glyph
                // cache, so — like Illustrator baking transformed type — convert to
                // glyph outlines (a Compound) and transform those. The text stops
                // being editable as text, but renders / exports exactly.
                *self = self.text_to_outlines();
                self.apply_affine(m);
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
            // The outline replaces the geometry, so the result is a plain path
            // (a clipped polygon / star is no longer parametric).
            live: None,
            visible: self.visible(),
            group: self.group(),
            clip: None,
            mask: false,
            // Carry opacity-mask tags through so a clipped, opacity-masked shape
            // keeps its mask after clipping.
            omask: self.omask(),
            omask_path: self.is_omask(),
            omask_invert: self.omask_invert(),
            // Carry blend tags through so a clipped blend member stays in its set.
            blend: self.blend(),
            blend_step: self.is_blend_step(),
            // Carry the Layers-panel metadata through so a clipped shape keeps
            // its name / lock / colour.
            name: self.name().map(str::to_string),
            locked: self.locked(),
            layer_color: self.layer_color(),
        }
    }

    /// Convert this shape into an equivalent [`Shape::Path`], preserving paint
    /// style. `Rect` becomes a four-corner closed corner-path; `Ellipse` becomes
    /// a four-anchor closed cubic approximation; `Line` becomes a two-point open
    /// path; an existing `Path` is returned unchanged.
    pub fn to_path(&self) -> Shape {
        match self {
            // A compound path is already an editable path object; there is no
            // single-ring `Path` it reduces to without losing its holes, so it is
            // returned unchanged (transform / Pathfinder handle it as a compound).
            Shape::Path { .. } | Shape::Compound { .. } => self.clone(),
            // Text reduces to its glyph outlines (a compound path), the editable
            // form for Pathfinder / direct-select.
            Shape::Text { .. } => self.text_to_outlines(),
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
                omask,
                omask_path,
                omask_invert,
                blend,
                blend_step,
                name,
                locked,
                layer_color,
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
                    live: None,
                    visible: *visible,
                    group: *group,
                    clip: *clip,
                    mask: *mask,
                    omask: *omask,
                    omask_path: *omask_path,
                    omask_invert: *omask_invert,
                    blend: *blend,
                    blend_step: *blend_step,
                    name: name.clone(),
                    locked: *locked,
                    layer_color: *layer_color,
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
                omask,
                omask_path,
                omask_invert,
                blend,
                blend_step,
                name,
                locked,
                layer_color,
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
                    live: None,
                    visible: *visible,
                    group: *group,
                    clip: *clip,
                    mask: *mask,
                    omask: *omask,
                    omask_path: *omask_path,
                    omask_invert: *omask_invert,
                    blend: *blend,
                    blend_step: *blend_step,
                    name: name.clone(),
                    locked: *locked,
                    layer_color: *layer_color,
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
                omask,
                omask_path,
                omask_invert,
                blend,
                blend_step,
                name,
                locked,
                layer_color,
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
                live: None,
                visible: *visible,
                group: *group,
                clip: *clip,
                mask: *mask,
                omask: *omask,
                omask_path: *omask_path,
                omask_invert: *omask_invert,
                blend: *blend,
                blend_step: *blend_step,
                name: name.clone(),
                locked: *locked,
                layer_color: *layer_color,
            },
        }
    }

    /// Insert an anchor into this path at segment `seg`, parameter `t`,
    /// preserving shape. No-op (returns `None`) on non-`Path` shapes.
    pub fn insert_anchor(&mut self, seg: usize, t: f32) -> Option<usize> {
        self.drop_live();
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
        self.drop_live();
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
        self.drop_live();
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

    // --- Direct-select sub-contour access -----------------------------------
    //
    // A `Path` is one contour; a `Compound` is several. The Direct-Select tool
    // edits anchors/handles uniformly across both by addressing a `(contour,
    // anchor)` pair, so these accessors expose the `(points, handles, closed)`
    // triple per contour index. `Rect`/`Ellipse`/`Line` are not directly
    // editable (the tool converts them to paths first), so they expose none.

    /// Number of editable sub-contours: 1 for a `Path`, the sub-path count for a
    /// `Compound`, 0 for anything else.
    pub fn contour_count(&self) -> usize {
        match self {
            Shape::Path { .. } => 1,
            Shape::Compound { subpaths, .. } => subpaths.len(),
            _ => 0,
        }
    }

    /// Read-only `(points, handles, closed)` of sub-contour `c`, if it exists.
    pub fn contour(&self, c: usize) -> Option<ContourRef<'_>> {
        match self {
            Shape::Path {
                points,
                handles,
                closed,
                ..
            } if c == 0 => Some((points, handles, *closed)),
            Shape::Compound { subpaths, .. } => subpaths
                .get(c)
                .map(|sp| (sp.points.as_slice(), sp.handles.as_slice(), sp.closed)),
            _ => None,
        }
    }

    /// Mutable `(points, handles, closed)` of sub-contour `c`, if it exists.
    /// `handles` is resized to match `points` first so callers can index it.
    pub fn contour_mut(&mut self, c: usize) -> Option<ContourMut<'_>> {
        match self {
            Shape::Path {
                points,
                handles,
                closed,
                ..
            } if c == 0 => {
                if handles.len() < points.len() {
                    handles.resize(points.len(), (0.0, 0.0));
                }
                Some((points, handles, *closed))
            }
            Shape::Compound { subpaths, .. } => subpaths.get_mut(c).map(|sp| {
                if sp.handles.len() < sp.points.len() {
                    sp.handles.resize(sp.points.len(), (0.0, 0.0));
                }
                let closed = sp.closed;
                (&mut sp.points, &mut sp.handles, closed)
            }),
            _ => None,
        }
    }

    /// Move anchor `a` of sub-contour `c` to `(x, y)`. Returns `true` on success.
    pub fn set_anchor(&mut self, c: usize, a: usize, x: f32, y: f32) -> bool {
        self.drop_live();
        if let Some((points, _, _)) = self.contour_mut(c) {
            if let Some(p) = points.get_mut(a) {
                *p = (x, y);
                return true;
            }
        }
        false
    }

    /// Set the out-tangent handle of anchor `a` of sub-contour `c` so its out-knob
    /// sits at `(x, y)` (the in-knob mirrors). Returns `true` on success.
    pub fn set_handle(&mut self, c: usize, a: usize, x: f32, y: f32) -> bool {
        self.drop_live();
        if let Some((points, handles, _)) = self.contour_mut(c) {
            if let (Some(&(ax, ay)), Some(h)) = (points.get(a), handles.get_mut(a)) {
                *h = (x - ax, y - ay);
                return true;
            }
        }
        false
    }

    /// Insert an anchor into sub-contour `c` at segment `seg`, parameter `t`.
    pub fn insert_anchor_in(&mut self, c: usize, seg: usize, t: f32) -> Option<usize> {
        let closed = self.contour(c)?.2;
        self.drop_live();
        if let Some((points, handles, _)) = self.contour_mut(c) {
            path::insert_anchor(points, handles, closed, seg, t)
        } else {
            None
        }
    }

    /// Delete anchor `a` from sub-contour `c` (keeps ≥2 points). `true` on remove.
    pub fn delete_anchor_in(&mut self, c: usize, a: usize) -> bool {
        self.drop_live();
        if let Some((points, handles, _)) = self.contour_mut(c) {
            path::delete_anchor(points, handles, a)
        } else {
            false
        }
    }

    /// Toggle anchor `a` of sub-contour `c` smooth↔corner. Returns the new smooth
    /// state (`true` = now smooth). A smooth anchor carries mirrored tangent
    /// handles; a corner carries none (its segments are straight unless the
    /// neighbouring anchor still curves its side).
    pub fn toggle_anchor_smooth_in(&mut self, c: usize, a: usize) -> bool {
        let (closed, was_corner) = match self.contour(c) {
            Some((_, handles, closed)) => (closed, path::is_corner(handles, a)),
            None => return false,
        };
        self.drop_live();
        if let Some((points, handles, _)) = self.contour_mut(c) {
            if was_corner {
                // Corner → smooth needs the neighbour points; snapshot them so the
                // immutable read and the mutable handle write don't alias.
                let pts = points.clone();
                path::make_smooth(&pts, handles, closed, a)
            } else {
                let n = points.len();
                !path::make_corner(handles, n, a)
            }
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
            Shape::Compound {
                subpaths,
                fill_rule,
                ..
            } => {
                // Fill hit-test against all sub-contours under the compound's fill
                // rule (so a click in a hole misses, a click on solid area hits),
                // then the stroke edges so the boundary stays clickable.
                let rings: Vec<Vec<(f32, f32)>> = subpaths
                    .iter()
                    .filter(|s| s.closed)
                    .map(|s| s.flatten())
                    .collect();
                if path::point_in_rings(x, y, &rings, *fill_rule) {
                    return true;
                }
                for sp in subpaths {
                    let flat = sp.flatten();
                    let n = flat.len();
                    if n < 2 {
                        continue;
                    }
                    let last = if sp.closed { n } else { n - 1 };
                    for i in 0..last {
                        if path::dist_to_segment(x, y, flat[i], flat[(i + 1) % n]) <= tol.max(2.0) {
                            return true;
                        }
                    }
                }
                false
            }
            Shape::Text { .. } => {
                // A text object is selected by clicking anywhere inside its
                // bounding box (the way a type object's bounds pick in
                // Illustrator), which is far friendlier than requiring a click on a
                // thin glyph stroke.
                self.bounds()
                    .map(|b| path::point_in_rect(x, y, &[b.x, b.y, b.w, b.h], tol))
                    .unwrap_or(false)
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
    /// The document's named-appearance library (the Graphic Styles panel): each
    /// entry captures an [`Appearance`] snapshot a user can apply to a selection.
    /// Additive (`#[serde(default)]`), so a pre-styles `.contour` file loads with
    /// an empty library; saved styles round-trip through serde.
    #[serde(default)]
    pub graphic_styles: GraphicStyles,
}

impl Default for Document {
    fn default() -> Self {
        Self {
            shapes: Vec::new(),
            guides: Vec::new(),
            artboards: default_artboards(),
            active_artboard: 0,
            swatches: Swatches::default(),
            graphic_styles: GraphicStyles::default(),
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

    /// Re-lay-out every text object's glyph cache from its `params` + `origin`,
    /// so a loaded document (whose cached `glyphs` may be absent / stale / from a
    /// different font build) always renders its text correctly. Cheap (only text
    /// objects do work) and idempotent. Called after deserialize.
    pub fn relayout_text(&mut self) {
        for s in self.shapes.iter_mut() {
            s.text_relayout();
        }
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
            // An opacity-mask *path* paints nothing on its own: it only supplies
            // luminance to its set's content (applied at raster time by the
            // renderers via [`opacity_mask_of`]).
            if shape.is_omask() {
                continue;
            }
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

    /// The resolved **opacity mask** for the content shape at `index`, if it
    /// belongs to an opacity-mask set as content (not the mask path itself):
    /// returns the set's luminance-mask shape (cloned) plus its invert flag. The
    /// renderers rasterize this mask's luminance and multiply it into the content
    /// shape's alpha ([`crate::effects::apply_luminance_mask`]). `None` for an
    /// unmasked shape, the mask path itself, or a dangling set.
    pub fn opacity_mask_of(&self, index: usize) -> Option<(Shape, bool)> {
        let s = self.shapes.get(index)?;
        let set = s.omask()?;
        if s.is_omask() {
            return None; // the mask path is not itself masked
        }
        let mask = self
            .shapes
            .iter()
            .find(|o| o.omask() == Some(set) && o.is_omask())?;
        Some((mask.clone(), s.omask_invert()))
    }
}
