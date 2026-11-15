//! Rasterizes a uniformly styled `Text` layer (no per-character runs).

use ab_glyph::{Font, FontRef, GlyphId, ScaleFont};
use tiny_skia::Pixmap;

use crate::model::{Layer, ListType, TextAlign, TextFont, TextResize, TextTransform, VerticalAlign};

use super::glyph_raster::{draw_decoration_rect, draw_text_glyph};
use super::paint::{to_color, with_opacity};
use super::text_wrap::wrap_line;

/// Rasterizes a `Text` layer's glyphs directly onto `pixmap` via `ab_glyph`,
/// blending each glyph's per-pixel AA coverage over the existing contents.
/// `bounds` is `layer.frame.bounds()` already translated by `offset` (same
/// value the caller derived for other shapes). Uses the same bundled font
/// files (`epaint_default_fonts`) the canvas draws with via egui, so export
/// visually matches the on-canvas glyphs.
///
/// Independent of `canvas/`/`text_layout.rs`'s `egui::Galley`-based layout
/// (per this codebase's convention of canvas/export being separate render
/// paths — see `CLAUDE.md`) except for `text_layout::display_string`, which
/// is pure string manipulation with no rendering-backend dependency and so
/// is shared to keep list numbering/text-transform behavior identical
/// between the two.
/// `render_scale`: pixmap pixels per document unit, see `glyph_raster::draw_text_glyph`.
#[allow(clippy::too_many_arguments)]
pub(super) fn draw_text(
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
    render_scale: f32,
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
            draw_text_glyph(pixmap, &face, id, scale, cursor_x, baseline_y, color, bold, italic, clip, render_scale);
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
                draw_decoration_rect(pixmap, rect, clip, to_color(color), render_scale);
            }
            if strikethrough {
                let y = baseline_y - font_size * 0.3;
                let rect = egui::Rect::from_min_size(
                    egui::Pos2::new(start_x, y),
                    egui::Vec2::new(cursor_x - start_x, deco_thickness),
                );
                draw_decoration_rect(pixmap, rect, clip, to_color(color), render_scale);
            }
        }
    }
}
