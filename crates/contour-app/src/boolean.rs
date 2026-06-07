//! Boolean polygon operations — the **Pathfinder** — on two closed shapes,
//! powered by the `i_overlay` crate.
//!
//! Each input shape is flattened to a polygon (its outer ring) in document
//! space, the op runs in `f64`, and **every** resulting contour is wrapped back
//! into a closed [`Shape::Path`]. Because the document model is single-ring (no
//! holes), an op that yields several disjoint regions — or a region with a hole —
//! returns *several* paths (Illustrator's "expanded" Pathfinder result): the
//! caller replaces the two inputs with the whole batch. Style (fill/stroke,
//! gradient, appearance) is inherited from the subject shape.
//!
//! The [`FillRule`] passed to `i_overlay` (selectable as
//! [`BoolFillRule`](crate::boolean::BoolFillRule)) decides how self-intersecting
//! or nested input is interpreted — **non-zero** vs **even-odd** — matching the
//! two compound-path fill rules in Illustrator.

use crate::document::Shape;
use i_overlay::core::fill_rule::FillRule;
use i_overlay::core::overlay_rule::OverlayRule;
use i_overlay::float::single::SingleFloatOverlay;

/// Which boolean operation the Pathfinder performs on the two operands.
///
/// `subj` is the lower (back) shape, `clip` is the upper (front / primary) one,
/// matching the selection order the caller passes in.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum BoolOp {
    /// `subj ∪ clip` — the combined area.
    Union,
    /// `subj ∩ clip` — only the overlap.
    Intersect,
    /// `subj − clip` — the front shape punched out of the back one (Minus Front).
    Difference,
    /// Symmetric difference (`subj ⊕ clip`) — everything *except* the overlap.
    Exclude,
    /// `clip − subj` — the back shape punched out of the front one (Minus Back).
    MinusBack,
    /// Split the pair into every non-overlapping region: `subj∩clip`, `subj−clip`
    /// and `clip−subj`, each as its own filled path.
    Divide,
    /// Keep the front shape and the part of the back shape it does *not* cover,
    /// removing the hidden (overlapped) area of the back shape. Result is the two
    /// trimmed faces (`clip` and `subj−clip`), each filled with its own colour.
    Trim,
    /// Like [`Trim`](Self::Trim) but the result faces are unified into a single
    /// region (`subj ∪ clip`), all taking the back shape's fill.
    Merge,
    /// Keep only the part of the back shape that lies inside the front shape
    /// (`subj ∩ clip`), discarding the front shape itself.
    Crop,
    /// Convert the combined outline into unfilled, stroked boundary paths
    /// (`subj ∪ clip`, emitted with no fill and a hairline stroke).
    Outline,
}

impl BoolOp {
    /// Short label for the menu / status line.
    pub fn label(self) -> &'static str {
        match self {
            BoolOp::Union => "Union",
            BoolOp::Intersect => "Intersect",
            BoolOp::Difference => "Minus Front",
            BoolOp::Exclude => "Exclude",
            BoolOp::MinusBack => "Minus Back",
            BoolOp::Divide => "Divide",
            BoolOp::Trim => "Trim",
            BoolOp::Merge => "Merge",
            BoolOp::Crop => "Crop",
            BoolOp::Outline => "Outline",
        }
    }
}

/// Which fill rule `i_overlay` uses to interpret self-intersecting / nested
/// input — the two compound-path fill rules Illustrator exposes.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum BoolFillRule {
    /// Non-zero winding (the default): a point is inside when the signed crossing
    /// count is non-zero. Overlapping same-direction rings stay filled.
    #[default]
    NonZero,
    /// Even-odd: a point is inside when an odd number of rings enclose it, so a
    /// ring drawn inside another *subtracts* (a hole) — the classic donut rule.
    EvenOdd,
}

impl BoolFillRule {
    fn rule(self) -> FillRule {
        match self {
            BoolFillRule::NonZero => FillRule::NonZero,
            BoolFillRule::EvenOdd => FillRule::EvenOdd,
        }
    }
}

/// Flatten a shape to a single closed polygon (outer ring) in document space.
/// Returns `None` for shapes that aren't a sensible closed area.
fn shape_to_polygon(shape: &Shape) -> Option<Vec<[f64; 2]>> {
    let pts = shape.outline_polygon()?;
    Some(pts.iter().map(|&(x, y)| [x as f64, y as f64]).collect())
}

/// Run one overlay rule on the two polygons and return **every** resulting
/// contour as document-space rings, ordered outer-rings-then-holes per shape
/// (the order `i_overlay` extracts them in). Rings shorter than three points are
/// dropped. An empty result (the op cleaves no geometry) yields an empty vec.
fn overlay_rings(
    subj: &Vec<[f64; 2]>,
    clip: &Vec<[f64; 2]>,
    rule: OverlayRule,
    fill: BoolFillRule,
) -> Vec<Vec<(f32, f32)>> {
    // `Shapes<P>` = Vec<Shape> = Vec<Vec<Contour>> = Vec<Vec<Vec<[f64;2]>>>:
    // shapes → contours (outer ring first, then its holes) → points.
    let shapes = subj.overlay(clip, rule, fill.rule());
    shapes
        .into_iter()
        .flat_map(|shape| shape.into_iter())
        .filter(|contour| contour.len() >= 3)
        .map(|contour| {
            contour
                .iter()
                .map(|p| (p[0] as f32, p[1] as f32))
                .collect()
        })
        .collect()
}

/// The subject's paint (solid fill, stroke colour, stroke width). A `Line`
/// (which has no fill region) falls back to a neutral grey fill, the way the
/// previous single-op path did.
fn subj_paint(subj_shape: &Shape) -> ([f32; 4], [f32; 4], f32) {
    let fill = subj_shape.fill_color().unwrap_or([0.5, 0.5, 0.5, 1.0]);
    let stroke = subj_shape.stroke_color().unwrap_or([0.0, 0.0, 0.0, 1.0]);
    (fill, stroke, subj_shape.stroke_width())
}

/// Wrap a document-space ring into a closed [`Shape::Path`] styled from `style`,
/// optionally overriding the fill (e.g. a Merge face that should take a specific
/// colour) and/or dropping the fill entirely (Outline). All anchors are corners.
fn ring_to_path(ring: Vec<(f32, f32)>, style: &Shape, fill_override: Option<[f32; 4]>) -> Shape {
    let (fill, stroke, stroke_w) = subj_paint(style);
    let fill = fill_override.unwrap_or(fill);
    Shape::Path {
        points: ring,
        closed: true,
        fill,
        // A specific fill override means a flat colour, so the gradient/appearance
        // (which paint the *subject's* original fill) are intentionally dropped.
        fill_gradient: if fill_override.is_some() {
            None
        } else {
            style.fill_gradient().cloned()
        },
        stroke,
        stroke_w,
        stroke_style: style.stroke_style().clone(),
        appearance: if fill_override.is_some() {
            None
        } else {
            style.appearance().cloned()
        },
        handles: Vec::new(),
        visible: true,
        group: None,
        clip: None,
        mask: false,
        omask: None,
        omask_path: false,
        omask_invert: false,
        blend: None,
        blend_step: false,
    }
}

/// Run `op` on two shapes with the given fill rule, returning the result as a
/// batch of closed `Shape::Path`s (styled from the operands). The caller replaces
/// the two inputs with the whole batch.
///
/// Returns an **empty vec** when either shape isn't a closed area or the op
/// yields no geometry — callers treat that the same as "no result".
pub fn apply(subj_shape: &Shape, clip_shape: &Shape, op: BoolOp, fill: BoolFillRule) -> Vec<Shape> {
    let (Some(subj), Some(clip)) = (shape_to_polygon(subj_shape), shape_to_polygon(clip_shape))
    else {
        return Vec::new();
    };

    // The single-overlay ops map straight onto one `OverlayRule`; the compound
    // ops (Divide / Trim / Merge / Crop / Outline) compose a couple of overlays
    // and re-style the resulting faces.
    match op {
        BoolOp::Union => overlay_rings(&subj, &clip, OverlayRule::Union, fill)
            .into_iter()
            .map(|r| ring_to_path(r, subj_shape, None))
            .collect(),
        BoolOp::Intersect => overlay_rings(&subj, &clip, OverlayRule::Intersect, fill)
            .into_iter()
            .map(|r| ring_to_path(r, subj_shape, None))
            .collect(),
        BoolOp::Difference => overlay_rings(&subj, &clip, OverlayRule::Difference, fill)
            .into_iter()
            .map(|r| ring_to_path(r, subj_shape, None))
            .collect(),
        BoolOp::Exclude => overlay_rings(&subj, &clip, OverlayRule::Xor, fill)
            .into_iter()
            .map(|r| ring_to_path(r, subj_shape, None))
            .collect(),
        BoolOp::MinusBack => overlay_rings(&subj, &clip, OverlayRule::InverseDifference, fill)
            .into_iter()
            // Minus Back keeps the *front* (clip) shape, so it takes its paint.
            .map(|r| ring_to_path(r, clip_shape, None))
            .collect(),
        BoolOp::Crop => overlay_rings(&subj, &clip, OverlayRule::Intersect, fill)
            .into_iter()
            .map(|r| ring_to_path(r, subj_shape, None))
            .collect(),
        BoolOp::Divide => {
            // Every non-overlapping region: the overlap plus each shape minus the
            // other. The overlap takes the front colour, each crescent its own.
            let mut out = Vec::new();
            for r in overlay_rings(&subj, &clip, OverlayRule::Intersect, fill) {
                out.push(ring_to_path(r, clip_shape, None));
            }
            for r in overlay_rings(&subj, &clip, OverlayRule::Difference, fill) {
                out.push(ring_to_path(r, subj_shape, None));
            }
            for r in overlay_rings(&subj, &clip, OverlayRule::InverseDifference, fill) {
                out.push(ring_to_path(r, clip_shape, None));
            }
            out
        }
        BoolOp::Trim => {
            // Keep the whole front shape plus the back shape with its hidden part
            // removed — two faces, each its own colour, abutting with no overlap.
            let mut out = Vec::new();
            for r in overlay_rings(&clip, &subj, OverlayRule::Subject, fill) {
                out.push(ring_to_path(r, clip_shape, None));
            }
            for r in overlay_rings(&subj, &clip, OverlayRule::Difference, fill) {
                out.push(ring_to_path(r, subj_shape, None));
            }
            out
        }
        BoolOp::Merge => {
            // Like Trim, but the abutting faces are unified into one region taking
            // the back shape's fill (the everyday "weld two same-colour shapes").
            let bg = subj_shape.fill_color().unwrap_or([0.5, 0.5, 0.5, 1.0]);
            overlay_rings(&subj, &clip, OverlayRule::Union, fill)
                .into_iter()
                .map(|r| ring_to_path(r, subj_shape, Some(bg)))
                .collect()
        }
        BoolOp::Outline => {
            // The combined boundary as unfilled, hairline-stroked paths: take the
            // union outline and emit each ring with a transparent fill so only the
            // edges show, the way Illustrator's Outline divides art into strokes.
            let stroke = subj_shape.stroke_color().unwrap_or([0.0, 0.0, 0.0, 1.0]);
            overlay_rings(&subj, &clip, OverlayRule::Union, fill)
                .into_iter()
                .map(|ring| {
                    let mut s = ring_to_path(ring, subj_shape, Some([0.0, 0.0, 0.0, 0.0]));
                    s.set_stroke_color(stroke);
                    s.set_stroke_width(1.0);
                    s
                })
                .collect()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::StrokeStyle;

    fn rect(x: f32, y: f32, w: f32, h: f32) -> Shape {
        Shape::Rect {
            rect: [x, y, w, h],
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
            blend: None,
            blend_step: false,
        }
    }

    /// Signed area of a closed ring (shoelace); magnitude is the area.
    fn area(pts: &[(f32, f32)]) -> f32 {
        let n = pts.len();
        let mut a = 0.0;
        for i in 0..n {
            let (x0, y0) = pts[i];
            let (x1, y1) = pts[(i + 1) % n];
            a += x0 * y1 - x1 * y0;
        }
        (a * 0.5).abs()
    }

    /// Total filled area across a batch of result paths.
    fn total_area(shapes: &[Shape]) -> f32 {
        shapes
            .iter()
            .map(|s| match s {
                Shape::Path { points, .. } => area(points),
                _ => 0.0,
            })
            .sum()
    }

    #[test]
    fn union_of_overlapping_rects() {
        let a = rect(0.0, 0.0, 10.0, 10.0);
        let b = rect(5.0, 5.0, 10.0, 10.0);
        let r = apply(&a, &b, BoolOp::Union, BoolFillRule::NonZero);
        assert_eq!(r.len(), 1, "union is a single region");
        match &r[0] {
            Shape::Path { points, closed, .. } => {
                assert!(closed);
                assert!(points.len() >= 6, "L-shape union ring");
            }
            _ => panic!("expected Path"),
        }
        // 100 + 100 − 25 overlap = 175.
        assert!((total_area(&r) - 175.0).abs() < 0.5);
    }

    #[test]
    fn intersect_of_overlapping_rects() {
        let a = rect(0.0, 0.0, 10.0, 10.0);
        let b = rect(5.0, 5.0, 10.0, 10.0);
        let r = apply(&a, &b, BoolOp::Intersect, BoolFillRule::NonZero);
        assert_eq!(r.len(), 1);
        assert!(matches!(r[0], Shape::Path { closed: true, .. }));
        assert!((total_area(&r) - 25.0).abs() < 0.5, "5×5 overlap");
    }

    #[test]
    fn intersect_of_disjoint_rects_is_empty() {
        let a = rect(0.0, 0.0, 10.0, 10.0);
        let b = rect(100.0, 100.0, 10.0, 10.0);
        assert!(apply(&a, &b, BoolOp::Intersect, BoolFillRule::NonZero).is_empty());
    }

    #[test]
    fn difference_punches_a_hole_region() {
        // A fully enclosing B: subj − clip leaves a frame (outer ring + hole) —
        // two rings in the single-ring model.
        let outer = rect(0.0, 0.0, 30.0, 30.0);
        let inner = rect(10.0, 10.0, 10.0, 10.0);
        let r = apply(&outer, &inner, BoolOp::Difference, BoolFillRule::NonZero);
        assert_eq!(r.len(), 2, "outer ring + the hole ring");
        // Net frame area = 900 − 100 = 800; here both rings count positive, so the
        // larger (900) minus would be wrong — assert each ring individually.
        let mut areas: Vec<f32> = r
            .iter()
            .map(|s| match s {
                Shape::Path { points, .. } => area(points),
                _ => 0.0,
            })
            .collect();
        areas.sort_by(|x, y| x.partial_cmp(y).unwrap());
        assert!((areas[0] - 100.0).abs() < 0.5, "hole ring is the 10×10 inner");
        assert!((areas[1] - 900.0).abs() < 0.5, "outer ring is the 30×30");
    }

    #[test]
    fn exclude_drops_the_overlap() {
        let a = rect(0.0, 0.0, 10.0, 10.0);
        let b = rect(5.0, 5.0, 10.0, 10.0);
        let r = apply(&a, &b, BoolOp::Exclude, BoolFillRule::NonZero);
        assert!(!r.is_empty());
        // Two 100-area squares minus twice the 25 overlap = 150.
        assert!((total_area(&r) - 150.0).abs() < 0.5);
    }

    #[test]
    fn minus_back_keeps_front_only() {
        let back = rect(0.0, 0.0, 10.0, 10.0);
        let front = rect(5.0, 5.0, 10.0, 10.0);
        let r = apply(&back, &front, BoolOp::MinusBack, BoolFillRule::NonZero);
        assert_eq!(r.len(), 1);
        // front − back = 100 − 25 = 75.
        assert!((total_area(&r) - 75.0).abs() < 0.5);
        // Inherits the *front* shape's paint (front fill is red from `rect`).
        assert_eq!(r[0].fill_color(), Some([1.0, 0.0, 0.0, 1.0]));
    }

    #[test]
    fn divide_partitions_the_union() {
        let a = rect(0.0, 0.0, 10.0, 10.0);
        let b = rect(5.0, 5.0, 10.0, 10.0);
        let r = apply(&a, &b, BoolOp::Divide, BoolFillRule::NonZero);
        // overlap + (a−b) + (b−a) = three faces.
        assert_eq!(r.len(), 3);
        // They tile the union exactly: 25 + 75 + 75 = 175 (no double-count).
        assert!((total_area(&r) - 175.0).abs() < 0.5);
    }

    #[test]
    fn trim_keeps_front_and_trims_back() {
        let back = rect(0.0, 0.0, 10.0, 10.0);
        let front = rect(5.0, 5.0, 10.0, 10.0);
        let r = apply(&back, &front, BoolOp::Trim, BoolFillRule::NonZero);
        assert_eq!(r.len(), 2, "front face + trimmed back face");
        // front (100) + (back − front) (75) = 175 with no overlap.
        assert!((total_area(&r) - 175.0).abs() < 0.5);
    }

    #[test]
    fn merge_welds_into_one_region() {
        let a = rect(0.0, 0.0, 10.0, 10.0);
        let b = rect(5.0, 5.0, 10.0, 10.0);
        let r = apply(&a, &b, BoolOp::Merge, BoolFillRule::NonZero);
        assert_eq!(r.len(), 1, "one welded region");
        assert!((total_area(&r) - 175.0).abs() < 0.5);
    }

    #[test]
    fn crop_keeps_only_the_overlap() {
        let a = rect(0.0, 0.0, 10.0, 10.0);
        let b = rect(5.0, 5.0, 10.0, 10.0);
        let r = apply(&a, &b, BoolOp::Crop, BoolFillRule::NonZero);
        assert_eq!(r.len(), 1);
        assert!((total_area(&r) - 25.0).abs() < 0.5);
    }

    #[test]
    fn outline_emits_unfilled_strokes() {
        let a = rect(0.0, 0.0, 10.0, 10.0);
        let b = rect(5.0, 5.0, 10.0, 10.0);
        let r = apply(&a, &b, BoolOp::Outline, BoolFillRule::NonZero);
        assert!(!r.is_empty());
        for s in &r {
            // Transparent fill, visible stroke.
            assert_eq!(s.fill_color(), Some([0.0, 0.0, 0.0, 0.0]));
            assert!(s.stroke_color().unwrap()[3] > 0.0);
        }
    }

    #[test]
    fn fill_rule_changes_a_ring_in_ring() {
        // A compound input: an outer 30×30 ring and an inner 10×10 ring wound the
        // *same* way, both in one operand, self-resolved (`Subject` rule). Only the
        // fill rule decides whether the inner ring carves a hole.
        let compound: Vec<Vec<[f64; 2]>> = vec![
            vec![[0.0, 0.0], [30.0, 0.0], [30.0, 30.0], [0.0, 30.0]],
            vec![[10.0, 10.0], [20.0, 10.0], [20.0, 20.0], [10.0, 20.0]],
        ];
        let empty: Vec<Vec<[f64; 2]>> = Vec::new();

        let rings = |fill: FillRule| -> Vec<Vec<(f32, f32)>> {
            compound
                .overlay(&empty, OverlayRule::Subject, fill)
                .into_iter()
                .flat_map(|s| s.into_iter())
                .filter(|c| c.len() >= 3)
                .map(|c| c.iter().map(|p| (p[0] as f32, p[1] as f32)).collect())
                .collect()
        };

        // Non-zero: both rings wind the same way → the inner does *not* subtract,
        // so the filled area is the full 30×30 = 900 (a single ring).
        let nz = rings(FillRule::NonZero);
        assert_eq!(nz.len(), 1, "non-zero fills the inner square in");
        assert!((area(&nz[0]) - 900.0).abs() < 0.5);

        // Even-odd: the inner ring carves a hole → outer ring + hole ring, net
        // 900 − 100 = 800.
        let eo = rings(FillRule::EvenOdd);
        assert_eq!(eo.len(), 2, "even-odd carves a hole");
        let mut areas: Vec<f32> = eo.iter().map(|r| area(r)).collect();
        areas.sort_by(|x, y| x.partial_cmp(y).unwrap());
        assert!((areas[0] - 100.0).abs() < 0.5);
        assert!((areas[1] - 900.0).abs() < 0.5);
    }

    #[test]
    fn open_path_yields_no_result() {
        let a = rect(0.0, 0.0, 10.0, 10.0);
        let line = Shape::Line {
            p0: (0.0, 0.0),
            p1: (10.0, 10.0),
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
            blend: None,
            blend_step: false,
        };
        assert!(apply(&a, &line, BoolOp::Union, BoolFillRule::NonZero).is_empty());
    }
}
