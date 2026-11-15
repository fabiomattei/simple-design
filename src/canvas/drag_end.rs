//! Drag release: resolves the finished `DragState` — commits new shapes,
//! marquee selections, guides and image-edit selections, and tidies up path
//! handles and container frames after a resize/rotate.

use egui::{Pos2, Rect, Response, Vec2};

use crate::history::History;
use crate::model::{
    Frame, Guide, GuideOrientation, LayerId, LayerKind, Page, PointType,
};
use crate::tools::Tool;

use super::hit_test::collect_marquee_hits;
use super::image_edit::{doc_to_pixel, merge_mask, rect_mask};
use super::layer_creation::{apply_text_auto_resize, insert_layer, new_layer_for_tool};
use super::path_editing::{normalize_path_frame, read_path};
use super::snapping::{
    build_snap_candidates, snap_guide_axis,
};
use super::transform_geometry::refit_container_to_children;
use super::{
    CanvasLayout, CanvasWidget, DragState, HandleSide,
    PEN_HANDLE_MIN_DRAG,
};

impl CanvasWidget {
    /// Resolves the gesture on release — commits a new shape, marquee
    /// selection, guide or image-edit selection, and cleans up near-zero
    /// path handles or container frames — leaving `drag` reset to `None`.
    /// `drag` is taken out up front rather than reset after the match (no arm
    /// reads it), which leaves the arms free to mutate `self`. One arm per
    /// `DragState` variant, the same flat dispatch as `continue_drag`.
    pub(super) fn end_drag(
        &mut self,
        ctx: &egui::Context,
        response: &Response,
        history: &mut History,
        tool: &mut Tool,
        selection: &mut Vec<LayerId>,
        layout: CanvasLayout,
    ) {
        let origin = layout.origin;
        let drag = std::mem::replace(&mut self.drag, DragState::None);
        match &drag {
            DragState::CreatingShape { start_doc } => {
                self.finish_creating_shape(ctx, response, history, tool, selection, origin, start_doc);
            }
            DragState::Marquee {
                start_doc,
                additive,
                base_selection,
                contained_only,
                ignore_groups,
                invert,
            } => {
                let mouse = response
                    .interact_pointer_pos()
                    .unwrap_or(self.to_screen(origin, *start_doc));
                let end_doc = self.to_doc(origin, mouse);
                let marquee = Rect::from_two_pos(*start_doc, end_doc);
                let mut hits = Vec::new();
                collect_marquee_hits(
                    &history.get().active_page().layers,
                    Vec2::ZERO,
                    marquee,
                    *contained_only,
                    *ignore_groups,
                    &mut hits,
                );
                *selection = merge_marquee_hits(base_selection, hits, *invert, *additive);
            }
            DragState::MarqueePoints {
                layer_id,
                start_doc,
                additive,
                base_selection,
            } => {
                self.finish_marquee_points(response, history, origin, layer_id, start_doc, additive, base_selection);
            }
            DragState::CreatingGuide { orientation } => {
                self.finish_creating_guide(response, history, layout, orientation);
            }
            DragState::MovingGuide { index, orientation } => {
                self.finish_moving_guide(response, history, layout, index, orientation);
            }
            DragState::DrawingPenHandle { point_index } => {
                self.finish_pen_handle(point_index);
            }
            DragState::EditingPathAnchor { layer_id, .. } => {
                normalize_path_frame(history.mutate().active_page_mut(), *layer_id);
            }
            DragState::EditingPathHandle {
                layer_id,
                point_index,
                side,
                ..
            } => {
                drop_tiny_path_handle(history.mutate().active_page_mut(), layer_id, point_index, side, self.zoom);
            }
            DragState::ResizingGroup { .. } | DragState::Rotating { .. } => {
                // The one-time counterpart to the comments in `dragged`'s
                // `ResizingGroup`/`Rotating` arms: now that the gesture
                // is over and no further frame will re-derive a leaf's
                // position from its drag-start `parent_offset`, it's
                // safe to bring every selected container's own `frame`
                // back in sync with its (rotated/resized) children — see
                // `refit_container_to_children`'s doc comment.
                for &id in selection.iter() {
                    refit_container_to_children(history.mutate().active_page_mut(), id);
                }
            }
            DragState::ImageEditDrag { start_doc, subtract, base_mask } => {
                self.finish_image_edit_drag(response, history, origin, start_doc, subtract, base_mask);
            }
            _ => {}
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn finish_creating_shape(
        &mut self,
        ctx: &egui::Context,
        response: &Response,
        history: &mut History,
        tool: &mut Tool,
        selection: &mut Vec<LayerId>,
        origin: Pos2,
        start_doc: &Pos2,
    ) {
        let mouse = response
            .interact_pointer_pos()
            .unwrap_or(self.to_screen(origin, *start_doc));
        let end_doc = self.to_doc(origin, mouse);
        let frame = Frame::from_two_points(*start_doc, end_doc);
        // A plain click (no meaningful drag) still places a Text
        // layer at a default size — text is usually authored by
        // clicking then typing, not by dragging out a box first.
        if (frame.size.x.abs() > 2.0 || frame.size.y.abs() > 2.0 || *tool == Tool::Text)
            && let Some(new_layer) = new_layer_for_tool(*tool, frame) {
                let new_id = new_layer.id;
                let is_text = matches!(new_layer.kind, LayerKind::Text { .. });
                let hint = self.insert_hint_parent.take();
                insert_layer(history.mutate().active_page_mut(), new_layer, *start_doc, hint);
                if is_text
                    && let Some(l) = history.mutate().active_page_mut().find_mut(new_id) {
                        apply_text_auto_resize(ctx, l);
                    }
                *selection = vec![new_id];
                *tool = Tool::Select;
            }
    }

    #[allow(clippy::too_many_arguments)]
    fn finish_marquee_points(
        &mut self,
        response: &Response,
        history: &History,
        origin: Pos2,
        layer_id: &LayerId,
        start_doc: &Pos2,
        additive: &bool,
        base_selection: &[usize],
    ) {
        let mouse = response
            .interact_pointer_pos()
            .unwrap_or(self.to_screen(origin, *start_doc));
        let end_doc = self.to_doc(origin, mouse);
        let marquee = Rect::from_two_pos(*start_doc, end_doc);
        if let Some((frame_pos, offset, points, _closed)) = read_path(history, *layer_id) {
            let hits: Vec<usize> = points
                .iter()
                .enumerate()
                .filter(|(_, p)| marquee.contains(p.anchor + frame_pos.to_vec2() + offset))
                .map(|(i, _)| i)
                .collect();
            if *additive {
                let mut merged = base_selection.to_vec();
                for i in hits {
                    if !merged.contains(&i) {
                        merged.push(i);
                    }
                }
                self.selected_points = merged;
            } else {
                self.selected_points = hits;
            }
        }
    }

    fn finish_creating_guide(
        &self,
        response: &Response,
        history: &mut History,
        layout: CanvasLayout,
        orientation: &GuideOrientation,
    ) {
        let CanvasLayout { canvas_rect, origin, .. } = layout;
        let mouse = response.interact_pointer_pos().unwrap_or(origin);
        // Dropped back on a ruler (or off the canvas): cancels
        // guide creation rather than placing one.
        if canvas_rect.contains(mouse) {
            let doc_pos = self.to_doc(origin, mouse);
            let candidates = build_snap_candidates(history.get().active_page(), &[]);
            let value = snap_guide_axis(
                *orientation,
                doc_pos,
                &candidates,
                self.zoom,
                self.snap_enabled,
                self.show_grid,
            );
            history.snapshot();
            history
                .mutate()
                .active_page_mut()
                .guides
                .push(Guide { orientation: *orientation, pos: value });
        }
    }

    fn finish_moving_guide(
        &self,
        response: &Response,
        history: &mut History,
        layout: CanvasLayout,
        index: &usize,
        orientation: &GuideOrientation,
    ) {
        let CanvasLayout { canvas_rect, origin, .. } = layout;
        let mouse = response.interact_pointer_pos().unwrap_or(origin);
        let new_value = if canvas_rect.contains(mouse) {
            let doc_pos = self.to_doc(origin, mouse);
            let candidates = build_snap_candidates(history.get().active_page(), &[]);
            Some(snap_guide_axis(
                *orientation,
                doc_pos,
                &candidates,
                self.zoom,
                self.snap_enabled,
                self.show_grid,
            ))
        } else {
            None
        };
        history.snapshot();
        let guides = &mut history.mutate().active_page_mut().guides;
        match new_value {
            Some(v) => {
                if let Some(g) = guides.get_mut(*index) {
                    g.pos = v;
                }
            }
            None => {
                if *index < guides.len() {
                    guides.remove(*index);
                }
            }
        }
    }

    fn finish_pen_handle(&mut self, point_index: &usize) {
        // A drag too small to be an intentional handle pull (e.g. a
        // slightly-jittery click) leaves a plain corner point instead
        // of a near-zero-length handle.
        if let Some(points) = &mut self.pen
            && let Some(pt) = points.get_mut(*point_index) {
                let tiny = pt
                    .handle_out
                    .map(|v| v.length() * self.zoom < PEN_HANDLE_MIN_DRAG)
                    .unwrap_or(true);
                if tiny {
                    pt.handle_in = None;
                    pt.handle_out = None;
                    pt.point_type = PointType::Straight;
                }
            }
    }

    #[allow(clippy::too_many_arguments)]
    fn finish_image_edit_drag(
        &mut self,
        response: &Response,
        history: &History,
        origin: Pos2,
        start_doc: &Pos2,
        subtract: &bool,
        base_mask: &[bool],
    ) {
        let (start_doc, subtract) = (*start_doc, *subtract);
        let mut new_mask = base_mask.to_vec();
        let mouse = response.interact_pointer_pos().unwrap_or(self.to_screen(origin, start_doc));
        let end_doc = self.to_doc(origin, mouse);
        if let Some((layer_id, bounds_doc, width, height)) = self.image_edit_doc_bounds(history) {
            // A near-zero-movement release is a Magic Wand
            // click; a real drag is a rectangular selection —
            // same click-vs-drag threshold `CreatingShape` uses.
            let tiny = (end_doc.x - start_doc.x).abs() <= 2.0 && (end_doc.y - start_doc.y).abs() <= 2.0;
            if tiny {
                if let Some((px, py)) = doc_to_pixel(start_doc, bounds_doc, width, height)
                    && let Some(layer) = history.get().active_page().find(layer_id)
                        && let LayerKind::Image { encoded, .. } = &layer.kind
                            && let Some(decoded) = crate::image_ops::decode(encoded) {
                                let tolerance = self.image_edit_tolerance();
                                let wand = crate::image_ops::magic_wand_mask(&decoded, (px, py), tolerance);
                                merge_mask(&mut new_mask, &wand, subtract);
                            }
            } else {
                let clipped = Rect::from_two_pos(start_doc, end_doc).intersect(bounds_doc);
                if clipped.width() > 0.0 && clipped.height() > 0.0 {
                    let inset = Pos2::new(clipped.max.x - 0.001, clipped.max.y - 0.001);
                    if let (Some((x0, y0)), Some((x1, y1))) = (
                        doc_to_pixel(clipped.min, bounds_doc, width, height),
                        doc_to_pixel(inset, bounds_doc, width, height),
                    ) {
                        let sel = rect_mask(width, height, x0, y0, x1, y1);
                        merge_mask(&mut new_mask, &sel, subtract);
                    }
                }
            }
        }
        if let Some(edit) = &mut self.image_edit {
            edit.mask = new_mask;
            edit.mark_dirty();
        }
    }
}

/// The selection a released layer marquee produces: `invert` toggles every
/// hit against `base_selection`, `additive` adds hits to it, and otherwise
/// the hits replace it.
fn merge_marquee_hits(base_selection: &[LayerId], hits: Vec<LayerId>, invert: bool, additive: bool) -> Vec<LayerId> {
    if invert {
        let mut merged = base_selection.to_vec();
        for id in hits {
            if let Some(pos) = merged.iter().position(|&s| s == id) {
                merged.remove(pos);
            } else {
                merged.push(id);
            }
        }
        merged
    } else if additive {
        let mut merged = base_selection.to_vec();
        for id in hits {
            if !merged.contains(&id) {
                merged.push(id);
            }
        }
        merged
    } else {
        hits
    }
}

fn drop_tiny_path_handle(page: &mut Page, layer_id: &LayerId, point_index: &usize, side: &HandleSide, zoom: f32) {
    // Same near-zero-drag cleanup as `DrawingPenHandle`: dragging
    // a handle back onto its anchor removes it, turning that side
    // back into a straight corner.
    if let Some(layer) = page.find_mut(*layer_id)
        && let LayerKind::Path { points, .. } = &mut layer.kind
            && let Some(pt) = points.get_mut(*point_index) {
                let target = match side {
                    HandleSide::In => &mut pt.handle_in,
                    HandleSide::Out => &mut pt.handle_out,
                };
                let tiny = target.map(|v| v.length() * zoom < PEN_HANDLE_MIN_DRAG).unwrap_or(false);
                if tiny {
                    *target = None;
                }
            }
}
