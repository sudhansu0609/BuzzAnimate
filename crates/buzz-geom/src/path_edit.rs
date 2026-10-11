//! Editing individual anchor points on a path.
//!
//! This is what Animate's Subselection tool does: show a path's anchors, let
//! the user drag one, and keep the surrounding curve attached to it.
//!
//! # What "moving an anchor" has to mean
//!
//! A naive implementation moves only the anchor point and leaves the Bézier
//! control handles where they were, which visibly kinks the curve on both
//! sides. Moving the adjacent handles by the same delta keeps the tangents
//! intact, so the curve slides rather than deforming — which is what the user
//! expects and what Animate does.

use kurbo::{BezPath, ParamCurveNearest, PathEl, PathSeg, Point, Vec2};

/// An editable anchor on a path.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Anchor {
    /// Index into `BezPath::elements`.
    pub element: usize,
    pub point: Point,
}

/// Every on-curve anchor, in path order.
///
/// Control points are deliberately excluded: Animate's Subselection tool shows
/// handles only for the anchor currently being edited, and listing them here
/// would make them all selectable at once.
pub fn anchors(path: &BezPath) -> Vec<Anchor> {
    path.elements()
        .iter()
        .enumerate()
        .filter_map(|(element, el)| {
            let point = match el {
                PathEl::MoveTo(p) | PathEl::LineTo(p) => *p,
                PathEl::QuadTo(_, p) | PathEl::CurveTo(_, _, p) => *p,
                PathEl::ClosePath => return None,
            };
            Some(Anchor { element, point })
        })
        .collect()
}

/// The anchor nearest `point`, if one is within `tolerance`.
pub fn nearest_anchor(path: &BezPath, point: Point, tolerance: f64) -> Option<Anchor> {
    anchors(path)
        .into_iter()
        .map(|a| (a, (a.point - point).hypot()))
        .filter(|(_, d)| *d <= tolerance)
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(a, _)| a)
}

/// Move the anchor at `element` by `delta`, carrying its handles with it.
///
/// Returns false if the index does not name an anchor.
pub fn move_anchor(path: &mut BezPath, element: usize, delta: Vec2) -> bool {
    if !delta.x.is_finite() || !delta.y.is_finite() {
        return false;
    }

    let elements = path.elements().to_vec();
    let Some(target) = elements.get(element) else {
        return false;
    };
    if matches!(target, PathEl::ClosePath) {
        return false;
    }

    let mut updated = elements.clone();

    // The anchor itself, plus the incoming control point that shapes the curve
    // arriving at it.
    updated[element] = match elements[element] {
        PathEl::MoveTo(p) => PathEl::MoveTo(p + delta),
        PathEl::LineTo(p) => PathEl::LineTo(p + delta),
        PathEl::QuadTo(c, p) => PathEl::QuadTo(c + delta, p + delta),
        PathEl::CurveTo(c1, c2, p) => PathEl::CurveTo(c1, c2 + delta, p + delta),
        PathEl::ClosePath => return false,
    };

    // The outgoing control point lives on the *next* element, and must follow
    // too or the curve leaving this anchor will kink.
    if let Some(next) = elements.get(element + 1) {
        updated[element + 1] = match *next {
            PathEl::QuadTo(c, p) => PathEl::QuadTo(c + delta, p),
            PathEl::CurveTo(c1, c2, p) => PathEl::CurveTo(c1 + delta, c2, p),
            other => other,
        };
    }

    // A closed subpath's first and last anchors are the same point on screen,
    // so moving one must move the other or the shape tears open.
    if matches!(elements.last(), Some(PathEl::ClosePath))
        && let Some(PathEl::MoveTo(start)) = elements.first()
        && element != 0
        && let Some(anchor_point) = anchor_of(&elements[element])
        && (anchor_point - *start).hypot() < 1e-9
    {
        updated[0] = PathEl::MoveTo(*start + delta);
    }

    *path = BezPath::from_vec(updated);
    true
}

fn anchor_of(el: &PathEl) -> Option<Point> {
    match el {
        PathEl::MoveTo(p) | PathEl::LineTo(p) => Some(*p),
        PathEl::QuadTo(_, p) | PathEl::CurveTo(_, _, p) => Some(*p),
        PathEl::ClosePath => None,
    }
}

// -- several anchors, tangent handles, deletion ------------------------------

/// Which of an anchor's two tangent handles.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HandleSide {
    /// The control point of the segment *arriving* at the anchor.
    In,
    /// The control point of the segment *leaving* it.
    Out,
}

impl HandleSide {
    pub fn opposite(self) -> Self {
        match self {
            Self::In => Self::Out,
            Self::Out => Self::In,
        }
    }
}

/// One tangent handle of an anchor, for drawing and for grabbing.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Handle {
    /// The element of the anchor this handle belongs to.
    pub element: usize,
    pub side: HandleSide,
    /// The control point itself.
    pub point: Point,
    /// The anchor it hangs from — the other end of the line drawn to it.
    pub anchor: Point,
    /// Whether the anchor is smooth, decided from the path as it stands: a drag
    /// that began on a smooth anchor turns the opposite handle with it.
    pub smooth: bool,
}

/// How far from collinear two handles may be, in degrees, and still count as
/// the two halves of one smooth point.
const SMOOTH_TOLERANCE_DEGREES: f64 = 2.0;

/// The two handles of one anchor, when it has them.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct AnchorHandles {
    pub incoming: Option<Point>,
    pub outgoing: Option<Point>,
}

/// Which control point of an element a handle is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Slot {
    /// The single control of a `QuadTo`.
    Quad,
    /// `c1` of a `CurveTo`: the handle leaving the previous anchor.
    First,
    /// `c2` of a `CurveTo`: the handle arriving at this element's anchor.
    Second,
}

fn control(el: &PathEl, slot: Slot) -> Option<Point> {
    match (el, slot) {
        (PathEl::QuadTo(c, _), Slot::Quad) => Some(*c),
        (PathEl::CurveTo(c1, _, _), Slot::First) => Some(*c1),
        (PathEl::CurveTo(_, c2, _), Slot::Second) => Some(*c2),
        _ => None,
    }
}

fn set_control(el: &mut PathEl, slot: Slot, to: Point) {
    match (el, slot) {
        (PathEl::QuadTo(c, _), Slot::Quad) => *c = to,
        (PathEl::CurveTo(c1, _, _), Slot::First) => *c1 = to,
        (PathEl::CurveTo(_, c2, _), Slot::Second) => *c2 = to,
        _ => {}
    }
}

/// Where one subpath of `elements` begins and ends.
#[derive(Debug, Clone, Copy)]
struct Subpath {
    /// Its `MoveTo`.
    start: usize,
    /// Its last element that carries an anchor.
    last: usize,
    closed: bool,
}

fn subpath_of(elements: &[PathEl], element: usize) -> Option<Subpath> {
    let start = (0..=element.min(elements.len().checked_sub(1)?))
        .rev()
        .find(|&i| matches!(elements[i], PathEl::MoveTo(_)))?;
    let end = (start + 1..elements.len())
        .find(|&i| matches!(elements[i], PathEl::MoveTo(_)))
        .unwrap_or(elements.len());
    let closed = end - 1 > start && matches!(elements[end - 1], PathEl::ClosePath);
    let last = if closed { end - 2 } else { end - 1 };
    Some(Subpath {
        start,
        last,
        closed,
    })
}

/// The other half of a closed subpath's seam: its first and last anchors are
/// the same point, and are one anchor to the user.
fn seam_partner(elements: &[PathEl], element: usize) -> Option<usize> {
    let sub = subpath_of(elements, element)?;
    if !sub.closed || sub.last <= sub.start {
        return None;
    }
    let (a, b) = (
        anchor_of(&elements[sub.start])?,
        anchor_of(&elements[sub.last])?,
    );
    if (a - b).hypot() >= 1e-9 {
        return None;
    }
    if element == sub.start {
        Some(sub.last)
    } else if element == sub.last {
        Some(sub.start)
    } else {
        None
    }
}

/// Where the `side` handle of the anchor at `element` is stored.
fn handle_slot(elements: &[PathEl], element: usize, side: HandleSide) -> Option<(usize, Slot)> {
    let sub = subpath_of(elements, element)?;
    let partner = seam_partner(elements, element);
    let (index, slot) = match side {
        HandleSide::In => {
            // The first anchor of a seam arrives by way of the last one.
            let owner = if element == sub.start { partner? } else { element };
            (owner, Slot::Second)
        }
        HandleSide::Out => {
            // And the last leaves by way of the first.
            let owner = if element == sub.last && partner == Some(sub.start) {
                sub.start
            } else {
                element
            };
            (owner + 1, Slot::First)
        }
    };
    let el = elements.get(index)?;
    match el {
        PathEl::CurveTo(..) => Some((index, slot)),
        PathEl::QuadTo(..) => Some((index, Slot::Quad)),
        _ => None,
    }
}

/// The tangent handles of the anchor at `element`.
pub fn anchor_handles(path: &BezPath, element: usize) -> AnchorHandles {
    let elements = path.elements();
    let read = |side| {
        handle_slot(elements, element, side).and_then(|(i, slot)| control(&elements[i], slot))
    };
    AnchorHandles {
        incoming: read(HandleSide::In),
        outgoing: read(HandleSide::Out),
    }
}

/// Is the anchor at `element` smooth — two handles, opposite each other, within
/// two degrees of one straight line through the anchor?
pub fn is_smooth(path: &BezPath, element: usize) -> bool {
    let Some(anchor) = path.elements().get(element).and_then(anchor_of) else {
        return false;
    };
    let AnchorHandles {
        incoming: Some(incoming),
        outgoing: Some(outgoing),
    } = anchor_handles(path, element)
    else {
        return false;
    };
    let (a, b) = (anchor - incoming, outgoing - anchor);
    let (la, lb) = (a.hypot(), b.hypot());
    if la < 1e-9 || lb < 1e-9 {
        return false;
    }
    a.dot(b) > 0.0 && a.cross(b).abs() <= SMOOTH_TOLERANCE_DEGREES.to_radians().sin() * la * lb
}

/// Every tangent handle on the path, anchor by anchor.
pub fn handles(path: &BezPath) -> Vec<Handle> {
    let mut out = Vec::new();
    for anchor in anchors(path) {
        let smooth = is_smooth(path, anchor.element);
        let AnchorHandles { incoming, outgoing } = anchor_handles(path, anchor.element);
        for (side, point) in [(HandleSide::In, incoming), (HandleSide::Out, outgoing)] {
            if let Some(point) = point {
                out.push(Handle {
                    element: anchor.element,
                    side,
                    point,
                    anchor: anchor.point,
                    smooth,
                });
            }
        }
    }
    out
}

/// Move several anchors together by `delta`, each carrying its handles.
///
/// One pass rather than a `move_anchor` per anchor, for two reasons that both
/// show up the moment more than one anchor is selected: a closed shape's seam is
/// *one* anchor stored twice, so selecting both halves must not move it twice;
/// and a quadratic's single control point is shared by the anchors either side
/// of it, so it must follow once when both of them move.
///
/// Returns false, touching nothing, if `delta` is not finite, `elements` is
/// empty, or any index does not name an anchor.
pub fn move_anchors(path: &mut BezPath, elements: &[usize], delta: Vec2) -> bool {
    if !delta.x.is_finite() || !delta.y.is_finite() || elements.is_empty() {
        return false;
    }
    let source = path.elements();
    if elements
        .iter()
        .any(|&i| source.get(i).and_then(anchor_of).is_none())
    {
        return false;
    }

    let mut anchors_to_move: Vec<usize> = Vec::new();
    for &i in elements {
        for j in std::iter::once(i).chain(seam_partner(source, i)) {
            if !anchors_to_move.contains(&j) {
                anchors_to_move.push(j);
            }
        }
    }

    let mut updated = source.to_vec();
    let mut moved: Vec<(usize, Slot)> = Vec::new();
    let mut shift = |updated: &mut Vec<PathEl>, index: usize, slot: Slot| {
        if moved.contains(&(index, slot)) {
            return;
        }
        moved.push((index, slot));
        if let Some(c) = updated.get(index).and_then(|el| control(el, slot)) {
            set_control(&mut updated[index], slot, c + delta);
        }
    };
    for &i in &anchors_to_move {
        // The incoming control, then the anchor itself, then the outgoing
        // control on the next element.
        shift(&mut updated, i, Slot::Second);
        shift(&mut updated, i, Slot::Quad);
        match &mut updated[i] {
            PathEl::MoveTo(p) | PathEl::LineTo(p) => *p += delta,
            PathEl::QuadTo(_, p) | PathEl::CurveTo(_, _, p) => *p += delta,
            PathEl::ClosePath => {}
        }
        shift(&mut updated, i + 1, Slot::First);
        shift(&mut updated, i + 1, Slot::Quad);
    }

    *path = BezPath::from_vec(updated);
    true
}

/// Put the `side` handle of the anchor at `element` at `to`.
///
/// With `mirror` the opposite handle turns to point away from it, keeping its
/// own length — what dragging one half of a smooth point does. Without it only
/// this handle moves, which breaks a smooth point into a corner: Alt-drag.
///
/// Absolute rather than a delta, so a drag applied step by step lands exactly
/// where one applied in a single step would.
///
/// Returns false if the anchor has no such handle.
pub fn move_handle(
    path: &mut BezPath,
    element: usize,
    side: HandleSide,
    to: Point,
    mirror: bool,
) -> bool {
    if !to.x.is_finite() || !to.y.is_finite() {
        return false;
    }
    let mut updated = path.elements().to_vec();
    let Some(anchor) = updated.get(element).and_then(anchor_of) else {
        return false;
    };
    let Some((index, slot)) = handle_slot(&updated, element, side) else {
        return false;
    };
    let opposite = handle_slot(&updated, element, side.opposite());
    set_control(&mut updated[index], slot, to);

    if mirror
        && let Some((oi, oslot)) = opposite
        && (oi, oslot) != (index, slot)
        && let Some(current) = control(&updated[oi], oslot)
    {
        let out = anchor - to;
        let length = (current - anchor).hypot();
        if out.hypot() > 1e-9 {
            set_control(&mut updated[oi], oslot, anchor + out.normalize() * length);
        }
    }

    *path = BezPath::from_vec(updated);
    true
}

/// Pull symmetric handles out of the anchor at `element`: the outgoing one at
/// `to`, the incoming one the same distance the other way. A straight segment
/// beside the anchor becomes a curve that still looks the same until the handle
/// moves, which is how a corner turns smooth.
///
/// Absolute, like [`move_handle`]: calling it again with a new `to` re-aims the
/// handles it already made. A closed shape whose closing line is only implied
/// has it written out first, so its first and last anchors can both curve.
///
/// Returns false if the anchor has no segment either side that can take a
/// handle.
pub fn pull_out_handles(path: &mut BezPath, element: usize, to: Point) -> bool {
    if !to.x.is_finite() || !to.y.is_finite() {
        return false;
    }
    let mut updated = path.elements().to_vec();
    let Some(anchor) = updated.get(element).and_then(anchor_of) else {
        return false;
    };

    // Write out an implied closing line, so the seam exists to curve.
    if let Some(sub) = subpath_of(&updated, element)
        && sub.closed
        && sub.last > sub.start
        && (element == sub.start || element == sub.last)
        && seam_partner(&updated, element).is_none()
        && let Some(start) = anchor_of(&updated[sub.start])
    {
        updated.insert(sub.last + 1, PathEl::LineTo(start));
    }

    // Straight segments become curves with the same shape: handles a third of
    // the way along.
    let thirds = |from: Point, towards: Point| {
        (
            from + (towards - from) / 3.0,
            from + (towards - from) * (2.0 / 3.0),
        )
    };
    let partner = seam_partner(&updated, element);
    let sub = subpath_of(&updated, element);
    let in_owner = match sub {
        Some(s) if element == s.start => partner,
        _ => Some(element),
    };
    if let Some(i) = in_owner
        && i > 0
        && let PathEl::LineTo(p) = updated[i]
        && let Some(before) = anchor_of(&updated[i - 1])
    {
        let (c1, c2) = thirds(before, p);
        updated[i] = PathEl::CurveTo(c1, c2, p);
    }
    let out_owner = match sub {
        Some(s) if element == s.last && partner == Some(s.start) => s.start,
        _ => element,
    };
    if let Some(PathEl::LineTo(p)) = updated.get(out_owner + 1).copied() {
        let start = anchor_of(&updated[out_owner]).unwrap_or(anchor);
        let (c1, c2) = thirds(start, p);
        updated[out_owner + 1] = PathEl::CurveTo(c1, c2, p);
    }

    let mut any = false;
    for (side, at) in [
        (HandleSide::Out, to),
        (HandleSide::In, anchor + (anchor - to)),
    ] {
        // A quadratic's one control is shared with the next anchor, so it
        // cannot be a symmetric pair; those sides are left alone.
        if let Some((i, slot)) = handle_slot(&updated, element, side)
            && slot != Slot::Quad
        {
            set_control(&mut updated[i], slot, at);
            any = true;
        }
    }
    if any {
        *path = BezPath::from_vec(updated);
    }
    any
}

/// One anchor with its handles, for rebuilding a path without some of them.
#[derive(Debug, Clone)]
struct Vertex {
    point: Point,
    incoming: Option<Point>,
    outgoing: Option<Point>,
    /// The elements it came from — two for a closed shape's seam.
    elements: Vec<usize>,
}

/// Remove the anchors at `elements`; the path closes up round the gap, the
/// neighbours joined by one segment that keeps their outer handles.
///
/// Returns the new path, or `None` if fewer than two anchors would be left — the
/// caller deletes the object, because there is no path to speak of. Indices that
/// do not name an anchor are ignored.
///
/// A path with a quadratic in it comes back with that segment as the cubic it
/// is equal to: rebuilding from anchors and handles has one segment kind.
pub fn delete_anchors(path: &BezPath, elements: &[usize]) -> Option<BezPath> {
    let source = path.elements();
    if !elements
        .iter()
        .any(|&i| source.get(i).and_then(anchor_of).is_some())
    {
        return Some(path.clone());
    }

    let mut subpaths: Vec<(Vec<Vertex>, bool)> = Vec::new();
    for (index, el) in source.iter().enumerate() {
        let vertex = |point| Vertex {
            point,
            incoming: None,
            outgoing: None,
            elements: vec![index],
        };
        match *el {
            PathEl::MoveTo(p) => subpaths.push((vec![vertex(p)], false)),
            PathEl::LineTo(p) => {
                if let Some((verts, _)) = subpaths.last_mut() {
                    verts.push(vertex(p));
                }
            }
            PathEl::QuadTo(c, p) => {
                if let Some((verts, _)) = subpaths.last_mut() {
                    let before = verts.last().map_or(p, |v| v.point);
                    if let Some(v) = verts.last_mut() {
                        v.outgoing = Some(before + (c - before) * (2.0 / 3.0));
                    }
                    let mut v = vertex(p);
                    v.incoming = Some(p + (c - p) * (2.0 / 3.0));
                    verts.push(v);
                }
            }
            PathEl::CurveTo(c1, c2, p) => {
                if let Some((verts, _)) = subpaths.last_mut() {
                    if let Some(v) = verts.last_mut() {
                        v.outgoing = Some(c1);
                    }
                    let mut v = vertex(p);
                    v.incoming = Some(c2);
                    verts.push(v);
                }
            }
            PathEl::ClosePath => {
                if let Some((verts, closed)) = subpaths.last_mut() {
                    *closed = true;
                    // The seam is one anchor stored twice.
                    if verts.len() > 1
                        && let Some(last) = verts.last()
                        && (last.point - verts[0].point).hypot() < 1e-9
                    {
                        let last = verts.pop().expect("just looked at it");
                        verts[0].incoming = last.incoming;
                        verts[0].elements.extend(last.elements);
                    }
                }
            }
        }
    }

    let mut out = BezPath::new();
    let mut remaining = 0;
    for (mut verts, closed) in subpaths {
        verts.retain(|v| !v.elements.iter().any(|e| elements.contains(e)));
        if verts.len() < 2 {
            continue;
        }
        remaining += verts.len();

        let segment = |from: &Vertex, to: &Vertex, out: &mut BezPath| {
            if from.outgoing.is_none() && to.incoming.is_none() {
                out.line_to(to.point);
            } else {
                out.curve_to(
                    from.outgoing.unwrap_or(from.point),
                    to.incoming.unwrap_or(to.point),
                    to.point,
                );
            }
        };
        out.move_to(verts[0].point);
        for pair in verts.windows(2) {
            segment(&pair[0], &pair[1], &mut out);
        }
        if closed {
            let (last, first) = (&verts[verts.len() - 1], &verts[0]);
            if last.outgoing.is_some() || first.incoming.is_some() {
                segment(last, first, &mut out);
            }
            out.close_path();
        }
    }

    (remaining >= 2).then_some(out)
}

// -- bending a segment: the Selection tool's arrow -----------------------------

/// How far along a segment a bend or a split may be taken, at either end.
///
/// A press close to an anchor would otherwise give a `t` of nearly 0 or 1, and
/// the bend's gain (`1 / t(1-t)…`) is unbounded there: a pixel of drag would
/// throw the handles across the page. Clamped, the worst case is a bend of
/// roughly four times the drag.
const BEND_T_MIN: f64 = 0.1;
const BEND_T_MAX: f64 = 0.9;

/// The segment of a path nearest a point.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SegmentHit {
    /// The element whose anchor ends the segment. For the closing line a
    /// `ClosePath` draws without saying so, the `ClosePath` itself.
    pub element: usize,
    /// Where along it, clamped to `[0.1, 0.9]` so a bend or a split taken there
    /// stays well behaved. See [`BEND_T_MIN`].
    pub t: f64,
    /// How far the nearest point is from the query.
    pub distance: f64,
}

/// The segment of `path` nearest `point`, with how far off it is.
///
/// Every segment counts, the closing line of a shape included: a rectangle's
/// last side is often implied by `ClosePath` rather than drawn by a `LineTo`.
pub fn nearest_segment(path: &BezPath, point: Point) -> Option<SegmentHit> {
    let mut best: Option<SegmentHit> = None;
    let mut start = Point::ORIGIN;
    let mut current = Point::ORIGIN;
    let mut consider = |element: usize, seg: PathSeg| {
        let near = seg.nearest(point, 1e-6);
        let distance = near.distance_sq.max(0.0).sqrt();
        if best.is_none_or(|b| distance < b.distance) {
            best = Some(SegmentHit {
                element,
                t: near.t.clamp(BEND_T_MIN, BEND_T_MAX),
                distance,
            });
        }
    };
    for (element, el) in path.elements().iter().enumerate() {
        match *el {
            PathEl::MoveTo(p) => {
                start = p;
                current = p;
            }
            PathEl::LineTo(p) => {
                consider(element, PathSeg::Line(kurbo::Line::new(current, p)));
                current = p;
            }
            PathEl::QuadTo(c, p) => {
                consider(element, PathSeg::Quad(kurbo::QuadBez::new(current, c, p)));
                current = p;
            }
            PathEl::CurveTo(c1, c2, p) => {
                consider(
                    element,
                    PathSeg::Cubic(kurbo::CubicBez::new(current, c1, c2, p)),
                );
                current = p;
            }
            PathEl::ClosePath => {
                if (current - start).hypot() > 1e-9 {
                    consider(element, PathSeg::Line(kurbo::Line::new(current, start)));
                }
                current = start;
            }
        }
    }
    best
}

/// If `element` is the `ClosePath` of a shape whose closing line is implied,
/// write that line out as a `LineTo`, so the segment exists to be edited. The
/// index of the segment's element either way.
///
/// Elements after it shift by one when a line was written.
fn write_out_closing_line(elements: &mut Vec<PathEl>, element: usize) -> Option<usize> {
    if !matches!(elements.get(element)?, PathEl::ClosePath) {
        return Some(element);
    }
    let sub = subpath_of(elements, element)?;
    let start = anchor_of(&elements[sub.start])?;
    let before = anchor_of(&elements[element.checked_sub(1)?])?;
    if (before - start).hypot() < 1e-9 {
        // The shape already ends on its start: there is no closing line.
        return None;
    }
    elements.insert(element, PathEl::LineTo(start));
    Some(element)
}

/// Bend the segment ending at `element` so the point at `t` moves by `delta`,
/// the segment's end anchors staying where they are.
///
/// A line or a quadratic becomes the cubic it is equal to first, so there are
/// two controls to move. With the controls shifted by
/// `Δc1 = delta·(1-t)·k` and `Δc2 = delta·t·k`, where
/// `k = 1 / (3t(1-t)((1-t)² + t²))`, the curve's point at `t` moves by exactly
/// `delta` — the curve passes through the pointer — and the control nearer the
/// grab moves more, which is what makes it a bend rather than a slide.
///
/// **Absolute**: it is a function of the segment as it is passed in, so a drag
/// is applied to a snapshot of the path taken at the press and lands where one
/// applied in a single step would. `t` is the *original* segment's parameter,
/// which degree elevation preserves.
///
/// Returns false, touching nothing, if `delta` is not finite, `t` is not
/// strictly inside the segment, or `element` is not a segment.
pub fn bend_segment(path: &mut BezPath, element: usize, t: f64, delta: Vec2) -> bool {
    if !delta.x.is_finite() || !delta.y.is_finite() || !(t > 0.0 && t < 1.0) {
        return false;
    }
    let mut updated = path.elements().to_vec();
    let Some(index) = write_out_closing_line(&mut updated, element) else {
        return false;
    };
    if index == 0 {
        return false;
    }
    let Some(from) = anchor_of(&updated[index - 1]) else {
        return false;
    };
    updated[index] = match updated[index] {
        PathEl::LineTo(p) => {
            PathEl::CurveTo(from + (p - from) / 3.0, from + (p - from) * (2.0 / 3.0), p)
        }
        PathEl::QuadTo(c, p) => {
            PathEl::CurveTo(from + (c - from) * (2.0 / 3.0), p + (c - p) * (2.0 / 3.0), p)
        }
        cubic @ PathEl::CurveTo(..) => cubic,
        PathEl::MoveTo(_) | PathEl::ClosePath => return false,
    };
    let s = 1.0 - t;
    let k = 1.0 / (3.0 * t * s * (s * s + t * t));
    if let PathEl::CurveTo(c1, c2, _) = &mut updated[index] {
        *c1 += delta * (s * k);
        *c2 += delta * (t * k);
    }
    *path = BezPath::from_vec(updated);
    true
}

/// Split the segment ending at `element` at `t` (de Casteljau), putting a new
/// anchor on the curve. Returns the element index of that new anchor.
///
/// The shape does not change: the two halves trace exactly what the one did,
/// whether it was a line, a quadratic or a cubic. The anchor is then free to be
/// dragged like any other — Animate's Ctrl-drag on a stroke.
///
/// Returns `None`, touching nothing, if `t` is not strictly inside the segment
/// or `element` is not a segment.
pub fn split_segment(path: &mut BezPath, element: usize, t: f64) -> Option<usize> {
    if !(t > 0.0 && t < 1.0) {
        return None;
    }
    let mut updated = path.elements().to_vec();
    let index = write_out_closing_line(&mut updated, element)?;
    if index == 0 {
        return None;
    }
    let from = anchor_of(&updated[index - 1])?;
    let lerp = |a: Point, b: Point| a + (b - a) * t;
    let (first, second) = match updated[index] {
        PathEl::LineTo(p) => {
            let m = lerp(from, p);
            (PathEl::LineTo(m), PathEl::LineTo(p))
        }
        PathEl::QuadTo(c, p) => {
            let (a, b) = (lerp(from, c), lerp(c, p));
            let m = lerp(a, b);
            (PathEl::QuadTo(a, m), PathEl::QuadTo(b, p))
        }
        PathEl::CurveTo(c1, c2, p) => {
            let (a, b, c) = (lerp(from, c1), lerp(c1, c2), lerp(c2, p));
            let (d, e) = (lerp(a, b), lerp(b, c));
            let m = lerp(d, e);
            (PathEl::CurveTo(a, d, m), PathEl::CurveTo(e, c, p))
        }
        PathEl::MoveTo(_) | PathEl::ClosePath => return None,
    };
    updated[index] = first;
    updated.insert(index + 1, second);
    *path = BezPath::from_vec(updated);
    Some(index)
}

#[cfg(test)]
mod tests {
    use super::*;
    use kurbo::{Rect, Shape};

    fn triangle() -> BezPath {
        let mut p = BezPath::new();
        p.move_to(Point::new(0.0, 0.0));
        p.line_to(Point::new(100.0, 0.0));
        p.line_to(Point::new(50.0, 80.0));
        p.close_path();
        p
    }

    fn curved() -> BezPath {
        let mut p = BezPath::new();
        p.move_to(Point::new(0.0, 0.0));
        p.curve_to(
            Point::new(20.0, 40.0),
            Point::new(80.0, 40.0),
            Point::new(100.0, 0.0),
        );
        p.curve_to(
            Point::new(120.0, -40.0),
            Point::new(180.0, -40.0),
            Point::new(200.0, 0.0),
        );
        p
    }

    #[test]
    fn anchors_are_the_on_curve_points_only() {
        let a = anchors(&triangle());
        assert_eq!(a.len(), 3, "close_path is not an anchor");
        assert_eq!(a[0].point, Point::new(0.0, 0.0));
        assert_eq!(a[2].point, Point::new(50.0, 80.0));
    }

    #[test]
    fn curves_report_their_endpoints_not_their_handles() {
        let a = anchors(&curved());
        assert_eq!(a.len(), 3);
        assert_eq!(a[1].point, Point::new(100.0, 0.0));
        assert_eq!(a[2].point, Point::new(200.0, 0.0));
    }

    #[test]
    fn the_nearest_anchor_is_found_within_tolerance() {
        let path = triangle();
        let hit = nearest_anchor(&path, Point::new(98.0, 3.0), 5.0).unwrap();
        assert_eq!(hit.point, Point::new(100.0, 0.0));

        assert!(
            nearest_anchor(&path, Point::new(50.0, 40.0), 5.0).is_none(),
            "the middle of an edge is not an anchor"
        );
    }

    #[test]
    fn moving_an_anchor_moves_it() {
        let mut path = triangle();
        assert!(move_anchor(&mut path, 1, Vec2::new(10.0, 5.0)));
        assert_eq!(anchors(&path)[1].point, Point::new(110.0, 5.0));
    }

    /// The property that separates a usable tool from a frustrating one: the
    /// curve must slide, not kink.
    #[test]
    fn moving_an_anchor_carries_both_adjacent_handles() {
        let mut path = curved();
        let delta = Vec2::new(0.0, 30.0);
        // Element 1 is the first curve; its endpoint is the middle anchor.
        assert!(move_anchor(&mut path, 1, delta));

        let elements = path.elements();
        // Incoming handle (c2 of element 1) must have moved.
        let PathEl::CurveTo(_, c2, p) = elements[1] else {
            panic!("expected a curve")
        };
        assert_eq!(p, Point::new(100.0, 30.0));
        assert_eq!(c2, Point::new(80.0, 70.0), "incoming handle should follow");

        // Outgoing handle (c1 of element 2) must have moved too.
        let PathEl::CurveTo(c1, _, end) = elements[2] else {
            panic!("expected a curve")
        };
        assert_eq!(
            c1,
            Point::new(120.0, -10.0),
            "outgoing handle should follow"
        );
        assert_eq!(end, Point::new(200.0, 0.0), "the far anchor must not move");
    }

    /// Tangent continuity is the visible consequence of carrying the handles.
    #[test]
    fn the_tangent_through_a_moved_anchor_is_preserved() {
        let before = curved();
        let mut after = curved();
        move_anchor(&mut after, 1, Vec2::new(15.0, 25.0));

        let tangent = |path: &BezPath| {
            let PathEl::CurveTo(_, c2, p) = path.elements()[1] else {
                panic!()
            };
            (p - c2).normalize()
        };

        let a = tangent(&before);
        let b = tangent(&after);
        assert!(
            (a - b).hypot() < 1e-9,
            "the tangent changed: {a:?} -> {b:?}"
        );
    }

    /// A closed shape must not tear open when its first anchor moves.
    #[test]
    fn closed_paths_stay_closed() {
        let mut path = Rect::new(0.0, 0.0, 100.0, 100.0).to_path(1e-9);
        let last = anchors(&path).len() - 1;
        assert!(move_anchor(&mut path, last, Vec2::new(20.0, 20.0)));

        assert!(
            path.elements()
                .iter()
                .any(|e| matches!(e, PathEl::ClosePath)),
            "the path should still be closed"
        );
        assert!(path.area().abs() > 0.0, "it should still enclose area");
    }

    #[test]
    fn moving_the_first_anchor_of_a_closed_path_works() {
        let mut path = triangle();
        assert!(move_anchor(&mut path, 0, Vec2::new(-10.0, -10.0)));
        assert_eq!(anchors(&path)[0].point, Point::new(-10.0, -10.0));
        assert!(
            path.elements()
                .iter()
                .any(|e| matches!(e, PathEl::ClosePath))
        );
    }

    #[test]
    fn a_bad_index_or_delta_is_refused() {
        let mut path = triangle();
        let before = path.elements().to_vec();

        assert!(!move_anchor(&mut path, 999, Vec2::new(1.0, 1.0)));
        assert!(
            !move_anchor(&mut path, 3, Vec2::new(1.0, 1.0)),
            "close_path"
        );
        assert!(!move_anchor(&mut path, 0, Vec2::new(f64::NAN, 0.0)));
        assert!(!move_anchor(&mut path, 0, Vec2::new(0.0, f64::INFINITY)));

        assert_eq!(path.elements(), before, "the path must be untouched");
    }

    #[test]
    fn an_empty_path_has_no_anchors() {
        let path = BezPath::new();
        assert!(anchors(&path).is_empty());
        assert!(nearest_anchor(&path, Point::ORIGIN, 10.0).is_none());
    }

    // -- several anchors, handles, deletion ----------------------------------

    /// A smooth middle anchor: handles (80,40) and (120,40)... made collinear.
    fn smooth() -> BezPath {
        let mut p = BezPath::new();
        p.move_to(Point::new(0.0, 0.0));
        p.curve_to(
            Point::new(20.0, 40.0),
            Point::new(80.0, 0.0),
            Point::new(100.0, 0.0),
        );
        p.curve_to(
            Point::new(130.0, 0.0),
            Point::new(180.0, -40.0),
            Point::new(200.0, 0.0),
        );
        p
    }

    #[test]
    fn moving_several_anchors_moves_each_with_its_handles() {
        let mut path = curved();
        assert!(move_anchors(&mut path, &[0, 2], Vec2::new(5.0, 7.0)));
        let a = anchors(&path);
        assert_eq!(a[0].point, Point::new(5.0, 7.0));
        assert_eq!(a[1].point, Point::new(100.0, 0.0), "unselected stays");
        assert_eq!(a[2].point, Point::new(205.0, 7.0));
        let PathEl::CurveTo(c1, _, _) = path.elements()[1] else {
            panic!()
        };
        assert_eq!(c1, Point::new(25.0, 47.0), "the first anchor's outgoing handle");
        let PathEl::CurveTo(_, c2, _) = path.elements()[2] else {
            panic!()
        };
        assert_eq!(c2, Point::new(185.0, -33.0), "the last anchor's incoming handle");
    }

    #[test]
    fn moving_adjacent_anchors_moves_the_handle_between_them_once() {
        let mut path = curved();
        let delta = Vec2::new(0.0, 10.0);
        assert!(move_anchors(&mut path, &[0, 1, 2], delta));
        // Every anchor and every handle shifted by exactly `delta`.
        let mut expected = curved();
        for el in expected.elements_mut() {
            *el = match *el {
                PathEl::MoveTo(p) => PathEl::MoveTo(p + delta),
                PathEl::CurveTo(a, b, c) => PathEl::CurveTo(a + delta, b + delta, c + delta),
                other => other,
            };
        }
        assert_eq!(path.elements(), expected.elements());
    }

    #[test]
    fn moving_both_halves_of_a_seam_moves_it_once() {
        // A closed shape written with an explicit closing segment.
        let mut path = BezPath::new();
        path.move_to(Point::new(0.0, 0.0));
        path.line_to(Point::new(100.0, 0.0));
        path.line_to(Point::new(100.0, 100.0));
        path.line_to(Point::new(0.0, 0.0));
        path.close_path();
        assert!(move_anchors(&mut path, &[0, 3], Vec2::new(10.0, 0.0)));
        let a = anchors(&path);
        assert_eq!(a[0].point, Point::new(10.0, 0.0));
        assert_eq!(a[3].point, Point::new(10.0, 0.0));
    }

    #[test]
    fn moving_anchors_refuses_a_bad_index_without_touching_the_path() {
        let mut path = triangle();
        let before = path.elements().to_vec();
        assert!(!move_anchors(&mut path, &[0, 99], Vec2::new(1.0, 1.0)));
        assert!(!move_anchors(&mut path, &[], Vec2::new(1.0, 1.0)));
        assert!(!move_anchors(&mut path, &[3], Vec2::new(1.0, 1.0)), "close_path");
        assert_eq!(path.elements(), before);
    }

    #[test]
    fn handles_are_reported_with_the_anchor_they_hang_from() {
        let path = smooth();
        let h = anchor_handles(&path, 1);
        assert_eq!(h.incoming, Some(Point::new(80.0, 0.0)));
        assert_eq!(h.outgoing, Some(Point::new(130.0, 0.0)));
        let first = anchor_handles(&path, 0);
        assert_eq!(first.incoming, None, "the start has nothing arriving");
        assert_eq!(first.outgoing, Some(Point::new(20.0, 40.0)));
        assert!(handles(&path).iter().all(|h| h.anchor == anchors(&path)[
            anchors(&path).iter().position(|a| a.element == h.element).unwrap()
        ].point));
        assert_eq!(handles(&path).len(), 4);
    }

    #[test]
    fn smooth_means_collinear_within_two_degrees() {
        assert!(is_smooth(&smooth(), 1));
        // `curved()` is smooth through the middle too; bend one handle and it
        // is not.
        assert!(is_smooth(&curved(), 1));
        let mut kinked = curved();
        assert!(move_handle(&mut kinked, 1, HandleSide::Out, Point::new(120.0, 40.0), false));
        assert!(!is_smooth(&kinked, 1));
        // One degree off still counts; five does not.
        let mut near = smooth();
        assert!(move_handle(&mut near, 1, HandleSide::Out, Point::new(100.0 + 30.0, 30.0 * 1f64.to_radians().tan()), false));
        assert!(is_smooth(&near, 1));
        let mut far = smooth();
        assert!(move_handle(&mut far, 1, HandleSide::Out, Point::new(100.0 + 30.0, 30.0 * 5f64.to_radians().tan()), false));
        assert!(!is_smooth(&far, 1));
        // A corner with no handles is not smooth.
        assert!(!is_smooth(&triangle(), 1));
    }

    #[test]
    fn dragging_a_smooth_handle_turns_the_opposite_one_and_keeps_its_length() {
        let mut path = smooth();
        let anchor = Point::new(100.0, 0.0);
        let before_len = (anchor - Point::new(80.0, 0.0)).hypot();
        // Drag the outgoing handle up and to the right.
        assert!(move_handle(&mut path, 1, HandleSide::Out, Point::new(130.0, 40.0), true));
        let h = anchor_handles(&path, 1);
        assert_eq!(h.outgoing, Some(Point::new(130.0, 40.0)));
        let incoming = h.incoming.unwrap();
        assert!(((anchor - incoming).hypot() - before_len).abs() < 1e-9, "length kept");
        // Opposite direction: still collinear through the anchor.
        assert!(is_smooth(&path, 1));
        assert!(incoming.y < 0.0 && incoming.x < anchor.x);
    }

    #[test]
    fn breaking_a_handle_moves_only_that_handle() {
        let mut path = smooth();
        let incoming = anchor_handles(&path, 1).incoming;
        assert!(move_handle(&mut path, 1, HandleSide::Out, Point::new(130.0, 40.0), false));
        assert_eq!(anchor_handles(&path, 1).incoming, incoming, "the other half stays");
        assert!(!is_smooth(&path, 1), "it is a corner now");
    }

    #[test]
    fn a_handle_that_does_not_exist_cannot_be_moved() {
        let mut path = triangle();
        assert!(!move_handle(&mut path, 1, HandleSide::Out, Point::new(1.0, 1.0), true));
        let mut path = smooth();
        assert!(!move_handle(&mut path, 0, HandleSide::In, Point::new(1.0, 1.0), true));
    }

    #[test]
    fn pulling_handles_from_a_corner_makes_symmetric_curves() {
        let mut path = triangle();
        // Anchor 1 is the (100, 0) corner, between two straight edges.
        assert!(pull_out_handles(&mut path, 1, Point::new(130.0, 20.0)));
        let h = anchor_handles(&path, 1);
        assert_eq!(h.outgoing, Some(Point::new(130.0, 20.0)));
        assert_eq!(h.incoming, Some(Point::new(70.0, -20.0)), "the same distance the other way");
        assert!(is_smooth(&path, 1));
        assert!(matches!(path.elements()[1], PathEl::CurveTo(..)));
        assert!(matches!(path.elements()[2], PathEl::CurveTo(..)));
        // The anchors did not move.
        assert_eq!(anchors(&path)[1].point, Point::new(100.0, 0.0));

        // Re-aiming re-uses the same handles rather than adding more.
        assert!(pull_out_handles(&mut path, 1, Point::new(140.0, 0.0)));
        assert_eq!(anchor_handles(&path, 1).outgoing, Some(Point::new(140.0, 0.0)));
        assert_eq!(path.elements().len(), triangle().elements().len());
    }

    #[test]
    fn pulling_handles_from_a_corner_keeps_the_curve_where_it_was_until_dragged() {
        // The promoted segments are the straight lines, written as cubics.
        let mut path = triangle();
        assert!(pull_out_handles(&mut path, 1, Point::new(100.0, 0.0)));
        let PathEl::CurveTo(c1, c2, p) = path.elements()[1] else {
            panic!()
        };
        assert!((c1 - Point::new(100.0 / 3.0, 0.0)).hypot() < 1e-9);
        assert!((c2 - Point::new(100.0, 0.0)).hypot() < 1e-9, "zero-length handle at the anchor");
        assert_eq!(p, Point::new(100.0, 0.0));
    }

    #[test]
    fn an_implied_closing_line_is_written_out_so_the_first_corner_can_curve() {
        let mut path = triangle();
        assert!(pull_out_handles(&mut path, 0, Point::new(-20.0, -20.0)));
        let h = anchor_handles(&path, 0);
        assert_eq!(h.outgoing, Some(Point::new(-20.0, -20.0)));
        assert_eq!(h.incoming, Some(Point::new(20.0, 20.0)), "both halves of the corner");
        assert!(path.elements().iter().any(|e| matches!(e, PathEl::ClosePath)));
        // Still the same three corners.
        assert_eq!(anchors(&path)[1].point, Point::new(100.0, 0.0));
        assert_eq!(anchors(&path)[2].point, Point::new(50.0, 80.0));
    }

    #[test]
    fn deleting_an_anchor_joins_its_neighbours() {
        let path = curved();
        let after = delete_anchors(&path, &[1]).expect("two anchors remain");
        let a = anchors(&after);
        assert_eq!(a.len(), 2);
        assert_eq!(a[0].point, Point::new(0.0, 0.0));
        assert_eq!(a[1].point, Point::new(200.0, 0.0));
        // The outer handles of the two segments survive on the joined one.
        assert_eq!(
            after.elements()[1],
            PathEl::CurveTo(
                Point::new(20.0, 40.0),
                Point::new(180.0, -40.0),
                Point::new(200.0, 0.0)
            )
        );
    }

    #[test]
    fn deleting_a_corner_of_a_polygon_leaves_a_closed_polygon() {
        let after = delete_anchors(&Rect::new(0.0, 0.0, 10.0, 10.0).to_path(1e-9), &[2])
            .expect("three corners remain");
        assert_eq!(anchors(&after).len(), 3);
        assert!(after.elements().iter().any(|e| matches!(e, PathEl::ClosePath)));
        assert!(after.area().abs() > 0.0);
    }

    #[test]
    fn deleting_the_first_anchor_of_an_open_path_moves_the_start() {
        let after = delete_anchors(&curved(), &[0]).unwrap();
        assert_eq!(anchors(&after)[0].point, Point::new(100.0, 0.0));
        assert!(matches!(after.elements()[0], PathEl::MoveTo(_)));
    }

    #[test]
    fn deleting_the_seam_of_a_closed_curve_removes_it_once() {
        let mut path = BezPath::new();
        path.move_to(Point::new(0.0, 0.0));
        path.line_to(Point::new(100.0, 0.0));
        path.line_to(Point::new(100.0, 100.0));
        path.line_to(Point::new(0.0, 100.0));
        path.line_to(Point::new(0.0, 0.0));
        path.close_path();
        // Deleting either half of the seam deletes the one anchor.
        for which in [0usize, 4] {
            let after = delete_anchors(&path, &[which]).unwrap();
            assert_eq!(anchors(&after).len(), 3, "deleting element {which}");
        }
    }

    #[test]
    fn deleting_down_to_fewer_than_two_anchors_gives_nothing() {
        assert!(delete_anchors(&triangle(), &[0, 1]).is_none());
        assert!(delete_anchors(&curved(), &[0, 1]).is_none());
        // Two left is still a path.
        assert!(delete_anchors(&triangle(), &[0]).is_some());
    }

    #[test]
    fn deleting_nothing_leaves_the_path_alone() {
        let path = curved();
        assert_eq!(
            delete_anchors(&path, &[99]).unwrap().elements(),
            path.elements()
        );
    }

    // -- bending and splitting segments ---------------------------------------

    /// A square, its closing line implied by `ClosePath`.
    fn open_square() -> BezPath {
        let mut p = BezPath::new();
        p.move_to(Point::new(0.0, 0.0));
        p.line_to(Point::new(100.0, 0.0));
        p.line_to(Point::new(100.0, 100.0));
        p.line_to(Point::new(0.0, 100.0));
        p.close_path();
        p
    }

    /// The point at `t` on the segment ending at `element`.
    fn at(path: &BezPath, element: usize, t: f64) -> Point {
        use kurbo::ParamCurve;
        let els = path.elements();
        let from = anchor_of(&els[element - 1]).unwrap();
        match els[element] {
            PathEl::LineTo(p) => kurbo::Line::new(from, p).eval(t),
            PathEl::QuadTo(c, p) => kurbo::QuadBez::new(from, c, p).eval(t),
            PathEl::CurveTo(c1, c2, p) => kurbo::CubicBez::new(from, c1, c2, p).eval(t),
            _ => panic!("not a segment"),
        }
    }

    #[test]
    fn nearest_segment_finds_the_side_and_where_along_it() {
        let hit = nearest_segment(&open_square(), Point::new(30.0, -2.0)).unwrap();
        assert_eq!(hit.element, 1, "the top side ends at element 1");
        assert!((hit.t - 0.3).abs() < 1e-6);
        assert!((hit.distance - 2.0).abs() < 1e-6);
    }

    #[test]
    fn nearest_segment_includes_the_implied_closing_line() {
        let path = open_square();
        // The left side, which only `ClosePath` draws.
        let hit = nearest_segment(&path, Point::new(-3.0, 40.0)).unwrap();
        assert_eq!(hit.element, 4, "the ClosePath element");
        assert!((hit.distance - 3.0).abs() < 1e-6);
        assert!((hit.t - 0.6).abs() < 1e-6, "from (0,100) up to (0,0)");
    }

    #[test]
    fn nearest_segment_clamps_t_away_from_the_ends() {
        let near_corner = nearest_segment(&open_square(), Point::new(1.0, -1.0)).unwrap();
        assert!(near_corner.t >= 0.1 - 1e-12 && near_corner.t <= 0.9 + 1e-12);
    }

    #[test]
    fn bending_a_line_lands_the_point_on_the_pointer_and_leaves_its_ends() {
        let mut path = open_square();
        let before = at(&path, 1, 0.4);
        let delta = Vec2::new(5.0, -30.0);
        assert!(bend_segment(&mut path, 1, 0.4, delta));
        assert!(
            matches!(path.elements()[1], PathEl::CurveTo(..)),
            "a line becomes a curve"
        );
        let after = at(&path, 1, 0.4);
        assert!((after - (before + delta)).hypot() < 1e-9, "{after:?}");
        // The end anchors did not move.
        assert_eq!(anchor_of(&path.elements()[0]), Some(Point::new(0.0, 0.0)));
        assert_eq!(
            anchor_of(&path.elements()[1]),
            Some(Point::new(100.0, 0.0))
        );
        // And the neighbouring sides are as they were.
        assert_eq!(path.elements()[2], open_square().elements()[2]);
    }

    #[test]
    fn the_control_nearer_the_grab_bends_more() {
        let mut path = open_square();
        assert!(bend_segment(&mut path, 1, 0.2, Vec2::new(0.0, -20.0)));
        let PathEl::CurveTo(c1, c2, _) = path.elements()[1] else {
            panic!("not a curve");
        };
        let straight = Point::new(100.0 / 3.0, 0.0);
        let straight2 = Point::new(200.0 / 3.0, 0.0);
        assert!((c1 - straight).hypot() > (c2 - straight2).hypot());
    }

    #[test]
    fn bending_a_quadratic_and_a_cubic_lands_on_the_pointer_too() {
        let mut quad = BezPath::new();
        quad.move_to(Point::new(0.0, 0.0));
        quad.quad_to(Point::new(50.0, 40.0), Point::new(100.0, 0.0));
        for path in [quad, curved()] {
            let mut bent = path.clone();
            let before = at(&bent, 1, 0.7);
            let delta = Vec2::new(-8.0, 13.0);
            assert!(bend_segment(&mut bent, 1, 0.7, delta));
            assert!((at(&bent, 1, 0.7) - (before + delta)).hypot() < 1e-9);
        }
    }

    #[test]
    fn bending_the_implied_closing_line_writes_it_out() {
        let mut path = open_square();
        // Half way up the left side, which only `ClosePath` draws.
        let before = Point::new(0.0, 50.0);
        let delta = Vec2::new(-20.0, 0.0);
        assert!(bend_segment(&mut path, 4, 0.5, delta));
        assert_eq!(path.elements().len(), 6, "a segment was written out");
        assert!(matches!(path.elements()[4], PathEl::CurveTo(..)));
        assert!(matches!(path.elements()[5], PathEl::ClosePath));
        let after = at(&path, 4, 0.5);
        assert!((after - (before + delta)).hypot() < 1e-9, "{after:?}");
    }

    #[test]
    fn bending_the_same_snapshot_twice_gives_the_same_path() {
        // The editor applies the whole drag to a snapshot at every step, so
        // nothing compounds.
        let snapshot = open_square();
        let mut a = snapshot.clone();
        let mut b = snapshot.clone();
        bend_segment(&mut a, 1, 0.5, Vec2::new(0.0, -10.0));
        bend_segment(&mut b, 1, 0.5, Vec2::new(0.0, -10.0));
        assert_eq!(a.elements(), b.elements());
    }

    #[test]
    fn bending_refuses_what_it_cannot_do() {
        let mut path = open_square();
        assert!(!bend_segment(&mut path, 0, 0.5, Vec2::new(1.0, 1.0)), "a MoveTo");
        assert!(!bend_segment(&mut path, 1, 0.0, Vec2::new(1.0, 1.0)), "t at the end");
        assert!(!bend_segment(&mut path, 1, 0.5, Vec2::new(f64::NAN, 1.0)));
        assert_eq!(path.elements(), open_square().elements());
    }

    #[test]
    fn splitting_leaves_the_shape_exactly_as_it_was() {
        for (path, element) in [(open_square(), 2usize), (curved(), 2usize)] {
            let mut split = path.clone();
            let t = 0.35;
            let on_curve = at(&path, element, t);
            let new = split_segment(&mut split, element, t).unwrap();
            assert_eq!(new, element);
            assert_eq!(split.elements().len(), path.elements().len() + 1);
            assert!((anchor_of(&split.elements()[new]).unwrap() - on_curve).hypot() < 1e-9);
            // Both halves trace the original: sample them.
            for i in 0..=10 {
                let u = i as f64 / 10.0;
                let first = at(&split, new, u);
                let original = at(&path, element, u * t);
                assert!((first - original).hypot() < 1e-9);
                let second = at(&split, new + 1, u);
                let original = at(&path, element, t + u * (1.0 - t));
                assert!((second - original).hypot() < 1e-9);
            }
        }
    }

    #[test]
    fn splitting_a_quadratic_and_the_closing_line_works() {
        let mut quad = BezPath::new();
        quad.move_to(Point::new(0.0, 0.0));
        quad.quad_to(Point::new(50.0, 40.0), Point::new(100.0, 0.0));
        let original = at(&quad, 1, 0.5);
        let new = split_segment(&mut quad, 1, 0.5).unwrap();
        assert!((anchor_of(&quad.elements()[new]).unwrap() - original).hypot() < 1e-9);

        let mut square = open_square();
        let new = split_segment(&mut square, 4, 0.25).unwrap();
        assert_eq!(
            square.elements().len(),
            7,
            "the closing line was written and split"
        );
        // From (0,100) a quarter of the way up to (0,0).
        assert_eq!(
            anchor_of(&square.elements()[new]),
            Some(Point::new(0.0, 75.0))
        );
    }

    #[test]
    fn splitting_refuses_what_it_cannot_do() {
        let mut path = open_square();
        assert!(split_segment(&mut path, 0, 0.5).is_none());
        assert!(split_segment(&mut path, 1, 1.0).is_none());
        assert_eq!(path.elements(), open_square().elements());
    }
}
