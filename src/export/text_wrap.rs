//! Greedy word-wrap for exported text, plain and run-tagged.

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
    measure: &crate::text_area_wrap::MeasureTaggedChars<'_>,
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

#[cfg(test)]
mod tests {
    use super::*;

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
}
