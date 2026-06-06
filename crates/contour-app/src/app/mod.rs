//! The Contour application: tool state, panels, menus, and the per-frame draw
//! loop that ties the document model to the canvas.

mod edit;
mod inspector;
mod tools;

use crate::align::AlignTo;
use crate::arrange::Arrange;
use crate::canvas::View;
use crate::clipboard::Clipboard;
use crate::document::{Document, Shape, StrokeStyle};
use crate::gradient::{Gradient, GradientKind, GradientStop, SpreadMode};
use crate::history::History;
use crate::snap::{SnapConfig, SnapResult};
use crate::transform::Handle;
use crate::workspace::Workspace;
use crate::{icons, theme};
use egui::{Color32, Vec2};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Tool {
    Select,
    Rect,
    Ellipse,
    Line,
    Pen,
    /// Create / move / resize artboards (Illustrator's Artboard tool).
    Artboard,
    /// Sample a shape's paint appearance and apply it to others (Illustrator's
    /// Eyedropper, `I`).
    Eyedropper,
}

impl Tool {
    fn icon(self) -> &'static str {
        match self {
            Tool::Select => icons::SELECT,
            Tool::Rect => icons::RECT,
            Tool::Ellipse => icons::ELLIPSE,
            Tool::Line => icons::LINE,
            Tool::Pen => icons::PEN,
            Tool::Artboard => icons::ARTBOARD,
            Tool::Eyedropper => icons::EYEDROPPER,
        }
    }
    fn name(self) -> &'static str {
        match self {
            Tool::Select => "Select",
            Tool::Rect => "Rectangle",
            Tool::Ellipse => "Ellipse",
            Tool::Line => "Line",
            Tool::Pen => "Pen",
            Tool::Artboard => "Artboard",
            Tool::Eyedropper => "Eyedropper (I)",
        }
    }
}

/// Which clipboard / duplicate command a key chord requested this frame.
#[derive(Clone, Copy, PartialEq, Eq)]
enum ClipKey {
    Copy,
    Cut,
    Paste,
    PasteInPlace,
    PasteInFront,
    PasteInBack,
    Duplicate,
}

/// The global key chords read once per frame, dispatched into editor commands.
struct Keys {
    enter: bool,
    delete: bool,
    undo: bool,
    redo: bool,
    arrange: Option<Arrange>,
    group: bool,
    ungroup: bool,
    /// `Cmd/Ctrl+7` makes a clipping mask; `Alt+Cmd/Ctrl+7` releases one.
    make_clip: bool,
    release_clip: bool,
    clip: Option<ClipKey>,
    /// Single-key `I` pressed (no modifiers) — activate the Eyedropper tool, à la
    /// Illustrator's per-tool letter shortcuts.
    eyedropper: bool,
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
    /// An in-progress artboard gesture (Artboard tool): creating a new board by
    /// drag, or moving an existing board.
    artboard_drag: Option<ArtboardDrag>,
}

/// An in-progress artboard gesture with the Artboard tool.
enum ArtboardDrag {
    /// Dragging out a brand-new artboard from `start` (document space).
    Create { start: (f32, f32) },
    /// Moving artboard `index`, last seen cursor at `last` (document space).
    Move { index: usize, last: (f32, f32) },
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
    /// Reference frame the Align operations measure against (selection bounds or
    /// the active artboard rectangle).
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
    /// Set by "View → Fit artboards"; consumed next frame by the canvas (which
    /// knows the real content rectangle) to zoom/pan so every artboard fits.
    fit_artboards_requested: bool,
    /// Which dockable panels are shown (Window menu) — tool column, inspector,
    /// and the bottom status bar.
    workspace: Workspace,
    /// Latest document-space cursor over the canvas, or `None` when the pointer
    /// is off canvas. Feeds the status bar's coordinate read-out.
    cursor_doc: Option<(f32, f32)>,
    /// Detached shapes from the last Copy / Cut, ready to paste. Not part of the
    /// document (paste is not undo-coupled to copy), so it survives undo/redo.
    clipboard: Clipboard,
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
            align_to: AlignTo::Selection,
            transform_angle: 45.0,
            snap: SnapConfig::default(),
            show_grid: false,
            show_rulers: true,
            show_guides: true,
            inter: Interaction::default(),
            history: History::new(),
            status: String::new(),
            fit_artboards_requested: false,
            workspace: Workspace::default(),
            cursor_doc: None,
            clipboard: Clipboard::default(),
        }
    }

    fn new_document(&mut self) {
        self.doc = Document::new();
        self.selection.clear();
        self.inter = Interaction::default();
        self.history.clear();
        self.status.clear();
    }
}

impl eframe::App for ContourApp {
    fn ui(&mut self, root: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = root.ctx().clone();

        // Global keyboard: Enter commits a pen path; Delete removes selection;
        // Cmd/Ctrl+Z undoes, Cmd/Ctrl+Shift+Z (or Ctrl+Y) redoes; Cmd/Ctrl+]/[
        // arrange the selection (with Shift: to front / to back), à la Illustrator.
        let keys = ctx.input(|i| {
            let cmd = i.modifiers.command;
            let shift = i.modifiers.shift;
            let alt = i.modifiers.alt;
            let z = i.key_pressed(egui::Key::Z);
            let y = i.key_pressed(egui::Key::Y);
            let g = i.key_pressed(egui::Key::G);
            let seven = i.key_pressed(egui::Key::Num7);
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
            // Clipboard shortcuts (Illustrator parity): Cmd/Ctrl + C/X/V,
            // Shift+V = paste in place, F/B = paste in front/back, D = duplicate.
            let clip = if cmd && i.key_pressed(egui::Key::C) {
                Some(ClipKey::Copy)
            } else if cmd && i.key_pressed(egui::Key::X) {
                Some(ClipKey::Cut)
            } else if cmd && shift && i.key_pressed(egui::Key::V) {
                Some(ClipKey::PasteInPlace)
            } else if cmd && i.key_pressed(egui::Key::V) {
                Some(ClipKey::Paste)
            } else if cmd && i.key_pressed(egui::Key::F) {
                Some(ClipKey::PasteInFront)
            } else if cmd && i.key_pressed(egui::Key::B) {
                Some(ClipKey::PasteInBack)
            } else if cmd && i.key_pressed(egui::Key::D) {
                Some(ClipKey::Duplicate)
            } else {
                None
            };
            Keys {
                enter: i.key_pressed(egui::Key::Enter),
                delete: i.key_pressed(egui::Key::Delete) || i.key_pressed(egui::Key::Backspace),
                undo: cmd && z && !shift,
                redo: (cmd && z && shift) || (cmd && y),
                arrange,
                group: cmd && g && !shift,
                ungroup: cmd && g && shift,
                make_clip: cmd && seven && !alt,
                release_clip: cmd && seven && alt,
                clip,
                // Plain `I` (no command) activates the eyedropper, like
                // Illustrator's single-key tool letters.
                eyedropper: !cmd && i.key_pressed(egui::Key::I),
            }
        });
        let Keys {
            enter,
            delete,
            undo,
            redo,
            arrange: arrange_key,
            group: group_key,
            ungroup: ungroup_key,
            make_clip: make_clip_key,
            release_clip: release_clip_key,
            clip: clip_key,
            eyedropper: eyedropper_key,
        } = keys;
        // `I` switches to the eyedropper (committing any in-progress pen path
        // first, mirroring how clicking a tool button behaves). Guarded so it is
        // ignored while a text field has keyboard focus.
        if eyedropper_key && self.tool != Tool::Eyedropper && !ctx.wants_keyboard_input() {
            if self.tool == Tool::Pen {
                self.commit_pen(false);
            }
            self.tool = Tool::Eyedropper;
        }
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
        // Ungroup before group so a Shift+G frame isn't misread as group.
        if ungroup_key {
            self.ungroup_selection();
        } else if group_key {
            self.group_selection();
        }
        // Release before make so an Alt+Cmd+7 frame isn't misread as make.
        if release_clip_key {
            self.release_clip();
        } else if make_clip_key {
            self.make_clip();
        }
        if let Some(c) = clip_key {
            match c {
                ClipKey::Copy => self.copy_selection(),
                ClipKey::Cut => self.cut_selection(),
                ClipKey::Paste => self.paste(),
                ClipKey::PasteInPlace => self.paste_in_place(),
                ClipKey::PasteInFront => self.paste_in_front(),
                ClipKey::PasteInBack => self.paste_in_back(),
                ClipKey::Duplicate => self.duplicate_selection(),
            }
        }

        self.menu_bar(root);
        // Side panels are shown only when the Window menu enables them; the
        // central canvas always fills whatever space is left.
        if self.workspace.visible(crate::workspace::Panel::Tools) {
            self.tool_palette(root);
        }
        if self.workspace.visible(crate::workspace::Panel::Inspector) {
            self.right_panel(root);
        }
        // The status bar docks to the bottom, between the side panels and the
        // canvas, so it spans the full window width under the artwork.
        if self.workspace.visible(crate::workspace::Panel::StatusBar) {
            self.status_bar(root);
        }
        self.central_canvas(root);
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
