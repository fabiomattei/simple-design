use std::path::PathBuf;

use egui::Context;

use crate::alignment::{self, AlignEdge, DistributeAxis};
use crate::boolean_ops;
use crate::canvas::CanvasWidget;
use crate::clipboard;
use crate::grouping;
use crate::history::History;
use crate::io;
use crate::model::{BoolOp, Document, Frame, Layer, LayerId, LayerKind, Paint};
use crate::palette_io;
use crate::tools::Tool;
use crate::transform_ops::{self, FlipAxis};
use crate::ui::inspector::InspectorAction;
use crate::ui::layers_panel::{LayerAction, LayersPanel};
use crate::ui::pages_bar::{PagesAction, PagesBar};
use crate::ui::palette_panel::PaletteAction;
use crate::ui::rename_all::{self, RenameAllAction, RenameAllConfig, RenameAllState};
use crate::ui::toolbar::ToolbarAction;
use crate::ui::{inspector, minimap_panel, palette_panel, toolbar};

pub struct App {
    history: History,
    canvas: CanvasWidget,
    layers_panel: LayersPanel,
    pages_bar: PagesBar,
    tool: Tool,
    selection: Vec<LayerId>,
    current_path: Option<PathBuf>,
    /// "Rotate Copies" popup state (Edit menu) — persisted across menu
    /// opens/closes so re-opening the menu remembers the last-used values.
    rotate_copies_count: u32,
    rotate_copies_degrees: f32,
    /// "Make Grid" popup state (Edit menu), same persist-across-opens
    /// convention as `rotate_copies_*` above.
    make_grid_cols: u32,
    make_grid_rows: u32,
    make_grid_gutter_x: f32,
    make_grid_gutter_y: f32,
    /// "Rename All" dialog state (Cmd+R on a multi-selection, or Edit >
    /// Rename All) — see `ui/rename_all.rs`.
    rename_all: RenameAllState,
    /// The last-applied `RenameAllConfig`, for Edit > "Rename with Last
    /// Format" (re-runs it against the current selection with no dialog).
    last_rename_config: Option<RenameAllConfig>,
    /// View menu toggle for the Palette panel (`ui/palette_panel.rs`) —
    /// runtime-only, not persisted with the document (same convention as
    /// `CanvasWidget::show_rulers`/`show_grid`).
    show_palette_panel: bool,
    /// View menu toggles for the Layers and Inspector panels — same
    /// runtime-only convention as `show_palette_panel`.
    show_layers_panel: bool,
    show_inspector_panel: bool,
    /// View menu toggle for the Minimap panel (`ui/minimap_panel.rs`) — an
    /// Aseprite-style bird's-eye view of the active page with a draggable
    /// viewport outline. Same runtime-only convention as `show_palette_panel`.
    show_minimap_panel: bool,
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        crate::fonts::install_fonts(&cc.egui_ctx);
        Self {
            history: History::new(Document::new()),
            canvas: CanvasWidget::default(),
            layers_panel: LayersPanel::default(),
            pages_bar: PagesBar::default(),
            tool: Tool::Select,
            selection: Vec::new(),
            current_path: None,
            rotate_copies_count: 5,
            rotate_copies_degrees: 360.0,
            make_grid_cols: 3,
            make_grid_rows: 3,
            make_grid_gutter_x: 20.0,
            make_grid_gutter_y: 20.0,
            rename_all: RenameAllState::default(),
            last_rename_config: None,
            show_palette_panel: false,
            show_layers_panel: true,
            show_inspector_panel: true,
            show_minimap_panel: true,
        }
    }

    /// Every layer on the active page, flattened (including nested
    /// descendants) — the candidate pool for `RenameAllState::items`, since
    /// `Scope::WholePage` needs to reach layers beyond the top level.
    fn all_layers_flat(&self) -> Vec<(LayerId, String, egui::Vec2)> {
        fn walk(layers: &[Layer], out: &mut Vec<(LayerId, String, egui::Vec2)>) {
            for l in layers {
                out.push((l.id, l.name.clone(), l.frame.bounds().size()));
                if let Some(children) = l.kind.children() {
                    walk(children, out);
                }
            }
        }
        let mut out = Vec::new();
        walk(&self.history.get().active_page().layers, &mut out);
        out
    }

    /// Opens the Rename All dialog. `default_whole_page`: Edit > Rename All
    /// with no selection defaults to whole-page scope (nothing to scope
    /// "Selected" to); Cmd+R on a multi-selection defaults to Selected.
    fn open_rename_all(&mut self, default_whole_page: bool) {
        self.rename_all.items = self.all_layers_flat();
        self.rename_all.selected_ids = self.selection.clone();
        self.rename_all.config = self.last_rename_config.clone().unwrap_or_default();
        self.rename_all.config.scope = if default_whole_page || self.selection.is_empty() {
            rename_all::Scope::WholePage { filter: String::new() }
        } else {
            rename_all::Scope::Selected
        };
        self.rename_all.open = true;
    }

    fn apply_renames(&mut self, names: Vec<(LayerId, String)>) {
        if names.is_empty() {
            return;
        }
        self.history.snapshot();
        let page = self.history.mutate().active_page_mut();
        for (id, name) in names {
            if let Some(l) = page.find_mut(id) {
                l.name = name;
            }
        }
    }

    /// Edit > "Rename with Last Format": re-runs the last-applied
    /// `RenameAllConfig` against the current selection, no dialog shown.
    fn rename_with_last_format(&mut self) {
        let Some(config) = self.last_rename_config.clone() else { return };
        if self.selection.is_empty() {
            return;
        }
        let page = self.history.get().active_page();
        let items: Vec<(LayerId, String, egui::Vec2)> = self
            .selection
            .iter()
            .filter_map(|&id| page.find(id).map(|l| (id, l.name.clone(), l.frame.bounds().size())))
            .collect();
        let names = rename_all::compute_names(&items, &config);
        self.apply_renames(names);
    }

    fn make_grid_selection(&mut self, cols: u32, rows: u32, gutter_x: f32, gutter_y: f32) {
        let [id] = self.selection[..] else { return };
        self.history.snapshot();
        let new_ids = crate::grid_ops::make_grid(self.history.mutate().active_page_mut(), id, cols, rows, gutter_x, gutter_y);
        if !new_ids.is_empty() {
            let mut all = vec![id];
            all.extend(new_ids);
            self.selection = all;
        }
    }

    fn new_document(&mut self) {
        self.history = History::new(Document::new());
        self.pages_bar = PagesBar::default();
        self.selection.clear();
        self.current_path = None;
    }

    fn group_selection(&mut self) {
        if self.selection.len() < 2 {
            return;
        }
        let ids = self.selection.clone();
        self.history.snapshot();
        if let Some(group_id) = grouping::group_layers(self.history.mutate().active_page_mut(), &ids) {
            self.selection = vec![group_id];
        }
    }

    fn flip_selection(&mut self, axis: FlipAxis) {
        if self.selection.is_empty() {
            return;
        }
        let ids = self.selection.clone();
        self.history.snapshot();
        transform_ops::flip_selection(self.history.mutate().active_page_mut(), &ids, axis);
    }

    fn rotate_copies_selection(&mut self, count: u32, degrees: f32) {
        if self.selection.is_empty() || count == 0 {
            return;
        }
        let ids = self.selection.clone();
        self.history.snapshot();
        let new_ids = transform_ops::rotate_copies(self.history.mutate().active_page_mut(), &ids, count, degrees);
        if !new_ids.is_empty() {
            self.selection = new_ids;
        }
    }

    fn flatten_selection(&mut self) {
        if self.selection.is_empty() {
            return;
        }
        let ids = self.selection.clone();
        self.history.snapshot();
        transform_ops::flatten_selection(self.history.mutate().active_page_mut(), &ids);
    }

    fn boolean_op_selection(&mut self, op: BoolOp) {
        if self.selection.len() < 2 {
            return;
        }
        let ids = self.selection.clone();
        self.history.snapshot();
        if let Some(id) = boolean_ops::create_boolean_group(self.history.mutate().active_page_mut(), &ids, op) {
            self.selection = vec![id];
        }
    }

    /// Applies a `PaletteAction` from `ui/palette_panel.rs` — same
    /// "panel returns an action, `app.rs` applies it" convention as
    /// `LayerAction`/`PagesAction`.
    fn apply_palette_action(&mut self, action: Option<PaletteAction>) {
        match action {
            None => {}
            Some(PaletteAction::Apply(color)) => {
                if self.selection.is_empty() {
                    return;
                }
                self.history.snapshot();
                let page = self.history.mutate().active_page_mut();
                for &id in &self.selection {
                    if let Some(l) = page.find_mut(id) {
                        l.style.fill = Some(Paint::Solid(color));
                    }
                }
            }
            Some(PaletteAction::AddFromSelection) => {
                let Some(color) =
                    self.selection.first().and_then(|&id| self.history.get().find(id)).and_then(|l| {
                        l.style.fill.as_ref().map(Paint::to_color32)
                    })
                else {
                    return;
                };
                self.history.snapshot();
                self.history.mutate().palette.add(color, String::new());
            }
            Some(PaletteAction::Remove(index)) => {
                self.history.snapshot();
                self.history.mutate().palette.remove(index);
            }
            Some(PaletteAction::ResetToDefault) => {
                self.history.snapshot();
                self.history.mutate().palette = crate::model::Palette::default();
            }
            Some(PaletteAction::Import) => {
                let Some(path) = io::open_palette_dialog() else { return };
                let Ok(loaded) = palette_io::load_gpl(&path) else { return };
                self.history.snapshot();
                self.history.mutate().palette = loaded;
            }
            Some(PaletteAction::Export) => {
                let default_name = format!("{}.gpl", self.history.get().name);
                let Some(path) = io::save_palette_dialog(&default_name) else { return };
                let doc = self.history.get();
                let _ = palette_io::save_gpl(&path, &doc.palette, &doc.name);
            }
        }
    }

    fn selection_is_single_text(&self) -> bool {
        matches!(self.selection[..], [id] if matches!(
            self.history.get().find(id).map(|l| &l.kind),
            Some(LayerKind::Text { .. })
        ))
    }

    /// "Convert to Outlines": replaces the single selected `Text` layer's
    /// `kind` in place with the `CompoundPath` of its glyph shapes (see
    /// `text_outline::convert_to_outlines`) — keeps id/frame/style/name, so
    /// selection and the layer-list entry survive the swap. Destructive
    /// (no longer editable as text) and irreversible except via undo.
    fn convert_selection_to_outlines(&mut self) {
        let [id] = self.selection[..] else { return };
        let Some(layer) = self.history.get().find(id) else { return };
        if !matches!(layer.kind, LayerKind::Text { .. }) {
            return;
        }
        let Some(polygons) = crate::text_outline::convert_to_outlines(layer) else { return };
        self.history.snapshot();
        if let Some(l) = self.history.mutate().active_page_mut().find_mut(id) {
            l.kind = LayerKind::CompoundPath { polygons };
        }
    }

    /// `to_artboard`: Option/Alt was held on the align button — override the
    /// default (align to the selection's own combined bounds) to align
    /// against the nearest enclosing Artboard instead. A reference layer set
    /// via `canvas.rs`'s "click an already-fully-selected layer again" takes
    /// priority over both when present and still part of the selection.
    fn align_selection(&mut self, edge: AlignEdge, to_artboard: bool) {
        if self.selection.len() < 2 {
            return;
        }
        let page = self.history.get().active_page();
        let align_to = self
            .canvas
            .reference_layer
            .filter(|id| self.selection.contains(id))
            .and_then(|id| page.find(id))
            .map(|l| l.frame.rotated_bounds())
            .or_else(|| to_artboard.then(|| alignment::artboard_bounds_in_parent_space(page, self.selection[0])).flatten());
        self.history.snapshot();
        alignment::align(self.history.mutate().active_page_mut(), &self.selection, edge, align_to);
    }

    fn distribute_selection(&mut self, axis: DistributeAxis) {
        if self.selection.len() < 3 {
            return;
        }
        self.history.snapshot();
        alignment::distribute(self.history.mutate().active_page_mut(), &self.selection, axis);
        self.canvas.last_distributed = Some((self.selection.clone(), axis));
    }

    /// Plain-arrow-key nudge: moves every selected layer's `frame.pos` by
    /// `delta` (1px, or 10px with Shift — see `handle_shortcuts`).
    fn nudge_selection(&mut self, delta: egui::Vec2) {
        if self.selection.is_empty() {
            return;
        }
        self.history.snapshot();
        let page = self.history.mutate().active_page_mut();
        for id in &self.selection {
            if let Some(l) = page.find_mut(*id) {
                l.frame.pos += delta;
            }
        }
    }

    /// Cmd+arrow-key nudge: resizes every selected layer's `frame.size` by
    /// `delta` instead of moving it (same 1px/10px step sizes).
    fn resize_selection_nudge(&mut self, delta: egui::Vec2) {
        if self.selection.is_empty() {
            return;
        }
        self.history.snapshot();
        let page = self.history.mutate().active_page_mut();
        for id in &self.selection {
            if let Some(l) = page.find_mut(*id) {
                l.frame.size += delta;
            }
        }
    }

    fn tidy_selection(&mut self, spacing: f32) {
        if self.selection.len() < 2 {
            return;
        }
        self.history.snapshot();
        alignment::tidy(self.history.mutate().active_page_mut(), &self.selection, spacing);
    }

    fn duplicate_selection(&mut self) {
        self.duplicate_selection_placed(false);
    }

    /// `below`: the Shift+Cmd+D / Edit > Duplicate shortcut — inserts each copy
    /// immediately *before* its original instead of after.
    fn duplicate_selection_placed(&mut self, below: bool) {
        if self.selection.is_empty() {
            return;
        }
        let ids = self.selection.clone();
        let offset = self.canvas.duplicate_offset;
        self.history.snapshot();
        let page = self.history.mutate().active_page_mut();
        let new_ids = if below {
            grouping::duplicate_layers_below(page, &ids, offset)
        } else {
            grouping::duplicate_layers(page, &ids, offset)
        };
        if !new_ids.is_empty() {
            self.selection = new_ids;
        }
    }

    /// Cmd+C: copies the current selection to the OS clipboard. See
    /// `clipboard::copy_selection`/`paste_selection` — shared with
    /// `canvas.rs`'s right-click Copy/Paste/Paste Over menu so both stay in
    /// sync with exactly one implementation.
    fn copy_selection(&mut self) {
        clipboard::copy_selection(&self.history, &self.selection);
    }

    /// Cmd+V ("Paste") / Shift+Cmd+V ("Paste Over") — see
    /// `clipboard::paste_selection` for the exact positioning/targeting
    /// rules.
    fn paste_selection(&mut self, over: bool) {
        clipboard::paste_selection(&mut self.history, &mut self.selection, self.canvas.duplicate_offset, over);
    }

    /// The Cmd/Ctrl+Shift+M "Use as Mask" shortcut — same effect as the
    /// layers-panel context menu's checkbox (`LayerAction::ToggleMask`), but
    /// keyed off the current selection instead of a single clicked row, so
    /// it can flip more than one layer at once.
    fn toggle_mask_selection(&mut self) {
        if self.selection.is_empty() {
            return;
        }
        let ids = self.selection.clone();
        self.history.snapshot();
        let page = self.history.mutate().active_page_mut();
        for id in ids {
            if let Some(l) = page.find_mut(id) {
                l.is_mask = !l.is_mask;
            }
        }
    }

    fn ungroup_selection(&mut self) {
        if self.selection.len() != 1 {
            return;
        }
        let id = self.selection[0];
        self.history.snapshot();
        let ids = grouping::ungroup(self.history.mutate().active_page_mut(), id);
        if !ids.is_empty() {
            self.selection = ids;
        }
    }

    fn save(&mut self, save_as: bool) {
        let path = if save_as || self.current_path.is_none() {
            let default_name = format!("{}.sdesign", self.history.get().name);
            io::save_dialog(&default_name)
        } else {
            self.current_path.clone()
        };
        if let Some(path) = path {
            if io::save_to(&path, self.history.get()).is_ok() {
                self.current_path = Some(path);
            }
        }
    }

    fn open(&mut self) {
        if let Some(path) = io::open_dialog() {
            if let Ok(doc) = io::load_from(&path) {
                self.history.replace(doc);
                self.pages_bar = PagesBar::default();
                self.selection.clear();
                self.current_path = Some(path);
            }
        }
    }

    /// Doc-space anchor for a fresh "Insert Image" batch: to the right of
    /// whatever's already on the page (or the top-left corner if it's
    /// empty), so repeated inserts don't all land in the exact same spot.
    fn default_insert_anchor(&self) -> egui::Pos2 {
        let page = self.history.get().active_page();
        let max_right = page
            .layers
            .iter()
            .map(|l| l.frame.bounds().max.x)
            .fold(0.0_f32, f32::max);
        egui::Pos2::new(if max_right > 0.0 { max_right + 40.0 } else { 40.0 }, 40.0)
    }

    /// Inserts a fixed-size Artboard chosen from the Inspector's preset
    /// picker (`ui::inspector::artboard_preset_picker`) — the click-to-place
    /// counterpart to dragging a custom-size one out on the canvas.
    fn insert_artboard_preset(&mut self, size: egui::Vec2) {
        let anchor = self.default_insert_anchor();
        let layer = Layer::new_artboard("Artboard", Frame { pos: anchor, size, rotation: 0.0 });
        self.history.snapshot();
        let new_id = layer.id;
        self.history.mutate().active_page_mut().layers.push(layer);
        self.selection = vec![new_id];
        self.tool = Tool::Select;
    }

    fn insert_images_via_dialog(&mut self) {
        let Some(paths) = io::open_image_dialog() else { return };
        if paths.is_empty() {
            return;
        }
        let anchor = self.default_insert_anchor();
        let layers = crate::image_ops::build_image_grid(&paths, anchor, 320.0, 20.0);
        if layers.is_empty() {
            return;
        }
        self.history.snapshot();
        let new_ids: Vec<LayerId> = layers.iter().map(|l| l.id).collect();
        let page = self.history.mutate().active_page_mut();
        let hint = self.canvas.insert_hint_parent.take();
        let hint_abs_pos = hint.and_then(|id| {
            page.absolute_offset(id).zip(page.find(id)).map(|(anc, l)| l.frame.pos.to_vec2() + anc)
        });
        match hint.zip(hint_abs_pos).and_then(|(id, abs)| page.find_mut(id).and_then(|l| l.kind.children_mut()).map(|c| (c, abs))) {
            Some((children, abs)) => {
                for mut l in layers {
                    l.frame.pos -= abs;
                    children.push(l);
                }
            }
            None => page.layers.extend(layers),
        }
        self.selection = new_ids;
    }

    /// "Reduce File Size": applies "Minimize File Size" to every `Image`
    /// layer in the document (every page, including nested ones inside
    /// Artboards/Groups) — the document-wide counterpart to the inspector's
    /// per-layer "Minimize File Size" button.
    fn reduce_file_size(&mut self) {
        self.history.snapshot();
        for page in &mut self.history.mutate().pages {
            for layer in &mut page.layers {
                minimize_image_file_size_recursive(layer);
            }
        }
    }

    fn export_selection_png(&mut self) {
        let [id] = self.selection.as_slice() else {
            return;
        };
        let Some(layer) = self.history.get().active_page().find(*id) else {
            return;
        };
        if let Some(path) = io::export_png_dialog(&format!("{}.png", layer.name)) {
            let _ = crate::export::export_layer_to_png(layer, &path);
        }
    }

    fn handle_shortcuts(&mut self, ctx: &Context) {
        let any_focused = ctx.memory(|m| m.focused().is_some());
        if !any_focused {
            ctx.input(|i| {
                for event in &i.events {
                    if let egui::Event::Key {
                        key,
                        pressed: true,
                        modifiers,
                        ..
                    } = event
                    {
                        // Shift-excluded too (not just Cmd/Ctrl): every
                        // `Tool::shortcut` key is a single unmodified letter,
                        // so e.g. plain `H` must not also fire when the user
                        // is holding Shift for a Shift+letter command below
                        // (⇧H/⇧V flip) — see the flip-shortcut block just
                        // past this closure.
                        if modifiers.command || modifiers.ctrl || modifiers.shift {
                            continue;
                        }
                        if let Some(t) = Tool::shortcut(key) {
                            self.tool = t;
                        }
                    }
                }
            });
        }

        let shift = ctx.input(|i| i.modifiers.shift);
        if ctx.input(|i| i.key_pressed(egui::Key::Z) && i.modifiers.command && !i.modifiers.shift) {
            self.history.undo();
        }
        if ctx.input(|i| i.key_pressed(egui::Key::Z) && i.modifiers.command && i.modifiers.shift)
            || ctx.input(|i| i.key_pressed(egui::Key::Y) && i.modifiers.command)
        {
            self.history.redo();
        }
        if ctx.input(|i| i.key_pressed(egui::Key::S) && i.modifiers.command) {
            self.save(shift);
        }
        if ctx.input(|i| i.key_pressed(egui::Key::G) && i.modifiers.command && !i.modifiers.shift) {
            self.group_selection();
        }
        if ctx.input(|i| i.key_pressed(egui::Key::G) && i.modifiers.command && i.modifiers.shift) {
            self.ungroup_selection();
        }
        if ctx.input(|i| i.key_pressed(egui::Key::D) && i.modifiers.command && i.modifiers.shift) {
            self.duplicate_selection_placed(true);
        } else if ctx.input(|i| i.key_pressed(egui::Key::D) && i.modifiers.command) {
            self.duplicate_selection();
        }
        if !any_focused && ctx.input(|i| i.key_pressed(egui::Key::C) && i.modifiers.command) {
            self.copy_selection();
        }
        if !any_focused && ctx.input(|i| i.key_pressed(egui::Key::V) && i.modifiers.command && i.modifiers.shift) {
            self.paste_selection(true);
        } else if !any_focused && ctx.input(|i| i.key_pressed(egui::Key::V) && i.modifiers.command) {
            self.paste_selection(false);
        }
        // Cmd+A: select every layer in the current selection's parent group
        // (or the whole page if nothing/multiple parents are selected) — see
        // `grouping::siblings_of`. Skipped when the selection is a single
        // Path, since `canvas.rs` already binds Cmd+A there to "select every
        // point on this path" (the common direct-selection-mode meaning).
        if !any_focused && ctx.input(|i| i.key_pressed(egui::Key::A) && i.modifiers.command) {
            let is_single_path = matches!(
                self.selection[..],
                [id] if matches!(self.history.get().active_page().find(id).map(|l| &l.kind), Some(LayerKind::Path { .. }))
            );
            if !is_single_path {
                self.selection = grouping::siblings_of(self.history.get().active_page(), &self.selection);
            }
        }
        if ctx.input(|i| i.key_pressed(egui::Key::M) && i.modifiers.command && i.modifiers.shift) {
            self.toggle_mask_selection();
        }
        if !any_focused
            && ctx.input(|i| i.key_pressed(egui::Key::H) && i.modifiers.shift && !i.modifiers.command)
        {
            self.flip_selection(FlipAxis::Horizontal);
        }
        if !any_focused
            && ctx.input(|i| i.key_pressed(egui::Key::V) && i.modifiers.shift && !i.modifiers.command)
        {
            self.flip_selection(FlipAxis::Vertical);
        }
        if ctx.input(|i| i.key_pressed(egui::Key::E) && i.modifiers.command) {
            self.export_selection_png();
        }
        if !any_focused
            && ctx.input(|i| i.key_pressed(egui::Key::O) && i.modifiers.command && i.modifiers.alt)
        {
            self.convert_selection_to_outlines();
        }
        if !any_focused && ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            self.selection.clear();
        }
        if !any_focused
            && ctx.input(|i| i.key_pressed(egui::Key::L) && i.modifiers.command && i.modifiers.alt)
        {
            self.canvas.aspect_locked = !self.canvas.aspect_locked;
        }
        // Arrow-key nudge: plain arrows move the selection (1px, 10px with
        // Shift); Cmd+arrows resize it instead (Right/Left -> width,
        // Up/Down -> height), same two step sizes. Skipped while editing
        // path points directly (no arrow-key point-nudge exists here, so
        // this at least avoids fighting some future point-editing use of
        // arrows) or while any widget has keyboard focus.
        if !any_focused && !self.canvas.has_point_selection() {
            let step = if ctx.input(|i| i.modifiers.shift) { 10.0 } else { 1.0 };
            let cmd = ctx.input(|i| i.modifiers.command);
            for (key, delta) in [
                (egui::Key::ArrowRight, egui::Vec2::new(step, 0.0)),
                (egui::Key::ArrowLeft, egui::Vec2::new(-step, 0.0)),
                (egui::Key::ArrowDown, egui::Vec2::new(0.0, step)),
                (egui::Key::ArrowUp, egui::Vec2::new(0.0, -step)),
            ] {
                if ctx.input(|i| i.key_pressed(key)) {
                    if cmd {
                        self.resize_selection_nudge(delta);
                    } else {
                        self.nudge_selection(delta);
                    }
                }
            }
        }
        // Cmd+R: single selection renames inline in the layers panel (same
        // state double-click/right-click-Rename use); multi-selection opens
        // Rename All pre-scoped to the selection, since sequential inline
        // rename isn't meaningful for more than one layer at once.
        if !any_focused && ctx.input(|i| i.key_pressed(egui::Key::R) && i.modifiers.command) {
            match self.selection[..] {
                [id] => {
                    if let Some(name) = self.history.get().active_page().find(id).map(|l| l.name.clone()) {
                        self.layers_panel.start_renaming(id, name);
                    }
                }
                [] => {}
                _ => self.open_rename_all(false),
            }
        }
        // K enters Scale mode on the current selection (see
        // `CanvasWidget::scaling`'s doc comment) — same "no modifiers"
        // exclusion as every other single-letter tool shortcut.
        if !any_focused
            && !self.selection.is_empty()
            && ctx.input(|i| i.key_pressed(egui::Key::K) && !i.modifiers.command && !i.modifiers.shift && !i.modifiers.alt)
        {
            self.canvas.scaling = Some(self.selection.clone());
        }
        // Enter/Return exits Scale mode if active (`⏎`/"Finish"),
        // else starts in-place canvas editing on a single selected `Text`
        // layer (`↵`/`⌘↵` — double-click is the other entry point,
        // handled directly in `CanvasWidget::ui`).
        if !any_focused && self.tool == Tool::Select && ctx.input(|i| i.key_pressed(egui::Key::Enter)) {
            if self.canvas.scaling.is_some() {
                self.canvas.scaling = None;
            } else if let [id] = self.selection[..] {
                let is_text = matches!(
                    self.history.get().active_page().find(id).map(|l| &l.kind),
                    Some(LayerKind::Text { .. })
                );
                if is_text {
                    self.canvas.start_editing_text(&mut self.history, id);
                }
            }
        }
        if !any_focused && ctx.input(|i| i.key_pressed(egui::Key::Delete) || i.key_pressed(egui::Key::Backspace)) {
            if self.canvas.has_point_selection() {
                self.canvas.delete_selected_points(&mut self.history, &mut self.selection);
            } else if !self.selection.is_empty() {
                let ids = std::mem::take(&mut self.selection);
                self.history.snapshot();
                let page = self.history.mutate().active_page_mut();
                // Captured before removal: for a single layer deleted from
                // inside a Group/Artboard/BooleanGroup, this lets us select
                // whatever now occupies its old spot (a common
                // auto-select-next behavior) instead of leaving the
                // selection empty.
                let context = if let [id] = ids[..] { grouping::parent_and_index(&page.layers, id) } else { None };
                for id in &ids {
                    page.remove(*id);
                }
                if let Some((Some(parent_id), index)) = context {
                    let next_selected = page.find(parent_id).and_then(|parent| {
                        let children = parent.kind.children()?;
                        children.get(index).or(children.last()).map(|l| l.id)
                    });
                    self.selection = vec![next_selected.unwrap_or(parent_id)];
                    // A shape/text layer drawn next lands back in this
                    // same group — see `insert_layer`'s `hint` param.
                    self.canvas.insert_hint_parent = Some(parent_id);
                }
            }
        }
    }
}

fn minimize_image_file_size_recursive(layer: &mut Layer) {
    crate::image_ops::minimize_image_file_size(layer);
    if let Some(children) = layer.kind.children_mut() {
        for child in children {
            minimize_image_file_size_recursive(child);
        }
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.handle_shortcuts(&ctx);

        egui::Panel::top("menu_bar").show(ui, |ui| {
            egui::MenuBar::new().ui(ui, |ui| {
                ui.menu_button("File", |ui| {
                    if ui.button("New").clicked() {
                        self.new_document();
                        ui.close();
                    }
                    if ui.button("Open...").clicked() {
                        self.open();
                        ui.close();
                    }
                    if ui.button("Save").clicked() {
                        self.save(false);
                        ui.close();
                    }
                    if ui.button("Save As...").clicked() {
                        self.save(true);
                        ui.close();
                    }
                    ui.separator();
                    if ui
                        .add_enabled(
                            self.selection.len() == 1,
                            egui::Button::new("Export Selection as PNG..."),
                        )
                        .clicked()
                    {
                        self.export_selection_png();
                        ui.close();
                    }
                    ui.separator();
                    if ui
                        .button("Reduce File Size")
                        .on_hover_text("Resamples every image in the document down to its on-screen pixel size.")
                        .clicked()
                    {
                        self.reduce_file_size();
                        ui.close();
                    }
                });
                ui.menu_button("Insert", |ui| {
                    for (label, t) in [
                        ("Artboard", Tool::Artboard),
                        ("Rectangle", Tool::Rectangle),
                        ("Oval", Tool::Oval),
                        ("Line", Tool::Line),
                        ("Arrow", Tool::Arrow),
                        ("Star", Tool::Star),
                        ("Polygon", Tool::Polygon),
                        ("Text", Tool::Text),
                    ] {
                        if ui.button(label).clicked() {
                            self.tool = t;
                            ui.close();
                        }
                    }
                    ui.separator();
                    if ui.button("Image...").clicked() {
                        self.insert_images_via_dialog();
                        ui.close();
                    }
                });
                ui.menu_button("Edit", |ui| {
                    if ui
                        .add_enabled(self.history.can_undo(), egui::Button::new("Undo"))
                        .clicked()
                    {
                        self.history.undo();
                        ui.close();
                    }
                    if ui
                        .add_enabled(self.history.can_redo(), egui::Button::new("Redo"))
                        .clicked()
                    {
                        self.history.redo();
                        ui.close();
                    }
                    ui.separator();
                    if ui
                        .add_enabled(!self.selection.is_empty(), egui::Button::new("Duplicate (⌘D)"))
                        .clicked()
                    {
                        self.duplicate_selection();
                        ui.close();
                    }
                    if ui
                        .add_enabled(!self.selection.is_empty(), egui::Button::new("Duplicate Below (⇧⌘D)"))
                        .clicked()
                    {
                        self.duplicate_selection_placed(true);
                        ui.close();
                    }
                    ui.menu_button("Settings", |ui| {
                        ui.label("Layers");
                        ui.horizontal(|ui| {
                            ui.label("Duplicate offset:");
                            ui.add(egui::DragValue::new(&mut self.canvas.duplicate_offset.x).prefix("x: "));
                            ui.add(egui::DragValue::new(&mut self.canvas.duplicate_offset.y).prefix("y: "));
                        });
                    });
                    ui.separator();
                    if ui
                        .add_enabled(self.selection.len() >= 2, egui::Button::new("Group"))
                        .clicked()
                    {
                        self.group_selection();
                        ui.close();
                    }
                    if ui
                        .add_enabled(self.selection.len() == 1, egui::Button::new("Ungroup"))
                        .clicked()
                    {
                        self.ungroup_selection();
                        ui.close();
                    }
                    ui.separator();
                    if ui
                        .add_enabled(!self.selection.is_empty(), egui::Button::new("Use as Mask (⌘⇧M)"))
                        .clicked()
                    {
                        self.toggle_mask_selection();
                        ui.close();
                    }
                    ui.separator();
                    if ui
                        .add_enabled(!self.selection.is_empty(), egui::Button::new("Flip Horizontal (⇧H)"))
                        .clicked()
                    {
                        self.flip_selection(FlipAxis::Horizontal);
                        ui.close();
                    }
                    if ui
                        .add_enabled(!self.selection.is_empty(), egui::Button::new("Flip Vertical (⇧V)"))
                        .clicked()
                    {
                        self.flip_selection(FlipAxis::Vertical);
                        ui.close();
                    }
                    ui.separator();
                    ui.add_enabled_ui(!self.selection.is_empty(), |ui| {
                        ui.label("Rotate Copies");
                        ui.horizontal(|ui| {
                            ui.add(egui::DragValue::new(&mut self.rotate_copies_count).prefix("Count: ").range(1..=100));
                            ui.add(
                                egui::DragValue::new(&mut self.rotate_copies_degrees)
                                    .prefix("Angle: ")
                                    .suffix("°"),
                            );
                            if ui.button("Apply").clicked() {
                                self.rotate_copies_selection(self.rotate_copies_count, self.rotate_copies_degrees);
                                ui.close();
                            }
                        });
                    });
                    ui.separator();
                    ui.add_enabled_ui(self.selection.len() == 1, |ui| {
                        ui.label("Make Grid");
                        ui.horizontal(|ui| {
                            ui.add(egui::DragValue::new(&mut self.make_grid_cols).prefix("Cols: ").range(1..=50));
                            ui.add(egui::DragValue::new(&mut self.make_grid_rows).prefix("Rows: ").range(1..=50));
                        });
                        ui.horizontal(|ui| {
                            ui.add(egui::DragValue::new(&mut self.make_grid_gutter_x).prefix("Gutter X: "));
                            ui.add(egui::DragValue::new(&mut self.make_grid_gutter_y).prefix("Gutter Y: "));
                            if ui.button("Apply").clicked() {
                                self.make_grid_selection(
                                    self.make_grid_cols,
                                    self.make_grid_rows,
                                    self.make_grid_gutter_x,
                                    self.make_grid_gutter_y,
                                );
                                ui.close();
                            }
                        });
                    });
                    ui.separator();
                    for (label, op) in [
                        ("Union", BoolOp::Union),
                        ("Subtract", BoolOp::Subtract),
                        ("Intersect", BoolOp::Intersect),
                        ("Difference", BoolOp::Difference),
                        ("Add", BoolOp::Add),
                    ] {
                        if ui
                            .add_enabled(self.selection.len() >= 2, egui::Button::new(label))
                            .clicked()
                        {
                            self.boolean_op_selection(op);
                            ui.close();
                        }
                    }
                    if ui
                        .add_enabled(!self.selection.is_empty(), egui::Button::new("Flatten"))
                        .clicked()
                    {
                        self.flatten_selection();
                        ui.close();
                    }
                    ui.separator();
                    if ui
                        .add_enabled(
                            self.selection_is_single_text(),
                            egui::Button::new("Convert to Outlines"),
                        )
                        .clicked()
                    {
                        self.convert_selection_to_outlines();
                        ui.close();
                    }
                    ui.separator();
                    if ui.button("Rename All...").clicked() {
                        self.open_rename_all(true);
                        ui.close();
                    }
                    if ui
                        .add_enabled(
                            self.last_rename_config.is_some() && !self.selection.is_empty(),
                            egui::Button::new("Rename with Last Format"),
                        )
                        .clicked()
                    {
                        self.rename_with_last_format();
                        ui.close();
                    }
                });
                ui.menu_button("View", |ui| {
                    if ui.checkbox(&mut self.canvas.show_rulers, "Rulers").clicked() {
                        ui.close();
                    }
                    if ui.checkbox(&mut self.canvas.show_grid, "Pixel Grid").clicked() {
                        ui.close();
                    }
                    if ui.checkbox(&mut self.canvas.snap_enabled, "Snapping").clicked() {
                        ui.close();
                    }
                    if ui.checkbox(&mut self.show_palette_panel, "Palette").clicked() {
                        ui.close();
                    }
                    ui.separator();
                    if ui.checkbox(&mut self.show_layers_panel, "Layers").clicked() {
                        ui.close();
                    }
                    if ui.checkbox(&mut self.show_inspector_panel, "Inspector").clicked() {
                        ui.close();
                    }
                    if ui.checkbox(&mut self.show_minimap_panel, "Minimap").clicked() {
                        ui.close();
                    }
                    ui.separator();
                    let has_guides = !self.history.get().active_page().guides.is_empty();
                    if ui
                        .add_enabled(has_guides, egui::Button::new("Clear Guides"))
                        .clicked()
                    {
                        self.history.snapshot();
                        self.history.mutate().active_page_mut().guides.clear();
                        ui.close();
                    }
                });
            });
        });

        egui::Panel::top("toolbar").show(ui, |ui| {
            ui.add_space(4.0);
            let action = toolbar::ui(ui, &mut self.tool);
            ui.add_space(4.0);
            if let Some(ToolbarAction::InsertImage) = action {
                self.insert_images_via_dialog();
            }
        });

        egui::Panel::bottom("pages_bar").show(ui, |ui| {
            let doc = self.history.get();
            let mut action = None;
            egui::Sides::new().show(
                ui,
                |ui| {
                    action = self.pages_bar.ui(ui, &doc.pages, doc.active_page);
                },
                |ui| {
                    let mut zoom_pct = self.canvas.zoom * 100.0;
                    let resp = ui.add(
                        egui::DragValue::new(&mut zoom_pct)
                            .suffix("%")
                            .range(10.0..=800.0)
                            .speed(1.0),
                    );
                    if resp.changed() {
                        self.canvas.zoom = zoom_pct / 100.0;
                    }
                    resp.on_hover_text("Zoom: Ctrl + Scroll (or pinch), or drag/click to set");
                },
            );
            match action {
                Some(PagesAction::Select(idx)) => {
                    self.history.mutate().active_page = idx;
                    self.selection.clear();
                }
                Some(PagesAction::Add) => {
                    self.history.snapshot();
                    let n = self.history.get().pages.len() + 1;
                    self.history.mutate().add_page(format!("Page {n}"));
                    self.selection.clear();
                }
                Some(PagesAction::Rename(idx, name)) => {
                    self.history.snapshot();
                    if let Some(page) = self.history.mutate().pages.get_mut(idx) {
                        page.name = name;
                    }
                }
                Some(PagesAction::Delete(idx)) => {
                    self.history.snapshot();
                    self.history.mutate().remove_page(idx);
                    self.selection.clear();
                }
                None => {}
            }
        });

        if self.show_layers_panel {
            egui::Panel::left("layers_panel")
                .default_size(220.0)
                .show(ui, |ui| {
                    let actions =
                        self.layers_panel
                            .ui(ui, self.history.get().active_page(), &mut self.selection);
                    if !actions.is_empty() {
                        // Copy/Paste/PasteOver manage their own history snapshot
                        // (Copy doesn't mutate the document at all; Paste/
                        // PasteOver snapshot inside `paste_selection`) — folding
                        // them into this blanket pre-snapshot would either
                        // record a spurious no-op undo step (Copy) or double up
                        // (Paste/PasteOver).
                        let needs_snapshot = actions.iter().any(|a| {
                            !matches!(
                                a,
                                LayerAction::Copy(_) | LayerAction::Paste | LayerAction::PasteOver | LayerAction::Close
                            )
                        });
                        if needs_snapshot {
                            self.history.snapshot();
                        }
                        for action in actions {
                            match action {
                                LayerAction::Close => self.show_layers_panel = false,
                                LayerAction::Copy(id) => clipboard::copy_selection(&self.history, &[id]),
                                LayerAction::Paste => self.paste_selection(false),
                                LayerAction::PasteOver => self.paste_selection(true),
                                LayerAction::ToggleVisible(id) => {
                                    if let Some(l) = self.history.mutate().active_page_mut().find_mut(id) {
                                        l.visible = !l.visible;
                                    }
                                }
                                LayerAction::ToggleLocked(id) => {
                                    if let Some(l) = self.history.mutate().active_page_mut().find_mut(id) {
                                        l.locked = !l.locked;
                                    }
                                }
                                LayerAction::Delete(id) => {
                                    let page = self.history.mutate().active_page_mut();
                                    let context = grouping::parent_and_index(&page.layers, id);
                                    page.remove(id);
                                    self.selection.retain(|&s| s != id);
                                    if let Some((Some(parent_id), index)) = context {
                                        let next_selected = page.find(parent_id).and_then(|parent| {
                                            let children = parent.kind.children()?;
                                            children.get(index).or(children.last()).map(|l| l.id)
                                        });
                                        self.selection = vec![next_selected.unwrap_or(parent_id)];
                                        self.canvas.insert_hint_parent = Some(parent_id);
                                    }
                                }
                                LayerAction::Duplicate(id) => {
                                    let offset = self.canvas.duplicate_offset;
                                    let new_ids = grouping::duplicate_layers(
                                        self.history.mutate().active_page_mut(),
                                        &[id],
                                        offset,
                                    );
                                    if let Some(&new_id) = new_ids.first() {
                                        self.selection = vec![new_id];
                                    }
                                }
                                LayerAction::Rename(id, name) => {
                                    if let Some(l) = self.history.mutate().active_page_mut().find_mut(id) {
                                        l.name = name;
                                    }
                                }
                                LayerAction::ToggleMask(id) => {
                                    if let Some(l) = self.history.mutate().active_page_mut().find_mut(id) {
                                        l.is_mask = !l.is_mask;
                                    }
                                }
                                LayerAction::ToggleIgnoreMask(id) => {
                                    if let Some(l) = self.history.mutate().active_page_mut().find_mut(id) {
                                        l.ignore_mask = !l.ignore_mask;
                                    }
                                }
                                LayerAction::Move { id, new_parent, index } => {
                                    grouping::move_layer(self.history.mutate().active_page_mut(), id, new_parent, index);
                                }
                                LayerAction::SetChildBoolOp(id, op) => {
                                    if let Some(l) = self.history.mutate().active_page_mut().find_mut(id) {
                                        l.bool_op = op;
                                    }
                                }
                            }
                        }
                    }
                });
        }

        if self.show_palette_panel {
            egui::Panel::left("palette_panel")
                .default_size(220.0)
                .show(ui, |ui| {
                    let action = palette_panel::ui(ui, &self.history.get().palette, !self.selection.is_empty());
                    self.apply_palette_action(action);
                });
        }

        let mut inspector_action = None;
        if self.show_inspector_panel {
            egui::Panel::right("inspector_panel")
                .default_size(240.0)
                .show(ui, |ui| {
                    inspector_action = inspector::ui(ui, &mut self.history, &self.selection, &mut self.canvas, self.tool);
                });
        }
        match inspector_action {
            Some(InspectorAction::Close) => self.show_inspector_panel = false,
            Some(InspectorAction::Duplicate) => self.duplicate_selection(),
            Some(InspectorAction::Group) => self.group_selection(),
            Some(InspectorAction::Ungroup) => self.ungroup_selection(),
            Some(InspectorAction::Boolean(op)) => self.boolean_op_selection(op),
            Some(InspectorAction::Align(edge, to_artboard)) => self.align_selection(edge, to_artboard),
            Some(InspectorAction::Distribute(axis)) => self.distribute_selection(axis),
            Some(InspectorAction::Tidy(spacing)) => self.tidy_selection(spacing),
            Some(InspectorAction::Flip(axis)) => self.flip_selection(axis),
            Some(InspectorAction::Flatten) => self.flatten_selection(),
            Some(InspectorAction::InsertArtboardPreset(size)) => self.insert_artboard_preset(size),
            None => {}
        }

        if self.show_minimap_panel {
            egui::Panel::right("minimap_panel")
                .default_size(220.0)
                .show(ui, |ui| {
                    minimap_panel::ui(
                        ui,
                        self.history.get().active_page(),
                        &mut self.canvas.pan,
                        self.canvas.zoom,
                        self.canvas.last_canvas_size,
                    );
                });
        }

        egui::CentralPanel::default().show(ui, |ui| {
            self.canvas
                .ui(ui, &mut self.history, &mut self.tool, &mut self.selection);
        });

        if let Some(RenameAllAction::Apply(names)) = rename_all::ui(&ctx, &mut self.rename_all) {
            self.last_rename_config = Some(self.rename_all.config.clone());
            self.apply_renames(names);
        }
    }
}
