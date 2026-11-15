//! Turns a layer tree into an HTML document, a stylesheet, and the image
//! files they reference — pure, nothing touches the disk here (see
//! `html_export::export_html` for that).
//!
//! Every layer becomes one element nested like the layer tree. In
//! `LayoutMode::Absolute` each is absolutely positioned, so its `left`/`top`
//! stay parent-relative exactly as `Layer::frame.pos` is. In
//! `LayoutMode::Flexbox` a container's children are laid out by inferred
//! rows and columns instead (see `flex_children`). Layers CSS can't describe become `<img>`s of
//! their rasterized rendering (see `raster_fallback`). Rich-text runs become
//! `<span>`s, and every bundled font a text uses ships in `fonts/`.

use egui::Rect;

use super::LayoutMode;
use super::flex_children::plan_children;
use super::font_files::bundled_font_file;
use super::raster_fallback::{is_css_expressible, rasterize};
use crate::css_rules::{flow_layer_declarations, layer_declarations, run_declarations};
use crate::model::text_runs::TextRun;
use crate::model::{Layer, LayerKind, TextFont, TextResize};

/// The exported files' contents. `assets` (images and fonts) paths are
/// relative to the folder holding `index.html`.
pub struct ExportedSite {
    pub html: String,
    pub css: String,
    pub assets: Vec<(String, Vec<u8>)>,
}

/// Builds the site for `layer` and its descendants. `roots` is the page's
/// top-level layers, needed to rasterize path-attached text. Fails if
/// `layer` is hidden or a fallback rendering fails.
pub fn build_site(layer: &Layer, roots: &[Layer], layout: LayoutMode) -> anyhow::Result<ExportedSite> {
    if !layer.visible {
        anyhow::bail!("layer \"{}\" is hidden", layer.name);
    }
    let mut builder = SiteBuilder {
        roots,
        layout,
        next_id: 0,
        next_stack_id: 0,
        body: String::new(),
        rules: String::new(),
        assets: Vec::new(),
        used_fonts: Vec::new(),
    };
    let bounds = builder.add_layer(layer, 2, Placement::Absolute, None)?;
    let mut font_faces = String::new();
    for (font, bold) in &builder.used_fonts {
        if let Some(file) = bundled_font_file(font, *bold) {
            font_faces.push_str(&file.face_rule);
            font_faces.push('\n');
            builder.assets.push((file.path, file.bytes.to_vec()));
        }
    }
    let html = format!(
        "<!doctype html>\n<html>\n<head>\n<meta charset=\"utf-8\">\n<title>{}</title>\n<link rel=\"stylesheet\" href=\"style.css\">\n</head>\n<body>\n<div class=\"page\">\n<div class=\"origin\">\n{}</div>\n</div>\n</body>\n</html>\n",
        escape_html(&layer.name),
        builder.body
    );
    // In flexbox mode every element states its own `position`, and the reset
    // has zero specificity (`:where`) so per-element margins override it.
    let element_reset = match layout {
        LayoutMode::Absolute => ".page div, .page img { position: absolute; box-sizing: border-box; margin: 0; }",
        LayoutMode::Flexbox => ":where(.page div, .page img) { box-sizing: border-box; margin: 0; flex-shrink: 0; }",
    };
    // `.origin` shifts the exported layer's own page position back to the
    // top-left corner of `.page`, which is sized to its visual footprint.
    let css = format!(
        "{font_faces}body {{ margin: 0; }}\n.page {{ position: relative; width: {}; height: {}; overflow: hidden; }}\n.origin {{ position: absolute; left: {}; top: {}; }}\n{element_reset}\n\n{}",
        px(bounds.width()),
        px(bounds.height()),
        px(-bounds.min.x),
        px(-bounds.min.y),
        builder.rules
    );
    Ok(ExportedSite { html, css, assets: builder.assets })
}

/// How an element is placed inside its parent element.
pub(super) enum Placement {
    /// By `left`/`top` from the layer's frame.
    Absolute,
    /// By a flex row or column; `margins` come from that stack's spacing.
    InFlow { margins: Vec<String> },
}

/// Fields are `pub(super)` for `flex_children`, which adds its own
/// `impl SiteBuilder` block for the flexbox mode.
pub(super) struct SiteBuilder<'a> {
    pub(super) roots: &'a [Layer],
    pub(super) layout: LayoutMode,
    pub(super) next_id: usize,
    pub(super) next_stack_id: usize,
    pub(super) body: String,
    pub(super) rules: String,
    pub(super) assets: Vec<(String, Vec<u8>)>,
    /// `(font, bold)` pairs any exported text uses, in first-use order.
    pub(super) used_fonts: Vec<(TextFont, bool)>,
}

impl SiteBuilder<'_> {
    /// Appends `layer`'s element (indented by `depth`) and CSS rule, and
    /// returns the area it covers in its parent's space. `z_index` is the
    /// layer's order among its siblings, needed only in flexbox mode.
    pub(super) fn add_layer(&mut self, layer: &Layer, depth: usize, placement: Placement, z_index: Option<usize>) -> anyhow::Result<Rect> {
        let class = format!("layer-{}", self.next_id);
        self.next_id += 1;
        let indent = " ".repeat(depth);

        if !is_css_expressible(layer) {
            let raster = rasterize(layer, self.roots)?;
            let src = format!("images/{class}.png");
            self.body.push_str(&format!("{indent}<img class=\"{class}\" src=\"{src}\" alt=\"{}\">\n", escape_html(&layer.name)));
            let b = raster.bounds;
            let mut declarations = vec![format!("left: {};", px(b.min.x)), format!("top: {};", px(b.min.y)), format!("width: {};", px(b.width())), format!("height: {};", px(b.height()))];
            declarations.extend(self.position_declarations(placement, z_index));
            self.push_rule(&class, &layer.name, &declarations);
            self.assets.push((src, raster.png));
            return Ok(b);
        }

        let mut declarations = match placement {
            Placement::Absolute => layer_declarations(layer),
            Placement::InFlow { .. } => flow_layer_declarations(layer),
        };
        if let LayerKind::Text { resize, .. } = &layer.kind {
            let wrap = if *resize == TextResize::Auto { "pre" } else { "pre-wrap" };
            declarations.push(format!("white-space: {wrap};"));
        }
        declarations.extend(self.position_declarations(placement, z_index));
        let child_plan = match (&layer.kind, self.layout) {
            (LayerKind::Artboard { children, .. } | LayerKind::Group { children }, LayoutMode::Flexbox) => Some(plan_children(children)),
            _ => None,
        };
        if let Some(plan) = &child_plan {
            declarations.extend(plan.container_declarations());
        }
        self.push_rule(&class, &layer.name, &declarations);
        match &layer.kind {
            LayerKind::Image { encoded, .. } => {
                let src = format!("images/{class}.png");
                self.body.push_str(&format!("{indent}<img class=\"{class}\" src=\"{src}\" alt=\"{}\">\n", escape_html(&layer.name)));
                self.assets.push((src, encoded.clone()));
            }
            LayerKind::Text { content, runs, font, bold, .. } => {
                self.use_font(font, *bold);
                let inner = if runs.is_empty() { escape_html(content) } else { self.run_spans(&class, content, runs, layer.style.fill_opacity) };
                self.body.push_str(&format!("{indent}<div class=\"{class}\">{inner}</div>\n"));
            }
            LayerKind::Artboard { children, .. } | LayerKind::Group { children } => {
                self.body.push_str(&format!("{indent}<div class=\"{class}\">\n"));
                match &child_plan {
                    Some(plan) => self.add_planned_children(plan, children, depth + 1)?,
                    None => {
                        for child in children.iter().filter(|child| child.visible) {
                            self.add_layer(child, depth + 1, Placement::Absolute, None)?;
                        }
                    }
                }
                self.body.push_str(&format!("{indent}</div>\n"));
            }
            _ => self.body.push_str(&format!("{indent}<div class=\"{class}\"></div>\n")),
        }
        Ok(layer.frame.rotated_bounds())
    }

    /// One `<span>` per run, each with its own rule. Relies on the
    /// `TextRun` invariant that run lengths sum to `content`'s char count
    /// (see `model/text_runs.rs`).
    fn run_spans(&mut self, class: &str, content: &str, runs: &[TextRun], fill_opacity: f32) -> String {
        let mut chars = content.chars();
        let mut html = String::new();
        for (index, run) in runs.iter().enumerate() {
            let text: String = chars.by_ref().take(run.len).collect();
            let run_class = format!("{class}-run-{index}");
            html.push_str(&format!("<span class=\"{run_class}\">{}</span>", escape_html(&text)));
            self.push_rule(&run_class, &format!("run {index}"), &run_declarations(&run.style, fill_opacity));
            self.use_font(&run.style.font, run.style.bold);
        }
        html
    }

    /// Absolute mode: nothing (the stylesheet's element reset positions
    /// everything). Flexbox mode: the element's own `position`, its stack
    /// margins, and `z-index` (see `flex_children`'s module doc).
    fn position_declarations(&self, placement: Placement, z_index: Option<usize>) -> Vec<String> {
        if self.layout == LayoutMode::Absolute {
            return Vec::new();
        }
        let mut declarations = match placement {
            Placement::Absolute => vec!["position: absolute;".to_string()],
            Placement::InFlow { margins } => std::iter::once("position: relative;".to_string()).chain(margins).collect(),
        };
        if let Some(z) = z_index {
            declarations.push(format!("z-index: {z};"));
        }
        declarations
    }

    fn use_font(&mut self, font: &TextFont, bold: bool) {
        let entry = (font.clone(), bold);
        if !self.used_fonts.contains(&entry) {
            self.used_fonts.push(entry);
        }
    }

    pub(super) fn push_rule(&mut self, class: &str, name: &str, declarations: &[String]) {
        self.rules.push_str(&format!("/* {} */\n.{class} {{\n", name.replace("*/", "* /")));
        for declaration in declarations {
            self.rules.push_str(&format!("  {declaration}\n"));
        }
        self.rules.push_str("}\n\n");
    }
}

fn escape_html(text: &str) -> String {
    text.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

fn px(value: f32) -> String {
    format!("{}px", (value * 100.0).round() / 100.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::text_runs::RunStyle;
    use crate::model::{CornerRadii, Frame, ListType, TextAlign, TextTransform, VerticalAlign};
    use egui::{Color32, Pos2, Vec2};

    fn frame(x: f32, y: f32, w: f32, h: f32) -> Frame {
        Frame { pos: Pos2::new(x, y), size: Vec2::new(w, h), rotation: 0.0 }
    }

    fn artboard(children: Vec<Layer>) -> Layer {
        Layer::new("Home <page>", frame(100.0, 50.0, 300.0, 200.0), LayerKind::Artboard { children, background: Color32::WHITE, adjust_content_on_resize: false })
    }

    #[test]
    fn nests_children_and_shifts_the_root_to_the_page_corner() {
        let rect = Layer::new("Card", frame(10.0, 20.0, 50.0, 40.0), LayerKind::Rectangle { corner_radius: CornerRadii::ZERO });
        let root = artboard(vec![rect]);
        let site = build_site(&root, std::slice::from_ref(&root), LayoutMode::Absolute).unwrap();
        assert!(site.html.contains("<title>Home &lt;page&gt;</title>"), "{}", site.html);
        assert!(site.html.contains("  <div class=\"layer-0\">\n   <div class=\"layer-1\"></div>\n  </div>"), "{}", site.html);
        assert!(site.css.contains(".page { position: relative; width: 300px; height: 200px;"), "{}", site.css);
        assert!(site.css.contains(".origin { position: absolute; left: -100px; top: -50px; }"), "{}", site.css);
        assert!(site.css.contains("/* Card */\n.layer-1 {\n  left: 10px;"), "{}", site.css);
        assert!(site.assets.is_empty());
    }

    #[test]
    fn flexbox_mode_stacks_children_in_a_column_with_a_gap() {
        let top = Layer::new("Top", frame(20.0, 30.0, 100.0, 40.0), LayerKind::Rectangle { corner_radius: CornerRadii::ZERO });
        let bottom = Layer::new("Bottom", frame(20.0, 90.0, 100.0, 40.0), LayerKind::Rectangle { corner_radius: CornerRadii::ZERO });
        let root = artboard(vec![bottom, top]);
        let site = build_site(&root, std::slice::from_ref(&root), LayoutMode::Flexbox).unwrap();
        assert!(site.css.contains("  display: flex;\n  align-items: flex-start;\n  padding: 30px 0 0 20px;"), "{}", site.css);
        assert!(site.css.contains(".stack-0 {\n  display: flex;\n  flex-direction: column;\n  align-items: flex-start;\n  gap: 20px;\n}"), "{}", site.css);
        // Visual order (Top first) in the DOM, layer order kept by z-index.
        assert!(site.html.contains("<div class=\"stack-0\">\n    <div class=\"layer-1\"></div>\n    <div class=\"layer-2\"></div>"), "{}", site.html);
        assert!(site.css.contains("/* Top */\n.layer-1 {\n  width: 100px;"), "{}", site.css);
        assert!(site.css.contains("position: relative;\n  z-index: 1;"), "{}", site.css);
    }

    #[test]
    fn hidden_children_are_skipped() {
        let mut rect = Layer::new("Ghost", frame(0.0, 0.0, 5.0, 5.0), LayerKind::Oval);
        rect.visible = false;
        let root = artboard(vec![rect]);
        let site = build_site(&root, std::slice::from_ref(&root), LayoutMode::Absolute).unwrap();
        assert!(!site.css.contains("Ghost"), "{}", site.css);
    }

    #[test]
    fn star_becomes_a_rasterized_image() {
        let star = Layer::new("Star", frame(10.0, 10.0, 30.0, 30.0), LayerKind::Star { points: 5, inner_ratio: 0.5 });
        let root = artboard(vec![star]);
        let site = build_site(&root, std::slice::from_ref(&root), LayoutMode::Absolute).unwrap();
        assert!(site.html.contains("<img class=\"layer-1\" src=\"images/layer-1.png\" alt=\"Star\">"), "{}", site.html);
        assert_eq!(site.assets.len(), 1);
        assert_eq!(site.assets[0].0, "images/layer-1.png");
    }

    #[test]
    fn rich_text_becomes_spans_and_ships_each_used_font_once() {
        let base = RunStyle { font: TextFont::Serif, font_size: 20.0, color: None, bold: false, italic: false, underline: false, strikethrough: false };
        let bold_run = RunStyle { bold: true, ..base.clone() };
        let text = Layer::new(
            "Title",
            frame(0.0, 0.0, 200.0, 40.0),
            LayerKind::Text {
                content: "Hi <you>".to_string(),
                font_size: 20.0,
                font: TextFont::Serif,
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
                runs: vec![TextRun { len: 3, style: base }, TextRun { len: 5, style: bold_run }],
                path_attachment: None,
            },
        );
        let root = artboard(vec![text]);
        let site = build_site(&root, std::slice::from_ref(&root), LayoutMode::Absolute).unwrap();
        assert!(site.html.contains("<div class=\"layer-1\"><span class=\"layer-1-run-0\">Hi </span><span class=\"layer-1-run-1\">&lt;you&gt;</span></div>"), "{}", site.html);
        assert!(site.css.contains(".layer-1-run-1 {\n  font-family: \"Merriweather\", serif;"), "{}", site.css);
        let font_paths: Vec<&str> = site.assets.iter().map(|(path, _)| path.as_str()).collect();
        assert_eq!(font_paths, ["fonts/Merriweather-Regular.ttf", "fonts/Merriweather-Bold.ttf"]);
        assert!(site.css.starts_with("@font-face { font-family: \"Merriweather\";"), "{}", site.css);
    }

    #[test]
    fn hidden_root_is_an_error() {
        let mut root = artboard(Vec::new());
        root.visible = false;
        assert!(build_site(&root, std::slice::from_ref(&root), LayoutMode::Absolute).is_err());
    }
}
