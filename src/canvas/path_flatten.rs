//! Flattens a `Path`'s Bezier segments and rounded corners into a polyline.


use egui::{Pos2, Vec2};

use crate::model::PathPoint;

pub(super) const BEZIER_SAMPLES_PER_SEGMENT: usize = 16;

pub(super) fn cubic_bezier_point(p0: Pos2, p1: Pos2, p2: Pos2, p3: Pos2, t: f32) -> Pos2 {
    let mt = 1.0 - t;
    let a = mt * mt * mt;
    let b = 3.0 * mt * mt * t;
    let c = 3.0 * mt * t * t;
    let d = t * t * t;
    Pos2::new(
        a * p0.x + b * p1.x + c * p2.x + d * p3.x,
        a * p0.y + b * p1.y + c * p2.y + d * p3.y,
    )
}

/// Whether `points[i]`'s corner can be rounded: it has a positive
/// `corner_radius`, no handle of its own, and both its neighboring segments
/// are straight lines (matching `PathPoint::corner_radius`'s doc comment).
/// Always `false` for an open path's two endpoints — rounding a corner needs
/// both a previous and a next anchor to inset along.
pub(super) fn is_roundable_corner(points: &[PathPoint], closed: bool, i: usize) -> bool {
    let n = points.len();
    let p = &points[i];
    if p.corner_radius <= 0.0 || p.handle_in.is_some() || p.handle_out.is_some() {
        return false;
    }
    let prev = if i == 0 {
        if !closed {
            return false;
        }
        n - 1
    } else {
        i - 1
    };
    let next = if i == n - 1 {
        if !closed {
            return false;
        }
        0
    } else {
        i + 1
    };
    points[prev].handle_out.is_none() && points[next].handle_in.is_none()
}

/// `[inset_from_prev, ...arc_samples..., inset_to_next]` for a rounded
/// corner at `points[i]` — see `shapes::rounded_corner_arc_points`. Only
/// called after `is_roundable_corner` confirms `i`'s wraparound neighbors
/// are valid (i.e. `closed` if `i` is an endpoint), so no `closed` parameter
/// is needed here.
pub(super) fn corner_arc_points(points: &[PathPoint], i: usize) -> Vec<Pos2> {
    let n = points.len();
    let prev = if i == 0 { n - 1 } else { i - 1 };
    let next = if i == n - 1 { 0 } else { i + 1 };
    crate::shapes::rounded_corner_arc_points(points[prev].anchor, points[i].anchor, points[next].anchor, points[i].corner_radius)
}

/// Flattens anchors + bezier handles into a polyline, for egui's `PathShape`
/// (which has no native curve primitive covering fill of a multi-segment
/// curve, unlike `tiny-skia`'s `cubic_to` used in `export.rs`). Output is in
/// the same coordinate space as the input anchors — the caller applies
/// whatever offset/to-screen transform is appropriate.
///
/// A corner with `corner_radius > 0.0` (and no handles of its own, and both
/// neighboring segments straight — see `is_roundable_corner`) contributes an
/// inset-and-arc (`corner_arc_points`) instead of its raw anchor. Only ever
/// possible between two straight segments — a rounded corner can't also be a
/// bezier endpoint, since rounding requires both adjacent segments straight
/// by definition — so the existing straight/curved per-segment split below
/// only needs a rounding check on the straight branch.
pub(crate) fn flatten_path(points: &[PathPoint], closed: bool) -> Vec<Pos2> {
    if points.is_empty() {
        return Vec::new();
    }
    let n = points.len();
    let mut out = if is_roundable_corner(points, closed, 0) {
        corner_arc_points(points, 0)
    } else {
        vec![points[0].anchor]
    };
    let last_index = if closed { n } else { n - 1 };
    for i in 0..last_index {
        let a = &points[i];
        let b_idx = (i + 1) % n;
        let b = &points[b_idx];
        let p0 = a.anchor;
        let p3 = b.anchor;
        if a.handle_out.is_none() && b.handle_in.is_none() {
            if b_idx == 0 && closed {
                // Anchor 0's entry/arc was already seeded above (including
                // its "entry from the previous/last segment" leg) — the
                // implicit closing edge (last point back to `out[0]`)
                // completes this final segment, nothing more to push.
            } else if is_roundable_corner(points, closed, b_idx) {
                out.extend(corner_arc_points(points, b_idx));
            } else {
                out.push(p3);
            }
        } else {
            let c1 = p0 + a.handle_out.unwrap_or(Vec2::ZERO);
            let c2 = p3 + b.handle_in.unwrap_or(Vec2::ZERO);
            for s in 1..=BEZIER_SAMPLES_PER_SEGMENT {
                let t = s as f32 / BEZIER_SAMPLES_PER_SEGMENT as f32;
                out.push(cubic_bezier_point(p0, c1, c2, p3, t));
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::PointType;

    fn straight_point(x: f32, y: f32) -> PathPoint {
        PathPoint {
            anchor: Pos2::new(x, y),
            handle_in: None,
            handle_out: None,
            point_type: PointType::Disconnected,
            corner_radius: 0.0,
        }
    }

    fn rounded_point(x: f32, y: f32, radius: f32) -> PathPoint {
        PathPoint {
            anchor: Pos2::new(x, y),
            handle_in: None,
            handle_out: None,
            point_type: PointType::Straight,
            corner_radius: radius,
        }
    }

    #[test]
    fn flatten_path_rounds_a_corner_with_radius() {
        let square = vec![
            straight_point(0.0, 0.0),
            rounded_point(40.0, 0.0, 8.0),
            straight_point(40.0, 40.0),
            straight_point(0.0, 40.0),
        ];
        let flat = flatten_path(&square, true);
        // A plain square would have exactly 4 vertices; a rounded corner
        // inserts several arc-sampled points instead of the single (40,0).
        assert!(flat.len() > 4, "expected extra arc points, got {}", flat.len());
        // No point in the polyline should land exactly on the un-rounded
        // corner (40,0) — it's been inset away on both sides.
        assert!(flat.iter().all(|p| (*p - Pos2::new(40.0, 0.0)).length() > 0.01));
    }

    #[test]
    fn flatten_path_clamps_radius_to_half_shorter_adjacent_segment() {
        // Corner at (40,0) with legs of length 40 (to (0,0)) and 10 (to
        // (40,10)) — radius should clamp to 5 (half the shorter leg).
        let points = vec![
            straight_point(0.0, 0.0),
            rounded_point(40.0, 0.0, 1000.0),
            straight_point(40.0, 10.0),
        ];
        let flat = flatten_path(&points, false);
        // The arc's exit point toward (40,10) should sit at exactly (40,5)
        // — half the shorter (length-10) adjacent leg, not the requested
        // radius of 1000.
        let target = Pos2::new(40.0, 5.0);
        let closest = flat.iter().min_by(|a, b| {
            (**a - target).length().partial_cmp(&(**b - target).length()).unwrap()
        }).unwrap();
        assert!((*closest - target).length() < 0.1, "expected a point at {target:?}, closest was {closest:?}");
    }

    #[test]
    fn flatten_path_zero_radius_is_unchanged() {
        let square = vec![
            straight_point(0.0, 0.0),
            straight_point(40.0, 0.0),
            straight_point(40.0, 40.0),
            straight_point(0.0, 40.0),
        ];
        assert_eq!(flatten_path(&square, true).len(), 4);
    }
}
