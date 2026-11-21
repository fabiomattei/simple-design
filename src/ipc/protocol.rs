//! Wire types for the socket protocol between a running GUI instance and
//! `simple-design-cli`. Newline-delimited JSON, one `Request` per line in,
//! one `Response` per line back.

use egui::{Color32, Pos2, Vec2};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::model::{BoolOp, Frame, MaskMode, TextAlign, TextFont, VerticalAlign};

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
    /// Walk into every `Artboard`/`Group`/`BooleanGroup` child instead of
    /// stopping at the page's top-level layers, flattening the whole tree
    /// into one list (each entry's `depth` says how nested it was). Default
    /// `false` keeps the original shallow behavior.
    #[serde(default)]
    pub recursive: bool,
}

#[derive(Serialize, Deserialize)]
pub struct GetLayerArgs {
    pub id: Uuid,
}

/// A lighter-weight alternative to `get_document`/`get_layer` for locating a
/// layer by name instead of id: walks the whole layer tree (like
/// `list_layers` with `recursive: true`) but only returns entries whose name
/// case-insensitively contains `query`, as the same lightweight summary
/// `list_layers` uses (`id`/`name`/`kind`/`frame`/`depth`) rather than each
/// match's full JSON. Built after a session spent repeatedly calling
/// `get_document` (which dumps every field — `style`, `runs`, etc. — for
/// every layer) just to recover one layer's id by name.
#[derive(Serialize, Deserialize)]
pub struct FindLayerArgs {
    #[serde(default)]
    pub page: Option<Uuid>,
    pub query: String,
}

#[derive(Serialize, Deserialize)]
pub struct AddShapeArgs {
    #[serde(default)]
    pub page: Option<Uuid>,
    pub frame: Frame,
    pub name: Option<String>,
    /// Omitted keeps `Style::default()`'s fill (a flat mid-gray).
    #[serde(default)]
    pub fill: Option<Color32>,
    /// Drops any fill (a transparent/outline-only shape). Takes priority
    /// over `fill` if both are given.
    #[serde(default)]
    pub no_fill: bool,
    /// Drops `Style::default()`'s 1px dark stroke — most CLI-authored shapes
    /// (flat design blocks, photo-backed buttons) don't want one. Takes
    /// priority over `stroke`/`stroke_width` if both are given.
    #[serde(default)]
    pub no_stroke: bool,
    /// Recolors the stroke, keeping its current width unless `stroke_width`
    /// is also given. Omitted keeps `Style::default()`'s dark stroke color.
    #[serde(default)]
    pub stroke: Option<Color32>,
    /// Omitted keeps the current stroke width (`Style::default()`'s `1.0` if
    /// this is a fresh shape).
    #[serde(default)]
    pub stroke_width: Option<f32>,
    /// `add_rect` only — ignored for `add_ellipse` (an `Oval` has no corners).
    #[serde(default)]
    pub corner_radius: Option<f32>,
    /// A container (Artboard/Group) to add into instead of the page's top
    /// level; `frame`/position is then relative to it, like every nested
    /// `Layer.frame`. Searched across all pages unless `page` is also given.
    #[serde(default)]
    pub parent: Option<Uuid>,
}

#[derive(Serialize, Deserialize)]
pub struct AddTextArgs {
    #[serde(default)]
    pub page: Option<Uuid>,
    pub frame: Frame,
    pub content: String,
    /// Defaults to `24.0` (the same default `Tool::Text` seeds in the GUI) if omitted.
    #[serde(default)]
    pub font_size: Option<f32>,
    /// Defaults to `TextFont::Proportional` if omitted.
    #[serde(default)]
    pub font: Option<TextFont>,
    #[serde(default)]
    pub bold: bool,
    /// A plain `bold` only gets the weak "faux bold" (offset double-draw)
    /// both `canvas/` and `export/` fall back to when nothing better is
    /// available — a real bold typeface is only baked in via the rich-text
    /// `runs` path (see `fonts::ab_glyph_bytes_bold`). `true_bold` opts a
    /// freshly-created layer into that: one `runs` entry spanning the whole
    /// content, styled to match every other field here, so it renders with
    /// actual bold-weight glyphs while still behaving like plain uniform
    /// text everywhere else (selection, alignment, editing the content).
    /// Implies `bold` regardless of what that field was set to.
    #[serde(default)]
    pub true_bold: bool,
    /// Defaults to `TextAlign::Left` if omitted.
    #[serde(default)]
    pub align: Option<TextAlign>,
    /// Defaults to `VerticalAlign::Top` if omitted.
    #[serde(default)]
    pub vertical_align: Option<VerticalAlign>,
    /// Defaults to black (the same default `Tool::Text` seeds in the GUI) if omitted.
    #[serde(default)]
    pub fill: Option<Color32>,
    pub name: Option<String>,
    /// A container (Artboard/Group) to add into instead of the page's top
    /// level; `frame`/position is then relative to it, like every nested
    /// `Layer.frame`. Searched across all pages unless `page` is also given.
    #[serde(default)]
    pub parent: Option<Uuid>,
}

/// Reads `path` off disk and re-encodes it PNG (same convention
/// `LayerKind::Image::encoded` always uses, see `image_ops::decode`) — `w`/`h`
/// default to the source image's own natural pixel size if omitted, matching
/// how dropping a file onto the canvas inserts it (`image_ops::build_image_grid`).
#[derive(Serialize, Deserialize)]
pub struct AddImageArgs {
    #[serde(default)]
    pub page: Option<Uuid>,
    pub path: String,
    pub x: f32,
    pub y: f32,
    #[serde(default)]
    pub w: Option<f32>,
    #[serde(default)]
    pub h: Option<f32>,
    pub name: Option<String>,
    /// A container (Artboard/Group) to add into instead of the page's top
    /// level; `frame`/position is then relative to it, like every nested
    /// `Layer.frame`. Searched across all pages unless `page` is also given.
    #[serde(default)]
    pub parent: Option<Uuid>,
}

/// A new top-level Artboard. Its size comes from `preset` (a
/// `SCREEN_PRESETS`/`PAPER_PRESETS` name, case-insensitive) and/or `w`/`h`,
/// which override the preset's dimensions individually; with no preset, both
/// `w` and `h` are required. `name` defaults to the preset's name, else
/// "Artboard"; `background` to white, like `Layer::new_artboard`.
#[derive(Serialize, Deserialize)]
pub struct AddArtboardArgs {
    #[serde(default)]
    pub page: Option<Uuid>,
    pub x: f32,
    pub y: f32,
    #[serde(default)]
    pub w: Option<f32>,
    #[serde(default)]
    pub h: Option<f32>,
    #[serde(default)]
    pub preset: Option<String>,
    pub name: Option<String>,
    #[serde(default)]
    pub background: Option<Color32>,
}

#[derive(Serialize, Deserialize)]
pub struct SetFrameArgs {
    pub id: Uuid,
    /// Replaces the whole frame; the fields below then adjust it. Omitted =
    /// start from the layer's current frame, so a request can change just
    /// one of x/y/w/h/rotation.
    #[serde(default)]
    pub frame: Option<Frame>,
    #[serde(default)]
    pub x: Option<f32>,
    #[serde(default)]
    pub y: Option<f32>,
    #[serde(default)]
    pub w: Option<f32>,
    #[serde(default)]
    pub h: Option<f32>,
    #[serde(default)]
    pub rotation: Option<f32>,
    /// Multiplies both w and h (1.15 = 15% bigger), keeping the top-left
    /// corner in place. Can't be combined with `w`/`h`.
    #[serde(default)]
    pub scale: Option<f32>,
    /// With only one of `w`/`h`, derive the other from the current aspect ratio.
    #[serde(default)]
    pub keep_aspect: bool,
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
    /// An anchor layer that stays put — the others in `ids` align to *its*
    /// bounds instead of the whole group's shared bounding box. Mirrors the
    /// GUI's `canvas.reference_layer` (see `app.rs::align_selection`).
    #[serde(default)]
    pub to: Option<Uuid>,
}

#[derive(Serialize, Deserialize)]
pub struct ExportPngArgs {
    pub id: Uuid,
    pub path: String,
}

/// `content_bounds`: where a layer's rendered pixels differ from a
/// background color — e.g. where the text baked into a background image ends.
#[derive(Serialize, Deserialize)]
pub struct ContentBoundsArgs {
    pub id: Uuid,
    /// Only scan this rect, in the layer's parent-relative space (the same
    /// space as its `frame`). Omitted = the whole layer.
    #[serde(default)]
    pub region: Option<egui::Rect>,
    /// The color counted as "empty". Omitted = the region's top-left pixel.
    #[serde(default)]
    pub background: Option<Color32>,
    /// Largest per-channel difference from `background` still counted as
    /// background. Omitted = 32.
    #[serde(default)]
    pub tolerance: Option<u8>,
}

#[derive(Serialize, Deserialize)]
pub struct SaveArgs {
    #[serde(default)]
    pub path: Option<String>,
}

#[derive(Serialize, Deserialize)]
pub struct InitArgs {
    #[serde(default)]
    pub name: Option<String>,
    /// A `templates::all_templates` name (case-insensitive) to start from
    /// instead of an empty document.
    #[serde(default)]
    pub template: Option<String>,
}

/// Sets which layers are highlighted in the GUI — purely a courtesy so you
/// can see what the agent just touched, no effect on the document itself.
/// Only meaningful live (there's no selection state outside a running
/// `App`); `ids: []` clears the selection.
#[derive(Serialize, Deserialize)]
pub struct SelectArgs {
    pub ids: Vec<Uuid>,
}

#[derive(Serialize, Deserialize)]
pub struct NewPageArgs {
    pub name: String,
}

#[derive(Serialize, Deserialize)]
pub struct RenamePageArgs {
    pub id: Uuid,
    pub name: String,
}

#[derive(Serialize, Deserialize)]
pub struct RenameLayerArgs {
    pub id: Uuid,
    pub name: String,
}

/// Sets a layer's `MaskSettings`: `enabled` ("Use as Mask", via `mask`),
/// `ignore_underlying` ("Ignore Underlying Mask", via `ignore_mask`),
/// `mode` and/or `inverted`; an omitted field is left unchanged. At least one must be
/// given. A mask clips the siblings *below* it in its parent, back to the
/// previous mask — so group the mask with its content first (`group`)
/// unless it should clip everything beneath it at that level.
#[derive(Serialize, Deserialize)]
pub struct SetMaskArgs {
    pub id: Uuid,
    #[serde(default)]
    pub mask: Option<bool>,
    #[serde(default)]
    pub ignore_mask: Option<bool>,
    #[serde(default)]
    pub mode: Option<MaskMode>,
    #[serde(default)]
    pub inverted: Option<bool>,
}

/// Sets a layer's `ResizingConstraints` — how it follows its parent
/// Group/Artboard when that is resized — and, for an Artboard only,
/// `adjust_content` ("Adjust content on resize"). An omitted field is left
/// unchanged; at least one must be given.
#[derive(Serialize, Deserialize)]
pub struct SetResizingArgs {
    pub id: Uuid,
    #[serde(default)]
    pub pin_left: Option<bool>,
    #[serde(default)]
    pub pin_right: Option<bool>,
    #[serde(default)]
    pub pin_top: Option<bool>,
    #[serde(default)]
    pub pin_bottom: Option<bool>,
    #[serde(default)]
    pub fixed_width: Option<bool>,
    #[serde(default)]
    pub fixed_height: Option<bool>,
    #[serde(default)]
    pub adjust_content: Option<bool>,
}

#[derive(Serialize, Deserialize)]
pub struct GroupArgs {
    #[serde(default)]
    pub page: Option<Uuid>,
    pub ids: Vec<Uuid>,
}

#[derive(Serialize, Deserialize)]
pub struct UngroupArgs {
    #[serde(default)]
    pub page: Option<Uuid>,
    pub id: Uuid,
}

#[derive(Serialize, Deserialize)]
pub struct FlipArgs {
    #[serde(default)]
    pub page: Option<Uuid>,
    pub ids: Vec<Uuid>,
    /// One of "horizontal" / "vertical" — same plain-string convention as
    /// `AlignArgs::edge` (`transform_ops::FlipAxis` has no `serde` derive).
    pub axis: String,
}

/// Replaces a layer's fill: one color is a solid fill, two or more a
/// gradient with evenly spaced stops. `from`/`to` are normalized points in
/// the layer's unrotated bounds (see `model::Gradient`); omitted, they default
/// to `Gradient::linear`/`Gradient::radial`'s own.
#[derive(Serialize, Deserialize)]
pub struct SetFillArgs {
    pub id: Uuid,
    pub colors: Vec<Color32>,
    #[serde(default)]
    pub radial: bool,
    #[serde(default)]
    pub from: Option<Pos2>,
    #[serde(default)]
    pub to: Option<Pos2>,
}

/// Replaces a layer's drop shadows (`inner_shadows` when `inner`) with one
/// shadow, each omitted field taken from `Shadow::default()` — or removes
/// them all when `clear`.
#[derive(Serialize, Deserialize)]
pub struct SetShadowArgs {
    pub id: Uuid,
    #[serde(default)]
    pub inner: bool,
    #[serde(default)]
    pub clear: bool,
    #[serde(default)]
    pub color: Option<Color32>,
    #[serde(default)]
    pub offset: Option<Vec2>,
    #[serde(default)]
    pub blur: Option<f32>,
    #[serde(default)]
    pub spread: Option<f32>,
}

/// A star, polygon or line — the shapes beyond `add_rect`/`add_ellipse`,
/// styled through the same `AddShapeArgs` fields (`corner_radius` ignored).
/// `kind` is "star" / "polygon" / "line", a plain string like
/// `AlignArgs::edge`. `points`/`inner_ratio` (star) and `sides` (polygon)
/// default to the GUI tools' own 5 / 0.5 / 6.
#[derive(Serialize, Deserialize)]
pub struct AddFigureArgs {
    #[serde(flatten)]
    pub shape: AddShapeArgs,
    pub kind: String,
    #[serde(default)]
    pub points: Option<u32>,
    #[serde(default)]
    pub inner_ratio: Option<f32>,
    #[serde(default)]
    pub sides: Option<u32>,
}

#[derive(Serialize, Deserialize)]
pub struct RotateCopiesArgs {
    #[serde(default)]
    pub page: Option<Uuid>,
    pub ids: Vec<Uuid>,
    pub count: u32,
    pub total_degrees: f32,
}
