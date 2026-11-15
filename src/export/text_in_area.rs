//! Text wrapped inside a closed shape's outline (`TextPathMode::AreaInside`),
//! using the backend-agnostic row layout from `text_area_wrap`.

use ab_glyph::{Font, FontRef, GlyphId, ScaleFont};
use tiny_skia::Pixmap;

use crate::model::text_runs::{RunStyle, TextRun};
use crate::model::{TextAlign, TextTransform};

use super::glyph_raster::{draw_decoration_rect, draw_text_glyph};
use super::paint::{to_color, with_opacity};

/// Rasterizes `content` word-wrapped inside the closed outline `outline`
/// (inset by `padding`), following its actual contour line by line — the
/// export-side counterpart to `text_area_layout::layout_text_in_area`, both
/// built on the shared, backend-agnostic wrapping decision in
/// `text_area_wrap::wrap_in_area` (only how "these tagged chars at this
/// position" become pixels differs, same split as `draw_text_on_path`/
/// `text_on_path_layout`). Always goes through the multi-face machinery
/// (same as `draw_text_rich`) rather than having a separate uniform-style
/// path, since an empty `runs` already degenerates to "every char uses
/// `base`" for free.
/// `render_scale`: pixmap pixels per document unit, see `glyph_raster::draw_text_glyph`.
#[allow(clippy::too_many_arguments)]
pub(super) fn draw_text_in_area(
    pixmap: &mut Pixmap,
    content: &str,
    runs: &[TextRun],
    base: &RunStyle,
    align: TextAlign,
    line_height: Option<f32>,
    letter_spacing: f32,
    transform: TextTransform,
    outline: &[egui::Pos2],
    padding: f32,
    opacity: f32,
    render_scale: f32,
) {
    if content.is_empty() {
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

    let measure = |chars: &[(char, Option<usize>)]| -> f32 {
        let mut width = 0.0;
        let mut prev: Option<(GlyphId, usize)> = None;
        for &(c, idx) in chars {
            let face_idx = face_index_of(idx);
            let scaled = faces[face_idx].as_scaled(scale_of(idx));
            let id = scaled.glyph_id(c);
            if let Some((prev_id, prev_face_idx)) = prev
                && prev_face_idx == face_idx {
                    width += scaled.kern(prev_id, id);
                }
            width += scaled.h_advance(id) + letter_spacing;
            prev = Some((id, face_idx));
        }
        width
    };

    let base_scaled = faces[0].as_scaled(scale_of(None));
    let resolved_line_height = line_height.unwrap_or_else(|| base_scaled.height() + base_scaled.line_gap()).max(1.0);

    let rows = crate::text_area_wrap::wrap_in_area(content, runs, transform, &measure, resolved_line_height, align, outline, padding);

    for row in rows {
        let ascent = row
            .chars
            .iter()
            .map(|&(_, idx)| faces[face_index_of(idx)].as_scaled(scale_of(idx)).ascent())
            .fold(base_scaled.ascent(), f32::max);
        let baseline_y = row.y + ascent;

        let mut cursor_x = row.x;
        let mut prev: Option<(GlyphId, usize)> = None;
        let mut extents: Vec<(f32, f32, Option<usize>)> = Vec::with_capacity(row.chars.len());
        for &(c, idx) in &row.chars {
            let face_idx = face_index_of(idx);
            let scale = scale_of(idx);
            let scaled = faces[face_idx].as_scaled(scale);
            let id = scaled.glyph_id(c);
            if let Some((prev_id, prev_face_idx)) = prev
                && prev_face_idx == face_idx {
                    cursor_x += scaled.kern(prev_id, id);
                }
            let style = style_of(idx);
            let color = with_opacity(style.color.unwrap_or(base.color.unwrap_or(egui::Color32::BLACK)), opacity);
            let x_before = cursor_x;
            draw_text_glyph(pixmap, &faces[face_idx], id, scale, cursor_x, baseline_y, color, false, style.italic, None, render_scale);
            cursor_x += scaled.h_advance(id) + letter_spacing;
            extents.push((x_before, cursor_x, idx));
            prev = Some((id, face_idx));
        }

        // Same contiguous-stretch decoration grouping as `draw_text_rich`.
        let mut i = 0usize;
        while i < extents.len() {
            let idx = extents[i].2;
            let style = style_of(idx);
            let mut j = i + 1;
            while j < extents.len()
                && style_of(extents[j].2).underline == style.underline
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
                        draw_decoration_rect(pixmap, rect, None, to_color(deco_color), render_scale);
                    }
                    if style.strikethrough {
                        let y = baseline_y - style.font_size * 0.3;
                        let rect = egui::Rect::from_min_size(egui::Pos2::new(x0, y), egui::Vec2::new(x1 - x0, deco_thickness));
                        draw_decoration_rect(pixmap, rect, None, to_color(deco_color), render_scale);
                    }
                }
            }
            i = j;
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::export::render_layer;
    use crate::model::{
        CornerRadii, Frame, Layer, LayerKind, ListType, TextAlign, TextFont, TextResize, TextTransform, VerticalAlign,
    };
    use egui::{Color32, Pos2};

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
    fn area_inside_text_stays_within_the_guide_rectangle() {
        let mut rect = Layer::new(
            "Guide",
            Frame::from_two_points(Pos2::new(10.0, 10.0), Pos2::new(190.0, 90.0)),
            LayerKind::Rectangle { corner_radius: CornerRadii::ZERO },
        );
        rect.style.fill = None;
        rect.style.stroke = None;
        let target_id = rect.id;

        let text = text_path_attachment_layer(target_id, crate::model::TextPathMode::AreaInside);

        let mut artboard = Layer::new_artboard("Board", Frame::from_two_points(Pos2::new(0.0, 0.0), Pos2::new(200.0, 100.0)));
        if let LayerKind::Artboard { children, .. } = &mut artboard.kind {
            children.push(rect);
            children.push(text);
        }

        let pixmap = render_layer(&artboard).expect("should render");
        let any_opaque = pixmap.pixels().iter().any(|p| p.alpha() > 0);
        assert!(any_opaque, "expected area-type text glyphs to render inside the guide rectangle, but the canvas is empty");
    }
}
