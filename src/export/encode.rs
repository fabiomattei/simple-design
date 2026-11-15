use std::io::Write;

use image::codecs::jpeg::JpegEncoder;
use image::codecs::png::{CompressionType, FilterType, PngEncoder};
use image::codecs::webp::WebPEncoder;
use image::{ExtendedColorType, ImageEncoder};
use tiny_skia::Pixmap;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExportFormat {
    Png,
    Jpeg,
    /// Lossless only — `image`'s WebP encoder has no lossy mode, so only
    /// `ExportSettings::scale` affects its file size.
    Webp,
}

impl ExportFormat {
    pub const ALL: [ExportFormat; 3] = [ExportFormat::Png, ExportFormat::Jpeg, ExportFormat::Webp];

    pub fn label(self) -> &'static str {
        match self {
            ExportFormat::Png => "PNG",
            ExportFormat::Jpeg => "JPG",
            ExportFormat::Webp => "WebP",
        }
    }

    pub fn extension(self) -> &'static str {
        match self {
            ExportFormat::Png => "png",
            ExportFormat::Jpeg => "jpg",
            ExportFormat::Webp => "webp",
        }
    }
}

/// PNG is always lossless; this only trades encoding time for file size.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PngCompression {
    Fast,
    Balanced,
    Smallest,
}

impl PngCompression {
    pub const ALL: [PngCompression; 3] = [PngCompression::Fast, PngCompression::Balanced, PngCompression::Smallest];

    pub fn label(self) -> &'static str {
        match self {
            PngCompression::Fast => "Fast (larger file)",
            PngCompression::Balanced => "Balanced",
            PngCompression::Smallest => "Smallest file (slower)",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ExportSettings {
    pub format: ExportFormat,
    /// Pixels per document unit — `2.0` exports at twice the layer's size.
    pub scale: f32,
    /// 1–100, JPEG only.
    pub jpeg_quality: u8,
    pub png_compression: PngCompression,
}

impl Default for ExportSettings {
    fn default() -> Self {
        Self { format: ExportFormat::Png, scale: 1.0, jpeg_quality: 85, png_compression: PngCompression::Balanced }
    }
}

/// Encodes `pixmap` per `settings.format` into `out`. JPEG has no alpha
/// channel, so transparent areas are flattened onto white.
pub fn encode_pixmap(pixmap: &Pixmap, settings: &ExportSettings, out: impl Write) -> anyhow::Result<()> {
    let (width, height) = (pixmap.width(), pixmap.height());
    match settings.format {
        ExportFormat::Png => {
            let compression = match settings.png_compression {
                PngCompression::Fast => CompressionType::Fast,
                PngCompression::Balanced => CompressionType::Default,
                PngCompression::Smallest => CompressionType::Best,
            };
            PngEncoder::new_with_quality(out, compression, FilterType::Adaptive).write_image(
                &straight_rgba(pixmap),
                width,
                height,
                ExtendedColorType::Rgba8,
            )?;
        }
        ExportFormat::Jpeg => {
            let quality = settings.jpeg_quality.clamp(1, 100);
            JpegEncoder::new_with_quality(out, quality).write_image(&rgb_on_white(pixmap), width, height, ExtendedColorType::Rgb8)?;
        }
        ExportFormat::Webp => {
            WebPEncoder::new_lossless(out).write_image(&straight_rgba(pixmap), width, height, ExtendedColorType::Rgba8)?;
        }
    }
    Ok(())
}

/// tiny-skia stores premultiplied alpha; every encoder here expects straight alpha.
fn straight_rgba(pixmap: &Pixmap) -> Vec<u8> {
    pixmap
        .pixels()
        .iter()
        .flat_map(|pixel| {
            let color = pixel.demultiply();
            [color.red(), color.green(), color.blue(), color.alpha()]
        })
        .collect()
}

/// Source-over onto opaque white: with premultiplied channels that's just
/// `channel + (255 - alpha)`, which can't overflow since `channel <= alpha`.
fn rgb_on_white(pixmap: &Pixmap) -> Vec<u8> {
    pixmap
        .pixels()
        .iter()
        .flat_map(|pixel| {
            let backdrop = 255 - pixel.alpha();
            [pixel.red() + backdrop, pixel.green() + backdrop, pixel.blue() + backdrop]
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tiny_skia::Color;

    /// Left half opaque red, right half fully transparent.
    fn half_red_pixmap() -> Pixmap {
        let mut pixmap = Pixmap::new(8, 4).unwrap();
        let rect = tiny_skia::Rect::from_xywh(0.0, 0.0, 4.0, 4.0).unwrap();
        let mut paint = tiny_skia::Paint::default();
        paint.set_color(Color::from_rgba8(255, 0, 0, 255));
        pixmap.fill_rect(rect, &paint, tiny_skia::Transform::identity(), None);
        pixmap
    }

    fn encode(format: ExportFormat) -> image::DynamicImage {
        let settings = ExportSettings { format, ..ExportSettings::default() };
        let mut bytes = Vec::new();
        encode_pixmap(&half_red_pixmap(), &settings, &mut bytes).expect("should encode");
        image::load_from_memory(&bytes).expect("should decode")
    }

    #[test]
    fn png_round_trips_with_transparency() {
        let decoded = encode(ExportFormat::Png).to_rgba8();
        assert_eq!(decoded.dimensions(), (8, 4));
        assert_eq!(decoded.get_pixel(1, 1).0, [255, 0, 0, 255]);
        assert_eq!(decoded.get_pixel(6, 1).0[3], 0);
    }

    #[test]
    fn webp_round_trips_losslessly_with_transparency() {
        let decoded = encode(ExportFormat::Webp).to_rgba8();
        assert_eq!(decoded.dimensions(), (8, 4));
        assert_eq!(decoded.get_pixel(1, 1).0, [255, 0, 0, 255]);
        assert_eq!(decoded.get_pixel(6, 1).0[3], 0);
    }

    #[test]
    fn jpeg_flattens_transparency_onto_white() {
        let decoded = encode(ExportFormat::Jpeg).to_rgb8();
        assert_eq!(decoded.dimensions(), (8, 4));
        let transparent_side = decoded.get_pixel(7, 1).0;
        assert!(transparent_side.iter().all(|&channel| channel > 240), "got {transparent_side:?}");
    }

    #[test]
    fn lower_jpeg_quality_produces_a_smaller_file() {
        let noisy = {
            let mut pixmap = Pixmap::new(64, 64).unwrap();
            for (i, byte) in pixmap.data_mut().iter_mut().enumerate() {
                *byte = if i % 4 == 3 { 255 } else { (i * 37 % 251) as u8 };
            }
            pixmap
        };
        let size_at = |jpeg_quality| {
            let settings = ExportSettings { format: ExportFormat::Jpeg, jpeg_quality, ..ExportSettings::default() };
            let mut bytes = Vec::new();
            encode_pixmap(&noisy, &settings, &mut bytes).expect("should encode");
            bytes.len()
        };
        assert!(size_at(20) < size_at(95));
    }
}
