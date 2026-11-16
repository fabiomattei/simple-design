//! Per-layer egui texture caches for the canvas renderer: decoded images,
//! rasterized masked groups, procedural fills and shadows, each rebuilt only
//! when its inputs change and evicted once its layer is gone.

use std::collections::HashMap;

use egui::{Pos2, Rect, Vec2};

use crate::model::{ColorAdjust, Gradient, HalftoneFill, Layer, LayerId, LayerKind, NoiseFill, Page, PatternFill};

/// One decoded-and-adjusted `Image` layer's egui texture, plus the
/// `(version, color_adjust)` pair it was built from — cheap to compare every
/// frame, so a texture is only re-decoded/re-uploaded when either actually
/// changes (a destructive edit bumps `version`; a Color Adjust slider tweak
/// changes `color_adjust`), not on every redraw.
pub(super) struct CachedImageTexture {
    version: LayerId,
    color_adjust: ColorAdjust,
    texture: egui::TextureHandle,
}

/// Per-canvas cache of `Image` layer textures, keyed by layer id. See
/// `CachedImageTexture` for the invalidation rule.
#[derive(Default)]
pub struct ImageTextureCache(HashMap<LayerId, CachedImageTexture>);

impl ImageTextureCache {
    pub(super) fn get_or_load(
        &mut self,
        ctx: &egui::Context,
        layer_id: LayerId,
        encoded: &[u8],
        version: LayerId,
        color_adjust: ColorAdjust,
    ) -> Option<egui::TextureHandle> {
        if let Some(cached) = self.0.get(&layer_id)
            && cached.version == version && cached.color_adjust == color_adjust {
                return Some(cached.texture.clone());
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
    pub(super) fn evict_stale(&mut self, live_ids: &std::collections::HashSet<LayerId>) {
        self.0.retain(|id, _| live_ids.contains(id));
    }
}

/// One masked run's rasterized composite (see `masking::composite_masked_run`),
/// cached as an egui texture keyed by the mask layer's id plus the run's
/// `segment` (see `masking::RenderUnit::Masked`) — the same
/// texture-cache-over-a-CPU-rasterizer pattern as `ImageTextureCache` above,
/// since egui's immediate-mode `Painter` has no way to clip to an arbitrary
/// shape itself (only axis-aligned clip rects). `mask`/`content` are cloned
/// so a cheap `PartialEq` (both `Layer` derives it) tells us whether
/// anything relevant changed since the last build, without needing a
/// separate dirty-flag/version scheme threaded through every edit path that
/// could touch a masked layer.
///
/// Built with the run's ambient (ancestor-accumulated) `opacity` passed to
/// each content layer, exactly as `export/` composites a masked run, rather
/// than built opaque and faded as a whole at draw time — the two differ
/// wherever content layers overlap. That makes `opacity` part of the key,
/// so dragging an ancestor's opacity slider rebuilds the texture.
///
/// Covers `region` (parent space) at `render_scale` pixels per document unit
/// (see `mask_texture_region`) so it stays sharp when zoomed in; both are
/// part of the key.
pub(super) struct CachedMaskTexture {
    mask: Layer,
    content: Vec<Layer>,
    opacity: f32,
    region: Rect,
    render_scale: f32,
    texture: egui::TextureHandle,
}

/// Hard cap on a masked-group texture's side length, so a texture can never
/// be unbounded — same rationale as `NOISE_TEXTURE_MAX`. With
/// `mask_texture_region` cropping large masks to the view, it's only reached
/// on a very large window, where the texture renders slightly below screen
/// resolution rather than failing.
pub(super) const MASK_TEXTURE_MAX: f32 = 4096.0;

/// Texels per grid cell a cropped masked-group texture's area snaps to, so
/// panning only rebuilds it when the view crosses a grid line.
const MASK_TEXTURE_TILE: f32 = 256.0;

/// Margin around the view, as a fraction of its size per side, that a
/// cropped masked-group texture also covers — small pans stay inside it.
const MASK_TEXTURE_MARGIN: f32 = 0.125;

/// The parent-space area to render a masked run's texture over, and its
/// pixels per document unit, for `pixels_per_unit` (canvas zoom times the
/// display's pixels per point). The scale is the next power of two at or
/// above that, so zooming only rebuilds when it crosses one. If all of
/// `bounds` (the mask's area) fits within `MASK_TEXTURE_MAX` at that scale,
/// the area is all of `bounds`, so panning never rebuilds. Otherwise only
/// the part near `visible` (the view, in parent space) is rendered: grown by
/// `MASK_TEXTURE_MARGIN`, snapped outward to a `MASK_TEXTURE_TILE` grid,
/// clipped to `bounds` — full resolution, rebuilt only when panning crosses
/// a grid line.
pub(super) fn mask_texture_region(bounds: Rect, visible: Rect, pixels_per_unit: f32) -> (Rect, f32) {
    let stepped = 2f32.powf(pixels_per_unit.max(1e-3).log2().ceil());
    let longest_side = |rect: Rect| rect.width().max(rect.height()).max(1.0);
    let region = if longest_side(bounds) * stepped <= MASK_TEXTURE_MAX {
        bounds
    } else {
        let tile = MASK_TEXTURE_TILE / stepped;
        let near = visible.expand2(visible.size() * MASK_TEXTURE_MARGIN);
        let snap = |v: f32, round: fn(f32) -> f32| round(v / tile) * tile;
        let snapped = Rect::from_min_max(
            Pos2::new(snap(near.min.x, f32::floor), snap(near.min.y, f32::floor)),
            Pos2::new(snap(near.max.x, f32::ceil), snap(near.max.y, f32::ceil)),
        );
        snapped.intersect(bounds)
    };
    (region, stepped.min(MASK_TEXTURE_MAX / longest_side(region)))
}

/// Whether `layer` or any descendant is a `Text` layer with a
/// `path_attachment` set — used by cache-by-equality render caches
/// (`MaskedGroupTextureCache`, `ShadowTextureCache`) to force a rebuild
/// instead of risking a stale render when the attachment's target (which
/// the cache key itself doesn't track) moves elsewhere on the page.
pub(super) fn contains_path_attached_text(layer: &Layer) -> bool {
    if matches!(&layer.kind, LayerKind::Text { path_attachment: Some(_), .. }) {
        return true;
    }
    layer.kind.children().is_some_and(|children| children.iter().any(contains_path_attached_text))
}

#[derive(Default)]
pub struct MaskedGroupTextureCache {
    textures: HashMap<(LayerId, usize), CachedMaskTexture>,
    /// Keys `get_or_build` served since the last `evict_unused` — see there.
    used: std::collections::HashSet<(LayerId, usize)>,
}

impl MaskedGroupTextureCache {
    /// Returns the cached (or freshly built) texture for this masked run,
    /// plus the parent-space area it should be drawn into on screen — all of
    /// `masking::masked_area`, or the part of it near `visible` for a large
    /// mask at high zoom (see `mask_texture_region`, which `pixels_per_unit`
    /// and `visible` feed). `None` if no part of the mask is in that area. `page` lets a
    /// path-attached `Text` layer inside `content` resolve its target anywhere on the page, not
    /// just within this masked run's own `mask`/`content` — but since that
    /// means this render can depend on a layer this cache's key (`mask`/
    /// `content` clone-equality) doesn't track, any masked run containing
    /// such a layer always rebuilds rather than risk a stale texture if its
    /// attachment target moves elsewhere on the page.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn get_or_build(
        &mut self,
        ctx: &egui::Context,
        mask: &Layer,
        content: &[&Layer],
        segment: usize,
        opacity: f32,
        pixels_per_unit: f32,
        visible: Rect,
        page: &Page,
    ) -> Option<(egui::TextureHandle, Rect)> {
        let key = (mask.id, segment);
        let (region, render_scale) = mask_texture_region(crate::masking::masked_area(mask, content), visible, pixels_per_unit);
        if !region.is_positive() {
            return None;
        }
        self.used.insert(key);
        let up_to_date = !content.iter().any(|l| contains_path_attached_text(l))
            && self.textures.get(&key).is_some_and(|cached| {
                cached.mask == *mask
                    && cached.opacity == opacity
                    && cached.region == region
                    && cached.render_scale == render_scale
                    && cached.content.len() == content.len()
                    && cached.content.iter().zip(content).all(|(a, b)| a == *b)
            });
        if !up_to_date {
            let width = (region.width() * render_scale).round().max(1.0) as u32;
            let height = (region.height() * render_scale).round().max(1.0) as u32;
            let mut pixmap = tiny_skia::Pixmap::new(width, height)?;
            // Shifts parent-space coordinates so `region.min` lands at the
            // scratch pixmap's `(0, 0)` — same idea as
            // `export::render_layer`'s `offset` computation (in document
            // units; `render_scale` is applied by `draw_layer`).
            let render_offset = Vec2::new(-region.min.x, -region.min.y);
            crate::masking::composite_masked_run(&mut pixmap, mask, content, render_offset, opacity, render_scale, |px, l, off, op| {
                crate::export::draw_layer_with_shadows(px, l, &page.layers, off, op, render_scale)
            });
            let color_image = egui::ColorImage::from_rgba_premultiplied([width as usize, height as usize], pixmap.data());
            let texture = ctx.load_texture(format!("mask-group-{}-{segment}", mask.id), color_image, egui::TextureOptions::LINEAR);
            self.textures.insert(
                key,
                CachedMaskTexture {
                    mask: mask.clone(),
                    content: content.iter().map(|l| (*l).clone()).collect(),
                    opacity,
                    region,
                    render_scale,
                    texture,
                },
            );
        }
        self.textures.get(&key).map(|cached| (cached.texture.clone(), region))
    }

    /// Drops every texture not drawn since the last call — called once per
    /// frame after the document is painted. Unlike the other caches'
    /// `evict_stale` (keyed by layer id alone), this also drops pieces of a
    /// masked run that no longer exist while the mask itself does (e.g. an
    /// `mask.ignore_underlying` layer that split it was removed), and runs that aren't
    /// currently drawn (they rebuild when shown again).
    pub(super) fn evict_unused(&mut self) {
        let used = std::mem::take(&mut self.used);
        self.textures.retain(|key, _| used.contains(key));
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
pub(super) struct CachedNoiseTexture {
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
pub(super) const NOISE_TEXTURE_QUANTUM: u32 = 32;
/// Hard cap on a noise texture's side length, so a very large or heavily
/// zoomed-in shape doesn't allocate an unbounded GPU texture.
pub(super) const NOISE_TEXTURE_MAX: u32 = 1024;

impl NoiseTextureCache {
    /// `screen_size` is the shape's current on-screen footprint (pixels),
    /// used only to pick a crisp-enough texture resolution — it does not
    /// affect the grain pattern itself, which is defined purely in
    /// `local_size`/`fill.scale` document-unit space (see `noise_fill::sample`).
    pub(super) fn get_or_build(&mut self, ctx: &egui::Context, layer_id: LayerId, fill: &NoiseFill, local_size: Vec2, screen_size: Vec2) -> Option<egui::TextureHandle> {
        let quantize = |v: f32| ((v.ceil() as u32).max(1)).div_ceil(NOISE_TEXTURE_QUANTUM).max(1) * NOISE_TEXTURE_QUANTUM;
        let pixel_size = (quantize(screen_size.x).min(NOISE_TEXTURE_MAX), quantize(screen_size.y).min(NOISE_TEXTURE_MAX));
        if let Some(cached) = self.0.get(&layer_id)
            && cached.fill == *fill && cached.local_size == local_size && cached.pixel_size == pixel_size {
                return Some(cached.texture.clone());
            }
        let (tw, th) = pixel_size;
        let mut rgba = Vec::with_capacity((tw * th) as usize * 4);
        for j in 0..th {
            for i in 0..tw {
                let frac = Pos2::new((i as f32 + 0.5) / tw as f32, (j as f32 + 0.5) / th as f32);
                let local_point = Pos2::new(frac.x * local_size.x, frac.y * local_size.y);
                let c = crate::noise_fill::sample(fill, local_point, 1.0);
                rgba.extend_from_slice(&c.to_array());
            }
        }
        let color_image = egui::ColorImage::from_rgba_premultiplied([tw as usize, th as usize], &rgba);
        let texture = ctx.load_texture(format!("noise-layer-{layer_id}"), color_image, egui::TextureOptions::NEAREST);
        let handle = texture.clone();
        self.0.insert(layer_id, CachedNoiseTexture { fill: *fill, local_size, pixel_size, texture });
        Some(handle)
    }

    /// See `ImageTextureCache::evict_stale` — same rationale.
    pub(super) fn evict_stale(&mut self, live_ids: &std::collections::HashSet<LayerId>) {
        self.0.retain(|id, _| live_ids.contains(id));
    }
}

/// `Paint::Halftone`'s sibling of `CachedNoiseTexture`/`NoiseTextureCache` —
/// identical shape, sampling `halftone_fill::sample` instead of
/// `noise_fill::sample`.
pub(super) struct CachedHalftoneTexture {
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
    pub(super) fn get_or_build(&mut self, ctx: &egui::Context, layer_id: LayerId, fill: &HalftoneFill, local_size: Vec2, screen_size: Vec2) -> Option<egui::TextureHandle> {
        let quantize = |v: f32| ((v.ceil() as u32).max(1)).div_ceil(NOISE_TEXTURE_QUANTUM).max(1) * NOISE_TEXTURE_QUANTUM;
        let pixel_size = (quantize(screen_size.x).min(NOISE_TEXTURE_MAX), quantize(screen_size.y).min(NOISE_TEXTURE_MAX));
        if let Some(cached) = self.0.get(&layer_id)
            && cached.fill == *fill && cached.local_size == local_size && cached.pixel_size == pixel_size {
                return Some(cached.texture.clone());
            }
        let (tw, th) = pixel_size;
        let mut rgba = Vec::with_capacity((tw * th) as usize * 4);
        for j in 0..th {
            for i in 0..tw {
                let frac = Pos2::new((i as f32 + 0.5) / tw as f32, (j as f32 + 0.5) / th as f32);
                let local_point = Pos2::new(frac.x * local_size.x, frac.y * local_size.y);
                let c = crate::halftone_fill::sample(fill, local_point, 1.0);
                rgba.extend_from_slice(&c.to_array());
            }
        }
        let color_image = egui::ColorImage::from_rgba_premultiplied([tw as usize, th as usize], &rgba);
        let texture = ctx.load_texture(format!("halftone-layer-{layer_id}"), color_image, egui::TextureOptions::NEAREST);
        let handle = texture.clone();
        self.0.insert(layer_id, CachedHalftoneTexture { fill: *fill, local_size, pixel_size, texture });
        Some(handle)
    }

    /// See `ImageTextureCache::evict_stale` — same rationale.
    pub(super) fn evict_stale(&mut self, live_ids: &std::collections::HashSet<LayerId>) {
        self.0.retain(|id, _| live_ids.contains(id));
    }
}

/// `Paint::Gradient` fill's sibling of `CachedNoiseTexture`/`NoiseTextureCache`
/// — identical shape, sampling `Gradient::sample_in_rect` over the layer's
/// local bounds. Rasterized rather than drawn as per-vertex mesh colors
/// because vertex-color interpolation is only exact for a two-stop linear
/// gradient: radial gradients and extra stops need per-pixel sampling to
/// match `export/`'s tiny-skia shader.
pub(super) struct CachedGradientTexture {
    gradient: Gradient,
    local_size: Vec2,
    pixel_size: (u32, u32),
    texture: egui::TextureHandle,
}

#[derive(Default)]
pub struct GradientTextureCache(HashMap<LayerId, CachedGradientTexture>);

/// A gradient texture's side length for an on-screen length of `screen_len`
/// pixels: the next power of two at or above it, capped at
/// `NOISE_TEXTURE_MAX`. Coarser than noise's `NOISE_TEXTURE_QUANTUM` steps on
/// purpose — a large gradient (e.g. a full-artboard radial glow) costs
/// millions of samples to rebuild, and 32px steps rebuilt it on nearly every
/// frame of a zoom gesture; a smooth gradient stretched up to 2x by
/// `TextureOptions::LINEAR` looks the same, unlike noise grain.
pub(super) fn gradient_texture_side(screen_len: f32) -> u32 {
    (screen_len.ceil() as u32).max(1).next_power_of_two().min(NOISE_TEXTURE_MAX)
}

impl GradientTextureCache {
    /// See `NoiseTextureCache::get_or_build` — same per-pixel-sample-function
    /// shape, but with `gradient_texture_side`'s power-of-two resolution steps.
    pub(super) fn get_or_build(&mut self, ctx: &egui::Context, layer_id: LayerId, gradient: &Gradient, local_size: Vec2, screen_size: Vec2) -> Option<egui::TextureHandle> {
        let pixel_size = (gradient_texture_side(screen_size.x), gradient_texture_side(screen_size.y));
        if let Some(cached) = self.0.get(&layer_id)
            && cached.gradient == *gradient && cached.local_size == local_size && cached.pixel_size == pixel_size {
                return Some(cached.texture.clone());
            }
        let (tw, th) = pixel_size;
        let local_rect = Rect::from_min_size(Pos2::ZERO, local_size);
        let mut rgba = Vec::with_capacity((tw * th) as usize * 4);
        for j in 0..th {
            for i in 0..tw {
                let frac = Pos2::new((i as f32 + 0.5) / tw as f32, (j as f32 + 0.5) / th as f32);
                let local_point = Pos2::new(frac.x * local_size.x, frac.y * local_size.y);
                let c = gradient.sample_in_rect(local_point, local_rect);
                rgba.extend_from_slice(&c.to_array());
            }
        }
        let color_image = egui::ColorImage::from_rgba_premultiplied([tw as usize, th as usize], &rgba);
        let texture = ctx.load_texture(format!("gradient-layer-{layer_id}"), color_image, egui::TextureOptions::LINEAR);
        let handle = texture.clone();
        self.0.insert(layer_id, CachedGradientTexture { gradient: gradient.clone(), local_size, pixel_size, texture });
        Some(handle)
    }

    /// See `ImageTextureCache::evict_stale` — same rationale.
    pub(super) fn evict_stale(&mut self, live_ids: &std::collections::HashSet<LayerId>) {
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
pub(super) struct CachedPatternTexture {
    encoded: Vec<u8>,
    aspect: f32,
    texture: egui::TextureHandle,
}

#[derive(Default)]
pub struct PatternTextureCache(HashMap<LayerId, CachedPatternTexture>);

impl PatternTextureCache {
    pub(super) fn get_or_build(&mut self, ctx: &egui::Context, layer_id: LayerId, fill: &PatternFill) -> Option<(egui::TextureHandle, f32)> {
        if let Some(cached) = self.0.get(&layer_id)
            && cached.encoded == fill.encoded {
                return Some((cached.texture.clone(), cached.aspect));
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
    pub(super) fn evict_stale(&mut self, live_ids: &std::collections::HashSet<LayerId>) {
        self.0.retain(|id, _| live_ids.contains(id));
    }
}

/// One layer's rendered drop/inner shadow textures (`Style::shadows`/
/// `inner_shadows`), keyed by layer id. Built from the layer's own
/// silhouette (`export::render_layer_plain` — always shadow-free, so this
/// never recurses into a child's own shadow; see `shadow.rs`'s module doc
/// comment for the overall shared-with-`export/` design) and cached the
/// same cheap-`PartialEq`-guarded way as `CachedMaskTexture`. Each shadow in
/// the stack gets its own texture (rather than compositing the stack into
/// one) since outer shadows can each be a different size/position (padding
/// depends on that shadow's own blur/spread) — simpler than merge-compositing
/// them for one extra draw call per stacked shadow, which in practice is
/// rarely more than two or three.
pub(super) struct CachedShadowTextures {
    pub(super) layer: Layer,
    /// Each outer shadow's texture plus where it lands, in the same
    /// parent-relative space as `layer.frame` (i.e. before `to_screen`).
    pub(super) outer: Vec<(egui::TextureHandle, Rect)>,
    /// Each inner shadow's texture plus where it lands — always
    /// `layer.frame.rotated_bounds()`, repeated per shadow so draw-site code
    /// doesn't need to special-case inner vs. outer.
    pub(super) inner: Vec<(egui::TextureHandle, Rect)>,
}

#[derive(Default)]
pub struct ShadowTextureCache(HashMap<LayerId, CachedShadowTextures>);

impl ShadowTextureCache {
    /// `None` if `layer` has no shadows at all (nothing to draw) or its
    /// silhouette failed to rasterize (zero-size layer). `page` lets a
    /// path-attached `Text` layer (`layer` itself, or nested inside it)
    /// resolve its target anywhere on the page for the shadow's silhouette,
    /// same as the live render — but since that means the silhouette can
    /// depend on a layer this cache's key (`cached.layer` equality) doesn't
    /// track, `layer` containing one always rebuilds rather than risk a
    /// stale (unattached-shaped) shadow if the target moves elsewhere.
    pub(super) fn get_or_build(&mut self, ctx: &egui::Context, layer: &Layer, page: &Page) -> Option<&CachedShadowTextures> {
        let has_outer = !layer.style.shadows.is_empty();
        let has_inner = !layer.style.inner_shadows.is_empty();
        if !has_outer && !has_inner {
            self.0.remove(&layer.id);
            return None;
        }
        let up_to_date = !contains_path_attached_text(layer) && self.0.get(&layer.id).is_some_and(|cached| cached.layer == *layer);
        if !up_to_date {
            let (silhouette, bounds) = crate::export::render_layer_plain(layer, &page.layers, 1.0)?;
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
    pub(super) fn evict_stale(&mut self, live_ids: &std::collections::HashSet<LayerId>) {
        self.0.retain(|id, _| live_ids.contains(id));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn square(side: f32) -> Rect {
        Rect::from_min_size(Pos2::ZERO, Vec2::splat(side))
    }

    #[test]
    fn mask_texture_region_steps_scale_up_to_the_next_power_of_two() {
        let view = square(1000.0);
        assert_eq!(mask_texture_region(square(100.0), view, 1.0), (square(100.0), 1.0));
        assert_eq!(mask_texture_region(square(100.0), view, 1.5).1, 2.0);
        assert_eq!(mask_texture_region(square(100.0), view, 3.0).1, 4.0);
        assert_eq!(mask_texture_region(square(100.0), view, 0.3).1, 0.5);
    }

    #[test]
    fn mask_texture_region_crops_a_large_mask_to_the_view_at_full_resolution() {
        // 1000 units at 8 px/unit would be 8000px: render only near the view instead.
        let view = Rect::from_min_size(Pos2::new(400.0, 400.0), Vec2::splat(100.0));
        let (region, render_scale) = mask_texture_region(square(1000.0), view, 8.0);
        assert_eq!(render_scale, 8.0, "full resolution, not capped down");
        assert!(region.contains_rect(view), "covers the whole view: {region:?}");
        assert!(region.width() * render_scale <= MASK_TEXTURE_MAX);
        // A small pan stays inside the margin and grid, so the area (and the texture) is reused.
        let panned = mask_texture_region(square(1000.0), view.translate(Vec2::new(5.0, 5.0)), 8.0).0;
        assert_eq!(panned, region);
    }

    #[test]
    fn gradient_texture_side_steps_up_to_the_next_power_of_two_and_caps() {
        assert_eq!(gradient_texture_side(0.0), 1);
        assert_eq!(gradient_texture_side(300.0), 512);
        assert_eq!(gradient_texture_side(512.0), 512);
        assert_eq!(gradient_texture_side(513.0), 1024);
        assert_eq!(gradient_texture_side(5000.0), NOISE_TEXTURE_MAX);
    }

    #[test]
    fn mask_texture_region_is_empty_when_the_mask_is_off_screen() {
        let view = Rect::from_min_size(Pos2::new(5000.0, 5000.0), Vec2::splat(100.0));
        assert!(!mask_texture_region(square(1000.0), view, 8.0).0.is_positive());
    }
}
