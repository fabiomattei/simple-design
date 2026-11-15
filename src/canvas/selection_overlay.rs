//! The selection overlay drawn over the document each frame — line
//! endpoints, path points or a bounding box with resize/rotate handles,
//! Smart Distribute gap handles, the Option-hover distance measurement — and
//! the hover hit-testing against those targets that drag start and
//! click-to-select act on.

use egui::{Color32, Pos2, Rect, Response, Stroke as EguiStroke, Ui, Vec2};

use crate::model::{
    GuideOrientation, Layer, LayerId, LayerKind, Page,
};
use crate::shapes::{rotate_point, rotated_corners};
use crate::tools::Tool;

use super::hit_test::{distribution_order, gap_handle_positions, hit_test};
use super::snapping::draw_distance_measurement;
use super::transform_geometry::display_bounds;
use super::{
    CanvasLayout, CanvasWidget, DragState, HANDLE_HIT_RADIUS, HANDLE_RADIUS, Handle, HoverState, PathPart, ROTATE_ZONE_OUTER_RADIUS, SELECTION_COLOR, SelectionView,
};

impl CanvasWidget {
    /// Paints the selection overlay (line endpoints, path points, or a
    /// bounding box with resize handles; Smart Distribute gap handles; the
    /// Option-hover distance measurement), sets the hover cursor, and returns
    /// which interactive target the pointer is over.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn paint_selection_overlay(
        &self,
        ui: &Ui,
        painter: &egui::Painter,
        response: &Response,
        page: &Page,
        view: &SelectionView,
        tool: &Tool,
        selection: &[LayerId],
        layout: CanvasLayout,
    ) -> HoverState {
        // On the exact frame a drag is recognized, `response.hover_pos()`
        // already reflects the *current*, post-threshold pointer position —
        // egui only flags `drag_started()` once the pointer has moved past
        // `InputOptions::max_click_dist` (6px), which can already exceed
        // `HANDLE_HIT_RADIUS` (8px) by the time the next frame samples it,
        // especially on a fast pointer move. Hit-testing anchors/handles
        // against that already-moved position (instead of where the button
        // actually went down) made grabbing a point to start editing it
        // flaky — working on a slow drag, missing on a fast one. Falling
        // back to the press origin for hit-testing on that one frame fixes
        // it; every other frame (plain hover, and every later frame of an
        // already-started drag) keeps using the live position exactly as
        // before, so the hover highlight still tracks the cursor normally.
        let hit_test_pos = if response.drag_started() {
            ui.input(|i| i.pointer.press_origin()).or_else(|| response.hover_pos())
        } else {
            response.hover_pos()
        };

        let (handle, rotate_corner, point) = self.paint_selection_handles(painter, view, layout.origin, hit_test_pos);
        let (gap_handle_order, gap_handle) =
            self.paint_gap_handles(painter, page, tool, selection, layout.origin, hit_test_pos);
        let hover = HoverState { handle, rotate_corner, point, gap_handle, gap_handle_order };
        self.update_cursor(ui, response, page, tool, layout, &hover);
        self.paint_distance_measurement(ui, painter, response, page, view, tool, selection, layout);
        hover
    }

    /// Paints the handles for the selection — a line/arrow's two endpoints,
    /// a single path's anchors and control handles, or a bounding box — and
    /// returns the resize handle, rotate zone and path point under
    /// `hit_test_pos`, in that order.
    fn paint_selection_handles(
        &self,
        painter: &egui::Painter,
        view: &SelectionView,
        origin: Pos2,
        hit_test_pos: Option<Pos2>,
    ) -> (Option<Handle>, Option<Handle>, Option<(usize, PathPart)>) {
        if let Some((layer, offset)) = view.single_line {
            (self.paint_line_handles(painter, layer, offset, origin, hit_test_pos), None, None)
        } else if let Some((layer, offset)) = view.single_path {
            (None, None, self.paint_path_points(painter, layer, offset, origin, hit_test_pos))
        } else if !view.layers.is_empty() {
            let (handle, rotate_corner) = self.paint_bounding_box_handles(painter, &view.layers, origin, hit_test_pos);
            (handle, rotate_corner, None)
        } else {
            (None, None, None)
        }
    }

    fn paint_line_handles(
        &self,
        painter: &egui::Painter,
        layer: &Layer,
        offset: Vec2,
        origin: Pos2,
        hit_test_pos: Option<Pos2>,
    ) -> Option<Handle> {
        let mut hovered_handle = None;
        // A line/arrow has no interior to bound, so (unlike every other
        // shape) it gets no rectangle outline — just its own two real
        // endpoints as handles. Using `frame.bounds()`'s min/max corners
        // here (as every other shape's resize handles do) would only
        // land on the actual endpoints when the shape was dragged
        // top-left-to-bottom-right; any other drag direction (or a
        // rotation baked in from an ungrouped parent, see
        // `Frame::rotation`'s doc comment) puts the circles off the
        // visible line entirely and swaps which endpoint each one
        // controls. `frame.start()`/`end()`, rotated the same way
        // `draw_layer` renders them, always match what's on screen.
        let doc_bounds = layer.frame.bounds().translate(offset);
        let center = self.to_screen(origin, doc_bounds.center());
        let start = rotate_point(self.to_screen(origin, layer.frame.start() + offset), center, layer.frame.rotation);
        let end = rotate_point(self.to_screen(origin, layer.frame.end() + offset), center, layer.frame.rotation);
        for (h, p) in [(Handle::TopLeft, start), (Handle::BottomRight, end)] {
            if let Some(mp) = hit_test_pos
                && mp.distance(p) <= HANDLE_HIT_RADIUS {
                    hovered_handle = Some(h);
                }
            painter.circle(p, HANDLE_RADIUS, Color32::WHITE, EguiStroke::new(1.5, SELECTION_COLOR));
        }
        hovered_handle
    }

    fn paint_path_points(
        &self,
        painter: &egui::Painter,
        layer: &Layer,
        offset: Vec2,
        origin: Pos2,
        hit_test_pos: Option<Pos2>,
    ) -> Option<(usize, PathPart)> {
        let mut hovered_point = None;
        if let LayerKind::Path { points, .. } = &layer.kind {
            for (i, p) in points.iter().enumerate() {
                let anchor_doc = p.anchor + layer.frame.pos.to_vec2() + offset;
                let anchor_screen = self.to_screen(origin, anchor_doc);
                for (part, handle) in [(PathPart::HandleIn, p.handle_in), (PathPart::HandleOut, p.handle_out)] {
                    let Some(h) = handle else { continue };
                    let handle_screen = self.to_screen(origin, anchor_doc + h);
                    painter.line_segment(
                        [anchor_screen, handle_screen],
                        EguiStroke::new(1.0, Color32::from_gray(120)),
                    );
                    if let Some(mp) = hit_test_pos
                        && mp.distance(handle_screen) <= HANDLE_HIT_RADIUS {
                            hovered_point = Some((i, part));
                        }
                    let hl = hovered_point == Some((i, part));
                    let r = if hl { 4.5 } else { 3.0 };
                    painter.circle_filled(handle_screen, r, Color32::WHITE);
                    painter.circle_stroke(
                        handle_screen,
                        r,
                        EguiStroke::new(1.0, if hl { SELECTION_COLOR } else { Color32::from_gray(120) }),
                    );
                }
                if let Some(mp) = hit_test_pos
                    && mp.distance(anchor_screen) <= HANDLE_HIT_RADIUS {
                        hovered_point = Some((i, PathPart::Anchor));
                    }
                let hl = hovered_point == Some((i, PathPart::Anchor));
                let selected = self.selected_points.contains(&i);
                let radius = if hl || selected { HANDLE_RADIUS * 1.3 } else { HANDLE_RADIUS };
                let fill = if hl || selected { SELECTION_COLOR } else { Color32::WHITE };
                painter.circle(anchor_screen, radius, fill, EguiStroke::new(1.5, SELECTION_COLOR));
            }
        }
        hovered_point
    }

    /// Returns the resize handle and rotate zone under `hit_test_pos`.
    fn paint_bounding_box_handles(
        &self,
        painter: &egui::Painter,
        selected_layers: &[(&Layer, Vec2)],
        origin: Pos2,
        hit_test_pos: Option<Pos2>,
    ) -> (Option<Handle>, Option<Handle>) {
        let (screen_rect, outline_rotation) = self.paint_selection_outlines(painter, selected_layers, origin);
        let mut hovered_handle = None;
        let mut hovered_rotate_corner = None;
        for h in Handle::ALL {
            let p = rotate_point(h.pos(screen_rect), screen_rect.center(), outline_rotation);
            if let Some(mp) = hit_test_pos {
                if mp.distance(p) <= HANDLE_HIT_RADIUS {
                    hovered_handle = Some(h);
                } else if matches!(h, Handle::TopLeft | Handle::TopRight | Handle::BottomRight | Handle::BottomLeft)
                    && mp.distance(p) <= ROTATE_ZONE_OUTER_RADIUS
                {
                    hovered_rotate_corner = Some(h);
                }
            }
            painter.circle(p, HANDLE_RADIUS, Color32::WHITE, EguiStroke::new(1.5, SELECTION_COLOR));
        }
        (hovered_handle, hovered_rotate_corner)
    }

    /// Outlines each selected layer and the selection's overall bounds;
    /// returns those bounds in screen space and the rotation they're drawn at.
    fn paint_selection_outlines(
        &self,
        painter: &egui::Painter,
        selected_layers: &[(&Layer, Vec2)],
        origin: Pos2,
    ) -> (Rect, f32) {
        // A rotated outline/handles only ever apply to a single selected
        // leaf's own rotation — a multi-selection always shows a plain
        // AABB (its members can each have different rotations, and
        // "the rotated bbox of mixed rotations" isn't well-defined), same
        // simplification precedent as a mixed multi-select losing a
        // `Line`'s exact direction fidelity (see `CLAUDE.md`).
        let outline_rotation =
            if let [(layer, _)] = selected_layers[..] { layer.frame.rotation } else { 0.0 };
        for (layer, offset) in selected_layers {
            let b = display_bounds(layer).translate(*offset);
            let sr = Rect::from_two_pos(self.to_screen(origin, b.min), self.to_screen(origin, b.max));
            // The alignment reference layer (see `reference_layer`'s doc
            // comment) gets a thicker, fully-opaque outline instead of
            // the normal thin/dim per-layer one.
            let is_reference = self.reference_layer == Some(layer.id);
            let stroke = if is_reference {
                EguiStroke::new(2.5, SELECTION_COLOR)
            } else {
                EguiStroke::new(1.0, SELECTION_COLOR.gamma_multiply(0.6))
            };
            if layer.frame.rotation == 0.0 {
                painter.rect_stroke(sr, 0.0, stroke, egui::StrokeKind::Outside);
            } else {
                let corners = rotated_corners(sr, layer.frame.rotation);
                painter.add(egui::Shape::closed_line(corners.to_vec(), stroke));
            }
        }
        let overall = selected_layers
            .iter()
            .map(|(l, o)| display_bounds(l).translate(*o))
            .reduce(|a, b| a.union(b))
            .unwrap();
        let screen_rect = Rect::from_two_pos(
            self.to_screen(origin, overall.min),
            self.to_screen(origin, overall.max),
        );
        if outline_rotation == 0.0 {
            painter.rect_stroke(screen_rect, 0.0, EguiStroke::new(1.5, SELECTION_COLOR), egui::StrokeKind::Outside);
        } else {
            let corners = rotated_corners(screen_rect, outline_rotation);
            painter.add(egui::Shape::closed_line(corners.to_vec(), EguiStroke::new(1.5, SELECTION_COLOR)));
        }
        (screen_rect, outline_rotation)
    }

    /// Smart Distribute gap-adjustment handles — only shown while the
    /// exact set of layers from the most recent Distribute is still
    /// selected (see `last_distributed`'s doc comment).
    /// Returns the distributed layers in gap order (empty when the handles
    /// aren't showing) and the gap handle under `hit_test_pos`.
    #[allow(clippy::too_many_arguments)]
    fn paint_gap_handles(
        &self,
        painter: &egui::Painter,
        page: &Page,
        tool: &Tool,
        selection: &[LayerId],
        origin: Pos2,
        hit_test_pos: Option<Pos2>,
    ) -> (Vec<(LayerId, Rect)>, Option<usize>) {
        let mut hovered_gap_handle = None;
        let mut gap_handle_order: Vec<(LayerId, Rect)> = Vec::new();
        if *tool == Tool::Select && matches!(self.drag, DragState::None)
            && let Some((dist_ids, axis)) = self.last_distributed.clone() {
                let mut a = dist_ids.clone();
                let mut b = selection.to_vec();
                a.sort();
                b.sort();
                if a == b && a.len() >= 3 {
                    gap_handle_order = distribution_order(page, &dist_ids, axis);
                    for (i, doc_pos) in gap_handle_positions(&gap_handle_order, axis).into_iter().enumerate() {
                        let p = self.to_screen(origin, doc_pos);
                        if let Some(mp) = hit_test_pos
                            && mp.distance(p) <= HANDLE_HIT_RADIUS {
                                hovered_gap_handle = Some(i);
                            }
                        let hl = hovered_gap_handle == Some(i);
                        painter.circle(p, if hl { 5.0 } else { 3.5 }, SELECTION_COLOR, EguiStroke::new(1.0, Color32::WHITE));
                    }
                }
            }
        (gap_handle_order, hovered_gap_handle)
    }

    /// Sets the pointer cursor for what's under it: a resize or gap handle,
    /// a rotate zone, a guide (Select tool), or the Pan/Scissors tool itself.
    fn update_cursor(
        &self,
        ui: &Ui,
        response: &Response,
        page: &Page,
        tool: &Tool,
        layout: CanvasLayout,
        hover: &HoverState,
    ) {
        let CanvasLayout { canvas_rect, origin, .. } = layout;
        if let Some(h) = hover.handle {
            ui.ctx().set_cursor_icon(h.cursor());
        } else if hover.gap_handle.is_some() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeColumn);
        } else if hover.rotate_corner.is_some() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::Crosshair);
        } else if *tool == Tool::Select {
            let guide_cursor = response
                .hover_pos()
                .filter(|mp| canvas_rect.contains(*mp))
                .and_then(|mp| self.hovered_guide_index(origin, &page.guides, mp))
                .map(|idx| match page.guides[idx].orientation {
                    GuideOrientation::Horizontal => egui::CursorIcon::ResizeVertical,
                    GuideOrientation::Vertical => egui::CursorIcon::ResizeHorizontal,
                });
            if let Some(cursor) = guide_cursor {
                ui.ctx().set_cursor_icon(cursor);
            }
        }

        if *tool == Tool::Pan {
            ui.ctx().set_cursor_icon(egui::CursorIcon::Grab);
        }
        if *tool == Tool::Scissors {
            ui.ctx().set_cursor_icon(egui::CursorIcon::Crosshair);
        }
    }

    /// Option-hover distance measurement: with a selection and Option
    /// held, hovering another layer shows the gap between their closest
    /// edges — a "measure distance" affordance. Only while
    /// idle (no drag in progress) so it doesn't fight the snap-line
    /// overlay drawn during an actual move/resize.
    #[allow(clippy::too_many_arguments)]
    fn paint_distance_measurement(
        &self,
        ui: &Ui,
        painter: &egui::Painter,
        response: &Response,
        page: &Page,
        view: &SelectionView,
        tool: &Tool,
        selection: &[LayerId],
        layout: CanvasLayout,
    ) {
        let CanvasLayout { canvas_rect, origin, .. } = layout;
        if *tool == Tool::Select && matches!(self.drag, DragState::None) && ui.input(|i| i.modifiers.alt)
            && let Some(sel_bounds) = view.layers
                .iter()
                .map(|(l, o)| l.frame.rotated_bounds().translate(*o))
                .reduce(|a, b| a.union(b))
                && let Some(mp) = response.hover_pos().filter(|mp| canvas_rect.contains(*mp)) {
                    let doc_pos = self.to_doc(origin, mp);
                    if let Some(hover_id) = hit_test(page, doc_pos)
                        && !selection.contains(&hover_id)
                            && let (Some(hl), Some(hoff)) = (page.find(hover_id), page.absolute_offset(hover_id)) {
                                let hover_bounds = hl.frame.rotated_bounds().translate(hoff);
                                draw_distance_measurement(painter, |p| self.to_screen(origin, p), sel_bounds, hover_bounds);
                            }
                }
    }
}
