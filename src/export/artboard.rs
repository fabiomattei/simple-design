//! Artboard rendering for the exporter: background fill plus children
//! clipped to the artboard's own rect, so anything placed outside it is cut.

use tiny_skia::{Pixmap, PixmapPaint, Transform};

use super::paint::to_color_with_opacity;
use super::sk_geometry::fill_rect;

/// Fills `bounds` (document units, already offset into `pixmap` space) with
/// `background`, then draws the artboard's children through `draw_children`
/// into a scratch pixmap cropped to `bounds` and composites it back — the
/// crop is the clip. Artboards never rotate (see `Frame::rotation`), so a
/// pixel-aligned rect crop is exact. `draw_children` receives the scratch
/// pixmap and the children's offset (`child_offset`) shifted into it.
/// Side effect: draws into `pixmap`.
pub(super) fn draw_artboard(
    pixmap: &mut Pixmap,
    bounds: egui::Rect,
    background: egui::Color32,
    opacity: f32,
    child_offset: egui::Vec2,
    render_scale: f32,
    draw_children: impl FnOnce(&mut Pixmap, egui::Vec2),
) {
    fill_rect(pixmap, bounds, to_color_with_opacity(background, opacity), render_scale);

    let x0 = (bounds.min.x * render_scale).round().max(0.0) as i32;
    let y0 = (bounds.min.y * render_scale).round().max(0.0) as i32;
    let x1 = (bounds.max.x * render_scale).round().min(pixmap.width() as f32) as i32;
    let y1 = (bounds.max.y * render_scale).round().min(pixmap.height() as f32) as i32;
    if x1 <= x0 || y1 <= y0 {
        return;
    }
    let Some(mut scratch) = Pixmap::new((x1 - x0) as u32, (y1 - y0) as u32) else { return };
    draw_children(&mut scratch, child_offset - egui::Vec2::new(x0 as f32, y0 as f32) / render_scale);
    pixmap.draw_pixmap(x0, y0, scratch.as_ref(), &PixmapPaint::default(), Transform::identity(), None);
}
