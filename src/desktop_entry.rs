//! The app's icon and, on Linux, its entry in the applications menu. Wayland takes a window's
//! icon only from the installed `.desktop` file whose name matches the window's app id, so
//! `with_icon` alone shows nothing there (it still works on X11, Windows and macOS).

#[cfg(any(target_os = "linux", test))]
use std::path::{Component, Path, PathBuf};
#[cfg(target_os = "linux")]
use std::{env, fs, io};

/// The window's app id, and the name of the `.desktop` file and icon installed for it.
pub const APP_ID: &str = "simple-design";

pub use simple_design::app_info::APP_ICON_PNG;

/// Writes the icon and, when `should_point_here` says so, `simple-design.desktop` pointing at
/// this build, into the user's data folder, skipping files whose contents are already up to
/// date. Returns the `.desktop` file.
#[cfg(target_os = "linux")]
pub fn install_desktop_entry() -> io::Result<PathBuf> {
    let program = env::current_exe()?.canonicalize()?;
    let data = data_folder()?;
    let icons = data.join("icons/hicolor/512x512/apps");
    write_if_changed(&icons.join(format!("{APP_ID}.png")), APP_ICON_PNG)?;
    let entry = data.join("applications").join(format!("{APP_ID}.desktop"));
    let current = fs::read_to_string(&entry).ok().and_then(|file| exec_program(&file));
    let current_exists = current.as_deref().is_some_and(Path::exists);
    if should_point_here(current.as_deref(), current_exists, &program) {
        write_if_changed(&entry, desktop_file(&program).as_bytes())?;
    }
    Ok(entry)
}

/// Whether the entry, now launching `current` (`None` when there is no entry or its `Exec` can't
/// be read), should be pointed at `program` instead. A working entry for another copy is left
/// alone, so running a build from a checkout doesn't take the applications menu over from the
/// installed app. The one exception is the reverse: an installed copy takes the entry over from
/// a cargo build.
#[cfg(any(target_os = "linux", test))]
fn should_point_here(current: Option<&Path>, current_exists: bool, program: &Path) -> bool {
    match current {
        None => true,
        Some(current) if current == program || !current_exists => true,
        Some(current) => is_cargo_build(current) && !is_cargo_build(program),
    }
}

/// Whether `program` sits in a cargo `target/debug` or `target/release` folder.
#[cfg(any(target_os = "linux", test))]
fn is_cargo_build(program: &Path) -> bool {
    let folders: Vec<_> = program.parent().into_iter().flat_map(Path::components).collect();
    folders.windows(2).any(|pair| {
        pair[0] == Component::Normal("target".as_ref())
            && matches!(pair[1], Component::Normal(name) if name == "debug" || name == "release")
    })
}

/// The program an entry's `Exec` line launches: the inverse of `quoted_exec`, or for an
/// unquoted value, everything up to the first space.
#[cfg(any(target_os = "linux", test))]
fn exec_program(desktop_file: &str) -> Option<PathBuf> {
    let value = desktop_file.lines().find_map(|line| line.strip_prefix("Exec="))?.replace("\\\\", "\\");
    let Some(quoted) = value.strip_prefix('"') else {
        return value.split(' ').next().filter(|program| !program.is_empty()).map(PathBuf::from);
    };
    let mut program = String::new();
    let mut chars = quoted.chars();
    loop {
        match chars.next()? {
            '"' => break,
            '\\' => program.push(chars.next()?),
            c => program.push(c),
        }
    }
    Some(PathBuf::from(program.replace("%%", "%")))
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
    fn exec_program_reads_back_what_quoted_exec_writes() {
        for program in ["/usr/bin/simple-design", "/opt/My Apps/sd$1%", "/a/\\b\"c`d"] {
            let file = desktop_file(Path::new(program));
            assert_eq!(exec_program(&file), Some(PathBuf::from(program)));
        }
        assert_eq!(exec_program("Exec=/usr/bin/sd %f\n"), Some(PathBuf::from("/usr/bin/sd")));
        assert_eq!(exec_program("Name=Simple Design\n"), None);
    }

    #[test]
    fn a_working_entry_for_another_copy_is_kept() {
        let installed = Path::new("/home/me/.local/bin/simple-design");
        let dev = Path::new("/home/me/src/simple-design/target/debug/simple-design");
        // A dev build never takes over a working entry...
        assert!(!should_point_here(Some(installed), true, dev));
        // ...and neither does another installed copy.
        assert!(!should_point_here(Some(installed), true, Path::new("/usr/bin/simple-design")));
        // An installed copy takes over from a dev build.
        assert!(should_point_here(Some(dev), true, installed));
        // A missing or broken entry is always (re)written, as is one already pointing here.
        assert!(should_point_here(None, false, dev));
        assert!(should_point_here(Some(installed), false, dev));
        assert!(should_point_here(Some(dev), true, dev));
    }

    #[test]
    fn recognizes_cargo_builds() {
        assert!(is_cargo_build(Path::new("/src/sd/target/debug/simple-design")));
        assert!(is_cargo_build(Path::new("/src/sd/target/release/simple-design")));
        assert!(!is_cargo_build(Path::new("/home/me/.local/bin/simple-design")));
        assert!(!is_cargo_build(Path::new("/opt/target/simple-design")));
    }

    #[test]
    fn entry_matches_the_window_app_id() {
        let file = desktop_file(Path::new("/usr/bin/simple-design"));
        assert!(file.contains("Exec=\"/usr/bin/simple-design\" %f\n"));
        assert!(file.contains("Icon=simple-design\n"));
        assert!(file.contains("StartupWMClass=simple-design\n"));
    }
}
