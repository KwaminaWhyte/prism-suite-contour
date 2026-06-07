//! Opacity masks — Illustrator's `Object ▸ Opacity Mask ▸ Make / Release`.
//!
//! An **opacity-mask set** is a flat run of shapes that share an additive
//! opacity-mask id (`Shape::omask` → `Some(id)`), one of which is flagged the
//! **mask path** (`Shape::is_omask`). The topmost selected shape becomes the
//! mask; the shapes below it are the *masked content*. The mask's **luminance**
//! (weighted by its own coverage) drives the content's alpha — white reveals,
//! black hides — with an optional **invert** that flips that. Because the mask
//! and the content keep their original geometry, the operation is fully
//! non-destructive: **Release** simply clears the tags and the originals reappear
//! at full opacity.
//!
//! Rendering is derived, never stored: the renderers rasterize the mask shape's
//! luminance and multiply it into the content's alpha
//! ([`crate::effects::apply_luminance_mask`]); the mask path itself paints nothing
//! (`render_shapes` drops it). Unlike a clipping mask this is a *soft* (per-pixel
//! alpha) crop rather than a hard geometric one.
//!
//! Everything here is pure: the planning helpers work over a slice of
//! `(omask_id, is_mask)` tags (one per shape, in paint order) plus a selection
//! index set, so all of it is unit-testable without any `Shape` or egui context.

/// One shape's opacity-mask tag: which set it belongs to (if any) and whether it
/// is that set's mask path. Mirrors the additive `(omask, is_omask)` pair stored
/// on every [`Shape`](crate::document::Shape).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct OMaskTag {
    pub omask: Option<u64>,
    pub is_mask: bool,
}

impl OMaskTag {
    pub fn new(omask: Option<u64>, is_mask: bool) -> Self {
        Self { omask, is_mask }
    }
}

/// The smallest opacity-mask id not currently used by any tag, so a freshly-made
/// mask never collides with an existing one. Ids are opaque; only equality
/// matters. (Independent of group / clip ids — a shape can be grouped, clipped,
/// and opacity-masked at once.)
pub fn next_omask_id(tags: &[OMaskTag]) -> u64 {
    tags.iter()
        .filter_map(|t| t.omask)
        .max()
        .map_or(0, |m| m + 1)
}

/// Whether a selection can be turned into an opacity mask: it must reference at
/// least two distinct in-range shapes (a mask plus at least one masked shape),
/// and none of them may already belong to an opacity-mask set.
pub fn can_make(tags: &[OMaskTag], selected: &[usize]) -> bool {
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
    s.iter().all(|&i| tags[i].omask.is_none())
}

/// Whether the selection touches any opacity-mask set (so Release is meaningful).
pub fn can_release(tags: &[OMaskTag], selected: &[usize]) -> bool {
    selected
        .iter()
        .any(|&i| tags.get(i).is_some_and(|t| t.omask.is_some()))
}

/// The opacity-mask ids referenced by `selected`, ascending and de-duplicated.
pub fn selected_omask_ids(tags: &[OMaskTag], selected: &[usize]) -> Vec<u64> {
    let mut ids: Vec<u64> = selected
        .iter()
        .filter_map(|&i| tags.get(i).and_then(|t| t.omask))
        .collect();
    ids.sort_unstable();
    ids.dedup();
    ids
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tag(omask: Option<u64>, mask: bool) -> OMaskTag {
        OMaskTag::new(omask, mask)
    }

    #[test]
    fn next_id_skips_existing() {
        assert_eq!(next_omask_id(&[]), 0);
        assert_eq!(next_omask_id(&[tag(None, false), tag(None, false)]), 0);
        assert_eq!(next_omask_id(&[tag(Some(0), true), tag(Some(0), false)]), 1);
        assert_eq!(next_omask_id(&[tag(Some(4), false)]), 5);
    }

    #[test]
    fn can_make_needs_two_unmasked_shapes() {
        let none = [tag(None, false), tag(None, false), tag(None, false)];
        assert!(!can_make(&none, &[]));
        assert!(!can_make(&none, &[1])); // one shape: nothing to mask
        assert!(!can_make(&none, &[1, 1])); // duplicate collapses
        assert!(can_make(&none, &[0, 2]));
        assert!(!can_make(&none, &[0, 9])); // out-of-range ignored
        // A shape already in a mask set blocks making a fresh mask.
        let some = [tag(Some(0), true), tag(Some(0), false), tag(None, false)];
        assert!(!can_make(&some, &[1, 2]));
    }

    #[test]
    fn can_release_detects_a_masked_member() {
        let tags = [tag(Some(0), true), tag(Some(0), false), tag(None, false)];
        assert!(can_release(&tags, &[2, 1])); // index 1 is masked
        assert!(!can_release(&tags, &[2])); // index 2 is loose
        assert!(!can_release(&tags, &[]));
    }

    #[test]
    fn selected_ids_are_sorted_deduped() {
        let tags = [
            tag(Some(2), true),
            tag(Some(2), false),
            tag(Some(0), true),
            tag(None, false),
        ];
        assert_eq!(selected_omask_ids(&tags, &[0, 1, 2, 3]), vec![0, 2]);
        assert!(selected_omask_ids(&tags, &[3]).is_empty());
    }
}
