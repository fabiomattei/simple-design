//! The template picker's previews: each template's first artboard rendered small through
//! `export::render_layer_scaled` and uploaded as a texture, once per opening of the picker.

use std::collections::HashMap;

use egui::{ColorImage, Context, TextureHandle, TextureOptions};

use crate::model::{Document, Layer, LayerKind};
use crate::templates::Template;

/// Longest side of a rendered preview, in pixels — about twice the tile's preview area, so it
/// stays sharp on a HiDPI screen.
const THUMBNAIL_PIXELS: f32 = 320.0;

/// Rendered previews by index into the picker's template list.
#[derive(Default)]
pub struct TemplateThumbnails {
    /// `None` for a template with no preview; it is not rendered again.
    textures: HashMap<usize, Option<TextureHandle>>,
}

impl TemplateThumbnails {
    pub fn clear(&mut self) {
        self.textures.clear();
    }

    /// The preview of `template` (at `index` in the picker's list), rendered on first use.
    pub fn get(&mut self, ctx: &Context, index: usize, template: &Template) -> Option<TextureHandle> {
        self.textures.entry(index).or_insert_with(|| render(ctx, template)).clone()
    }
}

/// `None` when the template has no top-level artboard on its first page, or fails to load:
/// the tile then shows its initials instead. A load failure is not reported here because
/// picking the template reports it, with its reason, through `App::new_from_template`.
fn render(ctx: &Context, template: &Template) -> Option<TextureHandle> {
    let document = template.instantiate().ok()?;
    let artboard = first_artboard(&document)?;
    let size = artboard.frame.rotated_bounds().size();
    let scale = THUMBNAIL_PIXELS / size.x.max(size.y).max(1.0);
    let pixmap = crate::export::render_layer_scaled(artboard, scale)?;
    let pixels = [pixmap.width() as usize, pixmap.height() as usize];
    let image = ColorImage::from_rgba_premultiplied(pixels, pixmap.data());
    let name = format!("template thumbnail {}", template.name);
    Some(ctx.load_texture(name, image, TextureOptions::LINEAR))
}

fn first_artboard(document: &Document) -> Option<&Layer> {
    document.pages.first()?.layers.iter().find(|layer| matches!(layer.kind, LayerKind::Artboard { .. }))
}
