use egui::widgets::color_picker;
use egui::{Color32, Response, Sense, Stroke, StrokeKind, Ui, Vec2};

use crate::model::Palette;

const SWATCH_SIZE: f32 = 22.0;

/// One palette swatch — a small colored square, hover-labeled with its name
/// (or hex if unnamed), reporting plain left-click and right-click. Shared
/// between `ui/palette_panel.rs`'s swatch grid and this module's popup so
/// the two can't visually drift apart.
pub fn swatch_button(ui: &mut Ui, color: Color32, name: &str) -> Response {
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(SWATCH_SIZE), Sense::click());
    ui.painter().rect_filled(rect, 3.0, color);
    ui.painter().rect_stroke(rect, 3.0, Stroke::new(1.0, Color32::from_black_alpha(80)), StrokeKind::Outside);
    let label = if name.is_empty() {
        format!("#{:02X}{:02X}{:02X}", color.r(), color.g(), color.b())
    } else {
        name.to_string()
    };
    response.on_hover_text(label)
}

/// Drop-in replacement for `ui.color_edit_button_srgba` that additionally
/// shows the document's `Palette` swatches at the top of the popup — click
/// one to apply it, same gesture as the standalone Palette panel
/// (`ui/palette_panel.rs`). Every fill/stroke/shadow/gradient-stop/etc.
/// color control in the inspector goes through this, so the palette is
/// reachable from wherever a color is being edited, not just its own panel.
///
/// Reuses egui's own `color_picker_color32` for the actual RGBA editor
/// (same `Alpha::BlendOrAdditive` mode `Ui::color_edit_button_srgba` uses)
/// rather than reimplementing it — only the button and popup shell are
/// custom, since egui's built-in popup has no hook to inject extra content.
pub fn edit(ui: &mut Ui, color: &mut Color32, palette: &Palette) -> Response {
    let popup_id = ui.auto_id_with("palette-color-popup");
    let open = egui::Popup::is_id_open(ui.ctx(), popup_id);
    let mut button_response = color_button(ui, *color, open);

    egui::Popup::menu(&button_response)
        .id(popup_id)
        .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
        .show(|ui| {
            if !palette.0.is_empty() {
                ui.horizontal_wrapped(|ui| {
                    for swatch in &palette.0 {
                        if swatch_button(ui, swatch.color, &swatch.name).clicked() {
                            *color = swatch.color;
                            button_response.mark_changed();
                        }
                    }
                });
                ui.separator();
            }
            ui.spacing_mut().slider_width = 275.0;
            if color_picker::color_picker_color32(ui, color, color_picker::Alpha::BlendOrAdditive) {
                button_response.mark_changed();
            }
        });

    button_response
}

/// Same look as egui's own (private) color-button, rebuilt from its public
/// building blocks (`show_color_at`) since we need our own popup content.
fn color_button(ui: &mut Ui, color: Color32, open: bool) -> Response {
    let size = ui.spacing().interact_size;
    let (rect, response) = ui.allocate_exact_size(size, Sense::click());
    if ui.is_rect_visible(rect) {
        let visuals = if open { &ui.visuals().widgets.open } else { ui.style().interact(&response) };
        let rect = rect.expand(visuals.expansion);
        color_picker::show_color_at(ui.painter(), color, rect.shrink(1.0));
        let corner_radius = visuals.corner_radius.at_most(2);
        ui.painter().rect_stroke(rect, corner_radius, (1.0, visuals.bg_fill), StrokeKind::Inside);
    }
    response
}
