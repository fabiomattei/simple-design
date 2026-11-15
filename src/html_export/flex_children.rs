//! Flexbox layout mode's handling of a container's children: decides which
//! children can flow (`plan_children`), then emits the inferred rows and
//! columns as wrapper `<div>`s, with the remaining children absolutely
//! positioned on top (`SiteBuilder::add_planned_children`).
//!
//! Every child gets `z-index` from its layer order, because flowed items are
//! emitted in visual order, not layer order, and positioned elements would
//! otherwise paint in DOM order. Flowed items are `position: relative` so
//! that `z-index` applies to them too.

use super::element_tree::{Placement, SiteBuilder};
use super::layout_inference::{infer_layout, Axis, InferredLayout, LayoutNode};
use super::raster_fallback::is_css_expressible;
use super::stack_spacing::{stack_spacing, CrossAlign};
use crate::model::{Layer, LayerKind};

/// Which of a container's children flow and how; built before the
/// container's own CSS rule, which needs `container_declarations`.
pub struct ChildPlan {
    layout: InferredLayout,
    /// Children that can't flow at all (see `can_flow`), plus the ones
    /// inference set aside — in that order.
    absolute: Vec<usize>,
}

/// Plans `children`'s layout. Pure.
pub fn plan_children(children: &[Layer]) -> ChildPlan {
    let visible: Vec<usize> = (0..children.len()).filter(|&i| children[i].visible).collect();
    let (flowing, mut absolute): (Vec<usize>, Vec<usize>) = visible.into_iter().partition(|&i| can_flow(&children[i]));
    let items: Vec<_> = flowing.iter().map(|&i| (i, children[i].frame.bounds())).collect();
    let layout = infer_layout(&items);
    absolute.extend(&layout.absolute);
    ChildPlan { layout, absolute }
}

/// Rasterized, rotated, and line layers keep their exact absolute placement;
/// so does anything sticking out past the container's top or left edge,
/// which flex padding can't reach (it can't be negative).
fn can_flow(layer: &Layer) -> bool {
    let bounds = layer.frame.bounds();
    is_css_expressible(layer)
        && layer.frame.rotation == 0.0
        && !matches!(layer.kind, LayerKind::Line | LayerKind::Arrow { .. })
        && bounds.min.x >= 0.0
        && bounds.min.y >= 0.0
}

impl ChildPlan {
    /// The container's own extra declarations: a flex row whose padding puts
    /// the flowed content where it was drawn.
    pub fn container_declarations(&self) -> Vec<String> {
        let Some(flow) = &self.layout.flow else {
            return Vec::new();
        };
        let start = flow.bounds().min;
        vec!["display: flex;".to_string(), "align-items: flex-start;".to_string(), format!("padding: {} 0 0 {};", px(start.y), px(start.x))]
    }
}

impl SiteBuilder<'_> {
    pub(super) fn add_planned_children(&mut self, plan: &ChildPlan, children: &[Layer], depth: usize) -> anyhow::Result<()> {
        if let Some(flow) = &plan.layout.flow {
            self.add_flow_node(flow, children, depth, Vec::new())?;
        }
        for &index in &plan.absolute {
            self.add_layer(&children[index], depth, Placement::Absolute, Some(index))?;
        }
        Ok(())
    }

    /// `margins` are the node's own, from its parent stack's spacing.
    fn add_flow_node(&mut self, node: &LayoutNode, children: &[Layer], depth: usize, margins: Vec<String>) -> anyhow::Result<()> {
        match node {
            LayoutNode::Item { index, .. } => {
                self.add_layer(&children[*index], depth, Placement::InFlow { margins }, Some(*index))?;
                Ok(())
            }
            LayoutNode::Stack { axis, bounds, children: nodes } => self.add_stack(*axis, *bounds, nodes, children, depth, margins),
        }
    }

    /// One wrapper `<div>` for an inferred row or column, then its nodes.
    fn add_stack(&mut self, axis: Axis, bounds: egui::Rect, nodes: &[LayoutNode], children: &[Layer], depth: usize, margins: Vec<String>) -> anyhow::Result<()> {
        let child_bounds: Vec<_> = nodes.iter().map(LayoutNode::bounds).collect();
        let spacing = stack_spacing(axis, bounds, &child_bounds);
        let class = format!("stack-{}", self.next_stack_id);
        self.next_stack_id += 1;

        let (direction, label) = match axis {
            Axis::Row => ("row", "row"),
            Axis::Column => ("column", "column"),
        };
        let align = match spacing.align {
            CrossAlign::Start => "flex-start",
            CrossAlign::Center => "center",
            CrossAlign::End => "flex-end",
        };
        let mut declarations = vec!["display: flex;".to_string(), format!("flex-direction: {direction};"), format!("align-items: {align};")];
        if let Some(gap) = spacing.gap {
            declarations.push(format!("gap: {};", px(gap)));
        }
        declarations.extend(margins);
        self.push_rule(&class, label, &declarations);

        let indent = " ".repeat(depth);
        self.body.push_str(&format!("{indent}<div class=\"{class}\">\n"));
        for (i, child) in nodes.iter().enumerate() {
            let child_margins = margin_declarations(axis, spacing.main_margins[i], spacing.cross_margins[i]);
            self.add_flow_node(child, children, depth + 1, child_margins)?;
        }
        self.body.push_str(&format!("{indent}</div>\n"));
        Ok(())
    }
}

fn margin_declarations(axis: Axis, main: f32, cross: f32) -> Vec<String> {
    let (main_side, cross_side) = match axis {
        Axis::Row => ("margin-left", "margin-top"),
        Axis::Column => ("margin-top", "margin-left"),
    };
    [(main_side, main), (cross_side, cross)]
        .into_iter()
        .filter(|(_, value)| *value != 0.0)
        .map(|(side, value)| format!("{side}: {};", px(value)))
        .collect()
}

fn px(value: f32) -> String {
    if value == 0.0 { "0".to_string() } else { format!("{}px", (value * 100.0).round() / 100.0) }
}
