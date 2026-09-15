//! Motion paths: turning "travel along this curve" into keyframes.
//!
//! # What a motion path is, and what it deliberately is not
//!
//! A motion path is a **function from time to a place on a curve**. Given a
//! drawn path and a fraction `0.0..1.0`, [`buzz_geom::edit::frame_at_fraction`]
//! answers where on the path that is, and which way the path is heading there.
//! That is the whole of it. Writing those places onto a timeline is a separate
//! job ([`follow`]) and knows nothing about curves.
//!
//! This mirrors [`crate::perform`] exactly, and for the same reason: the sampler
//! is testable without a document, and the honest description of the feature is
//! that it is arithmetic an animator would otherwise do by hand — key the object
//! at the start of the curve, at the end, and a dozen places between, reading
//! the position off the path each time.
//!
//! # It writes keyframes, and then it is gone
//!
//! There is no live "follows this path" property on the object. [`follow`]
//! leaves ordinary keyframes holding ordinary transforms, which the animator
//! then edits, retimes, or throws away one at a time. The straight-line tween
//! system cannot walk a curve — it lerps between two transforms — so the curve
//! is *baked* into keys here rather than referenced.
//!
//! # Why keys land on twos
//!
//! [`MotionPathOptions::step`] defaults to two frames with a tween between,
//! matching [`crate::perform`]: a key on every frame is a timeline nobody can
//! adjust, and the tween fills the gap so the motion is smooth on ones anyway.
//! The final key is always pinned to the last frame of the range, so the object
//! reaches the end of the curve exactly when the range ends however the step
//! divides it.

use buzz_geom::{Affine, BezPath};
use buzz_scene::{Easing, ObjectId, Scene, Tween};

/// Arc-length sampling tolerance, in document units. Small enough to be
/// invisible at any sane zoom, large enough that sampling a long path per
/// keyframe stays cheap. `frame_at_fraction` re-scales this to the path if it is
/// ever handed something degenerate.
const ACCURACY: f64 = 0.05;

/// How to lay an object along a drawn path.
#[derive(Debug, Clone)]
pub struct MotionPathOptions {
    /// The frames to fill, as a half-open range.
    pub frames: std::ops::Range<u32>,
    /// The timing curve, applied to *when* the object is where along the path —
    /// an ease-in makes it start slow and pick up. Baked into the key positions,
    /// not into any tween.
    pub easing: Easing,
    /// Rotate the object to face along the path as it travels. Off keeps the
    /// object's own orientation and only moves it.
    pub orient_to_path: bool,
    /// Frames between keys. Two is on twos; one is on ones.
    pub step: u32,
}

impl MotionPathOptions {
    /// Sensible defaults over `frames`: linear timing, facing along the path, on
    /// twos — the "just make it follow the curve" case.
    pub fn new(frames: std::ops::Range<u32>) -> Self {
        Self {
            frames,
            easing: Easing::Linear,
            orient_to_path: true,
            step: 2,
        }
    }
}

/// What [`follow`] did, for the status line and the tests.
#[derive(Debug, Clone, PartialEq)]
pub struct MotionReport {
    pub keyframes: u32,
    pub frames: u32,
    /// The path's arc length, in document units.
    pub length: f64,
    pub message: String,
}

/// Why a motion path could not be written.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MotionError {
    /// The object to move is not on the timeline (deleted, or never existed).
    NoObject,
    /// The frame range was empty, so there was nothing to fill.
    NoFrames,
    /// The drawn path had no length to travel along.
    EmptyPath,
}

impl std::fmt::Display for MotionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MotionError::NoObject => write!(f, "select an object to send along the path"),
            MotionError::NoFrames => write!(f, "there are no frames in that range to fill"),
            MotionError::EmptyPath => write!(f, "draw a longer path \u{2014} that one has no length"),
        }
    }
}

impl std::error::Error for MotionError {}

/// **Write a motion path onto the timeline.**
///
/// Every keyframe holds the object placed at one point along `path`, as an
/// ordinary transform on an ordinary keyframe: nothing about the result
/// remembers that it was generated, so all of it can be edited afterwards.
///
/// The caller wraps this in one [`buzz_scene::Scene`]-mutating `Document::edit`
/// so that the whole traverse is one Ctrl+Z.
pub fn follow(
    scene: &mut Scene,
    object: ObjectId,
    path: &BezPath,
    opts: &MotionPathOptions,
) -> Result<MotionReport, MotionError> {
    if opts.frames.is_empty() {
        return Err(MotionError::NoFrames);
    }
    let length = buzz_geom::edit::path_length(path, ACCURACY);
    if length <= 0.0 || path.segments().next().is_none() {
        return Err(MotionError::EmptyPath);
    }

    // Where it was placed. The object's own rotation and scale are kept; only
    // its position is driven by the path (plus its facing, if orienting), so a
    // character the animator sized and posed keeps that through the move.
    let Some((layer, found)) = scene.find_object(object) else {
        return Err(MotionError::NoObject);
    };
    let base = found.transform;

    // The frames to key: on the step, plus the last frame of the range always
    // pinned so the object lands on the end of the curve exactly when the range
    // ends. `span` measures time from the first to that last frame, so easing is
    // applied against real time rather than the key index.
    let start = opts.frames.start;
    let last = opts.frames.end - 1;
    let step = opts.step.max(1);
    let mut frames: Vec<u32> = (start..opts.frames.end).step_by(step as usize).collect();
    if frames.last() != Some(&last) {
        frames.push(last);
    }
    let span = (last - start).max(1) as f64;

    // The layer has to be long enough to hold the range before any of it can be
    // keyed: `ensure_keyframe` refuses past the end of the span, which would
    // otherwise silently write only the part that fits.
    scene.update_layer(layer, |l| {
        if l.frames.length() <= last {
            l.frames.insert_frame(last);
        }
    });

    let count = frames.len();
    for (i, &frame) in frames.iter().enumerate() {
        let progress = (frame - start) as f64 / span;
        let t = opts.easing.apply(progress);
        let Some((pos, tangent)) = buzz_geom::edit::frame_at_fraction(path, t, ACCURACY) else {
            continue;
        };

        scene.ensure_keyframe(layer, frame);
        scene.update_object_at(frame, object, |target| {
            target.transform = place(base, pos, tangent, opts.orient_to_path);
        });

        // A tween across the gap, so keying on twos is smooth on ones. The last
        // key gets none: there is nothing after it to tween to. The tween is a
        // plain straight lerp between adjacent path samples — the easing is
        // already baked into where those samples sit, so it must not be applied
        // a second time here.
        if i + 1 < count {
            scene.update_layer(layer, |l| {
                l.frames.set_tween(frame, Tween::motion());
            });
        }
    }

    let frame_count = opts.frames.len() as u32;
    let keyframes = count as u32;
    Ok(MotionReport {
        keyframes,
        frames: frame_count,
        length,
        message: format!(
            "Motion Path: {keyframes} keyframe(s) over {frame_count} frame(s), {length:.0} units"
        ),
    })
}

/// Place the object with its origin at `pos`.
///
/// Position always comes from the path. Rotation and scale come from where the
/// object already stood, *unless* `orient` is set, in which case the object is
/// turned to face along the path's tangent while keeping its size. Composed
/// `translate * rotate * scale`, matching [`crate::perform`] and
/// `buzz_geom::brush::stamp_transforms`.
fn place(base: Affine, pos: buzz_geom::Point, tangent: buzz_geom::Vec2, orient: bool) -> Affine {
    if orient {
        let (sx, sy) = scale_of(base);
        Affine::translate(pos.to_vec2())
            * Affine::rotate(tangent.y.atan2(tangent.x))
            * Affine::scale_non_uniform(sx, sy)
    } else {
        // Keep the object's whole linear part (rotation, scale and any shear);
        // just move its origin onto the path.
        let c = base.as_coeffs();
        let linear = Affine::new([c[0], c[1], c[2], c[3], 0.0, 0.0]);
        Affine::translate(pos.to_vec2()) * linear
    }
}

/// Signed x/y scale of an affine, so a mirrored object stays mirrored. The same
/// decomposition [`buzz_scene`]'s tween uses, kept here so orienting an object
/// along a path does not have to reach into another crate's internals.
fn scale_of(a: Affine) -> (f64, f64) {
    let c = a.as_coeffs();
    let scale_x = (c[0] * c[0] + c[1] * c[1]).sqrt();
    let determinant = c[0] * c[3] - c[1] * c[2];
    let scale_y_magnitude = (c[2] * c[2] + c[3] * c[3]).sqrt();
    let scale_y = if determinant < 0.0 {
        -scale_y_magnitude
    } else {
        scale_y_magnitude
    };
    (scale_x, scale_y)
}
