//! Sketch's "Frame" section for a single selected layer: the Align strip,
//! then the position/size/rotation/flip rows of `transform_section`.

use egui::Ui;

use crate::canvas::CanvasWidget;
use crate::grouping;
use crate::history::History;
use crate::model::Layer;

use super::align_strip::align_strip;
use super::geometry_section::transform_section;
use super::inspector_layout::{HeaderButton, section_header};
use super::InspectorAction;

/// Draws the Frame section for `layer`; returns the Align or Flip action if
/// one was clicked. Align is disabled for a top-level layer, which has no
/// parent container to align to (see `app.rs::align_selection`).
pub(super) fn frame_section(
    ui: &mut Ui,
    history: &mut History,
    canvas: &mut CanvasWidget,
    layer: &Layer,
) -> Option<InspectorAction> {
    section_header(ui, "Frame", HeaderButton::None);
    let has_parent = matches!(
        grouping::parent_and_index(&history.get().active_page().layers, layer.id),
        Some((Some(_), _))
    );
    let align_action = ui
        .horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 2.0;
            align_strip(ui, has_parent)
        })
        .inner;
    ui.add_space(4.0);
    let transform_action = transform_section(ui, history, canvas, layer);
    align_action.or(transform_action)
}
