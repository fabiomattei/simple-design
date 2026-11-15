//! The shared Text Style library section: capture a `Text` layer's
//! typography as a reusable style, and apply, update or detach one.

use egui::Ui;
use crate::canvas::apply_text_auto_resize;
use crate::history::History;
use crate::model::{Layer, LayerId, LayerKind, TextStyle};

/// Captures `layer`'s current text properties (everything a `TextStyle`
/// holds) as a new named style, keyed by `style_id`.
pub(super) fn text_style_from_layer(name: String, style_id: uuid::Uuid, layer: &Layer) -> Option<TextStyle> {
    let LayerKind::Text {
        font_size,
        font,
        align,
        vertical_align,
        line_height,
        letter_spacing,
        paragraph_spacing,
        bold,
        italic,
        underline,
        strikethrough,
        transform,
        list,
        list_start,
        ..
    } = &layer.kind
    else {
        return None;
    };
    Some(TextStyle {
        id: style_id,
        name,
        font_size: *font_size,
        font: font.clone(),
        align: *align,
        vertical_align: *vertical_align,
        line_height: *line_height,
        letter_spacing: *letter_spacing,
        paragraph_spacing: *paragraph_spacing,
        bold: *bold,
        italic: *italic,
        underline: *underline,
        strikethrough: *strikethrough,
        transform: *transform,
        list: *list,
        list_start: *list_start,
        fill: layer.style.fill.as_ref().map(crate::model::Paint::to_color32),
    })
}

/// Copies every field of `style` onto `layer` — both `LayerKind::Text`'s
/// fields and the outer `Layer::style.fill` — and links it via `style_id`.
/// `content` and geometry are untouched. Used both when applying a style to
/// a layer and, in `text_style_ui`'s "Update Style", when propagating an
/// edited style back to every other layer linked to it.
pub(super) fn apply_style_to_text(layer: &mut Layer, style: &TextStyle) {
    layer.style.fill = style.fill.map(crate::model::Paint::Solid);
    if let LayerKind::Text {
        font_size,
        font,
        align,
        vertical_align,
        line_height,
        letter_spacing,
        paragraph_spacing,
        bold,
        italic,
        underline,
        strikethrough,
        transform,
        list,
        list_start,
        style_id,
        ..
    } = &mut layer.kind
    {
        *font_size = style.font_size;
        *font = style.font.clone();
        *align = style.align;
        *vertical_align = style.vertical_align;
        *line_height = style.line_height;
        *letter_spacing = style.letter_spacing;
        *paragraph_spacing = style.paragraph_spacing;
        *bold = style.bold;
        *italic = style.italic;
        *underline = style.underline;
        *strikethrough = style.strikethrough;
        *transform = style.transform;
        *list = style.list;
        *list_start = style.list_start;
        *style_id = Some(style.id);
    }
}

/// Sketch's "Text Styles": a small shared/linked style library
/// (`Document::text_styles`). Applying a style copies its fields onto the
/// layer and links `style_id`; "Update Style" edits the style itself and
/// propagates to every other layer linked to it (`Document::for_each_layer_mut`);
/// "Detach" unlinks without changing the layer's current values.
pub(super) fn text_style_ui(ui: &mut Ui, history: &mut History, id: LayerId, style_id: Option<uuid::Uuid>) {
    let ctx = ui.ctx().clone();
    ui.add_space(4.0);
    ui.separator();
    ui.label("Text Style");

    let styles = history.get().text_styles.clone();
    if !styles.is_empty() {
        ui.horizontal_wrapped(|ui| {
            for style in &styles {
                let active = style_id == Some(style.id);
                if ui.selectable_label(active, &style.name).clicked() && !active {
                    history.snapshot();
                    if let Some(l) = history.mutate().active_page_mut().find_mut(id) {
                        apply_style_to_text(l, style);
                        apply_text_auto_resize(&ctx, l);
                    }
                }
            }
        });
    }

    let name_buf_id = ui.id().with("new_text_style_name");
    let mut name_buf = ui.memory_mut(|m| m.data.get_persisted_mut_or_default::<String>(name_buf_id).clone());
    ui.horizontal(|ui| {
        ui.add(egui::TextEdit::singleline(&mut name_buf).hint_text("Style name"));
        if ui.add_enabled(!name_buf.trim().is_empty(), egui::Button::new("Save as New Style")).clicked() {
            history.snapshot();
            let new_id = uuid::Uuid::new_v4();
            if let Some(layer) = history.get().find(id)
                && let Some(style) = text_style_from_layer(name_buf.trim().to_string(), new_id, layer) {
                    history.mutate().text_styles.push(style);
                    if let Some(l) = history.mutate().active_page_mut().find_mut(id)
                        && let LayerKind::Text { style_id, .. } = &mut l.kind {
                            *style_id = Some(new_id);
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
                .on_hover_text("Push this layer's current text properties to every layer linked to this style")
                .clicked()
            {
                history.snapshot();
                let updated = history
                    .get()
                    .find(id)
                    .zip(history.get().text_styles.iter().find(|s| s.id == sid).map(|s| s.name.clone()))
                    .and_then(|(layer, name)| text_style_from_layer(name, sid, layer));
                if let Some(updated) = updated {
                    let doc = history.mutate();
                    if let Some(existing) = doc.text_styles.iter_mut().find(|s| s.id == sid) {
                        *existing = updated.clone();
                    }
                    doc.for_each_layer_mut(&mut |l| {
                        let linked = matches!(&l.kind, LayerKind::Text { style_id: Some(s), .. } if *s == sid);
                        if linked {
                            apply_style_to_text(l, &updated);
                            apply_text_auto_resize(&ctx, l);
                        }
                    });
                }
            }
            if ui.button("Detach").clicked() {
                history.snapshot();
                if let Some(l) = history.mutate().active_page_mut().find_mut(id)
                    && let LayerKind::Text { style_id, .. } = &mut l.kind {
                        *style_id = None;
                    }
            }
        });
    }
}
