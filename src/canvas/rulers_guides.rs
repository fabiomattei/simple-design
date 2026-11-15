//! Rulers, persisted guides and the pixel grid drawn around and over the
//! canvas, plus hit-testing a guide under the pointer.

use egui::{Color32, Pos2, Rect, Stroke as EguiStroke, Vec2};

use crate::model::{Guide, GuideOrientation};

use super::{CanvasWidget, SELECTION_COLOR};

/// Color of a placed, static ruler guide.
const GUIDE_COLOR: Color32 = Color32::from_rgb(0, 170, 255);
/// How close (in screen pixels) the pointer must be to an existing guide
/// line to grab it for moving/deleting instead of starting a new one.
const GUIDE_HIT_SCREEN: f32 = 4.0;
/// Pixel grid lines only render once zoomed in enough that they're at least
/// a few screen pixels apart; below this, drawing one line per doc pixel
/// would be dense enough to look like solid fill and cost real overdraw.
const PIXEL_GRID_MIN_ZOOM: f32 = 4.0;
const PIXEL_GRID_COLOR: Color32 = Color32::from_black_alpha(40);

impl CanvasWidget {
    /// Index of the guide whose line passes within `GUIDE_HIT_SCREEN` of
    /// `mouse`, if any. `mouse` must already be known to be within the
    /// canvas area (rulers are handled separately, as guide-creation).
    pub(super) fn hovered_guide_index(&self, origin: Pos2, guides: &[Guide], mouse: Pos2) -> Option<usize> {
        guides.iter().position(|g| match g.orientation {
            GuideOrientation::Horizontal => {
                let sy = self.to_screen(origin, Pos2::new(0.0, g.pos)).y;
                (mouse.y - sy).abs() <= GUIDE_HIT_SCREEN
            }
            GuideOrientation::Vertical => {
                let sx = self.to_screen(origin, Pos2::new(g.pos, 0.0)).x;
                (mouse.x - sx).abs() <= GUIDE_HIT_SCREEN
            }
        })
    }
}

/// Draws one placed/in-progress guide line at doc-space `value`, spanning
/// the canvas area. Shared by the persisted-guides pass and the live
/// creating/moving preview.
pub(super) fn draw_guide_line(
    painter: &egui::Painter,
    canvas_rect: Rect,
    origin: Pos2,
    pan: Vec2,
    zoom: f32,
    orientation: GuideOrientation,
    value: f32,
) {
    let to_screen = |p: Pos2| origin + pan + p.to_vec2() * zoom;
    let stroke = EguiStroke::new(1.0, GUIDE_COLOR);
    match orientation {
        GuideOrientation::Horizontal => {
            let sy = to_screen(Pos2::new(0.0, value)).y;
            painter.line_segment(
                [Pos2::new(canvas_rect.min.x, sy), Pos2::new(canvas_rect.max.x, sy)],
                stroke,
            );
        }
        GuideOrientation::Vertical => {
            let sx = to_screen(Pos2::new(value, 0.0)).x;
            painter.line_segment(
                [Pos2::new(sx, canvas_rect.min.y), Pos2::new(sx, canvas_rect.max.y)],
                stroke,
            );
        }
    }
}

pub(super) fn draw_guides(
    painter: &egui::Painter,
    canvas_rect: Rect,
    origin: Pos2,
    pan: Vec2,
    zoom: f32,
    guides: &[Guide],
) {
    for g in guides {
        draw_guide_line(painter, canvas_rect, origin, pan, zoom, g.orientation, g.pos);
    }
}

/// Draws a line at every visible integer doc-space pixel, once zoomed in
/// enough (see `PIXEL_GRID_MIN_ZOOM`) that individual pixels are actually
/// distinguishable rather than a dense wash.
pub(super) fn draw_pixel_grid(painter: &egui::Painter, canvas_rect: Rect, origin: Pos2, pan: Vec2, zoom: f32) {
    if zoom < PIXEL_GRID_MIN_ZOOM {
        return;
    }
    let to_screen = |p: Pos2| origin + pan + p.to_vec2() * zoom;
    let to_doc = |p: Pos2| ((p - origin - pan) / zoom).to_pos2();
    let doc_min = to_doc(canvas_rect.min);
    let doc_max = to_doc(canvas_rect.max);
    let stroke = EguiStroke::new(1.0, PIXEL_GRID_COLOR);

    let x0 = doc_min.x.floor() as i64;
    let x1 = doc_max.x.ceil() as i64;
    for x in x0..=x1 {
        let sx = to_screen(Pos2::new(x as f32, 0.0)).x;
        painter.line_segment([Pos2::new(sx, canvas_rect.min.y), Pos2::new(sx, canvas_rect.max.y)], stroke);
    }
    let y0 = doc_min.y.floor() as i64;
    let y1 = doc_max.y.ceil() as i64;
    for y in y0..=y1 {
        let sy = to_screen(Pos2::new(0.0, y as f32)).y;
        painter.line_segment([Pos2::new(canvas_rect.min.x, sy), Pos2::new(canvas_rect.max.x, sy)], stroke);
    }
}

/// Picks a "nice" doc-space spacing (1/2/5 × a power of ten) between major
/// ruler ticks, targeting roughly `target_screen` screen pixels between them
/// at the given zoom.
pub(super) fn nice_ruler_step(zoom: f32, target_screen: f32) -> f32 {
    let raw = (target_screen / zoom).max(1e-3);
    let magnitude = 10f32.powf(raw.log10().floor());
    let residual = raw / magnitude;
    let nice = if residual > 5.0 {
        10.0
    } else if residual > 2.0 {
        5.0
    } else if residual > 1.0 {
        2.0
    } else {
        1.0
    };
    nice * magnitude
}

/// Draws the top and left ruler strips (tick marks + doc-space coordinate
/// labels tracking the current pan/zoom), the corner box where they meet,
/// and a thin highlight tracking the pointer's current position.
pub(super) fn draw_rulers(
    painter: &egui::Painter,
    full_rect: Rect,
    canvas_rect: Rect,
    origin: Pos2,
    pan: Vec2,
    zoom: f32,
    hover_pos: Option<Pos2>,
) {
    let colors = crate::ui::theme::canvas_colors(painter.ctx().global_style().visuals.dark_mode);
    let to_screen = |p: Pos2| origin + pan + p.to_vec2() * zoom;
    let to_doc = |p: Pos2| ((p - origin - pan) / zoom).to_pos2();

    let top_rect = Rect::from_min_max(Pos2::new(canvas_rect.min.x, full_rect.min.y), Pos2::new(full_rect.max.x, canvas_rect.min.y));
    let left_rect = Rect::from_min_max(Pos2::new(full_rect.min.x, canvas_rect.min.y), Pos2::new(canvas_rect.min.x, full_rect.max.y));
    let corner_rect = Rect::from_min_max(full_rect.min, canvas_rect.min);

    painter.rect_filled(top_rect, 0.0, colors.ruler_bg);
    painter.rect_filled(left_rect, 0.0, colors.ruler_bg);
    painter.rect_filled(corner_rect, 0.0, colors.ruler_bg);
    painter.line_segment([top_rect.left_bottom(), top_rect.right_bottom()], EguiStroke::new(1.0, colors.ruler_line));
    painter.line_segment([left_rect.right_top(), left_rect.right_bottom()], EguiStroke::new(1.0, colors.ruler_line));

    let step = nice_ruler_step(zoom, 70.0);
    let minor_step = step / 5.0;
    let font = egui::FontId::proportional(9.0);

    let doc_min = to_doc(canvas_rect.min);
    let doc_max = to_doc(canvas_rect.max);

    // Horizontal (top) ruler: ticks + labels along x.
    {
        let first = (doc_min.x / minor_step).floor() as i64;
        let last = (doc_max.x / minor_step).ceil() as i64;
        for i in first..=last {
            let x = i as f32 * minor_step;
            let sx = to_screen(Pos2::new(x, 0.0)).x;
            if sx < top_rect.min.x || sx > top_rect.max.x {
                continue;
            }
            let nearest_major = (x / step).round() * step;
            let major = (x - nearest_major).abs() < minor_step * 0.5;
            let tick_top = if major { top_rect.max.y - 9.0 } else { top_rect.max.y - 5.0 };
            painter.line_segment(
                [Pos2::new(sx, tick_top), Pos2::new(sx, top_rect.max.y)],
                EguiStroke::new(1.0, colors.ruler_line),
            );
            if major {
                let galley = painter.layout_no_wrap(format!("{x:.0}"), font.clone(), colors.ruler_text);
                painter.galley(Pos2::new(sx + 2.0, top_rect.min.y + 1.0), galley, colors.ruler_text);
            }
        }
    }

    // Vertical (left) ruler: ticks + rotated labels along y.
    {
        let first = (doc_min.y / minor_step).floor() as i64;
        let last = (doc_max.y / minor_step).ceil() as i64;
        for i in first..=last {
            let y = i as f32 * minor_step;
            let sy = to_screen(Pos2::new(0.0, y)).y;
            if sy < left_rect.min.y || sy > left_rect.max.y {
                continue;
            }
            let nearest_major = (y / step).round() * step;
            let major = (y - nearest_major).abs() < minor_step * 0.5;
            let tick_left = if major { left_rect.max.x - 9.0 } else { left_rect.max.x - 5.0 };
            painter.line_segment(
                [Pos2::new(tick_left, sy), Pos2::new(left_rect.max.x, sy)],
                EguiStroke::new(1.0, colors.ruler_line),
            );
            if major {
                let galley = painter.layout_no_wrap(format!("{y:.0}"), font.clone(), colors.ruler_text);
                painter.add(egui::Shape::Text(egui::epaint::TextShape {
                    angle: -std::f32::consts::FRAC_PI_2,
                    ..egui::epaint::TextShape::new(Pos2::new(left_rect.min.x + 1.0, sy + 2.0), galley, colors.ruler_text)
                }));
            }
        }
    }

    // A thin crosshair highlight on both rulers tracking the pointer, like
    // most vector tools' ruler readouts.
    if let Some(mp) = hover_pos
        && (canvas_rect.contains(mp) || top_rect.contains(mp) || left_rect.contains(mp)) {
            painter.line_segment(
                [Pos2::new(mp.x, top_rect.min.y), Pos2::new(mp.x, top_rect.max.y)],
                EguiStroke::new(1.0, SELECTION_COLOR),
            );
            painter.line_segment(
                [Pos2::new(left_rect.min.x, mp.y), Pos2::new(left_rect.max.x, mp.y)],
                EguiStroke::new(1.0, SELECTION_COLOR),
            );
        }
}
