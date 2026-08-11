use egui::Color32;
use serde::{Deserialize, Serialize};

/// One swatch. `name` mirrors the GIMP/Aseprite `.gpl` palette format's
/// per-color name column — empty for swatches that were never named (see
/// `crate::palette_io`).
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct PaletteColor {
    pub color: Color32,
    pub name: String,
}

/// A document's color swatch library — Aseprite's "Palette" panel
/// equivalent. Stored on `Document::palette` and edited via
/// `ui/palette_panel.rs`; `crate::palette_io` loads/saves it as an
/// Aseprite-compatible `.gpl` file.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Palette(pub Vec<PaletteColor>);

/// Richard "DawnBringer" Fhager's DB32 palette — the same 32 colors Aseprite
/// seeds every new sprite's palette with by default.
const DB32_HEX: [&str; 32] = [
    "000000", "222034", "45283c", "663931", "8f563b", "df7126", "d9a066", "eec39a", "fbf236", "99e550", "6abe30",
    "37946e", "4b692f", "524b24", "323c39", "3f3f74", "306082", "5b6ee1", "639bff", "5fcde4", "cbdbfc", "ffffff",
    "9badb7", "847e87", "696a6a", "595652", "76428a", "ac3232", "d95763", "d77bba", "8f974a", "8a6f30",
];

impl Palette {
    /// Builds a fresh palette seeded with the 32-color DawnBringer DB32 set.
    pub fn db32() -> Self {
        Self(DB32_HEX.iter().map(|hex| PaletteColor { color: hex_to_color32(hex), name: String::new() }).collect())
    }

    /// Appends a new swatch to the end of the palette.
    pub fn add(&mut self, color: Color32, name: String) {
        self.0.push(PaletteColor { color, name });
    }

    /// Removes the swatch at `index`, if it exists. No-op if out of bounds.
    pub fn remove(&mut self, index: usize) {
        if index < self.0.len() {
            self.0.remove(index);
        }
    }
}

impl Default for Palette {
    fn default() -> Self {
        Self::db32()
    }
}

fn hex_to_color32(hex: &str) -> Color32 {
    let r = u8::from_str_radix(&hex[0..2], 16).unwrap();
    let g = u8::from_str_radix(&hex[2..4], 16).unwrap();
    let b = u8::from_str_radix(&hex[4..6], 16).unwrap();
    Color32::from_rgb(r, g, b)
}
