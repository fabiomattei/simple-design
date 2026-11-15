//! Applies the Layers panel's `LayerAction`s (see `ui/layers_panel.rs`) to
//! the document, history and selection — same "panel returns actions,
//! `App` applies them" convention as `apply_palette_action`.

use crate::arrange;
use crate::clipboard;
use crate::grouping;
use crate::ui::layers_panel::LayerAction;

use super::App;

impl App {
    /// Over the 40-line function limit, but only as one flat match arm per
    /// `LayerAction` variant — splitting it would just scatter the arms.
    pub(super) fn apply_layer_actions(&mut self, actions: Vec<LayerAction>) {
        if actions.is_empty() {
            return;
        }
        // Copy/Paste/PasteOver manage their own history snapshot
        // (Copy doesn't mutate the document at all; Paste/
        // PasteOver snapshot inside `paste_selection`) — folding
        // them into this blanket pre-snapshot would either
        // record a spurious no-op undo step (Copy) or double up
        // (Paste/PasteOver).
        let needs_snapshot = actions.iter().any(|a| {
            !matches!(
                a,
                LayerAction::Copy(_) | LayerAction::Paste | LayerAction::PasteOver | LayerAction::Close
            )
        });
        if needs_snapshot {
            self.history.snapshot();
        }
        for action in actions {
            match action {
                LayerAction::Close => self.show_layers_panel = false,
                LayerAction::Copy(id) => clipboard::copy_selection(&self.history, &[id]),
                LayerAction::Paste => self.paste_selection(false),
                LayerAction::PasteOver => self.paste_selection(true),
                LayerAction::ToggleVisible(id) => {
                    if let Some(l) = self.history.mutate().active_page_mut().find_mut(id) {
                        l.visible = !l.visible;
                    }
                }
                LayerAction::ToggleLocked(id) => {
                    if let Some(l) = self.history.mutate().active_page_mut().find_mut(id) {
                        l.locked = !l.locked;
                    }
                }
                LayerAction::Delete(id) => {
                    let page = self.history.mutate().active_page_mut();
                    let context = grouping::parent_and_index(&page.layers, id);
                    page.remove(id);
                    self.selection.retain(|&s| s != id);
                    if let Some((Some(parent_id), index)) = context {
                        let next_selected = page.find(parent_id).and_then(|parent| {
                            let children = parent.kind.children()?;
                            children.get(index).or(children.last()).map(|l| l.id)
                        });
                        self.selection = vec![next_selected.unwrap_or(parent_id)];
                        self.canvas.insert_hint_parent = Some(parent_id);
                    }
                }
                LayerAction::Duplicate(id) => {
                    let offset = self.canvas.duplicate_offset;
                    let new_ids = grouping::duplicate_layers(
                        self.history.mutate().active_page_mut(),
                        &[id],
                        offset,
                    );
                    if let Some(&new_id) = new_ids.first() {
                        self.selection = vec![new_id];
                    }
                }
                LayerAction::Rename(id, name) => {
                    if let Some(l) = self.history.mutate().active_page_mut().find_mut(id) {
                        l.name = name;
                    }
                }
                LayerAction::ToggleMask(id) => {
                    if let Some(l) = self.history.mutate().active_page_mut().find_mut(id) {
                        l.mask.enabled = !l.mask.enabled;
                    }
                }
                LayerAction::ToggleIgnoreMask(id) => {
                    if let Some(l) = self.history.mutate().active_page_mut().find_mut(id) {
                        l.mask.ignore_underlying = !l.mask.ignore_underlying;
                    }
                }
                LayerAction::SetMaskMode(id, mode) => {
                    if let Some(l) = self.history.mutate().active_page_mut().find_mut(id) {
                        l.mask.mode = mode;
                    }
                }
                LayerAction::ToggleMaskInverted(id) => {
                    if let Some(l) = self.history.mutate().active_page_mut().find_mut(id) {
                        l.mask.inverted = !l.mask.inverted;
                    }
                }
                LayerAction::Move { id, new_parent, index } => {
                    grouping::move_layer(self.history.mutate().active_page_mut(), id, new_parent, index);
                }
                LayerAction::Arrange(id, command) => {
                    arrange::arrange(self.history.mutate().active_page_mut(), &[id], command);
                }
                LayerAction::SetChildBoolOp(id, op) => {
                    if let Some(l) = self.history.mutate().active_page_mut().find_mut(id) {
                        l.bool_op = op;
                    }
                }
            }
        }
    }
}
