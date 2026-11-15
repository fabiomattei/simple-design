//! File > New from Template: a full-window picker shown in place of the editor, modeled on
//! simple-text's welcome screen — a title row, a filter field that keeps the keyboard focus,
//! and the templates as preview tiles in a "Built-in" and a "My Templates" section. The arrow
//! keys move a selection (left and right only while the filter is empty, so its text can be
//! edited), Enter opens it (or the first match while filtering), Escape clears the selection
//! and then closes the picker, and ⌘1 to ⌘9 open the first nine templates.

mod grid_navigation;
mod picker_header;
mod picker_keys;
mod section_heading;
mod template_thumbnails;
mod template_tile;

use egui::{Frame, Margin, Popup, Rect, RichText, ScrollArea, Ui, vec2};

use crate::templates::{self, Template, TemplateSource};
use grid_navigation::grid_columns;
use picker_header::{filter_field, filter_id, title_row};
use picker_keys::{DIGITS, handle_keys};
use section_heading::section_heading;
use template_thumbnails::TemplateThumbnails;
use template_tile::{TILE_HEIGHT, TILE_WIDTH, TileClick, TileLook, template_tile};

const MARGIN: i8 = 24;
const HEADER_GAP: f32 = 16.0;
const ROW_GAP: f32 = 18.0;
const SECTION_GAP: f32 = 12.0;
const BUILT_IN_TITLE: &str = "Built-in";
const USER_TITLE: &str = "My Templates";

/// What the user picked this frame; indices are for `TemplatePickerState::template`.
pub enum TemplatePick {
    Open(usize),
    /// Show the user template's file in the file manager.
    Reveal(usize),
    /// Delete the user template's file, once confirmed.
    Delete(usize),
    Close,
}

/// What the picker keeps between frames.
#[derive(Default)]
pub struct TemplatePickerState {
    is_open: bool,
    /// Listed when the picker opened, so indices stay valid while it is open.
    templates: Vec<Template>,
    filter: String,
    /// The template chosen with the arrow keys.
    selected: Option<usize>,
    /// Tiles per row last frame, for moving up and down.
    columns: usize,
    thumbnails: TemplateThumbnails,
}

impl TemplatePickerState {
    /// Shows the picker with the templates listed afresh, so one saved since the last opening
    /// appears; also refreshes the list after a template is deleted.
    pub fn open(&mut self) {
        self.templates = templates::all_templates();
        self.thumbnails.clear();
        self.filter.clear();
        self.selected = None;
        self.is_open = true;
    }

    /// Hides the picker and drops its previews' textures.
    pub fn close(&mut self) {
        self.is_open = false;
        self.templates.clear();
        self.thumbnails.clear();
    }

    pub fn is_open(&self) -> bool {
        self.is_open
    }

    pub fn template(&self, index: usize) -> Option<&Template> {
        self.templates.get(index)
    }
}

/// The templates of one section, as indices into the picker's list.
struct Section {
    title: &'static str,
    templates: Vec<usize>,
}

/// The built-in and then the user templates whose name contains `filter` (ignoring case);
/// a section with none is left out.
fn template_sections(templates: &[Template], filter: &str) -> Vec<Section> {
    let filter = filter.trim().to_lowercase();
    let matching = |user: bool| -> Vec<usize> {
        templates
            .iter()
            .enumerate()
            .filter(|(_, template)| matches!(template.source, TemplateSource::User(_)) == user)
            .filter(|(_, template)| template.name.to_lowercase().contains(&filter))
            .map(|(index, _)| index)
            .collect()
    };
    [(BUILT_IN_TITLE, matching(false)), (USER_TITLE, matching(true))]
        .into_iter()
        .filter(|(_, templates)| !templates.is_empty())
        .map(|(title, templates)| Section { title, templates })
        .collect()
}

/// Every listed template, in reading order.
fn visible_templates(sections: &[Section]) -> Vec<usize> {
    sections.iter().flat_map(|section| section.templates.iter().copied()).collect()
}

/// Draws the picker filling `ui`; returns what was picked this frame.
pub fn ui(ui: &mut Ui, state: &mut TemplatePickerState) -> Option<TemplatePick> {
    let mut sections = template_sections(&state.templates, &state.filter);
    let mut reveal = false;
    let mut picked = handle_keys(ui, state, &sections, &mut reveal);
    Frame::NONE.inner_margin(Margin::same(MARGIN)).show(ui, |ui| {
        if title_row(ui) {
            picked = Some(TemplatePick::Close);
        }
        ui.add_space(HEADER_GAP);
        let before = state.filter.clone();
        filter_field(ui, &mut state.filter);
        // Typing goes to the filter unless something else has the keyboard.
        if ui.memory(|memory| memory.focused().is_none()) && !Popup::is_any_open(ui.ctx()) {
            ui.memory_mut(|memory| memory.request_focus(filter_id()));
        }
        if state.filter != before {
            sections = template_sections(&state.templates, &state.filter);
            let filtering = !state.filter.trim().is_empty();
            state.selected = visible_templates(&sections).first().copied().filter(|_| filtering);
            reveal = true;
        }
        ui.add_space(HEADER_GAP * 1.5);
        if sections.is_empty() {
            ui.label(RichText::new(format!("No templates match “{}”", state.filter.trim())).weak());
            return;
        }
        ScrollArea::vertical().id_salt("template picker").auto_shrink(false).show(ui, |ui| {
            if let Some(pick) = show_sections(ui, state, &sections, reveal) {
                picked = Some(pick);
            }
        });
    });
    picked
}

/// Draws `sections` one under another, spreading each row's tiles across the width; returns
/// what was done to a tile.
fn show_sections(ui: &mut Ui, state: &mut TemplatePickerState, sections: &[Section], reveal: bool) -> Option<TemplatePick> {
    let (columns, gap) = grid_columns(ui.available_width(), TILE_WIDTH);
    state.columns = columns;
    let shortcuts = ui.input(|input| input.modifiers.command);
    let visible = visible_templates(sections);
    let mut picked = None;
    for section in sections {
        section_heading(ui, section.title, section.templates.len());
        for row in section.templates.chunks(columns) {
            // Previews render on first use, so only rows scrolled into view pay for them;
            // rendering every one up front froze the picker on open.
            let row_rect = Rect::from_min_size(ui.cursor().min, vec2(ui.available_width(), TILE_HEIGHT));
            let row_visible = ui.is_rect_visible(row_rect);
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = gap;
                for &index in row {
                    let template = &state.templates[index];
                    let thumbnail = if row_visible { state.thumbnails.get(ui.ctx(), index, template) } else { None };
                    let selected = state.selected == Some(index);
                    let look = TileLook {
                        thumbnail: thumbnail.as_ref(),
                        selected,
                        reveal: reveal && selected,
                        shortcut: shortcuts
                            .then(|| visible.iter().position(|&shown| shown == index))
                            .flatten()
                            .filter(|&position| position < DIGITS.len())
                            .map(|position| position + 1),
                        is_user: matches!(template.source, TemplateSource::User(_)),
                    };
                    if let Some(click) = template_tile(ui, &template.name, look) {
                        picked = Some(match click {
                            TileClick::Open => TemplatePick::Open(index),
                            TileClick::Reveal => TemplatePick::Reveal(index),
                            TileClick::Delete => TemplatePick::Delete(index),
                        });
                    }
                }
            });
            ui.add_space(ROW_GAP);
        }
        ui.add_space(SECTION_GAP);
    }
    picked
}
