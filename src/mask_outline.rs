//! `MaskMode::Outline` (see `model::MaskMode`): turns a mask layer into a
//! copy whose rendered alpha is exactly its geometry, so
//! `masking::composite_masked_run` can clip to the shape regardless of the
//! mask's own fill, stroke, opacity or shadows.

use std::borrow::Cow;

use egui::Color32;

use crate::model::{CornerRadii, Layer, LayerKind, MaskMode, Paint};

/// The layer to rasterize for `mask`'s clipping alpha: `mask` itself in
/// `MaskMode::Alpha`, or an outline copy (`to_outline`) in `MaskMode::Outline`.
pub fn mask_silhouette(mask: &Layer) -> Cow<'_, Layer> {
    match mask.mask.mode {
        MaskMode::Alpha => Cow::Borrowed(mask),
        MaskMode::Outline => {
            let mut outline = mask.clone();
            to_outline(&mut outline);
            Cow::Owned(outline)
        }
    }
}

/// Restyles `layer` and every descendant to draw as solid opaque white
/// geometry: white fill at full opacity, no shadows, and no stroke —
/// except on a `Line`/`Arrow`, whose stroke *is* its shape (kept, made
/// white). An `Image` becomes a plain rectangle of its frame, an
/// `Artboard` gets a white background, and text runs lose their colors.
fn to_outline(layer: &mut Layer) {
    layer.opacity = 1.0;
    layer.style.fill = Some(Paint::Solid(Color32::WHITE));
    layer.style.fill_opacity = 1.0;
    layer.style.shadows.clear();
    layer.style.inner_shadows.clear();
    if matches!(layer.kind, LayerKind::Line | LayerKind::Arrow { .. }) {
        if let Some(stroke) = layer.style.stroke.as_mut() {
            stroke.paint = Paint::Solid(Color32::WHITE);
        }
        layer.style.stroke_opacity = 1.0;
    } else {
        layer.style.stroke = None;
    }
    if matches!(layer.kind, LayerKind::Image { .. }) {
        layer.kind = LayerKind::Rectangle { corner_radius: CornerRadii::ZERO };
    }
    match &mut layer.kind {
        LayerKind::Artboard { background, .. } => *background = Color32::WHITE,
        LayerKind::Text { runs, .. } => runs.iter_mut().for_each(|run| run.style.color = None),
        _ => {}
    }
    if let Some(children) = layer.kind.children_mut() {
        children.iter_mut().for_each(to_outline);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Frame, Stroke};
    use egui::{Pos2, Vec2};

    #[test]
    fn alpha_mode_uses_the_mask_unchanged() {
        let mut mask = Layer::new("Mask", Frame { pos: Pos2::ZERO, size: Vec2::splat(10.0), rotation: 0.0 }, LayerKind::Oval);
        mask.opacity = 0.3;
        assert!(matches!(mask_silhouette(&mask), Cow::Borrowed(_)));
    }

    #[test]
    fn outline_mode_draws_the_shape_opaque_with_no_stroke_or_transparency() {
        let mut mask = Layer::new("Mask", Frame { pos: Pos2::ZERO, size: Vec2::splat(10.0), rotation: 0.0 }, LayerKind::Oval);
        mask.mask.mode = MaskMode::Outline;
        mask.opacity = 0.3;
        mask.style.fill = None;
        mask.style.stroke = Some(Stroke { paint: Paint::Solid(Color32::RED), width: 4.0 });

        let outline = mask_silhouette(&mask);
        assert_eq!(outline.opacity, 1.0);
        assert_eq!(outline.style.fill, Some(Paint::Solid(Color32::WHITE)));
        assert!(outline.style.stroke.is_none());
    }

    #[test]
    fn outline_mode_keeps_a_lines_stroke_since_it_is_the_shape() {
        let mut line = Layer::new("Line", Frame::from_two_points(Pos2::ZERO, Pos2::new(10.0, 0.0)), LayerKind::Line);
        line.mask.mode = MaskMode::Outline;
        line.style.stroke = Some(Stroke { paint: Paint::Solid(Color32::RED), width: 4.0 });

        let outline = mask_silhouette(&line);
        assert_eq!(outline.style.stroke.as_ref().map(|s| &s.paint), Some(&Paint::Solid(Color32::WHITE)));
    }
}
