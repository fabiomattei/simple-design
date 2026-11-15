//! Per-layer egui texture caches for the canvas renderer: decoded images,
//! rasterized masked groups, procedural fills and shadows, each rebuilt only
//! when its inputs change and evicted once its layer is gone.

use std::collections::HashMap;

use egui::{Pos2, Rect, Vec2};

use crate::model::{ColorAdjust, HalftoneFill, Layer, LayerId, LayerKind, NoiseFill, Page, PatternFill};

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
pub(super) struct CachedMaskTexture {
    mask: Layer,
    content: Vec<Layer>,
    texture: egui::TextureHandle,
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
pub struct MaskedGroupTextureCache(HashMap<LayerId, CachedMaskTexture>);

impl MaskedGroupTextureCache {
    /// Returns the cached (or freshly built) texture for this masked run,
    /// plus the parent-space bounds (`mask.frame.rotated_bounds()`) it
    /// should be drawn into on screen. `page` lets a path-attached `Text`
    /// layer inside `content` resolve its target anywhere on the page, not
    /// just within this masked run's own `mask`/`content` — but since that
    /// means this render can depend on a layer this cache's key (`mask`/
    /// `content` clone-equality) doesn't track, any masked run containing
    /// such a layer always rebuilds rather than risk a stale texture if its
    /// attachment target moves elsewhere on the page.
    pub(super) fn get_or_build(&mut self, ctx: &egui::Context, mask: &Layer, content: &[&Layer], page: &Page) -> Option<(egui::TextureHandle, Rect)> {
        let bounds = mask.frame.rotated_bounds();
        let up_to_date = !content.iter().any(|l| contains_path_attached_text(l))
            && self.0.get(&mask.id).is_some_and(|cached| {
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
            crate::masking::composite_masked_run(&mut pixmap, mask, content, render_offset, 1.0, |px, l, off, op| {
                crate::export::draw_layer_with_shadows(px, l, &page.layers, off, op)
            });
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
    pub(super) fn evict_stale(&mut self, live_ids: &std::collections::HashSet<LayerId>) {
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
/// comment for the overall shared-with-`export.rs` design) and cached the
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
            let (silhouette, bounds) = crate::export::render_layer_plain(layer, &page.layers)?;
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
