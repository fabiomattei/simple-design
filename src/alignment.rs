use egui::{Pos2, Rect, Vec2};

use crate::grouping::find_common_parent_list;
use crate::model::{Layer, LayerId, LayerKind, Page};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AlignEdge {
    Left,
    HCenter,
    Right,
    Top,
    VCenter,
    Bottom,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DistributeAxis {
    Horizontal,
    Vertical,
}

/// Aligns every layer in `ids` (which must all be direct siblings — see
/// `find_common_parent_list`) to the given edge/center of `align_to` if
/// given, else their own combined bounding box (the default). `align_to` is
/// in the same coordinate space as the siblings' own `frame` — i.e. relative
/// to their common parent, not absolute page space (see
/// `artboard_bounds_in_parent_space` for computing an Artboard's bounds in
/// that space, and `reference layer` — the second override this backs, both
/// wired from `app.rs::align_selection`). Only translates `frame.pos`, so
/// each layer's size (and, for a `Line`, its drag direction) is preserved.
/// No-op if the ids don't share a common parent or fewer than two are found.
pub fn align(page: &mut Page, ids: &[LayerId], edge: AlignEdge, align_to: Option<Rect>) {
    if ids.len() < 2 {
        return;
    }
    let Some(siblings) = find_common_parent_list(&mut page.layers, ids) else {
        return;
    };
    let bbox = match align_to {
        Some(target) => target,
        None => {
            let bounds: Vec<Rect> = siblings
                .iter()
                .filter(|l| ids.contains(&l.id))
                .map(|l| l.frame.rotated_bounds())
                .collect();
            if bounds.len() < 2 {
                return;
            }
            bounds.into_iter().reduce(|a, b| a.union(b)).unwrap()
        }
    };

    for layer in siblings.iter_mut().filter(|l| ids.contains(&l.id)) {
        // A rotated layer's *visual* footprint (`rotated_bounds()`) is what
        // should align to the target edge — aligning by the unrotated local
        // `bounds()` would line up the invisible axis-aligned box instead of
        // what the user actually sees.
        let b = layer.frame.rotated_bounds();
        let delta = match edge {
            AlignEdge::Left => Vec2::new(bbox.min.x - b.min.x, 0.0),
            AlignEdge::HCenter => Vec2::new(bbox.center().x - b.center().x, 0.0),
            AlignEdge::Right => Vec2::new(bbox.max.x - b.max.x, 0.0),
            AlignEdge::Top => Vec2::new(0.0, bbox.min.y - b.min.y),
            AlignEdge::VCenter => Vec2::new(0.0, bbox.center().y - b.center().y),
            AlignEdge::Bottom => Vec2::new(0.0, bbox.max.y - b.max.y),
        };
        layer.frame.pos += delta;
    }
}

/// The bounds of `id`'s nearest ancestor `Artboard` (the Option-held
/// "align to frame instead of parent group" override), converted into `id`'s
/// own parent-relative coordinate space — i.e. suitable to pass straight as
/// `align`'s `align_to`. `None` if `id` isn't nested inside any Artboard, or
/// doesn't exist.
pub fn artboard_bounds_in_parent_space(page: &Page, id: LayerId) -> Option<Rect> {
    let artboard_id = nearest_ancestor_artboard(&page.layers, id, None)??;
    let artboard = page.find(artboard_id)?;
    let artboard_abs = artboard.frame.bounds().translate(page.absolute_offset(artboard_id)?);
    // `absolute_offset(id)` is the accumulated position of `id`'s ancestors
    // — exactly the origin `id`'s own (and its siblings') `frame.pos` is
    // relative to.
    let parent_offset = page.absolute_offset(id)?;
    Some(artboard_abs.translate(-parent_offset))
}

/// `Some(Some(artboard_id))` once `id` is located anywhere in the tree
/// (`None` inside if it has no Artboard ancestor); top-level `None` if `id`
/// isn't found at all. `current` is the nearest Artboard ancestor seen so
/// far while descending.
fn nearest_ancestor_artboard(layers: &[Layer], id: LayerId, current: Option<LayerId>) -> Option<Option<LayerId>> {
    for layer in layers {
        if layer.id == id {
            return Some(current);
        }
        if let Some(children) = layer.kind.children() {
            let next = if matches!(layer.kind, LayerKind::Artboard { .. }) {
                Some(layer.id)
            } else {
                current
            };
            if let Some(found) = nearest_ancestor_artboard(children, id, next) {
                return Some(found);
            }
        }
    }
    None
}

/// Evenly spaces the layers in `ids` along `axis`, keeping the frontmost and
/// backmost (by position) fixed and equalizing the edge-to-edge gap between
/// the rest, like a "distribute spacing" operation. Requires at least three
/// layers sharing a common parent; no-op otherwise.
pub fn distribute(page: &mut Page, ids: &[LayerId], axis: DistributeAxis) {
    if ids.len() < 3 {
        return;
    }
    let Some(siblings) = find_common_parent_list(&mut page.layers, ids) else {
        return;
    };

    let mut idxs: Vec<usize> = siblings
        .iter()
        .enumerate()
        .filter(|(_, l)| ids.contains(&l.id))
        .map(|(i, _)| i)
        .collect();
    if idxs.len() < 3 {
        return;
    }

    // `rotated_bounds()` throughout (not `bounds()`) so distribution is by
    // each layer's actual visual footprint — safe to mix with `frame.pos`
    // translation below since shifting `frame.pos` by a delta shifts
    // `rotated_bounds()` by that exact same delta (translating a shape
    // doesn't change its rotation, so its rotated AABB moves rigidly).
    let sort_key = |l: &crate::model::Layer| -> f32 {
        let b = l.frame.rotated_bounds();
        match axis {
            DistributeAxis::Horizontal => b.min.x,
            DistributeAxis::Vertical => b.min.y,
        }
    };
    idxs.sort_by(|&a, &b| sort_key(&siblings[a]).partial_cmp(&sort_key(&siblings[b])).unwrap());

    let first_bounds = siblings[idxs[0]].frame.rotated_bounds();
    let last_bounds = siblings[*idxs.last().unwrap()].frame.rotated_bounds();
    let n = idxs.len();

    match axis {
        DistributeAxis::Horizontal => {
            let span = last_bounds.max.x - first_bounds.min.x;
            let sum_widths: f32 = idxs.iter().map(|&i| siblings[i].frame.rotated_bounds().width()).sum();
            let gap = (span - sum_widths) / (n as f32 - 1.0);
            let mut cursor = first_bounds.max.x + gap;
            for &i in &idxs[1..n - 1] {
                let b = siblings[i].frame.rotated_bounds();
                siblings[i].frame.pos.x += cursor - b.min.x;
                cursor += b.width() + gap;
            }
        }
        DistributeAxis::Vertical => {
            let span = last_bounds.max.y - first_bounds.min.y;
            let sum_heights: f32 = idxs.iter().map(|&i| siblings[i].frame.rotated_bounds().height()).sum();
            let gap = (span - sum_heights) / (n as f32 - 1.0);
            let mut cursor = first_bounds.max.y + gap;
            for &i in &idxs[1..n - 1] {
                let b = siblings[i].frame.rotated_bounds();
                siblings[i].frame.pos.y += cursor - b.min.y;
                cursor += b.height() + gap;
            }
        }
    }
}

/// Arranges the layers in `ids` into an even reading-order (top-to-bottom,
/// then left-to-right) grid — a "Tidy" operation — with `spacing` between both
/// rows and columns. Column count defaults to `ceil(sqrt(n))`, the common
/// default, and each column/row is sized to its widest/tallest member so
/// mixed-size layers still line up cleanly. No-op with fewer than two ids or
/// no common parent.
pub fn tidy(page: &mut Page, ids: &[LayerId], spacing: f32) {
    if ids.len() < 2 {
        return;
    }
    let Some(siblings) = find_common_parent_list(&mut page.layers, ids) else {
        return;
    };
    let mut idxs: Vec<usize> = siblings.iter().enumerate().filter(|(_, l)| ids.contains(&l.id)).map(|(i, _)| i).collect();
    if idxs.len() < 2 {
        return;
    }
    idxs.sort_by(|&a, &b| {
        let ba = siblings[a].frame.rotated_bounds();
        let bb = siblings[b].frame.rotated_bounds();
        ba.min.y.partial_cmp(&bb.min.y).unwrap().then(ba.min.x.partial_cmp(&bb.min.x).unwrap())
    });

    let n = idxs.len();
    let cols = (n as f32).sqrt().ceil().max(1.0) as usize;
    let rows = n.div_ceil(cols);
    let origin = idxs
        .iter()
        .map(|&i| siblings[i].frame.rotated_bounds().min)
        .reduce(|a, b| Pos2::new(a.x.min(b.x), a.y.min(b.y)))
        .unwrap();

    let mut col_widths = vec![0.0f32; cols];
    let mut row_heights = vec![0.0f32; rows];
    for (k, &i) in idxs.iter().enumerate() {
        let b = siblings[i].frame.rotated_bounds();
        col_widths[k % cols] = col_widths[k % cols].max(b.width());
        row_heights[k / cols] = row_heights[k / cols].max(b.height());
    }
    let mut col_x = vec![0.0f32; cols];
    let mut cursor = origin.x;
    for (c, x) in col_x.iter_mut().enumerate() {
        *x = cursor;
        cursor += col_widths[c] + spacing;
    }
    let mut row_y = vec![0.0f32; rows];
    let mut cursor = origin.y;
    for (r, y) in row_y.iter_mut().enumerate() {
        *y = cursor;
        cursor += row_heights[r] + spacing;
    }

    for (k, &i) in idxs.iter().enumerate() {
        let b = siblings[i].frame.rotated_bounds();
        let target = Pos2::new(col_x[k % cols], row_y[k / cols]);
        siblings[i].frame.pos += target - b.min;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{CornerRadii, Frame, Layer, LayerKind, Page};
    use egui::Pos2;

    fn rect_layer(name: &str, x: f32, y: f32, w: f32, h: f32) -> Layer {
        Layer::new(
            name,
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
    fn align_left_moves_frames_to_bbox_min_x_preserving_size() {
        let (mut page, ids) = page_with(vec![
            rect_layer("A", 0.0, 0.0, 10.0, 10.0),
            rect_layer("B", 50.0, 20.0, 30.0, 5.0),
        ]);
        align(&mut page, &ids, AlignEdge::Left, None);
        assert_eq!(page.layers[0].frame.pos, Pos2::new(0.0, 0.0));
        assert_eq!(page.layers[1].frame.pos, Pos2::new(0.0, 20.0));
        assert_eq!(page.layers[1].frame.size, egui::Vec2::new(30.0, 5.0));
    }

    #[test]
    fn align_hcenter_centers_on_bbox_center() {
        let (mut page, ids) = page_with(vec![
            rect_layer("A", 0.0, 0.0, 10.0, 10.0),
            rect_layer("B", 40.0, 0.0, 10.0, 10.0),
        ]);
        align(&mut page, &ids, AlignEdge::HCenter, None);
        // bbox spans x 0..50, center x = 25; both 10-wide layers center on 25.
        assert_eq!(page.layers[0].frame.bounds().center().x, 25.0);
        assert_eq!(page.layers[1].frame.bounds().center().x, 25.0);
    }

    #[test]
    fn align_preserves_line_direction() {
        let mut line = Layer::new(
            "Line",
            Frame::from_two_points(Pos2::new(20.0, 20.0), Pos2::new(0.0, 0.0)),
            LayerKind::Line,
        );
        line.frame.pos = Pos2::new(20.0, 20.0);
        line.frame.size = egui::Vec2::new(-20.0, -20.0);
        let rect = rect_layer("R", 0.0, 0.0, 10.0, 10.0);
        let (mut page, ids) = page_with(vec![line, rect]);

        align(&mut page, &ids, AlignEdge::Top, None);

        // Direction (negative size) must survive the translate.
        assert_eq!(page.layers[0].frame.size, egui::Vec2::new(-20.0, -20.0));
    }

    #[test]
    fn align_rotated_shape_uses_its_visual_footprint_not_local_bounds() {
        // A 20x20 square rotated 45 degrees at (0,0) has a rotated visual
        // left edge at x ~= -4.14 (center 10,10 minus half-diagonal
        // 14.14), quite different from its unrotated local left edge at
        // x=0. Aligning "Left" against a second, unrotated rect should
        // move both so their *visual* left edges match.
        let mut rotated = rect_layer("Rotated", 0.0, 0.0, 20.0, 20.0);
        rotated.frame.rotation = 45.0;
        let plain = rect_layer("Plain", 100.0, 0.0, 20.0, 20.0);
        let (mut page, ids) = page_with(vec![rotated, plain]);

        align(&mut page, &ids, AlignEdge::Left, None);

        let bbox_min_x = page.layers.iter().map(|l| l.frame.rotated_bounds().min.x).fold(f32::INFINITY, f32::min);
        for layer in &page.layers {
            assert!(
                (layer.frame.rotated_bounds().min.x - bbox_min_x).abs() < 1e-3,
                "layer {} visual left edge {} should match bbox min {bbox_min_x}",
                layer.name,
                layer.frame.rotated_bounds().min.x
            );
        }
    }

    #[test]
    fn align_with_fewer_than_two_is_noop() {
        let (mut page, ids) = page_with(vec![rect_layer("A", 5.0, 5.0, 10.0, 10.0)]);
        align(&mut page, &ids, AlignEdge::Left, None);
        assert_eq!(page.layers[0].frame.pos, Pos2::new(5.0, 5.0));
    }

    #[test]
    fn distribute_horizontal_equalizes_gaps_and_fixes_endpoints() {
        let (mut page, ids) = page_with(vec![
            rect_layer("A", 0.0, 0.0, 10.0, 10.0),
            rect_layer("B", 15.0, 0.0, 10.0, 10.0),
            rect_layer("C", 90.0, 0.0, 10.0, 10.0),
        ]);
        distribute(&mut page, &ids, DistributeAxis::Horizontal);

        // Endpoints stay fixed.
        assert_eq!(page.layers[0].frame.bounds().min.x, 0.0);
        assert_eq!(page.layers[2].frame.bounds().min.x, 90.0);
        // Span 0..100, three 10-wide items, remaining 70 split into 2 gaps of 35.
        assert_eq!(page.layers[1].frame.bounds().min.x, 45.0);
    }

    #[test]
    fn distribute_with_fewer_than_three_is_noop() {
        let (mut page, ids) = page_with(vec![
            rect_layer("A", 0.0, 0.0, 10.0, 10.0),
            rect_layer("B", 15.0, 0.0, 10.0, 10.0),
        ]);
        distribute(&mut page, &ids, DistributeAxis::Horizontal);
        assert_eq!(page.layers[1].frame.pos, Pos2::new(15.0, 0.0));
    }

    #[test]
    fn tidy_arranges_four_layers_into_a_2x2_grid_with_spacing() {
        // 4 items -> ceil(sqrt(4)) = 2 columns, 2 rows. Scattered start
        // positions/order to confirm reading-order (top-to-bottom,
        // left-to-right) sorting, not original array order, drives layout.
        let (mut page, ids) = page_with(vec![
            rect_layer("D", 500.0, 500.0, 10.0, 10.0),
            rect_layer("B", 20.0, 0.0, 10.0, 10.0),
            rect_layer("C", 0.0, 20.0, 10.0, 10.0),
            rect_layer("A", 0.0, 0.0, 10.0, 10.0),
        ]);
        tidy(&mut page, &ids, 5.0);

        let by_name = |name: &str| page.layers.iter().find(|l| l.name == name).unwrap().frame.pos;
        assert_eq!(by_name("A"), Pos2::new(0.0, 0.0));
        assert_eq!(by_name("B"), Pos2::new(15.0, 0.0));
        assert_eq!(by_name("C"), Pos2::new(0.0, 15.0));
        assert_eq!(by_name("D"), Pos2::new(15.0, 15.0));
    }

    #[test]
    fn artboard_bounds_in_parent_space_converts_to_the_childs_local_coordinates() {
        let child = rect_layer("Child", 1.0, 1.0, 2.0, 2.0);
        let child_id = child.id;
        let artboard = Layer::new_artboard(
            "Board",
            Frame { pos: Pos2::new(100.0, 200.0), size: Vec2::new(300.0, 300.0), rotation: 0.0 },
        );
        let mut artboard = artboard;
        let LayerKind::Artboard { children, .. } = &mut artboard.kind else { unreachable!() };
        children.push(child);
        let mut page = Page::new("Page 1");
        page.layers.push(artboard);

        // Artboard's own bounds (0,0)-(300,300) in its local space, i.e.
        // exactly its own frame's unrotated bounds — no translation since
        // it's top-level.
        let bounds = artboard_bounds_in_parent_space(&page, child_id).expect("child is inside an artboard");
        assert_eq!(bounds, Rect::from_min_size(Pos2::ZERO, Vec2::new(300.0, 300.0)));
    }

    #[test]
    fn artboard_bounds_in_parent_space_is_none_without_an_artboard_ancestor() {
        let (page, ids) = page_with(vec![rect_layer("A", 0.0, 0.0, 10.0, 10.0)]);
        assert_eq!(artboard_bounds_in_parent_space(&page, ids[0]), None);
    }
}
