//! Undo/redo for the CLI's headless mode. With no GUI there's no in-memory
//! `History`, so every headless edit instead keeps the file's previous
//! contents as a snapshot under `<cache_dir>/headless-history/<path hash>/`:
//! `undo/` and `redo/` stacks of numbered `.sdesign` files, plus `current.sdesign`
//! — what the last headless edit wrote. `undo`/`redo` refuse to run when the
//! file no longer matches `current.sdesign` (it was edited since, e.g. in the
//! GUI), rather than silently discarding that edit.

use std::path::{Path, PathBuf};

/// Snapshots kept per document; the oldest is dropped past this.
const MAX_STEPS: usize = 50;

/// The history folder for `file`, or `None` when there's no cache folder.
pub fn dir_for(file: &Path) -> Option<PathBuf> {
    Some(crate::config_dir::cache_dir()?.join("headless-history").join(super::registry::path_hash(file)))
}

/// Records one headless edit of a file that held `before` and now holds
/// `after`: `before` becomes the newest undo step and the redo stack is cleared.
pub fn record(dir: &Path, before: &[u8], after: &[u8]) -> std::io::Result<()> {
    // An edit on top of a file someone else changed starts a fresh history:
    // undoing past that change would silently revert it.
    if std::fs::read(dir.join("current.sdesign")).ok().as_deref() != Some(before) {
        let _ = std::fs::remove_dir_all(dir);
    }
    push(&dir.join("undo"), before)?;
    let _ = std::fs::remove_dir_all(dir.join("redo"));
    std::fs::write(dir.join("current.sdesign"), after)
}

/// Restores `file` to its previous headless-edit snapshot. `redo` = true goes
/// the other way. Errors when there's nothing to undo/redo or `file` changed
/// since the last headless edit.
pub fn step(dir: &Path, file: &Path, redo: bool) -> Result<(), String> {
    let (from, to, what) = if redo { ("redo", "undo", "redo") } else { ("undo", "redo", "undo") };
    let current = std::fs::read(file).map_err(|err| err.to_string())?;
    if std::fs::read(dir.join("current.sdesign")).ok().as_deref() != Some(current.as_slice()) {
        return Err(format!("nothing to {what}: {} changed since the last headless CLI edit", file.display()));
    }
    let Some(snapshot) = pop(&dir.join(from)).map_err(|err| err.to_string())? else {
        return Err(format!("nothing to {what}"));
    };
    push(&dir.join(to), &current).map_err(|err| err.to_string())?;
    std::fs::write(file, &snapshot).map_err(|err| err.to_string())?;
    std::fs::write(dir.join("current.sdesign"), &snapshot).map_err(|err| err.to_string())
}

/// The stack's entries, oldest first.
fn entries(stack: &Path) -> Vec<(u64, PathBuf)> {
    let mut entries: Vec<(u64, PathBuf)> = std::fs::read_dir(stack)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| Some((e.path().file_stem()?.to_str()?.parse().ok()?, e.path())))
        .collect();
    entries.sort();
    entries
}

fn push(stack: &Path, contents: &[u8]) -> std::io::Result<()> {
    std::fs::create_dir_all(stack)?;
    let entries = entries(stack);
    let next = entries.last().map_or(0, |(n, _)| n + 1);
    std::fs::write(stack.join(format!("{next:06}.sdesign")), contents)?;
    for (_, path) in entries.iter().take((entries.len() + 1).saturating_sub(MAX_STEPS)) {
        std::fs::remove_file(path)?;
    }
    Ok(())
}

fn pop(stack: &Path) -> std::io::Result<Option<Vec<u8>>> {
    let Some((_, path)) = entries(stack).pop() else {
        return Ok(None);
    };
    let contents = std::fs::read(&path)?;
    std::fs::remove_file(&path)?;
    Ok(Some(contents))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("sd-headless-history-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// Writes `after` to `file` and records the edit, the way the CLI does.
    fn edit(dir: &Path, file: &Path, after: &str) {
        let before = std::fs::read(file).unwrap();
        std::fs::write(file, after).unwrap();
        record(dir, &before, after.as_bytes()).unwrap();
    }

    #[test]
    fn undo_and_redo_walk_back_and_forth_through_headless_edits() {
        let root = scratch();
        let (dir, file) = (root.join("history"), root.join("doc.sdesign"));
        std::fs::write(&file, "v0").unwrap();
        edit(&dir, &file, "v1");
        edit(&dir, &file, "v2");

        step(&dir, &file, false).unwrap();
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "v1");
        step(&dir, &file, false).unwrap();
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "v0");
        assert!(step(&dir, &file, false).unwrap_err().contains("nothing to undo"));
        step(&dir, &file, true).unwrap();
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "v1");

        // A new edit clears what's left to redo.
        edit(&dir, &file, "v3");
        assert!(step(&dir, &file, true).is_err());
        step(&dir, &file, false).unwrap();
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "v1");
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn undo_refuses_after_the_file_changed_outside_the_cli() {
        let root = scratch();
        let (dir, file) = (root.join("history"), root.join("doc.sdesign"));
        std::fs::write(&file, "v0").unwrap();
        edit(&dir, &file, "v1");
        std::fs::write(&file, "edited in the GUI").unwrap();

        assert!(step(&dir, &file, false).unwrap_err().contains("changed since"));
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "edited in the GUI");

        // ...and the next headless edit starts over instead of undoing past it.
        edit(&dir, &file, "v2");
        step(&dir, &file, false).unwrap();
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "edited in the GUI");
        assert!(step(&dir, &file, false).is_err());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn the_undo_stack_is_capped() {
        let root = scratch();
        let (dir, file) = (root.join("history"), root.join("doc.sdesign"));
        std::fs::write(&file, "start").unwrap();
        for i in 0..MAX_STEPS + 5 {
            edit(&dir, &file, &format!("v{i}"));
        }
        assert_eq!(entries(&dir.join("undo")).len(), MAX_STEPS);
        std::fs::remove_dir_all(root).unwrap();
    }
}
