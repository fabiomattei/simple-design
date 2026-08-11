//! Bundled non-default text fonts (see `TextFont` in `model/layer.rs`).
//! `Proportional`/`Monospace` reuse egui's own built-in default families
//! (`epaint_default_fonts`'s `Ubuntu-Light`/`Hack-Regular`) and need no setup
//! here. These three are extra families registered once at startup
//! (`install_fonts`, called from `main.rs`) so the canvas render (egui
//! `Galley`s) and the PNG export / outline-conversion rasterizers (raw
//! `ab_glyph` bytes below) draw from the exact same font files, matching the
//! convention the two built-ins already follow. A fourth tier,
//! `TextFont::System`, is handled separately by `system_fonts.rs` since it's
//! resolved at render time against whatever's actually installed rather
//! than bundled in the binary.
//!
//! Sourced from Google Fonts, SIL Open Font License — see the `OFL-*.txt`
//! files alongside the `.ttf`s in `assets/fonts/`. Each was subset to Latin
//! + common punctuation/symbols and, for the two that ship as variable
//! fonts upstream, instanced down to a single static weight to keep the
//! bundled binary size in line with the two built-in fonts.
//!
//! ## Bold
//!
//! egui 0.36's `FontId` has no weight field at all — `TextFormat.italics`
//! is a real flag epaint honors, but bold only exists as a distinct
//! `FontFamily` pointing at a separately-registered bold-weight font file.
//! So every one of the five built-in tiers above also gets a bundled bold
//! weight here (`*_BOLD_FAMILY`/`*_BOLD_REGULAR`) — including `Proportional`
//! and `Monospace`, whose egui-bundled regular weights (`Ubuntu-Light`,
//! `Hack-Regular`) have no bold counterpart in `epaint_default_fonts`, so
//! `Ubuntu-Bold.ttf`/`Hack-Bold.ttf` are bundled here too, sourced the same
//! way as the other three. `ab_glyph_bytes_bold` mirrors `ab_glyph_bytes`
//! for the rasterizer paths; `TextFont::System`'s bold lookup
//! (`system_fonts::family_data_bold`) queries the OS for a genuine bold
//! face and falls back to the regular face if the family doesn't have one
//! — a run marked bold in a system font with no bold face just doesn't
//! visually thicken, same graceful-degradation precedent as a `System`
//! font that isn't installed at all.

use std::borrow::Cow;

use egui::{FontData, FontDefinitions, FontFamily};

use crate::model::TextFont;

pub const SERIF_REGULAR: &[u8] = include_bytes!("../assets/fonts/Merriweather-Regular.ttf");
pub const DISPLAY_REGULAR: &[u8] = include_bytes!("../assets/fonts/Poppins-Regular.ttf");
pub const HANDWRITING_REGULAR: &[u8] = include_bytes!("../assets/fonts/Caveat-Regular.ttf");

pub const SERIF_FAMILY: &str = "Serif";
pub const DISPLAY_FAMILY: &str = "Display";
pub const HANDWRITING_FAMILY: &str = "Handwriting";

pub const PROPORTIONAL_BOLD_REGULAR: &[u8] = include_bytes!("../assets/fonts/Ubuntu-Bold.ttf");
pub const MONOSPACE_BOLD_REGULAR: &[u8] = include_bytes!("../assets/fonts/Hack-Bold.ttf");
pub const SERIF_BOLD_REGULAR: &[u8] = include_bytes!("../assets/fonts/Merriweather-Bold.ttf");
pub const DISPLAY_BOLD_REGULAR: &[u8] = include_bytes!("../assets/fonts/Poppins-Bold.ttf");
pub const HANDWRITING_BOLD_REGULAR: &[u8] = include_bytes!("../assets/fonts/Caveat-Bold.ttf");

pub const PROPORTIONAL_BOLD_FAMILY: &str = "Sans Bold";
pub const MONOSPACE_BOLD_FAMILY: &str = "Mono Bold";
pub const SERIF_BOLD_FAMILY: &str = "Serif Bold";
pub const DISPLAY_BOLD_FAMILY: &str = "Display Bold";
pub const HANDWRITING_BOLD_FAMILY: &str = "Handwriting Bold";

/// Registers the extra font families above with the egui context. Must run
/// once at startup, before any `Text` layer using a bundled non-default
/// family (`Serif`/`Display`/`Handwriting`, or any per-run bold text — see
/// this module's "Bold" doc section) is drawn on the canvas (`export.rs`/
/// `text_outline.rs` don't need this — they load the raw bytes directly).
pub fn install_fonts(ctx: &egui::Context) {
    let mut fonts = FontDefinitions::default();

    let mut insert = |key: &str, bytes: &'static [u8], family: &str| {
        fonts
            .font_data
            .insert(key.to_owned(), std::sync::Arc::new(FontData::from_static(bytes)));
        fonts
            .families
            .insert(FontFamily::Name(family.into()), vec![key.to_owned()]);
    };

    insert("serif", SERIF_REGULAR, SERIF_FAMILY);
    insert("display", DISPLAY_REGULAR, DISPLAY_FAMILY);
    insert("handwriting", HANDWRITING_REGULAR, HANDWRITING_FAMILY);
    insert("proportional-bold", PROPORTIONAL_BOLD_REGULAR, PROPORTIONAL_BOLD_FAMILY);
    insert("monospace-bold", MONOSPACE_BOLD_REGULAR, MONOSPACE_BOLD_FAMILY);
    insert("serif-bold", SERIF_BOLD_REGULAR, SERIF_BOLD_FAMILY);
    insert("display-bold", DISPLAY_BOLD_REGULAR, DISPLAY_BOLD_FAMILY);
    insert("handwriting-bold", HANDWRITING_BOLD_REGULAR, HANDWRITING_BOLD_FAMILY);

    ctx.set_fonts(fonts);
}

/// Raw font bytes + face index for any `TextFont`, for `ab_glyph`'s
/// rasterizer (`export.rs`, `text_outline.rs`) — these never go through
/// egui's font registry at all, so they need the bytes directly rather than
/// a `FontFamily` name. A `TextFont::System` naming a family that isn't
/// installed on this machine falls back to `Proportional`'s bytes, matching
/// `system_fonts::resolve_family`'s fallback for the interactive canvas.
pub fn ab_glyph_bytes(font: &TextFont) -> (Cow<'static, [u8]>, u32) {
    match font {
        TextFont::Proportional => (Cow::Borrowed(epaint_default_fonts::UBUNTU_LIGHT), 0),
        TextFont::Monospace => (Cow::Borrowed(epaint_default_fonts::HACK_REGULAR), 0),
        TextFont::Serif => (Cow::Borrowed(SERIF_REGULAR), 0),
        TextFont::Display => (Cow::Borrowed(DISPLAY_REGULAR), 0),
        TextFont::Handwriting => (Cow::Borrowed(HANDWRITING_REGULAR), 0),
        TextFont::System(name) => match crate::system_fonts::family_data(name) {
            Some((bytes, index)) => (Cow::Owned(bytes), index),
            None => (Cow::Borrowed(epaint_default_fonts::UBUNTU_LIGHT), 0),
        },
    }
}

/// Bold-weight counterpart to `ab_glyph_bytes` — see this module's "Bold"
/// doc section. Always returns *some* usable bytes (a `System` font with no
/// bold face falls back to its own regular face, not `Proportional`'s —
/// unlike `ab_glyph_bytes`'s not-installed fallback, the family itself is
/// known to exist here, just not in a bold weight).
pub fn ab_glyph_bytes_bold(font: &TextFont) -> (Cow<'static, [u8]>, u32) {
    match font {
        TextFont::Proportional => (Cow::Borrowed(PROPORTIONAL_BOLD_REGULAR), 0),
        TextFont::Monospace => (Cow::Borrowed(MONOSPACE_BOLD_REGULAR), 0),
        TextFont::Serif => (Cow::Borrowed(SERIF_BOLD_REGULAR), 0),
        TextFont::Display => (Cow::Borrowed(DISPLAY_BOLD_REGULAR), 0),
        TextFont::Handwriting => (Cow::Borrowed(HANDWRITING_BOLD_REGULAR), 0),
        TextFont::System(name) => match crate::system_fonts::family_data_bold(name) {
            Some((bytes, index)) => (Cow::Owned(bytes), index),
            None => ab_glyph_bytes(font),
        },
    }
}
