//! What the `set_fill`, `set_shadow` and `add_shape` ops build, kept out of
//! `ops.rs` (whose match arms call these) so that file only dispatches.

use crate::model::{Gradient, GradientKind, GradientStop, Layer, LayerKind, Paint, Shadow};

use super::protocol::{AddFigureArgs, SetFillArgs, SetShadowArgs};

/// The fill `args` describes: one color is solid, more are a gradient with
/// evenly spaced stops. Errors on an empty color list.
pub(super) fn fill_paint(args: &SetFillArgs) -> Result<Paint, String> {
    match args.colors.as_slice() {
        [] => Err("set_fill needs at least one color".to_string()),
        [color] => Ok(Paint::Solid(*color)),
        colors => {
            let last = (colors.len() - 1) as f32;
            let stops = colors.iter().enumerate().map(|(i, &color)| GradientStop { offset: i as f32 / last, color }).collect();
            let base = if args.radial { Gradient::radial(colors[0], colors[1]) } else { Gradient::linear(colors[0], colors[1]) };
            let kind = if args.radial { GradientKind::Radial } else { GradientKind::Linear };
            Ok(Paint::Gradient(Gradient { kind, stops, from: args.from.unwrap_or(base.from), to: args.to.unwrap_or(base.to) }))
        }
    }
}

/// Replaces `layer`'s drop (or inner) shadows with the one `args` describes,
/// or clears them.
pub(super) fn set_shadow(layer: &mut Layer, args: &SetShadowArgs) {
    let shadows = if args.inner { &mut layer.style.inner_shadows } else { &mut layer.style.shadows };
    shadows.clear();
    if args.clear {
        return;
    }
    let default = Shadow::default();
    shadows.push(Shadow {
        color: args.color.unwrap_or(default.color),
        offset: args.offset.unwrap_or(default.offset),
        blur: args.blur.unwrap_or(default.blur),
        spread: args.spread.unwrap_or(default.spread),
    });
}

/// The `LayerKind` and default layer name for `args.kind`; errors naming an
/// unknown kind.
pub(super) fn figure_kind(args: &AddFigureArgs) -> Result<(LayerKind, &'static str), String> {
    match args.kind.as_str() {
        "star" => Ok((LayerKind::Star { points: args.points.unwrap_or(5).max(3), inner_ratio: args.inner_ratio.unwrap_or(0.5) }, "Star")),
        "polygon" => Ok((LayerKind::Polygon { sides: args.sides.unwrap_or(6).max(3) }, "Polygon")),
        "line" => Ok((LayerKind::Line, "Line")),
        other => Err(format!("invalid kind: {other} (expected star/polygon/line)")),
    }
}

#[cfg(test)]
mod tests {
    use egui::{Color32, Pos2, Vec2};
    use serde_json::json;
    use uuid::Uuid;

    use super::super::ops::apply;
    use crate::model::{Document, Frame, GradientKind, LayerKind, Paint};

    fn frame() -> Frame {
        Frame { pos: Pos2::new(0.0, 0.0), size: Vec2::new(10.0, 10.0), rotation: 0.0 }
    }

    fn add_rect(doc: &mut Document) -> Uuid {
        let result = apply(doc, "add_rect", json!({ "frame": frame(), "name": null })).expect("add_rect should succeed");
        serde_json::from_value(result["id"].clone()).unwrap()
    }

    #[test]
    fn set_fill_with_one_color_is_solid_and_with_three_is_an_evenly_stopped_gradient() {
        let mut doc = Document::new();
        let id = add_rect(&mut doc);
        apply(&mut doc, "set_fill", json!({ "id": id, "colors": [[255, 0, 0, 255]] })).expect("set_fill should succeed");
        assert_eq!(doc.find(id).unwrap().style.fill, Some(Paint::Solid(Color32::RED)));

        let args = json!({ "id": id, "colors": [[255, 0, 0, 255], [0, 255, 0, 255], [0, 0, 255, 255]], "radial": true });
        apply(&mut doc, "set_fill", args).expect("set_fill should succeed");
        let Some(Paint::Gradient(gradient)) = &doc.find(id).unwrap().style.fill else { panic!("expected a gradient") };
        assert_eq!(gradient.kind, GradientKind::Radial);
        let offsets: Vec<f32> = gradient.stops.iter().map(|s| s.offset).collect();
        assert_eq!(offsets, vec![0.0, 0.5, 1.0]);

        assert!(apply(&mut doc, "set_fill", json!({ "id": id, "colors": [] })).is_err());
    }

    #[test]
    fn set_shadow_replaces_the_drop_shadow_and_clear_removes_it() {
        let mut doc = Document::new();
        let id = add_rect(&mut doc);
        apply(&mut doc, "set_shadow", json!({ "id": id, "blur": 20.0 })).expect("set_shadow should succeed");
        apply(&mut doc, "set_shadow", json!({ "id": id, "blur": 30.0 })).expect("set_shadow should succeed");
        let shadows = &doc.find(id).unwrap().style.shadows;
        assert_eq!(shadows.len(), 1, "a second set_shadow replaces, not appends");
        assert_eq!(shadows[0].blur, 30.0);
        assert!(doc.find(id).unwrap().style.inner_shadows.is_empty());

        apply(&mut doc, "set_shadow", json!({ "id": id, "clear": true })).expect("set_shadow should succeed");
        assert!(doc.find(id).unwrap().style.shadows.is_empty());
    }

    #[test]
    fn add_shape_builds_a_star_with_the_given_points_and_rejects_an_unknown_kind() {
        let mut doc = Document::new();
        let args = json!({ "frame": frame(), "name": null, "kind": "star", "points": 8, "no_stroke": true });
        let result = apply(&mut doc, "add_shape", args).expect("add_shape should succeed");
        let id: Uuid = serde_json::from_value(result["id"].clone()).unwrap();
        let layer = doc.find(id).unwrap();
        assert_eq!(layer.name, "Star");
        assert_eq!(layer.kind, LayerKind::Star { points: 8, inner_ratio: 0.5 });
        assert!(layer.style.stroke.is_none());

        assert!(apply(&mut doc, "add_shape", json!({ "frame": frame(), "name": null, "kind": "blob" })).is_err());
    }
}
