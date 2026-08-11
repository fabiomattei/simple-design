use egui::Vec2;

/// A named fixed size offered when creating an Artboard, e.g. "A4" or
/// "Desktop" — see `SCREEN_PRESETS`/`PAPER_PRESETS` and `ui/inspector.rs`'s
/// empty-selection-plus-`Tool::Artboard` branch, which renders these as
/// buttons.
pub struct ArtboardPreset {
    pub name: &'static str,
    pub size: Vec2,
}

/// Common device viewport sizes, in pixels.
pub const SCREEN_PRESETS: &[ArtboardPreset] = &[
    ArtboardPreset { name: "Desktop", size: Vec2::new(1440.0, 900.0) },
    ArtboardPreset { name: "Web", size: Vec2::new(1920.0, 1080.0) },
    ArtboardPreset { name: "Tablet", size: Vec2::new(768.0, 1024.0) },
    ArtboardPreset { name: "Phone", size: Vec2::new(375.0, 812.0) },
];

/// ISO 216 "A" paper sizes, in points at 72 points/inch — the standard
/// convention design tools use, so these match an A4 artboard created there.
pub const PAPER_PRESETS: &[ArtboardPreset] = &[
    ArtboardPreset { name: "A3", size: Vec2::new(842.0, 1191.0) },
    ArtboardPreset { name: "A4", size: Vec2::new(595.0, 842.0) },
    ArtboardPreset { name: "A5", size: Vec2::new(420.0, 595.0) },
    ArtboardPreset { name: "A6", size: Vec2::new(298.0, 420.0) },
];
