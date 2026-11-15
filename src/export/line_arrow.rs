//! Draws `Line` and `Arrow` layers: a round-capped stroke between the frame's
//! two endpoints, plus an arrow's chevron, triangle or disc caps.

use tiny_skia::{FillRule, PathBuilder, Pixmap, Stroke as SkStroke};

use crate::model::{ArrowCap, Layer};

use super::paint::to_sk_paint;
use super::sk_geometry::{arrow_cap_size, arrow_cap_wings, polygon_ring_path, rotation_transform, to_sk_rect};

pub(super) fn draw_line(pixmap: &mut Pixmap, layer: &Layer, offset: egui::Vec2, bounds: egui::Rect, opacity: f32) {
    let Some(stroke) = layer.style.stroke.as_ref() else {
        return;
    };
    let a = layer.frame.start() + offset;
    let b = layer.frame.end() + offset;
    let mut pb = PathBuilder::new();
    pb.move_to(a.x, a.y);
    pb.line_to(b.x, b.y);
    let Some(path) = pb.finish() else {
        return;
    };
    let mut noise_storage = None;
    let paint = to_sk_paint(&stroke.paint, bounds, opacity * layer.style.stroke_opacity, &mut noise_storage);
    let sk_stroke = SkStroke {
        width: stroke.width.max(0.5),
        line_cap: tiny_skia::LineCap::Round,
        ..Default::default()
    };
    pixmap.stroke_path(&path, &paint, &sk_stroke, rotation_transform(layer, bounds), None);
}

pub(super) fn draw_arrow(
    pixmap: &mut Pixmap,
    layer: &Layer,
    offset: egui::Vec2,
    bounds: egui::Rect,
    opacity: f32,
    start_cap: &ArrowCap,
    end_cap: &ArrowCap,
) {
    let Some(stroke) = layer.style.stroke.as_ref() else {
        return;
    };
    let a = layer.frame.start() + offset;
    let b = layer.frame.end() + offset;
    let transform = rotation_transform(layer, bounds);
    let sk_stroke = SkStroke {
        width: stroke.width.max(0.5),
        line_cap: tiny_skia::LineCap::Round,
        ..Default::default()
    };
    let mut noise_storage = None;
    let paint = to_sk_paint(&stroke.paint, bounds, opacity * layer.style.stroke_opacity, &mut noise_storage);

    // Shaft + any `Line`-style chevron caps share one stroked path.
    let mut pb = PathBuilder::new();
    pb.move_to(a.x, a.y);
    pb.line_to(b.x, b.y);
    for (tip, outward, cap) in [(a, a - b, *start_cap), (b, b - a, *end_cap)] {
        if cap == crate::model::ArrowCap::Line
            && let Some((p1, p2)) = arrow_cap_wings(tip, outward, stroke.width) {
                pb.move_to(p1.x, p1.y);
                pb.line_to(tip.x, tip.y);
                pb.line_to(p2.x, p2.y);
            }
    }
    if let Some(path) = pb.finish() {
        pixmap.stroke_path(&path, &paint, &sk_stroke, transform, None);
    }

    // Filled caps (Triangle/Disc) as separate fill paths, same color.
    for (tip, outward, cap) in [(a, a - b, *start_cap), (b, b - a, *end_cap)] {
        let fill_path = match cap {
            crate::model::ArrowCap::Triangle => arrow_cap_wings(tip, outward, stroke.width)
                .and_then(|(p1, p2)| polygon_ring_path(&[tip, p1, p2])),
            crate::model::ArrowCap::Disc => {
                let size = arrow_cap_size(stroke.width);
                let dir = if outward.length() > 1e-3 { outward.normalized() } else { egui::Vec2::ZERO };
                let center = tip - dir * size * 0.4;
                let radius = size * 0.4;
                to_sk_rect(egui::Rect::from_center_size(center, egui::Vec2::splat(radius * 2.0))).and_then(
                    |r| {
                        let mut pb = PathBuilder::new();
                        pb.push_oval(r);
                        pb.finish()
                    },
                )
            }
            crate::model::ArrowCap::None | crate::model::ArrowCap::Line => None,
        };
        if let Some(path) = fill_path {
            pixmap.fill_path(&path, &paint, FillRule::Winding, transform, None);
        }
    }
}
