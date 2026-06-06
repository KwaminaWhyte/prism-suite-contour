//! The Contour application: tool state, panels, menus, and the per-frame draw
//! loop that ties the document model to the canvas.

use crate::canvas::{self, View};
use crate::document::{Document, Shape};
use crate::{icons, theme};
use egui::{Color32, Pos2, Sense, Vec2};
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

/// In-progress interaction state (drag-to-create, pen point list, dragging).
#[derive(Default)]
struct Interaction {
    /// Document-space anchor where a create-drag began.
    drag_start: Option<(f32, f32)>,
    /// Current document-space point of an in-progress create-drag.
    drag_now: Option<(f32, f32)>,
    /// Points of the path being built with the pen tool.
    pen_points: Vec<(f32, f32)>,
    /// When moving a selected shape: last cursor position in document space.
    move_last: Option<(f32, f32)>,
}

pub struct ContourApp {
    doc: Document,
    view: View,
    tool: Tool,
    selected: Option<usize>,
    fill: [f32; 4],
    stroke: [f32; 4],
    stroke_w: f32,
    /// Logical artboard size (document units); from the shared `Size` type.
    artboard: Size,
    inter: Interaction,
}

impl ContourApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        theme::apply(&cc.egui_ctx);
        icons::install(&cc.egui_ctx);
        Self {
            doc: Document::new(),
            view: View::default(),
            tool: Tool::Select,
            selected: None,
            fill: [0.27, 0.55, 0.85, 1.0],
            stroke: [0.10, 0.12, 0.15, 1.0],
            stroke_w: 2.0,
            artboard: Size::new(1000, 700),
            inter: Interaction::default(),
        }
    }

    fn new_document(&mut self) {
        self.doc = Document::new();
        self.selected = None;
        self.inter = Interaction::default();
    }

    fn delete_selected(&mut self) {
        if let Some(i) = self.selected.take() {
            if i < self.doc.shapes.len() {
                self.doc.shapes.remove(i);
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
            self.doc.shapes.push(Shape::Path {
                points: std::mem::take(&mut self.inter.pen_points),
                closed,
                fill: self.fill,
                stroke: self.stroke,
                stroke_w: self.stroke_w,
            });
            self.selected = Some(self.doc.shapes.len() - 1);
        } else {
            self.inter.pen_points.clear();
        }
    }
}

impl eframe::App for ContourApp {
    fn ui(&mut self, root: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = root.ctx().clone();

        // Global keyboard: Enter commits a pen path; Delete removes selection.
        let (enter, delete) = ctx.input(|i| {
            (
                i.key_pressed(egui::Key::Enter),
                i.key_pressed(egui::Key::Delete) || i.key_pressed(egui::Key::Backspace),
            )
        });
        if enter && self.tool == Tool::Pen {
            self.commit_pen(true);
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
                    if ui.button("Save .contour…").clicked() {
                        self.save_dialog();
                        ui.close_menu();
                    }
                });
                ui.menu_button("Edit", |ui| {
                    ui.add_enabled_ui(self.selected.is_some(), |ui| {
                        if ui.button("Delete").clicked() {
                            self.delete_selected();
                            ui.close_menu();
                        }
                    });
                });
                ui.separator();
                ui.label(egui::RichText::new("Contour").strong());
                ui.weak("vector editor · Prism");
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
            .default_width(240.0)
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

                ui.separator();
                ui.heading("Shapes");
                ui.add_space(2.0);

                let mut to_delete: Option<usize> = None;
                egui::ScrollArea::vertical().show(ui, |ui| {
                    // Newest on top: iterate indices in reverse.
                    let n = self.doc.shapes.len();
                    for idx in (0..n).rev() {
                        let selected = self.selected == Some(idx);
                        ui.horizontal(|ui| {
                            let label = format!("{}  {}", n - idx, self.doc.shapes[idx].label());
                            if ui.selectable_label(selected, label).clicked() {
                                self.selected = Some(idx);
                            }
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    if ui.button(icons::TRASH).on_hover_text("Delete").clicked() {
                                        to_delete = Some(idx);
                                    }
                                },
                            );
                        });
                    }
                    if n == 0 {
                        ui.weak("No shapes yet. Pick a tool and draw.");
                    }
                });

                if let Some(i) = to_delete {
                    self.doc.shapes.remove(i);
                    self.selected = match self.selected {
                        Some(s) if s == i => None,
                        Some(s) if s > i => Some(s - 1),
                        other => other,
                    };
                }
            });
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
                canvas::paint_shape(&painter, &self.view, s, self.selected == Some(i));
            }

            self.handle_input(&response, &ctx);
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

        if response.drag_started() {
            if let Some((x, y)) = doc_pos {
                // Pick topmost shape under cursor to begin a move; else pan.
                let hit = self
                    .doc
                    .shapes
                    .iter()
                    .enumerate()
                    .rev()
                    .find(|(_, s)| s.hit(x, y, tol))
                    .map(|(i, _)| i);
                if let Some(i) = hit {
                    self.selected = Some(i);
                    self.inter.move_last = Some((x, y));
                } else {
                    self.inter.move_last = None;
                }
            }
        }

        if response.dragged() {
            if let (Some(i), Some((x, y)), Some((lx, ly))) =
                (self.selected, doc_pos, self.inter.move_last)
            {
                if i < self.doc.shapes.len() {
                    self.doc.shapes[i].translate(x - lx, y - ly);
                    self.inter.move_last = Some((x, y));
                }
            } else {
                // No shape grabbed: drag pans the canvas.
                self.view.pan += response.drag_delta();
            }
        }

        if response.drag_stopped() {
            self.inter.move_last = None;
        }

        if response.clicked() {
            if let Some((x, y)) = doc_pos {
                self.selected = self
                    .doc
                    .shapes
                    .iter()
                    .enumerate()
                    .rev()
                    .find(|(_, s)| s.hit(x, y, tol))
                    .map(|(i, _)| i);
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
                    self.doc.shapes.push(shape);
                    self.selected = Some(self.doc.shapes.len() - 1);
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
                    }
                } else {
                    Shape::Ellipse {
                        rect,
                        fill: self.fill,
                        stroke: self.stroke,
                        stroke_w: self.stroke_w,
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
                })
            }
            _ => None,
        }
    }

    fn handle_pen(&mut self, response: &egui::Response, doc_pos: Option<(f32, f32)>) {
        if response.double_clicked() {
            self.commit_pen(true);
            return;
        }
        if response.clicked() {
            if let Some(p) = doc_pos {
                self.inter.pen_points.push(p);
            }
        }
    }

    fn draw_preview(&self, painter: &egui::Painter) {
        // Rubber-band preview for create-drag.
        if let (Some(a), Some(b)) = (self.inter.drag_start, self.inter.drag_now) {
            if let Some(shape) = self.shape_from_drag(a, b) {
                canvas::paint_shape(painter, &self.view, &shape, false);
            }
        }
        // Pen in-progress polyline + vertices.
        if !self.inter.pen_points.is_empty() {
            let pts: Vec<Pos2> = self
                .inter
                .pen_points
                .iter()
                .map(|&p| self.view.doc_to_screen(p))
                .collect();
            if pts.len() >= 2 {
                painter.add(egui::Shape::line(
                    pts.clone(),
                    egui::Stroke::new(
                        self.stroke_w.max(1.0) * self.view.zoom,
                        canvas::to_color32(self.stroke),
                    ),
                ));
            }
            for p in &pts {
                painter.circle_filled(*p, 3.0, theme::accent());
            }
        }
    }
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
