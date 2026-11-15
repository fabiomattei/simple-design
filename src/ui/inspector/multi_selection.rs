//! The Scale Layers mode panel and the multi-selection panel (align,
//! distribute, tidy, X/Y/W/H, flip, group, combine, flatten).

use egui::Ui;

use crate::alignment::DistributeAxis;
use crate::canvas::CanvasWidget;
use crate::history::History;
use crate::model::{BoolOp, LayerId};
use crate::ui::icons;

use super::align_strip::align_strip;
use super::geometry_widgets::flip_buttons;
use super::inspector_layout::{HeaderButton, section_header};
use super::multi_frame_fields::multi_frame_fields;
use super::InspectorAction;

/// Sketch's "Scale Layers" mode (`K`) — shown regardless of single/multi
/// selection, since it's a drag-handle mode on the canvas, not a
/// property editor. See `CanvasWidget::scaling`'s doc comment for the
/// three ways to exit (this Finish button is one of them).
pub(super) fn scale_layers_panel(ui: &mut Ui, canvas: &mut CanvasWidget) {
    if canvas.scaling.is_some() {
        ui.label(egui::RichText::new("Scale Layers").strong());
        ui.label("Resize a handle to scale the layer(s) and their stroke width/corner radius together.");
        ui.horizontal(|ui| {
            ui.label("Anchor:");
            ui.selectable_value(&mut canvas.scale_anchor, crate::canvas::ScaleAnchor::Corners, "Corners");
            ui.selectable_value(&mut canvas.scale_anchor, crate::canvas::ScaleAnchor::Center, "Center");
        });
        if ui.button("Finish").clicked() {
            canvas.scaling = None;
        }
        ui.separator();
    }
}

/// The panel shown instead of property fields when more than one layer is
/// selected; returns the action of whichever button was clicked.
pub(super) fn multi_selection_panel(
    ui: &mut Ui,
    history: &mut History,
    selection: &[LayerId],
    canvas: &mut CanvasWidget,
) -> Option<InspectorAction> {
    ui.label(format!("{} layers selected", selection.len()));
    section_header(ui, "Frame", HeaderButton::None);
    if let Some(action) = arrange_controls(ui, selection, canvas) {
        return Some(action);
    }
    ui.add_space(4.0);
    multi_frame_fields(ui, history, canvas, selection);

    ui.add_space(4.0);
    if let Some(action) = flip_buttons(ui) {
        return Some(action);
    }

    ui.add_space(8.0);
    ui.separator();
    if icons::duplicate_button(ui).on_hover_text("Duplicate (Cmd+D)").clicked() {
        return Some(InspectorAction::Duplicate);
    }
    if ui.button("Group (Cmd+G)").clicked() {
        return Some(InspectorAction::Group);
    }
    ui.add_space(8.0);
    ui.separator();
    let mut return_action = None;
    ui.label("Combine");
    ui.horizontal(|ui| {
        for op in [
            BoolOp::Union,
            BoolOp::Subtract,
            BoolOp::Intersect,
            BoolOp::Difference,
            BoolOp::Add,
        ] {
            if icons::combine_button(ui, op).on_hover_text(op.label()).clicked() {
                return_action = Some(InspectorAction::Boolean(op));
            }
        }
    });
    if return_action.is_some() {
        return return_action;
    }
    if icons::flatten_button(ui).on_hover_text("Flatten").clicked() {
        return Some(InspectorAction::Flatten);
    }
    None
}

/// The shared Align strip followed by the two Distribute icons, then the
/// Tidy row; returns the clicked action.
fn arrange_controls(ui: &mut Ui, selection: &[LayerId], canvas: &mut CanvasWidget) -> Option<InspectorAction> {
    let mut return_action = None;
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing.x = 2.0;
        return_action = align_strip(ui, true);
        ui.separator();
        let enabled = selection.len() >= 3;
        if icons::distribute_button(ui, DistributeAxis::Horizontal, enabled)
            .on_hover_text("Distribute horizontal spacing")
            .clicked()
        {
            return_action = Some(InspectorAction::Distribute(DistributeAxis::Horizontal));
        }
        if icons::distribute_button(ui, DistributeAxis::Vertical, enabled)
            .on_hover_text("Distribute vertical spacing")
            .clicked()
        {
            return_action = Some(InspectorAction::Distribute(DistributeAxis::Vertical));
        }
    });
    ui.add_space(4.0);
    ui.label("Tidy");
    ui.horizontal(|ui| {
        ui.add(egui::DragValue::new(&mut canvas.tidy_spacing).prefix("Spacing: ").range(0.0..=500.0));
        if ui.button("Tidy").on_hover_text("Arrange into an even grid").clicked() {
            return_action = Some(InspectorAction::Tidy(canvas.tidy_spacing));
        }
    });
    return_action
}
