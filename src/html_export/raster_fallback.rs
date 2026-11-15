//! Decides which layers HTML export can describe with CSS alone, and
//! renders the rest to PNG through `export/` so nothing is dropped.

use egui::{Rect, Vec2};
use tiny_skia::Pixmap;

use crate::model::{Layer, LayerKind, Paint};

/// Pixmap pixels per document unit for rasterized layers, so they stay
/// sharp on high-density screens.
const RASTER_SCALE: f32 = 2.0;

/// A rasterized layer: PNG bytes plus where they go, in the layer's
/// parent-relative document space.
pub struct RasterImage {
    pub png: Vec<u8>,
    pub bounds: Rect,
}

/// Whether `layer` exports as a styled element (`css_rules`) rather than a
/// rasterized image. Containers only qualify if no child is a mask, since
/// CSS can't reproduce masking.
pub fn is_css_expressible(layer: &Layer) -> bool {
    match &layer.kind {
        LayerKind::Artboard { children, .. } | LayerKind::Group { children } => !children.iter().any(|child| child.mask.enabled),
        LayerKind::Rectangle { .. } | LayerKind::Oval | LayerKind::Line | LayerKind::Arrow { .. } => {
            !layer.style.fill.as_ref().is_some_and(is_texture_paint)
        }
        LayerKind::Text { path_attachment, .. } => path_attachment.is_none(),
        LayerKind::Image { color_adjust, .. } => color_adjust.is_identity(),
        LayerKind::Star { .. } | LayerKind::Polygon { .. } | LayerKind::Path { .. } | LayerKind::CompoundPath { .. } | LayerKind::BooleanGroup { .. } => false,
    }
}

fn is_texture_paint(paint: &Paint) -> bool {
    matches!(paint, Paint::Noise(_) | Paint::Halftone(_) | Paint::Pattern(_))
}

/// Renders `layer` (with its drop shadows and its own opacity) to PNG.
/// `roots` is the page's top-level layers, so path-attached text resolves
/// its target the same way `export::draw_layer` does on canvas.
///
/// Mirrors `export::render_layer_scaled`'s sizing (rotated bounds padded by
/// `shadow::outer_shadow_extent`), duplicated here because that function
/// can't take `roots`. Path-attached text instead goes through
/// `export::render_layer_plain`, the one entry point that sizes to the
/// attached glyphs — it draws no shadows, which is acceptable for that case.
pub fn rasterize(layer: &Layer, roots: &[Layer]) -> anyhow::Result<RasterImage> {
    let (pixmap, bounds) = if matches!(layer.kind, LayerKind::Text { path_attachment: Some(_), .. }) {
        crate::export::render_layer_plain(layer, roots, RASTER_SCALE).ok_or_else(|| no_area(layer))?
    } else {
        let pad = crate::shadow::outer_shadow_extent(&layer.style.shadows);
        let bounds = layer.frame.rotated_bounds().expand(pad);
        let width = (bounds.width() * RASTER_SCALE).round().max(1.0) as u32;
        let height = (bounds.height() * RASTER_SCALE).round().max(1.0) as u32;
        let mut pixmap = Pixmap::new(width, height).ok_or_else(|| no_area(layer))?;
        let offset = Vec2::new(-bounds.min.x, -bounds.min.y);
        crate::export::draw_layer_with_shadows(&mut pixmap, layer, roots, offset, 1.0, RASTER_SCALE);
        (pixmap, bounds)
    };
    Ok(RasterImage { png: pixmap.encode_png()?, bounds })
}

fn no_area(layer: &Layer) -> anyhow::Error {
    anyhow::anyhow!("layer \"{}\" has no visible area to render", layer.name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{CornerRadii, Frame, MaskSettings, NoiseFill};
    use egui::{Color32, Pos2};

    fn rect(name: &str) -> Layer {
        Layer::new(name, Frame { pos: Pos2::new(5.0, 5.0), size: Vec2::new(40.0, 20.0), rotation: 0.0 }, LayerKind::Rectangle { corner_radius: CornerRadii::ZERO })
    }

    #[test]
    fn solid_rectangle_is_css_but_noise_rectangle_is_not() {
        let mut layer = rect("R");
        assert!(is_css_expressible(&layer));
        layer.style.fill = Some(Paint::Noise(NoiseFill { base: Color32::GRAY, intensity: 0.5, scale: 2.0, seed: 1 }));
        assert!(!is_css_expressible(&layer));
    }

    #[test]
    fn group_with_a_mask_child_is_rasterized() {
        let mut mask = rect("Mask");
        mask.mask = MaskSettings { enabled: true, ..MaskSettings::default() };
        let group = Layer::new("G", Frame::from_bounds(Rect::from_min_size(Pos2::ZERO, Vec2::splat(50.0))), LayerKind::Group { children: vec![mask, rect("Content")] });
        assert!(!is_css_expressible(&group));
    }

    #[test]
    fn rasterized_star_is_a_png_at_twice_its_size() {
        let star = Layer::new("S", Frame { pos: Pos2::new(10.0, 10.0), size: Vec2::new(30.0, 30.0), rotation: 0.0 }, LayerKind::Star { points: 5, inner_ratio: 0.5 });
        let image = rasterize(&star, std::slice::from_ref(&star)).unwrap();
        let decoded = image::load_from_memory(&image.png).unwrap();
        assert_eq!(image.bounds, Rect::from_min_size(Pos2::new(10.0, 10.0), Vec2::splat(30.0)));
        assert_eq!((decoded.width(), decoded.height()), (60, 60));
    }
}
