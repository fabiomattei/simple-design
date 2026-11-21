use std::path::PathBuf;

use clap::Parser;
use simple_design::app;

mod desktop_entry;

use desktop_entry::{APP_ICON_PNG, APP_ID};

/// Parsed before any window opens, so `--help`/`--version` (or a mistyped
/// flag) answer on the terminal instead of launching the GUI.
#[derive(Parser)]
#[command(
    name = "simple-design",
    version,
    about = "A native vector design tool — opens the GUI.",
    after_help = "To edit a .sdesign file from the command line or a script (live in a running \
                  instance, or headless), use simple-design-cli instead: simple-design-cli --help"
)]
struct Args {
    /// A .sdesign file to open at startup.
    file: Option<PathBuf>,
}

fn main() -> eframe::Result<()> {
    let args = Args::parse();
    // Wayland ignores `with_icon` and shows the icon of the installed `.desktop` file named
    // after the app id, so keep that entry (and the icon it names) installed and current.
    // A Flatpak exports its own entry, named after the Flatpak's app id, which the window then
    // takes as its app id too; one written from inside the sandbox would land in its private
    // data folder and do nothing.
    let flatpak_id = std::env::var("FLATPAK_ID").ok();
    #[cfg(target_os = "linux")]
    if flatpak_id.is_none()
        && let Err(error) = desktop_entry::install_desktop_entry()
    {
        eprintln!("simple-design: could not install the desktop entry: {error}");
    }
    let icon = eframe::icon_data::from_png_bytes(APP_ICON_PNG)
        .expect("failed to load embedded app icon");
    let native_options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1280.0, 800.0])
            .with_title("Simple Design")
            .with_icon(icon)
            .with_app_id(flatpak_id.as_deref().unwrap_or(APP_ID)),
        ..Default::default()
    };
    // An optional `.sdesign` path to open at startup — this is also what
    // gives the IPC socket (see `simple_design::ipc`) a path to bind to
    // immediately, so a CLI command can reach this instance without first
    // doing a manual Save As.
    let path_arg = args.file;
    eframe::run_native(
        "Simple Design",
        native_options,
        Box::new(move |cc| {
            let mut app = app::App::new(cc);
            if let Some(path) = path_arg {
                app.open_path(path);
            }
            Ok(Box::new(app))
        }),
    )
}
