use egui::{Color32, Painter, Rect, Stroke, StrokeKind, Ui};

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
    /// Same as the ⌘⇧M "Use as Mask" shortcut (`App::toggle_mask_selection`).
    ToggleMask,
    /// Opens the export dialog for the single selected layer (`ui/export_dialog.rs`).
    Export,
}

/// Draws the tool row, updating `tool` in place on selection and returning any
/// non-tool action (Insert Image, align, distribute, layer order, mask, export).
/// `selection_len` only dims buttons that would do nothing; `selection_is_mask`
/// (exactly one layer selected, and it's a mask) shows the mask button pressed.
pub fn ui(ui: &mut Ui, tool: &mut Tool, selection_len: usize, selection_is_mask: bool) -> Option<ToolbarAction> {
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
        if let Some(selection_action) = selection_buttons(ui, selection_len, selection_is_mask) {
            action = Some(selection_action);
        }
    });
    action
}

/// The Align, Distribute, layer-order, mask and export buttons, in that order.
fn selection_buttons(ui: &mut Ui, selection_len: usize, selection_is_mask: bool) -> Option<ToolbarAction> {
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
        ui.separator();
        if icons::toggle_icon_button(ui, selection_is_mask, draw_mask_icon).on_hover_text("Use as Mask (⌘⇧M)").clicked() {
            action = Some(ToolbarAction::ToggleMask);
        }
    });
    ui.add_enabled_ui(selection_len == 1, |ui| {
        if icons::icon_button(ui, draw_export_icon).on_hover_text("Export... (⌘E)").clicked() {
            action = Some(ToolbarAction::Export);
        }
    });
    action
}

/// A frame outline with a solid disc inside: the disc is the shape that
/// clips, the frame the content it clips.
fn draw_mask_icon(painter: &Painter, rect: Rect, color: Color32) {
    let frame = rect.shrink(rect.width() * 0.12);
    painter.rect_stroke(frame, 2.0, Stroke::new(1.6, color), StrokeKind::Middle);
    painter.circle_filled(frame.center(), frame.width() * 0.28, color);
}

/// An open tray with an arrow leaving it upward.
fn draw_export_icon(painter: &Painter, rect: Rect, color: Color32) {
    let stroke = Stroke::new(1.6, color);
    let inner = rect.shrink(rect.width() * 0.12);
    let tray_top = inner.top() + inner.height() * 0.55;
    painter.line(vec![inner.left_top() + egui::vec2(0.0, tray_top - inner.top()), inner.left_bottom(), inner.right_bottom(), egui::pos2(inner.right(), tray_top)], stroke);
    let arrow_bottom = egui::pos2(inner.center().x, inner.bottom() - inner.height() * 0.2);
    let arrow_top = egui::pos2(inner.center().x, inner.top());
    let head = inner.width() * 0.25;
    painter.line_segment([arrow_bottom, arrow_top], stroke);
    painter.line(vec![arrow_top + egui::vec2(-head, head), arrow_top, arrow_top + egui::vec2(head, head)], stroke);
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
