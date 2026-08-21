//! Lit drawing: the crescent cache, and what keeps it warm while a light moves.
//!
//! # What is cached, and what deliberately is not
//!
//! A light generates three shapes per piece of artwork. Two of them — the
//! shading crescent away from the light and the highlight towards it — are
//! **boolean differences** between the artwork and a copy of itself, and
//! booleans are the most expensive thing this renderer does. The third, the
//! cast shadow, is the artwork under one affine
//! ([`buzz_light::shadow_transform`]).
//!
//! Only the booleans are cached. The shadow is rebuilt every frame because
//! rebuilding it costs a matrix multiply, and because *not* caching it is what
//! lets it follow a light live: dragging a sun now moves every shadow in the
//! document at the frame rate, while the crescents catch up behind.
//!
//! # The key, and why it is a direction rather than a light
//!
//! A crescent is the artwork minus itself shifted, so it depends on exactly
//! three things: the path, the direction the light lies in, and the softness.
//! Nothing else about a light touches it — not colour, not strength, not
//! height, not whether it casts. So the key is
//!
//! * **which artwork**, as copy-on-write pointer identity. An object that has
//!   not been edited is still the same `Arc` in every snapshot, however much
//!   else in the document changed, so drawing a brush stroke does not rebuild
//!   the shading of everything else. The entry holds the `Arc` it keyed on,
//!   which is what makes the address safe to compare: nothing can be freed and
//!   its address reused while the entry lives.
//! * **where it stands**, as the placement affine, quantised.
//! * **which way the light lies**, quantised to a fraction of a degree, and how
//!   soft it is.
//!
//! Keying on the direction rather than on the light is what makes a light rig
//! affordable. A sun climbing the sky does not turn a single terminator, so not
//! one crescent is rebuilt. Two lights that happen to lie the same way share
//! their geometry. And a light being *aimed* rounds to the same step for several
//! pointer positions in a row, so a slow drag mostly hits.
//!
//! # Nothing ever draws flat
//!
//! Every placement remembers the last crescents built for it, whatever
//! direction they were built for. A miss that cannot be built on this thread
//! hands those back rather than nothing, so artwork stays lit — very slightly
//! stale — through a drag, instead of flicking between shaded and flat. That
//! staleness is a fraction of a second and is invisible next to the shadow,
//! which is exact on every frame.
//!
//! Placements not drawn for a few frames are dropped, so scrubbing a long
//! timeline does not accumulate the geometry of every frame it passed through.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use buzz_geom::{Affine, BezPath, Vec2};
use buzz_light::ShadeGeometry;
use buzz_scene::Object;

/// One piece of artwork, where it stands. Everything a crescent is built from
/// except the light.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct Place {
    /// The object's address. Stable while it is unedited, thanks to
    /// copy-on-write, and kept valid by holding the `Arc` in the slot.
    object: usize,
    /// Which shape within that object, for groups and warps.
    shape: u16,
    /// The placement, quantised. A group moving changes this without changing
    /// its children's addresses, and a rotation changes the crescent as surely
    /// as a translation does — which the old key, a quantised centre point,
    /// could not see.
    doc: [i64; 6],
}

/// A light direction and softness, rounded to the step the cache works in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct Aim {
    /// The direction towards the light, in steps of [`DIRECTION_STEPS`] round
    /// the circle.
    direction: i32,
    /// Softness in steps of [`SOFTNESS_STEPS`].
    softness: i32,
}

/// How finely a light's direction is resolved: 512 steps round the circle is
/// about 0.7°, which is far below what a terminator on flat artwork shows and
/// coarse enough that aiming a light reuses geometry for several pointer
/// positions at a time.
const DIRECTION_STEPS: f64 = 512.0;

/// How finely softness is resolved. It is a `0..=1` fraction of the shape.
const SOFTNESS_STEPS: f64 = 256.0;

/// How many directions one placement keeps geometry for before starting over.
///
/// Small: this is a *drag* buffer, not a history. Twelve covers the few steps a
/// hand wobbles over while aiming and nothing more, so a long drag across the
/// whole dial cannot pile up a hundred crescents per shape.
const MAX_AIMS: usize = 12;

impl Aim {
    /// Round a direction and softness to the cache's steps.
    fn of(towards: Vec2, softness: f64) -> Self {
        let step = std::f64::consts::TAU / DIRECTION_STEPS;
        Self {
            direction: (towards.atan2() / step).round() as i32,
            softness: (softness.clamp(0.0, 1.0) * SOFTNESS_STEPS).round() as i32,
        }
    }

    /// The direction this rounds to, as a unit vector. Geometry is built from
    /// *this* rather than from what the caller passed, so the entry a hit
    /// returns is exactly the one a fresh build would have produced.
    fn towards(self) -> Vec2 {
        Vec2::from_angle(self.direction as f64 * (std::f64::consts::TAU / DIRECTION_STEPS))
    }

    fn softness(self) -> f64 {
        self.softness as f64 / SOFTNESS_STEPS
    }
}

/// Quantise a placement. The linear part finely, because a small rotation is
/// visible in a crescent; the translation to a tenth of a unit, because a shape
/// nudged by a hundredth of a pixel does not need rebuilding and exact float
/// keys would never hit.
fn quantise(doc: Affine) -> [i64; 6] {
    let c = doc.as_coeffs();
    [
        (c[0] * 4096.0).round() as i64,
        (c[1] * 4096.0).round() as i64,
        (c[2] * 4096.0).round() as i64,
        (c[3] * 4096.0).round() as i64,
        (c[4] * 10.0).round() as i64,
        (c[5] * 10.0).round() as i64,
    ]
}

/// Everything cached for one placement.
struct Slot {
    /// Held to keep the address unique for as long as the slot lives.
    #[allow(dead_code, reason = "held for pointer identity, never read")]
    owner: Arc<Object>,
    /// Crescents by the direction they were built for.
    built: HashMap<Aim, Arc<ShadeGeometry>>,
    /// The most recent crescents for this placement, whatever direction built
    /// them. Handed back on a miss that cannot be built here, so the artwork
    /// stays lit while the real geometry is on its way. See the module note.
    last: Arc<ShadeGeometry>,
    used: u64,
}

/// One piece of geometry that was asked for and is not built yet.
///
/// It carries everything [`crescents`](buzz_light::crescents) needs, owned
/// outright, so it can cross to a worker thread and be built there — which is
/// the whole point: a heavy scene's crescents are built on every core at once,
/// off the thread drawing the window, instead of one at a time in front of the
/// user. See [`LightCache::take_misses`].
pub struct Miss {
    key: (Place, Aim),
    path: BezPath,
    owner: Arc<Object>,
}

impl Miss {
    /// Build the geometry. Pure and self-contained, so a whole batch of these
    /// runs in parallel with no shared state.
    pub fn build(self) -> Built {
        let aim = self.key.1;
        Built {
            key: self.key,
            geometry: Arc::new(buzz_light::crescents(
                &self.path,
                aim.towards(),
                aim.softness(),
            )),
            owner: self.owner,
        }
    }
}

/// A [`Miss`] that has been built, ready to be put back into the cache.
pub struct Built {
    key: (Place, Aim),
    geometry: Arc<ShadeGeometry>,
    owner: Arc<Object>,
}

/// Generated crescent geometry, kept between frames.
pub struct LightCache {
    places: HashMap<Place, Slot>,
    frame: u64,
    /// How long this frame may spend building misses on this thread. The app
    /// sets it — see [`LightCache::set_inline_budget`].
    inline_budget: std::time::Duration,
    /// How much of that has gone, this frame.
    inline_spent: std::time::Duration,
    /// Whether a miss that cannot be built here is recorded for later. Off
    /// while a build is already in flight, because anything recorded then is
    /// thrown away, and recording it costs a copy of the path.
    queue: bool,
    /// Geometry asked for this frame that was not cached. Drained by the app,
    /// built off-thread, and returned through [`LightCache::install`].
    misses: Vec<Miss>,
    /// Keys already queued this frame, so the same shape drawn twice does not
    /// queue twice.
    queued: HashSet<(Place, Aim)>,
    /// Whether anything this frame was drawn with geometry that is not the
    /// geometry it asked for. See [`LightCache::is_stale`].
    stale: bool,
    /// The "nothing" handed back when there is not even stale geometry to show.
    empty: Arc<ShadeGeometry>,
}

impl Default for LightCache {
    /// **Builds everything, on this thread, unless told otherwise.**
    ///
    /// That is the right default because most callers have no frame to protect
    /// and no later frame to install into: the exporter renders each film frame
    /// once and writes it out, and the thumbnail renderer the same. A cache
    /// that deferred by default would hand those an unlit picture and then
    /// throw the real geometry away. Only the window sets a budget, because
    /// only the window has somewhere to put the work instead.
    fn default() -> Self {
        Self {
            places: HashMap::new(),
            frame: 0,
            inline_budget: std::time::Duration::MAX,
            inline_spent: std::time::Duration::ZERO,
            queue: true,
            misses: Vec::new(),
            queued: HashSet::new(),
            stale: false,
            empty: Arc::default(),
        }
    }
}

/// How many frames a placement survives without being drawn.
///
/// Two is enough to cover onion skinning, which draws neighbouring frames and
/// would otherwise evict everything the live frame just built.
const KEEP_FRAMES: u64 = 3;

/// The inline build budget an ordinary frame gets.
///
/// **Time, not a count of crescents**, because what a crescent costs is set by
/// the artwork: a boolean over a blob of a hundred curves runs five times a
/// boolean over a rounded rectangle, so any fixed count is either wasteful on
/// simple artwork or a stutter on real artwork. Measured as a count of sixteen
/// it was 11 ms on circles at a fine tolerance — most of a frame, for a number
/// chosen to be small.
///
/// Two milliseconds fits inside the 4 ms a frame section is allowed with room
/// for the encode around it. Under it, an edit lights on the spot with no
/// deferred frame and no flicker; over it, the rest goes off-thread. That is
/// what stops a bulk rebuild ever landing on the frame: there is no path
/// through this cache that builds an unbounded amount of geometry in front of
/// the user, whatever the document is made of.
pub const INLINE_BUDGET: std::time::Duration = std::time::Duration::from_millis(2);

impl LightCache {
    pub fn new() -> Self {
        Self::default()
    }

    /// Start a frame.
    ///
    /// This used to take the rig's fingerprint and throw the whole cache away
    /// when it moved. There is nothing to throw away now: an entry names the
    /// direction it was built for, so a light that changed simply stops being
    /// asked for and ages out, and every other light's geometry is undisturbed.
    pub fn begin(&mut self) {
        self.frame = self.frame.wrapping_add(1);
        self.discard_frame();
    }

    /// **Forget what this frame asked for**, because the frame is being thrown
    /// away and encoded again.
    ///
    /// A frame too big for the rasteriser is re-encoded with less of the light
    /// in it (see [`crate::document::DrawCache::reconsider`]), and what the
    /// discarded pass recorded is work for shapes the replacement will not
    /// shade at all. Left in place it would be handed to the workers, built,
    /// and never drawn — a bulk boolean rebuild for a picture nobody asked for.
    ///
    /// Not the built geometry: that is keyed on a direction and stays useful.
    /// Only what *this* pass owes.
    pub fn discard_frame(&mut self) {
        self.misses.clear();
        self.queued.clear();
        self.stale = false;
        self.inline_spent = std::time::Duration::ZERO;
    }

    /// **Did this frame draw anything with the wrong shading?**
    ///
    /// True when a shape asked for crescents the cache did not have and could
    /// not build here — so it drew with the ones it had last, or with none.
    ///
    /// The window has to ask, because it is the only thing that can put the
    /// picture right: it must keep asking for frames until this comes back
    /// false, or the last frame drawn is a stale one and stays on screen until
    /// something unrelated happens to provoke a repaint. That is not a
    /// hypothetical — a rule that only builds once a light has stopped moving
    /// left the frame *after* the light moved carrying the shading from before
    /// it, with nothing scheduled that would ever replace it. Switching a light
    /// on did nothing visible until the pointer next crossed the window.
    pub fn is_stale(&self) -> bool {
        self.stale
    }

    /// Drop anything that has not been drawn recently.
    pub fn end(&mut self) {
        let frame = self.frame;
        self.places
            .retain(|_, slot| frame.saturating_sub(slot.used) < KEEP_FRAMES);
    }

    /// How many placements are cached. Not how many crescents: a placement
    /// holds one per direction it has been lit from.
    pub fn len(&self) -> usize {
        self.places.len()
    }

    pub fn is_empty(&self) -> bool {
        self.places.is_empty()
    }

    /// How long this frame may spend building misses on this thread.
    ///
    /// Zero defers everything, which is what a cold cache and a light drag
    /// want. [`INLINE_BUDGET`] is the ordinary frame: a couple of milliseconds
    /// built on the spot so an edit lights with no flicker, and the rest
    /// deferred so a bulk rebuild can never be paid for in front of the user.
    pub fn set_inline_budget(&mut self, budget: std::time::Duration) {
        self.inline_budget = budget;
    }

    /// Whether misses are recorded for an off-thread build.
    ///
    /// Turned off while a build is already in flight: whatever were recorded
    /// then would be dropped on the floor, and recording one copies the whole
    /// transformed path. On a heavy document that was a thousand path copies a
    /// frame, thrown away, for every frame a build was running.
    pub fn set_queue(&mut self, queue: bool) {
        self.queue = queue;
    }

    /// Defer everything, or nothing. The blunt form of
    /// [`set_inline_budget`](Self::set_inline_budget), kept for callers that
    /// only want "not on this thread" — the exporter, which has no frame to
    /// protect, and the tests.
    pub fn set_defer(&mut self, defer: bool) {
        self.inline_budget = if defer {
            std::time::Duration::ZERO
        } else {
            std::time::Duration::MAX
        };
        self.queue = true;
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
            let (place, aim) = item.key;
            let frame = self.frame;
            let slot = self.places.entry(place).or_insert_with(|| Slot {
                owner: item.owner,
                built: HashMap::new(),
                last: Arc::clone(&item.geometry),
                used: frame,
            });
            slot.used = frame;
            if slot.built.len() >= MAX_AIMS {
                slot.built.clear();
            }
            slot.built.insert(aim, Arc::clone(&item.geometry));
            slot.last = item.geometry;
        }
    }

    /// The crescents for one shape, building them only if they are not here.
    ///
    /// `placed` is the path already in document space — the caller has it, from
    /// drawing the shape itself, so this never transforms a path that is going
    /// to hit. `doc` is the placement that produced it, and is what the entry is
    /// keyed on. `towards` points at the light, and only its direction is read.
    ///
    /// `owner` is the object's shared handle when it has one. Tweened artwork
    /// has none — it is rebuilt for the frame being drawn — so its geometry is
    /// generated and not kept, which is right: caching something that changes
    /// every frame only wastes the memory. Tweened artwork is also built inline
    /// however tight the budget, because there is nothing to defer *to*: it will
    /// not be here next frame either.
    pub fn crescents(
        &mut self,
        owner: Option<&Arc<Object>>,
        shape_index: u16,
        placed: &BezPath,
        doc: Affine,
        towards: Vec2,
        softness: f64,
    ) -> Arc<ShadeGeometry> {
        let aim = Aim::of(towards, softness);

        let Some(owner) = owner else {
            return Arc::new(buzz_light::crescents(placed, aim.towards(), aim.softness()));
        };

        let place = Place {
            object: Arc::as_ptr(owner) as usize,
            shape: shape_index,
            doc: quantise(doc),
        };

        // The common path: one hash lookup, no geometry touched at all.
        if let Some(slot) = self.places.get_mut(&place) {
            slot.used = self.frame;
            if let Some(geometry) = slot.built.get(&aim) {
                return Arc::clone(geometry);
            }
        }

        if self.inline_spent < self.inline_budget {
            let began = std::time::Instant::now();
            let geometry = Arc::new(buzz_light::crescents(
                placed,
                aim.towards(),
                aim.softness(),
            ));
            self.inline_spent = self.inline_spent.saturating_add(began.elapsed());
            self.install(vec![Built {
                key: (place, aim),
                geometry: Arc::clone(&geometry),
                owner: Arc::clone(owner),
            }]);
            return geometry;
        }

        // Not this frame, then. The picture is about to be a frame behind, and
        // the window needs to know that even when nothing is recorded — it is
        // what keeps it asking for frames until the shading is right.
        self.stale = true;

        // Queue it once — the path copy is why this is guarded — and hand back
        // the last crescents this artwork had, so it stays lit while the real
        // ones are built.
        if self.queue && self.queued.insert((place, aim)) {
            self.misses.push(Miss {
                key: (place, aim),
                path: placed.clone(),
                owner: Arc::clone(owner),
            });
        }
        match self.places.get(&place) {
            Some(slot) => Arc::clone(&slot.last),
            None => Arc::clone(&self.empty),
        }
    }
}

impl std::fmt::Debug for LightCache {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LightCache")
            .field("places", &self.places.len())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use buzz_geom::{Point, Rect, Shape as _};
    use buzz_light::{Light, LightId, LightKind};
    use buzz_scene::{ObjectId, ShapeData};
    use peniko::Color;

    fn object(id: u64) -> Arc<Object> {
        Arc::new(Object::shape(
            ObjectId(id),
            ShapeData::filled(Rect::new(0.0, 0.0, 100.0, 60.0).to_path(1e-9), Color::WHITE),
        ))
    }

    fn path_of(object: &Arc<Object>) -> BezPath {
        match &object.kind {
            buzz_scene::ObjectKind::Shape(shape) => shape.path.clone(),
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

    /// The direction a light lies in, from the middle of the test shape.
    fn towards(light: &Light) -> Vec2 {
        buzz_light::crescent_direction(light, Point::new(50.0, 30.0), 0.0, 1.0)
            .expect("the test lights all have a direction")
    }

    /// Ask the cache for the one test shape under `light`.
    fn ask(cache: &mut LightCache, object: &Arc<Object>, light: &Light) -> Arc<ShadeGeometry> {
        cache.crescents(
            Some(object),
            0,
            &path_of(object),
            Affine::IDENTITY,
            towards(light),
            light.softness,
        )
    }

    #[test]
    fn geometry_is_built_once_and_reused() {
        let mut cache = LightCache::new();
        let object = object(1);
        let light = sun();

        cache.begin();
        cache.set_defer(false);
        let first = ask(&mut cache, &object, &light);
        assert_eq!(cache.len(), 1);

        let second = ask(&mut cache, &object, &light);
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
        let light = sun();

        cache.begin();
        cache.set_defer(false);
        let before = ask(&mut cache, &untouched, &light);

        cache.begin();
        let after = ask(&mut cache, &untouched, &light);

        assert!(Arc::ptr_eq(&before, &after), "the geometry was rebuilt");
    }

    /// **Aiming a light rebuilds its crescents; anything else about it does
    /// not.** This is what makes a rig affordable, and it is the behaviour the
    /// old per-light fingerprint could not express: it hashed colour, strength,
    /// height and shadow settings into the key, so brightening a lamp threw away
    /// every boolean in the document.
    #[test]
    fn only_turning_a_light_rebuilds_its_crescents() {
        let mut cache = LightCache::new();
        let object = object(1);
        let light = sun();

        cache.begin();
        cache.set_defer(false);
        let before = ask(&mut cache, &object, &light);

        // Brighter, warmer, higher, no longer casting: the picture changes, the
        // terminator does not move.
        let mut same_aim = light.clone();
        same_aim.intensity = 3.0;
        same_aim.color = Color::from_rgb8(0x00, 0x40, 0xFF);
        same_aim.standing_height = 400.0;
        same_aim.shadows = false;
        if let LightKind::Sun { elevation, .. } = &mut same_aim.kind {
            *elevation = 1.4;
        }
        let after = ask(&mut cache, &object, &same_aim);
        assert!(
            Arc::ptr_eq(&before, &after),
            "nothing that leaves the terminator where it is may rebuild it"
        );

        // Swing it round, and it must rebuild.
        let mut turned = light.clone();
        if let LightKind::Sun { azimuth, .. } = &mut turned.kind {
            *azimuth += 1.0;
        }
        let turned = ask(&mut cache, &object, &turned);
        assert!(
            !Arc::ptr_eq(&before, &turned),
            "a light that moved must rebuild the crescents it drew"
        );
    }

    /// A nudge too small to see rounds to the same step, so aiming a light by
    /// hand mostly hits rather than rebuilding on every pointer move.
    #[test]
    fn a_nudge_below_the_step_reuses_the_geometry() {
        let mut cache = LightCache::new();
        let object = object(1);
        let step = std::f64::consts::TAU / DIRECTION_STEPS;
        let mut light = sun();
        // Squarely on a step, so a nudge below the step size cannot round to
        // the next one purely by where it started.
        if let LightKind::Sun { azimuth, .. } = &mut light.kind {
            *azimuth = 24.0 * step;
        }

        cache.begin();
        cache.set_defer(false);
        let before = ask(&mut cache, &object, &light);

        let mut nudged = light.clone();
        if let LightKind::Sun { azimuth, .. } = &mut nudged.kind {
            // A third of a step: below what the cache — or an eye — resolves.
            *azimuth += step / 3.0;
        }
        let after = ask(&mut cache, &object, &nudged);
        assert!(
            Arc::ptr_eq(&before, &after),
            "a sub-step nudge must not rebuild the geometry"
        );
    }

    /// Two lights that happen to lie the same way share one boolean.
    #[test]
    fn lights_pointing_the_same_way_share_geometry() {
        let mut cache = LightCache::new();
        let object = object(1);
        let a = sun();
        let mut b = sun();
        b.id = LightId(2);
        b.name = "Second".into();

        cache.begin();
        cache.set_defer(false);
        let first = ask(&mut cache, &object, &a);
        let second = ask(&mut cache, &object, &b);
        assert!(
            Arc::ptr_eq(&first, &second),
            "the same direction is the same crescent, whichever light lies in it"
        );
    }

    /// **A deferred miss draws the last crescents, not nothing.** This is what
    /// stops artwork flicking flat every time a light moves.
    #[test]
    fn a_deferred_miss_keeps_the_artwork_lit() {
        let mut cache = LightCache::new();
        let object = object(1);
        let light = sun();

        cache.begin();
        cache.set_defer(false);
        let lit = ask(&mut cache, &object, &light);
        assert!(!lit.is_empty(), "the warm build should have made crescents");

        // Now swing the light and refuse to build anything on this thread.
        let mut turned = light.clone();
        if let LightKind::Sun { azimuth, .. } = &mut turned.kind {
            *azimuth += 1.0;
        }
        cache.begin();
        cache.set_defer(true);
        let stale = ask(&mut cache, &object, &turned);
        assert!(
            Arc::ptr_eq(&stale, &lit),
            "a deferred miss must hand back the crescents it last had"
        );
        assert_eq!(cache.take_misses().len(), 1, "and queue the real build");
    }

    /// **A frame that could not build what it was asked for says so**, or the
    /// window sleeps with the wrong picture on screen and nothing ever replaces
    /// it. A frame that hit everything must not say so, or the window never
    /// sleeps at all.
    #[test]
    fn a_frame_reports_whether_its_shading_is_exact() {
        let mut cache = LightCache::new();
        let object = object(1);
        let light = sun();

        cache.begin();
        cache.set_defer(true);
        ask(&mut cache, &object, &light);
        assert!(cache.is_stale(), "a deferred frame drew the wrong shading");

        let built: Vec<Built> = cache.take_misses().into_iter().map(Miss::build).collect();
        cache.install(built);

        cache.begin();
        ask(&mut cache, &object, &light);
        assert!(!cache.is_stale(), "a frame that hit is not stale");
    }

    /// With nothing to fall back on — a cold cache — a deferred miss draws
    /// unlit, and the queued miss builds to what an inline build would have
    /// produced.
    #[test]
    fn deferred_misses_are_queued_and_build_the_same() {
        let object = object(1);
        let light = sun();

        let mut warm = LightCache::new();
        warm.begin();
        warm.set_defer(false);
        let inline = ask(&mut warm, &object, &light);

        let mut cache = LightCache::new();
        cache.begin();
        cache.set_defer(true);
        let handed_back = ask(&mut cache, &object, &light);
        assert!(
            handed_back.is_empty(),
            "with nothing cached, a deferred shape draws unlit for the frame"
        );

        let misses = cache.take_misses();
        assert_eq!(misses.len(), 1, "the miss should have been queued");
        let built: Vec<Built> = misses.into_iter().map(Miss::build).collect();
        assert_eq!(*built[0].geometry, *inline, "the deferred build must match");

        cache.install(built);
        assert_eq!(cache.len(), 1, "the built geometry is now cached");

        cache.set_defer(false);
        let hit = ask(&mut cache, &object, &light);
        assert!(!hit.is_empty(), "the installed geometry should be returned");
    }

    /// The same shape asked for twice in a frame is queued once.
    #[test]
    fn a_shape_asked_for_twice_is_queued_once() {
        let mut cache = LightCache::new();
        let object = object(1);
        let light = sun();

        cache.begin();
        cache.set_defer(true);
        ask(&mut cache, &object, &light);
        ask(&mut cache, &object, &light);

        assert_eq!(cache.take_misses().len(), 1, "queued once, not twice");
    }

    /// **Nothing is recorded while a build is in flight.** Whatever were
    /// recorded would be dropped, and recording one copies the whole path.
    #[test]
    fn misses_are_not_recorded_while_a_build_is_running() {
        let mut cache = LightCache::new();
        let object = object(1);
        let light = sun();

        cache.begin();
        cache.set_inline_budget(std::time::Duration::ZERO);
        cache.set_queue(false);
        ask(&mut cache, &object, &light);

        assert!(
            cache.take_misses().is_empty(),
            "a frame that cannot use its misses must not pay to collect them"
        );
        assert!(
            cache.is_stale(),
            "but it still drew the wrong shading, and must say so"
        );
    }

    /// **The inline budget bounds what a frame can build.** Past it, the rest
    /// is deferred — so there is no path through the cache that puts an
    /// unbounded number of booleans on the frame the user is waiting for.
    ///
    /// A nanosecond is the smallest budget that is not "defer everything": the
    /// first miss finds nothing spent and builds, and the second finds the
    /// budget gone. That makes the bound testable without timing anything.
    #[test]
    fn the_inline_budget_bounds_what_one_frame_builds() {
        let mut cache = LightCache::new();
        let light = sun();
        let objects: Vec<_> = (0..10).map(object).collect();

        cache.begin();
        cache.set_inline_budget(std::time::Duration::from_nanos(1));
        cache.set_queue(true);
        for o in &objects {
            ask(&mut cache, o, &light);
        }

        assert_eq!(cache.len(), 1, "only what the budget covers may be built");
        assert_eq!(cache.take_misses().len(), 9, "the rest is deferred");

        // And the budget is per frame, not for the life of the cache.
        cache.begin();
        cache.set_inline_budget(std::time::Duration::MAX);
        for o in &objects {
            ask(&mut cache, o, &light);
        }
        assert_eq!(cache.len(), 10, "a fresh frame gets a fresh budget");
    }

    /// Tweened artwork has no owner and is built inline even with no budget —
    /// there is nothing to defer it *to*, since it will not be here next frame.
    #[test]
    fn ownerless_artwork_builds_inline_even_when_deferring() {
        let mut cache = LightCache::new();
        let object = object(1);
        let light = sun();

        cache.begin();
        cache.set_defer(true);
        let geometry = cache.crescents(
            None,
            0,
            &path_of(&object),
            Affine::IDENTITY,
            towards(&light),
            light.softness,
        );

        assert!(cache.take_misses().is_empty(), "nothing should be queued");
        assert!(
            !geometry.is_empty(),
            "tweened artwork must still be lit on the spot"
        );
    }

    /// A shape that moves gets new geometry, and so does one that turns — the
    /// old key was a quantised centre point and could not see a rotation at all.
    #[test]
    fn moving_or_turning_a_shape_rebuilds_its_geometry() {
        let mut cache = LightCache::new();
        let object = object(1);
        let light = sun();
        let path = path_of(&object);
        let ask_at = |cache: &mut LightCache, doc: Affine| {
            cache.crescents(Some(&object), 0, &path, doc, towards(&light), light.softness)
        };

        cache.begin();
        cache.set_defer(false);
        ask_at(&mut cache, Affine::IDENTITY);
        ask_at(&mut cache, Affine::translate((300.0, 0.0)));
        ask_at(&mut cache, Affine::rotate(0.7));

        assert_eq!(cache.len(), 3, "each placement needs its own geometry");
    }

    #[test]
    fn entries_are_dropped_when_they_stop_being_drawn() {
        let mut cache = LightCache::new();
        let object = object(1);

        cache.begin();
        cache.set_defer(false);
        ask(&mut cache, &object, &sun());
        cache.end();
        assert_eq!(cache.len(), 1, "still fresh");

        for _ in 0..KEEP_FRAMES + 1 {
            cache.begin();
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

        cache.begin();
        cache.set_defer(false);
        ask(&mut cache, &object, &sun());
        cache.end();

        cache.begin();
        cache.end();

        cache.begin();
        let again = ask(&mut cache, &object, &sun());
        assert!(
            !again.is_empty(),
            "the geometry should still describe a shape"
        );
        assert_eq!(cache.len(), 1);
    }

    /// A drag across the dial must not pile up geometry for every step it
    /// passed through.
    #[test]
    fn a_long_drag_does_not_accumulate_unbounded_geometry() {
        let mut cache = LightCache::new();
        let object = object(1);
        let mut light = sun();

        cache.begin();
        cache.set_defer(false);
        for _ in 0..200 {
            if let LightKind::Sun { azimuth, .. } = &mut light.kind {
                *azimuth += 0.05;
            }
            ask(&mut cache, &object, &light);
        }

        let slot = cache.places.values().next().expect("the one placement");
        assert!(
            slot.built.len() <= MAX_AIMS,
            "a drag kept {} directions of geometry",
            slot.built.len()
        );
    }
}
