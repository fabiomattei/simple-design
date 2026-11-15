//! "Export as HTML": writes a layer tree as `index.html` + `style.css` +
//! `images/` into a folder. Layers CSS can describe become styled elements
//! (`css_rules`); everything else is rasterized (`raster_fallback`).

mod element_tree;
mod raster_fallback;

use std::path::{Path, PathBuf};

/// Exports `layer` into a new subfolder of `parent_dir` named after the
/// layer, and returns that subfolder. `roots` is the page's top-level
/// layers (see `element_tree::build_site`). Side effects: creates the
/// subfolder and overwrites any same-named files already in it.
pub fn export_html(layer: &crate::model::Layer, roots: &[crate::model::Layer], parent_dir: &Path) -> anyhow::Result<PathBuf> {
    let site = element_tree::build_site(layer, roots)?;
    let dir = parent_dir.join(folder_name(&layer.name));
    std::fs::create_dir_all(&dir)?;
    std::fs::write(dir.join("index.html"), site.html)?;
    std::fs::write(dir.join("style.css"), site.css)?;
    if !site.images.is_empty() {
        std::fs::create_dir_all(dir.join("images"))?;
    }
    for (relative_path, bytes) in site.images {
        std::fs::write(dir.join(relative_path), bytes)?;
    }
    Ok(dir)
}

/// `name` with anything unsafe in a file name replaced by `_`.
fn folder_name(name: &str) -> String {
    let cleaned: String = name.chars().map(|c| if c.is_alphanumeric() || matches!(c, ' ' | '-' | '_') { c } else { '_' }).collect();
    let trimmed = cleaned.trim();
    if trimmed.is_empty() { "export".to_string() } else { trimmed.to_string() }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn folder_name_strips_path_separators() {
        assert_eq!(folder_name("Home/Mobile: v2"), "Home_Mobile_ v2");
        assert_eq!(folder_name("  "), "export");
    }
}
