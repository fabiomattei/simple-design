//! Low-level glyph rasterization shared by every exported text path:
//! drawing one glyph's coverage, decoration rectangles, and blending a
//! coverage value into a pixel.

use ab_glyph::{Font, FontRef, GlyphId};
use tiny_skia::{Color, Pixmap, PremultipliedColorU8};

use super::sk_geometry::fill_rect;

/// Draws one glyph, faking bold (a second copy offset in x) and italic (a
/// per-scanline horizontal shear around the baseline) since neither is
/// available as a real font variant for the two bundled `TextFont`s — same
/// technique `canvas/draw_layer.rs`'s Text arm uses for bold. `clip`, if set
/// (`TextResize::Fixed`), drops any pixel outside it. `scale`, `x`,
/// `baseline_y` and `clip` are in document units; `render_scale` (pixmap
/// pixels per document unit, see `export::draw_layer`) converts them to
/// pixmap pixels here, so the glyph is rasterized at full resolution.
#[allow(clippy::too_many_arguments)]
pub(super) fn draw_text_glyph(
    pixmap: &mut Pixmap,
    face: &FontRef,
    id: GlyphId,
    scale: ab_glyph::PxScale,
    x: f32,
    baseline_y: f32,
    color: egui::Color32,
    bold: bool,
    italic: bool,
    clip: Option<egui::Rect>,
    render_scale: f32,
) {
    const ITALIC_SHEAR: f32 = 0.22;
    let scale = ab_glyph::PxScale { x: scale.x * render_scale, y: scale.y * render_scale };
    let (x, baseline_y) = (x * render_scale, baseline_y * render_scale);
    let clip = clip.map(|clip| egui::Rect::from_min_max((clip.min.to_vec2() * render_scale).to_pos2(), (clip.max.to_vec2() * render_scale).to_pos2()));
    let x_offsets: &[f32] = if bold { &[0.0, 0.6 * render_scale] } else { &[0.0] };
    for dx in x_offsets {
        let glyph = id.with_scale_and_position(scale, ab_glyph::point(x + dx, baseline_y));
        let Some(outlined) = face.outline_glyph(glyph) else { continue };
        let px_bounds = outlined.px_bounds();
        let (min_x, min_y) = (px_bounds.min.x as i32, px_bounds.min.y as i32);
        outlined.draw(|gx, gy, coverage| {
            let raw_y = min_y + gy as i32;
            let raw_x = min_x + gx as i32;
            let px = if italic {
                raw_x + ((baseline_y - raw_y as f32) * ITALIC_SHEAR).round() as i32
            } else {
                raw_x
            };
            if let Some(clip) = clip
                && ((px as f32) < clip.min.x
                    || (px as f32) >= clip.max.x
                    || (raw_y as f32) < clip.min.y
                    || (raw_y as f32) >= clip.max.y)
                {
                    return;
                }
            blend_glyph_pixel(pixmap, px, raw_y, coverage, color);
        });
    }
}

/// Fills an underline/strikethrough bar, intersected with `clip` (set for
/// `TextResize::Fixed`) so decorations don't spill past a clipped box. `rect`
/// and `clip` are in document units, scaled by `render_scale` at draw time.
pub(super) fn draw_decoration_rect(pixmap: &mut Pixmap, rect: egui::Rect, clip: Option<egui::Rect>, color: Color, render_scale: f32) {
    let rect = match clip {
        Some(clip) => rect.intersect(clip),
        None => rect,
    };
    if rect.width() > 0.0 && rect.height() > 0.0 {
        fill_rect(pixmap, rect, color, render_scale);
    }
}

/// Alpha-blends one glyph pixel (`coverage` in `0.0..=1.0`, from `ab_glyph`'s
/// rasterizer) over the existing pixmap contents using standard Porter-Duff
/// "over" compositing, done directly in premultiplied space since that's
/// what `Pixmap`'s backing store already holds.
pub(super) fn blend_glyph_pixel(pixmap: &mut Pixmap, x: i32, y: i32, coverage: f32, color: egui::Color32) {
    if x < 0 || y < 0 {
        return;
    }
    let (x, y) = (x as u32, y as u32);
    if x >= pixmap.width() || y >= pixmap.height() {
        return;
    }
    let alpha = coverage.clamp(0.0, 1.0) * (color.a() as f32 / 255.0);
    if alpha <= 0.0 {
        return;
    }
    let idx = (y * pixmap.width() + x) as usize;
    let dst = pixmap.pixels()[idx];
    let inv = 1.0 - alpha;
    let out_a = (255.0 * alpha + dst.alpha() as f32 * inv).round().clamp(0.0, 255.0);
    let out_r = (color.r() as f32 * alpha + dst.red() as f32 * inv).round().clamp(0.0, out_a);
    let out_g = (color.g() as f32 * alpha + dst.green() as f32 * inv).round().clamp(0.0, out_a);
    let out_b = (color.b() as f32 * alpha + dst.blue() as f32 * inv).round().clamp(0.0, out_a);
    if let Some(px) = PremultipliedColorU8::from_rgba(out_r as u8, out_g as u8, out_b as u8, out_a as u8) {
        pixmap.pixels_mut()[idx] = px;
    }
}
