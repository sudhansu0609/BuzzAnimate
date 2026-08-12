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
            .filter_map(|id| scene.find_object(*id).map(|(_, o)| scene.resolved_bounds(o)))
            .reduce(|a, b| a.union(b))
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
