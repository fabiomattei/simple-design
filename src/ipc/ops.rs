//! The actual command implementations, operating on a plain `Document` —
//! shared verbatim between the live server (`app/ipc_dispatch.rs`, which
//! wraps a call here with a `History` snapshot for undo) and the CLI's
//! headless fallback (`bin/simple-design-cli.rs`, which loads a `Document`
//! from disk, calls this, and saves it back).
//!
//! `undo`/`redo` (need a live `History`) and `save` (needs a file path, not
//! just a `Document`) aren't here — they're handled one level up, since
//! neither fits "pure function of a `Document`".

use serde_json::json;
use uuid::Uuid;

use crate::alignment::{self, AlignEdge};
use crate::boolean_ops;
use crate::grouping;
use crate::model::{CornerRadii, Document, Layer, LayerKind, Page};
use crate::transform_ops::{self, FlipAxis};

use super::protocol::{
    AddShapeArgs, AlignArgs, BooleanArgs, DeleteLayerArgs, ExportPngArgs, FlipArgs, GetLayerArgs, GroupArgs, ListLayersArgs, NewPageArgs,
    RenameLayerArgs, RenamePageArgs, RotateCopiesArgs, SetFrameArgs, UngroupArgs,
};

/// Ops that mutate `doc` — the CLI's headless mode only needs to re-save
/// the file after one of these; the rest are read-only queries or reach
/// outside the document (`export_png`).
pub fn mutates(op: &str) -> bool {
    matches!(
        op,
        "add_rect"
            | "add_ellipse"
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
            Ok(json!(page.layers.iter().map(layer_summary).collect::<Vec<_>>()))
        }

        "get_layer" => {
            let args: GetLayerArgs = parse(args)?;
            let layer = doc.find(args.id).ok_or_else(|| format!("layer not found: {}", args.id))?;
            serde_json::to_value(layer).map_err(|err| err.to_string())
        }

        "add_rect" => {
            let args: AddShapeArgs = parse(args)?;
            let name = args.name.unwrap_or_else(|| "Rectangle".to_string());
            let layer = Layer::new(name, args.frame, LayerKind::Rectangle { corner_radius: CornerRadii::ZERO });
            let id = layer.id;
            resolve_page_mut(doc, args.page)?.layers.push(layer);
            Ok(json!({ "id": id }))
        }

        "add_ellipse" => {
            let args: AddShapeArgs = parse(args)?;
            let name = args.name.unwrap_or_else(|| "Ellipse".to_string());
            let layer = Layer::new(name, args.frame, LayerKind::Oval);
            let id = layer.id;
            resolve_page_mut(doc, args.page)?.layers.push(layer);
            Ok(json!({ "id": id }))
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
            alignment::align(page, &args.ids, edge, None);
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

fn layer_summary(layer: &Layer) -> serde_json::Value {
    json!({
        "id": layer.id,
        "name": layer.name,
        "kind": layer_kind_name(&layer.kind),
        "frame": layer.frame,
    })
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
    fn list_layers_with_an_unknown_page_id_is_an_error() {
        let mut doc = Document::new();
        let bogus_page = Uuid::new_v4();
        let err = apply(&mut doc, "list_layers", json!({ "page": bogus_page })).unwrap_err();
        assert!(err.contains(&bogus_page.to_string()));
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
        assert!(mutates("delete_layer"));
        assert!(!mutates("list_layers"));
        assert!(!mutates("ping"));
        assert!(!mutates("export_png"));
    }
}
