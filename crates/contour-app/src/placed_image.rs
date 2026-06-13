//! Placed / linked raster images — the document's **Place Image** model.
//!
//! Contour is a vector editor, but it can carry raster images as first-class
//! document elements (Illustrator's *File ▸ Place*). A [`PlacedImage`] references
//! a raster either **embedded** (the decoded RGBA pixels are stored in the
//! `.contour` file) or **linked** (only the on-disk path is stored, and the
//! pixels are re-read from disk on demand). Either way it carries its own
//! placement [`Affine`] — position / scale / rotation — applied to the image's
//! natural pixel rectangle, so the same indirection that drives a **symbol
//! instance** drives a placed image: the element is light, the transform is the
//! state, and the drawn rectangle is *derived*.
//!
//! An optional **clipping mask** bounds where the image draws: a closed path (a
//! ring of document-space points) outside of which the image is not painted. The
//! placement / clip math here is pure `f32` arithmetic and free of egui, so it is
//! unit-testable without a window:
//!
//! - [`PlacedImage::corners`] — the image's four corners under its transform.
//! - [`PlacedImage::drawn_bounds`] — the axis-aligned bounds of those corners.
//! - [`PlacedImage::clip_bounds`] — the effective drawn bounds, intersected with
//!   the clip path's bounds when a clip is set.
//! - [`PlacedImage::clip_contains`] — whether a document point is inside the clip
//!   (always true when there is no clip), the per-pixel containment test a
//!   rasterizer / hit-test shares.
//!
//! Embedded pixels round-trip through serde verbatim; a linked image stores only
//! its path and is resolved against the live file (an explicit relink/refresh —
//! live mtime polling is a follow-up). Everything is additive on the document
//! (`#[serde(default)]`), so a pre-Place `.contour` round-trips unchanged.

use crate::document::{point_in_rings, FillRule};
use crate::transform::Affine;
use prism_core::geometry::Rect as CoreRect;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Where a placed image's pixels come from.
///
/// `Embedded` stores the decoded image inline (RGBA8, row-major, `w·h·4`
/// bytes) so the document is self-contained. `Linked` stores only the file path;
/// the pixels are re-read from disk on demand (relink / refresh), keeping the
/// `.contour` small and the image editable in its source app. A `Linked` source
/// also caches the image's natural pixel size so placement math works before the
/// file is (re-)read.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum ImageSource {
    /// Pixels embedded in the document: natural size plus row-major RGBA8 bytes.
    Embedded {
        width: u32,
        height: u32,
        /// `width · height · 4` straight (un-premultiplied) RGBA8 bytes.
        rgba: Vec<u8>,
    },
    /// A link to an on-disk file; pixels are re-read from `path`. The natural
    /// `width`/`height` are cached from the last read so placement is stable even
    /// before a refresh.
    Linked {
        path: PathBuf,
        width: u32,
        height: u32,
    },
}

impl ImageSource {
    /// The image's natural (unscaled) pixel size.
    pub fn natural_size(&self) -> (u32, u32) {
        match self {
            ImageSource::Embedded { width, height, .. }
            | ImageSource::Linked { width, height, .. } => (*width, *height),
        }
    }

    /// Whether the pixels live in the document (vs. a disk link).
    pub fn is_embedded(&self) -> bool {
        matches!(self, ImageSource::Embedded { .. })
    }

    /// The linked file path, if this is a linked source.
    pub fn link_path(&self) -> Option<&std::path::Path> {
        match self {
            ImageSource::Linked { path, .. } => Some(path.as_path()),
            ImageSource::Embedded { .. } => None,
        }
    }
}

/// One placed raster image: a stable id, a display name, its pixel source, an
/// optional clipping path, and the placement transform applied to the image's
/// natural pixel rectangle `[0, 0, w, h]`.
///
/// No `PartialEq` derive is needed by callers; tests compare drawn geometry or
/// the serialized form (round-trip / determinism).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PlacedImage {
    /// Unique, stable identifier (never reused within a document).
    pub id: u64,
    /// Display name (the source file stem by default).
    pub name: String,
    /// Where the pixels come from (embedded or linked).
    pub source: ImageSource,
    /// Placement transform applied to the natural pixel rect `[0,0,w,h]`. Maps
    /// the image's top-left to its document position and carries scale / rotation.
    pub transform: Affine,
    /// Optional **clipping mask**: a closed ring of document-space points outside
    /// of which the image is not drawn. `None` = the whole image draws. Additive
    /// (`#[serde(default)]`), so a pre-clip placed image loads unclipped.
    #[serde(default)]
    pub clip: Option<Vec<(f32, f32)>>,
    /// Whether the image is drawn. Additive (`#[serde(default = "true")]`).
    #[serde(default = "default_true")]
    pub visible: bool,
    /// Whether the image is locked from selection / editing. Additive.
    #[serde(default)]
    pub locked: bool,
}

fn default_true() -> bool {
    true
}

impl PlacedImage {
    /// Build a placed image whose top-left sits at document `(x, y)` with the
    /// identity scale (one document unit per pixel).
    pub fn new(id: u64, name: impl Into<String>, source: ImageSource, x: f32, y: f32) -> Self {
        Self {
            id,
            name: name.into(),
            source,
            transform: Affine::translate(x, y),
            clip: None,
            visible: true,
            locked: false,
        }
    }

    /// The image's natural pixel size.
    pub fn natural_size(&self) -> (u32, u32) {
        self.source.natural_size()
    }

    /// The four corners of the image's natural rectangle pushed through the
    /// placement transform, clockwise from the top-left: TL, TR, BR, BL. Under a
    /// rotation these are a rotated quad; under translate/scale an axis-aligned
    /// rectangle.
    pub fn corners(&self) -> [(f32, f32); 4] {
        let (w, h) = self.natural_size();
        let (w, h) = (w as f32, h as f32);
        let t = &self.transform;
        [
            t.apply_point(0.0, 0.0),
            t.apply_point(w, 0.0),
            t.apply_point(w, h),
            t.apply_point(0.0, h),
        ]
    }

    /// Axis-aligned document-space bounds of the placed (transformed) image, or
    /// `None` for a degenerate (zero-area) image.
    pub fn drawn_bounds(&self) -> Option<CoreRect> {
        bounds_of(&self.corners())
    }

    /// The clip path's axis-aligned bounds, or `None` when there is no clip (or it
    /// is degenerate).
    pub fn clip_path_bounds(&self) -> Option<CoreRect> {
        let ring = self.clip.as_deref()?;
        bounds_of(ring)
    }

    /// The **effective** drawn bounds: the image's drawn bounds, intersected with
    /// the clip path's bounds when a clip is set. With no clip this equals
    /// [`drawn_bounds`](Self::drawn_bounds); with a clip it is no larger than
    /// either, so a clipping mask can only ever *shrink* the region the image
    /// occupies. `None` when the image is degenerate or the clip excludes it
    /// entirely.
    pub fn clip_bounds(&self) -> Option<CoreRect> {
        let drawn = self.drawn_bounds()?;
        match self.clip_path_bounds() {
            Some(clip) => intersect(&drawn, &clip),
            None => Some(drawn),
        }
    }

    /// Whether document point `(x, y)` is inside the clip region. Always `true`
    /// when the image has no clip (the whole image is drawable); otherwise the
    /// point must lie inside the clip ring (even-odd / non-zero agree for a simple
    /// ring). This is the per-pixel test a rasterizer and a hit-test share.
    pub fn clip_contains(&self, x: f32, y: f32) -> bool {
        match self.clip.as_deref() {
            None => true,
            Some(ring) => point_in_rings(x, y, &[ring.to_vec()], FillRule::NonZero),
        }
    }

    /// Whether the placed image takes part in selection / editing: visible and
    /// unlocked.
    pub fn selectable(&self) -> bool {
        self.visible && !self.locked
    }

    /// Whether document point `(x, y)` hits the drawn image: inside the
    /// transformed quad **and** inside the clip (a clipped image isn't hit where
    /// it isn't drawn). Uses the quad's even-odd containment so a rotated image
    /// hits correctly, then defers to [`clip_contains`](Self::clip_contains).
    pub fn hit(&self, x: f32, y: f32) -> bool {
        let quad = self.corners();
        let ring: Vec<(f32, f32)> = quad.to_vec();
        point_in_rings(x, y, &[ring], FillRule::NonZero) && self.clip_contains(x, y)
    }

    /// Set the clip ring (a closed path of document-space points). An empty ring
    /// clears the clip.
    pub fn set_clip(&mut self, ring: Vec<(f32, f32)>) {
        self.clip = (!ring.is_empty()).then_some(ring);
    }

    /// Remove the clipping mask (the whole image draws again).
    pub fn clear_clip(&mut self) {
        self.clip = None;
    }
}

/// Axis-aligned bounds of a set of points, or `None` when empty.
fn bounds_of(pts: &[(f32, f32)]) -> Option<CoreRect> {
    if pts.is_empty() {
        return None;
    }
    let (mut minx, mut miny) = (f32::INFINITY, f32::INFINITY);
    let (mut maxx, mut maxy) = (f32::NEG_INFINITY, f32::NEG_INFINITY);
    for &(x, y) in pts {
        minx = minx.min(x);
        miny = miny.min(y);
        maxx = maxx.max(x);
        maxy = maxy.max(y);
    }
    let (w, h) = (maxx - minx, maxy - miny);
    if w <= 0.0 || h <= 0.0 {
        return None;
    }
    Some(CoreRect {
        x: minx,
        y: miny,
        w,
        h,
    })
}

/// Intersection of two axis-aligned rectangles, or `None` if they do not overlap.
fn intersect(a: &CoreRect, b: &CoreRect) -> Option<CoreRect> {
    let x0 = a.x.max(b.x);
    let y0 = a.y.max(b.y);
    let x1 = (a.x + a.w).min(b.x + b.w);
    let y1 = (a.y + a.h).min(b.y + b.h);
    if x1 <= x0 || y1 <= y0 {
        return None;
    }
    Some(CoreRect {
        x: x0,
        y: y0,
        w: x1 - x0,
        h: y1 - y0,
    })
}

/// The document's placed-image collection, in paint order (drawn over the plain
/// shapes and symbol instances). Ids are unique and never reused within a
/// session. Empty by default — a fresh document places no images.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct PlacedImages {
    /// The placed images, bottom-to-top paint order.
    pub list: Vec<PlacedImage>,
}

impl PlacedImages {
    /// Whether no images are placed.
    pub fn is_empty(&self) -> bool {
        self.list.is_empty()
    }

    /// Number of placed images.
    pub fn len(&self) -> usize {
        self.list.len()
    }

    /// The next free id (one past the current maximum, so ids never collide and a
    /// removed slot's id is not reused within a session).
    pub fn next_id(&self) -> u64 {
        self.list.iter().map(|p| p.id).max().map_or(0, |m| m + 1)
    }

    /// Place `source` with its top-left at document `(x, y)`, returning the new
    /// image's id. The display `name` is stored verbatim.
    pub fn place(&mut self, name: &str, source: ImageSource, x: f32, y: f32) -> u64 {
        let id = self.next_id();
        self.list.push(PlacedImage::new(id, name, source, x, y));
        id
    }

    /// Immutable lookup by id.
    pub fn get(&self, id: u64) -> Option<&PlacedImage> {
        self.list.iter().find(|p| p.id == id)
    }

    /// Mutable lookup by id (to retransform / (re)clip / relink).
    pub fn get_mut(&mut self, id: u64) -> Option<&mut PlacedImage> {
        self.list.iter_mut().find(|p| p.id == id)
    }

    /// Remove the placed image `id`. Returns `true` if one was removed.
    pub fn remove(&mut self, id: u64) -> bool {
        let before = self.list.len();
        self.list.retain(|p| p.id != id);
        self.list.len() != before
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn embedded(w: u32, h: u32) -> ImageSource {
        ImageSource::Embedded {
            width: w,
            height: h,
            // One white pixel per cell — content is irrelevant to placement math.
            rgba: vec![255; (w * h * 4) as usize],
        }
    }

    fn approx(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-3
    }

    fn approx_rect(r: &CoreRect, x: f32, y: f32, w: f32, h: f32) -> bool {
        approx(r.x, x) && approx(r.y, y) && approx(r.w, w) && approx(r.h, h)
    }

    /// A placed image's drawn rect reflects its placement transform: moving it
    /// shifts the rect; scaling it grows the rect.
    #[test]
    fn drawn_bounds_follow_position_and_scale() {
        let mut img = PlacedImage::new(0, "a", embedded(100, 50), 10.0, 20.0);
        // Identity scale, translated to (10,20): a 100×50 rect there.
        let b = img.drawn_bounds().unwrap();
        assert!(approx_rect(&b, 10.0, 20.0, 100.0, 50.0));

        // Scale ×2 about the origin, then translate to (10,20).
        img.transform = Affine::scale(2.0, 3.0).then(Affine::translate(10.0, 20.0));
        let b = img.drawn_bounds().unwrap();
        assert!(approx_rect(&b, 10.0, 20.0, 200.0, 150.0), "scaled rect");
    }

    /// Corners are the natural rectangle pushed through the transform (top-left
    /// first, clockwise).
    #[test]
    fn corners_are_transformed_rectangle() {
        let img = PlacedImage::new(0, "a", embedded(4, 2), 5.0, 7.0);
        let c = img.corners();
        assert_eq!(c[0], (5.0, 7.0));
        assert_eq!(c[1], (9.0, 7.0));
        assert_eq!(c[2], (9.0, 9.0));
        assert_eq!(c[3], (5.0, 9.0));
    }

    /// A clipping mask limits the image's drawn region to (no larger than) the
    /// clip path bounds.
    #[test]
    fn clip_limits_drawn_region_to_path_bounds() {
        let mut img = PlacedImage::new(0, "a", embedded(100, 100), 0.0, 0.0);
        // Unclipped: full 100×100.
        assert!(approx_rect(&img.clip_bounds().unwrap(), 0.0, 0.0, 100.0, 100.0));
        // Clip to a 40×40 box in the middle.
        img.set_clip(vec![
            (30.0, 30.0),
            (70.0, 30.0),
            (70.0, 70.0),
            (30.0, 70.0),
        ]);
        let cb = img.clip_bounds().unwrap();
        assert!(approx_rect(&cb, 30.0, 30.0, 40.0, 40.0), "clipped to box");
        // The clipped region can never exceed the image bounds.
        assert!(cb.w <= 100.0 && cb.h <= 100.0);
    }

    /// `clip_contains` is always true with no clip, and respects the ring once set.
    #[test]
    fn clip_contains_respects_the_ring() {
        let mut img = PlacedImage::new(0, "a", embedded(100, 100), 0.0, 0.0);
        assert!(img.clip_contains(5.0, 5.0), "no clip → all inside");
        img.set_clip(vec![
            (30.0, 30.0),
            (70.0, 30.0),
            (70.0, 70.0),
            (30.0, 70.0),
        ]);
        assert!(img.clip_contains(50.0, 50.0), "centre inside clip");
        assert!(!img.clip_contains(10.0, 10.0), "corner outside clip");
    }

    /// A clip that does not overlap the image excludes it entirely.
    #[test]
    fn non_overlapping_clip_excludes_the_image() {
        let mut img = PlacedImage::new(0, "a", embedded(10, 10), 0.0, 0.0);
        img.set_clip(vec![
            (100.0, 100.0),
            (110.0, 100.0),
            (110.0, 110.0),
            (100.0, 110.0),
        ]);
        assert!(img.clip_bounds().is_none(), "disjoint clip → nothing drawn");
    }

    /// Embedding stores pixels; linking stores only the path + natural size.
    #[test]
    fn embed_stores_pixels_link_stores_path() {
        let e = embedded(2, 2);
        assert!(e.is_embedded());
        assert_eq!(e.natural_size(), (2, 2));
        match &e {
            ImageSource::Embedded { rgba, .. } => assert_eq!(rgba.len(), 16),
            _ => unreachable!(),
        }
        let l = ImageSource::Linked {
            path: PathBuf::from("/tmp/photo.png"),
            width: 8,
            height: 6,
        };
        assert!(!l.is_embedded());
        assert_eq!(l.natural_size(), (8, 6));
        assert_eq!(l.link_path(), Some(std::path::Path::new("/tmp/photo.png")));
    }

    /// Ids are one-past-max and never reuse a removed id within a session.
    #[test]
    fn ids_never_reuse_within_a_session() {
        let mut imgs = PlacedImages::default();
        assert_eq!(imgs.next_id(), 0);
        let a = imgs.place("a", embedded(2, 2), 0.0, 0.0);
        imgs.place("b", embedded(2, 2), 0.0, 0.0);
        assert_eq!(imgs.next_id(), 2);
        assert!(imgs.remove(a));
        assert_eq!(imgs.next_id(), 2, "removed id 0 not reused");
        assert!(!imgs.remove(a), "second remove is a no-op");
    }

    /// The whole collection (embedded + linked + clipped) round-trips through
    /// serde unchanged.
    #[test]
    fn serde_round_trip_preserves_images() {
        let mut imgs = PlacedImages::default();
        let a = imgs.place("photo", embedded(3, 3), 10.0, 10.0);
        imgs.get_mut(a).unwrap().set_clip(vec![
            (0.0, 0.0),
            (5.0, 0.0),
            (5.0, 5.0),
        ]);
        imgs.list.push(PlacedImage::new(
            7,
            "link",
            ImageSource::Linked {
                path: PathBuf::from("/x/y.png"),
                width: 4,
                height: 4,
            },
            1.0,
            2.0,
        ));
        let json = serde_json::to_string(&imgs).unwrap();
        let back: PlacedImages = serde_json::from_str(&json).unwrap();
        assert_eq!(json, serde_json::to_string(&back).unwrap());
        assert_eq!(back.len(), 2);
        assert!(back.get(a).unwrap().clip.is_some());
    }

    /// A legacy placed image (JSON missing the additive `clip` / `visible` /
    /// `locked` keys) deserializes with the defaults.
    #[test]
    fn legacy_placed_image_gets_defaults() {
        let json = r#"{
            "list": [{
                "id": 0,
                "name": "old",
                "source": { "Embedded": { "width": 2, "height": 2, "rgba": [0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0] } },
                "transform": { "a":1.0,"b":0.0,"c":0.0,"d":1.0,"e":0.0,"f":0.0 }
            }]
        }"#;
        let imgs: PlacedImages = serde_json::from_str(json).unwrap();
        let img = imgs.get(0).unwrap();
        assert!(img.clip.is_none(), "no clip by default");
        assert!(img.visible, "visible by default");
        assert!(!img.locked, "unlocked by default");
    }

    /// Determinism: serializing the same collection twice yields identical bytes.
    #[test]
    fn serialization_is_deterministic() {
        let mut imgs = PlacedImages::default();
        imgs.place("a", embedded(2, 2), 1.0, 1.0);
        let first = serde_json::to_string(&imgs).unwrap();
        let second = serde_json::to_string(&imgs).unwrap();
        assert_eq!(first, second);
    }
}
