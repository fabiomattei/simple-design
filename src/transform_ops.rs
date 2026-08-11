//! Whole-layer geometric transforms that aren't tree surgery (see
//! `grouping.rs` for that) or a single drag gesture (see `canvas.rs`'s
//! `DragState`): flip, rotate copies, and flatten. Grouped in one module
//! since they share the same "read the current selection, mutate frame/
//! point geometry in place" shape.

use egui::{Pos2, Rect, Vec2};

use crate::model::{Layer, LayerId, LayerKind, Page, PathPoint, PathPolygon};
use crate::shapes::{ellipse_points, polygon_points, rotate_point, rounded_rect_points, star_points};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FlipAxis {
    Horizontal,
    Vertical,
}

/// Mirrors `layer` in place about its own frame's center, along `axis`.
/// Rectangle/Oval are visually symmetric so this only changes their stored
/// `frame.size` sign (still correct — matches how that sign already
/// preserves a `Line`'s drag direction); `Path`/`CompoundPath` additionally
/// mirror every point *in `frame.pos`-relative local space* (their own
/// coordinate convention — NOT `frame.bounds().center()`, which is in the
/// parent's space): the local mirror axis sits at half of the frame's own
/// (always non-negative, since a Path's frame is always derived from an
/// anchor-bounds union) `size`, computed before that size's sign is negated
/// below. `Image` mirrors the bitmap itself (destructive, same philosophy as
/// this codebase's other Image edits — see `image_ops::apply_cropped_image`).
/// `Text` is a frame-only no-op: genuinely mirrored glyph rendering would
/// need matching work in both `canvas.rs`'s egui layout and `export.rs`'s
/// `ab_glyph` rasterizer for very little payoff, so it's deliberately not
/// attempted.
pub fn flip_layer(layer: &mut Layer, axis: FlipAxis) {
    // Magnitude, not the signed `frame.size` — otherwise a second flip (size
    // now negative from the first) would mirror about the wrong axis and
    // break flip-twice-is-identity.
    let local_half = Vec2::new(layer.frame.size.x.abs(), layer.frame.size.y.abs()) * 0.5;
    match axis {
        FlipAxis::Horizontal => layer.frame.size.x = -layer.frame.size.x,
        FlipAxis::Vertical => layer.frame.size.y = -layer.frame.size.y,
    }
    match &mut layer.kind {
        LayerKind::Path { points, .. } => {
            for p in points.iter_mut() {
                flip_path_point(p, axis, local_half.x, local_half.y);
            }
        }
        LayerKind::CompoundPath { polygons } => {
            for poly in polygons.iter_mut() {
                flip_ring(&mut poly.exterior, axis, local_half.x, local_half.y);
                for hole in poly.holes.iter_mut() {
                    flip_ring(hole, axis, local_half.x, local_half.y);
                }
            }
        }
        LayerKind::Image { encoded, .. } => {
            if let Some(img) = crate::image_ops::decode(encoded) {
                let flipped = match axis {
                    FlipAxis::Horizontal => image::imageops::flip_horizontal(&img),
                    FlipAxis::Vertical => image::imageops::flip_vertical(&img),
                };
                *encoded = crate::image_ops::encode_png(&flipped);
            }
            if let LayerKind::Image { version, .. } = &mut layer.kind {
                *version = uuid::Uuid::new_v4();
            }
        }
        _ => {}
    }
}

/// `half_x`/`half_y` are half of the frame's own local (`frame.pos`-relative)
/// size — the mirror axis for anchors/points in that same local space.
fn flip_path_point(p: &mut PathPoint, axis: FlipAxis, half_x: f32, half_y: f32) {
    match axis {
        FlipAxis::Horizontal => {
            p.anchor.x = 2.0 * half_x - p.anchor.x;
            if let Some(h) = &mut p.handle_in {
                h.x = -h.x;
            }
            if let Some(h) = &mut p.handle_out {
                h.x = -h.x;
            }
        }
        FlipAxis::Vertical => {
            p.anchor.y = 2.0 * half_y - p.anchor.y;
            if let Some(h) = &mut p.handle_in {
                h.y = -h.y;
            }
            if let Some(h) = &mut p.handle_out {
                h.y = -h.y;
            }
        }
    }
}

fn flip_ring(ring: &mut [Pos2], axis: FlipAxis, half_x: f32, half_y: f32) {
    for p in ring.iter_mut() {
        match axis {
            FlipAxis::Horizontal => p.x = 2.0 * half_x - p.x,
            FlipAxis::Vertical => p.y = 2.0 * half_y - p.y,
        }
    }
}

/// Flips every layer in `ids` in place (each about its own frame center,
/// matching a per-layer flip when multiple layers are selected — not
/// a mirror about the selection's combined bounding box).
pub fn flip_selection(page: &mut Page, ids: &[LayerId], axis: FlipAxis) {
    for id in ids {
        if let Some(layer) = page.find_mut(*id) {
            flip_layer(layer, axis);
        }
    }
}

/// Duplicates every layer in `ids` `count` times, arranging the copies in an
/// evenly-spaced rotational fan around the original selection's own combined
/// `rotated_bounds()` center — a "Rotate Copies" operation. Each duplicate `i`
/// (`1..=count`) sits at `total_degrees * i / count`, so with the common
/// `total_degrees: 360.0` the *last* copy's angle coincides with the
/// original's own position (this mirrors how a plain `360/count` division
/// naturally closes the circle back on itself — pass e.g.
/// `360.0 * (count-1) as f32 / count as f32` instead if a copy exactly on
/// top of the original isn't wanted). The original layers themselves are
/// left untouched; only new duplicates are added. Reuses
/// `canvas::collect_rotatable_leaves`/`apply_rotation_delta` — the exact
/// same leaf-baking rotation machinery the interactive drag-to-rotate handle
/// uses — applied once per copy instead of continuously per drag frame.
/// Returns every new layer id created, grouped by copy (all of copy 1's new
/// ids, then copy 2's, ...).
pub fn rotate_copies(page: &mut Page, ids: &[LayerId], count: u32, total_degrees: f32) -> Vec<LayerId> {
    if count == 0 || ids.is_empty() {
        return Vec::new();
    }
    let pivot = ids
        .iter()
        .filter_map(|&id| {
            let layer = page.find(id)?;
            let offset = page.absolute_offset(id)?;
            Some(layer.frame.rotated_bounds().translate(offset))
        })
        .reduce(|a, b| a.union(b))
        .map(|r| r.center());
    let Some(pivot) = pivot else {
        return Vec::new();
    };

    let mut new_ids = Vec::new();
    for i in 1..=count {
        let angle = total_degrees * (i as f32) / (count as f32);
        let duplicated = crate::grouping::duplicate_layers(page, ids, crate::grouping::DEFAULT_DUPLICATE_OFFSET);
        let mut leaves = Vec::new();
        for &dup_id in &duplicated {
            if let (Some(layer), Some(offset)) = (page.find(dup_id), page.absolute_offset(dup_id)) {
                crate::canvas::collect_rotatable_leaves(layer, offset, &mut leaves);
            }
        }
        crate::canvas::apply_rotation_delta(page, pivot, angle, &leaves);
        // A duplicated container (Group/Artboard/BooleanGroup) among `ids`
        // needs the same frame refit a live rotate-drag does — see
        // `canvas::refit_container_to_children`'s doc comment — or its
        // selection outline/handles (and a `BooleanGroup`'s rendered
        // geometry) detach from its now-rotated children.
        for &dup_id in &duplicated {
            crate::canvas::refit_container_to_children(page, dup_id);
        }
        new_ids.extend(duplicated);
    }
    new_ids
}

/// "Layer > Combine > Flatten": bakes `frame.rotation` to `0.0` and
/// converts the shape to a single/multi-ring `CompoundPath`, reusing the
/// exact same point generators (`shapes::rounded_rect_points`/`ellipse_points`/
/// `star_points`/`polygon_points`, plus `canvas::flatten_path` for a closed
/// `Path`, all rotated the same way `boolean_ops.rs::flatten_layer` does)
/// that every other rotation-aware geometry consumer already shares — so
/// Flatten can't silently disagree with how the shape currently renders.
/// Keeps the original layer's `id`/`name`/`style`/`opacity`/`visible`/
/// `locked` (destructive — no longer editable as its original shape kind —
/// but the layer list entry doesn't change identity). `None` for a kind with
/// no fillable/flattenable geometry (`Line`, `Arrow`, `Text`, `Image`,
/// `Group`, `Artboard` — matching `boolean_ops.rs::flatten_layer`'s own
/// recognized-kinds set) or a `Path` that isn't closed.
///
/// Named to avoid colliding with the internal, differently-scoped
/// `boolean_ops::flatten_layer` (parent-local-space polygons *for a boolean
/// op*, not a standalone new layer).
pub fn flatten_to_compound_path(layer: &Layer) -> Option<Layer> {
    let bounds = layer.frame.bounds();
    let rotation = layer.frame.rotation;
    let rotate_pts = |pts: Vec<Pos2>| -> Vec<Pos2> {
        pts.into_iter().map(|p| rotate_point(p, bounds.center(), rotation)).collect()
    };

    let polygons: Vec<PathPolygon> = match &layer.kind {
        LayerKind::Rectangle { corner_radius } => {
            vec![PathPolygon { exterior: rotate_pts(rounded_rect_points(bounds, corner_radius.as_array())), holes: Vec::new() }]
        }
        LayerKind::Oval => {
            let pts = ellipse_points(bounds.center(), bounds.width() / 2.0, bounds.height() / 2.0);
            vec![PathPolygon { exterior: rotate_pts(pts), holes: Vec::new() }]
        }
        LayerKind::Star { points, inner_ratio } => {
            let pts = star_points(bounds.center(), bounds.width() / 2.0, bounds.height() / 2.0, *points, *inner_ratio);
            vec![PathPolygon { exterior: rotate_pts(pts), holes: Vec::new() }]
        }
        LayerKind::Polygon { sides } => {
            let pts = polygon_points(bounds.center(), bounds.width() / 2.0, bounds.height() / 2.0, *sides);
            vec![PathPolygon { exterior: rotate_pts(pts), holes: Vec::new() }]
        }
        LayerKind::Path { points, closed } => {
            if !*closed || points.len() < 3 {
                return None;
            }
            let offset = layer.frame.pos.to_vec2();
            let pts: Vec<Pos2> = crate::canvas::flatten_path(points, true).into_iter().map(|p| p + offset).collect();
            vec![PathPolygon { exterior: rotate_pts(pts), holes: Vec::new() }]
        }
        LayerKind::CompoundPath { polygons } => {
            let offset = layer.frame.pos.to_vec2();
            polygons
                .iter()
                .map(|p| PathPolygon {
                    exterior: rotate_pts(p.exterior.iter().map(|pt| *pt + offset).collect()),
                    holes: p.holes.iter().map(|h| rotate_pts(h.iter().map(|pt| *pt + offset).collect())).collect(),
                })
                .collect()
        }
        LayerKind::Artboard { .. }
        | LayerKind::Group { .. }
        | LayerKind::BooleanGroup { .. }
        | LayerKind::Line
        | LayerKind::Arrow { .. }
        | LayerKind::Text { .. }
        | LayerKind::Image { .. } => return None,
    };

    // Every point above is currently in *parent-local absolute* space (the
    // same space `bounds`/`layer.frame.pos` live in) — re-baseline to a
    // tight new frame the same way `boolean_ops.rs::compound_path_layer`
    // does, so the result follows the usual "frame is the tight bounding
    // box, points are relative to it" convention with `rotation` now `0.0`.
    let all_points: Vec<Pos2> =
        polygons.iter().flat_map(|p| p.exterior.iter().copied().chain(p.holes.iter().flatten().copied())).collect();
    if all_points.is_empty() {
        return None;
    }
    let new_bounds = Rect::from_points(&all_points);
    let frame_pos = new_bounds.min;
    let polygons: Vec<PathPolygon> = polygons
        .into_iter()
        .map(|p| PathPolygon {
            exterior: p.exterior.into_iter().map(|pt| pt - frame_pos.to_vec2()).collect(),
            holes: p.holes.into_iter().map(|h| h.into_iter().map(|pt| pt - frame_pos.to_vec2()).collect()).collect(),
        })
        .collect();

    let mut new_layer = layer.clone();
    new_layer.frame = crate::model::Frame { pos: frame_pos, size: new_bounds.size(), rotation: 0.0 };
    new_layer.kind = LayerKind::CompoundPath { polygons };
    Some(new_layer)
}

/// Flattens every layer in `ids` that `flatten_to_compound_path` supports, in
/// place (same id, same position in its parent's layer list). Layers of an
/// unsupported kind are left untouched.
pub fn flatten_selection(page: &mut Page, ids: &[LayerId]) {
    for &id in ids {
        let Some(layer) = page.find(id) else { continue };
        let Some(flattened) = flatten_to_compound_path(layer) else { continue };
        if let Some(slot) = page.find_mut(id) {
            *slot = flattened;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{CornerRadii, Frame, LayerKind as LK, Page, PointType};

    #[test]
    fn flatten_rotated_rectangle_produces_unrotated_compound_path_with_matching_footprint() {
        let layer = Layer::new(
            "Rect",
            Frame { pos: Pos2::new(0.0, 0.0), size: Vec2::new(20.0, 20.0), rotation: 45.0 },
            LayerKind::Rectangle { corner_radius: CornerRadii::ZERO },
        );
        let original_rotated_bounds = layer.frame.rotated_bounds();

        let flattened = flatten_to_compound_path(&layer).expect("rectangle should be flattenable");
        assert_eq!(flattened.id, layer.id, "flatten replaces the layer in place, same id");
        assert_eq!(flattened.frame.rotation, 0.0, "rotation is baked in, not carried forward");
        assert!(matches!(flattened.kind, LayerKind::CompoundPath { .. }));

        // Visual position/size preserved: the new (unrotated) frame's bounds
        // should match the original's *rotated* bounds.
        let new_bounds = flattened.frame.bounds();
        assert!((new_bounds.width() - original_rotated_bounds.width()).abs() < 0.5);
        assert!((new_bounds.height() - original_rotated_bounds.height()).abs() < 0.5);
        assert!((new_bounds.center() - original_rotated_bounds.center()).length() < 0.5);
    }

    #[test]
    fn flatten_unsupported_kinds_return_none() {
        let line = Layer::new(
            "Line",
            Frame { pos: Pos2::new(0.0, 0.0), size: Vec2::new(10.0, 10.0), rotation: 0.0 },
            LK::Line,
        );
        assert!(flatten_to_compound_path(&line).is_none());

        let group = Layer::new(
            "Group",
            Frame { pos: Pos2::new(0.0, 0.0), size: Vec2::new(10.0, 10.0), rotation: 0.0 },
            LK::Group { children: Vec::new() },
        );
        assert!(flatten_to_compound_path(&group).is_none());
    }

    #[test]
    fn rotate_copies_produces_n_new_layers_at_expected_angles() {
        let layer = Layer::new(
            "Rect",
            Frame { pos: Pos2::new(0.0, 0.0), size: Vec2::new(10.0, 10.0), rotation: 0.0 },
            LayerKind::Rectangle { corner_radius: CornerRadii::ZERO },
        );
        let id = layer.id;
        let mut page = Page::new("Page 1");
        page.layers.push(layer);

        let new_ids = rotate_copies(&mut page, &[id], 3, 90.0);
        assert_eq!(new_ids.len(), 3);
        // Original is untouched.
        assert_eq!(page.find(id).unwrap().frame.rotation, 0.0);

        let mut rotations: Vec<f32> = new_ids.iter().map(|&nid| page.find(nid).unwrap().frame.rotation).collect();
        rotations.sort_by(|a, b| a.partial_cmp(b).unwrap());
        for (actual, expected) in rotations.iter().zip([30.0, 60.0, 90.0]) {
            assert!((actual - expected).abs() < 1e-3, "rotations={rotations:?}");
        }
    }

    #[test]
    fn rotate_copies_of_a_group_bakes_rotation_into_each_copys_children() {
        let child_a = Layer::new(
            "A",
            Frame { pos: Pos2::new(0.0, 0.0), size: Vec2::new(10.0, 10.0), rotation: 0.0 },
            LayerKind::Rectangle { corner_radius: CornerRadii::ZERO },
        );
        let child_b = Layer::new(
            "B",
            Frame { pos: Pos2::new(20.0, 0.0), size: Vec2::new(10.0, 10.0), rotation: 0.0 },
            LayerKind::Rectangle { corner_radius: CornerRadii::ZERO },
        );
        let group = Layer::new(
            "Group",
            Frame { pos: Pos2::new(0.0, 0.0), size: Vec2::new(30.0, 10.0), rotation: 0.0 },
            LayerKind::Group { children: vec![child_a, child_b] },
        );
        let group_id = group.id;
        let mut page = Page::new("Page 1");
        page.layers.push(group);

        let new_ids = rotate_copies(&mut page, &[group_id], 1, 90.0);
        assert_eq!(new_ids.len(), 1);
        let copy = page.find(new_ids[0]).unwrap();
        assert_eq!(copy.frame.rotation, 0.0, "the group copy's own rotation never changes");
        let LayerKind::Group { children } = &copy.kind else { unreachable!() };
        assert_eq!(children.len(), 2);
        for child in children {
            assert_eq!(child.frame.rotation, 90.0, "rotation is baked into each child instead");
        }
    }

    #[test]
    fn flip_horizontal_reverses_line_direction() {
        let mut layer = Layer::new(
            "Line",
            Frame { pos: Pos2::new(0.0, 0.0), size: Vec2::new(10.0, 5.0), rotation: 0.0 },
            LayerKind::Line,
        );
        flip_layer(&mut layer, FlipAxis::Horizontal);
        assert_eq!(layer.frame.size, Vec2::new(-10.0, 5.0));
    }

    #[test]
    fn flip_path_mirrors_anchor_and_negates_handle_x() {
        let points = vec![
            PathPoint { anchor: Pos2::new(0.0, 0.0), handle_in: None, handle_out: None, point_type: PointType::Straight, corner_radius: 0.0 },
            PathPoint {
                anchor: Pos2::new(10.0, 4.0),
                handle_in: Some(Vec2::new(2.0, 1.0)),
                handle_out: Some(Vec2::new(-2.0, -1.0)),
                point_type: PointType::Mirror,
                corner_radius: 0.0,
            },
        ];
        let mut layer = Layer::new(
            "Path",
            Frame { pos: Pos2::new(0.0, 0.0), size: Vec2::new(10.0, 4.0), rotation: 0.0 },
            LayerKind::Path { points, closed: false },
        );
        flip_layer(&mut layer, FlipAxis::Horizontal);
        let LayerKind::Path { points, .. } = &layer.kind else { unreachable!() };
        assert_eq!(points[0].anchor, Pos2::new(10.0, 0.0));
        assert_eq!(points[1].anchor, Pos2::new(0.0, 4.0));
        assert_eq!(points[1].handle_in, Some(Vec2::new(-2.0, 1.0)));
        assert_eq!(points[1].handle_out, Some(Vec2::new(2.0, -1.0)));
    }

    #[test]
    fn flip_twice_is_identity_for_path() {
        let points = vec![
            PathPoint { anchor: Pos2::new(1.0, 1.0), handle_in: None, handle_out: None, point_type: PointType::Straight, corner_radius: 0.0 },
            PathPoint {
                anchor: Pos2::new(9.0, 5.0),
                handle_in: Some(Vec2::new(1.0, 2.0)),
                handle_out: None,
                point_type: PointType::Asymmetric,
                corner_radius: 0.0,
            },
        ];
        let original = points.clone();
        let mut layer = Layer::new(
            "Path",
            Frame { pos: Pos2::new(3.0, 3.0), size: Vec2::new(10.0, 6.0), rotation: 0.0 },
            LayerKind::Path { points, closed: false },
        );
        flip_layer(&mut layer, FlipAxis::Vertical);
        flip_layer(&mut layer, FlipAxis::Vertical);
        let LayerKind::Path { points, .. } = &layer.kind else { unreachable!() };
        assert_eq!(points, &original);
        assert_eq!(layer.frame.size, Vec2::new(10.0, 6.0));
    }
}
