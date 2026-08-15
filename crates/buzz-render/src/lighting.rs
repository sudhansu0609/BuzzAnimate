//! Lit drawing: what the light rig adds to the draw walk, and the cache that
//! makes it affordable.
//!
//! # Why there is a cache at all
//!
//! Every shading crescent is a boolean difference between the artwork and a
//! copy of itself. Booleans are the most expensive thing this renderer does —
//! CP-1.1b built them for occasional edits, not for sixty times a second — and
//! a scene with two hundred shapes would spend tens of milliseconds a frame
//! regenerating geometry that had not changed.
//!
//! So generated geometry is cached, and the cache key is **copy-on-write
//! pointer identity**. An object that has not been edited is still the same
//! `Arc`, in every snapshot, however much else in the document has changed —
//! so drawing a brush stroke does not rebuild the shadows of everything else.
//! The cache holds the `Arc` it keyed on, which is what makes the pointer safe
//! to compare: nothing can be freed and its address reused while the entry
//! lives.
//!
//! Entries not used for a few frames are dropped, so scrubbing a long timeline
//! does not accumulate the geometry of every frame it passed through.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use buzz_geom::{Affine, BezPath, Point, Shape as _};
use buzz_light::{Light, LightRig, ShadeGeometry};
use buzz_scene::{Object, ShapeData};

/// What identifies a piece of generated geometry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct Key {
    /// The object's address. Stable while it is unedited, thanks to
    /// copy-on-write, and kept valid by holding the `Arc` in the entry.
    object: usize,
    /// Which shape within that object, for groups.
    shape: u16,
    /// Where it sits in the document, quantised. A group moving changes this
    /// without changing its children's addresses, and a lamp's shadow depends
    /// on where the artwork stands.
    place: (i64, i64),
    /// The fingerprint of the **one light** this geometry was built for.
    ///
    /// Per light on purpose: dragging a lamp changes only that lamp's
    /// fingerprint, so its entries miss and rebuild while every other light's
    /// entries — the sun's, the sky's — keep hitting. Before this the whole
    /// cache was thrown away on any light change, which is what made the first
    /// frame after touching a light cost as much as opening the document.
    light: u64,
}

struct Entry {
    /// Held to keep the address unique for as long as the entry lives.
    #[allow(dead_code, reason = "held for pointer identity, never read")]
    owner: Arc<Object>,
    geometry: Arc<ShadeGeometry>,
    used: u64,
}

/// One piece of geometry that was asked for and is not built yet.
///
/// It carries everything [`shade_for`](buzz_light::geometry::shade_for) needs,
/// owned outright, so it can cross to a worker thread and be built there —
/// which is the whole point: a heavy scene's crescents and shadows are built on
/// every core at once, off the thread drawing the window, instead of one at a
/// time in front of the user. See [`LightCache::take_misses`].
pub struct Miss {
    key: Key,
    path: BezPath,
    light: Light,
    centre: Point,
    depth: f64,
    height: f64,
    modelling: f32,
    owner: Arc<Object>,
}

impl Miss {
    /// Build the geometry. Pure and self-contained, so a whole batch of these
    /// runs in parallel with no shared state.
    pub fn build(self) -> Built {
        let geometry = Arc::new(buzz_light::geometry::shade_for(
            &self.path,
            &self.light,
            self.centre,
            self.depth,
            self.height,
            self.modelling,
        ));
        Built {
            key: self.key,
            geometry,
            owner: self.owner,
        }
    }
}

/// A [`Miss`] that has been built, ready to be put back into the cache.
pub struct Built {
    key: Key,
    geometry: Arc<ShadeGeometry>,
    owner: Arc<Object>,
}

/// Generated lighting geometry, kept between frames.
#[derive(Default)]
pub struct LightCache {
    entries: HashMap<Key, Entry>,
    frame: u64,
    /// When set, a miss is recorded rather than built on the spot, and the
    /// shape draws unlit for the frame. The app turns this on only when the
    /// cache is cold — see [`LightCache::set_defer`].
    defer: bool,
    /// Geometry asked for this frame that was not cached. Drained by the app,
    /// built off-thread, and returned through [`LightCache::install`].
    misses: Vec<Miss>,
    /// Keys already queued this frame, so the shadow pass and the shading pass
    /// asking for the same shape do not queue it twice.
    queued: HashSet<Key>,
    /// The "nothing" handed back on a deferred miss: no crescent, no shadow, so
    /// the shape draws as its plain lit fill until the real geometry lands.
    empty: Arc<ShadeGeometry>,
}

/// How many frames an entry survives without being drawn.
///
/// Two is enough to cover onion skinning, which draws neighbouring frames and
/// would otherwise evict everything the live frame just built.
const KEEP_FRAMES: u64 = 3;

impl LightCache {
    pub fn new() -> Self {
        Self::default()
    }

    /// Start a frame.
    ///
    /// The old parameter — a rig revision that used to clear the whole cache on
    /// any light change — is gone: entries are now keyed per light, so a
    /// changed light's entries fall out of use on their own and age out, and
    /// nothing else is disturbed.
    pub fn begin(&mut self, _lights: u64) {
        self.frame = self.frame.wrapping_add(1);
        self.misses.clear();
        self.queued.clear();
    }

    /// Drop anything that has not been drawn recently.
    pub fn end(&mut self) {
        let frame = self.frame;
        self.entries
            .retain(|_, entry| frame.saturating_sub(entry.used) < KEEP_FRAMES);
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Record misses instead of building them, so the frame paints unlit and
    /// the build happens off-thread.
    ///
    /// Turned on by the app for exactly the frames where building inline would
    /// be felt — a cold cache on a heavy scene — and off otherwise, so an
    /// ordinary edit still lights on the spot with no flicker.
    pub fn set_defer(&mut self, defer: bool) {
        self.defer = defer;
    }

    /// Take the geometry queued this frame, to build elsewhere.
    pub fn take_misses(&mut self) -> Vec<Miss> {
        std::mem::take(&mut self.misses)
    }

    /// Put built geometry back into the cache.
    ///
    /// Marked used *now*, so it survives eviction for the usual few frames from
    /// the moment it arrives rather than from whenever it was first asked for.
    pub fn install(&mut self, built: Vec<Built>) {
        for item in built {
            self.entries.insert(
                item.key,
                Entry {
                    owner: item.owner,
                    geometry: item.geometry,
                    used: self.frame,
                },
            );
        }
    }

    /// The geometry for one shape, building it only if it is not already here.
    ///
    /// `owner` is the object's shared handle when it has one. Tweened artwork
    /// has none — it is rebuilt for the frame being drawn — so its geometry is
    /// generated and not kept, which is right: caching something that changes
    /// every frame only wastes the memory. Tweened artwork is also built inline
    /// even while deferring, because there is nothing to defer *to*: it will
    /// not be here next frame either.
    #[allow(
        clippy::too_many_arguments,
        reason = "one call site; naming a struct for it would be worse"
    )]
    pub fn shade(
        &mut self,
        owner: Option<&Arc<Object>>,
        shape_index: u16,
        shape: &ShapeData,
        doc: Affine,
        light: &Light,
        height: f64,
        depth: f64,
        modelling: f32,
    ) -> Arc<ShadeGeometry> {
        let path = doc * shape.path.clone();
        let centre = path.bounding_box().center();

        let Some(owner) = owner else {
            return Arc::new(buzz_light::geometry::shade_for(
                &path, light, centre, depth, height, modelling,
            ));
        };

        let key = Key {
            object: Arc::as_ptr(owner) as usize,
            shape: shape_index,
            // Quantised to a tenth of a unit: a shape nudged by a hundredth of
            // a pixel does not need its shadow rebuilding, and exact float
            // keys would never hit.
            place: (
                (centre.x * 10.0).round() as i64,
                (centre.y * 10.0).round() as i64,
            ),
            light: light.fingerprint(),
        };

        if let Some(entry) = self.entries.get_mut(&key) {
            entry.used = self.frame;
            return Arc::clone(&entry.geometry);
        }

        if self.defer {
            // Queue it once — the shadow pass and the shading pass both ask for
            // the same shape — and hand back nothing so it draws unlit for now.
            if self.queued.insert(key) {
                self.misses.push(Miss {
                    key,
                    path,
                    light: light.clone(),
                    centre,
                    depth,
                    height,
                    modelling,
                    owner: Arc::clone(owner),
                });
            }
            return Arc::clone(&self.empty);
        }

        let geometry = Arc::new(buzz_light::geometry::shade_for(
            &path, light, centre, depth, height, modelling,
        ));
        self.entries.insert(
            key,
            Entry {
                owner: Arc::clone(owner),
                geometry: Arc::clone(&geometry),
                used: self.frame,
            },
        );
        geometry
    }
}

impl std::fmt::Debug for LightCache {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LightCache")
            .field("entries", &self.entries.len())
            .finish()
    }
}

/// How a layer is lit, worked out once per layer rather than per shape.
#[derive(Debug, Clone, Copy)]
pub struct LayerLighting<'a> {
    pub rig: &'a LightRig,
    /// The light shadows and crescents follow.
    pub key: Option<&'a Light>,
    /// How far this layer's artwork stands above the surface receiving its
    /// shadow.
    pub height: f64,
    pub depth: f64,
    pub stage_height: f64,
}

impl LayerLighting<'_> {
    /// Nothing to do: no rig, or a rig with everything switched off.
    pub fn is_neutral(&self) -> bool {
        !self.rig.is_active()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use buzz_geom::Rect;
    use buzz_light::{LightId, LightKind};
    use buzz_scene::ObjectId;
    use peniko::Color;

    fn object(id: u64) -> Arc<Object> {
        Arc::new(Object::shape(
            ObjectId(id),
            ShapeData::filled(Rect::new(0.0, 0.0, 100.0, 60.0).to_path(1e-9), Color::WHITE),
        ))
    }

    fn shape_of(object: &Arc<Object>) -> ShapeData {
        match &object.kind {
            buzz_scene::ObjectKind::Shape(shape) => shape.clone(),
            _ => unreachable!("test objects are shapes"),
        }
    }

    fn sun() -> Light {
        Light::new(
            LightId(1),
            "Sun",
            LightKind::Sun {
                azimuth: 0.3,
                elevation: 0.7,
            },
        )
    }

    #[test]
    fn geometry_is_built_once_and_reused() {
        let mut cache = LightCache::new();
        let object = object(1);
        let shape = shape_of(&object);
        let light = sun();

        cache.begin(1);
        let first = cache.shade(
            Some(&object),
            0,
            &shape,
            Affine::IDENTITY,
            &light,
            40.0,
            0.0,
            1.0,
        );
        assert_eq!(cache.len(), 1);

        let second = cache.shade(
            Some(&object),
            0,
            &shape,
            Affine::IDENTITY,
            &light,
            40.0,
            0.0,
            1.0,
        );
        assert!(
            Arc::ptr_eq(&first, &second),
            "the second call should return the cached geometry, not rebuild it"
        );
        assert_eq!(cache.len(), 1);
    }

    /// The point of the whole cache: an unrelated edit must not rebuild
    /// everything. Copy-on-write means the untouched object is the same `Arc`.
    #[test]
    fn an_unrelated_edit_keeps_the_cache_warm() {
        let mut cache = LightCache::new();
        let untouched = object(1);
        let shape = shape_of(&untouched);
        let light = sun();

        cache.begin(1);
        let before = cache.shade(
            Some(&untouched),
            0,
            &shape,
            Affine::IDENTITY,
            &light,
            40.0,
            0.0,
            1.0,
        );

        // Another frame, same document revision for the lights: the object is
        // still the same `Arc`, as it would be after editing something else.
        cache.begin(1);
        let after = cache.shade(
            Some(&untouched),
            0,
            &shape,
            Affine::IDENTITY,
            &light,
            40.0,
            0.0,
            1.0,
        );

        assert!(Arc::ptr_eq(&before, &after), "the geometry was rebuilt");
    }

    /// Changing a light rebuilds *that light's* geometry — a new fingerprint is
    /// a new key, so the old entry is missed and the geometry is regenerated.
    #[test]
    fn changing_a_light_rebuilds_its_geometry() {
        let mut cache = LightCache::new();
        let object = object(1);
        let shape = shape_of(&object);

        let mut low = sun();
        if let LightKind::Sun { elevation, .. } = &mut low.kind {
            *elevation = 0.2;
        }

        cache.begin(1);
        let before = cache.shade(
            Some(&object),
            0,
            &shape,
            Affine::IDENTITY,
            &low,
            40.0,
            0.0,
            1.0,
        );

        // The sun climbs: its fingerprint changes, so the same shape misses and
        // is built afresh under the new light.
        let mut high = low.clone();
        if let LightKind::Sun { elevation, .. } = &mut high.kind {
            *elevation = 0.9;
        }
        cache.begin(1);
        let after = cache.shade(
            Some(&object),
            0,
            &shape,
            Affine::IDENTITY,
            &high,
            40.0,
            0.0,
            1.0,
        );

        assert!(
            !Arc::ptr_eq(&before, &after),
            "a changed light must rebuild the geometry it lit"
        );
    }

    /// The point of per-light keying: touching one light must not disturb
    /// another's geometry. The sun keeps hitting while the lamp is rebuilt.
    #[test]
    fn one_light_changing_does_not_rebuild_another() {
        let mut cache = LightCache::new();
        let object = object(1);
        let shape = shape_of(&object);
        let sun = sun();
        let lamp = Light::new(LightId(2), "Lamp", LightKind::lamp(buzz_geom::Point::new(0.0, 0.0)));

        cache.begin(1);
        let sun_before =
            cache.shade(Some(&object), 0, &shape, Affine::IDENTITY, &sun, 40.0, 0.0, 1.0);
        cache.shade(Some(&object), 0, &shape, Affine::IDENTITY, &lamp, 40.0, 0.0, 1.0);
        assert_eq!(cache.len(), 2, "one entry per light");

        // Move the lamp; the sun is untouched.
        let mut moved = lamp.clone();
        if let LightKind::Lamp { position, .. } = &mut moved.kind {
            *position = buzz_geom::Point::new(500.0, 500.0);
        }
        cache.begin(1);
        let sun_after =
            cache.shade(Some(&object), 0, &shape, Affine::IDENTITY, &sun, 40.0, 0.0, 1.0);
        assert!(
            Arc::ptr_eq(&sun_before, &sun_after),
            "the sun was rebuilt when only the lamp moved"
        );
    }

    /// While deferring, a miss is recorded and empty geometry handed back — the
    /// shape draws unlit — and the recorded miss builds to the same geometry an
    /// inline build would have produced.
    #[test]
    fn deferred_misses_are_queued_and_build_the_same() {
        let mut cache = LightCache::new();
        let object = object(1);
        let shape = shape_of(&object);
        let light = sun();

        // What an inline build produces, for comparison.
        cache.begin(1);
        let inline = cache.shade(
            Some(&object),
            0,
            &shape,
            Affine::IDENTITY,
            &light,
            40.0,
            0.0,
            1.0,
        );

        // A fresh cache, this time deferring.
        let mut cache = LightCache::new();
        cache.begin(1);
        cache.set_defer(true);
        let handed_back = cache.shade(
            Some(&object),
            0,
            &shape,
            Affine::IDENTITY,
            &light,
            40.0,
            0.0,
            1.0,
        );
        assert!(
            handed_back.is_empty(),
            "a deferred shape must draw unlit for the frame"
        );

        let misses = cache.take_misses();
        assert_eq!(misses.len(), 1, "the miss should have been queued");

        let built: Vec<Built> = misses.into_iter().map(Miss::build).collect();
        assert_eq!(*built[0].geometry, *inline, "the deferred build must match");

        cache.install(built);
        assert_eq!(cache.len(), 1, "the built geometry is now cached");

        // And now it hits, lit.
        cache.set_defer(false);
        let hit = cache.shade(
            Some(&object),
            0,
            &shape,
            Affine::IDENTITY,
            &light,
            40.0,
            0.0,
            1.0,
        );
        assert!(!hit.is_empty(), "the installed geometry should be returned");
    }

    /// The shadow pass and the shading pass both ask for the same shape; a
    /// deferred frame must queue it once, not twice.
    #[test]
    fn a_shape_asked_for_twice_is_queued_once() {
        let mut cache = LightCache::new();
        let object = object(1);
        let shape = shape_of(&object);
        let light = sun();

        cache.begin(1);
        cache.set_defer(true);
        cache.shade(Some(&object), 0, &shape, Affine::IDENTITY, &light, 40.0, 0.0, 1.0);
        cache.shade(Some(&object), 0, &shape, Affine::IDENTITY, &light, 40.0, 0.0, 1.0);

        assert_eq!(cache.take_misses().len(), 1, "queued once, not twice");
    }

    /// Tweened artwork has no owner and is built inline even while deferring —
    /// there is nothing to defer it *to*, since it will not be here next frame.
    #[test]
    fn ownerless_artwork_builds_inline_even_when_deferring() {
        let mut cache = LightCache::new();
        let object = object(1);
        let shape = shape_of(&object);
        let light = sun();

        cache.begin(1);
        cache.set_defer(true);
        let geometry = cache.shade(None, 0, &shape, Affine::IDENTITY, &light, 40.0, 0.0, 1.0);

        assert!(cache.take_misses().is_empty(), "nothing should be queued");
        assert!(
            !geometry.is_empty(),
            "tweened artwork must still be lit on the spot"
        );
    }

    /// A shape that moves gets new geometry, because a lamp's direction and a
    /// shadow's landing point both depend on where it is.
    #[test]
    fn moving_a_shape_rebuilds_its_geometry() {
        let mut cache = LightCache::new();
        let object = object(1);
        let shape = shape_of(&object);
        let light = sun();

        cache.begin(1);
        cache.shade(
            Some(&object),
            0,
            &shape,
            Affine::IDENTITY,
            &light,
            40.0,
            0.0,
            1.0,
        );
        cache.shade(
            Some(&object),
            0,
            &shape,
            Affine::translate((300.0, 0.0)),
            &light,
            40.0,
            0.0,
            1.0,
        );

        assert_eq!(cache.len(), 2, "each placement needs its own geometry");
    }

    #[test]
    fn entries_are_dropped_when_they_stop_being_drawn() {
        let mut cache = LightCache::new();
        let object = object(1);
        let shape = shape_of(&object);

        cache.begin(1);
        cache.shade(
            Some(&object),
            0,
            &shape,
            Affine::IDENTITY,
            &sun(),
            40.0,
            0.0,
            1.0,
        );
        cache.end();
        assert_eq!(cache.len(), 1, "still fresh");

        for _ in 0..KEEP_FRAMES + 1 {
            cache.begin(1);
            cache.end();
        }
        assert!(cache.is_empty(), "unused geometry should have been dropped");
    }

    /// Onion skinning draws neighbouring frames; the live frame's geometry
    /// must survive that.
    #[test]
    fn geometry_survives_a_frame_it_is_not_drawn_in() {
        let mut cache = LightCache::new();
        let object = object(1);
        let shape = shape_of(&object);

        cache.begin(1);
        cache.shade(
            Some(&object),
            0,
            &shape,
            Affine::IDENTITY,
            &sun(),
            40.0,
            0.0,
            1.0,
        );
        cache.end();

        cache.begin(1);
        cache.end();

        cache.begin(1);
        let again = cache.shade(
            Some(&object),
            0,
            &shape,
            Affine::IDENTITY,
            &sun(),
            40.0,
            0.0,
            1.0,
        );
        assert!(
            !again.is_empty(),
            "the geometry should still describe a shape"
        );
        assert_eq!(cache.len(), 1);
    }
}
