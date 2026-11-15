//! Infers flexbox rows and columns from absolutely-placed rectangles, by
//! recursive whitespace cuts ("XY-cut"): if a straight line of empty space
//! separates the items top-to-bottom they form a column; otherwise, if one
//! separates them left-to-right, a row; each group is then cut again.
//!
//! Items no cut can separate overlap each other, which flex layout can't
//! express. One of them — the largest, typically a backdrop behind the rest —
//! is set aside to stay absolutely positioned, and the cut is retried on the
//! others, so one overlap never forces the whole container out of flow.
//! Pure geometry: no layers, no CSS.

use egui::Rect;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Axis {
    /// Items side by side, left to right.
    Row,
    /// Items stacked, top to bottom.
    Column,
}

#[derive(Debug, PartialEq)]
pub enum LayoutNode {
    /// `index` is the item's position in `infer_layout`'s input.
    Item { index: usize, bounds: Rect },
    /// `children` are in visual order along `axis`; `bounds` is their union.
    Stack { axis: Axis, bounds: Rect, children: Vec<LayoutNode> },
}

impl LayoutNode {
    pub fn bounds(&self) -> Rect {
        match self {
            LayoutNode::Item { bounds, .. } | LayoutNode::Stack { bounds, .. } => *bounds,
        }
    }
}

#[derive(Debug, PartialEq)]
pub struct InferredLayout {
    /// `None` only when there were no items.
    pub flow: Option<LayoutNode>,
    /// Indices set aside for absolute positioning, in the order they were set aside.
    pub absolute: Vec<usize>,
}

/// Lays out `items` (`(index, rect)` pairs; the index is returned as-is in
/// the result) as nested rows and columns. Pure.
pub fn infer_layout(items: &[(usize, Rect)]) -> InferredLayout {
    let mut absolute = Vec::new();
    let flow = if items.is_empty() { None } else { Some(cut(items.to_vec(), &mut absolute)) };
    InferredLayout { flow, absolute }
}

/// `items` must be non-empty.
fn cut(mut items: Vec<(usize, Rect)>, absolute: &mut Vec<usize>) -> LayoutNode {
    if let [(index, bounds)] = items.as_slice() {
        return LayoutNode::Item { index: *index, bounds: *bounds };
    }
    for axis in [Axis::Column, Axis::Row] {
        let groups = split_by_whitespace(&items, axis);
        if groups.len() > 1 {
            let children: Vec<LayoutNode> = groups.into_iter().map(|group| cut(group, absolute)).collect();
            let bounds = children.iter().map(LayoutNode::bounds).reduce(|a, b| a.union(b)).unwrap_or(Rect::NOTHING);
            return LayoutNode::Stack { axis, bounds, children };
        }
    }
    let largest = (0..items.len())
        .max_by(|&a, &b| items[a].1.area().total_cmp(&items[b].1.area()))
        .unwrap_or(0);
    absolute.push(items.remove(largest).0);
    cut(items, absolute)
}

/// Groups `items` into runs separated by empty space along `axis`, in
/// visual order. Touching edges count as separated; any overlap joins.
fn split_by_whitespace(items: &[(usize, Rect)], axis: Axis) -> Vec<Vec<(usize, Rect)>> {
    let span = |rect: &Rect| match axis {
        Axis::Column => (rect.min.y, rect.max.y),
        Axis::Row => (rect.min.x, rect.max.x),
    };
    let mut sorted = items.to_vec();
    sorted.sort_by(|a, b| span(&a.1).0.total_cmp(&span(&b.1).0));
    let mut groups: Vec<Vec<(usize, Rect)>> = Vec::new();
    let mut group_end = f32::NEG_INFINITY;
    for item in sorted {
        let (start, end) = span(&item.1);
        match groups.last_mut() {
            Some(group) if start < group_end => group.push(item),
            _ => groups.push(vec![item]),
        }
        group_end = group_end.max(end);
    }
    groups
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::{Pos2, Vec2};

    fn rect(x: f32, y: f32, w: f32, h: f32) -> Rect {
        Rect::from_min_size(Pos2::new(x, y), Vec2::new(w, h))
    }

    fn item_indices(node: &LayoutNode) -> Vec<usize> {
        match node {
            LayoutNode::Item { index, .. } => vec![*index],
            LayoutNode::Stack { children, .. } => children.iter().flat_map(item_indices).collect(),
        }
    }

    #[test]
    fn stacked_items_form_a_column_in_visual_order() {
        let layout = infer_layout(&[(0, rect(0.0, 100.0, 50.0, 20.0)), (1, rect(0.0, 0.0, 50.0, 20.0))]);
        let Some(LayoutNode::Stack { axis, .. }) = &layout.flow else { panic!("{layout:?}") };
        assert_eq!(*axis, Axis::Column);
        assert_eq!(item_indices(layout.flow.as_ref().unwrap()), [1, 0]);
        assert!(layout.absolute.is_empty());
    }

    #[test]
    fn header_over_two_cards_is_a_column_containing_a_row() {
        let layout = infer_layout(&[(0, rect(0.0, 0.0, 200.0, 30.0)), (1, rect(0.0, 50.0, 90.0, 90.0)), (2, rect(110.0, 50.0, 90.0, 90.0))]);
        let Some(LayoutNode::Stack { axis: Axis::Column, children, .. }) = &layout.flow else { panic!("{layout:?}") };
        assert!(matches!(children[0], LayoutNode::Item { index: 0, .. }));
        assert!(matches!(&children[1], LayoutNode::Stack { axis: Axis::Row, children, .. } if children.len() == 2));
    }

    #[test]
    fn a_backdrop_behind_everything_is_set_aside_and_the_rest_still_flows() {
        let layout = infer_layout(&[(0, rect(0.0, 0.0, 300.0, 300.0)), (1, rect(10.0, 10.0, 50.0, 20.0)), (2, rect(10.0, 50.0, 50.0, 20.0))]);
        assert_eq!(layout.absolute, [0]);
        assert_eq!(item_indices(layout.flow.as_ref().unwrap()), [1, 2]);
    }

    #[test]
    fn no_items_means_no_flow() {
        assert_eq!(infer_layout(&[]), InferredLayout { flow: None, absolute: Vec::new() });
    }
}
