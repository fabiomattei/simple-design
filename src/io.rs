use std::path::PathBuf;

use crate::model::Document;

/// File picker for "Save As" — a `.sdesign` document.
pub fn save_dialog(default_name: &str) -> Option<PathBuf> {
    rfd::FileDialog::new()
        .set_file_name(default_name)
        .add_filter("Simple Design document", &["sdesign"])
        .save_file()
}

/// File picker for "Open" — a `.sdesign` document.
pub fn open_dialog() -> Option<PathBuf> {
    rfd::FileDialog::new()
        .add_filter("Simple Design document", &["sdesign"])
        .pick_file()
}

/// File picker for "Insert > Image" — multi-select, so a whole batch can be
/// placed at once (see `image_ops::build_image_grid`'s grid arrangement).
pub fn open_image_dialog() -> Option<Vec<PathBuf>> {
    rfd::FileDialog::new()
        .add_filter("Image", &["png", "jpg", "jpeg", "gif", "bmp", "tiff", "tif", "webp"])
        .pick_files()
}

/// File picker for a `Paint::Pattern` fill's tile image — single-select
/// (one tile per fill), same filter list as `open_image_dialog`.
pub fn open_pattern_image_dialog() -> Option<PathBuf> {
    rfd::FileDialog::new()
        .add_filter("Image", &["png", "jpg", "jpeg", "gif", "bmp", "tiff", "tif", "webp"])
        .pick_file()
}

/// File picker for the Palette panel's "Import..." — Aseprite-compatible
/// `.gpl` files (see `crate::palette_io`).
pub fn open_palette_dialog() -> Option<PathBuf> {
    rfd::FileDialog::new()
        .add_filter("GIMP/Aseprite palette", &["gpl"])
        .pick_file()
}

/// File picker for the Palette panel's "Export...".
pub fn save_palette_dialog(default_name: &str) -> Option<PathBuf> {
    rfd::FileDialog::new()
        .set_file_name(default_name)
        .add_filter("GIMP/Aseprite palette", &["gpl"])
        .save_file()
}

/// File picker for "Export > PNG".
pub fn export_png_dialog(default_name: &str) -> Option<PathBuf> {
    rfd::FileDialog::new()
        .set_file_name(default_name)
        .add_filter("PNG image", &["png"])
        .save_file()
}

/// Serializes `document` as pretty-printed JSON and writes it to `path`.
pub fn save_to(path: &PathBuf, document: &Document) -> anyhow::Result<()> {
    let json = serde_json::to_string_pretty(document)?;
    std::fs::write(path, json)?;
    Ok(())
}

/// Reads and deserializes a `.sdesign` JSON document from `path`.
pub fn load_from(path: &PathBuf) -> anyhow::Result<Document> {
    let data = std::fs::read_to_string(path)?;
    let document = serde_json::from_str(&data)?;
    Ok(document)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Frame, Layer};
    use egui::{Pos2, Vec2};

    /// Full `.sdesign` save/load round trip with an `Image` layer — the
    /// concern this covers (beyond `model::layer::tests::image_layer_round_trips_through_json`'s
    /// in-memory check) is that going through an actual file on disk with
    /// `to_string_pretty`'s formatting doesn't corrupt the base64 payload.
    #[test]
    fn saving_and_loading_a_document_with_an_image_layer_preserves_its_bytes() {
        let pixels = image::RgbaImage::from_pixel(3, 3, image::Rgba([10, 20, 30, 255]));
        let encoded = crate::image_ops::encode_png(&pixels);
        let layer = Layer::new_image(
            "Photo",
            Frame { pos: Pos2::new(0.0, 0.0), size: Vec2::new(30.0, 30.0), rotation: 0.0 },
            encoded.clone(),
            3,
            3,
        );

        let mut document = Document::new();
        document.active_page_mut().layers.push(layer);

        let path = std::env::temp_dir().join(format!("simple-design-test-{}.sdesign", uuid::Uuid::new_v4()));
        save_to(&path, &document).expect("save");
        let loaded = load_from(&path).expect("load");
        std::fs::remove_file(&path).ok();

        let loaded_layer = &loaded.active_page().layers[0];
        let crate::model::LayerKind::Image { encoded: loaded_encoded, .. } = &loaded_layer.kind else {
            panic!("expected an Image layer");
        };
        assert_eq!(loaded_encoded, &encoded);
    }
}
