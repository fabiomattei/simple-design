//! Right-click handling and the canvas's three popup menus: Copy/Paste,
//! the stacked-layer picker, and the path point-type picker.

use egui::{Pos2, Response, Ui, Vec2};

use crate::clipboard;
use crate::history::History;
use crate::model::{
    LayerId, LayerKind, PointType,
};
use crate::tools::Tool;

use super::hit_test::layers_at_point;
use super::CanvasWidget;

impl CanvasWidget {
    /// Right-click on an anchor of the active single Path: a point-type
    /// picker (mouse alternative to the Num1-4 shortcuts), taking
    /// priority over the Shift-overlap-picker/Copy-Paste menus below.
    /// Shift+right-click (anywhere else): open a picker menu of every
    /// layer stacked under the click point, instead of the normal single
    /// topmost selection. Plain right-click (anywhere else): a small
    /// Copy/Paste/Paste Over menu.
    pub(super) fn handle_secondary_click(
        &mut self,
        ui: &Ui,
        response: &Response,
        history: &History,
        tool: &Tool,
        selection: &[LayerId],
        origin: Pos2,
    ) {
        if *tool == Tool::Select && response.secondary_clicked() {
            let anchor_hit = response
                .interact_pointer_pos()
                .and_then(|mp| self.hit_test_path_anchor(history, selection, origin, mp));
            if let Some((layer_id, index)) = anchor_hit {
                if !self.selected_points.contains(&index) {
                    self.selected_points = vec![index];
                }
                self.point_type_menu = response.interact_pointer_pos().map(|mp| (mp, layer_id));
                self.canvas_menu = None;
                self.overlap_menu = None;
            } else if ui.input(|i| i.modifiers.shift) {
                if let Some(mp) = response.interact_pointer_pos() {
                    let doc_pos = self.to_doc(origin, mp);
                    let mut hits = Vec::new();
                    layers_at_point(&history.get().active_page().layers, Vec2::ZERO, doc_pos, &mut hits);
                    self.overlap_menu = if hits.is_empty() { None } else { Some((mp, hits)) };
                }
                self.canvas_menu = None;
                self.point_type_menu = None;
            } else {
                self.canvas_menu = response.interact_pointer_pos();
                self.overlap_menu = None;
                self.point_type_menu = None;
            }
        }
    }

    /// The plain right-click Copy/Paste/Paste Over menu, while `canvas_menu`
    /// is open; closes on a choice, any other click, or Escape.
    pub(super) fn show_canvas_menu(&mut self, ui: &Ui, history: &mut History, selection: &mut Vec<LayerId>) {
        if let Some(screen_pos) = self.canvas_menu {
            let mut close = false;
            egui::Area::new(egui::Id::new("canvas-context-menu"))
                .fixed_pos(screen_pos)
                .order(egui::Order::Foreground)
                .show(ui.ctx(), |ui| {
                    egui::Frame::popup(ui.style()).show(ui, |ui| {
                        if ui.add_enabled(!selection.is_empty(), egui::Button::new("Copy")).clicked() {
                            clipboard::copy_selection(history, selection);
                            close = true;
                        }
                        if ui.button("Paste").clicked() {
                            clipboard::paste_selection(history, selection, self.duplicate_offset, false);
                            close = true;
                        }
                        if ui.button("Paste Over").clicked() {
                            clipboard::paste_selection(history, selection, self.duplicate_offset, true);
                            close = true;
                        }
                    });
                });
            if close || ui.input(|i| i.pointer.button_clicked(egui::PointerButton::Primary) || i.key_pressed(egui::Key::Escape))
            {
                self.canvas_menu = None;
            }
        }
    }

    /// The Shift+right-click picker listing every layer stacked under the
    /// click; choosing one selects it.
    pub(super) fn show_overlap_menu(&mut self, ui: &Ui, history: &History, selection: &mut Vec<LayerId>) {
        if let Some((screen_pos, ids)) = self.overlap_menu.clone() {
            let mut clicked_id = None;
            let names: Vec<String> = ids
                .iter()
                .map(|id| {
                    history
                        .get()
                        .active_page()
                        .find(*id)
                        .map(|l| l.name.clone())
                        .unwrap_or_else(|| "Layer".to_string())
                })
                .collect();
            egui::Area::new(egui::Id::new("overlap-menu"))
                .fixed_pos(screen_pos)
                .order(egui::Order::Foreground)
                .show(ui.ctx(), |ui| {
                    egui::Frame::popup(ui.style()).show(ui, |ui| {
                        for (id, name) in ids.iter().zip(names.iter()) {
                            if ui.selectable_label(false, name).clicked() {
                                clicked_id = Some(*id);
                            }
                        }
                    });
                });
            if let Some(id) = clicked_id {
                *selection = vec![id];
                self.overlap_menu = None;
            } else if ui.input(|i| i.pointer.button_clicked(egui::PointerButton::Primary) || i.key_pressed(egui::Key::Escape))
            {
                self.overlap_menu = None;
            }
        }
    }

    /// Right-click point-type picker (mouse alternative to `apply_point_type`'s
    /// Num1-4 shortcuts): shown for the anchor that was right-clicked
    /// (see the `secondary_clicked` dispatch above, which also makes sure
    /// it's selected), applying to every point currently in
    /// `selected_points`, same as the keyboard shortcuts.
    pub(super) fn show_point_type_menu(&mut self, ui: &Ui, history: &mut History) {
        if let Some((screen_pos, layer_id)) = self.point_type_menu {
            let current_type = self.selected_points.first().and_then(|&index| {
                let doc = history.get();
                let LayerKind::Path { points, .. } = &doc.active_page().find(layer_id)?.kind else {
                    return None;
                };
                points.get(index).map(|p| p.point_type)
            });
            let mut chosen = None;
            egui::Area::new(egui::Id::new("point-type-menu"))
                .fixed_pos(screen_pos)
                .order(egui::Order::Foreground)
                .show(ui.ctx(), |ui| {
                    egui::Frame::popup(ui.style()).show(ui, |ui| {
                        for (label, point_type) in [
                            ("Straight", PointType::Straight),
                            ("Mirrored", PointType::Mirror),
                            ("Asymmetric", PointType::Asymmetric),
                            ("Disconnected", PointType::Disconnected),
                        ] {
                            if ui.selectable_label(current_type == Some(point_type), label).clicked() {
                                chosen = Some(point_type);
                            }
                        }
                    });
                });
            if let Some(point_type) = chosen {
                self.apply_point_type(history, layer_id, point_type);
            }
            if chosen.is_some()
                || ui.input(|i| i.pointer.button_clicked(egui::PointerButton::Primary) || i.key_pressed(egui::Key::Escape))
            {
                self.point_type_menu = None;
            }
        }
    }
}
