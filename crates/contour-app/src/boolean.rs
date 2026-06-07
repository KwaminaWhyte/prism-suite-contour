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

use crate::document::{FillRule as DocFillRule, Shape, SubPath};
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

/// Run one overlay rule and return the result **grouped by shape**: each inner
/// `Vec` is one filled region's contours, the first being the outer ring and any
/// following ones its holes (the structure `i_overlay` extracts). Empty contours
/// (< 3 points) are dropped; a shape with no usable outer ring is dropped.
fn overlay_grouped(
    subj: &Vec<[f64; 2]>,
    clip: &Vec<[f64; 2]>,
    rule: OverlayRule,
    fill: BoolFillRule,
) -> Vec<Vec<Vec<(f32, f32)>>> {
    let shapes = subj.overlay(clip, rule, fill.rule());
    shapes
        .into_iter()
        .map(|shape| {
            shape
                .into_iter()
                .filter(|c| c.len() >= 3)
                .map(|c| c.iter().map(|p| (p[0] as f32, p[1] as f32)).collect())
                .collect::<Vec<Vec<(f32, f32)>>>()
        })
        .filter(|contours| !contours.is_empty())
        .collect()
}

/// Turn `i_overlay`'s grouped result into document shapes: a region with a single
/// contour becomes a closed [`Shape::Path`]; a region with **holes** (an outer
/// ring plus inner rings) becomes one [`Shape::Compound`] keeping its holes as
/// sub-contours — the document model's real compound path, so a Pathfinder result
/// with holes is one object instead of separate rings. Styled from `style`,
/// optionally overriding the fill.
fn grouped_to_shapes(
    grouped: Vec<Vec<Vec<(f32, f32)>>>,
    style: &Shape,
    fill_override: Option<[f32; 4]>,
    fill_rule: BoolFillRule,
) -> Vec<Shape> {
    grouped
        .into_iter()
        .filter_map(|contours| region_to_shape(contours, style, fill_override, fill_rule))
        .collect()
}

/// One filled region (outer ring then holes) → a `Path` (no holes) or a
/// `Compound` (with holes).
fn region_to_shape(
    mut contours: Vec<Vec<(f32, f32)>>,
    style: &Shape,
    fill_override: Option<[f32; 4]>,
    fill_rule: BoolFillRule,
) -> Option<Shape> {
    if contours.is_empty() {
        return None;
    }
    if contours.len() == 1 {
        return Some(ring_to_path(contours.remove(0), style, fill_override));
    }
    // Holes present: build a compound path. The result is unambiguous geometry
    // (an outer ring plus inner hole rings), so it is stored under **even-odd** —
    // which carves the holes regardless of each ring's winding direction, the
    // robust choice for a derived Pathfinder result. (The `fill` rule passed in
    // governs how the *input* nesting was interpreted by `i_overlay`.)
    let _ = fill_rule;
    let subpaths: Vec<SubPath> = contours.into_iter().map(SubPath::ring).collect();
    Some(compound_from_subpaths(subpaths, style, fill_override))
}

/// Build a [`Shape::Compound`] from sub-contours, inheriting `style`'s paint (or
/// a flat `fill_override`), under the even-odd rule so holes always carve.
fn compound_from_subpaths(
    subpaths: Vec<SubPath>,
    style: &Shape,
    fill_override: Option<[f32; 4]>,
) -> Shape {
    let (fill, stroke, stroke_w) = subj_paint(style);
    let fill = fill_override.unwrap_or(fill);
    Shape::Compound {
        subpaths,
        fill_rule: DocFillRule::EvenOdd,
        fill,
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
        name: None,
        locked: false,
        layer_color: None,
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
    // and re-style the resulting faces. Area-conserving ops now return their
    // regions **grouped**, so a region with holes becomes one compound path
    // (outer ring + hole sub-contours) rather than separate rings.
    match op {
        BoolOp::Union => grouped_to_shapes(
            overlay_grouped(&subj, &clip, OverlayRule::Union, fill),
            subj_shape,
            None,
            fill,
        ),
        BoolOp::Intersect => grouped_to_shapes(
            overlay_grouped(&subj, &clip, OverlayRule::Intersect, fill),
            subj_shape,
            None,
            fill,
        ),
        BoolOp::Difference => grouped_to_shapes(
            overlay_grouped(&subj, &clip, OverlayRule::Difference, fill),
            subj_shape,
            None,
            fill,
        ),
        BoolOp::Exclude => grouped_to_shapes(
            overlay_grouped(&subj, &clip, OverlayRule::Xor, fill),
            subj_shape,
            None,
            fill,
        ),
        BoolOp::MinusBack => grouped_to_shapes(
            overlay_grouped(&subj, &clip, OverlayRule::InverseDifference, fill),
            // Minus Back keeps the *front* (clip) shape, so it takes its paint.
            clip_shape,
            None,
            fill,
        ),
        BoolOp::Crop => grouped_to_shapes(
            overlay_grouped(&subj, &clip, OverlayRule::Intersect, fill),
            subj_shape,
            None,
            fill,
        ),
        BoolOp::Divide => {
            // Every non-overlapping region: the overlap plus each shape minus the
            // other. The overlap takes the front colour, each crescent its own.
            // Each region keeps its holes as a compound (e.g. a Divide producing a
            // ring-with-hole face).
            let mut out = grouped_to_shapes(
                overlay_grouped(&subj, &clip, OverlayRule::Intersect, fill),
                clip_shape,
                None,
                fill,
            );
            out.extend(grouped_to_shapes(
                overlay_grouped(&subj, &clip, OverlayRule::Difference, fill),
                subj_shape,
                None,
                fill,
            ));
            out.extend(grouped_to_shapes(
                overlay_grouped(&subj, &clip, OverlayRule::InverseDifference, fill),
                clip_shape,
                None,
                fill,
            ));
            out
        }
        BoolOp::Trim => merge_trim(&subj, &clip, subj_shape, clip_shape, fill, false),
        BoolOp::Merge => merge_trim(&subj, &clip, subj_shape, clip_shape, fill, true),
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

/// Whether two straight-sRGB RGBA colours are equal within a small tolerance —
/// the "same colour" test that decides whether **Merge** welds two abutting /
/// overlapping faces (Illustrator merges adjacent *same-colour* faces).
fn same_color(a: [f32; 4], b: [f32; 4]) -> bool {
    a.iter().zip(b.iter()).all(|(x, y)| (x - y).abs() < 1e-3)
}

/// Illustrator's **Trim** and **Merge** pathfinders, on two operands.
///
/// Both *trim hidden parts*: the front (clip) shape stays whole and the back
/// (subj) shape has the part the front covers removed, so no two faces overlap
/// and **each face keeps its own fill** (unlike the old "weld to the back colour"
/// approximation). The difference is `merge`:
///
/// - **Trim** (`merge == false`): always two trimmed faces (front + back−front),
///   each its own colour, regardless of whether they match.
/// - **Merge** (`merge == true`): same as Trim when the two faces differ in
///   colour, but when they are the **same colour** the abutting faces are welded
///   into one path (`subj ∪ clip`) under that shared colour — Illustrator's
///   "merge adjacent same-colour faces" behaviour.
///
/// Each produced face keeps its holes (a face that comes out a ring-with-hole
/// becomes a compound path).
fn merge_trim(
    subj: &Vec<[f64; 2]>,
    clip: &Vec<[f64; 2]>,
    subj_shape: &Shape,
    clip_shape: &Shape,
    fill: BoolFillRule,
    merge: bool,
) -> Vec<Shape> {
    let subj_fill = subj_shape.fill_color().unwrap_or([0.5, 0.5, 0.5, 1.0]);
    let clip_fill = clip_shape.fill_color().unwrap_or([0.5, 0.5, 0.5, 1.0]);

    // Merge welds only when the two faces share a colour: union them into one
    // region (still keeping any holes as a compound), taking that shared fill.
    if merge && same_color(subj_fill, clip_fill) {
        return grouped_to_shapes(
            overlay_grouped(subj, clip, OverlayRule::Union, fill),
            subj_shape,
            Some(subj_fill),
            fill,
        );
    }

    // Otherwise (Trim, or Merge of differently-coloured faces): the front shape
    // whole on top, plus the back shape with the overlapped (hidden) part removed.
    // Two trimmed faces, each its own colour, abutting with no overlap.
    let mut out = grouped_to_shapes(
        overlay_grouped(clip, subj, OverlayRule::Subject, fill),
        clip_shape,
        None,
        fill,
    );
    out.extend(grouped_to_shapes(
        overlay_grouped(subj, clip, OverlayRule::Difference, fill),
        subj_shape,
        None,
        fill,
    ));
    out
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
            name: None,
            locked: false,
            layer_color: None,
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

    /// Net filled area of a single result shape: a `Path`'s ring area, or a
    /// compound path's outer ring minus its holes (the largest sub-contour is the
    /// outer ring, the rest are holes that subtract).
    fn shape_area(s: &Shape) -> f32 {
        match s {
            Shape::Path { points, .. } => area(points),
            Shape::Compound { subpaths, .. } => {
                let mut areas: Vec<f32> = subpaths.iter().map(|sp| area(&sp.flatten())).collect();
                areas.sort_by(|x, y| y.partial_cmp(x).unwrap());
                // outer (largest) minus the rest (holes)
                let outer = areas.first().copied().unwrap_or(0.0);
                let holes: f32 = areas.iter().skip(1).sum();
                (outer - holes).max(0.0)
            }
            _ => 0.0,
        }
    }

    /// Total filled area across a batch of result paths (net of holes).
    fn total_area(shapes: &[Shape]) -> f32 {
        shapes.iter().map(shape_area).sum()
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
        // A fully enclosing B: subj − clip leaves a frame (outer ring + hole).
        // The compound-path model keeps this as ONE object with two sub-contours
        // (outer 30×30 ring + inner 10×10 hole), instead of two separate rings.
        let outer = rect(0.0, 0.0, 30.0, 30.0);
        let inner = rect(10.0, 10.0, 10.0, 10.0);
        let r = apply(&outer, &inner, BoolOp::Difference, BoolFillRule::NonZero);
        assert_eq!(r.len(), 1, "one compound path (ring with a hole)");
        match &r[0] {
            Shape::Compound {
                subpaths,
                fill_rule,
                ..
            } => {
                assert_eq!(subpaths.len(), 2, "outer ring + one hole sub-contour");
                assert_eq!(
                    *fill_rule,
                    crate::document::FillRule::EvenOdd,
                    "holes carve under even-odd"
                );
                // Sub-contour areas: a 900 outer and a 100 hole.
                let mut areas: Vec<f32> = subpaths.iter().map(|sp| area(&sp.flatten())).collect();
                areas.sort_by(|x, y| x.partial_cmp(y).unwrap());
                assert!((areas[0] - 100.0).abs() < 0.5, "hole sub-contour is 10×10");
                assert!((areas[1] - 900.0).abs() < 0.5, "outer sub-contour is 30×30");
            }
            other => panic!("expected a Compound path, got {other:?}"),
        }
        // Net filled area = 900 − 100 = 800.
        assert!((total_area(&r) - 800.0).abs() < 0.5, "net frame area");
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

    /// A rect with an explicit fill colour, for the Merge same-/different-colour
    /// tests.
    fn colored_rect(x: f32, y: f32, w: f32, h: f32, fill: [f32; 4]) -> Shape {
        let mut s = rect(x, y, w, h);
        s.set_fill_color(fill);
        s
    }

    #[test]
    fn merge_welds_into_one_region() {
        // Same colour (both red from `rect`): Merge welds them into one region.
        let a = rect(0.0, 0.0, 10.0, 10.0);
        let b = rect(5.0, 5.0, 10.0, 10.0);
        let r = apply(&a, &b, BoolOp::Merge, BoolFillRule::NonZero);
        assert_eq!(r.len(), 1, "one welded region");
        assert!((total_area(&r) - 175.0).abs() < 0.5);
    }

    /// **Merge** of two *different-coloured* overlapping faces does NOT weld:
    /// it trims the hidden part of the back face and keeps each face its own
    /// colour (Illustrator's real Merge — merge same-colour, trim different).
    #[test]
    fn merge_trims_different_colored_faces() {
        let back = colored_rect(0.0, 0.0, 10.0, 10.0, [1.0, 0.0, 0.0, 1.0]); // red
        let front = colored_rect(5.0, 5.0, 10.0, 10.0, [0.0, 0.0, 1.0, 1.0]); // blue
        let r = apply(&back, &front, BoolOp::Merge, BoolFillRule::NonZero);
        // Two trimmed faces (front whole + back−front), no overlap.
        assert_eq!(r.len(), 2, "different colours stay two trimmed faces");
        // front (100) + (back − front) (75) = 175, no double-counted overlap.
        assert!((total_area(&r) - 175.0).abs() < 0.5);
        // Each face keeps its own colour: a blue (front) and a red (back) face.
        let colors: Vec<[f32; 4]> = r.iter().filter_map(|s| s.fill_color()).collect();
        assert!(colors.contains(&[0.0, 0.0, 1.0, 1.0]), "front blue preserved");
        assert!(colors.contains(&[1.0, 0.0, 0.0, 1.0]), "back red preserved");
        // No face is the welded back-colour-only union (that would be 1 region).
        let total_front_area: f32 = r
            .iter()
            .filter(|s| s.fill_color() == Some([0.0, 0.0, 1.0, 1.0]))
            .map(shape_area)
            .sum();
        assert!((total_front_area - 100.0).abs() < 0.5, "front face is whole");
    }

    /// **Merge** of two same-coloured faces welds them into one region under that
    /// shared colour (the explicit same-colour weld path).
    #[test]
    fn merge_welds_same_colored_faces() {
        let c = [0.2, 0.7, 0.3, 1.0];
        let a = colored_rect(0.0, 0.0, 10.0, 10.0, c);
        let b = colored_rect(5.0, 5.0, 10.0, 10.0, c);
        let r = apply(&a, &b, BoolOp::Merge, BoolFillRule::NonZero);
        assert_eq!(r.len(), 1, "same colour welds into one region");
        assert!((total_area(&r) - 175.0).abs() < 0.5);
        assert_eq!(r[0].fill_color(), Some(c), "welded region keeps the colour");
    }

    /// **Trim** always trims the hidden part of the back face and keeps each
    /// face's own colour (it never welds, even when colours match).
    #[test]
    fn trim_preserves_each_face_color() {
        let back = colored_rect(0.0, 0.0, 10.0, 10.0, [1.0, 0.0, 0.0, 1.0]);
        let front = colored_rect(5.0, 5.0, 10.0, 10.0, [0.0, 0.0, 1.0, 1.0]);
        let r = apply(&back, &front, BoolOp::Trim, BoolFillRule::NonZero);
        assert_eq!(r.len(), 2);
        let colors: Vec<[f32; 4]> = r.iter().filter_map(|s| s.fill_color()).collect();
        assert!(colors.contains(&[0.0, 0.0, 1.0, 1.0]), "front blue kept");
        assert!(colors.contains(&[1.0, 0.0, 0.0, 1.0]), "back red kept (trimmed)");
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
            name: None,
            locked: false,
            layer_color: None,
        };
        assert!(apply(&a, &line, BoolOp::Union, BoolFillRule::NonZero).is_empty());
    }
}
