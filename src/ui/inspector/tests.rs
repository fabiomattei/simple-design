//! Headless rendering tests for `inspector::ui`: each one draws the panel
//! for a given selection in a real `egui::Context` and checks which section
//! headings appear, and in what order. They don't click individual controls;
//! they guard against a section going missing, being duplicated, moving, or
//! panicking when `ui()` is restructured.

use egui::{Color32, Pos2, Rect, Vec2};

use super::ui;
use crate::canvas::CanvasWidget;
use crate::history::History;
use crate::model::text_runs::{RunStyle, TextRun};
use crate::model::{
    ArrowCap, CornerRadii, Document, Frame, Layer, LayerId, LayerKind, ListType, Paint, PathPoint, PointType,
    TextAlign, TextFont, TextResize, TextTransform, VerticalAlign,
};
use crate::tools::Tool;

/// Tall enough that every section of the longest panel (a `Text` layer) is
/// laid out on screen — widgets skip painting when they fall outside it.
const SCREEN_SIZE: Vec2 = Vec2::new(400.0, 8000.0);

/// Every label the tests treat as a section heading. Anything else the panel
/// paints (field values, button captions, hints) is ignored.
const HEADINGS: &[&str] = &[
    "No selection",
    "New Artboard",
    "Screen",
    "Scale Layers",
    "Finish",
    "Tidy",
    "Group (Cmd+G)",
    "Combine",
    "Text",
    "Image",
    "X",
    "Y",
    "W",
    "H",
    "Rotate",
    "Flip",
    "Closed",
    "Corner radius",
    "Points: ",
    "Sides: ",
    "Start:",
    "End:",
    "Font size: ",
    "Font:",
    "Style:",
    "Align:",
    "Vertical:",
    "Resize:",
    "Transform:",
    "List:",
    "Text Style",
    "Path / Area",
    "Color Adjust",
    "Opacity",
    "Layer Style",
    "Fills",
    "Borders",
    "Shadows",
    "Inner Shadows",
    "Ungroup (Cmd+Shift+G)",
];

const TRANSFORM: &[&str] = &["X", "Y", "W", "H", "Rotate"];
const APPEARANCE: &[&str] = &["Opacity", "Layer Style", "Fills", "Borders", "Shadows", "Inner Shadows"];
const TYPOGRAPHY: &[&str] = &[
    "Text",
    "Font size: ",
    "Font:",
    "Style:",
    "Align:",
    "Vertical:",
    "Resize:",
    "Transform:",
    "List:",
    "Text Style",
    "Path / Area",
];

/// Renders the inspector for `selection` and returns the section headings
/// it painted, in paint order.
fn headings(layers: Vec<Layer>, selection: &[LayerId], canvas: &mut CanvasWidget, tool: Tool) -> Vec<String> {
    let mut doc = Document::new();
    doc.active_page_mut().layers.extend(layers);
    let mut history = History::new(doc);
    let ctx = egui::Context::default();
    crate::fonts::install_fonts(&ctx);
    let input = || egui::RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, SCREEN_SIZE)), ..Default::default() };
    // Warm-up frame: loads the installed fonts before any text is laid out.
    ctx.run_ui(input(), |_| {}).drop_without_applying_deltas();
    let mut output = ctx.run_ui(input(), |root| {
        ui(root, &mut history, selection, canvas, tool);
    });
    let mut texts = Vec::new();
    for clipped in &output.shapes {
        collect_texts(&clipped.shape, &mut texts);
    }
    output.textures_delta.clear();
    texts.retain(|t| HEADINGS.contains(&t.as_str()));
    texts
}

fn collect_texts(shape: &egui::Shape, out: &mut Vec<String>) {
    match shape {
        egui::Shape::Text(text) => out.push(text.galley.text().to_string()),
        egui::Shape::Vec(shapes) => shapes.iter().for_each(|s| collect_texts(s, out)),
        _ => {}
    }
}

fn headings_for_single(layer: Layer) -> Vec<String> {
    let id = layer.id;
    headings(vec![layer], &[id], &mut CanvasWidget::default(), Tool::Select)
}

fn concat(parts: &[&[&str]]) -> Vec<String> {
    parts.iter().flat_map(|p| p.iter().map(|s| s.to_string())).collect()
}

fn layer(kind: LayerKind) -> Layer {
    Layer::new("Subject", Frame { pos: Pos2::new(10.0, 10.0), size: Vec2::new(100.0, 60.0), rotation: 0.0 }, kind)
}

fn straight_point(x: f32, y: f32) -> PathPoint {
    PathPoint { anchor: Pos2::new(x, y), handle_in: None, handle_out: None, point_type: PointType::Straight, corner_radius: 0.0 }
}

/// A `Text` layer with the Text tool's own defaults; non-empty `runs` makes
/// it a rich-text layer.
fn text_layer(runs: Vec<TextRun>) -> Layer {
    let mut text = layer(LayerKind::Text {
        content: "Hello".to_string(),
        font_size: 24.0,
        font: TextFont::Proportional,
        align: TextAlign::Left,
        vertical_align: VerticalAlign::Top,
        resize: TextResize::Fixed,
        line_height: None,
        letter_spacing: 0.0,
        paragraph_spacing: 0.0,
        bold: false,
        italic: false,
        underline: false,
        strikethrough: false,
        transform: TextTransform::None,
        list: ListType::None,
        list_start: 1,
        runs,
        path_attachment: None,
        style_id: None,
    });
    text.style.fill = Some(Paint::Solid(Color32::BLACK));
    text
}

#[test]
fn each_shape_kind_shows_its_own_fields_between_transform_and_appearance() {
    let path = LayerKind::Path {
        points: vec![straight_point(0.0, 0.0), straight_point(50.0, 0.0), straight_point(50.0, 50.0)],
        closed: true,
    };
    let cases: Vec<(&str, LayerKind, &[&str])> = vec![
        ("Rectangle", LayerKind::Rectangle { corner_radius: CornerRadii::ZERO }, &["Corner radius"]),
        ("Oval", LayerKind::Oval, &[]),
        ("Star", LayerKind::Star { points: 5, inner_ratio: 0.5 }, &["Points: "]),
        ("Polygon", LayerKind::Polygon { sides: 6 }, &["Sides: "]),
        ("Line", LayerKind::Line, &[]),
        ("Arrow", LayerKind::Arrow { start_cap: ArrowCap::None, end_cap: ArrowCap::Triangle }, &["Start:", "End:"]),
        ("Path", path, &["Closed"]),
    ];
    for (name, kind, own) in cases {
        assert_eq!(headings_for_single(layer(kind)), concat(&[TRANSFORM, own, APPEARANCE]), "{name}");
    }
}

#[test]
fn group_adds_an_ungroup_button_after_appearance() {
    let group = layer(LayerKind::Group { children: vec![layer(LayerKind::Oval)] });
    assert_eq!(headings_for_single(group), concat(&[TRANSFORM, APPEARANCE, &["Ungroup (Cmd+Shift+G)"]]));
}

#[test]
fn artboard_has_no_rotation_field() {
    let artboard = layer(LayerKind::Artboard { children: Vec::new(), background: Color32::WHITE, adjust_content_on_resize: false });
    assert_eq!(headings_for_single(artboard), concat(&[&["X", "Y", "W", "H"], APPEARANCE]));
}

#[test]
fn text_layer_shows_typography_text_style_and_path_sections() {
    assert_eq!(headings_for_single(text_layer(Vec::new())), concat(&[&["Text"], TRANSFORM, TYPOGRAPHY, APPEARANCE]));
}

#[test]
fn rich_text_layer_shows_the_same_sections_as_uniform_text() {
    let run = TextRun {
        len: 5,
        style: RunStyle {
            font: TextFont::Proportional,
            font_size: 24.0,
            color: Some(Color32::RED),
            bold: true,
            italic: false,
            underline: false,
            strikethrough: false,
        },
    };
    assert_eq!(headings_for_single(text_layer(vec![run])), concat(&[&["Text"], TRANSFORM, TYPOGRAPHY, APPEARANCE]));
}

#[test]
fn image_layer_shows_image_and_color_adjust_sections() {
    let pixels = image::RgbaImage::from_pixel(2, 2, image::Rgba([0, 200, 0, 255]));
    let frame = Frame::from_two_points(Pos2::ZERO, Pos2::new(40.0, 20.0));
    let image = Layer::new_image("Subject", frame, crate::image_ops::encode_png(&pixels), 2, 2);
    assert_eq!(headings_for_single(image), concat(&[&["Image"], TRANSFORM, &["Image", "Color Adjust"], APPEARANCE]));
}

#[test]
fn multi_selection_shows_arrange_strip_tidy_frame_fields_and_combine() {
    let (a, b) = (layer(LayerKind::Oval), layer(LayerKind::Oval));
    let ids = [a.id, b.id];
    let found = headings(vec![a, b], &ids, &mut CanvasWidget::default(), Tool::Select);
    assert_eq!(found, concat(&[&["Tidy", "Tidy", "X", "Y", "W", "H", "Flip", "Group (Cmd+G)", "Combine"]]));
}

#[test]
fn scale_mode_panel_comes_before_the_selection_panel() {
    let mut canvas = CanvasWidget::default();
    canvas.scaling = Some(Vec::new());
    let found = headings(Vec::new(), &[], &mut canvas, Tool::Select);
    assert_eq!(found, concat(&[&["Scale Layers", "Finish", "No selection"]]));
}

#[test]
fn artboard_tool_with_nothing_selected_offers_size_presets() {
    let found = headings(Vec::new(), &[], &mut CanvasWidget::default(), Tool::Artboard);
    assert_eq!(found, concat(&[&["New Artboard", "Screen"]]));
}

#[test]
fn nothing_selected_says_so() {
    let found = headings(Vec::new(), &[], &mut CanvasWidget::default(), Tool::Select);
    assert_eq!(found, concat(&[&["No selection"]]));
}

/// The docked panel's default width (`app/panel_columns.rs`'s `panels_column`).
const PANEL_WIDTH: f32 = 260.0;

/// Renders the inspector for one layer the way `app.rs` docks it — inside a
/// vertical `ScrollArea`, `PANEL_WIDTH` wide — and returns every painted
/// text with the x of its left edge.
fn texts_in_docked_panel(layer: Layer) -> Vec<(String, f32)> {
    let id = layer.id;
    let mut doc = Document::new();
    doc.active_page_mut().layers.push(layer);
    let mut history = History::new(doc);
    let mut canvas = CanvasWidget::default();
    let ctx = egui::Context::default();
    crate::fonts::install_fonts(&ctx);
    let input = || egui::RawInput {
        screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(PANEL_WIDTH, SCREEN_SIZE.y))),
        ..Default::default()
    };
    ctx.run_ui(input(), |_| {}).drop_without_applying_deltas();
    let mut output = ctx.run_ui(input(), |root| {
        egui::ScrollArea::vertical().show(root, |ui_in_scroll| {
            ui(ui_in_scroll, &mut history, &[id], &mut canvas, Tool::Select);
        });
    });
    output.textures_delta.clear();
    let mut positioned = Vec::new();
    for clipped in &output.shapes {
        collect_positioned_texts(&clipped.shape, &mut positioned);
    }
    positioned
}

fn collect_positioned_texts(shape: &egui::Shape, out: &mut Vec<(String, f32)>) {
    match shape {
        egui::Shape::Text(text) => out.push((text.galley.text().to_string(), text.pos.x)),
        egui::Shape::Vec(shapes) => shapes.iter().for_each(|s| collect_positioned_texts(s, out)),
        _ => {}
    }
}

/// Regression: an over-wide row (the Layer Style name field at egui's
/// default width) once widened the scroll content past the panel, pushing
/// every section's right-aligned + button out of view — so an unset fill,
/// border or shadow could never be added.
#[test]
fn section_add_buttons_stay_inside_the_docked_panel() {
    let mut bare = layer(LayerKind::Oval);
    bare.style.fill = None;
    bare.style.stroke = None;
    let add_button_xs: Vec<f32> =
        texts_in_docked_panel(bare).into_iter().filter(|(text, _)| text == "+").map(|(_, x)| x).collect();
    assert_eq!(add_button_xs.len(), 4, "Fills, Borders, Shadows and Inner Shadows each paint a +");
    assert!(add_button_xs.iter().all(|&x| x < PANEL_WIDTH), "+ painted outside the panel: {add_button_xs:?}");
}
