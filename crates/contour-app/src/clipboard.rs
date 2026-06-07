//! The shape clipboard and the pure logic behind copy / cut / paste,
//! paste-in-place, paste-in-front / -back, and duplicate.
//!
//! Contour's document is a flat paint-ordered `Vec<Shape>`; the clipboard holds
//! a detached copy of a set of shapes (snapshotted at copy time, so later edits
//! to the originals don't bleed into a paste). Pasting clones the buffer back
//! into the document. Two concerns are pure and unit-tested here, away from any
//! UI / egui:
//!
//! - **Where a plain paste lands** — Illustrator nudges each successive paste so
//!   stacked copies fan out instead of hiding behind one another;
//!   [`paste_offset`] computes that nudge from a paste counter.
//! - **Keeping pasted groups grouped without id collisions** — copied shapes may
//!   carry [group](crate::group) ids; on paste those ids must be remapped to
//!   fresh ones (so the paste is independent of the originals) while shapes that
//!   shared an id in the buffer still share one afterwards. [`remap_group_ids`]
//!   does that against the destination document's existing ids.

use crate::document::Shape;

/// Per-step nudge (document units) applied to each consecutive plain paste, so
/// repeated `Ctrl+V` fans the copies out diagonally rather than stacking them
/// exactly, à la Illustrator's "paste offsets a little each time".
pub const PASTE_NUDGE: f32 = 12.0;

/// The detached shapes copied from a document, ready to paste. Empty means
/// nothing has been copied yet. Shapes are stored exactly as they were (style,
/// handles, group tags) so a paste reproduces them faithfully.
#[derive(Default, Clone)]
pub struct Clipboard {
    shapes: Vec<Shape>,
    /// How many times the current buffer has been plain-pasted, so each paste
    /// can nudge a little further than the last (reset whenever new shapes are
    /// copied or the buffer is pasted in place).
    paste_count: u32,
}

impl Clipboard {
    /// Whether there is anything to paste.
    pub fn is_empty(&self) -> bool {
        self.shapes.is_empty()
    }

    /// Replace the buffer with a fresh snapshot of `shapes` and reset the paste
    /// nudge counter. Copying an empty set clears the buffer.
    pub fn set(&mut self, shapes: Vec<Shape>) {
        self.shapes = shapes;
        self.paste_count = 0;
    }

    /// Take one plain-paste slot: clones of the buffer, each translated by the
    /// next fanning nudge, plus the count bump. Returns `None` when empty.
    ///
    /// The returned shapes keep their relative layout (the whole set shifts by
    /// the same offset) so a pasted group stays self-consistent.
    pub fn take_paste(&mut self) -> Option<Vec<Shape>> {
        if self.shapes.is_empty() {
            return None;
        }
        self.paste_count += 1;
        let off = paste_offset(self.paste_count);
        let mut out = self.shapes.clone();
        for s in out.iter_mut() {
            s.translate(off.0, off.1);
        }
        Some(out)
    }

    /// Clones of the buffer with **no** offset, for paste-in-place / in-front /
    /// in-back and duplicate-in-place. Does not advance the nudge counter (an
    /// in-place paste lands exactly where it was copied from). Returns `None`
    /// when empty.
    pub fn clone_in_place(&self) -> Option<Vec<Shape>> {
        if self.shapes.is_empty() {
            None
        } else {
            Some(self.shapes.clone())
        }
    }
}

/// The diagonal nudge for the `n`-th consecutive plain paste (1-based). Paste #1
/// shifts by one [`PASTE_NUDGE`], #2 by two, and so on, so successive pastes step
/// down-and-right and stay individually grabbable.
pub fn paste_offset(n: u32) -> (f32, f32) {
    let d = PASTE_NUDGE * n as f32;
    (d, d)
}

/// Rewrite the group tags on a freshly-cloned `pasted` set so it is independent
/// of whatever is already in the destination document, while preserving the
/// buffer's *internal* grouping: shapes that shared a group id in the buffer
/// still share one (a brand-new id that can't collide with `existing`), and
/// distinct buffer groups map to distinct new ids. Ungrouped shapes stay
/// ungrouped.
///
/// `existing` is the destination document's per-shape group tags; the new ids
/// start above every id in use there. Mutates `pasted` in place.
pub fn remap_group_ids(pasted: &mut [Shape], existing: &[Option<u64>]) {
    // Next free id in the destination (mirrors group::next_group_id, but kept
    // local so the clipboard owns its own remap policy).
    let mut next = existing
        .iter()
        .filter_map(|t| *t)
        .max()
        .map_or(0, |m| m + 1);

    // Map each *old* buffer id to a fresh destination id, allocated on first
    // sight so equal old ids map to one new id.
    let mut mapping: Vec<(u64, u64)> = Vec::new();
    for s in pasted.iter_mut() {
        let Some(old) = s.group() else { continue };
        let new = match mapping.iter().find(|(o, _)| *o == old) {
            Some((_, n)) => *n,
            None => {
                let n = next;
                next += 1;
                mapping.push((old, n));
                n
            }
        };
        s.set_group(Some(new));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::{Shape, StrokeStyle};

    fn rect(group: Option<u64>) -> Shape {
        Shape::Rect {
            rect: [0.0, 0.0, 10.0, 10.0],
            fill: [1.0, 0.0, 0.0, 1.0],
            fill_gradient: None,
            stroke: [0.0, 0.0, 0.0, 1.0],
            stroke_w: 1.0,
            stroke_style: StrokeStyle::default(),
            appearance: None,
            visible: true,
            group,
            clip: None,
            mask: false,
            omask: None,
            omask_path: false,
            omask_invert: false,
        }
    }

    #[test]
    fn paste_offset_grows_each_step() {
        assert_eq!(paste_offset(1), (PASTE_NUDGE, PASTE_NUDGE));
        assert_eq!(paste_offset(2), (2.0 * PASTE_NUDGE, 2.0 * PASTE_NUDGE));
        assert_eq!(paste_offset(3), (3.0 * PASTE_NUDGE, 3.0 * PASTE_NUDGE));
    }

    #[test]
    fn empty_clipboard_yields_nothing() {
        let mut cb = Clipboard::default();
        assert!(cb.is_empty());
        assert!(cb.take_paste().is_none());
        assert!(cb.clone_in_place().is_none());
    }

    #[test]
    fn set_then_paste_nudges_progressively() {
        let mut cb = Clipboard::default();
        cb.set(vec![rect(None)]);
        assert!(!cb.is_empty());
        // First paste nudges by one step, second by two.
        let p1 = cb.take_paste().expect("first paste");
        match &p1[0] {
            Shape::Rect { rect, .. } => assert_eq!((rect[0], rect[1]), (PASTE_NUDGE, PASTE_NUDGE)),
            _ => panic!("expected rect"),
        }
        let p2 = cb.take_paste().expect("second paste");
        match &p2[0] {
            Shape::Rect { rect, .. } => {
                assert_eq!((rect[0], rect[1]), (2.0 * PASTE_NUDGE, 2.0 * PASTE_NUDGE))
            }
            _ => panic!("expected rect"),
        }
    }

    #[test]
    fn set_resets_the_nudge_counter() {
        let mut cb = Clipboard::default();
        cb.set(vec![rect(None)]);
        cb.take_paste();
        cb.take_paste();
        // Re-copying restarts the fan from the first step.
        cb.set(vec![rect(None)]);
        let p = cb.take_paste().expect("paste after re-copy");
        match &p[0] {
            Shape::Rect { rect, .. } => assert_eq!((rect[0], rect[1]), (PASTE_NUDGE, PASTE_NUDGE)),
            _ => panic!("expected rect"),
        }
    }

    #[test]
    fn clone_in_place_does_not_offset_or_advance() {
        let mut cb = Clipboard::default();
        cb.set(vec![rect(None)]);
        let p = cb.clone_in_place().expect("in-place clone");
        match &p[0] {
            Shape::Rect { rect, .. } => assert_eq!((rect[0], rect[1]), (0.0, 0.0)),
            _ => panic!("expected rect"),
        }
        // The nudge counter is untouched, so the next plain paste is still step 1.
        let q = cb.take_paste().expect("paste");
        match &q[0] {
            Shape::Rect { rect, .. } => assert_eq!((rect[0], rect[1]), (PASTE_NUDGE, PASTE_NUDGE)),
            _ => panic!("expected rect"),
        }
    }

    #[test]
    fn remap_assigns_fresh_ids_above_existing() {
        // Destination already uses ids 0 and 4 → next free id is 5.
        let existing = [Some(0u64), None, Some(4)];
        // Buffer has two shapes sharing old id 2, one ungrouped.
        let mut pasted = vec![rect(Some(2)), rect(Some(2)), rect(None)];
        remap_group_ids(&mut pasted, &existing);
        assert_eq!(pasted[0].group(), Some(5));
        assert_eq!(pasted[1].group(), Some(5)); // same buffer group → same new id
        assert_eq!(pasted[2].group(), None); // ungrouped stays ungrouped
    }

    #[test]
    fn remap_keeps_distinct_buffer_groups_distinct() {
        let existing: [Option<u64>; 0] = [];
        let mut pasted = vec![rect(Some(10)), rect(Some(20)), rect(Some(10))];
        remap_group_ids(&mut pasted, &existing);
        // Two distinct old ids → two distinct new ids, starting at 0.
        assert_eq!(pasted[0].group(), Some(0));
        assert_eq!(pasted[1].group(), Some(1));
        assert_eq!(pasted[2].group(), Some(0)); // matches the first
        assert_ne!(pasted[0].group(), pasted[1].group());
    }

    #[test]
    fn remap_is_a_noop_for_ungrouped_buffers() {
        let existing = [Some(7u64)];
        let mut pasted = vec![rect(None), rect(None)];
        remap_group_ids(&mut pasted, &existing);
        assert_eq!(pasted[0].group(), None);
        assert_eq!(pasted[1].group(), None);
    }
}
