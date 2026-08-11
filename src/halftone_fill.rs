use egui::{Color32, Pos2};

use crate::model::HalftoneFill;

/// How wide (in local-bounds units) the soft edge around a dot's radius is.
/// A hard threshold would alias visibly at any zoom/export resolution — see
/// `noise_fill`'s doc comment for why this module doesn't rely on texture
/// filtering for that instead (`canvas.rs`'s cache loads it `NEAREST`, same
/// as noise, for crispness at the grid's own hard edges elsewhere).
const EDGE_SOFTNESS: f32 = 0.75;

fn smoothstep(edge0: f32, edge1: f32, x: f32) -> f32 {
    let t = ((x - edge0) / (edge1 - edge0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// The halftone fill's color at `local_point`, in the same
/// unrotated-local-bounds space `noise_fill::sample` uses. A grid of dots
/// spaced `fill.scale` apart, each `fill.coverage` of the cell's half-width
/// in radius; odd rows shift half a cell horizontally (the classic
/// staggered/brick halftone look, not a plain square grid). `opacity` is
/// baked into the returned alpha, same convention as `noise_fill::sample`.
pub fn sample(fill: &HalftoneFill, local_point: Pos2, opacity: f32) -> Color32 {
    let scale = fill.scale.max(0.01);
    let row = (local_point.y / scale).floor();
    let stagger = if (row as i64).rem_euclid(2) == 1 { scale * 0.5 } else { 0.0 };
    let cell_x = ((local_point.x + stagger) / scale).floor();
    let cell_center = Pos2::new((cell_x + 0.5) * scale - stagger, (row + 0.5) * scale);
    let dist = (local_point - cell_center).length();
    let radius = (fill.coverage.clamp(0.0, 1.0) * scale * 0.5).max(0.0);
    // 1.0 inside the dot, 0.0 outside, smoothed across `EDGE_SOFTNESS`
    // local units at the boundary.
    let dot_weight = 1.0 - smoothstep(radius - EDGE_SOFTNESS * 0.5, radius + EDGE_SOFTNESS * 0.5, dist);
    let lerp = |a: u8, b: u8| (a as f32 + (b as f32 - a as f32) * dot_weight).round().clamp(0.0, 255.0) as u8;
    let base = fill.background;
    let dot = fill.dot;
    let r = lerp(base.r(), dot.r());
    let g = lerp(base.g(), dot.g());
    let b = lerp(base.b(), dot.b());
    let a_base = base.a() as f32 + (dot.a() as f32 - base.a() as f32) * dot_weight;
    let alpha = (a_base * opacity).round().clamp(0.0, 255.0) as u8;
    Color32::from_rgba_unmultiplied(r, g, b, alpha)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_fill() -> HalftoneFill {
        HalftoneFill {
            background: Color32::WHITE,
            dot: Color32::BLACK,
            scale: 10.0,
            coverage: 0.8,
        }
    }

    #[test]
    fn center_of_a_dot_is_the_dot_color() {
        let fill = sample_fill();
        // Row 0 is unstaggered, so its dot center is at (scale/2, scale/2).
        let c = sample(&fill, Pos2::new(5.0, 5.0), 1.0);
        assert_eq!(c, Color32::BLACK);
    }

    #[test]
    fn far_from_any_dot_center_is_the_background_color() {
        let fill = sample_fill();
        // A cell corner is maximally far from every neighboring dot center.
        let c = sample(&fill, Pos2::new(0.0, 0.0), 1.0);
        assert_eq!(c, Color32::WHITE);
    }

    #[test]
    fn odd_rows_are_staggered_by_half_a_cell() {
        let fill = sample_fill();
        // Row 1's dot center is at (scale, 1.5*scale) after the stagger,
        // not (scale/2, 1.5*scale) — sampling the unstaggered position
        // should land in the background, not the dot.
        let unstaggered = sample(&fill, Pos2::new(5.0, 15.0), 1.0);
        let staggered = sample(&fill, Pos2::new(10.0, 15.0), 1.0);
        assert_eq!(unstaggered, Color32::WHITE);
        assert_eq!(staggered, Color32::BLACK);
    }

    #[test]
    fn opacity_scales_alpha() {
        let fill = sample_fill();
        let c = sample(&fill, Pos2::new(5.0, 5.0), 0.5);
        assert_eq!(c.a(), 128);
    }
}
