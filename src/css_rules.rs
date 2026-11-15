//! Describes a layer's box, paint, and typography as CSS declarations, for
//! the read-only CSS panel (`ui/css_panel.rs`). One-way only: nothing parses
//! CSS back into the model. Anything CSS can't express (procedural fills,
//! non-box outlines, per-character rich-text runs) becomes a `/* ... */`
//! note rather than being silently dropped.
//!
//! `left`/`top` are `frame.pos`, which is parent-relative — the same meaning
//! as `position: absolute` inside a positioned parent.
//!
//! Also used by HTML export (`html_export/`). Kept as one file past the size
//! target on purpose: box, paint, and text rules all share the private
//! `color`/`px` formatting, and splitting would mean sharing or copying it.

use egui::{Color32, Vec2};

use crate::model::text_runs::RunStyle;
use crate::model::{GradientKind, Layer, LayerKind, Paint, Shadow, TextAlign, TextFont, TextTransform};

/// Returns `layer`'s declarations as one CSS block, headed by a comment with
/// the layer's name. Pure: no side effects.
pub fn layer_css(layer: &Layer) -> String {
    let mut lines = vec![format!("/* {} */", layer.name)];
    lines.extend(layer_declarations(layer));
    lines.join("\n")
}

/// `layer`'s declarations (and any `/* ... */` notes), one per entry, with
/// no header — the body of a CSS rule. Pure: no side effects.
pub fn layer_declarations(layer: &Layer) -> Vec<String> {
    let mut lines = Vec::new();
    box_rules(layer, &mut lines);
    match &layer.kind {
        LayerKind::Artboard { background, .. } => lines.push(format!("background: {};", color(*background, 1.0))),
        LayerKind::Group { .. } | LayerKind::Image { .. } => {}
        LayerKind::Text { .. } => text_rules(layer, &mut lines),
        _ => shape_rules(layer, &mut lines),
    }
    lines
}

fn box_rules(layer: &Layer, lines: &mut Vec<String>) {
    let frame = &layer.frame;
    if matches!(layer.kind, LayerKind::Line | LayerKind::Arrow { .. }) {
        // A line is drawn as a zero-height box rotated about its start point.
        let direction = frame.end() - frame.start();
        lines.push(format!("left: {};", px(frame.pos.x)));
        lines.push(format!("top: {};", px(frame.pos.y)));
        lines.push(format!("width: {};", px(direction.length())));
        lines.push("height: 0;".to_string());
        lines.push("transform-origin: 0 0;".to_string());
        lines.push(format!("transform: rotate({}deg);", round(direction.y.atan2(direction.x).to_degrees())));
    } else {
        let bounds = frame.bounds();
        lines.push(format!("left: {};", px(bounds.min.x)));
        lines.push(format!("top: {};", px(bounds.min.y)));
        lines.push(format!("width: {};", px(bounds.width())));
        lines.push(format!("height: {};", px(bounds.height())));
        if frame.rotation != 0.0 {
            lines.push(format!("transform: rotate({}deg);", round(frame.rotation)));
        }
    }
    if layer.opacity < 1.0 {
        lines.push(format!("opacity: {};", round(layer.opacity)));
    }
}

fn shape_rules(layer: &Layer, lines: &mut Vec<String>) {
    let style = &layer.style;
    let size = layer.frame.bounds().size();
    let is_line = matches!(layer.kind, LayerKind::Line | LayerKind::Arrow { .. });
    if let Some(fill) = &style.fill
        && !is_line
    {
        match paint_css(fill, style.fill_opacity, size) {
            Some(value) => lines.push(format!("background: {value};")),
            None => lines.push(format!("/* background: {} fill has no CSS equivalent */", paint_kind_name(fill))),
        }
    }
    if let Some(stroke) = &style.stroke {
        let property = if is_line { "border-top" } else { "border" };
        match &stroke.paint {
            Paint::Solid(c) => lines.push(format!("{property}: {} solid {};", px(stroke.width), color(*c, style.stroke_opacity))),
            other => {
                lines.push(format!("{property}: {} solid;", px(stroke.width)));
                if let Some(value) = paint_css(other, style.stroke_opacity, size) {
                    lines.push(format!("border-image: {value} 1;"));
                }
            }
        }
    }
    match &layer.kind {
        LayerKind::Rectangle { corner_radius } => {
            let radii = corner_radius.as_array();
            if radii.iter().all(|&r| r == radii[0]) {
                if radii[0] > 0.0 {
                    lines.push(format!("border-radius: {};", px(radii[0])));
                }
            } else {
                let values: Vec<String> = radii.iter().map(|&r| px(r)).collect();
                lines.push(format!("border-radius: {};", values.join(" ")));
            }
        }
        LayerKind::Oval => lines.push("border-radius: 50%;".to_string()),
        LayerKind::Star { .. } | LayerKind::Polygon { .. } | LayerKind::Path { .. } | LayerKind::CompoundPath { .. } | LayerKind::BooleanGroup { .. } => {
            lines.push(format!("/* {} outline has no CSS equivalent; only its box is described */", layer.kind.type_name()));
        }
        _ => {}
    }
    let shadows: Vec<String> = style
        .shadows
        .iter()
        .map(|s| shadow_css(s, ""))
        .chain(style.inner_shadows.iter().map(|s| shadow_css(s, "inset ")))
        .collect();
    if !shadows.is_empty() {
        lines.push(format!("box-shadow: {};", shadows.join(", ")));
    }
}

/// Text colors are always flat — glyphs render only `Paint::to_color32`
/// (see `Paint::to_color32`'s doc comment), so that's what CSS shows too.
fn text_rules(layer: &Layer, lines: &mut Vec<String>) {
    let LayerKind::Text { font_size, font, align, line_height, letter_spacing, bold, italic, underline, strikethrough, transform, runs, .. } = &layer.kind else {
        return;
    };
    let style = &layer.style;
    if let Some(fill) = &style.fill {
        lines.push(format!("color: {};", color(fill.to_color32(), style.fill_opacity)));
    }
    if let Some(stroke) = &style.stroke {
        lines.push(format!("-webkit-text-stroke: {} {};", px(stroke.width), color(stroke.paint.to_color32(), style.stroke_opacity)));
    }
    lines.push(format!("font-family: {};", font_family(font)));
    lines.push(format!("font-size: {};", px(*font_size)));
    if *bold {
        lines.push("font-weight: bold;".to_string());
    }
    if *italic {
        lines.push("font-style: italic;".to_string());
    }
    let decorations: Vec<&str> = [(*underline, "underline"), (*strikethrough, "line-through")]
        .into_iter()
        .filter_map(|(on, name)| on.then_some(name))
        .collect();
    if !decorations.is_empty() {
        lines.push(format!("text-decoration: {};", decorations.join(" ")));
    }
    let align_value = match align {
        TextAlign::Left => "left",
        TextAlign::Center => "center",
        TextAlign::Right => "right",
        TextAlign::Justify => "justify",
    };
    lines.push(format!("text-align: {align_value};"));
    if let Some(height) = line_height {
        lines.push(format!("line-height: {};", px(*height)));
    }
    if *letter_spacing != 0.0 {
        lines.push(format!("letter-spacing: {};", px(*letter_spacing)));
    }
    match transform {
        TextTransform::None => {}
        TextTransform::Uppercase => lines.push("text-transform: uppercase;".to_string()),
        TextTransform::Lowercase => lines.push("text-transform: lowercase;".to_string()),
        TextTransform::Titlecase => lines.push("text-transform: capitalize;".to_string()),
    }
    let text_shadows: Vec<String> = style.shadows.iter().map(|s| format!("{} {} {} {}", px(s.offset.x), px(s.offset.y), px(s.blur), color(s.color, 1.0))).collect();
    if !text_shadows.is_empty() {
        lines.push(format!("text-shadow: {};", text_shadows.join(", ")));
    }
    if !runs.is_empty() {
        lines.push("/* mixed per-character styles; values above are the layer defaults */".to_string());
    }
}

/// Declarations for one rich-text run (`TextRun::style`). Every property is
/// stated explicitly, even `normal`, so a run never inherits the layer-level
/// defaults that `text_rules` wrote on the enclosing element. A run with no
/// color inherits the element's `color`, same as on canvas. Pure.
pub fn run_declarations(style: &RunStyle, fill_opacity: f32) -> Vec<String> {
    let mut lines = vec![format!("font-family: {};", font_family(&style.font)), format!("font-size: {};", px(style.font_size))];
    if let Some(run_color) = style.color {
        lines.push(format!("color: {};", color(run_color, fill_opacity)));
    }
    lines.push(format!("font-weight: {};", if style.bold { "bold" } else { "normal" }));
    lines.push(format!("font-style: {};", if style.italic { "italic" } else { "normal" }));
    let decoration = match (style.underline, style.strikethrough) {
        (false, false) => "none",
        (true, false) => "underline",
        (false, true) => "line-through",
        (true, true) => "underline line-through",
    };
    lines.push(format!("text-decoration: {decoration};"));
    lines
}

/// `None` for the procedural/texture fills, which CSS has no equivalent for.
/// `size` is the painted box, needed because gradient endpoints are stored
/// normalized to it (see `Gradient`).
fn paint_css(paint: &Paint, opacity: f32, size: Vec2) -> Option<String> {
    match paint {
        Paint::Solid(c) => Some(color(*c, opacity)),
        Paint::Gradient(gradient) => {
            let stops: Vec<String> = gradient.stops.iter().map(|s| format!("{} {}%", color(s.color, opacity), round(s.offset * 100.0))).collect();
            let stops = stops.join(", ");
            Some(match gradient.kind {
                GradientKind::Linear => {
                    let axis = (gradient.to - gradient.from) * size;
                    // CSS angles start at "to top" and run clockwise.
                    let angle = axis.x.atan2(-axis.y).to_degrees().rem_euclid(360.0);
                    format!("linear-gradient({}deg, {stops})", round(angle))
                }
                GradientKind::Radial => {
                    let radius = ((gradient.to - gradient.from) * size).length();
                    let (x, y) = (gradient.from.x * 100.0, gradient.from.y * 100.0);
                    format!("radial-gradient(circle {} at {}% {}%, {stops})", px(radius), round(x), round(y))
                }
            })
        }
        Paint::Noise(_) | Paint::Halftone(_) | Paint::Pattern(_) => None,
    }
}

fn paint_kind_name(paint: &Paint) -> &'static str {
    match paint {
        Paint::Solid(_) => "solid",
        Paint::Gradient(_) => "gradient",
        Paint::Noise(_) => "noise",
        Paint::Halftone(_) => "halftone",
        Paint::Pattern(_) => "pattern",
    }
}

fn shadow_css(shadow: &Shadow, prefix: &str) -> String {
    format!("{prefix}{} {} {} {} {}", px(shadow.offset.x), px(shadow.offset.y), px(shadow.blur), px(shadow.spread), color(shadow.color, 1.0))
}

/// The CSS family name for a bundled font (its real name, see `fonts.rs` /
/// `assets/fonts/`); `None` for `TextFont::System`. Shared with
/// `html_export/font_files.rs`, whose `@font-face` rules must declare exactly
/// these names for `font_family`'s output to pick them up.
pub fn bundled_family_name(font: &TextFont) -> Option<&'static str> {
    match font {
        TextFont::Proportional => Some("Ubuntu"),
        TextFont::Monospace => Some("Hack"),
        TextFont::Serif => Some("Merriweather"),
        TextFont::Display => Some("Poppins"),
        TextFont::Handwriting => Some("Caveat"),
        TextFont::System(_) => None,
    }
}

/// A font's family name plus a generic fallback.
fn font_family(font: &TextFont) -> String {
    let generic = match font {
        TextFont::Monospace => "monospace",
        TextFont::Serif => "serif",
        TextFont::Handwriting => "cursive",
        TextFont::Proportional | TextFont::Display | TextFont::System(_) => "sans-serif",
    };
    match font {
        TextFont::System(name) => format!("\"{name}\", {generic}"),
        bundled => format!("\"{}\", {generic}", bundled_family_name(bundled).unwrap_or_default()),
    }
}

/// `#rrggbb` when fully opaque, `rgba(...)` otherwise. `opacity` is a
/// `Style` fill/stroke opacity, folded into the alpha since CSS has no
/// separate per-paint opacity.
fn color(c: Color32, opacity: f32) -> String {
    let [r, g, b, a] = c.to_srgba_unmultiplied();
    let alpha = a as f32 / 255.0 * opacity;
    if alpha >= 0.999 {
        format!("#{r:02x}{g:02x}{b:02x}")
    } else {
        format!("rgba({r}, {g}, {b}, {})", round(alpha))
    }
}

fn px(value: f32) -> String {
    if value == 0.0 { "0".to_string() } else { format!("{}px", round(value)) }
}

/// Two decimals at most, so `f32` noise like `0.30000001` doesn't leak into
/// the output; `Display` then drops a trailing `.0`.
fn round(value: f32) -> f32 {
    (value * 100.0).round() / 100.0
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{CornerRadii, Frame, Gradient, ListType, NoiseFill, Stroke, TextResize, VerticalAlign};
    use egui::Pos2;

    fn layer(kind: LayerKind) -> Layer {
        Layer::new("Box", Frame { pos: Pos2::new(10.0, 20.0), size: Vec2::new(100.0, 50.0), rotation: 0.0 }, kind)
    }

    fn text_layer() -> Layer {
        layer(LayerKind::Text {
            content: "Hi".to_string(),
            font_size: 24.0,
            font: TextFont::Serif,
            align: TextAlign::Center,
            vertical_align: VerticalAlign::Top,
            resize: TextResize::Fixed,
            line_height: Some(32.0),
            letter_spacing: 0.5,
            paragraph_spacing: 0.0,
            bold: true,
            italic: false,
            underline: true,
            strikethrough: true,
            transform: TextTransform::Uppercase,
            list: ListType::None,
            list_start: 1,
            style_id: None,
            runs: Vec::new(),
            path_attachment: None,
        })
    }

    #[test]
    fn rectangle_describes_box_fill_border_and_radius() {
        let mut rect = layer(LayerKind::Rectangle { corner_radius: CornerRadii::uniform(4.0) });
        rect.style.fill = Some(Paint::Solid(Color32::from_rgb(255, 0, 0)));
        rect.style.stroke = Some(Stroke { paint: Paint::Solid(Color32::BLACK), width: 2.0 });
        rect.frame.rotation = 15.0;
        let css = layer_css(&rect);
        for expected in ["/* Box */", "left: 10px;", "top: 20px;", "width: 100px;", "height: 50px;", "transform: rotate(15deg);", "background: #ff0000;", "border: 2px solid #000000;", "border-radius: 4px;"] {
            assert!(css.contains(expected), "missing `{expected}` in:\n{css}");
        }
    }

    #[test]
    fn fill_opacity_is_folded_into_the_color_alpha() {
        let mut rect = layer(LayerKind::Oval);
        rect.style.fill = Some(Paint::Solid(Color32::from_rgb(0, 0, 255)));
        rect.style.fill_opacity = 0.5;
        let css = layer_css(&rect);
        assert!(css.contains("background: rgba(0, 0, 255, 0.5);"), "{css}");
        assert!(css.contains("border-radius: 50%;"), "{css}");
    }

    #[test]
    fn horizontal_linear_gradient_points_to_the_right() {
        let mut gradient = Gradient::linear(Color32::WHITE, Color32::BLACK);
        gradient.from = Pos2::new(0.0, 0.5);
        gradient.to = Pos2::new(1.0, 0.5);
        let mut rect = layer(LayerKind::Rectangle { corner_radius: CornerRadii::ZERO });
        rect.style.fill = Some(Paint::Gradient(gradient));
        let css = layer_css(&rect);
        assert!(css.contains("background: linear-gradient(90deg, #ffffff 0%, #000000 100%);"), "{css}");
    }

    #[test]
    fn procedural_fill_becomes_a_note() {
        let mut rect = layer(LayerKind::Rectangle { corner_radius: CornerRadii::ZERO });
        rect.style.fill = Some(Paint::Noise(NoiseFill { base: Color32::GRAY, intensity: 0.5, scale: 2.0, seed: 1 }));
        let css = layer_css(&rect);
        assert!(css.contains("/* background: noise fill has no CSS equivalent */"), "{css}");
        assert!(!css.lines().any(|line| line.starts_with("background:")), "{css}");
    }

    #[test]
    fn shadows_and_inner_shadows_share_one_box_shadow() {
        let mut rect = layer(LayerKind::Rectangle { corner_radius: CornerRadii::ZERO });
        let shadow = Shadow { color: Color32::BLACK, offset: Vec2::new(0.0, 4.0), blur: 8.0, spread: 0.0 };
        rect.style.shadows = vec![shadow];
        rect.style.inner_shadows = vec![shadow];
        let css = layer_css(&rect);
        assert!(css.contains("box-shadow: 0 4px 8px 0 #000000, inset 0 4px 8px 0 #000000;"), "{css}");
    }

    #[test]
    fn text_describes_typography() {
        let mut text = text_layer();
        text.style.fill = Some(Paint::Solid(Color32::from_rgb(34, 34, 34)));
        text.style.stroke = None;
        let css = layer_css(&text);
        for expected in ["color: #222222;", "font-family: \"Merriweather\", serif;", "font-size: 24px;", "font-weight: bold;", "text-decoration: underline line-through;", "text-align: center;", "line-height: 32px;", "letter-spacing: 0.5px;", "text-transform: uppercase;"] {
            assert!(css.contains(expected), "missing `{expected}` in:\n{css}");
        }
        assert!(!css.contains("background"), "{css}");
    }

    #[test]
    fn run_states_every_property_explicitly() {
        let style = RunStyle { font: TextFont::Display, font_size: 12.0, color: None, bold: false, italic: true, underline: false, strikethrough: true };
        let lines = run_declarations(&style, 1.0);
        assert_eq!(lines, ["font-family: \"Poppins\", sans-serif;", "font-size: 12px;", "font-weight: normal;", "font-style: italic;", "text-decoration: line-through;"]);
    }

    #[test]
    fn line_is_a_rotated_zero_height_box_with_a_top_border() {
        let mut line = layer(LayerKind::Line);
        line.frame.size = Vec2::new(0.0, 30.0);
        let css = layer_css(&line);
        for expected in ["width: 30px;", "height: 0;", "transform: rotate(90deg);", "border-top: 1px solid #1e1e1e;"] {
            assert!(css.contains(expected), "missing `{expected}` in:\n{css}");
        }
    }
}
