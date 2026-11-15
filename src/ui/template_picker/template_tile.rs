//! One tile of the template picker's grid: a rounded card holding the template's preview (or
//! its initials when it has none) with the name below. It lifts on hover, like simple-text's
//! folder tiles, and a user template has a right-click menu.

use egui::epaint::Shadow;
use egui::text::LayoutJob;
use egui::{
    Align, Align2, Color32, CornerRadius, FontId, Painter, Rect, Sense, Stroke, StrokeKind, TextureHandle, Ui, pos2, vec2,
};

use crate::system_launch::FILE_MANAGER_NAME;

pub const TILE_WIDTH: f32 = 168.0;
const CARD_HEIGHT: f32 = 120.0;
const CARD_PADDING: f32 = 10.0;
const CARD_RADIUS: u8 = 8;
const NAME_GAP: f32 = 8.0;
/// Room for two rows of the name.
const NAME_HEIGHT: f32 = 34.0;
const NAME_SIZE: f32 = 13.0;
const NAME_ROWS: usize = 2;
/// A whole tile's height: the card plus the name below it.
pub const TILE_HEIGHT: f32 = CARD_HEIGHT + NAME_GAP + NAME_HEIGHT;
const LETTERS_SIZE: f32 = 34.0;
/// How far the card rises, and its shadow drops, when fully lifted.
const LIFT_HEIGHT: f32 = 3.0;
const LIFT_SECONDS: f32 = 0.12;
const SELECTION_PADDING: f32 = 5.0;
const SELECTION_RADIUS: u8 = 12;
const BADGE_SIZE: f32 = 18.0;

/// How a tile looks this frame.
pub struct TileLook<'a> {
    pub thumbnail: Option<&'a TextureHandle>,
    /// Chosen with the keyboard: ringed, and scrolled into view when `reveal`.
    pub selected: bool,
    pub reveal: bool,
    /// The number of its ⌘-digit shortcut, shown while ⌘ is held.
    pub shortcut: Option<usize>,
    /// A user template, which can be shown in the file manager or deleted.
    pub is_user: bool,
}

/// What the user did with a tile.
pub enum TileClick {
    Open,
    Reveal,
    Delete,
}

/// Draws the tile for the template `name` at the next place in `ui`; returns what was
/// clicked this frame.
pub fn template_tile(ui: &mut Ui, name: &str, look: TileLook) -> Option<TileClick> {
    let size = vec2(TILE_WIDTH, TILE_HEIGHT);
    let (rect, response) = ui.allocate_exact_size(size, Sense::click());
    if look.reveal {
        response.scroll_to_me(None);
    }
    let lift = ui.ctx().animate_bool_with_time(response.id.with("lift"), response.hovered(), LIFT_SECONDS);
    // Pressing pushes a lifted card most of the way back down.
    let lift = if response.is_pointer_button_down_on() { lift * 0.3 } else { lift };
    let card = Rect::from_min_size(rect.min, vec2(TILE_WIDTH, CARD_HEIGHT));
    let painter = ui.painter();
    if look.selected {
        painter.rect_stroke(
            card.expand(SELECTION_PADDING),
            CornerRadius::same(SELECTION_RADIUS),
            Stroke::new(2.0, crate::ui::theme::ACCENT),
            StrokeKind::Outside,
        );
    }
    paint_card(ui, painter, card.translate(vec2(0.0, -LIFT_HEIGHT * lift)), lift, name, look.thumbnail);
    if let Some(number) = look.shortcut {
        paint_badge(ui, painter, card, number);
    }
    paint_name(ui, painter, name, pos2(rect.center().x, card.bottom() + NAME_GAP));
    if response.clicked() {
        return Some(TileClick::Open);
    }
    let mut picked = None;
    if look.is_user {
        response.context_menu(|ui| {
            if ui.button(format!("Show in {FILE_MANAGER_NAME}")).clicked() {
                picked = Some(TileClick::Reveal);
            }
            if ui.button("Delete Template…").clicked() {
                picked = Some(TileClick::Delete);
            }
            if picked.is_some() {
                ui.close();
            }
        });
    }
    picked
}

/// The card over a soft shadow that widens with `lift` (0 to 1), holding the preview fitted
/// inside its padding, or the template's first two letters when it has none.
fn paint_card(ui: &Ui, painter: &Painter, card: Rect, lift: f32, name: &str, thumbnail: Option<&TextureHandle>) {
    let visuals = ui.visuals();
    let shadow_alpha = if visuals.dark_mode { 110.0 } else { 45.0 };
    let shadow = Shadow {
        offset: [0, (2.0 + 2.0 * LIFT_HEIGHT * lift) as i8],
        blur: (8.0 + 8.0 * lift) as u8,
        spread: 0,
        color: Color32::from_black_alpha((shadow_alpha * (1.0 - 0.3 * lift)) as u8),
    };
    painter.add(shadow.as_shape(card, CARD_RADIUS));
    let backdrop = crate::ui::theme::canvas_colors(visuals.dark_mode).backdrop;
    painter.rect_filled(card, CornerRadius::same(CARD_RADIUS), backdrop);
    painter.rect_stroke(card, CornerRadius::same(CARD_RADIUS), visuals.widgets.noninteractive.bg_stroke, StrokeKind::Inside);
    let Some(texture) = thumbnail else {
        let letters: String = name.chars().take(2).collect::<String>().to_lowercase();
        painter.text(card.center(), Align2::CENTER_CENTER, letters, FontId::proportional(LETTERS_SIZE), visuals.weak_text_color());
        return;
    };
    let area = card.shrink(CARD_PADDING);
    let image = texture.size_vec2();
    let scale = (area.width() / image.x).min(area.height() / image.y);
    let fitted = Rect::from_center_size(area.center(), image * scale);
    let uv = Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0));
    painter.image(texture.id(), fitted, uv, Color32::WHITE);
}

/// The shortcut's digit in a small round badge on the card's top right corner.
fn paint_badge(ui: &Ui, painter: &Painter, card: Rect, number: usize) {
    let visuals = ui.visuals();
    let radius = BADGE_SIZE / 2.0;
    let center = pos2(card.right() - radius / 2.0, card.top() + radius / 2.0);
    painter.circle_filled(center, radius, visuals.panel_fill);
    painter.circle_stroke(center, radius, Stroke::new(1.0, visuals.weak_text_color()));
    painter.text(center, Align2::CENTER_CENTER, number.to_string(), FontId::proportional(BADGE_SIZE * 0.6), visuals.text_color());
}

/// `name` centered under `top` (the middle of its top edge), in at most two rows ending in "…".
fn paint_name(ui: &Ui, painter: &Painter, name: &str, top: egui::Pos2) {
    let color = ui.visuals().text_color();
    let mut job = LayoutJob::simple(name.to_string(), FontId::proportional(NAME_SIZE), color, TILE_WIDTH);
    job.wrap.max_rows = NAME_ROWS;
    job.halign = Align::Center;
    painter.galley(top, painter.layout_job(job), color);
}
