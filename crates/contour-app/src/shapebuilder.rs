//! The **Shape Builder** tool — interactive merge / subtract by dragging across
//! overlapping regions, à la Illustrator.
//!
//! The user selects two or more overlapping shapes, then drags across the
//! regions: a plain drag **unites** every region the pointer crosses into one
//! path; an Alt/Option drag **deletes** the crossed regions. This module holds
//! the pure geometry — building the *region graph* (the atomic faces the selected
//! shapes partition the plane into), hit-testing which face a point is over, and
//! turning a set of picked faces into a merge / subtract result. It reuses the
//! `i_overlay` boolean backend (the same crate the Pathfinder runs on); the app
//! layer drives the pointer interaction and the undo step.
//!
//! Everything here is unit-tested without any egui / GPU context.

use crate::document::{point_in_rings, FillRule, Shape, SubPath};
use i_overlay::core::fill_rule::FillRule as IFillRule;
use i_overlay::core::overlay_rule::OverlayRule;
use i_overlay::float::single::SingleFloatOverlay;

/// One atomic **face** of the selected shapes' arrangement: a maximal region that
/// lies inside the same subset of the input shapes (so no input edge passes
/// through it). Carried as its outer ring plus any hole rings (document space),
/// and the index of the back-most input shape that covers it (so a merged result
/// can inherit that shape's paint, the way Illustrator colours a Shape-Builder
/// result with the back object's appearance).
#[derive(Clone, Debug, PartialEq)]
pub struct Face {
    /// Outer ring first, then any holes (the `i_overlay` contour order).
    pub rings: Vec<Vec<(f32, f32)>>,
    /// Index (into the `shapes` slice passed to [`build_faces`]) of the lowest
    /// (back-most) input shape covering this face, for paint inheritance.
    pub owner: usize,
}

impl Face {
    /// Whether `(x, y)` is inside this face (its outer ring minus its holes).
    pub fn contains(&self, x: f32, y: f32) -> bool {
        // A face is a simple region (outer + holes); even-odd carves the holes.
        point_in_rings(x, y, &self.rings, FillRule::EvenOdd)
    }

    /// This face's net signed area magnitude (outer minus holes), used to drop
    /// slivers and to order faces.
    pub fn area(&self) -> f32 {
        let mut a: Vec<f32> = self.rings.iter().map(|r| ring_area(r)).collect();
        a.sort_by(|x, y| y.partial_cmp(x).unwrap_or(std::cmp::Ordering::Equal));
        let outer = a.first().copied().unwrap_or(0.0);
        let holes: f32 = a.iter().skip(1).sum();
        (outer - holes).max(0.0)
    }

    /// This face as document `SubPath`s (closed corner rings).
    fn to_subpaths(&self) -> Vec<SubPath> {
        self.rings.iter().cloned().map(SubPath::ring).collect()
    }
}

/// Area magnitude (shoelace) of one ring.
fn ring_area(pts: &[(f32, f32)]) -> f32 {
    let n = pts.len();
    if n < 3 {
        return 0.0;
    }
    let mut a = 0.0;
    for i in 0..n {
        let (x0, y0) = pts[i];
        let (x1, y1) = pts[(i + 1) % n];
        a += x0 * y1 - x1 * y0;
    }
    (a * 0.5).abs()
}

/// Faces whose net area is below this (document-unit²) are dropped as numerical
/// slivers from the overlay.
const MIN_FACE_AREA: f32 = 1e-3;

fn to_i(ring: &[(f32, f32)]) -> Vec<[f64; 2]> {
    ring.iter().map(|&(x, y)| [x as f64, y as f64]).collect()
}

fn from_i(shapes: Vec<Vec<Vec<[f64; 2]>>>) -> Vec<Vec<Vec<(f32, f32)>>> {
    shapes
        .into_iter()
        .map(|shape| {
            shape
                .into_iter()
                .filter(|c| c.len() >= 3)
                .map(|c| c.iter().map(|p| (p[0] as f32, p[1] as f32)).collect())
                .collect::<Vec<Vec<(f32, f32)>>>()
        })
        .filter(|s| !s.is_empty())
        .collect()
}

/// `subj − clip` as grouped regions (outer + holes per region).
fn difference(subj: &[Vec<(f32, f32)>], clip: &[Vec<(f32, f32)>]) -> Vec<Vec<Vec<(f32, f32)>>> {
    let s: Vec<Vec<[f64; 2]>> = subj.iter().map(|r| to_i(r)).collect();
    let c: Vec<Vec<[f64; 2]>> = clip.iter().map(|r| to_i(r)).collect();
    from_i(s.overlay(&c, OverlayRule::Difference, IFillRule::NonZero))
}

/// `subj ∩ clip` as grouped regions.
fn intersect(subj: &[Vec<(f32, f32)>], clip: &[Vec<(f32, f32)>]) -> Vec<Vec<Vec<(f32, f32)>>> {
    let s: Vec<Vec<[f64; 2]>> = subj.iter().map(|r| to_i(r)).collect();
    let c: Vec<Vec<[f64; 2]>> = clip.iter().map(|r| to_i(r)).collect();
    from_i(s.overlay(&c, OverlayRule::Intersect, IFillRule::NonZero))
}

/// Union of `polys` (each a single ring) into grouped regions.
fn union_all(polys: &[Vec<(f32, f32)>]) -> Vec<Vec<Vec<(f32, f32)>>> {
    if polys.is_empty() {
        return Vec::new();
    }
    let subj: Vec<Vec<[f64; 2]>> = polys.iter().map(|r| to_i(r)).collect();
    let empty: Vec<Vec<[f64; 2]>> = Vec::new();
    from_i(subj.overlay(&empty, OverlayRule::Union, IFillRule::NonZero))
}

/// Build the **region graph** — the atomic faces — of the selected shapes.
///
/// Each shape is flattened to its outer ring. Faces are computed by iteratively
/// subdividing: starting with the first shape's region, each subsequent shape `S`
/// splits every existing face `F` into `F ∩ S` (now also covered by `S`) and
/// `F − S` (unchanged), and adds `S − (everything so far)` as new faces only
/// covered by `S`. The result is the set of maximal regions each lying inside a
/// fixed subset of the inputs (for two overlapping shapes: `A∩B`, `A−B`, `B−A`).
///
/// Each face records `owner` = the lowest input-shape index covering it, so a
/// merge can inherit the back object's paint. Faces below [`MIN_FACE_AREA`] are
/// dropped. Shapes with no fillable closed outline are skipped (they cannot
/// contribute a region).
pub fn build_faces(shapes: &[Shape]) -> Vec<Face> {
    // Flattened outer ring per usable shape, paired with its original index.
    let polys: Vec<(usize, Vec<(f32, f32)>)> = shapes
        .iter()
        .enumerate()
        .filter_map(|(i, s)| s.outline_polygon().map(|p| (i, p)))
        .filter(|(_, p)| p.len() >= 3)
        .collect();
    if polys.is_empty() {
        return Vec::new();
    }

    // Faces accumulate as (rings, owner). Seed with the first shape's region(s).
    let mut faces: Vec<Face> = Vec::new();
    let (first_idx, first_poly) = &polys[0];
    for region in union_all(std::slice::from_ref(first_poly)) {
        faces.push(Face {
            rings: region,
            owner: *first_idx,
        });
    }
    // Running union of all shapes processed so far (as a list of rings), so the
    // "new region only under S" piece is `S − running_union`.
    let mut covered: Vec<Vec<(f32, f32)>> = first_poly_rings(&faces);

    for (idx, poly) in polys.iter().skip(1) {
        let s_rings = vec![poly.clone()];
        let mut next: Vec<Face> = Vec::new();
        for face in &faces {
            // F ∩ S : now also covered by S → owner stays the back-most (existing,
            // since existing faces all have lower or equal index than `idx`).
            for region in intersect(&face.rings, &s_rings) {
                if region_area(&region) >= MIN_FACE_AREA {
                    next.push(Face {
                        rings: region,
                        owner: face.owner,
                    });
                }
            }
            // F − S : unchanged.
            for region in difference(&face.rings, &s_rings) {
                if region_area(&region) >= MIN_FACE_AREA {
                    next.push(Face {
                        rings: region,
                        owner: face.owner,
                    });
                }
            }
        }
        // S − covered : the part of S not over any existing face → owned by S.
        for region in difference(&s_rings, &covered) {
            if region_area(&region) >= MIN_FACE_AREA {
                next.push(Face {
                    rings: region,
                    owner: *idx,
                });
            }
        }
        faces = next;
        // Extend the running union with S.
        covered.push(poly.clone());
        covered = flatten_union(&covered);
    }
    faces
}

/// The outer rings of the seeded faces (the first shape's region) as a ring list.
fn first_poly_rings(faces: &[Face]) -> Vec<Vec<(f32, f32)>> {
    faces.iter().flat_map(|f| f.rings.clone()).collect()
}

/// Collapse a list of rings into the outer rings of their union (so the running
/// "covered" set stays a simple ring list for the next difference).
fn flatten_union(rings: &[Vec<(f32, f32)>]) -> Vec<Vec<(f32, f32)>> {
    union_all(rings).into_iter().flatten().collect()
}

/// Net area of a grouped region (outer minus holes).
fn region_area(region: &[Vec<(f32, f32)>]) -> f32 {
    let mut a: Vec<f32> = region.iter().map(|r| ring_area(r)).collect();
    a.sort_by(|x, y| y.partial_cmp(x).unwrap_or(std::cmp::Ordering::Equal));
    let outer = a.first().copied().unwrap_or(0.0);
    let holes: f32 = a.iter().skip(1).sum();
    (outer - holes).max(0.0)
}

/// The index of the face under document point `(x, y)`, if any. The smallest-area
/// covering face wins, so a face nested inside another (a hole region) is picked
/// over its surrounding face.
pub fn face_at(faces: &[Face], x: f32, y: f32) -> Option<usize> {
    faces
        .iter()
        .enumerate()
        .filter(|(_, f)| f.contains(x, y))
        .min_by(|(_, a), (_, b)| {
            a.area()
                .partial_cmp(&b.area())
                .unwrap_or(std::cmp::Ordering::Equal)
        })
        .map(|(i, _)| i)
}

/// The set of face indices a sampled drag path crosses, in first-touched order
/// (de-duplicated), so the gesture acts on every region the pointer dragged over.
pub fn faces_along(faces: &[Face], path: &[(f32, f32)]) -> Vec<usize> {
    let mut out: Vec<usize> = Vec::new();
    for &(x, y) in path {
        if let Some(i) = face_at(faces, x, y) {
            if !out.contains(&i) {
                out.push(i);
            }
        }
    }
    out
}

/// What the Shape Builder does with the picked faces on release.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum BuildMode {
    /// Unite the picked faces into one path (plain drag).
    Unite,
    /// Delete the picked faces, keeping the rest (Alt / Option drag).
    Subtract,
}

/// Apply the Shape Builder gesture: given the full face list and the picked face
/// indices, return the **replacement shapes** for the selection.
///
/// - [`BuildMode::Unite`]: the picked faces are unioned into one shape (a `Path`,
///   or a `Compound` if the union has holes), taking the back-most picked face's
///   owner shape's paint; every *unpicked* face stays its own shape (so the rest
///   of the artwork is preserved as the partition).
/// - [`BuildMode::Subtract`]: the picked faces are dropped; every unpicked face
///   becomes its own shape.
///
/// `shapes` is the selected shapes (used for paint inheritance via `owner`).
/// Returns the new shapes that should replace the selection.
pub fn apply_build(
    shapes: &[Shape],
    faces: &[Face],
    picked: &[usize],
    mode: BuildMode,
) -> Vec<Shape> {
    let picked_set: std::collections::BTreeSet<usize> = picked.iter().copied().collect();
    let mut out: Vec<Shape> = Vec::new();

    match mode {
        BuildMode::Subtract => {
            // Keep every unpicked face as its own shape.
            for (i, face) in faces.iter().enumerate() {
                if !picked_set.contains(&i) {
                    out.push(face_to_shape(face, shapes));
                }
            }
        }
        BuildMode::Unite => {
            if picked.is_empty() {
                // Nothing picked: leave the partition as-is (every face a shape).
                for face in faces {
                    out.push(face_to_shape(face, shapes));
                }
                return out;
            }
            // Union the picked faces' rings into one region set.
            let picked_rings: Vec<Vec<(f32, f32)>> = picked
                .iter()
                .filter_map(|&i| faces.get(i))
                .flat_map(|f| f.rings.clone())
                .collect();
            // Back-most picked owner supplies the merged paint.
            let owner = picked
                .iter()
                .filter_map(|&i| faces.get(i).map(|f| f.owner))
                .min()
                .unwrap_or(0);
            let style = shapes.get(owner);
            for region in union_all(&picked_rings) {
                out.push(region_to_shape(&region, style));
            }
            // Every unpicked face stays its own shape.
            for (i, face) in faces.iter().enumerate() {
                if !picked_set.contains(&i) {
                    out.push(face_to_shape(face, shapes));
                }
            }
        }
    }
    out
}

/// Turn a face into a document shape (a `Path` for a single ring, a `Compound`
/// for a ring-with-holes), inheriting its owner shape's paint.
fn face_to_shape(face: &Face, shapes: &[Shape]) -> Shape {
    let style = shapes.get(face.owner);
    region_to_shape_subpaths(face.to_subpaths(), style)
}

/// Turn a grouped region (outer + holes) into a shape, inheriting `style`'s paint.
fn region_to_shape(region: &[Vec<(f32, f32)>], style: Option<&Shape>) -> Shape {
    let subs: Vec<SubPath> = region.iter().cloned().map(SubPath::ring).collect();
    region_to_shape_subpaths(subs, style)
}

/// Build a `Path` (one sub-contour) or `Compound` (several) from sub-contours,
/// inheriting `style`'s paint (fill / gradient / stroke / appearance) or a sane
/// neutral default when there is no style shape.
fn region_to_shape_subpaths(subpaths: Vec<SubPath>, style: Option<&Shape>) -> Shape {
    let fill = style.and_then(|s| s.fill_color()).unwrap_or([0.5, 0.5, 0.5, 1.0]);
    let fill_gradient = style.and_then(|s| s.fill_gradient().cloned());
    let stroke = style.and_then(|s| s.stroke_color()).unwrap_or([0.0, 0.0, 0.0, 1.0]);
    let stroke_w = style.map(|s| s.stroke_width()).unwrap_or(1.0);
    let stroke_style = style
        .map(|s| s.stroke_style().clone())
        .unwrap_or_default();
    let appearance = style.and_then(|s| s.appearance().cloned());

    if subpaths.len() == 1 {
        let sp = subpaths.into_iter().next().unwrap();
        Shape::Path {
            points: sp.points,
            closed: true,
            fill,
            fill_gradient,
            stroke,
            stroke_w,
            stroke_style,
            appearance,
            handles: sp.handles,
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
    } else {
        Shape::Compound {
            subpaths,
            fill_rule: FillRule::EvenOdd,
            fill,
            fill_gradient,
            stroke,
            stroke_w,
            stroke_style,
            appearance,
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::StrokeStyle;

    fn rect(x: f32, y: f32, w: f32, h: f32, fill: [f32; 4]) -> Shape {
        Shape::Rect {
            rect: [x, y, w, h],
            fill,
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

    fn shape_area(s: &Shape) -> f32 {
        match s {
            Shape::Path { points, .. } => ring_area(points),
            Shape::Compound { subpaths, .. } => {
                let mut a: Vec<f32> = subpaths.iter().map(|sp| ring_area(&sp.flatten())).collect();
                a.sort_by(|x, y| y.partial_cmp(x).unwrap());
                let outer = a.first().copied().unwrap_or(0.0);
                let holes: f32 = a.iter().skip(1).sum();
                (outer - holes).max(0.0)
            }
            _ => 0.0,
        }
    }

    fn total_area(shapes: &[Shape]) -> f32 {
        shapes.iter().map(shape_area).sum()
    }

    /// Two overlapping rects partition into exactly three atomic faces
    /// (A−B, A∩B, B−A) tiling their union.
    #[test]
    fn two_overlapping_rects_make_three_faces() {
        let a = rect(0.0, 0.0, 10.0, 10.0, [1.0, 0.0, 0.0, 1.0]);
        let b = rect(5.0, 5.0, 10.0, 10.0, [0.0, 0.0, 1.0, 1.0]);
        let faces = build_faces(&[a, b]);
        assert_eq!(faces.len(), 3, "A−B, A∩B, B−A");
        // The three faces tile the union (175) with no double-count.
        let total: f32 = faces.iter().map(|f| f.area()).sum();
        assert!((total - 175.0).abs() < 0.5, "faces tile the union, got {total}");
        // Areas are 75, 25, 75 in some order.
        let mut areas: Vec<f32> = faces.iter().map(|f| f.area()).collect();
        areas.sort_by(|x, y| x.partial_cmp(y).unwrap());
        assert!((areas[0] - 25.0).abs() < 0.5, "overlap face is 25");
        assert!((areas[1] - 75.0).abs() < 0.5);
        assert!((areas[2] - 75.0).abs() < 0.5);
    }

    /// Disjoint shapes make one face each (no intersection).
    #[test]
    fn disjoint_rects_make_two_faces() {
        let a = rect(0.0, 0.0, 10.0, 10.0, [1.0, 0.0, 0.0, 1.0]);
        let b = rect(100.0, 100.0, 10.0, 10.0, [0.0, 0.0, 1.0, 1.0]);
        let faces = build_faces(&[a, b]);
        assert_eq!(faces.len(), 2);
        for f in &faces {
            assert!((f.area() - 100.0).abs() < 0.5);
        }
    }

    /// The overlap face is picked at the shared centre; an A-only point picks the
    /// A−B face; a point outside everything picks nothing.
    #[test]
    fn face_at_picks_the_right_region() {
        let a = rect(0.0, 0.0, 10.0, 10.0, [1.0, 0.0, 0.0, 1.0]);
        let b = rect(5.0, 5.0, 10.0, 10.0, [0.0, 0.0, 1.0, 1.0]);
        let faces = build_faces(&[a, b]);
        // Centre of the overlap (7.5, 7.5): the smallest face (the 25 overlap).
        let i = face_at(&faces, 7.5, 7.5).expect("over the overlap");
        assert!((faces[i].area() - 25.0).abs() < 0.5);
        // A-only point (2, 2): a 75-area face.
        let j = face_at(&faces, 2.0, 2.0).expect("over A only");
        assert!((faces[j].area() - 75.0).abs() < 0.5);
        // Outside everything.
        assert!(face_at(&faces, 50.0, 50.0).is_none());
    }

    /// A drag path crossing A−B then A∩B picks both faces in order, de-duplicated.
    #[test]
    fn faces_along_collects_crossed_faces() {
        let a = rect(0.0, 0.0, 10.0, 10.0, [1.0, 0.0, 0.0, 1.0]);
        let b = rect(5.0, 5.0, 10.0, 10.0, [0.0, 0.0, 1.0, 1.0]);
        let faces = build_faces(&[a, b]);
        // Sample a path from deep in A-only into the overlap.
        let path = vec![(2.0, 2.0), (3.0, 3.0), (7.5, 7.5), (7.6, 7.6)];
        let picked = faces_along(&faces, &path);
        assert_eq!(picked.len(), 2, "two distinct faces crossed");
    }

    /// Unite merges the picked faces into one region; un-picked faces stay.
    #[test]
    fn unite_merges_picked_faces() {
        let a = rect(0.0, 0.0, 10.0, 10.0, [1.0, 0.0, 0.0, 1.0]);
        let b = rect(5.0, 5.0, 10.0, 10.0, [0.0, 0.0, 1.0, 1.0]);
        let faces = build_faces(&[a.clone(), b.clone()]);
        // Pick A−B (area 75) and the overlap (25): their union is A (area 100),
        // plus the leftover B−A (75) face stays separate.
        let mut picks: Vec<usize> = (0..faces.len())
            .filter(|&i| {
                // A−B and overlap are the two faces whose centroid is inside A.
                faces[i].contains(2.0, 2.0) || faces[i].contains(7.5, 7.5)
            })
            .collect();
        picks.sort_unstable();
        picks.dedup();
        let out = apply_build(&[a, b], &faces, &picks, BuildMode::Unite);
        // One merged shape (A = 100) + the leftover B−A (75) = 175 total area.
        assert!((total_area(&out) - 175.0).abs() < 0.5);
        // The merged A shape exists as a single 100-area shape.
        assert!(out.iter().any(|s| (shape_area(s) - 100.0).abs() < 0.5));
    }

    /// Subtract drops the picked faces, keeping the rest.
    #[test]
    fn subtract_drops_picked_faces() {
        let a = rect(0.0, 0.0, 10.0, 10.0, [1.0, 0.0, 0.0, 1.0]);
        let b = rect(5.0, 5.0, 10.0, 10.0, [0.0, 0.0, 1.0, 1.0]);
        let faces = build_faces(&[a.clone(), b.clone()]);
        // Delete the overlap (25-area face).
        let overlap = face_at(&faces, 7.5, 7.5).unwrap();
        let out = apply_build(&[a, b], &faces, &[overlap], BuildMode::Subtract);
        // Remaining = the two 75-area crescents = 150.
        assert!((total_area(&out) - 150.0).abs() < 0.5);
        assert_eq!(out.len(), 2, "two leftover faces");
    }

    /// A merged result inherits the back-most picked face's owner shape's fill.
    #[test]
    fn unite_inherits_back_owner_paint() {
        let a = rect(0.0, 0.0, 10.0, 10.0, [1.0, 0.0, 0.0, 1.0]); // back, red
        let b = rect(5.0, 5.0, 10.0, 10.0, [0.0, 0.0, 1.0, 1.0]); // front, blue
        let faces = build_faces(&[a.clone(), b.clone()]);
        // Pick the overlap (owned by A, the back shape) and B−A (owned by B).
        let mut picks = vec![face_at(&faces, 7.5, 7.5).unwrap()];
        picks.push(face_at(&faces, 12.0, 12.0).unwrap());
        let out = apply_build(&[a, b], &faces, &picks, BuildMode::Unite);
        // The merged region takes the back-most owner's (A = red) fill.
        let merged = out
            .iter()
            .max_by(|x, y| shape_area(x).partial_cmp(&shape_area(y)).unwrap())
            .unwrap();
        assert_eq!(merged.fill_color(), Some([1.0, 0.0, 0.0, 1.0]), "back paint");
    }
}
