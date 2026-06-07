//! Pure layout logic for the **Layers** panel.
//!
//! Contour's document is a flat, paint-ordered `Vec<Shape>` (index `0` is the
//! back, the last index is the front). The Layers panel shows that list
//! **top-to-bottom in z-order** (front first), with grouped shapes (those
//! sharing a [`group`](crate::document::Shape::group) tag) gathered under one
//! expandable parent row — the closest a flat model gets to Illustrator's layer
//! tree without a recursive refactor.
//!
//! This module turns the shape list into the ordered list of [`LayerRow`]s the
//! panel renders, honoring which groups the user has collapsed. It is pure (it
//! takes a slice of group tags, never a `Shape`/UI), so the row ordering, the
//! group nesting, and the collapse behaviour are all unit-tested here without an
//! egui context. The panel only renders the rows and routes clicks back into the
//! tested editor operations.

/// One row in the Layers panel, in top-to-bottom (front-to-back) display order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LayerRow {
    /// The header for group `id`, summarising `count` members. Drawn with a
    /// disclosure triangle; toggling it collapses / expands the members below.
    Group { id: u64, count: usize },
    /// A single shape at document index `idx`. `depth` is the indent level (`0`
    /// at the top level, `1` when nested under a group header).
    Shape { idx: usize, depth: u8 },
}

/// Build the ordered Layers-panel rows from per-shape group tags (in paint
/// order) and the set of currently-collapsed group ids.
///
/// Rows are emitted **front-first** (highest document index at the top). The
/// first time a group is encountered (scanning from the front) a [`Group`]
/// header is emitted, immediately followed by that group's member shapes
/// (front-first, indented) — unless the group id is in `collapsed`, in which
/// case the header is emitted but its members are hidden. Loose (ungrouped)
/// shapes are emitted at depth `0` in z-order.
///
/// [`Group`]: LayerRow::Group
pub fn rows(group_tags: &[Option<u64>], collapsed: &[u64]) -> Vec<LayerRow> {
    let n = group_tags.len();
    let is_collapsed = |g: u64| collapsed.contains(&g);
    let count_of = |g: u64| group_tags.iter().filter(|t| **t == Some(g)).count();

    let mut out: Vec<LayerRow> = Vec::with_capacity(n);
    let mut emitted_group: Vec<u64> = Vec::new();

    // Walk front-to-back (highest index first) so the panel lists the topmost
    // shape first, matching every other "newest on top" surface in the app.
    for idx in (0..n).rev() {
        match group_tags[idx] {
            None => out.push(LayerRow::Shape { idx, depth: 0 }),
            Some(g) => {
                // Emit the group header the first time we reach any of its
                // members (the frontmost member fixes the header's position).
                if !emitted_group.contains(&g) {
                    emitted_group.push(g);
                    out.push(LayerRow::Group {
                        id: g,
                        count: count_of(g),
                    });
                }
                if !is_collapsed(g) {
                    out.push(LayerRow::Shape { idx, depth: 1 });
                }
            }
        }
    }
    out
}

/// Toggle group id `g` in the `collapsed` set: collapse it if expanded, expand
/// it if collapsed. Pure set bookkeeping the panel drives from the disclosure
/// triangle.
pub fn toggle_collapsed(collapsed: &mut Vec<u64>, g: u64) {
    if let Some(pos) = collapsed.iter().position(|&x| x == g) {
        collapsed.remove(pos);
    } else {
        collapsed.push(g);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loose_shapes_list_front_first() {
        // Three ungrouped shapes [A,B,C] (0..2) → listed C,B,A top-to-bottom.
        let tags = [None, None, None];
        assert_eq!(
            rows(&tags, &[]),
            vec![
                LayerRow::Shape { idx: 2, depth: 0 },
                LayerRow::Shape { idx: 1, depth: 0 },
                LayerRow::Shape { idx: 0, depth: 0 },
            ]
        );
    }

    #[test]
    fn group_header_precedes_indented_members() {
        // [g0, g0, loose] → header(g0) then members 1,0 indented, then the loose
        // shape. Front-first means the loose shape (idx 2) is at the very top.
        let tags = [Some(0), Some(0), None];
        assert_eq!(
            rows(&tags, &[]),
            vec![
                LayerRow::Shape { idx: 2, depth: 0 },
                LayerRow::Group { id: 0, count: 2 },
                LayerRow::Shape { idx: 1, depth: 1 },
                LayerRow::Shape { idx: 0, depth: 1 },
            ]
        );
    }

    #[test]
    fn collapsed_group_hides_its_members() {
        let tags = [Some(0), Some(0), None];
        // Collapsing g0 keeps its header but drops the two member rows.
        assert_eq!(
            rows(&tags, &[0]),
            vec![
                LayerRow::Shape { idx: 2, depth: 0 },
                LayerRow::Group { id: 0, count: 2 },
            ]
        );
    }

    #[test]
    fn two_groups_each_get_one_header_at_their_frontmost_member() {
        // [g0, g1, g0, g1] front-first scan: idx3=g1 (header g1, member 3),
        // idx2=g0 (header g0, member 2), idx1=g1 (member 1), idx0=g0 (member 0).
        let tags = [Some(0), Some(1), Some(0), Some(1)];
        assert_eq!(
            rows(&tags, &[]),
            vec![
                LayerRow::Group { id: 1, count: 2 },
                LayerRow::Shape { idx: 3, depth: 1 },
                LayerRow::Group { id: 0, count: 2 },
                LayerRow::Shape { idx: 2, depth: 1 },
                LayerRow::Shape { idx: 1, depth: 1 },
                LayerRow::Shape { idx: 0, depth: 1 },
            ]
        );
    }

    #[test]
    fn empty_document_yields_no_rows() {
        assert!(rows(&[], &[]).is_empty());
    }

    #[test]
    fn group_count_counts_every_member_even_interleaved() {
        // g0 has three members scattered through the list; the header reports 3.
        let tags = [Some(0), None, Some(0), None, Some(0)];
        let r = rows(&tags, &[]);
        assert!(
            r.contains(&LayerRow::Group { id: 0, count: 3 }),
            "header should count all three g0 members: {r:?}"
        );
    }

    #[test]
    fn toggle_collapsed_round_trips() {
        let mut c: Vec<u64> = Vec::new();
        toggle_collapsed(&mut c, 5);
        assert_eq!(c, vec![5], "first toggle collapses");
        toggle_collapsed(&mut c, 5);
        assert!(c.is_empty(), "second toggle expands");
        // Distinct groups accumulate.
        toggle_collapsed(&mut c, 1);
        toggle_collapsed(&mut c, 2);
        assert_eq!(c, vec![1, 2]);
    }
}
