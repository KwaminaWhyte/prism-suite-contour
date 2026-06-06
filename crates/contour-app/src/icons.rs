//! Icon font for Contour's tool palette and UI.
//!
//! Uses [`egui-phosphor`] (Phosphor icons, MIT) which ships a TTF and glyph
//! constants compatible with egui 0.34. We register the font into the
//! Proportional and Monospace families so glyphs render inline with text, then
//! re-export the codepoints under tool-oriented names.

use egui_phosphor::regular as ph;

/// Merge the Phosphor icon font into the context's font definitions.
///
/// Call once at startup with `cc.egui_ctx`.
pub fn install(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();

    fonts.font_data.insert(
        "phosphor".to_owned(),
        std::sync::Arc::new(egui_phosphor::Variant::Regular.font_data()),
    );

    for family in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
        fonts
            .families
            .entry(family)
            .or_default()
            .push("phosphor".to_owned());
    }

    ctx.set_fonts(fonts);
}

// --- Tool glyphs (re-exported Phosphor codepoints) --------------------------

/// Select / move tool (arrow cursor).
pub const SELECT: &str = ph::CURSOR;
/// Rectangle tool.
pub const RECT: &str = ph::SQUARE;
/// Ellipse tool.
pub const ELLIPSE: &str = ph::CIRCLE;
/// Line tool.
pub const LINE: &str = ph::LINE_SEGMENT;
/// Pen / bezier path tool.
pub const PEN: &str = ph::PEN_NIB;

// --- Actions ----------------------------------------------------------------

/// Delete / trash.
pub const TRASH: &str = ph::TRASH;
/// Undo (counter-clockwise arrow).
pub const UNDO: &str = ph::ARROW_COUNTER_CLOCKWISE;
/// Redo (clockwise arrow).
pub const REDO: &str = ph::ARROW_CLOCKWISE;

// --- Layers panel -----------------------------------------------------------

/// Visible (shown) layer.
pub const EYE: &str = ph::EYE;
/// Hidden layer.
pub const EYE_SLASH: &str = ph::EYE_SLASH;
/// Move layer up in paint order.
pub const CARET_UP: &str = ph::CARET_UP;
/// Move layer down in paint order.
pub const CARET_DOWN: &str = ph::CARET_DOWN;

// --- Align & distribute -----------------------------------------------------

/// Align left edges.
pub const ALIGN_LEFT: &str = ph::ALIGN_LEFT;
/// Align horizontal centres.
pub const ALIGN_CENTER_H: &str = ph::ALIGN_CENTER_HORIZONTAL;
/// Align right edges.
pub const ALIGN_RIGHT: &str = ph::ALIGN_RIGHT;
/// Align top edges.
pub const ALIGN_TOP: &str = ph::ALIGN_TOP;
/// Align vertical centres.
pub const ALIGN_CENTER_V: &str = ph::ALIGN_CENTER_VERTICAL;
/// Align bottom edges.
pub const ALIGN_BOTTOM: &str = ph::ALIGN_BOTTOM;
/// Distribute horizontally (spread along X).
pub const DISTRIBUTE_H: &str = ph::ARROWS_OUT_LINE_HORIZONTAL;
/// Distribute vertically (spread along Y).
pub const DISTRIBUTE_V: &str = ph::ARROWS_OUT_LINE_VERTICAL;

// --- Transform --------------------------------------------------------------

/// Rotate clockwise.
pub const ROTATE_CW: &str = ph::ARROW_CLOCKWISE;
/// Rotate counter-clockwise.
pub const ROTATE_CCW: &str = ph::ARROW_COUNTER_CLOCKWISE;
/// Flip horizontally (mirror across a vertical axis).
pub const FLIP_H: &str = ph::FLIP_HORIZONTAL;
/// Flip vertically (mirror across a horizontal axis).
pub const FLIP_V: &str = ph::FLIP_VERTICAL;

// --- Arrange (z-order) ------------------------------------------------------

/// Arrange / stacking-order submenu.
pub const ARRANGE: &str = ph::STACK;
/// Bring to front (jump to the top of the stack).
pub const BRING_TO_FRONT: &str = ph::ARROW_LINE_UP;
/// Send to back (drop to the bottom of the stack).
pub const SEND_TO_BACK: &str = ph::ARROW_LINE_DOWN;
/// Bring forward one step.
pub const BRING_FORWARD: &str = ph::ARROW_UP;
/// Send backward one step.
pub const SEND_BACKWARD: &str = ph::ARROW_DOWN;

// --- Grouping ---------------------------------------------------------------

/// Group the selected shapes into one unit.
pub const GROUP: &str = ph::BOUNDING_BOX;
/// Ungroup (dissolve the group back into loose shapes).
pub const UNGROUP: &str = ph::SELECTION_SLASH;

// --- Boolean ops ------------------------------------------------------------

/// Union of two shapes.
pub const UNITE: &str = ph::UNITE;
/// Intersection of two shapes.
pub const INTERSECT: &str = ph::INTERSECT;
/// Difference (subtract) of two shapes.
pub const EXCLUDE: &str = ph::EXCLUDE;
