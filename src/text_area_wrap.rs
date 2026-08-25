//! Backend-agnostic word-wrap for `TextPathMode::AreaInside` text: decides
//! where each line breaks and where it sits (`x`, `y`), by fitting words
//! greedily against the target outline's own horizontal span at each line's
//! height (`text_path_geometry::horizontal_spans_at_y`). Shared by the
//! canvas (`text_area_layout.rs`, which turns each row into an
//! `egui::Galley`) and the exporter (`export.rs`, which rasterizes each
//! row's glyphs via `ab_glyph`) so the two can't disagree on where a line
//! breaks or sits — only how "these tagged chars at this position" become
//! pixels differs per caller.
//!
//! `list`/`paragraph_spacing` aren't supported here — see
//! `text_area_layout.rs`'s module doc for why (both assume a fixed line
//! start column/rectangle an area shape doesn't have one of).

use egui::Pos2;

use crate::model::text_runs::TextRun;
use crate::model::{ListType, TextAlign, TextTransform};
use crate::text_path_geometry::horizontal_spans_at_y;

/// One wrapped, run-tagged line (`chars`, same content-relative tagging
/// convention as `text_layout::tagged_lines`): `x` is its left edge after
/// alignment, `y` its *top* (not baseline — callers add their own font's
/// ascent to place glyphs).
pub struct WrappedRow {
    pub chars: Vec<(char, Option<usize>)>,
    pub x: f32,
    pub y: f32,
}

/// Wraps `content` inside the closed outline `points` (inset by `padding` on
/// every side), stacking rows `line_height` apart starting at the shape's
/// topmost point. `measure` returns a run-tagged char slice's width in
/// whatever units/backend the caller renders with (an `egui::Galley`'s
/// width, or an `ab_glyph`-measured advance sum) — called repeatedly during
/// wrapping, so callers that build a real object to measure (a `Galley`)
/// should keep it cheap. Running out of vertical room inside `points` drops
/// the remaining text (like `TextResize::Fixed` clipping) rather than
/// overflowing it; a height with no usable span at all (e.g. between a
/// star's points) is skipped rather than breaking the line count.
#[allow(clippy::too_many_arguments)]
pub fn wrap_in_area(
    content: &str,
    runs: &[TextRun],
    transform: TextTransform,
    measure: &dyn Fn(&[(char, Option<usize>)]) -> f32,
    line_height: f32,
    align: TextAlign,
    points: &[Pos2],
    padding: f32,
) -> Vec<WrappedRow> {
    if points.len() < 3 || line_height <= 0.0 {
        return Vec::new();
    }
    let min_y = points.iter().map(|p| p.y).fold(f32::INFINITY, f32::min);
    let max_y = points.iter().map(|p| p.y).fold(f32::NEG_INFINITY, f32::max);
    if !(min_y.is_finite() && max_y.is_finite()) || max_y <= min_y {
        return Vec::new();
    }

    let mut queue: Vec<(Vec<(char, Option<usize>)>, bool)> = Vec::new();
    for (paragraph_index, paragraph) in crate::text_layout::tagged_lines(content, runs, transform, ListType::None, 1).into_iter().enumerate() {
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

    let mut rows = Vec::new();
    let mut current: Vec<(char, Option<usize>)> = Vec::new();
    let mut current_width = 0.0f32;
    let mut y = min_y + padding;

    while y + line_height <= max_y {
        let Some((x0, x1)) = widest_span(points, y + line_height / 2.0, padding) else {
            y += line_height;
            continue;
        };
        let avail = x1 - x0;

        while let Some((token, new_paragraph)) = queue.peek() {
            if *new_paragraph && !current.is_empty() {
                break;
            }
            let token_width = measure(token);
            if current_width + token_width > avail && !current.is_empty() {
                break;
            }
            current_width += token_width;
            current.extend_from_slice(token);
            queue.next();
        }

        if !current.is_empty() {
            push_row(&mut rows, std::mem::take(&mut current), measure, align, x0, avail, y);
            current_width = 0.0;
        } else if let Some((token, _)) = queue.peek() {
            // A single token wider than this line's whole span: place it
            // anyway (it overflows) rather than looping forever on it.
            let token = token.clone();
            queue.next();
            push_row(&mut rows, token, measure, align, x0, avail, y);
        }

        y += line_height;
        if queue.peek().is_none() {
            break;
        }
    }
    rows
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

fn push_row(
    rows: &mut Vec<WrappedRow>,
    chars: Vec<(char, Option<usize>)>,
    measure: &dyn Fn(&[(char, Option<usize>)]) -> f32,
    align: TextAlign,
    x0: f32,
    avail: f32,
    y: f32,
) {
    let leftover = (avail - measure(&chars)).max(0.0);
    let x = match align {
        TextAlign::Center => x0 + leftover / 2.0,
        TextAlign::Right => x0 + leftover,
        TextAlign::Left | TextAlign::Justify => x0,
    };
    rows.push(WrappedRow { chars, x, y });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixed_width_measure(chars: &[(char, Option<usize>)]) -> f32 {
        chars.len() as f32 * 10.0
    }

    #[test]
    fn wraps_into_multiple_rows_inside_a_tall_narrow_rect() {
        let rect = vec![Pos2::new(0.0, 0.0), Pos2::new(60.0, 0.0), Pos2::new(60.0, 200.0), Pos2::new(0.0, 200.0)];
        let rows = wrap_in_area("one two three four five", &[], TextTransform::None, &fixed_width_measure, 20.0, TextAlign::Left, &rect, 0.0);
        assert!(rows.len() > 1, "expected wrapping into multiple rows, got {}", rows.len());
    }

    #[test]
    fn degenerate_shape_produces_no_rows() {
        let rows = wrap_in_area("hello", &[], TextTransform::None, &fixed_width_measure, 20.0, TextAlign::Left, &[], 0.0);
        assert!(rows.is_empty());
    }

    /// The per-row available width genuinely comes from the target shape's
    /// own span at that height (`horizontal_spans_at_y`), not a fixed
    /// constant — the same content's first row fits far fewer words when
    /// the shape offering it is narrow than when it's wide. This is the
    /// property that lets wrapping hug an irregular contour (a star, a
    /// triangle) line by line instead of falling back to one inscribed
    /// rectangle for the whole shape.
    #[test]
    fn first_row_word_count_grows_with_the_shapes_width_at_that_height() {
        let words = "aa aa aa aa aa aa aa aa aa aa";
        let word_count = |points: &[Pos2]| -> usize {
            let rows = wrap_in_area(words, &[], TextTransform::None, &fixed_width_measure, 20.0, TextAlign::Left, points, 0.0);
            rows.first().map_or(0, |r| r.chars.iter().filter(|(c, _)| !c.is_whitespace()).count() / 2)
        };

        let narrow = vec![Pos2::new(0.0, 0.0), Pos2::new(25.0, 0.0), Pos2::new(25.0, 100.0), Pos2::new(0.0, 100.0)];
        let wide = vec![Pos2::new(0.0, 0.0), Pos2::new(250.0, 0.0), Pos2::new(250.0, 100.0), Pos2::new(0.0, 100.0)];

        let narrow_count = word_count(&narrow);
        let wide_count = word_count(&wide);
        assert!(
            wide_count > narrow_count,
            "expected the 250px-wide shape's first row (got {wide_count} words) to fit more than the 25px-wide shape's (got {narrow_count} words)"
        );
    }

    #[test]
    fn rows_stack_top_to_bottom_by_line_height() {
        let rect = vec![Pos2::new(0.0, 0.0), Pos2::new(200.0, 0.0), Pos2::new(200.0, 100.0), Pos2::new(0.0, 100.0)];
        let rows = wrap_in_area("a\nb\nc", &[], TextTransform::None, &fixed_width_measure, 20.0, TextAlign::Left, &rect, 0.0);
        assert_eq!(rows.len(), 3);
        assert!((rows[1].y - rows[0].y - 20.0).abs() < 1e-3);
        assert!((rows[2].y - rows[1].y - 20.0).abs() < 1e-3);
    }
}
