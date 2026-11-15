use egui::{Align, Color32, CursorIcon, Layout, Painter, Rect, Sense, Stroke, Ui, UiBuilder};

use crate::panel_layout::{DockSide, PanelKind};
use crate::ui::icons;

/// Vertical icon strip pinned to one outer window edge (see `app/panel_columns.rs`, which
/// adds it before that side's panel column so it stays outermost), with one icon per panel
/// docked on `side`, in the given order. Clicking an icon directly toggles its panel's
/// visibility bool — no action enum, since there's no history/side effect to route through
/// `app.rs` (same direct-mutation convention as the View menu's checkboxes). Right-clicking
/// one offers moving it to the opposite side; that choice is returned instead, since the
/// caller persists it (`PanelLayout::set_side`).
pub fn ui(ui: &mut Ui, side: DockSide, panels: &mut [(PanelKind, &mut bool)]) -> Option<PanelKind> {
    let move_label = match side {
        DockSide::Left => "Move to Right Side",
        DockSide::Right => "Move to Left Side",
    };
    let mut moved = None;
    ui.vertical(|ui| {
        ui.add_space(4.0);
        for (panel, open) in panels.iter_mut() {
            let response = icons::toggle_icon_button(ui, **open, icon_for(*panel)).on_hover_text(panel.label());
            if response.clicked() {
                **open = !**open;
            }
            response.context_menu(|ui| {
                if ui.button(move_label).clicked() {
                    moved = Some(*panel);
                    ui.close();
                }
            });
        }
    });
    moved
}

fn icon_for(panel: PanelKind) -> fn(&Painter, Rect, Color32) {
    match panel {
        PanelKind::Layers => icons::draw_layers_icon,
        PanelKind::Palette => icons::draw_palette_icon,
        PanelKind::Inspector => icons::draw_inspector_icon,
        PanelKind::Minimap => icons::draw_minimap_icon,
    }
}

const SPLITTER_HEIGHT: f32 = 6.0;
const SPLITTER_MIN_PANEL_HEIGHT: f32 = 60.0;

/// Docks one panel's content into its side's shared column (`app/panel_columns.rs`),
/// stacked top-to-bottom like IntelliJ IDEA's same-side tool windows. Every panel but the
/// bottommost currently open one (`is_last`) gets a fixed height plus a mouse-draggable
/// splitter strip below it; the bottommost one just renders directly into whatever height is
/// left, so the stack always fills the column with no dead space at the bottom.
///
/// Implemented as a plain drag-sensed strip (not a nested `egui::Panel`) — nesting a
/// resizable `Panel` inside another resizable `Panel`'s content `Ui` left the inner one's
/// resize handle completely unresponsive to hover, so this sidesteps that interaction
/// entirely with a self-contained widget.
///
/// The content area is a hard-clipped child `Ui` built from an explicitly reserved rect
/// (`ui.allocate_exact_size`), not `Ui::allocate_ui` — `allocate_ui`'s desired size is only a
/// suggestion ("if the contents overflow, more space will be allocated"), so taller-than-`height`
/// content (e.g. a long layer list) would silently grow the reserved space and the drag would
/// have no visible effect.
pub fn stacked_panel(ui: &mut Ui, id_salt: &'static str, height: &mut f32, is_last: bool, add_contents: impl FnOnce(&mut Ui)) {
    if is_last {
        add_contents(ui);
        return;
    }

    let width = ui.available_width();
    let (content_rect, _) = ui.allocate_exact_size(egui::vec2(width, *height), Sense::hover());
    let mut content_ui = ui.new_child(
        UiBuilder::new()
            .id_salt(id_salt)
            .max_rect(content_rect)
            .layout(Layout::top_down(Align::Min)),
    );
    content_ui.set_clip_rect(content_rect);
    add_contents(&mut content_ui);

    let (rect, _) = ui.allocate_exact_size(egui::vec2(width, SPLITTER_HEIGHT), Sense::hover());
    let response = ui.interact(rect, ui.id().with(id_salt).with("splitter"), Sense::drag());
    if ui.is_rect_visible(rect) {
        let color = if response.dragged() || response.hovered() {
            ui.visuals().selection.stroke.color
        } else {
            ui.visuals().widgets.noninteractive.bg_stroke.color
        };
        ui.painter()
            .line_segment([rect.left_center(), rect.right_center()], Stroke::new(1.5, color));
    }
    if response.hovered() || response.dragged() {
        ui.ctx().set_cursor_icon(CursorIcon::ResizeVertical);
    }
    *height = (*height + response.drag_delta().y).max(SPLITTER_MIN_PANEL_HEIGHT);
}
