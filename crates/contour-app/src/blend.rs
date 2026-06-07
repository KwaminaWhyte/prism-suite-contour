//! **Blend** — Illustrator's `Object ▸ Blend ▸ Make` (specified-steps mode).
//!
//! A blend generates a series of intermediate objects that morph between two
//! selected objects: each step interpolates **position**, **path geometry**
//! (point-by-point between the two outlines), and **appearance** (fill / stroke
//! colour + opacity, linearly in the existing straight-sRGB colour space). With
//! `steps = N` the document gains `N` intermediate shapes evenly spaced from the
//! first end (`t → 0`) to the second (`t → 1`), exclusive of the ends.
//!
//! **Expand-on-create.** This pass ships the *expanded* form: Make generates the
//! intermediate shapes as real objects and tags the whole run (the two ends plus
//! the generated steps) with a shared **blend-set id** (`Shape::blend`), the
//! generated ones additionally flagged [`is step`](crate::document::Shape::is_blend_step).
//! **Release** deletes the generated steps and clears the tags, leaving the two
//! ends. A persistent *live* re-blend (re-running when an end moves) is a noted
//! gap — see the changelog / PLAN.
//!
//! **Geometry.** Both outlines are resampled to a common point count along **arc
//! length** with `kurbo` (`PathSeg::arclen` / `inv_arclen` / `eval`), so two
//! paths with different anchor counts still interpolate corresponding points.
//! Closed↔closed and open↔open are handled; mixed open/closed falls back to a
//! plain resample of each (a noted edge-case gap). The interpolated step is a
//! corner [`Shape::Path`] tracing the morph (handles are not interpolated this
//! pass — the resampled polyline carries the shape).
//!
//! Everything here is pure and unit-tested without any egui / UI context.

use crate::document::Shape;
use crate::gradient::lerp_color;
use kurbo::{ParamCurve, ParamCurveArclen, PathSeg};

/// Arc-length resampling accuracy (document units) handed to kurbo's arclen /
/// inv_arclen. Tight enough that a unit circle samples within well under a pixel.
const ARCLEN_ACCURACY: f64 = 1e-3;

/// The smallest blend-set id not currently used by any tag, so a freshly-made
/// blend never collides with an existing one. Ids are opaque; only equality
/// matters (independent of group / clip / opacity-mask ids).
pub fn next_blend_id(tags: &[Option<u64>]) -> u64 {
    tags.iter().filter_map(|t| *t).max().map_or(0, |m| m + 1)
}

/// Whether a selection can be blended: exactly two distinct in-range shapes,
/// neither already part of a blend set. (Illustrator blends two objects at a
/// time; multi-object chained blends are a noted gap.)
pub fn can_make(blend_tags: &[Option<u64>], selected: &[usize]) -> bool {
    let mut s: Vec<usize> = selected
        .iter()
        .copied()
        .filter(|&i| i < blend_tags.len())
        .collect();
    s.sort_unstable();
    s.dedup();
    s.len() == 2 && s.iter().all(|&i| blend_tags[i].is_none())
}

/// Whether the selection touches any blend set (so Release / Expand is useful).
pub fn can_release(blend_tags: &[Option<u64>], selected: &[usize]) -> bool {
    selected
        .iter()
        .any(|&i| blend_tags.get(i).copied().flatten().is_some())
}

/// The blend-set ids referenced by `selected`, ascending and de-duplicated.
pub fn selected_blend_ids(blend_tags: &[Option<u64>], selected: &[usize]) -> Vec<u64> {
    let mut ids: Vec<u64> = selected
        .iter()
        .filter_map(|&i| blend_tags.get(i).copied().flatten())
        .collect();
    ids.sort_unstable();
    ids.dedup();
    ids
}

/// Resample a shape's outline to exactly `n` points spaced evenly along its
/// **arc length**, in document space. `n` is clamped to ≥ 2.
///
/// The shape is converted to its `kurbo` `BezPath` (honouring bezier handles)
/// and walked segment-by-segment: sample `k` is at fractional arc length
/// `k/(n-1)` of the total perimeter for an open path, or `k/n` for a closed
/// path (so the last sample doesn't duplicate the first). An empty / degenerate
/// shape yields `n` copies of a single point (or the origin).
pub fn resample(shape: &Shape, n: usize) -> Vec<(f32, f32)> {
    let n = n.max(2);
    let (bez, closed) = shape_bezpath(shape);
    // Collect segments with their arc lengths and cumulative offsets.
    let segs: Vec<PathSeg> = bez.segments().collect();
    if segs.is_empty() {
        // A zero-segment path: fall back to the shape's first anchor (or origin).
        let p = first_point(shape).unwrap_or((0.0, 0.0));
        return vec![p; n];
    }
    let lens: Vec<f64> = segs.iter().map(|s| s.arclen(ARCLEN_ACCURACY)).collect();
    let total: f64 = lens.iter().sum();
    if total <= f64::EPSILON {
        let p0 = segs[0].eval(0.0);
        return vec![(p0.x as f32, p0.y as f32); n];
    }
    // Cumulative arc length at the *start* of each segment.
    let mut starts: Vec<f64> = Vec::with_capacity(segs.len());
    let mut acc = 0.0;
    for &l in &lens {
        starts.push(acc);
        acc += l;
    }

    let mut out: Vec<(f32, f32)> = Vec::with_capacity(n);
    for k in 0..n {
        // Even spacing along arc length: open paths span [0, total] inclusive of
        // both ends; closed paths span [0, total) so the loop isn't doubled.
        let frac = if closed {
            k as f64 / n as f64
        } else {
            k as f64 / (n - 1) as f64
        };
        let target = (frac * total).min(total);
        let p = point_at_arclen(&segs, &starts, &lens, target);
        out.push((p.0 as f32, p.1 as f32));
    }
    out
}

/// The document-space point at absolute arc length `target` along the segment
/// list (with precomputed `starts` and `lens`). Locates the containing segment
/// then uses `inv_arclen` for the local parameter.
fn point_at_arclen(
    segs: &[PathSeg],
    starts: &[f64],
    lens: &[f64],
    target: f64,
) -> (f64, f64) {
    // Find the last segment whose start is <= target.
    let mut idx = 0usize;
    for (i, &st) in starts.iter().enumerate() {
        if st <= target {
            idx = i;
        } else {
            break;
        }
    }
    let local = target - starts[idx];
    let seg = segs[idx];
    let seg_len = lens[idx];
    let t = if seg_len <= f64::EPSILON {
        0.0
    } else {
        seg.inv_arclen(local.clamp(0.0, seg_len), ARCLEN_ACCURACY)
    };
    let p = seg.eval(t);
    (p.x, p.y)
}

/// The shape's `kurbo` `BezPath` plus whether it is a closed outline. `Rect` /
/// `Ellipse` / closed `Path` are closed; `Line` / open `Path` are open.
fn shape_bezpath(shape: &Shape) -> (kurbo::BezPath, bool) {
    let path = shape.to_path();
    match path {
        Shape::Path {
            points,
            handles,
            closed,
            ..
        } => (
            crate::document::bez_path(&points, &handles, closed),
            closed,
        ),
        // A compound path blends by its outer ring (a single closed contour).
        Shape::Compound { .. } => match shape.outline_polygon() {
            Some(ring) => {
                let h = vec![(0.0, 0.0); ring.len()];
                (crate::document::bez_path(&ring, &h, true), true)
            }
            None => (kurbo::BezPath::new(), false),
        },
        // to_path yields a Path or Compound; other arms are unreachable.
        _ => (kurbo::BezPath::new(), false),
    }
}

/// First anchor of a shape (for degenerate fallback).
fn first_point(shape: &Shape) -> Option<(f32, f32)> {
    match shape {
        Shape::Rect { rect, .. } | Shape::Ellipse { rect, .. } => Some((rect[0], rect[1])),
        Shape::Line { p0, .. } => Some(*p0),
        Shape::Path { points, .. } => points.first().copied(),
        Shape::Compound { subpaths, .. } => {
            subpaths.first().and_then(|s| s.points.first().copied())
        }
    }
}

/// Whether a shape's outline is closed (matters for picking a common topology).
fn is_closed(shape: &Shape) -> bool {
    match shape {
        Shape::Rect { .. } | Shape::Ellipse { .. } => true,
        Shape::Line { .. } => false,
        Shape::Path { closed, .. } => *closed,
        // A compound path is treated as closed (it is filled area); blending uses
        // its outer ring via `outline_polygon`.
        Shape::Compound { .. } => true,
    }
}

/// Linearly interpolate two `[f32; 4]` colours at `t` (clamped 0..=1) in straight
/// sRGB — the same space the pickers and gradients use. Re-exported wrapper over
/// [`lerp_color`] so call-sites read as colour interpolation.
pub fn lerp_rgba(a: [f32; 4], b: [f32; 4], t: f32) -> [f32; 4] {
    lerp_color(a, b, t)
}

/// Build the intermediate blend shape at parameter `t` (0 = `a`, 1 = `b`) between
/// shapes `a` and `b`.
///
/// Geometry: both outlines are resampled to a common point count (the larger of
/// the two flattened sizes, clamped to a sane band) along arc length, then
/// corresponding points are linearly interpolated — so position *and* path shape
/// morph together. The result is a corner [`Shape::Path`], closed iff both ends
/// are closed (mixed topology blends as an open path — a noted gap).
///
/// Appearance: the step's fill, stroke colour, stroke width, and the per-channel
/// alpha (opacity) all interpolate linearly between the two ends. The step
/// inherits `a`'s stroke style (caps / joins / dashes).
pub fn interpolate_shape(a: &Shape, b: &Shape, t: f32) -> Shape {
    let t = t.clamp(0.0, 1.0);
    let n = common_sample_count(a, b);
    let pa = resample(a, n);
    let pb = resample(b, n);
    // Align by rotation isn't done this pass (a noted refinement); index-to-index
    // interpolation of two arc-length resamples is correct for the common cases.
    let points: Vec<(f32, f32)> = pa
        .iter()
        .zip(pb.iter())
        .map(|(&(ax, ay), &(bx, by))| (ax + (bx - ax) * t, ay + (by - ay) * t))
        .collect();
    let closed = is_closed(a) && is_closed(b);

    // Appearance interpolation (linear, straight sRGB). A `Line` has no fill, so
    // its fill colour is treated as fully transparent for the interpolation.
    let fa = a.fill_color().unwrap_or([0.0, 0.0, 0.0, 0.0]);
    let fb = b.fill_color().unwrap_or([0.0, 0.0, 0.0, 0.0]);
    let fill = lerp_rgba(fa, fb, t);
    let sa = a.stroke_color().unwrap_or([0.0, 0.0, 0.0, 0.0]);
    let sb = b.stroke_color().unwrap_or([0.0, 0.0, 0.0, 0.0]);
    let stroke = lerp_rgba(sa, sb, t);
    let stroke_w = a.stroke_width() + (b.stroke_width() - a.stroke_width()) * t;

    let handles = vec![(0.0, 0.0); points.len()];
    Shape::Path {
        points,
        closed,
        fill,
        fill_gradient: None,
        stroke,
        stroke_w,
        stroke_style: a.stroke_style().clone(),
        handles,
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

/// The point count to resample both ends to: the larger of the two flattened
/// outlines, clamped to a `[8, 256]` band so a triangle and a 4-point rect still
/// interpolate smoothly without exploding on huge paths.
fn common_sample_count(a: &Shape, b: &Shape) -> usize {
    let fa = flattened_len(a);
    let fb = flattened_len(b);
    fa.max(fb).clamp(8, 256)
}

/// How many points a shape's outline flattens to (a proxy for its complexity).
fn flattened_len(shape: &Shape) -> usize {
    match shape.to_path() {
        Shape::Path {
            points,
            handles,
            closed,
            ..
        } => crate::document::flatten(&points, &handles, closed).len(),
        _ => 0,
    }
}

/// Generate the `steps` intermediate blend shapes between `a` and `b` (exclusive
/// of the two ends), evenly spaced in `t`. `steps == 0` yields no shapes (the
/// ends just become a blend set). The shapes are returned in front-to-back order
/// from `a` toward `b`.
pub fn make_steps(a: &Shape, b: &Shape, steps: usize) -> Vec<Shape> {
    if steps == 0 {
        return Vec::new();
    }
    (1..=steps)
        .map(|k| {
            let t = k as f32 / (steps + 1) as f32;
            interpolate_shape(a, b, t)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::Shape;

    fn rect(x: f32, y: f32, w: f32, h: f32) -> Shape {
        Shape::Rect {
            rect: [x, y, w, h],
            fill: [1.0, 0.0, 0.0, 1.0],
            fill_gradient: None,
            stroke: [0.0, 0.0, 0.0, 1.0],
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
        }
    }

    fn open_path(points: Vec<(f32, f32)>) -> Shape {
        let n = points.len();
        Shape::Path {
            points,
            closed: false,
            fill: [0.0; 4],
            fill_gradient: None,
            stroke: [0.0, 0.0, 0.0, 1.0],
            stroke_w: 2.0,
            stroke_style: Default::default(),
            handles: vec![(0.0, 0.0); n],
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

    fn dist(a: (f32, f32), b: (f32, f32)) -> f32 {
        (a.0 - b.0).hypot(a.1 - b.1)
    }

    #[test]
    fn next_id_skips_existing() {
        assert_eq!(next_blend_id(&[]), 0);
        assert_eq!(next_blend_id(&[None, None]), 0);
        assert_eq!(next_blend_id(&[Some(0), None, Some(3)]), 4);
    }

    #[test]
    fn can_make_needs_two_unblended() {
        let none = [None, None, None];
        assert!(!can_make(&none, &[0]));
        assert!(!can_make(&none, &[0, 0])); // duplicate collapses to one
        assert!(can_make(&none, &[0, 2]));
        assert!(!can_make(&none, &[0, 1, 2])); // three is too many this pass
        let some = [Some(0), None, None];
        assert!(!can_make(&some, &[0, 1])); // 0 already in a blend
    }

    #[test]
    fn can_release_detects_a_member() {
        let tags = [Some(0), None, Some(0)];
        assert!(can_release(&tags, &[1, 2]));
        assert!(!can_release(&tags, &[1]));
        assert_eq!(selected_blend_ids(&tags, &[0, 1, 2]), vec![0]);
    }

    #[test]
    fn resample_yields_n_points_on_a_line() {
        // A straight open path from (0,0) to (10,0): N evenly-spaced samples must
        // lie on the segment, endpoints included.
        let line = open_path(vec![(0.0, 0.0), (10.0, 0.0)]);
        let pts = resample(&line, 5);
        assert_eq!(pts.len(), 5);
        assert!(dist(pts[0], (0.0, 0.0)) < 1e-3);
        assert!(dist(pts[4], (10.0, 0.0)) < 1e-3);
        // Even arc-length spacing → x = 0, 2.5, 5, 7.5, 10.
        for (i, p) in pts.iter().enumerate() {
            let expect = i as f32 / 4.0 * 10.0;
            assert!((p.0 - expect).abs() < 1e-2, "x[{i}]={} != {expect}", p.0);
            assert!(p.1.abs() < 1e-3);
        }
    }

    #[test]
    fn resample_circle_points_lie_on_the_circle() {
        // An ellipse (circle r=50 centred at (100,100)); every resample point must
        // sit on the circle to sub-pixel accuracy.
        let circle = Shape::Ellipse {
            rect: [50.0, 50.0, 100.0, 100.0],
            fill: [0.0; 4],
            fill_gradient: None,
            stroke: [0.0; 4],
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
        };
        let pts = resample(&circle, 16);
        assert_eq!(pts.len(), 16);
        for p in &pts {
            let r = ((p.0 - 100.0).powi(2) + (p.1 - 100.0).powi(2)).sqrt();
            assert!((r - 50.0).abs() < 0.5, "r={r} not ~50");
        }
    }

    #[test]
    fn interpolating_identical_shapes_reproduces_them() {
        let r = rect(0.0, 0.0, 10.0, 10.0);
        let mid = interpolate_shape(&r, &r, 0.5);
        // Resampling the same rect twice and averaging must give the same points.
        let n = common_sample_count(&r, &r);
        let base = resample(&r, n);
        if let Shape::Path { points, .. } = &mid {
            assert_eq!(points.len(), base.len());
            for (p, q) in points.iter().zip(base.iter()) {
                assert!(dist(*p, *q) < 1e-3);
            }
        } else {
            panic!("expected a Path");
        }
        // Endpoints t=0 and t=1 reproduce a and b exactly.
        for t in [0.0, 1.0] {
            let s = interpolate_shape(&r, &r, t);
            if let Shape::Path { points, .. } = &s {
                for (p, q) in points.iter().zip(base.iter()) {
                    assert!(dist(*p, *q) < 1e-3);
                }
            }
        }
    }

    #[test]
    fn middle_is_geometric_midpoint_of_two_lines() {
        // Two parallel horizontal segments; the t=0.5 blend's points are the
        // midpoints (every resample point averages to the centre line).
        let a = open_path(vec![(0.0, 0.0), (10.0, 0.0)]);
        let b = open_path(vec![(0.0, 20.0), (10.0, 20.0)]);
        let mid = interpolate_shape(&a, &b, 0.5);
        if let Shape::Path { points, .. } = &mid {
            for p in points {
                assert!((p.1 - 10.0).abs() < 1e-2, "y={} != 10", p.1);
            }
        } else {
            panic!("expected a Path");
        }
    }

    #[test]
    fn color_interpolation_midpoint_is_correct() {
        // A red rect blended with a blue rect: the t=0.5 fill is purple, the
        // opacity averages, and the stroke width averages.
        let mut red = rect(0.0, 0.0, 10.0, 10.0);
        red.set_fill_color([1.0, 0.0, 0.0, 1.0]);
        red.set_stroke_width(2.0);
        let mut blue = rect(0.0, 0.0, 10.0, 10.0);
        blue.set_fill_color([0.0, 0.0, 1.0, 0.0]);
        blue.set_stroke_width(8.0);
        let mid = interpolate_shape(&red, &blue, 0.5);
        let f = mid.fill_color().unwrap();
        assert!((f[0] - 0.5).abs() < 1e-3);
        assert!(f[1].abs() < 1e-3);
        assert!((f[2] - 0.5).abs() < 1e-3);
        assert!((f[3] - 0.5).abs() < 1e-3); // alpha (opacity) averages
        assert!((mid.stroke_width() - 5.0).abs() < 1e-3);
    }

    #[test]
    fn make_steps_spaces_t_evenly_and_excludes_ends() {
        // Three steps between two lines 30 apart → y at 7.5, 15, 22.5.
        let a = open_path(vec![(0.0, 0.0), (10.0, 0.0)]);
        let b = open_path(vec![(0.0, 30.0), (10.0, 30.0)]);
        let steps = make_steps(&a, &b, 3);
        assert_eq!(steps.len(), 3);
        let ys: Vec<f32> = steps
            .iter()
            .map(|s| match s {
                Shape::Path { points, .. } => points[0].1,
                _ => panic!("expected Path"),
            })
            .collect();
        assert!((ys[0] - 7.5).abs() < 1e-2);
        assert!((ys[1] - 15.0).abs() < 1e-2);
        assert!((ys[2] - 22.5).abs() < 1e-2);
        // Zero steps yields nothing.
        assert!(make_steps(&a, &b, 0).is_empty());
    }

    #[test]
    fn closed_blends_stay_closed_open_blends_stay_open() {
        let ra = rect(0.0, 0.0, 10.0, 10.0);
        let rb = rect(20.0, 20.0, 10.0, 10.0);
        match interpolate_shape(&ra, &rb, 0.5) {
            Shape::Path { closed, .. } => assert!(closed),
            _ => panic!(),
        }
        let la = open_path(vec![(0.0, 0.0), (10.0, 0.0)]);
        let lb = open_path(vec![(0.0, 10.0), (10.0, 10.0)]);
        match interpolate_shape(&la, &lb, 0.5) {
            Shape::Path { closed, .. } => assert!(!closed),
            _ => panic!(),
        }
    }
}
