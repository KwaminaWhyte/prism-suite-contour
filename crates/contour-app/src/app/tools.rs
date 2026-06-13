//! The central canvas: per-frame painting, ruler-guide dragging, and the
//! tool-input state machine (select / create / pen), plus the transform-box
//! hit-tests and direct-select anchor editing that drive it.

use super::{AnchorRef, ContourApp, DsDrag, GuideDrag, PathEdit, PenDrag, Tool, TransformKind};
use crate::canvas;
use crate::document::{self, Guide, Shape};
use crate::liveshape::LiveShape;
use crate::snap::{self, SnapFeatures, SnapResult};
use crate::transform::Handle;

impl ContourApp {
    /// Whether the on-canvas transform box should be shown: the Select tool, a
    /// non-empty selection with geometry, and no active per-path anchor edit
    /// (which has its own handles). The box is suppressed while *directly*
    /// editing a single path's anchors so the two handle sets don't clash.
    pub(super) fn show_transform_box(&self) -> bool {
        // The transform box is a Select-tool affordance. Direct-Select shows the
        // anchor/handle overlay instead.
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
    pub(super) fn hit_transform_handle(
        &self,
        x: f32,
        y: f32,
        shear: bool,
    ) -> Option<TransformKind> {
        if !self.show_transform_box() {
            return None;
        }
        let bbox = self.selection_bbox()?;
        let cursor = self.view.doc_to_screen((x, y));

        // Scale handles take priority (inner pick radius). With the primary
        // modifier (Cmd/Ctrl) held, dragging an *edge* handle shears instead of
        // scaling — corner handles never shear, so they still scale.
        for h in Handle::ALL {
            let hp = self.view.doc_to_screen((
                bbox[0] + bbox[2] * h.unit_pos().0,
                bbox[1] + bbox[3] * h.unit_pos().1,
            ));
            if (cursor - hp).length() <= canvas::HANDLE_PICK_PX {
                if shear && !h.is_corner() {
                    return Some(TransformKind::Shear(h));
                }
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
            // Record the document-space cursor for the status bar (None when the
            // pointer leaves the canvas).
            self.cursor_doc = cursor.map(|p| self.view.screen_to_doc(p));

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
            // Bodies, with clipping + opacity masks resolved (mask paths paint
            // nothing; clipped content is cropped, opacity-masked content has the
            // mask's luminance multiplied into its alpha). Rings are drawn
            // separately below from the original shapes so a selected mask shows.
            for (i, s) in self.doc.render_shapes() {
                if !s.visible() {
                    continue;
                }
                let omask = self.doc.opacity_mask_of(i);
                canvas::paint_shape_masked(&painter, &self.view, &s, false, omask.as_ref());
            }
            // Placed symbol instances, resolved against their live masters (so a
            // master edit shows here immediately) and drawn over the plain
            // shapes. The selected instance gets a selection ring per resolved
            // shape so it reads as one placed object.
            for (inst_id, shapes) in self.doc.symbols.resolved_instances() {
                let selected = self.selected_instance == Some(inst_id);
                for s in &shapes {
                    if !s.visible() {
                        continue;
                    }
                    canvas::paint_shape(&painter, &self.view, s, false);
                    if selected {
                        canvas::paint_selection_ring(&painter, &self.view, s);
                    }
                }
            }
            // Placed / linked raster images, drawn over the shapes and symbol
            // instances. Each resolves its pixels (embedded bytes or a linked
            // file's cached pixels); a clipped image is clipped to its clip-path
            // bounds; the selected image gets a ring.
            for img in &self.doc.placed_images.list {
                if !img.visible {
                    continue;
                }
                if let Some((w, h, px)) = self.image_pixels_for(img) {
                    let selected = self.selected_image == Some(img.id);
                    canvas::paint_placed_image(
                        &painter,
                        &self.view,
                        img,
                        (w, h, &px),
                        selected,
                    );
                }
            }
            // Selection rings + a dashed outline for selected mask paths, drawn
            // from the original (pre-clip) shapes.
            for (i, s) in self.doc.shapes.iter().enumerate() {
                if !s.visible() {
                    continue;
                }
                if self.is_selected(i) {
                    canvas::paint_selection_ring(&painter, &self.view, s);
                }
                // A selected clip / opacity-mask path is otherwise invisible;
                // outline it so the user can see and edit the mask.
                if (s.is_mask() || s.is_omask()) && self.is_selected(i) {
                    canvas::paint_mask_outline(&painter, &self.view, s);
                }
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
            if self.tool == Tool::DirectSelect {
                self.paint_direct_select(&painter);
            } else if let Some(i) = self.primary() {
                if let Some(Shape::Path {
                    points, handles, ..
                }) = self.doc.shapes.get(i)
                {
                    canvas::paint_path_handles(&painter, &self.view, points, handles);
                }
            }

            self.draw_preview(&painter);
            self.draw_text_edit_overlay(&painter, &ctx);

            // Shape Builder gesture preview: highlight the regions the drag has
            // crossed and trace the drag path.
            if let Some(sb) = &self.inter.sb_drag {
                let picked = crate::shapebuilder::faces_along(&sb.faces, &sb.path);
                canvas::paint_shape_builder(
                    &painter,
                    &self.view,
                    &sb.faces,
                    &picked,
                    &sb.path,
                    sb.subtract,
                );
            }

            // Rubber-band marquee box (Select tool, drag on empty canvas).
            if let Some(bbox) = self.marquee_rect() {
                canvas::paint_marquee(&painter, &self.view, &bbox);
            }
            // Direct-Select anchor marquee.
            if let Some(bbox) = self.ds_marquee_rect() {
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
            Tool::DirectSelect => self.handle_direct_select(response, doc_pos),
            Tool::Rect | Tool::Ellipse | Tool::Line | Tool::Polygon | Tool::Star => {
                self.handle_create_drag(response, doc_pos)
            }
            Tool::Pen => self.handle_pen(response, doc_pos),
            Tool::Artboard => self.handle_artboard(response, doc_pos),
            Tool::Eyedropper => self.handle_eyedropper(response, doc_pos),
            Tool::ShapeBuilder => self.handle_shape_builder(response, doc_pos),
            Tool::Type => self.handle_type(response, doc_pos),
        }
    }

    /// Shape Builder input: drag across the selected shapes' overlapping regions
    /// to **unite** them into one path, or **Alt/Option-drag** to **delete** the
    /// regions dragged over.
    ///
    /// On drag start the region graph (atomic faces) of the selected shapes is
    /// built once; each dragged frame samples the pointer into the gesture path;
    /// on release the faces the path crossed are merged (or subtracted) and the
    /// selected shapes are replaced with the result (one undo step). Needs two or
    /// more selected shapes with a usable closed outline.
    fn handle_shape_builder(&mut self, response: &egui::Response, doc_pos: Option<(f32, f32)>) {
        use super::ShapeBuilderDrag;
        if response.drag_started() {
            // Gather the selected source shapes in paint order.
            let mut sources: Vec<usize> = self
                .selection
                .iter()
                .copied()
                .filter(|&i| i < self.doc.shapes.len())
                .collect();
            sources.sort_unstable();
            sources.dedup();
            let shapes: Vec<crate::document::Shape> = sources
                .iter()
                .map(|&i| self.doc.shapes[i].clone())
                .collect();
            let faces = crate::shapebuilder::build_faces(&shapes);
            if sources.len() < 2 || faces.is_empty() {
                self.status = "Shape Builder: select two or more overlapping shapes".into();
                return;
            }
            let subtract = response.ctx.input(|i| i.modifiers.alt);
            let mut path = Vec::new();
            if let Some(p) = doc_pos {
                path.push(p);
            }
            self.inter.sb_drag = Some(ShapeBuilderDrag {
                faces,
                sources,
                path,
                subtract,
            });
        }
        if response.dragged() {
            if let (Some(sb), Some(p)) = (self.inter.sb_drag.as_mut(), doc_pos) {
                sb.path.push(p);
            }
        }
        if response.drag_stopped() {
            if let Some(sb) = self.inter.sb_drag.take() {
                self.finish_shape_builder(sb.faces, sb.sources, sb.path, sb.subtract);
            }
        }
    }

    /// Eyedropper input: click a shape to sample its paint appearance.
    ///
    /// Mirrors Illustrator's eyedropper:
    /// - A **plain click** on a shape samples its appearance, applies it to the
    ///   current selection (one undo step), and updates the app's default paint
    ///   so the next new shape inherits it.
    /// - With **no selection**, a click only loads the sampled appearance into
    ///   the app defaults (nothing to apply to yet).
    /// - **Alt-click** never applies — it only loads the defaults (Illustrator's
    ///   "pick up but don't paint" modifier).
    ///
    /// Clicking empty canvas is a no-op (there is nothing to sample).
    fn handle_eyedropper(&mut self, response: &egui::Response, doc_pos: Option<(f32, f32)>) {
        if !response.clicked() {
            return;
        }
        let Some((x, y)) = doc_pos else { return };
        let tol = 4.0 / self.view.zoom;
        // Topmost visible shape under the cursor.
        let Some(src) = self
            .doc
            .shapes
            .iter()
            .enumerate()
            .rev()
            .find(|(_, s)| s.selectable() && s.hit(x, y, tol))
            .map(|(i, _)| i)
        else {
            return;
        };
        let appearance = crate::eyedropper::Appearance::sample(&self.doc.shapes[src]);
        let alt = response.ctx.input(|i| i.modifiers.alt);

        // Always load the app defaults from the sample (so the next drawn shape
        // inherits the picked look). Lines carry no fill; keep the prior default
        // fill in that case.
        if let Some(c) = appearance.fill {
            self.fill = c;
        }
        self.fill_gradient = appearance.fill_gradient.clone();
        self.stroke = appearance.stroke;
        self.stroke_w = appearance.stroke_w;
        self.stroke_style = appearance.stroke_style.clone();

        // Apply to the selection (unless Alt was held, or the source itself is the
        // only selected shape, or nothing is selected).
        let targets: Vec<usize> = self
            .selection
            .iter()
            .copied()
            .filter(|&i| i < self.doc.shapes.len() && i != src)
            .collect();
        if !alt && !targets.is_empty() {
            self.checkpoint();
            for i in &targets {
                appearance.apply_to(&mut self.doc.shapes[*i]);
            }
            let n = targets.len();
            self.status = format!(
                "Applied appearance to {n} {}",
                if n == 1 { "object" } else { "objects" }
            );
        } else {
            self.status = "Sampled appearance".into();
        }
    }

    /// Type tool input: click empty canvas to **place** a new point-type object
    /// and begin editing it; click an existing text object to **edit** its string;
    /// click anywhere else (or press Escape, handled by `handle_text_editing`) to
    /// **end** the current edit. Each placement is one undo step; subsequent
    /// keystrokes coalesce into the edit (committed when the edit ends).
    fn handle_type(&mut self, response: &egui::Response, doc_pos: Option<(f32, f32)>) {
        if !response.clicked() {
            return;
        }
        let Some((x, y)) = doc_pos else { return };
        let tol = 4.0 / self.view.zoom;
        // Existing text object under the cursor → edit it.
        let hit_text = self
            .doc
            .shapes
            .iter()
            .enumerate()
            .rev()
            .find(|(_, s)| {
                s.selectable() && matches!(s, Shape::Text { .. }) && s.hit(x, y, tol)
            })
            .map(|(i, _)| i);
        if let Some(i) = hit_text {
            self.begin_text_edit(i);
            return;
        }
        // Otherwise: finish any active edit, then place a fresh text object.
        self.end_text_edit();
        self.place_text(x, y);
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
                let shear = response.ctx.input(|i| i.modifiers.command);
                if let Some(kind) = self.hit_transform_handle(x, y, shear) {
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
                    .find(|(_, s)| s.selectable() && s.hit(x, y, tol))
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
                if self.hit_transform_handle(x, y, false).is_some() {
                    return;
                }
                let hit = self
                    .doc
                    .shapes
                    .iter()
                    .enumerate()
                    .rev()
                    .find(|(_, s)| s.selectable() && s.hit(x, y, tol))
                    .map(|(i, _)| i);
                // A placed image sits over the shapes; pick the topmost hit one
                // (clearing any shape selection) so it can be selected / clipped.
                let img_hit = self
                    .doc
                    .placed_images
                    .list
                    .iter()
                    .rev()
                    .find(|p| p.selectable() && p.hit(x, y))
                    .map(|p| p.id);
                if hit.is_none() {
                    if let Some(id) = img_hit {
                        self.selected_image = Some(id);
                        self.selected_instance = None;
                        self.select_only(None);
                        return;
                    }
                }
                // Clicking a shape (or empty canvas) drops any image selection.
                self.selected_image = None;
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
            // Hidden or locked shapes can't be marquee-picked.
            if !s.selectable() {
                continue;
            }
            if let Some(b) = s.bounds() {
                if document::rects_intersect(&[b.x, b.y, b.w, b.h], &rect) && !sel.contains(&i) {
                    sel.push(i);
                }
            }
        }
        // A marquee that touches any group / clip-set member selects the whole
        // unit (mirrors a click), via the shared group+clip expansion.
        self.selection = sel;
        self.expand_selection_to_groups();
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

    // --- Direct-Select tool --------------------------------------------------

    /// Switch the active tool, clearing the Direct-Select anchor selection when
    /// leaving (or entering) it so stale anchors never linger on another tool.
    pub(super) fn set_tool(&mut self, tool: Tool) {
        if tool != self.tool {
            self.clear_ds_selection();
            // Leaving the Type tool finalizes any in-progress text edit (dropping
            // an empty placeholder), so the keyboard returns to tool shortcuts.
            if self.tool == Tool::Type {
                self.end_text_edit();
            }
        }
        self.tool = tool;
    }

    /// Drop the Direct-Select anchor selection and any in-progress DS gesture.
    pub(super) fn clear_ds_selection(&mut self) {
        self.inter.ds_anchors.clear();
        self.inter.ds_drag = None;
        self.inter.ds_last = None;
        self.inter.ds_marquee = None;
        self.inter.ds_marquee_base.clear();
    }

    /// The index of the shape the Direct-Select tool edits: the primary
    /// selection if it is an editable (`Path` / `Compound`) shape, else `None`.
    fn ds_target(&self) -> Option<usize> {
        let i = self.primary()?;
        let s = self.doc.shapes.get(i)?;
        (s.contour_count() > 0).then_some(i)
    }

    /// Direct-Select input: pick / drag anchors and handles of the primary path
    /// (or compound path), marquee-select anchors on empty canvas, and edit the
    /// path with add (click a segment) / delete (Delete key) / convert
    /// (Alt-click an anchor) — all routed through the undo system.
    fn handle_direct_select(&mut self, response: &egui::Response, doc_pos: Option<(f32, f32)>) {
        let alt = response.ctx.input(|i| i.modifiers.alt);
        let shift = response.ctx.input(|i| i.modifiers.shift);

        // Alt-click an anchor converts it smooth↔corner (before a drag begins).
        if alt && response.clicked() {
            if let Some((x, y)) = doc_pos {
                if self.ds_convert_anchor(x, y) {
                    return;
                }
            }
        }

        if response.drag_started() {
            if let Some((x, y)) = doc_pos {
                // 1. A handle knob of an already-selected anchor takes priority.
                if let Some((anchor, out)) = self.ds_hit_handle(x, y) {
                    self.begin_interaction();
                    self.inter.ds_drag = Some(DsDrag::Handle { anchor, out });
                    return;
                }
                // 2. An anchor: select it (extend with shift) and start a move.
                if let Some(a) = self.ds_hit_anchor(x, y) {
                    if shift {
                        self.toggle_ds_anchor(a);
                    } else if !self.inter.ds_anchors.contains(&a) {
                        self.inter.ds_anchors = vec![a];
                    }
                    self.begin_interaction();
                    self.inter.ds_drag = Some(DsDrag::Anchors);
                    self.inter.ds_last = Some((x, y));
                    return;
                }
                // 3. Empty canvas: rubber-band over anchors. Shift adds to the
                //    current anchor selection; a plain marquee replaces it.
                self.inter.ds_marquee_base = if shift {
                    self.inter.ds_anchors.clone()
                } else {
                    Vec::new()
                };
                self.inter.ds_marquee = Some(((x, y), (x, y)));
            }
        }

        if response.dragged() {
            if let Some((x, y)) = doc_pos {
                if let Some(((ax, ay), _)) = self.inter.ds_marquee {
                    self.inter.ds_marquee = Some(((ax, ay), (x, y)));
                    self.update_ds_marquee();
                } else if let Some(DsDrag::Handle { anchor, out }) = self.inter.ds_drag {
                    self.ds_drag_handle(anchor, out, x, y);
                } else if self.inter.ds_drag == Some(DsDrag::Anchors) {
                    if let Some((lx, ly)) = self.inter.ds_last {
                        self.ds_move_anchors(x - lx, y - ly);
                        self.inter.ds_last = Some((x, y));
                    }
                } else {
                    // No anchor/handle grabbed: pan the canvas.
                    self.view.pan += response.drag_delta();
                }
            }
        }

        if response.drag_stopped() {
            // A marquee never mutates the document, so it commits no undo entry;
            // anchor / handle drags coalesce into one (commit drops no-op drags).
            self.inter.ds_marquee = None;
            self.inter.ds_marquee_base.clear();
            let was_edit = self.inter.ds_drag.take().is_some();
            self.inter.ds_last = None;
            if was_edit {
                self.commit_interaction();
            }
        }

        // A plain click (press-release, no drag): select / toggle the anchor it
        // lands on, add an anchor when it lands on a segment, or — on empty
        // canvas — re-pick the path under the cursor and clear the anchor set.
        if response.clicked() {
            if let Some((x, y)) = doc_pos {
                if alt {
                    return; // handled above
                }
                // A click on a handle knob keeps the selection (no drag occurred).
                if self.ds_hit_handle(x, y).is_some() {
                    return;
                }
                // Click an anchor: select it (Shift toggles in the multi-set).
                if let Some(a) = self.ds_hit_anchor(x, y) {
                    if shift {
                        self.toggle_ds_anchor(a);
                    } else {
                        self.inter.ds_anchors = vec![a];
                    }
                    return;
                }
                if self.ds_insert_anchor(x, y) {
                    return;
                }
                if shift {
                    return;
                }
                // Click empty space: pick the path under the cursor (so the tool
                // can move between paths) and drop the anchor selection.
                let tol = 4.0 / self.view.zoom;
                let hit = self
                    .doc
                    .shapes
                    .iter()
                    .enumerate()
                    .rev()
                    .find(|(_, s)| s.selectable() && s.hit(x, y, tol) && s.contour_count() > 0)
                    .map(|(i, _)| i);
                if let Some(i) = hit {
                    if Some(i) != self.primary() {
                        self.select_only(Some(i));
                    }
                }
                self.inter.ds_anchors.clear();
            }
        }
    }

    /// The current Direct-Select marquee as a normalised document `[x, y, w, h]`.
    pub(super) fn ds_marquee_rect(&self) -> Option<[f32; 4]> {
        self.inter.ds_marquee.map(|(a, b)| {
            let x = a.0.min(b.0);
            let y = a.1.min(b.1);
            [x, y, (a.0 - b.0).abs(), (a.1 - b.1).abs()]
        })
    }

    /// Recompute the selected-anchor set from the active marquee: every anchor of
    /// the primary shape caught in the box, on top of the captured base.
    fn update_ds_marquee(&mut self) {
        let Some(rect) = self.ds_marquee_rect() else {
            return;
        };
        let Some(i) = self.ds_target() else { return };
        let mut sel = self.inter.ds_marquee_base.clone();
        if let Some(shape) = self.doc.shapes.get(i) {
            for c in 0..shape.contour_count() {
                if let Some((points, _, _)) = shape.contour(c) {
                    for a in document::anchors_in_rect(points, &rect) {
                        let r = AnchorRef {
                            contour: c,
                            anchor: a,
                        };
                        if !sel.contains(&r) {
                            sel.push(r);
                        }
                    }
                }
            }
        }
        self.inter.ds_anchors = sel;
    }

    /// Find the anchor of the primary shape nearest `(x, y)` within tolerance,
    /// across every sub-contour. Anchors are picked over segments.
    fn ds_hit_anchor(&self, x: f32, y: f32) -> Option<AnchorRef> {
        let i = self.ds_target()?;
        let shape = self.doc.shapes.get(i)?;
        let tol = 6.0 / self.view.zoom;
        let mut best: Option<(AnchorRef, f32)> = None;
        for c in 0..shape.contour_count() {
            let (points, _, _) = shape.contour(c)?;
            for (a, &p) in points.iter().enumerate() {
                let d = (x - p.0).hypot(y - p.1);
                if d <= tol && best.is_none_or(|(_, bd)| d < bd) {
                    best = Some((
                        AnchorRef {
                            contour: c,
                            anchor: a,
                        },
                        d,
                    ));
                }
            }
        }
        best.map(|(r, _)| r)
    }

    /// Find a tangent handle knob (of a currently-selected anchor) near `(x, y)`.
    /// Only selected anchors expose their handles, mirroring Illustrator where
    /// the handles appear once an anchor is picked. Returns the anchor plus
    /// whether the out-knob (`true`) or the mirrored in-knob (`false`) was hit.
    fn ds_hit_handle(&self, x: f32, y: f32) -> Option<(AnchorRef, bool)> {
        let i = self.ds_target()?;
        let shape = self.doc.shapes.get(i)?;
        let tol = 6.0 / self.view.zoom;
        let mut best: Option<((AnchorRef, bool), f32)> = None;
        for &r in &self.inter.ds_anchors {
            let Some((points, handles, _)) = shape.contour(r.contour) else {
                continue;
            };
            if let Some((out, inp)) = document::handle_endpoints(points, handles, r.anchor) {
                let do_ = (x - out.0).hypot(y - out.1);
                if do_ <= tol && best.is_none_or(|(_, bd)| do_ < bd) {
                    best = Some(((r, true), do_));
                }
                let di = (x - inp.0).hypot(y - inp.1);
                if di <= tol && best.is_none_or(|(_, bd)| di < bd) {
                    best = Some(((r, false), di));
                }
            }
        }
        best.map(|(rb, _)| rb)
    }

    /// Toggle anchor `a` in the Direct-Select selection (shift-click).
    fn toggle_ds_anchor(&mut self, a: AnchorRef) {
        if let Some(pos) = self.inter.ds_anchors.iter().position(|&r| r == a) {
            self.inter.ds_anchors.remove(pos);
        } else {
            self.inter.ds_anchors.push(a);
        }
    }

    /// Move every selected anchor by `(dx, dy)` (the multi-anchor drag).
    fn ds_move_anchors(&mut self, dx: f32, dy: f32) {
        let Some(i) = self.ds_target() else { return };
        // Collect the new positions first (immutable reads), then apply.
        let moves: Vec<(usize, usize, f32, f32)> = {
            let Some(shape) = self.doc.shapes.get(i) else {
                return;
            };
            self.inter
                .ds_anchors
                .iter()
                .filter_map(|r| {
                    shape
                        .contour(r.contour)
                        .and_then(|(pts, _, _)| pts.get(r.anchor).copied())
                        .map(|(px, py)| (r.contour, r.anchor, px + dx, py + dy))
                })
                .collect()
        };
        if let Some(shape) = self.doc.shapes.get_mut(i) {
            for (c, a, nx, ny) in moves {
                shape.set_anchor(c, a, nx, ny);
            }
        }
    }

    /// Reshape a single anchor's tangent by dragging its out- or in-handle knob to
    /// `(x, y)`. Both knobs mirror through the anchor (the model stores one
    /// symmetric out-offset), so dragging the in-knob to `(x, y)` is the same as
    /// putting the out-knob at the mirror.
    fn ds_drag_handle(&mut self, anchor: AnchorRef, out: bool, x: f32, y: f32) {
        let Some(i) = self.ds_target() else { return };
        let Some(shape) = self.doc.shapes.get_mut(i) else {
            return;
        };
        if out {
            shape.set_handle(anchor.contour, anchor.anchor, x, y);
        } else {
            // Mirror the cursor about the anchor so the out-handle lands opposite.
            if let Some((pts, _, _)) = shape.contour(anchor.contour) {
                if let Some(&(ax, ay)) = pts.get(anchor.anchor) {
                    let (mx, my) = (2.0 * ax - x, 2.0 * ay - y);
                    shape.set_handle(anchor.contour, anchor.anchor, mx, my);
                }
            }
        }
    }

    /// Alt-click convert: toggle the anchor under `(x, y)` smooth↔corner. Returns
    /// `true` if an anchor was hit (undoable).
    fn ds_convert_anchor(&mut self, x: f32, y: f32) -> bool {
        let Some(i) = self.ds_target() else {
            return false;
        };
        let Some(a) = self.ds_hit_anchor(x, y) else {
            return false;
        };
        self.checkpoint();
        if let Some(shape) = self.doc.shapes.get_mut(i) {
            let smooth = shape.toggle_anchor_smooth_in(a.contour, a.anchor);
            self.status = if smooth {
                "Converted to smooth".into()
            } else {
                "Converted to corner".into()
            };
            // Keep just this anchor selected so its (new) handles are grabbable.
            if !self.inter.ds_anchors.contains(&a) {
                self.inter.ds_anchors = vec![a];
            }
        }
        true
    }

    /// Insert an anchor on the segment of the primary shape under `(x, y)`, across
    /// every sub-contour. Returns `true` if one was added (undoable). The new
    /// anchor becomes the sole selection.
    fn ds_insert_anchor(&mut self, x: f32, y: f32) -> bool {
        let Some(i) = self.ds_target() else {
            return false;
        };
        let tol = 6.0 / self.view.zoom;
        // Find the nearest segment across all sub-contours.
        let mut best: Option<(usize, usize, f32, f32)> = None; // (contour, seg, t, dist)
        if let Some(shape) = self.doc.shapes.get(i) {
            for c in 0..shape.contour_count() {
                if let Some((points, _, closed)) = shape.contour(c) {
                    if let Some((seg, t)) = document::nearest_segment(points, closed, x, y, tol) {
                        // Distance is re-derived: project the cursor on the chord.
                        let n = points.len();
                        let a = points[seg];
                        let b = points[(seg + 1) % n];
                        let d = point_seg_dist(x, y, a, b);
                        if best.is_none_or(|(_, _, _, bd)| d < bd) {
                            best = Some((c, seg, t, d));
                        }
                    }
                }
            }
        }
        let Some((c, seg, t, _)) = best else {
            return false;
        };
        self.checkpoint();
        if let Some(shape) = self.doc.shapes.get_mut(i) {
            if let Some(idx) = shape.insert_anchor_in(c, seg, t) {
                self.status = "Added anchor".into();
                self.inter.ds_anchors = vec![AnchorRef {
                    contour: c,
                    anchor: idx,
                }];
                return true;
            }
        }
        false
    }

    /// Delete every selected anchor of the primary shape (Delete key), re-fitting
    /// the path. Anchors are removed high-index-first per contour so indices stay
    /// valid. Refuses to drop a contour below two points. One undo step; a delete
    /// that removes nothing records no history (via the begin/commit no-op drop).
    pub(super) fn delete_selected_anchors(&mut self) {
        let Some(i) = self.ds_target() else { return };
        if self.inter.ds_anchors.is_empty() {
            return;
        }
        // Group anchors by contour, descending index, deduped.
        use std::collections::BTreeMap;
        let mut by_contour: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
        for r in &self.inter.ds_anchors {
            by_contour.entry(r.contour).or_default().push(r.anchor);
        }
        // Snapshot first; commit drops the checkpoint if nothing actually changed.
        self.begin_interaction();
        let mut removed = 0;
        if let Some(shape) = self.doc.shapes.get_mut(i) {
            for (c, mut anchors) in by_contour {
                anchors.sort_unstable();
                anchors.dedup();
                for a in anchors.into_iter().rev() {
                    if shape.delete_anchor_in(c, a) {
                        removed += 1;
                    }
                }
            }
        }
        self.inter.ds_anchors.clear();
        self.commit_interaction();
        if removed > 0 {
            self.status = format!(
                "Deleted {removed} {}",
                if removed == 1 { "anchor" } else { "anchors" }
            );
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
                    Shape::Ellipse {
                        rect,
                        fill: self.fill,
                        fill_gradient: self.fill_gradient.clone(),
                        stroke: self.stroke,
                        stroke_w: self.stroke_w,
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
                })
            }
            // Polygon / Star are drawn from the **centre** outward: the press
            // point `a` is the centre and the drag distance to `b` is the outer
            // radius. The parameters (sides / points / inner ratio) come from the
            // remembered app defaults and are then editable in the inspector.
            Tool::Polygon | Tool::Star => {
                let radius = ((b.0 - a.0).powi(2) + (b.1 - a.1).powi(2)).sqrt();
                if radius < 1.0 {
                    return None;
                }
                let live = if self.tool == Tool::Polygon {
                    LiveShape::Polygon {
                        sides: self.poly_sides,
                        radius,
                    }
                } else {
                    LiveShape::Star {
                        points: self.star_points,
                        radius,
                        inner_ratio: self.star_ratio,
                    }
                };
                Some(self.live_shape_at(a, live))
            }
            _ => None,
        }
    }

    /// Build a closed [`Shape::Path`] for a freshly-created live shape (`live`)
    /// centred at `center`, generating its outline and inheriting the current
    /// paint defaults — the live-shape analogue of the `Rect`/`Ellipse` arms of
    /// [`shape_from_drag`](Self::shape_from_drag).
    fn live_shape_at(&self, center: (f32, f32), live: LiveShape) -> Shape {
        let (points, handles) = live.outline(center);
        Shape::Path {
            points,
            closed: true,
            fill: self.fill,
            fill_gradient: self.fill_gradient.clone(),
            stroke: self.stroke,
            stroke_w: self.stroke_w,
            stroke_style: self.stroke_style.clone(),
            handles,
            live: Some(live),
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

    /// Paint the Direct-Select overlay: every anchor of the primary editable
    /// shape (selected vs unselected), plus tangent handle lines + knobs for each
    /// selected anchor. No-op when the primary isn't an editable shape.
    fn paint_direct_select(&self, painter: &egui::Painter) {
        let Some(i) = self.ds_target() else { return };
        let Some(shape) = self.doc.shapes.get(i) else {
            return;
        };
        for c in 0..shape.contour_count() {
            let Some((points, handles, _)) = shape.contour(c) else {
                continue;
            };
            // Which anchors of this contour are selected (their handles show).
            let selected: Vec<usize> = self
                .inter
                .ds_anchors
                .iter()
                .filter(|r| r.contour == c)
                .map(|r| r.anchor)
                .collect();
            canvas::paint_direct_select(&self.view, painter, points, handles, &selected);
        }
    }

    /// Draw the active text-edit affordance: a baseline caret at the editing text
    /// object's origin (so an empty / freshly-placed text object is visible), and
    /// keep the canvas repainting so typed characters appear immediately.
    fn draw_text_edit_overlay(&self, painter: &egui::Painter, ctx: &egui::Context) {
        let Some(idx) = self.editing_text else { return };
        let Some(Shape::Text {
            origin, params, ..
        }) = self.doc.shapes.get(idx)
        else {
            return;
        };
        // A short vertical insertion caret just below the origin (top-left of the
        // first em box), scaled by font size so it reads at any zoom.
        let h = params.font_size * 0.9;
        let top = self.view.doc_to_screen((origin.0, origin.1 + params.font_size * 0.15));
        let bot = self
            .view
            .doc_to_screen((origin.0, origin.1 + params.font_size * 0.15 + h));
        // Blink at ~1.5 Hz using the egui clock.
        let on = (ctx.input(|i| i.time) * 1.5).fract() < 0.6;
        if on {
            painter.line_segment([top, bot], egui::Stroke::new(1.5, crate::theme::accent()));
        }
        // Live edit needs continuous repaints for the blink + immediate glyphs.
        ctx.request_repaint();
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

/// Distance from `(px, py)` to the chord `a..b` (clamped to the segment). Used to
/// rank candidate segments when adding an anchor across a compound path's
/// sub-contours.
fn point_seg_dist(px: f32, py: f32, a: (f32, f32), b: (f32, f32)) -> f32 {
    let (ax, ay) = a;
    let (bx, by) = b;
    let dx = bx - ax;
    let dy = by - ay;
    let len2 = dx * dx + dy * dy;
    if len2 <= 1e-9 {
        return (px - ax).hypot(py - ay);
    }
    let t = (((px - ax) * dx + (py - ay) * dy) / len2).clamp(0.0, 1.0);
    let cx = ax + t * dx;
    let cy = ay + t * dy;
    (px - cx).hypot(py - cy)
}
