//! Per-glyph placement for a `Text` layer whose `path_attachment.mode` is
//! `TextPathMode::OnPath` — walks `content` character by character along a
//! path (from `text_path_geometry`), producing one already-laid-out
//! single-character `Galley` plus a position/angle per glyph, for the canvas
//! (`canvas.rs`) and exporter (`export.rs`) to each paint in their own way.
//!
//! Only the first `\n`-separated line of `content` is placed — on-path text
//! has no natural place to put a second line (there's exactly one path to
//! follow), so embedded line breaks beyond the first are ignored, same
//! simplification every "type on a path" tool makes.
//!
//! All spatial inputs/outputs (`path_points`, `offset`, the returned
//! `PathGlyph::pos`) are expected in the same already-zoom-scaled space —
//! callers pass screen-space points and a screen-space offset, exactly like
//! `text_layout::layout_paragraphs`'s already-scaled `wrap_width`.

use std::sync::Arc;

use egui::text::LayoutJob;
use egui::{Color32, Galley, Pos2, Vec2};

use crate::model::text_runs::{RunStyle, TextRun};
use crate::model::ListType;
use crate::text_layout::{run_text_format, tagged_lines, TextStyleParams};
use crate::text_path_geometry::{point_and_tangent_at, polyline_length};

/// One glyph's already-laid-out `Galley`, positioned and rotated to sit on
/// the path. `pos` is the galley's top-left corner (egui's `TextShape`
/// rotates about its own `pos`, so placing that corner directly on the path
/// and rotating by `angle` orients the glyph tangent to it — see
/// `canvas.rs`'s `draw_rotated_galley` for the same convention applied to
/// whole-layer rotation).
pub struct PathGlyph {
    pub galley: Arc<Galley>,
    pub pos: Pos2,
    pub angle: f32,
}

/// Lays out `content`'s first line along `path_points`, one glyph at a time.
/// `start` (`0.0..=1.0`) is where the text begins as a fraction of the
/// path's total length (for a closed path this wraps; for an open one it's
/// clamped by `point_and_tangent_at`); `style.align` shifts that start point
/// so `Center`/`Right` measure from the middle/end of the laid-out text
/// instead of its start. `flip` reverses the direction of travel. `offset`
/// shifts every glyph perpendicular to its own tangent (already zoom-scaled,
/// document-outward-positive). Returns an empty `Vec` if the path is
/// degenerate (fewer than 2 points, or ~zero length) or `content`'s first
/// line is empty.
#[allow(clippy::too_many_arguments)]
pub fn layout_glyphs_on_path(
    ctx: &egui::Context,
    content: &str,
    runs: &[TextRun],
    base: &RunStyle,
    style: &TextStyleParams,
    zoom: f32,
    layer_color: Color32,
    path_points: &[Pos2],
    closed: bool,
    offset: f32,
    start: f32,
    flip: bool,
) -> Vec<PathGlyph> {
    if polyline_length(path_points, closed) < 1.0 {
        return Vec::new();
    }
    let first_line = content.split('\n').next().unwrap_or("");
    if first_line.is_empty() {
        return Vec::new();
    }
    let Some(tagged) = tagged_lines(first_line, runs, style.transform, ListType::None, 1).into_iter().next() else {
        return Vec::new();
    };
    if tagged.is_empty() {
        return Vec::new();
    }

    let points: Vec<Pos2> = if flip { path_points.iter().rev().copied().collect() } else { path_points.to_vec() };
    let total = polyline_length(&points, closed);

    let letter_spacing = style.letter_spacing * zoom;
    let measured: Vec<(Arc<Galley>, f32)> = tagged
        .iter()
        .map(|&(ch, run_idx)| {
            let run_style = run_idx.and_then(|i| runs.get(i)).map_or(base, |r| &r.style);
            let format = run_text_format(ctx, run_style, style, zoom, layer_color);
            let galley = ctx.fonts_mut(|f| f.layout_job(LayoutJob::single_section(ch.to_string(), format)));
            let width = galley.rect.width();
            (galley, width)
        })
        .collect();
    let text_len: f32 = measured.iter().map(|(_, w)| w).sum::<f32>() + letter_spacing * (measured.len().max(1) - 1) as f32;

    let start_distance = start * total
        - match style.align {
            crate::model::TextAlign::Center => text_len / 2.0,
            crate::model::TextAlign::Right => text_len,
            crate::model::TextAlign::Left | crate::model::TextAlign::Justify => 0.0,
        };

    let mut distance = start_distance;
    let mut glyphs = Vec::with_capacity(measured.len());
    for (galley, width) in measured {
        if let Some((point, angle)) = point_and_tangent_at(&points, closed, distance) {
            let normal = Vec2::new(-angle.sin(), angle.cos());
            glyphs.push(PathGlyph { galley, pos: point + normal * offset, angle });
        }
        distance += width + letter_spacing;
    }
    glyphs
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{TextAlign, TextFont, TextTransform};

    fn style() -> TextStyleParams {
        TextStyleParams {
            font: TextFont::Proportional,
            font_size: 16.0,
            align: TextAlign::Left,
            letter_spacing: 0.0,
            line_height: None,
            italic: false,
            underline: false,
            strikethrough: false,
            transform: TextTransform::None,
            list: ListType::None,
            list_start: 1,
        }
    }

    fn base_run_style() -> RunStyle {
        RunStyle {
            font: TextFont::Proportional,
            font_size: 16.0,
            color: Some(Color32::BLACK),
            bold: false,
            italic: false,
            underline: false,
            strikethrough: false,
        }
    }

    /// `egui::Context::default()` has no fonts loaded until its first
    /// `run()` — needed before any test can call `fonts_mut`/`layout_job`.
    fn ready_ctx() -> egui::Context {
        let ctx = egui::Context::default();
        let mut output = ctx.run_ui(Default::default(), |_| {});
        output.textures_delta.clear();
        ctx
    }

    #[test]
    fn empty_path_produces_no_glyphs() {
        let ctx = ready_ctx();
        let glyphs = layout_glyphs_on_path(&ctx, "Hi", &[], &base_run_style(), &style(), 1.0, Color32::BLACK, &[], false, 0.0, 0.0, false);
        assert!(glyphs.is_empty());
    }

    #[test]
    fn one_glyph_per_character_on_a_long_enough_path() {
        let ctx = ready_ctx();
        let path = vec![Pos2::new(0.0, 0.0), Pos2::new(1000.0, 0.0)];
        let glyphs = layout_glyphs_on_path(&ctx, "Hi", &[], &base_run_style(), &style(), 1.0, Color32::BLACK, &path, false, 0.0, 0.0, false);
        assert_eq!(glyphs.len(), 2);
    }

    #[test]
    fn only_first_line_is_placed() {
        let ctx = ready_ctx();
        let path = vec![Pos2::new(0.0, 0.0), Pos2::new(1000.0, 0.0)];
        let glyphs = layout_glyphs_on_path(&ctx, "Hi\nBye", &[], &base_run_style(), &style(), 1.0, Color32::BLACK, &path, false, 0.0, 0.0, false);
        assert_eq!(glyphs.len(), 2);
    }
}
