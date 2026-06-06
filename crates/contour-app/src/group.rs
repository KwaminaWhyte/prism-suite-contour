//! Pure grouping logic over a flat paint-ordered list of group tags.
//!
//! Contour's document is a flat `Vec<Shape>`; rather than refactor it into a
//! recursive tree, grouping is modelled as an additive **group id** tag on each
//! shape (`Option<u64>`). Shapes sharing the same `Some(id)` form one group:
//! clicking any member selects the whole group, and the group moves / transforms
//! / arranges as a single unit, exactly the way Illustrator's groups behave for
//! day-to-day editing.
//!
//! The functions here operate on a generic slice of `Option<u64>` tags (one per
//! shape, in paint order) plus a selection index set, so they are trivially
//! unit-testable without any `Shape`/UI. They never mutate; the caller applies
//! the returned ids / index lists to the real document.

/// The smallest group id not currently used by any tag, so a freshly-formed
/// group can never collide with an existing one. Group ids are otherwise opaque;
/// only equality matters.
pub fn next_group_id(tags: &[Option<u64>]) -> u64 {
    tags.iter().filter_map(|t| *t).max().map_or(0, |m| m + 1)
}

/// Expand a set of selected indices so that whenever one member of a group is
/// selected, *every* member of that group is selected. Returns the expanded set
/// sorted ascending with duplicates removed.
///
/// Indices out of range for `tags` are dropped. Ungrouped shapes (`None` tag)
/// expand to just themselves.
pub fn expand_selection(tags: &[Option<u64>], selected: &[usize]) -> Vec<usize> {
    // Which group ids are touched by the selection.
    let mut groups: Vec<u64> = selected
        .iter()
        .filter_map(|&i| tags.get(i).copied().flatten())
        .collect();
    groups.sort_unstable();
    groups.dedup();

    let mut out: Vec<usize> = Vec::new();
    for (i, tag) in tags.iter().enumerate() {
        let directly = selected.contains(&i);
        let via_group = tag.is_some_and(|g| groups.binary_search(&g).is_ok());
        if directly || via_group {
            out.push(i);
        }
    }
    out.sort_unstable();
    out.dedup();
    out
}

/// All indices belonging to the same group as shape `i` (including `i` itself).
/// An ungrouped shape returns just `[i]`. Returns empty if `i` is out of range.
pub fn members_of(tags: &[Option<u64>], i: usize) -> Vec<usize> {
    match tags.get(i).copied().flatten() {
        Some(g) => tags
            .iter()
            .enumerate()
            .filter_map(|(j, t)| (*t == Some(g)).then_some(j))
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

/// Whether the selection is groupable: at least two distinct in-range shapes.
/// (Grouping a single shape — or nothing — is a no-op in Illustrator too.)
pub fn can_group(len: usize, selected: &[usize]) -> bool {
    let mut s: Vec<usize> = selected.iter().copied().filter(|&i| i < len).collect();
    s.sort_unstable();
    s.dedup();
    s.len() >= 2
}

/// Whether any selected shape currently carries a group tag (so Ungroup would do
/// something).
pub fn can_ungroup(tags: &[Option<u64>], selected: &[usize]) -> bool {
    selected
        .iter()
        .any(|&i| tags.get(i).copied().flatten().is_some())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn next_id_skips_existing_ids() {
        assert_eq!(next_group_id(&[]), 0);
        assert_eq!(next_group_id(&[None, None]), 0);
        assert_eq!(next_group_id(&[Some(0), None, Some(2)]), 3);
        assert_eq!(next_group_id(&[Some(5)]), 6);
    }

    #[test]
    fn expand_pulls_in_whole_group() {
        // Shapes: [g0, g0, none, g1, g1]; selecting index 0 grabs both g0 members.
        let tags = [Some(0), Some(0), None, Some(1), Some(1)];
        assert_eq!(expand_selection(&tags, &[0]), vec![0, 1]);
        // Selecting the ungrouped shape expands to just itself.
        assert_eq!(expand_selection(&tags, &[2]), vec![2]);
        // Selecting members of two groups grabs both whole groups.
        assert_eq!(expand_selection(&tags, &[1, 3]), vec![0, 1, 3, 4]);
    }

    #[test]
    fn expand_is_sorted_deduped_and_ignores_oob() {
        let tags = [Some(0), Some(0), None];
        // Out-of-range index is dropped; duplicates collapse; result sorted.
        assert_eq!(expand_selection(&tags, &[1, 1, 99, 0]), vec![0, 1]);
    }

    #[test]
    fn expand_mixes_grouped_and_loose_shapes() {
        let tags = [Some(7), None, Some(7), None];
        // Select a loose shape + one group member -> the loose shape stays, the
        // group fills in.
        assert_eq!(expand_selection(&tags, &[1, 0]), vec![0, 1, 2]);
    }

    #[test]
    fn members_of_returns_group_or_self() {
        let tags = [Some(3), None, Some(3), Some(9)];
        let mut m = members_of(&tags, 0);
        m.sort_unstable();
        assert_eq!(m, vec![0, 2]);
        assert_eq!(members_of(&tags, 1), vec![1]); // ungrouped: just itself
        assert_eq!(members_of(&tags, 3), vec![3]); // lone group member
        assert!(members_of(&tags, 99).is_empty()); // out of range
    }

    #[test]
    fn can_group_needs_two_distinct() {
        assert!(!can_group(3, &[]));
        assert!(!can_group(3, &[1]));
        assert!(!can_group(3, &[1, 1])); // duplicate collapses to one
        assert!(can_group(3, &[0, 2]));
        // Out-of-range indices don't count toward the two-shape minimum.
        assert!(!can_group(2, &[0, 5]));
    }

    #[test]
    fn can_ungroup_detects_a_grouped_member() {
        let tags = [Some(0), None, None];
        assert!(can_ungroup(&tags, &[0, 1]));
        assert!(!can_ungroup(&tags, &[1, 2]));
        assert!(!can_ungroup(&tags, &[]));
    }
}
