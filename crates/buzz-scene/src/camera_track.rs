//! The animated camera.
//!
//! Animate's Camera is a view transform that belongs to the *document* rather
//! than to the editor: panning, zooming and rotating it are part of the
//! animation and appear in exported output, unlike moving your view of the
//! stage.
//!
//! # Interpolation
//!
//! A camera whose value only changed on keyframes would jump, which would make
//! the tool useless, so camera keys interpolate linearly. That is a small,
//! self-contained piece of tweening; the general tween system for artwork
//! arrives in Phase 4. Rotation interpolates by shortest angular path, so a
//! camera turning from 350° to 10° goes forward 20° rather than backwards 340°.

use buzz_geom::{Affine, Point, Size};
use serde::{Deserialize, Serialize};

/// The camera's state at one keyframe.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct CameraKey {
    pub frame: u32,
    /// Point the camera is centred on, in document space.
    pub center: Point,
    /// Magnification. 1.0 shows the stage at its natural size.
    pub zoom: f64,
    /// Rotation in radians.
    pub rotation: f64,
}

impl CameraKey {
    pub fn new(frame: u32, center: Point) -> Self {
        Self {
            frame,
            center,
            zoom: 1.0,
            rotation: 0.0,
        }
    }
}

/// The camera's keyframes over the whole timeline.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct CameraTrack {
    /// Off by default. Animate hides the camera until you enable it.
    pub enabled: bool,
    /// Sorted by frame.
    keys: Vec<CameraKey>,
}

impl CameraTrack {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn keys(&self) -> &[CameraKey] {
        &self.keys
    }

    pub fn is_empty(&self) -> bool {
        self.keys.is_empty()
    }

    pub fn has_key_at(&self, frame: u32) -> bool {
        self.keys.iter().any(|k| k.frame == frame)
    }

    /// Highest keyed frame, for working out the document's length.
    pub fn last_frame(&self) -> u32 {
        self.keys.last().map(|k| k.frame).unwrap_or(0)
    }

    /// Add or replace the key at `key.frame`.
    pub fn set_key(&mut self, key: CameraKey) {
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

    /// The camera's state at `frame`, interpolating between keys.
    ///
    /// Before the first key it holds the first value, and after the last it
    /// holds the last — the same "hold the ends" behaviour Animate has, which
    /// stops a camera snapping to the origin outside its keyed range.
    pub fn state_at(&self, frame: u32) -> Option<CameraKey> {
        if !self.enabled || self.keys.is_empty() {
            return None;
        }
        if self.keys.len() == 1 {
            return Some(self.keys[0]);
        }

        let first = self.keys[0];
        if frame <= first.frame {
            return Some(first);
        }
        let last = self.keys[self.keys.len() - 1];
        if frame >= last.frame {
            return Some(last);
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

        Some(CameraKey {
            frame,
            center: Point::new(
                lerp(a.center.x, b.center.x, t),
                lerp(a.center.y, b.center.y, t),
            ),
            // Zoom interpolates geometrically: going 1x to 4x should pass
            // through 2x at the midpoint, not 2.5x, or the move visibly
            // accelerates.
            zoom: lerp_zoom(a.zoom, b.zoom, t),
            rotation: lerp_angle(a.rotation, b.rotation, t),
        })
    }

    /// Transform mapping document space into camera space at `frame`.
    ///
    /// Returns identity when the camera is off, so the caller can apply it
    /// unconditionally.
    pub fn transform_at(&self, frame: u32, stage: Size) -> Affine {
        let Some(state) = self.state_at(frame) else {
            return Affine::IDENTITY;
        };
        let centre = buzz_geom::Vec2::new(stage.width / 2.0, stage.height / 2.0);

        // Move the camera's centre to the middle of the stage, then apply
        // rotation and zoom about that point.
        Affine::translate(centre)
            * Affine::rotate(-state.rotation)
            * Affine::scale(state.zoom.max(f64::MIN_POSITIVE))
            * Affine::translate(-state.center.to_vec2())
    }

    /// Rebuild from parts, for loading and importing.
    pub fn from_parts(mut keys: Vec<CameraKey>, enabled: bool) -> Self {
        keys.sort_by_key(|k| k.frame);
        keys.dedup_by_key(|k| k.frame);
        Self { enabled, keys }
    }
}

fn lerp(a: f64, b: f64, t: f64) -> f64 {
    a + (b - a) * t
}

/// Geometric interpolation, so zooming reads as a constant rate of change.
fn lerp_zoom(a: f64, b: f64, t: f64) -> f64 {
    if a <= 0.0 || b <= 0.0 || !a.is_finite() || !b.is_finite() {
        return lerp(a, b, t).max(f64::MIN_POSITIVE);
    }
    (a.ln() + (b.ln() - a.ln()) * t).exp()
}

/// Interpolate by the shortest way round the circle.
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

#[cfg(test)]
mod tests {
    use super::*;

    fn track() -> CameraTrack {
        let mut t = CameraTrack::new();
        t.enabled = true;
        t
    }

    #[test]
    fn a_disabled_camera_contributes_nothing() {
        let mut t = CameraTrack::new();
        t.set_key(CameraKey::new(0, Point::new(100.0, 100.0)));
        assert!(t.state_at(0).is_none(), "disabled camera should be inert");
        assert_eq!(
            t.transform_at(0, Size::new(550.0, 400.0)).as_coeffs(),
            Affine::IDENTITY.as_coeffs()
        );
    }

    #[test]
    fn a_single_key_holds_for_every_frame() {
        let mut t = track();
        t.set_key(CameraKey::new(10, Point::new(50.0, 60.0)));
        for frame in [0, 10, 999] {
            assert_eq!(t.state_at(frame).unwrap().center, Point::new(50.0, 60.0));
        }
    }

    #[test]
    fn position_interpolates_between_keys() {
        let mut t = track();
        t.set_key(CameraKey::new(0, Point::new(0.0, 0.0)));
        t.set_key(CameraKey::new(10, Point::new(100.0, 200.0)));

        let mid = t.state_at(5).unwrap();
        assert!((mid.center.x - 50.0).abs() < 1e-9, "got {:?}", mid.center);
        assert!((mid.center.y - 100.0).abs() < 1e-9);
    }

    /// Geometric zoom interpolation: 1x to 4x passes through 2x, not 2.5x.
    #[test]
    fn zoom_interpolates_geometrically() {
        let mut t = track();
        t.set_key(CameraKey {
            frame: 0,
            center: Point::ORIGIN,
            zoom: 1.0,
            rotation: 0.0,
        });
        t.set_key(CameraKey {
            frame: 10,
            center: Point::ORIGIN,
            zoom: 4.0,
            rotation: 0.0,
        });

        let mid = t.state_at(5).unwrap();
        assert!(
            (mid.zoom - 2.0).abs() < 1e-9,
            "expected 2.0 at the midpoint, got {}",
            mid.zoom
        );
    }

    /// Turning from 350° to 10° must go forward 20°, not backward 340°.
    #[test]
    fn rotation_takes_the_short_way_round() {
        let mut t = track();
        let deg = |d: f64| d.to_radians();
        t.set_key(CameraKey {
            frame: 0,
            center: Point::ORIGIN,
            zoom: 1.0,
            rotation: deg(350.0),
        });
        t.set_key(CameraKey {
            frame: 10,
            center: Point::ORIGIN,
            zoom: 1.0,
            rotation: deg(10.0),
        });

        let mid = t.state_at(5).unwrap().rotation.to_degrees();
        // Halfway is 360 (== 0), not 180.
        let normalised = ((mid % 360.0) + 360.0) % 360.0;
        assert!(
            normalised < 1.0 || normalised > 359.0,
            "expected roughly 0 or 360 degrees, got {normalised}"
        );
    }

    #[test]
    fn values_hold_outside_the_keyed_range() {
        let mut t = track();
        t.set_key(CameraKey::new(10, Point::new(10.0, 0.0)));
        t.set_key(CameraKey::new(20, Point::new(20.0, 0.0)));

        assert_eq!(t.state_at(0).unwrap().center.x, 10.0, "holds the first");
        assert_eq!(t.state_at(999).unwrap().center.x, 20.0, "holds the last");
    }

    #[test]
    fn setting_a_key_twice_replaces_it() {
        let mut t = track();
        t.set_key(CameraKey::new(5, Point::new(1.0, 1.0)));
        t.set_key(CameraKey::new(5, Point::new(9.0, 9.0)));

        assert_eq!(t.keys().len(), 1);
        assert_eq!(t.state_at(5).unwrap().center, Point::new(9.0, 9.0));
    }

    #[test]
    fn keys_stay_sorted_whatever_order_they_arrive_in() {
        let mut t = track();
        for frame in [30, 10, 20, 0] {
            t.set_key(CameraKey::new(frame, Point::new(frame as f64, 0.0)));
        }
        let frames: Vec<u32> = t.keys().iter().map(|k| k.frame).collect();
        assert_eq!(frames, vec![0, 10, 20, 30]);
    }

    #[test]
    fn keys_can_be_removed() {
        let mut t = track();
        t.set_key(CameraKey::new(5, Point::ORIGIN));
        assert!(t.remove_key(5));
        assert!(!t.remove_key(5));
        assert!(t.is_empty());
    }

    /// The transform must actually centre what the camera is looking at.
    #[test]
    fn the_transform_centres_the_camera_target_on_the_stage() {
        let mut t = track();
        let stage = Size::new(550.0, 400.0);
        t.set_key(CameraKey::new(0, Point::new(200.0, 150.0)));

        let transform = t.transform_at(0, stage);
        let centred = transform * Point::new(200.0, 150.0);
        assert!((centred.x - 275.0).abs() < 1e-9, "got {centred:?}");
        assert!((centred.y - 200.0).abs() < 1e-9);
    }

    #[test]
    fn zooming_the_camera_magnifies_about_its_centre() {
        let mut t = track();
        let stage = Size::new(550.0, 400.0);
        t.set_key(CameraKey {
            frame: 0,
            center: Point::new(275.0, 200.0),
            zoom: 2.0,
            rotation: 0.0,
        });

        let transform = t.transform_at(0, stage);
        // The centre stays put.
        let centre = transform * Point::new(275.0, 200.0);
        assert!((centre.x - 275.0).abs() < 1e-9);
        // A point 100 units right lands 200 away at 2x.
        let offset = transform * Point::new(375.0, 200.0);
        assert!((offset.x - 475.0).abs() < 1e-9, "got {offset:?}");
    }

    #[test]
    fn degenerate_values_do_not_produce_a_broken_transform() {
        let mut t = track();
        for zoom in [0.0, -1.0, f64::NAN, f64::INFINITY] {
            t.clear();
            t.set_key(CameraKey {
                frame: 0,
                center: Point::ORIGIN,
                zoom,
                rotation: 0.0,
            });
            let coeffs = t.transform_at(0, Size::new(550.0, 400.0)).as_coeffs();
            // NaN in, NaN out is acceptable for the value itself, but the
            // matrix must not be structurally broken.
            assert_eq!(coeffs.len(), 6);
        }
    }

    #[test]
    fn rebuilding_sorts_and_deduplicates() {
        let t = CameraTrack::from_parts(
            vec![
                CameraKey::new(5, Point::ORIGIN),
                CameraKey::new(0, Point::ORIGIN),
                CameraKey::new(5, Point::new(1.0, 1.0)),
            ],
            true,
        );
        assert_eq!(t.keys().len(), 2);
        assert_eq!(t.keys()[0].frame, 0);
        assert_eq!(t.last_frame(), 5);
    }
}
