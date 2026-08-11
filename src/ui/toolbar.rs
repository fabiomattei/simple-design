use egui::Ui;

use crate::tools::Tool;
use crate::ui::icons;

pub enum ToolbarAction {
    InsertImage,
}

/// Draws the tool row, updating `tool` in place on selection and returning any non-tool action (e.g. Insert Image).
pub fn ui(ui: &mut Ui, tool: &mut Tool) -> Option<ToolbarAction> {
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
    });
    action
}
