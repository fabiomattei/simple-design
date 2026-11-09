//! File > Open Recent: the most recently opened/saved `.sdesign` paths, kept in
//! `$XDG_CONFIG_HOME/simple-design/recent.json` (or `~/.config/...`) so the list
//! survives restarts. Errors reading or writing it are ignored — losing the
//! list is never worth interrupting an open or a save.

use std::path::{Path, PathBuf};

const MAX_ENTRIES: usize = 10;

#[derive(Default)]
pub struct RecentFiles {
    paths: Vec<PathBuf>,
}

impl RecentFiles {
    pub fn load() -> Self {
        let paths = storage_path()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default();
        Self { paths }
    }

    /// Most recent first.
    pub fn paths(&self) -> &[PathBuf] {
        &self.paths
    }

    /// Moves `path` to the front (adding it if new) and persists the list.
    pub fn push(&mut self, path: &Path) {
        let path = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
        push_front(&mut self.paths, path);
        self.store();
    }

    pub fn remove(&mut self, path: &Path) {
        self.paths.retain(|p| p != path);
        self.store();
    }

    pub fn clear(&mut self) {
        self.paths.clear();
        self.store();
    }

    fn store(&self) {
        let Some(file) = storage_path() else { return };
        if let Some(parent) = file.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Ok(json) = serde_json::to_string_pretty(&self.paths) {
            let _ = std::fs::write(file, json);
        }
    }
}

fn push_front(paths: &mut Vec<PathBuf>, path: PathBuf) {
    paths.retain(|p| *p != path);
    paths.insert(0, path);
    paths.truncate(MAX_ENTRIES);
}

fn storage_path() -> Option<PathBuf> {
    let config = std::env::var_os("XDG_CONFIG_HOME")
        .filter(|c| !c.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
        .or_else(|| std::env::var_os("APPDATA").map(PathBuf::from))?;
    Some(config.join("simple-design").join("recent.json"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn push_front_dedupes_and_caps_length() {
        let mut paths = Vec::new();
        for i in 0..MAX_ENTRIES + 3 {
            push_front(&mut paths, PathBuf::from(format!("/f{i}.sdesign")));
        }
        assert_eq!(paths.len(), MAX_ENTRIES);
        assert_eq!(paths[0], PathBuf::from(format!("/f{}.sdesign", MAX_ENTRIES + 2)));

        push_front(&mut paths, PathBuf::from("/f5.sdesign"));
        assert_eq!(paths[0], PathBuf::from("/f5.sdesign"));
        assert_eq!(paths.iter().filter(|p| **p == PathBuf::from("/f5.sdesign")).count(), 1);
        assert_eq!(paths.len(), MAX_ENTRIES);
    }
}
