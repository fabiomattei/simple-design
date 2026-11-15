//! Drives a `.sdesign` document: talks to a running Simple Design instance
//! over its IPC socket if one has the file open (see `simple_design::ipc`),
//! otherwise loads the file, applies the one requested op, and saves it
//! back — no GUI required.

use std::path::PathBuf;

use clap::{Parser, Subcommand, ValueEnum};
use egui::{Pos2, Vec2};
use uuid::Uuid;

use simple_design::io;
use simple_design::templates;
use simple_design::ipc::{self, ops};
use simple_design::ipc::protocol::{
    AddArtboardArgs, AddFigureArgs, AddImageArgs, AddShapeArgs, AddTextArgs, AlignArgs, BooleanArgs, DeleteLayerArgs, ExportPngArgs, FindLayerArgs, FlipArgs,
    GetLayerArgs, GroupArgs, InitArgs, ListLayersArgs, NewPageArgs, RenameLayerArgs, RenamePageArgs, RotateCopiesArgs, SaveArgs, SelectArgs,
    SetFillArgs, SetFrameArgs, SetMaskArgs, SetResizingArgs, SetShadowArgs, UngroupArgs,
};
use simple_design::model::{BoolOp, Document, Frame, MaskMode, TextAlign, TextFont, VerticalAlign};

#[derive(Parser)]
#[command(name = "simple-design-cli", about = "Drive a running Simple Design instance, or edit a .sdesign file directly when none is open")]
struct Cli {
    /// The .sdesign file to operate on.
    file: PathBuf,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Create a brand-new document at `<file>` — errors if it already exists.
    /// Empty unless `--template` names one; an unknown name lists the available ones.
    Init {
        #[arg(long)]
        name: Option<String>,
        /// Start from this template (built-in or saved via File > Save as Template).
        #[arg(long)]
        template: Option<String>,
    },
    /// Check whether a running instance has this file open.
    Ping,
    /// Dump the whole document as JSON — same shape as the `.sdesign` file itself.
    GetDocument,
    /// List every page's id and name.
    ListPages,
    /// List the top-level layers of a page (default: the active page).
    ListLayers {
        #[arg(long)]
        page: Option<Uuid>,
        /// Walk into groups/artboards too, flattening the whole tree instead
        /// of stopping at the top level (each entry's `depth` says how nested it was).
        #[arg(long)]
        recursive: bool,
    },
    /// Dump one layer's full JSON, wherever it is in the layer tree.
    GetLayer {
        id: Uuid,
    },
    /// Find layers by name (case-insensitive substring match), anywhere in
    /// the layer tree — a cheaper alternative to `get-document`/`get-layer`
    /// when you know a layer's name but not its id. Returns the same light
    /// summary as `list-layers`, not each match's full JSON.
    FindLayer {
        query: String,
        #[arg(long)]
        page: Option<Uuid>,
    },
    /// Add a filled rectangle.
    AddRect {
        #[arg(long)]
        x: f32,
        #[arg(long)]
        y: f32,
        #[arg(long)]
        w: f32,
        #[arg(long)]
        h: f32,
        #[arg(long)]
        rotation: Option<f32>,
        /// Page to add to (default: the active page).
        #[arg(long)]
        page: Option<Uuid>,
        #[arg(long)]
        name: Option<String>,
        /// Hex fill color, RRGGBB or RRGGBBAA (defaults to `Style::default()`'s flat gray).
        #[arg(long)]
        fill: Option<HexColor>,
        /// Drop any fill (a transparent/outline-only shape). Takes priority over `--fill`.
        #[arg(long)]
        no_fill: bool,
        /// Drop the default 1px dark stroke. Takes priority over `--stroke`/`--stroke-width`.
        #[arg(long)]
        no_stroke: bool,
        /// Recolor the stroke (defaults to `Style::default()`'s dark gray).
        #[arg(long)]
        stroke: Option<HexColor>,
        /// Defaults to 1.0.
        #[arg(long)]
        stroke_width: Option<f32>,
        #[arg(long)]
        corner_radius: Option<f32>,
        /// Add inside this artboard/group instead of the page's top level —
        /// --x/--y are then relative to it.
        #[arg(long)]
        parent: Option<Uuid>,
    },
    /// Add a filled ellipse, inscribed in the given x/y/w/h box.
    AddEllipse {
        #[arg(long)]
        x: f32,
        #[arg(long)]
        y: f32,
        #[arg(long)]
        w: f32,
        #[arg(long)]
        h: f32,
        #[arg(long)]
        rotation: Option<f32>,
        #[arg(long)]
        page: Option<Uuid>,
        #[arg(long)]
        name: Option<String>,
        /// Hex fill color, RRGGBB or RRGGBBAA (defaults to `Style::default()`'s flat gray).
        #[arg(long)]
        fill: Option<HexColor>,
        /// Drop any fill (a transparent/outline-only shape). Takes priority over `--fill`.
        #[arg(long)]
        no_fill: bool,
        /// Drop the default 1px dark stroke. Takes priority over `--stroke`/`--stroke-width`.
        #[arg(long)]
        no_stroke: bool,
        /// Recolor the stroke (defaults to `Style::default()`'s dark gray).
        #[arg(long)]
        stroke: Option<HexColor>,
        /// Defaults to 1.0.
        #[arg(long)]
        stroke_width: Option<f32>,
        /// Add inside this artboard/group instead of the page's top level —
        /// --x/--y are then relative to it.
        #[arg(long)]
        parent: Option<Uuid>,
    },
    /// Add a text layer.
    AddText {
        content: String,
        #[arg(long)]
        x: f32,
        #[arg(long)]
        y: f32,
        #[arg(long)]
        w: f32,
        #[arg(long)]
        h: f32,
        #[arg(long)]
        rotation: Option<f32>,
        /// Defaults to 24.0.
        #[arg(long)]
        font_size: Option<f32>,
        /// Defaults to proportional.
        #[arg(long, value_enum)]
        font: Option<TextFontArg>,
        /// Only the weak "faux bold" (offset double-draw) both `canvas/` and
        /// export/ fall back to — see `--true-bold` for real bold glyphs.
        #[arg(long)]
        bold: bool,
        /// Real bold-weight glyphs (via a `runs` entry), not just the faux
        /// double-draw `--bold` alone gets. Implies `--bold`.
        #[arg(long)]
        true_bold: bool,
        /// Defaults to left.
        #[arg(long, value_enum)]
        align: Option<TextAlignArg>,
        /// Defaults to top.
        #[arg(long, value_enum)]
        vertical_align: Option<VerticalAlignArg>,
        /// Hex text color, RRGGBB or RRGGBBAA (defaults to black).
        #[arg(long)]
        fill: Option<HexColor>,
        #[arg(long)]
        page: Option<Uuid>,
        #[arg(long)]
        name: Option<String>,
        /// Add inside this artboard/group instead of the page's top level —
        /// --x/--y are then relative to it.
        #[arg(long)]
        parent: Option<Uuid>,
    },
    /// Add a bitmap image from a local file — PNG/JPEG/GIF/BMP/TIFF/WebP all
    /// work (re-encoded to PNG on insert, same as the GUI). `--w`/`--h`
    /// default to the source image's own pixel size if omitted.
    AddImage {
        path: PathBuf,
        #[arg(long)]
        x: f32,
        #[arg(long)]
        y: f32,
        #[arg(long)]
        w: Option<f32>,
        #[arg(long)]
        h: Option<f32>,
        #[arg(long)]
        page: Option<Uuid>,
        #[arg(long)]
        name: Option<String>,
        /// Add inside this artboard/group instead of the page's top level —
        /// --x/--y are then relative to it.
        #[arg(long)]
        parent: Option<Uuid>,
    },
    /// Add a top-level artboard. Size it with --preset (desktop, web, tablet,
    /// phone, a3, a4, a5, a6) and/or --w/--h, which override the preset's
    /// dimensions; without a preset both --w and --h are required. Put layers
    /// inside it with the add-* commands' --parent <artboard id>.
    AddArtboard {
        #[arg(long)]
        x: f32,
        #[arg(long)]
        y: f32,
        #[arg(long)]
        w: Option<f32>,
        #[arg(long)]
        h: Option<f32>,
        #[arg(long)]
        preset: Option<String>,
        /// Defaults to the preset's name, else "Artboard".
        #[arg(long)]
        name: Option<String>,
        /// Hex background color, RRGGBB or RRGGBBAA (defaults to white).
        #[arg(long)]
        background: Option<HexColor>,
        #[arg(long)]
        page: Option<Uuid>,
    },
    /// Replace a layer's position/size/rotation outright.
    SetFrame {
        id: Uuid,
        #[arg(long)]
        x: f32,
        #[arg(long)]
        y: f32,
        #[arg(long)]
        w: f32,
        #[arg(long)]
        h: f32,
        #[arg(long, default_value_t = 0.0)]
        rotation: f32,
    },
    /// Delete a layer.
    DeleteLayer {
        id: Uuid,
    },
    /// Combine layers with a boolean shape operation, replacing them with one new layer.
    Boolean {
        #[arg(value_enum)]
        op: BoolOpArg,
        /// The layers to combine, bottom to top.
        ids: Vec<Uuid>,
        #[arg(long)]
        page: Option<Uuid>,
    },
    /// Align layers to a shared edge/center.
    Align {
        #[arg(value_enum)]
        edge: AlignEdgeArg,
        ids: Vec<Uuid>,
        /// An anchor layer that stays put; the others align to *its* bounds
        /// instead of the group's shared bounding box.
        #[arg(long)]
        to: Option<Uuid>,
        #[arg(long)]
        page: Option<Uuid>,
    },
    /// Highlight layers in the running GUI (pass none to clear) — needs a running instance.
    Select {
        #[arg(num_args = 0..)]
        ids: Vec<Uuid>,
    },
    /// Undo the last change — needs a running instance (no history exists headless).
    Undo,
    /// Redo the last undone change — same requirement as `undo`.
    Redo,
    /// Save the document — to its current path, or elsewhere with --path.
    Save {
        #[arg(long)]
        path: Option<PathBuf>,
    },
    /// Render one layer to a standalone PNG file.
    ExportPng {
        id: Uuid,
        output: PathBuf,
    },
    /// Add a new page, which also becomes the active page.
    NewPage {
        name: String,
    },
    /// Rename a page.
    RenamePage {
        id: Uuid,
        name: String,
    },
    /// Rename a layer.
    RenameLayer {
        id: Uuid,
        name: String,
    },
    /// Set "Use as Mask" (`--mask true|false`), "Ignore Underlying Mask"
    /// (`--ignore-mask true|false`), the mask mode (`--mode alpha|outline`)
    /// and/or "Invert Mask" (`--inverted true|false`) on a layer. A mask
    /// clips the siblings below it in its parent, so group it with its
    /// content first.
    SetMask {
        id: Uuid,
        #[arg(long)]
        mask: Option<bool>,
        #[arg(long)]
        ignore_mask: Option<bool>,
        #[arg(long, value_enum)]
        mode: Option<MaskModeArg>,
        #[arg(long)]
        inverted: Option<bool>,
    },
    /// Set a layer's resizing constraints — how it follows its parent
    /// Group/Artboard when that is resized: `--pin-left`, `--pin-right`,
    /// `--pin-top`, `--pin-bottom`, `--fixed-width`, `--fixed-height` (each
    /// `true|false`, omitted = unchanged; all off = scale with the parent).
    /// `--adjust-content true|false` (Artboards only) sets whether resizing
    /// the artboard moves its content at all.
    SetResizing {
        id: Uuid,
        #[arg(long)]
        pin_left: Option<bool>,
        #[arg(long)]
        pin_right: Option<bool>,
        #[arg(long)]
        pin_top: Option<bool>,
        #[arg(long)]
        pin_bottom: Option<bool>,
        #[arg(long)]
        fixed_width: Option<bool>,
        #[arg(long)]
        fixed_height: Option<bool>,
        #[arg(long)]
        adjust_content: Option<bool>,
    },
    /// Group layers into a new Group layer.
    Group {
        ids: Vec<Uuid>,
        #[arg(long)]
        page: Option<Uuid>,
    },
    /// Splice a group's children back into its parent, dissolving the group.
    Ungroup {
        id: Uuid,
        #[arg(long)]
        page: Option<Uuid>,
    },
    /// Mirror layers about their own frame center.
    Flip {
        #[arg(value_enum)]
        axis: FlipAxisArg,
        ids: Vec<Uuid>,
        #[arg(long)]
        page: Option<Uuid>,
    },
    /// Replace a layer's fill: one `--colors` value is a solid fill, two or
    /// more (comma-separated hex) a gradient with evenly spaced stops.
    /// `--from`/`--to` are `x,y` points normalized to the layer's bounds
    /// (default: top-left to bottom-right, or for `--radial` the center to
    /// the right edge).
    SetFill {
        id: Uuid,
        #[arg(long, value_delimiter = ',', required = true)]
        colors: Vec<HexColor>,
        #[arg(long)]
        radial: bool,
        #[arg(long)]
        from: Option<PointArg>,
        #[arg(long)]
        to: Option<PointArg>,
    },
    /// Replace a layer's drop shadow (`--inner`: its inner shadow) with one
    /// shadow, or remove them with `--clear`. Omitted values default to a
    /// soft black shadow 4px down, 8px blur.
    SetShadow {
        id: Uuid,
        #[arg(long)]
        inner: bool,
        #[arg(long)]
        clear: bool,
        /// Hex color, RRGGBB or RRGGBBAA.
        #[arg(long)]
        color: Option<HexColor>,
        #[arg(long)]
        x: Option<f32>,
        #[arg(long)]
        y: Option<f32>,
        #[arg(long)]
        blur: Option<f32>,
        #[arg(long)]
        spread: Option<f32>,
    },
    /// Add a star, polygon or line, styled with the same flags as `add-rect`.
    /// A line runs from the box's top-left to its bottom-right corner and
    /// shows only its stroke.
    AddShape {
        #[arg(long, value_enum)]
        kind: ShapeKindArg,
        #[arg(long)]
        x: f32,
        #[arg(long)]
        y: f32,
        #[arg(long)]
        w: f32,
        #[arg(long)]
        h: f32,
        #[arg(long)]
        rotation: Option<f32>,
        /// Star only; defaults to 5.
        #[arg(long)]
        points: Option<u32>,
        /// Star only: inner radius as a fraction of the outer; defaults to 0.5.
        #[arg(long)]
        inner_ratio: Option<f32>,
        /// Polygon only; defaults to 6.
        #[arg(long)]
        sides: Option<u32>,
        #[arg(long)]
        page: Option<Uuid>,
        #[arg(long)]
        name: Option<String>,
        #[arg(long)]
        fill: Option<HexColor>,
        #[arg(long)]
        no_fill: bool,
        #[arg(long)]
        no_stroke: bool,
        #[arg(long)]
        stroke: Option<HexColor>,
        #[arg(long)]
        stroke_width: Option<f32>,
        #[arg(long)]
        parent: Option<Uuid>,
    },
    /// Replace layers with `count` copies evenly rotated across `total_degrees`.
    RotateCopies {
        ids: Vec<Uuid>,
        #[arg(long)]
        count: u32,
        #[arg(long)]
        total_degrees: f32,
        #[arg(long)]
        page: Option<Uuid>,
    },
}

#[derive(Clone, ValueEnum)]
enum BoolOpArg {
    Union,
    Subtract,
    Intersect,
    Difference,
    Add,
}

impl From<BoolOpArg> for BoolOp {
    fn from(op: BoolOpArg) -> Self {
        match op {
            BoolOpArg::Union => BoolOp::Union,
            BoolOpArg::Subtract => BoolOp::Subtract,
            BoolOpArg::Intersect => BoolOp::Intersect,
            BoolOpArg::Difference => BoolOp::Difference,
            BoolOpArg::Add => BoolOp::Add,
        }
    }
}

/// `--fill`'s CLI type: parses a bare `RRGGBB`/`RRGGBBAA` hex string (an
/// optional leading `#` is stripped) into a `Color32`, the way `--fill
/// ff8800` or `--fill #ff8800cc` reads on the command line. Wraps `Color32`
/// (rather than implementing `FromStr` on it directly) since it's a foreign
/// type this crate doesn't own.
#[derive(Clone, Copy)]
struct HexColor(egui::Color32);

impl std::str::FromStr for HexColor {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let hex = s.strip_prefix('#').unwrap_or(s);
        let byte = |range: std::ops::Range<usize>| -> Result<u8, String> {
            let chunk = hex.get(range).ok_or_else(|| format!("invalid hex color: {s:?} (expected RRGGBB or RRGGBBAA)"))?;
            u8::from_str_radix(chunk, 16).map_err(|_| format!("invalid hex color: {s:?} (expected RRGGBB or RRGGBBAA)"))
        };
        match hex.len() {
            6 => Ok(HexColor(egui::Color32::from_rgb(byte(0..2)?, byte(2..4)?, byte(4..6)?))),
            8 => Ok(HexColor(egui::Color32::from_rgba_unmultiplied(byte(0..2)?, byte(2..4)?, byte(4..6)?, byte(6..8)?))),
            _ => Err(format!("invalid hex color: {s:?} (expected RRGGBB or RRGGBBAA)")),
        }
    }
}

/// `--from`/`--to`'s CLI type: an `x,y` pair, like `--from 0,0.5`.
#[derive(Clone, Copy)]
struct PointArg(Pos2);

impl std::str::FromStr for PointArg {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let invalid = || format!("invalid point: {s:?} (expected x,y)");
        let (x, y) = s.split_once(',').ok_or_else(invalid)?;
        let x: f32 = x.trim().parse().map_err(|_| invalid())?;
        let y: f32 = y.trim().parse().map_err(|_| invalid())?;
        Ok(PointArg(Pos2::new(x, y)))
    }
}

#[derive(Clone, ValueEnum)]
enum ShapeKindArg {
    Star,
    Polygon,
    Line,
}

impl ShapeKindArg {
    fn as_str(&self) -> &'static str {
        match self {
            ShapeKindArg::Star => "star",
            ShapeKindArg::Polygon => "polygon",
            ShapeKindArg::Line => "line",
        }
    }
}

/// `clap::ValueEnum` for `TextFont`, minus `System` (a free-text family
/// name a clap enum can't represent) — pass a font that isn't installed via
/// `set-frame`/hand-edited JSON if that's ever needed from the CLI.
#[derive(Clone, ValueEnum)]
enum TextFontArg {
    Proportional,
    Monospace,
    Serif,
    Display,
    Handwriting,
}

impl From<TextFontArg> for TextFont {
    fn from(font: TextFontArg) -> Self {
        match font {
            TextFontArg::Proportional => TextFont::Proportional,
            TextFontArg::Monospace => TextFont::Monospace,
            TextFontArg::Serif => TextFont::Serif,
            TextFontArg::Display => TextFont::Display,
            TextFontArg::Handwriting => TextFont::Handwriting,
        }
    }
}

#[derive(Clone, ValueEnum)]
enum TextAlignArg {
    Left,
    Center,
    Right,
    Justify,
}

impl From<TextAlignArg> for TextAlign {
    fn from(align: TextAlignArg) -> Self {
        match align {
            TextAlignArg::Left => TextAlign::Left,
            TextAlignArg::Center => TextAlign::Center,
            TextAlignArg::Right => TextAlign::Right,
            TextAlignArg::Justify => TextAlign::Justify,
        }
    }
}

#[derive(Clone, ValueEnum)]
enum VerticalAlignArg {
    Top,
    Middle,
    Bottom,
}

impl From<VerticalAlignArg> for VerticalAlign {
    fn from(align: VerticalAlignArg) -> Self {
        match align {
            VerticalAlignArg::Top => VerticalAlign::Top,
            VerticalAlignArg::Middle => VerticalAlign::Middle,
            VerticalAlignArg::Bottom => VerticalAlign::Bottom,
        }
    }
}

#[derive(Clone, ValueEnum)]
enum AlignEdgeArg {
    Left,
    Hcenter,
    Right,
    Top,
    Vcenter,
    Bottom,
}

impl AlignEdgeArg {
    fn as_str(&self) -> &'static str {
        match self {
            AlignEdgeArg::Left => "left",
            AlignEdgeArg::Hcenter => "hcenter",
            AlignEdgeArg::Right => "right",
            AlignEdgeArg::Top => "top",
            AlignEdgeArg::Vcenter => "vcenter",
            AlignEdgeArg::Bottom => "bottom",
        }
    }
}

#[derive(Clone, ValueEnum)]
enum MaskModeArg {
    Alpha,
    Outline,
}

impl From<MaskModeArg> for MaskMode {
    fn from(mode: MaskModeArg) -> Self {
        match mode {
            MaskModeArg::Alpha => MaskMode::Alpha,
            MaskModeArg::Outline => MaskMode::Outline,
        }
    }
}

#[derive(Clone, ValueEnum)]
enum FlipAxisArg {
    Horizontal,
    Vertical,
}

impl FlipAxisArg {
    fn as_str(&self) -> &'static str {
        match self {
            FlipAxisArg::Horizontal => "horizontal",
            FlipAxisArg::Vertical => "vertical",
        }
    }
}

fn to_value(args: impl serde::Serialize) -> serde_json::Value {
    serde_json::to_value(args).expect("args always serialize")
}

fn build_request(command: Command) -> (&'static str, serde_json::Value) {
    match command {
        Command::Init { name, template } => ("init", to_value(InitArgs { name, template })),
        Command::Ping => ("ping", serde_json::Value::Null),
        Command::GetDocument => ("get_document", serde_json::Value::Null),
        Command::ListPages => ("list_pages", serde_json::Value::Null),
        Command::ListLayers { page, recursive } => ("list_layers", to_value(ListLayersArgs { page, recursive })),
        Command::GetLayer { id } => ("get_layer", to_value(GetLayerArgs { id })),
        Command::FindLayer { query, page } => ("find_layer", to_value(FindLayerArgs { page, query })),
        Command::AddRect { x, y, w, h, rotation, page, name, fill, no_fill, no_stroke, stroke, stroke_width, corner_radius, parent } => (
            "add_rect",
            to_value(AddShapeArgs {
                page,
                frame: Frame { pos: Pos2::new(x, y), size: Vec2::new(w, h), rotation: rotation.unwrap_or(0.0) },
                name,
                fill: fill.map(|c| c.0),
                no_fill,
                no_stroke,
                stroke: stroke.map(|c| c.0),
                stroke_width,
                corner_radius,
                parent,
            }),
        ),
        Command::AddEllipse { x, y, w, h, rotation, page, name, fill, no_fill, no_stroke, stroke, stroke_width, parent } => (
            "add_ellipse",
            to_value(AddShapeArgs {
                page,
                frame: Frame { pos: Pos2::new(x, y), size: Vec2::new(w, h), rotation: rotation.unwrap_or(0.0) },
                name,
                fill: fill.map(|c| c.0),
                no_fill,
                no_stroke,
                stroke: stroke.map(|c| c.0),
                stroke_width,
                corner_radius: None,
                parent,
            }),
        ),
        Command::AddText { content, x, y, w, h, rotation, font_size, font, bold, true_bold, align, vertical_align, fill, page, name, parent } => (
            "add_text",
            to_value(AddTextArgs {
                page,
                frame: Frame { pos: Pos2::new(x, y), size: Vec2::new(w, h), rotation: rotation.unwrap_or(0.0) },
                content,
                font_size,
                font: font.map(Into::into),
                bold,
                true_bold,
                align: align.map(Into::into),
                vertical_align: vertical_align.map(Into::into),
                fill: fill.map(|c| c.0),
                name,
                parent,
            }),
        ),
        Command::AddImage { path, x, y, w, h, page, name, parent } => (
            "add_image",
            to_value(AddImageArgs { page, path: path.to_string_lossy().into_owned(), x, y, w, h, name, parent }),
        ),
        Command::AddArtboard { x, y, w, h, preset, name, background, page } => (
            "add_artboard",
            to_value(AddArtboardArgs { page, x, y, w, h, preset, name, background: background.map(|c| c.0) }),
        ),
        Command::SetFrame { id, x, y, w, h, rotation } => {
            ("set_frame", to_value(SetFrameArgs { id, frame: Frame { pos: Pos2::new(x, y), size: Vec2::new(w, h), rotation } }))
        }
        Command::DeleteLayer { id } => ("delete_layer", to_value(DeleteLayerArgs { id })),
        Command::Boolean { op, ids, page } => ("boolean", to_value(BooleanArgs { page, ids, op: op.into() })),
        Command::Align { edge, ids, to, page } => ("align", to_value(AlignArgs { page, ids, edge: edge.as_str().to_string(), to })),
        Command::Select { ids } => ("select", to_value(SelectArgs { ids })),
        Command::Undo => ("undo", serde_json::Value::Null),
        Command::Redo => ("redo", serde_json::Value::Null),
        Command::Save { path } => ("save", to_value(SaveArgs { path: path.map(|p| p.to_string_lossy().into_owned()) })),
        Command::ExportPng { id, output } => ("export_png", to_value(ExportPngArgs { id, path: output.to_string_lossy().into_owned() })),
        Command::NewPage { name } => ("new_page", to_value(NewPageArgs { name })),
        Command::RenamePage { id, name } => ("rename_page", to_value(RenamePageArgs { id, name })),
        Command::RenameLayer { id, name } => ("rename_layer", to_value(RenameLayerArgs { id, name })),
        Command::SetMask { id, mask, ignore_mask, mode, inverted } => {
            ("set_mask", to_value(SetMaskArgs { id, mask, ignore_mask, mode: mode.map(Into::into), inverted }))
        }
        Command::SetResizing { id, pin_left, pin_right, pin_top, pin_bottom, fixed_width, fixed_height, adjust_content } => (
            "set_resizing",
            to_value(SetResizingArgs { id, pin_left, pin_right, pin_top, pin_bottom, fixed_width, fixed_height, adjust_content }),
        ),
        Command::Group { ids, page } => ("group", to_value(GroupArgs { page, ids })),
        Command::Ungroup { id, page } => ("ungroup", to_value(UngroupArgs { page, id })),
        Command::Flip { axis, ids, page } => ("flip", to_value(FlipArgs { page, ids, axis: axis.as_str().to_string() })),
        Command::SetFill { id, colors, radial, from, to } => (
            "set_fill",
            to_value(SetFillArgs { id, colors: colors.into_iter().map(|c| c.0).collect(), radial, from: from.map(|p| p.0), to: to.map(|p| p.0) }),
        ),
        Command::SetShadow { id, inner, clear, color, x, y, blur, spread } => {
            let offset = (x.is_some() || y.is_some()).then(|| Vec2::new(x.unwrap_or(0.0), y.unwrap_or(0.0)));
            ("set_shadow", to_value(SetShadowArgs { id, inner, clear, color: color.map(|c| c.0), offset, blur, spread }))
        }
        Command::AddShape {
            kind, x, y, w, h, rotation, points, inner_ratio, sides, page, name, fill, no_fill, no_stroke, stroke, stroke_width, parent,
        } => (
            "add_shape",
            to_value(AddFigureArgs {
                shape: AddShapeArgs {
                    page,
                    frame: Frame { pos: Pos2::new(x, y), size: Vec2::new(w, h), rotation: rotation.unwrap_or(0.0) },
                    name,
                    fill: fill.map(|c| c.0),
                    no_fill,
                    no_stroke,
                    stroke: stroke.map(|c| c.0),
                    stroke_width,
                    corner_radius: None,
                    parent,
                },
                kind: kind.as_str().to_string(),
                points,
                inner_ratio,
                sides,
            }),
        ),
        Command::RotateCopies { ids, count, total_degrees, page } => {
            ("rotate_copies", to_value(RotateCopiesArgs { page, ids, count, total_degrees }))
        }
    }
}

/// Runs `op` directly against the file on disk — used when no running
/// instance has it open. `init` is the one op that must run *before* a file
/// exists to load; `undo`/`redo` have no meaning here (there's no history
/// outside a live `App`); `save` and every op in `ops::mutates` need the
/// result written back, everything else is a read-only query.
fn run_headless(file: &PathBuf, op: &str, args: serde_json::Value) -> anyhow::Result<ipc::Response> {
    if op == "init" {
        if file.exists() {
            return Ok(ipc::Response::err(1, format!("{} already exists", file.display())));
        }
        let args: InitArgs = serde_json::from_value(args)?;
        let mut document = match args.template {
            None => Document::new(),
            Some(name) => match templates::find_template(&name) {
                Some(template) => template.instantiate()?,
                None => {
                    let available: Vec<String> = templates::all_templates().into_iter().map(|t| t.name).collect();
                    return Ok(ipc::Response::err(1, format!("no template named {name:?} — available: {}", available.join(", "))));
                }
            },
        };
        if let Some(name) = args.name {
            document.name = name;
        }
        io::save_to(file, &document)?;
        return Ok(ipc::Response::ok(1, serde_json::json!({ "path": file })));
    }

    if matches!(op, "undo" | "redo" | "select") {
        return Ok(ipc::Response::err(1, format!("{op} needs a running simple-design instance with this file open")));
    }

    if !file.exists() {
        return Ok(ipc::Response::err(1, format!("{} does not exist — use `init` to create it", file.display())));
    }

    let mut document = io::load_from(file)?;

    if op == "save" {
        let args: SaveArgs = serde_json::from_value(args)?;
        let target = args.path.map(PathBuf::from).unwrap_or_else(|| file.clone());
        io::save_to(&target, &document)?;
        return Ok(ipc::Response::ok(1, serde_json::json!({ "path": target })));
    }

    Ok(match ops::apply(&mut document, op, args) {
        Ok(value) => {
            if ops::mutates(op) {
                io::save_to(file, &document)?;
            }
            ipc::Response::ok(1, value)
        }
        Err(err) => ipc::Response::err(1, err),
    })
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    let (op, args) = build_request(cli.command);

    let response = match ipc::client::connect(&cli.file) {
        Some(stream) => ipc::client::send(&stream, op, args)?,
        None => run_headless(&cli.file, op, args)?,
    };

    if response.ok {
        println!("{}", serde_json::to_string_pretty(&response.result.unwrap_or(serde_json::Value::Null))?);
        Ok(())
    } else {
        eprintln!("error: {}", response.error.unwrap_or_default());
        std::process::exit(1);
    }
}
