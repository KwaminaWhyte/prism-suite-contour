//! The eyedropper: sampling a shape's paint **appearance** and applying it to
//! another shape (Illustrator / Affinity's `I` tool — "copy the look of that
//! object onto this one").
//!
//! An [`Appearance`] is the *paint* of a shape — its fill (solid or gradient),
//! stroke colour, stroke width, and stroke style — detached from the shape's
//! geometry, group membership, and visibility. Sampling reads an `Appearance`
//! off a shape; applying writes one onto a shape **without disturbing its
//! geometry** (so an ellipse stays an ellipse, only its colours change).
//!
//! Everything here is pure and unit-tested, away from any egui / canvas state:
//! the app's tool layer hit-tests which shape was clicked, then drives
//! [`Appearance::sample`] / [`Appearance::apply_to`].

use crate::document::{Shape, StrokeStyle};
use crate::gradient::Gradient;

/// The transferable paint of a shape: its fill (a solid colour and/or a
/// gradient that overrides it), its stroke colour and width, and its stroke
/// style (caps / joins / dashes). This is exactly the set of attributes the
/// eyedropper copies between objects — geometry, grouping, and visibility are
/// deliberately excluded.
#[derive(Clone, Debug, PartialEq)]
pub struct Appearance {
    /// Solid fill colour (straight sRGB RGBA). `None` for a shape with no fill
    /// region (a [`Shape::Line`]).
    pub fill: Option<[f32; 4]>,
    /// Gradient fill that overrides the solid colour when present.
    pub fill_gradient: Option<Gradient>,
    /// Stroke colour (straight sRGB RGBA).
    pub stroke: [f32; 4],
    /// Stroke width in document units.
    pub stroke_w: f32,
    /// Stroke caps / joins / dashes.
    pub stroke_style: StrokeStyle,
}

impl Appearance {
    /// Read the paint appearance off `shape`.
    pub fn sample(shape: &Shape) -> Self {
        Self {
            fill: shape.fill_color(),
            fill_gradient: shape.fill_gradient().cloned(),
            // Every shape variant has a stroke, so `stroke_color` is always Some;
            // fall back to opaque black only for a (impossible) None.
            stroke: shape.stroke_color().unwrap_or([0.0, 0.0, 0.0, 1.0]),
            stroke_w: shape.stroke_width(),
            stroke_style: shape.stroke_style().clone(),
        }
    }

    /// Apply this appearance onto `target`, leaving its geometry, group, and
    /// visibility untouched.
    ///
    /// A [`Shape::Line`] has no fill region, so its fill / gradient are skipped
    /// (it still takes the stroke). When the source had no solid fill (it was a
    /// line) but the target does, the target keeps its existing solid fill and
    /// only the gradient (cleared) and stroke transfer — matching Illustrator,
    /// where eyedropping a line onto a filled shape leaves the fill colour but
    /// removes any gradient and copies the stroke.
    pub fn apply_to(&self, target: &mut Shape) {
        if let Some(c) = self.fill {
            target.set_fill_color(c);
        }
        // The gradient is always synced (set or cleared) on shapes that can hold
        // one; `set_fill_gradient` is a no-op on a Line.
        target.set_fill_gradient(self.fill_gradient.clone());
        target.set_stroke_color(self.stroke);
        target.set_stroke_width(self.stroke_w);
        *target.stroke_style_mut() = self.stroke_style.clone();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::{LineCap, Shape, StrokeStyle};
    use crate::gradient::{Gradient, GradientKind};

    fn rect(fill: [f32; 4], stroke: [f32; 4], w: f32) -> Shape {
        Shape::Rect {
            rect: [10.0, 20.0, 30.0, 40.0],
            fill,
            fill_gradient: None,
            stroke,
            stroke_w: w,
            stroke_style: StrokeStyle::default(),
            visible: true,
            group: Some(7),
            clip: None,
            mask: false,
        }
    }

    fn ellipse() -> Shape {
        Shape::Ellipse {
            rect: [0.0, 0.0, 5.0, 5.0],
            fill: [0.0, 0.0, 0.0, 1.0],
            fill_gradient: None,
            stroke: [0.0, 0.0, 0.0, 1.0],
            stroke_w: 1.0,
            stroke_style: StrokeStyle::default(),
            visible: true,
            group: None,
            clip: None,
            mask: false,
        }
    }

    fn line() -> Shape {
        Shape::Line {
            p0: (0.0, 0.0),
            p1: (10.0, 10.0),
            stroke: [0.2, 0.4, 0.6, 1.0],
            stroke_w: 3.5,
            stroke_style: StrokeStyle::default(),
            visible: true,
            group: None,
            clip: None,
            mask: false,
        }
    }

    #[test]
    fn sample_reads_fill_stroke_and_width() {
        let src = rect([1.0, 0.0, 0.0, 1.0], [0.0, 1.0, 0.0, 1.0], 4.0);
        let a = Appearance::sample(&src);
        assert_eq!(a.fill, Some([1.0, 0.0, 0.0, 1.0]));
        assert_eq!(a.stroke, [0.0, 1.0, 0.0, 1.0]);
        assert_eq!(a.stroke_w, 4.0);
        assert!(a.fill_gradient.is_none());
    }

    #[test]
    fn apply_copies_paint_but_not_geometry_or_group() {
        let src = rect([1.0, 0.0, 0.0, 1.0], [0.0, 1.0, 0.0, 1.0], 6.0);
        let mut dst = ellipse();
        Appearance::sample(&src).apply_to(&mut dst);
        // Paint transferred.
        assert_eq!(dst.fill_color(), Some([1.0, 0.0, 0.0, 1.0]));
        assert_eq!(dst.stroke_color(), Some([0.0, 1.0, 0.0, 1.0]));
        assert_eq!(dst.stroke_width(), 6.0);
        // Geometry + grouping untouched: still an Ellipse, still ungrouped.
        assert!(matches!(dst, Shape::Ellipse { .. }));
        assert_eq!(dst.group(), None);
        assert_eq!(
            dst.bounds().map(|b| (b.x, b.y, b.w, b.h)),
            Some((0.0, 0.0, 5.0, 5.0))
        );
    }

    #[test]
    fn apply_transfers_gradient_and_clears_when_absent() {
        let grad = Gradient::two_stop(
            GradientKind::Radial,
            [1.0, 0.0, 0.0, 1.0],
            [0.0, 0.0, 1.0, 1.0],
        );
        // Source with a gradient → target gains it.
        let mut src = rect([1.0, 1.0, 1.0, 1.0], [0.0, 0.0, 0.0, 1.0], 2.0);
        src.set_fill_gradient(Some(grad.clone()));
        let mut dst = ellipse();
        Appearance::sample(&src).apply_to(&mut dst);
        assert_eq!(dst.fill_gradient().cloned(), Some(grad));

        // Now sample a solid (no-gradient) shape onto the gradient-filled dst:
        // the gradient is cleared.
        let solid = rect([0.5, 0.5, 0.5, 1.0], [0.0, 0.0, 0.0, 1.0], 1.0);
        Appearance::sample(&solid).apply_to(&mut dst);
        assert!(dst.fill_gradient().is_none());
        assert_eq!(dst.fill_color(), Some([0.5, 0.5, 0.5, 1.0]));
    }

    #[test]
    fn apply_transfers_stroke_style() {
        let mut src = rect([1.0, 0.0, 0.0, 1.0], [0.0, 0.0, 0.0, 1.0], 2.0);
        src.stroke_style_mut().cap = LineCap::Round;
        src.stroke_style_mut().dash = vec![12.0, 6.0];
        let mut dst = ellipse();
        Appearance::sample(&src).apply_to(&mut dst);
        assert_eq!(dst.stroke_style().cap, LineCap::Round);
        assert!(dst.stroke_style().is_dashed());
    }

    #[test]
    fn sample_line_has_no_fill() {
        let a = Appearance::sample(&line());
        assert_eq!(a.fill, None);
        assert_eq!(a.stroke, [0.2, 0.4, 0.6, 1.0]);
        assert_eq!(a.stroke_w, 3.5);
    }

    #[test]
    fn apply_line_appearance_keeps_target_fill_clears_gradient() {
        // Eyedropping a line (no fill) onto a filled shape with a gradient: the
        // solid fill colour is kept, the gradient removed, the stroke copied.
        let grad = Gradient::two_stop(
            GradientKind::Linear,
            [1.0, 0.0, 0.0, 1.0],
            [0.0, 0.0, 1.0, 1.0],
        );
        let mut dst = rect([0.9, 0.8, 0.7, 1.0], [1.0, 1.0, 1.0, 1.0], 1.0);
        dst.set_fill_gradient(Some(grad));
        Appearance::sample(&line()).apply_to(&mut dst);
        // Fill colour unchanged (the line carried no fill to overwrite it)…
        assert_eq!(dst.fill_color(), Some([0.9, 0.8, 0.7, 1.0]));
        // …gradient cleared, stroke taken from the line.
        assert!(dst.fill_gradient().is_none());
        assert_eq!(dst.stroke_color(), Some([0.2, 0.4, 0.6, 1.0]));
        assert_eq!(dst.stroke_width(), 3.5);
    }

    #[test]
    fn apply_to_line_takes_stroke_ignores_fill() {
        let src = rect([1.0, 0.0, 0.0, 1.0], [0.1, 0.2, 0.3, 1.0], 5.0);
        let mut dst = line();
        Appearance::sample(&src).apply_to(&mut dst);
        // A line has no fill region, so only the stroke transfers.
        assert!(matches!(dst, Shape::Line { .. }));
        assert_eq!(dst.fill_color(), None);
        assert_eq!(dst.stroke_color(), Some([0.1, 0.2, 0.3, 1.0]));
        assert_eq!(dst.stroke_width(), 5.0);
    }
}
