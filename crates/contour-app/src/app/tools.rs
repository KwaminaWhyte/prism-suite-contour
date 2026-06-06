//! The central canvas: per-frame painting, ruler-guide dragging, and the
//! tool-input state machine (select / create / pen), plus the transform-box
//! hit-tests and direct-select anchor editing that drive it.

use super::{ContourApp, GuideDrag, PathEdit, PenDrag, Tool, TransformKind};
use crate::canvas;
use crate::document::{self, Guide, Shape};
use crate::snap::{self, SnapFeatures, SnapResult};
use crate::transform::Handle;

impl ContourApp {
    /// Whether the on-canvas transform box should be shown: the Select tool, a
    /// non-empty selection with geometry, and no active per-path anchor edit
    /// (which has its own handles). The box is suppressed while *directly*
    /// editing a single path's anchors so the two handle sets don't clash.
    pub(super) fn show_transform_box(&self) -> bool {
        if self.tool != Tool::Select || self.inter.path_edit.is_some() {
            return false;
        }
        // For a single selected path the anchor handles are the primary editing
        // affordance; still show the box so the user can scale/rotate it.
        self.selection_bbox().is_some()
    }

    /// Hit-test the transform-box handles at document point `(x, y)`. Returns the
    /// gesture to begin: a corner/edge scale, or a rotate when the cursor is in
    /// the rotate ring just outside a corner. `None` if not on any handle.
    pub(super) fn hit_transform_handle(&self, x: f32, y: f32) -> Option<TransformKind> {
        if !self.show_transform_box() {
            return None;
        }
        let bbox = self.selection_bbox()?;
        let cursor = self.view.doc_to_screen((x, y));

        // Scale handles take priority (inner pick radius).
        for h in Handle::ALL {
            let hp = self.view.doc_to_screen((
                bbox[0] + bbox[2] * h.unit_pos().0,
                bbox[1] + bbox[3] * h.unit_pos().1,
            ));
            if (cursor - hp).length() <= canvas::HANDLE_PICK_PX {
                return Some(TransformKind::Scale(h));
            }
        }

        // Rotate ring: just outside a corner (within ROTATE_PICK_PX, but past the
        // handle pick radius). Checking corners only matches Illustrator.
        for h in [
            Handle::TopLeft,
            Handle::TopRight,
            Handle::BottomRight,
            Handle::BottomLeft,
        ] {
            let hp = self.view.doc_to_screen((
                bbox[0] + bbox[2] * h.unit_pos().0,
                bbox[1] + bbox[3] * h.unit_pos().1,
            ));
            let d = (cursor - hp).length();
            if d > canvas::HANDLE_PICK_PX && d <= canvas::ROTATE_PICK_PX {
                return Some(TransformKind::Rotate);
            }
        }
        None
    }

    pub(super) fn central_canvas(&mut self, root: &mut egui::Ui) {
        egui::CentralPanel::default().show_inside(root, |ui| {
            let ctx = ui.ctx().clone();
            let (response, painter) =
                ui.allocate_painter(ui.available_size(), egui::Sense::click_and_drag());

            canvas::handle_zoom(&mut self.view, &response, &ctx);

            let full = response.rect;
            let cursor = response.hover_pos();

            // The content rectangle excludes the ruler strips when rulers show.
            let content = if self.show_rulers {
                egui::Rect::from_min_max(
                    egui::Pos2::new(
                        full.left() + canvas::RULER_PX,
                        full.top() + canvas::RULER_PX,
                    ),
                    full.right_bottom(),
                )
            } else {
                full
            };

            // A pending "Fit artboards" zooms/pans so every board fits `content`.
            if self.fit_artboards_requested {
                self.fit_artboards_requested = false;
                self.fit_artboards_to(content);
            }

            // Grid behind the artboard.
            if self.show_grid {
                canvas::paint_grid(&painter, &self.view, content, self.snap.grid);
            }

            // Artboards (under the shapes) + all shapes (bottom-up paint order).
            let active_ab = self
                .doc
                .active_artboard
                .min(self.doc.artboards.len().saturating_sub(1).min(usize::MAX));
            for (i, ab) in self.doc.artboards.iter().enumerate() {
                canvas::paint_artboard(&painter, &self.view, &ab.rect, &ab.name, i == active_ab);
            }
            // Live preview of an artboard being dragged out / moved.
            if let Some(prev) = self.artboard_preview() {
                canvas::paint_artboard(&painter, &self.view, &prev, "", true);
            }
            for (i, s) in self.doc.shapes.iter().enumerate() {
                if !s.visible() {
                    continue;
                }
                canvas::paint_shape(&painter, &self.view, s, self.is_selected(i));
            }

            // Ruler guides (under handles/overlays, over shapes).
            if self.show_guides {
                canvas::paint_guides(&painter, &self.view, content, &self.doc.guides);
            }

            // Pull / move a guide when the drag began on a ruler strip; otherwise
            // run the active tool's input as usual.
            let handled_guide =
                self.show_rulers && self.handle_ruler_guides(&response, full, content);
            if !handled_guide {
                self.handle_input(&response, &ctx);
            }

            // Free-transform box around the selection (Select tool). Drawn under
            // the per-path anchor handles so the anchors stay clickable on top.
            if self.show_transform_box() {
                if let Some(bbox) = self.selection_bbox() {
                    canvas::paint_transform_box(&painter, &self.view, &bbox);
                }
            }

            // Editable anchors/handles for the primary selected path.
            if let Some(i) = self.primary() {
                if let Some(Shape::Path {
                    points, handles, ..
                }) = self.doc.shapes.get(i)
                {
                    canvas::paint_path_handles(&painter, &self.view, points, handles);
                }
            }

            self.draw_preview(&painter);

            // Rubber-band marquee box (Select tool, drag on empty canvas).
            if let Some(bbox) = self.marquee_rect() {
                canvas::paint_marquee(&painter, &self.view, &bbox);
            }

            // Active smart-guide snap lines over everything.
            canvas::paint_snap_lines(
                &painter,
                content,
                self.inter.snap_lines.line_x,
                self.inter.snap_lines.line_y,
                &self.view,
            );

            // Rulers last so the strips sit on top of any overscrolled content.
            if self.show_rulers {
                canvas::paint_rulers(&painter, &self.view, full, cursor);
            }
        });
    }

    /// Handle dragging guides out of (or along) the ruler strips. Returns `true`
    /// when a guide drag is active this frame, so the caller skips tool input.
    ///
    /// A drag that *starts* on a ruler strip pulls a new guide (vertical from the
    /// left ruler, horizontal from the top ruler). Dragging carries it; releasing
    /// over the rulers cancels (removes a fresh guide / deletes a moved one),
    /// otherwise it commits at the snapped coordinate. Each guide edit is one
    /// undo step.
    fn handle_ruler_guides(
        &mut self,
        response: &egui::Response,
        full: egui::Rect,
        content: egui::Rect,
    ) -> bool {
        let pointer = response.hover_pos();
        let in_top =
            pointer.is_some_and(|p| p.y < full.top() + canvas::RULER_PX && p.x >= full.left());
        let in_left =
            pointer.is_some_and(|p| p.x < full.left() + canvas::RULER_PX && p.y >= full.top());

        if response.drag_started() && self.inter.guide_drag.is_none() {
            // A fresh pull only begins inside a ruler strip. Left strip → vertical
            // guide; top strip → horizontal guide. (The top-left corner counts as
            // the top strip.)
            if in_top {
                self.begin_interaction();
                self.doc.guides.push(Guide::Horizontal(0.0));
                self.inter.guide_drag = Some(GuideDrag {
                    vertical: false,
                    existing: Some(self.doc.guides.len() - 1),
                });
            } else if in_left {
                self.begin_interaction();
                self.doc.guides.push(Guide::Vertical(0.0));
                self.inter.guide_drag = Some(GuideDrag {
                    vertical: true,
                    existing: Some(self.doc.guides.len() - 1),
                });
            }
        }

        let (vertical, existing) = match &self.inter.guide_drag {
            Some(gd) => (gd.vertical, gd.existing),
            None => return false,
        };

        if response.dragged() {
            if let (Some(p), Some(idx)) = (pointer, existing) {
                let (dx, dy) = self.view.screen_to_doc(p);
                let snapped = if self.snap.to_grid {
                    snap::snap_point_to_grid(dx, dy, self.snap.grid, self.snap_tol())
                } else {
                    (dx, dy)
                };
                if let Some(g) = self.doc.guides.get_mut(idx) {
                    *g = if vertical {
                        Guide::Vertical(snapped.0)
                    } else {
                        Guide::Horizontal(snapped.1)
                    };
                }
            }
        }

        if response.drag_stopped() {
            let over_ruler =
                in_top || in_left || !content.contains(pointer.unwrap_or(full.center()));
            if let Some(idx) = existing {
                if over_ruler && idx < self.doc.guides.len() {
                    // Dropped back on the ruler: discard this guide.
                    self.doc.guides.remove(idx);
                }
            }
            self.inter.guide_drag = None;
            self.commit_interaction();
        }

        true
    }

    fn handle_input(&mut self, response: &egui::Response, ctx: &egui::Context) {
        let pointer = response.hover_pos();
        let doc_pos = pointer.map(|p| self.view.screen_to_doc(p));

        // Middle-drag always pans, regardless of tool.
        let middle_down = ctx.input(|i| i.pointer.middle_down());
        if middle_down {
            self.view.pan += response.drag_delta();
            return;
        }

        match self.tool {
            Tool::Select => self.handle_select(response, doc_pos),
            Tool::Rect | Tool::Ellipse | Tool::Line => self.handle_create_drag(response, doc_pos),
            Tool::Pen => self.handle_pen(response, doc_pos),
            Tool::Artboard => self.handle_artboard(response, doc_pos),
        }
    }

    /// Artboard tool input: drag out a new artboard from empty canvas, drag an
    /// existing board to move it, or click a board to make it active. Each
    /// create / move lands as a single undo step (artboards live on the
    /// `Document`, so the snapshot history captures them).
    fn handle_artboard(&mut self, response: &egui::Response, doc_pos: Option<(f32, f32)>) {
        use super::ArtboardDrag;
        if response.drag_started() {
            if let Some((x, y)) = doc_pos {
                self.begin_interaction();
                match crate::artboard::artboard_at(&self.doc.artboards, x, y) {
                    Some(idx) => {
                        self.doc.active_artboard = idx;
                        self.inter.artboard_drag = Some(ArtboardDrag::Move {
                            index: idx,
                            last: (x, y),
                        });
                    }
                    None => {
                        self.inter.artboard_drag = Some(ArtboardDrag::Create { start: (x, y) });
                    }
                }
            }
        }
        if response.dragged() {
            if let (Some(ArtboardDrag::Move { index, last }), Some((x, y))) =
                (self.inter.artboard_drag.as_ref(), doc_pos)
            {
                let (index, last) = (*index, *last);
                let (mdx, mdy) = (x - last.0, y - last.1);
                if let Some(ab) = self.doc.artboards.get_mut(index) {
                    ab.rect[0] += mdx;
                    ab.rect[1] += mdy;
                }
                if let Some(ArtboardDrag::Move { last, .. }) = self.inter.artboard_drag.as_mut() {
                    *last = (x, y);
                }
            }
            // Create-drag preview is recomputed each frame from the live cursor.
            if let (Some(ArtboardDrag::Create { .. }), Some(p)) =
                (self.inter.artboard_drag.as_ref(), doc_pos)
            {
                self.inter.drag_now = Some(p);
            }
        }
        if response.drag_stopped() {
            match self.inter.artboard_drag.take() {
                Some(ArtboardDrag::Create { start }) => {
                    if let Some(end) = doc_pos {
                        self.finish_artboard_create(start, end);
                    }
                    self.inter.drag_now = None;
                    self.commit_interaction();
                }
                Some(ArtboardDrag::Move { .. }) => {
                    self.commit_interaction();
                }
                None => {}
            }
        }
        // Plain click selects the active artboard under the cursor.
        if response.clicked() {
            if let Some((x, y)) = doc_pos {
                if let Some(idx) = crate::artboard::artboard_at(&self.doc.artboards, x, y) {
                    self.doc.active_artboard = idx;
                }
            }
        }
    }

    /// The artboard rectangle being previewed this frame while the Artboard tool
    /// drags out a new board, or `None`. A `Move` drag mutates the board
    /// directly, so it needs no preview.
    fn artboard_preview(&self) -> Option<[f32; 4]> {
        use super::ArtboardDrag;
        match (&self.inter.artboard_drag, self.inter.drag_now) {
            (Some(ArtboardDrag::Create { start }), Some(now)) => {
                let x = start.0.min(now.0);
                let y = start.1.min(now.1);
                let w = (now.0 - start.0).abs();
                let h = (now.1 - start.1).abs();
                if w < 1.0 && h < 1.0 {
                    None
                } else {
                    Some([x, y, w, h])
                }
            }
            _ => None,
        }
    }

    fn handle_select(&mut self, response: &egui::Response, doc_pos: Option<(f32, f32)>) {
        let tol = 4.0 / self.view.zoom;
        let alt = response.ctx.input(|i| i.modifiers.alt);

        // Direct-select editing gestures on the selected path:
        //  · Alt-click an anchor  -> toggle smooth/corner (convert)
        //  · double-click an anchor -> delete it
        //  · double-click a segment -> insert an anchor there
        if response.double_clicked() {
            if let Some((x, y)) = doc_pos {
                if self.try_delete_anchor(x, y) || self.try_insert_anchor(x, y) {
                    return;
                }
            }
        }
        if alt && response.clicked() {
            if let Some((x, y)) = doc_pos {
                if self.try_convert_anchor(x, y) {
                    return;
                }
            }
        }

        if response.drag_started() {
            if let Some((x, y)) = doc_pos {
                // First: grabbing an anchor/handle of the primary path.
                if let Some(edit) = self.hit_path_edit(x, y) {
                    self.begin_interaction();
                    self.inter.path_edit = Some(edit);
                    self.inter.move_last = None;
                    return;
                }
                // Next: grabbing a transform-box handle (scale/rotate). These can
                // sit outside the shape, so they're tested before shape picking.
                if let Some(kind) = self.hit_transform_handle(x, y) {
                    self.begin_transform(kind, x, y);
                    self.inter.move_last = None;
                    return;
                }
                // Else: pick the topmost shape under the cursor to begin a move.
                // If it is already part of the selection, keep the whole set so
                // we drag everything together; otherwise it becomes the single
                // selection.
                let hit = self
                    .doc
                    .shapes
                    .iter()
                    .enumerate()
                    .rev()
                    .find(|(_, s)| s.visible() && s.hit(x, y, tol))
                    .map(|(i, _)| i);
                if let Some(i) = hit {
                    if !self.is_selected(i) {
                        self.select_only(Some(i));
                        self.expand_selection_to_groups();
                    }
                    self.inter.move_last = Some((x, y));
                    // Snapshot the start of a move so the whole drag is one undo.
                    self.begin_interaction();
                } else {
                    // Empty canvas: begin a rubber-band marquee. Shift extends the
                    // existing selection; a plain marquee replaces it.
                    self.inter.move_last = None;
                    let shift = response.ctx.input(|i| i.modifiers.shift);
                    self.inter.marquee_base = if shift {
                        self.selection.clone()
                    } else {
                        Vec::new()
                    };
                    self.inter.marquee = Some(((x, y), (x, y)));
                }
            }
        }

        if response.dragged() {
            if let Some((anchor, _)) = self.inter.marquee {
                // Grow the marquee and recompute the live selection.
                if let Some((x, y)) = doc_pos {
                    self.inter.marquee = Some((anchor, (x, y)));
                }
                self.update_marquee_selection();
            } else if self.inter.transform.is_some() {
                if let Some((x, y)) = doc_pos {
                    let uniform = response.ctx.input(|i| i.modifiers.shift);
                    self.drag_transform(x, y, uniform);
                }
            } else if let (Some(edit), Some((x, y)), Some(i)) =
                (self.inter.path_edit, doc_pos, self.primary())
            {
                self.drag_path_edit(i, edit, x, y);
            } else if let (Some((x, y)), Some((lx, ly))) = (doc_pos, self.inter.move_last) {
                // Move every selected shape by the same delta, then snap the
                // selection's bounding box to grid / guides / other objects.
                let (mut dx, mut dy) = (x - lx, y - ly);
                let n = self.doc.shapes.len();
                let exclude: Vec<usize> = self.selection.clone();
                self.inter.snap_lines = SnapResult::default();
                if let Some(b) = self.selection_bbox() {
                    let moved = [b[0] + dx, b[1] + dy, b[2], b[3]];
                    let r = self.snap_box(&moved, &exclude);
                    dx += r.dx;
                    dy += r.dy;
                    self.inter.snap_lines = r;
                }
                for &i in &self.selection {
                    if i < n {
                        self.doc.shapes[i].translate(dx, dy);
                    }
                }
                self.inter.move_last = Some((x, y));
            } else {
                // No shape grabbed: drag pans the canvas.
                self.view.pan += response.drag_delta();
            }
        }

        if response.drag_stopped() {
            // Finalize a marquee: the live selection is already set; just drop the
            // box. A marquee never mutates the document, so no undo entry.
            if self.inter.marquee.take().is_some() {
                self.inter.marquee_base.clear();
            }
            self.inter.move_last = None;
            self.inter.path_edit = None;
            self.inter.transform = None;
            self.inter.snap_lines = SnapResult::default();
            // Finalize a coalesced move / anchor-edit / transform (no-op drags
            // are dropped).
            self.commit_interaction();
        }

        if response.clicked() {
            if let Some((x, y)) = doc_pos {
                // A click that lands on a transform handle keeps the selection
                // (the user was aiming for the box, not the canvas behind it).
                if self.hit_transform_handle(x, y).is_some() {
                    return;
                }
                let hit = self
                    .doc
                    .shapes
                    .iter()
                    .enumerate()
                    .rev()
                    .find(|(_, s)| s.visible() && s.hit(x, y, tol))
                    .map(|(i, _)| i);
                let shift = response.ctx.input(|inp| inp.modifiers.shift);
                if shift {
                    // Shift-click toggles the hit shape (and its group) in the
                    // multi-selection.
                    if let Some(i) = hit {
                        self.toggle_group_selection(i);
                    }
                } else {
                    self.select_only(hit);
                    self.expand_selection_to_groups();
                }
            }
        }
    }

    /// The current marquee box as a normalised document-space `[x, y, w, h]`
    /// (non-negative extent), or `None` when no marquee is active.
    fn marquee_rect(&self) -> Option<[f32; 4]> {
        self.inter.marquee.map(|(a, b)| {
            let x = a.0.min(b.0);
            let y = a.1.min(b.1);
            [x, y, (a.0 - b.0).abs(), (a.1 - b.1).abs()]
        })
    }

    /// Recompute the live selection from the active marquee: every visible shape
    /// whose bounding box intersects the box is selected, added on top of the
    /// captured base (so a shift-marquee is additive). Intersected shapes are
    /// pushed in document order so the topmost stays primary.
    fn update_marquee_selection(&mut self) {
        let Some(rect) = self.marquee_rect() else {
            return;
        };
        // Start from the base (empty for a plain marquee, the prior selection for
        // a shift-marquee), then append intersected shapes not already present.
        let mut sel = self.inter.marquee_base.clone();
        for (i, s) in self.doc.shapes.iter().enumerate() {
            if !s.visible() {
                continue;
            }
            if let Some(b) = s.bounds() {
                if document::rects_intersect(&[b.x, b.y, b.w, b.h], &rect) && !sel.contains(&i) {
                    sel.push(i);
                }
            }
        }
        // A marquee that touches any group member selects the whole group.
        self.selection = crate::group::expand_selection(&self.group_tags(), &sel);
    }

    /// Mutate a selected path while dragging one of its anchors or handles.
    fn drag_path_edit(&mut self, i: usize, edit: PathEdit, x: f32, y: f32) {
        if let Some(Shape::Path {
            points, handles, ..
        }) = self.doc.shapes.get_mut(i)
        {
            if handles.len() < points.len() {
                handles.resize(points.len(), (0.0, 0.0));
            }
            match edit {
                PathEdit::Anchor(k) => {
                    if let Some(p) = points.get_mut(k) {
                        *p = (x, y);
                    }
                }
                PathEdit::Handle(k) => {
                    if let (Some(&(ax, ay)), Some(h)) = (points.get(k), handles.get_mut(k)) {
                        *h = (x - ax, y - ay);
                    }
                }
            }
        }
    }

    fn handle_create_drag(&mut self, response: &egui::Response, doc_pos: Option<(f32, f32)>) {
        // Snap both the start corner and the live corner so a fresh shape lands
        // on the grid / guides / other objects.
        if response.drag_started() {
            let snapped = doc_pos.map(|(x, y)| self.snap_point(x, y, &[]));
            self.inter.drag_start = snapped;
            self.inter.drag_now = snapped;
        }
        if response.dragged() {
            if let Some((x, y)) = doc_pos {
                let (sx, sy) = self.snap_point(x, y, &[]);
                self.inter.snap_lines = if self.snap.any() {
                    let f = SnapFeatures::point(x, y);
                    let mut t = self.snap_targets(&[]);
                    if self.snap.to_grid {
                        t.xs.extend(snap::grid_targets_near(x, self.snap.grid));
                        t.ys.extend(snap::grid_targets_near(y, self.snap.grid));
                    }
                    snap::snap_delta(&f, &t, self.snap_tol())
                } else {
                    SnapResult::default()
                };
                self.inter.drag_now = Some((sx, sy));
            }
        }
        if response.drag_stopped() {
            if let (Some(a), Some(b)) = (self.inter.drag_start, self.inter.drag_now) {
                if let Some(shape) = self.shape_from_drag(a, b) {
                    self.checkpoint();
                    self.doc.shapes.push(shape);
                    self.select_only(Some(self.doc.shapes.len() - 1));
                }
            }
            self.inter.drag_start = None;
            self.inter.drag_now = None;
            self.inter.snap_lines = SnapResult::default();
        }
    }

    fn shape_from_drag(&self, a: (f32, f32), b: (f32, f32)) -> Option<Shape> {
        match self.tool {
            Tool::Rect | Tool::Ellipse => {
                let x = a.0.min(b.0);
                let y = a.1.min(b.1);
                let w = (b.0 - a.0).abs();
                let h = (b.1 - a.1).abs();
                if w < 1.0 && h < 1.0 {
                    return None;
                }
                let rect = [x, y, w, h];
                Some(if self.tool == Tool::Rect {
                    Shape::Rect {
                        rect,
                        fill: self.fill,
                        fill_gradient: self.fill_gradient.clone(),
                        stroke: self.stroke,
                        stroke_w: self.stroke_w,
                        stroke_style: self.stroke_style.clone(),
                        visible: true,
                        group: None,
                    }
                } else {
                    Shape::Ellipse {
                        rect,
                        fill: self.fill,
                        fill_gradient: self.fill_gradient.clone(),
                        stroke: self.stroke,
                        stroke_w: self.stroke_w,
                        stroke_style: self.stroke_style.clone(),
                        visible: true,
                        group: None,
                    }
                })
            }
            Tool::Line => {
                if (b.0 - a.0).abs() < 1.0 && (b.1 - a.1).abs() < 1.0 {
                    return None;
                }
                Some(Shape::Line {
                    p0: a,
                    p1: b,
                    stroke: self.stroke,
                    stroke_w: self.stroke_w.max(1.0),
                    stroke_style: self.stroke_style.clone(),
                    visible: true,
                    group: None,
                })
            }
            _ => None,
        }
    }

    fn handle_pen(&mut self, response: &egui::Response, doc_pos: Option<(f32, f32)>) {
        // Double-click closes the path.
        if response.double_clicked() {
            self.commit_pen(true);
            return;
        }

        // Press: place a new anchor (with a zeroed handle) and arm a handle-drag
        // on it. A plain click that doesn't drag leaves the handle at zero (a
        // corner); dragging sets the out-tangent.
        if response.drag_started() || (response.clicked() && self.inter.pen_drag.is_none()) {
            if let Some(p) = doc_pos {
                self.inter.pen_points.push(p);
                self.inter.pen_handles.push((0.0, 0.0));
                let i = self.inter.pen_points.len() - 1;
                self.inter.pen_drag = Some(PenDrag::Handle(i));
            }
        }

        // Drag: set the out-handle of the freshest anchor (offset from anchor).
        if response.dragged() {
            if let (Some(PenDrag::Handle(i)), Some((x, y))) = (self.inter.pen_drag, doc_pos) {
                if let Some(&(ax, ay)) = self.inter.pen_points.get(i) {
                    if i < self.inter.pen_handles.len() {
                        self.inter.pen_handles[i] = (x - ax, y - ay);
                    }
                }
            }
        }

        if response.drag_stopped() || response.clicked() {
            self.inter.pen_drag = None;
        }
    }

    /// Find an editable element (anchor or handle) of the primary path near the
    /// document-space cursor. Handles take priority over anchors.
    fn hit_path_edit(&self, x: f32, y: f32) -> Option<PathEdit> {
        let i = self.primary()?;
        let Some(Shape::Path {
            points, handles, ..
        }) = self.doc.shapes.get(i)
        else {
            return None;
        };
        let tol = 6.0 / self.view.zoom;
        for (k, &p) in points.iter().enumerate() {
            let h = document::handle_at(handles, k);
            if h.0 != 0.0 || h.1 != 0.0 {
                let out = (p.0 + h.0, p.1 + h.1);
                let inp = (p.0 - h.0, p.1 - h.1);
                if (x - out.0).hypot(y - out.1) <= tol || (x - inp.0).hypot(y - inp.1) <= tol {
                    return Some(PathEdit::Handle(k));
                }
            }
        }
        for (k, &p) in points.iter().enumerate() {
            if (x - p.0).hypot(y - p.1) <= tol {
                return Some(PathEdit::Anchor(k));
            }
        }
        None
    }

    /// Whether the primary selection is a `Path` (the only directly-editable
    /// shape type).
    pub(super) fn selected_is_path(&self) -> bool {
        matches!(
            self.primary().and_then(|i| self.doc.shapes.get(i)),
            Some(Shape::Path { .. })
        )
    }

    /// Index of the anchor of the primary path nearest `(x, y)` within the
    /// anchor pick tolerance, if any.
    fn hit_anchor(&self, x: f32, y: f32) -> Option<usize> {
        let i = self.primary()?;
        let Some(Shape::Path { points, .. }) = self.doc.shapes.get(i) else {
            return None;
        };
        let tol = 6.0 / self.view.zoom;
        points
            .iter()
            .enumerate()
            .filter(|(_, &p)| (x - p.0).hypot(y - p.1) <= tol)
            .min_by(|(_, &a), (_, &b)| {
                let da = (x - a.0).hypot(y - a.1);
                let db = (x - b.0).hypot(y - b.1);
                da.total_cmp(&db)
            })
            .map(|(k, _)| k)
    }

    /// Delete the anchor under `(x, y)` on the selected path. Returns `true` if
    /// one was removed (undoable). Refuses to drop below two anchors.
    fn try_delete_anchor(&mut self, x: f32, y: f32) -> bool {
        let Some(i) = self.primary() else {
            return false;
        };
        let Some(k) = self.hit_anchor(x, y) else {
            return false;
        };
        // Only checkpoint if the delete will actually happen (≥3 points).
        let deletable = matches!(
            self.doc.shapes.get(i),
            Some(Shape::Path { points, .. }) if points.len() > 2
        );
        if !deletable {
            return false;
        }
        self.checkpoint();
        if let Some(shape) = self.doc.shapes.get_mut(i) {
            if shape.delete_anchor(k) {
                self.status = "Deleted anchor".into();
                return true;
            }
        }
        false
    }

    /// Insert an anchor on the path segment under `(x, y)`. Returns `true` if an
    /// anchor was added (undoable).
    fn try_insert_anchor(&mut self, x: f32, y: f32) -> bool {
        let Some(i) = self.primary() else {
            return false;
        };
        let Some(Shape::Path { points, closed, .. }) = self.doc.shapes.get(i) else {
            return false;
        };
        let tol = 8.0 / self.view.zoom;
        let Some((seg, t)) = document::nearest_segment(points, *closed, x, y, tol) else {
            return false;
        };
        self.checkpoint();
        if let Some(shape) = self.doc.shapes.get_mut(i) {
            if shape.insert_anchor(seg, t).is_some() {
                self.status = "Added anchor".into();
                return true;
            }
        }
        false
    }

    /// Toggle the anchor under `(x, y)` between smooth and corner. Returns `true`
    /// if an anchor was hit and converted (undoable).
    fn try_convert_anchor(&mut self, x: f32, y: f32) -> bool {
        let Some(i) = self.primary() else {
            return false;
        };
        let Some(k) = self.hit_anchor(x, y) else {
            return false;
        };
        self.checkpoint();
        if let Some(shape) = self.doc.shapes.get_mut(i) {
            let smooth = shape.toggle_anchor_smooth(k);
            self.status = if smooth {
                "Converted to smooth".into()
            } else {
                "Converted to corner".into()
            };
        }
        true
    }

    fn draw_preview(&self, painter: &egui::Painter) {
        // Rubber-band preview for create-drag.
        if let (Some(a), Some(b)) = (self.inter.drag_start, self.inter.drag_now) {
            if let Some(shape) = self.shape_from_drag(a, b) {
                canvas::paint_shape(painter, &self.view, &shape, false);
            }
        }
        // Pen in-progress curve preview (honors handles) + anchors/handles.
        if !self.inter.pen_points.is_empty() {
            if self.inter.pen_points.len() >= 2 {
                canvas::paint_path(
                    painter,
                    &self.view,
                    &self.inter.pen_points,
                    &self.inter.pen_handles,
                    false,
                    [0.0, 0.0, 0.0, 0.0],
                    None,
                    self.stroke,
                    self.stroke_w.max(1.0),
                    &self.stroke_style,
                );
            }
            canvas::paint_path_handles(
                painter,
                &self.view,
                &self.inter.pen_points,
                &self.inter.pen_handles,
            );
        }
    }
}
