//! Hand-drawn icons for the four layer-order (`ArrangeCommand`) toolbar
//! buttons: an arrow up/down for one step, plus a bar it stops at for
//! "all the way to the front/back". Same painter-only approach as `icons.rs`.

use egui::{Color32, Painter, Pos2, Rect, Stroke};

use crate::arrange::ArrangeCommand;

const STROKE_WIDTH: f32 = 1.6;

/// Maps normalized `(0..1, 0..1)` icon-space coordinates onto `rect`.
fn p(rect: Rect, x: f32, y: f32) -> Pos2 {
    Pos2::new(rect.left() + x * rect.width(), rect.top() + y * rect.height())
}

/// Draws `command`'s icon into `rect` with `color`.
pub fn draw_arrange_icon(painter: &Painter, command: ArrangeCommand, rect: Rect, color: Color32) {
    let stroke = Stroke::new(STROKE_WIDTH, color);
    let (pointing_up, with_bar) = match command {
        ArrangeCommand::BringForward => (true, false),
        ArrangeCommand::SendBackward => (false, false),
        ArrangeCommand::BringToFront => (true, true),
        ArrangeCommand::SendToBack => (false, true),
    };
    // Built pointing up, then mirrored vertically for the "back" variants.
    let y = |v: f32| if pointing_up { v } else { 1.0 - v };
    painter.line_segment([p(rect, 0.5, y(0.30)), p(rect, 0.5, y(0.90))], stroke);
    painter.line_segment([p(rect, 0.5, y(0.30)), p(rect, 0.25, y(0.55))], stroke);
    painter.line_segment([p(rect, 0.5, y(0.30)), p(rect, 0.75, y(0.55))], stroke);
    if with_bar {
        painter.line_segment([p(rect, 0.18, y(0.12)), p(rect, 0.82, y(0.12))], Stroke::new(STROKE_WIDTH * 1.4, color));
    }
}
