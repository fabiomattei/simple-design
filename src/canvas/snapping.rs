//! Drag-time feedback: snapping a moved/resized selection to guides and
//! other layers' edges/centers, drawing the snap lines, and the Option-hover
//! distance measurement between two layers.

use egui::{Color32, Pos2, Rect, Stroke as EguiStroke, Vec2};

use crate::model::{GuideOrientation, LayerId, Page};

use super::DragState;

/// Color of the transient "smart guide" alignment lines shown while
/// snapping to another layer's edge/center during a drag.
const SNAP_LINE_COLOR: Color32 = Color32::from_rgb(255, 0, 170);
/// The Option-hover distance-measurement overlay color.
const MEASURE_COLOR: Color32 = Color32::from_rgb(255, 64, 64);
/// Snap distance, in screen pixels (divided by zoom to get a doc-space
/// threshold), for guides/object-edge/pixel snapping.
const SNAP_THRESHOLD_SCREEN: f32 = 6.0;

/// Doc-space x/y values a drag can snap to: guide positions plus the edges
/// and centers of every other (non-dragged) top-level layer on the page.
/// Built fresh once per drag frame from live document state.
pub(super) struct SnapCandidates {
    xs: Vec<f32>,
    ys: Vec<f32>,
}

/// Ids that must be excluded from snap candidates because they're the ones
/// being moved/resized this drag (a layer shouldn't snap to its own edge).
pub(super) fn drag_exclude_ids(drag: &DragState) -> Vec<LayerId> {
    match drag {
        DragState::MovingSelection { starts, .. } => starts.iter().map(|(id, _)| *id).collect(),
        DragState::ResizingLine { id, .. } => vec![*id],
        DragState::ResizingGroup { layers, containers, .. } => {
            layers.iter().map(|l| l.id).chain(containers.iter().map(|c| c.original.id)).collect()
        }
        DragState::Rotating { layers, .. } => layers.iter().map(|l| l.id).collect(),
        _ => Vec::new(),
    }
}

/// Only top-level layers are considered, consistent with the marquee-select
/// simplification elsewhere (see module docs): nested descendants inside
/// artboards/groups don't contribute snap targets.
pub(super) fn build_snap_candidates(page: &Page, exclude: &[LayerId]) -> SnapCandidates {
    let mut xs: Vec<f32> = page
        .guides
        .iter()
        .filter(|g| g.orientation == GuideOrientation::Vertical)
        .map(|g| g.pos)
        .collect();
    let mut ys: Vec<f32> = page
        .guides
        .iter()
        .filter(|g| g.orientation == GuideOrientation::Horizontal)
        .map(|g| g.pos)
        .collect();
    for layer in &page.layers {
        if !layer.visible || exclude.contains(&layer.id) {
            continue;
        }
        let b = layer.frame.bounds();
        xs.extend([b.min.x, b.center().x, b.max.x]);
        ys.extend([b.min.y, b.center().y, b.max.y]);
    }
    SnapCandidates { xs, ys }
}

/// Closest candidate to `value` within `threshold`, if any.
pub(super) fn snap_value(value: f32, candidates: &[f32], threshold: f32) -> Option<f32> {
    candidates
        .iter()
        .copied()
        .map(|c| (c, (c - value).abs()))
        .filter(|&(_, d)| d <= threshold)
        .min_by(|a, b| a.1.partial_cmp(&b.1).unwrap())
        .map(|(c, _)| c)
}

/// Snaps a single dragged doc-space point (a shape corner, a line endpoint,
/// a resize handle) independently on each axis: first against guide/layer
/// candidates, falling back to the nearest whole pixel if `pixel_snap` is on
/// and nothing else matched. Returns the adjusted point plus whichever
/// candidate line matched on each axis (for smart-guide visual feedback);
/// the pixel-snap fallback never produces a feedback line, since it isn't
/// aligning to anything else on the page.
pub(super) fn snap_point(
    p: Pos2,
    candidates: &SnapCandidates,
    zoom: f32,
    snap_enabled: bool,
    pixel_snap: bool,
) -> (Pos2, Option<f32>, Option<f32>) {
    if !snap_enabled {
        return (p, None, None);
    }
    let threshold = SNAP_THRESHOLD_SCREEN / zoom;
    let (x, line_x) = match snap_value(p.x, &candidates.xs, threshold) {
        Some(sx) => (sx, Some(sx)),
        None if pixel_snap => (p.x.round(), None),
        None => (p.x, None),
    };
    let (y, line_y) = match snap_value(p.y, &candidates.ys, threshold) {
        Some(sy) => (sy, Some(sy)),
        None if pixel_snap => (p.y.round(), None),
        None => (p.y, None),
    };
    (Pos2::new(x, y), line_x, line_y)
}

/// Snaps a translation applied to a moving selection's overall bounding box:
/// tries each of the box's left/center/right edges (and top/center/bottom)
/// against the candidates, keeping whichever needs the smallest correction,
/// so the whole selection can align by its center as well as its edges. As
/// with `snap_point`, falls back to whole-pixel snap of the box's min corner.
pub(super) fn snap_bounds_delta(
    bounds: Rect,
    delta: Vec2,
    candidates: &SnapCandidates,
    zoom: f32,
    snap_enabled: bool,
    pixel_snap: bool,
) -> (Vec2, Option<f32>, Option<f32>) {
    if !snap_enabled {
        return (delta, None, None);
    }
    let threshold = SNAP_THRESHOLD_SCREEN / zoom;
    let moved = bounds.translate(delta);

    let mut best_x: Option<(f32, f32)> = None;
    for v in [moved.min.x, moved.center().x, moved.max.x] {
        if let Some(sv) = snap_value(v, &candidates.xs, threshold) {
            let corr = sv - v;
            if best_x.map(|(c, _)| corr.abs() < c.abs()).unwrap_or(true) {
                best_x = Some((corr, sv));
            }
        }
    }
    let mut best_y: Option<(f32, f32)> = None;
    for v in [moved.min.y, moved.center().y, moved.max.y] {
        if let Some(sv) = snap_value(v, &candidates.ys, threshold) {
            let corr = sv - v;
            if best_y.map(|(c, _)| corr.abs() < c.abs()).unwrap_or(true) {
                best_y = Some((corr, sv));
            }
        }
    }

    let mut new_delta = delta;
    let line_x = match best_x {
        Some((corr, sv)) => {
            new_delta.x += corr;
            Some(sv)
        }
        None if pixel_snap => {
            new_delta.x += moved.min.x.round() - moved.min.x;
            None
        }
        None => None,
    };
    let line_y = match best_y {
        Some((corr, sv)) => {
            new_delta.y += corr;
            Some(sv)
        }
        None if pixel_snap => {
            new_delta.y += moved.min.y.round() - moved.min.y;
            None
        }
        None => None,
    };
    (new_delta, line_x, line_y)
}

/// Snapped doc-space value for whichever axis a guide being dragged out (or
/// relocated) controls: `y` for a `Horizontal` guide, `x` for `Vertical`.
pub(super) fn snap_guide_axis(
    orientation: GuideOrientation,
    doc_pos: Pos2,
    candidates: &SnapCandidates,
    zoom: f32,
    snap_enabled: bool,
    pixel_snap: bool,
) -> f32 {
    let raw = match orientation {
        GuideOrientation::Horizontal => doc_pos.y,
        GuideOrientation::Vertical => doc_pos.x,
    };
    if !snap_enabled {
        return raw;
    }
    let threshold = SNAP_THRESHOLD_SCREEN / zoom;
    let axis_candidates = match orientation {
        GuideOrientation::Horizontal => &candidates.ys,
        GuideOrientation::Vertical => &candidates.xs,
    };
    match snap_value(raw, axis_candidates, threshold) {
        Some(v) => v,
        None if pixel_snap => raw.round(),
        None => raw,
    }
}

/// Draws the transient pink "smart guide" alignment line(s) for whichever
/// axes matched a candidate this frame, spanning the full canvas area.
pub(super) fn draw_snap_lines(
    painter: &egui::Painter,
    canvas_rect: Rect,
    origin: Pos2,
    pan: Vec2,
    zoom: f32,
    line_x: Option<f32>,
    line_y: Option<f32>,
) {
    let to_screen = |p: Pos2| origin + pan + p.to_vec2() * zoom;
    let stroke = EguiStroke::new(1.0, SNAP_LINE_COLOR);
    if let Some(x) = line_x {
        let sx = to_screen(Pos2::new(x, 0.0)).x;
        painter.line_segment(
            [Pos2::new(sx, canvas_rect.min.y), Pos2::new(sx, canvas_rect.max.y)],
            stroke,
        );
    }
    if let Some(y) = line_y {
        let sy = to_screen(Pos2::new(0.0, y)).y;
        painter.line_segment(
            [Pos2::new(canvas_rect.min.x, sy), Pos2::new(canvas_rect.max.x, sy)],
            stroke,
        );
    }
}

pub(super) fn draw_dashed_line(painter: &egui::Painter, a: Pos2, b: Pos2, stroke: EguiStroke) {
    const DASH_LEN: f32 = 4.0;
    const GAP_LEN: f32 = 3.0;
    let total = a.distance(b);
    if total < 0.01 {
        return;
    }
    let dir = (b - a) / total;
    let mut t = 0.0;
    while t < total {
        let seg_end = (t + DASH_LEN).min(total);
        painter.line_segment([a + dir * t, a + dir * seg_end], stroke);
        t = seg_end + GAP_LEN;
    }
}

/// The gap `(near_edge, far_edge)` along one axis between two disjoint
/// intervals, or `None` if they overlap (nothing to measure on that axis).
pub(super) fn axis_gap(a_min: f32, a_max: f32, b_min: f32, b_max: f32) -> Option<(f32, f32)> {
    if a_max <= b_min {
        Some((a_max, b_min))
    } else if b_max <= a_min {
        Some((b_max, a_min))
    } else {
        None
    }
}

/// Draws the Option-hover measurement overlay between `sel` (the
/// current selection's combined bounds) and `hover` (the layer under the
/// cursor), in doc space — a dashed line plus a pixel-distance label for
/// whichever axes the two rects don't already overlap on.
pub(super) fn draw_distance_measurement(painter: &egui::Painter, to_screen: impl Fn(Pos2) -> Pos2, sel: Rect, hover: Rect) {
    let stroke = EguiStroke::new(1.0, MEASURE_COLOR);
    if let Some((x0, x1)) = axis_gap(sel.min.x, sel.max.x, hover.min.x, hover.max.x) {
        let overlap_y = sel.min.y.max(hover.min.y)..=sel.max.y.min(hover.max.y);
        let y = if overlap_y.start() <= overlap_y.end() {
            (overlap_y.start() + overlap_y.end()) / 2.0
        } else {
            (sel.center().y + hover.center().y) / 2.0
        };
        let a = to_screen(Pos2::new(x0, y));
        let b = to_screen(Pos2::new(x1, y));
        draw_dashed_line(painter, a, b, stroke);
        let mid = a.lerp(b, 0.5);
        painter.text(
            mid + Vec2::new(0.0, -8.0),
            egui::Align2::CENTER_BOTTOM,
            format!("{:.0}", x1 - x0),
            egui::FontId::proportional(11.0),
            MEASURE_COLOR,
        );
    }
    if let Some((y0, y1)) = axis_gap(sel.min.y, sel.max.y, hover.min.y, hover.max.y) {
        let overlap_x = sel.min.x.max(hover.min.x)..=sel.max.x.min(hover.max.x);
        let x = if overlap_x.start() <= overlap_x.end() {
            (overlap_x.start() + overlap_x.end()) / 2.0
        } else {
            (sel.center().x + hover.center().x) / 2.0
        };
        let a = to_screen(Pos2::new(x, y0));
        let b = to_screen(Pos2::new(x, y1));
        draw_dashed_line(painter, a, b, stroke);
        let mid = a.lerp(b, 0.5);
        painter.text(
            mid + Vec2::new(8.0, 0.0),
            egui::Align2::LEFT_CENTER,
            format!("{:.0}", y1 - y0),
            egui::FontId::proportional(11.0),
            MEASURE_COLOR,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn axis_gap_none_when_intervals_overlap() {
        assert_eq!(axis_gap(0.0, 10.0, 5.0, 15.0), None);
    }

    #[test]
    fn axis_gap_returns_the_facing_edges_when_disjoint() {
        assert_eq!(axis_gap(0.0, 10.0, 20.0, 30.0), Some((10.0, 20.0)));
        assert_eq!(axis_gap(20.0, 30.0, 0.0, 10.0), Some((10.0, 20.0)));
    }
}
