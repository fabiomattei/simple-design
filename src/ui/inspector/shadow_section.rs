//! The Drop Shadow / Inner Shadow sections: a list of stacked shadows,
//! each with color, offset, blur and spread.

use egui::Ui;
use crate::history::History;
use crate::model::{Layer, LayerId, Shadow};
use crate::ui::color_picker;

use super::should_snapshot;

/// A stacked list of drop ("outer") or inner shadows (`heading` labels which)
/// — same clone-locally/mutate/write-back-through-`apply` pattern as
/// `paint_editor`'s gradient-stops list, since a `Vec<Shadow>` needs the same
/// per-row edit + add/remove shape. Each row's color popover doubles as the
/// shadow's opacity (its alpha channel — see `Shadow`'s doc comment).
pub(super) fn shadow_section(ui: &mut Ui, history: &mut History, id: LayerId, heading: &str, shadows: &[Shadow], apply: impl Fn(&mut Layer, Vec<Shadow>)) {
    ui.add_space(4.0);
    ui.label(heading);
    let mut list = shadows.to_vec();
    let mut changed = false;
    let mut snapshot_now = false;
    let mut remove_idx: Option<usize> = None;

    for (i, shadow) in list.iter_mut().enumerate() {
        ui.horizontal(|ui| {
            let cr = color_picker::edit(ui, &mut shadow.color, &history.get().palette);
            if should_snapshot(&cr) {
                snapshot_now = true;
            }
            changed |= cr.changed();

            let xr = ui.add(egui::DragValue::new(&mut shadow.offset.x).prefix("X: ").speed(0.5));
            if should_snapshot(&xr) {
                snapshot_now = true;
            }
            changed |= xr.changed();

            let yr = ui.add(egui::DragValue::new(&mut shadow.offset.y).prefix("Y: ").speed(0.5));
            if should_snapshot(&yr) {
                snapshot_now = true;
            }
            changed |= yr.changed();

            let br = ui.add(egui::DragValue::new(&mut shadow.blur).prefix("Blur: ").range(0.0..=500.0).speed(0.5));
            if should_snapshot(&br) {
                snapshot_now = true;
            }
            changed |= br.changed();

            let sr = ui.add(egui::DragValue::new(&mut shadow.spread).prefix("Spread: ").speed(0.5));
            if should_snapshot(&sr) {
                snapshot_now = true;
            }
            changed |= sr.changed();

            if ui.small_button("🗑").clicked() {
                remove_idx = Some(i);
                changed = true;
                snapshot_now = true;
            }
        });
    }
    if let Some(i) = remove_idx {
        list.remove(i);
    }
    if ui.small_button(format!("+ {heading}")).clicked() {
        list.push(Shadow::default());
        changed = true;
        snapshot_now = true;
    }

    if snapshot_now {
        history.snapshot();
    }
    if changed
        && let Some(l) = history.mutate().active_page_mut().find_mut(id) {
            apply(l, list);
        }
}
