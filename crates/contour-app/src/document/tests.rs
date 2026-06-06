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
    // A pre-guides document loads with no guides.
    assert!(doc.guides.is_empty());
    if let Shape::Path { handles, .. } = &doc.shapes[1] {
        assert!(handles.is_empty());
    } else {
        panic!("expected Path");
    }
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
        visible: true,
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
        visible: true,
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
        visible: true,
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
        visible: true,
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
        visible: true,
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
        handles: vec![(3.0, 4.0), (0.0, 0.0)],
        visible: true,
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
        visible: true,
    };
    line.set_fill_gradient(Some(Gradient::two_stop(
        GradientKind::Linear,
        [0.0; 4],
        [1.0; 4],
    )));
    assert!(line.fill_gradient().is_none());
    assert!(line.fill_color().is_none());
}
