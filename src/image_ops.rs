//! Pixel-level operations backing the `LayerKind::Image` features: decode/encode,
//! the non-destructive Color Adjust effect, and the destructive editing-mode
//! operations (Crop, Fill, Magic Wand, Trim Transparent Pixels, Remove
//! Background, Minimize File Size).
//!
//! `image::RgbaImage` (straight, non-premultiplied alpha — same convention as
//! PNG itself) is the one working representation used throughout; callers
//! convert to whatever the consumer needs (`to_egui_color_image` for the
//! canvas texture cache, `to_premultiplied_rgba` for compositing into
//! `export.rs`'s `tiny_skia::Pixmap`).
use std::collections::VecDeque;
use std::path::Path;

use egui::{Pos2, Vec2};
use image::RgbaImage;
use uuid::Uuid;

use crate::model::{ColorAdjust, Frame, Layer, LayerKind};

/// Decodes image bytes of any supported format (PNG/JPEG/GIF/BMP/TIFF/WebP)
/// into RGBA8. Returns `None` on anything unreadable (a non-image file
/// dropped on the canvas, corrupt data, etc.).
pub fn decode(bytes: &[u8]) -> Option<RgbaImage> {
    image::load_from_memory(bytes).ok().map(|d| d.to_rgba8())
}

/// Re-encodes an RGBA buffer to PNG bytes — the one format `LayerKind::Image::encoded`
/// is always stored as, regardless of the original insert format.
pub fn encode_png(img: &RgbaImage) -> Vec<u8> {
    let mut buf = Vec::new();
    let mut cursor = std::io::Cursor::new(&mut buf);
    image::DynamicImage::ImageRgba8(img.clone())
        .write_to(&mut cursor, image::ImageFormat::Png)
        .expect("encoding an in-memory RGBA buffer to PNG cannot fail");
    buf
}

/// Converts to egui's own `ColorImage` representation, for uploading as a canvas texture.
pub fn to_egui_color_image(img: &RgbaImage) -> egui::ColorImage {
    egui::ColorImage::from_rgba_unmultiplied([img.width() as usize, img.height() as usize], img.as_raw())
}

/// Premultiplies `img`'s alpha, the representation `tiny_skia::Pixmap`
/// requires (see `export.rs`'s `draw_image`).
pub fn to_premultiplied_rgba(img: &RgbaImage) -> Vec<u8> {
    let mut out = Vec::with_capacity(img.as_raw().len());
    for px in img.pixels() {
        let [r, g, b, a] = px.0;
        let a16 = a as u16;
        out.push(((r as u16 * a16) / 255) as u8);
        out.push(((g as u16 * a16) / 255) as u8);
        out.push(((b as u16 * a16) / 255) as u8);
        out.push(a);
    }
    out
}

/// Applies the non-destructive hue/saturation/brightness/contrast effect.
/// Returns a clone unchanged if `adjust` is the identity, so callers can call
/// this unconditionally without a fast-path check of their own.
pub fn apply_color_adjust(img: &RgbaImage, adjust: ColorAdjust) -> RgbaImage {
    if adjust.is_identity() {
        return img.clone();
    }
    let mut out = img.clone();
    let contrast_scale = 1.0 + adjust.contrast.clamp(-1.0, 1.0);
    for px in out.pixels_mut() {
        let [r, g, b, a] = px.0;
        if a == 0 {
            continue;
        }
        let (h, s, v) = rgb_to_hsv(r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0);
        let h = (h + adjust.hue).rem_euclid(360.0);
        let s = (s * (1.0 + adjust.saturation.clamp(-1.0, 1.0))).clamp(0.0, 1.0);
        let (mut rf, mut gf, mut bf) = hsv_to_rgb(h, s, v);

        rf += adjust.brightness;
        gf += adjust.brightness;
        bf += adjust.brightness;

        rf = (rf - 0.5) * contrast_scale + 0.5;
        gf = (gf - 0.5) * contrast_scale + 0.5;
        bf = (bf - 0.5) * contrast_scale + 0.5;

        px.0 = [
            (rf.clamp(0.0, 1.0) * 255.0).round() as u8,
            (gf.clamp(0.0, 1.0) * 255.0).round() as u8,
            (bf.clamp(0.0, 1.0) * 255.0).round() as u8,
            a,
        ];
    }
    out
}

/// Replaces `layer`'s image content with `new_img`, which was cropped out of
/// its old bitmap at pixel offset `(offset_x, offset_y)` — rescales and
/// repositions `layer.frame` so the crop's on-screen size/position stays
/// consistent with how the un-cropped image was displayed, re-encodes, and
/// bumps `version`. Shared by "Edit Image" mode's Crop-to-Selection
/// (`canvas.rs`) and the inspector's Trim Transparent Pixels — both are "cut
/// a sub-rect out of the current bitmap" operations that differ only in how
/// the sub-rect was chosen. A no-op if `layer.kind` isn't `Image`.
pub fn apply_cropped_image(layer: &mut Layer, new_img: &RgbaImage, offset_x: u32, offset_y: u32) {
    let LayerKind::Image { width, height, .. } = &layer.kind else { return };
    let bounds = layer.frame.bounds();
    let scale = Vec2::new(bounds.width() / *width as f32, bounds.height() / *height as f32);
    let new_pos = bounds.min + Vec2::new(offset_x as f32 * scale.x, offset_y as f32 * scale.y);
    let new_size = Vec2::new(new_img.width() as f32 * scale.x, new_img.height() as f32 * scale.y);
    let new_encoded = encode_png(new_img);
    layer.frame = Frame { pos: new_pos, size: new_size, rotation: layer.frame.rotation };
    if let LayerKind::Image { encoded, width, height, version, .. } = &mut layer.kind {
        *encoded = new_encoded;
        *width = new_img.width();
        *height = new_img.height();
        *version = Uuid::new_v4();
    }
}

/// "Minimize File Size"/"Reduce File Size": resizes `layer`'s bitmap down to
/// its current on-screen pixel size (never upscales — `resize_to` already
/// no-ops if the image is already that small or smaller) and re-encodes,
/// shedding resolution a shrunk frame no longer needs. A no-op if
/// `layer.kind` isn't `Image`.
pub fn minimize_image_file_size(layer: &mut Layer) {
    let bounds = layer.frame.bounds();
    let target_w = bounds.width().round().max(1.0) as u32;
    let target_h = bounds.height().round().max(1.0) as u32;
    let LayerKind::Image { encoded, .. } = &layer.kind else { return };
    let Some(decoded) = decode(encoded) else { return };
    let resized = resize_to(&decoded, target_w, target_h);
    let new_encoded = encode_png(&resized);
    let (w, h) = (resized.width(), resized.height());
    if let LayerKind::Image { encoded, width, height, version, .. } = &mut layer.kind {
        *encoded = new_encoded;
        *width = w;
        *height = h;
        *version = Uuid::new_v4();
    }
}

fn layer_name_for(path: &Path) -> String {
    path.file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "Image".to_string())
}

/// Builds `Image` layers for a batch of source paths (a multi-file OS
/// drag-and-drop, or a multi-select in the Insert Image dialog), arranged in
/// a left-to-right, wrapping grid anchored at `top_left` — an
/// "automatic grid arrangement" for multiple dropped images. Each image is
/// placed at its native pixel size, capped to `max_cell` in its largest
/// dimension (preserving aspect ratio) so a handful of huge photos doesn't
/// produce a grid many times the size of the canvas; unreadable paths are
/// silently skipped rather than failing the whole batch.
pub fn build_image_grid(paths: &[std::path::PathBuf], top_left: Pos2, max_cell: f32, gap: f32) -> Vec<Layer> {
    let max_row_width = max_cell * 4.0;
    let mut layers = Vec::new();
    let mut cursor = top_left;
    let mut row_height = 0.0f32;
    for path in paths {
        let Some(bytes) = std::fs::read(path).ok() else { continue };
        let Some(decoded) = decode(&bytes) else { continue };
        let (w, h) = (decoded.width() as f32, decoded.height() as f32);
        let scale = (max_cell / w.max(h).max(1.0)).min(1.0);
        let size = Vec2::new(w * scale, h * scale);
        if cursor.x > top_left.x && cursor.x + size.x > top_left.x + max_row_width {
            cursor.x = top_left.x;
            cursor.y += row_height + gap;
            row_height = 0.0;
        }
        let encoded = encode_png(&decoded);
        layers.push(Layer::new_image(
            layer_name_for(path),
            Frame { pos: cursor, size, rotation: 0.0 },
            encoded,
            decoded.width(),
            decoded.height(),
        ));
        cursor.x += size.x + gap;
        row_height = row_height.max(size.y);
    }
    layers
}

fn rgb_to_hsv(r: f32, g: f32, b: f32) -> (f32, f32, f32) {
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let delta = max - min;
    let h = if delta.abs() < 1e-6 {
        0.0
    } else if max == r {
        60.0 * (((g - b) / delta).rem_euclid(6.0))
    } else if max == g {
        60.0 * (((b - r) / delta) + 2.0)
    } else {
        60.0 * (((r - g) / delta) + 4.0)
    };
    let s = if max <= 1e-6 { 0.0 } else { delta / max };
    (h, s, max)
}

fn hsv_to_rgb(h: f32, s: f32, v: f32) -> (f32, f32, f32) {
    let c = v * s;
    let x = c * (1.0 - ((h / 60.0).rem_euclid(2.0) - 1.0).abs());
    let m = v - c;
    let (r, g, b) = match (h / 60.0) as u32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    (r + m, g + m, b + m)
}

/// Euclidean distance between two RGBA colors, folded to roughly `0..=100`
/// (matching the tolerance sliders in the inspector) rather than raw
/// `0..=510`.
fn color_distance(a: [u8; 4], b: [u8; 4]) -> f32 {
    let dr = a[0] as f32 - b[0] as f32;
    let dg = a[1] as f32 - b[1] as f32;
    let db = a[2] as f32 - b[2] as f32;
    let da = a[3] as f32 - b[3] as f32;
    (dr * dr + dg * dg + db * db + da * da).sqrt() / 5.1
}

fn neighbors4(x: u32, y: u32, w: u32, h: u32) -> impl Iterator<Item = (u32, u32)> {
    let mut v = Vec::with_capacity(4);
    if x > 0 {
        v.push((x - 1, y));
    }
    if x + 1 < w {
        v.push((x + 1, y));
    }
    if y > 0 {
        v.push((x, y - 1));
    }
    if y + 1 < h {
        v.push((x, y + 1));
    }
    v.into_iter()
}

/// Magic Wand: a boolean mask (row-major, `width * height`) of the
/// 4-connected region around `seed` whose pixels are all within `tolerance`
/// (`0..=100`) of the seed's own color. Out-of-bounds seeds return an
/// all-`false` mask.
pub fn magic_wand_mask(img: &RgbaImage, seed: (u32, u32), tolerance: f32) -> Vec<bool> {
    let (w, h) = img.dimensions();
    let mut mask = vec![false; (w as usize) * (h as usize)];
    if seed.0 >= w || seed.1 >= h {
        return mask;
    }
    let idx = |x: u32, y: u32| (y as usize) * (w as usize) + (x as usize);
    let seed_color = img.get_pixel(seed.0, seed.1).0;
    let mut queue = VecDeque::new();
    mask[idx(seed.0, seed.1)] = true;
    queue.push_back(seed);
    while let Some((x, y)) = queue.pop_front() {
        for (nx, ny) in neighbors4(x, y, w, h) {
            let i = idx(nx, ny);
            if mask[i] {
                continue;
            }
            if color_distance(img.get_pixel(nx, ny).0, seed_color) <= tolerance {
                mask[i] = true;
                queue.push_back((nx, ny));
            }
        }
    }
    mask
}

/// Heuristic, local "Remove Background": a multi-source flood fill seeded
/// from every border pixel, where each step compares a neighbor to the pixel
/// *just visited* (not the original seed) so it can follow gentle gradients
/// (vignettes, soft-lit backdrops) rather than only flat color. This is a
/// classical connected-region/color-tolerance technique — deliberately not
/// the ML semantic-segmentation models some design tools use (that needs a
/// trained model this offline environment has no way to fetch or bundle
/// safely), so it works best on fairly uniform or smoothly-varying
/// backgrounds and won't reliably separate a busy/textured background from a
/// similarly-colored subject.
///
/// Clears alpha to 0 on identified background pixels, then applies a 1px
/// feather (halves alpha on opaque pixels touching a cleared one) so the cut
/// edge isn't perfectly hard.
pub fn remove_background(img: &RgbaImage, tolerance: f32) -> RgbaImage {
    let (w, h) = img.dimensions();
    let mut out = img.clone();
    let n = (w as usize) * (h as usize);
    let idx = |x: u32, y: u32| (y as usize) * (w as usize) + (x as usize);
    let mut visited = vec![false; n];
    let mut queue = VecDeque::new();
    if w > 0 && h > 0 {
        for x in 0..w {
            for y in [0, h - 1] {
                if !visited[idx(x, y)] {
                    visited[idx(x, y)] = true;
                    queue.push_back((x, y));
                }
            }
        }
        for y in 0..h {
            for x in [0, w - 1] {
                if !visited[idx(x, y)] {
                    visited[idx(x, y)] = true;
                    queue.push_back((x, y));
                }
            }
        }
    }
    let mut cleared = vec![false; n];
    while let Some((x, y)) = queue.pop_front() {
        cleared[idx(x, y)] = true;
        let c = *img.get_pixel(x, y);
        for (nx, ny) in neighbors4(x, y, w, h) {
            let i = idx(nx, ny);
            if visited[i] {
                continue;
            }
            if color_distance(img.get_pixel(nx, ny).0, c.0) <= tolerance {
                visited[i] = true;
                queue.push_back((nx, ny));
            }
        }
    }
    for y in 0..h {
        for x in 0..w {
            if cleared[idx(x, y)] {
                out.get_pixel_mut(x, y).0[3] = 0;
            }
        }
    }
    for y in 0..h {
        for x in 0..w {
            if cleared[idx(x, y)] {
                continue;
            }
            let touches_cleared = neighbors4(x, y, w, h).any(|(nx, ny)| cleared[idx(nx, ny)]);
            if touches_cleared {
                let px = out.get_pixel_mut(x, y);
                px.0[3] = (px.0[3] as u16 * 128 / 255) as u8;
            }
        }
    }
    out
}

/// Bounding box (in pixel coordinates) of every pixel with nonzero alpha,
/// plus the trimmed image itself. `None` if the whole image is transparent.
pub fn trim_transparent(img: &RgbaImage) -> Option<(RgbaImage, u32, u32)> {
    let (w, h) = img.dimensions();
    let (mut min_x, mut min_y, mut max_x, mut max_y) = (w, h, 0u32, 0u32);
    let mut any = false;
    for y in 0..h {
        for x in 0..w {
            if img.get_pixel(x, y).0[3] > 0 {
                any = true;
                min_x = min_x.min(x);
                min_y = min_y.min(y);
                max_x = max_x.max(x);
                max_y = max_y.max(y);
            }
        }
    }
    if !any {
        return None;
    }
    let cropped = image::imageops::crop_imm(img, min_x, min_y, max_x - min_x + 1, max_y - min_y + 1).to_image();
    Some((cropped, min_x, min_y))
}

/// Crops to an axis-aligned pixel rect, clamped to the image bounds.
pub fn crop_to_rect(img: &RgbaImage, x: u32, y: u32, w: u32, h: u32) -> RgbaImage {
    let (iw, ih) = img.dimensions();
    let x = x.min(iw.saturating_sub(1));
    let y = y.min(ih.saturating_sub(1));
    let w = w.min(iw - x).max(1);
    let h = h.min(ih - y).max(1);
    image::imageops::crop_imm(img, x, y, w, h).to_image()
}

/// Paints every masked pixel (row-major, matching `magic_wand_mask`'s
/// layout) solid `color`, fully opaque — an image-editing "Fill".
pub fn fill_mask(img: &mut RgbaImage, mask: &[bool], color: [u8; 3]) {
    for (i, px) in img.pixels_mut().enumerate() {
        if mask[i] {
            px.0 = [color[0], color[1], color[2], 255];
        }
    }
}

/// Clears every masked pixel to fully transparent — used by the image-edit
/// mode's Delete action (magic-wand-select then delete, a common "erase the
/// background by hand" workflow alongside the automatic Remove Background).
pub fn clear_mask(img: &mut RgbaImage, mask: &[bool]) {
    for (i, px) in img.pixels_mut().enumerate() {
        if mask[i] {
            px.0[3] = 0;
        }
    }
}

/// Downscales (never upscales) to `target_w`x`target_h` with a high-quality
/// filter — "Minimize File Size"/"Reduce File Size" resampling the bitmap
/// down to whatever size it's actually displayed at, shedding excess
/// resolution a shrunk frame no longer needs.
pub fn resize_to(img: &RgbaImage, target_w: u32, target_h: u32) -> RgbaImage {
    let target_w = target_w.max(1);
    let target_h = target_h.max(1);
    if target_w >= img.width() && target_h >= img.height() {
        return img.clone();
    }
    image::imageops::resize(img, target_w, target_h, image::imageops::FilterType::Lanczos3)
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::Rgba;

    #[test]
    fn magic_wand_flood_fills_uniform_region_only() {
        let mut img = RgbaImage::from_pixel(4, 4, Rgba([255, 255, 255, 255]));
        img.put_pixel(3, 3, Rgba([0, 0, 0, 255]));
        let mask = magic_wand_mask(&img, (0, 0), 10.0);
        assert!(mask[0]);
        assert!(!mask[(3 * 4 + 3) as usize]);
        assert_eq!(mask.iter().filter(|&&m| m).count(), 15);
    }

    #[test]
    fn trim_transparent_finds_tight_bbox() {
        let mut img = RgbaImage::from_pixel(10, 10, Rgba([0, 0, 0, 0]));
        for y in 2..5 {
            for x in 3..6 {
                img.put_pixel(x, y, Rgba([255, 0, 0, 255]));
            }
        }
        let (cropped, ox, oy) = trim_transparent(&img).expect("non-empty image should trim");
        assert_eq!((cropped.width(), cropped.height()), (3, 3));
        assert_eq!((ox, oy), (3, 2));
    }

    #[test]
    fn remove_background_clears_uniform_border_but_keeps_center() {
        let mut img = RgbaImage::from_pixel(10, 10, Rgba([255, 255, 255, 255]));
        for y in 3..7 {
            for x in 3..7 {
                img.put_pixel(x, y, Rgba([10, 200, 10, 255]));
            }
        }
        let out = remove_background(&img, 10.0);
        assert_eq!(out.get_pixel(0, 0).0[3], 0);
        assert_eq!(out.get_pixel(5, 5).0[3], 255);
    }

    #[test]
    fn color_adjust_identity_is_a_noop() {
        let img = RgbaImage::from_pixel(2, 2, Rgba([10, 20, 30, 255]));
        let out = apply_color_adjust(&img, ColorAdjust::default());
        assert_eq!(img, out);
    }
}
