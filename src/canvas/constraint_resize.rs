//! Resizes a layer tree according to its children's resizing constraints
//! (`crate::resizing_constraints`) and the Artboard's
//! `adjust_content_on_resize` flag — shared by the canvas's resize drag, the
//! inspector's W/H fields and `selection_transform`.

use egui::{Pos2, Rect, Vec2};

use crate::model::{Layer, LayerId, LayerKind, Page};
use crate::resizing_constraints::constrained_child_bounds;

use super::transform_geometry::{ResizeLayerInfo, apply_resize_delta, collect_resizable_leaves, refit_container_to_children};

/// Resizes layer `id` so its unrotated bounds become `new_bounds` (in its
/// parent's local space), laying out its content:
/// - a leaf is scaled into `new_bounds` (paths keep their shape);
/// - a Group's children follow their constraints, then it is refit to them;
/// - an Artboard's children follow their constraints only with
///   `adjust_content_on_resize`; otherwise they keep their page position;
/// - a BooleanGroup's operands always scale with it, ignoring constraints.
///
/// No-op if `id` doesn't exist.
pub(crate) fn resize_layer(page: &mut Page, id: LayerId, new_bounds: Rect) {
    let Some(layer) = page.find(id) else { return };
    let old_bounds = layer.frame.bounds();
    match &layer.kind {
        LayerKind::Group { .. } => {
            layout_children(page, id, old_bounds, new_bounds);
            refit_container_to_children(page, id);
        }
        LayerKind::Artboard { adjust_content_on_resize: true, .. } => layout_children(page, id, old_bounds, new_bounds),
        LayerKind::Artboard { .. } => set_frame_keeping_children_in_place(page, id, new_bounds),
        _ => scale_into(page, id, old_bounds, new_bounds),
    }
}

/// Scales `id` and every leaf under it from `old_bounds` to `new_bounds`
/// with the canvas's resize-drag machinery, in `id`'s parent-local space
/// (hence the zero parent offset), then refits any container.
fn scale_into(page: &mut Page, id: LayerId, old_bounds: Rect, new_bounds: Rect) {
    let mut leaves: Vec<ResizeLayerInfo> = Vec::new();
    let Some(layer) = page.find(id) else { return };
    collect_resizable_leaves(layer, Vec2::ZERO, &mut leaves);
    let axis_scale = |new: f32, old: f32| if old.abs() > 1e-6 { new / old } else { 1.0 };
    let scale = Vec2::new(axis_scale(new_bounds.width(), old_bounds.width()), axis_scale(new_bounds.height(), old_bounds.height()));
    let transform = |p: Pos2| {
        Pos2::new(
            new_bounds.min.x + (p.x - old_bounds.min.x) * scale.x,
            new_bounds.min.y + (p.y - old_bounds.min.y) * scale.y,
        )
    };
    apply_resize_delta(page, &leaves, transform, scale, false, 1.0);
    refit_container_to_children(page, id);
}

/// Sets container `id`'s frame to `new_bounds`, then resizes each child to
/// its constrained bounds (recursively, via `resize_layer`).
fn layout_children(page: &mut Page, id: LayerId, old_bounds: Rect, new_bounds: Rect) {
    let Some(layer) = page.find_mut(id) else { return };
    set_frame_bounds(layer, new_bounds);
    let Some(children) = layer.kind.children() else { return };
    // Children's local space is relative to the container's `frame.pos`,
    // which `set_frame_bounds` already moved — so a child's local bounds
    // are unchanged by that move, only its constrained layout differs.
    let targets: Vec<(LayerId, Rect)> = children
        .iter()
        .map(|c| (c.id, constrained_child_bounds(c.frame.bounds(), &c.resizing, old_bounds.size(), new_bounds.size())))
        .collect();
    for (child_id, child_bounds) in targets {
        resize_layer(page, child_id, child_bounds);
    }
}

/// Sets container `id`'s frame to `new_bounds` and shifts its children back
/// by the frame's movement, so they stay where they are on the page.
fn set_frame_keeping_children_in_place(page: &mut Page, id: LayerId, new_bounds: Rect) {
    let Some(layer) = page.find_mut(id) else { return };
    let moved = set_frame_bounds(layer, new_bounds);
    if let Some(children) = layer.kind.children_mut() {
        for child in children {
            child.frame.pos -= moved;
        }
    }
}

/// Sets container `layer`'s frame to `new_bounds`; returns how far
/// `frame.pos` moved. Only used for Group/Artboard, whose size is never
/// negative (unlike a Line's), so `pos` is always the min corner.
fn set_frame_bounds(layer: &mut Layer, new_bounds: Rect) -> Vec2 {
    let moved = new_bounds.min - layer.frame.pos;
    layer.frame.pos = new_bounds.min;
    layer.frame.size = new_bounds.size();
    moved
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{CornerRadii, Frame, ResizingConstraints};

    const PARENT_OLD: Vec2 = Vec2::new(100.0, 100.0);

    fn rect_layer(x: f32, y: f32, w: f32, h: f32) -> Layer {
        Layer::new(
            "Rect",
            Frame::from_two_points(Pos2::new(x, y), Pos2::new(x + w, y + h)),
            LayerKind::Rectangle { corner_radius: CornerRadii::ZERO },
        )
    }

    fn container(kind: LayerKind, children: Vec<Layer>) -> (Page, LayerId, Vec<LayerId>) {
        let child_ids = children.iter().map(|c| c.id).collect();
        let mut layer = Layer::new("Container", Frame { pos: Pos2::new(50.0, 50.0), size: PARENT_OLD, rotation: 0.0 }, kind);
        *layer.kind.children_mut().unwrap() = children;
        let id = layer.id;
        let mut page = Page::new("Page 1");
        page.layers.push(layer);
        (page, id, child_ids)
    }

    #[test]
    fn artboard_without_adjust_content_keeps_children_on_the_page() {
        let kind = LayerKind::Artboard { children: Vec::new(), background: egui::Color32::WHITE, adjust_content_on_resize: false };
        let (mut page, id, child_ids) = container(kind, vec![rect_layer(10.0, 10.0, 10.0, 10.0)]);
        let before = page.absolute_offset(child_ids[0]).unwrap() + page.find(child_ids[0]).unwrap().frame.pos.to_vec2();

        resize_layer(&mut page, id, Rect::from_min_size(Pos2::new(30.0, 50.0), Vec2::new(300.0, 300.0)));

        let artboard = page.find(id).unwrap();
        assert_eq!((artboard.frame.pos, artboard.frame.size), (Pos2::new(30.0, 50.0), Vec2::new(300.0, 300.0)));
        let after = page.absolute_offset(child_ids[0]).unwrap() + page.find(child_ids[0]).unwrap().frame.pos.to_vec2();
        assert_eq!(after, before);
        assert_eq!(page.find(child_ids[0]).unwrap().frame.size, Vec2::new(10.0, 10.0));
    }

    #[test]
    fn artboard_with_adjust_content_lays_children_out_by_their_constraints() {
        let kind = LayerKind::Artboard { children: Vec::new(), background: egui::Color32::WHITE, adjust_content_on_resize: true };
        let mut pinned = rect_layer(80.0, 0.0, 10.0, 10.0);
        pinned.resizing = ResizingConstraints { pin_right: true, fixed_width: true, pin_top: true, fixed_height: true, ..Default::default() };
        let (mut page, id, child_ids) = container(kind, vec![pinned, rect_layer(0.0, 0.0, 50.0, 50.0)]);

        resize_layer(&mut page, id, Rect::from_min_size(Pos2::new(50.0, 50.0), Vec2::new(200.0, 100.0)));

        // Pinned right with a 10 margin: stays 10 from the new right edge.
        assert_eq!(page.find(child_ids[0]).unwrap().frame.pos, Pos2::new(180.0, 0.0));
        // Unconstrained: scales 2x horizontally.
        assert_eq!(page.find(child_ids[1]).unwrap().frame.size, Vec2::new(100.0, 50.0));
    }

    #[test]
    fn group_lays_children_out_and_refits_to_them() {
        let mut fixed = rect_layer(0.0, 0.0, 10.0, 100.0);
        fixed.resizing = ResizingConstraints { pin_left: true, fixed_width: true, ..Default::default() };
        let (mut page, id, child_ids) =
            container(LayerKind::Group { children: Vec::new() }, vec![fixed, rect_layer(90.0, 0.0, 10.0, 100.0)]);

        resize_layer(&mut page, id, Rect::from_min_size(Pos2::new(50.0, 50.0), Vec2::new(200.0, 100.0)));

        assert_eq!(page.find(child_ids[0]).unwrap().frame.size, Vec2::new(10.0, 100.0));
        assert_eq!(page.find(child_ids[1]).unwrap().frame.size, Vec2::new(20.0, 100.0));
        let group = page.find(id).unwrap();
        assert_eq!((group.frame.pos, group.frame.size), (Pos2::new(50.0, 50.0), Vec2::new(200.0, 100.0)));
    }

    #[test]
    fn leaf_scales_into_the_new_bounds() {
        let layer = rect_layer(0.0, 0.0, 10.0, 10.0);
        let id = layer.id;
        let mut page = Page::new("Page 1");
        page.layers.push(layer);
        resize_layer(&mut page, id, Rect::from_min_size(Pos2::new(5.0, 5.0), Vec2::new(20.0, 40.0)));
        let frame = page.find(id).unwrap().frame;
        assert_eq!((frame.pos, frame.size), (Pos2::new(5.0, 5.0), Vec2::new(20.0, 40.0)));
    }
}
