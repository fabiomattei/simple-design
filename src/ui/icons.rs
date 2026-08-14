//! Hand-drawn vector line icons for toolbar buttons.
//!
//! Rendered directly with `egui::Painter` (no image/font assets) so they stay crisp at any
//! DPI and follow the current theme's text color automatically.

use egui::{Color32, CornerRadius, Painter, Pos2, Rect, Response, Sense, Shape, Stroke, StrokeKind, Ui, Vec2};

use crate::alignment::{AlignEdge, DistributeAxis};
use crate::model::BoolOp;
use crate::tools::Tool;
use crate::transform_ops::FlipAxis;

/// Which corner of a rectangle a `corner_radius_icon` diagram highlights.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RectCorner {
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight,
}

const BUTTON_SIZE: f32 = 30.0;
const ICON_SIZE: f32 = 17.0;
const STROKE_WIDTH: f32 = 1.6;

/// Maps normalized `(0..1, 0..1)` icon-space coordinates onto the icon's on-screen rect.
fn p(rect: Rect, x: f32, y: f32) -> Pos2 {
    Pos2::new(
        rect.left() + x * rect.width(),
        rect.top() + y * rect.height(),
    )
}

fn stroke(color: Color32) -> Stroke {
    Stroke::new(STROKE_WIDTH, color)
}

/// Draws a toggle-style icon button for a `Tool` and returns its `Response`.
pub fn tool_button(ui: &mut Ui, tool: Tool, selected: bool) -> Response {
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(BUTTON_SIZE), Sense::click());
    if ui.is_rect_visible(rect) {
        let visuals = ui.style().interact_selectable(&response, selected);
        if selected || response.hovered() {
            ui.painter()
                .rect_filled(rect, 6.0, visuals.weak_bg_fill);
        }
        let color = if selected {
            ui.visuals().selection.stroke.color
        } else {
            ui.visuals().text_color()
        };
        let icon_rect = Rect::from_center_size(rect.center(), Vec2::splat(ICON_SIZE));
        draw_tool_icon(ui.painter(), tool, icon_rect, color);
    }
    response
}

/// Draws a toggle-style icon button from an arbitrary draw closure, e.g. for the panel rail
/// (`ui/panel_rail.rs`) — same highlight-when-selected behavior as `tool_button`, but not tied
/// to the `Tool` enum.
pub fn toggle_icon_button(ui: &mut Ui, selected: bool, draw: impl FnOnce(&Painter, Rect, Color32)) -> Response {
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(BUTTON_SIZE), Sense::click());
    if ui.is_rect_visible(rect) {
        let visuals = ui.style().interact_selectable(&response, selected);
        if selected || response.hovered() {
            ui.painter().rect_filled(rect, 6.0, visuals.weak_bg_fill);
        }
        let color = if selected {
            ui.visuals().selection.stroke.color
        } else {
            ui.visuals().text_color()
        };
        let icon_rect = Rect::from_center_size(rect.center(), Vec2::splat(ICON_SIZE));
        draw(ui.painter(), icon_rect, color);
    }
    response
}

/// Draws a plain (non-toggling) icon button, e.g. for "Insert Image...".
pub fn icon_button(ui: &mut Ui, draw: impl FnOnce(&Painter, Rect, Color32)) -> Response {
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(BUTTON_SIZE), Sense::click());
    if ui.is_rect_visible(rect) {
        let visuals = ui.style().interact(&response);
        if response.hovered() {
            ui.painter().rect_filled(rect, 6.0, visuals.weak_bg_fill);
        }
        let color = ui.visuals().text_color();
        let icon_rect = Rect::from_center_size(rect.center(), Vec2::splat(ICON_SIZE));
        draw(ui.painter(), icon_rect, color);
    }
    response
}

fn draw_tool_icon(painter: &Painter, tool: Tool, rect: Rect, color: Color32) {
    match tool {
        Tool::Select => draw_select(painter, rect, color),
        Tool::Pan => draw_pan(painter, rect, color),
        Tool::Artboard => draw_artboard(painter, rect, color),
        Tool::Rectangle => draw_rectangle(painter, rect, color),
        Tool::Oval => draw_oval(painter, rect, color),
        Tool::Line => draw_line(painter, rect, color),
        Tool::Arrow => draw_arrow(painter, rect, color),
        Tool::Star => draw_star(painter, rect, color),
        Tool::Polygon => draw_polygon(painter, rect, color),
        Tool::Pen => draw_pen(painter, rect, color),
        Tool::Text => draw_text(painter, rect, color),
        Tool::Scissors => draw_scissors(painter, rect, color),
    }
}

fn draw_select(painter: &Painter, rect: Rect, color: Color32) {
    // Classic arrow-cursor silhouette.
    let pts = [
        p(rect, 0.18, 0.08),
        p(rect, 0.18, 0.80),
        p(rect, 0.38, 0.62),
        p(rect, 0.52, 0.92),
        p(rect, 0.63, 0.86),
        p(rect, 0.48, 0.56),
        p(rect, 0.74, 0.56),
    ];
    painter.add(Shape::closed_line(pts.to_vec(), stroke(color)));
}

fn draw_pan(painter: &Painter, rect: Rect, color: Color32) {
    // A stylized open hand: palm + four fingers + thumb.
    let s = stroke(color);
    painter.add(Shape::closed_line(
        vec![
            p(rect, 0.28, 0.55),
            p(rect, 0.28, 0.80),
            p(rect, 0.78, 0.80),
            p(rect, 0.78, 0.55),
        ],
        s,
    ));
    for x in [0.36_f32, 0.50, 0.64] {
        painter.line_segment([p(rect, x, 0.55), p(rect, x, 0.15)], s);
    }
    painter.line_segment([p(rect, 0.28, 0.60), p(rect, 0.10, 0.42)], s);
}

fn draw_artboard(painter: &Painter, rect: Rect, color: Color32) {
    // Corner brackets, matching Sketch/Figma's artboard tool glyph.
    let s = stroke(color);
    let arm = 0.20_f32;
    let corners = [(0.12, 0.12), (0.88, 0.12), (0.12, 0.88), (0.88, 0.88)];
    for (cx, cy) in corners {
        let dx = if cx < 0.5 { arm } else { -arm };
        let dy = if cy < 0.5 { arm } else { -arm };
        let c = p(rect, cx, cy);
        painter.line_segment([c, p(rect, cx + dx, cy)], s);
        painter.line_segment([c, p(rect, cx, cy + dy)], s);
    }
}

fn draw_rectangle(painter: &Painter, rect: Rect, color: Color32) {
    let r = Rect::from_min_max(p(rect, 0.12, 0.18), p(rect, 0.88, 0.82));
    painter.rect_stroke(r, 2.0, stroke(color), StrokeKind::Middle);
}

fn draw_oval(painter: &Painter, rect: Rect, color: Color32) {
    painter.circle_stroke(rect.center(), rect.width() * 0.42, stroke(color));
}

fn draw_line(painter: &Painter, rect: Rect, color: Color32) {
    painter.line_segment([p(rect, 0.16, 0.84), p(rect, 0.84, 0.16)], stroke(color));
}

fn draw_arrow(painter: &Painter, rect: Rect, color: Color32) {
    let origin = p(rect, 0.14, 0.86);
    let tip = p(rect, 0.86, 0.14);
    painter.arrow(origin, tip - origin, stroke(color));
}

fn regular_polygon_points(rect: Rect, sides: usize, radius: f32, rotation: f32) -> Vec<Pos2> {
    let center = rect.center();
    let r = rect.width() * radius;
    (0..sides)
        .map(|i| {
            let angle = rotation + std::f32::consts::TAU * i as f32 / sides as f32;
            center + Vec2::new(r * angle.cos(), r * angle.sin())
        })
        .collect()
}

fn draw_star(painter: &Painter, rect: Rect, color: Color32) {
    let center = rect.center();
    let outer = rect.width() * 0.46;
    let inner = rect.width() * 0.18;
    let start = -std::f32::consts::FRAC_PI_2;
    let pts: Vec<Pos2> = (0..10)
        .map(|i| {
            let angle = start + std::f32::consts::PI * i as f32 / 5.0;
            let r = if i % 2 == 0 { outer } else { inner };
            center + Vec2::new(r * angle.cos(), r * angle.sin())
        })
        .collect();
    painter.add(Shape::closed_line(pts, stroke(color)));
}

fn draw_polygon(painter: &Painter, rect: Rect, color: Color32) {
    let pts = regular_polygon_points(rect, 6, 0.44, -std::f32::consts::FRAC_PI_2);
    painter.add(Shape::closed_line(pts, stroke(color)));
}

fn draw_pen(painter: &Painter, rect: Rect, color: Color32) {
    let s = stroke(color);
    let start = p(rect, 0.16, 0.82);
    let end = p(rect, 0.78, 0.32);
    let ctrl = p(rect, 0.72, 0.86);
    // Sample a quadratic bezier as a short polyline for the drawn path.
    let curve: Vec<Pos2> = (0..=12)
        .map(|i| {
            let t = i as f32 / 12.0;
            let a = start.to_vec2() * (1.0 - t) * (1.0 - t)
                + ctrl.to_vec2() * 2.0 * (1.0 - t) * t
                + end.to_vec2() * t * t;
            a.to_pos2()
        })
        .collect();
    painter.add(Shape::line(curve, s));
    // Anchor squares at each end, plus a curve-handle stub at the leading point.
    let anchor = 0.045 * rect.width();
    painter.rect_stroke(
        Rect::from_center_size(start, Vec2::splat(anchor * 2.0)),
        0.0,
        s,
        StrokeKind::Middle,
    );
    painter.rect_stroke(
        Rect::from_center_size(end, Vec2::splat(anchor * 2.0)),
        0.0,
        s,
        StrokeKind::Middle,
    );
    let handle_tip = p(rect, 0.94, 0.14);
    painter.line_segment([end, handle_tip], s);
    painter.circle_filled(handle_tip, anchor * 0.9, color);
}

fn draw_text(painter: &Painter, rect: Rect, color: Color32) {
    let s = stroke(color);
    painter.line_segment([p(rect, 0.20, 0.20), p(rect, 0.80, 0.20)], s);
    painter.line_segment([p(rect, 0.50, 0.20), p(rect, 0.50, 0.82)], s);
}

fn draw_scissors(painter: &Painter, rect: Rect, color: Color32) {
    let s = stroke(color);
    let left_loop = p(rect, 0.30, 0.76);
    let right_loop = p(rect, 0.70, 0.76);
    let tip = p(rect, 0.5, 0.14);
    painter.circle_stroke(left_loop, rect.width() * 0.12, s);
    painter.circle_stroke(right_loop, rect.width() * 0.12, s);
    painter.line_segment([left_loop, p(rect, 0.72, 0.20)], s);
    painter.line_segment([right_loop, p(rect, 0.28, 0.20)], s);
    let _ = tip;
}

const ALIGN_BUTTON_SIZE: f32 = 26.0;
const ALIGN_ICON_SIZE: f32 = 20.0;

/// Draws an "Align" button (align-left/h-center/right/top/v-center/bottom).
pub fn align_button(ui: &mut Ui, edge: AlignEdge) -> Response {
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(ALIGN_BUTTON_SIZE), Sense::click());
    if ui.is_rect_visible(rect) {
        let visuals = ui.style().interact(&response);
        if response.hovered() {
            ui.painter().rect_filled(rect, 4.0, visuals.weak_bg_fill);
        }
        let color = ui.visuals().text_color();
        let icon_rect = Rect::from_center_size(rect.center(), Vec2::splat(ALIGN_ICON_SIZE));
        draw_align_icon(ui.painter(), edge, icon_rect, color);
    }
    response
}

/// Draws a "Distribute spacing" button (horizontal/vertical), dimmed when `enabled` is false.
pub fn distribute_button(ui: &mut Ui, axis: DistributeAxis, enabled: bool) -> Response {
    let sense = if enabled { Sense::click() } else { Sense::hover() };
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(ALIGN_BUTTON_SIZE), sense);
    if ui.is_rect_visible(rect) {
        let visuals = ui.style().interact(&response);
        if enabled && response.hovered() {
            ui.painter().rect_filled(rect, 4.0, visuals.weak_bg_fill);
        }
        let color = if enabled {
            ui.visuals().text_color()
        } else {
            ui.visuals().weak_text_color()
        };
        let icon_rect = Rect::from_center_size(rect.center(), Vec2::splat(ALIGN_ICON_SIZE));
        match axis {
            DistributeAxis::Horizontal => draw_distribute_horizontal(ui.painter(), icon_rect, color),
            DistributeAxis::Vertical => draw_distribute_vertical(ui.painter(), icon_rect, color),
        }
    }
    response
}

fn draw_align_icon(painter: &Painter, edge: AlignEdge, rect: Rect, color: Color32) {
    match edge {
        AlignEdge::Left => draw_align_axis(painter, rect, color, true, 0.20, &[(0.20, 0.62), (0.20, 0.86), (0.20, 0.42)]),
        AlignEdge::HCenter => draw_align_center(painter, rect, color, true),
        AlignEdge::Right => draw_align_axis(painter, rect, color, true, 0.80, &[(0.38, 0.80), (0.14, 0.80), (0.58, 0.80)]),
        AlignEdge::Top => draw_align_axis(painter, rect, color, false, 0.20, &[(0.20, 0.62), (0.20, 0.86), (0.20, 0.42)]),
        AlignEdge::VCenter => draw_align_center(painter, rect, color, false),
        AlignEdge::Bottom => draw_align_axis(painter, rect, color, false, 0.80, &[(0.38, 0.80), (0.14, 0.80), (0.58, 0.80)]),
    }
}

/// Draws the align guide line plus three bars anchored to it. `vertical` selects whether the
/// guide (and the bars' anchored edge) runs along x (left/right align) or y (top/bottom align).
/// `extents` gives each bar's `(offset_from_guide, thickness)` in icon-space fractions of size.
fn draw_align_axis(
    painter: &Painter,
    rect: Rect,
    color: Color32,
    vertical: bool,
    guide: f32,
    extents: &[(f32, f32); 3],
) {
    let s = stroke(color);
    if vertical {
        painter.line_segment([p(rect, guide, 0.06), p(rect, guide, 0.94)], s);
    } else {
        painter.line_segment([p(rect, 0.06, guide), p(rect, 0.94, guide)], s);
    }
    let cross_positions = [0.20_f32, 0.50, 0.80];
    for (&cross, &(from, to)) in cross_positions.iter().zip(extents.iter()) {
        let (a, b) = if from < to { (from, to) } else { (to, from) };
        let bar = if vertical {
            [p(rect, a, cross), p(rect, b, cross)]
        } else {
            [p(rect, cross, a), p(rect, cross, b)]
        };
        painter.line_segment(bar, Stroke::new(STROKE_WIDTH * 1.8, color));
    }
}

fn draw_align_center(painter: &Painter, rect: Rect, color: Color32, vertical: bool) {
    let s = stroke(color);
    if vertical {
        painter.line_segment([p(rect, 0.5, 0.06), p(rect, 0.5, 0.94)], s);
    } else {
        painter.line_segment([p(rect, 0.06, 0.5), p(rect, 0.94, 0.5)], s);
    }
    let cross_positions = [0.20_f32, 0.50, 0.80];
    let half_lengths = [0.30_f32, 0.14, 0.22];
    for (&cross, &half) in cross_positions.iter().zip(half_lengths.iter()) {
        let bar = if vertical {
            [p(rect, 0.5 - half, cross), p(rect, 0.5 + half, cross)]
        } else {
            [p(rect, cross, 0.5 - half), p(rect, cross, 0.5 + half)]
        };
        painter.line_segment(bar, Stroke::new(STROKE_WIDTH * 1.8, color));
    }
}

fn draw_distribute_horizontal(painter: &Painter, rect: Rect, color: Color32) {
    let s = stroke(color);
    let bars = [0.16_f32, 0.5, 0.84];
    for x in bars {
        painter.line_segment(
            [p(rect, x, 0.20), p(rect, x, 0.80)],
            Stroke::new(STROKE_WIDTH * 1.8, color),
        );
    }
    for x in [(bars[0] + bars[1]) / 2.0, (bars[1] + bars[2]) / 2.0] {
        painter.line_segment([p(rect, x, 0.46), p(rect, x, 0.54)], s);
    }
}

fn draw_distribute_vertical(painter: &Painter, rect: Rect, color: Color32) {
    let s = stroke(color);
    let bars = [0.16_f32, 0.5, 0.84];
    for y in bars {
        painter.line_segment(
            [p(rect, 0.20, y), p(rect, 0.80, y)],
            Stroke::new(STROKE_WIDTH * 1.8, color),
        );
    }
    for y in [(bars[0] + bars[1]) / 2.0, (bars[1] + bars[2]) / 2.0] {
        painter.line_segment([p(rect, 0.46, y), p(rect, 0.54, y)], s);
    }
}

/// Draws a "Flip Horizontal"/"Flip Vertical" button: a shape mirrored across a dashed axis.
pub fn flip_button(ui: &mut Ui, axis: FlipAxis) -> Response {
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(ALIGN_BUTTON_SIZE), Sense::click());
    if ui.is_rect_visible(rect) {
        let visuals = ui.style().interact(&response);
        if response.hovered() {
            ui.painter().rect_filled(rect, 4.0, visuals.weak_bg_fill);
        }
        let color = ui.visuals().text_color();
        let icon_rect = Rect::from_center_size(rect.center(), Vec2::splat(ALIGN_ICON_SIZE));
        match axis {
            FlipAxis::Horizontal => draw_flip_horizontal(ui.painter(), icon_rect, color),
            FlipAxis::Vertical => draw_flip_vertical(ui.painter(), icon_rect, color),
        }
    }
    response
}

fn draw_flip_horizontal(painter: &Painter, rect: Rect, color: Color32) {
    let s = stroke(color);
    painter.extend(Shape::dashed_line(
        &[p(rect, 0.5, 0.04), p(rect, 0.5, 0.96)],
        s,
        3.0,
        3.0,
    ));
    // Two mirrored triangles pointing at the axis, like a shape and its reflection.
    painter.add(Shape::closed_line(
        vec![p(rect, 0.08, 0.20), p(rect, 0.08, 0.80), p(rect, 0.38, 0.50)],
        s,
    ));
    painter.add(Shape::closed_line(
        vec![p(rect, 0.92, 0.20), p(rect, 0.92, 0.80), p(rect, 0.62, 0.50)],
        s,
    ));
}

fn draw_flip_vertical(painter: &Painter, rect: Rect, color: Color32) {
    let s = stroke(color);
    painter.extend(Shape::dashed_line(
        &[p(rect, 0.04, 0.5), p(rect, 0.96, 0.5)],
        s,
        3.0,
        3.0,
    ));
    painter.add(Shape::closed_line(
        vec![p(rect, 0.20, 0.08), p(rect, 0.80, 0.08), p(rect, 0.50, 0.38)],
        s,
    ));
    painter.add(Shape::closed_line(
        vec![p(rect, 0.20, 0.92), p(rect, 0.80, 0.92), p(rect, 0.50, 0.62)],
        s,
    ));
}

const CORNER_RADIUS_ICON_SIZE: f32 = 18.0;

/// Draws a small non-interactive diagram of a rounded rectangle with only `corner` rounded,
/// so the corner-radius input fields don't rely on "TL"/"TR"/"BL"/"BR" text abbreviations.
pub fn corner_radius_icon(ui: &mut Ui, corner: RectCorner) {
    let (rect, _) = ui.allocate_exact_size(Vec2::splat(CORNER_RADIUS_ICON_SIZE), Sense::hover());
    if ui.is_rect_visible(rect) {
        let color = ui.visuals().text_color();
        let square = Rect::from_min_max(p(rect, 0.12, 0.12), p(rect, 0.88, 0.88));
        let r = (square.width() * 0.55) as u8;
        let radius = match corner {
            RectCorner::TopLeft => CornerRadius { nw: r, ne: 0, sw: 0, se: 0 },
            RectCorner::TopRight => CornerRadius { nw: 0, ne: r, sw: 0, se: 0 },
            RectCorner::BottomLeft => CornerRadius { nw: 0, ne: 0, sw: r, se: 0 },
            RectCorner::BottomRight => CornerRadius { nw: 0, ne: 0, sw: 0, se: r },
        };
        ui.painter()
            .rect_stroke(square, radius, stroke(color), StrokeKind::Middle);
    }
}

const COMBINE_BUTTON_SIZE: f32 = 34.0;
const COMBINE_ICON_SIZE: f32 = 24.0;

/// Draws a boolean-operation button (Union/Subtract/Intersect/Difference/Add) as a filled
/// silhouette of two overlapping squares showing which region the operation keeps — text
/// labels alone don't convey which shape "wins" the overlap.
pub fn combine_button(ui: &mut Ui, op: BoolOp) -> Response {
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(COMBINE_BUTTON_SIZE), Sense::click());
    if ui.is_rect_visible(rect) {
        let visuals = ui.style().interact(&response);
        if response.hovered() {
            ui.painter().rect_filled(rect, 4.0, visuals.weak_bg_fill);
        }
        let color = ui.visuals().text_color();
        let icon_rect = Rect::from_center_size(rect.center(), Vec2::splat(COMBINE_ICON_SIZE));
        draw_combine_icon(ui.painter(), op, icon_rect, color);
    }
    response
}

fn draw_combine_icon(painter: &Painter, op: BoolOp, rect: Rect, color: Color32) {
    if op == BoolOp::Add {
        draw_combine_add(painter, rect, color);
        return;
    }
    // Two overlapping squares (A back/top-left, B front/bottom-right); the overlap is their
    // intersection. Each op is drawn by filling exactly the region it keeps, plus a faint
    // outline of whichever original shape(s) aren't fully part of the result, for context.
    let faint = color.gamma_multiply(0.35);
    let a = Rect::from_min_max(p(rect, 0.08, 0.08), p(rect, 0.56, 0.56));
    let b = Rect::from_min_max(p(rect, 0.34, 0.34), p(rect, 0.82, 0.82));
    let overlap = Rect::from_min_max(p(rect, 0.34, 0.34), p(rect, 0.56, 0.56));
    let a_top = Rect::from_min_max(p(rect, 0.08, 0.08), p(rect, 0.56, 0.34));
    let a_left = Rect::from_min_max(p(rect, 0.08, 0.34), p(rect, 0.34, 0.56));
    let b_right = Rect::from_min_max(p(rect, 0.56, 0.34), p(rect, 0.82, 0.56));
    let b_bottom = Rect::from_min_max(p(rect, 0.34, 0.56), p(rect, 0.82, 0.82));

    match op {
        BoolOp::Union => {
            painter.rect_filled(a, 1.0, color);
            painter.rect_filled(b, 1.0, color);
        }
        BoolOp::Subtract => {
            painter.rect_stroke(b, 1.0, stroke(faint), StrokeKind::Middle);
            painter.rect_filled(a_top, 1.0, color);
            painter.rect_filled(a_left, 1.0, color);
        }
        BoolOp::Intersect => {
            painter.rect_stroke(a, 1.0, stroke(faint), StrokeKind::Middle);
            painter.rect_stroke(b, 1.0, stroke(faint), StrokeKind::Middle);
            painter.rect_filled(overlap, 1.0, color);
        }
        BoolOp::Difference => {
            painter.rect_filled(a_top, 1.0, color);
            painter.rect_filled(a_left, 1.0, color);
            painter.rect_filled(b_right, 1.0, color);
            painter.rect_filled(b_bottom, 1.0, color);
        }
        BoolOp::Add => unreachable!("handled above"),
    }
}

fn draw_combine_add(painter: &Painter, rect: Rect, color: Color32) {
    // Two disjoint filled shapes joined by a "+", matching Add's actual behavior:
    // pieces are concatenated as-is with no clipping math (see `BoolOp::Add`'s doc comment).
    let a = Rect::from_min_max(p(rect, 0.08, 0.20), p(rect, 0.46, 0.58));
    let b = Rect::from_min_max(p(rect, 0.54, 0.42), p(rect, 0.92, 0.80));
    painter.rect_filled(a, 1.0, color);
    painter.rect_filled(b, 1.0, color);
    let s = stroke(color);
    painter.line_segment([p(rect, 0.45, 0.31), p(rect, 0.55, 0.31)], s);
    painter.line_segment([p(rect, 0.50, 0.26), p(rect, 0.50, 0.36)], s);
}

/// Draws the "Flatten" button: two overlapping shape outlines collapsing (via a downward
/// arrow) into a single solid filled shape below.
pub fn flatten_button(ui: &mut Ui) -> Response {
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(ALIGN_BUTTON_SIZE), Sense::click());
    if ui.is_rect_visible(rect) {
        let visuals = ui.style().interact(&response);
        if response.hovered() {
            ui.painter().rect_filled(rect, 4.0, visuals.weak_bg_fill);
        }
        let color = ui.visuals().text_color();
        let icon_rect = Rect::from_center_size(rect.center(), Vec2::splat(ALIGN_ICON_SIZE));
        draw_flatten(ui.painter(), icon_rect, color);
    }
    response
}

fn draw_flatten(painter: &Painter, rect: Rect, color: Color32) {
    let faint = color.gamma_multiply(0.5);
    let a = Rect::from_min_max(p(rect, 0.10, 0.04), p(rect, 0.56, 0.34));
    let b = Rect::from_min_max(p(rect, 0.30, 0.14), p(rect, 0.76, 0.44));
    painter.rect_stroke(a, 1.0, stroke(faint), StrokeKind::Middle);
    painter.rect_stroke(b, 1.0, stroke(color), StrokeKind::Middle);
    let origin = p(rect, 0.5, 0.48);
    let end = p(rect, 0.5, 0.66);
    painter.arrow(origin, end - origin, stroke(color));
    let bar = Rect::from_min_max(p(rect, 0.14, 0.76), p(rect, 0.72, 0.94));
    painter.rect_filled(bar, 1.0, color);
}

/// Draws the "Duplicate" button: two overlapping square outlines, the universal copy/duplicate
/// glyph — a back copy (faint, for depth) behind a front copy (full stroke).
pub fn duplicate_button(ui: &mut Ui) -> Response {
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(ALIGN_BUTTON_SIZE), Sense::click());
    if ui.is_rect_visible(rect) {
        let visuals = ui.style().interact(&response);
        if response.hovered() {
            ui.painter().rect_filled(rect, 4.0, visuals.weak_bg_fill);
        }
        let color = ui.visuals().text_color();
        let icon_rect = Rect::from_center_size(rect.center(), Vec2::splat(ALIGN_ICON_SIZE));
        draw_duplicate(ui.painter(), icon_rect, color);
    }
    response
}

fn draw_duplicate(painter: &Painter, rect: Rect, color: Color32) {
    let faint = color.gamma_multiply(0.5);
    let back = Rect::from_min_max(p(rect, 0.08, 0.08), p(rect, 0.62, 0.62));
    let front = Rect::from_min_max(p(rect, 0.36, 0.36), p(rect, 0.90, 0.90));
    painter.rect_stroke(back, 1.0, stroke(faint), StrokeKind::Middle);
    painter.rect_stroke(front, 1.0, stroke(color), StrokeKind::Middle);
}

/// Samples a quadratic bezier (`start`->`ctrl`->`end`) as a polyline of `segments` points.
fn quad_bezier(start: Pos2, ctrl: Pos2, end: Pos2, segments: usize) -> Vec<Pos2> {
    (0..=segments)
        .map(|i| {
            let t = i as f32 / segments as f32;
            let a = start.to_vec2() * (1.0 - t) * (1.0 - t)
                + ctrl.to_vec2() * 2.0 * (1.0 - t) * t
                + end.to_vec2() * t * t;
            a.to_pos2()
        })
        .collect()
}

const TOGGLE_BUTTON_SIZE: f32 = 20.0;
const TOGGLE_ICON_SIZE: f32 = 13.0;

/// Draws the layers panel's per-row "visibility" toggle: an open eye (lens outline + iris +
/// pupil) when `visible`, a closed eyelid with lashes when not — the layers panel row is
/// dimmed to `weak_text_color` for the hidden state so it reads as secondary at a glance.
pub fn eye_button(ui: &mut Ui, visible: bool) -> Response {
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(TOGGLE_BUTTON_SIZE), Sense::click());
    if ui.is_rect_visible(rect) {
        let visuals = ui.style().interact(&response);
        if response.hovered() {
            ui.painter().rect_filled(rect, 4.0, visuals.weak_bg_fill);
        }
        let color = if visible { ui.visuals().text_color() } else { ui.visuals().weak_text_color() };
        let icon_rect = Rect::from_center_size(rect.center(), Vec2::splat(TOGGLE_ICON_SIZE));
        draw_eye(ui.painter(), icon_rect, color, visible);
    }
    response
}

fn draw_eye(painter: &Painter, rect: Rect, color: Color32, visible: bool) {
    let s = stroke(color);
    if visible {
        let left = p(rect, 0.04, 0.5);
        let right = p(rect, 0.96, 0.5);
        let mut lens = quad_bezier(left, p(rect, 0.5, -0.10), right, 10);
        lens.extend(quad_bezier(right, p(rect, 0.5, 1.10), left, 10).into_iter().skip(1));
        painter.add(Shape::closed_line(lens, s));
        painter.circle_stroke(rect.center(), rect.width() * 0.16, s);
        painter.circle_filled(rect.center(), rect.width() * 0.06, color);
    } else {
        // Closed eyelid: a single downward-bowed lid line plus three short lashes.
        let left = p(rect, 0.04, 0.55);
        let right = p(rect, 0.96, 0.55);
        let lid = quad_bezier(left, p(rect, 0.5, 0.20), right, 10);
        painter.add(Shape::line(lid, s));
        for x in [0.28_f32, 0.5, 0.72] {
            painter.line_segment([p(rect, x, 0.62), p(rect, x, 0.86)], s);
        }
    }
}

/// Draws the layers panel's per-row "lock" toggle: a closed shackle over the body when
/// `locked`, a shackle swung open (anchored on one side only) when not — dimmed to
/// `weak_text_color` for the unlocked state, same "secondary state" convention as `eye_button`.
pub fn lock_button(ui: &mut Ui, locked: bool) -> Response {
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(TOGGLE_BUTTON_SIZE), Sense::click());
    if ui.is_rect_visible(rect) {
        let visuals = ui.style().interact(&response);
        if response.hovered() {
            ui.painter().rect_filled(rect, 4.0, visuals.weak_bg_fill);
        }
        let color = if locked { ui.visuals().text_color() } else { ui.visuals().weak_text_color() };
        let icon_rect = Rect::from_center_size(rect.center(), Vec2::splat(TOGGLE_ICON_SIZE));
        draw_lock(ui.painter(), icon_rect, color, locked);
    }
    response
}

/// Traces the padlock's shackle as a semicircular arc (left -> top -> right) around `center`.
fn shackle_arc(center: Pos2, radius: f32) -> Vec<Pos2> {
    (0..=12)
        .map(|i| {
            let angle = std::f32::consts::PI * (1.0 - i as f32 / 12.0);
            center + Vec2::new(radius * angle.cos(), -radius * angle.sin())
        })
        .collect()
}

fn draw_lock(painter: &Painter, rect: Rect, color: Color32, locked: bool) {
    let s = stroke(color);
    let body = Rect::from_min_max(p(rect, 0.14, 0.46), p(rect, 0.86, 0.94));
    painter.rect_stroke(body, 1.5, s, StrokeKind::Middle);
    painter.circle_filled(p(rect, 0.5, 0.63), rect.width() * 0.07, color);
    painter.line_segment([p(rect, 0.5, 0.66), p(rect, 0.5, 0.80)], Stroke::new(STROKE_WIDTH * 1.3, color));
    let radius = rect.width() * 0.24;
    if locked {
        // Shackle legs land exactly on the body's top edge, reading as one closed loop.
        let arc = shackle_arc(p(rect, 0.5, 0.46), radius);
        painter.add(Shape::line(arc, s));
    } else {
        // Shackle pivots up and to the right: the right leg still meets the body, the left
        // leg is left floating open, reading as "unlocked".
        let center = p(rect, 0.5, 0.46) + Vec2::new(rect.width() * 0.06, -rect.height() * 0.16);
        let arc = shackle_arc(center, radius);
        painter.add(Shape::line(arc.clone(), s));
        painter.line_segment([arc[arc.len() - 1], p(rect, 0.86, 0.46)], s);
    }
}

/// Draws a dismissible panel's header "X" close button (Layers/Inspector — see `app.rs`'s
/// `show_layers_panel`/`show_inspector_panel`).
pub fn close_button(ui: &mut Ui) -> Response {
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(TOGGLE_BUTTON_SIZE), Sense::click());
    if ui.is_rect_visible(rect) {
        let visuals = ui.style().interact(&response);
        if response.hovered() {
            ui.painter().rect_filled(rect, 4.0, visuals.weak_bg_fill);
        }
        let color = ui.visuals().text_color();
        let icon_rect = Rect::from_center_size(rect.center(), Vec2::splat(TOGGLE_ICON_SIZE * 0.7));
        let s = stroke(color);
        ui.painter().line_segment([icon_rect.left_top(), icon_rect.right_bottom()], s);
        ui.painter().line_segment([icon_rect.right_top(), icon_rect.left_bottom()], s);
    }
    response.on_hover_text("Close")
}

/// Draws the panel rail's "Layers" glyph: a diamond with two chevrons stacked below it, the
/// standard "layers" icon shorthand.
pub fn draw_layers_icon(painter: &Painter, rect: Rect, color: Color32) {
    let s = stroke(color);
    painter.add(Shape::closed_line(
        vec![p(rect, 0.5, 0.08), p(rect, 0.92, 0.34), p(rect, 0.5, 0.60), p(rect, 0.08, 0.34)],
        s,
    ));
    painter.add(Shape::line(vec![p(rect, 0.08, 0.50), p(rect, 0.5, 0.76), p(rect, 0.92, 0.50)], s));
    painter.add(Shape::line(vec![p(rect, 0.08, 0.64), p(rect, 0.5, 0.90), p(rect, 0.92, 0.64)], s));
}

/// Draws the panel rail's "Palette" glyph: a ring of color swatches.
pub fn draw_palette_icon(painter: &Painter, rect: Rect, color: Color32) {
    painter.circle_stroke(rect.center(), rect.width() * 0.44, stroke(color));
    for (x, y) in [(0.34, 0.32), (0.62, 0.30), (0.72, 0.56), (0.38, 0.62)] {
        painter.circle_filled(p(rect, x, y), rect.width() * 0.09, color);
    }
}

/// Draws the panel rail's "Inspector" glyph: three property sliders.
pub fn draw_inspector_icon(painter: &Painter, rect: Rect, color: Color32) {
    let s = stroke(color);
    for (y, knob_x) in [(0.28, 0.62), (0.5, 0.38), (0.72, 0.74)] {
        painter.line_segment([p(rect, 0.08, y), p(rect, 0.92, y)], s);
        painter.circle_filled(p(rect, knob_x, y), rect.width() * 0.07, color);
    }
}

/// Draws the panel rail's "Minimap" glyph: a document frame with the visible-viewport box
/// inside it, matching what `ui/minimap_panel.rs` actually renders.
pub fn draw_minimap_icon(painter: &Painter, rect: Rect, color: Color32) {
    let outer = Rect::from_min_max(p(rect, 0.08, 0.12), p(rect, 0.92, 0.88));
    painter.rect_stroke(outer, 1.0, stroke(color), StrokeKind::Middle);
    let inner = Rect::from_min_max(p(rect, 0.34, 0.36), p(rect, 0.74, 0.64));
    painter.rect_stroke(inner, 1.0, Stroke::new(STROKE_WIDTH * 1.3, color), StrokeKind::Middle);
}

/// Draws the "Insert Image..." glyph: a picture frame with a sun and mountains.
pub fn draw_image(painter: &Painter, rect: Rect, color: Color32) {
    let s = stroke(color);
    let frame = Rect::from_min_max(p(rect, 0.10, 0.14), p(rect, 0.90, 0.86));
    painter.rect_stroke(frame, 2.0, s, StrokeKind::Middle);
    painter.circle_stroke(p(rect, 0.32, 0.36), rect.width() * 0.09, s);
    painter.add(Shape::line(
        vec![
            p(rect, 0.10, 0.72),
            p(rect, 0.36, 0.50),
            p(rect, 0.54, 0.66),
            p(rect, 0.70, 0.46),
            p(rect, 0.90, 0.68),
        ],
        s,
    ));
}
