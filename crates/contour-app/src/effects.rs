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
}
