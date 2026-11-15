//! Building a new layer for the active tool, inserting it into the right
//! parent, and sizing auto-resizing `Text` layers to their content.

use egui::{Color32, Pos2, Vec2};

use crate::model::{
    ArrowCap, CornerRadii, Frame, Layer, LayerId, LayerKind, Page, TextAlign, TextFont, TextResize, VerticalAlign,
};
use crate::text_layout::{self, TextStyleParams};
use crate::tools::Tool;

/// Default size for a `Text` layer created with a plain click (no drag) of
/// the Text tool, roughly matching one line of the default font size.
const DEFAULT_TEXT_SIZE: Vec2 = Vec2::new(140.0, 34.0);

/// Recomputes a `Text` layer's `frame.size` from its current content/style,
/// for `resize != Fixed` — `Auto` recomputes both width and height (no
/// wrapping); `AutoHeight` wraps at the current width and only recomputes
/// height. A no-op for `Fixed`. Called after every edit that could change a
/// `Text` layer's natural size: creation, Inspector field changes, in-place
/// canvas-edit keystrokes, and shared Text Style application.
pub(crate) fn apply_text_auto_resize(ctx: &egui::Context, layer: &mut Layer) {
    let LayerKind::Text {
        content,
        font_size,
        font,
        align,
        resize,
        line_height,
        letter_spacing,
        paragraph_spacing,
        bold,
        italic,
        underline,
        strikethrough,
        transform,
        list,
        list_start,
        runs,
        ..
    } = &layer.kind
    else {
        return;
    };
    if *resize == TextResize::Fixed {
        return;
    }
    let style = TextStyleParams {
        font: font.clone(),
        font_size: *font_size,
        align: *align,
        letter_spacing: *letter_spacing,
        line_height: *line_height,
        italic: *italic,
        underline: *underline,
        strikethrough: *strikethrough,
        transform: *transform,
        list: *list,
        list_start: *list_start,
    };
    let wrap_width = if *resize == TextResize::AutoHeight {
        layer.frame.size.x.abs()
    } else {
        f32::INFINITY
    };
    let is_auto = *resize == TextResize::Auto;
    // Color never affects glyph metrics, so `Color32::BLACK` is just a
    // placeholder here, same as the uniform-style path below.
    let galleys = if runs.is_empty() {
        text_layout::layout_paragraphs(ctx, content, &style, 1.0, Color32::BLACK, wrap_width)
    } else {
        let base_style = crate::model::text_runs::RunStyle {
            font: font.clone(),
            font_size: *font_size,
            color: layer.style.fill.as_ref().map(crate::model::Paint::to_color32),
            bold: *bold,
            italic: *italic,
            underline: *underline,
            strikethrough: *strikethrough,
        };
        text_layout::layout_paragraphs_rich(ctx, content, runs, &base_style, &style, 1.0, Color32::BLACK, wrap_width)
    };
    let (_, total_size) = text_layout::stack_paragraphs(&galleys, *paragraph_spacing);

    layer.frame.size.y = total_size.y.max(1.0);
    if is_auto {
        layer.frame.size.x = total_size.x.max(1.0);
    }
}

pub(super) fn new_layer_for_tool(tool: Tool, frame: Frame) -> Option<Layer> {
    match tool {
        Tool::Rectangle => Some(Layer::new(
            "Rectangle",
            Frame::from_bounds(frame.bounds()),
            LayerKind::Rectangle { corner_radius: CornerRadii::ZERO },
        )),
        Tool::Oval => Some(Layer::new(
            "Oval",
            Frame::from_bounds(frame.bounds()),
            LayerKind::Oval,
        )),
        Tool::Line => Some(Layer::new("Line", frame, LayerKind::Line)),
        Tool::Arrow => Some(Layer::new(
            "Arrow",
            frame,
            LayerKind::Arrow { start_cap: ArrowCap::None, end_cap: ArrowCap::Triangle },
        )),
        Tool::Star => Some(Layer::new(
            "Star",
            Frame::from_bounds(frame.bounds()),
            LayerKind::Star { points: 5, inner_ratio: 0.5 },
        )),
        Tool::Polygon => Some(Layer::new(
            "Polygon",
            Frame::from_bounds(frame.bounds()),
            LayerKind::Polygon { sides: 6 },
        )),
        Tool::Artboard => Some(Layer::new_artboard(
            "Artboard",
            Frame::from_bounds(frame.bounds()),
        )),
        Tool::Text => {
            let bounds = frame.bounds();
            // A meaningful drag creates a fixed-size wrapping box (a
            // "click and drag" interaction); a plain click creates an auto-sizing layer
            // ("click anywhere") — `apply_text_auto_resize` (called
            // right after insertion, see the `drag_stopped` handler) fits
            // `DEFAULT_TEXT_SIZE` to the actual default content in that case.
            let dragged = bounds.width().abs() > 2.0 || bounds.height().abs() > 2.0;
            let (size, resize) = if dragged {
                (bounds.size(), TextResize::Fixed)
            } else {
                (DEFAULT_TEXT_SIZE, TextResize::Auto)
            };
            let mut layer = Layer::new(
                "Text",
                Frame { pos: bounds.min, size, rotation: 0.0 },
                LayerKind::Text {
                    content: "Text".to_string(),
                    font_size: 24.0,
                    font: TextFont::Proportional,
                    align: TextAlign::Left,
                    vertical_align: VerticalAlign::Top,
                    resize,
                    line_height: None,
                    letter_spacing: 0.0,
                    paragraph_spacing: 0.0,
                    bold: false,
                    italic: false,
                    underline: false,
                    strikethrough: false,
                    transform: crate::model::TextTransform::None,
                    list: crate::model::ListType::None,
                    list_start: 1,
                    style_id: None,
                    runs: Vec::new(),
                    path_attachment: None,
                },
            );
            layer.style =
                crate::model::Style { fill: Some(crate::model::Paint::Solid(Color32::BLACK)), stroke: None, ..Default::default() };
            Some(layer)
        }
        // Pen-drawn paths are built up over several click/drag gestures
        // (see `CanvasWidget::pen`) and committed via `finish_pen_path`,
        // not through the single-drag `CreatingShape` flow other shapes use.
        // Scissors doesn't create a new layer by dragging either — see
        // `CanvasWidget::try_scissor_path`.
        Tool::Select | Tool::Pan | Tool::Pen | Tool::Scissors => None,
    }
}

/// Inserts a newly created layer at the top level of the page, unless
/// `start_doc` falls within an existing top-level artboard, in which case
/// it's nested as a child (with frame made relative to that artboard).
/// Inserts a freshly-created shape/text layer, choosing its parent. `hint`
/// ("a new layer lands in the group a just-deleted layer's delete
/// left behind" — see `App`'s Delete/Backspace handler, the only place that
/// sets `CanvasWidget::insert_hint_parent`) takes priority when present and
/// still a real container, regardless of `start_doc`'s position; otherwise
/// falls back to the original position-based rule (inside the top-level
/// Artboard, if any, that `start_doc` falls within).
pub(super) fn insert_layer(page: &mut Page, mut layer: Layer, start_doc: Pos2, hint: Option<LayerId>) {
    if let Some(hint_id) = hint {
        let hint_abs_pos = page
            .absolute_offset(hint_id)
            .zip(page.find(hint_id))
            .map(|(ancestor_offset, l)| l.frame.pos.to_vec2() + ancestor_offset);
        if let Some(hint_abs_pos) = hint_abs_pos
            && let Some(children) = page.find_mut(hint_id).and_then(|l| l.kind.children_mut()) {
                layer.frame.pos -= hint_abs_pos;
                children.push(layer);
                return;
            }
    }
    for parent in page.layers.iter_mut() {
        if let LayerKind::Artboard { children, .. } = &mut parent.kind
            && parent.frame.bounds().contains(start_doc) {
                layer.frame.pos -= parent.frame.pos.to_vec2();
                children.push(layer);
                return;
            }
    }
    page.layers.push(layer);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect_layer(name: &str, x: f32, y: f32, w: f32, h: f32) -> Layer {
        Layer::new(
            name,
            Frame { pos: Pos2::new(x, y), size: Vec2::new(w, h), rotation: 0.0 },
            LayerKind::Rectangle { corner_radius: CornerRadii::ZERO },
        )
    }

    #[test]
    fn insert_layer_with_hint_lands_inside_the_hinted_group_at_relative_coords() {
        let child = rect_layer("Child", 1.0, 1.0, 2.0, 2.0);
        let group = Layer::new(
            "Group",
            Frame { pos: Pos2::new(50.0, 50.0), size: Vec2::new(20.0, 20.0), rotation: 0.0 },
            LayerKind::Group { children: vec![child] },
        );
        let group_id = group.id;
        let mut page = Page::new("Page 1");
        page.layers.push(group);

        let new_layer = rect_layer("New", 55.0, 55.0, 5.0, 5.0);
        let new_id = new_layer.id;
        // start_doc deliberately outside the group's bounds — the hint
        // should still win over the position-based fallback.
        insert_layer(&mut page, new_layer, Pos2::new(500.0, 500.0), Some(group_id));

        let group = page.find(group_id).unwrap();
        let LayerKind::Group { children } = &group.kind else { unreachable!() };
        let inserted = children.iter().find(|l| l.id == new_id).expect("should be inside the group");
        // Absolute pos (55,55) minus the group's own absolute pos (50,50).
        assert_eq!(inserted.frame.pos, Pos2::new(5.0, 5.0));
    }
}
