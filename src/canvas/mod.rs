
use egui::{Color32, Pos2, Rect, Sense, Stroke as EguiStroke, Ui, Vec2};

use crate::alignment::DistributeAxis;
use crate::clipboard;
use crate::grouping;
use crate::history::History;
use crate::model::{
    Frame, Guide, GuideOrientation, Layer, LayerId, LayerKind, Page, Paint, PathPoint, PointType, Style, TextAlign,
    TextFont, TextResize, VerticalAlign,
};
use crate::shapes::{ellipse_points, rotate_point, rotated_corners};
use crate::text_layout::{self, TextStyleParams};
use crate::tools::Tool;

mod draw_layer;
mod fill_mesh;
mod hit_test;
mod image_edit;
mod layer_creation;
mod path_editing;
mod path_flatten;
mod rulers_guides;
mod snapping;
mod texture_caches;
mod transform_geometry;
#[cfg(test)]
mod ui_gesture_tests;

use draw_layer::draw_children;
use hit_test::{collect_marquee_hits, distribution_order, gap_handle_positions, hit_test, layers_at_point};
use image_edit::{ImageEditState, build_mask_overlay, doc_to_pixel, merge_mask, rect_mask};
use layer_creation::{insert_layer, new_layer_for_tool};
use path_editing::{draw_pen_preview, normalize_path_frame, read_path};
use rulers_guides::{draw_guide_line, draw_guides, draw_pixel_grid, draw_rulers};
use snapping::{
    build_snap_candidates, drag_exclude_ids, draw_distance_measurement, draw_snap_lines, snap_bounds_delta,
    snap_guide_axis, snap_point,
};
use texture_caches::{
    HalftoneTextureCache, ImageTextureCache, MaskedGroupTextureCache, NoiseTextureCache, PatternTextureCache,
    ShadowTextureCache,
};
use transform_geometry::{
    ResizeLayerInfo, RotateLayerInfo, apply_resize_delta, collect_resizable_leaves, display_bounds,
};

pub(crate) use path_flatten::flatten_path;
pub(crate) use layer_creation::apply_text_auto_resize;
pub use transform_geometry::ScaleAnchor;
pub(crate) use transform_geometry::{apply_rotation_delta, collect_rotatable_leaves, refit_container_to_children};

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
    /// The on-screen size (in points) of `canvas_rect` as of last frame's
    /// `ui()` call — the piece of doc-space actually visible is
    /// `last_canvas_size / zoom`. One frame stale for panels drawn earlier
    /// in the same pass (e.g. `ui/minimap_panel.rs`'s viewport rectangle),
    /// which is imperceptible since it only changes on window resize.
    pub last_canvas_size: Vec2,
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
            last_canvas_size: Vec2::ZERO,
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
        if let Some((Some(parent_id), _)) = grouping::parent_and_index(&page.layers, shape_id)
            && matches!(page.find(parent_id).map(|l| &l.kind), Some(LayerKind::BooleanGroup { .. })) {
                return None;
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
                path_attachment: None,
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

    fn to_screen(&self, origin: Pos2, p: Pos2) -> Pos2 {
        origin + self.pan + p.to_vec2() * self.zoom
    }

    fn to_doc(&self, origin: Pos2, p: Pos2) -> Pos2 {
        ((p - origin - self.pan) / self.zoom).to_pos2()
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
        self.last_canvas_size = canvas_rect.size();
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
        if !dropped_paths.is_empty()
            && let Some(drop_pos) = ui.ctx().input(|i| i.pointer.interact_pos())
                && canvas_rect.contains(drop_pos) {
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
            } else if response.clicked() && ui.input(|i| i.modifiers.alt)
                && let Some(mouse) = response.interact_pointer_pos() {
                    let doc_pos = self.to_doc(origin, mouse);
                    self.try_delete_path_point(history, selection, doc_pos, origin);
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
        if *tool == Tool::Select && response.double_clicked()
            && let Some(mouse) = response.interact_pointer_pos() {
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
                    } else if is_labelable_shape
                        && let Some(text_id) = self.add_shape_label(history, id) {
                            *selection = vec![text_id];
                        }
                }
            }

        if *tool == Tool::Scissors && response.clicked()
            && let Some(mouse) = response.interact_pointer_pos() {
                let doc_pos = self.to_doc(origin, mouse);
                self.try_scissor_path(history, selection, doc_pos, origin);
            }

        canvas_painter.rect_filled(canvas_rect, 0.0, crate::ui::theme::canvas_colors(ui.visuals().dark_mode).backdrop);
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
            page,
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
        if let Some(edit) = &self.image_edit
            && let (Some(bounds_layer), Some(offset)) =
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
                if let Some(mp) = hit_test_pos
                    && mp.distance(p) <= HANDLE_HIT_RADIUS {
                        hovered_handle = Some(h);
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
                        if let Some(mp) = hit_test_pos
                            && mp.distance(handle_screen) <= HANDLE_HIT_RADIUS {
                                hovered_point = Some((i, part));
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
                    if let Some(mp) = hit_test_pos
                        && mp.distance(anchor_screen) <= HANDLE_HIT_RADIUS {
                            hovered_point = Some((i, PathPart::Anchor));
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
        if *tool == Tool::Select && matches!(self.drag, DragState::None)
            && let Some((dist_ids, axis)) = self.last_distributed.clone() {
                let mut a = dist_ids.clone();
                let mut b = selection.clone();
                a.sort();
                b.sort();
                if a == b && a.len() >= 3 {
                    gap_handle_order = distribution_order(page, &dist_ids, axis);
                    for (i, doc_pos) in gap_handle_positions(&gap_handle_order, axis).into_iter().enumerate() {
                        let p = self.to_screen(origin, doc_pos);
                        if let Some(mp) = hit_test_pos
                            && mp.distance(p) <= HANDLE_HIT_RADIUS {
                                hovered_gap_handle = Some(i);
                            }
                        let hl = hovered_gap_handle == Some(i);
                        painter.circle(p, if hl { 5.0 } else { 3.5 }, SELECTION_COLOR, EguiStroke::new(1.0, Color32::WHITE));
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
        if *tool == Tool::Select && matches!(self.drag, DragState::None) && ui.input(|i| i.modifiers.alt)
            && let Some(sel_bounds) = selected_layers
                .iter()
                .map(|(l, o)| l.frame.rotated_bounds().translate(*o))
                .reduce(|a, b| a.union(b))
                && let Some(mp) = response.hover_pos().filter(|mp| canvas_rect.contains(*mp)) {
                    let doc_pos = self.to_doc(origin, mp);
                    if let Some(hover_id) = hit_test(page, doc_pos)
                        && !selection.contains(&hover_id)
                            && let (Some(hl), Some(hoff)) = (page.find(hover_id), page.absolute_offset(hover_id)) {
                                let hover_bounds = hl.frame.rotated_bounds().translate(hoff);
                                draw_distance_measurement(&painter, |p| self.to_screen(origin, p), sel_bounds, hover_bounds);
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
            if let Some((layer, _)) = single_path
                && ui.input(|i| i.modifiers.command && i.key_pressed(egui::Key::A))
                    && let LayerKind::Path { points, .. } = &layer.kind {
                        self.selected_points = (0..points.len()).collect();
                    }
            if response.clicked() && !response.double_clicked() && !ui.input(|i| i.modifiers.alt)
                && let Some((index, PathPart::Anchor)) = hovered_point {
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
                    if let Some(points) = &mut self.pen
                        && let Some(pt) = points.get_mut(*point_index) {
                            let delta = doc_pos - pt.anchor;
                            pt.handle_out = Some(delta);
                            pt.handle_in = Some(-delta);
                        }
                }
                DragState::EditingPathAnchor {
                    layer_id,
                    point_indices,
                    start_doc,
                    start_anchors,
                } => {
                    let delta = doc_pos - *start_doc;
                    if let Some(layer) = history.mutate().active_page_mut().find_mut(*layer_id)
                        && let LayerKind::Path { points, .. } = &mut layer.kind {
                            for (&index, &start_anchor) in point_indices.iter().zip(start_anchors.iter()) {
                                if let Some(pt) = points.get_mut(index) {
                                    pt.anchor = start_anchor + delta;
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
                        if let LayerKind::Path { points, .. } = &mut layer.kind
                            && let Some(pt) = points.get_mut(*point_index) {
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
                    if (frame.size.x.abs() > 2.0 || frame.size.y.abs() > 2.0 || *tool == Tool::Text)
                        && let Some(new_layer) = new_layer_for_tool(*tool, frame) {
                            let new_id = new_layer.id;
                            let is_text = matches!(new_layer.kind, LayerKind::Text { .. });
                            let hint = self.insert_hint_parent.take();
                            insert_layer(history.mutate().active_page_mut(), new_layer, *start_doc, hint);
                            if is_text
                                && let Some(l) = history.mutate().active_page_mut().find_mut(new_id) {
                                    apply_text_auto_resize(&ctx, l);
                                }
                            *selection = vec![new_id];
                            *tool = Tool::Select;
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
                    if let Some(points) = &mut self.pen
                        && let Some(pt) = points.get_mut(*point_index) {
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
                    if let Some(layer) = history.mutate().active_page_mut().find_mut(*layer_id)
                        && let LayerKind::Path { points, .. } = &mut layer.kind
                            && let Some(pt) = points.get_mut(*point_index) {
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
                            if let Some((px, py)) = doc_to_pixel(start_doc, bounds_doc, width, height)
                                && let Some(layer) = history.get().active_page().find(layer_id)
                                    && let LayerKind::Image { encoded, .. } = &layer.kind
                                        && let Some(decoded) = crate::image_ops::decode(encoded) {
                                            let tolerance = self.image_edit_tolerance();
                                            let wand = crate::image_ops::magic_wand_mask(&decoded, (px, py), tolerance);
                                            merge_mask(&mut new_mask, &wand, subtract);
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

            if inner.response.response.changed()
                && let Some(l) = history.mutate().active_page_mut().find_mut(id) {
                    if let LayerKind::Text { content, runs, .. } = &mut l.kind {
                        if !runs.is_empty() {
                            let (start, removed, inserted) = crate::model::text_runs::diff_chars(content, &buf);
                            crate::model::text_runs::splice(runs, start, removed, inserted, &base);
                        }
                        *content = buf;
                    }
                    apply_text_auto_resize(&ctx, l);
                }

            let escape_pressed = ui.input(|i| i.key_pressed(egui::Key::Escape));
            if inner.response.response.lost_focus() || escape_pressed {
                self.editing_text = None;
                self.text_edit_selection = None;
            }
        }
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{CornerRadii, Document};

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

}
