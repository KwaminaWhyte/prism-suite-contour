//! Panel and menu UI: the menu bar, tool palette, and the right-hand inspector
//! (fill, stroke, transform, arrange, align, and the layers list).

use super::{align_button, color_row, gradient_editor, ContourApp, Tool};
use crate::align::{Align, AlignTo, Distribute};
use crate::arrange::{self, Arrange};
use crate::boolean::BoolOp;
use crate::document::{LineCap, LineJoin};
use crate::gradient::{Gradient, GradientKind};
use crate::workspace::{self, Panel};
use crate::{icons, theme};
use egui::{Color32, Vec2};

impl ContourApp {
    pub(super) fn menu_bar(&mut self, root: &mut egui::Ui) {
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
                    let has_sel = !self.selection.is_empty();
                    ui.add_enabled_ui(has_sel, |ui| {
                        if ui.button("Cut").clicked() {
                            self.cut_selection();
                            ui.close_menu();
                        }
                        if ui.button("Copy").clicked() {
                            self.copy_selection();
                            ui.close_menu();
                        }
                    });
                    ui.add_enabled_ui(self.can_paste(), |ui| {
                        if ui.button("Paste").clicked() {
                            self.paste();
                            ui.close_menu();
                        }
                        if ui.button("Paste in Place").clicked() {
                            self.paste_in_place();
                            ui.close_menu();
                        }
                        if ui.button("Paste in Front").clicked() {
                            self.paste_in_front();
                            ui.close_menu();
                        }
                        if ui.button("Paste in Back").clicked() {
                            self.paste_in_back();
                            ui.close_menu();
                        }
                    });
                    ui.separator();
                    ui.add_enabled_ui(has_sel, |ui| {
                        if ui.button("Duplicate").clicked() {
                            self.duplicate_selection();
                            ui.close_menu();
                        }
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
                    ui.add_enabled_ui(self.can_group(), |ui| {
                        if ui.button(format!("{}  Group", icons::GROUP)).clicked() {
                            self.group_selection();
                            ui.close_menu();
                        }
                    });
                    ui.add_enabled_ui(self.can_ungroup(), |ui| {
                        if ui.button(format!("{}  Ungroup", icons::UNGROUP)).clicked() {
                            self.ungroup_selection();
                            ui.close_menu();
                        }
                    });

                    ui.separator();
                    ui.menu_button("Clipping Mask", |ui| {
                        ui.add_enabled_ui(self.can_make_clip(), |ui| {
                            if ui
                                .button(format!("{}  Make", icons::CLIP_MAKE))
                                .on_hover_text("Clip the lower objects to the topmost (Ctrl/Cmd+7)")
                                .clicked()
                            {
                                self.make_clip();
                                ui.close_menu();
                            }
                        });
                        ui.add_enabled_ui(self.can_release_clip(), |ui| {
                            if ui
                                .button(format!("{}  Release", icons::CLIP_RELEASE))
                                .on_hover_text("Release the clipping mask (Alt+Ctrl/Cmd+7)")
                                .clicked()
                            {
                                self.release_clip();
                                ui.close_menu();
                            }
                        });
                    });

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

                    ui.separator();
                    if ui
                        .button(format!("{}  New Artboard", icons::ARTBOARD))
                        .clicked()
                    {
                        self.add_artboard();
                        ui.close_menu();
                    }
                });
                ui.menu_button("View", |ui| {
                    ui.checkbox(&mut self.show_rulers, "Rulers");
                    ui.checkbox(&mut self.show_grid, "Grid");
                    ui.checkbox(&mut self.show_guides, "Guides");
                    ui.separator();
                    if ui.button("Fit artboards").clicked() {
                        self.fit_artboards_requested = true;
                        ui.close_menu();
                    }
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
                ui.menu_button("Window", |ui| {
                    // A checkbox per panel toggles its visibility; the canvas is
                    // always shown so it is not listed. Bound straight to the
                    // workspace flags so the menu reflects (and drives) state.
                    for panel in Panel::ALL {
                        ui.checkbox(self.workspace.flag_mut(panel), panel.label());
                    }
                    ui.separator();
                    ui.add_enabled_ui(!self.workspace.is_default(), |ui| {
                        if ui.button("Reset panels").clicked() {
                            self.workspace.reset();
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

    /// The bottom status / context bar: cursor coordinates, selection count, the
    /// active artboard, and the zoom percentage. A right-aligned "fit zoom"
    /// reset (and `1:1`) make the zoom read-out interactive, à la Illustrator's
    /// bottom-left zoom field. Toggled from the Window menu.
    pub(super) fn status_bar(&mut self, root: &mut egui::Ui) {
        egui::TopBottomPanel::bottom("status_bar").show_inside(root, |ui| {
            ui.horizontal(|ui| {
                let active = self
                    .doc
                    .active_artboard
                    .min(self.doc.artboards.len().saturating_sub(1));
                let artboard = self
                    .doc
                    .artboards
                    .get(active)
                    .map(|a| a.name.as_str())
                    .unwrap_or("");
                let line = workspace::status_line(
                    self.cursor_doc,
                    self.selection.len(),
                    artboard,
                    self.view.zoom,
                );
                ui.add(egui::Label::new(egui::RichText::new(line).weak()).truncate());

                // Right side: a quick reset-to-100% button on the zoom read-out.
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui
                        .small_button("1:1")
                        .on_hover_text("Reset zoom to 100%")
                        .clicked()
                    {
                        self.view.zoom = 1.0;
                    }
                    if ui
                        .small_button(format!("{}  Fit", icons::ARTBOARD))
                        .on_hover_text("Fit all artboards in view")
                        .clicked()
                    {
                        self.fit_artboards_requested = true;
                    }
                });
            });
        });
    }

    pub(super) fn tool_palette(&mut self, root: &mut egui::Ui) {
        egui::SidePanel::left("tools")
            .exact_width(56.0)
            .resizable(false)
            .show_inside(root, |ui| {
                // Scroll the compact tool column so every tool stays reachable
                // even on a short window (Affinity-style left tool strip).
                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        ui.add_space(6.0);
                        ui.vertical_centered(|ui| {
                            for tool in [
                                Tool::Select,
                                Tool::Rect,
                                Tool::Ellipse,
                                Tool::Line,
                                Tool::Pen,
                                Tool::Eyedropper,
                                Tool::Artboard,
                            ] {
                                let selected = self.tool == tool;
                                let btn =
                                    egui::Button::new(egui::RichText::new(tool.icon()).size(20.0))
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
            });
    }

    pub(super) fn right_panel(&mut self, root: &mut egui::Ui) {
        egui::SidePanel::right("inspector")
            .default_width(248.0)
            .show_inside(root, |ui| {
                // The inspector is a tall stack of property groups (Affinity's
                // right-side "Studio"). Scroll the whole body so no section is
                // unreachable on a short window, and group each into a
                // collapsible header so users can hide what they don't need.
                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        egui::CollapsingHeader::new("Style")
                            .default_open(true)
                            .show(ui, |ui| {
                                self.fill_section(ui);
                                color_row(ui, "Stroke", &mut self.stroke);
                                ui.horizontal(|ui| {
                                    ui.label("Width");
                                    ui.add(
                                        egui::Slider::new(&mut self.stroke_w, 0.0..=40.0)
                                            .suffix(" px"),
                                    );
                                });
                            });

                        egui::CollapsingHeader::new("Stroke options")
                            .default_open(false)
                            .show(ui, |ui| {
                                self.stroke_section(ui);
                            });
                        egui::CollapsingHeader::new("Transform")
                            .default_open(true)
                            .show(ui, |ui| {
                                self.transform_section(ui);
                            });
                        egui::CollapsingHeader::new("Group")
                            .default_open(false)
                            .show(ui, |ui| {
                                self.group_section(ui);
                            });
                        egui::CollapsingHeader::new("Clipping Mask")
                            .default_open(false)
                            .show(ui, |ui| {
                                self.clip_section(ui);
                            });
                        egui::CollapsingHeader::new("Arrange")
                            .default_open(false)
                            .show(ui, |ui| {
                                self.arrange_section(ui);
                            });
                        egui::CollapsingHeader::new("Align")
                            .default_open(true)
                            .show(ui, |ui| {
                                self.align_section(ui);
                            });
                        egui::CollapsingHeader::new("Artboards")
                            .default_open(false)
                            .show(ui, |ui| {
                                self.artboards_section(ui);
                            });

                        // Direct-select hint when a path is the active selection.
                        if self.tool == Tool::Select && self.selected_is_path() {
                            ui.separator();
                            ui.label(egui::RichText::new("Edit path").strong());
                            ui.weak("Drag an anchor or handle to reshape.");
                            ui.weak("Dbl-click a segment to add an anchor.");
                            ui.weak("Dbl-click an anchor to delete it.");
                            ui.weak("Alt-click an anchor: smooth ⇄ corner.");
                        }

                        // Eyedropper usage hint.
                        if self.tool == Tool::Eyedropper {
                            ui.separator();
                            ui.label(egui::RichText::new("Eyedropper").strong());
                            ui.weak("Click a shape to sample its fill & stroke.");
                            ui.weak("With a selection, the look is applied to it.");
                            ui.weak("Alt-click: sample only (don't apply).");
                        }

                        egui::CollapsingHeader::new("Layers")
                            .default_open(true)
                            .show(ui, |ui| {
                                self.layers_panel(ui);
                            });
                    });
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

    /// Group / Ungroup controls. Group folds the selection into one unit that
    /// selects and transforms together (Cmd/Ctrl+G); Ungroup dissolves it
    /// (Cmd/Ctrl+Shift+G). Each is a single undo step; buttons disable when the
    /// gesture would be a no-op. Mirrors the `Object → Group / Ungroup` menu.
    fn group_section(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.add_enabled_ui(self.can_group(), |ui| {
                if ui
                    .button(format!("{}  Group", icons::GROUP))
                    .on_hover_text("Group selection (Cmd/Ctrl+G)")
                    .clicked()
                {
                    self.group_selection();
                }
            });
            ui.add_enabled_ui(self.can_ungroup(), |ui| {
                if ui
                    .button(format!("{}  Ungroup", icons::UNGROUP))
                    .on_hover_text("Ungroup selection (Cmd/Ctrl+Shift+G)")
                    .clicked()
                {
                    self.ungroup_selection();
                }
            });
        });
        if !self.can_group() && !self.can_ungroup() {
            ui.weak("Select 2+ shapes to group.");
        }
    }

    /// Make / Release clipping-mask controls. Make clips the lower selected
    /// objects to the topmost one's outline (Cmd/Ctrl+7); Release restores the
    /// originals (Alt+Cmd/Ctrl+7). Each is a single undo step; buttons disable
    /// when the gesture is a no-op. Mirrors `Object → Clipping Mask`.
    fn clip_section(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.add_enabled_ui(self.can_make_clip(), |ui| {
                if ui
                    .button(format!("{}  Make", icons::CLIP_MAKE))
                    .on_hover_text("Clip lower objects to the topmost (Cmd/Ctrl+7)")
                    .clicked()
                {
                    self.make_clip();
                }
            });
            ui.add_enabled_ui(self.can_release_clip(), |ui| {
                if ui
                    .button(format!("{}  Release", icons::CLIP_RELEASE))
                    .on_hover_text("Release clipping mask (Alt+Cmd/Ctrl+7)")
                    .clicked()
                {
                    self.release_clip();
                }
            });
        });
        if !self.can_make_clip() && !self.can_release_clip() {
            ui.weak("Select 2+ objects; the topmost becomes the mask.");
        }
    }

    /// Arrange (paint-order / stacking) controls: bring-to-front, forward,
    /// backward, and send-to-back. Each is a single undo step; a button is
    /// disabled when the move would not change the order (e.g. the selection is
    /// already on top). Mirrors `Object → Arrange` and the Cmd/Ctrl+]/[ keys.
    fn arrange_section(&mut self, ui: &mut egui::Ui) {
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
        ui.horizontal(|ui| {
            ui.label("Relative to");
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

    /// The Artboards section: a list of named artboards (click to make active),
    /// a per-board rename + size editor for the active board, an "Add" button
    /// (tiles a new board to the right), and a delete button. Resizing / renaming
    /// the active board and adding / deleting are each a single undo step. The
    /// Artboard tool (left palette) drags out / moves boards on canvas.
    fn artboards_section(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui
                    .button("+ Add")
                    .on_hover_text("Add an artboard to the right")
                    .clicked()
                {
                    self.add_artboard();
                }
            });
        });

        let active = self
            .doc
            .active_artboard
            .min(self.doc.artboards.len().saturating_sub(1));
        // Board list: click a row to activate; trash to delete (≥2 boards).
        let mut activate: Option<usize> = None;
        let mut delete: Option<usize> = None;
        let count = self.doc.artboards.len();
        for i in 0..count {
            let name = self.doc.artboards[i].name.clone();
            ui.horizontal(|ui| {
                if ui.selectable_label(i == active, name).clicked() {
                    activate = Some(i);
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.add_enabled_ui(count > 1, |ui| {
                        if ui
                            .small_button(icons::TRASH)
                            .on_hover_text("Delete artboard")
                            .clicked()
                        {
                            delete = Some(i);
                        }
                    });
                });
            });
        }
        if let Some(i) = activate {
            self.set_active_artboard(i);
        }
        if let Some(i) = delete {
            self.delete_artboard(i);
            return;
        }

        // Active-board editor: rename + width/height (one undo step on change).
        if let Some(ab) = self.doc.artboards.get(active).cloned() {
            let mut name = ab.name.clone();
            let mut w = ab.rect[2];
            let mut h = ab.rect[3];
            let mut changed = false;
            ui.horizontal(|ui| {
                ui.label("Name");
                if ui.text_edit_singleline(&mut name).changed() {
                    changed = true;
                }
            });
            ui.horizontal(|ui| {
                ui.label("W");
                if ui
                    .add(
                        egui::DragValue::new(&mut w)
                            .speed(1.0)
                            .range(1.0..=100_000.0),
                    )
                    .changed()
                {
                    changed = true;
                }
                ui.label("H");
                if ui
                    .add(
                        egui::DragValue::new(&mut h)
                            .speed(1.0)
                            .range(1.0..=100_000.0),
                    )
                    .changed()
                {
                    changed = true;
                }
            });
            if changed {
                self.checkpoint();
                if let Some(b) = self.doc.artboards.get_mut(active) {
                    b.name = name;
                    b.rect[2] = w.max(1.0);
                    b.rect[3] = h.max(1.0);
                }
            }
        }
    }

    /// The Layers list: newest on top, with visibility toggle, reorder up/down,
    /// delete, and click-to-select (shift-click toggles the shape in the
    /// multi-selection set).
    fn layers_panel(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
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

        // Paint order: index 0 painted first (bottom). "Newest on top" =>
        // iterate indices in reverse so the last (topmost) is listed first. The
        // enclosing inspector ScrollArea scrolls these rows; no inner scroll.
        for idx in (0..n).rev() {
            let primary = self.primary() == Some(idx);
            // A non-primary member of a multi-selection.
            let secondary = !primary && self.is_selected(idx);
            let visible = self.doc.shapes[idx].visible();
            let label = self.doc.shapes[idx].label();
            let grouped = self.doc.shapes[idx].group().is_some();

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

                // A leading group glyph marks shapes that belong to a group.
                let prefix = if grouped {
                    format!("{}  ", icons::GROUP)
                } else {
                    String::new()
                };
                let mut text = egui::RichText::new(format!("{}{}  {}", prefix, n - idx, label));
                if !visible {
                    text = text.weak();
                }
                if secondary {
                    text = text.color(theme::accent());
                }
                if ui
                    .selectable_label(primary, text)
                    .on_hover_text(if grouped { "Grouped" } else { "" })
                    .clicked()
                {
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
                self.toggle_group_selection(i);
            } else {
                self.select_only(Some(i));
                self.expand_selection_to_groups();
            }
        }
        if let Some(i) = to_delete {
            self.checkpoint();
            self.remove_shape(i);
        }
    }
}
