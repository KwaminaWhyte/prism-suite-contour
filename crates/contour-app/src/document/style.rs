//! Non-geometry stroke styling: line caps, joins, miter limit, and dashes.

use serde::{Deserialize, Serialize};

/// Line-cap style for the ends of an open stroke (and the ends of every dash).
/// Mirrors SVG's `stroke-linecap` and tiny-skia's `LineCap`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum LineCap {
    /// Square end flush with the endpoint (SVG `butt`).
    #[default]
    Butt,
    /// Half-circle past the endpoint (SVG `round`).
    Round,
    /// Square end projecting half-width past the endpoint (SVG `square`).
    Square,
}

impl LineCap {
    pub const ALL: [LineCap; 3] = [LineCap::Butt, LineCap::Round, LineCap::Square];

    /// Short label for UI.
    pub fn label(self) -> &'static str {
        match self {
            LineCap::Butt => "Butt",
            LineCap::Round => "Round",
            LineCap::Square => "Square",
        }
    }

    /// SVG `stroke-linecap` keyword.
    pub fn svg(self) -> &'static str {
        match self {
            LineCap::Butt => "butt",
            LineCap::Round => "round",
            LineCap::Square => "square",
        }
    }
}

/// Line-join style for the corners where two stroke segments meet. Mirrors
/// SVG's `stroke-linejoin` and tiny-skia's `LineJoin`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum LineJoin {
    /// Sharp corner, clipped to the miter limit (SVG `miter`).
    #[default]
    Miter,
    /// Rounded corner (SVG `round`).
    Round,
    /// Flattened corner (SVG `bevel`).
    Bevel,
}

impl LineJoin {
    pub const ALL: [LineJoin; 3] = [LineJoin::Miter, LineJoin::Round, LineJoin::Bevel];

    pub fn label(self) -> &'static str {
        match self {
            LineJoin::Miter => "Miter",
            LineJoin::Round => "Round",
            LineJoin::Bevel => "Bevel",
        }
    }

    pub fn svg(self) -> &'static str {
        match self {
            LineJoin::Miter => "miter",
            LineJoin::Round => "round",
            LineJoin::Bevel => "bevel",
        }
    }
}

/// Default miter limit (matches the SVG / PostScript default of 4).
fn default_miter() -> f32 {
    4.0
}

/// Non-geometry stroke attributes: caps, joins, miter limit, and a dash
/// pattern. Carried additively on every [`Shape`](super::Shape) via
/// `#[serde(default)]`, so pre-existing `.contour` files load as a solid
/// butt/miter stroke.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct StrokeStyle {
    #[serde(default)]
    pub cap: LineCap,
    #[serde(default)]
    pub join: LineJoin,
    #[serde(default = "default_miter")]
    pub miter_limit: f32,
    /// Dash run lengths in document units (alternating on/off). Empty = solid.
    #[serde(default)]
    pub dash: Vec<f32>,
    /// Phase offset into the dash pattern, in document units.
    #[serde(default)]
    pub dash_offset: f32,
}

impl Default for StrokeStyle {
    fn default() -> Self {
        Self {
            cap: LineCap::default(),
            join: LineJoin::default(),
            miter_limit: default_miter(),
            dash: Vec::new(),
            dash_offset: 0.0,
        }
    }
}

impl StrokeStyle {
    /// Whether this stroke draws any dashes (a non-empty pattern with at least
    /// one strictly-positive run). A pattern of all-zeros is treated as solid.
    pub fn is_dashed(&self) -> bool {
        self.dash.iter().any(|&d| d > 0.0)
    }

    /// Normalize the dash pattern for renderers that need an even-length,
    /// strictly-positive intervals list (tiny-skia, SVG): drops negatives,
    /// and if the count is odd, repeats it so on/off runs alternate cleanly
    /// (matching the SVG `stroke-dasharray` doubling rule). Returns `None` when
    /// the pattern has no positive run (i.e. should render solid).
    pub fn normalized_dash(&self) -> Option<Vec<f32>> {
        if !self.is_dashed() {
            return None;
        }
        let mut runs: Vec<f32> = self.dash.iter().map(|&d| d.max(0.0)).collect();
        if runs.len() % 2 == 1 {
            let doubled = runs.clone();
            runs.extend(doubled);
        }
        Some(runs)
    }
}
