//! Geometry fields for a single selected layer: position, size, rotation,
//! flip, and each shape kind's own fields.

use std::cell::Cell;

use egui::Ui;

use crate::canvas::CanvasWidget;
use crate::grouping;
use crate::history::History;
use crate::model::{Layer, LayerId, LayerKind};
use crate::numeric_input::{self, Anchor};
use crate::ui::icons;

use super::geometry_widgets::{arrow_cap_combo, corner_radius_field, flip_buttons};
use super::{InspectorAction, should_snapshot};

/// Position, Size and Rotation fields plus the Flip buttons; returns the
/// Flip action if one was clicked.
pub(super) fn transform_section(
    ui: &mut Ui,
    history: &mut History,
    canvas: &mut CanvasWidget,
    layer: &Layer,
) -> Option<InspectorAction> {
    let id = layer.id;
    // `x_anchor`/`y_anchor`/`w_anchor`/`h_anchor` are the anchor-letter side
    // channel out of `DragValue::custom_parser` (which can only return the
    // parsed number itself, not the trailing `l`/`r`/`t`/`b`/`c`/`m` letter
    // — see `numeric_input::parse_dimension_expr`'s doc comment). Each is
    // set as a side effect of parsing, then read back right after the
    // widget to decide how the anchor affects `frame.pos`.
    let (extent_w, extent_h) = parent_extent(history, id, (layer.frame.size.x as f64, layer.frame.size.y as f64));
    position_fields(ui, history, layer, extent_w, extent_h);
    size_fields(ui, history, canvas, layer, extent_w, extent_h);
    rotation_field(ui, history, layer);

    ui.add_space(4.0);
    if let Some(action) = flip_buttons(ui) {
        return Some(action);
    }
    None
}

fn position_fields(ui: &mut Ui, history: &mut History, layer: &Layer, extent_w: f64, extent_h: f64) {
    let id = layer.id;
    ui.label("Position");
    ui.horizontal(|ui| {
        let mut x = layer.frame.pos.x;
        let mut y = layer.frame.pos.y;
        let (cur_x, cur_y) = (x as f64, y as f64);
        let x_anchor: Cell<Option<Anchor>> = Cell::new(None);
        let y_anchor: Cell<Option<Anchor>> = Cell::new(None);
        let rx = ui.add(egui::DragValue::new(&mut x).prefix("X: ").custom_parser(|s| {
            let (val, anchor) = numeric_input::parse_dimension_expr(cur_x, s, extent_w)?;
            x_anchor.set(anchor);
            Some(val)
        }));
        let ry = ui.add(egui::DragValue::new(&mut y).prefix("Y: ").custom_parser(|s| {
            let (val, anchor) = numeric_input::parse_dimension_expr(cur_y, s, extent_h)?;
            y_anchor.set(anchor);
            Some(val)
        }));
        if should_snapshot(&rx) || should_snapshot(&ry) {
            history.snapshot();
        }
        if (rx.changed() || ry.changed())
            && let Some(l) = history.mutate().active_page_mut().find_mut(id) {
                let (w, h) = (l.frame.size.x, l.frame.size.y);
                // A typed X/Y value positions whichever edge/center the
                // anchor letter names, not always the top-left corner — see
                // `numeric_input`'s doc comment on the Position/Size syntax.
                l.frame.pos.x = match x_anchor.get() {
                    Some(Anchor::Right) => x - w,
                    Some(Anchor::Center) => x - w / 2.0,
                    _ => x,
                };
                l.frame.pos.y = match y_anchor.get() {
                    Some(Anchor::Bottom) => y - h,
                    Some(Anchor::Center) => y - h / 2.0,
                    _ => y,
                };
            }
    });
}

fn size_fields(
    ui: &mut Ui,
    history: &mut History,
    canvas: &mut CanvasWidget,
    layer: &Layer,
    extent_w: f64,
    extent_h: f64,
) {
    let id = layer.id;
    ui.label("Size");
    ui.horizontal(|ui| {
        let mut w = layer.frame.size.x;
        let mut h = layer.frame.size.y;
        let (cur_w, cur_h) = (w as f64, h as f64);
        let w_anchor: Cell<Option<Anchor>> = Cell::new(None);
        let h_anchor: Cell<Option<Anchor>> = Cell::new(None);
        let rw = ui.add(egui::DragValue::new(&mut w).prefix("W: ").custom_parser(|s| {
            let (val, anchor) = numeric_input::parse_dimension_expr(cur_w, s, extent_w)?;
            w_anchor.set(anchor);
            Some(val)
        }));
        let rh = ui.add(egui::DragValue::new(&mut h).prefix("H: ").custom_parser(|s| {
            let (val, anchor) = numeric_input::parse_dimension_expr(cur_h, s, extent_h)?;
            h_anchor.set(anchor);
            Some(val)
        }));
        let lock_icon = if canvas.aspect_locked { "🔒" } else { "🔓" };
        if ui
            .button(lock_icon)
            .on_hover_text("Lock proportions (⌥⌘L)")
            .clicked()
        {
            canvas.aspect_locked = !canvas.aspect_locked;
        }
        if should_snapshot(&rw) || should_snapshot(&rh) {
            history.snapshot();
        }
        if (rw.changed() || rh.changed())
            && let Some(l) = history.mutate().active_page_mut().find_mut(id) {
                let (old_w, old_h) = (l.frame.size.x, l.frame.size.y);
                let mut new_w = w;
                let mut new_h = h;
                // Proportional lock cascades whichever field the user
                // *didn't* type into, based on the other's ratio change.
                if canvas.aspect_locked {
                    if rw.changed() && old_w != 0.0 {
                        new_h = old_h * (w / old_w);
                    } else if rh.changed() && old_h != 0.0 {
                        new_w = old_w * (h / old_h);
                    }
                }
                if let Some(a) = w_anchor.get() {
                    match a {
                        Anchor::Right => l.frame.pos.x += old_w - new_w,
                        Anchor::Center => l.frame.pos.x += (old_w - new_w) / 2.0,
                        _ => {}
                    }
                }
                if let Some(a) = h_anchor.get() {
                    match a {
                        Anchor::Bottom => l.frame.pos.y += old_h - new_h,
                        Anchor::Center => l.frame.pos.y += (old_h - new_h) / 2.0,
                        _ => {}
                    }
                }
                l.frame.size.x = new_w;
                l.frame.size.y = new_h;
            }
    });
}

fn rotation_field(ui: &mut Ui, history: &mut History, layer: &Layer) {
    let id = layer.id;
    // Rotation is meaningless for an Artboard (a page-frame boundary, not a
    // rotatable shape — see `model/layer.rs`'s `Frame::rotation` doc
    // comment) and, for a multi-selection, ambiguous as a single absolute
    // value (only well-defined as a *delta*, which the interactive
    // drag-to-rotate handle provides) — so this field only ever shows for a
    // single non-Artboard layer.
    if !matches!(layer.kind, LayerKind::Artboard { .. }) {
        ui.label("Rotation");
        ui.horizontal(|ui| {
            let mut rotation = layer.frame.rotation;
            let r = ui.add(egui::DragValue::new(&mut rotation).prefix("° ").speed(1.0));
            if should_snapshot(&r) {
                history.snapshot();
            }
            if r.changed()
                && let Some(l) = history.mutate().active_page_mut().find_mut(id) {
                    l.frame.rotation = rotation;
                }
        });
    }
}

/// The fields specific to one shape kind: a Path's Closed toggle and point
/// corner radius, a Rectangle's corner radii, a Star's points, a Polygon's
/// sides, an Arrow's caps. Draws nothing for other kinds.
pub(super) fn shape_fields(ui: &mut Ui, history: &mut History, canvas: &mut CanvasWidget, layer: &Layer) {
    path_fields(ui, history, canvas, layer);
    rectangle_corner_fields(ui, history, layer);
    star_fields(ui, history, layer);
    polygon_fields(ui, history, layer);
    arrow_cap_fields(ui, history, layer);
}

fn path_fields(ui: &mut Ui, history: &mut History, canvas: &mut CanvasWidget, layer: &Layer) {
    let id = layer.id;
    if let LayerKind::Path { closed, .. } = &layer.kind {
        let mut is_closed = *closed;
        let r = ui.checkbox(&mut is_closed, "Closed");
        if r.changed() {
            history.snapshot();
            if let Some(l) = history.mutate().active_page_mut().find_mut(id)
                && let LayerKind::Path { closed, .. } = &mut l.kind {
                    *closed = is_closed;
                }
        }

        // Per-point corner radius/smoothing — only meaningful once at least
        // one anchor is selected (`canvas.has_point_selection()`; point
        // selection is keyboard/click-driven in the canvas itself, same as
        // the Num1-4 point-type shortcuts, so this is the first inspector UI
        // for individual path points rather than the whole layer).
        if canvas.has_point_selection()
            && let Some(radius) = canvas.selected_point_corner_radius(history, id) {
                ui.horizontal(|ui| {
                    let mut r = radius;
                    let resp = ui.add(
                        egui::DragValue::new(&mut r)
                            .prefix("Corner radius: ")
                            .range(0.0..=100000.0),
                    );
                    if should_snapshot(&resp) {
                        history.snapshot();
                    }
                    if resp.changed() {
                        canvas.apply_corner_radius(history, id, r);
                    }
                    if ui.button("Max").on_hover_text("Snap to the largest radius this corner allows").clicked() {
                        canvas.apply_max_corner_radius(history, id);
                    }
                });
            }
    }
}

fn rectangle_corner_fields(ui: &mut Ui, history: &mut History, layer: &Layer) {
    let id = layer.id;
    if let LayerKind::Rectangle { corner_radius } = &layer.kind {
        let radii = *corner_radius;
        ui.label("Corner radius");
        egui::Grid::new(("corner_radius_grid", id)).num_columns(2).spacing([8.0, 4.0]).show(ui, |ui| {
            corner_radius_field(ui, history, id, icons::RectCorner::TopLeft, radii.top_left, |c, v| c.top_left = v);
            corner_radius_field(ui, history, id, icons::RectCorner::TopRight, radii.top_right, |c, v| c.top_right = v);
            ui.end_row();
            corner_radius_field(ui, history, id, icons::RectCorner::BottomLeft, radii.bottom_left, |c, v| c.bottom_left = v);
            corner_radius_field(ui, history, id, icons::RectCorner::BottomRight, radii.bottom_right, |c, v| c.bottom_right = v);
            ui.end_row();
        });
    }
}

fn star_fields(ui: &mut Ui, history: &mut History, layer: &Layer) {
    let id = layer.id;
    if let LayerKind::Star { points, inner_ratio } = &layer.kind {
        let mut point_count = *points;
        let mut ratio = *inner_ratio;
        ui.horizontal(|ui| {
            let pr = ui.add(egui::DragValue::new(&mut point_count).prefix("Points: ").range(3..=30));
            let rr = ui.add(
                egui::DragValue::new(&mut ratio)
                    .prefix("Inner: ")
                    .range(0.0..=1.0)
                    .speed(0.01),
            );
            if should_snapshot(&pr) || should_snapshot(&rr) {
                history.snapshot();
            }
            if (pr.changed() || rr.changed())
                && let Some(l) = history.mutate().active_page_mut().find_mut(id)
                    && let LayerKind::Star { points, inner_ratio } = &mut l.kind {
                        *points = point_count;
                        *inner_ratio = ratio;
                    }
        });
    }
}

fn polygon_fields(ui: &mut Ui, history: &mut History, layer: &Layer) {
    let id = layer.id;
    if let LayerKind::Polygon { sides } = &layer.kind {
        let mut side_count = *sides;
        let r = ui.add(egui::DragValue::new(&mut side_count).prefix("Sides: ").range(3..=30));
        if should_snapshot(&r) {
            history.snapshot();
        }
        if r.changed()
            && let Some(l) = history.mutate().active_page_mut().find_mut(id)
                && let LayerKind::Polygon { sides } = &mut l.kind {
                    *sides = side_count;
                }
    }
}

fn arrow_cap_fields(ui: &mut Ui, history: &mut History, layer: &Layer) {
    let id = layer.id;
    if let LayerKind::Arrow { start_cap, end_cap } = &layer.kind {
        let (mut start, mut end) = (*start_cap, *end_cap);
        ui.horizontal(|ui| {
            ui.label("Start:");
            let changed_start = arrow_cap_combo(ui, ("arrow-start-cap", id), &mut start);
            ui.label("End:");
            let changed_end = arrow_cap_combo(ui, ("arrow-end-cap", id), &mut end);
            if changed_start || changed_end {
                history.snapshot();
                if let Some(l) = history.mutate().active_page_mut().find_mut(id)
                    && let LayerKind::Arrow { start_cap, end_cap } = &mut l.kind {
                        *start_cap = start;
                        *end_cap = end;
                    }
            }
        });
    }
}

/// The nearest parent container's `(width, height)`, in doc units — the
/// 100% reference for percentage dimension input (see
/// `numeric_input::parse_dimension_expr`). Falls back to `fallback` (the
/// field's own current value) when `id` has no parent container, so typing
/// "100%" on a top-level layer degenerates to "unchanged" rather than being
/// undefined.
fn parent_extent(history: &History, id: LayerId, fallback: (f64, f64)) -> (f64, f64) {
    let page = history.get().active_page();
    let Some((Some(parent_id), _)) = grouping::parent_and_index(&page.layers, id) else {
        return fallback;
    };
    match page.find(parent_id) {
        Some(parent) => {
            let b = parent.frame.bounds();
            (b.width() as f64, b.height() as f64)
        }
        None => fallback,
    }
}
