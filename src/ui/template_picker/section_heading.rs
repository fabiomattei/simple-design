//! The template picker's section heading, as on simple-text's welcome screen: the title in
//! spaced capitals, the number of templates in a pill, and a thin rule to the right edge.

use egui::text::{LayoutJob, TextFormat};
use egui::{CornerRadius, FontId, Rect, Sense, Stroke, Ui, pos2, vec2};

const HEADING_HEIGHT: f32 = 24.0;
const HEADING_SIZE: f32 = 11.5;
const HEADING_LETTER_SPACING: f32 = 1.2;
const PILL_PADDING: f32 = 6.0;
const PILL_HEIGHT: f32 = 16.0;
const HEADING_GAP: f32 = 8.0;

/// A full-width heading for a section of `count` templates.
pub fn section_heading(ui: &mut Ui, title: &str, count: usize) {
    let size = vec2(ui.available_width(), HEADING_HEIGHT);
    let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
    let visuals = ui.visuals();
    let text_color = visuals.text_color();
    let weak_color = visuals.weak_text_color();
    let pill_fill = visuals.widgets.inactive.bg_fill;
    let painter = ui.painter();
    let format = TextFormat {
        font_id: FontId::proportional(HEADING_SIZE),
        color: text_color,
        extra_letter_spacing: HEADING_LETTER_SPACING,
        ..Default::default()
    };
    let galley = painter.layout_job(LayoutJob::single_section(title.to_uppercase(), format));
    let title_top = rect.center().y - galley.size().y / 2.0;
    let title_right = rect.left() + galley.size().x;
    painter.galley(pos2(rect.left(), title_top), galley, text_color);
    let count_galley = painter.layout_no_wrap(count.to_string(), FontId::proportional(HEADING_SIZE - 1.0), weak_color);
    let pill = Rect::from_min_size(
        pos2(title_right + HEADING_GAP, rect.center().y - PILL_HEIGHT / 2.0),
        vec2(count_galley.size().x + PILL_PADDING * 2.0, PILL_HEIGHT),
    );
    painter.rect_filled(pill, CornerRadius::same((PILL_HEIGHT / 2.0) as u8), pill_fill);
    painter.galley(pill.center() - count_galley.size() / 2.0, count_galley, weak_color);
    if pill.right() + HEADING_GAP < rect.right() {
        painter.hline((pill.right() + HEADING_GAP)..=rect.right(), rect.center().y, Stroke::new(1.0, pill_fill));
    }
    ui.add_space(HEADING_GAP);
}
