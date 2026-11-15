//! Recognizes an inferred column of rows that is really a grid: every row
//! has the same number of cells, and each cell column starts at the same x
//! in every row with an even spacing between columns. Such a stack exports
//! as `display: grid`, which keeps the columns aligned the way the flex
//! rows would only by coincidence. Pure geometry.

use super::layout_inference::{Axis, LayoutNode};
use super::stack_spacing::within_tolerance;

#[derive(Debug, PartialEq)]
pub struct GridSpec {
    /// Each column's track width: its widest cell.
    pub column_widths: Vec<f32>,
    /// The average space between columns.
    pub column_gap: f32,
}

/// `Some` when `rows` (the children of a `Column` stack, in order) form a
/// grid of at least 2×2. Tolerances are `stack_spacing::TOLERANCE`.
pub fn detect_grid(rows: &[LayoutNode]) -> Option<GridSpec> {
    let cell_rows: Vec<&[LayoutNode]> = rows
        .iter()
        .map(|row| match row {
            LayoutNode::Stack { axis: Axis::Row, children, .. } => Some(children.as_slice()),
            _ => None,
        })
        .collect::<Option<_>>()?;
    let column_count = cell_rows.first()?.len();
    if cell_rows.len() < 2 || column_count < 2 || cell_rows.iter().any(|cells| cells.len() != column_count) {
        return None;
    }
    let rows_have_aligned_tops = rows.iter().zip(&cell_rows).all(|(row, cells)| {
        let tops: Vec<f32> = cells.iter().map(|cell| cell.bounds().min.y - row.bounds().min.y).collect();
        within_tolerance(&tops)
    });
    if !rows_have_aligned_tops {
        return None;
    }

    let mut column_starts = Vec::with_capacity(column_count);
    let mut column_widths = Vec::with_capacity(column_count);
    for column in 0..column_count {
        let starts: Vec<f32> = cell_rows.iter().map(|cells| cells[column].bounds().min.x).collect();
        if !within_tolerance(&starts) {
            return None;
        }
        let start = starts.iter().copied().fold(f32::INFINITY, f32::min);
        let width = cell_rows.iter().map(|cells| cells[column].bounds().max.x - start).fold(0.0, f32::max);
        column_starts.push(start);
        column_widths.push(width);
    }
    let gaps: Vec<f32> = (1..column_count).map(|c| column_starts[c] - (column_starts[c - 1] + column_widths[c - 1])).collect();
    if gaps.iter().any(|&gap| gap < 0.0) || !within_tolerance(&gaps) {
        return None;
    }
    let column_gap = gaps.iter().sum::<f32>() / gaps.len() as f32;
    Some(GridSpec { column_widths, column_gap })
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::{Pos2, Rect, Vec2};

    fn item(index: usize, x: f32, y: f32, w: f32, h: f32) -> LayoutNode {
        LayoutNode::Item { index, bounds: Rect::from_min_size(Pos2::new(x, y), Vec2::new(w, h)) }
    }

    fn row(children: Vec<LayoutNode>) -> LayoutNode {
        let bounds = children.iter().map(LayoutNode::bounds).reduce(|a, b| a.union(b)).unwrap();
        LayoutNode::Stack { axis: Axis::Row, bounds, children }
    }

    #[test]
    fn two_by_two_cards_form_a_grid() {
        let rows = [
            row(vec![item(0, 0.0, 0.0, 100.0, 50.0), item(1, 120.0, 0.0, 100.0, 50.0)]),
            row(vec![item(2, 0.0, 80.0, 98.0, 40.0), item(3, 121.0, 80.0, 100.0, 40.0)]),
        ];
        assert_eq!(detect_grid(&rows), Some(GridSpec { column_widths: vec![100.0, 101.0], column_gap: 20.0 }));
    }

    #[test]
    fn rows_with_different_cell_counts_are_not_a_grid() {
        let rows = [row(vec![item(0, 0.0, 0.0, 50.0, 50.0), item(1, 60.0, 0.0, 50.0, 50.0)]), row(vec![item(2, 0.0, 80.0, 50.0, 50.0), item(3, 60.0, 80.0, 50.0, 50.0), item(4, 120.0, 80.0, 50.0, 50.0)])];
        assert_eq!(detect_grid(&rows), None);
    }

    #[test]
    fn misaligned_columns_are_not_a_grid() {
        let rows = [row(vec![item(0, 0.0, 0.0, 50.0, 50.0), item(1, 60.0, 0.0, 50.0, 50.0)]), row(vec![item(2, 0.0, 80.0, 50.0, 50.0), item(3, 90.0, 80.0, 50.0, 50.0)])];
        assert_eq!(detect_grid(&rows), None);
    }

    #[test]
    fn a_single_row_is_not_a_grid() {
        let rows = [row(vec![item(0, 0.0, 0.0, 50.0, 50.0), item(1, 60.0, 0.0, 50.0, 50.0)])];
        assert_eq!(detect_grid(&rows), None);
    }
}
