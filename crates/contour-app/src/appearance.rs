//! The **Appearance** stack: multiple fills and strokes per object, rendered
//! bottom-to-top (Illustrator's Appearance panel).
//!
//! Today a [`Shape`](crate::document::Shape) carries one solid-or-gradient fill
//! and one stroke in its own fields. The [`Appearance`] here generalises that to
//! an ordered `Vec` of [`Fill`]s and a `Vec` of [`Stroke`]s, each with its own
//! paint (solid colour or [`Gradient`]), per-item opacity, and (stored, not yet
//! composited) [`BlendMode`]. Painting walks the fills then the strokes from the
//! bottom of each list to the top, so later entries sit over earlier ones —
//! exactly like a layer stack.
//!
//! **Backward compatibility.** A shape stores `appearance: Option<Appearance>`
//! additively (`#[serde(default)]` → `None`). When it is `None` the shape renders
//! from its legacy single fill/stroke fields, so every pre-existing `.contour`
//! file loads and renders identically. [`Appearance::from_legacy`] migrates a
//! single fill/stroke into a one-element-each stack on demand (what the inspector
//! does the first time the user opens the Appearance section on an old shape).
//!
//! Everything here is pure and unit-tested; the canvas painter, PNG exporter and
//! SVG exporter all consume the same model so the three surfaces stay in lock-step.

use crate::document::StrokeStyle;
use crate::gradient::Gradient;
use serde::{Deserialize, Serialize};

/// A per-attribute blend mode. Stored on every [`Fill`] / [`Stroke`] so the
/// model is forward-compatible with compositing; **only [`BlendMode::Normal`] is
/// composited** by the current egui-painter / tiny-skia render paths (the others
/// round-trip through serde and the UI but render as Normal — see the crate-level
/// gap note). Kept app-local rather than reusing `prism-core`'s 18-mode enum to
/// keep the Appearance model self-contained for this pass.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum BlendMode {
    #[default]
    Normal,
    Multiply,
    Screen,
    Overlay,
    Darken,
    Lighten,
}

impl BlendMode {
    pub const ALL: [BlendMode; 6] = [
        BlendMode::Normal,
        BlendMode::Multiply,
        BlendMode::Screen,
        BlendMode::Overlay,
        BlendMode::Darken,
        BlendMode::Lighten,
    ];

    pub fn label(self) -> &'static str {
        match self {
            BlendMode::Normal => "Normal",
            BlendMode::Multiply => "Multiply",
            BlendMode::Screen => "Screen",
            BlendMode::Overlay => "Overlay",
            BlendMode::Darken => "Darken",
            BlendMode::Lighten => "Lighten",
        }
    }
}

/// What a fill or stroke paints with: a solid straight-sRGB RGBA colour, or a
/// multi-stop [`Gradient`].
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Paint {
    Solid([f32; 4]),
    Gradient(Gradient),
}

impl Default for Paint {
    fn default() -> Self {
        Paint::Solid([0.0, 0.0, 0.0, 1.0])
    }
}

impl Paint {
    /// The solid colour of this paint, or the first gradient stop's colour as a
    /// representative swatch (for the small colour chip in the UI).
    pub fn swatch(&self) -> [f32; 4] {
        match self {
            Paint::Solid(c) => *c,
            Paint::Gradient(g) => g.stops.first().map(|s| s.color).unwrap_or([0.0; 4]),
        }
    }

    /// The gradient, if this is a gradient paint.
    pub fn gradient(&self) -> Option<&Gradient> {
        match self {
            Paint::Gradient(g) => Some(g),
            Paint::Solid(_) => None,
        }
    }
}

/// One fill in the stack: a paint, a per-item opacity (`0..=1`), a blend mode,
/// and a visibility toggle.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Fill {
    pub paint: Paint,
    #[serde(default = "one")]
    pub opacity: f32,
    #[serde(default)]
    pub blend: BlendMode,
    #[serde(default = "default_true")]
    pub visible: bool,
}

impl Default for Fill {
    fn default() -> Self {
        Self {
            paint: Paint::default(),
            opacity: 1.0,
            blend: BlendMode::Normal,
            visible: true,
        }
    }
}

impl Fill {
    pub fn solid(color: [f32; 4]) -> Self {
        Self {
            paint: Paint::Solid(color),
            ..Self::default()
        }
    }
}

/// One stroke in the stack: a paint, a width (document units), a
/// [`StrokeStyle`] (caps/joins/dashes), a per-item opacity, a blend mode, and a
/// visibility toggle.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Stroke {
    pub paint: Paint,
    pub width: f32,
    #[serde(default)]
    pub style: StrokeStyle,
    #[serde(default = "one")]
    pub opacity: f32,
    #[serde(default)]
    pub blend: BlendMode,
    #[serde(default = "default_true")]
    pub visible: bool,
}

impl Default for Stroke {
    fn default() -> Self {
        Self {
            paint: Paint::Solid([0.0, 0.0, 0.0, 1.0]),
            width: 1.0,
            style: StrokeStyle::default(),
            opacity: 1.0,
            blend: BlendMode::Normal,
            visible: true,
        }
    }
}

impl Stroke {
    pub fn solid(color: [f32; 4], width: f32) -> Self {
        Self {
            paint: Paint::Solid(color),
            width,
            ..Self::default()
        }
    }
}

/// A non-destructive **live effect** applied to a shape's rasterized appearance.
///
/// Effects sit on top of the fill/stroke stack: the fills + strokes are
/// rasterized first (via `tiny-skia`, the same path PNG export uses), then each
/// effect transforms that raster in order, bottom-to-top. Because the effect
/// works on a *rendered* raster (not the path), an egui painter — which cannot
/// blur — can still show drop-shadows / blurs by compositing the processed
/// texture. The parameters here are pure data (no GPU / context), so the model
/// round-trips through serde and the inspector edits it like any other stack
/// item; the raster math lives in [`crate::effects`].
///
/// Only [`Effect::DropShadow`] and [`Effect::GaussianBlur`] ship this pass;
/// Transform / Outer Glow / distorts are deferred (see the crate gap notes).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Effect {
    /// A soft offset shadow drawn *behind* the artwork: the shape's alpha is
    /// tinted `color`, offset by `(dx, dy)` document units, blurred by `blur`
    /// (a Gaussian-equivalent radius in document units), scaled by `opacity`,
    /// and composited under the original artwork.
    DropShadow {
        dx: f32,
        dy: f32,
        blur: f32,
        /// Straight-sRGB RGBA shadow colour (alpha is the base shadow strength).
        color: [f32; 4],
        /// Extra `0..=1` multiplier on the shadow alpha.
        opacity: f32,
    },
    /// A Gaussian blur of the whole artwork by `radius` document units.
    GaussianBlur { radius: f32 },
}

impl Effect {
    /// A sensible default Drop Shadow (down-right soft black shadow).
    pub fn drop_shadow() -> Self {
        Effect::DropShadow {
            dx: 4.0,
            dy: 4.0,
            blur: 4.0,
            color: [0.0, 0.0, 0.0, 0.75],
            opacity: 1.0,
        }
    }

    /// A default Gaussian Blur.
    pub fn gaussian_blur() -> Self {
        Effect::GaussianBlur { radius: 4.0 }
    }

    /// Short label for the inspector / list rows.
    pub fn label(&self) -> &'static str {
        match self {
            Effect::DropShadow { .. } => "Drop Shadow",
            Effect::GaussianBlur { .. } => "Gaussian Blur",
        }
    }

    /// Whether this effect does any visible work (skippable when not).
    pub fn is_active(&self) -> bool {
        match self {
            Effect::DropShadow {
                blur,
                color,
                opacity,
                ..
            } => (color[3] * opacity) > 0.0 && *blur >= 0.0,
            Effect::GaussianBlur { radius } => *radius > 0.0,
        }
    }

    /// How far (document units) this effect can spill past the shape's tight
    /// bounds, so a rasterizer knows how much padding to add around the artwork
    /// before applying it. Drop-shadow padding covers the offset plus the blur
    /// reach; blur padding covers the blur reach. A generous `~3σ` (≈ `3×`
    /// radius) margin keeps the soft edge from clipping.
    pub fn bounds_pad(&self) -> f32 {
        match self {
            Effect::DropShadow { dx, dy, blur, .. } => {
                dx.abs().max(dy.abs()) + blur.abs() * 3.0
            }
            Effect::GaussianBlur { radius } => radius.abs() * 3.0,
        }
    }
}

/// An object's stacked appearance: fills (bottom-to-top), strokes (bottom-to-top)
/// painted over the geometry, then live [`Effect`]s applied to the rasterized
/// result (bottom-to-top).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Appearance {
    #[serde(default)]
    pub fills: Vec<Fill>,
    #[serde(default)]
    pub strokes: Vec<Stroke>,
    /// Live, non-destructive effects applied to the rendered fill/stroke raster
    /// (drop-shadow, blur, …). Additive (`#[serde(default)]` → empty) so every
    /// pre-existing `.contour` file loads with no effects.
    #[serde(default)]
    pub effects: Vec<Effect>,
}

impl Appearance {
    /// Build a one-fill / one-stroke stack from a shape's legacy single fields.
    ///
    /// `fill` / `gradient` describe the legacy fill (a `None` `fill` means the
    /// shape has no fill region — a line — so the stack gets no fill). A
    /// zero-width or fully-transparent legacy stroke produces no stroke entry, so
    /// migrating a fill-only shape doesn't invent a stroke. This is the bridge
    /// the inspector uses the first time a user edits an old shape's appearance,
    /// and what the renderers fall back through when `appearance` is `None`.
    pub fn from_legacy(
        fill: Option<[f32; 4]>,
        gradient: Option<&Gradient>,
        stroke: [f32; 4],
        stroke_w: f32,
        stroke_style: &StrokeStyle,
    ) -> Self {
        let mut fills = Vec::new();
        if let Some(c) = fill {
            let paint = match gradient {
                Some(g) => Paint::Gradient(g.clone()),
                None => Paint::Solid(c),
            };
            fills.push(Fill {
                paint,
                ..Fill::default()
            });
        }
        let mut strokes = Vec::new();
        if stroke_w > 0.0 && stroke[3] > 0.0 {
            strokes.push(Stroke {
                paint: Paint::Solid(stroke),
                width: stroke_w,
                style: stroke_style.clone(),
                ..Stroke::default()
            });
        }
        Self {
            fills,
            strokes,
            effects: Vec::new(),
        }
    }

    /// Whether the stack has nothing to paint (no visible fills or strokes).
    /// Effects alone can't paint (they transform a fill/stroke raster), so an
    /// effects-only stack is still "empty".
    pub fn is_empty(&self) -> bool {
        self.fills.is_empty() && self.strokes.is_empty()
    }

    /// Whether any active live effect is present (so the renderer takes the
    /// rasterize-and-composite path instead of the plain painter path).
    pub fn has_active_effects(&self) -> bool {
        self.effects.iter().any(Effect::is_active)
    }

    /// Total document-unit padding any effect needs around the artwork bounds
    /// (the max over all active effects' [`Effect::bounds_pad`]). `0.0` when
    /// there are no effects.
    pub fn effect_pad(&self) -> f32 {
        self.effects
            .iter()
            .filter(|e| e.is_active())
            .map(Effect::bounds_pad)
            .fold(0.0, f32::max)
    }

    // --- Reorder / stack editing (pure; the inspector drives these) ----------

    /// Move fill `i` one step up the stack (towards the top / end of the list).
    /// No-op at the top. Returns `true` if it moved.
    pub fn raise_fill(&mut self, i: usize) -> bool {
        move_up(&mut self.fills, i)
    }

    /// Move fill `i` one step down the stack (towards the bottom / start).
    pub fn lower_fill(&mut self, i: usize) -> bool {
        move_down(&mut self.fills, i)
    }

    /// Move stroke `i` one step up the stack.
    pub fn raise_stroke(&mut self, i: usize) -> bool {
        move_up(&mut self.strokes, i)
    }

    /// Move stroke `i` one step down the stack.
    pub fn lower_stroke(&mut self, i: usize) -> bool {
        move_down(&mut self.strokes, i)
    }

    /// Move effect `i` one step up the stack (applied later).
    pub fn raise_effect(&mut self, i: usize) -> bool {
        move_up(&mut self.effects, i)
    }

    /// Move effect `i` one step down the stack (applied earlier).
    pub fn lower_effect(&mut self, i: usize) -> bool {
        move_down(&mut self.effects, i)
    }
}

/// Swap element `i` with `i + 1` (towards the end). `true` if it moved.
fn move_up<T>(v: &mut [T], i: usize) -> bool {
    if i + 1 < v.len() {
        v.swap(i, i + 1);
        true
    } else {
        false
    }
}

/// Swap element `i` with `i - 1` (towards the start). `true` if it moved.
fn move_down<T>(v: &mut [T], i: usize) -> bool {
    if i > 0 && i < v.len() {
        v.swap(i, i - 1);
        true
    } else {
        false
    }
}

fn one() -> f32 {
    1.0
}

fn default_true() -> bool {
    true
}

/// Premultiply a paint colour's alpha by a per-item `opacity` (`0..=1`). Used so
/// a fill/stroke's opacity slider scales its alpha on every render surface.
pub fn apply_opacity(mut c: [f32; 4], opacity: f32) -> [f32; 4] {
    c[3] = (c[3] * opacity).clamp(0.0, 1.0);
    c
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gradient::{Gradient, GradientKind};

    #[test]
    fn from_legacy_solid_fill_and_stroke() {
        let a = Appearance::from_legacy(
            Some([1.0, 0.0, 0.0, 1.0]),
            None,
            [0.0, 0.0, 0.0, 1.0],
            2.0,
            &StrokeStyle::default(),
        );
        assert_eq!(a.fills.len(), 1, "one fill from a single legacy fill");
        assert_eq!(a.strokes.len(), 1, "one stroke from a single legacy stroke");
        assert_eq!(a.fills[0].paint.swatch(), [1.0, 0.0, 0.0, 1.0]);
        assert_eq!(a.strokes[0].width, 2.0);
        assert_eq!(a.fills[0].opacity, 1.0);
    }

    #[test]
    fn from_legacy_gradient_fill() {
        let g = Gradient::two_stop(
            GradientKind::Radial,
            [1.0, 0.0, 0.0, 1.0],
            [0.0, 0.0, 1.0, 1.0],
        );
        let a = Appearance::from_legacy(
            Some([0.5, 0.5, 0.5, 1.0]),
            Some(&g),
            [0.0, 0.0, 0.0, 1.0],
            1.0,
            &StrokeStyle::default(),
        );
        assert_eq!(a.fills.len(), 1);
        assert_eq!(a.fills[0].paint.gradient(), Some(&g));
    }

    #[test]
    fn from_legacy_no_fill_no_zero_stroke() {
        // A line: no fill region, and a zero-width stroke makes no stroke entry.
        let a = Appearance::from_legacy(None, None, [0.0, 0.0, 0.0, 1.0], 0.0, &StrokeStyle::default());
        assert!(a.fills.is_empty());
        assert!(a.strokes.is_empty());
        assert!(a.is_empty());
    }

    #[test]
    fn from_legacy_transparent_stroke_dropped() {
        // A positive width but fully-transparent stroke colour paints nothing, so
        // migration drops it (matches the renderers, which skip alpha-0 strokes).
        let a = Appearance::from_legacy(
            Some([1.0, 1.0, 1.0, 1.0]),
            None,
            [0.0, 0.0, 0.0, 0.0],
            3.0,
            &StrokeStyle::default(),
        );
        assert_eq!(a.fills.len(), 1);
        assert!(a.strokes.is_empty());
    }

    #[test]
    fn reorder_fills_moves_one_step() {
        let mut a = Appearance {
            fills: vec![
                Fill::solid([0.0, 0.0, 0.0, 1.0]), // index 0 (bottom)
                Fill::solid([1.0, 0.0, 0.0, 1.0]), // index 1
                Fill::solid([0.0, 1.0, 0.0, 1.0]), // index 2 (top)
            ],
            strokes: vec![],
            effects: vec![],
        };
        // Raise index 0 (towards top) → swaps with 1.
        assert!(a.raise_fill(0));
        assert_eq!(a.fills[0].paint.swatch(), [1.0, 0.0, 0.0, 1.0]);
        assert_eq!(a.fills[1].paint.swatch(), [0.0, 0.0, 0.0, 1.0]);
        // Lower index 2 (towards bottom) → swaps with 1.
        assert!(a.lower_fill(2));
        assert_eq!(a.fills[2].paint.swatch(), [0.0, 0.0, 0.0, 1.0]);
        // Edges are no-ops.
        assert!(!a.raise_fill(2));
        assert!(!a.lower_fill(0));
    }

    #[test]
    fn reorder_strokes_moves_one_step() {
        let mut a = Appearance {
            fills: vec![],
            strokes: vec![
                Stroke::solid([0.0, 0.0, 0.0, 1.0], 1.0),
                Stroke::solid([1.0, 1.0, 1.0, 1.0], 4.0),
            ],
            effects: vec![],
        };
        assert!(a.raise_stroke(0));
        assert_eq!(a.strokes[0].width, 4.0);
        assert_eq!(a.strokes[1].width, 1.0);
        assert!(!a.raise_stroke(1));
    }

    #[test]
    fn serde_round_trip_preserves_stack() {
        let a = Appearance {
            fills: vec![
                Fill::solid([0.1, 0.2, 0.3, 1.0]),
                Fill {
                    paint: Paint::Gradient(Gradient::default()),
                    opacity: 0.5,
                    blend: BlendMode::Multiply,
                    visible: false,
                },
            ],
            strokes: vec![Stroke {
                paint: Paint::Solid([1.0, 0.0, 0.0, 0.8]),
                width: 3.5,
                style: StrokeStyle::default(),
                opacity: 0.75,
                blend: BlendMode::Screen,
                visible: true,
            }],
            effects: vec![
                Effect::drop_shadow(),
                Effect::GaussianBlur { radius: 6.5 },
            ],
        };
        let json = serde_json::to_string(&a).unwrap();
        let back: Appearance = serde_json::from_str(&json).unwrap();
        assert_eq!(a, back);
    }

    #[test]
    fn serde_defaults_fill_in_missing_item_fields() {
        // A minimal fill JSON (only `paint`) must default opacity/blend/visible,
        // so a hand-written or older stack still loads.
        let json = r#"{"fills":[{"paint":{"Solid":[1,0,0,1]}}],"strokes":[]}"#;
        let a: Appearance = serde_json::from_str(json).unwrap();
        assert_eq!(a.fills.len(), 1);
        assert_eq!(a.fills[0].opacity, 1.0);
        assert_eq!(a.fills[0].blend, BlendMode::Normal);
        assert!(a.fills[0].visible);
    }

    #[test]
    fn apply_opacity_scales_alpha() {
        let c = apply_opacity([1.0, 0.5, 0.0, 0.8], 0.5);
        assert!((c[3] - 0.4).abs() < 1e-6);
        // RGB untouched.
        assert_eq!([c[0], c[1], c[2]], [1.0, 0.5, 0.0]);
    }

    // --- Effects ------------------------------------------------------------

    #[test]
    fn effect_serde_round_trip() {
        let fx = vec![
            Effect::DropShadow {
                dx: 5.0,
                dy: -3.0,
                blur: 8.0,
                color: [0.1, 0.2, 0.3, 0.6],
                opacity: 0.9,
            },
            Effect::GaussianBlur { radius: 12.5 },
        ];
        let json = serde_json::to_string(&fx).unwrap();
        let back: Vec<Effect> = serde_json::from_str(&json).unwrap();
        assert_eq!(fx, back);
    }

    /// An old document's appearance JSON (no `effects` key) loads with an empty
    /// effects vec — back-compat for every pre-effects `.contour` file.
    #[test]
    fn effects_default_empty_on_old_docs() {
        let json = r#"{"fills":[{"paint":{"Solid":[1,0,0,1]}}],"strokes":[]}"#;
        let a: Appearance = serde_json::from_str(json).unwrap();
        assert!(a.effects.is_empty(), "missing effects key defaults to empty");
        assert!(!a.has_active_effects());
        assert_eq!(a.effect_pad(), 0.0);
    }

    #[test]
    fn full_appearance_with_effects_round_trips() {
        let a = Appearance {
            fills: vec![Fill::solid([1.0, 0.0, 0.0, 1.0])],
            strokes: vec![],
            effects: vec![Effect::drop_shadow(), Effect::gaussian_blur()],
        };
        let json = serde_json::to_string(&a).unwrap();
        let back: Appearance = serde_json::from_str(&json).unwrap();
        assert_eq!(a, back);
        assert!(back.has_active_effects());
    }

    #[test]
    fn effect_is_active_logic() {
        assert!(Effect::drop_shadow().is_active());
        assert!(Effect::GaussianBlur { radius: 1.0 }.is_active());
        // A zero-radius blur and a fully-transparent / zero-opacity shadow are
        // no-ops and must be skipped by the renderer.
        assert!(!Effect::GaussianBlur { radius: 0.0 }.is_active());
        assert!(!Effect::DropShadow {
            dx: 4.0,
            dy: 4.0,
            blur: 4.0,
            color: [0.0, 0.0, 0.0, 0.0],
            opacity: 1.0,
        }
        .is_active());
        assert!(!Effect::DropShadow {
            dx: 4.0,
            dy: 4.0,
            blur: 4.0,
            color: [0.0, 0.0, 0.0, 1.0],
            opacity: 0.0,
        }
        .is_active());
    }

    #[test]
    fn effect_bounds_pad_covers_offset_and_blur() {
        // Shadow padding = max(|dx|,|dy|) + 3·blur.
        let s = Effect::DropShadow {
            dx: 10.0,
            dy: -2.0,
            blur: 4.0,
            color: [0.0, 0.0, 0.0, 1.0],
            opacity: 1.0,
        };
        assert!((s.bounds_pad() - (10.0 + 12.0)).abs() < 1e-6);
        // Blur padding = 3·radius.
        assert!((Effect::GaussianBlur { radius: 5.0 }.bounds_pad() - 15.0).abs() < 1e-6);
    }

    #[test]
    fn effect_pad_is_max_over_active_effects() {
        let a = Appearance {
            fills: vec![Fill::solid([0.0; 4])],
            strokes: vec![],
            effects: vec![
                Effect::GaussianBlur { radius: 2.0 }, // pad 6
                Effect::DropShadow {
                    dx: 20.0,
                    dy: 0.0,
                    blur: 0.0,
                    color: [0.0, 0.0, 0.0, 1.0],
                    opacity: 1.0,
                }, // pad 20
                Effect::GaussianBlur { radius: 0.0 }, // inactive → ignored
            ],
        };
        assert_eq!(a.effect_pad(), 20.0);
    }

    #[test]
    fn reorder_effects_moves_one_step() {
        let mut a = Appearance {
            fills: vec![],
            strokes: vec![],
            effects: vec![
                Effect::GaussianBlur { radius: 1.0 },
                Effect::GaussianBlur { radius: 2.0 },
                Effect::GaussianBlur { radius: 3.0 },
            ],
        };
        assert!(a.raise_effect(0));
        assert_eq!(a.effects[0], Effect::GaussianBlur { radius: 2.0 });
        assert_eq!(a.effects[1], Effect::GaussianBlur { radius: 1.0 });
        assert!(a.lower_effect(2));
        assert_eq!(a.effects[2], Effect::GaussianBlur { radius: 1.0 });
        assert!(!a.raise_effect(2));
        assert!(!a.lower_effect(0));
    }
}
