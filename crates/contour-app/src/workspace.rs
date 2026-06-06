//! Workspace state: which dockable panels are visible, plus the pure formatting
//! that backs the bottom status / context bar.
//!
//! Kept apart from any UI so the panel-toggle bookkeeping and the status-line
//! string building are unit-testable without an egui context. The app's
//! `Window` menu toggles [`Panel`] visibility through [`Workspace`]; the central
//! canvas reads it to decide which side panels to show; and the status bar is
//! built from [`status_line`] / [`zoom_percent`].

use serde::{Deserialize, Serialize};

/// A dockable panel whose visibility the user can toggle from the Window menu.
///
/// The two side panels Contour ships today; the canvas is always shown (it is
/// not a panel) so it is deliberately absent here.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Panel {
    /// The left tool column (Select / Rect / … / Artboard).
    Tools,
    /// The right-hand inspector stack (style, transform, layers, …).
    Inspector,
    /// The left-docked Swatches panel — the document colour palette.
    Swatches,
    /// The bottom status / context bar (zoom %, coords, selection, artboard).
    StatusBar,
}

impl Panel {
    /// Every toggleable panel, in the order the Window menu lists them.
    pub const ALL: [Panel; 4] = [
        Panel::Tools,
        Panel::Inspector,
        Panel::Swatches,
        Panel::StatusBar,
    ];

    /// Human label for the Window menu entry.
    pub fn label(self) -> &'static str {
        match self {
            Panel::Tools => "Tools",
            Panel::Inspector => "Inspector",
            Panel::Swatches => "Swatches",
            Panel::StatusBar => "Status bar",
        }
    }
}

/// Per-panel visibility, with the suite's default (everything shown). The
/// canvas is always present and is not represented here.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct Workspace {
    #[serde(default = "yes")]
    pub tools: bool,
    #[serde(default = "yes")]
    pub inspector: bool,
    /// The Swatches panel. Additive (`#[serde(default = "yes")]`), so an older
    /// saved workspace loads with it shown.
    #[serde(default = "yes")]
    pub swatches: bool,
    #[serde(default = "yes")]
    pub status_bar: bool,
}

fn yes() -> bool {
    true
}

impl Default for Workspace {
    fn default() -> Self {
        Self {
            tools: true,
            inspector: true,
            swatches: true,
            status_bar: true,
        }
    }
}

impl Workspace {
    /// Whether the given panel is currently shown.
    pub fn visible(&self, panel: Panel) -> bool {
        match panel {
            Panel::Tools => self.tools,
            Panel::Inspector => self.inspector,
            Panel::Swatches => self.swatches,
            Panel::StatusBar => self.status_bar,
        }
    }

    /// Mutable handle to a panel's visibility flag, for binding directly to an
    /// egui checkbox.
    pub fn flag_mut(&mut self, panel: Panel) -> &mut bool {
        match panel {
            Panel::Tools => &mut self.tools,
            Panel::Inspector => &mut self.inspector,
            Panel::Swatches => &mut self.swatches,
            Panel::StatusBar => &mut self.status_bar,
        }
    }

    /// Restore the default layout (every panel shown). Backs the Window menu's
    /// "Reset panels" command.
    pub fn reset(&mut self) {
        *self = Workspace::default();
    }

    /// Whether the layout already matches the default (used to disable a
    /// no-op "Reset panels" entry).
    pub fn is_default(&self) -> bool {
        *self == Workspace::default()
    }
}

/// Format a zoom factor (screen-pixels per document-unit) as an integer
/// percentage string, e.g. `1.0 -> "100%"`, `0.255 -> "26%"`.
pub fn zoom_percent(zoom: f32) -> String {
    format!("{}%", (zoom * 100.0).round() as i64)
}

/// Build the status-bar text from the live editor state. Pure so the exact
/// wording is pinned by tests.
///
/// - `cursor`: document-space cursor, or `None` when the pointer is off canvas.
/// - `selection`: number of selected shapes.
/// - `artboard`: active artboard name (empty string falls back to a default).
/// - `zoom`: the view's zoom factor.
///
/// Sections are separated by a middle dot so the bar reads as one line.
pub fn status_line(
    cursor: Option<(f32, f32)>,
    selection: usize,
    artboard: &str,
    zoom: f32,
) -> String {
    let mut parts: Vec<String> = Vec::with_capacity(4);

    parts.push(match cursor {
        Some((x, y)) => format!("X {:.0}  Y {:.0} px", x, y),
        None => "X –  Y – px".to_string(),
    });

    parts.push(match selection {
        0 => "No selection".to_string(),
        1 => "1 selected".to_string(),
        n => format!("{n} selected"),
    });

    let board = if artboard.trim().is_empty() {
        "Artboard"
    } else {
        artboard
    };
    parts.push(board.to_string());

    parts.push(zoom_percent(zoom));

    parts.join("   ·   ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_shows_every_panel() {
        let w = Workspace::default();
        assert!(w.visible(Panel::Tools));
        assert!(w.visible(Panel::Inspector));
        assert!(w.visible(Panel::Swatches));
        assert!(w.visible(Panel::StatusBar));
        assert!(w.is_default());
    }

    #[test]
    fn hiding_one_panel_leaves_the_rest_shown() {
        let mut w = Workspace::default();
        *w.flag_mut(Panel::Inspector) = false;
        assert!(!w.visible(Panel::Inspector));
        assert!(w.visible(Panel::Tools));
        assert!(w.visible(Panel::StatusBar));
        assert!(!w.is_default());
        // Re-showing it restores the default layout.
        *w.flag_mut(Panel::Inspector) = true;
        assert!(w.visible(Panel::Inspector));
        assert!(w.is_default());
    }

    #[test]
    fn flag_mut_targets_the_right_field() {
        let mut w = Workspace::default();
        *w.flag_mut(Panel::Tools) = false;
        assert!(!w.tools);
        assert!(w.inspector);
        assert!(w.status_bar);
    }

    #[test]
    fn reset_restores_all_panels() {
        let mut w = Workspace {
            tools: false,
            inspector: false,
            swatches: false,
            status_bar: false,
        };
        assert!(!w.is_default());
        w.reset();
        assert_eq!(w, Workspace::default());
        assert!(w.is_default());
    }

    #[test]
    fn all_lists_each_panel_once() {
        assert_eq!(Panel::ALL.len(), 4);
        assert!(Panel::ALL.contains(&Panel::Tools));
        assert!(Panel::ALL.contains(&Panel::Inspector));
        assert!(Panel::ALL.contains(&Panel::Swatches));
        assert!(Panel::ALL.contains(&Panel::StatusBar));
    }

    #[test]
    fn zoom_percent_rounds_to_whole_numbers() {
        assert_eq!(zoom_percent(1.0), "100%");
        assert_eq!(zoom_percent(0.5), "50%");
        assert_eq!(zoom_percent(2.5), "250%");
        // 0.255 -> 25.5% -> rounds to 26%.
        assert_eq!(zoom_percent(0.255), "26%");
    }

    #[test]
    fn status_line_reports_cursor_selection_artboard_zoom() {
        let s = status_line(Some((12.4, 88.6)), 3, "Artboard 1", 1.0);
        assert!(s.contains("X 12"), "cursor x: {s}");
        assert!(s.contains("Y 89"), "cursor y rounds: {s}");
        assert!(s.contains("3 selected"), "selection count: {s}");
        assert!(s.contains("Artboard 1"), "artboard name: {s}");
        assert!(s.contains("100%"), "zoom: {s}");
    }

    #[test]
    fn status_line_handles_no_cursor_and_no_selection() {
        let s = status_line(None, 0, "", 0.5);
        assert!(s.contains("X –"), "no-cursor placeholder: {s}");
        assert!(s.contains("No selection"), "{s}");
        // Empty artboard name falls back to a generic label.
        assert!(s.contains("Artboard"), "{s}");
        assert!(s.contains("50%"), "{s}");
    }

    #[test]
    fn status_line_singular_selection() {
        let s = status_line(Some((0.0, 0.0)), 1, "Board", 1.0);
        assert!(s.contains("1 selected"), "{s}");
        assert!(!s.contains("1 selecteds"));
    }
}
