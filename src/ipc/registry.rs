//! Maps a `.sdesign` file's canonical path to the Unix socket a running GUI
//! instance is listening on for it, so the CLI can find the right instance
//! (or learn there isn't one) without being told a port/path explicitly.

use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

fn runtime_dir() -> PathBuf {
    let base = std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
    let dir = base.join("simple-design");
    let _ = std::fs::create_dir_all(&dir);
    dir
}

fn registry_path() -> PathBuf {
    runtime_dir().join("instances.json")
}

fn canonical_key(doc_path: &Path) -> String {
    doc_path.canonicalize().unwrap_or_else(|_| doc_path.to_path_buf()).to_string_lossy().into_owned()
}

/// A short, filesystem-safe name derived from the document's canonical path
/// — the path itself can't be used as a socket filename (it contains `/`).
pub fn socket_path_for(doc_path: &Path) -> PathBuf {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    canonical_key(doc_path).hash(&mut hasher);
    runtime_dir().join(format!("{:016x}.sock", hasher.finish()))
}

#[derive(Default, Serialize, Deserialize)]
struct RegistryFile(HashMap<String, String>);

fn load() -> RegistryFile {
    std::fs::read_to_string(registry_path()).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default()
}

fn store(registry: &RegistryFile) {
    if let Ok(json) = serde_json::to_string_pretty(registry) {
        let _ = std::fs::write(registry_path(), json);
    }
}

pub fn register(doc_path: &Path, socket_path: &Path) {
    let mut registry = load();
    registry.0.insert(canonical_key(doc_path), socket_path.to_string_lossy().into_owned());
    store(&registry);
}

pub fn unregister(doc_path: &Path) {
    let mut registry = load();
    registry.0.remove(&canonical_key(doc_path));
    store(&registry);
}

/// Connects to the running instance registered for `doc_path`, if any. A
/// registry entry whose socket no longer accepts connections (the GUI
/// crashed instead of exiting cleanly) is treated as stale and removed.
pub fn connect_live(doc_path: &Path) -> Option<UnixStream> {
    let key = canonical_key(doc_path);
    let registry = load();
    let socket = registry.0.get(&key)?.clone();
    match UnixStream::connect(&socket) {
        Ok(stream) => Some(stream),
        Err(_) => {
            unregister(doc_path);
            None
        }
    }
}
