//! Draws a `Text` layer: path-attached text goes to `text_on_path` or
//! `text_in_area`; everything else renders as plain or rich text, through an
//! offscreen pixmap when the layer is rotated.

use tiny_skia::{Pixmap, Transform};

use crate::model::text_runs::RunStyle;
use crate::model::{Layer, LayerKind};

use super::text_in_area::draw_text_in_area;
use super::text_on_path::draw_text_on_path;
use super::text_plain::draw_text;
use super::text_rich::draw_text_rich;

/// Draws `layer` (a `Text` layer; any other kind draws nothing) into
/// `pixmap`. `bounds` is its frame already translated by `offset`.
pub(super) fn draw_text_layer(
    pixmap: &mut Pixmap,
    layer: &Layer,
    roots: &[Layer],
    offset: egui::Vec2,
    bounds: egui::Rect,
    opacity: f32,
) {
    if draw_attached_text(pixmap, layer, roots, offset, opacity) {
        return;
    }
    if layer.frame.rotation == 0.0 {
        render_text(pixmap, layer, bounds, opacity);
    } else {
        // Glyph rasterization writes pixels directly (`ab_glyph`'s
        // `outlined.draw` callback in `glyph_raster.rs`), bypassing `tiny_skia::Path`/
        // `Transform` entirely — so rotation can't be passed through
        // the same way as the vector shapes in `draw_layer`. Instead: render at
        // full opacity into an unrotated offscreen sub-pixmap sized to
        // `bounds`, then composite that sub-pixmap onto the real
        // destination via `draw_pixmap` with a rotate-about-center
        // transform, mirroring `draw_image`'s existing
        // `PixmapPaint`+`Transform` composite pattern. `draw_text`/
        // `draw_text_rich` themselves are called completely unmodified.
        let w = bounds.width().round().max(1.0) as u32;
        let h = bounds.height().round().max(1.0) as u32;
        if let Some(mut sub) = Pixmap::new(w, h) {
            let local_bounds = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::Vec2::new(w as f32, h as f32));
            render_text(&mut sub, layer, local_bounds, 1.0);
            let paint = tiny_skia::PixmapPaint {
                opacity: opacity.clamp(0.0, 1.0),
                blend_mode: tiny_skia::BlendMode::SourceOver,
                quality: tiny_skia::FilterQuality::Bilinear,
            };
            let composite_transform = Transform::from_translate(bounds.min.x, bounds.min.y)
                .post_rotate_at(layer.frame.rotation, bounds.center().x, bounds.center().y);
            pixmap.draw_pixmap(0, 0, sub.as_ref(), &paint, composite_transform, None);
        }
    }
}

/// Draws `layer` along or inside its `path_attachment` target's outline and
/// returns `true`; returns `false` (drawing nothing) when it has no
/// attachment or the target can't be resolved, so the caller renders it as
/// ordinary text instead.
fn draw_attached_text(pixmap: &mut Pixmap, layer: &Layer, roots: &[Layer], offset: egui::Vec2, opacity: f32) -> bool {
    let LayerKind::Text {
        content,
        font_size,
        font,
        align,
        line_height,
        letter_spacing,
        bold,
        italic,
        underline,
        strikethrough,
        transform,
        runs,
        path_attachment,
        ..
    } = &layer.kind
    else {
        return false;
    };
    let attached = path_attachment.as_ref().and_then(|attachment| {
        let (outline, closed) = crate::text_path_geometry::resolve_target_outline(roots, attachment.target)?;
        // `resolve_target_outline` returns points relative to `roots`
        // itself (offset `0` at `roots`'s own top level) — converting
        // that into whatever pixmap-space `offset` this call happens
        // to be accumulating needs this layer's own position in that
        // same space, which `absolute_offset_of` gives without
        // threading a second offset accumulator through every draw
        // function alongside `roots` (see that function's doc comment).
        let self_root_offset = crate::text_path_geometry::absolute_offset_of(roots, layer.id).unwrap_or(egui::Vec2::ZERO);
        let pixmap_shift = offset - self_root_offset;
        let absolute_outline: Vec<egui::Pos2> = outline.iter().map(|&p| p + pixmap_shift).collect();
        Some((attachment, absolute_outline, closed))
    });
    if let Some((attachment, outline, closed)) = attached {
        match attachment.mode {
            crate::model::TextPathMode::OnPath => {
                let base = RunStyle {
                    font: font.clone(),
                    font_size: *font_size,
                    color: layer.style.fill.as_ref().map(|p| p.to_color32()),
                    bold: *bold,
                    italic: *italic,
                    underline: *underline,
                    strikethrough: *strikethrough,
                };
                draw_text_on_path(
                    pixmap,
                    content,
                    runs,
                    &base,
                    *align,
                    *letter_spacing,
                    *transform,
                    &outline,
                    closed,
                    attachment.offset,
                    attachment.start,
                    attachment.flip,
                    opacity * layer.style.fill_opacity,
                );
                return true;
            }
            crate::model::TextPathMode::AreaInside => {
                let base = RunStyle {
                    font: font.clone(),
                    font_size: *font_size,
                    color: layer.style.fill.as_ref().map(|p| p.to_color32()),
                    bold: *bold,
                    italic: *italic,
                    underline: *underline,
                    strikethrough: *strikethrough,
                };
                draw_text_in_area(
                    pixmap,
                    content,
                    runs,
                    &base,
                    *align,
                    *line_height,
                    *letter_spacing,
                    *transform,
                    &outline,
                    attachment.offset,
                    opacity * layer.style.fill_opacity,
                );
                return true;
            }
        }
    }
    false
}

/// `runs` non-empty is the rich-text path (see
/// `LayerKind::Text::runs`'s doc comment) — `draw_text` below is
/// untouched from before that feature existed, reached only for
/// the uniform-style case, so it can't regress.
fn render_text(target: &mut Pixmap, layer: &Layer, target_bounds: egui::Rect, target_opacity: f32) {
    let LayerKind::Text {
        content,
        font_size,
        font,
        align,
        vertical_align,
        resize,
        line_height,
        letter_spacing,
        paragraph_spacing,
        bold,
        italic,
        underline,
        strikethrough,
        transform,
        list,
        list_start,
        runs,
        ..
    } = &layer.kind
    else {
        return;
    };
    if runs.is_empty() {
        draw_text(
            target,
            layer,
            target_bounds,
            content,
            *font_size,
            font,
            *align,
            *vertical_align,
            *resize,
            *line_height,
            *letter_spacing,
            *paragraph_spacing,
            *bold,
            *italic,
            *underline,
            *strikethrough,
            *transform,
            *list,
            *list_start,
            target_opacity,
        );
    } else {
        let Some(fill) = layer.style.fill.as_ref() else { return };
        let base = RunStyle {
            font: font.clone(),
            font_size: *font_size,
            color: Some(fill.to_color32()),
            bold: *bold,
            italic: *italic,
            underline: *underline,
            strikethrough: *strikethrough,
        };
        draw_text_rich(
            target,
            target_bounds,
            content,
            runs,
            &base,
            *align,
            *vertical_align,
            *resize,
            *line_height,
            *letter_spacing,
            *paragraph_spacing,
            *transform,
            *list,
            *list_start,
            // `draw_text_rich` takes a `RunStyle`, not a whole
            // `Layer`, so `fill_opacity` (a `Style` field) has
            // to be folded into the opacity passed in here
            // rather than read inside it, unlike `draw_text`
            // just above (which does take `layer`).
            target_opacity * layer.style.fill_opacity,
        );
    }
}
