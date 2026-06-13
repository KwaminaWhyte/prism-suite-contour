//! Parametric "live" shapes — polygon & star primitives.
//!
//! A live shape is a [`Shape::Path`](crate::document::Shape::Path) whose closed
//! outline is *generated* from a small parameter set rather than drawn by hand.
//! The generated anchor `points` (and zeroed corner `handles`) live on the path
//! exactly like any other polyline, so every render surface, hit-test, boolean
//! op, and exporter treats a polygon / star like an ordinary path with no
//! special cases. The parameters ride along in an additive
//! [`Path::live`](crate::document::Shape::Path) field (`#[serde(default)]` →
//! `None`), so older `.contour` files load unchanged and editing a count /
//! radius re-runs [`LiveShape::outline`] to refresh the geometry live — the same
//! cache-from-params idea text type uses (`params` → `glyphs`).
//!
//! Geometry generation is a pure function (no egui / document state) so it is
//! unit-testable headlessly. Vertices are emitted clockwise starting at the
//! top (12 o'clock), matching how Illustrator orients a fresh polygon / star.

use serde::{Deserialize, Serialize};
use std::f32::consts::PI;

/// A generated closed outline: `(anchor points, per-anchor out-tangent
/// handles)`, in document space. A live shape is straight-edged so the handles
/// are all `(0.0, 0.0)` (corner anchors) — the shape matches a [`Shape::Path`]'s
/// `(points, handles)` pair so it can be assigned straight onto one.
///
/// [`Shape::Path`]: crate::document::Shape::Path
pub type Outline = (Vec<(f32, f32)>, Vec<(f32, f32)>);

/// The smallest sensible polygon (a triangle) / star (a 3-point star).
pub const MIN_SIDES: u32 = 3;
/// An upper bound that keeps the generated point count (and the inspector
/// sliders) reasonable; nothing breaks above it, it just gets dense.
pub const MAX_SIDES: u32 = 100;

/// The parameters of a parametric closed shape. Centre is *not* stored here — it
/// is derived from the generated points' bounding box on demand (so a moved /
/// transformed shape needs no parameter fix-up), and supplied to
/// [`outline`](Self::outline) when regenerating.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum LiveShape {
    /// A regular N-gon: `sides` vertices evenly spaced on a circle of `radius`.
    Polygon {
        /// Number of sides / vertices (clamped to `MIN_SIDES..=MAX_SIDES`).
        sides: u32,
        /// Circumscribed-circle radius, in document units.
        radius: f32,
    },
    /// A star: `points` outer tips on `radius`, alternating with `points` inner
    /// vertices on `radius * inner_ratio`.
    Star {
        /// Number of star points (clamped to `MIN_SIDES..=MAX_SIDES`); the
        /// generated outline has `2 * points` vertices.
        points: u32,
        /// Outer (tip) radius, in document units.
        radius: f32,
        /// Inner radius as a fraction of `radius`, in `0.05..=1.0`. `0.5` is a
        /// typical 5-point star; `1.0` degenerates to a `2*points`-gon.
        inner_ratio: f32,
    },
}

impl LiveShape {
    /// Generate the closed outline about `center`, returning `(points, handles)`
    /// where `handles` is all-zero (corner anchors — a polygon / star is
    /// straight-edged). This is the one pure entry point both creation and the
    /// inspector regenerate through.
    ///
    /// Vertices start at 12 o'clock and wind clockwise (screen-y-down). The
    /// counts and radii are clamped so a hand-edited file with silly values
    /// still yields a valid closed ring.
    pub fn outline(&self, center: (f32, f32)) -> Outline {
        let (cx, cy) = center;
        let pts = match *self {
            LiveShape::Polygon { sides, radius } => {
                let n = sides.clamp(MIN_SIDES, MAX_SIDES);
                let r = radius.max(0.0);
                (0..n)
                    .map(|i| {
                        let a = vertex_angle(i, n);
                        (cx + r * a.cos(), cy + r * a.sin())
                    })
                    .collect::<Vec<_>>()
            }
            LiveShape::Star {
                points,
                radius,
                inner_ratio,
            } => {
                let n = points.clamp(MIN_SIDES, MAX_SIDES);
                let r_outer = radius.max(0.0);
                let r_inner = r_outer * inner_ratio.clamp(0.05, 1.0);
                // 2*n vertices: even index = outer tip, odd index = inner notch,
                // each pair half a sector apart so tips/notches alternate.
                (0..2 * n)
                    .map(|i| {
                        let a = vertex_angle(i, 2 * n);
                        let r = if i % 2 == 0 { r_outer } else { r_inner };
                        (cx + r * a.cos(), cy + r * a.sin())
                    })
                    .collect::<Vec<_>>()
            }
        };
        let handles = vec![(0.0, 0.0); pts.len()];
        (pts, handles)
    }

    /// Human label for the layer list / inspector.
    pub fn label(&self) -> &'static str {
        match self {
            LiveShape::Polygon { .. } => "Polygon",
            LiveShape::Star { .. } => "Star",
        }
    }
}

/// Angle (radians) of vertex `i` of `n`, starting at 12 o'clock and winding
/// clockwise in screen space (y down). `-PI/2` puts vertex 0 straight up.
fn vertex_angle(i: u32, n: u32) -> f32 {
    -PI / 2.0 + (i as f32) * (2.0 * PI / n as f32)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The Euclidean distance from `c` to `p`.
    fn dist(c: (f32, f32), p: (f32, f32)) -> f32 {
        ((p.0 - c.0).powi(2) + (p.1 - c.1).powi(2)).sqrt()
    }

    #[test]
    fn polygon_emits_one_vertex_per_side() {
        for sides in [3u32, 4, 5, 6, 8, 12] {
            let ls = LiveShape::Polygon {
                sides,
                radius: 50.0,
            };
            let (pts, handles) = ls.outline((0.0, 0.0));
            assert_eq!(pts.len(), sides as usize);
            // Handles are zeroed corner anchors, one per point.
            assert_eq!(handles.len(), pts.len());
            assert!(handles.iter().all(|&h| h == (0.0, 0.0)));
        }
    }

    #[test]
    fn polygon_vertices_lie_on_the_radius() {
        let c = (10.0, -7.0);
        let ls = LiveShape::Polygon {
            sides: 7,
            radius: 33.0,
        };
        let (pts, _) = ls.outline(c);
        for p in &pts {
            assert!((dist(c, *p) - 33.0).abs() < 1e-3, "vertex off the radius");
        }
    }

    #[test]
    fn polygon_first_vertex_is_straight_up() {
        let ls = LiveShape::Polygon {
            sides: 5,
            radius: 20.0,
        };
        let (pts, _) = ls.outline((0.0, 0.0));
        // 12 o'clock in screen space (y down) is directly above the centre.
        assert!((pts[0].0 - 0.0).abs() < 1e-3);
        assert!((pts[0].1 - (-20.0)).abs() < 1e-3);
    }

    #[test]
    fn radius_scales_vertices_linearly() {
        let small = LiveShape::Polygon {
            sides: 6,
            radius: 10.0,
        }
        .outline((0.0, 0.0))
        .0;
        let big = LiveShape::Polygon {
            sides: 6,
            radius: 30.0,
        }
        .outline((0.0, 0.0))
        .0;
        for (s, b) in small.iter().zip(&big) {
            assert!((b.0 - s.0 * 3.0).abs() < 1e-3);
            assert!((b.1 - s.1 * 3.0).abs() < 1e-3);
        }
    }

    #[test]
    fn star_has_double_the_vertices_and_alternates_radii() {
        let c = (0.0, 0.0);
        let ls = LiveShape::Star {
            points: 5,
            radius: 40.0,
            inner_ratio: 0.5,
        };
        let (pts, _) = ls.outline(c);
        assert_eq!(pts.len(), 10);
        for (i, p) in pts.iter().enumerate() {
            let want = if i % 2 == 0 { 40.0 } else { 20.0 };
            assert!(
                (dist(c, *p) - want).abs() < 1e-3,
                "vertex {i} radius {} != {want}",
                dist(c, *p)
            );
        }
    }

    #[test]
    fn star_inner_ratio_one_is_a_regular_polygon() {
        let ls = LiveShape::Star {
            points: 4,
            radius: 25.0,
            inner_ratio: 1.0,
        };
        let (pts, _) = ls.outline((0.0, 0.0));
        for p in &pts {
            assert!((dist((0.0, 0.0), *p) - 25.0).abs() < 1e-3);
        }
    }

    #[test]
    fn counts_are_clamped_to_the_valid_range() {
        // Below the minimum.
        let tiny = LiveShape::Polygon {
            sides: 1,
            radius: 10.0,
        };
        assert_eq!(tiny.outline((0.0, 0.0)).0.len(), MIN_SIDES as usize);
        // Above the maximum.
        let huge = LiveShape::Polygon {
            sides: 10_000,
            radius: 10.0,
        };
        assert_eq!(huge.outline((0.0, 0.0)).0.len(), MAX_SIDES as usize);
    }

    #[test]
    fn generation_is_deterministic() {
        let ls = LiveShape::Star {
            points: 6,
            radius: 17.5,
            inner_ratio: 0.4,
        };
        let a = ls.outline((3.0, 4.0));
        let b = ls.outline((3.0, 4.0));
        assert_eq!(a, b);
    }

    #[test]
    fn translated_center_offsets_every_vertex() {
        let ls = LiveShape::Polygon {
            sides: 5,
            radius: 12.0,
        };
        let base = ls.outline((0.0, 0.0)).0;
        let moved = ls.outline((100.0, -50.0)).0;
        for (b, m) in base.iter().zip(&moved) {
            assert!((m.0 - (b.0 + 100.0)).abs() < 1e-3);
            assert!((m.1 - (b.1 - 50.0)).abs() < 1e-3);
        }
    }
}
