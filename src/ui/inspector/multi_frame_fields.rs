//! X/Y and W/lock/H fields for a multi-selection, editing the selection's
//! combined bounding box (see `selection_transform`).

use egui::{Rect, Ui};

use crate::canvas::CanvasWidget;
use crate::history::History;
use crate::model::LayerId;
use crate::selection_transform::{selection_bounds, set_selection_bounds};

use super::inspector_layout::{FIELD_WIDTH, captioned_field};
use super::should_snapshot;

/// Draws the X/Y and W/H rows for `selection`. Editing X/Y moves every
/// layer; editing W/H scales them all inside the box, honoring the shared
/// proportion lock (`canvas.aspect_locked`). Draws nothing if none of the
/// selected layers exist.
pub(super) fn multi_frame_fields(ui: &mut Ui, history: &mut History, canvas: &mut CanvasWidget, selection: &[LayerId]) {
    let Some(bounds) = selection_bounds(history.get().active_page(), selection) else {
        return;
    };
    let (mut x, mut y) = (bounds.min.x, bounds.min.y);
    let (mut w, mut h) = (bounds.width(), bounds.height());

    let (rx, ry) = ui
        .horizontal_top(|ui| {
            let rx = captioned_field(ui, "X", FIELD_WIDTH, egui::DragValue::new(&mut x));
            let ry = captioned_field(ui, "Y", FIELD_WIDTH, egui::DragValue::new(&mut y));
            (rx, ry)
        })
        .inner;
    let (rw, rh) = ui
        .horizontal_top(|ui| {
            let rw = captioned_field(ui, "W", FIELD_WIDTH, egui::DragValue::new(&mut w).range(0.0..=f32::MAX));
            let lock_icon = if canvas.aspect_locked { "🔒" } else { "🔓" };
            if ui.small_button(lock_icon).on_hover_text("Lock proportions (⌥⌘L)").clicked() {
                canvas.aspect_locked = !canvas.aspect_locked;
            }
            let rh = captioned_field(ui, "H", FIELD_WIDTH, egui::DragValue::new(&mut h).range(0.0..=f32::MAX));
            (rw, rh)
        })
        .inner;

    if [&rx, &ry, &rw, &rh].into_iter().any(should_snapshot) {
        history.snapshot();
    }
    if !(rx.changed() || ry.changed() || rw.changed() || rh.changed()) {
        return;
    }
    // Same cascade as the single-layer W/H fields: the field the user
    // didn't touch follows the other's ratio change.
    if canvas.aspect_locked {
        if rw.changed() && bounds.width() != 0.0 {
            h = bounds.height() * (w / bounds.width());
        } else if rh.changed() && bounds.height() != 0.0 {
            w = bounds.width() * (h / bounds.height());
        }
    }
    let new_bounds = Rect::from_min_size(egui::pos2(x, y), egui::vec2(w, h));
    set_selection_bounds(history.mutate().active_page_mut(), selection, new_bounds);
}
