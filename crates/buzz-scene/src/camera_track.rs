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

use crate::time::AtTime;
use buzz_geom::{Affine, Point, Projection, Size};
use serde::{Deserialize, Serialize};

/// The camera's state at one keyframe.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct CameraKey {
    pub frame: u32,
    /// Point the camera is centred on, in document space.
    pub center: Point,
    /// Magnification. 1.0 shows the stage at its natural size.
    pub zoom: f64,
    /// Rotation in radians. Roll, in camera terms: the horizon tipping.
    pub rotation: f64,

    /// Tilt up and down, in radians — the camera nodding.
    ///
    /// This is what makes the camera **spatial** rather than a pan-and-zoom
    /// over a flat picture: with pitch, a layer's far edge really is further
    /// away than its near edge, so a rectangle is drawn as a trapezoid. Zero
    /// is looking straight at the stage, which is where every document that
    /// does not use this stays.
    pub pitch: f64,
    /// Turn left and right, in radians.
    pub yaw: f64,
}

/// How far the camera may tilt.
///
/// Past this a layer plane is nearly edge-on: it occupies a sliver of the
/// frame, the flattening tolerance collapses, and the picture is no longer
/// something anybody meant. Animate has no equivalent because Animate has no
/// tilt; the bound is here because the arithmetic needs one.
pub const MAX_TILT: f64 = 1.3;

impl CameraKey {
    pub fn new(frame: u32, center: Point) -> Self {
        Self {
            frame,
            center,
            zoom: 1.0,
            rotation: 0.0,
            pitch: 0.0,
            yaw: 0.0,
        }
    }

    /// Is the camera looking straight at the stage?
    ///
    /// The render path leans on this: an untilted camera is an affine, and
    /// takes exactly the route it always did.
    pub fn is_flat(&self) -> bool {
        self.pitch == 0.0 && self.yaw == 0.0
    }

    /// The same key with its tilt brought into range.
    pub fn clamped(mut self) -> Self {
        self.pitch = self.pitch.clamp(-MAX_TILT, MAX_TILT);
        self.yaw = self.yaw.clamp(-MAX_TILT, MAX_TILT);
        self
    }
}

/// How far the camera sits from the plane where depth is zero.
///
/// Layers nearer than this look bigger and slide further as the camera pans;
/// layers beyond it look smaller and slide less. The number is in document
/// units, so at the default a layer at depth 1000 is twice as far from the
/// camera as the stage and therefore renders at half size.
pub const DEFAULT_FOCAL_DISTANCE: f64 = 1000.0;

/// Instants across an open shutter, when a document asks for motion blur but
/// does not say how many.
///
/// Eight is enough for the speeds hand-drawn animation actually moves at, and
/// is eight times the cost of a clean frame — which is the honest price of the
/// effect, and the reason it is not on by default.
pub const DEFAULT_BLUR_SAMPLES: u32 = 8;

/// Most instants worth sampling across one shutter.
///
/// Past this the picture stops changing and the export merely takes longer;
/// bounded here so a hand-edited file cannot ask for a thousand.
pub const MAX_BLUR_SAMPLES: u32 = 64;

/// Nearest a layer may come to the camera before it is treated as behind it.
///
/// At the camera plane the perspective divide blows up; a hair in front of it
/// a layer would be magnified by thousands and swamp the frame. Layers this
/// close or closer are simply not drawn, which is what Animate does and is far
/// better than a frame filled by one runaway layer.
const NEAR_PLANE: f64 = 1.0;

/// **How near the lens a layer may be put**, as a fraction of the focal
/// distance.
///
/// Not [`NEAR_PLANE`], which is where the projection actually gives up. A layer
/// a hair in front of that is magnified by hundreds and swamps the frame, so
/// every control that moves a layer stops well short — and so does
/// [`Scene::set_focal_distance`](crate::Scene::set_focal_distance), which has
/// to keep the same promise when it is the *lens* that moves rather than the
/// layer.
///
/// One number, in one place, because the two controls used to disagree: the
/// timeline's depth column bounded a drag at 0.95 of the focal distance and the
/// Layer Depth panel at 0.9, so the same layer had two different "as near as it
/// goes" depending on which one you reached for.
const NEAR_LIMIT: f64 = 0.9;

/// A named camera position — a "shot" of the staged scene.
///
/// An angle is a *camera state*, not a new scene: the stage is furniture,
/// characters and lights, and the camera already carries everything a viewpoint
/// needs — centre, zoom, roll, pitch, yaw. Naming one lets an animator stage a
/// set once and shoot it from several angles, jumping between them or cutting
/// between them on the timeline. See [`CameraTrack::angles`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NamedAngle {
    pub name: String,
    pub state: CameraKey,
}

/// **The lens at one keyframe** — where it is focused, and how wide it is open.
///
/// A focus *pull* is the shot's most quietly expressive move: the background
/// softens away and the eye is carried to the face in front of it, without a
/// cut and without anything on the stage moving. That needs focus to be a thing
/// that changes over time, so it is keyed, like every other camera value.
///
/// Aperture rides along in the same key rather than in one of its own. The two
/// are pulled together in practice — opening up is how a pull is made visible
/// at all — and a second track of keys to keep in step would buy nothing but
/// the chance for them to disagree.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FocusKey {
    pub frame: u32,
    /// The layer depth that is sharp at this frame.
    pub focus_depth: f64,
    /// Depth-of-field strength at this frame. Zero is a pinhole: all sharp.
    pub aperture: f64,
}

/// The camera's keyframes over the whole timeline.
#[derive(Debug, Clone, PartialEq)]
pub struct CameraTrack {
    /// Off by default. Animate hides the camera until you enable it.
    pub enabled: bool,
    /// Distance from the camera to the depth-zero plane.
    ///
    /// This is what turns a layer's depth into a size: the smaller it is, the
    /// more violent the perspective, exactly as a shorter lens exaggerates it.
    pub focal_distance: f64,
    /// Depth-of-field strength, in blur document-units per unit of depth away
    /// from focus. Zero — the default — is a pinhole camera: everything sharp.
    ///
    /// This is the *geometric* half of depth of field: layers off the focal
    /// plane get a blur proportional to how far out of focus they are, reusing
    /// the per-shape blur the filter path already draws. The per-pixel sliced
    /// version is a later upgrade; this one ships by reusing what exists.
    pub aperture: f64,
    /// The layer depth that is in focus. Zero is the focal plane.
    pub focus_depth: f64,
    /// **How long the shutter is open**, as a fraction of a frame. Zero — the
    /// default — is an infinitely fast shutter: every frame is a clean instant,
    /// which is how this program has always drawn.
    ///
    /// A film camera's shutter is open for part of each frame and records
    /// everything that happens while it is, which is why fast motion on film is
    /// a smear rather than a crisp displaced copy. Half a frame is the
    /// hundred-and-eighty-degree shutter almost all cinema is shot at, and is
    /// what "normal" motion blur looks like.
    pub shutter: f64,
    /// How many instants the open shutter is sampled at.
    ///
    /// The smear is built by drawing the frame this many times across the
    /// shutter and adding the results up, so this is a direct multiplier on the
    /// cost of an export — and, past a point, invisible: too few samples read
    /// as a row of ghosts rather than a smear, and the number that fixes that
    /// depends on how far the artwork travels, not on the resolution.
    pub blur_samples: u32,
    /// Named viewpoints of the staged scene — Wave 10b. Empty for every
    /// document that never saves one.
    pub angles: Vec<NamedAngle>,
    /// Sorted by frame.
    keys: Vec<CameraKey>,
    /// The focus pull, sorted by frame.
    ///
    /// Empty in every document that never pulls focus, which is the state
    /// `aperture` and `focus_depth` above describe on their own. That is why
    /// this is a separate list rather than two more fields on [`CameraKey`]:
    /// depth of field has never needed the camera track to be *enabled*, and
    /// folding focus into the camera's own keys would have made a focus pull
    /// require a camera move.
    focus_keys: Vec<FocusKey>,
}

impl Default for CameraTrack {
    fn default() -> Self {
        Self {
            enabled: false,
            focal_distance: DEFAULT_FOCAL_DISTANCE,
            aperture: 0.0,
            focus_depth: 0.0,
            shutter: 0.0,
            blur_samples: DEFAULT_BLUR_SAMPLES,
            angles: Vec::new(),
            keys: Vec::new(),
            focus_keys: Vec::new(),
        }
    }
}

impl CameraTrack {
    pub fn new() -> Self {
        Self::default()
    }

    /// How much smaller a layer at `depth` appears.
    ///
    /// Straight pinhole projection: a layer at distance `f + depth` from the
    /// camera subtends `f / (f + depth)` of what the same layer would at the
    /// focal plane. Depth 0 gives exactly 1.0, so a document that never
    /// touches depth is unaffected — which is why this is safe to apply
    /// unconditionally rather than behind a flag.
    ///
    /// `None` means the layer is at or behind the camera and should not be
    /// drawn.
    pub fn depth_scale(&self, depth: f64) -> Option<f64> {
        if depth == 0.0 {
            // Exactly 1.0, with no arithmetic to round it away.
            return Some(1.0);
        }
        let focal = self.focal_distance.max(NEAR_PLANE);
        let distance = focal + depth;
        (distance >= NEAR_PLANE).then(|| focal / distance)
    }

    /// **As near the camera as a layer may be put**, in depth.
    ///
    /// Negative, because depth counts away from the lens. This is the one bound
    /// that has to hold: past it [`depth_scale`](Self::depth_scale) answers
    /// `None` and the layer is not drawn at all. There is deliberately no
    /// matching far bound — a layer behind the stage only ever gets smaller,
    /// and no distance takes it out of the picture — so this is a floor and not
    /// a range. Anything a *control* wants to bound the far side at is that
    /// control's business.
    ///
    /// **Two bounds, whichever is the nearer to the stage.** [`NEAR_LIMIT`] is
    /// the comfortable one and holds at every ordinary focal distance. It is
    /// not sufficient on its own: it leaves a *fraction* of the focal distance
    /// in front of the layer, and below ten units that fraction is smaller than
    /// [`NEAR_PLANE`] — so a short lens would hand back a bound that
    /// `depth_scale` refuses to draw, which is the one thing this must never
    /// do. `NEAR_PLANE - focal` is that case stated exactly.
    pub fn nearest_depth(&self) -> f64 {
        // The same clamp `depth_scale` applies, so the two agree about where
        // the lens is even for a focal distance smaller than the near plane.
        let focal = self.focal_distance.max(NEAR_PLANE);
        (-(focal * NEAR_LIMIT)).max(NEAR_PLANE - focal)
    }

    /// The transform for artwork on a layer at `depth`.
    ///
    /// The same construction as [`Self::transform_at`] with the zoom scaled by
    /// the perspective factor. That single change gives both effects at once,
    /// and gives them consistently: a far layer is drawn smaller *and*, because
    /// its offset from the camera centre is scaled by the same factor, slides
    /// less when the camera pans. Parallax is not a separate rule bolted on —
    /// it falls out of the projection.
    ///
    /// `None` when the layer is at or behind the camera.
    pub fn transform_at_depth(&self, at: impl AtTime, stage: Size, depth: f64) -> Option<Affine> {
        let scale = self.depth_scale(depth)?;

        let centre = buzz_geom::Vec2::new(stage.width / 2.0, stage.height / 2.0);
        let state = self.state_at(at);

        // With no camera keys the camera still has a position: the middle of
        // the stage, unzoomed. Depth has to work without one, or setting a
        // layer's depth would do nothing until a camera keyframe existed.
        let (camera_centre, zoom, rotation) = match state {
            Some(s) => (s.center, s.zoom, s.rotation),
            None => (centre.to_point(), 1.0, 0.0),
        };

        Some(
            Affine::translate(centre)
                * Affine::rotate(-rotation)
                * Affine::scale((zoom * scale).max(f64::MIN_POSITIVE))
                * Affine::translate(-camera_centre.to_vec2()),
        )
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
    pub fn state_at(&self, at: impl AtTime) -> Option<CameraKey> {
        if !self.enabled || self.keys.is_empty() {
            return None;
        }
        if self.keys.len() == 1 {
            return Some(self.keys[0]);
        }

        // Continuous, so a shutter open between two frames sees the camera part
        // of the way between them rather than jumping on the frame boundary.
        let time = at.as_time();
        let first = self.keys[0];
        if time <= first.frame as f64 {
            return Some(first);
        }
        let last = self.keys[self.keys.len() - 1];
        if time >= last.frame as f64 {
            return Some(last);
        }

        let after = self.keys.partition_point(|k| (k.frame as f64) <= time);
        let a = self.keys[after - 1];
        let b = self.keys[after];

        let span = (b.frame - a.frame) as f64;
        let t = if span > 0.0 {
            (time - a.frame as f64) / span
        } else {
            0.0
        };

        Some(CameraKey {
            frame: at.frame(),
            center: Point::new(
                lerp(a.center.x, b.center.x, t),
                lerp(a.center.y, b.center.y, t),
            ),
            // Zoom interpolates geometrically: going 1x to 4x should pass
            // through 2x at the midpoint, not 2.5x, or the move visibly
            // accelerates.
            zoom: lerp_zoom(a.zoom, b.zoom, t),
            rotation: lerp_angle(a.rotation, b.rotation, t),
            // Tilt interpolates like rotation — the short way round — so a
            // camera swinging from one side of straight-ahead to the other
            // does not take the long route through the back of the scene.
            pitch: lerp_angle(a.pitch, b.pitch, t),
            yaw: lerp_angle(a.yaw, b.yaw, t),
        })
    }

    /// Transform mapping document space into camera space at `frame`.
    ///
    /// Returns identity when the camera is off, so the caller can apply it
    /// unconditionally.
    pub fn transform_at(&self, at: impl AtTime, stage: Size) -> Affine {
        let Some(state) = self.state_at(at) else {
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

    /// How a layer at `depth` is projected onto the frame.
    ///
    /// This is [`Self::transform_at_depth`] generalised: with no tilt it is
    /// exactly that affine, and the render path takes the same route it always
    /// did. With pitch or yaw it is a homography, and the layer's plane is
    /// drawn in perspective — a rectangle becomes a trapezoid.
    ///
    /// `None` when the layer is at or behind the camera, or turned so far past
    /// edge-on that it would be drawn inside out.
    pub fn projection_at_depth(
        &self,
        at: impl AtTime,
        stage: Size,
        depth: f64,
    ) -> Option<Projection> {
        let state = self.state_at(at).map(CameraKey::clamped);

        // Flat is the common case and must stay bit-exact, so it does not go
        // near the homography at all.
        if state.is_none_or(|s| s.is_flat()) {
            return self
                .transform_at_depth(at, stage, depth)
                .map(Projection::from_affine);
        }
        let state = state?;

        let focal = self.focal_distance.max(NEAR_PLANE);
        let distance = focal + depth;
        if distance < NEAR_PLANE {
            return None;
        }

        // The lens looks at the layer's plane...
        let lens = Projection::look_at_plane(distance, focal, state.pitch, state.yaw)?;

        // ...about the point the camera is centred on, and the result is
        // placed on the stage with the zoom and roll the shot asks for. The
        // order matters: the camera's centre has to be subtracted *before* the
        // projection, because that is the point the lens is pointed at.
        let centre = buzz_geom::Vec2::new(stage.width / 2.0, stage.height / 2.0);
        Some(
            lens.pre_affine(Affine::translate(-state.center.to_vec2()))
                .then_affine(
                    Affine::translate(centre)
                        * Affine::rotate(-state.rotation)
                        * Affine::scale(state.zoom.max(f64::MIN_POSITIVE)),
                ),
        )
    }

    /// How an object that faces its own way is projected onto the frame.
    ///
    /// `pivot` is the point the object turns about, in document space, and
    /// `depth` is its layer's. The object's plane passes through the pivot,
    /// tipped by its own angles and pushed by its own `z`.
    ///
    /// With a flat object this is exactly [`Self::projection_at_depth`] — the
    /// test says so — which is what keeps every document that does not use 3D
    /// rendering through the transform it always did.
    ///
    /// `None` when the object is at or behind the camera, or exactly edge-on.
    pub fn projection_for_object(
        &self,
        at: impl AtTime,
        stage: Size,
        depth: f64,
        pivot: Point,
        spatial: &crate::object::Spatial,
    ) -> Option<Projection> {
        if spatial.is_flat() {
            return self.projection_at_depth(at, stage, depth);
        }

        let state = self.state_at(at).map(CameraKey::clamped);
        let (camera_centre, zoom, rotation, pitch, yaw) = match state {
            Some(s) => (s.center, s.zoom, s.rotation, s.pitch, s.yaw),
            None => (
                Point::new(stage.width / 2.0, stage.height / 2.0),
                1.0,
                0.0,
                0.0,
                0.0,
            ),
        };

        let focal = self.focal_distance.max(NEAR_PLANE);
        let distance = focal + depth;
        if distance < NEAR_PLANE {
            return None;
        }

        // Where the camera is, relative to the plane the layer lies in: behind
        // its target by `distance`, along its own view axis.
        let (sy, cy) = yaw.sin_cos();
        let (sp, cp) = pitch.sin_cos();
        let forward = [sy * cp, -sp, cy * cp];

        // The object's origin, seen from the camera: the pivot's offset within
        // the layer, its own push in depth, and the camera's stand-off.
        let offset = pivot - camera_centre.to_vec2();
        let to_origin = [
            offset.x + distance * forward[0],
            offset.y + distance * forward[1],
            spatial.z + distance * forward[2],
        ];

        let (ex, ey) = spatial.basis();
        let lens = Projection::plane_in_view(ex, ey, to_origin, focal, pitch, yaw)?;

        // The object's own coordinates are measured **from its pivot** — the
        // point its plane passes through — and the result is placed on the
        // stage with the shot's zoom and roll.
        //
        // From the pivot's *absolute* position, not from its offset to the
        // camera: `to_origin` already says where the pivot is relative to the
        // lens, so subtracting the offset here as well would leave the artwork
        // measured from the stage's origin and draw it distorted and tiny. It
        // did, and the GPU test that pushes an object towards the camera and
        // expects it to grow is what caught it.
        let centre = buzz_geom::Vec2::new(stage.width / 2.0, stage.height / 2.0);
        Some(
            lens.pre_affine(Affine::translate(-pivot.to_vec2()))
                .then_affine(
                    Affine::translate(centre)
                        * Affine::rotate(-rotation)
                        * Affine::scale(zoom.max(f64::MIN_POSITIVE)),
                ),
        )
    }

    /// Is any part of the shot tilted, at any frame?
    ///
    /// Lets the renderer and the editor skip the whole perspective path for
    /// the documents — nearly all of them — that never tilt.
    pub fn has_tilt(&self) -> bool {
        self.enabled && self.keys.iter().any(|k| !k.is_flat())
    }

    /// Rebuild from parts, for loading and importing.
    pub fn from_parts(mut keys: Vec<CameraKey>, enabled: bool, focal_distance: f64) -> Self {
        keys.sort_by_key(|k| k.frame);
        keys.dedup_by_key(|k| k.frame);
        Self {
            enabled,
            // A file written before depth existed, or a corrupt one, must not
            // produce a camera sitting on the artwork.
            focal_distance: if focal_distance.is_finite() && focal_distance > 0.0 {
                focal_distance
            } else {
                DEFAULT_FOCAL_DISTANCE
            },
            aperture: 0.0,
            focus_depth: 0.0,
            shutter: 0.0,
            blur_samples: DEFAULT_BLUR_SAMPLES,
            angles: Vec::new(),
            keys,
            focus_keys: Vec::new(),
        }
    }

    /// **The lens at `frame`** — the focus pull, resolved.
    ///
    /// With no focus keys this is simply the static [`focus_depth`](Self::focus_depth)
    /// and [`aperture`](Self::aperture), so a document that never pulls focus
    /// takes exactly the path it always did.
    ///
    /// Unlike [`state_at`](Self::state_at) this does **not** require the camera
    /// to be enabled. Depth of field has always applied with the camera track
    /// switched off — it is a property of the lens, not of a camera *move* — and
    /// making a pull the exception would mean switching the camera on, and so
    /// keying a shot, just to soften a background.
    ///
    /// Before the first key it holds the first value and after the last it holds
    /// the last, which is what a held focus is; between them it interpolates
    /// linearly, like the camera's own keys.
    pub fn focus_at(&self, at: impl AtTime) -> FocusKey {
        let time = at.as_time();
        let frame = at.frame();
        if self.focus_keys.is_empty() {
            return FocusKey {
                frame,
                focus_depth: self.focus_depth,
                aperture: self.aperture,
            };
        }

        let first = self.focus_keys[0];
        if time <= first.frame as f64 {
            return FocusKey { frame, ..first };
        }
        let last = self.focus_keys[self.focus_keys.len() - 1];
        if time >= last.frame as f64 {
            return FocusKey { frame, ..last };
        }

        let after = self.focus_keys.partition_point(|k| (k.frame as f64) <= time);
        let a = self.focus_keys[after - 1];
        let b = self.focus_keys[after];
        let span = (b.frame - a.frame) as f64;
        let t = if span > 0.0 {
            (time - a.frame as f64) / span
        } else {
            0.0
        };

        FocusKey {
            frame,
            focus_depth: lerp(a.focus_depth, b.focus_depth, t),
            aperture: lerp(a.aperture, b.aperture, t),
        }
    }

    /// The depth-of-field blur, in document units, for a layer at `depth` on
    /// `frame`.
    ///
    /// `None` when the lens is a pinhole (`aperture == 0`) or the layer is in
    /// focus, so the sharp common case sets no blur at all. The blur grows with
    /// distance from the focus depth, which is the geometric approximation to a
    /// lens's circle of confusion.
    ///
    /// The frame is what makes a **focus pull** possible: with focus keyed, the
    /// same layer at the same depth is sharp on one frame and soft on another.
    pub fn dof_blur_at(&self, at: impl AtTime, depth: f64) -> Option<f64> {
        let lens = self.focus_at(at);
        if lens.aperture <= 0.0 {
            return None;
        }
        let coc = lens.aperture * (depth - lens.focus_depth).abs();
        (coc > 0.05).then_some(coc)
    }

    /// **The instants one frame's shutter is open for**, as offsets in frames.
    ///
    /// `None` when there is no blur to draw — no shutter, or only one sample —
    /// and that is the answer for every document that does not ask for it, so
    /// the export takes exactly the path it always did.
    ///
    /// The shutter is **centred on the frame**: the offsets run from
    /// `-shutter/2` to `+shutter/2`, sampled at the middle of each equal slice.
    /// Centring is what keeps the smear *around* where the artwork is rather
    /// than trailing behind it — open the shutter at the frame instead and
    /// every moving thing sits half a shutter late, which reads as the
    /// animation itself having slipped.
    pub fn shutter_offsets(&self) -> Option<Vec<f64>> {
        if !(self.shutter > 0.0) || !self.shutter.is_finite() {
            return None;
        }
        let samples = self.blur_samples.clamp(1, MAX_BLUR_SAMPLES);
        if samples < 2 {
            return None;
        }
        let n = samples as f64;
        Some(
            (0..samples)
                .map(|i| self.shutter * ((i as f64 + 0.5) / n - 0.5))
                .collect(),
        )
    }

    /// The focus keys, sorted by frame. Empty unless the shot pulls focus.
    pub fn focus_keys(&self) -> &[FocusKey] {
        &self.focus_keys
    }

    /// Is the focus keyed exactly on this frame?
    pub fn has_focus_key_at(&self, frame: u32) -> bool {
        self.focus_keys.iter().any(|k| k.frame == frame)
    }

    /// Highest focus-keyed frame, so a shot whose only animation is a focus
    /// pull is still as long as the pull.
    pub fn focus_last_frame(&self) -> u32 {
        self.focus_keys.last().map(|k| k.frame).unwrap_or(0)
    }

    /// Add or replace the focus key at `key.frame`.
    pub fn set_focus_key(&mut self, key: FocusKey) {
        match self.focus_keys.iter().position(|k| k.frame == key.frame) {
            Some(index) => self.focus_keys[index] = key,
            None => {
                let at = self.focus_keys.partition_point(|k| k.frame < key.frame);
                self.focus_keys.insert(at, key);
            }
        }
    }

    pub fn remove_focus_key(&mut self, frame: u32) -> bool {
        let before = self.focus_keys.len();
        self.focus_keys.retain(|k| k.frame != frame);
        self.focus_keys.len() != before
    }

    /// Drop the pull, leaving whatever the static aperture and focus say.
    pub fn clear_focus_keys(&mut self) {
        self.focus_keys.clear();
    }

    /// Replace the whole pull, as the loader does.
    ///
    /// Sorted and de-duplicated on the way in, and a negative aperture is
    /// clamped away: a hand-edited or corrupt file must not be able to produce
    /// keys the lookup would read out of order.
    pub fn set_focus_keys(&mut self, mut keys: Vec<FocusKey>) {
        keys.sort_by_key(|k| k.frame);
        keys.dedup_by_key(|k| k.frame);
        for key in &mut keys {
            key.aperture = key.aperture.max(0.0);
        }
        self.focus_keys = keys;
    }

    /// Save the camera's state at `frame` under `name`, replacing any angle of
    /// the same name.
    pub fn save_angle(&mut self, name: impl Into<String>, frame: u32) {
        let name = name.into();
        let state = self
            .state_at(frame)
            .unwrap_or_else(|| CameraKey::new(frame, Point::ORIGIN));
        match self.angles.iter_mut().find(|a| a.name == name) {
            Some(angle) => angle.state = state,
            None => self.angles.push(NamedAngle { name, state }),
        }
    }

    /// The saved angle of this name, if any.
    pub fn angle(&self, name: &str) -> Option<&NamedAngle> {
        self.angles.iter().find(|a| a.name == name)
    }

    pub fn remove_angle(&mut self, name: &str) -> bool {
        let before = self.angles.len();
        self.angles.retain(|a| a.name != name);
        self.angles.len() != before
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
    fn a_camera_with_no_shutter_has_no_motion_blur() {
        let track = CameraTrack::new();
        assert_eq!(track.shutter, 0.0, "off by default");
        assert!(
            track.shutter_offsets().is_none(),
            "no shutter, no instants to add up"
        );
    }

    #[test]
    fn one_sample_is_not_a_smear() {
        let mut track = CameraTrack::new();
        track.shutter = 0.5;
        track.blur_samples = 1;
        assert!(
            track.shutter_offsets().is_none(),
            "a single instant is the clean frame, and should take the clean path"
        );
    }

    /// The offsets straddle the frame evenly, so the smear sits around the
    /// artwork rather than trailing it.
    #[test]
    fn the_shutter_is_centred_on_the_frame() {
        let mut track = CameraTrack::new();
        track.shutter = 0.5;
        track.blur_samples = 8;
        let offsets = track.shutter_offsets().expect("a shutter");

        assert_eq!(offsets.len(), 8);
        let mean: f64 = offsets.iter().sum::<f64>() / offsets.len() as f64;
        assert!(mean.abs() < 1e-9, "the instants average to the frame: {mean}");

        let first = offsets[0];
        let last = offsets[offsets.len() - 1];
        assert!(first < 0.0 && last > 0.0, "they straddle it: {first}..{last}");
        assert!(
            first > -0.25 && last < 0.25,
            "and stay inside the open shutter: {first}..{last}"
        );

        // Evenly spaced, so no instant is weighted more than another.
        let step = offsets[1] - offsets[0];
        for pair in offsets.windows(2) {
            assert!((pair[1] - pair[0] - step).abs() < 1e-9, "even spacing");
        }
    }

    #[test]
    fn a_wider_shutter_reaches_further_either_way() {
        let mut narrow = CameraTrack::new();
        narrow.shutter = 0.25;
        let mut wide = CameraTrack::new();
        wide.shutter = 1.0;

        let n = narrow.shutter_offsets().expect("narrow");
        let w = wide.shutter_offsets().expect("wide");
        assert!(
            w[0] < n[0] && *w.last().unwrap() > *n.last().unwrap(),
            "a longer exposure sees more of the move"
        );
    }

    #[test]
    fn a_nonsense_shutter_asks_for_nothing() {
        for bad in [-1.0, f64::NAN, f64::INFINITY] {
            let mut track = CameraTrack::new();
            track.shutter = bad;
            assert!(track.shutter_offsets().is_none(), "for {bad}");
        }
    }

    #[test]
    fn the_sample_count_is_bounded() {
        let mut track = CameraTrack::new();
        track.shutter = 0.5;
        track.blur_samples = 100_000;
        let offsets = track.shutter_offsets().expect("a shutter");
        assert_eq!(
            offsets.len(),
            MAX_BLUR_SAMPLES as usize,
            "a hand-edited file cannot ask for a thousand instants"
        );
    }

    #[test]
    fn a_pinhole_camera_has_no_depth_of_field() {
        let track = CameraTrack::new();
        assert_eq!(track.aperture, 0.0);
        assert_eq!(track.dof_blur_at(0, 500.0), None, "no aperture, no blur");
    }

    #[test]
    fn depth_of_field_grows_with_distance_from_focus() {
        let mut track = CameraTrack::new();
        track.aperture = 0.02;
        track.focus_depth = 0.0;
        assert_eq!(track.dof_blur_at(0, 0.0), None, "the focus plane is sharp");
        let near = track.dof_blur_at(0, 200.0).expect("out of focus");
        let far = track.dof_blur_at(0, 800.0).expect("further out of focus");
        assert!(far > near, "further from focus should blur more: {near} vs {far}");
    }

    /// The whole point of the static fields staying: a document that never
    /// keys focus behaves exactly as it did before there was a pull at all,
    /// on every frame.
    #[test]
    fn without_focus_keys_the_lens_never_changes() {
        let mut track = CameraTrack::new();
        track.aperture = 0.02;
        track.focus_depth = 300.0;
        for frame in [0, 1, 50, 10_000] {
            let lens = track.focus_at(frame);
            assert_eq!(lens.aperture, 0.02, "frame {frame}");
            assert_eq!(lens.focus_depth, 300.0, "frame {frame}");
        }
    }

    /// A focus pull: the lens starts focused on the background and travels to
    /// the foreground, so the *same layer at the same depth* goes from sharp to
    /// soft without anything on the stage moving.
    #[test]
    fn a_focus_pull_moves_the_sharp_plane_over_time() {
        let mut track = CameraTrack::new();
        track.set_focus_key(FocusKey {
            frame: 0,
            focus_depth: 600.0,
            aperture: 0.05,
        });
        track.set_focus_key(FocusKey {
            frame: 24,
            focus_depth: 0.0,
            aperture: 0.05,
        });

        // The layer sitting at depth 600: in focus at the start, out of it by
        // the end.
        assert_eq!(track.dof_blur_at(0, 600.0), None, "sharp where the focus is");
        let pulled = track
            .dof_blur_at(24, 600.0)
            .expect("the focus has left this layer behind");
        assert!(pulled > 1.0, "a full pull should be a real blur, got {pulled}");

        // And the foreground it travelled to does the reverse.
        assert!(track.dof_blur_at(0, 0.0).is_some(), "foreground starts soft");
        assert_eq!(track.dof_blur_at(24, 0.0), None, "and ends sharp");
    }

    #[test]
    fn focus_holds_before_the_first_key_and_after_the_last() {
        let mut track = CameraTrack::new();
        track.set_focus_key(FocusKey {
            frame: 10,
            focus_depth: 100.0,
            aperture: 0.01,
        });
        track.set_focus_key(FocusKey {
            frame: 20,
            focus_depth: 200.0,
            aperture: 0.02,
        });

        assert_eq!(track.focus_at(0).focus_depth, 100.0, "held before the first");
        assert_eq!(track.focus_at(99).focus_depth, 200.0, "held after the last");
        let middle = track.focus_at(15);
        assert!(
            (middle.focus_depth - 150.0).abs() < 1e-9,
            "halfway is halfway: {}",
            middle.focus_depth
        );
        assert!(
            (middle.aperture - 0.015).abs() < 1e-9,
            "the aperture rides along: {}",
            middle.aperture
        );
    }

    /// Focus keys arrive from a file, where nothing guarantees their order.
    #[test]
    fn loaded_focus_keys_are_sorted_and_sane() {
        let mut track = CameraTrack::new();
        track.set_focus_keys(vec![
            FocusKey { frame: 20, focus_depth: 200.0, aperture: 0.02 },
            FocusKey { frame: 0, focus_depth: 0.0, aperture: -1.0 },
            FocusKey { frame: 20, focus_depth: 999.0, aperture: 0.5 },
        ]);
        let frames: Vec<u32> = track.focus_keys().iter().map(|k| k.frame).collect();
        assert_eq!(frames, vec![0, 20], "sorted, one key per frame");
        assert_eq!(
            track.focus_keys()[0].aperture,
            0.0,
            "a negative aperture is clamped away"
        );
    }

    #[test]
    fn a_focus_key_can_be_replaced_and_removed() {
        let mut track = CameraTrack::new();
        let key = FocusKey { frame: 5, focus_depth: 1.0, aperture: 0.1 };
        track.set_focus_key(key);
        track.set_focus_key(FocusKey { focus_depth: 2.0, ..key });
        assert_eq!(track.focus_keys().len(), 1, "re-keying replaces");
        assert_eq!(track.focus_keys()[0].focus_depth, 2.0);
        assert!(track.has_focus_key_at(5));
        assert!(track.remove_focus_key(5));
        assert!(!track.remove_focus_key(5), "removing twice is not a change");
        assert!(track.focus_keys().is_empty());
    }

    #[test]
    fn a_saved_angle_captures_the_camera_and_comes_back() {
        let mut track = track();
        track.set_key(CameraKey {
            frame: 0,
            center: Point::new(120.0, 80.0),
            zoom: 2.5,
            rotation: 0.3,
            pitch: 0.0,
            yaw: 0.0,
        });
        track.save_angle("Wide", 0);
        let angle = track.angle("Wide").expect("saved");
        assert_eq!(angle.state.center, Point::new(120.0, 80.0));
        assert_eq!(angle.state.zoom, 2.5);

        // Saving the same name again replaces it rather than duplicating.
        track.save_angle("Wide", 0);
        assert_eq!(track.angles.len(), 1);
        assert!(track.remove_angle("Wide"));
        assert!(track.angle("Wide").is_none());
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
            pitch: 0.0,
            yaw: 0.0,
        });
        t.set_key(CameraKey {
            frame: 10,
            center: Point::ORIGIN,
            zoom: 4.0,
            rotation: 0.0,
            pitch: 0.0,
            yaw: 0.0,
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
            pitch: 0.0,
            yaw: 0.0,
        });
        t.set_key(CameraKey {
            frame: 10,
            center: Point::ORIGIN,
            zoom: 1.0,
            rotation: deg(10.0),
            pitch: 0.0,
            yaw: 0.0,
        });

        let mid = t.state_at(5).unwrap().rotation.to_degrees();
        // Halfway is 360 (== 0), not 180.
        let normalised = ((mid % 360.0) + 360.0) % 360.0;
        assert!(
            !(1.0..=359.0).contains(&normalised),
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
            pitch: 0.0,
            yaw: 0.0,
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
                pitch: 0.0,
                yaw: 0.0,
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
            DEFAULT_FOCAL_DISTANCE,
        );
        assert_eq!(t.keys().len(), 2);
        assert_eq!(t.keys()[0].frame, 0);
        assert_eq!(t.last_frame(), 5);
    }

    // -- layer depth --------------------------------------------------------

    const STAGE: Size = Size::new(400.0, 300.0);

    /// A document that never touches depth must render exactly as it did
    /// before depth existed, so the focal plane has to be exactly 1.0 — not
    /// 0.9999999 from a divide.
    #[test]
    fn the_focal_plane_is_untouched_by_perspective() {
        let track = CameraTrack::new();
        assert_eq!(track.depth_scale(0.0), Some(1.0));

        let plain = track.transform_at(0, STAGE);
        let at_depth = track.transform_at_depth(0, STAGE, 0.0).unwrap();
        // With no camera keys the plain transform is the identity, and depth
        // zero must agree with it on where artwork lands.
        let probe = Point::new(37.0, 91.0);
        assert!((plain * probe - at_depth * probe).hypot() < 1e-9);
    }

    /// The headline behaviour: further away is smaller, nearer is larger.
    #[test]
    fn distance_shrinks_a_layer_and_nearness_enlarges_it() {
        let track = CameraTrack::new(); // focal distance 1000

        // Twice as far from the camera, so half the size.
        assert_eq!(track.depth_scale(1000.0), Some(0.5));
        // Half as far, so twice the size.
        assert_eq!(track.depth_scale(-500.0), Some(2.0));

        let far = track.depth_scale(400.0).unwrap();
        let near = track.depth_scale(-400.0).unwrap();
        assert!(far < 1.0 && near > 1.0, "far {far}, near {near}");
    }

    /// A shorter focal distance exaggerates perspective, exactly as a wider
    /// lens does. This is the knob that makes depth subtle or dramatic.
    #[test]
    fn a_shorter_focal_distance_exaggerates_perspective() {
        let mut gentle = CameraTrack::new();
        gentle.focal_distance = 4000.0;
        let mut dramatic = CameraTrack::new();
        dramatic.focal_distance = 250.0;

        let a = gentle.depth_scale(500.0).unwrap();
        let b = dramatic.depth_scale(500.0).unwrap();
        assert!(
            b < a,
            "the shorter lens should shrink it more: gentle {a}, dramatic {b}"
        );
    }

    /// A layer at or behind the camera is not drawn. Without this the
    /// perspective divide explodes and one layer swamps the frame.
    #[test]
    fn a_layer_at_or_behind_the_camera_is_not_drawn() {
        let track = CameraTrack::new();
        assert_eq!(track.depth_scale(-1000.0), None, "exactly at the camera");
        assert_eq!(track.depth_scale(-5000.0), None, "behind it");
        assert!(track.transform_at_depth(0, STAGE, -1000.0).is_none());

        // Just in front of it still works, and is very large.
        let just_in_front = track.depth_scale(-999.0).expect("still visible");
        assert!(just_in_front > 100.0, "got {just_in_front}");
    }

    /// Parallax: as the camera pans, a distant layer slides less than a near
    /// one. This is the whole reason layer depth exists, and it falls out of
    /// the projection rather than being a separate rule.
    #[test]
    fn panning_the_camera_moves_a_distant_layer_less_than_a_near_one() {
        let mut track = CameraTrack::new();
        track.enabled = true;
        track.set_key(CameraKey::new(0, Point::new(200.0, 150.0)));
        // The camera pans 100 units to the right by frame 10.
        track.set_key(CameraKey::new(10, Point::new(300.0, 150.0)));

        // The same document point, on layers at three depths.
        let probe = Point::new(200.0, 150.0);
        let shift = |depth: f64| -> f64 {
            let before = track.transform_at_depth(0, STAGE, depth).unwrap() * probe;
            let after = track.transform_at_depth(10, STAGE, depth).unwrap() * probe;
            (before - after).hypot()
        };

        let near = shift(-500.0);
        let focal = shift(0.0);
        let far = shift(2000.0);

        assert!(
            far < focal && focal < near,
            "a pan should move near layers most and far layers least: \
             near {near:.1}, focal {focal:.1}, far {far:.1}"
        );
        // The focal plane moves exactly with the camera.
        assert!((focal - 100.0).abs() < 1e-9, "focal moved {focal}");
    }

    /// Depth has to work before any camera keyframe exists, or setting a
    /// layer's depth would appear to do nothing at all.
    #[test]
    fn depth_applies_without_a_camera_keyframe() {
        let track = CameraTrack::new();
        assert!(track.state_at(0).is_none(), "no keys, and not enabled");

        // A point on the stage edge, on a layer pushed into the distance.
        let transform = track.transform_at_depth(0, STAGE, 1000.0).unwrap();
        let corner = transform * Point::new(0.0, 0.0);
        let centre = Point::new(200.0, 150.0);

        // Half size about the stage centre.
        assert!((corner.x - 100.0).abs() < 1e-9, "got {corner:?}");
        assert!((corner.y - 75.0).abs() < 1e-9, "got {corner:?}");
        // And the centre itself does not move.
        assert!((transform * centre - centre).hypot() < 1e-9);
    }

    /// The camera's own zoom multiplies the perspective scale rather than
    /// replacing it.
    #[test]
    fn camera_zoom_and_depth_compose() {
        let mut track = CameraTrack::new();
        track.enabled = true;
        let mut key = CameraKey::new(0, Point::new(200.0, 150.0));
        key.zoom = 3.0;
        track.set_key(key);

        // Depth 1000 halves; zoom 3 triples; together 1.5x.
        let transform = track.transform_at_depth(0, STAGE, 1000.0).unwrap();
        let probe = Point::new(300.0, 150.0); // 100 right of the camera centre
        let moved = transform * probe;
        assert!(
            (moved.x - (200.0 + 150.0)).abs() < 1e-9,
            "expected 100 x 1.5 = 150 from the centre, got {moved:?}"
        );
    }

    /// **The bound the controls stop at must be a depth that still draws.**
    ///
    /// `nearest_depth` and `depth_scale` are two expressions of the same fact —
    /// where the lens is — written apart, and a layer dragged exactly to the
    /// bound and then not drawn would be the worst of both. Ties them together
    /// so neither constant can be moved without the other.
    #[test]
    fn a_layer_at_the_nearest_depth_is_still_drawn() {
        for focal in [1.0, 5.0, 50.0, 200.0, DEFAULT_FOCAL_DISTANCE, 6000.0] {
            let mut track = CameraTrack::default();
            track.focal_distance = focal;
            let nearest = track.nearest_depth();
            // Never *behind* the stage. It reaches zero only for a lens sitting
            // on the near plane, where there is honestly no room in front of it
            // — no control offers that, but a file or a script can.
            assert!(nearest <= 0.0, "the bound is never behind the stage: {focal}");
            assert!(
                track.depth_scale(nearest).is_some(),
                "a layer at the bound must still draw, at focal {focal}"
            );
            // And it is a real bound, not a formality: past it, nothing.
            assert!(
                track.depth_scale(-focal * 2.0).is_none(),
                "well past the lens must not draw, at focal {focal}"
            );
        }
    }

    #[test]
    fn a_corrupt_focal_distance_falls_back_to_the_default() {
        for bad in [0.0, -100.0, f64::NAN, f64::INFINITY] {
            let track = CameraTrack::from_parts(Vec::new(), false, bad);
            assert_eq!(track.focal_distance, DEFAULT_FOCAL_DISTANCE, "for {bad}");
        }
        let good = CameraTrack::from_parts(Vec::new(), false, 750.0);
        assert_eq!(good.focal_distance, 750.0);
    }

    // -- a spatial camera ----------------------------------------------------

    fn stage() -> Size {
        Size::new(550.0, 400.0)
    }

    fn tilted(pitch: f64, yaw: f64) -> CameraTrack {
        let mut track = CameraTrack::new();
        track.enabled = true;
        track.set_key(CameraKey {
            frame: 0,
            center: Point::new(275.0, 200.0),
            zoom: 1.0,
            rotation: 0.0,
            pitch,
            yaw,
        });
        track
    }

    /// The promise every document depends on: a camera that does not tilt
    /// produces exactly the affine it always did.
    #[test]
    fn an_untilted_camera_gives_exactly_the_old_transform() {
        let track = tilted(0.0, 0.0);
        for depth in [0.0, 500.0, -300.0, 2000.0] {
            let affine = track.transform_at_depth(0, stage(), depth);
            let projection = track.projection_at_depth(0, stage(), depth);

            match (affine, projection) {
                (Some(a), Some(p)) => {
                    assert!(p.is_affine(), "depth {depth} stopped being affine");
                    assert_eq!(
                        p.as_affine().unwrap().as_coeffs(),
                        a.as_coeffs(),
                        "depth {depth} changed"
                    );
                }
                (None, None) => {}
                other => panic!("depth {depth} disagreed: {other:?}"),
            }
        }
    }

    /// And so does a camera that is switched off, or has no keys at all.
    #[test]
    fn a_camera_with_no_keys_still_projects() {
        let track = CameraTrack::new();
        let projection = track
            .projection_at_depth(0, stage(), 0.0)
            .expect("in front");
        assert!(projection.is_affine());
    }

    /// The point of the whole exercise: pitch turns the stage into a trapezoid.
    #[test]
    fn pitch_makes_the_stage_a_trapezoid() {
        let track = tilted(0.4, 0.0);
        let projection = track
            .projection_at_depth(0, stage(), 0.0)
            .expect("in front");
        assert!(!projection.is_affine());

        let corners = projection
            .map_rect(buzz_geom::Rect::new(0.0, 0.0, 550.0, 400.0))
            .expect("all four corners in front");
        let top = (corners[1] - corners[0]).hypot();
        let bottom = (corners[2] - corners[3]).hypot();
        assert!(
            (top - bottom).abs() > 5.0,
            "the stage should be a trapezoid: {top} vs {bottom}"
        );
    }

    /// Yaw does the same thing about the other axis — the left and right edges
    /// differ instead of the top and bottom.
    #[test]
    fn yaw_makes_the_stage_a_trapezoid_the_other_way() {
        let projection = tilted(0.0, 0.4)
            .projection_at_depth(0, stage(), 0.0)
            .expect("in front");

        let corners = projection
            .map_rect(buzz_geom::Rect::new(0.0, 0.0, 550.0, 400.0))
            .expect("in front");
        let left = (corners[3] - corners[0]).hypot();
        let right = (corners[2] - corners[1]).hypot();
        assert!(
            (left - right).abs() > 5.0,
            "the sides should differ: {left} vs {right}"
        );
    }

    /// A tilted camera still respects depth: a layer further away is still
    /// drawn smaller, so tilt and parallax compose rather than replacing each
    /// other.
    #[test]
    fn tilt_and_depth_work_together() {
        let track = tilted(0.35, 0.0);
        let area = |depth: f64| {
            let projection = track
                .projection_at_depth(0, stage(), depth)
                .expect("in front");
            let bounds = projection
                .map_rect_bounds(buzz_geom::Rect::new(0.0, 0.0, 550.0, 400.0))
                .expect("in front");
            bounds.width() * bounds.height()
        };
        assert!(
            area(1000.0) < area(0.0),
            "a further layer should still be smaller"
        );
    }

    /// Tilt is bounded. A camera turned past edge-on has nothing sensible to
    /// draw, and a corrupt file must not produce one.
    #[test]
    fn tilt_is_clamped_into_range() {
        let key = CameraKey {
            frame: 0,
            center: Point::ZERO,
            zoom: 1.0,
            rotation: 0.0,
            pitch: 99.0,
            yaw: -99.0,
        }
        .clamped();
        assert!(key.pitch <= MAX_TILT && key.yaw >= -MAX_TILT);

        // And the projection is built from the clamped value, so it exists.
        let mut track = CameraTrack::new();
        track.enabled = true;
        track.set_key(CameraKey {
            pitch: 99.0,
            ..CameraKey::new(0, Point::new(275.0, 200.0))
        });
        assert!(track.projection_at_depth(0, stage(), 0.0).is_some());
    }

    /// Tilt interpolates between keys, the short way round.
    #[test]
    fn tilt_interpolates_between_keys() {
        let mut track = CameraTrack::new();
        track.enabled = true;
        track.set_key(CameraKey {
            pitch: 0.0,
            ..CameraKey::new(0, Point::new(275.0, 200.0))
        });
        track.set_key(CameraKey {
            pitch: 0.8,
            ..CameraKey::new(10, Point::new(275.0, 200.0))
        });

        let middle = track.state_at(5).expect("keyed");
        assert!(
            (middle.pitch - 0.4).abs() < 1e-9,
            "half way should be half the tilt: {}",
            middle.pitch
        );
    }

    /// `has_tilt` is what lets the renderer skip the perspective path for the
    /// documents that never use it.
    #[test]
    fn a_flat_shot_reports_no_tilt() {
        assert!(!tilted(0.0, 0.0).has_tilt());
        assert!(tilted(0.3, 0.0).has_tilt());
        assert!(tilted(0.0, -0.3).has_tilt());

        let mut off = tilted(0.5, 0.5);
        off.enabled = false;
        assert!(!off.has_tilt(), "a camera switched off tilts nothing");
    }
}
