//! Per-character style overrides ("rich text") for a `Text` layer's
//! `content` — see `LayerKind::Text::runs`'s doc comment in `layer.rs` for
//! the invariant this module maintains: `runs` is either empty (uniform
//! style, the layer's own scalar fields govern) or non-empty and spans the
//! *entire* content (`runs.iter().map(|r| r.len).sum() == content.chars().count()`).
//!
//! `TextRun::len` is a **char** count, not a byte count — chosen to match
//! `egui::text::CCursor`'s indexing, since that's what the in-place
//! editor's selection API speaks natively (`canvas.rs`'s rich-text editing
//! overlay reads/writes char-index `CCursorRange`s). A run carries no text
//! of its own, only "this many of the upcoming characters share this
//! style" — so growing/shrinking a run never needs to know *which part* of
//! its span an edit touched, only by how much.
//!
//! Every function here is a pure transformation over `(content, runs)` —
//! no egui/UI dependency — so the tricky part of this feature (keeping the
//! run list structurally valid through arbitrary typing/deleting/pasting
//! and range-based formatting) is fully unit-testable in isolation. Never
//! mutate a `Vec<TextRun>` outside of `splice`/`apply`/`mixed_or`.
use std::ops::Range;

use egui::Color32;
use serde::{Deserialize, Serialize};

use crate::model::layer::TextFont;

/// Every character-level style property a run can override. Paragraph-
/// level properties (alignment, resize, spacing, transform, list) are
/// deliberately not here — see `layer.rs`'s `LayerKind::Text::runs` doc.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct RunStyle {
    pub font: TextFont,
    pub font_size: f32,
    /// `None` means inherit `Layer::style.fill`, matching how the
    /// uniform-style (`runs` empty) case already sources its color.
    pub color: Option<Color32>,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub strikethrough: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct TextRun {
    pub len: usize,
    pub style: RunStyle,
}

/// The single entry point for adjusting `runs` after *any* content edit
/// (typing, deleting a selection, pasting) — `change_start`/`chars_removed`/
/// `chars_inserted` describe the edit purely as char counts, typically
/// obtained by diffing the old and new buffer via common-prefix/suffix
/// trimming (see `canvas.rs`'s editing overlay). No-op if `runs` is empty
/// (uniform style hasn't been touched yet, nothing to keep in sync).
///
/// Removes `chars_removed` chars at `change_start` first (shrinking/
/// splitting/dropping runs as needed), then inserts `chars_inserted` chars
/// by growing whichever run now ends at-or-after `change_start` (i.e. the
/// character immediately to the left inherits its style onto the new
/// text — standard "continue the current formatting" behavior); inserting
/// at position 0 grows the first run instead (nothing to inherit from on
/// the left), and `base` is only used if `runs` becomes/starts empty.
pub fn splice(runs: &mut Vec<TextRun>, change_start: usize, chars_removed: usize, chars_inserted: usize, base: &RunStyle) {
    if runs.is_empty() {
        return;
    }
    if chars_removed > 0 {
        remove_range(runs, change_start..change_start + chars_removed);
    }
    if chars_inserted > 0 {
        insert_at(runs, change_start, chars_inserted, base);
    }
    coalesce(runs);
}

/// Applies `edit` to every run's `RunStyle` that falls within `char_range`
/// — the "make this selection bold" entry point. Lazily materializes
/// `runs` as one run spanning all of `content` (styled `base.clone()`) if
/// currently empty, so callers never need to special-case "not rich yet".
/// Splits any run straddling the range's boundaries first so the edit
/// never bleeds outside `char_range`, then coalesces adjacent runs left
/// with equal styles back together afterward.
pub fn apply<F: Fn(&mut RunStyle)>(content: &str, runs: &mut Vec<TextRun>, base: &RunStyle, char_range: Range<usize>, edit: F) {
    if char_range.start >= char_range.end {
        return;
    }
    if runs.is_empty() {
        let total = content.chars().count();
        if total == 0 {
            return;
        }
        runs.push(TextRun { len: total, style: base.clone() });
    }
    split_at(runs, char_range.start);
    split_at(runs, char_range.end);

    let mut pos = 0usize;
    for run in runs.iter_mut() {
        let run_start = pos;
        let run_end = pos + run.len;
        pos = run_end;
        if run_start >= char_range.start && run_end <= char_range.end {
            edit(&mut run.style);
        }
    }
    coalesce(runs);
}

/// Reads a single property across `char_range`: `Some(value)` if every run
/// touching the range (even partially) agrees on it, `None` if it's mixed
/// or the range is empty. Callers treat an empty `runs` (uniform style) as
/// trivially non-mixed via the layer's own scalar field — this only
/// queries *into* an already-rich `runs` list.
pub fn mixed_or<T: PartialEq + Clone>(runs: &[TextRun], char_range: Range<usize>, field: impl Fn(&RunStyle) -> T) -> Option<T> {
    if runs.is_empty() || char_range.start >= char_range.end {
        return None;
    }
    let mut pos = 0usize;
    let mut result: Option<T> = None;
    for run in runs {
        let run_start = pos;
        let run_end = pos + run.len;
        pos = run_end;
        if run_end <= char_range.start || run_start >= char_range.end {
            continue;
        }
        let value = field(&run.style);
        match &result {
            None => result = Some(value),
            Some(existing) if *existing != value => return None,
            Some(_) => {}
        }
    }
    result
}

/// Diffs `old` against `new` via common-prefix/common-suffix trimming and
/// returns `(change_start, chars_removed, chars_inserted)` in char
/// coordinates, ready to hand to `splice`. Not a general/minimal diff —
/// cheap and correct for the actual editing gestures a text field
/// produces (typing, deleting, pasting, replacing a selection), which is
/// all `splice`'s callers need.
pub fn diff_chars(old: &str, new: &str) -> (usize, usize, usize) {
    let old_chars: Vec<char> = old.chars().collect();
    let new_chars: Vec<char> = new.chars().collect();
    let max_common = old_chars.len().min(new_chars.len());

    let mut prefix = 0;
    while prefix < max_common && old_chars[prefix] == new_chars[prefix] {
        prefix += 1;
    }
    let mut suffix = 0;
    while suffix < max_common - prefix && old_chars[old_chars.len() - 1 - suffix] == new_chars[new_chars.len() - 1 - suffix] {
        suffix += 1;
    }

    let chars_removed = old_chars.len() - prefix - suffix;
    let chars_inserted = new_chars.len() - prefix - suffix;
    (prefix, chars_removed, chars_inserted)
}

fn remove_range(runs: &mut Vec<TextRun>, range: Range<usize>) {
    if range.start >= range.end {
        return;
    }
    let mut pos = 0usize;
    runs.retain_mut(|run| {
        let run_start = pos;
        let run_len = run.len;
        let run_end = run_start + run_len;
        pos = run_end;

        let overlap_start = range.start.max(run_start);
        let overlap_end = range.end.min(run_end);
        if overlap_start < overlap_end {
            run.len -= overlap_end - overlap_start;
        }
        run.len > 0
    });
}

fn insert_at(runs: &mut Vec<TextRun>, change_start: usize, additional_len: usize, base: &RunStyle) {
    if runs.is_empty() {
        runs.push(TextRun { len: additional_len, style: base.clone() });
        return;
    }
    if change_start == 0 {
        runs[0].len += additional_len;
        return;
    }
    let mut pos = 0usize;
    for run in runs.iter_mut() {
        pos += run.len;
        if pos >= change_start {
            run.len += additional_len;
            return;
        }
    }
    // `change_start` was beyond the end of every run (shouldn't happen for
    // a valid diff against the current content, but fail soft rather than
    // silently drop the inserted text).
    runs.push(TextRun { len: additional_len, style: base.clone() });
}

/// Splits the run spanning `at` into two runs at that boundary (same
/// style on both halves); a no-op if `at` already falls on a run boundary
/// (including 0 and the total length).
fn split_at(runs: &mut Vec<TextRun>, at: usize) {
    if at == 0 {
        return;
    }
    let mut pos = 0usize;
    for i in 0..runs.len() {
        let run_start = pos;
        let run_len = runs[i].len;
        let run_end = run_start + run_len;
        pos = run_end;
        if at == run_end {
            return;
        }
        if at > run_start && at < run_end {
            let left_len = at - run_start;
            let right_len = run_len - left_len;
            let style = runs[i].style.clone();
            runs[i].len = left_len;
            runs.insert(i + 1, TextRun { len: right_len, style });
            return;
        }
    }
}

fn coalesce(runs: &mut Vec<TextRun>) {
    runs.retain(|r| r.len > 0);
    let mut i = 0;
    while i + 1 < runs.len() {
        if runs[i].style == runs[i + 1].style {
            let extra = runs.remove(i + 1);
            runs[i].len += extra.len;
        } else {
            i += 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn style(tag: u8) -> RunStyle {
        RunStyle {
            font: TextFont::Proportional,
            font_size: 16.0,
            color: Some(Color32::from_gray(tag)),
            bold: false,
            italic: false,
            underline: false,
            strikethrough: false,
        }
    }

    fn run(len: usize, tag: u8) -> TextRun {
        TextRun { len, style: style(tag) }
    }

    #[test]
    fn splice_is_noop_on_empty_runs() {
        let mut runs = Vec::new();
        splice(&mut runs, 0, 0, 3, &style(1));
        assert!(runs.is_empty(), "uniform-style layers shouldn't gain runs just from typing");
    }

    #[test]
    fn typing_at_the_end_extends_the_last_run() {
        // "abc" as one run (tag 1); type "d" at the end.
        let mut runs = vec![run(3, 1)];
        splice(&mut runs, 3, 0, 1, &style(9));
        assert_eq!(runs, vec![run(4, 1)], "new text at the end should inherit the last run's style");
    }

    #[test]
    fn typing_at_the_start_extends_the_first_run() {
        let mut runs = vec![run(3, 1)];
        splice(&mut runs, 0, 0, 2, &style(9));
        assert_eq!(runs, vec![run(5, 1)], "nothing to the left at position 0 — inherit the first run instead");
    }

    #[test]
    fn typing_in_the_middle_of_a_run_grows_it_without_splitting() {
        let mut runs = vec![run(3, 1), run(3, 2)];
        // Insert 2 chars after char index 1 (inside the first run).
        splice(&mut runs, 1, 0, 2, &style(9));
        assert_eq!(runs, vec![run(5, 1), run(3, 2)], "a run has no internal position, so growing it is exact regardless of where inside it the insert landed");
    }

    #[test]
    fn typing_right_at_a_run_boundary_extends_the_left_run() {
        let mut runs = vec![run(3, 1), run(3, 2)];
        splice(&mut runs, 3, 0, 2, &style(9));
        assert_eq!(runs, vec![run(5, 1), run(3, 2)], "typing right after run 1 continues run 1's style");
    }

    #[test]
    fn deleting_entirely_within_one_run_shrinks_it() {
        let mut runs = vec![run(5, 1), run(3, 2)];
        splice(&mut runs, 1, 2, 0, &style(9));
        assert_eq!(runs, vec![run(3, 1), run(3, 2)]);
    }

    #[test]
    fn deleting_an_entire_run_removes_it() {
        let mut runs = vec![run(3, 1), run(2, 2), run(3, 3)];
        splice(&mut runs, 3, 2, 0, &style(9));
        assert_eq!(runs, vec![run(3, 1), run(3, 3)]);
    }

    #[test]
    fn deleting_across_a_run_boundary_shrinks_both_sides() {
        let mut runs = vec![run(4, 1), run(4, 2)];
        // Remove chars [2, 6): last 2 of run 1, first 2 of run 2.
        splice(&mut runs, 2, 4, 0, &style(9));
        assert_eq!(runs, vec![run(2, 1), run(2, 2)]);
    }

    #[test]
    fn pasting_replaces_a_whole_run_and_inherits_the_left_neighbor() {
        let mut runs = vec![run(3, 1), run(3, 2), run(3, 3)];
        // Replace all of run 2 (chars [3,6)) with 5 new chars.
        splice(&mut runs, 3, 3, 5, &style(9));
        assert_eq!(runs, vec![run(8, 1), run(3, 3)], "replacement text at a former run's start inherits the preceding run's style");
    }

    #[test]
    fn deleting_everything_leaves_an_empty_run_list() {
        let mut runs = vec![run(3, 1), run(3, 2)];
        splice(&mut runs, 0, 6, 0, &style(9));
        assert!(runs.is_empty());
    }

    #[test]
    fn apply_materializes_runs_from_empty_then_edits_the_range() {
        let mut runs = Vec::new();
        let base = style(1);
        apply("Hello world", &mut runs, &base, 6..11, |s| s.bold = true);
        assert_eq!(runs.len(), 2);
        assert_eq!(runs[0].len, 6);
        assert!(!runs[0].style.bold);
        assert_eq!(runs[1].len, 5);
        assert!(runs[1].style.bold);
    }

    #[test]
    fn apply_splits_a_run_straddling_the_range_boundary() {
        let mut runs = vec![run(11, 1)]; // "Hello world" all one style
        apply("Hello world", &mut runs, &style(1), 6..11, |s| s.italic = true);
        assert_eq!(runs.len(), 2);
        assert_eq!(runs[0], run(6, 1));
        assert!(runs[1].style.italic);
        assert_eq!(runs[1].len, 5);
    }

    #[test]
    fn apply_overlapping_bold_then_italic_on_different_subranges() {
        let mut runs = Vec::new();
        let base = style(1);
        // "Hello world" — bold "Hello", italic "world".
        apply("Hello world", &mut runs, &base, 0..5, |s| s.bold = true);
        apply("Hello world", &mut runs, &base, 6..11, |s| s.italic = true);
        assert_eq!(runs.len(), 3, "bold Hello / plain space / italic world");
        assert!(runs[0].style.bold && !runs[0].style.italic);
        assert!(!runs[1].style.bold && !runs[1].style.italic);
        assert!(!runs[2].style.bold && runs[2].style.italic);
    }

    #[test]
    fn apply_coalesces_back_together_when_a_range_matches_its_neighbors() {
        let mut runs = vec![run(3, 1), run(3, 1), run(3, 1)];
        // Middle run already matches its neighbors' style/tag — applying a
        // no-op edit should coalesce all three back into one run.
        apply("aaabbbccc", &mut runs, &style(9), 3..6, |_| {});
        assert_eq!(runs, vec![run(9, 1)]);
    }

    #[test]
    fn apply_ignores_an_empty_range() {
        let mut runs = vec![run(6, 1)];
        apply("abcdef", &mut runs, &style(9), 3..3, |s| s.bold = true);
        assert_eq!(runs, vec![run(6, 1)], "a collapsed cursor (no selection) shouldn't materialize or edit anything");
    }

    #[test]
    fn mixed_or_reports_none_for_uniform_and_differing_selections() {
        let runs = vec![
            TextRun { len: 3, style: RunStyle { bold: true, ..style(1) } },
            TextRun { len: 3, style: RunStyle { bold: false, ..style(2) } },
        ];
        // Entirely inside the first (bold) run.
        assert_eq!(mixed_or(&runs, 0..3, |s| s.bold), Some(true));
        // Spans both runs — bold differs.
        assert_eq!(mixed_or(&runs, 2..4, |s| s.bold), None);
        // Entirely inside the second (non-bold) run.
        assert_eq!(mixed_or(&runs, 3..6, |s| s.bold), Some(false));
    }

    #[test]
    fn mixed_or_is_none_for_empty_runs_or_empty_range() {
        assert_eq!(mixed_or(&[], 0..3, |s: &RunStyle| s.bold), None);
        let runs = vec![run(6, 1)];
        assert_eq!(mixed_or(&runs, 3..3, |s| s.bold), None);
    }

    #[test]
    fn diff_chars_no_change() {
        assert_eq!(diff_chars("hello", "hello"), (5, 0, 0));
    }

    #[test]
    fn diff_chars_typed_at_the_end() {
        assert_eq!(diff_chars("hello", "hello!"), (5, 0, 1));
    }

    #[test]
    fn diff_chars_typed_at_the_start() {
        assert_eq!(diff_chars("world", "!world"), (0, 0, 1));
    }

    #[test]
    fn diff_chars_typed_in_the_middle() {
        assert_eq!(diff_chars("helloworld", "hello, world"), (5, 0, 2));
    }

    #[test]
    fn diff_chars_deleted_a_selection() {
        assert_eq!(diff_chars("hello world", "hello"), (5, 6, 0));
    }

    #[test]
    fn diff_chars_replaced_a_selection() {
        assert_eq!(diff_chars("hello world", "hello there"), (6, 5, 5));
    }

    #[test]
    fn diff_chars_cleared_everything() {
        assert_eq!(diff_chars("hello", ""), (0, 5, 0));
    }
}
