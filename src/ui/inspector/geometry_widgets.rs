//! Small reusable inspector widgets for geometry fields: the artboard size
//! preset picker, arrow-cap combos, flip buttons and the per-corner radius field.

use egui::Ui;
use crate::history::History;
use crate::model::{ArrowCap, ArtboardPreset, CornerRadii, LayerId, LayerKind, PAPER_PRESETS, SCREEN_PRESETS};
use crate::transform_ops::FlipAxis;
use crate::ui::icons;

use super::{InspectorAction, should_snapshot};

/// Shown in place of "No selection" while the Artboard tool is active and
/// nothing has been drawn yet: a fixed-size alternative to dragging out a
/// custom-size artboard on the canvas (still available, unchanged).
pub(super) fn artboard_preset_picker(ui: &mut Ui) -> Option<InspectorAction> {
    ui.label("New Artboard");
    ui.weak("Choose a size, or drag on the canvas for a custom one.");
    ui.add_space(8.0);

    let mut action = None;
    let mut preset_group = |ui: &mut Ui, label: &str, presets: &[ArtboardPreset]| {
        ui.label(label);
        ui.horizontal_wrapped(|ui| {
            for preset in presets {
                let text = format!("{}\n{}\u{d7}{}", preset.name, preset.size.x as i32, preset.size.y as i32);
                if ui.button(text).clicked() {
                    action = Some(InspectorAction::InsertArtboardPreset(preset.size));
                }
            }
        });
        ui.add_space(4.0);
    };
    preset_group(ui, "Screen", SCREEN_PRESETS);
    preset_group(ui, "Paper", PAPER_PRESETS);
    action
}

/// The "Flip" row shared by both the single- and multi-selection branches of
/// `ui()` below — flipping is well-defined (per-layer, about each layer's own
/// frame center, see `transform_ops::flip_layer`) regardless of how many
/// layers are selected, unlike Align/Distribute.
pub(super) fn arrow_cap_label(cap: ArrowCap) -> &'static str {
    match cap {
        ArrowCap::None => "None",
        ArrowCap::Line => "Line",
        ArrowCap::Triangle => "Triangle",
        ArrowCap::Disc => "Disc",
    }
}

/// A small `ArrowCap` picker, `salt` making the two (start/end) instances on
/// one layer distinct widget ids. Returns whether the selection changed.
pub(super) fn arrow_cap_combo(ui: &mut Ui, salt: (&str, LayerId), cap: &mut ArrowCap) -> bool {
    let mut changed = false;
    egui::ComboBox::from_id_salt(salt)
        .selected_text(arrow_cap_label(*cap))
        .width(90.0)
        .show_ui(ui, |ui| {
            for choice in [ArrowCap::None, ArrowCap::Line, ArrowCap::Triangle, ArrowCap::Disc] {
                if ui.selectable_value(cap, choice, arrow_cap_label(choice)).changed() {
                    changed = true;
                }
            }
        });
    changed
}

pub(super) fn flip_buttons(ui: &mut Ui) -> Option<InspectorAction> {
    ui.label("Flip");
    ui.horizontal(flip_icon_buttons).inner
}

/// Just the two Flip icons, unlabeled — laid out in whatever row `ui` is,
/// e.g. beside the single-layer Rotate field.
pub(super) fn flip_icon_buttons(ui: &mut Ui) -> Option<InspectorAction> {
    let mut action = None;
    if icons::flip_button(ui, FlipAxis::Horizontal)
        .on_hover_text("Flip Horizontal (⇧H)")
        .clicked()
    {
        action = Some(InspectorAction::Flip(FlipAxis::Horizontal));
    }
    if icons::flip_button(ui, FlipAxis::Vertical)
        .on_hover_text("Flip Vertical (⇧V)")
        .clicked()
    {
        action = Some(InspectorAction::Flip(FlipAxis::Vertical));
    }
    action
}

/// One corner's `DragValue` in the `Rectangle` inspector's corner-radius
/// grid — reads `value` (that corner's current radius) and, on change,
/// writes back through `apply` (e.g. `|c, v| c.top_left = v`) into whichever
/// corner of `CornerRadii` it closes over.
pub(super) fn corner_radius_field(
    ui: &mut Ui,
    history: &mut History,
    id: LayerId,
    corner: icons::RectCorner,
    value: f32,
    apply: impl Fn(&mut CornerRadii, f32),
) {
    let mut v = value;
    let resp = ui
        .horizontal(|ui| {
            icons::corner_radius_icon(ui, corner);
            ui.add(egui::DragValue::new(&mut v).range(0.0..=1000.0))
        })
        .inner;
    if should_snapshot(&resp) {
        history.snapshot();
    }
    if resp.changed()
        && let Some(l) = history.mutate().active_page_mut().find_mut(id)
            && let LayerKind::Rectangle { corner_radius } = &mut l.kind {
                apply(corner_radius, v);
            }
}
