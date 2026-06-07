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
/// Direct-Select tool (edit individual anchors / handles).
pub const DIRECT_SELECT: &str = ph::CURSOR_CLICK;
/// Rectangle tool.
pub const RECT: &str = ph::SQUARE;
/// Ellipse tool.
pub const ELLIPSE: &str = ph::CIRCLE;
/// Line tool.
pub const LINE: &str = ph::LINE_SEGMENT;
/// Pen / bezier path tool.
pub const PEN: &str = ph::PEN_NIB;
/// Artboard tool (create / move / resize artboards).
pub const ARTBOARD: &str = ph::FRAME_CORNERS;
/// Eyedropper tool (sample / apply a shape's paint appearance).
pub const EYEDROPPER: &str = ph::EYEDROPPER;
/// Shape Builder tool (interactive merge / subtract across regions).
pub const SHAPE_BUILDER: &str = ph::SELECTION_PLUS;

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
/// Locked layer (cannot be selected / edited).
pub const LOCK: &str = ph::LOCK_SIMPLE;
/// Unlocked layer.
pub const LOCK_OPEN: &str = ph::LOCK_SIMPLE_OPEN;
/// A collapsed group row (disclosure pointing right).
pub const CARET_RIGHT: &str = ph::CARET_RIGHT;
/// Bring to front (top of the stack) — reused in the Layers panel.
pub const LAYER_TO_FRONT: &str = ph::ARROW_LINE_UP;
/// Send to back (bottom of the stack) — reused in the Layers panel.
pub const LAYER_TO_BACK: &str = ph::ARROW_LINE_DOWN;

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

// --- Clipping masks ---------------------------------------------------------

/// Make a clipping mask (crop content to the topmost shape).
pub const CLIP_MAKE: &str = ph::CROP;
/// Release a clipping mask (restore the clipped originals).
pub const CLIP_RELEASE: &str = ph::SCISSORS;

// --- Blend ------------------------------------------------------------------

/// Make a blend (interpolate intermediate objects between two shapes).
pub const BLEND: &str = ph::LINE_SEGMENTS;

// --- Compound path ----------------------------------------------------------

/// Make / release a compound path (an outer ring with holes as one object).
pub const COMPOUND: &str = ph::CIRCLE_DASHED;

// --- Boolean ops ------------------------------------------------------------

/// Union of two shapes.
pub const UNITE: &str = ph::UNITE;
/// Intersection of two shapes.
pub const INTERSECT: &str = ph::INTERSECT;
/// Difference (subtract front) of two shapes.
pub const EXCLUDE: &str = ph::EXCLUDE;
/// Exclude (symmetric difference) of two shapes.
pub const EXCLUDE_OVERLAP: &str = ph::EXCLUDE_SQUARE;
/// Minus Back (subtract the back shape from the front).
pub const MINUS_BACK: &str = ph::SUBTRACT;
/// Divide into every non-overlapping region.
pub const DIVIDE: &str = ph::DIVIDE;
/// Trim hidden parts of the back shape.
pub const TRIM: &str = ph::SCISSORS;
/// Merge abutting faces into one region.
pub const MERGE: &str = ph::STACK;
/// Crop to the overlap region.
pub const CROP: &str = ph::FRAME_CORNERS;
/// Outline the combined boundary as strokes.
pub const OUTLINE: &str = ph::BEZIER_CURVE;
