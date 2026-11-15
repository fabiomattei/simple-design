//! Clicks, key shortcuts and file drops handled outside the drag state
//! machine: OS image drops, Pen-tool clicks, path-point insert/delete/convert
//! clicks and point-type shortcuts, text double-click editing, Scissors cuts,
//! direct-selection point clicks, plain click-to-select, and scroll/zoom.

use egui::{Pos2, Response, Ui, Vec2};

use crate::history::History;
use crate::model::{
    LayerId, LayerKind, PathPoint, PointType,
};
use crate::tools::Tool;

use super::hit_test::{hit_test, hit_test_through_mask};
use super::layer_creation::insert_layer;
use super::{
    CanvasLayout, CanvasWidget, DragState, HoverState, PathPart, SelectionView,
};

impl CanvasWidget {
    /// OS drag-and-drop of image files onto the canvas — "drag images
    /// from your Mac or browser directly onto the canvas" (the
    /// primary insertion path). `dropped_files` is only non-empty on the
    /// one frame the OS drop event fires, so no manual dedup is needed.
    /// Files dropped outside `canvas_rect` (e.g. onto a side panel) are
    /// ignored rather than inserted at some arbitrary fallback position.
    pub(super) fn handle_dropped_images(
        &mut self,
        ui: &Ui,
        history: &mut History,
        selection: &mut Vec<LayerId>,
        layout: CanvasLayout,
    ) {
        let CanvasLayout { canvas_rect, origin, .. } = layout;
        let dropped_paths: Vec<std::path::PathBuf> =
            ui.ctx().input(|i| i.raw.dropped_files.iter().map(|f| f.path().to_path_buf()).collect());
        if !dropped_paths.is_empty()
            && let Some(drop_pos) = ui.ctx().input(|i| i.pointer.interact_pos())
                && canvas_rect.contains(drop_pos) {
                    let doc_pos = self.to_doc(origin, drop_pos);
                    let layers = crate::image_ops::build_image_grid(&dropped_paths, doc_pos, 320.0, 20.0);
                    if !layers.is_empty() {
                        history.snapshot();
                        let page = history.mutate().active_page_mut();
                        let new_ids: Vec<LayerId> = layers.iter().map(|l| l.id).collect();
                        let hint = self.insert_hint_parent.take();
                        for layer in layers {
                            insert_layer(page, layer, doc_pos, hint);
                        }
                        *selection = new_ids;
                    }
                }
    }

    /// Direct-selection point selection (`selected_points`) is scoped to
    /// whichever single Path layer it was made on. Drop it the moment
    /// that's no longer the active selection (a different layer/selection,
    /// multi-layer selection, or a tool switch away from Select), and on
    /// Escape.
    pub(super) fn scope_point_selection(&mut self, ui: &Ui, history: &History, tool: &Tool, selection: &[LayerId]) {
        let current_path_id = if *tool == Tool::Select && selection.len() == 1 {
            history
                .get()
                .active_page()
                .find(selection[0])
                .filter(|l| matches!(l.kind, LayerKind::Path { .. }))
                .map(|l| l.id)
        } else {
            None
        };
        if current_path_id != self.point_edit_layer {
            self.selected_points.clear();
        }
        self.point_edit_layer = current_path_id;
        if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
            self.selected_points.clear();
        }
    }

    /// "Edit Image" mode is scoped to one layer; drop it the moment that
    /// layer stops existing (deleted mid-edit) or stops being an `Image`
    /// (an undo landing on an earlier document state), or on Escape.
    pub(super) fn scope_image_edit(&mut self, ui: &Ui, history: &History) {
        if let Some(edit) = &self.image_edit {
            let still_image = matches!(
                history.get().active_page().find(edit.layer_id).map(|l| &l.kind),
                Some(LayerKind::Image { .. })
            );
            if !still_image {
                self.end_image_edit();
            }
        }
        if self.image_edit.is_some() && ui.input(|i| i.key_pressed(egui::Key::Escape)) {
            self.end_image_edit();
        }
    }

    /// Pen-tool bookkeeping that can commit/cancel the in-progress path
    /// runs first, before `ui()` paints and hit-tests the document from
    /// `history` for the rest of the frame — committing
    /// touches `history` mutably and may flip `*tool` back to `Select`.
    pub(super) fn handle_pen_input(
        &mut self,
        ui: &Ui,
        response: &Response,
        history: &mut History,
        tool: &mut Tool,
        selection: &mut Vec<LayerId>,
        origin: Pos2,
    ) {
        if *tool != Tool::Pen && self.pen.is_some() {
            // A tool switch away from Pen mid-path commits whatever was
            // drawn so far as an open path, rather than silently discarding it.
            self.finish_pen_path(history, selection, tool, false);
        }
        if self.pen.is_some() {
            if ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                self.finish_pen_path(history, selection, tool, false);
            } else if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                self.pen = None;
                self.drag = DragState::None;
            }
        }
        if *tool == Tool::Pen {
            if response.double_clicked() {
                self.finish_pen_path(history, selection, tool, false);
            } else if response.clicked() {
                let mouse = response.interact_pointer_pos().unwrap_or(origin);
                let doc_pos = self.to_doc(origin, mouse);
                if !self.try_close_pen_path(doc_pos, origin, history, selection, tool) {
                    self.pen.get_or_insert_with(Vec::new).push(PathPoint {
                        anchor: doc_pos,
                        handle_in: None,
                        handle_out: None,
                        point_type: PointType::Straight,
                        corner_radius: 0.0,
                    });
                }
            }
        }
    }

    /// Direct-selection editing of a single already-committed Path's
    /// points: double-click a segment to insert an anchor, Alt/Option-click
    /// an anchor to delete it. Dragging an existing anchor/handle is
    /// handled by the drag state machine instead (`begin_drag`/
    /// `continue_drag`/`end_drag`, via `HoverState::point`), since that
    /// only needs to read the document at drag start, not mutate it before
    /// the document is painted.
    pub(super) fn handle_path_point_clicks(
        &mut self,
        ui: &Ui,
        response: &Response,
        history: &mut History,
        tool: &Tool,
        selection: &mut [LayerId],
        origin: Pos2,
    ) {
        if *tool == Tool::Select && selection.len() == 1 {
            if response.double_clicked() {
                if let Some(mouse) = response.interact_pointer_pos() {
                    let doc_pos = self.to_doc(origin, mouse);
                    if !self.try_convert_anchor_to_curve(history, selection, doc_pos, origin) {
                        self.try_insert_path_point(history, selection, doc_pos, origin);
                    }
                }
            } else if response.clicked() && ui.input(|i| i.modifiers.alt)
                && let Some(mouse) = response.interact_pointer_pos() {
                    let doc_pos = self.to_doc(origin, mouse);
                    self.try_delete_path_point(history, selection, doc_pos, origin);
                }
            // Point-type shortcuts (1-4), applied to every point in
            // `selected_points`. Only meaningful once a point is selected, so
            // this can't misfire as e.g. a zoom-level or tool shortcut typed
            // elsewhere while a Path just happens to be selected.
            if !self.selected_points.is_empty() {
                self.handle_point_type_shortcuts(ui, history, selection[0]);
            }
        }
    }

    /// Number keys 1-4 switch every point in `selected_points` of
    /// `layer_id` to Straight/Mirrored/Asymmetric/Disconnected.
    fn handle_point_type_shortcuts(&mut self, ui: &Ui, history: &mut History, layer_id: LayerId) {
        let new_type = if ui.input(|i| i.key_pressed(egui::Key::Num1)) {
            Some(PointType::Straight)
        } else if ui.input(|i| i.key_pressed(egui::Key::Num2)) {
            Some(PointType::Mirror)
        } else if ui.input(|i| i.key_pressed(egui::Key::Num3)) {
            Some(PointType::Asymmetric)
        } else if ui.input(|i| i.key_pressed(egui::Key::Num4)) {
            Some(PointType::Disconnected)
        } else {
            None
        };
        if let Some(new_type) = new_type {
            self.apply_point_type(history, layer_id, new_type);
        }
    }

    /// Double-click a `Text` layer (any Select-tool double-click,
    /// regardless of what's currently selected) to start in-place canvas
    /// editing — the primary text-editing entry point, and its
    /// "click another text layer while editing" cross-layer convenience
    /// falls out naturally since this always re-targets `editing_text`.
    /// Double-clicking a bare `Rectangle`/`Oval` instead gives it a
    /// inline text label (see `add_shape_label`) and starts
    /// editing that immediately. Double-clicking a mask (which a single
    /// click selects, since it sits on top of what it clips) instead
    /// selects the content layer it reveals under the pointer — see
    /// `hit_test_through_mask` — and starts editing it in place if it's a
    /// `Text` layer.
    pub(super) fn handle_text_double_click(
        &mut self,
        response: &Response,
        history: &mut History,
        tool: &Tool,
        selection: &mut Vec<LayerId>,
        origin: Pos2,
    ) {
        if *tool == Tool::Select && response.double_clicked()
            && let Some(mouse) = response.interact_pointer_pos() {
                let doc_pos = self.to_doc(origin, mouse);
                let page = history.get().active_page();
                if let Some(mask_id) = hit_test(page, doc_pos)
                    && page.find(mask_id).is_some_and(|l| l.mask.enabled) {
                        if let Some(content_id) = hit_test_through_mask(page, mask_id, doc_pos) {
                            let is_text = matches!(page.find(content_id).map(|l| &l.kind), Some(LayerKind::Text { .. }));
                            *selection = vec![content_id];
                            if is_text {
                                self.start_editing_text(history, content_id);
                            }
                        }
                        return;
                    }
                if let Some(id) = hit_test(history.get().active_page(), doc_pos) {
                    let is_text =
                        matches!(history.get().active_page().find(id).map(|l| &l.kind), Some(LayerKind::Text { .. }));
                    let is_labelable_shape = matches!(
                        history.get().active_page().find(id).map(|l| &l.kind),
                        Some(LayerKind::Rectangle { .. } | LayerKind::Oval)
                    );
                    if is_text {
                        *selection = vec![id];
                        self.start_editing_text(history, id);
                    } else if is_labelable_shape
                        && let Some(text_id) = self.add_shape_label(history, id) {
                            *selection = vec![text_id];
                        }
                }
            }
    }

    /// Scissors tool: a plain click cuts the path under the pointer (see
    /// `try_scissor_path`) rather than starting a drag.
    pub(super) fn handle_scissors_click(
        &mut self,
        response: &Response,
        history: &mut History,
        tool: &Tool,
        selection: &mut Vec<LayerId>,
        origin: Pos2,
    ) {
        if *tool == Tool::Scissors && response.clicked()
            && let Some(mouse) = response.interact_pointer_pos() {
                let doc_pos = self.to_doc(origin, mouse);
                self.try_scissor_path(history, selection, doc_pos, origin);
            }
    }

    /// Direct-selection point selection: shift-click toggles a point in
    /// `selected_points`, a plain click replaces it, and Cmd/Ctrl+A
    /// selects every point of the active single Path. A drag starting on
    /// an anchor (in `begin_drag`) applies the same
    /// replace/extend rule so a click-that-becomes-a-drag behaves
    /// consistently with a plain click. Excludes double-click (segment
    /// insert) and Alt-click (point delete), which are handled earlier
    /// and shouldn't also toggle a selection.
    pub(super) fn handle_point_selection_clicks(
        &mut self,
        ui: &Ui,
        response: &Response,
        view: &SelectionView,
        tool: &Tool,
        hover: &HoverState,
    ) {
        if *tool == Tool::Select {
            if let Some((layer, _)) = view.single_path
                && ui.input(|i| i.modifiers.command && i.key_pressed(egui::Key::A))
                    && let LayerKind::Path { points, .. } = &layer.kind {
                        self.selected_points = (0..points.len()).collect();
                    }
            if response.clicked() && !response.double_clicked() && !ui.input(|i| i.modifiers.alt)
                && let Some((index, PathPart::Anchor)) = hover.point {
                    if ui.input(|i| i.modifiers.shift) {
                        if let Some(pos) = self.selected_points.iter().position(|&i| i == index) {
                            self.selected_points.remove(pos);
                        } else {
                            self.selected_points.push(index);
                        }
                    } else {
                        self.selected_points = vec![index];
                    }
                }
        }
    }

    /// A plain click (press+release with no perceptible pointer movement)
    /// never fires `drag_started()` — egui only classifies an interaction
    /// as a drag once movement crosses its threshold — so click-to-select
    /// needs its own path here rather than living solely in
    /// `begin_drag`, which only ever runs for an actual drag. Excludes anything with a dedicated hover target (handles,
    /// path points, gap handles) since those have no meaning without
    /// movement, and Edit Image mode, whose clicks are Magic Wand/selection
    /// within the image rather than canvas layer selection.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn handle_click_select(
        &mut self,
        ui: &Ui,
        response: &Response,
        history: &History,
        tool: &Tool,
        selection: &mut Vec<LayerId>,
        origin: Pos2,
        hover: &HoverState,
        is_single_path_selected: bool,
    ) {
        if *tool == Tool::Select
            && response.clicked()
            && !response.double_clicked()
            && self.image_edit.is_none()
            && hover.point.is_none()
            && hover.handle.is_none()
            && hover.rotate_corner.is_none()
            && hover.gap_handle.is_none()
            && !is_single_path_selected
        {
            let mouse = response.interact_pointer_pos().unwrap_or(origin);
            let doc_pos = self.to_doc(origin, mouse);
            let modifiers = ui.input(|i| i.modifiers);
            match hit_test(history.get().active_page(), doc_pos) {
                Some(id) => {
                    if modifiers.shift {
                        if selection.contains(&id) {
                            selection.retain(|&s| s != id);
                        } else {
                            selection.push(id);
                        }
                    } else if selection.len() == 1 && selection[0] == id {
                        self.reference_layer = if self.reference_layer == Some(id) { None } else { Some(id) };
                    } else {
                        *selection = vec![id];
                        self.reference_layer = None;
                    }
                }
                None => {
                    self.scaling = None;
                    if !modifiers.shift {
                        selection.clear();
                    }
                }
            }
        }
    }

    /// Zoom with ctrl/cmd+scroll or pinch; plain scroll pans.
    pub(super) fn handle_scroll_zoom(&mut self, ui: &Ui, response: &Response) {
        if response.hovered() {
            let scroll = ui.input(|i| i.smooth_scroll_delta);
            let zoom_delta = ui.input(|i| i.zoom_delta());
            if zoom_delta != 1.0 {
                self.zoom = (self.zoom * zoom_delta).clamp(0.1, 8.0);
            } else if scroll != Vec2::ZERO {
                self.pan += scroll;
            }
        }
    }
}
