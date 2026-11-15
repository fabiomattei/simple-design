//! Pen-tool path creation and direct-selection editing of committed
//! `Path` layers: closing, inserting/deleting/converting anchors, point
//! types, corner radii and scissor cuts.

use egui::{Color32, Pos2, Rect, Stroke as EguiStroke, Vec2};

use crate::history::History;
use crate::model::{Frame, Layer, LayerId, LayerKind, Page, PathPoint, PointType, Style};
use crate::tools::Tool;

use super::layer_creation::insert_layer;
use super::path_flatten::flatten_path;
use super::{CanvasWidget, DragState, HANDLE_HIT_RADIUS, HANDLE_RADIUS, SELECTION_COLOR};

impl CanvasWidget {
    /// If a Pen path is in progress and `doc_pos` lands on top of its first
    /// anchor (within handle-hit distance, in screen space), finishes it as
    /// a closed path and returns true. Otherwise leaves state untouched.
    pub(super) fn try_close_pen_path(
        &mut self,
        doc_pos: Pos2,
        origin: Pos2,
        history: &mut History,
        selection: &mut Vec<LayerId>,
        tool: &mut Tool,
    ) -> bool {
        let Some(points) = &self.pen else { return false };
        if points.len() < 2 {
            return false;
        }
        let first = points[0].anchor;
        if self.to_screen(origin, first).distance(self.to_screen(origin, doc_pos)) > HANDLE_HIT_RADIUS {
            return false;
        }
        self.finish_pen_path(history, selection, tool, true);
        true
    }

    /// Commits the in-progress Pen path (if any, and if it has at least 2
    /// points) as a new `Path` layer, relativizing anchors to the layer's
    /// own frame the same way a `Group`/`Artboard` offsets its children.
    pub(super) fn finish_pen_path(
        &mut self,
        history: &mut History,
        selection: &mut Vec<LayerId>,
        tool: &mut Tool,
        closed: bool,
    ) {
        let Some(points) = self.pen.take() else { return };
        self.drag = DragState::None;
        if points.len() < 2 {
            return;
        }
        let bounds = points
            .iter()
            .map(|p| Rect::from_pos(p.anchor))
            .reduce(|a, b| a.union(b))
            .unwrap();
        let frame_pos = bounds.min;
        let relative_points: Vec<PathPoint> = points
            .iter()
            .map(|p| PathPoint {
                anchor: p.anchor - frame_pos.to_vec2(),
                handle_in: p.handle_in,
                handle_out: p.handle_out,
                point_type: p.point_type,
                corner_radius: p.corner_radius,
            })
            .collect();
        let frame = Frame {
            pos: frame_pos,
            size: bounds.size(),
            rotation: 0.0,
        };
        let layer = Layer::new("Path", frame, LayerKind::Path { points: relative_points, closed });
        let new_id = layer.id;
        history.snapshot();
        insert_layer(history.mutate().active_page_mut(), layer, frame_pos, None);
        *selection = vec![new_id];
        if *tool == Tool::Pen {
            *tool = Tool::Select;
        }
    }

    /// Closest anchor of the single selected `Path` to `screen_pos`, if any
    /// is within `HANDLE_HIT_RADIUS` — used by the right-click point-type
    /// menu, which (unlike the drag/hover machinery further down) needs a
    /// one-shot hit test at an arbitrary click position rather than a value
    /// already computed against the live hover position.
    pub(super) fn hit_test_path_anchor(
        &self,
        history: &History,
        selection: &[LayerId],
        origin: Pos2,
        screen_pos: Pos2,
    ) -> Option<(LayerId, usize)> {
        let [layer_id] = selection else { return None };
        let layer_id = *layer_id;
        let (frame_pos, offset, points, _closed) = read_path(history, layer_id)?;
        let mut best: Option<(usize, f32)> = None;
        for (i, p) in points.iter().enumerate() {
            let anchor_screen = self.to_screen(origin, p.anchor + frame_pos.to_vec2() + offset);
            let d = screen_pos.distance(anchor_screen);
            if best.map(|(_, bd)| d < bd).unwrap_or(true) {
                best = Some((i, d));
            }
        }
        let (index, dist) = best?;
        (dist <= HANDLE_HIT_RADIUS).then_some((layer_id, index))
    }

    /// Double-click a straight anchor to convert it to a curved (`Mirror`)
    /// point with a bisector-derived default handle pair, matching a common
    /// "double-click a corner to make it a curve" gesture — the caller
    /// (the double-click dispatch in `ui()`) checks this *before* falling
    /// back to `try_insert_path_point`'s segment-insert, so double-clicking
    /// directly on an anchor curves it instead of inserting a redundant
    /// duplicate point there. Returns `false` (does nothing) if no anchor is
    /// close enough, or the anchor already has a handle on either side —
    /// this only ever turns a straight corner into a curve, it's not a
    /// toggle back to straight on a second double-click.
    pub(super) fn try_convert_anchor_to_curve(
        &mut self,
        history: &mut History,
        selection: &[LayerId],
        doc_pos: Pos2,
        origin: Pos2,
    ) -> bool {
        let [layer_id] = selection else { return false };
        let layer_id = *layer_id;
        let Some((frame_pos, offset, points, closed)) = read_path(history, layer_id) else {
            return false;
        };
        let click_screen = self.to_screen(origin, doc_pos);
        let mut best: Option<(usize, f32)> = None;
        for (i, p) in points.iter().enumerate() {
            let screen = self.to_screen(origin, p.anchor + frame_pos.to_vec2() + offset);
            let d = click_screen.distance(screen);
            if best.map(|(_, bd)| d < bd).unwrap_or(true) {
                best = Some((i, d));
            }
        }
        let Some((index, dist)) = best else { return false };
        if dist > HANDLE_HIT_RADIUS {
            return false;
        }
        if points[index].handle_in.is_some() || points[index].handle_out.is_some() {
            return false;
        }
        let Some(handle_out) = default_curve_handle_out(&points, index, closed) else {
            return false;
        };
        history.snapshot();
        if let Some(layer) = history.mutate().active_page_mut().find_mut(layer_id)
            && let LayerKind::Path { points, .. } = &mut layer.kind
                && let Some(pt) = points.get_mut(index) {
                    pt.point_type = PointType::Mirror;
                    pt.handle_out = Some(handle_out);
                    pt.handle_in = Some(-handle_out);
                }
        true
    }

    /// Double-click-to-insert-anchor on a single selected `Path`: finds the
    /// segment whose chord is closest to `doc_pos` and splits it with a new
    /// plain corner point there. No-ops for anything else selected, or if
    /// the click isn't close enough to any segment.
    pub(super) fn try_insert_path_point(
        &mut self,
        history: &mut History,
        selection: &[LayerId],
        doc_pos: Pos2,
        origin: Pos2,
    ) {
        let [layer_id] = selection else { return };
        let layer_id = *layer_id;
        let Some((frame_pos, offset, points, closed)) = read_path(history, layer_id) else {
            return;
        };
        let n = points.len();
        let last = if closed { n } else { n.saturating_sub(1) };
        if last == 0 {
            return;
        }
        let click_screen = self.to_screen(origin, doc_pos);
        let mut best: Option<(usize, f32)> = None;
        for i in 0..last {
            let a = self.to_screen(origin, points[i].anchor + frame_pos.to_vec2() + offset);
            let b = self.to_screen(origin, points[(i + 1) % n].anchor + frame_pos.to_vec2() + offset);
            let d = distance_to_segment(click_screen, a, b);
            if best.map(|(_, bd)| d < bd).unwrap_or(true) {
                best = Some((i, d));
            }
        }
        let Some((seg_index, dist)) = best else { return };
        if dist > HANDLE_HIT_RADIUS * 2.0 {
            return;
        }
        let local_anchor = doc_pos - frame_pos.to_vec2() - offset;
        history.snapshot();
        if let Some(layer) = history.mutate().active_page_mut().find_mut(layer_id)
            && let LayerKind::Path { points, .. } = &mut layer.kind {
                points.insert(
                    seg_index + 1,
                    PathPoint {
                        anchor: local_anchor,
                        handle_in: None,
                        handle_out: None,
                        point_type: PointType::Disconnected,
                        corner_radius: 0.0,
                    },
                );
            }
    }

    /// Alt/Option-click-to-delete-anchor on a single selected `Path`.
    /// Refuses to drop below 2 points (a path needs at least that many to
    /// mean anything).
    pub(super) fn try_delete_path_point(
        &mut self,
        history: &mut History,
        selection: &[LayerId],
        doc_pos: Pos2,
        origin: Pos2,
    ) {
        let [layer_id] = selection else { return };
        let layer_id = *layer_id;
        let Some((frame_pos, offset, points, _closed)) = read_path(history, layer_id) else {
            return;
        };
        if points.len() <= 2 {
            return;
        }
        let click_screen = self.to_screen(origin, doc_pos);
        let hit = points.iter().position(|p| {
            let abs = p.anchor + frame_pos.to_vec2() + offset;
            self.to_screen(origin, abs).distance(click_screen) <= HANDLE_HIT_RADIUS
        });
        let Some(index) = hit else { return };
        history.snapshot();
        if let Some(layer) = history.mutate().active_page_mut().find_mut(layer_id)
            && let LayerKind::Path { points, .. } = &mut layer.kind {
                points.remove(index);
            }
        normalize_path_frame(history.mutate().active_page_mut(), layer_id);
    }

    /// Deletes every point in `selected_points` from the active single Path
    /// (`point_edit_layer`), used by the Delete/Backspace shortcut when a
    /// point selection is active (see `has_point_selection`). Unlike
    /// `try_delete_path_point`'s single-point "refuse if it would drop below
    /// 2 points" guard, a batch delete that would leave fewer than 2 points
    /// removes the whole layer instead of silently no-opping — matching
    /// common vector editors (deleting a path down to nothing deletes the path)
    /// and avoiding a Delete keypress that visibly does nothing, which is
    /// especially easy to trigger on a simple 2-point path (e.g. a
    /// Pen-drawn straight line) since clicking anywhere near either end
    /// selects that anchor rather than the layer.
    pub fn delete_selected_points(&mut self, history: &mut History, selection: &mut Vec<LayerId>) {
        let Some(layer_id) = self.point_edit_layer else { return };
        if self.selected_points.is_empty() {
            return;
        }
        let Some((_, _, points, _)) = read_path(history, layer_id) else {
            return;
        };
        let mut indices = self.selected_points.clone();
        indices.sort_unstable();
        indices.dedup();
        if points.len() < indices.len() + 2 {
            history.snapshot();
            history.mutate().active_page_mut().remove(layer_id);
            selection.retain(|&s| s != layer_id);
            self.selected_points.clear();
            self.point_edit_layer = None;
            return;
        }
        history.snapshot();
        if let Some(layer) = history.mutate().active_page_mut().find_mut(layer_id)
            && let LayerKind::Path { points, .. } = &mut layer.kind {
                for &index in indices.iter().rev() {
                    points.remove(index);
                }
            }
        normalize_path_frame(history.mutate().active_page_mut(), layer_id);
        self.selected_points.clear();
    }

    /// Applies a `PointType` change (the Num1-4 shortcuts) to every point in
    /// `selected_points` on `layer_id`, normalizing handle geometry so the
    /// change is visually immediate: `Straight` clears both handles,
    /// `Mirror` mirrors whichever handle is already set onto the other side
    /// (preferring `handle_out` if both happen to be set), or — if the point
    /// has no handle at all yet — generates the same chord-tangent default
    /// double-clicking an anchor would (`default_curve_handle_out`), so the
    /// shortcut always visibly curves a plain corner instead of silently
    /// no-oping on one. `Asymmetric` and `Disconnected` only change future
    /// handle-drag behavior (see `DragState::EditingPathHandle` handling),
    /// so no geometry changes on switch.
    pub(super) fn apply_point_type(&mut self, history: &mut History, layer_id: LayerId, point_type: PointType) {
        history.snapshot();
        if let Some(layer) = history.mutate().active_page_mut().find_mut(layer_id)
            && let LayerKind::Path { points, closed } = &mut layer.kind {
                let closed = *closed;
                for &index in &self.selected_points {
                    let existing = points.get(index).and_then(|pt| pt.handle_out.or(pt.handle_in.map(|h| -h)));
                    let default_mirror_handle = if point_type == PointType::Mirror && existing.is_none() {
                        default_curve_handle_out(points, index, closed)
                    } else {
                        None
                    };
                    let Some(pt) = points.get_mut(index) else { continue };
                    pt.point_type = point_type;
                    match point_type {
                        PointType::Straight => {
                            pt.handle_in = None;
                            pt.handle_out = None;
                        }
                        PointType::Mirror => {
                            if let Some(h) = existing.or(default_mirror_handle) {
                                pt.handle_out = Some(h);
                                pt.handle_in = Some(-h);
                            }
                        }
                        PointType::Asymmetric | PointType::Disconnected => {}
                    }
                }
            }
    }

    /// The first selected point's `corner_radius` on `layer_id`, for the
    /// inspector's radius field — `None` if nothing (or no `Path`) is
    /// selected. Multiple selected points with differing radii just show
    /// the first one's value (this codebase has no "Mixed" convention for
    /// per-point fields yet, unlike text runs' `mixed_or`).
    pub fn selected_point_corner_radius(&self, history: &History, layer_id: LayerId) -> Option<f32> {
        let &index = self.selected_points.first()?;
        let layer = history.get().find(layer_id)?;
        let LayerKind::Path { points, .. } = &layer.kind else { return None };
        points.get(index).map(|p| p.corner_radius)
    }

    /// Sets every point in `selected_points` on `layer_id` to `radius`, for
    /// the inspector's radius `DragValue`. Unlike `apply_point_type`/
    /// `apply_max_corner_radius` (each a discrete one-shot action), this is
    /// driven by a continuously-dragged `DragValue` — so, matching
    /// `ui/inspector/mod.rs`'s `should_snapshot` convention used everywhere else
    /// a `DragValue` writes through, snapshotting is the *caller's*
    /// responsibility (once per gesture), not done here on every call.
    pub fn apply_corner_radius(&mut self, history: &mut History, layer_id: LayerId, radius: f32) {
        if let Some(layer) = history.mutate().active_page_mut().find_mut(layer_id)
            && let LayerKind::Path { points, .. } = &mut layer.kind {
                for &index in &self.selected_points {
                    if let Some(pt) = points.get_mut(index) {
                        pt.corner_radius = radius;
                    }
                }
            }
    }

    /// Sets every point in `selected_points` on `layer_id` to *its own*
    /// geometric maximum radius (half its shorter adjacent segment — the
    /// same clamp `shapes::rounded_corner_arc_points` applies at render
    /// time), for the inspector's "Max" button. A one-shot snap to the
    /// current geometry's max, not a persisted "always track max" mode —
    /// editing the frame/points afterward doesn't keep this in sync.
    pub fn apply_max_corner_radius(&mut self, history: &mut History, layer_id: LayerId) {
        history.snapshot();
        if let Some(layer) = history.mutate().active_page_mut().find_mut(layer_id)
            && let LayerKind::Path { points, closed } = &mut layer.kind {
                let closed = *closed;
                let n = points.len();
                for &index in &self.selected_points {
                    if index >= n {
                        continue;
                    }
                    let prev = if index == 0 { closed.then(|| n - 1) } else { Some(index - 1) };
                    let next = if index == n - 1 { closed.then_some(0) } else { Some(index + 1) };
                    let (Some(prev), Some(next)) = (prev, next) else { continue };
                    let anchor = points[index].anchor;
                    let len_prev = (points[prev].anchor - anchor).length();
                    let len_next = (points[next].anchor - anchor).length();
                    points[index].corner_radius = len_prev.min(len_next) * 0.5;
                }
            }
    }

    /// Scissors-tool click on a single selected `Path`: cuts it at the
    /// clicked anchor or segment. Cutting a closed path opens it there
    /// (duplicating the cut point into two coincident endpoints so they can
    /// later be dragged apart); cutting an open path at an interior anchor
    /// or segment splits it into two separate `Path` layers. No-ops for
    /// anything else selected, an already-open endpoint, or a click too far
    /// from any anchor/segment.
    pub(super) fn try_scissor_path(
        &mut self,
        history: &mut History,
        selection: &mut Vec<LayerId>,
        doc_pos: Pos2,
        origin: Pos2,
    ) {
        let [layer_id] = selection.as_slice() else { return };
        let layer_id = *layer_id;
        let Some((frame_pos, offset, points, closed)) = read_path(history, layer_id) else {
            return;
        };
        let n = points.len();
        if n < 2 {
            return;
        }
        let base = frame_pos.to_vec2() + offset;
        let click_screen = self.to_screen(origin, doc_pos);

        let anchor_hit = points
            .iter()
            .position(|p| self.to_screen(origin, p.anchor + base).distance(click_screen) <= HANDLE_HIT_RADIUS);

        if let Some(i) = anchor_hit {
            if closed {
                let mut opened: Vec<PathPoint> = points[i..].to_vec();
                opened.extend_from_slice(&points[..i]);
                opened.push(points[i]);
                history.snapshot();
                if let Some(layer) = history.mutate().active_page_mut().find_mut(layer_id)
                    && let LayerKind::Path { points, closed } = &mut layer.kind {
                        *points = opened;
                        *closed = false;
                    }
                normalize_path_frame(history.mutate().active_page_mut(), layer_id);
            } else if i > 0 && i < n - 1 {
                let first = points[..=i].to_vec();
                let second = points[i..].to_vec();
                self.replace_with_split_paths(history, selection, layer_id, first, second);
            }
            return;
        }

        let last = if closed { n } else { n - 1 };
        let mut best: Option<(usize, f32)> = None;
        for i in 0..last {
            let a = self.to_screen(origin, points[i].anchor + base);
            let b = self.to_screen(origin, points[(i + 1) % n].anchor + base);
            let d = distance_to_segment(click_screen, a, b);
            if best.map(|(_, bd)| d < bd).unwrap_or(true) {
                best = Some((i, d));
            }
        }
        let Some((seg_index, dist)) = best else { return };
        if dist > HANDLE_HIT_RADIUS * 2.0 {
            return;
        }
        let new_point = PathPoint {
            anchor: doc_pos - base,
            handle_in: None,
            handle_out: None,
            point_type: PointType::Disconnected,
            corner_radius: 0.0,
        };

        if closed {
            let mut opened: Vec<PathPoint> = Vec::with_capacity(n + 2);
            opened.push(new_point);
            for k in 0..n {
                opened.push(points[(seg_index + 1 + k) % n]);
            }
            opened.push(new_point);
            history.snapshot();
            if let Some(layer) = history.mutate().active_page_mut().find_mut(layer_id)
                && let LayerKind::Path { points, closed } = &mut layer.kind {
                    *points = opened;
                    *closed = false;
                }
            normalize_path_frame(history.mutate().active_page_mut(), layer_id);
        } else {
            let mut first = points[..=seg_index].to_vec();
            first.push(new_point);
            let mut second = vec![new_point];
            second.extend_from_slice(&points[seg_index + 1..]);
            self.replace_with_split_paths(history, selection, layer_id, first, second);
        }
    }

    /// Replaces `layer_id` (an open `Path`) in place with two new `Path`
    /// layers built from `first`/`second` (each a run of points from the
    /// original, relative to its old `frame.pos`), spliced into the same
    /// parent list at the same position. Selects both new layers.
    pub(super) fn replace_with_split_paths(
        &mut self,
        history: &mut History,
        selection: &mut Vec<LayerId>,
        layer_id: LayerId,
        first: Vec<PathPoint>,
        second: Vec<PathPoint>,
    ) {
        history.snapshot();
        let page = history.mutate().active_page_mut();
        let Some(orig) = page.find(layer_id) else { return };
        let name = orig.name.clone();
        let style = orig.style.clone();
        let frame_pos = orig.frame.pos;
        let rotation = orig.frame.rotation;
        let layer_a = build_path_layer(&name, &style, frame_pos, rotation, &first);
        let layer_b = build_path_layer(&name, &style, frame_pos, rotation, &second);
        let id_a = layer_a.id;
        let id_b = layer_b.id;
        let Some(parent) = crate::grouping::find_common_parent_list(&mut page.layers, &[layer_id]) else {
            return;
        };
        let Some(idx) = parent.iter().position(|l| l.id == layer_id) else {
            return;
        };
        parent.remove(idx);
        parent.insert(idx, layer_b);
        parent.insert(idx, layer_a);
        *selection = vec![id_a, id_b];
    }
}

/// Reads out a `Path` layer's frame position, ancestor offset, and points
/// (cloned) in one short-lived immutable borrow of `history`, so callers can
/// use the result across a later `history.mutate()` call without holding
/// onto a reference into the document.
pub(super) fn read_path(history: &History, layer_id: LayerId) -> Option<(Pos2, Vec2, Vec<PathPoint>, bool)> {
    let doc = history.get();
    let page = doc.active_page();
    let layer = page.find(layer_id)?;
    let offset = page.absolute_offset(layer_id)?;
    if let LayerKind::Path { points, closed } = &layer.kind {
        Some((layer.frame.pos, offset, points.clone(), *closed))
    } else {
        None
    }
}

/// Default bezier `handle_out` for turning anchor `index` into a curve point
/// from scratch: a chord-parallel tangent running from the previous anchor
/// straight through to the next one — a simple, generally-smooth default; an
/// open path's own endpoint (only one neighbor) instead points along that
/// single adjacent segment. Length is 0.35 of whichever neighbor is closer,
/// clamped to `[4.0, 40.0]`. `None` if `index` has no usable neighbor (an
/// open path with a single point) or the neighbors are coincident with it.
/// Shared by `try_convert_anchor_to_curve`'s double-click and
/// `apply_point_type`'s keyboard Mirror shortcut, so a point gets the same
/// default curve either way.
pub(super) fn default_curve_handle_out(points: &[PathPoint], index: usize, closed: bool) -> Option<Vec2> {
    let n = points.len();
    let prev = if index == 0 { closed.then(|| n - 1) } else { Some(index - 1) };
    let next = if index == n - 1 { closed.then_some(0) } else { Some(index + 1) };
    let anchor = points[index].anchor;
    let tangent_vec = match (prev, next) {
        (Some(p), Some(nx)) => points[nx].anchor - points[p].anchor,
        (Some(p), None) => anchor - points[p].anchor,
        (None, Some(nx)) => points[nx].anchor - anchor,
        (None, None) => return None,
    };
    if tangent_vec.length() < 1e-3 {
        return None;
    }
    let tangent = tangent_vec.normalized();
    let len_prev = prev.map(|p| (points[p].anchor - anchor).length()).unwrap_or(f32::INFINITY);
    let len_next = next.map(|nx| (points[nx].anchor - anchor).length()).unwrap_or(f32::INFINITY);
    let handle_len = (len_prev.min(len_next) * 0.35).clamp(4.0, 40.0);
    Some(tangent * handle_len)
}

pub(super) fn distance_to_segment(p: Pos2, a: Pos2, b: Pos2) -> f32 {
    let ab = b - a;
    let len2 = ab.length_sq();
    if len2 < 1e-6 {
        return p.distance(a);
    }
    let t = ((p - a).dot(ab) / len2).clamp(0.0, 1.0);
    p.distance(a + ab * t)
}

/// Recomputes a `Path` layer's `frame` as the tight bounding box of its
/// anchors (handles aren't included, matching `finish_pen_path`), shifting
/// every point's relative anchor so their absolute positions don't move.
/// Anchor edits update `frame.pos`/`points` independently mid-drag (see
/// `DragState::EditingPathAnchor`), so this restores the invariant that
/// `frame` is the anchors' bounding box once the gesture ends.
pub(super) fn normalize_path_frame(page: &mut Page, layer_id: LayerId) {
    let Some(layer) = page.find_mut(layer_id) else { return };
    let old_frame_pos = layer.frame.pos;
    let rotation = layer.frame.rotation;
    let LayerKind::Path { points, .. } = &mut layer.kind else { return };
    if points.is_empty() {
        return;
    }
    let bounds = points
        .iter()
        .map(|p| Rect::from_pos(old_frame_pos + p.anchor.to_vec2()))
        .reduce(|a, b| a.union(b))
        .unwrap();
    for pt in points.iter_mut() {
        let abs = old_frame_pos + pt.anchor.to_vec2();
        pt.anchor = abs - (bounds.min.to_vec2());
    }
    layer.frame = Frame {
        pos: bounds.min,
        size: bounds.size(),
        rotation,
    };
}

/// Builds a new open `Path` layer from `points` (anchors relative to
/// `old_frame_pos`, the source layer's `frame.pos` — same convention
/// `try_scissor_path` reads them in), recomputing a tight bounding-box frame
/// the same way `finish_pen_path` does. Used to materialize the two pieces a
/// Scissors cut splits an open path into. `rotation` carries over the source
/// layer's rotation unchanged; since the new frame's bounds (and therefore
/// rotation pivot) generally differ from the original, a cut piece of a
/// rotated path can shift slightly relative to where it visually sat before
/// the cut — an accepted approximation, not exact-pivot-preserving.
pub(super) fn build_path_layer(name: &str, style: &Style, old_frame_pos: Pos2, rotation: f32, points: &[PathPoint]) -> Layer {
    let absolute: Vec<PathPoint> = points
        .iter()
        .map(|p| PathPoint {
            anchor: p.anchor + old_frame_pos.to_vec2(),
            handle_in: p.handle_in,
            handle_out: p.handle_out,
            point_type: p.point_type,
            corner_radius: p.corner_radius,
        })
        .collect();
    let bounds = absolute
        .iter()
        .map(|p| Rect::from_pos(p.anchor))
        .reduce(|a, b| a.union(b))
        .unwrap();
    let relative: Vec<PathPoint> = absolute
        .iter()
        .map(|p| PathPoint {
            anchor: p.anchor - bounds.min.to_vec2(),
            handle_in: p.handle_in,
            handle_out: p.handle_out,
            point_type: p.point_type,
            corner_radius: p.corner_radius,
        })
        .collect();
    let frame = Frame {
        pos: bounds.min,
        size: bounds.size(),
        rotation,
    };
    let mut layer = Layer::new(name, frame, LayerKind::Path { points: relative, closed: false });
    layer.style = style.clone();
    layer
}

/// Draws the in-progress Pen path: the committed segments so far, a
/// rubber-band preview segment to the current mouse position, and small
/// handles at each anchor/control point so the user can see what they're
/// placing.
pub(super) fn draw_pen_preview(
    painter: &egui::Painter,
    points: &[PathPoint],
    origin: Pos2,
    pan: Vec2,
    zoom: f32,
    hover_pos: Option<Pos2>,
) {
    let to_screen = |p: Pos2| origin + pan + p.to_vec2() * zoom;
    let stroke = EguiStroke::new(1.5, SELECTION_COLOR);

    let flat = flatten_path(points, false);
    let screen_points: Vec<Pos2> = flat.iter().map(|&p| to_screen(p)).collect();
    if screen_points.len() >= 2 {
        painter.add(egui::Shape::Path(egui::epaint::PathShape {
            points: screen_points,
            closed: false,
            fill: Color32::TRANSPARENT,
            stroke: stroke.into(),
        }));
    }
    if let (Some(last), Some(hover)) = (points.last(), hover_pos) {
        painter.line_segment(
            [to_screen(last.anchor), hover],
            EguiStroke::new(1.0, SELECTION_COLOR.gamma_multiply(0.6)),
        );
    }

    for (i, p) in points.iter().enumerate() {
        let anchor_screen = to_screen(p.anchor);
        for h in [p.handle_in, p.handle_out].into_iter().flatten() {
            let handle_screen = to_screen(p.anchor + h);
            painter.line_segment(
                [anchor_screen, handle_screen],
                EguiStroke::new(1.0, Color32::from_gray(120)),
            );
            painter.circle_filled(handle_screen, 3.0, Color32::WHITE);
            painter.circle_stroke(handle_screen, 3.0, EguiStroke::new(1.0, Color32::from_gray(120)));
        }
        let is_first = i == 0;
        let radius = if is_first { HANDLE_RADIUS } else { HANDLE_RADIUS * 0.7 };
        painter.circle(anchor_screen, radius, Color32::WHITE, stroke);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Document, Paint};

    fn straight_point(x: f32, y: f32) -> PathPoint {
        PathPoint {
            anchor: Pos2::new(x, y),
            handle_in: None,
            handle_out: None,
            point_type: PointType::Disconnected,
            corner_radius: 0.0,
        }
    }

    fn setup(points: Vec<PathPoint>, closed: bool) -> (CanvasWidget, History, Vec<LayerId>, LayerId) {
        let layer = Layer::new(
            "Path",
            Frame { pos: Pos2::ZERO, size: Vec2::new(40.0, 40.0), rotation: 0.0 },
            LayerKind::Path { points, closed },
        );
        let id = layer.id;
        let mut doc = Document::new();
        doc.active_page_mut().layers.push(layer);
        let history = History::new(doc);
        let widget = CanvasWidget::default();
        (widget, history, vec![id], id)
    }

    #[test]
    fn double_click_on_straight_anchor_converts_to_curved() {
        let square = vec![
            straight_point(0.0, 0.0),
            straight_point(20.0, 0.0),
            straight_point(20.0, 20.0),
            straight_point(0.0, 20.0),
        ];
        let (mut widget, mut history, selection, id) = setup(square, true);

        let converted = widget.try_convert_anchor_to_curve(&mut history, &selection, Pos2::new(20.0, 0.0), Pos2::ZERO);
        assert!(converted, "clicking directly on a straight anchor should convert it");

        let layer = history.get().active_page().find(id).unwrap();
        let LayerKind::Path { points, .. } = &layer.kind else { unreachable!() };
        assert_eq!(points[1].point_type, PointType::Mirror);
        assert!(points[1].handle_in.is_some());
        assert!(points[1].handle_out.is_some());
        // Mirror semantics: handles are exact opposites.
        assert_eq!(points[1].handle_in, points[1].handle_out.map(|h| -h));
    }

    #[test]
    fn right_click_hit_test_finds_the_nearest_anchor_within_radius() {
        let square = vec![
            straight_point(0.0, 0.0),
            straight_point(20.0, 0.0),
            straight_point(20.0, 20.0),
            straight_point(0.0, 20.0),
        ];
        let (widget, history, selection, id) = setup(square, true);

        let hit = widget.hit_test_path_anchor(&history, &selection, Pos2::ZERO, Pos2::new(20.0, 0.0));
        assert_eq!(hit, Some((id, 1)));
    }

    #[test]
    fn right_click_hit_test_misses_a_click_far_from_every_anchor() {
        let square = vec![
            straight_point(0.0, 0.0),
            straight_point(20.0, 0.0),
            straight_point(20.0, 20.0),
            straight_point(0.0, 20.0),
        ];
        let (widget, history, selection, _id) = setup(square, true);

        // Segment midpoint, same as the double-click miss case above — not
        // within `HANDLE_HIT_RADIUS` of any anchor.
        let hit = widget.hit_test_path_anchor(&history, &selection, Pos2::ZERO, Pos2::new(10.0, 0.0));
        assert_eq!(hit, None);
    }

    #[test]
    fn double_click_on_segment_midpoint_does_not_convert_so_insert_still_applies() {
        let square = vec![
            straight_point(0.0, 0.0),
            straight_point(20.0, 0.0),
            straight_point(20.0, 20.0),
            straight_point(0.0, 20.0),
        ];
        let (mut widget, mut history, selection, id) = setup(square, true);

        // A point on the segment between anchors 0 and 1, far from either
        // anchor — the double-click dispatch's regression requirement is
        // that this still falls through to inserting a new point, not
        // silently doing nothing.
        let midpoint = Pos2::new(10.0, 0.0);
        let converted = widget.try_convert_anchor_to_curve(&mut history, &selection, midpoint, Pos2::ZERO);
        assert!(!converted, "a segment midpoint isn't an anchor, so it shouldn't convert");

        widget.try_insert_path_point(&mut history, &selection, midpoint, Pos2::ZERO);
        let layer = history.get().active_page().find(id).unwrap();
        let LayerKind::Path { points, .. } = &layer.kind else { unreachable!() };
        assert_eq!(points.len(), 5, "insert should still add a point when the click wasn't on an anchor");
    }

    #[test]
    fn double_click_on_already_curved_anchor_does_nothing() {
        let points = vec![
            straight_point(0.0, 0.0),
            PathPoint {
                anchor: Pos2::new(20.0, 0.0),
                handle_in: Some(Vec2::new(-5.0, 0.0)),
                handle_out: Some(Vec2::new(5.0, 0.0)),
                point_type: PointType::Mirror,
                corner_radius: 0.0,
            },
            straight_point(20.0, 20.0),
        ];
        let (mut widget, mut history, selection, _id) = setup(points, false);
        let converted = widget.try_convert_anchor_to_curve(&mut history, &selection, Pos2::new(20.0, 0.0), Pos2::ZERO);
        assert!(!converted, "an anchor that already has handles shouldn't be re-converted");
    }

    #[test]
    fn cutting_closed_path_at_an_anchor_opens_it_with_a_duplicated_endpoint() {
        let square = vec![
            straight_point(0.0, 0.0),
            straight_point(40.0, 0.0),
            straight_point(40.0, 40.0),
            straight_point(0.0, 40.0),
        ];
        let (mut widget, mut history, mut selection, id) = setup(square, true);

        widget.try_scissor_path(&mut history, &mut selection, Pos2::new(0.0, 0.0), Pos2::ZERO);

        let layer = history.get().active_page().find(id).unwrap();
        let LayerKind::Path { points, closed } = &layer.kind else {
            panic!("expected Path");
        };
        assert!(!closed);
        assert_eq!(points.len(), 5);
        assert_eq!(selection, vec![id]);
        let first_abs = points[0].anchor + layer.frame.pos.to_vec2();
        let last_abs = points[4].anchor + layer.frame.pos.to_vec2();
        assert_eq!(first_abs, last_abs);
    }

    #[test]
    fn cutting_closed_path_at_a_segment_opens_it_with_an_inserted_point() {
        let square = vec![
            straight_point(0.0, 0.0),
            straight_point(40.0, 0.0),
            straight_point(40.0, 40.0),
            straight_point(0.0, 40.0),
        ];
        let (mut widget, mut history, mut selection, id) = setup(square, true);

        // Midpoint of the bottom edge, far from any anchor.
        widget.try_scissor_path(&mut history, &mut selection, Pos2::new(20.0, 0.0), Pos2::ZERO);

        let layer = history.get().active_page().find(id).unwrap();
        let LayerKind::Path { points, closed } = &layer.kind else {
            panic!("expected Path");
        };
        assert!(!closed);
        assert_eq!(points.len(), 6);
        let first_abs = points[0].anchor + layer.frame.pos.to_vec2();
        let last_abs = points[points.len() - 1].anchor + layer.frame.pos.to_vec2();
        assert_eq!(first_abs, last_abs);
        assert!((first_abs - Pos2::new(20.0, 0.0)).length() < 0.01);
    }

    #[test]
    fn cutting_open_path_at_an_interior_anchor_splits_it_into_two_layers() {
        let points = vec![
            straight_point(0.0, 0.0),
            straight_point(20.0, 0.0),
            straight_point(40.0, 0.0),
            straight_point(60.0, 0.0),
        ];
        let (mut widget, mut history, mut selection, original_id) = setup(points, false);

        widget.try_scissor_path(&mut history, &mut selection, Pos2::new(40.0, 0.0), Pos2::ZERO);

        assert_eq!(selection.len(), 2);
        assert!(!selection.contains(&original_id));
        let page = history.get().active_page();
        assert!(page.find(original_id).is_none());
        assert_eq!(page.layers.len(), 2);

        let layer_a = page.find(selection[0]).unwrap();
        let layer_b = page.find(selection[1]).unwrap();
        let LayerKind::Path { points: pa, closed: ca } = &layer_a.kind else { panic!() };
        let LayerKind::Path { points: pb, closed: cb } = &layer_b.kind else { panic!() };
        assert!(!ca && !cb);
        assert_eq!(pa.len(), 3);
        assert_eq!(pb.len(), 2);
    }

    #[test]
    fn cutting_open_path_at_an_interior_segment_splits_it_and_inserts_the_cut_point() {
        let points = vec![straight_point(0.0, 0.0), straight_point(40.0, 0.0)];
        let (mut widget, mut history, mut selection, original_id) = setup(points, false);

        widget.try_scissor_path(&mut history, &mut selection, Pos2::new(20.0, 0.0), Pos2::ZERO);

        assert_eq!(selection.len(), 2);
        let page = history.get().active_page();
        assert!(page.find(original_id).is_none());
        let layer_a = page.find(selection[0]).unwrap();
        let layer_b = page.find(selection[1]).unwrap();
        let LayerKind::Path { points: pa, .. } = &layer_a.kind else { panic!() };
        let LayerKind::Path { points: pb, .. } = &layer_b.kind else { panic!() };
        assert_eq!(pa.len(), 2);
        assert_eq!(pb.len(), 2);
        let cut_abs = pa[1].anchor + layer_a.frame.pos.to_vec2();
        assert!((cut_abs - Pos2::new(20.0, 0.0)).length() < 0.01);
    }

    #[test]
    fn cutting_open_path_at_its_endpoint_is_a_noop() {
        let points = vec![
            straight_point(0.0, 0.0),
            straight_point(20.0, 0.0),
            straight_point(40.0, 0.0),
        ];
        let (mut widget, mut history, mut selection, id) = setup(points, false);

        widget.try_scissor_path(&mut history, &mut selection, Pos2::new(0.0, 0.0), Pos2::ZERO);

        assert_eq!(selection, vec![id]);
        assert!(!history.can_undo());
        let layer = history.get().active_page().find(id).unwrap();
        let LayerKind::Path { points, .. } = &layer.kind else { panic!() };
        assert_eq!(points.len(), 3);
    }

    #[test]
    fn deleting_selected_points_removes_exactly_those_and_renormalizes_frame() {
        let points = vec![
            straight_point(0.0, 0.0),
            straight_point(20.0, 0.0),
            straight_point(40.0, 0.0),
            straight_point(40.0, 40.0),
            straight_point(0.0, 40.0),
        ];
        let (mut widget, mut history, mut selection, id) = setup(points, true);
        widget.point_edit_layer = Some(id);
        widget.selected_points = vec![1, 3];

        widget.delete_selected_points(&mut history, &mut selection);

        assert!(widget.selected_points.is_empty());
        let layer = history.get().active_page().find(id).unwrap();
        let LayerKind::Path { points, .. } = &layer.kind else { panic!() };
        assert_eq!(points.len(), 3);
        let abs: Vec<Pos2> = points.iter().map(|p| p.anchor + layer.frame.pos.to_vec2()).collect();
        assert_eq!(abs, vec![Pos2::new(0.0, 0.0), Pos2::new(40.0, 0.0), Pos2::new(0.0, 40.0)]);
    }

    #[test]
    fn deleting_selected_points_below_two_remaining_deletes_the_whole_layer() {
        let points = vec![straight_point(0.0, 0.0), straight_point(20.0, 0.0), straight_point(40.0, 0.0)];
        let (mut widget, mut history, mut selection, id) = setup(points, false);
        widget.point_edit_layer = Some(id);
        widget.selected_points = vec![0, 1];

        widget.delete_selected_points(&mut history, &mut selection);

        assert!(history.can_undo());
        assert!(history.get().active_page().find(id).is_none());
        assert!(selection.is_empty());
        assert!(widget.selected_points.is_empty());
        assert!(widget.point_edit_layer.is_none());
    }

    #[test]
    fn deleting_a_pen_drawn_lines_only_anchor_selection_deletes_the_line() {
        let points = vec![straight_point(0.0, 0.0), straight_point(20.0, 0.0)];
        let (mut widget, mut history, mut selection, id) = setup(points, false);
        widget.point_edit_layer = Some(id);
        widget.selected_points = vec![0];

        widget.delete_selected_points(&mut history, &mut selection);

        assert!(history.get().active_page().find(id).is_none());
        assert!(selection.is_empty());
    }

    #[test]
    fn switching_selected_points_to_mirror_mirrors_the_existing_handle() {
        let mut point = straight_point(20.0, 20.0);
        point.handle_out = Some(Vec2::new(10.0, 0.0));
        point.point_type = PointType::Disconnected;
        let (mut widget, mut history, _selection, id) = setup(vec![point, straight_point(40.0, 40.0)], false);
        widget.point_edit_layer = Some(id);
        widget.selected_points = vec![0];

        widget.apply_point_type(&mut history, id, PointType::Mirror);

        let layer = history.get().active_page().find(id).unwrap();
        let LayerKind::Path { points, .. } = &layer.kind else { panic!() };
        assert_eq!(points[0].point_type, PointType::Mirror);
        assert_eq!(points[0].handle_out, Some(Vec2::new(10.0, 0.0)));
        assert_eq!(points[0].handle_in, Some(Vec2::new(-10.0, 0.0)));
    }

    #[test]
    fn switching_a_handle_less_point_to_mirror_generates_a_default_curve() {
        let (mut widget, mut history, _selection, id) = setup(
            vec![straight_point(0.0, 0.0), straight_point(20.0, 0.0), straight_point(40.0, 0.0)],
            false,
        );
        widget.point_edit_layer = Some(id);
        widget.selected_points = vec![1];

        widget.apply_point_type(&mut history, id, PointType::Mirror);

        let layer = history.get().active_page().find(id).unwrap();
        let LayerKind::Path { points, .. } = &layer.kind else { panic!() };
        assert_eq!(points[1].point_type, PointType::Mirror);
        assert!(points[1].handle_out.is_some(), "Mirror on a plain corner must generate a visible curve");
        assert_eq!(points[1].handle_out, points[1].handle_in.map(|h| -h));
    }

    #[test]
    fn switching_selected_points_to_straight_clears_both_handles() {
        let mut point = straight_point(20.0, 20.0);
        point.handle_in = Some(Vec2::new(-10.0, 0.0));
        point.handle_out = Some(Vec2::new(10.0, 0.0));
        point.point_type = PointType::Mirror;
        let (mut widget, mut history, _selection, id) = setup(vec![point, straight_point(40.0, 40.0)], false);
        widget.point_edit_layer = Some(id);
        widget.selected_points = vec![0];

        widget.apply_point_type(&mut history, id, PointType::Straight);

        let layer = history.get().active_page().find(id).unwrap();
        let LayerKind::Path { points, .. } = &layer.kind else { panic!() };
        assert_eq!(points[0].point_type, PointType::Straight);
        assert_eq!(points[0].handle_in, None);
        assert_eq!(points[0].handle_out, None);
    }

    /// Reproduces the actual crash report: scissoring a closed, filled
    /// Pen-drawn figure open, then rendering it. `draw_layer` used to pass
    /// the layer's fill straight through to `PathShape` regardless of
    /// `closed`; epaint's tessellator debug-asserts that an open path has a
    /// transparent fill ("You asked to fill a path that is not closed"),
    /// which panics (and thus crashes the whole app) in a debug build —
    /// exactly what `cargo run` uses. Exercises the same fill-selection
    /// logic `draw_layer` uses, through the real epaint tessellator, so a
    /// regression here fails loudly instead of silently reintroducing the
    /// panic.
    #[test]
    fn opening_a_filled_closed_path_with_scissors_does_not_panic_when_rendered() {
        let square = vec![
            straight_point(0.0, 0.0),
            straight_point(40.0, 0.0),
            straight_point(40.0, 40.0),
            straight_point(0.0, 40.0),
        ];
        let (mut widget, mut history, mut selection, id) = setup(square, true);

        let layer_before = history.get().active_page().find(id).unwrap();
        assert!(layer_before.style.fill.is_some(), "a Pen-drawn figure is filled by default");

        widget.try_scissor_path(&mut history, &mut selection, Pos2::new(0.0, 0.0), Pos2::ZERO);

        let layer = history.get().active_page().find(id).unwrap();
        let LayerKind::Path { points, closed } = &layer.kind else {
            panic!("expected Path");
        };
        assert!(!closed, "scissoring a closed path should open it");
        assert!(layer.style.fill.is_some(), "cutting shouldn't itself clear the fill");

        let screen_points = flatten_path(points, *closed);
        let fill = layer.style.fill.as_ref().map(Paint::to_color32).unwrap_or(Color32::TRANSPARENT);
        let path_fill = if *closed { fill } else { Color32::TRANSPARENT };
        let shape = egui::epaint::PathShape {
            points: screen_points,
            closed: *closed,
            fill: path_fill,
            stroke: egui::Stroke::NONE.into(),
        };

        let mut tessellator = egui::epaint::Tessellator::new(
            1.0,
            egui::epaint::TessellationOptions::default(),
            [1, 1],
            Vec::new(),
        );
        let mut mesh = egui::epaint::Mesh::default();
        tessellator.tessellate_path(&shape, &mut mesh);
    }

    /// Reproduces cutting a Pen-drawn figure: curved anchors (bezier
    /// handles set, unlike the straight corners the other tests use), a
    /// non-origin `frame.pos` (a Pen path's frame is the bounding box of
    /// wherever it was drawn, not (0,0)), and nested one level inside an
    /// Artboard (so `offset` is also non-zero). Cuts at every anchor and
    /// every segment midpoint, for both an open and a closed path, to probe
    /// for a panic reported when scissoring a Pen-made shape.
    #[test]
    fn cutting_a_pen_drawn_curved_path_at_every_anchor_and_segment_does_not_panic() {
        fn curved_point(x: f32, y: f32) -> PathPoint {
            PathPoint {
                anchor: Pos2::new(x, y),
                handle_in: Some(Vec2::new(-8.0, 3.0)),
                handle_out: Some(Vec2::new(8.0, -3.0)),
                point_type: PointType::Disconnected,
                corner_radius: 0.0,
            }
        }

        for closed in [false, true] {
            let base_points = vec![
                curved_point(0.0, 0.0),
                curved_point(30.0, -10.0),
                curved_point(60.0, 0.0),
                curved_point(60.0, 40.0),
                curved_point(30.0, 50.0),
                curved_point(0.0, 40.0),
            ];
            let n = base_points.len();

            let frame_pos = Pos2::new(150.0, 200.0);
            let artboard_offset = Vec2::new(500.0, 300.0);

            let make_history = || {
                let layer = Layer::new(
                    "Path",
                    Frame { pos: frame_pos, size: Vec2::new(60.0, 50.0), rotation: 0.0 },
                    LayerKind::Path { points: base_points.clone(), closed },
                );
                let path_id = layer.id;
                let mut artboard = Layer::new_artboard(
                    "Artboard",
                    Frame { pos: artboard_offset.to_pos2(), size: Vec2::new(800.0, 600.0), rotation: 0.0 },
                );
                if let LayerKind::Artboard { children, .. } = &mut artboard.kind {
                    children.push(layer);
                }
                let mut doc = Document::new();
                doc.active_page_mut().layers.push(artboard);
                (History::new(doc), path_id)
            };

            // Anchor cuts: rebuild fresh state for each index since cutting
            // mutates/replaces the layer.
            for point in &base_points {
                let (mut history, path_id) = make_history();
                let mut selection = vec![path_id];
                let mut widget = CanvasWidget::default();
                let abs = point.anchor + frame_pos.to_vec2() + artboard_offset;
                widget.try_scissor_path(&mut history, &mut selection, abs, Pos2::ZERO);
            }

            // Segment-midpoint cuts.
            let last = if closed { n } else { n - 1 };
            for i in 0..last {
                let (mut history, path_id) = make_history();
                let mut selection = vec![path_id];
                let mut widget = CanvasWidget::default();
                let a = base_points[i].anchor;
                let b = base_points[(i + 1) % n].anchor;
                let mid = Pos2::new((a.x + b.x) / 2.0, (a.y + b.y) / 2.0) + frame_pos.to_vec2() + artboard_offset;
                widget.try_scissor_path(&mut history, &mut selection, mid, Pos2::ZERO);
            }
        }
    }
}
