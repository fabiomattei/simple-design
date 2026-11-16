//! Maps a `.sdesign` file's canonical path to the Unix socket a running GUI
//! instance is listening on for it, so the CLI can find the right instance
//! (or learn there isn't one) without being told a port/path explicitly.

use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Where this process registers its instances and binds its sockets. Inside a Flatpak that is
/// under `$XDG_RUNTIME_DIR/app/<app id>`, the one runtime folder Flatpak shares between every
/// instance of the app (`flatpak run --command=simple-design-cli` included) and the host, at the
/// same path on both sides, so a CLI anywhere can reach a sandboxed GUI.
fn runtime_dir() -> PathBuf {
    let base = runtime_base();
    let dir = match std::env::var_os("FLATPAK_ID") {
        Some(app_id) => base.join("app").join(app_id),
        None => base,
    }
    .join("simple-design");
    let _ = std::fs::create_dir_all(&dir);
    dir
}

fn runtime_base() -> PathBuf {
    std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from).unwrap_or_else(std::env::temp_dir)
}

/// Every folder a running instance may have registered in: this process's own, and, outside a
/// Flatpak, the Flatpak build's too (see `runtime_dir`).
fn registry_dirs() -> Vec<PathBuf> {
    let mut dirs = vec![runtime_dir()];
    if std::env::var_os("FLATPAK_ID").is_none() {
        dirs.push(runtime_base().join("app").join(crate::app_info::FLATPAK_ID).join("simple-design"));
    }
    dirs
}

fn registry_path(dir: &Path) -> PathBuf {
    dir.join("instances.json")
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

fn load(dir: &Path) -> RegistryFile {
    std::fs::read_to_string(registry_path(dir)).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default()
}

fn store(dir: &Path, registry: &RegistryFile) {
    if let Ok(json) = serde_json::to_string_pretty(registry) {
        let _ = std::fs::write(registry_path(dir), json);
    }
}

pub fn register(doc_path: &Path, socket_path: &Path) {
    let dir = runtime_dir();
    let mut registry = load(&dir);
    registry.0.insert(canonical_key(doc_path), socket_path.to_string_lossy().into_owned());
    store(&dir, &registry);
}

pub fn unregister(doc_path: &Path) {
    unregister_in(&runtime_dir(), doc_path);
}

fn unregister_in(dir: &Path, doc_path: &Path) {
    let mut registry = load(dir);
    registry.0.remove(&canonical_key(doc_path));
    store(dir, &registry);
}

/// Connects to the running instance registered for `doc_path`, if any. A
/// registry entry whose socket no longer accepts connections (the GUI
/// crashed instead of exiting cleanly) is treated as stale and removed.
pub fn connect_live(doc_path: &Path) -> Option<UnixStream> {
    let key = canonical_key(doc_path);
    registry_dirs().into_iter().find_map(|dir| {
        let socket = load(&dir).0.get(&key)?.clone();
        match UnixStream::connect(&socket) {
            Ok(stream) => Some(stream),
            Err(_) => {
                unregister_in(&dir, doc_path);
                None
            }
        }
    })
}
