//! Canvas-side text layout helpers, shared by the interactive `Text` layer
//! renderer and by auto-resize measurement in `canvas.rs`. Deliberately not
//! shared with `export.rs`'s independent `ab_glyph` rasterizer (see
//! `CLAUDE.md`'s note on canvas vs. export being separate render paths) —
//! this module only ever produces `egui::Galley`s for on-screen use.
use std::sync::Arc;

use egui::text::LayoutJob;
use egui::{Align, Color32, FontId, Galley, Stroke, TextFormat, Vec2};

use crate::model::text_runs::{RunStyle, TextRun};
use crate::model::{ListType, TextAlign, TextFont, TextTransform};

/// Every `LayerKind::Text` field that affects layout, bundled so call sites
/// don't need a long parameter list.
pub struct TextStyleParams {
    pub font: TextFont,
    pub font_size: f32,
    pub align: TextAlign,
    pub letter_spacing: f32,
    pub line_height: Option<f32>,
    pub italic: bool,
    pub underline: bool,
    pub strikethrough: bool,
    pub transform: TextTransform,
    pub list: ListType,
    pub list_start: i32,
}

/// Applies the non-destructive display transform and, if `list != None`,
/// prepends a bullet/number prefix to every non-blank line — purely for
/// display; the stored `content` is never touched by this.
pub fn display_string(content: &str, transform: TextTransform, list: ListType, list_start: i32) -> String {
    let transformed = match transform {
        TextTransform::None => content.to_string(),
        TextTransform::Uppercase => content.to_uppercase(),
        TextTransform::Lowercase => content.to_lowercase(),
        TextTransform::Titlecase => title_case(content),
    };
    if list == ListType::None {
        return transformed;
    }
    let mut n = list_start;
    transformed
        .split('\n')
        .map(|line| {
            if line.trim().is_empty() {
                line.to_string()
            } else {
                match list {
                    ListType::Bullet => format!("\u{2022} {line}"),
                    ListType::Numbered => {
                        let prefix = format!("{n}. ");
                        n += 1;
                        format!("{prefix}{line}")
                    }
                    ListType::None => unreachable!(),
                }
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn title_case(s: &str) -> String {
    let mut result = String::with_capacity(s.len());
    let mut capitalize_next = true;
    for ch in s.chars() {
        if ch.is_whitespace() {
            capitalize_next = true;
            result.push(ch);
        } else if capitalize_next {
            result.extend(ch.to_uppercase());
            capitalize_next = false;
        } else {
            result.extend(ch.to_lowercase());
        }
    }
    result
}

/// Resolves any `TextFont` tier to the `FontId` egui would actually draw it
/// with — shared with `ui/inspector.rs`'s font-picker preview so it renders
/// each candidate in its real face rather than guessing at a mapping.
pub(crate) fn font_id(ctx: &egui::Context, font: &TextFont, size: f32) -> FontId {
    match font {
        TextFont::Proportional => FontId::proportional(size),
        TextFont::Monospace => FontId::monospace(size),
        TextFont::Serif => FontId::new(size, egui::FontFamily::Name(crate::fonts::SERIF_FAMILY.into())),
        TextFont::Display => FontId::new(size, egui::FontFamily::Name(crate::fonts::DISPLAY_FAMILY.into())),
        TextFont::Handwriting => {
            FontId::new(size, egui::FontFamily::Name(crate::fonts::HANDWRITING_FAMILY.into()))
        }
        TextFont::System(name) => FontId::new(size, crate::system_fonts::resolve_family(ctx, name)),
    }
}

fn text_format(ctx: &egui::Context, style: &TextStyleParams, zoom: f32, color: Color32) -> TextFormat {
    let deco_width = (style.font_size * zoom * 0.06).max(1.0);
    TextFormat {
        font_id: font_id(ctx, &style.font, style.font_size * zoom),
        extra_letter_spacing: style.letter_spacing * zoom,
        line_height: style.line_height.map(|h| h * zoom),
        color,
        italics: style.italic,
        underline: if style.underline { Stroke::new(deco_width, color) } else { Stroke::NONE },
        strikethrough: if style.strikethrough { Stroke::new(deco_width, color) } else { Stroke::NONE },
        ..Default::default()
    }
}

fn halign(align: TextAlign) -> (Align, bool) {
    match align {
        TextAlign::Left => (Align::LEFT, false),
        TextAlign::Center => (Align::Center, false),
        TextAlign::Right => (Align::RIGHT, false),
        TextAlign::Justify => (Align::LEFT, true),
    }
}

/// Lays out one `Text` layer's content as one `Galley` per authored line.
/// This app's content model has no soft-return, so each `\n`-separated line
/// is treated as its own paragraph (matching the common definition, where
/// a plain Return starts a new paragraph). A line may still wrap into
/// multiple visual rows within its own galley if `wrap_width` is finite and
/// narrower than the line's natural width. `color`/`zoom` are baked into the
/// returned galleys; `wrap_width` is in the same (already zoom-scaled)
/// screen-pixel space the caller draws in.
pub fn layout_paragraphs(
    ctx: &egui::Context,
    content: &str,
    style: &TextStyleParams,
    zoom: f32,
    color: Color32,
    wrap_width: f32,
) -> Vec<Arc<Galley>> {
    let text = display_string(content, style.transform, style.list, style.list_start);
    let format = text_format(ctx, style, zoom, color);
    let (align, justify) = halign(style.align);

    text.split('\n')
        .map(|line| {
            let mut job = LayoutJob::single_section(line.to_string(), format.clone());
            job.wrap.max_width = wrap_width;
            job.halign = align;
            job.justify = justify;
            ctx.fonts_mut(|f| f.layout_job(job))
        })
        .collect()
}

/// Bold counterpart to `font_id` — see `fonts.rs`'s "Bold" doc section for
/// why bold needs a distinct `FontFamily` rather than a flag.
fn font_id_bold(ctx: &egui::Context, font: &TextFont, size: f32) -> FontId {
    match font {
        TextFont::Proportional => FontId::new(size, egui::FontFamily::Name(crate::fonts::PROPORTIONAL_BOLD_FAMILY.into())),
        TextFont::Monospace => FontId::new(size, egui::FontFamily::Name(crate::fonts::MONOSPACE_BOLD_FAMILY.into())),
        TextFont::Serif => FontId::new(size, egui::FontFamily::Name(crate::fonts::SERIF_BOLD_FAMILY.into())),
        TextFont::Display => FontId::new(size, egui::FontFamily::Name(crate::fonts::DISPLAY_BOLD_FAMILY.into())),
        TextFont::Handwriting => {
            FontId::new(size, egui::FontFamily::Name(crate::fonts::HANDWRITING_BOLD_FAMILY.into()))
        }
        TextFont::System(name) => FontId::new(size, crate::system_fonts::resolve_family_bold(ctx, name)),
    }
}

/// `TextFormat` for one run (or, when `run_style` is `base`, for
/// list-prefix characters, which don't belong to any run). Paragraph-level
/// fields (`extra_letter_spacing`/`line_height`) still come from the
/// layer-wide `layer_style`, matching `text_format`.
fn run_text_format(ctx: &egui::Context, run_style: &RunStyle, layer_style: &TextStyleParams, zoom: f32, layer_color: Color32) -> TextFormat {
    let font_size = run_style.font_size * zoom;
    let font_id = if run_style.bold {
        font_id_bold(ctx, &run_style.font, font_size)
    } else {
        font_id(ctx, &run_style.font, font_size)
    };
    let color = run_style.color.unwrap_or(layer_color);
    let deco_width = (run_style.font_size * zoom * 0.06).max(1.0);
    TextFormat {
        font_id,
        extra_letter_spacing: layer_style.letter_spacing * zoom,
        line_height: layer_style.line_height.map(|h| h * zoom),
        color,
        italics: run_style.italic,
        underline: if run_style.underline { Stroke::new(deco_width, color) } else { Stroke::NONE },
        strikethrough: if run_style.strikethrough { Stroke::new(deco_width, color) } else { Stroke::NONE },
        ..Default::default()
    }
}

/// Maps each of `char_count` content-relative char positions to which
/// `runs` index it belongs to. Empty if `runs` is empty. Defensive against
/// `runs`'s lengths not summing to exactly `char_count` (pads/truncates
/// with the last run) — this must never be the reason text fails to
/// render, even if some other bug left `runs` briefly out of sync with
/// `content`.
fn run_index_per_char(char_count: usize, runs: &[TextRun]) -> Vec<usize> {
    if runs.is_empty() {
        return Vec::new();
    }
    let mut out = Vec::with_capacity(char_count);
    for (i, run) in runs.iter().enumerate() {
        for _ in 0..run.len {
            out.push(i);
        }
    }
    while out.len() < char_count {
        out.push(runs.len() - 1);
    }
    out.truncate(char_count);
    out
}

/// Tags every content-relative char with which `runs` index it belongs to
/// (`None` meaning "use the base/layer style" — only ever produced for
/// list-prefix characters, added below, which don't belong to any run),
/// after applying `transform`'s case mapping (which can expand one source
/// char into several — e.g. `'ß'.to_uppercase()` — all tagged with the
/// same run as their source char), then splits on `\n` into per-paragraph
/// lines and prepends each non-blank line's list-prefix (`•` / `1.` / etc,
/// per `list`/`list_start`) if `list != None`.
///
/// The single shared source of truth for run-boundary/transform/list-
/// prefix semantics across every rich-text render path — `canvas.rs`
/// (`layout_paragraphs_rich`, below), `export.rs`'s `draw_text_rich`, and
/// `text_outline.rs`'s rich outline conversion all call this rather than
/// each re-deriving it, so they can't silently drift apart on where a run
/// boundary actually falls.
pub(crate) fn tagged_lines(
    content: &str,
    runs: &[TextRun],
    transform: TextTransform,
    list: ListType,
    list_start: i32,
) -> Vec<Vec<(char, Option<usize>)>> {
    let chars: Vec<char> = content.chars().collect();
    let run_index = run_index_per_char(chars.len(), runs);

    let mut tagged: Vec<(char, Option<usize>)> = Vec::with_capacity(chars.len());
    let mut capitalize_next = true;
    for (i, &ch) in chars.iter().enumerate() {
        let idx = run_index.get(i).copied();
        if ch == '\n' || transform == TextTransform::None {
            tagged.push((ch, idx));
            if ch == '\n' {
                capitalize_next = true;
            }
            continue;
        }
        match transform {
            TextTransform::None => unreachable!(),
            TextTransform::Uppercase => tagged.extend(ch.to_uppercase().map(|c| (c, idx))),
            TextTransform::Lowercase => tagged.extend(ch.to_lowercase().map(|c| (c, idx))),
            TextTransform::Titlecase => {
                if ch.is_whitespace() {
                    capitalize_next = true;
                    tagged.push((ch, idx));
                } else if capitalize_next {
                    tagged.extend(ch.to_uppercase().map(|c| (c, idx)));
                    capitalize_next = false;
                } else {
                    tagged.extend(ch.to_lowercase().map(|c| (c, idx)));
                }
            }
        }
    }

    let mut list_n = list_start;
    tagged
        .split(|&(c, _)| c == '\n')
        .map(|line| {
            let is_blank = line.iter().all(|(c, _)| c.is_whitespace());
            let mut prefixed: Vec<(char, Option<usize>)> = Vec::with_capacity(line.len() + 3);
            if list != ListType::None && !is_blank {
                let prefix = match list {
                    ListType::Bullet => "\u{2022} ".to_string(),
                    ListType::Numbered => {
                        let p = format!("{list_n}. ");
                        list_n += 1;
                        p
                    }
                    ListType::None => unreachable!(),
                };
                prefixed.extend(prefix.chars().map(|c| (c, None)));
            }
            prefixed.extend_from_slice(line);
            prefixed
        })
        .collect()
}

/// Rich-text counterpart to `layout_paragraphs`, used whenever `runs` is
/// non-empty (see `LayerKind::Text::runs`'s doc comment in `model/layer.rs`
/// for the invariant). Builds each line's `LayoutJob` from one
/// `LayoutSection` per contiguous same-style stretch instead of
/// `single_section`, so a single paragraph can mix fonts/sizes/colors/
/// bold/italic/underline/strikethrough — egui's click-drag selection and
/// cursor placement work against whichever `Galley` this produces with no
/// extra work, which is what lets the in-place editor (`canvas.rs`) offer
/// real text selection over rich content just by feeding this through
/// `TextEdit::layouter`.
///
/// `base` supplies the style for list-prefix characters, which don't
/// belong to any run. Only `style`'s paragraph-level fields
/// (`align`/`letter_spacing`/`line_height`/`transform`/`list`/
/// `list_start`) are used here — its `font`/`font_size`/`italic`/
/// `underline`/`strikethrough` are superseded by `base`/`runs` and
/// deliberately ignored, since character styling now comes from there
/// instead.
#[allow(clippy::too_many_arguments)]
pub fn layout_paragraphs_rich(
    ctx: &egui::Context,
    content: &str,
    runs: &[TextRun],
    base: &RunStyle,
    style: &TextStyleParams,
    zoom: f32,
    layer_color: Color32,
    wrap_width: f32,
) -> Vec<Arc<Galley>> {
    let (align, justify) = halign(style.align);

    tagged_lines(content, runs, style.transform, style.list, style.list_start)
        .into_iter()
        .map(|prefixed| {
            let mut job = LayoutJob::default();
            job.wrap.max_width = wrap_width;
            job.halign = align;
            job.justify = justify;

            let mut start = 0usize;
            while start < prefixed.len() {
                let key = prefixed[start].1;
                let mut end = start + 1;
                while end < prefixed.len() && prefixed[end].1 == key {
                    end += 1;
                }
                let text: String = prefixed[start..end].iter().map(|(c, _)| *c).collect();
                let run_style = key.and_then(|i| runs.get(i)).map_or(base, |r| &r.style);
                let format = run_text_format(ctx, run_style, style, zoom, layer_color);
                job.append(&text, 0.0, format);
                start = end;
            }
            if prefixed.is_empty() {
                job.append("", 0.0, run_text_format(ctx, base, style, zoom, layer_color));
            }

            ctx.fonts_mut(|f| f.layout_job(job))
        })
        .collect()
}

/// Builds a single whole-buffer `LayoutJob` (not split by paragraph — one
/// `Galley` for the *entire* edit buffer, which is what `egui::TextEdit`'s
/// `.layouter()` callback needs) reflecting `runs`' per-character styling,
/// for the in-place rich-text editing overlay (`canvas.rs`). Because egui
/// uses this same `Galley` for both painting and all click/drag/keyboard
/// selection and cursor placement, the editor gets real text selection
/// over rich content for free — no custom selection code needed.
///
/// Deliberately does *not* apply `TextTransform`/list-prefix display
/// cosmetics the way `layout_paragraphs_rich` does — those aren't applied
/// while editing at all (matching the existing uniform-style editor's
/// behavior, which only ever showed raw `content`), and since they'd
/// inject characters that don't exist in the real buffer, doing so here
/// would desync egui's char-index cursor/selection from the actual
/// content. `letter_spacing`/`line_height`/`zoom` are the layer's raw
/// (unzoomed) values — scaling happens inside `run_text_format`, same as
/// every other render path. `align` *is* applied (via `job.halign`/
/// `job.justify`, set once up front so it covers both the empty- and
/// non-empty-buffer paths below) — unlike the cosmetics above, alignment
/// doesn't inject or reorder characters, so it can't desync the cursor.
pub fn editor_layout_job(
    ctx: &egui::Context,
    buf_text: &str,
    runs: &[TextRun],
    base: &RunStyle,
    align: TextAlign,
    letter_spacing: f32,
    line_height: Option<f32>,
    zoom: f32,
    layer_color: Color32,
) -> LayoutJob {
    let paragraph_style = TextStyleParams {
        font: base.font.clone(),
        font_size: base.font_size,
        align,
        letter_spacing,
        line_height,
        italic: base.italic,
        underline: base.underline,
        strikethrough: base.strikethrough,
        transform: TextTransform::None,
        list: ListType::None,
        list_start: 1,
    };

    let chars: Vec<char> = buf_text.chars().collect();
    let mut job = LayoutJob::default();
    let (h, j) = halign(align);
    job.halign = h;
    job.justify = j;
    if chars.is_empty() {
        job.append("", 0.0, run_text_format(ctx, base, &paragraph_style, zoom, layer_color));
        return job;
    }

    // Guaranteed the same length as `chars` since `runs` is non-empty
    // (every character belongs to some run) — this function is only ever
    // called on the rich-text path.
    let run_index = run_index_per_char(chars.len(), runs);
    let mut start = 0usize;
    while start < chars.len() {
        let key = run_index[start];
        let mut end = start + 1;
        while end < chars.len() && run_index[end] == key {
            end += 1;
        }
        let text: String = chars[start..end].iter().collect();
        let run_style = runs.get(key).map_or(base, |r| &r.style);
        let format = run_text_format(ctx, run_style, &paragraph_style, zoom, layer_color);
        job.append(&text, 0.0, format);
        start = end;
    }
    job
}

/// `editor_layout_job`'s non-rich sibling: a single whole-buffer `LayoutJob`
/// for the uniform-style (no `runs`) in-place editor overlay. Needed because
/// `egui::TextEdit`'s default layouter has no alignment concept, so honoring
/// `align`/decorations (italic/underline/strikethrough) while editing
/// requires supplying a custom layouter here too — same reasoning as
/// `editor_layout_job`, just without the per-run splitting since the whole
/// buffer shares one style.
pub fn plain_editor_layout_job(ctx: &egui::Context, buf_text: &str, style: &TextStyleParams, zoom: f32, color: Color32) -> LayoutJob {
    let format = text_format(ctx, style, zoom, color);
    let (align, justify) = halign(style.align);
    let mut job = LayoutJob::single_section(buf_text.to_string(), format);
    job.halign = align;
    job.justify = justify;
    job
}

/// Stacks already-laid-out paragraph galleys top-to-bottom, inserting
/// `paragraph_spacing` (already zoom-scaled, same space as the galleys)
/// between each pair. Returns each galley's y-offset from the first
/// galley's top, plus the total stacked size (max width, summed height) —
/// used both to position draw calls and to measure natural size for
/// auto-resize (see `canvas.rs::apply_text_auto_resize`).
pub fn stack_paragraphs(galleys: &[Arc<Galley>], paragraph_spacing: f32) -> (Vec<f32>, Vec2) {
    let mut y = 0.0;
    let mut offsets = Vec::with_capacity(galleys.len());
    let mut max_width: f32 = 0.0;
    for (i, galley) in galleys.iter().enumerate() {
        if i > 0 {
            y += paragraph_spacing;
        }
        offsets.push(y);
        y += galley.rect.height();
        max_width = max_width.max(galley.rect.width());
    }
    (offsets, Vec2::new(max_width, y))
}
