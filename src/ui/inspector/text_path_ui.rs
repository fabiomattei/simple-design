//! The Path / Area section: attach a `Text` layer to another shape's
//! outline as a baseline, or wrap it inside that outline.

use egui::Ui;
use crate::history::History;
use crate::model::{Layer, LayerId, LayerKind, TextPathAttachment, TextPathMode};

use super::should_snapshot;

/// Collects every layer in `layers` (recursing into containers) whose kind
/// has a usable outline for `TextPathAttachment::target` — the same kind
/// restriction `text_path_geometry::resolve_target_outline` enforces at
/// render time, kept in sync here so a layer offered in this picker is
/// always one attaching to it will actually do something.
pub(super) fn attachable_targets(layers: &[Layer], out: &mut Vec<(LayerId, String)>) {
    for layer in layers {
        if matches!(
            layer.kind,
            LayerKind::Rectangle { .. }
                | LayerKind::Oval
                | LayerKind::Star { .. }
                | LayerKind::Polygon { .. }
                | LayerKind::Line
                | LayerKind::Arrow { .. }
                | LayerKind::Path { .. }
                | LayerKind::CompoundPath { .. }
                | LayerKind::BooleanGroup { .. }
        ) {
            out.push((layer.id, layer.name.clone()));
        }
        if let Some(children) = layer.kind.children() {
            attachable_targets(children, out);
        }
    }
}

/// "Path / Area" section: lets a `Text` layer attach to (or detach from)
/// another shape's outline — see `LayerKind::Text::path_attachment`.
/// Attaching always starts a fresh `TextPathAttachment` at `TextPathMode::OnPath`
/// with no offset/start/flip (except switching the *target* while already
/// attached keeps the current mode/offset, only replacing `target`), so
/// picking a different shape doesn't silently reset settings the user just
/// tuned.
pub(super) fn text_path_attachment_ui(ui: &mut Ui, history: &mut History, id: LayerId, path_attachment: Option<&TextPathAttachment>) {
    ui.add_space(8.0);
    ui.separator();
    ui.label(egui::RichText::new("Path / Area").strong());
    ui.weak("Follow another shape's outline (Pen curve, or any figure) as a baseline, or wrap inside it.");

    let mut targets = Vec::new();
    attachable_targets(&history.get().active_page().layers, &mut targets);

    let current_name = path_attachment
        .and_then(|a| targets.iter().find(|(target_id, _)| *target_id == a.target).map(|(_, name)| name.clone()))
        .unwrap_or_else(|| "None".to_string());

    egui::ComboBox::from_id_salt(("text-path-attachment-target", id))
        .selected_text(current_name)
        .show_ui(ui, |ui| {
            if ui.selectable_label(path_attachment.is_none(), "None").clicked() && path_attachment.is_some() {
                history.snapshot();
                if let Some(l) = history.mutate().active_page_mut().find_mut(id)
                    && let LayerKind::Text { path_attachment, .. } = &mut l.kind {
                        *path_attachment = None;
                    }
            }
            for (target_id, name) in &targets {
                let selected = path_attachment.map(|a| a.target) == Some(*target_id);
                if ui.selectable_label(selected, name).clicked() && !selected {
                    history.snapshot();
                    if let Some(l) = history.mutate().active_page_mut().find_mut(id)
                        && let LayerKind::Text { path_attachment, .. } = &mut l.kind {
                            let (mode, offset) = path_attachment.as_ref().map_or((TextPathMode::OnPath, 0.0), |a| (a.mode, a.offset));
                            *path_attachment = Some(TextPathAttachment { target: *target_id, mode, offset, start: 0.0, flip: false });
                        }
                }
            }
        });

    let Some(attachment) = path_attachment else {
        return;
    };

    ui.horizontal(|ui| {
        ui.label("Mode:");
        let mut mode = attachment.mode;
        egui::ComboBox::from_id_salt(("text-path-attachment-mode", id))
            .selected_text(match mode {
                TextPathMode::OnPath => "On Path",
                TextPathMode::AreaInside => "Area Inside",
            })
            .show_ui(ui, |ui| {
                ui.selectable_value(&mut mode, TextPathMode::OnPath, "On Path");
                ui.selectable_value(&mut mode, TextPathMode::AreaInside, "Area Inside");
            });
        if mode != attachment.mode {
            history.snapshot();
            if let Some(l) = history.mutate().active_page_mut().find_mut(id)
                && let LayerKind::Text { path_attachment: Some(a), .. } = &mut l.kind {
                    a.mode = mode;
                }
        }
    });

    let mut offset = attachment.offset;
    let offset_resp = ui.add(
        egui::DragValue::new(&mut offset)
            .prefix(if attachment.mode == TextPathMode::OnPath { "Offset from path: " } else { "Inward padding: " })
            .speed(0.5),
    );
    if should_snapshot(&offset_resp) {
        history.snapshot();
    }
    if offset_resp.changed()
        && let Some(l) = history.mutate().active_page_mut().find_mut(id)
            && let LayerKind::Text { path_attachment: Some(a), .. } = &mut l.kind {
                a.offset = offset;
            }

    if attachment.mode == TextPathMode::OnPath {
        let mut start = attachment.start;
        let start_resp = ui.add(egui::Slider::new(&mut start, 0.0..=1.0).text("Start position"));
        if should_snapshot(&start_resp) {
            history.snapshot();
        }
        if start_resp.changed()
            && let Some(l) = history.mutate().active_page_mut().find_mut(id)
                && let LayerKind::Text { path_attachment: Some(a), .. } = &mut l.kind {
                    a.start = start;
                }

        let mut flip = attachment.flip;
        if ui.checkbox(&mut flip, "Flip direction").changed() {
            history.snapshot();
            if let Some(l) = history.mutate().active_page_mut().find_mut(id)
                && let LayerKind::Text { path_attachment: Some(a), .. } = &mut l.kind {
                    a.flip = flip;
                }
        }
    }
}
