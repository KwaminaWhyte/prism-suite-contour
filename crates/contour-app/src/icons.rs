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

// --- Boolean ops ------------------------------------------------------------

/// Union of two shapes.
pub const UNITE: &str = ph::UNITE;
/// Intersection of two shapes.
pub const INTERSECT: &str = ph::INTERSECT;
/// Difference (subtract) of two shapes.
pub const EXCLUDE: &str = ph::EXCLUDE;
