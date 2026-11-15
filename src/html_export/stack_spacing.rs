//! The spacing and alignment of one inferred row or column
//! (`layout_inference::LayoutNode::Stack`): a single `gap` when the
//! spaces between its children are near-equal, otherwise one margin per
//! child; and a shared cross-axis alignment when the children line up on a
//! common edge or center, otherwise per-child cross margins. Pure geometry.

use egui::Rect;

use super::layout_inference::Axis;

/// How far apart (in document units) gaps or edges may be and still count
/// as equal. Snapping to their average moves elements by at most this much,
/// in exchange for much cleaner CSS (`gap` / `align-items`). Shared with
/// `grid_detection`, so rows/columns and grids snap the same way.
pub(super) const TOLERANCE: f32 = 3.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CrossAlign {
    Start,
    Center,
    End,
}

#[derive(Debug, PartialEq)]
pub struct StackSpacing {
    /// `Some` when every gap is within `TOLERANCE` of the others (their
    /// average); `main_margins` are then all `0.0`.
    pub gap: Option<f32>,
    /// Space before each child along the axis; the first is always `0.0`.
    pub main_margins: Vec<f32>,
    pub align: CrossAlign,
    /// Offset of each child from the stack's cross-axis start; all `0.0`
    /// unless the children share no alignment (`align` is then `Start`).
    pub cross_margins: Vec<f32>,
}

/// Spacing for children with `child_bounds` (in visual order along `axis`)
/// inside a stack spanning `stack_bounds`. `child_bounds` must be non-empty
/// and separated along `axis`, as `layout_inference` guarantees.
pub fn stack_spacing(axis: Axis, stack_bounds: Rect, child_bounds: &[Rect]) -> StackSpacing {
    let main = |rect: &Rect| match axis {
        Axis::Column => (rect.min.y, rect.max.y),
        Axis::Row => (rect.min.x, rect.max.x),
    };
    let cross = |rect: &Rect| match axis {
        Axis::Column => (rect.min.x, rect.max.x),
        Axis::Row => (rect.min.y, rect.max.y),
    };
    let count = child_bounds.len();
    let gaps: Vec<f32> = child_bounds.windows(2).map(|pair| main(&pair[1]).0 - main(&pair[0]).1).collect();
    let (gap, main_margins) = if within_tolerance(&gaps) && !gaps.is_empty() {
        (Some(gaps.iter().sum::<f32>() / gaps.len() as f32), vec![0.0; count])
    } else {
        (None, std::iter::once(0.0).chain(gaps.iter().copied()).collect())
    };

    let (stack_start, stack_end) = cross(&stack_bounds);
    let starts: Vec<f32> = child_bounds.iter().map(|r| cross(r).0 - stack_start).collect();
    let ends: Vec<f32> = child_bounds.iter().map(|r| stack_end - cross(r).1).collect();
    let centers: Vec<f32> = child_bounds.iter().map(|r| (cross(r).0 + cross(r).1) / 2.0).collect();
    let (align, cross_margins) = if within_tolerance(&starts) {
        (CrossAlign::Start, vec![0.0; count])
    } else if within_tolerance(&centers) {
        (CrossAlign::Center, vec![0.0; count])
    } else if within_tolerance(&ends) {
        (CrossAlign::End, vec![0.0; count])
    } else {
        (CrossAlign::Start, starts)
    };
    StackSpacing { gap, main_margins, align, cross_margins }
}

/// Whether all `values` lie within `TOLERANCE` of each other (true when empty).
pub(super) fn within_tolerance(values: &[f32]) -> bool {
    let min = values.iter().copied().fold(f32::INFINITY, f32::min);
    let max = values.iter().copied().fold(f32::NEG_INFINITY, f32::max);
    values.is_empty() || max - min <= TOLERANCE
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::{Pos2, Vec2};

    fn rect(x: f32, y: f32, w: f32, h: f32) -> Rect {
        Rect::from_min_size(Pos2::new(x, y), Vec2::new(w, h))
    }

    #[test]
    fn near_equal_gaps_become_one_averaged_gap() {
        let children = [rect(0.0, 0.0, 10.0, 10.0), rect(0.0, 30.0, 10.0, 10.0), rect(0.0, 62.0, 10.0, 10.0)];
        let spacing = stack_spacing(Axis::Column, rect(0.0, 0.0, 10.0, 72.0), &children);
        assert_eq!(spacing.gap, Some(21.0));
        assert_eq!(spacing.main_margins, [0.0, 0.0, 0.0]);
    }

    #[test]
    fn uneven_gaps_become_per_child_margins() {
        let children = [rect(0.0, 0.0, 10.0, 10.0), rect(20.0, 0.0, 10.0, 10.0), rect(80.0, 0.0, 10.0, 10.0)];
        let spacing = stack_spacing(Axis::Row, rect(0.0, 0.0, 90.0, 10.0), &children);
        assert_eq!(spacing.gap, None);
        assert_eq!(spacing.main_margins, [0.0, 10.0, 50.0]);
    }

    #[test]
    fn centered_children_align_center() {
        let children = [rect(0.0, 0.0, 100.0, 10.0), rect(25.0, 20.0, 50.0, 10.0)];
        let spacing = stack_spacing(Axis::Column, rect(0.0, 0.0, 100.0, 30.0), &children);
        assert_eq!(spacing.align, CrossAlign::Center);
    }

    #[test]
    fn unaligned_children_keep_their_cross_offsets() {
        let children = [rect(0.0, 0.0, 100.0, 10.0), rect(10.0, 20.0, 20.0, 10.0)];
        let spacing = stack_spacing(Axis::Column, rect(0.0, 0.0, 100.0, 30.0), &children);
        assert_eq!(spacing.align, CrossAlign::Start);
        assert_eq!(spacing.cross_margins, [0.0, 10.0]);
    }
}
