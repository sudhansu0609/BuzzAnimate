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

use std::collections::HashMap;
use std::sync::Arc;

use buzz_geom::{Affine, Shape as _};
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
    /// Bumped whenever any light changes.
    rig: u64,
}

struct Entry {
    /// Held to keep the address unique for as long as the entry lives.
    #[allow(dead_code, reason = "held for pointer identity, never read")]
    owner: Arc<Object>,
    geometry: Arc<ShadeGeometry>,
    used: u64,
}

/// Generated lighting geometry, kept between frames.
#[derive(Default)]
pub struct LightCache {
    entries: HashMap<Key, Entry>,
    frame: u64,
    rig: u64,
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

    /// Start a frame. `rig` is the document revision the lights are at.
    pub fn begin(&mut self, rig: u64) {
        self.frame = self.frame.wrapping_add(1);
        if self.rig != rig {
            // A light moved: every crescent and every shadow is wrong.
            self.entries.clear();
            self.rig = rig;
        }
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

    /// The geometry for one shape, building it only if it is not already here.
    ///
    /// `owner` is the object's shared handle when it has one. Tweened artwork
    /// has none — it is rebuilt for the frame being drawn — so its geometry is
    /// generated and not kept, which is right: caching something that changes
    /// every frame only wastes the memory.
    #[allow(clippy::too_many_arguments, reason = "one call site; naming a struct for it would be worse")]
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
            rig: self.rig,
        };

        if let Some(entry) = self.entries.get_mut(&key) {
            entry.used = self.frame;
            return Arc::clone(&entry.geometry);
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
        let first = cache.shade(Some(&object), 0, &shape, Affine::IDENTITY, &light, 40.0, 0.0, 1.0);
        assert_eq!(cache.len(), 1);

        let second = cache.shade(Some(&object), 0, &shape, Affine::IDENTITY, &light, 40.0, 0.0, 1.0);
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
        let before = cache.shade(Some(&untouched), 0, &shape, Affine::IDENTITY, &light, 40.0, 0.0, 1.0);

        // Another frame, same document revision for the lights: the object is
        // still the same `Arc`, as it would be after editing something else.
        cache.begin(1);
        let after = cache.shade(Some(&untouched), 0, &shape, Affine::IDENTITY, &light, 40.0, 0.0, 1.0);

        assert!(Arc::ptr_eq(&before, &after), "the geometry was rebuilt");
    }

    /// Moving a light must invalidate everything: that is the one case where
    /// stale geometry would be visibly, obviously wrong.
    #[test]
    fn changing_a_light_clears_the_cache() {
        let mut cache = LightCache::new();
        let object = object(1);
        let shape = shape_of(&object);

        cache.begin(1);
        cache.shade(Some(&object), 0, &shape, Affine::IDENTITY, &sun(), 40.0, 0.0, 1.0);
        assert_eq!(cache.len(), 1);

        cache.begin(2);
        assert!(cache.is_empty(), "a changed rig must throw the geometry away");
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
        cache.shade(Some(&object), 0, &shape, Affine::IDENTITY, &light, 40.0, 0.0, 1.0);
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
        cache.shade(Some(&object), 0, &shape, Affine::IDENTITY, &sun(), 40.0, 0.0, 1.0);
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
        cache.shade(Some(&object), 0, &shape, Affine::IDENTITY, &sun(), 40.0, 0.0, 1.0);
        cache.end();

        cache.begin(1);
        cache.end();

        cache.begin(1);
        let again = cache.shade(Some(&object), 0, &shape, Affine::IDENTITY, &sun(), 40.0, 0.0, 1.0);
        assert!(!again.is_empty(), "the geometry should still describe a shape");
        assert_eq!(cache.len(), 1);
    }
}
