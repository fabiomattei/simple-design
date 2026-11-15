//! Numeric move/resize of a whole multi-selection as one bounding box — the
//! inspector's multi-selection X/Y/W/H fields. Each selected layer is mapped
//! from the old box into the new one and resized with
//! `canvas::resize_layer`, so containers follow their children's resizing
//! constraints exactly as a canvas handle drag does.
//!
//! Coordinates are in the space of the *first* selected layer's parent (the
//! same parent-relative convention as a single layer's X/Y fields). For the
//! usual case of sibling layers that is their shared parent; for layers under
//! different parents it is still one consistent space.

use egui::{Pos2, Rect, Vec2};

use crate::canvas::resize_layer;
use crate::model::{LayerId, Page};

/// The combined bounding box of `ids` (each layer's own unrotated frame
/// bounds, as the canvas's resize handles use), relative to the first id's
/// parent. `None` if none of `ids` exist.
pub fn selection_bounds(page: &Page, ids: &[LayerId]) -> Option<Rect> {
    let abs = absolute_bounds(page, ids).into_iter().map(|(_, b, _)| b).reduce(|a, b| a.union(b))?;
    Some(abs.translate(-reference_offset(page, ids)))
}

/// Moves and resizes every layer in `ids` so their combined bounding box
/// becomes `new_bounds` (same coordinate space as `selection_bounds`).
/// Stroke widths and corner radii are left unchanged, as with a plain
/// (non-Scale-mode) canvas resize. No-op if none of `ids` exist.
pub fn set_selection_bounds(page: &mut Page, ids: &[LayerId], new_bounds: Rect) {
    let layers = absolute_bounds(page, ids);
    let Some(old) = layers.iter().map(|(_, b, _)| *b).reduce(|a, b| a.union(b)) else {
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
    for (id, abs, offset) in layers {
        let new_layer_abs = Rect::from_two_pos(transform(abs.min), transform(abs.max));
        resize_layer(page, id, new_layer_abs.translate(-offset));
    }
}

/// `(id, page-space frame bounds, parent offset)` for each of `ids` that
/// exists.
fn absolute_bounds(page: &Page, ids: &[LayerId]) -> Vec<(LayerId, Rect, Vec2)> {
    ids.iter()
        .filter_map(|&id| {
            let offset = page.absolute_offset(id)?;
            Some((id, page.find(id)?.frame.bounds().translate(offset), offset))
        })
        .collect()
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
    fn an_artboard_is_measured_and_resized_by_its_own_frame_not_its_content() {
        let child = rect_layer(10.0, 10.0, 10.0, 10.0);
        let child_id = child.id;
        let mut artboard = Layer::new_artboard(
            "Board",
            Frame { pos: Pos2::ZERO, size: Vec2::new(100.0, 100.0), rotation: 0.0 },
        );
        let LayerKind::Artboard { children, .. } = &mut artboard.kind else { unreachable!() };
        children.push(child);
        let (mut page, ids) = page_with(vec![artboard, rect_layer(200.0, 0.0, 10.0, 10.0)]);

        assert_eq!(selection_bounds(&page, &ids), Some(Rect::from_min_max(Pos2::ZERO, Pos2::new(210.0, 100.0))));
        set_selection_bounds(&mut page, &ids, Rect::from_min_max(Pos2::ZERO, Pos2::new(420.0, 100.0)));
        assert_eq!(page.layers[0].frame.size, Vec2::new(200.0, 100.0), "artboard keeps its own frame, scaled");
        assert_eq!(page.find(child_id).unwrap().frame.pos, Pos2::new(10.0, 10.0), "content stays put");
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
