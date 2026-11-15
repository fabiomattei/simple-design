
use egui::{Color32, Pos2, Rect, Sense, Stroke as EguiStroke, Ui, Vec2};

use crate::alignment::DistributeAxis;
use crate::grouping;
use crate::history::History;
use crate::model::{
    GuideOrientation, Layer, LayerId, LayerKind, Page, Paint, PathPoint, Style, TextAlign,
    TextFont, TextResize, VerticalAlign,
};
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
mod click_handling;
mod constraint_resize;
mod context_menus;
mod drag_begin;
mod drag_end;
mod drag_update;
mod selection_overlay;
mod text_edit_overlay;
#[cfg(test)]
mod ui_gesture_tests;

use draw_layer::draw_children;
use image_edit::{ImageEditState, build_mask_overlay};
use path_editing::draw_pen_preview;
use rulers_guides::{draw_guides, draw_pixel_grid, draw_rulers};
use texture_caches::{
    HalftoneTextureCache, ImageTextureCache, GradientTextureCache, MaskedGroupTextureCache, NoiseTextureCache, PatternTextureCache,
    ShadowTextureCache,
};
use transform_geometry::{ResizeContainerInfo, ResizeLayerInfo, RotateLayerInfo};

pub(crate) use path_flatten::flatten_path;
pub(crate) use layer_creation::apply_text_auto_resize;
pub use transform_geometry::ScaleAnchor;
pub(crate) use constraint_resize::resize_layer;
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
/// Distance, in screen pixels, from the top-center resize handle to the
/// dedicated rotation handle above it — the only place a rotate drag starts.
const ROTATE_HANDLE_OFFSET: f32 = 24.0;
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

/// Per-frame canvas geometry: the document area inside the ruler strips, and
/// that area's top-left corner — the `origin` every `to_screen`/`to_doc`
/// conversion is relative to.
#[derive(Clone, Copy)]
struct CanvasLayout {
    canvas_rect: Rect,
    origin: Pos2,
}

/// The selection resolved against a page: every selected layer that still
/// exists with its absolute offset, plus the two single-selection cases that
/// get their own handles instead of a bounding box. Built by both the
/// selection overlay and drag start, so the two always agree.
struct SelectionView<'a> {
    layers: Vec<(&'a Layer, Vec2)>,
    single_line: Option<(&'a Layer, Vec2)>,
    single_path: Option<(&'a Layer, Vec2)>,
}

impl<'a> SelectionView<'a> {
    fn new(page: &'a Page, selection: &[LayerId]) -> Self {
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
        SelectionView { layers: selected_layers, single_line, single_path }
    }
}

/// Which of the selection overlay's interactive targets the pointer is over
/// this frame, found while painting them. Owned, with no borrows into the
/// document, so drag start and click-to-select can use it after `history`
/// has been mutated.
struct HoverState {
    handle: Option<Handle>,
    rotate_handle: bool,
    point: Option<(usize, PathPart)>,
    gap_handle: Option<usize>,
    /// The Smart Distribute layers in gap-handle order; empty unless the gap
    /// handles are showing.
    gap_handle_order: Vec<(LayerId, Rect)>,
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
    /// `containers`: selected Groups/Artboards/BooleanGroups, resized by
    /// their children's resizing constraints (`constraint_resize`) instead
    /// of having every leaf scaled — except in Scale mode, where only
    /// Artboards go here and other containers' leaves are scaled (with
    /// their stroke widths) into `layers` as before.
    ResizingGroup {
        handle: Handle,
        start_overall_bounds: Rect,
        layers: Vec<ResizeLayerInfo>,
        containers: Vec<ResizeContainerInfo>,
        scale_style: bool,
        scale_anchor: ScaleAnchor,
    },
    /// Dragging the rotation handle above the top edge (see
    /// `ROTATE_HANDLE_OFFSET` and `paint_bounding_box_handles`).
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
    gradient_cache: GradientTextureCache,
    shadow_cache: ShadowTextureCache,
    image_edit: Option<ImageEditState>,
    /// The `Text` layer currently being edited in place on canvas (see
    /// `show_text_edit_overlay`), if any.
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
    /// (see `apply_text_edit_output`); `ui/inspector/mod.rs` reads
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
    /// `ui/inspector/geometry_section.rs`'s Position/Size fields for where this is read.
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
            gradient_cache: GradientTextureCache::default(),
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

    /// Called by `ui/inspector/text_font_picker.rs` right after picking a `TextFont::System`
    /// whose family egui hadn't bound yet at that moment, so `id`'s
    /// auto-resize just ran against the `Proportional` fallback instead of
    /// the real font's metrics. `ui()` retries the measurement once the
    /// family becomes available (see `pending_font_resize`'s doc comment).
    pub fn queue_font_resize_retry(&mut self, id: LayerId, family: String) {
        self.pending_font_resize.push((id, family, 5));
    }

    /// Starts in-place editing of `id` (a `Text` layer) via the floating
    /// canvas overlay (see `show_text_edit_overlay`) — entered by
    /// double-clicking a `Text` layer, or by `app.rs`'s Enter/Return
    /// shortcut on a single selected `Text` layer. Snapshots history once on
    /// entry, matching the app's "snapshot at gesture start" convention
    /// (`ui/inspector/mod.rs`'s `should_snapshot`) — the keystrokes that follow
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
    /// double-click handler (`handle_text_double_click`), which only routes here for a bare
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
    /// `MaskSettings::enabled`) or a `BooleanGroup` child (its combine-op geometry
    /// depends on staying a direct child of the group).
    pub fn add_shape_label(&mut self, history: &mut History, shape_id: LayerId) -> Option<LayerId> {
        let page = history.get().active_page();
        let shape = page.find(shape_id)?;
        if shape.mask.enabled {
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

    /// Draws the canvas and handles every pointer/keyboard interaction on it
    /// for one frame. The phases run in an order that matters: clicks that
    /// edit `history` run before the document is painted (so this frame
    /// already shows their result), the selection overlay's hover hits feed
    /// drag start and click-to-select, and the menus and in-place text editor
    /// run last so they float over everything else.
    pub fn ui(
        &mut self,
        ui: &mut Ui,
        history: &mut History,
        tool: &mut Tool,
        selection: &mut Vec<LayerId>,
    ) {
        self.retry_pending_font_resizes(ui, history);

        let (response, painter) =
            ui.allocate_painter(ui.available_size(), Sense::click_and_drag());
        let full_rect = response.rect;
        let ruler_size = if self.show_rulers { RULER_SIZE } else { 0.0 };
        let canvas_rect = Rect::from_min_max(full_rect.min + Vec2::splat(ruler_size), full_rect.max);
        self.last_canvas_size = canvas_rect.size();
        let origin = canvas_rect.min;
        let layout = CanvasLayout { canvas_rect, origin };
        // Everything drawn on the document itself (background, grid, layers,
        // guides, drag previews) is clipped to `canvas_rect` so none of it
        // bleeds under the ruler strips reserved above/left of it.
        let canvas_painter = painter.with_clip_rect(canvas_rect);

        self.handle_dropped_images(ui, history, selection, layout);
        self.scope_point_selection(ui, history, tool, selection);
        self.scope_image_edit(ui, history);
        self.handle_pen_input(ui, &response, history, tool, selection, layout.origin);
        self.handle_path_point_clicks(ui, &response, history, tool, selection, layout.origin);
        self.handle_text_double_click(&response, history, tool, selection, layout.origin);
        self.handle_scissors_click(&response, history, tool, selection, layout.origin);

        self.paint_document(ui, &canvas_painter, history, layout);

        // `is_single_path_selected` is an owned copy of
        // `view.single_path.is_some()`: click-to-select runs after drag start,
        // which may edit `history`, so it can't keep `view`'s borrow of it.
        let (hover, is_single_path_selected) = {
            let page = history.get().active_page();
            let view = SelectionView::new(page, selection);
            let hover = self.paint_selection_overlay(ui, &painter, &response, page, &view, tool, selection, layout);
            self.handle_point_selection_clicks(ui, &response, &view, tool, &hover);
            (hover, view.single_path.is_some())
        };

        if response.drag_started() {
            self.begin_drag(ui, &response, history, tool, selection, layout, &hover);
        }
        self.handle_click_select(ui, &response, history, tool, selection, layout.origin, &hover, is_single_path_selected);
        if response.dragged() {
            self.continue_drag(ui, &response, &canvas_painter, history, tool, layout);
        }
        if response.drag_stopped() {
            self.end_drag(ui.ctx(), &response, history, tool, selection, layout);
        }

        if let Some(points) = &self.pen {
            draw_pen_preview(&painter, points, layout.origin, self.pan, self.zoom, response.hover_pos());
        }

        // Rulers paint last (and unclipped, over the full widget) so their
        // strips stay on top of anything that scrolled near the canvas edge.
        if self.show_rulers {
            draw_rulers(&painter, full_rect, canvas_rect, origin, self.pan, self.zoom, response.hover_pos());
        }
        self.handle_scroll_zoom(ui, &response, layout.origin);

        self.handle_secondary_click(ui, &response, history, tool, selection, layout.origin);
        self.show_canvas_menu(ui, history, selection);
        self.show_overlap_menu(ui, history, selection);
        self.show_point_type_menu(ui, history);

        if let Some(id) = self.editing_text {
            self.show_text_edit_overlay(ui, history, id, layout);
        }
    }

    /// Re-runs `apply_text_auto_resize` for each layer queued by
    /// `queue_font_resize_retry` once its font family is bound in egui's
    /// font definitions, dropping the entry when its retries run out.
    fn retry_pending_font_resizes(&mut self, ui: &Ui, history: &mut History) {
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
    }

    /// Paints the document itself, clipped to the canvas area: backdrop,
    /// pixel grid, layers, the "Edit Image" selection tint and guides. Also
    /// evicts texture-cache entries for layers that no longer exist.
    fn paint_document(&mut self, ui: &Ui, canvas_painter: &egui::Painter, history: &History, layout: CanvasLayout) {
        let CanvasLayout { canvas_rect, origin, .. } = layout;
        canvas_painter.rect_filled(canvas_rect, 0.0, crate::ui::theme::canvas_colors(ui.visuals().dark_mode).backdrop);
        if self.show_grid {
            draw_pixel_grid(canvas_painter, canvas_rect, origin, self.pan, self.zoom);
        }

        let doc = history.get();
        let page = doc.active_page();
        let ctx = ui.ctx().clone();
        draw_children(
            canvas_painter,
            &ctx,
            &mut self.image_cache,
            &mut self.mask_cache,
            &mut self.noise_cache,
            &mut self.halftone_cache,
            &mut self.pattern_cache,
            &mut self.gradient_cache,
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
        self.mask_cache.evict_unused();
        self.noise_cache.evict_stale(&page.all_layer_ids());
        self.halftone_cache.evict_stale(&page.all_layer_ids());
        self.pattern_cache.evict_stale(&page.all_layer_ids());
        self.gradient_cache.evict_stale(&page.all_layer_ids());
        self.shadow_cache.evict_stale(&page.all_layer_ids());

        self.paint_image_edit_overlay(&ctx, canvas_painter, page, origin);
        draw_guides(canvas_painter, canvas_rect, origin, self.pan, self.zoom, &page.guides);
    }

    /// "Edit Image" mode's selection overlay: a translucent tint over
    /// every masked pixel, rebuilt only when the mask actually changed
    /// (see `ImageEditState::overlay_stale`).
    fn paint_image_edit_overlay(&mut self, ctx: &egui::Context, canvas_painter: &egui::Painter, page: &Page, origin: Pos2) {
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
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::transform_geometry::{apply_resize_delta, collect_resizable_leaves, display_bounds};
    use crate::model::{CornerRadii, Document, Frame};
    use crate::shapes::rotate_point;

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
