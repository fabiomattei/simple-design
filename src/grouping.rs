use egui::Vec2;

use crate::model::{Frame, Layer, LayerId, LayerKind, Page};

/// Default offset applied to a duplicate's position relative to its
/// original, so the copy is visibly distinguishable from (and doesn't sit
/// exactly under) the layer it was duplicated from — the "Offset
/// duplicated layers" setting (`CanvasWidget::duplicate_offset` in this
/// codebase, since it's session/UI state rather than part of the saved
/// document). Callers that want the copy to land exactly on top of the
/// original instead (e.g. Option-drag, which immediately drags it away with
/// the mouse) pass `Vec2::ZERO`.
pub const DEFAULT_DUPLICATE_OFFSET: Vec2 = Vec2::new(10.0, 10.0);

/// Duplicates each given layer in place: the copy gets a fresh id (and fresh
/// ids for every descendant), is inserted immediately after the original in
/// its own parent list (page top level, or an artboard/group's children),
/// and is nudged by `offset`. A default-named copy's trailing number is
/// incremented (`"Frame 1"` -> `"Frame 2"`); a custom name is left as-is
/// (see `increment_trailing_number`). Each id is resolved independently, so
/// a selection spanning multiple parents duplicates fine. Returns the new
/// layers' ids, in the same order as `ids` (skipping any id not found).
pub fn duplicate_layers(page: &mut Page, ids: &[LayerId], offset: Vec2) -> Vec<LayerId> {
    ids.iter()
        .filter_map(|&id| duplicate_one(&mut page.layers, id, offset, false))
        .collect()
}

/// `duplicate_layers`, but inserts each copy immediately *before* the
/// original instead of after — the Shift+Cmd+D "Duplicate Below" shortcut.
pub fn duplicate_layers_below(page: &mut Page, ids: &[LayerId], offset: Vec2) -> Vec<LayerId> {
    ids.iter()
        .filter_map(|&id| duplicate_one(&mut page.layers, id, offset, true))
        .collect()
}

fn duplicate_one(layers: &mut Vec<Layer>, id: LayerId, offset: Vec2, below: bool) -> Option<LayerId> {
    if let Some(pos) = layers.iter().position(|l| l.id == id) {
        let mut clone = layers[pos].clone();
        clone.regenerate_ids();
        clone.frame.pos += offset;
        clone.name = increment_trailing_number(&clone.name);
        let new_id = clone.id;
        layers.insert(if below { pos } else { pos + 1 }, clone);
        return Some(new_id);
    }
    for layer in layers.iter_mut() {
        if let Some(children) = layer.kind.children_mut() {
            if let Some(new_id) = duplicate_one(children, id, offset, below) {
                return Some(new_id);
            }
        }
    }
    None
}

/// `"Frame 2"` -> `"Frame 3"`, but `"Icon"` (no trailing number) is returned
/// unchanged — matches the common duplicate-naming rule that a custom name
/// (however you define "custom" — this codebase just checks for a trailing
/// `" <digits>"`) is left alone while a default numbered name increments.
pub fn increment_trailing_number(name: &str) -> String {
    let Some(space_idx) = name.rfind(' ') else { return name.to_string() };
    let (base, suffix) = name.split_at(space_idx);
    let digits = &suffix[1..];
    if digits.is_empty() || !digits.chars().all(|c| c.is_ascii_digit()) {
        return name.to_string();
    }
    let Ok(n) = digits.parse::<u64>() else { return name.to_string() };
    format!("{base} {}", n + 1)
}

/// Groups the given layers (which must all be direct siblings under the same
/// parent — the page itself, an artboard, or another group) into a new
/// `Group` layer inserted at the position of the frontmost selected layer.
/// Returns the new group's id, or `None` if the layers don't share a common
/// parent or none of the ids were found.
pub fn group_layers(page: &mut Page, ids: &[LayerId]) -> Option<LayerId> {
    if ids.is_empty() {
        return None;
    }
    let parent = find_common_parent_list(&mut page.layers, ids)?;

    let mut removed = Vec::new();
    let mut insert_at = 0;
    let mut i = 0;
    while i < parent.len() {
        if ids.contains(&parent[i].id) {
            insert_at = insert_at.max(i);
            removed.push(parent.remove(i));
        } else {
            i += 1;
        }
    }
    if removed.is_empty() {
        return None;
    }
    insert_at = insert_at.min(parent.len());

    // `rotated_bounds()`, not `bounds()`, so the new Group's own frame
    // tightly wraps each child's actual visual footprint — the children's
    // own `frame.pos`/`rotation` are only ever translated by a constant
    // offset below, never touched otherwise, so this doesn't disturb their
    // individual rotation math.
    let bbox = removed
        .iter()
        .map(|l| l.frame.rotated_bounds())
        .reduce(|a, b| a.union(b))?;

    for child in &mut removed {
        child.frame.pos -= bbox.min.to_vec2();
    }

    let group = Layer::new("Group", Frame::from_bounds(bbox), LayerKind::Group { children: removed });
    let id = group.id;
    parent.insert(insert_at, group);
    Some(id)
}

/// Ungroups the given group layer, splicing its children back into the
/// parent at the group's former position (converted back to parent-relative
/// coordinates). Returns the ids of the layers spliced in, in their new
/// selection order, or an empty vec if `group_id` isn't a group.
pub fn ungroup(page: &mut Page, group_id: LayerId) -> Vec<LayerId> {
    let Some(parent) = find_common_parent_list(&mut page.layers, &[group_id]) else {
        return Vec::new();
    };
    let Some(idx) = parent.iter().position(|l| l.id == group_id) else {
        return Vec::new();
    };
    if !matches!(parent[idx].kind, LayerKind::Group { .. }) {
        return Vec::new();
    }
    let group = parent.remove(idx);
    let offset = group.frame.pos.to_vec2();
    let LayerKind::Group { mut children } = group.kind else {
        unreachable!("checked above")
    };
    for child in &mut children {
        child.frame.pos += offset;
    }
    let ids: Vec<LayerId> = children.iter().map(|c| c.id).collect();
    for (i, child) in children.into_iter().enumerate() {
        parent.insert(idx + i, child);
    }
    ids
}

/// Moves layer `id` to array index `index` within `new_parent`'s children
/// (or the page's top-level list if `new_parent` is `None`) — the model-level
/// operation behind the layers panel's drag-and-drop reordering (see
/// `ui/layers_panel.rs`'s `LayerAction::Move`). `index` follows this
/// codebase's back-to-front array convention (`0` = furthest back), and is
/// interpreted in the *destination* list's coordinates as they'd read
/// *before* the moved layer is pulled out of its old spot — if the old and
/// new parent are the same list, this adjusts for the removal shift itself.
///
/// A no-op (rather than losing the layer) if `new_parent` doesn't exist,
/// isn't a container, or is `id` itself or one of `id`'s own descendants —
/// any of which would otherwise create a cycle or silently drop content.
pub fn move_layer(page: &mut Page, id: LayerId, new_parent: Option<LayerId>, mut index: usize) {
    if new_parent == Some(id) {
        return;
    }
    if let Some(new_parent_id) = new_parent {
        if page.find(id).is_some_and(|dragged| dragged.find(new_parent_id).is_some()) {
            return; // would move a layer into its own descendant
        }
    }
    let Some((removed, old_parent, old_index)) = extract(page, id) else { return };

    if old_parent == new_parent && old_index < index {
        index -= 1;
    }

    let target = match new_parent {
        None => Some(&mut page.layers),
        Some(pid) => page.find_mut(pid).and_then(|l| l.kind.children_mut()),
    };
    match target {
        Some(list) => {
            let index = index.min(list.len());
            list.insert(index, removed);
        }
        // Target vanished or isn't a container (shouldn't happen from the
        // UI, which only offers real drop zones) — put it back at the top
        // level rather than lose it.
        None => page.layers.push(removed),
    }
}

/// Removes a layer by id from wherever it currently lives, returning it
/// along with its former parent's id (`None` = page top level) and its
/// index within that list — the extra bookkeeping `Page::remove` doesn't
/// need for its own (delete) use case, but `move_layer` does to correctly
/// re-target an in-place reorder.
fn extract(page: &mut Page, id: LayerId) -> Option<(Layer, Option<LayerId>, usize)> {
    if let Some(pos) = page.layers.iter().position(|l| l.id == id) {
        return Some((page.layers.remove(pos), None, pos));
    }
    extract_from_children(&mut page.layers, id)
}

fn extract_from_children(layers: &mut Vec<Layer>, id: LayerId) -> Option<(Layer, Option<LayerId>, usize)> {
    for layer in layers.iter_mut() {
        if let Some(children) = layer.kind.children_mut() {
            if let Some(pos) = children.iter().position(|c| c.id == id) {
                return Some((children.remove(pos), Some(layer.id), pos));
            }
            if let Some(found) = extract_from_children(children, id) {
                return Some(found);
            }
        }
    }
    None
}

/// Every id in `ids`'s shared parent's children list — the page's top-level
/// `layers` if `ids` is empty or doesn't share a common parent. Backs
/// the Cmd+A shortcut ("select all layers in the parent group"), see
/// `app.rs::handle_shortcuts`.
pub fn siblings_of(page: &Page, ids: &[LayerId]) -> Vec<LayerId> {
    if !ids.is_empty() {
        if let Some(list) = find_common_parent_list_ref(&page.layers, ids) {
            return list.iter().map(|l| l.id).collect();
        }
    }
    page.layers.iter().map(|l| l.id).collect()
}

/// `id`'s parent (`None` = page top level) and its index within that
/// parent's children — the read-only lookup behind "select the next
/// layer after deleting one from inside a group" (see `app.rs`'s Delete/
/// Backspace handler). `None` if `id` isn't found at all.
pub fn parent_and_index(layers: &[Layer], id: LayerId) -> Option<(Option<LayerId>, usize)> {
    if let Some(idx) = layers.iter().position(|l| l.id == id) {
        return Some((None, idx));
    }
    for layer in layers {
        if let Some(children) = layer.kind.children() {
            if let Some(idx) = children.iter().position(|c| c.id == id) {
                return Some((Some(layer.id), idx));
            }
            if let Some(found) = parent_and_index(children, id) {
                return Some(found);
            }
        }
    }
    None
}

/// Read-only counterpart to `find_common_parent_list`, used where no
/// mutation is needed (avoids cloning the tree just to look).
fn find_common_parent_list_ref<'a>(layers: &'a [Layer], ids: &[LayerId]) -> Option<&'a [Layer]> {
    let all_here = ids.iter().all(|id| layers.iter().any(|l| l.id == *id));
    if all_here {
        return Some(layers);
    }
    for layer in layers {
        if let Some(children) = layer.kind.children() {
            if let Some(found) = find_common_parent_list_ref(children, ids) {
                return Some(found);
            }
        }
    }
    None
}

/// Finds the `Vec<Layer>` (page's top-level list, or some artboard/group's
/// children) that directly contains every id in `ids`. Returns `None` if no
/// single list holds all of them (e.g. the selection spans multiple parents).
pub(crate) fn find_common_parent_list<'a>(
    layers: &'a mut Vec<Layer>,
    ids: &[LayerId],
) -> Option<&'a mut Vec<Layer>> {
    let all_here = ids.iter().all(|id| layers.iter().any(|l| l.id == *id));
    if all_here {
        return Some(layers);
    }
    for layer in layers.iter_mut() {
        if let Some(children) = layer.kind.children_mut() {
            if let Some(found) = find_common_parent_list(children, ids) {
                return Some(found);
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::Pos2;
    use crate::model::CornerRadii;

    #[test]
    fn group_layers_bbox_tightly_wraps_a_rotated_childs_visual_footprint() {
        // A 20x20 square rotated 45 degrees at (0,0) has a rotated visual
        // AABB of side 20*sqrt(2) ~= 28.28, centered on (10,10) — quite
        // different from its unrotated local `bounds()` (0,0)-(20,20).
        let rotated = Layer::new(
            "Rotated",
            Frame { pos: Pos2::new(0.0, 0.0), size: Vec2::new(20.0, 20.0), rotation: 45.0 },
            LayerKind::Rectangle { corner_radius: CornerRadii::ZERO },
        );
        let id = rotated.id;
        let mut page = Page::new("Page 1");
        page.layers.push(rotated);

        let group_id = group_layers(&mut page, &[id]).expect("should group");
        let group = page.find(group_id).expect("group should exist");

        let rotated_aabb_side = 20.0 * std::f32::consts::SQRT_2;
        assert!((group.frame.size.x - rotated_aabb_side).abs() < 0.5, "width={}", group.frame.size.x);
        assert!((group.frame.size.y - rotated_aabb_side).abs() < 0.5, "height={}", group.frame.size.y);

        // The child's own rotation is untouched by grouping.
        let LayerKind::Group { children } = &group.kind else { unreachable!() };
        assert_eq!(children[0].frame.rotation, 45.0);
    }

    fn rect(name: &str) -> Layer {
        Layer::new(name, Frame { pos: Pos2::ZERO, size: Vec2::new(10.0, 10.0), rotation: 0.0 }, LayerKind::Rectangle { corner_radius: CornerRadii::ZERO })
    }

    fn names(page: &Page) -> Vec<&str> {
        page.layers.iter().map(|l| l.name.as_str()).collect()
    }

    #[test]
    fn move_layer_reorders_within_the_same_top_level_list() {
        let mut page = Page::new("Page 1");
        page.layers.push(rect("A"));
        page.layers.push(rect("B"));
        page.layers.push(rect("C"));
        let a_id = page.layers[0].id;

        // Move "A" (index 0) to the front (index 2, i.e. past the end of
        // the pre-removal list) -> back-to-front order becomes B, C, A.
        move_layer(&mut page, a_id, None, 3);
        assert_eq!(names(&page), vec!["B", "C", "A"]);
    }

    #[test]
    fn move_layer_moves_a_layer_into_a_group() {
        let mut page = Page::new("Page 1");
        page.layers.push(rect("Loose"));
        let group = Layer::new("Group", Frame { pos: Pos2::ZERO, size: Vec2::new(10.0, 10.0), rotation: 0.0 }, LayerKind::Group { children: vec![rect("Inside")] });
        let group_id = group.id;
        page.layers.push(group);
        let loose_id = page.layers[0].id;

        move_layer(&mut page, loose_id, Some(group_id), 0);

        assert_eq!(names(&page), vec!["Group"]);
        let LayerKind::Group { children } = &page.layers[0].kind else { unreachable!() };
        assert_eq!(children.iter().map(|l| l.name.as_str()).collect::<Vec<_>>(), vec!["Loose", "Inside"]);
    }

    #[test]
    fn move_layer_moves_a_layer_out_of_a_group_to_the_top_level() {
        let mut page = Page::new("Page 1");
        let group = Layer::new("Group", Frame { pos: Pos2::ZERO, size: Vec2::new(10.0, 10.0), rotation: 0.0 }, LayerKind::Group { children: vec![rect("Inside")] });
        let inside_id = if let LayerKind::Group { children } = &group.kind { children[0].id } else { unreachable!() };
        page.layers.push(group);

        move_layer(&mut page, inside_id, None, 0);

        assert_eq!(names(&page), vec!["Inside", "Group"]);
        let LayerKind::Group { children } = &page.layers[1].kind else { unreachable!() };
        assert!(children.is_empty());
    }

    #[test]
    fn move_layer_into_its_own_descendant_is_a_noop() {
        let mut page = Page::new("Page 1");
        let inner_group = Layer::new(
            "Inner",
            Frame { pos: Pos2::ZERO, size: Vec2::new(10.0, 10.0), rotation: 0.0 },
            LayerKind::Group { children: vec![] },
        );
        let inner_id = inner_group.id;
        let outer_group =
            Layer::new("Outer", Frame { pos: Pos2::ZERO, size: Vec2::new(10.0, 10.0), rotation: 0.0 }, LayerKind::Group { children: vec![inner_group] });
        let outer_id = outer_group.id;
        page.layers.push(outer_group);

        // "Outer" contains "Inner" — moving "Outer" into "Inner" would
        // create a cycle, so this must do nothing.
        move_layer(&mut page, outer_id, Some(inner_id), 0);

        assert_eq!(names(&page), vec!["Outer"]);
        let LayerKind::Group { children } = &page.layers[0].kind else { unreachable!() };
        assert_eq!(children.len(), 1);
        assert_eq!(children[0].id, inner_id);
    }

    #[test]
    fn siblings_of_returns_the_common_parents_children() {
        let mut page = Page::new("Page 1");
        let inner = rect("Inner");
        let inner_id = inner.id;
        let group = Layer::new("Group", Frame { pos: Pos2::ZERO, size: Vec2::new(10.0, 10.0), rotation: 0.0 }, LayerKind::Group { children: vec![inner] });
        page.layers.push(rect("Loose"));
        page.layers.push(group);

        assert_eq!(siblings_of(&page, &[inner_id]), vec![inner_id]);
    }

    #[test]
    fn siblings_of_falls_back_to_top_level_when_empty_or_no_common_parent() {
        let mut page = Page::new("Page 1");
        page.layers.push(rect("A"));
        page.layers.push(rect("B"));
        let top_level: Vec<LayerId> = page.layers.iter().map(|l| l.id).collect();

        assert_eq!(siblings_of(&page, &[]), top_level);
        // A bogus id shares no parent with anything -> falls back to top level.
        assert_eq!(siblings_of(&page, &[LayerId::new_v4()]), top_level);
    }

    #[test]
    fn increment_trailing_number_increments_a_trailing_count() {
        assert_eq!(increment_trailing_number("Frame 1"), "Frame 2");
        assert_eq!(increment_trailing_number("Rectangle 9"), "Rectangle 10");
    }

    #[test]
    fn increment_trailing_number_leaves_custom_names_unchanged() {
        assert_eq!(increment_trailing_number("Icon"), "Icon");
        assert_eq!(increment_trailing_number("My Cool Shape"), "My Cool Shape");
    }

    #[test]
    fn duplicate_layers_offsets_and_increments_default_names() {
        let mut page = Page::new("Page 1");
        page.layers.push(rect("Rectangle 1"));
        let id = page.layers[0].id;

        let new_ids = duplicate_layers(&mut page, &[id], Vec2::new(5.0, 5.0));
        assert_eq!(new_ids.len(), 1);
        let dup = page.find(new_ids[0]).unwrap();
        assert_eq!(dup.name, "Rectangle 2");
        assert_eq!(dup.frame.pos, Pos2::new(5.0, 5.0));
    }

    #[test]
    fn duplicate_layers_below_inserts_before_the_original() {
        let mut page = Page::new("Page 1");
        page.layers.push(rect("A"));
        let id = page.layers[0].id;

        duplicate_layers_below(&mut page, &[id], Vec2::ZERO);
        assert_eq!(page.layers[0].name, "A");
        assert_eq!(page.layers[1].id, id);
    }

    #[test]
    fn parent_and_index_finds_top_level_and_nested_layers() {
        let mut page = Page::new("Page 1");
        let inner = rect("Inner");
        let inner_id = inner.id;
        let group = Layer::new("Group", Frame { pos: Pos2::ZERO, size: Vec2::new(10.0, 10.0), rotation: 0.0 }, LayerKind::Group { children: vec![inner] });
        let group_id = group.id;
        page.layers.push(rect("A"));
        page.layers.push(group);

        assert_eq!(parent_and_index(&page.layers, group_id), Some((None, 1)));
        assert_eq!(parent_and_index(&page.layers, inner_id), Some((Some(group_id), 0)));
        assert_eq!(parent_and_index(&page.layers, LayerId::new_v4()), None);
    }

    #[test]
    fn move_layer_onto_itself_is_a_noop() {
        let mut page = Page::new("Page 1");
        page.layers.push(rect("A"));
        let a_id = page.layers[0].id;

        move_layer(&mut page, a_id, Some(a_id), 0);

        assert_eq!(names(&page), vec!["A"]);
    }
}
