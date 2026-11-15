//! Numeric move/resize of a whole multi-selection as one bounding box — the
//! inspector's multi-selection X/Y/W/H fields. Reuses the canvas's resize
//! drag machinery (`collect_resizable_leaves`/`apply_resize_delta`), so a
//! typed value moves and scales layers exactly as dragging a handle would.
//!
//! Coordinates are in the space of the *first* selected layer's parent (the
//! same parent-relative convention as a single layer's X/Y fields). For the
//! usual case of sibling layers that is their shared parent; for layers under
//! different parents it is still one consistent space.

use egui::{Pos2, Rect, Vec2};

use crate::canvas::{ResizeLayerInfo, apply_resize_delta, collect_resizable_leaves, refit_container_to_children};
use crate::model::{LayerId, Page};

/// The combined bounding box of `ids` (unrotated leaf bounds, as the canvas's
/// resize handles use), relative to the first id's parent. `None` if none of
/// `ids` exist.
pub fn selection_bounds(page: &Page, ids: &[LayerId]) -> Option<Rect> {
    let leaves = collect_leaves(page, ids);
    let abs = leaves.iter().map(|l| l.abs_bounds).reduce(|a, b| a.union(b))?;
    Some(abs.translate(-reference_offset(page, ids)))
}

/// Moves and scales every layer in `ids` so their combined bounding box
/// becomes `new_bounds` (same coordinate space as `selection_bounds`).
/// Stroke widths and corner radii are left unchanged, as with a plain
/// (non-Scale-mode) canvas resize. Each selected container is refit to its
/// children afterwards. No-op if none of `ids` exist.
pub fn set_selection_bounds(page: &mut Page, ids: &[LayerId], new_bounds: Rect) {
    let leaves = collect_leaves(page, ids);
    let Some(old) = leaves.iter().map(|l| l.abs_bounds).reduce(|a, b| a.union(b)) else {
        return;
    };
    let new_abs = new_bounds.translate(reference_offset(page, ids));
    // A zero-width/height selection (e.g. a horizontal Line) can't be scaled
    // along that axis — keep it at 1.0, as the canvas's resize drag does.
    let axis_scale = |new: f32, old: f32| if old.abs() > 1e-6 { new / old } else { 1.0 };
    let scale = Vec2::new(axis_scale(new_abs.width(), old.width()), axis_scale(new_abs.height(), old.height()));
    let transform = |p: Pos2| {
        Pos2::new(
            new_abs.min.x + (p.x - old.min.x) * scale.x,
            new_abs.min.y + (p.y - old.min.y) * scale.y,
        )
    };
    apply_resize_delta(page, &leaves, transform, scale, false, 1.0);
    for &id in ids {
        refit_container_to_children(page, id);
    }
}

fn collect_leaves(page: &Page, ids: &[LayerId]) -> Vec<ResizeLayerInfo> {
    let mut leaves = Vec::new();
    for &id in ids {
        if let (Some(layer), Some(offset)) = (page.find(id), page.absolute_offset(id)) {
            collect_resizable_leaves(layer, offset, &mut leaves);
        }
    }
    leaves
}

fn reference_offset(page: &Page, ids: &[LayerId]) -> Vec2 {
    ids.first().and_then(|&id| page.absolute_offset(id)).unwrap_or(Vec2::ZERO)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{CornerRadii, Frame, Layer, LayerKind};

    fn rect_layer(x: f32, y: f32, w: f32, h: f32) -> Layer {
        Layer::new(
            "Rect",
            Frame::from_two_points(Pos2::new(x, y), Pos2::new(x + w, y + h)),
            LayerKind::Rectangle { corner_radius: CornerRadii::ZERO },
        )
    }

    fn page_with(layers: Vec<Layer>) -> (Page, Vec<LayerId>) {
        let ids = layers.iter().map(|l| l.id).collect();
        let mut page = Page::new("Page 1");
        page.layers = layers;
        (page, ids)
    }

    #[test]
    fn selection_bounds_is_the_union_of_the_layers() {
        let (page, ids) = page_with(vec![rect_layer(0.0, 0.0, 10.0, 10.0), rect_layer(20.0, 5.0, 10.0, 20.0)]);
        assert_eq!(selection_bounds(&page, &ids), Some(Rect::from_min_max(Pos2::ZERO, Pos2::new(30.0, 25.0))));
    }

    #[test]
    fn set_selection_bounds_with_same_size_moves_every_layer() {
        let (mut page, ids) = page_with(vec![rect_layer(0.0, 0.0, 10.0, 10.0), rect_layer(20.0, 5.0, 10.0, 20.0)]);
        set_selection_bounds(&mut page, &ids, Rect::from_min_size(Pos2::new(100.0, 50.0), Vec2::new(30.0, 25.0)));
        assert_eq!(page.layers[0].frame.pos, Pos2::new(100.0, 50.0));
        assert_eq!(page.layers[1].frame.pos, Pos2::new(120.0, 55.0));
        assert_eq!(page.layers[1].frame.size, Vec2::new(10.0, 20.0));
    }

    #[test]
    fn set_selection_bounds_with_double_size_scales_positions_and_sizes() {
        let (mut page, ids) = page_with(vec![rect_layer(0.0, 0.0, 10.0, 10.0), rect_layer(20.0, 0.0, 10.0, 10.0)]);
        set_selection_bounds(&mut page, &ids, Rect::from_min_size(Pos2::ZERO, Vec2::new(60.0, 20.0)));
        assert_eq!(page.layers[0].frame.size, Vec2::new(20.0, 20.0));
        assert_eq!(page.layers[1].frame.pos, Pos2::new(40.0, 0.0));
        assert_eq!(page.layers[1].frame.size, Vec2::new(20.0, 20.0));
    }

    #[test]
    fn coordinates_are_relative_to_the_first_layers_parent() {
        let a = rect_layer(10.0, 10.0, 10.0, 10.0);
        let b = rect_layer(30.0, 10.0, 10.0, 10.0);
        let ids = vec![a.id, b.id];
        let mut artboard = Layer::new_artboard(
            "Board",
            Frame { pos: Pos2::new(100.0, 200.0), size: Vec2::new(300.0, 300.0), rotation: 0.0 },
        );
        let LayerKind::Artboard { children, .. } = &mut artboard.kind else { unreachable!() };
        children.extend([a, b]);
        let mut page = Page::new("Page 1");
        page.layers.push(artboard);

        assert_eq!(
            selection_bounds(&page, &ids),
            Some(Rect::from_min_max(Pos2::new(10.0, 10.0), Pos2::new(40.0, 20.0)))
        );
        set_selection_bounds(&mut page, &ids, Rect::from_min_max(Pos2::new(0.0, 0.0), Pos2::new(30.0, 10.0)));
        assert_eq!(page.find(ids[0]).unwrap().frame.pos, Pos2::new(0.0, 0.0));
        assert_eq!(page.find(ids[1]).unwrap().frame.pos, Pos2::new(20.0, 0.0));
    }
}
