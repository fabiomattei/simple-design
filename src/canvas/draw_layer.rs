//! Paints the layer tree onto the canvas with egui's painter — the
//! interactive counterpart of `export/mod.rs`'s `draw_layer`.

use egui::{Color32, Pos2, Rect, Stroke as EguiStroke, Vec2};

use crate::model::{Layer, LayerId, LayerKind, Page, Paint, TextAlign, TextResize, VerticalAlign};
use crate::shapes::{ellipse_points, rotate_point, rounded_rect_points};
use crate::text_layout::{self, TextStyleParams};

use super::fill_mesh::{draw_even_odd_polygons, paint_gradient_stroke, paint_polygon, with_opacity};
use super::path_flatten::flatten_path;
use super::texture_caches::{
    HalftoneTextureCache, ImageTextureCache, MaskedGroupTextureCache, NoiseTextureCache, PatternTextureCache,
    ShadowTextureCache,
};

/// Draws one parent's `children` (`Page::layers`, or an `Artboard`/`Group`'s
/// own children, already shifted to that parent's coordinate space),
/// applying `Layer::is_mask`/`ignore_mask` via
/// `masking::partition_mask_runs` — see that function's doc comment for the
/// masked-run semantics, and `MaskedGroupTextureCache` for how a masked run
/// actually gets drawn (a cached rasterized texture, since egui's immediate-
/// mode painter can't clip to an arbitrary shape).
#[allow(clippy::too_many_arguments)]
pub(super) fn draw_children(
    painter: &egui::Painter,
    ctx: &egui::Context,
    image_cache: &mut ImageTextureCache,
    mask_cache: &mut MaskedGroupTextureCache,
    noise_cache: &mut NoiseTextureCache,
    halftone_cache: &mut HalftoneTextureCache,
    pattern_cache: &mut PatternTextureCache,
    shadow_cache: &mut ShadowTextureCache,
    children: &[Layer],
    page: &Page,
    parent_offset: Vec2,
    origin: Pos2,
    pan: Vec2,
    zoom: f32,
    opacity: f32,
    editing_text: Option<LayerId>,
) {
    for unit in crate::masking::partition_mask_runs(children) {
        match unit {
            crate::masking::RenderUnit::Plain(child) => {
                draw_layer(painter, ctx, image_cache, mask_cache, noise_cache, halftone_cache, pattern_cache, shadow_cache, child, page, parent_offset, origin, pan, zoom, opacity, editing_text);
            }
            crate::masking::RenderUnit::Masked { mask, content } => {
                draw_masked_run(painter, ctx, mask_cache, mask, &content, page, parent_offset, origin, pan, zoom, opacity);
            }
        }
    }
}

/// Draws a masked run's cached composite texture (see
/// `MaskedGroupTextureCache::get_or_build`) as a single textured rect over
/// the mask's on-screen bounds — the same "cached raster + `painter.image`"
/// pattern as an `Image` layer, including applying the run's *ambient*
/// opacity via `tint` at draw time rather than baking it into the texture
/// (see `CachedMaskTexture`'s doc comment for why).
#[allow(clippy::too_many_arguments)]
pub(super) fn draw_masked_run(
    painter: &egui::Painter,
    ctx: &egui::Context,
    mask_cache: &mut MaskedGroupTextureCache,
    mask: &Layer,
    content: &[&Layer],
    page: &Page,
    parent_offset: Vec2,
    origin: Pos2,
    pan: Vec2,
    zoom: f32,
    opacity: f32,
) {
    let Some((texture, bounds)) = mask_cache.get_or_build(ctx, mask, content, page) else {
        return;
    };
    let to_screen = |p: Pos2| origin + pan + (p.to_vec2() + parent_offset) * zoom;
    let screen_rect = Rect::from_two_pos(to_screen(bounds.min), to_screen(bounds.max));
    let tint = with_opacity(Color32::WHITE, opacity);
    painter.image(texture.id(), screen_rect, Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)), tint);
}

#[allow(clippy::too_many_arguments)]
pub(super) fn draw_layer(
    painter: &egui::Painter,
    ctx: &egui::Context,
    image_cache: &mut ImageTextureCache,
    mask_cache: &mut MaskedGroupTextureCache,
    noise_cache: &mut NoiseTextureCache,
    halftone_cache: &mut HalftoneTextureCache,
    pattern_cache: &mut PatternTextureCache,
    shadow_cache: &mut ShadowTextureCache,
    layer: &Layer,
    page: &Page,
    parent_offset: Vec2,
    origin: Pos2,
    pan: Vec2,
    zoom: f32,
    parent_opacity: f32,
    editing_text: Option<LayerId>,
) {
    if !layer.visible {
        return;
    }
    let opacity = parent_opacity * layer.opacity;
    let to_screen = |p: Pos2| origin + pan + (p.to_vec2() + parent_offset) * zoom;
    let fill = with_opacity(
        layer.style.fill.as_ref().map(Paint::to_color32).unwrap_or(Color32::TRANSPARENT),
        opacity * layer.style.fill_opacity,
    );
    let stroke = layer
        .style
        .stroke
        .as_ref()
        .map(|s| EguiStroke::new(s.width * zoom, with_opacity(s.paint.to_color32(), opacity * layer.style.stroke_opacity)))
        .unwrap_or(EguiStroke::NONE);

    // Cloning the (cheap, `Arc`-backed) `TextureHandle`s out of the cache
    // releases its borrow before recursing into `draw_children` below for
    // `Artboard`/`Group` — which needs `shadow_cache` mutably for its own
    // descendants' shadows — rather than holding an immutable borrow of the
    // cache across that recursive call.
    let (shadow_outer, shadow_inner) = match shadow_cache.get_or_build(ctx, layer, page) {
        Some(cached) => (cached.outer.clone(), cached.inner.clone()),
        None => (Vec::new(), Vec::new()),
    };
    for (texture, rect) in &shadow_outer {
        let screen_rect = Rect::from_two_pos(to_screen(rect.min), to_screen(rect.max));
        painter.image(texture.id(), screen_rect, Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)), with_opacity(Color32::WHITE, opacity));
    }

    match &layer.kind {
        LayerKind::Artboard { children, background } => {
            let bounds = layer.frame.bounds();
            let screen_rect = Rect::from_two_pos(to_screen(bounds.min), to_screen(bounds.max));
            painter.rect_filled(screen_rect, 0.0, with_opacity(*background, opacity));
            painter.rect_stroke(
                screen_rect,
                0.0,
                EguiStroke::new(1.0, with_opacity(Color32::from_gray(150), opacity)),
                egui::StrokeKind::Outside,
            );
            // Fixed screen-space size (not zoom-scaled) so the label stays
            // legible when many artboards are visible at once, zoomed out.
            painter.text(
                screen_rect.left_top() - Vec2::new(0.0, 16.0),
                egui::Align2::LEFT_BOTTOM,
                &layer.name,
                egui::FontId::proportional(12.0),
                Color32::from_gray(100),
            );
            let child_offset = parent_offset + layer.frame.pos.to_vec2();
            draw_children(painter, ctx, image_cache, mask_cache, noise_cache, halftone_cache, pattern_cache, shadow_cache, children, page, child_offset, origin, pan, zoom, opacity, editing_text);
        }
        LayerKind::Group { children } => {
            let child_offset = parent_offset + layer.frame.pos.to_vec2();
            draw_children(painter, ctx, image_cache, mask_cache, noise_cache, halftone_cache, pattern_cache, shadow_cache, children, page, child_offset, origin, pan, zoom, opacity, editing_text);
        }
        LayerKind::Rectangle { corner_radius } => {
            let bounds = layer.frame.bounds();
            let screen_rect = Rect::from_two_pos(to_screen(bounds.min), to_screen(bounds.max));
            let scaled = corner_radius.scaled(zoom);
            let tessellation_required = layer.style.fill.as_ref().is_some_and(Paint::needs_tessellated_fill)
                || layer.style.stroke.as_ref().is_some_and(|s| s.paint.is_gradient());
            if layer.frame.rotation == 0.0 && !tessellation_required {
                // Fast, pixel-identical-to-before path: egui's native
                // rounded-rect painter has no rotation (or gradient/noise)
                // parameter (checked against epaint-0.36.0), so a rotated or
                // gradient/noise-filled/stroked rect switches to the
                // hand-tessellated `paint_polygon` path below instead.
                painter.rect(
                    screen_rect,
                    egui::CornerRadius {
                        nw: scaled.top_left.round() as u8,
                        ne: scaled.top_right.round() as u8,
                        se: scaled.bottom_right.round() as u8,
                        sw: scaled.bottom_left.round() as u8,
                    },
                    fill,
                    stroke,
                    egui::StrokeKind::Inside,
                );
            } else {
                let center = screen_rect.center();
                let points: Vec<Pos2> = rounded_rect_points(screen_rect, scaled.as_array())
                    .into_iter()
                    .map(|p| rotate_point(p, center, layer.frame.rotation))
                    .collect();
                paint_polygon(painter, ctx, noise_cache, halftone_cache, pattern_cache, points, true, screen_rect, center, layer.frame.rotation, layer, fill, stroke, opacity, false);
            }
        }
        LayerKind::Oval => {
            let bounds = layer.frame.bounds();
            let screen_rect = Rect::from_two_pos(to_screen(bounds.min), to_screen(bounds.max));
            let center = screen_rect.center();
            let points: Vec<Pos2> = ellipse_points(center, screen_rect.width() / 2.0, screen_rect.height() / 2.0)
                .into_iter()
                .map(|p| rotate_point(p, center, layer.frame.rotation))
                .collect();
            paint_polygon(painter, ctx, noise_cache, halftone_cache, pattern_cache, points, true, screen_rect, center, layer.frame.rotation, layer, fill, stroke, opacity, false);
        }
        LayerKind::Star { points: point_count, inner_ratio } => {
            let bounds = layer.frame.bounds();
            let screen_rect = Rect::from_two_pos(to_screen(bounds.min), to_screen(bounds.max));
            let center = screen_rect.center();
            let points: Vec<Pos2> = crate::shapes::star_points(
                center,
                screen_rect.width() / 2.0,
                screen_rect.height() / 2.0,
                *point_count,
                *inner_ratio,
            )
            .into_iter()
            .map(|p| rotate_point(p, center, layer.frame.rotation))
            .collect();
            paint_polygon(painter, ctx, noise_cache, halftone_cache, pattern_cache, points, true, screen_rect, center, layer.frame.rotation, layer, fill, stroke, opacity, false);
        }
        LayerKind::Polygon { sides } => {
            let bounds = layer.frame.bounds();
            let screen_rect = Rect::from_two_pos(to_screen(bounds.min), to_screen(bounds.max));
            let center = screen_rect.center();
            let points: Vec<Pos2> =
                crate::shapes::polygon_points(center, screen_rect.width() / 2.0, screen_rect.height() / 2.0, *sides)
                    .into_iter()
                    .map(|p| rotate_point(p, center, layer.frame.rotation))
                    .collect();
            paint_polygon(painter, ctx, noise_cache, halftone_cache, pattern_cache, points, true, screen_rect, center, layer.frame.rotation, layer, fill, stroke, opacity, false);
        }
        LayerKind::Line => {
            let bounds = layer.frame.bounds();
            let screen_rect = Rect::from_two_pos(to_screen(bounds.min), to_screen(bounds.max));
            let center = to_screen(bounds.center());
            let a = rotate_point(to_screen(layer.frame.start()), center, layer.frame.rotation);
            let b = rotate_point(to_screen(layer.frame.end()), center, layer.frame.rotation);
            if let Some(Paint::Gradient(g)) = layer.style.stroke.as_ref().map(|s| &s.paint) {
                paint_gradient_stroke(
                    painter,
                    &[a, b],
                    false,
                    g,
                    screen_rect,
                    center,
                    layer.frame.rotation,
                    stroke.width,
                    opacity * layer.style.stroke_opacity,
                );
            } else {
                painter.line_segment([a, b], stroke);
            }
        }
        LayerKind::Arrow { start_cap, end_cap } => {
            let bounds = layer.frame.bounds();
            let screen_rect = Rect::from_two_pos(to_screen(bounds.min), to_screen(bounds.max));
            let center = to_screen(bounds.center());
            let a = rotate_point(to_screen(layer.frame.start()), center, layer.frame.rotation);
            let b = rotate_point(to_screen(layer.frame.end()), center, layer.frame.rotation);
            if let Some(Paint::Gradient(g)) = layer.style.stroke.as_ref().map(|s| &s.paint) {
                paint_gradient_stroke(
                    painter,
                    &[a, b],
                    false,
                    g,
                    screen_rect,
                    center,
                    layer.frame.rotation,
                    stroke.width,
                    opacity * layer.style.stroke_opacity,
                );
            } else {
                painter.line_segment([a, b], stroke);
            }
            if stroke.color != Color32::TRANSPARENT {
                // Caps point along the segment's own screen-space direction
                // (not `layer.frame.rotation`, which is already baked into
                // `a`/`b` above) — always outward from the shaft. Caps stay
                // solid-colored (at the stroke's end color) even for a
                // gradient stroke — a minor approximation, consistent with
                // "canvas gradients are a preview" elsewhere in this file.
                let cap_color = layer
                    .style
                    .stroke
                    .as_ref()
                    .map(|s| match &s.paint {
                        Paint::Gradient(g) => with_opacity(g.color_at(1.0), opacity * layer.style.stroke_opacity),
                        // Stroke paint can never be `Noise`/`Halftone`/
                        // `Pattern` (see `ui/inspector.rs::paint_editor`'s
                        // `allow_texture_fills`, fill-only) — kept
                        // exhaustive rather than a wildcard so a future
                        // stroke-texture-fill feature can't silently fall
                        // through to this flat-color case.
                        Paint::Solid(_) | Paint::Noise(_) | Paint::Halftone(_) | Paint::Pattern(_) => stroke.color,
                    })
                    .unwrap_or(stroke.color);
                draw_arrow_cap(painter, a, a - b, *start_cap, cap_color, stroke.width);
                draw_arrow_cap(painter, b, b - a, *end_cap, cap_color, stroke.width);
            }
        }
        LayerKind::Path { points, closed } => {
            let screen_center = to_screen(layer.frame.bounds().center());
            let screen_bounds = Rect::from_two_pos(to_screen(layer.frame.bounds().min), to_screen(layer.frame.bounds().max));
            let screen_points: Vec<Pos2> = flatten_path(points, *closed)
                .into_iter()
                .map(|p| rotate_point(to_screen(layer.frame.pos + p.to_vec2()), screen_center, layer.frame.rotation))
                .collect();
            if screen_points.len() >= 2 {
                // epaint's tessellator asserts that an open `PathShape` has a
                // transparent fill ("You asked to fill a path that is not
                // closed") — a `Style::fill` set on an open path (e.g. one a
                // Scissors cut just opened) would otherwise panic here.
                paint_polygon(
                    painter,
                    ctx,
                    noise_cache,
                    halftone_cache,
                    pattern_cache,
                    screen_points,
                    *closed,
                    screen_bounds,
                    screen_center,
                    layer.frame.rotation,
                    layer,
                    fill,
                    stroke,
                    opacity,
                    !*closed,
                );
            }
        }
        LayerKind::CompoundPath { polygons } => {
            draw_even_odd_polygons(painter, ctx, noise_cache, halftone_cache, pattern_cache, polygons, layer, fill, stroke, opacity, to_screen);
        }
        LayerKind::BooleanGroup { children } => {
            // No `draw_children` call here, unlike `Group`/`Artboard` — only
            // the computed geometry participates in what's drawn (children's
            // own `style` is inert once inside a `BooleanGroup`, see the
            // type's doc comment); `fill`/`stroke` above are already derived
            // from this layer's own `style`.
            let combined = crate::boolean_ops::compute_boolean_group(children);
            let polygons = crate::boolean_ops::multipolygon_to_polygons(&combined);
            draw_even_odd_polygons(painter, ctx, noise_cache, halftone_cache, pattern_cache, &polygons, layer, fill, stroke, opacity, to_screen);
        }
        LayerKind::Text {
            content,
            font_size,
            font,
            align,
            vertical_align,
            resize,
            line_height,
            letter_spacing,
            paragraph_spacing,
            bold,
            italic,
            underline,
            strikethrough,
            transform,
            list,
            list_start,
            runs,
            path_attachment,
            ..
        } => {
            // While this layer is being edited in place, the floating
            // `egui::TextEdit` overlay (see
            // `CanvasWidget::show_text_edit_overlay`) is the only visible copy of its text.
            if editing_text != Some(layer.id) {
                let bounds = layer.frame.bounds();
                let style = TextStyleParams {
                    font: font.clone(),
                    font_size: *font_size,
                    align: *align,
                    letter_spacing: *letter_spacing,
                    line_height: *line_height,
                    italic: *italic,
                    underline: *underline,
                    strikethrough: *strikethrough,
                    transform: *transform,
                    list: *list,
                    list_start: *list_start,
                };
                let color = with_opacity(
                    layer.style.fill.as_ref().map(Paint::to_color32).unwrap_or(Color32::BLACK),
                    opacity * layer.style.fill_opacity,
                );
                let attached_outline = path_attachment
                    .as_ref()
                    .and_then(|a| crate::text_path_geometry::resolve_target_outline(&page.layers, a.target).map(|o| (a, o)));
                if let Some((attachment, (outline, closed))) = attached_outline {
                    let base_style = crate::model::text_runs::RunStyle {
                        font: font.clone(),
                        font_size: *font_size,
                        color: layer.style.fill.as_ref().map(Paint::to_color32),
                        bold: *bold,
                        italic: *italic,
                        underline: *underline,
                        strikethrough: *strikethrough,
                    };
                    let screen_points: Vec<Pos2> = outline.iter().map(|&p| to_screen(p)).collect();
                    match attachment.mode {
                        crate::model::TextPathMode::OnPath => {
                            let glyphs = crate::text_on_path_layout::layout_glyphs_on_path(
                                ctx,
                                content,
                                runs,
                                &base_style,
                                &style,
                                zoom,
                                color,
                                &screen_points,
                                closed,
                                attachment.offset * zoom,
                                attachment.start,
                                attachment.flip,
                            );
                            for glyph in glyphs {
                                let mut shape = egui::epaint::TextShape::new(glyph.pos, glyph.galley, color);
                                shape.angle = glyph.angle;
                                painter.add(egui::Shape::Text(shape));
                            }
                        }
                        crate::model::TextPathMode::AreaInside => {
                            let lines = crate::text_area_layout::layout_text_in_area(
                                ctx,
                                content,
                                runs,
                                &base_style,
                                &style,
                                zoom,
                                color,
                                &screen_points,
                                attachment.offset * zoom,
                            );
                            for line in lines {
                                if line.galley.is_empty() {
                                    continue;
                                }
                                painter.add(egui::Shape::Text(egui::epaint::TextShape::new(line.pos, line.galley, color)));
                            }
                        }
                    }
                } else {
                    let wrap_width = match resize {
                        TextResize::Auto => f32::INFINITY,
                        TextResize::AutoHeight | TextResize::Fixed => bounds.width() * zoom,
                    };
                    // `runs` non-empty is the rich-text path (see
                    // `LayerKind::Text::runs`'s doc comment) — everything below
                    // this `if` is untouched from before that feature existed,
                    // reached only for the (overwhelmingly common) uniform-style
                    // case, so it can't regress.
                    let galleys = if runs.is_empty() {
                        text_layout::layout_paragraphs(ctx, content, &style, zoom, color, wrap_width)
                    } else {
                        let base_style = crate::model::text_runs::RunStyle {
                            font: font.clone(),
                            font_size: *font_size,
                            color: layer.style.fill.as_ref().map(Paint::to_color32),
                            bold: *bold,
                            italic: *italic,
                            underline: *underline,
                            strikethrough: *strikethrough,
                        };
                        text_layout::layout_paragraphs_rich(ctx, content, runs, &base_style, &style, zoom, color, wrap_width)
                    };
                    let (offsets, total_size) = text_layout::stack_paragraphs(&galleys, *paragraph_spacing * zoom);

                    let start_y = match vertical_align {
                        VerticalAlign::Top => bounds.min.y,
                        VerticalAlign::Middle => bounds.center().y - (total_size.y / zoom) / 2.0,
                        VerticalAlign::Bottom => bounds.max.y - total_size.y / zoom,
                    };
                    let anchor_x = match align {
                        TextAlign::Left | TextAlign::Justify => bounds.min.x,
                        TextAlign::Center => bounds.center().x,
                        TextAlign::Right => bounds.max.x,
                    };
                    let base = to_screen(Pos2::new(anchor_x, start_y));

                    let clip_painter;
                    let text_painter = if *resize == TextResize::Fixed {
                        let screen_rect = Rect::from_two_pos(to_screen(bounds.min), to_screen(bounds.max));
                        clip_painter = painter.with_clip_rect(screen_rect);
                        &clip_painter
                    } else {
                        painter
                    };
                    // egui's `TextShape` rotates about its own `pos` (the
                    // unrotated top-left of that galley), not about a shared
                    // pivot — so a multi-line/multi-paragraph text layer needs
                    // each galley's `pos` individually rotated about the whole
                    // layer's own bounds center first, with the same angle then
                    // applied to each galley so every line still reads upright
                    // relative to the others.
                    let screen_center = to_screen(bounds.center());
                    let angle_rad = layer.frame.rotation.to_radians();
                    let draw_rotated_galley = |p: &egui::Painter, pos: Pos2, galley: std::sync::Arc<egui::Galley>, color: Color32| {
                        if galley.is_empty() {
                            return;
                        }
                        let mut shape = egui::epaint::TextShape::new(
                            rotate_point(pos, screen_center, layer.frame.rotation),
                            galley,
                            color,
                        );
                        shape.angle = angle_rad;
                        p.add(egui::Shape::Text(shape));
                    };
                    for (galley, y_off) in galleys.into_iter().zip(offsets) {
                        let pos = base + Vec2::new(0.0, y_off);
                        draw_rotated_galley(text_painter, pos, galley.clone(), color);
                        if *bold && runs.is_empty() {
                            // No real bold weight is available for any of the
                            // bundled fonts (see `TextFont`) — fake it by
                            // drawing a second copy offset by half a screen
                            // pixel, regardless of zoom. The rich-text path
                            // (`runs` non-empty) doesn't need this: bold there
                            // is already a real bold-weight font family baked
                            // into the galley per run (see `fonts.rs`'s "Bold"
                            // doc section). Passing the plain unrotated offset
                            // through the same `draw_rotated_galley` rotation
                            // (which is linear in the pos-minus-pivot deviation)
                            // keeps the nudge aligned with the rotated baseline,
                            // without double-applying the rotation.
                            draw_rotated_galley(text_painter, pos + Vec2::new(0.6, 0.0), galley, color);
                        }
                    }
                }
            }
        }
        LayerKind::Image { encoded, version, color_adjust, .. } => {
            if let Some(texture) = image_cache.get_or_load(ctx, layer.id, encoded, *version, *color_adjust) {
                let bounds = layer.frame.bounds();
                let screen_rect = Rect::from_two_pos(to_screen(bounds.min), to_screen(bounds.max));
                let tint = with_opacity(Color32::WHITE, opacity);
                if layer.frame.rotation == 0.0 {
                    painter.image(
                        texture.id(),
                        screen_rect,
                        Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
                        tint,
                    );
                } else {
                    // `Painter::image` only takes an axis-aligned `Rect`, so
                    // a rotated image is drawn as a hand-built textured quad
                    // mesh instead (egui's mesh path supports arbitrary
                    // vertex positions).
                    let corners = crate::shapes::rotated_corners(screen_rect, layer.frame.rotation);
                    let uvs = [
                        Pos2::new(0.0, 0.0),
                        Pos2::new(1.0, 0.0),
                        Pos2::new(1.0, 1.0),
                        Pos2::new(0.0, 1.0),
                    ];
                    let mut mesh = egui::Mesh::with_texture(texture.id());
                    for (pos, uv) in corners.into_iter().zip(uvs) {
                        mesh.vertices.push(egui::epaint::Vertex { pos, uv, color: tint });
                    }
                    mesh.indices.extend_from_slice(&[0, 1, 2, 0, 2, 3]);
                    painter.add(egui::Shape::mesh(mesh));
                }
            }
        }
    }

    for (texture, rect) in &shadow_inner {
        let screen_rect = Rect::from_two_pos(to_screen(rect.min), to_screen(rect.max));
        painter.image(texture.id(), screen_rect, Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)), with_opacity(Color32::WHITE, opacity));
    }
}

/// Draws an `Arrow` layer's end marker at `tip` (already in screen space),
/// oriented by `outward` (points away from the shaft, e.g. `a - b` at
/// endpoint `a`) — every cap style is sized off `stroke_width` (the
/// already-zoom-scaled screen stroke width), not `frame.rotation`: an
/// arrow's caps always point along its own segment direction (which
/// `outward` already reflects, since the caller derives it from the
/// already-rotated endpoints), independent of whether the layer additionally
/// carries a `frame.rotation` on top.
pub(super) fn draw_arrow_cap(painter: &egui::Painter, tip: Pos2, outward: Vec2, cap: crate::model::ArrowCap, color: Color32, stroke_width: f32) {
    if outward.length() < 1e-3 {
        return;
    }
    let dir = outward.normalized();
    let perp = Vec2::new(-dir.y, dir.x);
    let size = (stroke_width * 3.5).max(8.0);
    match cap {
        crate::model::ArrowCap::None => {}
        crate::model::ArrowCap::Triangle => {
            let base = tip - dir * size;
            let p1 = base + perp * size * 0.5;
            let p2 = base - perp * size * 0.5;
            painter.add(egui::Shape::Path(egui::epaint::PathShape {
                points: vec![tip, p1, p2],
                closed: true,
                fill: color,
                stroke: EguiStroke::NONE.into(),
            }));
        }
        crate::model::ArrowCap::Disc => {
            painter.circle_filled(tip - dir * size * 0.4, size * 0.4, color);
        }
        crate::model::ArrowCap::Line => {
            let base = tip - dir * size;
            let p1 = base + perp * size * 0.5;
            let p2 = base - perp * size * 0.5;
            let cap_stroke = EguiStroke::new(stroke_width, color);
            painter.line_segment([p1, tip], cap_stroke);
            painter.line_segment([p2, tip], cap_stroke);
        }
    }
}
