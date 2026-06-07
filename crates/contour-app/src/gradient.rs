//! Multi-stop gradient fills (linear & radial).
//!
//! A [`Gradient`] is a resolution-independent description of a fill: an ordered
//! set of colour [`GradientStop`]s at parametric offsets `0..=1`, a
//! [`GradientKind`] (linear at an angle, or radial), and how it repeats past the
//! ends ([`SpreadMode`]). It is geometry-free — the renderer maps the `0..=1`
//! parameter onto a concrete shape via the shape's bounding box
//! ([`linear_endpoints`] / [`radial_params`]), exactly the way Illustrator's
//! gradient fills follow the object's bounds.
//!
//! Everything here is pure and unit-tested; the canvas painter, PNG exporter
//! (`tiny-skia`) and SVG exporter all consume the same [`Gradient`] so the three
//! surfaces stay in lock-step.

use prism_core::color::{linear_to_srgb, srgb_to_linear};
use serde::{Deserialize, Serialize};

/// One colour stop: a straight-sRGB RGBA colour at a parametric `offset` in
/// `0..=1` along the gradient.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct GradientStop {
    pub offset: f32,
    pub color: [f32; 4],
}

impl GradientStop {
    pub fn new(offset: f32, color: [f32; 4]) -> Self {
        Self {
            offset: offset.clamp(0.0, 1.0),
            color,
        }
    }
}

/// Linear (directional), radial (concentric), or angle (conic) gradient
/// geometry. These mirror the suite's shared [`prism_core::gradient::GradientType`]
/// subset that maps cleanly onto Illustrator's three primary gradient types.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum GradientKind {
    /// A directional gradient: stop 0 at one edge, stop 1 at the opposite edge,
    /// the line oriented at the gradient's `angle`.
    #[default]
    Linear,
    /// A concentric gradient radiating from the bounding-box centre outward.
    Radial,
    /// A conic (angular) gradient sweeping `angle`° around the box centre — stop 0
    /// at the start angle, stop 1 a full turn later (Illustrator's *Angle* type).
    Angle,
}

impl GradientKind {
    pub const ALL: [GradientKind; 3] =
        [GradientKind::Linear, GradientKind::Radial, GradientKind::Angle];

    pub fn label(self) -> &'static str {
        match self {
            GradientKind::Linear => "Linear",
            GradientKind::Radial => "Radial",
            GradientKind::Angle => "Angle",
        }
    }
}

/// How the gradient repeats outside the `0..=1` parameter range. Mirrors
/// tiny-skia's `SpreadMode` and SVG's `spreadMethod`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum SpreadMode {
    /// Clamp the end stops (the default; SVG `pad`).
    #[default]
    Pad,
    /// Repeat the pattern (SVG `repeat`).
    Repeat,
    /// Mirror every other repeat (SVG `reflect`).
    Reflect,
}

impl SpreadMode {
    pub const ALL: [SpreadMode; 3] = [SpreadMode::Pad, SpreadMode::Repeat, SpreadMode::Reflect];

    pub fn label(self) -> &'static str {
        match self {
            SpreadMode::Pad => "Pad",
            SpreadMode::Repeat => "Repeat",
            SpreadMode::Reflect => "Reflect",
        }
    }

    /// SVG `spreadMethod` keyword.
    pub fn svg(self) -> &'static str {
        match self {
            SpreadMode::Pad => "pad",
            SpreadMode::Repeat => "repeat",
            SpreadMode::Reflect => "reflect",
        }
    }
}

/// How colours are interpolated between stops. Mirrors the suite's linear-light
/// compositing philosophy: `Perceptual` blends in **linear light** (the suite's
/// working space — smoother, no muddy mid-tones, IL-2025 parity), `Srgb` blends
/// in raw straight-sRGB component space (the naive default, kept for fidelity
/// with older files and SVG `color-interpolation="sRGB"`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum Interpolation {
    /// Interpolate in linear light (perceptually smoother).
    #[default]
    Perceptual,
    /// Interpolate in straight-sRGB component space.
    Srgb,
}

impl Interpolation {
    pub const ALL: [Interpolation; 2] = [Interpolation::Perceptual, Interpolation::Srgb];

    pub fn label(self) -> &'static str {
        match self {
            Interpolation::Perceptual => "Perceptual",
            Interpolation::Srgb => "sRGB",
        }
    }
}

/// Serde default for [`Gradient::interpolation`]: older `.contour` files (which
/// predate the field) blended in straight sRGB, so they load as [`Srgb`] to keep
/// their appearance byte-identical, while *new* gradients default to `Perceptual`.
fn legacy_interpolation() -> Interpolation {
    Interpolation::Srgb
}

/// A multi-stop gradient fill. Stored on a shape (additively) as an override of
/// its solid `fill` colour.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Gradient {
    pub kind: GradientKind,
    /// Colour stops in authoring order (not necessarily sorted by offset;
    /// [`sorted_stops`] returns the render order).
    pub stops: Vec<GradientStop>,
    /// Linear/angle gradient direction, in degrees clockwise from the +x axis
    /// (0° = left→right, 90° = top→bottom). Ignored for radial gradients.
    pub angle: f32,
    pub spread: SpreadMode,
    /// Colour-space the stops are blended in. Additive
    /// (`#[serde(default = "legacy_interpolation")]` → `Srgb` for old files).
    #[serde(default = "legacy_interpolation")]
    pub interpolation: Interpolation,
    /// Apply ordered (Bayer) dithering on raster export to kill 8-bit banding.
    /// Additive (`#[serde(default)]` → `false` for old files, which never
    /// dithered).
    #[serde(default)]
    pub dither: bool,
}

impl Default for Gradient {
    fn default() -> Self {
        // A black→white left-to-right linear gradient, like Illustrator's
        // default swatch — perceptual + dithered, matching the suite's shared
        // gradient defaults (`prism_core::gradient::Gradient::default`).
        Self {
            kind: GradientKind::Linear,
            stops: vec![
                GradientStop::new(0.0, [0.0, 0.0, 0.0, 1.0]),
                GradientStop::new(1.0, [1.0, 1.0, 1.0, 1.0]),
            ],
            angle: 0.0,
            spread: SpreadMode::Pad,
            interpolation: Interpolation::Perceptual,
            dither: true,
        }
    }
}

impl Gradient {
    /// Build a two-stop gradient between `a` (offset 0) and `b` (offset 1).
    pub fn two_stop(kind: GradientKind, a: [f32; 4], b: [f32; 4]) -> Self {
        Self {
            kind,
            stops: vec![GradientStop::new(0.0, a), GradientStop::new(1.0, b)],
            angle: 0.0,
            spread: SpreadMode::default(),
            interpolation: Interpolation::default(),
            dither: true,
        }
    }

    /// The stops sorted by ascending offset (the order renderers need). A stable
    /// sort keeps the authoring order of stops that share an offset.
    pub fn sorted_stops(&self) -> Vec<GradientStop> {
        let mut s = self.stops.clone();
        s.sort_by(|a, b| {
            a.offset
                .partial_cmp(&b.offset)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        s
    }

    /// Wrap a raw parameter `t` into `0..=1` according to the spread mode. Used
    /// by [`color_at`] and by any sampler that needs the effective parameter.
    pub fn wrap(&self, t: f32) -> f32 {
        spread_param(t, self.spread)
    }

    /// The interpolated colour at parameter `t` (any real value; the spread mode
    /// folds it into range first). Linear interpolation in straight sRGB between
    /// the two bracketing sorted stops. An empty gradient is transparent; a
    /// single stop is constant.
    pub fn color_at(&self, t: f32) -> [f32; 4] {
        let stops = self.sorted_stops();
        match stops.len() {
            0 => [0.0, 0.0, 0.0, 0.0],
            1 => stops[0].color,
            _ => {
                let u = self.wrap(t);
                // Clamp below the first / above the last stop.
                if u <= stops[0].offset {
                    return stops[0].color;
                }
                let last = stops.len() - 1;
                if u >= stops[last].offset {
                    return stops[last].color;
                }
                // Find the bracketing pair.
                for w in stops.windows(2) {
                    let (a, b) = (w[0], w[1]);
                    if u >= a.offset && u <= b.offset {
                        let span = b.offset - a.offset;
                        let f = if span <= f32::EPSILON {
                            0.0
                        } else {
                            (u - a.offset) / span
                        };
                        return blend_color(a.color, b.color, f, self.interpolation);
                    }
                }
                stops[last].color
            }
        }
    }
}

/// Fold a raw gradient parameter into `0..=1` per the spread mode.
fn spread_param(t: f32, mode: SpreadMode) -> f32 {
    match mode {
        SpreadMode::Pad => t.clamp(0.0, 1.0),
        SpreadMode::Repeat => t.rem_euclid(1.0),
        SpreadMode::Reflect => {
            // Triangle wave with period 2: 0→1→0.
            let m = t.rem_euclid(2.0);
            if m <= 1.0 {
                m
            } else {
                2.0 - m
            }
        }
    }
}

/// Component-wise linear interpolation between two straight-sRGB RGBA colours.
pub fn lerp_color(a: [f32; 4], b: [f32; 4], t: f32) -> [f32; 4] {
    let t = t.clamp(0.0, 1.0);
    [
        a[0] + (b[0] - a[0]) * t,
        a[1] + (b[1] - a[1]) * t,
        a[2] + (b[2] - a[2]) * t,
        a[3] + (b[3] - a[3]) * t,
    ]
}

/// Blend two straight-sRGB RGBA colours by `t`, in the requested colour space.
///
/// `Srgb` blends the raw components (the naive default). `Perceptual` decodes RGB
/// to **linear light**, blends there, then re-encodes — the suite's working space,
/// which avoids the dark muddy mid-tones of sRGB blending (e.g. red→green no
/// longer dips through murky brown). Alpha is always linear in `t` (it is already
/// a linear quantity).
pub fn blend_color(a: [f32; 4], b: [f32; 4], t: f32, interp: Interpolation) -> [f32; 4] {
    let t = t.clamp(0.0, 1.0);
    let alpha = a[3] + (b[3] - a[3]) * t;
    match interp {
        Interpolation::Srgb => lerp_color(a, b, t),
        Interpolation::Perceptual => {
            let mut out = [0.0f32; 4];
            for c in 0..3 {
                let la = srgb_to_linear(a[c]);
                let lb = srgb_to_linear(b[c]);
                out[c] = linear_to_srgb(la + (lb - la) * t);
            }
            out[3] = alpha;
            out
        }
    }
}

/// Expand this gradient's stops into a denser, **pre-interpolated** list suitable
/// for renderers that only interpolate linearly in sRGB component space
/// (tiny-skia, SVG `<stop>`s). For [`Interpolation::Srgb`] the original sorted
/// stops are returned unchanged (the renderer's own sRGB lerp is already exact).
/// For [`Interpolation::Perceptual`] each adjacent pair is subdivided into
/// `samples` straight-sRGB sub-stops computed in linear light, so the renderer's
/// linear-sRGB lerp reproduces the perceptual ramp to within the sampling
/// resolution. A two-stop perceptual ramp therefore becomes `samples`+1 stops.
impl Gradient {
    pub fn render_stops(&self, samples: usize) -> Vec<GradientStop> {
        let sorted = self.sorted_stops();
        if self.interpolation == Interpolation::Srgb || sorted.len() < 2 {
            return sorted;
        }
        let samples = samples.max(2);
        let mut out: Vec<GradientStop> = Vec::with_capacity(sorted.len() + samples);
        for pair in sorted.windows(2) {
            let (a, b) = (pair[0], pair[1]);
            // Emit `samples` points across [a.offset, b.offset]; the first point
            // is the segment start (a). The previous segment's end == this start,
            // so skip duplicates after the first segment.
            let start = if out.is_empty() { 0 } else { 1 };
            for s in start..=samples {
                let f = s as f32 / samples as f32;
                let off = a.offset + (b.offset - a.offset) * f;
                let col = blend_color(a.color, b.color, f, Interpolation::Perceptual);
                out.push(GradientStop::new(off, col));
            }
        }
        out
    }
}

/// The two document-space endpoints of a linear gradient over the axis-aligned
/// bounding box `[x, y, w, h]` at `angle` degrees (clockwise from +x).
///
/// The gradient line passes through the box centre; its endpoints are the two
/// points where the line at `angle` meets the box's projected extent, so the
/// full `0..=1` parameter range spans the box exactly along that direction (the
/// same convention Illustrator uses: a 0° gradient runs left→right edge, 90°
/// runs top→bottom edge).
pub fn linear_endpoints(bbox: &[f32; 4], angle: f32) -> ((f32, f32), (f32, f32)) {
    let (x, y, w, h) = (bbox[0], bbox[1], bbox[2], bbox[3]);
    let cx = x + w * 0.5;
    let cy = y + h * 0.5;
    let rad = angle.to_radians();
    let (dx, dy) = (rad.cos(), rad.sin());
    // Half-extent of the box projected onto the gradient direction: this is the
    // support function of the centred rectangle, giving endpoints that touch the
    // box edges along `angle`.
    let half = (w * 0.5) * dx.abs() + (h * 0.5) * dy.abs();
    (
        (cx - dx * half, cy - dy * half),
        (cx + dx * half, cy + dy * half),
    )
}

/// Map a document-space point to the conic (angle) gradient parameter `0..=1`
/// over the bounding box `bbox`, sweeping counter-clockwise from `angle`° (the
/// same 0°=+x, 90°=down convention as [`linear_endpoints`]). The centre is the
/// box centre. Pure; used by the canvas sampler and the angle-stop expansion that
/// the raster/SVG exporters fall back to (neither tiny-skia 0.11 nor SVG 1.1 has
/// a native conic shader).
pub fn angle_param(bbox: &[f32; 4], angle: f32, px: f32, py: f32) -> f32 {
    let cx = bbox[0] + bbox[2] * 0.5;
    let cy = bbox[1] + bbox[3] * 0.5;
    let base = angle.to_radians();
    // atan2 in screen space (y grows downward); subtract the start angle and wrap
    // to 0..1 going clockwise so 0°→ start, increasing angle sweeps the ramp.
    let a = ((py - cy).atan2(px - cx) - base).rem_euclid(std::f32::consts::TAU);
    a / std::f32::consts::TAU
}

/// Centre and radius of a radial gradient over the bounding box: centred on the
/// box, radius reaching the farthest corner so the box is fully covered.
pub fn radial_params(bbox: &[f32; 4]) -> ((f32, f32), f32) {
    let (x, y, w, h) = (bbox[0], bbox[1], bbox[2], bbox[3]);
    let cx = x + w * 0.5;
    let cy = y + h * 0.5;
    let r = ((w * 0.5).powi(2) + (h * 0.5).powi(2)).sqrt();
    ((cx, cy), r.max(1e-3))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn approx(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-4
    }

    fn approx_color(a: [f32; 4], b: [f32; 4]) -> bool {
        a.iter().zip(b).all(|(x, y)| (x - y).abs() < 1e-4)
    }

    #[test]
    fn default_is_black_to_white_linear() {
        let g = Gradient::default();
        assert_eq!(g.kind, GradientKind::Linear);
        assert_eq!(g.stops.len(), 2);
        // The default now blends perceptually + dithers, matching the shared
        // `prism_core::gradient::Gradient::default`.
        assert_eq!(g.interpolation, Interpolation::Perceptual);
        assert!(g.dither);
        assert!(approx_color(g.color_at(0.0), [0.0, 0.0, 0.0, 1.0]));
        assert!(approx_color(g.color_at(1.0), [1.0, 1.0, 1.0, 1.0]));
        // The black→white midpoint blends in linear light, so it encodes back to
        // sRGB ≈ 0.735 (brighter than the naive 0.5).
        let mid = g.color_at(0.5)[0];
        assert!((mid - linear_to_srgb(0.5)).abs() < 1e-3, "perceptual mid {mid}");
    }

    #[test]
    fn color_at_interpolates_between_stops() {
        // sRGB interpolation gives the plain component lerp checked below.
        let g = Gradient {
            kind: GradientKind::Linear,
            stops: vec![
                GradientStop::new(0.0, [1.0, 0.0, 0.0, 1.0]),
                GradientStop::new(0.5, [0.0, 1.0, 0.0, 1.0]),
                GradientStop::new(1.0, [0.0, 0.0, 1.0, 1.0]),
            ],
            angle: 0.0,
            spread: SpreadMode::Pad,
            interpolation: Interpolation::Srgb,
            dither: false,
        };
        // Exactly on the middle stop.
        assert!(approx_color(g.color_at(0.5), [0.0, 1.0, 0.0, 1.0]));
        // A quarter of the way: halfway between red and green.
        assert!(approx_color(g.color_at(0.25), [0.5, 0.5, 0.0, 1.0]));
        // Three-quarters: halfway between green and blue.
        assert!(approx_color(g.color_at(0.75), [0.0, 0.5, 0.5, 1.0]));
    }

    #[test]
    fn color_at_pads_outside_range() {
        let g = Gradient::two_stop(
            GradientKind::Linear,
            [1.0, 0.0, 0.0, 1.0],
            [0.0, 0.0, 1.0, 1.0],
        );
        assert!(approx_color(g.color_at(-3.0), [1.0, 0.0, 0.0, 1.0]));
        assert!(approx_color(g.color_at(7.0), [0.0, 0.0, 1.0, 1.0]));
    }

    #[test]
    fn sorted_stops_orders_by_offset() {
        let g = Gradient {
            kind: GradientKind::Linear,
            stops: vec![
                GradientStop::new(1.0, [0.0, 0.0, 1.0, 1.0]),
                GradientStop::new(0.0, [1.0, 0.0, 0.0, 1.0]),
                GradientStop::new(0.5, [0.0, 1.0, 0.0, 1.0]),
            ],
            angle: 0.0,
            spread: SpreadMode::Pad,
            interpolation: Interpolation::Srgb,
            dither: false,
        };
        let s = g.sorted_stops();
        assert!(approx(s[0].offset, 0.0));
        assert!(approx(s[1].offset, 0.5));
        assert!(approx(s[2].offset, 1.0));
        // Sampling is unaffected by authoring order.
        assert!(approx_color(g.color_at(0.0), [1.0, 0.0, 0.0, 1.0]));
    }

    #[test]
    fn spread_repeat_wraps() {
        assert!(approx(spread_param(1.25, SpreadMode::Repeat), 0.25));
        assert!(approx(spread_param(-0.25, SpreadMode::Repeat), 0.75));
    }

    #[test]
    fn spread_reflect_mirrors() {
        // 0..1 identity, 1..2 mirrors back to 1..0.
        assert!(approx(spread_param(0.3, SpreadMode::Reflect), 0.3));
        assert!(approx(spread_param(1.3, SpreadMode::Reflect), 0.7));
        assert!(approx(spread_param(2.3, SpreadMode::Reflect), 0.3));
    }

    #[test]
    fn spread_pad_clamps() {
        assert!(approx(spread_param(-2.0, SpreadMode::Pad), 0.0));
        assert!(approx(spread_param(5.0, SpreadMode::Pad), 1.0));
        assert!(approx(spread_param(0.4, SpreadMode::Pad), 0.4));
    }

    #[test]
    fn linear_endpoints_horizontal_span_box_width() {
        // 0°: endpoints on the left and right edges, at the vertical centre.
        let bbox = [10.0, 20.0, 100.0, 40.0];
        let (a, b) = linear_endpoints(&bbox, 0.0);
        assert!(approx(a.0, 10.0) && approx(a.1, 40.0));
        assert!(approx(b.0, 110.0) && approx(b.1, 40.0));
    }

    #[test]
    fn linear_endpoints_vertical_span_box_height() {
        // 90°: endpoints on the top and bottom edges, at the horizontal centre.
        let bbox = [10.0, 20.0, 100.0, 40.0];
        let (a, b) = linear_endpoints(&bbox, 90.0);
        assert!(approx(a.0, 60.0) && approx(a.1, 20.0));
        assert!(approx(b.0, 60.0) && approx(b.1, 60.0));
    }

    #[test]
    fn radial_params_centres_and_reaches_corner() {
        let bbox = [0.0, 0.0, 6.0, 8.0];
        let ((cx, cy), r) = radial_params(&bbox);
        assert!(approx(cx, 3.0) && approx(cy, 4.0));
        // Half-diagonal of a 6×8 box: sqrt(3² + 4²) = 5.
        assert!(approx(r, 5.0));
    }

    #[test]
    fn empty_gradient_is_transparent() {
        let g = Gradient {
            kind: GradientKind::Linear,
            stops: vec![],
            angle: 0.0,
            spread: SpreadMode::Pad,
            interpolation: Interpolation::Srgb,
            dither: false,
        };
        assert!(approx_color(g.color_at(0.5), [0.0, 0.0, 0.0, 0.0]));
    }

    // ---- perceptual (linear-light) interpolation ---------------------------

    #[test]
    fn srgb_interpolation_is_component_lerp() {
        // Black→white midpoint is exactly 0.5 in component space under sRGB blend.
        let g = Gradient {
            interpolation: Interpolation::Srgb,
            ..Gradient::two_stop(GradientKind::Linear, [0.0; 4], [1.0, 1.0, 1.0, 1.0])
        };
        assert!(approx(g.color_at(0.5)[0], 0.5));
    }

    #[test]
    fn perceptual_interpolation_blends_in_linear_light() {
        // Black→white: the linear-light midpoint encodes back to ~0.735 in sRGB
        // (sRGB encode of 0.5), distinctly brighter than the naive 0.5.
        let g = Gradient {
            interpolation: Interpolation::Perceptual,
            ..Gradient::two_stop(GradientKind::Linear, [0.0; 4], [1.0, 1.0, 1.0, 1.0])
        };
        let mid = g.color_at(0.5)[0];
        let expect = linear_to_srgb(0.5);
        assert!((mid - expect).abs() < 1e-3, "perceptual mid {mid} != {expect}");
        // Endpoints are exact in either space.
        assert!(approx(g.color_at(0.0)[0], 0.0));
        assert!(approx(g.color_at(1.0)[0], 1.0));
        // Alpha stays a plain linear lerp regardless of colour space.
        let g2 = Gradient {
            interpolation: Interpolation::Perceptual,
            ..Gradient::two_stop(GradientKind::Linear, [1.0, 0.0, 0.0, 1.0], [1.0, 0.0, 0.0, 0.0])
        };
        assert!(approx(g2.color_at(0.5)[3], 0.5));
    }

    #[test]
    fn render_stops_srgb_passthrough() {
        // sRGB gradients are not expanded — the renderer's own lerp is exact.
        let g = Gradient {
            interpolation: Interpolation::Srgb,
            ..Gradient::two_stop(GradientKind::Linear, [0.0; 4], [1.0; 4])
        };
        assert_eq!(g.render_stops(16).len(), 2);
    }

    #[test]
    fn render_stops_perceptual_expands_and_tracks_color_at() {
        // A perceptual two-stop ramp expands to `samples`+1 monotonic stops whose
        // colours match `color_at` sampled in linear light.
        let g = Gradient {
            interpolation: Interpolation::Perceptual,
            ..Gradient::two_stop(GradientKind::Linear, [0.0; 4], [1.0, 1.0, 1.0, 1.0])
        };
        let s = g.render_stops(8);
        assert_eq!(s.len(), 9, "expanded stop count");
        // Offsets strictly ascending and bracket the full range.
        assert!(approx(s[0].offset, 0.0) && approx(s[s.len() - 1].offset, 1.0));
        for w in s.windows(2) {
            assert!(w[1].offset >= w[0].offset);
        }
        // Each expanded stop's colour equals color_at at that offset.
        for st in &s {
            assert!(approx_color(st.color, g.color_at(st.offset)));
        }
    }

    #[test]
    fn render_stops_multi_segment_no_duplicate_seams() {
        // Three stops, perceptual: segment seams must not be duplicated.
        let g = Gradient {
            kind: GradientKind::Linear,
            stops: vec![
                GradientStop::new(0.0, [1.0, 0.0, 0.0, 1.0]),
                GradientStop::new(0.5, [0.0, 1.0, 0.0, 1.0]),
                GradientStop::new(1.0, [0.0, 0.0, 1.0, 1.0]),
            ],
            angle: 0.0,
            spread: SpreadMode::Pad,
            interpolation: Interpolation::Perceptual,
            dither: false,
        };
        // 2 segments × 4 samples, sharing the middle seam: 4 + 4 = 8 → 9 stops.
        let s = g.render_stops(4);
        assert_eq!(s.len(), 9);
    }

    // ---- angle (conic) geometry --------------------------------------------

    #[test]
    fn angle_param_sweeps_around_centre() {
        // 10×10 box centred at (5,5); 0° start. Points clockwise from +x.
        let bbox = [0.0, 0.0, 10.0, 10.0];
        // +x direction → t≈0 (or ≈1 at the wrap).
        let t0 = angle_param(&bbox, 0.0, 10.0, 5.0);
        assert!(!(0.02..=0.98).contains(&t0), "t0={t0}");
        // Straight down (+y screen) → quarter turn.
        assert!(approx(angle_param(&bbox, 0.0, 5.0, 10.0), 0.25));
        // -x → half turn.
        assert!(approx(angle_param(&bbox, 0.0, 0.0, 5.0), 0.5));
        // A non-zero start angle rotates the whole sweep: starting at 90° (down),
        // the straight-down point lands at the ramp start (t≈0).
        let s = angle_param(&bbox, 90.0, 5.0, 10.0);
        assert!(!(0.02..=0.98).contains(&s), "rotated start t={s}");
    }

    #[test]
    fn gradient_kind_all_has_angle() {
        assert!(GradientKind::ALL.contains(&GradientKind::Angle));
        assert_eq!(GradientKind::Angle.label(), "Angle");
    }
}
