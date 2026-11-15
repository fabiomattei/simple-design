//! The Drop Shadow / Inner Shadow sections: a list of stacked shadows,
//! each with color, offset, blur and spread.

use egui::Ui;
use crate::history::History;
use crate::model::{Layer, LayerId, Shadow};
use crate::ui::color_picker;

use super::inspector_layout::{HeaderButton, SMALL_FIELD_WIDTH, captioned_field, section_header};
use super::should_snapshot;

/// A stacked list of drop ("outer") or inner shadows (`heading` labels which)
/// — same clone-locally/mutate/write-back-through-`apply` pattern as
/// `paint_editor`'s gradient-stops list, since a `Vec<Shadow>` needs the same
/// per-row edit + add/remove shape. Each row's color popover doubles as the
/// shadow's opacity (its alpha channel — see `Shadow`'s doc comment).
/// Sketch-style: a header whose + appends a shadow, then one row per shadow
/// with captioned X/Y/Blur/Spread fields and a − to remove it.
pub(super) fn shadow_section(ui: &mut Ui, history: &mut History, id: LayerId, heading: &str, shadows: &[Shadow], apply: impl Fn(&mut Layer, Vec<Shadow>)) {
    let mut list = shadows.to_vec();
    let mut changed = false;
    let mut snapshot_now = false;
    let mut remove_idx: Option<usize> = None;

    if section_header(ui, heading, HeaderButton::Add) {
        list.push(Shadow::default());
        changed = true;
        snapshot_now = true;
    }

    for (i, shadow) in list.iter_mut().enumerate() {
        ui.horizontal_top(|ui| {
            let cr = color_picker::edit(ui, &mut shadow.color, &history.get().palette);
            if should_snapshot(&cr) {
                snapshot_now = true;
            }
            changed |= cr.changed();

            let xr = captioned_field(ui, "X", SMALL_FIELD_WIDTH, egui::DragValue::new(&mut shadow.offset.x).speed(0.5));
            if should_snapshot(&xr) {
                snapshot_now = true;
            }
            changed |= xr.changed();

            let yr = captioned_field(ui, "Y", SMALL_FIELD_WIDTH, egui::DragValue::new(&mut shadow.offset.y).speed(0.5));
            if should_snapshot(&yr) {
                snapshot_now = true;
            }
            changed |= yr.changed();

            let br = captioned_field(ui, "Blur", SMALL_FIELD_WIDTH, egui::DragValue::new(&mut shadow.blur).range(0.0..=500.0).speed(0.5));
            if should_snapshot(&br) {
                snapshot_now = true;
            }
            changed |= br.changed();

            let sr = captioned_field(ui, "Spread", SMALL_FIELD_WIDTH, egui::DragValue::new(&mut shadow.spread).speed(0.5));
            if should_snapshot(&sr) {
                snapshot_now = true;
            }
            changed |= sr.changed();

            if ui.small_button("−").on_hover_text("Remove shadow").clicked() {
                remove_idx = Some(i);
                changed = true;
                snapshot_now = true;
            }
        });
    }
    if let Some(i) = remove_idx {
        list.remove(i);
    }

    if snapshot_now {
        history.snapshot();
    }
    if changed
        && let Some(l) = history.mutate().active_page_mut().find_mut(id) {
            apply(l, list);
        }
}
