//! The Text section of the inspector: content, font size, style toggles,
//! alignment, resize, line height, spacing, transform and list controls.

use std::ops::Range;

use egui::Ui;

use crate::canvas::{CanvasWidget, apply_text_auto_resize};
use crate::history::History;
use crate::model::text_runs::RunStyle;
use crate::model::{Layer, LayerKind, ListType, TextAlign, TextResize, TextTransform, VerticalAlign};

use super::text_path_ui::text_path_attachment_ui;
use super::text_styles::text_style_ui;
use super::{selected_or_base, should_snapshot};

use super::text_font_picker::font_picker;

/// The Text section of a single selected `Text` layer: content, typography,
/// paragraph controls, then the shared Text Style and Path / Area sections.
/// Draws nothing for other kinds. `text_selection` is a non-empty range
/// selected in the in-place canvas editor, which switches the dual-mode
/// controls (size, font, style toggles) to formatting just that range.
pub(super) fn text_section(
    ui: &mut Ui,
    history: &mut History,
    canvas: &mut CanvasWidget,
    layer: &Layer,
    text_selection: &Option<Range<usize>>,
) {
    let LayerKind::Text { font, font_size, bold, italic, underline, strikethrough, style_id, path_attachment, .. } =
        &layer.kind
    else {
        return;
    };
    let id = layer.id;
    let ctx = ui.ctx().clone();
    let base_style = crate::model::text_runs::RunStyle {
        font: font.clone(),
        font_size: *font_size,
        color: layer.style.fill.as_ref().map(crate::model::Paint::to_color32),
        bold: *bold,
        italic: *italic,
        underline: *underline,
        strikethrough: *strikethrough,
    };
    ui.add_space(8.0);
    ui.separator();
    ui.label("Text");
    if canvas.is_editing_text(id) {
        ui.weak(if text_selection.is_some() {
            "Editing on canvas — formatting controls below apply to the selection."
        } else {
            "Editing on canvas — click elsewhere or press Esc to finish."
        });
    }

    text_content_field(ui, history, layer, &ctx);
    font_size_field(ui, history, layer, &ctx, text_selection, &base_style);
    font_picker(ui, history, canvas, layer, &ctx, text_selection, &base_style);
    style_toggles(ui, history, layer, &ctx, text_selection, &base_style);
    alignment_controls(ui, history, layer, &ctx);
    resize_control(ui, history, layer, &ctx);
    line_height_control(ui, history, layer, &ctx);
    spacing_controls(ui, history, layer, &ctx);
    transform_control(ui, history, layer, &ctx);
    list_control(ui, history, layer, &ctx);

    text_style_ui(ui, history, id, *style_id);
    text_path_attachment_ui(ui, history, id, path_attachment.as_ref());
}

fn text_content_field(ui: &mut Ui, history: &mut History, layer: &Layer, ctx: &egui::Context) {
    let LayerKind::Text { content, .. } = &layer.kind else {
        return;
    };
    let id = layer.id;
    let mut buf = content.clone();
    let content_resp = ui.add(egui::TextEdit::multiline(&mut buf).desired_rows(3));
    if should_snapshot(&content_resp) {
        history.snapshot();
    }
    if content_resp.changed()
        && let Some(l) = history.mutate().active_page_mut().find_mut(id) {
            if let LayerKind::Text { content, .. } = &mut l.kind {
                *content = buf;
            }
            apply_text_auto_resize(ctx, l);
        }
}

fn font_size_field(
    ui: &mut Ui,
    history: &mut History,
    layer: &Layer,
    ctx: &egui::Context,
    text_selection: &Option<Range<usize>>, base_style: &RunStyle,
) {
    let LayerKind::Text { font_size, runs, .. } = &layer.kind else {
        return;
    };
    let id = layer.id;
    if let Some(sel) = &text_selection {
        let current = selected_or_base(runs, sel, base_style.font_size, |s| s.font_size);
        let mut size = current.unwrap_or(base_style.font_size);
        let size_resp = ui.add(
            egui::DragValue::new(&mut size)
                .prefix(if current.is_none() { "Font size (Mixed): " } else { "Font size: " })
                .range(1.0..=500.0),
        );
        if should_snapshot(&size_resp) {
            history.snapshot();
        }
        if size_resp.changed()
            && let Some(l) = history.mutate().active_page_mut().find_mut(id) {
                if let LayerKind::Text { content, runs, .. } = &mut l.kind {
                    crate::model::text_runs::apply(content, runs, base_style, sel.clone(), |s| s.font_size = size);
                }
                apply_text_auto_resize(ctx, l);
            }
    } else {
        let mut size = *font_size;
        let size_resp = ui.add(
            egui::DragValue::new(&mut size)
                .prefix("Font size: ")
                .range(1.0..=500.0),
        );
        if should_snapshot(&size_resp) {
            history.snapshot();
        }
        if size_resp.changed()
            && let Some(l) = history.mutate().active_page_mut().find_mut(id) {
                if let LayerKind::Text { font_size, .. } = &mut l.kind {
                    *font_size = size;
                }
                apply_text_auto_resize(ctx, l);
            }
    }
}

/// Bold / Italic / Underline / Strikethrough toggles.
fn style_toggles(
    ui: &mut Ui,
    history: &mut History,
    layer: &Layer,
    ctx: &egui::Context,
    text_selection: &Option<Range<usize>>, base_style: &RunStyle,
) {
    let LayerKind::Text { bold, italic, underline, strikethrough, runs, .. } = &layer.kind else {
        return;
    };
    let id = layer.id;
    ui.horizontal(|ui| {
        ui.label("Style:");
        // With an active text selection, a click applies to just that
        // range (materializing `runs` lazily via `text_runs::apply`)
        // instead of the whole layer; a `Mixed` selection (differing
        // values across the range) or an unset value both count as
        // "off" for display, and either way one click sets the whole
        // selection to `true` (only a uniformly-`true` selection
        // clears back to `false`) — the standard "any unset → set
        // all" convention.
        let mut toggle = |label: &str,
                           base_active: bool,
                           run_field: fn(&crate::model::text_runs::RunStyle) -> bool,
                           run_apply: fn(&mut crate::model::text_runs::RunStyle, bool),
                           layer_apply: fn(&mut LayerKind, bool)| {
            let current: Option<bool> = match &text_selection {
                Some(sel) => selected_or_base(runs, sel, base_active, run_field),
                None => Some(base_active),
            };
            let active = current == Some(true);
            if ui.selectable_label(active, label).clicked() {
                history.snapshot();
                let new_value = current != Some(true);
                if let Some(l) = history.mutate().active_page_mut().find_mut(id) {
                    if let Some(sel) = &text_selection {
                        if let LayerKind::Text { content, runs, .. } = &mut l.kind {
                            crate::model::text_runs::apply(content, runs, base_style, sel.clone(), |s| {
                                run_apply(s, new_value)
                            });
                        }
                    } else {
                        layer_apply(&mut l.kind, new_value);
                    }
                    apply_text_auto_resize(ctx, l);
                }
            }
        };
        toggle(
            "Bold",
            *bold,
            |s| s.bold,
            |s, v| s.bold = v,
            |k, v| {
                if let LayerKind::Text { bold, .. } = k {
                    *bold = v;
                }
            },
        );
        toggle(
            "Italic",
            *italic,
            |s| s.italic,
            |s, v| s.italic = v,
            |k, v| {
                if let LayerKind::Text { italic, .. } = k {
                    *italic = v;
                }
            },
        );
        toggle(
            "Underline",
            *underline,
            |s| s.underline,
            |s, v| s.underline = v,
            |k, v| {
                if let LayerKind::Text { underline, .. } = k {
                    *underline = v;
                }
            },
        );
        toggle(
            "Strikethrough",
            *strikethrough,
            |s| s.strikethrough,
            |s, v| s.strikethrough = v,
            |k, v| {
                if let LayerKind::Text { strikethrough, .. } = k {
                    *strikethrough = v;
                }
            },
        );
    });
}

/// Horizontal and vertical alignment rows.
fn alignment_controls(ui: &mut Ui, history: &mut History, layer: &Layer, ctx: &egui::Context) {
    let LayerKind::Text { align, vertical_align, .. } = &layer.kind else {
        return;
    };
    let id = layer.id;
    ui.horizontal(|ui| {
        ui.label("Align:");
        for (label, choice) in [
            ("Left", TextAlign::Left),
            ("Center", TextAlign::Center),
            ("Right", TextAlign::Right),
            ("Justify", TextAlign::Justify),
        ] {
            if ui.selectable_label(*align == choice, label).clicked() && *align != choice {
                history.snapshot();
                if let Some(l) = history.mutate().active_page_mut().find_mut(id) {
                    if let LayerKind::Text { align, .. } = &mut l.kind {
                        *align = choice;
                    }
                    apply_text_auto_resize(ctx, l);
                }
            }
        }
    });

    ui.horizontal(|ui| {
        ui.label("Vertical:");
        for (label, choice) in [
            ("Top", VerticalAlign::Top),
            ("Middle", VerticalAlign::Middle),
            ("Bottom", VerticalAlign::Bottom),
        ] {
            if ui.selectable_label(*vertical_align == choice, label).clicked() && *vertical_align != choice {
                history.snapshot();
                if let Some(l) = history.mutate().active_page_mut().find_mut(id)
                    && let LayerKind::Text { vertical_align, .. } = &mut l.kind {
                        *vertical_align = choice;
                    }
            }
        }
    });
}

fn resize_control(ui: &mut Ui, history: &mut History, layer: &Layer, ctx: &egui::Context) {
    let LayerKind::Text { resize, .. } = &layer.kind else {
        return;
    };
    let id = layer.id;
    ui.horizontal(|ui| {
        ui.label("Resize:");
        for (label, choice) in [
            ("Auto", TextResize::Auto),
            ("Auto Height", TextResize::AutoHeight),
            ("Fixed", TextResize::Fixed),
        ] {
            if ui.selectable_label(*resize == choice, label).clicked() && *resize != choice {
                history.snapshot();
                if let Some(l) = history.mutate().active_page_mut().find_mut(id) {
                    if let LayerKind::Text { resize, .. } = &mut l.kind {
                        *resize = choice;
                    }
                    apply_text_auto_resize(ctx, l);
                }
            }
        }
    });
}

fn line_height_control(ui: &mut Ui, history: &mut History, layer: &Layer, ctx: &egui::Context) {
    let LayerKind::Text { line_height, font_size, .. } = &layer.kind else {
        return;
    };
    let id = layer.id;
    ui.horizontal(|ui| {
        let mut auto_line_height = line_height.is_none();
        let auto_resp = ui.checkbox(&mut auto_line_height, "Auto line height");
        if auto_resp.changed() {
            history.snapshot();
            if let Some(l) = history.mutate().active_page_mut().find_mut(id) {
                if let LayerKind::Text { line_height, font_size, .. } = &mut l.kind {
                    *line_height = if auto_line_height { None } else { Some(*font_size * 1.2) };
                }
                apply_text_auto_resize(ctx, l);
            }
        }
        if !auto_line_height {
            let mut value = line_height.unwrap_or(*font_size * 1.2);
            let resp = ui.add(egui::DragValue::new(&mut value).prefix("Line height: ").range(1.0..=1000.0));
            if should_snapshot(&resp) {
                history.snapshot();
            }
            if resp.changed()
                && let Some(l) = history.mutate().active_page_mut().find_mut(id) {
                    if let LayerKind::Text { line_height, .. } = &mut l.kind {
                        *line_height = Some(value);
                    }
                    apply_text_auto_resize(ctx, l);
                }
        }
    });
}

/// Letter and paragraph spacing.
fn spacing_controls(ui: &mut Ui, history: &mut History, layer: &Layer, ctx: &egui::Context) {
    let LayerKind::Text { letter_spacing, paragraph_spacing, .. } = &layer.kind else {
        return;
    };
    let id = layer.id;
    ui.horizontal(|ui| {
        let mut spacing = *letter_spacing;
        let resp = ui.add(egui::DragValue::new(&mut spacing).prefix("Letter spacing: ").range(-50.0..=200.0));
        if should_snapshot(&resp) {
            history.snapshot();
        }
        if resp.changed()
            && let Some(l) = history.mutate().active_page_mut().find_mut(id) {
                if let LayerKind::Text { letter_spacing, .. } = &mut l.kind {
                    *letter_spacing = spacing;
                }
                apply_text_auto_resize(ctx, l);
            }

        let mut para_spacing = *paragraph_spacing;
        let resp = ui.add(
            egui::DragValue::new(&mut para_spacing)
                .prefix("Paragraph spacing: ")
                .range(0.0..=1000.0),
        );
        if should_snapshot(&resp) {
            history.snapshot();
        }
        if resp.changed()
            && let Some(l) = history.mutate().active_page_mut().find_mut(id) {
                if let LayerKind::Text { paragraph_spacing, .. } = &mut l.kind {
                    *paragraph_spacing = para_spacing;
                }
                apply_text_auto_resize(ctx, l);
            }
    });
}

fn transform_control(ui: &mut Ui, history: &mut History, layer: &Layer, ctx: &egui::Context) {
    let LayerKind::Text { transform, .. } = &layer.kind else {
        return;
    };
    let id = layer.id;
    ui.horizontal(|ui| {
        ui.label("Transform:");
        for (label, choice) in [
            ("Normal", TextTransform::None),
            ("UPPER", TextTransform::Uppercase),
            ("lower", TextTransform::Lowercase),
            ("Title", TextTransform::Titlecase),
        ] {
            if ui.selectable_label(*transform == choice, label).clicked() && *transform != choice {
                history.snapshot();
                if let Some(l) = history.mutate().active_page_mut().find_mut(id) {
                    if let LayerKind::Text { transform, .. } = &mut l.kind {
                        *transform = choice;
                    }
                    apply_text_auto_resize(ctx, l);
                }
            }
        }
    });
}

fn list_control(ui: &mut Ui, history: &mut History, layer: &Layer, ctx: &egui::Context) {
    let LayerKind::Text { list, list_start, .. } = &layer.kind else {
        return;
    };
    let id = layer.id;
    ui.horizontal(|ui| {
        ui.label("List:");
        for (label, choice) in [
            ("None", ListType::None),
            ("Bullet", ListType::Bullet),
            ("Numbered", ListType::Numbered),
        ] {
            if ui.selectable_label(*list == choice, label).clicked() && *list != choice {
                history.snapshot();
                if let Some(l) = history.mutate().active_page_mut().find_mut(id) {
                    if let LayerKind::Text { list, .. } = &mut l.kind {
                        *list = choice;
                    }
                    apply_text_auto_resize(ctx, l);
                }
            }
        }
        if *list == ListType::Numbered {
            let mut start = *list_start;
            let resp = ui.add(egui::DragValue::new(&mut start).prefix("Start: ").range(0..=100000));
            if should_snapshot(&resp) {
                history.snapshot();
            }
            if resp.changed()
                && let Some(l) = history.mutate().active_page_mut().find_mut(id) {
                    if let LayerKind::Text { list_start, .. } = &mut l.kind {
                        *list_start = start;
                    }
                    apply_text_auto_resize(ctx, l);
                }
        }
    });
}
