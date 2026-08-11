use egui::{Color32, Ui};

use crate::model::Palette;
use crate::ui::color_picker::swatch_button;

/// What the user asked for — `app.rs` is the only place that applies these
/// to `History`/`Document`, same convention as `LayerAction`/`PagesAction`.
pub enum PaletteAction {
    /// Set every selected layer's fill to this swatch — Aseprite's "click a
    /// palette entry to pick a color" behavior.
    Apply(Color32),
    /// Add the primary selection's current fill color as a new swatch.
    AddFromSelection,
    Remove(usize),
    /// Opens a file picker and loads an Aseprite-compatible `.gpl` file.
    Import,
    /// Opens a file picker and saves the current palette as `.gpl`.
    Export,
    ResetToDefault,
}

/// Draws the Palette panel and returns the action the user requested, if any.
pub fn ui(ui: &mut Ui, palette: &Palette, has_selection: bool) -> Option<PaletteAction> {
    let mut action = None;

    ui.heading("Palette");
    ui.separator();

    egui::ScrollArea::vertical().show(ui, |ui| {
        ui.horizontal_wrapped(|ui| {
            for (index, swatch) in palette.0.iter().enumerate() {
                let response = swatch_button(ui, swatch.color, &swatch.name);
                if response.clicked() {
                    action = Some(PaletteAction::Apply(swatch.color));
                }
                if response.secondary_clicked() {
                    action = Some(PaletteAction::Remove(index));
                }
            }
        });
    });

    ui.add_space(6.0);
    ui.horizontal_wrapped(|ui| {
        if ui.add_enabled(has_selection, egui::Button::new("Add from Selection")).clicked() {
            action = Some(PaletteAction::AddFromSelection);
        }
        if ui.button("Import...").clicked() {
            action = Some(PaletteAction::Import);
        }
        if ui.button("Export...").clicked() {
            action = Some(PaletteAction::Export);
        }
        if ui.button("Reset to Default").clicked() {
            action = Some(PaletteAction::ResetToDefault);
        }
    });
    ui.weak("Click a swatch to apply as fill · Right-click to remove");

    action
}
