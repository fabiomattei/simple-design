//! Text attached to a path outline: each glyph rasterized individually,
//! rotated to the path's tangent, with underline/strikethrough drawn as
//! rotated per-glyph segments.

use ab_glyph::{Font, FontRef, GlyphId, ScaleFont};
use tiny_skia::{FillRule, Paint, PathBuilder, Pixmap, PixmapPaint, Transform};

use crate::model::text_runs::{RunStyle, TextRun};
use crate::model::{ListType, TextAlign, TextTransform};

use super::glyph_raster::blend_glyph_pixel;
use super::paint::{to_color, with_opacity};

/// Renders `content`'s first line along `path_points` (already in the same
/// pixmap coordinate space `pixmap` is drawn in), one glyph at a time — the
/// export-side counterpart to `text_on_path_layout::layout_glyphs_on_path`,
/// built on `ab_glyph`/`tiny_skia` instead of `egui::Galley` (see `draw_text`'s
/// doc comment on canvas/export being separate render paths). Uses the same
/// multi-face, run-tagged resolution as `draw_text_rich`/`draw_text_in_area`
/// (via `tagged_lines`/`fonts::run_font_byte_bufs`), so per-character font/
/// size/color/bold/italic all carry over from the live canvas render — an
/// empty `runs` degenerates to "every char uses `base`" for free, same
/// convention as those two. Underline/strikethrough are the one thing still
/// not reproduced here: a decoration would need its own rotated-quad
/// composite per glyph run, which isn't worth the complexity for a rarely
/// combined pair of features (documented v1 limit).
/// `render_scale`: pixmap pixels per document unit, see `glyph_raster::draw_text_glyph`.
#[allow(clippy::too_many_arguments)]
pub(super) fn draw_text_on_path(
    pixmap: &mut Pixmap,
    content: &str,
    runs: &[TextRun],
    base: &RunStyle,
    align: TextAlign,
    letter_spacing: f32,
    transform: TextTransform,
    path_points: &[egui::Pos2],
    closed: bool,
    path_offset: f32,
    start: f32,
    flip: bool,
    opacity: f32,
    render_scale: f32,
) {
    if crate::text_path_geometry::polyline_length(path_points, closed) < 1.0 {
        return;
    }
    let Some(tagged_first_line) = crate::text_layout::tagged_lines(content, runs, transform, ListType::None, 1).into_iter().next() else {
        return;
    };
    if tagged_first_line.is_empty() {
        return;
    }

    let byte_bufs = crate::fonts::run_font_byte_bufs(base, runs);
    let Ok(base_face) = FontRef::try_from_slice_and_index(&byte_bufs[0].0, byte_bufs[0].1) else { return };
    let mut faces: Vec<FontRef> = vec![base_face];
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

    let points: Vec<egui::Pos2> = if flip { path_points.iter().rev().copied().collect() } else { path_points.to_vec() };
    let total = crate::text_path_geometry::polyline_length(&points, closed);

    let mut glyphs = Vec::with_capacity(tagged_first_line.len());
    let mut prev: Option<(GlyphId, usize)> = None;
    for &(c, idx) in &tagged_first_line {
        let face_idx = face_index_of(idx);
        let scale = scale_of(idx);
        let scaled = faces[face_idx].as_scaled(scale);
        let id = scaled.glyph_id(c);
        let kern = match prev {
            Some((prev_id, prev_face_idx)) if prev_face_idx == face_idx => scaled.kern(prev_id, id),
            _ => 0.0,
        };
        let width = scaled.h_advance(id) + letter_spacing + kern;
        glyphs.push((id, face_idx, scale, idx, width));
        prev = Some((id, face_idx));
    }
    let text_len: f32 = glyphs.iter().map(|&(.., w)| w).sum();
    let start_distance = start * total
        - match align {
            TextAlign::Center => text_len / 2.0,
            TextAlign::Right => text_len,
            TextAlign::Left | TextAlign::Justify => 0.0,
        };

    let mut distance = start_distance;
    for (id, face_idx, scale, idx, width) in glyphs {
        if let Some((point, angle)) = crate::text_path_geometry::point_and_tangent_at(&points, closed, distance) {
            let style = style_of(idx);
            let color = with_opacity(style.color.unwrap_or(base.color.unwrap_or(egui::Color32::BLACK)), opacity);
            let normal = egui::Vec2::new(-angle.sin(), angle.cos());
            let anchor = point + normal * path_offset;
            draw_rotated_glyph(pixmap, &faces[face_idx], id, scale, anchor, angle, color, style.bold, style.italic, render_scale);
            if style.underline || style.strikethrough {
                let thickness = (style.font_size * 0.06).max(1.0);
                if style.underline {
                    draw_rotated_decoration_segment(pixmap, anchor, angle, width - letter_spacing, style.font_size * 0.08, thickness, color, render_scale);
                }
                if style.strikethrough {
                    draw_rotated_decoration_segment(pixmap, anchor, angle, width - letter_spacing, -style.font_size * 0.3, thickness, color, render_scale);
                }
            }
        }
        distance += width;
    }
}

/// Rasterizes one glyph unrotated into a scratch pixmap sized to its own
/// outline bounds (plus extra width if `italic`, to leave room for the
/// shear), then composites it onto `pixmap` rotated by `angle` (radians,
/// clockwise) about `anchor` — `anchor` is the glyph's baseline-left origin
/// (the point passed to `ab_glyph`'s `with_scale_and_position`), so rotating
/// about it tilts the glyph tangent to the path, mirroring
/// `text_on_path_layout::PathGlyph`'s "rotate about `pos`" convention. Glyph
/// rasterization writes pixels directly (`ab_glyph`'s `outline.draw`
/// callback), bypassing `tiny_skia::Transform` entirely — so, same as
/// `draw_layer`'s whole-layer rotation branch above, this renders unrotated
/// into a scratch pixmap first and composites that with a rotate transform,
/// just applied once per glyph instead of once per layer. `bold` fakes a
/// heavier weight via the same double-draw-offset hack `draw_text_glyph`
/// uses; `italic` applies the same shear `draw_text_glyph` does, just in the
/// scratch pixmap's own local (pre-rotation) coordinates, so it tilts with
/// the glyph rather than staying screen-vertical. `scale` and `anchor` are in
/// document units; `render_scale` (see `export::draw_layer`) converts them to
/// pixmap pixels, so the glyph is rasterized at full resolution.
#[allow(clippy::too_many_arguments)]
pub(super) fn draw_rotated_glyph(pixmap: &mut Pixmap, face: &FontRef, id: GlyphId, scale: ab_glyph::PxScale, anchor: egui::Pos2, angle: f32, color: egui::Color32, bold: bool, italic: bool, render_scale: f32) {
    const ITALIC_SHEAR: f32 = 0.22;
    let scale = ab_glyph::PxScale { x: scale.x * render_scale, y: scale.y * render_scale };
    let anchor = (anchor.to_vec2() * render_scale).to_pos2();
    let x_offsets: &[f32] = if bold { &[0.0, 0.6 * render_scale] } else { &[0.0] };
    for &dx in x_offsets {
        let glyph = id.with_scale_and_position(scale, ab_glyph::point(dx, 0.0));
        let Some(outlined) = face.outline_glyph(glyph) else { continue };
        let px_bounds = outlined.px_bounds();
        let w = px_bounds.width().ceil().max(1.0) as u32;
        let h = px_bounds.height().ceil().max(1.0) as u32;
        // The row (in local, pre-rotation coordinates) where the real
        // baseline (y=0 in font units) falls — glyph pixels above it shear
        // right, below it shear left, matching `draw_text_glyph`'s
        // `(baseline_y - raw_y) * ITALIC_SHEAR` convention.
        let local_baseline = -px_bounds.min.y;
        let shear_pad = if italic { (h as f32 * ITALIC_SHEAR).ceil() as i32 + 1 } else { 0 };
        let Some(mut sub) = Pixmap::new((w as i32 + 2 * shear_pad).max(1) as u32, h) else { continue };
        outlined.draw(|gx, gy, coverage| {
            let x = if italic {
                gx as i32 + shear_pad + ((local_baseline - gy as f32) * ITALIC_SHEAR).round() as i32
            } else {
                gx as i32
            };
            blend_glyph_pixel(&mut sub, x, gy as i32, coverage, color);
        });
        let paint = PixmapPaint { opacity: 1.0, blend_mode: tiny_skia::BlendMode::SourceOver, quality: tiny_skia::FilterQuality::Bilinear };
        let transform = Transform::from_translate(anchor.x + px_bounds.min.x - shear_pad as f32, anchor.y + px_bounds.min.y)
            .post_rotate_at(angle.to_degrees(), anchor.x, anchor.y);
        pixmap.draw_pixmap(0, 0, sub.as_ref(), &paint, transform, None);
    }
}

/// Fills a `width`-long, `thickness`-tall bar in the glyph's own *local*
/// (pre-rotation) space — starting at local `(0, local_y)`, spanning that
/// one glyph's own advance width — then rotates its 4 corners about
/// `anchor` by `angle` (radians, clockwise), same convention as
/// `draw_rotated_glyph`. One short straight segment per glyph, chained
/// along the path, approximates a continuous underline/strikethrough
/// hugging the curve (each segment is too short for the curve's own bend to
/// be visible within it at any font size this app supports). Built in
/// document units, scaled by `render_scale` at draw time.
#[allow(clippy::too_many_arguments)]
pub(super) fn draw_rotated_decoration_segment(pixmap: &mut Pixmap, anchor: egui::Pos2, angle: f32, width: f32, local_y: f32, thickness: f32, color: egui::Color32, render_scale: f32) {
    if width <= 0.0 {
        return;
    }
    let (sin_a, cos_a) = angle.sin_cos();
    let local_corners = [(0.0, local_y), (width, local_y), (width, local_y + thickness), (0.0, local_y + thickness)];
    let mut pb = PathBuilder::new();
    for (i, &(lx, ly)) in local_corners.iter().enumerate() {
        let x = anchor.x + lx * cos_a - ly * sin_a;
        let y = anchor.y + lx * sin_a + ly * cos_a;
        if i == 0 {
            pb.move_to(x, y);
        } else {
            pb.line_to(x, y);
        }
    }
    pb.close();
    let Some(path) = pb.finish() else { return };
    let mut paint = Paint::default();
    paint.set_color(to_color(color));
    paint.anti_alias = true;
    pixmap.fill_path(&path, &paint, FillRule::Winding, Transform::from_scale(render_scale, render_scale), None);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::export::render_layer;
    use crate::model::{Frame, Layer, LayerKind, TextFont, TextResize, VerticalAlign};
    use egui::{Color32, Pos2, Vec2};

    fn text_path_attachment_layer(target: crate::model::LayerId, mode: crate::model::TextPathMode) -> Layer {
        let mut text = Layer::new(
            "Label",
            Frame { pos: Pos2::new(0.0, 0.0), size: egui::Vec2::new(10.0, 10.0), rotation: 0.0 },
            LayerKind::Text {
                content: "HELLO".to_string(),
                font_size: 20.0,
                font: TextFont::Proportional,
                align: TextAlign::Left,
                vertical_align: VerticalAlign::Top,
                resize: TextResize::Auto,
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
                path_attachment: Some(crate::model::TextPathAttachment { target, mode, offset: 0.0, start: 0.0, flip: false }),
            },
        );
        text.style.fill = Some(crate::model::Paint::Solid(Color32::BLACK));
        text.style.stroke = None;
        text
    }

    #[test]
    fn on_path_text_renders_glyphs_along_the_targets_outline() {
        let mut guide = Layer::new(
            "Guide",
            Frame::from_two_points(Pos2::new(10.0, 30.0), Pos2::new(190.0, 30.0)),
            LayerKind::Line,
        );
        guide.style.fill = None;
        guide.style.stroke = None;
        let target_id = guide.id;

        let text = text_path_attachment_layer(target_id, crate::model::TextPathMode::OnPath);

        let mut artboard = Layer::new_artboard("Board", Frame::from_two_points(Pos2::new(0.0, 0.0), Pos2::new(200.0, 60.0)));
        if let LayerKind::Artboard { children, .. } = &mut artboard.kind {
            children.push(guide);
            children.push(text);
        }

        let pixmap = render_layer(&artboard).expect("should render");
        let any_opaque = pixmap.pixels().iter().any(|p| p.alpha() > 0);
        assert!(any_opaque, "expected on-path text glyphs to render along the guide line, but the canvas is empty");
    }

    /// A drop shadow on a path-attached `Text` layer must be shaped from the
    /// *attached* glyphs (sitting along the guide, far from the layer's own
    /// tiny unattached frame) — not from the plain unattached layout
    /// `render_layer_plain`'s silhouette pass would fall back to if it
    /// couldn't resolve the attachment (see `render_layer_plain`'s `roots`
    /// param). Proven by a distinctly-colored shadow showing up near the
    /// guide, not clustered at the unattached frame's own tiny position.
    #[test]
    fn drop_shadow_on_path_attached_text_follows_the_attached_glyphs_not_the_unattached_frame() {
        let mut guide = Layer::new("Guide", Frame::from_two_points(Pos2::new(10.0, 100.0), Pos2::new(190.0, 100.0)), LayerKind::Line);
        guide.style.fill = None;
        guide.style.stroke = None;
        let target_id = guide.id;

        let mut text = text_path_attachment_layer(target_id, crate::model::TextPathMode::OnPath);
        text.style.fill = Some(crate::model::Paint::Solid(Color32::from_rgb(255, 0, 0)));
        text.style.shadows = vec![crate::model::Shadow {
            color: Color32::from_rgb(0, 0, 255),
            offset: Vec2::ZERO,
            blur: 6.0,
            spread: 0.0,
        }];

        let mut artboard = Layer::new_artboard("Board", Frame::from_two_points(Pos2::new(0.0, 0.0), Pos2::new(200.0, 150.0)));
        if let LayerKind::Artboard { children, .. } = &mut artboard.kind {
            children.push(guide);
            children.push(text);
        }

        let pixmap = render_layer(&artboard).expect("should render");
        let blueish_near_guide = (0..pixmap.width()).any(|x| {
            (85u32..135).any(|y| pixmap.pixel(x, y.min(pixmap.height() - 1)).is_some_and(|p| p.blue() > 100 && p.blue() as i32 - p.red() as i32 > 40))
        });
        let blueish_near_unattached_frame = (0..pixmap.width()).any(|x| {
            (0u32..10).any(|y| pixmap.pixel(x, y).is_some_and(|p| p.blue() > 100 && p.blue() as i32 - p.red() as i32 > 40))
        });
        assert!(blueish_near_guide, "expected the blue drop shadow to show up near the guide line (y~100), where the attached glyphs actually sit");
        assert!(
            !blueish_near_unattached_frame,
            "the blue drop shadow should not appear near the layer's own tiny unattached frame (y=0..10) — that would mean the silhouette fell back to the unattached layout"
        );
    }

    #[test]
    fn on_path_text_reproduces_per_character_run_colors() {
        let mut guide = Layer::new(
            "Guide",
            Frame::from_two_points(Pos2::new(10.0, 30.0), Pos2::new(190.0, 30.0)),
            LayerKind::Line,
        );
        guide.style.fill = None;
        guide.style.stroke = None;
        let target_id = guide.id;

        let mut text = text_path_attachment_layer(target_id, crate::model::TextPathMode::OnPath);
        if let LayerKind::Text { content, font_size, runs, .. } = &mut text.kind {
            *content = "AABB".to_string();
            *font_size = 32.0;
            *runs = vec![
                TextRun {
                    len: 2,
                    style: RunStyle { font: TextFont::Proportional, font_size: 32.0, color: Some(Color32::BLACK), bold: false, italic: false, underline: false, strikethrough: false },
                },
                TextRun {
                    len: 2,
                    style: RunStyle {
                        font: TextFont::Proportional,
                        font_size: 32.0,
                        color: Some(Color32::from_rgb(220, 20, 20)),
                        bold: false,
                        italic: false,
                        underline: false,
                        strikethrough: false,
                    },
                },
            ];
        }

        let mut artboard = Layer::new_artboard("Board", Frame::from_two_points(Pos2::new(0.0, 0.0), Pos2::new(200.0, 60.0)));
        if let LayerKind::Artboard { children, .. } = &mut artboard.kind {
            children.push(guide);
            children.push(text);
        }

        let pixmap = render_layer(&artboard).expect("should render");
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
        assert!(saw_black && saw_red, "expected both the black first run and the red second run to appear along the path (black={saw_black}, red={saw_red})");
    }

    fn render_on_path_opaque_count(underline: bool, strikethrough: bool) -> usize {
        let mut guide = Layer::new(
            "Guide",
            Frame::from_two_points(Pos2::new(10.0, 30.0), Pos2::new(190.0, 30.0)),
            LayerKind::Line,
        );
        guide.style.fill = None;
        guide.style.stroke = None;
        let target_id = guide.id;

        let mut text = text_path_attachment_layer(target_id, crate::model::TextPathMode::OnPath);
        if let LayerKind::Text { content, font_size, underline: u, strikethrough: s, .. } = &mut text.kind {
            *content = "III".to_string();
            *font_size = 32.0;
            *u = underline;
            *s = strikethrough;
        }

        let group = Layer::new(
            "Group",
            Frame::from_two_points(Pos2::new(0.0, 0.0), Pos2::new(200.0, 60.0)),
            LayerKind::Group { children: vec![guide, text] },
        );

        let pixmap = render_layer(&group).expect("should render");
        pixmap.pixels().iter().filter(|p| p.alpha() > 0).count()
    }

    #[test]
    fn on_path_underline_adds_extra_opaque_pixels() {
        let base = render_on_path_opaque_count(false, false);
        let underlined = render_on_path_opaque_count(true, false);
        assert!(underlined > base, "underlined on-path text (opaque={underlined}) should cover more pixels than plain (opaque={base})");
    }

    #[test]
    fn on_path_strikethrough_adds_extra_opaque_pixels() {
        let base = render_on_path_opaque_count(false, false);
        let struck = render_on_path_opaque_count(false, true);
        assert!(struck > base, "struck-through on-path text (opaque={struck}) should cover more pixels than plain (opaque={base})");
    }

    #[test]
    fn italic_on_path_text_renders_without_clipping_the_sheared_glyphs() {
        let mut guide = Layer::new(
            "Guide",
            Frame::from_two_points(Pos2::new(10.0, 30.0), Pos2::new(190.0, 30.0)),
            LayerKind::Line,
        );
        guide.style.fill = None;
        guide.style.stroke = None;
        let target_id = guide.id;

        let mut text = text_path_attachment_layer(target_id, crate::model::TextPathMode::OnPath);
        if let LayerKind::Text { content, font_size, italic, .. } = &mut text.kind {
            *content = "HELLO".to_string();
            *font_size = 32.0;
            *italic = true;
        }

        let mut artboard = Layer::new_artboard("Board", Frame::from_two_points(Pos2::new(0.0, 0.0), Pos2::new(200.0, 60.0)));
        if let LayerKind::Artboard { children, .. } = &mut artboard.kind {
            children.push(guide);
            children.push(text);
        }

        let pixmap = render_layer(&artboard).expect("should render");
        let any_opaque = pixmap.pixels().iter().any(|p| p.alpha() > 0);
        assert!(any_opaque, "expected italic on-path text to still render glyphs, not panic or vanish");
    }
}
