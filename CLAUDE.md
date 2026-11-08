# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## What this is

A native vector design tool (Sketch/Figma-ish: shapes, boolean ops, text-on-path,
masking, layer styles) built on `egui`/`eframe`. Two binaries share one library
crate (`simple_design`, declared in `src/lib.rs`):

- `simple-design` (`src/main.rs`) — the GUI. `cargo run` or `simple-design [file.sdesign]`.
- `simple-design-cli` (`src/bin/simple-design-cli.rs`) — a CLI that edits the
  same `.sdesign` documents, live against a running GUI instance or headless
  against the file. See "CLI / IPC" below — this is the one subsystem here
  that isn't covered by `docs/`, since it was added after those pages were written.

Full developer docs live in `docs/*.html` (Getting Started, Architecture, Text
& Fonts, File Format, Packaging) and are more detailed than this file for
everything except the CLI. Read `docs/architecture.html` for the full module
map and data model before making a non-trivial change — this file only
summarizes it and covers what those pages don't.

## Commands

```
cargo check                    # type-check only — fast, prefer this while iterating
cargo build
cargo run                      # launches the GUI (a native window, not a CLI)
cargo run --bin simple-design-cli -- <file.sdesign> <command>   # or ./target/debug/simple-design-cli directly
cargo test                     # run everything
cargo test --lib <test_name>   # run one test, e.g. cargo test --lib renders_filled_rect_at_correct_size_and_color
```

No separate lint/format CI — `cargo check`/`cargo build` emitting no warnings
is the bar to clear. Tests are `#[cfg(test)] mod tests` blocks inline in the
module they cover (most densely in `export.rs`, `io.rs`, and `ipc/ops.rs`).

## egui 0.36 gotcha

This pins `egui`/`eframe` 0.36, bleeding-edge enough that its API differs from
most training data: the `App` trait method is `fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame)`
(not `update`), panels are `egui::Panel::top/bottom/left/right(...)` (not
separate `TopBottomPanel`/`SidePanel` types), and menu bars are
`egui::MenuBar::new().ui(ui, ...)`. Before assuming an API from memory or
training data, check `~/.cargo/registry/src/*/egui-0.36.0/src/` directly.

## Architecture summary

(Full detail: `docs/architecture.html`.)

- **Data model** (`model/`): `Document { pages: Vec<Page>, text_styles, layer_styles, palette }`,
  `Page { layers: Vec<Layer>, guides }`, and `Layer` (recursive — only
  `Artboard`/`Group`/`BooleanGroup` have `children`). Pure serde data, no
  UI-framework dependency beyond `egui`'s geometry/color types. A `.sdesign`
  file is exactly `serde_json::to_string_pretty(&Document)`, no schema
  version header — see `docs/file-format.html`.
- **Coordinate convention**: `Layer.frame.pos` is parent-relative, not
  absolute — use `Page::absolute_offset(id)` to get page-space. `Frame.size`
  can be negative (preserves `Line`/`Arrow` drag direction); use
  `frame.bounds()` for anything direction-insensitive, `frame.rotated_bounds()`
  when rotation matters. Any new recursive layer-tree traversal must
  accumulate `offset + layer.frame.pos.to_vec2()` the same way
  `canvas.rs`/`export.rs`/`ipc::ops` already do, or nested shapes render or
  align in the wrong place.
- **Undo** (`history.rs`): whole-`Document` snapshot cloning, not a command
  pattern. Convention: `history.snapshot()` before mutating, then mutate via
  `history.mutate()`. For a continuous gesture, snapshot only once at the start.
- **Rendering is not shared**: `canvas.rs` (interactive) and `export.rs` (PNG)
  are two separate implementations. A new `LayerKind`/`Paint` variant needs a
  matching case in both, or exports will silently not match what's on screen.
- **Adding a new shape/tool** touches, in order: `tools.rs` → `model/layer.rs`
  → `canvas.rs` (`new_layer_for_tool`, `draw_layer`) → `export.rs` (`draw_layer`)
  → `ui/inspector.rs` → `ui/layers_panel.rs`'s `icon_for`.

## CLI / IPC (`src/ipc/`, `src/app/ipc_dispatch.rs`)

Lets an agent (or a script) drive a `.sdesign` document from the command line,
interleaved with live GUI editing in the same session — a CLI command applied
while the GUI has that file open lands on the live `Document`/`History`, not
just the file on disk.

Every `.sdesign` file carries a required `generator` field (`model::document::GENERATOR`)
naming this CLI, specifically so an agent that's handed a `.sdesign` file with
no other context (no source checkout, no this file) still discovers the CLI
exists by reading the raw JSON. It's deliberately *not* `#[serde(default)]`
like the rest of `Document`'s fields — see `docs/file-format.html` — so a file
missing it fails to load outright rather than silently.

- **Discovery** (`ipc/registry.rs`): keyed by the document's *canonical file
  path*. A running GUI instance binds a Unix socket at
  `$XDG_RUNTIME_DIR/simple-design/<hash(path)>.sock` and registers it in
  `instances.json`. The CLI checks that registry: found → **live mode**
  (send one newline-delimited-JSON request per connection, `ipc/protocol.rs`);
  not found → **headless mode** (load the file, mutate, save it back).
- **One op implementation, both modes**: `ipc/ops.rs::apply(doc: &mut Document, op, args)`
  is called both by the live server (`app/ipc_dispatch.rs`, wrapped in a
  `History::snapshot()` so each command is one undo step) and by the CLI's
  headless fallback directly. Adding an op means adding one match arm there
  (plus its `Args` struct in `protocol.rs`, a CLI subcommand in
  `bin/simple-design-cli.rs`, and a test in `ipc/ops.rs`'s `#[cfg(test)]`
  module) — not two separate implementations.
- `undo`/`redo`/`save`/`init`/`select` are *not* in `ops.rs` — they need
  `History`, a file path, or `App::selection`, none of which fit "pure
  function of a `Document`", so they're handled directly in
  `app/ipc_dispatch.rs` (live) / `bin/simple-design-cli.rs` (headless, where
  `undo`/`redo`/`select` are simply rejected — there's no history or
  selection state outside a running `App`).
- **`ctx.request_repaint()` from the socket's background thread is not
  reliable enough alone** to wake every windowing backend's idle event loop —
  confirmed hanging indefinitely in at least one dev environment.
  `App::drain_ipc` also calls `ctx.request_repaint_after(100ms)` whenever a
  socket is bound, as a guaranteed fallback. Don't remove this as "redundant"
  without retesting that a `ping` returns promptly against an idle (unfocused,
  no recent input) running instance.
- **`Document::add_page` switches `active_page`** to the new page. Any op
  with no explicit `page` id defaults to the *active* page
  (`ops::resolve_page`/`resolve_page_mut`), so a `new_page` immediately
  followed by another op without `--page` silently targets the empty new
  page instead of wherever the caller was working.
- `align`'s `to` field (an anchor layer id) is the only way to align a layer
  to a fixed reference instead of the whole group's shared bounding box —
  mirrors the GUI's own `canvas.reference_layer` (`app.rs::align_selection`).
  Without it, `align` moves *every* listed layer to their common bounding
  box, including ones you meant to keep fixed.
- **Prefer `list-layers --recursive` / `find-layer <query>` over `get-document`
  for locating a layer.** `get-document` dumps the *entire* document —
  every field (`style`, `runs`, `letter_spacing`, ...) for every layer — which
  gets expensive fast once a design has more than a couple dozen layers.
  `list-layers --recursive` and `find-layer` (a case-insensitive substring
  match on `name`, searched through the whole tree, not just the page's top
  level) both return the same lightweight summary instead: `id`/`name`/`kind`/
  `frame`/`depth`. Reach for `get-layer <id>`/`get-document` only when you
  actually need a layer's full style/content, not just its id.
