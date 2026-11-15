//! `tiny-skia` geometry for the exporter: paths, rounded rectangles, arrow
//! caps, rotation transforms, and the shared fill-then-stroke step.

use tiny_skia::{Color, FillRule, Paint, PathBuilder, Pixmap, Rect as SkRect, Stroke as SkStroke, Transform};

use crate::model::{CornerRadii, Layer, PathPoint, PathPolygon};

use super::paint::to_sk_paint;

pub(super) fn fill_and_stroke(
    pixmap: &mut Pixmap,
    path: &tiny_skia::Path,
    layer: &Layer,
    fill_rule: FillRule,
    bounds: egui::Rect,
    opacity: f32,
    transform: Transform,
) {
    if let Some(fill) = layer.style.fill.as_ref() {
        let mut noise_storage = None;
        let paint = to_sk_paint(fill, bounds, opacity * layer.style.fill_opacity, &mut noise_storage);
        pixmap.fill_path(path, &paint, fill_rule, transform, None);
    }
    if let Some(stroke) = layer.style.stroke.as_ref() {
        let mut noise_storage = None;
        let paint = to_sk_paint(&stroke.paint, bounds, opacity * layer.style.stroke_opacity, &mut noise_storage);
        let sk_stroke = SkStroke {
            width: stroke.width,
            ..Default::default()
        };
        pixmap.stroke_path(path, &paint, &sk_stroke, transform, None);
    }
}

/// `tiny_skia::Transform::from_rotate_at` centered on `bounds`'s (already
/// offset-translated) center — the one place every non-Text/Image shape arm
/// below builds its rotation transform, so `layer.frame.rotation`'s pivot
/// convention (`bounds().center()`, per `model/layer.rs`) stays consistent.
/// `Transform::identity()` at `rotation == 0.0` (the overwhelmingly common
/// case) avoids the floating-point round-trip through `from_rotate_at` for
/// unrotated shapes, though the two are mathematically equivalent.
pub(super) fn rotation_transform(layer: &Layer, bounds: egui::Rect) -> Transform {
    if layer.frame.rotation == 0.0 {
        Transform::identity()
    } else {
        Transform::from_rotate_at(layer.frame.rotation, bounds.center().x, bounds.center().y)
    }
}

/// Builds a `tiny-skia` path from anchors + bezier handles, offsetting every
/// point by `offset` (the layer's absolute position within the pixmap).
/// Straight segments (no handle on either side) use `line_to`; segments with
/// a handle use a native cubic bezier rather than manual flattening — unless
/// any point has a nonzero `corner_radius`, in which case the whole path is
/// built from `canvas::flatten_path`'s already-rounding-aware sampled
/// polyline instead (via `line_to` for every sample), so canvas and export
/// share the exact same corner-rounding geometry rather than reimplementing
/// `shapes::rounded_corner_arc_points`'s inset-and-arc math a second time
/// here. Every existing (`corner_radius == 0.0` for every point, true for
/// all pre-existing documents) path keeps the original native-bezier path
/// unchanged.
pub(super) fn path_to_sk_path(points: &[PathPoint], closed: bool, offset: egui::Vec2) -> Option<tiny_skia::Path> {
    if points.len() < 2 {
        return None;
    }
    if points.iter().any(|p| p.corner_radius > 0.0) {
        let flat = crate::canvas::flatten_path(points, closed);
        if flat.len() < 2 {
            return None;
        }
        let mut pb = PathBuilder::new();
        let start = flat[0] + offset;
        pb.move_to(start.x, start.y);
        for p in &flat[1..] {
            let p = *p + offset;
            pb.line_to(p.x, p.y);
        }
        if closed {
            pb.close();
        }
        return pb.finish();
    }
    let abs = |p: &PathPoint| p.anchor + offset;
    let mut pb = PathBuilder::new();
    let start = abs(&points[0]);
    pb.move_to(start.x, start.y);
    let n = points.len();
    let last_index = if closed { n } else { n - 1 };
    for i in 0..last_index {
        let a = &points[i];
        let b = &points[(i + 1) % n];
        let p0 = abs(a);
        let p3 = abs(b);
        if a.handle_out.is_none() && b.handle_in.is_none() {
            pb.line_to(p3.x, p3.y);
        } else {
            let c1 = p0 + a.handle_out.unwrap_or(egui::Vec2::ZERO);
            let c2 = p3 + b.handle_in.unwrap_or(egui::Vec2::ZERO);
            pb.cubic_to(c1.x, c1.y, c2.x, c2.y, p3.x, p3.y);
        }
    }
    if closed {
        pb.close();
    }
    pb.finish()
}

/// Builds one multi-subpath `tiny-skia` path from every polygon's exterior +
/// hole rings, offsetting every point by `offset`. Fill with `FillRule::EvenOdd`
/// so holes actually punch through (`Winding` would just fill everything).
pub(super) fn compound_path_to_sk_path(polygons: &[PathPolygon], offset: egui::Vec2) -> Option<tiny_skia::Path> {
    let mut pb = PathBuilder::new();
    let mut any_ring = false;
    for poly in polygons {
        for ring in std::iter::once(&poly.exterior).chain(poly.holes.iter()) {
            if ring.len() < 3 {
                continue;
            }
            any_ring = true;
            let start = ring[0] + offset;
            pb.move_to(start.x, start.y);
            for p in &ring[1..] {
                let p = *p + offset;
                pb.line_to(p.x, p.y);
            }
            pb.close();
        }
    }
    if !any_ring {
        return None;
    }
    pb.finish()
}

pub(super) fn rounded_rect_path(rect: egui::Rect, radii: CornerRadii) -> Option<tiny_skia::Path> {
    let mut pb = PathBuilder::new();
    let max_r = rect.width().min(rect.height()) / 2.0;
    let [tl, tr, br, bl] = radii.as_array().map(|r| r.max(0.0).min(max_r));
    if tl < 0.5 && tr < 0.5 && br < 0.5 && bl < 0.5 {
        pb.push_rect(to_sk_rect(rect)?);
    } else {
        pb.move_to(rect.min.x + tl, rect.min.y);
        pb.line_to(rect.max.x - tr, rect.min.y);
        if tr >= 0.5 {
            pb.quad_to(rect.max.x, rect.min.y, rect.max.x, rect.min.y + tr);
        }
        pb.line_to(rect.max.x, rect.max.y - br);
        if br >= 0.5 {
            pb.quad_to(rect.max.x, rect.max.y, rect.max.x - br, rect.max.y);
        }
        pb.line_to(rect.min.x + bl, rect.max.y);
        if bl >= 0.5 {
            pb.quad_to(rect.min.x, rect.max.y, rect.min.x, rect.max.y - bl);
        }
        pb.line_to(rect.min.x, rect.min.y + tl);
        if tl >= 0.5 {
            pb.quad_to(rect.min.x, rect.min.y, rect.min.x + tl, rect.min.y);
        }
        pb.close();
    }
    pb.finish()
}

/// Cap size for an `Arrow` end marker, scaled off the stroke width — mirrors
/// `canvas/draw_layer.rs::draw_arrow_cap`'s sizing exactly, so canvas and export caps
/// match visually.
pub(super) fn arrow_cap_size(stroke_width: f32) -> f32 {
    (stroke_width * 3.5).max(8.0)
}

/// The two "wing" points of a `Triangle`/`Line` cap at `tip`, pointing
/// `outward` from the shaft — `None` if `outward` is degenerate (zero-length,
/// a start/end coincident with the other endpoint).
pub(super) fn arrow_cap_wings(tip: egui::Pos2, outward: egui::Vec2, stroke_width: f32) -> Option<(egui::Pos2, egui::Pos2)> {
    if outward.length() < 1e-3 {
        return None;
    }
    let dir = outward.normalized();
    let perp = egui::Vec2::new(-dir.y, dir.x);
    let size = arrow_cap_size(stroke_width);
    let base = tip - dir * size;
    Some((base + perp * size * 0.5, base - perp * size * 0.5))
}

/// A closed `tiny-skia` path tracing `points` (already in absolute pixmap
/// space) in order — the shared builder for `Star`/`Polygon`.
pub(super) fn polygon_ring_path(points: &[egui::Pos2]) -> Option<tiny_skia::Path> {
    if points.len() < 3 {
        return None;
    }
    let mut pb = PathBuilder::new();
    pb.move_to(points[0].x, points[0].y);
    for p in &points[1..] {
        pb.line_to(p.x, p.y);
    }
    pb.close();
    pb.finish()
}

pub(super) fn fill_rect(pixmap: &mut Pixmap, rect: egui::Rect, color: Color) {
    let Some(sk_rect) = to_sk_rect(rect) else {
        return;
    };
    let mut paint = Paint::default();
    paint.set_color(color);
    paint.anti_alias = true;
    pixmap.fill_rect(sk_rect, &paint, Transform::identity(), None);
}

pub(super) fn to_sk_rect(rect: egui::Rect) -> Option<SkRect> {
    SkRect::from_ltrb(rect.min.x, rect.min.y, rect.max.x, rect.max.y)
}
