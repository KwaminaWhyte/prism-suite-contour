use super::path::{
    anchors_in_rect, delete_anchor, handle_endpoints, insert_anchor, is_corner, make_corner,
    make_smooth, segment_count, toggle_anchor_smooth,
};
use super::*;
use crate::transform::Affine;

/// Old `.contour` JSON (pre-`visible`/`handles`) must still deserialize,
/// defaulting `visible = true` and `handles = []`.
#[test]
fn loads_legacy_document() {
    let json = r#"{"shapes":[
        {"Rect":{"rect":[0,0,10,10],"fill":[1,0,0,1],"stroke":[0,0,0,1],"stroke_w":2}},
        {"Path":{"points":[[0,0],[10,0],[10,10]],"closed":true,"fill":[0,1,0,1],"stroke":[0,0,0,1],"stroke_w":1}}
    ]}"#;
    let doc: Document = serde_json::from_str(json).unwrap();
    assert_eq!(doc.shapes.len(), 2);
    assert!(doc.shapes[0].visible());
    assert!(doc.shapes[1].visible());
    // A pre-group document loads with every shape ungrouped.
    assert_eq!(doc.shapes[0].group(), None);
    assert_eq!(doc.shapes[1].group(), None);
    // A pre-guides document loads with no guides.
    assert!(doc.guides.is_empty());
    // A pre-artboards document loads with exactly one default 1000×700 board.
    assert_eq!(doc.artboards.len(), 1);
    assert_eq!(doc.artboards[0].rect, [0.0, 0.0, 1000.0, 700.0]);
    assert_eq!(doc.active_artboard, 0);
    assert_eq!(
        doc.active_artboard().map(|a| a.rect),
        Some([0.0, 0.0, 1000.0, 700.0])
    );
    if let Shape::Path { handles, .. } = &doc.shapes[1] {
        assert!(handles.is_empty());
    } else {
        panic!("expected Path");
    }
    // A pre-live-shape document loads with every path as a plain (non-live) path.
    assert_eq!(doc.shapes[1].live_shape(), None);
    // A pre-appearance document loads with no explicit stack on any shape.
    assert!(doc.shapes[0].appearance().is_none());
    assert!(doc.shapes[1].appearance().is_none());
    // A pre-blend document loads with every shape un-blended.
    assert_eq!(doc.shapes[0].blend(), None);
    assert!(!doc.shapes[0].is_blend_step());
}

/// A pre-stroke-options `.contour` (a `stroke_style` with only caps/joins/dash)
/// loads with the new align / arrowhead fields at their defaults (center align,
/// no arrowheads, 1× scale) — so older files render unchanged.
#[test]
fn loads_legacy_stroke_style_with_default_align_and_arrows() {
    // `stroke_style` carries the original fields only; align / start_arrow /
    // end_arrow / arrow_scale are absent and must default.
    let json = r#"{"shapes":[
        {"Line":{"p0":[0,0],"p1":[10,0],"stroke":[0,0,0,1],"stroke_w":2,
                 "stroke_style":{"cap":"Round","join":"Miter","miter_limit":4,"dash":[12,6],"dash_offset":0}}}
    ]}"#;
    let doc: Document = serde_json::from_str(json).unwrap();
    let st = doc.shapes[0].stroke_style();
    assert_eq!(st.cap, LineCap::Round);
    assert!(st.is_dashed(), "legacy dash preserved");
    // New fields default cleanly.
    assert_eq!(st.align, StrokeAlign::Center);
    assert_eq!(st.start_arrow, Arrowhead::None);
    assert_eq!(st.end_arrow, Arrowhead::None);
    assert_eq!(st.arrow_scale, 1.0);
    assert!(!st.has_arrows());
}

/// The new stroke-options fields (align + arrowheads + scale) round-trip through
/// serde on a Shape's `stroke_style`.
#[test]
fn stroke_align_and_arrows_round_trip() {
    let mut s = Shape::Line {
        p0: (0.0, 0.0),
        p1: (10.0, 0.0),
        stroke: [0.0, 0.0, 0.0, 1.0],
        stroke_w: 2.0,
        stroke_style: StrokeStyle::default(),
        appearance: None,
        visible: true,
        group: None,
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
    };
    {
        let st = s.stroke_style_mut();
        st.align = StrokeAlign::Outside;
        st.start_arrow = Arrowhead::Circle;
        st.end_arrow = Arrowhead::Triangle;
        st.arrow_scale = 1.75;
    }
    let doc = Document {
        shapes: vec![s],
        ..Default::default()
    };
    let json = serde_json::to_string(&doc).unwrap();
    let back: Document = serde_json::from_str(&json).unwrap();
    let st = back.shapes[0].stroke_style();
    assert_eq!(st.align, StrokeAlign::Outside);
    assert_eq!(st.start_arrow, Arrowhead::Circle);
    assert_eq!(st.end_arrow, Arrowhead::Triangle);
    assert_eq!(st.arrow_scale, 1.75);
}

/// Blend-set tags round-trip through serde on a Shape (back-compat: the new
/// `blend` / `blend_step` fields are additive, defaulting to un-blended).
#[test]
fn blend_tags_round_trip() {
    let mut s = Shape::Rect {
        rect: [0.0, 0.0, 10.0, 10.0],
        fill: [1.0, 0.0, 0.0, 1.0],
        fill_gradient: None,
        stroke: [0.0, 0.0, 0.0, 1.0],
        stroke_w: 2.0,
        stroke_style: StrokeStyle::default(),
        appearance: None,
        visible: true,
        group: None,
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
    };
    s.set_blend(Some(7));
    s.set_blend_step(true);
    let doc = Document {
        shapes: vec![s],
        ..Default::default()
    };
    let json = serde_json::to_string(&doc).unwrap();
    let back: Document = serde_json::from_str(&json).unwrap();
    assert_eq!(back.shapes[0].blend(), Some(7));
    assert!(back.shapes[0].is_blend_step());
}

/// Build a live polygon / star `Shape::Path` centred at the origin (the form the
/// Polygon / Star tools create). Mirrors `ContourApp::live_shape_at`.
fn live_path(live: crate::liveshape::LiveShape) -> Shape {
    let (points, handles) = live.outline((0.0, 0.0));
    Shape::Path {
        points,
        closed: true,
        fill: [0.0; 4],
        fill_gradient: None,
        stroke: [0.0, 0.0, 0.0, 1.0],
        stroke_w: 1.0,
        stroke_style: StrokeStyle::default(),
        handles,
        live: Some(live),
        appearance: None,
        visible: true,
        group: None,
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
    }
}

/// A live polygon / star's parameters round-trip through serde and keep the
/// generated geometry; the layer label follows the live kind.
#[test]
fn live_shape_round_trips_and_labels() {
    use crate::liveshape::LiveShape;
    let doc = Document {
        shapes: vec![
            live_path(LiveShape::Polygon {
                sides: 6,
                radius: 50.0,
            }),
            live_path(LiveShape::Star {
                points: 5,
                radius: 40.0,
                inner_ratio: 0.5,
            }),
        ],
        ..Default::default()
    };
    assert_eq!(doc.shapes[0].label(), "Polygon");
    assert_eq!(doc.shapes[1].label(), "Star");

    let json = serde_json::to_string(&doc).unwrap();
    let back: Document = serde_json::from_str(&json).unwrap();
    assert_eq!(
        back.shapes[0].live_shape(),
        Some(LiveShape::Polygon {
            sides: 6,
            radius: 50.0
        })
    );
    assert_eq!(
        back.shapes[1].live_shape(),
        Some(LiveShape::Star {
            points: 5,
            radius: 40.0,
            inner_ratio: 0.5
        })
    );
    // The polygon's six vertices survived serialization.
    if let Shape::Path { points, .. } = &back.shapes[0] {
        assert_eq!(points.len(), 6);
    } else {
        panic!("expected Path");
    }
}

/// Editing a live shape's parameters regenerates its outline about the current
/// centre (so a moved shape stays put), and directly editing an anchor demotes
/// it to a plain path.
#[test]
fn live_shape_regenerates_and_demotes_on_anchor_edit() {
    use crate::liveshape::LiveShape;
    let mut s = live_path(LiveShape::Polygon {
        sides: 4,
        radius: 10.0,
    });
    // Move it, then bump the side count: the new outline keeps the moved centre.
    s.translate(100.0, 0.0);
    assert!(s.set_live_shape(LiveShape::Polygon {
        sides: 8,
        radius: 10.0,
    }));
    if let Shape::Path { points, .. } = &s {
        assert_eq!(points.len(), 8, "regenerated to the new side count");
        let cx = points.iter().map(|p| p.0).sum::<f32>() / points.len() as f32;
        assert!((cx - 100.0).abs() < 1e-2, "centre stayed at the moved x");
    } else {
        panic!("expected Path");
    }
    // Directly editing an anchor drops the live parameters (it becomes a plain
    // editable path, Illustrator-style).
    assert!(s.set_anchor(0, 0, 5.0, 5.0));
    assert_eq!(s.live_shape(), None);
}

/// A shape with no explicit `appearance` migrates its legacy single fill/stroke
/// into a one-fill / one-stroke effective stack on demand.
#[test]
fn legacy_shape_migrates_to_one_fill_one_stroke() {
    let json = r#"{"shapes":[
        {"Rect":{"rect":[0,0,10,10],"fill":[1,0,0,1],"stroke":[0,0,0,1],"stroke_w":2}}
    ]}"#;
    let doc: Document = serde_json::from_str(json).unwrap();
    let ap = doc.shapes[0].effective_appearance();
    assert_eq!(ap.fills.len(), 1, "one fill migrated from the legacy fill");
    assert_eq!(ap.strokes.len(), 1, "one stroke migrated from the legacy stroke");
    assert_eq!(ap.fills[0].paint.swatch(), [1.0, 0.0, 0.0, 1.0]);
    assert_eq!(ap.strokes[0].width, 2.0);
}

/// An explicit stacked appearance round-trips through serde on a Shape and is
/// preferred over the legacy fields by `effective_appearance`.
#[test]
fn appearance_round_trips_on_shape_and_overrides_legacy() {
    use crate::appearance::{Appearance, Fill};
    let mut s = Shape::Rect {
        rect: [0.0, 0.0, 10.0, 10.0],
        fill: [1.0, 0.0, 0.0, 1.0],
        fill_gradient: None,
        stroke: [0.0, 0.0, 0.0, 1.0],
        stroke_w: 2.0,
        stroke_style: StrokeStyle::default(),
        appearance: None,
        visible: true,
        group: None,
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
    };
    // Two stacked fills override the single legacy red fill.
    s.set_appearance(Some(Appearance {
        fills: vec![
            Fill::solid([0.0, 1.0, 0.0, 1.0]),
            Fill::solid([0.0, 0.0, 1.0, 0.5]),
        ],
        strokes: vec![],
        effects: vec![],
    }));
    let doc = Document {
        shapes: vec![s],
        ..Default::default()
    };
    let json = serde_json::to_string(&doc).unwrap();
    let back: Document = serde_json::from_str(&json).unwrap();
    let ap = back.shapes[0].effective_appearance();
    assert_eq!(ap.fills.len(), 2, "stacked fills survive the round-trip");
    assert_eq!(ap.fills[1].paint.swatch(), [0.0, 0.0, 1.0, 0.5]);
    // The legacy `fill` field is untouched but ignored when a stack is present.
    assert_eq!(back.shapes[0].fill_color(), Some([1.0, 0.0, 0.0, 1.0]));
}

/// A gradient fill (with the new Angle kind, perceptual interpolation, dither,
/// multi-stop + per-stop opacity) round-trips through serde on a Shape unchanged.
#[test]
fn gradient_fill_round_trips_on_shape() {
    use crate::gradient::{Gradient, GradientKind, GradientStop, Interpolation, SpreadMode};
    let grad = Gradient {
        kind: GradientKind::Angle,
        stops: vec![
            GradientStop::new(0.0, [1.0, 0.0, 0.0, 1.0]),
            GradientStop::new(0.5, [0.0, 1.0, 0.0, 0.5]),
            GradientStop::new(1.0, [0.0, 0.0, 1.0, 0.0]),
        ],
        angle: 45.0,
        spread: SpreadMode::Reflect,
        interpolation: Interpolation::Perceptual,
        dither: true,
    };
    let mut s = Shape::Rect {
        rect: [0.0, 0.0, 10.0, 10.0],
        fill: [1.0, 0.0, 0.0, 1.0],
        fill_gradient: None,
        stroke: [0.0, 0.0, 0.0, 1.0],
        stroke_w: 2.0,
        stroke_style: StrokeStyle::default(),
        appearance: None,
        visible: true,
        group: None,
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
    };
    s.set_fill_gradient(Some(grad.clone()));
    let doc = Document {
        shapes: vec![s],
        ..Default::default()
    };
    let json = serde_json::to_string(&doc).unwrap();
    let back: Document = serde_json::from_str(&json).unwrap();
    assert_eq!(back.shapes[0].fill_gradient(), Some(&grad));
}

/// A legacy gradient JSON (predating the `interpolation` / `dither` fields) loads
/// with the back-compat defaults — sRGB interpolation + dither off — so older
/// `.contour` files render byte-identically to how they were authored.
#[test]
fn legacy_gradient_loads_with_back_compat_defaults() {
    use crate::gradient::{GradientKind, Interpolation};
    // Note: no `interpolation` / `dither` keys, mirroring a file saved before the
    // perceptual/dither feature landed.
    let json = r#"{"shapes":[
        {"Rect":{"rect":[0,0,10,10],"fill":[1,0,0,1],"stroke":[0,0,0,1],"stroke_w":1,
                 "fill_gradient":{"kind":"Linear",
                   "stops":[{"offset":0.0,"color":[0,0,0,1]},{"offset":1.0,"color":[1,1,1,1]}],
                   "angle":0.0,"spread":"Pad"}}}
    ]}"#;
    let doc: Document = serde_json::from_str(json).unwrap();
    let g = doc.shapes[0].fill_gradient().expect("gradient loaded");
    assert_eq!(g.kind, GradientKind::Linear);
    assert_eq!(g.stops.len(), 2);
    // The absent fields take their back-compat defaults.
    assert_eq!(g.interpolation, Interpolation::Srgb);
    assert!(!g.dither);
}

/// Guides round-trip through serde and load back as the same variant.
#[test]
fn guides_round_trip() {
    let mut doc = Document::new();
    doc.guides.push(Guide::Vertical(100.0));
    doc.guides.push(Guide::Horizontal(42.5));
    let json = serde_json::to_string(&doc).unwrap();
    let back: Document = serde_json::from_str(&json).unwrap();
    assert_eq!(
        back.guides,
        vec![Guide::Vertical(100.0), Guide::Horizontal(42.5)]
    );
}

/// Placed images carried on the document round-trip through serde, and a fresh
/// document has none.
#[test]
fn placed_images_round_trip_on_document() {
    use crate::placed_image::{ImageSource, PlacedImage};
    let doc = Document::new();
    assert!(doc.placed_images.is_empty(), "fresh document places no images");

    let mut doc = Document::new();
    let id = doc.placed_images.place(
        "logo",
        ImageSource::Embedded {
            width: 2,
            height: 2,
            rgba: vec![1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16],
        },
        10.0,
        20.0,
    );
    doc.placed_images
        .get_mut(id)
        .unwrap()
        .set_clip(vec![(0.0, 0.0), (5.0, 0.0), (5.0, 5.0)]);
    doc.placed_images.list.push(PlacedImage::new(
        9,
        "linked",
        ImageSource::Linked {
            path: std::path::PathBuf::from("/tmp/a.png"),
            width: 4,
            height: 3,
        },
        1.0,
        2.0,
    ));
    let json = serde_json::to_string(&doc).unwrap();
    let back: Document = serde_json::from_str(&json).unwrap();
    assert_eq!(back.placed_images.len(), 2);
    assert!(back.placed_images.get(id).unwrap().clip.is_some());
    assert_eq!(
        back.placed_images.list[1].source.natural_size(),
        (4, 3),
        "linked natural size preserved"
    );
}

/// A pre-Place `.contour` (JSON missing the `placed_images` key) loads with an
/// empty placed-image collection — the additive default.
#[test]
fn legacy_document_loads_without_placed_images() {
    // A minimal legacy document: just a shapes list (the other fields are all
    // `#[serde(default)]`).
    let json = r#"{ "shapes": [] }"#;
    let doc: Document = serde_json::from_str(json).unwrap();
    assert!(doc.placed_images.is_empty());
}

/// A fresh document has exactly one default artboard, active.
#[test]
fn fresh_document_has_one_default_artboard() {
    let doc = Document::new();
    assert_eq!(doc.artboards.len(), 1);
    assert_eq!(doc.artboards[0].rect, [0.0, 0.0, 1000.0, 700.0]);
    assert_eq!(doc.active_artboard, 0);
}

/// Artboards and the active index round-trip through serde unchanged.
#[test]
fn artboards_round_trip() {
    let mut doc = Document::new();
    doc.artboards.push(crate::artboard::Artboard::new(
        "Mobile",
        [1100.0, 0.0, 375.0, 812.0],
    ));
    doc.active_artboard = 1;
    let json = serde_json::to_string(&doc).unwrap();
    let back: Document = serde_json::from_str(&json).unwrap();
    assert_eq!(back.artboards.len(), 2);
    assert_eq!(back.artboards[1].name, "Mobile");
    assert_eq!(back.artboards[1].rect, [1100.0, 0.0, 375.0, 812.0]);
    assert_eq!(back.active_artboard, 1);
}

/// `normalize_artboards` repairs an empty stack / out-of-range active index.
#[test]
fn normalize_repairs_artboards() {
    let mut doc = Document::new();
    doc.artboards.clear(); // simulate a corrupt / hand-edited file
    doc.active_artboard = 9;
    doc.normalize_artboards();
    assert_eq!(doc.artboards.len(), 1);
    assert_eq!(doc.active_artboard, 0);

    // Out-of-range active with valid boards clamps to the last board.
    let mut doc2 = Document::new();
    doc2.artboards
        .push(crate::artboard::Artboard::new("b", [0.0, 0.0, 10.0, 10.0]));
    doc2.active_artboard = 7;
    doc2.normalize_artboards();
    assert_eq!(doc2.active_artboard, 1);
}

#[test]
fn rects_intersect_basic_cases() {
    let a = [0.0, 0.0, 10.0, 10.0];
    // Overlapping.
    assert!(rects_intersect(&a, &[5.0, 5.0, 10.0, 10.0]));
    // Fully inside.
    assert!(rects_intersect(&a, &[2.0, 2.0, 2.0, 2.0]));
    // Contains a (b bigger).
    assert!(rects_intersect(&a, &[-5.0, -5.0, 100.0, 100.0]));
    // Edge-touching counts.
    assert!(rects_intersect(&a, &[10.0, 0.0, 5.0, 5.0]));
    // Disjoint on x.
    assert!(!rects_intersect(&a, &[20.0, 0.0, 5.0, 5.0]));
    // Disjoint on y.
    assert!(!rects_intersect(&a, &[0.0, 20.0, 5.0, 5.0]));
}

/// A path with no handles flattens to its raw points (polyline).
#[test]
fn flatten_polyline_is_identity() {
    let pts = vec![(0.0, 0.0), (10.0, 0.0), (10.0, 10.0)];
    let out = flatten(&pts, &[], false);
    assert_eq!(out, pts);
}

/// A path with a non-zero handle flattens into more segments (a curve).
#[test]
fn flatten_curve_subdivides() {
    let pts = vec![(0.0, 0.0), (100.0, 0.0)];
    let handles = vec![(0.0, 50.0), (0.0, 50.0)];
    let out = flatten(&pts, &handles, false);
    assert!(out.len() > 2, "curve should subdivide, got {}", out.len());
}

// --- Direct-select path editing ----------------------------------------

#[test]
fn segment_count_open_vs_closed() {
    assert_eq!(segment_count(0, false), 0);
    assert_eq!(segment_count(0, true), 0);
    assert_eq!(segment_count(3, false), 2);
    assert_eq!(segment_count(3, true), 3);
}

#[test]
fn nearest_segment_picks_closest() {
    // Square-ish open path; click near the middle of the first segment.
    let pts = vec![(0.0, 0.0), (100.0, 0.0), (100.0, 100.0)];
    let (seg, t) = nearest_segment(&pts, false, 50.0, 1.0, 5.0).expect("hit");
    assert_eq!(seg, 0);
    assert!((t - 0.5).abs() < 1e-3, "t={t}");
}

#[test]
fn nearest_segment_misses_when_far() {
    let pts = vec![(0.0, 0.0), (100.0, 0.0)];
    assert!(nearest_segment(&pts, false, 50.0, 50.0, 5.0).is_none());
}

#[test]
fn nearest_segment_closed_uses_wrap_segment() {
    // Triangle; click near the closing edge (last anchor back to first).
    let pts = vec![(0.0, 0.0), (100.0, 0.0), (50.0, 100.0)];
    // Midpoint of closing segment (idx 2): (25, 50).
    let (seg, _t) = nearest_segment(&pts, true, 25.0, 50.0, 5.0).expect("hit");
    assert_eq!(seg, 2);
}

#[test]
fn insert_anchor_splits_straight_segment_at_midpoint() {
    let mut pts = vec![(0.0, 0.0), (100.0, 0.0)];
    let mut handles = vec![(0.0, 0.0), (0.0, 0.0)];
    let idx = insert_anchor(&mut pts, &mut handles, false, 0, 0.5).expect("inserted");
    assert_eq!(idx, 1);
    assert_eq!(pts.len(), 3);
    assert_eq!(handles.len(), 3);
    assert_eq!(pts[1], (50.0, 0.0));
    // New anchor on a straight segment is a corner.
    assert!(is_corner(&handles, 1));
}

#[test]
fn insert_anchor_on_curve_preserves_shape() {
    // A cubic segment; inserting at t splits it via de Casteljau, so the new
    // anchor must land exactly on the original cubic evaluated at t, and the
    // endpoints must be untouched.
    let a = (0.0, 0.0);
    let b = (100.0, 0.0);
    let pts = vec![a, b];
    let handles = vec![(30.0, 60.0), (30.0, -60.0)]; // both smooth

    // Original cubic control points (mirror in-handle of b).
    let c1 = (a.0 + handles[0].0, a.1 + handles[0].1);
    let c2 = (b.0 - handles[1].0, b.1 - handles[1].1);
    let t = 0.5_f32;
    let cubic = |t: f32| {
        let mt = 1.0 - t;
        let x = mt * mt * mt * a.0
            + 3.0 * mt * mt * t * c1.0
            + 3.0 * mt * t * t * c2.0
            + t * t * t * b.0;
        let y = mt * mt * mt * a.1
            + 3.0 * mt * mt * t * c1.1
            + 3.0 * mt * t * t * c2.1
            + t * t * t * b.1;
        (x, y)
    };
    let expected = cubic(t);

    let mut pts2 = pts.clone();
    let mut handles2 = handles.clone();
    let idx = insert_anchor(&mut pts2, &mut handles2, false, 0, t).expect("inserted");
    assert_eq!(idx, 1);
    assert_eq!(pts2.len(), 3);

    // Endpoints unchanged.
    assert_eq!(pts2[0], a);
    assert_eq!(pts2[2], b);
    // New anchor lies exactly on the original cubic at t.
    let mid = pts2[1];
    assert!(
        (mid.0 - expected.0).abs() < 1e-3 && (mid.1 - expected.1).abs() < 1e-3,
        "inserted {mid:?} != on-curve {expected:?}"
    );

    // And the split halves still trace the original curve: sample several
    // points on the new (two-segment) path against the original cubic.
    let after = flatten(&pts2, &handles2, false);
    for &(x, y) in &after {
        // nearest distance from this point to the original cubic (dense sample)
        let mut min_d = f32::INFINITY;
        for s in 0..=200 {
            let cp = cubic(s as f32 / 200.0);
            min_d = min_d.min((x - cp.0).hypot(y - cp.1));
        }
        assert!(
            min_d < 0.5,
            "split point ({x},{y}) off original curve by {min_d}"
        );
    }
}

#[test]
fn delete_anchor_keeps_min_two_points() {
    let mut pts = vec![(0.0, 0.0), (10.0, 0.0), (20.0, 0.0)];
    let mut handles = vec![(0.0, 0.0); 3];
    assert!(delete_anchor(&mut pts, &mut handles, 1));
    assert_eq!(pts, vec![(0.0, 0.0), (20.0, 0.0)]);
    assert_eq!(handles.len(), 2);
    // Now at 2 points: refuse to delete further.
    assert!(!delete_anchor(&mut pts, &mut handles, 0));
    assert_eq!(pts.len(), 2);
}

#[test]
fn toggle_anchor_corner_to_smooth_and_back() {
    let pts = vec![(0.0, 0.0), (100.0, 0.0), (200.0, 0.0)];
    let mut handles = vec![(0.0, 0.0); 3];
    // Middle anchor, neighbours straddle horizontally -> horizontal tangent.
    let now_smooth = toggle_anchor_smooth(&pts, &mut handles, false, 1);
    assert!(now_smooth);
    assert!(!is_corner(&handles, 1));
    // Tangent should be ~horizontal (dir prev->next is +x).
    let (hx, hy) = handles[1];
    assert!(hx > 0.0 && hy.abs() < 1e-3, "handle=({hx},{hy})");
    // Toggle again -> corner.
    let now_smooth = toggle_anchor_smooth(&pts, &mut handles, false, 1);
    assert!(!now_smooth);
    assert!(is_corner(&handles, 1));
}

#[test]
fn toggle_anchor_endpoint_uses_single_neighbour() {
    let pts = vec![(0.0, 0.0), (100.0, 0.0)];
    let mut handles = vec![(0.0, 0.0); 2];
    // First anchor of an open path: tangent toward the only neighbour.
    let now_smooth = toggle_anchor_smooth(&pts, &mut handles, false, 0);
    assert!(now_smooth);
    let (hx, hy) = handles[0];
    assert!(hx > 0.0 && hy.abs() < 1e-3);
}

// --- Direct-Select: marquee, handle math, convert, compound editing ----

#[test]
fn anchors_in_rect_selects_only_contained_anchors() {
    let pts = vec![(0.0, 0.0), (10.0, 0.0), (10.0, 10.0), (50.0, 50.0)];
    // A box covering the first three anchors but not the far one.
    let inside = anchors_in_rect(&pts, &[-1.0, -1.0, 12.0, 12.0]);
    assert_eq!(inside, vec![0, 1, 2]);
    // Edge-touching counts (anchor exactly on the boundary).
    let edge = anchors_in_rect(&pts, &[10.0, 0.0, 40.0, 50.0]);
    assert!(edge.contains(&1) && edge.contains(&3));
    // A box catching nothing.
    assert!(anchors_in_rect(&pts, &[100.0, 100.0, 5.0, 5.0]).is_empty());
}

#[test]
fn anchors_in_rect_normalises_negative_extent() {
    let pts = vec![(5.0, 5.0), (50.0, 50.0)];
    // A box dragged "up-left" (negative w/h) still selects by its real extent.
    let sel = anchors_in_rect(&pts, &[10.0, 10.0, -10.0, -10.0]);
    assert_eq!(sel, vec![0]);
}

#[test]
fn handle_endpoints_mirror_about_anchor() {
    let pts = vec![(10.0, 10.0), (50.0, 10.0)];
    let handles = vec![(5.0, -8.0), (0.0, 0.0)];
    // Smooth anchor: out = anchor + offset, in = anchor − offset (mirror).
    let (out, inp) = handle_endpoints(&pts, &handles, 0).expect("has handle");
    assert_eq!(out, (15.0, 2.0));
    assert_eq!(inp, (5.0, 18.0));
    // Corner anchor: no handle endpoints.
    assert!(handle_endpoints(&pts, &handles, 1).is_none());
}

#[test]
fn make_corner_drops_handle_make_smooth_adds_mirror() {
    let pts = vec![(0.0, 0.0), (100.0, 0.0), (200.0, 0.0)];
    let mut handles = vec![(0.0, 0.0); 3];
    // Corner → smooth: middle anchor gets a non-zero (mirrored) tangent.
    assert!(make_smooth(&pts, &mut handles, false, 1));
    assert!(!is_corner(&handles, 1));
    let (hx, hy) = handles[1];
    assert!(hx > 0.0 && hy.abs() < 1e-3, "smooth tangent ~horizontal");
    // make_smooth on an already-smooth anchor is a no-op.
    assert!(!make_smooth(&pts, &mut handles, false, 1));
    // Smooth → corner: handle zeroed.
    assert!(make_corner(&mut handles, pts.len(), 1));
    assert!(is_corner(&handles, 1));
    // make_corner on an already-corner anchor is a no-op.
    assert!(!make_corner(&mut handles, pts.len(), 1));
}

#[test]
fn shape_contour_count_and_access() {
    let path = open_path();
    assert_eq!(path.contour_count(), 1);
    assert!(path.contour(0).is_some());
    assert!(path.contour(1).is_none());

    let compound = donut(FillRule::NonZero);
    assert_eq!(compound.contour_count(), 2);
    let (pts, _, closed) = compound.contour(1).expect("inner ring");
    assert!(closed);
    assert_eq!(pts.len(), 4);

    // Non-editable shapes expose no contours.
    let rect = Shape::Rect {
        rect: [0.0, 0.0, 10.0, 10.0],
        fill: [0.0; 4],
        fill_gradient: None,
        stroke: [0.0; 4],
        stroke_w: 1.0,
        stroke_style: StrokeStyle::default(),
        appearance: None,
        visible: true,
        group: None,
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
    };
    assert_eq!(rect.contour_count(), 0);
}

#[test]
fn set_anchor_and_handle_move_the_right_point() {
    let mut path = open_path();
    assert!(path.set_anchor(0, 1, 42.0, 7.0));
    let (pts, _, _) = path.contour(0).unwrap();
    assert_eq!(pts[1], (42.0, 7.0));

    // set_handle places the out-knob at the cursor (offset stored relative to
    // the anchor).
    assert!(path.set_handle(0, 1, 52.0, 7.0));
    let (pts, handles, _) = path.contour(0).unwrap();
    assert_eq!(handles[1], (52.0 - pts[1].0, 7.0 - pts[1].1));
}

#[test]
fn insert_and_delete_anchor_on_compound_subcontour() {
    let mut compound = donut(FillRule::NonZero);
    // Insert on the inner ring (contour 1), first segment, midpoint.
    let before = compound.contour(1).unwrap().0.len();
    let idx = compound.insert_anchor_in(1, 0, 0.5).expect("inserted");
    assert_eq!(idx, 1);
    assert_eq!(compound.contour(1).unwrap().0.len(), before + 1);
    // Delete it again.
    assert!(compound.delete_anchor_in(1, idx));
    assert_eq!(compound.contour(1).unwrap().0.len(), before);
    // The outer ring (contour 0) is untouched.
    assert_eq!(compound.contour(0).unwrap().0.len(), 4);
}

#[test]
fn convert_anchor_on_compound_toggles_smooth_corner() {
    let mut compound = donut(FillRule::NonZero);
    // Inner ring corner → smooth.
    let smooth = compound.toggle_anchor_smooth_in(1, 0);
    assert!(smooth);
    let (_, handles, _) = compound.contour(1).unwrap();
    assert!(!is_corner(handles, 0));
    // Back to corner.
    let smooth = compound.toggle_anchor_smooth_in(1, 0);
    assert!(!smooth);
    let (_, handles, _) = compound.contour(1).unwrap();
    assert!(is_corner(handles, 0));
}

#[test]
fn delete_anchor_in_refuses_below_two_points() {
    // A two-point open path: deleting any anchor would leave a single point.
    let mut path = Shape::Path {
        points: vec![(0.0, 0.0), (10.0, 0.0)],
        closed: false,
        fill: [0.0; 4],
        fill_gradient: None,
        stroke: [0.0; 4],
        stroke_w: 1.0,
        stroke_style: StrokeStyle::default(),
        appearance: None,
        handles: vec![(0.0, 0.0); 2],
        live: None,
        visible: true,
        group: None,
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
    };
    assert!(!path.delete_anchor_in(0, 0));
    assert_eq!(path.contour(0).unwrap().0.len(), 2);
}

/// A simple three-anchor open corner path for the Direct-Select shape tests.
fn open_path() -> Shape {
    Shape::Path {
        points: vec![(0.0, 0.0), (50.0, 0.0), (100.0, 0.0)],
        closed: false,
        fill: [0.0; 4],
        fill_gradient: None,
        stroke: [0.0, 0.0, 0.0, 1.0],
        stroke_w: 1.0,
        stroke_style: StrokeStyle::default(),
        appearance: None,
        handles: vec![(0.0, 0.0); 3],
        live: None,
        visible: true,
        group: None,
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
    }
}

// --- Stroke style ------------------------------------------------------

#[test]
fn stroke_style_default_is_solid_butt_miter() {
    let s = StrokeStyle::default();
    assert_eq!(s.cap, LineCap::Butt);
    assert_eq!(s.join, LineJoin::Miter);
    assert_eq!(s.miter_limit, 4.0);
    assert!(!s.is_dashed());
    assert!(s.normalized_dash().is_none());
}

#[test]
fn is_dashed_ignores_all_zero_pattern() {
    let solid = StrokeStyle {
        dash: vec![0.0, 0.0],
        ..Default::default()
    };
    assert!(!solid.is_dashed());
    let dashed = StrokeStyle {
        dash: vec![6.0, 3.0],
        ..Default::default()
    };
    assert!(dashed.is_dashed());
}

#[test]
fn normalized_dash_doubles_odd_pattern() {
    // Odd-length pattern must be repeated so on/off runs alternate evenly
    // (the SVG stroke-dasharray rule).
    let s = StrokeStyle {
        dash: vec![5.0],
        ..Default::default()
    };
    let n = s.normalized_dash().expect("dashed");
    assert_eq!(n, vec![5.0, 5.0]);

    let s2 = StrokeStyle {
        dash: vec![6.0, 2.0, 1.0],
        ..Default::default()
    };
    let n2 = s2.normalized_dash().expect("dashed");
    assert_eq!(n2, vec![6.0, 2.0, 1.0, 6.0, 2.0, 1.0]);
}

#[test]
fn normalized_dash_clamps_negatives() {
    let s = StrokeStyle {
        dash: vec![6.0, -2.0],
        ..Default::default()
    };
    let n = s.normalized_dash().expect("has a positive run");
    assert_eq!(n, vec![6.0, 0.0]);
}

// --- Affine transforms -------------------------------------------------

#[test]
fn axis_aligned_scale_keeps_rect_a_rect() {
    let mut s = Shape::Rect {
        rect: [10.0, 20.0, 40.0, 30.0],
        fill: [0.0; 4],
        fill_gradient: None,
        stroke: [0.0; 4],
        stroke_w: 1.0,
        stroke_style: StrokeStyle::default(),
        appearance: None,
        visible: true,
        group: None,
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
    };
    // Scale ×2 about the origin.
    s.apply_affine(&Affine::scale(2.0, 2.0));
    match s {
        Shape::Rect { rect, .. } => {
            assert_eq!(rect, [20.0, 40.0, 80.0, 60.0]);
        }
        _ => panic!("axis-aligned scale should keep a Rect a Rect"),
    }
}

#[test]
fn flip_keeps_rect_normalized() {
    let mut s = Shape::Rect {
        rect: [10.0, 0.0, 40.0, 20.0],
        fill: [0.0; 4],
        fill_gradient: None,
        stroke: [0.0; 4],
        stroke_w: 1.0,
        stroke_style: StrokeStyle::default(),
        appearance: None,
        visible: true,
        group: None,
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
    };
    // Horizontal flip about x = 30 (the rect's centre): bounds unchanged,
    // width/height stay positive.
    s.apply_affine(&Affine::scale_about(-1.0, 1.0, 30.0, 0.0));
    match s {
        Shape::Rect { rect, .. } => {
            assert!((rect[0] - 10.0).abs() < 1e-3);
            assert!((rect[2] - 40.0).abs() < 1e-3);
            assert!(rect[2] > 0.0 && rect[3] > 0.0);
        }
        _ => panic!("flip should keep a Rect a Rect"),
    }
}

#[test]
fn rotation_converts_rect_to_path() {
    let mut s = Shape::Rect {
        rect: [0.0, 0.0, 10.0, 10.0],
        fill: [0.0; 4],
        fill_gradient: None,
        stroke: [0.0; 4],
        stroke_w: 1.0,
        stroke_style: StrokeStyle::default(),
        appearance: None,
        visible: true,
        group: None,
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
    };
    s.apply_affine(&Affine::rotate_about(0.5, 5.0, 5.0));
    assert!(
        matches!(s, Shape::Path { .. }),
        "rotation must rasterise to a Path"
    );
    if let Shape::Path { points, closed, .. } = &s {
        assert_eq!(points.len(), 4);
        assert!(*closed);
    }
}

#[test]
fn rotation_preserves_rect_center() {
    let mut s = Shape::Rect {
        rect: [0.0, 0.0, 100.0, 40.0],
        fill: [0.0; 4],
        fill_gradient: None,
        stroke: [0.0; 4],
        stroke_w: 1.0,
        stroke_style: StrokeStyle::default(),
        appearance: None,
        visible: true,
        group: None,
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
    };
    let before = s.bounds().unwrap();
    let (cx0, cy0) = (before.x + before.w * 0.5, before.y + before.h * 0.5);
    // Rotate 90° about the rect centre; the centre must be fixed.
    s.apply_affine(&Affine::rotate_about(std::f32::consts::FRAC_PI_2, cx0, cy0));
    let after = s.bounds().unwrap();
    let (cx1, cy1) = (after.x + after.w * 0.5, after.y + after.h * 0.5);
    assert!((cx0 - cx1).abs() < 0.5 && (cy0 - cy1).abs() < 0.5);
    // A 90° turn swaps the bbox extents.
    assert!((after.w - before.h).abs() < 0.5);
    assert!((after.h - before.w).abs() < 0.5);
}

#[test]
fn ellipse_to_path_round_trips_bounds() {
    let s = Shape::Ellipse {
        rect: [0.0, 0.0, 80.0, 40.0],
        fill: [0.0; 4],
        fill_gradient: None,
        stroke: [0.0; 4],
        stroke_w: 1.0,
        stroke_style: StrokeStyle::default(),
        appearance: None,
        visible: true,
        group: None,
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
    };
    let p = s.to_path();
    let pb = p.bounds().unwrap();
    // The cubic ellipse path should hug the original ellipse box closely.
    assert!((pb.x - 0.0).abs() < 0.5);
    assert!((pb.y - 0.0).abs() < 0.5);
    assert!((pb.w - 80.0).abs() < 0.5);
    assert!((pb.h - 40.0).abs() < 0.5);
}

#[test]
fn path_handles_transform_by_linear_part() {
    // A path with a curve handle; under a translate the handle (an offset)
    // must NOT move, but the anchors must.
    let mut s = Shape::Path {
        points: vec![(0.0, 0.0), (10.0, 0.0)],
        closed: false,
        fill: [0.0; 4],
        fill_gradient: None,
        stroke: [0.0; 4],
        stroke_w: 1.0,
        stroke_style: StrokeStyle::default(),
        appearance: None,
        handles: vec![(3.0, 4.0), (0.0, 0.0)],
        live: None,
        visible: true,
        group: None,
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
    };
    s.apply_affine(&Affine::translate(100.0, 50.0));
    if let Shape::Path {
        points, handles, ..
    } = &s
    {
        assert_eq!(points[0], (100.0, 50.0));
        assert_eq!(handles[0], (3.0, 4.0)); // offset unchanged by translation
    } else {
        panic!("still a path");
    }
}

/// `.contour` files written before stroke styles existed must load with a
/// default (solid, butt, miter) stroke style on every shape.
#[test]
fn loads_pre_stroke_style_document() {
    let json = r#"{"shapes":[
        {"Line":{"p0":[0,0],"p1":[10,0],"stroke":[0,0,0,1],"stroke_w":2}}
    ]}"#;
    let doc: Document = serde_json::from_str(json).unwrap();
    assert_eq!(doc.shapes.len(), 1);
    let st = doc.shapes[0].stroke_style();
    assert_eq!(st, &StrokeStyle::default());
}

// --- Gradient fills ----------------------------------------------------

/// `.contour` files written before gradient fills existed must load with no
/// gradient (a solid fill), and a gradient must round-trip through serde.
#[test]
fn fill_gradient_is_additive_and_round_trips() {
    use crate::gradient::{Gradient, GradientKind};
    // A pre-gradient Rect (no `fill_gradient` key) loads with `None`.
    let json = r#"{"shapes":[
        {"Rect":{"rect":[0,0,10,10],"fill":[1,0,0,1],"stroke":[0,0,0,1],"stroke_w":1}}
    ]}"#;
    let doc: Document = serde_json::from_str(json).unwrap();
    assert!(doc.shapes[0].fill_gradient().is_none());
    assert_eq!(doc.shapes[0].fill_color(), Some([1.0, 0.0, 0.0, 1.0]));

    // Setting a gradient and serializing round-trips it back.
    let mut doc = doc;
    let g = Gradient::two_stop(
        GradientKind::Radial,
        [1.0, 1.0, 1.0, 1.0],
        [0.0, 0.0, 0.0, 1.0],
    );
    doc.shapes[0].set_fill_gradient(Some(g.clone()));
    let s = serde_json::to_string(&doc).unwrap();
    let back: Document = serde_json::from_str(&s).unwrap();
    assert_eq!(back.shapes[0].fill_gradient(), Some(&g));
}

/// A `Line` has no fill region, so setting a gradient on it is a no-op.
#[test]
fn line_ignores_gradient_fill() {
    use crate::gradient::{Gradient, GradientKind};
    let mut line = Shape::Line {
        p0: (0.0, 0.0),
        p1: (10.0, 0.0),
        stroke: [0.0; 4],
        stroke_w: 1.0,
        stroke_style: StrokeStyle::default(),
        appearance: None,
        visible: true,
        group: None,
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
    };
    line.set_fill_gradient(Some(Gradient::two_stop(
        GradientKind::Linear,
        [0.0; 4],
        [1.0; 4],
    )));
    assert!(line.fill_gradient().is_none());
    assert!(line.fill_color().is_none());
}

// --- Group membership --------------------------------------------------

/// The additive `group` tag round-trips through serde and is `None` on a
/// document written before grouping existed.
#[test]
fn group_tag_is_additive_and_round_trips() {
    // A pre-group Rect (no `group` key) loads ungrouped.
    let json = r#"{"shapes":[
        {"Rect":{"rect":[0,0,10,10],"fill":[1,0,0,1],"stroke":[0,0,0,1],"stroke_w":1}}
    ]}"#;
    let mut doc: Document = serde_json::from_str(json).unwrap();
    assert_eq!(doc.shapes[0].group(), None);

    // Tagging it with a group and serializing round-trips the id back.
    doc.shapes[0].set_group(Some(7));
    let s = serde_json::to_string(&doc).unwrap();
    let back: Document = serde_json::from_str(&s).unwrap();
    assert_eq!(back.shapes[0].group(), Some(7));
}

/// `set_group` / `group` work uniformly across every variant, including `Line`
/// (which has no fill but can still belong to a group).
#[test]
fn group_accessor_covers_every_variant() {
    let mut line = Shape::Line {
        p0: (0.0, 0.0),
        p1: (10.0, 0.0),
        stroke: [0.0; 4],
        stroke_w: 1.0,
        stroke_style: StrokeStyle::default(),
        appearance: None,
        visible: true,
        group: None,
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
    };
    line.set_group(Some(3));
    assert_eq!(line.group(), Some(3));
    line.set_group(None);
    assert_eq!(line.group(), None);
}

/// Converting a grouped shape to a path preserves its group membership, so a
/// rotation (which rasterises a `Rect`/`Ellipse` into a `Path`) keeps it in its
/// group.
#[test]
fn to_path_preserves_group() {
    let r = Shape::Rect {
        rect: [0.0, 0.0, 10.0, 10.0],
        fill: [0.0; 4],
        fill_gradient: None,
        stroke: [0.0; 4],
        stroke_w: 1.0,
        stroke_style: StrokeStyle::default(),
        appearance: None,
        visible: true,
        group: Some(42),
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
    };
    assert_eq!(r.to_path().group(), Some(42));

    // And a rotation (Rect -> Path under the hood) keeps the group too.
    let mut r2 = r;
    r2.apply_affine(&Affine::rotate_about(0.5, 5.0, 5.0));
    assert!(matches!(r2, Shape::Path { .. }));
    assert_eq!(r2.group(), Some(42));
}

/// A `Rect` with a gradient fill carries that gradient through `with_outline`,
/// and the produced shape is a clipped, plain (un-clip-tagged) closed path.
#[test]
fn with_outline_inherits_paint_and_clears_clip() {
    let mut s = Shape::Rect {
        rect: [0.0, 0.0, 10.0, 10.0],
        fill: [0.2, 0.4, 0.6, 1.0],
        fill_gradient: None,
        stroke: [0.1, 0.1, 0.1, 1.0],
        stroke_w: 3.0,
        stroke_style: StrokeStyle::default(),
        appearance: None,
        visible: true,
        group: Some(5),
        clip: Some(9),
        mask: false,
        omask: None,
        omask_path: false,
        omask_invert: false,
        blend: None,
        blend_step: false,
        name: None,
        locked: false,
        layer_color: None,
    };
    s.set_mask(true);
    let ring = vec![(0.0, 0.0), (4.0, 0.0), (4.0, 4.0), (0.0, 4.0)];
    let out = s.with_outline(ring.clone());
    match out {
        Shape::Path {
            points,
            closed,
            fill,
            stroke_w,
            group,
            clip,
            mask,
            ..
        } => {
            assert_eq!(points, ring);
            assert!(closed);
            assert_eq!(fill, [0.2, 0.4, 0.6, 1.0]);
            assert_eq!(stroke_w, 3.0);
            assert_eq!(group, Some(5)); // group survives
            assert_eq!(clip, None); // clip tag cleared (already clipped)
            assert!(!mask);
        }
        _ => panic!("with_outline must produce a Path"),
    }
}

/// Helper: a styled rect with explicit clip tagging, for clip-set tests.
fn clip_rect(rect: [f32; 4], clip: Option<u64>, mask: bool) -> Shape {
    Shape::Rect {
        rect,
        fill: [0.5, 0.5, 0.5, 1.0],
        fill_gradient: None,
        stroke: [0.0, 0.0, 0.0, 1.0],
        stroke_w: 1.0,
        stroke_style: StrokeStyle::default(),
        appearance: None,
        visible: true,
        group: None,
        clip,
        mask,
        omask: None,
        omask_path: false,
        omask_invert: false,
        blend: None,
        blend_step: false,
        name: None,
        locked: false,
        layer_color: None,
    }
}

/// `render_shapes` resolves a clip set: the mask paints nothing, and the clipped
/// content is cropped to the mask outline.
#[test]
fn render_shapes_resolves_a_clip_set() {
    let mut doc = Document::new();
    // A 20×20 content rect clipped by a 10×10 mask (set id 0; the mask is on top).
    doc.shapes
        .push(clip_rect([0.0, 0.0, 20.0, 20.0], Some(0), false));
    doc.shapes
        .push(clip_rect([0.0, 0.0, 10.0, 10.0], Some(0), true));
    // Plus a loose rect that should pass through untouched.
    doc.shapes
        .push(clip_rect([50.0, 50.0, 5.0, 5.0], None, false));

    let rendered = doc.render_shapes();
    // The mask is omitted; the content (clipped) + the loose rect remain.
    assert_eq!(rendered.len(), 2);
    let indices: Vec<usize> = rendered.iter().map(|(i, _)| *i).collect();
    assert!(indices.contains(&0)); // clipped content kept (original index 0)
    assert!(indices.contains(&2)); // loose rect kept
    assert!(!indices.contains(&1)); // mask dropped

    // The clipped content's bounds shrink to the 10×10 mask region.
    let content = &rendered.iter().find(|(i, _)| *i == 0).unwrap().1;
    let b = content.bounds().unwrap();
    assert!((b.x - 0.0).abs() < 1e-2 && (b.y - 0.0).abs() < 1e-2);
    assert!((b.w - 10.0).abs() < 1e-2 && (b.h - 10.0).abs() < 1e-2);
}

/// Content lying entirely outside the mask is dropped from the render.
#[test]
fn render_shapes_drops_content_outside_the_mask() {
    let mut doc = Document::new();
    doc.shapes
        .push(clip_rect([100.0, 100.0, 10.0, 10.0], Some(0), false));
    doc.shapes
        .push(clip_rect([0.0, 0.0, 10.0, 10.0], Some(0), true));
    let rendered = doc.render_shapes();
    // Disjoint content clips to nothing; the mask paints nothing → empty render.
    assert!(rendered.is_empty());
}

/// A clip set round-trips through serde, and clearing the tags (Release) restores
/// the originals so `render_shapes` returns every shape unclipped.
#[test]
fn clip_tags_serde_round_trip_and_release() {
    let mut doc = Document::new();
    doc.shapes
        .push(clip_rect([0.0, 0.0, 20.0, 20.0], Some(3), false));
    doc.shapes
        .push(clip_rect([0.0, 0.0, 10.0, 10.0], Some(3), true));

    let json = serde_json::to_string(&doc).unwrap();
    let back: Document = serde_json::from_str(&json).unwrap();
    assert_eq!(back.shapes[0].clip(), Some(3));
    assert!(back.shapes[1].is_mask());

    // Release: clear the clip tags; both shapes now render plainly.
    let mut released = back;
    for s in released.shapes.iter_mut() {
        s.clear_clip();
    }
    assert!(released.shapes.iter().all(|s| s.clip().is_none()));
    assert_eq!(released.render_shapes().len(), 2);
}

/// A pre-clip `.contour` (no `clip`/`mask` keys) loads unclipped and renders every
/// shape as-is.
#[test]
fn loads_pre_clip_document() {
    let json = r#"{"shapes":[
        {"Rect":{"rect":[0,0,10,10],"fill":[1,0,0,1],"stroke":[0,0,0,1],"stroke_w":2}}
    ]}"#;
    let doc: Document = serde_json::from_str(json).unwrap();
    assert_eq!(doc.shapes[0].clip(), None);
    assert!(!doc.shapes[0].is_mask());
    assert_eq!(doc.render_shapes().len(), 1);
}

// --- Opacity masks -----------------------------------------------------

/// A styled rect with explicit opacity-mask tagging, for opacity-mask tests.
fn omask_rect(rect: [f32; 4], fill: [f32; 4], omask: Option<u64>, mask: bool) -> Shape {
    Shape::Rect {
        rect,
        fill,
        fill_gradient: None,
        stroke: [0.0, 0.0, 0.0, 1.0],
        stroke_w: 1.0,
        stroke_style: StrokeStyle::default(),
        appearance: None,
        visible: true,
        group: None,
        clip: None,
        mask: false,
        omask,
        omask_path: mask,
        omask_invert: false,
        blend: None,
        blend_step: false,
        name: None,
        locked: false,
        layer_color: None,
    }
}

/// A pre-opacity-mask `.contour` (no `omask`/`omask_path`/`omask_invert` keys)
/// loads unmasked and renders every shape as-is.
#[test]
fn loads_pre_opacity_mask_document() {
    let json = r#"{"shapes":[
        {"Rect":{"rect":[0,0,10,10],"fill":[1,0,0,1],"stroke":[0,0,0,1],"stroke_w":2}}
    ]}"#;
    let doc: Document = serde_json::from_str(json).unwrap();
    assert_eq!(doc.shapes[0].omask(), None);
    assert!(!doc.shapes[0].is_omask());
    assert!(!doc.shapes[0].omask_invert());
    assert!(doc.opacity_mask_of(0).is_none());
}

/// An opacity-masked shape round-trips through serde (id + mask flag + invert),
/// and `opacity_mask_of` resolves the content's mask shape; `render_shapes` drops
/// the mask path (it paints nothing) but keeps the masked content.
#[test]
fn opacity_mask_round_trip_and_resolution() {
    let mut doc = Document::new();
    doc.shapes.clear();
    // Content (index 0) masked by the white mask path (index 1), invert on content.
    let mut content = omask_rect([0.0, 0.0, 20.0, 20.0], [1.0, 0.0, 0.0, 1.0], Some(7), false);
    content.set_omask_invert(true);
    doc.shapes.push(content);
    doc.shapes
        .push(omask_rect([0.0, 0.0, 20.0, 20.0], [1.0, 1.0, 1.0, 1.0], Some(7), true));

    let json = serde_json::to_string(&doc).unwrap();
    let back: Document = serde_json::from_str(&json).unwrap();
    assert_eq!(back.shapes[0].omask(), Some(7));
    assert!(back.shapes[0].omask_invert());
    assert!(back.shapes[1].is_omask());

    // Resolution: the content's mask is the white rect; invert carried through.
    let (mask_shape, invert) = back.opacity_mask_of(0).expect("content has a mask");
    assert!(invert, "invert flag carried");
    assert_eq!(mask_shape.fill_color(), Some([1.0, 1.0, 1.0, 1.0]));
    // The mask path itself is not "masked".
    assert!(back.opacity_mask_of(1).is_none());

    // render_shapes drops the mask path but keeps the (still full-geometry) content.
    let rendered = back.render_shapes();
    let indices: Vec<usize> = rendered.iter().map(|(i, _)| *i).collect();
    assert!(indices.contains(&0), "masked content kept");
    assert!(!indices.contains(&1), "mask path dropped");
}

/// A test rect with the given fill / stroke colours.
fn swatch_rect(fill: [f32; 4], stroke: [f32; 4]) -> Shape {
    Shape::Rect {
        rect: [0.0, 0.0, 10.0, 10.0],
        fill,
        fill_gradient: None,
        stroke,
        stroke_w: 1.0,
        stroke_style: StrokeStyle::default(),
        appearance: None,
        visible: true,
        group: None,
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
    }
}

/// A fresh document opens with the default starter palette.
#[test]
fn fresh_document_has_starter_palette() {
    let doc = Document::new();
    assert_eq!(doc.swatches.len(), 8);
    // The starter palette includes Black and Blue.
    assert!(doc.swatches.id_for_color([0.0, 0.0, 0.0, 1.0]).is_some());
}

/// A pre-swatches `.contour` (no `swatches` key) loads with the default starter
/// palette via `#[serde(default)]`.
#[test]
fn loads_pre_swatches_document() {
    let json = r#"{"shapes":[
        {"Rect":{"rect":[0,0,10,10],"fill":[1,0,0,1],"stroke":[0,0,0,1],"stroke_w":2}}
    ]}"#;
    let doc: Document = serde_json::from_str(json).unwrap();
    assert_eq!(doc.swatches.len(), 8);
}

/// A document's swatch palette round-trips through serde unchanged.
#[test]
fn swatches_round_trip() {
    let mut doc = Document::new();
    let id = doc.swatches.add("Brand", [0.3, 0.6, 0.9, 1.0]);
    doc.swatches.set_global(id, true);
    let json = serde_json::to_string(&doc).unwrap();
    let back: Document = serde_json::from_str(&json).unwrap();
    let sw = back.swatches.get(id).unwrap();
    assert_eq!(sw.name, "Brand");
    assert_eq!(sw.color, [0.3, 0.6, 0.9, 1.0]);
    assert!(sw.global);
}

/// `Document::remap_color` rewrites a colour across fills, strokes, and gradient
/// stops — the artwork half of a global-swatch recolour — and reports the number
/// of shapes touched.
#[test]
fn remap_color_recolors_matching_fills_strokes_and_gradients() {
    let old = [0.2, 0.2, 0.2, 1.0];
    let new = [0.8, 0.1, 0.1, 1.0];
    let mut doc = Document::new();
    doc.shapes.clear();
    // (0) fill matches, (1) stroke matches, (2) nothing matches, (3) gradient stop.
    doc.shapes.push(swatch_rect(old, [1.0, 1.0, 1.0, 1.0]));
    doc.shapes.push(swatch_rect([1.0, 1.0, 1.0, 1.0], old));
    doc.shapes
        .push(swatch_rect([0.5, 0.5, 0.5, 1.0], [0.0, 0.0, 0.0, 1.0]));
    let mut grad_shape = swatch_rect([1.0, 1.0, 1.0, 1.0], [0.0, 0.0, 0.0, 1.0]);
    grad_shape.set_fill_gradient(Some(crate::gradient::Gradient::two_stop(
        crate::gradient::GradientKind::Linear,
        old,
        [0.0, 0.0, 1.0, 1.0],
    )));
    doc.shapes.push(grad_shape);

    let n = doc.remap_color(old, new);
    assert_eq!(n, 3, "fill, stroke, and gradient-stop shapes changed");
    assert_eq!(doc.shapes[0].fill_color(), Some(new));
    assert_eq!(doc.shapes[1].stroke_color(), Some(new));
    // The untouched shape keeps its colours.
    assert_eq!(doc.shapes[2].fill_color(), Some([0.5, 0.5, 0.5, 1.0]));
    // The gradient stop was remapped.
    assert_eq!(doc.shapes[3].fill_gradient().unwrap().stops[0].color, new);
}

/// Remapping a colour to itself is a no-op that touches nothing.
#[test]
fn remap_color_to_same_color_is_noop() {
    let c = [0.4, 0.4, 0.4, 1.0];
    let mut doc = Document::new();
    doc.shapes.clear();
    doc.shapes.push(swatch_rect(c, [0.0, 0.0, 0.0, 1.0]));
    assert_eq!(doc.remap_color(c, c), 0);
}

/// End-to-end global-swatch flow (the composition the app's `recolor_swatch`
/// performs): painting shapes with a **global** swatch's colour, then editing
/// the swatch, re-colours every bound shape; a **non-global** swatch edit is a
/// one-time copy that leaves all artwork untouched.
#[test]
fn global_swatch_edit_recolors_bound_shapes_non_global_does_not() {
    let brand = [0.20, 0.40, 0.80, 1.0];
    let other = [0.90, 0.90, 0.90, 1.0];
    let mut doc = Document::new();
    doc.shapes.clear();
    // Two shapes painted with the swatch colour, one painted with something else.
    doc.shapes.push(swatch_rect(brand, [0.0, 0.0, 0.0, 1.0]));
    doc.shapes.push(swatch_rect(other, brand)); // stroke uses the colour
    doc.shapes.push(swatch_rect(other, [0.0, 0.0, 0.0, 1.0]));

    // --- Global swatch: editing it follows every shape painted with it. ---
    let id = doc.swatches.add("Brand", brand);
    doc.swatches.set_global(id, true);
    let edited = [0.85, 0.10, 0.20, 1.0];
    // The app composes recolor (gives the (old,new) pair for a global) with
    // remap_color (walks the artwork).
    let pair = doc.swatches.recolor(id, edited);
    assert_eq!(pair, Some((brand, edited)));
    let (old, new) = pair.unwrap();
    let n = doc.remap_color(old, new);
    assert_eq!(n, 2, "both shapes bound to the swatch colour re-coloured");
    assert_eq!(doc.shapes[0].fill_color(), Some(edited));
    assert_eq!(doc.shapes[1].stroke_color(), Some(edited));
    // The unrelated shape is untouched, and the swatch itself now holds the edit.
    assert_eq!(doc.shapes[2].fill_color(), Some(other));
    assert_eq!(doc.swatches.get(id).unwrap().color, edited);

    // --- Non-global swatch: editing it is a one-time copy, no artwork moves. ---
    // A distinct (non-global) swatch whose colour happens to match shape 2's fill.
    let plain = doc.swatches.add("Accent", other);
    assert!(!doc.swatches.get(plain).unwrap().global);
    let snapshot: Vec<_> = doc.shapes.iter().map(|s| s.fill_color()).collect();
    assert_eq!(doc.swatches.recolor(plain, [0.0, 1.0, 0.0, 1.0]), None);
    // No remap is performed (recolor returned None) → artwork is unchanged.
    let after: Vec<_> = doc.shapes.iter().map(|s| s.fill_color()).collect();
    assert_eq!(snapshot, after, "a non-global edit never touches the artwork");
    // …but the swatch colour itself did change.
    assert_eq!(doc.swatches.get(plain).unwrap().color, [0.0, 1.0, 0.0, 1.0]);
}

// --- Compound paths ----------------------------------------------------

/// A compound path: a 30×30 outer ring with a 10×10 inner hole sub-contour, with
/// the given fill rule.
fn donut(fill_rule: FillRule) -> Shape {
    let outer = SubPath::ring(vec![
        (0.0, 0.0),
        (30.0, 0.0),
        (30.0, 30.0),
        (0.0, 30.0),
    ]);
    let inner = SubPath::ring(vec![
        (10.0, 10.0),
        (20.0, 10.0),
        (20.0, 20.0),
        (10.0, 20.0),
    ]);
    Shape::Compound {
        subpaths: vec![outer, inner],
        fill_rule,
        fill: [1.0, 0.0, 0.0, 1.0],
        fill_gradient: None,
        stroke: [0.0, 0.0, 0.0, 1.0],
        stroke_w: 1.0,
        stroke_style: StrokeStyle::default(),
        appearance: None,
        visible: true,
        group: None,
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
    }
}

/// Even-odd carves the inner ring as a hole (a click in the middle misses the
/// fill); non-zero with same-wound rings keeps the middle filled.
#[test]
fn compound_fill_rule_even_odd_carves_hole() {
    let eo = donut(FillRule::EvenOdd);
    // Inside the outer ring but inside the hole (centre 15,15): even-odd → not
    // filled; on the solid frame (5,5): filled.
    assert!(!eo.hit(15.0, 15.0, 0.1), "even-odd hole is empty");
    assert!(eo.hit(5.0, 5.0, 0.1), "even-odd frame is solid");
    // Outside the outer ring entirely: not filled.
    assert!(!eo.hit(40.0, 40.0, 0.1));

    // Non-zero with both rings wound the *same* way: the inner does not subtract,
    // so the centre is filled (the classic non-zero behaviour).
    let nz = donut(FillRule::NonZero);
    assert!(nz.hit(15.0, 15.0, 0.1), "non-zero same-wound fills the centre");
}

/// `point_in_rings` matches the documented winding behaviour directly: even-odd
/// parity vs non-zero winding, with an opposite-wound inner ring carving under
/// both rules.
#[test]
fn point_in_rings_winding_rules() {
    let outer = vec![(0.0, 0.0), (30.0, 0.0), (30.0, 30.0), (0.0, 30.0)]; // CCW-ish
    let inner_same = vec![(10.0, 10.0), (20.0, 10.0), (20.0, 20.0), (10.0, 20.0)]; // same dir
    let inner_rev = vec![(10.0, 10.0), (10.0, 20.0), (20.0, 20.0), (20.0, 10.0)]; // reversed

    // Even-odd: a ring inside another always carves, regardless of direction.
    let eo = vec![outer.clone(), inner_same.clone()];
    assert!(!point_in_rings(15.0, 15.0, &eo, FillRule::EvenOdd));
    assert!(point_in_rings(5.0, 5.0, &eo, FillRule::EvenOdd));

    // Non-zero, same winding: inner does NOT subtract (filled centre).
    assert!(point_in_rings(15.0, 15.0, &eo, FillRule::NonZero));
    // Non-zero, reversed inner winding: inner DOES subtract (empty centre).
    let nz_rev = vec![outer.clone(), inner_rev];
    assert!(!point_in_rings(15.0, 15.0, &nz_rev, FillRule::NonZero));
    assert!(point_in_rings(5.0, 5.0, &nz_rev, FillRule::NonZero));
}

/// A compound path's bounds union all sub-contours; its net is the frame.
#[test]
fn compound_bounds_union_subcontours() {
    let s = donut(FillRule::EvenOdd);
    let b = s.bounds().unwrap();
    assert!((b.x - 0.0).abs() < 1e-3 && (b.y - 0.0).abs() < 1e-3);
    assert!((b.w - 30.0).abs() < 1e-3 && (b.h - 30.0).abs() < 1e-3);
}

/// Translating a compound moves every sub-contour together.
#[test]
fn compound_translate_moves_all_subcontours() {
    let mut s = donut(FillRule::EvenOdd);
    s.translate(100.0, 50.0);
    let b = s.bounds().unwrap();
    assert!((b.x - 100.0).abs() < 1e-3 && (b.y - 50.0).abs() < 1e-3);
    // The hole is still carved after the move (point in the moved centre).
    assert!(!s.hit(115.0, 65.0, 0.1), "moved hole stays empty");
    assert!(s.hit(105.0, 55.0, 0.1), "moved frame stays solid");
}

/// A compound path round-trips through serde (sub-contours + fill rule + paint),
/// and the outline polygon is its outer ring.
#[test]
fn compound_round_trips_and_outlines_outer_ring() {
    let s = donut(FillRule::EvenOdd);
    let doc = Document {
        shapes: vec![s],
        ..Default::default()
    };
    let json = serde_json::to_string(&doc).unwrap();
    let back: Document = serde_json::from_str(&json).unwrap();
    match &back.shapes[0] {
        Shape::Compound {
            subpaths,
            fill_rule,
            fill,
            ..
        } => {
            assert_eq!(subpaths.len(), 2, "outer + hole sub-contours survive");
            assert_eq!(*fill_rule, FillRule::EvenOdd);
            assert_eq!(*fill, [1.0, 0.0, 0.0, 1.0]);
        }
        other => panic!("expected a Compound, got {other:?}"),
    }
    // The outline polygon is the outer ring (its bbox is the full 30×30).
    let outline = back.shapes[0].outline_polygon().unwrap();
    let xs: Vec<f32> = outline.iter().map(|p| p.0).collect();
    let max_x = xs.iter().cloned().fold(f32::MIN, f32::max);
    assert!((max_x - 30.0).abs() < 1e-3, "outline is the outer 30×30 ring");
}

/// A pre-compound `.contour` (no `Compound` variant) loads unchanged — the new
/// variant is additive (a back-compat check that adding the variant didn't break
/// older single-ring documents).
#[test]
fn loads_pre_compound_document() {
    let json = r#"{"shapes":[
        {"Path":{"points":[[0,0],[10,0],[10,10]],"closed":true,"fill":[0,1,0,1],"stroke":[0,0,0,1],"stroke_w":1}}
    ]}"#;
    let doc: Document = serde_json::from_str(json).unwrap();
    assert_eq!(doc.shapes.len(), 1);
    assert!(matches!(doc.shapes[0], Shape::Path { .. }));
}

/// A compound `SubPath` deserializes with its `closed` defaulting to true and an
/// empty `handles` (back-compat for a minimal / hand-written compound).
#[test]
fn compound_subpath_serde_defaults() {
    let json = r#"{"Compound":{
        "subpaths":[{"points":[[0,0],[10,0],[10,10],[0,10]]}],
        "fill":[1,0,0,1],"stroke":[0,0,0,1],"stroke_w":1
    }}"#;
    let s: Shape = serde_json::from_str(json).unwrap();
    match s {
        Shape::Compound {
            subpaths,
            fill_rule,
            ..
        } => {
            assert_eq!(subpaths.len(), 1);
            assert!(subpaths[0].closed, "closed defaults to true");
            assert!(subpaths[0].handles.is_empty(), "handles default to empty");
            assert_eq!(fill_rule, FillRule::NonZero, "fill_rule defaults to non-zero");
        }
        _ => panic!("expected Compound"),
    }
}

/// Build a plain 10×10 red rectangle at the origin for the Layers-panel tests.
fn layer_rect() -> Shape {
    Shape::Rect {
        rect: [0.0, 0.0, 10.0, 10.0],
        fill: [1.0, 0.0, 0.0, 1.0],
        fill_gradient: None,
        stroke: [0.0, 0.0, 0.0, 1.0],
        stroke_w: 1.0,
        stroke_style: StrokeStyle::default(),
        appearance: None,
        visible: true,
        group: None,
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
    }
}

/// A **locked** shape is excluded from selection / hit-testing via the shared
/// [`Shape::selectable`] gate (it still reports `visible`).
#[test]
fn locked_shape_is_not_selectable() {
    let mut s = layer_rect();
    assert!(s.selectable(), "an unlocked, visible shape is selectable");
    s.set_locked(true);
    assert!(s.locked());
    assert!(s.visible(), "locking doesn't change visibility");
    assert!(!s.selectable(), "a locked shape can't be selected / picked");
    // It still geometrically contains the point — only the *gate* blocks the pick.
    assert!(s.hit(5.0, 5.0, 1.0), "geometry is unchanged by the lock");
    s.toggle_locked();
    assert!(s.selectable(), "unlocking restores selectability");
}

/// A **hidden** shape is excluded from selection / hit-testing via the shared
/// gate (the canvas pick paths use `selectable()`, the renderers use `visible()`).
#[test]
fn hidden_shape_is_not_selectable() {
    let mut s = layer_rect();
    s.toggle_visible();
    assert!(!s.visible());
    assert!(!s.selectable(), "a hidden shape can't be picked");
    // The hit-test geometry itself is unaffected; the gate is what excludes it.
    assert!(s.hit(5.0, 5.0, 1.0));
}

/// `selectable()` requires **both** visible and unlocked.
#[test]
fn selectable_requires_visible_and_unlocked() {
    let mut s = layer_rect();
    s.set_locked(true);
    s.toggle_visible(); // now hidden AND locked
    assert!(!s.selectable());
    s.set_locked(false);
    assert!(!s.selectable(), "still hidden");
    s.toggle_visible(); // now visible AND unlocked
    assert!(s.selectable());
}

/// The new Layers-panel metadata (name / locked / layer-colour) round-trips
/// through serde on a Shape, alongside the existing `visible` flag.
#[test]
fn layer_metadata_round_trips() {
    let mut s = layer_rect();
    s.set_name("Hero badge");
    s.set_locked(true);
    s.set_layer_color(Some([0.2, 0.4, 0.6, 1.0]));
    s.toggle_visible(); // hidden
    let doc = Document {
        shapes: vec![s],
        ..Default::default()
    };
    let json = serde_json::to_string(&doc).unwrap();
    let back: Document = serde_json::from_str(&json).unwrap();
    let r = &back.shapes[0];
    assert_eq!(r.name(), Some("Hero badge"));
    assert_eq!(r.display_name(), "Hero badge");
    assert!(r.locked());
    assert!(!r.visible());
    assert_eq!(r.layer_color(), Some([0.2, 0.4, 0.6, 1.0]));
}

/// A pre-Layers-panel `.contour` (no `name` / `locked` / `layer_color` keys)
/// deserializes with the additive defaults: unnamed (falls back to the type
/// label), unlocked, no layer colour — so older files load unchanged.
#[test]
fn legacy_document_defaults_layer_metadata() {
    let json = r#"{"shapes":[
        {"Rect":{"rect":[0,0,10,10],"fill":[1,0,0,1],"stroke":[0,0,0,1],"stroke_w":2}}
    ]}"#;
    let doc: Document = serde_json::from_str(json).unwrap();
    let r = &doc.shapes[0];
    assert_eq!(r.name(), None, "no stored name");
    assert_eq!(r.display_name(), "Rectangle", "falls back to the type label");
    assert!(!r.locked(), "unlocked by default");
    assert_eq!(r.layer_color(), None, "no layer colour by default");
    assert!(r.selectable(), "a legacy shape is selectable");
}

/// A blank name clears back to the type label (stored as `None`, not `""`).
#[test]
fn blank_name_clears_to_label() {
    let mut s = layer_rect();
    s.set_name("Renamed");
    assert_eq!(s.name(), Some("Renamed"));
    s.set_name("   ");
    assert_eq!(s.name(), None, "a blank name is cleared");
    assert_eq!(s.display_name(), "Rectangle");
}

/// `to_path` carries the Layers-panel metadata onto the converted path so a
/// rotated rectangle (which rasterises to a `Path`) keeps its name / lock /
/// colour.
#[test]
fn to_path_preserves_layer_metadata() {
    let mut s = layer_rect();
    s.set_name("Box");
    s.set_locked(true);
    s.set_layer_color(Some([0.1, 0.2, 0.3, 1.0]));
    let p = s.to_path();
    assert_eq!(p.name(), Some("Box"));
    assert!(p.locked());
    assert_eq!(p.layer_color(), Some([0.1, 0.2, 0.3, 1.0]));
}

// --- Type / text objects -----------------------------------------------

use crate::text::{TextAlign, TextParams};

/// A point-type text object with the given string at `origin`, glyphs laid out.
fn text_shape(text: &str, origin: (f32, f32)) -> Shape {
    let params = TextParams {
        text: text.to_string(),
        font_size: 72.0,
        align: TextAlign::Left,
        font_family: None,
    };
    let glyphs = crate::text::layout(&params, origin).0;
    Shape::Text {
        params,
        origin,
        glyphs,
        fill: [0.0, 0.0, 0.0, 1.0],
        fill_gradient: None,
        stroke: [0.0, 0.0, 0.0, 1.0],
        stroke_w: 0.0,
        stroke_style: StrokeStyle::default(),
        appearance: None,
        visible: true,
        group: None,
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
    }
}

/// A freshly-built text object has a non-empty glyph cache and tight bounds, and
/// fills under the even-odd rule (so glyph counters are holes).
#[test]
fn text_shape_has_glyphs_bounds_and_even_odd_fill() {
    let s = text_shape("Hi", (10.0, 20.0));
    match &s {
        Shape::Text { glyphs, .. } => assert!(!glyphs.is_empty(), "glyphs laid out"),
        _ => panic!("expected Text"),
    }
    let b = s.bounds().expect("text has bounds");
    assert!(b.w > 0.0 && b.h > 0.0, "non-empty bbox");
    assert_eq!(s.fill_rule(), Some(FillRule::EvenOdd));
    assert_eq!(s.label(), "Type");
    // The display name shows the string.
    assert_eq!(s.display_name(), "Hi");
}

/// Editing the params via `set_text_params` re-lays-out the glyph cache, growing
/// the bounds as the string lengthens.
#[test]
fn set_text_params_relays_out_glyphs() {
    let mut s = text_shape("I", (0.0, 0.0));
    let before = s.bounds().unwrap().w;
    let ok = s.set_text_params(TextParams {
        text: "IIIIIIII".into(),
        font_size: 72.0,
        align: TextAlign::Left,
        font_family: None,
    });
    assert!(ok, "set_text_params applies to a text object");
    let after = s.bounds().unwrap().w;
    assert!(after > before, "wider string widens the bbox: {before} -> {after}");
    // A non-text shape rejects the edit.
    let mut r = layer_rect();
    assert!(!r.set_text_params(TextParams::default()));
}

/// Convert-to-outlines turns a text object into a `Compound` of glyph contours,
/// preserving paint + even-odd fill rule, with non-empty geometry.
#[test]
fn text_to_outlines_yields_compound_paths() {
    let mut s = text_shape("Ag", (5.0, 5.0));
    s.set_fill_color([0.2, 0.4, 0.8, 1.0]);
    let out = s.text_to_outlines();
    match out {
        Shape::Compound {
            subpaths,
            fill_rule,
            fill,
            ..
        } => {
            assert!(!subpaths.is_empty(), "glyph contours become sub-paths");
            assert!(
                subpaths.iter().any(|sp| sp.points.len() >= 3),
                "real geometry"
            );
            assert_eq!(fill_rule, FillRule::EvenOdd, "counters stay holes");
            assert_eq!(fill, [0.2, 0.4, 0.8, 1.0], "paint carried through");
        }
        other => panic!("expected Compound, got {other:?}"),
    }
}

/// Translating a text object moves its origin *and* its cached glyph outlines.
#[test]
fn text_translate_moves_origin_and_glyphs() {
    let mut s = text_shape("X", (0.0, 0.0));
    let b0 = s.bounds().unwrap();
    s.translate(100.0, 50.0);
    let b1 = s.bounds().unwrap();
    assert!((b1.x - (b0.x + 100.0)).abs() < 1e-2);
    assert!((b1.y - (b0.y + 50.0)).abs() < 1e-2);
    if let Shape::Text { origin, .. } = &s {
        assert_eq!(*origin, (100.0, 50.0), "editable origin tracks the move");
    } else {
        panic!("still text");
    }
}

/// A text object round-trips through serde (params + origin + glyph cache + the
/// alignment field).
#[test]
fn text_round_trips_through_serde() {
    let mut s = text_shape("Round\nTrip", (12.0, 34.0));
    s.set_text_params(TextParams {
        text: "Round\nTrip".into(),
        font_size: 48.0,
        align: TextAlign::Center,
        font_family: None,
    });
    let doc = Document {
        shapes: vec![s],
        ..Default::default()
    };
    let json = serde_json::to_string(&doc).unwrap();
    let back: Document = serde_json::from_str(&json).unwrap();
    match &back.shapes[0] {
        Shape::Text {
            params,
            origin,
            glyphs,
            ..
        } => {
            assert_eq!(params.text, "Round\nTrip");
            assert_eq!(params.font_size, 48.0);
            assert_eq!(params.align, TextAlign::Center);
            assert_eq!(*origin, (12.0, 34.0));
            assert!(!glyphs.is_empty(), "glyph cache survives the round-trip");
        }
        other => panic!("expected Text, got {other:?}"),
    }
}

/// A hand-written / legacy text object missing the additive `glyphs` and `align`
/// keys deserializes with the back-compat defaults (empty cache, left align), and
/// `relayout_text` rebuilds the glyphs from `params` + `origin`.
#[test]
fn text_serde_defaults_and_relayout_rebuilds_cache() {
    let json = r#"{"shapes":[
        {"Text":{
            "params":{"text":"Hi","font_size":72.0},
            "origin":[0.0,0.0],
            "fill":[0,0,0,1],"stroke":[0,0,0,1],"stroke_w":0
        }}
    ]}"#;
    let mut doc: Document = serde_json::from_str(json).unwrap();
    match &doc.shapes[0] {
        Shape::Text { params, glyphs, .. } => {
            assert_eq!(params.align, TextAlign::Left, "align defaults to Left");
            assert!(glyphs.is_empty(), "glyph cache defaults to empty");
        }
        _ => panic!("expected Text"),
    }
    // Repairing the document lays the glyphs out from the params.
    doc.relayout_text();
    match &doc.shapes[0] {
        Shape::Text { glyphs, .. } => assert!(!glyphs.is_empty(), "relayout filled the cache"),
        _ => panic!("expected Text"),
    }
    // And the text now has real bounds.
    assert!(doc.shapes[0].bounds().is_some());
}

/// A text object hit-tests by its bounding box (so it is easy to select).
#[test]
fn text_hit_tests_inside_bounds() {
    let s = text_shape("Hi", (0.0, 0.0));
    let b = s.bounds().unwrap();
    let (cx, cy) = (b.x + b.w * 0.5, b.y + b.h * 0.5);
    assert!(s.hit(cx, cy, 0.1), "centre of the text box hits");
    assert!(!s.hit(b.x + b.w + 100.0, cy, 0.1), "far outside misses");
}

/// A pre-graphic-styles `.contour` (no `graphic_styles` key) loads with an empty
/// style library — the additive `#[serde(default)]` field keeps older files
/// round-tripping unchanged.
#[test]
fn loads_legacy_document_with_empty_graphic_styles() {
    let json = r#"{"shapes":[
        {"Rect":{"rect":[0,0,10,10],"fill":[1,0,0,1],"stroke":[0,0,0,1],"stroke_w":2}}
    ]}"#;
    let doc: Document = serde_json::from_str(json).unwrap();
    assert!(doc.graphic_styles.is_empty(), "missing key defaults to empty");
}

/// The document's graphic-styles library — each entry a full `Appearance`
/// snapshot — round-trips through `.contour` serialization unchanged.
#[test]
fn graphic_styles_library_round_trips_on_document() {
    use crate::appearance::{Appearance, BlendMode, Effect, Fill, Paint, Stroke as AppStroke};
    let style = Appearance {
        fills: vec![
            Fill::solid([0.1, 0.2, 0.3, 1.0]),
            Fill {
                paint: Paint::Gradient(crate::gradient::Gradient::default()),
                opacity: 0.5,
                blend: BlendMode::Multiply,
                visible: false,
            },
        ],
        strokes: vec![AppStroke {
            paint: Paint::Solid([1.0, 0.0, 0.0, 0.8]),
            width: 3.0,
            style: StrokeStyle::default(),
            opacity: 0.75,
            blend: BlendMode::Screen,
            visible: true,
        }],
        effects: vec![Effect::drop_shadow()],
    };
    let mut doc = Document::default();
    let id = doc.graphic_styles.add("Card", style.clone());

    let json = serde_json::to_string(&doc).unwrap();
    let back: Document = serde_json::from_str(&json).unwrap();
    assert_eq!(back.graphic_styles.len(), 1, "the style survives the round-trip");
    assert_eq!(back.graphic_styles.get(id).map(|s| s.name.as_str()), Some("Card"));
    // The whole captured appearance stack is preserved byte-for-byte.
    assert_eq!(back.graphic_styles.appearance_of(id), Some(&style));
}

/// A pre-symbols `.contour` (no `symbols` key) loads with an empty symbol
/// library — the additive `#[serde(default)]` field keeps older files
/// round-tripping unchanged.
#[test]
fn loads_legacy_document_with_empty_symbols() {
    let json = r#"{"shapes":[
        {"Rect":{"rect":[0,0,10,10],"fill":[1,0,0,1],"stroke":[0,0,0,1],"stroke_w":2}}
    ]}"#;
    let doc: Document = serde_json::from_str(json).unwrap();
    assert!(doc.symbols.is_empty(), "missing key defaults to empty");
    assert!(doc.symbols.instances.is_empty());
}

/// The document's symbol library + placed instances round-trip through `.contour`
/// serialization, and a master edit (re-serialized) propagates to instances.
#[test]
fn symbols_round_trip_and_propagate_on_document() {
    use crate::transform::Affine;
    let mut doc = Document::default();
    let sq = Shape::Rect {
        rect: [0.0, 0.0, 10.0, 10.0],
        fill: [1.0, 0.0, 0.0, 1.0],
        fill_gradient: None,
        stroke: [0.0, 0.0, 0.0, 1.0],
        stroke_w: 1.0,
        stroke_style: StrokeStyle::default(),
        appearance: None,
        visible: true,
        group: None,
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
    };
    let id = doc.symbols.add("Box", vec![sq]);
    doc.symbols.place(id, Affine::translate(100.0, 0.0));
    doc.symbols.place(id, Affine::translate(0.0, 100.0));

    // Round-trips: re-serializing the loaded doc matches the original JSON.
    let json = serde_json::to_string(&doc).unwrap();
    let back: Document = serde_json::from_str(&json).unwrap();
    assert_eq!(json, serde_json::to_string(&back).unwrap());
    assert_eq!(back.symbols.len(), 1);
    assert_eq!(back.symbols.instances.len(), 2);

    // Edit the master to 50 wide → both instances resolve 50 wide.
    let mut wider = back.symbols.get(id).unwrap().shapes[0].clone();
    if let Shape::Rect { rect, .. } = &mut wider {
        rect[2] = 50.0;
    }
    let mut edited = back;
    edited.symbols.set_master_shapes(id, vec![wider]);
    for inst in &edited.symbols.instances {
        let r = edited.symbols.resolve(inst);
        match &r[0] {
            Shape::Rect { rect, .. } => assert_eq!(rect[2], 50.0),
            _ => unreachable!(),
        }
    }
}

/// Build a placed point-type [`Shape::Text`] at `origin` for the text-placement
/// regression tests below: glyph cache laid out immediately, every additive
/// field at its default, so it matches what the Type tool produces.
#[cfg(test)]
fn placed_text(text: &str, font_size: f32, origin: (f32, f32)) -> Shape {
    use crate::text::TextParams;
    let params = TextParams {
        text: text.to_string(),
        font_size,
        align: crate::text::TextAlign::Left,
        font_family: None,
    };
    let glyphs = crate::text::layout(&params, origin).0;
    Shape::Text {
        params,
        origin,
        glyphs,
        fill: [0.0, 0.0, 0.0, 1.0],
        fill_gradient: None,
        stroke: [0.0, 0.0, 0.0, 1.0],
        stroke_w: 0.0,
        stroke_style: StrokeStyle::default(),
        appearance: None,
        visible: true,
        group: None,
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
    }
}

/// Changing a text object's **font size** (the inspector's Size path, which routes
/// through [`Shape::set_text_params`]) re-lays-out its glyphs but must keep the
/// object where the user placed it: the editable `origin` is untouched and the
/// re-extracted glyphs stay anchored at that origin — they do **not** jump to the
/// canvas corner (the Pigment-class "edit resets position to top-left" bug).
#[test]
fn text_size_change_preserves_origin_and_placement() {
    let origin = (137.0, 84.0);
    let mut shape = placed_text("Ag", 40.0, origin);

    // Top-left of the laid-out glyphs before the edit (should sit near `origin`).
    let before = shape.bounds().expect("placed text has bounds");

    let mut new = shape.text_params().unwrap().clone();
    new.font_size = 96.0; // larger size, fresh glyph extraction
    assert!(shape.set_text_params(new), "set_text_params applies to text");

    // The placement anchor is untouched.
    match &shape {
        Shape::Text { origin: o, .. } => assert_eq!(*o, origin, "origin must not move"),
        _ => panic!("still a text object"),
    }

    // The re-laid-out glyphs are still anchored at the origin: their top-left
    // tracks `origin.x` / `origin.y` exactly as before the edit (a small em-box
    // offset, never the (0,0) corner). The x-origin is exact; the y top lands a
    // hair below `origin.y` (the em-box top), and that offset is stable across
    // sizes only up to scale — so we assert the box did not jump to the corner.
    let after = shape.bounds().expect("resized text still has bounds");
    assert!(
        (after.x - before.x).abs() < 1.0,
        "glyph left edge stays put ({} vs {})",
        after.x,
        before.x
    );
    assert!(
        after.x > origin.0 - 1.0 && after.y > origin.1 - 1.0,
        "glyphs stay anchored at the placed origin, not the (0,0) corner: {:?}",
        (after.x, after.y)
    );
    assert!(
        after.w > before.w,
        "the bigger size produced wider glyphs (proves a real relayout happened)"
    );
}

/// Changing a text object's **font family** (the inspector's Font dropdown path,
/// also through [`Shape::set_text_params`]) re-extracts glyph outlines from a
/// different face but must not move the object: the `origin` is preserved and the
/// glyphs stay anchored there. Locks in that Contour does not have the
/// font-change-resets-position bug found in the sibling app.
#[test]
fn text_font_change_preserves_position() {
    let origin = (250.5, 60.0);
    let mut shape = placed_text("Hi", 50.0, origin);
    let before = shape.bounds().expect("placed text has bounds");

    let mut new = shape.text_params().unwrap().clone();
    // An unknown family resolves to the bundled face (so this runs identically on
    // any host), but it still exercises the full re-extract-on-family-change path.
    new.font_family = Some("No Such Font 99999".to_string());
    assert!(shape.set_text_params(new), "set_text_params applies to text");

    match &shape {
        Shape::Text { origin: o, params, .. } => {
            assert_eq!(*o, origin, "origin must survive a font-family change");
            assert_eq!(
                params.font_family.as_deref(),
                Some("No Such Font 99999"),
                "the chosen family is recorded"
            );
        }
        _ => panic!("still a text object"),
    }

    let after = shape.bounds().expect("re-faced text still has bounds");
    assert!(
        (after.x - before.x).abs() < 1.0 && (after.y - before.y).abs() < 1.0,
        "the text did not jump on a font change: {:?} -> {:?}",
        (before.x, before.y),
        (after.x, after.y)
    );
}

/// A text object that is **moved** and then has a property changed stays at the
/// moved location: translate keeps `origin` and the glyph cache in sync, and a
/// later [`Shape::set_text_params`] re-lays-out about the moved origin. This is
/// the end-to-end "place it, move it, change font/size — does it stay put?" path.
#[test]
fn moved_text_keeps_position_after_property_change() {
    let mut shape = placed_text("Ag", 48.0, (10.0, 10.0));
    shape.translate(200.0, 150.0); // drag the object across the canvas
    let moved_origin = match &shape {
        Shape::Text { origin, .. } => *origin,
        _ => unreachable!(),
    };
    assert_eq!(moved_origin, (210.0, 160.0), "translate moved the origin");
    let before = shape.bounds().expect("moved text has bounds");

    // Now change size + family in one edit, as the inspector does.
    let mut new = shape.text_params().unwrap().clone();
    new.font_size = 24.0;
    new.font_family = Some("Some Other Font".to_string());
    shape.set_text_params(new);

    match &shape {
        Shape::Text { origin, .. } => {
            assert_eq!(*origin, moved_origin, "origin stays at the moved location");
        }
        _ => panic!("still text"),
    }
    let after = shape.bounds().expect("edited text has bounds");
    // Left/top stay anchored near the moved origin (allowing the em-box offset),
    // and certainly nowhere near the original (10,10) placement or the corner.
    assert!(
        after.x > moved_origin.0 - 1.0 && after.y > moved_origin.1 - 1.0,
        "glyphs stay anchored at the moved origin after the edit: {:?}",
        (after.x, after.y)
    );
    assert!(
        (after.x - before.x).abs() < 1.0,
        "left edge unchanged by the edit ({} vs {})",
        after.x,
        before.x
    );
}
