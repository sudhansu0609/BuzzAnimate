//! What is currently selected.
//!
//! Animate keeps selection separate from the document: selecting nothing is a
//! normal state, selection survives most edits, and it is *not* saved with the
//! file. Modelling it outside [`buzz_scene::Scene`] keeps it out of the undo
//! history too, which matches how Animate behaves — Ctrl+Z undoes your last
//! edit, not your last click.

use std::collections::BTreeSet;

use buzz_geom::Rect;
use buzz_scene::{LayerId, ObjectId, Scene};

/// The current selection.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Selection {
    /// `BTreeSet` so iteration order is stable, which keeps grouping and
    /// z-order operations deterministic.
    objects: BTreeSet<ObjectId>,
    /// The layer new artwork goes on. Animate always has exactly one.
    active_layer: Option<LayerId>,
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
        self.objects.clear();
        self.objects.insert(id);
    }

    pub fn add(&mut self, id: ObjectId) {
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
        self.objects = ids.into_iter().collect();
    }

    pub fn extend(&mut self, ids: impl IntoIterator<Item = ObjectId>) {
        self.objects.extend(ids);
    }

    pub fn active_layer(&self) -> Option<LayerId> {
        self.active_layer
    }

    pub fn set_active_layer(&mut self, layer: Option<LayerId>) {
        self.active_layer = layer;
    }

    /// Drop anything no longer in the document.
    ///
    /// Undo can remove objects that are still selected; without this the
    /// selection would keep referring to them and transform handles would be
    /// drawn around nothing.
    pub fn prune(&mut self, scene: &Scene) {
        self.objects.retain(|id| scene.find_object(*id).is_some());
        if let Some(layer) = self.active_layer
            && scene.layers().get(layer).is_none()
        {
            self.active_layer = scene.layers().iter().next().map(|l| l.id);
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

    /// Combined bounds of the selection, for transform handles.
    ///
    /// Goes through the scene rather than [`Object::bounds`] so that a
    /// selected symbol instance reports the extents of the artwork inside it.
    pub fn bounds(&self, scene: &Scene) -> Option<Rect> {
        self.objects
            .iter()
            .filter_map(|id| {
                scene
                    .find_object(*id)
                    .map(|(_, o)| scene.resolved_bounds(o))
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
        self.objects.clear();

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

        let bounds = s.bounds(&scene).unwrap();
        assert!((bounds.x0 - 0.0).abs() < 1e-9);
        assert!((bounds.x1 - 50.0).abs() < 1e-9, "got {bounds:?}");
    }

    #[test]
    fn an_empty_selection_has_no_bounds() {
        let (scene, _, _) = scene_with(2);
        assert!(Selection::new().bounds(&scene).is_none());
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
}
