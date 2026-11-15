//! Which layer(s) sit under a document-space point or inside a marquee,
//! plus the ordering behind Smart Distribute's gap handles.

use std::collections::HashMap;

use egui::{Pos2, Rect, Vec2};

use crate::alignment::DistributeAxis;
use crate::model::{Layer, LayerId, LayerKind, Page};
use crate::shapes::rotate_point;

/// The layers in `ids` that still exist, sorted along `axis` by absolute
/// bounds — the same ordering `alignment::distribute` itself sorts by, so
/// the gap handles line up with the actual gaps it produced.
pub(super) fn distribution_order(page: &Page, ids: &[LayerId], axis: DistributeAxis) -> Vec<(LayerId, Rect)> {
    let mut items: Vec<(LayerId, Rect)> = ids
        .iter()
        .filter_map(|&id| {
            let layer = page.find(id)?;
            let offset = page.absolute_offset(id)?;
            Some((id, layer.frame.rotated_bounds().translate(offset)))
        })
        .collect();
    items.sort_by(|a, b| {
        let key = |r: &Rect| match axis {
            DistributeAxis::Horizontal => r.min.x,
            DistributeAxis::Vertical => r.min.y,
        };
        key(&a.1).partial_cmp(&key(&b.1)).unwrap()
    });
    items
}

/// Doc-space midpoint of the gap between each consecutive pair in
/// `distribution_order`'s output — one fewer than the item count.
pub(super) fn gap_handle_positions(items: &[(LayerId, Rect)], axis: DistributeAxis) -> Vec<Pos2> {
    items
        .windows(2)
        .map(|w| {
            let ra = w[0].1;
            let rb = w[1].1;
            match axis {
                DistributeAxis::Horizontal => Pos2::new((ra.max.x + rb.min.x) / 2.0, (ra.center().y + rb.center().y) / 2.0),
                DistributeAxis::Vertical => Pos2::new((ra.center().x + rb.center().x) / 2.0, (ra.max.y + rb.min.y) / 2.0),
            }
        })
        .collect()
}

pub(super) fn hit_test(page: &Page, doc_pos: Pos2) -> Option<LayerId> {
    hit_test_layers(&page.layers, doc_pos)
}

/// The topmost layer that mask `mask_id` actually reveals at `doc_pos`
/// (page space), or `None` if the mask reveals nothing there. A plain click
/// on a mask selects the mask itself (it sits on top of what it clips, see
/// `hit_test_layers`); this is the double-click "enter the masked content"
/// path. Searches every masked run `mask_id` heads in its parent, front to
/// back, with the same per-layer test `hit_test_layers` uses.
pub(super) fn hit_test_through_mask(page: &Page, mask_id: LayerId, doc_pos: Pos2) -> Option<LayerId> {
    let mask = page.find(mask_id)?;
    let siblings = match crate::grouping::parent_and_index(&page.layers, mask_id)? {
        (Some(parent_id), _) => page.find(parent_id)?.kind.children()?,
        (None, _) => page.layers.as_slice(),
    };
    let local_pos = doc_pos - page.absolute_offset(mask_id)?;
    if !crate::masking::mask_reveals_point(mask, local_pos) {
        return None;
    }
    let mut content: Vec<&Layer> = Vec::new();
    for unit in crate::masking::partition_mask_runs(siblings) {
        if let crate::masking::RenderUnit::Masked { mask, content: run, .. } = unit
            && mask.id == mask_id {
                content.extend(run);
            }
    }
    content.iter().rev().find_map(|layer| hit_test_layers(std::slice::from_ref(*layer), local_pos))
}

/// Every layer under `doc_pos`, front-to-back, for the Shift+right-click
/// "pick among overlapping layers" menu. Bounding-box based (via
/// `rotated_bounds()`) rather than `hit_test_layers`'s pixel-accurate/
/// mask-aware test — good enough for a picker menu, and much simpler than
/// duplicating that function's boolean-group/mask logic.
pub(super) fn layers_at_point(layers: &[Layer], offset: Vec2, doc_pos: Pos2, out: &mut Vec<LayerId>) {
    for layer in layers.iter().rev() {
        if !layer.visible || layer.locked {
            continue;
        }
        if layer.frame.rotated_bounds().translate(offset).contains(doc_pos) {
            out.push(layer.id);
        }
        if let Some(children) = layer.kind.children() {
            layers_at_point(children, offset + layer.frame.pos.to_vec2(), doc_pos, out);
        }
    }
}

/// Collects every layer overlapping (or, if `contained_only`, fully inside)
/// `marquee` (in page/doc space) into `out`. With `ignore_groups` false
/// (the default, top-level-only rubber-band select), only `layers` itself is
/// tested — a `Group`/`Artboard`/`BooleanGroup` is matched as one unit, same
/// as today. With `ignore_groups` true (Command held), containers are never
/// matched themselves — only their descendant leaves are, recursively, per
/// "ignore groups and select only contained layers". `offset` is the
/// accumulated ancestor position (see the module doc's coordinate-system
/// convention); `rotated_bounds()` is used throughout so a rotated leaf's
/// actual visual footprint is what's tested, not its unrotated local bounds.
pub(super) fn collect_marquee_hits(
    layers: &[Layer],
    offset: Vec2,
    marquee: Rect,
    contained_only: bool,
    ignore_groups: bool,
    out: &mut Vec<LayerId>,
) {
    for layer in layers {
        if !layer.visible || layer.locked {
            continue;
        }
        let has_children = layer.kind.children().is_some();
        if ignore_groups && has_children {
            let child_offset = offset + layer.frame.pos.to_vec2();
            collect_marquee_hits(
                layer.kind.children().unwrap(),
                child_offset,
                marquee,
                contained_only,
                ignore_groups,
                out,
            );
            continue;
        }
        let abs_bounds = layer.frame.rotated_bounds().translate(offset);
        let matches = if contained_only {
            marquee.contains_rect(abs_bounds)
        } else {
            marquee.intersects(abs_bounds)
        };
        if matches {
            out.push(layer.id);
        }
    }
}

pub(super) fn hit_test_layers(layers: &[Layer], doc_pos: Pos2) -> Option<LayerId> {
    // Alpha-accurate: a content layer under an active mask (see
    // `masking::partition_mask_runs`) only registers a hit where the mask
    // actually reveals it, not just anywhere in the content layer's own
    // bounding box — e.g. a layer that extends beyond its mask's bounds is
    // no longer clickable in the clipped-away part. The mask layer *itself*
    // keeps the normal plain bounding-box test (unchanged) — it's still a
    // real, selectable layer sitting on top like any other, this only
    // changes what's reachable *through* it (double-click reaches the
    // content itself, via `hit_test_through_mask`). Built fresh per call (only
    // ever invoked on an actual click, not per frame) rather than threaded
    // through from the caller, so this stays a self-contained,
    // easy-to-reason-about function like it was before masking existed.
    let mut mask_for: HashMap<LayerId, &Layer> = HashMap::new();
    for unit in crate::masking::partition_mask_runs(layers) {
        if let crate::masking::RenderUnit::Masked { mask, content, .. } = unit {
            for layer in content {
                mask_for.insert(layer.id, mask);
            }
        }
    }

    for layer in layers.iter().rev() {
        if !layer.visible || layer.locked {
            continue;
        }

        // Opaque, unlike Group/Artboard just below (which fall through to
        // the generic bounds/children handling): a click selects the
        // BooleanGroup itself as one unit — members are only reachable via
        // the layers panel (see `LayerKind::BooleanGroup`'s doc comment).
        // Tested against the live-computed geometry exactly (`geo::Contains`)
        // rather than a rasterize-and-sample approach, since the combined
        // shape is already a clean `MultiPolygon` (contrast with
        // `masking::mask_reveals_point`, whose rasterize approach exists
        // specifically because a mask's coverage is arbitrary alpha, not a
        // clean polygon). No rotation handling needed here — a
        // `BooleanGroup`'s own `frame.rotation` is always `0.0`, same
        // convention as `Group`/`Artboard`.
        if let LayerKind::BooleanGroup { children } = &layer.kind {
            let local_pos = doc_pos - layer.frame.pos.to_vec2();
            let combined = crate::boolean_ops::compute_boolean_group(children);
            if !crate::boolean_ops::point_in_multipolygon(&combined, local_pos) {
                continue;
            }
            if let Some(mask) = mask_for.get(&layer.id)
                && !crate::masking::mask_reveals_point(mask, doc_pos) {
                    continue;
                }
            return Some(layer.id);
        }

        let bounds = layer.frame.bounds();
        // Only leaf shapes ever have nonzero rotation (a Group/Artboard's
        // own rotation is always 0.0 — rotating one bakes the angle into
        // each descendant's own frame instead, see `model/layer.rs`'s
        // `Frame::rotation` doc comment), so inverse-rotating `doc_pos` into
        // the layer's own unrotated local space here is enough — no matching
        // change is needed for the `children()` recursion below.
        let test_pos = rotate_point(doc_pos, bounds.center(), -layer.frame.rotation);
        if !bounds.contains(test_pos) {
            continue;
        }
        if let Some(mask) = mask_for.get(&layer.id)
            && !crate::masking::mask_reveals_point(mask, doc_pos) {
                continue;
            }
        if let Some(children) = layer.kind.children() {
            let local_pos = doc_pos - layer.frame.pos.to_vec2();
            if let Some(hit) = hit_test_layers(children, local_pos) {
                return Some(hit);
            }
        }
        return Some(layer.id);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{CornerRadii, Document, Frame, Paint};
    use egui::Color32;

    fn rect_layer(name: &str, x: f32, y: f32, w: f32, h: f32) -> Layer {
        Layer::new(
            name,
            Frame { pos: Pos2::new(x, y), size: Vec2::new(w, h), rotation: 0.0 },
            LayerKind::Rectangle { corner_radius: CornerRadii::ZERO },
        )
    }

    fn bg_rect(name: &str, pos: Pos2, size: Vec2) -> Layer {
        Layer::new(name, Frame { pos, size, rotation: 0.0 }, LayerKind::Rectangle { corner_radius: CornerRadii::ZERO })
    }

    #[test]
    fn hit_test_respects_rotation() {
        let layer = Layer::new(
            "Rect",
            Frame { pos: Pos2::new(0.0, 0.0), size: Vec2::new(20.0, 20.0), rotation: 45.0 },
            LayerKind::Rectangle { corner_radius: CornerRadii::ZERO },
        );
        let mut doc = Document::new();
        doc.active_page_mut().layers.push(layer);
        let page = doc.active_page();

        // The center is unaffected by rotation (it's the pivot).
        assert!(hit_test(page, Pos2::new(10.0, 10.0)).is_some());
        // The unrotated frame's own corner (0,0) is now outside the
        // rotated (45 degree) square's actual diagonal-edged footprint.
        assert!(hit_test(page, Pos2::new(1.0, 1.0)).is_none());
        // A point along the rotated square's actual edge, straight up from
        // the center by half its diagonal, should hit.
        let half_diagonal = 10.0 * std::f32::consts::SQRT_2 * 0.5;
        assert!(hit_test(page, Pos2::new(10.0, 10.0 - half_diagonal + 0.5)).is_some());
    }

    #[test]
    fn hit_test_skips_masked_out_region_of_content_layer() {
        // A content layer larger than its mask: the mask (on top, still
        // hit-tested by its own plain bounding box like any other layer —
        // only what it *masks* gets the alpha-accurate treatment) sits at
        // (0,0)-(100,100); the square it masks is much bigger, spanning
        // (-50,-50)-(150,150).
        let mut square = Layer::new(
            "Square",
            Frame { pos: Pos2::new(-50.0, -50.0), size: Vec2::new(200.0, 200.0), rotation: 0.0 },
            LayerKind::Rectangle { corner_radius: CornerRadii::ZERO },
        );
        square.style.fill = Some(Paint::Solid(Color32::RED));
        let mut mask = Layer::new(
            "Mask",
            Frame { pos: Pos2::new(0.0, 0.0), size: Vec2::new(100.0, 100.0), rotation: 0.0 },
            LayerKind::Oval,
        );
        mask.is_mask = true;
        let mask_id = mask.id;

        let mut doc = Document::new();
        doc.active_page_mut().layers.push(square);
        doc.active_page_mut().layers.push(mask);
        let page = doc.active_page();

        // Center: within the mask's own bounding box, so the mask (topmost)
        // claims the hit itself, same as any other layer stacked on top.
        assert_eq!(hit_test(page, Pos2::new(50.0, 50.0)), Some(mask_id));
        // Well outside the mask's bounding box entirely, but still deep
        // inside the square's own (much bigger) bounding box: the old
        // bbox-only test would have hit the square here even though the
        // mask clips it away completely at this point — now it correctly
        // finds nothing.
        assert_eq!(hit_test(page, Pos2::new(-20.0, -20.0)), None);
    }

    #[test]
    fn hit_test_through_mask_finds_revealed_content_inside_a_group() {
        let back = rect_layer("Back", 0.0, 0.0, 100.0, 100.0);
        let back_id = back.id;
        let front = rect_layer("Front", 50.0, 0.0, 50.0, 100.0);
        let front_id = front.id;
        let mut mask = Layer::new(
            "Mask",
            Frame { pos: Pos2::new(0.0, 0.0), size: Vec2::new(100.0, 100.0), rotation: 0.0 },
            LayerKind::Oval,
        );
        mask.is_mask = true;
        let mask_id = mask.id;
        let group = Layer::new(
            "Group",
            Frame { pos: Pos2::new(200.0, 200.0), size: Vec2::new(100.0, 100.0), rotation: 0.0 },
            LayerKind::Group { children: vec![back, front, mask] },
        );

        let mut doc = Document::new();
        doc.active_page_mut().layers.push(group);
        let page = doc.active_page();

        // A single click still lands on the mask (topmost in the group).
        assert_eq!(hit_test(page, Pos2::new(220.0, 250.0)), Some(mask_id));
        // Through the mask: left half reveals only Back, right half Front (on top).
        assert_eq!(hit_test_through_mask(page, mask_id, Pos2::new(220.0, 250.0)), Some(back_id));
        assert_eq!(hit_test_through_mask(page, mask_id, Pos2::new(280.0, 250.0)), Some(front_id));
        // Inside the mask's bbox but outside the oval: nothing is revealed.
        assert_eq!(hit_test_through_mask(page, mask_id, Pos2::new(202.0, 202.0)), None);
    }

    #[test]
    fn hit_test_a_boolean_group_selects_the_group_not_a_child() {
        let a = bg_rect("A", Pos2::new(0.0, 0.0), Vec2::new(40.0, 40.0));
        let mut b = bg_rect("B", Pos2::new(20.0, 20.0), Vec2::new(40.0, 40.0));
        b.bool_op = crate::model::BoolOp::Union;
        let group = Layer::new(
            "Boolean Group",
            Frame { pos: Pos2::new(0.0, 0.0), size: Vec2::new(60.0, 60.0), rotation: 0.0 },
            LayerKind::BooleanGroup { children: vec![a, b] },
        );
        let group_id = group.id;

        let mut doc = Document::new();
        doc.active_page_mut().layers.push(group);
        let page = doc.active_page();

        // Inside the overlap of the two unioned rects — a click here should
        // select the BooleanGroup itself, not descend into a child (unlike
        // Group/Artboard).
        assert_eq!(hit_test(page, Pos2::new(25.0, 25.0)), Some(group_id));
        // Inside A only.
        assert_eq!(hit_test(page, Pos2::new(5.0, 5.0)), Some(group_id));
    }

    #[test]
    fn hit_test_a_boolean_group_is_exact_not_just_bounding_box() {
        let base = bg_rect("Base", Pos2::new(0.0, 0.0), Vec2::new(60.0, 60.0));
        let mut hole = bg_rect("Hole", Pos2::new(20.0, 20.0), Vec2::new(20.0, 20.0));
        hole.bool_op = crate::model::BoolOp::Subtract;
        let group = Layer::new(
            "Boolean Group",
            Frame { pos: Pos2::new(0.0, 0.0), size: Vec2::new(60.0, 60.0), rotation: 0.0 },
            LayerKind::BooleanGroup { children: vec![base, hole] },
        );

        let mut doc = Document::new();
        doc.active_page_mut().layers.push(group);
        let page = doc.active_page();

        // Inside the punched-out hole but well within the group's bounding
        // box: a bbox-only test would hit here; the exact silhouette test
        // must not.
        assert_eq!(hit_test(page, Pos2::new(30.0, 30.0)), None);
    }

    #[test]
    fn hit_test_a_masked_boolean_group_still_respects_the_mask() {
        let a = bg_rect("A", Pos2::new(0.0, 0.0), Vec2::new(40.0, 40.0));
        let mut b = bg_rect("B", Pos2::new(20.0, 20.0), Vec2::new(40.0, 40.0));
        b.bool_op = crate::model::BoolOp::Union;
        let group = Layer::new(
            "Boolean Group",
            Frame { pos: Pos2::new(0.0, 0.0), size: Vec2::new(60.0, 60.0), rotation: 0.0 },
            LayerKind::BooleanGroup { children: vec![a, b] },
        );

        // A mask covering only the group's left half, sitting above it (so
        // it's the mask for the group's own render/hit-test run).
        let mut mask = Layer::new(
            "Mask",
            Frame { pos: Pos2::new(0.0, 0.0), size: Vec2::new(30.0, 60.0), rotation: 0.0 },
            LayerKind::Rectangle { corner_radius: CornerRadii::ZERO },
        );
        mask.is_mask = true;

        let mut doc = Document::new();
        doc.active_page_mut().layers.push(group);
        doc.active_page_mut().layers.push(mask);
        let page = doc.active_page();

        // Inside the group's own silhouette (the A/B overlap) but outside
        // the mask's coverage (x > 30) — should be clipped away entirely.
        assert_eq!(hit_test(page, Pos2::new(55.0, 55.0)), None);
    }

    #[test]
    fn collect_marquee_hits_default_is_top_level_intersecting() {
        let a = rect_layer("A", 0.0, 0.0, 10.0, 10.0);
        let b = rect_layer("B", 100.0, 100.0, 10.0, 10.0);
        let layers = vec![a, b];
        let marquee = Rect::from_min_size(Pos2::new(-5.0, -5.0), Vec2::new(20.0, 20.0));
        let mut hits = Vec::new();
        collect_marquee_hits(&layers, Vec2::ZERO, marquee, false, false, &mut hits);
        assert_eq!(hits, vec![layers[0].id]);
    }

    #[test]
    fn collect_marquee_hits_contained_only_excludes_partial_overlap() {
        let a = rect_layer("A", 0.0, 0.0, 10.0, 10.0);
        let layers = vec![a];
        // Marquee only partially covers the layer.
        let marquee = Rect::from_min_size(Pos2::new(-5.0, -5.0), Vec2::new(10.0, 10.0));
        let mut hits = Vec::new();
        collect_marquee_hits(&layers, Vec2::ZERO, marquee, true, false, &mut hits);
        assert!(hits.is_empty(), "partially-overlapped layer should not match contained_only");

        let full_marquee = Rect::from_min_size(Pos2::new(-5.0, -5.0), Vec2::new(20.0, 20.0));
        let mut hits2 = Vec::new();
        collect_marquee_hits(&layers, Vec2::ZERO, full_marquee, true, false, &mut hits2);
        assert_eq!(hits2, vec![layers[0].id]);
    }

    #[test]
    fn collect_marquee_hits_ignore_groups_descends_into_children_only() {
        let child = rect_layer("Child", 2.0, 2.0, 4.0, 4.0);
        let child_id = child.id;
        let group = Layer::new(
            "Group",
            Frame { pos: Pos2::new(0.0, 0.0), size: Vec2::new(10.0, 10.0), rotation: 0.0 },
            LayerKind::Group { children: vec![child] },
        );
        let group_id = group.id;
        let layers = vec![group];
        let marquee = Rect::from_min_size(Pos2::ZERO, Vec2::new(20.0, 20.0));

        let mut default_hits = Vec::new();
        collect_marquee_hits(&layers, Vec2::ZERO, marquee, false, false, &mut default_hits);
        assert_eq!(default_hits, vec![group_id], "without ignore_groups, the group matches as one unit");

        let mut ignoring_hits = Vec::new();
        collect_marquee_hits(&layers, Vec2::ZERO, marquee, false, true, &mut ignoring_hits);
        assert_eq!(ignoring_hits, vec![child_id], "with ignore_groups, only the leaf child matches");
    }

    #[test]
    fn layers_at_point_returns_every_stacked_layer_front_to_back() {
        let bottom = rect_layer("Bottom", 0.0, 0.0, 10.0, 10.0);
        let bottom_id = bottom.id;
        let top = rect_layer("Top", 0.0, 0.0, 10.0, 10.0);
        let top_id = top.id;
        let layers = vec![bottom, top];
        let mut hits = Vec::new();
        layers_at_point(&layers, Vec2::ZERO, Pos2::new(5.0, 5.0), &mut hits);
        assert_eq!(hits, vec![top_id, bottom_id]);
    }

    #[test]
    fn distribution_order_sorts_by_position_along_the_axis() {
        let mut page = Page::new("Page 1");
        page.layers.push(rect_layer("C", 90.0, 0.0, 10.0, 10.0));
        page.layers.push(rect_layer("A", 0.0, 0.0, 10.0, 10.0));
        page.layers.push(rect_layer("B", 15.0, 0.0, 10.0, 10.0));
        let ids: Vec<LayerId> = page.layers.iter().map(|l| l.id).collect();

        let order = distribution_order(&page, &ids, DistributeAxis::Horizontal);
        let names: Vec<&str> = order.iter().map(|(id, _)| page.find(*id).unwrap().name.as_str()).collect();
        assert_eq!(names, vec!["A", "B", "C"]);
    }

    #[test]
    fn gap_handle_positions_returns_one_fewer_than_item_count() {
        let mut page = Page::new("Page 1");
        page.layers.push(rect_layer("A", 0.0, 0.0, 10.0, 10.0));
        page.layers.push(rect_layer("B", 15.0, 0.0, 10.0, 10.0));
        page.layers.push(rect_layer("C", 90.0, 0.0, 10.0, 10.0));
        let ids: Vec<LayerId> = page.layers.iter().map(|l| l.id).collect();

        let order = distribution_order(&page, &ids, DistributeAxis::Horizontal);
        let handles = gap_handle_positions(&order, DistributeAxis::Horizontal);
        assert_eq!(handles.len(), 2);
        // First gap midpoint between A's right edge (10) and B's left edge (15).
        assert_eq!(handles[0].x, 12.5);
    }
}
