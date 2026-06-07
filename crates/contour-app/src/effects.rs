//! **Live (non-destructive) effects** on the Appearance stack.
//!
//! egui's painter cannot blur, so live effects (drop-shadow, Gaussian blur) are
//! rendered the way Illustrator renders them and the way this app's PNG export
//! already works: **rasterize the affected geometry with `tiny-skia`, transform
//! that raster, and composite the result.** The shape's fills + strokes are
//! drawn into a padded `Pixmap` (the padding leaves room for a shadow / blur to
//! spill past the artwork), each [`Effect`] is applied to that raster in order,
//! and the processed pixmap is handed back to the caller — the canvas uploads it
//! as an egui texture, the PNG exporter draws it straight onto the page pixmap.
//!
//! The blur is a **three-pass separable box blur**, which converges on a true
//! Gaussian (central-limit) and is what most renderers ship for "Gaussian blur"
//! in raster filters. It runs directly on `tiny-skia`'s premultiplied RGBA8
//! buffer — blurring in premultiplied space is what keeps a soft edge from
//! haloing toward transparent-black. The pure pixel helpers are unit-tested with
//! no egui / GPU context.

use crate::appearance::Effect;
use tiny_skia::{BlendMode, Color, Pixmap, PixmapPaint, Transform};

/// A horizontal-then-vertical box blur of `radius` **pixels**, repeated `passes`
/// times. Three passes approximate a Gaussian closely; one pass is a plain box
/// blur. Operates in place on the premultiplied RGBA8 buffer.
pub fn box_blur(pixmap: &mut Pixmap, radius: f32, passes: u32) {
    let r = radius.round().max(0.0) as i32;
    if r <= 0 || passes == 0 {
        return;
    }
    let w = pixmap.width() as i32;
    let h = pixmap.height() as i32;
    if w == 0 || h == 0 {
        return;
    }
    let data = pixmap.data_mut();
    for _ in 0..passes {
        blur_pass_h(data, w, h, r);
        blur_pass_v(data, w, h, r);
    }
}

/// One horizontal box-blur pass over a premultiplied RGBA8 buffer. Each output
/// pixel is the average of the `2r+1` window centred on it (clamped at edges).
fn blur_pass_h(data: &mut [u8], w: i32, h: i32, r: i32) {
    let window = (2 * r + 1) as u32;
    let mut row: Vec<u8> = vec![0; (w * 4) as usize];
    for y in 0..h {
        let base = (y * w * 4) as usize;
        // Running sums of each channel over the window.
        let (mut sr, mut sg, mut sb, mut sa) = (0u32, 0u32, 0u32, 0u32);
        let at = |x: i32| {
            let xc = x.clamp(0, w - 1);
            base + (xc * 4) as usize
        };
        // Prime the window for x = 0: indices -r..=r.
        for dx in -r..=r {
            let i = at(dx);
            sr += data[i] as u32;
            sg += data[i + 1] as u32;
            sb += data[i + 2] as u32;
            sa += data[i + 3] as u32;
        }
        for x in 0..w {
            let o = (x * 4) as usize;
            row[o] = (sr / window) as u8;
            row[o + 1] = (sg / window) as u8;
            row[o + 2] = (sb / window) as u8;
            row[o + 3] = (sa / window) as u8;
            // Slide the window: drop x-r, add x+r+1.
            let out = at(x - r);
            let inc = at(x + r + 1);
            sr = sr - data[out] as u32 + data[inc] as u32;
            sg = sg - data[out + 1] as u32 + data[inc + 1] as u32;
            sb = sb - data[out + 2] as u32 + data[inc + 2] as u32;
            sa = sa - data[out + 3] as u32 + data[inc + 3] as u32;
        }
        data[base..base + (w * 4) as usize].copy_from_slice(&row);
    }
}

/// One vertical box-blur pass (the column analog of [`blur_pass_h`]).
fn blur_pass_v(data: &mut [u8], w: i32, h: i32, r: i32) {
    let window = (2 * r + 1) as u32;
    let mut col: Vec<u8> = vec![0; (h * 4) as usize];
    for x in 0..w {
        let (mut sr, mut sg, mut sb, mut sa) = (0u32, 0u32, 0u32, 0u32);
        let at = |y: i32| {
            let yc = y.clamp(0, h - 1);
            ((yc * w + x) * 4) as usize
        };
        for dy in -r..=r {
            let i = at(dy);
            sr += data[i] as u32;
            sg += data[i + 1] as u32;
            sb += data[i + 2] as u32;
            sa += data[i + 3] as u32;
        }
        for y in 0..h {
            let o = (y * 4) as usize;
            col[o] = (sr / window) as u8;
            col[o + 1] = (sg / window) as u8;
            col[o + 2] = (sb / window) as u8;
            col[o + 3] = (sa / window) as u8;
            let out = at(y - r);
            let inc = at(y + r + 1);
            sr = sr - data[out] as u32 + data[inc] as u32;
            sg = sg - data[out + 1] as u32 + data[inc + 1] as u32;
            sb = sb - data[out + 2] as u32 + data[inc + 2] as u32;
            sa = sa - data[out + 3] as u32 + data[inc + 3] as u32;
        }
        for y in 0..h {
            let i = ((y * w + x) * 4) as usize;
            let o = (y * 4) as usize;
            data[i..i + 4].copy_from_slice(&col[o..o + 4]);
        }
    }
}

/// Build a shadow layer from `src`'s **alpha silhouette**, tinted by `color`
/// (straight-sRGB RGBA, alpha = base strength) scaled by `opacity`, blurred by
/// `blur_px`. Returns a new same-size pixmap holding only the soft shadow (no
/// offset applied yet — the caller composites it offset).
fn shadow_layer(src: &Pixmap, color: [f32; 4], opacity: f32, blur_px: f32) -> Pixmap {
    let w = src.width();
    let h = src.height();
    let mut shadow = Pixmap::new(w, h).unwrap_or_else(|| Pixmap::new(1, 1).unwrap());
    let strength = (color[3] * opacity).clamp(0.0, 1.0);
    let (cr, cg, cb) = (
        color[0].clamp(0.0, 1.0),
        color[1].clamp(0.0, 1.0),
        color[2].clamp(0.0, 1.0),
    );
    {
        let s = src.data();
        let d = shadow.data_mut();
        for i in (0..s.len()).step_by(4) {
            // Source alpha drives the shadow alpha; tint with the shadow colour,
            // premultiplied (tiny-skia buffers are premultiplied RGBA8).
            let a = (s[i + 3] as f32 / 255.0) * strength;
            let pa = (a * 255.0).round() as u8;
            d[i] = (cr * a * 255.0).round() as u8;
            d[i + 1] = (cg * a * 255.0).round() as u8;
            d[i + 2] = (cb * a * 255.0).round() as u8;
            d[i + 3] = pa;
        }
    }
    box_blur(&mut shadow, blur_px, 3);
    shadow
}

/// Apply one [`Effect`] to a padded artwork `pixmap`, in place. `scale` is the
/// raster's pixels-per-document-unit, used to convert document-unit parameters
/// (offsets, radii) into pixels. Inactive effects ([`Effect::is_active`]) are
/// no-ops.
pub fn apply_effect(pixmap: &mut Pixmap, effect: &Effect, scale: f32) {
    if !effect.is_active() {
        return;
    }
    match effect {
        Effect::GaussianBlur { radius } => {
            box_blur(pixmap, radius * scale, 3);
        }
        Effect::DropShadow {
            dx,
            dy,
            blur,
            color,
            opacity,
        } => {
            let shadow = shadow_layer(pixmap, *color, *opacity, blur * scale);
            // Composite: shadow first (offset, source-over), then the original
            // artwork back on top. Build into a fresh pixmap so the artwork
            // overpaints the shadow where they overlap.
            let mut out = Pixmap::new(pixmap.width(), pixmap.height())
                .unwrap_or_else(|| Pixmap::new(1, 1).unwrap());
            let pp = PixmapPaint {
                blend_mode: BlendMode::SourceOver,
                ..PixmapPaint::default()
            };
            let ox = (dx * scale).round() as i32;
            let oy = (dy * scale).round() as i32;
            out.draw_pixmap(ox, oy, shadow.as_ref(), &pp, Transform::identity(), None);
            out.draw_pixmap(0, 0, pixmap.as_ref(), &pp, Transform::identity(), None);
            *pixmap = out;
        }
    }
}

/// Apply a whole stack of effects (bottom-to-top) to a padded artwork pixmap in
/// place. `scale` converts the effects' document-unit params to pixels.
pub fn apply_effects(pixmap: &mut Pixmap, effects: &[Effect], scale: f32) {
    for e in effects {
        apply_effect(pixmap, e, scale);
    }
}

/// Allocate a transparent pixmap of `(w, h)` pixels (clamped to ≥ 1×1).
pub fn transparent_pixmap(w: u32, h: u32) -> Pixmap {
    let mut p = Pixmap::new(w.max(1), h.max(1)).unwrap_or_else(|| Pixmap::new(1, 1).unwrap());
    p.fill(Color::TRANSPARENT);
    p
}

// --- Blend-mode compositing --------------------------------------------------

use crate::appearance::BlendMode as AppBlend;

/// Composite one channel of a source pixel over a backdrop pixel with a separable
/// blend mode, following the W3C compositing model in **straight** (un-
/// premultiplied) `0..=1` space, and return the resulting *premultiplied* output
/// channel.
///
/// The general separable composite is, per channel `c`:
/// ```text
/// co = αs·(1 − αb)·Cs + αs·αb·B(Cb, Cs) + (1 − αs)·αb·Cb
/// ```
/// where `αs`/`αb` are source/backdrop alpha, `Cs`/`Cb` the straight channels and
/// `B` the mode's blend function ([`AppBlend::blend`]). `co` is already the
/// premultiplied output channel (it is the un-premultiplied result times the
/// output alpha `αo = αs + αb·(1 − αs)`), so callers store it directly.
fn blend_channel(mode: AppBlend, cb: f32, cs: f32, ab: f32, a_s: f32) -> f32 {
    let blended = mode.blend(cb, cs);
    a_s * (1.0 - ab) * cs + a_s * ab * blended + (1.0 - a_s) * ab * cb
}

/// Composite the whole `src` pixmap onto `dst` (same size, both premultiplied
/// RGBA8) using a separable [`AppBlend`] mode. `Normal` falls back to the fast
/// `tiny-skia` source-over draw; every other mode runs the per-pixel separable
/// composite (de-premultiply, blend, re-store premultiplied).
///
/// Both buffers are the same dimensions because the caller rasterizes the blended
/// layer into a scratch pixmap matching the destination layer it is blending into.
pub fn composite_blended(dst: &mut Pixmap, src: &Pixmap, mode: AppBlend) {
    if !mode.is_separable_blend() {
        // Plain source-over: let tiny-skia do it (premultiplied, AA-correct).
        let pp = PixmapPaint {
            blend_mode: BlendMode::SourceOver,
            ..PixmapPaint::default()
        };
        dst.draw_pixmap(0, 0, src.as_ref(), &pp, Transform::identity(), None);
        return;
    }
    if dst.width() != src.width() || dst.height() != src.height() {
        return; // mismatched layers: skip rather than corrupt
    }
    let s = src.data().to_vec();
    let d = dst.data_mut();
    for i in (0..d.len()).step_by(4) {
        let a_s = s[i + 3] as f32 / 255.0;
        if a_s <= 0.0 {
            continue; // transparent source pixel leaves the backdrop untouched
        }
        let ab = d[i + 3] as f32 / 255.0;
        // De-premultiply both pixels to straight channels for the blend function.
        let unpm = |v: u8, a: f32| if a > 0.0 { (v as f32 / 255.0) / a } else { 0.0 };
        let (sb_r, sb_g, sb_b) = (unpm(s[i], a_s), unpm(s[i + 1], a_s), unpm(s[i + 2], a_s));
        let (db_r, db_g, db_b) = (unpm(d[i], ab), unpm(d[i + 1], ab), unpm(d[i + 2], ab));
        // Output alpha (source-over) and per-channel premultiplied composite.
        let ao = a_s + ab * (1.0 - a_s);
        let cr = blend_channel(mode, db_r, sb_r, ab, a_s);
        let cg = blend_channel(mode, db_g, sb_g, ab, a_s);
        let cb_ = blend_channel(mode, db_b, sb_b, ab, a_s);
        let to_u8 = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
        d[i] = to_u8(cr);
        d[i + 1] = to_u8(cg);
        d[i + 2] = to_u8(cb_);
        d[i + 3] = to_u8(ao);
    }
}

// --- Opacity masks -----------------------------------------------------------

/// The Rec. 709 relative luminance of a straight-sRGB channel triple in `0..=1`.
/// Drives an opacity mask: a mask shape's luminance becomes the masked object's
/// alpha multiplier (Illustrator's "Make Opacity Mask" — white reveals, black
/// hides).
pub fn luminance(r: f32, g: f32, b: f32) -> f32 {
    (0.2126 * r + 0.7152 * g + 0.0722 * b).clamp(0.0, 1.0)
}

/// Multiply an artwork pixmap's alpha by the **luminance** of a same-size mask
/// pixmap, in place (premultiplied RGBA8). For each pixel the mask's
/// luminance — weighted by the mask's own coverage (alpha), so a transparent mask
/// region reveals nothing — scales the artwork's alpha *and* its premultiplied
/// colour, keeping the buffer premultiplied. With `invert`, `1 − luminance` is
/// used instead (black reveals, white hides). This is the raster core of
/// `Object ▸ Opacity Mask ▸ Make`.
pub fn apply_luminance_mask(art: &mut Pixmap, mask: &Pixmap, invert: bool) {
    if art.width() != mask.width() || art.height() != mask.height() {
        return;
    }
    let m = mask.data().to_vec();
    let a = art.data_mut();
    for i in (0..a.len()).step_by(4) {
        let ma = m[i + 3] as f32 / 255.0;
        // Mask colour is premultiplied; de-premultiply to read its luminance.
        let (mr, mg, mb) = if ma > 0.0 {
            (
                (m[i] as f32 / 255.0) / ma,
                (m[i + 1] as f32 / 255.0) / ma,
                (m[i + 2] as f32 / 255.0) / ma,
            )
        } else {
            (0.0, 0.0, 0.0)
        };
        // Luminance weighted by mask coverage (outside the mask shape ⇒ alpha 0
        // ⇒ contributes 0, so the artwork is fully hidden there — like IL).
        let lum = luminance(mr, mg, mb) * ma;
        let factor = if invert { 1.0 - lum } else { lum };
        let scale = |v: u8| ((v as f32) * factor).round().clamp(0.0, 255.0) as u8;
        a[i] = scale(a[i]);
        a[i + 1] = scale(a[i + 1]);
        a[i + 2] = scale(a[i + 2]);
        a[i + 3] = scale(a[i + 3]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Make a transparent pixmap with one opaque white pixel at the centre.
    fn dot(size: u32) -> Pixmap {
        let mut p = transparent_pixmap(size, size);
        let c = size / 2;
        let i = ((c * size + c) * 4) as usize;
        let d = p.data_mut();
        d[i] = 255;
        d[i + 1] = 255;
        d[i + 2] = 255;
        d[i + 3] = 255;
        p
    }

    fn alpha_at(p: &Pixmap, x: u32, y: u32) -> u8 {
        p.pixel(x, y).map(|px| px.alpha()).unwrap_or(0)
    }

    #[test]
    fn box_blur_spreads_alpha_and_conserves_roughly() {
        let mut p = dot(11);
        let before_centre = alpha_at(&p, 5, 5);
        assert_eq!(before_centre, 255);
        // A neighbour starts empty.
        assert_eq!(alpha_at(&p, 5, 7), 0);
        box_blur(&mut p, 2.0, 3);
        // The blur lowers the peak and lifts the neighbours (alpha spreads).
        assert!(alpha_at(&p, 5, 5) < 255, "peak should drop");
        assert!(alpha_at(&p, 5, 7) > 0, "alpha should spread to neighbours");
    }

    #[test]
    fn box_blur_zero_radius_is_noop() {
        let mut p = dot(7);
        let before: Vec<u8> = p.data().to_vec();
        box_blur(&mut p, 0.0, 3);
        assert_eq!(p.data(), before.as_slice());
    }

    #[test]
    fn box_blur_symmetric_about_centre() {
        let mut p = dot(11);
        box_blur(&mut p, 2.0, 1);
        // Symmetric kernel ⇒ equal alpha on opposite sides of the centre.
        assert_eq!(alpha_at(&p, 3, 5), alpha_at(&p, 7, 5));
        assert_eq!(alpha_at(&p, 5, 3), alpha_at(&p, 5, 7));
    }

    #[test]
    fn drop_shadow_offsets_and_keeps_artwork() {
        // 21×21 with an opaque centre dot; shadow pushed +4,+4, no blur.
        let mut p = dot(21);
        let fx = Effect::DropShadow {
            dx: 4.0,
            dy: 4.0,
            blur: 0.0,
            color: [0.0, 0.0, 0.0, 1.0],
            opacity: 1.0,
        };
        apply_effect(&mut p, &fx, 1.0);
        // Original artwork still opaque at the centre.
        assert_eq!(alpha_at(&p, 10, 10), 255, "artwork preserved on top");
        // Shadow appears offset down-right of the centre.
        assert!(alpha_at(&p, 14, 14) > 0, "shadow at the offset position");
    }

    #[test]
    fn drop_shadow_inactive_when_transparent() {
        let mut p = dot(9);
        let before: Vec<u8> = p.data().to_vec();
        let fx = Effect::DropShadow {
            dx: 3.0,
            dy: 3.0,
            blur: 2.0,
            color: [0.0, 0.0, 0.0, 0.0], // transparent ⇒ no shadow
            opacity: 1.0,
        };
        apply_effect(&mut p, &fx, 1.0);
        assert_eq!(p.data(), before.as_slice(), "no-op for a transparent shadow");
    }

    #[test]
    fn gaussian_blur_effect_scales_with_doc_scale() {
        // Same radius, larger scale ⇒ wider spread in pixels.
        let mut a = dot(31);
        let mut b = dot(31);
        apply_effect(&mut a, &Effect::GaussianBlur { radius: 2.0 }, 1.0);
        apply_effect(&mut b, &Effect::GaussianBlur { radius: 2.0 }, 3.0);
        // At a point 6px out, the 3× scale blur should have spread more alpha.
        assert!(alpha_at(&b, 15, 21) >= alpha_at(&a, 15, 21));
    }

    // --- Blend compositing --------------------------------------------------

    /// Fill a 1×1 pixmap with a straight-sRGB colour (premultiplied internally).
    fn solid_px(c: [f32; 4]) -> Pixmap {
        let mut p = transparent_pixmap(1, 1);
        let a = c[3];
        let d = p.data_mut();
        d[0] = (c[0] * a * 255.0).round() as u8;
        d[1] = (c[1] * a * 255.0).round() as u8;
        d[2] = (c[2] * a * 255.0).round() as u8;
        d[3] = (a * 255.0).round() as u8;
        p
    }

    fn rgb(p: &Pixmap) -> (u8, u8, u8) {
        let px = p.pixel(0, 0).unwrap();
        (px.red(), px.green(), px.blue())
    }

    #[test]
    fn composite_multiply_darkens_opaque_pixels() {
        // 0.6 grey over 0.6 grey, fully opaque both → Multiply = 0.36 grey.
        let mut dst = solid_px([0.6, 0.6, 0.6, 1.0]);
        let src = solid_px([0.6, 0.6, 0.6, 1.0]);
        composite_blended(&mut dst, &src, AppBlend::Multiply);
        let (r, _, _) = rgb(&dst);
        let expected = (0.36_f32 * 255.0).round() as u8; // premultiplied == straight (α=1)
        assert!((r as i32 - expected as i32).abs() <= 2, "multiply r={r}");
    }

    #[test]
    fn composite_screen_lightens_opaque_pixels() {
        // 0.5 over 0.5 opaque → Screen = 0.75.
        let mut dst = solid_px([0.5, 0.5, 0.5, 1.0]);
        let src = solid_px([0.5, 0.5, 0.5, 1.0]);
        composite_blended(&mut dst, &src, AppBlend::Screen);
        let (r, _, _) = rgb(&dst);
        let expected = (0.75_f32 * 255.0).round() as u8;
        assert!((r as i32 - expected as i32).abs() <= 2, "screen r={r}");
    }

    #[test]
    fn composite_difference_yields_absolute_difference() {
        // |0.8 − 0.3| = 0.5 opaque.
        let mut dst = solid_px([0.8, 0.8, 0.8, 1.0]);
        let src = solid_px([0.3, 0.3, 0.3, 1.0]);
        composite_blended(&mut dst, &src, AppBlend::Difference);
        let (r, _, _) = rgb(&dst);
        let expected = (0.5_f32 * 255.0).round() as u8;
        assert!((r as i32 - expected as i32).abs() <= 2, "difference r={r}");
    }

    #[test]
    fn composite_transparent_source_leaves_backdrop() {
        let mut dst = solid_px([0.2, 0.4, 0.6, 1.0]);
        let before = rgb(&dst);
        let src = transparent_pixmap(1, 1); // fully transparent
        composite_blended(&mut dst, &src, AppBlend::Multiply);
        assert_eq!(rgb(&dst), before, "transparent source must not change backdrop");
    }

    #[test]
    fn composite_normal_is_source_over() {
        // Opaque source over a backdrop just replaces it (Normal path).
        let mut dst = solid_px([0.1, 0.2, 0.3, 1.0]);
        let src = solid_px([0.9, 0.8, 0.7, 1.0]);
        composite_blended(&mut dst, &src, AppBlend::Normal);
        let (r, g, b) = rgb(&dst);
        assert!(r > 200 && g > 180 && b > 150, "normal over: {r},{g},{b}");
    }

    // --- Opacity masks ------------------------------------------------------

    #[test]
    fn luminance_matches_rec709() {
        assert!((luminance(1.0, 1.0, 1.0) - 1.0).abs() < 1e-6); // white
        assert_eq!(luminance(0.0, 0.0, 0.0), 0.0); // black
        // Pure green dominates Rec.709 luma.
        assert!((luminance(0.0, 1.0, 0.0) - 0.7152).abs() < 1e-4);
    }

    #[test]
    fn opacity_mask_white_reveals_black_hides() {
        // Artwork: opaque red. White mask → unchanged; black mask → erased.
        let mut art_white = solid_px([1.0, 0.0, 0.0, 1.0]);
        let white = solid_px([1.0, 1.0, 1.0, 1.0]);
        apply_luminance_mask(&mut art_white, &white, false);
        assert_eq!(art_white.pixel(0, 0).unwrap().alpha(), 255, "white reveals");

        let mut art_black = solid_px([1.0, 0.0, 0.0, 1.0]);
        let black = solid_px([0.0, 0.0, 0.0, 1.0]);
        apply_luminance_mask(&mut art_black, &black, false);
        assert_eq!(art_black.pixel(0, 0).unwrap().alpha(), 0, "black hides");
    }

    #[test]
    fn opacity_mask_invert_swaps_reveal() {
        // Inverted: white now hides, black reveals.
        let mut art = solid_px([0.0, 1.0, 0.0, 1.0]);
        let white = solid_px([1.0, 1.0, 1.0, 1.0]);
        apply_luminance_mask(&mut art, &white, true);
        assert_eq!(art.pixel(0, 0).unwrap().alpha(), 0, "inverted white hides");
    }

    #[test]
    fn opacity_mask_midgrey_halves_alpha() {
        // A 50%-grey mask (luma ≈ 0.5) halves an opaque artwork's alpha.
        let mut art = solid_px([1.0, 1.0, 1.0, 1.0]);
        let grey = solid_px([0.5, 0.5, 0.5, 1.0]);
        apply_luminance_mask(&mut art, &grey, false);
        let a = art.pixel(0, 0).unwrap().alpha();
        assert!((a as i32 - 128).abs() <= 4, "mid-grey halves alpha, got {a}");
    }

    #[test]
    fn opacity_mask_transparent_region_hides() {
        // Outside the mask shape (transparent mask pixel) the artwork is hidden.
        let mut art = solid_px([1.0, 1.0, 1.0, 1.0]);
        let none = transparent_pixmap(1, 1);
        apply_luminance_mask(&mut art, &none, false);
        assert_eq!(art.pixel(0, 0).unwrap().alpha(), 0, "no mask coverage hides");
    }
}
