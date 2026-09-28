use std::path::PathBuf;

use simple_design::app;

fn main() -> eframe::Result<()> {
    let icon = eframe::icon_data::from_png_bytes(include_bytes!("../assets/icon.png"))
        .expect("failed to load embedded app icon");
    let native_options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1280.0, 800.0])
            .with_title("Simple Design")
            .with_icon(icon)
            .with_app_id("simple-design"),
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
