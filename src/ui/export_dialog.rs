use egui::Vec2;

use crate::export::{ExportFormat, ExportSettings, PngCompression};
use crate::html_export::LayoutMode;
use crate::model::LayerId;

/// The export dialog's state. `layer_id`/`layer_name`/`layer_size` are
/// snapshotted when it opens, so changing the selection while it's open
/// doesn't retarget the export. `settings` persists across opens.
#[derive(Default)]
pub struct ExportDialogState {
    pub open: bool,
    pub settings: ExportSettings,
    /// Layout for "Export as HTML..."; persists across opens like `settings`.
    pub html_layout: LayoutMode,
    pub layer_id: Option<LayerId>,
    pub layer_name: String,
    /// The layer's rotated bounds in document units, for the pixel-size
    /// preview (excludes drop-shadow padding, which the export adds).
    pub layer_size: Vec2,
    /// The last failed export's message, shown until the next attempt.
    pub error: Option<String>,
}

pub enum ExportDialogAction {
    Export,
    /// HTML + CSS + images into a folder (see `html_export`); ignores `settings`.
    ExportHtml,
}

const SCALES: [f32; 4] = [0.5, 1.0, 2.0, 3.0];

/// Draws the dialog if `state.open`; returns `Some(Export)` or
/// `Some(ExportHtml)` the frame the user clicks the matching button. The
/// caller picks the path, writes the file(s), and either closes the dialog or
/// sets `state.error`.
pub fn ui(ctx: &egui::Context, state: &mut ExportDialogState) -> Option<ExportDialogAction> {
    if !state.open {
        return None;
    }
    let mut action = None;
    let mut still_open = state.open;
    egui::Window::new("Export").open(&mut still_open).resizable(false).collapsible(false).show(ctx, |ui| {
        ui.label(format!("Layer: {}", state.layer_name));
        ui.separator();
        settings_controls(ui, &mut state.settings);
        let pixel_size = state.layer_size * state.settings.scale;
        ui.weak(format!("Output: about {} × {} px", pixel_size.x.round(), pixel_size.y.round()));
        if let Some(error) = &state.error {
            ui.colored_label(ui.visuals().error_fg_color, error);
        }
        ui.separator();
        if ui.button("Export...").clicked() {
            action = Some(ExportDialogAction::Export);
        }
        ui.separator();
        ui.horizontal(|ui| {
            ui.label("HTML layout:");
            for layout in LayoutMode::ALL {
                ui.selectable_value(&mut state.html_layout, layout, layout.label());
            }
        });
        if ui.button("Export as HTML...").clicked() {
            action = Some(ExportDialogAction::ExportHtml);
        }
        ui.weak("HTML + CSS + images into a folder. Format and scale don't apply.");
    });
    state.open = state.open && still_open;
    action
}

fn settings_controls(ui: &mut egui::Ui, settings: &mut ExportSettings) {
    ui.horizontal(|ui| {
        ui.label("Format:");
        for format in ExportFormat::ALL {
            ui.selectable_value(&mut settings.format, format, format.label());
        }
    });
    ui.horizontal(|ui| {
        ui.label("Scale:");
        for scale in SCALES {
            ui.selectable_value(&mut settings.scale, scale, format!("{scale}x"));
        }
    });
    match settings.format {
        ExportFormat::Png => {
            egui::ComboBox::from_label("Compression")
                .selected_text(settings.png_compression.label())
                .show_ui(ui, |ui| {
                    for compression in PngCompression::ALL {
                        ui.selectable_value(&mut settings.png_compression, compression, compression.label());
                    }
                });
        }
        ExportFormat::Jpeg => {
            ui.add(egui::Slider::new(&mut settings.jpeg_quality, 1..=100).text("Quality"));
            ui.weak("Lower quality = smaller file. Transparency becomes white.");
        }
        ExportFormat::Webp => {
            ui.weak("Lossless. Use Scale to reduce file size.");
        }
    }
}
