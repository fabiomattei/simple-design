//! Fill and stroke geometry for the canvas renderer: even-odd ring sets,
//! earcut triangulation, gradient/texture-sampled meshes and the gradient
//! stroke approximation.

use egui::{Color32, Pos2, Rect, Stroke as EguiStroke};

use crate::model::{Gradient, HalftoneFill, Layer, NoiseFill, Paint, PathPolygon, PatternFill};
use crate::shapes::rotate_point;

use super::texture_caches::{HalftoneTextureCache, NoiseTextureCache, PatternTextureCache};

/// Scales `color`'s alpha channel by `opacity` (`0.0..=1.0`), used to apply a
/// layer's (and its ancestors', multiplicatively accumulated) opacity to its
/// fill/stroke without needing an offscreen compositing pass.
pub(super) fn with_opacity(color: Color32, opacity: f32) -> Color32 {
    let a = (color.a() as f32 * opacity.clamp(0.0, 1.0)).round() as u8;
    Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), a)
}

/// Draws a set of even-odd-filled rings (each exterior + its holes) using
/// `layer`'s own `style` — shared by `CompoundPath` (its own stored
/// `polygons`) and `BooleanGroup` (`boolean_ops::compute_boolean_group`'s
/// live result, converted via `boolean_ops::multipolygon_to_polygons`), so
/// the two can't silently diverge in how a ring set gets rasterized on
/// canvas. `polygons`' points are relative to `layer.frame.pos`, same
/// convention `PathPolygon`'s own doc comment describes.
#[allow(clippy::too_many_arguments)]
pub(super) fn draw_even_odd_polygons(
    painter: &egui::Painter,
    ctx: &egui::Context,
    noise_cache: &mut NoiseTextureCache,
    halftone_cache: &mut HalftoneTextureCache,
    pattern_cache: &mut PatternTextureCache,
    polygons: &[PathPolygon],
    layer: &Layer,
    fill: Color32,
    stroke: EguiStroke,
    opacity: f32,
    to_screen: impl Fn(Pos2) -> Pos2,
) {
    let bounds = layer.frame.bounds();
    let screen_rect = Rect::from_two_pos(to_screen(bounds.min), to_screen(bounds.max));
    let screen_center = to_screen(bounds.center());
    let rotation = layer.frame.rotation;
    let fill_gradient = match layer.style.fill.as_ref() {
        Some(Paint::Gradient(g)) => Some(g),
        _ => None,
    };
    let fill_noise = match layer.style.fill.as_ref() {
        Some(Paint::Noise(n)) => Some(n),
        _ => None,
    };
    let fill_halftone = match layer.style.fill.as_ref() {
        Some(Paint::Halftone(h)) => Some(h),
        _ => None,
    };
    let fill_pattern = match layer.style.fill.as_ref() {
        Some(Paint::Pattern(p)) => Some(p),
        _ => None,
    };
    let stroke_gradient = match layer.style.stroke.as_ref().map(|s| &s.paint) {
        Some(Paint::Gradient(g)) => Some(g),
        _ => None,
    };
    for poly in polygons {
        let ext_screen: Vec<Pos2> = poly
            .exterior
            .iter()
            .map(|p| rotate_point(to_screen(layer.frame.pos + p.to_vec2()), screen_center, rotation))
            .collect();
        let holes_screen: Vec<Vec<Pos2>> = poly
            .holes
            .iter()
            .map(|h| {
                h.iter()
                    .map(|p| rotate_point(to_screen(layer.frame.pos + p.to_vec2()), screen_center, rotation))
                    .collect()
            })
            .collect();

        // Fill via earcut triangulation: it natively handles holes,
        // unlike `PathShape`'s fill, which only covers a single ring.
        if layer.style.fill.is_some() {
            let mesh = if let Some(g) = fill_gradient {
                let color_at = |p: Pos2| {
                    gradient_color_at_screen_point(g, p, screen_rect, screen_center, rotation, opacity * layer.style.fill_opacity)
                };
                triangulate_fill_with(&ext_screen, &holes_screen, color_at)
            } else if let Some(n) = fill_noise {
                noise_textured_mesh(ctx, noise_cache, layer, n, &ext_screen, &holes_screen, screen_rect, screen_center, rotation, opacity)
            } else if let Some(h) = fill_halftone {
                halftone_textured_mesh(ctx, halftone_cache, layer, h, &ext_screen, &holes_screen, screen_rect, screen_center, rotation, opacity)
            } else if let Some(p) = fill_pattern {
                pattern_textured_mesh(ctx, pattern_cache, layer, p, &ext_screen, &holes_screen, screen_rect, screen_center, rotation, opacity)
            } else {
                triangulate_fill(&ext_screen, &holes_screen, fill)
            };
            if let Some(mesh) = mesh {
                painter.add(egui::Shape::mesh(mesh));
            }
        }

        // Stroke outline: exterior + every hole as its own closed
        // unfilled path (PathShape's fill is skipped via TRANSPARENT
        // since the mesh above already handled filling). A gradient stroke
        // uses `paint_gradient_stroke`'s segmented approximation instead
        // (same tradeoff as every other shape kind — see its doc comment).
        if let Some(g) = stroke_gradient {
            let opacity = opacity * layer.style.stroke_opacity;
            for ring in std::iter::once(&ext_screen).chain(holes_screen.iter()) {
                paint_gradient_stroke(painter, ring, true, g, screen_rect, screen_center, rotation, stroke.width, opacity);
            }
        } else if layer.style.stroke.is_some() {
            for ring in std::iter::once(&ext_screen).chain(holes_screen.iter()) {
                if ring.len() >= 2 {
                    painter.add(egui::Shape::Path(egui::epaint::PathShape {
                        points: ring.clone(),
                        closed: true,
                        fill: Color32::TRANSPARENT,
                        stroke: stroke.into(),
                    }));
                }
            }
        }
    }
}

/// Shared earcut step for `triangulate_fill_with`/`triangulate_fill_textured`:
/// flattens `exterior`/`holes` (already in screen space) into the vertex
/// positions and index list `earcutr` produces, before either per-vertex
/// color or UV is attached — `egui::epaint::PathShape`'s own fill is a fan
/// from vertex 0 that's only correct for convex polygons, hence earcut.
pub(super) fn earcut_polygon(exterior: &[Pos2], holes: &[Vec<Pos2>]) -> Option<(Vec<Pos2>, Vec<u32>)> {
    if exterior.len() < 3 {
        return None;
    }
    let mut flat: Vec<f64> = Vec::new();
    let mut hole_indices = Vec::new();
    for p in exterior {
        flat.push(p.x as f64);
        flat.push(p.y as f64);
    }
    for hole in holes {
        hole_indices.push(flat.len() / 2);
        for p in hole {
            flat.push(p.x as f64);
            flat.push(p.y as f64);
        }
    }
    let indices = earcutr::earcut(&flat, &hole_indices, 2).ok()?;
    let positions: Vec<Pos2> = flat.chunks_exact(2).map(|c| Pos2::new(c[0] as f32, c[1] as f32)).collect();
    Some((positions, indices.into_iter().map(|i| i as u32).collect()))
}

/// Triangulates a polygon-with-holes (already in screen space) via `earcutr`
/// so it can be filled correctly even with holes, which `egui::epaint::PathShape`
/// (a single-ring fan/ear-clip) can't represent. Each vertex's color comes from
/// `color_at`, so a flat closure gives the old single-color behavior while a
/// gradient-sampling one (see `paint_polygon`) gets a properly interpolated
/// gradient fill for free from the same triangulation.
pub(super) fn triangulate_fill_with(exterior: &[Pos2], holes: &[Vec<Pos2>], color_at: impl Fn(Pos2) -> Color32) -> Option<egui::Mesh> {
    let (positions, indices) = earcut_polygon(exterior, holes)?;
    Some(egui::Mesh {
        vertices: positions.into_iter().map(|p| egui::epaint::Vertex::untextured(p, color_at(p))).collect(),
        indices,
        ..Default::default()
    })
}

pub(super) fn triangulate_fill(exterior: &[Pos2], holes: &[Vec<Pos2>], color: Color32) -> Option<egui::Mesh> {
    triangulate_fill_with(exterior, holes, |_| color)
}

/// The `Paint::Noise` sibling of `triangulate_fill_with`'s gradient path:
/// same earcut triangulation, but each vertex samples `texture_id` at
/// `uv_at(vertex)` instead of carrying an interpolated flat color, and
/// `tint` (typically opacity-only white, see `Image` layers' own textured
/// path) is multiplied over the sampled texel by egui's mesh renderer.
pub(super) fn triangulate_fill_textured(
    exterior: &[Pos2],
    holes: &[Vec<Pos2>],
    texture_id: egui::TextureId,
    uv_at: impl Fn(Pos2) -> Pos2,
    tint: Color32,
) -> Option<egui::Mesh> {
    let (positions, indices) = earcut_polygon(exterior, holes)?;
    let mut mesh = egui::Mesh::with_texture(texture_id);
    mesh.vertices = positions.into_iter().map(|p| egui::epaint::Vertex { pos: p, uv: uv_at(p), color: tint }).collect();
    mesh.indices = indices;
    Some(mesh)
}

/// The gradient-sampled color at absolute screen point `p`, undoing
/// `rotation` about `center` first so the gradient stays fixed to the
/// shape as it rotates (`Gradient`'s normalized `from`/`to` are relative to
/// the shape's *unrotated* bounding box — see its doc comment), then
/// normalizing against `screen_rect` (that same unrotated bounding box, in
/// screen space) before sampling.
pub(super) fn gradient_color_at_screen_point(gradient: &Gradient, p: Pos2, screen_rect: Rect, center: Pos2, rotation: f32, opacity: f32) -> Color32 {
    let local = rotate_point(p, center, -rotation);
    let w = screen_rect.width().max(1e-3);
    let h = screen_rect.height().max(1e-3);
    let u = (local.x - screen_rect.min.x) / w;
    let v = (local.y - screen_rect.min.y) / h;
    with_opacity(gradient.sample_normalized(Pos2::new(u, v)), opacity)
}

/// Builds (or reuses, via `noise_cache`) `n`'s grain texture and tessellates
/// `exterior`/`holes` (already in screen space) into a UV-mapped mesh
/// sampling it — the `Paint::Noise` sibling of `triangulate_fill_with`'s
/// gradient path. Each vertex's `uv` is its fractional position within the
/// shape's *unrotated* `screen_rect` (rotation undone about `center` first,
/// same normalization `gradient_color_at_screen_point` uses above) — this
/// lines up with how `NoiseTextureCache::get_or_build` rasterized the
/// texture in the first place, so UV `(0,0)`-`(1,1)` always covers exactly
/// one copy of the grain, whatever pixel resolution the texture was built
/// at. Opacity is applied via `tint`, not baked into the texture — same
/// convention `Image` layers use — so a fill-opacity slider drag doesn't
/// force a texture rebuild.
#[allow(clippy::too_many_arguments)]
pub(super) fn noise_textured_mesh(
    ctx: &egui::Context,
    noise_cache: &mut NoiseTextureCache,
    layer: &Layer,
    n: &NoiseFill,
    exterior: &[Pos2],
    holes: &[Vec<Pos2>],
    screen_rect: Rect,
    center: Pos2,
    rotation: f32,
    opacity: f32,
) -> Option<egui::Mesh> {
    let texture = noise_cache.get_or_build(ctx, layer.id, n, layer.frame.bounds().size(), screen_rect.size())?;
    let uv_at = |p: Pos2| {
        let local = rotate_point(p, center, -rotation);
        let w = screen_rect.width().max(1e-3);
        let h = screen_rect.height().max(1e-3);
        Pos2::new((local.x - screen_rect.min.x) / w, (local.y - screen_rect.min.y) / h)
    };
    let tint = with_opacity(Color32::WHITE, opacity * layer.style.fill_opacity);
    triangulate_fill_textured(exterior, holes, texture.id(), uv_at, tint)
}

/// `Paint::Halftone`'s sibling of `noise_textured_mesh` — identical shape,
/// backed by `HalftoneTextureCache`/`halftone_fill::sample` instead.
#[allow(clippy::too_many_arguments)]
pub(super) fn halftone_textured_mesh(
    ctx: &egui::Context,
    halftone_cache: &mut HalftoneTextureCache,
    layer: &Layer,
    h: &HalftoneFill,
    exterior: &[Pos2],
    holes: &[Vec<Pos2>],
    screen_rect: Rect,
    center: Pos2,
    rotation: f32,
    opacity: f32,
) -> Option<egui::Mesh> {
    let texture = halftone_cache.get_or_build(ctx, layer.id, h, layer.frame.bounds().size(), screen_rect.size())?;
    let uv_at = |p: Pos2| {
        let local = rotate_point(p, center, -rotation);
        let w = screen_rect.width().max(1e-3);
        let h = screen_rect.height().max(1e-3);
        Pos2::new((local.x - screen_rect.min.x) / w, (local.y - screen_rect.min.y) / h)
    };
    let tint = with_opacity(Color32::WHITE, opacity * layer.style.fill_opacity);
    triangulate_fill_textured(exterior, holes, texture.id(), uv_at, tint)
}

/// `Paint::Pattern`'s sibling of `noise_textured_mesh` — differs only in
/// its `uv_at`: instead of a `0..1` fraction covering the *whole* texture,
/// it converts the screen point to local document-space units (the same
/// fraction-of-`screen_rect` × `layer.frame.bounds().size()` trick, so no
/// raw `zoom` needs threading through this call chain) and divides by the
/// pattern's tile size — values past `1.0` are exactly what makes the
/// mesh tile, since `PatternTextureCache` loads its texture with
/// `TextureWrapMode::Repeat`.
#[allow(clippy::too_many_arguments)]
pub(super) fn pattern_textured_mesh(
    ctx: &egui::Context,
    pattern_cache: &mut PatternTextureCache,
    layer: &Layer,
    p: &PatternFill,
    exterior: &[Pos2],
    holes: &[Vec<Pos2>],
    screen_rect: Rect,
    center: Pos2,
    rotation: f32,
    opacity: f32,
) -> Option<egui::Mesh> {
    let (texture, aspect) = pattern_cache.get_or_build(ctx, layer.id, p)?;
    let local_size = layer.frame.bounds().size();
    let tile_width = p.tile_width.max(0.01);
    let tile_height = (tile_width * aspect).max(0.01);
    let uv_at = |pt: Pos2| {
        let local = rotate_point(pt, center, -rotation);
        let w = screen_rect.width().max(1e-3);
        let h = screen_rect.height().max(1e-3);
        let frac = Pos2::new((local.x - screen_rect.min.x) / w, (local.y - screen_rect.min.y) / h);
        let local_units = Pos2::new(frac.x * local_size.x, frac.y * local_size.y);
        Pos2::new(local_units.x / tile_width, local_units.y / tile_height)
    };
    let tint = with_opacity(Color32::WHITE, opacity * layer.style.fill_opacity);
    triangulate_fill_textured(exterior, holes, texture.id(), uv_at, tint)
}

/// How many solid-colored sub-segments approximate one gradient-stroked
/// polygon edge — see `paint_gradient_stroke`'s doc comment.
pub(super) const GRADIENT_STROKE_SEGMENTS_PER_EDGE: usize = 10;

/// Approximates a gradient-colored stroke by subdividing the outline
/// (`points`, already in absolute screen space) into short solid-colored
/// segments, each sampled at its own midpoint. `egui` has no per-vertex-color
/// stroke primitive to interpolate properly like `triangulate_fill_with` does
/// for fill, and building real mitered stroke-ribbon geometry here is more
/// than a live-editing preview needs — the PNG exporter (`export/`) still
/// renders the exact gradient via a `tiny-skia` shader, so export quality
/// isn't affected by this approximation.
#[allow(clippy::too_many_arguments)]
pub(super) fn paint_gradient_stroke(
    painter: &egui::Painter,
    points: &[Pos2],
    closed: bool,
    gradient: &Gradient,
    screen_rect: Rect,
    center: Pos2,
    rotation: f32,
    width: f32,
    opacity: f32,
) {
    if points.len() < 2 || width <= 0.0 {
        return;
    }
    let n = points.len();
    let edge_count = if closed { n } else { n - 1 };
    for i in 0..edge_count {
        let a = points[i];
        let b = points[(i + 1) % n];
        for s in 0..GRADIENT_STROKE_SEGMENTS_PER_EDGE {
            let t0 = s as f32 / GRADIENT_STROKE_SEGMENTS_PER_EDGE as f32;
            let t1 = (s + 1) as f32 / GRADIENT_STROKE_SEGMENTS_PER_EDGE as f32;
            let p0 = a + (b - a) * t0;
            let p1 = a + (b - a) * t1;
            let mid = a + (b - a) * ((t0 + t1) * 0.5);
            let color = gradient_color_at_screen_point(gradient, mid, screen_rect, center, rotation, opacity);
            painter.line_segment([p0, p1], EguiStroke::new(width, color));
        }
    }
}

/// Fills+strokes a screen-space polygon (`points`, already rotated) using
/// `layer`'s own `Style::fill`/`stroke`. When both resolve to a flat color,
/// this is exactly the single `PathShape` `draw_layer` always drew (`fill_flat`/
/// `stroke_flat` are the same values it already computed) — a `Paint::Gradient`
/// fill instead triangulates via `triangulate_fill_with` and a gradient stroke
/// falls back to `paint_gradient_stroke`'s segmented approximation.
/// `force_transparent_fill` mirrors the open-`Path` "can't fill an open path"
/// case `draw_layer`'s `Path` arm already handled.
#[allow(clippy::too_many_arguments)]
pub(super) fn paint_polygon(
    painter: &egui::Painter,
    ctx: &egui::Context,
    noise_cache: &mut NoiseTextureCache,
    halftone_cache: &mut HalftoneTextureCache,
    pattern_cache: &mut PatternTextureCache,
    points: Vec<Pos2>,
    closed: bool,
    screen_rect: Rect,
    center: Pos2,
    rotation: f32,
    layer: &Layer,
    fill_flat: Color32,
    stroke_flat: EguiStroke,
    opacity: f32,
    force_transparent_fill: bool,
) {
    let fill_paint = layer.style.fill.as_ref();

    // Fill is always triangulated via earcut (`triangulate_fill`/
    // `triangulate_fill_with`), never `PathShape`'s own fill: that fill is a
    // fan from vertex 0 that's only correct for convex polygons (see its doc
    // comment, "convex area") — a concave shape like `Star` produces stray
    // triangles (most visibly one jutting out past the top point) since the
    // fan crosses outside the outline between non-adjacent concave vertices.
    if !force_transparent_fill {
        if let Some(Paint::Gradient(g)) = fill_paint {
            let color_at =
                |p: Pos2| gradient_color_at_screen_point(g, p, screen_rect, center, rotation, opacity * layer.style.fill_opacity);
            if let Some(mesh) = triangulate_fill_with(&points, &[], color_at) {
                painter.add(egui::Shape::mesh(mesh));
            }
        } else if let Some(Paint::Noise(n)) = fill_paint {
            if let Some(mesh) = noise_textured_mesh(ctx, noise_cache, layer, n, &points, &[], screen_rect, center, rotation, opacity) {
                painter.add(egui::Shape::mesh(mesh));
            }
        } else if let Some(Paint::Halftone(h)) = fill_paint {
            if let Some(mesh) = halftone_textured_mesh(ctx, halftone_cache, layer, h, &points, &[], screen_rect, center, rotation, opacity) {
                painter.add(egui::Shape::mesh(mesh));
            }
        } else if let Some(Paint::Pattern(p)) = fill_paint {
            if let Some(mesh) = pattern_textured_mesh(ctx, pattern_cache, layer, p, &points, &[], screen_rect, center, rotation, opacity) {
                painter.add(egui::Shape::mesh(mesh));
            }
        } else if fill_flat != Color32::TRANSPARENT
            && let Some(mesh) = triangulate_fill(&points, &[], fill_flat) {
                painter.add(egui::Shape::mesh(mesh));
            }
    }

    if let Some(stroke) = layer.style.stroke.as_ref() {
        if let Paint::Gradient(g) = &stroke.paint {
            paint_gradient_stroke(
                painter,
                &points,
                closed,
                g,
                screen_rect,
                center,
                rotation,
                stroke_flat.width,
                opacity * layer.style.stroke_opacity,
            );
        } else if stroke_flat.color != Color32::TRANSPARENT {
            painter.add(egui::Shape::Path(egui::epaint::PathShape {
                points,
                closed,
                fill: Color32::TRANSPARENT,
                stroke: stroke_flat.into(),
            }));
        }
    }
}
