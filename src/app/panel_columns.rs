//! The dockable side panels (Layers, Palette, Inspector, Minimap): each
//! window side's icon rail (`ui/panel_rail.rs`) plus the column its open
//! panels stack into, per the persisted `PanelLayout`.

use crate::panel_layout::{DockSide, PanelKind};
use crate::ui::inspector::{self, InspectorAction};
use crate::ui::{minimap_panel, palette_panel, panel_rail};

use super::App;

const PANEL_RAIL_WIDTH: f32 = 40.0;

impl App {
    /// Adds both sides' rails, then both sides' columns — rails first, so
    /// egui's panel stacking keeps each rail on the outer edge. Must run
    /// before the canvas's `CentralPanel`, which takes the space left over.
    pub(super) fn show_side_panels(&mut self, ui: &mut egui::Ui) {
        for side in [DockSide::Left, DockSide::Right] {
            self.show_panel_rail(ui, side);
        }
        for side in [DockSide::Left, DockSide::Right] {
            self.show_panel_column(ui, side);
        }
    }

    /// Hidden entirely while no panel is docked on `side`.
    fn show_panel_rail(&mut self, ui: &mut egui::Ui, side: DockSide) {
        let layout = self.panel_layout;
        if !PanelKind::ALL.iter().any(|&panel| layout.side(panel) == side) {
            return;
        }
        let mut moved = None;
        side_panel(side, "panel_rail").exact_size(PANEL_RAIL_WIDTH).show(ui, |ui| {
            let mut panels: Vec<(PanelKind, &mut bool)> = [
                (PanelKind::Layers, &mut self.show_layers_panel),
                (PanelKind::Palette, &mut self.show_palette_panel),
                (PanelKind::Inspector, &mut self.show_inspector_panel),
                (PanelKind::Minimap, &mut self.show_minimap_panel),
            ]
            .into_iter()
            .filter(|(panel, _)| layout.side(*panel) == side)
            .collect();
            moved = panel_rail::ui(ui, side, &mut panels);
        });
        if let Some(panel) = moved {
            self.panel_layout.set_side(panel, side.opposite());
        }
    }

    /// The open panels docked on `side` stack top-to-bottom in `PanelKind::ALL`
    /// order — see `panel_rail::stacked_panel`'s doc comment for the
    /// IntelliJ-style rationale. The bottommost one fills the remaining
    /// height with no dead space.
    fn show_panel_column(&mut self, ui: &mut egui::Ui, side: DockSide) {
        let open: Vec<PanelKind> = PanelKind::ALL
            .into_iter()
            .filter(|&panel| self.panel_layout.side(panel) == side && self.is_panel_open(panel))
            .collect();
        let Some(&bottommost) = open.last() else { return };
        side_panel(side, "panels_column").default_size(260.0).show(ui, |ui| {
            for &panel in &open {
                let is_last = panel == bottommost;
                match panel {
                    PanelKind::Layers => self.draw_layers_panel(ui, is_last),
                    PanelKind::Palette => self.draw_palette_panel(ui, is_last),
                    PanelKind::Inspector => {
                        let action = self.draw_inspector_panel(ui, is_last);
                        self.apply_inspector_action(action);
                    }
                    PanelKind::Minimap => self.draw_minimap_panel(ui),
                }
            }
        });
    }

    fn is_panel_open(&self, panel: PanelKind) -> bool {
        match panel {
            PanelKind::Layers => self.show_layers_panel,
            PanelKind::Palette => self.show_palette_panel,
            PanelKind::Inspector => self.show_inspector_panel,
            PanelKind::Minimap => self.show_minimap_panel,
        }
    }

    fn draw_layers_panel(&mut self, ui: &mut egui::Ui, is_last: bool) {
        // `stacked_panel`'s closure below calls several `self.foo(...)` methods, so
        // it must capture `self` as a whole — the height can't be borrowed straight
        // out of `self` for the same call without conflicting with that capture.
        let mut height = self.layers_panel_height;
        panel_rail::stacked_panel(ui, "layers_panel", &mut height, is_last, |ui| {
            let actions = self.layers_panel.ui(ui, self.history.get().active_page(), &mut self.selection);
            self.apply_layer_actions(actions);
        });
        self.layers_panel_height = height;
    }

    fn draw_palette_panel(&mut self, ui: &mut egui::Ui, is_last: bool) {
        let mut height = self.palette_panel_height;
        panel_rail::stacked_panel(ui, "palette_panel", &mut height, is_last, |ui| {
            let action = palette_panel::ui(ui, &self.history.get().palette, !self.selection.is_empty());
            self.apply_palette_action(action);
        });
        self.palette_panel_height = height;
    }

    fn draw_inspector_panel(&mut self, ui: &mut egui::Ui, is_last: bool) -> Option<InspectorAction> {
        let mut action = None;
        panel_rail::stacked_panel(ui, "inspector_panel", &mut self.inspector_panel_height, is_last, |ui| {
            // Unlike Layers/Palette, `inspector::ui`'s body doesn't scroll itself —
            // wrapped here (rather than inside `inspector::ui`) so a tall property
            // list can't get silently clipped by the panel's fixed stacked height.
            egui::ScrollArea::vertical().show(ui, |ui| {
                action = inspector::ui(ui, &mut self.history, &self.selection, &mut self.canvas, self.tool);
            });
        });
        action
    }

    fn apply_inspector_action(&mut self, action: Option<InspectorAction>) {
        match action {
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
    }

    /// Minimap is always last in `PanelKind::ALL`, so whenever it's open it's
    /// the bottommost panel of its column — it always fills the leftover
    /// height directly, with no height field or splitter of its own.
    fn draw_minimap_panel(&mut self, ui: &mut egui::Ui) {
        minimap_panel::ui(
            ui,
            self.history.get().active_page(),
            &mut self.canvas.pan,
            self.canvas.zoom,
            self.canvas.last_canvas_size,
        );
    }
}

/// A side panel on `side` named `id`. The right side keeps the bare `id` it
/// had before panels could dock left, so egui's remembered column width
/// carries over.
fn side_panel(side: DockSide, id: &'static str) -> egui::Panel {
    match side {
        DockSide::Left => egui::Panel::left(egui::Id::new((id, "left"))),
        DockSide::Right => egui::Panel::right(id),
    }
}
