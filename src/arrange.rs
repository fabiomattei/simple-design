//! Z-order commands (Bring Forward / Send Backward / Bring to Front / Send
//! to Back). Each selected layer only moves within its own parent's
//! children list — arranging never reparents, unlike the layers panel's
//! drag-and-drop (`grouping::move_layer`).
//!
//! Arranging also never changes what a mask clips. A parent's children
//! split into mask blocks (a mask plus the run below it that it clips, see
//! `masking::partition_mask_runs`) followed by the unclipped layers above
//! the topmost mask. A mask moves with its whole block, only among the other
//! blocks; any other layer only reorders inside its own block (below its
//! mask) or inside the unclipped part.

use crate::model::{Layer, LayerId, Page};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ArrangeCommand {
    BringForward,
    SendBackward,
    BringToFront,
    SendToBack,
}

impl ArrangeCommand {
    pub const ALL: [ArrangeCommand; 4] =
        [Self::BringForward, Self::SendBackward, Self::BringToFront, Self::SendToBack];

    pub fn label(self) -> &'static str {
        match self {
            Self::BringForward => "Bring Forward (⌘])",
            Self::SendBackward => "Send Backward (⌘[)",
            Self::BringToFront => "Bring to Front (⌥⌘])",
            Self::SendToBack => "Send to Back (⌥⌘[)",
        }
    }
}

/// Reorders every layer in `ids` within its own parent's children list
/// (or the page's top-level list). Selected siblings move together and keep
/// their relative order, so a contiguous selected block moves as one unit.
/// Ids that don't exist on `page` are ignored.
pub fn arrange(page: &mut Page, ids: &[LayerId], command: ArrangeCommand) {
    arrange_in_list(&mut page.layers, ids, command);
}

fn arrange_in_list(list: &mut Vec<Layer>, ids: &[LayerId], command: ArrangeCommand) {
    reorder_siblings(list, ids, command);
    for layer in list.iter_mut() {
        if let Some(children) = layer.kind.children_mut() {
            arrange_in_list(children, ids, command);
        }
    }
}

fn reorder_siblings(list: &mut Vec<Layer>, ids: &[LayerId], command: ArrangeCommand) {
    let is_selected = |layer: &Layer| ids.contains(&layer.id);
    let (mut blocks, mut unclipped) = split_into_mask_blocks(std::mem::take(list));
    reorder(&mut blocks, |block: &Vec<Layer>| block.last().is_some_and(is_selected), command);
    for block in &mut blocks {
        let mask = block.pop().expect("every mask block ends with its mask");
        reorder(block, is_selected, command);
        block.push(mask);
    }
    reorder(&mut unclipped, is_selected, command);
    list.extend(blocks.into_iter().flatten());
    list.extend(unclipped);
}

/// Splits back-to-front `list` into mask blocks (each ending with its mask)
/// and the unclipped layers left above the topmost mask. A hidden mask
/// counts as a plain layer, matching `masking::partition_mask_runs` (an
/// invisible mask clips nothing).
fn split_into_mask_blocks(list: Vec<Layer>) -> (Vec<Vec<Layer>>, Vec<Layer>) {
    let mut blocks = Vec::new();
    let mut pending = Vec::new();
    for layer in list {
        let is_mask = layer.mask.enabled && layer.visible;
        pending.push(layer);
        if is_mask {
            blocks.push(std::mem::take(&mut pending));
        }
    }
    (blocks, pending)
}

/// `items` is back-to-front (index 0 = furthest back), so "forward" means a
/// higher index.
fn reorder<T>(items: &mut Vec<T>, is_selected: impl Fn(&T) -> bool, command: ArrangeCommand) {
    match command {
        // Walking from the front backward lets a selected run shift one
        // step as a unit: its frontmost member swaps first, opening the gap
        // the next one swaps into.
        ArrangeCommand::BringForward => {
            for i in (0..items.len().saturating_sub(1)).rev() {
                if is_selected(&items[i]) && !is_selected(&items[i + 1]) {
                    items.swap(i, i + 1);
                }
            }
        }
        ArrangeCommand::SendBackward => {
            for i in 1..items.len() {
                if is_selected(&items[i]) && !is_selected(&items[i - 1]) {
                    items.swap(i, i - 1);
                }
            }
        }
        ArrangeCommand::BringToFront => {
            let (selected, rest): (Vec<T>, Vec<T>) = items.drain(..).partition(|item| is_selected(item));
            items.extend(rest);
            items.extend(selected);
        }
        ArrangeCommand::SendToBack => {
            let (selected, rest): (Vec<T>, Vec<T>) = items.drain(..).partition(|item| is_selected(item));
            items.extend(selected);
            items.extend(rest);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{CornerRadii, Frame, LayerKind};
    use egui::{Pos2, Vec2};

    fn rect(name: &str) -> Layer {
        Layer::new(name, Frame { pos: Pos2::ZERO, size: Vec2::new(10.0, 10.0), rotation: 0.0 }, LayerKind::Rectangle { corner_radius: CornerRadii::ZERO })
    }

    fn page_of(names: &[&str]) -> Page {
        let mut page = Page::new("Page 1");
        for name in names {
            page.layers.push(rect(name));
        }
        page
    }

    fn names(layers: &[Layer]) -> Vec<&str> {
        layers.iter().map(|l| l.name.as_str()).collect()
    }

    #[test]
    fn bring_forward_moves_one_step_toward_the_front() {
        let mut page = page_of(&["A", "B", "C"]);
        let a = page.layers[0].id;
        arrange(&mut page, &[a], ArrangeCommand::BringForward);
        assert_eq!(names(&page.layers), vec!["B", "A", "C"]);
    }

    #[test]
    fn bring_forward_moves_a_contiguous_selection_as_a_block() {
        let mut page = page_of(&["A", "B", "C", "D"]);
        let ids = [page.layers[0].id, page.layers[1].id];
        arrange(&mut page, &ids, ArrangeCommand::BringForward);
        assert_eq!(names(&page.layers), vec!["C", "A", "B", "D"]);
    }

    #[test]
    fn bring_forward_at_the_front_is_a_noop() {
        let mut page = page_of(&["A", "B"]);
        let b = page.layers[1].id;
        arrange(&mut page, &[b], ArrangeCommand::BringForward);
        assert_eq!(names(&page.layers), vec!["A", "B"]);
    }

    #[test]
    fn send_backward_moves_one_step_toward_the_back() {
        let mut page = page_of(&["A", "B", "C"]);
        let c = page.layers[2].id;
        arrange(&mut page, &[c], ArrangeCommand::SendBackward);
        assert_eq!(names(&page.layers), vec!["A", "C", "B"]);
    }

    #[test]
    fn bring_to_front_keeps_the_selections_relative_order() {
        let mut page = page_of(&["A", "B", "C", "D"]);
        let ids = [page.layers[2].id, page.layers[0].id];
        arrange(&mut page, &ids, ArrangeCommand::BringToFront);
        assert_eq!(names(&page.layers), vec!["B", "D", "A", "C"]);
    }

    #[test]
    fn send_to_back_keeps_the_selections_relative_order() {
        let mut page = page_of(&["A", "B", "C", "D"]);
        let ids = [page.layers[3].id, page.layers[1].id];
        arrange(&mut page, &ids, ArrangeCommand::SendToBack);
        assert_eq!(names(&page.layers), vec!["B", "D", "A", "C"]);
    }

    /// "B", "C" | "M1" | "D" | "M2" | "E", "F" — two mask blocks under two
    /// unclipped layers.
    fn masked_page() -> Page {
        let mut page = page_of(&["B", "C", "M1", "D", "M2", "E", "F"]);
        page.layers[2].mask.enabled = true;
        page.layers[4].mask.enabled = true;
        page
    }

    #[test]
    fn a_mask_moves_together_with_the_layers_it_clips() {
        let mut page = masked_page();
        let m1 = page.layers[2].id;
        arrange(&mut page, &[m1], ArrangeCommand::BringForward);
        assert_eq!(names(&page.layers), vec!["D", "M2", "B", "C", "M1", "E", "F"]);
    }

    #[test]
    fn a_mask_brought_to_front_stays_below_the_unclipped_layers() {
        let mut page = masked_page();
        let m1 = page.layers[2].id;
        arrange(&mut page, &[m1], ArrangeCommand::BringToFront);
        assert_eq!(names(&page.layers), vec!["D", "M2", "B", "C", "M1", "E", "F"]);
    }

    #[test]
    fn a_clipped_layer_stays_inside_its_mask() {
        let mut page = masked_page();
        let b = page.layers[0].id;
        arrange(&mut page, &[b], ArrangeCommand::BringToFront);
        assert_eq!(names(&page.layers), vec!["C", "B", "M1", "D", "M2", "E", "F"]);
    }

    #[test]
    fn an_unclipped_layer_never_moves_under_a_mask() {
        let mut page = masked_page();
        let e = page.layers[5].id;
        arrange(&mut page, &[e], ArrangeCommand::SendToBack);
        assert_eq!(names(&page.layers), vec!["B", "C", "M1", "D", "M2", "E", "F"]);
    }

    #[test]
    fn a_hidden_mask_is_arranged_like_a_plain_layer() {
        let mut page = page_of(&["A", "M", "B"]);
        page.layers[1].mask.enabled = true;
        page.layers[1].visible = false;
        let m = page.layers[1].id;
        arrange(&mut page, &[m], ArrangeCommand::BringForward);
        assert_eq!(names(&page.layers), vec!["A", "B", "M"]);
    }

    #[test]
    fn arranges_inside_a_group_without_reparenting() {
        let mut page = page_of(&["Outside"]);
        let inner = vec![rect("X"), rect("Y"), rect("Z")];
        let x = inner[0].id;
        page.layers.push(Layer::new("Group", Frame { pos: Pos2::ZERO, size: Vec2::new(10.0, 10.0), rotation: 0.0 }, LayerKind::Group { children: inner }));

        arrange(&mut page, &[x], ArrangeCommand::BringToFront);

        assert_eq!(names(&page.layers), vec!["Outside", "Group"]);
        let LayerKind::Group { children } = &page.layers[1].kind else { unreachable!() };
        assert_eq!(names(children), vec!["Y", "Z", "X"]);
    }
}
