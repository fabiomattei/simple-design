//! The shared Layer Style library section: capture a layer's fill, stroke,
//! opacities and shadows as a reusable style, and apply, update or detach one.

use egui::Ui;
use crate::history::History;
use crate::model::{Layer, LayerId, LayerStyle};

/// Captures `layer.style`'s current fields as a new named `LayerStyle`,
/// keyed by `style_id`.
pub(super) fn layer_style_from_layer(name: String, style_id: uuid::Uuid, layer: &Layer) -> LayerStyle {
    LayerStyle {
        id: style_id,
        name,
        fill: layer.style.fill.clone(),
        stroke: layer.style.stroke.clone(),
        fill_opacity: layer.style.fill_opacity,
        stroke_opacity: layer.style.stroke_opacity,
        shadows: layer.style.shadows.clone(),
        inner_shadows: layer.style.inner_shadows.clone(),
    }
}

/// Copies every field of `style` onto `layer.style` and links `layer.style_id`.
/// `frame`/`kind`/`opacity` are untouched. Used both when applying a style to
/// a layer and, in `layer_style_ui`'s "Update Style", when propagating an
/// edited style back to every other layer linked to it.
pub(super) fn apply_layer_style(layer: &mut Layer, style: &LayerStyle) {
    layer.style.fill = style.fill.clone();
    layer.style.stroke = style.stroke.clone();
    layer.style.fill_opacity = style.fill_opacity;
    layer.style.stroke_opacity = style.stroke_opacity;
    layer.style.shadows = style.shadows.clone();
    layer.style.inner_shadows = style.inner_shadows.clone();
    layer.style_id = Some(style.id);
}

/// Sketch's "Layer Styles": a small shared/linked style library
/// (`Document::layer_styles`) covering Fill/Border/Shadow, the shape-side
/// sibling of `text_style_ui`. Applying a style copies its fields onto
/// `layer.style` and links `style_id`; "Update Style" edits the style itself
/// and propagates to every other layer linked to it
/// (`Document::for_each_layer_mut`); "Detach" unlinks without changing the
/// layer's current values. Right-clicking a style chip opens a small manager
/// (rename/delete) — deleting a style unlinks every layer that referenced it
/// rather than leaving a dangling `style_id`.
pub(super) fn layer_style_ui(ui: &mut Ui, history: &mut History, id: LayerId, style_id: Option<uuid::Uuid>) {
    ui.add_space(4.0);
    ui.separator();
    ui.label("Layer Style");

    let styles = history.get().layer_styles.clone();
    if !styles.is_empty() {
        ui.horizontal_wrapped(|ui| {
            for style in &styles {
                let active = style_id == Some(style.id);
                let resp = ui.selectable_label(active, &style.name);
                if resp.clicked() && !active {
                    history.snapshot();
                    if let Some(l) = history.mutate().active_page_mut().find_mut(id) {
                        apply_layer_style(l, style);
                    }
                }
                resp.context_menu(|ui| {
                    let name_buf_id = ui.id().with(("rename_layer_style", style.id));
                    let mut name_buf = ui.memory_mut(|m| {
                        m.data
                            .get_persisted_mut_or_insert_with(name_buf_id, || style.name.clone())
                            .clone()
                    });
                    ui.horizontal(|ui| {
                        ui.label("Name:");
                        let name_resp = ui.add(egui::TextEdit::singleline(&mut name_buf).desired_width(100.0));
                        if name_resp.changed() {
                            ui.memory_mut(|m| *m.data.get_persisted_mut_or_default::<String>(name_buf_id) = name_buf.clone());
                        }
                        let trimmed = name_buf.trim();
                        if name_resp.lost_focus() && !trimmed.is_empty() && trimmed != style.name {
                            history.snapshot();
                            if let Some(existing) =
                                history.mutate().layer_styles.iter_mut().find(|s| s.id == style.id)
                            {
                                existing.name = trimmed.to_string();
                            }
                        }
                    });
                    if ui.button("Delete Style").clicked() {
                        history.snapshot();
                        let doc = history.mutate();
                        doc.layer_styles.retain(|s| s.id != style.id);
                        doc.for_each_layer_mut(&mut |l| {
                            if l.style_id == Some(style.id) {
                                l.style_id = None;
                            }
                        });
                        ui.close();
                    }
                });
            }
        });
    }

    let name_buf_id = ui.id().with("new_layer_style_name");
    let mut name_buf = ui.memory_mut(|m| m.data.get_persisted_mut_or_default::<String>(name_buf_id).clone());
    ui.horizontal(|ui| {
        ui.add(egui::TextEdit::singleline(&mut name_buf).hint_text("Style name"));
        if ui.add_enabled(!name_buf.trim().is_empty(), egui::Button::new("Save as New Style")).clicked() {
            history.snapshot();
            let new_id = uuid::Uuid::new_v4();
            if let Some(layer) = history.get().find(id) {
                let style = layer_style_from_layer(name_buf.trim().to_string(), new_id, layer);
                history.mutate().layer_styles.push(style);
                if let Some(l) = history.mutate().active_page_mut().find_mut(id) {
                    l.style_id = Some(new_id);
                }
                name_buf.clear();
            }
        }
    });
    ui.memory_mut(|m| *m.data.get_persisted_mut_or_default::<String>(name_buf_id) = name_buf);

    if let Some(sid) = style_id {
        ui.horizontal(|ui| {
            if ui
                .button("Update Style")
                .on_hover_text("Push this layer's current fill/border/shadow to every layer linked to this style")
                .clicked()
            {
                history.snapshot();
                let updated = history
                    .get()
                    .find(id)
                    .zip(history.get().layer_styles.iter().find(|s| s.id == sid).map(|s| s.name.clone()))
                    .map(|(layer, name)| layer_style_from_layer(name, sid, layer));
                if let Some(updated) = updated {
                    let doc = history.mutate();
                    if let Some(existing) = doc.layer_styles.iter_mut().find(|s| s.id == sid) {
                        *existing = updated.clone();
                    }
                    doc.for_each_layer_mut(&mut |l| {
                        if l.style_id == Some(sid) {
                            apply_layer_style(l, &updated);
                        }
                    });
                }
            }
            if ui.button("Detach").clicked() {
                history.snapshot();
                if let Some(l) = history.mutate().active_page_mut().find_mut(id) {
                    l.style_id = None;
                }
            }
        });
    }
}
