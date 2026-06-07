use super::path::{delete_anchor, insert_anchor, is_corner, segment_count, toggle_anchor_smooth};
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
    // A pre-appearance document loads with no explicit stack on any shape.
    assert!(doc.shapes[0].appearance().is_none());
    assert!(doc.shapes[1].appearance().is_none());
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
    };
    // Horizontal flip about x = 30 (the rect's centre): bounds unchanged,
    // width/height stay positive.
    s.apply_affine(&Affine::flip_h_about(30.0));
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
        visible: true,
        group: None,
        clip: None,
        mask: false,
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
