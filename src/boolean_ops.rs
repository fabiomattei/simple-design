//! Boolean shape operations (Union / Subtract / Intersect / Difference /
//! Add), analogous to a "Combine" menu.
//!
//! The live path (`create_boolean_group`/`compute_boolean_group`) is what's
//! wired up to the UI: it builds/renders a `LayerKind::BooleanGroup` whose
//! `children` stay independently editable, recomputing the combined shape
//! from scratch on every call rather than caching it. The closest existing
//! precedent, `CompoundPath` rendering, already re-triangulates from
//! scratch every frame with no caching (`canvas.rs`'s `draw_even_odd_polygons`
//! call sites); a `geo` boolean-op fold over a handful of shapes is smaller
//! in cost than that, for the simple-shape complexity this app targets. If
//! profiling ever shows this to be a real cost (e.g. a `BooleanGroup` built
//! from an already-huge flattened `CompoundPath`), `MaskedGroupTextureCache`
//! (`canvas.rs`) is the proven cache-by-content-clone pattern to reach for.
//!
//! The older, destructive path (`apply_boolean_op`) flattens selected shapes
//! into a single new `CompoundPath` layer, discarding the originals, instead
//! of building a live group. It's no longer wired to any UI entry point
//! (superseded by the live `BooleanGroup` path above) but is kept working
//! for its test coverage and as a building block for a possible future
//! generic "Flatten" command.
//!
//! Both paths flatten shapes to polygons and combine them via `geo`'s
//! `BooleanOps`.

use egui::{Pos2, Rect, Vec2};
use geo::{BooleanOps, Contains, Coord, LineString, MultiPolygon, Point, Polygon};

use crate::canvas::flatten_path;
use crate::grouping::find_common_parent_list;
use crate::model::{BoolOp, Frame, Layer, LayerId, LayerKind, Page, PathPolygon, Style};
use crate::shapes::{ellipse_points, rotate_point, rounded_rect_points};

/// Combines the layers in `ids` (which must all be direct siblings — the
/// page itself, or some artboard/group's children, like `grouping::group_layers`
/// requires) using `op`, replacing them with one new `CompoundPath` layer at
/// the frontmost removed layer's position. Only shapes with fillable area
/// (`Rectangle`, `Oval`, a closed `Path`, or another `CompoundPath`)
/// participate; if the selection includes anything else, or the result is
/// empty (e.g. an `Intersect` of non-overlapping shapes), nothing is mutated
/// and `None` is returned.
///
/// Not wired to any UI entry point — superseded by the live
/// `create_boolean_group` below — kept for its test coverage and as a
/// building block for a possible future generic "Flatten" command.
#[allow(dead_code)]
pub fn apply_boolean_op(page: &mut Page, ids: &[LayerId], op: BoolOp) -> Option<LayerId> {
    if ids.len() < 2 {
        return None;
    }
    let parent = find_common_parent_list(&mut page.layers, ids)?;

    // Preserve z-order (Vec order = back-to-front, matching layers_panel.rs
    // and grouping.rs) rather than selection order.
    let ordered: Vec<&Layer> = parent.iter().filter(|l| ids.contains(&l.id)).collect();
    if ordered.len() != ids.len() {
        return None;
    }
    let polys: Vec<MultiPolygon<f64>> = ordered
        .iter()
        .map(|l| flatten_layer(l))
        .collect::<Option<Vec<_>>>()?;
    let style = ordered.last()?.style.clone();

    let result = combine(op, &polys);
    if result.0.is_empty() {
        return None;
    }
    let new_layer = compound_path_layer(op.label(), style, result);

    let mut insert_at = 0;
    let mut i = 0;
    while i < parent.len() {
        if ids.contains(&parent[i].id) {
            insert_at = insert_at.max(i);
            parent.remove(i);
        } else {
            i += 1;
        }
    }
    insert_at = insert_at.min(parent.len());
    let id = new_layer.id;
    parent.insert(insert_at, new_layer);
    Some(id)
}

#[allow(dead_code)]
fn combine(op: BoolOp, polys: &[MultiPolygon<f64>]) -> MultiPolygon<f64> {
    let (first, rest) = polys.split_first().expect("apply_boolean_op requires >= 2 shapes");
    rest.iter().fold(first.clone(), |acc, p| combine_step(op, acc, p))
}

/// Combines one more operand (`next`) onto an accumulated result (`acc`)
/// using `op` — the single per-pair step both the destructive `combine`
/// (uniform op across a whole selection) and the live `compute_boolean_group`
/// (a potentially different op per child) fold with.
fn combine_step(op: BoolOp, acc: MultiPolygon<f64>, next: &MultiPolygon<f64>) -> MultiPolygon<f64> {
    match op {
        BoolOp::Union => acc.union(next),
        BoolOp::Intersect => acc.intersection(next),
        BoolOp::Difference => acc.xor(next),
        BoolOp::Subtract => acc.difference(next),
        // "Add": literal concatenation, no clipping — see `BoolOp::Add`'s
        // doc comment for the resulting even-odd-fill edge case with
        // overlapping operands.
        BoolOp::Add => MultiPolygon::new(acc.0.into_iter().chain(next.0.iter().cloned()).collect()),
    }
}

/// Flattens a single layer's fill geometry to parent-local-space polygons.
/// Siblings share the same parent, so a layer's own `frame.pos`/`frame.bounds()`
/// is already in the right coordinate space — no ancestor-offset walk needed
/// (unlike `Page::absolute_offset`, which is for non-sibling lookups).
/// Returns `None` for shape kinds with no fillable area (`Line`, `Text`,
/// `Group`, `Artboard`) — those can't participate in a boolean op.
fn flatten_layer(layer: &Layer) -> Option<MultiPolygon<f64>> {
    let center = layer.frame.bounds().center();
    let rotation = layer.frame.rotation;
    let rotated = |pts: Vec<Pos2>| -> Vec<Pos2> { pts.into_iter().map(|p| rotate_point(p, center, rotation)).collect() };
    match &layer.kind {
        LayerKind::Rectangle { corner_radius } => {
            let ring = ring_from_points(&rotated(rounded_rect_points(layer.frame.bounds(), corner_radius.as_array())));
            Some(MultiPolygon::new(vec![Polygon::new(ring, vec![])]))
        }
        LayerKind::Oval => {
            let b = layer.frame.bounds();
            let pts = ellipse_points(b.center(), b.width() / 2.0, b.height() / 2.0);
            Some(MultiPolygon::new(vec![Polygon::new(ring_from_points(&rotated(pts)), vec![])]))
        }
        LayerKind::Star { points, inner_ratio } => {
            let b = layer.frame.bounds();
            let pts = crate::shapes::star_points(b.center(), b.width() / 2.0, b.height() / 2.0, *points, *inner_ratio);
            Some(MultiPolygon::new(vec![Polygon::new(ring_from_points(&rotated(pts)), vec![])]))
        }
        LayerKind::Polygon { sides } => {
            let b = layer.frame.bounds();
            let pts = crate::shapes::polygon_points(b.center(), b.width() / 2.0, b.height() / 2.0, *sides);
            Some(MultiPolygon::new(vec![Polygon::new(ring_from_points(&rotated(pts)), vec![])]))
        }
        LayerKind::Path { points, closed } => {
            if !*closed || points.len() < 3 {
                return None;
            }
            let offset = layer.frame.pos.to_vec2();
            let pts: Vec<Pos2> = flatten_path(points, true).into_iter().map(|p| p + offset).collect();
            Some(MultiPolygon::new(vec![Polygon::new(ring_from_points(&rotated(pts)), vec![])]))
        }
        LayerKind::CompoundPath { polygons } => {
            let offset = layer.frame.pos.to_vec2();
            let polys = polygons
                .iter()
                .map(|p| {
                    Polygon::new(
                        ring_from_points(&rotated(translated(&p.exterior, offset))),
                        p.holes.iter().map(|h| ring_from_points(&rotated(translated(h, offset)))).collect(),
                    )
                })
                .collect();
            Some(MultiPolygon::new(polys))
        }
        LayerKind::BooleanGroup { children } => {
            let inner = compute_boolean_group(children);
            let offset = layer.frame.pos.to_vec2();
            let polys = inner
                .0
                .iter()
                .map(|poly| {
                    Polygon::new(
                        ring_from_points(&rotated(translated(&ring_to_points(poly.exterior()), offset))),
                        poly.interiors()
                            .iter()
                            .map(|h| ring_from_points(&rotated(translated(&ring_to_points(h), offset))))
                            .collect(),
                    )
                })
                .collect();
            Some(MultiPolygon::new(polys))
        }
        LayerKind::Artboard { .. }
        | LayerKind::Group { .. }
        | LayerKind::Line
        | LayerKind::Arrow { .. }
        | LayerKind::Text { .. }
        | LayerKind::Image { .. } => None,
    }
}

/// Computes the live combined geometry of a `BooleanGroup`'s `children`, in
/// the same coordinate space `PathPolygon` fields use relative to their
/// owning layer's `frame.pos` (children's own `frame.pos` are already
/// relative to the `BooleanGroup`'s own frame, same convention as `Group`'s
/// children — see `model/layer.rs`'s coordinate-system notes — so flattening
/// them with no extra offset lands directly in that space).
///
/// Recomputed from scratch on every call — no caching — see this module's
/// doc comment for why that's an acceptable tradeoff here.
///
/// Only `visible` children with fillable area participate (same restriction
/// `flatten_layer` already has: `Rectangle`/`Oval`/`Star`/`Polygon`/closed
/// `Path`/`CompoundPath`/nested `BooleanGroup` only). The bottommost eligible
/// child (by z-order, i.e. the first one found in `children`) is the base;
/// every other eligible child folds onto the accumulator via its own
/// `Layer::bool_op` (`combine_step`). Returns an empty `MultiPolygon` if no
/// eligible visible child exists.
pub fn compute_boolean_group(children: &[Layer]) -> MultiPolygon<f64> {
    let mut eligible = children.iter().filter(|c| c.visible).filter_map(|c| flatten_layer(c).map(|p| (c.bool_op, p)));
    let Some((_, base)) = eligible.next() else {
        return MultiPolygon::new(Vec::new());
    };
    eligible.fold(base, |acc, (op, next)| combine_step(op, acc, &next))
}

/// Converts a computed/combined `MultiPolygon` into the same ring
/// representation `CompoundPath::polygons` stores, so a live `BooleanGroup`
/// result can be rendered through the exact same even-odd-fill code path as
/// a `CompoundPath` (`canvas.rs`/`export.rs`).
pub fn multipolygon_to_polygons(mp: &MultiPolygon<f64>) -> Vec<PathPolygon> {
    mp.0.iter()
        .map(|poly| PathPolygon {
            exterior: ring_to_points(poly.exterior()),
            holes: poly.interiors().iter().map(ring_to_points).collect(),
        })
        .collect()
}

/// Exact point-in-`MultiPolygon` test backing `canvas.rs`'s opaque
/// `BooleanGroup` hit-testing — `point` must already be in the same
/// coordinate space `mp` is in (see `compute_boolean_group`'s doc comment).
/// Uses `geo::Contains` directly rather than `masking::mask_covers_point`'s
/// rasterize-and-sample approach, since a boolean-op result is already a
/// clean `MultiPolygon` (a mask's coverage is arbitrary rendered alpha,
/// which isn't).
///
/// Note: for `BoolOp::Add`, whose operands can genuinely overlap (see its
/// doc comment), `Contains` reports a hit anywhere covered by *any* piece —
/// including the overlap region, even though that region renders as an
/// even-odd "hole." This is a narrow, intentional consequence of `Add`'s
/// "no clipping" semantics, not a bug.
pub fn point_in_multipolygon(mp: &MultiPolygon<f64>, p: Pos2) -> bool {
    mp.contains(&Point::new(p.x as f64, p.y as f64))
}

/// Creates a new `LayerKind::BooleanGroup` from `ids` (which must be direct
/// siblings, like `grouping::group_layers` requires), replacing them with
/// one new container at the frontmost removed layer's position — mirrors
/// `grouping::group_layers`'s z-order-preserving remove/reinsert, but
/// (unlike `group_layers`) also assigns `op` to every non-base child's
/// `bool_op`. The bottommost (z-order-first) selected layer becomes the
/// base — its `bool_op` is left at whatever it already was, since
/// `compute_boolean_group` never reads it. Validates every operand is a
/// fillable shape kind (same rule as `flatten_layer`) *before* mutating
/// anything, so an ineligible selection (e.g. containing a `Line`) aborts
/// with no partial mutation.
pub fn create_boolean_group(page: &mut Page, ids: &[LayerId], op: BoolOp) -> Option<LayerId> {
    if ids.len() < 2 {
        return None;
    }
    let parent = find_common_parent_list(&mut page.layers, ids)?;

    let ordered: Vec<&Layer> = parent.iter().filter(|l| ids.contains(&l.id)).collect();
    if ordered.len() != ids.len() || !ordered.iter().all(|l| flatten_layer(l).is_some()) {
        return None;
    }

    let mut removed = Vec::new();
    let mut insert_at = 0;
    let mut i = 0;
    while i < parent.len() {
        if ids.contains(&parent[i].id) {
            insert_at = insert_at.max(i);
            removed.push(parent.remove(i));
        } else {
            i += 1;
        }
    }
    insert_at = insert_at.min(parent.len());

    for (idx, child) in removed.iter_mut().enumerate() {
        if idx > 0 {
            child.bool_op = op;
        }
    }

    // `rotated_bounds()`, not `bounds()`, so the new group's own frame
    // tightly wraps each child's actual visual footprint — same reasoning
    // as `grouping::group_layers`.
    let bbox = removed.iter().map(|l| l.frame.rotated_bounds()).reduce(|a, b| a.union(b))?;
    for child in &mut removed {
        child.frame.pos -= bbox.min.to_vec2();
    }

    let group = Layer::new(op.label(), Frame::from_bounds(bbox), LayerKind::BooleanGroup { children: removed });
    let id = group.id;
    parent.insert(insert_at, group);
    Some(id)
}

fn translated(points: &[Pos2], offset: Vec2) -> Vec<Pos2> {
    points.iter().map(|p| *p + offset).collect()
}

/// Builds a closed `geo::LineString` from a ring of points, auto-closing it
/// (duplicating the first point at the end) if the caller hasn't already —
/// `flatten_path`'s closed output already duplicates it; `ellipse_points`/
/// `rounded_rect_points`/stored `PathPolygon` rings don't.
fn ring_from_points(points: &[Pos2]) -> LineString<f64> {
    let mut coords: Vec<Coord<f64>> = points.iter().map(|p| Coord { x: p.x as f64, y: p.y as f64 }).collect();
    if let (Some(first), Some(last)) = (coords.first().copied(), coords.last().copied()) {
        if (first.x - last.x).abs() > 1e-9 || (first.y - last.y).abs() > 1e-9 {
            coords.push(first);
        }
    }
    LineString::new(coords)
}

/// Inverse of `ring_from_points`: drops the closing duplicate point, matching
/// the non-duplicated-endpoint convention `PathPoint`/`PathPolygon` use.
fn ring_to_points(ls: &LineString<f64>) -> Vec<Pos2> {
    let mut pts: Vec<Pos2> = ls.0.iter().map(|c| Pos2::new(c.x as f32, c.y as f32)).collect();
    if pts.len() >= 2 {
        let (first, last) = (pts[0], pts[pts.len() - 1]);
        if (first.x - last.x).abs() < 1e-6 && (first.y - last.y).abs() < 1e-6 {
            pts.pop();
        }
    }
    pts
}

#[allow(dead_code)]
fn compound_path_layer(name: &str, style: Style, mp: MultiPolygon<f64>) -> Layer {
    let polygons = multipolygon_to_polygons(&mp);

    let all_points: Vec<Pos2> = polygons
        .iter()
        .flat_map(|p| p.exterior.iter().copied().chain(p.holes.iter().flatten().copied()))
        .collect();
    let bounds = Rect::from_points(&all_points);
    let frame_pos = bounds.min;

    let polygons: Vec<PathPolygon> = polygons
        .into_iter()
        .map(|p| PathPolygon {
            exterior: p.exterior.into_iter().map(|pt| pt - frame_pos.to_vec2()).collect(),
            holes: p
                .holes
                .into_iter()
                .map(|h| h.into_iter().map(|pt| pt - frame_pos.to_vec2()).collect())
                .collect(),
        })
        .collect();

    let frame = Frame::from_bounds(bounds);
    let mut layer = Layer::new(name, frame, LayerKind::CompoundPath { polygons });
    layer.style = style;
    layer
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{CornerRadii, Paint};

    fn rect(name: &str, pos: Pos2, size: Vec2) -> Layer {
        Layer::new(name, Frame { pos, size, rotation: 0.0 }, LayerKind::Rectangle { corner_radius: CornerRadii::ZERO })
    }

    #[test]
    fn compute_boolean_group_unions_two_overlapping_rects() {
        let a = rect("A", Pos2::new(0.0, 0.0), Vec2::new(40.0, 40.0));
        let mut b = rect("B", Pos2::new(20.0, 20.0), Vec2::new(40.0, 40.0));
        b.bool_op = BoolOp::Union;

        let combined = compute_boolean_group(&[a, b]);

        assert!(point_in_multipolygon(&combined, Pos2::new(5.0, 5.0)), "in-A-only point should be covered");
        assert!(point_in_multipolygon(&combined, Pos2::new(55.0, 55.0)), "in-B-only point should be covered");
        assert!(point_in_multipolygon(&combined, Pos2::new(25.0, 25.0)), "overlap point should be covered");
        assert!(!point_in_multipolygon(&combined, Pos2::new(2.0, 58.0)), "point outside both should not be covered");
    }

    #[test]
    fn compute_boolean_group_subtract_punches_a_hole() {
        let base = rect("Base", Pos2::new(0.0, 0.0), Vec2::new(60.0, 60.0));
        let mut hole = rect("Hole", Pos2::new(20.0, 20.0), Vec2::new(20.0, 20.0));
        hole.bool_op = BoolOp::Subtract;

        let combined = compute_boolean_group(&[base, hole]);

        assert!(!point_in_multipolygon(&combined, Pos2::new(30.0, 30.0)), "subtracted region should be a real hole");
        assert!(point_in_multipolygon(&combined, Pos2::new(5.0, 5.0)), "rest of the base should remain covered");
    }

    #[test]
    fn compute_boolean_group_intersect_keeps_only_the_overlap() {
        let a = rect("A", Pos2::new(0.0, 0.0), Vec2::new(40.0, 40.0));
        let mut b = rect("B", Pos2::new(20.0, 20.0), Vec2::new(40.0, 40.0));
        b.bool_op = BoolOp::Intersect;

        let combined = compute_boolean_group(&[a, b]);

        assert!(point_in_multipolygon(&combined, Pos2::new(25.0, 25.0)), "overlap point should be covered");
        assert!(!point_in_multipolygon(&combined, Pos2::new(5.0, 5.0)), "in-A-only point should not be covered");
        assert!(!point_in_multipolygon(&combined, Pos2::new(55.0, 55.0)), "in-B-only point should not be covered");
    }

    #[test]
    fn compute_boolean_group_add_concatenates_without_clipping() {
        let a = rect("A", Pos2::new(0.0, 0.0), Vec2::new(40.0, 40.0));
        let mut b = rect("B", Pos2::new(20.0, 20.0), Vec2::new(40.0, 40.0));
        b.bool_op = BoolOp::Add;

        let combined = compute_boolean_group(&[a, b]);

        // No clipping happened: two disjoint pieces, unlike Union's one.
        assert_eq!(combined.0.len(), 2, "Add should keep operands as separate, unclipped pieces");
    }

    #[test]
    fn compute_boolean_group_ignores_hidden_children() {
        let a = rect("A", Pos2::new(0.0, 0.0), Vec2::new(40.0, 40.0));
        let mut b = rect("B", Pos2::new(60.0, 0.0), Vec2::new(20.0, 20.0));
        b.bool_op = BoolOp::Union;
        b.visible = false;

        let combined = compute_boolean_group(&[a, b]);

        assert!(point_in_multipolygon(&combined, Pos2::new(5.0, 5.0)), "visible base should still be covered");
        assert!(!point_in_multipolygon(&combined, Pos2::new(65.0, 5.0)), "hidden child should not contribute");
    }

    #[test]
    fn compute_boolean_group_ignores_the_base_childs_own_bool_op() {
        // children[0] is always the base — its `bool_op`, even if set to
        // something meaningful-looking, must never be read.
        let mut a = rect("A", Pos2::new(0.0, 0.0), Vec2::new(40.0, 40.0));
        a.bool_op = BoolOp::Subtract;
        let mut b = rect("B", Pos2::new(20.0, 20.0), Vec2::new(40.0, 40.0));
        b.bool_op = BoolOp::Union;

        let combined = compute_boolean_group(&[a, b]);

        assert!(point_in_multipolygon(&combined, Pos2::new(5.0, 5.0)), "should union, not subtract, ignoring base's own bool_op");
    }

    #[test]
    fn flatten_layer_handles_a_nested_boolean_group_rotated_and_offset() {
        let a = rect("A", Pos2::new(0.0, 0.0), Vec2::new(40.0, 40.0));
        let mut b = rect("B", Pos2::new(20.0, 20.0), Vec2::new(40.0, 40.0));
        b.bool_op = BoolOp::Union;
        let bbox = Rect::from_two_pos(Pos2::new(0.0, 0.0), Pos2::new(60.0, 60.0));

        let inner_group =
            Layer::new("Inner", Frame::from_bounds(bbox), LayerKind::BooleanGroup { children: vec![a, b] });

        // Nest the group as a plain (non-rotated) child of an outer container
        // offset by (100, 50) — `flatten_layer`'s `BooleanGroup` arm should
        // translate the inner group's own computed geometry by that offset.
        let mut outer = inner_group.clone();
        outer.frame.pos = Pos2::new(100.0, 50.0);

        let flattened = flatten_layer(&outer).expect("nested BooleanGroup should flatten");
        let offset = outer.frame.pos.to_vec2();
        assert!(
            point_in_multipolygon(&flattened, Pos2::new(5.0, 5.0) + offset),
            "flattened geometry should be translated by the outer layer's own frame.pos"
        );
        assert!(
            !point_in_multipolygon(&flattened, Pos2::new(5.0, 5.0)),
            "untranslated point should no longer be covered"
        );
    }

    #[test]
    fn create_boolean_group_wraps_originals_as_still_editable_children() {
        let mut page = Page::new("Test");
        let mut a = rect("A", Pos2::new(0.0, 0.0), Vec2::new(40.0, 40.0));
        a.style.fill = Some(Paint::Solid(egui::Color32::from_rgb(200, 0, 0)));
        let a_id = a.id;
        let b = rect("B", Pos2::new(20.0, 20.0), Vec2::new(40.0, 40.0));
        let b_id = b.id;
        page.layers.push(a);
        page.layers.push(b);

        let group_id = create_boolean_group(&mut page, &[a_id, b_id], BoolOp::Subtract)
            .expect("valid selection should produce a group");

        assert_eq!(page.layers.len(), 1);
        let group = page.find(group_id).expect("group should be findable");
        let LayerKind::BooleanGroup { children } = &group.kind else { panic!("expected a BooleanGroup") };
        assert_eq!(children.len(), 2);
        // Base (z-order-first) child's bool_op is irrelevant/untouched; every
        // other child gets the chosen op.
        assert_eq!(children[1].bool_op, BoolOp::Subtract);
        // Original per-layer state (e.g. style) survives — nothing flattened.
        assert_eq!(children[0].style.fill, Some(Paint::Solid(egui::Color32::from_rgb(200, 0, 0))));
    }

    #[test]
    fn create_boolean_group_aborts_without_mutating_on_an_ineligible_operand() {
        let mut page = Page::new("Test");
        let a = rect("A", Pos2::new(0.0, 0.0), Vec2::new(40.0, 40.0));
        let a_id = a.id;
        let line = Layer::new(
            "Line",
            Frame { pos: Pos2::new(0.0, 0.0), size: Vec2::new(10.0, 10.0), rotation: 0.0 },
            LayerKind::Line,
        );
        let line_id = line.id;
        page.layers.push(a);
        page.layers.push(line);

        let result = create_boolean_group(&mut page, &[a_id, line_id], BoolOp::Union);

        assert!(result.is_none(), "a Line can't participate in a boolean op");
        assert_eq!(page.layers.len(), 2, "selection should be left untouched on abort");
    }
}
