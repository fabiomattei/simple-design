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

Open a specific file at startup so the CLI has something to talk to:

```
simple-design <file.sdesign>
```

## License

GNU GENERAL PUBLIC LICENSE Version 3

