//! Canvas-side rendering for a `Text` layer whose `path_attachment.mode` is
//! `TextPathMode::AreaInside` ("Area Type"): turns `text_area_wrap`'s
//! backend-agnostic wrapping decision into `egui::Galley`s. The wrapping
//! itself (where each line breaks, its `x`/`y`) lives in `text_area_wrap.rs`,
//! shared with `export.rs`'s `ab_glyph`-based rendering, so the two can't
//! disagree on where a line breaks or sits.
//!
//! `list`/`paragraph_spacing` are deliberately not honored (only `transform`
//! and per-character `runs` are) — see `text_area_wrap.rs`'s module doc.
//! All spatial inputs/outputs are in the same already-zoom-scaled space as
//! `text_on_path_layout.rs`.

use std::sync::Arc;

use egui::text::LayoutJob;
use egui::{Color32, Galley, Pos2};

use crate::model::text_runs::{RunStyle, TextRun};
use crate::text_area_wrap::wrap_in_area;
use crate::text_layout::{run_text_format, TextStyleParams};

/// One wrapped line's already-laid-out `Galley` and top-left position.
pub struct AreaLine {
    pub galley: Arc<Galley>,
    pub pos: Pos2,
}

/// Word-wraps `content` to fit inside the closed outline `target_points`,
/// inset by `padding` on every side. Returns one `AreaLine` per visual line;
/// wrapping stops (remaining text is dropped, like `TextResize::Fixed`
/// clipping) once there's no more vertical room inside the shape's bounds.
#[allow(clippy::too_many_arguments)]
pub fn layout_text_in_area(
    ctx: &egui::Context,
    content: &str,
    runs: &[TextRun],
    base: &RunStyle,
    style: &TextStyleParams,
    zoom: f32,
    layer_color: Color32,
    target_points: &[Pos2],
    padding: f32,
) -> Vec<AreaLine> {
    let line_height = style
        .line_height
        .map(|h| h * zoom)
        .unwrap_or_else(|| build_run_job(ctx, &[(' ', None)], runs, base, style, zoom, layer_color).rect.height())
        .max(1.0);
    let measure = |chars: &[(char, Option<usize>)]| build_run_job(ctx, chars, runs, base, style, zoom, layer_color).rect.width();

    wrap_in_area(content, runs, style.transform, &measure, line_height, style.align, target_points, padding)
        .into_iter()
        .map(|row| AreaLine { galley: build_run_job(ctx, &row.chars, runs, base, style, zoom, layer_color), pos: Pos2::new(row.x, row.y) })
        .collect()
}

/// Builds one `Galley` for a contiguous stretch of tagged chars, splitting
/// into one `LayoutJob` section per contiguous same-run stretch — same
/// "run-aware line" construction `text_layout::layout_paragraphs_rich` uses
/// for a whole paragraph, just applied to one word/line-worth of chars here.
fn build_run_job(
    ctx: &egui::Context,
    chars: &[(char, Option<usize>)],
    runs: &[TextRun],
    base: &RunStyle,
    style: &TextStyleParams,
    zoom: f32,
    layer_color: Color32,
) -> Arc<Galley> {
    let mut job = LayoutJob::default();
    if chars.is_empty() {
        job.append("", 0.0, run_text_format(ctx, base, style, zoom, layer_color));
    } else {
        let mut start = 0;
        while start < chars.len() {
            let key = chars[start].1;
            let mut end = start + 1;
            while end < chars.len() && chars[end].1 == key {
                end += 1;
            }
            let text: String = chars[start..end].iter().map(|&(c, _)| c).collect();
            let run_style = key.and_then(|i| runs.get(i)).map_or(base, |r| &r.style);
            let format = run_text_format(ctx, run_style, style, zoom, layer_color);
            job.append(&text, 0.0, format);
            start = end;
        }
    }
    ctx.fonts_mut(|f| f.layout_job(job))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{ListType, TextAlign, TextFont, TextTransform};

    fn style() -> TextStyleParams {
        TextStyleParams {
            font: TextFont::Proportional,
            font_size: 16.0,
            align: TextAlign::Left,
            letter_spacing: 0.0,
            line_height: Some(20.0),
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
    fn wraps_into_multiple_lines_inside_a_tall_narrow_rect() {
        let ctx = ready_ctx();
        let rect = vec![Pos2::new(0.0, 0.0), Pos2::new(60.0, 0.0), Pos2::new(60.0, 200.0), Pos2::new(0.0, 200.0)];
        let lines = layout_text_in_area(&ctx, "one two three four five", &[], &base_run_style(), &style(), 1.0, Color32::BLACK, &rect, 0.0);
        assert!(lines.len() > 1, "expected wrapping into multiple lines, got {}", lines.len());
    }

    #[test]
    fn degenerate_shape_produces_no_lines() {
        let ctx = ready_ctx();
        let lines = layout_text_in_area(&ctx, "hello", &[], &base_run_style(), &style(), 1.0, Color32::BLACK, &[], 0.0);
        assert!(lines.is_empty());
    }
}
