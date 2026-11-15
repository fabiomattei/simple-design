//! Sketch's unlabeled strip of the six Align icons, shared by the
//! single-layer Frame section and the multi-selection panel.

use egui::Ui;

use crate::alignment::AlignEdge;
use crate::ui::icons;

use super::InspectorAction;

/// The six Align icons in one row, appended to whatever row `ui` is (the
/// multi-selection panel adds the Distribute icons after them). Dimmed and
/// unclickable when `enabled` is false — e.g. a single top-level layer, which
/// has no parent to align to. Returns the clicked edge as an `Align` action,
/// carrying whether Option/Alt was held (align to the enclosing Artboard).
pub(super) fn align_strip(ui: &mut Ui, enabled: bool) -> Option<InspectorAction> {
    ui.add_enabled_ui(enabled, |ui| {
        let mut action = None;
        for edge in [
            AlignEdge::Left,
            AlignEdge::HCenter,
            AlignEdge::Right,
            AlignEdge::Top,
            AlignEdge::VCenter,
            AlignEdge::Bottom,
        ] {
            if icons::align_button(ui, edge).on_hover_text(align_label(edge)).clicked() {
                action = Some(InspectorAction::Align(edge, ui.input(|i| i.modifiers.alt)));
            }
        }
        action
    })
    .inner
}

fn align_label(edge: AlignEdge) -> &'static str {
    match edge {
        AlignEdge::Left => "Align left",
        AlignEdge::HCenter => "Align center horizontally",
        AlignEdge::Right => "Align right",
        AlignEdge::Top => "Align top",
        AlignEdge::VCenter => "Align center vertically",
        AlignEdge::Bottom => "Align bottom",
    }
}
