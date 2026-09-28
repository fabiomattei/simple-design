//! Wire types for the socket protocol between a running GUI instance and
//! `simple-design-cli`. Newline-delimited JSON, one `Request` per line in,
//! one `Response` per line back.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::model::{BoolOp, Frame};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Request {
    pub id: u64,
    pub op: String,
    #[serde(default)]
    pub args: serde_json::Value,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Response {
    pub id: u64,
    pub ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl Response {
    pub fn ok(id: u64, result: serde_json::Value) -> Self {
        Self { id, ok: true, result: Some(result), error: None }
    }

    pub fn err(id: u64, error: impl Into<String>) -> Self {
        Self { id, ok: false, result: None, error: Some(error.into()) }
    }
}

// Each args struct derives both directions: the server (`app::ipc_dispatch`,
// or the CLI's own headless fallback) deserializes it out of a `Request`,
// and the CLI's live path serializes one straight from typed clap args
// instead of hand-assembling `serde_json::json!` — one field layout, used
// both ways.

#[derive(Serialize, Deserialize)]
pub struct ListLayersArgs {
    pub page: Option<Uuid>,
}

#[derive(Serialize, Deserialize)]
pub struct GetLayerArgs {
    pub id: Uuid,
}

#[derive(Serialize, Deserialize)]
pub struct AddShapeArgs {
    #[serde(default)]
    pub page: Option<Uuid>,
    pub frame: Frame,
    pub name: Option<String>,
}

#[derive(Serialize, Deserialize)]
pub struct SetFrameArgs {
    pub id: Uuid,
    pub frame: Frame,
}

#[derive(Serialize, Deserialize)]
pub struct DeleteLayerArgs {
    pub id: Uuid,
}

#[derive(Serialize, Deserialize)]
pub struct BooleanArgs {
    #[serde(default)]
    pub page: Option<Uuid>,
    pub ids: Vec<Uuid>,
    pub op: BoolOp,
}

#[derive(Serialize, Deserialize)]
pub struct AlignArgs {
    #[serde(default)]
    pub page: Option<Uuid>,
    pub ids: Vec<Uuid>,
    /// One of "left" / "hcenter" / "right" / "top" / "vcenter" / "bottom" —
    /// kept as a plain string here rather than pulling `alignment::AlignEdge`
    /// (which has no `serde` derive of its own) into the wire protocol.
    pub edge: String,
}

#[derive(Serialize, Deserialize)]
pub struct ExportPngArgs {
    pub id: Uuid,
    pub path: String,
}

#[derive(Serialize, Deserialize)]
pub struct SaveArgs {
    #[serde(default)]
    pub path: Option<String>,
}
