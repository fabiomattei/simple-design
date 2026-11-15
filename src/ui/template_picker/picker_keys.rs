//! The template picker's keyboard handling (see the module doc of `template_picker`).

use egui::{Key, Modifiers, Popup, Ui};

use super::grid_navigation::{Step, grid_positions, step};
use super::picker_header::filter_id;
use super::{Section, TemplatePick, TemplatePickerState, visible_templates};

/// The ⌘-digit shortcuts, opening the first nine templates.
pub const DIGITS: [Key; 9] =
    [Key::Num1, Key::Num2, Key::Num3, Key::Num4, Key::Num5, Key::Num6, Key::Num7, Key::Num8, Key::Num9];

/// The picker's keys (see the module doc). Nothing happens while another field or a menu has
/// the keyboard. Sets `reveal` when the selection moved.
pub fn handle_keys(ui: &mut Ui, state: &mut TemplatePickerState, sections: &[Section], reveal: &mut bool) -> Option<TemplatePick> {
    let focus = ui.memory(|memory| memory.focused());
    if focus.is_some_and(|id| id != filter_id()) || Popup::is_any_open(ui.ctx()) {
        return None;
    }
    let visible = visible_templates(sections);
    let consume = |modifiers, key| ui.input_mut(|input| input.consume_key(modifiers, key));
    for (position, &key) in DIGITS.iter().enumerate().take(visible.len()) {
        if consume(Modifiers::COMMAND, key) {
            return Some(TemplatePick::Open(visible[position]));
        }
    }
    let current = state.selected.and_then(|selected| visible.iter().position(|&shown| shown == selected));
    if current.is_none() {
        state.selected = None;
    }
    // The filter field takes Escape first while it has text.
    if state.filter.is_empty() && consume(Modifiers::NONE, Key::Escape) {
        if current.is_some() {
            state.selected = None;
            return None;
        }
        return Some(TemplatePick::Close);
    }
    if visible.is_empty() {
        return None;
    }
    let sideways = state.filter.is_empty();
    let steps =
        [(Key::ArrowLeft, Step::Left, sideways), (Key::ArrowRight, Step::Right, sideways), (Key::ArrowUp, Step::Up, true), (Key::ArrowDown, Step::Down, true)];
    let section_sizes: Vec<usize> = sections.iter().map(|section| section.templates.len()).collect();
    for (key, direction, enabled) in steps {
        if enabled && consume(Modifiers::NONE, key) {
            let next = current.map_or(0, |from| step(&grid_positions(&section_sizes, state.columns), from, direction));
            state.selected = Some(visible[next]);
            *reveal = true;
            return None;
        }
    }
    let filtering = !state.filter.trim().is_empty();
    if let Some(position) = current.or(filtering.then_some(0))
        && consume(Modifiers::NONE, Key::Enter)
    {
        return Some(TemplatePick::Open(visible[position]));
    }
    None
}
