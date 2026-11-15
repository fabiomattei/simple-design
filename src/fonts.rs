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
//! files alongside the `.ttf`s in `assets/fonts/`. Each was subset to
//! Latin plus common punctuation/symbols and, for the two that ship as variable
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
/// this module's "Bold" doc section) is drawn on the canvas (`export/`/
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

/// `ab_glyph::PxScale` for `font_size` taken as the font's *em* size — the
/// convention egui (the canvas renderer) and design tools use — rather than
/// `PxScale::from(font_size)`, which ab_glyph takes as the ascent-to-descent
/// height and so draws glyphs smaller by `height / em` (measured up to 1.4x
/// for `TextFont::Display`). Every `ab_glyph` text path (`export/`,
/// `text_outline.rs`) goes through this so their text matches the canvas.
/// Same conversion as `ab_glyph::Font::pt_to_px_scale`, minus its pt-to-px
/// factor; a font with no `units_per_em` falls back to the height convention.
pub fn em_px_scale(face: &impl ab_glyph::Font, font_size: f32) -> ab_glyph::PxScale {
    match face.units_per_em() {
        Some(units_per_em) => ab_glyph::PxScale::from(font_size * face.height_unscaled() / units_per_em),
        None => ab_glyph::PxScale::from(font_size),
    }
}

/// Raw font bytes + face index for any `TextFont`, for `ab_glyph`'s
/// rasterizer (`export/`, `text_outline.rs`) — these never go through
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

/// One `(bytes, face_index)` per style source a rich-text render needs to
/// build an `ab_glyph::FontRef` from — `[0]` = `base`, `[i + 1]` =
/// `runs[i]` — picking each source's bold or regular bytes per its own
/// `RunStyle::bold`. Factored out of `export/text_rich.rs`'s `draw_text_rich` and
/// `text_outline.rs`'s `convert_to_outlines_rich` (which used to duplicate
/// this exact selection logic) since `export/`'s new `draw_text_in_area`
/// needs the identical resolution a third time — callers still build their
/// own `Vec<FontRef>` from the returned bytes locally (a `FontRef` borrows
/// from its bytes, so it can't be returned from here without also returning
/// the bytes it'd borrow from, at which point returning the bytes alone and
/// letting each caller parse them is simpler).
pub fn run_font_byte_bufs(base: &crate::model::text_runs::RunStyle, runs: &[crate::model::text_runs::TextRun]) -> Vec<(Cow<'static, [u8]>, u32)> {
    let mut byte_bufs = Vec::with_capacity(runs.len() + 1);
    byte_bufs.push(if base.bold { ab_glyph_bytes_bold(&base.font) } else { ab_glyph_bytes(&base.font) });
    for run in runs {
        byte_bufs.push(if run.style.bold { ab_glyph_bytes_bold(&run.style.font) } else { ab_glyph_bytes(&run.style.font) });
    }
    byte_bufs
}

#[cfg(test)]
mod tests {
    use ab_glyph::{FontRef, ScaleFont};

    use crate::model::{ListType, TextAlign, TextFont, TextTransform};
    use crate::text_layout::{layout_paragraphs, TextStyleParams};

    /// The canvas (egui) and every `ab_glyph` text path size a font the
    /// same way (`em_px_scale`): for each bundled font, the same string's
    /// width and line height agree within 1% (egui rounds metrics to whole
    /// pixels; Serif's kerning differs slightly between the two).
    #[test]
    fn em_px_scale_matches_the_canvas_text_size_for_every_bundled_font() {
        let ctx = egui::Context::default();
        super::install_fonts(&ctx);
        ctx.run_ui(Default::default(), |_| {}).drop_without_applying_deltas();
        let text = "The quick brown fox jumps over the lazy dog";
        let mut checked = 0;
        ctx.run_ui(Default::default(), |_| {
            for font in [TextFont::Proportional, TextFont::Monospace, TextFont::Serif, TextFont::Display, TextFont::Handwriting] {
                let style = TextStyleParams {
                    font: font.clone(),
                    font_size: 48.0,
                    align: TextAlign::Left,
                    letter_spacing: 0.0,
                    line_height: None,
                    italic: false,
                    underline: false,
                    strikethrough: false,
                    transform: TextTransform::None,
                    list: ListType::None,
                    list_start: 1,
                };
                let galley = &layout_paragraphs(&ctx, text, &style, 1.0, egui::Color32::BLACK, f32::INFINITY)[0];

                let (bytes, index) = super::ab_glyph_bytes(&font);
                let face = FontRef::try_from_slice_and_index(&bytes, index).unwrap();
                let scaled = ab_glyph::Font::as_scaled(&face, super::em_px_scale(&face, 48.0));
                let mut width = 0.0;
                let mut prev = None;
                for c in text.chars() {
                    let id = scaled.glyph_id(c);
                    if let Some(p) = prev {
                        width += scaled.kern(p, id);
                    }
                    width += scaled.h_advance(id);
                    prev = Some(id);
                }
                let line_height = scaled.height() + scaled.line_gap();

                let close = |a: f32, b: f32| (a - b).abs() / b < 0.01;
                assert!(close(width, galley.rect.width()), "{font:?}: width export {width} vs canvas {}", galley.rect.width());
                assert!(close(line_height, galley.rect.height()), "{font:?}: line height export {line_height} vs canvas {}", galley.rect.height());
                checked += 1;
            }
        })
        .drop_without_applying_deltas();
        assert_eq!(checked, 5);
    }
}
