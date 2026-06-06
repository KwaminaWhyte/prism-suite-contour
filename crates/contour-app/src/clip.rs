//! Clipping masks — Illustrator's `Object → Clipping Mask → Make / Release`.
//!
//! A **clip set** is a flat run of shapes that share an additive clip-set id
//! (`Shape::clip` → `Some(id)`), one of which is flagged the **mask**
//! (`Shape::is_mask`). The topmost shape of a selection becomes the mask; the
//! shapes below it are the *clipped content*. The mask's outline confines what
//! of the content is visible, exactly as in Illustrator — and because the mask
//! and the content keep their original geometry, the operation is fully
//! non-destructive: **Release** simply clears the tags and the originals reappear.
//!
//! Rendering is derived, never stored: [`clip_polygon`] intersects a content
//! shape's outline against the mask outline with `i_overlay`, yielding an
//! ordinary single-ring polygon that every render surface (canvas / PNG / SVG)
//! already knows how to draw. The mask shape itself paints nothing (an
//! Illustrator clipping path has no fill or stroke once it becomes a mask).
//!
//! Everything here is pure: the planning helpers work over a slice of
//! `(clip_id, is_mask)` tags (one per shape, in paint order) plus a selection
//! index set, and the geometry helper works over flat polygons — so all of it is
//! unit-testable without any `Shape` or egui context. Callers apply the returned
//! ids / index lists to the real document.

use i_overlay::core::fill_rule::FillRule;
use i_overlay::core::overlay_rule::OverlayRule;
use i_overlay::float::single::SingleFloatOverlay;

/// One shape's clip-set tag: which set it belongs to (if any) and whether it is
/// that set's masking path. Mirrors the additive `(clip, is_mask)` pair stored on
/// every [`Shape`](crate::document::Shape).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct ClipTag {
    pub clip: Option<u64>,
    pub is_mask: bool,
}

impl ClipTag {
    pub fn new(clip: Option<u64>, is_mask: bool) -> Self {
        Self { clip, is_mask }
    }
}

/// The smallest clip-set id not currently used by any tag, so a freshly-made
/// clip set never collides with an existing one. Ids are opaque; only equality
/// matters. (Independent of group ids — a shape can be both grouped and clipped.)
pub fn next_clip_id(tags: &[ClipTag]) -> u64 {
    tags.iter()
        .filter_map(|t| t.clip)
        .max()
        .map_or(0, |m| m + 1)
}

/// Whether a selection can be turned into a clipping mask: it must reference at
/// least two distinct in-range shapes (a mask plus at least one clipped shape),
/// and none of them may already belong to a clip set (Illustrator won't nest a
/// fresh mask straight onto an existing one). Returns `false` otherwise.
pub fn can_make(tags: &[ClipTag], selected: &[usize]) -> bool {
    let mut s: Vec<usize> = selected
        .iter()
        .copied()
        .filter(|&i| i < tags.len())
        .collect();
    s.sort_unstable();
    s.dedup();
    if s.len() < 2 {
        return false;
    }
    s.iter().all(|&i| tags[i].clip.is_none())
}

/// Whether the selection touches any clip set (so Release would do something).
pub fn can_release(tags: &[ClipTag], selected: &[usize]) -> bool {
    selected
        .iter()
        .any(|&i| tags.get(i).is_some_and(|t| t.clip.is_some()))
}

/// All indices belonging to the same clip set as shape `i` (including `i`), in
/// ascending order. A shape with no clip set returns just `[i]`; an out-of-range
/// index returns empty.
pub fn members_of(tags: &[ClipTag], i: usize) -> Vec<usize> {
    match tags.get(i).and_then(|t| t.clip) {
        Some(c) => tags
            .iter()
            .enumerate()
            .filter_map(|(j, t)| (t.clip == Some(c)).then_some(j))
            .collect(),
        None => {
            if i < tags.len() {
                vec![i]
            } else {
                Vec::new()
            }
        }
    }
}

/// The clip-set ids referenced by `selected`, ascending and de-duplicated.
pub fn selected_clip_ids(tags: &[ClipTag], selected: &[usize]) -> Vec<u64> {
    let mut ids: Vec<u64> = selected
        .iter()
        .filter_map(|&i| tags.get(i).and_then(|t| t.clip))
        .collect();
    ids.sort_unstable();
    ids.dedup();
    ids
}

/// Index of the mask shape of the clip set containing `i`, if any. Returns the
/// first (and normally only) member flagged `is_mask`.
pub fn mask_of(tags: &[ClipTag], i: usize) -> Option<usize> {
    let c = tags.get(i)?.clip?;
    tags.iter()
        .enumerate()
        .find_map(|(j, t)| (t.clip == Some(c) && t.is_mask).then_some(j))
}

/// Intersect content polygon `subject` against mask polygon `mask`, returning the
/// clipped outline as a single closed ring (the largest resulting contour, since
/// the document model stores single-ring paths). Returns `None` when the inputs
/// are degenerate or the intersection is empty (the content lies fully outside
/// the mask). Coordinates are document-space `(x, y)` in `f32`.
pub fn clip_polygon(subject: &[(f32, f32)], mask: &[(f32, f32)]) -> Option<Vec<(f32, f32)>> {
    if subject.len() < 3 || mask.len() < 3 {
        return None;
    }
    let subj: Vec<[f64; 2]> = subject.iter().map(|&(x, y)| [x as f64, y as f64]).collect();
    let clip: Vec<[f64; 2]> = mask.iter().map(|&(x, y)| [x as f64, y as f64]).collect();
    let shapes = subj.overlay(&clip, OverlayRule::Intersect, FillRule::NonZero);
    let best = shapes
        .into_iter()
        .flat_map(|shape| shape.into_iter())
        .max_by_key(|contour| contour.len())?;
    if best.len() < 3 {
        return None;
    }
    Some(best.iter().map(|p| (p[0] as f32, p[1] as f32)).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tag(clip: Option<u64>, mask: bool) -> ClipTag {
        ClipTag::new(clip, mask)
    }

    #[test]
    fn next_id_skips_existing() {
        assert_eq!(next_clip_id(&[]), 0);
        assert_eq!(next_clip_id(&[tag(None, false), tag(None, false)]), 0);
        assert_eq!(next_clip_id(&[tag(Some(0), true), tag(Some(0), false)]), 1);
        assert_eq!(next_clip_id(&[tag(Some(4), false)]), 5);
    }

    #[test]
    fn can_make_needs_two_unclipped_shapes() {
        let none = [tag(None, false), tag(None, false), tag(None, false)];
        assert!(!can_make(&none, &[]));
        assert!(!can_make(&none, &[1])); // one shape: nothing to clip
        assert!(!can_make(&none, &[1, 1])); // duplicate collapses
        assert!(can_make(&none, &[0, 2]));
        // Out-of-range indices don't count toward the minimum.
        assert!(!can_make(&none, &[0, 9]));
        // A shape already in a clip set blocks making a fresh mask.
        let some = [tag(Some(0), true), tag(Some(0), false), tag(None, false)];
        assert!(!can_make(&some, &[1, 2]));
    }

    #[test]
    fn can_release_detects_a_clipped_member() {
        let tags = [tag(Some(0), true), tag(Some(0), false), tag(None, false)];
        assert!(can_release(&tags, &[2, 1])); // index 1 is clipped
        assert!(!can_release(&tags, &[2])); // index 2 is loose
        assert!(!can_release(&tags, &[]));
    }

    #[test]
    fn members_and_mask_lookup() {
        // set 0 spans indices 0..=2 (0 is the mask); index 3 is loose.
        let tags = [
            tag(Some(0), false),
            tag(Some(0), true),
            tag(Some(0), false),
            tag(None, false),
        ];
        assert_eq!(members_of(&tags, 2), vec![0, 1, 2]);
        assert_eq!(members_of(&tags, 3), vec![3]); // loose: just itself
        assert!(members_of(&tags, 99).is_empty());
        assert_eq!(mask_of(&tags, 0), Some(1));
        assert_eq!(mask_of(&tags, 3), None); // loose shape has no mask
    }

    #[test]
    fn selected_ids_are_sorted_deduped() {
        let tags = [
            tag(Some(2), true),
            tag(Some(2), false),
            tag(Some(0), true),
            tag(None, false),
        ];
        assert_eq!(selected_clip_ids(&tags, &[0, 1, 2, 3]), vec![0, 2]);
        assert!(selected_clip_ids(&tags, &[3]).is_empty());
    }

    fn square(x: f32, y: f32, s: f32) -> Vec<(f32, f32)> {
        vec![(x, y), (x + s, y), (x + s, y + s), (x, y + s)]
    }

    #[test]
    fn clip_intersection_crops_to_the_mask() {
        // A 20×20 content square clipped by a 10×10 mask at the origin yields the
        // 10×10 overlap.
        let content = square(0.0, 0.0, 20.0);
        let mask = square(0.0, 0.0, 10.0);
        let out = clip_polygon(&content, &mask).expect("overlap exists");
        // Bounds of the clipped ring should be the 10×10 mask region.
        let (mut minx, mut miny, mut maxx, mut maxy) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
        for (x, y) in out {
            minx = minx.min(x);
            miny = miny.min(y);
            maxx = maxx.max(x);
            maxy = maxy.max(y);
        }
        assert!((minx - 0.0).abs() < 1e-3 && (miny - 0.0).abs() < 1e-3);
        assert!((maxx - 10.0).abs() < 1e-3 && (maxy - 10.0).abs() < 1e-3);
    }

    #[test]
    fn clip_of_disjoint_shapes_is_empty() {
        let content = square(0.0, 0.0, 10.0);
        let mask = square(100.0, 100.0, 10.0);
        assert!(clip_polygon(&content, &mask).is_none());
    }

    #[test]
    fn clip_rejects_degenerate_input() {
        let line = vec![(0.0, 0.0), (10.0, 0.0)];
        let sq = square(0.0, 0.0, 10.0);
        assert!(clip_polygon(&line, &sq).is_none());
        assert!(clip_polygon(&sq, &line).is_none());
    }
}
