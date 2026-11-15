//! The fill/stroke paint editor: solid, gradient, noise, halftone and
//! pattern paints, each with its own controls.

use egui::Ui;
use crate::history::History;
use crate::model::{Gradient, GradientKind, GradientStop, HalftoneFill, Layer, LayerId, NoiseFill, Paint};
use crate::ui::color_picker;

use super::should_snapshot;

/// Reads and decodes the image at `path`, re-encoding it to PNG bytes —
/// the same normalization `image_ops::build_image_grid` applies when
/// inserting an `Image` layer, so a `Paint::Pattern`'s `encoded` follows
/// the same "always PNG regardless of source format" convention as
/// `LayerKind::Image::encoded`. `tile_width` defaults to the image's own
/// native width, capped so a large photo doesn't start out tiled
/// absurdly large.
pub(super) fn pattern_fill_from_path(path: &std::path::Path) -> Option<crate::model::PatternFill> {
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
#[allow(clippy::too_many_arguments)]
pub(super) fn paint_editor(
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
                if let Some(path) = crate::io::open_pattern_image_dialog()
                    && let Some(pattern) = pattern_fill_from_path(&path) {
                        history.snapshot();
                        if let Some(l) = history.mutate().active_page_mut().find_mut(id) {
                            apply(l, Paint::Pattern(pattern));
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
            if cr.changed()
                && let Some(l) = history.mutate().active_page_mut().find_mut(id) {
                    apply(l, Paint::Solid(color));
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
            if changed
                && let Some(l) = history.mutate().active_page_mut().find_mut(id) {
                    apply(l, Paint::Noise(n));
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
            if changed
                && let Some(l) = history.mutate().active_page_mut().find_mut(id) {
                    apply(l, Paint::Halftone(h));
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

            if ui.small_button("Replace Image…").clicked()
                && let Some(path) = crate::io::open_pattern_image_dialog()
                    && let Some(new_pattern) = pattern_fill_from_path(&path) {
                        // Keeps the current `tile_width` rather than
                        // resetting it to the new image's native width —
                        // replacing the tile shouldn't undo a scale the
                        // user already dialed in.
                        p.encoded = new_pattern.encoded;
                        changed = true;
                        snapshot_now = true;
                    }

            if snapshot_now {
                history.snapshot();
            }
            if changed
                && let Some(l) = history.mutate().active_page_mut().find_mut(id) {
                    apply(l, Paint::Pattern(p));
                }
        }
    }
}
