//! The bundled font files an HTML export ships in `fonts/`, plus their
//! `@font-face` rules, so the browser draws text with the same faces as the
//! canvas. System fonts aren't shipped: the viewer's machine may or may not
//! have them, the same as for a document opened on another computer.

use crate::css_rules::bundled_family_name;
use crate::model::TextFont;

/// One font file to ship. `path` is relative to the folder holding
/// `index.html`.
pub struct FontFile {
    pub path: String,
    pub bytes: &'static [u8],
    pub face_rule: String,
}

/// The file for `font` at regular or bold weight, or `None` for a system
/// font. Same bytes `fonts::ab_glyph_bytes`/`ab_glyph_bytes_bold` render
/// with, so export and canvas agree.
pub fn bundled_font_file(font: &TextFont, bold: bool) -> Option<FontFile> {
    let family = bundled_family_name(font)?;
    let (file_name, bytes): (&str, &'static [u8]) = match (font, bold) {
        (TextFont::Proportional, false) => ("Ubuntu-Light.ttf", epaint_default_fonts::UBUNTU_LIGHT),
        (TextFont::Proportional, true) => ("Ubuntu-Bold.ttf", crate::fonts::PROPORTIONAL_BOLD_REGULAR),
        (TextFont::Monospace, false) => ("Hack-Regular.ttf", epaint_default_fonts::HACK_REGULAR),
        (TextFont::Monospace, true) => ("Hack-Bold.ttf", crate::fonts::MONOSPACE_BOLD_REGULAR),
        (TextFont::Serif, false) => ("Merriweather-Regular.ttf", crate::fonts::SERIF_REGULAR),
        (TextFont::Serif, true) => ("Merriweather-Bold.ttf", crate::fonts::SERIF_BOLD_REGULAR),
        (TextFont::Display, false) => ("Poppins-Regular.ttf", crate::fonts::DISPLAY_REGULAR),
        (TextFont::Display, true) => ("Poppins-Bold.ttf", crate::fonts::DISPLAY_BOLD_REGULAR),
        (TextFont::Handwriting, false) => ("Caveat-Regular.ttf", crate::fonts::HANDWRITING_REGULAR),
        (TextFont::Handwriting, true) => ("Caveat-Bold.ttf", crate::fonts::HANDWRITING_BOLD_REGULAR),
        (TextFont::System(_), _) => return None,
    };
    let path = format!("fonts/{file_name}");
    let weight = if bold { "bold" } else { "normal" };
    let face_rule = format!("@font-face {{ font-family: \"{family}\"; src: url(\"{path}\") format(\"truetype\"); font-weight: {weight}; }}");
    Some(FontFile { path, bytes, face_rule })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bold_serif_maps_to_merriweather_bold() {
        let file = bundled_font_file(&TextFont::Serif, true).unwrap();
        assert_eq!(file.path, "fonts/Merriweather-Bold.ttf");
        assert_eq!(file.face_rule, "@font-face { font-family: \"Merriweather\"; src: url(\"fonts/Merriweather-Bold.ttf\") format(\"truetype\"); font-weight: bold; }");
    }

    #[test]
    fn system_fonts_are_not_shipped() {
        assert!(bundled_font_file(&TextFont::System("Helvetica".to_string()), false).is_none());
    }
}
