//! Drag frames: advances the active `DragState` with each pointer move —
//! live document edits for move/resize/rotate/point gestures, previews for
//! shape/marquee/guide gestures that commit on release.

use egui::{Color32, Pos2, Rect, Response, Stroke as EguiStroke, Ui, Vec2};

use crate::alignment::DistributeAxis;
use crate::history::History;
use crate::model::{
    Frame, LayerId, LayerKind, Page, PointType,
};
use crate::shapes::{ellipse_points, rotate_point};
use crate::tools::Tool;

use super::rulers_guides::draw_guide_line;
use super::snapping::{
    SnapCandidates, build_snap_candidates, drag_exclude_ids, draw_snap_lines,
    snap_bounds_delta, snap_guide_axis, snap_point,
};
use super::transform_geometry::{
    ResizeLayerInfo, apply_resize_delta, apply_rotation_delta,
};
use super::{
    CanvasLayout, CanvasWidget, DragState, Handle, HandleSide, SELECTION_COLOR, ScaleAnchor,
};

impl CanvasWidget {
    /// Advances the active `DragState` by one frame of pointer movement:
    /// live-edits the document for move/resize/rotate/point gestures, and
    /// paints previews (new shape, marquee, guide, snap lines) for those that
    /// only commit on release in `end_drag`. One arm per `DragState`
    /// variant — a flat enum dispatch, deliberately kept as a single match
    /// rather than split to meet the usual branch-count limit.
    pub(super) fn continue_drag(
        &mut self,
        ui: &Ui,
        response: &Response,
        canvas_painter: &egui::Painter,
        history: &mut History,
        tool: &Tool,
        layout: CanvasLayout,
    ) {
        let CanvasLayout { canvas_rect, origin, .. } = layout;
        let mouse = response.interact_pointer_pos().unwrap_or(origin);
        let doc_pos = self.to_doc(origin, mouse);
        let exclude = drag_exclude_ids(&self.drag);
        let snap_candidates = build_snap_candidates(history.get().active_page(), &exclude);
        match &self.drag {
            DragState::PanningView {
                start_mouse,
                start_pan,
            } => {
                self.pan = *start_pan + (mouse - *start_mouse);
            }
            DragState::CreatingShape { start_doc } => {
                self.preview_creating_shape(canvas_painter, tool, layout, &snap_candidates, start_doc, doc_pos);
            }
            DragState::Marquee { start_doc, .. }
            | DragState::MarqueePoints { start_doc, .. }
            | DragState::ImageEditDrag { start_doc, .. } => {
                let start_screen = self.to_screen(origin, *start_doc);
                let cur_screen = self.to_screen(origin, doc_pos);
                let rect = Rect::from_two_pos(start_screen, cur_screen);
                canvas_painter.rect(
                    rect,
                    0.0,
                    SELECTION_COLOR.gamma_multiply(0.15),
                    EguiStroke::new(1.0, SELECTION_COLOR),
                    egui::StrokeKind::Inside,
                );
            }
            DragState::MovingSelection { start_doc, starts } => {
                self.drag_moving_selection(ui, canvas_painter, history, layout, &snap_candidates, start_doc, starts, doc_pos);
            }
            DragState::AdjustingDistributionGap {
                start_doc,
                axis,
                gap_index,
                order,
                starts,
            } => {
                drag_distribution_gap(history.mutate().active_page_mut(), doc_pos, start_doc, axis, gap_index, order, starts);
            }
            DragState::ResizingLine {
                id,
                handle,
                parent_offset,
            } => {
                let (doc_pos, lx, ly) =
                    snap_point(doc_pos, &snap_candidates, self.zoom, self.snap_enabled, self.show_grid);
                draw_snap_lines(canvas_painter, canvas_rect, origin, self.pan, self.zoom, lx, ly);
                resize_line(history.mutate().active_page_mut(), id, handle, parent_offset, doc_pos);
            }
            DragState::ResizingGroup {
                handle,
                start_overall_bounds,
                layers,
                scale_style,
                scale_anchor,
            } => {
                let (doc_pos, lx, ly) =
                    snap_point(doc_pos, &snap_candidates, self.zoom, self.snap_enabled, self.show_grid);
                draw_snap_lines(canvas_painter, canvas_rect, origin, self.pan, self.zoom, lx, ly);
                resize_group(
                    history.mutate().active_page_mut(),
                    doc_pos,
                    handle,
                    start_overall_bounds,
                    layers,
                    scale_style,
                    scale_anchor,
                );
                // NOT a `refit_container_to_children` pass here, unlike
                // `end_drag` — every `ResizeLayerInfo` in
                // `layers` carries a `parent_offset` captured once at
                // drag start (see `collect_resizable_leaves`) and
                // `apply_resize_delta` re-derives each leaf's position
                // from it *every* frame. Refitting a selected
                // container's own `frame.pos` mid-drag would move that
                // baseline out from under still-live `ResizeLayerInfo`s,
                // so each subsequent frame would compound the
                // container's drift on top of the real resize — the
                // selection stays (harmlessly) stale-boxed for the
                // duration of the drag and only gets refit once, after
                // it's released.
            }
            DragState::Rotating { pivot, start_angle, layers } => {
                let mut delta_deg = ((doc_pos - *pivot).angle() - *start_angle).to_degrees();
                if ui.input(|i| i.modifiers.shift) {
                    delta_deg = (delta_deg / 15.0).round() * 15.0;
                }
                apply_rotation_delta(history.mutate().active_page_mut(), *pivot, delta_deg, layers);
                // See the matching comment in the `ResizingGroup` arm
                // just above — same reason a live refit here would
                // desync `RotateLayerInfo::parent_offset` mid-drag and
                // make the container drift.
            }
            DragState::DrawingPenHandle { point_index } => {
                if let Some(points) = &mut self.pen
                    && let Some(pt) = points.get_mut(*point_index) {
                        let delta = doc_pos - pt.anchor;
                        pt.handle_out = Some(delta);
                        pt.handle_in = Some(-delta);
                    }
            }
            DragState::EditingPathAnchor {
                layer_id,
                point_indices,
                start_doc,
                start_anchors,
            } => {
                move_path_anchors(history.mutate().active_page_mut(), layer_id, point_indices, start_doc, start_anchors, doc_pos);
            }
            DragState::EditingPathHandle {
                layer_id,
                parent_offset,
                point_index,
                side,
            } => {
                drag_path_handle(history.mutate().active_page_mut(), layer_id, parent_offset, point_index, side, doc_pos);
            }
            DragState::CreatingGuide { orientation } => {
                let value = snap_guide_axis(
                    *orientation,
                    doc_pos,
                    &snap_candidates,
                    self.zoom,
                    self.snap_enabled,
                    self.show_grid,
                );
                draw_guide_line(canvas_painter, canvas_rect, origin, self.pan, self.zoom, *orientation, value);
            }
            DragState::MovingGuide { orientation, .. } => {
                // Dragging back over a ruler (or past the canvas edge)
                // previews as "no line" — released there, it deletes.
                if canvas_rect.contains(mouse) {
                    let value = snap_guide_axis(
                        *orientation,
                        doc_pos,
                        &snap_candidates,
                        self.zoom,
                        self.snap_enabled,
                        self.show_grid,
                    );
                    draw_guide_line(canvas_painter, canvas_rect, origin, self.pan, self.zoom, *orientation, value);
                }
            }
            DragState::None => {}
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn preview_creating_shape(
        &self,
        canvas_painter: &egui::Painter,
        tool: &Tool,
        layout: CanvasLayout,
        snap_candidates: &SnapCandidates,
        start_doc: &Pos2,
        doc_pos: Pos2,
    ) {
        let CanvasLayout { canvas_rect, origin, .. } = layout;
        let (doc_pos, lx, ly) =
            snap_point(doc_pos, snap_candidates, self.zoom, self.snap_enabled, self.show_grid);
        draw_snap_lines(canvas_painter, canvas_rect, origin, self.pan, self.zoom, lx, ly);
        let start_screen = self.to_screen(origin, *start_doc);
        let cur_screen = self.to_screen(origin, doc_pos);
        if matches!(tool, Tool::Line | Tool::Arrow) {
            canvas_painter.line_segment(
                [start_screen, cur_screen],
                EguiStroke::new(2.0, Color32::from_rgb(30, 30, 30)),
            );
        } else {
            let preview = Rect::from_two_pos(start_screen, cur_screen);
            canvas_painter.rect(
                preview,
                0.0,
                Color32::from_rgba_unmultiplied(216, 216, 216, 160),
                EguiStroke::new(1.0, Color32::from_rgb(30, 30, 30)),
                egui::StrokeKind::Inside,
            );
            if *tool == Tool::Oval {
                let center = preview.center();
                let points: Vec<Pos2> =
                    ellipse_points(center, preview.width() / 2.0, preview.height() / 2.0);
                canvas_painter.add(egui::Shape::Path(egui::epaint::PathShape {
                    points,
                    closed: true,
                    fill: Color32::from_rgba_unmultiplied(180, 180, 180, 160),
                    stroke: EguiStroke::new(1.0, Color32::from_rgb(30, 30, 30)).into(),
                }));
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn drag_moving_selection(
        &self,
        ui: &Ui,
        canvas_painter: &egui::Painter,
        history: &mut History,
        layout: CanvasLayout,
        snap_candidates: &SnapCandidates,
        start_doc: &Pos2,
        starts: &[(LayerId, Pos2)],
        doc_pos: Pos2,
    ) {
        let CanvasLayout { canvas_rect, origin, .. } = layout;
        let mut raw_delta = doc_pos - *start_doc;
        // Shift constrains the move to whichever axis has moved
        // further from the drag start, matching a common
        // horizontal/vertical move lock.
        if ui.input(|i| i.modifiers.shift) {
            if raw_delta.x.abs() >= raw_delta.y.abs() {
                raw_delta.y = 0.0;
            } else {
                raw_delta.x = 0.0;
            }
        }
        let original_bounds = {
            let live_page = history.get().active_page();
            starts
                .iter()
                .filter_map(|(id, start_pos)| {
                    let size = live_page.find(*id)?.frame.size;
                    Some(Rect::from_two_pos(*start_pos, *start_pos + size))
                })
                .reduce(|a, b| a.union(b))
        };
        let delta = if let Some(ob) = original_bounds {
            let (d, lx, ly) = snap_bounds_delta(
                ob,
                raw_delta,
                snap_candidates,
                self.zoom,
                self.snap_enabled,
                self.show_grid,
            );
            draw_snap_lines(canvas_painter, canvas_rect, origin, self.pan, self.zoom, lx, ly);
            d
        } else {
            raw_delta
        };
        for (id, start_pos) in starts {
            if let Some(layer) = history.mutate().active_page_mut().find_mut(*id) {
                layer.frame.pos = *start_pos + delta;
            }
        }
    }
}

fn drag_distribution_gap(
    mutable_page: &mut Page,
    doc_pos: Pos2,
    start_doc: &Pos2,
    axis: &DistributeAxis,
    gap_index: &usize,
    order: &[LayerId],
    starts: &[Pos2],
) {
    let delta = doc_pos - *start_doc;
    let shift = match axis {
        DistributeAxis::Horizontal => Vec2::new(delta.x, 0.0),
        DistributeAxis::Vertical => Vec2::new(0.0, delta.y),
    };
    for (i, (id, start_pos)) in order.iter().zip(starts.iter()).enumerate() {
        if i <= *gap_index {
            continue; // layers before the gap stay put.
        }
        if let Some(layer) = mutable_page.find_mut(*id) {
            layer.frame.pos = *start_pos + shift;
        }
    }
}

fn resize_line(page: &mut Page, id: &LayerId, handle: &Handle, parent_offset: &Vec2, doc_pos: Pos2) {
    let local_pos = doc_pos - *parent_offset;
    if let Some(layer) = page.find_mut(*id) {
        let rotation = layer.frame.rotation;
        if *handle == Handle::TopLeft {
            let end = layer.frame.end();
            layer.frame = Frame::from_two_points(local_pos, end);
        } else {
            let start = layer.frame.start();
            layer.frame = Frame::from_two_points(start, local_pos);
        }
        layer.frame.rotation = rotation;
    }
}

/// Applies one frame of a bounding-box resize to every leaf captured at drag
/// start. Long, but a single computation: inverse-rotate the pointer, resize
/// the overall bounds, derive the scale transform, apply it to each leaf.
fn resize_group(
    page: &mut Page,
    doc_pos: Pos2,
    handle: &Handle,
    start_overall_bounds: &Rect,
    layers: &[ResizeLayerInfo],
    scale_style: &bool,
    scale_anchor: &ScaleAnchor,
) {
    // A single rotated shape's resize handles are drawn (and
    // hit-tested) at its *rotated* screen corners (see
    // `paint_bounding_box_handles`), but `start_overall_bounds`
    // and the `handle.resize`/`transform` math below all
    // operate in the shape's own unrotated local space. So
    // the mouse position needs the inverse rotation applied
    // first — inverse-rotating it back onto the unrotated
    // rect's corner the user visually grabbed — before
    // resizing; the result is a plain axis-aligned resize in
    // local space (stays rectangular, doesn't shear), with
    // `original_rotation` re-applied unchanged below. A
    // multi-selection (mixed/no single rotation) is
    // deliberately left as a plain page-space resize — same
    // accepted-tradeoff category as a mixed Line multi-select
    // losing exact direction fidelity (see `CLAUDE.md`).
    let single_rotation = if let [only] = layers { only.original_rotation } else { 0.0 };
    let doc_pos = if single_rotation == 0.0 {
        doc_pos
    } else {
        rotate_point(doc_pos, start_overall_bounds.center(), -single_rotation)
    };
    let mut new_overall = handle.resize(*start_overall_bounds, doc_pos);
    if *scale_anchor == ScaleAnchor::Center {
        // Re-center on the *original* bounds' center instead
        // of leaving the dragged handle's opposite
        // corner/edge fixed — same new size either way, just
        // a different anchor, so this is a plain override of
        // where `new_overall` sits rather than a different
        // resize computation.
        new_overall = Rect::from_center_size(start_overall_bounds.center(), new_overall.size());
    }
    let old = *start_overall_bounds;
    let scale = Vec2::new(
        if old.width().abs() > 1e-6 {
            new_overall.width() / old.width()
        } else {
            1.0
        },
        if old.height().abs() > 1e-6 {
            new_overall.height() / old.height()
        } else {
            1.0
        },
    );
    let old_anchor = old.min;
    let new_anchor = new_overall.min;
    let transform = |p: Pos2| -> Pos2 {
        Pos2::new(
            new_anchor.x + (p.x - old_anchor.x) * scale.x,
            new_anchor.y + (p.y - old_anchor.y) * scale.y,
        )
    };
    // Stroke width/corner radius aren't axis-specific, so
    // Scale mode uses one uniform factor (the average of the
    // two axis scales) rather than `scale.x`/`scale.y`
    // separately.
    let uniform_scale = (scale.x.abs() + scale.y.abs()) / 2.0;
    apply_resize_delta(
        page,
        layers,
        transform,
        scale,
        *scale_style,
        uniform_scale,
    );
}

fn move_path_anchors(
    page: &mut Page,
    layer_id: &LayerId,
    point_indices: &[usize],
    start_doc: &Pos2,
    start_anchors: &[Pos2],
    doc_pos: Pos2,
) {
    let delta = doc_pos - *start_doc;
    if let Some(layer) = page.find_mut(*layer_id)
        && let LayerKind::Path { points, .. } = &mut layer.kind {
            for (&index, &start_anchor) in point_indices.iter().zip(start_anchors.iter()) {
                if let Some(pt) = points.get_mut(index) {
                    pt.anchor = start_anchor + delta;
                }
            }
        }
}

fn drag_path_handle(
    page: &mut Page,
    layer_id: &LayerId,
    parent_offset: &Vec2,
    point_index: &usize,
    side: &HandleSide,
    doc_pos: Pos2,
) {
    if let Some(layer) = page.find_mut(*layer_id) {
        let frame_pos = layer.frame.pos;
        if let LayerKind::Path { points, .. } = &mut layer.kind
            && let Some(pt) = points.get_mut(*point_index) {
                let anchor_abs = frame_pos + *parent_offset + pt.anchor.to_vec2();
                let delta = doc_pos - anchor_abs;
                match side {
                    HandleSide::In => pt.handle_in = Some(delta),
                    HandleSide::Out => pt.handle_out = Some(delta),
                }
                // Opposite-handle coupling depends on the
                // point's type: `Mirror` keeps both handles
                // an exact reflection of each other,
                // `Asymmetric` keeps them at opposite angles
                // but lets each side have its own length,
                // and `Disconnected`/`Straight` leave the
                // untouched side exactly as it was.
                match pt.point_type {
                    PointType::Mirror => {
                        let mirrored = Some(-delta);
                        match side {
                            HandleSide::In => pt.handle_out = mirrored,
                            HandleSide::Out => pt.handle_in = mirrored,
                        }
                    }
                    PointType::Asymmetric => {
                        let opposite_len = match side {
                            HandleSide::In => pt.handle_out,
                            HandleSide::Out => pt.handle_in,
                        }
                        .map(|h| h.length())
                        .unwrap_or_else(|| delta.length());
                        let new_opposite = -delta.normalized() * opposite_len;
                        match side {
                            HandleSide::In => pt.handle_out = Some(new_opposite),
                            HandleSide::Out => pt.handle_in = Some(new_opposite),
                        }
                    }
                    PointType::Disconnected | PointType::Straight => {}
                }
            }
    }
}
