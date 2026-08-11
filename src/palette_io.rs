//! Load/save `Palette` as Aseprite's palette file format: GIMP Palette
//! (`.gpl`) extended with a `Channels: RGBA` header and a per-row alpha
//! column, per Aseprite's own spec
//! (github.com/aseprite/aseprite/blob/main/docs/gpl-palette-extension.md).
//! Plain (non-alpha) `.gpl` files load fine too — rows are just `R G B Name`,
//! alpha defaults to opaque.

use std::path::Path;

use egui::Color32;

use crate::model::{Palette, PaletteColor};

const MAGIC: &str = "GIMP Palette";

/// Loads a `.gpl` palette file (plain GIMP or Aseprite's RGBA extension) from disk.
pub fn load_gpl(path: &Path) -> anyhow::Result<Palette> {
    let text = std::fs::read_to_string(path)?;
    let mut lines = text.lines();
    let magic = lines.next().unwrap_or("").trim();
    anyhow::ensure!(magic.eq_ignore_ascii_case(MAGIC), "not a GIMP palette file: {}", path.display());

    let mut colors = Vec::new();
    for line in lines {
        let trimmed = line.trim();
        // Header lines (`Name:`, `Columns:`, `Channels:`) and comments
        // (`#...`) never start with a digit — every actual color row does
        // (its leading R value), so this alone disambiguates them without
        // caring what order the header lines came in.
        if trimmed.chars().next().is_none_or(|c| !c.is_ascii_digit()) {
            continue;
        }
        if let Some(color) = parse_color_line(trimmed) {
            colors.push(color);
        }
    }
    Ok(Palette(colors))
}

fn parse_color_line(line: &str) -> Option<PaletteColor> {
    let mut tokens = line.split_whitespace();
    let r: u8 = tokens.next()?.parse().ok()?;
    let g: u8 = tokens.next()?.parse().ok()?;
    let b: u8 = tokens.next()?.parse().ok()?;
    let rest: Vec<&str> = tokens.collect();
    // Aseprite's `Channels: RGBA` extension inserts a 4th numeric column
    // (alpha) before the name; a plain GIMP palette has no alpha column and
    // the 4th token, if any, starts the name straight away.
    let (a, name) = match rest.first().and_then(|t| t.parse::<u8>().ok()) {
        Some(a) => (a, rest[1..].join(" ")),
        None => (255, rest.join(" ")),
    };
    Some(PaletteColor { color: Color32::from_rgba_unmultiplied(r, g, b, a), name })
}

/// Writes `palette` to disk as an Aseprite-compatible RGBA `.gpl` file, labeled `name`.
pub fn save_gpl(path: &Path, palette: &Palette, name: &str) -> anyhow::Result<()> {
    let mut out = String::new();
    out.push_str(MAGIC);
    out.push('\n');
    out.push_str(&format!("Name: {name}\n"));
    out.push_str("Columns: 0\n");
    out.push_str("Channels: RGBA\n");
    out.push_str("#\n");
    for swatch in &palette.0 {
        let [r, g, b, a] = swatch.color.to_srgba_unmultiplied();
        let label = if swatch.name.is_empty() { "Untitled" } else { &swatch.name };
        out.push_str(&format!("{r:3} {g:3} {b:3} {a:3} {label}\n"));
    }
    std::fs::write(path, out)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_a_palette_through_aseprite_rgba_gpl() {
        let mut palette = Palette(Vec::new());
        palette.add(Color32::from_rgba_unmultiplied(254, 91, 89, 255), "Red".to_string());
        palette.add(Color32::from_rgba_unmultiplied(0, 0, 0, 0), "Transparent".to_string());

        let path = std::env::temp_dir().join(format!("simple-design-test-{}.gpl", uuid::Uuid::new_v4()));
        save_gpl(&path, &palette, "Test Palette").expect("save");
        let loaded = load_gpl(&path).expect("load");
        std::fs::remove_file(&path).ok();

        assert_eq!(loaded, palette);
    }

    #[test]
    fn loads_a_plain_non_alpha_gpl_file_defaulting_to_opaque() {
        let path = std::env::temp_dir().join(format!("simple-design-test-{}.gpl", uuid::Uuid::new_v4()));
        std::fs::write(
            &path,
            "GIMP Palette\nName: Plain\n#\n255   0   0 Red\n  0 255   0 Green\n",
        )
        .unwrap();
        let loaded = load_gpl(&path).expect("load");
        std::fs::remove_file(&path).ok();

        assert_eq!(
            loaded,
            Palette(vec![
                PaletteColor { color: Color32::from_rgb(255, 0, 0), name: "Red".to_string() },
                PaletteColor { color: Color32::from_rgb(0, 255, 0), name: "Green".to_string() },
            ])
        );
    }

    #[test]
    fn rejects_a_file_without_the_gimp_palette_magic_header() {
        let path = std::env::temp_dir().join(format!("simple-design-test-{}.gpl", uuid::Uuid::new_v4()));
        std::fs::write(&path, "Not A Palette\n255 0 0 Red\n").unwrap();
        let result = load_gpl(&path);
        std::fs::remove_file(&path).ok();

        assert!(result.is_err());
    }
}
