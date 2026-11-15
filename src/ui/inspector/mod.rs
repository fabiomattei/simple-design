
use egui::Ui;

use crate::alignment::{AlignEdge, DistributeAxis};
use crate::canvas::CanvasWidget;
use crate::history::History;
use crate::model::{BoolOp, Layer, LayerId, LayerKind};
use crate::tools::Tool;
use crate::transform_ops::FlipAxis;
use crate::ui::icons;

mod align_strip;
mod appearance_section;
mod geometry_section;
mod geometry_widgets;
mod frame_section;
mod image_section;
mod inspector_layout;
mod layer_styles;
mod multi_frame_fields;
mod multi_selection;
mod paint_editor;
mod shadow_section;
mod text_font_picker;
mod text_path_ui;
mod text_section;
mod text_styles;
#[cfg(test)]
mod tests;

use geometry_widgets::artboard_preset_picker;
use appearance_section::appearance_section;
use frame_section::frame_section;
use geometry_section::shape_fields;
use image_section::image_section;
use multi_selection::{multi_selection_panel, scale_layers_panel};
use text_section::text_section;

pub enum InspectorAction {
    /// The header's "X" button — see `app.rs`'s `show_inspector_panel`.
    Close,
    Duplicate,
    Group,
    Ungroup,
    Boolean(BoolOp),
    /// `bool`: Option/Alt was held at click time — Sketch's "align to the
    /// enclosing Artboard instead of the immediate parent" override (see
    /// `alignment::artboard_bounds_in_parent_space`, wired from
    /// `app.rs::align_selection`).
    Align(AlignEdge, bool),
    Distribute(DistributeAxis),
    /// `f32`: spacing, from the inspector's Tidy row (see `alignment::tidy`).
    Tidy(f32),
    Flip(FlipAxis),
    Flatten,
    InsertArtboardPreset(egui::Vec2),
}

/// Snapshots history exactly once at the start of an edit gesture (a drag,
/// or a discrete click/keystroke), rather than every frame a value changes.
fn should_snapshot(resp: &egui::Response) -> bool {
    resp.changed() && (!resp.dragged() || resp.drag_started())
}

/// The effective value of a character-level property across `selection` —
/// `Some(base_value)` unconditionally when `runs` is empty (uniform style,
/// so nothing can be mixed), otherwise `text_runs::mixed_or`'s usual
/// "`None` if the selection spans differing values" query. Drives every
/// dual-mode Text control's displayed state (and, for a boolean toggle,
/// which direction the next click sets: mixed or unset → set all to
/// `true`; uniformly `true` → clear to `false`).
fn selected_or_base<T: PartialEq + Clone>(
    runs: &[crate::model::text_runs::TextRun],
    selection: &std::ops::Range<usize>,
    base_value: T,
    field: impl Fn(&crate::model::text_runs::RunStyle) -> T,
) -> Option<T> {
    if runs.is_empty() {
        Some(base_value)
    } else {
        crate::model::text_runs::mixed_or(runs, selection.clone(), field)
    }
}

/// Draws the Inspector panel for the current selection/tool and applies any edits directly to `history`.
pub fn ui(
    ui: &mut Ui,
    history: &mut History,
    selection: &[LayerId],
    canvas: &mut CanvasWidget,
    tool: Tool,
) -> Option<InspectorAction> {
    if crate::ui::theme::panel_header(ui, "Inspector", true) {
        return Some(InspectorAction::Close);
    }

    scale_layers_panel(ui, canvas);

    if selection.len() > 1 {
        return multi_selection_panel(ui, history, selection, canvas);
    }

    let Some(&id) = selection.first() else {
        if tool == Tool::Artboard {
            return artboard_preset_picker(ui);
        }
        ui.weak("No selection");
        return None;
    };
    let Some(layer) = history.get().find(id).cloned() else {
        ui.weak("No selection");
        return None;
    };
    // A non-empty text selection in the in-place editor, for `id`
    // specifically — drives every dual-mode Text control below (Bold/
    // Italic/Underline/Strikethrough, font, size, fill) between "format
    // just the selected range" and today's "format the whole layer".
    let text_selection: Option<std::ops::Range<usize>> =
        if canvas.is_editing_text(id) { canvas.text_edit_selection.clone().filter(|r| !r.is_empty()) } else { None };

    ui.label(egui::RichText::new(&layer.name).strong());
    ui.weak(layer.kind.type_name());
    ui.add_space(6.0);

    if let Some(action) = frame_section(ui, history, canvas, &layer) {
        return Some(action);
    }
    shape_fields(ui, history, canvas, &layer);
    text_section(ui, history, canvas, &layer, &text_selection);
    image_section(ui, history, canvas, &layer);
    appearance_section(ui, history, &layer, &text_selection);
    footer_actions(ui, &layer)
}

/// Duplicate, Ungroup (Groups only) and Flatten (flattenable kinds only).
fn footer_actions(ui: &mut Ui, layer: &Layer) -> Option<InspectorAction> {
    ui.add_space(8.0);
    ui.separator();
    if icons::duplicate_button(ui).on_hover_text("Duplicate (Cmd+D)").clicked() {
        return Some(InspectorAction::Duplicate);
    }

    if matches!(layer.kind, LayerKind::Group { .. })
        && ui.button("Ungroup (Cmd+Shift+G)").clicked() {
            return Some(InspectorAction::Ungroup);
        }

    // Only offered for kinds `transform_ops::flatten_to_compound_path`
    // actually supports (fillable shapes, plus an already-closed Path or
    // CompoundPath) — matches `boolean_ops.rs::flatten_layer`'s own
    // recognized-kinds set.
    let flattenable = matches!(
        layer.kind,
        LayerKind::Rectangle { .. }
            | LayerKind::Oval
            | LayerKind::Star { .. }
            | LayerKind::Polygon { .. }
            | LayerKind::CompoundPath { .. }
    ) || matches!(&layer.kind, LayerKind::Path { closed, points } if *closed && points.len() >= 3);
    if flattenable && icons::flatten_button(ui).on_hover_text("Flatten").clicked() {
        return Some(InspectorAction::Flatten);
    }

    None
}
