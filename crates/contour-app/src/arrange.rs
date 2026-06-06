//! Pure z-order ("Arrange") logic over a flat paint-ordered list.
//!
//! Contour's document is an ordered `Vec<Shape>` where index `0` is painted
//! first (visually at the back) and the last index is painted on top (front).
//! The four Illustrator arrange commands — **bring to front**, **send to back**,
//! **bring forward**, **send backward** — are reorderings of that list applied to
//! a *set* of selected indices.
//!
//! The functions here are written against a generic `len` + selected-index set
//! so they are trivially unit-testable without any `Shape`/UI. Each returns a
//! **permutation** `perm` of `0..len`, where `perm[new_index] = old_index`: the
//! element that should land at each new slot. The caller applies it to the real
//! `Vec<Shape>` and remaps the selection through the same permutation.

/// One of the four z-order operations.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Arrange {
    /// Move the selection above everything else (to the end of the list).
    BringToFront,
    /// Move the selection below everything else (to the start of the list).
    SendToBack,
    /// Move the selection up one step in paint order (toward the front).
    BringForward,
    /// Move the selection down one step in paint order (toward the back).
    SendBackward,
}

impl Arrange {
    /// Menu / tooltip label.
    pub fn label(self) -> &'static str {
        match self {
            Arrange::BringToFront => "Bring to Front",
            Arrange::SendToBack => "Send to Back",
            Arrange::BringForward => "Bring Forward",
            Arrange::SendBackward => "Send Backward",
        }
    }
}

/// Compute the new paint order for `len` elements after applying `op` to the
/// selected indices in `selected`.
///
/// Returns a permutation `perm` of `0..len` with `perm[new] = old`. The relative
/// order of the selected elements among themselves is always preserved, as is
/// the relative order of the unselected elements. Returns the identity
/// permutation when the move is a no-op (empty / full selection, or the
/// selection is already at the extreme for a to-front/to-back move).
pub fn reorder(len: usize, selected: &[usize], op: Arrange) -> Vec<usize> {
    let identity: Vec<usize> = (0..len).collect();
    if len == 0 || selected.is_empty() {
        return identity;
    }

    // Normalise the selection to the sorted set of valid, distinct indices.
    let mut sel: Vec<usize> = selected.iter().copied().filter(|&i| i < len).collect();
    sel.sort_unstable();
    sel.dedup();
    if sel.is_empty() || sel.len() == len {
        // Moving everything (or nothing) never changes the order.
        return identity;
    }
    let is_sel = |i: usize| sel.binary_search(&i).is_ok();

    match op {
        Arrange::BringToFront => {
            // Unselected first (keeping their order), then the selected block.
            let mut perm: Vec<usize> = (0..len).filter(|&i| !is_sel(i)).collect();
            perm.extend(sel.iter().copied());
            perm
        }
        Arrange::SendToBack => {
            // Selected block first, then the unselected (keeping their order).
            let mut perm: Vec<usize> = sel.clone();
            perm.extend((0..len).filter(|&i| !is_sel(i)));
            perm
        }
        Arrange::BringForward => step_forward(len, &sel),
        Arrange::SendBackward => step_backward(len, &sel),
    }
}

/// Move the selected block up one slot in paint order (toward the front, i.e. a
/// higher index). Each selected run swaps with the single unselected element
/// directly above it; runs already at the top stay put. Equivalent to
/// Illustrator's "Bring Forward".
fn step_forward(len: usize, sel: &[usize]) -> Vec<usize> {
    // Work on a boolean "selected" mask over slots, then bubble selected slots
    // up past the first unselected neighbour above each contiguous run.
    let mut perm: Vec<usize> = (0..len).collect();
    let mut mask = vec![false; len];
    for &i in sel {
        mask[i] = true;
    }
    // Scan from the top down so a run moves as a block by one.
    // For index i (from len-2 .. 0): if slot i is selected and slot i+1 is not,
    // swap them. Process top-down to avoid double-moving within one pass.
    for i in (0..len.saturating_sub(1)).rev() {
        if mask[i] && !mask[i + 1] {
            perm.swap(i, i + 1);
            mask.swap(i, i + 1);
        }
    }
    perm
}

/// Move the selected block down one slot in paint order (toward the back, i.e. a
/// lower index). Mirror of [`step_forward`]; Illustrator's "Send Backward".
fn step_backward(len: usize, sel: &[usize]) -> Vec<usize> {
    let mut perm: Vec<usize> = (0..len).collect();
    let mut mask = vec![false; len];
    for &i in sel {
        mask[i] = true;
    }
    // Scan bottom-up: if slot i is selected and slot i-1 is not, swap with below.
    for i in 1..len {
        if mask[i] && !mask[i - 1] {
            perm.swap(i, i - 1);
            mask.swap(i, i - 1);
        }
    }
    perm
}

/// Given a permutation `perm` (`perm[new] = old`), build the inverse map
/// `inv[old] = new`, used to remap selection indices after a reorder.
pub fn invert(perm: &[usize]) -> Vec<usize> {
    let mut inv = vec![0usize; perm.len()];
    for (new, &old) in perm.iter().enumerate() {
        inv[old] = new;
    }
    inv
}

/// Whether `op` would actually change the order of `len` elements for the given
/// selection (so the UI can disable a no-op command and the caller can skip an
/// undo checkpoint).
pub fn changes_order(len: usize, selected: &[usize], op: Arrange) -> bool {
    let perm = reorder(len, selected, op);
    perm.iter().enumerate().any(|(new, &old)| new != old)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Apply a permutation to a vector of test tokens (here, just the original
    /// indices) so assertions read in terms of "which original elements ended up
    /// where".
    fn applied(len: usize, selected: &[usize], op: Arrange) -> Vec<usize> {
        reorder(len, selected, op)
    }

    #[test]
    fn empty_or_full_selection_is_identity() {
        assert_eq!(reorder(4, &[], Arrange::BringToFront), vec![0, 1, 2, 3]);
        assert_eq!(
            reorder(4, &[0, 1, 2, 3], Arrange::SendToBack),
            vec![0, 1, 2, 3]
        );
        assert_eq!(reorder(0, &[0], Arrange::BringForward), Vec::<usize>::new());
    }

    #[test]
    fn bring_to_front_moves_selection_to_end() {
        // Select element 1; it should jump to the top (last slot).
        let perm = applied(4, &[1], Arrange::BringToFront);
        assert_eq!(perm, vec![0, 2, 3, 1]);
    }

    #[test]
    fn send_to_back_moves_selection_to_start() {
        let perm = applied(4, &[2], Arrange::SendToBack);
        assert_eq!(perm, vec![2, 0, 1, 3]);
    }

    #[test]
    fn bring_to_front_preserves_relative_order_of_selection() {
        // Selecting 0 and 2 keeps 0 before 2 in the moved block.
        let perm = applied(5, &[0, 2], Arrange::BringToFront);
        assert_eq!(perm, vec![1, 3, 4, 0, 2]);
    }

    #[test]
    fn bring_forward_swaps_with_neighbour_above() {
        // Element 1 moves up one slot (1 <-> 2).
        let perm = applied(4, &[1], Arrange::BringForward);
        assert_eq!(perm, vec![0, 2, 1, 3]);
    }

    #[test]
    fn send_backward_swaps_with_neighbour_below() {
        let perm = applied(4, &[2], Arrange::SendBackward);
        assert_eq!(perm, vec![0, 2, 1, 3]);
    }

    #[test]
    fn bring_forward_at_top_is_noop() {
        // Element 3 is already on top of a 4-element list.
        let perm = applied(4, &[3], Arrange::BringForward);
        assert_eq!(perm, vec![0, 1, 2, 3]);
        assert!(!changes_order(4, &[3], Arrange::BringForward));
    }

    #[test]
    fn send_backward_at_bottom_is_noop() {
        let perm = applied(4, &[0], Arrange::SendBackward);
        assert_eq!(perm, vec![0, 1, 2, 3]);
        assert!(!changes_order(4, &[0], Arrange::SendBackward));
    }

    #[test]
    fn bring_forward_moves_contiguous_run_as_a_block() {
        // Run {1,2} moves up one, swapping with 3.
        let perm = applied(5, &[1, 2], Arrange::BringForward);
        assert_eq!(perm, vec![0, 3, 1, 2, 4]);
    }

    #[test]
    fn send_backward_moves_contiguous_run_as_a_block() {
        // Run {2,3} moves down one, swapping with 1.
        let perm = applied(5, &[2, 3], Arrange::SendBackward);
        assert_eq!(perm, vec![0, 2, 3, 1, 4]);
    }

    #[test]
    fn bring_forward_split_runs_each_advance() {
        // Disjoint selection {0,2} in a 4-list: 0 swaps with 1, 2 swaps with 3.
        let perm = applied(4, &[0, 2], Arrange::BringForward);
        assert_eq!(perm, vec![1, 0, 3, 2]);
    }

    #[test]
    fn reorder_is_always_a_permutation() {
        // Every op over a few sizes/selections must yield a true permutation
        // (each original index appears exactly once).
        for len in 0usize..6 {
            for sel in [
                vec![],
                vec![0],
                vec![len.saturating_sub(1)],
                vec![0, 2],
                vec![1, 2, 3],
            ] {
                for op in [
                    Arrange::BringToFront,
                    Arrange::SendToBack,
                    Arrange::BringForward,
                    Arrange::SendBackward,
                ] {
                    let sel: Vec<usize> = sel.iter().copied().filter(|&i| i < len).collect();
                    let perm = reorder(len, &sel, op);
                    assert_eq!(perm.len(), len);
                    let mut seen = perm.clone();
                    seen.sort_unstable();
                    assert_eq!(seen, (0..len).collect::<Vec<_>>(), "not a permutation");
                }
            }
        }
    }

    #[test]
    fn invert_round_trips() {
        let perm = vec![2, 0, 1, 3];
        let inv = invert(&perm);
        // inv[old] = new. old 2 went to new slot 0, etc.
        assert_eq!(inv, vec![1, 2, 0, 3]);
        // Composing perm and inv yields identity.
        for old in 0..perm.len() {
            assert_eq!(perm[inv[old]], old);
        }
    }

    #[test]
    fn marquee_does_not_apply_here_but_permutation_remaps_selection() {
        // Simulate: shapes [A,B,C,D] (0..3), select {B,D} (1,3), bring to front.
        let perm = reorder(4, &[1, 3], Arrange::BringToFront);
        // New order = old [A,C,B,D] -> perm = [0,2,1,3].
        assert_eq!(perm, vec![0, 2, 1, 3]);
        let inv = invert(&perm);
        // Selection {1,3} maps to new slots {inv[1], inv[3]} = {2,3}.
        let new_sel: Vec<usize> = [1usize, 3].iter().map(|&old| inv[old]).collect();
        assert_eq!(new_sel, vec![2, 3]);
    }
}
