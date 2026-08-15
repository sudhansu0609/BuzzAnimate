//! Keyframed lights: a sun that swings through a shot, a lamp that brightens.
//!
//! # Why per light, not per rig
//!
//! Each [`Light`] carries its own optional [`LightTrack`], mirroring the way the
//! camera carries a `CameraTrack`. Keyframing the whole rig as one unit would
//! stop lights animating on independent timings, and — worse — would bring back
//! the all-or-nothing cache clear that once made the first lit frame cost a
//! third of a second: a single moving light would rebuild the geometry of every
//! *other* light too.
//!
//! # How this stays affordable
//!
//! The shading cache is keyed by [`Light::fingerprint`], per light. The renderer
//! resolves the rig to concrete light values at the frame being drawn
//! ([`LightRig::resolved_at`]) *before* the cache sees it, so:
//!
//! * a **static** light resolves to the same values every frame — same
//!   fingerprint, cache hit;
//! * an **animating** light resolves to new values — new fingerprint, rebuild,
//!   which is unavoidable and by construction;
//! * animating light A leaves light B's entries alone, because the keys are per
//!   light.
//!
//! That is the whole design, and it falls out of the cache that already exists.

use crate::{Light, LightKind, mix};
use buzz_geom::Point;
use peniko::Color;
use serde::{Deserialize, Serialize};

/// A light's animatable state at one keyframe.
///
/// It carries the whole animatable snapshot — the colour, the intensity, the
/// crescent softness, and the kind's own numbers — so interpolation is one
/// blend of two keys rather than a scatter of per-field tracks. The static
/// fields (`id`, `name`, `enabled`, `shadows`) stay on the base [`Light`].
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct LightKey {
    pub frame: u32,
    pub color: Color,
    pub intensity: f32,
    pub softness: f64,
    pub standing_height: f64,
    pub shadow_strength: f32,
    /// The kind and its numbers — azimuth/elevation for a sun, position/height/
    /// radius for a lamp, the horizon colour for a sky. Interpolated within a
    /// matching variant.
    pub kind: LightKind,
}

impl LightKey {
    /// A key at `frame` capturing `light`'s current animatable state.
    pub fn from_light(frame: u32, light: &Light) -> Self {
        Self {
            frame,
            color: light.color,
            intensity: light.intensity,
            softness: light.softness,
            standing_height: light.standing_height,
            shadow_strength: light.shadow_strength,
            kind: light.kind,
        }
    }
}

/// A light's keyframes over the timeline.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct LightTrack {
    /// Off by default: a light with keys but a disabled track holds its base
    /// value, exactly as a disabled camera does.
    pub enabled: bool,
    /// Sorted by frame.
    keys: Vec<LightKey>,
}

impl LightTrack {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn keys(&self) -> &[LightKey] {
        &self.keys
    }

    pub fn is_empty(&self) -> bool {
        self.keys.is_empty()
    }

    pub fn has_key_at(&self, frame: u32) -> bool {
        self.keys.iter().any(|k| k.frame == frame)
    }

    pub fn last_frame(&self) -> u32 {
        self.keys.last().map(|k| k.frame).unwrap_or(0)
    }

    /// Does this track actually animate the light? False when off or empty, so
    /// the resolver can skip it and hand back the base light untouched.
    pub fn animates(&self) -> bool {
        self.enabled && !self.keys.is_empty()
    }

    /// Add or replace the key at `key.frame`, keeping the list sorted.
    pub fn set_key(&mut self, key: LightKey) {
        match self.keys.iter().position(|k| k.frame == key.frame) {
            Some(index) => self.keys[index] = key,
            None => {
                let at = self.keys.partition_point(|k| k.frame < key.frame);
                self.keys.insert(at, key);
            }
        }
    }

    pub fn remove_key(&mut self, frame: u32) -> bool {
        let before = self.keys.len();
        self.keys.retain(|k| k.frame != frame);
        self.keys.len() != before
    }

    pub fn clear(&mut self) {
        self.keys.clear();
    }

    /// The light as it stands at `frame`, interpolating between keys.
    ///
    /// `base` supplies the static fields and the fallback when the track is
    /// inert. Outside the keyed range the ends hold, exactly as the camera does,
    /// so a light does not snap to black before its first key.
    pub fn state_at(&self, frame: u32, base: &Light) -> Light {
        if !self.animates() {
            return base.clone();
        }
        let key = self.resolved_key(frame);
        let mut light = base.clone();
        light.color = key.color;
        light.intensity = key.intensity;
        light.softness = key.softness;
        light.standing_height = key.standing_height;
        light.shadow_strength = key.shadow_strength;
        light.kind = key.kind;
        light
    }

    /// The interpolated key at `frame`.
    fn resolved_key(&self, frame: u32) -> LightKey {
        if self.keys.len() == 1 {
            return self.keys[0];
        }
        let first = self.keys[0];
        if frame <= first.frame {
            return first;
        }
        let last = self.keys[self.keys.len() - 1];
        if frame >= last.frame {
            return last;
        }

        let after = self.keys.partition_point(|k| k.frame <= frame);
        let a = self.keys[after - 1];
        let b = self.keys[after];
        let span = (b.frame - a.frame) as f64;
        let t = if span > 0.0 {
            (frame - a.frame) as f64 / span
        } else {
            0.0
        };
        lerp_key(&a, &b, t)
    }
}

fn lerp(a: f64, b: f64, t: f64) -> f64 {
    a + (b - a) * t
}

/// Interpolate by the shortest way round the circle — the same rule the camera
/// uses, so a sun swinging past due-south does not take the long route.
fn lerp_angle(a: f64, b: f64, t: f64) -> f64 {
    let tau = std::f64::consts::TAU;
    let mut delta = (b - a) % tau;
    if delta > tau / 2.0 {
        delta -= tau;
    } else if delta < -tau / 2.0 {
        delta += tau;
    }
    a + delta * t
}

fn lerp_key(a: &LightKey, b: &LightKey, t: f64) -> LightKey {
    LightKey {
        frame: a.frame,
        color: mix(a.color, b.color, t as f32),
        intensity: lerp(a.intensity as f64, b.intensity as f64, t) as f32,
        softness: lerp(a.softness, b.softness, t),
        standing_height: lerp(a.standing_height, b.standing_height, t),
        shadow_strength: lerp(a.shadow_strength as f64, b.shadow_strength as f64, t) as f32,
        kind: lerp_kind(a.kind, b.kind, t),
    }
}

/// Interpolate two light kinds. Keys of one light are always the same variant —
/// you cannot animate a sun into a lamp — so matching variants interpolate and
/// a mismatch (only reachable through a corrupt file) holds the first.
fn lerp_kind(a: LightKind, b: LightKind, t: f64) -> LightKind {
    match (a, b) {
        (
            LightKind::Sun {
                azimuth: az_a,
                elevation: el_a,
            },
            LightKind::Sun {
                azimuth: az_b,
                elevation: el_b,
            },
        ) => LightKind::Sun {
            azimuth: lerp_angle(az_a, az_b, t),
            elevation: lerp(el_a, el_b, t),
        },
        (LightKind::Sky { horizon: h_a }, LightKind::Sky { horizon: h_b }) => LightKind::Sky {
            horizon: mix(h_a, h_b, t as f32),
        },
        (
            LightKind::Lamp {
                position: p_a,
                height: h_a,
                radius: r_a,
            },
            LightKind::Lamp {
                position: p_b,
                height: h_b,
                radius: r_b,
            },
        ) => LightKind::Lamp {
            position: Point::new(lerp(p_a.x, p_b.x, t), lerp(p_a.y, p_b.y, t)),
            height: lerp(h_a, h_b, t),
            radius: lerp(r_a, r_b, t),
        },
        // Mismatched variants: hold the first. Not reachable from the editor.
        _ => a,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::LightId;

    fn sun_light() -> Light {
        Light::new(
            LightId(1),
            "Sun",
            LightKind::Sun {
                azimuth: 0.0,
                elevation: 0.5,
            },
        )
    }

    fn key(frame: u32, elevation: f64, intensity: f32) -> LightKey {
        LightKey {
            frame,
            color: Color::WHITE,
            intensity,
            softness: 0.35,
            standing_height: 40.0,
            shadow_strength: 0.45,
            kind: LightKind::Sun {
                azimuth: 0.0,
                elevation,
            },
        }
    }

    #[test]
    fn an_inert_track_holds_the_base_light() {
        let base = sun_light();
        let mut track = LightTrack::new();
        track.set_key(key(0, 1.0, 2.0));
        // Enabled is false, so nothing animates.
        assert!(!track.animates());
        let resolved = track.state_at(5, &base);
        assert_eq!(resolved.intensity, base.intensity);
    }

    #[test]
    fn a_single_key_holds_for_every_frame() {
        let base = sun_light();
        let mut track = LightTrack::new();
        track.enabled = true;
        track.set_key(key(10, 0.9, 3.0));
        for frame in [0, 10, 999] {
            assert_eq!(track.state_at(frame, &base).intensity, 3.0);
        }
    }

    #[test]
    fn intensity_interpolates_between_keys() {
        let base = sun_light();
        let mut track = LightTrack::new();
        track.enabled = true;
        track.set_key(key(0, 0.5, 0.0));
        track.set_key(key(10, 0.5, 2.0));
        assert!((track.state_at(5, &base).intensity - 1.0).abs() < 1e-6);
    }

    #[test]
    fn the_sun_elevation_interpolates() {
        let base = sun_light();
        let mut track = LightTrack::new();
        track.enabled = true;
        track.set_key(key(0, 0.2, 1.0));
        track.set_key(key(10, 0.8, 1.0));
        let mid = track.state_at(5, &base);
        match mid.kind {
            LightKind::Sun { elevation, .. } => {
                assert!((elevation - 0.5).abs() < 1e-9, "got {elevation}")
            }
            _ => panic!("kind changed"),
        }
    }

    #[test]
    fn values_hold_outside_the_keyed_range() {
        let base = sun_light();
        let mut track = LightTrack::new();
        track.enabled = true;
        track.set_key(key(10, 0.5, 1.0));
        track.set_key(key(20, 0.5, 3.0));
        assert_eq!(track.state_at(0, &base).intensity, 1.0, "holds the first");
        assert_eq!(track.state_at(999, &base).intensity, 3.0, "holds the last");
    }

    #[test]
    fn keys_stay_sorted() {
        let mut track = LightTrack::new();
        for frame in [30, 10, 20, 0] {
            track.set_key(key(frame, 0.5, 1.0));
        }
        let frames: Vec<u32> = track.keys().iter().map(|k| k.frame).collect();
        assert_eq!(frames, vec![0, 10, 20, 30]);
    }
}
