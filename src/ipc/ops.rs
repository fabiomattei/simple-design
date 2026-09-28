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
