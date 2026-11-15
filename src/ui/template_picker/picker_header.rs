//! The top of the template picker: its title with a Cancel button, and the filter field that
//! keeps the keyboard focus.

use egui::{Align, FontId, Id, Key, Layout, Modifiers, RichText, TextEdit, Ui, Vec2};

const TITLE_SIZE: f32 = 24.0;
const FILTER_WIDTH: f32 = 360.0;
const FILTER_TEXT_SIZE: f32 = 14.0;
const FILTER_MARGIN: f32 = 6.0;

/// The id of the filter field, so the picker can keep the keyboard focus on it.
pub fn filter_id() -> Id {
    Id::new("template_picker_filter")
}

/// "New from Template" on the left, a Cancel button on the right; returns whether it was clicked.
pub fn title_row(ui: &mut Ui) -> bool {
    let mut cancel = false;
    ui.horizontal(|ui| {
        ui.label(RichText::new("New from Template").size(TITLE_SIZE).strong());
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            cancel = ui.button("Cancel").clicked();
        });
    });
    cancel
}

/// The filter field. Escape empties it.
pub fn filter_field(ui: &mut Ui, text: &mut String) {
    if !text.is_empty() && ui.input_mut(|input| input.consume_key(Modifiers::NONE, Key::Escape)) {
        text.clear();
    }
    ui.add(
        TextEdit::singleline(text)
            .id(filter_id())
            .hint_text("Filter templates — arrow keys to choose, Enter to open")
            .font(FontId::proportional(FILTER_TEXT_SIZE))
            .margin(Vec2::splat(FILTER_MARGIN))
            .desired_width(FILTER_WIDTH),
    );
}
