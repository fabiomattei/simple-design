use std::borrow::Cow;

use ab_glyph::{Font, FontRef, GlyphId, ScaleFont};
use tiny_skia::{
    Color, FillRule, Paint, PathBuilder, Pixmap, PixmapPaint, PremultipliedColorU8, Rect as SkRect,
    Stroke as SkStroke, Transform,
};

use crate::boolean_ops;
use crate::model::text_runs::{RunStyle, TextRun};
use crate::model::{
    ColorAdjust, CornerRadii, Layer, LayerKind, ListType, PathPoint, PathPolygon, TextAlign, TextFont, TextResize,
    TextTransform, VerticalAlign,
};

/// Renders a single layer (and its descendants, if it's a container) to a
/// standalone pixmap sized to the layer's own *rotated* bounds (its actual
/// visual footprint — plain `bounds()` would clip a rotated layer's corners,
/// since they extend beyond the unrotated local rect), padded further to fit
/// any drop shadow the layer's own `Style::shadows` extends beyond that
/// (see `shadow::outer_shadow_extent`) — otherwise a standalone "Export
/// Layer as PNG" would clip the shadow at the shape's own edge. Artboards
/// render with their own background color; everything else renders on a
/// transparent background.
pub fn render_layer(layer: &Layer) -> Option<Pixmap> {
    let bounds = layer.frame.rotated_bounds();
    let pad = crate::shadow::outer_shadow_extent(&layer.style.shadows);
    let width = (bounds.width() + 2.0 * pad).round().max(1.0) as u32;
    let height = (bounds.height() + 2.0 * pad).round().max(1.0) as u32;
    let mut pixmap = Pixmap::new(width, height)?;
    let offset = egui::Vec2::new(-bounds.min.x + pad, -bounds.min.y + pad);
    draw_layer_with_shadows(&mut pixmap, layer, offset, 1.0);
    Some(pixmap)
}

/// Same as `render_layer`, but without shadow padding or drawing — a plain
/// silhouette/appearance render used internally as the input to shadow
/// rendering itself (`shadow::render_outer_shadow`/`render_inner_shadow`
/// need the shape's own rasterized alpha, not a shadow of it) and by
/// `canvas.rs`'s `ShadowTextureCache`, which needs the exact same silhouette
/// the exported PNG's shadow would be built from.
pub(crate) fn render_layer_plain(layer: &Layer) -> Option<(Pixmap, egui::Rect)> {
    let bounds = layer.frame.rotated_bounds();
    let width = bounds.width().round().max(1.0) as u32;
    let height = bounds.height().round().max(1.0) as u32;
    let mut pixmap = Pixmap::new(width, height)?;
    let offset = egui::Vec2::new(-bounds.min.x, -bounds.min.y);
    draw_layer(&mut pixmap, layer, offset, 1.0);
    Some((pixmap, bounds))
}

/// Renders `layer` (and descendants) to a standalone PNG and writes it to `path`.
pub fn export_layer_to_png(layer: &Layer, path: &std::path::Path) -> anyhow::Result<()> {
    let pixmap = render_layer(layer).ok_or_else(|| anyhow::anyhow!("layer has no visible area"))?;
    pixmap.save_png(path)?;
    Ok(())
}

/// `pub(crate)` (rather than private) so `masking.rs` can use it as the
/// per-layer drawing callback for both `export.rs`'s own masked runs and
/// `canvas.rs`'s masked-group texture cache — see `masking::composite_masked_run`.
pub(crate) fn draw_layer(pixmap: &mut Pixmap, layer: &Layer, offset: egui::Vec2, parent_opacity: f32) {
    if !layer.visible {
        return;
    }
    let opacity = parent_opacity * layer.opacity;
    let bounds = layer.frame.bounds().translate(offset);
    match &layer.kind {
        LayerKind::Artboard { children, background } => {
            fill_rect(pixmap, bounds, to_color_with_opacity(*background, opacity));
            let child_offset = offset + layer.frame.pos.to_vec2();
            draw_children(pixmap, children, child_offset, opacity);
        }
        LayerKind::Group { children } => {
            let child_offset = offset + layer.frame.pos.to_vec2();
            draw_children(pixmap, children, child_offset, opacity);
        }
        LayerKind::Rectangle { corner_radius } => {
            let Some(path) = rounded_rect_path(bounds, *corner_radius) else {
                return;
            };
            fill_and_stroke(pixmap, &path, layer, FillRule::Winding, bounds, opacity, rotation_transform(layer, bounds));
        }
        LayerKind::Oval => {
            let Some(sk_rect) = to_sk_rect(bounds) else {
                return;
            };
            let mut pb = PathBuilder::new();
            pb.push_oval(sk_rect);
            let Some(path) = pb.finish() else {
                return;
            };
            fill_and_stroke(pixmap, &path, layer, FillRule::Winding, bounds, opacity, rotation_transform(layer, bounds));
        }
        LayerKind::Star { points, inner_ratio } => {
            let pts = crate::shapes::star_points(bounds.center(), bounds.width() / 2.0, bounds.height() / 2.0, *points, *inner_ratio);
            let Some(path) = polygon_ring_path(&pts) else {
                return;
            };
            fill_and_stroke(pixmap, &path, layer, FillRule::Winding, bounds, opacity, rotation_transform(layer, bounds));
        }
        LayerKind::Polygon { sides } => {
            let pts = crate::shapes::polygon_points(bounds.center(), bounds.width() / 2.0, bounds.height() / 2.0, *sides);
            let Some(path) = polygon_ring_path(&pts) else {
                return;
            };
            fill_and_stroke(pixmap, &path, layer, FillRule::Winding, bounds, opacity, rotation_transform(layer, bounds));
        }
        LayerKind::Line => {
            let Some(stroke) = layer.style.stroke.as_ref() else {
                return;
            };
            let a = layer.frame.start() + offset;
            let b = layer.frame.end() + offset;
            let mut pb = PathBuilder::new();
            pb.move_to(a.x, a.y);
            pb.line_to(b.x, b.y);
            let Some(path) = pb.finish() else {
                return;
            };
            let mut noise_storage = None;
            let paint = to_sk_paint(&stroke.paint, bounds, opacity * layer.style.stroke_opacity, &mut noise_storage);
            let sk_stroke = SkStroke {
                width: stroke.width.max(0.5),
                line_cap: tiny_skia::LineCap::Round,
                ..Default::default()
            };
            pixmap.stroke_path(&path, &paint, &sk_stroke, rotation_transform(layer, bounds), None);
        }
        LayerKind::Arrow { start_cap, end_cap } => {
            let Some(stroke) = layer.style.stroke.as_ref() else {
                return;
            };
            let a = layer.frame.start() + offset;
            let b = layer.frame.end() + offset;
            let transform = rotation_transform(layer, bounds);
            let sk_stroke = SkStroke {
                width: stroke.width.max(0.5),
                line_cap: tiny_skia::LineCap::Round,
                ..Default::default()
            };
            let mut noise_storage = None;
            let paint = to_sk_paint(&stroke.paint, bounds, opacity * layer.style.stroke_opacity, &mut noise_storage);

            // Shaft + any `Line`-style chevron caps share one stroked path.
            let mut pb = PathBuilder::new();
            pb.move_to(a.x, a.y);
            pb.line_to(b.x, b.y);
            for (tip, outward, cap) in [(a, a - b, *start_cap), (b, b - a, *end_cap)] {
                if cap == crate::model::ArrowCap::Line {
                    if let Some((p1, p2)) = arrow_cap_wings(tip, outward, stroke.width) {
                        pb.move_to(p1.x, p1.y);
                        pb.line_to(tip.x, tip.y);
                        pb.line_to(p2.x, p2.y);
                    }
                }
            }
            if let Some(path) = pb.finish() {
                pixmap.stroke_path(&path, &paint, &sk_stroke, transform, None);
            }

            // Filled caps (Triangle/Disc) as separate fill paths, same color.
            for (tip, outward, cap) in [(a, a - b, *start_cap), (b, b - a, *end_cap)] {
                let fill_path = match cap {
                    crate::model::ArrowCap::Triangle => arrow_cap_wings(tip, outward, stroke.width)
                        .and_then(|(p1, p2)| polygon_ring_path(&[tip, p1, p2])),
                    crate::model::ArrowCap::Disc => {
                        let size = arrow_cap_size(stroke.width);
                        let dir = if outward.length() > 1e-3 { outward.normalized() } else { egui::Vec2::ZERO };
                        let center = tip - dir * size * 0.4;
                        let radius = size * 0.4;
                        to_sk_rect(egui::Rect::from_center_size(center, egui::Vec2::splat(radius * 2.0))).and_then(
                            |r| {
                                let mut pb = PathBuilder::new();
                                pb.push_oval(r);
                                pb.finish()
                            },
                        )
                    }
                    crate::model::ArrowCap::None | crate::model::ArrowCap::Line => None,
                };
                if let Some(path) = fill_path {
                    pixmap.fill_path(&path, &paint, FillRule::Winding, transform, None);
                }
            }
        }
        LayerKind::Path { points, closed } => {
            let Some(path) = path_to_sk_path(points, *closed, offset + layer.frame.pos.to_vec2()) else {
                return;
            };
            fill_and_stroke(pixmap, &path, layer, FillRule::Winding, bounds, opacity, rotation_transform(layer, bounds));
        }
        LayerKind::CompoundPath { polygons } => {
            let Some(path) = compound_path_to_sk_path(polygons, offset + layer.frame.pos.to_vec2()) else {
                return;
            };
            // Ring winding direction doesn't matter for `EvenOdd` — it only
            // cares about crossing parity, which is what makes holes work.
            fill_and_stroke(pixmap, &path, layer, FillRule::EvenOdd, bounds, opacity, rotation_transform(layer, bounds));
        }
        LayerKind::BooleanGroup { children } => {
            // No child-recursion here, unlike `Group`/`Artboard` — only the
            // computed geometry is drawn, using this layer's own `style`
            // (via `fill_and_stroke`), same as `CompoundPath` above.
            let combined = boolean_ops::compute_boolean_group(children);
            let polygons = boolean_ops::multipolygon_to_polygons(&combined);
            let Some(path) = compound_path_to_sk_path(&polygons, offset + layer.frame.pos.to_vec2()) else {
                return;
            };
            fill_and_stroke(pixmap, &path, layer, FillRule::EvenOdd, bounds, opacity, rotation_transform(layer, bounds));
        }
        LayerKind::Text {
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
        } => {
            // `runs` non-empty is the rich-text path (see
            // `LayerKind::Text::runs`'s doc comment) — `draw_text` below is
            // untouched from before that feature existed, reached only for
            // the uniform-style case, so it can't regress.
            let render = |target: &mut Pixmap, target_bounds: egui::Rect, target_opacity: f32| {
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
            };
            if layer.frame.rotation == 0.0 {
                render(pixmap, bounds, opacity);
            } else {
                // Glyph rasterization writes pixels directly (`ab_glyph`'s
                // `outlined.draw` callback below), bypassing `tiny_skia::Path`/
                // `Transform` entirely — so rotation can't be passed through
                // the same way as the vector shapes above. Instead: render at
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
                    render(&mut sub, local_bounds, 1.0);
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
        LayerKind::Image { encoded, color_adjust, .. } => {
            draw_image(pixmap, encoded, *color_adjust, bounds, opacity, layer.frame.rotation);
        }
    }
}

/// Draws one parent's `children` (an `Artboard`/`Group`'s, already offset to
/// that parent's coordinate space), applying `Layer::is_mask`/`ignore_mask`
/// via `masking::partition_mask_runs` — see that function's doc comment for
/// the masked-run semantics. Deliberately shadow-free (plain `draw_layer`)
/// — this is what `render_layer_plain`'s silhouette rendering recurses
/// through for a `Group`/`Artboard`, so a parent's own shadow is computed
/// from its children's *shapes*, not from halos of the children's own
/// shadows. `draw_children_with_shadows` is the shadow-aware sibling used
/// for real rendering.
fn draw_children(pixmap: &mut Pixmap, children: &[Layer], offset: egui::Vec2, opacity: f32) {
    for unit in crate::masking::partition_mask_runs(children) {
        match unit {
            crate::masking::RenderUnit::Plain(child) => draw_layer(pixmap, child, offset, opacity),
            crate::masking::RenderUnit::Masked { mask, content } => {
                crate::masking::composite_masked_run(pixmap, mask, &content, offset, opacity, draw_layer);
            }
        }
    }
}

/// `draw_children`'s shadow-aware sibling: same masked-run partitioning, but
/// draws every child (plain or masked-run content) through
/// `draw_layer_with_shadows` instead of plain `draw_layer`, so nested
/// layers at any depth get their own `Style::shadows`/`inner_shadows`.
fn draw_children_with_shadows(pixmap: &mut Pixmap, children: &[Layer], offset: egui::Vec2, opacity: f32) {
    for unit in crate::masking::partition_mask_runs(children) {
        match unit {
            crate::masking::RenderUnit::Plain(child) => draw_layer_with_shadows(pixmap, child, offset, opacity),
            crate::masking::RenderUnit::Masked { mask, content } => {
                crate::masking::composite_masked_run(pixmap, mask, &content, offset, opacity, draw_layer_with_shadows);
            }
        }
    }
}

/// Wraps a layer's normal appearance with `layer.style.shadows`/
/// `inner_shadows`: outer shadows composited *behind* it (drawn first), then
/// its normal appearance, then inner shadows on top, clipped inside it.
/// Renders the layer's own silhouette once (via `render_layer_plain`, which
/// is always shadow-free — no recursion risk) and reuses it for every
/// stacked shadow rather than re-rasterizing per shadow.
///
/// `Group`/`Artboard` are special-cased to recurse via
/// `draw_children_with_shadows` (so descendants get their own shadows too);
/// every other kind has no children of its own and can delegate straight to
/// plain `draw_layer`. This duplicates `draw_layer`'s few lines of
/// `Artboard`/`Group` handling rather than threading a children-drawing
/// callback through `draw_layer`'s signature — `draw_layer` itself must stay
/// shadow-free (see `draw_children`'s doc comment for why), so it can't just
/// call back into this function for its own recursion.
pub(crate) fn draw_layer_with_shadows(pixmap: &mut Pixmap, layer: &Layer, offset: egui::Vec2, parent_opacity: f32) {
    if !layer.visible {
        return;
    }
    let has_outer = !layer.style.shadows.is_empty();
    let has_inner = !layer.style.inner_shadows.is_empty();
    let silhouette = if has_outer || has_inner { render_layer_plain(layer) } else { None };

    if let (true, Some((silhouette, bounds))) = (has_outer, &silhouette) {
        let silhouette_origin = egui::Vec2::new(bounds.min.x, bounds.min.y) + offset;
        for shadow in &layer.style.shadows {
            if let Some(crate::shadow::ShadowLayer { pixmap: shadow_px, origin }) =
                crate::shadow::render_outer_shadow(silhouette, silhouette_origin, shadow)
            {
                let paint = PixmapPaint { opacity: parent_opacity, ..Default::default() };
                pixmap.draw_pixmap(origin.x.round() as i32, origin.y.round() as i32, shadow_px.as_ref(), &paint, Transform::identity(), None);
            }
        }
    }

    let opacity = parent_opacity * layer.opacity;
    match &layer.kind {
        LayerKind::Artboard { children, background } => {
            let bounds = layer.frame.bounds().translate(offset);
            fill_rect(pixmap, bounds, to_color_with_opacity(*background, opacity));
            let child_offset = offset + layer.frame.pos.to_vec2();
            draw_children_with_shadows(pixmap, children, child_offset, opacity);
        }
        LayerKind::Group { children } => {
            let child_offset = offset + layer.frame.pos.to_vec2();
            draw_children_with_shadows(pixmap, children, child_offset, opacity);
        }
        _ => draw_layer(pixmap, layer, offset, parent_opacity),
    }

    if let (true, Some((silhouette, bounds))) = (has_inner, &silhouette) {
        let draw_at = bounds.min + offset;
        for shadow in &layer.style.inner_shadows {
            if let Some(inner_px) = crate::shadow::render_inner_shadow(silhouette, shadow) {
                let paint = PixmapPaint { opacity: parent_opacity, ..Default::default() };
                pixmap.draw_pixmap(draw_at.x.round() as i32, draw_at.y.round() as i32, inner_px.as_ref(), &paint, Transform::identity(), None);
            }
        }
    }
}

/// Rasterizes an `Image` layer into `pixmap`, scaled from its native pixel
/// size to `bounds` (already offset-translated, same as every other shape
/// case above) with bilinear filtering, applying the non-destructive Color
/// Adjust effect first — mirrors what the canvas texture cache does for the
/// live view (`canvas.rs`'s `ImageTextureCache`), so exported PNGs match.
fn draw_image(
    pixmap: &mut Pixmap,
    encoded: &[u8],
    color_adjust: ColorAdjust,
    bounds: egui::Rect,
    opacity: f32,
    rotation: f32,
) {
    let Some(decoded) = crate::image_ops::decode(encoded) else { return };
    let adjusted = crate::image_ops::apply_color_adjust(&decoded, color_adjust);
    let (width, height) = (adjusted.width(), adjusted.height());
    if width == 0 || height == 0 || bounds.width().abs() < 0.01 || bounds.height().abs() < 0.01 {
        return;
    }
    let Some(size) = tiny_skia::IntSize::from_wh(width, height) else { return };
    let premultiplied = crate::image_ops::to_premultiplied_rgba(&adjusted);
    let Some(src) = Pixmap::from_vec(premultiplied, size) else { return };

    let mut transform = Transform::from_translate(bounds.min.x, bounds.min.y)
        .pre_scale(bounds.width() / width as f32, bounds.height() / height as f32);
    if rotation != 0.0 {
        transform = transform.post_rotate_at(rotation, bounds.center().x, bounds.center().y);
    }
    let paint = tiny_skia::PixmapPaint {
        opacity: opacity.clamp(0.0, 1.0),
        blend_mode: tiny_skia::BlendMode::SourceOver,
        quality: tiny_skia::FilterQuality::Bilinear,
    };
    pixmap.draw_pixmap(0, 0, src.as_ref(), &paint, transform, None);
}

/// Rasterizes a `Text` layer's glyphs directly onto `pixmap` via `ab_glyph`,
/// blending each glyph's per-pixel AA coverage over the existing contents.
/// `bounds` is `layer.frame.bounds()` already translated by `offset` (same
/// value the caller derived for other shapes). Uses the same bundled font
/// files (`epaint_default_fonts`) the canvas draws with via egui, so export
/// visually matches the on-canvas glyphs.
///
/// Independent of `canvas.rs`/`text_layout.rs`'s `egui::Galley`-based layout
/// (per this codebase's convention of canvas/export being separate render
/// paths — see `CLAUDE.md`) except for `text_layout::display_string`, which
/// is pure string manipulation with no rendering-backend dependency and so
/// is shared to keep list numbering/text-transform behavior identical
/// between the two.
#[allow(clippy::too_many_arguments)]
fn draw_text(
    pixmap: &mut Pixmap,
    layer: &Layer,
    bounds: egui::Rect,
    content: &str,
    font_size: f32,
    font: &TextFont,
    align: TextAlign,
    vertical_align: VerticalAlign,
    resize: TextResize,
    line_height: Option<f32>,
    letter_spacing: f32,
    paragraph_spacing: f32,
    bold: bool,
    italic: bool,
    underline: bool,
    strikethrough: bool,
    transform: TextTransform,
    list: ListType,
    list_start: i32,
    opacity: f32,
) {
    if content.is_empty() {
        return;
    }
    let Some(fill) = layer.style.fill.as_ref() else { return };
    let color = with_opacity(fill.to_color32(), opacity * layer.style.fill_opacity);
    let (font_bytes, face_index) = crate::fonts::ab_glyph_bytes(font);
    let Ok(face) = FontRef::try_from_slice_and_index(&font_bytes, face_index) else { return };
    let scale = ab_glyph::PxScale::from(font_size);
    let scaled = face.as_scaled(scale);
    let line_step = line_height.unwrap_or(scaled.height() + scaled.line_gap());

    let measure = |s: &str| -> f32 {
        let mut width = 0.0;
        let mut prev: Option<GlyphId> = None;
        for c in s.chars() {
            let id = scaled.glyph_id(c);
            if let Some(p) = prev {
                width += scaled.kern(p, id);
            }
            width += scaled.h_advance(id) + letter_spacing;
            prev = Some(id);
        }
        width
    };
    let space_width = scaled.h_advance(scaled.glyph_id(' ')) + letter_spacing;

    let display = crate::text_layout::display_string(content, transform, list, list_start);
    // This app's content model has no soft-return, so — matching
    // `text_layout::layout_paragraphs` — each `\n`-separated authored line
    // is its own paragraph; `resize != Auto` additionally word-wraps a
    // paragraph into further visual lines at `bounds.width()`.
    let max_width = match resize {
        TextResize::Auto => f32::INFINITY,
        TextResize::AutoHeight | TextResize::Fixed => bounds.width(),
    };

    struct VLine {
        text: String,
        width: f32,
        justify: bool,
    }
    let mut lines: Vec<VLine> = Vec::new();
    let mut is_paragraph_start: Vec<bool> = Vec::new();
    for paragraph in display.split('\n') {
        let wrapped = wrap_line(paragraph, &measure, space_width, max_width);
        let n = wrapped.len();
        for (i, text) in wrapped.into_iter().enumerate() {
            let width = measure(&text);
            let justify = align == TextAlign::Justify && i + 1 < n;
            is_paragraph_start.push(i == 0);
            lines.push(VLine { text, width, justify });
        }
    }

    let mut y_offsets = Vec::with_capacity(lines.len());
    let mut cursor = 0.0f32;
    for (i, &is_start) in is_paragraph_start.iter().enumerate() {
        if i > 0 {
            cursor += line_step;
            if is_start {
                cursor += paragraph_spacing;
            }
        }
        y_offsets.push(cursor);
    }
    let total_height = y_offsets.last().copied().unwrap_or(0.0) + line_step;

    let start_y = match vertical_align {
        VerticalAlign::Top => bounds.min.y,
        VerticalAlign::Middle => bounds.min.y + (bounds.height() - total_height) / 2.0,
        VerticalAlign::Bottom => bounds.min.y + bounds.height() - total_height,
    };
    let clip = (resize == TextResize::Fixed).then_some(bounds);
    let deco_thickness = (font_size * 0.06).max(1.0);

    for (line, &y_off) in lines.iter().zip(&y_offsets) {
        let baseline_y = start_y + y_off + scaled.ascent();
        let start_x = match align {
            TextAlign::Left | TextAlign::Justify => bounds.min.x,
            TextAlign::Center => bounds.min.x + (bounds.width() - line.width) / 2.0,
            TextAlign::Right => bounds.max.x - line.width,
        };
        let gap_count = line.text.matches(' ').count();
        let justify_extra = if line.justify && gap_count > 0 {
            ((max_width - line.width) / gap_count as f32).max(0.0)
        } else {
            0.0
        };

        let mut cursor_x = start_x;
        let mut prev: Option<GlyphId> = None;
        for c in line.text.chars() {
            let id = scaled.glyph_id(c);
            if let Some(p) = prev {
                cursor_x += scaled.kern(p, id);
            }
            draw_text_glyph(pixmap, &face, id, scale, cursor_x, baseline_y, color, bold, italic, clip);
            cursor_x += scaled.h_advance(id) + letter_spacing;
            if c == ' ' {
                cursor_x += justify_extra;
            }
            prev = Some(id);
        }

        if (underline || strikethrough) && cursor_x > start_x {
            if underline {
                let y = baseline_y + font_size * 0.08;
                let rect = egui::Rect::from_min_size(
                    egui::Pos2::new(start_x, y),
                    egui::Vec2::new(cursor_x - start_x, deco_thickness),
                );
                draw_decoration_rect(pixmap, rect, clip, to_color(color));
            }
            if strikethrough {
                let y = baseline_y - font_size * 0.3;
                let rect = egui::Rect::from_min_size(
                    egui::Pos2::new(start_x, y),
                    egui::Vec2::new(cursor_x - start_x, deco_thickness),
                );
                draw_decoration_rect(pixmap, rect, clip, to_color(color));
            }
        }
    }
}

/// Rich-text counterpart to `draw_text`, used when `runs` is non-empty —
/// see `LayerKind::Text::runs`'s doc comment. Generalizes wrapping/
/// measuring from "one shared face/scale for the whole call" to a
/// character-driven walk that looks up the right run's face/scale per
/// character (via `tagged_lines`, the same char→run tagging
/// `text_layout::layout_paragraphs_rich` uses for the canvas render, so
/// the two can't disagree on where a run boundary falls). Kerning only
/// applies between two characters resolved to the *same* face — kerning
/// across a font/run boundary isn't well-defined and the visual difference
/// is negligible. `bold` runs use `fonts::ab_glyph_bytes_bold` (a real
/// bold-weight face) rather than `draw_text_glyph`'s double-draw hack,
/// matching the canvas rich-text render (so `draw_text_glyph` is always
/// called with `bold: false` here).
///
/// Auto (`line_height: None`) line spacing is computed per visual row from
/// the tallest run actually present on that row, so mixing font sizes
/// looks right; an explicit `line_height` overrides that uniformly for
/// every row, same as `draw_text`.
#[allow(clippy::too_many_arguments)]
fn draw_text_rich(
    pixmap: &mut Pixmap,
    bounds: egui::Rect,
    content: &str,
    runs: &[TextRun],
    base: &RunStyle,
    align: TextAlign,
    vertical_align: VerticalAlign,
    resize: TextResize,
    line_height: Option<f32>,
    letter_spacing: f32,
    paragraph_spacing: f32,
    transform: TextTransform,
    list: ListType,
    list_start: i32,
    opacity: f32,
) {
    if content.is_empty() {
        return;
    }

    // One `(bytes, face_index)` per style source: [0] = base, [i+1] = runs[i].
    let mut byte_bufs: Vec<(Cow<'static, [u8]>, u32)> = Vec::with_capacity(runs.len() + 1);
    byte_bufs.push(if base.bold {
        crate::fonts::ab_glyph_bytes_bold(&base.font)
    } else {
        crate::fonts::ab_glyph_bytes(&base.font)
    });
    for run in runs {
        byte_bufs.push(if run.style.bold {
            crate::fonts::ab_glyph_bytes_bold(&run.style.font)
        } else {
            crate::fonts::ab_glyph_bytes(&run.style.font)
        });
    }

    let Ok(base_face) = FontRef::try_from_slice_and_index(&byte_bufs[0].0, byte_bufs[0].1) else { return };
    let mut faces: Vec<FontRef> = vec![base_face];
    // `runs` index -> index into `faces` (falls back to 0 / base's face if
    // that specific run's bytes fail to parse — shouldn't happen, but
    // drawing the wrong weight beats drawing nothing).
    let mut face_for_run: Vec<usize> = Vec::with_capacity(runs.len());
    for (bytes, index) in &byte_bufs[1..] {
        match FontRef::try_from_slice_and_index(bytes, *index) {
            Ok(f) => {
                face_for_run.push(faces.len());
                faces.push(f);
            }
            Err(_) => face_for_run.push(0),
        }
    }
    let style_of = |idx: Option<usize>| -> &RunStyle {
        match idx {
            None => base,
            Some(i) => runs.get(i).map_or(base, |r| &r.style),
        }
    };
    let face_index_of = |idx: Option<usize>| -> usize {
        match idx {
            None => 0,
            Some(i) => face_for_run.get(i).copied().unwrap_or(0),
        }
    };
    let scale_of = |idx: Option<usize>| ab_glyph::PxScale::from(style_of(idx).font_size);

    let measure = |chars: &[(char, Option<usize>)]| -> f32 {
        let mut width = 0.0;
        let mut prev: Option<(GlyphId, usize)> = None;
        for &(c, idx) in chars {
            let face_idx = face_index_of(idx);
            let scaled = faces[face_idx].as_scaled(scale_of(idx));
            let id = scaled.glyph_id(c);
            if let Some((prev_id, prev_face_idx)) = prev {
                if prev_face_idx == face_idx {
                    width += scaled.kern(prev_id, id);
                }
            }
            width += scaled.h_advance(id) + letter_spacing;
            prev = Some((id, face_idx));
        }
        width
    };
    let base_scaled = faces[0].as_scaled(scale_of(None));
    let space_width = base_scaled.h_advance(base_scaled.glyph_id(' ')) + letter_spacing;

    let max_width = match resize {
        TextResize::Auto => f32::INFINITY,
        TextResize::AutoHeight | TextResize::Fixed => bounds.width(),
    };

    struct VRow {
        chars: Vec<(char, Option<usize>)>,
        width: f32,
        justify: bool,
        height: f32,
        ascent: f32,
    }
    let mut rows: Vec<VRow> = Vec::new();
    let mut is_paragraph_start: Vec<bool> = Vec::new();
    for paragraph in crate::text_layout::tagged_lines(content, runs, transform, list, list_start) {
        let wrapped = wrap_line_rich(&paragraph, &measure, space_width, max_width);
        let n = wrapped.len();
        for (i, chars) in wrapped.into_iter().enumerate() {
            let width = measure(&chars);
            let justify = align == TextAlign::Justify && i + 1 < n;
            let (height, ascent) = if let Some(fixed) = line_height {
                (fixed, base_scaled.ascent())
            } else if chars.is_empty() {
                (base_scaled.height() + base_scaled.line_gap(), base_scaled.ascent())
            } else {
                chars
                    .iter()
                    .map(|&(_, idx)| {
                        let scaled = faces[face_index_of(idx)].as_scaled(scale_of(idx));
                        (scaled.height() + scaled.line_gap(), scaled.ascent())
                    })
                    .fold((0.0f32, 0.0f32), |(h, a), (h2, a2)| (h.max(h2), a.max(a2)))
            };
            is_paragraph_start.push(i == 0);
            rows.push(VRow { chars, width, justify, height, ascent });
        }
    }

    let mut y_offsets = Vec::with_capacity(rows.len());
    let mut cursor = 0.0f32;
    for (i, &is_start) in is_paragraph_start.iter().enumerate() {
        if i > 0 {
            cursor += rows[i - 1].height;
            if is_start {
                cursor += paragraph_spacing;
            }
        }
        y_offsets.push(cursor);
    }
    let total_height = y_offsets.last().copied().unwrap_or(0.0) + rows.last().map_or(0.0, |r| r.height);

    let start_y = match vertical_align {
        VerticalAlign::Top => bounds.min.y,
        VerticalAlign::Middle => bounds.min.y + (bounds.height() - total_height) / 2.0,
        VerticalAlign::Bottom => bounds.min.y + bounds.height() - total_height,
    };
    let clip = (resize == TextResize::Fixed).then_some(bounds);

    for (row, &y_off) in rows.iter().zip(&y_offsets) {
        let baseline_y = start_y + y_off + row.ascent;
        let start_x = match align {
            TextAlign::Left | TextAlign::Justify => bounds.min.x,
            TextAlign::Center => bounds.min.x + (bounds.width() - row.width) / 2.0,
            TextAlign::Right => bounds.max.x - row.width,
        };
        let gap_count = row.chars.iter().filter(|&&(c, _)| c == ' ').count();
        let justify_extra = if row.justify && gap_count > 0 {
            ((max_width - row.width) / gap_count as f32).max(0.0)
        } else {
            0.0
        };

        let mut cursor_x = start_x;
        let mut prev: Option<(GlyphId, usize)> = None;
        // (x_start, x_end, style index) per char, so decorations below can
        // be grouped into contiguous same-decoration/color stretches
        // rather than one rect for the whole row.
        let mut extents: Vec<(f32, f32, Option<usize>)> = Vec::with_capacity(row.chars.len());
        for &(c, idx) in &row.chars {
            let face_idx = face_index_of(idx);
            let scale = scale_of(idx);
            let scaled = faces[face_idx].as_scaled(scale);
            let id = scaled.glyph_id(c);
            if let Some((prev_id, prev_face_idx)) = prev {
                if prev_face_idx == face_idx {
                    cursor_x += scaled.kern(prev_id, id);
                }
            }
            let style = style_of(idx);
            let run_color = with_opacity(style.color.unwrap_or(base.color.unwrap_or(egui::Color32::BLACK)), opacity);
            let x_before = cursor_x;
            draw_text_glyph(pixmap, &faces[face_idx], id, scale, cursor_x, baseline_y, run_color, false, style.italic, clip);
            cursor_x += scaled.h_advance(id) + letter_spacing;
            if c == ' ' {
                cursor_x += justify_extra;
            }
            extents.push((x_before, cursor_x, idx));
            prev = Some((id, face_idx));
        }

        // Group into contiguous stretches sharing the same decoration
        // flags + color, and draw one underline/strikethrough rect per
        // stretch (rather than assuming the whole row is uniform).
        let mut i = 0usize;
        while i < extents.len() {
            let idx = extents[i].2;
            let style = style_of(idx);
            let mut j = i + 1;
            while j < extents.len() && style_of(extents[j].2).underline == style.underline
                && style_of(extents[j].2).strikethrough == style.strikethrough
                && style_of(extents[j].2).color == style.color
            {
                j += 1;
            }
            if style.underline || style.strikethrough {
                let x0 = extents[i].0;
                let x1 = extents[j - 1].1;
                if x1 > x0 {
                    let deco_color = with_opacity(style.color.unwrap_or(base.color.unwrap_or(egui::Color32::BLACK)), opacity);
                    let deco_thickness = (style.font_size * 0.06).max(1.0);
                    if style.underline {
                        let y = baseline_y + style.font_size * 0.08;
                        let rect = egui::Rect::from_min_size(egui::Pos2::new(x0, y), egui::Vec2::new(x1 - x0, deco_thickness));
                        draw_decoration_rect(pixmap, rect, clip, to_color(deco_color));
                    }
                    if style.strikethrough {
                        let y = baseline_y - style.font_size * 0.3;
                        let rect = egui::Rect::from_min_size(egui::Pos2::new(x0, y), egui::Vec2::new(x1 - x0, deco_thickness));
                        draw_decoration_rect(pixmap, rect, clip, to_color(deco_color));
                    }
                }
            }
            i = j;
        }
    }
}

/// Greedy word-wrap of one authored line into visual lines no wider than
/// `max_width` (an infinite `max_width` — `TextResize::Auto` — always
/// returns the line unchanged). Wraps at whitespace only, and multiple
/// consecutive spaces collapse to one — a simplification acceptable for
/// this export-only rewrap (the canvas's `egui::Galley`-based wrapping,
/// which this doesn't need to match pixel-for-pixel, handles the general
/// case).
pub(crate) fn wrap_line(line: &str, measure: &dyn Fn(&str) -> f32, space_width: f32, max_width: f32) -> Vec<String> {
    if !max_width.is_finite() || line.is_empty() {
        return vec![line.to_string()];
    }
    let mut lines = Vec::new();
    let mut current = String::new();
    let mut current_width = 0.0;
    for word in line.split_whitespace() {
        let word_width = measure(word);
        if current.is_empty() {
            current.push_str(word);
            current_width = word_width;
        } else if current_width + space_width + word_width <= max_width {
            current.push(' ');
            current.push_str(word);
            current_width += space_width + word_width;
        } else {
            lines.push(std::mem::take(&mut current));
            current.push_str(word);
            current_width = word_width;
        }
    }
    lines.push(current);
    lines
}

/// Rich-text counterpart to `wrap_line` — same greedy break-at-whitespace
/// policy and same "multiple consecutive spaces collapse to one"
/// simplification, but measures each candidate word by walking its own
/// characters and looking up the right run's face/scale per character (via
/// `measure`) instead of one shared face for the whole call. The
/// artificial space re-inserted between wrapped words is measured using
/// `space_width` (computed by the caller from the *base* style) regardless
/// of which run borders it — a small, deliberate extension of the same
/// "space width is approximate" simplification `wrap_line` already makes.
pub(crate) fn wrap_line_rich(
    line: &[(char, Option<usize>)],
    measure: &dyn Fn(&[(char, Option<usize>)]) -> f32,
    space_width: f32,
    max_width: f32,
) -> Vec<Vec<(char, Option<usize>)>> {
    if !max_width.is_finite() || line.is_empty() {
        return vec![line.to_vec()];
    }
    let mut words: Vec<Vec<(char, Option<usize>)>> = Vec::new();
    let mut word: Vec<(char, Option<usize>)> = Vec::new();
    for &(c, idx) in line {
        if c.is_whitespace() {
            if !word.is_empty() {
                words.push(std::mem::take(&mut word));
            }
        } else {
            word.push((c, idx));
        }
    }
    if !word.is_empty() {
        words.push(word);
    }

    let mut lines: Vec<Vec<(char, Option<usize>)>> = Vec::new();
    let mut current: Vec<(char, Option<usize>)> = Vec::new();
    let mut current_width = 0.0;
    for w in words {
        let word_width = measure(&w);
        if current.is_empty() {
            current_width = word_width;
            current = w;
        } else if current_width + space_width + word_width <= max_width {
            current.push((' ', None));
            current.extend(w);
            current_width += space_width + word_width;
        } else {
            lines.push(std::mem::take(&mut current));
            current_width = word_width;
            current = w;
        }
    }
    lines.push(current);
    lines
}

/// Draws one glyph, faking bold (a second copy offset in x) and italic (a
/// per-scanline horizontal shear around the baseline) since neither is
/// available as a real font variant for the two bundled `TextFont`s — same
/// technique `canvas.rs`'s draw arm uses for bold. `clip`, if set
/// (`TextResize::Fixed`), drops any pixel outside it.
#[allow(clippy::too_many_arguments)]
fn draw_text_glyph(
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
) {
    const ITALIC_SHEAR: f32 = 0.22;
    let x_offsets: &[f32] = if bold { &[0.0, 0.6] } else { &[0.0] };
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
            if let Some(clip) = clip {
                if (px as f32) < clip.min.x
                    || (px as f32) >= clip.max.x
                    || (raw_y as f32) < clip.min.y
                    || (raw_y as f32) >= clip.max.y
                {
                    return;
                }
            }
            blend_glyph_pixel(pixmap, px, raw_y, coverage, color);
        });
    }
}

/// Fills an underline/strikethrough bar, intersected with `clip` (set for
/// `TextResize::Fixed`) so decorations don't spill past a clipped box.
fn draw_decoration_rect(pixmap: &mut Pixmap, rect: egui::Rect, clip: Option<egui::Rect>, color: Color) {
    let rect = match clip {
        Some(clip) => rect.intersect(clip),
        None => rect,
    };
    if rect.width() > 0.0 && rect.height() > 0.0 {
        fill_rect(pixmap, rect, color);
    }
}

/// Alpha-blends one glyph pixel (`coverage` in `0.0..=1.0`, from `ab_glyph`'s
/// rasterizer) over the existing pixmap contents using standard Porter-Duff
/// "over" compositing, done directly in premultiplied space since that's
/// what `Pixmap`'s backing store already holds.
fn blend_glyph_pixel(pixmap: &mut Pixmap, x: i32, y: i32, coverage: f32, color: egui::Color32) {
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

fn fill_and_stroke(
    pixmap: &mut Pixmap,
    path: &tiny_skia::Path,
    layer: &Layer,
    fill_rule: FillRule,
    bounds: egui::Rect,
    opacity: f32,
    transform: Transform,
) {
    if let Some(fill) = layer.style.fill.as_ref() {
        let mut noise_storage = None;
        let paint = to_sk_paint(fill, bounds, opacity * layer.style.fill_opacity, &mut noise_storage);
        pixmap.fill_path(path, &paint, fill_rule, transform, None);
    }
    if let Some(stroke) = layer.style.stroke.as_ref() {
        let mut noise_storage = None;
        let paint = to_sk_paint(&stroke.paint, bounds, opacity * layer.style.stroke_opacity, &mut noise_storage);
        let sk_stroke = SkStroke {
            width: stroke.width,
            ..Default::default()
        };
        pixmap.stroke_path(path, &paint, &sk_stroke, transform, None);
    }
}

/// `tiny_skia::Transform::from_rotate_at` centered on `bounds`'s (already
/// offset-translated) center — the one place every non-Text/Image shape arm
/// below builds its rotation transform, so `layer.frame.rotation`'s pivot
/// convention (`bounds().center()`, per `model/layer.rs`) stays consistent.
/// `Transform::identity()` at `rotation == 0.0` (the overwhelmingly common
/// case) avoids the floating-point round-trip through `from_rotate_at` for
/// unrotated shapes, though the two are mathematically equivalent.
fn rotation_transform(layer: &Layer, bounds: egui::Rect) -> Transform {
    if layer.frame.rotation == 0.0 {
        Transform::identity()
    } else {
        Transform::from_rotate_at(layer.frame.rotation, bounds.center().x, bounds.center().y)
    }
}

/// Builds a `tiny-skia` path from anchors + bezier handles, offsetting every
/// point by `offset` (the layer's absolute position within the pixmap).
/// Straight segments (no handle on either side) use `line_to`; segments with
/// a handle use a native cubic bezier rather than manual flattening — unless
/// any point has a nonzero `corner_radius`, in which case the whole path is
/// built from `canvas::flatten_path`'s already-rounding-aware sampled
/// polyline instead (via `line_to` for every sample), so canvas and export
/// share the exact same corner-rounding geometry rather than reimplementing
/// `shapes::rounded_corner_arc_points`'s inset-and-arc math a second time
/// here. Every existing (`corner_radius == 0.0` for every point, true for
/// all pre-existing documents) path keeps the original native-bezier path
/// unchanged.
fn path_to_sk_path(points: &[PathPoint], closed: bool, offset: egui::Vec2) -> Option<tiny_skia::Path> {
    if points.len() < 2 {
        return None;
    }
    if points.iter().any(|p| p.corner_radius > 0.0) {
        let flat = crate::canvas::flatten_path(points, closed);
        if flat.len() < 2 {
            return None;
        }
        let mut pb = PathBuilder::new();
        let start = flat[0] + offset;
        pb.move_to(start.x, start.y);
        for p in &flat[1..] {
            let p = *p + offset;
            pb.line_to(p.x, p.y);
        }
        if closed {
            pb.close();
        }
        return pb.finish();
    }
    let abs = |p: &PathPoint| p.anchor + offset;
    let mut pb = PathBuilder::new();
    let start = abs(&points[0]);
    pb.move_to(start.x, start.y);
    let n = points.len();
    let last_index = if closed { n } else { n - 1 };
    for i in 0..last_index {
        let a = &points[i];
        let b = &points[(i + 1) % n];
        let p0 = abs(a);
        let p3 = abs(b);
        if a.handle_out.is_none() && b.handle_in.is_none() {
            pb.line_to(p3.x, p3.y);
        } else {
            let c1 = p0 + a.handle_out.unwrap_or(egui::Vec2::ZERO);
            let c2 = p3 + b.handle_in.unwrap_or(egui::Vec2::ZERO);
            pb.cubic_to(c1.x, c1.y, c2.x, c2.y, p3.x, p3.y);
        }
    }
    if closed {
        pb.close();
    }
    pb.finish()
}

/// Builds one multi-subpath `tiny-skia` path from every polygon's exterior +
/// hole rings, offsetting every point by `offset`. Fill with `FillRule::EvenOdd`
/// so holes actually punch through (`Winding` would just fill everything).
fn compound_path_to_sk_path(polygons: &[PathPolygon], offset: egui::Vec2) -> Option<tiny_skia::Path> {
    let mut pb = PathBuilder::new();
    let mut any_ring = false;
    for poly in polygons {
        for ring in std::iter::once(&poly.exterior).chain(poly.holes.iter()) {
            if ring.len() < 3 {
                continue;
            }
            any_ring = true;
            let start = ring[0] + offset;
            pb.move_to(start.x, start.y);
            for p in &ring[1..] {
                let p = *p + offset;
                pb.line_to(p.x, p.y);
            }
            pb.close();
        }
    }
    if !any_ring {
        return None;
    }
    pb.finish()
}

fn rounded_rect_path(rect: egui::Rect, radii: CornerRadii) -> Option<tiny_skia::Path> {
    let mut pb = PathBuilder::new();
    let max_r = rect.width().min(rect.height()) / 2.0;
    let [tl, tr, br, bl] = radii.as_array().map(|r| r.max(0.0).min(max_r));
    if tl < 0.5 && tr < 0.5 && br < 0.5 && bl < 0.5 {
        pb.push_rect(to_sk_rect(rect)?);
    } else {
        pb.move_to(rect.min.x + tl, rect.min.y);
        pb.line_to(rect.max.x - tr, rect.min.y);
        if tr >= 0.5 {
            pb.quad_to(rect.max.x, rect.min.y, rect.max.x, rect.min.y + tr);
        }
        pb.line_to(rect.max.x, rect.max.y - br);
        if br >= 0.5 {
            pb.quad_to(rect.max.x, rect.max.y, rect.max.x - br, rect.max.y);
        }
        pb.line_to(rect.min.x + bl, rect.max.y);
        if bl >= 0.5 {
            pb.quad_to(rect.min.x, rect.max.y, rect.min.x, rect.max.y - bl);
        }
        pb.line_to(rect.min.x, rect.min.y + tl);
        if tl >= 0.5 {
            pb.quad_to(rect.min.x, rect.min.y, rect.min.x + tl, rect.min.y);
        }
        pb.close();
    }
    pb.finish()
}

/// Cap size for an `Arrow` end marker, scaled off the stroke width — mirrors
/// `canvas.rs::draw_arrow_cap`'s sizing exactly, so canvas and export caps
/// match visually.
fn arrow_cap_size(stroke_width: f32) -> f32 {
    (stroke_width * 3.5).max(8.0)
}

/// The two "wing" points of a `Triangle`/`Line` cap at `tip`, pointing
/// `outward` from the shaft — `None` if `outward` is degenerate (zero-length,
/// a start/end coincident with the other endpoint).
fn arrow_cap_wings(tip: egui::Pos2, outward: egui::Vec2, stroke_width: f32) -> Option<(egui::Pos2, egui::Pos2)> {
    if outward.length() < 1e-3 {
        return None;
    }
    let dir = outward.normalized();
    let perp = egui::Vec2::new(-dir.y, dir.x);
    let size = arrow_cap_size(stroke_width);
    let base = tip - dir * size;
    Some((base + perp * size * 0.5, base - perp * size * 0.5))
}

/// A closed `tiny-skia` path tracing `points` (already in absolute pixmap
/// space) in order — the shared builder for `Star`/`Polygon`.
fn polygon_ring_path(points: &[egui::Pos2]) -> Option<tiny_skia::Path> {
    if points.len() < 3 {
        return None;
    }
    let mut pb = PathBuilder::new();
    pb.move_to(points[0].x, points[0].y);
    for p in &points[1..] {
        pb.line_to(p.x, p.y);
    }
    pb.close();
    pb.finish()
}

fn fill_rect(pixmap: &mut Pixmap, rect: egui::Rect, color: Color) {
    let Some(sk_rect) = to_sk_rect(rect) else {
        return;
    };
    let mut paint = Paint::default();
    paint.set_color(color);
    paint.anti_alias = true;
    pixmap.fill_rect(sk_rect, &paint, Transform::identity(), None);
}

fn to_sk_rect(rect: egui::Rect) -> Option<SkRect> {
    SkRect::from_ltrb(rect.min.x, rect.min.y, rect.max.x, rect.max.y)
}

fn to_color(c: egui::Color32) -> Color {
    Color::from_rgba8(c.r(), c.g(), c.b(), c.a())
}

/// Scales `color`'s alpha channel by `opacity` (`0.0..=1.0`) before
/// converting, mirroring `canvas.rs`'s `with_opacity` so a layer's (and its
/// ancestors', multiplicatively accumulated) opacity renders the same in the
/// exported PNG as it does live.
fn with_opacity(color: egui::Color32, opacity: f32) -> egui::Color32 {
    let a = (color.a() as f32 * opacity.clamp(0.0, 1.0)).round() as u8;
    egui::Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), a)
}

fn to_color_with_opacity(c: egui::Color32, opacity: f32) -> Color {
    to_color(with_opacity(c, opacity))
}

/// Builds a `tiny-skia` `Paint` for `paint` (solid or gradient), anti-aliased.
/// `bounds` is the shape's own absolute, *unrotated* bounds within the
/// pixmap (matching whatever local space the path being filled/stroked was
/// built in, before `rotation_transform` rotates it at draw time) —
/// `Gradient::from`/`to` are normalized `0.0..=1.0` per axis within it, per
/// `model::Gradient`'s doc comment. Building the shader's control points in
/// that same unrotated space, rather than pre-rotating them here, is what
/// lets the gradient rotate along with the shape: `Pixmap::fill_path`
/// applies its `transform` argument to the shader exactly like it does to
/// the path (see `tiny-skia`'s `painter.rs::fill_path`), so passing the same
/// `rotation_transform(layer, bounds)` both places keeps them in sync.
/// `noise_storage` exists only so a `Paint::Noise` fill's generated grain
/// `Pixmap` can outlive this call: `tiny_skia::Pattern`'s shader *borrows*
/// its source pixmap (unlike `Solid`/`Gradient`, which own their color data
/// outright), so the caller must keep that pixmap alive for exactly as long
/// as the returned `Paint` — declaring `let mut noise_storage = None;`
/// immediately before the call, in the same scope as the following
/// `fill_path`/`stroke_path`, does that. Every other `Paint` variant leaves
/// it `None`.
fn to_sk_paint<'a>(paint: &crate::model::Paint, bounds: egui::Rect, opacity: f32, noise_storage: &'a mut Option<Pixmap>) -> Paint<'a> {
    let mut sk_paint = Paint::default();
    sk_paint.anti_alias = true;
    match paint {
        crate::model::Paint::Solid(c) => {
            sk_paint.set_color(to_color_with_opacity(*c, opacity));
        }
        crate::model::Paint::Gradient(g) => {
            let stops: Vec<tiny_skia::GradientStop> = g
                .stops
                .iter()
                .map(|s| tiny_skia::GradientStop::new(s.offset.clamp(0.0, 1.0), to_color_with_opacity(s.color, opacity)))
                .collect();
            let w = bounds.width().max(1e-3);
            let h = bounds.height().max(1e-3);
            let to_abs =
                |p: egui::Pos2| tiny_skia::Point::from_xy(bounds.min.x + p.x * w, bounds.min.y + p.y * h);
            let shader = match g.kind {
                crate::model::GradientKind::Linear => tiny_skia::LinearGradient::new(
                    to_abs(g.from),
                    to_abs(g.to),
                    stops,
                    tiny_skia::SpreadMode::Pad,
                    Transform::identity(),
                ),
                crate::model::GradientKind::Radial => {
                    let center = to_abs(g.from);
                    let edge = to_abs(g.to);
                    let radius = ((edge.x - center.x).powi(2) + (edge.y - center.y).powi(2)).sqrt().max(0.01);
                    tiny_skia::RadialGradient::new(
                        center,
                        0.0,
                        center,
                        radius,
                        stops,
                        tiny_skia::SpreadMode::Pad,
                        Transform::identity(),
                    )
                }
            };
            if let Some(shader) = shader {
                sk_paint.shader = shader;
            }
        }
        crate::model::Paint::Noise(n) => {
            let width = bounds.width().round().max(1.0) as u32;
            let height = bounds.height().round().max(1.0) as u32;
            if let Some(mut pixmap) = Pixmap::new(width, height) {
                for j in 0..height {
                    for i in 0..width {
                        // Zero-based, one pixel per document unit — export
                        // renders unscaled (1:1), so this is directly the
                        // same local-bounds coordinate space
                        // `canvas.rs::NoiseTextureCache` samples, just at
                        // full resolution instead of a quantized texture.
                        let local_point = egui::Pos2::new(i as f32 + 0.5, j as f32 + 0.5);
                        let color = crate::noise_fill::sample(n, local_point, opacity);
                        if let Some(px) = premultiplied(color) {
                            pixmap.pixels_mut()[(j * width + i) as usize] = px;
                        }
                    }
                }
                *noise_storage = Some(pixmap);
                let pixmap_ref = noise_storage.as_ref().expect("just assigned").as_ref();
                // Translates the pattern pixmap's own (0,0)-(width,height)
                // origin onto `bounds`' unrotated absolute position — same
                // role `to_abs` plays for the gradient shader above, and
                // subject to the same `rotation_transform` composition this
                // function's doc comment describes.
                let transform = Transform::from_translate(bounds.min.x, bounds.min.y);
                sk_paint.shader = tiny_skia::Pattern::new(pixmap_ref, tiny_skia::SpreadMode::Pad, tiny_skia::FilterQuality::Nearest, 1.0, transform);
            }
        }
        crate::model::Paint::Halftone(h) => {
            // Same shape as the `Paint::Noise` arm above — generated
            // directly at full `bounds` resolution (not tiled from a
            // smaller source), so `SpreadMode::Pad` like Noise, not Repeat.
            let width = bounds.width().round().max(1.0) as u32;
            let height = bounds.height().round().max(1.0) as u32;
            if let Some(mut pixmap) = Pixmap::new(width, height) {
                for j in 0..height {
                    for i in 0..width {
                        let local_point = egui::Pos2::new(i as f32 + 0.5, j as f32 + 0.5);
                        let color = crate::halftone_fill::sample(h, local_point, opacity);
                        if let Some(px) = premultiplied(color) {
                            pixmap.pixels_mut()[(j * width + i) as usize] = px;
                        }
                    }
                }
                *noise_storage = Some(pixmap);
                let pixmap_ref = noise_storage.as_ref().expect("just assigned").as_ref();
                let transform = Transform::from_translate(bounds.min.x, bounds.min.y);
                sk_paint.shader = tiny_skia::Pattern::new(pixmap_ref, tiny_skia::SpreadMode::Pad, tiny_skia::FilterQuality::Nearest, 1.0, transform);
            }
        }
        crate::model::Paint::Pattern(p) => {
            // Unlike `Noise`/`Halftone`, this pixmap holds the actual
            // decoded tile image at its native resolution — the exact
            // conversion `draw_image` (this file's `Image`-layer renderer)
            // already uses for the same purpose.
            if let Some(decoded) = crate::image_ops::decode(&p.encoded) {
                let (width, height) = (decoded.width(), decoded.height());
                if width > 0 && height > 0 {
                    if let Some(size) = tiny_skia::IntSize::from_wh(width, height) {
                        let premultiplied = crate::image_ops::to_premultiplied_rgba(&decoded);
                        if let Some(pixmap) = Pixmap::from_vec(premultiplied, size) {
                            *noise_storage = Some(pixmap);
                            let pixmap_ref = noise_storage.as_ref().expect("just assigned").as_ref();
                            let tile_width = p.tile_width.max(0.01);
                            let tile_height = tile_width * (height as f32 / width as f32);
                            // Scales the pixmap's native (width, height)
                            // pixels down to one (tile_width, tile_height)
                            // tile, then translates to `bounds`' unrotated
                            // absolute position — `SpreadMode::Repeat` is
                            // what makes it tile across the rest of the fill.
                            let transform = Transform::from_translate(bounds.min.x, bounds.min.y)
                                .pre_scale(tile_width / width as f32, tile_height / height as f32);
                            // Unlike Noise/Halftone, opacity isn't baked
                            // per-pixel here — the decoded tile is used
                            // as-is, so it's applied via `Pattern`'s own
                            // `opacity` argument instead.
                            sk_paint.shader = tiny_skia::Pattern::new(
                                pixmap_ref,
                                tiny_skia::SpreadMode::Repeat,
                                tiny_skia::FilterQuality::Bilinear,
                                opacity.clamp(0.0, 1.0),
                                transform,
                            );
                        }
                    }
                }
            }
        }
    }
    sk_paint
}

/// Premultiplies `color`'s RGB by its own alpha — the representation
/// `tiny_skia::Pixmap`'s raw pixel storage requires (unlike `tiny_skia::Paint::set_color`,
/// which accepts straight alpha and premultiplies internally at fill time).
fn premultiplied(color: egui::Color32) -> Option<PremultipliedColorU8> {
    let a = color.a() as u16;
    let mul = |v: u8| ((v as u16 * a) / 255) as u8;
    PremultipliedColorU8::from_rgba(mul(color.r()), mul(color.g()), mul(color.b()), color.a())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::boolean_ops::apply_boolean_op;
    use crate::model::{ArrowCap, BoolOp, Frame, Page, PointType};
    use egui::{Color32, Pos2, Vec2};

    #[test]
    fn renders_image_layer_scaled_to_its_frame() {
        let pixels = image::RgbaImage::from_pixel(2, 2, image::Rgba([0, 200, 0, 255]));
        let encoded = crate::image_ops::encode_png(&pixels);
        let layer = Layer::new_image(
            "Photo",
            Frame::from_two_points(Pos2::new(0.0, 0.0), Pos2::new(40.0, 20.0)),
            encoded,
            2,
            2,
        );

        let pixmap = render_layer(&layer).expect("should render");
        assert_eq!((pixmap.width(), pixmap.height()), (40, 20));
        let center = pixmap.pixel(20, 10).expect("center pixel");
        assert_eq!((center.red(), center.green(), center.blue()), (0, 200, 0));
        assert_eq!(center.alpha(), 255);
    }

    /// End-to-end check of `render_layer`'s shadow padding + `draw_layer_with_shadows`
    /// wiring (as opposed to `shadow.rs`'s own unit tests, which exercise the
    /// blur/composite math in isolation): a standalone-exported rectangle
    /// with a drop shadow should come out padded wider than its own frame,
    /// with visible shadow color just past the shape's own edge.
    #[test]
    fn exported_layer_with_drop_shadow_is_padded_and_shows_shadow_past_its_edge() {
        let mut layer = Layer::new(
            "Rect",
            Frame::from_two_points(Pos2::new(0.0, 0.0), Pos2::new(40.0, 40.0)),
            LayerKind::Rectangle { corner_radius: CornerRadii::ZERO },
        );
        layer.style.fill = Some(crate::model::Paint::Solid(Color32::from_rgb(255, 0, 0)));
        layer.style.stroke = None;
        layer.style.shadows = vec![crate::model::Shadow {
            color: Color32::BLACK,
            offset: Vec2::ZERO,
            blur: 10.0,
            spread: 0.0,
        }];

        let pixmap = render_layer(&layer).expect("should render");
        assert!(pixmap.width() > 40 && pixmap.height() > 40, "padded export should be larger than the shape's own 40x40 frame");

        let pad = ((pixmap.width() - 40) / 2) as i32;
        // A couple pixels above the shape's top edge (still inside the
        // padded canvas) should show blurred shadow, not full transparency.
        let x = pixmap.width() as i32 / 2;
        let y = (pad - 3).max(0);
        let above_edge = pixmap.pixel(x as u32, y as u32).expect("in-bounds pixel");
        assert!(above_edge.alpha() > 0, "blurred shadow should be visible just outside the shape's own edge");

        // The shape's own center is still the plain red fill, unaffected.
        let center = pixmap.pixel((pad + 20) as u32, (pad + 20) as u32).expect("center pixel");
        assert_eq!((center.red(), center.green(), center.blue()), (255, 0, 0));
    }

    #[test]
    fn renders_filled_rect_at_correct_size_and_color() {
        let mut layer = Layer::new(
            "Rect",
            Frame::from_two_points(Pos2::new(10.0, 10.0), Pos2::new(50.0, 30.0)),
            LayerKind::Rectangle { corner_radius: CornerRadii::ZERO },
        );
        layer.style.fill = Some(crate::model::Paint::Solid(Color32::from_rgb(255, 0, 0)));
        layer.style.stroke = None;

        let pixmap = render_layer(&layer).expect("should render");
        assert_eq!(pixmap.width(), 40);
        assert_eq!(pixmap.height(), 20);

        let center = pixmap.pixel(20, 10).expect("center pixel");
        assert_eq!((center.red(), center.green(), center.blue(), center.alpha()), (255, 0, 0, 255));

        let corner = pixmap.pixel(0, 0).expect("corner pixel");
        assert_eq!(corner.alpha(), 255);
    }

    #[test]
    fn fill_opacity_and_stroke_opacity_scale_alpha_independently_of_each_other() {
        let mut layer = Layer::new(
            "Rect",
            Frame::from_two_points(Pos2::new(0.0, 0.0), Pos2::new(40.0, 40.0)),
            LayerKind::Rectangle { corner_radius: CornerRadii::ZERO },
        );
        layer.style.fill = Some(crate::model::Paint::Solid(Color32::from_rgb(255, 0, 0)));
        layer.style.stroke = Some(crate::model::Stroke { paint: crate::model::Paint::Solid(Color32::from_rgb(0, 0, 255)), width: 4.0 });
        layer.style.fill_opacity = 0.5;
        layer.style.stroke_opacity = 1.0;

        let pixmap = render_layer(&layer).expect("should render");
        let fill_pixel = pixmap.pixel(20, 20).expect("center pixel, inside the fill only");
        // Premultiplied alpha: a straight-alpha channel value of 127/255
        // shows up as roughly half the full-alpha red component too.
        assert!(
            (fill_pixel.alpha() as i32 - 127).abs() <= 2,
            "expected ~50% alpha from fill_opacity, got {}",
            fill_pixel.alpha()
        );

        let stroke_pixel = pixmap.pixel(1, 20).expect("left edge, inside the stroke");
        assert_eq!(stroke_pixel.alpha(), 255, "stroke_opacity=1.0 should stay fully opaque");
    }

    #[test]
    fn renders_star_filled_at_center_but_not_at_the_bounding_boxs_corner() {
        let mut layer = Layer::new(
            "Star",
            Frame::from_two_points(Pos2::new(0.0, 0.0), Pos2::new(40.0, 40.0)),
            LayerKind::Star { points: 5, inner_ratio: 0.5 },
        );
        layer.style.fill = Some(crate::model::Paint::Solid(Color32::from_rgb(255, 200, 0)));
        layer.style.stroke = None;

        let pixmap = render_layer(&layer).expect("should render");
        let center = pixmap.pixel(20, 20).expect("center pixel");
        assert_eq!((center.red(), center.green(), center.blue(), center.alpha()), (255, 200, 0, 255));

        // A 5-point star inscribed in its bounding ellipse never reaches
        // the bounding box's own corner.
        let corner = pixmap.pixel(1, 1).expect("corner pixel");
        assert_eq!(corner.alpha(), 0);
    }

    #[test]
    fn renders_regular_polygon_filled_at_center() {
        let mut layer = Layer::new(
            "Hexagon",
            Frame::from_two_points(Pos2::new(0.0, 0.0), Pos2::new(40.0, 40.0)),
            LayerKind::Polygon { sides: 6 },
        );
        layer.style.fill = Some(crate::model::Paint::Solid(Color32::from_rgb(0, 100, 255)));
        layer.style.stroke = None;

        let pixmap = render_layer(&layer).expect("should render");
        let center = pixmap.pixel(20, 20).expect("center pixel");
        assert_eq!((center.red(), center.green(), center.blue(), center.alpha()), (0, 100, 255, 255));

        let corner = pixmap.pixel(1, 1).expect("corner pixel");
        assert_eq!(corner.alpha(), 0);
    }

    // `Frame::from_two_points` for a purely horizontal/vertical Line/Arrow
    // has zero size along the perpendicular axis (`end.y == start.y` forces
    // `size.y == 0`), so `render_layer`'s auto-sizing-to-bounds would give a
    // 1px-tall pixmap that clips the stroke's own perpendicular extent —
    // call `draw_layer` directly onto a fixed-size pixmap instead, sidestepping
    // that (pre-existing, not Arrow-specific) auto-sizing edge case.
    #[test]
    fn renders_arrow_with_no_caps_matches_plain_line() {
        let mut layer = Layer::new(
            "Arrow",
            Frame::from_two_points(Pos2::new(0.0, 20.0), Pos2::new(40.0, 20.0)),
            LayerKind::Arrow { start_cap: ArrowCap::None, end_cap: ArrowCap::None },
        );
        layer.style.stroke = Some(crate::model::Stroke { paint: crate::model::Paint::Solid(Color32::BLACK), width: 2.0 });
        layer.style.fill = None;

        let mut pixmap = Pixmap::new(40, 40).unwrap();
        draw_layer(&mut pixmap, &layer, egui::Vec2::ZERO, 1.0);
        let on_line = pixmap.pixel(20, 20).expect("on-line pixel");
        assert_eq!(on_line.alpha(), 255);
        let off_line = pixmap.pixel(20, 5).expect("off-line pixel");
        assert_eq!(off_line.alpha(), 0);
    }

    #[test]
    fn renders_arrow_with_triangle_end_cap_wider_than_the_shaft() {
        let mut layer = Layer::new(
            "Arrow",
            Frame::from_two_points(Pos2::new(0.0, 20.0), Pos2::new(40.0, 20.0)),
            LayerKind::Arrow { start_cap: ArrowCap::None, end_cap: ArrowCap::Triangle },
        );
        layer.style.stroke = Some(crate::model::Stroke { paint: crate::model::Paint::Solid(Color32::BLACK), width: 6.0 });
        layer.style.fill = None;

        let mut pixmap = Pixmap::new(40, 40).unwrap();
        draw_layer(&mut pixmap, &layer, egui::Vec2::ZERO, 1.0);
        // Well inside the triangle wing (cap size = max(6*3.5, 8) = 21, base
        // at x=19, half-width 7.5 at x=25) but clearly outside the shaft's
        // own stroke half-width (3, so shaft covers y 17..23 only).
        let wing = pixmap.pixel(25, 15).expect("wing pixel");
        assert!(wing.alpha() > 0, "triangle cap should extend beyond the shaft's stroke width");
        // Far from both the shaft and the cap's base range.
        let outside = pixmap.pixel(5, 5).expect("outside pixel");
        assert_eq!(outside.alpha(), 0);
    }

    #[test]
    fn zero_rotation_rectangle_renders_identically_to_unrotated_path() {
        // Regression lock for the `Rectangle` arm's switch from
        // `painter.rect`-equivalent tiny-skia's native rounded-rect path to
        // the shared `fill_and_stroke` + `rotation_transform` path: at
        // `rotation == 0.0`, `rotation_transform` returns `Transform::identity()`
        // so this must produce byte-identical output to the pre-rotation code.
        let mut layer = Layer::new(
            "Rect",
            Frame::from_two_points(Pos2::new(0.0, 0.0), Pos2::new(40.0, 40.0)),
            LayerKind::Rectangle { corner_radius: CornerRadii::uniform(8.0) },
        );
        layer.style.fill = Some(crate::model::Paint::Solid(Color32::from_rgb(10, 20, 30)));
        layer.style.stroke = None;
        let pixmap = render_layer(&layer).expect("should render");
        let center = pixmap.pixel(20, 20).expect("center pixel");
        assert_eq!((center.red(), center.green(), center.blue(), center.alpha()), (10, 20, 30, 255));
        // Corner pixel should stay unfilled (excluded by the rounded corner).
        let corner = pixmap.pixel(0, 0).expect("corner pixel");
        assert_eq!(corner.alpha(), 0);
    }

    #[test]
    fn independent_corner_radii_round_only_the_corners_given_a_nonzero_radius() {
        let mut layer = Layer::new(
            "Rect",
            Frame::from_two_points(Pos2::new(0.0, 0.0), Pos2::new(40.0, 40.0)),
            LayerKind::Rectangle {
                corner_radius: CornerRadii { top_left: 12.0, top_right: 0.0, bottom_right: 0.0, bottom_left: 0.0 },
            },
        );
        layer.style.fill = Some(crate::model::Paint::Solid(Color32::from_rgb(10, 20, 30)));
        layer.style.stroke = None;
        let pixmap = render_layer(&layer).expect("should render");
        // Rounded corner (top-left) stays unfilled.
        let rounded = pixmap.pixel(0, 0).expect("top-left pixel");
        assert_eq!(rounded.alpha(), 0);
        // The other three (sharp) corners are filled all the way to the edge.
        for (x, y) in [(39, 0), (0, 39), (39, 39)] {
            let sharp = pixmap.pixel(x, y).expect("sharp corner pixel");
            assert_eq!(sharp.alpha(), 255, "expected corner ({x}, {y}) to be filled");
        }
    }

    #[test]
    fn rotated_square_fills_center_but_not_the_new_bounding_boxs_corner() {
        let mut layer = Layer::new(
            "Rect",
            Frame { pos: Pos2::new(0.0, 0.0), size: Vec2::new(20.0, 20.0), rotation: 45.0 },
            LayerKind::Rectangle { corner_radius: CornerRadii::ZERO },
        );
        layer.style.fill = Some(crate::model::Paint::Solid(Color32::from_rgb(200, 0, 0)));
        layer.style.stroke = None;

        let pixmap = render_layer(&layer).expect("should render");
        // A 20x20 square rotated 45 degrees has a rotated-bounds AABB of
        // side 20*sqrt(2) ~= 28.28, rounded to 28.
        assert_eq!((pixmap.width(), pixmap.height()), (28, 28));

        let center = pixmap.pixel(14, 14).expect("center pixel");
        assert_eq!((center.red(), center.green(), center.blue(), center.alpha()), (200, 0, 0, 255));

        // The rotated square's diagonal edge doesn't reach the new AABB's
        // own corner — only its unrotated `bounds()` would have.
        let corner = pixmap.pixel(0, 0).expect("corner pixel");
        assert_eq!(corner.alpha(), 0);
    }

    #[test]
    fn rotated_image_export_pixmap_is_sized_to_rotated_bounds_not_clipped() {
        let encoded = crate::image_ops::encode_png(&image::RgbaImage::from_pixel(2, 2, image::Rgba([0, 200, 0, 255])));
        let layer = Layer::new_image(
            "Photo",
            Frame { pos: Pos2::new(0.0, 0.0), size: Vec2::new(40.0, 20.0), rotation: 90.0 },
            encoded,
            2,
            2,
        );
        let pixmap = render_layer(&layer).expect("should render");
        // A 40x20 layer rotated exactly 90 degrees has a 20x40 rotated AABB.
        assert_eq!((pixmap.width(), pixmap.height()), (20, 40));
        let center = pixmap.pixel(10, 20).expect("center pixel");
        assert_eq!((center.red(), center.green(), center.blue(), center.alpha()), (0, 200, 0, 255));
    }

    #[test]
    fn artboard_background_fills_whole_canvas_and_children_are_offset() {
        let mut child = Layer::new(
            "Child",
            Frame::from_two_points(Pos2::new(5.0, 5.0), Pos2::new(15.0, 15.0)),
            LayerKind::Rectangle { corner_radius: CornerRadii::ZERO },
        );
        child.style.fill = Some(crate::model::Paint::Solid(Color32::from_rgb(0, 255, 0)));
        child.style.stroke = None;

        let mut artboard = Layer::new_artboard(
            "Board",
            Frame::from_two_points(Pos2::new(0.0, 0.0), Pos2::new(30.0, 30.0)),
        );
        if let LayerKind::Artboard { children, background } = &mut artboard.kind {
            *background = Color32::WHITE;
            children.push(child);
        }

        let pixmap = render_layer(&artboard).expect("should render");
        assert_eq!((pixmap.width(), pixmap.height()), (30, 30));

        let bg_pixel = pixmap.pixel(1, 1).unwrap();
        assert_eq!((bg_pixel.red(), bg_pixel.green(), bg_pixel.blue()), (255, 255, 255));

        let child_pixel = pixmap.pixel(10, 10).unwrap();
        assert_eq!((child_pixel.red(), child_pixel.green(), child_pixel.blue()), (0, 255, 0));
    }

    #[test]
    fn renders_closed_path_as_filled_polygon() {
        // A 40x40 right triangle with straight (non-bezier) corners.
        let points = vec![
            PathPoint { anchor: Pos2::new(0.0, 0.0), handle_in: None, handle_out: None, point_type: PointType::Disconnected, corner_radius: 0.0 },
            PathPoint { anchor: Pos2::new(40.0, 0.0), handle_in: None, handle_out: None, point_type: PointType::Disconnected, corner_radius: 0.0 },
            PathPoint { anchor: Pos2::new(0.0, 40.0), handle_in: None, handle_out: None, point_type: PointType::Disconnected, corner_radius: 0.0 },
        ];
        let mut layer = Layer::new(
            "Path",
            Frame { pos: Pos2::new(0.0, 0.0), size: egui::Vec2::new(40.0, 40.0), rotation: 0.0 },
            LayerKind::Path { points, closed: true },
        );
        layer.style.fill = Some(crate::model::Paint::Solid(Color32::from_rgb(0, 0, 255)));
        layer.style.stroke = None;

        let pixmap = render_layer(&layer).expect("should render");

        // Inside the triangle (near the right-angle corner).
        let inside = pixmap.pixel(5, 5).expect("inside pixel");
        assert_eq!((inside.red(), inside.green(), inside.blue(), inside.alpha()), (0, 0, 255, 255));

        // Outside the triangle's hypotenuse, still inside the bounding box.
        let outside = pixmap.pixel(35, 35).expect("outside pixel");
        assert_eq!(outside.alpha(), 0);
    }

    #[test]
    fn exported_png_rounds_a_path_corner() {
        let points = vec![
            PathPoint { anchor: Pos2::new(0.0, 0.0), handle_in: None, handle_out: None, point_type: PointType::Disconnected, corner_radius: 0.0 },
            PathPoint { anchor: Pos2::new(40.0, 0.0), handle_in: None, handle_out: None, point_type: PointType::Disconnected, corner_radius: 15.0 },
            PathPoint { anchor: Pos2::new(40.0, 40.0), handle_in: None, handle_out: None, point_type: PointType::Disconnected, corner_radius: 0.0 },
            PathPoint { anchor: Pos2::new(0.0, 40.0), handle_in: None, handle_out: None, point_type: PointType::Disconnected, corner_radius: 0.0 },
        ];
        let mut layer = Layer::new(
            "Path",
            Frame { pos: Pos2::new(0.0, 0.0), size: egui::Vec2::new(40.0, 40.0), rotation: 0.0 },
            LayerKind::Path { points, closed: true },
        );
        layer.style.fill = Some(crate::model::Paint::Solid(Color32::from_rgb(0, 0, 255)));
        layer.style.stroke = None;

        let pixmap = render_layer(&layer).expect("should render");
        // Center is still filled.
        let inside = pixmap.pixel(20, 20).expect("inside pixel");
        assert_eq!(inside.alpha(), 255);
        // The rounded corner itself (top-right, where a plain square would
        // be filled right up to the edge) is now cut away.
        let corner = pixmap.pixel(39, 0).expect("corner pixel");
        assert_eq!(corner.alpha(), 0, "rounded corner should not be filled");
    }

    #[test]
    fn open_path_with_two_points_is_not_dropped() {
        let points = vec![
            PathPoint { anchor: Pos2::new(0.0, 2.0), handle_in: None, handle_out: None, point_type: PointType::Disconnected, corner_radius: 0.0 },
            PathPoint { anchor: Pos2::new(20.0, 2.0), handle_in: None, handle_out: None, point_type: PointType::Disconnected, corner_radius: 0.0 },
        ];
        let mut layer = Layer::new(
            "Path",
            Frame { pos: Pos2::new(0.0, 0.0), size: egui::Vec2::new(20.0, 4.0), rotation: 0.0 },
            LayerKind::Path { points, closed: false },
        );
        layer.style.fill = None;
        layer.style.stroke = Some(crate::model::Stroke { paint: crate::model::Paint::Solid(Color32::from_rgb(255, 0, 0)), width: 2.0 });

        let pixmap = render_layer(&layer).expect("should render");
        let on_line = pixmap.pixel(10, 2).expect("pixel on the stroked line");
        assert_eq!(on_line.alpha(), 255);
    }

    #[test]
    fn boolean_union_of_two_overlapping_stars_produces_one_merged_layer() {
        let mut page = Page::new("Test");

        let mut a = Layer::new(
            "A",
            Frame { pos: Pos2::new(0.0, 0.0), size: egui::Vec2::new(40.0, 40.0), rotation: 0.0 },
            LayerKind::Star { points: 5, inner_ratio: 0.5 },
        );
        a.style.fill = Some(crate::model::Paint::Solid(Color32::from_rgb(0, 200, 0)));
        a.style.stroke = None;
        let a_id = a.id;

        let mut b = Layer::new(
            "B",
            Frame { pos: Pos2::new(10.0, 10.0), size: egui::Vec2::new(40.0, 40.0), rotation: 0.0 },
            LayerKind::Star { points: 5, inner_ratio: 0.5 },
        );
        b.style.fill = Some(crate::model::Paint::Solid(Color32::from_rgb(0, 200, 0)));
        b.style.stroke = None;
        let b_id = b.id;

        page.layers.push(a);
        page.layers.push(b);

        let result_id = apply_boolean_op(&mut page, &[a_id, b_id], BoolOp::Union)
            .expect("union of overlapping stars should produce a layer");
        assert_eq!(page.layers.len(), 1, "originals should be replaced by the merged layer");
        let merged = page.find(result_id).expect("merged layer should be findable");
        assert!(matches!(merged.kind, LayerKind::CompoundPath { .. }));
    }

    #[test]
    fn boolean_union_of_two_overlapping_rects_fills_both_and_replaces_originals() {
        let mut page = Page::new("Test");

        let mut a = Layer::new(
            "A",
            Frame { pos: Pos2::new(0.0, 0.0), size: egui::Vec2::new(40.0, 40.0), rotation: 0.0 },
            LayerKind::Rectangle { corner_radius: CornerRadii::ZERO },
        );
        a.style.fill = Some(crate::model::Paint::Solid(Color32::from_rgb(0, 200, 0)));
        a.style.stroke = None;
        let a_id = a.id;

        let mut b = Layer::new(
            "B",
            Frame { pos: Pos2::new(20.0, 20.0), size: egui::Vec2::new(40.0, 40.0), rotation: 0.0 },
            LayerKind::Rectangle { corner_radius: CornerRadii::ZERO },
        );
        b.style.fill = Some(crate::model::Paint::Solid(Color32::from_rgb(0, 200, 0)));
        b.style.stroke = None;
        let b_id = b.id;

        page.layers.push(a);
        page.layers.push(b);

        let result_id = apply_boolean_op(&mut page, &[a_id, b_id], BoolOp::Union)
            .expect("union of overlapping rects should produce a layer");

        assert_eq!(page.layers.len(), 1, "originals should be replaced by the merged layer");
        let merged = page.find(result_id).expect("merged layer should be findable");
        assert!(matches!(merged.kind, LayerKind::CompoundPath { .. }));

        let pixmap = render_layer(merged).expect("should render");

        let in_a_only = pixmap.pixel(5, 5).expect("in-A pixel");
        assert_eq!(in_a_only.alpha(), 255, "point only inside A should be filled");

        let in_b_only = pixmap.pixel(55, 55).expect("in-B pixel");
        assert_eq!(in_b_only.alpha(), 255, "point only inside B should be filled");

        let overlap = pixmap.pixel(25, 25).expect("overlap pixel");
        assert_eq!(overlap.alpha(), 255, "overlap point should be filled");

        let outside = pixmap.pixel(2, 58).expect("outside pixel");
        assert_eq!(outside.alpha(), 0, "point outside both rects should be transparent");
    }

    #[test]
    fn boolean_union_respects_a_rotated_operands_actual_footprint() {
        let mut page = Page::new("Test");

        // A 40x10 rect rotated 90 degrees about its own center (20,5)
        // occupies a 10-wide, 40-tall vertical footprint (x:15..25,
        // y:-15..25) — nothing like its unrotated (x:0..40, y:0..10) frame.
        let mut a = Layer::new(
            "A",
            Frame { pos: Pos2::new(0.0, 0.0), size: egui::Vec2::new(40.0, 10.0), rotation: 90.0 },
            LayerKind::Rectangle { corner_radius: CornerRadii::ZERO },
        );
        a.style.fill = Some(crate::model::Paint::Solid(Color32::from_rgb(0, 200, 0)));
        a.style.stroke = None;
        let a_id = a.id;

        // Fully inside A's *rotated* footprint, but nowhere near A's
        // unrotated frame (entirely negative y).
        let mut b = Layer::new(
            "B",
            Frame { pos: Pos2::new(16.0, -14.0), size: egui::Vec2::new(8.0, 4.0), rotation: 0.0 },
            LayerKind::Rectangle { corner_radius: CornerRadii::ZERO },
        );
        b.style.fill = Some(crate::model::Paint::Solid(Color32::from_rgb(0, 200, 0)));
        b.style.stroke = None;
        let b_id = b.id;

        page.layers.push(a);
        page.layers.push(b);

        let result_id = apply_boolean_op(&mut page, &[a_id, b_id], BoolOp::Union)
            .expect("union should produce a layer");
        let merged = page.find(result_id).expect("merged layer should be findable");

        // If rotation were ignored, `flatten_layer` would flatten A to its
        // unrotated (40x10) frame, disjoint from B, and the merged bounding
        // box would span the full x:0..40 range. Respecting rotation instead
        // collapses to (B being a strict subset of A's rotated footprint)
        // just A's own rotated bounds: ~10 wide, ~40 tall.
        let bounds = merged.frame.bounds();
        assert!((bounds.width() - 10.0).abs() < 1.0, "width={}", bounds.width());
        assert!((bounds.height() - 40.0).abs() < 1.0, "height={}", bounds.height());
    }

    #[test]
    fn boolean_subtract_punches_a_hole() {
        let mut page = Page::new("Test");

        // Backmost (subtracted-from) shape: a 60x60 square.
        let mut base = Layer::new(
            "Base",
            Frame { pos: Pos2::new(0.0, 0.0), size: egui::Vec2::new(60.0, 60.0), rotation: 0.0 },
            LayerKind::Rectangle { corner_radius: CornerRadii::ZERO },
        );
        base.style.fill = Some(crate::model::Paint::Solid(Color32::from_rgb(200, 0, 0)));
        base.style.stroke = None;
        let base_id = base.id;

        // Frontmost shape, fully inside the base: a 20x20 square centered in it.
        let mut hole = Layer::new(
            "Hole",
            Frame { pos: Pos2::new(20.0, 20.0), size: egui::Vec2::new(20.0, 20.0), rotation: 0.0 },
            LayerKind::Rectangle { corner_radius: CornerRadii::ZERO },
        );
        hole.style.fill = Some(crate::model::Paint::Solid(Color32::from_rgb(200, 0, 0)));
        hole.style.stroke = None;
        let hole_id = hole.id;

        page.layers.push(base);
        page.layers.push(hole);

        let result_id = apply_boolean_op(&mut page, &[base_id, hole_id], BoolOp::Subtract)
            .expect("subtracting an inner square should produce a layer");
        let merged = page.find(result_id).expect("merged layer should be findable");

        let pixmap = render_layer(merged).expect("should render");

        let in_hole = pixmap.pixel(30, 30).expect("hole pixel");
        assert_eq!(in_hole.alpha(), 0, "the subtracted region should be transparent (a real hole)");

        let outside_hole = pixmap.pixel(5, 5).expect("remaining ring pixel");
        assert_eq!(outside_hole.alpha(), 255, "the rest of the base square should still be filled");
    }

    #[test]
    fn boolean_group_renders_live_from_still_editable_children() {
        let mut page = Page::new("Test");

        let mut a = Layer::new(
            "A",
            Frame { pos: Pos2::new(0.0, 0.0), size: egui::Vec2::new(40.0, 40.0), rotation: 0.0 },
            LayerKind::Rectangle { corner_radius: CornerRadii::ZERO },
        );
        a.style.fill = Some(crate::model::Paint::Solid(Color32::from_rgb(0, 200, 0)));
        a.style.stroke = None;
        let a_id = a.id;

        let mut b = Layer::new(
            "B",
            Frame { pos: Pos2::new(20.0, 20.0), size: egui::Vec2::new(40.0, 40.0), rotation: 0.0 },
            LayerKind::Rectangle { corner_radius: CornerRadii::ZERO },
        );
        b.style.fill = Some(crate::model::Paint::Solid(Color32::from_rgb(0, 0, 200)));
        b.style.stroke = None;
        let b_id = b.id;

        page.layers.push(a);
        page.layers.push(b);

        let group_id = boolean_ops::create_boolean_group(&mut page, &[a_id, b_id], BoolOp::Union)
            .expect("union of overlapping rects should produce a live group");
        let group = page.find(group_id).expect("group should be findable");
        assert!(matches!(group.kind, LayerKind::BooleanGroup { .. }));
        // Originals stay as real, separately editable children, not flattened.
        let LayerKind::BooleanGroup { children } = &group.kind else { unreachable!() };
        assert_eq!(children.len(), 2);
        assert!(matches!(children[0].kind, LayerKind::Rectangle { .. }));
        assert!(matches!(children[1].kind, LayerKind::Rectangle { .. }));

        let pixmap = render_layer(group).expect("should render");
        let in_a_only = pixmap.pixel(5, 5).expect("in-A pixel");
        assert_eq!(in_a_only.alpha(), 255, "point only inside A should be filled");
        let in_b_only = pixmap.pixel(55, 55).expect("in-B pixel");
        assert_eq!(in_b_only.alpha(), 255, "point only inside B should be filled");
        let outside = pixmap.pixel(2, 58).expect("outside pixel");
        assert_eq!(outside.alpha(), 0, "point outside both rects should be transparent");
    }

    #[test]
    fn boolean_group_excludes_hidden_children_from_the_live_result() {
        let mut page = Page::new("Test");

        let mut base = Layer::new(
            "Base",
            Frame { pos: Pos2::new(0.0, 0.0), size: egui::Vec2::new(40.0, 40.0), rotation: 0.0 },
            LayerKind::Rectangle { corner_radius: CornerRadii::ZERO },
        );
        base.style.fill = Some(crate::model::Paint::Solid(Color32::from_rgb(0, 200, 0)));
        base.style.stroke = None;
        let base_id = base.id;

        let mut extra = Layer::new(
            "Extra",
            Frame { pos: Pos2::new(60.0, 0.0), size: egui::Vec2::new(20.0, 20.0), rotation: 0.0 },
            LayerKind::Rectangle { corner_radius: CornerRadii::ZERO },
        );
        extra.style.fill = Some(crate::model::Paint::Solid(Color32::from_rgb(0, 200, 0)));
        extra.style.stroke = None;
        let extra_id = extra.id;

        page.layers.push(base);
        page.layers.push(extra);

        let group_id = boolean_ops::create_boolean_group(&mut page, &[base_id, extra_id], BoolOp::Union)
            .expect("union should produce a live group");
        let group = page.find_mut(group_id).expect("group should be findable");
        let LayerKind::BooleanGroup { children } = &mut group.kind else { unreachable!() };
        children[1].visible = false;

        let group = page.find(group_id).expect("group should be findable");
        let pixmap = render_layer(group).expect("should render");
        // The group's own frame still spans both children's original bounds
        // (unchanged by hiding one), but the hidden "Extra" rect no longer
        // contributes any fill — only "Base"'s region should render.
        let base_region = pixmap.pixel(5, 5).expect("base pixel");
        assert_eq!(base_region.alpha(), 255, "visible base child should still render");
        let hidden_region = pixmap.pixel(65, 5).expect("hidden-child pixel");
        assert_eq!(hidden_region.alpha(), 0, "hidden child should not contribute to the combined result");
    }

    #[test]
    fn boolean_group_add_of_overlapping_rects_shows_an_even_odd_hole_in_the_overlap() {
        // Documents the intentional edge case noted on `BoolOp::Add` and
        // `boolean_ops::point_in_multipolygon`: Add concatenates operands
        // with no clipping, so two overlapping pieces both cover the overlap
        // region — which an even-odd fill (required for Subtract/Difference
        // holes elsewhere) renders as transparent, not doubly-filled.
        let mut page = Page::new("Test");

        let mut a = Layer::new(
            "A",
            Frame { pos: Pos2::new(0.0, 0.0), size: egui::Vec2::new(40.0, 40.0), rotation: 0.0 },
            LayerKind::Rectangle { corner_radius: CornerRadii::ZERO },
        );
        a.style.fill = Some(crate::model::Paint::Solid(Color32::from_rgb(0, 200, 0)));
        a.style.stroke = None;
        let a_id = a.id;

        let mut b = Layer::new(
            "B",
            Frame { pos: Pos2::new(20.0, 20.0), size: egui::Vec2::new(40.0, 40.0), rotation: 0.0 },
            LayerKind::Rectangle { corner_radius: CornerRadii::ZERO },
        );
        b.style.fill = Some(crate::model::Paint::Solid(Color32::from_rgb(0, 200, 0)));
        b.style.stroke = None;
        let b_id = b.id;

        page.layers.push(a);
        page.layers.push(b);

        let group_id = boolean_ops::create_boolean_group(&mut page, &[a_id, b_id], BoolOp::Add)
            .expect("add of overlapping rects should produce a live group");
        let group = page.find(group_id).expect("group should be findable");

        let pixmap = render_layer(group).expect("should render");
        let in_a_only = pixmap.pixel(5, 5).expect("in-A-only pixel");
        assert_eq!(in_a_only.alpha(), 255, "region only covered by A should be filled");
        let overlap = pixmap.pixel(25, 25).expect("overlap pixel");
        assert_eq!(overlap.alpha(), 0, "overlap region should be an even-odd hole, not filled");
    }

    #[test]
    fn renders_text_glyphs_as_opaque_pixels_on_transparent_background() {
        let mut layer = Layer::new(
            "Text",
            Frame { pos: Pos2::new(0.0, 0.0), size: egui::Vec2::new(120.0, 40.0), rotation: 0.0 },
            LayerKind::Text {
                content: "W".to_string(),
                font_size: 32.0,
                font: TextFont::Proportional,
                align: TextAlign::Left,
                vertical_align: VerticalAlign::Top,
                resize: TextResize::Fixed,
                line_height: None,
                letter_spacing: 0.0,
                paragraph_spacing: 0.0,
                bold: false,
                italic: false,
                underline: false,
                strikethrough: false,
                transform: TextTransform::None,
                list: ListType::None,
                list_start: 1,
                style_id: None,
                runs: Vec::new(),
            },
        );
        layer.style.fill = Some(crate::model::Paint::Solid(Color32::BLACK));
        layer.style.stroke = None;

        let pixmap = render_layer(&layer).expect("should render");
        let any_opaque = (0..pixmap.width()).any(|x| {
            (0..pixmap.height()).any(|y| pixmap.pixel(x, y).map(|p| p.alpha() > 0).unwrap_or(false))
        });
        assert!(any_opaque, "expected at least one covered pixel from the rasterized glyph");
    }

    #[test]
    fn system_font_not_installed_falls_back_to_proportional_instead_of_drawing_nothing() {
        let mut layer = Layer::new(
            "Text",
            Frame { pos: Pos2::new(0.0, 0.0), size: egui::Vec2::new(120.0, 40.0), rotation: 0.0 },
            LayerKind::Text {
                content: "W".to_string(),
                font_size: 32.0,
                font: TextFont::System("Definitely Not An Installed Font Xyzzy".to_string()),
                align: TextAlign::Left,
                vertical_align: VerticalAlign::Top,
                resize: TextResize::Fixed,
                line_height: None,
                letter_spacing: 0.0,
                paragraph_spacing: 0.0,
                bold: false,
                italic: false,
                underline: false,
                strikethrough: false,
                transform: TextTransform::None,
                list: ListType::None,
                list_start: 1,
                style_id: None,
                runs: Vec::new(),
            },
        );
        layer.style.fill = Some(crate::model::Paint::Solid(Color32::BLACK));
        layer.style.stroke = None;

        let pixmap = render_layer(&layer).expect("should render");
        let any_opaque = (0..pixmap.width()).any(|x| {
            (0..pixmap.height()).any(|y| pixmap.pixel(x, y).map(|p| p.alpha() > 0).unwrap_or(false))
        });
        assert!(any_opaque, "an unresolvable system font should fall back to rendering with Proportional, not draw nothing");
    }

    #[test]
    fn right_aligned_text_hugs_the_right_edge_of_the_frame() {
        let mut layer = Layer::new(
            "Text",
            Frame { pos: Pos2::new(0.0, 0.0), size: egui::Vec2::new(200.0, 40.0), rotation: 0.0 },
            LayerKind::Text {
                content: "W".to_string(),
                font_size: 32.0,
                font: TextFont::Proportional,
                align: TextAlign::Right,
                vertical_align: VerticalAlign::Top,
                resize: TextResize::Fixed,
                line_height: None,
                letter_spacing: 0.0,
                paragraph_spacing: 0.0,
                bold: false,
                italic: false,
                underline: false,
                strikethrough: false,
                transform: TextTransform::None,
                list: ListType::None,
                list_start: 1,
                style_id: None,
                runs: Vec::new(),
            },
        );
        layer.style.fill = Some(crate::model::Paint::Solid(Color32::BLACK));
        layer.style.stroke = None;

        let pixmap = render_layer(&layer).expect("should render");
        let mut max_covered_x = None;
        for x in 0..pixmap.width() {
            for y in 0..pixmap.height() {
                if pixmap.pixel(x, y).map(|p| p.alpha() > 0).unwrap_or(false) {
                    max_covered_x = Some(x);
                }
            }
        }
        let max_covered_x = max_covered_x.expect("glyph should cover some pixel");
        assert!(
            max_covered_x as f32 > pixmap.width() as f32 * 0.7,
            "right-aligned glyph should sit near the right edge, got max covered x = {max_covered_x}"
        );
    }

    #[test]
    fn wrap_line_breaks_at_word_boundaries_within_max_width() {
        let width_per_char = 10.0;
        let measure = |s: &str| s.chars().count() as f32 * width_per_char;
        // "aaa bbb" is 70px (fits in 71); adding " ccc" would be 110px (doesn't).
        let lines = wrap_line("aaa bbb ccc", &measure, width_per_char, 71.0);
        assert_eq!(lines, vec!["aaa bbb".to_string(), "ccc".to_string()]);
    }

    #[test]
    fn wrap_line_leaves_auto_width_text_on_one_line() {
        let measure = |s: &str| s.chars().count() as f32 * 10.0;
        let lines = wrap_line("aaa bbb ccc", &measure, 10.0, f32::INFINITY);
        assert_eq!(lines, vec!["aaa bbb ccc".to_string()]);
    }

    fn text_layer(content: &str, font_size: f32, size: egui::Vec2, align: TextAlign) -> Layer {
        let mut layer = Layer::new(
            "Text",
            Frame { pos: Pos2::new(0.0, 0.0), size, rotation: 0.0 },
            LayerKind::Text {
                content: content.to_string(),
                font_size,
                font: TextFont::Proportional,
                align,
                vertical_align: VerticalAlign::Top,
                resize: TextResize::Fixed,
                line_height: None,
                letter_spacing: 0.0,
                paragraph_spacing: 0.0,
                bold: false,
                italic: false,
                underline: false,
                strikethrough: false,
                transform: TextTransform::None,
                list: ListType::None,
                list_start: 1,
                style_id: None,
                runs: Vec::new(),
            },
        );
        layer.style.fill = Some(crate::model::Paint::Solid(Color32::BLACK));
        layer.style.stroke = None;
        layer
    }

    fn opaque_pixel_count(pixmap: &Pixmap) -> usize {
        (0..pixmap.width())
            .flat_map(|x| (0..pixmap.height()).map(move |y| (x, y)))
            .filter(|&(x, y)| pixmap.pixel(x, y).map(|p| p.alpha() > 0).unwrap_or(false))
            .count()
    }

    #[test]
    fn underline_and_strikethrough_add_extra_opaque_pixels() {
        let mut without_decoration = text_layer("I", 32.0, egui::Vec2::new(120.0, 60.0), TextAlign::Left);
        let mut with_underline = without_decoration.clone();
        let mut with_strikethrough = without_decoration.clone();
        if let LayerKind::Text { underline, .. } = &mut with_underline.kind {
            *underline = true;
        }
        if let LayerKind::Text { strikethrough, .. } = &mut with_strikethrough.kind {
            *strikethrough = true;
        }
        without_decoration.style.fill = Some(crate::model::Paint::Solid(Color32::BLACK));

        let base = opaque_pixel_count(&render_layer(&without_decoration).expect("should render"));
        let underlined = opaque_pixel_count(&render_layer(&with_underline).expect("should render"));
        let struck = opaque_pixel_count(&render_layer(&with_strikethrough).expect("should render"));
        assert!(underlined > base, "underline should add extra opaque pixels below the glyph");
        assert!(struck > base, "strikethrough should add extra opaque pixels through the glyph");
    }

    #[test]
    fn fixed_width_box_wraps_long_content_onto_a_second_line() {
        let layer = text_layer("aaaaaaaa bbbbbbbb", 24.0, egui::Vec2::new(70.0, 80.0), TextAlign::Left);
        let pixmap = render_layer(&layer).expect("should render");
        let opaque_in_rows = |y0: u32, y1: u32| {
            (0..pixmap.width())
                .any(|x| (y0..y1.min(pixmap.height())).any(|y| pixmap.pixel(x, y).map(|p| p.alpha() > 0).unwrap_or(false)))
        };
        assert!(opaque_in_rows(0, 30), "expected a first line near the top");
        assert!(
            opaque_in_rows(pixmap.height() / 2, pixmap.height()),
            "70px is too narrow for both words on one line, so a wrapped second line should reach the lower half"
        );
    }

    #[test]
    fn justified_non_last_line_stretches_further_right_than_left_aligned() {
        // Narrow enough that "aaaa bbbb" (line 1, non-last) wraps before
        // "cccc" (line 2, last — never stretched, matching typographic
        // convention and `canvas.rs`'s `egui::LayoutJob` behavior), wide
        // enough that "aaaa bbbb" alone still fits on one line.
        let size = egui::Vec2::new(100.0, 150.0);
        let left = text_layer("aaaa bbbb cccc", 24.0, size, TextAlign::Left);
        let justify = text_layer("aaaa bbbb cccc", 24.0, size, TextAlign::Justify);
        let left_pixmap = render_layer(&left).expect("should render");
        let justify_pixmap = render_layer(&justify).expect("should render");

        // Both wrap identically (same content/size/font); restrict the scan
        // to the first line's row band (top quarter) so this only measures
        // the non-last, justify-stretched line.
        let max_x_in_top_quarter = |pixmap: &Pixmap| {
            let mut max_x = 0u32;
            for x in 0..pixmap.width() {
                for y in 0..(pixmap.height() / 4) {
                    if pixmap.pixel(x, y).map(|p| p.alpha() > 0).unwrap_or(false) {
                        max_x = max_x.max(x);
                    }
                }
            }
            max_x
        };
        let left_max = max_x_in_top_quarter(&left_pixmap);
        let justify_max = max_x_in_top_quarter(&justify_pixmap);
        assert!(
            justify_max > left_max,
            "justified first line (max_x={justify_max}) should stretch further right than left-aligned (max_x={left_max})"
        );
    }

    fn run_style(font_size: f32) -> RunStyle {
        RunStyle {
            font: TextFont::Proportional,
            font_size,
            color: None,
            bold: false,
            italic: false,
            underline: false,
            strikethrough: false,
        }
    }

    #[test]
    fn rich_text_second_run_color_shows_up_distinct_from_the_first() {
        // "AA" in black, "BB" in red — scan the rendered row for both a
        // black-ish and a distinctly red pixel.
        let mut layer = text_layer("AABB", 40.0, egui::Vec2::new(120.0, 60.0), TextAlign::Left);
        if let LayerKind::Text { runs, .. } = &mut layer.kind {
            *runs = vec![
                TextRun { len: 2, style: run_style(40.0) },
                TextRun { len: 2, style: RunStyle { color: Some(Color32::from_rgb(220, 20, 20)), ..run_style(40.0) } },
            ];
        }
        let pixmap = render_layer(&layer).expect("should render");
        let mut saw_black = false;
        let mut saw_red = false;
        for x in 0..pixmap.width() {
            for y in 0..pixmap.height() {
                let Some(p) = pixmap.pixel(x, y) else { continue };
                if p.alpha() == 0 {
                    continue;
                }
                if p.red() > 150 && p.green() < 80 && p.blue() < 80 {
                    saw_red = true;
                } else if p.red() < 80 && p.green() < 80 && p.blue() < 80 {
                    saw_black = true;
                }
            }
        }
        assert!(saw_black, "expected the first (default black) run to render");
        assert!(saw_red, "expected the second run's color override to render");
    }

    #[test]
    fn rich_text_run_bold_thickens_strokes_same_as_whole_layer_bold() {
        let plain = text_layer("W", 40.0, egui::Vec2::new(80.0, 60.0), TextAlign::Left);
        let mut whole_layer_bold = plain.clone();
        if let LayerKind::Text { bold, .. } = &mut whole_layer_bold.kind {
            *bold = true;
        }
        let mut run_bold = plain.clone();
        if let LayerKind::Text { runs, .. } = &mut run_bold.kind {
            *runs = vec![TextRun { len: 1, style: RunStyle { bold: true, ..run_style(40.0) } }];
        }

        let plain_count = opaque_pixel_count(&render_layer(&plain).unwrap());
        let whole_bold_count = opaque_pixel_count(&render_layer(&whole_layer_bold).unwrap());
        let run_bold_count = opaque_pixel_count(&render_layer(&run_bold).unwrap());

        assert!(whole_bold_count > plain_count, "whole-layer bold (faux double-draw) should thicken the glyph");
        assert!(run_bold_count > plain_count, "a bold run (real bold-weight face) should thicken the glyph too");
    }

    #[test]
    fn rich_text_wraps_when_a_run_font_size_makes_a_word_too_wide() {
        // Synthetic per-char width: chars tagged with run 0 are drawn
        // "twice as wide" (simulating a much bigger font size than the
        // rest) — deterministic, unlike scanning rendered pixels against
        // real font metrics.
        let measure = |chars: &[(char, Option<usize>)]| -> f32 {
            chars.iter().map(|&(_, idx)| if idx == Some(0) { 20.0 } else { 10.0 }).sum()
        };
        let line: Vec<(char, Option<usize>)> = "aa bb"
            .chars()
            .map(|c| (c, if c == 'a' { Some(0) } else { None }))
            .collect();
        // "aa" (run 0) alone is 40px; "bb" alone is 20px; joined with a
        // 10px space that's 70px — over a 45px max_width, so they must
        // wrap onto separate lines even though the same content at a
        // uniform (non-run) width would comfortably fit on one.
        let wrapped = wrap_line_rich(&line, &measure, 10.0, 45.0);
        assert_eq!(wrapped.len(), 2, "expected the run's larger characters to force a wrap that a uniform-width line wouldn't need");
    }

    #[test]
    fn rich_text_wraps_visibly_onto_a_second_line_when_forced() {
        // Not trying to predict exact real-font wrap points (see the
        // deterministic `wrap_line_rich` test above for that) — just
        // confirms the whole pipeline (tagging → rich wrap → draw) ends up
        // with content in both halves of a box too narrow for the content
        // at any single line.
        let mut layer = text_layer("aaaaaaaa bbbbbbbb", 24.0, egui::Vec2::new(70.0, 80.0), TextAlign::Left);
        if let LayerKind::Text { resize, runs, .. } = &mut layer.kind {
            *resize = TextResize::Fixed;
            *runs = vec![
                TextRun { len: "aaaaaaaa ".chars().count(), style: run_style(24.0) },
                TextRun { len: "bbbbbbbb".chars().count(), style: run_style(24.0) },
            ];
        }
        let pixmap = render_layer(&layer).expect("should render");
        let opaque_in_band = |y_range: std::ops::Range<u32>| {
            (0..pixmap.width())
                .any(|x| y_range.clone().any(|y| pixmap.pixel(x, y).map(|p| p.alpha() > 0).unwrap_or(false)))
        };
        assert!(opaque_in_band(0..(pixmap.height() / 2)), "expected content on the first wrapped line");
        assert!(opaque_in_band((pixmap.height() / 2)..pixmap.height()), "expected content wrapped onto a second line");
    }

    #[test]
    fn linear_gradient_fill_interpolates_left_to_right() {
        let mut layer = Layer::new(
            "Rect",
            Frame::from_two_points(Pos2::new(0.0, 0.0), Pos2::new(100.0, 20.0)),
            LayerKind::Rectangle { corner_radius: CornerRadii::ZERO },
        );
        layer.style.fill = Some(crate::model::Paint::Gradient(crate::model::Gradient::linear(
            Color32::from_rgb(255, 0, 0),
            Color32::from_rgb(0, 0, 255),
        )));
        layer.style.stroke = None;

        let pixmap = render_layer(&layer).expect("should render");
        let left = pixmap.pixel(1, 10).expect("left pixel");
        let right = pixmap.pixel(98, 10).expect("right pixel");
        assert!(left.red() > 200 && left.blue() < 50, "left edge should be near the start color (red)");
        assert!(right.blue() > 200 && right.red() < 50, "right edge should be near the end color (blue)");
    }

    #[test]
    fn radial_gradient_fill_is_brightest_at_center() {
        let mut layer = Layer::new(
            "Oval",
            Frame::from_two_points(Pos2::new(0.0, 0.0), Pos2::new(80.0, 80.0)),
            LayerKind::Oval,
        );
        layer.style.fill = Some(crate::model::Paint::Gradient(crate::model::Gradient::radial(
            Color32::from_rgb(255, 255, 255),
            Color32::from_rgb(0, 0, 0),
        )));
        layer.style.stroke = None;

        let pixmap = render_layer(&layer).expect("should render");
        let center = pixmap.pixel(40, 40).expect("center pixel");
        let edge = pixmap.pixel(40, 41 + 38).expect("near-edge pixel");
        assert!(center.red() > edge.red(), "radial gradient should be brightest at its center");
    }

    #[test]
    fn gradient_stroke_interpolates_along_a_line() {
        let mut layer = Layer::new(
            "Line",
            Frame::from_two_points(Pos2::new(0.0, 0.0), Pos2::new(100.0, 0.0)),
            LayerKind::Line,
        );
        layer.style.fill = None;
        layer.style.stroke = Some(crate::model::Stroke {
            paint: crate::model::Paint::Gradient(crate::model::Gradient::linear(
                Color32::from_rgb(255, 0, 0),
                Color32::from_rgb(0, 0, 255),
            )),
            width: 6.0,
        });

        let pixmap = render_layer(&layer).expect("should render");
        let left = pixmap.pixel(2, pixmap.height() / 2).expect("left pixel");
        let right = pixmap.pixel(pixmap.width() - 3, pixmap.height() / 2).expect("right pixel");
        assert!(left.red() > 200 && left.blue() < 50, "stroke start should be near the start color (red)");
        assert!(right.blue() > 200 && right.red() < 50, "stroke end should be near the end color (blue)");
    }

    fn noise_rect_layer(seed: u32) -> Layer {
        let mut layer = Layer::new(
            "Rect",
            Frame::from_two_points(Pos2::new(0.0, 0.0), Pos2::new(60.0, 60.0)),
            LayerKind::Rectangle { corner_radius: CornerRadii::ZERO },
        );
        layer.style.fill = Some(crate::model::Paint::Noise(crate::model::NoiseFill {
            base: Color32::from_rgb(128, 128, 128),
            intensity: 0.8,
            scale: 3.0,
            seed,
        }));
        layer.style.stroke = None;
        layer
    }

    #[test]
    fn noise_fill_renders_grain_not_a_flat_color() {
        let pixmap = render_layer(&noise_rect_layer(7)).expect("should render");
        let colors: std::collections::HashSet<[u8; 4]> =
            pixmap.pixels().iter().map(|p| [p.red(), p.green(), p.blue(), p.alpha()]).collect();
        assert!(colors.len() > 1, "noise fill should vary pixel-to-pixel, not render as a single flat color");
    }

    #[test]
    fn noise_fill_is_deterministic_for_the_same_seed() {
        let a = render_layer(&noise_rect_layer(7)).expect("should render");
        let b = render_layer(&noise_rect_layer(7)).expect("should render");
        assert_eq!(a.data(), b.data(), "the same seed should always render the same grain pattern");
    }

    #[test]
    fn noise_fill_differs_across_seeds() {
        let a = render_layer(&noise_rect_layer(7)).expect("should render");
        let b = render_layer(&noise_rect_layer(8)).expect("should render");
        assert_ne!(a.data(), b.data(), "different seeds should render different grain patterns");
    }

    fn halftone_rect_layer() -> Layer {
        let mut layer = Layer::new(
            "Rect",
            Frame::from_two_points(Pos2::new(0.0, 0.0), Pos2::new(60.0, 60.0)),
            LayerKind::Rectangle { corner_radius: CornerRadii::ZERO },
        );
        layer.style.fill = Some(crate::model::Paint::Halftone(crate::model::HalftoneFill {
            background: Color32::WHITE,
            dot: Color32::BLACK,
            scale: 10.0,
            coverage: 0.8,
        }));
        layer.style.stroke = None;
        layer
    }

    #[test]
    fn halftone_fill_renders_a_dot_grid_not_a_flat_color() {
        let pixmap = render_layer(&halftone_rect_layer()).expect("should render");
        let colors: std::collections::HashSet<[u8; 4]> =
            pixmap.pixels().iter().map(|p| [p.red(), p.green(), p.blue(), p.alpha()]).collect();
        assert!(colors.len() > 1, "halftone fill should show both dot and background colors, not a single flat color");
    }

    fn two_color_tile_png() -> Vec<u8> {
        let mut img = image::RgbaImage::new(2, 1);
        img.put_pixel(0, 0, image::Rgba([255, 0, 0, 255]));
        img.put_pixel(1, 0, image::Rgba([0, 0, 255, 255]));
        crate::image_ops::encode_png(&img)
    }

    fn pattern_rect_layer(tile_width: f32) -> Layer {
        let mut layer = Layer::new(
            "Rect",
            Frame::from_two_points(Pos2::new(0.0, 0.0), Pos2::new(80.0, 20.0)),
            LayerKind::Rectangle { corner_radius: CornerRadii::ZERO },
        );
        layer.style.fill = Some(crate::model::Paint::Pattern(crate::model::PatternFill {
            encoded: two_color_tile_png(),
            tile_width,
        }));
        layer.style.stroke = None;
        layer
    }

    #[test]
    fn pattern_fill_contains_the_tile_images_colors() {
        let pixmap = render_layer(&pattern_rect_layer(20.0)).expect("should render");
        let has_reddish = pixmap.pixels().iter().any(|p| p.red() > 150 && p.blue() < 100);
        let has_blueish = pixmap.pixels().iter().any(|p| p.blue() > 150 && p.red() < 100);
        assert!(has_reddish && has_blueish, "pattern fill should contain both of the tile image's colors");
    }

    #[test]
    fn pattern_fill_tiles_the_same_color_reappears_at_tile_width_offset() {
        let tile_width = 20.0;
        let pixmap = render_layer(&pattern_rect_layer(tile_width)).expect("should render");
        let y = pixmap.height() / 2;
        let first = pixmap.pixel(2, y).expect("first tile pixel");
        let second = pixmap.pixel(2 + tile_width as u32, y).expect("second tile pixel");
        assert!(first.red() > 150 && first.blue() < 100, "first tile should start reddish");
        assert!(second.red() > 150 && second.blue() < 100, "pattern should repeat: second tile should also start reddish");
    }
}
