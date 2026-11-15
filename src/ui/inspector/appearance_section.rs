//! The appearance part of the inspector shared by every layer kind: opacity,
//! layer style, fill, stroke and shadows.

use std::ops::Range;

use egui::Ui;

use crate::history::History;
use crate::model::text_runs::{RunStyle, TextRun};
use crate::model::{Layer, LayerKind, Paint, Stroke};
use crate::ui::color_picker;

use super::layer_styles::layer_style_ui;
use super::paint_editor::paint_editor;
use super::shadow_section::shadow_section;
use super::{selected_or_base, should_snapshot};

/// Opacity, Layer Style, Fill, Stroke and both shadow sections — shown for
/// every single selected layer.
pub(super) fn appearance_section(
    ui: &mut Ui,
    history: &mut History,
    layer: &Layer,
    text_selection: &Option<Range<usize>>,
) {
    let id = layer.id;
    ui.add_space(8.0);
    let mut opacity_pct = layer.opacity * 100.0;
    let opacity_resp = ui.add(
        egui::Slider::new(&mut opacity_pct, 0.0..=100.0)
            .suffix("%")
            .text("Opacity"),
    );
    if should_snapshot(&opacity_resp) {
        history.snapshot();
    }
    if opacity_resp.changed()
        && let Some(l) = history.mutate().active_page_mut().find_mut(id) {
            l.opacity = opacity_pct / 100.0;
        }

    layer_style_ui(ui, history, id, layer.style_id);

    fill_section(ui, history, layer, text_selection);
    stroke_section(ui, history, layer);

    shadow_section(ui, history, id, "Drop Shadow", &layer.style.shadows, |l, shadows| l.style.shadows = shadows);
    shadow_section(ui, history, id, "Inner Shadow", &layer.style.inner_shadows, |l, shadows| l.style.inner_shadows = shadows);
}

fn fill_section(ui: &mut Ui, history: &mut History, layer: &Layer, text_selection: &Option<Range<usize>>) {
    ui.add_space(8.0);
    ui.separator();
    ui.label("Fill");
    // A `Text` layer with an active canvas selection gets a per-run color
    // picker instead of the usual whole-layer Enabled+color controls —
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

/// The layer-wide fill: Enabled toggle, paint editor and fill opacity.
fn layer_fill_controls(ui: &mut Ui, history: &mut History, layer: &Layer) {
    let id = layer.id;
    let mut has_fill = layer.style.fill.is_some();
    let fill_toggle = ui.checkbox(&mut has_fill, "Enabled");
    if fill_toggle.changed() {
        history.snapshot();
        if let Some(l) = history.mutate().active_page_mut().find_mut(id) {
            l.style.fill = if has_fill {
                Some(Paint::Solid(egui::Color32::from_rgb(216, 216, 216)))
            } else {
                None
            };
        }
    }
    if let Some(current) = layer.style.fill.clone() {
        // Gradients/noise don't apply to glyph color (see
        // `Paint::to_color32`'s doc comment) — Text layers stay
        // solid-only here.
        let allow_gradient = !matches!(layer.kind, LayerKind::Text { .. });
        paint_editor(ui, history, id, "fill", &current, allow_gradient, allow_gradient, |l, p| l.style.fill = Some(p));

        let mut fill_opacity_pct = layer.style.fill_opacity * 100.0;
        let fo = ui.add(
            egui::Slider::new(&mut fill_opacity_pct, 0.0..=100.0)
                .suffix("%")
                .text("Fill opacity"),
        );
        if should_snapshot(&fo) {
            history.snapshot();
        }
        if fo.changed()
            && let Some(l) = history.mutate().active_page_mut().find_mut(id) {
                l.style.fill_opacity = fill_opacity_pct / 100.0;
            }
    }
}

fn stroke_section(ui: &mut Ui, history: &mut History, layer: &Layer) {
    let id = layer.id;
    ui.add_space(4.0);
    ui.label("Stroke");
    let mut has_stroke = layer.style.stroke.is_some();
    let stroke_toggle = ui.checkbox(&mut has_stroke, "Enabled");
    if stroke_toggle.changed() {
        history.snapshot();
        if let Some(l) = history.mutate().active_page_mut().find_mut(id) {
            l.style.stroke = if has_stroke { Some(Stroke::default()) } else { None };
        }
    }
    if let Some(current_stroke) = layer.style.stroke.clone() {
        // Noise is fill-only (see `paint_editor`'s doc comment) — strokes
        // keep gradients but never offer the Noise option.
        paint_editor(ui, history, id, "stroke", &current_stroke.paint, true, false, |l, p| {
            if let Some(s) = l.style.stroke.as_mut() {
                s.paint = p;
            }
        });
        let mut width = current_stroke.width;
        let wr = ui.add(egui::DragValue::new(&mut width).prefix("Width: ").range(0.0..=100.0));
        if should_snapshot(&wr) {
            history.snapshot();
        }
        if wr.changed()
            && let Some(l) = history.mutate().active_page_mut().find_mut(id)
                && let Some(s) = l.style.stroke.as_mut() {
                    s.width = width;
                }
        let mut stroke_opacity_pct = layer.style.stroke_opacity * 100.0;
        let so = ui.add(
            egui::Slider::new(&mut stroke_opacity_pct, 0.0..=100.0)
                .suffix("%")
                .text("Stroke opacity"),
        );
        if should_snapshot(&so) {
            history.snapshot();
        }
        if so.changed()
            && let Some(l) = history.mutate().active_page_mut().find_mut(id) {
                l.style.stroke_opacity = stroke_opacity_pct / 100.0;
            }
    }
}
