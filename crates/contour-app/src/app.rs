//! The Contour application: tool state, panels, menus, and the per-frame draw
//! loop that ties the document model to the canvas.

use crate::align::{self, Align, AlignTo, Distribute};
use crate::boolean::{self, BoolOp};
use crate::canvas::{self, View};
use crate::document::{self, Document, LineCap, LineJoin, Shape, StrokeStyle};
use crate::history::History;
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
            stroke: [0.10, 0.12, 0.15, 1.0],
            stroke_w: 2.0,
            stroke_style: StrokeStyle::default(),
            artboard: Size::new(1000, 700),
            align_to: AlignTo::Selection,
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
        // Cmd/Ctrl+Z undoes, Cmd/Ctrl+Shift+Z (or Ctrl+Y) redoes.
        let (enter, delete, undo, redo) = ctx.input(|i| {
            let cmd = i.modifiers.command;
            let shift = i.modifiers.shift;
            let z = i.key_pressed(egui::Key::Z);
            let y = i.key_pressed(egui::Key::Y);
            (
                i.key_pressed(egui::Key::Enter),
                i.key_pressed(egui::Key::Delete) || i.key_pressed(egui::Key::Backspace),
                cmd && z && !shift,
                (cmd && z && shift) || (cmd && y),
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

                color_row(ui, "Fill", &mut self.fill);
                color_row(ui, "Stroke", &mut self.stroke);
                ui.horizontal(|ui| {
                    ui.label("Width");
                    ui.add(egui::Slider::new(&mut self.stroke_w, 0.0..=40.0).suffix(" px"));
                });

                self.stroke_section(ui);
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

            self.handle_input(&response, &ctx);

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
        });
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
                    self.inter.move_last = None;
                }
            }
        }

        if response.dragged() {
            if let (Some(edit), Some((x, y)), Some(i)) =
                (self.inter.path_edit, doc_pos, self.primary())
            {
                self.drag_path_edit(i, edit, x, y);
            } else if let (Some((x, y)), Some((lx, ly))) = (doc_pos, self.inter.move_last) {
                // Move every selected shape by the same delta.
                let (dx, dy) = (x - lx, y - ly);
                let n = self.doc.shapes.len();
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
            self.inter.move_last = None;
            self.inter.path_edit = None;
            // Finalize a coalesced move / anchor-edit (no-op drags are dropped).
            self.commit_interaction();
        }

        if response.clicked() {
            if let Some((x, y)) = doc_pos {
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
        if response.drag_started() {
            self.inter.drag_start = doc_pos;
            self.inter.drag_now = doc_pos;
        }
        if response.dragged() {
            self.inter.drag_now = doc_pos;
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
                        stroke: self.stroke,
                        stroke_w: self.stroke_w,
                        stroke_style: self.stroke_style.clone(),
                        visible: true,
                    }
                } else {
                    Shape::Ellipse {
                        rect,
                        fill: self.fill,
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
