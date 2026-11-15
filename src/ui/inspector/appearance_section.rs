//! The appearance part of the inspector shared by every layer kind: opacity,
//! layer style, fills, borders and shadows — laid out Sketch-style, each
//! with a section header whose +/− button adds or removes the paint.

use std::ops::Range;

use egui::Ui;

use crate::history::History;
use crate::model::text_runs::{RunStyle, TextRun};
use crate::model::{Layer, LayerId, LayerKind, Paint, Stroke};
use crate::ui::color_picker;

use super::inspector_layout::{HeaderButton, section_header};
use super::layer_styles::layer_style_ui;
use super::paint_editor::paint_editor;
use super::shadow_section::shadow_section;
use super::{selected_or_base, should_snapshot};

/// Opacity, Layer Style, Fills, Borders and both shadow sections — shown for
/// every single selected layer.
pub(super) fn appearance_section(
    ui: &mut Ui,
    history: &mut History,
    layer: &Layer,
    text_selection: &Option<Range<usize>>,
) {
    let id = layer.id;
    ui.add_space(6.0);
    layer_opacity_row(ui, history, layer);

    layer_style_ui(ui, history, id, layer.style_id);

    fill_section(ui, history, layer, text_selection);
    stroke_section(ui, history, layer);

    shadow_section(ui, history, id, "Shadows", &layer.style.shadows, |l, shadows| l.style.shadows = shadows);
    shadow_section(ui, history, id, "Inner Shadows", &layer.style.inner_shadows, |l, shadows| l.style.inner_shadows = shadows);
}

/// The layer's own opacity: a slider and a typed percentage on one row.
fn layer_opacity_row(ui: &mut Ui, history: &mut History, layer: &Layer) {
    let mut opacity_pct = layer.opacity * 100.0;
    ui.horizontal(|ui| {
        ui.label("Opacity");
        let slider = ui.add(egui::Slider::new(&mut opacity_pct, 0.0..=100.0).show_value(false));
        let typed = ui.add(egui::DragValue::new(&mut opacity_pct).range(0.0..=100.0).suffix("%"));
        if should_snapshot(&slider) || should_snapshot(&typed) {
            history.snapshot();
        }
        if (slider.changed() || typed.changed())
            && let Some(l) = history.mutate().active_page_mut().find_mut(layer.id) {
                l.opacity = opacity_pct / 100.0;
            }
    });
}

fn fill_section(ui: &mut Ui, history: &mut History, layer: &Layer, text_selection: &Option<Range<usize>>) {
    // A `Text` layer with an active canvas selection gets a per-run color
    // picker instead of the usual whole-layer add/remove + paint controls —
    // matches every other dual-mode Text control (see `text_selection`'s
    // doc comment in `inspector::ui`).
    let text_run_fill = match (&layer.kind, &text_selection) {
        (LayerKind::Text { font, font_size, bold, italic, underline, strikethrough, runs, .. }, Some(sel)) => {
            let base_style = crate::model::text_runs::RunStyle {
                font: font.clone(),
                font_size: *font_size,
                color: layer.style.fill.as_ref().map(crate::model::Paint::to_color32),
                bold: *bold,
                italic: *italic,
                underline: *underline,
                strikethrough: *strikethrough,
            };
            Some((runs, sel.clone(), base_style))
        }
        _ => None,
    };
    if let Some((runs, sel, base_style)) = text_run_fill {
        section_header(ui, "Fills", HeaderButton::None);
        text_run_fill_picker(ui, history, layer, runs, sel, base_style);
    } else {
        layer_fill_controls(ui, history, layer);
    }
}

/// The per-run color picker shown instead of the layer fill controls while
/// text is selected in the in-place editor.
fn text_run_fill_picker(
    ui: &mut Ui,
    history: &mut History,
    layer: &Layer,
    runs: &[TextRun],
    sel: Range<usize>,
    base_style: RunStyle,
) {
    let id = layer.id;
    let base_fill = layer.style.fill.as_ref().map(Paint::to_color32).unwrap_or(egui::Color32::BLACK);
    let current = selected_or_base(runs, &sel, base_fill, |s| s.color.unwrap_or(base_fill));
    let mut color = current.unwrap_or(base_fill);
    ui.horizontal(|ui| {
        if current.is_none() {
            ui.weak("Mixed");
        }
        let cr = color_picker::edit(ui, &mut color, &history.get().palette);
        if should_snapshot(&cr) {
            history.snapshot();
        }
        if cr.changed()
            && let Some(l) = history.mutate().active_page_mut().find_mut(id)
                && let LayerKind::Text { content, runs, .. } = &mut l.kind {
                    crate::model::text_runs::apply(content, runs, &base_style, sel.clone(), |s| {
                        s.color = Some(color)
                    });
                }
    });
}

/// The layer-wide fill: a "Fills" header whose +/− adds or removes the
/// paint, then one row of paint editor + fill opacity.
fn layer_fill_controls(ui: &mut Ui, history: &mut History, layer: &Layer) {
    let id = layer.id;
    let has_fill = layer.style.fill.is_some();
    let button = if has_fill { HeaderButton::Remove } else { HeaderButton::Add };
    if section_header(ui, "Fills", button) {
        history.snapshot();
        if let Some(l) = history.mutate().active_page_mut().find_mut(id) {
            l.style.fill = if has_fill {
                None
            } else {
                Some(Paint::Solid(egui::Color32::from_rgb(216, 216, 216)))
            };
        }
    }
    if let Some(current) = layer.style.fill.clone() {
        // Gradients/noise don't apply to glyph color (see
        // `Paint::to_color32`'s doc comment) — Text layers stay
        // solid-only here.
        let allow_gradient = !matches!(layer.kind, LayerKind::Text { .. });
        ui.horizontal_top(|ui| {
            ui.vertical(|ui| {
                paint_editor(ui, history, id, "fill", &current, allow_gradient, allow_gradient, |l, p| l.style.fill = Some(p));
            });
            opacity_field(ui, history, id, layer.style.fill_opacity, "Fill opacity", |l, v| l.style.fill_opacity = v);
        });
    }
}

/// The border: a "Borders" header whose +/− adds or removes the stroke,
/// then one row of paint editor + width + border opacity.
fn stroke_section(ui: &mut Ui, history: &mut History, layer: &Layer) {
    let id = layer.id;
    let has_stroke = layer.style.stroke.is_some();
    let button = if has_stroke { HeaderButton::Remove } else { HeaderButton::Add };
    if section_header(ui, "Borders", button) {
        history.snapshot();
        if let Some(l) = history.mutate().active_page_mut().find_mut(id) {
            l.style.stroke = if has_stroke { None } else { Some(Stroke::default()) };
        }
    }
    let Some(current_stroke) = layer.style.stroke.clone() else {
        return;
    };
    ui.horizontal_top(|ui| {
        ui.vertical(|ui| {
            // Noise is fill-only (see `paint_editor`'s doc comment) — strokes
            // keep gradients but never offer the Noise option.
            paint_editor(ui, history, id, "stroke", &current_stroke.paint, true, false, |l, p| {
                if let Some(s) = l.style.stroke.as_mut() {
                    s.paint = p;
                }
            });
        });
        let mut width = current_stroke.width;
        let wr = ui
            .add(egui::DragValue::new(&mut width).range(0.0..=100.0).suffix("px"))
            .on_hover_text("Border width");
        if should_snapshot(&wr) {
            history.snapshot();
        }
        if wr.changed()
            && let Some(l) = history.mutate().active_page_mut().find_mut(id)
                && let Some(s) = l.style.stroke.as_mut() {
                    s.width = width;
                }
        opacity_field(ui, history, id, layer.style.stroke_opacity, "Border opacity", |l, v| l.style.stroke_opacity = v);
    });
}

/// A compact percentage field for a 0–1 opacity (`value`), written back
/// through `apply` on change.
fn opacity_field(ui: &mut Ui, history: &mut History, id: LayerId, value: f32, hover: &str, apply: impl Fn(&mut Layer, f32)) {
    let mut pct = value * 100.0;
    let resp = ui
        .add(egui::DragValue::new(&mut pct).range(0.0..=100.0).suffix("%"))
        .on_hover_text(hover);
    if should_snapshot(&resp) {
        history.snapshot();
    }
    if resp.changed()
        && let Some(l) = history.mutate().active_page_mut().find_mut(id) {
            apply(l, pct / 100.0);
        }
}
