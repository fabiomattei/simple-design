//! Which window side each dockable panel (Layers, Palette, Inspector,
//! Minimap) is docked on, kept in `panels.json` in the app's config folder
//! (see `config_dir.rs`) so it survives restarts. Same error policy as
//! `recent_files.rs`: a missing or unreadable file falls back to the default
//! (everything on the right), and a failed write is ignored — losing the
//! layout is never worth interrupting the user.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum DockSide {
    Left,
    Right,
}

impl DockSide {
    pub fn opposite(self) -> Self {
        match self {
            Self::Left => Self::Right,
            Self::Right => Self::Left,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PanelKind {
    Layers,
    Palette,
    Inspector,
    Minimap,
}

impl PanelKind {
    /// Top-to-bottom order, both within a side's column and on its icon rail.
    pub const ALL: [PanelKind; 4] = [Self::Layers, Self::Palette, Self::Inspector, Self::Minimap];

    pub fn label(self) -> &'static str {
        match self {
            Self::Layers => "Layers",
            Self::Palette => "Palette",
            Self::Inspector => "Inspector",
            Self::Minimap => "Minimap",
        }
    }
}

/// `#[serde(default)]` so a `panels.json` written before a panel existed
/// still loads, with that panel on its default side.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct PanelLayout {
    layers: DockSide,
    palette: DockSide,
    inspector: DockSide,
    minimap: DockSide,
}

impl Default for PanelLayout {
    fn default() -> Self {
        Self { layers: DockSide::Right, palette: DockSide::Right, inspector: DockSide::Right, minimap: DockSide::Right }
    }
}

impl PanelLayout {
    pub fn load() -> Self {
        storage_path()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    pub fn side(&self, panel: PanelKind) -> DockSide {
        match panel {
            PanelKind::Layers => self.layers,
            PanelKind::Palette => self.palette,
            PanelKind::Inspector => self.inspector,
            PanelKind::Minimap => self.minimap,
        }
    }

    /// Docks `panel` on `side` and persists the whole layout.
    pub fn set_side(&mut self, panel: PanelKind, side: DockSide) {
        match panel {
            PanelKind::Layers => self.layers = side,
            PanelKind::Palette => self.palette = side,
            PanelKind::Inspector => self.inspector = side,
            PanelKind::Minimap => self.minimap = side,
        }
        self.store();
    }

    fn store(&self) {
        let Some(file) = storage_path() else { return };
        if let Some(parent) = file.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Ok(json) = serde_json::to_string_pretty(self) {
            let _ = std::fs::write(file, json);
        }
    }
}

fn storage_path() -> Option<PathBuf> {
    Some(crate::config_dir::config_dir()?.join("panels.json"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layout_round_trips_through_json() {
        let layout = PanelLayout { layers: DockSide::Left, ..PanelLayout::default() };
        let json = serde_json::to_string(&layout).unwrap();
        assert_eq!(serde_json::from_str::<PanelLayout>(&json).unwrap(), layout);
    }

    #[test]
    fn a_panel_missing_from_the_file_defaults_to_the_right() {
        let layout: PanelLayout = serde_json::from_str(r#"{ "layers": "Left" }"#).unwrap();
        assert_eq!(layout.side(PanelKind::Layers), DockSide::Left);
        assert_eq!(layout.side(PanelKind::Inspector), DockSide::Right);
    }
}
