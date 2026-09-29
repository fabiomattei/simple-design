//! The actual command implementations, operating on a plain `Document` —
//! shared verbatim between the live server (`app/ipc_dispatch.rs`, which
//! wraps a call here with a `History` snapshot for undo) and the CLI's
//! headless fallback (`bin/simple-design-cli.rs`, which loads a `Document`
//! from disk, calls this, and saves it back).
//!
//! `undo`/`redo` (need a live `History`) and `save` (needs a file path, not
//! just a `Document`) aren't here — they're handled one level up, since
//! neither fits "pure function of a `Document`".

use egui::{Color32, Pos2, Vec2};
use serde_json::json;
use uuid::Uuid;

use crate::alignment::{self, AlignEdge};
use crate::boolean_ops;
use crate::grouping;
use crate::image_ops;
use crate::model::text_runs::{RunStyle, TextRun};
use crate::model::{
    CornerRadii, Document, Frame, Layer, LayerKind, ListType, Page, Paint, Stroke, Style, TextAlign, TextFont, TextResize,
    TextTransform, VerticalAlign,
};
use crate::transform_ops::{self, FlipAxis};

use super::protocol::{
    AddImageArgs, AddShapeArgs, AddTextArgs, AlignArgs, BooleanArgs, DeleteLayerArgs, ExportPngArgs, FindLayerArgs, FlipArgs, GetLayerArgs,
    GroupArgs, ListLayersArgs, NewPageArgs, RenameLayerArgs, RenamePageArgs, RotateCopiesArgs, SetFrameArgs, UngroupArgs,
};

/// Ops that mutate `doc` — the CLI's headless mode only needs to re-save
/// the file after one of these; the rest are read-only queries or reach
/// outside the document (`export_png`).
pub fn mutates(op: &str) -> bool {
    matches!(
        op,
        "add_rect"
            | "add_ellipse"
            | "add_text"
            | "add_image"
            | "set_frame"
            | "delete_layer"
            | "boolean"
            | "align"
            | "new_page"
            | "rename_page"
            | "rename_layer"
            | "group"
            | "ungroup"
            | "flip"
            | "rotate_copies"
    )
}

pub fn apply(doc: &mut Document, op: &str, args: serde_json::Value) -> Result<serde_json::Value, String> {
    fn parse<T: serde::de::DeserializeOwned>(args: serde_json::Value) -> Result<T, String> {
        serde_json::from_value(args).map_err(|err| format!("invalid args: {err}"))
    }

    match op {
        "ping" => Ok(json!({ "pong": true })),

        "get_document" => serde_json::to_value(&*doc).map_err(|err| err.to_string()),

        "list_pages" => {
            let pages: Vec<_> = doc.pages.iter().map(|p| json!({ "id": p.id, "name": p.name })).collect();
            Ok(json!(pages))
        }

        "list_layers" => {
            let args: ListLayersArgs = parse(args)?;
            let page = resolve_page(doc, args.page)?;
            let mut out = Vec::new();
            if args.recursive {
                collect_layers(&page.layers, 0, &mut out);
            } else {
                out.extend(page.layers.iter().map(|l| layer_summary(l, 0)));
            }
            Ok(json!(out))
        }

        "get_layer" => {
            let args: GetLayerArgs = parse(args)?;
            let layer = doc.find(args.id).ok_or_else(|| format!("layer not found: {}", args.id))?;
            serde_json::to_value(layer).map_err(|err| err.to_string())
        }

        // See `FindLayerArgs`'s doc comment: a `get_document`/`get_layer`
        // alternative for the common case of knowing a layer's name, not its
        // id — searches the whole tree, not just the page's top level, and
        // returns the same lightweight summary `list_layers` does instead of
        // each match's full JSON.
        "find_layer" => {
            let args: FindLayerArgs = parse(args)?;
            let page = resolve_page(doc, args.page)?;
            let query = args.query.to_lowercase();
            let mut out = Vec::new();
            find_layers(&page.layers, &query, 0, &mut out);
            Ok(json!(out))
        }

        "add_rect" => {
            let args: AddShapeArgs = parse(args)?;
            let name = args.name.clone().unwrap_or_else(|| "Rectangle".to_string());
            let radius = args.corner_radius.map(CornerRadii::uniform).unwrap_or(CornerRadii::ZERO);
            let mut layer = Layer::new(name, args.frame, LayerKind::Rectangle { corner_radius: radius });
            apply_shape_style(&mut layer, &args);
            let id = layer.id;
            resolve_page_mut(doc, args.page)?.layers.push(layer);
            Ok(json!({ "id": id }))
        }

        "add_ellipse" => {
            let args: AddShapeArgs = parse(args)?;
            let name = args.name.clone().unwrap_or_else(|| "Ellipse".to_string());
            let mut layer = Layer::new(name, args.frame, LayerKind::Oval);
            apply_shape_style(&mut layer, &args);
            let id = layer.id;
            resolve_page_mut(doc, args.page)?.layers.push(layer);
            Ok(json!({ "id": id }))
        }

        // Mirrors `Tool::Text`'s own defaults (`canvas.rs::new_layer_for_tool`)
        // except `resize`, which is always `Fixed` here — a CLI-given frame is
        // always the caller's explicit choice, unlike a plain click in the GUI
        // (which has no drag to size a box from, so falls back to `Auto`).
        "add_text" => {
            let args: AddTextArgs = parse(args)?;
            let name = args.name.unwrap_or_else(|| "Text".to_string());
            let font = args.font.unwrap_or(TextFont::Proportional);
            let font_size = args.font_size.unwrap_or(24.0);
            let fill = args.fill.unwrap_or(Color32::BLACK);
            let bold = args.bold || args.true_bold;
            // See `AddTextArgs::true_bold`'s doc comment: one run spanning
            // the whole content, mirroring every other scalar field here, is
            // what actually gets a real bold typeface instead of the faux
            // one a plain scalar `bold` falls back to.
            let runs = if args.true_bold {
                vec![TextRun {
                    len: args.content.chars().count(),
                    style: RunStyle { font: font.clone(), font_size, color: Some(fill), bold: true, italic: false, underline: false, strikethrough: false },
                }]
            } else {
                Vec::new()
            };
            let mut layer = Layer::new(
                name,
                args.frame,
                LayerKind::Text {
                    content: args.content,
                    font_size,
                    font,
                    align: args.align.unwrap_or(TextAlign::Left),
                    vertical_align: args.vertical_align.unwrap_or(VerticalAlign::Top),
                    resize: TextResize::Fixed,
                    line_height: None,
                    letter_spacing: 0.0,
                    paragraph_spacing: 0.0,
                    bold,
                    italic: false,
                    underline: false,
                    strikethrough: false,
                    transform: TextTransform::None,
                    list: ListType::None,
                    list_start: 1,
                    style_id: None,
                    runs,
                    path_attachment: None,
                },
            );
            layer.style = Style { fill: Some(Paint::Solid(fill)), stroke: None, ..Default::default() };
            let id = layer.id;
            resolve_page_mut(doc, args.page)?.layers.push(layer);
            Ok(json!({ "id": id }))
        }

        // Reads and re-encodes the file (same PNG-always convention
        // `LayerKind::Image::encoded` uses everywhere — see `image_ops::decode`),
        // so the CLI needs local filesystem access to `args.path` — true both in
        // headless mode and live (the CLI and the running GUI instance it's
        // driving share one filesystem in the intended one-machine workflow —
        // see `CLAUDE.md`'s "CLI / IPC" section).
        "add_image" => {
            let args: AddImageArgs = parse(args)?;
            let path = std::path::Path::new(&args.path);
            let bytes = std::fs::read(path).map_err(|err| format!("failed to read {}: {err}", args.path))?;
            let decoded = image_ops::decode(&bytes).ok_or_else(|| format!("unrecognized image format: {}", args.path))?;
            let (natural_w, natural_h) = (decoded.width() as f32, decoded.height() as f32);
            let size = Vec2::new(args.w.unwrap_or(natural_w), args.h.unwrap_or(natural_h));
            let encoded = image_ops::encode_png(&decoded);
            let name = args.name.unwrap_or_else(|| image_ops::layer_name_for(path));
            let layer = Layer::new_image(
                name,
                Frame { pos: Pos2::new(args.x, args.y), size, rotation: 0.0 },
                encoded,
                decoded.width(),
                decoded.height(),
            );
            let id = layer.id;
            resolve_page_mut(doc, args.page)?.layers.push(layer);
            Ok(json!({ "id": id, "width": decoded.width(), "height": decoded.height() }))
        }

        "set_frame" => {
            let args: SetFrameArgs = parse(args)?;
            let layer = doc.find_mut(args.id).ok_or_else(|| format!("layer not found: {}", args.id))?;
            layer.frame = args.frame;
            Ok(json!({ "id": args.id }))
        }

        "delete_layer" => {
            let args: DeleteLayerArgs = parse(args)?;
            doc.remove(args.id).ok_or_else(|| format!("layer not found: {}", args.id))?;
            Ok(json!({ "id": args.id }))
        }

        "boolean" => {
            let args: BooleanArgs = parse(args)?;
            let page = resolve_page_mut(doc, args.page)?;
            let id = boolean_ops::apply_boolean_op(page, &args.ids, args.op).ok_or("boolean op needs at least 2 layers")?;
            Ok(json!({ "id": id }))
        }

        "align" => {
            let args: AlignArgs = parse(args)?;
            let edge = parse_align_edge(&args.edge)?;
            let page = resolve_page_mut(doc, args.page)?;

            // With `to`, the anchor's own bounds become the alignment target
            // and it's folded into the id list passed to `alignment::align`
            // (which needs >= 2 siblings even with an explicit target) — its
            // own delta then computes to zero, so it doesn't move.
            let align_to = match args.to {
                Some(anchor) => {
                    Some(page.find(anchor).ok_or_else(|| format!("layer not found: {anchor}"))?.frame.rotated_bounds())
                }
                None => None,
            };
            let mut ids = args.ids.clone();
            if let Some(anchor) = args.to {
                if !ids.contains(&anchor) {
                    ids.push(anchor);
                }
            }

            alignment::align(page, &ids, edge, align_to);
            Ok(json!({ "ids": args.ids }))
        }

        "export_png" => {
            let args: ExportPngArgs = parse(args)?;
            let layer = doc.find(args.id).ok_or_else(|| format!("layer not found: {}", args.id))?;
            crate::export::export_layer_to_png(layer, std::path::Path::new(&args.path)).map_err(|err| err.to_string())?;
            Ok(json!({ "path": args.path }))
        }

        "new_page" => {
            let args: NewPageArgs = parse(args)?;
            let index = doc.add_page(args.name);
            Ok(json!({ "id": doc.pages[index].id }))
        }

        "rename_page" => {
            let args: RenamePageArgs = parse(args)?;
            let page = doc.pages.iter_mut().find(|p| p.id == args.id).ok_or_else(|| format!("page not found: {}", args.id))?;
            page.name = args.name;
            Ok(json!({ "id": args.id }))
        }

        "rename_layer" => {
            let args: RenameLayerArgs = parse(args)?;
            let layer = doc.find_mut(args.id).ok_or_else(|| format!("layer not found: {}", args.id))?;
            layer.name = args.name;
            Ok(json!({ "id": args.id }))
        }

        "group" => {
            let args: GroupArgs = parse(args)?;
            let page = resolve_page_mut(doc, args.page)?;
            let id = grouping::group_layers(page, &args.ids).ok_or("group needs at least one layer")?;
            Ok(json!({ "id": id }))
        }

        "ungroup" => {
            let args: UngroupArgs = parse(args)?;
            let page = resolve_page_mut(doc, args.page)?;
            let ids = grouping::ungroup(page, args.id);
            Ok(json!({ "ids": ids }))
        }

        "flip" => {
            let args: FlipArgs = parse(args)?;
            let axis = parse_flip_axis(&args.axis)?;
            let page = resolve_page_mut(doc, args.page)?;
            transform_ops::flip_selection(page, &args.ids, axis);
            Ok(json!({ "ids": args.ids }))
        }

        "rotate_copies" => {
            let args: RotateCopiesArgs = parse(args)?;
            let page = resolve_page_mut(doc, args.page)?;
            let ids = transform_ops::rotate_copies(page, &args.ids, args.count, args.total_degrees);
            Ok(json!({ "ids": ids }))
        }

        other => Err(format!("unknown op: {other}")),
    }
}

fn parse_align_edge(edge: &str) -> Result<AlignEdge, String> {
    match edge {
        "left" => Ok(AlignEdge::Left),
        "hcenter" => Ok(AlignEdge::HCenter),
        "right" => Ok(AlignEdge::Right),
        "top" => Ok(AlignEdge::Top),
        "vcenter" => Ok(AlignEdge::VCenter),
        "bottom" => Ok(AlignEdge::Bottom),
        other => Err(format!("invalid edge: {other} (expected left/hcenter/right/top/vcenter/bottom)")),
    }
}

fn parse_flip_axis(axis: &str) -> Result<FlipAxis, String> {
    match axis {
        "horizontal" => Ok(FlipAxis::Horizontal),
        "vertical" => Ok(FlipAxis::Vertical),
        other => Err(format!("invalid axis: {other} (expected horizontal/vertical)")),
    }
}

/// Applies `AddShapeArgs`'s optional style overrides on top of whatever
/// `Layer::new` seeded (`Style::default()` — flat mid-gray fill, 1px dark
/// stroke), leaving either alone when the caller didn't ask for a change.
fn apply_shape_style(layer: &mut Layer, args: &AddShapeArgs) {
    if let Some(fill) = args.fill {
        layer.style.fill = Some(Paint::Solid(fill));
    }
    if args.no_fill {
        layer.style.fill = None;
    }
    match (args.stroke, args.stroke_width) {
        (Some(color), width) => {
            let width = width.unwrap_or_else(|| layer.style.stroke.as_ref().map_or(1.0, |s| s.width));
            layer.style.stroke = Some(Stroke { paint: Paint::Solid(color), width });
        }
        (None, Some(width)) => {
            if let Some(stroke) = layer.style.stroke.as_mut() {
                stroke.width = width;
            }
        }
        (None, None) => {}
    }
    if args.no_stroke {
        layer.style.stroke = None;
    }
}

fn resolve_page(doc: &Document, page: Option<Uuid>) -> Result<&Page, String> {
    match page {
        Some(id) => doc.pages.iter().find(|p| p.id == id).ok_or_else(|| format!("page not found: {id}")),
        None => Ok(doc.active_page()),
    }
}

fn resolve_page_mut(doc: &mut Document, page: Option<Uuid>) -> Result<&mut Page, String> {
    match page {
        Some(id) => doc.pages.iter_mut().find(|p| p.id == id).ok_or_else(|| format!("page not found: {id}")),
        None => Ok(doc.active_page_mut()),
    }
}

fn layer_summary(layer: &Layer, depth: usize) -> serde_json::Value {
    json!({
        "id": layer.id,
        "name": layer.name,
        "kind": layer_kind_name(&layer.kind),
        "frame": layer.frame,
        "depth": depth,
    })
}

/// Flattens `layers` and every descendant (via `LayerKind::children`) into
/// `out`, depth-first, each as a `layer_summary` — the recursive backbone of
/// both `list_layers { recursive: true }` and `find_layer`.
fn collect_layers(layers: &[Layer], depth: usize, out: &mut Vec<serde_json::Value>) {
    for layer in layers {
        out.push(layer_summary(layer, depth));
        if let Some(children) = layer.kind.children() {
            collect_layers(children, depth + 1, out);
        }
    }
}

/// Same walk as `collect_layers`, but only keeps layers whose name
/// case-insensitively contains `query` (already lowercased by the caller).
fn find_layers(layers: &[Layer], query: &str, depth: usize, out: &mut Vec<serde_json::Value>) {
    for layer in layers {
        if layer.name.to_lowercase().contains(query) {
            out.push(layer_summary(layer, depth));
        }
        if let Some(children) = layer.kind.children() {
            find_layers(children, query, depth + 1, out);
        }
    }
}

fn layer_kind_name(kind: &LayerKind) -> &'static str {
    match kind {
        LayerKind::Artboard { .. } => "artboard",
        LayerKind::Group { .. } => "group",
        LayerKind::Rectangle { .. } => "rectangle",
        LayerKind::Oval => "oval",
        LayerKind::Line => "line",
        LayerKind::Star { .. } => "star",
        LayerKind::Polygon { .. } => "polygon",
        LayerKind::Arrow { .. } => "arrow",
        LayerKind::Path { .. } => "path",
        LayerKind::CompoundPath { .. } => "compound_path",
        LayerKind::BooleanGroup { .. } => "boolean_group",
        LayerKind::Text { .. } => "text",
        LayerKind::Image { .. } => "image",
    }
}

#[cfg(test)]
mod tests {
    use egui::{Pos2, Vec2};
    use serde_json::json;

    use super::*;
    use crate::model::Frame;

    fn frame(x: f32, y: f32, w: f32, h: f32) -> Frame {
        Frame { pos: Pos2::new(x, y), size: Vec2::new(w, h), rotation: 0.0 }
    }

    fn add_rect(doc: &mut Document, x: f32, y: f32, w: f32, h: f32) -> Uuid {
        let args = json!({ "page": null, "frame": frame(x, y, w, h), "name": null });
        let result = apply(doc, "add_rect", args).expect("add_rect should succeed");
        serde_json::from_value(result["id"].clone()).unwrap()
    }

    #[test]
    fn add_rect_with_no_fill_args_keeps_the_default_gray_fill_and_stroke() {
        let mut doc = Document::new();
        let id = add_rect(&mut doc, 0.0, 0.0, 10.0, 10.0);
        let layer = doc.find(id).unwrap();
        assert_eq!(layer.style.fill, Some(Paint::Solid(Color32::from_rgb(216, 216, 216))));
        assert!(layer.style.stroke.is_some());
    }

    #[test]
    fn add_rect_honors_fill_no_stroke_and_corner_radius() {
        let mut doc = Document::new();
        let args = json!({
            "page": null,
            "frame": frame(0.0, 0.0, 10.0, 10.0),
            "name": null,
            "fill": [239, 85, 43, 255],
            "no_stroke": true,
            "corner_radius": 8.0,
        });
        let result = apply(&mut doc, "add_rect", args).expect("add_rect should succeed");
        let id: Uuid = serde_json::from_value(result["id"].clone()).unwrap();
        let layer = doc.find(id).unwrap();
        assert_eq!(layer.style.fill, Some(Paint::Solid(Color32::from_rgb(239, 85, 43))));
        assert!(layer.style.stroke.is_none());
        assert_eq!(layer.kind, LayerKind::Rectangle { corner_radius: CornerRadii::uniform(8.0) });
    }

    #[test]
    fn add_rect_no_fill_drops_the_default_gray_fill_for_an_outline_only_shape() {
        let mut doc = Document::new();
        let args = json!({
            "page": null,
            "frame": frame(0.0, 0.0, 10.0, 10.0),
            "name": null,
            "no_fill": true,
            "stroke": [255, 255, 255, 255],
        });
        let result = apply(&mut doc, "add_rect", args).expect("add_rect should succeed");
        let id: Uuid = serde_json::from_value(result["id"].clone()).unwrap();
        let layer = doc.find(id).unwrap();
        assert_eq!(layer.style.fill, None);
        assert_eq!(layer.style.stroke.as_ref().unwrap().paint, Paint::Solid(Color32::WHITE));
    }

    #[test]
    fn add_rect_stroke_recolors_without_a_fill() {
        let mut doc = Document::new();
        let args = json!({
            "page": null,
            "frame": frame(0.0, 0.0, 10.0, 10.0),
            "name": null,
            "stroke": [255, 255, 255, 255],
            "stroke_width": 2.0,
        });
        let result = apply(&mut doc, "add_rect", args).expect("add_rect should succeed");
        let id: Uuid = serde_json::from_value(result["id"].clone()).unwrap();
        let layer = doc.find(id).unwrap();
        assert_eq!(layer.style.fill, Some(Paint::Solid(Color32::from_rgb(216, 216, 216))), "fill untouched when only stroke is given");
        let stroke = layer.style.stroke.as_ref().expect("stroke should be set");
        assert_eq!(stroke.paint, Paint::Solid(Color32::WHITE));
        assert_eq!(stroke.width, 2.0);
    }

    #[test]
    fn add_ellipse_honors_fill_and_no_stroke() {
        let mut doc = Document::new();
        let args = json!({
            "page": null,
            "frame": frame(0.0, 0.0, 10.0, 10.0),
            "name": null,
            "fill": [0, 0, 0, 255],
            "no_stroke": true,
        });
        let result = apply(&mut doc, "add_ellipse", args).expect("add_ellipse should succeed");
        let id: Uuid = serde_json::from_value(result["id"].clone()).unwrap();
        let layer = doc.find(id).unwrap();
        assert_eq!(layer.style.fill, Some(Paint::Solid(Color32::BLACK)));
        assert!(layer.style.stroke.is_none());
    }

    #[test]
    fn ping_reports_pong() {
        let mut doc = Document::new();
        assert_eq!(apply(&mut doc, "ping", json!({})).unwrap(), json!({ "pong": true }));
    }

    #[test]
    fn get_document_round_trips_through_the_same_json_shape_as_save_to() {
        let mut doc = Document::new();
        add_rect(&mut doc, 0.0, 0.0, 10.0, 10.0);
        let via_op = apply(&mut doc, "get_document", json!({})).unwrap();
        let via_serde = serde_json::to_value(&doc).unwrap();
        assert_eq!(via_op, via_serde);
    }

    #[test]
    fn unknown_op_is_an_error_naming_it() {
        let mut doc = Document::new();
        let err = apply(&mut doc, "frobnicate", json!({})).unwrap_err();
        assert!(err.contains("frobnicate"), "error should name the unknown op: {err}");
    }

    #[test]
    fn add_rect_with_no_page_lands_on_the_active_page() {
        let mut doc = Document::new();
        let id = add_rect(&mut doc, 10.0, 20.0, 30.0, 40.0);
        let layer = doc.active_page().find(id).expect("layer should be on the active page");
        assert_eq!(layer.frame, frame(10.0, 20.0, 30.0, 40.0));
    }

    #[test]
    fn add_text_stores_content_and_falls_back_to_defaults() {
        let mut doc = Document::new();
        let args = json!({ "page": null, "frame": frame(0.0, 0.0, 100.0, 20.0), "content": "Hello", "name": null });
        let result = apply(&mut doc, "add_text", args).expect("add_text should succeed");
        let id: Uuid = serde_json::from_value(result["id"].clone()).unwrap();
        let layer = doc.find(id).expect("layer should exist");
        let LayerKind::Text { content, font_size, font, align, bold, resize, .. } = &layer.kind else {
            panic!("expected a Text layer");
        };
        assert_eq!(content, "Hello");
        assert_eq!(*font_size, 24.0);
        assert_eq!(*font, TextFont::Proportional);
        assert_eq!(*align, TextAlign::Left);
        assert!(!bold);
        assert_eq!(*resize, TextResize::Fixed, "a CLI-given frame is always explicit, so it shouldn't auto-resize");
    }

    #[test]
    fn add_text_honors_explicit_font_size_font_align_and_bold() {
        let mut doc = Document::new();
        let args = json!({
            "page": null,
            "frame": frame(0.0, 0.0, 100.0, 20.0),
            "content": "Hi",
            "font_size": 46.0,
            "bold": true,
            "font": "Display",
            "align": "Center",
            "name": null,
        });
        let result = apply(&mut doc, "add_text", args).expect("add_text should succeed");
        let id: Uuid = serde_json::from_value(result["id"].clone()).unwrap();
        let layer = doc.find(id).unwrap();
        let LayerKind::Text { font_size, font, align, bold, .. } = &layer.kind else {
            panic!("expected a Text layer");
        };
        assert_eq!(*font_size, 46.0);
        assert_eq!(*font, TextFont::Display);
        assert_eq!(*align, TextAlign::Center);
        assert!(bold);
    }

    #[test]
    fn add_text_defaults_to_black_fill_and_honors_an_explicit_one() {
        let mut doc = Document::new();
        let default_id = add_text_helper(&mut doc, "Hi", None);
        assert_eq!(doc.find(default_id).unwrap().style.fill, Some(Paint::Solid(Color32::BLACK)));

        let orange_id = add_text_helper(&mut doc, "Hi", Some([239, 85, 43, 255]));
        assert_eq!(doc.find(orange_id).unwrap().style.fill, Some(Paint::Solid(Color32::from_rgb(239, 85, 43))));
    }

    fn add_text_helper(doc: &mut Document, content: &str, fill: Option<[u8; 4]>) -> Uuid {
        let args = json!({ "page": null, "frame": frame(0.0, 0.0, 100.0, 20.0), "content": content, "fill": fill, "name": null });
        let result = apply(doc, "add_text", args).expect("add_text should succeed");
        serde_json::from_value(result["id"].clone()).unwrap()
    }

    #[test]
    fn add_text_with_plain_bold_leaves_runs_empty() {
        let mut doc = Document::new();
        let args = json!({ "page": null, "frame": frame(0.0, 0.0, 100.0, 20.0), "content": "Hi", "bold": true, "name": null });
        let result = apply(&mut doc, "add_text", args).expect("add_text should succeed");
        let id: Uuid = serde_json::from_value(result["id"].clone()).unwrap();
        let LayerKind::Text { bold, runs, .. } = &doc.find(id).unwrap().kind else {
            panic!("expected a Text layer");
        };
        assert!(bold);
        assert!(runs.is_empty(), "plain bold should stay the faux-bold (no runs) path");
    }

    #[test]
    fn add_text_with_true_bold_populates_a_matching_run_and_implies_bold() {
        let mut doc = Document::new();
        let args = json!({
            "page": null,
            "frame": frame(0.0, 0.0, 100.0, 20.0),
            "content": "Hi!",
            "true_bold": true,
            "font_size": 46.0,
            "font": "Display",
            "fill": [239, 85, 43, 255],
            "name": null,
        });
        let result = apply(&mut doc, "add_text", args).expect("add_text should succeed");
        let id: Uuid = serde_json::from_value(result["id"].clone()).unwrap();
        let LayerKind::Text { bold, runs, .. } = &doc.find(id).unwrap().kind else {
            panic!("expected a Text layer");
        };
        assert!(bold, "true_bold should imply the scalar bold field too");
        assert_eq!(runs.len(), 1);
        let run = &runs[0];
        assert_eq!(run.len, "Hi!".chars().count());
        assert!(run.style.bold);
        assert_eq!(run.style.font, TextFont::Display);
        assert_eq!(run.style.font_size, 46.0);
        assert_eq!(run.style.color, Some(Color32::from_rgb(239, 85, 43)));
    }

    #[test]
    fn add_image_reads_and_reencodes_a_file_at_its_natural_size_by_default() {
        let mut doc = Document::new();
        let img = image::RgbaImage::from_pixel(4, 3, image::Rgba([10, 20, 30, 255]));
        let path = std::env::temp_dir().join(format!("simple-design-cli-test-{}.png", Uuid::new_v4()));
        img.save(&path).expect("writing the fixture PNG should succeed");

        let args = json!({ "page": null, "path": path.to_string_lossy(), "x": 5.0, "y": 6.0, "name": null });
        let result = apply(&mut doc, "add_image", args).expect("add_image should succeed");
        std::fs::remove_file(&path).ok();

        let id: Uuid = serde_json::from_value(result["id"].clone()).unwrap();
        let layer = doc.find(id).expect("layer should exist");
        assert_eq!(layer.frame.pos, Pos2::new(5.0, 6.0));
        assert_eq!(layer.frame.size, Vec2::new(4.0, 3.0));
        let LayerKind::Image { width, height, .. } = &layer.kind else {
            panic!("expected an Image layer");
        };
        assert_eq!(*width, 4);
        assert_eq!(*height, 3);
    }

    #[test]
    fn add_image_with_explicit_w_h_overrides_the_natural_size() {
        let mut doc = Document::new();
        let img = image::RgbaImage::from_pixel(4, 3, image::Rgba([10, 20, 30, 255]));
        let path = std::env::temp_dir().join(format!("simple-design-cli-test-{}.png", Uuid::new_v4()));
        img.save(&path).expect("writing the fixture PNG should succeed");

        let args = json!({ "page": null, "path": path.to_string_lossy(), "x": 0.0, "y": 0.0, "w": 40.0, "h": 30.0, "name": null });
        let result = apply(&mut doc, "add_image", args).expect("add_image should succeed");
        std::fs::remove_file(&path).ok();

        let id: Uuid = serde_json::from_value(result["id"].clone()).unwrap();
        let layer = doc.find(id).unwrap();
        assert_eq!(layer.frame.size, Vec2::new(40.0, 30.0), "the requested size should stretch the displayed frame");
        let LayerKind::Image { width, height, .. } = &layer.kind else {
            panic!("expected an Image layer");
        };
        assert_eq!((*width, *height), (4, 3), "the source pixel dimensions themselves are untouched");
    }

    #[test]
    fn add_image_errors_naming_a_missing_file() {
        let mut doc = Document::new();
        let err = apply(&mut doc, "add_image", json!({ "page": null, "path": "/no/such/file.png", "x": 0.0, "y": 0.0, "name": null }))
            .unwrap_err();
        assert!(err.contains("/no/such/file.png"));
    }

    #[test]
    fn list_layers_with_an_unknown_page_id_is_an_error() {
        let mut doc = Document::new();
        let bogus_page = Uuid::new_v4();
        let err = apply(&mut doc, "list_layers", json!({ "page": bogus_page })).unwrap_err();
        assert!(err.contains(&bogus_page.to_string()));
    }

    #[test]
    fn list_layers_recursive_flattens_a_group_and_marks_depth() {
        let mut doc = Document::new();
        let a = add_rect(&mut doc, 0.0, 0.0, 10.0, 10.0);
        let b = add_rect(&mut doc, 20.0, 0.0, 10.0, 10.0);
        let group_result = apply(&mut doc, "group", json!({ "ids": [a, b] })).expect("group should succeed");
        let group_id: Uuid = serde_json::from_value(group_result["id"].clone()).unwrap();

        let shallow = apply(&mut doc, "list_layers", json!({ "recursive": false })).unwrap();
        assert_eq!(shallow.as_array().unwrap().len(), 1, "shallow listing should only see the group, not its children");

        let deep = apply(&mut doc, "list_layers", json!({ "recursive": true })).unwrap();
        let deep = deep.as_array().unwrap();
        assert_eq!(deep.len(), 3, "recursive listing should include the group and both children");
        assert_eq!(deep[0]["id"], json!(group_id));
        assert_eq!(deep[0]["depth"], json!(0));
        assert_eq!(deep[1]["depth"], json!(1));
        assert_eq!(deep[2]["depth"], json!(1));
    }

    #[test]
    fn find_layer_matches_by_case_insensitive_substring_anywhere_in_the_tree() {
        let mut doc = Document::new();
        let args = json!({ "page": null, "frame": frame(0.0, 0.0, 10.0, 10.0), "name": "Hero Caption" });
        let id = apply(&mut doc, "add_rect", args).unwrap()["id"].clone();
        let id: Uuid = serde_json::from_value(id).unwrap();
        let sibling = add_rect(&mut doc, 20.0, 0.0, 10.0, 10.0);
        apply(&mut doc, "group", json!({ "ids": [id, sibling] })).expect("group should succeed");

        let result = apply(&mut doc, "find_layer", json!({ "query": "hero" })).unwrap();
        let matches = result.as_array().unwrap();
        assert_eq!(matches.len(), 1, "should find the nested layer by a lowercase substring of its name");
        assert_eq!(matches[0]["id"], json!(id));
        assert_eq!(matches[0]["depth"], json!(1), "the match is one level inside the new group");
    }

    #[test]
    fn find_layer_with_no_matches_is_an_empty_list_not_an_error() {
        let mut doc = Document::new();
        add_rect(&mut doc, 0.0, 0.0, 10.0, 10.0);
        let result = apply(&mut doc, "find_layer", json!({ "query": "nonexistent" })).unwrap();
        assert_eq!(result.as_array().unwrap().len(), 0);
    }

    #[test]
    fn set_frame_updates_an_existing_layer_and_errors_on_an_unknown_id() {
        let mut doc = Document::new();
        let id = add_rect(&mut doc, 0.0, 0.0, 10.0, 10.0);
        apply(&mut doc, "set_frame", json!({ "id": id, "frame": frame(5.0, 5.0, 20.0, 20.0) })).expect("set_frame should succeed");
        assert_eq!(doc.find(id).unwrap().frame, frame(5.0, 5.0, 20.0, 20.0));

        let err = apply(&mut doc, "set_frame", json!({ "id": Uuid::new_v4(), "frame": frame(0.0, 0.0, 1.0, 1.0) })).unwrap_err();
        assert!(err.contains("not found"));
    }

    #[test]
    fn delete_layer_removes_it_and_a_second_delete_errors() {
        let mut doc = Document::new();
        let id = add_rect(&mut doc, 0.0, 0.0, 10.0, 10.0);
        apply(&mut doc, "delete_layer", json!({ "id": id })).expect("first delete should succeed");
        assert!(doc.find(id).is_none());
        assert!(apply(&mut doc, "delete_layer", json!({ "id": id })).is_err());
    }

    #[test]
    fn boolean_op_needs_at_least_two_layers() {
        let mut doc = Document::new();
        let id = add_rect(&mut doc, 0.0, 0.0, 10.0, 10.0);
        let err = apply(&mut doc, "boolean", json!({ "ids": [id], "op": "Union" })).unwrap_err();
        assert!(err.contains("at least 2"));
    }

    #[test]
    fn boolean_union_of_two_rects_consumes_the_operands_into_one_new_layer() {
        let mut doc = Document::new();
        let a = add_rect(&mut doc, 0.0, 0.0, 20.0, 20.0);
        let b = add_rect(&mut doc, 10.0, 10.0, 20.0, 20.0);
        let result = apply(&mut doc, "boolean", json!({ "ids": [a, b], "op": "Union" })).expect("union should succeed");
        let new_id: Uuid = serde_json::from_value(result["id"].clone()).unwrap();
        assert!(doc.find(new_id).is_some());
        assert!(doc.find(a).is_none(), "operands should be consumed");
        assert!(doc.find(b).is_none());
    }

    #[test]
    fn align_rejects_an_unrecognized_edge_string() {
        let mut doc = Document::new();
        let id = add_rect(&mut doc, 0.0, 0.0, 10.0, 10.0);
        let err = apply(&mut doc, "align", json!({ "ids": [id], "edge": "diagonal" })).unwrap_err();
        assert!(err.contains("diagonal"));
    }

    #[test]
    fn align_with_a_reference_layer_moves_only_the_other_layer() {
        let mut doc = Document::new();
        let card = add_rect(&mut doc, 0.0, 0.0, 100.0, 100.0);
        let badge = add_rect(&mut doc, 200.0, 5.0, 20.0, 20.0);
        apply(&mut doc, "align", json!({ "ids": [badge], "edge": "hcenter", "to": card })).expect("align should succeed");

        assert_eq!(doc.find(card).unwrap().frame, frame(0.0, 0.0, 100.0, 100.0), "the reference layer should not move");
        let card_center = doc.find(card).unwrap().frame.rotated_bounds().center().x;
        let badge_center = doc.find(badge).unwrap().frame.rotated_bounds().center().x;
        assert_eq!(badge_center, card_center);
    }

    #[test]
    fn align_with_an_unknown_reference_layer_is_an_error() {
        let mut doc = Document::new();
        let id = add_rect(&mut doc, 0.0, 0.0, 10.0, 10.0);
        let bogus = Uuid::new_v4();
        let err = apply(&mut doc, "align", json!({ "ids": [id], "edge": "hcenter", "to": bogus })).unwrap_err();
        assert!(err.contains(&bogus.to_string()));
    }

    #[test]
    fn flip_rejects_an_unrecognized_axis_string() {
        let mut doc = Document::new();
        let id = add_rect(&mut doc, 0.0, 0.0, 10.0, 10.0);
        let err = apply(&mut doc, "flip", json!({ "ids": [id], "axis": "diagonal" })).unwrap_err();
        assert!(err.contains("diagonal"));
    }

    #[test]
    fn flip_horizontal_negates_the_frames_width() {
        let mut doc = Document::new();
        let id = add_rect(&mut doc, 0.0, 0.0, 10.0, 20.0);
        apply(&mut doc, "flip", json!({ "ids": [id], "axis": "horizontal" })).expect("flip should succeed");
        assert_eq!(doc.find(id).unwrap().frame.size.x, -10.0);
    }

    #[test]
    fn group_with_no_ids_is_an_error() {
        let mut doc = Document::new();
        let err = apply(&mut doc, "group", json!({ "ids": [] })).unwrap_err();
        assert!(err.contains("at least one layer"));
    }

    #[test]
    fn group_then_ungroup_round_trips_back_to_the_original_layers() {
        let mut doc = Document::new();
        let a = add_rect(&mut doc, 0.0, 0.0, 10.0, 10.0);
        let b = add_rect(&mut doc, 20.0, 0.0, 10.0, 10.0);
        let result = apply(&mut doc, "group", json!({ "ids": [a, b] })).expect("group should succeed");
        let group_id: Uuid = serde_json::from_value(result["id"].clone()).unwrap();

        let result = apply(&mut doc, "ungroup", json!({ "id": group_id })).expect("ungroup should succeed");
        let freed_ids: Vec<Uuid> = serde_json::from_value(result["ids"].clone()).unwrap();
        assert_eq!(freed_ids.len(), 2);
        assert!(doc.find(a).is_some());
        assert!(doc.find(b).is_some());
        assert!(doc.find(group_id).is_none());
    }

    #[test]
    fn new_page_appends_a_page_and_switches_the_active_page_to_it() {
        let mut doc = Document::new();
        let pages_before = doc.pages.len();
        let result = apply(&mut doc, "new_page", json!({ "name": "Page 2" })).expect("new_page should succeed");
        let id: Uuid = serde_json::from_value(result["id"].clone()).unwrap();
        assert_eq!(doc.pages.len(), pages_before + 1);
        assert_eq!(doc.active_page().id, id);
        assert_eq!(doc.active_page().name, "Page 2");
    }

    #[test]
    fn rotate_copies_produces_the_requested_number_of_new_layers() {
        let mut doc = Document::new();
        let id = add_rect(&mut doc, 0.0, 0.0, 10.0, 10.0);
        let result = apply(&mut doc, "rotate_copies", json!({ "ids": [id], "count": 3, "total_degrees": 90.0 }))
            .expect("rotate_copies should succeed");
        let new_ids: Vec<Uuid> = serde_json::from_value(result["ids"].clone()).unwrap();
        assert_eq!(new_ids.len(), 3);
    }

    #[test]
    fn mutates_distinguishes_writes_from_queries() {
        assert!(mutates("add_rect"));
        assert!(mutates("add_text"));
        assert!(mutates("add_image"));
        assert!(mutates("delete_layer"));
        assert!(!mutates("list_layers"));
        assert!(!mutates("ping"));
        assert!(!mutates("export_png"));
    }
}
