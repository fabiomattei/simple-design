//! The app's icon and, on Linux, its entry in the applications menu. Wayland takes a window's
//! icon only from the installed `.desktop` file whose name matches the window's app id, so
//! `with_icon` alone shows nothing there (it still works on X11, Windows and macOS).

#[cfg(any(target_os = "linux", test))]
use std::path::Path;
#[cfg(target_os = "linux")]
use std::{env, fs, io, path::PathBuf};

/// The window's app id, and the name of the `.desktop` file and icon installed for it.
pub const APP_ID: &str = "simple-design";

/// The lady's slipper orchid, drawn as `assets/icon/simple-design.svg`.
pub const APP_ICON_PNG: &[u8] = include_bytes!("../assets/icon/simple-design.png");

/// Writes the icon and `simple-design.desktop`, pointing at this build, into the user's data
/// folder, skipping files whose contents are already up to date. Returns the `.desktop` file.
#[cfg(target_os = "linux")]
pub fn install_desktop_entry() -> io::Result<PathBuf> {
    let program = env::current_exe()?.canonicalize()?;
    let data = data_folder()?;
    let icons = data.join("icons/hicolor/512x512/apps");
    write_if_changed(&icons.join(format!("{APP_ID}.png")), APP_ICON_PNG)?;
    let entry = data.join("applications").join(format!("{APP_ID}.desktop"));
    write_if_changed(&entry, desktop_file(&program).as_bytes())?;
    Ok(entry)
}

#[cfg(target_os = "linux")]
fn write_if_changed(path: &Path, contents: &[u8]) -> io::Result<()> {
    if fs::read(path).is_ok_and(|current| current == contents) {
        return Ok(());
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, contents)
}

/// `$XDG_DATA_HOME`, or `~/.local/share`.
#[cfg(target_os = "linux")]
fn data_folder() -> io::Result<PathBuf> {
    if let Some(data) = env::var_os("XDG_DATA_HOME").filter(|data| !data.is_empty()) {
        return Ok(PathBuf::from(data));
    }
    env::var_os("HOME")
        .map(|home| PathBuf::from(home).join(".local/share"))
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "HOME is not set"))
}

#[cfg(any(target_os = "linux", test))]
fn desktop_file(program: &Path) -> String {
    format!(
        "[Desktop Entry]\n\
         Type=Application\n\
         Name=Simple Design\n\
         Comment=Vector design tool\n\
         Exec={} %f\n\
         Icon={APP_ID}\n\
         StartupWMClass={APP_ID}\n\
         Terminal=false\n\
         Categories=Graphics;2DGraphics;VectorGraphics;\n",
        quoted_exec(program)
    )
}

/// The program path as the Desktop Entry spec wants it in `Exec`: double-quoted, with `"`,
/// `` ` ``, `$` and `\` escaped by a backslash and `%` doubled; then every backslash doubled
/// again, as in any string value of the file.
#[cfg(any(target_os = "linux", test))]
fn quoted_exec(program: &Path) -> String {
    let mut quoted = String::from('"');
    for c in program.to_string_lossy().chars() {
        match c {
            '"' | '`' | '$' | '\\' => {
                quoted.push('\\');
                quoted.push(c);
            }
            '%' => quoted.push_str("%%"),
            _ => quoted.push(c),
        }
    }
    quoted.push('"');
    quoted.replace('\\', "\\\\")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exec_quotes_spaces_and_escapes_specials() {
        assert_eq!(
            quoted_exec(Path::new("/opt/My Apps/sd$1%")),
            "\"/opt/My Apps/sd\\\\$1%%\""
        );
    }

    #[test]
    fn entry_matches_the_window_app_id() {
        let file = desktop_file(Path::new("/usr/bin/simple-design"));
        assert!(file.contains("Exec=\"/usr/bin/simple-design\" %f\n"));
        assert!(file.contains("Icon=simple-design\n"));
        assert!(file.contains("StartupWMClass=simple-design\n"));
    }
}
