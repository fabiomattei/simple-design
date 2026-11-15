# simple-design
My personal project to create a design software

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

## Update Linux installation

> cargo build --release
> install -m755 target/release/simple-design ~/.local/bin/simple-design



## License

GNU GENERAL PUBLIC LICENSE Version 3

