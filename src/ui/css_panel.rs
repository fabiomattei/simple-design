//! Read-only CSS view of the selection: one `css_rules::layer_css` block per
//! selected layer, with a button copying all of them to the clipboard.

use egui::{RichText, Ui};

use crate::css_rules::layer_css;
use crate::model::{Document, LayerId};

/// Draws the CSS panel for `selection`. Side effect: "Copy" writes the
/// shown text to the system clipboard.
pub fn ui(ui: &mut Ui, document: &Document, selection: &[LayerId]) {
    crate::ui::theme::panel_header(ui, "CSS", false);

    let blocks: Vec<String> = selection.iter().filter_map(|&id| document.find(id)).map(layer_css).collect();
    if blocks.is_empty() {
        ui.weak("No selection");
        return;
    }
    let css = blocks.join("\n\n");

    if ui.button("Copy").clicked() {
        ui.ctx().copy_text(css.clone());
    }
    ui.add_space(4.0);
    egui::ScrollArea::both().show(ui, |ui| {
        ui.add(egui::Label::new(RichText::new(css).monospace()).selectable(true).extend());
    });
}
