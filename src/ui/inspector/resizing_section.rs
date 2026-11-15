//! The "Resizing" section: Sketch's pin diagram for a layer's resizing
//! constraints (only for a layer inside a Group/Artboard — a top-level
//! layer has no parent to follow) and an Artboard's "Adjust content on
//! resize" toggle. See `resizing_constraints` for what each setting does.

use egui::{Pos2, Rect, Sense, Stroke, StrokeKind, Ui, Vec2};

use crate::grouping;
use crate::history::History;
use crate::model::{Layer, LayerKind, ResizingConstraints};

use super::inspector_layout::{HeaderButton, section_header};

const DIAGRAM_SIZE: f32 = 72.0;
/// Half-thickness of each toggle's click area across its line.
const HIT_SLOP: f32 = 4.0;

/// One clickable line in the diagram and the constraint flag it toggles.
type Toggle = (Pos2, Pos2, &'static str, fn(&mut ResizingConstraints) -> &mut bool);

/// Draws the section for `layer`, or nothing if it is a top-level
/// non-Artboard layer. Applies edits directly to `history`.
pub(super) fn resizing_section(ui: &mut Ui, history: &mut History, layer: &Layer) {
    let has_parent = matches!(
        grouping::parent_and_index(&history.get().active_page().layers, layer.id),
        Some((Some(_), _))
    );
    let adjust_content = match layer.kind {
        LayerKind::Artboard { adjust_content_on_resize, .. } => Some(adjust_content_on_resize),
        _ => None,
    };
    if !has_parent && adjust_content.is_none() {
        return;
    }
    section_header(ui, "Resizing", HeaderButton::None);
    if has_parent {
        constraints_diagram(ui, history, layer);
    }
    if let Some(mut adjust) = adjust_content
        && ui
            .checkbox(&mut adjust, "Adjust content on resize")
            .on_hover_text("Resize this artboard's layers along with it, following their constraints")
            .changed()
    {
        history.snapshot();
        if let Some(l) = history.mutate().active_page_mut().find_mut(layer.id)
            && let LayerKind::Artboard { adjust_content_on_resize, .. } = &mut l.kind
        {
            *adjust_content_on_resize = adjust;
        }
    }
}

/// The pin diagram: an outer square (the parent) around an inner one (the
/// layer). The four lines between them pin an edge; the two lines across
/// the inner square fix the width/height. A set flag is drawn highlighted.
fn constraints_diagram(ui: &mut Ui, history: &mut History, layer: &Layer) {
    let (rect, _) = ui.allocate_exact_size(Vec2::splat(DIAGRAM_SIZE), Sense::hover());
    let inner = Rect::from_center_size(rect.center(), Vec2::splat(DIAGRAM_SIZE * 0.4));
    let center = rect.center();
    let toggles: [Toggle; 6] = [
        (Pos2::new(rect.left(), center.y), Pos2::new(inner.left(), center.y), "Pin to left edge", |c| &mut c.pin_left),
        (Pos2::new(inner.right(), center.y), Pos2::new(rect.right(), center.y), "Pin to right edge", |c| &mut c.pin_right),
        (Pos2::new(center.x, rect.top()), Pos2::new(center.x, inner.top()), "Pin to top edge", |c| &mut c.pin_top),
        (Pos2::new(center.x, inner.bottom()), Pos2::new(center.x, rect.bottom()), "Pin to bottom edge", |c| &mut c.pin_bottom),
        (Pos2::new(inner.left() + HIT_SLOP, center.y), Pos2::new(inner.right() - HIT_SLOP, center.y), "Fix width", |c| &mut c.fixed_width),
        (Pos2::new(center.x, inner.top() + HIT_SLOP), Pos2::new(center.x, inner.bottom() - HIT_SLOP), "Fix height", |c| &mut c.fixed_height),
    ];

    let weak = ui.visuals().weak_text_color();
    ui.painter().rect_stroke(rect.shrink(1.0), 2.0, Stroke::new(1.0, weak), StrokeKind::Inside);
    ui.painter().rect_stroke(inner, 0.0, Stroke::new(1.0, weak), StrokeKind::Middle);

    let mut constraints = layer.resizing;
    let mut changed = false;
    for (index, (start, end, hint, flag)) in toggles.into_iter().enumerate() {
        // Thicken the click area across the line only, never along it, so
        // neighboring toggles (e.g. "Pin left" and "Fix width") don't overlap.
        let slop = if start.y == end.y { Vec2::new(0.0, HIT_SLOP) } else { Vec2::new(HIT_SLOP, 0.0) };
        let hit = Rect::from_two_pos(start, end).expand2(slop);
        let response = ui.interact(hit, ui.id().with(("resizing-toggle", layer.id, index)), Sense::click()).on_hover_text(hint);
        if response.clicked() {
            let value = flag(&mut constraints);
            *value = !*value;
            changed = true;
        }
        let stroke = if *flag(&mut constraints) {
            Stroke::new(2.0, ui.visuals().selection.bg_fill)
        } else if response.hovered() {
            Stroke::new(1.0, ui.visuals().text_color())
        } else {
            Stroke::new(1.0, weak)
        };
        ui.painter().line_segment([start, end], stroke);
    }

    if changed {
        history.snapshot();
        if let Some(l) = history.mutate().active_page_mut().find_mut(layer.id) {
            l.resizing = constraints;
        }
    }
}
