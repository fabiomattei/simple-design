//! Rasterizes a `Text` layer with per-character style runs, resolving each
//! run's face and scale per character.

use ab_glyph::{Font, FontRef, GlyphId, ScaleFont};
use tiny_skia::Pixmap;

use crate::model::text_runs::{RunStyle, TextRun};
use crate::model::{ListType, TextAlign, TextResize, TextTransform, VerticalAlign};

use super::glyph_raster::{draw_decoration_rect, draw_text_glyph};
use super::paint::{to_color, with_opacity};
use super::text_wrap::wrap_line_rich;

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
/// `render_scale`: pixmap pixels per document unit, see `glyph_raster::draw_text_glyph`.
#[allow(clippy::too_many_arguments)]
pub(super) fn draw_text_rich(
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
    render_scale: f32,
) {
    if content.is_empty() {
        return;
    }

    let byte_bufs = crate::fonts::run_font_byte_bufs(base, runs);
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
            if let Some((prev_id, prev_face_idx)) = prev
                && prev_face_idx == face_idx {
                    cursor_x += scaled.kern(prev_id, id);
                }
            let style = style_of(idx);
            let run_color = with_opacity(style.color.unwrap_or(base.color.unwrap_or(egui::Color32::BLACK)), opacity);
            let x_before = cursor_x;
            draw_text_glyph(pixmap, &faces[face_idx], id, scale, cursor_x, baseline_y, run_color, false, style.italic, clip, render_scale);
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
                        draw_decoration_rect(pixmap, rect, clip, to_color(deco_color), render_scale);
                    }
                    if style.strikethrough {
                        let y = baseline_y - style.font_size * 0.3;
                        let rect = egui::Rect::from_min_size(egui::Pos2::new(x0, y), egui::Vec2::new(x1 - x0, deco_thickness));
                        draw_decoration_rect(pixmap, rect, clip, to_color(deco_color), render_scale);
                    }
                }
            }
            i = j;
        }
    }
}
