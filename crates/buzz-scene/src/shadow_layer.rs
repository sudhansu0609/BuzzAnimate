//! **The Shadow layer**: a row of its own on the main timeline, fixed just
//! under the Camera's, that gives each object on the stage a shadow of its own
//! (user 2026-11-28: "like camera layer let's have a shadow layer, which will
//! show only shadow option like features for all individual objects on stage
//! and can be added keyframe and other such features like camera layer. Also
//! it sits below camera fixed").
//!
//! # What a shadow is here
//!
//! The object's silhouette laid on the ground from its feet — the bottom of
//! the artwork it draws, at its middle across — the way a light's ground
//! shadow is ([`buzz_light::ShadowThrow::Ground`]), but set by hand for each
//! object, with no light needed: which way it falls, how long it is, then an
//! offset, a scale and a skew about the feet, how soft its edge is, how dark,
//! and what colour.
//!
//! # Keys
//!
//! Like the camera's: a key on the Shadow row is the state of every object's
//! shadow at that frame, and the frames between two keys blend from one to the
//! next (the numbers ease; whether a shadow is on holds until the next key).
//! After the last key the shadows hold. An object missing from a key has no
//! shadow there.

use std::collections::BTreeMap;

use buzz_geom::Vec2;
use peniko::Color;

use crate::ObjectId;
use crate::tween::Easing;

/// One object's shadow, at one key.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ObjectShadow {
    /// Drawn at all.
    pub on: bool,
    /// **Which way it falls**, in degrees on the screen: 0 to the right, 90
    /// straight down towards the viewer, 180 to the left.
    pub angle: f64,
    /// **How long it is**, as a fraction of the object's own height.
    pub length: f64,
    /// Moved by this much, in document units, after it is laid down.
    pub offset: Vec2,
    /// Stretched across and along, about the object's feet.
    pub scale_x: f64,
    pub scale_y: f64,
    /// Sheared across, in degrees, about the feet.
    pub skew: f64,
    /// **How soft its edge is**: the blur's radius, in document units.
    pub softness: f64,
    /// **How dark**: `0.0` none, `1.0` solid.
    pub darkness: f32,
    pub color: Color,
}

impl Default for ObjectShadow {
    /// A soft shadow falling down and a little to the right, a third of the
    /// object's height, at not quite half strength — what reads as a figure
    /// standing on the ground under an ordinary light.
    fn default() -> Self {
        Self {
            on: true,
            angle: 70.0,
            length: 0.35,
            offset: Vec2::ZERO,
            scale_x: 1.0,
            scale_y: 1.0,
            skew: 0.0,
            softness: 4.0,
            darkness: 0.45,
            color: Color::BLACK,
        }
    }
}

impl ObjectShadow {
    /// The shadow `t` of the way from `self` to `to`: every number blended,
    /// the colour mixed, and on or off held from `self` (a shadow does not
    /// fade in by switching on half way).
    pub fn lerp(&self, to: &Self, t: f64) -> Self {
        let mix = |a: f64, b: f64| a + (b - a) * t;
        Self {
            on: self.on,
            angle: mix(self.angle, to.angle),
            length: mix(self.length, to.length),
            offset: Vec2::new(mix(self.offset.x, to.offset.x), mix(self.offset.y, to.offset.y)),
            scale_x: mix(self.scale_x, to.scale_x),
            scale_y: mix(self.scale_y, to.scale_y),
            skew: mix(self.skew, to.skew),
            softness: mix(self.softness, to.softness),
            darkness: mix(self.darkness as f64, to.darkness as f64) as f32,
            color: crate::lerp_color(self.color, to.color, t),
        }
    }

    /// The throw a ground shadow of these settings makes: the bearing it
    /// falls along, its length per unit of height, and the adjustment about
    /// the feet. See [`buzz_light::ShadowThrow::Ground`].
    pub fn throw(&self) -> buzz_light::ShadowThrow {
        let (sin, cos) = self.angle.to_radians().sin_cos();
        buzz_light::ShadowThrow::Ground {
            away: Some(Vec2::new(cos, sin)),
            stretch: Some(self.length.max(0.0)),
            lamp: None,
            adjustment: buzz_light::ShadowAdjustment {
                offset: self.offset,
                scale_x: self.scale_x,
                scale_y: self.scale_y,
                rotation: 0.0,
                skew_x: self.skew.to_radians(),
                skew_y: 0.0,
                pivot: buzz_geom::Point::ZERO,
            },
        }
    }
}

/// A key on the Shadow row: every object's shadow at `frame`.
#[derive(Debug, Clone, PartialEq)]
pub struct ShadowKey {
    pub frame: u32,
    /// The shadows, by the object casting them. An object not here has none.
    pub shadows: BTreeMap<ObjectId, ObjectShadow>,
    /// How the frames after this key, up to the next, move between the two.
    pub ease: Easing,
}

impl ShadowKey {
    pub fn new(frame: u32) -> Self {
        Self { frame, shadows: BTreeMap::new(), ease: Easing::Linear }
    }
}

/// The Shadow layer of a scene.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ShadowLayer {
    /// The layer is on the timeline (added with the Shadow button). Off, it
    /// draws nothing and its keys are kept for when it is added back.
    pub enabled: bool,
    /// Its row's eye: shadows drawn or not, the keys untouched.
    pub hidden: bool,
    /// Its row's padlock: no key is added, changed or removed.
    pub locked: bool,
    keys: Vec<ShadowKey>,
}

impl ShadowLayer {
    /// Nothing to keep: never added, and no keys.
    pub fn is_unused(&self) -> bool {
        !self.enabled && self.keys.is_empty()
    }

    /// Shadows are drawn: the layer is added and its eye is open.
    pub fn draws(&self) -> bool {
        self.enabled && !self.hidden && !self.keys.is_empty()
    }

    pub fn keys(&self) -> &[ShadowKey] {
        &self.keys
    }

    pub fn has_key_at(&self, frame: u32) -> bool {
        self.keys.iter().any(|k| k.frame == frame)
    }

    pub fn last_frame(&self) -> u32 {
        self.keys.last().map_or(0, |k| k.frame)
    }

    /// The keys, in frame order, one per frame — as a file stores them.
    pub fn from_keys(enabled: bool, hidden: bool, locked: bool, mut keys: Vec<ShadowKey>) -> Self {
        keys.sort_by_key(|k| k.frame);
        keys.dedup_by_key(|k| k.frame);
        Self { enabled, hidden, locked, keys }
    }

    /// Put `key` in, replacing any at its frame.
    pub fn set_key(&mut self, key: ShadowKey) {
        match self.keys.binary_search_by_key(&key.frame, |k| k.frame) {
            Ok(i) => self.keys[i] = key,
            Err(i) => self.keys.insert(i, key),
        }
    }

    /// Take the key at `frame` out. `false` when there was none.
    pub fn remove_key(&mut self, frame: u32) -> bool {
        let before = self.keys.len();
        self.keys.retain(|k| k.frame != frame);
        self.keys.len() != before
    }

    /// Every key after `frame` moved by `delta` frames — Insert Frame (`+1`)
    /// and Remove Frame (`-1`) on the row. A key moved onto `frame` or before
    /// it by a removal is dropped. `false` when nothing moved.
    pub fn shift_after(&mut self, frame: u32, delta: i32) -> bool {
        let mut changed = false;
        let mut kept = Vec::with_capacity(self.keys.len());
        for mut key in std::mem::take(&mut self.keys) {
            if key.frame > frame {
                let moved = key.frame as i64 + delta as i64;
                changed = true;
                if moved <= frame as i64 {
                    continue;
                }
                key.frame = moved as u32;
            }
            kept.push(key);
        }
        self.keys = kept;
        changed
    }

    /// **Every object's shadow at `frame`**: the key there; between two keys
    /// each object present in both blended by the first key's ease, one in
    /// only the earlier key held; after the last key, the last key's; before
    /// the first, none.
    pub fn state_at(&self, frame: u32) -> BTreeMap<ObjectId, ObjectShadow> {
        let at = self.keys.partition_point(|k| k.frame <= frame);
        let Some(before) = at.checked_sub(1).map(|i| &self.keys[i]) else {
            return BTreeMap::new();
        };
        let Some(after) = self.keys.get(at) else {
            return before.shadows.clone();
        };
        let span = (after.frame - before.frame) as f64;
        let t = before.ease.apply(((frame - before.frame) as f64 / span).clamp(0.0, 1.0));
        before
            .shadows
            .iter()
            .map(|(id, from)| (*id, after.shadows.get(id).map_or(*from, |to| from.lerp(to, t))))
            .collect()
    }

    /// One object's shadow at `frame`, `None` when it has none there.
    pub fn shadow_at(&self, frame: u32, object: ObjectId) -> Option<ObjectShadow> {
        self.state_at(frame).remove(&object).filter(|s| s.on)
    }

    /// **The key a change at `frame` lands on.** With `make_key` (Animate's
    /// auto-keyframe, and always for a row with no keys yet) a key is made at
    /// `frame` from what the shadows show there; otherwise the key governing
    /// `frame` is changed. `None` while the row is locked.
    pub fn key_for_edit(&mut self, frame: u32, make_key: bool) -> Option<&mut ShadowKey> {
        if self.locked {
            return None;
        }
        let governing = self.keys.iter().rposition(|k| k.frame <= frame);
        if make_key || governing.is_none() {
            if !self.has_key_at(frame) {
                let shadows = self.state_at(frame);
                let ease = governing.map_or(Easing::Linear, |i| self.keys[i].ease);
                self.set_key(ShadowKey { frame, shadows, ease });
            }
            return self.keys.iter_mut().find(|k| k.frame == frame);
        }
        governing.map(move |i| &mut self.keys[i])
    }

    /// **Set one object's shadow at `frame`**, through [`Self::key_for_edit`].
    /// `false` while the row is locked.
    pub fn set_shadow(&mut self, frame: u32, object: ObjectId, shadow: ObjectShadow, make_key: bool) -> bool {
        match self.key_for_edit(frame, make_key) {
            Some(key) => {
                key.shadows.insert(object, shadow);
                true
            }
            None => false,
        }
    }

    /// The ease of the key governing `frame` — the Shadow row's tween.
    pub fn set_ease_at(&mut self, frame: u32, ease: Easing) -> bool {
        if self.locked {
            return false;
        }
        match self.keys.iter_mut().rev().find(|k| k.frame <= frame) {
            Some(key) => {
                key.ease = ease;
                true
            }
            None => false,
        }
    }

    /// Drop the shadows of objects that no longer exist.
    pub fn retain_objects(&mut self, alive: impl Fn(ObjectId) -> bool) {
        for key in &mut self.keys {
            key.shadows.retain(|id, _| alive(*id));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn layer_with(keys: &[(u32, f64)]) -> ShadowLayer {
        let mut layer = ShadowLayer { enabled: true, ..Default::default() };
        for &(frame, length) in keys {
            let mut key = ShadowKey::new(frame);
            key.shadows.insert(ObjectId(7), ObjectShadow { length, ..Default::default() });
            layer.set_key(key);
        }
        layer
    }

    #[test]
    fn shadows_blend_between_keys_and_hold_after_the_last() {
        let layer = layer_with(&[(0, 0.2), (10, 1.2)]);
        let length = |f| layer.shadow_at(f, ObjectId(7)).map(|s| s.length);
        assert_eq!(length(0), Some(0.2));
        assert!((length(5).unwrap() - 0.7).abs() < 1e-9, "half way: {:?}", length(5));
        assert_eq!(length(10), Some(1.2));
        assert_eq!(length(40), Some(1.2), "held after the last key");
        assert!(layer_with(&[(5, 0.2)]).shadow_at(2, ObjectId(7)).is_none(), "nothing before the first key");
    }

    #[test]
    fn off_holds_and_a_missing_object_has_no_shadow() {
        let mut layer = layer_with(&[(0, 0.5), (10, 0.5)]);
        layer.set_shadow(10, ObjectId(7), ObjectShadow { on: false, ..Default::default() }, true);
        assert!(layer.shadow_at(9, ObjectId(7)).is_some(), "on until the key that turns it off");
        assert!(layer.shadow_at(10, ObjectId(7)).is_none());
        assert!(layer.shadow_at(3, ObjectId(8)).is_none(), "an object never given one");
    }

    #[test]
    fn an_edit_keys_the_playhead_or_changes_the_governing_key() {
        let mut layer = layer_with(&[(0, 0.2), (10, 1.2)]);
        let longer = ObjectShadow { length: 2.0, ..Default::default() };
        layer.set_shadow(5, ObjectId(7), longer, true);
        assert!(layer.has_key_at(5), "auto-keyframe keys the playhead");
        assert_eq!(layer.keys().len(), 3);
        let mut held = layer_with(&[(0, 0.2), (10, 1.2)]);
        held.set_shadow(5, ObjectId(7), longer, false);
        assert!(!held.has_key_at(5));
        assert_eq!(held.shadow_at(0, ObjectId(7)).unwrap().length, 2.0, "the governing key changed");
        held.locked = true;
        assert!(!held.set_shadow(5, ObjectId(7), ObjectShadow::default(), true), "locked");
    }

    #[test]
    fn inserting_and_removing_frames_moves_the_later_keys() {
        let mut layer = layer_with(&[(0, 0.2), (10, 1.2)]);
        assert!(layer.shift_after(4, 3));
        assert!(layer.has_key_at(13));
        assert!(layer.shift_after(4, -3));
        assert!(layer.has_key_at(10));
    }
}
