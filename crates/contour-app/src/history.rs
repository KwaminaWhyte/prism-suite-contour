//! Undo / redo for the document.
//!
//! Contour's document is cheap (paths + params, no pixel buffers), so the most
//! robust history model is a **snapshot stack**: before any mutation we clone
//! the current [`Document`] onto an undo stack; undo/redo swap the live document
//! with a saved snapshot. This sidesteps the bookkeeping of per-command inverse
//! deltas while staying well within memory budget for vector docs.
//!
//! The plan (PLAN.md §6.1) explicitly endorses this: *"the doc is small
//! (paths+params), so full structural undo is cheap — no tile/COW tricks
//! needed."*
//!
//! Usage from the app:
//! - Call [`History::push`] with the *pre-edit* document immediately before a
//!   mutation that should be undoable (a "checkpoint").
//! - For continuous drags (move / anchor edit) call [`History::begin`] once at
//!   `drag_started` to snapshot the start state, then [`History::commit`] at
//!   `drag_stopped` to finalize a single coalesced history entry.
//! - [`History::undo`] / [`History::redo`] return the document to install.

use crate::document::Document;

/// Hard cap on retained snapshots so a long session can't grow unbounded.
/// 200 structural edits of cheap vector docs is comfortably small.
const MAX_DEPTH: usize = 200;

/// A snapshot-based undo/redo stack over whole [`Document`]s.
#[derive(Default)]
pub struct History {
    /// Past states, oldest first; the top is the state to restore on undo.
    undo: Vec<Document>,
    /// Future states (populated by undo), newest first; the top is restored on
    /// redo.
    redo: Vec<Document>,
    /// Pending pre-edit snapshot for a coalesced drag (see [`begin`]/[`commit`]).
    pending: Option<Document>,
}

impl History {
    pub fn new() -> Self {
        Self::default()
    }

    /// Record `pre` (the document *before* an edit) as an undo checkpoint.
    /// Redo history is discarded — a new edit forks the timeline.
    pub fn push(&mut self, pre: Document) {
        self.redo.clear();
        self.undo.push(pre);
        if self.undo.len() > MAX_DEPTH {
            // Drop the oldest snapshot.
            self.undo.remove(0);
        }
    }

    /// Snapshot the start of a coalesced interaction (e.g. a drag). Only the
    /// first `begin` in a sequence is retained, so repeated per-frame calls are
    /// safe and the whole drag collapses into one undo entry.
    pub fn begin(&mut self, pre: &Document) {
        if self.pending.is_none() {
            self.pending = Some(pre.clone());
        }
    }

    /// Finalize a coalesced interaction started with [`begin`]. `current` is the
    /// post-drag document; if it is unchanged from the pending snapshot the
    /// checkpoint is dropped (no-op drags don't pollute history). Returns `true`
    /// when a checkpoint was actually recorded.
    pub fn commit(&mut self, current: &Document) -> bool {
        let Some(pre) = self.pending.take() else {
            return false;
        };
        if documents_equal(&pre, current) {
            return false;
        }
        self.push(pre);
        true
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    /// Undo: push `current` onto the redo stack and return the previous state to
    /// install, or `None` if there is nothing to undo.
    pub fn undo(&mut self, current: &Document) -> Option<Document> {
        let prev = self.undo.pop()?;
        self.redo.push(current.clone());
        Some(prev)
    }

    /// Redo: push `current` onto the undo stack and return the next state to
    /// install, or `None` if there is nothing to redo.
    pub fn redo(&mut self, current: &Document) -> Option<Document> {
        let next = self.redo.pop()?;
        self.undo.push(current.clone());
        Some(next)
    }

    /// Forget all history (e.g. on New / Open). Keeps the stack tidy after a
    /// document replacement that should not be undoable into the old doc.
    pub fn clear(&mut self) {
        self.undo.clear();
        self.redo.clear();
        self.pending = None;
    }
}

/// Structural equality of two documents, used to drop no-op drags. We compare
/// the serialized form, which is stable and cheap for vector docs and avoids
/// requiring `PartialEq` on the float-bearing `Shape` model.
fn documents_equal(a: &Document, b: &Document) -> bool {
    match (serde_json::to_vec(a), serde_json::to_vec(b)) {
        (Ok(x), Ok(y)) => x == y,
        // If either fails to serialize, treat them as different so we err on the
        // side of recording a checkpoint rather than silently losing an edit.
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::{Shape, StrokeStyle};

    fn rect(x: f32) -> Shape {
        Shape::Rect {
            rect: [x, 0.0, 10.0, 10.0],
            fill: [1.0, 0.0, 0.0, 1.0],
            fill_gradient: None,
            stroke: [0.0, 0.0, 0.0, 1.0],
            stroke_w: 1.0,
            stroke_style: StrokeStyle::default(),
            visible: true,
            group: None,
        }
    }

    fn doc_with(n: usize) -> Document {
        Document {
            shapes: (0..n).map(|i| rect(i as f32)).collect(),
            ..Default::default()
        }
    }

    #[test]
    fn undo_then_redo_round_trips() {
        let mut h = History::new();
        let v0 = doc_with(0);
        let v1 = doc_with(1);

        // Edit v0 -> v1: checkpoint the pre-edit doc.
        h.push(v0.clone());
        assert!(h.can_undo());
        assert!(!h.can_redo());

        // Undo from v1 returns v0.
        let restored = h.undo(&v1).expect("undo available");
        assert_eq!(restored.shapes.len(), 0);
        assert!(!h.can_undo());
        assert!(h.can_redo());

        // Redo from v0 returns v1.
        let redone = h.redo(&restored).expect("redo available");
        assert_eq!(redone.shapes.len(), 1);
        assert!(h.can_undo());
        assert!(!h.can_redo());
    }

    #[test]
    fn new_edit_forks_timeline_and_clears_redo() {
        let mut h = History::new();
        h.push(doc_with(0)); // v0 -> v1
        let v1 = doc_with(1);
        let _ = h.undo(&v1); // now at v0, redo has v1
        assert!(h.can_redo());

        // A fresh edit from v0 must clear redo.
        h.push(doc_with(0));
        assert!(!h.can_redo());
        assert!(h.can_undo());
    }

    #[test]
    fn begin_commit_coalesces_a_drag() {
        let mut h = History::new();
        let start = doc_with(1);
        // Repeated begins during a drag retain only the first snapshot.
        h.begin(&start);
        h.begin(&doc_with(2));
        h.begin(&doc_with(3));

        // Drag changed the doc: one checkpoint recorded.
        let recorded = h.commit(&doc_with(2));
        assert!(recorded);
        assert_eq!(h.undo.len(), 1);
        assert_eq!(h.undo[0].shapes.len(), 1, "first snapshot retained");
    }

    #[test]
    fn no_op_drag_records_nothing() {
        let mut h = History::new();
        let start = doc_with(1);
        h.begin(&start);
        // Committed identical document -> no checkpoint.
        let recorded = h.commit(&doc_with(1));
        assert!(!recorded);
        assert!(!h.can_undo());
    }

    #[test]
    fn depth_is_capped() {
        let mut h = History::new();
        for _ in 0..(MAX_DEPTH + 50) {
            h.push(doc_with(0));
        }
        assert_eq!(h.undo.len(), MAX_DEPTH);
    }

    #[test]
    fn clear_empties_both_stacks() {
        let mut h = History::new();
        h.push(doc_with(0));
        let _ = h.undo(&doc_with(1));
        h.clear();
        assert!(!h.can_undo());
        assert!(!h.can_redo());
    }
}
