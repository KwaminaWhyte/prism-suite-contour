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

    /// Shift-click toggle that treats a group atomically: if shape `i` belongs to
    /// a group, the whole group is added (when any member is currently absent) or
    /// removed (when all members are present); an ungrouped shape toggles alone.
    pub(super) fn toggle_group_selection(&mut self, i: usize) {
        let tags = self.group_tags();
        let members = group::members_of(&tags, i);
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

    /// Expand the current selection so that selecting any member of a group
    /// selects the whole group. Keeps the original primary (last-added) shape as
    /// primary when it survives expansion; otherwise the topmost expanded shape
    /// becomes primary. Pure bookkeeping — records no undo entry.
    pub(super) fn expand_selection_to_groups(&mut self) {
        let tags = self.group_tags();
        let expanded = group::expand_selection(&tags, &self.selection);
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
                Ok(json) => match serde_json::from_str(&json) {
                    Ok(doc) => {
                        self.doc = doc;
                        self.history = crate::history::History::default();
                        self.selection.clear();
                        log::info!("opened {} from {}", path.display(), path.display());
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
            let svg = export::to_svg(
                &self.doc,
                self.artboard.width as f32,
                self.artboard.height as f32,
            );
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
            match export::to_png(
                &self.doc,
                self.artboard.width as f32,
                self.artboard.height as f32,
            ) {
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
            AlignTo::Artboard => Some(prism_core::geometry::Rect::new(
                0.0,
                0.0,
                self.artboard.width as f32,
                self.artboard.height as f32,
            )),
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
