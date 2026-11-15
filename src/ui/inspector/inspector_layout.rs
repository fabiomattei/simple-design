//! Sketch-style layout primitives shared by the inspector sections: a
//! section header with an optional add/remove button, and a numeric field
//! with a small caption under it.

use egui::{Align, Layout, Response, RichText, Ui, Vec2, Widget};

/// Width of a geometry field (X, Y, W, H, Rotate).
pub(super) const FIELD_WIDTH: f32 = 64.0;
/// Width of a shadow field (X, Y, Blur, Spread) — narrower so a whole
/// shadow fits on one row.
pub(super) const SMALL_FIELD_WIDTH: f32 = 40.0;
const FIELD_HEIGHT: f32 = 20.0;

/// The button on the right of a section header.
pub(super) enum HeaderButton {
    None,
    Add,
    Remove,
}

/// A hairline, then `title` in bold with `button` right-aligned on the same
/// row. Returns whether `button` was clicked (always `false` for `None`).
pub(super) fn section_header(ui: &mut Ui, title: &str, button: HeaderButton) -> bool {
    ui.add_space(6.0);
    ui.separator();
    let mut clicked = false;
    ui.horizontal(|ui| {
        ui.label(RichText::new(title).strong());
        let (symbol, hint) = match button {
            HeaderButton::None => return,
            HeaderButton::Add => ("+", format!("Add {title}")),
            HeaderButton::Remove => ("−", format!("Remove {title}")),
        };
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            clicked = ui.small_button(symbol).on_hover_text(hint).clicked();
        });
    });
    clicked
}

/// `widget` at a fixed `width`, with `caption` centered in small weak text
/// under it. Returns the widget's response.
pub(super) fn captioned_field(ui: &mut Ui, caption: &str, width: f32, widget: impl Widget) -> Response {
    ui.allocate_ui_with_layout(Vec2::new(width, 0.0), Layout::top_down(Align::Center), |ui| {
        ui.spacing_mut().item_spacing.y = 1.0;
        let response = ui.add_sized([width, FIELD_HEIGHT], widget);
        ui.label(RichText::new(caption).small().weak());
        response
    })
    .inner
}
