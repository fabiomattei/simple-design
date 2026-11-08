use std::path::PathBuf;

use simple_design::app;

mod desktop_entry;

use desktop_entry::{APP_ICON_PNG, APP_ID};

fn main() -> eframe::Result<()> {
    // Wayland ignores `with_icon` and shows the icon of the installed `.desktop` file named
    // after the app id, so keep that entry (and the icon it names) installed and current.
    #[cfg(target_os = "linux")]
    if let Err(error) = desktop_entry::install_desktop_entry() {
        eprintln!("simple-design: could not install the desktop entry: {error}");
    }
    let icon = eframe::icon_data::from_png_bytes(APP_ICON_PNG)
        .expect("failed to load embedded app icon");
    let native_options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1280.0, 800.0])
            .with_title("Simple Design")
            .with_icon(icon)
            .with_app_id(APP_ID),
        ..Default::default()
    };
    // An optional `.sdesign` path to open at startup — this is also what
    // gives the IPC socket (see `simple_design::ipc`) a path to bind to
    // immediately, so a CLI command can reach this instance without first
    // doing a manual Save As.
    let path_arg = std::env::args().nth(1).map(PathBuf::from);
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
