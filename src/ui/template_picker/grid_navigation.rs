//! Laying out the template picker's tile grid and moving its keyboard selection through it:
//! how many tiles fit in a row, the row and column of each tile, and the tile one arrow-key
//! step away. Ported from simple-text's welcome screen.

const MIN_TILE_GAP: f32 = 24.0;
/// Past this the tiles stop spreading, so a wide window does not scatter them.
const MAX_TILE_GAP: f32 = 48.0;

/// How many tiles of `tile_width` fit in a row of `width`, and the gap that spreads them
/// across it.
pub fn grid_columns(width: f32, tile_width: f32) -> (usize, f32) {
    let columns = (((width + MIN_TILE_GAP) / (tile_width + MIN_TILE_GAP)) as usize).max(1);
    let gap = if columns > 1 {
        ((width - columns as f32 * tile_width) / (columns - 1) as f32).clamp(MIN_TILE_GAP, MAX_TILE_GAP)
    } else {
        MIN_TILE_GAP
    };
    (columns, gap)
}

/// The row and column of every tile, section after section (`section_sizes` holds how many
/// tiles each has), with `columns` tiles per row; each section starts a new row.
pub fn grid_positions(section_sizes: &[usize], columns: usize) -> Vec<(usize, usize)> {
    let columns = columns.max(1);
    let mut positions = Vec::new();
    let mut first_row = 0;
    for &count in section_sizes {
        positions.extend((0..count).map(|index| (first_row + index / columns, index % columns)));
        first_row += count.div_ceil(columns);
    }
    positions
}

#[derive(Clone, Copy)]
pub enum Step {
    Left,
    Right,
    Up,
    Down,
}

/// The tile reached from the one at `from` in `positions` (from `grid_positions`) by one
/// `step`. Up and down go to the same column of the row above or below, or that row's last
/// tile when it is shorter; at an edge the selection stays.
pub fn step(positions: &[(usize, usize)], from: usize, step: Step) -> usize {
    let last = positions.len().saturating_sub(1);
    let (row, column) = positions[from];
    let target_row = match step {
        Step::Left => return from.saturating_sub(1),
        Step::Right => return (from + 1).min(last),
        Step::Up if row == 0 => return from,
        Step::Up => row - 1,
        Step::Down => row + 1,
    };
    positions
        .iter()
        .enumerate()
        .filter(|(_, (tile_row, tile_column))| *tile_row == target_row && *tile_column <= column)
        .map(|(index, _)| index)
        .next_back()
        .unwrap_or(from)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn positions_start_each_section_on_a_new_row() {
        assert_eq!(grid_positions(&[3, 0, 1], 2), [(0, 0), (0, 1), (1, 0), (2, 0)]);
    }

    #[test]
    fn steps_move_through_rows_and_clamp_to_shorter_rows() {
        // Rows: [0 1 2] [3] [4 5 6]
        let positions = [(0, 0), (0, 1), (0, 2), (1, 0), (2, 0), (2, 1), (2, 2)];
        assert_eq!(step(&positions, 2, Step::Down), 3);
        assert_eq!(step(&positions, 3, Step::Down), 4);
        assert_eq!(step(&positions, 5, Step::Up), 3);
        assert_eq!(step(&positions, 1, Step::Up), 1);
        assert_eq!(step(&positions, 6, Step::Down), 6);
        assert_eq!(step(&positions, 0, Step::Left), 0);
        assert_eq!(step(&positions, 6, Step::Right), 6);
    }

    #[test]
    fn columns_fit_the_width_and_spread_within_limits() {
        assert_eq!(grid_columns(100.0, 168.0), (1, MIN_TILE_GAP));
        let (columns, gap) = grid_columns(1000.0, 168.0);
        assert_eq!(columns, 5);
        assert!((MIN_TILE_GAP..=MAX_TILE_GAP).contains(&gap));
    }
}
