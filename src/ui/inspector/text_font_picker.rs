//! The Text section's Font picker.

use std::ops::Range;

use egui::Ui;

use crate::canvas::{CanvasWidget, apply_text_auto_resize};
use crate::history::History;
use crate::model::text_runs::RunStyle;
use crate::model::{Layer, LayerKind, TextFont};

use super::selected_or_base;

/// The Font combo box: the bundled faces, then a searchable list of system
/// fonts, each row previewing the layer's own text in that face.
pub(super) fn font_picker(
    ui: &mut Ui,
    history: &mut History,
    canvas: &mut CanvasWidget,
    layer: &Layer,
    ctx: &egui::Context,
    text_selection: &Option<Range<usize>>,
    base_style: &RunStyle,
) {
    let LayerKind::Text { content, font, runs, .. } = &layer.kind else {
        return;
    };
    let id = layer.id;
    ui.horizontal(|ui| {
        ui.label("Font:");
        let current_font: Option<TextFont> = match &text_selection {
            Some(sel) => selected_or_base(runs, sel, font.clone(), |s| s.font.clone()),
            None => Some(font.clone()),
        };
        let selected_text =
            current_font.as_ref().map_or_else(|| "Mixed".to_string(), |f| font_label(f).to_string());
        let preview_source: String = if content.trim().is_empty() {
            "The quick brown fox".to_string()
        } else {
            content.chars().take(60).collect()
        };
        egui::ComboBox::from_id_salt(("font-picker", id))
            .selected_text(selected_text)
            .width(180.0)
            .show_ui(ui, |ui| {
                // Each row shows its own label plus `preview_source` set
                // in that candidate's actual font, so browsing the list
                // previews every option in place rather than needing a
                // separate preview area that scrolls out of view.
                let preview_row = |ui: &mut egui::Ui, selected: bool, label: &str, choice: &TextFont| {
                    let preview = egui::RichText::new(&preview_source).font(crate::text_layout::font_id(ctx, choice, 16.0));
                    ui.selectable_label(selected, (label, preview))
                };

                let mut pick = |choice: TextFont| {
                    if current_font.as_ref() == Some(&choice) {
                        return;
                    }
                    // For a `System` font egui hasn't bound yet, this
                    // can't come back true until egui's *next* pass
                    // (see `CLAUDE.md`'s "Fonts" section) — so the
                    // auto-resize below measures against the
                    // `Proportional` fallback. Queue a retry so
                    // `frame.size` gets corrected once the real font
                    // lands, instead of staying pinned to the wrong
                    // metrics.
                    let already_bound = if let TextFont::System(name) = &choice {
                        let family = egui::FontFamily::Name(name.as_str().into());
                        ctx.fonts(|f| f.definitions().families.contains_key(&family))
                    } else {
                        true
                    };
                    history.snapshot();
                    if let Some(l) = history.mutate().active_page_mut().find_mut(id) {
                        if let Some(sel) = &text_selection {
                            if let LayerKind::Text { content, runs, .. } = &mut l.kind {
                                let choice = choice.clone();
                                crate::model::text_runs::apply(content, runs, base_style, sel.clone(), |s| {
                                    s.font = choice.clone()
                                });
                            }
                        } else if let LayerKind::Text { font, .. } = &mut l.kind {
                            *font = choice.clone();
                        }
                        apply_text_auto_resize(ctx, l);
                    }
                    if let TextFont::System(name) = &choice {
                        crate::system_fonts::ensure_registered(ctx, name);
                        if !already_bound {
                            canvas.queue_font_resize_retry(id, name.clone());
                        }
                    }
                };

                for (label, choice) in [
                    ("Sans", TextFont::Proportional),
                    ("Mono", TextFont::Monospace),
                    ("Serif", TextFont::Serif),
                    ("Display", TextFont::Display),
                    ("Handwriting", TextFont::Handwriting),
                ] {
                    let selected = current_font.as_ref() == Some(&choice);
                    if preview_row(ui, selected, label, &choice).clicked() {
                        pick(choice);
                    }
                }

                ui.separator();
                ui.weak("System fonts");
                let filter_id = egui::Id::new(("font-filter", id));
                let mut filter = ui.ctx().data(|d| d.get_temp::<String>(filter_id)).unwrap_or_default();
                let filter_resp =
                    ui.add(egui::TextEdit::singleline(&mut filter).hint_text("Search\u{2026}"));
                if filter_resp.changed() {
                    ui.ctx().data_mut(|d| d.insert_temp(filter_id, filter.clone()));
                }
                let filter_lower = filter.to_lowercase();
                let matching_names: Vec<&String> = crate::system_fonts::family_names()
                    .iter()
                    .filter(|name| filter_lower.is_empty() || name.to_lowercase().contains(&filter_lower))
                    .collect();
                // Sized for a 16px preview line plus button padding.
                // `show_rows` only builds rows actually scrolled into
                // view, so only those candidates' `System` faces get
                // resolved/registered with egui — loading every
                // installed family (some multi-MB) the moment the
                // dropdown opens would be a real memory/latency hit.
                let row_height = 28.0;
                egui::ScrollArea::vertical().max_height(220.0).show_rows(
                    ui,
                    row_height,
                    matching_names.len(),
                    |ui, row_range| {
                        for i in row_range {
                            let name = matching_names[i];
                            let choice = TextFont::System(name.clone());
                            let selected = current_font.as_ref() == Some(&choice);
                            if preview_row(ui, selected, name, &choice).clicked() {
                                pick(choice);
                            }
                        }
                    },
                );
            });
    });
}

/// Display label for the Font picker's collapsed/selected state.
fn font_label(font: &TextFont) -> &str {
    match font {
        TextFont::Proportional => "Sans",
        TextFont::Monospace => "Mono",
        TextFont::Serif => "Serif",
        TextFont::Display => "Display",
        TextFont::Handwriting => "Handwriting",
        TextFont::System(name) => name,
    }
}
