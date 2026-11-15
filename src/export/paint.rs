//! Color and paint conversion for the exporter: egui colors to `tiny-skia`
//! colors, and each `model::Paint` variant (solid, gradient, noise,
//! halftone, pattern) to a `tiny_skia::Paint`.

use tiny_skia::{Color, Paint, Pixmap, PremultipliedColorU8, Transform};

pub(super) fn to_color(c: egui::Color32) -> Color {
    Color::from_rgba8(c.r(), c.g(), c.b(), c.a())
}

/// Scales `color`'s alpha channel by `opacity` (`0.0..=1.0`) before
/// converting, mirroring `canvas/fill_mesh.rs`'s `with_opacity` so a layer's (and its
/// ancestors', multiplicatively accumulated) opacity renders the same in the
/// exported PNG as it does live.
pub(super) fn with_opacity(color: egui::Color32, opacity: f32) -> egui::Color32 {
    let a = (color.a() as f32 * opacity.clamp(0.0, 1.0)).round() as u8;
    egui::Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), a)
}

pub(super) fn to_color_with_opacity(c: egui::Color32, opacity: f32) -> Color {
    to_color(with_opacity(c, opacity))
}

/// Builds a `tiny-skia` `Paint` for `paint` (solid or gradient), anti-aliased.
/// `bounds` is the shape's own absolute, *unrotated* bounds within the
/// pixmap (matching whatever local space the path being filled/stroked was
/// built in, before `layer_transform` rotates and scales it at draw time) —
/// `Gradient::from`/`to` are normalized `0.0..=1.0` per axis within it, per
/// `model::Gradient`'s doc comment. Building the shader's control points in
/// that same unrotated space, rather than pre-rotating them here, is what
/// lets the gradient rotate along with the shape: `Pixmap::fill_path`
/// applies its `transform` argument to the shader exactly like it does to
/// the path (see `tiny-skia`'s `painter.rs::fill_path`), so passing the same
/// `layer_transform(layer, bounds, render_scale)` both places keeps them in sync.
/// `noise_storage` exists only so a `Paint::Noise` fill's generated grain
/// `Pixmap` can outlive this call: `tiny_skia::Pattern`'s shader *borrows*
/// its source pixmap (unlike `Solid`/`Gradient`, which own their color data
/// outright), so the caller must keep that pixmap alive for exactly as long
/// as the returned `Paint` — declaring `let mut noise_storage = None;`
/// immediately before the call, in the same scope as the following
/// `fill_path`/`stroke_path`, does that. Every other `Paint` variant leaves
/// it `None`. `render_scale` is pixmap pixels per document unit (see
/// `export::draw_layer`): the generated Noise/Halftone pixmaps are built
/// at that resolution so their grain stays one pixel per pixmap pixel —
/// `bounds` itself, like the shader transforms here, stays in document units.
pub(super) fn to_sk_paint<'a>(paint: &crate::model::Paint, bounds: egui::Rect, opacity: f32, render_scale: f32, noise_storage: &'a mut Option<Pixmap>) -> Paint<'a> {
    let mut sk_paint = Paint { anti_alias: true, ..Default::default() };
    match paint {
        crate::model::Paint::Solid(c) => {
            sk_paint.set_color(to_color_with_opacity(*c, opacity));
        }
        crate::model::Paint::Gradient(g) => {
            let stops: Vec<tiny_skia::GradientStop> = g
                .stops
                .iter()
                .map(|s| tiny_skia::GradientStop::new(s.offset.clamp(0.0, 1.0), to_color_with_opacity(s.color, opacity)))
                .collect();
            let w = bounds.width().max(1e-3);
            let h = bounds.height().max(1e-3);
            let to_abs =
                |p: egui::Pos2| tiny_skia::Point::from_xy(bounds.min.x + p.x * w, bounds.min.y + p.y * h);
            let shader = match g.kind {
                crate::model::GradientKind::Linear => tiny_skia::LinearGradient::new(
                    to_abs(g.from),
                    to_abs(g.to),
                    stops,
                    tiny_skia::SpreadMode::Pad,
                    Transform::identity(),
                ),
                crate::model::GradientKind::Radial => {
                    let center = to_abs(g.from);
                    let edge = to_abs(g.to);
                    let radius = ((edge.x - center.x).powi(2) + (edge.y - center.y).powi(2)).sqrt().max(0.01);
                    tiny_skia::RadialGradient::new(
                        center,
                        0.0,
                        center,
                        radius,
                        stops,
                        tiny_skia::SpreadMode::Pad,
                        Transform::identity(),
                    )
                }
            };
            if let Some(shader) = shader {
                sk_paint.shader = shader;
            }
        }
        crate::model::Paint::Noise(n) => {
            let width = (bounds.width() * render_scale).round().max(1.0) as u32;
            let height = (bounds.height() * render_scale).round().max(1.0) as u32;
            if let Some(mut pixmap) = Pixmap::new(width, height) {
                for j in 0..height {
                    for i in 0..width {
                        // Pixel center mapped back to the same local-bounds
                        // document-unit space
                        // `canvas/texture_caches.rs::NoiseTextureCache` samples,
                        // one sample per pixmap pixel at `render_scale`.
                        let local_point = egui::Pos2::new((i as f32 + 0.5) / render_scale, (j as f32 + 0.5) / render_scale);
                        let color = crate::noise_fill::sample(n, local_point, opacity);
                        if let Some(px) = premultiplied(color) {
                            pixmap.pixels_mut()[(j * width + i) as usize] = px;
                        }
                    }
                }
                *noise_storage = Some(pixmap);
                let pixmap_ref = noise_storage.as_ref().expect("just assigned").as_ref();
                // Translates the pattern pixmap's own (0,0)-(width,height)
                // origin onto `bounds`' unrotated absolute position — same
                // role `to_abs` plays for the gradient shader above, and
                // subject to the same `layer_transform` composition this
                // function's doc comment describes. `1 / render_scale` maps the
                // pixmap's scaled-resolution pixels back to document units.
                let transform = Transform::from_translate(bounds.min.x, bounds.min.y).pre_scale(1.0 / render_scale, 1.0 / render_scale);
                sk_paint.shader = tiny_skia::Pattern::new(pixmap_ref, tiny_skia::SpreadMode::Pad, tiny_skia::FilterQuality::Nearest, 1.0, transform);
            }
        }
        crate::model::Paint::Halftone(h) => {
            // Same shape as the `Paint::Noise` arm above — generated
            // directly at full `bounds` resolution (not tiled from a
            // smaller source), so `SpreadMode::Pad` like Noise, not Repeat.
            let width = (bounds.width() * render_scale).round().max(1.0) as u32;
            let height = (bounds.height() * render_scale).round().max(1.0) as u32;
            if let Some(mut pixmap) = Pixmap::new(width, height) {
                for j in 0..height {
                    for i in 0..width {
                        let local_point = egui::Pos2::new((i as f32 + 0.5) / render_scale, (j as f32 + 0.5) / render_scale);
                        let color = crate::halftone_fill::sample(h, local_point, opacity);
                        if let Some(px) = premultiplied(color) {
                            pixmap.pixels_mut()[(j * width + i) as usize] = px;
                        }
                    }
                }
                *noise_storage = Some(pixmap);
                let pixmap_ref = noise_storage.as_ref().expect("just assigned").as_ref();
                let transform = Transform::from_translate(bounds.min.x, bounds.min.y).pre_scale(1.0 / render_scale, 1.0 / render_scale);
                sk_paint.shader = tiny_skia::Pattern::new(pixmap_ref, tiny_skia::SpreadMode::Pad, tiny_skia::FilterQuality::Nearest, 1.0, transform);
            }
        }
        crate::model::Paint::Pattern(p) => {
            // Unlike `Noise`/`Halftone`, this pixmap holds the actual
            // decoded tile image at its native resolution — the exact
            // conversion `draw_image` (this file's `Image`-layer renderer)
            // already uses for the same purpose.
            if let Some(decoded) = crate::image_ops::decode(&p.encoded) {
                let (width, height) = (decoded.width(), decoded.height());
                if width > 0 && height > 0
                    && let Some(size) = tiny_skia::IntSize::from_wh(width, height) {
                        let premultiplied = crate::image_ops::to_premultiplied_rgba(&decoded);
                        if let Some(pixmap) = Pixmap::from_vec(premultiplied, size) {
                            *noise_storage = Some(pixmap);
                            let pixmap_ref = noise_storage.as_ref().expect("just assigned").as_ref();
                            let tile_width = p.tile_width.max(0.01);
                            let tile_height = tile_width * (height as f32 / width as f32);
                            // Scales the pixmap's native (width, height)
                            // pixels down to one (tile_width, tile_height)
                            // tile, then translates to `bounds`' unrotated
                            // absolute position — `SpreadMode::Repeat` is
                            // what makes it tile across the rest of the fill.
                            let transform = Transform::from_translate(bounds.min.x, bounds.min.y)
                                .pre_scale(tile_width / width as f32, tile_height / height as f32);
                            // Unlike Noise/Halftone, opacity isn't baked
                            // per-pixel here — the decoded tile is used
                            // as-is, so it's applied via `Pattern`'s own
                            // `opacity` argument instead.
                            sk_paint.shader = tiny_skia::Pattern::new(
                                pixmap_ref,
                                tiny_skia::SpreadMode::Repeat,
                                tiny_skia::FilterQuality::Bilinear,
                                opacity.clamp(0.0, 1.0),
                                transform,
                            );
                        }
                    }
            }
        }
    }
    sk_paint
}

/// Premultiplies `color`'s RGB by its own alpha — the representation
/// `tiny_skia::Pixmap`'s raw pixel storage requires (unlike `tiny_skia::Paint::set_color`,
/// which accepts straight alpha and premultiplies internally at fill time).
pub(super) fn premultiplied(color: egui::Color32) -> Option<PremultipliedColorU8> {
    let a = color.a() as u16;
    let mul = |v: u8| ((v as u16 * a) / 255) as u8;
    PremultipliedColorU8::from_rgba(mul(color.r()), mul(color.g()), mul(color.b()), color.a())
}

#[cfg(test)]
mod tests {
    use crate::export::render_layer;
    use crate::model::{CornerRadii, Frame, Layer, LayerKind};
    use egui::{Color32, Pos2};

    #[test]
    fn linear_gradient_fill_interpolates_left_to_right() {
        let mut layer = Layer::new(
            "Rect",
            Frame::from_two_points(Pos2::new(0.0, 0.0), Pos2::new(100.0, 20.0)),
            LayerKind::Rectangle { corner_radius: CornerRadii::ZERO },
        );
        layer.style.fill = Some(crate::model::Paint::Gradient(crate::model::Gradient::linear(
            Color32::from_rgb(255, 0, 0),
            Color32::from_rgb(0, 0, 255),
        )));
        layer.style.stroke = None;

        let pixmap = render_layer(&layer).expect("should render");
        let left = pixmap.pixel(1, 10).expect("left pixel");
        let right = pixmap.pixel(98, 10).expect("right pixel");
        assert!(left.red() > 200 && left.blue() < 50, "left edge should be near the start color (red)");
        assert!(right.blue() > 200 && right.red() < 50, "right edge should be near the end color (blue)");
    }

    #[test]
    fn radial_gradient_fill_is_brightest_at_center() {
        let mut layer = Layer::new(
            "Oval",
            Frame::from_two_points(Pos2::new(0.0, 0.0), Pos2::new(80.0, 80.0)),
            LayerKind::Oval,
        );
        layer.style.fill = Some(crate::model::Paint::Gradient(crate::model::Gradient::radial(
            Color32::from_rgb(255, 255, 255),
            Color32::from_rgb(0, 0, 0),
        )));
        layer.style.stroke = None;

        let pixmap = render_layer(&layer).expect("should render");
        let center = pixmap.pixel(40, 40).expect("center pixel");
        let edge = pixmap.pixel(40, 41 + 38).expect("near-edge pixel");
        assert!(center.red() > edge.red(), "radial gradient should be brightest at its center");
    }

    #[test]
    fn gradient_stroke_interpolates_along_a_line() {
        let mut layer = Layer::new(
            "Line",
            Frame::from_two_points(Pos2::new(0.0, 0.0), Pos2::new(100.0, 0.0)),
            LayerKind::Line,
        );
        layer.style.fill = None;
        layer.style.stroke = Some(crate::model::Stroke {
            paint: crate::model::Paint::Gradient(crate::model::Gradient::linear(
                Color32::from_rgb(255, 0, 0),
                Color32::from_rgb(0, 0, 255),
            )),
            width: 6.0,
        });

        let pixmap = render_layer(&layer).expect("should render");
        let left = pixmap.pixel(2, pixmap.height() / 2).expect("left pixel");
        let right = pixmap.pixel(pixmap.width() - 3, pixmap.height() / 2).expect("right pixel");
        assert!(left.red() > 200 && left.blue() < 50, "stroke start should be near the start color (red)");
        assert!(right.blue() > 200 && right.red() < 50, "stroke end should be near the end color (blue)");
    }

    fn noise_rect_layer(seed: u32) -> Layer {
        let mut layer = Layer::new(
            "Rect",
            Frame::from_two_points(Pos2::new(0.0, 0.0), Pos2::new(60.0, 60.0)),
            LayerKind::Rectangle { corner_radius: CornerRadii::ZERO },
        );
        layer.style.fill = Some(crate::model::Paint::Noise(crate::model::NoiseFill {
            base: Color32::from_rgb(128, 128, 128),
            intensity: 0.8,
            scale: 3.0,
            seed,
        }));
        layer.style.stroke = None;
        layer
    }

    #[test]
    fn noise_fill_renders_grain_not_a_flat_color() {
        let pixmap = render_layer(&noise_rect_layer(7)).expect("should render");
        let colors: std::collections::HashSet<[u8; 4]> =
            pixmap.pixels().iter().map(|p| [p.red(), p.green(), p.blue(), p.alpha()]).collect();
        assert!(colors.len() > 1, "noise fill should vary pixel-to-pixel, not render as a single flat color");
    }

    #[test]
    fn noise_fill_is_deterministic_for_the_same_seed() {
        let a = render_layer(&noise_rect_layer(7)).expect("should render");
        let b = render_layer(&noise_rect_layer(7)).expect("should render");
        assert_eq!(a.data(), b.data(), "the same seed should always render the same grain pattern");
    }

    #[test]
    fn noise_fill_differs_across_seeds() {
        let a = render_layer(&noise_rect_layer(7)).expect("should render");
        let b = render_layer(&noise_rect_layer(8)).expect("should render");
        assert_ne!(a.data(), b.data(), "different seeds should render different grain patterns");
    }

    fn halftone_rect_layer() -> Layer {
        let mut layer = Layer::new(
            "Rect",
            Frame::from_two_points(Pos2::new(0.0, 0.0), Pos2::new(60.0, 60.0)),
            LayerKind::Rectangle { corner_radius: CornerRadii::ZERO },
        );
        layer.style.fill = Some(crate::model::Paint::Halftone(crate::model::HalftoneFill {
            background: Color32::WHITE,
            dot: Color32::BLACK,
            scale: 10.0,
            coverage: 0.8,
        }));
        layer.style.stroke = None;
        layer
    }

    #[test]
    fn halftone_fill_renders_a_dot_grid_not_a_flat_color() {
        let pixmap = render_layer(&halftone_rect_layer()).expect("should render");
        let colors: std::collections::HashSet<[u8; 4]> =
            pixmap.pixels().iter().map(|p| [p.red(), p.green(), p.blue(), p.alpha()]).collect();
        assert!(colors.len() > 1, "halftone fill should show both dot and background colors, not a single flat color");
    }

    fn two_color_tile_png() -> Vec<u8> {
        let mut img = image::RgbaImage::new(2, 1);
        img.put_pixel(0, 0, image::Rgba([255, 0, 0, 255]));
        img.put_pixel(1, 0, image::Rgba([0, 0, 255, 255]));
        crate::image_ops::encode_png(&img)
    }

    fn pattern_rect_layer(tile_width: f32) -> Layer {
        let mut layer = Layer::new(
            "Rect",
            Frame::from_two_points(Pos2::new(0.0, 0.0), Pos2::new(80.0, 20.0)),
            LayerKind::Rectangle { corner_radius: CornerRadii::ZERO },
        );
        layer.style.fill = Some(crate::model::Paint::Pattern(crate::model::PatternFill {
            encoded: two_color_tile_png(),
            tile_width,
        }));
        layer.style.stroke = None;
        layer
    }

    #[test]
    fn pattern_fill_contains_the_tile_images_colors() {
        let pixmap = render_layer(&pattern_rect_layer(20.0)).expect("should render");
        let has_reddish = pixmap.pixels().iter().any(|p| p.red() > 150 && p.blue() < 100);
        let has_blueish = pixmap.pixels().iter().any(|p| p.blue() > 150 && p.red() < 100);
        assert!(has_reddish && has_blueish, "pattern fill should contain both of the tile image's colors");
    }

    #[test]
    fn pattern_fill_tiles_the_same_color_reappears_at_tile_width_offset() {
        let tile_width = 20.0;
        let pixmap = render_layer(&pattern_rect_layer(tile_width)).expect("should render");
        let y = pixmap.height() / 2;
        let first = pixmap.pixel(2, y).expect("first tile pixel");
        let second = pixmap.pixel(2 + tile_width as u32, y).expect("second tile pixel");
        assert!(first.red() > 150 && first.blue() < 100, "first tile should start reddish");
        assert!(second.red() > 150 && second.blue() < 100, "pattern should repeat: second tile should also start reddish");
    }
}
