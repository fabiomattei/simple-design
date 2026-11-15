//! Page-space rotate/resize/refit geometry shared by the canvas's drag
//! gestures and `transform_ops.rs`'s numeric transforms.

use egui::{Pos2, Rect, Vec2};

use crate::model::{Frame, Layer, LayerId, LayerKind, Page};
use crate::shapes::rotate_point;

/// A leaf layer's rotation state captured at drag start, in page-space
/// (absolute, pan/zoom-only coordinates) — the same shape as `ResizeLayerInfo`
/// and used the same way (`original_kind` re-derived from every frame, never
/// accumulated from a previous frame's already-rotated state). Built by
/// `collect_rotatable_leaves`, which recurses into `Group`/`Artboard`
/// children instead of adding an entry for the container itself — the
/// mechanism behind "rotating a group bakes the angle into each descendant's
/// own frame" (see `model/layer.rs`'s `Frame::rotation` doc comment).
pub(crate) struct RotateLayerInfo {
    pub(super) id: LayerId,
    pub(super) parent_offset: Vec2,
    pub(super) abs_bounds: Rect,
    pub(super) original_frame: Frame,
    pub(super) original_kind: LayerKind,
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
/// (not inlined in `CanvasWidget::continue_drag`'s match) so it's directly
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
/// the container's own `frame.bounds()`, see `CanvasWidget::paint_selection_outlines`) and, for a
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
/// AABB — see `CanvasWidget::paint_selection_outlines`), but for a
/// rotated `BooleanGroup`/`Group`/`Artboard`'s children — aggregated here
/// via `children_local_bbox` and `refit_container_to_children`, both of
/// which only ever draw/store a plain axis-aligned box, never a tilted one
/// — that slack compounds into a visibly oversized, detached-looking
/// selection box. Uses the standard tight rotated-ellipse AABB formula
/// (half-extent along each axis is `hypot(semi_axis * cos, other_semi_axis
/// * sin)` of the rotation angle) instead.
pub(super) fn tight_rotated_bounds(layer: &Layer) -> Rect {
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
pub(super) fn children_local_bbox(layer: &Layer) -> Option<Rect> {
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
pub(super) fn display_bounds(layer: &Layer) -> Rect {
    match children_local_bbox(layer) {
        Some(bbox) => bbox.translate(layer.frame.pos.to_vec2()),
        None => layer.frame.bounds(),
    }
}

/// A layer's resize state captured at drag start, in page-space (absolute,
/// pan/zoom-only coordinates — not offset by ancestor layer positions).
/// Where Scale mode's resize is anchored — see `DragState::ResizingGroup`'s
/// `scale_anchor` field and `ui/inspector/multi_selection.rs`'s "Scale Layers" panel.
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

pub(crate) struct ResizeLayerInfo {
    pub(super) id: LayerId,
    pub(super) parent_offset: Vec2,
    pub(crate) abs_bounds: Rect,
    /// A clone of the layer's `kind` as it was *before* the drag touched it.
    /// `Path`/`CompoundPath` need this: unlike Rectangle/Oval (whose geometry
    /// IS `frame.bounds()`, always rederived fresh from `abs_bounds` below),
    /// their geometry lives in per-point data with no such single source of
    /// truth. Every frame must recompute from this untouched original —
    /// `transform` below always maps the *original* `start_overall_bounds`
    /// to the current live bounds, so applying it to already-transformed
    /// points from a previous frame would double-scale them.
    pub(super) original_kind: LayerKind,
    /// The layer's rotation before the drag — `Frame::from_bounds` (used to
    /// rebuild `frame` from the resized bounds each frame) always zeroes
    /// `rotation`, so it must be re-applied afterward or a bounding-box
    /// resize would silently un-rotate the layer. This resize is still only
    /// a plain axis-aligned bounding-box scale (not yet a rotation-aware
    /// local-space resize — see the "rotation-aware resize" work), so a
    /// nonzero-rotation shape resized this way keeps its angle but its
    /// bounding box scales in page-space, not along its own tilted axes.
    pub(super) original_rotation: f32,
    /// The stroke width before the drag — used only in Scale mode (see
    /// `CanvasWidget::scaling`) to scale it proportionally with the layer,
    /// same non-compounding "always derive from the untouched original"
    /// convention as `original_kind` above. Captured unconditionally (cheap)
    /// rather than only when scale mode is active, so this struct doesn't
    /// need two slightly different construction sites.
    pub(super) original_stroke_width: Option<f32>,
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
pub(crate) fn collect_resizable_leaves(layer: &Layer, parent_offset: Vec2, out: &mut Vec<ResizeLayerInfo>) {
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
/// Standalone (not inlined in `CanvasWidget::continue_drag`'s match) so
/// it's directly unit-testable, same rationale as `apply_rotation_delta`.
/// Recomputed from each `ResizeLayerInfo`'s untouched `original_kind`/
/// `abs_bounds` every call, never from the layer's own live state — see
/// `ResizeLayerInfo::original_kind`'s doc comment. Does *not* touch any
/// container's own `frame`: `layers` only ever holds real leaves (see
/// `collect_resizable_leaves`), so the caller must follow up with
/// `refit_container_to_children` for every affected top-level container.
pub(crate) fn apply_resize_delta(
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
