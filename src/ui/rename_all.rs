use egui::Vec2;

use crate::model::LayerId;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NameCase {
    AsIs,
    Upper,
    Lower,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SequenceStyle {
    /// 1, 2, 3, ...
    Numeric,
    /// a, b, c, ..., z, aa, ab, ... (spreadsheet-column style), 1-based.
    Alphabetic,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SizeFormat {
    Width,
    Height,
    Both,
}

/// One piece of a Rename All template, composed in order to build each
/// layer's new name (see `render_token`/`compute_name`). Mirrors Sketch's
/// "modifier tokens": Name, Sequence, Size — plus a plain `Literal` for
/// separators/spacing between them, since Sketch's own UI inserts those
/// implicitly but this one needs them explicit.
#[derive(Clone, Debug, PartialEq)]
pub enum Token {
    Name(NameCase),
    Sequence(SequenceStyle),
    Size(SizeFormat),
    Literal(String),
}

/// Which layers a Rename All run applies to. `WholePage`'s `filter` (only
/// shown/used in that mode) restricts it to layers whose current name
/// contains the filter text (case-insensitive substring), matching Sketch's
/// own "Filter field only appears if you've opted to rename all layers on
/// the current page".
#[derive(Clone, Debug, PartialEq)]
pub enum Scope {
    Selected,
    WholePage { filter: String },
}

#[derive(Clone, Debug, PartialEq)]
pub struct RenameAllConfig {
    pub scope: Scope,
    /// Find/replace applied to each name *before* token rendering (Sketch's
    /// "Match", separate from `Scope::WholePage`'s filter). Empty
    /// `match_find` is a no-op. `regex` treats `match_find` as a regex
    /// pattern instead of a literal substring; either way, `case_sensitive`
    /// controls matching case — both paths go through the same `regex`
    /// engine (a literal search is just `regex::escape`d first), so there's
    /// only one matching implementation to get right.
    pub match_find: String,
    pub match_replace: String,
    pub regex: bool,
    pub case_sensitive: bool,
    pub sequence_start: i64,
    pub tokens: Vec<Token>,
}

impl Default for RenameAllConfig {
    fn default() -> Self {
        Self {
            scope: Scope::Selected,
            match_find: String::new(),
            match_replace: String::new(),
            regex: false,
            case_sensitive: false,
            sequence_start: 1,
            tokens: vec![Token::Name(NameCase::AsIs), Token::Literal(" ".to_string()), Token::Sequence(SequenceStyle::Numeric)],
        }
    }
}

/// Applies `config`'s Match find/replace to `name`. Both the plain-substring
/// and regex paths run through the `regex` crate — a literal search is just
/// `match_find` regex-escaped first — so case-sensitivity and matching
/// behavior can't silently disagree between the two modes.
pub fn apply_match(name: &str, config: &RenameAllConfig) -> String {
    if config.match_find.is_empty() {
        return name.to_string();
    }
    let pattern = if config.regex { config.match_find.clone() } else { regex::escape(&config.match_find) };
    match regex::RegexBuilder::new(&pattern).case_insensitive(!config.case_sensitive).build() {
        Ok(re) => re.replace_all(name, config.match_replace.as_str()).to_string(),
        Err(_) => name.to_string(),
    }
}

/// 1-based spreadsheet-column-style sequence: 1 -> "a", 26 -> "z",
/// 27 -> "aa", ... `n < 1` returns an empty string (shouldn't happen given
/// `sequence_start` + a non-negative index, but avoids an underflow panic if
/// it somehow does).
fn alphabetic_sequence(n: i64) -> String {
    if n < 1 {
        return String::new();
    }
    let mut n = n as u64;
    let mut bytes = Vec::new();
    while n > 0 {
        let rem = ((n - 1) % 26) as u8;
        bytes.push(b'a' + rem);
        n = (n - 1) / 26;
    }
    bytes.reverse();
    String::from_utf8(bytes).unwrap()
}

fn render_token(token: &Token, matched_name: &str, sequence_n: i64, size: Vec2) -> String {
    match token {
        Token::Literal(s) => s.clone(),
        Token::Name(NameCase::AsIs) => matched_name.to_string(),
        Token::Name(NameCase::Upper) => matched_name.to_uppercase(),
        Token::Name(NameCase::Lower) => matched_name.to_lowercase(),
        Token::Sequence(SequenceStyle::Numeric) => sequence_n.to_string(),
        Token::Sequence(SequenceStyle::Alphabetic) => alphabetic_sequence(sequence_n),
        Token::Size(SizeFormat::Width) => format!("{:.0}", size.x),
        Token::Size(SizeFormat::Height) => format!("{:.0}", size.y),
        Token::Size(SizeFormat::Both) => format!("{:.0}x{:.0}", size.x, size.y),
    }
}

/// One layer's new name: `config.match_find`/`match_replace` applied to
/// `original`, then every token rendered against that result and
/// concatenated. `index` is this layer's position within the batch being
/// renamed (0-based) — `config.sequence_start + index` is the Sequence
/// token's value, so every layer in one Rename All run gets a distinct
/// number/letter even though they're renamed independently.
pub fn compute_name(original: &str, index: usize, size: Vec2, config: &RenameAllConfig) -> String {
    let matched = apply_match(original, config);
    let sequence_n = config.sequence_start + index as i64;
    config.tokens.iter().map(|t| render_token(t, &matched, sequence_n, size)).collect()
}

/// `compute_name` over a whole batch — the preview list and the actual
/// applied rename both go through this, so they can't disagree.
pub fn compute_names(items: &[(LayerId, String, Vec2)], config: &RenameAllConfig) -> Vec<(LayerId, String)> {
    items
        .iter()
        .enumerate()
        .map(|(i, (id, name, size))| (*id, compute_name(name, i, *size, config)))
        .collect()
}

fn token_label(token: &Token) -> String {
    match token {
        Token::Literal(s) => format!("Text: \"{s}\""),
        Token::Name(NameCase::AsIs) => "Name".to_string(),
        Token::Name(NameCase::Upper) => "Name (UPPER)".to_string(),
        Token::Name(NameCase::Lower) => "Name (lower)".to_string(),
        Token::Sequence(SequenceStyle::Numeric) => "Sequence (1, 2, 3, ...)".to_string(),
        Token::Sequence(SequenceStyle::Alphabetic) => "Sequence (a, b, c, ...)".to_string(),
        Token::Size(SizeFormat::Width) => "Size (Width)".to_string(),
        Token::Size(SizeFormat::Height) => "Size (Height)".to_string(),
        Token::Size(SizeFormat::Both) => "Size (WxH)".to_string(),
    }
}

/// Modal-style dialog state, owned by `App` (`app.rs`'s `rename_all` field)
/// so it persists across frames while open. `items` is captured once when
/// the dialog opens (`App`'s Cmd+R/"Rename All" handlers), not re-derived
/// live each frame — same "snapshot the working set at open time" choice
/// `layers_panel.rs`'s inline rename makes for its own single-layer case.
pub struct RenameAllState {
    pub open: bool,
    pub config: RenameAllConfig,
    /// `(id, original name, size)` for *every* layer on the active page
    /// (flattened, including nested descendants), gathered when the dialog
    /// opens — covers both `Scope`s from one snapshot. `Scope::WholePage`'s
    /// filter (and the separate Match find/replace) are applied against
    /// these at render/apply time, not baked in here, so toggling the
    /// filter text updates the preview live.
    pub items: Vec<(LayerId, String, Vec2)>,
    /// The ids that were selected when the dialog opened — `Scope::Selected`
    /// restricts `items` down to these (in this original order, so the
    /// Sequence token numbers them consistently regardless of the page's
    /// own back-to-front layer order).
    pub selected_ids: Vec<LayerId>,
}

impl Default for RenameAllState {
    fn default() -> Self {
        Self { open: false, config: RenameAllConfig::default(), items: Vec::new(), selected_ids: Vec::new() }
    }
}

/// Sketch's "Rename with Last Format" (re-running `config` against a new
/// selection with no dialog shown at all) isn't a variant here — `app.rs`
/// handles it by calling `compute_names` directly against the last-used
/// `RenameAllConfig` it keeps around, bypassing this dialog entirely.
pub enum RenameAllAction {
    Apply(Vec<(LayerId, String)>),
}

/// Draws the dialog if `state.open`; returns `Some(Apply(..))` the frame the
/// user clicks "Rename". Closing/canceling just clears `state.open` and
/// returns `None`.
pub fn ui(ctx: &egui::Context, state: &mut RenameAllState) -> Option<RenameAllAction> {
    if !state.open {
        return None;
    }
    let mut action = None;
    let mut still_open = state.open;
    let mut close_requested = false;
    egui::Window::new("Rename All").open(&mut still_open).resizable(true).show(ctx, |ui| {
        ui.horizontal(|ui| {
            ui.label("Scope:");
            let is_whole_page = matches!(state.config.scope, Scope::WholePage { .. });
            if ui.selectable_label(!is_whole_page, "Selected").clicked() {
                state.config.scope = Scope::Selected;
            }
            if ui.selectable_label(is_whole_page, "Whole Page").clicked() {
                state.config.scope = Scope::WholePage { filter: String::new() };
            }
        });
        if let Scope::WholePage { filter } = &mut state.config.scope {
            ui.horizontal(|ui| {
                ui.label("Filter:");
                ui.text_edit_singleline(filter);
            });
        }

        ui.separator();
        ui.label("Match");
        ui.horizontal(|ui| {
            ui.label("Find:");
            ui.text_edit_singleline(&mut state.config.match_find);
        });
        ui.horizontal(|ui| {
            ui.label("Replace:");
            ui.text_edit_singleline(&mut state.config.match_replace);
        });
        ui.horizontal(|ui| {
            ui.checkbox(&mut state.config.regex, "Regular expression");
            ui.checkbox(&mut state.config.case_sensitive, "Case sensitive");
        });

        ui.separator();
        ui.horizontal(|ui| {
            ui.label("Sequence starts at:");
            ui.add(egui::DragValue::new(&mut state.config.sequence_start));
        });

        ui.separator();
        ui.label("Tokens");
        let mut move_up: Option<usize> = None;
        let mut move_down: Option<usize> = None;
        let mut remove: Option<usize> = None;
        let n_tokens = state.config.tokens.len();
        for (i, token) in state.config.tokens.iter_mut().enumerate() {
            ui.horizontal(|ui| {
                match token {
                    Token::Literal(s) => {
                        ui.label("Text:");
                        ui.text_edit_singleline(s);
                    }
                    Token::Name(case) => {
                        egui::ComboBox::from_id_salt(("rename-token-name", i))
                            .selected_text(token_label(&Token::Name(*case)))
                            .show_ui(ui, |ui| {
                                for c in [NameCase::AsIs, NameCase::Upper, NameCase::Lower] {
                                    ui.selectable_value(case, c, token_label(&Token::Name(c)));
                                }
                            });
                    }
                    Token::Sequence(style) => {
                        egui::ComboBox::from_id_salt(("rename-token-sequence", i))
                            .selected_text(token_label(&Token::Sequence(*style)))
                            .show_ui(ui, |ui| {
                                for s in [SequenceStyle::Numeric, SequenceStyle::Alphabetic] {
                                    ui.selectable_value(style, s, token_label(&Token::Sequence(s)));
                                }
                            });
                    }
                    Token::Size(fmt) => {
                        egui::ComboBox::from_id_salt(("rename-token-size", i))
                            .selected_text(token_label(&Token::Size(*fmt)))
                            .show_ui(ui, |ui| {
                                for f in [SizeFormat::Width, SizeFormat::Height, SizeFormat::Both] {
                                    ui.selectable_value(fmt, f, token_label(&Token::Size(f)));
                                }
                            });
                    }
                }
                if ui.small_button("↑").on_hover_text("Move earlier").clicked() && i > 0 {
                    move_up = Some(i);
                }
                if ui.small_button("↓").on_hover_text("Move later").clicked() && i + 1 < n_tokens {
                    move_down = Some(i);
                }
                if ui.small_button("🗑").on_hover_text("Remove").clicked() {
                    remove = Some(i);
                }
            });
        }
        if let Some(i) = move_up {
            state.config.tokens.swap(i, i - 1);
        }
        if let Some(i) = move_down {
            state.config.tokens.swap(i, i + 1);
        }
        if let Some(i) = remove {
            state.config.tokens.remove(i);
        }
        ui.horizontal(|ui| {
            ui.label("Add:");
            if ui.button("Name").clicked() {
                state.config.tokens.push(Token::Name(NameCase::AsIs));
            }
            if ui.button("Sequence").clicked() {
                state.config.tokens.push(Token::Sequence(SequenceStyle::Numeric));
            }
            if ui.button("Size").clicked() {
                state.config.tokens.push(Token::Size(SizeFormat::Both));
            }
            if ui.button("Text").clicked() {
                state.config.tokens.push(Token::Literal(String::new()));
            }
        });

        ui.separator();
        ui.label("Preview");
        let candidates = filtered_items(&state.items, &state.selected_ids, &state.config.scope);
        let previews = compute_names(&candidates, &state.config);
        egui::ScrollArea::vertical().max_height(160.0).show(ui, |ui| {
            for ((_, old_name, _), (_, new_name)) in candidates.iter().zip(previews.iter()) {
                ui.label(format!("{old_name}  →  {new_name}"));
            }
        });

        ui.separator();
        ui.horizontal(|ui| {
            if ui.add_enabled(!candidates.is_empty(), egui::Button::new("Rename")).clicked() {
                action = Some(RenameAllAction::Apply(previews));
                close_requested = true;
            }
            if ui.button("Cancel").clicked() {
                close_requested = true;
            }
        });
    });
    state.open = still_open && !close_requested;
    action
}

fn filtered_items(items: &[(LayerId, String, Vec2)], selected_ids: &[LayerId], scope: &Scope) -> Vec<(LayerId, String, Vec2)> {
    match scope {
        Scope::Selected => selected_ids.iter().filter_map(|id| items.iter().find(|(iid, _, _)| iid == id).cloned()).collect(),
        Scope::WholePage { filter } => {
            if filter.is_empty() {
                items.to_vec()
            } else {
                let filter_lower = filter.to_lowercase();
                items.iter().filter(|(_, name, _)| name.to_lowercase().contains(&filter_lower)).cloned().collect()
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    #[test]
    fn plain_name_token_passes_through_unchanged() {
        let config = RenameAllConfig { tokens: vec![Token::Name(NameCase::AsIs)], ..RenameAllConfig::default() };
        assert_eq!(compute_name("Rectangle", 0, Vec2::ZERO, &config), "Rectangle");
    }

    #[test]
    fn sequence_token_increments_per_index_from_the_configured_start() {
        let config = RenameAllConfig { tokens: vec![Token::Sequence(SequenceStyle::Numeric)], sequence_start: 5, ..RenameAllConfig::default() };
        assert_eq!(compute_name("X", 0, Vec2::ZERO, &config), "5");
        assert_eq!(compute_name("X", 1, Vec2::ZERO, &config), "6");
    }

    #[test]
    fn alphabetic_sequence_wraps_past_z_into_aa() {
        assert_eq!(alphabetic_sequence(1), "a");
        assert_eq!(alphabetic_sequence(26), "z");
        assert_eq!(alphabetic_sequence(27), "aa");
        assert_eq!(alphabetic_sequence(28), "ab");
    }

    #[test]
    fn size_token_formats_dimensions() {
        let config = RenameAllConfig { tokens: vec![Token::Size(SizeFormat::Both)], ..RenameAllConfig::default() };
        assert_eq!(compute_name("X", 0, Vec2::new(100.0, 50.0), &config), "100x50");
    }

    #[test]
    fn match_find_replace_applies_before_tokens_plain_mode() {
        let config = RenameAllConfig {
            match_find: "Rect".to_string(),
            match_replace: "Square".to_string(),
            tokens: vec![Token::Name(NameCase::AsIs)],
            ..RenameAllConfig::default()
        };
        assert_eq!(compute_name("Rectangle", 0, Vec2::ZERO, &config), "Squareangle");
    }

    #[test]
    fn match_find_replace_case_insensitive_by_default() {
        let config = RenameAllConfig {
            match_find: "rect".to_string(),
            match_replace: "X".to_string(),
            tokens: vec![Token::Name(NameCase::AsIs)],
            ..RenameAllConfig::default()
        };
        assert_eq!(compute_name("Rectangle", 0, Vec2::ZERO, &config), "Xangle");
    }

    #[test]
    fn match_find_replace_regex_mode() {
        let config = RenameAllConfig {
            match_find: r"\d+".to_string(),
            match_replace: "#".to_string(),
            regex: true,
            tokens: vec![Token::Name(NameCase::AsIs)],
            ..RenameAllConfig::default()
        };
        assert_eq!(compute_name("Layer 123", 0, Vec2::ZERO, &config), "Layer #");
    }

    #[test]
    fn name_case_tokens_transform_case() {
        let upper = RenameAllConfig { tokens: vec![Token::Name(NameCase::Upper)], ..RenameAllConfig::default() };
        let lower = RenameAllConfig { tokens: vec![Token::Name(NameCase::Lower)], ..RenameAllConfig::default() };
        assert_eq!(compute_name("MixedCase", 0, Vec2::ZERO, &upper), "MIXEDCASE");
        assert_eq!(compute_name("MixedCase", 0, Vec2::ZERO, &lower), "mixedcase");
    }

    #[test]
    fn filtered_items_whole_page_keeps_only_matching_names() {
        let items = vec![
            (Uuid::new_v4(), "Icon A".to_string(), Vec2::ZERO),
            (Uuid::new_v4(), "Shape B".to_string(), Vec2::ZERO),
        ];
        let scope = Scope::WholePage { filter: "icon".to_string() };
        let filtered = filtered_items(&items, &[], &scope);
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].1, "Icon A");
    }

    #[test]
    fn filtered_items_selected_scope_uses_selected_ids_in_their_own_order() {
        let id_a = Uuid::new_v4();
        let id_b = Uuid::new_v4();
        let items = vec![(id_a, "A".to_string(), Vec2::ZERO), (id_b, "B".to_string(), Vec2::ZERO)];
        // Selected order is B then A, opposite of `items`' own order.
        let filtered = filtered_items(&items, &[id_b, id_a], &Scope::Selected);
        assert_eq!(filtered.iter().map(|(_, n, _)| n.as_str()).collect::<Vec<_>>(), vec!["B", "A"]);
    }

    #[test]
    fn compute_names_zips_ids_with_computed_names() {
        let id_a = Uuid::new_v4();
        let id_b = Uuid::new_v4();
        let items = vec![(id_a, "A".to_string(), Vec2::ZERO), (id_b, "B".to_string(), Vec2::ZERO)];
        let config = RenameAllConfig { tokens: vec![Token::Sequence(SequenceStyle::Numeric)], ..RenameAllConfig::default() };
        let result = compute_names(&items, &config);
        assert_eq!(result, vec![(id_a, "1".to_string()), (id_b, "2".to_string())]);
    }
}
