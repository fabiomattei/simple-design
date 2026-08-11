use std::cell::Cell;

use egui::Ui;

use crate::alignment::{AlignEdge, DistributeAxis};
use crate::canvas::{apply_text_auto_resize, CanvasWidget};
use crate::grouping;
use crate::history::History;
use crate::model::{
    ArrowCap, ArtboardPreset, BoolOp, ColorAdjust, CornerRadii, Gradient, GradientKind, GradientStop, HalftoneFill,
    Layer, LayerId, LayerKind, LayerStyle, ListType, NoiseFill, Paint, Shadow, Stroke, TextAlign, TextFont,
    TextResize, TextStyle, TextTransform, VerticalAlign, PAPER_PRESETS, SCREEN_PRESETS,
};
use crate::numeric_input::{self, Anchor};
use crate::tools::Tool;
use crate::transform_ops::FlipAxis;
use crate::ui::{color_picker, icons};

/// The nearest parent container's `(width, height)`, in doc units — the
/// 100% reference for percentage dimension input (see
/// `numeric_input::parse_dimension_expr`). Falls back to `fallback` (the
/// field's own current value) when `id` has no parent container, so typing
/// "100%" on a top-level layer degenerates to "unchanged" rather than being
/// undefined.
fn parent_extent(history: &History, id: LayerId, fallback: (f64, f64)) -> (f64, f64) {
    let page = history.get().active_page();
    let Some((Some(parent_id), _)) = grouping::parent_and_index(&page.layers, id) else {
        return fallback;
    };
    match page.find(parent_id) {
        Some(parent) => {
            let b = parent.frame.bounds();
            (b.width() as f64, b.height() as f64)
        }
        None => fallback,
    }
}

/// Heuristic Remove Background's color tolerance (see
/// `image_ops::remove_background`) — fixed rather than user-tunable, since
/// Sketch's own "automatically remove background" has no manual slider
/// either (unlike Magic Wand, which does).
const BACKGROUND_REMOVAL_TOLERANCE: f32 = 24.0;

pub enum InspectorAction {
    /// The header's "X" button — see `app.rs`'s `show_inspector_panel`.
    Close,
    Duplicate,
    Group,
    Ungroup,
    Boolean(BoolOp),
    /// `bool`: Option/Alt was held at click time — Sketch's "align to the
    /// enclosing Artboard instead of the immediate parent" override (see
    /// `alignment::artboard_bounds_in_parent_space`, wired from
    /// `app.rs::align_selection`).
    Align(AlignEdge, bool),
    Distribute(DistributeAxis),
    /// `f32`: spacing, from the inspector's Tidy row (see `alignment::tidy`).
    Tidy(f32),
    Flip(FlipAxis),
    Flatten,
    InsertArtboardPreset(egui::Vec2),
}

/// Shown in place of "No selection" while the Artboard tool is active and
/// nothing has been drawn yet: a fixed-size alternative to dragging out a
/// custom-size artboard on the canvas (still available, unchanged).
fn artboard_preset_picker(ui: &mut Ui) -> Option<InspectorAction> {
    ui.label("New Artboard");
    ui.weak("Choose a size, or drag on the canvas for a custom one.");
    ui.add_space(8.0);

    let mut action = None;
    let mut preset_group = |ui: &mut Ui, label: &str, presets: &[ArtboardPreset]| {
        ui.label(label);
        ui.horizontal_wrapped(|ui| {
            for preset in presets {
                let text = format!("{}\n{}\u{d7}{}", preset.name, preset.size.x as i32, preset.size.y as i32);
                if ui.button(text).clicked() {
                    action = Some(InspectorAction::InsertArtboardPreset(preset.size));
                }
            }
        });
        ui.add_space(4.0);
    };
    preset_group(ui, "Screen", SCREEN_PRESETS);
    preset_group(ui, "Paper", PAPER_PRESETS);
    action
}

/// The "Flip" row shared by both the single- and multi-selection branches of
/// `ui()` below — flipping is well-defined (per-layer, about each layer's own
/// frame center, see `transform_ops::flip_layer`) regardless of how many
/// layers are selected, unlike Align/Distribute.
fn arrow_cap_label(cap: ArrowCap) -> &'static str {
    match cap {
        ArrowCap::None => "None",
        ArrowCap::Line => "Line",
        ArrowCap::Triangle => "Triangle",
        ArrowCap::Disc => "Disc",
    }
}

/// A small `ArrowCap` picker, `salt` making the two (start/end) instances on
/// one layer distinct widget ids. Returns whether the selection changed.
fn arrow_cap_combo(ui: &mut Ui, salt: (&str, LayerId), cap: &mut ArrowCap) -> bool {
    let mut changed = false;
    egui::ComboBox::from_id_salt(salt)
        .selected_text(arrow_cap_label(*cap))
        .width(90.0)
        .show_ui(ui, |ui| {
            for choice in [ArrowCap::None, ArrowCap::Line, ArrowCap::Triangle, ArrowCap::Disc] {
                if ui.selectable_value(cap, choice, arrow_cap_label(choice)).changed() {
                    changed = true;
                }
            }
        });
    changed
}

fn flip_buttons(ui: &mut Ui) -> Option<InspectorAction> {
    let mut action = None;
    ui.label("Flip");
    ui.horizontal(|ui| {
        if icons::flip_button(ui, FlipAxis::Horizontal)
            .on_hover_text("Flip Horizontal (⇧H)")
            .clicked()
        {
            action = Some(InspectorAction::Flip(FlipAxis::Horizontal));
        }
        if icons::flip_button(ui, FlipAxis::Vertical)
            .on_hover_text("Flip Vertical (⇧V)")
            .clicked()
        {
            action = Some(InspectorAction::Flip(FlipAxis::Vertical));
        }
    });
    action
}

/// Snapshots history exactly once at the start of an edit gesture (a drag,
/// or a discrete click/keystroke), rather than every frame a value changes.
fn should_snapshot(resp: &egui::Response) -> bool {
    resp.changed() && (!resp.dragged() || resp.drag_started())
}

/// A stacked list of drop ("outer") or inner shadows (`heading` labels which)
/// — same clone-locally/mutate/write-back-through-`apply` pattern as
/// `paint_editor`'s gradient-stops list, since a `Vec<Shadow>` needs the same
/// per-row edit + add/remove shape. Each row's color popover doubles as the
/// shadow's opacity (its alpha channel — see `Shadow`'s doc comment).
fn shadow_section(ui: &mut Ui, history: &mut History, id: LayerId, heading: &str, shadows: &[Shadow], apply: impl Fn(&mut Layer, Vec<Shadow>)) {
    ui.add_space(4.0);
    ui.label(heading);
    let mut list = shadows.to_vec();
    let mut changed = false;
    let mut snapshot_now = false;
    let mut remove_idx: Option<usize> = None;

    for (i, shadow) in list.iter_mut().enumerate() {
        ui.horizontal(|ui| {
            let cr = color_picker::edit(ui, &mut shadow.color, &history.get().palette);
            if should_snapshot(&cr) {
                snapshot_now = true;
            }
            changed |= cr.changed();

            let xr = ui.add(egui::DragValue::new(&mut shadow.offset.x).prefix("X: ").speed(0.5));
            if should_snapshot(&xr) {
                snapshot_now = true;
            }
            changed |= xr.changed();

            let yr = ui.add(egui::DragValue::new(&mut shadow.offset.y).prefix("Y: ").speed(0.5));
            if should_snapshot(&yr) {
                snapshot_now = true;
            }
            changed |= yr.changed();

            let br = ui.add(egui::DragValue::new(&mut shadow.blur).prefix("Blur: ").range(0.0..=500.0).speed(0.5));
            if should_snapshot(&br) {
                snapshot_now = true;
            }
            changed |= br.changed();

            let sr = ui.add(egui::DragValue::new(&mut shadow.spread).prefix("Spread: ").speed(0.5));
            if should_snapshot(&sr) {
                snapshot_now = true;
            }
            changed |= sr.changed();

            if ui.small_button("🗑").clicked() {
                remove_idx = Some(i);
                changed = true;
                snapshot_now = true;
            }
        });
    }
    if let Some(i) = remove_idx {
        list.remove(i);
    }
    if ui.small_button(format!("+ {heading}")).clicked() {
        list.push(Shadow::default());
        changed = true;
        snapshot_now = true;
    }

    if snapshot_now {
        history.snapshot();
    }
    if changed {
        if let Some(l) = history.mutate().active_page_mut().find_mut(id) {
            apply(l, list);
        }
    }
}

/// One corner's `DragValue` in the `Rectangle` inspector's corner-radius
/// grid — reads `value` (that corner's current radius) and, on change,
/// writes back through `apply` (e.g. `|c, v| c.top_left = v`) into whichever
/// corner of `CornerRadii` it closes over.
fn corner_radius_field(
    ui: &mut Ui,
    history: &mut History,
    id: LayerId,
    corner: icons::RectCorner,
    value: f32,
    apply: impl Fn(&mut CornerRadii, f32),
) {
    let mut v = value;
    let resp = ui
        .horizontal(|ui| {
            icons::corner_radius_icon(ui, corner);
            ui.add(egui::DragValue::new(&mut v).range(0.0..=1000.0))
        })
        .inner;
    if should_snapshot(&resp) {
        history.snapshot();
    }
    if resp.changed() {
        if let Some(l) = history.mutate().active_page_mut().find_mut(id) {
            if let LayerKind::Rectangle { corner_radius } = &mut l.kind {
                apply(corner_radius, v);
            }
        }
    }
}

/// Reads and decodes the image at `path`, re-encoding it to PNG bytes —
/// the same normalization `image_ops::build_image_grid` applies when
/// inserting an `Image` layer, so a `Paint::Pattern`'s `encoded` follows
/// the same "always PNG regardless of source format" convention as
/// `LayerKind::Image::encoded`. `tile_width` defaults to the image's own
/// native width, capped so a large photo doesn't start out tiled
/// absurdly large.
fn pattern_fill_from_path(path: &std::path::Path) -> Option<crate::model::PatternFill> {
    let bytes = std::fs::read(path).ok()?;
    let decoded = crate::image_ops::decode(&bytes)?;
    let tile_width = (decoded.width() as f32).clamp(1.0, 200.0);
    Some(crate::model::PatternFill { encoded: crate::image_ops::encode_png(&decoded), tile_width })
}

/// A Solid/Linear/Radial/Noise/Halftone/Pattern paint editor: a type
/// dropdown (omitted when `allow_gradient` is `false` — a plain
/// solid-color picker for contexts that don't render gradients, e.g. text
/// glyph color) plus whatever controls match the current type — one color
/// picker for `Solid`; a stop list (color + position, add/remove) and an
/// angle (linear) / radius (radial) slider for a `Gradient`; a base color +
/// intensity + grain size + reroll button for `Noise`; background/dot
/// colors + grid scale + coverage for `Halftone`; or a tile-width slider +
/// "Replace Image…" button for `Pattern`. Deliberately inspector-only, no
/// on-canvas draggable handles (see `ROADMAP.md`/gradient fill notes) — the
/// angle/radius/intensity/etc. sliders are the whole direction/tuning
/// surface.
///
/// `allow_texture_fills` is independent of `allow_gradient`: Noise/
/// Halftone/Pattern are all fill-only concepts (stroking a path with a
/// sampled texture isn't implemented) — so the stroke call site passes
/// `false` while still allowing gradients.
///
/// Calls `apply(layer, new_paint)` — through the usual
/// `history.mutate().active_page_mut().find_mut(id)` — on every change,
/// snapshotting first exactly when a drag/edit gesture starts
/// (`should_snapshot`), same convention as every other control here.
fn paint_editor(
    ui: &mut Ui,
    history: &mut History,
    id: LayerId,
    slot: &str,
    current: &Paint,
    allow_gradient: bool,
    allow_texture_fills: bool,
    apply: impl Fn(&mut Layer, Paint),
) {
    if allow_gradient {
        let mut kind_idx = match current {
            Paint::Solid(_) => 0,
            Paint::Gradient(g) => match g.kind {
                GradientKind::Linear => 1,
                GradientKind::Radial => 2,
            },
            Paint::Noise(_) => 3,
            Paint::Halftone(_) => 4,
            Paint::Pattern(_) => 5,
        };
        let prev_idx = kind_idx;
        egui::ComboBox::from_id_salt(("paint-kind", slot, id))
            .selected_text(match kind_idx {
                0 => "Solid",
                1 => "Linear",
                2 => "Radial",
                3 => "Noise",
                4 => "Halftone",
                _ => "Pattern",
            })
            .show_ui(ui, |ui| {
                ui.selectable_value(&mut kind_idx, 0, "Solid");
                ui.selectable_value(&mut kind_idx, 1, "Linear");
                ui.selectable_value(&mut kind_idx, 2, "Radial");
                if allow_texture_fills {
                    ui.selectable_value(&mut kind_idx, 3, "Noise");
                    ui.selectable_value(&mut kind_idx, 4, "Halftone");
                    ui.selectable_value(&mut kind_idx, 5, "Pattern");
                }
            });
        if kind_idx != prev_idx {
            // `Pattern` needs a file picked synchronously before there's
            // anything to apply — same blocking `rfd` call every other
            // picker in this app already makes (just from here instead of
            // an `app.rs` menu handler). Cancelling the dialog leaves the
            // fill type unchanged, unlike every other option here.
            if kind_idx == 5 {
                if let Some(path) = crate::io::open_pattern_image_dialog() {
                    if let Some(pattern) = pattern_fill_from_path(&path) {
                        history.snapshot();
                        if let Some(l) = history.mutate().active_page_mut().find_mut(id) {
                            apply(l, Paint::Pattern(pattern));
                        }
                    }
                }
                return;
            }
            history.snapshot();
            let base = current.to_color32();
            let new_paint = match kind_idx {
                0 => Paint::Solid(base),
                1 => Paint::Gradient(Gradient::linear(base, base)),
                2 => Paint::Gradient(Gradient::radial(base, base)),
                3 => Paint::Noise(NoiseFill { base, intensity: 0.5, scale: 4.0, seed: 1 }),
                _ => Paint::Halftone(HalftoneFill { background: egui::Color32::WHITE, dot: base, scale: 8.0, coverage: 0.7 }),
            };
            if let Some(l) = history.mutate().active_page_mut().find_mut(id) {
                apply(l, new_paint);
            }
            return;
        }
    }

    match current {
        Paint::Solid(color) => {
            let mut color = *color;
            let cr = color_picker::edit(ui, &mut color, &history.get().palette);
            if should_snapshot(&cr) {
                history.snapshot();
            }
            if cr.changed() {
                if let Some(l) = history.mutate().active_page_mut().find_mut(id) {
                    apply(l, Paint::Solid(color));
                }
            }
        }
        Paint::Gradient(gradient) => {
            let mut g = gradient.clone();
            let mut changed = false;
            let mut snapshot_now = false;

            let mut remove_idx: Option<usize> = None;
            let stop_count = g.stops.len();
            for (i, stop) in g.stops.iter_mut().enumerate() {
                ui.horizontal(|ui| {
                    let cr = color_picker::edit(ui, &mut stop.color, &history.get().palette);
                    if should_snapshot(&cr) {
                        snapshot_now = true;
                    }
                    changed |= cr.changed();
                    let or = ui.add(
                        egui::DragValue::new(&mut stop.offset)
                            .range(0.0..=1.0)
                            .speed(0.01)
                            .prefix("at "),
                    );
                    if should_snapshot(&or) {
                        snapshot_now = true;
                    }
                    changed |= or.changed();
                    if stop_count > 2 && ui.small_button("🗑").clicked() {
                        remove_idx = Some(i);
                        changed = true;
                        snapshot_now = true;
                    }
                });
            }
            if let Some(i) = remove_idx {
                g.stops.remove(i);
            }
            if ui.small_button("+ Stop").clicked() {
                let last = g.stops.last().copied().unwrap_or(GradientStop { offset: 1.0, color: egui::Color32::WHITE });
                g.stops.push(GradientStop { offset: (last.offset + 0.1).min(1.0), color: last.color });
                changed = true;
                snapshot_now = true;
            }

            match g.kind {
                GradientKind::Linear => {
                    let axis = g.to - g.from;
                    let mut angle = axis.y.atan2(axis.x).to_degrees();
                    let ar = ui.add(egui::Slider::new(&mut angle, 0.0..=360.0).suffix("°").text("Angle"));
                    if should_snapshot(&ar) {
                        snapshot_now = true;
                    }
                    if ar.changed() {
                        let rad = angle.to_radians();
                        let half = egui::Vec2::new(rad.cos(), rad.sin()) * 0.5;
                        g.from = egui::Pos2::new(0.5, 0.5) - half;
                        g.to = egui::Pos2::new(0.5, 0.5) + half;
                        changed = true;
                    }
                }
                GradientKind::Radial => {
                    let mut radius = (g.to - g.from).length();
                    let rr = ui.add(egui::Slider::new(&mut radius, 0.05..=1.5).text("Radius"));
                    if should_snapshot(&rr) {
                        snapshot_now = true;
                    }
                    if rr.changed() {
                        g.to = g.from + egui::Vec2::new(radius, 0.0);
                        changed = true;
                    }
                }
            }

            if snapshot_now {
                history.snapshot();
            }
            if changed {
                g.stops.sort_by(|a, b| a.offset.partial_cmp(&b.offset).unwrap_or(std::cmp::Ordering::Equal));
                if let Some(l) = history.mutate().active_page_mut().find_mut(id) {
                    apply(l, Paint::Gradient(g));
                }
            }
        }
        Paint::Noise(noise) => {
            let mut n = *noise;
            let mut changed = false;
            let mut snapshot_now = false;

            let cr = color_picker::edit(ui, &mut n.base, &history.get().palette);
            if should_snapshot(&cr) {
                snapshot_now = true;
            }
            changed |= cr.changed();

            let ir = ui.add(egui::Slider::new(&mut n.intensity, 0.0..=1.0).text("Intensity"));
            if should_snapshot(&ir) {
                snapshot_now = true;
            }
            changed |= ir.changed();

            let sr = ui.add(egui::Slider::new(&mut n.scale, 0.5..=40.0).text("Grain size"));
            if should_snapshot(&sr) {
                snapshot_now = true;
            }
            changed |= sr.changed();

            if ui.small_button("🎲 Regenerate").clicked() {
                // A fixed odd increment, not a time-based RNG — deterministic,
                // but looks arbitrary to the user on each click, with no new
                // dependency needed (see `noise_fill` module doc).
                n.seed = n.seed.wrapping_add(0x9E37_79B9);
                changed = true;
                snapshot_now = true;
            }

            if snapshot_now {
                history.snapshot();
            }
            if changed {
                if let Some(l) = history.mutate().active_page_mut().find_mut(id) {
                    apply(l, Paint::Noise(n));
                }
            }
        }
        Paint::Halftone(halftone) => {
            let mut h = *halftone;
            let mut changed = false;
            let mut snapshot_now = false;

            ui.horizontal(|ui| {
                ui.label("Background");
                let cr = color_picker::edit(ui, &mut h.background, &history.get().palette);
                if should_snapshot(&cr) {
                    snapshot_now = true;
                }
                changed |= cr.changed();
            });
            ui.horizontal(|ui| {
                ui.label("Dot");
                let cr = color_picker::edit(ui, &mut h.dot, &history.get().palette);
                if should_snapshot(&cr) {
                    snapshot_now = true;
                }
                changed |= cr.changed();
            });

            let sr = ui.add(egui::Slider::new(&mut h.scale, 2.0..=60.0).text("Grid size"));
            if should_snapshot(&sr) {
                snapshot_now = true;
            }
            changed |= sr.changed();

            let mut coverage_pct = h.coverage * 100.0;
            let cvr = ui.add(egui::Slider::new(&mut coverage_pct, 0.0..=100.0).suffix("%").text("Coverage"));
            if should_snapshot(&cvr) {
                snapshot_now = true;
            }
            if cvr.changed() {
                h.coverage = coverage_pct / 100.0;
                changed = true;
            }

            if snapshot_now {
                history.snapshot();
            }
            if changed {
                if let Some(l) = history.mutate().active_page_mut().find_mut(id) {
                    apply(l, Paint::Halftone(h));
                }
            }
        }
        Paint::Pattern(pattern) => {
            let mut p = pattern.clone();
            let mut changed = false;
            let mut snapshot_now = false;

            let tr = ui.add(egui::DragValue::new(&mut p.tile_width).prefix("Tile width: ").range(1.0..=2000.0));
            if should_snapshot(&tr) {
                snapshot_now = true;
            }
            changed |= tr.changed();

            if ui.small_button("Replace Image…").clicked() {
                if let Some(path) = crate::io::open_pattern_image_dialog() {
                    if let Some(new_pattern) = pattern_fill_from_path(&path) {
                        // Keeps the current `tile_width` rather than
                        // resetting it to the new image's native width —
                        // replacing the tile shouldn't undo a scale the
                        // user already dialed in.
                        p.encoded = new_pattern.encoded;
                        changed = true;
                        snapshot_now = true;
                    }
                }
            }

            if snapshot_now {
                history.snapshot();
            }
            if changed {
                if let Some(l) = history.mutate().active_page_mut().find_mut(id) {
                    apply(l, Paint::Pattern(p));
                }
            }
        }
    }
}

/// Display label for the Font picker's collapsed/selected state.
fn font_label(font: &TextFont) -> &str {
    match font {
        TextFont::Proportional => "Sans",
        TextFont::Monospace => "Mono",
        TextFont::Serif => "Serif",
        TextFont::Display => "Display",
        TextFont::Handwriting => "Handwriting",
        TextFont::System(name) => name,
    }
}

/// The effective value of a character-level property across `selection` —
/// `Some(base_value)` unconditionally when `runs` is empty (uniform style,
/// so nothing can be mixed), otherwise `text_runs::mixed_or`'s usual
/// "`None` if the selection spans differing values" query. Drives every
/// dual-mode Text control's displayed state (and, for a boolean toggle,
/// which direction the next click sets: mixed or unset → set all to
/// `true`; uniformly `true` → clear to `false`).
fn selected_or_base<T: PartialEq + Clone>(
    runs: &[crate::model::text_runs::TextRun],
    selection: &std::ops::Range<usize>,
    base_value: T,
    field: impl Fn(&crate::model::text_runs::RunStyle) -> T,
) -> Option<T> {
    if runs.is_empty() {
        Some(base_value)
    } else {
        crate::model::text_runs::mixed_or(runs, selection.clone(), field)
    }
}

/// Captures `layer`'s current text properties (everything a `TextStyle`
/// holds) as a new named style, keyed by `style_id`.
fn text_style_from_layer(name: String, style_id: uuid::Uuid, layer: &Layer) -> Option<TextStyle> {
    let LayerKind::Text {
        font_size,
        font,
        align,
        vertical_align,
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
        ..
    } = &layer.kind
    else {
        return None;
    };
    Some(TextStyle {
        id: style_id,
        name,
        font_size: *font_size,
        font: font.clone(),
        align: *align,
        vertical_align: *vertical_align,
        line_height: *line_height,
        letter_spacing: *letter_spacing,
        paragraph_spacing: *paragraph_spacing,
        bold: *bold,
        italic: *italic,
        underline: *underline,
        strikethrough: *strikethrough,
        transform: *transform,
        list: *list,
        list_start: *list_start,
        fill: layer.style.fill.as_ref().map(crate::model::Paint::to_color32),
    })
}

/// Copies every field of `style` onto `layer` — both `LayerKind::Text`'s
/// fields and the outer `Layer::style.fill` — and links it via `style_id`.
/// `content` and geometry are untouched. Used both when applying a style to
/// a layer and, in `text_style_ui`'s "Update Style", when propagating an
/// edited style back to every other layer linked to it.
fn apply_style_to_text(layer: &mut Layer, style: &TextStyle) {
    layer.style.fill = style.fill.map(crate::model::Paint::Solid);
    if let LayerKind::Text {
        font_size,
        font,
        align,
        vertical_align,
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
        style_id,
        ..
    } = &mut layer.kind
    {
        *font_size = style.font_size;
        *font = style.font.clone();
        *align = style.align;
        *vertical_align = style.vertical_align;
        *line_height = style.line_height;
        *letter_spacing = style.letter_spacing;
        *paragraph_spacing = style.paragraph_spacing;
        *bold = style.bold;
        *italic = style.italic;
        *underline = style.underline;
        *strikethrough = style.strikethrough;
        *transform = style.transform;
        *list = style.list;
        *list_start = style.list_start;
        *style_id = Some(style.id);
    }
}

/// Sketch's "Text Styles": a small shared/linked style library
/// (`Document::text_styles`). Applying a style copies its fields onto the
/// layer and links `style_id`; "Update Style" edits the style itself and
/// propagates to every other layer linked to it (`Document::for_each_layer_mut`);
/// "Detach" unlinks without changing the layer's current values.
fn text_style_ui(ui: &mut Ui, history: &mut History, id: LayerId, style_id: Option<uuid::Uuid>) {
    let ctx = ui.ctx().clone();
    ui.add_space(4.0);
    ui.separator();
    ui.label("Text Style");

    let styles = history.get().text_styles.clone();
    if !styles.is_empty() {
        ui.horizontal_wrapped(|ui| {
            for style in &styles {
                let active = style_id == Some(style.id);
                if ui.selectable_label(active, &style.name).clicked() && !active {
                    history.snapshot();
                    if let Some(l) = history.mutate().active_page_mut().find_mut(id) {
                        apply_style_to_text(l, style);
                        apply_text_auto_resize(&ctx, l);
                    }
                }
            }
        });
    }

    let name_buf_id = ui.id().with("new_text_style_name");
    let mut name_buf = ui.memory_mut(|m| m.data.get_persisted_mut_or_default::<String>(name_buf_id).clone());
    ui.horizontal(|ui| {
        ui.add(egui::TextEdit::singleline(&mut name_buf).hint_text("Style name"));
        if ui.add_enabled(!name_buf.trim().is_empty(), egui::Button::new("Save as New Style")).clicked() {
            history.snapshot();
            let new_id = uuid::Uuid::new_v4();
            if let Some(layer) = history.get().find(id) {
                if let Some(style) = text_style_from_layer(name_buf.trim().to_string(), new_id, layer) {
                    history.mutate().text_styles.push(style);
                    if let Some(l) = history.mutate().active_page_mut().find_mut(id) {
                        if let LayerKind::Text { style_id, .. } = &mut l.kind {
                            *style_id = Some(new_id);
                        }
                    }
                    name_buf.clear();
                }
            }
        }
    });
    ui.memory_mut(|m| *m.data.get_persisted_mut_or_default::<String>(name_buf_id) = name_buf);

    if let Some(sid) = style_id {
        ui.horizontal(|ui| {
            if ui
                .button("Update Style")
                .on_hover_text("Push this layer's current text properties to every layer linked to this style")
                .clicked()
            {
                history.snapshot();
                let updated = history
                    .get()
                    .find(id)
                    .zip(history.get().text_styles.iter().find(|s| s.id == sid).map(|s| s.name.clone()))
                    .and_then(|(layer, name)| text_style_from_layer(name, sid, layer));
                if let Some(updated) = updated {
                    let doc = history.mutate();
                    if let Some(existing) = doc.text_styles.iter_mut().find(|s| s.id == sid) {
                        *existing = updated.clone();
                    }
                    doc.for_each_layer_mut(&mut |l| {
                        let linked = matches!(&l.kind, LayerKind::Text { style_id: Some(s), .. } if *s == sid);
                        if linked {
                            apply_style_to_text(l, &updated);
                            apply_text_auto_resize(&ctx, l);
                        }
                    });
                }
            }
            if ui.button("Detach").clicked() {
                history.snapshot();
                if let Some(l) = history.mutate().active_page_mut().find_mut(id) {
                    if let LayerKind::Text { style_id, .. } = &mut l.kind {
                        *style_id = None;
                    }
                }
            }
        });
    }
}

/// Captures `layer.style`'s current fields as a new named `LayerStyle`,
/// keyed by `style_id`.
fn layer_style_from_layer(name: String, style_id: uuid::Uuid, layer: &Layer) -> LayerStyle {
    LayerStyle {
        id: style_id,
        name,
        fill: layer.style.fill.clone(),
        stroke: layer.style.stroke.clone(),
        fill_opacity: layer.style.fill_opacity,
        stroke_opacity: layer.style.stroke_opacity,
        shadows: layer.style.shadows.clone(),
        inner_shadows: layer.style.inner_shadows.clone(),
    }
}

/// Copies every field of `style` onto `layer.style` and links `layer.style_id`.
/// `frame`/`kind`/`opacity` are untouched. Used both when applying a style to
/// a layer and, in `layer_style_ui`'s "Update Style", when propagating an
/// edited style back to every other layer linked to it.
fn apply_layer_style(layer: &mut Layer, style: &LayerStyle) {
    layer.style.fill = style.fill.clone();
    layer.style.stroke = style.stroke.clone();
    layer.style.fill_opacity = style.fill_opacity;
    layer.style.stroke_opacity = style.stroke_opacity;
    layer.style.shadows = style.shadows.clone();
    layer.style.inner_shadows = style.inner_shadows.clone();
    layer.style_id = Some(style.id);
}

/// Sketch's "Layer Styles": a small shared/linked style library
/// (`Document::layer_styles`) covering Fill/Border/Shadow, the shape-side
/// sibling of `text_style_ui`. Applying a style copies its fields onto
/// `layer.style` and links `style_id`; "Update Style" edits the style itself
/// and propagates to every other layer linked to it
/// (`Document::for_each_layer_mut`); "Detach" unlinks without changing the
/// layer's current values. Right-clicking a style chip opens a small manager
/// (rename/delete) — deleting a style unlinks every layer that referenced it
/// rather than leaving a dangling `style_id`.
fn layer_style_ui(ui: &mut Ui, history: &mut History, id: LayerId, style_id: Option<uuid::Uuid>) {
    ui.add_space(4.0);
    ui.separator();
    ui.label("Layer Style");

    let styles = history.get().layer_styles.clone();
    if !styles.is_empty() {
        ui.horizontal_wrapped(|ui| {
            for style in &styles {
                let active = style_id == Some(style.id);
                let resp = ui.selectable_label(active, &style.name);
                if resp.clicked() && !active {
                    history.snapshot();
                    if let Some(l) = history.mutate().active_page_mut().find_mut(id) {
                        apply_layer_style(l, style);
                    }
                }
                resp.context_menu(|ui| {
                    let name_buf_id = ui.id().with(("rename_layer_style", style.id));
                    let mut name_buf = ui.memory_mut(|m| {
                        m.data
                            .get_persisted_mut_or_insert_with(name_buf_id, || style.name.clone())
                            .clone()
                    });
                    ui.horizontal(|ui| {
                        ui.label("Name:");
                        let name_resp = ui.add(egui::TextEdit::singleline(&mut name_buf).desired_width(100.0));
                        if name_resp.changed() {
                            ui.memory_mut(|m| *m.data.get_persisted_mut_or_default::<String>(name_buf_id) = name_buf.clone());
                        }
                        let trimmed = name_buf.trim();
                        if name_resp.lost_focus() && !trimmed.is_empty() && trimmed != style.name {
                            history.snapshot();
                            if let Some(existing) =
                                history.mutate().layer_styles.iter_mut().find(|s| s.id == style.id)
                            {
                                existing.name = trimmed.to_string();
                            }
                        }
                    });
                    if ui.button("Delete Style").clicked() {
                        history.snapshot();
                        let doc = history.mutate();
                        doc.layer_styles.retain(|s| s.id != style.id);
                        doc.for_each_layer_mut(&mut |l| {
                            if l.style_id == Some(style.id) {
                                l.style_id = None;
                            }
                        });
                        ui.close();
                    }
                });
            }
        });
    }

    let name_buf_id = ui.id().with("new_layer_style_name");
    let mut name_buf = ui.memory_mut(|m| m.data.get_persisted_mut_or_default::<String>(name_buf_id).clone());
    ui.horizontal(|ui| {
        ui.add(egui::TextEdit::singleline(&mut name_buf).hint_text("Style name"));
        if ui.add_enabled(!name_buf.trim().is_empty(), egui::Button::new("Save as New Style")).clicked() {
            history.snapshot();
            let new_id = uuid::Uuid::new_v4();
            if let Some(layer) = history.get().find(id) {
                let style = layer_style_from_layer(name_buf.trim().to_string(), new_id, layer);
                history.mutate().layer_styles.push(style);
                if let Some(l) = history.mutate().active_page_mut().find_mut(id) {
                    l.style_id = Some(new_id);
                }
                name_buf.clear();
            }
        }
    });
    ui.memory_mut(|m| *m.data.get_persisted_mut_or_default::<String>(name_buf_id) = name_buf);

    if let Some(sid) = style_id {
        ui.horizontal(|ui| {
            if ui
                .button("Update Style")
                .on_hover_text("Push this layer's current fill/border/shadow to every layer linked to this style")
                .clicked()
            {
                history.snapshot();
                let updated = history
                    .get()
                    .find(id)
                    .zip(history.get().layer_styles.iter().find(|s| s.id == sid).map(|s| s.name.clone()))
                    .map(|(layer, name)| layer_style_from_layer(name, sid, layer));
                if let Some(updated) = updated {
                    let doc = history.mutate();
                    if let Some(existing) = doc.layer_styles.iter_mut().find(|s| s.id == sid) {
                        *existing = updated.clone();
                    }
                    doc.for_each_layer_mut(&mut |l| {
                        if l.style_id == Some(sid) {
                            apply_layer_style(l, &updated);
                        }
                    });
                }
            }
            if ui.button("Detach").clicked() {
                history.snapshot();
                if let Some(l) = history.mutate().active_page_mut().find_mut(id) {
                    l.style_id = None;
                }
            }
        });
    }
}

/// "Replace Image": swaps `id`'s pixel content for a file picked via a
/// dialog, keeping the current frame (so the new image stretches/shrinks to
/// fit — "Reset to Original Size" undoes that if unwanted, matching
/// Sketch's own Replace Image behavior).
fn replace_image(history: &mut History, id: LayerId) {
    let Some(mut paths) = crate::io::open_image_dialog() else { return };
    let Some(path) = paths.drain(..).next() else { return };
    let Ok(bytes) = std::fs::read(&path) else { return };
    let Some(decoded) = crate::image_ops::decode(&bytes) else { return };
    history.snapshot();
    if let Some(l) = history.mutate().active_page_mut().find_mut(id) {
        if let LayerKind::Image { encoded, width, height, version, .. } = &mut l.kind {
            *encoded = crate::image_ops::encode_png(&decoded);
            *width = decoded.width();
            *height = decoded.height();
            *version = uuid::Uuid::new_v4();
        }
    }
}

fn align_label(edge: AlignEdge) -> &'static str {
    match edge {
        AlignEdge::Left => "Align left",
        AlignEdge::HCenter => "Align center horizontally",
        AlignEdge::Right => "Align right",
        AlignEdge::Top => "Align top",
        AlignEdge::VCenter => "Align center vertically",
        AlignEdge::Bottom => "Align bottom",
    }
}

/// Draws the Inspector panel for the current selection/tool and applies any edits directly to `history`.
pub fn ui(
    ui: &mut Ui,
    history: &mut History,
    selection: &[LayerId],
    canvas: &mut CanvasWidget,
    tool: Tool,
) -> Option<InspectorAction> {
    let mut close_clicked = false;
    ui.horizontal(|ui| {
        ui.heading("Inspector");
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            close_clicked = icons::close_button(ui).clicked();
        });
    });
    if close_clicked {
        return Some(InspectorAction::Close);
    }
    ui.separator();

    // Sketch's "Scale Layers" mode (`K`) — shown regardless of single/multi
    // selection, since it's a drag-handle mode on the canvas, not a
    // property editor. See `CanvasWidget::scaling`'s doc comment for the
    // three ways to exit (this Finish button is one of them).
    if canvas.scaling.is_some() {
        ui.label(egui::RichText::new("Scale Layers").strong());
        ui.label("Resize a handle to scale the layer(s) and their stroke width/corner radius together.");
        ui.horizontal(|ui| {
            ui.label("Anchor:");
            ui.selectable_value(&mut canvas.scale_anchor, crate::canvas::ScaleAnchor::Corners, "Corners");
            ui.selectable_value(&mut canvas.scale_anchor, crate::canvas::ScaleAnchor::Center, "Center");
        });
        if ui.button("Finish").clicked() {
            canvas.scaling = None;
        }
        ui.separator();
    }

    if selection.len() > 1 {
        ui.label(format!("{} layers selected", selection.len()));
        ui.add_space(8.0);

        let mut return_action = None;
        ui.label("Align");
        ui.horizontal(|ui| {
            for edge in [AlignEdge::Left, AlignEdge::HCenter, AlignEdge::Right] {
                if icons::align_button(ui, edge)
                    .on_hover_text(align_label(edge))
                    .clicked()
                {
                    return_action = Some(InspectorAction::Align(edge, ui.input(|i| i.modifiers.alt)));
                }
            }
            for edge in [AlignEdge::Top, AlignEdge::VCenter, AlignEdge::Bottom] {
                if icons::align_button(ui, edge)
                    .on_hover_text(align_label(edge))
                    .clicked()
                {
                    return_action = Some(InspectorAction::Align(edge, ui.input(|i| i.modifiers.alt)));
                }
            }
        });

        ui.add_space(4.0);
        ui.label("Distribute");
        ui.horizontal(|ui| {
            let enabled = selection.len() >= 3;
            if icons::distribute_button(ui, DistributeAxis::Horizontal, enabled)
                .on_hover_text("Distribute horizontal spacing")
                .clicked()
            {
                return_action = Some(InspectorAction::Distribute(DistributeAxis::Horizontal));
            }
            if icons::distribute_button(ui, DistributeAxis::Vertical, enabled)
                .on_hover_text("Distribute vertical spacing")
                .clicked()
            {
                return_action = Some(InspectorAction::Distribute(DistributeAxis::Vertical));
            }
        });
        ui.add_space(4.0);
        ui.label("Tidy");
        ui.horizontal(|ui| {
            ui.add(egui::DragValue::new(&mut canvas.tidy_spacing).prefix("Spacing: ").range(0.0..=500.0));
            if ui.button("Tidy").on_hover_text("Arrange into an even grid").clicked() {
                return_action = Some(InspectorAction::Tidy(canvas.tidy_spacing));
            }
        });
        if return_action.is_some() {
            return return_action;
        }

        ui.add_space(4.0);
        if let Some(action) = flip_buttons(ui) {
            return Some(action);
        }

        ui.add_space(8.0);
        ui.separator();
        if icons::duplicate_button(ui).on_hover_text("Duplicate (Cmd+D)").clicked() {
            return Some(InspectorAction::Duplicate);
        }
        if ui.button("Group (Cmd+G)").clicked() {
            return Some(InspectorAction::Group);
        }
        ui.add_space(8.0);
        ui.separator();
        ui.label("Combine");
        ui.horizontal(|ui| {
            for op in [
                BoolOp::Union,
                BoolOp::Subtract,
                BoolOp::Intersect,
                BoolOp::Difference,
                BoolOp::Add,
            ] {
                if icons::combine_button(ui, op).on_hover_text(op.label()).clicked() {
                    return_action = Some(InspectorAction::Boolean(op));
                }
            }
        });
        if return_action.is_some() {
            return return_action;
        }
        if icons::flatten_button(ui).on_hover_text("Flatten").clicked() {
            return Some(InspectorAction::Flatten);
        }
        return None;
    }

    let Some(&id) = selection.first() else {
        if tool == Tool::Artboard {
            return artboard_preset_picker(ui);
        }
        ui.weak("No selection");
        return None;
    };
    let Some(layer) = history.get().find(id).cloned() else {
        ui.weak("No selection");
        return None;
    };
    // A non-empty text selection in the in-place editor, for `id`
    // specifically — drives every dual-mode Text control below (Bold/
    // Italic/Underline/Strikethrough, font, size, fill) between "format
    // just the selected range" and today's "format the whole layer".
    let text_selection: Option<std::ops::Range<usize>> =
        if canvas.is_editing_text(id) { canvas.text_edit_selection.clone().filter(|r| !r.is_empty()) } else { None };

    ui.label(egui::RichText::new(&layer.name).strong());
    ui.label(layer.kind.type_name());
    ui.add_space(8.0);

    // `x_anchor`/`y_anchor`/`w_anchor`/`h_anchor` are the anchor-letter side
    // channel out of `DragValue::custom_parser` (which can only return the
    // parsed number itself, not the trailing `l`/`r`/`t`/`b`/`c`/`m` letter
    // — see `numeric_input::parse_dimension_expr`'s doc comment). Each is
    // set as a side effect of parsing, then read back right after the
    // widget to decide how the anchor affects `frame.pos`.
    let (extent_w, extent_h) = parent_extent(history, id, (layer.frame.size.x as f64, layer.frame.size.y as f64));

    ui.label("Position");
    ui.horizontal(|ui| {
        let mut x = layer.frame.pos.x;
        let mut y = layer.frame.pos.y;
        let (cur_x, cur_y) = (x as f64, y as f64);
        let x_anchor: Cell<Option<Anchor>> = Cell::new(None);
        let y_anchor: Cell<Option<Anchor>> = Cell::new(None);
        let rx = ui.add(egui::DragValue::new(&mut x).prefix("X: ").custom_parser(|s| {
            let (val, anchor) = numeric_input::parse_dimension_expr(cur_x, s, extent_w)?;
            x_anchor.set(anchor);
            Some(val)
        }));
        let ry = ui.add(egui::DragValue::new(&mut y).prefix("Y: ").custom_parser(|s| {
            let (val, anchor) = numeric_input::parse_dimension_expr(cur_y, s, extent_h)?;
            y_anchor.set(anchor);
            Some(val)
        }));
        if should_snapshot(&rx) || should_snapshot(&ry) {
            history.snapshot();
        }
        if rx.changed() || ry.changed() {
            if let Some(l) = history.mutate().active_page_mut().find_mut(id) {
                let (w, h) = (l.frame.size.x, l.frame.size.y);
                // A typed X/Y value positions whichever edge/center the
                // anchor letter names, not always the top-left corner — see
                // `numeric_input`'s doc comment on the Position/Size syntax.
                l.frame.pos.x = match x_anchor.get() {
                    Some(Anchor::Right) => x - w,
                    Some(Anchor::Center) => x - w / 2.0,
                    _ => x,
                };
                l.frame.pos.y = match y_anchor.get() {
                    Some(Anchor::Bottom) => y - h,
                    Some(Anchor::Center) => y - h / 2.0,
                    _ => y,
                };
            }
        }
    });

    ui.label("Size");
    ui.horizontal(|ui| {
        let mut w = layer.frame.size.x;
        let mut h = layer.frame.size.y;
        let (cur_w, cur_h) = (w as f64, h as f64);
        let w_anchor: Cell<Option<Anchor>> = Cell::new(None);
        let h_anchor: Cell<Option<Anchor>> = Cell::new(None);
        let rw = ui.add(egui::DragValue::new(&mut w).prefix("W: ").custom_parser(|s| {
            let (val, anchor) = numeric_input::parse_dimension_expr(cur_w, s, extent_w)?;
            w_anchor.set(anchor);
            Some(val)
        }));
        let rh = ui.add(egui::DragValue::new(&mut h).prefix("H: ").custom_parser(|s| {
            let (val, anchor) = numeric_input::parse_dimension_expr(cur_h, s, extent_h)?;
            h_anchor.set(anchor);
            Some(val)
        }));
        let lock_icon = if canvas.aspect_locked { "🔒" } else { "🔓" };
        if ui
            .button(lock_icon)
            .on_hover_text("Lock proportions (⌥⌘L)")
            .clicked()
        {
            canvas.aspect_locked = !canvas.aspect_locked;
        }
        if should_snapshot(&rw) || should_snapshot(&rh) {
            history.snapshot();
        }
        if rw.changed() || rh.changed() {
            if let Some(l) = history.mutate().active_page_mut().find_mut(id) {
                let (old_w, old_h) = (l.frame.size.x, l.frame.size.y);
                let mut new_w = w;
                let mut new_h = h;
                // Proportional lock cascades whichever field the user
                // *didn't* type into, based on the other's ratio change.
                if canvas.aspect_locked {
                    if rw.changed() && old_w != 0.0 {
                        new_h = old_h * (w / old_w);
                    } else if rh.changed() && old_h != 0.0 {
                        new_w = old_w * (h / old_h);
                    }
                }
                if let Some(a) = w_anchor.get() {
                    match a {
                        Anchor::Right => l.frame.pos.x += old_w - new_w,
                        Anchor::Center => l.frame.pos.x += (old_w - new_w) / 2.0,
                        _ => {}
                    }
                }
                if let Some(a) = h_anchor.get() {
                    match a {
                        Anchor::Bottom => l.frame.pos.y += old_h - new_h,
                        Anchor::Center => l.frame.pos.y += (old_h - new_h) / 2.0,
                        _ => {}
                    }
                }
                l.frame.size.x = new_w;
                l.frame.size.y = new_h;
            }
        }
    });

    // Rotation is meaningless for an Artboard (a page-frame boundary, not a
    // rotatable shape — see `model/layer.rs`'s `Frame::rotation` doc
    // comment) and, for a multi-selection, ambiguous as a single absolute
    // value (only well-defined as a *delta*, which the interactive
    // drag-to-rotate handle provides) — so this field only ever shows for a
    // single non-Artboard layer.
    if !matches!(layer.kind, LayerKind::Artboard { .. }) {
        ui.label("Rotation");
        ui.horizontal(|ui| {
            let mut rotation = layer.frame.rotation;
            let r = ui.add(egui::DragValue::new(&mut rotation).prefix("° ").speed(1.0));
            if should_snapshot(&r) {
                history.snapshot();
            }
            if r.changed() {
                if let Some(l) = history.mutate().active_page_mut().find_mut(id) {
                    l.frame.rotation = rotation;
                }
            }
        });
    }

    ui.add_space(4.0);
    if let Some(action) = flip_buttons(ui) {
        return Some(action);
    }

    if let LayerKind::Path { closed, .. } = &layer.kind {
        let mut is_closed = *closed;
        let r = ui.checkbox(&mut is_closed, "Closed");
        if r.changed() {
            history.snapshot();
            if let Some(l) = history.mutate().active_page_mut().find_mut(id) {
                if let LayerKind::Path { closed, .. } = &mut l.kind {
                    *closed = is_closed;
                }
            }
        }

        // Per-point corner radius/smoothing — only meaningful once at least
        // one anchor is selected (`canvas.has_point_selection()`; point
        // selection is keyboard/click-driven in the canvas itself, same as
        // the Num1-4 point-type shortcuts, so this is the first inspector UI
        // for individual path points rather than the whole layer).
        if canvas.has_point_selection() {
            if let Some(radius) = canvas.selected_point_corner_radius(history, id) {
                ui.horizontal(|ui| {
                    let mut r = radius;
                    let resp = ui.add(
                        egui::DragValue::new(&mut r)
                            .prefix("Corner radius: ")
                            .range(0.0..=100000.0),
                    );
                    if should_snapshot(&resp) {
                        history.snapshot();
                    }
                    if resp.changed() {
                        canvas.apply_corner_radius(history, id, r);
                    }
                    if ui.button("Max").on_hover_text("Snap to the largest radius this corner allows").clicked() {
                        canvas.apply_max_corner_radius(history, id);
                    }
                });
            }
        }
    }

    if let LayerKind::Rectangle { corner_radius } = &layer.kind {
        let radii = *corner_radius;
        ui.label("Corner radius");
        egui::Grid::new(("corner_radius_grid", id)).num_columns(2).spacing([8.0, 4.0]).show(ui, |ui| {
            corner_radius_field(ui, history, id, icons::RectCorner::TopLeft, radii.top_left, |c, v| c.top_left = v);
            corner_radius_field(ui, history, id, icons::RectCorner::TopRight, radii.top_right, |c, v| c.top_right = v);
            ui.end_row();
            corner_radius_field(ui, history, id, icons::RectCorner::BottomLeft, radii.bottom_left, |c, v| c.bottom_left = v);
            corner_radius_field(ui, history, id, icons::RectCorner::BottomRight, radii.bottom_right, |c, v| c.bottom_right = v);
            ui.end_row();
        });
    }

    if let LayerKind::Star { points, inner_ratio } = &layer.kind {
        let mut point_count = *points;
        let mut ratio = *inner_ratio;
        ui.horizontal(|ui| {
            let pr = ui.add(egui::DragValue::new(&mut point_count).prefix("Points: ").range(3..=30));
            let rr = ui.add(
                egui::DragValue::new(&mut ratio)
                    .prefix("Inner: ")
                    .range(0.0..=1.0)
                    .speed(0.01),
            );
            if should_snapshot(&pr) || should_snapshot(&rr) {
                history.snapshot();
            }
            if pr.changed() || rr.changed() {
                if let Some(l) = history.mutate().active_page_mut().find_mut(id) {
                    if let LayerKind::Star { points, inner_ratio } = &mut l.kind {
                        *points = point_count;
                        *inner_ratio = ratio;
                    }
                }
            }
        });
    }

    if let LayerKind::Polygon { sides } = &layer.kind {
        let mut side_count = *sides;
        let r = ui.add(egui::DragValue::new(&mut side_count).prefix("Sides: ").range(3..=30));
        if should_snapshot(&r) {
            history.snapshot();
        }
        if r.changed() {
            if let Some(l) = history.mutate().active_page_mut().find_mut(id) {
                if let LayerKind::Polygon { sides } = &mut l.kind {
                    *sides = side_count;
                }
            }
        }
    }

    if let LayerKind::Arrow { start_cap, end_cap } = &layer.kind {
        let (mut start, mut end) = (*start_cap, *end_cap);
        ui.horizontal(|ui| {
            ui.label("Start:");
            let changed_start = arrow_cap_combo(ui, ("arrow-start-cap", id), &mut start);
            ui.label("End:");
            let changed_end = arrow_cap_combo(ui, ("arrow-end-cap", id), &mut end);
            if changed_start || changed_end {
                history.snapshot();
                if let Some(l) = history.mutate().active_page_mut().find_mut(id) {
                    if let LayerKind::Arrow { start_cap, end_cap } = &mut l.kind {
                        *start_cap = start;
                        *end_cap = end;
                    }
                }
            }
        });
    }

    if let LayerKind::Text {
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
        style_id,
        runs,
    } = &layer.kind
    {
        let ctx = ui.ctx().clone();
        let base_style = crate::model::text_runs::RunStyle {
            font: font.clone(),
            font_size: *font_size,
            color: layer.style.fill.as_ref().map(crate::model::Paint::to_color32),
            bold: *bold,
            italic: *italic,
            underline: *underline,
            strikethrough: *strikethrough,
        };
        ui.add_space(8.0);
        ui.separator();
        ui.label("Text");
        if canvas.is_editing_text(id) {
            ui.weak(if text_selection.is_some() {
                "Editing on canvas — formatting controls below apply to the selection."
            } else {
                "Editing on canvas — click elsewhere or press Esc to finish."
            });
        }

        let mut buf = content.clone();
        let content_resp = ui.add(egui::TextEdit::multiline(&mut buf).desired_rows(3));
        if should_snapshot(&content_resp) {
            history.snapshot();
        }
        if content_resp.changed() {
            if let Some(l) = history.mutate().active_page_mut().find_mut(id) {
                if let LayerKind::Text { content, .. } = &mut l.kind {
                    *content = buf;
                }
                apply_text_auto_resize(&ctx, l);
            }
        }

        if let Some(sel) = &text_selection {
            let current = selected_or_base(runs, sel, base_style.font_size, |s| s.font_size);
            let mut size = current.unwrap_or(base_style.font_size);
            let size_resp = ui.add(
                egui::DragValue::new(&mut size)
                    .prefix(if current.is_none() { "Font size (Mixed): " } else { "Font size: " })
                    .range(1.0..=500.0),
            );
            if should_snapshot(&size_resp) {
                history.snapshot();
            }
            if size_resp.changed() {
                if let Some(l) = history.mutate().active_page_mut().find_mut(id) {
                    if let LayerKind::Text { content, runs, .. } = &mut l.kind {
                        crate::model::text_runs::apply(content, runs, &base_style, sel.clone(), |s| s.font_size = size);
                    }
                    apply_text_auto_resize(&ctx, l);
                }
            }
        } else {
            let mut size = *font_size;
            let size_resp = ui.add(
                egui::DragValue::new(&mut size)
                    .prefix("Font size: ")
                    .range(1.0..=500.0),
            );
            if should_snapshot(&size_resp) {
                history.snapshot();
            }
            if size_resp.changed() {
                if let Some(l) = history.mutate().active_page_mut().find_mut(id) {
                    if let LayerKind::Text { font_size, .. } = &mut l.kind {
                        *font_size = size;
                    }
                    apply_text_auto_resize(&ctx, l);
                }
            }
        }

        ui.horizontal(|ui| {
            ui.label("Font:");
            let current_font: Option<TextFont> = match &text_selection {
                Some(sel) => selected_or_base(runs, sel, font.clone(), |s| s.font.clone()),
                None => Some(font.clone()),
            };
            let selected_text =
                current_font.as_ref().map_or_else(|| "Mixed".to_string(), |f| font_label(f).to_string());
            let preview_source: String = if content.trim().is_empty() {
                "The quick brown fox".to_string()
            } else {
                content.chars().take(60).collect()
            };
            egui::ComboBox::from_id_salt(("font-picker", id))
                .selected_text(selected_text)
                .width(180.0)
                .show_ui(ui, |ui| {
                    // Each row shows its own label plus `preview_source` set
                    // in that candidate's actual font, so browsing the list
                    // previews every option in place rather than needing a
                    // separate preview area that scrolls out of view.
                    let preview_row = |ui: &mut egui::Ui, selected: bool, label: &str, choice: &TextFont| {
                        let preview = egui::RichText::new(&preview_source).font(crate::text_layout::font_id(&ctx, choice, 16.0));
                        ui.selectable_label(selected, (label, preview))
                    };

                    let mut pick = |choice: TextFont| {
                        if current_font.as_ref() == Some(&choice) {
                            return;
                        }
                        // For a `System` font egui hasn't bound yet, this
                        // can't come back true until egui's *next* pass
                        // (see `CLAUDE.md`'s "Fonts" section) — so the
                        // auto-resize below measures against the
                        // `Proportional` fallback. Queue a retry so
                        // `frame.size` gets corrected once the real font
                        // lands, instead of staying pinned to the wrong
                        // metrics.
                        let already_bound = if let TextFont::System(name) = &choice {
                            let family = egui::FontFamily::Name(name.as_str().into());
                            ctx.fonts(|f| f.definitions().families.contains_key(&family))
                        } else {
                            true
                        };
                        history.snapshot();
                        if let Some(l) = history.mutate().active_page_mut().find_mut(id) {
                            if let Some(sel) = &text_selection {
                                if let LayerKind::Text { content, runs, .. } = &mut l.kind {
                                    let choice = choice.clone();
                                    crate::model::text_runs::apply(content, runs, &base_style, sel.clone(), |s| {
                                        s.font = choice.clone()
                                    });
                                }
                            } else if let LayerKind::Text { font, .. } = &mut l.kind {
                                *font = choice.clone();
                            }
                            apply_text_auto_resize(&ctx, l);
                        }
                        if let TextFont::System(name) = &choice {
                            crate::system_fonts::ensure_registered(&ctx, name);
                            if !already_bound {
                                canvas.queue_font_resize_retry(id, name.clone());
                            }
                        }
                    };

                    for (label, choice) in [
                        ("Sans", TextFont::Proportional),
                        ("Mono", TextFont::Monospace),
                        ("Serif", TextFont::Serif),
                        ("Display", TextFont::Display),
                        ("Handwriting", TextFont::Handwriting),
                    ] {
                        let selected = current_font.as_ref() == Some(&choice);
                        if preview_row(ui, selected, label, &choice).clicked() {
                            pick(choice);
                        }
                    }

                    ui.separator();
                    ui.weak("System fonts");
                    let filter_id = egui::Id::new(("font-filter", id));
                    let mut filter = ui.ctx().data(|d| d.get_temp::<String>(filter_id)).unwrap_or_default();
                    let filter_resp =
                        ui.add(egui::TextEdit::singleline(&mut filter).hint_text("Search\u{2026}"));
                    if filter_resp.changed() {
                        ui.ctx().data_mut(|d| d.insert_temp(filter_id, filter.clone()));
                    }
                    let filter_lower = filter.to_lowercase();
                    let matching_names: Vec<&String> = crate::system_fonts::family_names()
                        .iter()
                        .filter(|name| filter_lower.is_empty() || name.to_lowercase().contains(&filter_lower))
                        .collect();
                    // Sized for a 16px preview line plus button padding.
                    // `show_rows` only builds rows actually scrolled into
                    // view, so only those candidates' `System` faces get
                    // resolved/registered with egui — loading every
                    // installed family (some multi-MB) the moment the
                    // dropdown opens would be a real memory/latency hit.
                    let row_height = 28.0;
                    egui::ScrollArea::vertical().max_height(220.0).show_rows(
                        ui,
                        row_height,
                        matching_names.len(),
                        |ui, row_range| {
                            for i in row_range {
                                let name = matching_names[i];
                                let choice = TextFont::System(name.clone());
                                let selected = current_font.as_ref() == Some(&choice);
                                if preview_row(ui, selected, name, &choice).clicked() {
                                    pick(choice);
                                }
                            }
                        },
                    );
                });
        });

        ui.horizontal(|ui| {
            ui.label("Style:");
            // With an active text selection, a click applies to just that
            // range (materializing `runs` lazily via `text_runs::apply`)
            // instead of the whole layer; a `Mixed` selection (differing
            // values across the range) or an unset value both count as
            // "off" for display, and either way one click sets the whole
            // selection to `true` (only a uniformly-`true` selection
            // clears back to `false`) — the standard "any unset → set
            // all" convention.
            let mut toggle = |label: &str,
                               base_active: bool,
                               run_field: fn(&crate::model::text_runs::RunStyle) -> bool,
                               run_apply: fn(&mut crate::model::text_runs::RunStyle, bool),
                               layer_apply: fn(&mut LayerKind, bool)| {
                let current: Option<bool> = match &text_selection {
                    Some(sel) => selected_or_base(runs, sel, base_active, run_field),
                    None => Some(base_active),
                };
                let active = current == Some(true);
                if ui.selectable_label(active, label).clicked() {
                    history.snapshot();
                    let new_value = current != Some(true);
                    if let Some(l) = history.mutate().active_page_mut().find_mut(id) {
                        if let Some(sel) = &text_selection {
                            if let LayerKind::Text { content, runs, .. } = &mut l.kind {
                                crate::model::text_runs::apply(content, runs, &base_style, sel.clone(), |s| {
                                    run_apply(s, new_value)
                                });
                            }
                        } else {
                            layer_apply(&mut l.kind, new_value);
                        }
                        apply_text_auto_resize(&ctx, l);
                    }
                }
            };
            toggle(
                "Bold",
                *bold,
                |s| s.bold,
                |s, v| s.bold = v,
                |k, v| {
                    if let LayerKind::Text { bold, .. } = k {
                        *bold = v;
                    }
                },
            );
            toggle(
                "Italic",
                *italic,
                |s| s.italic,
                |s, v| s.italic = v,
                |k, v| {
                    if let LayerKind::Text { italic, .. } = k {
                        *italic = v;
                    }
                },
            );
            toggle(
                "Underline",
                *underline,
                |s| s.underline,
                |s, v| s.underline = v,
                |k, v| {
                    if let LayerKind::Text { underline, .. } = k {
                        *underline = v;
                    }
                },
            );
            toggle(
                "Strikethrough",
                *strikethrough,
                |s| s.strikethrough,
                |s, v| s.strikethrough = v,
                |k, v| {
                    if let LayerKind::Text { strikethrough, .. } = k {
                        *strikethrough = v;
                    }
                },
            );
        });

        ui.horizontal(|ui| {
            ui.label("Align:");
            for (label, choice) in [
                ("Left", TextAlign::Left),
                ("Center", TextAlign::Center),
                ("Right", TextAlign::Right),
                ("Justify", TextAlign::Justify),
            ] {
                if ui.selectable_label(*align == choice, label).clicked() && *align != choice {
                    history.snapshot();
                    if let Some(l) = history.mutate().active_page_mut().find_mut(id) {
                        if let LayerKind::Text { align, .. } = &mut l.kind {
                            *align = choice;
                        }
                        apply_text_auto_resize(&ctx, l);
                    }
                }
            }
        });

        ui.horizontal(|ui| {
            ui.label("Vertical:");
            for (label, choice) in [
                ("Top", VerticalAlign::Top),
                ("Middle", VerticalAlign::Middle),
                ("Bottom", VerticalAlign::Bottom),
            ] {
                if ui.selectable_label(*vertical_align == choice, label).clicked() && *vertical_align != choice {
                    history.snapshot();
                    if let Some(l) = history.mutate().active_page_mut().find_mut(id) {
                        if let LayerKind::Text { vertical_align, .. } = &mut l.kind {
                            *vertical_align = choice;
                        }
                    }
                }
            }
        });

        ui.horizontal(|ui| {
            ui.label("Resize:");
            for (label, choice) in [
                ("Auto", TextResize::Auto),
                ("Auto Height", TextResize::AutoHeight),
                ("Fixed", TextResize::Fixed),
            ] {
                if ui.selectable_label(*resize == choice, label).clicked() && *resize != choice {
                    history.snapshot();
                    if let Some(l) = history.mutate().active_page_mut().find_mut(id) {
                        if let LayerKind::Text { resize, .. } = &mut l.kind {
                            *resize = choice;
                        }
                        apply_text_auto_resize(&ctx, l);
                    }
                }
            }
        });

        ui.horizontal(|ui| {
            let mut auto_line_height = line_height.is_none();
            let auto_resp = ui.checkbox(&mut auto_line_height, "Auto line height");
            if auto_resp.changed() {
                history.snapshot();
                if let Some(l) = history.mutate().active_page_mut().find_mut(id) {
                    if let LayerKind::Text { line_height, font_size, .. } = &mut l.kind {
                        *line_height = if auto_line_height { None } else { Some(*font_size * 1.2) };
                    }
                    apply_text_auto_resize(&ctx, l);
                }
            }
            if !auto_line_height {
                let mut value = line_height.unwrap_or(*font_size * 1.2);
                let resp = ui.add(egui::DragValue::new(&mut value).prefix("Line height: ").range(1.0..=1000.0));
                if should_snapshot(&resp) {
                    history.snapshot();
                }
                if resp.changed() {
                    if let Some(l) = history.mutate().active_page_mut().find_mut(id) {
                        if let LayerKind::Text { line_height, .. } = &mut l.kind {
                            *line_height = Some(value);
                        }
                        apply_text_auto_resize(&ctx, l);
                    }
                }
            }
        });

        ui.horizontal(|ui| {
            let mut spacing = *letter_spacing;
            let resp = ui.add(egui::DragValue::new(&mut spacing).prefix("Letter spacing: ").range(-50.0..=200.0));
            if should_snapshot(&resp) {
                history.snapshot();
            }
            if resp.changed() {
                if let Some(l) = history.mutate().active_page_mut().find_mut(id) {
                    if let LayerKind::Text { letter_spacing, .. } = &mut l.kind {
                        *letter_spacing = spacing;
                    }
                    apply_text_auto_resize(&ctx, l);
                }
            }

            let mut para_spacing = *paragraph_spacing;
            let resp = ui.add(
                egui::DragValue::new(&mut para_spacing)
                    .prefix("Paragraph spacing: ")
                    .range(0.0..=1000.0),
            );
            if should_snapshot(&resp) {
                history.snapshot();
            }
            if resp.changed() {
                if let Some(l) = history.mutate().active_page_mut().find_mut(id) {
                    if let LayerKind::Text { paragraph_spacing, .. } = &mut l.kind {
                        *paragraph_spacing = para_spacing;
                    }
                    apply_text_auto_resize(&ctx, l);
                }
            }
        });

        ui.horizontal(|ui| {
            ui.label("Transform:");
            for (label, choice) in [
                ("Normal", TextTransform::None),
                ("UPPER", TextTransform::Uppercase),
                ("lower", TextTransform::Lowercase),
                ("Title", TextTransform::Titlecase),
            ] {
                if ui.selectable_label(*transform == choice, label).clicked() && *transform != choice {
                    history.snapshot();
                    if let Some(l) = history.mutate().active_page_mut().find_mut(id) {
                        if let LayerKind::Text { transform, .. } = &mut l.kind {
                            *transform = choice;
                        }
                        apply_text_auto_resize(&ctx, l);
                    }
                }
            }
        });

        ui.horizontal(|ui| {
            ui.label("List:");
            for (label, choice) in [
                ("None", ListType::None),
                ("Bullet", ListType::Bullet),
                ("Numbered", ListType::Numbered),
            ] {
                if ui.selectable_label(*list == choice, label).clicked() && *list != choice {
                    history.snapshot();
                    if let Some(l) = history.mutate().active_page_mut().find_mut(id) {
                        if let LayerKind::Text { list, .. } = &mut l.kind {
                            *list = choice;
                        }
                        apply_text_auto_resize(&ctx, l);
                    }
                }
            }
            if *list == ListType::Numbered {
                let mut start = *list_start;
                let resp = ui.add(egui::DragValue::new(&mut start).prefix("Start: ").range(0..=100000));
                if should_snapshot(&resp) {
                    history.snapshot();
                }
                if resp.changed() {
                    if let Some(l) = history.mutate().active_page_mut().find_mut(id) {
                        if let LayerKind::Text { list_start, .. } = &mut l.kind {
                            *list_start = start;
                        }
                        apply_text_auto_resize(&ctx, l);
                    }
                }
            }
        });

        text_style_ui(ui, history, id, *style_id);
    }

    if let LayerKind::Image { width, height, color_adjust, .. } = &layer.kind {
        ui.add_space(8.0);
        ui.separator();
        ui.label(egui::RichText::new("Image").strong());
        ui.label(format!("{width} × {height} px"));

        ui.add_space(4.0);
        ui.horizontal(|ui| {
            if ui.button("Replace Image...").clicked() {
                replace_image(history, id);
            }
            if ui.button("Reset to Original Size").clicked() {
                history.snapshot();
                if let Some(l) = history.mutate().active_page_mut().find_mut(id) {
                    if let LayerKind::Image { width, height, .. } = &l.kind {
                        l.frame.size = egui::Vec2::new(*width as f32, *height as f32);
                    }
                }
            }
        });

        ui.add_space(4.0);
        if canvas.image_edit_active_for(id) {
            ui.separator();
            ui.label("Edit Image — drag to select, click for Magic Wand");
            ui.weak("Shift: add to selection · Option: subtract");

            let mut tolerance = canvas.image_edit_tolerance();
            let tol_resp = ui.add(egui::Slider::new(&mut tolerance, 0.0..=100.0).text("Wand tolerance"));
            if tol_resp.changed() {
                canvas.set_image_edit_tolerance(tolerance);
            }

            let has_selection = canvas.image_edit_has_selection();
            ui.horizontal(|ui| {
                if ui.add_enabled(has_selection, egui::Button::new("Crop to Selection")).clicked() {
                    canvas.apply_image_edit_crop(history);
                }
                if ui.add_enabled(has_selection, egui::Button::new("Delete")).clicked() {
                    canvas.apply_image_edit_delete(history);
                }
            });
            ui.horizontal(|ui| {
                let mut fill_color = canvas.image_edit_fill_color();
                if color_picker::edit(ui, &mut fill_color, &history.get().palette).changed() {
                    canvas.set_image_edit_fill_color(fill_color);
                }
                if ui.add_enabled(has_selection, egui::Button::new("Fill")).clicked() {
                    canvas.apply_image_edit_fill(history, fill_color);
                }
                if ui.add_enabled(has_selection, egui::Button::new("Clear Selection")).clicked() {
                    canvas.clear_image_edit_selection();
                }
            });
            if ui.button("Done Editing").clicked() {
                canvas.end_image_edit();
            }
        } else if ui.button("Edit Image...").clicked() {
            canvas.begin_image_edit(history, id);
        }

        ui.add_space(4.0);
        if ui.button("Remove Background").clicked() {
            history.snapshot();
            if let Some(l) = history.mutate().active_page_mut().find_mut(id) {
                if let LayerKind::Image { encoded, .. } = &l.kind {
                    if let Some(decoded) = crate::image_ops::decode(encoded) {
                        let out = crate::image_ops::remove_background(&decoded, BACKGROUND_REMOVAL_TOLERANCE);
                        if let LayerKind::Image { encoded, version, .. } = &mut l.kind {
                            *encoded = crate::image_ops::encode_png(&out);
                            *version = uuid::Uuid::new_v4();
                        }
                    }
                }
            }
        }
        if ui.button("Trim Transparent Pixels").clicked() {
            history.snapshot();
            if let Some(l) = history.mutate().active_page_mut().find_mut(id) {
                if let LayerKind::Image { encoded, .. } = &l.kind {
                    if let Some(decoded) = crate::image_ops::decode(encoded) {
                        if let Some((trimmed, ox, oy)) = crate::image_ops::trim_transparent(&decoded) {
                            crate::image_ops::apply_cropped_image(l, &trimmed, ox, oy);
                        }
                    }
                }
            }
        }
        if ui.button("Minimize File Size").on_hover_text(
            "Resamples the image down to its current on-screen pixel size, discarding excess resolution."
        ).clicked() {
            history.snapshot();
            if let Some(l) = history.mutate().active_page_mut().find_mut(id) {
                crate::image_ops::minimize_image_file_size(l);
            }
        }

        ui.add_space(8.0);
        ui.separator();
        ui.label("Color Adjust");
        let mut adjust = *color_adjust;
        let hue_r = ui.add(egui::Slider::new(&mut adjust.hue, -180.0..=180.0).text("Hue"));
        let sat_r = ui.add(egui::Slider::new(&mut adjust.saturation, -1.0..=1.0).text("Saturation"));
        let bri_r = ui.add(egui::Slider::new(&mut adjust.brightness, -1.0..=1.0).text("Brightness"));
        let con_r = ui.add(egui::Slider::new(&mut adjust.contrast, -1.0..=1.0).text("Contrast"));
        if should_snapshot(&hue_r) || should_snapshot(&sat_r) || should_snapshot(&bri_r) || should_snapshot(&con_r) {
            history.snapshot();
        }
        if hue_r.changed() || sat_r.changed() || bri_r.changed() || con_r.changed() {
            if let Some(l) = history.mutate().active_page_mut().find_mut(id) {
                if let LayerKind::Image { color_adjust, .. } = &mut l.kind {
                    *color_adjust = adjust;
                }
            }
        }
        if ui.button("Reset Color Adjust").clicked() {
            history.snapshot();
            if let Some(l) = history.mutate().active_page_mut().find_mut(id) {
                if let LayerKind::Image { color_adjust, .. } = &mut l.kind {
                    *color_adjust = ColorAdjust::default();
                }
            }
        }
    }

    ui.add_space(8.0);
    let mut opacity_pct = layer.opacity * 100.0;
    let opacity_resp = ui.add(
        egui::Slider::new(&mut opacity_pct, 0.0..=100.0)
            .suffix("%")
            .text("Opacity"),
    );
    if should_snapshot(&opacity_resp) {
        history.snapshot();
    }
    if opacity_resp.changed() {
        if let Some(l) = history.mutate().active_page_mut().find_mut(id) {
            l.opacity = opacity_pct / 100.0;
        }
    }

    layer_style_ui(ui, history, id, layer.style_id);

    ui.add_space(8.0);
    ui.separator();
    ui.label("Fill");
    // A `Text` layer with an active canvas selection gets a per-run color
    // picker instead of the usual whole-layer Enabled+color controls —
    // matches every other dual-mode Text control (see `text_selection`'s
    // doc comment above).
    let text_run_fill = match (&layer.kind, &text_selection) {
        (LayerKind::Text { font, font_size, bold, italic, underline, strikethrough, runs, .. }, Some(sel)) => {
            let base_style = crate::model::text_runs::RunStyle {
                font: font.clone(),
                font_size: *font_size,
                color: layer.style.fill.as_ref().map(crate::model::Paint::to_color32),
                bold: *bold,
                italic: *italic,
                underline: *underline,
                strikethrough: *strikethrough,
            };
            Some((runs, sel.clone(), base_style))
        }
        _ => None,
    };
    if let Some((runs, sel, base_style)) = text_run_fill {
        let base_fill = layer.style.fill.as_ref().map(Paint::to_color32).unwrap_or(egui::Color32::BLACK);
        let current = selected_or_base(runs, &sel, base_fill, |s| s.color.unwrap_or(base_fill));
        let mut color = current.unwrap_or(base_fill);
        ui.horizontal(|ui| {
            if current.is_none() {
                ui.weak("Mixed");
            }
            let cr = color_picker::edit(ui, &mut color, &history.get().palette);
            if should_snapshot(&cr) {
                history.snapshot();
            }
            if cr.changed() {
                if let Some(l) = history.mutate().active_page_mut().find_mut(id) {
                    if let LayerKind::Text { content, runs, .. } = &mut l.kind {
                        crate::model::text_runs::apply(content, runs, &base_style, sel.clone(), |s| {
                            s.color = Some(color)
                        });
                    }
                }
            }
        });
    } else {
        let mut has_fill = layer.style.fill.is_some();
        let fill_toggle = ui.checkbox(&mut has_fill, "Enabled");
        if fill_toggle.changed() {
            history.snapshot();
            if let Some(l) = history.mutate().active_page_mut().find_mut(id) {
                l.style.fill = if has_fill {
                    Some(Paint::Solid(egui::Color32::from_rgb(216, 216, 216)))
                } else {
                    None
                };
            }
        }
        if let Some(current) = layer.style.fill.clone() {
            // Gradients/noise don't apply to glyph color (see
            // `Paint::to_color32`'s doc comment) — Text layers stay
            // solid-only here.
            let allow_gradient = !matches!(layer.kind, LayerKind::Text { .. });
            paint_editor(ui, history, id, "fill", &current, allow_gradient, allow_gradient, |l, p| l.style.fill = Some(p));

            let mut fill_opacity_pct = layer.style.fill_opacity * 100.0;
            let fo = ui.add(
                egui::Slider::new(&mut fill_opacity_pct, 0.0..=100.0)
                    .suffix("%")
                    .text("Fill opacity"),
            );
            if should_snapshot(&fo) {
                history.snapshot();
            }
            if fo.changed() {
                if let Some(l) = history.mutate().active_page_mut().find_mut(id) {
                    l.style.fill_opacity = fill_opacity_pct / 100.0;
                }
            }
        }
    }

    ui.add_space(4.0);
    ui.label("Stroke");
    let mut has_stroke = layer.style.stroke.is_some();
    let stroke_toggle = ui.checkbox(&mut has_stroke, "Enabled");
    if stroke_toggle.changed() {
        history.snapshot();
        if let Some(l) = history.mutate().active_page_mut().find_mut(id) {
            l.style.stroke = if has_stroke { Some(Stroke::default()) } else { None };
        }
    }
    if let Some(current_stroke) = layer.style.stroke.clone() {
        // Noise is fill-only (see `paint_editor`'s doc comment) — strokes
        // keep gradients but never offer the Noise option.
        paint_editor(ui, history, id, "stroke", &current_stroke.paint, true, false, |l, p| {
            if let Some(s) = l.style.stroke.as_mut() {
                s.paint = p;
            }
        });
        let mut width = current_stroke.width;
        let wr = ui.add(egui::DragValue::new(&mut width).prefix("Width: ").range(0.0..=100.0));
        if should_snapshot(&wr) {
            history.snapshot();
        }
        if wr.changed() {
            if let Some(l) = history.mutate().active_page_mut().find_mut(id) {
                if let Some(s) = l.style.stroke.as_mut() {
                    s.width = width;
                }
            }
        }
        let mut stroke_opacity_pct = layer.style.stroke_opacity * 100.0;
        let so = ui.add(
            egui::Slider::new(&mut stroke_opacity_pct, 0.0..=100.0)
                .suffix("%")
                .text("Stroke opacity"),
        );
        if should_snapshot(&so) {
            history.snapshot();
        }
        if so.changed() {
            if let Some(l) = history.mutate().active_page_mut().find_mut(id) {
                l.style.stroke_opacity = stroke_opacity_pct / 100.0;
            }
        }
    }

    shadow_section(ui, history, id, "Drop Shadow", &layer.style.shadows, |l, shadows| l.style.shadows = shadows);
    shadow_section(ui, history, id, "Inner Shadow", &layer.style.inner_shadows, |l, shadows| l.style.inner_shadows = shadows);

    ui.add_space(8.0);
    ui.separator();
    if icons::duplicate_button(ui).on_hover_text("Duplicate (Cmd+D)").clicked() {
        return Some(InspectorAction::Duplicate);
    }

    if matches!(layer.kind, LayerKind::Group { .. }) {
        if ui.button("Ungroup (Cmd+Shift+G)").clicked() {
            return Some(InspectorAction::Ungroup);
        }
    }

    // Only offered for kinds `transform_ops::flatten_to_compound_path`
    // actually supports (fillable shapes, plus an already-closed Path or
    // CompoundPath) — matches `boolean_ops.rs::flatten_layer`'s own
    // recognized-kinds set.
    let flattenable = matches!(
        layer.kind,
        LayerKind::Rectangle { .. }
            | LayerKind::Oval
            | LayerKind::Star { .. }
            | LayerKind::Polygon { .. }
            | LayerKind::CompoundPath { .. }
    ) || matches!(&layer.kind, LayerKind::Path { closed, points } if *closed && points.len() >= 3);
    if flattenable && icons::flatten_button(ui).on_hover_text("Flatten").clicked() {
        return Some(InspectorAction::Flatten);
    }

    None
}
