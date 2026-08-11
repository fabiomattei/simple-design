use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::guide::Guide;
use super::layer::{Layer, LayerId, LayerStyle, TextStyle};
use super::palette::Palette;

pub type PageId = Uuid;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Page {
    pub id: PageId,
    pub name: String,
    pub layers: Vec<Layer>,
    /// User-placed ruler guides. Absent in older save files, hence `default`.
    #[serde(default)]
    pub guides: Vec<Guide>,
}

impl Page {
    /// Creates an empty page with a fresh id and no layers or guides.
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            id: Uuid::new_v4(),
            name: name.into(),
            layers: Vec::new(),
            guides: Vec::new(),
        }
    }

    /// Recursively searches this page's layer tree for `id`, returning a reference if found.
    pub fn find(&self, id: LayerId) -> Option<&Layer> {
        self.layers.iter().find_map(|l| l.find(id))
    }

    /// Recursively searches this page's layer tree for `id`, returning a mutable reference if found.
    pub fn find_mut(&mut self, id: LayerId) -> Option<&mut Layer> {
        self.layers.iter_mut().find_map(|l| l.find_mut(id))
    }

    /// Removes a top-level or nested layer by id, returning it if found.
    pub fn remove(&mut self, id: LayerId) -> Option<Layer> {
        if let Some(pos) = self.layers.iter().position(|l| l.id == id) {
            return Some(self.layers.remove(pos));
        }
        for layer in &mut self.layers {
            if let Some(found) = layer.remove(id) {
                return Some(found);
            }
        }
        None
    }

    /// Every layer id on this page, including nested descendants — used to
    /// evict stale entries from per-layer runtime caches (e.g. the canvas's
    /// image texture cache) that aren't part of the serialized document.
    pub fn all_layer_ids(&self) -> std::collections::HashSet<LayerId> {
        fn walk(layers: &[Layer], out: &mut std::collections::HashSet<LayerId>) {
            for layer in layers {
                out.insert(layer.id);
                if let Some(children) = layer.kind.children() {
                    walk(children, out);
                }
            }
        }
        let mut out = std::collections::HashSet::new();
        walk(&self.layers, &mut out);
        out
    }

    /// Returns the accumulated offset of all ancestors of `id`, i.e. the
    /// position that must be added to a child's frame to get page-space coordinates.
    pub fn absolute_offset(&self, id: LayerId) -> Option<egui::Vec2> {
        fn walk(layers: &[Layer], id: LayerId, offset: egui::Vec2) -> Option<egui::Vec2> {
            for layer in layers {
                if layer.id == id {
                    return Some(offset);
                }
                if let Some(children) = layer.kind.children() {
                    let child_offset = offset + layer.frame.pos.to_vec2();
                    if let Some(found) = walk(children, id, child_offset) {
                        return Some(found);
                    }
                }
            }
            None
        }
        walk(&self.layers, id, egui::Vec2::ZERO)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Document {
    pub name: String,
    pub pages: Vec<Page>,
    pub active_page: usize,
    /// Shared/linked Text Styles library, applied
    /// to `Text` layers via `LayerKind::Text::style_id`. Absent in older
    /// save files, hence `default`.
    #[serde(default)]
    pub text_styles: Vec<TextStyle>,
    /// Shared/linked Layer Styles library, applied to any
    /// layer via `Layer::style_id`. Absent in older save files, hence
    /// `default`.
    #[serde(default)]
    pub layer_styles: Vec<LayerStyle>,
    /// Color swatch library — Aseprite's "Palette" panel equivalent (see
    /// `crate::palette_io` for `.gpl` file compatibility). Absent in older
    /// save files, hence `default` (falls back to `Palette::default()`,
    /// DawnBringer's DB32 — the same default Aseprite seeds new files with).
    #[serde(default)]
    pub palette: Palette,
}

impl Document {
    /// Creates a new document with one empty page, the default palette, and no styles.
    pub fn new() -> Self {
        Self {
            name: "Untitled".to_string(),
            pages: vec![Page::new("Page 1")],
            active_page: 0,
            text_styles: Vec::new(),
            layer_styles: Vec::new(),
            palette: Palette::default(),
        }
    }

    /// The currently active page, clamped to a valid index if `active_page` is stale.
    pub fn active_page(&self) -> &Page {
        &self.pages[self.active_page.min(self.pages.len() - 1)]
    }

    /// Mutable access to the currently active page, clamped to a valid index if `active_page` is stale.
    pub fn active_page_mut(&mut self) -> &mut Page {
        let idx = self.active_page.min(self.pages.len() - 1);
        &mut self.pages[idx]
    }

    /// Searches every page's layer tree for `id`, returning a reference if found.
    pub fn find(&self, id: LayerId) -> Option<&Layer> {
        self.pages.iter().find_map(|p| p.find(id))
    }

    /// Visits every layer on every page, recursing into `Artboard`/`Group`
    /// children — used by `ui/inspector.rs`'s "Update Style" to propagate a
    /// shared `TextStyle` edit to every `Text` layer linked to it, across
    /// the whole document rather than just the active page.
    pub fn for_each_layer_mut(&mut self, f: &mut impl FnMut(&mut Layer)) {
        fn walk(layers: &mut [Layer], f: &mut impl FnMut(&mut Layer)) {
            for layer in layers {
                f(layer);
                if let Some(children) = layer.kind.children_mut() {
                    walk(children, f);
                }
            }
        }
        for page in &mut self.pages {
            walk(&mut page.layers, f);
        }
    }

    /// Adds a new page and makes it active, returning its index.
    pub fn add_page(&mut self, name: impl Into<String>) -> usize {
        self.pages.push(Page::new(name));
        self.active_page = self.pages.len() - 1;
        self.active_page
    }

    /// Removes the page at `index`, unless it's the last remaining page.
    /// Adjusts `active_page` so it still points at a valid page.
    pub fn remove_page(&mut self, index: usize) {
        if self.pages.len() <= 1 || index >= self.pages.len() {
            return;
        }
        self.pages.remove(index);
        if self.active_page > index {
            self.active_page -= 1;
        }
        if self.active_page >= self.pages.len() {
            self.active_page = self.pages.len() - 1;
        }
    }
}

impl Default for Document {
    fn default() -> Self {
        Self::new()
    }
}
