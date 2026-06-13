//! Image Trace: raster → vector via [`vtracer`].
//!
//! vtracer clusters a raster into colour regions and fits a (cubic-spline)
//! outline to each, the same engine Illustrator's Image Trace uses. We drive it
//! with a small [`TraceConfig`] (a **mode preset** plus the handful of knobs that
//! matter — colour precision, speckle filter, corner threshold, path precision,
//! and a B/W threshold) and turn its per-region output into the document's own
//! [`Shape`] path model: one filled, closed [`Shape::Path`] per traced region,
//! ready to be added to the document as a single grouped, undoable block.
//!
//! The vtracer output is a list of `(svg_d, translate_offset, rgba)` triples —
//! each region's outline is an SVG `d` string (`M`/`L`/`C`/`Z`, absolute, but
//! emitted relative to the region's first point with a `transform=translate(…)`
//! that must be added back). The string→geometry step
//! ([`svg_path_to_contours`]) is a pure, dependency-light parser so it can be
//! unit-tested on synthetic `d` strings without invoking vtracer at all, and the
//! same code path serves any future SVG path importer.

use crate::document::{Shape, StrokeStyle};

/// Which tracing preset to use: a binary (single-foreground) trace or a full
/// colour-region trace. Maps onto vtracer's [`ColorMode`](vtracer::ColorMode).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum TraceMode {
    /// One-colour (foreground vs background) trace driven by [`TraceConfig::threshold`].
    #[default]
    BlackWhite,
    /// Multi-region colour trace; each colour cluster becomes its own filled path.
    Color,
}

impl TraceMode {
    /// Menu / status label.
    pub fn label(self) -> &'static str {
        match self {
            TraceMode::BlackWhite => "Black & White",
            TraceMode::Color => "Color",
        }
    }
}

/// The user-facing tracing knobs. Defaults mirror vtracer's own defaults (the
/// values its `Config::default` / `from_preset` use) so a fresh trace matches
/// the upstream tool, with [`TraceConfig::threshold`] added for the B/W mode.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct TraceConfig {
    pub mode: TraceMode,
    /// Colour precision in **bits** (1..=8): higher keeps more distinct colours
    /// (Color mode only). vtracer default is 6.
    pub color_precision: i32,
    /// Discard speckles smaller than this many pixels across (filter speckle).
    /// vtracer default is 4.
    pub filter_speckle: usize,
    /// Corner threshold in **degrees**: a turn sharper than this becomes a corner
    /// rather than a smooth spline. vtracer default is 60.
    pub corner_threshold: i32,
    /// Decimal places kept in fitted coordinates (path precision). vtracer
    /// default is 2.
    pub path_precision: u32,
    /// Luminance cut for B/W mode, 0..=255: pixels darker than this are
    /// foreground. Ignored in Color mode. Default 128 (mid-grey).
    pub threshold: u8,
}

impl Default for TraceConfig {
    fn default() -> Self {
        Self {
            mode: TraceMode::BlackWhite,
            color_precision: 6,
            filter_speckle: 4,
            corner_threshold: 60,
            path_precision: 2,
            threshold: 128,
        }
    }
}

/// One contour of a traced region, in the document's path model: absolute
/// anchor `points`, per-anchor out-tangent `handles` (mirror in-tangent;
/// `(0,0)` = corner), and `closed`. Matches a [`Shape::Path`]'s geometry triple.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct Contour {
    pub points: Vec<(f32, f32)>,
    pub handles: Vec<(f32, f32)>,
    pub closed: bool,
}

impl Contour {
    /// Axis-aligned bounding box `[x, y, w, h]` of the (anchor) points, or
    /// `None` when empty. (Used by the trace tests; kept public as a handy
    /// contour query.)
    #[allow(dead_code)]
    pub fn bbox(&self) -> Option<[f32; 4]> {
        let (mut minx, mut miny) = (f32::INFINITY, f32::INFINITY);
        let (mut maxx, mut maxy) = (f32::NEG_INFINITY, f32::NEG_INFINITY);
        for &(x, y) in &self.points {
            minx = minx.min(x);
            miny = miny.min(y);
            maxx = maxx.max(x);
            maxy = maxy.max(y);
        }
        if self.points.is_empty() {
            return None;
        }
        Some([minx, miny, maxx - minx, maxy - miny])
    }
}

/// Parse an SVG path `d` string (subset: `M`/`L`/`C`/`Z`, absolute coordinates,
/// the exact grammar vtracer emits) plus a translate `offset` into one or more
/// [`Contour`]s — a new contour starts at every `M`. Cubic `C c1 c2 p` segments
/// become a `LineTo`-free curve: the previous anchor takes out-handle `c1 − prev`
/// and the new anchor `p` takes in-handle `c2 − p`, stored as the document's
/// mirror-offset handle (so `bez_path` reconstructs the same cubic). `L`
/// produces a corner anchor; `Z` closes the current contour.
///
/// Pure and vtracer-free, so it is unit-testable on hand-written `d` strings and
/// reusable as a general (line/cubic) SVG path importer.
pub fn svg_path_to_contours(d: &str, offset: (f32, f32)) -> Vec<Contour> {
    let toks = tokenize(d);
    let mut contours: Vec<Contour> = Vec::new();
    let mut cur: Option<Contour> = None;
    let (ox, oy) = offset;
    let mut i = 0;
    // Pull `n` floats starting just after the command token at `i`.
    while i < toks.len() {
        match &toks[i] {
            Tok::Cmd(c) => {
                match c {
                    'M' | 'm' => {
                        if let Some(done) = cur.take() {
                            if !done.points.is_empty() {
                                contours.push(done);
                            }
                        }
                        let (x, y) = (num(&toks, i + 1) + ox, num(&toks, i + 2) + oy);
                        let mut ct = Contour::default();
                        ct.points.push((x, y));
                        ct.handles.push((0.0, 0.0));
                        cur = Some(ct);
                        i += 3;
                    }
                    'L' | 'l' => {
                        let (x, y) = (num(&toks, i + 1) + ox, num(&toks, i + 2) + oy);
                        if let Some(ct) = cur.as_mut() {
                            ct.points.push((x, y));
                            ct.handles.push((0.0, 0.0));
                        }
                        i += 3;
                    }
                    'C' | 'c' => {
                        let c1 = (num(&toks, i + 1) + ox, num(&toks, i + 2) + oy);
                        let c2 = (num(&toks, i + 3) + ox, num(&toks, i + 4) + oy);
                        let p = (num(&toks, i + 5) + ox, num(&toks, i + 6) + oy);
                        if let Some(ct) = cur.as_mut() {
                            // Out-handle of the previous anchor = c1 − prev.
                            if let Some(prev) = ct.points.last().copied() {
                                let last = ct.handles.len() - 1;
                                ct.handles[last] = (c1.0 - prev.0, c1.1 - prev.1);
                            }
                            // New anchor p; its in-handle is c2, stored as the
                            // mirror out-offset (p − c2) so `−offset` ⇒ in = c2.
                            ct.points.push(p);
                            ct.handles.push((p.0 - c2.0, p.1 - c2.1));
                        }
                        i += 7;
                    }
                    'Z' | 'z' => {
                        if let Some(ct) = cur.as_mut() {
                            ct.closed = true;
                            // Drop a final anchor that duplicates the start (the
                            // closing segment is implied by `closed`).
                            if ct.points.len() > 1 {
                                let first = ct.points[0];
                                let last = *ct.points.last().expect("non-empty");
                                if (first.0 - last.0).abs() < 1e-4
                                    && (first.1 - last.1).abs() < 1e-4
                                {
                                    ct.points.pop();
                                    ct.handles.pop();
                                }
                            }
                        }
                        i += 1;
                    }
                    _ => i += 1,
                }
            }
            // A stray number with no command: skip it.
            Tok::Num(_) => i += 1,
        }
    }
    if let Some(done) = cur.take() {
        if !done.points.is_empty() {
            contours.push(done);
        }
    }
    contours
}

/// A token in an SVG path `d` string: a single-letter command or a number.
enum Tok {
    Cmd(char),
    Num(f32),
}

/// Split a path `d` string into command / number tokens. Commands are the SVG
/// letters; everything else is parsed as an `f32` (commas and whitespace are
/// separators, and a leading sign after `e`/`E` is handled by `f32::parse`).
fn tokenize(d: &str) -> Vec<Tok> {
    let mut out = Vec::new();
    let mut num = String::new();
    let flush = |num: &mut String, out: &mut Vec<Tok>| {
        if !num.is_empty() {
            if let Ok(v) = num.parse::<f32>() {
                out.push(Tok::Num(v));
            }
            num.clear();
        }
    };
    for ch in d.chars() {
        match ch {
            'M' | 'm' | 'L' | 'l' | 'C' | 'c' | 'Z' | 'z' | 'H' | 'h' | 'V' | 'v' | 'S' | 's'
            | 'Q' | 'q' | 'T' | 't' | 'A' | 'a' => {
                flush(&mut num, &mut out);
                out.push(Tok::Cmd(ch));
            }
            ',' | ' ' | '\t' | '\n' | '\r' => flush(&mut num, &mut out),
            '-' if !num.is_empty() && !num.ends_with(['e', 'E']) => {
                // A '-' that starts a new number (not an exponent sign).
                flush(&mut num, &mut out);
                num.push('-');
            }
            _ => num.push(ch),
        }
    }
    flush(&mut num, &mut out);
    out
}

/// Read the float token at index `i`, or `0.0` if it is missing / a command
/// (defensive against a malformed `d` string).
fn num(toks: &[Tok], i: usize) -> f32 {
    match toks.get(i) {
        Some(Tok::Num(v)) => *v,
        _ => 0.0,
    }
}

/// One traced region: its filled contours (outer ring plus any holes) and the
/// region's RGBA fill colour. Produced by [`trace_pixels`].
#[derive(Clone, Debug)]
pub struct TracedRegion {
    pub contours: Vec<Contour>,
    pub fill: [f32; 4],
}

/// Trace an RGBA pixel buffer (`width`×`height`, row-major, 4 bytes/pixel) into
/// vector regions with vtracer, using `cfg`. Returns one [`TracedRegion`] per
/// traced colour cluster (B/W mode yields the foreground region). Pure apart
/// from the vtracer call — takes pixels, returns geometry — so the UI layer only
/// has to load the file and add the resulting shapes.
pub fn trace_pixels(
    rgba: &[u8],
    width: usize,
    height: usize,
    cfg: TraceConfig,
) -> Result<Vec<TracedRegion>, String> {
    if width == 0 || height == 0 || rgba.len() < width * height * 4 {
        return Err("empty or undersized image".into());
    }

    // Build vtracer's ColorImage. In B/W mode we pre-threshold to pure black
    // foreground / white background so the binary tracer keys on luminance with
    // the user's cut, rather than vtracer's internal default.
    let mut pixels = Vec::with_capacity(width * height * 4);
    match cfg.mode {
        TraceMode::Color => pixels.extend_from_slice(&rgba[..width * height * 4]),
        TraceMode::BlackWhite => {
            for px in rgba[..width * height * 4].chunks_exact(4) {
                let (r, g, b, a) = (px[0] as f32, px[1] as f32, px[2] as f32, px[3]);
                // Rec.601 luma; fully-transparent pixels read as background.
                let luma = 0.299 * r + 0.587 * g + 0.114 * b;
                let fg = a > 0 && luma < cfg.threshold as f32;
                let v = if fg { 0u8 } else { 255u8 };
                pixels.extend_from_slice(&[v, v, v, 255]);
            }
        }
    }
    let img = visioncortex::ColorImage {
        pixels,
        width,
        height,
    };

    let vcfg = vtracer::Config {
        color_mode: match cfg.mode {
            TraceMode::Color => vtracer::ColorMode::Color,
            TraceMode::BlackWhite => vtracer::ColorMode::Binary,
        },
        hierarchical: vtracer::Hierarchical::Stacked,
        filter_speckle: cfg.filter_speckle,
        color_precision: cfg.color_precision.clamp(1, 8),
        corner_threshold: cfg.corner_threshold,
        path_precision: Some(cfg.path_precision),
        ..vtracer::Config::default()
    };

    let svg = vtracer::convert(img, vcfg)?;
    let mut regions = Vec::with_capacity(svg.paths.len());
    for p in &svg.paths {
        // vtracer emits each compound region relative to its first point, with a
        // translate offset that must be added back to recover absolute coords.
        let (d, off) = p
            .path
            .to_svg_string(true, visioncortex::PointF64 { x: 0.0, y: 0.0 }, None);
        let contours = svg_path_to_contours(&d, (off.x as f32, off.y as f32));
        let contours: Vec<Contour> = contours
            .into_iter()
            .filter(|c| c.points.len() >= 2)
            .collect();
        if contours.is_empty() {
            continue;
        }
        let c = p.color;
        let fill = [
            c.r as f32 / 255.0,
            c.g as f32 / 255.0,
            c.b as f32 / 255.0,
            c.a as f32 / 255.0,
        ];
        regions.push(TracedRegion { contours, fill });
    }
    Ok(regions)
}

/// Turn traced regions into document [`Shape`]s: one closed, filled
/// [`Shape::Path`] per contour, all tagged with a fresh `group` id so the trace
/// result selects / moves as one object. `group_id` should be unique in the
/// document (see [`crate::group::next_group_id`]). Regions with holes contribute one
/// path per ring (the outer and the holes); a future pass could promote a
/// multi-ring region to a `Shape::Compound`, but separate filled rings render
/// and edit cleanly for now.
pub fn regions_to_shapes(regions: &[TracedRegion], group_id: u64) -> Vec<Shape> {
    let mut shapes = Vec::new();
    for region in regions {
        for c in &region.contours {
            if c.points.len() < 2 {
                continue;
            }
            shapes.push(Shape::Path {
                points: c.points.clone(),
                closed: c.closed,
                fill: region.fill,
                fill_gradient: None,
                stroke: [0.0, 0.0, 0.0, 0.0],
                stroke_w: 0.0,
                stroke_style: StrokeStyle::default(),
                handles: c.handles.clone(),
                live: None,
                appearance: None,
                visible: true,
                group: Some(group_id),
                clip: None,
                mask: false,
                omask: None,
                omask_path: false,
                omask_invert: false,
                blend: None,
                blend_step: false,
                name: None,
                locked: false,
                layer_color: None,
            });
        }
    }
    shapes
}

/// Trace `rgba` into ready-to-insert grouped [`Shape`]s in one call: traces with
/// `cfg`, then tags every result path with `group_id`. Returns an empty vec when
/// the trace finds nothing.
pub fn trace_to_shapes(
    rgba: &[u8],
    width: usize,
    height: usize,
    cfg: TraceConfig,
    group_id: u64,
) -> Result<Vec<Shape>, String> {
    let regions = trace_pixels(rgba, width, height, cfg)?;
    Ok(regions_to_shapes(&regions, group_id))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A black `w×h` square on a white `(w+2m)×(h+2m)` field, RGBA.
    fn black_square(field: usize, margin: usize, side: usize) -> (Vec<u8>, usize) {
        let mut buf = vec![255u8; field * field * 4];
        for y in margin..margin + side {
            for x in margin..margin + side {
                let i = (y * field + x) * 4;
                buf[i] = 0;
                buf[i + 1] = 0;
                buf[i + 2] = 0;
                buf[i + 3] = 255;
            }
        }
        (buf, field)
    }

    #[test]
    fn parse_single_closed_cubic() {
        // A unit-ish square traced as cubics, emitted relative with an offset.
        let d = "M0 0 C0 0 10 0 10 0 C10 0 10 10 10 10 C10 10 0 10 0 10 Z";
        let contours = svg_path_to_contours(d, (5.0, 7.0));
        assert_eq!(contours.len(), 1);
        let c = &contours[0];
        assert!(c.closed, "Z should close the contour");
        // First anchor is M + offset.
        assert_eq!(c.points[0], (5.0, 7.0));
        // Handles align with points.
        assert_eq!(c.points.len(), c.handles.len());
        let bb = c.bbox().expect("bbox");
        assert!((bb[2] - 10.0).abs() < 1e-3 && (bb[3] - 10.0).abs() < 1e-3);
    }

    #[test]
    fn parse_handles_are_curve_correct() {
        // One cubic: M(0,0) C c1=(0,4) c2=(4,0) p=(4,4). Out-handle of the start
        // anchor is c1−start=(0,4); in-handle of the end anchor is c2, stored as
        // the mirror offset p−c2=(0,4).
        let d = "M0 0 C0 4 4 0 4 4";
        let c = &svg_path_to_contours(d, (0.0, 0.0))[0];
        assert_eq!(c.points, vec![(0.0, 0.0), (4.0, 4.0)]);
        assert_eq!(c.handles[0], (0.0, 4.0));
        assert_eq!(c.handles[1], (0.0, 4.0));
        // Reconstructed cubic via bez_path matches the original control points.
        let bp = crate::document::bez_path(&c.points, &c.handles, false);
        let mut ctrl = Vec::new();
        for el in bp.iter() {
            if let kurbo::PathEl::CurveTo(a, b, _) = el {
                ctrl.push((a.x as f32, a.y as f32));
                ctrl.push((b.x as f32, b.y as f32));
            }
        }
        assert_eq!(ctrl, vec![(0.0, 4.0), (4.0, 0.0)]);
    }

    #[test]
    fn parse_multiple_subpaths() {
        let d = "M0 0 L10 0 L10 10 Z M2 2 L4 2 L4 4 Z";
        let contours = svg_path_to_contours(d, (0.0, 0.0));
        assert_eq!(contours.len(), 2);
        assert!(contours.iter().all(|c| c.closed));
    }

    #[test]
    fn parse_negative_and_decimal_coords() {
        let d = "M-1.5 2.25 L3 -4 Z";
        let c = &svg_path_to_contours(d, (0.0, 0.0))[0];
        assert_eq!(c.points, vec![(-1.5, 2.25), (3.0, -4.0)]);
    }

    #[test]
    fn trace_bw_square_yields_closed_path_near_bbox() {
        let (buf, field) = black_square(40, 8, 24);
        let regions = trace_pixels(&buf, field, field, TraceConfig::default())
            .expect("trace ok");
        assert!(!regions.is_empty(), "B/W trace should find the square");
        // Some contour should be closed and roughly cover the square bbox.
        let mut best: Option<[f32; 4]> = None;
        for r in &regions {
            for c in &r.contours {
                if c.closed {
                    if let Some(bb) = c.bbox() {
                        let area = bb[2] * bb[3];
                        if best.is_none_or(|b| area > b[2] * b[3]) {
                            best = Some(bb);
                        }
                    }
                }
            }
        }
        let bb = best.expect("a closed contour");
        // Square spans [8,32) in both axes ⇒ ~24px, allow tracer slack.
        assert!(bb[0] >= 4.0 && bb[0] <= 12.0, "x0 {:?}", bb);
        assert!(bb[1] >= 4.0 && bb[1] <= 12.0, "y0 {:?}", bb);
        assert!((bb[2] - 24.0).abs() <= 6.0, "w {:?}", bb);
        assert!((bb[3] - 24.0).abs() <= 6.0, "h {:?}", bb);
    }

    #[test]
    fn trace_color_and_bw_both_produce_shapes() {
        // Two solid colour quadrants on a square field.
        let field = 32usize;
        let mut buf = vec![255u8; field * field * 4];
        for y in 0..field {
            for x in 0..field {
                let i = (y * field + x) * 4;
                let (r, g, b) = if x < field / 2 {
                    (200u8, 30, 30)
                } else {
                    (30, 30, 200)
                };
                buf[i] = r;
                buf[i + 1] = g;
                buf[i + 2] = b;
                buf[i + 3] = 255;
            }
        }
        let color = trace_to_shapes(
            &buf,
            field,
            field,
            TraceConfig {
                mode: TraceMode::Color,
                ..TraceConfig::default()
            },
            1,
        )
        .expect("color trace");
        assert!(!color.is_empty(), "color mode produces shapes");

        let bw = trace_to_shapes(
            &buf,
            field,
            field,
            TraceConfig {
                mode: TraceMode::BlackWhite,
                threshold: 128,
                ..TraceConfig::default()
            },
            2,
        )
        .expect("bw trace");
        assert!(!bw.is_empty(), "bw mode produces shapes");
        // Every traced shape carries the group tag.
        for s in &color {
            assert!(matches!(s, Shape::Path { group: Some(1), .. }));
        }
    }

    #[test]
    fn trace_is_deterministic() {
        let (buf, field) = black_square(40, 8, 24);
        let a = trace_pixels(&buf, field, field, TraceConfig::default()).unwrap();
        let b = trace_pixels(&buf, field, field, TraceConfig::default()).unwrap();
        let pts = |rs: &[TracedRegion]| {
            rs.iter()
                .flat_map(|r| r.contours.iter().map(|c| c.points.clone()))
                .collect::<Vec<_>>()
        };
        assert_eq!(pts(&a), pts(&b), "same input ⇒ same trace");
    }

    #[test]
    fn empty_image_errors() {
        assert!(trace_pixels(&[], 0, 0, TraceConfig::default()).is_err());
    }
}
