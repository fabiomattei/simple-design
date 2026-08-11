use egui::{Color32, Pos2};

use crate::model::NoiseFill;

/// Deterministic hash of a `(seed, cell)` triple to a byte, 0-255. Same
/// inputs always produce the same output — this is what lets `canvas.rs`
/// and `export.rs` independently rasterize the same grain pattern at
/// different resolutions without sharing a texture. Wang-hash-style integer
/// mixing; not cryptographic, just enough avalanche to avoid visible grid
/// artifacts.
fn cell_value(seed: u32, cell_x: i32, cell_y: i32) -> u8 {
    let mut x = seed ^ (cell_x as u32).wrapping_mul(0x1f1f_1f1f) ^ (cell_y as u32).wrapping_mul(0x2f2f_2f2f);
    x ^= x >> 16;
    x = x.wrapping_mul(0x7feb_352d);
    x ^= x >> 15;
    x = x.wrapping_mul(0x846c_a68b);
    x ^= x >> 16;
    (x & 0xff) as u8
}

/// The noise fill's color at `local_point`, expressed in the same
/// unrotated-local-bounds space `Gradient::from`/`to` and
/// `gradient_color_at_screen_point` use (see `canvas.rs`). `opacity` is the
/// caller's already-combined layer/fill opacity, baked into the returned
/// alpha — same convention `export.rs::to_sk_paint`'s gradient branch uses
/// per stop via `to_color_with_opacity`.
pub fn sample(fill: &NoiseFill, local_point: Pos2, opacity: f32) -> Color32 {
    let scale = fill.scale.max(0.01);
    let cell_x = (local_point.x / scale).floor() as i32;
    let cell_y = (local_point.y / scale).floor() as i32;
    let deviation = (cell_value(fill.seed, cell_x, cell_y) as f32 / 255.0 - 0.5) * 2.0 * fill.intensity;
    let factor = 1.0 + deviation;
    let apply = |channel: u8| (channel as f32 * factor).round().clamp(0.0, 255.0) as u8;
    let alpha = (fill.base.a() as f32 * opacity).round().clamp(0.0, 255.0) as u8;
    Color32::from_rgba_unmultiplied(apply(fill.base.r()), apply(fill.base.g()), apply(fill.base.b()), alpha)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_fill() -> NoiseFill {
        NoiseFill { base: Color32::from_rgb(128, 128, 128), intensity: 0.5, scale: 4.0, seed: 42 }
    }

    #[test]
    fn same_seed_and_point_produce_same_color() {
        let fill = sample_fill();
        let a = sample(&fill, Pos2::new(10.0, 10.0), 1.0);
        let b = sample(&fill, Pos2::new(10.0, 10.0), 1.0);
        assert_eq!(a, b);
    }

    #[test]
    fn different_seeds_produce_different_grain() {
        let mut fill = sample_fill();
        let a = sample(&fill, Pos2::new(10.0, 10.0), 1.0);
        fill.seed = 43;
        let b = sample(&fill, Pos2::new(10.0, 10.0), 1.0);
        assert_ne!(a, b);
    }

    #[test]
    fn zero_intensity_returns_flat_base_color() {
        let mut fill = sample_fill();
        fill.intensity = 0.0;
        for (x, y) in [(0.0, 0.0), (5.0, 5.0), (37.0, 12.0)] {
            let c = sample(&fill, Pos2::new(x, y), 1.0);
            assert_eq!(c, fill.base);
        }
    }

    #[test]
    fn opacity_scales_alpha() {
        let fill = sample_fill();
        let c = sample(&fill, Pos2::new(1.0, 1.0), 0.5);
        assert_eq!(c.a(), 128);
    }
}
