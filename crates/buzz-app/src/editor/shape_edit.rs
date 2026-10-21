//! Reshaping a raw shape by its own geometry: the Selection tool's arrow, and
//! Free Transform's Distort.
//!
//! # Why this is not in the tool machine
//!
//! Both begin with a question about the document that [`crate::tools`] cannot
//! ask — *which raw shapes have an edge or a corner under the pointer?* — and
//! both edit paths in several objects at once (a fill and its outline are
//! separate objects that happen to coincide, and must bend as one). So, like a
//! bone being posed, they are decided at the press by the editor and carried
//! here as a [`ShapeGesture`].
//!
//! # How a drag stays what the release commits
//!
//! Every move is **absolute from the press**: the gesture keeps each path as it
//! was when the pointer came down, and every pointer position re-derives the
//! reshaped path from that snapshot and the total distance travelled. Nothing
//! compounds, however many moves the window delivers, so the artwork on screen
//! mid-drag is exactly what the release leaves. One label for the whole drag
//! makes the history fold it into a single undo step.

use super::*;
use crate::tools::{
    ANCHOR_GRAB_PX, CLICK_SLOP_PX, TRANSFORM_GRAB_PX, TransformZone, transform_zone,
};

/// How many cubics each segment is cut into before a Distort warps it. See
/// [`buzz_geom::subdivide`].
const DISTORT_PIECES: usize = 8;

/// What a pointer over an unselected raw shape would take hold of — for the
/// cursor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShapeGrip {
    /// A corner (an on-curve anchor): a drag moves it.
    Corner,
    /// An edge: a drag bends it, and Ctrl-drag adds a corner there.
    Edge,
}

/// What the press found on one object.
#[derive(Debug, Clone, Copy)]
enum Hit {
    /// The anchor at this element.
    Anchor(usize),
    /// The segment ending at this element, at `t` along it.
    Segment { element: usize, t: f64 },
    /// The whole path — Distort.
    Whole,
}

/// One raw shape taking part in a gesture.
#[derive(Debug, Clone)]
struct Piece {
    id: ObjectId,
    /// The path in the object's own space **as it was at the press** (for
    /// Distort, cut into small cubics). Every move starts from this.
    snapshot: BezPath,
    /// The object's space → the space the pointer works in.
    to_edit: Affine,
    hit: Hit,
}

#[derive(Debug, Clone)]
enum Mode {
    /// Move the anchor each piece has hold of, carrying its handles.
    Corner,
    /// Bend the segment each piece has hold of.
    Bend,
    /// Warp every piece by the bilinear map that carries the transform box as
    /// it was onto the box with `corner` dragged.
    Distort { quad: [Point; 4], corner: usize },
}

/// A drag in progress on raw shapes' own geometry. See the module notes.
#[derive(Debug, Clone)]
pub struct ShapeGesture {
    mode: Mode,
    /// The undo label — the same for every step, so they fold into one.
    label: &'static str,
    pieces: Vec<Piece>,
    /// The press, in the space the pointer works in.
    origin: Point,
    origin_screen: Point,
    /// What was held on the last move; a release does not know.
    mods: Mods,
    /// The pointer has gone far enough for this to be a drag, not a click.
    dragged: bool,
}

/// Is this object a raw shape — a path of its own, not an instance, group,
/// rig, warp or text, and lying flat in its layer's plane?
///
/// The gestures here edit the path directly, so they must not touch anything
/// whose look is derived from elsewhere. A turned-in-space shape is left out
/// too: the pointer would be measured on a plane that is not the one the path
/// is drawn on.
fn is_raw_shape(object: &buzz_scene::Object) -> bool {
    matches!(object.kind, ObjectKind::Shape(_)) && object.text.is_none() && object.spatial.is_flat()
}

impl Editor {
    /// Take hold of raw geometry at a press, if there is any to take hold of.
    /// Returns whether a gesture began, in which case the press is spoken for.
    ///
    /// * **Selection tool** (without Shift, which extends the selection, or Alt,
    ///   which drags a copy of whatever is pressed on): an unselected raw
    ///   shape's corner or edge. See [`Self::shape_pieces_at`].
    /// * **Free Transform with Ctrl**, on a corner of the box, when everything
    ///   selected is a raw shape: Distort.
    pub(super) fn begin_shape_gesture(&mut self, doc: Point, screen: Point, mods: Mods) -> bool {
        let gesture = match self.machine.tool() {
            ToolId::Selection if !mods.shift && !mods.alt => {
                self.edit_gesture_at(doc, screen, mods)
            }
            ToolId::FreeTransform if mods.ctrl => self.distort_gesture_at(doc, screen, mods),
            _ => None,
        };
        let began = gesture.is_some();
        self.shape_gesture = gesture;
        began
    }

    /// What a pointer at `screen` is over, if the Selection tool would reshape
    /// it on a press — the cursor's question.
    pub fn shape_grip_hover(&self, screen: Point) -> Option<ShapeGrip> {
        if self.machine.tool() != ToolId::Selection || self.is_gesturing() {
            return None;
        }
        self.shape_pieces_at(self.screen_to_edit(screen))
            .map(|(grip, _)| grip)
    }

    /// The raw shapes with a corner or an edge under `doc`, and which of the two.
    ///
    /// **Corners first.** If any raw shape has an anchor within reach, those
    /// anchors are what is grabbed, and edges are not looked at: a press near a
    /// corner means the corner, not the edge running into it. Every raw path
    /// with such an anchor (or, failing that, such an edge) takes part, so a
    /// fill and its outline — separate objects, coincident in the drawing —
    /// move together.
    ///
    /// `None` when the press is on an **already-selected object**: that stays a
    /// move, as it always was. Only layers that can be edited are looked at,
    /// and only artwork on the current frame.
    fn shape_pieces_at(&self, doc: Point) -> Option<(ShapeGrip, Vec<Piece>)> {
        let scene = self.doc.scene();
        let frame = self.current_frame;
        let anchor_reach = ANCHOR_GRAB_PX / self.camera.zoom.max(f64::MIN_POSITIVE);
        let edge_reach = self.pick_tolerance();
        let reach = anchor_reach.max(edge_reach);

        let mut at_corner = Vec::new();
        let mut at_edge = Vec::new();
        for layer in scene
            .layers()
            .selectable_in_paint_order(scene.stage().sort_by_depth)
        {
            let follows = scene.layers().inherited_transform(layer.id, frame);
            for object in layer.objects_at(frame) {
                if !object.visible || object.locked || !is_raw_shape(object) {
                    continue;
                }
                let ObjectKind::Shape(shape) = &object.kind else {
                    continue;
                };
                // The same chain a drawn shape goes through, minus depth: the
                // path is in the object's space, the pointer in the layer's.
                let to_edit = follows * object.transform;
                let Some(back) = invert(to_edit) else {
                    continue;
                };
                // Most shapes are nowhere near the pointer.
                let near = to_edit
                    .transform_rect_bbox(shape.path.bounding_box())
                    .inflate(reach, reach);
                if !near.contains(doc) {
                    continue;
                }

                let anchor = buzz_geom::anchors(&shape.path)
                    .into_iter()
                    .map(|a| (a.element, (to_edit * a.point - doc).hypot()))
                    .filter(|(_, d)| *d <= anchor_reach)
                    .min_by(|a, b| a.1.total_cmp(&b.1));
                if let Some((element, _)) = anchor {
                    at_corner.push(Piece {
                        id: object.id,
                        snapshot: shape.path.clone(),
                        to_edit,
                        hit: Hit::Anchor(element),
                    });
                    continue;
                }
                // Measured in the object's space, then scaled back: exact for
                // a shape that is only turned and uniformly scaled, close
                // enough for a squeezed one at a few pixels' reach.
                if let Some(hit) = buzz_geom::nearest_segment(&shape.path, back * doc)
                    && hit.distance * to_edit.determinant().abs().sqrt() <= edge_reach
                {
                    at_edge.push(Piece {
                        id: object.id,
                        snapshot: shape.path.clone(),
                        to_edit,
                        hit: Hit::Segment {
                            element: hit.element,
                            t: hit.t,
                        },
                    });
                }
            }
        }
        if at_corner.is_empty() && at_edge.is_empty() {
            return None;
        }

        // A press on a selected object moves it. Asked last, and only when
        // there is something to take hold of: it is the costly question.
        //
        // A press on something drawn *above* every grabbed piece is a press
        // on that thing, not on geometry hidden under it: a rig's arm (a
        // symbol on its own layer) over a background shape's corner used to
        // reshape the background, so the arm — and every layer following it —
        // never moved.
        let (grip, pieces) = if at_corner.is_empty() {
            (ShapeGrip::Edge, at_edge)
        } else {
            (ShapeGrip::Corner, at_corner)
        };
        if let Some(top) = self.object_at(doc, self.pick_tolerance())
            && (self.selection.contains(top) || !pieces.iter().any(|p| p.id == top))
        {
            return None;
        }
        Some((grip, pieces))
    }

    /// The Selection tool's press on raw geometry: move a corner, bend an
    /// edge, or (Ctrl) put a new corner on an edge and move that.
    fn edit_gesture_at(&self, doc: Point, screen: Point, mods: Mods) -> Option<ShapeGesture> {
        let (grip, mut pieces) = self.shape_pieces_at(doc)?;
        let (mode, label) = match grip {
            ShapeGrip::Corner => (Mode::Corner, "Move Corner"),
            ShapeGrip::Edge if mods.ctrl => {
                // **Split at the press, in the snapshots**, not in the
                // document: a click that never becomes a drag must leave the
                // drawing alone. From the first move on, the new corner is an
                // anchor like any other.
                pieces = pieces
                    .into_iter()
                    .filter_map(|mut piece| {
                        let Hit::Segment { element, t } = piece.hit else {
                            return None;
                        };
                        let new = buzz_geom::split_segment(&mut piece.snapshot, element, t)?;
                        piece.hit = Hit::Anchor(new);
                        Some(piece)
                    })
                    .collect();
                if pieces.is_empty() {
                    return None;
                }
                (Mode::Corner, "Add Anchor")
            }
            ShapeGrip::Edge => (Mode::Bend, "Bend Curve"),
        };
        Some(ShapeGesture {
            mode,
            label,
            pieces,
            origin: doc,
            origin_screen: screen,
            mods,
            dragged: false,
        })
    }

    /// Free Transform's Ctrl-press on a corner of the box: Distort, if every
    /// selected object is a raw shape. Anything else is an ordinary scale.
    fn distort_gesture_at(&self, doc: Point, screen: Point, mods: Mods) -> Option<ShapeGesture> {
        let ids = self.selection.ids();
        if ids.is_empty() {
            return None;
        }
        let frame = self.selection_box()?;
        let pivot = self.pivot().unwrap_or_else(|| frame.center());
        let grab = TRANSFORM_GRAB_PX / self.camera.zoom.max(f64::MIN_POSITIVE);
        if transform_zone(frame, pivot, doc, grab) != TransformZone::Corner {
            return None;
        }
        let quad = frame.corners();
        let corner = (0..4).min_by(|&a, &b| {
            (quad[a] - doc)
                .hypot()
                .total_cmp(&(quad[b] - doc).hypot())
        })?;
        // A box with no area has nothing to warp.
        buzz_geom::bilinear_warp(quad, quad)?;

        let scene = self.doc.scene();
        let mut pieces = Vec::with_capacity(ids.len());
        for id in ids {
            let (at_frame, layer) = self.drawn_at(id)?;
            let object = scene
                .layers()
                .get(layer)?
                .objects_at(at_frame)
                .iter()
                .find(|o| o.id == id)?;
            let ObjectKind::Shape(shape) = &object.kind else {
                return None;
            };
            if !is_raw_shape(object) {
                return None;
            }
            let to_edit = scene.layers().inherited_transform(layer, at_frame) * object.transform;
            invert(to_edit)?;
            pieces.push(Piece {
                id,
                snapshot: buzz_geom::subdivide(&shape.path, DISTORT_PIECES),
                to_edit,
                hit: Hit::Whole,
            });
        }
        Some(ShapeGesture {
            mode: Mode::Distort { quad, corner },
            label: "Distort",
            pieces,
            origin: doc,
            origin_screen: screen,
            mods,
            dragged: false,
        })
    }

    /// The pointer moved during a gesture.
    pub(super) fn move_shape_gesture(&mut self, doc: Point, screen: Point, mods: Mods) {
        let Some(gesture) = self.shape_gesture.as_mut() else {
            return;
        };
        gesture.mods = mods;
        // Nothing happens until the pointer has gone far enough to be a drag,
        // so a click does not nudge anything.
        if !gesture.dragged && (screen - gesture.origin_screen).hypot() <= CLICK_SLOP_PX {
            return;
        }
        gesture.dragged = true;
        self.apply_shape_gesture(doc);
    }

    /// The pointer came up: finish what the last move left, and end the undo
    /// step. A click that never became a drag selects, as it always did.
    pub(super) fn finish_shape_gesture(&mut self, doc: Point, screen: Point) {
        let Some(gesture) = &self.shape_gesture else {
            return;
        };
        let moved = gesture.dragged || (screen - gesture.origin_screen).hypot() > CLICK_SLOP_PX;
        if moved {
            // Absolute, so repeating the last step is harmless.
            self.apply_shape_gesture(doc);
        }
        let Some(gesture) = self.shape_gesture.take() else {
            return;
        };
        if !moved && !matches!(gesture.mode, Mode::Distort { .. }) {
            self.apply(ToolAction::PickAt {
                point: doc,
                additive: false,
            });
        }
        self.doc.end_gesture();
    }

    /// Put every piece where a pointer at `pointer` leaves it.
    fn apply_shape_gesture(&mut self, pointer: Point) {
        let Some(gesture) = &self.shape_gesture else {
            return;
        };
        let updates = reshaped_paths(gesture, pointer);
        if updates.is_empty() {
            return;
        }
        let label = gesture.label;
        let at = self.edit_at();
        self.doc.edit(label, |scene| {
            for (id, path) in &updates {
                update_shape(scene, at, *id, |s| s.path = path.clone());
            }
        });
    }
}

/// Every piece's path as a pointer at `pointer` leaves it, from the snapshots.
///
/// The pointer's travel is a vector in the pointer's space; each object's path
/// lives in its own, so it comes back through that object's transform — the
/// same trip the Subselection drags make, which is what keeps a turned or
/// squeezed shape's corner under the pointer.
fn reshaped_paths(gesture: &ShapeGesture, pointer: Point) -> Vec<(ObjectId, BezPath)> {
    let delta = pointer - gesture.origin;
    let warp = match &gesture.mode {
        Mode::Distort { quad, corner } => buzz_geom::bilinear_warp(
            *quad,
            buzz_geom::drag_corner(*quad, *corner, delta, gesture.mods.shift),
        ),
        _ => None,
    };
    gesture
        .pieces
        .iter()
        .filter_map(|piece| {
            let back = invert(piece.to_edit)?;
            let local = back.deref_vector(delta);
            let mut path = piece.snapshot.clone();
            match (&gesture.mode, piece.hit) {
                (Mode::Corner, Hit::Anchor(element)) => {
                    buzz_geom::move_anchors(&mut path, &[element], local).then_some(())?;
                }
                (Mode::Bend, Hit::Segment { element, t }) => {
                    buzz_geom::bend_segment(&mut path, element, t, local).then_some(())?;
                }
                (Mode::Distort { .. }, Hit::Whole) => {
                    let warp = warp?;
                    path = buzz_geom::map_points(&piece.snapshot, |p| {
                        back * warp.map(piece.to_edit * p)
                    });
                }
                _ => return None,
            }
            Some((piece.id, path))
        })
        .collect()
}
