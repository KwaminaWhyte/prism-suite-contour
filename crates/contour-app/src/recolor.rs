//! **Recolor Artwork** — the pure colour-remapping engine behind Contour's
//! Recolor Artwork dialog (Illustrator's *Edit ▸ Edit Colors ▸ Recolor
//! Artwork*).
//!
//! The flow is: from the selected artwork, **extract** its set of used colours
//! (the *current palette*), let the user build a **new palette** of the same
//! length — by hand, by **reducing to N colours** (clustering the artwork's
//! colours down to N representatives), or from a **harmony rule** generated off
//! a base hue — and then **recolor**, which pairs each old colour with its new
//! one and applies the remap to the artwork.
//!
//! Everything here is pure and unit-tested. The app layer ([`crate::app`]) calls
//! [`extract_palette`] over the selection, drives [`reduce_to_n`] / [`harmony`]
//! to compute a new palette, and applies the result through the existing
//! [`Document::remap_color`](crate::document::Document::remap_color) path — the
//! same machinery a **global swatch** edit uses — so a recolour is one undo step
//! and remaps fills, strokes, gradient stops and appearance-stack paints alike.
//!
//! ## Colour space
//!
//! Clustering and nearest-colour assignment measure distance in a cheap
//! *perceptual-ish* space: straight-sRGB RGB scaled by the luminance weights
//! `(0.299, 0.587, 0.114)` (Rec. 601), so green differences count more than blue
//! ones, the way the eye sees them. Alpha is carried along but not weighted into
//! the distance (two colours that differ only in alpha are still "close"); a
//! cluster's representative averages its members' alpha. This avoids pulling in
//! a full CIELAB dependency while being markedly better than raw RGB Euclidean.

use crate::document::Shape;
use crate::swatches::colors_eq;

/// Rec. 601 luminance weights used to bias the RGB distance metric toward
/// perceived brightness differences.
const LUMA: [f32; 3] = [0.299, 0.587, 0.114];

/// Squared perceptual-ish distance between two straight-sRGB RGBA colours.
/// Luminance-weighted over RGB; alpha is ignored (see module docs).
fn dist2(a: [f32; 4], b: [f32; 4]) -> f32 {
    (0..3)
        .map(|c| {
            let d = (a[c] - b[c]) * LUMA[c];
            d * d
        })
        .sum()
}

/// Push `color` onto `out` unless an equal colour (within the swatch picker
/// tolerance) is already present. Keeps first-seen order — the basis for
/// deterministic extraction.
fn push_unique(out: &mut Vec<[f32; 4]>, color: [f32; 4]) {
    if !out.iter().any(|c| colors_eq(*c, color)) {
        out.push(color);
    }
}

/// Append every colour a single shape paints with — solid fill, stroke,
/// gradient-fill stops, and any appearance-stack solid/gradient paints — to
/// `out`, de-duplicated with the swatch picker tolerance. Mirrors the colour
/// coverage of [`Shape::remap_color`](crate::document::Shape::remap_color) so
/// extraction and remapping see exactly the same set.
fn collect_shape_colors(shape: &Shape, out: &mut Vec<[f32; 4]>) {
    if let Some(c) = shape.fill_color() {
        push_unique(out, c);
    }
    if let Some(c) = shape.stroke_color() {
        push_unique(out, c);
    }
    if let Some(g) = shape.fill_gradient() {
        for stop in &g.stops {
            push_unique(out, stop.color);
        }
    }
    if let Some(ap) = shape.appearance() {
        use crate::appearance::Paint;
        let mut handle = |p: &Paint| match p {
            Paint::Solid(c) => push_unique(out, *c),
            Paint::Gradient(g) => {
                for stop in &g.stops {
                    push_unique(out, stop.color);
                }
            }
        };
        for f in &ap.fills {
            handle(&f.paint);
        }
        for s in &ap.strokes {
            handle(&s.paint);
        }
    }
}

/// The set of distinct colours `shapes` paint with, in first-seen order
/// (fill, then stroke, then gradient stops, then appearance paints, shape by
/// shape). De-duplicated with the swatch picker-rounding tolerance, so colours
/// that read as equal in the UI collapse to one palette entry. Deterministic.
pub fn extract_palette(shapes: &[Shape]) -> Vec<[f32; 4]> {
    let mut out = Vec::new();
    for s in shapes {
        collect_shape_colors(s, &mut out);
    }
    out
}

/// The result of [`reduce_to_n`]: the `clusters` (the new, reduced palette, one
/// representative colour per cluster) and a `mapping` of the same length as the
/// input palette, where `mapping[i]` is the index into `clusters` that input
/// colour `i` was assigned to. So `clusters[mapping[i]]` is the colour that the
/// `i`-th original colour becomes.
#[derive(Clone, Debug, PartialEq)]
pub struct Reduction {
    /// The reduced palette: one representative colour per cluster.
    pub clusters: Vec<[f32; 4]>,
    /// `mapping[i]` = cluster index assigned to input palette colour `i`.
    pub mapping: Vec<usize>,
}

/// Index of the colour in `centers` nearest `color` under the perceptual metric.
/// `centers` must be non-empty.
fn nearest(color: [f32; 4], centers: &[[f32; 4]]) -> usize {
    let mut best = 0;
    let mut best_d = f32::INFINITY;
    for (i, c) in centers.iter().enumerate() {
        let d = dist2(color, *c);
        if d < best_d {
            best_d = d;
            best = i;
        }
    }
    best
}

/// Cluster `palette` down to (at most) `n` representative colours and report, for
/// each input colour, which cluster it falls into.
///
/// Uses **k-means** in the perceptual-ish space (see module docs), with a
/// **deterministic, seedless** initialisation — *farthest-point* seeding: the
/// first centre is the first palette colour, and each next centre is the palette
/// colour farthest (max-min distance) from the chosen ones. That makes the whole
/// reduction reproducible with no RNG. Iteration is run to convergence (or a
/// small fixed cap); empty clusters keep their seed so the output always has the
/// requested count when `n <= palette.len()`.
///
/// Degenerate cases: `n == 0` or an empty palette yields no clusters and an
/// empty mapping; `n >= palette.len()` returns the palette unchanged (identity
/// mapping), since there is nothing to merge.
pub fn reduce_to_n(palette: &[[f32; 4]], n: usize) -> Reduction {
    if n == 0 || palette.is_empty() {
        return Reduction {
            clusters: Vec::new(),
            mapping: Vec::new(),
        };
    }
    if n >= palette.len() {
        return Reduction {
            clusters: palette.to_vec(),
            mapping: (0..palette.len()).collect(),
        };
    }

    // --- Farthest-point seeding (deterministic) ----------------------------
    let mut centers: Vec<[f32; 4]> = Vec::with_capacity(n);
    centers.push(palette[0]);
    while centers.len() < n {
        // Pick the palette colour with the largest distance to its nearest
        // existing centre; ties resolve to the lowest index (stable).
        let mut best_i = 0;
        let mut best_d = -1.0f32;
        for (i, c) in palette.iter().enumerate() {
            let d = centers
                .iter()
                .map(|k| dist2(*c, *k))
                .fold(f32::INFINITY, f32::min);
            if d > best_d {
                best_d = d;
                best_i = i;
            }
        }
        centers.push(palette[best_i]);
    }

    // --- Lloyd iterations --------------------------------------------------
    let mut mapping = vec![0usize; palette.len()];
    for _ in 0..32 {
        // Assignment step.
        let mut changed = false;
        for (i, c) in palette.iter().enumerate() {
            let a = nearest(*c, &centers);
            if a != mapping[i] {
                mapping[i] = a;
                changed = true;
            }
        }
        // Update step: each centre = mean of its members (RGBA averaged). An
        // empty cluster keeps its previous centre (its seed), so the count
        // never collapses.
        let mut sums = vec![[0.0f32; 4]; centers.len()];
        let mut counts = vec![0usize; centers.len()];
        for (i, c) in palette.iter().enumerate() {
            let k = mapping[i];
            for ch in 0..4 {
                sums[k][ch] += c[ch];
            }
            counts[k] += 1;
        }
        for k in 0..centers.len() {
            if counts[k] > 0 {
                for ch in 0..4 {
                    centers[k][ch] = sums[k][ch] / counts[k] as f32;
                }
            }
        }
        if !changed {
            break;
        }
    }

    // Final assignment against the converged centres (so mapping matches the
    // returned clusters exactly).
    for (i, c) in palette.iter().enumerate() {
        mapping[i] = nearest(*c, &centers);
    }

    Reduction {
        clusters: centers,
        mapping,
    }
}

/// A colour-harmony rule generated off a base hue, à la the Recolor dialog's
/// harmony presets.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Harmony {
    /// The base colour plus its hue-opposite (180°). 2 colours.
    #[default]
    Complementary,
    /// Three colours: the base and its two neighbours ±30° around the wheel.
    Analogous,
    /// Three colours evenly spaced 120° apart (the base and two others).
    Triadic,
    /// Four colours: two complementary pairs, 90° apart (a rectangle on the
    /// wheel).
    Tetradic,
}

impl Harmony {
    /// All rules, for a UI selector.
    pub const ALL: [Harmony; 4] = [
        Harmony::Complementary,
        Harmony::Analogous,
        Harmony::Triadic,
        Harmony::Tetradic,
    ];

    /// Short display label.
    pub fn label(self) -> &'static str {
        match self {
            Harmony::Complementary => "Complementary",
            Harmony::Analogous => "Analogous",
            Harmony::Triadic => "Triadic",
            Harmony::Tetradic => "Tetradic",
        }
    }

    /// Hue offsets (degrees) this rule adds to the base hue, including the base's
    /// own `0.0`. The length of this is the number of colours the rule yields.
    fn offsets(self) -> &'static [f32] {
        match self {
            Harmony::Complementary => &[0.0, 180.0],
            Harmony::Analogous => &[0.0, -30.0, 30.0],
            Harmony::Triadic => &[0.0, 120.0, 240.0],
            Harmony::Tetradic => &[0.0, 90.0, 180.0, 270.0],
        }
    }

    /// How many colours the rule generates.
    pub fn count(self) -> usize {
        self.offsets().len()
    }
}

/// Convert straight-sRGB RGB to HSL. Returns `(hue_degrees, sat, light)` with
/// hue in `[0, 360)` and sat/light in `[0, 1]`.
fn rgb_to_hsl(rgb: [f32; 3]) -> (f32, f32, f32) {
    let (r, g, b) = (rgb[0], rgb[1], rgb[2]);
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let l = (max + min) * 0.5;
    let d = max - min;
    if d.abs() < f32::EPSILON {
        return (0.0, 0.0, l); // achromatic
    }
    let s = if l > 0.5 {
        d / (2.0 - max - min)
    } else {
        d / (max + min)
    };
    let h = if max == r {
        ((g - b) / d).rem_euclid(6.0)
    } else if max == g {
        (b - r) / d + 2.0
    } else {
        (r - g) / d + 4.0
    };
    ((h * 60.0).rem_euclid(360.0), s, l)
}

/// Convert HSL (hue degrees, sat, light) back to straight-sRGB RGB.
fn hsl_to_rgb(h: f32, s: f32, l: f32) -> [f32; 3] {
    if s.abs() < f32::EPSILON {
        return [l, l, l]; // achromatic
    }
    let q = if l < 0.5 {
        l * (1.0 + s)
    } else {
        l + s - l * s
    };
    let p = 2.0 * l - q;
    let hk = h.rem_euclid(360.0) / 360.0;
    let hue = |mut t: f32| {
        if t < 0.0 {
            t += 1.0;
        }
        if t > 1.0 {
            t -= 1.0;
        }
        if t < 1.0 / 6.0 {
            p + (q - p) * 6.0 * t
        } else if t < 0.5 {
            q
        } else if t < 2.0 / 3.0 {
            p + (q - p) * (2.0 / 3.0 - t) * 6.0
        } else {
            p
        }
    };
    [hue(hk + 1.0 / 3.0), hue(hk), hue(hk - 1.0 / 3.0)]
}

/// Generate a harmony palette from a `base` colour by rotating its hue per the
/// rule, keeping the base's saturation, lightness and alpha. The first entry is
/// always the base colour itself. Deterministic; the returned length equals
/// [`Harmony::count`].
pub fn harmony(base: [f32; 4], rule: Harmony) -> Vec<[f32; 4]> {
    let (h, s, l) = rgb_to_hsl([base[0], base[1], base[2]]);
    rule.offsets()
        .iter()
        .map(|off| {
            let rgb = hsl_to_rgb(h + off, s, l);
            [rgb[0], rgb[1], rgb[2], base[3]]
        })
        .collect()
}

/// Pair each old colour with its new one, dropping pairs that are equal (within
/// the picker tolerance) — those are no-ops the remap would skip anyway. Returns
/// `(old, new)` pairs ready to feed to
/// [`Document::remap_color`](crate::document::Document::remap_color).
///
/// `old_palette` and `new_palette` are matched by index; if they differ in
/// length the extra tail of the longer one is ignored (an unmatched old colour
/// keeps its value). This is the pure description of a recolour; applying it to
/// the document is the app layer's job (one undo checkpoint, then one
/// `remap_color` per pair).
pub fn recolor(
    old_palette: &[[f32; 4]],
    new_palette: &[[f32; 4]],
) -> Vec<([f32; 4], [f32; 4])> {
    old_palette
        .iter()
        .zip(new_palette.iter())
        .filter(|(o, n)| !colors_eq(**o, **n))
        .map(|(o, n)| (*o, *n))
        .collect()
}

/// Transient state for the Recolor Artwork dialog, held on the app while the
/// dialog is open. Captures the artwork's `current` palette once (at open) and a
/// mutable `new_palette` the user edits, reduces, or fills from a harmony rule;
/// the two are matched by index when [`recolor`] is applied.
#[derive(Clone, Debug, Default)]
pub struct RecolorState {
    /// Whether the dialog window is shown.
    pub open: bool,
    /// The artwork's extracted palette, captured when the dialog opened. Fixed
    /// while the dialog is open (it is the *old* side of every remap pair).
    pub current: Vec<[f32; 4]>,
    /// The user's working palette, same length as `current` and matched by
    /// index. Edited directly, or rewritten by reduce-to-N / a harmony rule.
    pub new_palette: Vec<[f32; 4]>,
    /// Target colour count for reduce-to-N (clamped to the palette length).
    pub reduce_n: usize,
    /// The harmony rule the harmony button generates from the base colour.
    pub harmony_rule: Harmony,
}

impl RecolorState {
    /// Open the dialog over `palette` (the selection's extracted colours),
    /// seeding the working palette as a copy of it (identity = no-op until the
    /// user edits). A reasonable default reduce target is half the colours.
    pub fn open_with(&mut self, palette: Vec<[f32; 4]>) {
        self.reduce_n = (palette.len().max(1) / 2).max(1).min(palette.len().max(1));
        self.new_palette = palette.clone();
        self.current = palette;
        self.open = true;
    }

    /// Replace the working palette by reducing the *current* artwork palette to
    /// `reduce_n` clusters and assigning each original colour its cluster's
    /// representative — so `new_palette[i]` is the reduced colour for
    /// `current[i]`, keeping the index-matched pairing [`recolor`] expects.
    pub fn apply_reduce(&mut self) {
        let n = self.reduce_n.clamp(1, self.current.len().max(1));
        let r = reduce_to_n(&self.current, n);
        self.new_palette = self
            .current
            .iter()
            .enumerate()
            .map(|(i, _)| r.clusters[r.mapping[i]])
            .collect();
    }

    /// Replace the working palette from the selected harmony rule, generated off
    /// the first current colour as the base. The harmony's colours are tiled
    /// across the palette (cycled if the artwork has more colours than the rule
    /// produces), so every original colour gets a new one and the index pairing
    /// is preserved.
    pub fn apply_harmony(&mut self) {
        let base = self.current.first().copied().unwrap_or([0.5, 0.5, 0.5, 1.0]);
        let h = harmony(base, self.harmony_rule);
        if h.is_empty() {
            return;
        }
        self.new_palette = (0..self.current.len()).map(|i| h[i % h.len()]).collect();
    }

    /// The `(old, new)` remap pairs this dialog would apply (no-op pairs
    /// dropped). Empty when the working palette equals the current one.
    pub fn pairs(&self) -> Vec<([f32; 4], [f32; 4])> {
        recolor(&self.current, &self.new_palette)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::Shape;

    /// A solid-fill / solid-stroke rectangle, the simplest coloured shape.
    fn rect(fill: [f32; 4], stroke: [f32; 4]) -> Shape {
        Shape::Rect {
            rect: [0.0, 0.0, 10.0, 10.0],
            fill,
            fill_gradient: None,
            stroke,
            stroke_w: 1.0,
            stroke_style: Default::default(),
            appearance: None,
            visible: true,
            group: None,
            clip: None,
            mask: false,
            omask: None,
            omask_path: false,
            omask_invert: false,
            blend: None,
            blend_step: false,
            name: None,
            locked: false,
            layer_color: None,
        }
    }

    const RED: [f32; 4] = [1.0, 0.0, 0.0, 1.0];
    const GREEN: [f32; 4] = [0.0, 1.0, 0.0, 1.0];
    const BLUE: [f32; 4] = [0.0, 0.0, 1.0, 1.0];
    const BLACK: [f32; 4] = [0.0, 0.0, 0.0, 1.0];
    const WHITE: [f32; 4] = [1.0, 1.0, 1.0, 1.0];

    #[test]
    fn extract_returns_distinct_used_colors_in_order() {
        let shapes = vec![rect(RED, BLACK), rect(GREEN, BLACK), rect(RED, WHITE)];
        let p = extract_palette(&shapes);
        // RED, BLACK (shape 0); GREEN (shape 1, BLACK already seen);
        // (RED already seen), WHITE (shape 2). Order = first-seen.
        assert_eq!(p, vec![RED, BLACK, GREEN, WHITE]);
    }

    #[test]
    fn extract_dedups_with_picker_tolerance() {
        // A colour off by less than half a u8 channel is the same palette entry.
        let near_red = [1.0 - 1.0 / 1024.0, 0.0, 0.0, 1.0];
        let shapes = vec![rect(RED, BLACK), rect(near_red, BLACK)];
        let p = extract_palette(&shapes);
        assert_eq!(p, vec![RED, BLACK], "near-equal red collapses to one");
    }

    #[test]
    fn extract_covers_gradient_stops() {
        use crate::gradient::{Gradient, GradientStop};
        let mut s = rect(RED, BLACK);
        if let Shape::Rect { fill_gradient, .. } = &mut s {
            *fill_gradient = Some(Gradient {
                stops: vec![GradientStop::new(0.0, GREEN), GradientStop::new(1.0, BLUE)],
                ..Default::default()
            });
        }
        let p = extract_palette(&[s]);
        assert!(p.contains(&GREEN) && p.contains(&BLUE));
    }

    #[test]
    fn reduce_to_2_yields_2_clusters_and_maps_every_color() {
        // Two warm + three cool colours → expect them to split 2 ways.
        let palette = vec![
            [0.95, 0.10, 0.10, 1.0], // red
            [0.90, 0.30, 0.10, 1.0], // orange-red
            [0.10, 0.20, 0.90, 1.0], // blue
            [0.10, 0.40, 0.85, 1.0], // azure
            [0.15, 0.60, 0.80, 1.0], // teal-blue
        ];
        let r = reduce_to_n(&palette, 2);
        assert_eq!(r.clusters.len(), 2);
        assert_eq!(r.mapping.len(), palette.len());
        // Every input colour maps to a valid cluster index.
        assert!(r.mapping.iter().all(|&m| m < 2));
        // The two warm reds share a cluster; the three cool blues share one.
        assert_eq!(r.mapping[0], r.mapping[1], "reds cluster together");
        assert_eq!(r.mapping[2], r.mapping[3], "blues cluster together");
        assert_eq!(r.mapping[3], r.mapping[4], "blues cluster together");
        assert_ne!(r.mapping[0], r.mapping[2], "warm vs cool split");
    }

    #[test]
    fn reduce_is_deterministic() {
        let palette = vec![RED, GREEN, BLUE, BLACK, WHITE, [0.5, 0.5, 0.5, 1.0]];
        let a = reduce_to_n(&palette, 3);
        let b = reduce_to_n(&palette, 3);
        assert_eq!(a, b, "seedless k-means is reproducible");
    }

    #[test]
    fn reduce_n_ge_len_is_identity() {
        let palette = vec![RED, GREEN, BLUE];
        let r = reduce_to_n(&palette, 5);
        assert_eq!(r.clusters, palette);
        assert_eq!(r.mapping, vec![0, 1, 2]);
    }

    #[test]
    fn reduce_zero_or_empty_is_empty() {
        assert_eq!(reduce_to_n(&[RED, GREEN], 0).clusters.len(), 0);
        assert_eq!(reduce_to_n(&[], 3).clusters.len(), 0);
    }

    #[test]
    fn harmony_counts_and_base_first() {
        for rule in Harmony::ALL {
            let h = harmony(RED, rule);
            assert_eq!(h.len(), rule.count(), "{:?} count", rule);
            // First entry is the base colour (hue offset 0).
            assert!(colors_eq(h[0], RED), "{:?} keeps base first", rule);
        }
    }

    #[test]
    fn complementary_is_hue_opposite() {
        // Pure red (hue 0) → complement is cyan (hue 180): r low, g+b high.
        let h = harmony(RED, Harmony::Complementary);
        assert_eq!(h.len(), 2);
        let comp = h[1];
        assert!(comp[0] < 0.1, "complement of red has little red");
        assert!(comp[1] > 0.9 && comp[2] > 0.9, "complement of red is cyan");
        // Alpha is carried through.
        assert_eq!(comp[3], 1.0);
    }

    #[test]
    fn triadic_spaces_hues_120_apart() {
        // Red base → green-ish and blue-ish thirds (120° / 240°).
        let h = harmony(RED, Harmony::Triadic);
        assert_eq!(h.len(), 3);
        // 120°: dominant green; 240°: dominant blue.
        assert!(h[1][1] > h[1][0] && h[1][1] > h[1][2], "120° is green-dominant");
        assert!(h[2][2] > h[2][0] && h[2][2] > h[2][1], "240° is blue-dominant");
    }

    #[test]
    fn hsl_round_trips() {
        for rgb in [
            [1.0, 0.0, 0.0],
            [0.0, 1.0, 0.0],
            [0.0, 0.0, 1.0],
            [0.3, 0.6, 0.2],
            [0.7, 0.2, 0.9],
        ] {
            let (h, s, l) = rgb_to_hsl(rgb);
            let back = hsl_to_rgb(h, s, l);
            for ch in 0..3 {
                assert!((rgb[ch] - back[ch]).abs() < 1e-4, "round-trip {rgb:?}");
            }
        }
    }

    #[test]
    fn recolor_pairs_skip_noops() {
        let old = vec![RED, GREEN, BLUE];
        // GREEN unchanged → dropped; RED→BLACK and BLUE→WHITE kept.
        let new = vec![BLACK, GREEN, WHITE];
        let pairs = recolor(&old, &new);
        assert_eq!(pairs, vec![(RED, BLACK), (BLUE, WHITE)]);
    }

    #[test]
    fn recolor_identity_is_a_noop() {
        let old = vec![RED, GREEN, BLUE];
        assert!(recolor(&old, &old).is_empty(), "identity remap produces no pairs");
    }

    #[test]
    fn state_open_seeds_identity_palette() {
        let mut st = RecolorState::default();
        st.open_with(vec![RED, GREEN, BLUE, BLACK]);
        assert!(st.open);
        assert_eq!(st.new_palette, st.current);
        assert!(st.pairs().is_empty(), "fresh open is a no-op");
        assert!(st.reduce_n >= 1 && st.reduce_n <= 4);
    }

    #[test]
    fn state_apply_reduce_keeps_index_pairing() {
        let mut st = RecolorState::default();
        st.open_with(vec![RED, [0.9, 0.1, 0.1, 1.0], BLUE, [0.1, 0.1, 0.9, 1.0]]);
        st.reduce_n = 2;
        st.apply_reduce();
        assert_eq!(st.new_palette.len(), st.current.len());
        // The two reds now share one colour; the two blues another.
        assert!(colors_eq(st.new_palette[0], st.new_palette[1]));
        assert!(colors_eq(st.new_palette[2], st.new_palette[3]));
        assert!(!colors_eq(st.new_palette[0], st.new_palette[2]));
    }

    #[test]
    fn state_apply_harmony_fills_palette() {
        let mut st = RecolorState::default();
        st.open_with(vec![RED, GREEN, BLUE]);
        st.harmony_rule = Harmony::Complementary;
        st.apply_harmony();
        assert_eq!(st.new_palette.len(), 3);
        // Base-first, then the complement, then cycled back to base.
        assert!(colors_eq(st.new_palette[0], RED));
    }

    #[test]
    fn recolor_applied_remaps_only_bound_shapes() {
        // Build a tiny doc, extract, reduce-to-1, apply, and confirm only the
        // artwork's colours move (and an unrelated colour is untouched).
        let mut doc = crate::document::Document::new();
        doc.shapes = vec![rect(RED, BLACK), rect(GREEN, BLACK)];
        let old = extract_palette(&doc.shapes);
        let red_unrelated_before = doc.shapes[0].stroke_color();
        assert!(red_unrelated_before.is_some());

        // Map every colour to a single grey.
        let new: Vec<[f32; 4]> = old.iter().map(|_| [0.5, 0.5, 0.5, 1.0]).collect();
        let pairs = recolor(&old, &new);
        let mut total = 0;
        for (o, n) in pairs {
            total += doc.remap_color(o, n);
        }
        assert!(total > 0, "some shapes were recoloured");
        // Every fill is now grey.
        assert!(colors_eq(doc.shapes[0].fill_color().unwrap(), [0.5, 0.5, 0.5, 1.0]));
        assert!(colors_eq(doc.shapes[1].fill_color().unwrap(), [0.5, 0.5, 0.5, 1.0]));
    }
}
