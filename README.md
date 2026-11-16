# simple-design

A native vector design tool for UI mockups and vector artwork, written in
pure Rust with [egui](https://github.com/emilk/egui)/eframe. It works like
Sketch or Figma: pages of artboards, shapes, paths, text and images, edited
through a canvas, a layers panel and an inspector. It runs in a native window,
not a web app or webview. Rendering, hit-testing and export are all built from
scratch on top of egui.

It's a personal project, but already covers most of what a core vector
editor needs.

## Features

- **Documents**: multiple pages, artboards (screen and paper presets),
  groups, and a nested layer tree with fill, stroke, shadow and opacity
  styling. You can also save shared text styles, layer styles and a color
  palette.
- **Shapes**: Rectangle (separate radius per corner), Oval, Line, Arrow,
  Star, Polygon, a Pen tool with bezier handles, and Scissors to cut paths.
- **Boolean operations**: Union, Subtract, Intersect, Difference and Add.
  The result is a non-destructive group that updates when its shapes change.
  Flatten bakes it into a plain path.
- **Text**: auto-sizing labels or fixed-width wrapping boxes, text on a path,
  per-character rich formatting, and lists. Five font families are bundled,
  and any font installed on the system works too. Text converts to outlines.
- **Images**: insert, crop, magic-wand selection, background removal and
  color adjustment.
- **Masking**: alpha or outline masks, inverted masks, and per-layer
  "ignore mask".
- **Layout helpers**: align and distribute, Make Grid, Rotate Copies, flip,
  rulers, guides, snapping and a pixel grid.
- **Editing**: undo/redo, and copy/paste through the OS clipboard (works
  between running instances). Batch rename and a scale mode that also
  scales stroke widths and corner radii.
- **Templates**: built-in starting points (Landing Page, Mobile App Screen,
  Social Post, Presentation, Resume, Dashboard and more), plus your own,
  saved with File > Save as Template.
- **Export**:
  - **PNG**: rendered by its own rasterizer (tiny-skia).
  - **HTML/CSS**: a folder with `index.html`, `style.css`, images and fonts.
    In Flexbox mode it infers rows, columns and grids from where things sit,
    so text is real text and the layout can reflow. Absolute mode keeps
    every element at its exact position instead.
- **CLI**: `simple-design-cli` edits the same documents from scripts or AI
  agents. It edits the file directly, or makes the change live in a running
  GUI (see below).

Documents are saved as `.sdesign` files: plain, pretty-printed JSON.

## Getting started

Needs a recent stable Rust toolchain (edition 2024) on macOS or Linux.

```
cargo run                      # launch the app
cargo run -- file.sdesign      # open a file at startup
cargo test                     # run the tests
```

## Documentation

Detailed developer and user docs are in [`docs/`](docs/index.html):
Getting Started, Tools & Features (every tool and shortcut), Architecture,
Text & Fonts, File Format, HTML Export, and Packaging (Linux desktop entry
and Flatpak).

## CLI

`simple-design-cli` drives a `.sdesign` document from the command line — by
itself (load, edit, save) or against an already-running `simple-design`
instance that has the same file open, in which case the edit lands live in
the GUI and shows up as a single undo step:

```
simple-design-cli <file.sdesign> <command> [args...]
simple-design-cli --help
```

Nothing to open yet? `init` creates a brand-new document (errors instead of
overwriting one that already exists):

```
simple-design-cli new-project.sdesign init --name "My Project"
```

…or start from a template — a built-in one (Landing Page, Mobile App Screen,
Social Post, Poster & Business Card, Presentation, Story, Resume, Dashboard,
Video Thumbnail, Email Newsletter) or one you saved with File > Save as
Template. Names are case-insensitive; an unknown one lists what's available:

```
simple-design-cli new-project.sdesign init --template "landing page"
```

Artboards and nesting: `add-artboard` creates one, sized by `--preset`
(desktop, web, tablet, phone, a3–a6) and/or `--w`/`--h`. Pass its id as
`--parent` to `add-rect`/`add-ellipse`/`add-text`/`add-image` to put layers
inside it, with `--x`/`--y` relative to the artboard:

```
simple-design-cli f.sdesign add-artboard --x 0 --y 0 --preset phone --background f3f4f6
simple-design-cli f.sdesign add-text "Hello" --x 24 --y 64 --w 300 --h 40 --parent <artboard-id>
```

Open a specific file at startup so the CLI has something to talk to:

```
simple-design <file.sdesign>
```

## Installing (Linux)

Requirements: a recent stable Rust toolchain from [rustup](https://rustup.rs)
(edition 2024, so Rust 1.85 or newer) and a C toolchain for the native
dependencies (`gcc`, plus `pkg-config` on most distros).

```
git clone <this repo> simple-design && cd simple-design
./packaging/linux/install.sh
```

This builds a release build and installs it for your user only, no root
needed:

| What | Where |
|---|---|
| GUI (`simple-design`) and CLI (`simple-design-cli`) | `~/.local/bin/` |
| App icon | `~/.local/share/icons/hicolor/512x512/apps/simple-design.png` |
| Desktop entry (app grid, dock, "Open with") | `~/.local/share/applications/simple-design.desktop` |

Afterwards, launch **Simple Design** from your desktop's app menu, or run
`simple-design [file.sdesign]` in a terminal. If `~/.local/bin` isn't on your
`PATH`, the script says so. Add `export PATH="$HOME/.local/bin:$PATH"` to
`~/.bashrc` to fix it. If the icon doesn't show up right away, log out and
back in, because GNOME caches app icons.

The installed copy doesn't depend on the checkout, so you can move or delete
it. To **update**, pull and run the script again:

```
git pull && ./packaging/linux/install.sh
```

Other options:

```
./packaging/linux/install.sh --no-build    # install an existing target/release build
./packaging/linux/install.sh --uninstall   # remove everything the script installed
PREFIX=/opt/simple-design ./packaging/linux/install.sh   # install somewhere else
```

For a sandboxed install, a Flatpak manifest is included. See
[`docs/packaging.html`](docs/packaging.html).



## License

GNU GENERAL PUBLIC LICENSE Version 3

