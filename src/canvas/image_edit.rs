//! "Edit Image" mode: per-image Selection/Magic Wand masks feeding Crop,
//! Fill and Delete, scoped to one `Image` layer at a time.

use egui::{Color32, Pos2, Rect};

use crate::history::History;
use crate::model::{LayerId, LayerKind};

use super::{CanvasWidget, DragState};

impl CanvasWidget {
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
        if let Some(layer) = history.mutate().active_page_mut().find_mut(layer_id)
            && let LayerKind::Image { encoded, .. } = &layer.kind
                && let Some(decoded) = crate::image_ops::decode(encoded) {
                    let cropped = crate::image_ops::crop_to_rect(&decoded, x0, y0, w, h);
                    crate::image_ops::apply_cropped_image(layer, &cropped, x0, y0);
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
        if let Some(layer) = history.mutate().active_page_mut().find_mut(layer_id)
            && let LayerKind::Image { encoded, version, .. } = &mut layer.kind
                && let Some(mut decoded) = crate::image_ops::decode(encoded) {
                    crate::image_ops::fill_mask(&mut decoded, &mask, [color.r(), color.g(), color.b()]);
                    *encoded = crate::image_ops::encode_png(&decoded);
                    *version = LayerId::new_v4();
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
        if let Some(layer) = history.mutate().active_page_mut().find_mut(layer_id)
            && let LayerKind::Image { encoded, version, .. } = &mut layer.kind
                && let Some(mut decoded) = crate::image_ops::decode(encoded) {
                    crate::image_ops::clear_mask(&mut decoded, &mask);
                    *encoded = crate::image_ops::encode_png(&decoded);
                    *version = LayerId::new_v4();
                }
        self.clear_image_edit_selection();
    }

    /// Doc-space bounds (already offset by any ancestor Group/Artboard) of
    /// the layer currently being image-edited, plus its pixel dimensions —
    /// used to map mouse positions into the mask's pixel space.
    pub(super) fn image_edit_doc_bounds(&self, history: &History) -> Option<(LayerId, Rect, u32, u32)> {
        let edit = self.image_edit.as_ref()?;
        let page = history.get().active_page();
        let layer = page.find(edit.layer_id)?;
        let offset = page.absolute_offset(edit.layer_id)?;
        Some((edit.layer_id, layer.frame.bounds().translate(offset), edit.width, edit.height))
    }
}

/// A per-image "Edit Image" mode (Selection + Magic Wand, feeding
/// Crop/Fill/Delete), scoped to one `Image` layer at a time. Entered via the
/// inspector (`CanvasWidget::begin_image_edit`); while active, canvas
/// clicks/drags are captured for selection instead of the normal
/// move/resize/marquee behavior (see the `image_edit`-gated branch in
/// `CanvasWidget::begin_drag`).
pub(super) struct ImageEditState {
    pub(super) layer_id: LayerId,
    /// Magic Wand color-distance tolerance, `0..=100` (see
    /// `image_ops::magic_wand_mask`).
    pub(super) tolerance: f32,
    /// Row-major `width * height` selection mask in the image's own pixel
    /// space, captured at `begin_image_edit` — a destructive edit
    /// (Crop/Fill/Delete) always ends edit mode rather than trying to remap
    /// a stale mask onto possibly-new dimensions.
    pub(super) mask: Vec<bool>,
    pub(super) width: u32,
    pub(super) height: u32,
    /// The color the inspector's "Fill" button last used/will use next —
    /// kept here (rather than in the stateless `ui/inspector.rs`) so it
    /// persists across frames alongside the rest of the edit session.
    pub(super) fill_color: Color32,
    /// Cached translucent overlay texture visualizing `mask`; only rebuilt
    /// when `overlay_stale` (set on every mask mutation), since re-uploading
    /// a mask the size of a large photo every frame would be far too slow.
    pub(super) overlay: Option<egui::TextureHandle>,
    pub(super) overlay_stale: bool,
}

impl ImageEditState {
    pub(super) fn mark_dirty(&mut self) {
        self.overlay_stale = true;
    }

    pub(super) fn has_selection(&self) -> bool {
        self.mask.iter().any(|&m| m)
    }
}

/// Maps a doc-space point into an edited image's own pixel space, clamped to
/// its bounds (rather than `None`) so a drag that ends slightly outside the
/// image's edge — very easy to do at typical zoom levels — still resolves to
/// the nearest edge pixel instead of silently doing nothing.
pub(super) fn doc_to_pixel(doc_pos: Pos2, bounds_doc: Rect, width: u32, height: u32) -> Option<(u32, u32)> {
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
pub(super) fn rect_mask(width: u32, height: u32, x0: u32, y0: u32, x1: u32, y1: u32) -> Vec<bool> {
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
pub(super) fn merge_mask(base: &mut [bool], delta: &[bool], subtract: bool) {
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
pub(super) fn mask_bbox(mask: &[bool], width: u32) -> Option<(u32, u32, u32, u32)> {
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
pub(super) fn build_mask_overlay(mask: &[bool], width: u32, height: u32) -> egui::ColorImage {
    let selected = Color32::from_rgba_unmultiplied(0, 90, 158, 120);
    let pixels: Vec<Color32> = mask.iter().map(|&m| if m { selected } else { Color32::TRANSPARENT }).collect();
    egui::ColorImage::new([width as usize, height as usize], pixels)
}
