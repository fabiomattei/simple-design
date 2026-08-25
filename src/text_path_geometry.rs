//! Pure geometry for a `Text` layer's `path_attachment`: resolving the
//! target layer's outline (as an absolute-space polyline) and the arc-length
//! / scanline queries `text_on_path_layout.rs`/`text_area_layout.rs` need to
//! place glyphs or wrap lines against it. Shared by the canvas renderer
//! (`canvas.rs`) and the exporter (`export.rs`) so both agree on where the
//! outline actually is, the same reason `shapes.rs` centralizes plain shape
//! point generation.
//!
//! No egui painter / tiny-skia dependency — everything here is plain
//! `Vec<Pos2>` in, `Vec<Pos2>` out, same convention as `shapes.rs`.

use egui::{Pos2, Vec2};

use crate::canvas::flatten_path;
use crate::model::{Layer, LayerId};
use crate::shapes::{ellipse_points, polygon_points, rotate_point, rounded_rect_points, star_points};

/// Searches `roots` (a page's top-level layers, or a single exported layer's
/// subtree passed as `std::slice::from_ref(root)`) for `target`, returning
/// its outline as an absolute-space polyline plus whether it's closed.
/// `None` if `target` doesn't resolve, or resolves to a kind with no usable
/// outline (`Group`/`Artboard`/`Text`/`Image`) — callers treat that the same
/// as "no attachment," falling back to plain layout.
pub fn resolve_target_outline(roots: &[Layer], target: LayerId) -> Option<(Vec<Pos2>, bool)> {
    let (offset, layer) = find_with_absolute_offset(roots, target, Vec2::ZERO)?;
    target_outline(layer, offset)
}

/// The accumulated offset of `id`'s ancestors within `roots` — i.e. the
/// value that must be added to `id`'s own `frame.pos` to land in the same
/// absolute space `resolve_target_outline`'s output is already in. `export.rs`
/// uses this (for both the `Text` layer itself and its attachment target) to
/// convert that "root-relative" outline into whatever pixmap-space offset
/// its own recursion happens to be accumulating, without needing a second
/// offset accumulator threaded through every draw function alongside `root`.
/// Mirrors `model::document::Page::absolute_offset`, just generalized to any
/// `roots` slice (a page's top-level layers, or a single exported layer's
/// subtree) instead of always `Page::layers`.
pub fn absolute_offset_of(roots: &[Layer], id: LayerId) -> Option<Vec2> {
    find_with_absolute_offset(roots, id, Vec2::ZERO).map(|(offset, _)| offset)
}

fn find_with_absolute_offset(layers: &[Layer], id: LayerId, offset: Vec2) -> Option<(Vec2, &Layer)> {
    for layer in layers {
        if layer.id == id {
            return Some((offset, layer));
        }
        if let Some(children) = layer.kind.children() {
            let child_offset = offset + layer.frame.pos.to_vec2();
            if let Some(found) = find_with_absolute_offset(children, id, child_offset) {
                return Some(found);
            }
        }
    }
    None
}

/// `layer`'s own outline translated by `offset` (the accumulated offset of
/// its ancestors, from `find_with_absolute_offset`) and rotated by its own
/// `frame.rotation` — i.e. fully resolved to the same absolute space
/// `offset` is already in. Mirrors `boolean_ops::flatten_layer`'s per-kind
/// geometry (including its "largest region wins" simplification for a
/// multi-region `CompoundPath`/`BooleanGroup`, which has no single "the"
/// outline) rather than re-deriving it, but returns a plain point list (not
/// a `geo::Polygon`) since callers need to walk it as a path, not test
/// point-containment.
fn target_outline(layer: &Layer, offset: Vec2) -> Option<(Vec<Pos2>, bool)> {
    use crate::model::LayerKind;

    let bounds = layer.frame.bounds().translate(offset);
    let center = bounds.center();
    let rotation = layer.frame.rotation;
    let rotated = |pts: Vec<Pos2>| -> Vec<Pos2> { pts.into_iter().map(|p| rotate_point(p, center, rotation)).collect() };

    match &layer.kind {
        LayerKind::Rectangle { corner_radius } => Some((rotated(rounded_rect_points(bounds, corner_radius.as_array())), true)),
        LayerKind::Oval => Some((rotated(ellipse_points(center, bounds.width() / 2.0, bounds.height() / 2.0)), true)),
        LayerKind::Star { points, inner_ratio } => {
            Some((rotated(star_points(center, bounds.width() / 2.0, bounds.height() / 2.0, *points, *inner_ratio)), true))
        }
        LayerKind::Polygon { sides } => Some((rotated(polygon_points(center, bounds.width() / 2.0, bounds.height() / 2.0, *sides)), true)),
        LayerKind::Line | LayerKind::Arrow { .. } => {
            Some((rotated(vec![layer.frame.start() + offset, layer.frame.end() + offset]), false))
        }
        LayerKind::Path { points, closed } => {
            if points.len() < 2 {
                return None;
            }
            let local_offset = offset + layer.frame.pos.to_vec2();
            let pts = flatten_path(points, *closed).into_iter().map(|p| p + local_offset).collect();
            Some((rotated(pts), *closed))
        }
        LayerKind::CompoundPath { polygons } => {
            let local_offset = offset + layer.frame.pos.to_vec2();
            let largest = largest_ring(polygons.iter().map(|p| p.exterior.as_slice()))?;
            Some((rotated(largest.iter().map(|p| *p + local_offset).collect()), true))
        }
        LayerKind::BooleanGroup { children } => {
            let combined = crate::boolean_ops::compute_boolean_group(children);
            let polygons = crate::boolean_ops::multipolygon_to_polygons(&combined);
            let local_offset = offset + layer.frame.pos.to_vec2();
            let largest = largest_ring(polygons.iter().map(|p| p.exterior.as_slice()))?;
            Some((rotated(largest.iter().map(|p| *p + local_offset).collect()), true))
        }
        LayerKind::Artboard { .. } | LayerKind::Group { .. } | LayerKind::Text { .. } | LayerKind::Image { .. } => None,
    }
}

/// The largest-area ring among `rings` (by absolute shoelace area) — the
/// "one outline" a multi-region `CompoundPath`/`BooleanGroup` contributes to
/// text attachment, since those kinds can have several disjoint exteriors
/// with no single correct choice. `None` if `rings` is empty.
fn largest_ring<'a>(rings: impl Iterator<Item = &'a [Pos2]>) -> Option<&'a [Pos2]> {
    rings.max_by(|a, b| polygon_area(a).partial_cmp(&polygon_area(b)).unwrap_or(std::cmp::Ordering::Equal))
}

fn polygon_area(points: &[Pos2]) -> f32 {
    if points.len() < 3 {
        return 0.0;
    }
    let mut area = 0.0;
    for i in 0..points.len() {
        let a = points[i];
        let b = points[(i + 1) % points.len()];
        area += a.x * b.y - b.x * a.y;
    }
    (area * 0.5).abs()
}

/// Total length of the polyline `points` forms, closing the last segment
/// back to `points[0]` if `closed`.
pub fn polyline_length(points: &[Pos2], closed: bool) -> f32 {
    if points.len() < 2 {
        return 0.0;
    }
    let mut len = 0.0;
    for i in 0..points.len() - 1 {
        len += (points[i + 1] - points[i]).length();
    }
    if closed {
        len += (points[0] - points[points.len() - 1]).length();
    }
    len
}

/// The point and tangent angle (radians, same clockwise/y-down convention as
/// `egui::epaint::TextShape::angle`) at arc-length `distance` along `points`.
/// For a closed path, `distance` wraps modulo the total length; for an open
/// one, it clamps to `[0, length]`. `None` if `points` has fewer than 2
/// vertices or their total length is ~zero.
pub fn point_and_tangent_at(points: &[Pos2], closed: bool, distance: f32) -> Option<(Pos2, f32)> {
    let total = polyline_length(points, closed);
    if points.len() < 2 || total < 1e-6 {
        return None;
    }
    let mut remaining = if closed { distance.rem_euclid(total) } else { distance.clamp(0.0, total) };
    let n = points.len();
    let segment_count = if closed { n } else { n - 1 };
    for i in 0..segment_count {
        let a = points[i];
        let b = points[(i + 1) % n];
        let seg_len = (b - a).length();
        if seg_len < 1e-6 {
            continue;
        }
        if remaining <= seg_len || i == segment_count - 1 {
            let t = (remaining / seg_len).clamp(0.0, 1.0);
            let point = a + (b - a) * t;
            let angle = (b.y - a.y).atan2(b.x - a.x);
            return Some((point, angle));
        }
        remaining -= seg_len;
    }
    None
}

/// The x-intervals (sorted, left to right) where the horizontal scanline at
/// `y` is inside the closed polygon `points`, via the standard even-odd
/// crossing rule. Empty if `y` misses the polygon's vertical range entirely
/// or `points` has fewer than 3 vertices. Ignores winding direction (only
/// crossing parity matters), same as `CompoundPath`'s even-odd fill.
pub fn horizontal_spans_at_y(points: &[Pos2], y: f32) -> Vec<(f32, f32)> {
    if points.len() < 3 {
        return Vec::new();
    }
    let n = points.len();
    let mut xs: Vec<f32> = Vec::new();
    for i in 0..n {
        let a = points[i];
        let b = points[(i + 1) % n];
        if (a.y <= y && b.y > y) || (b.y <= y && a.y > y) {
            let t = (y - a.y) / (b.y - a.y);
            xs.push(a.x + t * (b.x - a.x));
        }
    }
    xs.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    xs.chunks(2).filter(|c| c.len() == 2).map(|c| (c[0], c[1])).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn polyline_length_of_open_square_side_sums_three_edges() {
        let pts = vec![Pos2::new(0.0, 0.0), Pos2::new(10.0, 0.0), Pos2::new(10.0, 10.0), Pos2::new(0.0, 10.0)];
        assert!((polyline_length(&pts, false) - 30.0).abs() < 1e-3);
    }

    #[test]
    fn polyline_length_of_closed_square_sums_four_edges() {
        let pts = vec![Pos2::new(0.0, 0.0), Pos2::new(10.0, 0.0), Pos2::new(10.0, 10.0), Pos2::new(0.0, 10.0)];
        assert!((polyline_length(&pts, true) - 40.0).abs() < 1e-3);
    }

    #[test]
    fn point_and_tangent_at_start_of_horizontal_segment() {
        let pts = vec![Pos2::new(0.0, 0.0), Pos2::new(10.0, 0.0)];
        let (p, angle) = point_and_tangent_at(&pts, false, 0.0).unwrap();
        assert!((p.x - 0.0).abs() < 1e-3 && (p.y - 0.0).abs() < 1e-3);
        assert!(angle.abs() < 1e-3);
    }

    #[test]
    fn point_and_tangent_at_clamps_past_open_path_end() {
        let pts = vec![Pos2::new(0.0, 0.0), Pos2::new(10.0, 0.0)];
        let (p, _) = point_and_tangent_at(&pts, false, 1000.0).unwrap();
        assert!((p.x - 10.0).abs() < 1e-3);
    }

    #[test]
    fn point_and_tangent_at_wraps_past_closed_path_length() {
        let pts = vec![Pos2::new(0.0, 0.0), Pos2::new(10.0, 0.0), Pos2::new(10.0, 10.0), Pos2::new(0.0, 10.0)];
        let total = polyline_length(&pts, true);
        let (p_wrapped, _) = point_and_tangent_at(&pts, true, total + 5.0).unwrap();
        let (p_direct, _) = point_and_tangent_at(&pts, true, 5.0).unwrap();
        assert!((p_wrapped - p_direct).length() < 1e-3);
    }

    #[test]
    fn horizontal_spans_at_y_finds_one_span_through_a_square() {
        let square = vec![Pos2::new(0.0, 0.0), Pos2::new(10.0, 0.0), Pos2::new(10.0, 10.0), Pos2::new(0.0, 10.0)];
        let spans = horizontal_spans_at_y(&square, 5.0);
        assert_eq!(spans.len(), 1);
        assert!((spans[0].0 - 0.0).abs() < 1e-3);
        assert!((spans[0].1 - 10.0).abs() < 1e-3);
    }

    #[test]
    fn horizontal_spans_at_y_is_empty_outside_the_shape() {
        let square = vec![Pos2::new(0.0, 0.0), Pos2::new(10.0, 0.0), Pos2::new(10.0, 10.0), Pos2::new(0.0, 10.0)];
        assert!(horizontal_spans_at_y(&square, 50.0).is_empty());
    }
}
