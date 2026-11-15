//! The Image section of the inspector: file actions, "Edit Image" mode,
//! pixel operations and Color Adjust.

use egui::Ui;

use crate::canvas::CanvasWidget;
use crate::history::History;
use crate::model::{ColorAdjust, Layer, LayerId, LayerKind};
use crate::ui::color_picker;

use super::should_snapshot;

/// The Image section of a single selected `Image` layer: file actions, the
/// "Edit Image" mode controls, pixel operations and Color Adjust. Draws
/// nothing for other kinds.
pub(super) fn image_section(ui: &mut Ui, history: &mut History, canvas: &mut CanvasWidget, layer: &Layer) {
    let LayerKind::Image { width, height, color_adjust, .. } = &layer.kind else {
        return;
    };
    let id = layer.id;
    ui.add_space(8.0);
    ui.separator();
    ui.label(egui::RichText::new("Image").strong());
    ui.label(format!("{width} × {height} px"));

    image_file_buttons(ui, history, id);
    image_edit_controls(ui, history, canvas, id);
    pixel_operations(ui, history, id);
    color_adjust_controls(ui, history, id, color_adjust);
}

/// Replace Image and Reset to Original Size.
fn image_file_buttons(ui: &mut Ui, history: &mut History, id: LayerId) {
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        if ui.button("Replace Image...").clicked() {
            replace_image(history, id);
        }
        if ui.button("Reset to Original Size").clicked() {
            history.snapshot();
            if let Some(l) = history.mutate().active_page_mut().find_mut(id)
                && let LayerKind::Image { width, height, .. } = &l.kind {
                    l.frame.size = egui::Vec2::new(*width as f32, *height as f32);
                }
        }
    });
}

/// "Edit Image" mode: wand tolerance, crop/delete/fill/clear on the current
/// selection, or the button that enters the mode.
fn image_edit_controls(ui: &mut Ui, history: &mut History, canvas: &mut CanvasWidget, id: LayerId) {
    ui.add_space(4.0);
    if canvas.image_edit_active_for(id) {
        ui.separator();
        ui.label("Edit Image — drag to select, click for Magic Wand");
        ui.weak("Shift: add to selection · Option: subtract");

        let mut tolerance = canvas.image_edit_tolerance();
        let tol_resp = ui.add(egui::Slider::new(&mut tolerance, 0.0..=100.0).text("Wand tolerance"));
        if tol_resp.changed() {
            canvas.set_image_edit_tolerance(tolerance);
        }

        let has_selection = canvas.image_edit_has_selection();
        ui.horizontal(|ui| {
            if ui.add_enabled(has_selection, egui::Button::new("Crop to Selection")).clicked() {
                canvas.apply_image_edit_crop(history);
            }
            if ui.add_enabled(has_selection, egui::Button::new("Delete")).clicked() {
                canvas.apply_image_edit_delete(history);
            }
        });
        ui.horizontal(|ui| {
            let mut fill_color = canvas.image_edit_fill_color();
            if color_picker::edit(ui, &mut fill_color, &history.get().palette).changed() {
                canvas.set_image_edit_fill_color(fill_color);
            }
            if ui.add_enabled(has_selection, egui::Button::new("Fill")).clicked() {
                canvas.apply_image_edit_fill(history, fill_color);
            }
            if ui.add_enabled(has_selection, egui::Button::new("Clear Selection")).clicked() {
                canvas.clear_image_edit_selection();
            }
        });
        if ui.button("Done Editing").clicked() {
            canvas.end_image_edit();
        }
    } else if ui.button("Edit Image...").clicked() {
        canvas.begin_image_edit(history, id);
    }
}

/// Remove Background, Trim Transparent Pixels and Minimize File Size.
fn pixel_operations(ui: &mut Ui, history: &mut History, id: LayerId) {
    ui.add_space(4.0);
    if ui.button("Remove Background").clicked() {
        history.snapshot();
        if let Some(l) = history.mutate().active_page_mut().find_mut(id)
            && let LayerKind::Image { encoded, .. } = &l.kind
                && let Some(decoded) = crate::image_ops::decode(encoded) {
                    let out = crate::image_ops::remove_background(&decoded, BACKGROUND_REMOVAL_TOLERANCE);
                    if let LayerKind::Image { encoded, version, .. } = &mut l.kind {
                        *encoded = crate::image_ops::encode_png(&out);
                        *version = uuid::Uuid::new_v4();
                    }
                }
    }
    if ui.button("Trim Transparent Pixels").clicked() {
        history.snapshot();
        if let Some(l) = history.mutate().active_page_mut().find_mut(id)
            && let LayerKind::Image { encoded, .. } = &l.kind
                && let Some(decoded) = crate::image_ops::decode(encoded)
                    && let Some((trimmed, ox, oy)) = crate::image_ops::trim_transparent(&decoded) {
                        crate::image_ops::apply_cropped_image(l, &trimmed, ox, oy);
                    }
    }
    if ui.button("Minimize File Size").on_hover_text(
        "Resamples the image down to its current on-screen pixel size, discarding excess resolution."
    ).clicked() {
        history.snapshot();
        if let Some(l) = history.mutate().active_page_mut().find_mut(id) {
            crate::image_ops::minimize_image_file_size(l);
        }
    }
}

fn color_adjust_controls(ui: &mut Ui, history: &mut History, id: LayerId, color_adjust: &ColorAdjust) {
    ui.add_space(8.0);
    ui.separator();
    ui.label("Color Adjust");
    let mut adjust = *color_adjust;
    let hue_r = ui.add(egui::Slider::new(&mut adjust.hue, -180.0..=180.0).text("Hue"));
    let sat_r = ui.add(egui::Slider::new(&mut adjust.saturation, -1.0..=1.0).text("Saturation"));
    let bri_r = ui.add(egui::Slider::new(&mut adjust.brightness, -1.0..=1.0).text("Brightness"));
    let con_r = ui.add(egui::Slider::new(&mut adjust.contrast, -1.0..=1.0).text("Contrast"));
    if should_snapshot(&hue_r) || should_snapshot(&sat_r) || should_snapshot(&bri_r) || should_snapshot(&con_r) {
        history.snapshot();
    }
    if (hue_r.changed() || sat_r.changed() || bri_r.changed() || con_r.changed())
        && let Some(l) = history.mutate().active_page_mut().find_mut(id)
            && let LayerKind::Image { color_adjust, .. } = &mut l.kind {
                *color_adjust = adjust;
            }
    if ui.button("Reset Color Adjust").clicked() {
        history.snapshot();
        if let Some(l) = history.mutate().active_page_mut().find_mut(id)
            && let LayerKind::Image { color_adjust, .. } = &mut l.kind {
                *color_adjust = ColorAdjust::default();
            }
    }
}

/// Heuristic Remove Background's color tolerance (see
/// `image_ops::remove_background`) — fixed rather than user-tunable, since
/// Sketch's own "automatically remove background" has no manual slider
/// either (unlike Magic Wand, which does).
const BACKGROUND_REMOVAL_TOLERANCE: f32 = 24.0;

/// "Replace Image": swaps `id`'s pixel content for a file picked via a
/// dialog, keeping the current frame (so the new image stretches/shrinks to
/// fit — "Reset to Original Size" undoes that if unwanted, matching
/// Sketch's own Replace Image behavior).
fn replace_image(history: &mut History, id: LayerId) {
    let Some(mut paths) = crate::io::open_image_dialog() else { return };
    let Some(path) = paths.drain(..).next() else { return };
    let Ok(bytes) = std::fs::read(&path) else { return };
    let Some(decoded) = crate::image_ops::decode(&bytes) else { return };
    history.snapshot();
    if let Some(l) = history.mutate().active_page_mut().find_mut(id)
        && let LayerKind::Image { encoded, width, height, version, .. } = &mut l.kind {
            *encoded = crate::image_ops::encode_png(&decoded);
            *width = decoded.width();
            *height = decoded.height();
            *version = uuid::Uuid::new_v4();
        }
}
