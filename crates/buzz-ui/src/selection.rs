//! What is currently selected.
//!
//! Animate keeps selection separate from the document: selecting nothing is a
//! normal state, selection survives most edits, and it is *not* saved with the
//! file. Modelling it outside [`buzz_scene::Scene`] keeps it out of the undo
//! history too, which matches how Animate behaves — Ctrl+Z undoes your last
//! edit, not your last click.

use std::collections::BTreeSet;

use buzz_geom::Rect;
use buzz_scene::{LayerId, LayerKind, ObjectId, Scene};

/// Animate's Shift+click range: every id between `anchor` and `clicked` in
/// `order`, inclusive of both ends and regardless of which one comes first in
/// `order`. Pure and `Scene`-free, so the sweep itself is simple to test —
/// I2's collapsed folders are folded in separately, by
/// [`expand_collapsed_folders`], since that needs the scene. See I1 in
/// `docs/INTERACTION_FIXES_PLAN.md`.
///
/// Either id missing from `order` (for instance an anchor that has since been
/// deleted) makes shift-click behave like a plain click on `clicked` rather
/// than panic or silently select nothing.
pub fn layer_range(order: &[LayerId], anchor: LayerId, clicked: LayerId) -> Vec<LayerId> {
    let (Some(a), Some(b)) = (
        order.iter().position(|&id| id == anchor),
        order.iter().position(|&id| id == clicked),
    ) else {
        return vec![clicked];
    };
    let (lo, hi) = (a.min(b), a.max(b));
    order[lo..=hi].to_vec()
}

/// I2: a range's own collapsed folders bring their hidden children with
/// them — a folder still counts as "between" the anchor and the click even
/// though its contents are not drawn as rows. Expands every collapsed
/// [`LayerKind::Folder`] in `ids` to include [`buzz_scene::LayerStack::descendants_of`]
/// it; anything else in `ids` is kept as-is.
pub fn expand_collapsed_folders(scene: &Scene, ids: &[LayerId]) -> Vec<LayerId> {
    let mut out = Vec::new();
    for &id in ids {
        out.push(id);
        if let Some(layer) = scene.layers().get(id)
            && layer.kind == LayerKind::Folder
            && layer.collapsed
        {
            out.extend(scene.layers().descendants_of(id));
        }
    }
    out
}

/// A timeline cell selection, kept separate from artwork selection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TimelineSelection {
    pub layers: BTreeSet<LayerId>,
    pub first_frame: u32,
    pub last_frame: u32,
    pub anchor_layer: LayerId,
    pub anchor_frame: u32,
}

impl TimelineSelection {
    pub fn cell(layer: LayerId, frame: u32) -> Self {
        Self {
            layers: [layer].into_iter().collect(),
            first_frame: frame,
            last_frame: frame,
            anchor_layer: layer,
            anchor_frame: frame,
        }
    }

    pub fn new(
        layers: impl IntoIterator<Item = LayerId>,
        first_frame: u32,
        last_frame: u32,
        anchor_layer: LayerId,
        anchor_frame: u32,
    ) -> Self {
        Self {
            layers: layers.into_iter().collect(),
            first_frame: first_frame.min(last_frame),
            last_frame: first_frame.max(last_frame),
            anchor_layer,
            anchor_frame,
        }
    }

    pub fn contains(&self, layer: LayerId, frame: u32) -> bool {
        self.layers.contains(&layer) && (self.first_frame..=self.last_frame).contains(&frame)
    }

    pub fn frames(&self) -> std::ops::RangeInclusive<u32> {
        self.first_frame..=self.last_frame
    }
}

/// The current selection.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Selection {
    /// `BTreeSet` so iteration order is stable, which keeps grouping and
    /// z-order operations deterministic.
    objects: BTreeSet<ObjectId>,
    /// The layer new artwork goes on. Animate always has exactly one.
    active_layer: Option<LayerId>,
    /// All layers currently selected (e.g. from vertical drag across layers on a frame).
    selected_layers: Vec<LayerId>,
    timeline: Option<TimelineSelection>,
    /// **I1: the last plain-clicked layer.** Shift+click sweeps a range from
    /// here to the layer clicked; Ctrl/Cmd+click moves this to the layer it
    /// toggles, so a shift-click right after one sweeps from there instead.
    /// Not saved with the document — it is as much a view thing as the rest
    /// of `Selection`.
    range_anchor: Option<LayerId>,
}

impl Selection {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn is_empty(&self) -> bool {
        self.objects.is_empty()
    }

    pub fn len(&self) -> usize {
        self.objects.len()
    }

    pub fn contains(&self, id: ObjectId) -> bool {
        self.objects.contains(&id)
    }

    pub fn iter(&self) -> impl Iterator<Item = ObjectId> + '_ {
        self.objects.iter().copied()
    }

    pub fn ids(&self) -> Vec<ObjectId> {
        self.objects.iter().copied().collect()
    }

    pub fn clear(&mut self) {
        self.objects.clear();
    }

    pub fn select_one(&mut self, id: ObjectId) {
        self.timeline = None;
        self.objects.clear();
        self.objects.insert(id);
    }

    pub fn add(&mut self, id: ObjectId) {
        self.timeline = None;
        self.objects.insert(id);
    }

    pub fn remove(&mut self, id: ObjectId) {
        self.objects.remove(&id);
    }

    /// Shift-click behaviour: in the selection it comes out, otherwise it goes
    /// in.
    pub fn toggle(&mut self, id: ObjectId) {
        if !self.objects.remove(&id) {
            self.objects.insert(id);
        }
    }

    pub fn set(&mut self, ids: impl IntoIterator<Item = ObjectId>) {
        self.timeline = None;
        self.objects = ids.into_iter().collect();
    }

    pub fn extend(&mut self, ids: impl IntoIterator<Item = ObjectId>) {
        self.objects.extend(ids);
    }

    pub fn active_layer(&self) -> Option<LayerId> {
        self.active_layer
    }

    pub fn selected_layers(&self) -> &[LayerId] {
        &self.selected_layers
    }

    pub fn timeline(&self) -> Option<&TimelineSelection> {
        self.timeline.as_ref()
    }

    pub fn set_timeline(&mut self, selection: TimelineSelection) {
        self.selected_layers = selection.layers.iter().copied().collect();
        self.active_layer = Some(selection.anchor_layer);
        self.timeline = Some(selection);
    }

    pub fn clear_timeline(&mut self) {
        self.timeline = None;
    }

    pub fn is_layer_selected(&self, id: LayerId) -> bool {
        self.active_layer == Some(id) || self.selected_layers.contains(&id)
    }

    pub fn set_selected_layers(&mut self, layers: &[LayerId]) {
        self.selected_layers = layers.to_vec();
        if self.active_layer.is_none() || !layers.iter().any(|&l| Some(l) == self.active_layer) {
            self.active_layer = layers.first().copied();
        }
    }

    pub fn set_active_layer(&mut self, layer: Option<LayerId>) {
        self.active_layer = layer;
        self.selected_layers = layer.into_iter().collect();
        self.timeline = None;
    }

    /// I1: the last plain-clicked layer, which a Shift+click ranges from.
    pub fn range_anchor(&self) -> Option<LayerId> {
        self.range_anchor
    }

    /// Move the anchor without otherwise touching the selection — what
    /// Ctrl/Cmd+click does, so the *next* Shift+click sweeps from the layer
    /// just toggled rather than an older click.
    pub fn set_range_anchor(&mut self, layer: Option<LayerId>) {
        self.range_anchor = layer;
    }

    /// Drop anything no longer in the document.
    ///
    /// Undo can remove objects that are still selected; without this the
    /// selection would keep referring to them and transform handles would be
    /// drawn around nothing.
    pub fn prune(&mut self, scene: &Scene) {
        self.objects.retain(|id| scene.find_object(*id).is_some());
        self.selected_layers
            .retain(|id| scene.layers().get(*id).is_some());
        if let Some(timeline) = &mut self.timeline {
            timeline
                .layers
                .retain(|id| scene.layers().get(*id).is_some());
            if timeline.layers.is_empty() {
                self.timeline = None;
            } else if !timeline.layers.contains(&timeline.anchor_layer) {
                timeline.anchor_layer = *timeline.layers.iter().next().expect("non-empty");
            }
        }
        if let Some(layer) = self.active_layer
            && scene.layers().get(layer).is_none()
        {
            self.active_layer = scene.layers().iter().next().map(|l| l.id);
            if let Some(first) = self.active_layer {
                if !self.selected_layers.contains(&first) {
                    self.selected_layers.push(first);
                }
            }
        }
        // A deleted anchor cannot sweep a range; the active layer is the
        // nearest thing to "the layer last clicked" left standing.
        if let Some(anchor) = self.range_anchor
            && scene.layers().get(anchor).is_none()
        {
            self.range_anchor = self.active_layer;
        }
    }

    /// Keep only the objects a caller says are still on screen.
    ///
    /// Edit Multiple Frames decides that against a *range* of frames rather
    /// than one, so the rule lives with the caller and this only applies it.
    pub fn retain(&mut self, keep: impl Fn(ObjectId) -> bool) {
        self.objects.retain(|id| keep(*id));
    }

    /// Drop anything not visible at `frame`.
    ///
    /// Moving the playhead can leave the selection pointing at artwork on a
    /// keyframe that is no longer showing, which would draw transform handles
    /// around something the user cannot see.
    pub fn prune_to_frame(&mut self, scene: &Scene, frame: u32) {
        self.objects.retain(|id| {
            scene
                .layers()
                .iter()
                .any(|l| l.objects_at(frame).iter().any(|o| o.id == *id))
        });
    }

    /// Ensure there is an active layer, choosing a sensible one if not.
    ///
    /// Prefers the topmost layer that can actually hold artwork, since locked
    /// and hidden layers cannot be drawn on.
    pub fn ensure_active_layer(&mut self, scene: &Scene) -> Option<LayerId> {
        let usable = |id: LayerId| {
            scene
                .layers()
                .get(id)
                .is_some_and(|l| l.is_editable() && scene.layers().is_effectively_visible(id))
        };

        if let Some(current) = self.active_layer
            && usable(current)
        {
            return Some(current);
        }

        self.active_layer = scene
            .layers()
            .iter()
            .find(|l| usable(l.id))
            .map(|l| l.id)
            // Fall back to any layer, so the panel still shows something.
            .or_else(|| scene.layers().iter().next().map(|l| l.id));
        self.active_layer
    }

    /// Combined bounds of the selection **where the artwork is drawn**, for
    /// transform handles.
    ///
    /// Goes through the scene rather than [`Object::bounds`] so that a
    /// selected symbol instance reports the extents of the artwork inside it.
    ///
    /// # Why the frame is needed
    ///
    /// Layer parenting draws a layer's artwork somewhere other than where its
    /// geometry sits, and how far depends on where the parent is *on this
    /// frame*. Without it these bounds described a rig's limb where it was
    /// drawn at rest rather than where it is now, and everything built on them
    /// was wrong in the same way: the transform box and its handles drew off
    /// the artwork, the transformation point sat beside it, and — worst,
    /// because it is silent — the "did this drag start inside the selection?"
    /// test said no when the user pressed on their own character, so dragging
    /// it rubber-banded a marquee instead of moving it.
    ///
    /// This is the same space [`crate::Selection`]'s callers hit-test in: the
    /// artwork as drawn, before the document camera.
    /// Combined bounds of the selection in the objects' **own** space, before
    /// layer parenting moves the artwork.
    ///
    /// For edits that rebase geometry rather than measure it on screen —
    /// Convert to Symbol lifts the objects out with their own transforms, so
    /// the registration point it computes has to be in the space those
    /// transforms are written in. Anything the user points at or that is drawn
    /// as chrome wants [`Self::bounds_at`] instead.
    pub fn local_bounds(&self, scene: &Scene) -> Option<Rect> {
        self.objects
            .iter()
            .filter_map(|id| {
                scene
                    .find_object(*id)
                    .map(|(_, o)| scene.resolved_bounds(o))
            })
            .reduce(|a, b| a.union(b))
    }

    pub fn bounds_at(&self, scene: &Scene, frame: u32) -> Option<Rect> {
        self.objects
            .iter()
            .filter_map(|id| {
                let (layer, object) = scene.find_object(*id)?;
                let follows = scene.layers().inherited_transform(layer, frame);
                Some(buzz_scene::object::transform_rect(
                    follows,
                    scene.resolved_bounds(object),
                ))
            })
            .reduce(|a, b| a.union(b))
    }

    /// Make `layer` the active one **and select what is on it** at `frame`.
    ///
    /// This is what clicking a layer does in Animate: the layer becomes the one
    /// new artwork goes on, and everything already on it is selected — so the
    /// obvious next move, transforming or colouring the lot, needs no second
    /// gesture. Before this, clicking a layer only moved the highlight, and the
    /// artwork had to be marquee-selected afterwards.
    ///
    /// **Locked and hidden artwork is skipped**, and a locked or hidden *layer*
    /// selects nothing at all — the same rule hit-testing uses. Selecting
    /// something that cannot then be moved would be an empty promise, and
    /// worse, a delete would take it.
    pub fn select_layer(&mut self, scene: &Scene, layer: LayerId, frame: u32) {
        self.active_layer = Some(layer);
        self.selected_layers = vec![layer];
        self.timeline = None;
        self.objects.clear();
        // I1: a plain click is always the new anchor a Shift+click sweeps from.
        self.range_anchor = Some(layer);

        // `selectable` is the same rule hit-testing uses — editable, visible
        // through its folders, and not locked through them either. Reusing it
        // rather than re-deriving it is what keeps clicking a layer and
        // clicking its artwork agreeing about what can be had.
        let Some(target) = scene.layers().selectable().find(|l| l.id == layer) else {
            return;
        };
        for object in target.objects_at(frame).iter() {
            if object.visible && !object.locked {
                self.objects.insert(object.id);
            }
        }
    }

    /// Select multiple layers at once (e.g. from vertical drag across frames on a column).
    ///
    /// The first layer in the list becomes the active layer for tools, and artwork on
    /// all selectable specified layers at `frame` is selected.
    pub fn select_layers(&mut self, scene: &Scene, layers: &[LayerId], frame: u32) {
        if let Some(&first) = layers.first() {
            self.active_layer = Some(first);
        }
        self.selected_layers = layers.to_vec();
        self.timeline = None;
        self.objects.clear();

        for &layer in layers {
            let Some(target) = scene.layers().selectable().find(|l| l.id == layer) else {
                continue;
            };
            for object in target.objects_at(frame).iter() {
                if object.visible && !object.locked {
                    self.objects.insert(object.id);
                }
            }
        }
    }

    /// Select what is on the active layer, if anything is.
    ///
    /// Used when a tool is chosen that needs something to work on: see
    /// `Editor::select_active_layer_contents`.
    pub fn select_active_layer_contents(&mut self, scene: &Scene, frame: u32) {
        if let Some(layer) = self.active_layer {
            self.select_layer(scene, layer, frame);
        }
    }

    /// Description for the Properties panel header.
    pub fn describe(&self, scene: &Scene) -> String {
        match self.objects.len() {
            0 => "Document".to_string(),
            1 => {
                let id = *self.objects.iter().next().expect("len is 1");
                match scene.find_object(id) {
                    Some((_, o)) => match &o.kind {
                        buzz_scene::ObjectKind::Shape(_) => "Shape".to_string(),
                        buzz_scene::ObjectKind::Group(c) => format!("Group ({} items)", c.len()),
                        // Animate names the symbol kind here, not "Instance".
                        buzz_scene::ObjectKind::Instance(i) => {
                            match scene.library().get(i.symbol) {
                                Some(s) => format!("{} — {}", s.kind.label(), s.name),
                                None => "Missing Symbol".to_string(),
                            }
                        }
                        // Animate calls rigged artwork an armature and says how
                        // many bones it has, which is the number you need when
                        // deciding whether you are looking at the right rig.
                        buzz_scene::ObjectKind::Armature(rig) => {
                            format!("Armature ({} bones)", rig.armature.len())
                        }
                        buzz_scene::ObjectKind::Warp(w) => {
                            format!("Warp ({} handles)", w.handles.len())
                        }
                    },
                    None => "Shape".to_string(),
                }
            }
            n => format!("{n} objects selected"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use buzz_geom::Shape as _;
    use buzz_scene::{LayerKind, ShapeData};
    use kurbo::Rect as KRect;
    use peniko::Color;

    #[test]
    fn timeline_selection_keeps_inclusive_range_and_repairs_deleted_anchor() {
        let mut scene = Scene::empty();
        let first = scene.add_layer("First", LayerKind::Normal);
        let second = scene.add_layer("Second", LayerKind::Normal);
        let mut selection = Selection::new();
        selection.set_timeline(TimelineSelection::new([first, second], 8, 3, first, 8));
        let cells = selection.timeline().unwrap();
        assert_eq!(
            cells.frames().collect::<Vec<_>>(),
            (3..=8).collect::<Vec<_>>()
        );
        scene.remove_layer(first);
        selection.prune(&scene);
        let cells = selection.timeline().unwrap();
        assert_eq!(cells.anchor_layer, second);
        assert!(cells.layers.contains(&second));
    }

    fn scene_with(n: u64) -> (Scene, LayerId, Vec<ObjectId>) {
        let mut scene = Scene::empty();
        let layer = scene.add_layer("L", LayerKind::Normal);
        let ids = (0..n)
            .map(|i| {
                scene
                    .add_shape(
                        layer,
                        ShapeData::filled(
                            KRect::new(i as f64 * 20.0, 0.0, i as f64 * 20.0 + 10.0, 10.0)
                                .to_path(1e-9),
                            Color::WHITE,
                        ),
                    )
                    .unwrap()
            })
            .collect();
        (scene, layer, ids)
    }

    /// Clicking a layer selects what is on it — the whole point of the change.
    #[test]
    fn selecting_a_layer_selects_its_artwork() {
        let (scene, layer, ids) = scene_with(3);
        let mut selection = Selection::new();

        selection.select_layer(&scene, layer, 0);

        assert_eq!(selection.active_layer(), Some(layer));
        assert_eq!(selection.len(), 3);
        for id in ids {
            assert!(selection.contains(id), "{id:?} should have been selected");
        }
    }

    /// It **replaces** rather than adds, so clicking one layer and then another
    /// leaves only the second's artwork selected.
    #[test]
    fn selecting_another_layer_replaces_the_selection() {
        let (mut scene, first, _) = scene_with(2);
        let second = scene.add_layer("Second", LayerKind::Normal);
        let other = scene
            .add_shape(
                second,
                ShapeData::filled(KRect::new(0.0, 0.0, 5.0, 5.0).to_path(1e-9), Color::WHITE),
            )
            .unwrap();

        let mut selection = Selection::new();
        selection.select_layer(&scene, first, 0);
        assert_eq!(selection.len(), 2);

        selection.select_layer(&scene, second, 0);
        assert_eq!(selection.ids(), vec![other]);
        assert_eq!(selection.active_layer(), Some(second));
    }

    /// A locked layer becomes active — you can still aim at it — but nothing on
    /// it is selected. Selecting artwork that cannot then be moved would be an
    /// empty promise, and a delete would take it anyway.
    #[test]
    fn a_locked_layer_is_made_active_but_selects_nothing() {
        let (mut scene, layer, _) = scene_with(2);
        scene.update_layer(layer, |l| l.locked = true);

        let mut selection = Selection::new();
        selection.select_layer(&scene, layer, 0);

        assert_eq!(selection.active_layer(), Some(layer));
        assert!(selection.is_empty(), "a locked layer selected its artwork");
    }

    #[test]
    fn a_hidden_layer_selects_nothing_either() {
        let (mut scene, layer, _) = scene_with(2);
        scene.update_layer(layer, |l| l.visible = false);

        let mut selection = Selection::new();
        selection.select_layer(&scene, layer, 0);
        assert!(selection.is_empty());
    }

    /// The frame matters: a layer's artwork is different on different frames,
    /// so clicking it while the playhead is at forty must not select frame
    /// zero's drawing.
    #[test]
    fn the_artwork_selected_is_the_artwork_on_that_frame() {
        let (mut scene, layer, ids) = scene_with(1);
        // A second keyframe further along, with its own drawing.
        scene.update_layer(layer, |l| {
            l.frames.insert_blank_keyframe(10);
        });
        let later = scene
            .add_shape_at(
                layer,
                10,
                ShapeData::filled(KRect::new(0.0, 0.0, 5.0, 5.0).to_path(1e-9), Color::WHITE),
            )
            .unwrap();

        let mut selection = Selection::new();
        selection.select_layer(&scene, layer, 0);
        assert_eq!(selection.ids(), ids);

        selection.select_layer(&scene, layer, 10);
        assert_eq!(selection.ids(), vec![later]);
    }

    /// A layer with nothing on it selects nothing, and does not keep whatever
    /// was selected before.
    #[test]
    fn an_empty_layer_clears_the_selection() {
        let (mut scene, layer, _) = scene_with(2);
        let empty = scene.add_layer("Empty", LayerKind::Normal);

        let mut selection = Selection::new();
        selection.select_layer(&scene, layer, 0);
        assert_eq!(selection.len(), 2);

        selection.select_layer(&scene, empty, 0);
        assert!(selection.is_empty());
        assert_eq!(selection.active_layer(), Some(empty));
    }

    #[test]
    fn selecting_one_replaces_whatever_was_selected() {
        let (_, _, ids) = scene_with(3);
        let mut s = Selection::new();
        s.select_one(ids[0]);
        s.select_one(ids[1]);
        assert_eq!(s.len(), 1);
        assert!(s.contains(ids[1]));
    }

    #[test]
    fn shift_click_toggles() {
        let (_, _, ids) = scene_with(3);
        let mut s = Selection::new();
        s.toggle(ids[0]);
        s.toggle(ids[1]);
        assert_eq!(s.len(), 2);

        s.toggle(ids[0]);
        assert_eq!(s.len(), 1);
        assert!(!s.contains(ids[0]));
    }

    #[test]
    fn iteration_order_is_stable() {
        let (_, _, ids) = scene_with(5);
        let mut a = Selection::new();
        a.set(ids.iter().copied());
        let mut b = Selection::new();
        b.set(ids.iter().rev().copied());
        assert_eq!(a.ids(), b.ids(), "insertion order must not affect ordering");
    }

    /// Undo can delete objects that are still selected.
    #[test]
    fn pruning_drops_objects_that_no_longer_exist() {
        let (mut scene, _, ids) = scene_with(3);
        let mut s = Selection::new();
        s.set(ids.iter().copied());

        scene.remove_object(ids[1]);
        s.prune(&scene);

        assert_eq!(s.len(), 2);
        assert!(!s.contains(ids[1]));
    }

    #[test]
    fn pruning_repairs_a_dangling_active_layer() {
        let (mut scene, layer, _) = scene_with(1);
        let mut s = Selection::new();
        s.set_active_layer(Some(layer));

        scene.add_layer("Other", LayerKind::Normal);
        scene.remove_layer(layer);
        s.prune(&scene);

        assert!(s.active_layer().is_some());
        assert_ne!(s.active_layer(), Some(layer));
    }

    #[test]
    fn an_active_layer_is_chosen_when_none_is_set() {
        let (scene, layer, _) = scene_with(1);
        let mut s = Selection::new();
        assert_eq!(s.ensure_active_layer(&scene), Some(layer));
    }

    /// Drawing must not land on a locked layer.
    #[test]
    fn a_locked_layer_is_not_chosen_as_active() {
        let mut scene = Scene::empty();
        let locked = scene.add_layer("Locked", LayerKind::Normal);
        let usable = scene.add_layer("Usable", LayerKind::Normal);
        scene.update_layer(locked, |l| l.locked = true);

        let mut s = Selection::new();
        s.set_active_layer(Some(locked));
        assert_eq!(
            s.ensure_active_layer(&scene),
            Some(usable),
            "a locked layer should be replaced with a usable one"
        );
    }

    #[test]
    fn a_hidden_layer_is_not_chosen_as_active() {
        let mut scene = Scene::empty();
        let hidden = scene.add_layer("Hidden", LayerKind::Normal);
        let visible = scene.add_layer("Visible", LayerKind::Normal);
        scene.update_layer(hidden, |l| l.visible = false);

        let mut s = Selection::new();
        s.set_active_layer(Some(hidden));
        assert_eq!(s.ensure_active_layer(&scene), Some(visible));
    }

    #[test]
    fn selection_bounds_enclose_everything_selected() {
        let (scene, _, ids) = scene_with(3);
        let mut s = Selection::new();
        s.set([ids[0], ids[2]]);

        let bounds = s.bounds_at(&scene, 0).unwrap();
        assert!((bounds.x0 - 0.0).abs() < 1e-9);
        assert!((bounds.x1 - 50.0).abs() < 1e-9, "got {bounds:?}");
    }

    #[test]
    fn an_empty_selection_has_no_bounds() {
        let (scene, _, _) = scene_with(2);
        assert!(Selection::new().bounds_at(&scene, 0).is_none());
    }

    #[test]
    fn the_properties_header_describes_the_selection() {
        let (scene, _, ids) = scene_with(3);
        let mut s = Selection::new();
        assert_eq!(s.describe(&scene), "Document");

        s.select_one(ids[0]);
        assert_eq!(s.describe(&scene), "Shape");

        s.set(ids.iter().copied());
        assert_eq!(s.describe(&scene), "3 objects selected");
    }

    #[test]
    fn selecting_multiple_layers_selects_artwork_across_all_of_them() {
        let mut scene = Scene::empty();
        let l1 = scene.add_layer("L1", LayerKind::Normal);
        let l2 = scene.add_layer("L2", LayerKind::Normal);
        let s1 = scene
            .add_shape(
                l1,
                ShapeData::filled(KRect::new(0.0, 0.0, 10.0, 10.0).to_path(1e-9), Color::WHITE),
            )
            .unwrap();
        let s2 = scene
            .add_shape(
                l2,
                ShapeData::filled(
                    KRect::new(20.0, 20.0, 30.0, 30.0).to_path(1e-9),
                    Color::WHITE,
                ),
            )
            .unwrap();

        let mut s = Selection::new();
        s.select_layers(&scene, &[l1, l2], 0);
        assert_eq!(s.selected_layers(), &[l1, l2]);
        assert_eq!(s.active_layer(), Some(l1));
        assert!(s.contains(s1));
        assert!(s.contains(s2));
        assert_eq!(s.len(), 2);
    }

    /// A plain click sets the anchor a later Shift+click sweeps from.
    #[test]
    fn selecting_a_layer_sets_the_range_anchor() {
        let (scene, layer, _) = scene_with(1);
        let mut s = Selection::new();
        assert_eq!(s.range_anchor(), None);
        s.select_layer(&scene, layer, 0);
        assert_eq!(s.range_anchor(), Some(layer));
    }

    /// I1's pure range function: every id between the anchor and the clicked
    /// one, inclusive, whichever direction the sweep runs.
    #[test]
    fn layer_range_is_inclusive_both_directions() {
        let order = vec![LayerId(1), LayerId(2), LayerId(3), LayerId(4), LayerId(5)];
        assert_eq!(
            layer_range(&order, LayerId(2), LayerId(4)),
            vec![LayerId(2), LayerId(3), LayerId(4)]
        );
        assert_eq!(
            layer_range(&order, LayerId(4), LayerId(2)),
            vec![LayerId(2), LayerId(3), LayerId(4)],
            "the anchor coming after the clicked layer must still sweep the same range"
        );
        assert_eq!(
            layer_range(&order, LayerId(3), LayerId(3)),
            vec![LayerId(3)],
            "shift-clicking the anchor itself selects just the one row"
        );
    }

    /// An anchor no longer in `order` (e.g. deleted) falls back to a plain
    /// click on whatever was actually clicked, rather than panicking.
    #[test]
    fn layer_range_with_a_missing_anchor_falls_back_to_the_click() {
        let order = vec![LayerId(1), LayerId(2), LayerId(3)];
        assert_eq!(
            layer_range(&order, LayerId(99), LayerId(2)),
            vec![LayerId(2)]
        );
    }

    /// I2: a collapsed folder caught inside the swept range brings its
    /// hidden children along, even though their rows are not drawn.
    #[test]
    fn expand_collapsed_folders_pulls_in_hidden_descendants() {
        let mut scene = Scene::empty();
        let folder = scene.add_layer("Folder", LayerKind::Folder);
        let child = scene.add_layer("Child", LayerKind::Normal);
        scene.update_layer(child, |l| l.parent = Some(folder));
        scene.update_layer(folder, |l| l.collapsed = true);
        let other = scene.add_layer("Other", LayerKind::Normal);

        let expanded = expand_collapsed_folders(&scene, &[folder, other]);
        assert_eq!(expanded, vec![folder, child, other]);
    }

    /// An expanded folder (or anything that is not a folder) is left as-is:
    /// its children are already rows of their own in the swept range.
    #[test]
    fn expand_collapsed_folders_leaves_expanded_folders_alone() {
        let mut scene = Scene::empty();
        let folder = scene.add_layer("Folder", LayerKind::Folder);
        let child = scene.add_layer("Child", LayerKind::Normal);
        scene.update_layer(child, |l| l.parent = Some(folder));

        let expanded = expand_collapsed_folders(&scene, &[folder, child]);
        assert_eq!(expanded, vec![folder, child]);
    }
}
