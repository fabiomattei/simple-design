use std::collections::HashMap;

use egui::{Color32, Pos2, Rect, Sense, Stroke as EguiStroke, Ui, Vec2};

use crate::alignment::DistributeAxis;
use crate::clipboard;
use crate::grouping;
use crate::history::History;
use crate::model::{
    ArrowCap, ColorAdjust, CornerRadii, Frame, Gradient, Guide, GuideOrientation, HalftoneFill, Layer, LayerId,
    LayerKind, NoiseFill, Page, Paint, PathPoint, PathPolygon, PatternFill, PointType, Style, TextAlign, TextFont,
    TextResize, VerticalAlign,
};
use crate::shapes::{ellipse_points, rotate_point, rotated_corners, rounded_rect_points};
use crate::text_layout::{self, TextStyleParams};
use crate::tools::Tool;

/// One decoded-and-adjusted `Image` layer's egui texture, plus the
/// `(version, color_adjust)` pair it was built from — cheap to compare every
/// frame, so a texture is only re-decoded/re-uploaded when either actually
/// changes (a destructive edit bumps `version`; a Color Adjust slider tweak
/// changes `color_adjust`), not on every redraw.
struct CachedImageTexture {
    version: LayerId,
    color_adjust: ColorAdjust,
    texture: egui::TextureHandle,
}

/// Per-canvas cache of `Image` layer textures, keyed by layer id. See
/// `CachedImageTexture` for the invalidation rule.
#[derive(Default)]
pub struct ImageTextureCache(HashMap<LayerId, CachedImageTexture>);

impl ImageTextureCache {
    fn get_or_load(
        &mut self,
        ctx: &egui::Context,
        layer_id: LayerId,
        encoded: &[u8],
        version: LayerId,
        color_adjust: ColorAdjust,
    ) -> Option<egui::TextureHandle> {
        if let Some(cached) = self.0.get(&layer_id) {
            if cached.version == version && cached.color_adjust == color_adjust {
                return Some(cached.texture.clone());
            }
        }
        let decoded = crate::image_ops::decode(encoded)?;
        let adjusted = crate::image_ops::apply_color_adjust(&decoded, color_adjust);
        let color_image = crate::image_ops::to_egui_color_image(&adjusted);
        let texture = ctx.load_texture(format!("image-layer-{layer_id}"), color_image, egui::TextureOptions::LINEAR);
        let handle = texture.clone();
        self.0.insert(layer_id, CachedImageTexture { version, color_adjust, texture });
        Some(handle)
    }

    /// Drops entries for layers no longer present on the active page, so a
    /// long session of inserting/deleting images doesn't grow this
    /// unboundedly. Cheap enough to call once per frame.
    fn evict_stale(&mut self, live_ids: &std::collections::HashSet<LayerId>) {
        self.0.retain(|id, _| live_ids.contains(id));
    }
}

/// One masked run's rasterized composite (see `masking::composite_masked_run`),
/// cached as an egui texture keyed by the mask layer's id — the same
/// texture-cache-over-a-CPU-rasterizer pattern as `ImageTextureCache` above,
/// since egui's immediate-mode `Painter` has no way to clip to an arbitrary
/// shape itself (only axis-aligned clip rects). `mask`/`content` are cloned
/// so a cheap `PartialEq` (both `Layer` derives it) tells us whether
/// anything relevant changed since the last build, without needing a
/// separate dirty-flag/version scheme threaded through every edit path that
/// could touch a masked layer.
///
/// Built at full opacity (`parent_opacity = 1.0`) regardless of the run's
/// ambient (ancestor-accumulated) opacity — that's applied at draw time via
/// the same `tint` mechanism `Image` layers use, so a change to an ancestor
/// `Group`/`Artboard`'s opacity slider doesn't need to invalidate this
/// cache. Each content layer's own `opacity` field, by contrast, *is* baked
/// into the texture, which is why it's part of the `PartialEq` key (it's a
/// plain field on the cloned `Layer`).
struct CachedMaskTexture {
    mask: Layer,
    content: Vec<Layer>,
    texture: egui::TextureHandle,
}

#[derive(Default)]
pub struct MaskedGroupTextureCache(HashMap<LayerId, CachedMaskTexture>);

impl MaskedGroupTextureCache {
    /// Returns the cached (or freshly built) texture for this masked run,
    /// plus the parent-space bounds (`mask.frame.rotated_bounds()`) it
    /// should be drawn into on screen.
    fn get_or_build(&mut self, ctx: &egui::Context, mask: &Layer, content: &[&Layer]) -> Option<(egui::TextureHandle, Rect)> {
        let bounds = mask.frame.rotated_bounds();
        let up_to_date = self.0.get(&mask.id).is_some_and(|cached| {
            cached.mask == *mask && cached.content.len() == content.len() && cached.content.iter().zip(content).all(|(a, b)| a == *b)
        });
        if !up_to_date {
            let width = bounds.width().round().max(1.0) as u32;
            let height = bounds.height().round().max(1.0) as u32;
            let mut pixmap = tiny_skia::Pixmap::new(width, height)?;
            // Shifts parent-space coordinates so `bounds.min` (the mask's
            // own top-left) lands at the scratch pixmap's `(0, 0)` — same
            // idea as `export::render_layer`'s `offset` computation.
            let render_offset = Vec2::new(-bounds.min.x, -bounds.min.y);
            crate::masking::composite_masked_run(&mut pixmap, mask, content, render_offset, 1.0, crate::export::draw_layer_with_shadows);
            let color_image = egui::ColorImage::from_rgba_premultiplied([width as usize, height as usize], pixmap.data());
            let texture = ctx.load_texture(format!("mask-group-{}", mask.id), color_image, egui::TextureOptions::LINEAR);
            self.0.insert(
                mask.id,
                CachedMaskTexture {
                    mask: mask.clone(),
                    content: content.iter().map(|l| (*l).clone()).collect(),
                    texture,
                },
            );
        }
        self.0.get(&mask.id).map(|cached| (cached.texture.clone(), bounds))
    }

    /// See `ImageTextureCache::evict_stale` — same rationale, keyed off the
    /// mask layer's id instead.
    fn evict_stale(&mut self, live_ids: &std::collections::HashSet<LayerId>) {
        self.0.retain(|id, _| live_ids.contains(id));
    }
}

/// One `Paint::Noise` fill's generated grain texture, plus the
/// `(fill, local_size, pixel_size)` triple it was built from — same
/// cheap-`PartialEq`-guarded invalidation rule as `CachedImageTexture`.
/// `local_size` is the layer's own unrotated `Frame::bounds().size()` (the
/// same document-unit space `NoiseFill::scale` is defined in — see
/// `noise_fill::sample`'s doc comment); `pixel_size` is how many texels the
/// texture is rasterized at, chosen from the shape's current on-screen size
/// so grain stays crisp while zoomed, independent of `local_size`.
struct CachedNoiseTexture {
    fill: NoiseFill,
    local_size: Vec2,
    pixel_size: (u32, u32),
    texture: egui::TextureHandle,
}

/// Per-canvas cache of `Paint::Noise` fill textures, keyed by layer id. See
/// `CachedNoiseTexture` for the invalidation rule.
#[derive(Default)]
pub struct NoiseTextureCache(HashMap<LayerId, CachedNoiseTexture>);

/// Texture resolution is quantized to the nearest multiple of this many
/// pixels, so small on-screen size changes (e.g. a slow zoom drag) don't
/// force a texture rebuild every frame.
const NOISE_TEXTURE_QUANTUM: u32 = 32;
/// Hard cap on a noise texture's side length, so a very large or heavily
/// zoomed-in shape doesn't allocate an unbounded GPU texture.
const NOISE_TEXTURE_MAX: u32 = 1024;

impl NoiseTextureCache {
    /// `screen_size` is the shape's current on-screen footprint (pixels),
    /// used only to pick a crisp-enough texture resolution — it does not
    /// affect the grain pattern itself, which is defined purely in
    /// `local_size`/`fill.scale` document-unit space (see `noise_fill::sample`).
    fn get_or_build(&mut self, ctx: &egui::Context, layer_id: LayerId, fill: &NoiseFill, local_size: Vec2, screen_size: Vec2) -> Option<egui::TextureHandle> {
        let quantize = |v: f32| ((v.ceil() as u32).max(1)).div_ceil(NOISE_TEXTURE_QUANTUM).max(1) * NOISE_TEXTURE_QUANTUM;
        let pixel_size = (quantize(screen_size.x).min(NOISE_TEXTURE_MAX), quantize(screen_size.y).min(NOISE_TEXTURE_MAX));
        if let Some(cached) = self.0.get(&layer_id) {
            if cached.fill == *fill && cached.local_size == local_size && cached.pixel_size == pixel_size {
                return Some(cached.texture.clone());
            }
        }
        let (tw, th) = pixel_size;
        let mut rgba = Vec::with_capacity((tw * th) as usize * 4);
        for j in 0..th {
            for i in 0..tw {
                let frac = Pos2::new((i as f32 + 0.5) / tw as f32, (j as f32 + 0.5) / th as f32);
                let local_point = Pos2::new(frac.x * local_size.x, frac.y * local_size.y);
                let c = crate::noise_fill::sample(fill, local_point, 1.0);
                rgba.extend_from_slice(&[c.r(), c.g(), c.b(), c.a()]);
            }
        }
        let color_image = egui::ColorImage::from_rgba_unmultiplied([tw as usize, th as usize], &rgba);
        let texture = ctx.load_texture(format!("noise-layer-{layer_id}"), color_image, egui::TextureOptions::NEAREST);
        let handle = texture.clone();
        self.0.insert(layer_id, CachedNoiseTexture { fill: *fill, local_size, pixel_size, texture });
        Some(handle)
    }

    /// See `ImageTextureCache::evict_stale` — same rationale.
    fn evict_stale(&mut self, live_ids: &std::collections::HashSet<LayerId>) {
        self.0.retain(|id, _| live_ids.contains(id));
    }
}

/// `Paint::Halftone`'s sibling of `CachedNoiseTexture`/`NoiseTextureCache` —
/// identical shape, sampling `halftone_fill::sample` instead of
/// `noise_fill::sample`.
struct CachedHalftoneTexture {
    fill: HalftoneFill,
    local_size: Vec2,
    pixel_size: (u32, u32),
    texture: egui::TextureHandle,
}

#[derive(Default)]
pub struct HalftoneTextureCache(HashMap<LayerId, CachedHalftoneTexture>);

impl HalftoneTextureCache {
    /// See `NoiseTextureCache::get_or_build` — same quantized-resolution
    /// rationale, same per-pixel-sample-function shape.
    fn get_or_build(&mut self, ctx: &egui::Context, layer_id: LayerId, fill: &HalftoneFill, local_size: Vec2, screen_size: Vec2) -> Option<egui::TextureHandle> {
        let quantize = |v: f32| ((v.ceil() as u32).max(1)).div_ceil(NOISE_TEXTURE_QUANTUM).max(1) * NOISE_TEXTURE_QUANTUM;
        let pixel_size = (quantize(screen_size.x).min(NOISE_TEXTURE_MAX), quantize(screen_size.y).min(NOISE_TEXTURE_MAX));
        if let Some(cached) = self.0.get(&layer_id) {
            if cached.fill == *fill && cached.local_size == local_size && cached.pixel_size == pixel_size {
                return Some(cached.texture.clone());
            }
        }
        let (tw, th) = pixel_size;
        let mut rgba = Vec::with_capacity((tw * th) as usize * 4);
        for j in 0..th {
            for i in 0..tw {
                let frac = Pos2::new((i as f32 + 0.5) / tw as f32, (j as f32 + 0.5) / th as f32);
                let local_point = Pos2::new(frac.x * local_size.x, frac.y * local_size.y);
                let c = crate::halftone_fill::sample(fill, local_point, 1.0);
                rgba.extend_from_slice(&[c.r(), c.g(), c.b(), c.a()]);
            }
        }
        let color_image = egui::ColorImage::from_rgba_unmultiplied([tw as usize, th as usize], &rgba);
        let texture = ctx.load_texture(format!("halftone-layer-{layer_id}"), color_image, egui::TextureOptions::NEAREST);
        let handle = texture.clone();
        self.0.insert(layer_id, CachedHalftoneTexture { fill: *fill, local_size, pixel_size, texture });
        Some(handle)
    }

    /// See `ImageTextureCache::evict_stale` — same rationale.
    fn evict_stale(&mut self, live_ids: &std::collections::HashSet<LayerId>) {
        self.0.retain(|id, _| live_ids.contains(id));
    }
}

/// `Paint::Pattern`'s texture cache — closer to `ImageTextureCache` than
/// `NoiseTextureCache` (it decodes bytes rather than sampling a pure
/// function). Keyed by `encoded` byte equality alone: `PatternFill`'s only
/// other field, `tile_width`, affects UV scale at draw time (see
/// `pattern_textured_mesh`), not the texture itself, so it isn't part of
/// the cache key. Returns the decoded image's `height/width` aspect ratio
/// alongside the texture — needed to turn `tile_width` into a tile height.
struct CachedPatternTexture {
    encoded: Vec<u8>,
    aspect: f32,
    texture: egui::TextureHandle,
}

#[derive(Default)]
pub struct PatternTextureCache(HashMap<LayerId, CachedPatternTexture>);

impl PatternTextureCache {
    fn get_or_build(&mut self, ctx: &egui::Context, layer_id: LayerId, fill: &PatternFill) -> Option<(egui::TextureHandle, f32)> {
        if let Some(cached) = self.0.get(&layer_id) {
            if cached.encoded == fill.encoded {
                return Some((cached.texture.clone(), cached.aspect));
            }
        }
        let decoded = crate::image_ops::decode(&fill.encoded)?;
        let aspect = decoded.height() as f32 / (decoded.width().max(1) as f32);
        let color_image = crate::image_ops::to_egui_color_image(&decoded);
        // `wrap_mode: Repeat` is what makes UV coordinates past `1.0`
        // (`pattern_textured_mesh`'s whole point) tile instead of clamp to
        // the edge texel.
        let options = egui::TextureOptions { wrap_mode: egui::TextureWrapMode::Repeat, ..egui::TextureOptions::LINEAR };
        let texture = ctx.load_texture(format!("pattern-layer-{layer_id}"), color_image, options);
        let handle = texture.clone();
        self.0.insert(layer_id, CachedPatternTexture { encoded: fill.encoded.clone(), aspect, texture });
        Some((handle, aspect))
    }

    /// See `ImageTextureCache::evict_stale` — same rationale.
    fn evict_stale(&mut self, live_ids: &std::collections::HashSet<LayerId>) {
        self.0.retain(|id, _| live_ids.contains(id));
    }
}

/// One layer's rendered drop/inner shadow textures (`Style::shadows`/
/// `inner_shadows`), keyed by layer id. Built from the layer's own
/// silhouette (`export::render_layer_plain` — always shadow-free, so this
/// never recurses into a child's own shadow; see `shadow.rs`'s module doc
/// comment for the overall shared-with-`export.rs` design) and cached the
/// same cheap-`PartialEq`-guarded way as `CachedMaskTexture`. Each shadow in
/// the stack gets its own texture (rather than compositing the stack into
/// one) since outer shadows can each be a different size/position (padding
/// depends on that shadow's own blur/spread) — simpler than merge-compositing
/// them for one extra draw call per stacked shadow, which in practice is
/// rarely more than two or three.
struct CachedShadowTextures {
    layer: Layer,
    /// Each outer shadow's texture plus where it lands, in the same
    /// parent-relative space as `layer.frame` (i.e. before `to_screen`).
    outer: Vec<(egui::TextureHandle, Rect)>,
    /// Each inner shadow's texture plus where it lands — always
    /// `layer.frame.rotated_bounds()`, repeated per shadow so draw-site code
    /// doesn't need to special-case inner vs. outer.
    inner: Vec<(egui::TextureHandle, Rect)>,
}

#[derive(Default)]
pub struct ShadowTextureCache(HashMap<LayerId, CachedShadowTextures>);

impl ShadowTextureCache {
    /// `None` if `layer` has no shadows at all (nothing to draw) or its
    /// silhouette failed to rasterize (zero-size layer).
    fn get_or_build(&mut self, ctx: &egui::Context, layer: &Layer) -> Option<&CachedShadowTextures> {
        let has_outer = !layer.style.shadows.is_empty();
        let has_inner = !layer.style.inner_shadows.is_empty();
        if !has_outer && !has_inner {
            self.0.remove(&layer.id);
            return None;
        }
        let up_to_date = self.0.get(&layer.id).is_some_and(|cached| cached.layer == *layer);
        if !up_to_date {
            let (silhouette, bounds) = crate::export::render_layer_plain(layer)?;
            let silhouette_origin = Vec2::new(bounds.min.x, bounds.min.y);

            let mut outer = Vec::new();
            for shadow in &layer.style.shadows {
                if let Some(crate::shadow::ShadowLayer { pixmap, origin }) =
                    crate::shadow::render_outer_shadow(&silhouette, silhouette_origin, shadow)
                {
                    let rect = Rect::from_min_size(Pos2::new(origin.x, origin.y), Vec2::new(pixmap.width() as f32, pixmap.height() as f32));
                    let color_image = egui::ColorImage::from_rgba_premultiplied([pixmap.width() as usize, pixmap.height() as usize], pixmap.data());
                    let texture = ctx.load_texture(format!("outer-shadow-{}-{}", layer.id, outer.len()), color_image, egui::TextureOptions::LINEAR);
                    outer.push((texture, rect));
                }
            }
            let mut inner = Vec::new();
            for shadow in &layer.style.inner_shadows {
                if let Some(pixmap) = crate::shadow::render_inner_shadow(&silhouette, shadow) {
                    let color_image = egui::ColorImage::from_rgba_premultiplied([pixmap.width() as usize, pixmap.height() as usize], pixmap.data());
                    let texture = ctx.load_texture(format!("inner-shadow-{}-{}", layer.id, inner.len()), color_image, egui::TextureOptions::LINEAR);
                    inner.push((texture, bounds));
                }
            }
            self.0.insert(layer.id, CachedShadowTextures { layer: layer.clone(), outer, inner });
        }
        self.0.get(&layer.id)
    }

    /// See `ImageTextureCache::evict_stale` — same rationale.
    fn evict_stale(&mut self, live_ids: &std::collections::HashSet<LayerId>) {
        self.0.retain(|id, _| live_ids.contains(id));
    }
}

/// A per-image "Edit Image" mode (Selection + Magic Wand, feeding
/// Crop/Fill/Delete), scoped to one `Image` layer at a time. Entered via the
/// inspector (`CanvasWidget::begin_image_edit`); while active, canvas
/// clicks/drags are captured for selection instead of the normal
/// move/resize/marquee behavior (see the `image_edit`-gated branch in
/// `CanvasWidget::ui`'s drag handling).
struct ImageEditState {
    layer_id: LayerId,
    /// Magic Wand color-distance tolerance, `0..=100` (see
    /// `image_ops::magic_wand_mask`).
    tolerance: f32,
    /// Row-major `width * height` selection mask in the image's own pixel
    /// space, captured at `begin_image_edit` — a destructive edit
    /// (Crop/Fill/Delete) always ends edit mode rather than trying to remap
    /// a stale mask onto possibly-new dimensions.
    mask: Vec<bool>,
    width: u32,
    height: u32,
    /// The color the inspector's "Fill" button last used/will use next —
    /// kept here (rather than in the stateless `ui/inspector.rs`) so it
    /// persists across frames alongside the rest of the edit session.
    fill_color: Color32,
    /// Cached translucent overlay texture visualizing `mask`; only rebuilt
    /// when `overlay_stale` (set on every mask mutation), since re-uploading
    /// a mask the size of a large photo every frame would be far too slow.
    overlay: Option<egui::TextureHandle>,
    overlay_stale: bool,
}

impl ImageEditState {
    fn mark_dirty(&mut self) {
        self.overlay_stale = true;
    }

    fn has_selection(&self) -> bool {
        self.mask.iter().any(|&m| m)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Handle {
    TopLeft,
    Top,
    TopRight,
    Right,
    BottomRight,
    Bottom,
    BottomLeft,
    Left,
}

impl Handle {
    const ALL: [Handle; 8] = [
        Handle::TopLeft,
        Handle::Top,
        Handle::TopRight,
        Handle::Right,
        Handle::BottomRight,
        Handle::Bottom,
        Handle::BottomLeft,
        Handle::Left,
    ];

    fn pos(&self, r: Rect) -> Pos2 {
        match self {
            Handle::TopLeft => r.left_top(),
            Handle::Top => r.center_top(),
            Handle::TopRight => r.right_top(),
            Handle::Right => r.right_center(),
            Handle::BottomRight => r.right_bottom(),
            Handle::Bottom => r.center_bottom(),
            Handle::BottomLeft => r.left_bottom(),
            Handle::Left => r.left_center(),
        }
    }

    /// Resizes `r` (in some consistent coordinate space) by dragging this
    /// handle to `p` (same space as `r`).
    fn resize(&self, r: Rect, p: Pos2) -> Rect {
        let mut min = r.min;
        let mut max = r.max;
        match self {
            Handle::TopLeft => {
                min.x = p.x;
                min.y = p.y;
            }
            Handle::Top => min.y = p.y,
            Handle::TopRight => {
                max.x = p.x;
                min.y = p.y;
            }
            Handle::Right => max.x = p.x,
            Handle::BottomRight => {
                max.x = p.x;
                max.y = p.y;
            }
            Handle::Bottom => max.y = p.y,
            Handle::BottomLeft => {
                min.x = p.x;
                max.y = p.y;
            }
            Handle::Left => min.x = p.x,
        }
        Rect::from_two_pos(min, max)
    }

    fn cursor(&self) -> egui::CursorIcon {
        match self {
            Handle::TopLeft | Handle::BottomRight => egui::CursorIcon::ResizeNwSe,
            Handle::TopRight | Handle::BottomLeft => egui::CursorIcon::ResizeNeSw,
            Handle::Top | Handle::Bottom => egui::CursorIcon::ResizeVertical,
            Handle::Left | Handle::Right => egui::CursorIcon::ResizeHorizontal,
        }
    }
}

const HANDLE_RADIUS: f32 = 4.5;
const HANDLE_HIT_RADIUS: f32 = 8.0;
/// Outer bound (in screen pixels, from a corner handle's center) of the
/// "hover here to rotate" zone — an annulus starting just past
/// `HANDLE_HIT_RADIUS` (which claims the resize hit-test first) and ending
/// here, so hovering a bit further out from a corner than a resize grab
/// switches to rotate instead, a common corner-rotate gesture.
const ROTATE_ZONE_OUTER_RADIUS: f32 = 20.0;
const SELECTION_COLOR: Color32 = Color32::from_rgb(0, 122, 255);

/// Width, in screen pixels, of the top/left ruler strips.
const RULER_SIZE: f32 = 20.0;
const RULER_BG: Color32 = Color32::from_gray(246);
const RULER_LINE: Color32 = Color32::from_gray(170);
const RULER_TEXT: Color32 = Color32::from_gray(100);
/// Color of a placed, static ruler guide.
const GUIDE_COLOR: Color32 = Color32::from_rgb(0, 170, 255);
/// How close (in screen pixels) the pointer must be to an existing guide
/// line to grab it for moving/deleting instead of starting a new one.
const GUIDE_HIT_SCREEN: f32 = 4.0;
/// Color of the transient "smart guide" alignment lines shown while
/// snapping to another layer's edge/center during a drag.
const SNAP_LINE_COLOR: Color32 = Color32::from_rgb(255, 0, 170);
/// The Option-hover distance-measurement overlay color.
const MEASURE_COLOR: Color32 = Color32::from_rgb(255, 64, 64);
/// Snap distance, in screen pixels (divided by zoom to get a doc-space
/// threshold), for guides/object-edge/pixel snapping.
const SNAP_THRESHOLD_SCREEN: f32 = 6.0;
/// Pixel grid lines only render once zoomed in enough that they're at least
/// a few screen pixels apart; below this, drawing one line per doc pixel
/// would be dense enough to look like solid fill and cost real overdraw.
const PIXEL_GRID_MIN_ZOOM: f32 = 4.0;
const PIXEL_GRID_COLOR: Color32 = Color32::from_black_alpha(40);

/// Which part of a `Path` point is under the cursor, for direct-selection
/// editing of an already-committed Path (as opposed to `Handle`, which is
/// for the generic bounding-box resize).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PathPart {
    Anchor,
    HandleIn,
    HandleOut,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum HandleSide {
    In,
    Out,
}

/// A leaf layer's rotation state captured at drag start, in page-space
/// (absolute, pan/zoom-only coordinates) — the same shape as `ResizeLayerInfo`
/// and used the same way (`original_kind` re-derived from every frame, never
/// accumulated from a previous frame's already-rotated state). Built by
/// `collect_rotatable_leaves`, which recurses into `Group`/`Artboard`
/// children instead of adding an entry for the container itself — the
/// mechanism behind "rotating a group bakes the angle into each descendant's
/// own frame" (see `model/layer.rs`'s `Frame::rotation` doc comment).
pub(crate) struct RotateLayerInfo {
    id: LayerId,
    parent_offset: Vec2,
    abs_bounds: Rect,
    original_frame: Frame,
    original_kind: LayerKind,
}

/// Recursively collects every rotatable leaf under `layer` (everything
/// except `Group`/`Artboard`, which recurse into their children instead of
/// being added themselves — their own `frame.rotation` always stays `0.0`).
/// `pub(crate)` (not just used by `CanvasWidget`'s own drag handling) since
/// `transform_ops::rotate_copies` reuses this same leaf-baking mechanism.
pub(crate) fn collect_rotatable_leaves(layer: &Layer, parent_offset: Vec2, out: &mut Vec<RotateLayerInfo>) {
    if let Some(children) = layer.kind.children() {
        let child_offset = parent_offset + layer.frame.pos.to_vec2();
        for child in children {
            collect_rotatable_leaves(child, child_offset, out);
        }
    } else {
        out.push(RotateLayerInfo {
            id: layer.id,
            parent_offset,
            abs_bounds: layer.frame.bounds().translate(parent_offset),
            original_frame: layer.frame,
            original_kind: layer.kind.clone(),
        });
    }
}

/// Applies a `delta_deg` rigid rotation about `pivot` (absolute page-space)
/// to every leaf in `layers`, writing the result into `page`. Standalone
/// (not inlined in `CanvasWidget::ui`'s `dragged()` match) so it's directly
/// unit-testable without simulating pointer events — see the `tests` module.
/// Every point (frame center, path anchors, compound-path ring points)
/// rotates about the SAME shared `pivot` — for a single selected leaf this
/// coincides with that leaf's own center, but for a multi-selection it's the
/// overall bounds center, so members orbit it together rather than spinning
/// in place. Bezier handles are relative vectors, not absolute points, so
/// they only get the rotation (no pivot-relative translation). Recomputed
/// from each `RotateLayerInfo`'s `original_kind`/`original_frame` every
/// call — never from the layer's own live state, which would already
/// reflect a previous frame's rotation and so get double-rotated (same
/// discipline `DragState::ResizingGroup`'s handler uses, for the same
/// reason). `pub(crate)` since `transform_ops::rotate_copies` reuses it too.
pub(crate) fn apply_rotation_delta(page: &mut Page, pivot: Pos2, delta_deg: f32, layers: &[RotateLayerInfo]) {
    let rotate_abs = |p: Pos2| rotate_point(p, pivot, delta_deg);
    let rotate_vec = |v: Vec2| rotate_point(v.to_pos2(), Pos2::ZERO, delta_deg).to_vec2();
    for info in layers {
        if let Some(layer) = page.find_mut(info.id) {
            let new_center = rotate_abs(info.abs_bounds.center());
            let new_pos = (new_center - info.parent_offset) - info.original_frame.size * 0.5;
            layer.frame.pos = new_pos;
            layer.frame.size = info.original_frame.size;
            layer.frame.rotation = info.original_frame.rotation + delta_deg;
            if let LayerKind::Path { points, .. } = &mut layer.kind {
                let LayerKind::Path { points: orig_points, .. } = &info.original_kind else {
                    unreachable!("original_kind matches the layer's own kind")
                };
                for (pt, orig) in points.iter_mut().zip(orig_points.iter()) {
                    let abs_anchor = info.parent_offset + info.original_frame.pos.to_vec2() + orig.anchor.to_vec2();
                    let new_abs_anchor = rotate_abs(abs_anchor.to_pos2());
                    pt.anchor = new_abs_anchor - info.parent_offset - new_pos.to_vec2();
                    pt.handle_in = orig.handle_in.map(rotate_vec);
                    pt.handle_out = orig.handle_out.map(rotate_vec);
                }
            }
            if let LayerKind::CompoundPath { polygons } = &mut layer.kind {
                let LayerKind::CompoundPath { polygons: orig_polygons } = &info.original_kind else {
                    unreachable!("original_kind matches the layer's own kind")
                };
                for (poly, orig_poly) in polygons.iter_mut().zip(orig_polygons.iter()) {
                    let rings = std::iter::once((&mut poly.exterior, &orig_poly.exterior))
                        .chain(poly.holes.iter_mut().zip(orig_poly.holes.iter()));
                    for (ring, orig_ring) in rings {
                        for (p, orig_p) in ring.iter_mut().zip(orig_ring.iter()) {
                            let abs = info.parent_offset + info.original_frame.pos.to_vec2() + orig_p.to_vec2();
                            let new_abs = rotate_abs(abs.to_pos2());
                            *p = new_abs - info.parent_offset - new_pos.to_vec2();
                        }
                    }
                }
            }
        }
    }
}

/// Recomputes `id`'s own `frame` (pos + size, `rotation` forced back to
/// `0.0`) to exactly bound its children's current geometry, shifting each
/// child's `frame.pos` by the same amount in the opposite direction so every
/// descendant's absolute position is unchanged. Recurses into any child
/// that is itself a container first (bottom-up), so a nested container's
/// frame is already correct before this level reads its `rotated_bounds()`.
///
/// Needed after `apply_rotation_delta` bakes a rotation into a selected
/// `Group`/`Artboard`/`BooleanGroup`'s descendant leaves: unlike a leaf
/// (whose `frame` *is* its own geometry), a container's `frame` is never
/// otherwise touched by a rotate, so it goes stale relative to its
/// now-rotated descendants — both the selection outline/handles (which read
/// the container's own `frame.bounds()`, see `CanvasWidget::ui`) and, for a
/// `BooleanGroup`, the render offset for its live-computed geometry (see
/// `compute_boolean_group`'s doc comment) would otherwise stay anchored to
/// the pre-rotation bounds while the actual (rotated) content moves away
/// from them. A no-op for a leaf id (no children to bound). `pub(crate)`
/// since `transform_ops::rotate_copies` reuses the same leaf-baking
/// mechanism (`collect_rotatable_leaves`/`apply_rotation_delta`) and so
/// needs the same follow-up refit for any duplicated container.
pub(crate) fn refit_container_to_children(page: &mut Page, id: LayerId) {
    let Some(layer) = page.find(id) else { return };
    let Some(child_container_ids) = layer
        .kind
        .children()
        .map(|children| children.iter().filter(|c| c.kind.children().is_some()).map(|c| c.id).collect::<Vec<_>>())
    else {
        return;
    };
    for child_id in child_container_ids {
        refit_container_to_children(page, child_id);
    }
    let Some(layer) = page.find_mut(id) else { return };
    let Some(children) = layer.kind.children() else { return };
    let Some(local_bbox) = children.iter().map(tight_rotated_bounds).reduce(|a, b| a.union(b)) else {
        return;
    };
    let delta = local_bbox.min.to_vec2();
    layer.frame.pos += delta;
    layer.frame.size = local_bbox.size();
    if let Some(children) = layer.kind.children_mut() {
        for child in children {
            child.frame.pos -= delta;
        }
    }
}

/// `layer.frame.rotated_bounds()`, except for a `LayerKind::Oval`: that
/// generic formula is the AABB of the *frame rectangle's* rotated corners,
/// which is exact for a shape that actually reaches its frame's corners
/// (Rectangle, most Paths) but a loose over-estimate for an inscribed
/// ellipse — a circle (equal width/height) doesn't even change shape when
/// rotated, yet `rotated_bounds()` still reports a bigger box the more it's
/// rotated toward 45°. Harmless for a single rotated leaf (its own outline
/// is drawn as a *tilted* rectangle exactly matching the frame, not this
/// AABB — see the "rotated bbox" handling in `CanvasWidget::ui`), but for a
/// rotated `BooleanGroup`/`Group`/`Artboard`'s children — aggregated here
/// via `children_local_bbox` and `refit_container_to_children`, both of
/// which only ever draw/store a plain axis-aligned box, never a tilted one
/// — that slack compounds into a visibly oversized, detached-looking
/// selection box. Uses the standard tight rotated-ellipse AABB formula
/// (half-extent along each axis is `hypot(semi_axis * cos, other_semi_axis
/// * sin)` of the rotation angle) instead.
fn tight_rotated_bounds(layer: &Layer) -> Rect {
    let LayerKind::Oval = layer.kind else {
        return layer.frame.rotated_bounds();
    };
    if layer.frame.rotation == 0.0 {
        return layer.frame.bounds();
    }
    let bounds = layer.frame.bounds();
    let (a, b) = (bounds.width() / 2.0, bounds.height() / 2.0);
    let theta = layer.frame.rotation.to_radians();
    let (sin, cos) = theta.sin_cos();
    let half_extent = Vec2::new((a * cos).hypot(b * sin), (a * sin).hypot(b * cos));
    Rect::from_center_size(bounds.center(), half_extent * 2.0)
}

/// The union bounding box `layer`'s children currently occupy, in `layer`'s
/// own local space (i.e. *not* yet translated by `layer.frame.pos` — same
/// convention `refit_container_to_children`'s `local_bbox` uses, and in
/// fact the same computation, just read-only). `None` for a leaf (no
/// children) or an empty container.
fn children_local_bbox(layer: &Layer) -> Option<Rect> {
    let children = layer.kind.children()?;
    children
        .iter()
        .map(|c| match children_local_bbox(c) {
            Some(bbox) => bbox.translate(c.frame.pos.to_vec2()),
            None => tight_rotated_bounds(c),
        })
        .reduce(|a, b| a.union(b))
}

/// `layer.frame.bounds()` (i.e. in its *parent's* coordinate space, same as
/// every other call site that reads it for the selection outline/handles),
/// except a `Group`/`Artboard`/`BooleanGroup` reports the *live* union of
/// its children instead of its own possibly-stale stored `frame` — a leaf
/// is unaffected (`children_local_bbox` is `None`, so this is just
/// `frame.bounds()`, preserving the separate-rotation-for-display handling
/// every existing call site already does around it).
///
/// Exists so the selection outline/handles track a container's actual
/// content on *every* frame, without depending on `refit_container_to_children`
/// having already run — which, mid-drag, it deliberately hasn't (see that
/// function's doc comment: refitting the stored `frame` every frame would
/// desync the drag's own position math). Using this instead of
/// `layer.frame.bounds()` directly for the outline means the box tracks
/// live during a rotate/resize gesture and is never one repaint behind the
/// `drag_stopped` refit, regardless of exactly when that mutation lands
/// relative to this frame's painting.
fn display_bounds(layer: &Layer) -> Rect {
    match children_local_bbox(layer) {
        Some(bbox) => bbox.translate(layer.frame.pos.to_vec2()),
        None => layer.frame.bounds(),
    }
}

/// A layer's resize state captured at drag start, in page-space (absolute,
/// pan/zoom-only coordinates — not offset by ancestor layer positions).
/// Where Scale mode's resize is anchored — see `DragState::ResizingGroup`'s
/// `scale_anchor` field and `ui/inspector.rs`'s "Scale Layers" panel.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ScaleAnchor {
    /// The dragged handle's opposite corner/edge stays fixed — identical to
    /// a plain (non-Scale-mode) resize.
    #[default]
    Corners,
    /// The selection's own center stays fixed, regardless of which handle
    /// was dragged.
    Center,
}

struct ResizeLayerInfo {
    id: LayerId,
    parent_offset: Vec2,
    abs_bounds: Rect,
    /// A clone of the layer's `kind` as it was *before* the drag touched it.
    /// `Path`/`CompoundPath` need this: unlike Rectangle/Oval (whose geometry
    /// IS `frame.bounds()`, always rederived fresh from `abs_bounds` below),
    /// their geometry lives in per-point data with no such single source of
    /// truth. Every frame must recompute from this untouched original —
    /// `transform` below always maps the *original* `start_overall_bounds`
    /// to the current live bounds, so applying it to already-transformed
    /// points from a previous frame would double-scale them.
    original_kind: LayerKind,
    /// The layer's rotation before the drag — `Frame::from_bounds` (used to
    /// rebuild `frame` from the resized bounds each frame) always zeroes
    /// `rotation`, so it must be re-applied afterward or a bounding-box
    /// resize would silently un-rotate the layer. This resize is still only
    /// a plain axis-aligned bounding-box scale (not yet a rotation-aware
    /// local-space resize — see the "rotation-aware resize" work), so a
    /// nonzero-rotation shape resized this way keeps its angle but its
    /// bounding box scales in page-space, not along its own tilted axes.
    original_rotation: f32,
    /// The stroke width before the drag — used only in Scale mode (see
    /// `CanvasWidget::scaling`) to scale it proportionally with the layer,
    /// same non-compounding "always derive from the untouched original"
    /// convention as `original_kind` above. Captured unconditionally (cheap)
    /// rather than only when scale mode is active, so this struct doesn't
    /// need two slightly different construction sites.
    original_stroke_width: Option<f32>,
}

/// Recursively collects every resizable leaf under `layer` — mirrors
/// `collect_rotatable_leaves`'s "expand a `Group`/`Artboard`/`BooleanGroup`
/// into its descendant leaves instead of adding the container itself"
/// structure, for the same reason: a container's own `frame` isn't its
/// geometry, so resizing it directly (as if it were a leaf) only moves an
/// otherwise-disconnected bounding box while its actual (unscaled) content
/// stays put. Scaling every real leaf instead, then calling
/// `refit_container_to_children` on each affected top-level container once
/// the drag has applied its transform, keeps the container's own frame (and
/// a `BooleanGroup`'s render offset) in sync with what's actually drawn.
fn collect_resizable_leaves(layer: &Layer, parent_offset: Vec2, out: &mut Vec<ResizeLayerInfo>) {
    if let Some(children) = layer.kind.children() {
        let child_offset = parent_offset + layer.frame.pos.to_vec2();
        for child in children {
            collect_resizable_leaves(child, child_offset, out);
        }
    } else {
        out.push(ResizeLayerInfo {
            id: layer.id,
            parent_offset,
            abs_bounds: layer.frame.bounds().translate(parent_offset),
            original_kind: layer.kind.clone(),
            original_rotation: layer.frame.rotation,
            original_stroke_width: layer.style.stroke.as_ref().map(|s| s.width),
        });
    }
}

/// Applies one resize-drag frame's `transform` (an axis-aligned
/// scale+translate, `scale`/`uniform_scale` its per-axis and averaged
/// factors) to every leaf in `layers`, writing the result into `page`.
/// Standalone (not inlined in `CanvasWidget::ui`'s `dragged()` match) so
/// it's directly unit-testable, same rationale as `apply_rotation_delta`.
/// Recomputed from each `ResizeLayerInfo`'s untouched `original_kind`/
/// `abs_bounds` every call, never from the layer's own live state — see
/// `ResizeLayerInfo::original_kind`'s doc comment. Does *not* touch any
/// container's own `frame`: `layers` only ever holds real leaves (see
/// `collect_resizable_leaves`), so the caller must follow up with
/// `refit_container_to_children` for every affected top-level container.
fn apply_resize_delta(
    page: &mut Page,
    layers: &[ResizeLayerInfo],
    transform: impl Fn(Pos2) -> Pos2,
    scale: Vec2,
    scale_style: bool,
    uniform_scale: f32,
) {
    for info in layers {
        if let Some(layer) = page.find_mut(info.id) {
            if scale_style {
                if let (Some(width), Some(stroke)) = (info.original_stroke_width, layer.style.stroke.as_mut()) {
                    stroke.width = (width * uniform_scale).max(0.0);
                }
                if let (LayerKind::Rectangle { corner_radius }, LayerKind::Rectangle { corner_radius: orig }) =
                    (&mut layer.kind, &info.original_kind)
                {
                    *corner_radius = orig.scaled(uniform_scale);
                }
            }
            let new_bounds =
                Rect::from_two_pos(transform(info.abs_bounds.min), transform(info.abs_bounds.max)).translate(-info.parent_offset);
            // A Path's geometry lives in `points`, stored relative to
            // `frame.pos` — unlike Rectangle/Oval, whose geometry IS
            // `frame.bounds()`, so just replacing `frame` below would move
            // the bounding box without rescaling the drawn shape inside it.
            if let LayerKind::Path { points, .. } = &mut layer.kind {
                let LayerKind::Path { points: orig_points, .. } = &info.original_kind else {
                    unreachable!("original_kind matches the layer's own kind")
                };
                for (pt, orig) in points.iter_mut().zip(orig_points.iter()) {
                    let abs_anchor = info.abs_bounds.min + orig.anchor.to_vec2();
                    pt.anchor = transform(abs_anchor) - info.parent_offset - new_bounds.min.to_vec2();
                    pt.handle_in = orig.handle_in.map(|h| Vec2::new(h.x * scale.x, h.y * scale.y));
                    pt.handle_out = orig.handle_out.map(|h| Vec2::new(h.x * scale.x, h.y * scale.y));
                }
            }
            // Same reasoning as the `Path` case above — a `CompoundPath`'s
            // geometry lives in its rings, not in `frame.bounds()`.
            if let LayerKind::CompoundPath { polygons } = &mut layer.kind {
                let LayerKind::CompoundPath { polygons: orig_polygons } = &info.original_kind else {
                    unreachable!("original_kind matches the layer's own kind")
                };
                for (poly, orig_poly) in polygons.iter_mut().zip(orig_polygons.iter()) {
                    let rings = std::iter::once((&mut poly.exterior, &orig_poly.exterior))
                        .chain(poly.holes.iter_mut().zip(orig_poly.holes.iter()));
                    for (ring, orig_ring) in rings {
                        for (p, orig_p) in ring.iter_mut().zip(orig_ring.iter()) {
                            let abs = info.abs_bounds.min + orig_p.to_vec2();
                            *p = transform(abs) - info.parent_offset - new_bounds.min.to_vec2();
                        }
                    }
                }
            }
            layer.frame = Frame::from_bounds(new_bounds);
            layer.frame.rotation = info.original_rotation;
        }
    }
}

/// All coordinates named `doc_*` below are in page-space: pan/zoom applied,
/// but NOT offset by any ancestor layer position. A layer's `frame.pos` is
/// relative to its parent, so converting between the two requires adding
/// the parent's `absolute_offset` (see `Page::absolute_offset`).
enum DragState {
    None,
    PanningView {
        start_mouse: Pos2,
        start_pan: Vec2,
    },
    CreatingShape {
        start_doc: Pos2,
    },
    /// Rubber-band selection over empty canvas. `additive` preserves
    /// `base_selection` and adds newly-enclosed layers to it on release.
    /// `contained_only` (Option held at drag start): a layer must be fully
    /// inside the marquee, not just overlapping it. `ignore_groups` (Command
    /// held): descend into `Group`/`Artboard`/`BooleanGroup` children instead
    /// of only matching top-level layers. `invert` (Command+Shift held):
    /// toggle each hit against `base_selection` (add if absent, remove if
    /// present) instead of a plain union — takes precedence over `additive`
    /// when both are set, since Command+Shift implies Shift.
    Marquee {
        start_doc: Pos2,
        additive: bool,
        base_selection: Vec<LayerId>,
        contained_only: bool,
        ignore_groups: bool,
        invert: bool,
    },
    /// Rubber-band selection of points on the active single Path, used
    /// instead of `Marquee` when the drag starts on empty space while a
    /// single Path is already selected. Mirrors `Marquee`'s
    /// additive/base_selection convention.
    MarqueePoints {
        layer_id: LayerId,
        start_doc: Pos2,
        additive: bool,
        base_selection: Vec<usize>,
    },
    MovingSelection {
        start_doc: Pos2,
        starts: Vec<(LayerId, Pos2)>,
    },
    /// Special-cased single-line endpoint drag, preserving the line's exact
    /// drag direction (which a bounds-based scale transform would lose).
    ResizingLine {
        id: LayerId,
        handle: Handle,
        parent_offset: Vec2,
    },
    /// Scales every selected layer's bounds by the same transform, anchored
    /// at the overall bounding box. Covers single non-line resize (where it's
    /// equivalent to a direct edge drag) and multi-selection group resize.
    /// `scale_style` (set from `CanvasWidget::scaling` at drag start —
    /// "Scale Layers" mode, entered with `K`): also scales each
    /// layer's stroke width and (for a `Rectangle`) corner radius by the
    /// same uniform factor, which a plain resize deliberately leaves alone.
    /// `scale_anchor` picks whether the resize is anchored at the dragged
    /// handle's opposite corner/edge (`Corners`, identical to a plain
    /// resize) or at the selection's own center (`Center`) regardless of
    /// which handle was dragged.
    ResizingGroup {
        handle: Handle,
        start_overall_bounds: Rect,
        layers: Vec<ResizeLayerInfo>,
        scale_style: bool,
        scale_anchor: ScaleAnchor,
    },
    /// Dragging a corner's outer "rotate" zone (just beyond its resize hit
    /// radius — see `HANDLE_HIT_RADIUS`/rotate-ring check in `CanvasWidget::ui`).
    /// `pivot` is the selection's overall bounds center in page-space,
    /// captured once at drag start; `layers` is built by
    /// `collect_rotatable_leaves`, so a selected `Group`/`Artboard` rotates
    /// by having the angle baked into each descendant's own frame rather
    /// than the container's (which never gets a nonzero `rotation`).
    Rotating {
        pivot: Pos2,
        start_angle: f32,
        layers: Vec<RotateLayerInfo>,
    },
    /// Dragging one of the "Smart Distribute" gap handles (see
    /// `last_distributed`/`distribution_gap_handles`): widens or narrows the
    /// gap at `gap_index` (between the `gap_index`-th and `gap_index+1`-th
    /// layer in `order`, sorted along `axis`) by shifting every layer from
    /// `gap_index + 1` onwards by the drag delta along that axis.
    /// `starts` parallels `order`, each layer's `frame.pos` at drag start.
    AdjustingDistributionGap {
        start_doc: Pos2,
        axis: DistributeAxis,
        gap_index: usize,
        order: Vec<LayerId>,
        starts: Vec<Pos2>,
    },
    /// Dragging out the bezier handle of the anchor just placed by the Pen
    /// tool (index into `CanvasWidget::pen`, the in-progress path).
    DrawingPenHandle {
        point_index: usize,
    },
    /// Direct-selection drag of one or more anchors (`CanvasWidget::selected_points`)
    /// on an already-committed `Path` layer, moved together by the same
    /// delta. `start_anchors` holds each point's original anchor (relative
    /// to `frame.pos`, same space it's stored in, parallel to
    /// `point_indices`) so the drag delta can be applied without depending
    /// on `frame.pos` staying fixed mid-gesture (it's only renormalized at
    /// `drag_stopped`).
    EditingPathAnchor {
        layer_id: LayerId,
        point_indices: Vec<usize>,
        start_doc: Pos2,
        start_anchors: Vec<Pos2>,
    },
    /// Direct-selection drag of one bezier handle on an already-committed
    /// `Path` layer. Unlike `EditingPathAnchor`, this is recomputed fresh
    /// from the live document each frame rather than from a captured start
    /// state, since a handle drag never needs `frame.pos` to stay fixed
    /// (handles don't affect the anchor bounding box `frame` tracks).
    EditingPathHandle {
        layer_id: LayerId,
        parent_offset: Vec2,
        point_index: usize,
        side: HandleSide,
    },
    /// Dragging a new guide out of a ruler strip. Not committed to
    /// `Page::guides` until `drag_stopped`, and only then if the pointer
    /// ends up over the canvas (dropping it back on a ruler cancels).
    CreatingGuide {
        orientation: GuideOrientation,
    },
    /// Dragging an already-placed guide (grabbed by its line, not a ruler).
    /// Like `CreatingGuide`, the document isn't touched per-frame; dropping
    /// back over a ruler (or off the guide's own axis of the canvas)
    /// deletes it instead of relocating it.
    MovingGuide {
        index: usize,
        orientation: GuideOrientation,
    },
    /// "Edit Image" mode's Selection/Magic Wand drag (see `ImageEditState`
    /// and `CanvasWidget::begin_image_edit`), in doc-space. Resolved at
    /// `drag_stopped` into either a rectangular selection (a real drag) or a
    /// Magic Wand flood fill from `start_doc` (a near-zero-movement
    /// release) — the same click-vs-drag threshold `CreatingShape` uses.
    /// `base_mask` is the mask this drag's result gets OR'd/AND-NOT'd onto:
    /// already seeded to all-`false` at drag start if this is a plain
    /// (non-modified) click/drag, so a fresh selection replaces the old one.
    ImageEditDrag {
        start_doc: Pos2,
        subtract: bool,
        base_mask: Vec<bool>,
    },
}

/// Minimum on-screen drag distance (in screen pixels) before a Pen-tool
/// anchor is considered "dragged" rather than a plain click, and thus gets
/// bezier handles instead of staying a straight corner.
const PEN_HANDLE_MIN_DRAG: f32 = 2.0;

pub struct CanvasWidget {
    pub pan: Vec2,
    pub zoom: f32,
    pub show_rulers: bool,
    pub show_grid: bool,
    pub snap_enabled: bool,
    drag: DragState,
    /// Anchors placed so far by an in-progress Pen-tool path, in doc-space
    /// (absolute page coordinates, not yet relativized to a layer frame).
    /// `None` when no path is being drawn.
    pen: Option<Vec<PathPoint>>,
    /// Indices into the currently-active single `Path`'s `points`, for
    /// direct-selection multi-point editing (shift-click / marquee / Cmd+A,
    /// group drag, group delete, point-type shortcuts). Scoped to whichever
    /// `Path` layer `point_edit_layer` names; cleared whenever the active
    /// single-Path selection changes, the tool leaves `Select`, or Escape is
    /// pressed.
    selected_points: Vec<usize>,
    /// The `Path` layer `selected_points` currently applies to, so a change
    /// in which layer is singly selected (or a switch away from a
    /// single-Path selection) can be detected and the stale point selection
    /// dropped.
    point_edit_layer: Option<LayerId>,
    image_cache: ImageTextureCache,
    mask_cache: MaskedGroupTextureCache,
    noise_cache: NoiseTextureCache,
    halftone_cache: HalftoneTextureCache,
    pattern_cache: PatternTextureCache,
    shadow_cache: ShadowTextureCache,
    image_edit: Option<ImageEditState>,
    /// The `Text` layer currently being edited in place on canvas (see
    /// `CanvasWidget::ui`'s post-tree-draw overlay block), if any.
    editing_text: Option<LayerId>,
    /// Set alongside `editing_text` on entry; consumed (and cleared) the
    /// first frame the overlay is shown, to request focus + select-all
    /// exactly once rather than every frame.
    editing_text_just_started: bool,
    /// `(layer, family, retries left)` for a layer whose `TextFont::System`
    /// pick just ran `apply_text_auto_resize` before egui had actually
    /// bound that family (`system_fonts::resolve_family` can't return it
    /// until the *next* pass — see `CLAUDE.md`'s "Fonts" section) — so the
    /// resulting `frame.size` was measured against the `Proportional`
    /// fallback instead of the real font. Re-checked once per frame in
    /// `ui()`; re-measures and drops the entry once the family is bound,
    /// or after a few frames if it never resolves (not installed).
    pending_font_resize: Vec<(LayerId, String, u8)>,
    /// The in-place editor's current text selection, as a char range into
    /// the edited layer's `content` — `None` while not editing, or while
    /// editing with the cursor collapsed (nothing selected). Populated
    /// each frame from the `TextEdit`'s own `CCursorRange` after `.show()`
    /// (see the `editing_text` block in `ui()`); `ui/inspector.rs` reads
    /// this to decide whether a format button should apply to just the
    /// selection (rich range edit) or the whole layer (today's behavior).
    pub text_edit_selection: Option<std::ops::Range<usize>>,
    /// The "Offset duplicated layers" setting (Cmd+D / Edit > Duplicate),
    /// editable via `app.rs`'s Edit > Settings > Layers submenu. In-memory
    /// only (not persisted with the document — a UI preference, not document
    /// state, same category as `show_rulers`/`snap_enabled` above).
    pub duplicate_offset: Vec2,
    /// The behavior where "a layer deleted from inside a group, then a new layer
    /// drawn, lands back in that same group" — set by `App`'s Delete/
    /// Backspace handler, consumed (taken) the next time a shape/text layer
    /// is committed (see `insert_layer`'s `hint` param). One-shot: cleared
    /// on use, not on some later unrelated selection change.
    pub insert_hint_parent: Option<LayerId>,
    /// A "reference layer" for alignment: click an already-fully-
    /// selected single layer again to mark it (drawn with a thicker outline
    /// — see the selection-outline pass), then Align targets its bounds
    /// instead of the selection's own combined bbox (see
    /// `App::align_selection`). Stale/no-longer-selected values are simply
    /// ignored by that filter rather than actively cleared here.
    pub reference_layer: Option<LayerId>,
    /// Spacing used by the inspector's "Tidy" button (see
    /// `alignment::tidy`) — session UI state, not persisted with the
    /// document, same category as `duplicate_offset` above.
    pub tidy_spacing: f32,
    /// The most recent Distribute's ids + axis (see `App::distribute_selection`),
    /// so `ui()` can show the "Smart Distribute" gap-adjustment handles
    /// as long as the exact same set is still selected. Reordering-by-drag
    /// (the other Smart Distribute gesture, dragging a layer's own
    /// center handle past a neighbor to swap positions) is deliberately not
    /// implemented — gap-width adjustment alone covers the common case.
    pub last_distributed: Option<(Vec<LayerId>, DistributeAxis)>,
    /// The Option+Cmd+L "proportional lock" toggle for the inspector's
    /// Size fields — session UI state, not persisted with the document. See
    /// `ui/inspector.rs`'s Position/Size fields for where this is read.
    pub aspect_locked: bool,
    /// Scale mode (`K`, with a non-empty selection): while `Some`,
    /// dragging a resize handle scales stroke width/corner radius along
    /// with the layer's bounds (see `DragState::ResizingGroup`'s
    /// `scale_style`). The held selection is the set Scale mode started
    /// with, so `app.rs`'s Enter/Finish-button exit can restore it if the
    /// user changed `selection` mid-mode without ever dragging a handle.
    /// Also exits when the user clicks empty canvas (the third exit
    /// path) — see the `None` branch of `hit_test` in `drag_started`.
    pub scaling: Option<Vec<LayerId>>,
    /// See `ScaleAnchor`; editable from the "Scale Layers" inspector panel
    /// shown while `scaling` is `Some`.
    pub scale_anchor: ScaleAnchor,
    /// The Shift+right-click "pick a layer among overlapping ones" menu:
    /// the doc-space click point and every layer under it (bounding-box
    /// based, front-to-back), shown as a small popup until a choice is made
    /// or it's dismissed. `None` when not showing.
    overlap_menu: Option<(Pos2, Vec<LayerId>)>,
    /// Plain (non-Shift) right-click's Copy/Paste/Paste Over menu — screen
    /// position, `None` when not showing. Kept separate from `overlap_menu`
    /// so the two never show at once (see the Shift check where both are
    /// opened).
    canvas_menu: Option<Pos2>,
    /// Right-click-on-an-anchor's point-type picker (`apply_point_type`'s
    /// Straight/Mirror/Asymmetric/Disconnected, mouse alternative to the
    /// Num1-4 shortcuts) — screen position and the anchor's owning `Path`
    /// layer, `None` when not showing. Takes priority over `canvas_menu`
    /// when the right-click landed on an anchor (see the `secondary_clicked`
    /// dispatch), so the two never show at once either.
    point_type_menu: Option<(Pos2, LayerId)>,
}

impl Default for CanvasWidget {
    fn default() -> Self {
        Self {
            pan: Vec2::ZERO,
            zoom: 1.0,
            show_rulers: true,
            show_grid: true,
            snap_enabled: true,
            drag: DragState::None,
            pen: None,
            selected_points: Vec::new(),
            point_edit_layer: None,
            image_cache: ImageTextureCache::default(),
            mask_cache: MaskedGroupTextureCache::default(),
            noise_cache: NoiseTextureCache::default(),
            halftone_cache: HalftoneTextureCache::default(),
            pattern_cache: PatternTextureCache::default(),
            shadow_cache: ShadowTextureCache::default(),
            image_edit: None,
            editing_text: None,
            editing_text_just_started: false,
            pending_font_resize: Vec::new(),
            text_edit_selection: None,
            duplicate_offset: crate::grouping::DEFAULT_DUPLICATE_OFFSET,
            insert_hint_parent: None,
            reference_layer: None,
            tidy_spacing: 20.0,
            last_distributed: None,
            aspect_locked: false,
            scaling: None,
            scale_anchor: ScaleAnchor::default(),
            overlap_menu: None,
            canvas_menu: None,
            point_type_menu: None,
        }
    }
}

impl CanvasWidget {
    /// Whether direct-selection point editing currently has one or more
    /// points selected. `app.rs`'s Delete/Backspace shortcut checks this to
    /// decide whether the key should delete the selected points instead of
    /// the selected layer.
    pub fn has_point_selection(&self) -> bool {
        !self.selected_points.is_empty()
    }

    /// Whether "Edit Image" mode is currently active for `id` — the
    /// inspector uses this to decide whether to show the editing controls
    /// (tolerance slider, Crop/Fill/Delete) or the "Edit Image" entry button.
    pub fn image_edit_active_for(&self, id: LayerId) -> bool {
        self.image_edit.as_ref().map(|s| s.layer_id) == Some(id)
    }

    /// Called by `ui/inspector.rs` right after picking a `TextFont::System`
    /// whose family egui hadn't bound yet at that moment, so `id`'s
    /// auto-resize just ran against the `Proportional` fallback instead of
    /// the real font's metrics. `ui()` retries the measurement once the
    /// family becomes available (see `pending_font_resize`'s doc comment).
    pub fn queue_font_resize_retry(&mut self, id: LayerId, family: String) {
        self.pending_font_resize.push((id, family, 5));
    }

    /// Starts in-place editing of `id` (a `Text` layer) via the floating
    /// canvas overlay (see `ui`'s post-tree-draw block) — entered by
    /// double-clicking a `Text` layer, or by `app.rs`'s Enter/Return
    /// shortcut on a single selected `Text` layer. Snapshots history once on
    /// entry, matching the app's "snapshot at gesture start" convention
    /// (`ui/inspector.rs`'s `should_snapshot`) — the keystrokes that follow
    /// don't add further snapshots. Re-targeting to a different layer while
    /// already editing (double-clicking another text layer) is just another
    /// call to this method.
    pub fn start_editing_text(&mut self, history: &mut History, id: LayerId) {
        if self.editing_text == Some(id) {
            return;
        }
        history.snapshot();
        self.editing_text = Some(id);
        self.editing_text_just_started = true;
    }

    /// Whether `id` is currently being edited in place on canvas.
    pub fn is_editing_text(&self, id: LayerId) -> bool {
        self.editing_text == Some(id)
    }

    /// "Type directly inside a shape": wraps `shape_id` and a
    /// brand-new, empty, center-aligned `Text` layer sized to match its
    /// frame into an ordinary `Group` — exactly what manually drawing a
    /// `Text` layer on top and pressing Cmd+G would produce, since this
    /// codebase has no single merged shape+text layer type either. Reusing
    /// `Group` this way means every other operation (move, resize, delete,
    /// duplicate, undo, export, layers panel) already handles the pairing
    /// correctly with no new code. Called from the `Tool::Select`
    /// double-click handler in `ui()`, which only routes here for a bare
    /// `Rectangle`/`Oval` hit — a double-click that lands on an
    /// already-labeled shape instead hits the label `Text` layer directly
    /// (same bounding box, drawn on top), so re-entering an existing label
    /// needs no separate branch.
    ///
    /// Snapshots history once for the whole gesture (matching
    /// `start_editing_text`'s "snapshot at gesture start" convention) and
    /// starts editing the new label immediately. Returns its id, or `None`
    /// if `shape_id` isn't found or isn't safe to wrap: a mask (its clipping
    /// depends on staying a direct sibling of what it masks — see
    /// `Layer::is_mask`) or a `BooleanGroup` child (its combine-op geometry
    /// depends on staying a direct child of the group).
    pub fn add_shape_label(&mut self, history: &mut History, shape_id: LayerId) -> Option<LayerId> {
        let page = history.get().active_page();
        let shape = page.find(shape_id)?;
        if shape.is_mask {
            return None;
        }
        if let Some((Some(parent_id), _)) = grouping::parent_and_index(&page.layers, shape_id) {
            if matches!(page.find(parent_id).map(|l| &l.kind), Some(LayerKind::BooleanGroup { .. })) {
                return None;
            }
        }
        let frame = shape.frame;

        history.snapshot();
        let page = history.mutate().active_page_mut();
        let siblings = grouping::find_common_parent_list(&mut page.layers, &[shape_id])?;
        let index = siblings.iter().position(|l| l.id == shape_id)?;
        let mut label = Layer::new(
            "Text",
            frame,
            LayerKind::Text {
                content: String::new(),
                font_size: 24.0,
                font: TextFont::Proportional,
                align: TextAlign::Center,
                vertical_align: VerticalAlign::Middle,
                resize: TextResize::Fixed,
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
            },
        );
        label.style = Style { fill: Some(Paint::Solid(Color32::BLACK)), stroke: None, ..Default::default() };
        let text_id = label.id;
        siblings.insert(index + 1, label);
        grouping::group_layers(page, &[shape_id, text_id]);

        self.editing_text = Some(text_id);
        self.editing_text_just_started = true;
        Some(text_id)
    }

    /// Enters "Edit Image" mode for `id`, starting from an empty selection.
    /// No-ops if `id` isn't (currently) an `Image` layer.
    pub fn begin_image_edit(&mut self, history: &History, id: LayerId) {
        let Some(layer) = history.get().find(id) else { return };
        let LayerKind::Image { width, height, .. } = layer.kind else { return };
        self.image_edit = Some(ImageEditState {
            layer_id: id,
            tolerance: 24.0,
            mask: vec![false; (width as usize) * (height as usize)],
            width,
            height,
            fill_color: Color32::WHITE,
            overlay: None,
            overlay_stale: false,
        });
        self.drag = DragState::None;
    }

    pub fn end_image_edit(&mut self) {
        self.image_edit = None;
    }

    pub fn image_edit_tolerance(&self) -> f32 {
        self.image_edit.as_ref().map(|s| s.tolerance).unwrap_or(24.0)
    }

    pub fn set_image_edit_tolerance(&mut self, tolerance: f32) {
        if let Some(edit) = &mut self.image_edit {
            edit.tolerance = tolerance;
        }
    }

    pub fn image_edit_has_selection(&self) -> bool {
        self.image_edit.as_ref().map(|s| s.has_selection()).unwrap_or(false)
    }

    pub fn image_edit_fill_color(&self) -> Color32 {
        self.image_edit.as_ref().map(|s| s.fill_color).unwrap_or(Color32::WHITE)
    }

    pub fn set_image_edit_fill_color(&mut self, color: Color32) {
        if let Some(edit) = &mut self.image_edit {
            edit.fill_color = color;
        }
    }

    pub fn clear_image_edit_selection(&mut self) {
        if let Some(edit) = &mut self.image_edit {
            edit.mask.iter_mut().for_each(|m| *m = false);
            edit.mark_dirty();
        }
    }

    /// Crops the edited image to its selection's bounding box (Crop is
    /// always rectangular, even after an irregular Magic Wand selection),
    /// rescaling/repositioning the frame so the crop's on-screen size stays
    /// consistent with how the un-cropped image was displayed. Always ends
    /// edit mode: cropping changes the image's pixel dimensions, which the
    /// captured mask/`width`/`height` no longer match.
    pub fn apply_image_edit_crop(&mut self, history: &mut History) {
        let Some(edit) = &self.image_edit else { return };
        let Some((x0, y0, x1, y1)) = mask_bbox(&edit.mask, edit.width) else {
            self.end_image_edit();
            return;
        };
        let layer_id = edit.layer_id;
        let (w, h) = (x1 - x0 + 1, y1 - y0 + 1);
        history.snapshot();
        if let Some(layer) = history.mutate().active_page_mut().find_mut(layer_id) {
            if let LayerKind::Image { encoded, .. } = &layer.kind {
                if let Some(decoded) = crate::image_ops::decode(encoded) {
                    let cropped = crate::image_ops::crop_to_rect(&decoded, x0, y0, w, h);
                    crate::image_ops::apply_cropped_image(layer, &cropped, x0, y0);
                }
            }
        }
        self.end_image_edit();
    }

    /// Paints every selected pixel solid `color`, opaque — an
    /// image-editing "Fill". Clears the selection afterward but stays in
    /// edit mode, so Fill can be applied a few times in a row.
    pub fn apply_image_edit_fill(&mut self, history: &mut History, color: Color32) {
        let Some(edit) = &self.image_edit else { return };
        if !edit.has_selection() {
            return;
        }
        let (layer_id, mask) = (edit.layer_id, edit.mask.clone());
        history.snapshot();
        if let Some(layer) = history.mutate().active_page_mut().find_mut(layer_id) {
            if let LayerKind::Image { encoded, version, .. } = &mut layer.kind {
                if let Some(mut decoded) = crate::image_ops::decode(encoded) {
                    crate::image_ops::fill_mask(&mut decoded, &mask, [color.r(), color.g(), color.b()]);
                    *encoded = crate::image_ops::encode_png(&decoded);
                    *version = LayerId::new_v4();
                }
            }
        }
        self.clear_image_edit_selection();
    }

    /// Clears every selected pixel to fully transparent — a manual
    /// hand-selected alternative/complement to the automatic Remove
    /// Background. Stays in edit mode, selection cleared afterward.
    pub fn apply_image_edit_delete(&mut self, history: &mut History) {
        let Some(edit) = &self.image_edit else { return };
        if !edit.has_selection() {
            return;
        }
        let (layer_id, mask) = (edit.layer_id, edit.mask.clone());
        history.snapshot();
        if let Some(layer) = history.mutate().active_page_mut().find_mut(layer_id) {
            if let LayerKind::Image { encoded, version, .. } = &mut layer.kind {
                if let Some(mut decoded) = crate::image_ops::decode(encoded) {
                    crate::image_ops::clear_mask(&mut decoded, &mask);
                    *encoded = crate::image_ops::encode_png(&decoded);
                    *version = LayerId::new_v4();
                }
            }
        }
        self.clear_image_edit_selection();
    }

    /// Doc-space bounds (already offset by any ancestor Group/Artboard) of
    /// the layer currently being image-edited, plus its pixel dimensions —
    /// used to map mouse positions into the mask's pixel space.
    fn image_edit_doc_bounds(&self, history: &History) -> Option<(LayerId, Rect, u32, u32)> {
        let edit = self.image_edit.as_ref()?;
        let page = history.get().active_page();
        let layer = page.find(edit.layer_id)?;
        let offset = page.absolute_offset(edit.layer_id)?;
        Some((edit.layer_id, layer.frame.bounds().translate(offset), edit.width, edit.height))
    }

    fn to_screen(&self, origin: Pos2, p: Pos2) -> Pos2 {
        origin + self.pan + p.to_vec2() * self.zoom
    }

    fn to_doc(&self, origin: Pos2, p: Pos2) -> Pos2 {
        ((p - origin - self.pan) / self.zoom).to_pos2()
    }

    /// Index of the guide whose line passes within `GUIDE_HIT_SCREEN` of
    /// `mouse`, if any. `mouse` must already be known to be within the
    /// canvas area (rulers are handled separately, as guide-creation).
    fn hovered_guide_index(&self, origin: Pos2, guides: &[Guide], mouse: Pos2) -> Option<usize> {
        guides.iter().position(|g| match g.orientation {
            GuideOrientation::Horizontal => {
                let sy = self.to_screen(origin, Pos2::new(0.0, g.pos)).y;
                (mouse.y - sy).abs() <= GUIDE_HIT_SCREEN
            }
            GuideOrientation::Vertical => {
                let sx = self.to_screen(origin, Pos2::new(g.pos, 0.0)).x;
                (mouse.x - sx).abs() <= GUIDE_HIT_SCREEN
            }
        })
    }

    /// If a Pen path is in progress and `doc_pos` lands on top of its first
    /// anchor (within handle-hit distance, in screen space), finishes it as
    /// a closed path and returns true. Otherwise leaves state untouched.
    fn try_close_pen_path(
        &mut self,
        doc_pos: Pos2,
        origin: Pos2,
        history: &mut History,
        selection: &mut Vec<LayerId>,
        tool: &mut Tool,
    ) -> bool {
        let Some(points) = &self.pen else { return false };
        if points.len() < 2 {
            return false;
        }
        let first = points[0].anchor;
        if self.to_screen(origin, first).distance(self.to_screen(origin, doc_pos)) > HANDLE_HIT_RADIUS {
            return false;
        }
        self.finish_pen_path(history, selection, tool, true);
        true
    }

    /// Commits the in-progress Pen path (if any, and if it has at least 2
    /// points) as a new `Path` layer, relativizing anchors to the layer's
    /// own frame the same way a `Group`/`Artboard` offsets its children.
    fn finish_pen_path(
        &mut self,
        history: &mut History,
        selection: &mut Vec<LayerId>,
        tool: &mut Tool,
        closed: bool,
    ) {
        let Some(points) = self.pen.take() else { return };
        self.drag = DragState::None;
        if points.len() < 2 {
            return;
        }
        let bounds = points
            .iter()
            .map(|p| Rect::from_pos(p.anchor))
            .reduce(|a, b| a.union(b))
            .unwrap();
        let frame_pos = bounds.min;
        let relative_points: Vec<PathPoint> = points
            .iter()
            .map(|p| PathPoint {
                anchor: p.anchor - frame_pos.to_vec2(),
                handle_in: p.handle_in,
                handle_out: p.handle_out,
                point_type: p.point_type,
                corner_radius: p.corner_radius,
            })
            .collect();
        let frame = Frame {
            pos: frame_pos,
            size: bounds.size(),
            rotation: 0.0,
        };
        let layer = Layer::new("Path", frame, LayerKind::Path { points: relative_points, closed });
        let new_id = layer.id;
        history.snapshot();
        insert_layer(history.mutate().active_page_mut(), layer, frame_pos, None);
        *selection = vec![new_id];
        if *tool == Tool::Pen {
            *tool = Tool::Select;
        }
    }

    /// Closest anchor of the single selected `Path` to `screen_pos`, if any
    /// is within `HANDLE_HIT_RADIUS` — used by the right-click point-type
    /// menu, which (unlike the drag/hover machinery further down) needs a
    /// one-shot hit test at an arbitrary click position rather than a value
    /// already computed against the live hover position.
    fn hit_test_path_anchor(
        &self,
        history: &History,
        selection: &[LayerId],
        origin: Pos2,
        screen_pos: Pos2,
    ) -> Option<(LayerId, usize)> {
        let [layer_id] = selection else { return None };
        let layer_id = *layer_id;
        let (frame_pos, offset, points, _closed) = read_path(history, layer_id)?;
        let mut best: Option<(usize, f32)> = None;
        for (i, p) in points.iter().enumerate() {
            let anchor_screen = self.to_screen(origin, p.anchor + frame_pos.to_vec2() + offset);
            let d = screen_pos.distance(anchor_screen);
            if best.map(|(_, bd)| d < bd).unwrap_or(true) {
                best = Some((i, d));
            }
        }
        let (index, dist) = best?;
        (dist <= HANDLE_HIT_RADIUS).then_some((layer_id, index))
    }

    /// Double-click a straight anchor to convert it to a curved (`Mirror`)
    /// point with a bisector-derived default handle pair, matching a common
    /// "double-click a corner to make it a curve" gesture — the caller
    /// (the double-click dispatch in `ui()`) checks this *before* falling
    /// back to `try_insert_path_point`'s segment-insert, so double-clicking
    /// directly on an anchor curves it instead of inserting a redundant
    /// duplicate point there. Returns `false` (does nothing) if no anchor is
    /// close enough, or the anchor already has a handle on either side —
    /// this only ever turns a straight corner into a curve, it's not a
    /// toggle back to straight on a second double-click.
    fn try_convert_anchor_to_curve(
        &mut self,
        history: &mut History,
        selection: &[LayerId],
        doc_pos: Pos2,
        origin: Pos2,
    ) -> bool {
        let [layer_id] = selection else { return false };
        let layer_id = *layer_id;
        let Some((frame_pos, offset, points, closed)) = read_path(history, layer_id) else {
            return false;
        };
        let click_screen = self.to_screen(origin, doc_pos);
        let mut best: Option<(usize, f32)> = None;
        for (i, p) in points.iter().enumerate() {
            let screen = self.to_screen(origin, p.anchor + frame_pos.to_vec2() + offset);
            let d = click_screen.distance(screen);
            if best.map(|(_, bd)| d < bd).unwrap_or(true) {
                best = Some((i, d));
            }
        }
        let Some((index, dist)) = best else { return false };
        if dist > HANDLE_HIT_RADIUS {
            return false;
        }
        if points[index].handle_in.is_some() || points[index].handle_out.is_some() {
            return false;
        }
        let Some(handle_out) = default_curve_handle_out(&points, index, closed) else {
            return false;
        };
        history.snapshot();
        if let Some(layer) = history.mutate().active_page_mut().find_mut(layer_id) {
            if let LayerKind::Path { points, .. } = &mut layer.kind {
                if let Some(pt) = points.get_mut(index) {
                    pt.point_type = PointType::Mirror;
                    pt.handle_out = Some(handle_out);
                    pt.handle_in = Some(-handle_out);
                }
            }
        }
        true
    }

    /// Double-click-to-insert-anchor on a single selected `Path`: finds the
    /// segment whose chord is closest to `doc_pos` and splits it with a new
    /// plain corner point there. No-ops for anything else selected, or if
    /// the click isn't close enough to any segment.
    fn try_insert_path_point(
        &mut self,
        history: &mut History,
        selection: &[LayerId],
        doc_pos: Pos2,
        origin: Pos2,
    ) {
        let [layer_id] = selection else { return };
        let layer_id = *layer_id;
        let Some((frame_pos, offset, points, closed)) = read_path(history, layer_id) else {
            return;
        };
        let n = points.len();
        let last = if closed { n } else { n.saturating_sub(1) };
        if last == 0 {
            return;
        }
        let click_screen = self.to_screen(origin, doc_pos);
        let mut best: Option<(usize, f32)> = None;
        for i in 0..last {
            let a = self.to_screen(origin, points[i].anchor + frame_pos.to_vec2() + offset);
            let b = self.to_screen(origin, points[(i + 1) % n].anchor + frame_pos.to_vec2() + offset);
            let d = distance_to_segment(click_screen, a, b);
            if best.map(|(_, bd)| d < bd).unwrap_or(true) {
                best = Some((i, d));
            }
        }
        let Some((seg_index, dist)) = best else { return };
        if dist > HANDLE_HIT_RADIUS * 2.0 {
            return;
        }
        let local_anchor = doc_pos - frame_pos.to_vec2() - offset;
        history.snapshot();
        if let Some(layer) = history.mutate().active_page_mut().find_mut(layer_id) {
            if let LayerKind::Path { points, .. } = &mut layer.kind {
                points.insert(
                    seg_index + 1,
                    PathPoint {
                        anchor: local_anchor,
                        handle_in: None,
                        handle_out: None,
                        point_type: PointType::Disconnected,
                        corner_radius: 0.0,
                    },
                );
            }
        }
    }

    /// Alt/Option-click-to-delete-anchor on a single selected `Path`.
    /// Refuses to drop below 2 points (a path needs at least that many to
    /// mean anything).
    fn try_delete_path_point(
        &mut self,
        history: &mut History,
        selection: &[LayerId],
        doc_pos: Pos2,
        origin: Pos2,
    ) {
        let [layer_id] = selection else { return };
        let layer_id = *layer_id;
        let Some((frame_pos, offset, points, _closed)) = read_path(history, layer_id) else {
            return;
        };
        if points.len() <= 2 {
            return;
        }
        let click_screen = self.to_screen(origin, doc_pos);
        let hit = points.iter().position(|p| {
            let abs = p.anchor + frame_pos.to_vec2() + offset;
            self.to_screen(origin, abs).distance(click_screen) <= HANDLE_HIT_RADIUS
        });
        let Some(index) = hit else { return };
        history.snapshot();
        if let Some(layer) = history.mutate().active_page_mut().find_mut(layer_id) {
            if let LayerKind::Path { points, .. } = &mut layer.kind {
                points.remove(index);
            }
        }
        normalize_path_frame(history.mutate().active_page_mut(), layer_id);
    }

    /// Deletes every point in `selected_points` from the active single Path
    /// (`point_edit_layer`), used by the Delete/Backspace shortcut when a
    /// point selection is active (see `has_point_selection`). Unlike
    /// `try_delete_path_point`'s single-point "refuse if it would drop below
    /// 2 points" guard, a batch delete that would leave fewer than 2 points
    /// removes the whole layer instead of silently no-opping — matching
    /// common vector editors (deleting a path down to nothing deletes the path)
    /// and avoiding a Delete keypress that visibly does nothing, which is
    /// especially easy to trigger on a simple 2-point path (e.g. a
    /// Pen-drawn straight line) since clicking anywhere near either end
    /// selects that anchor rather than the layer.
    pub fn delete_selected_points(&mut self, history: &mut History, selection: &mut Vec<LayerId>) {
        let Some(layer_id) = self.point_edit_layer else { return };
        if self.selected_points.is_empty() {
            return;
        }
        let Some((_, _, points, _)) = read_path(history, layer_id) else {
            return;
        };
        let mut indices = self.selected_points.clone();
        indices.sort_unstable();
        indices.dedup();
        if points.len() < indices.len() + 2 {
            history.snapshot();
            history.mutate().active_page_mut().remove(layer_id);
            selection.retain(|&s| s != layer_id);
            self.selected_points.clear();
            self.point_edit_layer = None;
            return;
        }
        history.snapshot();
        if let Some(layer) = history.mutate().active_page_mut().find_mut(layer_id) {
            if let LayerKind::Path { points, .. } = &mut layer.kind {
                for &index in indices.iter().rev() {
                    points.remove(index);
                }
            }
        }
        normalize_path_frame(history.mutate().active_page_mut(), layer_id);
        self.selected_points.clear();
    }

    /// Applies a `PointType` change (the Num1-4 shortcuts) to every point in
    /// `selected_points` on `layer_id`, normalizing handle geometry so the
    /// change is visually immediate: `Straight` clears both handles,
    /// `Mirror` mirrors whichever handle is already set onto the other side
    /// (preferring `handle_out` if both happen to be set), or — if the point
    /// has no handle at all yet — generates the same chord-tangent default
    /// double-clicking an anchor would (`default_curve_handle_out`), so the
    /// shortcut always visibly curves a plain corner instead of silently
    /// no-oping on one. `Asymmetric` and `Disconnected` only change future
    /// handle-drag behavior (see `DragState::EditingPathHandle` handling),
    /// so no geometry changes on switch.
    fn apply_point_type(&mut self, history: &mut History, layer_id: LayerId, point_type: PointType) {
        history.snapshot();
        if let Some(layer) = history.mutate().active_page_mut().find_mut(layer_id) {
            if let LayerKind::Path { points, closed } = &mut layer.kind {
                let closed = *closed;
                for &index in &self.selected_points {
                    let existing = points.get(index).and_then(|pt| pt.handle_out.or(pt.handle_in.map(|h| -h)));
                    let default_mirror_handle = if point_type == PointType::Mirror && existing.is_none() {
                        default_curve_handle_out(points, index, closed)
                    } else {
                        None
                    };
                    let Some(pt) = points.get_mut(index) else { continue };
                    pt.point_type = point_type;
                    match point_type {
                        PointType::Straight => {
                            pt.handle_in = None;
                            pt.handle_out = None;
                        }
                        PointType::Mirror => {
                            if let Some(h) = existing.or(default_mirror_handle) {
                                pt.handle_out = Some(h);
                                pt.handle_in = Some(-h);
                            }
                        }
                        PointType::Asymmetric | PointType::Disconnected => {}
                    }
                }
            }
        }
    }

    /// The first selected point's `corner_radius` on `layer_id`, for the
    /// inspector's radius field — `None` if nothing (or no `Path`) is
    /// selected. Multiple selected points with differing radii just show
    /// the first one's value (this codebase has no "Mixed" convention for
    /// per-point fields yet, unlike text runs' `mixed_or`).
    pub fn selected_point_corner_radius(&self, history: &History, layer_id: LayerId) -> Option<f32> {
        let &index = self.selected_points.first()?;
        let layer = history.get().find(layer_id)?;
        let LayerKind::Path { points, .. } = &layer.kind else { return None };
        points.get(index).map(|p| p.corner_radius)
    }

    /// Sets every point in `selected_points` on `layer_id` to `radius`, for
    /// the inspector's radius `DragValue`. Unlike `apply_point_type`/
    /// `apply_max_corner_radius` (each a discrete one-shot action), this is
    /// driven by a continuously-dragged `DragValue` — so, matching
    /// `ui/inspector.rs`'s `should_snapshot` convention used everywhere else
    /// a `DragValue` writes through, snapshotting is the *caller's*
    /// responsibility (once per gesture), not done here on every call.
    pub fn apply_corner_radius(&mut self, history: &mut History, layer_id: LayerId, radius: f32) {
        if let Some(layer) = history.mutate().active_page_mut().find_mut(layer_id) {
            if let LayerKind::Path { points, .. } = &mut layer.kind {
                for &index in &self.selected_points {
                    if let Some(pt) = points.get_mut(index) {
                        pt.corner_radius = radius;
                    }
                }
            }
        }
    }

    /// Sets every point in `selected_points` on `layer_id` to *its own*
    /// geometric maximum radius (half its shorter adjacent segment — the
    /// same clamp `shapes::rounded_corner_arc_points` applies at render
    /// time), for the inspector's "Max" button. A one-shot snap to the
    /// current geometry's max, not a persisted "always track max" mode —
    /// editing the frame/points afterward doesn't keep this in sync.
    pub fn apply_max_corner_radius(&mut self, history: &mut History, layer_id: LayerId) {
        history.snapshot();
        if let Some(layer) = history.mutate().active_page_mut().find_mut(layer_id) {
            if let LayerKind::Path { points, closed } = &mut layer.kind {
                let closed = *closed;
                let n = points.len();
                for &index in &self.selected_points {
                    if index >= n {
                        continue;
                    }
                    let prev = if index == 0 { closed.then(|| n - 1) } else { Some(index - 1) };
                    let next = if index == n - 1 { closed.then_some(0) } else { Some(index + 1) };
                    let (Some(prev), Some(next)) = (prev, next) else { continue };
                    let anchor = points[index].anchor;
                    let len_prev = (points[prev].anchor - anchor).length();
                    let len_next = (points[next].anchor - anchor).length();
                    points[index].corner_radius = len_prev.min(len_next) * 0.5;
                }
            }
        }
    }

    /// Scissors-tool click on a single selected `Path`: cuts it at the
    /// clicked anchor or segment. Cutting a closed path opens it there
    /// (duplicating the cut point into two coincident endpoints so they can
    /// later be dragged apart); cutting an open path at an interior anchor
    /// or segment splits it into two separate `Path` layers. No-ops for
    /// anything else selected, an already-open endpoint, or a click too far
    /// from any anchor/segment.
    fn try_scissor_path(
        &mut self,
        history: &mut History,
        selection: &mut Vec<LayerId>,
        doc_pos: Pos2,
        origin: Pos2,
    ) {
        let [layer_id] = selection.as_slice() else { return };
        let layer_id = *layer_id;
        let Some((frame_pos, offset, points, closed)) = read_path(history, layer_id) else {
            return;
        };
        let n = points.len();
        if n < 2 {
            return;
        }
        let base = frame_pos.to_vec2() + offset;
        let click_screen = self.to_screen(origin, doc_pos);

        let anchor_hit = points
            .iter()
            .position(|p| self.to_screen(origin, p.anchor + base).distance(click_screen) <= HANDLE_HIT_RADIUS);

        if let Some(i) = anchor_hit {
            if closed {
                let mut opened: Vec<PathPoint> = points[i..].to_vec();
                opened.extend_from_slice(&points[..i]);
                opened.push(points[i]);
                history.snapshot();
                if let Some(layer) = history.mutate().active_page_mut().find_mut(layer_id) {
                    if let LayerKind::Path { points, closed } = &mut layer.kind {
                        *points = opened;
                        *closed = false;
                    }
                }
                normalize_path_frame(history.mutate().active_page_mut(), layer_id);
            } else if i > 0 && i < n - 1 {
                let first = points[..=i].to_vec();
                let second = points[i..].to_vec();
                self.replace_with_split_paths(history, selection, layer_id, first, second);
            }
            return;
        }

        let last = if closed { n } else { n - 1 };
        let mut best: Option<(usize, f32)> = None;
        for i in 0..last {
            let a = self.to_screen(origin, points[i].anchor + base);
            let b = self.to_screen(origin, points[(i + 1) % n].anchor + base);
            let d = distance_to_segment(click_screen, a, b);
            if best.map(|(_, bd)| d < bd).unwrap_or(true) {
                best = Some((i, d));
            }
        }
        let Some((seg_index, dist)) = best else { return };
        if dist > HANDLE_HIT_RADIUS * 2.0 {
            return;
        }
        let new_point = PathPoint {
            anchor: doc_pos - base,
            handle_in: None,
            handle_out: None,
            point_type: PointType::Disconnected,
            corner_radius: 0.0,
        };

        if closed {
            let mut opened: Vec<PathPoint> = Vec::with_capacity(n + 2);
            opened.push(new_point);
            for k in 0..n {
                opened.push(points[(seg_index + 1 + k) % n]);
            }
            opened.push(new_point);
            history.snapshot();
            if let Some(layer) = history.mutate().active_page_mut().find_mut(layer_id) {
                if let LayerKind::Path { points, closed } = &mut layer.kind {
                    *points = opened;
                    *closed = false;
                }
            }
            normalize_path_frame(history.mutate().active_page_mut(), layer_id);
        } else {
            let mut first = points[..=seg_index].to_vec();
            first.push(new_point);
            let mut second = vec![new_point];
            second.extend_from_slice(&points[seg_index + 1..]);
            self.replace_with_split_paths(history, selection, layer_id, first, second);
        }
    }

    /// Replaces `layer_id` (an open `Path`) in place with two new `Path`
    /// layers built from `first`/`second` (each a run of points from the
    /// original, relative to its old `frame.pos`), spliced into the same
    /// parent list at the same position. Selects both new layers.
    fn replace_with_split_paths(
        &mut self,
        history: &mut History,
        selection: &mut Vec<LayerId>,
        layer_id: LayerId,
        first: Vec<PathPoint>,
        second: Vec<PathPoint>,
    ) {
        history.snapshot();
        let page = history.mutate().active_page_mut();
        let Some(orig) = page.find(layer_id) else { return };
        let name = orig.name.clone();
        let style = orig.style.clone();
        let frame_pos = orig.frame.pos;
        let rotation = orig.frame.rotation;
        let layer_a = build_path_layer(&name, &style, frame_pos, rotation, &first);
        let layer_b = build_path_layer(&name, &style, frame_pos, rotation, &second);
        let id_a = layer_a.id;
        let id_b = layer_b.id;
        let Some(parent) = crate::grouping::find_common_parent_list(&mut page.layers, &[layer_id]) else {
            return;
        };
        let Some(idx) = parent.iter().position(|l| l.id == layer_id) else {
            return;
        };
        parent.remove(idx);
        parent.insert(idx, layer_b);
        parent.insert(idx, layer_a);
        *selection = vec![id_a, id_b];
    }

    pub fn ui(
        &mut self,
        ui: &mut Ui,
        history: &mut History,
        tool: &mut Tool,
        selection: &mut Vec<LayerId>,
    ) {
        if !self.pending_font_resize.is_empty() {
            let ctx = ui.ctx().clone();
            self.pending_font_resize.retain_mut(|(id, family, retries_left)| {
                let bound = ctx.fonts(|f| {
                    f.definitions().families.contains_key(&egui::FontFamily::Name(family.as_str().into()))
                });
                if bound {
                    if let Some(l) = history.mutate().active_page_mut().find_mut(*id) {
                        apply_text_auto_resize(&ctx, l);
                    }
                    return false;
                }
                *retries_left = retries_left.saturating_sub(1);
                *retries_left > 0
            });
        }

        let (response, painter) =
            ui.allocate_painter(ui.available_size(), Sense::click_and_drag());
        let full_rect = response.rect;
        let ruler_size = if self.show_rulers { RULER_SIZE } else { 0.0 };
        let canvas_rect = Rect::from_min_max(full_rect.min + Vec2::splat(ruler_size), full_rect.max);
        let origin = canvas_rect.min;
        // Everything drawn on the document itself (background, grid, layers,
        // guides, drag previews) is clipped to `canvas_rect` so none of it
        // bleeds under the ruler strips reserved above/left of it.
        let canvas_painter = painter.with_clip_rect(canvas_rect);

        // OS drag-and-drop of image files onto the canvas — "drag images
        // from your Mac or browser directly onto the canvas" (the
        // primary insertion path). `dropped_files` is only non-empty on the
        // one frame the OS drop event fires, so no manual dedup is needed.
        // Files dropped outside `canvas_rect` (e.g. onto a side panel) are
        // ignored rather than inserted at some arbitrary fallback position.
        let dropped_paths: Vec<std::path::PathBuf> =
            ui.ctx().input(|i| i.raw.dropped_files.iter().map(|f| f.path().to_path_buf()).collect());
        if !dropped_paths.is_empty() {
            if let Some(drop_pos) = ui.ctx().input(|i| i.pointer.interact_pos()) {
                if canvas_rect.contains(drop_pos) {
                    let doc_pos = self.to_doc(origin, drop_pos);
                    let layers = crate::image_ops::build_image_grid(&dropped_paths, doc_pos, 320.0, 20.0);
                    if !layers.is_empty() {
                        history.snapshot();
                        let page = history.mutate().active_page_mut();
                        let new_ids: Vec<LayerId> = layers.iter().map(|l| l.id).collect();
                        let hint = self.insert_hint_parent.take();
                        for layer in layers {
                            insert_layer(page, layer, doc_pos, hint);
                        }
                        *selection = new_ids;
                    }
                }
            }
        }

        // Direct-selection point selection (`selected_points`) is scoped to
        // whichever single Path layer it was made on. Drop it the moment
        // that's no longer the active selection (a different layer/selection,
        // multi-layer selection, or a tool switch away from Select), and on
        // Escape.
        let current_path_id = if *tool == Tool::Select && selection.len() == 1 {
            history
                .get()
                .active_page()
                .find(selection[0])
                .filter(|l| matches!(l.kind, LayerKind::Path { .. }))
                .map(|l| l.id)
        } else {
            None
        };
        if current_path_id != self.point_edit_layer {
            self.selected_points.clear();
        }
        self.point_edit_layer = current_path_id;
        if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
            self.selected_points.clear();
        }

        // "Edit Image" mode is scoped to one layer; drop it the moment that
        // layer stops existing (deleted mid-edit) or stops being an `Image`
        // (an undo landing on an earlier document state), or on Escape.
        if let Some(edit) = &self.image_edit {
            let still_image = matches!(
                history.get().active_page().find(edit.layer_id).map(|l| &l.kind),
                Some(LayerKind::Image { .. })
            );
            if !still_image {
                self.end_image_edit();
            }
        }
        if self.image_edit.is_some() && ui.input(|i| i.key_pressed(egui::Key::Escape)) {
            self.end_image_edit();
        }

        // Pen-tool bookkeeping that can commit/cancel the in-progress path
        // runs first, before `history.get()` below is borrowed for the rest
        // of the frame (drawing + Select-tool hit-testing) — committing
        // touches `history` mutably and may flip `*tool` back to `Select`.
        if *tool != Tool::Pen && self.pen.is_some() {
            // A tool switch away from Pen mid-path commits whatever was
            // drawn so far as an open path, rather than silently discarding it.
            self.finish_pen_path(history, selection, tool, false);
        }
        if self.pen.is_some() {
            if ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                self.finish_pen_path(history, selection, tool, false);
            } else if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                self.pen = None;
                self.drag = DragState::None;
            }
        }
        if *tool == Tool::Pen {
            if response.double_clicked() {
                self.finish_pen_path(history, selection, tool, false);
            } else if response.clicked() {
                let mouse = response.interact_pointer_pos().unwrap_or(origin);
                let doc_pos = self.to_doc(origin, mouse);
                if !self.try_close_pen_path(doc_pos, origin, history, selection, tool) {
                    self.pen.get_or_insert_with(Vec::new).push(PathPoint {
                        anchor: doc_pos,
                        handle_in: None,
                        handle_out: None,
                        point_type: PointType::Straight,
                        corner_radius: 0.0,
                    });
                }
            }
        }

        // Direct-selection editing of a single already-committed Path's
        // points: double-click a segment to insert an anchor, Alt/Option-click
        // an anchor to delete it. Dragging an existing anchor/handle is
        // handled below, in the main drag_started/dragged/drag_stopped
        // sections (see `hovered_point`), since that only needs to read the
        // document, not mutate it, so it doesn't need this same early-exit
        // treatment to avoid the `history.get()` borrow further down.
        if *tool == Tool::Select && selection.len() == 1 {
            if response.double_clicked() {
                if let Some(mouse) = response.interact_pointer_pos() {
                    let doc_pos = self.to_doc(origin, mouse);
                    if !self.try_convert_anchor_to_curve(history, selection, doc_pos, origin) {
                        self.try_insert_path_point(history, selection, doc_pos, origin);
                    }
                }
            } else if response.clicked() && ui.input(|i| i.modifiers.alt) {
                if let Some(mouse) = response.interact_pointer_pos() {
                    let doc_pos = self.to_doc(origin, mouse);
                    self.try_delete_path_point(history, selection, doc_pos, origin);
                }
            }
            // Point-type shortcuts (1-4), applied to every point in
            // `selected_points`. Only meaningful once a point is selected, so
            // this can't misfire as e.g. a zoom-level or tool shortcut typed
            // elsewhere while a Path just happens to be selected.
            if !self.selected_points.is_empty() {
                let new_type = if ui.input(|i| i.key_pressed(egui::Key::Num1)) {
                    Some(PointType::Straight)
                } else if ui.input(|i| i.key_pressed(egui::Key::Num2)) {
                    Some(PointType::Mirror)
                } else if ui.input(|i| i.key_pressed(egui::Key::Num3)) {
                    Some(PointType::Asymmetric)
                } else if ui.input(|i| i.key_pressed(egui::Key::Num4)) {
                    Some(PointType::Disconnected)
                } else {
                    None
                };
                if let Some(new_type) = new_type {
                    self.apply_point_type(history, selection[0], new_type);
                }
            }
        }

        // Double-click a `Text` layer (any Select-tool double-click,
        // regardless of what's currently selected) to start in-place canvas
        // editing — the primary text-editing entry point, and its
        // "click another text layer while editing" cross-layer convenience
        // falls out naturally since this always re-targets `editing_text`.
        // Double-clicking a bare `Rectangle`/`Oval` instead gives it a
        // inline text label (see `add_shape_label`) and starts
        // editing that immediately.
        if *tool == Tool::Select && response.double_clicked() {
            if let Some(mouse) = response.interact_pointer_pos() {
                let doc_pos = self.to_doc(origin, mouse);
                if let Some(id) = hit_test(history.get().active_page(), doc_pos) {
                    let is_text =
                        matches!(history.get().active_page().find(id).map(|l| &l.kind), Some(LayerKind::Text { .. }));
                    let is_labelable_shape = matches!(
                        history.get().active_page().find(id).map(|l| &l.kind),
                        Some(LayerKind::Rectangle { .. } | LayerKind::Oval)
                    );
                    if is_text {
                        *selection = vec![id];
                        self.start_editing_text(history, id);
                    } else if is_labelable_shape {
                        if let Some(text_id) = self.add_shape_label(history, id) {
                            *selection = vec![text_id];
                        }
                    }
                }
            }
        }

        if *tool == Tool::Scissors && response.clicked() {
            if let Some(mouse) = response.interact_pointer_pos() {
                let doc_pos = self.to_doc(origin, mouse);
                self.try_scissor_path(history, selection, doc_pos, origin);
            }
        }

        canvas_painter.rect_filled(canvas_rect, 0.0, Color32::from_gray(235));
        if self.show_grid {
            draw_pixel_grid(&canvas_painter, canvas_rect, origin, self.pan, self.zoom);
        }

        let doc = history.get();
        let page = doc.active_page();
        let ctx = ui.ctx().clone();
        draw_children(
            &canvas_painter,
            &ctx,
            &mut self.image_cache,
            &mut self.mask_cache,
            &mut self.noise_cache,
            &mut self.halftone_cache,
            &mut self.pattern_cache,
            &mut self.shadow_cache,
            &page.layers,
            Vec2::ZERO,
            origin,
            self.pan,
            self.zoom,
            1.0,
            self.editing_text,
        );
        self.image_cache.evict_stale(&page.all_layer_ids());
        self.mask_cache.evict_stale(&page.all_layer_ids());
        self.noise_cache.evict_stale(&page.all_layer_ids());
        self.halftone_cache.evict_stale(&page.all_layer_ids());
        self.pattern_cache.evict_stale(&page.all_layer_ids());
        self.shadow_cache.evict_stale(&page.all_layer_ids());

        // "Edit Image" mode's selection overlay: a translucent tint over
        // every masked pixel, rebuilt only when the mask actually changed
        // (see `ImageEditState::overlay_stale`).
        if let Some(edit) = &self.image_edit {
            if let (Some(bounds_layer), Some(offset)) =
                (page.find(edit.layer_id), page.absolute_offset(edit.layer_id))
            {
                let doc_bounds = bounds_layer.frame.bounds().translate(offset);
                let needs_rebuild = edit.overlay_stale || edit.overlay.is_none();
                let stale_mask = needs_rebuild.then(|| (edit.mask.clone(), edit.width, edit.height));
                if let Some((mask, width, height)) = stale_mask {
                    let color_image = build_mask_overlay(&mask, width, height);
                    let texture = ctx.load_texture("image-edit-overlay", color_image, egui::TextureOptions::NEAREST);
                    if let Some(edit) = &mut self.image_edit {
                        edit.overlay = Some(texture);
                        edit.overlay_stale = false;
                    }
                }
                let screen_rect =
                    Rect::from_two_pos(self.to_screen(origin, doc_bounds.min), self.to_screen(origin, doc_bounds.max));
                if let Some(texture) = self.image_edit.as_ref().and_then(|e| e.overlay.as_ref()) {
                    canvas_painter.image(
                        texture.id(),
                        screen_rect,
                        Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
                        Color32::WHITE,
                    );
                }
                canvas_painter.rect_stroke(
                    screen_rect,
                    0.0,
                    EguiStroke::new(2.0, Color32::from_rgb(0, 90, 158)),
                    egui::StrokeKind::Outside,
                );
            }
        }

        draw_guides(&canvas_painter, canvas_rect, origin, self.pan, self.zoom, &page.guides);

        // Gather (layer, absolute-offset) for every selected id that still exists.
        let selected_layers: Vec<(&Layer, Vec2)> = selection
            .iter()
            .filter_map(|&id| {
                let layer = page.find(id)?;
                let offset = page.absolute_offset(id)?;
                Some((layer, offset))
            })
            .collect();
        // `Arrow` shares `Line`'s exact two-endpoint resize model (see
        // `LayerKind::Arrow`'s doc comment), so it reuses this same
        // endpoint-drag path rather than the generic bounding-box handles —
        // and, per the rotation-aware-resize design, never gets rotate
        // corner-handles either (its two endpoints already fully describe
        // its orientation).
        let single_line = if selected_layers.len() == 1
            && matches!(selected_layers[0].0.kind, LayerKind::Line | LayerKind::Arrow { .. })
        {
            Some(selected_layers[0])
        } else {
            None
        };
        let single_path = if selected_layers.len() == 1 && matches!(selected_layers[0].0.kind, LayerKind::Path { .. }) {
            Some(selected_layers[0])
        } else {
            None
        };
        // Owned mirror of `single_path.is_some()` — the click-to-select block
        // below runs after `history.mutate()` calls elsewhere in this
        // function, so it can't hold onto `single_path`'s borrow of `history`.
        let is_single_path_selected = single_path.is_some();

        // On the exact frame a drag is recognized, `response.hover_pos()`
        // already reflects the *current*, post-threshold pointer position —
        // egui only flags `drag_started()` once the pointer has moved past
        // `InputOptions::max_click_dist` (6px), which can already exceed
        // `HANDLE_HIT_RADIUS` (8px) by the time the next frame samples it,
        // especially on a fast pointer move. Hit-testing anchors/handles
        // against that already-moved position (instead of where the button
        // actually went down) made grabbing a point to start editing it
        // flaky — working on a slow drag, missing on a fast one. Falling
        // back to the press origin for hit-testing on that one frame fixes
        // it; every other frame (plain hover, and every later frame of an
        // already-started drag) keeps using the live position exactly as
        // before, so the hover highlight still tracks the cursor normally.
        let hit_test_pos = if response.drag_started() {
            ui.input(|i| i.pointer.press_origin()).or_else(|| response.hover_pos())
        } else {
            response.hover_pos()
        };

        let mut hovered_handle: Option<Handle> = None;
        let mut hovered_rotate_corner: Option<Handle> = None;
        let mut hovered_point: Option<(usize, PathPart)> = None;
        let mut hovered_gap_handle: Option<usize> = None;
        if let Some((layer, offset)) = single_line {
            // A line/arrow has no interior to bound, so (unlike every other
            // shape) it gets no rectangle outline — just its own two real
            // endpoints as handles. Using `frame.bounds()`'s min/max corners
            // here (as every other shape's resize handles do) would only
            // land on the actual endpoints when the shape was dragged
            // top-left-to-bottom-right; any other drag direction (or a
            // rotation baked in from an ungrouped parent, see
            // `Frame::rotation`'s doc comment) puts the circles off the
            // visible line entirely and swaps which endpoint each one
            // controls. `frame.start()`/`end()`, rotated the same way
            // `draw_layer` renders them, always match what's on screen.
            let doc_bounds = layer.frame.bounds().translate(offset);
            let center = self.to_screen(origin, doc_bounds.center());
            let start = rotate_point(self.to_screen(origin, layer.frame.start() + offset), center, layer.frame.rotation);
            let end = rotate_point(self.to_screen(origin, layer.frame.end() + offset), center, layer.frame.rotation);
            for (h, p) in [(Handle::TopLeft, start), (Handle::BottomRight, end)] {
                if let Some(mp) = hit_test_pos {
                    if mp.distance(p) <= HANDLE_HIT_RADIUS {
                        hovered_handle = Some(h);
                    }
                }
                painter.circle(p, HANDLE_RADIUS, Color32::WHITE, EguiStroke::new(1.5, SELECTION_COLOR));
            }
        } else if let Some((layer, offset)) = single_path {
            if let LayerKind::Path { points, .. } = &layer.kind {
                for (i, p) in points.iter().enumerate() {
                    let anchor_doc = p.anchor + layer.frame.pos.to_vec2() + offset;
                    let anchor_screen = self.to_screen(origin, anchor_doc);
                    for (part, handle) in [(PathPart::HandleIn, p.handle_in), (PathPart::HandleOut, p.handle_out)] {
                        let Some(h) = handle else { continue };
                        let handle_screen = self.to_screen(origin, anchor_doc + h);
                        painter.line_segment(
                            [anchor_screen, handle_screen],
                            EguiStroke::new(1.0, Color32::from_gray(120)),
                        );
                        if let Some(mp) = hit_test_pos {
                            if mp.distance(handle_screen) <= HANDLE_HIT_RADIUS {
                                hovered_point = Some((i, part));
                            }
                        }
                        let hl = hovered_point == Some((i, part));
                        let r = if hl { 4.5 } else { 3.0 };
                        painter.circle_filled(handle_screen, r, Color32::WHITE);
                        painter.circle_stroke(
                            handle_screen,
                            r,
                            EguiStroke::new(1.0, if hl { SELECTION_COLOR } else { Color32::from_gray(120) }),
                        );
                    }
                    if let Some(mp) = hit_test_pos {
                        if mp.distance(anchor_screen) <= HANDLE_HIT_RADIUS {
                            hovered_point = Some((i, PathPart::Anchor));
                        }
                    }
                    let hl = hovered_point == Some((i, PathPart::Anchor));
                    let selected = self.selected_points.contains(&i);
                    let radius = if hl || selected { HANDLE_RADIUS * 1.3 } else { HANDLE_RADIUS };
                    let fill = if hl || selected { SELECTION_COLOR } else { Color32::WHITE };
                    painter.circle(anchor_screen, radius, fill, EguiStroke::new(1.5, SELECTION_COLOR));
                }
            }
        } else if !selected_layers.is_empty() {
            // A rotated outline/handles only ever apply to a single selected
            // leaf's own rotation — a multi-selection always shows a plain
            // AABB (its members can each have different rotations, and
            // "the rotated bbox of mixed rotations" isn't well-defined), same
            // simplification precedent as a mixed multi-select losing a
            // `Line`'s exact direction fidelity (see `CLAUDE.md`).
            let outline_rotation =
                if let [(layer, _)] = selected_layers[..] { layer.frame.rotation } else { 0.0 };
            for (layer, offset) in &selected_layers {
                let b = display_bounds(layer).translate(*offset);
                let sr = Rect::from_two_pos(self.to_screen(origin, b.min), self.to_screen(origin, b.max));
                // The alignment reference layer (see `reference_layer`'s doc
                // comment) gets a thicker, fully-opaque outline instead of
                // the normal thin/dim per-layer one.
                let is_reference = self.reference_layer == Some(layer.id);
                let stroke = if is_reference {
                    EguiStroke::new(2.5, SELECTION_COLOR)
                } else {
                    EguiStroke::new(1.0, SELECTION_COLOR.gamma_multiply(0.6))
                };
                if layer.frame.rotation == 0.0 {
                    painter.rect_stroke(sr, 0.0, stroke, egui::StrokeKind::Outside);
                } else {
                    let corners = rotated_corners(sr, layer.frame.rotation);
                    painter.add(egui::Shape::closed_line(corners.to_vec(), stroke));
                }
            }
            let overall = selected_layers
                .iter()
                .map(|(l, o)| display_bounds(l).translate(*o))
                .reduce(|a, b| a.union(b))
                .unwrap();
            let screen_rect = Rect::from_two_pos(
                self.to_screen(origin, overall.min),
                self.to_screen(origin, overall.max),
            );
            if outline_rotation == 0.0 {
                painter.rect_stroke(screen_rect, 0.0, EguiStroke::new(1.5, SELECTION_COLOR), egui::StrokeKind::Outside);
            } else {
                let corners = rotated_corners(screen_rect, outline_rotation);
                painter.add(egui::Shape::closed_line(corners.to_vec(), EguiStroke::new(1.5, SELECTION_COLOR)));
            }
            for h in Handle::ALL {
                let p = rotate_point(h.pos(screen_rect), screen_rect.center(), outline_rotation);
                if let Some(mp) = hit_test_pos {
                    if mp.distance(p) <= HANDLE_HIT_RADIUS {
                        hovered_handle = Some(h);
                    } else if matches!(h, Handle::TopLeft | Handle::TopRight | Handle::BottomRight | Handle::BottomLeft)
                        && mp.distance(p) <= ROTATE_ZONE_OUTER_RADIUS
                    {
                        hovered_rotate_corner = Some(h);
                    }
                }
                painter.circle(p, HANDLE_RADIUS, Color32::WHITE, EguiStroke::new(1.5, SELECTION_COLOR));
            }
        }
        // Smart Distribute gap-adjustment handles — only shown while the
        // exact set of layers from the most recent Distribute is still
        // selected (see `last_distributed`'s doc comment).
        let mut gap_handle_order: Vec<(LayerId, Rect)> = Vec::new();
        if *tool == Tool::Select && matches!(self.drag, DragState::None) {
            if let Some((dist_ids, axis)) = self.last_distributed.clone() {
                let mut a = dist_ids.clone();
                let mut b = selection.clone();
                a.sort();
                b.sort();
                if a == b && a.len() >= 3 {
                    gap_handle_order = distribution_order(page, &dist_ids, axis);
                    for (i, doc_pos) in gap_handle_positions(&gap_handle_order, axis).into_iter().enumerate() {
                        let p = self.to_screen(origin, doc_pos);
                        if let Some(mp) = hit_test_pos {
                            if mp.distance(p) <= HANDLE_HIT_RADIUS {
                                hovered_gap_handle = Some(i);
                            }
                        }
                        let hl = hovered_gap_handle == Some(i);
                        painter.circle(p, if hl { 5.0 } else { 3.5 }, SELECTION_COLOR, EguiStroke::new(1.0, Color32::WHITE));
                    }
                }
            }
        }
        if let Some(h) = hovered_handle {
            ui.ctx().set_cursor_icon(h.cursor());
        } else if hovered_gap_handle.is_some() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeColumn);
        } else if hovered_rotate_corner.is_some() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::Crosshair);
        } else if *tool == Tool::Select {
            let guide_cursor = response
                .hover_pos()
                .filter(|mp| canvas_rect.contains(*mp))
                .and_then(|mp| self.hovered_guide_index(origin, &page.guides, mp))
                .map(|idx| match page.guides[idx].orientation {
                    GuideOrientation::Horizontal => egui::CursorIcon::ResizeVertical,
                    GuideOrientation::Vertical => egui::CursorIcon::ResizeHorizontal,
                });
            if let Some(cursor) = guide_cursor {
                ui.ctx().set_cursor_icon(cursor);
            }
        }

        if *tool == Tool::Pan {
            ui.ctx().set_cursor_icon(egui::CursorIcon::Grab);
        }
        if *tool == Tool::Scissors {
            ui.ctx().set_cursor_icon(egui::CursorIcon::Crosshair);
        }

        // Option-hover distance measurement: with a selection and Option
        // held, hovering another layer shows the gap between their closest
        // edges — a "measure distance" affordance. Only while
        // idle (no drag in progress) so it doesn't fight the snap-line
        // overlay drawn during an actual move/resize.
        if *tool == Tool::Select && matches!(self.drag, DragState::None) && ui.input(|i| i.modifiers.alt) {
            if let Some(sel_bounds) = selected_layers
                .iter()
                .map(|(l, o)| l.frame.rotated_bounds().translate(*o))
                .reduce(|a, b| a.union(b))
            {
                if let Some(mp) = response.hover_pos().filter(|mp| canvas_rect.contains(*mp)) {
                    let doc_pos = self.to_doc(origin, mp);
                    if let Some(hover_id) = hit_test(page, doc_pos) {
                        if !selection.contains(&hover_id) {
                            if let (Some(hl), Some(hoff)) = (page.find(hover_id), page.absolute_offset(hover_id)) {
                                let hover_bounds = hl.frame.rotated_bounds().translate(hoff);
                                draw_distance_measurement(&painter, |p| self.to_screen(origin, p), sel_bounds, hover_bounds);
                            }
                        }
                    }
                }
            }
        }

        // Direct-selection point selection: shift-click toggles a point in
        // `selected_points`, a plain click replaces it, and Cmd/Ctrl+A
        // selects every point of the active single Path. A drag starting on
        // an anchor (below, in `drag_started`) applies the same
        // replace/extend rule so a click-that-becomes-a-drag behaves
        // consistently with a plain click. Excludes double-click (segment
        // insert) and Alt-click (point delete), which are handled earlier
        // and shouldn't also toggle a selection.
        if *tool == Tool::Select {
            if let Some((layer, _)) = single_path {
                if ui.input(|i| i.modifiers.command && i.key_pressed(egui::Key::A)) {
                    if let LayerKind::Path { points, .. } = &layer.kind {
                        self.selected_points = (0..points.len()).collect();
                    }
                }
            }
            if response.clicked() && !response.double_clicked() && !ui.input(|i| i.modifiers.alt) {
                if let Some((index, PathPart::Anchor)) = hovered_point {
                    if ui.input(|i| i.modifiers.shift) {
                        if let Some(pos) = self.selected_points.iter().position(|&i| i == index) {
                            self.selected_points.remove(pos);
                        } else {
                            self.selected_points.push(index);
                        }
                    } else {
                        self.selected_points = vec![index];
                    }
                }
            }
        }

        // --- Input handling ---
        if response.drag_started() {
            // Same reasoning as `hit_test_pos` above: on this exact frame,
            // `interact_pointer_pos()` already reflects the pointer's
            // current (post-threshold) position, not where the button
            // actually went down. Every `DragState` that records a
            // `start_doc`/starting bounds to diff future positions against
            // needs the true press origin here, or the dragged content lags
            // behind the cursor by however far it moved before the drag was
            // recognized.
            let mouse = ui
                .input(|i| i.pointer.press_origin())
                .or_else(|| response.interact_pointer_pos())
                .unwrap_or(origin);
            let doc_pos = self.to_doc(origin, mouse);
            if let Some(edit) = &self.image_edit {
                // "Edit Image" mode captures every drag for
                // Selection/Magic Wand, bypassing the normal
                // guide/pan/select/pen/shape handling below entirely — it's
                // modal, same as the Pen tool's in-progress path.
                let subtract = ui.input(|i| i.modifiers.alt);
                let replace = !subtract && !ui.input(|i| i.modifiers.shift);
                let base_mask = if replace { vec![false; edit.mask.len()] } else { edit.mask.clone() };
                self.drag = DragState::ImageEditDrag { start_doc: doc_pos, subtract, base_mask };
            } else {
            let in_top_ruler = self.show_rulers && mouse.y < canvas_rect.min.y && mouse.x >= canvas_rect.min.x;
            let in_left_ruler = self.show_rulers && mouse.x < canvas_rect.min.x && mouse.y >= canvas_rect.min.y;
            let guide_grab = if *tool == Tool::Select
                && canvas_rect.contains(mouse)
                && hovered_point.is_none()
                && hovered_handle.is_none()
            {
                self.hovered_guide_index(origin, &page.guides, mouse)
            } else {
                None
            };
            if in_top_ruler && !in_left_ruler {
                self.drag = DragState::CreatingGuide {
                    orientation: GuideOrientation::Horizontal,
                };
            } else if in_left_ruler && !in_top_ruler {
                self.drag = DragState::CreatingGuide {
                    orientation: GuideOrientation::Vertical,
                };
            } else if let Some(index) = guide_grab {
                self.drag = DragState::MovingGuide {
                    index,
                    orientation: page.guides[index].orientation,
                };
            } else if *tool == Tool::Pan {
                self.drag = DragState::PanningView {
                    start_mouse: mouse,
                    start_pan: self.pan,
                };
            } else if *tool == Tool::Select {
                if let (Some((point_index, part)), Some((layer, offset))) = (hovered_point, single_path) {
                    let layer_id = layer.id;
                    let new_drag = match part {
                        PathPart::Anchor => {
                            if let LayerKind::Path { points, .. } = &layer.kind {
                                // Dragging an already-selected anchor moves the
                                // whole selection together; dragging an
                                // unselected one replaces the selection with
                                // just this point (shift extends it instead),
                                // matching the click-selection rule above so a
                                // click-that-becomes-a-drag behaves the same way.
                                if !self.selected_points.contains(&point_index) {
                                    if ui.input(|i| i.modifiers.shift) {
                                        self.selected_points.push(point_index);
                                    } else {
                                        self.selected_points = vec![point_index];
                                    }
                                }
                                let point_indices = self.selected_points.clone();
                                let start_anchors =
                                    point_indices.iter().map(|&i| points[i].anchor).collect();
                                Some(DragState::EditingPathAnchor {
                                    layer_id,
                                    point_indices,
                                    start_doc: doc_pos,
                                    start_anchors,
                                })
                            } else {
                                None
                            }
                        }
                        PathPart::HandleIn | PathPart::HandleOut => {
                            let side = if part == PathPart::HandleIn { HandleSide::In } else { HandleSide::Out };
                            Some(DragState::EditingPathHandle {
                                layer_id,
                                parent_offset: offset,
                                point_index,
                                side,
                            })
                        }
                    };
                    if let Some(new_drag) = new_drag {
                        self.drag = new_drag;
                        history.snapshot();
                    }
                } else if let Some(gap_index) = hovered_gap_handle {
                    let (_, axis) = self.last_distributed.clone().expect("hovered_gap_handle only set when last_distributed matches");
                    let order: Vec<LayerId> = gap_handle_order.iter().map(|(id, _)| *id).collect();
                    let starts: Vec<Pos2> = order
                        .iter()
                        .filter_map(|&id| page.find(id).map(|l| l.frame.pos))
                        .collect();
                    if starts.len() == order.len() {
                        self.drag = DragState::AdjustingDistributionGap { start_doc: doc_pos, axis, gap_index, order, starts };
                        history.snapshot();
                    }
                } else if hovered_rotate_corner.is_some() {
                    let overall = selected_layers
                        .iter()
                        .map(|(l, o)| display_bounds(l).translate(*o))
                        .reduce(|a, b| a.union(b));
                    if let Some(overall) = overall {
                        let pivot = overall.center();
                        let start_angle = (doc_pos - pivot).angle();
                        let mut layers = Vec::new();
                        for (layer, offset) in &selected_layers {
                            collect_rotatable_leaves(layer, *offset, &mut layers);
                        }
                        if !layers.is_empty() {
                            self.drag = DragState::Rotating { pivot, start_angle, layers };
                            history.snapshot();
                        }
                    }
                } else if let Some(handle) = hovered_handle {
                    if single_line.is_some() {
                        let id = selection[0];
                        let offset = page.absolute_offset(id).unwrap_or(Vec2::ZERO);
                        self.drag = DragState::ResizingLine { id, handle, parent_offset: offset };
                        history.snapshot();
                    } else {
                        let mut layers: Vec<ResizeLayerInfo> = Vec::new();
                        for (l, o) in &selected_layers {
                            collect_resizable_leaves(l, *o, &mut layers);
                        }
                        if let Some(overall) = layers.iter().map(|l| l.abs_bounds).reduce(|a, b| a.union(b)) {
                            self.drag = DragState::ResizingGroup {
                                handle,
                                start_overall_bounds: overall,
                                layers,
                                scale_style: self.scaling.is_some(),
                                scale_anchor: self.scale_anchor,
                            };
                            history.snapshot();
                        }
                    }
                } else if let Some((layer, _)) = single_path
                    && hit_test(page, doc_pos) != Some(layer.id)
                {
                    // Empty-space drag while a single Path is the active
                    // selection: box-select its points instead of falling
                    // through to layer marquee/hit-test. A drag starting on
                    // the path's own body instead falls through to the
                    // generic hit-test branch below so it moves the layer,
                    // same as every other shape.
                    let shift = ui.input(|i| i.modifiers.shift);
                    if !shift {
                        self.selected_points.clear();
                    }
                    self.drag = DragState::MarqueePoints {
                        layer_id: layer.id,
                        start_doc: doc_pos,
                        additive: shift,
                        base_selection: self.selected_points.clone(),
                    };
                } else {
                    let modifiers = ui.input(|i| i.modifiers);
                    let shift = modifiers.shift;
                    if modifiers.command && modifiers.alt && !selection.is_empty() {
                        // Cmd+Option+drag: move the current selection even
                        // when it isn't the topmost layer under the cursor
                        // ("move a layer buried under others"),
                        // bypassing hit_test entirely.
                        self.drag = DragState::MovingSelection {
                            start_doc: doc_pos,
                            starts: move_starts(page, selection),
                        };
                        history.snapshot();
                    } else {
                    match hit_test(page, doc_pos) {
                        Some(id) => {
                            if shift {
                                if selection.contains(&id) {
                                    selection.retain(|&s| s != id);
                                } else {
                                    selection.push(id);
                                    self.drag = DragState::MovingSelection {
                                        start_doc: doc_pos,
                                        starts: move_starts(page, selection),
                                    };
                                    history.snapshot();
                                }
                            } else if modifiers.alt {
                                // Option+drag: duplicate the selection in
                                // place (no offset — the drag itself moves
                                // it away) and drag the copies, leaving the
                                // originals untouched.
                                if !selection.contains(&id) {
                                    *selection = vec![id];
                                }
                                let ids = selection.clone();
                                history.snapshot();
                                let new_ids = grouping::duplicate_layers(history.mutate().active_page_mut(), &ids, Vec2::ZERO);
                                if !new_ids.is_empty() {
                                    *selection = new_ids;
                                    let page = history.get().active_page();
                                    self.drag = DragState::MovingSelection {
                                        start_doc: doc_pos,
                                        starts: move_starts(page, selection),
                                    };
                                }
                            } else {
                                // Clicking an already-fully-selected single
                                // layer again marks it as the alignment
                                // "reference layer" (a common gesture)
                                // instead of re-selecting it.
                                if selection.len() == 1 && selection[0] == id {
                                    self.reference_layer = if self.reference_layer == Some(id) { None } else { Some(id) };
                                } else if hit_is_within_selection(page, selection, id) {
                                    // The hit leaf is nested inside a
                                    // Group/Artboard that's already part of
                                    // the current selection (e.g. a masked
                                    // picture's mask, both children of a
                                    // selected Group) — hit_test always
                                    // drills down to the leaf, but dragging
                                    // here should move the whole selected
                                    // ancestor, not silently narrow the
                                    // selection down to just the one child
                                    // under the cursor.
                                } else {
                                    *selection = vec![id];
                                    self.reference_layer = None;
                                }
                                self.drag = DragState::MovingSelection {
                                    start_doc: doc_pos,
                                    starts: move_starts(page, selection),
                                };
                                history.snapshot();
                            }
                        }
                        None => {
                            // Clicking empty canvas is one of Scale mode's
                            // three exit paths (Enter and the inspector's
                            // Finish button are the other two).
                            self.scaling = None;
                            // Command+Shift (invert) implies Shift's
                            // preserve-existing-selection behavior, so only a
                            // plain (non-Command) Shift clears nothing while a
                            // bare click still starts fresh.
                            if !shift {
                                selection.clear();
                            }
                            self.drag = DragState::Marquee {
                                start_doc: doc_pos,
                                additive: shift,
                                base_selection: selection.clone(),
                                contained_only: modifiers.alt,
                                ignore_groups: modifiers.command,
                                invert: modifiers.command && modifiers.shift,
                            };
                        }
                    }
                    }
                }
            } else if *tool == Tool::Pen {
                if !self.try_close_pen_path(doc_pos, origin, history, selection, tool) {
                    let point_index = {
                        let points = self.pen.get_or_insert_with(Vec::new);
                        points.push(PathPoint {
                            anchor: doc_pos,
                            handle_in: None,
                            handle_out: None,
                            point_type: PointType::Mirror,
                            corner_radius: 0.0,
                        });
                        points.len() - 1
                    };
                    self.drag = DragState::DrawingPenHandle { point_index };
                }
            } else if *tool == Tool::Scissors {
                // Cutting is a plain click, handled up front (alongside the
                // insert/delete-point click handling) rather than as a drag.
            } else {
                self.drag = DragState::CreatingShape { start_doc: doc_pos };
                history.snapshot();
            }
            }
        }

        // A plain click (press+release with no perceptible pointer movement)
        // never fires `drag_started()` — egui only classifies an interaction
        // as a drag once movement crosses its threshold — so click-to-select
        // needs its own path here rather than living solely in the
        // `drag_started` block above, which only ever runs for an actual
        // drag. Excludes anything with a dedicated hover target (handles,
        // path points, gap handles) since those have no meaning without
        // movement, and Edit Image mode, whose clicks are Magic Wand/selection
        // within the image rather than canvas layer selection.
        if *tool == Tool::Select
            && response.clicked()
            && !response.double_clicked()
            && self.image_edit.is_none()
            && hovered_point.is_none()
            && hovered_handle.is_none()
            && hovered_rotate_corner.is_none()
            && hovered_gap_handle.is_none()
            && !is_single_path_selected
        {
            let mouse = response.interact_pointer_pos().unwrap_or(origin);
            let doc_pos = self.to_doc(origin, mouse);
            let modifiers = ui.input(|i| i.modifiers);
            match hit_test(history.get().active_page(), doc_pos) {
                Some(id) => {
                    if modifiers.shift {
                        if selection.contains(&id) {
                            selection.retain(|&s| s != id);
                        } else {
                            selection.push(id);
                        }
                    } else if selection.len() == 1 && selection[0] == id {
                        self.reference_layer = if self.reference_layer == Some(id) { None } else { Some(id) };
                    } else {
                        *selection = vec![id];
                        self.reference_layer = None;
                    }
                }
                None => {
                    self.scaling = None;
                    if !modifiers.shift {
                        selection.clear();
                    }
                }
            }
        }

        if response.dragged() {
            let mouse = response.interact_pointer_pos().unwrap_or(origin);
            let doc_pos = self.to_doc(origin, mouse);
            let exclude = drag_exclude_ids(&self.drag);
            let snap_candidates = build_snap_candidates(history.get().active_page(), &exclude);
            match &self.drag {
                DragState::PanningView {
                    start_mouse,
                    start_pan,
                } => {
                    self.pan = *start_pan + (mouse - *start_mouse);
                }
                DragState::CreatingShape { start_doc } => {
                    let (doc_pos, lx, ly) =
                        snap_point(doc_pos, &snap_candidates, self.zoom, self.snap_enabled, self.show_grid);
                    draw_snap_lines(&canvas_painter, canvas_rect, origin, self.pan, self.zoom, lx, ly);
                    let start_screen = self.to_screen(origin, *start_doc);
                    let cur_screen = self.to_screen(origin, doc_pos);
                    if matches!(tool, Tool::Line | Tool::Arrow) {
                        canvas_painter.line_segment(
                            [start_screen, cur_screen],
                            EguiStroke::new(2.0, Color32::from_rgb(30, 30, 30)),
                        );
                    } else {
                        let preview = Rect::from_two_pos(start_screen, cur_screen);
                        canvas_painter.rect(
                            preview,
                            0.0,
                            Color32::from_rgba_unmultiplied(216, 216, 216, 160),
                            EguiStroke::new(1.0, Color32::from_rgb(30, 30, 30)),
                            egui::StrokeKind::Inside,
                        );
                        if *tool == Tool::Oval {
                            let center = preview.center();
                            let points: Vec<Pos2> =
                                ellipse_points(center, preview.width() / 2.0, preview.height() / 2.0);
                            canvas_painter.add(egui::Shape::Path(egui::epaint::PathShape {
                                points,
                                closed: true,
                                fill: Color32::from_rgba_unmultiplied(180, 180, 180, 160),
                                stroke: EguiStroke::new(1.0, Color32::from_rgb(30, 30, 30)).into(),
                            }));
                        }
                    }
                }
                DragState::Marquee { start_doc, .. }
                | DragState::MarqueePoints { start_doc, .. }
                | DragState::ImageEditDrag { start_doc, .. } => {
                    let start_screen = self.to_screen(origin, *start_doc);
                    let cur_screen = self.to_screen(origin, doc_pos);
                    let rect = Rect::from_two_pos(start_screen, cur_screen);
                    canvas_painter.rect(
                        rect,
                        0.0,
                        SELECTION_COLOR.gamma_multiply(0.15),
                        EguiStroke::new(1.0, SELECTION_COLOR),
                        egui::StrokeKind::Inside,
                    );
                }
                DragState::MovingSelection { start_doc, starts } => {
                    let mut raw_delta = doc_pos - *start_doc;
                    // Shift constrains the move to whichever axis has moved
                    // further from the drag start, matching a common
                    // horizontal/vertical move lock.
                    if ui.input(|i| i.modifiers.shift) {
                        if raw_delta.x.abs() >= raw_delta.y.abs() {
                            raw_delta.y = 0.0;
                        } else {
                            raw_delta.x = 0.0;
                        }
                    }
                    let original_bounds = {
                        let live_page = history.get().active_page();
                        starts
                            .iter()
                            .filter_map(|(id, start_pos)| {
                                let size = live_page.find(*id)?.frame.size;
                                Some(Rect::from_two_pos(*start_pos, *start_pos + size))
                            })
                            .reduce(|a, b| a.union(b))
                    };
                    let delta = if let Some(ob) = original_bounds {
                        let (d, lx, ly) = snap_bounds_delta(
                            ob,
                            raw_delta,
                            &snap_candidates,
                            self.zoom,
                            self.snap_enabled,
                            self.show_grid,
                        );
                        draw_snap_lines(&canvas_painter, canvas_rect, origin, self.pan, self.zoom, lx, ly);
                        d
                    } else {
                        raw_delta
                    };
                    for (id, start_pos) in starts {
                        if let Some(layer) = history.mutate().active_page_mut().find_mut(*id) {
                            layer.frame.pos = *start_pos + delta;
                        }
                    }
                }
                DragState::AdjustingDistributionGap {
                    start_doc,
                    axis,
                    gap_index,
                    order,
                    starts,
                } => {
                    let delta = doc_pos - *start_doc;
                    let shift = match axis {
                        DistributeAxis::Horizontal => Vec2::new(delta.x, 0.0),
                        DistributeAxis::Vertical => Vec2::new(0.0, delta.y),
                    };
                    let mutable_page = history.mutate().active_page_mut();
                    for (i, (id, start_pos)) in order.iter().zip(starts.iter()).enumerate() {
                        if i <= *gap_index {
                            continue; // layers before the gap stay put.
                        }
                        if let Some(layer) = mutable_page.find_mut(*id) {
                            layer.frame.pos = *start_pos + shift;
                        }
                    }
                }
                DragState::ResizingLine {
                    id,
                    handle,
                    parent_offset,
                } => {
                    let (doc_pos, lx, ly) =
                        snap_point(doc_pos, &snap_candidates, self.zoom, self.snap_enabled, self.show_grid);
                    draw_snap_lines(&canvas_painter, canvas_rect, origin, self.pan, self.zoom, lx, ly);
                    let local_pos = doc_pos - *parent_offset;
                    if let Some(layer) = history.mutate().active_page_mut().find_mut(*id) {
                        let rotation = layer.frame.rotation;
                        if *handle == Handle::TopLeft {
                            let end = layer.frame.end();
                            layer.frame = Frame::from_two_points(local_pos, end);
                        } else {
                            let start = layer.frame.start();
                            layer.frame = Frame::from_two_points(start, local_pos);
                        }
                        layer.frame.rotation = rotation;
                    }
                }
                DragState::ResizingGroup {
                    handle,
                    start_overall_bounds,
                    layers,
                    scale_style,
                    scale_anchor,
                } => {
                    let (doc_pos, lx, ly) =
                        snap_point(doc_pos, &snap_candidates, self.zoom, self.snap_enabled, self.show_grid);
                    draw_snap_lines(&canvas_painter, canvas_rect, origin, self.pan, self.zoom, lx, ly);
                    // A single rotated shape's resize handles are drawn (and
                    // hit-tested) at its *rotated* screen corners (see the
                    // selection-outline block above), but `start_overall_bounds`
                    // and the `handle.resize`/`transform` math below all
                    // operate in the shape's own unrotated local space. So
                    // the mouse position needs the inverse rotation applied
                    // first — inverse-rotating it back onto the unrotated
                    // rect's corner the user visually grabbed — before
                    // resizing; the result is a plain axis-aligned resize in
                    // local space (stays rectangular, doesn't shear), with
                    // `original_rotation` re-applied unchanged below. A
                    // multi-selection (mixed/no single rotation) is
                    // deliberately left as a plain page-space resize — same
                    // accepted-tradeoff category as a mixed Line multi-select
                    // losing exact direction fidelity (see `CLAUDE.md`).
                    let single_rotation = if let [only] = &layers[..] { only.original_rotation } else { 0.0 };
                    let doc_pos = if single_rotation == 0.0 {
                        doc_pos
                    } else {
                        rotate_point(doc_pos, start_overall_bounds.center(), -single_rotation)
                    };
                    let mut new_overall = handle.resize(*start_overall_bounds, doc_pos);
                    if *scale_anchor == ScaleAnchor::Center {
                        // Re-center on the *original* bounds' center instead
                        // of leaving the dragged handle's opposite
                        // corner/edge fixed — same new size either way, just
                        // a different anchor, so this is a plain override of
                        // where `new_overall` sits rather than a different
                        // resize computation.
                        new_overall = Rect::from_center_size(start_overall_bounds.center(), new_overall.size());
                    }
                    let old = *start_overall_bounds;
                    let scale = Vec2::new(
                        if old.width().abs() > 1e-6 {
                            new_overall.width() / old.width()
                        } else {
                            1.0
                        },
                        if old.height().abs() > 1e-6 {
                            new_overall.height() / old.height()
                        } else {
                            1.0
                        },
                    );
                    let old_anchor = old.min;
                    let new_anchor = new_overall.min;
                    let transform = |p: Pos2| -> Pos2 {
                        Pos2::new(
                            new_anchor.x + (p.x - old_anchor.x) * scale.x,
                            new_anchor.y + (p.y - old_anchor.y) * scale.y,
                        )
                    };
                    // Stroke width/corner radius aren't axis-specific, so
                    // Scale mode uses one uniform factor (the average of the
                    // two axis scales) rather than `scale.x`/`scale.y`
                    // separately.
                    let uniform_scale = (scale.x.abs() + scale.y.abs()) / 2.0;
                    apply_resize_delta(
                        history.mutate().active_page_mut(),
                        layers,
                        transform,
                        scale,
                        *scale_style,
                        uniform_scale,
                    );
                    // NOT a `refit_container_to_children` pass here, unlike
                    // `drag_stopped` below — every `ResizeLayerInfo` in
                    // `layers` carries a `parent_offset` captured once at
                    // drag start (see `collect_resizable_leaves`) and
                    // `apply_resize_delta` re-derives each leaf's position
                    // from it *every* frame. Refitting a selected
                    // container's own `frame.pos` mid-drag would move that
                    // baseline out from under still-live `ResizeLayerInfo`s,
                    // so each subsequent frame would compound the
                    // container's drift on top of the real resize — the
                    // selection stays (harmlessly) stale-boxed for the
                    // duration of the drag and only gets refit once, after
                    // it's released.
                }
                DragState::Rotating { pivot, start_angle, layers } => {
                    let mut delta_deg = ((doc_pos - *pivot).angle() - *start_angle).to_degrees();
                    if ui.input(|i| i.modifiers.shift) {
                        delta_deg = (delta_deg / 15.0).round() * 15.0;
                    }
                    apply_rotation_delta(history.mutate().active_page_mut(), *pivot, delta_deg, layers);
                    // See the matching comment in the `ResizingGroup` arm
                    // just above — same reason a live refit here would
                    // desync `RotateLayerInfo::parent_offset` mid-drag and
                    // make the container drift.
                }
                DragState::DrawingPenHandle { point_index } => {
                    if let Some(points) = &mut self.pen {
                        if let Some(pt) = points.get_mut(*point_index) {
                            let delta = doc_pos - pt.anchor;
                            pt.handle_out = Some(delta);
                            pt.handle_in = Some(-delta);
                        }
                    }
                }
                DragState::EditingPathAnchor {
                    layer_id,
                    point_indices,
                    start_doc,
                    start_anchors,
                } => {
                    let delta = doc_pos - *start_doc;
                    if let Some(layer) = history.mutate().active_page_mut().find_mut(*layer_id) {
                        if let LayerKind::Path { points, .. } = &mut layer.kind {
                            for (&index, &start_anchor) in point_indices.iter().zip(start_anchors.iter()) {
                                if let Some(pt) = points.get_mut(index) {
                                    pt.anchor = start_anchor + delta;
                                }
                            }
                        }
                    }
                }
                DragState::EditingPathHandle {
                    layer_id,
                    parent_offset,
                    point_index,
                    side,
                } => {
                    if let Some(layer) = history.mutate().active_page_mut().find_mut(*layer_id) {
                        let frame_pos = layer.frame.pos;
                        if let LayerKind::Path { points, .. } = &mut layer.kind {
                            if let Some(pt) = points.get_mut(*point_index) {
                                let anchor_abs = frame_pos + *parent_offset + pt.anchor.to_vec2();
                                let delta = doc_pos - anchor_abs;
                                match side {
                                    HandleSide::In => pt.handle_in = Some(delta),
                                    HandleSide::Out => pt.handle_out = Some(delta),
                                }
                                // Opposite-handle coupling depends on the
                                // point's type: `Mirror` keeps both handles
                                // an exact reflection of each other,
                                // `Asymmetric` keeps them at opposite angles
                                // but lets each side have its own length,
                                // and `Disconnected`/`Straight` leave the
                                // untouched side exactly as it was.
                                match pt.point_type {
                                    PointType::Mirror => {
                                        let mirrored = Some(-delta);
                                        match side {
                                            HandleSide::In => pt.handle_out = mirrored,
                                            HandleSide::Out => pt.handle_in = mirrored,
                                        }
                                    }
                                    PointType::Asymmetric => {
                                        let opposite_len = match side {
                                            HandleSide::In => pt.handle_out,
                                            HandleSide::Out => pt.handle_in,
                                        }
                                        .map(|h| h.length())
                                        .unwrap_or_else(|| delta.length());
                                        let new_opposite = -delta.normalized() * opposite_len;
                                        match side {
                                            HandleSide::In => pt.handle_out = Some(new_opposite),
                                            HandleSide::Out => pt.handle_in = Some(new_opposite),
                                        }
                                    }
                                    PointType::Disconnected | PointType::Straight => {}
                                }
                            }
                        }
                    }
                }
                DragState::CreatingGuide { orientation } => {
                    let value = snap_guide_axis(
                        *orientation,
                        doc_pos,
                        &snap_candidates,
                        self.zoom,
                        self.snap_enabled,
                        self.show_grid,
                    );
                    draw_guide_line(&canvas_painter, canvas_rect, origin, self.pan, self.zoom, *orientation, value);
                }
                DragState::MovingGuide { orientation, .. } => {
                    // Dragging back over a ruler (or past the canvas edge)
                    // previews as "no line" — released there, it deletes.
                    if canvas_rect.contains(mouse) {
                        let value = snap_guide_axis(
                            *orientation,
                            doc_pos,
                            &snap_candidates,
                            self.zoom,
                            self.snap_enabled,
                            self.show_grid,
                        );
                        draw_guide_line(&canvas_painter, canvas_rect, origin, self.pan, self.zoom, *orientation, value);
                    }
                }
                DragState::None => {}
            }
        }

        if response.drag_stopped() {
            match &self.drag {
                DragState::CreatingShape { start_doc } => {
                    let mouse = response
                        .interact_pointer_pos()
                        .unwrap_or(self.to_screen(origin, *start_doc));
                    let end_doc = self.to_doc(origin, mouse);
                    let frame = Frame::from_two_points(*start_doc, end_doc);
                    // A plain click (no meaningful drag) still places a Text
                    // layer at a default size — text is usually authored by
                    // clicking then typing, not by dragging out a box first.
                    if frame.size.x.abs() > 2.0 || frame.size.y.abs() > 2.0 || *tool == Tool::Text {
                        if let Some(new_layer) = new_layer_for_tool(*tool, frame) {
                            let new_id = new_layer.id;
                            let is_text = matches!(new_layer.kind, LayerKind::Text { .. });
                            let hint = self.insert_hint_parent.take();
                            insert_layer(history.mutate().active_page_mut(), new_layer, *start_doc, hint);
                            if is_text {
                                if let Some(l) = history.mutate().active_page_mut().find_mut(new_id) {
                                    apply_text_auto_resize(&ctx, l);
                                }
                            }
                            *selection = vec![new_id];
                            *tool = Tool::Select;
                        }
                    }
                }
                DragState::Marquee {
                    start_doc,
                    additive,
                    base_selection,
                    contained_only,
                    ignore_groups,
                    invert,
                } => {
                    let mouse = response
                        .interact_pointer_pos()
                        .unwrap_or(self.to_screen(origin, *start_doc));
                    let end_doc = self.to_doc(origin, mouse);
                    let marquee = Rect::from_two_pos(*start_doc, end_doc);
                    let mut hits = Vec::new();
                    collect_marquee_hits(
                        &history.get().active_page().layers,
                        Vec2::ZERO,
                        marquee,
                        *contained_only,
                        *ignore_groups,
                        &mut hits,
                    );
                    if *invert {
                        let mut merged = base_selection.clone();
                        for id in hits {
                            if let Some(pos) = merged.iter().position(|&s| s == id) {
                                merged.remove(pos);
                            } else {
                                merged.push(id);
                            }
                        }
                        *selection = merged;
                    } else if *additive {
                        let mut merged = base_selection.clone();
                        for id in hits {
                            if !merged.contains(&id) {
                                merged.push(id);
                            }
                        }
                        *selection = merged;
                    } else {
                        *selection = hits;
                    }
                }
                DragState::MarqueePoints {
                    layer_id,
                    start_doc,
                    additive,
                    base_selection,
                } => {
                    let mouse = response
                        .interact_pointer_pos()
                        .unwrap_or(self.to_screen(origin, *start_doc));
                    let end_doc = self.to_doc(origin, mouse);
                    let marquee = Rect::from_two_pos(*start_doc, end_doc);
                    if let Some((frame_pos, offset, points, _closed)) = read_path(history, *layer_id) {
                        let hits: Vec<usize> = points
                            .iter()
                            .enumerate()
                            .filter(|(_, p)| marquee.contains(p.anchor + frame_pos.to_vec2() + offset))
                            .map(|(i, _)| i)
                            .collect();
                        if *additive {
                            let mut merged = base_selection.clone();
                            for i in hits {
                                if !merged.contains(&i) {
                                    merged.push(i);
                                }
                            }
                            self.selected_points = merged;
                        } else {
                            self.selected_points = hits;
                        }
                    }
                }
                DragState::CreatingGuide { orientation } => {
                    let mouse = response.interact_pointer_pos().unwrap_or(origin);
                    // Dropped back on a ruler (or off the canvas): cancels
                    // guide creation rather than placing one.
                    if canvas_rect.contains(mouse) {
                        let doc_pos = self.to_doc(origin, mouse);
                        let candidates = build_snap_candidates(history.get().active_page(), &[]);
                        let value = snap_guide_axis(
                            *orientation,
                            doc_pos,
                            &candidates,
                            self.zoom,
                            self.snap_enabled,
                            self.show_grid,
                        );
                        history.snapshot();
                        history
                            .mutate()
                            .active_page_mut()
                            .guides
                            .push(Guide { orientation: *orientation, pos: value });
                    }
                }
                DragState::MovingGuide { index, orientation } => {
                    let mouse = response.interact_pointer_pos().unwrap_or(origin);
                    let new_value = if canvas_rect.contains(mouse) {
                        let doc_pos = self.to_doc(origin, mouse);
                        let candidates = build_snap_candidates(history.get().active_page(), &[]);
                        Some(snap_guide_axis(
                            *orientation,
                            doc_pos,
                            &candidates,
                            self.zoom,
                            self.snap_enabled,
                            self.show_grid,
                        ))
                    } else {
                        None
                    };
                    history.snapshot();
                    let guides = &mut history.mutate().active_page_mut().guides;
                    match new_value {
                        Some(v) => {
                            if let Some(g) = guides.get_mut(*index) {
                                g.pos = v;
                            }
                        }
                        None => {
                            if *index < guides.len() {
                                guides.remove(*index);
                            }
                        }
                    }
                }
                DragState::DrawingPenHandle { point_index } => {
                    // A drag too small to be an intentional handle pull (e.g. a
                    // slightly-jittery click) leaves a plain corner point instead
                    // of a near-zero-length handle.
                    if let Some(points) = &mut self.pen {
                        if let Some(pt) = points.get_mut(*point_index) {
                            let tiny = pt
                                .handle_out
                                .map(|v| v.length() * self.zoom < PEN_HANDLE_MIN_DRAG)
                                .unwrap_or(true);
                            if tiny {
                                pt.handle_in = None;
                                pt.handle_out = None;
                                pt.point_type = PointType::Straight;
                            }
                        }
                    }
                }
                DragState::EditingPathAnchor { layer_id, .. } => {
                    normalize_path_frame(history.mutate().active_page_mut(), *layer_id);
                }
                DragState::EditingPathHandle {
                    layer_id,
                    point_index,
                    side,
                    ..
                } => {
                    // Same near-zero-drag cleanup as `DrawingPenHandle`: dragging
                    // a handle back onto its anchor removes it, turning that side
                    // back into a straight corner.
                    if let Some(layer) = history.mutate().active_page_mut().find_mut(*layer_id) {
                        if let LayerKind::Path { points, .. } = &mut layer.kind {
                            if let Some(pt) = points.get_mut(*point_index) {
                                let target = match side {
                                    HandleSide::In => &mut pt.handle_in,
                                    HandleSide::Out => &mut pt.handle_out,
                                };
                                let tiny = target.map(|v| v.length() * self.zoom < PEN_HANDLE_MIN_DRAG).unwrap_or(false);
                                if tiny {
                                    *target = None;
                                }
                            }
                        }
                    }
                }
                DragState::ResizingGroup { .. } | DragState::Rotating { .. } => {
                    // The one-time counterpart to the comments in `dragged`'s
                    // `ResizingGroup`/`Rotating` arms: now that the gesture
                    // is over and no further frame will re-derive a leaf's
                    // position from its drag-start `parent_offset`, it's
                    // safe to bring every selected container's own `frame`
                    // back in sync with its (rotated/resized) children — see
                    // `refit_container_to_children`'s doc comment.
                    for &id in selection.iter() {
                        refit_container_to_children(history.mutate().active_page_mut(), id);
                    }
                }
                DragState::ImageEditDrag { start_doc, subtract, base_mask } => {
                    let (start_doc, subtract) = (*start_doc, *subtract);
                    let mut new_mask = base_mask.clone();
                    let mouse = response.interact_pointer_pos().unwrap_or(self.to_screen(origin, start_doc));
                    let end_doc = self.to_doc(origin, mouse);
                    if let Some((layer_id, bounds_doc, width, height)) = self.image_edit_doc_bounds(history) {
                        // A near-zero-movement release is a Magic Wand
                        // click; a real drag is a rectangular selection —
                        // same click-vs-drag threshold `CreatingShape` uses.
                        let tiny = (end_doc.x - start_doc.x).abs() <= 2.0 && (end_doc.y - start_doc.y).abs() <= 2.0;
                        if tiny {
                            if let Some((px, py)) = doc_to_pixel(start_doc, bounds_doc, width, height) {
                                if let Some(layer) = history.get().active_page().find(layer_id) {
                                    if let LayerKind::Image { encoded, .. } = &layer.kind {
                                        if let Some(decoded) = crate::image_ops::decode(encoded) {
                                            let tolerance = self.image_edit_tolerance();
                                            let wand = crate::image_ops::magic_wand_mask(&decoded, (px, py), tolerance);
                                            merge_mask(&mut new_mask, &wand, subtract);
                                        }
                                    }
                                }
                            }
                        } else {
                            let clipped = Rect::from_two_pos(start_doc, end_doc).intersect(bounds_doc);
                            if clipped.width() > 0.0 && clipped.height() > 0.0 {
                                let inset = Pos2::new(clipped.max.x - 0.001, clipped.max.y - 0.001);
                                if let (Some((x0, y0)), Some((x1, y1))) = (
                                    doc_to_pixel(clipped.min, bounds_doc, width, height),
                                    doc_to_pixel(inset, bounds_doc, width, height),
                                ) {
                                    let sel = rect_mask(width, height, x0, y0, x1, y1);
                                    merge_mask(&mut new_mask, &sel, subtract);
                                }
                            }
                        }
                    }
                    if let Some(edit) = &mut self.image_edit {
                        edit.mask = new_mask;
                        edit.mark_dirty();
                    }
                }
                _ => {}
            }
            self.drag = DragState::None;
        }

        if let Some(points) = &self.pen {
            draw_pen_preview(&painter, points, origin, self.pan, self.zoom, response.hover_pos());
        }

        // Rulers paint last (and unclipped, over the full widget) so their
        // strips stay on top of anything that scrolled near the canvas edge.
        if self.show_rulers {
            draw_rulers(&painter, full_rect, canvas_rect, origin, self.pan, self.zoom, response.hover_pos());
        }

        // Zoom with ctrl/cmd+scroll or pinch; plain scroll pans.
        if response.hovered() {
            let scroll = ui.input(|i| i.smooth_scroll_delta);
            let zoom_delta = ui.input(|i| i.zoom_delta());
            if zoom_delta != 1.0 {
                self.zoom = (self.zoom * zoom_delta).clamp(0.1, 8.0);
            } else if scroll != Vec2::ZERO {
                self.pan += scroll;
            }
        }

        // Right-click on an anchor of the active single Path: a point-type
        // picker (mouse alternative to the Num1-4 shortcuts), taking
        // priority over the Shift-overlap-picker/Copy-Paste menus below.
        // Shift+right-click (anywhere else): open a picker menu of every
        // layer stacked under the click point, instead of the normal single
        // topmost selection. Plain right-click (anywhere else): a small
        // Copy/Paste/Paste Over menu.
        if *tool == Tool::Select && response.secondary_clicked() {
            let anchor_hit = response
                .interact_pointer_pos()
                .and_then(|mp| self.hit_test_path_anchor(history, selection, origin, mp));
            if let Some((layer_id, index)) = anchor_hit {
                if !self.selected_points.contains(&index) {
                    self.selected_points = vec![index];
                }
                self.point_type_menu = response.interact_pointer_pos().map(|mp| (mp, layer_id));
                self.canvas_menu = None;
                self.overlap_menu = None;
            } else if ui.input(|i| i.modifiers.shift) {
                if let Some(mp) = response.interact_pointer_pos() {
                    let doc_pos = self.to_doc(origin, mp);
                    let mut hits = Vec::new();
                    layers_at_point(&history.get().active_page().layers, Vec2::ZERO, doc_pos, &mut hits);
                    self.overlap_menu = if hits.is_empty() { None } else { Some((mp, hits)) };
                }
                self.canvas_menu = None;
                self.point_type_menu = None;
            } else {
                self.canvas_menu = response.interact_pointer_pos();
                self.overlap_menu = None;
                self.point_type_menu = None;
            }
        }
        if let Some(screen_pos) = self.canvas_menu {
            let mut close = false;
            egui::Area::new(egui::Id::new("canvas-context-menu"))
                .fixed_pos(screen_pos)
                .order(egui::Order::Foreground)
                .show(ui.ctx(), |ui| {
                    egui::Frame::popup(ui.style()).show(ui, |ui| {
                        if ui.add_enabled(!selection.is_empty(), egui::Button::new("Copy")).clicked() {
                            clipboard::copy_selection(history, selection);
                            close = true;
                        }
                        if ui.button("Paste").clicked() {
                            clipboard::paste_selection(history, selection, self.duplicate_offset, false);
                            close = true;
                        }
                        if ui.button("Paste Over").clicked() {
                            clipboard::paste_selection(history, selection, self.duplicate_offset, true);
                            close = true;
                        }
                    });
                });
            if close || ui.input(|i| i.pointer.button_clicked(egui::PointerButton::Primary) || i.key_pressed(egui::Key::Escape))
            {
                self.canvas_menu = None;
            }
        }
        if let Some((screen_pos, ids)) = self.overlap_menu.clone() {
            let mut clicked_id = None;
            let names: Vec<String> = ids
                .iter()
                .map(|id| {
                    history
                        .get()
                        .active_page()
                        .find(*id)
                        .map(|l| l.name.clone())
                        .unwrap_or_else(|| "Layer".to_string())
                })
                .collect();
            egui::Area::new(egui::Id::new("overlap-menu"))
                .fixed_pos(screen_pos)
                .order(egui::Order::Foreground)
                .show(ui.ctx(), |ui| {
                    egui::Frame::popup(ui.style()).show(ui, |ui| {
                        for (id, name) in ids.iter().zip(names.iter()) {
                            if ui.selectable_label(false, name).clicked() {
                                clicked_id = Some(*id);
                            }
                        }
                    });
                });
            if let Some(id) = clicked_id {
                *selection = vec![id];
                self.overlap_menu = None;
            } else if ui.input(|i| i.pointer.button_clicked(egui::PointerButton::Primary) || i.key_pressed(egui::Key::Escape))
            {
                self.overlap_menu = None;
            }
        }
        // Right-click point-type picker (mouse alternative to `apply_point_type`'s
        // Num1-4 shortcuts): shown for the anchor that was right-clicked
        // (see the `secondary_clicked` dispatch above, which also makes sure
        // it's selected), applying to every point currently in
        // `selected_points`, same as the keyboard shortcuts.
        if let Some((screen_pos, layer_id)) = self.point_type_menu {
            let current_type = self.selected_points.first().and_then(|&index| {
                let doc = history.get();
                let LayerKind::Path { points, .. } = &doc.active_page().find(layer_id)?.kind else {
                    return None;
                };
                points.get(index).map(|p| p.point_type)
            });
            let mut chosen = None;
            egui::Area::new(egui::Id::new("point-type-menu"))
                .fixed_pos(screen_pos)
                .order(egui::Order::Foreground)
                .show(ui.ctx(), |ui| {
                    egui::Frame::popup(ui.style()).show(ui, |ui| {
                        for (label, point_type) in [
                            ("Straight", PointType::Straight),
                            ("Mirrored", PointType::Mirror),
                            ("Asymmetric", PointType::Asymmetric),
                            ("Disconnected", PointType::Disconnected),
                        ] {
                            if ui.selectable_label(current_type == Some(point_type), label).clicked() {
                                chosen = Some(point_type);
                            }
                        }
                    });
                });
            if let Some(point_type) = chosen {
                self.apply_point_type(history, layer_id, point_type);
            }
            if chosen.is_some()
                || ui.input(|i| i.pointer.button_clicked(egui::PointerButton::Primary) || i.key_pressed(egui::Key::Escape))
            {
                self.point_type_menu = None;
            }
        }

        // In-place canvas text editing: a floating `egui::Area` positioned
        // exactly over the layer being edited (`start_editing_text`), so it
        // reads as editing the glyphs directly rather than a side-panel
        // form. Placed last in `ui` so the earlier `history.get()`-derived
        // `doc`/`page` borrows have already ended by the time this needs
        // `history` mutably.
        if let Some(id) = self.editing_text {
            struct EditSnapshot {
                content: String,
                font_size: f32,
                font: TextFont,
                align: TextAlign,
                vertical_align: VerticalAlign,
                resize: TextResize,
                fill: Option<Color32>,
                bounds: Rect,
                letter_spacing: f32,
                line_height: Option<f32>,
                bold: bool,
                italic: bool,
                underline: bool,
                strikethrough: bool,
                runs: Vec<crate::model::text_runs::TextRun>,
            }
            let snapshot = {
                let doc = history.get();
                doc.active_page().find(id).zip(doc.active_page().absolute_offset(id)).and_then(
                    |(layer, offset)| {
                        if let LayerKind::Text {
                            content,
                            font_size,
                            font,
                            align,
                            vertical_align,
                            resize,
                            letter_spacing,
                            line_height,
                            bold,
                            italic,
                            underline,
                            strikethrough,
                            runs,
                            ..
                        } = &layer.kind
                        {
                            Some(EditSnapshot {
                                content: content.clone(),
                                font_size: *font_size,
                                font: font.clone(),
                                align: *align,
                                vertical_align: *vertical_align,
                                resize: *resize,
                                fill: layer.style.fill.as_ref().map(crate::model::Paint::to_color32),
                                bounds: layer.frame.bounds().translate(offset),
                                letter_spacing: *letter_spacing,
                                line_height: *line_height,
                                bold: *bold,
                                italic: *italic,
                                underline: *underline,
                                strikethrough: *strikethrough,
                                runs: runs.clone(),
                            })
                        } else {
                            None
                        }
                    },
                )
            };
            let Some(snap) = snapshot else {
                // Layer deleted, or an undo/redo landed on a state where it's
                // no longer a `Text` layer — just fall out of edit mode.
                self.editing_text = None;
                self.text_edit_selection = None;
                return;
            };
            let mut buf = snap.content.clone();
            let color = snap.fill.unwrap_or(Color32::BLACK);
            let bounds = snap.bounds;
            let zoom = self.zoom;
            let is_rich = !snap.runs.is_empty();

            let area_id = egui::Id::new(("text-edit-overlay", id));
            let text_edit_id = area_id.with("buf");
            // `Auto` grows without wrapping as you type (mirroring the common
            // "Auto Width"), so give it a generous width rather than the
            // current (possibly stale-until-next-frame) frame width.
            let width = match snap.resize {
                TextResize::Auto => 2000.0_f32.min(canvas_rect.width()),
                TextResize::AutoHeight | TextResize::Fixed => (bounds.width() * zoom).max(40.0),
            };

            let base = crate::model::text_runs::RunStyle {
                font: snap.font.clone(),
                font_size: snap.font_size,
                color: snap.fill,
                bold: snap.bold,
                italic: snap.italic,
                underline: snap.underline,
                strikethrough: snap.strikethrough,
            };
            let style_params = TextStyleParams {
                font: snap.font.clone(),
                font_size: snap.font_size,
                align: snap.align,
                letter_spacing: snap.letter_spacing,
                line_height: snap.line_height,
                italic: snap.italic,
                underline: snap.underline,
                strikethrough: snap.strikethrough,
                transform: crate::model::TextTransform::None,
                list: crate::model::ListType::None,
                list_start: 1,
            };

            // `TextEdit` always draws from the top of its box and has no
            // alignment concept of its own — `align` is handled inside
            // `build_edit_layout_job` (via `job.halign`), but `vertical_align`
            // needs the rendered content's height to know how far down to
            // shift the whole overlay, so it's measured once upfront here
            // (the same job is built again, deterministically, inside the
            // layouter below).
            let content_height = ui
                .ctx()
                .fonts_mut(|f| {
                    f.layout_job(build_edit_layout_job(&ctx, &buf, width, is_rich, &snap.runs, &base, &style_params, zoom, color))
                })
                .rect
                .height();
            let box_height = bounds.height() * zoom;
            let y_offset = match snap.vertical_align {
                VerticalAlign::Top => 0.0,
                VerticalAlign::Middle => ((box_height - content_height) * 0.5).max(0.0),
                VerticalAlign::Bottom => (box_height - content_height).max(0.0),
            };
            let screen_pos = self.to_screen(origin, bounds.min) + Vec2::new(0.0, y_offset);

            // One shared layouter for both uniform-style and rich-text
            // editing (`is_rich` picks between them inside
            // `build_edit_layout_job`) — egui uses the returned `Galley` for
            // both painting and all click/drag/keyboard selection, so this
            // is the entire mechanism that makes selecting-and-formatting
            // rich text work, same as before this shared alignment/vertical-
            // align support existed.
            let runs_for_layout = snap.runs.clone();
            let base_for_layout = base.clone();
            let layouter_ctx = ctx.clone();
            let mut layouter = move |_ui: &egui::Ui, buf: &dyn egui::TextBuffer, wrap_width: f32| {
                let job = build_edit_layout_job(
                    &layouter_ctx,
                    buf.as_str(),
                    wrap_width,
                    is_rich,
                    &runs_for_layout,
                    &base_for_layout,
                    &style_params,
                    zoom,
                    color,
                );
                layouter_ctx.fonts_mut(|f| f.layout_job(job))
            };
            let inner = egui::Area::new(area_id)
                .order(egui::Order::Foreground)
                .fixed_pos(screen_pos)
                .show(ui.ctx(), |ui| {
                    ui.set_width(width);
                    egui::TextEdit::multiline(&mut buf)
                        .id(text_edit_id)
                        .layouter(&mut layouter)
                        .frame(egui::Frame::NONE)
                        .desired_width(width)
                        .show(ui)
                })
                .inner;

            if self.editing_text_just_started {
                inner.response.response.request_focus();
                let range = egui::text::CCursorRange::two(
                    egui::text::CCursor::new(0),
                    egui::text::CCursor::new(buf.chars().count()),
                );
                let mut state = inner.state.clone();
                state.cursor.set_char_range(Some(range));
                state.store(ui.ctx(), text_edit_id);
                self.editing_text_just_started = false;
            }

            self.text_edit_selection = inner.cursor_range.and_then(|r| {
                let range = r.as_sorted_char_range();
                (range.start.0 < range.end.0).then_some(range.start.0..range.end.0)
            });

            if inner.response.response.changed() {
                if let Some(l) = history.mutate().active_page_mut().find_mut(id) {
                    if let LayerKind::Text { content, runs, .. } = &mut l.kind {
                        if !runs.is_empty() {
                            let (start, removed, inserted) = crate::model::text_runs::diff_chars(content, &buf);
                            crate::model::text_runs::splice(runs, start, removed, inserted, &base);
                        }
                        *content = buf;
                    }
                    apply_text_auto_resize(&ctx, l);
                }
            }

            let escape_pressed = ui.input(|i| i.key_pressed(egui::Key::Escape));
            if inner.response.response.lost_focus() || escape_pressed {
                self.editing_text = None;
                self.text_edit_selection = None;
            }
        }
    }
}

// --- "Edit Image" mode helpers (Selection/Magic Wand/Crop/Fill) ---

/// Maps a doc-space point into an edited image's own pixel space, clamped to
/// its bounds (rather than `None`) so a drag that ends slightly outside the
/// image's edge — very easy to do at typical zoom levels — still resolves to
/// the nearest edge pixel instead of silently doing nothing.
fn doc_to_pixel(doc_pos: Pos2, bounds_doc: Rect, width: u32, height: u32) -> Option<(u32, u32)> {
    if width == 0 || height == 0 || bounds_doc.width() <= 0.0 || bounds_doc.height() <= 0.0 {
        return None;
    }
    let rel = doc_pos - bounds_doc.min;
    let fx = (rel.x / bounds_doc.width()) * width as f32;
    let fy = (rel.y / bounds_doc.height()) * height as f32;
    let px = fx.floor().clamp(0.0, (width - 1) as f32) as u32;
    let py = fy.floor().clamp(0.0, (height - 1) as f32) as u32;
    Some((px, py))
}

/// Boolean mask (row-major `width * height`) of the axis-aligned pixel rect
/// spanning `(x0,y0)`..=`(x1,y1)` (inclusive on both ends, order-independent).
fn rect_mask(width: u32, height: u32, x0: u32, y0: u32, x1: u32, y1: u32) -> Vec<bool> {
    let (xlo, xhi) = (x0.min(x1), x0.max(x1));
    let (ylo, yhi) = (y0.min(y1), y0.max(y1));
    let mut mask = vec![false; (width as usize) * (height as usize)];
    for y in ylo..=yhi {
        for x in xlo..=xhi {
            mask[(y as usize) * (width as usize) + (x as usize)] = true;
        }
    }
    mask
}

/// OR's (or, if `subtract`, AND-NOT's) `delta` into `base` in place — the
/// shared merge step for both a Magic Wand click and a rectangular drag
/// selection, whichever `ImageEditDrag::subtract`/pre-seeded `base_mask`
/// says this gesture should do.
fn merge_mask(base: &mut [bool], delta: &[bool], subtract: bool) {
    for (b, &d) in base.iter_mut().zip(delta.iter()) {
        if subtract {
            if d {
                *b = false;
            }
        } else if d {
            *b = true;
        }
    }
}

/// Bounding box (inclusive, pixel coordinates) of every `true` cell in a
/// row-major `width`-wide mask. `None` if nothing is selected.
fn mask_bbox(mask: &[bool], width: u32) -> Option<(u32, u32, u32, u32)> {
    let mut x0 = u32::MAX;
    let mut y0 = u32::MAX;
    let mut x1 = 0u32;
    let mut y1 = 0u32;
    let mut any = false;
    for (i, &m) in mask.iter().enumerate() {
        if !m {
            continue;
        }
        any = true;
        let x = (i as u32) % width;
        let y = (i as u32) / width;
        x0 = x0.min(x);
        y0 = y0.min(y);
        x1 = x1.max(x);
        y1 = y1.max(y);
    }
    any.then_some((x0, y0, x1, y1))
}

/// Builds a translucent blue `ColorImage` the same size as `mask`, opaque-ish
/// where selected and fully transparent elsewhere — the pixel data behind
/// `ImageEditState::overlay`.
fn build_mask_overlay(mask: &[bool], width: u32, height: u32) -> egui::ColorImage {
    let selected = Color32::from_rgba_unmultiplied(0, 90, 158, 120);
    let pixels: Vec<Color32> = mask.iter().map(|&m| if m { selected } else { Color32::TRANSPARENT }).collect();
    egui::ColorImage::new([width as usize, height as usize], pixels)
}

/// True if `id` is a strict descendant of any layer already in `selection` —
/// see the call site in the click-to-drag handler above for why this needs
/// checking (`hit_test` always resolves to the innermost leaf, even when a
/// containing Group/Artboard is already the current selection).
fn hit_is_within_selection(page: &Page, selection: &[LayerId], id: LayerId) -> bool {
    selection
        .iter()
        .any(|&sel_id| sel_id != id && page.find(sel_id).is_some_and(|layer| layer.find(id).is_some()))
}

fn move_starts(page: &Page, ids: &[LayerId]) -> Vec<(LayerId, Pos2)> {
    ids.iter()
        .filter_map(|&id| page.find(id).map(|l| (id, l.frame.pos)))
        .collect()
}

// --- Rulers, guides, snapping, pixel grid ---

/// Doc-space x/y values a drag can snap to: guide positions plus the edges
/// and centers of every other (non-dragged) top-level layer on the page.
/// Built fresh once per drag frame from live document state.
struct SnapCandidates {
    xs: Vec<f32>,
    ys: Vec<f32>,
}

/// Ids that must be excluded from snap candidates because they're the ones
/// being moved/resized this drag (a layer shouldn't snap to its own edge).
fn drag_exclude_ids(drag: &DragState) -> Vec<LayerId> {
    match drag {
        DragState::MovingSelection { starts, .. } => starts.iter().map(|(id, _)| *id).collect(),
        DragState::ResizingLine { id, .. } => vec![*id],
        DragState::ResizingGroup { layers, .. } => layers.iter().map(|l| l.id).collect(),
        DragState::Rotating { layers, .. } => layers.iter().map(|l| l.id).collect(),
        _ => Vec::new(),
    }
}

/// Only top-level layers are considered, consistent with the marquee-select
/// simplification elsewhere (see module docs): nested descendants inside
/// artboards/groups don't contribute snap targets.
fn build_snap_candidates(page: &Page, exclude: &[LayerId]) -> SnapCandidates {
    let mut xs: Vec<f32> = page
        .guides
        .iter()
        .filter(|g| g.orientation == GuideOrientation::Vertical)
        .map(|g| g.pos)
        .collect();
    let mut ys: Vec<f32> = page
        .guides
        .iter()
        .filter(|g| g.orientation == GuideOrientation::Horizontal)
        .map(|g| g.pos)
        .collect();
    for layer in &page.layers {
        if !layer.visible || exclude.contains(&layer.id) {
            continue;
        }
        let b = layer.frame.bounds();
        xs.extend([b.min.x, b.center().x, b.max.x]);
        ys.extend([b.min.y, b.center().y, b.max.y]);
    }
    SnapCandidates { xs, ys }
}

/// Closest candidate to `value` within `threshold`, if any.
fn snap_value(value: f32, candidates: &[f32], threshold: f32) -> Option<f32> {
    candidates
        .iter()
        .copied()
        .map(|c| (c, (c - value).abs()))
        .filter(|&(_, d)| d <= threshold)
        .min_by(|a, b| a.1.partial_cmp(&b.1).unwrap())
        .map(|(c, _)| c)
}

/// Snaps a single dragged doc-space point (a shape corner, a line endpoint,
/// a resize handle) independently on each axis: first against guide/layer
/// candidates, falling back to the nearest whole pixel if `pixel_snap` is on
/// and nothing else matched. Returns the adjusted point plus whichever
/// candidate line matched on each axis (for smart-guide visual feedback);
/// the pixel-snap fallback never produces a feedback line, since it isn't
/// aligning to anything else on the page.
fn snap_point(
    p: Pos2,
    candidates: &SnapCandidates,
    zoom: f32,
    snap_enabled: bool,
    pixel_snap: bool,
) -> (Pos2, Option<f32>, Option<f32>) {
    if !snap_enabled {
        return (p, None, None);
    }
    let threshold = SNAP_THRESHOLD_SCREEN / zoom;
    let (x, line_x) = match snap_value(p.x, &candidates.xs, threshold) {
        Some(sx) => (sx, Some(sx)),
        None if pixel_snap => (p.x.round(), None),
        None => (p.x, None),
    };
    let (y, line_y) = match snap_value(p.y, &candidates.ys, threshold) {
        Some(sy) => (sy, Some(sy)),
        None if pixel_snap => (p.y.round(), None),
        None => (p.y, None),
    };
    (Pos2::new(x, y), line_x, line_y)
}

/// Snaps a translation applied to a moving selection's overall bounding box:
/// tries each of the box's left/center/right edges (and top/center/bottom)
/// against the candidates, keeping whichever needs the smallest correction,
/// so the whole selection can align by its center as well as its edges. As
/// with `snap_point`, falls back to whole-pixel snap of the box's min corner.
fn snap_bounds_delta(
    bounds: Rect,
    delta: Vec2,
    candidates: &SnapCandidates,
    zoom: f32,
    snap_enabled: bool,
    pixel_snap: bool,
) -> (Vec2, Option<f32>, Option<f32>) {
    if !snap_enabled {
        return (delta, None, None);
    }
    let threshold = SNAP_THRESHOLD_SCREEN / zoom;
    let moved = bounds.translate(delta);

    let mut best_x: Option<(f32, f32)> = None;
    for v in [moved.min.x, moved.center().x, moved.max.x] {
        if let Some(sv) = snap_value(v, &candidates.xs, threshold) {
            let corr = sv - v;
            if best_x.map(|(c, _)| corr.abs() < c.abs()).unwrap_or(true) {
                best_x = Some((corr, sv));
            }
        }
    }
    let mut best_y: Option<(f32, f32)> = None;
    for v in [moved.min.y, moved.center().y, moved.max.y] {
        if let Some(sv) = snap_value(v, &candidates.ys, threshold) {
            let corr = sv - v;
            if best_y.map(|(c, _)| corr.abs() < c.abs()).unwrap_or(true) {
                best_y = Some((corr, sv));
            }
        }
    }

    let mut new_delta = delta;
    let line_x = match best_x {
        Some((corr, sv)) => {
            new_delta.x += corr;
            Some(sv)
        }
        None if pixel_snap => {
            new_delta.x += moved.min.x.round() - moved.min.x;
            None
        }
        None => None,
    };
    let line_y = match best_y {
        Some((corr, sv)) => {
            new_delta.y += corr;
            Some(sv)
        }
        None if pixel_snap => {
            new_delta.y += moved.min.y.round() - moved.min.y;
            None
        }
        None => None,
    };
    (new_delta, line_x, line_y)
}

/// Snapped doc-space value for whichever axis a guide being dragged out (or
/// relocated) controls: `y` for a `Horizontal` guide, `x` for `Vertical`.
fn snap_guide_axis(
    orientation: GuideOrientation,
    doc_pos: Pos2,
    candidates: &SnapCandidates,
    zoom: f32,
    snap_enabled: bool,
    pixel_snap: bool,
) -> f32 {
    let raw = match orientation {
        GuideOrientation::Horizontal => doc_pos.y,
        GuideOrientation::Vertical => doc_pos.x,
    };
    if !snap_enabled {
        return raw;
    }
    let threshold = SNAP_THRESHOLD_SCREEN / zoom;
    let axis_candidates = match orientation {
        GuideOrientation::Horizontal => &candidates.ys,
        GuideOrientation::Vertical => &candidates.xs,
    };
    match snap_value(raw, axis_candidates, threshold) {
        Some(v) => v,
        None if pixel_snap => raw.round(),
        None => raw,
    }
}

/// Draws the transient pink "smart guide" alignment line(s) for whichever
/// axes matched a candidate this frame, spanning the full canvas area.
fn draw_snap_lines(
    painter: &egui::Painter,
    canvas_rect: Rect,
    origin: Pos2,
    pan: Vec2,
    zoom: f32,
    line_x: Option<f32>,
    line_y: Option<f32>,
) {
    let to_screen = |p: Pos2| origin + pan + p.to_vec2() * zoom;
    let stroke = EguiStroke::new(1.0, SNAP_LINE_COLOR);
    if let Some(x) = line_x {
        let sx = to_screen(Pos2::new(x, 0.0)).x;
        painter.line_segment(
            [Pos2::new(sx, canvas_rect.min.y), Pos2::new(sx, canvas_rect.max.y)],
            stroke,
        );
    }
    if let Some(y) = line_y {
        let sy = to_screen(Pos2::new(0.0, y)).y;
        painter.line_segment(
            [Pos2::new(canvas_rect.min.x, sy), Pos2::new(canvas_rect.max.x, sy)],
            stroke,
        );
    }
}

fn draw_dashed_line(painter: &egui::Painter, a: Pos2, b: Pos2, stroke: EguiStroke) {
    const DASH_LEN: f32 = 4.0;
    const GAP_LEN: f32 = 3.0;
    let total = a.distance(b);
    if total < 0.01 {
        return;
    }
    let dir = (b - a) / total;
    let mut t = 0.0;
    while t < total {
        let seg_end = (t + DASH_LEN).min(total);
        painter.line_segment([a + dir * t, a + dir * seg_end], stroke);
        t = seg_end + GAP_LEN;
    }
}

/// The gap `(near_edge, far_edge)` along one axis between two disjoint
/// intervals, or `None` if they overlap (nothing to measure on that axis).
fn axis_gap(a_min: f32, a_max: f32, b_min: f32, b_max: f32) -> Option<(f32, f32)> {
    if a_max <= b_min {
        Some((a_max, b_min))
    } else if b_max <= a_min {
        Some((b_max, a_min))
    } else {
        None
    }
}

/// Draws the Option-hover measurement overlay between `sel` (the
/// current selection's combined bounds) and `hover` (the layer under the
/// cursor), in doc space — a dashed line plus a pixel-distance label for
/// whichever axes the two rects don't already overlap on.
fn draw_distance_measurement(painter: &egui::Painter, to_screen: impl Fn(Pos2) -> Pos2, sel: Rect, hover: Rect) {
    let stroke = EguiStroke::new(1.0, MEASURE_COLOR);
    if let Some((x0, x1)) = axis_gap(sel.min.x, sel.max.x, hover.min.x, hover.max.x) {
        let overlap_y = sel.min.y.max(hover.min.y)..=sel.max.y.min(hover.max.y);
        let y = if overlap_y.start() <= overlap_y.end() {
            (overlap_y.start() + overlap_y.end()) / 2.0
        } else {
            (sel.center().y + hover.center().y) / 2.0
        };
        let a = to_screen(Pos2::new(x0, y));
        let b = to_screen(Pos2::new(x1, y));
        draw_dashed_line(painter, a, b, stroke);
        let mid = a.lerp(b, 0.5);
        painter.text(
            mid + Vec2::new(0.0, -8.0),
            egui::Align2::CENTER_BOTTOM,
            format!("{:.0}", x1 - x0),
            egui::FontId::proportional(11.0),
            MEASURE_COLOR,
        );
    }
    if let Some((y0, y1)) = axis_gap(sel.min.y, sel.max.y, hover.min.y, hover.max.y) {
        let overlap_x = sel.min.x.max(hover.min.x)..=sel.max.x.min(hover.max.x);
        let x = if overlap_x.start() <= overlap_x.end() {
            (overlap_x.start() + overlap_x.end()) / 2.0
        } else {
            (sel.center().x + hover.center().x) / 2.0
        };
        let a = to_screen(Pos2::new(x, y0));
        let b = to_screen(Pos2::new(x, y1));
        draw_dashed_line(painter, a, b, stroke);
        let mid = a.lerp(b, 0.5);
        painter.text(
            mid + Vec2::new(8.0, 0.0),
            egui::Align2::LEFT_CENTER,
            format!("{:.0}", y1 - y0),
            egui::FontId::proportional(11.0),
            MEASURE_COLOR,
        );
    }
}

/// Draws one placed/in-progress guide line at doc-space `value`, spanning
/// the canvas area. Shared by the persisted-guides pass and the live
/// creating/moving preview.
fn draw_guide_line(
    painter: &egui::Painter,
    canvas_rect: Rect,
    origin: Pos2,
    pan: Vec2,
    zoom: f32,
    orientation: GuideOrientation,
    value: f32,
) {
    let to_screen = |p: Pos2| origin + pan + p.to_vec2() * zoom;
    let stroke = EguiStroke::new(1.0, GUIDE_COLOR);
    match orientation {
        GuideOrientation::Horizontal => {
            let sy = to_screen(Pos2::new(0.0, value)).y;
            painter.line_segment(
                [Pos2::new(canvas_rect.min.x, sy), Pos2::new(canvas_rect.max.x, sy)],
                stroke,
            );
        }
        GuideOrientation::Vertical => {
            let sx = to_screen(Pos2::new(value, 0.0)).x;
            painter.line_segment(
                [Pos2::new(sx, canvas_rect.min.y), Pos2::new(sx, canvas_rect.max.y)],
                stroke,
            );
        }
    }
}

fn draw_guides(
    painter: &egui::Painter,
    canvas_rect: Rect,
    origin: Pos2,
    pan: Vec2,
    zoom: f32,
    guides: &[Guide],
) {
    for g in guides {
        draw_guide_line(painter, canvas_rect, origin, pan, zoom, g.orientation, g.pos);
    }
}

/// Draws a line at every visible integer doc-space pixel, once zoomed in
/// enough (see `PIXEL_GRID_MIN_ZOOM`) that individual pixels are actually
/// distinguishable rather than a dense wash.
fn draw_pixel_grid(painter: &egui::Painter, canvas_rect: Rect, origin: Pos2, pan: Vec2, zoom: f32) {
    if zoom < PIXEL_GRID_MIN_ZOOM {
        return;
    }
    let to_screen = |p: Pos2| origin + pan + p.to_vec2() * zoom;
    let to_doc = |p: Pos2| ((p - origin - pan) / zoom).to_pos2();
    let doc_min = to_doc(canvas_rect.min);
    let doc_max = to_doc(canvas_rect.max);
    let stroke = EguiStroke::new(1.0, PIXEL_GRID_COLOR);

    let x0 = doc_min.x.floor() as i64;
    let x1 = doc_max.x.ceil() as i64;
    for x in x0..=x1 {
        let sx = to_screen(Pos2::new(x as f32, 0.0)).x;
        painter.line_segment([Pos2::new(sx, canvas_rect.min.y), Pos2::new(sx, canvas_rect.max.y)], stroke);
    }
    let y0 = doc_min.y.floor() as i64;
    let y1 = doc_max.y.ceil() as i64;
    for y in y0..=y1 {
        let sy = to_screen(Pos2::new(0.0, y as f32)).y;
        painter.line_segment([Pos2::new(canvas_rect.min.x, sy), Pos2::new(canvas_rect.max.x, sy)], stroke);
    }
}

/// Picks a "nice" doc-space spacing (1/2/5 × a power of ten) between major
/// ruler ticks, targeting roughly `target_screen` screen pixels between them
/// at the given zoom.
fn nice_ruler_step(zoom: f32, target_screen: f32) -> f32 {
    let raw = (target_screen / zoom).max(1e-3);
    let magnitude = 10f32.powf(raw.log10().floor());
    let residual = raw / magnitude;
    let nice = if residual > 5.0 {
        10.0
    } else if residual > 2.0 {
        5.0
    } else if residual > 1.0 {
        2.0
    } else {
        1.0
    };
    nice * magnitude
}

/// Draws the top and left ruler strips (tick marks + doc-space coordinate
/// labels tracking the current pan/zoom), the corner box where they meet,
/// and a thin highlight tracking the pointer's current position.
fn draw_rulers(
    painter: &egui::Painter,
    full_rect: Rect,
    canvas_rect: Rect,
    origin: Pos2,
    pan: Vec2,
    zoom: f32,
    hover_pos: Option<Pos2>,
) {
    let to_screen = |p: Pos2| origin + pan + p.to_vec2() * zoom;
    let to_doc = |p: Pos2| ((p - origin - pan) / zoom).to_pos2();

    let top_rect = Rect::from_min_max(Pos2::new(canvas_rect.min.x, full_rect.min.y), Pos2::new(full_rect.max.x, canvas_rect.min.y));
    let left_rect = Rect::from_min_max(Pos2::new(full_rect.min.x, canvas_rect.min.y), Pos2::new(canvas_rect.min.x, full_rect.max.y));
    let corner_rect = Rect::from_min_max(full_rect.min, canvas_rect.min);

    painter.rect_filled(top_rect, 0.0, RULER_BG);
    painter.rect_filled(left_rect, 0.0, RULER_BG);
    painter.rect_filled(corner_rect, 0.0, RULER_BG);
    painter.line_segment([top_rect.left_bottom(), top_rect.right_bottom()], EguiStroke::new(1.0, RULER_LINE));
    painter.line_segment([left_rect.right_top(), left_rect.right_bottom()], EguiStroke::new(1.0, RULER_LINE));

    let step = nice_ruler_step(zoom, 70.0);
    let minor_step = step / 5.0;
    let font = egui::FontId::proportional(9.0);

    let doc_min = to_doc(canvas_rect.min);
    let doc_max = to_doc(canvas_rect.max);

    // Horizontal (top) ruler: ticks + labels along x.
    {
        let first = (doc_min.x / minor_step).floor() as i64;
        let last = (doc_max.x / minor_step).ceil() as i64;
        for i in first..=last {
            let x = i as f32 * minor_step;
            let sx = to_screen(Pos2::new(x, 0.0)).x;
            if sx < top_rect.min.x || sx > top_rect.max.x {
                continue;
            }
            let nearest_major = (x / step).round() * step;
            let major = (x - nearest_major).abs() < minor_step * 0.5;
            let tick_top = if major { top_rect.max.y - 9.0 } else { top_rect.max.y - 5.0 };
            painter.line_segment(
                [Pos2::new(sx, tick_top), Pos2::new(sx, top_rect.max.y)],
                EguiStroke::new(1.0, RULER_LINE),
            );
            if major {
                let galley = painter.layout_no_wrap(format!("{x:.0}"), font.clone(), RULER_TEXT);
                painter.galley(Pos2::new(sx + 2.0, top_rect.min.y + 1.0), galley, RULER_TEXT);
            }
        }
    }

    // Vertical (left) ruler: ticks + rotated labels along y.
    {
        let first = (doc_min.y / minor_step).floor() as i64;
        let last = (doc_max.y / minor_step).ceil() as i64;
        for i in first..=last {
            let y = i as f32 * minor_step;
            let sy = to_screen(Pos2::new(0.0, y)).y;
            if sy < left_rect.min.y || sy > left_rect.max.y {
                continue;
            }
            let nearest_major = (y / step).round() * step;
            let major = (y - nearest_major).abs() < minor_step * 0.5;
            let tick_left = if major { left_rect.max.x - 9.0 } else { left_rect.max.x - 5.0 };
            painter.line_segment(
                [Pos2::new(tick_left, sy), Pos2::new(left_rect.max.x, sy)],
                EguiStroke::new(1.0, RULER_LINE),
            );
            if major {
                let galley = painter.layout_no_wrap(format!("{y:.0}"), font.clone(), RULER_TEXT);
                painter.add(egui::Shape::Text(egui::epaint::TextShape {
                    angle: -std::f32::consts::FRAC_PI_2,
                    ..egui::epaint::TextShape::new(Pos2::new(left_rect.min.x + 1.0, sy + 2.0), galley, RULER_TEXT)
                }));
            }
        }
    }

    // A thin crosshair highlight on both rulers tracking the pointer, like
    // most vector tools' ruler readouts.
    if let Some(mp) = hover_pos {
        if canvas_rect.contains(mp) || top_rect.contains(mp) || left_rect.contains(mp) {
            painter.line_segment(
                [Pos2::new(mp.x, top_rect.min.y), Pos2::new(mp.x, top_rect.max.y)],
                EguiStroke::new(1.0, SELECTION_COLOR),
            );
            painter.line_segment(
                [Pos2::new(left_rect.min.x, mp.y), Pos2::new(left_rect.max.x, mp.y)],
                EguiStroke::new(1.0, SELECTION_COLOR),
            );
        }
    }
}

/// Reads out a `Path` layer's frame position, ancestor offset, and points
/// (cloned) in one short-lived immutable borrow of `history`, so callers can
/// use the result across a later `history.mutate()` call without holding
/// onto a reference into the document.
fn read_path(history: &History, layer_id: LayerId) -> Option<(Pos2, Vec2, Vec<PathPoint>, bool)> {
    let doc = history.get();
    let page = doc.active_page();
    let layer = page.find(layer_id)?;
    let offset = page.absolute_offset(layer_id)?;
    if let LayerKind::Path { points, closed } = &layer.kind {
        Some((layer.frame.pos, offset, points.clone(), *closed))
    } else {
        None
    }
}

/// Default bezier `handle_out` for turning anchor `index` into a curve point
/// from scratch: a chord-parallel tangent running from the previous anchor
/// straight through to the next one — a simple, generally-smooth default; an
/// open path's own endpoint (only one neighbor) instead points along that
/// single adjacent segment. Length is 0.35 of whichever neighbor is closer,
/// clamped to `[4.0, 40.0]`. `None` if `index` has no usable neighbor (an
/// open path with a single point) or the neighbors are coincident with it.
/// Shared by `try_convert_anchor_to_curve`'s double-click and
/// `apply_point_type`'s keyboard Mirror shortcut, so a point gets the same
/// default curve either way.
fn default_curve_handle_out(points: &[PathPoint], index: usize, closed: bool) -> Option<Vec2> {
    let n = points.len();
    let prev = if index == 0 { closed.then(|| n - 1) } else { Some(index - 1) };
    let next = if index == n - 1 { closed.then_some(0) } else { Some(index + 1) };
    let anchor = points[index].anchor;
    let tangent_vec = match (prev, next) {
        (Some(p), Some(nx)) => points[nx].anchor - points[p].anchor,
        (Some(p), None) => anchor - points[p].anchor,
        (None, Some(nx)) => points[nx].anchor - anchor,
        (None, None) => return None,
    };
    if tangent_vec.length() < 1e-3 {
        return None;
    }
    let tangent = tangent_vec.normalized();
    let len_prev = prev.map(|p| (points[p].anchor - anchor).length()).unwrap_or(f32::INFINITY);
    let len_next = next.map(|nx| (points[nx].anchor - anchor).length()).unwrap_or(f32::INFINITY);
    let handle_len = (len_prev.min(len_next) * 0.35).clamp(4.0, 40.0);
    Some(tangent * handle_len)
}

fn distance_to_segment(p: Pos2, a: Pos2, b: Pos2) -> f32 {
    let ab = b - a;
    let len2 = ab.length_sq();
    if len2 < 1e-6 {
        return p.distance(a);
    }
    let t = ((p - a).dot(ab) / len2).clamp(0.0, 1.0);
    p.distance(a + ab * t)
}

/// Recomputes a `Path` layer's `frame` as the tight bounding box of its
/// anchors (handles aren't included, matching `finish_pen_path`), shifting
/// every point's relative anchor so their absolute positions don't move.
/// Anchor edits update `frame.pos`/`points` independently mid-drag (see
/// `DragState::EditingPathAnchor`), so this restores the invariant that
/// `frame` is the anchors' bounding box once the gesture ends.
fn normalize_path_frame(page: &mut Page, layer_id: LayerId) {
    let Some(layer) = page.find_mut(layer_id) else { return };
    let old_frame_pos = layer.frame.pos;
    let rotation = layer.frame.rotation;
    let LayerKind::Path { points, .. } = &mut layer.kind else { return };
    if points.is_empty() {
        return;
    }
    let bounds = points
        .iter()
        .map(|p| Rect::from_pos(old_frame_pos + p.anchor.to_vec2()))
        .reduce(|a, b| a.union(b))
        .unwrap();
    for pt in points.iter_mut() {
        let abs = old_frame_pos + pt.anchor.to_vec2();
        pt.anchor = abs - (bounds.min.to_vec2());
    }
    layer.frame = Frame {
        pos: bounds.min,
        size: bounds.size(),
        rotation,
    };
}

/// Builds a new open `Path` layer from `points` (anchors relative to
/// `old_frame_pos`, the source layer's `frame.pos` — same convention
/// `try_scissor_path` reads them in), recomputing a tight bounding-box frame
/// the same way `finish_pen_path` does. Used to materialize the two pieces a
/// Scissors cut splits an open path into. `rotation` carries over the source
/// layer's rotation unchanged; since the new frame's bounds (and therefore
/// rotation pivot) generally differ from the original, a cut piece of a
/// rotated path can shift slightly relative to where it visually sat before
/// the cut — an accepted approximation, not exact-pivot-preserving.
fn build_path_layer(name: &str, style: &Style, old_frame_pos: Pos2, rotation: f32, points: &[PathPoint]) -> Layer {
    let absolute: Vec<PathPoint> = points
        .iter()
        .map(|p| PathPoint {
            anchor: p.anchor + old_frame_pos.to_vec2(),
            handle_in: p.handle_in,
            handle_out: p.handle_out,
            point_type: p.point_type,
            corner_radius: p.corner_radius,
        })
        .collect();
    let bounds = absolute
        .iter()
        .map(|p| Rect::from_pos(p.anchor))
        .reduce(|a, b| a.union(b))
        .unwrap();
    let relative: Vec<PathPoint> = absolute
        .iter()
        .map(|p| PathPoint {
            anchor: p.anchor - bounds.min.to_vec2(),
            handle_in: p.handle_in,
            handle_out: p.handle_out,
            point_type: p.point_type,
            corner_radius: p.corner_radius,
        })
        .collect();
    let frame = Frame {
        pos: bounds.min,
        size: bounds.size(),
        rotation,
    };
    let mut layer = Layer::new(name, frame, LayerKind::Path { points: relative, closed: false });
    layer.style = style.clone();
    layer
}

/// Default size for a `Text` layer created with a plain click (no drag) of
/// the Text tool, roughly matching one line of the default font size.
const DEFAULT_TEXT_SIZE: Vec2 = Vec2::new(140.0, 34.0);

/// Builds the `LayoutJob` the in-place text-editing overlay (in `ui()`)
/// hands to `egui::TextEdit`'s `.layouter()` — shared by both the uniform-
/// style and rich-text editing paths (`is_rich` selects between them), and
/// called twice per frame: once upfront just to measure the resulting
/// `Galley`'s height (for `vertical_align`, which egui's `TextEdit` has no
/// native concept of), then again inside the layouter callback itself. A
/// plain function rather than a closure so neither call site has to worry
/// about capturing/cloning the same handful of values twice.
#[allow(clippy::too_many_arguments)]
fn build_edit_layout_job(
    ctx: &egui::Context,
    text: &str,
    wrap_width: f32,
    is_rich: bool,
    runs: &[crate::model::text_runs::TextRun],
    base: &crate::model::text_runs::RunStyle,
    style: &TextStyleParams,
    zoom: f32,
    color: Color32,
) -> egui::text::LayoutJob {
    let mut job = if is_rich {
        text_layout::editor_layout_job(ctx, text, runs, base, style.align, style.letter_spacing, style.line_height, zoom, color)
    } else {
        text_layout::plain_editor_layout_job(ctx, text, style, zoom, color)
    };
    job.wrap.max_width = wrap_width;
    job
}

/// Recomputes a `Text` layer's `frame.size` from its current content/style,
/// for `resize != Fixed` — `Auto` recomputes both width and height (no
/// wrapping); `AutoHeight` wraps at the current width and only recomputes
/// height. A no-op for `Fixed`. Called after every edit that could change a
/// `Text` layer's natural size: creation, Inspector field changes, in-place
/// canvas-edit keystrokes, and shared Text Style application.
pub(crate) fn apply_text_auto_resize(ctx: &egui::Context, layer: &mut Layer) {
    let LayerKind::Text {
        content,
        font_size,
        font,
        align,
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
        ..
    } = &layer.kind
    else {
        return;
    };
    if *resize == TextResize::Fixed {
        return;
    }
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
    let wrap_width = if *resize == TextResize::AutoHeight {
        layer.frame.size.x.abs()
    } else {
        f32::INFINITY
    };
    let is_auto = *resize == TextResize::Auto;
    // Color never affects glyph metrics, so `Color32::BLACK` is just a
    // placeholder here, same as the uniform-style path below.
    let galleys = if runs.is_empty() {
        text_layout::layout_paragraphs(ctx, content, &style, 1.0, Color32::BLACK, wrap_width)
    } else {
        let base_style = crate::model::text_runs::RunStyle {
            font: font.clone(),
            font_size: *font_size,
            color: layer.style.fill.as_ref().map(crate::model::Paint::to_color32),
            bold: *bold,
            italic: *italic,
            underline: *underline,
            strikethrough: *strikethrough,
        };
        text_layout::layout_paragraphs_rich(ctx, content, runs, &base_style, &style, 1.0, Color32::BLACK, wrap_width)
    };
    let (_, total_size) = text_layout::stack_paragraphs(&galleys, *paragraph_spacing);

    layer.frame.size.y = total_size.y.max(1.0);
    if is_auto {
        layer.frame.size.x = total_size.x.max(1.0);
    }
}

fn new_layer_for_tool(tool: Tool, frame: Frame) -> Option<Layer> {
    match tool {
        Tool::Rectangle => Some(Layer::new(
            "Rectangle",
            Frame::from_bounds(frame.bounds()),
            LayerKind::Rectangle { corner_radius: CornerRadii::ZERO },
        )),
        Tool::Oval => Some(Layer::new(
            "Oval",
            Frame::from_bounds(frame.bounds()),
            LayerKind::Oval,
        )),
        Tool::Line => Some(Layer::new("Line", frame, LayerKind::Line)),
        Tool::Arrow => Some(Layer::new(
            "Arrow",
            frame,
            LayerKind::Arrow { start_cap: ArrowCap::None, end_cap: ArrowCap::Triangle },
        )),
        Tool::Star => Some(Layer::new(
            "Star",
            Frame::from_bounds(frame.bounds()),
            LayerKind::Star { points: 5, inner_ratio: 0.5 },
        )),
        Tool::Polygon => Some(Layer::new(
            "Polygon",
            Frame::from_bounds(frame.bounds()),
            LayerKind::Polygon { sides: 6 },
        )),
        Tool::Artboard => Some(Layer::new_artboard(
            "Artboard",
            Frame::from_bounds(frame.bounds()),
        )),
        Tool::Text => {
            let bounds = frame.bounds();
            // A meaningful drag creates a fixed-size wrapping box (a
            // "click and drag" interaction); a plain click creates an auto-sizing layer
            // ("click anywhere") — `apply_text_auto_resize` (called
            // right after insertion, see the `drag_stopped` handler) fits
            // `DEFAULT_TEXT_SIZE` to the actual default content in that case.
            let dragged = bounds.width().abs() > 2.0 || bounds.height().abs() > 2.0;
            let (size, resize) = if dragged {
                (bounds.size(), TextResize::Fixed)
            } else {
                (DEFAULT_TEXT_SIZE, TextResize::Auto)
            };
            let mut layer = Layer::new(
                "Text",
                Frame { pos: bounds.min, size, rotation: 0.0 },
                LayerKind::Text {
                    content: "Text".to_string(),
                    font_size: 24.0,
                    font: TextFont::Proportional,
                    align: TextAlign::Left,
                    vertical_align: VerticalAlign::Top,
                    resize,
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
                },
            );
            layer.style =
                crate::model::Style { fill: Some(crate::model::Paint::Solid(Color32::BLACK)), stroke: None, ..Default::default() };
            Some(layer)
        }
        // Pen-drawn paths are built up over several click/drag gestures
        // (see `CanvasWidget::pen`) and committed via `finish_pen_path`,
        // not through the single-drag `CreatingShape` flow other shapes use.
        // Scissors doesn't create a new layer by dragging either — see
        // `CanvasWidget::try_scissor_path`.
        Tool::Select | Tool::Pan | Tool::Pen | Tool::Scissors => None,
    }
}

/// Inserts a newly created layer at the top level of the page, unless
/// `start_doc` falls within an existing top-level artboard, in which case
/// it's nested as a child (with frame made relative to that artboard).
/// Inserts a freshly-created shape/text layer, choosing its parent. `hint`
/// ("a new layer lands in the group a just-deleted layer's delete
/// left behind" — see `App`'s Delete/Backspace handler, the only place that
/// sets `CanvasWidget::insert_hint_parent`) takes priority when present and
/// still a real container, regardless of `start_doc`'s position; otherwise
/// falls back to the original position-based rule (inside the top-level
/// Artboard, if any, that `start_doc` falls within).
fn insert_layer(page: &mut Page, mut layer: Layer, start_doc: Pos2, hint: Option<LayerId>) {
    if let Some(hint_id) = hint {
        let hint_abs_pos = page
            .absolute_offset(hint_id)
            .zip(page.find(hint_id))
            .map(|(ancestor_offset, l)| l.frame.pos.to_vec2() + ancestor_offset);
        if let Some(hint_abs_pos) = hint_abs_pos {
            if let Some(children) = page.find_mut(hint_id).and_then(|l| l.kind.children_mut()) {
                layer.frame.pos -= hint_abs_pos;
                children.push(layer);
                return;
            }
        }
    }
    for parent in page.layers.iter_mut() {
        if let LayerKind::Artboard { children, .. } = &mut parent.kind {
            if parent.frame.bounds().contains(start_doc) {
                layer.frame.pos -= parent.frame.pos.to_vec2();
                children.push(layer);
                return;
            }
        }
    }
    page.layers.push(layer);
}

/// The layers in `ids` that still exist, sorted along `axis` by absolute
/// bounds — the same ordering `alignment::distribute` itself sorts by, so
/// the gap handles line up with the actual gaps it produced.
fn distribution_order(page: &Page, ids: &[LayerId], axis: DistributeAxis) -> Vec<(LayerId, Rect)> {
    let mut items: Vec<(LayerId, Rect)> = ids
        .iter()
        .filter_map(|&id| {
            let layer = page.find(id)?;
            let offset = page.absolute_offset(id)?;
            Some((id, layer.frame.rotated_bounds().translate(offset)))
        })
        .collect();
    items.sort_by(|a, b| {
        let key = |r: &Rect| match axis {
            DistributeAxis::Horizontal => r.min.x,
            DistributeAxis::Vertical => r.min.y,
        };
        key(&a.1).partial_cmp(&key(&b.1)).unwrap()
    });
    items
}

/// Doc-space midpoint of the gap between each consecutive pair in
/// `distribution_order`'s output — one fewer than the item count.
fn gap_handle_positions(items: &[(LayerId, Rect)], axis: DistributeAxis) -> Vec<Pos2> {
    items
        .windows(2)
        .map(|w| {
            let ra = w[0].1;
            let rb = w[1].1;
            match axis {
                DistributeAxis::Horizontal => Pos2::new((ra.max.x + rb.min.x) / 2.0, (ra.center().y + rb.center().y) / 2.0),
                DistributeAxis::Vertical => Pos2::new((ra.center().x + rb.center().x) / 2.0, (ra.max.y + rb.min.y) / 2.0),
            }
        })
        .collect()
}

fn hit_test(page: &Page, doc_pos: Pos2) -> Option<LayerId> {
    hit_test_layers(&page.layers, doc_pos)
}

/// Every layer under `doc_pos`, front-to-back, for the Shift+right-click
/// "pick among overlapping layers" menu. Bounding-box based (via
/// `rotated_bounds()`) rather than `hit_test_layers`'s pixel-accurate/
/// mask-aware test — good enough for a picker menu, and much simpler than
/// duplicating that function's boolean-group/mask logic.
fn layers_at_point(layers: &[Layer], offset: Vec2, doc_pos: Pos2, out: &mut Vec<LayerId>) {
    for layer in layers.iter().rev() {
        if !layer.visible || layer.locked {
            continue;
        }
        if layer.frame.rotated_bounds().translate(offset).contains(doc_pos) {
            out.push(layer.id);
        }
        if let Some(children) = layer.kind.children() {
            layers_at_point(children, offset + layer.frame.pos.to_vec2(), doc_pos, out);
        }
    }
}

/// Collects every layer overlapping (or, if `contained_only`, fully inside)
/// `marquee` (in page/doc space) into `out`. With `ignore_groups` false
/// (the default, top-level-only rubber-band select), only `layers` itself is
/// tested — a `Group`/`Artboard`/`BooleanGroup` is matched as one unit, same
/// as today. With `ignore_groups` true (Command held), containers are never
/// matched themselves — only their descendant leaves are, recursively, per
/// "ignore groups and select only contained layers". `offset` is the
/// accumulated ancestor position (see the module doc's coordinate-system
/// convention); `rotated_bounds()` is used throughout so a rotated leaf's
/// actual visual footprint is what's tested, not its unrotated local bounds.
fn collect_marquee_hits(
    layers: &[Layer],
    offset: Vec2,
    marquee: Rect,
    contained_only: bool,
    ignore_groups: bool,
    out: &mut Vec<LayerId>,
) {
    for layer in layers {
        if !layer.visible || layer.locked {
            continue;
        }
        let has_children = layer.kind.children().is_some();
        if ignore_groups && has_children {
            let child_offset = offset + layer.frame.pos.to_vec2();
            collect_marquee_hits(
                layer.kind.children().unwrap(),
                child_offset,
                marquee,
                contained_only,
                ignore_groups,
                out,
            );
            continue;
        }
        let abs_bounds = layer.frame.rotated_bounds().translate(offset);
        let matches = if contained_only {
            marquee.contains_rect(abs_bounds)
        } else {
            marquee.intersects(abs_bounds)
        };
        if matches {
            out.push(layer.id);
        }
    }
}

fn hit_test_layers(layers: &[Layer], doc_pos: Pos2) -> Option<LayerId> {
    // Alpha-accurate: a content layer under an active mask (see
    // `masking::partition_mask_runs`) only registers a hit where the mask
    // actually reveals it, not just anywhere in the content layer's own
    // bounding box — e.g. a layer that extends beyond its mask's bounds is
    // no longer clickable in the clipped-away part. The mask layer *itself*
    // keeps the normal plain bounding-box test (unchanged) — it's still a
    // real, selectable layer sitting on top like any other, this only
    // changes what's reachable *through* it. Built fresh per call (only
    // ever invoked on an actual click, not per frame) rather than threaded
    // through from the caller, so this stays a self-contained,
    // easy-to-reason-about function like it was before masking existed.
    let mut mask_for: HashMap<LayerId, &Layer> = HashMap::new();
    for unit in crate::masking::partition_mask_runs(layers) {
        if let crate::masking::RenderUnit::Masked { mask, content } = unit {
            for layer in content {
                mask_for.insert(layer.id, mask);
            }
        }
    }

    for layer in layers.iter().rev() {
        if !layer.visible || layer.locked {
            continue;
        }

        // Opaque, unlike Group/Artboard just below (which fall through to
        // the generic bounds/children handling): a click selects the
        // BooleanGroup itself as one unit — members are only reachable via
        // the layers panel (see `LayerKind::BooleanGroup`'s doc comment).
        // Tested against the live-computed geometry exactly (`geo::Contains`)
        // rather than a rasterize-and-sample approach, since the combined
        // shape is already a clean `MultiPolygon` (contrast with
        // `masking::mask_covers_point`, whose rasterize approach exists
        // specifically because a mask's coverage is arbitrary alpha, not a
        // clean polygon). No rotation handling needed here — a
        // `BooleanGroup`'s own `frame.rotation` is always `0.0`, same
        // convention as `Group`/`Artboard`.
        if let LayerKind::BooleanGroup { children } = &layer.kind {
            let local_pos = doc_pos - layer.frame.pos.to_vec2();
            let combined = crate::boolean_ops::compute_boolean_group(children);
            if !crate::boolean_ops::point_in_multipolygon(&combined, local_pos) {
                continue;
            }
            if let Some(mask) = mask_for.get(&layer.id) {
                if !crate::masking::mask_covers_point(mask, doc_pos) {
                    continue;
                }
            }
            return Some(layer.id);
        }

        let bounds = layer.frame.bounds();
        // Only leaf shapes ever have nonzero rotation (a Group/Artboard's
        // own rotation is always 0.0 — rotating one bakes the angle into
        // each descendant's own frame instead, see `model/layer.rs`'s
        // `Frame::rotation` doc comment), so inverse-rotating `doc_pos` into
        // the layer's own unrotated local space here is enough — no matching
        // change is needed for the `children()` recursion below.
        let test_pos = rotate_point(doc_pos, bounds.center(), -layer.frame.rotation);
        if !bounds.contains(test_pos) {
            continue;
        }
        if let Some(mask) = mask_for.get(&layer.id) {
            if !crate::masking::mask_covers_point(mask, doc_pos) {
                continue;
            }
        }
        if let Some(children) = layer.kind.children() {
            let local_pos = doc_pos - layer.frame.pos.to_vec2();
            if let Some(hit) = hit_test_layers(children, local_pos) {
                return Some(hit);
            }
        }
        return Some(layer.id);
    }
    None
}

/// Draws one parent's `children` (`Page::layers`, or an `Artboard`/`Group`'s
/// own children, already shifted to that parent's coordinate space),
/// applying `Layer::is_mask`/`ignore_mask` via
/// `masking::partition_mask_runs` — see that function's doc comment for the
/// masked-run semantics, and `MaskedGroupTextureCache` for how a masked run
/// actually gets drawn (a cached rasterized texture, since egui's immediate-
/// mode painter can't clip to an arbitrary shape).
#[allow(clippy::too_many_arguments)]
fn draw_children(
    painter: &egui::Painter,
    ctx: &egui::Context,
    image_cache: &mut ImageTextureCache,
    mask_cache: &mut MaskedGroupTextureCache,
    noise_cache: &mut NoiseTextureCache,
    halftone_cache: &mut HalftoneTextureCache,
    pattern_cache: &mut PatternTextureCache,
    shadow_cache: &mut ShadowTextureCache,
    children: &[Layer],
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
                draw_layer(painter, ctx, image_cache, mask_cache, noise_cache, halftone_cache, pattern_cache, shadow_cache, child, parent_offset, origin, pan, zoom, opacity, editing_text);
            }
            crate::masking::RenderUnit::Masked { mask, content } => {
                draw_masked_run(painter, ctx, mask_cache, mask, &content, parent_offset, origin, pan, zoom, opacity);
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
fn draw_masked_run(
    painter: &egui::Painter,
    ctx: &egui::Context,
    mask_cache: &mut MaskedGroupTextureCache,
    mask: &Layer,
    content: &[&Layer],
    parent_offset: Vec2,
    origin: Pos2,
    pan: Vec2,
    zoom: f32,
    opacity: f32,
) {
    let Some((texture, bounds)) = mask_cache.get_or_build(ctx, mask, content) else {
        return;
    };
    let to_screen = |p: Pos2| origin + pan + (p.to_vec2() + parent_offset) * zoom;
    let screen_rect = Rect::from_two_pos(to_screen(bounds.min), to_screen(bounds.max));
    let tint = with_opacity(Color32::WHITE, opacity);
    painter.image(texture.id(), screen_rect, Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)), tint);
}

#[allow(clippy::too_many_arguments)]
fn draw_layer(
    painter: &egui::Painter,
    ctx: &egui::Context,
    image_cache: &mut ImageTextureCache,
    mask_cache: &mut MaskedGroupTextureCache,
    noise_cache: &mut NoiseTextureCache,
    halftone_cache: &mut HalftoneTextureCache,
    pattern_cache: &mut PatternTextureCache,
    shadow_cache: &mut ShadowTextureCache,
    layer: &Layer,
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
    let (shadow_outer, shadow_inner) = match shadow_cache.get_or_build(ctx, layer) {
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
            draw_children(painter, ctx, image_cache, mask_cache, noise_cache, halftone_cache, pattern_cache, shadow_cache, children, child_offset, origin, pan, zoom, opacity, editing_text);
        }
        LayerKind::Group { children } => {
            let child_offset = parent_offset + layer.frame.pos.to_vec2();
            draw_children(painter, ctx, image_cache, mask_cache, noise_cache, halftone_cache, pattern_cache, shadow_cache, children, child_offset, origin, pan, zoom, opacity, editing_text);
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
            ..
        } => {
            // While this layer is being edited in place, the floating
            // `egui::TextEdit` overlay (see `CanvasWidget::ui`'s
            // post-tree-draw block) is the only visible copy of its text.
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
                let wrap_width = match resize {
                    TextResize::Auto => f32::INFINITY,
                    TextResize::AutoHeight | TextResize::Fixed => bounds.width() * zoom,
                };
                let color = with_opacity(
                    layer.style.fill.as_ref().map(Paint::to_color32).unwrap_or(Color32::BLACK),
                    opacity * layer.style.fill_opacity,
                );
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
fn draw_arrow_cap(painter: &egui::Painter, tip: Pos2, outward: Vec2, cap: crate::model::ArrowCap, color: Color32, stroke_width: f32) {
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

/// Scales `color`'s alpha channel by `opacity` (`0.0..=1.0`), used to apply a
/// layer's (and its ancestors', multiplicatively accumulated) opacity to its
/// fill/stroke without needing an offscreen compositing pass.
fn with_opacity(color: Color32, opacity: f32) -> Color32 {
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
fn draw_even_odd_polygons(
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
fn earcut_polygon(exterior: &[Pos2], holes: &[Vec<Pos2>]) -> Option<(Vec<Pos2>, Vec<u32>)> {
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
fn triangulate_fill_with(exterior: &[Pos2], holes: &[Vec<Pos2>], color_at: impl Fn(Pos2) -> Color32) -> Option<egui::Mesh> {
    let (positions, indices) = earcut_polygon(exterior, holes)?;
    let mut mesh = egui::Mesh::default();
    mesh.vertices = positions.into_iter().map(|p| egui::epaint::Vertex::untextured(p, color_at(p))).collect();
    mesh.indices = indices;
    Some(mesh)
}

fn triangulate_fill(exterior: &[Pos2], holes: &[Vec<Pos2>], color: Color32) -> Option<egui::Mesh> {
    triangulate_fill_with(exterior, holes, |_| color)
}

/// The `Paint::Noise` sibling of `triangulate_fill_with`'s gradient path:
/// same earcut triangulation, but each vertex samples `texture_id` at
/// `uv_at(vertex)` instead of carrying an interpolated flat color, and
/// `tint` (typically opacity-only white, see `Image` layers' own textured
/// path) is multiplied over the sampled texel by egui's mesh renderer.
fn triangulate_fill_textured(
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
fn gradient_color_at_screen_point(gradient: &Gradient, p: Pos2, screen_rect: Rect, center: Pos2, rotation: f32, opacity: f32) -> Color32 {
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
fn noise_textured_mesh(
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
fn halftone_textured_mesh(
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
fn pattern_textured_mesh(
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
const GRADIENT_STROKE_SEGMENTS_PER_EDGE: usize = 10;

/// Approximates a gradient-colored stroke by subdividing the outline
/// (`points`, already in absolute screen space) into short solid-colored
/// segments, each sampled at its own midpoint. `egui` has no per-vertex-color
/// stroke primitive to interpolate properly like `triangulate_fill_with` does
/// for fill, and building real mitered stroke-ribbon geometry here is more
/// than a live-editing preview needs — the PNG exporter (`export.rs`) still
/// renders the exact gradient via a `tiny-skia` shader, so export quality
/// isn't affected by this approximation.
fn paint_gradient_stroke(
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
fn paint_polygon(
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
        } else if fill_flat != Color32::TRANSPARENT {
            if let Some(mesh) = triangulate_fill(&points, &[], fill_flat) {
                painter.add(egui::Shape::mesh(mesh));
            }
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

const BEZIER_SAMPLES_PER_SEGMENT: usize = 16;

fn cubic_bezier_point(p0: Pos2, p1: Pos2, p2: Pos2, p3: Pos2, t: f32) -> Pos2 {
    let mt = 1.0 - t;
    let a = mt * mt * mt;
    let b = 3.0 * mt * mt * t;
    let c = 3.0 * mt * t * t;
    let d = t * t * t;
    Pos2::new(
        a * p0.x + b * p1.x + c * p2.x + d * p3.x,
        a * p0.y + b * p1.y + c * p2.y + d * p3.y,
    )
}

/// Whether `points[i]`'s corner can be rounded: it has a positive
/// `corner_radius`, no handle of its own, and both its neighboring segments
/// are straight lines (matching `PathPoint::corner_radius`'s doc comment).
/// Always `false` for an open path's two endpoints — rounding a corner needs
/// both a previous and a next anchor to inset along.
fn is_roundable_corner(points: &[PathPoint], closed: bool, i: usize) -> bool {
    let n = points.len();
    let p = &points[i];
    if p.corner_radius <= 0.0 || p.handle_in.is_some() || p.handle_out.is_some() {
        return false;
    }
    let prev = if i == 0 {
        if !closed {
            return false;
        }
        n - 1
    } else {
        i - 1
    };
    let next = if i == n - 1 {
        if !closed {
            return false;
        }
        0
    } else {
        i + 1
    };
    points[prev].handle_out.is_none() && points[next].handle_in.is_none()
}

/// `[inset_from_prev, ...arc_samples..., inset_to_next]` for a rounded
/// corner at `points[i]` — see `shapes::rounded_corner_arc_points`. Only
/// called after `is_roundable_corner` confirms `i`'s wraparound neighbors
/// are valid (i.e. `closed` if `i` is an endpoint), so no `closed` parameter
/// is needed here.
fn corner_arc_points(points: &[PathPoint], i: usize) -> Vec<Pos2> {
    let n = points.len();
    let prev = if i == 0 { n - 1 } else { i - 1 };
    let next = if i == n - 1 { 0 } else { i + 1 };
    crate::shapes::rounded_corner_arc_points(points[prev].anchor, points[i].anchor, points[next].anchor, points[i].corner_radius)
}

/// Flattens anchors + bezier handles into a polyline, for egui's `PathShape`
/// (which has no native curve primitive covering fill of a multi-segment
/// curve, unlike `tiny-skia`'s `cubic_to` used in `export.rs`). Output is in
/// the same coordinate space as the input anchors — the caller applies
/// whatever offset/to-screen transform is appropriate.
///
/// A corner with `corner_radius > 0.0` (and no handles of its own, and both
/// neighboring segments straight — see `is_roundable_corner`) contributes an
/// inset-and-arc (`corner_arc_points`) instead of its raw anchor. Only ever
/// possible between two straight segments — a rounded corner can't also be a
/// bezier endpoint, since rounding requires both adjacent segments straight
/// by definition — so the existing straight/curved per-segment split below
/// only needs a rounding check on the straight branch.
pub(crate) fn flatten_path(points: &[PathPoint], closed: bool) -> Vec<Pos2> {
    if points.is_empty() {
        return Vec::new();
    }
    let n = points.len();
    let mut out = if is_roundable_corner(points, closed, 0) {
        corner_arc_points(points, 0)
    } else {
        vec![points[0].anchor]
    };
    let last_index = if closed { n } else { n - 1 };
    for i in 0..last_index {
        let a = &points[i];
        let b_idx = (i + 1) % n;
        let b = &points[b_idx];
        let p0 = a.anchor;
        let p3 = b.anchor;
        if a.handle_out.is_none() && b.handle_in.is_none() {
            if b_idx == 0 && closed {
                // Anchor 0's entry/arc was already seeded above (including
                // its "entry from the previous/last segment" leg) — the
                // implicit closing edge (last point back to `out[0]`)
                // completes this final segment, nothing more to push.
            } else if is_roundable_corner(points, closed, b_idx) {
                out.extend(corner_arc_points(points, b_idx));
            } else {
                out.push(p3);
            }
        } else {
            let c1 = p0 + a.handle_out.unwrap_or(Vec2::ZERO);
            let c2 = p3 + b.handle_in.unwrap_or(Vec2::ZERO);
            for s in 1..=BEZIER_SAMPLES_PER_SEGMENT {
                let t = s as f32 / BEZIER_SAMPLES_PER_SEGMENT as f32;
                out.push(cubic_bezier_point(p0, c1, c2, p3, t));
            }
        }
    }
    out
}

/// Draws the in-progress Pen path: the committed segments so far, a
/// rubber-band preview segment to the current mouse position, and small
/// handles at each anchor/control point so the user can see what they're
/// placing.
fn draw_pen_preview(
    painter: &egui::Painter,
    points: &[PathPoint],
    origin: Pos2,
    pan: Vec2,
    zoom: f32,
    hover_pos: Option<Pos2>,
) {
    let to_screen = |p: Pos2| origin + pan + p.to_vec2() * zoom;
    let stroke = EguiStroke::new(1.5, SELECTION_COLOR);

    let flat = flatten_path(points, false);
    let screen_points: Vec<Pos2> = flat.iter().map(|&p| to_screen(p)).collect();
    if screen_points.len() >= 2 {
        painter.add(egui::Shape::Path(egui::epaint::PathShape {
            points: screen_points,
            closed: false,
            fill: Color32::TRANSPARENT,
            stroke: stroke.into(),
        }));
    }
    if let (Some(last), Some(hover)) = (points.last(), hover_pos) {
        painter.line_segment(
            [to_screen(last.anchor), hover],
            EguiStroke::new(1.0, SELECTION_COLOR.gamma_multiply(0.6)),
        );
    }

    for (i, p) in points.iter().enumerate() {
        let anchor_screen = to_screen(p.anchor);
        for handle in [p.handle_in, p.handle_out] {
            if let Some(h) = handle {
                let handle_screen = to_screen(p.anchor + h);
                painter.line_segment(
                    [anchor_screen, handle_screen],
                    EguiStroke::new(1.0, Color32::from_gray(120)),
                );
                painter.circle_filled(handle_screen, 3.0, Color32::WHITE);
                painter.circle_stroke(handle_screen, 3.0, EguiStroke::new(1.0, Color32::from_gray(120)));
            }
        }
        let is_first = i == 0;
        let radius = if is_first { HANDLE_RADIUS } else { HANDLE_RADIUS * 0.7 };
        painter.circle(anchor_screen, radius, Color32::WHITE, stroke);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Document;

    fn straight_point(x: f32, y: f32) -> PathPoint {
        PathPoint {
            anchor: Pos2::new(x, y),
            handle_in: None,
            handle_out: None,
            point_type: PointType::Disconnected,
            corner_radius: 0.0,
        }
    }

    fn rounded_point(x: f32, y: f32, radius: f32) -> PathPoint {
        PathPoint {
            anchor: Pos2::new(x, y),
            handle_in: None,
            handle_out: None,
            point_type: PointType::Straight,
            corner_radius: radius,
        }
    }

    #[test]
    fn flatten_path_rounds_a_corner_with_radius() {
        let square = vec![
            straight_point(0.0, 0.0),
            rounded_point(40.0, 0.0, 8.0),
            straight_point(40.0, 40.0),
            straight_point(0.0, 40.0),
        ];
        let flat = flatten_path(&square, true);
        // A plain square would have exactly 4 vertices; a rounded corner
        // inserts several arc-sampled points instead of the single (40,0).
        assert!(flat.len() > 4, "expected extra arc points, got {}", flat.len());
        // No point in the polyline should land exactly on the un-rounded
        // corner (40,0) — it's been inset away on both sides.
        assert!(flat.iter().all(|p| (*p - Pos2::new(40.0, 0.0)).length() > 0.01));
    }

    #[test]
    fn flatten_path_clamps_radius_to_half_shorter_adjacent_segment() {
        // Corner at (40,0) with legs of length 40 (to (0,0)) and 10 (to
        // (40,10)) — radius should clamp to 5 (half the shorter leg).
        let points = vec![
            straight_point(0.0, 0.0),
            rounded_point(40.0, 0.0, 1000.0),
            straight_point(40.0, 10.0),
        ];
        let flat = flatten_path(&points, false);
        // The arc's exit point toward (40,10) should sit at exactly (40,5)
        // — half the shorter (length-10) adjacent leg, not the requested
        // radius of 1000.
        let target = Pos2::new(40.0, 5.0);
        let closest = flat.iter().min_by(|a, b| {
            (**a - target).length().partial_cmp(&(**b - target).length()).unwrap()
        }).unwrap();
        assert!((*closest - target).length() < 0.1, "expected a point at {target:?}, closest was {closest:?}");
    }

    #[test]
    fn flatten_path_zero_radius_is_unchanged() {
        let square = vec![
            straight_point(0.0, 0.0),
            straight_point(40.0, 0.0),
            straight_point(40.0, 40.0),
            straight_point(0.0, 40.0),
        ];
        assert_eq!(flatten_path(&square, true).len(), 4);
    }

    fn setup(points: Vec<PathPoint>, closed: bool) -> (CanvasWidget, History, Vec<LayerId>, LayerId) {
        let layer = Layer::new(
            "Path",
            Frame { pos: Pos2::ZERO, size: Vec2::new(40.0, 40.0), rotation: 0.0 },
            LayerKind::Path { points, closed },
        );
        let id = layer.id;
        let mut doc = Document::new();
        doc.active_page_mut().layers.push(layer);
        let history = History::new(doc);
        let widget = CanvasWidget::default();
        (widget, history, vec![id], id)
    }

    #[test]
    fn double_click_on_straight_anchor_converts_to_curved() {
        let square = vec![
            straight_point(0.0, 0.0),
            straight_point(20.0, 0.0),
            straight_point(20.0, 20.0),
            straight_point(0.0, 20.0),
        ];
        let (mut widget, mut history, selection, id) = setup(square, true);

        let converted = widget.try_convert_anchor_to_curve(&mut history, &selection, Pos2::new(20.0, 0.0), Pos2::ZERO);
        assert!(converted, "clicking directly on a straight anchor should convert it");

        let layer = history.get().active_page().find(id).unwrap();
        let LayerKind::Path { points, .. } = &layer.kind else { unreachable!() };
        assert_eq!(points[1].point_type, PointType::Mirror);
        assert!(points[1].handle_in.is_some());
        assert!(points[1].handle_out.is_some());
        // Mirror semantics: handles are exact opposites.
        assert_eq!(points[1].handle_in, points[1].handle_out.map(|h| -h));
    }

    #[test]
    fn right_click_hit_test_finds_the_nearest_anchor_within_radius() {
        let square = vec![
            straight_point(0.0, 0.0),
            straight_point(20.0, 0.0),
            straight_point(20.0, 20.0),
            straight_point(0.0, 20.0),
        ];
        let (widget, history, selection, id) = setup(square, true);

        let hit = widget.hit_test_path_anchor(&history, &selection, Pos2::ZERO, Pos2::new(20.0, 0.0));
        assert_eq!(hit, Some((id, 1)));
    }

    #[test]
    fn right_click_hit_test_misses_a_click_far_from_every_anchor() {
        let square = vec![
            straight_point(0.0, 0.0),
            straight_point(20.0, 0.0),
            straight_point(20.0, 20.0),
            straight_point(0.0, 20.0),
        ];
        let (widget, history, selection, _id) = setup(square, true);

        // Segment midpoint, same as the double-click miss case above — not
        // within `HANDLE_HIT_RADIUS` of any anchor.
        let hit = widget.hit_test_path_anchor(&history, &selection, Pos2::ZERO, Pos2::new(10.0, 0.0));
        assert_eq!(hit, None);
    }

    #[test]
    fn double_click_on_segment_midpoint_does_not_convert_so_insert_still_applies() {
        let square = vec![
            straight_point(0.0, 0.0),
            straight_point(20.0, 0.0),
            straight_point(20.0, 20.0),
            straight_point(0.0, 20.0),
        ];
        let (mut widget, mut history, selection, id) = setup(square, true);

        // A point on the segment between anchors 0 and 1, far from either
        // anchor — the double-click dispatch's regression requirement is
        // that this still falls through to inserting a new point, not
        // silently doing nothing.
        let midpoint = Pos2::new(10.0, 0.0);
        let converted = widget.try_convert_anchor_to_curve(&mut history, &selection, midpoint, Pos2::ZERO);
        assert!(!converted, "a segment midpoint isn't an anchor, so it shouldn't convert");

        widget.try_insert_path_point(&mut history, &selection, midpoint, Pos2::ZERO);
        let layer = history.get().active_page().find(id).unwrap();
        let LayerKind::Path { points, .. } = &layer.kind else { unreachable!() };
        assert_eq!(points.len(), 5, "insert should still add a point when the click wasn't on an anchor");
    }

    #[test]
    fn double_click_on_already_curved_anchor_does_nothing() {
        let points = vec![
            straight_point(0.0, 0.0),
            PathPoint {
                anchor: Pos2::new(20.0, 0.0),
                handle_in: Some(Vec2::new(-5.0, 0.0)),
                handle_out: Some(Vec2::new(5.0, 0.0)),
                point_type: PointType::Mirror,
                corner_radius: 0.0,
            },
            straight_point(20.0, 20.0),
        ];
        let (mut widget, mut history, selection, _id) = setup(points, false);
        let converted = widget.try_convert_anchor_to_curve(&mut history, &selection, Pos2::new(20.0, 0.0), Pos2::ZERO);
        assert!(!converted, "an anchor that already has handles shouldn't be re-converted");
    }

    #[test]
    fn hit_test_respects_rotation() {
        let layer = Layer::new(
            "Rect",
            Frame { pos: Pos2::new(0.0, 0.0), size: Vec2::new(20.0, 20.0), rotation: 45.0 },
            LayerKind::Rectangle { corner_radius: CornerRadii::ZERO },
        );
        let mut doc = Document::new();
        doc.active_page_mut().layers.push(layer);
        let page = doc.active_page();

        // The center is unaffected by rotation (it's the pivot).
        assert!(hit_test(page, Pos2::new(10.0, 10.0)).is_some());
        // The unrotated frame's own corner (0,0) is now outside the
        // rotated (45 degree) square's actual diagonal-edged footprint.
        assert!(hit_test(page, Pos2::new(1.0, 1.0)).is_none());
        // A point along the rotated square's actual edge, straight up from
        // the center by half its diagonal, should hit.
        let half_diagonal = 10.0 * std::f32::consts::SQRT_2 * 0.5;
        assert!(hit_test(page, Pos2::new(10.0, 10.0 - half_diagonal + 0.5)).is_some());
    }

    #[test]
    fn hit_test_skips_masked_out_region_of_content_layer() {
        // A content layer larger than its mask: the mask (on top, still
        // hit-tested by its own plain bounding box like any other layer —
        // only what it *masks* gets the alpha-accurate treatment) sits at
        // (0,0)-(100,100); the square it masks is much bigger, spanning
        // (-50,-50)-(150,150).
        let mut square = Layer::new(
            "Square",
            Frame { pos: Pos2::new(-50.0, -50.0), size: Vec2::new(200.0, 200.0), rotation: 0.0 },
            LayerKind::Rectangle { corner_radius: CornerRadii::ZERO },
        );
        square.style.fill = Some(Paint::Solid(Color32::RED));
        let mut mask = Layer::new(
            "Mask",
            Frame { pos: Pos2::new(0.0, 0.0), size: Vec2::new(100.0, 100.0), rotation: 0.0 },
            LayerKind::Oval,
        );
        mask.is_mask = true;
        let mask_id = mask.id;

        let mut doc = Document::new();
        doc.active_page_mut().layers.push(square);
        doc.active_page_mut().layers.push(mask);
        let page = doc.active_page();

        // Center: within the mask's own bounding box, so the mask (topmost)
        // claims the hit itself, same as any other layer stacked on top.
        assert_eq!(hit_test(page, Pos2::new(50.0, 50.0)), Some(mask_id));
        // Well outside the mask's bounding box entirely, but still deep
        // inside the square's own (much bigger) bounding box: the old
        // bbox-only test would have hit the square here even though the
        // mask clips it away completely at this point — now it correctly
        // finds nothing.
        assert_eq!(hit_test(page, Pos2::new(-20.0, -20.0)), None);
    }

    fn bg_rect(name: &str, pos: Pos2, size: Vec2) -> Layer {
        Layer::new(name, Frame { pos, size, rotation: 0.0 }, LayerKind::Rectangle { corner_radius: CornerRadii::ZERO })
    }

    #[test]
    fn hit_test_a_boolean_group_selects_the_group_not_a_child() {
        let a = bg_rect("A", Pos2::new(0.0, 0.0), Vec2::new(40.0, 40.0));
        let mut b = bg_rect("B", Pos2::new(20.0, 20.0), Vec2::new(40.0, 40.0));
        b.bool_op = crate::model::BoolOp::Union;
        let group = Layer::new(
            "Boolean Group",
            Frame { pos: Pos2::new(0.0, 0.0), size: Vec2::new(60.0, 60.0), rotation: 0.0 },
            LayerKind::BooleanGroup { children: vec![a, b] },
        );
        let group_id = group.id;

        let mut doc = Document::new();
        doc.active_page_mut().layers.push(group);
        let page = doc.active_page();

        // Inside the overlap of the two unioned rects — a click here should
        // select the BooleanGroup itself, not descend into a child (unlike
        // Group/Artboard).
        assert_eq!(hit_test(page, Pos2::new(25.0, 25.0)), Some(group_id));
        // Inside A only.
        assert_eq!(hit_test(page, Pos2::new(5.0, 5.0)), Some(group_id));
    }

    #[test]
    fn hit_test_a_boolean_group_is_exact_not_just_bounding_box() {
        let base = bg_rect("Base", Pos2::new(0.0, 0.0), Vec2::new(60.0, 60.0));
        let mut hole = bg_rect("Hole", Pos2::new(20.0, 20.0), Vec2::new(20.0, 20.0));
        hole.bool_op = crate::model::BoolOp::Subtract;
        let group = Layer::new(
            "Boolean Group",
            Frame { pos: Pos2::new(0.0, 0.0), size: Vec2::new(60.0, 60.0), rotation: 0.0 },
            LayerKind::BooleanGroup { children: vec![base, hole] },
        );

        let mut doc = Document::new();
        doc.active_page_mut().layers.push(group);
        let page = doc.active_page();

        // Inside the punched-out hole but well within the group's bounding
        // box: a bbox-only test would hit here; the exact silhouette test
        // must not.
        assert_eq!(hit_test(page, Pos2::new(30.0, 30.0)), None);
    }

    #[test]
    fn hit_test_a_masked_boolean_group_still_respects_the_mask() {
        let a = bg_rect("A", Pos2::new(0.0, 0.0), Vec2::new(40.0, 40.0));
        let mut b = bg_rect("B", Pos2::new(20.0, 20.0), Vec2::new(40.0, 40.0));
        b.bool_op = crate::model::BoolOp::Union;
        let group = Layer::new(
            "Boolean Group",
            Frame { pos: Pos2::new(0.0, 0.0), size: Vec2::new(60.0, 60.0), rotation: 0.0 },
            LayerKind::BooleanGroup { children: vec![a, b] },
        );

        // A mask covering only the group's left half, sitting above it (so
        // it's the mask for the group's own render/hit-test run).
        let mut mask = Layer::new(
            "Mask",
            Frame { pos: Pos2::new(0.0, 0.0), size: Vec2::new(30.0, 60.0), rotation: 0.0 },
            LayerKind::Rectangle { corner_radius: CornerRadii::ZERO },
        );
        mask.is_mask = true;

        let mut doc = Document::new();
        doc.active_page_mut().layers.push(group);
        doc.active_page_mut().layers.push(mask);
        let page = doc.active_page();

        // Inside the group's own silhouette (the A/B overlap) but outside
        // the mask's coverage (x > 30) — should be clipped away entirely.
        assert_eq!(hit_test(page, Pos2::new(55.0, 55.0)), None);
    }

    #[test]
    fn rotate_drag_sets_frame_rotation_and_preserves_pivot_distance() {
        let layer = Layer::new(
            "Rect",
            Frame { pos: Pos2::new(0.0, 0.0), size: Vec2::new(20.0, 10.0), rotation: 0.0 },
            LayerKind::Rectangle { corner_radius: CornerRadii::ZERO },
        );
        let id = layer.id;
        let mut doc = Document::new();
        doc.active_page_mut().layers.push(layer);

        let mut leaves = Vec::new();
        collect_rotatable_leaves(doc.active_page().find(id).unwrap(), Vec2::ZERO, &mut leaves);
        let pivot = leaves[0].abs_bounds.center();
        let pivot_dist_before = (leaves[0].abs_bounds.center() - pivot).length();

        apply_rotation_delta(doc.active_page_mut(), pivot, 90.0, &leaves);

        let rotated = doc.active_page().find(id).unwrap();
        assert_eq!(rotated.frame.rotation, 90.0);
        assert_eq!(rotated.frame.size, Vec2::new(20.0, 10.0), "size is preserved, only orientation changes");
        let pivot_dist_after = (rotated.frame.bounds().center() - pivot).length();
        assert!((pivot_dist_after - pivot_dist_before).abs() < 1e-3, "distance from pivot must be preserved");
    }

    #[test]
    fn rotating_a_group_bakes_rotation_into_children_not_the_group() {
        let child_a = Layer::new(
            "A",
            Frame { pos: Pos2::new(0.0, 0.0), size: Vec2::new(10.0, 10.0), rotation: 0.0 },
            LayerKind::Rectangle { corner_radius: CornerRadii::ZERO },
        );
        let child_b = Layer::new(
            "B",
            Frame { pos: Pos2::new(20.0, 0.0), size: Vec2::new(10.0, 10.0), rotation: 0.0 },
            LayerKind::Rectangle { corner_radius: CornerRadii::ZERO },
        );
        let (a_id, b_id) = (child_a.id, child_b.id);
        let group = Layer::new(
            "Group",
            Frame { pos: Pos2::new(0.0, 0.0), size: Vec2::new(30.0, 10.0), rotation: 0.0 },
            LayerKind::Group { children: vec![child_a, child_b] },
        );
        let group_id = group.id;

        let mut leaves = Vec::new();
        collect_rotatable_leaves(&group, Vec2::ZERO, &mut leaves);
        // Both children collected as leaves, not the group itself.
        assert_eq!(leaves.len(), 2);
        assert!(leaves.iter().all(|l| l.id != group_id));

        let mut doc = Document::new();
        doc.active_page_mut().layers.push(group);
        let overall_center = Pos2::new(15.0, 5.0);
        apply_rotation_delta(doc.active_page_mut(), overall_center, 90.0, &leaves);

        let group_after = doc.active_page().find(group_id).unwrap();
        assert_eq!(group_after.frame.rotation, 0.0, "the group's own rotation never changes");
        let LayerKind::Group { children } = &group_after.kind else { unreachable!() };
        let a_after = children.iter().find(|l| l.id == a_id).unwrap();
        let b_after = children.iter().find(|l| l.id == b_id).unwrap();
        assert_eq!(a_after.frame.rotation, 90.0, "rotation is baked into each child instead");
        assert_eq!(b_after.frame.rotation, 90.0);
    }

    #[test]
    fn refit_after_rotating_a_boolean_group_keeps_its_frame_in_sync_with_children() {
        // Two circles side by side, unioned into a `BooleanGroup` — matches
        // the reported bug: rotating the result left the selection
        // rectangle/handles anchored to the pre-rotation bounds while the
        // live-rendered (children-derived) shape moved away from them.
        let child_a = Layer::new(
            "A",
            Frame { pos: Pos2::new(0.0, 0.0), size: Vec2::new(10.0, 10.0), rotation: 0.0 },
            LayerKind::Oval,
        );
        let child_b = Layer::new(
            "B",
            Frame { pos: Pos2::new(20.0, 0.0), size: Vec2::new(10.0, 10.0), rotation: 0.0 },
            LayerKind::Oval,
        );
        let (a_id, b_id) = (child_a.id, child_b.id);
        let group = Layer::new(
            "Union",
            Frame { pos: Pos2::new(0.0, 0.0), size: Vec2::new(30.0, 10.0), rotation: 0.0 },
            LayerKind::BooleanGroup { children: vec![child_a, child_b] },
        );
        let group_id = group.id;

        let mut leaves = Vec::new();
        collect_rotatable_leaves(&group, Vec2::ZERO, &mut leaves);
        assert_eq!(leaves.len(), 2, "children are collected as rotatable leaves, not the BooleanGroup itself");

        let mut doc = Document::new();
        doc.active_page_mut().layers.push(group);
        let overall_center = Pos2::new(15.0, 5.0);
        apply_rotation_delta(doc.active_page_mut(), overall_center, 90.0, &leaves);

        // Before the refit pass: the container's own frame is untouched —
        // this is the stale/detached state the bug report screenshots.
        let stale = doc.active_page().find(group_id).unwrap();
        assert_eq!(stale.frame.pos, Pos2::new(0.0, 0.0));
        assert_eq!(stale.frame.size, Vec2::new(30.0, 10.0));
        let stale_pos = stale.frame.pos;
        let LayerKind::BooleanGroup { children: stale_children } = &stale.kind else { unreachable!() };
        let expected_local_bbox =
            stale_children.iter().map(|c| c.frame.rotated_bounds()).reduce(|a, b| a.union(b)).unwrap();

        // Absolute positions of the children right after the rotation —
        // `refit_container_to_children` must not move visible content, only
        // repackage the container/children frame split.
        let abs_before: Vec<Pos2> = [a_id, b_id]
            .iter()
            .map(|&id| {
                let offset = doc.active_page().absolute_offset(id).unwrap();
                doc.active_page().find(id).unwrap().frame.pos + offset
            })
            .collect();

        refit_container_to_children(doc.active_page_mut(), group_id);

        let refit = doc.active_page().find(group_id).unwrap();
        assert_eq!(refit.frame.rotation, 0.0, "the container's own rotation stays 0");
        assert_eq!(
            refit.frame.size,
            expected_local_bbox.size(),
            "the container's own frame now exactly bounds its rotated children"
        );
        assert_eq!(refit.frame.pos, stale_pos + expected_local_bbox.min.to_vec2());

        let abs_after: Vec<Pos2> = [a_id, b_id]
            .iter()
            .map(|&id| {
                let offset = doc.active_page().absolute_offset(id).unwrap();
                doc.active_page().find(id).unwrap().frame.pos + offset
            })
            .collect();
        for (before, after) in abs_before.iter().zip(abs_after.iter()) {
            assert!(
                (*before - *after).length() < 1e-3,
                "refitting must not move any child's absolute position: {before:?} vs {after:?}"
            );
        }
    }

    #[test]
    fn refit_after_resizing_a_boolean_group_keeps_its_frame_in_sync_with_children() {
        // Same reported bug as the rotate case, but via a resize-handle
        // drag: `apply_resize_delta` only ever writes into the leaves
        // `collect_resizable_leaves` found, so a selected `BooleanGroup`'s
        // own `frame` must be explicitly refit afterward or its selection
        // box stays anchored to the pre-resize bounds (too small/offset)
        // while its live-rendered geometry grows with the actual children.
        let child_a = Layer::new(
            "A",
            Frame { pos: Pos2::new(0.0, 0.0), size: Vec2::new(10.0, 10.0), rotation: 0.0 },
            LayerKind::Oval,
        );
        let child_b = Layer::new(
            "B",
            Frame { pos: Pos2::new(20.0, 0.0), size: Vec2::new(10.0, 10.0), rotation: 0.0 },
            LayerKind::Oval,
        );
        let (a_id, b_id) = (child_a.id, child_b.id);
        let group = Layer::new(
            "Union",
            Frame { pos: Pos2::new(0.0, 0.0), size: Vec2::new(30.0, 10.0), rotation: 0.0 },
            LayerKind::BooleanGroup { children: vec![child_a, child_b] },
        );
        let group_id = group.id;

        let mut layers = Vec::new();
        collect_resizable_leaves(&group, Vec2::ZERO, &mut layers);
        assert_eq!(layers.len(), 2, "children are collected as resizable leaves, not the BooleanGroup itself");

        let mut doc = Document::new();
        doc.active_page_mut().layers.push(group);

        // Drag the bottom-right handle out to double the overall size,
        // anchored at the top-left corner (0,0) — a plain 2x scale.
        let scale = Vec2::new(2.0, 2.0);
        let transform = |p: Pos2| Pos2::new(p.x * scale.x, p.y * scale.y);
        apply_resize_delta(doc.active_page_mut(), &layers, transform, scale, false, 2.0);

        // Before the refit pass: the container's own frame is untouched —
        // this is the stale/detached state the bug report screenshots (a
        // box that no longer matches the now-larger rendered shape).
        let stale = doc.active_page().find(group_id).unwrap();
        assert_eq!(stale.frame.pos, Pos2::new(0.0, 0.0));
        assert_eq!(stale.frame.size, Vec2::new(30.0, 10.0));

        refit_container_to_children(doc.active_page_mut(), group_id);

        let refit = doc.active_page().find(group_id).unwrap();
        assert_eq!(refit.frame.rotation, 0.0);
        assert_eq!(refit.frame.pos, Pos2::new(0.0, 0.0));
        assert_eq!(
            refit.frame.size,
            Vec2::new(60.0, 20.0),
            "the container's own frame now exactly bounds its resized children"
        );

        // Children scaled 2x from the (0,0) anchor, same as a leaf would.
        let abs_a = doc.active_page().absolute_offset(a_id).unwrap() + doc.active_page().find(a_id).unwrap().frame.pos.to_vec2();
        let abs_b = doc.active_page().absolute_offset(b_id).unwrap() + doc.active_page().find(b_id).unwrap().frame.pos.to_vec2();
        assert_eq!(abs_a, Vec2::new(0.0, 0.0));
        assert_eq!(abs_b, Vec2::new(40.0, 0.0));
        assert_eq!(doc.active_page().find(a_id).unwrap().frame.size, Vec2::new(20.0, 20.0));
        assert_eq!(doc.active_page().find(b_id).unwrap().frame.size, Vec2::new(20.0, 20.0));
    }

    #[test]
    fn resizing_a_boolean_group_across_multiple_drag_frames_does_not_drift() {
        // Regression for a translation bug the refit fix above introduced:
        // `apply_resize_delta` re-derives every leaf's position each frame
        // from a `parent_offset` captured once at drag start (see
        // `collect_resizable_leaves`). If something refits the container's
        // own `frame.pos` *during* the drag (i.e. between two `dragged()`
        // frames using the same `layers`), that baseline goes stale and the
        // next frame's leaf placement is off by the container's own shift —
        // compounding every frame into a runaway translation. So
        // `refit_container_to_children` must run only once, after the drag
        // ends (`drag_stopped`), never inside the live per-frame handler —
        // this simulates several `dragged()` frames in a row, growing the
        // scale each time, with no refit in between (matching what the
        // fixed code path now does), and checks the shape lands exactly
        // where a single direct application of the final scale would.
        let child_a = Layer::new(
            "A",
            Frame { pos: Pos2::new(0.0, 0.0), size: Vec2::new(10.0, 10.0), rotation: 0.0 },
            LayerKind::Oval,
        );
        let child_b = Layer::new(
            "B",
            Frame { pos: Pos2::new(20.0, 0.0), size: Vec2::new(10.0, 10.0), rotation: 0.0 },
            LayerKind::Oval,
        );
        let (a_id, b_id) = (child_a.id, child_b.id);
        let group = Layer::new(
            "Union",
            Frame { pos: Pos2::new(0.0, 0.0), size: Vec2::new(30.0, 10.0), rotation: 0.0 },
            LayerKind::BooleanGroup { children: vec![child_a, child_b] },
        );
        let group_id = group.id;

        let mut layers = Vec::new();
        collect_resizable_leaves(&group, Vec2::ZERO, &mut layers);

        let mut doc = Document::new();
        doc.active_page_mut().layers.push(group);

        // Several consecutive drag frames, as if the mouse kept moving
        // further out — same `layers` captured once at drag start, no
        // refit between calls, exactly like the fixed `dragged()` handler.
        for factor in [1.2, 1.5, 1.8, 2.0] {
            let scale = Vec2::new(factor, factor);
            let transform = |p: Pos2| Pos2::new(p.x * scale.x, p.y * scale.y);
            apply_resize_delta(doc.active_page_mut(), &layers, transform, scale, false, factor);
        }

        // The container's own frame must still be untouched — no drift was
        // introduced by the repeated per-frame calls.
        let stale = doc.active_page().find(group_id).unwrap();
        assert_eq!(stale.frame.pos, Pos2::new(0.0, 0.0));
        assert_eq!(stale.frame.size, Vec2::new(30.0, 10.0));

        // The final frame's 2x scale must land exactly where one direct 2x
        // application would (not compounded across the 1.2/1.5/1.8 steps).
        let abs_a = doc.active_page().absolute_offset(a_id).unwrap() + doc.active_page().find(a_id).unwrap().frame.pos.to_vec2();
        let abs_b = doc.active_page().absolute_offset(b_id).unwrap() + doc.active_page().find(b_id).unwrap().frame.pos.to_vec2();
        assert_eq!(abs_a, Vec2::new(0.0, 0.0));
        assert_eq!(abs_b, Vec2::new(40.0, 0.0));
        assert_eq!(doc.active_page().find(a_id).unwrap().frame.size, Vec2::new(20.0, 20.0));
        assert_eq!(doc.active_page().find(b_id).unwrap().frame.size, Vec2::new(20.0, 20.0));

        // Only now (drag_stopped, in the real code path) does the refit run.
        refit_container_to_children(doc.active_page_mut(), group_id);
        let refit = doc.active_page().find(group_id).unwrap();
        assert_eq!(refit.frame.pos, Pos2::new(0.0, 0.0));
        assert_eq!(refit.frame.size, Vec2::new(60.0, 20.0));
    }

    #[test]
    fn resizing_a_freshly_created_boolean_group_via_a_real_handle_drag_stays_tight() {
        // End-to-end version of the two tests above, built the same way the
        // real UI does it: `create_boolean_group` (not a hand-built `Frame`)
        // on two *overlapping* circles, then the exact
        // `handle.resize`/`scale`/`transform` sequence `dragged()`'s
        // `ResizingGroup` arm runs for a `BottomRight`-handle drag. Guards
        // against the reported "box ends up detached from the shape after
        // resizing" regression by checking the refit box against the union
        // computed independently from each child's *own* resized bounds,
        // not by re-deriving it the same way `refit_container_to_children`
        // internally does (which would just tautologically pass).
        let circle_a = Layer::new(
            "A",
            Frame { pos: Pos2::new(0.0, 0.0), size: Vec2::new(100.0, 100.0), rotation: 0.0 },
            LayerKind::Oval,
        );
        let circle_b = Layer::new(
            "B",
            Frame { pos: Pos2::new(60.0, 0.0), size: Vec2::new(100.0, 100.0), rotation: 0.0 },
            LayerKind::Oval,
        );
        let (a_id, b_id) = (circle_a.id, circle_b.id);
        let mut page = Page::new("Page 1");
        page.layers.push(circle_a);
        page.layers.push(circle_b);
        let group_id = crate::boolean_ops::create_boolean_group(&mut page, &[a_id, b_id], crate::model::BoolOp::Union)
            .expect("both operands are fillable ovals");

        // Matches `create_boolean_group`'s own tight-wrap bbox.
        let group_frame_at_creation = page.find(group_id).unwrap().frame;
        assert_eq!(group_frame_at_creation.pos, Pos2::new(0.0, 0.0));
        assert_eq!(group_frame_at_creation.size, Vec2::new(160.0, 100.0));

        // --- drag start (mirrors `hovered_handle` branch in `dragged`) ---
        let mut layers = Vec::new();
        collect_resizable_leaves(page.find(group_id).unwrap(), Vec2::ZERO, &mut layers);
        let start_overall_bounds =
            layers.iter().map(|l| l.abs_bounds).reduce(|a, b| a.union(b)).unwrap();
        assert_eq!(start_overall_bounds, group_frame_at_creation.bounds(), "handle positions and the resize anchor must agree");

        // --- one drag frame: drag BottomRight out to (400, 300) ---
        let handle = Handle::BottomRight;
        let doc_pos = Pos2::new(400.0, 300.0);
        let new_overall = handle.resize(start_overall_bounds, doc_pos);
        let old = start_overall_bounds;
        let scale = Vec2::new(new_overall.width() / old.width(), new_overall.height() / old.height());
        let old_anchor = old.min;
        let new_anchor = new_overall.min;
        let transform = |p: Pos2| Pos2::new(new_anchor.x + (p.x - old_anchor.x) * scale.x, new_anchor.y + (p.y - old_anchor.y) * scale.y);
        apply_resize_delta(&mut page, &layers, transform, scale, false, (scale.x + scale.y) / 2.0);

        // Union bbox of the children's *own* post-resize bounds, still in
        // the group's pre-refit local space (group.frame.pos hasn't moved
        // yet at this point) — independent of `refit_container_to_children`'s
        // internals, so comparing against it actually checks the box
        // against the shape instead of tautologically re-deriving it.
        let group_pos_before_refit = page.find(group_id).unwrap().frame.pos;
        let expected_local_bbox = page
            .find(a_id)
            .unwrap()
            .frame
            .rotated_bounds()
            .union(page.find(b_id).unwrap().frame.rotated_bounds());

        // --- drag_stopped: the one-time refit ---
        refit_container_to_children(&mut page, group_id);

        let refit = page.find(group_id).unwrap();
        assert_eq!(
            refit.frame.pos,
            group_pos_before_refit + expected_local_bbox.min.to_vec2(),
            "the box's position must exactly match the resized children, no slack on any side"
        );
        assert_eq!(
            refit.frame.size,
            expected_local_bbox.size(),
            "the box's size must exactly match the resized children, no slack on any side"
        );
    }

    #[test]
    fn display_bounds_of_a_boolean_group_tracks_children_without_a_refit() {
        // The reported "box disappears while resizing, then ends up
        // detached from the shape" bug: `refit_container_to_children` only
        // runs once, at `drag_stopped` (see its doc comment for why it
        // can't run every frame), so mid-drag — and even on the very frame
        // the drag ends, if painting happens to read the document before
        // that refit lands — `layer.frame.bounds()` is stale. The
        // selection outline must use `display_bounds` instead, which
        // reports the live union of a container's children with no
        // dependency on the stored `frame` ever having been refit.
        let child_a = Layer::new(
            "A",
            Frame { pos: Pos2::new(0.0, 0.0), size: Vec2::new(10.0, 10.0), rotation: 0.0 },
            LayerKind::Oval,
        );
        let child_b = Layer::new(
            "B",
            Frame { pos: Pos2::new(20.0, 0.0), size: Vec2::new(10.0, 10.0), rotation: 0.0 },
            LayerKind::Oval,
        );
        let group = Layer::new(
            "Union",
            Frame { pos: Pos2::new(0.0, 0.0), size: Vec2::new(30.0, 10.0), rotation: 0.0 },
            LayerKind::BooleanGroup { children: vec![child_a, child_b] },
        );

        // Matches the container's own (still correct, freshly-created)
        // frame before anything has moved.
        assert_eq!(display_bounds(&group), group.frame.bounds());

        // Simulate a resize (or rotate) leaving the container's own `frame`
        // stale — mutate a child directly, as `apply_resize_delta`/
        // `apply_rotation_delta` would, without calling
        // `refit_container_to_children`.
        let mut resized = group.clone();
        let LayerKind::BooleanGroup { children } = &mut resized.kind else { unreachable!() };
        children[1].frame.pos = Pos2::new(60.0, 0.0);
        children[1].frame.size = Vec2::new(20.0, 20.0);

        // The stored frame is untouched (this is the expected, deliberately
        // deferred staleness)...
        assert_eq!(resized.frame.bounds(), Rect::from_min_size(Pos2::new(0.0, 0.0), Vec2::new(30.0, 10.0)));
        // ...but `display_bounds` already reports the live, correct extent:
        // A still spans (0,0)-(10,10), B now spans (60,0)-(80,20).
        assert_eq!(display_bounds(&resized), Rect::from_min_size(Pos2::new(0.0, 0.0), Vec2::new(80.0, 20.0)));
    }

    #[test]
    fn rotating_a_boolean_group_of_circles_at_45_degrees_stays_tight_not_inflated() {
        // Regression for the reported "rotating a group makes the box
        // balloon out, detached from the shape" bug: a true circle (equal
        // width/height) is rotationally invariant — rotating it changes
        // nothing about its actual silhouette — but `Frame::rotated_bounds()`'s
        // generic formula (the AABB of the *frame rectangle's* rotated
        // corners) doesn't know that, and reports up to ~41% more area at a
        // 45° rotation. `tight_rotated_bounds` special-cases
        // `LayerKind::Oval` with the exact rotated-ellipse AABB formula so
        // the aggregated container box doesn't inherit that slack.
        let circle_a = Layer::new(
            "A",
            Frame { pos: Pos2::new(0.0, 0.0), size: Vec2::new(100.0, 100.0), rotation: 0.0 },
            LayerKind::Oval,
        );
        let circle_b = Layer::new(
            "B",
            Frame { pos: Pos2::new(100.0, 0.0), size: Vec2::new(100.0, 100.0), rotation: 0.0 },
            LayerKind::Oval,
        );
        let a_id = circle_a.id;
        let group = Layer::new(
            "Union",
            Frame { pos: Pos2::new(0.0, 0.0), size: Vec2::new(200.0, 100.0), rotation: 0.0 },
            LayerKind::BooleanGroup { children: vec![circle_a, circle_b] },
        );
        let group_id = group.id;

        let mut leaves = Vec::new();
        collect_rotatable_leaves(&group, Vec2::ZERO, &mut leaves);

        let mut doc = Document::new();
        doc.active_page_mut().layers.push(group);
        let pivot = Pos2::new(100.0, 50.0);
        apply_rotation_delta(doc.active_page_mut(), pivot, 45.0, &leaves);
        refit_container_to_children(doc.active_page_mut(), group_id);

        // Independently expected: each circle's own bounding box is
        // unaffected by rotation (it's a true circle) — only its *center*
        // orbits the pivot.
        let rotate = |p: Pos2| rotate_point(p, pivot, 45.0);
        let expected_a = Rect::from_center_size(rotate(Pos2::new(50.0, 50.0)), Vec2::new(100.0, 100.0));
        let expected_b = Rect::from_center_size(rotate(Pos2::new(150.0, 50.0)), Vec2::new(100.0, 100.0));
        let expected = expected_a.union(expected_b);

        let actual = doc.active_page().find(group_id).unwrap().frame.bounds();
        assert!(
            (actual.min - expected.min).length() < 1e-2 && (actual.max - expected.max).length() < 1e-2,
            "box must stay tight around the rotated circles, not inflate toward the rotated frame's corners: expected {expected:?}, got {actual:?}"
        );

        // Sanity check the fix is actually doing something: the old
        // (unfixed) formula — union of the generic `Frame::rotated_bounds()`
        // per circle — would report a visibly larger box at 45°.
        let inflated_a = doc.active_page().find(a_id).unwrap().frame.rotated_bounds();
        assert!(
            inflated_a.width() > expected_a.width() + 1.0,
            "sanity check: the generic rotated-rectangle AABB should indeed be looser than the true circle bbox at 45°"
        );
    }

    #[test]
    fn cutting_closed_path_at_an_anchor_opens_it_with_a_duplicated_endpoint() {
        let square = vec![
            straight_point(0.0, 0.0),
            straight_point(40.0, 0.0),
            straight_point(40.0, 40.0),
            straight_point(0.0, 40.0),
        ];
        let (mut widget, mut history, mut selection, id) = setup(square, true);

        widget.try_scissor_path(&mut history, &mut selection, Pos2::new(0.0, 0.0), Pos2::ZERO);

        let layer = history.get().active_page().find(id).unwrap();
        let LayerKind::Path { points, closed } = &layer.kind else {
            panic!("expected Path");
        };
        assert!(!closed);
        assert_eq!(points.len(), 5);
        assert_eq!(selection, vec![id]);
        let first_abs = points[0].anchor + layer.frame.pos.to_vec2();
        let last_abs = points[4].anchor + layer.frame.pos.to_vec2();
        assert_eq!(first_abs, last_abs);
    }

    #[test]
    fn cutting_closed_path_at_a_segment_opens_it_with_an_inserted_point() {
        let square = vec![
            straight_point(0.0, 0.0),
            straight_point(40.0, 0.0),
            straight_point(40.0, 40.0),
            straight_point(0.0, 40.0),
        ];
        let (mut widget, mut history, mut selection, id) = setup(square, true);

        // Midpoint of the bottom edge, far from any anchor.
        widget.try_scissor_path(&mut history, &mut selection, Pos2::new(20.0, 0.0), Pos2::ZERO);

        let layer = history.get().active_page().find(id).unwrap();
        let LayerKind::Path { points, closed } = &layer.kind else {
            panic!("expected Path");
        };
        assert!(!closed);
        assert_eq!(points.len(), 6);
        let first_abs = points[0].anchor + layer.frame.pos.to_vec2();
        let last_abs = points[points.len() - 1].anchor + layer.frame.pos.to_vec2();
        assert_eq!(first_abs, last_abs);
        assert!((first_abs - Pos2::new(20.0, 0.0)).length() < 0.01);
    }

    #[test]
    fn cutting_open_path_at_an_interior_anchor_splits_it_into_two_layers() {
        let points = vec![
            straight_point(0.0, 0.0),
            straight_point(20.0, 0.0),
            straight_point(40.0, 0.0),
            straight_point(60.0, 0.0),
        ];
        let (mut widget, mut history, mut selection, original_id) = setup(points, false);

        widget.try_scissor_path(&mut history, &mut selection, Pos2::new(40.0, 0.0), Pos2::ZERO);

        assert_eq!(selection.len(), 2);
        assert!(!selection.contains(&original_id));
        let page = history.get().active_page();
        assert!(page.find(original_id).is_none());
        assert_eq!(page.layers.len(), 2);

        let layer_a = page.find(selection[0]).unwrap();
        let layer_b = page.find(selection[1]).unwrap();
        let LayerKind::Path { points: pa, closed: ca } = &layer_a.kind else { panic!() };
        let LayerKind::Path { points: pb, closed: cb } = &layer_b.kind else { panic!() };
        assert!(!ca && !cb);
        assert_eq!(pa.len(), 3);
        assert_eq!(pb.len(), 2);
    }

    #[test]
    fn cutting_open_path_at_an_interior_segment_splits_it_and_inserts_the_cut_point() {
        let points = vec![straight_point(0.0, 0.0), straight_point(40.0, 0.0)];
        let (mut widget, mut history, mut selection, original_id) = setup(points, false);

        widget.try_scissor_path(&mut history, &mut selection, Pos2::new(20.0, 0.0), Pos2::ZERO);

        assert_eq!(selection.len(), 2);
        let page = history.get().active_page();
        assert!(page.find(original_id).is_none());
        let layer_a = page.find(selection[0]).unwrap();
        let layer_b = page.find(selection[1]).unwrap();
        let LayerKind::Path { points: pa, .. } = &layer_a.kind else { panic!() };
        let LayerKind::Path { points: pb, .. } = &layer_b.kind else { panic!() };
        assert_eq!(pa.len(), 2);
        assert_eq!(pb.len(), 2);
        let cut_abs = pa[1].anchor + layer_a.frame.pos.to_vec2();
        assert!((cut_abs - Pos2::new(20.0, 0.0)).length() < 0.01);
    }

    #[test]
    fn cutting_open_path_at_its_endpoint_is_a_noop() {
        let points = vec![
            straight_point(0.0, 0.0),
            straight_point(20.0, 0.0),
            straight_point(40.0, 0.0),
        ];
        let (mut widget, mut history, mut selection, id) = setup(points, false);

        widget.try_scissor_path(&mut history, &mut selection, Pos2::new(0.0, 0.0), Pos2::ZERO);

        assert_eq!(selection, vec![id]);
        assert!(!history.can_undo());
        let layer = history.get().active_page().find(id).unwrap();
        let LayerKind::Path { points, .. } = &layer.kind else { panic!() };
        assert_eq!(points.len(), 3);
    }

    #[test]
    fn deleting_selected_points_removes_exactly_those_and_renormalizes_frame() {
        let points = vec![
            straight_point(0.0, 0.0),
            straight_point(20.0, 0.0),
            straight_point(40.0, 0.0),
            straight_point(40.0, 40.0),
            straight_point(0.0, 40.0),
        ];
        let (mut widget, mut history, mut selection, id) = setup(points, true);
        widget.point_edit_layer = Some(id);
        widget.selected_points = vec![1, 3];

        widget.delete_selected_points(&mut history, &mut selection);

        assert!(widget.selected_points.is_empty());
        let layer = history.get().active_page().find(id).unwrap();
        let LayerKind::Path { points, .. } = &layer.kind else { panic!() };
        assert_eq!(points.len(), 3);
        let abs: Vec<Pos2> = points.iter().map(|p| p.anchor + layer.frame.pos.to_vec2()).collect();
        assert_eq!(abs, vec![Pos2::new(0.0, 0.0), Pos2::new(40.0, 0.0), Pos2::new(0.0, 40.0)]);
    }

    #[test]
    fn deleting_selected_points_below_two_remaining_deletes_the_whole_layer() {
        let points = vec![straight_point(0.0, 0.0), straight_point(20.0, 0.0), straight_point(40.0, 0.0)];
        let (mut widget, mut history, mut selection, id) = setup(points, false);
        widget.point_edit_layer = Some(id);
        widget.selected_points = vec![0, 1];

        widget.delete_selected_points(&mut history, &mut selection);

        assert!(history.can_undo());
        assert!(history.get().active_page().find(id).is_none());
        assert!(selection.is_empty());
        assert!(widget.selected_points.is_empty());
        assert!(widget.point_edit_layer.is_none());
    }

    #[test]
    fn deleting_a_pen_drawn_lines_only_anchor_selection_deletes_the_line() {
        let points = vec![straight_point(0.0, 0.0), straight_point(20.0, 0.0)];
        let (mut widget, mut history, mut selection, id) = setup(points, false);
        widget.point_edit_layer = Some(id);
        widget.selected_points = vec![0];

        widget.delete_selected_points(&mut history, &mut selection);

        assert!(history.get().active_page().find(id).is_none());
        assert!(selection.is_empty());
    }

    #[test]
    fn switching_selected_points_to_mirror_mirrors_the_existing_handle() {
        let mut point = straight_point(20.0, 20.0);
        point.handle_out = Some(Vec2::new(10.0, 0.0));
        point.point_type = PointType::Disconnected;
        let (mut widget, mut history, _selection, id) = setup(vec![point, straight_point(40.0, 40.0)], false);
        widget.point_edit_layer = Some(id);
        widget.selected_points = vec![0];

        widget.apply_point_type(&mut history, id, PointType::Mirror);

        let layer = history.get().active_page().find(id).unwrap();
        let LayerKind::Path { points, .. } = &layer.kind else { panic!() };
        assert_eq!(points[0].point_type, PointType::Mirror);
        assert_eq!(points[0].handle_out, Some(Vec2::new(10.0, 0.0)));
        assert_eq!(points[0].handle_in, Some(Vec2::new(-10.0, 0.0)));
    }

    #[test]
    fn switching_a_handle_less_point_to_mirror_generates_a_default_curve() {
        let (mut widget, mut history, _selection, id) = setup(
            vec![straight_point(0.0, 0.0), straight_point(20.0, 0.0), straight_point(40.0, 0.0)],
            false,
        );
        widget.point_edit_layer = Some(id);
        widget.selected_points = vec![1];

        widget.apply_point_type(&mut history, id, PointType::Mirror);

        let layer = history.get().active_page().find(id).unwrap();
        let LayerKind::Path { points, .. } = &layer.kind else { panic!() };
        assert_eq!(points[1].point_type, PointType::Mirror);
        assert!(points[1].handle_out.is_some(), "Mirror on a plain corner must generate a visible curve");
        assert_eq!(points[1].handle_out, points[1].handle_in.map(|h| -h));
    }

    #[test]
    fn switching_selected_points_to_straight_clears_both_handles() {
        let mut point = straight_point(20.0, 20.0);
        point.handle_in = Some(Vec2::new(-10.0, 0.0));
        point.handle_out = Some(Vec2::new(10.0, 0.0));
        point.point_type = PointType::Mirror;
        let (mut widget, mut history, _selection, id) = setup(vec![point, straight_point(40.0, 40.0)], false);
        widget.point_edit_layer = Some(id);
        widget.selected_points = vec![0];

        widget.apply_point_type(&mut history, id, PointType::Straight);

        let layer = history.get().active_page().find(id).unwrap();
        let LayerKind::Path { points, .. } = &layer.kind else { panic!() };
        assert_eq!(points[0].point_type, PointType::Straight);
        assert_eq!(points[0].handle_in, None);
        assert_eq!(points[0].handle_out, None);
    }

    /// Reproduces the actual crash report: scissoring a closed, filled
    /// Pen-drawn figure open, then rendering it. `draw_layer` used to pass
    /// the layer's fill straight through to `PathShape` regardless of
    /// `closed`; epaint's tessellator debug-asserts that an open path has a
    /// transparent fill ("You asked to fill a path that is not closed"),
    /// which panics (and thus crashes the whole app) in a debug build —
    /// exactly what `cargo run` uses. Exercises the same fill-selection
    /// logic `draw_layer` uses, through the real epaint tessellator, so a
    /// regression here fails loudly instead of silently reintroducing the
    /// panic.
    #[test]
    fn opening_a_filled_closed_path_with_scissors_does_not_panic_when_rendered() {
        let square = vec![
            straight_point(0.0, 0.0),
            straight_point(40.0, 0.0),
            straight_point(40.0, 40.0),
            straight_point(0.0, 40.0),
        ];
        let (mut widget, mut history, mut selection, id) = setup(square, true);

        let layer_before = history.get().active_page().find(id).unwrap();
        assert!(layer_before.style.fill.is_some(), "a Pen-drawn figure is filled by default");

        widget.try_scissor_path(&mut history, &mut selection, Pos2::new(0.0, 0.0), Pos2::ZERO);

        let layer = history.get().active_page().find(id).unwrap();
        let LayerKind::Path { points, closed } = &layer.kind else {
            panic!("expected Path");
        };
        assert!(!closed, "scissoring a closed path should open it");
        assert!(layer.style.fill.is_some(), "cutting shouldn't itself clear the fill");

        let screen_points = flatten_path(points, *closed);
        let fill = layer.style.fill.as_ref().map(Paint::to_color32).unwrap_or(Color32::TRANSPARENT);
        let path_fill = if *closed { fill } else { Color32::TRANSPARENT };
        let shape = egui::epaint::PathShape {
            points: screen_points,
            closed: *closed,
            fill: path_fill,
            stroke: egui::Stroke::NONE.into(),
        };

        let mut tessellator = egui::epaint::Tessellator::new(
            1.0,
            egui::epaint::TessellationOptions::default(),
            [1, 1],
            Vec::new(),
        );
        let mut mesh = egui::epaint::Mesh::default();
        tessellator.tessellate_path(&shape, &mut mesh);
    }

    /// Reproduces cutting a Pen-drawn figure: curved anchors (bezier
    /// handles set, unlike the straight corners the other tests use), a
    /// non-origin `frame.pos` (a Pen path's frame is the bounding box of
    /// wherever it was drawn, not (0,0)), and nested one level inside an
    /// Artboard (so `offset` is also non-zero). Cuts at every anchor and
    /// every segment midpoint, for both an open and a closed path, to probe
    /// for a panic reported when scissoring a Pen-made shape.
    #[test]
    fn cutting_a_pen_drawn_curved_path_at_every_anchor_and_segment_does_not_panic() {
        fn curved_point(x: f32, y: f32) -> PathPoint {
            PathPoint {
                anchor: Pos2::new(x, y),
                handle_in: Some(Vec2::new(-8.0, 3.0)),
                handle_out: Some(Vec2::new(8.0, -3.0)),
                point_type: PointType::Disconnected,
                corner_radius: 0.0,
            }
        }

        for closed in [false, true] {
            let base_points = vec![
                curved_point(0.0, 0.0),
                curved_point(30.0, -10.0),
                curved_point(60.0, 0.0),
                curved_point(60.0, 40.0),
                curved_point(30.0, 50.0),
                curved_point(0.0, 40.0),
            ];
            let n = base_points.len();

            let frame_pos = Pos2::new(150.0, 200.0);
            let artboard_offset = Vec2::new(500.0, 300.0);

            let make_history = || {
                let layer = Layer::new(
                    "Path",
                    Frame { pos: frame_pos, size: Vec2::new(60.0, 50.0), rotation: 0.0 },
                    LayerKind::Path { points: base_points.clone(), closed },
                );
                let path_id = layer.id;
                let mut artboard = Layer::new_artboard(
                    "Artboard",
                    Frame { pos: artboard_offset.to_pos2(), size: Vec2::new(800.0, 600.0), rotation: 0.0 },
                );
                if let LayerKind::Artboard { children, .. } = &mut artboard.kind {
                    children.push(layer);
                }
                let mut doc = Document::new();
                doc.active_page_mut().layers.push(artboard);
                (History::new(doc), path_id)
            };

            // Anchor cuts: rebuild fresh state for each index since cutting
            // mutates/replaces the layer.
            for i in 0..n {
                let (mut history, path_id) = make_history();
                let mut selection = vec![path_id];
                let mut widget = CanvasWidget::default();
                let abs = base_points[i].anchor + frame_pos.to_vec2() + artboard_offset;
                widget.try_scissor_path(&mut history, &mut selection, abs, Pos2::ZERO);
            }

            // Segment-midpoint cuts.
            let last = if closed { n } else { n - 1 };
            for i in 0..last {
                let (mut history, path_id) = make_history();
                let mut selection = vec![path_id];
                let mut widget = CanvasWidget::default();
                let a = base_points[i].anchor;
                let b = base_points[(i + 1) % n].anchor;
                let mid = Pos2::new((a.x + b.x) / 2.0, (a.y + b.y) / 2.0) + frame_pos.to_vec2() + artboard_offset;
                widget.try_scissor_path(&mut history, &mut selection, mid, Pos2::ZERO);
            }
        }
    }

    fn rect_layer(name: &str, x: f32, y: f32, w: f32, h: f32) -> Layer {
        Layer::new(
            name,
            Frame { pos: Pos2::new(x, y), size: Vec2::new(w, h), rotation: 0.0 },
            LayerKind::Rectangle { corner_radius: CornerRadii::ZERO },
        )
    }

    #[test]
    fn collect_marquee_hits_default_is_top_level_intersecting() {
        let a = rect_layer("A", 0.0, 0.0, 10.0, 10.0);
        let b = rect_layer("B", 100.0, 100.0, 10.0, 10.0);
        let layers = vec![a, b];
        let marquee = Rect::from_min_size(Pos2::new(-5.0, -5.0), Vec2::new(20.0, 20.0));
        let mut hits = Vec::new();
        collect_marquee_hits(&layers, Vec2::ZERO, marquee, false, false, &mut hits);
        assert_eq!(hits, vec![layers[0].id]);
    }

    #[test]
    fn collect_marquee_hits_contained_only_excludes_partial_overlap() {
        let a = rect_layer("A", 0.0, 0.0, 10.0, 10.0);
        let layers = vec![a];
        // Marquee only partially covers the layer.
        let marquee = Rect::from_min_size(Pos2::new(-5.0, -5.0), Vec2::new(10.0, 10.0));
        let mut hits = Vec::new();
        collect_marquee_hits(&layers, Vec2::ZERO, marquee, true, false, &mut hits);
        assert!(hits.is_empty(), "partially-overlapped layer should not match contained_only");

        let full_marquee = Rect::from_min_size(Pos2::new(-5.0, -5.0), Vec2::new(20.0, 20.0));
        let mut hits2 = Vec::new();
        collect_marquee_hits(&layers, Vec2::ZERO, full_marquee, true, false, &mut hits2);
        assert_eq!(hits2, vec![layers[0].id]);
    }

    #[test]
    fn collect_marquee_hits_ignore_groups_descends_into_children_only() {
        let child = rect_layer("Child", 2.0, 2.0, 4.0, 4.0);
        let child_id = child.id;
        let group = Layer::new(
            "Group",
            Frame { pos: Pos2::new(0.0, 0.0), size: Vec2::new(10.0, 10.0), rotation: 0.0 },
            LayerKind::Group { children: vec![child] },
        );
        let group_id = group.id;
        let layers = vec![group];
        let marquee = Rect::from_min_size(Pos2::ZERO, Vec2::new(20.0, 20.0));

        let mut default_hits = Vec::new();
        collect_marquee_hits(&layers, Vec2::ZERO, marquee, false, false, &mut default_hits);
        assert_eq!(default_hits, vec![group_id], "without ignore_groups, the group matches as one unit");

        let mut ignoring_hits = Vec::new();
        collect_marquee_hits(&layers, Vec2::ZERO, marquee, false, true, &mut ignoring_hits);
        assert_eq!(ignoring_hits, vec![child_id], "with ignore_groups, only the leaf child matches");
    }

    #[test]
    fn layers_at_point_returns_every_stacked_layer_front_to_back() {
        let bottom = rect_layer("Bottom", 0.0, 0.0, 10.0, 10.0);
        let bottom_id = bottom.id;
        let top = rect_layer("Top", 0.0, 0.0, 10.0, 10.0);
        let top_id = top.id;
        let layers = vec![bottom, top];
        let mut hits = Vec::new();
        layers_at_point(&layers, Vec2::ZERO, Pos2::new(5.0, 5.0), &mut hits);
        assert_eq!(hits, vec![top_id, bottom_id]);
    }

    #[test]
    fn insert_layer_with_hint_lands_inside_the_hinted_group_at_relative_coords() {
        let child = rect_layer("Child", 1.0, 1.0, 2.0, 2.0);
        let group = Layer::new(
            "Group",
            Frame { pos: Pos2::new(50.0, 50.0), size: Vec2::new(20.0, 20.0), rotation: 0.0 },
            LayerKind::Group { children: vec![child] },
        );
        let group_id = group.id;
        let mut page = Page::new("Page 1");
        page.layers.push(group);

        let new_layer = rect_layer("New", 55.0, 55.0, 5.0, 5.0);
        let new_id = new_layer.id;
        // start_doc deliberately outside the group's bounds — the hint
        // should still win over the position-based fallback.
        insert_layer(&mut page, new_layer, Pos2::new(500.0, 500.0), Some(group_id));

        let group = page.find(group_id).unwrap();
        let LayerKind::Group { children } = &group.kind else { unreachable!() };
        let inserted = children.iter().find(|l| l.id == new_id).expect("should be inside the group");
        // Absolute pos (55,55) minus the group's own absolute pos (50,50).
        assert_eq!(inserted.frame.pos, Pos2::new(5.0, 5.0));
    }

    #[test]
    fn distribution_order_sorts_by_position_along_the_axis() {
        let mut page = Page::new("Page 1");
        page.layers.push(rect_layer("C", 90.0, 0.0, 10.0, 10.0));
        page.layers.push(rect_layer("A", 0.0, 0.0, 10.0, 10.0));
        page.layers.push(rect_layer("B", 15.0, 0.0, 10.0, 10.0));
        let ids: Vec<LayerId> = page.layers.iter().map(|l| l.id).collect();

        let order = distribution_order(&page, &ids, DistributeAxis::Horizontal);
        let names: Vec<&str> = order.iter().map(|(id, _)| page.find(*id).unwrap().name.as_str()).collect();
        assert_eq!(names, vec!["A", "B", "C"]);
    }

    #[test]
    fn gap_handle_positions_returns_one_fewer_than_item_count() {
        let mut page = Page::new("Page 1");
        page.layers.push(rect_layer("A", 0.0, 0.0, 10.0, 10.0));
        page.layers.push(rect_layer("B", 15.0, 0.0, 10.0, 10.0));
        page.layers.push(rect_layer("C", 90.0, 0.0, 10.0, 10.0));
        let ids: Vec<LayerId> = page.layers.iter().map(|l| l.id).collect();

        let order = distribution_order(&page, &ids, DistributeAxis::Horizontal);
        let handles = gap_handle_positions(&order, DistributeAxis::Horizontal);
        assert_eq!(handles.len(), 2);
        // First gap midpoint between A's right edge (10) and B's left edge (15).
        assert_eq!(handles[0].x, 12.5);
    }

    #[test]
    fn axis_gap_none_when_intervals_overlap() {
        assert_eq!(axis_gap(0.0, 10.0, 5.0, 15.0), None);
    }

    #[test]
    fn axis_gap_returns_the_facing_edges_when_disjoint() {
        assert_eq!(axis_gap(0.0, 10.0, 20.0, 30.0), Some((10.0, 20.0)));
        assert_eq!(axis_gap(20.0, 30.0, 0.0, 10.0), Some((10.0, 20.0)));
    }
}
