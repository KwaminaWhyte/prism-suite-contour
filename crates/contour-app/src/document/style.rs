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

/// How the stroke band is positioned relative to the path centerline. Mirrors
/// Illustrator's *Align Stroke* (Center / Inside / Outside).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum StrokeAlign {
    /// The stroke straddles the path, half its width on each side (the SVG /
    /// PostScript default, and the existing behaviour).
    #[default]
    Center,
    /// The whole stroke band lies on the *inside* of a closed path (the path is
    /// the stroke's outer edge).
    Inside,
    /// The whole stroke band lies on the *outside* of a closed path (the path is
    /// the stroke's inner edge).
    Outside,
}

impl StrokeAlign {
    pub const ALL: [StrokeAlign; 3] =
        [StrokeAlign::Center, StrokeAlign::Inside, StrokeAlign::Outside];

    pub fn label(self) -> &'static str {
        match self {
            StrokeAlign::Center => "Center",
            StrokeAlign::Inside => "Inside",
            StrokeAlign::Outside => "Outside",
        }
    }

    /// The signed centerline offset (as a multiple of half the stroke width)
    /// that lands the centered stroke band on the requested side. The polygon
    /// offset helper treats *positive* as the polygon's outward (right-hand)
    /// normal, so `Outside` shifts the centerline outward by `+w/2` and
    /// `Inside` inward by `-w/2`; `Center` leaves it on the path.
    pub fn offset_sign(self) -> f32 {
        match self {
            StrokeAlign::Center => 0.0,
            StrokeAlign::Inside => -1.0,
            StrokeAlign::Outside => 1.0,
        }
    }
}

/// A built-in arrowhead marker drawn at the start and/or end of an open stroke.
/// Geometry is *baked* (a small filled / stroked outline at the endpoint), so it
/// is portable to every renderer (canvas, SVG, PNG) without marker defs.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum Arrowhead {
    /// No marker.
    #[default]
    None,
    /// A solid filled triangle pointing along the stroke.
    Triangle,
    /// An open "V" chevron (two strokes), no fill.
    Open,
    /// A filled circle (dot) centered on the endpoint.
    Circle,
}

impl Arrowhead {
    pub const ALL: [Arrowhead; 4] = [
        Arrowhead::None,
        Arrowhead::Triangle,
        Arrowhead::Open,
        Arrowhead::Circle,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Arrowhead::None => "None",
            Arrowhead::Triangle => "Triangle",
            Arrowhead::Open => "Open",
            Arrowhead::Circle => "Circle",
        }
    }
}

/// Default miter limit (matches the SVG / PostScript default of 4).
fn default_miter() -> f32 {
    4.0
}

/// Default arrowhead scale (1.0 == arrowhead sized to the stroke width).
fn default_arrow_scale() -> f32 {
    1.0
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
    /// Where the stroke band sits relative to the path (center / inside /
    /// outside). Additive (`#[serde(default)]` → `Center`), so older files load
    /// with the existing centered behaviour.
    #[serde(default)]
    pub align: StrokeAlign,
    /// Marker drawn at the path's *start* endpoint (open paths only). Additive.
    #[serde(default)]
    pub start_arrow: Arrowhead,
    /// Marker drawn at the path's *end* endpoint (open paths only). Additive.
    #[serde(default)]
    pub end_arrow: Arrowhead,
    /// Arrowhead size as a multiple of the stroke width. Additive
    /// (`#[serde(default = "default_arrow_scale")]` → `1.0`).
    #[serde(default = "default_arrow_scale")]
    pub arrow_scale: f32,
}

impl Default for StrokeStyle {
    fn default() -> Self {
        Self {
            cap: LineCap::default(),
            join: LineJoin::default(),
            miter_limit: default_miter(),
            dash: Vec::new(),
            dash_offset: 0.0,
            align: StrokeAlign::default(),
            start_arrow: Arrowhead::default(),
            end_arrow: Arrowhead::default(),
            arrow_scale: default_arrow_scale(),
        }
    }
}

impl StrokeStyle {
    /// Whether this stroke draws any dashes (a non-empty pattern with at least
    /// one strictly-positive run). A pattern of all-zeros is treated as solid.
    pub fn is_dashed(&self) -> bool {
        self.dash.iter().any(|&d| d > 0.0)
    }

    /// Whether either endpoint carries an arrowhead marker.
    pub fn has_arrows(&self) -> bool {
        self.start_arrow != Arrowhead::None || self.end_arrow != Arrowhead::None
    }

    /// The effective arrowhead scale, clamped to a sane positive range so a
    /// zero / negative value never collapses or inverts a marker.
    pub fn arrow_scale_clamped(&self) -> f32 {
        self.arrow_scale.clamp(0.1, 10.0)
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
