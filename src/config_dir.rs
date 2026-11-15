//! The per-user folder this app keeps its small settings files in
//! (`recent.json`, `panels.json`) — the one place that reads the environment
//! to find it.

use std::path::PathBuf;

/// `$XDG_CONFIG_HOME/simple-design`, else `~/.config/simple-design`, else
/// `%APPDATA%/simple-design`. `None` when none of those variables is set.
/// Does not create the folder.
pub fn config_dir() -> Option<PathBuf> {
    let config = std::env::var_os("XDG_CONFIG_HOME")
        .filter(|c| !c.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
        .or_else(|| std::env::var_os("APPDATA").map(PathBuf::from))?;
    Some(config.join("simple-design"))
}
