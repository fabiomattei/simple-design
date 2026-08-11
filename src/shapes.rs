//! Pure shape-geometry generators shared by the live canvas renderer
//! (`canvas.rs`), the PNG exporter (`export.rs`), and boolean-op flattening
//! (`boolean_ops.rs`). No egui painter / tiny-skia dependency — everything
//! here is plain `Vec<Pos2>` in, `Vec<Pos2>` out, so all three consumers stay
//! in sync by construction instead of re-deriving the same math three times.

use egui::{Pos2, Rect};

/// Rotates `p` about `center` by `degrees` clockwise (screen-space y-down
/// convention, matching every other angle in this codebase).
pub fn rotate_point(p: Pos2, center: Pos2, degrees: f32) -> Pos2 {
    if degrees == 0.0 {
        return p;
    }
    let rad = degrees.to_radians();
    let (sin, cos) = rad.sin_cos();
    let d = p - center;
    Pos2::new(center.x + d.x * cos - d.y * sin, center.y + d.x * sin + d.y * cos)
}

/// The 4 corners of `rect`, rotated about `rect.center()` by `degrees`,
/// clockwise starting at the top-left. Used for drawing a rotated selection
/// outline / resize handles.
pub fn rotated_corners(rect: Rect, degrees: f32) -> [Pos2; 4] {
    let c = rect.center();
    [
        rotate_point(rect.left_top(), c, degrees),
        rotate_point(rect.right_top(), c, degrees),
        rotate_point(rect.right_bottom(), c, degrees),
        rotate_point(rect.left_bottom(), c, degrees),
    ]
}

/// An ellipse outline inscribed in a `2*rx` x `2*ry` box centered on `center`, as a fixed 64-point polygon ring.
pub fn ellipse_points(center: Pos2, rx: f32, ry: f32) -> Vec<Pos2> {
    const N: usize = 64;
    (0..N)
        .map(|i| {
            let t = i as f32 / N as f32 * std::f32::consts::TAU;
            Pos2::new(center.x + rx * t.cos(), center.y + ry * t.sin())
        })
        .collect()
}

fn arc_points(center: Pos2, r: f32, start_deg: f32, end_deg: f32, segments: usize) -> Vec<Pos2> {
    (0..=segments)
        .map(|i| {
            let t = (start_deg + (end_deg - start_deg) * (i as f32 / segments as f32)).to_radians();
            Pos2::new(center.x + r * t.cos(), center.y + r * t.sin())
        })
        .collect()
}

/// A rounded rect's outline as a polygon ring, corners flattened to arcs.
/// Consecutive corner arcs are joined by implicit straight edges (the
/// polygon boundary between the last point of one arc and the first point
/// of the next) — no separate straight-edge points are needed. `radii` is
/// CSS `border-radius` order: `[top_left, top_right, bottom_right,
/// bottom_left]`; a corner under the flattening threshold degrades to a
/// plain sharp vertex instead of a degenerate zero-radius arc.
pub fn rounded_rect_points(rect: Rect, radii: [f32; 4]) -> Vec<Pos2> {
    let max_r = rect.width().min(rect.height()) / 2.0;
    let [tl, tr, br, bl] = radii.map(|r| r.max(0.0).min(max_r));
    if tl < 0.5 && tr < 0.5 && br < 0.5 && bl < 0.5 {
        return vec![rect.left_top(), rect.right_top(), rect.right_bottom(), rect.left_bottom()];
    }
    const SEGMENTS: usize = 8;
    let mut pts = Vec::with_capacity(SEGMENTS * 4 + 4);
    if tr < 0.5 {
        pts.push(rect.right_top());
    } else {
        pts.extend(arc_points(Pos2::new(rect.max.x - tr, rect.min.y + tr), tr, -90.0, 0.0, SEGMENTS));
    }
    if br < 0.5 {
        pts.push(rect.right_bottom());
    } else {
        pts.extend(arc_points(Pos2::new(rect.max.x - br, rect.max.y - br), br, 0.0, 90.0, SEGMENTS));
    }
    if bl < 0.5 {
        pts.push(rect.left_bottom());
    } else {
        pts.extend(arc_points(Pos2::new(rect.min.x + bl, rect.max.y - bl), bl, 90.0, 180.0, SEGMENTS));
    }
    if tl < 0.5 {
        pts.push(rect.left_top());
    } else {
        pts.extend(arc_points(Pos2::new(rect.min.x + tl, rect.min.y + tl), tl, 180.0, 270.0, SEGMENTS));
    }
    pts
}

/// A regular star outline: `points` outer vertices alternating with `points`
/// inner vertices, starting straight up (12 o'clock), the default
/// star orientation. `inner_ratio` (`0.0..=1.0`) is the inner vertices'
/// radius as a fraction of the outer radius.
pub fn star_points(center: Pos2, rx: f32, ry: f32, points: u32, inner_ratio: f32) -> Vec<Pos2> {
    let points = points.max(3);
    let n = points * 2;
    (0..n)
        .map(|i| {
            let angle = i as f32 / n as f32 * std::f32::consts::TAU - std::f32::consts::FRAC_PI_2;
            let scale = if i % 2 == 0 { 1.0 } else { inner_ratio.clamp(0.0, 1.0) };
            Pos2::new(center.x + rx * scale * angle.cos(), center.y + ry * scale * angle.sin())
        })
        .collect()
}

/// A regular `sides`-gon outline, starting straight up (12 o'clock), the
/// default polygon orientation.
pub fn polygon_points(center: Pos2, rx: f32, ry: f32, sides: u32) -> Vec<Pos2> {
    let sides = sides.max(3);
    (0..sides)
        .map(|i| {
            let angle = i as f32 / sides as f32 * std::f32::consts::TAU - std::f32::consts::FRAC_PI_2;
            Pos2::new(center.x + rx * angle.cos(), center.y + ry * angle.sin())
        })
        .collect()
}

/// Rounds the corner at `corner` (between straight segments `prev->corner`
/// and `corner->next`) by inset-and-arc: moves in along each adjacent
/// segment by `radius` (clamped to at most half of the shorter adjacent
/// segment, matching a "maximum radius" behavior) and arcs between
/// the two inset points. Returns `[inset_from_prev, ...arc_samples...,
/// inset_to_next]`; an empty vec if `radius <= 0`.
pub fn rounded_corner_arc_points(prev: Pos2, corner: Pos2, next: Pos2, radius: f32) -> Vec<Pos2> {
    if radius <= 0.0 {
        return Vec::new();
    }
    let to_prev = prev - corner;
    let to_next = next - corner;
    let len_prev = to_prev.length();
    let len_next = to_next.length();
    if len_prev < 1e-6 || len_next < 1e-6 {
        return Vec::new();
    }
    let max_radius = (len_prev.min(len_next)) * 0.5;
    let r = radius.min(max_radius);
    if r < 1e-6 {
        return vec![corner];
    }
    let dir_prev = to_prev / len_prev;
    let dir_next = to_next / len_next;
    let inset_prev = corner + dir_prev * r;
    let inset_next = corner + dir_next * r;

    // Distance from `corner` to the arc's center, along the angle bisector:
    // for an isoceles triangle with the two equal sides = r and base angle
    // half the corner angle, center_dist = r / sin(half_angle) is the
    // standard tangent-circle construction — but simpler to just find the
    // center as the point equidistant (by r) from both inset points, lying
    // on the bisector, via the angle between dir_prev/dir_next directly.
    let cos_theta = (dir_prev.x * dir_next.x + dir_prev.y * dir_next.y).clamp(-1.0, 1.0);
    let half_angle = (cos_theta.acos()) * 0.5;
    if half_angle < 1e-4 || half_angle > std::f32::consts::FRAC_PI_2 - 1e-4 {
        // Degenerate (near-straight or near-180) corner: skip the arc, just
        // use the two inset points with a straight join between them.
        return vec![inset_prev, inset_next];
    }
    let center_dist = r / half_angle.sin();
    let bisector = (dir_prev + dir_next).normalized();
    let arc_center = corner + bisector * center_dist;

    let start_angle = (inset_prev - arc_center).angle();
    let end_angle = (inset_next - arc_center).angle();
    // Walk from start to end the short way around.
    let mut delta = end_angle - start_angle;
    if delta > std::f32::consts::PI {
        delta -= std::f32::consts::TAU;
    } else if delta < -std::f32::consts::PI {
        delta += std::f32::consts::TAU;
    }
    const SEGMENTS: usize = 8;
    (0..=SEGMENTS)
        .map(|i| {
            let a = start_angle + delta * (i as f32 / SEGMENTS as f32);
            arc_center + egui::Vec2::new(a.cos(), a.sin()) * r
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rotate_point_by_zero_is_identity() {
        let p = Pos2::new(3.0, 4.0);
        assert_eq!(rotate_point(p, Pos2::new(1.0, 1.0), 0.0), p);
    }

    #[test]
    fn rotate_point_by_90_degrees_about_origin() {
        let p = Pos2::new(1.0, 0.0);
        let rotated = rotate_point(p, Pos2::ZERO, 90.0);
        assert!((rotated.x - 0.0).abs() < 1e-4);
        assert!((rotated.y - 1.0).abs() < 1e-4);
    }

    #[test]
    fn star_points_has_2n_vertices() {
        let pts = star_points(Pos2::ZERO, 10.0, 10.0, 5, 0.5);
        assert_eq!(pts.len(), 10);
    }

    #[test]
    fn polygon_points_has_n_vertices() {
        let pts = polygon_points(Pos2::ZERO, 10.0, 10.0, 6);
        assert_eq!(pts.len(), 6);
    }

    #[test]
    fn rounded_corner_arc_clamps_to_half_shorter_segment() {
        // Right-angle corner at origin, legs of length 10 and 4 along axes.
        let prev = Pos2::new(0.0, -10.0);
        let corner = Pos2::ZERO;
        let next = Pos2::new(4.0, 0.0);
        let pts = rounded_corner_arc_points(prev, corner, next, 100.0);
        // Radius should clamp to half the shorter leg (4/2 = 2).
        let inset_next = pts.last().unwrap();
        assert!((inset_next.x - 2.0).abs() < 1e-3, "inset_next={inset_next:?}");
    }
}
