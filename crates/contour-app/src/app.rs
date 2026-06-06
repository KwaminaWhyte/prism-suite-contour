//! The Contour application: tool state, panels, menus, and the per-frame draw
//! loop that ties the document model to the canvas.

use crate::align::{self, Align, AlignTo, Distribute};
use crate::arrange::{self, Arrange};
use crate::boolean::{self, BoolOp};
use crate::canvas::{self, View};
use crate::document::{self, Document, Guide, LineCap, LineJoin, Shape, StrokeStyle};
use crate::gradient::{Gradient, GradientKind, GradientStop, SpreadMode};
use crate::history::History;
use crate::snap::{self, SnapConfig, SnapFeatures, SnapResult, SnapTargets};
use crate::transform::{self, Affine, Handle};
use crate::{export, icons, theme};
use egui::{Color32, Sense, Vec2};
use prism_core::Size;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Tool {
    Select,
    Rect,
    Ellipse,
    Line,
    Pen,
}

impl Tool {
    fn icon(self) -> &'static str {
        match self {
            Tool::Select => icons::SELECT,
            Tool::Rect => icons::RECT,
            Tool::Ellipse => icons::ELLIPSE,
            Tool::Line => icons::LINE,
            Tool::Pen => icons::PEN,
        }
    }
    fn name(self) -> &'static str {
        match self {
            Tool::Select => "Select",
            Tool::Rect => "Rectangle",
            Tool::Ellipse => "Ellipse",
            Tool::Line => "Line",
            Tool::Pen => "Pen",
        }
    }
}

/// While building a pen path, which part of the freshest anchor is being
/// dragged to set its tangent handle.
#[derive(Clone, Copy, PartialEq, Eq)]
enum PenDrag {
    /// Setting the out-handle of the anchor at the given index.
    Handle(usize),
}

/// On a selected path, which editable element is being dragged.
#[derive(Clone, Copy, PartialEq, Eq)]
enum PathEdit {
    /// Dragging anchor `i` (moves the point).
    Anchor(usize),
    /// Dragging the out-handle of anchor `i` (the in-handle mirrors).
    Handle(usize),
}

/// An in-progress free-transform on the selection: which gesture, the pivot it
/// turns around, the cursor where the drag began, and a snapshot of the
/// selected shapes (index + original geometry) so each frame transforms from the
/// pristine start rather than accumulating float error.
struct TransformDrag {
    kind: TransformKind,
    /// Document-space pivot kept fixed (opposite handle for scale; box centre
    /// for rotate).
    pivot: (f32, f32),
    /// Document-space cursor at drag start.
    start: (f32, f32),
    /// (shape index, pristine shape) snapshot taken at drag start.
    snapshot: Vec<(usize, Shape)>,
}

/// Which free-transform gesture a [`TransformDrag`] performs.
#[derive(Clone, Copy, PartialEq, Eq)]
enum TransformKind {
    /// Scale by dragging the given box handle (pivot = opposite handle).
    Scale(Handle),
    /// Rotate about the box centre.
    Rotate,
}

/// In-progress interaction state (drag-to-create, pen point list, dragging).
#[derive(Default)]
struct Interaction {
    /// Document-space anchor where a create-drag began.
    drag_start: Option<(f32, f32)>,
    /// Current document-space point of an in-progress create-drag.
    drag_now: Option<(f32, f32)>,
    /// Anchor points of the path being built with the pen tool.
    pen_points: Vec<(f32, f32)>,
    /// Per-anchor out-tangent handle offsets for the in-progress pen path.
    pen_handles: Vec<(f32, f32)>,
    /// Active handle-drag on the in-progress pen path (after click-press).
    pen_drag: Option<PenDrag>,
    /// When moving a selected shape: last cursor position in document space.
    move_last: Option<(f32, f32)>,
    /// Active edit on the currently selected path (anchor/handle drag).
    path_edit: Option<PathEdit>,
    /// Active free-transform of the selection (scale/rotate via the box handles).
    transform: Option<TransformDrag>,
    /// A guide being dragged out of (or moved along) a ruler.
    guide_drag: Option<GuideDrag>,
    /// Snap lines that fired on the latest drag frame, drawn as smart guides.
    snap_lines: SnapResult,
    /// An in-progress rubber-band (marquee) selection: `(anchor, current)` in
    /// document space. Began on empty canvas with the Select tool.
    marquee: Option<((f32, f32), (f32, f32))>,
    /// Selection set captured when a shift-marquee began, so the marquee is
    /// additive (toggling intersected shapes against this base).
    marquee_base: Vec<usize>,
}

/// A ruler guide being created or moved: which orientation, and whether it is a
/// brand-new pull (so dropping it back on the ruler cancels) or an existing
/// guide at `existing` being repositioned.
struct GuideDrag {
    vertical: bool,
    /// Index into `doc.guides` of the guide being moved, or `None` while pulling
    /// a fresh one from the ruler.
    existing: Option<usize>,
}

pub struct ContourApp {
    doc: Document,
    view: View,
    tool: Tool,
    /// Multi-selection set of shape indices, in click order. The **last** entry
    /// is the *primary* (active) shape: it drives the inspector and direct-select
    /// path editing. Shift-click toggles membership; a plain click selects one.
    /// Two-operand boolean ops use the two most-recently-added members.
    selection: Vec<usize>,
    fill: [f32; 4],
    /// Current gradient fill applied to new shapes (and to the selected shape via
    /// the inspector's Fill section). `None` = a solid `fill`.
    fill_gradient: Option<Gradient>,
    stroke: [f32; 4],
    stroke_w: f32,
    /// Current stroke attributes (caps/joins/dashes) applied to new shapes and
    /// to the selected shape via the inspector's Stroke section.
    stroke_style: StrokeStyle,
    /// Logical artboard size (document units); from the shared `Size` type.
    artboard: Size,
    /// Reference frame the Align operations measure against (selection bounds or
    /// the artboard rectangle).
    align_to: AlignTo,
    /// Angle (degrees) for the inspector's numeric "Rotate by" control.
    transform_angle: f32,
    /// Which snapping sources (grid / guides / objects) are active + grid size.
    snap: SnapConfig,
    /// Whether to paint the document grid.
    show_grid: bool,
    /// Whether to paint the ruler strips (and allow pulling guides from them).
    show_rulers: bool,
    /// Whether ruler guides are drawn (independent of whether they snap).
    show_guides: bool,
    inter: Interaction,
    /// Undo / redo snapshot stack over the whole document.
    history: History,
    /// Transient status line shown in the menu bar (e.g. export results).
    status: String,
}

impl ContourApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        theme::apply(&cc.egui_ctx);
        icons::install(&cc.egui_ctx);
        Self {
            doc: Document::new(),
            view: View::default(),
            tool: Tool::Select,
            selection: Vec::new(),
            fill: [0.27, 0.55, 0.85, 1.0],
            fill_gradient: None,
            stroke: [0.10, 0.12, 0.15, 1.0],
            stroke_w: 2.0,
            stroke_style: StrokeStyle::default(),
            artboard: Size::new(1000, 700),
            align_to: AlignTo::Selection,
            transform_angle: 45.0,
            snap: SnapConfig::default(),
            show_grid: false,
            show_rulers: true,
            show_guides: true,
            inter: Interaction::default(),
            history: History::new(),
            status: String::new(),
        }
    }

    fn new_document(&mut self) {
        self.doc = Document::new();
        self.selection.clear();
        self.inter = Interaction::default();
        self.history.clear();
        self.status.clear();
    }

    // --- Selection helpers ---------------------------------------------------

    /// The primary (active) shape index — the last one added to the selection.
    fn primary(&self) -> Option<usize> {
        self.selection.last().copied()
    }

    /// Whether shape `i` is in the selection set.
    fn is_selected(&self, i: usize) -> bool {
        self.selection.contains(&i)
    }

    /// Replace the selection with a single shape (or clear it when `None`).
    fn select_only(&mut self, i: Option<usize>) {
        self.selection.clear();
        if let Some(i) = i {
            self.selection.push(i);
        }
    }

    /// Toggle shape `i` in the selection (shift-click). Re-adding moves it to the
    /// end so it becomes primary.
    fn toggle_selection(&mut self, i: usize) {
        if let Some(pos) = self.selection.iter().position(|&s| s == i) {
            self.selection.remove(pos);
        } else {
            self.selection.push(i);
        }
    }

    // --- Undo / redo ---------------------------------------------------------

    /// Record the current document as an undo checkpoint *before* applying a
    /// discrete (non-drag) edit. Call this immediately prior to mutating
    /// `self.doc`.
    fn checkpoint(&mut self) {
        self.history.push(self.doc.clone());
    }

    /// Snapshot the start of a continuous interaction (drag). Idempotent within
    /// a drag, so per-frame calls coalesce into one undo entry.
    fn begin_interaction(&mut self) {
        self.history.begin(&self.doc);
    }

    /// Finalize a continuous interaction; drops the checkpoint if nothing
    /// actually changed.
    fn commit_interaction(&mut self) {
        self.history.commit(&self.doc);
    }

    fn undo(&mut self) {
        if let Some(prev) = self.history.undo(&self.doc) {
            self.doc = prev;
            self.clamp_selection();
            self.status = "Undo".into();
        }
    }

    fn redo(&mut self) {
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
    fn remove_shape(&mut self, i: usize) {
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

    fn delete_selected(&mut self) {
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
    fn swap_shapes(&mut self, a: usize, b: usize) {
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
    fn arrange_selection(&mut self, op: Arrange) {
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

    fn open_dialog(&mut self) {
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

    fn save_dialog(&self) {
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

    fn commit_pen(&mut self, closed: bool) {
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
            });
            self.select_only(Some(self.doc.shapes.len() - 1));
        } else {
            self.inter.pen_points.clear();
            self.inter.pen_handles.clear();
        }
        self.inter.pen_drag = None;
    }

    fn export_svg_dialog(&mut self) {
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

    fn export_png_dialog(&mut self) {
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
    fn apply_bool(&mut self, op: BoolOp) {
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
    fn align_selection(&mut self, op: Align) {
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
    fn selection_bbox(&self) -> Option<[f32; 4]> {
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
    fn rotate_selection(&mut self, radians: f32, label: &str) {
        let Some(b) = self.selection_bbox() else {
            return;
        };
        let (cx, cy) = (b[0] + b[2] * 0.5, b[1] + b[3] * 0.5);
        self.transform_selection(&Affine::rotate_about(radians, cx, cy), label);
    }

    /// Mirror the selection across its bounding-box centre, horizontally
    /// (`horizontal = true`) or vertically, as one undo step.
    fn flip_selection(&mut self, horizontal: bool) {
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

    /// Whether the on-canvas transform box should be shown: the Select tool, a
    /// non-empty selection with geometry, and no active per-path anchor edit
    /// (which has its own handles). The box is suppressed while *directly*
    /// editing a single path's anchors so the two handle sets don't clash.
    fn show_transform_box(&self) -> bool {
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
    fn hit_transform_handle(&self, x: f32, y: f32) -> Option<TransformKind> {
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

    /// Begin a free-transform: snapshot the selected shapes and the pivot.
    fn begin_transform(&mut self, kind: TransformKind, x: f32, y: f32) {
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
    fn drag_transform(&mut self, x: f32, y: f32, uniform: bool) {
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
    fn snap_tol(&self) -> f32 {
        6.0 / self.view.zoom
    }

    /// Gather the candidate snap-target coordinates from the active sources,
    /// excluding the shapes in `exclude` (the ones being dragged, so a shape
    /// never snaps to itself). Grid lines are added per-feature by the caller via
    /// [`snap::grid_targets_near`], so only guides and objects are collected here.
    fn snap_targets(&self, exclude: &[usize]) -> SnapTargets {
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
    fn snap_box(&self, bbox: &[f32; 4], exclude: &[usize]) -> SnapResult {
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
    fn snap_point(&self, x: f32, y: f32, exclude: &[usize]) -> (f32, f32) {
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
    fn distribute_selection(&mut self, op: Distribute) {
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

impl eframe::App for ContourApp {
    fn ui(&mut self, root: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = root.ctx().clone();

        // Global keyboard: Enter commits a pen path; Delete removes selection;
        // Cmd/Ctrl+Z undoes, Cmd/Ctrl+Shift+Z (or Ctrl+Y) redoes; Cmd/Ctrl+]/[
        // arrange the selection (with Shift: to front / to back), à la Illustrator.
        let (enter, delete, undo, redo, arrange_key) = ctx.input(|i| {
            let cmd = i.modifiers.command;
            let shift = i.modifiers.shift;
            let z = i.key_pressed(egui::Key::Z);
            let y = i.key_pressed(egui::Key::Y);
            let arrange = if cmd && i.key_pressed(egui::Key::CloseBracket) {
                Some(if shift {
                    Arrange::BringToFront
                } else {
                    Arrange::BringForward
                })
            } else if cmd && i.key_pressed(egui::Key::OpenBracket) {
                Some(if shift {
                    Arrange::SendToBack
                } else {
                    Arrange::SendBackward
                })
            } else {
                None
            };
            (
                i.key_pressed(egui::Key::Enter),
                i.key_pressed(egui::Key::Delete) || i.key_pressed(egui::Key::Backspace),
                cmd && z && !shift,
                (cmd && z && shift) || (cmd && y),
                arrange,
            )
        });
        if enter && self.tool == Tool::Pen {
            self.commit_pen(true);
        }
        // Redo before undo so a Shift+Z frame can't be misread as undo.
        if redo {
            self.redo();
        } else if undo {
            self.undo();
        }
        if delete {
            self.delete_selected();
        }
        if let Some(op) = arrange_key {
            self.arrange_selection(op);
        }

        self.menu_bar(root);
        self.tool_palette(root);
        self.right_panel(root);
        self.central_canvas(root);
    }
}

impl ContourApp {
    fn menu_bar(&mut self, root: &mut egui::Ui) {
        egui::TopBottomPanel::top("menu_bar").show_inside(root, |ui| {
            egui::menu::bar(ui, |ui| {
                ui.menu_button("File", |ui| {
                    if ui.button("New").clicked() {
                        self.new_document();
                        ui.close_menu();
                    }
                    if ui.button("Open .contour…").clicked() {
                        self.open_dialog();
                        ui.close_menu();
                    }
                    if ui.button("Save .contour…").clicked() {
                        self.save_dialog();
                        ui.close_menu();
                    }
                    ui.separator();
                    if ui.button("Export SVG…").clicked() {
                        self.export_svg_dialog();
                        ui.close_menu();
                    }
                    if ui.button("Export PNG…").clicked() {
                        self.export_png_dialog();
                        ui.close_menu();
                    }
                });
                ui.menu_button("Edit", |ui| {
                    ui.add_enabled_ui(self.history.can_undo(), |ui| {
                        if ui.button(format!("{}  Undo", icons::UNDO)).clicked() {
                            self.undo();
                            ui.close_menu();
                        }
                    });
                    ui.add_enabled_ui(self.history.can_redo(), |ui| {
                        if ui.button(format!("{}  Redo", icons::REDO)).clicked() {
                            self.redo();
                            ui.close_menu();
                        }
                    });
                    ui.separator();
                    ui.add_enabled_ui(!self.selection.is_empty(), |ui| {
                        if ui.button("Delete").clicked() {
                            self.delete_selected();
                            ui.close_menu();
                        }
                    });
                });
                ui.menu_button("Object", |ui| {
                    let two = self.selection.len() == 2;
                    ui.add_enabled_ui(two, |ui| {
                        if ui.button(format!("{}  Union", icons::UNITE)).clicked() {
                            self.apply_bool(BoolOp::Union);
                            ui.close_menu();
                        }
                        if ui
                            .button(format!("{}  Intersect", icons::INTERSECT))
                            .clicked()
                        {
                            self.apply_bool(BoolOp::Intersect);
                            ui.close_menu();
                        }
                        if ui
                            .button(format!("{}  Difference", icons::EXCLUDE))
                            .clicked()
                        {
                            self.apply_bool(BoolOp::Difference);
                            ui.close_menu();
                        }
                    });
                    if !two {
                        ui.weak("Boolean: select exactly 2");
                    }

                    ui.separator();
                    ui.menu_button("Align", |ui| {
                        let can_align =
                            self.selection.len() >= 2 || self.align_to == AlignTo::Artboard;
                        ui.add_enabled_ui(can_align, |ui| {
                            for (icon, label, op) in [
                                (icons::ALIGN_LEFT, "Align Left", Align::Left),
                                (icons::ALIGN_CENTER_H, "Align Center H", Align::CenterH),
                                (icons::ALIGN_RIGHT, "Align Right", Align::Right),
                                (icons::ALIGN_TOP, "Align Top", Align::Top),
                                (icons::ALIGN_CENTER_V, "Align Center V", Align::CenterV),
                                (icons::ALIGN_BOTTOM, "Align Bottom", Align::Bottom),
                            ] {
                                if ui.button(format!("{icon}  {label}")).clicked() {
                                    self.align_selection(op);
                                    ui.close_menu();
                                }
                            }
                        });
                    });
                    ui.menu_button("Distribute", |ui| {
                        ui.add_enabled_ui(self.selection.len() >= 3, |ui| {
                            for (icon, label, op) in [
                                (icons::DISTRIBUTE_H, "Left Edges", Distribute::LeftEdges),
                                (
                                    icons::DISTRIBUTE_H,
                                    "Horizontal Centers",
                                    Distribute::CentersH,
                                ),
                                (icons::DISTRIBUTE_H, "Right Edges", Distribute::RightEdges),
                                (
                                    icons::DISTRIBUTE_H,
                                    "Horizontal Gaps",
                                    Distribute::HorizontalGap,
                                ),
                            ] {
                                if ui.button(format!("{icon}  {label}")).clicked() {
                                    self.distribute_selection(op);
                                    ui.close_menu();
                                }
                            }
                            ui.separator();
                            for (icon, label, op) in [
                                (icons::DISTRIBUTE_V, "Top Edges", Distribute::TopEdges),
                                (
                                    icons::DISTRIBUTE_V,
                                    "Vertical Centers",
                                    Distribute::CentersV,
                                ),
                                (icons::DISTRIBUTE_V, "Bottom Edges", Distribute::BottomEdges),
                                (
                                    icons::DISTRIBUTE_V,
                                    "Vertical Gaps",
                                    Distribute::VerticalGap,
                                ),
                            ] {
                                if ui.button(format!("{icon}  {label}")).clicked() {
                                    self.distribute_selection(op);
                                    ui.close_menu();
                                }
                            }
                        });
                    });

                    ui.separator();
                    ui.menu_button("Transform", |ui| {
                        ui.add_enabled_ui(!self.selection.is_empty(), |ui| {
                            use std::f32::consts::PI;
                            if ui
                                .button(format!("{}  Rotate 90° CW", icons::ROTATE_CW))
                                .clicked()
                            {
                                self.rotate_selection(PI * 0.5, "Rotated 90° CW");
                                ui.close_menu();
                            }
                            if ui
                                .button(format!("{}  Rotate 90° CCW", icons::ROTATE_CCW))
                                .clicked()
                            {
                                self.rotate_selection(-PI * 0.5, "Rotated 90° CCW");
                                ui.close_menu();
                            }
                            if ui
                                .button(format!("{}  Rotate 180°", icons::ROTATE_CW))
                                .clicked()
                            {
                                self.rotate_selection(PI, "Rotated 180°");
                                ui.close_menu();
                            }
                            ui.separator();
                            if ui
                                .button(format!("{}  Flip Horizontal", icons::FLIP_H))
                                .clicked()
                            {
                                self.flip_selection(true);
                                ui.close_menu();
                            }
                            if ui
                                .button(format!("{}  Flip Vertical", icons::FLIP_V))
                                .clicked()
                            {
                                self.flip_selection(false);
                                ui.close_menu();
                            }
                        });
                    });

                    ui.separator();
                    ui.menu_button(format!("{}  Arrange", icons::ARRANGE), |ui| {
                        ui.add_enabled_ui(!self.selection.is_empty(), |ui| {
                            for (icon, op) in [
                                (icons::BRING_TO_FRONT, Arrange::BringToFront),
                                (icons::BRING_FORWARD, Arrange::BringForward),
                                (icons::SEND_BACKWARD, Arrange::SendBackward),
                                (icons::SEND_TO_BACK, Arrange::SendToBack),
                            ] {
                                if ui.button(format!("{icon}  {}", op.label())).clicked() {
                                    self.arrange_selection(op);
                                    ui.close_menu();
                                }
                            }
                        });
                    });
                });
                ui.menu_button("View", |ui| {
                    ui.checkbox(&mut self.show_rulers, "Rulers");
                    ui.checkbox(&mut self.show_grid, "Grid");
                    ui.checkbox(&mut self.show_guides, "Guides");
                    ui.separator();
                    ui.label(egui::RichText::new("Snap to").weak());
                    ui.checkbox(&mut self.snap.to_grid, "Grid");
                    ui.checkbox(&mut self.snap.to_guides, "Guides");
                    ui.checkbox(&mut self.snap.to_objects, "Objects");
                    ui.separator();
                    ui.horizontal(|ui| {
                        ui.label("Grid size");
                        ui.add(
                            egui::DragValue::new(&mut self.snap.grid)
                                .speed(1.0)
                                .range(2.0..=500.0)
                                .suffix(" px"),
                        );
                    });
                    ui.add_enabled_ui(!self.doc.guides.is_empty(), |ui| {
                        if ui.button("Clear guides").clicked() {
                            self.checkpoint();
                            self.doc.guides.clear();
                            ui.close_menu();
                        }
                    });
                });
                ui.separator();
                ui.label(egui::RichText::new("Contour").strong());
                ui.weak("vector editor · Prism");
                if !self.status.is_empty() {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.weak(&self.status);
                    });
                }
            });
        });
    }

    fn tool_palette(&mut self, root: &mut egui::Ui) {
        egui::SidePanel::left("tools")
            .exact_width(56.0)
            .resizable(false)
            .show_inside(root, |ui| {
                ui.add_space(6.0);
                ui.vertical_centered(|ui| {
                    for tool in [
                        Tool::Select,
                        Tool::Rect,
                        Tool::Ellipse,
                        Tool::Line,
                        Tool::Pen,
                    ] {
                        let selected = self.tool == tool;
                        let btn = egui::Button::new(egui::RichText::new(tool.icon()).size(20.0))
                            .min_size(Vec2::new(40.0, 40.0))
                            .selected(selected);
                        if ui.add(btn).on_hover_text(tool.name()).clicked() {
                            // Switching away from pen mid-path commits it.
                            if self.tool == Tool::Pen && tool != Tool::Pen {
                                self.commit_pen(false);
                            }
                            self.tool = tool;
                        }
                        ui.add_space(4.0);
                    }
                });
            });
    }

    fn right_panel(&mut self, root: &mut egui::Ui) {
        egui::SidePanel::right("inspector")
            .default_width(248.0)
            .show_inside(root, |ui| {
                ui.add_space(4.0);
                ui.heading("Style");
                ui.add_space(4.0);

                self.fill_section(ui);
                color_row(ui, "Stroke", &mut self.stroke);
                ui.horizontal(|ui| {
                    ui.label("Width");
                    ui.add(egui::Slider::new(&mut self.stroke_w, 0.0..=40.0).suffix(" px"));
                });

                self.stroke_section(ui);
                self.transform_section(ui);
                self.arrange_section(ui);
                self.align_section(ui);

                // Direct-select hint when a path is the active selection.
                if self.tool == Tool::Select && self.selected_is_path() {
                    ui.separator();
                    ui.label(egui::RichText::new("Edit path").strong());
                    ui.weak("Drag an anchor or handle to reshape.");
                    ui.weak("Dbl-click a segment to add an anchor.");
                    ui.weak("Dbl-click an anchor to delete it.");
                    ui.weak("Alt-click an anchor: smooth ⇄ corner.");
                }

                ui.separator();
                self.layers_panel(ui);
            });
    }

    /// Fill controls: a solid colour or a multi-stop gradient (linear / radial).
    /// Like the stroke section, the controls edit the primary selected shape's
    /// fill (one undo step per discrete change) and the app default tracks along
    /// so the next new shape inherits it; with no selection only the app default
    /// is edited.
    fn fill_section(&mut self, ui: &mut egui::Ui) {
        // Seed the working state from the primary selected shape (so the panel
        // reflects the selection), falling back to the app default.
        let primary = self.primary();
        let seeded = primary.and_then(|i| self.doc.shapes.get(i));
        let mut solid = match seeded.and_then(|s| s.fill_color()) {
            Some(c) => c,
            None => self.fill,
        };
        let mut grad: Option<Gradient> = match seeded {
            Some(s) => s.fill_gradient().cloned(),
            None => self.fill_gradient.clone(),
        };

        let mut changed = false;

        ui.horizontal(|ui| {
            ui.label("Fill");
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                // Solid / Gradient toggle.
                let mut is_grad = grad.is_some();
                if ui.selectable_label(!is_grad, "Solid").clicked() && is_grad {
                    grad = None;
                    is_grad = false;
                    changed = true;
                }
                if ui.selectable_label(is_grad, "Gradient").clicked() && !is_grad {
                    // Start a gradient from the current solid colour to white,
                    // unless the shape already had one to restore.
                    grad = Some(Gradient::two_stop(
                        GradientKind::Linear,
                        solid,
                        [1.0, 1.0, 1.0, 1.0],
                    ));
                    changed = true;
                }
            });
        });

        match &mut grad {
            None => {
                // Solid colour swatch.
                let mut c = Color32::from_rgba_unmultiplied(
                    (solid[0] * 255.0) as u8,
                    (solid[1] * 255.0) as u8,
                    (solid[2] * 255.0) as u8,
                    (solid[3] * 255.0) as u8,
                );
                if ui.color_edit_button_srgba(&mut c).changed() {
                    solid = [
                        c.r() as f32 / 255.0,
                        c.g() as f32 / 255.0,
                        c.b() as f32 / 255.0,
                        c.a() as f32 / 255.0,
                    ];
                    changed = true;
                }
            }
            Some(g) => {
                changed |= gradient_editor(ui, g);
            }
        }

        if changed {
            // Update the app defaults so the next new shape inherits the fill.
            self.fill = solid;
            self.fill_gradient = grad.clone();
            // Apply to the selected shape as a single undo step.
            if let Some(i) = primary {
                self.checkpoint();
                if let Some(shape) = self.doc.shapes.get_mut(i) {
                    shape.set_fill_color(solid);
                    shape.set_fill_gradient(grad);
                }
            }
        }
    }

    /// Stroke options: caps, joins, miter limit, and a dash pattern. When a
    /// shape is selected the controls edit *its* stroke style (one undo step per
    /// discrete change) and the app default tracks along, so the next new shape
    /// inherits it; with no selection the controls edit the app default only.
    fn stroke_section(&mut self, ui: &mut egui::Ui) {
        ui.separator();
        ui.label(egui::RichText::new("Stroke options").strong());

        // Edit a working copy seeded from the primary selected shape (so the
        // panel reflects what's selected), falling back to the app default.
        let mut s = match self.primary().and_then(|i| self.doc.shapes.get(i)) {
            Some(shape) => shape.stroke_style().clone(),
            None => self.stroke_style.clone(),
        };
        let mut changed = false;
        let s = &mut s;

        // Cap.
        ui.horizontal(|ui| {
            ui.label("Cap");
            egui::ComboBox::from_id_salt("cap")
                .selected_text(s.cap.label())
                .show_ui(ui, |ui| {
                    for cap in LineCap::ALL {
                        if ui.selectable_value(&mut s.cap, cap, cap.label()).changed() {
                            changed = true;
                        }
                    }
                });
        });

        // Join (+ miter limit when miter is selected).
        ui.horizontal(|ui| {
            ui.label("Join");
            egui::ComboBox::from_id_salt("join")
                .selected_text(s.join.label())
                .show_ui(ui, |ui| {
                    for join in LineJoin::ALL {
                        if ui
                            .selectable_value(&mut s.join, join, join.label())
                            .changed()
                        {
                            changed = true;
                        }
                    }
                });
        });
        if s.join == LineJoin::Miter {
            ui.horizontal(|ui| {
                ui.label("Miter");
                if ui
                    .add(egui::Slider::new(&mut s.miter_limit, 1.0..=20.0))
                    .changed()
                {
                    changed = true;
                }
            });
        }

        // Dash preset buttons + offset.
        ui.horizontal(|ui| {
            ui.label("Dashes");
            // (label, pattern) — empty pattern == solid.
            let presets: [(&str, &[f32]); 4] = [
                ("Solid", &[]),
                ("Dashed", &[12.0, 6.0]),
                ("Dotted", &[2.0, 4.0]),
                ("Dash-dot", &[12.0, 4.0, 2.0, 4.0]),
            ];
            for (label, pat) in presets {
                let active = s.dash.as_slice() == pat;
                if ui.selectable_label(active, label).clicked() && !active {
                    s.dash = pat.to_vec();
                    changed = true;
                }
            }
        });
        if s.is_dashed() {
            ui.horizontal(|ui| {
                ui.label("Offset");
                if ui
                    .add(egui::Slider::new(&mut s.dash_offset, 0.0..=40.0).suffix(" px"))
                    .changed()
                {
                    changed = true;
                }
            });
        }

        // Commit the working copy: always update the app default (so new shapes
        // inherit it), and push onto the selected shape as one undo step.
        if changed {
            let style = s.clone();
            self.stroke_style = style.clone();
            if let Some(i) = self.primary() {
                self.checkpoint();
                if let Some(shape) = self.doc.shapes.get_mut(i) {
                    *shape.stroke_style_mut() = style;
                }
            }
        }
    }

    /// Transform controls: quick 90°/180° rotations, horizontal/vertical flips,
    /// and a numeric "rotate by" about the selection's centre. Mirrors the
    /// on-canvas transform box (drag a handle to scale, drag just outside a
    /// corner to rotate). Each action is one undo step.
    fn transform_section(&mut self, ui: &mut egui::Ui) {
        ui.separator();
        ui.label(egui::RichText::new("Transform").strong());

        let enabled = !self.selection.is_empty();
        ui.add_enabled_ui(enabled, |ui| {
            use std::f32::consts::PI;
            ui.horizontal(|ui| {
                if align_button(ui, icons::ROTATE_CCW)
                    .on_hover_text("Rotate 90° counter-clockwise")
                    .clicked()
                {
                    self.rotate_selection(-PI * 0.5, "Rotated 90° CCW");
                }
                if align_button(ui, icons::ROTATE_CW)
                    .on_hover_text("Rotate 90° clockwise")
                    .clicked()
                {
                    self.rotate_selection(PI * 0.5, "Rotated 90° CW");
                }
                ui.add_space(6.0);
                if align_button(ui, icons::FLIP_H)
                    .on_hover_text("Flip horizontal")
                    .clicked()
                {
                    self.flip_selection(true);
                }
                if align_button(ui, icons::FLIP_V)
                    .on_hover_text("Flip vertical")
                    .clicked()
                {
                    self.flip_selection(false);
                }
            });

            ui.horizontal(|ui| {
                ui.label("Rotate by");
                ui.add(
                    egui::DragValue::new(&mut self.transform_angle)
                        .speed(1.0)
                        .range(-360.0..=360.0)
                        .suffix("°"),
                );
                if ui.button("Apply").clicked() {
                    let rad = self.transform_angle.to_radians();
                    self.rotate_selection(rad, "Rotated");
                }
            });
        });

        if !enabled {
            ui.weak("Select a shape to transform.");
        }
    }

    /// Arrange (paint-order / stacking) controls: bring-to-front, forward,
    /// backward, and send-to-back. Each is a single undo step; a button is
    /// disabled when the move would not change the order (e.g. the selection is
    /// already on top). Mirrors `Object → Arrange` and the Cmd/Ctrl+]/[ keys.
    fn arrange_section(&mut self, ui: &mut egui::Ui) {
        ui.separator();
        ui.label(egui::RichText::new("Arrange").strong());

        let len = self.doc.shapes.len();
        let mut op: Option<Arrange> = None;
        ui.horizontal(|ui| {
            for (icon, tip, a) in [
                (icons::SEND_TO_BACK, "Send to back", Arrange::SendToBack),
                (icons::SEND_BACKWARD, "Send backward", Arrange::SendBackward),
                (icons::BRING_FORWARD, "Bring forward", Arrange::BringForward),
                (
                    icons::BRING_TO_FRONT,
                    "Bring to front",
                    Arrange::BringToFront,
                ),
            ] {
                let can = arrange::changes_order(len, &self.selection, a);
                ui.add_enabled_ui(can, |ui| {
                    if align_button(ui, icon).on_hover_text(tip).clicked() {
                        op = Some(a);
                    }
                });
            }
        });
        if let Some(a) = op {
            self.arrange_selection(a);
        }
        if self.selection.is_empty() {
            ui.weak("Select a shape to reorder.");
        }
    }

    /// Align & distribute controls. Align snaps the selection's edges/centres to
    /// a reference frame (selection bounds or the artboard); distribute spreads
    /// three-or-more shapes evenly. Each click is a single undo step. Disabled
    /// rows guide the user toward a usable selection size.
    fn align_section(&mut self, ui: &mut egui::Ui) {
        ui.separator();
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("Align").strong());
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                egui::ComboBox::from_id_salt("align_to")
                    .selected_text(match self.align_to {
                        AlignTo::Selection => "To selection",
                        AlignTo::Artboard => "To artboard",
                    })
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut self.align_to, AlignTo::Selection, "To selection");
                        ui.selectable_value(&mut self.align_to, AlignTo::Artboard, "To artboard");
                    });
            });
        });

        let can_align = self.selection.len() >= 2 || self.align_to == AlignTo::Artboard;
        let mut align_op: Option<Align> = None;
        ui.add_enabled_ui(can_align, |ui| {
            ui.horizontal(|ui| {
                for (icon, tip, op) in [
                    (icons::ALIGN_LEFT, "Align left edges", Align::Left),
                    (
                        icons::ALIGN_CENTER_H,
                        "Align horizontal centers",
                        Align::CenterH,
                    ),
                    (icons::ALIGN_RIGHT, "Align right edges", Align::Right),
                ] {
                    if align_button(ui, icon).on_hover_text(tip).clicked() {
                        align_op = Some(op);
                    }
                }
                ui.add_space(6.0);
                for (icon, tip, op) in [
                    (icons::ALIGN_TOP, "Align top edges", Align::Top),
                    (
                        icons::ALIGN_CENTER_V,
                        "Align vertical centers",
                        Align::CenterV,
                    ),
                    (icons::ALIGN_BOTTOM, "Align bottom edges", Align::Bottom),
                ] {
                    if align_button(ui, icon).on_hover_text(tip).clicked() {
                        align_op = Some(op);
                    }
                }
            });
        });
        if let Some(op) = align_op {
            self.align_selection(op);
        }

        let mut dist_op: Option<Distribute> = None;
        ui.add_enabled_ui(self.selection.len() >= 3, |ui| {
            ui.horizontal(|ui| {
                ui.label("Distribute");
                for (icon, tip, op) in [
                    (
                        icons::DISTRIBUTE_H,
                        "Distribute horizontal centers",
                        Distribute::CentersH,
                    ),
                    (
                        icons::DISTRIBUTE_V,
                        "Distribute vertical centers",
                        Distribute::CentersV,
                    ),
                ] {
                    if align_button(ui, icon).on_hover_text(tip).clicked() {
                        dist_op = Some(op);
                    }
                }
                if align_button(ui, icons::DISTRIBUTE_H)
                    .on_hover_text("Distribute horizontal gaps")
                    .clicked()
                {
                    dist_op = Some(Distribute::HorizontalGap);
                }
                if align_button(ui, icons::DISTRIBUTE_V)
                    .on_hover_text("Distribute vertical gaps")
                    .clicked()
                {
                    dist_op = Some(Distribute::VerticalGap);
                }
            });
        });
        if let Some(op) = dist_op {
            self.distribute_selection(op);
        }

        if !can_align {
            ui.weak("Select 2+ shapes (3+ to distribute).");
        }
    }

    /// The Layers list: newest on top, with visibility toggle, reorder up/down,
    /// delete, and click-to-select (shift-click toggles the shape in the
    /// multi-selection set).
    fn layers_panel(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.heading("Layers");
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.weak(format!("{}", self.doc.shapes.len()));
            });
        });
        ui.add_space(2.0);

        // Deferred mutations so we don't borrow `self.doc` while iterating.
        let mut to_delete: Option<usize> = None;
        let mut to_toggle: Option<usize> = None;
        let mut to_raise: Option<usize> = None; // swap with idx+1 (towards top)
        let mut to_lower: Option<usize> = None; // swap with idx-1 (towards bottom)
        let mut to_select: Option<(usize, bool)> = None; // (idx, shift held)

        let shift = ui.input(|i| i.modifiers.shift);
        let n = self.doc.shapes.len();

        egui::ScrollArea::vertical().show(ui, |ui| {
            // Paint order: index 0 painted first (bottom). "Newest on top" =>
            // iterate indices in reverse so the last (topmost) is listed first.
            for idx in (0..n).rev() {
                let primary = self.primary() == Some(idx);
                // A non-primary member of a multi-selection.
                let secondary = !primary && self.is_selected(idx);
                let visible = self.doc.shapes[idx].visible();
                let label = self.doc.shapes[idx].label();

                ui.horizontal(|ui| {
                    // Visibility toggle.
                    let eye = if visible {
                        icons::EYE
                    } else {
                        icons::EYE_SLASH
                    };
                    if ui
                        .add(egui::Button::new(eye).frame(false))
                        .on_hover_text("Toggle visibility")
                        .clicked()
                    {
                        to_toggle = Some(idx);
                    }

                    let mut text = egui::RichText::new(format!("{}  {}", n - idx, label));
                    if !visible {
                        text = text.weak();
                    }
                    if secondary {
                        text = text.color(theme::accent());
                    }
                    if ui.selectable_label(primary, text).clicked() {
                        to_select = Some((idx, shift));
                    }

                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.button(icons::TRASH).on_hover_text("Delete").clicked() {
                            to_delete = Some(idx);
                        }
                        ui.add_enabled_ui(idx > 0, |ui| {
                            if ui
                                .button(icons::CARET_DOWN)
                                .on_hover_text("Move down")
                                .clicked()
                            {
                                to_lower = Some(idx);
                            }
                        });
                        ui.add_enabled_ui(idx + 1 < n, |ui| {
                            if ui
                                .button(icons::CARET_UP)
                                .on_hover_text("Move up")
                                .clicked()
                            {
                                to_raise = Some(idx);
                            }
                        });
                    });
                });
            }
            if n == 0 {
                ui.weak("No shapes yet. Pick a tool and draw.");
            }
        });

        if let Some(i) = to_toggle {
            self.checkpoint();
            self.doc.shapes[i].toggle_visible();
        }
        if let Some(i) = to_raise {
            self.checkpoint();
            self.swap_shapes(i, i + 1);
        }
        if let Some(i) = to_lower {
            self.checkpoint();
            self.swap_shapes(i, i - 1);
        }
        if let Some((i, shift)) = to_select {
            if shift {
                self.toggle_selection(i);
            } else {
                self.select_only(Some(i));
            }
        }
        if let Some(i) = to_delete {
            self.checkpoint();
            self.remove_shape(i);
        }
    }

    fn central_canvas(&mut self, root: &mut egui::Ui) {
        egui::CentralPanel::default().show_inside(root, |ui| {
            let ctx = ui.ctx().clone();
            let (response, painter) =
                ui.allocate_painter(ui.available_size(), Sense::click_and_drag());

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

            // Grid behind the artboard.
            if self.show_grid {
                canvas::paint_grid(&painter, &self.view, content, self.snap.grid);
            }

            // Artboard + all shapes (bottom-up paint order).
            canvas::paint_artboard(
                &painter,
                &self.view,
                self.artboard.width as f32,
                self.artboard.height as f32,
            );
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
                    // Shift-click toggles the hit shape in the multi-selection.
                    if let Some(i) = hit {
                        self.toggle_selection(i);
                    }
                } else {
                    self.select_only(hit);
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
        self.selection = sel;
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
    fn selected_is_path(&self) -> bool {
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

/// A small square icon button for the align/distribute row.
fn align_button(ui: &mut egui::Ui, icon: &str) -> egui::Response {
    ui.add(egui::Button::new(egui::RichText::new(icon).size(16.0)).min_size(Vec2::new(26.0, 26.0)))
}

/// Edit a [`Gradient`] in place: kind (linear / radial), spread, linear angle,
/// quick presets, and an editable list of colour stops (add / remove / move /
/// recolour). Returns `true` if any control changed the gradient this frame.
fn gradient_editor(ui: &mut egui::Ui, g: &mut Gradient) -> bool {
    let mut changed = false;

    // Kind + spread.
    ui.horizontal(|ui| {
        egui::ComboBox::from_id_salt("grad_kind")
            .selected_text(g.kind.label())
            .show_ui(ui, |ui| {
                for k in [GradientKind::Linear, GradientKind::Radial] {
                    if ui.selectable_value(&mut g.kind, k, k.label()).changed() {
                        changed = true;
                    }
                }
            });
        egui::ComboBox::from_id_salt("grad_spread")
            .selected_text(g.spread.label())
            .show_ui(ui, |ui| {
                for m in SpreadMode::ALL {
                    if ui.selectable_value(&mut g.spread, m, m.label()).changed() {
                        changed = true;
                    }
                }
            });
    });

    // Angle (linear only).
    if g.kind == GradientKind::Linear {
        ui.horizontal(|ui| {
            ui.label("Angle");
            if ui
                .add(egui::Slider::new(&mut g.angle, 0.0..=360.0).suffix("°"))
                .changed()
            {
                changed = true;
            }
        });
    }

    // Stops: each row is offset + colour + a remove button. A stable id per row
    // keeps egui widgets distinct as stops are added/removed.
    ui.label(egui::RichText::new("Stops").weak());
    let mut remove: Option<usize> = None;
    let can_remove = g.stops.len() > 2;
    for (idx, stop) in g.stops.iter_mut().enumerate() {
        ui.horizontal(|ui| {
            if ui
                .add(
                    egui::DragValue::new(&mut stop.offset)
                        .speed(0.005)
                        .range(0.0..=1.0)
                        .fixed_decimals(2),
                )
                .changed()
            {
                changed = true;
            }
            let mut c = Color32::from_rgba_unmultiplied(
                (stop.color[0] * 255.0) as u8,
                (stop.color[1] * 255.0) as u8,
                (stop.color[2] * 255.0) as u8,
                (stop.color[3] * 255.0) as u8,
            );
            if ui.color_edit_button_srgba(&mut c).changed() {
                stop.color = [
                    c.r() as f32 / 255.0,
                    c.g() as f32 / 255.0,
                    c.b() as f32 / 255.0,
                    c.a() as f32 / 255.0,
                ];
                changed = true;
            }
            ui.add_enabled_ui(can_remove, |ui| {
                if ui.small_button("✕").on_hover_text("Remove stop").clicked() {
                    remove = Some(idx);
                }
            });
        });
    }
    if let Some(i) = remove {
        g.stops.remove(i);
        changed = true;
    }

    // Add a stop at the midpoint of the widest gap, coloured by sampling there.
    if ui.button("+ Add stop").clicked() {
        let sorted = g.sorted_stops();
        let mut best_gap = -1.0;
        let mut best_mid = 0.5;
        for w in sorted.windows(2) {
            let gap = w[1].offset - w[0].offset;
            if gap > best_gap {
                best_gap = gap;
                best_mid = (w[0].offset + w[1].offset) * 0.5;
            }
        }
        let color = g.color_at(best_mid);
        g.stops.push(GradientStop::new(best_mid, color));
        changed = true;
    }

    changed
}

fn color_row(ui: &mut egui::Ui, label: &str, rgba: &mut [f32; 4]) {
    ui.horizontal(|ui| {
        ui.label(label);
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let mut c = Color32::from_rgba_unmultiplied(
                (rgba[0] * 255.0) as u8,
                (rgba[1] * 255.0) as u8,
                (rgba[2] * 255.0) as u8,
                (rgba[3] * 255.0) as u8,
            );
            if ui.color_edit_button_srgba(&mut c).changed() {
                rgba[0] = c.r() as f32 / 255.0;
                rgba[1] = c.g() as f32 / 255.0;
                rgba[2] = c.b() as f32 / 255.0;
                rgba[3] = c.a() as f32 / 255.0;
            }
        });
    });
}
