//! Converts a `Text` layer's glyphs into vector outlines ("Convert
//! to Outlines"), wired up in `app.rs` (Edit menu + `⌥⌘O`).
//!
//! Independent of both `canvas.rs`'s `egui::Galley`-based render and
//! `export.rs`'s `ab_glyph`-rasterizer render (per this codebase's
//! canvas/export-are-separate-implementations convention — see
//! `CLAUDE.md`), except for the pure layout math it reuses from `export.rs`
//! (`wrap_line`) and `text_layout.rs` (`display_string`), which have no
//! rendering-backend dependency.
use ab_glyph::{Font, FontRef, GlyphId, OutlineCurve, Point, ScaleFont};
use egui::Pos2;

use crate::model::text_runs::{RunStyle, TextRun};
use crate::model::{Layer, LayerKind, ListType, PathPolygon, TextAlign, TextResize, TextTransform, VerticalAlign};

/// Number of line segments each quadratic/cubic outline curve is flattened
/// into — `PathPolygon` rings are straight-line-only (see its doc comment
/// in `model/layer.rs`), same as every other boolean-op/compound-path
/// result in this app.
const CURVE_STEPS: usize = 8;

/// Approximates the faux-italic shear `canvas.rs`/`export.rs` apply when
/// drawing, so a converted-to-outlines italic layer keeps its slant.
const ITALIC_SHEAR: f32 = 0.22;

/// Builds one `PathPolygon` per disjoint filled region across every glyph
/// in `layer`'s content (e.g. "o" is one region with a hole; "i" is two
/// disjoint regions — body and dot), positioned exactly as the live/export
/// render lays the text out (word-wrap, alignment, vertical align, line/
/// letter/paragraph spacing, the transform/list display prefix). Points are
/// relative to `layer.frame.pos`, matching `PathPolygon`'s usual convention
/// — the caller typically replaces `layer.kind` with the result in place,
/// keeping id/frame/style (see `app.rs::convert_selection_to_outlines`).
///
/// `underline`/`strikethrough` (decorations, not glyph shapes) are never
/// reproduced. Uniform-style (`runs` empty) `bold` isn't either — it's a
/// raster-only faux-weight trick with no vector equivalent — but a rich
/// (`runs` non-empty) layer's bold *runs* are, since those draw from a
/// real bold-weight face's own outlines (see `convert_to_outlines_rich`).
/// `TextAlign::Justify` is treated as `Left`. Returns `None` for empty
/// content or if the bundled font fails to load (shouldn't happen).
pub fn convert_to_outlines(layer: &Layer) -> Option<Vec<PathPolygon>> {
    let LayerKind::Text {
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
        transform,
        list,
        list_start,
        runs,
        ..
    } = &layer.kind
    else {
        return None;
    };
    if content.is_empty() {
        return None;
    }
    if !runs.is_empty() {
        let base = RunStyle {
            font: font.clone(),
            font_size: *font_size,
            color: None,
            bold: *bold,
            italic: *italic,
            underline: false,
            strikethrough: false,
        };
        return convert_to_outlines_rich(
            layer, content, runs, &base, *align, *vertical_align, *resize, *line_height, *letter_spacing, *paragraph_spacing,
            *transform, *list, *list_start,
        );
    }

    let (font_bytes, face_index) = crate::fonts::ab_glyph_bytes(font);
    let face = FontRef::try_from_slice_and_index(&font_bytes, face_index).ok()?;
    let scale = ab_glyph::PxScale::from(*font_size);
    let scaled = face.as_scaled(scale);
    let scale_factor = scaled.scale_factor();
    let line_step = line_height.unwrap_or(scaled.height() + scaled.line_gap());
    let letter_spacing = *letter_spacing;

    let measure = |s: &str| -> f32 {
        let mut width = 0.0;
        let mut prev: Option<GlyphId> = None;
        for c in s.chars() {
            let id = scaled.glyph_id(c);
            if let Some(p) = prev {
                width += scaled.kern(p, id);
            }
            width += scaled.h_advance(id) + letter_spacing;
            prev = Some(id);
        }
        width
    };
    let space_width = scaled.h_advance(scaled.glyph_id(' ')) + letter_spacing;

    let display = crate::text_layout::display_string(content, *transform, *list, *list_start);
    let size = layer.frame.size.abs();
    let max_width = match resize {
        TextResize::Auto => f32::INFINITY,
        TextResize::AutoHeight | TextResize::Fixed => size.x,
    };

    struct VLine {
        text: String,
        width: f32,
    }
    let mut lines: Vec<VLine> = Vec::new();
    let mut is_paragraph_start: Vec<bool> = Vec::new();
    for paragraph in display.split('\n') {
        let wrapped = crate::export::wrap_line(paragraph, &measure, space_width, max_width);
        for (i, text) in wrapped.into_iter().enumerate() {
            let width = measure(&text);
            is_paragraph_start.push(i == 0);
            lines.push(VLine { text, width });
        }
    }

    let mut y_offsets = Vec::with_capacity(lines.len());
    let mut cursor = 0.0f32;
    for (i, &is_start) in is_paragraph_start.iter().enumerate() {
        if i > 0 {
            cursor += line_step;
            if is_start {
                cursor += *paragraph_spacing;
            }
        }
        y_offsets.push(cursor);
    }
    let total_height = y_offsets.last().copied().unwrap_or(0.0) + line_step;

    let start_y = match vertical_align {
        VerticalAlign::Top => 0.0,
        VerticalAlign::Middle => (size.y - total_height) / 2.0,
        VerticalAlign::Bottom => size.y - total_height,
    };

    let mut polygons = Vec::new();
    for (line, &y_off) in lines.iter().zip(&y_offsets) {
        let baseline_y = start_y + y_off + scaled.ascent();
        let start_x = match align {
            TextAlign::Left | TextAlign::Justify => 0.0,
            TextAlign::Center => (size.x - line.width) / 2.0,
            TextAlign::Right => size.x - line.width,
        };

        let mut cursor_x = start_x;
        let mut prev: Option<GlyphId> = None;
        for c in line.text.chars() {
            let id = scaled.glyph_id(c);
            if let Some(p) = prev {
                cursor_x += scaled.kern(p, id);
            }
            if let Some(outline) = face.outline(id) {
                let contours = glyph_contours(&outline.curves);
                let placed: Vec<Vec<Pos2>> = contours
                    .into_iter()
                    .map(|contour| {
                        contour
                            .into_iter()
                            .map(|p| {
                                let x = p.x * scale_factor.horizontal + cursor_x;
                                let y = p.y * -scale_factor.vertical + baseline_y;
                                let x = if *italic { x + (baseline_y - y) * ITALIC_SHEAR } else { x };
                                Pos2::new(x, y)
                            })
                            .collect()
                    })
                    .filter(|c: &Vec<Pos2>| c.len() >= 3)
                    .collect();
                polygons.extend(group_contours_into_polygons(placed));
            }
            cursor_x += scaled.h_advance(id) + letter_spacing;
            prev = Some(id);
        }
    }

    Some(polygons)
}

/// Rich-text counterpart to `convert_to_outlines`'s main loop, used when
/// `runs` is non-empty — see `LayerKind::Text::runs`'s doc comment. Mirrors
/// `export.rs`'s `draw_text_rich` (per-run face/scale resolution, char-
/// driven `wrap_line_rich`/tagging via `text_layout::tagged_lines`, same-
/// face-only kerning) but produces `PathPolygon`s from each glyph's own
/// outline instead of rasterizing. Bold runs use `fonts::
/// ab_glyph_bytes_bold`'s real bold-weight face directly — unlike the
/// raster paths, vector output has no faux-bold approximation to fall back
/// to, so this reproduces bold where the uniform (`runs` empty) path
/// can't. Still doesn't reproduce underline/strikethrough (decorations,
/// not glyph shapes) — same known gap as the uniform path.
#[allow(clippy::too_many_arguments)]
fn convert_to_outlines_rich(
    layer: &Layer,
    content: &str,
    runs: &[TextRun],
    base: &RunStyle,
    align: TextAlign,
    vertical_align: VerticalAlign,
    resize: TextResize,
    line_height: Option<f32>,
    letter_spacing: f32,
    paragraph_spacing: f32,
    transform: TextTransform,
    list: ListType,
    list_start: i32,
) -> Option<Vec<PathPolygon>> {
    let mut byte_bufs: Vec<(std::borrow::Cow<'static, [u8]>, u32)> = Vec::with_capacity(runs.len() + 1);
    byte_bufs.push(if base.bold {
        crate::fonts::ab_glyph_bytes_bold(&base.font)
    } else {
        crate::fonts::ab_glyph_bytes(&base.font)
    });
    for run in runs {
        byte_bufs.push(if run.style.bold {
            crate::fonts::ab_glyph_bytes_bold(&run.style.font)
        } else {
            crate::fonts::ab_glyph_bytes(&run.style.font)
        });
    }
    let base_face = FontRef::try_from_slice_and_index(&byte_bufs[0].0, byte_bufs[0].1).ok()?;
    let mut faces: Vec<FontRef> = vec![base_face];
    let mut face_for_run: Vec<usize> = Vec::with_capacity(runs.len());
    for (bytes, index) in &byte_bufs[1..] {
        match FontRef::try_from_slice_and_index(bytes, *index) {
            Ok(f) => {
                face_for_run.push(faces.len());
                faces.push(f);
            }
            Err(_) => face_for_run.push(0),
        }
    }
    let style_of = |idx: Option<usize>| -> &RunStyle {
        match idx {
            None => base,
            Some(i) => runs.get(i).map_or(base, |r| &r.style),
        }
    };
    let face_index_of = |idx: Option<usize>| -> usize {
        match idx {
            None => 0,
            Some(i) => face_for_run.get(i).copied().unwrap_or(0),
        }
    };
    let scale_of = |idx: Option<usize>| ab_glyph::PxScale::from(style_of(idx).font_size);

    let measure = |chars: &[(char, Option<usize>)]| -> f32 {
        let mut width = 0.0;
        let mut prev: Option<(GlyphId, usize)> = None;
        for &(c, idx) in chars {
            let face_idx = face_index_of(idx);
            let scaled = faces[face_idx].as_scaled(scale_of(idx));
            let id = scaled.glyph_id(c);
            if let Some((prev_id, prev_face_idx)) = prev {
                if prev_face_idx == face_idx {
                    width += scaled.kern(prev_id, id);
                }
            }
            width += scaled.h_advance(id) + letter_spacing;
            prev = Some((id, face_idx));
        }
        width
    };
    let base_scaled = faces[0].as_scaled(scale_of(None));
    let space_width = base_scaled.h_advance(base_scaled.glyph_id(' ')) + letter_spacing;

    let size = layer.frame.size.abs();
    let max_width = match resize {
        TextResize::Auto => f32::INFINITY,
        TextResize::AutoHeight | TextResize::Fixed => size.x,
    };

    struct VRow {
        chars: Vec<(char, Option<usize>)>,
        width: f32,
        height: f32,
        ascent: f32,
    }
    let mut rows: Vec<VRow> = Vec::new();
    let mut is_paragraph_start: Vec<bool> = Vec::new();
    for paragraph in crate::text_layout::tagged_lines(content, runs, transform, list, list_start) {
        let wrapped = crate::export::wrap_line_rich(&paragraph, &measure, space_width, max_width);
        for (i, chars) in wrapped.into_iter().enumerate() {
            let width = measure(&chars);
            let (height, ascent) = if let Some(fixed) = line_height {
                (fixed, base_scaled.ascent())
            } else if chars.is_empty() {
                (base_scaled.height() + base_scaled.line_gap(), base_scaled.ascent())
            } else {
                chars
                    .iter()
                    .map(|&(_, idx)| {
                        let scaled = faces[face_index_of(idx)].as_scaled(scale_of(idx));
                        (scaled.height() + scaled.line_gap(), scaled.ascent())
                    })
                    .fold((0.0f32, 0.0f32), |(h, a), (h2, a2)| (h.max(h2), a.max(a2)))
            };
            is_paragraph_start.push(i == 0);
            rows.push(VRow { chars, width, height, ascent });
        }
    }

    let mut y_offsets = Vec::with_capacity(rows.len());
    let mut cursor = 0.0f32;
    for (i, &is_start) in is_paragraph_start.iter().enumerate() {
        if i > 0 {
            cursor += rows[i - 1].height;
            if is_start {
                cursor += paragraph_spacing;
            }
        }
        y_offsets.push(cursor);
    }
    let total_height = y_offsets.last().copied().unwrap_or(0.0) + rows.last().map_or(0.0, |r| r.height);

    let start_y = match vertical_align {
        VerticalAlign::Top => 0.0,
        VerticalAlign::Middle => (size.y - total_height) / 2.0,
        VerticalAlign::Bottom => size.y - total_height,
    };

    let mut polygons = Vec::new();
    for (row, &y_off) in rows.iter().zip(&y_offsets) {
        let baseline_y = start_y + y_off + row.ascent;
        let start_x = match align {
            TextAlign::Left | TextAlign::Justify => 0.0,
            TextAlign::Center => (size.x - row.width) / 2.0,
            TextAlign::Right => size.x - row.width,
        };

        let mut cursor_x = start_x;
        let mut prev: Option<(GlyphId, usize)> = None;
        for &(c, idx) in &row.chars {
            let face_idx = face_index_of(idx);
            let style = style_of(idx);
            let scale = scale_of(idx);
            let face = &faces[face_idx];
            let scaled = face.as_scaled(scale);
            let scale_factor = scaled.scale_factor();
            let id = scaled.glyph_id(c);
            if let Some((prev_id, prev_face_idx)) = prev {
                if prev_face_idx == face_idx {
                    cursor_x += scaled.kern(prev_id, id);
                }
            }
            if let Some(outline) = face.outline(id) {
                let contours = glyph_contours(&outline.curves);
                let placed: Vec<Vec<Pos2>> = contours
                    .into_iter()
                    .map(|contour| {
                        contour
                            .into_iter()
                            .map(|p| {
                                let x = p.x * scale_factor.horizontal + cursor_x;
                                let y = p.y * -scale_factor.vertical + baseline_y;
                                let x = if style.italic { x + (baseline_y - y) * ITALIC_SHEAR } else { x };
                                Pos2::new(x, y)
                            })
                            .collect()
                    })
                    .filter(|c: &Vec<Pos2>| c.len() >= 3)
                    .collect();
                polygons.extend(group_contours_into_polygons(placed));
            }
            cursor_x += scaled.h_advance(id) + letter_spacing;
            prev = Some((id, face_idx));
        }
    }

    Some(polygons)
}

/// Splits one glyph's flat curve list into its separate closed contours
/// (a glyph like "i" has two: body and dot), flattening quadratic/cubic
/// segments to `CURVE_STEPS`-point polylines along the way. Contours are
/// implicit in `ab_glyph::Outline`'s flat `Vec<OutlineCurve>` — detected
/// here by a discontinuity between one segment's end and the next's start.
fn glyph_contours(curves: &[OutlineCurve]) -> Vec<Vec<Point>> {
    let mut contours = Vec::new();
    let mut current: Vec<Point> = Vec::new();
    for curve in curves {
        let (start, rest) = match *curve {
            OutlineCurve::Line(p0, p1) => (p0, vec![p1]),
            OutlineCurve::Quad(p0, c, p1) => (p0, flatten_quad(p0, c, p1)),
            OutlineCurve::Cubic(p0, c1, c2, p1) => (p0, flatten_cubic(p0, c1, c2, p1)),
        };
        let discontinuous = current.last().is_some_and(|last| (last.x - start.x).abs() > 1.0 || (last.y - start.y).abs() > 1.0);
        if discontinuous {
            contours.push(std::mem::take(&mut current));
        }
        if current.is_empty() {
            current.push(start);
        }
        current.extend(rest);
    }
    if !current.is_empty() {
        contours.push(current);
    }
    contours
}

fn flatten_quad(p0: Point, c: Point, p1: Point) -> Vec<Point> {
    (1..=CURVE_STEPS)
        .map(|i| {
            let t = i as f32 / CURVE_STEPS as f32;
            let mt = 1.0 - t;
            ab_glyph::point(
                mt * mt * p0.x + 2.0 * mt * t * c.x + t * t * p1.x,
                mt * mt * p0.y + 2.0 * mt * t * c.y + t * t * p1.y,
            )
        })
        .collect()
}

fn flatten_cubic(p0: Point, c1: Point, c2: Point, p1: Point) -> Vec<Point> {
    (1..=CURVE_STEPS)
        .map(|i| {
            let t = i as f32 / CURVE_STEPS as f32;
            let mt = 1.0 - t;
            ab_glyph::point(
                mt * mt * mt * p0.x + 3.0 * mt * mt * t * c1.x + 3.0 * mt * t * t * c2.x + t * t * t * p1.x,
                mt * mt * mt * p0.y + 3.0 * mt * mt * t * c1.y + 3.0 * mt * t * t * c2.y + t * t * t * p1.y,
            )
        })
        .collect()
}

/// Nests a glyph's contours into `PathPolygon`s by point-in-polygon
/// containment depth (even depth = a fillable exterior, odd = a hole of its
/// immediate parent) — winding direction is irrelevant since every consumer
/// of `PathPolygon` (`canvas.rs`/`export.rs`) already fills with
/// `FillRule::EvenOdd`. One glyph is usually 1-3 contours, so the O(n²)
/// containment check is negligible.
fn group_contours_into_polygons(contours: Vec<Vec<Pos2>>) -> Vec<PathPolygon> {
    let n = contours.len();
    if n == 0 {
        return Vec::new();
    }
    if n == 1 {
        return vec![PathPolygon { exterior: contours.into_iter().next().unwrap(), holes: Vec::new() }];
    }

    let sample: Vec<Pos2> = contours.iter().map(|c| c[0]).collect();
    let mut depth = vec![0usize; n];
    for i in 0..n {
        for j in 0..n {
            if i != j && point_in_polygon(sample[i], &contours[j]) {
                depth[i] += 1;
            }
        }
    }
    let mut parent: Vec<Option<usize>> = vec![None; n];
    for i in 0..n {
        for j in 0..n {
            if i != j && depth[j] + 1 == depth[i] && point_in_polygon(sample[i], &contours[j]) {
                parent[i] = Some(j);
            }
        }
    }

    let mut polygon_index = vec![None; n];
    let mut polygons = Vec::new();
    for i in 0..n {
        if depth[i] % 2 == 0 {
            polygon_index[i] = Some(polygons.len());
            polygons.push(PathPolygon { exterior: contours[i].clone(), holes: Vec::new() });
        }
    }
    for i in 0..n {
        if depth[i] % 2 == 1 {
            if let Some(idx) = parent[i].and_then(|p| polygon_index[p]) {
                polygons[idx].holes.push(contours[i].clone());
            }
        }
    }
    polygons
}

fn point_in_polygon(pt: Pos2, ring: &[Pos2]) -> bool {
    let mut inside = false;
    let n = ring.len();
    let mut j = n - 1;
    for i in 0..n {
        let (xi, yi) = (ring[i].x, ring[i].y);
        let (xj, yj) = (ring[j].x, ring[j].y);
        if (yi > pt.y) != (yj > pt.y) && pt.x < (xj - xi) * (pt.y - yi) / (yj - yi) + xi {
            inside = !inside;
        }
        j = i;
    }
    inside
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Frame, ListType, Paint, TextFont, TextTransform};

    fn text_layer(content: &str) -> Layer {
        let mut layer = Layer::new(
            "Text",
            Frame { pos: Pos2::new(0.0, 0.0), size: egui::Vec2::new(300.0, 100.0), rotation: 0.0 },
            LayerKind::Text {
                content: content.to_string(),
                font_size: 60.0,
                font: TextFont::Proportional,
                align: TextAlign::Left,
                vertical_align: VerticalAlign::Top,
                resize: TextResize::Fixed,
                line_height: None,
                letter_spacing: 0.0,
                paragraph_spacing: 0.0,
                bold: false,
                italic: false,
                underline: false,
                strikethrough: false,
                transform: TextTransform::None,
                list: ListType::None,
                list_start: 1,
                style_id: None,
                runs: Vec::new(),
            },
        );
        layer.style.fill = Some(Paint::Solid(egui::Color32::BLACK));
        layer
    }

    fn run_style(bold: bool) -> RunStyle {
        RunStyle {
            font: TextFont::Proportional,
            font_size: 60.0,
            color: None,
            bold,
            italic: false,
            underline: false,
            strikethrough: false,
        }
    }

    #[test]
    fn rich_outlines_produce_polygons_spanning_both_runs() {
        let mut layer = text_layer("Ab");
        if let LayerKind::Text { runs, .. } = &mut layer.kind {
            *runs = vec![TextRun { len: 1, style: run_style(false) }, TextRun { len: 1, style: run_style(true) }];
        }
        let polygons = convert_to_outlines(&layer).expect("should produce outlines for a rich two-run layer");
        assert!(!polygons.is_empty(), "expected at least one filled polygon for 'Ab'");
    }

    fn polygon_area(ring: &[Pos2]) -> f32 {
        let n = ring.len();
        let mut area = 0.0;
        for i in 0..n {
            let j = (i + 1) % n;
            area += ring[i].x * ring[j].y - ring[j].x * ring[i].y;
        }
        (area / 2.0).abs()
    }

    #[test]
    fn rich_outlines_bold_run_covers_more_area_than_regular_weight() {
        let plain = text_layer("M");
        let mut bold = text_layer("M");
        if let LayerKind::Text { runs, .. } = &mut bold.kind {
            *runs = vec![TextRun { len: 1, style: run_style(true) }];
        }

        let plain_area: f32 = convert_to_outlines(&plain).unwrap().iter().map(|p| polygon_area(&p.exterior)).sum();
        let bold_area: f32 = convert_to_outlines(&bold).unwrap().iter().map(|p| polygon_area(&p.exterior)).sum();

        assert!(
            bold_area > plain_area,
            "a bold run's real bold-weight outline (area={bold_area}) should cover more area than the regular weight (area={plain_area})"
        );
    }
}
