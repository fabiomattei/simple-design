use egui::Ui;

use crate::alignment::{AlignEdge, DistributeAxis};
use crate::arrange::ArrangeCommand;
use crate::tools::Tool;
use crate::ui::{arrange_icons, icons};

pub enum ToolbarAction {
    InsertImage,
    /// The edge to align to, and whether Option/Alt was held (align to the
    /// enclosing Artboard) — same meaning as `InspectorAction::Align`.
    Align(AlignEdge, bool),
    Distribute(DistributeAxis),
    Arrange(ArrangeCommand),
}

/// Draws the tool row, updating `tool` in place on selection and returning any
/// non-tool action (Insert Image, align, distribute, layer order).
/// `selection_len` only dims buttons that would do nothing.
pub fn ui(ui: &mut Ui, tool: &mut Tool, selection_len: usize) -> Option<ToolbarAction> {
    let mut action = None;
    ui.horizontal(|ui| {
        for t in Tool::ALL {
            let selected = *tool == t;
            if icons::tool_button(ui, t, selected)
                .on_hover_text(t.label())
                .clicked()
            {
                *tool = t;
            }
        }
        ui.separator();
        if icons::icon_button(ui, icons::draw_image)
            .on_hover_text("Insert Image...")
            .clicked()
        {
            action = Some(ToolbarAction::InsertImage);
        }
        ui.separator();
        if let Some(selection_action) = selection_buttons(ui, selection_len) {
            action = Some(selection_action);
        }
    });
    action
}

/// The Align, Distribute and layer-order buttons, in that order.
fn selection_buttons(ui: &mut Ui, selection_len: usize) -> Option<ToolbarAction> {
    let mut action = None;
    ui.add_enabled_ui(selection_len > 0, |ui| {
        for edge in [
            AlignEdge::Left,
            AlignEdge::HCenter,
            AlignEdge::Right,
            AlignEdge::Top,
            AlignEdge::VCenter,
            AlignEdge::Bottom,
        ] {
            if icons::align_button(ui, edge).on_hover_text(align_label(edge)).clicked() {
                action = Some(ToolbarAction::Align(edge, ui.input(|i| i.modifiers.alt)));
            }
        }
    });
    ui.separator();
    for (axis, label) in [
        (DistributeAxis::Horizontal, "Distribute horizontal spacing"),
        (DistributeAxis::Vertical, "Distribute vertical spacing"),
    ] {
        if icons::distribute_button(ui, axis, selection_len >= 3).on_hover_text(label).clicked() {
            action = Some(ToolbarAction::Distribute(axis));
        }
    }
    ui.separator();
    ui.add_enabled_ui(selection_len > 0, |ui| {
        for command in ArrangeCommand::ALL {
            let draw = |painter: &egui::Painter, rect, color| arrange_icons::draw_arrange_icon(painter, command, rect, color);
            if icons::icon_button(ui, draw).on_hover_text(command.label()).clicked() {
                action = Some(ToolbarAction::Arrange(command));
            }
        }
    });
    action
}

fn align_label(edge: AlignEdge) -> &'static str {
    match edge {
        AlignEdge::Left => "Align left (⌥: to artboard)",
        AlignEdge::HCenter => "Align center horizontally (⌥: to artboard)",
        AlignEdge::Right => "Align right (⌥: to artboard)",
        AlignEdge::Top => "Align top (⌥: to artboard)",
        AlignEdge::VCenter => "Align center vertically (⌥: to artboard)",
        AlignEdge::Bottom => "Align bottom (⌥: to artboard)",
    }
}
