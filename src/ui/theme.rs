//! App-wide look and feel: one dark and one light `egui::Style`, installed for both
//! `egui::Theme`s so the UI follows the OS color scheme, plus the matching colors for the
//! canvas chrome (backdrop, rulers, minimap) that `canvas.rs`/`minimap_panel.rs` paint by
//! hand rather than through egui widgets.

use egui::{Color32, CornerRadius, Margin, RichText, Stroke, Theme, Ui, Vec2, Visuals};

use crate::ui::icons;

/// Shared accent — same blue as `canvas.rs`'s `SELECTION_COLOR`, so selected widgets in the
/// panels read as the same "selection" as handles on the canvas.
pub const ACCENT: Color32 = Color32::from_rgb(0, 122, 255);

/// Colors for the parts of the canvas area that aren't the document itself.
pub struct CanvasColors {
    /// The infinite workspace behind artboards.
    pub backdrop: Color32,
    pub ruler_bg: Color32,
    pub ruler_line: Color32,
    pub ruler_text: Color32,
}

pub fn canvas_colors(dark_mode: bool) -> CanvasColors {
    if dark_mode {
        CanvasColors {
            backdrop: Color32::from_gray(30),
            ruler_bg: Color32::from_gray(44),
            ruler_line: Color32::from_gray(70),
            ruler_text: Color32::from_gray(150),
        }
    } else {
        CanvasColors {
            backdrop: Color32::from_gray(229),
            ruler_bg: Color32::from_gray(250),
            ruler_line: Color32::from_gray(205),
            ruler_text: Color32::from_gray(110),
        }
    }
}

/// Installs both themes' styles on `ctx`. Call once at startup.
pub fn install(ctx: &egui::Context) {
    ctx.set_visuals_of(Theme::Dark, dark_visuals());
    ctx.set_visuals_of(Theme::Light, light_visuals());
    ctx.all_styles_mut(|style| {
        let spacing = &mut style.spacing;
        spacing.item_spacing = Vec2::new(6.0, 5.0);
        spacing.button_padding = Vec2::new(8.0, 3.0);
        spacing.interact_size.y = 22.0;
        spacing.menu_margin = Margin::same(6);
        spacing.window_margin = Margin::same(10);
        spacing.scroll.bar_width = 6.0;
        spacing.scroll.floating = true;
    });
}

struct Palette {
    panel: Color32,
    window: Color32,
    input: Color32,
    faint: Color32,
    border: Color32,
    inactive: Color32,
    hovered: Color32,
    active: Color32,
    text: Color32,
    text_strong: Color32,
}

fn dark_visuals() -> Visuals {
    themed(
        Visuals::dark(),
        Palette {
            panel: Color32::from_gray(40),
            window: Color32::from_gray(44),
            input: Color32::from_gray(32),
            faint: Color32::from_gray(46),
            border: Color32::from_gray(58),
            inactive: Color32::from_gray(54),
            hovered: Color32::from_gray(64),
            active: Color32::from_gray(74),
            text: Color32::from_gray(205),
            text_strong: Color32::from_gray(240),
        },
    )
}

fn light_visuals() -> Visuals {
    themed(
        Visuals::light(),
        Palette {
            panel: Color32::from_gray(255),
            window: Color32::from_gray(255),
            input: Color32::from_gray(245),
            faint: Color32::from_gray(248),
            border: Color32::from_gray(222),
            inactive: Color32::from_gray(238),
            hovered: Color32::from_gray(228),
            active: Color32::from_gray(216),
            text: Color32::from_gray(50),
            text_strong: Color32::from_gray(15),
        },
    )
}

fn themed(mut v: Visuals, p: Palette) -> Visuals {
    let radius = CornerRadius::same(5);

    v.panel_fill = p.panel;
    v.window_fill = p.window;
    v.extreme_bg_color = p.input;
    v.text_edit_bg_color = Some(p.input);
    v.faint_bg_color = p.faint;
    v.code_bg_color = p.input;
    v.window_stroke = Stroke::new(1.0, p.border);
    v.window_corner_radius = CornerRadius::same(8);
    v.menu_corner_radius = CornerRadius::same(6);
    v.hyperlink_color = ACCENT;
    v.selection.bg_fill = ACCENT.gamma_multiply(if v.dark_mode { 0.55 } else { 0.25 });
    v.selection.stroke = Stroke::new(1.0, if v.dark_mode { Color32::from_rgb(110, 175, 255) } else { ACCENT });

    let w = &mut v.widgets;
    w.noninteractive.bg_fill = p.panel;
    w.noninteractive.weak_bg_fill = p.panel;
    w.noninteractive.bg_stroke = Stroke::new(1.0, p.border);
    w.noninteractive.fg_stroke = Stroke::new(1.0, p.text);
    w.noninteractive.corner_radius = radius;

    for (state, fill) in [
        (&mut w.inactive, p.inactive),
        (&mut w.hovered, p.hovered),
        (&mut w.active, p.active),
        (&mut w.open, p.active),
    ] {
        state.bg_fill = fill;
        state.weak_bg_fill = fill;
        state.corner_radius = radius;
        state.expansion = 0.0;
    }
    w.inactive.bg_stroke = Stroke::NONE;
    w.inactive.fg_stroke = Stroke::new(1.0, p.text);
    w.hovered.bg_stroke = Stroke::new(1.0, p.border);
    w.hovered.fg_stroke = Stroke::new(1.5, p.text_strong);
    w.active.bg_stroke = Stroke::new(1.0, ACCENT);
    w.active.fg_stroke = Stroke::new(2.0, p.text_strong);
    w.open.bg_stroke = Stroke::new(1.0, p.border);
    w.open.fg_stroke = Stroke::new(1.0, p.text_strong);
    v
}

/// The title row shared by the docked right-hand panels: a compact bold label, an optional
/// close "X" on the right, and a hairline under it. Returns whether the X was clicked.
pub fn panel_header(ui: &mut Ui, title: &str, closable: bool) -> bool {
    let mut close_clicked = false;
    ui.add_space(2.0);
    ui.horizontal(|ui| {
        ui.label(RichText::new(title).strong().size(13.0));
        if closable {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                close_clicked = icons::close_button(ui).clicked();
            });
        }
    });
    ui.separator();
    close_clicked
}
