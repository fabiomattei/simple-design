//! Aseprite-style "Navigator": a small scaled-down overview of every
//! top-level layer on the active page, with the canvas's current viewport
//! drawn as a draggable outline on top. Click or drag inside it to re-center
//! the canvas (`CanvasWidget::pan`) on that point.
//!
//! Rendering here is deliberately approximate, not a second full renderer:
//! every shape collapses to its rotated bounding box (true outlines only for
//! `Oval`/`Star`/`Polygon`, since `shapes.rs` already has cheap generators
//! for those) filled with `Paint::to_color32`'s flattened representative
//! color — gradients/noise/halftone/patterns/shadows/text glyphs are all
//! irrelevant at thumbnail scale. Same "top-level layers only" bounds
//! convention already accepted for marquee-select/snapping (see CLAUDE.md's
//! "Known simplifications").

use egui::{Color32, Pos2, Rect, Sense, Stroke as EguiStroke, Vec2};

use crate::model::{Layer, LayerKind, Page};
use crate::shapes;

const THUMB_HEIGHT: f32 = 160.0;
const VIEWPORT_COLOR: Color32 = Color32::from_rgb(64, 148, 255);

/// Draws the minimap and, if the user clicked/dragged inside it, updates
/// `pan` so the canvas re-centers on that doc-space point. `zoom` and
/// `viewport_size` (`CanvasWidget::last_canvas_size`) describe the on-screen
/// canvas's current doc-space footprint (`viewport_size / zoom`).
pub fn ui(ui: &mut egui::Ui, page: &Page, pan: &mut Vec2, zoom: f32, viewport_size: Vec2) {
    ui.heading("Minimap");
    ui.separator();

    let (response, painter) = ui.allocate_painter(Vec2::new(ui.available_width(), THUMB_HEIGHT), Sense::click_and_drag());
    let panel_rect = response.rect;
    painter.rect_filled(panel_rect, 4.0, Color32::from_gray(230));

    let Some(mut bounds) = content_bounds(&page.layers) else {
        painter.text(
            panel_rect.center(),
            egui::Align2::CENTER_CENTER,
            "Empty page",
            egui::FontId::default(),
            Color32::GRAY,
        );
        return;
    };
    // Also make sure the current viewport itself is never cropped out of the
    // thumbnail, even when it's panned somewhere past the content's own
    // bounds.
    let viewport_doc = Rect::from_min_size((-*pan / zoom).to_pos2(), viewport_size / zoom.max(0.0001));
    bounds = bounds.union(viewport_doc);
    let margin = bounds.size().max(Vec2::splat(1.0)) * 0.05;
    bounds = bounds.expand2(margin);

    let scale = (panel_rect.width() / bounds.width()).min(panel_rect.height() / bounds.height());
    let thumb_origin = panel_rect.center() - bounds.size() * scale / 2.0;
    let to_thumb = |p: Pos2| thumb_origin + (p - bounds.min) * scale;

    let clipped = painter.with_clip_rect(panel_rect);
    for layer in &page.layers {
        draw_layer(&clipped, layer, Vec2::ZERO, &to_thumb);
    }

    let vp_screen = Rect::from_min_max(to_thumb(viewport_doc.min), to_thumb(viewport_doc.max));
    clipped.rect_stroke(vp_screen, 0.0, EguiStroke::new(1.5, VIEWPORT_COLOR), egui::StrokeKind::Outside);

    if (response.clicked() || response.dragged()) && scale > 0.0 {
        if let Some(pos) = response.interact_pointer_pos() {
            let doc_pos = bounds.min + (pos - thumb_origin) / scale;
            *pan = viewport_size / 2.0 - doc_pos.to_vec2() * zoom;
        }
    }
}

/// Union of every visible top-level layer's rotated on-screen footprint, in
/// page/doc space.
fn content_bounds(layers: &[Layer]) -> Option<Rect> {
    layers.iter().filter(|l| l.visible).map(|l| l.frame.rotated_bounds()).reduce(|a, b| a.union(b))
}

fn draw_layer(painter: &egui::Painter, layer: &Layer, offset: Vec2, to_thumb: &impl Fn(Pos2) -> Pos2) {
    if !layer.visible {
        return;
    }
    let bounds = layer.frame.bounds().translate(offset);
    let child_offset = offset + layer.frame.pos.to_vec2();

    match &layer.kind {
        LayerKind::Artboard { children, background } => {
            let r = Rect::from_min_max(to_thumb(bounds.min), to_thumb(bounds.max));
            painter.rect_filled(r, 0.0, *background);
            for child in children {
                draw_layer(painter, child, child_offset, to_thumb);
            }
        }
        LayerKind::Group { children } | LayerKind::BooleanGroup { children } => {
            for child in children {
                draw_layer(painter, child, child_offset, to_thumb);
            }
        }
        LayerKind::Line | LayerKind::Arrow { .. } => {
            let a = to_thumb(layer.frame.start() + offset);
            let b = to_thumb(layer.frame.end() + offset);
            painter.line_segment([a, b], EguiStroke::new(1.0, fill_color(layer)));
        }
        LayerKind::Oval => {
            let center = bounds.center();
            let points: Vec<Pos2> = shapes::ellipse_points(center, bounds.width() / 2.0, bounds.height() / 2.0)
                .into_iter()
                .map(|p| to_thumb(shapes::rotate_point(p, center, layer.frame.rotation)))
                .collect();
            painter.add(egui::Shape::convex_polygon(points, fill_color(layer), EguiStroke::NONE));
        }
        LayerKind::Star { points, inner_ratio } => {
            let center = bounds.center();
            let pts: Vec<Pos2> = shapes::star_points(center, bounds.width() / 2.0, bounds.height() / 2.0, *points, *inner_ratio)
                .into_iter()
                .map(|p| to_thumb(shapes::rotate_point(p, center, layer.frame.rotation)))
                .collect();
            painter.add(egui::Shape::convex_polygon(pts, fill_color(layer), EguiStroke::NONE));
        }
        LayerKind::Polygon { sides } => {
            let center = bounds.center();
            let pts: Vec<Pos2> = shapes::polygon_points(center, bounds.width() / 2.0, bounds.height() / 2.0, *sides)
                .into_iter()
                .map(|p| to_thumb(shapes::rotate_point(p, center, layer.frame.rotation)))
                .collect();
            painter.add(egui::Shape::convex_polygon(pts, fill_color(layer), EguiStroke::NONE));
        }
        // Rectangle/Path/CompoundPath/Text/Image: rotated bounding box —
        // enough at thumbnail scale, and avoids re-deriving path/glyph
        // geometry here.
        _ => {
            let corners = shapes::rotated_corners(bounds, layer.frame.rotation);
            let pts: Vec<Pos2> = corners.iter().map(|&p| to_thumb(p)).collect();
            painter.add(egui::Shape::convex_polygon(pts, fill_color(layer), EguiStroke::NONE));
        }
    }
}

fn fill_color(layer: &Layer) -> Color32 {
    layer.style.fill.as_ref().map(|p| p.to_color32()).unwrap_or(Color32::from_gray(170))
}
