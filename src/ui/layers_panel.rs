use egui::Ui;

use crate::model::{BoolOp, Layer, LayerId, LayerKind, Page};
use crate::ui::icons;

pub enum LayerAction {
    /// The header's "X" button — see `app.rs`'s `show_layers_panel`.
    Close,
    ToggleVisible(LayerId),
    ToggleLocked(LayerId),
    Delete(LayerId),
    Duplicate(LayerId),
    /// Right-click "Copy"/"Paste"/"Paste Over" (see `app.rs`'s
    /// `copy_selection`/`paste_selection`, which these just call into —
    /// `Copy` copies only this one row's layer, independent of the broader
    /// `selection`, matching `Duplicate`'s existing per-row precedent above.
    Copy(LayerId),
    Paste,
    PasteOver,
    Rename(LayerId, String),
    /// Sketch's "Use as Mask" — see `Layer::is_mask`.
    ToggleMask(LayerId),
    /// Sketch's "Ignore Underlying Mask" — see `Layer::ignore_mask`.
    ToggleIgnoreMask(LayerId),
    /// Drag-and-drop reorder/reparent — see `grouping::move_layer` for the
    /// exact semantics of `new_parent`/`index`, which this mirrors exactly
    /// (the panel only computes *where* to drop; the actual tree surgery
    /// happens in `app.rs`'s action dispatch, same as every other action
    /// here).
    Move { id: LayerId, new_parent: Option<LayerId>, index: usize },
    /// Reassigns a `BooleanGroup` child's combine mode — see `Layer::bool_op`.
    SetChildBoolOp(LayerId, BoolOp),
}

#[derive(Default)]
pub struct LayersPanel {
    renaming: Option<(LayerId, String)>,
    /// Set alongside `renaming` when it's first entered; consumed (and
    /// cleared) the next time the rename field is drawn, so focus is
    /// requested exactly once. Requesting it every frame would re-grab focus
    /// right after `TextEdit`'s own Enter-triggered `surrender_focus`, which
    /// makes `lost_focus()` (checked immediately after, in the same frame)
    /// read as still-focused — silently swallowing both Enter and click-away.
    renaming_needs_focus: bool,
    /// The last plain- or cmd/ctrl-clicked row, i.e. the fixed end a
    /// shift-click range-select extends from (Finder/Sketch convention).
    /// Left untouched by shift-click itself, so repeated shift-clicks keep
    /// extending/shrinking the same range rather than re-anchoring each time.
    range_anchor: Option<LayerId>,
}

impl LayersPanel {
    /// Programmatic entry into the inline rename state (Sketch's Cmd+R on a
    /// single-layer selection — see `app.rs::handle_shortcuts`), same
    /// underlying state as double-clicking/right-clicking a row's name.
    pub fn start_renaming(&mut self, id: LayerId, current_name: String) {
        self.renaming = Some((id, current_name));
        self.renaming_needs_focus = true;
    }

    pub fn ui(
        &mut self,
        ui: &mut Ui,
        page: &Page,
        selection: &mut Vec<LayerId>,
    ) -> Vec<LayerAction> {
        let mut actions = Vec::new();
        ui.horizontal(|ui| {
            ui.heading("Layers");
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if icons::close_button(ui).clicked() {
                    actions.push(LayerAction::Close);
                }
            });
        });
        ui.separator();
        let mut order = Vec::new();
        visual_order(&page.layers, &mut order);
        egui::ScrollArea::vertical().show(ui, |ui| {
            self.show_children(ui, &page.layers, None, 0, false, selection, &order, &mut actions);
        });
        actions
    }

    /// Renders one parent's `children` (the page's top-level `layers`, or
    /// some artboard/group's own children) in front-to-back panel order,
    /// interleaved with thin drop-zone "gaps" between (and before/after) the
    /// rows — every gap doubles as a reparent target, since a group/artboard's
    /// own children are rendered by a recursive call to this same function,
    /// so dropping into one of *its* gaps is how a layer moves inside it.
    /// `parent` is this list's own owning layer id (`None` at the page's top
    /// level) — what every gap in this call reports back as `new_parent`.
    /// `in_boolean_group` is whether `children` themselves are a
    /// `BooleanGroup`'s children (i.e. `parent` is one) — drives whether each
    /// row gets the "combine mode" control (see `show_layer`).
    #[allow(clippy::too_many_arguments)]
    fn show_children(
        &mut self,
        ui: &mut Ui,
        children: &[Layer],
        parent: Option<LayerId>,
        depth: usize,
        in_boolean_group: bool,
        selection: &mut Vec<LayerId>,
        order: &[LayerId],
        actions: &mut Vec<LayerAction>,
    ) {
        let len = children.len();
        // `children` is back-to-front (index 0 = furthest back); the panel
        // shows it front-to-back, so the first gap drawn (visually above
        // every row) targets the *last* array index (the new front), and
        // the gap drawn right after the row for array index `i` targets
        // index `i` itself (displacing that row backward by one).
        drop_gap(ui, parent, len, depth, actions);
        for (index, layer) in children.iter().enumerate().rev() {
            self.show_layer(ui, layer, depth, in_boolean_group, index == 0 && in_boolean_group, selection, order, actions);
            drop_gap(ui, parent, index, depth, actions);
        }
    }

    /// `in_boolean_group`/`is_boolean_base` are only meaningful together: a
    /// row is only ever `is_boolean_base` when it's also `in_boolean_group`
    /// (see `show_children`, the only caller).
    #[allow(clippy::too_many_arguments)]
    fn show_layer(
        &mut self,
        ui: &mut Ui,
        layer: &Layer,
        depth: usize,
        in_boolean_group: bool,
        is_boolean_base: bool,
        selection: &mut Vec<LayerId>,
        order: &[LayerId],
        actions: &mut Vec<LayerAction>,
    ) {
        let is_selected = selection.contains(&layer.id);
        let drag_id = egui::Id::new(("layer-row", layer.id));
        ui.horizontal(|ui| {
            ui.add_space(depth as f32 * 14.0);
            // Only this handle senses drag — the rest of the row (label,
            // buttons, combo box) stays plain-click-only. Wrapping the whole
            // row in `dnd_drag_source` used to make its `Sense::drag()` hit
            // region compete with every click-sensing child for the pointer,
            // so ordinary clicks (select, toggle, rename) would regularly get
            // swallowed into starting a reorder-drag instead.
            ui.dnd_drag_source(drag_id, layer.id, |ui| {
                ui.label(egui::RichText::new("::").weak())
                    .on_hover_text("Drag to reorder")
                    .on_hover_cursor(egui::CursorIcon::Grab);
            });
            if crate::ui::icons::eye_button(ui, layer.visible)
                .on_hover_text("Toggle visibility")
                .clicked()
            {
                actions.push(LayerAction::ToggleVisible(layer.id));
            }
            if crate::ui::icons::lock_button(ui, layer.locked)
                .on_hover_text("Toggle lock")
                .clicked()
            {
                actions.push(LayerAction::ToggleLocked(layer.id));
            }
            ui.label(egui::RichText::new(icon_for(&layer.kind)).weak());
            if layer.is_mask {
                ui.label(egui::RichText::new("[Mask]").weak().color(egui::Color32::from_rgb(100, 140, 220)))
                    .on_hover_text("Masks the layers below it in this group");
            }
            if layer.ignore_mask {
                ui.label(egui::RichText::new("[Ignores Mask]").weak())
                    .on_hover_text("Not clipped by any mask above it");
            }
            if in_boolean_group {
                if is_boolean_base {
                    ui.label(egui::RichText::new("[Base]").weak())
                        .on_hover_text("The base shape the other members combine onto");
                } else {
                    egui::ComboBox::from_id_salt(("bool-op-combo", layer.id))
                        .selected_text(layer.bool_op.label())
                        .show_ui(ui, |ui| {
                            for op in [BoolOp::Union, BoolOp::Subtract, BoolOp::Intersect, BoolOp::Difference, BoolOp::Add] {
                                if ui.selectable_label(layer.bool_op == op, op.label()).clicked() {
                                    actions.push(LayerAction::SetChildBoolOp(layer.id, op));
                                }
                            }
                        });
                }
            }

            let renaming_this = matches!(&self.renaming, Some((id, _)) if *id == layer.id);
            if renaming_this {
                let (_, buf) = self.renaming.as_mut().unwrap();
                let resp = ui.text_edit_singleline(buf);
                if self.renaming_needs_focus {
                    resp.request_focus();
                    self.renaming_needs_focus = false;
                }
                if resp.lost_focus() {
                    let (id, buf) = self.renaming.take().unwrap();
                    actions.push(LayerAction::Rename(id, buf));
                }
            } else {
                let label_resp = ui.selectable_label(is_selected, &layer.name);
                if label_resp.clicked() {
                    let (shift, cmd) = ui.input(|i| (i.modifiers.shift, i.modifiers.command));
                    if shift {
                        let range = self
                            .range_anchor
                            .and_then(|anchor| order.iter().position(|&id| id == anchor))
                            .zip(order.iter().position(|&id| id == layer.id));
                        if let Some((anchor_idx, click_idx)) = range {
                            let (lo, hi) = if anchor_idx <= click_idx { (anchor_idx, click_idx) } else { (click_idx, anchor_idx) };
                            *selection = order[lo..=hi].to_vec();
                        } else {
                            *selection = vec![layer.id];
                            self.range_anchor = Some(layer.id);
                        }
                    } else if cmd {
                        if is_selected {
                            selection.retain(|&s| s != layer.id);
                        } else {
                            selection.push(layer.id);
                        }
                        self.range_anchor = Some(layer.id);
                    } else {
                        *selection = vec![layer.id];
                        self.range_anchor = Some(layer.id);
                    }
                }
                if label_resp.double_clicked() {
                    self.renaming = Some((layer.id, layer.name.clone()));
                    self.renaming_needs_focus = true;
                }
                label_resp.context_menu(|ui| {
                    if ui.button("Rename").clicked() {
                        self.renaming = Some((layer.id, layer.name.clone()));
                        self.renaming_needs_focus = true;
                        ui.close();
                    }
                    if ui.button("Duplicate").clicked() {
                        actions.push(LayerAction::Duplicate(layer.id));
                        ui.close();
                    }
                    if ui.button("Delete").clicked() {
                        actions.push(LayerAction::Delete(layer.id));
                        ui.close();
                    }
                    ui.separator();
                    if ui.button("Copy").clicked() {
                        actions.push(LayerAction::Copy(layer.id));
                        ui.close();
                    }
                    if ui.button("Paste").clicked() {
                        actions.push(LayerAction::Paste);
                        ui.close();
                    }
                    if ui.button("Paste Over").clicked() {
                        actions.push(LayerAction::PasteOver);
                        ui.close();
                    }
                    ui.separator();
                    let mut is_mask = layer.is_mask;
                    if ui.checkbox(&mut is_mask, "Use as Mask").clicked() {
                        actions.push(LayerAction::ToggleMask(layer.id));
                        ui.close();
                    }
                    let mut ignore_mask = layer.ignore_mask;
                    if ui.checkbox(&mut ignore_mask, "Ignore Underlying Mask").clicked() {
                        actions.push(LayerAction::ToggleIgnoreMask(layer.id));
                        ui.close();
                    }
                    ui.separator();
                    if ui.button("Export as PNG...").clicked() {
                        if let Some(path) = crate::io::export_png_dialog(&format!("{}.png", layer.name)) {
                            let _ = crate::export::export_layer_to_png(layer, &path);
                        }
                        ui.close();
                    }
                });
            }
        });
        if let Some(children) = layer.kind.children() {
            let child_in_boolean_group = matches!(layer.kind, LayerKind::BooleanGroup { .. });
            self.show_children(ui, children, Some(layer.id), depth + 1, child_in_boolean_group, selection, order, actions);
        }
    }
}

/// A thin drop target between two rows (or before the first / after the
/// last row) of one parent's children list, at array position `index`. Only
/// visibly highlights while something is actually being dragged, via
/// `dnd_drop_zone`'s own active/inactive widget styling.
fn drop_gap(ui: &mut Ui, parent: Option<LayerId>, index: usize, depth: usize, actions: &mut Vec<LayerAction>) {
    ui.push_id(("layer-drop-gap", parent, index), |ui| {
        ui.horizontal(|ui| {
            ui.add_space(depth as f32 * 14.0 + 4.0);
            let (_, dropped) = ui.dnd_drop_zone::<LayerId, _>(egui::Frame::default(), |ui| {
                ui.set_min_size(egui::vec2(ui.available_width().max(20.0), 5.0));
            });
            if let Some(dragged_id) = dropped {
                actions.push(LayerAction::Move { id: *dragged_id, new_parent: parent, index });
            }
        });
    });
}

/// Flattens `children` into the same front-to-back, depth-first order the
/// panel renders rows in (`show_children`/`show_layer`'s own recursion), so
/// shift-click range-select operates on visual row order rather than the
/// underlying back-to-front array order.
fn visual_order(children: &[Layer], out: &mut Vec<LayerId>) {
    for layer in children.iter().rev() {
        out.push(layer.id);
        if let Some(kids) = layer.kind.children() {
            visual_order(kids, out);
        }
    }
}

fn icon_for(kind: &LayerKind) -> &'static str {
    match kind {
        LayerKind::Artboard { .. } => "[Artboard]",
        LayerKind::Group { .. } => "[Group]",
        LayerKind::Rectangle { .. } => "[Rect]",
        LayerKind::Oval => "[Oval]",
        LayerKind::Line => "[Line]",
        LayerKind::Star { .. } => "[Star]",
        LayerKind::Polygon { .. } => "[Polygon]",
        LayerKind::Arrow { .. } => "[Arrow]",
        LayerKind::Path { .. } => "[Path]",
        LayerKind::CompoundPath { .. } => "[Compound]",
        LayerKind::BooleanGroup { .. } => "[Boolean]",
        LayerKind::Text { .. } => "[Text]",
        LayerKind::Image { .. } => "[Image]",
    }
}
