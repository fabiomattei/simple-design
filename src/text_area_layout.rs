//! Line-wrapping for a `Text` layer whose `path_attachment.mode` is
//! `TextPathMode::AreaInside` ("Area Type"): each line is fit to whatever
//! horizontal span the target shape's outline (from `text_path_geometry`)
//! actually offers at that height, rather than a fixed rectangle width.
//!
//! `list`/`paragraph_spacing` are deliberately not honored here (only
//! `transform` and per-character `runs` are) — list bullets and extra
//! paragraph gaps assume a fixed line start column/rectangle, which an
//! area shape doesn't have one of; keeping area-type to plain wrapped
//! paragraphs avoids that mismatch rather than fudging it. All spatial
//! inputs/outputs are in the same already-zoom-scaled space as
//! `text_on_path_layout.rs`.

use std::sync::Arc;

use egui::text::LayoutJob;
use egui::{Color32, Galley, Pos2};

use crate::model::text_runs::{RunStyle, TextRun};
use crate::model::{ListType, TextAlign};
use crate::text_layout::{run_text_format, tagged_lines, TextStyleParams};
use crate::text_path_geometry::horizontal_spans_at_y;

/// One wrapped line's already-laid-out `Galley` and top-left position.
pub struct AreaLine {
    pub galley: Arc<Galley>,
    pub pos: Pos2,
}

/// Word-wraps `content` to fit inside the closed outline `target_points`,
/// inset by `padding` on every side. Returns one `AreaLine` per visual line,
/// top-aligned starting just inside the shape's topmost point; wrapping
/// stops (remaining text is dropped, like `TextResize::Fixed` clipping)
/// once there's no more vertical room inside the shape's bounds. A height
/// with no usable span at all (e.g. between a star's points) is skipped
/// rather than breaking the line count.
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
    if target_points.len() < 3 {
        return Vec::new();
    }
    let min_y = target_points.iter().map(|p| p.y).fold(f32::INFINITY, f32::min);
    let max_y = target_points.iter().map(|p| p.y).fold(f32::NEG_INFINITY, f32::max);
    if !(min_y.is_finite() && max_y.is_finite()) || max_y <= min_y {
        return Vec::new();
    }

    let line_height = style
        .line_height
        .map(|h| h * zoom)
        .unwrap_or_else(|| build_run_job(ctx, &[(' ', None)], runs, base, style, zoom, layer_color).rect.height())
        .max(1.0);

    let mut queue: Vec<(Vec<(char, Option<usize>)>, bool)> = Vec::new();
    for (paragraph_index, paragraph) in tagged_lines(content, runs, style.transform, ListType::None, 1).into_iter().enumerate() {
        let tokens = tokenize(&paragraph);
        if tokens.is_empty() {
            queue.push((Vec::new(), paragraph_index > 0));
        } else {
            for (i, token) in tokens.into_iter().enumerate() {
                queue.push((token, i == 0 && paragraph_index > 0));
            }
        }
    }
    let mut queue = queue.into_iter().peekable();

    let mut lines = Vec::new();
    let mut current: Vec<(char, Option<usize>)> = Vec::new();
    let mut current_width = 0.0f32;
    let mut y = min_y + padding;

    while y + line_height <= max_y {
        let Some((x0, x1)) = widest_span(target_points, y + line_height / 2.0, padding) else {
            y += line_height;
            continue;
        };
        let avail = x1 - x0;

        while let Some((token, new_paragraph)) = queue.peek() {
            if *new_paragraph && !current.is_empty() {
                break;
            }
            let token_width = build_run_job(ctx, token, runs, base, style, zoom, layer_color).rect.width();
            if current_width + token_width > avail && !current.is_empty() {
                break;
            }
            current_width += token_width;
            current.extend_from_slice(token);
            queue.next();
        }

        if !current.is_empty() {
            push_line(&mut lines, ctx, &current, runs, base, style, zoom, layer_color, x0, avail, y);
            current.clear();
            current_width = 0.0;
        } else if let Some((token, _)) = queue.peek() {
            // A single token wider than this line's whole span: place it
            // anyway (it overflows) rather than looping forever on it.
            let token = token.clone();
            queue.next();
            push_line(&mut lines, ctx, &token, runs, base, style, zoom, layer_color, x0, avail, y);
        }

        y += line_height;
        if queue.peek().is_none() {
            break;
        }
    }
    lines
}

fn widest_span(points: &[Pos2], y: f32, padding: f32) -> Option<(f32, f32)> {
    horizontal_spans_at_y(points, y)
        .into_iter()
        .map(|(a, b)| (a + padding, b - padding))
        .filter(|(a, b)| b > a)
        .max_by(|a, b| (a.1 - a.0).partial_cmp(&(b.1 - b.0)).unwrap_or(std::cmp::Ordering::Equal))
}

/// Splits one paragraph's tagged chars into word tokens, each a word plus
/// its immediately following whitespace run (so concatenating tokens back
/// to back exactly reconstructs the paragraph) — the standard "break
/// opportunities sit at whitespace, attached to the preceding word" tokenization.
fn tokenize(paragraph: &[(char, Option<usize>)]) -> Vec<Vec<(char, Option<usize>)>> {
    let mut tokens = Vec::new();
    let mut i = 0;
    while i < paragraph.len() {
        let start = i;
        while i < paragraph.len() && !paragraph[i].0.is_whitespace() {
            i += 1;
        }
        while i < paragraph.len() && paragraph[i].0.is_whitespace() {
            i += 1;
        }
        tokens.push(paragraph[start..i].to_vec());
    }
    tokens
}

#[allow(clippy::too_many_arguments)]
fn push_line(
    lines: &mut Vec<AreaLine>,
    ctx: &egui::Context,
    chars: &[(char, Option<usize>)],
    runs: &[TextRun],
    base: &RunStyle,
    style: &TextStyleParams,
    zoom: f32,
    layer_color: Color32,
    x0: f32,
    avail: f32,
    y: f32,
) {
    let galley = build_run_job(ctx, chars, runs, base, style, zoom, layer_color);
    let leftover = (avail - galley.rect.width()).max(0.0);
    let x = match style.align {
        TextAlign::Center => x0 + leftover / 2.0,
        TextAlign::Right => x0 + leftover,
        TextAlign::Left | TextAlign::Justify => x0,
    };
    lines.push(AreaLine { galley, pos: Pos2::new(x, y) });
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
    use crate::model::{TextFont, TextTransform};

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
