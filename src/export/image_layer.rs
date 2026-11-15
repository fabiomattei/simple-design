//! Rasterizes `Image` layers: decode, Color Adjust, then a bilinear
//! scaled (and rotated) draw into the export pixmap, at `render_scale`.

use tiny_skia::{Pixmap, Transform};

use crate::model::ColorAdjust;

/// Rasterizes an `Image` layer into `pixmap`, scaled from its native pixel
/// size to `bounds` (already offset-translated, same as every other shape
/// case above) with bilinear filtering, applying the non-destructive Color
/// Adjust effect first — mirrors what the canvas texture cache does for the
/// live view (`canvas/texture_caches.rs`'s `ImageTextureCache`), so exported PNGs match.
pub(super) fn draw_image(
    pixmap: &mut Pixmap,
    encoded: &[u8],
    color_adjust: ColorAdjust,
    bounds: egui::Rect,
    opacity: f32,
    rotation: f32,
    render_scale: f32,
) {
    let Some(decoded) = crate::image_ops::decode(encoded) else { return };
    let adjusted = crate::image_ops::apply_color_adjust(&decoded, color_adjust);
    let (width, height) = (adjusted.width(), adjusted.height());
    if width == 0 || height == 0 || bounds.width().abs() < 0.01 || bounds.height().abs() < 0.01 {
        return;
    }
    let Some(size) = tiny_skia::IntSize::from_wh(width, height) else { return };
    let premultiplied = crate::image_ops::to_premultiplied_rgba(&adjusted);
    let Some(src) = Pixmap::from_vec(premultiplied, size) else { return };

    let mut transform = Transform::from_translate(bounds.min.x, bounds.min.y)
        .pre_scale(bounds.width() / width as f32, bounds.height() / height as f32);
    if rotation != 0.0 {
        transform = transform.post_rotate_at(rotation, bounds.center().x, bounds.center().y);
    }
    let transform = transform.post_scale(render_scale, render_scale);
    let paint = tiny_skia::PixmapPaint {
        opacity: opacity.clamp(0.0, 1.0),
        blend_mode: tiny_skia::BlendMode::SourceOver,
        quality: tiny_skia::FilterQuality::Bilinear,
    };
    pixmap.draw_pixmap(0, 0, src.as_ref(), &paint, transform, None);
}

#[cfg(test)]
mod tests {
    use crate::export::render_layer;
    use crate::model::{Frame, Layer};
    use egui::Pos2;

    #[test]
    fn renders_image_layer_scaled_to_its_frame() {
        let pixels = image::RgbaImage::from_pixel(2, 2, image::Rgba([0, 200, 0, 255]));
        let encoded = crate::image_ops::encode_png(&pixels);
        let layer = Layer::new_image(
            "Photo",
            Frame::from_two_points(Pos2::new(0.0, 0.0), Pos2::new(40.0, 20.0)),
            encoded,
            2,
            2,
        );

        let pixmap = render_layer(&layer).expect("should render");
        assert_eq!((pixmap.width(), pixmap.height()), (40, 20));
        let center = pixmap.pixel(20, 10).expect("center pixel");
        assert_eq!((center.red(), center.green(), center.blue()), (0, 200, 0));
        assert_eq!(center.alpha(), 255);
    }
}
