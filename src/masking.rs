//! "Use shape as mask" (see `Layer::is_mask`/`Layer::ignore_mask`
//! in `model/layer.rs`). This module holds the one algorithm both renderers
//! share — `export/` composites directly with it, `canvas/` uses it to
//! rasterize a masked run into an offscreen `Pixmap` that gets cached as an
//! egui texture (mirroring how `canvas/texture_caches.rs`'s `ImageTextureCache` handles
//! `Image` layers) — so canvas and export can never disagree about what a
//! mask looks like.
//!
//! Deliberately has no `egui`/canvas dependency: it operates purely on
//! `tiny_skia::Pixmap` and `Layer`, taking the actual per-layer drawing
//! (`export::draw_layer`) as a callback so it doesn't need to know how a
//! `Layer` is rasterized.

use egui::{Pos2, Vec2};
use tiny_skia::{Mask, MaskType, Pixmap, PixmapPaint, Transform};

use crate::model::Layer;

/// One drawable unit within a single parent's `children` list, after
/// resolving `is_mask`/`ignore_mask` — see `partition_mask_runs`.
pub enum RenderUnit<'a> {
    /// Drawn exactly as before masking existed.
    Plain(&'a Layer),
    /// `content` (in original back-to-front order) clipped to `mask`'s own
    /// silhouette; `mask`'s own fill/stroke are never drawn. `segment`
    /// numbers the pieces (from the back, starting at 0) one mask's run is
    /// split into by interleaved `ignore_mask` layers, so a cache keyed per
    /// unit can tell the pieces of the same mask apart.
    Masked { mask: &'a Layer, content: Vec<&'a Layer>, segment: usize },
}

/// Groups a parent's `children` (already in back-to-front draw order, same
/// convention as `Page::layers`/`LayerKind::children`) into `RenderUnit`s.
///
/// A layer with `is_mask` masks the run of plain siblings immediately below
/// it (nearer the start of `children`) back to the previous mask or the
/// start of the list. A mask with nothing below it to mask is dropped
/// entirely (an empty mask is invisible). An `is_mask` layer
/// that's also `!visible` has no masking effect at all (falls through to
/// `pending` like a normal layer, where `draw_layer`'s own `!visible` check
/// then draws it as nothing) — same "an invisible layer has no effect"
/// principle already applied everywhere else in this codebase, extended to
/// masks: hiding a mask via its eye icon reveals its content normally,
/// rather than hiding the content along with it. `ignore_mask`
/// layers opt out and always emit as `Plain` at their exact depth: an
/// `ignore_mask` layer interleaved inside a mask's run splits that run into
/// separately masked pieces below and above it (see `split_run_around_ignored`).
pub fn partition_mask_runs(children: &[Layer]) -> Vec<RenderUnit<'_>> {
    let mut units = Vec::new();
    let mut pending: Vec<&Layer> = Vec::new();
    for child in children {
        if child.is_mask && child.visible {
            units.extend(split_run_around_ignored(child, std::mem::take(&mut pending)));
        } else {
            pending.push(child);
        }
    }
    units.extend(pending.into_iter().map(RenderUnit::Plain));
    units
}

/// Turns one mask's `run` (back-to-front) into units: each maximal stretch
/// of non-`ignore_mask` layers becomes a `Masked` piece, each `ignore_mask`
/// layer a `Plain` unit between them. A run with no maskable layer at all
/// yields only `Plain` units (the mask itself is dropped, as with an empty run).
fn split_run_around_ignored<'a>(mask: &'a Layer, run: Vec<&'a Layer>) -> Vec<RenderUnit<'a>> {
    let mut units = Vec::new();
    let mut content: Vec<&Layer> = Vec::new();
    let mut segment = 0;
    for layer in run {
        if layer.ignore_mask {
            if !content.is_empty() {
                units.push(RenderUnit::Masked { mask, content: std::mem::take(&mut content), segment });
                segment += 1;
            }
            units.push(RenderUnit::Plain(layer));
        } else {
            content.push(layer);
        }
    }
    if !content.is_empty() {
        units.push(RenderUnit::Masked { mask, content, segment });
    }
    units
}

/// Renders `mask` and `content` into two scratch pixmaps the same size as
/// `target`, clips the content scratch to the mask scratch's alpha, and
/// composites the result onto `target` at `(0, 0)`. `offset`/`parent_opacity`
/// are forwarded to `draw_layer` exactly as a normal recursive draw call
/// would (see `export::draw_layer`'s own `offset` accumulation convention) —
/// callers are responsible for `target`, `offset` and scale all agreeing on
/// the same coordinate mapping (export/ draws at 1 document unit = 1
/// pixel; `canvas/texture_caches.rs`'s texture cache draws into a pixmap cropped and sized to
/// the mask's own bounds, with `offset` shifted so the mask's local origin
/// lands at the scratch pixmaps' `(0, 0)`).
///
/// This is an alpha mask: content shows through in proportion to the
/// mask's rendered alpha, so the mask's own `opacity`, `fill_opacity`,
/// gradient stops and missing fill (only its stroke then reveals content)
/// all affect the result. Ambient `parent_opacity` is *not* applied to the
/// mask pass (only to `content`), so it isn't counted twice. The mask's own
/// colors never appear in the output.
pub fn composite_masked_run(
    target: &mut Pixmap,
    mask: &Layer,
    content: &[&Layer],
    offset: Vec2,
    parent_opacity: f32,
    draw_layer: impl Fn(&mut Pixmap, &Layer, Vec2, f32),
) {
    let (width, height) = (target.width(), target.height());
    let Some(mut mask_scratch) = Pixmap::new(width, height) else { return };
    draw_layer(&mut mask_scratch, mask, offset, 1.0);
    let sk_mask = Mask::from_pixmap(mask_scratch.as_ref(), MaskType::Alpha);

    let Some(mut content_scratch) = Pixmap::new(width, height) else { return };
    for layer in content {
        draw_layer(&mut content_scratch, layer, offset, parent_opacity);
    }
    content_scratch.apply_mask(&sk_mask);

    target.draw_pixmap(0, 0, content_scratch.as_ref(), &PixmapPaint::default(), Transform::identity(), None);
}

/// Alpha-accurate point test against `mask`'s own silhouette, in the same
/// parent-relative coordinate space as `point` (matching `mask.frame`'s own
/// space) — backs `canvas/hit_test.rs`'s hit-testing so a click inside a masked
/// content layer's bounding box, but outside the mask's actual coverage,
/// doesn't select it.
///
/// Only ever called on an actual click, not per frame — `canvas/texture_caches.rs`'s own
/// on-screen masked-run texture (`MaskedGroupTextureCache`) can't be reused
/// here since egui textures live GPU-side with no CPU read-back, so this
/// re-rasterizes `mask` alone (via `export::draw_layer`, same as
/// `composite_masked_run`'s own mask pass) into a scratch pixmap sized to
/// its bounds, which is cheap enough at click-time cadence.
pub fn mask_covers_point(mask: &Layer, point: Pos2) -> bool {
    let bounds = mask.frame.rotated_bounds();
    if !bounds.contains(point) {
        return false;
    }
    let width = bounds.width().round().max(1.0) as u32;
    let height = bounds.height().round().max(1.0) as u32;
    let Some(mut pixmap) = Pixmap::new(width, height) else { return false };
    let render_offset = Vec2::new(-bounds.min.x, -bounds.min.y);
    crate::export::draw_layer(&mut pixmap, mask, std::slice::from_ref(mask), render_offset, 1.0);
    let px = ((point.x - bounds.min.x).floor().max(0.0) as u32).min(width.saturating_sub(1));
    let py = ((point.y - bounds.min.y).floor().max(0.0) as u32).min(height.saturating_sub(1));
    pixmap.pixel(px, py).is_some_and(|p| p.alpha() > 0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{CornerRadii, Frame, LayerKind, Paint};
    use egui::{Color32, Pos2};

    fn rect_layer(name: &str, pos: Pos2, size: Vec2, fill: Color32) -> Layer {
        let mut layer = Layer::new(name, Frame { pos, size, rotation: 0.0 }, LayerKind::Rectangle { corner_radius: CornerRadii::ZERO });
        layer.style.fill = Some(Paint::Solid(fill));
        layer.style.stroke = None;
        layer
    }

    fn oval_layer(name: &str, pos: Pos2, size: Vec2) -> Layer {
        let mut layer = Layer::new(name, Frame { pos, size, rotation: 0.0 }, LayerKind::Oval);
        layer.style.fill = Some(Paint::Solid(Color32::WHITE));
        layer.style.stroke = None;
        layer
    }

    fn pixel(pixmap: &Pixmap, x: u32, y: u32) -> tiny_skia::PremultipliedColorU8 {
        pixmap.pixel(x, y).unwrap()
    }

    /// A circular mask over a filled square only reveals the square's
    /// content where the two overlap — the corners (inside the square,
    /// outside the circle) stay transparent.
    #[test]
    fn circle_mask_over_square_only_reveals_intersection() {
        let square = rect_layer("Square", Pos2::new(0.0, 0.0), Vec2::new(100.0, 100.0), Color32::RED);
        let mask = oval_layer("Mask", Pos2::new(0.0, 0.0), Vec2::new(100.0, 100.0));

        let mut target = Pixmap::new(100, 100).unwrap();
        composite_masked_run(&mut target, &mask, &[&square], Vec2::ZERO, 1.0, |px, l, off, op| crate::export::draw_layer(px, l, std::slice::from_ref(&mask), off, op));

        // Center: inside both the square and the circle -> opaque red.
        let center = pixel(&target, 50, 50);
        assert!(center.alpha() > 200, "center should be opaque, got alpha={}", center.alpha());
        assert!(center.red() > 200, "center should be red");

        // Corner: inside the square's bbox but outside the inscribed circle -> transparent.
        let corner = pixel(&target, 2, 2);
        assert_eq!(corner.alpha(), 0, "corner outside the circle should be fully transparent");
    }

    /// A masked `Text` layer's `path_attachment` can resolve against a
    /// target that isn't part of the masked run's own `content` at all —
    /// `composite_masked_run`'s `draw_layer` callback decides what "roots"
    /// means, and `canvas/texture_caches.rs`'s `MaskedGroupTextureCache` passes the whole
    /// page's layers, not just `mask`/`content` (see that cache's doc
    /// comment). Proven here directly against `export::draw_layer`, without
    /// needing an `egui::Context`/live texture cache.
    #[test]
    fn masked_path_attached_text_resolves_a_target_outside_its_own_content() {
        let guide = Layer::new("Guide", Frame::from_two_points(Pos2::new(0.0, 20.0), Pos2::new(100.0, 20.0)), LayerKind::Line);
        let target_id = guide.id;

        let mut text = Layer::new(
            "Label",
            Frame { pos: Pos2::new(0.0, 0.0), size: Vec2::new(10.0, 10.0), rotation: 0.0 },
            LayerKind::Text {
                content: "HI".to_string(),
                font_size: 20.0,
                font: crate::model::TextFont::Proportional,
                align: crate::model::TextAlign::Left,
                vertical_align: crate::model::VerticalAlign::Top,
                resize: crate::model::TextResize::Auto,
                line_height: None,
                letter_spacing: 0.0,
                paragraph_spacing: 0.0,
                bold: false,
                italic: false,
                underline: false,
                strikethrough: false,
                transform: crate::model::TextTransform::None,
                list: crate::model::ListType::None,
                list_start: 1,
                style_id: None,
                runs: Vec::new(),
                path_attachment: Some(crate::model::TextPathAttachment {
                    target: target_id,
                    mode: crate::model::TextPathMode::OnPath,
                    offset: 0.0,
                    start: 0.0,
                    flip: false,
                }),
            },
        );
        text.style.fill = Some(Paint::Solid(Color32::BLACK));
        text.style.stroke = None;

        let mask = rect_layer("Mask", Pos2::new(0.0, 0.0), Vec2::new(100.0, 40.0), Color32::WHITE);
        // `content` deliberately does NOT include `guide` — only `roots`
        // (standing in for `canvas/`'s page-wide layer list) does.
        let roots = vec![guide, mask.clone(), text.clone()];

        let mut target = Pixmap::new(100, 40).unwrap();
        composite_masked_run(&mut target, &mask, &[&text], Vec2::ZERO, 1.0, |px, l, off, op| crate::export::draw_layer(px, l, &roots, off, op));

        let any_opaque = target.pixels().iter().any(|p| p.alpha() > 0);
        assert!(any_opaque, "expected the masked text's on-path glyphs to render using a target outside its own masked content");
    }

    /// A mask with no content below it in the same parent has nothing to
    /// clip and is dropped entirely by `partition_mask_runs`.
    #[test]
    fn mask_with_no_preceding_content_is_dropped() {
        let mut mask_layer = oval_layer("Mask", Pos2::ZERO, Vec2::new(10.0, 10.0));
        mask_layer.is_mask = true;
        let children = vec![mask_layer];

        let units = partition_mask_runs(&children);
        assert_eq!(units.len(), 0);
    }

    /// `ignore_mask` opts a layer out of an active mask above it, keeping
    /// its exact depth (above the masked content it sits above).
    #[test]
    fn ignore_mask_layer_is_not_absorbed_into_the_masked_run() {
        let content = rect_layer("Content", Pos2::ZERO, Vec2::new(10.0, 10.0), Color32::BLUE);
        let mut ignored = rect_layer("Ignored", Pos2::ZERO, Vec2::new(10.0, 10.0), Color32::GREEN);
        ignored.ignore_mask = true;
        let mut mask_layer = oval_layer("Mask", Pos2::ZERO, Vec2::new(10.0, 10.0));
        mask_layer.is_mask = true;
        let children = vec![content, ignored, mask_layer];

        let units = partition_mask_runs(&children);
        assert_eq!(units.len(), 2);
        assert!(matches!(&units[0], RenderUnit::Masked { content, .. } if content.len() == 1 && content[0].name == "Content"));
        assert!(matches!(units[1], RenderUnit::Plain(l) if l.name == "Ignored"));
    }

    /// An `ignore_mask` layer interleaved in a run splits it into two
    /// pieces of the same mask, with the ignored layer drawn between them.
    #[test]
    fn interleaved_ignore_mask_layer_splits_the_run_at_its_exact_depth() {
        let below = rect_layer("Below", Pos2::ZERO, Vec2::new(10.0, 10.0), Color32::BLUE);
        let mut ignored = rect_layer("Ignored", Pos2::ZERO, Vec2::new(10.0, 10.0), Color32::GREEN);
        ignored.ignore_mask = true;
        let above = rect_layer("Above", Pos2::ZERO, Vec2::new(10.0, 10.0), Color32::RED);
        let mut mask_layer = oval_layer("Mask", Pos2::ZERO, Vec2::new(10.0, 10.0));
        mask_layer.is_mask = true;
        let children = vec![below, ignored, above, mask_layer];

        let units = partition_mask_runs(&children);
        assert_eq!(units.len(), 3);
        assert!(matches!(&units[0], RenderUnit::Masked { content, segment: 0, .. } if content[0].name == "Below"));
        assert!(matches!(units[1], RenderUnit::Plain(l) if l.name == "Ignored"));
        assert!(matches!(&units[2], RenderUnit::Masked { content, segment: 1, .. } if content[0].name == "Above"));
    }

    /// The mask layer's own fill never appears in the composited output —
    /// only its silhouette is used.
    #[test]
    fn masks_own_fill_is_never_drawn() {
        let square = rect_layer("Square", Pos2::new(0.0, 0.0), Vec2::new(100.0, 100.0), Color32::RED);
        let mut mask = oval_layer("Mask", Pos2::new(0.0, 0.0), Vec2::new(100.0, 100.0));
        mask.style.fill = Some(Paint::Solid(Color32::from_rgb(0, 255, 0))); // distinct from both Square's red and transparent.

        let mut target = Pixmap::new(100, 100).unwrap();
        composite_masked_run(&mut target, &mask, &[&square], Vec2::ZERO, 1.0, |px, l, off, op| crate::export::draw_layer(px, l, std::slice::from_ref(&mask), off, op));

        let center = pixel(&target, 50, 50);
        assert!(center.green() < 50, "mask's own green fill should not appear in the output");
        assert!(center.red() > 200, "should show the square's red through the mask instead");
    }

    /// `mask_covers_point` should follow the mask shape's actual silhouette,
    /// not just its bounding box.
    #[test]
    fn mask_covers_point_follows_the_shapes_silhouette_not_its_bbox() {
        let mask = oval_layer("Mask", Pos2::new(0.0, 0.0), Vec2::new(100.0, 100.0));

        assert!(mask_covers_point(&mask, Pos2::new(50.0, 50.0)), "center should be covered");
        assert!(!mask_covers_point(&mask, Pos2::new(2.0, 2.0)), "corner outside the inscribed circle should not be covered");
        assert!(!mask_covers_point(&mask, Pos2::new(-5.0, 50.0)), "outside the bounding box entirely should not be covered");
    }
}
