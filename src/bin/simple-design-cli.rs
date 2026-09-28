//! Drives a `.sdesign` document: talks to a running Simple Design instance
//! over its IPC socket if one has the file open (see `simple_design::ipc`),
//! otherwise loads the file, applies the one requested op, and saves it
//! back — no GUI required.

use std::path::PathBuf;

use clap::{Parser, Subcommand, ValueEnum};
use egui::{Pos2, Vec2};
use uuid::Uuid;

use simple_design::io;
use simple_design::ipc::{self, ops};
use simple_design::ipc::protocol::{
    AddShapeArgs, AlignArgs, BooleanArgs, DeleteLayerArgs, ExportPngArgs, FlipArgs, GetLayerArgs, GroupArgs, InitArgs, ListLayersArgs,
    NewPageArgs, RenameLayerArgs, RenamePageArgs, RotateCopiesArgs, SaveArgs, SelectArgs, SetFrameArgs, UngroupArgs,
};
use simple_design::model::{BoolOp, Document, Frame};

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
    /// Create a brand-new, empty document at `<file>` — errors if it already exists.
    Init {
        #[arg(long)]
        name: Option<String>,
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
    },
    /// Dump one layer's full JSON, wherever it is in the layer tree.
    GetLayer {
        id: Uuid,
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
        Command::Init { name } => ("init", to_value(InitArgs { name })),
        Command::Ping => ("ping", serde_json::Value::Null),
        Command::GetDocument => ("get_document", serde_json::Value::Null),
        Command::ListPages => ("list_pages", serde_json::Value::Null),
        Command::ListLayers { page } => ("list_layers", to_value(ListLayersArgs { page })),
        Command::GetLayer { id } => ("get_layer", to_value(GetLayerArgs { id })),
        Command::AddRect { x, y, w, h, rotation, page, name } => (
            "add_rect",
            to_value(AddShapeArgs {
                page,
                frame: Frame { pos: Pos2::new(x, y), size: Vec2::new(w, h), rotation: rotation.unwrap_or(0.0) },
                name,
            }),
        ),
        Command::AddEllipse { x, y, w, h, rotation, page, name } => (
            "add_ellipse",
            to_value(AddShapeArgs {
                page,
                frame: Frame { pos: Pos2::new(x, y), size: Vec2::new(w, h), rotation: rotation.unwrap_or(0.0) },
                name,
            }),
        ),
        Command::SetFrame { id, x, y, w, h, rotation } => {
            ("set_frame", to_value(SetFrameArgs { id, frame: Frame { pos: Pos2::new(x, y), size: Vec2::new(w, h), rotation } }))
        }
        Command::DeleteLayer { id } => ("delete_layer", to_value(DeleteLayerArgs { id })),
        Command::Boolean { op, ids, page } => ("boolean", to_value(BooleanArgs { page, ids, op: op.into() })),
        Command::Align { edge, ids, page } => ("align", to_value(AlignArgs { page, ids, edge: edge.as_str().to_string() })),
        Command::Select { ids } => ("select", to_value(SelectArgs { ids })),
        Command::Undo => ("undo", serde_json::Value::Null),
        Command::Redo => ("redo", serde_json::Value::Null),
        Command::Save { path } => ("save", to_value(SaveArgs { path: path.map(|p| p.to_string_lossy().into_owned()) })),
        Command::ExportPng { id, output } => ("export_png", to_value(ExportPngArgs { id, path: output.to_string_lossy().into_owned() })),
        Command::NewPage { name } => ("new_page", to_value(NewPageArgs { name })),
        Command::RenamePage { id, name } => ("rename_page", to_value(RenamePageArgs { id, name })),
        Command::RenameLayer { id, name } => ("rename_layer", to_value(RenameLayerArgs { id, name })),
        Command::Group { ids, page } => ("group", to_value(GroupArgs { page, ids })),
        Command::Ungroup { id, page } => ("ungroup", to_value(UngroupArgs { page, id })),
        Command::Flip { axis, ids, page } => ("flip", to_value(FlipArgs { page, ids, axis: axis.as_str().to_string() })),
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
        let mut document = Document::new();
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
