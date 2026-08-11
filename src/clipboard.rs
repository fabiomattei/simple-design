use egui::Vec2;

use crate::history::History;
use crate::model::{Layer, LayerId};

/// Prefixed onto the JSON payload so `paste_layers` can tell "this is our own
/// copied-layers data" apart from arbitrary text sitting in the OS clipboard
/// (which should make Cmd+V a no-op, not a parse error/panic).
const SENTINEL: &str = "SDESIGN_LAYERS_V1\n";

/// Copies `layers` to the OS clipboard (Cmd+C), so paste also works across
/// separate instances of this app — not just within one running session.
/// Each layer's `frame.pos` is copied exactly as stored (parent-relative, see
/// `model/layer.rs`'s coordinate-system convention); `paste_layers`'s callers
/// decide how to reposition the result.
pub fn copy_layers(layers: &[Layer]) {
    let Ok(json) = serde_json::to_string(layers) else { return };
    let Ok(mut clipboard) = arboard::Clipboard::new() else { return };
    let _ = clipboard.set_text(format!("{SENTINEL}{json}"));
}

/// Reads layers previously placed on the clipboard by `copy_layers`. Returns
/// `None` if the clipboard is unavailable, empty, holds plain OS text (no
/// `SENTINEL`), or fails to parse — any of which should leave Cmd+V a no-op
/// rather than erroring. Every returned layer (and, recursively, its
/// descendants) gets a fresh id via `regenerate_ids`, so pasting doesn't
/// collide with the copied original if it's still on the page.
pub fn paste_layers() -> Option<Vec<Layer>> {
    let mut clipboard = arboard::Clipboard::new().ok()?;
    let text = clipboard.get_text().ok()?;
    let json = text.strip_prefix(SENTINEL)?;
    let mut layers: Vec<Layer> = serde_json::from_str(json).ok()?;
    for layer in &mut layers {
        layer.regenerate_ids();
    }
    Some(layers)
}

/// Cmd+C: copies every layer in `selection` (skipping any id that's since
/// vanished) — shared by `app.rs`'s keyboard shortcut and `canvas.rs`'s
/// right-click menu so both stay in sync with exactly one implementation.
pub fn copy_selection(history: &History, selection: &[LayerId]) {
    let page = history.get().active_page();
    let layers: Vec<Layer> = selection.iter().filter_map(|&id| page.find(id).cloned()).collect();
    if !layers.is_empty() {
        copy_layers(&layers);
    }
}

/// Cmd+V ("Paste", `over: false`) / Shift+Cmd+V ("Paste Over", `over: true`).
/// See `App::paste_selection`'s doc comment (the sole other caller, from
/// which this was extracted) for the exact positioning/targeting rules.
pub fn paste_selection(history: &mut History, selection: &mut Vec<LayerId>, duplicate_offset: Vec2, over: bool) {
    let Some(mut layers) = paste_layers() else { return };
    if layers.is_empty() {
        return;
    }
    if over {
        let sel_bounds = selection
            .iter()
            .filter_map(|&id| {
                let page = history.get().active_page();
                let layer = page.find(id)?;
                let offset = page.absolute_offset(id)?;
                Some(layer.frame.rotated_bounds().translate(offset))
            })
            .reduce(|a, b| a.union(b));
        if let Some(sel_bounds) = sel_bounds {
            if let Some(paste_bounds) = layers.iter().map(|l| l.frame.bounds()).reduce(|a, b| a.union(b)) {
                let delta = sel_bounds.min - paste_bounds.min;
                for l in &mut layers {
                    l.frame.pos += delta;
                }
            }
        }
    } else {
        for l in &mut layers {
            l.frame.pos += duplicate_offset;
        }
    }

    history.snapshot();
    let new_ids: Vec<LayerId> = layers.iter().map(|l| l.id).collect();
    let target_parent = match selection[..] {
        [id] => history
            .get()
            .active_page()
            .find(id)
            .filter(|l| l.kind.children().is_some())
            .map(|_| id),
        _ => None,
    };
    let page = history.mutate().active_page_mut();
    match target_parent.and_then(|id| page.find_mut(id)).and_then(|l| l.kind.children_mut()) {
        Some(children) => children.extend(layers),
        None => page.layers.extend(layers),
    }
    *selection = new_ids;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{CornerRadii, Frame, LayerKind};
    use egui::Pos2;

    fn rect(name: &str) -> Layer {
        Layer::new(
            name,
            Frame { pos: Pos2::new(1.0, 2.0), size: Vec2::new(10.0, 10.0), rotation: 0.0 },
            LayerKind::Rectangle { corner_radius: CornerRadii::ZERO },
        )
    }

    /// Exercises the real OS clipboard — skipped (rather than failed) in
    /// environments without one (headless CI, sandboxed test runners). Both
    /// cases live in one test (rather than two `#[test]`s) since they'd
    /// otherwise race on the same process-wide OS clipboard under cargo
    /// test's default parallel execution.
    #[test]
    fn copy_and_paste_round_trip_then_ignore_plain_os_text() {
        let Ok(mut probe) = arboard::Clipboard::new() else {
            eprintln!("skipping: no OS clipboard available in this environment");
            return;
        };
        let original = rect("Rectangle 1");
        let original_id = original.id;
        copy_layers(&[original]);
        // Sanity check the sentinel actually made it onto the clipboard
        // before asserting on `paste_layers`'s parsed result.
        let Ok(raw) = probe.get_text() else {
            eprintln!("skipping: clipboard round-trip unavailable in this environment");
            return;
        };
        if !raw.starts_with(SENTINEL) {
            eprintln!("skipping: clipboard did not retain the set text in this environment");
            return;
        }

        let pasted = paste_layers().expect("should parse what copy_layers just wrote");
        assert_eq!(pasted.len(), 1);
        assert_eq!(pasted[0].name, "Rectangle 1");
        assert_eq!(pasted[0].frame.pos, Pos2::new(1.0, 2.0));
        assert_ne!(pasted[0].id, original_id, "paste must regenerate ids");

        let _ = probe.set_text("just some ordinary clipboard text".to_string());
        assert!(paste_layers().is_none());
    }
}
