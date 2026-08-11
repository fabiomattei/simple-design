//! Drop ("outer") and inner shadow rendering (`model::Shadow`, `Style::shadows`/
//! `inner_shadows`). Deliberately has no `egui` UI / canvas dependency beyond
//! plain geometry types — same rationale as `masking.rs`: it operates purely
//! on `tiny_skia::Pixmap`s, taking a layer's already-rasterized silhouette
//! (its alpha-only shape, however it got drawn — `export::render_layer_plain`
//! for both `export.rs` and `canvas.rs`'s `ShadowTextureCache`) as input, so
//! both renderers produce pixel-identical shadows.
//!
//! `tiny-skia` has no built-in blur/filter pipeline, so blur here is a
//! 3-pass box blur — the standard cheap approximation of a Gaussian blur
//! (three box blurs in sequence converge to within a few percent of a true
//! Gaussian; see e.g. Ivan Kutskir's "Fastest Gaussian Blur"), sized from
//! `Shadow::blur` treated as `2*sigma`, matching the CSS `box-shadow` blur
//! radius convention this UI's numeric field mirrors. `spread` is a
//! separable max/min filter (square structuring element) — a reasonable
//! approximation of a circular spread for the small radii a shadow
//! effect actually uses.

use egui::{Color32, Vec2};
use tiny_skia::{Pixmap, PremultipliedColorU8};

use crate::model::Shadow;

/// A rendered, positioned outer shadow — `pixmap` is padded beyond the
/// source silhouette's own size to hold the blurred/spread falloff, so
/// `origin` (not the silhouette's own origin) is where its top-left should
/// land in the same coordinate space the silhouette was rasterized in.
pub struct ShadowLayer {
    pub pixmap: Pixmap,
    pub origin: Vec2,
}

/// Hard cap on the padding added around a silhouette for blur/spread, so a
/// pathological blur/spread value can't allocate an unbounded pixmap — same
/// safety rationale as `canvas.rs`'s `NOISE_TEXTURE_MAX`.
const MAX_SHADOW_PAD: u32 = 512;

fn shadow_pad(shadow: &Shadow) -> u32 {
    let pad = shadow.blur.max(0.0).ceil() + shadow.spread.max(0.0).ceil() + 4.0;
    (pad.max(0.0) as u32).min(MAX_SHADOW_PAD)
}

/// Worst-case distance any of `shadows` can paint beyond the shape's own
/// unpadded bounds, in any direction — used by `export::render_layer` to
/// size a standalone single-layer export so its drop shadows aren't clipped.
/// Symmetric (doesn't distinguish which side `offset` pushes toward), which
/// over-pads the opposite side slightly rather than needing a full per-side
/// margin for what's a rarely-hit export path.
pub fn outer_shadow_extent(shadows: &[Shadow]) -> f32 {
    shadows
        .iter()
        .filter(|s| s.color.a() > 0)
        .map(|s| {
            let pad = s.blur.max(0.0).ceil() + s.spread.max(0.0).ceil() + 4.0;
            pad + s.offset.x.abs().ceil().max(s.offset.y.abs().ceil())
        })
        .fold(0.0f32, f32::max)
}

/// Renders one outer/drop shadow from `silhouette` (a layer's own rasterized
/// alpha shape — RGB is ignored, only coverage matters). `silhouette_origin`
/// is where `silhouette`'s pixel `(0, 0)` sits in the caller's coordinate
/// space (mirrors `export::render_layer`'s own `offset` convention), used to
/// compute the returned `ShadowLayer::origin`. `None` for a fully transparent
/// shadow color (nothing to draw) or a zero-size silhouette.
pub fn render_outer_shadow(silhouette: &Pixmap, silhouette_origin: Vec2, shadow: &Shadow) -> Option<ShadowLayer> {
    if shadow.color.a() == 0 {
        return None;
    }
    let (sw, sh) = (silhouette.width(), silhouette.height());
    if sw == 0 || sh == 0 {
        return None;
    }
    let pad = shadow_pad(shadow);
    let width = sw + 2 * pad;
    let height = sh + 2 * pad;

    let mut alpha = vec![0.0f32; (width * height) as usize];
    for y in 0..sh {
        for x in 0..sw {
            let a = silhouette.pixel(x, y).map(|p| p.alpha() as f32 / 255.0).unwrap_or(0.0);
            alpha[((y + pad) * width + (x + pad)) as usize] = a;
        }
    }

    apply_spread(&mut alpha, width as usize, height as usize, shadow.spread);
    box_blur_alpha(&mut alpha, width as usize, height as usize, shadow.blur);

    let pixmap = tint_pixmap(&alpha, width, height, shadow.color)?;
    let origin = silhouette_origin + Vec2::new(-(pad as f32), -(pad as f32)) + shadow.offset;
    Some(ShadowLayer { pixmap, origin })
}

/// Renders one inner shadow, same size as `silhouette` (an inner shadow
/// never extends past the shape's own bounds). Built by shifting the shape's
/// own silhouette by `-shadow.offset` (the side the "light" comes from),
/// blurring the resulting mask, and taking what's inside the original shape
/// but *not* covered by that shifted mask — the standard inverted-mask inner
/// shadow technique. `None` for a fully transparent shadow color or a
/// zero-size silhouette.
pub fn render_inner_shadow(silhouette: &Pixmap, shadow: &Shadow) -> Option<Pixmap> {
    if shadow.color.a() == 0 {
        return None;
    }
    let (width, height) = (silhouette.width(), silhouette.height());
    if width == 0 || height == 0 {
        return None;
    }
    let shape_alpha: Vec<f32> = silhouette.pixels().iter().map(|p| p.alpha() as f32 / 255.0).collect();

    let ox = -shadow.offset.x.round() as i32;
    let oy = -shadow.offset.y.round() as i32;
    let mut shifted = vec![0.0f32; (width * height) as usize];
    for y in 0..height as i32 {
        for x in 0..width as i32 {
            let (sx, sy) = (x - ox, y - oy);
            if sx >= 0 && sy >= 0 && (sx as u32) < width && (sy as u32) < height {
                shifted[(y as u32 * width + x as u32) as usize] = shape_alpha[(sy as u32 * width + sx as u32) as usize];
            }
        }
    }

    // Positive spread tightens an inner shadow toward the edge (the common
    // convention) — grow the shifted "lit" mask so less of the shape reads
    // as shadow, the opposite sign from an outer shadow's spread.
    apply_spread(&mut shifted, width as usize, height as usize, shadow.spread);
    box_blur_alpha(&mut shifted, width as usize, height as usize, shadow.blur);

    let mut result = vec![0.0f32; (width * height) as usize];
    for i in 0..result.len() {
        result[i] = (shape_alpha[i] * (1.0 - shifted[i])).clamp(0.0, 1.0) * shape_alpha[i];
    }
    tint_pixmap(&result, width, height, shadow.color)
}

/// Positive `amount` dilates (grows) the mask, negative erodes (shrinks) it,
/// via a separable max/min filter — see module doc for the square-vs-circle
/// approximation tradeoff.
fn apply_spread(alpha: &mut [f32], width: usize, height: usize, amount: f32) {
    let radius = amount.round() as i32;
    if radius == 0 {
        return;
    }
    morph_horizontal(alpha, width, height, radius.abs(), radius > 0);
    morph_vertical(alpha, width, height, radius.abs(), radius > 0);
}

fn morph_horizontal(buf: &mut [f32], width: usize, height: usize, radius: i32, dilate: bool) {
    if radius <= 0 || width == 0 {
        return;
    }
    let src = buf.to_vec();
    for y in 0..height {
        let row = &src[y * width..(y + 1) * width];
        for x in 0..width {
            let lo = (x as i32 - radius).max(0) as usize;
            let hi = ((x as i32 + radius).min(width as i32 - 1)) as usize;
            let mut v = if dilate { 0.0f32 } else { 1.0f32 };
            for value in &row[lo..=hi] {
                v = if dilate { v.max(*value) } else { v.min(*value) };
            }
            buf[y * width + x] = v;
        }
    }
}

fn morph_vertical(buf: &mut [f32], width: usize, height: usize, radius: i32, dilate: bool) {
    if radius <= 0 || height == 0 {
        return;
    }
    let src = buf.to_vec();
    for x in 0..width {
        for y in 0..height {
            let lo = (y as i32 - radius).max(0) as usize;
            let hi = ((y as i32 + radius).min(height as i32 - 1)) as usize;
            let mut v = if dilate { 0.0f32 } else { 1.0f32 };
            for row in lo..=hi {
                let value = src[row * width + x];
                v = if dilate { v.max(value) } else { v.min(value) };
            }
            buf[y * width + x] = v;
        }
    }
}

/// In-place separable box blur of an alpha-only buffer, 3 passes sized from
/// `blur_px` (treated as `2*sigma`) via `box_radii_for_gauss` — see module
/// doc comment.
fn box_blur_alpha(alpha: &mut [f32], width: usize, height: usize, blur_px: f32) {
    if blur_px <= 0.0 || width == 0 || height == 0 {
        return;
    }
    let sigma = blur_px / 2.0;
    let mut buf = alpha.to_vec();
    let mut tmp = vec![0.0f32; alpha.len()];
    for radius in box_radii_for_gauss(sigma, 3) {
        box_blur_pass(&buf, &mut tmp, width, height, radius, true);
        box_blur_pass(&tmp, &mut buf, width, height, radius, false);
    }
    alpha.copy_from_slice(&buf);
}

/// Three box-blur radii that together approximate a Gaussian of standard
/// deviation `sigma` — the standard `boxesForGauss` construction (see e.g.
/// Ivan Kutskir's "Fastest Gaussian Blur", widely reused for this exact
/// no-real-blur-filter-available situation).
fn box_radii_for_gauss(sigma: f32, passes: i32) -> Vec<i32> {
    if sigma <= 0.0 {
        return vec![0; passes as usize];
    }
    let n = passes as f32;
    let ideal_width = (12.0 * sigma * sigma / n + 1.0).sqrt();
    let mut lower = ideal_width.floor() as i32;
    if lower % 2 == 0 {
        lower -= 1;
    }
    let upper = lower + 2;
    let ideal_m = (12.0 * sigma * sigma - n * (lower * lower) as f32 - 4.0 * n * lower as f32 - 3.0 * n) / (-4.0 * lower as f32 - 4.0);
    let m = ideal_m.round() as i32;
    (0..passes).map(|i| (if i < m { lower } else { upper }).max(0) / 2).collect()
}

/// One axis of a box blur, zero-padded at the buffer edge (there's nothing
/// beyond a shadow's own padded canvas, so clamping to the edge value would
/// incorrectly smear it outward) via a running-sum sliding window.
fn box_blur_pass(src: &[f32], dst: &mut [f32], width: usize, height: usize, radius: i32, horizontal: bool) {
    if radius <= 0 {
        dst.copy_from_slice(src);
        return;
    }
    let window = (2 * radius + 1) as f32;
    if horizontal {
        for y in 0..height {
            let row = &src[y * width..(y + 1) * width];
            let mut sum = 0.0f32;
            for x in 0..=radius {
                if (x as usize) < width {
                    sum += row[x as usize];
                }
            }
            for x in 0..width as i32 {
                dst[y * width + x as usize] = sum / window;
                let add_x = x + radius + 1;
                let sub_x = x - radius;
                if add_x >= 0 && (add_x as usize) < width {
                    sum += row[add_x as usize];
                }
                if sub_x >= 0 && (sub_x as usize) < width {
                    sum -= row[sub_x as usize];
                }
            }
        }
    } else {
        for x in 0..width {
            let mut sum = 0.0f32;
            for y in 0..=radius {
                if (y as usize) < height {
                    sum += src[y as usize * width + x];
                }
            }
            for y in 0..height as i32 {
                dst[y as usize * width + x] = sum / window;
                let add_y = y + radius + 1;
                let sub_y = y - radius;
                if add_y >= 0 && (add_y as usize) < height {
                    sum += src[add_y as usize * width + x];
                }
                if sub_y >= 0 && (sub_y as usize) < height {
                    sum -= src[sub_y as usize * width + x];
                }
            }
        }
    }
}

/// Colors a coverage-only alpha buffer with a flat `color`, premultiplying
/// as it goes — same premultiplied-`Pixmap` convention every other renderer
/// in this codebase writes directly (see `export.rs::blend_glyph_pixel`).
fn tint_pixmap(alpha: &[f32], width: u32, height: u32, color: Color32) -> Option<Pixmap> {
    let mut pixmap = Pixmap::new(width, height)?;
    let base_a = color.a() as f32 / 255.0;
    let (r, g, b) = (color.r() as f32, color.g() as f32, color.b() as f32);
    for (i, px) in pixmap.pixels_mut().iter_mut().enumerate() {
        let a = (alpha[i] * base_a).clamp(0.0, 1.0);
        if a <= 0.0 {
            continue;
        }
        let out_a = (a * 255.0).round().clamp(0.0, 255.0);
        let out_r = (r * a).round().clamp(0.0, out_a);
        let out_g = (g * a).round().clamp(0.0, out_a);
        let out_b = (b * a).round().clamp(0.0, out_a);
        if let Some(p) = PremultipliedColorU8::from_rgba(out_r as u8, out_g as u8, out_b as u8, out_a as u8) {
            *px = p;
        }
    }
    Some(pixmap)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn opaque_square(size: u32) -> Pixmap {
        let mut pixmap = Pixmap::new(size, size).unwrap();
        let paint = tiny_skia::Paint {
            anti_alias: false,
            ..Default::default()
        };
        let rect = tiny_skia::Rect::from_xywh(0.0, 0.0, size as f32, size as f32).unwrap();
        pixmap.fill_rect(rect, &paint, tiny_skia::Transform::identity(), None);
        pixmap
    }

    /// A zero-blur, zero-spread outer shadow is just the silhouette
    /// re-tinted and moved by `offset` — the padding degenerates to the
    /// minimum margin, and `origin` reflects the offset directly.
    #[test]
    fn zero_blur_outer_shadow_sits_at_silhouette_origin_plus_offset() {
        let square = opaque_square(20);
        let shadow = Shadow {
            color: Color32::BLACK,
            offset: Vec2::new(5.0, 3.0),
            blur: 0.0,
            spread: 0.0,
        };
        let layer = render_outer_shadow(&square, Vec2::ZERO, &shadow).unwrap();
        assert_eq!(layer.origin, Vec2::new(5.0, 3.0) + Vec2::new(-4.0, -4.0));
        // Center of the (padded) shadow pixmap should be fully opaque black.
        let center = (layer.pixmap.width() / 2, layer.pixmap.height() / 2);
        let px = layer.pixmap.pixel(center.0, center.1).unwrap();
        assert!(px.alpha() > 200, "expected opaque shadow center, got alpha={}", px.alpha());
    }

    /// A fully transparent shadow color produces nothing to draw.
    #[test]
    fn transparent_shadow_color_renders_nothing() {
        let square = opaque_square(20);
        let shadow = Shadow {
            color: Color32::TRANSPARENT,
            offset: Vec2::ZERO,
            blur: 4.0,
            spread: 0.0,
        };
        assert!(render_outer_shadow(&square, Vec2::ZERO, &shadow).is_none());
        assert!(render_inner_shadow(&square, &shadow).is_none());
    }

    /// Blur softens a hard silhouette edge: just outside the original
    /// silhouette, a blurred outer shadow should now have partial coverage
    /// instead of the hard `0` cutoff a zero-blur shadow would have.
    #[test]
    fn blur_spreads_coverage_past_the_silhouette_edge() {
        let square = opaque_square(20);
        let blurred = Shadow {
            color: Color32::BLACK,
            offset: Vec2::ZERO,
            blur: 12.0,
            spread: 0.0,
        };
        let layer = render_outer_shadow(&square, Vec2::ZERO, &blurred).unwrap();
        // One pixel outside the original square's right edge, vertically centered.
        let pad = layer.pixmap.width().saturating_sub(20) / 2;
        let x = pad + 21;
        let y = layer.pixmap.height() / 2;
        let px = layer.pixmap.pixel(x, y).unwrap();
        assert!(px.alpha() > 0, "blurred shadow should extend past the original silhouette edge");
    }

    /// An inner shadow never paints outside the original silhouette, even
    /// after blurring — it must stay re-clipped to the shape.
    #[test]
    fn inner_shadow_stays_within_the_original_silhouette() {
        let square = opaque_square(20);
        let shadow = Shadow {
            color: Color32::BLACK,
            offset: Vec2::new(4.0, 4.0),
            blur: 6.0,
            spread: 0.0,
        };
        let inner = render_inner_shadow(&square, &shadow).unwrap();
        assert_eq!((inner.width(), inner.height()), (20, 20));
        // A silhouette pixel that's transparent (outside a smaller inset
        // region we didn't fill) should stay transparent in the result.
        // Use a pixel that's part of the square but far from any edge, on
        // the side opposite the offset, which should still get some shadow.
        let far_corner = inner.pixel(1, 1).unwrap();
        assert!(far_corner.alpha() > 0, "corner opposite the light offset should show some inner shadow");
    }
}
