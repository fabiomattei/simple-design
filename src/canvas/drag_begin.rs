//! Drag start: turns a press under the pointer into the right `DragState`
//! (guide create/move, pan, path point/handle edit, gap adjust, rotate,
//! resize, move, duplicate, marquee, pen handle, new shape).

use egui::{Modifiers, Pos2, Rect, Response, Ui, Vec2};

use crate::grouping;
use crate::history::History;
use crate::model::{
    GuideOrientation, Layer, LayerId, LayerKind, Page, PathPoint, PointType,
};
use crate::tools::Tool;

use super::hit_test::hit_test;
use super::transform_geometry::{
    ResizeContainerInfo, ResizeLayerInfo, collect_resizable_leaves, collect_rotatable_leaves,
    display_bounds,
};
use super::{
    CanvasLayout, CanvasWidget, DragState, Handle, HandleSide, HoverState, PathPart, SelectionView,
};

impl CanvasWidget {
    /// Starts the `DragState` for a press under the pointer, on the frame
    /// egui recognizes it as a drag. "Edit Image" mode captures every drag;
    /// otherwise the rulers and guides come first, then the active tool.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn begin_drag(
        &mut self,
        ui: &Ui,
        response: &Response,
        history: &mut History,
        tool: &mut Tool,
        selection: &mut Vec<LayerId>,
        layout: CanvasLayout,
        hover: &HoverState,
    ) {
        let CanvasLayout { canvas_rect, origin, .. } = layout;
        // Same reasoning as `hit_test_pos` in `paint_selection_overlay`: on this exact frame,
        // `interact_pointer_pos()` already reflects the pointer's
        // current (post-threshold) position, not where the button
        // actually went down. Every `DragState` that records a
        // `start_doc`/starting bounds to diff future positions against
        // needs the true press origin here, or the dragged content lags
        // behind the cursor by however far it moved before the drag was
        // recognized.
        let mouse = ui
            .input(|i| i.pointer.press_origin())
            .or_else(|| response.interact_pointer_pos())
            .unwrap_or(origin);
        let doc_pos = self.to_doc(origin, mouse);
        if let Some(edit) = &self.image_edit {
            // "Edit Image" mode captures every drag for
            // Selection/Magic Wand, bypassing the normal
            // guide/pan/select/pen/shape handling below entirely — it's
            // modal, same as the Pen tool's in-progress path.
            let subtract = ui.input(|i| i.modifiers.alt);
            let replace = !subtract && !ui.input(|i| i.modifiers.shift);
            let base_mask = if replace { vec![false; edit.mask.len()] } else { edit.mask.clone() };
            self.drag = DragState::ImageEditDrag { start_doc: doc_pos, subtract, base_mask };
            return;
        }
        if let Some(drag) = self.guide_drag_at(history, tool, canvas_rect, origin, mouse, hover) {
            self.drag = drag;
        } else if *tool == Tool::Pan {
            self.drag = DragState::PanningView {
                start_mouse: mouse,
                start_pan: self.pan,
            };
        } else if *tool == Tool::Select {
            self.begin_select_drag(ui, history, selection, doc_pos, hover);
        } else if *tool == Tool::Pen {
            self.begin_pen_drag(history, selection, tool, doc_pos, origin);
        } else if *tool == Tool::Scissors {
            // Cutting is a plain click, handled up front (alongside the
            // insert/delete-point click handling) rather than as a drag.
        } else {
            self.drag = DragState::CreatingShape { start_doc: doc_pos };
            history.snapshot();
        }
    }

    /// A drag starting in a ruler strip creates a guide; one starting on an
    /// existing guide (Select tool, not over a handle or path point) moves it.
    fn guide_drag_at(
        &self,
        history: &History,
        tool: &Tool,
        canvas_rect: Rect,
        origin: Pos2,
        mouse: Pos2,
        hover: &HoverState,
    ) -> Option<DragState> {
        let page = history.get().active_page();
        let in_top_ruler = self.show_rulers && mouse.y < canvas_rect.min.y && mouse.x >= canvas_rect.min.x;
        let in_left_ruler = self.show_rulers && mouse.x < canvas_rect.min.x && mouse.y >= canvas_rect.min.y;
        let guide_grab = if *tool == Tool::Select
            && canvas_rect.contains(mouse)
            && hover.point.is_none()
            && hover.handle.is_none()
        {
            self.hovered_guide_index(origin, &page.guides, mouse)
        } else {
            None
        };
        if in_top_ruler && !in_left_ruler {
            Some(DragState::CreatingGuide { orientation: GuideOrientation::Horizontal })
        } else if in_left_ruler && !in_top_ruler {
            Some(DragState::CreatingGuide { orientation: GuideOrientation::Vertical })
        } else {
            guide_grab.map(|index| DragState::MovingGuide { index, orientation: page.guides[index].orientation })
        }
    }

    /// Select-tool drag start, by priority: a path point or control handle,
    /// a Smart Distribute gap handle, the rotation handle, a resize handle, empty
    /// space around a single selected path (point marquee), then whatever
    /// layer — or empty canvas — is under the pointer.
    ///
    /// Re-resolves the page and selection from `history` rather than taking
    /// them from `ui()`: every branch that edits the document snapshots
    /// `history` first, which can't happen while a caller-held borrow of the
    /// page is alive. Nothing mutates `history` between the hover pass and
    /// here, so this resolves to exactly what the overlay was drawn from.
    fn begin_select_drag(
        &mut self,
        ui: &Ui,
        history: &mut History,
        selection: &mut Vec<LayerId>,
        doc_pos: Pos2,
        hover: &HoverState,
    ) {
        let page = history.get().active_page();
        let view = SelectionView::new(page, selection);
        if let (Some((point_index, part)), Some((layer, offset))) = (hover.point, view.single_path) {
            if let Some(drag) = self.path_point_drag(ui, layer, offset, point_index, part, doc_pos) {
                self.drag = drag;
                history.snapshot();
            }
        } else if let Some(gap_index) = hover.gap_handle {
            if let Some(drag) = self.gap_drag(page, &hover.gap_handle_order, gap_index, doc_pos) {
                self.drag = drag;
                history.snapshot();
            }
        } else if hover.rotate_handle {
            if let Some(drag) = rotate_drag(&view.layers, doc_pos) {
                self.drag = drag;
                history.snapshot();
            }
        } else if let Some(handle) = hover.handle {
            if let Some(drag) = self.resize_drag(page, &view, selection, handle) {
                self.drag = drag;
                history.snapshot();
            }
        } else if let Some((layer, _)) = view.single_path
            && hit_test(page, doc_pos) != Some(layer.id)
        {
            // Empty-space drag while a single Path is the active
            // selection: box-select its points instead of falling
            // through to layer marquee/hit-test. A drag starting on
            // the path's own body instead falls through to the
            // generic hit-test branch below so it moves the layer,
            // same as every other shape.
            self.drag = self.point_marquee_drag(ui, layer.id, doc_pos);
        } else {
            self.begin_layer_drag(ui, history, selection, doc_pos);
        }
    }

    fn path_point_drag(
        &mut self,
        ui: &Ui,
        layer: &Layer,
        offset: Vec2,
        point_index: usize,
        part: PathPart,
        doc_pos: Pos2,
    ) -> Option<DragState> {
        let layer_id = layer.id;
        match part {
            PathPart::Anchor => {
                if let LayerKind::Path { points, .. } = &layer.kind {
                    // Dragging an already-selected anchor moves the
                    // whole selection together; dragging an
                    // unselected one replaces the selection with
                    // just this point (shift extends it instead),
                    // matching `handle_point_selection_clicks`'s rule so a
                    // click-that-becomes-a-drag behaves the same way.
                    if !self.selected_points.contains(&point_index) {
                        if ui.input(|i| i.modifiers.shift) {
                            self.selected_points.push(point_index);
                        } else {
                            self.selected_points = vec![point_index];
                        }
                    }
                    let point_indices = self.selected_points.clone();
                    let start_anchors =
                        point_indices.iter().map(|&i| points[i].anchor).collect();
                    Some(DragState::EditingPathAnchor {
                        layer_id,
                        point_indices,
                        start_doc: doc_pos,
                        start_anchors,
                    })
                } else {
                    None
                }
            }
            PathPart::HandleIn | PathPart::HandleOut => {
                let side = if part == PathPart::HandleIn { HandleSide::In } else { HandleSide::Out };
                Some(DragState::EditingPathHandle {
                    layer_id,
                    parent_offset: offset,
                    point_index,
                    side,
                })
            }
        }
    }

    fn gap_drag(
        &self,
        page: &Page,
        gap_handle_order: &[(LayerId, Rect)],
        gap_index: usize,
        doc_pos: Pos2,
    ) -> Option<DragState> {
        let (_, axis) = self.last_distributed.clone().expect("hovered_gap_handle only set when last_distributed matches");
        let order: Vec<LayerId> = gap_handle_order.iter().map(|(id, _)| *id).collect();
        let starts: Vec<Pos2> = order
            .iter()
            .filter_map(|&id| page.find(id).map(|l| l.frame.pos))
            .collect();
        if starts.len() == order.len() {
            Some(DragState::AdjustingDistributionGap { start_doc: doc_pos, axis, gap_index, order, starts })
        } else {
            None
        }
    }

    fn resize_drag(
        &self,
        page: &Page,
        view: &SelectionView,
        selection: &[LayerId],
        handle: Handle,
    ) -> Option<DragState> {
        if view.single_line.is_some() {
            let id = selection[0];
            let offset = page.absolute_offset(id).unwrap_or(Vec2::ZERO);
            Some(DragState::ResizingLine { id, handle, parent_offset: offset })
        } else {
            let scale_style = self.scaling.is_some();
            let mut layers: Vec<ResizeLayerInfo> = Vec::new();
            let mut containers: Vec<ResizeContainerInfo> = Vec::new();
            for (l, o) in &view.layers {
                let is_artboard = matches!(l.kind, LayerKind::Artboard { .. });
                if is_artboard || (l.kind.children().is_some() && !scale_style) {
                    containers.push(ResizeContainerInfo { original: (*l).clone(), parent_offset: *o });
                } else {
                    collect_resizable_leaves(l, *o, &mut layers);
                }
            }
            let overall = layers
                .iter()
                .map(|l| l.abs_bounds)
                .chain(containers.iter().map(|c| c.original.frame.bounds().translate(c.parent_offset)))
                .reduce(|a, b| a.union(b))?;
            Some(DragState::ResizingGroup {
                handle,
                start_overall_bounds: overall,
                layers,
                containers,
                scale_style,
                scale_anchor: self.scale_anchor,
            })
        }
    }

    fn point_marquee_drag(&mut self, ui: &Ui, layer_id: LayerId, doc_pos: Pos2) -> DragState {
        let shift = ui.input(|i| i.modifiers.shift);
        if !shift {
            self.selected_points.clear();
        }
        DragState::MarqueePoints {
            layer_id,
            start_doc: doc_pos,
            additive: shift,
            base_selection: self.selected_points.clone(),
        }
    }

    /// Select-tool drag over a layer or empty canvas: move the selection,
    /// shift-toggle, Option-duplicate, or start a marquee.
    fn begin_layer_drag(&mut self, ui: &Ui, history: &mut History, selection: &mut Vec<LayerId>, doc_pos: Pos2) {
        let page = history.get().active_page();
        let modifiers = ui.input(|i| i.modifiers);
        if modifiers.command && modifiers.alt && !selection.is_empty() {
            // Cmd+Option+drag: move the current selection even
            // when it isn't the topmost layer under the cursor
            // ("move a layer buried under others"),
            // bypassing hit_test entirely.
            self.drag = DragState::MovingSelection {
                start_doc: doc_pos,
                starts: move_starts(page, selection),
            };
            history.snapshot();
        } else {
            match hit_test(page, doc_pos) {
                Some(id) => self.begin_hit_layer_drag(history, selection, modifiers, id, doc_pos),
                None => self.begin_marquee_drag(selection, modifiers, doc_pos),
            }
        }
    }

    fn begin_hit_layer_drag(
        &mut self,
        history: &mut History,
        selection: &mut Vec<LayerId>,
        modifiers: Modifiers,
        id: LayerId,
        doc_pos: Pos2,
    ) {
        let page = history.get().active_page();
        if modifiers.shift {
            // Shift+drag always area-selects, even when it starts on a
            // layer (or inside an artboard, which hit-tests as the
            // artboard). Shift+click toggling is handled on click instead.
            self.begin_marquee_drag(selection, modifiers, doc_pos);
        } else if modifiers.alt {
            self.begin_duplicate_drag(history, selection, id, doc_pos);
        } else {
            // Clicking an already-fully-selected single
            // layer again marks it as the alignment
            // "reference layer" (a common gesture)
            // instead of re-selecting it.
            if selection.len() == 1 && selection[0] == id {
                self.reference_layer = if self.reference_layer == Some(id) { None } else { Some(id) };
            } else if hit_is_within_selection(page, selection, id) {
                // The hit leaf is nested inside a
                // Group/Artboard that's already part of
                // the current selection (e.g. a masked
                // picture's mask, both children of a
                // selected Group) — hit_test always
                // drills down to the leaf, but dragging
                // here should move the whole selected
                // ancestor, not silently narrow the
                // selection down to just the one child
                // under the cursor.
            } else {
                *selection = vec![id];
                self.reference_layer = None;
            }
            self.drag = DragState::MovingSelection {
                start_doc: doc_pos,
                starts: move_starts(page, selection),
            };
            history.snapshot();
        }
    }

    /// Option+drag: duplicate the selection in
    /// place (no offset — the drag itself moves
    /// it away) and drag the copies, leaving the
    /// originals untouched.
    fn begin_duplicate_drag(&mut self, history: &mut History, selection: &mut Vec<LayerId>, id: LayerId, doc_pos: Pos2) {
        if !selection.contains(&id) {
            *selection = vec![id];
        }
        let ids = selection.clone();
        history.snapshot();
        let new_ids = grouping::duplicate_layers(history.mutate().active_page_mut(), &ids, Vec2::ZERO);
        if !new_ids.is_empty() {
            *selection = new_ids;
            let page = history.get().active_page();
            self.drag = DragState::MovingSelection {
                start_doc: doc_pos,
                starts: move_starts(page, selection),
            };
        }
    }

    fn begin_marquee_drag(&mut self, selection: &mut Vec<LayerId>, modifiers: Modifiers, doc_pos: Pos2) {
        let shift = modifiers.shift;
        // Clicking empty canvas is one of Scale mode's
        // three exit paths (Enter and the inspector's
        // Finish button are the other two).
        self.scaling = None;
        // Command+Shift (invert) implies Shift's
        // preserve-existing-selection behavior, so only a
        // plain (non-Command) Shift clears nothing while a
        // bare click still starts fresh.
        if !shift {
            selection.clear();
        }
        self.drag = DragState::Marquee {
            start_doc: doc_pos,
            additive: shift,
            base_selection: selection.clone(),
            contained_only: modifiers.alt,
            ignore_groups: modifiers.command,
            invert: modifiers.command && modifiers.shift,
        };
    }

    fn begin_pen_drag(
        &mut self,
        history: &mut History,
        selection: &mut Vec<LayerId>,
        tool: &mut Tool,
        doc_pos: Pos2,
        origin: Pos2,
    ) {
        if !self.try_close_pen_path(doc_pos, origin, history, selection, tool) {
            let point_index = {
                let points = self.pen.get_or_insert_with(Vec::new);
                points.push(PathPoint {
                    anchor: doc_pos,
                    handle_in: None,
                    handle_out: None,
                    point_type: PointType::Mirror,
                    corner_radius: 0.0,
                });
                points.len() - 1
            };
            self.drag = DragState::DrawingPenHandle { point_index };
        }
    }
}

fn rotate_drag(selected_layers: &[(&Layer, Vec2)], doc_pos: Pos2) -> Option<DragState> {
    let overall = selected_layers.iter().map(|(l, o)| display_bounds(l).translate(*o)).reduce(|a, b| a.union(b))?;
    let pivot = overall.center();
    let start_angle = (doc_pos - pivot).angle();
    let mut layers = Vec::new();
    for (layer, offset) in selected_layers {
        collect_rotatable_leaves(layer, *offset, &mut layers);
    }
    if layers.is_empty() {
        None
    } else {
        Some(DragState::Rotating { pivot, start_angle, layers })
    }
}

/// True if `id` is a strict descendant of any layer already in `selection` —
/// see the call site in `begin_hit_layer_drag` for why this needs
/// checking (`hit_test` always resolves to the innermost leaf, even when a
/// containing Group/Artboard is already the current selection).
fn hit_is_within_selection(page: &Page, selection: &[LayerId], id: LayerId) -> bool {
    selection
        .iter()
        .any(|&sel_id| sel_id != id && page.find(sel_id).is_some_and(|layer| layer.find(id).is_some()))
}

fn move_starts(page: &Page, ids: &[LayerId]) -> Vec<(LayerId, Pos2)> {
    ids.iter()
        .filter_map(|&id| page.find(id).map(|l| (id, l.frame.pos)))
        .collect()
}
