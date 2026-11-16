//! Handing a path to the platform's file manager or default app.

use std::io;
use std::path::Path;
use std::process::Command;

/// The file manager's name, for the "Show in …" menu item.
#[cfg(target_os = "macos")]
pub const FILE_MANAGER_NAME: &str = "Finder";
#[cfg(windows)]
pub const FILE_MANAGER_NAME: &str = "Explorer";
#[cfg(not(any(target_os = "macos", windows)))]
pub const FILE_MANAGER_NAME: &str = "File Manager";

/// Shows `path` in the file manager: selected in its folder on macOS and Windows; elsewhere
/// the folder that contains it is opened. Fails when the program cannot be started.
pub fn show_in_file_manager(path: &Path) -> io::Result<()> {
    #[cfg(target_os = "macos")]
    let mut command = {
        let mut command = Command::new("open");
        command.arg("-R").arg(path);
        command
    };
    #[cfg(windows)]
    let mut command = {
        use std::os::windows::process::CommandExt;
        // Explorer parses "/select," itself, so the quoting must not be Rust's.
        let mut command = Command::new("explorer");
        command.raw_arg(format!("/select,\"{}\"", path.display()));
        command
    };
    #[cfg(not(any(target_os = "macos", windows)))]
    let mut command = {
        let mut command = Command::new("xdg-open");
        command.arg(path.parent().unwrap_or(path));
        command
    };
    // The file manager keeps running on its own; only a failure to start it matters.
    command.spawn().map(drop)
}

/// Opens `path` in the app the platform associates with its type, e.g. an `.html` file in the
/// web browser. Fails when the program cannot be started.
pub fn open_with_default_app(path: &Path) -> io::Result<()> {
    #[cfg(target_os = "macos")]
    let mut command = Command::new("open");
    #[cfg(windows)]
    let mut command = Command::new("explorer");
    #[cfg(not(any(target_os = "macos", windows)))]
    let mut command = Command::new("xdg-open");
    command.arg(path).spawn().map(drop)
}
