//! Help > About Simple Design: a modal with the app's icon, version, authors and license, all
//! from `app_info`.

use egui::{Align, Layout, RichText, TextureHandle, TextureOptions};

use crate::app_info;

/// Side of the icon in the dialog, in points.
const ICON_SIZE: f32 = 96.0;

#[derive(Default)]
pub struct AboutDialogState {
    pub open: bool,
    /// The app icon, decoded the first time the dialog opens.
    icon: Option<TextureHandle>,
}

/// Draws the dialog if `state.open`. It closes on its Close button, Escape, or a click outside.
pub fn ui(ctx: &egui::Context, state: &mut AboutDialogState) {
    if !state.open {
        return;
    }
    let icon = state
        .icon
        .get_or_insert_with(|| {
            let image = crate::image_ops::decode(app_info::APP_ICON_PNG).expect("embedded app icon is a valid PNG");
            ctx.load_texture("about-icon", crate::image_ops::to_egui_color_image(&image), TextureOptions::LINEAR)
        })
        .clone();

    let response = egui::Modal::new(egui::Id::new("about_dialog")).show(ctx, |ui| {
        ui.set_width(360.0);
        let mut close = false;
        ui.vertical_centered(|ui| {
            ui.add(egui::Image::new(&icon).fit_to_exact_size(egui::vec2(ICON_SIZE, ICON_SIZE)));
            ui.add_space(4.0);
            ui.label(RichText::new(app_info::NAME).heading().strong());
            ui.label(format!("Version {}", app_info::VERSION));
            ui.add_space(8.0);
            ui.label(app_info::DESCRIPTION);
        });
        ui.add_space(8.0);
        ui.separator();
        egui::Grid::new("about_details").num_columns(2).spacing([12.0, 6.0]).show(ui, |ui| {
            ui.weak("Author");
            ui.label(app_info::authors());
            ui.end_row();
            ui.weak("License");
            ui.label(app_info::license_name());
            ui.end_row();
            ui.weak("Website");
            ui.hyperlink(app_info::HOMEPAGE);
            ui.end_row();
            ui.weak("Built with");
            ui.label("Rust, egui and eframe");
            ui.end_row();
        });
        ui.separator();
        ui.vertical_centered(|ui| {
            ui.weak(format!("Copyright © {} {}", app_info::FIRST_YEAR, app_info::authors()));
            ui.weak("This program comes with ABSOLUTELY NO WARRANTY.");
        });
        ui.add_space(8.0);
        ui.with_layout(Layout::right_to_left(Align::Min), |ui| {
            close = ui.button("Close").clicked();
        });
        close
    });
    if response.inner || response.should_close() {
        state.open = false;
    }
}
