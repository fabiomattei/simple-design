mod alignment;
mod boolean_ops;
mod canvas;
mod clipboard;
mod export;
mod fonts;
mod grid_ops;
mod grouping;
mod halftone_fill;
mod history;
mod image_ops;
mod io;
mod masking;
mod model;
mod noise_fill;
mod numeric_input;
mod palette_io;
mod shadow;
mod shapes;
mod system_fonts;
mod transform_ops;
mod text_layout;
mod text_outline;
mod tools;
mod ui;

mod app;

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
    eframe::run_native(
        "Simple Design",
        native_options,
        Box::new(|cc| Ok(Box::new(app::App::new(cc)))),
    )
}
