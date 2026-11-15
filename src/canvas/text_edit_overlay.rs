//! In-place canvas text editing: a floating `egui::TextEdit` positioned over
//! the `Text` layer being edited so it reads as editing the glyphs directly.

use egui::{Color32, Rect, Ui, Vec2};

use crate::history::History;
use crate::model::{
    LayerId, LayerKind, TextAlign, TextFont,
    TextResize, VerticalAlign,
};
use crate::text_layout::{self, TextStyleParams};

use super::layer_creation::apply_text_auto_resize;
use super::{
    CanvasLayout, CanvasWidget,
};

impl CanvasWidget {
    /// In-place canvas text editing: a floating `egui::Area` positioned
    /// exactly over the layer being edited (`start_editing_text`), so it
    /// reads as editing the glyphs directly rather than a side-panel
    /// form. Called last in `ui()`, so the editor floats over everything
    /// else drawn this frame.
    ///
    /// One function despite its length: the measuring pass, the layouter
    /// closure and the `TextEdit` area all share the same per-frame values
    /// and only make sense together.
    pub(super) fn show_text_edit_overlay(&mut self, ui: &Ui, history: &mut History, id: LayerId, layout: CanvasLayout) {
        let CanvasLayout { canvas_rect, origin, .. } = layout;
        let ctx = ui.ctx().clone();
        let Some(snap) = read_edit_snapshot(history, id) else {
            // Layer deleted, or an undo/redo landed on a state where it's
            // no longer a `Text` layer — just fall out of edit mode.
            self.editing_text = None;
            self.text_edit_selection = None;
            return;
        };
        let mut buf = snap.content.clone();
        let color = snap.fill.unwrap_or(Color32::BLACK);
        let bounds = snap.bounds;
        let zoom = self.zoom;
        let is_rich = !snap.runs.is_empty();

        let area_id = egui::Id::new(("text-edit-overlay", id));
        let text_edit_id = area_id.with("buf");
        // `Auto` grows without wrapping as you type (mirroring the common
        // "Auto Width"), so give it a generous width rather than the
        // current (possibly stale-until-next-frame) frame width.
        let width = match snap.resize {
            TextResize::Auto => 2000.0_f32.min(canvas_rect.width()),
            TextResize::AutoHeight | TextResize::Fixed => (bounds.width() * zoom).max(40.0),
        };

        let (base, style_params) = edit_styles(&snap);

        // `TextEdit` always draws from the top of its box and has no
        // alignment concept of its own — `align` is handled inside
        // `build_edit_layout_job` (via `job.halign`), but `vertical_align`
        // needs the rendered content's height to know how far down to
        // shift the whole overlay, so it's measured once upfront here
        // (the same job is built again, deterministically, inside the
        // layouter below).
        let content_height = ui
            .ctx()
            .fonts_mut(|f| {
                f.layout_job(build_edit_layout_job(&ctx, &buf, width, is_rich, &snap.runs, &base, &style_params, zoom, color))
            })
            .rect
            .height();
        let box_height = bounds.height() * zoom;
        let y_offset = match snap.vertical_align {
            VerticalAlign::Top => 0.0,
            VerticalAlign::Middle => ((box_height - content_height) * 0.5).max(0.0),
            VerticalAlign::Bottom => (box_height - content_height).max(0.0),
        };
        let screen_pos = self.to_screen(origin, bounds.min) + Vec2::new(0.0, y_offset);

        // One shared layouter for both uniform-style and rich-text
        // editing (`is_rich` picks between them inside
        // `build_edit_layout_job`) — egui uses the returned `Galley` for
        // both painting and all click/drag/keyboard selection, so this
        // is the entire mechanism that makes selecting-and-formatting
        // rich text work, same as before this shared alignment/vertical-
        // align support existed.
        let runs_for_layout = snap.runs.clone();
        let base_for_layout = base.clone();
        let layouter_ctx = ctx.clone();
        let mut layouter = move |_ui: &egui::Ui, buf: &dyn egui::TextBuffer, wrap_width: f32| {
            let job = build_edit_layout_job(
                &layouter_ctx,
                buf.as_str(),
                wrap_width,
                is_rich,
                &runs_for_layout,
                &base_for_layout,
                &style_params,
                zoom,
                color,
            );
            layouter_ctx.fonts_mut(|f| f.layout_job(job))
        };
        let inner = egui::Area::new(area_id)
            .order(egui::Order::Foreground)
            .fixed_pos(screen_pos)
            .show(ui.ctx(), |ui| {
                ui.set_width(width);
                egui::TextEdit::multiline(&mut buf)
                    .id(text_edit_id)
                    .layouter(&mut layouter)
                    .frame(egui::Frame::NONE)
                    .desired_width(width)
                    .show(ui)
            })
            .inner;

        self.apply_text_edit_output(ui, history, id, &ctx, inner, buf, &base, text_edit_id);
    }

    /// Handles the editor's output for this frame: select-all focus on the
    /// first frame, the live selection range, writing changed content back
    /// (splicing rich-text runs), and leaving edit mode on blur or Escape.
    #[allow(clippy::too_many_arguments)]
    fn apply_text_edit_output(
        &mut self,
        ui: &Ui,
        history: &mut History,
        id: LayerId,
        ctx: &egui::Context,
        inner: egui::text_edit::TextEditOutput,
        buf: String,
        base: &crate::model::text_runs::RunStyle,
        text_edit_id: egui::Id,
    ) {
        if self.editing_text_just_started {
            inner.response.response.request_focus();
            let range = egui::text::CCursorRange::two(
                egui::text::CCursor::new(0),
                egui::text::CCursor::new(buf.chars().count()),
            );
            let mut state = inner.state.clone();
            state.cursor.set_char_range(Some(range));
            state.store(ui.ctx(), text_edit_id);
            self.editing_text_just_started = false;
        }

        self.text_edit_selection = inner.cursor_range.and_then(|r| {
            let range = r.as_sorted_char_range();
            (range.start.0 < range.end.0).then_some(range.start.0..range.end.0)
        });

        if inner.response.response.changed()
            && let Some(l) = history.mutate().active_page_mut().find_mut(id) {
                if let LayerKind::Text { content, runs, .. } = &mut l.kind {
                    if !runs.is_empty() {
                        let (start, removed, inserted) = crate::model::text_runs::diff_chars(content, &buf);
                        crate::model::text_runs::splice(runs, start, removed, inserted, base);
                    }
                    *content = buf;
                }
                apply_text_auto_resize(ctx, l);
            }

        let escape_pressed = ui.input(|i| i.key_pressed(egui::Key::Escape));
        if inner.response.response.lost_focus() || escape_pressed {
            self.editing_text = None;
            self.text_edit_selection = None;
        }
    }
}

/// The edited `Text` layer's content/style captured in one short borrow of
/// `history`, so the editor can run while `history` is later mutated.
struct EditSnapshot {
    content: String,
    font_size: f32,
    font: TextFont,
    align: TextAlign,
    vertical_align: VerticalAlign,
    resize: TextResize,
    fill: Option<Color32>,
    bounds: Rect,
    letter_spacing: f32,
    line_height: Option<f32>,
    bold: bool,
    italic: bool,
    underline: bool,
    strikethrough: bool,
    runs: Vec<crate::model::text_runs::TextRun>,
}

/// Reads the edited layer's `EditSnapshot`; `None` if it was deleted or is no
/// longer a `Text` layer (e.g. after an undo).
fn read_edit_snapshot(history: &History, id: LayerId) -> Option<EditSnapshot> {
    let doc = history.get();
    doc.active_page().find(id).zip(doc.active_page().absolute_offset(id)).and_then(
        |(layer, offset)| {
            if let LayerKind::Text {
                content,
                font_size,
                font,
                align,
                vertical_align,
                resize,
                letter_spacing,
                line_height,
                bold,
                italic,
                underline,
                strikethrough,
                runs,
                ..
            } = &layer.kind
            {
                Some(EditSnapshot {
                    content: content.clone(),
                    font_size: *font_size,
                    font: font.clone(),
                    align: *align,
                    vertical_align: *vertical_align,
                    resize: *resize,
                    fill: layer.style.fill.as_ref().map(crate::model::Paint::to_color32),
                    bounds: layer.frame.bounds().translate(offset),
                    letter_spacing: *letter_spacing,
                    line_height: *line_height,
                    bold: *bold,
                    italic: *italic,
                    underline: *underline,
                    strikethrough: *strikethrough,
                    runs: runs.clone(),
                })
            } else {
                None
            }
        },
    )
}

/// The editor's base run style and paragraph style, built from the layer.
fn edit_styles(snap: &EditSnapshot) -> (crate::model::text_runs::RunStyle, TextStyleParams) {
    let base = crate::model::text_runs::RunStyle {
        font: snap.font.clone(),
        font_size: snap.font_size,
        color: snap.fill,
        bold: snap.bold,
        italic: snap.italic,
        underline: snap.underline,
        strikethrough: snap.strikethrough,
    };
    let style_params = TextStyleParams {
        font: snap.font.clone(),
        font_size: snap.font_size,
        align: snap.align,
        letter_spacing: snap.letter_spacing,
        line_height: snap.line_height,
        italic: snap.italic,
        underline: snap.underline,
        strikethrough: snap.strikethrough,
        transform: crate::model::TextTransform::None,
        list: crate::model::ListType::None,
        list_start: 1,
    };
    (base, style_params)
}

/// Builds the `LayoutJob` the in-place text-editing overlay (`show_text_edit_overlay`)
/// hands to `egui::TextEdit`'s `.layouter()` — shared by both the uniform-
/// style and rich-text editing paths (`is_rich` selects between them), and
/// called twice per frame: once upfront just to measure the resulting
/// `Galley`'s height (for `vertical_align`, which egui's `TextEdit` has no
/// native concept of), then again inside the layouter callback itself. A
/// plain function rather than a closure so neither call site has to worry
/// about capturing/cloning the same handful of values twice.
#[allow(clippy::too_many_arguments)]
fn build_edit_layout_job(
    ctx: &egui::Context,
    text: &str,
    wrap_width: f32,
    is_rich: bool,
    runs: &[crate::model::text_runs::TextRun],
    base: &crate::model::text_runs::RunStyle,
    style: &TextStyleParams,
    zoom: f32,
    color: Color32,
) -> egui::text::LayoutJob {
    let mut job = if is_rich {
        text_layout::editor_layout_job(ctx, text, runs, base, style.align, style.letter_spacing, style.line_height, zoom, color)
    } else {
        text_layout::plain_editor_layout_job(ctx, text, style, zoom, color)
    };
    job.wrap.max_width = wrap_width;
    job
}
