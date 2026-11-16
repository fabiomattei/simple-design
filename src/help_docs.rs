//! The HTML docs in `docs/`, embedded in the binary so Help > Documentation works from any
//! install (`~/.local/bin`, Flatpak, `cargo run`), not just next to a source checkout. A browser
//! can't open pages from inside the binary, so they are written out to the cache folder first.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

/// The Overview page, which links to all the others.
pub const INDEX: &str = "index.html";
/// Tools & Features: every tool, menu command and keyboard shortcut.
pub const FEATURES: &str = "features.html";

/// Every file in `docs/`, by file name. A test checks that none is missing.
const FILES: &[(&str, &str)] = &[
    ("index.html", include_str!("../docs/index.html")),
    ("getting-started.html", include_str!("../docs/getting-started.html")),
    ("features.html", include_str!("../docs/features.html")),
    ("architecture.html", include_str!("../docs/architecture.html")),
    ("text-and-fonts.html", include_str!("../docs/text-and-fonts.html")),
    ("file-format.html", include_str!("../docs/file-format.html")),
    ("html-export.html", include_str!("../docs/html-export.html")),
    ("packaging.html", include_str!("../docs/packaging.html")),
    ("style.css", include_str!("../docs/style.css")),
];

/// Writes the docs into the cache folder and returns the path of `page` there. They are
/// rewritten every time, so they always match this build.
pub fn write_page(page: &str) -> io::Result<PathBuf> {
    let folder = docs_folder();
    write_all(&folder)?;
    Ok(folder.join(page))
}

fn write_all(folder: &Path) -> io::Result<()> {
    fs::create_dir_all(folder)?;
    for (name, contents) in FILES {
        fs::write(folder.join(name), contents)?;
    }
    Ok(())
}

/// `$XDG_CACHE_HOME/simple-design/docs`, else `~/.cache/simple-design/docs`, else the same
/// under the system's temporary folder.
fn docs_folder() -> PathBuf {
    std::env::var_os("XDG_CACHE_HOME")
        .filter(|cache| !cache.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".cache")))
        .unwrap_or_else(std::env::temp_dir)
        .join("simple-design")
        .join("docs")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_docs_file_is_embedded() {
        let docs = Path::new(env!("CARGO_MANIFEST_DIR")).join("docs");
        for entry in fs::read_dir(docs).unwrap() {
            let name = entry.unwrap().file_name().into_string().unwrap();
            assert!(FILES.iter().any(|(file, _)| *file == name), "docs/{name} is not in help_docs::FILES");
        }
        assert!(FILES.iter().any(|(file, _)| *file == INDEX));
        assert!(FILES.iter().any(|(file, _)| *file == FEATURES));
    }

    #[test]
    fn writes_every_file() {
        let folder = std::env::temp_dir().join(format!("simple-design-docs-test-{}", std::process::id()));
        write_all(&folder).unwrap();
        for (name, contents) in FILES {
            assert_eq!(fs::read_to_string(folder.join(name)).unwrap(), *contents);
        }
        fs::remove_dir_all(folder).unwrap();
    }
}
