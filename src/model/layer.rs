use egui::{Color32, Pos2, Vec2};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::model::text_runs::TextRun;

pub type LayerId = Uuid;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Stroke {
    pub paint: Paint,
    pub width: f32,
}

impl Default for Stroke {
    fn default() -> Self {
        Self {
            paint: Paint::Solid(Color32::from_rgb(30, 30, 30)),
            width: 1.0,
        }
    }
}

/// One color stop in a `Gradient`, at normalized position `offset` (0.0 =
/// `Gradient::from`, 1.0 = `Gradient::to`).
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq)]
pub struct GradientStop {
    pub offset: f32,
    pub color: Color32,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum GradientKind {
    Linear,
    Radial,
}

/// A linear or radial gradient: `from`/`to` are normalized
/// (0.0-1.0 per axis) points within the *unrotated* bounding box of whatever
/// it's painting (`Frame::bounds()`, not absolute canvas/pixmap space — each
/// renderer maps them to its own coordinate space at draw time). For
/// `Linear`, `from`→`to` is the gradient axis. For `Radial`, `from` is the
/// center and `|to - from|` (in bounds-space, scaled by the bounds size) is
/// the radius.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Gradient {
    pub kind: GradientKind,
    /// Kept sorted by `offset`; `color_at` assumes this.
    pub stops: Vec<GradientStop>,
    pub from: Pos2,
    pub to: Pos2,
}

impl Gradient {
    /// A two-stop linear gradient from `start` to `end`, spanning the full bounding box diagonal.
    pub fn linear(start: Color32, end: Color32) -> Self {
        Self {
            kind: GradientKind::Linear,
            stops: vec![GradientStop { offset: 0.0, color: start }, GradientStop { offset: 1.0, color: end }],
            from: Pos2::new(0.0, 0.0),
            to: Pos2::new(1.0, 1.0),
        }
    }

    /// A two-stop radial gradient from `start` (center) to `end` (edge), spanning half the bounding box width.
    pub fn radial(start: Color32, end: Color32) -> Self {
        Self {
            kind: GradientKind::Radial,
            stops: vec![GradientStop { offset: 0.0, color: start }, GradientStop { offset: 1.0, color: end }],
            from: Pos2::new(0.5, 0.5),
            to: Pos2::new(1.0, 0.5),
        }
    }

    /// The gradient's parametric position `t` (unclamped — callers that need
    /// `color_at`'s clamped sampling should just call that directly) for a
    /// point already expressed in the same normalized (0.0-1.0 per axis)
    /// space as `from`/`to`. `Linear` projects onto the `from`→`to` axis;
    /// `Radial` is the point's normalized distance from `from`, in units of
    /// `|to - from|`.
    pub fn t_at(&self, point: Pos2) -> f32 {
        match self.kind {
            GradientKind::Linear => {
                let axis = self.to - self.from;
                let len2 = axis.length_sq();
                if len2 < 1e-9 {
                    return 0.0;
                }
                (point - self.from).dot(axis) / len2
            }
            GradientKind::Radial => {
                let radius = (self.to - self.from).length();
                if radius < 1e-6 {
                    return 0.0;
                }
                (point - self.from).length() / radius
            }
        }
    }

    /// `color_at(self.t_at(point))` — the color at a point in the gradient's
    /// own normalized (0.0-1.0 per axis, relative to the unrotated bounding
    /// box being painted) space.
    pub fn sample_normalized(&self, point: Pos2) -> Color32 {
        self.color_at(self.t_at(point))
    }

    /// Linearly interpolates the color at normalized position `t` (clamped
    /// to `0.0..=1.0`) along the gradient's stops. Falls back to
    /// fully-transparent if `stops` is empty (shouldn't normally happen —
    /// every constructor above seeds two stops — but keeps this total rather
    /// than panicking on a hand-edited/corrupted document).
    pub fn color_at(&self, t: f32) -> Color32 {
        let t = t.clamp(0.0, 1.0);
        if self.stops.is_empty() {
            return Color32::TRANSPARENT;
        }
        if self.stops.len() == 1 {
            return self.stops[0].color;
        }
        let mut prev = &self.stops[0];
        if t <= prev.offset {
            return prev.color;
        }
        for stop in &self.stops[1..] {
            if t <= stop.offset {
                let span = (stop.offset - prev.offset).max(1e-6);
                let local_t = ((t - prev.offset) / span).clamp(0.0, 1.0);
                return lerp_color(prev.color, stop.color, local_t);
            }
            prev = stop;
        }
        prev.color
    }
}

fn lerp_color(a: Color32, b: Color32, t: f32) -> Color32 {
    let lerp = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round().clamp(0.0, 255.0) as u8;
    Color32::from_rgba_unmultiplied(lerp(a.r(), b.r()), lerp(a.g(), b.g()), lerp(a.b(), b.b()), lerp(a.a(), b.a()))
}

/// A procedural grayscale-grain fill: brightness varies around `base` per
/// grain cell, deterministically from `seed` (see `noise_fill::sample`, the
/// single function both the canvas and PNG export sample from, so they
/// render the same grain pattern despite rasterizing at different
/// resolutions).
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq)]
pub struct NoiseFill {
    pub base: Color32,
    /// 0.0-1.0, how strongly grain brightness deviates from `base`.
    pub intensity: f32,
    /// Grain cell size, in the same local-bounds units as `Frame::size`.
    pub scale: f32,
    pub seed: u32,
}

/// A procedural two-color dot-grid fill (see `halftone_fill::sample`):
/// `background`/`dot` colors, grid `scale` (spacing, same local-bounds-unit
/// convention as `NoiseFill::scale`), and `coverage` (0.0-1.0, dot radius as
/// a fraction of the cell's half-width). Rows stagger by half a cell —
/// `halftone_fill::sample`'s doc comment has the full grid convention.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq)]
pub struct HalftoneFill {
    pub background: Color32,
    pub dot: Color32,
    pub scale: f32,
    pub coverage: f32,
}

/// A tileable image fill: `encoded` is PNG bytes, the same convention
/// `LayerKind::Image::encoded` already uses (decoded on demand, not kept
/// around as a decoded buffer). `tile_width` is the repeat width, in the
/// same local-bounds units as `Frame::size`/`NoiseFill::scale`; height
/// follows the source image's own aspect ratio (see
/// `canvas.rs::PatternTextureCache`/`export.rs::to_sk_paint`, which both
/// derive it from the decoded image rather than storing it here).
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct PatternFill {
    pub encoded: Vec<u8>,
    pub tile_width: f32,
}

/// A fill or stroke color source: a flat color, a gradient, or (fill only —
/// see `ui/inspector.rs::paint_editor`'s `allow_texture_fills`) one of the
/// three procedural/texture fills: noise, halftone dots, or a tiled image
/// pattern. `#[serde(untagged)]` so `Paint::Solid` serializes/deserializes
/// as a bare `Color32` — the exact shape `Style::fill`/`Stroke::color` used
/// before this type existed, so documents saved by older versions of this
/// app (where those fields were plain `Color32`) still load unchanged, with
/// every solid color becoming `Paint::Solid`. `Noise`/`Halftone`/`Pattern`
/// must stay after `Solid`/`Gradient`: untagged deserialization tries
/// variants in declared order, and `Solid`/`Gradient` must keep winning
/// their existing document shapes.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(untagged)]
pub enum Paint {
    Solid(Color32),
    Gradient(Gradient),
    Noise(NoiseFill),
    Halftone(HalftoneFill),
    Pattern(PatternFill),
}

impl Paint {
    /// A single representative flat color: the solid color itself, a
    /// gradient's first stop, a noise fill's base color, a halftone fill's
    /// dot color, or (no cheap single representative color exists for an
    /// image) a neutral gray for `Pattern` — only ever seen transiently
    /// while switching fill types in the inspector. Used wherever call
    /// sites need "just a color" rather than the full paint (e.g. text
    /// glyph color, which only supports `Solid` — see `CLAUDE.md`/`ROADMAP.md`).
    pub fn to_color32(&self) -> Color32 {
        match self {
            Paint::Solid(c) => *c,
            Paint::Gradient(g) => g.stops.first().map(|s| s.color).unwrap_or(Color32::TRANSPARENT),
            Paint::Noise(n) => n.base,
            Paint::Halftone(h) => h.dot,
            Paint::Pattern(_) => Color32::from_gray(200),
        }
    }

    /// Whether this paint is a `Gradient` (as opposed to solid or a procedural/texture fill).
    pub fn is_gradient(&self) -> bool {
        matches!(self, Paint::Gradient(_))
    }

    /// Whether this paint needs the tessellated/textured rendering path
    /// (`canvas.rs::paint_polygon`'s mesh-based fill) instead of egui's
    /// flat-color fast path (e.g. a plain `Painter::rect` for an unrotated
    /// rectangle) — true for every non-`Solid` fill kind now that `Noise`/
    /// `Halftone`/`Pattern` join `Gradient`. Stroke-side call sites keep
    /// using `is_gradient()` directly, since a stroke can only ever be
    /// `Solid`/`Gradient` (see `paint_editor`'s `allow_texture_fills`,
    /// fill-only).
    pub fn needs_tessellated_fill(&self) -> bool {
        !matches!(self, Paint::Solid(_))
    }
}

impl From<Color32> for Paint {
    fn from(color: Color32) -> Self {
        Paint::Solid(color)
    }
}

/// One drop ("outer") or inner shadow effect (see `Style::shadows`/
/// `inner_shadows`): `color`'s own alpha channel doubles as
/// the shadow's opacity (same convention `Paint::Solid`/`Stroke` colors
/// already use), `offset` moves it horizontally/vertically, `blur` is the
/// softness radius in px, and `spread` expands (+) or contracts (-) it
/// uniformly before blurring. Rendered by `shadow.rs`, which both
/// `canvas.rs` and `export.rs` call into — see that module's doc comment.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq)]
pub struct Shadow {
    pub color: Color32,
    pub offset: Vec2,
    pub blur: f32,
    pub spread: f32,
}

impl Default for Shadow {
    fn default() -> Self {
        Self {
            color: Color32::from_black_alpha(80),
            offset: Vec2::new(0.0, 4.0),
            blur: 8.0,
            spread: 0.0,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Style {
    pub fill: Option<Paint>,
    pub stroke: Option<Stroke>,
    /// Independent opacity for the fill only (0.0-1.0), multiplied together
    /// with `Layer::opacity`/ancestor opacity at draw time — a
    /// per-attribute opacity (see `ui/inspector.rs`'s Fill/Border sliders).
    /// A `Text` layer's glyph color is `fill`, so this also covers "text
    /// color opacity" per CLAUDE.md's Fonts section. `#[serde(default)]`
    /// (via the same helper `Layer::opacity` uses) so documents saved
    /// before this field existed load at full (`1.0`) opacity, unchanged.
    #[serde(default = "default_opacity")]
    pub fill_opacity: f32,
    /// Same as `fill_opacity`, for the stroke only.
    #[serde(default = "default_opacity")]
    pub stroke_opacity: f32,
    /// Drop ("outer") shadows, stacked in list order (each later entry drawn
    /// on top of the earlier ones) — see `Shadow`. `#[serde(default)]` so
    /// documents saved before this field existed load with none.
    #[serde(default)]
    pub shadows: Vec<Shadow>,
    /// Inner shadows, same stacking convention as `shadows`.
    #[serde(default)]
    pub inner_shadows: Vec<Shadow>,
}

impl Default for Style {
    fn default() -> Self {
        Self {
            fill: Some(Paint::Solid(Color32::from_rgb(216, 216, 216))),
            stroke: Some(Stroke::default()),
            fill_opacity: 1.0,
            stroke_opacity: 1.0,
            shadows: Vec::new(),
            inner_shadows: Vec::new(),
        }
    }
}

/// A named, reusable Fill/Border/Shadow style any layer can link to via
/// `Layer::style_id` (see `Document::layer_styles`) — a "Layer
/// Styles" library, the shape-side sibling of `TextStyle`. Holds every field of
/// `Style` plus `id`/`name`, so applying one to a layer is a straight field
/// copy (see `ui/inspector.rs`).
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct LayerStyle {
    pub id: Uuid,
    pub name: String,
    pub fill: Option<Paint>,
    pub stroke: Option<Stroke>,
    pub fill_opacity: f32,
    pub stroke_opacity: f32,
    pub shadows: Vec<Shadow>,
    pub inner_shadows: Vec<Shadow>,
}

/// Frame of a layer, in the coordinate space of its parent (page or artboard/group).
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq)]
pub struct Frame {
    pub pos: Pos2,
    pub size: Vec2,
    /// Clockwise degrees of rotation about `bounds().center()`. `0.0` (the
    /// default, and always true for documents saved before this field
    /// existed) means axis-aligned, matching every pre-rotation-support
    /// behavior exactly. Only ever nonzero on a leaf shape layer — a
    /// `Group`/`Artboard`'s own `rotation` is always `0.0`; rotating a group
    /// bakes the rotation into each descendant's own frame instead (see
    /// `canvas.rs`'s rotate-drag handling), so nothing needs to propagate
    /// rotation through `Page::absolute_offset` or either renderer's
    /// child-offset accumulation.
    #[serde(default)]
    pub rotation: f32,
}

impl Frame {
    /// Builds an unrotated frame from a drag's start point `a` and end point `b`, preserving drag direction in `size`.
    pub fn from_two_points(a: Pos2, b: Pos2) -> Self {
        Self {
            pos: a,
            size: b - a,
            rotation: 0.0,
        }
    }

    /// Start point as dragged (top-left corner for most shapes, first
    /// endpoint for a line). May not be the geometric min corner.
    pub fn start(&self) -> Pos2 {
        self.pos
    }

    /// End point as dragged (second endpoint for a line).
    pub fn end(&self) -> Pos2 {
        self.pos + self.size
    }

    /// Axis-aligned bounding rect of the *unrotated* local frame, regardless
    /// of drag direction. Use this for anything that reads/writes the
    /// shape's own local geometry (resize math, point coordinates) — NOT for
    /// anything that needs the shape's actual on-screen footprint when
    /// rotated (hit-testing, selection outlines, alignment, grouping bboxes),
    /// which should use `rotated_bounds()` instead.
    pub fn bounds(&self) -> egui::Rect {
        egui::Rect::from_two_pos(self.pos, self.pos + self.size)
    }

    /// Axis-aligned bounding box of `bounds()`'s 4 corners after rotating
    /// them about its center by `rotation` degrees. Equals `bounds()` when
    /// `rotation == 0.0`.
    pub fn rotated_bounds(&self) -> egui::Rect {
        if self.rotation == 0.0 {
            return self.bounds();
        }
        let corners = crate::shapes::rotated_corners(self.bounds(), self.rotation);
        egui::Rect::from_points(&corners)
    }

    /// Builds an unrotated frame from an axis-aligned rect, using `rect.min` as `pos`.
    pub fn from_bounds(rect: egui::Rect) -> Self {
        Self {
            pos: rect.min,
            size: rect.size(),
            rotation: 0.0,
        }
    }
}

/// How a `PathPoint`'s two bezier handles behave relative to each other when
/// one of them is dragged (see `canvas.rs`'s `EditingPathHandle` handling).
/// Mirrors the standard four vector point types.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub enum PointType {
    /// Handles move fully independently of each other. Default so that
    /// points loaded from documents saved before this field existed keep
    /// behaving exactly as they did (today's only post-creation-edit
    /// behavior), rather than acquiring a mirroring behavior on load that
    /// their stored handle geometry may not actually satisfy.
    #[default]
    Disconnected,
    /// No curve: both handles are cleared.
    Straight,
    /// Opposite handle mirrors this one exactly (same angle and length).
    Mirror,
    /// Opposite handle mirrors this one's angle but keeps its own length.
    Asymmetric,
}

/// A single anchor of a vector path, in the coordinate space of the owning
/// `Layer`'s `frame.pos` (i.e. `frame.pos + anchor` gives the point's
/// position in the parent's space, mirroring how a `Group`/`Artboard`
/// offsets its children). `handle_in`/`handle_out` are bezier control-point
/// offsets *relative to the anchor*; `None` means a straight corner on that
/// side.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq)]
pub struct PathPoint {
    pub anchor: Pos2,
    pub handle_in: Option<Vec2>,
    pub handle_out: Option<Vec2>,
    #[serde(default)]
    pub point_type: PointType,
    /// Per-anchor corner rounding, in document units.
    /// Only meaningful where both adjacent segments are straight (this
    /// point and both its neighbors have no `handle_in`/`handle_out`
    /// touching either of those segments) — a point that's otherwise
    /// curved just ignores a stray nonzero value here. `0.0` (the default,
    /// and always true for documents saved before this field existed)
    /// means a plain sharp corner. `flatten_path`/`export.rs`'s
    /// `path_to_sk_path` both inset-and-arc via
    /// `shapes::rounded_corner_arc_points`, clamped there to at most half
    /// the shorter adjacent segment — so setting this arbitrarily high is
    /// exactly a "maximum radius" behavior, not a separate mode.
    #[serde(default)]
    pub corner_radius: f32,
}

/// A single closed contour with no holes of its own — one ring of a
/// `CompoundPath` polygon (either its outer boundary or one hole in it).
/// Points are straight-line only (no bezier handles), and relative to the
/// owning `Layer`'s `frame.pos`, same convention as `PathPoint::anchor`.
/// Boolean-op results are always polygonal (curves get flattened as part of
/// the operation), so no handle fields are needed here.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct PathPolygon {
    pub exterior: Vec<Pos2>,
    pub holes: Vec<Vec<Pos2>>,
}

/// Independent per-corner rounding for a `Rectangle`, in document units.
/// Each corner is clamped to at most half the shorter side by
/// `shapes::rounded_rect_points`/`export.rs`'s `rounded_rect_path` — same
/// "maximum radius, not a separate mode" convention as `PathPoint::corner_radius`.
#[derive(Clone, Copy, Debug, Serialize, PartialEq)]
pub struct CornerRadii {
    pub top_left: f32,
    pub top_right: f32,
    pub bottom_right: f32,
    pub bottom_left: f32,
}

impl CornerRadii {
    pub const ZERO: Self = Self { top_left: 0.0, top_right: 0.0, bottom_right: 0.0, bottom_left: 0.0 };

    /// All four corners set to the same `radius`.
    pub fn uniform(radius: f32) -> Self {
        Self { top_left: radius, top_right: radius, bottom_right: radius, bottom_left: radius }
    }

    /// CSS `border-radius` order: top-left, top-right, bottom-right, bottom-left.
    pub fn as_array(&self) -> [f32; 4] {
        [self.top_left, self.top_right, self.bottom_right, self.bottom_left]
    }

    /// Scales every corner by `factor` (used when a resize handle scales a
    /// rectangle's geometry, mirroring how it also scales stroke width).
    pub fn scaled(&self, factor: f32) -> Self {
        Self {
            top_left: (self.top_left * factor).max(0.0),
            top_right: (self.top_right * factor).max(0.0),
            bottom_right: (self.bottom_right * factor).max(0.0),
            bottom_left: (self.bottom_left * factor).max(0.0),
        }
    }
}

impl Default for CornerRadii {
    fn default() -> Self {
        Self::ZERO
    }
}

/// Accepts either a bare number (documents saved before per-corner rounding
/// existed, where `corner_radius` was a single `f32`) or the four-field
/// object shape, so old `.sdesign` files load with all corners set to that
/// one uniform radius.
impl<'de> Deserialize<'de> for CornerRadii {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Repr {
            Uniform(f32),
            PerCorner { top_left: f32, top_right: f32, bottom_right: f32, bottom_left: f32 },
        }
        Ok(match Repr::deserialize(deserializer)? {
            Repr::Uniform(r) => CornerRadii::uniform(r),
            Repr::PerCorner { top_left, top_right, bottom_right, bottom_left } => {
                CornerRadii { top_left, top_right, bottom_right, bottom_left }
            }
        })
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub enum LayerKind {
    Artboard { children: Vec<Layer>, background: Color32 },
    Group { children: Vec<Layer> },
    Rectangle { corner_radius: CornerRadii },
    Oval,
    Line,
    /// A regular star: `points` outer vertices alternating with `points`
    /// inner ones, `inner_ratio` (`0.0..=1.0`) the inner vertices' radius as
    /// a fraction of the outer — see `shapes::star_points`.
    Star { points: u32, inner_ratio: f32 },
    /// A regular `sides`-gon — see `shapes::polygon_points`.
    Polygon { sides: u32 },
    /// A straight segment with optional decorative end markers — same
    /// `frame.start()`/`frame.end()` two-endpoint geometry as `Line`
    /// (including `frame.size`'s sign preserving drag direction), just with
    /// `start_cap`/`end_cap` drawn at each end pointing along the segment's
    /// own direction.
    Arrow { start_cap: ArrowCap, end_cap: ArrowCap },
    Path { points: Vec<PathPoint>, closed: bool },
    /// The flattened result of a boolean shape operation (Union/Subtract/
    /// Intersect/Difference) — see `boolean_ops.rs`. Each `PathPolygon` is
    /// one disjoint region of the result; a region's `holes` render as
    /// actual holes (even-odd fill), which a single-contour `Path` can't
    /// represent.
    CompoundPath { polygons: Vec<PathPolygon> },
    /// A non-destructive "Boolean Group" (Union/Subtract/Intersect/
    /// Difference/Add), structurally parallel to `Group` — `children` are
    /// kept as real, independently-editable `Layer`s (their `frame`/`style`/
    /// `visible` all still meaningful for editing), never flattened. What's
    /// actually drawn/hit-tested is computed live on every call from
    /// `children` — see `boolean_ops::compute_boolean_group` — using this
    /// layer's own `style` for fill/stroke; children's own `style`/
    /// `is_mask`/`ignore_mask`/`opacity` become inert once inside a
    /// `BooleanGroup` (only their geometry, their own `visible`, and their
    /// own `bool_op` participate). The bottommost (z-order-first,
    /// `children[0]`) child is always the base its siblings combine onto;
    /// its own `bool_op` is ignored. `frame.rotation` is always `0.0` here,
    /// same convention as `Group`/`Artboard` (see `Frame::rotation`'s doc
    /// comment) — rotating one bakes the rotation into each descendant's own
    /// frame instead.
    BooleanGroup { children: Vec<Layer> },
    Text {
        content: String,
        font_size: f32,
        font: TextFont,
        align: TextAlign,
        #[serde(default)]
        vertical_align: VerticalAlign,
        #[serde(default)]
        resize: TextResize,
        /// `None` means automatic (font-determined) line height.
        #[serde(default)]
        line_height: Option<f32>,
        #[serde(default)]
        letter_spacing: f32,
        /// Extra vertical gap added after each blank-line-separated
        /// paragraph, on top of the normal line height.
        #[serde(default)]
        paragraph_spacing: f32,
        #[serde(default)]
        bold: bool,
        #[serde(default)]
        italic: bool,
        #[serde(default)]
        underline: bool,
        #[serde(default)]
        strikethrough: bool,
        #[serde(default)]
        transform: TextTransform,
        #[serde(default)]
        list: ListType,
        #[serde(default = "default_list_start")]
        list_start: i32,
        /// Links this layer to a `Document::text_styles` entry it was last
        /// applied from; `None` means unlinked (either never applied, or
        /// explicitly detached). Editing a linked layer's fields directly
        /// does not clear this — only "Detach" does (see `ui/inspector.rs`).
        #[serde(default)]
        style_id: Option<Uuid>,
        /// Per-character style overrides ("rich text") — empty (the
        /// default, and always true for documents saved before this field
        /// existed) means every one of this variant's own scalar fields
        /// above (`font`, `font_size`, `bold`, `italic`, `underline`,
        /// `strikethrough`) plus `Layer::style.fill` for color apply
        /// uniformly to all of `content`, exactly as if this field didn't
        /// exist. Non-empty means `runs` is the *sole* source of character
        /// styling and together spans the whole of `content`
        /// (`runs.iter().map(|r| r.len).sum() == content.chars().count()`)
        /// — the scalar fields above then only matter as the seed for
        /// brand-new text with no adjacent run to inherit from. Paragraph-
        /// level properties (alignment, resize, line/letter/paragraph
        /// spacing, transform, list) are deliberately *not* per-run — see
        /// `model/text_runs.rs`'s module doc for the invariant-maintaining
        /// helpers (`splice`/`apply`/`mixed_or`); never mutate `runs`
        /// directly outside of those.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        runs: Vec<TextRun>,
    },
    /// A bitmap image, always stored PNG-encoded regardless of the format it
    /// was originally inserted as (see `image_ops::decode`), so every
    /// consumer (canvas texture cache, PNG export, the destructive pixel
    /// edits below) only ever has to deal with one format.
    ///
    /// `width`/`height` are `encoded`'s pixel dimensions — the *displayed*
    /// size is `frame.bounds()`, which can differ (a resized frame stretches
    /// the bitmap; see "Reset to Original Size" in the inspector).
    ///
    /// Crop/Fill/Magic-Wand-delete/Trim/Remove-Background/Minimize-File-Size
    /// all destructively replace `encoded` (and bump `version`) rather than
    /// keeping a non-destructive mask, matching a typical image-editing
    /// mode. `color_adjust` is the one deliberately non-destructive
    /// exception, applied at render/export time.
    Image {
        #[serde(with = "base64_bytes")]
        encoded: Vec<u8>,
        width: u32,
        height: u32,
        /// Regenerated every time `encoded` changes; the texture cache
        /// (`canvas.rs`) keys off `(LayerId, version, color_adjust)` instead
        /// of hashing `encoded` every frame. A fresh UUID (rather than an
        /// incrementing counter) avoids collisions across undo/redo, where
        /// re-editing after an undo can otherwise revisit a counter value a
        /// *different* prior edit already used.
        #[serde(default = "Uuid::new_v4")]
        version: LayerId,
        #[serde(default)]
        color_adjust: ColorAdjust,
    },
}

/// Font families available to a `Text` layer. `Proportional`/`Monospace`
/// are egui's two bundled default typefaces (`Ubuntu-Light` / `Hack-
/// Regular`, see `epaint_default_fonts`); `Serif`/`Display`/`Handwriting`
/// are extra fonts bundled in `assets/fonts/` and registered with egui in
/// `fonts.rs`. `System` names a font installed on the user's machine,
/// discovered at runtime by `system_fonts.rs` (`fontdb`, works on macOS and
/// Linux) — unlike the other variants it isn't guaranteed to resolve to
/// anything (a different machine, or one without that font installed);
/// every render site falls back to `Proportional` when it doesn't. Not
/// `Copy` because of that `String`.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum TextFont {
    Proportional,
    Monospace,
    Serif,
    Display,
    Handwriting,
    System(String),
}

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub enum TextAlign {
    #[default]
    Left,
    Center,
    Right,
    Justify,
}

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub enum VerticalAlign {
    #[default]
    Top,
    Middle,
    Bottom,
}

/// How a `Text` layer's `frame.size` tracks its content, mirroring the
/// standard "Resizing" options. `Auto` recomputes both width and height from the
/// unwrapped content (no wrapping); `AutoHeight` wraps at the current width
/// but recomputes height; `Fixed` never auto-adjusts and clips/wraps within
/// the stored frame. See `canvas.rs::apply_text_auto_resize`.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub enum TextResize {
    #[default]
    Auto,
    AutoHeight,
    Fixed,
}

/// Non-destructive display transform — the stored `content` is never
/// mutated by this, only how it's rendered/exported.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub enum TextTransform {
    #[default]
    None,
    Uppercase,
    Lowercase,
    Titlecase,
}

/// Applies a bullet/numbered prefix to each non-empty line, matching
/// a "select line-separated items, choose list type" model.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub enum ListType {
    #[default]
    None,
    Bullet,
    Numbered,
}

/// An `Arrow` layer's end-marker style, drawn pointing along the segment's
/// own direction at that end (see `shapes`/`canvas.rs`/`export.rs`'s arrow
/// cap builders).
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub enum ArrowCap {
    #[default]
    None,
    Line,
    Triangle,
    Disc,
}

/// The combine mode a layer contributes when it's a direct child of a
/// `LayerKind::BooleanGroup` (see `Layer::bool_op`) — ignored everywhere
/// else. Mirrors the standard five boolean operations; see `boolean_ops.rs` for
/// how each is actually computed (`combine_step`).
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub enum BoolOp {
    #[default]
    Union,
    Subtract,
    Intersect,
    Difference,
    /// "Add": concatenates operands as separate disjoint pieces of
    /// one shape with no clipping/union math. Two operands that actually
    /// overlap will show as a hole in the overlap when rendered, since a
    /// `BooleanGroup`'s result is always drawn with an even-odd fill rule
    /// (needed for `Subtract`/`Difference`'s holes) — see `boolean_ops.rs`'s
    /// `combine_step` and `point_in_multipolygon` for this documented edge
    /// case.
    Add,
}

impl BoolOp {
    /// Human-readable label for this combine mode, as shown in the "Combine" menu.
    pub fn label(&self) -> &'static str {
        match self {
            BoolOp::Union => "Union",
            BoolOp::Subtract => "Subtract",
            BoolOp::Intersect => "Intersect",
            BoolOp::Difference => "Difference",
            BoolOp::Add => "Add",
        }
    }
}

fn default_list_start() -> i32 {
    1
}

/// A named, reusable text style a `Text` layer can link to via `style_id`
/// (see `Document::text_styles`). Holds every per-layer style field except
/// `content` and geometry, so applying one to a layer is a straight field
/// copy (see `ui/inspector.rs`).
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct TextStyle {
    pub id: Uuid,
    pub name: String,
    pub font_size: f32,
    pub font: TextFont,
    pub align: TextAlign,
    pub vertical_align: VerticalAlign,
    pub line_height: Option<f32>,
    pub letter_spacing: f32,
    pub paragraph_spacing: f32,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub strikethrough: bool,
    pub transform: TextTransform,
    pub list: ListType,
    pub list_start: i32,
    pub fill: Option<Color32>,
}

/// Non-destructive per-image adjustment (a "Color Adjust" effect),
/// applied at render/export time rather than baked into `Image::encoded`.
/// `hue` is in degrees; `saturation`/`brightness`/`contrast` are in
/// `-1.0..=1.0`, where `0.0` is a no-op on all four.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq)]
pub struct ColorAdjust {
    pub hue: f32,
    pub saturation: f32,
    pub brightness: f32,
    pub contrast: f32,
}

impl Default for ColorAdjust {
    fn default() -> Self {
        Self { hue: 0.0, saturation: 0.0, brightness: 0.0, contrast: 0.0 }
    }
}

impl ColorAdjust {
    pub fn is_identity(&self) -> bool {
        *self == Self::default()
    }
}

/// Serializes a `Vec<u8>` as a base64 string instead of a JSON array of
/// numbers — an embedded image's PNG bytes would otherwise bloat the
/// `.sdesign` JSON by roughly 4x (one comma-separated decimal per byte).
mod base64_bytes {
    use base64::Engine;
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    pub fn serialize<S: Serializer>(bytes: &[u8], s: S) -> Result<S::Ok, S::Error> {
        base64::engine::general_purpose::STANDARD.encode(bytes).serialize(s)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<u8>, D::Error> {
        let encoded = String::deserialize(d)?;
        base64::engine::general_purpose::STANDARD
            .decode(&encoded)
            .map_err(serde::de::Error::custom)
    }
}

impl LayerKind {
    /// Human-readable type name, as shown in the layers panel and inspector.
    pub fn type_name(&self) -> &'static str {
        match self {
            LayerKind::Artboard { .. } => "Artboard",
            LayerKind::Group { .. } => "Group",
            LayerKind::Rectangle { .. } => "Rectangle",
            LayerKind::Oval => "Oval",
            LayerKind::Line => "Line",
            LayerKind::Star { .. } => "Star",
            LayerKind::Polygon { .. } => "Polygon",
            LayerKind::Arrow { .. } => "Arrow",
            LayerKind::Path { .. } => "Path",
            LayerKind::CompoundPath { .. } => "Compound Path",
            LayerKind::BooleanGroup { .. } => "Boolean Group",
            LayerKind::Text { .. } => "Text",
            LayerKind::Image { .. } => "Image",
        }
    }

    /// This kind's children, if it's a container (`Artboard`/`Group`/`BooleanGroup`); `None` for leaf shapes.
    pub fn children(&self) -> Option<&Vec<Layer>> {
        match self {
            LayerKind::Artboard { children, .. } | LayerKind::Group { children } | LayerKind::BooleanGroup { children } => Some(children),
            _ => None,
        }
    }

    /// Mutable version of `children`.
    pub fn children_mut(&mut self) -> Option<&mut Vec<Layer>> {
        match self {
            LayerKind::Artboard { children, .. } | LayerKind::Group { children } | LayerKind::BooleanGroup { children } => Some(children),
            _ => None,
        }
    }
}

fn default_opacity() -> f32 {
    1.0
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Layer {
    pub id: LayerId,
    pub name: String,
    pub frame: Frame,
    pub style: Style,
    pub visible: bool,
    pub locked: bool,
    /// 0.0 (fully transparent) to 1.0 (fully opaque). Absent in older save
    /// files, hence `default`. A container's opacity multiplies into its
    /// descendants' effective opacity when drawn (see `canvas.rs`/`export.rs`
    /// `draw_layer`), rather than each layer's opacity being independent.
    #[serde(default = "default_opacity")]
    pub opacity: f32,
    /// "Use as Mask": when `true`, this layer's own fill/stroke
    /// are never drawn — instead its silhouette clips the run of sibling
    /// layers immediately below it (behind it, in the same parent's
    /// `children`/`Page::layers` list) down to the previous mask or the start
    /// of the list. See `masking::partition_mask_runs`, the single place
    /// that interprets this field (and `ignore_mask` below) for both
    /// `canvas.rs` and `export.rs`. `#[serde(default)]` so documents saved
    /// before this field existed load with masking off everywhere.
    #[serde(default)]
    pub is_mask: bool,
    /// "Ignore Underlying Mask": opts this layer out of being
    /// clipped by a mask above it, so it always renders normally regardless
    /// of `is_mask` layers elsewhere in the same parent's children.
    #[serde(default)]
    pub ignore_mask: bool,
    /// The combine mode this layer contributes when it's a direct child of
    /// a `LayerKind::BooleanGroup` — ignored everywhere else, same "one
    /// field, many places ignore it" convention as `is_mask`/`ignore_mask`
    /// above. A `BooleanGroup`'s own bottommost (z-order-first) child's
    /// `bool_op` is also ignored — it's always the base the others combine
    /// onto, regardless of what's stored here.
    #[serde(default)]
    pub bool_op: BoolOp,
    /// Links this layer to a `Document::layer_styles` entry it was last
    /// applied from; `None` means unlinked (either never applied, or
    /// explicitly detached). Editing a linked layer's `style` fields
    /// directly does not clear this — only "Detach" does (see
    /// `ui/inspector.rs::layer_style_ui`). Same convention as
    /// `LayerKind::Text::style_id`.
    #[serde(default)]
    pub style_id: Option<Uuid>,
    pub kind: LayerKind,
}

impl Layer {
    /// Builds a new visible, unlocked layer with default style and a fresh id.
    pub fn new(name: impl Into<String>, frame: Frame, kind: LayerKind) -> Self {
        Self {
            id: Uuid::new_v4(),
            name: name.into(),
            frame,
            style: Style::default(),
            visible: true,
            locked: false,
            opacity: 1.0,
            is_mask: false,
            ignore_mask: false,
            bool_op: BoolOp::default(),
            style_id: None,
            kind,
        }
    }

    /// Assigns a fresh id to this layer and, recursively, to every
    /// descendant — used when duplicating a layer so the copy doesn't share
    /// ids with the original (which selection, history, and hit-testing all
    /// key off of).
    pub fn regenerate_ids(&mut self) {
        self.id = Uuid::new_v4();
        if let Some(children) = self.kind.children_mut() {
            for child in children {
                child.regenerate_ids();
            }
        }
    }

    /// Builds a new empty `Artboard` layer with a white background and no fill/stroke of its own.
    pub fn new_artboard(name: impl Into<String>, frame: Frame) -> Self {
        let mut layer = Self::new(
            name,
            frame,
            LayerKind::Artboard {
                children: Vec::new(),
                background: Color32::WHITE,
            },
        );
        layer.style = Style { fill: None, stroke: None, ..Default::default() };
        layer
    }

    /// Builds a new `Image` layer displayed at `frame`, from already
    /// PNG-encoded bytes of the given pixel dimensions (see
    /// `image_ops::decode`/`encode_png`).
    pub fn new_image(name: impl Into<String>, frame: Frame, encoded: Vec<u8>, width: u32, height: u32) -> Self {
        let mut layer = Self::new(
            name,
            frame,
            LayerKind::Image {
                encoded,
                width,
                height,
                version: Uuid::new_v4(),
                color_adjust: ColorAdjust::default(),
            },
        );
        layer.style = Style { fill: None, stroke: None, ..Default::default() };
        layer
    }

    /// Recursively finds a layer by id, returning a reference along with
    /// its accumulated offset (sum of ancestor frame positions).
    pub fn find(&self, id: LayerId) -> Option<&Layer> {
        if self.id == id {
            return Some(self);
        }
        if let Some(children) = self.kind.children() {
            for child in children {
                if let Some(found) = child.find(id) {
                    return Some(found);
                }
            }
        }
        None
    }

    /// Recursively finds a layer by id, returning a mutable reference if found.
    pub fn find_mut(&mut self, id: LayerId) -> Option<&mut Layer> {
        if self.id == id {
            return Some(self);
        }
        if let Some(children) = self.kind.children_mut() {
            for child in children {
                if let Some(found) = child.find_mut(id) {
                    return Some(found);
                }
            }
        }
        None
    }

    /// Removes a layer with the given id anywhere in this subtree. Returns the removed layer.
    pub fn remove(&mut self, id: LayerId) -> Option<Layer> {
        if let Some(children) = self.kind.children_mut() {
            if let Some(pos) = children.iter().position(|c| c.id == id) {
                return Some(children.remove(pos));
            }
            for child in children {
                if let Some(found) = child.remove(id) {
                    return Some(found);
                }
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An `Image` layer's PNG bytes are base64-encoded in JSON (see
    /// `base64_bytes`) rather than serialized as a raw byte array — this
    /// locks in that the round trip through `serde_json` still reproduces
    /// the exact original bytes and every other field, the way a
    /// `.sdesign` save/load cycle (`io::save_to`/`io::load_from`) relies on.
    #[test]
    fn image_layer_round_trips_through_json() {
        let encoded: Vec<u8> = (0..=255).collect();
        let layer = Layer::new_image(
            "Photo",
            Frame { pos: Pos2::new(1.0, 2.0), size: Vec2::new(300.0, 150.0), rotation: 0.0 },
            encoded.clone(),
            100,
            50,
        );

        let json = serde_json::to_string(&layer).expect("serialize");
        let round_tripped: Layer = serde_json::from_str(&json).expect("deserialize");

        let LayerKind::Image { encoded: rt_encoded, width, height, version, color_adjust } = &round_tripped.kind
        else {
            panic!("expected an Image layer");
        };
        assert_eq!(rt_encoded, &encoded);
        assert_eq!(*width, 100);
        assert_eq!(*height, 50);
        let LayerKind::Image { version: orig_version, .. } = &layer.kind else { unreachable!() };
        assert_eq!(version, orig_version);
        assert!(color_adjust.is_identity());
    }

    /// A `.sdesign` saved before per-corner rounding existed stored
    /// `corner_radius` as a bare number — loading one must still apply that
    /// value uniformly to all four corners rather than failing to parse.
    #[test]
    fn corner_radii_deserializes_a_bare_number_from_old_json_as_uniform() {
        let radii: CornerRadii = serde_json::from_str("12.5").expect("deserialize");
        assert_eq!(radii, CornerRadii::uniform(12.5));
    }

    #[test]
    fn corner_radii_round_trips_per_corner_values_through_json() {
        let radii = CornerRadii { top_left: 1.0, top_right: 2.0, bottom_right: 3.0, bottom_left: 4.0 };
        let json = serde_json::to_string(&radii).expect("serialize");
        let round_tripped: CornerRadii = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(round_tripped, radii);
    }

    /// `Style::fill_opacity`/`stroke_opacity` must default to `1.0` (fully
    /// opaque) when loading a `.sdesign` saved before these fields existed,
    /// so old documents keep rendering exactly as before.
    #[test]
    fn style_fill_and_stroke_opacity_default_to_one_on_old_json() {
        let json = r#"{"fill":null,"stroke":null}"#;
        let style: Style = serde_json::from_str(json).expect("deserialize");
        assert_eq!(style.fill_opacity, 1.0);
        assert_eq!(style.stroke_opacity, 1.0);
    }

    /// `Frame::rotation` must default to `0.0` when loading a `.sdesign`
    /// saved before the field existed, so old documents keep rendering
    /// exactly as before (see the `#[serde(default)]` on the field).
    #[test]
    fn frame_rotation_defaults_to_zero_on_old_json() {
        let json = r#"{"pos":{"x":0.0,"y":0.0},"size":{"x":10.0,"y":10.0}}"#;
        let frame: Frame = serde_json::from_str(json).expect("deserialize");
        assert_eq!(frame.rotation, 0.0);
    }

    #[test]
    fn rotated_bounds_equals_bounds_at_zero_rotation() {
        let frame = Frame { pos: Pos2::new(1.0, 2.0), size: Vec2::new(30.0, 10.0), rotation: 0.0 };
        assert_eq!(frame.rotated_bounds(), frame.bounds());
    }

    #[test]
    fn rotated_bounds_of_square_rotated_45_degrees_is_larger_aabb() {
        let frame = Frame { pos: Pos2::new(0.0, 0.0), size: Vec2::new(10.0, 10.0), rotation: 45.0 };
        let rb = frame.rotated_bounds();
        // A 10x10 square rotated 45 degrees has an AABB diagonal of 10*sqrt(2).
        let expected = 10.0 * std::f32::consts::SQRT_2;
        assert!((rb.width() - expected).abs() < 1e-3, "width={}", rb.width());
        assert!((rb.height() - expected).abs() < 1e-3, "height={}", rb.height());
        // Center should be preserved.
        assert!((rb.center() - frame.bounds().center()).length() < 1e-3);
    }
}
