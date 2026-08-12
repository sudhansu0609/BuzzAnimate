//! The BuzzAnimate document model.
//!
//! # Copy-on-write, and why the whole architecture rests on it
//!
//! A [`Scene`] is an immutable snapshot built from `Arc`s. Cloning one copies
//! pointers, not artwork, and editing a clone touches only the objects that
//! actually changed — [`Arc::make_mut`] clones a node just when another
//! snapshot still shares it.
//!
//! Three properties fall out of that, and they are the reason for the design:
//!
//! 1. **The renderer never locks.** It holds a snapshot and reads it freely
//!    while the document thread builds the next one. No mutex sits between
//!    editing and drawing, so an edit cannot stall a frame and a frame cannot
//!    stall an edit.
//! 2. **Undo is nearly free.** Old snapshots *are* the history. Undo is
//!    swapping back to a previous `Scene`, not replaying inverse operations —
//!    which is where undo systems usually accumulate bugs.
//! 3. **Background work is safe by construction.** Autosave, thumbnails and
//!    index rebuilds take a snapshot and work from it with no coordination.
//!
//! # Revisions
//!
//! Every edit bumps [`Scene::revision`]. Derived data such as
//! [`SpatialIndex`] records the revision it was built from, so a consumer can
//! tell whether what it holds is current instead of trusting it blindly.

pub mod index;
pub mod layer;
pub mod object;

use std::sync::Arc;

use buzz_geom::{Affine, Rect, Size};
use peniko::Color;
use serde::{Deserialize, Serialize};

pub use index::{IndexEntry, SpatialIndex};
pub use layer::{Layer, LayerHeight, LayerId, LayerKind, LayerStack, MaskGroup};
pub use object::{FillSpec, Object, ObjectId, ObjectKind, ShapeData, StrokeSpec};

/// Stage setup, matching Animate's Document Properties dialog.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct StageProperties {
    /// Stage size in document units. Animate's default is 550×400.
    pub size: Size,
    pub background: Color,
    pub frame_rate: f64,
}

impl Default for StageProperties {
    fn default() -> Self {
        Self {
            size: Size::new(550.0, 400.0),
            background: Color::WHITE,
            frame_rate: 24.0,
        }
    }
}

impl StageProperties {
    /// The stage rectangle, with its origin at the top left as in Animate.
    pub fn stage_rect(&self) -> Rect {
        Rect::new(0.0, 0.0, self.size.width, self.size.height)
    }
}

/// Hands out identifiers that stay unique for the life of a document.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct IdAllocator {
    next: u64,
}

impl Default for IdAllocator {
    fn default() -> Self {
        // Start at 1 so 0 can mean "none" in file formats.
        Self { next: 1 }
    }
}

impl IdAllocator {
    fn take(&mut self) -> u64 {
        let id = self.next;
        self.next += 1;
        id
    }

    /// Ensure future ids do not collide with one already in use.
    ///
    /// Importers assign ids from the source file, so the allocator has to be
    /// told about them or the next new object would reuse an existing id.
    pub fn reserve_above(&mut self, used: u64) {
        self.next = self.next.max(used + 1);
    }
}

/// An immutable snapshot of the document.
#[derive(Debug, Clone, PartialEq)]
pub struct Scene {
    pub stage: StageProperties,
    layers: LayerStack,
    ids: IdAllocator,
    revision: u64,
}

impl Default for Scene {
    fn default() -> Self {
        let mut scene = Self {
            stage: StageProperties::default(),
            layers: LayerStack::new(),
            ids: IdAllocator::default(),
            revision: 0,
        };
        // Animate starts every document with one layer named "Layer_1".
        scene.add_layer("Layer_1", LayerKind::Normal);
        scene
    }
}

impl Scene {
    /// An empty document with no layers at all.
    ///
    /// Importers want this; the editor wants [`Scene::default`].
    pub fn empty() -> Self {
        Self {
            stage: StageProperties::default(),
            layers: LayerStack::new(),
            ids: IdAllocator::default(),
            revision: 0,
        }
    }

    /// Which edit this snapshot represents.
    pub fn revision(&self) -> u64 {
        self.revision
    }

    pub fn layers(&self) -> &LayerStack {
        &self.layers
    }

    /// Mutable access that bumps the revision.
    ///
    /// All edits go through here so no path can change the document without
    /// invalidating derived data.
    pub fn edit_layers(&mut self) -> &mut LayerStack {
        self.revision += 1;
        &mut self.layers
    }

    fn bump(&mut self) {
        self.revision += 1;
    }

    /// Add a layer at the top, as Animate does.
    pub fn add_layer(&mut self, name: impl Into<String>, kind: LayerKind) -> LayerId {
        let id = LayerId(self.ids.take());
        self.layers.push_front(Layer::new(id, name, kind));
        self.bump();
        id
    }

    pub fn remove_layer(&mut self, id: LayerId) -> Option<Arc<Layer>> {
        let removed = self.layers.remove(id);
        if removed.is_some() {
            self.bump();
        }
        removed
    }

    /// Edit a layer in place. Returns false if it does not exist.
    pub fn update_layer(&mut self, id: LayerId, f: impl FnOnce(&mut Layer)) -> bool {
        let ok = self.layers.update(id, f);
        if ok {
            self.bump();
        }
        ok
    }

    /// Place a shape on a layer, returning its new id.
    pub fn add_shape(&mut self, layer: LayerId, shape: ShapeData) -> Option<ObjectId> {
        let id = ObjectId(self.ids.take());
        let object = Arc::new(Object::shape(id, shape));
        self.layers
            .update(layer, |l| l.push_object(object))
            .then(|| {
                self.bump();
                id
            })
    }

    /// Place an already-built object on a layer.
    pub fn add_object(&mut self, layer: LayerId, object: Object) -> Option<ObjectId> {
        let id = object.id;
        self.ids.reserve_above(id.0);
        self.layers
            .update(layer, |l| l.push_object(Arc::new(object)))
            .then(|| {
                self.bump();
                id
            })
    }

    /// Allocate an id without attaching anything to the document yet.
    pub fn next_object_id(&mut self) -> ObjectId {
        ObjectId(self.ids.take())
    }

    /// Find an object and the layer holding it.
    pub fn find_object(&self, id: ObjectId) -> Option<(LayerId, &Arc<Object>)> {
        self.layers
            .iter()
            .find_map(|l| l.find_object(id).map(|o| (l.id, o)))
    }

    pub fn remove_object(&mut self, id: ObjectId) -> Option<Arc<Object>> {
        let layer_id = self.find_object(id).map(|(l, _)| l)?;
        let mut removed = None;
        self.layers.update(layer_id, |l| removed = l.remove_object(id));
        if removed.is_some() {
            self.bump();
        }
        removed
    }

    /// Total leaf shapes across every layer.
    pub fn shape_count(&self) -> usize {
        self.layers
            .iter()
            .flat_map(|l| l.objects.iter())
            .map(|o| o.shape_count())
            .sum()
    }

    /// Bounds of all artwork, ignoring the stage rectangle.
    pub fn content_bounds(&self) -> Option<Rect> {
        self.layers.iter().filter_map(|l| l.bounds()).reduce(|a, b| a.union(b))
    }

    /// Everything the user could reasonably want framed: the stage plus any
    /// artwork sitting out on the pasteboard.
    pub fn fit_bounds(&self) -> Rect {
        match self.content_bounds() {
            Some(content) => content.union(self.stage.stage_rect()),
            None => self.stage.stage_rect(),
        }
    }

    /// Build the entries for a spatial index, in paint order.
    ///
    /// Depth increases towards the front, matching the convention in
    /// `buzz_geom::hit` where later entries are on top.
    pub fn index_entries(&self) -> Vec<IndexEntry> {
        let mut entries = Vec::new();
        let mut depth = 0usize;
        for layer in self.layers.drawable() {
            for object in layer.objects.iter() {
                if !object.visible {
                    continue;
                }
                entries.push(IndexEntry {
                    object: object.id,
                    layer: layer.id,
                    bounds: object.bounds(),
                    depth,
                });
                depth += 1;
            }
        }
        entries
    }

    /// Build a spatial index for this snapshot.
    ///
    /// Cheap enough to call directly for small documents; for large ones run it
    /// on the background pool and hand the result over when it is ready.
    pub fn build_index(&self) -> SpatialIndex {
        SpatialIndex::build(self.index_entries(), self.revision)
    }

    /// Every drawable shape with its resolved world transform, in paint order.
    ///
    /// This is what the renderer consumes.
    pub fn flatten_for_render(&self) -> Vec<(Affine, ShapeData)> {
        let mut out = Vec::new();
        for layer in self.layers.drawable() {
            for object in layer.objects.iter() {
                object.flatten(Affine::IDENTITY, &mut out);
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use buzz_geom::{BezPath, Point, Shape as _};
    use kurbo::Rect as KRect;

    fn square(x: f64, y: f64, size: f64) -> BezPath {
        KRect::new(x, y, x + size, y + size).to_path(1e-9)
    }

    fn scene_with_shapes(n: u64) -> (Scene, LayerId) {
        let mut scene = Scene::default();
        let layer = scene.layers().iter().next().unwrap().id;
        for i in 0..n {
            scene.add_shape(
                layer,
                ShapeData::filled(square(i as f64 * 20.0, 0.0, 10.0), Color::WHITE),
            );
        }
        (scene, layer)
    }

    #[test]
    fn a_new_document_matches_animates_defaults() {
        let scene = Scene::default();
        assert_eq!(scene.stage.size, Size::new(550.0, 400.0));
        assert_eq!(scene.stage.frame_rate, 24.0);
        assert_eq!(scene.layers().len(), 1, "Animate starts with one layer");
        assert_eq!(scene.layers().iter().next().unwrap().name, "Layer_1");
    }

    #[test]
    fn every_edit_bumps_the_revision() {
        let mut scene = Scene::default();
        let layer = scene.layers().iter().next().unwrap().id;

        let r0 = scene.revision();
        scene.add_shape(layer, ShapeData::filled(square(0.0, 0.0, 10.0), Color::WHITE));
        let r1 = scene.revision();
        assert!(r1 > r0);

        scene.update_layer(layer, |l| l.name = "renamed".into());
        assert!(scene.revision() > r1);
    }

    #[test]
    fn a_failed_edit_does_not_bump_the_revision() {
        let mut scene = Scene::default();
        let before = scene.revision();
        assert!(!scene.update_layer(LayerId(9999), |l| l.name = "nope".into()));
        assert_eq!(scene.revision(), before);
        assert!(scene.remove_layer(LayerId(9999)).is_none());
        assert_eq!(scene.revision(), before);
    }

    #[test]
    fn ids_are_unique_across_layers_and_objects() {
        let (mut scene, layer) = scene_with_shapes(5);
        let extra = scene.add_layer("Another", LayerKind::Normal);
        let shape = scene
            .add_shape(extra, ShapeData::filled(square(0.0, 0.0, 5.0), Color::WHITE))
            .unwrap();

        assert_ne!(layer.0, extra.0);
        assert!(scene.find_object(shape).is_some());
    }

    /// Importers assign their own ids; the allocator must not later reuse them.
    #[test]
    fn imported_ids_are_reserved() {
        let mut scene = Scene::empty();
        let layer = scene.add_layer("L", LayerKind::Normal);

        let imported = Object::shape(
            ObjectId(5000),
            ShapeData::filled(square(0.0, 0.0, 10.0), Color::WHITE),
        );
        scene.add_object(layer, imported).unwrap();

        let fresh = scene.next_object_id();
        assert!(
            fresh.0 > 5000,
            "a new id ({}) must not collide with an imported one",
            fresh.0
        );
    }

    /// The property the whole architecture depends on.
    #[test]
    fn a_snapshot_is_unaffected_by_later_edits() {
        let (mut scene, layer) = scene_with_shapes(10);
        let snapshot = scene.clone();
        let before = snapshot.shape_count();

        scene.add_shape(layer, ShapeData::filled(square(999.0, 0.0, 10.0), Color::WHITE));
        scene.update_layer(layer, |l| l.name = "changed".into());

        assert_eq!(snapshot.shape_count(), before, "the snapshot changed");
        assert_eq!(scene.shape_count(), before + 1);
        assert_eq!(snapshot.layers().get(layer).unwrap().name, "Layer_1");
        assert!(snapshot.revision() < scene.revision());
    }

    /// Undo as snapshot swapping, which is the point of the design.
    #[test]
    fn undo_is_just_restoring_an_earlier_snapshot() {
        let (mut scene, layer) = scene_with_shapes(3);
        let history: Vec<Scene> = (0..4)
            .map(|i| {
                let snap = scene.clone();
                scene.add_shape(
                    layer,
                    ShapeData::filled(square(i as f64, 100.0, 5.0), Color::WHITE),
                );
                snap
            })
            .collect();

        assert_eq!(scene.shape_count(), 7);
        let restored = history[0].clone();
        assert_eq!(restored.shape_count(), 3, "undo should return to 3 shapes");
        // And the later snapshots are still intact and distinct.
        assert_eq!(history[2].shape_count(), 5);
    }

    /// Snapshotting is `O(1)`, not `O(objects)`.
    ///
    /// Cloning a `Scene` clones the single `Arc` wrapping the layer list, so
    /// nothing per-object happens at all — the individual object refcounts are
    /// untouched. Copy-on-write only descends into a layer, and then into an
    /// object, at the moment one is actually edited.
    #[test]
    fn cloning_a_scene_shares_structure_wholesale() {
        let (scene, layer) = scene_with_shapes(50);
        let object = Arc::clone(scene.layers().get(layer).unwrap().objects.first().unwrap());
        let before = Arc::strong_count(&object);

        let clones: Vec<Scene> = (0..20).map(|_| scene.clone()).collect();

        assert_eq!(
            Arc::strong_count(&object),
            before,
            "snapshotting should not touch per-object refcounts at all"
        );
        assert_eq!(clones.len(), 20);
        assert_eq!(clones[0].shape_count(), 50);
    }

    /// ...and copy-on-write descends only when an edit forces it.
    #[test]
    fn editing_a_snapshot_forks_only_the_touched_layer() {
        let (mut scene, layer) = scene_with_shapes(50);
        let snapshot = scene.clone();

        let layer_before = Arc::as_ptr(snapshot.layers().get(layer).unwrap());
        scene.update_layer(layer, |l| l.name = "edited".into());
        let layer_after = Arc::as_ptr(scene.layers().get(layer).unwrap());

        assert_ne!(
            layer_before, layer_after,
            "the edited layer should have been forked"
        );
        assert_eq!(
            Arc::as_ptr(snapshot.layers().get(layer).unwrap()),
            layer_before,
            "the snapshot must still point at the original layer"
        );
    }

    /// Editing one object must leave its neighbours as the same allocations.
    #[test]
    fn an_edit_touches_only_what_changed() {
        let (mut scene, layer) = scene_with_shapes(20);
        let untouched_before =
            Arc::as_ptr(scene.layers().get(layer).unwrap().objects.last().unwrap());

        scene.update_layer(layer, |l| {
            let objects = Arc::make_mut(&mut l.objects);
            let first = Arc::make_mut(&mut objects[0]);
            first.transform = Affine::translate((5.0, 5.0));
        });

        let untouched_after =
            Arc::as_ptr(scene.layers().get(layer).unwrap().objects.last().unwrap());
        assert_eq!(
            untouched_before, untouched_after,
            "an unrelated object was reallocated"
        );
    }

    #[test]
    fn snapshotting_a_large_scene_is_cheap() {
        let (scene, _) = scene_with_shapes(20_000);

        let start = std::time::Instant::now();
        let snapshots: Vec<Scene> = (0..100).map(|_| scene.clone()).collect();
        let elapsed = start.elapsed();

        assert_eq!(snapshots.len(), 100);
        // 100 snapshots of a 20k-object scene. Generous, to catch an accidental
        // deep copy rather than to police small drift.
        assert!(
            elapsed.as_millis() < 500,
            "100 snapshots of 20k objects took {elapsed:?}; \
             cloning is probably deep-copying"
        );
    }

    #[test]
    fn hidden_layers_and_objects_are_excluded_from_rendering() {
        let mut scene = Scene::empty();
        let visible = scene.add_layer("Visible", LayerKind::Normal);
        let hidden = scene.add_layer("Hidden", LayerKind::Normal);

        scene.add_shape(visible, ShapeData::filled(square(0.0, 0.0, 10.0), Color::WHITE));
        let ghost = scene
            .add_shape(hidden, ShapeData::filled(square(50.0, 0.0, 10.0), Color::WHITE))
            .unwrap();
        scene.update_layer(hidden, |l| l.visible = false);

        assert_eq!(scene.flatten_for_render().len(), 1);
        assert!(
            !scene.index_entries().iter().any(|e| e.object == ghost),
            "a hidden layer's objects must not be indexed"
        );
    }

    #[test]
    fn folders_contribute_no_artwork_to_the_render() {
        let mut scene = Scene::empty();
        let folder = scene.add_layer("Folder", LayerKind::Folder);
        // Even if something were somehow attached to a folder.
        scene.add_shape(folder, ShapeData::filled(square(0.0, 0.0, 10.0), Color::WHITE));
        assert!(scene.flatten_for_render().is_empty());
    }

    #[test]
    fn index_depth_increases_towards_the_front() {
        let mut scene = Scene::empty();
        let back = scene.add_layer("Back", LayerKind::Normal);
        let front = scene.add_layer("Front", LayerKind::Normal);

        let back_shape = scene
            .add_shape(back, ShapeData::filled(square(0.0, 0.0, 10.0), Color::WHITE))
            .unwrap();
        let front_shape = scene
            .add_shape(front, ShapeData::filled(square(0.0, 0.0, 10.0), Color::WHITE))
            .unwrap();

        let entries = scene.index_entries();
        let depth_of = |id: ObjectId| entries.iter().find(|e| e.object == id).unwrap().depth;
        assert!(
            depth_of(front_shape) > depth_of(back_shape),
            "the layer above must index at a greater depth"
        );
    }

    #[test]
    fn the_index_matches_the_snapshot_revision() {
        let (mut scene, layer) = scene_with_shapes(10);
        let index = scene.build_index();
        assert!(index.is_current_for(scene.revision()));

        scene.add_shape(layer, ShapeData::filled(square(0.0, 500.0, 10.0), Color::WHITE));
        assert!(
            !index.is_current_for(scene.revision()),
            "the index should now report itself stale"
        );
    }

    /// The index must be buildable off the UI thread from a snapshot, with no
    /// locking and no coordination. That is the whole reason the document model
    /// is immutable.
    #[test]
    fn an_index_can_be_rebuilt_on_another_thread_while_editing_continues() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<Scene>();
        assert_send_sync::<SpatialIndex>();

        let (mut scene, layer) = scene_with_shapes(5_000);
        let snapshot = scene.clone();
        let snapshot_revision = snapshot.revision();

        // Hand the snapshot to a worker...
        let worker = std::thread::spawn(move || snapshot.build_index());

        // ...and keep editing the document meanwhile.
        for i in 0..100 {
            scene.add_shape(
                layer,
                ShapeData::filled(square(i as f64, 900.0, 4.0), Color::WHITE),
            );
        }

        let index = worker.join().expect("index build panicked");

        assert_eq!(index.len(), 5_000, "the index describes the snapshot");
        assert!(index.is_current_for(snapshot_revision));
        assert!(
            !index.is_current_for(scene.revision()),
            "concurrent edits should leave the index detectably stale"
        );
        assert_eq!(scene.shape_count(), 5_100);
    }

    #[test]
    fn index_queries_find_the_right_objects() {
        let (scene, _) = scene_with_shapes(200);
        let index = scene.build_index();
        assert_eq!(index.len(), 200);

        let hits = index.query_point(Point::new(45.0, 5.0));
        assert_eq!(hits.len(), 1, "expected exactly one shape at x=45");
    }

    #[test]
    fn removing_an_object_finds_it_on_any_layer() {
        let mut scene = Scene::empty();
        let a = scene.add_layer("A", LayerKind::Normal);
        let b = scene.add_layer("B", LayerKind::Normal);
        scene.add_shape(a, ShapeData::filled(square(0.0, 0.0, 10.0), Color::WHITE));
        let target = scene
            .add_shape(b, ShapeData::filled(square(20.0, 0.0, 10.0), Color::WHITE))
            .unwrap();

        assert!(scene.remove_object(target).is_some());
        assert!(scene.find_object(target).is_none());
        assert_eq!(scene.shape_count(), 1);
        assert!(scene.remove_object(target).is_none(), "removing twice is a no-op");
    }

    #[test]
    fn fit_bounds_always_includes_the_stage() {
        let scene = Scene::default();
        assert_eq!(scene.fit_bounds(), scene.stage.stage_rect());

        let (mut scene, layer) = scene_with_shapes(0);
        scene.add_shape(
            layer,
            ShapeData::filled(square(-500.0, -500.0, 10.0), Color::WHITE),
        );
        let fit = scene.fit_bounds();
        assert!(fit.x0 <= -500.0, "pasteboard artwork should be included");
        assert!(
            fit.x1 >= scene.stage.size.width,
            "the stage should still be included"
        );
    }
}
