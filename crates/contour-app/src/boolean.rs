//! Boolean polygon operations (Union / Intersect / Difference) on two closed
//! shapes, powered by the `i_overlay` crate.
//!
//! Each input shape is flattened to a polygon (its outer ring) in document
//! space, the op runs in `f64`, and the largest resulting contour is wrapped
//! back into a closed [`Shape::Path`]. Style (fill/stroke) is inherited from the
//! subject shape.

use crate::document::{self, Shape};
use i_overlay::core::fill_rule::FillRule;
use i_overlay::core::overlay_rule::OverlayRule;
use i_overlay::float::single::SingleFloatOverlay;

/// Which boolean operation to perform.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum BoolOp {
    Union,
    Intersect,
    Difference,
}

impl BoolOp {
    fn rule(self) -> OverlayRule {
        match self {
            BoolOp::Union => OverlayRule::Union,
            BoolOp::Intersect => OverlayRule::Intersect,
            BoolOp::Difference => OverlayRule::Difference,
        }
    }
}

/// Flatten a shape to a single closed polygon (outer ring) in document space.
/// Returns `None` for shapes that aren't a sensible closed area.
fn shape_to_polygon(shape: &Shape) -> Option<Vec<[f64; 2]>> {
    let pts: Vec<(f32, f32)> = match shape {
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
            document::flatten(points, handles, true)
        }
        Shape::Line { .. } => return None,
    };
    if pts.len() < 3 {
        return None;
    }
    Some(pts.iter().map(|&(x, y)| [x as f64, y as f64]).collect())
}

/// Run `op` on two shapes, returning the result as a closed `Shape::Path`
/// (styled from `subj`). Returns `None` if either shape isn't a closed area or
/// the op yields no geometry.
pub fn apply(subj_shape: &Shape, clip_shape: &Shape, op: BoolOp) -> Option<Shape> {
    let subj = shape_to_polygon(subj_shape)?;
    let clip = shape_to_polygon(clip_shape)?;

    // `Shapes<P>` = Vec<Shape> = Vec<Vec<Contour>> = Vec<Vec<Vec<[f64;2]>>>.
    let shapes = subj.overlay(&clip, op.rule(), FillRule::NonZero);

    // Pick the contour with the most points (the dominant outer ring). Holes
    // and extra islands are dropped — the document model is single-ring paths.
    let best = shapes
        .into_iter()
        .flat_map(|shape| shape.into_iter())
        .max_by_key(|contour| contour.len())?;
    if best.len() < 3 {
        return None;
    }
    let points: Vec<(f32, f32)> = best.iter().map(|p| (p[0] as f32, p[1] as f32)).collect();

    let (fill, stroke, stroke_w) = match subj_shape {
        Shape::Rect {
            fill,
            stroke,
            stroke_w,
            ..
        }
        | Shape::Ellipse {
            fill,
            stroke,
            stroke_w,
            ..
        }
        | Shape::Path {
            fill,
            stroke,
            stroke_w,
            ..
        } => (*fill, *stroke, *stroke_w),
        Shape::Line {
            stroke, stroke_w, ..
        } => ([0.5, 0.5, 0.5, 1.0], *stroke, *stroke_w),
    };
    let stroke_style = subj_shape.stroke_style().clone();
    let fill_gradient = subj_shape.fill_gradient().cloned();
    // Carry the subject's stacked appearance onto the boolean result.
    let appearance = subj_shape.appearance().cloned();

    Some(Shape::Path {
        points,
        closed: true,
        fill,
        fill_gradient,
        stroke,
        stroke_w,
        stroke_style,
        appearance,
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
    })
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

    #[test]
    fn union_of_overlapping_rects() {
        let a = rect(0.0, 0.0, 10.0, 10.0);
        let b = rect(5.0, 5.0, 10.0, 10.0);
        let r = apply(&a, &b, BoolOp::Union).expect("union should produce geometry");
        match r {
            Shape::Path { points, closed, .. } => {
                assert!(closed);
                assert!(points.len() >= 6, "L-shape union ring");
            }
            _ => panic!("expected Path"),
        }
    }

    #[test]
    fn intersect_of_overlapping_rects() {
        let a = rect(0.0, 0.0, 10.0, 10.0);
        let b = rect(5.0, 5.0, 10.0, 10.0);
        let r = apply(&a, &b, BoolOp::Intersect).expect("intersection should produce geometry");
        assert!(matches!(r, Shape::Path { closed: true, .. }));
    }

    #[test]
    fn intersect_of_disjoint_rects_is_empty() {
        let a = rect(0.0, 0.0, 10.0, 10.0);
        let b = rect(100.0, 100.0, 10.0, 10.0);
        assert!(apply(&a, &b, BoolOp::Intersect).is_none());
    }
}
