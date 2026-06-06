//! Document-mutating operations behind the menus, inspector, and keyboard:
//! selection bookkeeping, undo/redo checkpoints, delete/swap/arrange, file I/O
//! and export dialogs, boolean ops, and the align / distribute / transform /
//! snap math.

use super::{ContourApp, TransformDrag, TransformKind};
use crate::align::{self, Align, AlignTo, Distribute};
use crate::arrange::{self, Arrange};
use crate::boolean::{self, BoolOp};
use crate::document::{self, Guide, Shape};
use crate::export;
use crate::group;
use crate::snap::{self, SnapFeatures, SnapResult, SnapTargets};
use crate::transform::{self, Affine};

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

    /// Swap shapes `a` and `b`, keeping the selection pinned to the moved shapes.
    pub(super) fn swap_shapes(&mut self, a: usize, b: usize) {
        let n = self.doc.shapes.len();
        if a >= n || b >= n || a == b {
            return;
        }
        self.doc.shapes.swap(a, b);
        for s in self.selection.iter_mut() {
            if *s == a {
                *s = b;
            } else if *s == b {
                *s = a;
            }
        }
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
                handles,
                visible: true,
                group: None,
                clip: None,
                mask: false,
            });
            self.select_only(Some(self.doc.shapes.len() - 1));
        } else {
            self.inter.pen_points.clear();
            self.inter.pen_handles.clear();
        }
        self.inter.pen_drag = None;
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

    /// Apply a boolean op to exactly two selected shapes (subject = first added,
    /// clip = second/primary), replacing both with the single result path.
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
        match boolean::apply(&subj, &clip, op) {
            Some(result) => {
                self.checkpoint();
                // Remove the higher index first so the lower stays valid.
                let (hi, lo) = if a > b { (a, b) } else { (b, a) };
                self.doc.shapes.remove(hi);
                self.doc.shapes.remove(lo);
                self.doc.shapes.push(result);
                self.select_only(Some(self.doc.shapes.len() - 1));
                self.status = "Boolean op applied".into();
            }
            None => self.status = "Boolean op produced no geometry".into(),
        }
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
        self.status = "Aligned".into();
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
    fn transform_selection(&mut self, m: &Affine, label: &str) {
        if m.is_identity() || self.selection.is_empty() {
            return;
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
    }

    /// Rotate the selection about its bounding-box centre by `radians`
    /// (positive = clockwise) as one undo step.
    pub(super) fn rotate_selection(&mut self, radians: f32, label: &str) {
        let Some(b) = self.selection_bbox() else {
            return;
        };
        let (cx, cy) = (b[0] + b[2] * 0.5, b[1] + b[3] * 0.5);
        self.transform_selection(&Affine::rotate_about(radians, cx, cy), label);
    }

    /// Mirror the selection across its bounding-box centre, horizontally
    /// (`horizontal = true`) or vertically, as one undo step.
    pub(super) fn flip_selection(&mut self, horizontal: bool) {
        let Some(b) = self.selection_bbox() else {
            return;
        };
        let m = if horizontal {
            Affine::flip_h_about(b[0] + b[2] * 0.5)
        } else {
            Affine::flip_v_about(b[1] + b[3] * 0.5)
        };
        self.transform_selection(
            &m,
            if horizontal {
                "Flipped horizontal"
            } else {
                "Flipped vertical"
            },
        );
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
        let m = match td.kind {
            TransformKind::Scale(h) => {
                let (px, py) = td.pivot;
                let orig_dx = td.start.0 - px;
                let orig_dy = td.start.1 - py;
                let cur_dx = x - px;
                let cur_dy = y - py;
                let (sx, sy) = transform::scale_factors_for_handle(
                    h, orig_dx, orig_dy, cur_dx, cur_dy, uniform,
                );
                Affine::scale_about(sx, sy, px, py)
            }
            TransformKind::Rotate => {
                let ang = transform::angle_between(td.start, (x, y), td.pivot);
                Affine::rotate_about(ang, td.pivot.0, td.pivot.1)
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
        self.status = "Distributed".into();
    }
}
