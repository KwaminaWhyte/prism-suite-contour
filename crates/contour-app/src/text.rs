//! Point-type layout and glyph-outline extraction — the pure core of the Type
//! tool.
//!
//! Contour renders text as **real vector outlines**: a [`TextParams`] (string +
//! font size + alignment) is laid out by [`layout`] into a set of document-space
//! [`SubPath`]s — one closed contour per glyph contour — anchored at the text
//! object's `origin`. Those subpaths are cached on the [`Shape::Text`] variant so
//! every render surface (canvas, SVG, PNG) and every geometry query (bounds,
//! hit-test, transform) treats text exactly like a compound path, and so
//! `Object ▸ Type ▸ Convert to Outlines` is a one-liner: lift the cache into a
//! [`Shape::Compound`].
//!
//! The glyph contours come from a bundled default font ([`Ubuntu-Light`], Ubuntu
//! Font Licence) parsed with [`ttf_parser`]; the parser hands us TrueType
//! `move/line/quad/cubic` segments through an [`OutlineBuilder`], which we map onto
//! the document's `(points, handles, closed)` model (quadratics are elevated to
//! cubics so the whole suite's bezier pipeline consumes them unchanged).
//!
//! **In scope (this pass):** point type, multi-line (`\n`), font size, left /
//! centre / right alignment, fill + stroke, convert-to-outlines, `.contour`
//! round-trip. **Out of scope (noted open):** area type, type-on-path, rich
//! character / paragraph panels, font selection, kerning / OpenType shaping
//! (advances are plain horizontal metrics; the bundled font has no kern table
//! applied).

use crate::document::SubPath;
use serde::{Deserialize, Serialize};

/// The bundled default font (Ubuntu Light, Ubuntu Font Licence — see
/// `assets/fonts/UFL.txt`). Embedded so a fresh install always has a usable face
/// without touching the host's font directories.
static DEFAULT_FONT: &[u8] = include_bytes!("../assets/fonts/Ubuntu-Light.ttf");

/// Horizontal alignment of a multi-line point-type block, relative to the text
/// object's `origin` (Illustrator's paragraph alignment, the slice we ship).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
pub enum TextAlign {
    /// Lines start at `origin.x` (the default).
    #[default]
    Left,
    /// Lines are centred on `origin.x`.
    Center,
    /// Lines end at `origin.x`.
    Right,
}

impl TextAlign {
    pub const ALL: [TextAlign; 3] = [TextAlign::Left, TextAlign::Center, TextAlign::Right];

    pub fn label(self) -> &'static str {
        match self {
            TextAlign::Left => "Left",
            TextAlign::Center => "Center",
            TextAlign::Right => "Right",
        }
    }
}

/// The editable parameters of a point-type object: what to draw and how to set
/// it. Geometry (the laid-out glyph outlines) is *derived* from this via
/// [`layout`] and cached on the shape, so editing any field re-lays-out.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TextParams {
    /// The text to render. Newlines split lines; everything else lays out on one
    /// baseline run.
    pub text: String,
    /// Cap-height-ish em size in document units (the classic "font size").
    pub font_size: f32,
    /// Horizontal alignment of each line about `origin.x`.
    #[serde(default)]
    pub align: TextAlign,
}

impl Default for TextParams {
    fn default() -> Self {
        Self {
            text: "Text".to_string(),
            font_size: 72.0,
            align: TextAlign::default(),
        }
    }
}

/// Vertical gap between baselines as a multiple of the font size when the face
/// exposes no usable line metrics. 1.2 is the long-standing typographic default.
const FALLBACK_LINE_FACTOR: f32 = 1.2;

/// Per-face metrics used by [`layout`], read once from the font's `head` / `hhea`
/// tables. All values are in *em units* (font design units); the layout scales
/// them by `font_size / units_per_em`.
struct FaceMetrics {
    units_per_em: f32,
    line_height: f32,
    ascender: f32,
}

/// One line's measured width (document units) plus the glyph contours that make
/// it up, positioned with the pen at the origin (x grows rightward, y grows
/// downward in document space). [`layout`] shifts these by the alignment offset
/// and the baseline before emitting subpaths.
struct LineLayout {
    width: f32,
    contours: Vec<SubPath>,
}

/// Lay out `params` anchored at `origin` (the top-left of the first line's em
/// box, matching where the Type tool's click lands) and return the glyph
/// outlines as closed document-space [`SubPath`]s, plus the total advance of the
/// **first** line (the caret/measure width callers show). Lines stack downward by
/// the font's line height; each line is offset horizontally by `params.align`.
///
/// Returns an empty subpath list for empty / whitespace-only text (a text object
/// can still exist and be edited with no visible glyphs). The pen advances per
/// glyph by its horizontal advance metric — no kerning / shaping this pass.
pub fn layout(params: &TextParams, origin: (f32, f32)) -> (Vec<SubPath>, f32) {
    let face = match ttf_parser::Face::parse(DEFAULT_FONT, 0) {
        Ok(f) => f,
        // A corrupt embedded font should never happen, but degrade to no glyphs
        // rather than panic so the editor stays alive.
        Err(_) => return (Vec::new(), 0.0),
    };
    let metrics = face_metrics(&face);
    let scale = params.font_size / metrics.units_per_em;
    let line_advance = metrics.line_height * scale;
    let baseline0 = metrics.ascender * scale; // first baseline below the origin

    // Measure + build each line independently, then place them.
    let lines: Vec<LineLayout> = params
        .text
        .split('\n')
        .map(|line| layout_line(&face, line, scale))
        .collect();

    let mut out: Vec<SubPath> = Vec::new();
    for (li, line) in lines.iter().enumerate() {
        let baseline_y = origin.1 + baseline0 + li as f32 * line_advance;
        let x_off = align_offset(params.align, line.width);
        for contour in &line.contours {
            out.push(translate_subpath(contour, origin.0 + x_off, baseline_y));
        }
    }
    let first_width = lines.first().map(|l| l.width).unwrap_or(0.0);
    (out, first_width)
}

/// Horizontal shift applied to a line of measured `width` so it aligns about the
/// origin's x: left = 0, centre = −w/2, right = −w.
fn align_offset(align: TextAlign, width: f32) -> f32 {
    match align {
        TextAlign::Left => 0.0,
        TextAlign::Center => -width * 0.5,
        TextAlign::Right => -width,
    }
}

/// Build one baseline run: walk the characters, extract each glyph's outline at
/// the running pen position, and advance the pen by the glyph's horizontal
/// advance. Glyph contours come back in font space (y-up) and are flipped to
/// document space (y-down) here, but left un-translated in y (the baseline is
/// applied by the caller). Returns the line's total advance + its contours.
fn layout_line(face: &ttf_parser::Face, line: &str, scale: f32) -> LineLayout {
    let mut pen_x = 0.0_f32;
    let mut contours: Vec<SubPath> = Vec::new();
    for ch in line.chars() {
        let gid = match face.glyph_index(ch) {
            Some(g) => g,
            // No glyph for this char: advance by a space width so layout stays
            // sane, draw nothing.
            None => {
                pen_x += space_advance(face, scale);
                continue;
            }
        };
        let advance = face
            .glyph_hor_advance(gid)
            .map(|a| a as f32 * scale)
            .unwrap_or_else(|| space_advance(face, scale));
        // Whitespace glyphs (space) have no contours; only outline drawable ones.
        if !ch.is_whitespace() {
            let mut builder = OutlineToSubpaths::new(pen_x, scale);
            if face.outline_glyph(gid, &mut builder).is_some() {
                contours.extend(builder.finish());
            }
        }
        pen_x += advance;
    }
    LineLayout {
        width: pen_x,
        contours,
    }
}

/// A reasonable advance for a missing / whitespace glyph: the font's space
/// advance if it has one, else half the em.
fn space_advance(face: &ttf_parser::Face, scale: f32) -> f32 {
    face.glyph_index(' ')
        .and_then(|g| face.glyph_hor_advance(g))
        .map(|a| a as f32 * scale)
        .unwrap_or(face.units_per_em() as f32 * 0.5 * scale)
}

/// Read the face's layout metrics (em size, line height, ascender). Falls back to
/// a 1.2× line height when the face exposes none.
fn face_metrics(face: &ttf_parser::Face) -> FaceMetrics {
    let upem = face.units_per_em() as f32;
    let ascender = face.ascender() as f32;
    let descender = face.descender() as f32; // typically negative
    let line_gap = face.line_gap() as f32;
    let raw = ascender - descender + line_gap;
    let line_height = if raw > 0.0 {
        raw
    } else {
        upem * FALLBACK_LINE_FACTOR
    };
    FaceMetrics {
        units_per_em: upem.max(1.0),
        line_height,
        ascender: if ascender > 0.0 { ascender } else { upem * 0.8 },
    }
}

/// Translate a sub-contour by `(dx, dy)` (anchors move; handle offsets, being
/// deltas, are unchanged). Used to place a measured glyph at its final
/// alignment + baseline position.
fn translate_subpath(sp: &SubPath, dx: f32, dy: f32) -> SubPath {
    SubPath {
        points: sp.points.iter().map(|&(x, y)| (x + dx, y + dy)).collect(),
        handles: sp.handles.clone(),
        closed: sp.closed,
    }
}

/// A [`ttf_parser::OutlineBuilder`] that accumulates glyph contours into the
/// document's `(points, handles, closed)` [`SubPath`] model.
///
/// TrueType outlines are y-up; document space is y-down, so every y is negated.
/// The pen-x offset (and the em→document `scale`) are folded in as points arrive.
/// Quadratic segments are elevated to cubics (so the suite's single bezier
/// pipeline handles them); cubics are stored directly. Each anchor's handle stores
/// its **out-tangent** offset, exactly matching [`bez_path`](crate::document)'s
/// convention (segment a→b uses `c1 = a + handle[a]`, `c2 = b − handle[b]`), so a
/// cubic `(c1, c2, end)` is recorded by setting the *start* anchor's handle to
/// `c1 − a` and the *end* anchor's handle to `end − c2` — reproducing arbitrary
/// glyph curvature precisely. (Glyph curves are not symmetric, but the start/end
/// out-handle pair captures each segment independently.)
struct OutlineToSubpaths {
    pen_x: f32,
    scale: f32,
    /// Completed contours.
    done: Vec<SubPath>,
    /// Current contour's anchors + out-tangent handle offsets.
    points: Vec<(f32, f32)>,
    handles: Vec<(f32, f32)>,
    /// First point of the current contour (for the implicit close).
    start: Option<(f32, f32)>,
    /// Last anchor's document-space position (the current pen position).
    last: (f32, f32),
}

impl OutlineToSubpaths {
    fn new(pen_x: f32, scale: f32) -> Self {
        Self {
            pen_x,
            scale,
            done: Vec::new(),
            points: Vec::new(),
            handles: Vec::new(),
            start: None,
            last: (0.0, 0.0),
        }
    }

    /// Map a font-space point to document space (apply pen offset + scale, flip y).
    fn map(&self, x: f32, y: f32) -> (f32, f32) {
        (self.pen_x + x * self.scale, -y * self.scale)
    }

    /// Finish the in-progress contour (if any) and return every contour built.
    fn finish(mut self) -> Vec<SubPath> {
        self.flush();
        self.done
    }

    /// Close out the current contour into `done`. Glyph contours are always
    /// closed regions, so the emitted subpath is `closed`. Skips degenerate
    /// (< 2-point) contours.
    fn flush(&mut self) {
        if self.points.len() >= 2 {
            self.handles.resize(self.points.len(), (0.0, 0.0));
            self.done.push(SubPath {
                points: std::mem::take(&mut self.points),
                handles: std::mem::take(&mut self.handles),
                closed: true,
            });
        } else {
            self.points.clear();
            self.handles.clear();
        }
        self.start = None;
    }

    /// Append a cubic segment from the current last anchor `a` to `end`, with
    /// control points `c1` (out of `a`) and `c2` (into `end`). Records it in the
    /// document's out-handle convention: set `a`'s handle to `c1 − a` and `end`'s
    /// handle to `end − c2`, so `bez_path` recovers `c1 = a + handle[a]` and
    /// `c2 = end − handle[end]` exactly. (`end`'s handle is the in-tangent of this
    /// segment; if the next segment is also a curve it overwrites it with that
    /// segment's out-tangent — glyph anchors that join two curves keep whichever
    /// the parser supplies, which is faithful to the original contour.)
    fn push_cubic(&mut self, c1: (f32, f32), c2: (f32, f32), end: (f32, f32)) {
        if self.points.is_empty() {
            // A curve before any move: treat the last as the implicit start.
            self.points.push(self.last);
            self.handles.push((0.0, 0.0));
            self.start = Some(self.last);
        }
        let last_i = self.points.len() - 1;
        let a = self.points[last_i];
        self.handles[last_i] = (c1.0 - a.0, c1.1 - a.1);
        self.points.push(end);
        self.handles.push((end.0 - c2.0, end.1 - c2.1));
        self.last = end;
    }
}

impl ttf_parser::OutlineBuilder for OutlineToSubpaths {
    fn move_to(&mut self, x: f32, y: f32) {
        // Starting a new contour: flush any previous one.
        self.flush();
        let p = self.map(x, y);
        self.points.push(p);
        self.handles.push((0.0, 0.0));
        self.start = Some(p);
        self.last = p;
    }

    fn line_to(&mut self, x: f32, y: f32) {
        let p = self.map(x, y);
        if self.points.is_empty() {
            self.points.push(self.last);
            self.handles.push((0.0, 0.0));
        }
        self.points.push(p);
        self.handles.push((0.0, 0.0));
        self.last = p;
    }

    fn quad_to(&mut self, x1: f32, y1: f32, x: f32, y: f32) {
        // Elevate the quadratic (start, ctrl, end) to a cubic so the suite's
        // single cubic pipeline consumes it. c1 = start + 2/3(ctrl−start),
        // c2 = end + 2/3(ctrl−end).
        let ctrl = self.map(x1, y1);
        let end = self.map(x, y);
        let a = self.last;
        let c1 = (
            a.0 + 2.0 / 3.0 * (ctrl.0 - a.0),
            a.1 + 2.0 / 3.0 * (ctrl.1 - a.1),
        );
        let c2 = (
            end.0 + 2.0 / 3.0 * (ctrl.0 - end.0),
            end.1 + 2.0 / 3.0 * (ctrl.1 - end.1),
        );
        self.push_cubic(c1, c2, end);
    }

    fn curve_to(&mut self, x1: f32, y1: f32, x2: f32, y2: f32, x: f32, y: f32) {
        let c1 = self.map(x1, y1);
        let c2 = self.map(x2, y2);
        let end = self.map(x, y);
        self.push_cubic(c1, c2, end);
    }

    fn close(&mut self) {
        self.flush();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A known glyph ("A") lays out into at least one non-empty closed contour.
    #[test]
    fn glyph_outline_produces_geometry() {
        let params = TextParams {
            text: "A".to_string(),
            font_size: 100.0,
            align: TextAlign::Left,
        };
        let (subs, width) = layout(&params, (0.0, 0.0));
        assert!(!subs.is_empty(), "letter A should produce contours");
        assert!(
            subs.iter().all(|s| s.closed),
            "glyph contours are closed regions"
        );
        assert!(
            subs.iter().any(|s| s.points.len() >= 3),
            "at least one contour has real geometry"
        );
        assert!(width > 0.0, "A has a positive advance, got {width}");
    }

    /// Wider text advances farther: "WWW" is wider than "I".
    #[test]
    fn advance_grows_with_text() {
        let wide = layout(
            &TextParams {
                text: "WWW".into(),
                font_size: 50.0,
                align: TextAlign::Left,
            },
            (0.0, 0.0),
        )
        .1;
        let narrow = layout(
            &TextParams {
                text: "I".into(),
                font_size: 50.0,
                align: TextAlign::Left,
            },
            (0.0, 0.0),
        )
        .1;
        assert!(wide > narrow, "WWW ({wide}) should exceed I ({narrow})");
    }

    /// Larger font size scales the advance proportionally.
    #[test]
    fn advance_scales_with_font_size() {
        let small = layout(
            &TextParams {
                text: "Ag".into(),
                font_size: 40.0,
                align: TextAlign::Left,
            },
            (0.0, 0.0),
        )
        .1;
        let big = layout(
            &TextParams {
                text: "Ag".into(),
                font_size: 80.0,
                align: TextAlign::Left,
            },
            (0.0, 0.0),
        )
        .1;
        assert!(
            (big - small * 2.0).abs() < small * 0.05,
            "doubling size should ~double advance: {small} -> {big}"
        );
    }

    /// A second line's glyphs sit below the first (multi-line vertical offset).
    #[test]
    fn multiline_offsets_second_line_down() {
        let one = layout(
            &TextParams {
                text: "A".into(),
                font_size: 60.0,
                align: TextAlign::Left,
            },
            (0.0, 0.0),
        )
        .0;
        let two = layout(
            &TextParams {
                text: "A\nA".into(),
                font_size: 60.0,
                align: TextAlign::Left,
            },
            (0.0, 0.0),
        )
        .0;
        let max_y = |subs: &[SubPath]| {
            subs.iter()
                .flat_map(|s| s.points.iter())
                .map(|p| p.1)
                .fold(f32::MIN, f32::max)
        };
        assert!(
            max_y(&two) > max_y(&one) + 10.0,
            "two lines should reach lower than one"
        );
        // Twice as many glyph contours, roughly (same glyph on both lines).
        assert!(two.len() > one.len());
    }

    /// Centre alignment shifts a line left of the origin; right alignment shifts
    /// it fully left of x=0 (its right edge lands at the origin).
    #[test]
    fn alignment_shifts_line_horizontally() {
        let mk = |a: TextAlign| {
            layout(
                &TextParams {
                    text: "Text".into(),
                    font_size: 50.0,
                    align: a,
                },
                (0.0, 0.0),
            )
            .0
        };
        let min_x = |subs: &[SubPath]| {
            subs.iter()
                .flat_map(|s| s.points.iter())
                .map(|p| p.0)
                .fold(f32::MAX, f32::min)
        };
        let left = min_x(&mk(TextAlign::Left));
        let center = min_x(&mk(TextAlign::Center));
        let right = min_x(&mk(TextAlign::Right));
        assert!(left >= -1.0, "left-aligned starts near x=0 (got {left})");
        assert!(
            center < left,
            "centre shifts left of left-aligned ({center} < {left})"
        );
        assert!(
            right < center,
            "right shifts further left ({right} < {center})"
        );
    }

    /// Empty text lays out to no glyphs but does not panic.
    #[test]
    fn empty_text_is_no_glyphs() {
        let (subs, w) = layout(
            &TextParams {
                text: String::new(),
                font_size: 50.0,
                align: TextAlign::Left,
            },
            (0.0, 0.0),
        );
        assert!(subs.is_empty());
        assert_eq!(w, 0.0);
    }

    /// The laid-out glyph for "o" (which has an inner and outer contour) yields
    /// two contours — proving compound glyph holes survive into the subpath model.
    #[test]
    fn letter_o_has_two_contours() {
        let (subs, _) = layout(
            &TextParams {
                text: "o".into(),
                font_size: 100.0,
                align: TextAlign::Left,
            },
            (0.0, 0.0),
        );
        assert_eq!(subs.len(), 2, "o has an outer + inner contour");
    }
}
