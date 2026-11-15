//! End-to-end gesture tests for `CanvasWidget::ui`: each one drives a real
//! headless `egui::Context` frame by frame with synthesized pointer/keyboard
//! events, then checks the resulting document/selection/widget state. Every
//! other canvas test calls a helper directly; these are the only ones that
//! exercise `ui()`'s own dispatch (hover → drag start → drag → drag stop →
//! menus/overlays), so they guard against a gesture silently breaking when
//! `ui()` itself is restructured.

use egui::{Event, Key, Modifiers, PointerButton, Pos2, Rect, Vec2};

use crate::history::History;
use crate::model::{CornerRadii, Document, Frame, GuideOrientation, Layer, LayerId, LayerKind};
use crate::tools::Tool;

use super::layer_creation::new_layer_for_tool;
use super::{CanvasWidget, RULER_SIZE};

/// Simulated time advanced per frame — small enough that a press/release
/// pair counts as a click and two clicks count as a double-click.
const FRAME_SECONDS: f64 = 0.05;
const SCREEN_SIZE: Vec2 = Vec2::new(800.0, 600.0);

struct Harness {
    ctx: egui::Context,
    widget: CanvasWidget,
    history: History,
    tool: Tool,
    selection: Vec<LayerId>,
    time: f64,
}

impl Harness {
    fn new(layers: Vec<Layer>) -> Self {
        let mut doc = Document::new();
        doc.active_page_mut().layers.extend(layers);
        let ctx = egui::Context::default();
        crate::fonts::install_fonts(&ctx);
        let mut harness = Harness {
            ctx,
            widget: CanvasWidget::default(),
            history: History::new(doc),
            tool: Tool::Select,
            selection: Vec::new(),
            time: 0.0,
        };
        // Warm-up frame: loads the installed fonts before any text is laid out.
        harness.frame(Vec::new());
        harness
    }

    fn frame(&mut self, events: Vec<Event>) {
        let input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, SCREEN_SIZE)),
            time: Some(self.time),
            events,
            ..Default::default()
        };
        self.time += FRAME_SECONDS;
        let Harness { ctx, widget, history, tool, selection, .. } = self;
        ctx.run_ui(input, |ui| widget.ui(ui, history, tool, selection)).drop_without_applying_deltas();
    }

    /// Doc-space → screen-space, for the widget's default pan/zoom with rulers shown.
    fn screen(&self, doc: Pos2) -> Pos2 {
        self.widget.to_screen(Pos2::new(RULER_SIZE, RULER_SIZE), doc)
    }

    fn press(pos: Pos2, button: PointerButton, pressed: bool, modifiers: Modifiers) -> Event {
        Event::PointerButton { pos, button, pressed, modifiers }
    }

    fn click_screen(&mut self, pos: Pos2, button: PointerButton, modifiers: Modifiers) {
        self.frame(vec![Event::ModifiersChanged(modifiers), Event::PointerMoved(pos)]);
        self.frame(vec![Self::press(pos, button, true, modifiers)]);
        self.frame(vec![Self::press(pos, button, false, modifiers)]);
        self.frame(vec![Event::ModifiersChanged(Modifiers::NONE)]);
    }

    fn click(&mut self, doc: Pos2, modifiers: Modifiers) {
        self.click_screen(self.screen(doc), PointerButton::Primary, modifiers);
    }

    fn double_click(&mut self, doc: Pos2) {
        let pos = self.screen(doc);
        self.frame(vec![Event::PointerMoved(pos)]);
        for _ in 0..2 {
            self.frame(vec![Self::press(pos, PointerButton::Primary, true, Modifiers::NONE)]);
            self.frame(vec![Self::press(pos, PointerButton::Primary, false, Modifiers::NONE)]);
        }
        self.frame(Vec::new());
    }

    /// Press at `from`, move to `to` over several frames, release there.
    fn drag_screen(&mut self, from: Pos2, to: Pos2) {
        self.frame(vec![Event::PointerMoved(from)]);
        self.frame(vec![Self::press(from, PointerButton::Primary, true, Modifiers::NONE)]);
        for step in 1..=4 {
            self.frame(vec![Event::PointerMoved(from + (to - from) * (step as f32 / 4.0))]);
        }
        self.frame(vec![Self::press(to, PointerButton::Primary, false, Modifiers::NONE)]);
        self.frame(Vec::new());
    }

    fn drag(&mut self, from_doc: Pos2, to_doc: Pos2) {
        self.drag_screen(self.screen(from_doc), self.screen(to_doc));
    }

    fn key(&mut self, key: Key) {
        let event = |pressed| Event::Key { key, physical_key: None, pressed, repeat: false, modifiers: Modifiers::NONE };
        self.frame(vec![event(true)]);
        self.frame(vec![event(false)]);
    }

    fn layer(&self, id: LayerId) -> &Layer {
        self.history.get().active_page().find(id).expect("layer exists")
    }
}

fn rect_layer(name: &str, x: f32, y: f32, w: f32, h: f32) -> Layer {
    Layer::new(
        name,
        Frame { pos: Pos2::new(x, y), size: Vec2::new(w, h), rotation: 0.0 },
        LayerKind::Rectangle { corner_radius: CornerRadii::ZERO },
    )
}

#[test]
fn click_selects_the_layer_under_the_pointer() {
    let rect = rect_layer("A", 100.0, 100.0, 100.0, 80.0);
    let id = rect.id;
    let mut h = Harness::new(vec![rect]);
    h.click(Pos2::new(150.0, 140.0), Modifiers::NONE);
    assert_eq!(h.selection, vec![id]);
}

#[test]
fn click_on_empty_canvas_clears_the_selection() {
    let rect = rect_layer("A", 100.0, 100.0, 100.0, 80.0);
    let id = rect.id;
    let mut h = Harness::new(vec![rect]);
    h.selection = vec![id];
    h.click(Pos2::new(500.0, 400.0), Modifiers::NONE);
    assert!(h.selection.is_empty());
}

#[test]
fn shift_click_adds_then_removes_a_layer_from_the_selection() {
    let a = rect_layer("A", 100.0, 100.0, 80.0, 80.0);
    let b = rect_layer("B", 300.0, 100.0, 80.0, 80.0);
    let (a_id, b_id) = (a.id, b.id);
    let mut h = Harness::new(vec![a, b]);
    h.click(Pos2::new(140.0, 140.0), Modifiers::NONE);
    h.click(Pos2::new(340.0, 140.0), Modifiers::SHIFT);
    assert_eq!(h.selection, vec![a_id, b_id]);
    h.click(Pos2::new(140.0, 140.0), Modifiers::SHIFT);
    assert_eq!(h.selection, vec![b_id]);
}

#[test]
fn marquee_drag_over_empty_canvas_selects_every_intersecting_layer() {
    let a = rect_layer("A", 100.0, 100.0, 80.0, 80.0);
    let b = rect_layer("B", 300.0, 100.0, 80.0, 80.0);
    let far = rect_layer("Far", 600.0, 450.0, 50.0, 50.0);
    let (a_id, b_id) = (a.id, b.id);
    let mut h = Harness::new(vec![a, b, far]);
    h.drag(Pos2::new(50.0, 50.0), Pos2::new(350.0, 250.0));
    let mut selected = h.selection.clone();
    selected.sort();
    let mut expected = vec![a_id, b_id];
    expected.sort();
    assert_eq!(selected, expected);
}

#[test]
fn dragging_a_layer_moves_it_by_the_pointer_delta() {
    let rect = rect_layer("A", 100.0, 100.0, 100.0, 80.0);
    let id = rect.id;
    let mut h = Harness::new(vec![rect]);
    h.drag(Pos2::new(150.0, 140.0), Pos2::new(230.0, 190.0));
    assert_eq!(h.selection, vec![id]);
    assert_eq!(h.layer(id).frame.pos, Pos2::new(180.0, 150.0));
}

#[test]
fn dragging_the_bottom_right_handle_resizes_the_selected_layer() {
    let rect = rect_layer("A", 100.0, 100.0, 100.0, 80.0);
    let id = rect.id;
    let mut h = Harness::new(vec![rect]);
    h.click(Pos2::new(150.0, 140.0), Modifiers::NONE);
    h.drag(Pos2::new(200.0, 180.0), Pos2::new(250.0, 220.0));
    let bounds = h.layer(id).frame.bounds();
    assert_eq!(bounds.min, Pos2::new(100.0, 100.0));
    assert_eq!(bounds.size(), Vec2::new(150.0, 120.0));
}

#[test]
fn rectangle_tool_drag_creates_a_rectangle_and_switches_back_to_select() {
    let mut h = Harness::new(Vec::new());
    h.tool = Tool::Rectangle;
    h.drag(Pos2::new(300.0, 300.0), Pos2::new(400.0, 350.0));
    let page = h.history.get().active_page();
    assert_eq!(page.layers.len(), 1);
    let created = &page.layers[0];
    assert!(matches!(created.kind, LayerKind::Rectangle { .. }));
    assert_eq!(created.frame.bounds(), Rect::from_min_size(Pos2::new(300.0, 300.0), Vec2::new(100.0, 50.0)));
    assert_eq!(h.selection, vec![created.id]);
    assert_eq!(h.tool, Tool::Select);
}

#[test]
fn dragging_out_of_the_top_ruler_creates_a_horizontal_guide() {
    let mut h = Harness::new(Vec::new());
    let from = Pos2::new(300.0, RULER_SIZE / 2.0);
    let to = h.screen(Pos2::new(280.0, 200.0));
    h.drag_screen(from, to);
    let guides = &h.history.get().active_page().guides;
    assert_eq!(guides.len(), 1);
    assert_eq!(guides[0].orientation, GuideOrientation::Horizontal);
    assert_eq!(guides[0].pos, 200.0);
}

#[test]
fn pen_clicks_then_enter_commit_an_open_path() {
    let mut h = Harness::new(Vec::new());
    h.tool = Tool::Pen;
    for doc in [Pos2::new(100.0, 100.0), Pos2::new(200.0, 100.0), Pos2::new(200.0, 200.0)] {
        h.click(doc, Modifiers::NONE);
    }
    h.key(Key::Enter);
    let page = h.history.get().active_page();
    assert_eq!(page.layers.len(), 1);
    let LayerKind::Path { points, closed } = &page.layers[0].kind else {
        panic!("expected a Path layer, got {:?}", page.layers[0].kind);
    };
    assert_eq!(points.len(), 3);
    assert!(!closed);
}

#[test]
fn double_clicking_a_text_layer_starts_in_place_editing() {
    let frame = Frame { pos: Pos2::new(100.0, 100.0), size: Vec2::new(140.0, 34.0), rotation: 0.0 };
    let text = new_layer_for_tool(Tool::Text, frame).expect("Text tool creates a layer");
    let id = text.id;
    let mut h = Harness::new(vec![text]);
    h.double_click(Pos2::new(150.0, 115.0));
    assert_eq!(h.widget.editing_text, Some(id));
    assert_eq!(h.selection, vec![id]);
}

#[test]
fn double_clicking_a_mask_over_text_starts_editing_the_masked_text() {
    let frame = Frame { pos: Pos2::new(100.0, 100.0), size: Vec2::new(140.0, 34.0), rotation: 0.0 };
    let text = new_layer_for_tool(Tool::Text, frame).expect("Text tool creates a layer");
    let id = text.id;
    let mut mask = rect_layer("Mask", 90.0, 90.0, 160.0, 54.0);
    mask.mask.enabled = true;
    let mask_id = mask.id;
    let mut h = Harness::new(vec![text, mask]);

    // A single click selects the mask, which sits on top.
    h.click(Pos2::new(150.0, 115.0), Modifiers::NONE);
    assert_eq!(h.selection, vec![mask_id]);

    // Let egui's double-click window lapse so the single click above isn't
    // counted as the first half of the double-click below.
    for _ in 0..10 {
        h.frame(Vec::new());
    }
    h.double_click(Pos2::new(150.0, 115.0));
    assert_eq!(h.widget.editing_text, Some(id));
    assert_eq!(h.selection, vec![id]);
}

#[test]
fn right_click_on_the_canvas_opens_the_copy_paste_menu() {
    let mut h = Harness::new(Vec::new());
    let pos = h.screen(Pos2::new(300.0, 300.0));
    h.click_screen(pos, PointerButton::Secondary, Modifiers::NONE);
    assert_eq!(h.widget.canvas_menu, Some(pos));
    assert!(h.widget.overlap_menu.is_none());
}

#[test]
fn shift_right_click_lists_every_layer_stacked_under_the_pointer() {
    let below = rect_layer("Below", 100.0, 100.0, 100.0, 100.0);
    let above = rect_layer("Above", 150.0, 150.0, 100.0, 100.0);
    let mut h = Harness::new(vec![below, above]);
    let pos = h.screen(Pos2::new(175.0, 175.0));
    h.click_screen(pos, PointerButton::Secondary, Modifiers::SHIFT);
    let (_, hits) = h.widget.overlap_menu.clone().expect("overlap menu open");
    assert_eq!(hits.len(), 2);
    assert!(h.widget.canvas_menu.is_none());
}

#[test]
fn pinch_zoom_keeps_the_point_under_the_pointer_fixed() {
    let mut h = Harness::new(Vec::new());
    let doc_point = Pos2::new(300.0, 200.0);
    let pointer = h.screen(doc_point);
    h.frame(vec![Event::PointerMoved(pointer)]);
    h.frame(vec![Event::Zoom(2.0)]);
    assert_eq!(h.widget.zoom, 2.0);
    let after = h.screen(doc_point);
    assert!((after - pointer).length() < 0.01, "{doc_point:?} moved from {pointer:?} to {after:?}");
}
