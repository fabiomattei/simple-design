//! Sketch-style resizing constraints: the pure per-axis math for where a
//! child goes when its parent container is resized (see
//! `ResizingConstraints`). Applying it to a layer tree lives in
//! `canvas::constraint_resize::resize_layer`, next to the resize machinery it
//! reuses — keeping this module free of any `canvas` dependency.

use egui::{Pos2, Rect, Vec2};

use crate::model::ResizingConstraints;

/// One axis of a constraint: whether the start (left/top) and end
/// (right/bottom) margins are pinned, and whether the size is fixed.
struct AxisConstraint {
    pin_start: bool,
    pin_end: bool,
    fixed_size: bool,
}

/// A child's new `(start, size)` along one axis, in its parent's local
/// space, when the parent's extent on that axis goes from `old_parent` to
/// `new_parent`. With no pins the child scales with the parent; a pinned
/// edge keeps its margin; both pinned stretches the child (ignoring
/// `fixed_size`). A stretched size never goes below zero.
fn constrained_axis(start: f32, size: f32, old_parent: f32, new_parent: f32, axis: AxisConstraint) -> (f32, f32) {
    let ratio = if old_parent.abs() > 1e-6 { new_parent / old_parent } else { 1.0 };
    let scaled_size = if axis.fixed_size { size } else { size * ratio };
    match (axis.pin_start, axis.pin_end) {
        (true, true) => (start, (size + new_parent - old_parent).max(0.0)),
        (true, false) => (start, scaled_size),
        (false, true) => {
            let end_margin = old_parent - (start + size);
            (new_parent - end_margin - scaled_size, scaled_size)
        }
        (false, false) => {
            let center = (start + size / 2.0) * ratio;
            (center - scaled_size / 2.0, scaled_size)
        }
    }
}

/// `child` (unrotated bounds in its parent's local space) laid out inside a
/// parent whose size went from `old_parent` to `new_parent`.
pub fn constrained_child_bounds(child: Rect, constraints: &ResizingConstraints, old_parent: Vec2, new_parent: Vec2) -> Rect {
    let horizontal = AxisConstraint {
        pin_start: constraints.pin_left,
        pin_end: constraints.pin_right,
        fixed_size: constraints.fixed_width,
    };
    let vertical = AxisConstraint {
        pin_start: constraints.pin_top,
        pin_end: constraints.pin_bottom,
        fixed_size: constraints.fixed_height,
    };
    let (x, w) = constrained_axis(child.min.x, child.width(), old_parent.x, new_parent.x, horizontal);
    let (y, h) = constrained_axis(child.min.y, child.height(), old_parent.y, new_parent.y, vertical);
    Rect::from_min_size(Pos2::new(x, y), Vec2::new(w, h))
}

#[cfg(test)]
mod tests {
    use super::*;

    const PARENT_OLD: Vec2 = Vec2::new(100.0, 100.0);
    const PARENT_NEW: Vec2 = Vec2::new(200.0, 50.0);

    fn child() -> Rect {
        Rect::from_min_size(Pos2::new(10.0, 20.0), Vec2::new(30.0, 40.0))
    }

    #[test]
    fn no_constraints_scales_with_the_parent() {
        let bounds = constrained_child_bounds(child(), &ResizingConstraints::default(), PARENT_OLD, PARENT_NEW);
        assert_eq!(bounds, Rect::from_min_size(Pos2::new(20.0, 10.0), Vec2::new(60.0, 20.0)));
    }

    #[test]
    fn fixed_size_without_pins_keeps_size_and_scales_the_center() {
        let c = ResizingConstraints { fixed_width: true, fixed_height: true, ..Default::default() };
        let bounds = constrained_child_bounds(child(), &c, PARENT_OLD, PARENT_NEW);
        // Centers (25, 40) scale to (50, 20).
        assert_eq!(bounds, Rect::from_center_size(Pos2::new(50.0, 20.0), Vec2::new(30.0, 40.0)));
    }

    #[test]
    fn pin_start_keeps_the_start_margin() {
        let c = ResizingConstraints { pin_left: true, fixed_width: true, pin_top: true, ..Default::default() };
        let bounds = constrained_child_bounds(child(), &c, PARENT_OLD, PARENT_NEW);
        assert_eq!(bounds, Rect::from_min_size(Pos2::new(10.0, 20.0), Vec2::new(30.0, 20.0)));
    }

    #[test]
    fn pin_end_keeps_the_end_margin() {
        let c = ResizingConstraints { pin_right: true, fixed_width: true, ..Default::default() };
        let bounds = constrained_child_bounds(child(), &c, PARENT_OLD, PARENT_NEW);
        // Right margin was 100 - 40 = 60, so the child ends at 200 - 60.
        assert_eq!(bounds.max.x, 140.0);
        assert_eq!(bounds.width(), 30.0);
    }

    #[test]
    fn both_pins_stretch_and_ignore_fixed_size() {
        let c = ResizingConstraints { pin_left: true, pin_right: true, fixed_width: true, ..Default::default() };
        let bounds = constrained_child_bounds(child(), &c, PARENT_OLD, PARENT_NEW);
        assert_eq!((bounds.min.x, bounds.width()), (10.0, 130.0));
    }

    #[test]
    fn stretched_size_never_goes_negative() {
        let c = ResizingConstraints { pin_left: true, pin_right: true, ..Default::default() };
        let bounds = constrained_child_bounds(child(), &c, PARENT_OLD, Vec2::new(10.0, 100.0));
        assert_eq!(bounds.width(), 0.0);
    }
}
