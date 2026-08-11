use egui::Vec2;

use crate::grouping;
use crate::model::{LayerId, Page};

/// The "Arrange > Make Grid" operation: duplicates the layer `id` across a
/// `cols` x `rows` grid (the original occupies the top-left cell, unmoved),
/// spaced by `gutter_x`/`gutter_y` from its own bounds size. Each duplicate
/// goes through `grouping::duplicate_layers` (same offset/rename/id-
/// regeneration behavior as Cmd+D), just called once per cell with that
/// cell's specific offset from the original rather than a single fixed
/// offset. Returns every new layer's id; empty if `cols`/`rows` is `0` or
/// `id` doesn't exist.
pub fn make_grid(page: &mut Page, id: LayerId, cols: u32, rows: u32, gutter_x: f32, gutter_y: f32) -> Vec<LayerId> {
    if cols == 0 || rows == 0 {
        return Vec::new();
    }
    let Some(original) = page.find(id) else { return Vec::new() };
    let size = original.frame.bounds().size();

    let mut new_ids = Vec::new();
    for row in 0..rows {
        for col in 0..cols {
            if row == 0 && col == 0 {
                continue; // the original itself occupies this cell.
            }
            let offset = Vec2::new(col as f32 * (size.x + gutter_x), row as f32 * (size.y + gutter_y));
            new_ids.extend(grouping::duplicate_layers(page, &[id], offset));
        }
    }
    new_ids
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{CornerRadii, Frame, Layer, LayerKind};
    use egui::Pos2;

    #[test]
    fn make_grid_places_cols_times_rows_minus_one_new_layers_on_a_grid() {
        let mut page = Page::new("Page 1");
        let layer = Layer::new(
            "Rect",
            Frame { pos: Pos2::new(0.0, 0.0), size: Vec2::new(10.0, 10.0), rotation: 0.0 },
            LayerKind::Rectangle { corner_radius: CornerRadii::ZERO },
        );
        let id = layer.id;
        page.layers.push(layer);

        let new_ids = make_grid(&mut page, id, 2, 3, 5.0, 5.0);
        assert_eq!(new_ids.len(), 5); // 2*3 - 1 (the original)
        assert_eq!(page.layers.len(), 6);

        // Cell (col=1, row=2) should sit at (15, 30).
        let target = Pos2::new(1.0 * (10.0 + 5.0), 2.0 * (10.0 + 5.0));
        assert!(page.layers.iter().any(|l| l.frame.pos == target));
        // The original stays exactly where it was.
        assert_eq!(page.find(id).unwrap().frame.pos, Pos2::ZERO);
    }

    #[test]
    fn make_grid_with_zero_cols_or_rows_is_a_noop() {
        let mut page = Page::new("Page 1");
        let layer = Layer::new(
            "Rect",
            Frame { pos: Pos2::ZERO, size: Vec2::new(10.0, 10.0), rotation: 0.0 },
            LayerKind::Rectangle { corner_radius: CornerRadii::ZERO },
        );
        let id = layer.id;
        page.layers.push(layer);

        assert!(make_grid(&mut page, id, 0, 3, 5.0, 5.0).is_empty());
        assert_eq!(page.layers.len(), 1);
    }
}
