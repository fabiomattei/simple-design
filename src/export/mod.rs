use tiny_skia::{FillRule, PathBuilder, Pixmap, PixmapPaint, Transform};

mod glyph_raster;
mod image_layer;
mod line_arrow;
mod paint;
mod sk_geometry;
mod text_in_area;
mod text_layer;
mod text_on_path;
mod text_plain;
mod text_rich;
mod text_wrap;

use line_arrow::{draw_arrow, draw_line};
use text_layer::draw_text_layer;

pub(crate) use text_wrap::{wrap_line, wrap_line_rich};

use image_layer::draw_image;
use paint::to_color_with_opacity;
use sk_geometry::{
    compound_path_to_sk_path, fill_and_stroke, fill_rect, path_to_sk_path, polygon_ring_path, rotation_transform, rounded_rect_path, to_sk_rect,
};

use crate::boolean_ops;
use crate::model::{Layer, LayerKind};

/// Renders a single layer (and its descendants, if it's a container) to a
/// standalone pixmap sized to the layer's own *rotated* bounds (its actual
/// visual footprint — plain `bounds()` would clip a rotated layer's corners,
/// since they extend beyond the unrotated local rect), padded further to fit
/// any drop shadow the layer's own `Style::shadows` extends beyond that
/// (see `shadow::outer_shadow_extent`) — otherwise a standalone "Export
/// Layer as PNG" would clip the shadow at the shape's own edge. Artboards
/// render with their own background color; everything else renders on a
/// transparent background.
pub fn render_layer(layer: &Layer) -> Option<Pixmap> {
    let bounds = layer.frame.rotated_bounds();
    let pad = crate::shadow::outer_shadow_extent(&layer.style.shadows);
    let width = (bounds.width() + 2.0 * pad).round().max(1.0) as u32;
    let height = (bounds.height() + 2.0 * pad).round().max(1.0) as u32;
    let mut pixmap = Pixmap::new(width, height)?;
    let offset = egui::Vec2::new(-bounds.min.x + pad, -bounds.min.y + pad);
    draw_layer_with_shadows(&mut pixmap, layer, std::slice::from_ref(layer), offset, 1.0);
    Some(pixmap)
}

/// `layer.frame.rotated_bounds()`, except for a `Text` layer whose
/// `path_attachment` resolves against `roots` — there, the attached glyphs
/// can sit anywhere along/inside the target, arbitrarily far from the
/// layer's own (often tiny, e.g. click-to-place default size) frame, so
/// sizing off `layer.frame` alone would silently clip them out of any
/// silhouette/standalone render. Falls back to `layer.frame.rotated_bounds()`
/// whenever there's no attachment, or it doesn't resolve — the plain,
/// pre-attachment behavior for every other case.
fn effective_rotated_bounds(layer: &Layer, roots: &[Layer]) -> egui::Rect {
    let LayerKind::Text { path_attachment: Some(attachment), font_size, .. } = &layer.kind else {
        return layer.frame.rotated_bounds();
    };
    let Some((outline, _closed)) = crate::text_path_geometry::resolve_target_outline(roots, attachment.target) else {
        return layer.frame.rotated_bounds();
    };
    if outline.is_empty() {
        return layer.frame.rotated_bounds();
    }
    // `resolve_target_outline` returns points relative to `roots` itself —
    // converting to this layer's own parent-relative space (what
    // `layer.frame.rotated_bounds()` is already in) needs this layer's own
    // position in that same space, same `absolute_offset_of` trick
    // `draw_layer`'s `Text` branch and `text_outline.rs` both use.
    let self_root_offset = crate::text_path_geometry::absolute_offset_of(roots, layer.id).unwrap_or(egui::Vec2::ZERO);
    let relative: Vec<egui::Pos2> = outline.iter().map(|&p| p - self_root_offset).collect();
    // The outline is only the *path*, not the text drawn along/inside it —
    // pad by the font size (a generous stand-in for glyph height/ascent/
    // descender reach) plus the attachment's own perpendicular offset, so
    // on-path text hugging the outside of a curve doesn't get clipped.
    egui::Rect::from_points(&relative).expand(font_size.abs().max(1.0) + attachment.offset.abs())
}

/// Same as `render_layer`, but without shadow padding or drawing — a plain
/// silhouette/appearance render used internally as the input to shadow
/// rendering itself (`shadow::render_outer_shadow`/`render_inner_shadow`
/// need the shape's own rasterized alpha, not a shadow of it) and by
/// `canvas/texture_caches.rs`'s `ShadowTextureCache`, which needs the exact same silhouette
/// the exported PNG's shadow would be built from. `roots` is forwarded to
/// `draw_layer` for `path_attachment` resolution, same convention as
/// `draw_layer`'s own doc comment — so a shadow cast by a path-attached
/// `Text` layer is shaped like its actual attached glyphs, not the plain
/// unattached layout `std::slice::from_ref(layer)` alone would fall back to
/// (see `effective_rotated_bounds`, which sizes this render to fit them).
pub(crate) fn render_layer_plain(layer: &Layer, roots: &[Layer]) -> Option<(Pixmap, egui::Rect)> {
    let bounds = effective_rotated_bounds(layer, roots);
    let width = bounds.width().round().max(1.0) as u32;
    let height = bounds.height().round().max(1.0) as u32;
    let mut pixmap = Pixmap::new(width, height)?;
    let offset = egui::Vec2::new(-bounds.min.x, -bounds.min.y);
    draw_layer(&mut pixmap, layer, roots, offset, 1.0);
    Some((pixmap, bounds))
}

/// Renders `layer` (and descendants) to a standalone PNG and writes it to `path`.
pub fn export_layer_to_png(layer: &Layer, path: &std::path::Path) -> anyhow::Result<()> {
    let pixmap = render_layer(layer).ok_or_else(|| anyhow::anyhow!("layer has no visible area"))?;
    pixmap.save_png(path)?;
    Ok(())
}

/// `pub(crate)` (rather than private) so `masking.rs` can use it as the
/// per-layer drawing callback for both `export/`'s own masked runs and
/// `canvas/texture_caches.rs`'s masked-group texture cache — see `masking::composite_masked_run`.
///
/// `roots` is where a `Text` layer's `path_attachment` looks for its target
/// (via `text_path_geometry::resolve_target_outline`) — a standalone
/// "Export Layer as PNG"/"Convert to Outlines" call passes just the one
/// layer being rendered (`std::slice::from_ref`), while `canvas/texture_caches.rs`'s
/// masked-group texture cache can pass the whole page's top-level layers,
/// so a masked Text layer can still attach to a target anywhere on the
/// page, not just within its own masked run.
pub(crate) fn draw_layer(pixmap: &mut Pixmap, layer: &Layer, roots: &[Layer], offset: egui::Vec2, parent_opacity: f32) {
    if !layer.visible {
        return;
    }
    let opacity = parent_opacity * layer.opacity;
    let bounds = layer.frame.bounds().translate(offset);
    match &layer.kind {
        LayerKind::Artboard { children, background } => {
            fill_rect(pixmap, bounds, to_color_with_opacity(*background, opacity));
            let child_offset = offset + layer.frame.pos.to_vec2();
            draw_children(pixmap, children, roots, child_offset, opacity);
        }
        LayerKind::Group { children } => {
            let child_offset = offset + layer.frame.pos.to_vec2();
            draw_children(pixmap, children, roots, child_offset, opacity);
        }
        LayerKind::Rectangle { corner_radius } => {
            let Some(path) = rounded_rect_path(bounds, *corner_radius) else {
                return;
            };
            fill_and_stroke(pixmap, &path, layer, FillRule::Winding, bounds, opacity, rotation_transform(layer, bounds));
        }
        LayerKind::Oval => {
            let Some(sk_rect) = to_sk_rect(bounds) else {
                return;
            };
            let mut pb = PathBuilder::new();
            pb.push_oval(sk_rect);
            let Some(path) = pb.finish() else {
                return;
            };
            fill_and_stroke(pixmap, &path, layer, FillRule::Winding, bounds, opacity, rotation_transform(layer, bounds));
        }
        LayerKind::Star { points, inner_ratio } => {
            let pts = crate::shapes::star_points(bounds.center(), bounds.width() / 2.0, bounds.height() / 2.0, *points, *inner_ratio);
            let Some(path) = polygon_ring_path(&pts) else {
                return;
            };
            fill_and_stroke(pixmap, &path, layer, FillRule::Winding, bounds, opacity, rotation_transform(layer, bounds));
        }
        LayerKind::Polygon { sides } => {
            let pts = crate::shapes::polygon_points(bounds.center(), bounds.width() / 2.0, bounds.height() / 2.0, *sides);
            let Some(path) = polygon_ring_path(&pts) else {
                return;
            };
            fill_and_stroke(pixmap, &path, layer, FillRule::Winding, bounds, opacity, rotation_transform(layer, bounds));
        }
        LayerKind::Line => {
            draw_line(pixmap, layer, offset, bounds, opacity);
        }
        LayerKind::Arrow { start_cap, end_cap } => {
            draw_arrow(pixmap, layer, offset, bounds, opacity, start_cap, end_cap);
        }
        LayerKind::Path { points, closed } => {
            let Some(path) = path_to_sk_path(points, *closed, offset + layer.frame.pos.to_vec2()) else {
                return;
            };
            fill_and_stroke(pixmap, &path, layer, FillRule::Winding, bounds, opacity, rotation_transform(layer, bounds));
        }
        LayerKind::CompoundPath { polygons } => {
            let Some(path) = compound_path_to_sk_path(polygons, offset + layer.frame.pos.to_vec2()) else {
                return;
            };
            // Ring winding direction doesn't matter for `EvenOdd` — it only
            // cares about crossing parity, which is what makes holes work.
            fill_and_stroke(pixmap, &path, layer, FillRule::EvenOdd, bounds, opacity, rotation_transform(layer, bounds));
        }
        LayerKind::BooleanGroup { children } => {
            // No child-recursion here, unlike `Group`/`Artboard` — only the
            // computed geometry is drawn, using this layer's own `style`
            // (via `fill_and_stroke`), same as `CompoundPath` above.
            let combined = boolean_ops::compute_boolean_group(children);
            let polygons = boolean_ops::multipolygon_to_polygons(&combined);
            let Some(path) = compound_path_to_sk_path(&polygons, offset + layer.frame.pos.to_vec2()) else {
                return;
            };
            fill_and_stroke(pixmap, &path, layer, FillRule::EvenOdd, bounds, opacity, rotation_transform(layer, bounds));
        }
        LayerKind::Text { .. } => {
            draw_text_layer(pixmap, layer, roots, offset, bounds, opacity);
        }
        LayerKind::Image { encoded, color_adjust, .. } => {
            draw_image(pixmap, encoded, *color_adjust, bounds, opacity, layer.frame.rotation);
        }
    }
}

/// Draws one parent's `children` (an `Artboard`/`Group`'s, already offset to
/// that parent's coordinate space), applying `Layer::is_mask`/`ignore_mask`
/// via `masking::partition_mask_runs` — see that function's doc comment for
/// the masked-run semantics. Deliberately shadow-free (plain `draw_layer`)
/// — this is what `render_layer_plain`'s silhouette rendering recurses
/// through for a `Group`/`Artboard`, so a parent's own shadow is computed
/// from its children's *shapes*, not from halos of the children's own
/// shadows. `draw_children_with_shadows` is the shadow-aware sibling used
/// for real rendering.
fn draw_children(pixmap: &mut Pixmap, children: &[Layer], roots: &[Layer], offset: egui::Vec2, opacity: f32) {
    for unit in crate::masking::partition_mask_runs(children) {
        match unit {
            crate::masking::RenderUnit::Plain(child) => draw_layer(pixmap, child, roots, offset, opacity),
            crate::masking::RenderUnit::Masked { mask, content } => {
                crate::masking::composite_masked_run(pixmap, mask, &content, offset, opacity, |px, l, off, op| {
                    draw_layer(px, l, roots, off, op)
                });
            }
        }
    }
}

/// `draw_children`'s shadow-aware sibling: same masked-run partitioning, but
/// draws every child (plain or masked-run content) through
/// `draw_layer_with_shadows` instead of plain `draw_layer`, so nested
/// layers at any depth get their own `Style::shadows`/`inner_shadows`.
fn draw_children_with_shadows(pixmap: &mut Pixmap, children: &[Layer], roots: &[Layer], offset: egui::Vec2, opacity: f32) {
    for unit in crate::masking::partition_mask_runs(children) {
        match unit {
            crate::masking::RenderUnit::Plain(child) => draw_layer_with_shadows(pixmap, child, roots, offset, opacity),
            crate::masking::RenderUnit::Masked { mask, content } => {
                crate::masking::composite_masked_run(pixmap, mask, &content, offset, opacity, |px, l, off, op| {
                    draw_layer_with_shadows(px, l, roots, off, op)
                });
            }
        }
    }
}

/// Wraps a layer's normal appearance with `layer.style.shadows`/
/// `inner_shadows`: outer shadows composited *behind* it (drawn first), then
/// its normal appearance, then inner shadows on top, clipped inside it.
/// Renders the layer's own silhouette once (via `render_layer_plain`, which
/// is always shadow-free — no recursion risk) and reuses it for every
/// stacked shadow rather than re-rasterizing per shadow.
///
/// `Group`/`Artboard` are special-cased to recurse via
/// `draw_children_with_shadows` (so descendants get their own shadows too);
/// every other kind has no children of its own and can delegate straight to
/// plain `draw_layer`. This duplicates `draw_layer`'s few lines of
/// `Artboard`/`Group` handling rather than threading a children-drawing
/// callback through `draw_layer`'s signature — `draw_layer` itself must stay
/// shadow-free (see `draw_children`'s doc comment for why), so it can't just
/// call back into this function for its own recursion.
pub(crate) fn draw_layer_with_shadows(pixmap: &mut Pixmap, layer: &Layer, roots: &[Layer], offset: egui::Vec2, parent_opacity: f32) {
    if !layer.visible {
        return;
    }
    let has_outer = !layer.style.shadows.is_empty();
    let has_inner = !layer.style.inner_shadows.is_empty();
    let silhouette = if has_outer || has_inner { render_layer_plain(layer, roots) } else { None };

    if let (true, Some((silhouette, bounds))) = (has_outer, &silhouette) {
        let silhouette_origin = egui::Vec2::new(bounds.min.x, bounds.min.y) + offset;
        for shadow in &layer.style.shadows {
            if let Some(crate::shadow::ShadowLayer { pixmap: shadow_px, origin }) =
                crate::shadow::render_outer_shadow(silhouette, silhouette_origin, shadow)
            {
                let paint = PixmapPaint { opacity: parent_opacity, ..Default::default() };
                pixmap.draw_pixmap(origin.x.round() as i32, origin.y.round() as i32, shadow_px.as_ref(), &paint, Transform::identity(), None);
            }
        }
    }

    let opacity = parent_opacity * layer.opacity;
    match &layer.kind {
        LayerKind::Artboard { children, background } => {
            let bounds = layer.frame.bounds().translate(offset);
            fill_rect(pixmap, bounds, to_color_with_opacity(*background, opacity));
            let child_offset = offset + layer.frame.pos.to_vec2();
            draw_children_with_shadows(pixmap, children, roots, child_offset, opacity);
        }
        LayerKind::Group { children } => {
            let child_offset = offset + layer.frame.pos.to_vec2();
            draw_children_with_shadows(pixmap, children, roots, child_offset, opacity);
        }
        _ => draw_layer(pixmap, layer, roots, offset, parent_opacity),
    }

    if let (true, Some((silhouette, bounds))) = (has_inner, &silhouette) {
        let draw_at = bounds.min + offset;
        for shadow in &layer.style.inner_shadows {
            if let Some(inner_px) = crate::shadow::render_inner_shadow(silhouette, shadow) {
                let paint = PixmapPaint { opacity: parent_opacity, ..Default::default() };
                pixmap.draw_pixmap(draw_at.x.round() as i32, draw_at.y.round() as i32, inner_px.as_ref(), &paint, Transform::identity(), None);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::boolean_ops::apply_boolean_op;
    use crate::model::text_runs::{RunStyle, TextRun};
    use crate::model::{
        ArrowCap, BoolOp, CornerRadii, Frame, ListType, Page, PathPoint, PointType, TextAlign, TextFont, TextResize,
        TextTransform, VerticalAlign,
    };
    use egui::{Color32, Pos2, Vec2};

    /// End-to-end check of `render_layer`'s shadow padding + `draw_layer_with_shadows`
    /// wiring (as opposed to `shadow.rs`'s own unit tests, which exercise the
    /// blur/composite math in isolation): a standalone-exported rectangle
    /// with a drop shadow should come out padded wider than its own frame,
    /// with visible shadow color just past the shape's own edge.
    #[test]
    fn exported_layer_with_drop_shadow_is_padded_and_shows_shadow_past_its_edge() {
        let mut layer = Layer::new(
            "Rect",
            Frame::from_two_points(Pos2::new(0.0, 0.0), Pos2::new(40.0, 40.0)),
            LayerKind::Rectangle { corner_radius: CornerRadii::ZERO },
        );
        layer.style.fill = Some(crate::model::Paint::Solid(Color32::from_rgb(255, 0, 0)));
        layer.style.stroke = None;
        layer.style.shadows = vec![crate::model::Shadow {
            color: Color32::BLACK,
            offset: Vec2::ZERO,
            blur: 10.0,
            spread: 0.0,
        }];

        let pixmap = render_layer(&layer).expect("should render");
        assert!(pixmap.width() > 40 && pixmap.height() > 40, "padded export should be larger than the shape's own 40x40 frame");

        let pad = ((pixmap.width() - 40) / 2) as i32;
        // A couple pixels above the shape's top edge (still inside the
        // padded canvas) should show blurred shadow, not full transparency.
        let x = pixmap.width() as i32 / 2;
        let y = (pad - 3).max(0);
        let above_edge = pixmap.pixel(x as u32, y as u32).expect("in-bounds pixel");
        assert!(above_edge.alpha() > 0, "blurred shadow should be visible just outside the shape's own edge");

        // The shape's own center is still the plain red fill, unaffected.
        let center = pixmap.pixel((pad + 20) as u32, (pad + 20) as u32).expect("center pixel");
        assert_eq!((center.red(), center.green(), center.blue()), (255, 0, 0));
    }

    #[test]
    fn renders_filled_rect_at_correct_size_and_color() {
        let mut layer = Layer::new(
            "Rect",
            Frame::from_two_points(Pos2::new(10.0, 10.0), Pos2::new(50.0, 30.0)),
            LayerKind::Rectangle { corner_radius: CornerRadii::ZERO },
        );
        layer.style.fill = Some(crate::model::Paint::Solid(Color32::from_rgb(255, 0, 0)));
        layer.style.stroke = None;

        let pixmap = render_layer(&layer).expect("should render");
        assert_eq!(pixmap.width(), 40);
        assert_eq!(pixmap.height(), 20);

        let center = pixmap.pixel(20, 10).expect("center pixel");
        assert_eq!((center.red(), center.green(), center.blue(), center.alpha()), (255, 0, 0, 255));

        let corner = pixmap.pixel(0, 0).expect("corner pixel");
        assert_eq!(corner.alpha(), 255);
    }

    #[test]
    fn fill_opacity_and_stroke_opacity_scale_alpha_independently_of_each_other() {
        let mut layer = Layer::new(
            "Rect",
            Frame::from_two_points(Pos2::new(0.0, 0.0), Pos2::new(40.0, 40.0)),
            LayerKind::Rectangle { corner_radius: CornerRadii::ZERO },
        );
        layer.style.fill = Some(crate::model::Paint::Solid(Color32::from_rgb(255, 0, 0)));
        layer.style.stroke = Some(crate::model::Stroke { paint: crate::model::Paint::Solid(Color32::from_rgb(0, 0, 255)), width: 4.0 });
        layer.style.fill_opacity = 0.5;
        layer.style.stroke_opacity = 1.0;

        let pixmap = render_layer(&layer).expect("should render");
        let fill_pixel = pixmap.pixel(20, 20).expect("center pixel, inside the fill only");
        // Premultiplied alpha: a straight-alpha channel value of 127/255
        // shows up as roughly half the full-alpha red component too.
        assert!(
            (fill_pixel.alpha() as i32 - 127).abs() <= 2,
            "expected ~50% alpha from fill_opacity, got {}",
            fill_pixel.alpha()
        );

        let stroke_pixel = pixmap.pixel(1, 20).expect("left edge, inside the stroke");
        assert_eq!(stroke_pixel.alpha(), 255, "stroke_opacity=1.0 should stay fully opaque");
    }

    #[test]
    fn renders_star_filled_at_center_but_not_at_the_bounding_boxs_corner() {
        let mut layer = Layer::new(
            "Star",
            Frame::from_two_points(Pos2::new(0.0, 0.0), Pos2::new(40.0, 40.0)),
            LayerKind::Star { points: 5, inner_ratio: 0.5 },
        );
        layer.style.fill = Some(crate::model::Paint::Solid(Color32::from_rgb(255, 200, 0)));
        layer.style.stroke = None;

        let pixmap = render_layer(&layer).expect("should render");
        let center = pixmap.pixel(20, 20).expect("center pixel");
        assert_eq!((center.red(), center.green(), center.blue(), center.alpha()), (255, 200, 0, 255));

        // A 5-point star inscribed in its bounding ellipse never reaches
        // the bounding box's own corner.
        let corner = pixmap.pixel(1, 1).expect("corner pixel");
        assert_eq!(corner.alpha(), 0);
    }

    #[test]
    fn renders_regular_polygon_filled_at_center() {
        let mut layer = Layer::new(
            "Hexagon",
            Frame::from_two_points(Pos2::new(0.0, 0.0), Pos2::new(40.0, 40.0)),
            LayerKind::Polygon { sides: 6 },
        );
        layer.style.fill = Some(crate::model::Paint::Solid(Color32::from_rgb(0, 100, 255)));
        layer.style.stroke = None;

        let pixmap = render_layer(&layer).expect("should render");
        let center = pixmap.pixel(20, 20).expect("center pixel");
        assert_eq!((center.red(), center.green(), center.blue(), center.alpha()), (0, 100, 255, 255));

        let corner = pixmap.pixel(1, 1).expect("corner pixel");
        assert_eq!(corner.alpha(), 0);
    }

    // `Frame::from_two_points` for a purely horizontal/vertical Line/Arrow
    // has zero size along the perpendicular axis (`end.y == start.y` forces
    // `size.y == 0`), so `render_layer`'s auto-sizing-to-bounds would give a
    // 1px-tall pixmap that clips the stroke's own perpendicular extent —
    // call `draw_layer` directly onto a fixed-size pixmap instead, sidestepping
    // that (pre-existing, not Arrow-specific) auto-sizing edge case.
    #[test]
    fn renders_arrow_with_no_caps_matches_plain_line() {
        let mut layer = Layer::new(
            "Arrow",
            Frame::from_two_points(Pos2::new(0.0, 20.0), Pos2::new(40.0, 20.0)),
            LayerKind::Arrow { start_cap: ArrowCap::None, end_cap: ArrowCap::None },
        );
        layer.style.stroke = Some(crate::model::Stroke { paint: crate::model::Paint::Solid(Color32::BLACK), width: 2.0 });
        layer.style.fill = None;

        let mut pixmap = Pixmap::new(40, 40).unwrap();
        draw_layer(&mut pixmap, &layer, std::slice::from_ref(&layer), egui::Vec2::ZERO, 1.0);
        let on_line = pixmap.pixel(20, 20).expect("on-line pixel");
        assert_eq!(on_line.alpha(), 255);
        let off_line = pixmap.pixel(20, 5).expect("off-line pixel");
        assert_eq!(off_line.alpha(), 0);
    }

    #[test]
    fn renders_arrow_with_triangle_end_cap_wider_than_the_shaft() {
        let mut layer = Layer::new(
            "Arrow",
            Frame::from_two_points(Pos2::new(0.0, 20.0), Pos2::new(40.0, 20.0)),
            LayerKind::Arrow { start_cap: ArrowCap::None, end_cap: ArrowCap::Triangle },
        );
        layer.style.stroke = Some(crate::model::Stroke { paint: crate::model::Paint::Solid(Color32::BLACK), width: 6.0 });
        layer.style.fill = None;

        let mut pixmap = Pixmap::new(40, 40).unwrap();
        draw_layer(&mut pixmap, &layer, std::slice::from_ref(&layer), egui::Vec2::ZERO, 1.0);
        // Well inside the triangle wing (cap size = max(6*3.5, 8) = 21, base
        // at x=19, half-width 7.5 at x=25) but clearly outside the shaft's
        // own stroke half-width (3, so shaft covers y 17..23 only).
        let wing = pixmap.pixel(25, 15).expect("wing pixel");
        assert!(wing.alpha() > 0, "triangle cap should extend beyond the shaft's stroke width");
        // Far from both the shaft and the cap's base range.
        let outside = pixmap.pixel(5, 5).expect("outside pixel");
        assert_eq!(outside.alpha(), 0);
    }

    #[test]
    fn zero_rotation_rectangle_renders_identically_to_unrotated_path() {
        // Regression lock for the `Rectangle` arm's switch from
        // `painter.rect`-equivalent tiny-skia's native rounded-rect path to
        // the shared `fill_and_stroke` + `rotation_transform` path: at
        // `rotation == 0.0`, `rotation_transform` returns `Transform::identity()`
        // so this must produce byte-identical output to the pre-rotation code.
        let mut layer = Layer::new(
            "Rect",
            Frame::from_two_points(Pos2::new(0.0, 0.0), Pos2::new(40.0, 40.0)),
            LayerKind::Rectangle { corner_radius: CornerRadii::uniform(8.0) },
        );
        layer.style.fill = Some(crate::model::Paint::Solid(Color32::from_rgb(10, 20, 30)));
        layer.style.stroke = None;
        let pixmap = render_layer(&layer).expect("should render");
        let center = pixmap.pixel(20, 20).expect("center pixel");
        assert_eq!((center.red(), center.green(), center.blue(), center.alpha()), (10, 20, 30, 255));
        // Corner pixel should stay unfilled (excluded by the rounded corner).
        let corner = pixmap.pixel(0, 0).expect("corner pixel");
        assert_eq!(corner.alpha(), 0);
    }

    #[test]
    fn independent_corner_radii_round_only_the_corners_given_a_nonzero_radius() {
        let mut layer = Layer::new(
            "Rect",
            Frame::from_two_points(Pos2::new(0.0, 0.0), Pos2::new(40.0, 40.0)),
            LayerKind::Rectangle {
                corner_radius: CornerRadii { top_left: 12.0, top_right: 0.0, bottom_right: 0.0, bottom_left: 0.0 },
            },
        );
        layer.style.fill = Some(crate::model::Paint::Solid(Color32::from_rgb(10, 20, 30)));
        layer.style.stroke = None;
        let pixmap = render_layer(&layer).expect("should render");
        // Rounded corner (top-left) stays unfilled.
        let rounded = pixmap.pixel(0, 0).expect("top-left pixel");
        assert_eq!(rounded.alpha(), 0);
        // The other three (sharp) corners are filled all the way to the edge.
        for (x, y) in [(39, 0), (0, 39), (39, 39)] {
            let sharp = pixmap.pixel(x, y).expect("sharp corner pixel");
            assert_eq!(sharp.alpha(), 255, "expected corner ({x}, {y}) to be filled");
        }
    }

    #[test]
    fn rotated_square_fills_center_but_not_the_new_bounding_boxs_corner() {
        let mut layer = Layer::new(
            "Rect",
            Frame { pos: Pos2::new(0.0, 0.0), size: Vec2::new(20.0, 20.0), rotation: 45.0 },
            LayerKind::Rectangle { corner_radius: CornerRadii::ZERO },
        );
        layer.style.fill = Some(crate::model::Paint::Solid(Color32::from_rgb(200, 0, 0)));
        layer.style.stroke = None;

        let pixmap = render_layer(&layer).expect("should render");
        // A 20x20 square rotated 45 degrees has a rotated-bounds AABB of
        // side 20*sqrt(2) ~= 28.28, rounded to 28.
        assert_eq!((pixmap.width(), pixmap.height()), (28, 28));

        let center = pixmap.pixel(14, 14).expect("center pixel");
        assert_eq!((center.red(), center.green(), center.blue(), center.alpha()), (200, 0, 0, 255));

        // The rotated square's diagonal edge doesn't reach the new AABB's
        // own corner — only its unrotated `bounds()` would have.
        let corner = pixmap.pixel(0, 0).expect("corner pixel");
        assert_eq!(corner.alpha(), 0);
    }

    #[test]
    fn rotated_image_export_pixmap_is_sized_to_rotated_bounds_not_clipped() {
        let encoded = crate::image_ops::encode_png(&image::RgbaImage::from_pixel(2, 2, image::Rgba([0, 200, 0, 255])));
        let layer = Layer::new_image(
            "Photo",
            Frame { pos: Pos2::new(0.0, 0.0), size: Vec2::new(40.0, 20.0), rotation: 90.0 },
            encoded,
            2,
            2,
        );
        let pixmap = render_layer(&layer).expect("should render");
        // A 40x20 layer rotated exactly 90 degrees has a 20x40 rotated AABB.
        assert_eq!((pixmap.width(), pixmap.height()), (20, 40));
        let center = pixmap.pixel(10, 20).expect("center pixel");
        assert_eq!((center.red(), center.green(), center.blue(), center.alpha()), (0, 200, 0, 255));
    }

    #[test]
    fn artboard_background_fills_whole_canvas_and_children_are_offset() {
        let mut child = Layer::new(
            "Child",
            Frame::from_two_points(Pos2::new(5.0, 5.0), Pos2::new(15.0, 15.0)),
            LayerKind::Rectangle { corner_radius: CornerRadii::ZERO },
        );
        child.style.fill = Some(crate::model::Paint::Solid(Color32::from_rgb(0, 255, 0)));
        child.style.stroke = None;

        let mut artboard = Layer::new_artboard(
            "Board",
            Frame::from_two_points(Pos2::new(0.0, 0.0), Pos2::new(30.0, 30.0)),
        );
        if let LayerKind::Artboard { children, background } = &mut artboard.kind {
            *background = Color32::WHITE;
            children.push(child);
        }

        let pixmap = render_layer(&artboard).expect("should render");
        assert_eq!((pixmap.width(), pixmap.height()), (30, 30));

        let bg_pixel = pixmap.pixel(1, 1).unwrap();
        assert_eq!((bg_pixel.red(), bg_pixel.green(), bg_pixel.blue()), (255, 255, 255));

        let child_pixel = pixmap.pixel(10, 10).unwrap();
        assert_eq!((child_pixel.red(), child_pixel.green(), child_pixel.blue()), (0, 255, 0));
    }

    #[test]
    fn renders_closed_path_as_filled_polygon() {
        // A 40x40 right triangle with straight (non-bezier) corners.
        let points = vec![
            PathPoint { anchor: Pos2::new(0.0, 0.0), handle_in: None, handle_out: None, point_type: PointType::Disconnected, corner_radius: 0.0 },
            PathPoint { anchor: Pos2::new(40.0, 0.0), handle_in: None, handle_out: None, point_type: PointType::Disconnected, corner_radius: 0.0 },
            PathPoint { anchor: Pos2::new(0.0, 40.0), handle_in: None, handle_out: None, point_type: PointType::Disconnected, corner_radius: 0.0 },
        ];
        let mut layer = Layer::new(
            "Path",
            Frame { pos: Pos2::new(0.0, 0.0), size: egui::Vec2::new(40.0, 40.0), rotation: 0.0 },
            LayerKind::Path { points, closed: true },
        );
        layer.style.fill = Some(crate::model::Paint::Solid(Color32::from_rgb(0, 0, 255)));
        layer.style.stroke = None;

        let pixmap = render_layer(&layer).expect("should render");

        // Inside the triangle (near the right-angle corner).
        let inside = pixmap.pixel(5, 5).expect("inside pixel");
        assert_eq!((inside.red(), inside.green(), inside.blue(), inside.alpha()), (0, 0, 255, 255));

        // Outside the triangle's hypotenuse, still inside the bounding box.
        let outside = pixmap.pixel(35, 35).expect("outside pixel");
        assert_eq!(outside.alpha(), 0);
    }

    #[test]
    fn exported_png_rounds_a_path_corner() {
        let points = vec![
            PathPoint { anchor: Pos2::new(0.0, 0.0), handle_in: None, handle_out: None, point_type: PointType::Disconnected, corner_radius: 0.0 },
            PathPoint { anchor: Pos2::new(40.0, 0.0), handle_in: None, handle_out: None, point_type: PointType::Disconnected, corner_radius: 15.0 },
            PathPoint { anchor: Pos2::new(40.0, 40.0), handle_in: None, handle_out: None, point_type: PointType::Disconnected, corner_radius: 0.0 },
            PathPoint { anchor: Pos2::new(0.0, 40.0), handle_in: None, handle_out: None, point_type: PointType::Disconnected, corner_radius: 0.0 },
        ];
        let mut layer = Layer::new(
            "Path",
            Frame { pos: Pos2::new(0.0, 0.0), size: egui::Vec2::new(40.0, 40.0), rotation: 0.0 },
            LayerKind::Path { points, closed: true },
        );
        layer.style.fill = Some(crate::model::Paint::Solid(Color32::from_rgb(0, 0, 255)));
        layer.style.stroke = None;

        let pixmap = render_layer(&layer).expect("should render");
        // Center is still filled.
        let inside = pixmap.pixel(20, 20).expect("inside pixel");
        assert_eq!(inside.alpha(), 255);
        // The rounded corner itself (top-right, where a plain square would
        // be filled right up to the edge) is now cut away.
        let corner = pixmap.pixel(39, 0).expect("corner pixel");
        assert_eq!(corner.alpha(), 0, "rounded corner should not be filled");
    }

    #[test]
    fn open_path_with_two_points_is_not_dropped() {
        let points = vec![
            PathPoint { anchor: Pos2::new(0.0, 2.0), handle_in: None, handle_out: None, point_type: PointType::Disconnected, corner_radius: 0.0 },
            PathPoint { anchor: Pos2::new(20.0, 2.0), handle_in: None, handle_out: None, point_type: PointType::Disconnected, corner_radius: 0.0 },
        ];
        let mut layer = Layer::new(
            "Path",
            Frame { pos: Pos2::new(0.0, 0.0), size: egui::Vec2::new(20.0, 4.0), rotation: 0.0 },
            LayerKind::Path { points, closed: false },
        );
        layer.style.fill = None;
        layer.style.stroke = Some(crate::model::Stroke { paint: crate::model::Paint::Solid(Color32::from_rgb(255, 0, 0)), width: 2.0 });

        let pixmap = render_layer(&layer).expect("should render");
        let on_line = pixmap.pixel(10, 2).expect("pixel on the stroked line");
        assert_eq!(on_line.alpha(), 255);
    }

    #[test]
    fn boolean_union_of_two_overlapping_stars_produces_one_merged_layer() {
        let mut page = Page::new("Test");

        let mut a = Layer::new(
            "A",
            Frame { pos: Pos2::new(0.0, 0.0), size: egui::Vec2::new(40.0, 40.0), rotation: 0.0 },
            LayerKind::Star { points: 5, inner_ratio: 0.5 },
        );
        a.style.fill = Some(crate::model::Paint::Solid(Color32::from_rgb(0, 200, 0)));
        a.style.stroke = None;
        let a_id = a.id;

        let mut b = Layer::new(
            "B",
            Frame { pos: Pos2::new(10.0, 10.0), size: egui::Vec2::new(40.0, 40.0), rotation: 0.0 },
            LayerKind::Star { points: 5, inner_ratio: 0.5 },
        );
        b.style.fill = Some(crate::model::Paint::Solid(Color32::from_rgb(0, 200, 0)));
        b.style.stroke = None;
        let b_id = b.id;

        page.layers.push(a);
        page.layers.push(b);

        let result_id = apply_boolean_op(&mut page, &[a_id, b_id], BoolOp::Union)
            .expect("union of overlapping stars should produce a layer");
        assert_eq!(page.layers.len(), 1, "originals should be replaced by the merged layer");
        let merged = page.find(result_id).expect("merged layer should be findable");
        assert!(matches!(merged.kind, LayerKind::CompoundPath { .. }));
    }

    #[test]
    fn boolean_union_of_two_overlapping_rects_fills_both_and_replaces_originals() {
        let mut page = Page::new("Test");

        let mut a = Layer::new(
            "A",
            Frame { pos: Pos2::new(0.0, 0.0), size: egui::Vec2::new(40.0, 40.0), rotation: 0.0 },
            LayerKind::Rectangle { corner_radius: CornerRadii::ZERO },
        );
        a.style.fill = Some(crate::model::Paint::Solid(Color32::from_rgb(0, 200, 0)));
        a.style.stroke = None;
        let a_id = a.id;

        let mut b = Layer::new(
            "B",
            Frame { pos: Pos2::new(20.0, 20.0), size: egui::Vec2::new(40.0, 40.0), rotation: 0.0 },
            LayerKind::Rectangle { corner_radius: CornerRadii::ZERO },
        );
        b.style.fill = Some(crate::model::Paint::Solid(Color32::from_rgb(0, 200, 0)));
        b.style.stroke = None;
        let b_id = b.id;

        page.layers.push(a);
        page.layers.push(b);

        let result_id = apply_boolean_op(&mut page, &[a_id, b_id], BoolOp::Union)
            .expect("union of overlapping rects should produce a layer");

        assert_eq!(page.layers.len(), 1, "originals should be replaced by the merged layer");
        let merged = page.find(result_id).expect("merged layer should be findable");
        assert!(matches!(merged.kind, LayerKind::CompoundPath { .. }));

        let pixmap = render_layer(merged).expect("should render");

        let in_a_only = pixmap.pixel(5, 5).expect("in-A pixel");
        assert_eq!(in_a_only.alpha(), 255, "point only inside A should be filled");

        let in_b_only = pixmap.pixel(55, 55).expect("in-B pixel");
        assert_eq!(in_b_only.alpha(), 255, "point only inside B should be filled");

        let overlap = pixmap.pixel(25, 25).expect("overlap pixel");
        assert_eq!(overlap.alpha(), 255, "overlap point should be filled");

        let outside = pixmap.pixel(2, 58).expect("outside pixel");
        assert_eq!(outside.alpha(), 0, "point outside both rects should be transparent");
    }

    #[test]
    fn boolean_union_respects_a_rotated_operands_actual_footprint() {
        let mut page = Page::new("Test");

        // A 40x10 rect rotated 90 degrees about its own center (20,5)
        // occupies a 10-wide, 40-tall vertical footprint (x:15..25,
        // y:-15..25) — nothing like its unrotated (x:0..40, y:0..10) frame.
        let mut a = Layer::new(
            "A",
            Frame { pos: Pos2::new(0.0, 0.0), size: egui::Vec2::new(40.0, 10.0), rotation: 90.0 },
            LayerKind::Rectangle { corner_radius: CornerRadii::ZERO },
        );
        a.style.fill = Some(crate::model::Paint::Solid(Color32::from_rgb(0, 200, 0)));
        a.style.stroke = None;
        let a_id = a.id;

        // Fully inside A's *rotated* footprint, but nowhere near A's
        // unrotated frame (entirely negative y).
        let mut b = Layer::new(
            "B",
            Frame { pos: Pos2::new(16.0, -14.0), size: egui::Vec2::new(8.0, 4.0), rotation: 0.0 },
            LayerKind::Rectangle { corner_radius: CornerRadii::ZERO },
        );
        b.style.fill = Some(crate::model::Paint::Solid(Color32::from_rgb(0, 200, 0)));
        b.style.stroke = None;
        let b_id = b.id;

        page.layers.push(a);
        page.layers.push(b);

        let result_id = apply_boolean_op(&mut page, &[a_id, b_id], BoolOp::Union)
            .expect("union should produce a layer");
        let merged = page.find(result_id).expect("merged layer should be findable");

        // If rotation were ignored, `flatten_layer` would flatten A to its
        // unrotated (40x10) frame, disjoint from B, and the merged bounding
        // box would span the full x:0..40 range. Respecting rotation instead
        // collapses to (B being a strict subset of A's rotated footprint)
        // just A's own rotated bounds: ~10 wide, ~40 tall.
        let bounds = merged.frame.bounds();
        assert!((bounds.width() - 10.0).abs() < 1.0, "width={}", bounds.width());
        assert!((bounds.height() - 40.0).abs() < 1.0, "height={}", bounds.height());
    }

    #[test]
    fn boolean_subtract_punches_a_hole() {
        let mut page = Page::new("Test");

        // Backmost (subtracted-from) shape: a 60x60 square.
        let mut base = Layer::new(
            "Base",
            Frame { pos: Pos2::new(0.0, 0.0), size: egui::Vec2::new(60.0, 60.0), rotation: 0.0 },
            LayerKind::Rectangle { corner_radius: CornerRadii::ZERO },
        );
        base.style.fill = Some(crate::model::Paint::Solid(Color32::from_rgb(200, 0, 0)));
        base.style.stroke = None;
        let base_id = base.id;

        // Frontmost shape, fully inside the base: a 20x20 square centered in it.
        let mut hole = Layer::new(
            "Hole",
            Frame { pos: Pos2::new(20.0, 20.0), size: egui::Vec2::new(20.0, 20.0), rotation: 0.0 },
            LayerKind::Rectangle { corner_radius: CornerRadii::ZERO },
        );
        hole.style.fill = Some(crate::model::Paint::Solid(Color32::from_rgb(200, 0, 0)));
        hole.style.stroke = None;
        let hole_id = hole.id;

        page.layers.push(base);
        page.layers.push(hole);

        let result_id = apply_boolean_op(&mut page, &[base_id, hole_id], BoolOp::Subtract)
            .expect("subtracting an inner square should produce a layer");
        let merged = page.find(result_id).expect("merged layer should be findable");

        let pixmap = render_layer(merged).expect("should render");

        let in_hole = pixmap.pixel(30, 30).expect("hole pixel");
        assert_eq!(in_hole.alpha(), 0, "the subtracted region should be transparent (a real hole)");

        let outside_hole = pixmap.pixel(5, 5).expect("remaining ring pixel");
        assert_eq!(outside_hole.alpha(), 255, "the rest of the base square should still be filled");
    }

    #[test]
    fn boolean_group_renders_live_from_still_editable_children() {
        let mut page = Page::new("Test");

        let mut a = Layer::new(
            "A",
            Frame { pos: Pos2::new(0.0, 0.0), size: egui::Vec2::new(40.0, 40.0), rotation: 0.0 },
            LayerKind::Rectangle { corner_radius: CornerRadii::ZERO },
        );
        a.style.fill = Some(crate::model::Paint::Solid(Color32::from_rgb(0, 200, 0)));
        a.style.stroke = None;
        let a_id = a.id;

        let mut b = Layer::new(
            "B",
            Frame { pos: Pos2::new(20.0, 20.0), size: egui::Vec2::new(40.0, 40.0), rotation: 0.0 },
            LayerKind::Rectangle { corner_radius: CornerRadii::ZERO },
        );
        b.style.fill = Some(crate::model::Paint::Solid(Color32::from_rgb(0, 0, 200)));
        b.style.stroke = None;
        let b_id = b.id;

        page.layers.push(a);
        page.layers.push(b);

        let group_id = boolean_ops::create_boolean_group(&mut page, &[a_id, b_id], BoolOp::Union)
            .expect("union of overlapping rects should produce a live group");
        let group = page.find(group_id).expect("group should be findable");
        assert!(matches!(group.kind, LayerKind::BooleanGroup { .. }));
        // Originals stay as real, separately editable children, not flattened.
        let LayerKind::BooleanGroup { children } = &group.kind else { unreachable!() };
        assert_eq!(children.len(), 2);
        assert!(matches!(children[0].kind, LayerKind::Rectangle { .. }));
        assert!(matches!(children[1].kind, LayerKind::Rectangle { .. }));

        let pixmap = render_layer(group).expect("should render");
        let in_a_only = pixmap.pixel(5, 5).expect("in-A pixel");
        assert_eq!(in_a_only.alpha(), 255, "point only inside A should be filled");
        let in_b_only = pixmap.pixel(55, 55).expect("in-B pixel");
        assert_eq!(in_b_only.alpha(), 255, "point only inside B should be filled");
        let outside = pixmap.pixel(2, 58).expect("outside pixel");
        assert_eq!(outside.alpha(), 0, "point outside both rects should be transparent");
    }

    #[test]
    fn boolean_group_excludes_hidden_children_from_the_live_result() {
        let mut page = Page::new("Test");

        let mut base = Layer::new(
            "Base",
            Frame { pos: Pos2::new(0.0, 0.0), size: egui::Vec2::new(40.0, 40.0), rotation: 0.0 },
            LayerKind::Rectangle { corner_radius: CornerRadii::ZERO },
        );
        base.style.fill = Some(crate::model::Paint::Solid(Color32::from_rgb(0, 200, 0)));
        base.style.stroke = None;
        let base_id = base.id;

        let mut extra = Layer::new(
            "Extra",
            Frame { pos: Pos2::new(60.0, 0.0), size: egui::Vec2::new(20.0, 20.0), rotation: 0.0 },
            LayerKind::Rectangle { corner_radius: CornerRadii::ZERO },
        );
        extra.style.fill = Some(crate::model::Paint::Solid(Color32::from_rgb(0, 200, 0)));
        extra.style.stroke = None;
        let extra_id = extra.id;

        page.layers.push(base);
        page.layers.push(extra);

        let group_id = boolean_ops::create_boolean_group(&mut page, &[base_id, extra_id], BoolOp::Union)
            .expect("union should produce a live group");
        let group = page.find_mut(group_id).expect("group should be findable");
        let LayerKind::BooleanGroup { children } = &mut group.kind else { unreachable!() };
        children[1].visible = false;

        let group = page.find(group_id).expect("group should be findable");
        let pixmap = render_layer(group).expect("should render");
        // The group's own frame still spans both children's original bounds
        // (unchanged by hiding one), but the hidden "Extra" rect no longer
        // contributes any fill — only "Base"'s region should render.
        let base_region = pixmap.pixel(5, 5).expect("base pixel");
        assert_eq!(base_region.alpha(), 255, "visible base child should still render");
        let hidden_region = pixmap.pixel(65, 5).expect("hidden-child pixel");
        assert_eq!(hidden_region.alpha(), 0, "hidden child should not contribute to the combined result");
    }

    #[test]
    fn boolean_group_add_of_overlapping_rects_shows_an_even_odd_hole_in_the_overlap() {
        // Documents the intentional edge case noted on `BoolOp::Add` and
        // `boolean_ops::point_in_multipolygon`: Add concatenates operands
        // with no clipping, so two overlapping pieces both cover the overlap
        // region — which an even-odd fill (required for Subtract/Difference
        // holes elsewhere) renders as transparent, not doubly-filled.
        let mut page = Page::new("Test");

        let mut a = Layer::new(
            "A",
            Frame { pos: Pos2::new(0.0, 0.0), size: egui::Vec2::new(40.0, 40.0), rotation: 0.0 },
            LayerKind::Rectangle { corner_radius: CornerRadii::ZERO },
        );
        a.style.fill = Some(crate::model::Paint::Solid(Color32::from_rgb(0, 200, 0)));
        a.style.stroke = None;
        let a_id = a.id;

        let mut b = Layer::new(
            "B",
            Frame { pos: Pos2::new(20.0, 20.0), size: egui::Vec2::new(40.0, 40.0), rotation: 0.0 },
            LayerKind::Rectangle { corner_radius: CornerRadii::ZERO },
        );
        b.style.fill = Some(crate::model::Paint::Solid(Color32::from_rgb(0, 200, 0)));
        b.style.stroke = None;
        let b_id = b.id;

        page.layers.push(a);
        page.layers.push(b);

        let group_id = boolean_ops::create_boolean_group(&mut page, &[a_id, b_id], BoolOp::Add)
            .expect("add of overlapping rects should produce a live group");
        let group = page.find(group_id).expect("group should be findable");

        let pixmap = render_layer(group).expect("should render");
        let in_a_only = pixmap.pixel(5, 5).expect("in-A-only pixel");
        assert_eq!(in_a_only.alpha(), 255, "region only covered by A should be filled");
        let overlap = pixmap.pixel(25, 25).expect("overlap pixel");
        assert_eq!(overlap.alpha(), 0, "overlap region should be an even-odd hole, not filled");
    }

    #[test]
    fn renders_text_glyphs_as_opaque_pixels_on_transparent_background() {
        let mut layer = Layer::new(
            "Text",
            Frame { pos: Pos2::new(0.0, 0.0), size: egui::Vec2::new(120.0, 40.0), rotation: 0.0 },
            LayerKind::Text {
                content: "W".to_string(),
                font_size: 32.0,
                font: TextFont::Proportional,
                align: TextAlign::Left,
                vertical_align: VerticalAlign::Top,
                resize: TextResize::Fixed,
                line_height: None,
                letter_spacing: 0.0,
                paragraph_spacing: 0.0,
                bold: false,
                italic: false,
                underline: false,
                strikethrough: false,
                transform: TextTransform::None,
                list: ListType::None,
                list_start: 1,
                style_id: None,
                runs: Vec::new(),
                path_attachment: None,
            },
        );
        layer.style.fill = Some(crate::model::Paint::Solid(Color32::BLACK));
        layer.style.stroke = None;

        let pixmap = render_layer(&layer).expect("should render");
        let any_opaque = (0..pixmap.width()).any(|x| {
            (0..pixmap.height()).any(|y| pixmap.pixel(x, y).map(|p| p.alpha() > 0).unwrap_or(false))
        });
        assert!(any_opaque, "expected at least one covered pixel from the rasterized glyph");
    }

    #[test]
    fn system_font_not_installed_falls_back_to_proportional_instead_of_drawing_nothing() {
        let mut layer = Layer::new(
            "Text",
            Frame { pos: Pos2::new(0.0, 0.0), size: egui::Vec2::new(120.0, 40.0), rotation: 0.0 },
            LayerKind::Text {
                content: "W".to_string(),
                font_size: 32.0,
                font: TextFont::System("Definitely Not An Installed Font Xyzzy".to_string()),
                align: TextAlign::Left,
                vertical_align: VerticalAlign::Top,
                resize: TextResize::Fixed,
                line_height: None,
                letter_spacing: 0.0,
                paragraph_spacing: 0.0,
                bold: false,
                italic: false,
                underline: false,
                strikethrough: false,
                transform: TextTransform::None,
                list: ListType::None,
                list_start: 1,
                style_id: None,
                runs: Vec::new(),
                path_attachment: None,
            },
        );
        layer.style.fill = Some(crate::model::Paint::Solid(Color32::BLACK));
        layer.style.stroke = None;

        let pixmap = render_layer(&layer).expect("should render");
        let any_opaque = (0..pixmap.width()).any(|x| {
            (0..pixmap.height()).any(|y| pixmap.pixel(x, y).map(|p| p.alpha() > 0).unwrap_or(false))
        });
        assert!(any_opaque, "an unresolvable system font should fall back to rendering with Proportional, not draw nothing");
    }

    #[test]
    fn right_aligned_text_hugs_the_right_edge_of_the_frame() {
        let mut layer = Layer::new(
            "Text",
            Frame { pos: Pos2::new(0.0, 0.0), size: egui::Vec2::new(200.0, 40.0), rotation: 0.0 },
            LayerKind::Text {
                content: "W".to_string(),
                font_size: 32.0,
                font: TextFont::Proportional,
                align: TextAlign::Right,
                vertical_align: VerticalAlign::Top,
                resize: TextResize::Fixed,
                line_height: None,
                letter_spacing: 0.0,
                paragraph_spacing: 0.0,
                bold: false,
                italic: false,
                underline: false,
                strikethrough: false,
                transform: TextTransform::None,
                list: ListType::None,
                list_start: 1,
                style_id: None,
                runs: Vec::new(),
                path_attachment: None,
            },
        );
        layer.style.fill = Some(crate::model::Paint::Solid(Color32::BLACK));
        layer.style.stroke = None;

        let pixmap = render_layer(&layer).expect("should render");
        let mut max_covered_x = None;
        for x in 0..pixmap.width() {
            for y in 0..pixmap.height() {
                if pixmap.pixel(x, y).map(|p| p.alpha() > 0).unwrap_or(false) {
                    max_covered_x = Some(x);
                }
            }
        }
        let max_covered_x = max_covered_x.expect("glyph should cover some pixel");
        assert!(
            max_covered_x as f32 > pixmap.width() as f32 * 0.7,
            "right-aligned glyph should sit near the right edge, got max covered x = {max_covered_x}"
        );
    }

    fn text_layer(content: &str, font_size: f32, size: egui::Vec2, align: TextAlign) -> Layer {
        let mut layer = Layer::new(
            "Text",
            Frame { pos: Pos2::new(0.0, 0.0), size, rotation: 0.0 },
            LayerKind::Text {
                content: content.to_string(),
                font_size,
                font: TextFont::Proportional,
                align,
                vertical_align: VerticalAlign::Top,
                resize: TextResize::Fixed,
                line_height: None,
                letter_spacing: 0.0,
                paragraph_spacing: 0.0,
                bold: false,
                italic: false,
                underline: false,
                strikethrough: false,
                transform: TextTransform::None,
                list: ListType::None,
                list_start: 1,
                style_id: None,
                runs: Vec::new(),
                path_attachment: None,
            },
        );
        layer.style.fill = Some(crate::model::Paint::Solid(Color32::BLACK));
        layer.style.stroke = None;
        layer
    }

    fn opaque_pixel_count(pixmap: &Pixmap) -> usize {
        (0..pixmap.width())
            .flat_map(|x| (0..pixmap.height()).map(move |y| (x, y)))
            .filter(|&(x, y)| pixmap.pixel(x, y).map(|p| p.alpha() > 0).unwrap_or(false))
            .count()
    }

    #[test]
    fn underline_and_strikethrough_add_extra_opaque_pixels() {
        let mut without_decoration = text_layer("I", 32.0, egui::Vec2::new(120.0, 60.0), TextAlign::Left);
        let mut with_underline = without_decoration.clone();
        let mut with_strikethrough = without_decoration.clone();
        if let LayerKind::Text { underline, .. } = &mut with_underline.kind {
            *underline = true;
        }
        if let LayerKind::Text { strikethrough, .. } = &mut with_strikethrough.kind {
            *strikethrough = true;
        }
        without_decoration.style.fill = Some(crate::model::Paint::Solid(Color32::BLACK));

        let base = opaque_pixel_count(&render_layer(&without_decoration).expect("should render"));
        let underlined = opaque_pixel_count(&render_layer(&with_underline).expect("should render"));
        let struck = opaque_pixel_count(&render_layer(&with_strikethrough).expect("should render"));
        assert!(underlined > base, "underline should add extra opaque pixels below the glyph");
        assert!(struck > base, "strikethrough should add extra opaque pixels through the glyph");
    }

    #[test]
    fn fixed_width_box_wraps_long_content_onto_a_second_line() {
        let layer = text_layer("aaaaaaaa bbbbbbbb", 24.0, egui::Vec2::new(70.0, 80.0), TextAlign::Left);
        let pixmap = render_layer(&layer).expect("should render");
        let opaque_in_rows = |y0: u32, y1: u32| {
            (0..pixmap.width())
                .any(|x| (y0..y1.min(pixmap.height())).any(|y| pixmap.pixel(x, y).map(|p| p.alpha() > 0).unwrap_or(false)))
        };
        assert!(opaque_in_rows(0, 30), "expected a first line near the top");
        assert!(
            opaque_in_rows(pixmap.height() / 2, pixmap.height()),
            "70px is too narrow for both words on one line, so a wrapped second line should reach the lower half"
        );
    }

    #[test]
    fn justified_non_last_line_stretches_further_right_than_left_aligned() {
        // Narrow enough that "aaaa bbbb" (line 1, non-last) wraps before
        // "cccc" (line 2, last — never stretched, matching typographic
        // convention and `canvas/`'s `egui::LayoutJob` behavior), wide
        // enough that "aaaa bbbb" alone still fits on one line.
        let size = egui::Vec2::new(100.0, 150.0);
        let left = text_layer("aaaa bbbb cccc", 24.0, size, TextAlign::Left);
        let justify = text_layer("aaaa bbbb cccc", 24.0, size, TextAlign::Justify);
        let left_pixmap = render_layer(&left).expect("should render");
        let justify_pixmap = render_layer(&justify).expect("should render");

        // Both wrap identically (same content/size/font); restrict the scan
        // to the first line's row band (top quarter) so this only measures
        // the non-last, justify-stretched line.
        let max_x_in_top_quarter = |pixmap: &Pixmap| {
            let mut max_x = 0u32;
            for x in 0..pixmap.width() {
                for y in 0..(pixmap.height() / 4) {
                    if pixmap.pixel(x, y).map(|p| p.alpha() > 0).unwrap_or(false) {
                        max_x = max_x.max(x);
                    }
                }
            }
            max_x
        };
        let left_max = max_x_in_top_quarter(&left_pixmap);
        let justify_max = max_x_in_top_quarter(&justify_pixmap);
        assert!(
            justify_max > left_max,
            "justified first line (max_x={justify_max}) should stretch further right than left-aligned (max_x={left_max})"
        );
    }

    fn run_style(font_size: f32) -> RunStyle {
        RunStyle {
            font: TextFont::Proportional,
            font_size,
            color: None,
            bold: false,
            italic: false,
            underline: false,
            strikethrough: false,
        }
    }

    #[test]
    fn rich_text_second_run_color_shows_up_distinct_from_the_first() {
        // "AA" in black, "BB" in red — scan the rendered row for both a
        // black-ish and a distinctly red pixel.
        let mut layer = text_layer("AABB", 40.0, egui::Vec2::new(120.0, 60.0), TextAlign::Left);
        if let LayerKind::Text { runs, .. } = &mut layer.kind {
            *runs = vec![
                TextRun { len: 2, style: run_style(40.0) },
                TextRun { len: 2, style: RunStyle { color: Some(Color32::from_rgb(220, 20, 20)), ..run_style(40.0) } },
            ];
        }
        let pixmap = render_layer(&layer).expect("should render");
        let mut saw_black = false;
        let mut saw_red = false;
        for x in 0..pixmap.width() {
            for y in 0..pixmap.height() {
                let Some(p) = pixmap.pixel(x, y) else { continue };
                if p.alpha() == 0 {
                    continue;
                }
                if p.red() > 150 && p.green() < 80 && p.blue() < 80 {
                    saw_red = true;
                } else if p.red() < 80 && p.green() < 80 && p.blue() < 80 {
                    saw_black = true;
                }
            }
        }
        assert!(saw_black, "expected the first (default black) run to render");
        assert!(saw_red, "expected the second run's color override to render");
    }

    #[test]
    fn rich_text_run_bold_thickens_strokes_same_as_whole_layer_bold() {
        let plain = text_layer("W", 40.0, egui::Vec2::new(80.0, 60.0), TextAlign::Left);
        let mut whole_layer_bold = plain.clone();
        if let LayerKind::Text { bold, .. } = &mut whole_layer_bold.kind {
            *bold = true;
        }
        let mut run_bold = plain.clone();
        if let LayerKind::Text { runs, .. } = &mut run_bold.kind {
            *runs = vec![TextRun { len: 1, style: RunStyle { bold: true, ..run_style(40.0) } }];
        }

        let plain_count = opaque_pixel_count(&render_layer(&plain).unwrap());
        let whole_bold_count = opaque_pixel_count(&render_layer(&whole_layer_bold).unwrap());
        let run_bold_count = opaque_pixel_count(&render_layer(&run_bold).unwrap());

        assert!(whole_bold_count > plain_count, "whole-layer bold (faux double-draw) should thicken the glyph");
        assert!(run_bold_count > plain_count, "a bold run (real bold-weight face) should thicken the glyph too");
    }

    #[test]
    fn rich_text_wraps_when_a_run_font_size_makes_a_word_too_wide() {
        // Synthetic per-char width: chars tagged with run 0 are drawn
        // "twice as wide" (simulating a much bigger font size than the
        // rest) — deterministic, unlike scanning rendered pixels against
        // real font metrics.
        let measure = |chars: &[(char, Option<usize>)]| -> f32 {
            chars.iter().map(|&(_, idx)| if idx == Some(0) { 20.0 } else { 10.0 }).sum()
        };
        let line: Vec<(char, Option<usize>)> = "aa bb"
            .chars()
            .map(|c| (c, if c == 'a' { Some(0) } else { None }))
            .collect();
        // "aa" (run 0) alone is 40px; "bb" alone is 20px; joined with a
        // 10px space that's 70px — over a 45px max_width, so they must
        // wrap onto separate lines even though the same content at a
        // uniform (non-run) width would comfortably fit on one.
        let wrapped = wrap_line_rich(&line, &measure, 10.0, 45.0);
        assert_eq!(wrapped.len(), 2, "expected the run's larger characters to force a wrap that a uniform-width line wouldn't need");
    }

    #[test]
    fn rich_text_wraps_visibly_onto_a_second_line_when_forced() {
        // Not trying to predict exact real-font wrap points (see the
        // deterministic `wrap_line_rich` test above for that) — just
        // confirms the whole pipeline (tagging → rich wrap → draw) ends up
        // with content in both halves of a box too narrow for the content
        // at any single line.
        let mut layer = text_layer("aaaaaaaa bbbbbbbb", 24.0, egui::Vec2::new(70.0, 80.0), TextAlign::Left);
        if let LayerKind::Text { resize, runs, .. } = &mut layer.kind {
            *resize = TextResize::Fixed;
            *runs = vec![
                TextRun { len: "aaaaaaaa ".chars().count(), style: run_style(24.0) },
                TextRun { len: "bbbbbbbb".chars().count(), style: run_style(24.0) },
            ];
        }
        let pixmap = render_layer(&layer).expect("should render");
        let opaque_in_band = |y_range: std::ops::Range<u32>| {
            (0..pixmap.width())
                .any(|x| y_range.clone().any(|y| pixmap.pixel(x, y).map(|p| p.alpha() > 0).unwrap_or(false)))
        };
        assert!(opaque_in_band(0..(pixmap.height() / 2)), "expected content on the first wrapped line");
        assert!(opaque_in_band((pixmap.height() / 2)..pixmap.height()), "expected content wrapped onto a second line");
    }
}
