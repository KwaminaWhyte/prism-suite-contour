//! Document-mutating operations behind the menus, inspector, and keyboard:
//! selection bookkeeping, undo/redo checkpoints, delete/swap/arrange, file I/O
//! and export dialogs, boolean ops, and the align / distribute / transform /
//! snap math.

use super::{ContourApp, LastTransform, TransformDrag, TransformKind};
use crate::align::{self, Align, AlignTo, Distribute};
use crate::arrange::{self, Arrange};
use crate::boolean::{self, BoolOp};
use crate::document::{self, Guide, Shape};
use crate::export;
use crate::group;
use crate::snap::{self, SnapFeatures, SnapResult, SnapTargets};
use crate::transform::{self, Affine, NumericTransform};

impl ContourApp {
    // --- Selection helpers ---------------------------------------------------

    /// The primary (active) shape index — the last one added to the selection.
    pub(super) fn primary(&self) -> Option<usize> {
        self.selection.last().copied()
    }

    /// Whether shape `i` is in the selection set.
    pub(super) fn is_selected(&self, i: usize) -> bool {
        self.selection.contains(&i)
    }

    /// Replace the selection with a single shape (or clear it when `None`).
    pub(super) fn select_only(&mut self, i: Option<usize>) {
        self.selection.clear();
        if let Some(i) = i {
            self.selection.push(i);
        }
    }

    /// Toggle shape `i` in the selection (shift-click). Re-adding moves it to the
    /// end so it becomes primary.
    pub(super) fn toggle_selection(&mut self, i: usize) {
        if let Some(pos) = self.selection.iter().position(|&s| s == i) {
            self.selection.remove(pos);
        } else {
            self.selection.push(i);
        }
    }

    /// The shapes that select together with shape `i`: the union of `i`'s group
    /// members and its clip-set members (a clipping mask, like a group, selects
    /// and moves as one unit). Sorted, de-duplicated; `[i]` for a loose shape.
    pub(super) fn unit_members_of(&self, i: usize) -> Vec<usize> {
        let gtags = self.group_tags();
        let ctags = self.clip_tags();
        let mut members = group::members_of(&gtags, i);
        members.extend(crate::clip::members_of(&ctags, i));
        members.sort_unstable();
        members.dedup();
        members
    }

    /// Shift-click toggle that treats a group / clip set atomically: if shape `i`
    /// belongs to one, the whole unit is added (when any member is currently
    /// absent) or removed (when all members are present); a loose shape toggles
    /// alone.
    pub(super) fn toggle_group_selection(&mut self, i: usize) {
        let members = self.unit_members_of(i);
        if members.len() <= 1 {
            self.toggle_selection(i);
            return;
        }
        let all_present = members.iter().all(|m| self.is_selected(*m));
        if all_present {
            self.selection.retain(|s| !members.contains(s));
        } else {
            for m in members {
                if !self.is_selected(m) {
                    self.selection.push(m);
                }
            }
        }
    }

    // --- Grouping ------------------------------------------------------------

    /// The per-shape group tags in paint order, for the pure [`group`] helpers.
    pub(super) fn group_tags(&self) -> Vec<Option<u64>> {
        self.doc.shapes.iter().map(|s| s.group()).collect()
    }

    /// Expand the current selection so that selecting any member of a group — or
    /// of a clipping mask — selects the whole unit. Keeps the original primary
    /// (last-added) shape as primary when it survives expansion; otherwise the
    /// topmost expanded shape becomes primary. Pure bookkeeping — no undo entry.
    pub(super) fn expand_selection_to_groups(&mut self) {
        // Groups expand via the shared pure helper; clip sets are unioned in.
        let mut expanded = group::expand_selection(&self.group_tags(), &self.selection);
        let ctags = self.clip_tags();
        let mut clip_extra: Vec<usize> = Vec::new();
        for &i in &expanded {
            clip_extra.extend(crate::clip::members_of(&ctags, i));
        }
        expanded.extend(clip_extra);
        expanded.sort_unstable();
        expanded.dedup();
        // Compare against the normalised current selection; bail if unchanged.
        let mut current = self.selection.clone();
        current.sort_unstable();
        current.dedup();
        if expanded == current {
            return;
        }
        // Preserve the primary if it is still present; else use the topmost.
        let primary = self.primary().filter(|p| expanded.contains(p));
        self.selection = expanded;
        if let Some(p) = primary {
            self.selection.retain(|&i| i != p);
            self.selection.push(p);
        }
    }

    /// Whether the current selection can be grouped (≥2 distinct shapes).
    pub(super) fn can_group(&self) -> bool {
        group::can_group(self.doc.shapes.len(), &self.selection)
    }

    /// Whether the current selection contains any grouped shape (so Ungroup is
    /// meaningful).
    pub(super) fn can_ungroup(&self) -> bool {
        group::can_ungroup(&self.group_tags(), &self.selection)
    }

    /// Group the selected shapes: tag them all with a fresh group id, then gather
    /// them into one contiguous block in paint order (anchored at the topmost
    /// selected shape, preserving their relative order) so the group reads as a
    /// single stacked unit, à la Illustrator. One undo step; the selection is
    /// remapped to the moved shapes so the group stays selected.
    pub(super) fn group_selection(&mut self) {
        if !self.can_group() {
            return;
        }
        self.checkpoint();
        let id = group::next_group_id(&self.group_tags());

        // Sorted, de-duplicated, in-range selection in paint order.
        let mut sel: Vec<usize> = self
            .selection
            .iter()
            .copied()
            .filter(|&i| i < self.doc.shapes.len())
            .collect();
        sel.sort_unstable();
        sel.dedup();

        // Tag the members, then lift them out and re-insert as a block. The block
        // lands so its top sits where the topmost selected shape was, matching
        // "group in place" (Illustrator keeps the group at the frontmost member).
        for &i in &sel {
            self.doc.shapes[i].set_group(Some(id));
        }
        let top = *sel.last().expect("non-empty");
        // Count how many unselected shapes sit at or below `top`; the block is
        // inserted right after them.
        let insert_at = (0..=top).filter(|i| !sel.contains(i)).count();

        // Remove members (highest first to keep indices valid) collecting them in
        // ascending order.
        let mut block: Vec<Shape> = Vec::with_capacity(sel.len());
        for &i in sel.iter().rev() {
            block.push(self.doc.shapes.remove(i));
        }
        block.reverse(); // restore ascending paint order
        for (k, shape) in block.into_iter().enumerate() {
            self.doc.shapes.insert(insert_at + k, shape);
        }
        // The group now occupies a contiguous run starting at `insert_at`.
        self.selection = (insert_at..insert_at + sel.len()).collect();
        self.status = "Grouped".into();
    }

    /// Ungroup: clear the group tag on every selected shape (and, so an Illustrator
    /// "select a group then ungroup" gesture works, on every other member of any
    /// group a selected shape belongs to). One undo step.
    pub(super) fn ungroup_selection(&mut self) {
        if !self.can_ungroup() {
            return;
        }
        self.checkpoint();
        let tags = self.group_tags();
        // Group ids touched by the selection.
        let mut ids: Vec<u64> = self
            .selection
            .iter()
            .filter_map(|&i| tags.get(i).copied().flatten())
            .collect();
        ids.sort_unstable();
        ids.dedup();
        for s in self.doc.shapes.iter_mut() {
            if s.group().is_some_and(|g| ids.binary_search(&g).is_ok()) {
                s.set_group(None);
            }
        }
        self.status = "Ungrouped".into();
    }

    // --- Clipping masks ------------------------------------------------------

    /// The per-shape clip tags in paint order, for the pure [`clip`] helpers.
    pub(super) fn clip_tags(&self) -> Vec<crate::clip::ClipTag> {
        self.doc.shapes.iter().map(|s| s.clip_tag()).collect()
    }

    /// Whether the selection can be made into a clipping mask (≥2 distinct
    /// shapes, none already clipped). Drives menu / button enablement.
    pub(super) fn can_make_clip(&self) -> bool {
        crate::clip::can_make(&self.clip_tags(), &self.selection)
    }

    /// Whether the selection touches any clip set (so Release is meaningful).
    pub(super) fn can_release_clip(&self) -> bool {
        crate::clip::can_release(&self.clip_tags(), &self.selection)
    }

    /// Make a clipping mask from the selection (Illustrator's
    /// `Object → Clipping Mask → Make`): the **topmost** selected shape becomes
    /// the mask and the shapes below it are clipped to its outline. Tags all
    /// members with a fresh clip-set id, flags the top one as the mask, and
    /// gathers them into one contiguous block anchored at the top (so the set
    /// reads as a unit and the mask sits above its content). One undo step; the
    /// selection is remapped to the moved block so the set stays selected.
    pub(super) fn make_clip(&mut self) {
        if !self.can_make_clip() {
            self.status = "Clipping mask: select two or more unclipped objects".into();
            return;
        }
        self.checkpoint();
        let id = crate::clip::next_clip_id(&self.clip_tags());

        // Sorted, de-duplicated, in-range selection in paint order.
        let mut sel: Vec<usize> = self
            .selection
            .iter()
            .copied()
            .filter(|&i| i < self.doc.shapes.len())
            .collect();
        sel.sort_unstable();
        sel.dedup();

        // The topmost (frontmost) selected shape is the mask.
        let top = *sel.last().expect("non-empty");
        for &i in &sel {
            self.doc.shapes[i].set_clip(Some(id));
            self.doc.shapes[i].set_mask(i == top);
        }

        // Gather the members into one contiguous block ending at the top's slot,
        // preserving relative order (mask stays last = frontmost), mirroring how
        // grouping re-stacks. Insert right after the unselected shapes at/below
        // `top`.
        let insert_at = (0..=top).filter(|i| !sel.contains(i)).count();
        let mut block: Vec<Shape> = Vec::with_capacity(sel.len());
        for &i in sel.iter().rev() {
            block.push(self.doc.shapes.remove(i));
        }
        block.reverse();
        for (k, shape) in block.into_iter().enumerate() {
            self.doc.shapes.insert(insert_at + k, shape);
        }
        self.selection = (insert_at..insert_at + sel.len()).collect();
        self.status = "Made clipping mask".into();
    }

    /// Release every clip set the selection touches (Illustrator's
    /// `Object → Clipping Mask → Release`): clears the clip / mask tags on all
    /// members so the originals reappear unclipped. One undo step.
    pub(super) fn release_clip(&mut self) {
        if !self.can_release_clip() {
            return;
        }
        self.checkpoint();
        let tags = self.clip_tags();
        let ids = crate::clip::selected_clip_ids(&tags, &self.selection);
        for s in self.doc.shapes.iter_mut() {
            if s.clip().is_some_and(|c| ids.binary_search(&c).is_ok()) {
                s.clear_clip();
            }
        }
        self.status = "Released clipping mask".into();
    }

    // --- Opacity masks -------------------------------------------------------

    /// The per-shape opacity-mask tags in paint order, for the pure
    /// [`opacity_mask`](crate::opacity_mask) helpers.
    pub(super) fn omask_tags(&self) -> Vec<crate::opacity_mask::OMaskTag> {
        self.doc
            .shapes
            .iter()
            .map(|s| crate::opacity_mask::OMaskTag::new(s.omask(), s.is_omask()))
            .collect()
    }

    /// Whether the selection can be made into an opacity mask (≥2 distinct
    /// unmasked shapes). Drives menu / button enablement.
    pub(super) fn can_make_omask(&self) -> bool {
        crate::opacity_mask::can_make(&self.omask_tags(), &self.selection)
    }

    /// Whether the selection touches any opacity-mask set (so Release is useful).
    pub(super) fn can_release_omask(&self) -> bool {
        crate::opacity_mask::can_release(&self.omask_tags(), &self.selection)
    }

    /// Make an opacity mask from the selection (Illustrator's
    /// `Object ▸ Opacity Mask ▸ Make`): the **topmost** selected shape becomes the
    /// luminance mask and the shapes below it are masked by it. Tags all members
    /// with a fresh opacity-mask id, flags the top one as the mask path, and
    /// gathers them into one contiguous block anchored at the top (mirroring how
    /// clipping masks re-stack). One undo step; the selection is remapped to the
    /// moved block so the set stays selected.
    pub(super) fn make_omask(&mut self) {
        if !self.can_make_omask() {
            self.status = "Opacity mask: select two or more unmasked objects".into();
            return;
        }
        self.checkpoint();
        let id = crate::opacity_mask::next_omask_id(&self.omask_tags());

        let mut sel: Vec<usize> = self
            .selection
            .iter()
            .copied()
            .filter(|&i| i < self.doc.shapes.len())
            .collect();
        sel.sort_unstable();
        sel.dedup();

        // The topmost (frontmost) selected shape is the mask.
        let top = *sel.last().expect("non-empty");
        for &i in &sel {
            self.doc.shapes[i].set_omask(Some(id));
            self.doc.shapes[i].set_omask_path(i == top);
        }

        // Gather the members into one contiguous block ending at the top's slot,
        // preserving relative order (mask stays last = frontmost).
        let insert_at = (0..=top).filter(|i| !sel.contains(i)).count();
        let mut block: Vec<Shape> = Vec::with_capacity(sel.len());
        for &i in sel.iter().rev() {
            block.push(self.doc.shapes.remove(i));
        }
        block.reverse();
        for (k, shape) in block.into_iter().enumerate() {
            self.doc.shapes.insert(insert_at + k, shape);
        }
        self.selection = (insert_at..insert_at + sel.len()).collect();
        self.status = "Made opacity mask".into();
    }

    /// Release every opacity-mask set the selection touches (Illustrator's
    /// `Object ▸ Opacity Mask ▸ Release`): clears the mask tags on all members so
    /// the originals reappear at full opacity. One undo step.
    pub(super) fn release_omask(&mut self) {
        if !self.can_release_omask() {
            return;
        }
        self.checkpoint();
        let tags = self.omask_tags();
        let ids = crate::opacity_mask::selected_omask_ids(&tags, &self.selection);
        for s in self.doc.shapes.iter_mut() {
            if s.omask().is_some_and(|c| ids.binary_search(&c).is_ok()) {
                s.clear_omask();
            }
        }
        self.status = "Released opacity mask".into();
    }

    /// Toggle the **invert** flag on the masked content of every opacity-mask set
    /// the selection touches (the mask path itself carries no invert). One undo
    /// step. Returns whether anything changed.
    pub(super) fn toggle_omask_invert(&mut self) -> bool {
        if !self.can_release_omask() {
            return false;
        }
        self.checkpoint();
        let tags = self.omask_tags();
        let ids = crate::opacity_mask::selected_omask_ids(&tags, &self.selection);
        let mut changed = false;
        for s in self.doc.shapes.iter_mut() {
            if s.omask().is_some_and(|c| ids.binary_search(&c).is_ok()) && !s.is_omask() {
                let inv = !s.omask_invert();
                s.set_omask_invert(inv);
                changed = true;
            }
        }
        if changed {
            self.status = "Toggled opacity-mask invert".into();
        }
        changed
    }

    // --- Blend (Object ▸ Blend) ----------------------------------------------

    /// The per-shape blend-set tags in paint order, for the pure [`blend`]
    /// helpers.
    pub(super) fn blend_tags(&self) -> Vec<Option<u64>> {
        self.doc.shapes.iter().map(|s| s.blend()).collect()
    }

    /// Whether the selection can be blended (exactly two distinct un-blended
    /// shapes). Drives menu / button enablement.
    pub(super) fn can_make_blend(&self) -> bool {
        crate::blend::can_make(&self.blend_tags(), &self.selection)
    }

    /// Whether the selection touches any blend set (so Release / Expand is useful).
    pub(super) fn can_release_blend(&self) -> bool {
        crate::blend::can_release(&self.blend_tags(), &self.selection)
    }

    /// Make a blend from the two selected objects (Illustrator's
    /// `Object ▸ Blend ▸ Make`, specified-steps mode): generate `self.blend_steps`
    /// intermediate shapes that morph between them — interpolating position, path
    /// geometry (arc-length resampled, point-by-point), and appearance (fill /
    /// stroke colour + opacity + stroke width) — and insert them, in order,
    /// between the two ends. The two ends plus the generated steps are tagged with
    /// a fresh blend-set id so they select / move as a unit and **Release** can
    /// undo it. **Expand-on-create**: the steps are real objects (a live re-blend
    /// when an end moves is a noted gap). One undo step; the new run is selected.
    pub(super) fn make_blend(&mut self) {
        if !self.can_make_blend() {
            self.status = "Blend: select exactly two un-blended objects".into();
            return;
        }
        // The two ends in paint order: lower index is the back end (t→0), higher
        // is the front end (t→1).
        let mut sel: Vec<usize> = self
            .selection
            .iter()
            .copied()
            .filter(|&i| i < self.doc.shapes.len())
            .collect();
        sel.sort_unstable();
        sel.dedup();
        let (lo, hi) = (sel[0], sel[1]);

        let steps = self.blend_steps;
        let mut a = self.doc.shapes[lo].clone();
        let mut b = self.doc.shapes[hi].clone();
        let mut generated = crate::blend::make_steps(&a, &b, steps);

        self.checkpoint();
        let id = crate::blend::next_blend_id(&self.blend_tags());

        // Tag the two ends (set members, not steps) and every generated step.
        a.set_blend(Some(id));
        b.set_blend(Some(id));
        for g in generated.iter_mut() {
            g.set_blend(Some(id));
            g.set_blend_step(true);
        }

        // Gather the whole run into one contiguous block — [a, step1 … stepN, b]
        // — anchored where the back end was, so the blend reads as a unit even if
        // the two ends weren't adjacent. Remove the two ends (highest first to
        // keep indices valid), then splice the ordered block in at the back end's
        // slot, mirroring how grouping re-stacks a selection.
        self.doc.shapes.remove(hi);
        self.doc.shapes.remove(lo);
        let insert_at = lo; // `lo < hi`, so removing `hi` first leaves `lo` valid.
        let mut block: Vec<Shape> = Vec::with_capacity(generated.len() + 2);
        block.push(a);
        block.extend(generated);
        block.push(b);
        let run_len = block.len();
        for (k, shape) in block.into_iter().enumerate() {
            self.doc.shapes.insert(insert_at + k, shape);
        }
        self.selection = (insert_at..insert_at + run_len).collect();
        let n_steps = run_len - 2;
        self.status = format!(
            "Blended ({n_steps} step{})",
            if n_steps == 1 { "" } else { "s" }
        );
    }

    /// Release every blend set the selection touches (Illustrator's
    /// `Object ▸ Blend ▸ Release`): delete the generated intermediate steps and
    /// clear the blend tags on the surviving ends, restoring the two originals.
    /// One undo step.
    pub(super) fn release_blend(&mut self) {
        if !self.can_release_blend() {
            return;
        }
        self.checkpoint();
        let ids = crate::blend::selected_blend_ids(&self.blend_tags(), &self.selection);
        // Drop the generated steps of the touched sets (highest index first so the
        // lower indices stay valid), then clear tags on the surviving ends.
        let to_remove: Vec<usize> = self
            .doc
            .shapes
            .iter()
            .enumerate()
            .filter(|(_, s)| {
                s.is_blend_step() && s.blend().is_some_and(|b| ids.binary_search(&b).is_ok())
            })
            .map(|(i, _)| i)
            .collect();
        for &i in to_remove.iter().rev() {
            self.remove_shape(i);
        }
        for s in self.doc.shapes.iter_mut() {
            if s.blend().is_some_and(|b| ids.binary_search(&b).is_ok()) {
                s.clear_blend();
            }
        }
        self.status = "Released blend".into();
    }

    /// Expand the touched blend sets (Illustrator's `Object ▸ Blend ▸ Expand`):
    /// since the steps are already real objects, this simply clears the blend tags
    /// — including the step flag — so the run becomes a set of plain, independent
    /// objects. One undo step.
    pub(super) fn expand_blend(&mut self) {
        if !self.can_release_blend() {
            return;
        }
        self.checkpoint();
        let ids = crate::blend::selected_blend_ids(&self.blend_tags(), &self.selection);
        for s in self.doc.shapes.iter_mut() {
            if s.blend().is_some_and(|b| ids.binary_search(&b).is_ok()) {
                s.clear_blend();
            }
        }
        self.status = "Expanded blend".into();
    }

    // --- Path editing (Simplify / Offset Path) -------------------------------

    /// Whether Simplify / Offset Path can act on the current selection: a single
    /// outline shape is primary. The op demotes the shape to a plain path first
    /// (Text → glyph compound), so any shape with geometry qualifies.
    pub(super) fn can_edit_path(&self) -> bool {
        self.primary()
            .and_then(|i| self.doc.shapes.get(i))
            .is_some()
    }

    /// **Simplify** the selected path: flatten its outline, run Douglas–Peucker
    /// anchor reduction at `self.simplify_tol`, and write the result back as a
    /// plain corner path (dropping any `live` parametric params). One undo step.
    pub(super) fn simplify_selected(&mut self) {
        let tol = self.simplify_tol;
        let applied = self.apply_path_geometry(|pts, closed| crate::pathedit::simplify(pts, closed, tol));
        if let Some(n) = applied {
            self.status = format!("Simplified to {n} anchor{}", if n == 1 { "" } else { "s" });
        } else {
            self.status = "Simplify: select a path".into();
        }
    }

    /// **Offset Path** on the selected path: flatten its outline, offset it by
    /// the signed `self.offset_dist` (miter joins; + grows a closed path, −
    /// shrinks it), and write the result back as a plain corner path. One undo
    /// step.
    pub(super) fn offset_selected(&mut self) {
        let dist = self.offset_dist;
        let applied =
            self.apply_path_geometry(|pts, closed| crate::pathedit::offset_path(pts, closed, dist));
        if applied.is_some() {
            self.status = format!("Offset path by {dist:.1}");
        } else {
            self.status = "Offset Path: select a path".into();
        }
    }

    /// Whether **Outline Stroke** can act on the selection: a single shape that
    /// is currently stroked (positive width and a non-transparent stroke colour).
    pub(super) fn can_outline_stroke(&self) -> bool {
        let Some(s) = self.primary().and_then(|i| self.doc.shapes.get(i)) else {
            return false;
        };
        s.stroke_width() > 0.0 && s.stroke_color().is_some_and(|c| c[3] > 0.0)
    }

    /// **Outline Stroke** on the selected path: convert its stroke into a filled
    /// outline shape (Illustrator's `Object ▸ Path ▸ Outline Stroke`). Each
    /// (sub)contour is replaced by the band / annulus its centred stroke covers
    /// (`pathedit::outline_stroke` at half the stroke width); the result is one
    /// shape whose **fill** is the former stroke colour with **no stroke**.
    ///
    /// An open path yields a single closed band; a closed path (or a compound /
    /// multi-contour shape) yields a [`Shape::Compound`] of every band / annulus
    /// ring, filled even-odd so a closed ring's interior is carved out. `live`
    /// params are dropped (the result is a plain corner path), and a shape with
    /// no visible stroke is a no-op. One undo step.
    pub(super) fn outline_stroke_selected(&mut self) {
        let Some(i) = self.primary() else {
            self.status = "Outline Stroke: select a path".into();
            return;
        };
        let Some(src) = self.doc.shapes.get(i).cloned() else {
            return;
        };
        let stroke = src.stroke_color().unwrap_or([0.0, 0.0, 0.0, 1.0]);
        let half_w = src.stroke_width() * 0.5;
        if half_w <= 0.0 || stroke[3] <= 0.0 {
            self.status = "Outline Stroke: nothing to outline".into();
            return;
        }

        // Demote to a path/compound, then outline each (sub)contour into one or
        // more closed rings.
        let demoted = src.to_path();
        let contours: Vec<(Vec<(f32, f32)>, bool)> = match &demoted {
            Shape::Path {
                points,
                handles,
                closed,
                ..
            } => {
                let flat = crate::document::flatten(points, handles, *closed);
                vec![(flat, *closed)]
            }
            Shape::Compound { subpaths, .. } => subpaths
                .iter()
                .map(|sp| (sp.flatten(), sp.closed))
                .collect(),
            _ => Vec::new(),
        };

        let mut rings: Vec<crate::document::SubPath> = Vec::new();
        let mut all_closed = true;
        for (flat, closed) in &contours {
            for ring in crate::pathedit::outline_stroke(flat, *closed, half_w) {
                if ring.len() >= 3 {
                    rings.push(crate::document::SubPath::ring(ring));
                }
            }
            all_closed &= *closed;
        }
        if rings.is_empty() {
            self.status = "Outline Stroke: nothing to outline".into();
            return;
        }

        self.checkpoint();
        // A single open path's band is one ring — keep it a plain Path. Anything
        // with an annulus or multiple rings becomes an even-odd Compound so the
        // closed-path hole is carved.
        let outlined = if rings.len() == 1 && !all_closed {
            let ring = rings.into_iter().next().expect("one ring");
            Shape::Path {
                points: ring.points,
                closed: true,
                fill: stroke,
                fill_gradient: None,
                stroke: [0.0, 0.0, 0.0, 0.0],
                stroke_w: 0.0,
                stroke_style: src.stroke_style().clone(),
                handles: ring.handles,
                live: None,
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
        } else {
            Shape::Compound {
                subpaths: rings,
                fill_rule: crate::document::FillRule::EvenOdd,
                fill: stroke,
                fill_gradient: None,
                stroke: [0.0, 0.0, 0.0, 0.0],
                stroke_w: 0.0,
                stroke_style: src.stroke_style().clone(),
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
        };
        self.doc.shapes[i] = outlined;
        self.select_only(Some(i));
        self.status = "Outlined stroke".into();
    }

    /// Demote the primary-selected shape to a plain [`Shape::Path`] (via
    /// [`Shape::to_path`], preserving paint / group / membership tags), flatten
    /// its outline to a polyline, run `op` on it, and store the result back as a
    /// corner path with no `live` params. Returns the resulting anchor count, or
    /// `None` when nothing suitable is selected / the result is degenerate. One
    /// undo step (checkpoint taken only when an edit is actually applied).
    ///
    /// A `Compound` is offset / simplified per sub-contour, keeping it a compound.
    fn apply_path_geometry(
        &mut self,
        op: impl Fn(&[(f32, f32)], bool) -> Vec<(f32, f32)>,
    ) -> Option<usize> {
        let i = self.primary()?;
        let src = self.doc.shapes.get(i)?.clone();
        // A bare Line / open-ended primitive still demotes to a path; Text
        // demotes to a compound of glyph outlines.
        let demoted = src.to_path();

        match demoted {
            Shape::Path { .. } => {
                let (pts, handles, closed) = match &demoted {
                    Shape::Path {
                        points,
                        handles,
                        closed,
                        ..
                    } => (points.clone(), handles.clone(), *closed),
                    _ => unreachable!(),
                };
                let flat = crate::document::flatten(&pts, &handles, closed);
                if flat.len() < 2 {
                    return None;
                }
                let out = op(&flat, closed);
                let min = if closed { 3 } else { 2 };
                if out.len() < min {
                    return None;
                }
                let count = out.len();
                self.checkpoint();
                let mut shape = demoted;
                if let Shape::Path {
                    points,
                    handles,
                    live,
                    ..
                } = &mut shape
                {
                    let n = out.len();
                    *points = out;
                    *handles = vec![(0.0, 0.0); n];
                    *live = None;
                }
                self.doc.shapes[i] = shape;
                self.select_only(Some(i));
                Some(count)
            }
            mut shape @ Shape::Compound { .. } => {
                let Shape::Compound { subpaths, .. } = &mut shape else {
                    unreachable!()
                };
                let mut total = 0usize;
                let mut any = false;
                for sp in subpaths.iter_mut() {
                    let flat = sp.flatten();
                    if flat.len() < 2 {
                        continue;
                    }
                    let out = op(&flat, sp.closed);
                    let min = if sp.closed { 3 } else { 2 };
                    if out.len() < min {
                        continue;
                    }
                    let n = out.len();
                    sp.points = out;
                    sp.handles = vec![(0.0, 0.0); n];
                    total += n;
                    any = true;
                }
                if !any {
                    return None;
                }
                self.checkpoint();
                self.doc.shapes[i] = shape;
                self.select_only(Some(i));
                Some(total)
            }
            _ => None,
        }
    }

    // --- Compound paths ------------------------------------------------------

    /// Whether the selection can be made into a compound path: at least two
    /// distinct shapes that each contribute a closed sub-contour. (One shape that
    /// is already a compound, or a single shape, has nothing to combine.)
    pub(super) fn can_make_compound(&self) -> bool {
        let mut sel: Vec<usize> = self
            .selection
            .iter()
            .copied()
            .filter(|&i| i < self.doc.shapes.len())
            .collect();
        sel.sort_unstable();
        sel.dedup();
        if sel.len() < 2 {
            return false;
        }
        // Every selected shape must contribute at least one closed contour.
        sel.iter()
            .filter(|&&i| compound_subpaths(&self.doc.shapes[i]).is_some())
            .count()
            >= 2
    }

    /// Whether the selection touches any compound path (so Release is meaningful).
    pub(super) fn can_release_compound(&self) -> bool {
        self.selection
            .iter()
            .filter_map(|&i| self.doc.shapes.get(i))
            .any(|s| matches!(s, Shape::Compound { .. }))
    }

    /// Make a compound path from the selection (Illustrator's
    /// `Object ▸ Compound Path ▸ Make`, `Cmd/Ctrl+8`): gather every selected
    /// shape's closed contour(s) into one [`Shape::Compound`] with the app's
    /// current fill rule, anchored where the topmost selected shape was, taking
    /// the *bottom* (back-most) selected shape's paint — matching Illustrator,
    /// where a compound path adopts the back object's appearance. One undo step;
    /// the new compound is selected.
    pub(super) fn make_compound(&mut self) {
        if !self.can_make_compound() {
            self.status = "Compound path: select two or more closed shapes".into();
            return;
        }
        let mut sel: Vec<usize> = self
            .selection
            .iter()
            .copied()
            .filter(|&i| i < self.doc.shapes.len())
            .collect();
        sel.sort_unstable();
        sel.dedup();

        // Collect every contributing sub-contour in paint order; the back-most
        // (lowest index) contributing shape supplies the compound's paint.
        let mut subpaths: Vec<crate::document::SubPath> = Vec::new();
        let mut style: Option<Shape> = None;
        for &i in &sel {
            if let Some(subs) = compound_subpaths(&self.doc.shapes[i]) {
                if style.is_none() {
                    style = Some(self.doc.shapes[i].clone());
                }
                subpaths.extend(subs);
            }
        }
        let Some(style) = style else {
            return;
        };

        self.checkpoint();
        let top = *sel.last().expect("non-empty");
        let insert_at = (0..=top).filter(|i| !sel.contains(i)).count();
        // Remove the members (highest first), then insert the compound.
        for &i in sel.iter().rev() {
            self.doc.shapes.remove(i);
        }
        let compound = Shape::Compound {
            subpaths,
            fill_rule: self.compound_fill_rule_doc(),
            fill: style.fill_color().unwrap_or([0.5, 0.5, 0.5, 1.0]),
            fill_gradient: style.fill_gradient().cloned(),
            stroke: style.stroke_color().unwrap_or([0.0, 0.0, 0.0, 1.0]),
            stroke_w: style.stroke_width(),
            stroke_style: style.stroke_style().clone(),
            appearance: style.appearance().cloned(),
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
        };
        self.doc.shapes.insert(insert_at, compound);
        self.select_only(Some(insert_at));
        self.status = "Made compound path".into();
    }

    /// Release every compound path the selection touches (Illustrator's
    /// `Object ▸ Compound Path ▸ Release`): split each back into one closed
    /// [`Shape::Path`] per sub-contour, all inheriting the compound's paint. One
    /// undo step; the released paths are selected.
    pub(super) fn release_compound(&mut self) {
        if !self.can_release_compound() {
            return;
        }
        self.checkpoint();
        // Indices of selected compounds, highest first so removals stay valid.
        let mut idx: Vec<usize> = self
            .selection
            .iter()
            .copied()
            .filter(|&i| matches!(self.doc.shapes.get(i), Some(Shape::Compound { .. })))
            .collect();
        idx.sort_unstable();
        idx.dedup();
        let mut new_selection: Vec<usize> = Vec::new();
        // Process highest first; track produced indices by re-scanning after.
        for &i in idx.iter().rev() {
            let compound = self.doc.shapes.remove(i);
            let paths = release_compound_to_paths(&compound);
            let n = paths.len();
            for (k, p) in paths.into_iter().enumerate() {
                self.doc.shapes.insert(i + k, p);
            }
            // The released run occupies i..i+n.
            for k in 0..n {
                new_selection.push(i + k);
            }
            // Shift previously-recorded indices that sit above this insertion.
            // (We process descending, so earlier-recorded ones are at higher
            // indices and unaffected; nothing to fix up.)
        }
        new_selection.sort_unstable();
        self.selection = new_selection;
        self.status = "Released compound path".into();
    }

    /// The app's compound fill rule as the document-model [`FillRule`].
    fn compound_fill_rule_doc(&self) -> crate::document::FillRule {
        match self.bool_fill_rule {
            crate::boolean::BoolFillRule::EvenOdd => crate::document::FillRule::EvenOdd,
            crate::boolean::BoolFillRule::NonZero => crate::document::FillRule::NonZero,
        }
    }

    /// Set the fill rule of every selected compound path (one undo step). Returns
    /// whether anything changed.
    pub(super) fn set_compound_fill_rule(&mut self, rule: crate::document::FillRule) -> bool {
        let targets: Vec<usize> = self
            .selection
            .iter()
            .copied()
            .filter(|&i| matches!(self.doc.shapes.get(i), Some(Shape::Compound { .. })))
            .collect();
        if targets.is_empty() {
            return false;
        }
        // Skip if nothing would change.
        let already = targets
            .iter()
            .all(|&i| self.doc.shapes[i].fill_rule() == Some(rule));
        if already {
            return false;
        }
        self.checkpoint();
        for i in targets {
            if let Some(Shape::Compound { fill_rule, .. }) = self.doc.shapes.get_mut(i) {
                *fill_rule = rule;
            }
        }
        self.status = "Set compound fill rule".into();
        true
    }

    // --- Shape Builder -------------------------------------------------------

    /// Commit a Shape Builder gesture: pick the faces the drag `path` crossed,
    /// merge (or subtract) them, and replace the `sources` shapes with the result.
    /// One undo step. Picking nothing (a click that hit no face, or a drag that
    /// stayed off the artwork) does nothing.
    pub(super) fn finish_shape_builder(
        &mut self,
        faces: Vec<crate::shapebuilder::Face>,
        sources: Vec<usize>,
        path: Vec<(f32, f32)>,
        subtract: bool,
    ) {
        let picked = crate::shapebuilder::faces_along(&faces, &path);
        if picked.is_empty() {
            self.status = "Shape Builder: drag across the regions".into();
            return;
        }
        let shapes: Vec<Shape> = sources
            .iter()
            .filter(|&&i| i < self.doc.shapes.len())
            .map(|&i| self.doc.shapes[i].clone())
            .collect();
        let mode = if subtract {
            crate::shapebuilder::BuildMode::Subtract
        } else {
            crate::shapebuilder::BuildMode::Unite
        };
        let result = crate::shapebuilder::apply_build(&shapes, &faces, &picked, mode);

        self.checkpoint();
        // Replace the source shapes (highest index first so the rest stay valid)
        // with the result block, anchored where the back-most source was.
        let mut srcs = sources.clone();
        srcs.sort_unstable();
        srcs.dedup();
        let insert_at = srcs.first().copied().unwrap_or(self.doc.shapes.len());
        let insert_at = insert_at.min(self.doc.shapes.len());
        for &i in srcs.iter().rev() {
            if i < self.doc.shapes.len() {
                self.doc.shapes.remove(i);
            }
        }
        // Count shapes removed below the insertion point to keep it valid.
        let removed_below = srcs.iter().filter(|&&i| i < insert_at).count();
        let at = insert_at - removed_below;
        let n = result.len();
        for (k, s) in result.into_iter().enumerate() {
            self.doc.shapes.insert(at + k, s);
        }
        self.selection = (at..at + n).collect();
        self.status = if subtract {
            format!("Shape Builder: deleted {} region(s)", picked.len())
        } else {
            format!("Shape Builder: united {} region(s)", picked.len())
        };
    }

    // --- Undo / redo ---------------------------------------------------------

    /// Record the current document as an undo checkpoint *before* applying a
    /// discrete (non-drag) edit. Call this immediately prior to mutating
    /// `self.doc`.
    pub(super) fn checkpoint(&mut self) {
        self.history.push(self.doc.clone());
    }

    /// Snapshot the start of a continuous interaction (drag). Idempotent within
    /// a drag, so per-frame calls coalesce into one undo entry.
    pub(super) fn begin_interaction(&mut self) {
        self.history.begin(&self.doc);
    }

    /// Finalize a continuous interaction; drops the checkpoint if nothing
    /// actually changed.
    pub(super) fn commit_interaction(&mut self) {
        self.history.commit(&self.doc);
    }

    pub(super) fn undo(&mut self) {
        if let Some(prev) = self.history.undo(&self.doc) {
            self.doc = prev;
            self.clamp_selection();
            self.status = "Undo".into();
        }
    }

    pub(super) fn redo(&mut self) {
        if let Some(next) = self.history.redo(&self.doc) {
            self.doc = next;
            self.clamp_selection();
            self.status = "Redo".into();
        }
    }

    /// Drop selection indices that fall outside the (possibly restored) doc.
    fn clamp_selection(&mut self) {
        let n = self.doc.shapes.len();
        self.selection.retain(|&i| i < n);
    }

    /// Remove shape `i`, fixing up the selection indices (entries above shift
    /// down by one; the removed entry, if selected, is dropped).
    pub(super) fn remove_shape(&mut self, i: usize) {
        if i >= self.doc.shapes.len() {
            return;
        }
        self.doc.shapes.remove(i);
        self.selection.retain(|&s| s != i);
        for s in self.selection.iter_mut() {
            if *s > i {
                *s -= 1;
            }
        }
    }

    pub(super) fn delete_selected(&mut self) {
        if self.selection.is_empty() {
            return;
        }
        self.checkpoint();
        // Remove highest index first so lower indices stay valid.
        let mut idx: Vec<usize> = std::mem::take(&mut self.selection);
        idx.sort_unstable();
        idx.dedup();
        for &i in idx.iter().rev() {
            if i < self.doc.shapes.len() {
                self.doc.shapes.remove(i);
            }
        }
    }

    // --- Clipboard: copy / cut / paste / duplicate --------------------------

    /// Whether anything is on the clipboard (drives menu enablement).
    pub(super) fn can_paste(&self) -> bool {
        !self.clipboard.is_empty()
    }

    /// The currently-selected shapes (expanded to whole groups) in paint order,
    /// snapshotted for the clipboard. Returns the indices it gathered so callers
    /// that also delete (Cut) can reuse them.
    fn selection_in_paint_order(&self) -> Vec<usize> {
        let mut idx: Vec<usize> = self
            .selection
            .iter()
            .copied()
            .filter(|&i| i < self.doc.shapes.len())
            .collect();
        idx.sort_unstable();
        idx.dedup();
        idx
    }

    /// Copy the selection (whole groups, in paint order) onto the clipboard. The
    /// document is untouched, so Copy records no undo step.
    pub(super) fn copy_selection(&mut self) {
        let idx = self.selection_in_paint_order();
        if idx.is_empty() {
            return;
        }
        let shapes: Vec<Shape> = idx.iter().map(|&i| self.doc.shapes[i].clone()).collect();
        let n = shapes.len();
        self.clipboard.set(shapes);
        self.status = format!("Copied {n} {}", if n == 1 { "object" } else { "objects" });
    }

    /// Cut: copy the selection to the clipboard, then delete it (one undo step).
    pub(super) fn cut_selection(&mut self) {
        let idx = self.selection_in_paint_order();
        if idx.is_empty() {
            return;
        }
        let shapes: Vec<Shape> = idx.iter().map(|&i| self.doc.shapes[i].clone()).collect();
        let n = shapes.len();
        self.clipboard.set(shapes);
        self.checkpoint();
        for &i in idx.iter().rev() {
            self.doc.shapes.remove(i);
        }
        self.selection.clear();
        self.status = format!("Cut {n} {}", if n == 1 { "object" } else { "objects" });
    }

    /// Append `shapes` to the top of the document (paint order) as one undo step,
    /// remapping their group ids so a pasted group stays grouped without
    /// colliding with the document's existing groups, and selecting the result.
    fn paste_shapes(&mut self, mut shapes: Vec<Shape>) {
        if shapes.is_empty() {
            return;
        }
        crate::clipboard::remap_group_ids(&mut shapes, &self.group_tags());
        self.checkpoint();
        let start = self.doc.shapes.len();
        let n = shapes.len();
        self.doc.shapes.extend(shapes);
        self.selection = (start..start + n).collect();
        self.status = format!("Pasted {n} {}", if n == 1 { "object" } else { "objects" });
    }

    /// Insert `shapes` at paint-order position `at` (0 = back) as one undo step,
    /// remapping group ids and selecting the inserted block. Used by paste-in-
    /// front / -back so the paste lands at a precise stacking position.
    fn paste_shapes_at(&mut self, mut shapes: Vec<Shape>, at: usize) {
        if shapes.is_empty() {
            return;
        }
        crate::clipboard::remap_group_ids(&mut shapes, &self.group_tags());
        let at = at.min(self.doc.shapes.len());
        let n = shapes.len();
        self.checkpoint();
        for (k, s) in shapes.into_iter().enumerate() {
            self.doc.shapes.insert(at + k, s);
        }
        self.selection = (at..at + n).collect();
        self.status = format!("Pasted {n} {}", if n == 1 { "object" } else { "objects" });
    }

    /// Plain Paste: drop the clipboard onto the top of the stack, fanned out by a
    /// growing nudge so repeated pastes don't hide behind one another.
    pub(super) fn paste(&mut self) {
        if let Some(shapes) = self.clipboard.take_paste() {
            self.paste_shapes(shapes);
        }
    }

    /// Paste In Place: drop the clipboard at its original coordinates, on top.
    pub(super) fn paste_in_place(&mut self) {
        if let Some(shapes) = self.clipboard.clone_in_place() {
            self.paste_shapes(shapes);
        }
    }

    /// Paste In Front: clipboard at original coordinates, stacked above
    /// everything (front of the paint list).
    pub(super) fn paste_in_front(&mut self) {
        if let Some(shapes) = self.clipboard.clone_in_place() {
            let top = self.doc.shapes.len();
            self.paste_shapes_at(shapes, top);
        }
    }

    /// Paste In Back: clipboard at original coordinates, stacked below
    /// everything (back of the paint list).
    pub(super) fn paste_in_back(&mut self) {
        if let Some(shapes) = self.clipboard.clone_in_place() {
            self.paste_shapes_at(shapes, 0);
        }
    }

    /// Duplicate: copy the selection in place and immediately paste a nudged copy
    /// on top, without disturbing the clipboard — Illustrator's `Ctrl+D`-class
    /// quick-clone. The new copies become the selection.
    pub(super) fn duplicate_selection(&mut self) {
        let idx = self.selection_in_paint_order();
        if idx.is_empty() {
            return;
        }
        let mut shapes: Vec<Shape> = idx.iter().map(|&i| self.doc.shapes[i].clone()).collect();
        let off = crate::clipboard::paste_offset(1);
        for s in shapes.iter_mut() {
            s.translate(off.0, off.1);
        }
        self.paste_shapes(shapes);
        if self.status.starts_with("Pasted") {
            let n = idx.len();
            self.status = format!(
                "Duplicated {n} {}",
                if n == 1 { "object" } else { "objects" }
            );
        }
    }

    // --- Layers panel operations --------------------------------------------

    /// Toggle shape `i`'s visibility as one undo step (Layers eye button).
    pub(super) fn toggle_shape_visible(&mut self, i: usize) {
        if i >= self.doc.shapes.len() {
            return;
        }
        self.checkpoint();
        self.doc.shapes[i].toggle_visible();
    }

    /// Toggle shape `i`'s lock as one undo step (Layers lock button). Locking a
    /// shape that is currently selected drops it from the selection, since a
    /// locked shape can't be part of an editable selection.
    pub(super) fn toggle_shape_locked(&mut self, i: usize) {
        if i >= self.doc.shapes.len() {
            return;
        }
        self.checkpoint();
        self.doc.shapes[i].toggle_locked();
        if self.doc.shapes[i].locked() {
            self.selection.retain(|&s| s != i);
        }
    }

    /// Rename shape `i` from the Layers panel as one undo step. A blank name
    /// clears back to the generic type label.
    pub(super) fn set_shape_name(&mut self, i: usize, name: &str) {
        if i >= self.doc.shapes.len() {
            return;
        }
        // Normalise the way `Shape::set_name` does (blank → cleared), then skip
        // the checkpoint when nothing changes so committing an unedited text
        // field doesn't pile up empty undo steps.
        let trimmed = name.trim();
        let next = (!trimmed.is_empty()).then(|| trimmed.to_string());
        if self.doc.shapes[i].name().map(str::to_string) == next {
            return;
        }
        self.checkpoint();
        self.doc.shapes[i].set_name(name);
    }

    /// Set (or clear, with `None`) shape `i`'s Layers-panel colour as one undo
    /// step.
    pub(super) fn set_shape_layer_color(&mut self, i: usize, color: Option<[f32; 4]>) {
        if i >= self.doc.shapes.len() {
            return;
        }
        self.checkpoint();
        self.doc.shapes[i].set_layer_color(color);
    }

    /// Select shape `i` from the Layers panel (the row's target affordance),
    /// expanding to its whole group / clip unit so panel ↔ canvas selection stay
    /// in sync. A **locked** shape can't be selected, so the click is ignored.
    pub(super) fn select_shape_from_panel(&mut self, i: usize, additive: bool) {
        if i >= self.doc.shapes.len() || self.doc.shapes[i].locked() {
            return;
        }
        if additive {
            self.toggle_group_selection(i);
        } else {
            self.select_only(Some(i));
            self.expand_selection_to_groups();
        }
    }

    /// Select every member of group `g` (the Layers group header's target
    /// affordance). No-op if the group has no selectable (unlocked) members.
    pub(super) fn select_group_from_panel(&mut self, g: u64) {
        let members: Vec<usize> = self
            .doc
            .shapes
            .iter()
            .enumerate()
            .filter(|(_, s)| s.group() == Some(g) && !s.locked())
            .map(|(i, _)| i)
            .collect();
        if members.is_empty() {
            return;
        }
        self.selection = members;
    }

    /// Reorder a single shape `i` in paint order (the Layers-panel reorder
    /// buttons), independent of the current selection: the op is computed for
    /// just `{i}`, and the real selection is remapped through the same
    /// permutation so it follows the move. One undo step; no-op if the move
    /// wouldn't change the order.
    pub(super) fn arrange_shape(&mut self, i: usize, op: Arrange) {
        let len = self.doc.shapes.len();
        if i >= len || !arrange::changes_order(len, &[i], op) {
            return;
        }
        let perm = arrange::reorder(len, &[i], op);
        let inv = arrange::invert(&perm);
        self.checkpoint();
        let old = std::mem::take(&mut self.doc.shapes);
        let mut taken: Vec<Option<Shape>> = old.into_iter().map(Some).collect();
        let mut reordered = Vec::with_capacity(len);
        for &src in &perm {
            reordered.push(taken[src].take().expect("permutation visits each once"));
        }
        self.doc.shapes = reordered;
        // A shape at old index `j` is now at `inv[j]`; remap the live selection.
        for s in self.selection.iter_mut() {
            *s = inv[*s];
        }
        self.status = op.label().into();
    }

    /// Reorder the selected shapes in paint order (Arrange / stacking) as one
    /// undo step, remapping the selection through the same permutation so the
    /// same shapes stay selected. No-op (and no checkpoint) when the move would
    /// not change the order.
    pub(super) fn arrange_selection(&mut self, op: Arrange) {
        let len = self.doc.shapes.len();
        if self.selection.is_empty() || !arrange::changes_order(len, &self.selection, op) {
            return;
        }
        let perm = arrange::reorder(len, &self.selection, op);
        let inv = arrange::invert(&perm);
        self.checkpoint();
        // Rebuild the shape list in the new order (perm[new] = old).
        let old = std::mem::take(&mut self.doc.shapes);
        // `perm` is a permutation of 0..len, so reorder by lifting each slot.
        let mut taken: Vec<Option<Shape>> = old.into_iter().map(Some).collect();
        let mut reordered = Vec::with_capacity(len);
        for &src in &perm {
            reordered.push(taken[src].take().expect("permutation visits each once"));
        }
        self.doc.shapes = reordered;
        // Remap selection indices: a shape at old index `i` is now at `inv[i]`.
        for s in self.selection.iter_mut() {
            *s = inv[*s];
        }
        self.status = op.label().into();
    }

    pub(super) fn open_dialog(&mut self) {
        if let Some(path) = rfd::FileDialog::new()
            .add_filter("Contour document", &["contour", "json"])
            .pick_file()
        {
            match std::fs::read_to_string(&path) {
                Ok(json) => match serde_json::from_str::<crate::document::Document>(&json) {
                    Ok(mut doc) => {
                        // Repair a legacy / hand-edited file so the editor always
                        // has at least one artboard and a valid active index.
                        doc.normalize_artboards();
                        // Re-lay-out text from its params so glyphs match the
                        // current font build (the cache is advisory only).
                        doc.relayout_text();
                        self.doc = doc;
                        self.history = crate::history::History::default();
                        self.selection.clear();
                        log::info!("opened {}", path.display());
                    }
                    Err(e) => log::error!("parse failed: {e}"),
                },
                Err(e) => log::error!("open failed: {e}"),
            }
        }
    }

    pub(super) fn save_dialog(&self) {
        if let Some(path) = rfd::FileDialog::new()
            .add_filter("Contour document", &["contour", "json"])
            .set_file_name("untitled.contour")
            .save_file()
        {
            match serde_json::to_string_pretty(&self.doc) {
                Ok(json) => {
                    if let Err(e) = std::fs::write(&path, json) {
                        log::error!("save failed: {e}");
                    } else {
                        log::info!(
                            "saved {} shapes to {}",
                            self.doc.shapes.len(),
                            path.display()
                        );
                    }
                }
                Err(e) => log::error!("serialize failed: {e}"),
            }
        }
    }

    pub(super) fn commit_pen(&mut self, closed: bool) {
        if self.inter.pen_points.len() >= 2 {
            self.checkpoint();
            let mut handles = std::mem::take(&mut self.inter.pen_handles);
            let points = std::mem::take(&mut self.inter.pen_points);
            handles.resize(points.len(), (0.0, 0.0));
            self.doc.shapes.push(Shape::Path {
                points,
                closed,
                fill: self.fill,
                fill_gradient: self.fill_gradient.clone(),
                stroke: self.stroke,
                stroke_w: self.stroke_w,
                stroke_style: self.stroke_style.clone(),
                appearance: None,
                handles,
                live: None,
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
            });
            self.select_only(Some(self.doc.shapes.len() - 1));
        } else {
            self.inter.pen_points.clear();
            self.inter.pen_handles.clear();
        }
        self.inter.pen_drag = None;
    }

    // --- Type tool -----------------------------------------------------------

    /// Build a fresh point-type [`Shape::Text`] at `origin` with the given params,
    /// taking the app's current fill / stroke / style defaults. The glyph cache is
    /// laid out immediately so the object renders the moment it is placed.
    fn new_text_shape(&self, origin: (f32, f32), params: crate::text::TextParams) -> Shape {
        let glyphs = crate::text::layout(&params, origin).0;
        Shape::Text {
            params,
            origin,
            glyphs,
            fill: self.fill,
            fill_gradient: self.fill_gradient.clone(),
            // New type defaults to a fill with no stroke (Illustrator's default
            // type appearance), so glyph counters read cleanly.
            stroke: self.stroke,
            stroke_w: 0.0,
            stroke_style: self.stroke_style.clone(),
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

    /// Place a new point-type object at the clicked document point, select it, and
    /// begin editing its (initially placeholder) string. One undo step.
    pub(super) fn place_text(&mut self, x: f32, y: f32) {
        self.checkpoint();
        let params = crate::text::TextParams {
            text: String::new(),
            ..Default::default()
        };
        let shape = self.new_text_shape((x, y), params);
        self.doc.shapes.push(shape);
        let idx = self.doc.shapes.len() - 1;
        self.select_only(Some(idx));
        self.editing_text = Some(idx);
        self.status = "Type: start typing (Esc to finish)".into();
    }

    /// Begin editing the text object at `idx` (select it + make it the keyboard
    /// target). No-op if `idx` isn't a text object.
    pub(super) fn begin_text_edit(&mut self, idx: usize) {
        if matches!(self.doc.shapes.get(idx), Some(Shape::Text { .. })) {
            self.select_only(Some(idx));
            self.editing_text = Some(idx);
            self.status = "Type: editing (Esc to finish)".into();
        }
    }

    /// End the active text edit. An empty text object left behind (e.g. placed but
    /// never typed into) is deleted so the canvas isn't littered with invisible
    /// zero-glyph objects, matching Illustrator.
    pub(super) fn end_text_edit(&mut self) {
        let Some(idx) = self.editing_text.take() else {
            return;
        };
        let empty = matches!(
            self.doc.shapes.get(idx),
            Some(Shape::Text { params, .. }) if params.text.trim().is_empty()
        );
        if empty && idx < self.doc.shapes.len() {
            self.doc.shapes.remove(idx);
            self.selection.retain(|&i| i != idx);
            for s in self.selection.iter_mut() {
                if *s > idx {
                    *s -= 1;
                }
            }
        }
    }

    /// Apply this frame's keyboard input to the text object under edit: `Text`
    /// events append characters, Backspace removes the last char, Enter inserts a
    /// newline, Escape ends the edit. Re-lays-out the glyph cache after any change
    /// so the canvas updates live. Called once per frame while a Type edit is
    /// active.
    pub(super) fn handle_text_editing(&mut self, ctx: &egui::Context) {
        let Some(idx) = self.editing_text else {
            return;
        };
        if !matches!(self.doc.shapes.get(idx), Some(Shape::Text { .. })) {
            self.editing_text = None;
            return;
        }
        // Pull the relevant events for this frame.
        let (mut insert, mut backspace, mut newline, mut escape) = (String::new(), 0usize, 0usize, false);
        ctx.input(|i| {
            for ev in &i.events {
                match ev {
                    egui::Event::Text(t) => insert.push_str(t),
                    egui::Event::Key {
                        key: egui::Key::Backspace,
                        pressed: true,
                        ..
                    } => backspace += 1,
                    egui::Event::Key {
                        key: egui::Key::Enter,
                        pressed: true,
                        ..
                    } => newline += 1,
                    egui::Event::Key {
                        key: egui::Key::Escape,
                        pressed: true,
                        ..
                    } => escape = true,
                    _ => {}
                }
            }
        });

        if escape {
            self.end_text_edit();
            self.status = "Type: finished".into();
            return;
        }
        if insert.is_empty() && backspace == 0 && newline == 0 {
            return;
        }
        // Coalesce the whole typing run into one undo step (begin once, commit on
        // edit-end is implicit; here each frame's batch is a checkpoint so undo
        // steps back through the text in reasonable chunks).
        self.checkpoint();
        if let Some(Shape::Text { params, .. }) = self.doc.shapes.get(idx) {
            let mut text = params.text.clone();
            for _ in 0..backspace {
                text.pop();
            }
            text.push_str(&insert);
            for _ in 0..newline {
                text.push('\n');
            }
            let mut new = params.clone();
            new.text = text;
            if let Some(shape) = self.doc.shapes.get_mut(idx) {
                shape.set_text_params(new);
            }
        }
    }

    /// Replace every selected text object with its glyph outlines (Illustrator's
    /// `Type ▸ Create Outlines`): the live text becomes a real editable
    /// [`Shape::Compound`] of the glyph contours. One undo step; the produced
    /// compounds stay selected. No-op (returns whether anything converted) if the
    /// selection holds no text objects.
    pub(super) fn convert_text_to_outlines(&mut self) -> bool {
        let targets: Vec<usize> = self
            .selection
            .iter()
            .copied()
            .filter(|&i| matches!(self.doc.shapes.get(i), Some(Shape::Text { .. })))
            .collect();
        if targets.is_empty() {
            self.status = "Create Outlines: select a type object".into();
            return false;
        }
        self.end_text_edit();
        self.checkpoint();
        for &i in &targets {
            if let Some(shape) = self.doc.shapes.get(i) {
                self.doc.shapes[i] = shape.text_to_outlines();
            }
        }
        self.status = format!(
            "Converted {} type {} to outlines",
            targets.len(),
            if targets.len() == 1 { "object" } else { "objects" }
        );
        true
    }

    /// Whether the selection contains at least one editable text object (gates the
    /// `Type ▸ Create Outlines` menu item).
    pub(super) fn has_text_selected(&self) -> bool {
        self.selection
            .iter()
            .any(|&i| matches!(self.doc.shapes.get(i), Some(Shape::Text { .. })))
    }

    pub(super) fn export_svg_dialog(&mut self) {
        if let Some(path) = rfd::FileDialog::new()
            .add_filter("SVG image", &["svg"])
            .set_file_name("untitled.svg")
            .save_file()
        {
            let svg = export::to_svg_artboard(&self.doc, self.active_artboard_rect());
            match std::fs::write(&path, svg) {
                Ok(()) => self.status = format!("Exported SVG → {}", path.display()),
                Err(e) => {
                    log::error!("SVG export failed: {e}");
                    self.status = format!("SVG export failed: {e}");
                }
            }
        }
    }

    pub(super) fn export_png_dialog(&mut self) {
        if let Some(path) = rfd::FileDialog::new()
            .add_filter("PNG image", &["png"])
            .set_file_name("untitled.png")
            .save_file()
        {
            match export::to_png_artboard(&self.doc, self.active_artboard_rect()) {
                Some(bytes) => match std::fs::write(&path, bytes) {
                    Ok(()) => self.status = format!("Exported PNG → {}", path.display()),
                    Err(e) => {
                        log::error!("PNG export failed: {e}");
                        self.status = format!("PNG export failed: {e}");
                    }
                },
                None => {
                    log::error!("PNG rasterization failed");
                    self.status = "PNG rasterization failed".into();
                }
            }
        }
    }

    /// Apply a Pathfinder op to exactly two selected shapes (subject = back /
    /// first added, clip = front / second / primary), replacing both with the
    /// resulting batch of paths. Most ops yield one path, but those that produce
    /// disjoint regions or holes (Difference, Exclude, Divide, Trim, …) yield
    /// several — the single-ring model expands them into separate paths, the way
    /// Illustrator expands a Pathfinder result. The whole batch is selected.
    pub(super) fn apply_bool(&mut self, op: BoolOp) {
        if self.selection.len() != 2 {
            self.status = "Boolean op needs exactly two selected shapes".into();
            return;
        }
        let (a, b) = (self.selection[0], self.selection[1]);
        if a == b || a >= self.doc.shapes.len() || b >= self.doc.shapes.len() {
            return;
        }
        let subj = self.doc.shapes[a].clone();
        let clip = self.doc.shapes[b].clone();
        let results = boolean::apply(&subj, &clip, op, self.bool_fill_rule);
        if results.is_empty() {
            self.status = format!("{} produced no geometry", op.label());
            return;
        }
        self.checkpoint();
        // Remove the higher index first so the lower stays valid.
        let (hi, lo) = if a > b { (a, b) } else { (b, a) };
        self.doc.shapes.remove(hi);
        self.doc.shapes.remove(lo);
        let first = self.doc.shapes.len();
        let n = results.len();
        self.doc.shapes.extend(results);
        // Select every path the op produced.
        self.selection = (first..first + n).collect();
        self.status = format!(
            "{} applied ({n} path{})",
            op.label(),
            if n == 1 { "" } else { "s" }
        );
    }

    /// Reference rectangle the Align operations measure against.
    fn align_frame(
        &self,
        sel_boxes: &[document::ShapeBounds],
    ) -> Option<prism_core::geometry::Rect> {
        match self.align_to {
            AlignTo::Artboard => {
                let r = self.active_artboard_rect();
                Some(prism_core::geometry::Rect::new(r[0], r[1], r[2], r[3]))
            }
            AlignTo::Selection => {
                let boxes: Vec<_> = sel_boxes.iter().map(|sb| sb.rect).collect();
                align::union_bounds(&boxes)
            }
        }
    }

    /// Bounding box of each currently-selected shape, paired with its shape
    /// index, skipping shapes with no geometry. Order follows the selection set.
    fn selection_bounds(&self) -> Vec<document::ShapeBounds> {
        self.selection
            .iter()
            .filter_map(|&i| {
                self.doc
                    .shapes
                    .get(i)
                    .and_then(|s| s.bounds())
                    .map(|rect| document::ShapeBounds { index: i, rect })
            })
            .collect()
    }

    /// Align every selected shape to the current reference frame as one undo
    /// step. Needs ≥2 selected shapes (or ≥1 when aligning to the artboard).
    pub(super) fn align_selection(&mut self, op: Align) {
        let sel = self.selection_bounds();
        let min = if self.align_to == AlignTo::Artboard {
            1
        } else {
            2
        };
        if sel.len() < min {
            self.status = "Select shapes to align".into();
            return;
        }
        let Some(frame) = self.align_frame(&sel) else {
            return;
        };
        let boxes: Vec<_> = sel.iter().map(|sb| sb.rect).collect();
        let deltas = align::align_deltas(&boxes, op, frame);
        self.checkpoint();
        for (sb, (dx, dy)) in sel.iter().zip(deltas) {
            if (dx != 0.0 || dy != 0.0) && sb.index < self.doc.shapes.len() {
                self.doc.shapes[sb.index].translate(dx, dy);
            }
        }
        self.status = op.label().into();
    }

    // --- Transform -----------------------------------------------------------

    /// The selection's combined (axis-aligned) bounding box `[x, y, w, h]` in
    /// document space, or `None` when nothing with geometry is selected. This is
    /// the rectangle the transform box draws around and scales from.
    pub(super) fn selection_bbox(&self) -> Option<[f32; 4]> {
        let boxes: Vec<_> = self.selection_bounds().iter().map(|sb| sb.rect).collect();
        align::union_bounds(&boxes).map(|r| [r.x, r.y, r.w, r.h])
    }

    /// Apply an affine to every selected shape as one undo step, dropping no-op
    /// (identity) transforms.
    /// Apply `m` to every selected shape as one undo step. Returns `true` when it
    /// did something (a non-identity matrix and a non-empty selection), `false`
    /// on a no-op so callers can skip recording it for Transform Again.
    fn transform_selection(&mut self, m: &Affine, label: &str) -> bool {
        if m.is_identity() || self.selection.is_empty() {
            return false;
        }
        self.checkpoint();
        let n = self.doc.shapes.len();
        let indices: Vec<usize> = self.selection.clone();
        for i in indices {
            if i < n {
                self.doc.shapes[i].apply_affine(m);
            }
        }
        self.status = label.into();
        true
    }

    /// The selection's bounding-box centre in document space, or `None` when
    /// nothing with geometry is selected. This is the pivot every centred
    /// transform (rotate / reflect / scale / shear) turns about.
    pub(super) fn selection_center(&self) -> Option<(f32, f32)> {
        self.selection_bbox()
            .map(|b| (b[0] + b[2] * 0.5, b[1] + b[3] * 0.5))
    }

    /// Rotate the selection about its bounding-box centre by `radians`
    /// (positive = clockwise) as one undo step.
    pub(super) fn rotate_selection(&mut self, radians: f32, label: &str) {
        let Some((cx, cy)) = self.selection_center() else {
            return;
        };
        if self.transform_selection(&Affine::rotate_about(radians, cx, cy), label) {
            self.last_transform = Some(LastTransform::Rotate(radians));
        }
    }

    /// Mirror the selection across its bounding-box centre, horizontally
    /// (`horizontal = true`) or vertically, as one undo step.
    pub(super) fn flip_selection(&mut self, horizontal: bool) {
        // A horizontal flip mirrors across the vertical centre line (reflect at
        // π/2); a vertical flip across the horizontal centre line (reflect at 0).
        let angle = if horizontal {
            std::f32::consts::FRAC_PI_2
        } else {
            0.0
        };
        self.reflect_selection(
            angle,
            if horizontal {
                "Flipped horizontal"
            } else {
                "Flipped vertical"
            },
        );
    }

    /// Reflect the selection across the line through its centre at `radians` from
    /// the +x axis, as one undo step. The general Reflect tool; `flip_selection`
    /// is the axis-aligned shorthand.
    pub(super) fn reflect_selection(&mut self, radians: f32, label: &str) {
        let Some((cx, cy)) = self.selection_center() else {
            return;
        };
        if self.transform_selection(&Affine::reflect_about(radians, cx, cy), label) {
            self.last_transform = Some(LastTransform::Reflect(radians));
        }
    }

    /// Apply a full numeric transform (move / scale / rotate / shear) about the
    /// selection centre in one undo step, recording it for Transform Again.
    pub(super) fn apply_numeric_transform(&mut self, nt: NumericTransform) {
        let Some((cx, cy)) = self.selection_center() else {
            return;
        };
        let m = nt.to_affine(cx, cy);
        if self.transform_selection(&m, "Transformed") {
            // Repeat re-derives about the new centre; a move repeats verbatim. A
            // pure-move numeric reduces to a Move so Transform Again nudges again.
            self.last_transform = Some(
                if nt.scale_x == 1.0
                    && nt.scale_y == 1.0
                    && nt.rotate == 0.0
                    && nt.shear_x == 0.0
                    && nt.shear_y == 0.0
                {
                    LastTransform::Move(nt.move_x, nt.move_y)
                } else {
                    LastTransform::Numeric(nt)
                },
            );
        }
    }

    /// Repeat the most recent transform on the current selection about its centre
    /// (Illustrator's Transform Again, Cmd/Ctrl+D). No-op when nothing has been
    /// transformed yet or the selection is empty.
    pub(super) fn transform_again(&mut self) {
        let Some(last) = self.last_transform else {
            self.status = "Nothing to transform again".into();
            return;
        };
        let Some((cx, cy)) = self.selection_center() else {
            return;
        };
        let (m, label) = match last {
            LastTransform::Move(dx, dy) => (Affine::translate(dx, dy), "Moved again"),
            LastTransform::Scale(sx, sy) => (Affine::scale_about(sx, sy, cx, cy), "Scaled again"),
            LastTransform::Rotate(r) => (Affine::rotate_about(r, cx, cy), "Rotated again"),
            LastTransform::Shear(shx, shy) => {
                (Affine::shear_about(shx, shy, cx, cy), "Sheared again")
            }
            LastTransform::Reflect(r) => (Affine::reflect_about(r, cx, cy), "Reflected again"),
            LastTransform::Numeric(nt) => (nt.to_affine(cx, cy), "Transformed again"),
        };
        // Re-apply without overwriting `last_transform` (the recipe is unchanged).
        self.transform_selection(&m, label);
    }

    /// Begin a free-transform: snapshot the selected shapes and the pivot.
    pub(super) fn begin_transform(&mut self, kind: TransformKind, x: f32, y: f32) {
        let Some(bbox) = self.selection_bbox() else {
            return;
        };
        let pivot = match kind {
            TransformKind::Scale(h) => {
                let opp = h.opposite();
                (
                    bbox[0] + bbox[2] * opp.unit_pos().0,
                    bbox[1] + bbox[3] * opp.unit_pos().1,
                )
            }
            TransformKind::Rotate => (bbox[0] + bbox[2] * 0.5, bbox[1] + bbox[3] * 0.5),
            TransformKind::Shear(h) => transform::shear_pivot(h, &bbox)
                .unwrap_or((bbox[0] + bbox[2] * 0.5, bbox[1] + bbox[3] * 0.5)),
        };
        let snapshot: Vec<(usize, Shape)> = self
            .selection
            .iter()
            .filter_map(|&i| self.doc.shapes.get(i).map(|s| (i, s.clone())))
            .collect();
        self.begin_interaction();
        self.inter.transform = Some(TransformDrag {
            kind,
            pivot,
            box_wh: (bbox[2], bbox[3]),
            start: (x, y),
            snapshot,
        });
    }

    /// Drive an active free-transform from the current cursor `(x, y)`. Rebuilds
    /// the selection from the start snapshot every frame and applies the affine
    /// derived from the gesture, so dragging is exact and reversible in one undo.
    pub(super) fn drag_transform(&mut self, x: f32, y: f32, uniform: bool) {
        let Some(td) = &self.inter.transform else {
            return;
        };
        // Each gesture yields both the matrix to apply this frame and the
        // repeatable [`LastTransform`] recipe (so Transform Again, Cmd/Ctrl+D,
        // replays the same gesture about the current selection centre).
        let (m, last) = match td.kind {
            TransformKind::Scale(h) => {
                let (px, py) = td.pivot;
                let orig_dx = td.start.0 - px;
                let orig_dy = td.start.1 - py;
                let cur_dx = x - px;
                let cur_dy = y - py;
                let (sx, sy) = transform::scale_factors_for_handle(
                    h, orig_dx, orig_dy, cur_dx, cur_dy, uniform,
                );
                (
                    Affine::scale_about(sx, sy, px, py),
                    LastTransform::Scale(sx, sy),
                )
            }
            TransformKind::Rotate => {
                let ang = transform::angle_between(td.start, (x, y), td.pivot);
                (
                    Affine::rotate_about(ang, td.pivot.0, td.pivot.1),
                    LastTransform::Rotate(ang),
                )
            }
            TransformKind::Shear(h) => {
                let dx = x - td.start.0;
                let dy = y - td.start.1;
                let (shx, shy) =
                    transform::shear_factors_for_handle(h, td.box_wh.0, td.box_wh.1, dx, dy);
                (
                    Affine::shear_about(shx, shy, td.pivot.0, td.pivot.1),
                    LastTransform::Shear(shx, shy),
                )
            }
        };

        // Reset each selected shape to its pristine snapshot, then transform.
        let snapshot = td.snapshot.clone();
        for (i, shape) in &snapshot {
            if let Some(slot) = self.doc.shapes.get_mut(*i) {
                *slot = shape.clone();
                slot.apply_affine(&m);
            }
        }
        self.last_transform = Some(last);
    }

    // --- Artboards -----------------------------------------------------------

    /// Commit a dragged-out new artboard between document-space corners
    /// `start`..`end`. Tiny drags (a click) are ignored. The new board becomes
    /// active. Called inside an open interaction (one undo step).
    pub(super) fn finish_artboard_create(&mut self, start: (f32, f32), end: (f32, f32)) {
        let x = start.0.min(end.0);
        let y = start.1.min(end.1);
        let w = (end.0 - start.0).abs();
        let h = (end.1 - start.1).abs();
        if w < 4.0 && h < 4.0 {
            return;
        }
        let name = crate::artboard::default_name(self.doc.artboards.len());
        self.doc
            .artboards
            .push(crate::artboard::Artboard::new(name, [x, y, w, h]));
        self.doc.active_artboard = self.doc.artboards.len() - 1;
        self.status = "Added artboard".into();
    }

    /// Add a new artboard the size of the active one (falling back to the
    /// document default), tiled to the right of the rightmost board, à la
    /// Illustrator's "New Artboard". One undo step; the new board becomes active.
    pub(super) fn add_artboard(&mut self) {
        let template = self
            .doc
            .active_artboard()
            .map(|a| [a.rect[2], a.rect[3]])
            .unwrap_or(crate::document::DEFAULT_ARTBOARD);
        let rect = crate::artboard::next_placement(&self.doc.artboards, template, 40.0);
        self.checkpoint();
        let name = crate::artboard::default_name(self.doc.artboards.len());
        self.doc
            .artboards
            .push(crate::artboard::Artboard::new(name, rect));
        self.doc.active_artboard = self.doc.artboards.len() - 1;
        self.status = "Added artboard".into();
    }

    /// Delete artboard `i`, keeping at least one board. The active index is
    /// re-clamped. One undo step.
    pub(super) fn delete_artboard(&mut self, i: usize) {
        if self.doc.artboards.len() <= 1 || i >= self.doc.artboards.len() {
            return;
        }
        self.checkpoint();
        self.doc.artboards.remove(i);
        if self.doc.active_artboard >= self.doc.artboards.len() {
            self.doc.active_artboard = self.doc.artboards.len() - 1;
        } else if self.doc.active_artboard > i {
            self.doc.active_artboard -= 1;
        }
        self.status = "Deleted artboard".into();
    }

    /// Make artboard `i` the active one (no undo entry — view state).
    pub(super) fn set_active_artboard(&mut self, i: usize) {
        if i < self.doc.artboards.len() {
            self.doc.active_artboard = i;
        }
    }

    /// Zoom + pan the view so the union of all artboards fits within the screen
    /// rectangle `content`, with a small margin and centred — Illustrator's
    /// "Fit All in Window". View-only (no undo entry).
    pub(super) fn fit_artboards_to(&mut self, content: egui::Rect) {
        let Some(u) = crate::artboard::union_rect(&self.doc.artboards) else {
            return;
        };
        let (uw, uh) = (u[2].max(1.0), u[3].max(1.0));
        let margin = 40.0;
        let avail_w = (content.width() - margin * 2.0).max(1.0);
        let avail_h = (content.height() - margin * 2.0).max(1.0);
        let zoom = (avail_w / uw).min(avail_h / uh).clamp(0.05, 64.0);
        self.view.zoom = zoom;
        // Centre the union in the content rectangle.
        let (ucx, ucy) = (u[0] + uw * 0.5, u[1] + uh * 0.5);
        self.view.pan.x = content.center().x - ucx * zoom;
        self.view.pan.y = content.center().y - ucy * zoom;
        self.status = "Fit artboards".into();
    }

    /// The active artboard's rectangle `[x, y, w, h]`, falling back to a
    /// default-sized board at the origin for a (malformed) empty stack.
    pub(super) fn active_artboard_rect(&self) -> [f32; 4] {
        self.doc.active_artboard().map(|a| a.rect).unwrap_or([
            0.0,
            0.0,
            crate::document::DEFAULT_ARTBOARD[0],
            crate::document::DEFAULT_ARTBOARD[1],
        ])
    }

    // --- Swatches ------------------------------------------------------------

    /// Apply swatch `id`'s colour to the **fill** of the current selection (one
    /// undo step) and load it as the app's default fill so the next new shape
    /// inherits it. Clears any gradient fill on the selected shapes (a solid
    /// swatch replaces a gradient, à la Illustrator). With no selection only the
    /// default updates.
    pub(super) fn apply_swatch_fill(&mut self, id: u64) {
        let Some(color) = self.doc.swatches.get(id).map(|s| s.color) else {
            return;
        };
        self.fill = color;
        self.fill_gradient = None;
        if self.selection.is_empty() {
            self.status = "Set fill swatch".into();
            return;
        }
        self.checkpoint();
        let indices: Vec<usize> = self.selection.clone();
        for i in indices {
            if let Some(s) = self.doc.shapes.get_mut(i) {
                s.set_fill_color(color);
                s.set_fill_gradient(None);
            }
        }
        self.status = "Applied fill swatch".into();
    }

    /// Apply swatch `id`'s colour to the **stroke** of the current selection (one
    /// undo step) and load it as the app's default stroke. With no selection only
    /// the default updates.
    pub(super) fn apply_swatch_stroke(&mut self, id: u64) {
        let Some(color) = self.doc.swatches.get(id).map(|s| s.color) else {
            return;
        };
        self.stroke = color;
        if self.selection.is_empty() {
            self.status = "Set stroke swatch".into();
            return;
        }
        self.checkpoint();
        let indices: Vec<usize> = self.selection.clone();
        for i in indices {
            if let Some(s) = self.doc.shapes.get_mut(i) {
                s.set_stroke_color(color);
            }
        }
        self.status = "Applied stroke swatch".into();
    }

    /// Add a new swatch for the current fill colour (the primary selection's
    /// solid fill, else the app default fill). Equal colours de-dupe, so this is
    /// idempotent. One undo step.
    pub(super) fn add_fill_swatch(&mut self) {
        let color = self
            .primary()
            .and_then(|i| self.doc.shapes.get(i))
            .and_then(|s| s.fill_color())
            .unwrap_or(self.fill);
        self.checkpoint();
        self.doc.swatches.add("Swatch", color);
        self.status = "Added swatch".into();
    }

    /// Rename swatch `id` (one undo step). No-op if the name is unchanged.
    pub(super) fn rename_swatch(&mut self, id: u64, name: &str) {
        if self.doc.swatches.get(id).map(|s| s.name.as_str()) == Some(name) {
            return;
        }
        self.checkpoint();
        self.doc.swatches.rename(id, name);
    }

    /// Toggle swatch `id`'s global flag (one undo step).
    pub(super) fn set_swatch_global(&mut self, id: u64, global: bool) {
        self.checkpoint();
        self.doc.swatches.set_global(id, global);
    }

    /// Recolour swatch `id` (one undo step). When the swatch is **global**, the
    /// edit propagates: every shape painted with the swatch's old colour is
    /// remapped to the new colour.
    pub(super) fn recolor_swatch(&mut self, id: u64, color: [f32; 4]) {
        self.checkpoint();
        if let Some((old, new)) = self.doc.swatches.recolor(id, color) {
            let n = self.doc.remap_color(old, new);
            self.status = if n > 0 {
                format!("Recoloured swatch · {n} updated")
            } else {
                "Recoloured swatch".into()
            };
        } else {
            self.status = "Recoloured swatch".into();
        }
    }

    /// Delete swatch `id` (one undo step). The artwork is untouched — a swatch is
    /// only a named shortcut, so removing it leaves shapes' colours intact.
    pub(super) fn delete_swatch(&mut self, id: u64) {
        self.checkpoint();
        if self.doc.swatches.remove(id) {
            self.status = "Deleted swatch".into();
        }
    }

    // --- Graphic styles ------------------------------------------------------

    /// Save the primary selection's effective appearance as a new named graphic
    /// style (one undo step). No-op with no selection. Returns the new style's id
    /// so the panel can select it for renaming. The captured appearance is the
    /// shape's explicit stack, or one migrated from its legacy fields, so a style
    /// can be saved off any shape — stacked or not.
    pub(super) fn save_graphic_style(&mut self) -> Option<u64> {
        let i = self.primary()?;
        let ap = self.doc.shapes.get(i)?.effective_appearance();
        self.checkpoint();
        let id = self.doc.graphic_styles.add("Style", ap);
        self.status = "Saved graphic style".into();
        Some(id)
    }

    /// Apply graphic style `id` to every selected shape, overwriting each one's
    /// appearance with the style's snapshot (one undo step). Routes through the
    /// same `set_appearance` path the Appearance panel uses, so it undoes as a
    /// single labelled step. No-op with no selection or an unknown id.
    pub(super) fn apply_graphic_style(&mut self, id: u64) {
        let Some(ap) = self.doc.graphic_styles.appearance_of(id).cloned() else {
            return;
        };
        if self.selection.is_empty() {
            self.status = "Select a shape to apply a style".into();
            return;
        }
        self.checkpoint();
        let indices: Vec<usize> = self.selection.clone();
        for i in indices {
            if let Some(s) = self.doc.shapes.get_mut(i) {
                s.set_appearance(Some(ap.clone()));
            }
        }
        self.status = "Apply Graphic Style".into();
    }

    /// Rename graphic style `id` (one undo step). No-op if the name is unchanged.
    pub(super) fn rename_graphic_style(&mut self, id: u64, name: &str) {
        if self.doc.graphic_styles.get(id).map(|s| s.name.as_str()) == Some(name) {
            return;
        }
        self.checkpoint();
        self.doc.graphic_styles.rename(id, name);
    }

    /// Delete graphic style `id` (one undo step). The artwork is untouched — a
    /// style is only a named shortcut, so removing it leaves shapes' appearances
    /// intact.
    pub(super) fn delete_graphic_style(&mut self, id: u64) {
        self.checkpoint();
        if self.doc.graphic_styles.remove(id) {
            self.status = "Deleted graphic style".into();
        }
    }

    // --- Snapping ------------------------------------------------------------

    /// Document-space snap tolerance: a fixed ~6px pulled into document units so
    /// the snap feels identical at every zoom level.
    pub(super) fn snap_tol(&self) -> f32 {
        6.0 / self.view.zoom
    }

    /// Gather the candidate snap-target coordinates from the active sources,
    /// excluding the shapes in `exclude` (the ones being dragged, so a shape
    /// never snaps to itself). Grid lines are added per-feature by the caller via
    /// [`snap::grid_targets_near`], so only guides and objects are collected here.
    pub(super) fn snap_targets(&self, exclude: &[usize]) -> SnapTargets {
        let mut t = SnapTargets::default();
        if self.snap.to_guides {
            for g in &self.doc.guides {
                match *g {
                    Guide::Vertical(x) => t.xs.push(x),
                    Guide::Horizontal(y) => t.ys.push(y),
                }
            }
        }
        if self.snap.to_objects {
            for (i, s) in self.doc.shapes.iter().enumerate() {
                if exclude.contains(&i) || !s.visible() {
                    continue;
                }
                if let Some(b) = s.bounds() {
                    // Edges + centre of each other object are snap candidates.
                    t.xs.push(b.x);
                    t.xs.push(b.x + b.w * 0.5);
                    t.xs.push(b.x + b.w);
                    t.ys.push(b.y);
                    t.ys.push(b.y + b.h * 0.5);
                    t.ys.push(b.y + b.h);
                }
            }
        }
        t
    }

    /// Compute the snap adjustment for moving the box `bbox` (already offset by
    /// the raw drag) given the dragged shape indices `exclude`. Folds grid lines
    /// in per box-feature. Returns a [`SnapResult`] with the delta + fired lines.
    pub(super) fn snap_box(&self, bbox: &[f32; 4], exclude: &[usize]) -> SnapResult {
        if !self.snap.any() {
            return SnapResult::default();
        }
        let tol = self.snap_tol();
        let features = SnapFeatures::bbox(bbox);
        let mut targets = self.snap_targets(exclude);
        if self.snap.to_grid {
            for &fx in &features.xs {
                targets
                    .xs
                    .extend(snap::grid_targets_near(fx, self.snap.grid));
            }
            for &fy in &features.ys {
                targets
                    .ys
                    .extend(snap::grid_targets_near(fy, self.snap.grid));
            }
        }
        if targets.is_empty() {
            return SnapResult::default();
        }
        snap::snap_delta(&features, &targets, tol)
    }

    /// Snap a single point (e.g. a fresh anchor or a create-drag corner) to the
    /// active sources, returning the adjusted point.
    pub(super) fn snap_point(&self, x: f32, y: f32, exclude: &[usize]) -> (f32, f32) {
        if !self.snap.any() {
            return (x, y);
        }
        let tol = self.snap_tol();
        let features = SnapFeatures::point(x, y);
        let mut targets = self.snap_targets(exclude);
        if self.snap.to_grid {
            targets
                .xs
                .extend(snap::grid_targets_near(x, self.snap.grid));
            targets
                .ys
                .extend(snap::grid_targets_near(y, self.snap.grid));
        }
        if targets.is_empty() {
            return (x, y);
        }
        let r = snap::snap_delta(&features, &targets, tol);
        (x + r.dx, y + r.dy)
    }

    /// Distribute the selected shapes evenly along the operation's axis as one
    /// undo step. Needs ≥3 selected shapes.
    pub(super) fn distribute_selection(&mut self, op: Distribute) {
        let sel = self.selection_bounds();
        if sel.len() < 3 {
            self.status = "Distribute needs three or more shapes".into();
            return;
        }
        let boxes: Vec<_> = sel.iter().map(|sb| sb.rect).collect();
        let deltas = align::distribute_deltas(&boxes, op);
        self.checkpoint();
        for (sb, (dx, dy)) in sel.iter().zip(deltas) {
            if (dx != 0.0 || dy != 0.0) && sb.index < self.doc.shapes.len() {
                self.doc.shapes[sb.index].translate(dx, dy);
            }
        }
        self.status = op.label().into();
    }
}

/// The closed sub-contours a shape contributes to a compound path, or `None` for
/// a shape with no closed fillable region (an open path / line). A `Rect` /
/// `Ellipse` / closed `Path` contributes one sub-contour; an existing `Compound`
/// contributes all of its sub-contours (so combining a compound flattens it).
fn compound_subpaths(shape: &Shape) -> Option<Vec<crate::document::SubPath>> {
    match shape {
        Shape::Compound { subpaths, .. } => (!subpaths.is_empty()).then(|| subpaths.clone()),
        // A text object contributes its glyph outlines (it converts to a compound
        // path), so it can be combined with other shapes.
        Shape::Text { glyphs, .. } => (!glyphs.is_empty()).then(|| glyphs.clone()),
        Shape::Path {
            points,
            handles,
            closed,
            ..
        } => {
            if !*closed || points.len() < 3 {
                return None;
            }
            let mut h = handles.clone();
            h.resize(points.len(), (0.0, 0.0));
            Some(vec![crate::document::SubPath {
                points: points.clone(),
                handles: h,
                closed: true,
            }])
        }
        Shape::Rect { .. } | Shape::Ellipse { .. } => {
            // Convert the primitive to a path and take its (single closed) ring.
            if let Shape::Path {
                points,
                handles,
                closed,
                ..
            } = shape.to_path()
            {
                (closed && points.len() >= 3).then(|| {
                    vec![crate::document::SubPath {
                        points,
                        handles,
                        closed: true,
                    }]
                })
            } else {
                None
            }
        }
        Shape::Line { .. } => None,
    }
}

/// Split a compound path into one closed [`Shape::Path`] per sub-contour, each
/// inheriting the compound's paint (fill / gradient / stroke / appearance / tags).
fn release_compound_to_paths(compound: &Shape) -> Vec<Shape> {
    let Shape::Compound {
        subpaths,
        fill,
        fill_gradient,
        stroke,
        stroke_w,
        stroke_style,
        appearance,
        visible,
        group,
        clip,
        mask,
        omask,
        omask_path,
        omask_invert,
        blend,
        blend_step,
        name,
        locked,
        layer_color,
        ..
    } = compound
    else {
        return vec![compound.clone()];
    };
    subpaths
        .iter()
        .filter(|sp| sp.points.len() >= 2)
        .map(|sp| {
            let mut h = sp.handles.clone();
            h.resize(sp.points.len(), (0.0, 0.0));
            Shape::Path {
                points: sp.points.clone(),
                closed: sp.closed,
                fill: *fill,
                fill_gradient: fill_gradient.clone(),
                stroke: *stroke,
                stroke_w: *stroke_w,
                stroke_style: stroke_style.clone(),
                appearance: appearance.clone(),
                handles: h,
                live: None,
                visible: *visible,
                group: *group,
                clip: *clip,
                mask: *mask,
                omask: *omask,
                omask_path: *omask_path,
                omask_invert: *omask_invert,
                blend: *blend,
                blend_step: *blend_step,
                name: name.clone(),
                locked: *locked,
                layer_color: *layer_color,
            }
        })
        .collect()
}
