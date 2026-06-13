//! Panel and menu UI: the menu bar, tool palette, and the right-hand inspector
//! (fill, stroke, transform, arrange, align, and the layers list).

use super::{align_button, color_row, gradient_editor, ContourApp, Tool};
use crate::align::{Align, AlignTo, Distribute};
use crate::appearance::{BlendMode, Effect, Fill, Paint, Stroke as AppStroke};
use crate::arrange::{self, Arrange};
use crate::boolean::{BoolFillRule, BoolOp};
use crate::document::{Arrowhead, LineCap, LineJoin, StrokeAlign};
use crate::gradient::{Gradient, GradientKind};
use crate::workspace::{self, Panel};
use crate::{icons, theme};
use egui::{Color32, Sense, Stroke, Vec2};

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
                    ui.menu_button(format!("{}  Pathfinder", icons::UNITE), |ui| {
                        // Compound-path fill rule the ops interpret nested input
                        // with (non-zero vs even-odd). Drives every op below.
                        ui.horizontal(|ui| {
                            ui.label("Fill rule");
                            ui.selectable_value(
                                &mut self.bool_fill_rule,
                                BoolFillRule::NonZero,
                                "Non-zero",
                            )
                            .on_hover_text("Overlapping same-direction rings stay filled");
                            ui.selectable_value(
                                &mut self.bool_fill_rule,
                                BoolFillRule::EvenOdd,
                                "Even-odd",
                            )
                            .on_hover_text("A ring inside another carves a hole (donut rule)");
                        });
                        ui.separator();
                        ui.add_enabled_ui(two, |ui| {
                            // Shape modes: combine the two operands into new area.
                            let modes = [
                                (icons::UNITE, BoolOp::Union),
                                (icons::INTERSECT, BoolOp::Intersect),
                                (icons::EXCLUDE_OVERLAP, BoolOp::Exclude),
                                (icons::EXCLUDE, BoolOp::Difference),
                                (icons::MINUS_BACK, BoolOp::MinusBack),
                            ];
                            for (icon, op) in modes {
                                if ui.button(format!("{icon}  {}", op.label())).clicked() {
                                    self.apply_bool(op);
                                    ui.close_menu();
                                }
                            }
                            ui.separator();
                            // Pathfinders: split / trim the operands into faces.
                            let finders = [
                                (icons::DIVIDE, BoolOp::Divide),
                                (icons::TRIM, BoolOp::Trim),
                                (icons::MERGE, BoolOp::Merge),
                                (icons::CROP, BoolOp::Crop),
                                (icons::OUTLINE, BoolOp::Outline),
                            ];
                            for (icon, op) in finders {
                                if ui.button(format!("{icon}  {}", op.label())).clicked() {
                                    self.apply_bool(op);
                                    ui.close_menu();
                                }
                            }
                        });
                        if !two {
                            ui.weak("Select exactly 2 shapes");
                        }
                    });

                    ui.separator();
                    ui.menu_button(format!("{}  Compound Path", icons::COMPOUND), |ui| {
                        ui.add_enabled_ui(self.can_make_compound(), |ui| {
                            if ui
                                .button("Make")
                                .on_hover_text(
                                    "Combine the selected closed shapes into one \
                                     compound path with holes (Ctrl/Cmd+8)",
                                )
                                .clicked()
                            {
                                self.make_compound();
                                ui.close_menu();
                            }
                        });
                        ui.add_enabled_ui(self.can_release_compound(), |ui| {
                            if ui
                                .button("Release")
                                .on_hover_text(
                                    "Split the compound path back into separate paths \
                                     (Alt+Ctrl/Cmd+8)",
                                )
                                .clicked()
                            {
                                self.release_compound();
                                ui.close_menu();
                            }
                        });
                        // Fill-rule toggle for the selected compound path(s).
                        ui.add_enabled_ui(self.can_release_compound(), |ui| {
                            ui.separator();
                            ui.label("Fill rule");
                            if ui.button("Non-zero").clicked() {
                                self.set_compound_fill_rule(crate::document::FillRule::NonZero);
                                ui.close_menu();
                            }
                            if ui
                                .button("Even-odd")
                                .on_hover_text("A sub-contour inside another carves a hole")
                                .clicked()
                            {
                                self.set_compound_fill_rule(crate::document::FillRule::EvenOdd);
                                ui.close_menu();
                            }
                        });
                    });

                    ui.menu_button(format!("{}  Type", icons::TYPE), |ui| {
                        ui.add_enabled_ui(self.has_text_selected(), |ui| {
                            if ui
                                .button("Create Outlines")
                                .on_hover_text(
                                    "Replace the selected type with editable glyph \
                                     outlines (a compound path)",
                                )
                                .clicked()
                            {
                                self.convert_text_to_outlines();
                                ui.close_menu();
                            }
                        });
                    });

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
                    ui.menu_button("Opacity Mask", |ui| {
                        ui.add_enabled_ui(self.can_make_omask(), |ui| {
                            if ui
                                .button(format!("{}  Make", icons::CLIP_MAKE))
                                .on_hover_text(
                                    "Mask lower objects by the topmost shape's luminance",
                                )
                                .clicked()
                            {
                                self.make_omask();
                                ui.close_menu();
                            }
                        });
                        ui.add_enabled_ui(self.can_release_omask(), |ui| {
                            if ui
                                .button(format!("{}  Release", icons::CLIP_RELEASE))
                                .on_hover_text("Release the opacity mask")
                                .clicked()
                            {
                                self.release_omask();
                                ui.close_menu();
                            }
                            if ui
                                .button("Invert Mask")
                                .on_hover_text("Flip the mask: black reveals, white hides")
                                .clicked()
                            {
                                self.toggle_omask_invert();
                                ui.close_menu();
                            }
                        });
                    });
                    ui.menu_button("Blend", |ui| {
                        // Specified-steps count for Make Blend.
                        ui.horizontal(|ui| {
                            ui.label("Steps");
                            let mut steps = self.blend_steps as u32;
                            if ui
                                .add(egui::DragValue::new(&mut steps).speed(0.2).range(1..=64))
                                .on_hover_text("Intermediate objects between the two ends")
                                .changed()
                            {
                                self.blend_steps = steps as usize;
                            }
                        });
                        ui.add_enabled_ui(self.can_make_blend(), |ui| {
                            if ui
                                .button(format!("{}  Make", icons::BLEND))
                                .on_hover_text(
                                    "Interpolate intermediate objects between two selected shapes",
                                )
                                .clicked()
                            {
                                self.make_blend();
                                ui.close_menu();
                            }
                        });
                        ui.add_enabled_ui(self.can_release_blend(), |ui| {
                            if ui
                                .button("Release")
                                .on_hover_text("Delete the intermediate steps, keep the two ends")
                                .clicked()
                            {
                                self.release_blend();
                                ui.close_menu();
                            }
                            if ui
                                .button("Expand")
                                .on_hover_text("Detach the steps into independent objects")
                                .clicked()
                            {
                                self.expand_blend();
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

    /// The Swatches panel: the document colour palette. A grid of colour chips —
    /// **click** to paint the selection's fill (and set the default fill),
    /// **Shift-click** to paint the stroke, **Alt-click** to select a swatch for
    /// editing — above an editor for the selected swatch (rename, recolour,
    /// global toggle, delete) and an "add the current fill" button. A **global**
    /// swatch propagates a recolour to all artwork using its colour. Docked on
    /// the left; toggled from the Window menu.
    pub(super) fn swatches_panel(&mut self, root: &mut egui::Ui) {
        egui::SidePanel::left("swatches")
            .default_width(176.0)
            .show_inside(root, |ui| {
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new("Swatches").strong());
                    ui.weak(format!("{}", self.doc.swatches.len()));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui
                            .button("+")
                            .on_hover_text("Add a swatch from the current fill")
                            .clicked()
                        {
                            self.add_fill_swatch();
                        }
                    });
                });
                ui.weak("Click: fill · Shift: stroke · Alt: edit");
                ui.separator();

                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        // Which swatch (if any) names the current fill — drawn with
                        // a ring so the active colour is visible at a glance.
                        let active_fill = self.doc.swatches.id_for_color(self.fill);

                        // Deferred actions so we don't borrow the palette while
                        // iterating it.
                        let mut apply_fill: Option<u64> = None;
                        let mut apply_stroke: Option<u64> = None;
                        let mut select: Option<u64> = None;

                        let chip = Vec2::new(24.0, 24.0);
                        let per_row = ((ui.available_width() + 4.0) / (chip.x + 4.0))
                            .floor()
                            .max(1.0) as usize;

                        let entries: Vec<(u64, [f32; 4], String, bool)> = self
                            .doc
                            .swatches
                            .list
                            .iter()
                            .map(|s| (s.id, s.color, s.name.clone(), s.global))
                            .collect();

                        for row in entries.chunks(per_row) {
                            ui.horizontal(|ui| {
                                for (id, color, name, global) in row {
                                    let (rect, resp) = ui.allocate_exact_size(chip, Sense::click());
                                    let fill = Color32::from_rgba_unmultiplied(
                                        (color[0] * 255.0) as u8,
                                        (color[1] * 255.0) as u8,
                                        (color[2] * 255.0) as u8,
                                        (color[3] * 255.0) as u8,
                                    );
                                    let selected = self.selected_swatch == Some(*id);
                                    let is_active = active_fill == Some(*id);
                                    ui.painter().rect_filled(rect, 4.0, fill);
                                    // Selected: thick accent ring. Active fill:
                                    // thinner accent ring. Else a faint border.
                                    let border = if selected {
                                        Stroke::new(2.0, theme::accent())
                                    } else if is_active {
                                        Stroke::new(1.5, theme::accent())
                                    } else {
                                        Stroke::new(1.0, Color32::from_gray(70))
                                    };
                                    ui.painter().rect_stroke(
                                        rect,
                                        4.0,
                                        border,
                                        egui::StrokeKind::Inside,
                                    );
                                    // A global swatch gets a small corner dot, the
                                    // way Illustrator marks its global swatches.
                                    if *global {
                                        ui.painter().circle_filled(
                                            rect.right_bottom() + Vec2::new(-4.0, -4.0),
                                            2.0,
                                            theme::accent(),
                                        );
                                    }
                                    let tip: String = if *global {
                                        format!("{name} (global)")
                                    } else {
                                        name.to_string()
                                    };
                                    let resp = resp.on_hover_text(tip);
                                    if resp.clicked() {
                                        let mods = ui.input(|i| i.modifiers);
                                        if mods.alt {
                                            select = Some(*id);
                                        } else if mods.shift {
                                            apply_stroke = Some(*id);
                                        } else {
                                            apply_fill = Some(*id);
                                        }
                                    }
                                }
                            });
                            ui.add_space(4.0);
                        }

                        if self.doc.swatches.is_empty() {
                            ui.weak("No swatches. Add one with +.");
                        }

                        if let Some(id) = apply_fill {
                            self.apply_swatch_fill(id);
                            self.selected_swatch = Some(id);
                        }
                        if let Some(id) = apply_stroke {
                            self.apply_swatch_stroke(id);
                            self.selected_swatch = Some(id);
                        }
                        if let Some(id) = select {
                            self.selected_swatch = Some(id);
                        }

                        // Editor for the selected swatch.
                        if let Some(id) = self.selected_swatch {
                            if let Some(sw) = self.doc.swatches.get(id).cloned() {
                                ui.separator();
                                ui.label(egui::RichText::new("Edit swatch").strong());

                                let mut name = sw.name.clone();
                                ui.horizontal(|ui| {
                                    ui.label("Name");
                                    if ui.text_edit_singleline(&mut name).lost_focus() {
                                        self.rename_swatch(id, &name);
                                    }
                                });

                                let mut c = Color32::from_rgba_unmultiplied(
                                    (sw.color[0] * 255.0) as u8,
                                    (sw.color[1] * 255.0) as u8,
                                    (sw.color[2] * 255.0) as u8,
                                    (sw.color[3] * 255.0) as u8,
                                );
                                ui.horizontal(|ui| {
                                    ui.label("Colour");
                                    if ui.color_edit_button_srgba(&mut c).changed() {
                                        let color = [
                                            c.r() as f32 / 255.0,
                                            c.g() as f32 / 255.0,
                                            c.b() as f32 / 255.0,
                                            c.a() as f32 / 255.0,
                                        ];
                                        self.recolor_swatch(id, color);
                                    }
                                });

                                let mut global = sw.global;
                                if ui
                                    .checkbox(&mut global, "Global")
                                    .on_hover_text(
                                        "Recolouring a global swatch updates all artwork using it",
                                    )
                                    .changed()
                                {
                                    self.set_swatch_global(id, global);
                                }

                                ui.horizontal(|ui| {
                                    if ui.button("Fill").clicked() {
                                        self.apply_swatch_fill(id);
                                    }
                                    if ui.button("Stroke").clicked() {
                                        self.apply_swatch_stroke(id);
                                    }
                                    ui.with_layout(
                                        egui::Layout::right_to_left(egui::Align::Center),
                                        |ui| {
                                            if ui
                                                .button(icons::TRASH)
                                                .on_hover_text("Delete swatch")
                                                .clicked()
                                            {
                                                self.delete_swatch(id);
                                                self.selected_swatch = None;
                                            }
                                        },
                                    );
                                });
                            } else {
                                // The selected swatch was removed (e.g. undo).
                                self.selected_swatch = None;
                            }
                        }
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
                                Tool::DirectSelect,
                                Tool::Rect,
                                Tool::Ellipse,
                                Tool::Line,
                                Tool::Polygon,
                                Tool::Star,
                                Tool::Pen,
                                Tool::Type,
                                Tool::Eyedropper,
                                Tool::ShapeBuilder,
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
                                    self.set_tool(tool);
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

                        // Type section: only meaningful (and only shown) when a
                        // text object is the primary selection.
                        if self.primary_is_text() {
                            egui::CollapsingHeader::new("Type")
                                .default_open(true)
                                .show(ui, |ui| {
                                    self.type_section(ui);
                                });
                        }

                        // Live Shape section: only shown when a parametric
                        // polygon / star is the primary selection. Editing a
                        // count / radius / ratio regenerates its outline live.
                        if self.primary_live_shape().is_some() {
                            egui::CollapsingHeader::new("Live Shape")
                                .default_open(true)
                                .show(ui, |ui| {
                                    self.live_shape_section(ui);
                                });
                        }

                        egui::CollapsingHeader::new("Stroke options")
                            .default_open(false)
                            .show(ui, |ui| {
                                self.stroke_section(ui);
                            });
                        egui::CollapsingHeader::new("Appearance")
                            .default_open(false)
                            .show(ui, |ui| {
                                self.appearance_section(ui);
                            });
                        egui::CollapsingHeader::new("Graphic Styles")
                            .default_open(false)
                            .show(ui, |ui| {
                                self.graphic_styles_section(ui);
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
                        egui::CollapsingHeader::new("Opacity Mask")
                            .default_open(false)
                            .show(ui, |ui| {
                                self.opacity_mask_section(ui);
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

                        // Direct-Select tool hint.
                        if self.tool == Tool::DirectSelect {
                            ui.separator();
                            ui.label(egui::RichText::new("Direct Select (A)").strong());
                            ui.weak("Click / marquee anchors; drag to move them.");
                            ui.weak("Drag a handle knob to reshape the curve.");
                            ui.weak("Click a segment to add an anchor.");
                            ui.weak("Delete removes the selected anchors.");
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
        // reflects the selection), falling back to the app default. For a shape
        // with an explicit Appearance stack the Style section edits its *topmost*
        // fill (Illustrator's "basic appearance" row), so what's shown matches
        // what renders; otherwise it edits the legacy single fill.
        let primary = self.primary();
        let seeded = primary.and_then(|i| self.doc.shapes.get(i));
        let stacked = seeded.is_some_and(|s| s.appearance().is_some());
        let top_fill = seeded
            .and_then(|s| s.appearance())
            .and_then(|a| a.fills.last());
        let mut solid = match (top_fill, seeded.and_then(|s| s.fill_color())) {
            (Some(f), _) => f.paint.swatch(),
            (None, Some(c)) => c,
            (None, None) => self.fill,
        };
        let mut grad: Option<Gradient> = match (top_fill, seeded) {
            (Some(f), _) => f.paint.gradient().cloned(),
            (None, Some(s)) => s.fill_gradient().cloned(),
            (None, None) => self.fill_gradient.clone(),
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
                    if stacked {
                        // Write into the topmost fill of the explicit stack (the
                        // basic-appearance row), so the edit renders.
                        if let Some(ap) = shape.appearance_mut() {
                            if let Some(f) = ap.fills.last_mut() {
                                f.paint = match grad {
                                    Some(g) => Paint::Gradient(g),
                                    None => Paint::Solid(solid),
                                };
                            } else {
                                ap.fills.push(Fill::solid(solid));
                                if let (Some(g), Some(f)) = (grad, ap.fills.last_mut()) {
                                    f.paint = Paint::Gradient(g);
                                }
                            }
                        }
                    } else {
                        shape.set_fill_color(solid);
                        shape.set_fill_gradient(grad);
                    }
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
        // panel reflects what's selected), falling back to the app default. For a
        // shape with an explicit Appearance stack the controls edit its *topmost*
        // stroke's style so the edit renders; otherwise the legacy stroke style.
        let seeded = self.primary().and_then(|i| self.doc.shapes.get(i));
        let stacked = seeded.is_some_and(|sh| sh.appearance().is_some());
        let mut s = match seeded {
            Some(shape) => match shape.appearance().and_then(|a| a.strokes.last()) {
                Some(top) => top.style.clone(),
                None => shape.stroke_style().clone(),
            },
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

        // Align stroke (center / inside / outside).
        ui.horizontal(|ui| {
            ui.label("Align");
            egui::ComboBox::from_id_salt("stroke_align")
                .selected_text(s.align.label())
                .show_ui(ui, |ui| {
                    for a in StrokeAlign::ALL {
                        if ui.selectable_value(&mut s.align, a, a.label()).changed() {
                            changed = true;
                        }
                    }
                });
        });

        // Arrowheads (start / end markers + scale).
        ui.horizontal(|ui| {
            ui.label("Start");
            egui::ComboBox::from_id_salt("arrow_start")
                .selected_text(s.start_arrow.label())
                .show_ui(ui, |ui| {
                    for a in Arrowhead::ALL {
                        if ui
                            .selectable_value(&mut s.start_arrow, a, a.label())
                            .changed()
                        {
                            changed = true;
                        }
                    }
                });
            ui.label("End");
            egui::ComboBox::from_id_salt("arrow_end")
                .selected_text(s.end_arrow.label())
                .show_ui(ui, |ui| {
                    for a in Arrowhead::ALL {
                        if ui
                            .selectable_value(&mut s.end_arrow, a, a.label())
                            .changed()
                        {
                            changed = true;
                        }
                    }
                });
        });
        if s.has_arrows() {
            ui.horizontal(|ui| {
                ui.label("Arrow size");
                if ui
                    .add(egui::Slider::new(&mut s.arrow_scale, 0.25..=4.0).suffix("×"))
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
                    if stacked {
                        if let Some(top) = shape
                            .appearance_mut()
                            .as_mut()
                            .and_then(|a| a.strokes.last_mut())
                        {
                            top.style = style;
                        }
                    } else {
                        *shape.stroke_style_mut() = style;
                    }
                }
            }
        }
    }

    /// The **Appearance** panel: the primary selected shape's stacked fills and
    /// strokes (Illustrator's Appearance panel). Each list is shown top-to-bottom
    /// (the topmost paint layer first), with per-item add / remove / move-up /
    /// move-down, a visibility toggle, a solid-or-gradient paint editor, and an
    /// opacity + blend-mode row. Editing migrates a legacy single-fill/stroke
    /// shape into an explicit stack on first touch. Each discrete change is one
    /// undo step.
    fn appearance_section(&mut self, ui: &mut egui::Ui) {
        let Some(i) = self.primary() else {
            ui.weak("Select a shape to edit its appearance.");
            return;
        };
        // Work on a clone of the shape's *effective* appearance (its explicit
        // stack, or one migrated from its legacy fields), edit it, then commit the
        // whole thing back as one undo step if anything changed.
        let Some(shape) = self.doc.shapes.get(i) else {
            return;
        };
        let fillable = shape.fill_color().is_some();
        let mut ap = shape.effective_appearance();
        let mut changed = false;

        // --- Fills (top-to-bottom) ------------------------------------------
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("Fills").strong());
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.add_enabled_ui(fillable, |ui| {
                    if ui.button("+").on_hover_text("Add a fill").clicked() {
                        ap.fills.push(Fill::solid(self.fill));
                        changed = true;
                    }
                });
            });
        });
        if !fillable {
            ui.weak("This shape has no fill region.");
        } else if ap.fills.is_empty() {
            ui.weak("No fills. Add one with +.");
        } else {
            changed |= appearance_fill_list(ui, &mut ap);
        }

        ui.separator();

        // --- Strokes (top-to-bottom) ----------------------------------------
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("Strokes").strong());
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button("+").on_hover_text("Add a stroke").clicked() {
                    ap.strokes
                        .push(AppStroke::solid(self.stroke, self.stroke_w.max(1.0)));
                    changed = true;
                }
            });
        });
        if ap.strokes.is_empty() {
            ui.weak("No strokes. Add one with +.");
        } else {
            changed |= appearance_stroke_list(ui, &mut ap);
        }

        ui.separator();

        // --- Effects (live, non-destructive; top-to-bottom) -----------------
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("Effects").strong());
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.menu_button("+", |ui| {
                    if ui.button("Drop Shadow").clicked() {
                        ap.effects.push(Effect::drop_shadow());
                        changed = true;
                        ui.close_menu();
                    }
                    if ui.button("Gaussian Blur").clicked() {
                        ap.effects.push(Effect::gaussian_blur());
                        changed = true;
                        ui.close_menu();
                    }
                })
                .response
                .on_hover_text("Add a live effect");
            });
        });
        if ap.effects.is_empty() {
            ui.weak("No effects. Add one with +.");
        } else {
            changed |= appearance_effect_list(ui, &mut ap);
        }

        if changed {
            self.checkpoint();
            if let Some(shape) = self.doc.shapes.get_mut(i) {
                shape.set_appearance(Some(ap));
            }
        }
    }

    /// The **Graphic Styles** panel: the document's named-appearance library
    /// (Illustrator's Graphic Styles panel). **Save** snapshots the current
    /// selection's appearance as a new style; clicking a style **applies** it to
    /// the selection (replacing its appearance); the per-style editor **renames**
    /// or **deletes** it. Each action is one undo step.
    fn graphic_styles_section(&mut self, ui: &mut egui::Ui) {
        let has_selection = !self.selection.is_empty();

        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("Styles").strong());
            ui.weak(format!("{}", self.doc.graphic_styles.len()));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.add_enabled_ui(has_selection, |ui| {
                    if ui
                        .button("+")
                        .on_hover_text("Save the selection's appearance as a style")
                        .clicked()
                    {
                        // Select the new style so it's ready to rename.
                        self.selected_style = self.save_graphic_style();
                    }
                });
            });
        });
        ui.weak("Click a style to apply it to the selection.");

        if self.doc.graphic_styles.is_empty() {
            ui.weak("No styles. Save one with +.");
            return;
        }

        // Deferred actions so we don't borrow the library while iterating it.
        let mut apply: Option<u64> = None;
        let mut select: Option<u64> = None;

        let entries: Vec<(u64, String)> = self
            .doc
            .graphic_styles
            .list
            .iter()
            .map(|s| (s.id, s.name.clone()))
            .collect();

        for (id, name) in &entries {
            let selected = self.selected_style == Some(*id);
            ui.horizontal(|ui| {
                if ui
                    .selectable_label(selected, format!("{}  {name}", icons::STYLE))
                    .on_hover_text("Apply to the selection (Alt: just select)")
                    .clicked()
                {
                    // Alt-click selects for editing without applying.
                    if ui.input(|i| i.modifiers.alt) {
                        select = Some(*id);
                    } else {
                        apply = Some(*id);
                    }
                }
            });
        }

        if let Some(id) = apply {
            self.apply_graphic_style(id);
            self.selected_style = Some(id);
        }
        if let Some(id) = select {
            self.selected_style = Some(id);
        }

        // Editor for the selected style.
        if let Some(id) = self.selected_style {
            if let Some(st) = self.doc.graphic_styles.get(id).cloned() {
                ui.separator();
                ui.label(egui::RichText::new("Edit style").strong());

                let mut name = st.name.clone();
                ui.horizontal(|ui| {
                    ui.label("Name");
                    if ui.text_edit_singleline(&mut name).lost_focus() {
                        self.rename_graphic_style(id, &name);
                    }
                });

                ui.horizontal(|ui| {
                    if ui
                        .add_enabled(has_selection, egui::Button::new("Apply"))
                        .clicked()
                    {
                        self.apply_graphic_style(id);
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui
                            .button(icons::TRASH)
                            .on_hover_text("Delete style")
                            .clicked()
                        {
                            self.delete_graphic_style(id);
                            self.selected_style = None;
                        }
                    });
                });
            } else {
                // The selected style was removed (e.g. undo).
                self.selected_style = None;
            }
        }
    }

    /// Transform controls: quick 90°/180° rotations, horizontal/vertical flips,
    /// and a numeric "rotate by" about the selection's centre. Mirrors the
    /// on-canvas transform box (drag a handle to scale, drag just outside a
    /// corner to rotate). Each action is one undo step.
    /// Whether the primary selection is a text object (gates the Type section).
    pub(super) fn primary_is_text(&self) -> bool {
        matches!(
            self.primary().and_then(|i| self.doc.shapes.get(i)),
            Some(crate::document::Shape::Text { .. })
        )
    }

    /// The live-shape parameters of the primary selection, if it is a parametric
    /// polygon / star (gates the Live Shape section).
    fn primary_live_shape(&self) -> Option<crate::liveshape::LiveShape> {
        self.primary()
            .and_then(|i| self.doc.shapes.get(i))
            .and_then(|s| s.live_shape())
    }

    /// The Live Shape inspector: edit the primary polygon / star's parameters
    /// (sides / points, radius, inner ratio). Any change regenerates its outline
    /// (via `set_live_shape`) as one undo step, like the Type section.
    fn live_shape_section(&mut self, ui: &mut egui::Ui) {
        use crate::liveshape::{LiveShape, MAX_SIDES, MIN_SIDES};
        let Some(idx) = self.primary() else { return };
        let Some(mut live) = self.primary_live_shape() else {
            return;
        };
        let mut changed = false;

        match &mut live {
            LiveShape::Polygon { sides, radius } => {
                ui.horizontal(|ui| {
                    ui.label("Sides");
                    if ui
                        .add(egui::Slider::new(sides, MIN_SIDES..=MAX_SIDES))
                        .changed()
                    {
                        changed = true;
                    }
                });
                ui.horizontal(|ui| {
                    ui.label("Radius");
                    if ui
                        .add(
                            egui::DragValue::new(radius)
                                .speed(0.5)
                                .range(0.0..=f32::MAX)
                                .suffix(" px"),
                        )
                        .changed()
                    {
                        changed = true;
                    }
                });
            }
            LiveShape::Star {
                points,
                radius,
                inner_ratio,
            } => {
                ui.horizontal(|ui| {
                    ui.label("Points");
                    if ui
                        .add(egui::Slider::new(points, MIN_SIDES..=MAX_SIDES))
                        .changed()
                    {
                        changed = true;
                    }
                });
                ui.horizontal(|ui| {
                    ui.label("Radius");
                    if ui
                        .add(
                            egui::DragValue::new(radius)
                                .speed(0.5)
                                .range(0.0..=f32::MAX)
                                .suffix(" px"),
                        )
                        .changed()
                    {
                        changed = true;
                    }
                });
                ui.horizontal(|ui| {
                    ui.label("Inner ratio");
                    if ui
                        .add(egui::Slider::new(inner_ratio, 0.05..=1.0).fixed_decimals(2))
                        .changed()
                    {
                        changed = true;
                    }
                });
            }
        }

        if changed {
            // Mirror the working defaults so the next new shape inherits them.
            match live {
                LiveShape::Polygon { sides, .. } => self.poly_sides = sides,
                LiveShape::Star {
                    points,
                    inner_ratio,
                    ..
                } => {
                    self.star_points = points;
                    self.star_ratio = inner_ratio;
                }
            }
            self.checkpoint();
            if let Some(shape) = self.doc.shapes.get_mut(idx) {
                shape.set_live_shape(live);
            }
        }
    }

    /// The Type inspector: edit the primary text object's string, font size, and
    /// alignment, plus a Create-Outlines button. Any change re-lays-out the glyph
    /// cache (via `set_text_params`) as one undo step.
    fn type_section(&mut self, ui: &mut egui::Ui) {
        use crate::text::{TextAlign, TextParams};
        let Some(idx) = self.primary() else { return };
        let Some(mut params): Option<TextParams> =
            self.doc.shapes.get(idx).and_then(|s| s.text_params().cloned())
        else {
            return;
        };
        let mut changed = false;

        ui.label("Text");
        // Multi-line string editor (canvas typing still works; this is the panel
        // path for precise edits).
        if ui
            .add(
                egui::TextEdit::multiline(&mut params.text)
                    .desired_rows(2)
                    .desired_width(f32::INFINITY),
            )
            .changed()
        {
            changed = true;
        }

        ui.horizontal(|ui| {
            ui.label("Font");
            // The currently-selected family (None → the bundled default's name).
            let current = params
                .font_family
                .clone()
                .unwrap_or_else(|| crate::fonts::DEFAULT_FONT_FAMILY.to_string());
            // Flag a persisted family this machine doesn't have (it renders with
            // the bundled fallback) so the mismatch is visible rather than silent.
            let label = if crate::fonts::is_available(&current) {
                current.clone()
            } else {
                format!("{current} (missing)")
            };
            egui::ComboBox::from_id_salt("type_font_family")
                .selected_text(label)
                .width(160.0)
                .show_ui(ui, |ui| {
                    for fam in crate::fonts::families() {
                        if ui
                            .selectable_label(current == *fam, fam)
                            .clicked()
                            && current != *fam
                        {
                            // Store None for the bundled default so new objects and
                            // the explicit default serialize identically (and old
                            // files keep round-tripping); a real family is stored
                            // by name.
                            params.font_family = (fam != crate::fonts::DEFAULT_FONT_FAMILY)
                                .then(|| fam.clone());
                            changed = true;
                        }
                    }
                });
        });

        ui.horizontal(|ui| {
            ui.label("Size");
            if ui
                .add(
                    egui::DragValue::new(&mut params.font_size)
                        .speed(0.5)
                        .range(1.0..=2000.0)
                        .suffix(" px"),
                )
                .changed()
            {
                changed = true;
            }
        });

        ui.horizontal(|ui| {
            ui.label("Align");
            for a in TextAlign::ALL {
                if ui
                    .selectable_label(params.align == a, a.label())
                    .clicked()
                {
                    params.align = a;
                    changed = true;
                }
            }
        });

        if changed {
            self.checkpoint();
            if let Some(shape) = self.doc.shapes.get_mut(idx) {
                shape.set_text_params(params);
            }
        }

        ui.separator();
        if ui
            .button(format!("{}  Create Outlines", icons::COMPOUND))
            .on_hover_text("Replace the type with editable glyph outlines")
            .clicked()
        {
            self.convert_text_to_outlines();
        }
    }

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

            // Numeric scale + move about the selection centre (one undo step).
            // Shear is available on-canvas (Cmd/Ctrl-drag an edge handle).
            ui.horizontal(|ui| {
                ui.label("Scale");
                ui.add(
                    egui::DragValue::new(&mut self.numeric.scale_x)
                        .speed(0.01)
                        .range(0.01..=100.0)
                        .prefix("x "),
                );
                ui.add(
                    egui::DragValue::new(&mut self.numeric.scale_y)
                        .speed(0.01)
                        .range(0.01..=100.0)
                        .prefix("y "),
                );
            });
            ui.horizontal(|ui| {
                ui.label("Move");
                ui.add(
                    egui::DragValue::new(&mut self.numeric.move_x)
                        .speed(0.5)
                        .suffix(" x"),
                );
                ui.add(
                    egui::DragValue::new(&mut self.numeric.move_y)
                        .speed(0.5)
                        .suffix(" y"),
                );
                if ui
                    .button("Apply")
                    .on_hover_text(
                        "Apply numeric transform (Transform Again, Cmd/Ctrl+D, repeats it)",
                    )
                    .clicked()
                {
                    let nt = self.numeric;
                    self.apply_numeric_transform(nt);
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

    /// Opacity-mask controls: Make (topmost shape's luminance drives the lower
    /// objects' alpha), Release, and Invert. Each is a single undo step; buttons
    /// disable when the gesture is a no-op. Mirrors `Object ▸ Opacity Mask`.
    fn opacity_mask_section(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.add_enabled_ui(self.can_make_omask(), |ui| {
                if ui
                    .button(format!("{}  Make", icons::CLIP_MAKE))
                    .on_hover_text("Mask lower objects by the topmost shape's luminance")
                    .clicked()
                {
                    self.make_omask();
                }
            });
            ui.add_enabled_ui(self.can_release_omask(), |ui| {
                if ui
                    .button(format!("{}  Release", icons::CLIP_RELEASE))
                    .on_hover_text("Release the opacity mask")
                    .clicked()
                {
                    self.release_omask();
                }
            });
        });
        ui.add_enabled_ui(self.can_release_omask(), |ui| {
            if ui
                .button("Invert mask")
                .on_hover_text("Flip the mask: black reveals, white hides")
                .clicked()
            {
                self.toggle_omask_invert();
            }
        });
        if !self.can_make_omask() && !self.can_release_omask() {
            ui.weak("Select 2+ objects; the topmost (white=reveal) masks the rest.");
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

    /// A real Layers panel: every object (and group, as an expandable parent)
    /// listed top-to-bottom in z-order, each row carrying a visibility toggle, a
    /// lock toggle, an editable name, a layer-colour swatch, click-to-target
    /// selection (kept in sync with the canvas), and reorder controls
    /// (front/back + up/down). Hidden / locked rows are dimmed; the gates are
    /// shared with the canvas via [`Shape::selectable`](crate::document::Shape).
    fn layers_panel(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.weak(format!("{}", self.doc.shapes.len()));
            });
        });
        ui.add_space(2.0);

        // Deferred mutations so we don't borrow `self.doc` while iterating.
        let mut to_delete: Option<usize> = None;
        let mut to_toggle_vis: Option<usize> = None;
        let mut to_toggle_lock: Option<usize> = None;
        let mut to_rename: Option<(usize, String)> = None;
        let mut to_recolor: Option<(usize, Option<[f32; 4]>)> = None;
        let mut arrange_one: Option<(usize, Arrange)> = None;
        let mut select_shape: Option<(usize, bool)> = None; // (idx, shift held)
        let mut select_group: Option<u64> = None;
        let mut toggle_group: Option<u64> = None; // collapse / expand

        let shift = ui.input(|i| i.modifiers.shift);
        let n = self.doc.shapes.len();

        // Build the top-to-bottom row layout (front first), grouping members
        // under a collapsible header, via the pure `layers` helper.
        let group_tags: Vec<Option<u64>> = self.doc.shapes.iter().map(|s| s.group()).collect();
        let rows = crate::layers::rows(&group_tags, &self.collapsed_layers);

        for row in rows {
            match row {
                crate::layers::LayerRow::Group { id, count } => {
                    let collapsed = self.collapsed_layers.contains(&id);
                    ui.horizontal(|ui| {
                        let caret = if collapsed {
                            icons::CARET_RIGHT
                        } else {
                            icons::CARET_DOWN
                        };
                        if ui
                            .add(egui::Button::new(caret).frame(false))
                            .on_hover_text(if collapsed { "Expand" } else { "Collapse" })
                            .clicked()
                        {
                            toggle_group = Some(id);
                        }
                        let label = format!("{}  Group ({count})", icons::GROUP);
                        if ui
                            .selectable_label(false, egui::RichText::new(label).strong())
                            .on_hover_text("Select the whole group")
                            .clicked()
                        {
                            select_group = Some(id);
                        }
                    });
                }
                crate::layers::LayerRow::Shape { idx, depth } => {
                    let primary = self.primary() == Some(idx);
                    let secondary = !primary && self.is_selected(idx);
                    let visible = self.doc.shapes[idx].visible();
                    let locked = self.doc.shapes[idx].locked();
                    let name = self.doc.shapes[idx].display_name();
                    let layer_color = self.doc.shapes[idx].layer_color();

                    ui.push_id(("layer", idx), |ui| {
                        ui.horizontal(|ui| {
                            // Indent grouped members under their header.
                            if depth > 0 {
                                ui.add_space(16.0 * depth as f32);
                            }

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
                                to_toggle_vis = Some(idx);
                            }

                            // Lock toggle.
                            let lock = if locked {
                                icons::LOCK
                            } else {
                                icons::LOCK_OPEN
                            };
                            if ui
                                .add(egui::Button::new(lock).frame(false))
                                .on_hover_text(if locked {
                                    "Unlock (locked: can't be selected)"
                                } else {
                                    "Lock"
                                })
                                .clicked()
                            {
                                to_toggle_lock = Some(idx);
                            }

                            // Layer-colour swatch: a small chip; click to set,
                            // right-click to clear back to none.
                            let mut chip = layer_color.unwrap_or([0.6, 0.6, 0.6, 1.0]);
                            let mut c = Color32::from_rgba_unmultiplied(
                                (chip[0] * 255.0) as u8,
                                (chip[1] * 255.0) as u8,
                                (chip[2] * 255.0) as u8,
                                (chip[3] * 255.0) as u8,
                            );
                            let resp = ui
                                .color_edit_button_srgba(&mut c)
                                .on_hover_text("Layer colour (right-click to clear)");
                            if resp.changed() {
                                chip = [
                                    c.r() as f32 / 255.0,
                                    c.g() as f32 / 255.0,
                                    c.b() as f32 / 255.0,
                                    c.a() as f32 / 255.0,
                                ];
                                to_recolor = Some((idx, Some(chip)));
                            }
                            if resp.secondary_clicked() {
                                to_recolor = Some((idx, None));
                            }

                            // Editable name, taking the remaining width before the
                            // reorder buttons. A blank name reverts to the type
                            // label (handled by `set_shape_name`).
                            let mut buf = self.doc.shapes[idx].name().unwrap_or("").to_string();
                            let hint = self.doc.shapes[idx].label();
                            let edit = egui::TextEdit::singleline(&mut buf)
                                .hint_text(hint)
                                .desired_width(96.0);
                            let mut text = egui::RichText::new(&name);
                            if !visible || locked {
                                text = text.weak();
                            }
                            if secondary {
                                text = text.color(theme::accent());
                            }
                            // A selectable label that, when the row is the active
                            // selection, swaps to an inline text editor. The name
                            // is committed on focus-loss / Enter (not per keystroke)
                            // so it is one undo step.
                            if primary {
                                if ui.add(edit).lost_focus() {
                                    to_rename = Some((idx, buf.clone()));
                                }
                            } else if ui
                                .selectable_label(secondary, text)
                                .on_hover_text(if locked { "Locked" } else { "Click to select" })
                                .clicked()
                            {
                                select_shape = Some((idx, shift));
                            }

                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    if ui.button(icons::TRASH).on_hover_text("Delete").clicked() {
                                        to_delete = Some(idx);
                                    }
                                    ui.add_enabled_ui(idx > 0, |ui| {
                                        if ui
                                            .button(icons::CARET_DOWN)
                                            .on_hover_text("Move down")
                                            .clicked()
                                        {
                                            arrange_one = Some((idx, Arrange::SendBackward));
                                        }
                                    });
                                    ui.add_enabled_ui(idx + 1 < n, |ui| {
                                        if ui
                                            .button(icons::CARET_UP)
                                            .on_hover_text("Move up")
                                            .clicked()
                                        {
                                            arrange_one = Some((idx, Arrange::BringForward));
                                        }
                                    });
                                    ui.add_enabled_ui(idx > 0, |ui| {
                                        if ui
                                            .button(icons::LAYER_TO_BACK)
                                            .on_hover_text("Send to back")
                                            .clicked()
                                        {
                                            arrange_one = Some((idx, Arrange::SendToBack));
                                        }
                                    });
                                    ui.add_enabled_ui(idx + 1 < n, |ui| {
                                        if ui
                                            .button(icons::LAYER_TO_FRONT)
                                            .on_hover_text("Bring to front")
                                            .clicked()
                                        {
                                            arrange_one = Some((idx, Arrange::BringToFront));
                                        }
                                    });
                                },
                            );
                        });
                    });
                }
            }
        }
        if n == 0 {
            ui.weak("No shapes yet. Pick a tool and draw.");
        }

        // Apply the deferred mutations (each is its own undo step where it
        // touches the document).
        if let Some(g) = toggle_group {
            crate::layers::toggle_collapsed(&mut self.collapsed_layers, g);
        }
        if let Some(i) = to_toggle_vis {
            self.toggle_shape_visible(i);
        }
        if let Some(i) = to_toggle_lock {
            self.toggle_shape_locked(i);
        }
        if let Some((i, name)) = to_rename {
            self.set_shape_name(i, &name);
        }
        if let Some((i, color)) = to_recolor {
            self.set_shape_layer_color(i, color);
        }
        if let Some((i, op)) = arrange_one {
            // Reorder just this row through the tested arrange ops; the live
            // selection is remapped through the same permutation so it follows.
            self.arrange_shape(i, op);
        }
        if let Some((i, shift)) = select_shape {
            self.select_shape_from_panel(i, shift);
        }
        if let Some(g) = select_group {
            self.select_group_from_panel(g);
        }
        if let Some(i) = to_delete {
            self.checkpoint();
            self.remove_shape(i);
        }
    }
}

/// Editable list of [`Fill`]s for the Appearance panel, shown top-to-bottom (the
/// topmost paint layer first; the model stores bottom-to-top, so the list
/// iterates in reverse). Reorders route through the model's tested
/// [`Appearance::raise_fill`] / [`Appearance::lower_fill`]. Returns `true` if any
/// control changed the stack.
fn appearance_fill_list(ui: &mut egui::Ui, ap: &mut crate::appearance::Appearance) -> bool {
    let mut changed = false;
    let n = ap.fills.len();
    let mut remove: Option<usize> = None;
    let mut raise: Option<usize> = None; // towards top (end of vec)
    let mut lower: Option<usize> = None; // towards bottom (start of vec)

    // Reverse so the topmost layer (highest index) is listed first.
    for idx in (0..n).rev() {
        let fill = &mut ap.fills[idx];
        ui.push_id(("fill", idx), |ui| {
            ui.horizontal(|ui| {
                if ui
                    .add(egui::Button::new(eye_glyph(fill.visible)).frame(false))
                    .on_hover_text("Toggle visibility")
                    .clicked()
                {
                    fill.visible = !fill.visible;
                    changed = true;
                }
                let mut is_grad = matches!(fill.paint, Paint::Gradient(_));
                if paint_swatch_toggle(ui, &mut fill.paint, &mut is_grad) {
                    changed = true;
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.small_button("✕").on_hover_text("Remove fill").clicked() {
                        remove = Some(idx);
                    }
                    // idx+1 < n means there is a layer above to swap with.
                    ui.add_enabled_ui(idx + 1 < n, |ui| {
                        if ui.small_button("▲").on_hover_text("Move up").clicked() {
                            raise = Some(idx);
                        }
                    });
                    ui.add_enabled_ui(idx > 0, |ui| {
                        if ui.small_button("▼").on_hover_text("Move down").clicked() {
                            lower = Some(idx);
                        }
                    });
                });
            });
            // Gradient editor (when this fill is a gradient).
            if let Paint::Gradient(g) = &mut fill.paint {
                changed |= gradient_editor(ui, g);
            }
            changed |= opacity_blend_row(ui, &mut fill.opacity, &mut fill.blend);
        });
        ui.add_space(2.0);
    }

    if let Some(i) = remove {
        ap.fills.remove(i);
        changed = true;
    }
    if let Some(i) = raise {
        changed |= ap.raise_fill(i);
    }
    if let Some(i) = lower {
        changed |= ap.lower_fill(i);
    }
    changed
}

/// Editable list of [`AppStroke`]s for the Appearance panel (top-to-bottom).
fn appearance_stroke_list(ui: &mut egui::Ui, ap: &mut crate::appearance::Appearance) -> bool {
    let mut changed = false;
    let n = ap.strokes.len();
    let mut remove: Option<usize> = None;
    let mut raise: Option<usize> = None;
    let mut lower: Option<usize> = None;

    for idx in (0..n).rev() {
        let stroke = &mut ap.strokes[idx];
        ui.push_id(("stroke", idx), |ui| {
            ui.horizontal(|ui| {
                if ui
                    .add(egui::Button::new(eye_glyph(stroke.visible)).frame(false))
                    .on_hover_text("Toggle visibility")
                    .clicked()
                {
                    stroke.visible = !stroke.visible;
                    changed = true;
                }
                let mut is_grad = matches!(stroke.paint, Paint::Gradient(_));
                if paint_swatch_toggle(ui, &mut stroke.paint, &mut is_grad) {
                    changed = true;
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui
                        .small_button("✕")
                        .on_hover_text("Remove stroke")
                        .clicked()
                    {
                        remove = Some(idx);
                    }
                    ui.add_enabled_ui(idx + 1 < n, |ui| {
                        if ui.small_button("▲").on_hover_text("Move up").clicked() {
                            raise = Some(idx);
                        }
                    });
                    ui.add_enabled_ui(idx > 0, |ui| {
                        if ui.small_button("▼").on_hover_text("Move down").clicked() {
                            lower = Some(idx);
                        }
                    });
                });
            });
            ui.horizontal(|ui| {
                ui.label("Width");
                if ui
                    .add(egui::Slider::new(&mut stroke.width, 0.0..=40.0).suffix(" px"))
                    .changed()
                {
                    changed = true;
                }
            });
            if let Paint::Gradient(g) = &mut stroke.paint {
                changed |= gradient_editor(ui, g);
            }
            changed |= opacity_blend_row(ui, &mut stroke.opacity, &mut stroke.blend);
        });
        ui.add_space(2.0);
    }

    if let Some(i) = remove {
        ap.strokes.remove(i);
        changed = true;
    }
    if let Some(i) = raise {
        changed |= ap.raise_stroke(i);
    }
    if let Some(i) = lower {
        changed |= ap.lower_stroke(i);
    }
    changed
}

/// Editable list of live [`Effect`]s for the Appearance panel, shown
/// top-to-bottom (the model stores bottom-to-top, so the list iterates in
/// reverse). Each effect row carries a label, remove + reorder buttons, and the
/// effect's own parameter editors. Reorders route through the tested
/// [`Appearance::raise_effect`] / [`Appearance::lower_effect`]. Returns `true`
/// if any control changed the stack.
fn appearance_effect_list(ui: &mut egui::Ui, ap: &mut crate::appearance::Appearance) -> bool {
    let mut changed = false;
    let n = ap.effects.len();
    let mut remove: Option<usize> = None;
    let mut raise: Option<usize> = None;
    let mut lower: Option<usize> = None;

    for idx in (0..n).rev() {
        let effect = &mut ap.effects[idx];
        ui.push_id(("effect", idx), |ui| {
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new(effect.label()).strong());
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui
                        .small_button("✕")
                        .on_hover_text("Remove effect")
                        .clicked()
                    {
                        remove = Some(idx);
                    }
                    ui.add_enabled_ui(idx + 1 < n, |ui| {
                        if ui.small_button("▲").on_hover_text("Move up").clicked() {
                            raise = Some(idx);
                        }
                    });
                    ui.add_enabled_ui(idx > 0, |ui| {
                        if ui.small_button("▼").on_hover_text("Move down").clicked() {
                            lower = Some(idx);
                        }
                    });
                });
            });
            changed |= effect_editor(ui, effect);
        });
        ui.add_space(2.0);
    }

    if let Some(i) = remove {
        ap.effects.remove(i);
        changed = true;
    }
    if let Some(i) = raise {
        changed |= ap.raise_effect(i);
    }
    if let Some(i) = lower {
        changed |= ap.lower_effect(i);
    }
    changed
}

/// Per-variant parameter editors for one live [`Effect`]. Returns `true` if any
/// control changed.
fn effect_editor(ui: &mut egui::Ui, effect: &mut Effect) -> bool {
    let mut changed = false;
    match effect {
        Effect::DropShadow {
            dx,
            dy,
            blur,
            color,
            opacity,
        } => {
            ui.horizontal(|ui| {
                ui.label("Offset X");
                changed |= ui
                    .add(egui::DragValue::new(dx).speed(0.5).suffix(" px"))
                    .changed();
                ui.label("Y");
                changed |= ui
                    .add(egui::DragValue::new(dy).speed(0.5).suffix(" px"))
                    .changed();
            });
            ui.horizontal(|ui| {
                ui.label("Blur");
                changed |= ui
                    .add(egui::Slider::new(blur, 0.0..=50.0).suffix(" px"))
                    .changed();
            });
            ui.horizontal(|ui| {
                ui.label("Color");
                let mut col = Color32::from_rgba_unmultiplied(
                    (color[0] * 255.0) as u8,
                    (color[1] * 255.0) as u8,
                    (color[2] * 255.0) as u8,
                    (color[3] * 255.0) as u8,
                );
                if ui.color_edit_button_srgba(&mut col).changed() {
                    *color = [
                        col.r() as f32 / 255.0,
                        col.g() as f32 / 255.0,
                        col.b() as f32 / 255.0,
                        col.a() as f32 / 255.0,
                    ];
                    changed = true;
                }
            });
            ui.horizontal(|ui| {
                ui.label("Opacity");
                changed |= ui
                    .add(egui::Slider::new(opacity, 0.0..=1.0).fixed_decimals(2))
                    .changed();
            });
        }
        Effect::GaussianBlur { radius } => {
            ui.horizontal(|ui| {
                ui.label("Radius");
                changed |= ui
                    .add(egui::Slider::new(radius, 0.0..=50.0).suffix(" px"))
                    .changed();
            });
        }
    }
    changed
}

/// A Solid/Gradient toggle plus the colour swatch for the solid case. Mutates
/// `paint` in place; `is_grad` mirrors the current kind. Returns `true` on change.
fn paint_swatch_toggle(ui: &mut egui::Ui, paint: &mut Paint, is_grad: &mut bool) -> bool {
    let mut changed = false;
    // Solid colour chip (for a solid paint) or a label (for a gradient).
    match paint {
        Paint::Solid(c) => {
            let mut col = Color32::from_rgba_unmultiplied(
                (c[0] * 255.0) as u8,
                (c[1] * 255.0) as u8,
                (c[2] * 255.0) as u8,
                (c[3] * 255.0) as u8,
            );
            if ui.color_edit_button_srgba(&mut col).changed() {
                *c = [
                    col.r() as f32 / 255.0,
                    col.g() as f32 / 255.0,
                    col.b() as f32 / 255.0,
                    col.a() as f32 / 255.0,
                ];
                changed = true;
            }
        }
        Paint::Gradient(_) => {
            ui.label(egui::RichText::new("Gradient").weak());
        }
    }
    // Solid / Gradient kind toggle.
    if ui
        .selectable_label(!*is_grad, "S")
        .on_hover_text("Solid")
        .clicked()
        && *is_grad
    {
        let c = paint.swatch();
        *paint = Paint::Solid(c);
        *is_grad = false;
        changed = true;
    }
    if ui
        .selectable_label(*is_grad, "G")
        .on_hover_text("Gradient")
        .clicked()
        && !*is_grad
    {
        let base = paint.swatch();
        *paint = Paint::Gradient(Gradient::two_stop(
            GradientKind::Linear,
            base,
            [1.0, 1.0, 1.0, 1.0],
        ));
        *is_grad = true;
        changed = true;
    }
    changed
}

/// An opacity slider + blend-mode combo row for one appearance item.
fn opacity_blend_row(ui: &mut egui::Ui, opacity: &mut f32, blend: &mut BlendMode) -> bool {
    let mut changed = false;
    ui.horizontal(|ui| {
        ui.label("Opacity");
        if ui
            .add(egui::Slider::new(opacity, 0.0..=1.0).fixed_decimals(2))
            .changed()
        {
            changed = true;
        }
    });
    ui.horizontal(|ui| {
        ui.label("Blend");
        egui::ComboBox::from_id_salt("blend")
            .selected_text(blend.label())
            .show_ui(ui, |ui| {
                for m in BlendMode::ALL {
                    if ui.selectable_value(blend, m, m.label()).changed() {
                        changed = true;
                    }
                }
            });
    });
    changed
}

/// Eye / eye-slash glyph for an appearance item's visibility toggle.
fn eye_glyph(visible: bool) -> &'static str {
    if visible {
        icons::EYE
    } else {
        icons::EYE_SLASH
    }
}
