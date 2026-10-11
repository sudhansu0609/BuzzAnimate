//! Encoding a document into a Vello scene.
//!
//! This is the walk from [`buzz_scene::Scene`] down through layers, tweens,
//! groups and nested symbols to the fills and strokes Vello draws. It lives
//! here, below the application, because **three callers must agree exactly**:
//! the window, the exporter, and the headless tests that read pixels back. A
//! second copy of this walk anywhere would be a second answer to "what does
//! this document look like", and the difference would show up as an export
//! that does not match the screen — the single most damaging kind of bug an
//! animation tool can have.
//!
//! Chrome is deliberately *not* here. Rulers, guides, selection handles and
//! the brush preview are authoring aids drawn in screen space by the
//! application, and they must never reach an exported frame.

use buzz_geom::{Affine, Projection, RenderClip, RenderSplit, Shape as _};
use buzz_scene::{ColorTransform, LayerKind, Object, ObjectKind, Scene};
use peniko::Color;

use crate::{PathLineStats, SceneBuilder};
use crate::filters::FilterCache;
use crate::lighting::LightCache;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// How deep a symbol may nest before we stop.
///
/// A symbol containing an instance of itself is a cycle; without a limit the
/// renderer would recurse until the stack ran out. Animate refuses to create
/// one, but an imported or hand-edited file can contain it.
pub const MAX_SYMBOL_DEPTH: usize = 12;

/// Modifiers for drawing one frame.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FrameOptions {
    /// Onion-skin ghost alpha. `None` draws the frame normally.
    pub ghost: Option<f64>,
    /// Draw ghosts as outlines rather than faded artwork.
    pub ghost_outlines: bool,
    /// When a mask actually clips what is under it.
    pub masks: MaskDisplay,
    /// Draw the document's lights.
    ///
    /// On by default: an unlit document has no lights, so this costs a boolean
    /// check. Off is for the authoring passes that want the artwork as drawn —
    /// onion-skin ghosts, which are reference rather than picture.
    pub lit: bool,
    /// Draw the lamps' pools of light over the finished frame.
    ///
    /// On by default, and separate from `lit` because it belongs to the **frame**
    /// rather than to a layer: a pool is light in the air, laid once over
    /// everything, where shading and shadows are drawn per layer. A screen frame
    /// that draws the document several times over — Edit Multiple Frames — wants
    /// exactly one of them, so the extra passes turn this off and keep their
    /// shading.
    pub pools: bool,
    /// Honour each layer's working transparency.
    ///
    /// **Off by default, which is the export.** Layer transparency is a thing
    /// an animator does to see what they are doing — dimming a reference layer
    /// to draw over it — and a film that came out faded because somebody left
    /// a layer at 40% would be a trap rather than a feature. The stage turns it
    /// on; nothing else does.
    pub layer_alpha: bool,
    /// Where this stack sits in the document's space.
    ///
    /// Identity for the document's own timeline. Editing a symbol **in place**
    /// draws its contents through the transform of the instance that was
    /// opened, so a head stays on the shoulders it belongs to instead of
    /// jumping to the origin — see [`buzz_scene::Scene::edit_place`].
    pub place: Affine,
    /// **How far into the frame the shutter is**, in frames, for motion blur.
    ///
    /// Zero — the default, and every pass but one — draws the frame at its own
    /// instant, exactly as it always has. A motion-blurred export renders the
    /// same frame at a succession of these offsets and adds the results up; the
    /// offset is what makes each of those a *different* picture, by moving the
    /// camera, the tweens and the wiggles on to where they are part-way through
    /// the frame. See [`FrameOptions::at`].
    pub subframe: f64,
    /// **Draw guide layers.**
    ///
    /// A guide is reference geometry — a motion path, a perspective grid, a
    /// photograph to trace over. `LayerKind::paints_to_output` has said since
    /// it was written that a guide does not reach the film, and until this
    /// existed nothing read it: the render walk asked `paints_on_stage`, which
    /// is the *authoring* question, so a guide was drawn faded into the export
    /// as well as onto the stage.
    ///
    /// Faded, so it survived review as "a bit of the background" rather than as
    /// a bug — and a traced photograph delivered at 35% over the drawing is
    /// exactly the outcome the layer kind exists to prevent.
    ///
    /// On by default, which is the stage. The exporter turns it off.
    pub guides: bool,
    /// The visible rectangle in **document space**, for culling.
    ///
    /// Skips artwork far off the edge of a huge document rather than
    /// transforming, brushing and encoding it into a scene that clips it away
    /// anyway. The window passes its viewport rect. **The exporter culls too
    /// now** — to the render region or the stage, grown by a small margin
    /// (`Exporter::draw_into_target`), and the export preview strip likewise
    /// (`preview.rs`) — because an import too large to draw at all is a worse
    /// outcome than a saved millisecond, and leaving the whole document
    /// unculled is exactly what let a frame overflow the rasteriser's buffers
    /// unnoticed. `None` still means "render everything, undisturbed": the
    /// thumbnail renderer and a headless test comparing pixels against an
    /// unculled reference both rely on it.
    ///
    /// The cull is applied conservatively — culling too little only costs
    /// time, culling too much is a wrong picture — so it can only remove
    /// geometry a straight, unculled render would also have discarded once
    /// clipped to the viewport. That is the parity
    /// `culling_to_the_viewport_matches_no_cull` proves: a culled render and
    /// one with `cull: None` come out pixel-identical. See [`DrawCtx::cull`].
    pub cull: Option<buzz_geom::Rect>,
    /// **A narrower rectangle to judge a stamped symbol's "mostly
    /// off-screen" rule against, in place of `cull` above.**
    ///
    /// `None` means "the same as `cull`", which every caller uses: a band is
    /// culled to its own strip (`cull`), and `try_stamp_symbol`'s rule is
    /// judged against that same strip, so a character mostly inside one band
    /// is stamped once there and drawn live, culled — genuinely smaller — in
    /// whichever other band it barely touches. A follow-up once fed this
    /// field the *frame's* whole visible rectangle instead, reasoning that a
    /// band's narrow strip would fail the rule for every character
    /// straddling a boundary — but the fix was worse than the problem: a
    /// symbol that passed the frame-wide rule was appended *whole*, unclipped
    /// to any one band, into every band it touched, so more bands stopped
    /// shrinking the largest band's own lines and an over-budget band
    /// rasterised as black. See H0 item 1 in `docs/IMPORT_PERF_PLAN.md` and
    /// [`DrawCtx::stamp_view`]. Left in place rather than removed: a future
    /// caller with a genuine reason to judge the rule against something other
    /// than its own `cull` still has somewhere to say so.
    pub stamp_view: Option<buzz_geom::Rect>,
}

impl Default for FrameOptions {
    fn default() -> Self {
        Self {
            ghost: None,
            ghost_outlines: false,
            masks: MaskDisplay::default(),
            lit: false,
            // Gated by `lit` anyway, so a pass that wants no lighting gets no
            // pools without having to say so twice.
            pools: true,
            layer_alpha: false,
            guides: true,
            place: Affine::IDENTITY,
            subframe: 0.0,
            cull: None,
            stamp_view: None,
        }
    }
}

impl FrameOptions {
    /// The instant this pass is drawing: `frame`, moved on by the shutter
    /// offset.
    ///
    /// With no offset this is the whole frame, and every lookup it is handed to
    /// answers bit-for-bit what it answered before there was such a thing as a
    /// sub-frame — which is the parity the un-blurred export depends on.
    pub fn at(&self, frame: u32) -> f64 {
        frame as f64 + self.subframe
    }
}

/// When mask layers clip the layers they claim.
///
/// # Why this is a choice at all
///
/// Animate does **not** show the mask effect while you are editing: a mask
/// layer clips its masked layers only once both are *locked*, because you
/// cannot draw inside a region you cannot see. Published output always masks.
/// So the stage asks for [`MaskDisplay::WhenLocked`] and the exporter asks for
/// [`MaskDisplay::Always`], and the difference between the window and the
/// finished frame is a deliberate, Animate-shaped one rather than an accident.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum MaskDisplay {
    /// Always clip — what an export or a preview means.
    #[default]
    Always,
    /// Clip only where the mask layer is locked, as Animate's stage does.
    WhenLocked,
}

/// Draw a document's artwork for one frame, with its camera: no onion skins,
/// no preview, no editor chrome.
///
/// This is what an export renders and what a headless test reads back. The
/// document camera is applied here rather than by the caller, because a helper
/// that quietly skipped it would report that a camera pan moves nothing.
pub fn draw_document(builder: &mut SceneBuilder<'_>, scene: &Scene, frame: u32) {
    let camera = scene.camera_transform(frame);
    draw_frame(builder, scene, frame, camera, &FrameOptions::default());
}

/// Draw one frame's layers through a given camera transform.
pub fn draw_frame(
    builder: &mut SceneBuilder<'_>,
    scene: &Scene,
    frame: u32,
    camera: Affine,
    options: &FrameOptions,
) {
    draw_frame_lit(
        builder,
        scene,
        frame,
        camera,
        options,
        &mut LightCache::new(),
    );
}

/// Everything the draw walk keeps between frames.
///
/// Two caches travel together because they are both generated geometry with
/// the same lifetime and the same eviction rule; passing them separately would
/// mean two more arguments through every function in this file.
#[derive(Debug, Default)]
pub struct DrawCache {
    pub lights: LightCache,
    pub filters: FilterCache,
    pub bounds: BoundsCache,
    /// Which symbols hold an animated instance, for cache keys that must move
    /// with the playhead only when they have to. See [`AnimatedMemo`].
    animated: AnimatedMemo,
    /// A layer's rim and filter ops, kept while its content holds still. See
    /// [`LayerFxMemo`].
    layer_fx: LayerFxMemo,
    /// Rig parts' outlines for the silhouette gather. See [`SilhouetteMemo`].
    silhouette: SilhouetteMemo,
    /// A layer's whole shadow pass as an appendable fragment. See [`ShadowMemo`].
    shadow_memo: ShadowMemo,
    /// Per-symbol facts (fingerprint, extent, cache-eligibility), rebuilt only
    /// when the document changes rather than every frame.
    pub symbols: SymbolTable,
    /// Whole eligible symbols encoded once and stamped per instance.
    pub symbol_scenes: SymbolSceneCache,
    /// The gradients a ramping lamp is drawn with, and the light field they were
    /// built from. One entry: every shape a lamp reaches asks for the same
    /// three, so the first shape of a frame builds them and the rest share.
    lamp: Option<(u64, Arc<LampPaints>)>,
    /// How much of the light this cache's frames can afford to draw. See
    /// [`LightDetail`].
    detail: LightDetail,
    /// What the last frame judged by [`DrawCache::reconsider`] cost, for the
    /// HUD and for tests that need to see the number the level was chosen from.
    encode: u32,
    /// The ceiling the last frame was judged against, which depends on the
    /// output it was drawn at. Kept for the HUD and for tests.
    ceiling: u32,
    /// A level held against the judgement, for a caller that has already
    /// decided — a test measuring one level, or an animator who would rather
    /// have the modelling off than have it come and go.
    pinned: Option<LightDetail>,
    /// The last frame's own [`crate::SceneBuilder::estimated_lines`], set by
    /// [`Self::note_lines`] just before [`Self::reconsider`] judges it.
    /// Stays `0` for a caller that never calls `note_lines`, which is exactly
    /// today's segment-only behaviour — see Phase 4c in
    /// `docs/FLA_IMPORT_BLANK_AND_LAG_PLAN.md`.
    lines: u64,
    /// **L7 (`docs/RIG_SCENE_LAG_PLAN.md`): what each [`LightDetail`] pass
    /// measured this screen frame, at the current [`Self::scale`]** — indexed by
    /// [`LightDetail::index`]. Reset in [`Self::begin`] and whenever the scale
    /// changes (a level refused at one scale says nothing about another). Lets
    /// [`Self::reconsider`] refuse to step up into a level it has just seen
    /// over budget, and [`Self::fitting_fallback`] name a level known to fit.
    measured_lines: [Option<u64>; 4],
    /// The encoded segments of the same passes as [`Self::measured_lines`], so
    /// a level refused by the *segment* ceiling is refused too.
    measured_segments: [Option<u32>; 4],
    /// Per level, the measured `lines(level.up()) / lines(level)`, kept across
    /// frames. See [`RatioMemo`].
    step_ratio: [RatioMemo; 4],
    /// Did the last pass [`Self::reconsider`] judged exceed the segment ceiling
    /// or the line budget? See [`Self::over_budget_last_pass`].
    last_pass_over: bool,
    /// The level the last pass judged was *encoded* at. [`Self::detail`] has
    /// already moved on when that judgement stepped the ladder.
    last_pass_detail: LightDetail,
    /// The `vello_encoding` buffer scale this cache judges [`segment_ceiling`]
    /// and [`line_budget`] against — see [`Self::set_scale`]. P2 in
    /// `docs/IMPORT_PERF_PLAN.md`: the budget belongs to this cache, not the
    /// process, so `DrawCache::default()` (every test) stays at scale `1`
    /// regardless of what any GPU context elsewhere in the process has set
    /// the global [`vello_encoding::buffer_scale`] to.
    scale: BudgetScale,
    /// The ceiling [`Self::scale`] may grow to before [`Self::reconsider`]
    /// falls back to stepping the detail ladder down — this cache's own copy
    /// of [`crate::GpuContext::max_buffer_scale`], adopted through
    /// [`Self::set_max_scale`]. Defaults to `1` (same as [`Self::scale`]
    /// itself), so a cache nobody has called that on never grows and
    /// behaves exactly as it did before V3 in
    /// `docs/VRAM_ADAPTIVE_BUFFERS_PLAN.md` — every existing test that never
    /// calls it is unaffected.
    max_scale: BudgetScale,
    /// Since when a judged frame has comfortably fit at half [`Self::scale`]
    /// — set by [`Self::note_shrink_headroom`], read by
    /// [`Self::pending_shrink`]. `None` whenever the last frame judged did
    /// not comfortably fit, or [`Self::scale`] is already `1`. See V4.
    shrink_eligible_since: Option<Instant>,
    /// The half scale [`Self::note_shrink_headroom`] is offering, once
    /// [`Self::shrink_eligible_since`] has held for
    /// [`Self::SHRINK_IDLE`] — see [`Self::pending_shrink`].
    pending_shrink: Option<u32>,
    /// Encoded layer fragments, reused across frames when a layer's own
    /// content, lighting and view have not changed. P2 in
    /// `docs/IMPORT_PERF_PLAN.md`.
    layer_scenes: LayerSceneCache,
    /// **E5: a flag that ends a pass early.** Set by whoever lent this cache to
    /// an encode thread (see `buzz-app`'s `encode_ahead`) to say "stop, the
    /// frame being drawn is not wanted any more". Checked between layers by
    /// [`draw_layers`]; `None` (the default) means never aborted. See
    /// [`DrawCache::set_abort`].
    abort: Option<Arc<std::sync::atomic::AtomicBool>>,
    /// **E7: draw only every `n`th object of a frame, starting at `k`** (`(k, n)`,
    /// counted across the stage layers in paint order, from [`Self::begin`]).
    /// For a stamp pre-build worker that is given a share of one frame's work;
    /// the scene it draws is partial and thrown away. `None` (the default) draws
    /// every object. See [`DrawCache::set_work_slice`].
    work_slice: Option<(usize, usize)>,
    /// Objects seen so far this generation, for [`Self::work_slice`].
    slice_counter: usize,
    /// Test hook: raise the abort flag once this many symbol layers have been
    /// drawn (inside a symbol's own contents), so a test can stop a pass in the
    /// middle of a stamp build deterministically. `None` (the default) never
    /// trips. See [`DrawCache::trip_abort_after_symbol_layers`].
    abort_trip: Option<usize>,
}

/// The last eight measured `lines(up) / lines(level)` ratios of one
/// [`LightDetail`] step, as a ring; [`Self::max`] is the one the ladder trusts.
/// A max, not a mean: the step up is refused on the worst case seen, because the
/// mistake it guards against costs the whole frame (L7).
#[derive(Debug, Clone, Copy, Default)]
struct RatioMemo {
    ring: [f32; 8],
    next: usize,
    len: usize,
}

impl RatioMemo {
    fn push(&mut self, ratio: f32) {
        if !ratio.is_finite() || ratio <= 0.0 {
            return;
        }
        self.ring[self.next] = ratio;
        self.next = (self.next + 1) % self.ring.len();
        self.len = (self.len + 1).min(self.ring.len());
    }

    fn max(&self) -> Option<f32> {
        self.ring[..self.len.min(self.ring.len())]
            .iter()
            .copied()
            .fold(None, |m, r| Some(m.map_or(r, |m: f32| m.max(r))))
    }
}

/// [`DrawCache::scale`]'s own type, so `#[derive(Default)]` on `DrawCache`
/// still lands on scale `1` rather than `0` — a plain `u64` field would
/// default to `0`, which would zero every budget it multiplies.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct BudgetScale(u64);

impl Default for BudgetScale {
    fn default() -> Self {
        Self(1)
    }
}

/// **How much of the light a frame can afford to draw.**
///
/// # Why a frame can fail to afford it
///
/// Vello rasterises from fixed-size buffers, sized once in
/// `vello_encoding::BufferSizes` from numbers its own comment calls "hand
/// picked". The one that matters here holds `1 << 21` flattened lines. Past
/// that the bump allocator fails, and `Renderer::render_to_texture` — the call
/// the window and the exporter both make — **does not check**: the fine pass
/// writes nothing and the target keeps whatever was in it before.
///
/// So an over-large frame is not slow, and it is not wrong. It is *absent*.
/// The window presents the previous picture again, which is indistinguishable
/// from nothing having changed — and since what most often pushes a frame over
/// is switching the lights on, what it looks like is that the lights do
/// nothing. Measured on the film this was reported from: 1.57 M lines unlit,
/// 1.90 M with the light's colour and its cast shadows, and 3.46 M once the
/// shading crescents were added — 165% of the buffer, so every lit frame was
/// discarded and the stage sat on its last unlit one. Switching the lamp on,
/// moving it, recolouring it, even setting the ambient to pure red, all
/// produced a byte-identical picture.
///
/// # What is given up, and in what order
///
/// The artwork itself is never given up: a frame that cannot draw its own
/// drawing has nothing to say. What lighting adds is given up instead, cheapest
/// thing kept longest:
///
/// * **The tint is never given up.** A lit colour is folded into the paint the
///   shape was going to be drawn with anyway ([`buzz_light::Illumination::apply`]),
///   so it costs no geometry at all. Neither does a lamp's pool, which is one
///   circle for the whole frame. At every level below, the light still colours
///   the picture and still lays its pool — the light *works*.
/// * **The modelling goes first.** A terminator and a highlight are a boolean
///   difference each, and on dense artwork they come out at about 4.7× the
///   geometry of the shape they model. That is the one part of lighting whose
///   cost is a multiple of the drawing's own.
/// * **Then the cast shadows**, which are the artwork under one affine — about
///   1× — and only reached by a document whose artwork alone is close to the
///   ceiling.
/// * **The seam seal is never given up automatically, and it is the one thing
///   here that is not lighting at all.** Every opaque fill is stroked a second
///   time, at a hairline width, to close the rasteriser seam between abutting
///   shapes ([`crate::SceneBuilder::fill_shape_paint_sealed`]) — which doubles
///   the segments a dense frame's *own artwork* costs, independent of whether
///   any light is even on. Trading it away would open a seam on every abutting
///   edge in the frame, and Phase 4's banded rendering (see
///   `docs/FLA_IMPORT_BLANK_AND_LAG_PLAN.md`) makes an over-budget frame safe
///   without giving anything up, so the ladder stops once modelling and
///   shadows are both gone: a document whose artwork alone overflows the
///   ceiling at [`LightDetail::Flat`] draws in bands instead (Phase 4d), still
///   sealed. See `docs/RENDER_SEAMS_PLAN.md` Step 1.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LightDetail {
    /// Everything: tint, pool, cast shadows, terminator and highlight.
    #[default]
    Full,
    /// No terminator and no highlight. Tint, pool and cast shadows stand.
    NoModelling,
    /// Tint and pool only.
    Flat,
    /// Tint and pool only, and the seam seal is given up too — the artwork
    /// itself draws unsealed. Below every lighting level because it is not a
    /// lighting level, and below [`Self::Flat`] specifically: nothing selects
    /// it automatically any more. The automatic ladder stops at `Flat`, and a
    /// frame still over budget there draws in bands instead (Phase 4d) rather
    /// than trading the seal for budget — see `docs/RENDER_SEAMS_PLAN.md` Step
    /// 1. Reachable only by pinning ([`DrawCache::pin_detail`]) — a draft mode,
    /// or a test.
    Unsealed,
}

impl LightDetail {
    /// May a shape draw its terminator and highlight?
    fn models(self) -> bool {
        self == Self::Full
    }

    /// May a shape cast a shadow?
    fn casts(self) -> bool {
        self == Self::Full || self == Self::NoModelling
    }

    /// May an opaque fill be sealed with the sub-pixel stroke that closes the
    /// rasteriser seam between abutting shapes? False only at the bottom
    /// level, where even the artwork's own doubled cost has to give.
    fn seals(self) -> bool {
        self != Self::Unsealed
    }

    /// This level's slot in [`DrawCache`]'s per-level measurements (L7).
    fn index(self) -> usize {
        match self {
            Self::Full => 0,
            Self::NoModelling => 1,
            Self::Flat => 2,
            Self::Unsealed => 3,
        }
    }

    fn down(self) -> Self {
        match self {
            Self::Full => Self::NoModelling,
            Self::NoModelling => Self::Flat,
            // The automatic ladder stops here: trading the seal for budget
            // opens every seam on the frame, and bands (Phase 4d) make an
            // over-budget frame safe without giving anything up. See
            // `docs/RENDER_SEAMS_PLAN.md` Step 1.
            Self::Flat => Self::Flat,
            // Only reached by a pinned `Unsealed` that has since been
            // unpinned; holds rather than re-sealing under `down`, which
            // would be a step *up* in quality wearing a step-down's name.
            Self::Unsealed => Self::Unsealed,
        }
    }

    fn up(self) -> Self {
        match self {
            Self::Unsealed => Self::Flat,
            Self::Flat => Self::NoModelling,
            Self::NoModelling | Self::Full => Self::Full,
        }
    }

    /// How small a frame has to be here before it is worth stepping up.
    ///
    /// **Stepping up has to be safe, or the two levels alternate for ever, one
    /// frame each.** So the room asked for is the room the step itself needs.
    /// Measured on the film this was reported from — 617 k encoded segments
    /// flat, 921 k with cast shadows, 2.46 M with the modelling as well —
    /// shadows cost about half as much again and modelling about two and a half
    /// times on top. Both bounds below are well past those, so a frame small
    /// enough to step up is small enough to stay stepped up.
    ///
    /// Conservative on purpose, and it errs the same way everything else here
    /// does: a document between the two thresholds keeps a level it might have
    /// climbed out of, which costs it some shading. The other mistake costs it
    /// the frame.
    ///
    /// **`Unsealed` is different: stepping up from it re-seals the artwork,
    /// which doubles the fills' own segments** rather than adding a multiple of
    /// lighting geometry on top of a budget already proven to fit. So the room
    /// asked for is room for that doubling itself, not the smaller margins the
    /// lighting levels above it ask for.
    fn room_to_step_up(self, ceiling: u32) -> u32 {
        match self {
            Self::Unsealed => ceiling * 2 / 5,
            Self::Flat => ceiling / 3,
            Self::NoModelling => ceiling / 6,
            Self::Full => u32::MAX,
        }
    }
}

/// **The encoded path segments a frame may reach before lighting is trimmed**,
/// at an output of `pixels` device pixels.
///
/// Vello's line buffer holds `1 << 21` — 2.1 M — flattened lines, and what a
/// segment costs in lines depends on two things. The artwork: a straight edge
/// is one line however it is drawn, a curve is many. And the *output*, because
/// flattening is to a tolerance in device pixels — the same document rendered
/// larger is flattened finer and costs more lines for exactly the same
/// geometry. Lines per curve go as the square root of the tolerance, so
/// capacity in segments goes as the inverse square root of the output's linear
/// size, which is what this is.
///
/// # Where the constant comes from
///
/// Measured, not derived, by rendering one dense film at six output sizes with
/// each level of [`LightDetail`] pinned and watching for the frame that never
/// landed:
///
/// | output | 617 k segments | 921 k | 2.46 M |
/// |---|---|---|---|
/// | 512 × 512 | fits | fits | **lost** |
/// | 1295 × 855 | fits | fits | lost |
/// | 1920 × 1200 | fits | fits | lost |
/// | 2080 × 1300 | fits | **lost** | lost |
/// | 3840 × 2160 | fits | lost | lost |
///
/// A capacity of `C / sqrt(linear)` reproduces every one of those with
/// `C ≈ 36 M`. The constant below is a tenth under that, which is the margin
/// for artwork curvier than the film it was fitted to: erring low costs a
/// document some shading, erring high costs it the frame.
///
/// # Why not just count the lines
///
/// Because the flattening happens on the GPU. Vello can report what it
/// actually used — `BumpAllocators` — but only through the async render path
/// with the `debug_layers` feature, which means a full GPU sync per frame. So
/// this stays a proxy over the one number available before the frame is
/// submitted: [`crate::SceneBuilder::encoded_segments`].
pub fn segment_ceiling(pixels: f64) -> u32 {
    segment_ceiling_at(pixels, buffer_scale())
}

/// [`segment_ceiling`], at an explicit `scale` rather than the process-wide
/// [`buffer_scale`]. What [`DrawCache::reconsider`] calls, with the scale it
/// captured in [`DrawCache::set_scale`] — see P2 in
/// `docs/IMPORT_PERF_PLAN.md`. [`segment_ceiling`] itself stays the one
/// callers without a `DrawCache` reach — the exporter's band-splitting judges
/// a single band without reconsidering, so there is no cache state to ask.
pub fn segment_ceiling_at(pixels: f64, scale: u64) -> u32 {
    // **An output size that is not a size answers for an ordinary window.**
    //
    // The window derives the stage's rectangle from the one egui gave the
    // central panel, and that is `egui::Rect::NOTHING` — infinities — until the
    // layout has been measured once: the first frame of a session, and a frame
    // again after a resize. Multiplied out that is an infinite area, an
    // infinite divisor and a ceiling of zero, which refuses *every* frame's
    // lighting and drops the level to `Flat` on a document that had no trouble
    // at all. `an_unmeasured_stage_area_still_draws_the_artwork` is what caught
    // it, having been written for the same rectangle blacking the whole stage.
    let pixels = if pixels.is_finite() && pixels > 0.0 {
        pixels.clamp(REFERENCE_PIXELS / 64.0, REFERENCE_PIXELS * 64.0)
    } else {
        REFERENCE_PIXELS
    };
    // The output's linear size, as the side of the square with that area, so a
    // wide frame and a tall one of the same area are charged the same.
    let linear = pixels.sqrt();
    ((CEILING_CONSTANT * scale as f64) / linear.sqrt()) as u32
}

/// What an unmeasured output is taken to be: an ordinary full-HD window. Also
/// the middle of the range [`segment_ceiling`] will answer for at all, since a
/// degenerate one-pixel viewport should not be granted an unbounded frame.
const REFERENCE_PIXELS: f64 = 1920.0 * 1080.0;

/// The fitted constant of [`segment_ceiling`] at [`buffer_scale`] `1` — Vello's
/// own upstream buffer sizes. See there for the measurements.
const CEILING_CONSTANT: f64 = 33_000_000.0;

/// **The active [`vello_encoding`] buffer scale** — see `GpuContext::new`
/// (`crates/buzz-render/src/lib.rs`) and `third_party/vello_encoding`, P1 in
/// `docs/IMPORT_PERF_PLAN.md`. [`line_budget`] and [`segment_ceiling`] both
/// read the scale from this one place, so a bigger single-pass Vello buffer
/// raises both ceilings together.
fn buffer_scale() -> u64 {
    u64::from(vello_encoding::buffer_scale())
}

/// **The flattened lines a frame may reach before it is judged over budget**,
/// alongside [`segment_ceiling`] rather than instead of it.
///
/// Vello's own line buffer holds `1 << 21` — 2,097,152 — at [`buffer_scale`]
/// `1`, and scales with it above that. `segment_ceiling` assumes about 1.7
/// lines per encoded segment, which curvy fills and strokes that flatten on
/// both sides plus their joins can run well past — measured at about 2.6 on
/// the film this is fitted to, which let a frame at 85% of the segment
/// ceiling actually cost 128% of the line buffer and get dropped anyway (see
/// "Status after Phases 1-3" in `docs/FLA_IMPORT_BLANK_AND_LAG_PLAN.md`). 86%
/// of the real buffer, the same margin `segment_ceiling`'s own constant
/// leaves under its fitted value.
///
/// A function, not a `const`, unlike [`CEILING_CONSTANT`]: both read
/// [`buffer_scale`] rather than baking in Vello's own fixed upstream sizes.
/// `pub`: the exporter's own band-splitting (Phase 4d) judges a single band's
/// `estimated_lines()` against this same number from another crate, rather
/// than against a `DrawCache` — a band is drawn without reconsidering, so
/// there is no cache state to ask.
pub fn line_budget() -> u64 {
    line_budget_at(buffer_scale())
}

/// [`line_budget`], at an explicit `scale` — see [`segment_ceiling_at`].
pub fn line_budget_at(scale: u64) -> u64 {
    1_800_000 * scale
}

/// **S9, size once: the buffer scale that holds a frame of `lines` flattened
/// lines and `segments` encoded segments at `output_pixels`**, by the same
/// formula [`DrawCache::reconsider`] jumps with (S4): the smallest power of two
/// covering the heavier of the two budgets with 10% headroom, clamped to
/// `max_scale`. `1` for a frame that fits the base buffers.
pub fn fitting_scale(lines: u64, segments: u32, output_pixels: f64, max_scale: u32) -> u32 {
    let seg_at_1 = f64::from(segment_ceiling_at(output_pixels, 1)).max(1.0);
    let lines_at_1 = line_budget_at(1) as f64;
    let need = (f64::from(segments) / seg_at_1).max(lines as f64 / lines_at_1) * 1.1;
    let target = (need.ceil().max(1.0) as u64)
        .checked_next_power_of_two()
        .unwrap_or(u64::from(max_scale));
    target.min(u64::from(max_scale.max(1))) as u32
}

impl DrawCache {
    /// The lamp's gradients, built if this is the first shape to ask.
    ///
    /// Keyed on [`buzz_light::LightField::fingerprint`], which moves when — and
    /// only when — the gradients would differ. A rig of one lamp over one layer
    /// therefore builds them once a frame however many shapes it lights, while a
    /// sky mixing by height still gives shapes at different heights their own.
    fn lamp_paints(
        &mut self,
        field: &buzz_light::LightField,
        centre: buzz_geom::Point,
        reach: f64,
    ) -> Arc<LampPaints> {
        let key = field.fingerprint();
        if let Some((cached, paints)) = &self.lamp
            && *cached == key
        {
            return Arc::clone(paints);
        }
        let paints = Arc::new(LampPaints::build(field, centre, reach));
        self.lamp = Some((key, Arc::clone(&paints)));
        paints
    }

    /// How much of the light the frames drawn through this cache may show.
    pub fn detail(&self) -> LightDetail {
        self.detail
    }

    /// The encoded path segments of the last frame judged, in the units
    /// [`segment_ceiling`] is expressed in.
    pub fn last_encode(&self) -> u32 {
        self.encode
    }

    /// The flattened lines [`Self::note_lines`] recorded for the last frame
    /// judged — the single-pass estimate [`line_budget`] is judged against.
    /// `examples/stage_cost.rs` reports this for every sampled frame, banded
    /// or not, so which frames actually need bands is visible (P1 in
    /// `docs/IMPORT_PERF_PLAN.md`).
    pub fn last_lines(&self) -> u64 {
        self.lines
    }

    /// What [`segment_ceiling`] came to for the last frame judged — which needs
    /// that frame's output size, so it is remembered rather than recomputed.
    pub fn last_ceiling(&self) -> u32 {
        self.ceiling
    }

    /// Hold the level here, or (`None`) let each frame be judged again.
    pub fn pin_detail(&mut self, pinned: Option<LightDetail>) {
        self.pinned = pinned;
        if let Some(detail) = pinned {
            self.detail = detail;
        }
    }

    /// **Judge the encode just finished, and say whether to build it again.**
    ///
    /// Called by whoever owns the `vello::Scene` — the window's stage pass, the
    /// exporter — with [`crate::SceneBuilder::encoded_segments`] once the frame
    /// is built and before it is submitted. A `true` answer means the level has
    /// changed and this frame must be encoded a second time at the new one;
    /// build in a loop until it answers `false`. It answers `false` on the
    /// fourth pass at the latest for any given frame, because a level is only
    /// ever stepped one at a time and there are four of them.
    ///
    /// Judging *after* the encode rather than guessing before it is the whole
    /// point: the number is Vello's own count of what it will rasterise, so it
    /// cannot disagree with what was drawn. The cost is one wasted encode on
    /// the frame where the verdict changes, and the verdict is remembered, so
    /// every frame after it builds once.
    ///
    /// A frame still over the ceiling at [`LightDetail::Flat`] is a frame
    /// whose *artwork* will not fit even with nothing left to trim; the ladder
    /// stops there rather than trading the seal away, and bands (Phase 4d)
    /// take over instead — see `docs/RENDER_SEAMS_PLAN.md` Step 1.
    ///
    /// **Judges lines as well as segments** (Phase 4c). `segment_ceiling` is a
    /// proxy that assumes a fixed lines-per-segment ratio, and curvy, heavily
    /// stroked artwork can run well past it while still under the segment
    /// ceiling — see [`line_budget`]. A caller that never calls
    /// [`Self::note_lines`] leaves `lines` at `0`, which never trips either
    /// branch below, so it is judged on segments exactly as before.
    ///
    /// **V3 (`docs/VRAM_ADAPTIVE_BUFFERS_PLAN.md`): an overflow grows the
    /// buffer scale before it trims any detail, while [`Self::scale`] has
    /// room left to grow towards [`Self::max_scale`].** The ladder below is
    /// reached only once already there — exactly today's behaviour for any
    /// cache nobody has raised `max_scale` on, since `scale < max_scale` is
    /// then never true. Growing never gives anything up, so it returns
    /// `true` (encode again) the same way a detail change does; the caller's
    /// own loop re-encodes the same frame at the same detail, which now
    /// fits. The caller is responsible for actually growing its
    /// `GpuContext` (`GpuContext::grow_to(cache.scale())`) before the next
    /// real render call — this only judges budgets, it does not touch a GPU.
    pub fn reconsider(&mut self, segments: u32, output_pixels: f64) -> bool {
        self.encode = segments;
        self.ceiling = segment_ceiling_at(output_pixels, self.scale.0);
        // L7: remember what this level cost, and which level it was encoded at
        // (`self.detail` is about to move on).
        self.last_pass_over = false;
        self.last_pass_detail = self.detail;
        self.measured_segments[self.detail.index()] = Some(segments);
        if self.pinned.is_some() {
            return false;
        }
        let was_detail = self.detail;
        let was_scale = self.scale;
        let budget = line_budget_at(self.scale.0);
        if segments > self.ceiling || self.lines > budget {
            self.last_pass_over = true;
            if self.scale.0 < self.max_scale.0 {
                // S4 (`docs/BIMARI5_LAG_PLAN.md`): one jump to the size this
                // frame needs, not a doubling per re-encode. The smallest
                // power of two covering the heavier of the two budgets, with
                // 10% headroom, clamped to `max_scale`.
                let seg_at_1 = f64::from(segment_ceiling_at(output_pixels, 1)).max(1.0);
                let lines_at_1 = line_budget_at(1) as f64;
                let need = (segments as f64 / seg_at_1).max(self.lines as f64 / lines_at_1) * 1.1;
                let target = (need.ceil().max(1.0) as u64)
                    .checked_next_power_of_two()
                    .unwrap_or(self.max_scale.0);
                // Always at least one doubling, so a frame over budget can
                // never be answered with an unchanged scale.
                let target = target.max(self.scale.0.saturating_mul(2));
                self.scale = BudgetScale(target.min(self.max_scale.0));
                self.clear_measured();
                self.ceiling = segment_ceiling_at(output_pixels, self.scale.0);
                // Growing is never a reason to shrink later in the same
                // frame's own bookkeeping — see V4.
                self.shrink_eligible_since = None;
                self.pending_shrink = None;
                return true;
            }
            self.detail = self.detail.down();
        } else if self.may_step_up(segments, budget) {
            self.detail = self.detail.up();
        }
        if self.detail == was_detail && self.scale == was_scale {
            return false;
        }
        // The pass just judged is being thrown away, so the shading it recorded
        // as owed is owed for shapes the replacement will not shade. See
        // `LightCache::discard_frame`.
        self.lights.discard_frame();
        true
    }

    /// **L7: may the ladder climb from the current level, given the pass just
    /// judged fits?** `docs/RIG_SCENE_LAG_PLAN.md` § L7.
    ///
    /// Never into a level this screen frame has already measured over the line
    /// budget at this scale — the fixed margins of [`LightDetail::room_to_step_up`] assume
    /// the next level adds a small multiple of this one, and a frame whose next
    /// level adds ten times (a rim glow) made the two rules alternate, ending
    /// on an over-budget pass. Once a ratio `lines(up) / lines(this)` has been
    /// measured for this level it replaces the fixed line margin: climb only
    /// when `lines * ratio * 1.1` still fits. The segment margin stays as it was.
    fn may_step_up(&self, segments: u32, budget: u64) -> bool {
        let up = self.detail.up();
        if up == self.detail {
            return false;
        }
        if self.measured_over(up, budget) {
            return false;
        }
        if segments >= self.detail.room_to_step_up(self.ceiling) {
            return false;
        }
        match self.step_ratio[self.detail.index()].max() {
            Some(ratio) => (self.lines as f64) * f64::from(ratio) * 1.1 < budget as f64,
            None => self.lines < u64::from(self.detail.room_to_step_up(budget as u32)),
        }
    }

    /// Did `detail`'s pass this screen frame, at this scale, exceed the line
    /// budget? `false` when it has not been measured. Lines only: the refusal
    /// must not fire for a caller that never calls [`Self::begin`] between
    /// frames and never calls [`Self::note_lines`] (segment-only judging).
    fn measured_over(&self, detail: LightDetail, budget: u64) -> bool {
        self.measured_lines[detail.index()].is_some_and(|l| l > budget)
    }

    /// Forget this frame's per-level measurements: they belong to one buffer
    /// scale. The cross-frame ratios stay.
    fn clear_measured(&mut self) {
        self.measured_lines = [None; 4];
        self.measured_segments = [None; 4];
    }

    /// Fold the pair(s) of levels that include `just` into [`Self::step_ratio`]
    /// when both ends have been measured this frame at this scale.
    fn update_ratios(&mut self, just: LightDetail) {
        for lower in [LightDetail::Unsealed, LightDetail::Flat, LightDetail::NoModelling] {
            let upper = lower.up();
            if upper == lower || (lower != just && upper != just) {
                continue;
            }
            if let (Some(lo), Some(hi)) = (
                self.measured_lines[lower.index()],
                self.measured_lines[upper.index()],
            ) && lo > 0
            {
                self.step_ratio[lower.index()].push(hi as f32 / lo as f32);
            }
        }
    }

    /// **L7: did the last pass [`Self::reconsider`] judged exceed the segment
    /// ceiling or the line budget?** Read after the encode loop: a caller that
    /// ends the loop on such a pass has submitted a frame that may not draw.
    pub fn over_budget_last_pass(&self) -> bool {
        self.last_pass_over
    }

    /// **L7: the level the last judged pass was encoded at.** Not
    /// [`Self::detail`], which a judgement that stepped the ladder has already
    /// moved to the level the next pass would use.
    pub fn last_pass_detail(&self) -> LightDetail {
        self.last_pass_detail
    }

    /// **L7: a level measured under budget this screen frame, at this scale, to
    /// fall back to when the last judged pass was over budget** — the cheapest
    /// such level (fewest lines). `None` when the last pass fit, when the level
    /// is pinned, or when no other level has been measured fitting. The seam
    /// seal ([`LightDetail::Unsealed`]) is never offered.
    pub fn fitting_fallback(&self) -> Option<LightDetail> {
        if self.pinned.is_some() || !self.last_pass_over {
            return None;
        }
        let budget = line_budget_at(self.scale.0);
        [
            LightDetail::Full,
            LightDetail::NoModelling,
            LightDetail::Flat,
        ]
        .into_iter()
        .filter(|d| *d != self.last_pass_detail)
        .filter_map(|d| {
            let lines = self.measured_lines[d.index()]?;
            let segments_fit = self.measured_segments[d.index()].is_none_or(|s| s <= self.ceiling);
            (lines <= budget && segments_fit).then_some((d, lines))
        })
        .min_by_key(|(_, lines)| *lines)
        .map(|(d, _)| d)
    }

    /// **L7: settle on `detail` for one more encode, without pinning it.** For
    /// the caller that ended its loop on an over-budget pass and re-encodes once
    /// at [`Self::fitting_fallback`]. The pass being replaced is thrown away, so
    /// the shading it recorded as owed is dropped, as [`Self::reconsider`] does.
    /// Follow the encode with [`Self::note_settled_pass`].
    pub fn settle_on(&mut self, detail: LightDetail) {
        self.detail = detail;
        self.lights.discard_frame();
    }

    /// **L7: record the pass encoded after [`Self::settle_on`]**, with no
    /// stepping — so [`Self::over_budget`], [`Self::last_lines`] and
    /// [`Self::last_encode`] describe the pass that was actually submitted.
    pub fn note_settled_pass(&mut self, segments: u32, lines: u64, output_pixels: f64) {
        self.note_lines(lines);
        self.encode = segments;
        self.ceiling = segment_ceiling_at(output_pixels, self.scale.0);
        self.measured_segments[self.detail.index()] = Some(segments);
        self.last_pass_detail = self.detail;
        self.last_pass_over = segments > self.ceiling || lines > line_budget_at(self.scale.0);
    }

    /// Idle window a comfortable frame must hold for before
    /// [`Self::note_shrink_headroom`] offers its buffer scale back. See V4 in
    /// `docs/VRAM_ADAPTIVE_BUFFERS_PLAN.md`.
    const SHRINK_IDLE: Duration = Duration::from_secs(20);

    /// **Offer to give buffer scale back, once a settled frame has
    /// comfortably fit at half [`Self::scale`] for [`Self::SHRINK_IDLE`]
    /// straight.** Call once per screen frame, after [`Self::reconsider`]'s
    /// own loop has settled (never mid-loop, and never while a gesture or
    /// playback is under way — the caller gates that, since
    /// [`Self::pending_shrink`] leads to a renderer rebuild that would cost
    /// a hitch mid-drag). `now` is passed in rather than read here so a test
    /// can move the clock without a real 20 s wait.
    ///
    /// "Comfortably" is the same `40%` of the *half*-scale budget
    /// [`Self::over_budget`]'s own bands-mode margin uses, so a frame that
    /// would only just clear the smaller buffers is not shrunk into
    /// immediately growing back.
    pub fn note_shrink_headroom(&mut self, output_pixels: f64, now: Instant) {
        if self.scale.0 <= 1 {
            self.shrink_eligible_since = None;
            self.pending_shrink = None;
            return;
        }
        let half = self.scale.0 / 2;
        let half_ceiling = f64::from(segment_ceiling_at(output_pixels, half));
        let half_budget = line_budget_at(half) as f64;
        let comfortable =
            f64::from(self.encode) < 0.4 * half_ceiling && (self.lines as f64) < 0.4 * half_budget;
        if !comfortable {
            self.shrink_eligible_since = None;
            self.pending_shrink = None;
            return;
        }
        let since = *self.shrink_eligible_since.get_or_insert(now);
        if now.duration_since(since) >= Self::SHRINK_IDLE {
            self.pending_shrink = Some(half as u32);
        }
    }

    /// The half scale [`Self::note_shrink_headroom`] is offering, if any —
    /// see V4. A caller that acts on it should rebuild its `GpuContext` at
    /// this scale (`GpuContext::shrink_to`) and then call
    /// [`Self::commit_shrink`] with the same value.
    pub fn pending_shrink(&self) -> Option<u32> {
        self.pending_shrink
    }

    /// Record that the caller's `GpuContext` actually shrank to `scale` —
    /// updates this cache's own budget to match and resets
    /// [`Self::shrink_eligible_since`]. See V4.
    pub fn commit_shrink(&mut self, scale: u32) {
        let new = BudgetScale(u64::from(scale.max(1)));
        if new != self.scale {
            self.clear_measured();
        }
        self.scale = new;
        self.shrink_eligible_since = None;
        self.pending_shrink = None;
    }

    /// Record this frame's own [`crate::SceneBuilder::estimated_lines`], so
    /// the next [`Self::reconsider`] judges lines as well as segments. Called
    /// by every `reconsider` caller just before it.
    pub fn note_lines(&mut self, lines: u64) {
        self.lines = lines;
        // L7: this is what `self.detail` costs, this frame, at this scale.
        self.measured_lines[self.detail.index()] = Some(lines);
        self.update_ratios(self.detail);
    }

    /// Adopt the `vello_encoding` buffer scale of the [`crate::GpuContext`]
    /// this cache serves — see [`Self::scale`]. `DrawCache::default()` /
    /// `DrawCache::new()` stay at scale `1` until a caller with a real GPU
    /// calls this, which is what keeps every test deterministic (P2 in
    /// `docs/IMPORT_PERF_PLAN.md`).
    pub fn set_scale(&mut self, scale: u32) {
        let new = BudgetScale(u64::from(scale.max(1)));
        if new != self.scale {
            self.clear_measured();
        }
        self.scale = new;
    }

    /// This cache's own current buffer scale — see [`Self::set_scale`]. A
    /// caller syncing a `GpuContext` to a scale [`Self::reconsider`] grew
    /// reads this after the ladder settles: `GpuContext::grow_to(cache.scale())`.
    pub fn scale(&self) -> u32 {
        self.scale.0 as u32
    }

    /// The ceiling this cache's own [`Self::scale`] may grow towards before
    /// [`Self::reconsider`] falls back to the detail ladder — see
    /// [`crate::GpuContext::max_buffer_scale`]. Defaults to `1`; a cache this
    /// is never called on never grows. See V3 in
    /// `docs/VRAM_ADAPTIVE_BUFFERS_PLAN.md`.
    pub fn set_max_scale(&mut self, max_scale: u32) {
        self.max_scale = BudgetScale(u64::from(max_scale.max(1)));
    }

    /// The ceiling [`Self::set_max_scale`] set — what [`Self::scale`] may grow
    /// towards. See S3 in `docs/BIMARI5_LAG_PLAN.md`.
    pub fn max_scale(&self) -> u32 {
        self.max_scale.0 as u32
    }

    /// [`segment_ceiling`], at this cache's own [`Self::scale`] rather than
    /// the thread that happens to be calling — the free function reads
    /// `vello_encoding`'s own thread-local, which only reflects a scale once
    /// some `GpuContext::render`/`render_measured` call has actually set it
    /// on this thread. A caller with a `DrawCache` but no frame to judge yet
    /// (planning stage bands before any of them are encoded) should call
    /// this instead. See V2 in `docs/VRAM_ADAPTIVE_BUFFERS_PLAN.md`.
    pub fn segment_ceiling(&self, output_pixels: f64) -> u32 {
        segment_ceiling_at(output_pixels, self.scale.0)
    }

    /// [`line_budget`], at this cache's own [`Self::scale`] — see
    /// [`Self::segment_ceiling`].
    pub fn line_budget(&self) -> u64 {
        line_budget_at(self.scale.0)
    }

    /// Was the last frame judged over either budget — the segment ceiling or
    /// [`line_budget`]? What [`Self::reconsider`] already found, read back
    /// rather than recomputed, so a caller can tell a frame that is over
    /// budget even at [`LightDetail::Flat`] — where stepping down no longer
    /// moves `detail` — from one that genuinely fits. See Phase 4d.
    pub fn over_budget(&self) -> bool {
        self.encode > self.ceiling || self.lines > line_budget_at(self.scale.0)
    }

    /// How many bands [`Self::over_budget`] artwork needs to draw in, each
    /// culled to its own strip so it costs a fraction of the whole frame's
    /// segments and lines. `1.3 ×` the worse of the two overrun ratios, so a
    /// frame at 200% of either budget is not split into exactly two bands
    /// that are each still over. See Phase 4d.
    pub fn bands_needed(&self) -> u32 {
        let segment_ratio = if self.ceiling > 0 {
            f64::from(self.encode) / f64::from(self.ceiling)
        } else {
            1.0
        };
        let line_ratio = self.lines as f64 / line_budget_at(self.scale.0) as f64;
        let bands = (1.3 * segment_ratio.max(line_ratio)).ceil();
        if bands.is_finite() {
            (bands as u32).clamp(2, 16)
        } else {
            16
        }
    }

    /// Would [`Self::bands_needed`]'s cap of 16 leave the worst band still
    /// over one of the budgets — the one case a band split cannot rescue a
    /// frame from? [`Self::bands_needed`] silently clamps its answer to the
    /// cap; this is the check a caller makes before trusting that answer
    /// alone, rather than assuming 16 bands is always enough. An even split
    /// is the same approximation `bands_needed` itself makes, without its
    /// `1.3×` margin — margin a caller already at the cap has none left to
    /// spend. See `RENDER_FOLLOWUPS_PLAN.md` item 3.
    pub fn bands_would_still_overrun(&self) -> bool {
        let segment_ratio = if self.ceiling > 0 {
            f64::from(self.encode) / f64::from(self.ceiling)
        } else {
            1.0
        };
        let line_ratio = self.lines as f64 / line_budget_at(self.scale.0) as f64;
        segment_ratio.max(line_ratio) / 16.0 > 1.0
    }
}

/// Resolved document-space bounds of instances, kept between frames so culling
/// an off-screen symbol does not re-walk its whole library subtree every frame.
///
/// Keyed by the object's copy-on-write pointer, exactly as the lighting cache is:
/// an instance's extent is a pure function of its transform and the symbol it
/// points at, both of which are captured by its `Arc` identity, so an unedited
/// instance keeps hitting. The bounds are in the object's own (parent) space, so
/// they do not depend on where the instance is nested and can be transformed to
/// document space at the point of use.
#[derive(Debug, Default)]
pub struct BoundsCache {
    entries: std::collections::HashMap<usize, BoundsEntry>,
    frame: u64,
    /// The library the entries were measured against, **held** (so its address
    /// cannot be handed to another library while it is the key): an instance's
    /// extent reads the symbol it points at, which an edit changes without
    /// touching the instance's own `Arc`. Any other library forgets everything.
    library: Option<buzz_scene::Library>,
    /// How many times an extent was measured (a miss), for tests.
    computed: u64,
}

#[derive(Debug)]
struct BoundsEntry {
    /// Held: while the cache has it nobody can edit the object in place
    /// (`Arc::make_mut` copies), so its address names these contents.
    _object: Arc<Object>,
    bounds: buzz_geom::Rect,
    used: u64,
    /// For a rig: [`buzz_scene::ArmatureData::pose_fingerprint`] when measured,
    /// checked again on a hit. Redundant for an object held here (its pose
    /// cannot change under its address), kept as the cheap guard the key is
    /// documented to have: a rig is a lookup while its pose holds still, and
    /// measured again the moment it does not.
    pose: Option<u64>,
}

impl BoundsCache {
    fn begin(&mut self) {
        self.frame = self.frame.wrapping_add(1);
    }

    fn end(&mut self) {
        let frame = self.frame;
        self.entries
            .retain(|_, entry| frame.saturating_sub(entry.used) < 3);
    }

    /// The resolved parent-space bounds of `owner`, computed once and reused.
    ///
    /// Keyed on the object's address (and, for a rig, its pose fingerprint) and
    /// the library, so a held rig — the shadow pass, the placement pass and the
    /// cull all ask — is a lookup after the first frame, and a moved one (a new
    /// object) or an edited library is measured again.
    fn resolved(&mut self, owner: &Arc<Object>, scene: &Scene) -> buzz_geom::Rect {
        let library = scene.library();
        if !self
            .library
            .as_ref()
            .is_some_and(|held| held.content_id() == library.content_id())
        {
            self.entries.clear();
            self.library = Some(library.clone());
        }
        let key = Arc::as_ptr(owner) as usize;
        let frame = self.frame;
        let pose = match &owner.kind {
            ObjectKind::Armature(rig) => Some(rig.pose_fingerprint()),
            _ => None,
        };
        if let Some(entry) = self.entries.get_mut(&key)
            && entry.pose == pose
        {
            entry.used = frame;
            return entry.bounds;
        }
        self.computed += 1;
        let bounds = scene.resolved_bounds(owner);
        self.entries.insert(
            key,
            BoundsEntry {
                _object: Arc::clone(owner),
                bounds,
                used: frame,
                pose,
            },
        );
        bounds
    }

    /// **Test reading:** how many extents were measured (misses) so far.
    #[doc(hidden)]
    pub fn computed(&self) -> u64 {
        self.computed
    }

    /// **Test reading:** [`Self::resolved`], reachable from an integration test.
    #[doc(hidden)]
    pub fn resolved_for_test(&mut self, owner: &Arc<Object>, scene: &Scene) -> buzz_geom::Rect {
        self.resolved(owner, scene)
    }
}

/// Is the layer-fx / silhouette memo on? Read once: `BUZZ_NO_LAYER_FX_MEMO=1`
/// turns it off for an A/B against the live path (every gather and filter build
/// done afresh, as before the memo existed). The result is identical either way;
/// only the time differs.
fn fx_memo_default() -> bool {
    static OFF: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    !*OFF.get_or_init(|| std::env::var("BUZZ_NO_LAYER_FX_MEMO").is_ok_and(|v| v.trim() == "1"))
}

/// **A layer's rim and filter ops, kept between frames.**
///
/// Building them costs a walk of every shape on the layer plus a Glow laid over
/// the thousands-of-subpaths silhouette that comes out of it; both are pure
/// functions of what is on the layer, so a layer that has not changed does not
/// pay again. Keyed on `(layer, signature)`: see [`layer_fx_signature`] for what
/// the signature covers and why it is exact.
///
/// Only the finished ops are kept, not the silhouette they were built from: a
/// silhouette is megabytes on a character layer and nothing reads it afterwards.
/// At most [`Self::PER_LAYER`] entries per layer, so a walking character — a new
/// pose, and so a new entry, every frame — holds the last couple of frames' ops
/// and not a history of them.
#[derive(Debug)]
pub struct LayerFxMemo {
    entries: std::collections::HashMap<(buzz_scene::LayerId, u64), FxEntry>,
    frame: u64,
    seq: u64,
    /// Whether the memo is consulted at all. See [`fx_memo_default`].
    enabled: bool,
    /// Answered from the memo / built afresh, for tests and the HUD.
    hits: u64,
    misses: u64,
}

#[derive(Debug)]
struct FxEntry {
    fx: Arc<buzz_fx::Painted>,
    used: u64,
    seq: u64,
    /// Held, not read: while an allocation has a `Weak` to it, its address
    /// cannot be handed to another object and its contents cannot be edited in
    /// place (`Arc::make_mut` copies instead). That is what makes an address in
    /// the signature a sound identity for as long as this entry lives.
    _identity: Vec<std::sync::Weak<Object>>,
    /// Held for the same reason, for the library identity in the signature.
    _library: buzz_scene::Library,
}

impl Default for LayerFxMemo {
    fn default() -> Self {
        Self {
            entries: std::collections::HashMap::new(),
            frame: 0,
            seq: 0,
            enabled: fx_memo_default(),
            hits: 0,
            misses: 0,
        }
    }
}

impl LayerFxMemo {
    /// Entries kept per layer. Two: the frame being drawn and the one before,
    /// which is what scrubbing back and forth and onion skins revisit.
    const PER_LAYER: usize = 2;
    /// Generations an entry survives unused, as the lighting cache ages its own.
    const KEEP_FRAMES: u64 = 2;

    fn begin(&mut self) {
        self.frame = self.frame.wrapping_add(1);
    }

    fn end(&mut self) {
        let frame = self.frame;
        self.entries
            .retain(|_, entry| frame.saturating_sub(entry.used) < Self::KEEP_FRAMES);
    }

    fn get(&mut self, layer: buzz_scene::LayerId, hash: u64) -> Option<Arc<buzz_fx::Painted>> {
        let frame = self.frame;
        match self.entries.get_mut(&(layer, hash)) {
            Some(entry) => {
                entry.used = frame;
                self.hits += 1;
                Some(Arc::clone(&entry.fx))
            }
            None => {
                self.misses += 1;
                None
            }
        }
    }

    fn insert(
        &mut self,
        layer: buzz_scene::LayerId,
        hash: u64,
        fx: Arc<buzz_fx::Painted>,
        identity: Vec<std::sync::Weak<Object>>,
        library: buzz_scene::Library,
    ) {
        // Make room: drop this layer's least recently used entries beyond the cap.
        loop {
            let mine: Vec<_> = self
                .entries
                .iter()
                .filter(|((l, _), _)| *l == layer)
                .map(|(key, entry)| (*key, entry.used, entry.seq))
                .collect();
            if mine.len() < Self::PER_LAYER {
                break;
            }
            if let Some((key, _, _)) = mine
                .into_iter()
                .min_by_key(|(_, used, seq)| (*used, *seq))
            {
                self.entries.remove(&key);
            }
        }
        self.seq += 1;
        self.entries.insert(
            (layer, hash),
            FxEntry {
                fx,
                used: self.frame,
                seq: self.seq,
                _identity: identity,
                _library: library,
            },
        );
    }
}

/// Is the shadow-fragment memo on? Read once: `BUZZ_NO_SHADOW_MEMO=1` turns it
/// off for an A/B against the live path (every layer's shadow pass encoded
/// straight into the frame, as before the memo existed). The picture is the same
/// either way; only the time differs.
fn shadow_memo_default() -> bool {
    static OFF: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    !*OFF.get_or_init(|| std::env::var("BUZZ_NO_SHADOW_MEMO").is_ok_and(|v| v.trim() == "1"))
}

/// **A layer's whole shadow pass, kept as an encoded fragment** (`docs/
/// RIG_SCENE_LAG_PLAN.md` L8').
///
/// The pass is the group the layer's shadows are drawn into: its bounds, the
/// opaque silhouette of every caster thrown by the light, and the tone put on
/// the group. On a lit character layer it was most of what a held frame
/// encoded — 3000 subpaths in one fill and a stroke per outline, rebuilt every
/// frame — and it is a pure function of what is on the layer, the light and the
/// view. The first time a layer is drawn the pass is encoded into a fragment
/// (through a builder with exactly the frame's own view, so the fragment holds
/// the very bytes the frame would have got) and appended; while the signature
/// (see [`shadow_memo_signature`]) holds still, every later frame appends the
/// fragment and does none of the work.
///
/// Aged and capped like [`LayerFxMemo`]: at most [`Self::PER_LAYER`] fragments
/// per layer, unused for [`Self::KEEP_FRAMES`] generations dropped.
pub struct ShadowMemo {
    entries: std::collections::HashMap<(buzz_scene::LayerId, u64), ShadowEntry>,
    frame: u64,
    seq: u64,
    /// Whether the memo is consulted at all. See [`shadow_memo_default`].
    enabled: bool,
    /// Appended from the memo / encoded afresh (and kept), for tests and the HUD.
    hits: u64,
    misses: u64,
}

struct ShadowEntry {
    fragment: Arc<crate::vello::Scene>,
    /// The fragment's own line count, charged to the frame on every append, as a
    /// stamp's is.
    lines: u64,
    used: u64,
    seq: u64,
    /// Held, not read: see [`FxEntry::_identity`].
    _identity: Vec<std::sync::Weak<Object>>,
    /// Held for the same reason, for the library identity in the signature.
    _library: buzz_scene::Library,
}

impl std::fmt::Debug for ShadowMemo {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ShadowMemo")
            .field("entries", &self.entries.len())
            .field("enabled", &self.enabled)
            .field("hits", &self.hits)
            .field("misses", &self.misses)
            .finish()
    }
}

impl Default for ShadowMemo {
    fn default() -> Self {
        Self {
            entries: std::collections::HashMap::new(),
            frame: 0,
            seq: 0,
            enabled: shadow_memo_default(),
            hits: 0,
            misses: 0,
        }
    }
}

impl ShadowMemo {
    /// Fragments kept per layer: the frame being drawn and the one before.
    const PER_LAYER: usize = 2;
    /// Generations a fragment survives unused.
    const KEEP_FRAMES: u64 = 2;

    fn begin(&mut self) {
        self.frame = self.frame.wrapping_add(1);
    }

    fn end(&mut self) {
        let frame = self.frame;
        self.entries
            .retain(|_, entry| frame.saturating_sub(entry.used) < Self::KEEP_FRAMES);
    }

    fn get(
        &mut self,
        layer: buzz_scene::LayerId,
        hash: u64,
    ) -> Option<(Arc<crate::vello::Scene>, u64)> {
        let frame = self.frame;
        match self.entries.get_mut(&(layer, hash)) {
            Some(entry) => {
                entry.used = frame;
                self.hits += 1;
                Some((Arc::clone(&entry.fragment), entry.lines))
            }
            None => {
                self.misses += 1;
                None
            }
        }
    }

    fn insert(
        &mut self,
        layer: buzz_scene::LayerId,
        hash: u64,
        fragment: Arc<crate::vello::Scene>,
        lines: u64,
        identity: Vec<std::sync::Weak<Object>>,
        library: buzz_scene::Library,
    ) {
        // Make room: drop this layer's least recently used entries beyond the cap.
        loop {
            let mine: Vec<_> = self
                .entries
                .iter()
                .filter(|((l, _), _)| *l == layer)
                .map(|(key, entry)| (*key, entry.used, entry.seq))
                .collect();
            if mine.len() < Self::PER_LAYER {
                break;
            }
            if let Some((key, _, _)) = mine
                .into_iter()
                .min_by_key(|(_, used, seq)| (*used, *seq))
            {
                self.entries.remove(&key);
            }
        }
        self.seq += 1;
        self.entries.insert(
            (layer, hash),
            ShadowEntry {
                fragment,
                lines,
                used: self.frame,
                seq: self.seq,
                _identity: identity,
                _library: library,
            },
        );
    }
}

/// One outline a rig part contributes to a silhouette, remembered.
///
/// `chain` is every transform between the part's own placement and the shape,
/// **in the order the live walk multiplies them**: `world * part.transform`,
/// then `* chain[0]`, `* chain[1]`, … — floating-point multiplication is not
/// associative, and replaying the products in the same order is what makes a
/// replayed outline bit-for-bit the one the live walk would have produced.
#[derive(Debug)]
struct SilLeaf {
    chain: Box<[Affine]>,
    path: buzz_geom::BezPath,
    /// `Some(width)`: the leaf is a stroke, outlined **after** it is placed
    /// (a stroke's width is in document units, so its outline is not the
    /// placed outline of an identity-space stroke). `None`: a fill.
    stroke: Option<f64>,
}

#[derive(Debug)]
struct PartEntry {
    /// See [`FxEntry::_identity`]: the address key below stays this artwork.
    _artwork: std::sync::Weak<Object>,
    /// The depth the walk was at when this was recorded: the depth guard
    /// inside the walk makes the outlines depth-dependent, so they are only
    /// replayed at the same depth.
    depth: usize,
    /// `None`: this part cannot be remembered (animated symbol inside).
    leaves: Option<Arc<[SilLeaf]>>,
    used: u64,
}

/// **A rig part's outlines, remembered, for the silhouette gather.**
///
/// A rigid part moves with one bone and is otherwise the same drawing, so its
/// outlines never change — only where they are placed. Walking it (through the
/// library for a symbol part, allocating layer lists and transforms at every
/// level) every time a pose moves is most of what a miss costs. Recorded once
/// per `part.artwork`, replayed per pose as `world * outline`.
///
/// Not remembered, and walked live exactly as before: skinned parts (their
/// shapes deform), parts with an animated symbol inside (they change with the
/// playhead), and any gather under a breath (a breath deforms the placed path).
#[derive(Debug)]
pub struct SilhouetteMemo {
    /// Off with the layer memo's kill switch: every gather is then the live walk.
    enabled: bool,
    parts: std::collections::HashMap<usize, PartEntry>,
    library: Option<buzz_scene::Library>,
    frame: u64,
    /// Test counters: shapes the live walk visited, outlines replayed from a
    /// part's memo, and parts recorded.
    live: u64,
    replayed: u64,
    recorded: u64,
}

impl Default for SilhouetteMemo {
    fn default() -> Self {
        Self {
            enabled: fx_memo_default(),
            parts: std::collections::HashMap::new(),
            library: None,
            frame: 0,
            live: 0,
            replayed: 0,
            recorded: 0,
        }
    }
}

impl SilhouetteMemo {
    const KEEP_FRAMES: u64 = 3;

    fn begin(&mut self) {
        self.frame = self.frame.wrapping_add(1);
    }

    fn end(&mut self) {
        let frame = self.frame;
        self.parts
            .retain(|_, entry| frame.saturating_sub(entry.used) < Self::KEEP_FRAMES);
    }

    /// The part's outlines, recorded now if they have not been, or `None` if
    /// this part has to be walked live.
    fn leaves_for(
        &mut self,
        scene: &Scene,
        artwork: &Arc<Object>,
        depth: usize,
    ) -> Option<Arc<[SilLeaf]>> {
        // Symbols are read through the library, so any edit to it forgets
        // everything recorded.
        let current = scene.library();
        if !self
            .library
            .as_ref()
            .is_some_and(|held| held.content_id() == current.content_id())
        {
            self.parts.clear();
            self.library = Some(current.clone());
        }
        let key = Arc::as_ptr(artwork) as usize;
        let frame = self.frame;
        if let Some(entry) = self.parts.get_mut(&key) {
            entry.used = frame;
            return if entry.depth == depth {
                entry.leaves.clone()
            } else {
                None
            };
        }
        let mut out = Vec::new();
        let ok = record_leaves(scene, artwork, &mut Vec::new(), depth, &mut out);
        self.recorded += 1;
        let leaves: Option<Arc<[SilLeaf]>> = ok.then(|| out.into());
        self.parts.insert(
            key,
            PartEntry {
                _artwork: Arc::downgrade(artwork),
                depth,
                leaves: leaves.clone(),
                used: frame,
            },
        );
        leaves
    }
}

/// Record what [`append_silhouette`] would push for `object`'s **contents** —
/// not for `object`'s own transform, which the caller replaces with the posed
/// one. Mirrors the live walk branch for branch; returns `false` if anything
/// inside depends on the playhead (an instance of a symbol with more than one
/// frame), in which case nothing may be remembered.
fn record_leaves(
    scene: &Scene,
    object: &Object,
    chain: &mut Vec<Affine>,
    depth: usize,
    out: &mut Vec<SilLeaf>,
) -> bool {
    if !object.visible || depth >= MAX_SYMBOL_DEPTH {
        return true;
    }
    let leaf = |chain: &[Affine], path: buzz_geom::BezPath, stroke: Option<f64>| SilLeaf {
        chain: chain.into(),
        path,
        stroke,
    };
    match &object.kind {
        ObjectKind::Shape(shape) => {
            if shape.fill.is_some() {
                out.push(leaf(chain, shape.path.clone(), None));
            }
            if let Some(stroke) = &shape.stroke
                && stroke.width > 0.0
            {
                out.push(leaf(chain, shape.path.clone(), Some(stroke.width)));
            }
            true
        }
        ObjectKind::Warp(warp) => {
            out.push(leaf(chain, warp.warped().path, None));
            true
        }
        ObjectKind::Group(children) => {
            for child in children {
                chain.push(child.transform);
                let ok = record_leaves(scene, child, chain, depth + 1, out);
                chain.pop();
                if !ok {
                    return false;
                }
            }
            true
        }
        ObjectKind::Armature(rig) => {
            for part in rig.posed() {
                chain.push(part.transform);
                let ok = record_leaves(scene, &part, chain, depth + 1, out);
                chain.pop();
                if !ok {
                    return false;
                }
            }
            true
        }
        ObjectKind::Instance(instance) => {
            let Some(symbol) = scene.library().get(instance.symbol) else {
                return true;
            };
            // The only thing a playhead changes is which frame of a symbol an
            // instance shows, and a one-frame symbol has only one.
            if symbol.length() > 1 {
                return false;
            }
            let frame = instance.resolve_frame(symbol.kind, 0, symbol.length());
            for layer in symbol.layers.drawable_at(frame) {
                if !layer.visible || layer.kind.is_mask() {
                    continue;
                }
                let follows = symbol.layers.inherited_transform(layer.id, frame);
                for child in symbol.layers.resolved_at_rigged(layer.id, frame).iter() {
                    chain.push(follows);
                    chain.push(child.transform);
                    let ok = record_leaves(scene, child, chain, depth + 1, out);
                    chain.pop();
                    chain.pop();
                    if !ok {
                        return false;
                    }
                }
            }
            true
        }
    }
}

/// **What a layer's rim and filter ops depend on**, hashed — or `None` if
/// something on the layer cannot be identified and the ops must be built and
/// thrown away.
///
/// Per object, in order: *which* object (the stored `Arc`'s address — copy-on-write
/// gives an edited object a new one — or, for a rig that has no stored object
/// behind it, a fingerprint of its pose and the addresses of its artwork), the
/// modifier evaluation that changed how it is drawn (the `prepend` transform and
/// the breath; a re-posed copy is a rig and is fingerprinted like one), and — only
/// if something animated is inside it — the frame it is playing. Then the
/// library's identity, the rim's reach and colour, and the layer's filters. That
/// is every input the gather and the build read, so a hit gives the same ops.
///
/// The `Weak`s returned pin every address the hash read (see
/// [`FxEntry::_identity`]).
#[allow(clippy::too_many_arguments)]
fn layer_fx_signature(
    scene: &Scene,
    layer: &buzz_scene::Layer,
    resolved: &buzz_scene::ResolvedFrame<'_>,
    evals: &[Option<buzz_scene::modifier::ModifierEval>],
    elapsed: f64,
    stage_frame: u32,
    rim: Option<&buzz_light::RimGlow>,
    animated: &mut AnimatedMemo,
) -> Option<(u64, Vec<std::sync::Weak<Object>>)> {
    use std::hash::{Hash, Hasher};

    let mut h = std::collections::hash_map::DefaultHasher::new();
    let mut identity: Vec<std::sync::Weak<Object>> = Vec::new();
    layer.id.hash(&mut h);
    scene.library().content_id().hash(&mut h);
    match rim {
        Some(rim) => {
            1u8.hash(&mut h);
            rim.reach.to_bits().hash(&mut h);
            format!("{:?}", rim.color).hash(&mut h);
        }
        None => 0u8.hash(&mut h),
    }
    format!("{:?}", layer.filters).hash(&mut h);

    for ((object, owner), eval) in resolved.iter_owned().zip(evals) {
        // The object as it is actually drawn, and whether the stored `Arc` is
        // the object at all (a re-posed copy replaces it).
        let (base, stored) = match eval {
            Some(eval) => match &eval.object {
                Some(held) => (held, None),
                None => (object, owner),
            },
            None => (object, owner),
        };
        match stored {
            Some(arc) => {
                (Arc::as_ptr(arc) as usize).hash(&mut h);
                identity.push(Arc::downgrade(arc));
            }
            None => match &base.kind {
                // A rig with no stored object behind it — tweened, or posed
                // by a modifier — is a fresh object every frame, but its
                // pose and the artwork it is made of say everything it draws.
                ObjectKind::Armature(rig) => {
                    2u8.hash(&mut h);
                    rig.pose_fingerprint().hash(&mut h);
                    base.visible.hash(&mut h);
                    for coefficient in base.transform.as_coeffs() {
                        coefficient.to_bits().hash(&mut h);
                    }
                    for part in &rig.parts {
                        identity.push(Arc::downgrade(&part.artwork));
                    }
                }
                // Anything else fresh has no identity to key on.
                _ => return None,
            },
        }
        if let Some(eval) = eval {
            for coefficient in eval.prepend.as_coeffs() {
                coefficient.to_bits().hash(&mut h);
            }
            match &eval.breath {
                Some(breath) => {
                    for number in [
                        breath.hips,
                        breath.shoulders,
                        breath.centre,
                        breath.rise,
                        breath.widen,
                    ] {
                        number.to_bits().hash(&mut h);
                    }
                }
                None => 0u64.hash(&mut h),
            }
        }
        if animated.object(scene, base, 0) {
            elapsed.to_bits().hash(&mut h);
            stage_frame.hash(&mut h);
        }
    }
    Some((h.finish(), identity))
}

/// **Does anything animated live inside this artwork?** An instance of a symbol
/// with more than one frame, at any depth, is the only thing whose drawing — and
/// so whose silhouette — can change with the playhead while the object, its
/// address and its placement sit still. An instance of a one-frame symbol
/// resolves to that frame whatever the playhead says.
///
/// Per symbol, memoised against the library's identity (held, so its address
/// cannot be handed to another library while the memo refers to it — the same
/// trick as [`SymbolTable`]). A symbol that cannot be resolved, is nested too
/// deep, or refers back to itself answers `true`: the conservative answer for a
/// cache key is "it may change".
#[derive(Debug, Default)]
pub struct AnimatedMemo {
    library: Option<buzz_scene::Library>,
    symbols: std::collections::HashMap<buzz_scene::SymbolId, bool>,
}

impl AnimatedMemo {
    /// Is there anything animated in `object`, looking through groups, rigs and
    /// instances?
    fn object(&mut self, scene: &Scene, object: &Object, depth: usize) -> bool {
        if depth >= MAX_SYMBOL_DEPTH {
            return true;
        }
        match &object.kind {
            ObjectKind::Shape(_) | ObjectKind::Warp(_) => false,
            ObjectKind::Group(children) => children
                .iter()
                .any(|child| self.object(scene, child, depth + 1)),
            ObjectKind::Armature(rig) => rig
                .parts
                .iter()
                .any(|part| self.object(scene, &part.artwork, depth + 1)),
            ObjectKind::Instance(instance) => {
                self.sync(scene);
                self.symbol(scene, instance.symbol, depth + 1)
            }
        }
    }

    /// Forget everything if the library is not the one the answers came from.
    fn sync(&mut self, scene: &Scene) {
        let current = scene.library();
        if self
            .library
            .as_ref()
            .is_some_and(|held| held.content_id() == current.content_id())
        {
            return;
        }
        self.symbols.clear();
        self.library = Some(current.clone());
    }

    fn symbol(&mut self, scene: &Scene, id: buzz_scene::SymbolId, depth: usize) -> bool {
        if let Some(known) = self.symbols.get(&id) {
            return *known;
        }
        // A symbol that is not there draws nothing, so nothing animates; the
        // library's identity moves if it is ever added.
        let Some(symbol) = scene.library().get(id) else {
            return false;
        };
        if depth >= MAX_SYMBOL_DEPTH {
            return true;
        }
        // Provisional, for a symbol that nests itself: answers `true` and
        // terminates. Overwritten below.
        self.symbols.insert(id, true);
        let animated = symbol.length() > 1
            || symbol.layers.iter().any(|layer| {
                layer
                    .all_objects()
                    .any(|object| self.object(scene, object, depth))
            });
        self.symbols.insert(id, animated);
        animated
    }
}

/// What the draw walk needs to know about a symbol that does not change between
/// frames: how to key a cached encoding of it, how big it is, and whether it is
/// simple enough to cache at all.
///
/// Computed once per document edit (see [`SymbolTable`]) and read on every
/// instance of the symbol, so the per-frame cost of deciding "can I reuse this
/// symbol's encoding?" is a hash-map lookup rather than a walk of its subtree.
#[derive(Debug, Clone)]
pub struct SymbolInfo {
    /// Held, not read: keeping a reference to each symbol means the next edit
    /// finds its `Arc` shared and `Arc::make_mut` forks it, giving it a new
    /// address — which is what moves the fingerprint below. This is the same
    /// pointer-identity convention the other caches rely on (see [`BoundsCache`],
    /// which holds an `Arc<Object>` for the same reason). Without it, an in-place
    /// edit of an unshared library would leave the fingerprint stale.
    #[allow(dead_code)]
    arc: Arc<buzz_scene::Symbol>,
    /// Changes whenever the symbol's own contents change **or** any symbol it
    /// nests changes. Editing a nested symbol forks that symbol's `Arc` but not
    /// its parent's, so a fingerprint that folds in the children's is what makes
    /// a nested edit invalidate the parent's cached encoding.
    fingerprint: u64,
    /// The symbol's real extent, resolved through the library across every frame
    /// (`Scene::symbol_bounds`). Placeholder instance bounds would be far too
    /// small — this is why the cache clips and anchors from here, not from
    /// `Symbol::bounds`.
    /// Read by the scene cache to anchor and clip the child encode.
    resolved_bounds: buzz_geom::Rect,
    flags: SymFlags,
}

impl SymbolInfo {
    /// Simple enough to cache: nothing inside it renders in a way that depends on
    /// where the instance sits (no filters, group blends, out-of-plane objects
    /// or inverse masks, transitively).
    fn cacheable_content(&self) -> bool {
        !(self.flags.filters
            || self.flags.group_blend
            || self.flags.non_flat
            || self.flags.inverse_mask)
    }
}

/// Content facts about a symbol's whole subtree, OR-combined across every object
/// in every frame and through every nested symbol.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct SymFlags {
    /// Any object carries filters — drawn from a document-space silhouette whose
    /// blur radius does not survive being stamped at another position.
    filters: bool,
    /// Any object uses a blend that needs an isolation group, which the group's
    /// bounds — position-dependent — are pushed for.
    group_blend: bool,
    /// Any object faces out of its plane, so it is drawn through the stage
    /// camera's projection and moves with its placement.
    non_flat: bool,
    /// Any layer is an inverse mask, whose isolation the live path bounds in
    /// symbol-local space — a placement-dependent quirk the cache must not "fix".
    inverse_mask: bool,
    /// Any shape paints additively, so a layer holding this symbol must be
    /// isolated. Not a cache-eligibility fact — this one memoises
    /// [`has_additive_paint`].
    additive: bool,
    /// Any shape is filled with a bitmap. Not a cache-eligibility fact for an
    /// unlit stamp; it is for a **lit** one: a picture takes the light by
    /// compositing (`draw_lit_composited`), inside an isolation group whose
    /// bounds are the picture's own box *in the space it is encoded in* — for a
    /// stamp that is the symbol's local space, so a rotated placement clips the
    /// group to a rotated rectangle that coincides with the picture's own edge
    /// (edge pixels at coverage squared; measured 362 pixels / 29 levels off
    /// live), where the live walk's group is the larger world-aligned box. Such
    /// a symbol draws live under a light.
    images: bool,
}

impl SymFlags {
    /// Everything true — the conservative answer for a dangling reference or a
    /// symbol caught in a cycle, which keeps it out of the cache and isolated.
    const ALL: SymFlags = SymFlags {
        filters: true,
        group_blend: true,
        non_flat: true,
        inverse_mask: true,
        additive: true,
        images: true,
    };

    fn or(self, other: SymFlags) -> SymFlags {
        SymFlags {
            filters: self.filters || other.filters,
            group_blend: self.group_blend || other.group_blend,
            non_flat: self.non_flat || other.non_flat,
            inverse_mask: self.inverse_mask || other.inverse_mask,
            additive: self.additive || other.additive,
            images: self.images || other.images,
        }
    }
}

/// A per-symbol memo of [`SymbolInfo`], rebuilt only when the document changes.
///
/// The interactive view camera is not part of the scene, so panning, zooming and
/// playback never bump [`Scene::revision`] — the table is built once on import
/// and reused for every frame the user then works through. It is rebuilt in full
/// on any edit, which is a single depth-first pass over the library.
#[derive(Debug, Default, Clone)]
pub struct SymbolTable {
    /// The library this table was built from, **held**.
    ///
    /// Everything in here is derived from the library and nothing else — see
    /// [`build_symbol`], which reads `scene.library()` and no other part of
    /// the scene. So the library's own identity is the whole validity key.
    ///
    /// It used to be keyed on that identity *and* the scene revision, which
    /// made every edit to anything rebuild it: dragging one object across the
    /// stage bumps the revision on every pointer move, and each one threw away
    /// a table that was still perfectly good and re-walked the entire library —
    /// depth-first, fingerprinting every symbol and every nested child. On an
    /// imported character that is the most expensive thing in the frame, and it
    /// was being paid per mouse move.
    ///
    /// The revision was there to guard against a *reused address*: a document
    /// closed and another opened whose library lands on the same allocation.
    /// Holding the `Library` closes that off properly — the allocation cannot
    /// be freed, so its address cannot be handed to anything else, while this
    /// table still refers to it. It is the same trick the thumbnail cache plays
    /// on symbols, and it is sound for the same reason. Cheap to hold: a
    /// `Library` is two `Arc`s, and an edit to it copies on write, which is
    /// exactly the change this needs to notice.
    library: Option<buzz_scene::Library>,
    infos: std::collections::HashMap<buzz_scene::SymbolId, SymbolInfo>,
}

impl SymbolTable {
    /// Bring the table up to date with the scene, cheaply if nothing changed.
    pub fn refresh(&mut self, scene: &Scene) {
        let current = scene.library();
        if self
            .library
            .as_ref()
            .is_some_and(|held| held.content_id() == current.content_id())
        {
            return;
        }
        self.infos.clear();
        let mut visiting = std::collections::HashSet::new();
        for symbol in scene.library().iter() {
            build_symbol(symbol.id, scene, &mut self.infos, &mut visiting);
        }
        self.library = Some(current.clone());
    }

    fn get(&self, id: buzz_scene::SymbolId) -> Option<&SymbolInfo> {
        self.infos.get(&id)
    }
}

/// Compute (and memoise) one symbol's [`SymbolInfo`], returning its fingerprint.
///
/// Depth-first through nested symbols. `visiting` breaks cycles: a back-edge to a
/// symbol still being built contributes only its pointer to the fingerprint and
/// the conservative [`SymFlags::ALL`] to its flags, so a self-referencing symbol
/// terminates and stays out of the cache.
fn build_symbol(
    id: buzz_scene::SymbolId,
    scene: &Scene,
    infos: &mut std::collections::HashMap<buzz_scene::SymbolId, SymbolInfo>,
    visiting: &mut std::collections::HashSet<buzz_scene::SymbolId>,
) -> u64 {
    use std::hash::{Hash, Hasher};

    if let Some(info) = infos.get(&id) {
        return info.fingerprint;
    }
    let Some(symbol) = scene.library().get(id) else {
        return 0;
    };
    if !visiting.insert(id) {
        // Cycle: contribute the pointer and unwind. The still-building ancestor
        // will fold in SymFlags::ALL for us below.
        return Arc::as_ptr(symbol) as u64;
    }

    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    (Arc::as_ptr(symbol) as usize).hash(&mut hasher);

    let mut flags = SymFlags::default();
    // Bounds are unioned here in the *same* memoised pass — a nested symbol's
    // extent is read from its already-computed entry, never re-measured. Doing
    // it any other way (a `Scene::symbol_bounds` call per symbol) re-walks each
    // subtree from scratch and is exponential in the nesting depth, which froze
    // the first frame after importing a rig-heavy document.
    let mut bounds: Option<buzz_geom::Rect> = None;
    for layer in symbol.layers.iter() {
        if layer.kind == LayerKind::InverseMask {
            flags.inverse_mask = true;
        }
        for object in layer.all_objects() {
            let (f, b) = scan_object(object, scene, infos, visiting, &mut hasher);
            flags = flags.or(f);
            if b != buzz_geom::Rect::ZERO {
                bounds = Some(bounds.map_or(b, |acc| acc.union(b)));
            }
        }
    }

    visiting.remove(&id);
    let fingerprint = hasher.finish();
    infos.insert(
        id,
        SymbolInfo {
            arc: Arc::clone(symbol),
            fingerprint,
            resolved_bounds: bounds.unwrap_or(buzz_geom::Rect::ZERO),
            flags,
        },
    );
    fingerprint
}

/// OR the content flags of one object's subtree and return its resolved bounds
/// in the parent's space, folding nested symbols' whole (already-computed) flags
/// and extents in and hashing their fingerprints into `hasher`.
///
/// The bounds mirror `Scene::resolved_bounds_with` exactly, but read a nested
/// instance's extent from the memo rather than re-measuring the library.
fn scan_object(
    object: &Object,
    scene: &Scene,
    infos: &mut std::collections::HashMap<buzz_scene::SymbolId, SymbolInfo>,
    visiting: &mut std::collections::HashSet<buzz_scene::SymbolId>,
    hasher: &mut std::collections::hash_map::DefaultHasher,
) -> (SymFlags, buzz_geom::Rect) {
    use buzz_scene::object::transform_rect;
    use std::hash::Hash;

    // Filters, blend and facing live on every object, not just shapes.
    let mut flags = SymFlags {
        filters: !object.filters.is_empty(),
        group_blend: object.blend.needs_group(),
        non_flat: !object.spatial.is_flat(),
        ..SymFlags::default()
    };

    let bounds = match &object.kind {
        ObjectKind::Shape(shape) => {
            flags.additive |= shape.blend.is_additive();
            flags.images |= shape.fill.as_ref().is_some_and(|f| f.paint.is_image());
            object.bounds()
        }
        // A posed rig, like a warp, measures itself — its bounds do not come
        // from unioning its parts (which are the undeformed source).
        ObjectKind::Warp(warp) => {
            flags.additive |= warp.shape.blend.is_additive();
            flags.images |= warp
                .shape
                .fill
                .as_ref()
                .is_some_and(|f| f.paint.is_image());
            object.bounds()
        }
        ObjectKind::Armature(rig) => {
            for part in &rig.parts {
                let (f, _) = scan_object(&part.artwork, scene, infos, visiting, hasher);
                flags = flags.or(f);
            }
            object.bounds()
        }
        ObjectKind::Group(children) => {
            let mut group: Option<buzz_geom::Rect> = None;
            for child in children {
                let (f, cb) = scan_object(child, scene, infos, visiting, hasher);
                flags = flags.or(f);
                let tb = transform_rect(object.transform, cb);
                group = Some(group.map_or(tb, |acc| acc.union(tb)));
            }
            group.unwrap_or_else(|| object.bounds())
        }
        ObjectKind::Instance(instance) => {
            let child_fp = build_symbol(instance.symbol, scene, infos, visiting);
            child_fp.hash(hasher);
            match infos.get(&instance.symbol) {
                Some(child) => {
                    flags = flags.or(child.flags);
                    if child.resolved_bounds == buzz_geom::Rect::ZERO {
                        // An empty (or cyclic) symbol has no measurable extent;
                        // fall back to the placeholder, as `instance_bounds`
                        // does when the library measure comes back empty.
                        object.bounds()
                    } else {
                        transform_rect(object.transform, child.resolved_bounds)
                    }
                }
                // Dangling reference or cycle back-edge: assume the worst, and
                // measure with the placeholder.
                None => {
                    flags = flags.or(SymFlags::ALL);
                    object.bounds()
                }
            }
        }
    };
    (flags, bounds)
}

/// A cache of whole symbols encoded once and stamped per instance.
///
/// The lag on an imported Animate document is that a symbol placed hundreds of
/// times is walked and encoded hundreds of times a frame. This encodes each
/// eligible `(symbol, inner-frame, depth, zoom)` once into its own Vello scene
/// and then [appends](crate::vello::Scene::append) it — an O(N) memcpy of the
/// encoding with a transform folded into every child transform — at each
/// instance, turning N encodes into one build and N stamps.
///
/// Entries age out like the other caches (a few generations) and are capped, so
/// a zoom gesture — which changes the key's `scale_bits` every frame — cannot
/// pile up generations of full-document encodings.
pub struct SymbolSceneCache {
    entries: std::collections::HashMap<SymKey, SymEntry>,
    /// On by default; the window turns it off when `BUZZ_NO_SYMBOL_CACHE` is
    /// set, and tests toggle it to compare. Off, every instance draws live
    /// exactly as before.
    enabled: bool,
    /// **S2 non-similarity stamps, on by default** (`BUZZ_STAMP_NON_SIMILAR=0`
    /// opts out; always on under `cfg(test)`). Was off after the Opus review
    /// 2026-09-28 (`docs/BIMARI5_LAG_PLAN.md`); the S13 exactness fixes made the
    /// path match live pixels, see the S13 rows there.
    non_similar: bool,
    /// Generation, bumped per window frame like the lighting cache.
    frame: u64,
    /// How many child scenes were encoded this session, for tests and the HUD.
    pub builds: u64,
    /// How many times a cached scene was stamped, likewise.
    pub stamps: u64,
    /// Of [`Self::stamps`], how many were **lit** stamps (encoded with the
    /// light applied, `RIG_SCENE_LAG_PLAN` L3'').
    pub stamps_lit: u64,

    // Permanent refusal counters for `try_stamp_symbol` — one per `return
    // false` reason, in the order they are tested there. Diagnosis for
    // `docs/BIMARI5_LAG_PLAN.md` S2: which gate actually turns a live-drawn
    // instance's stamp away, on a real document. Read via
    // [`Self::refusal_counts`]; never reset except by a fresh cache.
    /// `!self.enabled` (`BUZZ_NO_SYMBOL_CACHE`, or a test comparing against
    /// the live path).
    pub refused_disabled: u64,
    /// The symbol has no measured [`SymInfo`] yet (dangling id, or the
    /// content-flags memo has not run for it).
    pub refused_no_info: u64,
    /// `SymInfo::cacheable_content` is false because a filter reaches this
    /// subtree.
    pub refused_filters: u64,
    /// `cacheable_content` is false because a group blend reaches this
    /// subtree.
    pub refused_group_blend: u64,
    /// `cacheable_content` is false because an out-of-plane (non-flat)
    /// object reaches this subtree.
    pub refused_non_flat: u64,
    /// `cacheable_content` is false because an inverse mask reaches this
    /// subtree.
    pub refused_inverse_mask: u64,
    /// The instance's own context is not neutral: a tint, fade, ghost alpha,
    /// Adjust Color, gradient map, blur, lighting or breath is in force, so a
    /// shared encoding would be wrong here.
    pub refused_neutral_ctx: u64,
    /// A **lit, non-similar** placement while a figure's silhouette is being
    /// gathered: its stamp has the projection baked into its local space, so
    /// its paths cannot be handed to the figure pass in document space
    /// exactly; the live walk gives them.
    pub refused_lit_silhouette: u64,
    /// The projection has no affine form (a tilted camera on this object).
    pub refused_no_affine_projection: u64,
    /// The placement's linear part has (numerically) no area, or its
    /// quantised scale came out non-finite or non-positive: no scale can be
    /// factored out of it at all.
    pub refused_degenerate: u64,
    /// The placement is not a similarity **and** the exact-linear-part path
    /// (S2) still could not take it — kept for diagnosis; expected to stay
    /// at 0 once S2 lands, since every non-similarity that reaches this
    /// point is otherwise eligible.
    pub refused_non_similar: u64,
    /// `builder.split.scale` (the stage's own render scale) is non-finite or
    /// non-positive.
    pub refused_bad_scale: u64,
    /// The child's encoded geometry would land outside the well-conditioned
    /// `f32` range (`child_scale × diagonal`, or the non-similar path's own
    /// safe bound on it, `> 1e5`).
    pub refused_too_big: u64,
    /// The stamp's translation would land outside the well-conditioned
    /// range (`> 1e7` render-space units from the camera anchor).
    pub refused_too_far: u64,
    /// Less than half the instance's projected bounds lies inside the
    /// current cull/stamp view.
    pub refused_under_half_visible: u64,
    /// Keys judged "mostly off-screen" (the `under_half_visible` gate),
    /// however that judgement came out, with the generation they were last
    /// judged in — the "static across frames" half of the S2 allowance: a
    /// **miss** that was *also* judged mostly off-screen last generation (not
    /// just this one) is static enough — the same camera, the same
    /// placement, asked again — to build now rather than redraw live every
    /// generation forever, exactly as an on-screen miss already does. A miss
    /// seen only *once* still refuses, so a placement passed through on a
    /// single scrub or a single band never pays to build something that is
    /// never asked for again. Ages out with the rest of the cache in `end`.
    seen_off_screen: std::collections::HashMap<SymKey, u64>,
    /// **First sightings of keys that missed** (S2-rest (b) in
    /// `docs/BIMARI5_LAG_PLAN.md`), with the generation they were last asked
    /// for in. Building a stamp costs more than drawing live once (the child
    /// is encoded into its own scene and then appended), so a miss for a key
    /// never sighted before is drawn live and only recorded here; the key
    /// builds on its **second** sighting (a later generation, or a second
    /// request within the same generation — a symbol used many times on one
    /// frame builds on its second instance). Retained far longer than
    /// [`SYM_KEEP_FRAMES`] ([`SYM_SIGHT_KEEP_FRAMES`]) so a scrub that
    /// revisits a frame is still recognised, and count-capped
    /// ([`SYM_SIGHT_CAP`]).
    sighted: std::collections::HashMap<SymKey, u64>,
    /// Memory budget for the encoded scenes held in `entries`, in bytes.
    /// `BUZZ_STAMP_CACHE_MB` overrides the [`SYM_CACHE_DEFAULT_MIB`] default.
    /// Least-recently-used entries are evicted in `end` while the total is
    /// over it.
    pub budget_bytes: usize,
    /// Approximate bytes held by `entries` right now (see [`scene_bytes`]).
    pub bytes: usize,
    /// Entries evicted for memory or count since the cache was created.
    pub evictions: u64,
    /// Misses drawn live because their key had not been sighted before.
    pub first_sightings: u64,
    /// Monotonic use clock — the LRU order (generations are too coarse: one
    /// generation can touch thousands of entries).
    clock: u64,
    /// Off by default (the 2nd-sighting rule above). A warm-up sweep that
    /// pre-builds every stamp at document open turns it on so a miss builds
    /// at once. See [`DrawCache::set_build_on_first_sighting`].
    build_on_first_sighting: bool,
    /// On while one frame is drawn as several bands (see
    /// [`DrawCache::set_band_mode`]): a sighting recorded in **this**
    /// generation does not count as an earlier one, so the bands of one frame
    /// together count as a single sighting.
    band_mode: bool,
    /// **E6: keys installed by [`DrawCache::install_stamps`] and not yet used
    /// by a frame of this cache.** Built ahead on a worker, not asked for yet,
    /// so [`Self::end`] evicts them before anything a frame has used. A hit
    /// clears the mark. Empty on every cache that never installs.
    transient: std::collections::HashSet<SymKey>,
    /// **E6:** keys built since the last [`DrawCache::drain_new_stamps`],
    /// recorded only while `collect_new` is on.
    fresh: Vec<SymKey>,
    /// Record builds in `fresh` (a pre-build worker's cache turns it on).
    collect_new: bool,
    /// Stamps installed from a worker since the cache was created.
    pub installed: u64,
    /// **S7 (stop latency):** when on, [`Self::end`] keeps the encodings it evicts
    /// in `graveyard` instead of freeing them itself (a pass that evicts hundreds
    /// of multi-megabyte stamps spent up to 48 ms in `free`); the owner takes
    /// them with [`DrawCache::take_graveyard`] and frees them elsewhere.
    defer_frees: bool,
    graveyard: Vec<Arc<crate::vello::Scene>>,
    /// Ceiling on live entries; [`SYM_CACHE_CAP`] unless a warm-up at document
    /// open raised it ([`DrawCache::set_stamp_limits`], S9): a whole film holds
    /// tens of thousands of distinct stamps and the byte budget alone must
    /// then decide what stays.
    entry_cap: usize,
    /// **S9 window mode** (set by a warm-up, see [`DrawCache::set_stamp_window`]):
    /// the cache holds a warmed film that may be larger than its budget, so it
    /// slides along the playback instead of thrashing: [`Self::end`] evicts down to
    /// 90% of the budget (one sort per ~10%, not one per frame) and
    /// [`DrawCache::make_room_for_stamps`] lets the pre-build keep running.
    window: bool,
}

impl Default for SymbolSceneCache {
    fn default() -> Self {
        Self {
            entries: std::collections::HashMap::new(),
            enabled: true,
            non_similar: cfg!(test)
                || std::env::var("BUZZ_STAMP_NON_SIMILAR").map_or(true, |v| v != "0"),
            frame: 0,
            builds: 0,
            stamps: 0,
            stamps_lit: 0,
            seen_off_screen: std::collections::HashMap::new(),
            sighted: std::collections::HashMap::new(),
            budget_bytes: std::env::var("BUZZ_STAMP_CACHE_MB")
                .ok()
                .and_then(|v| v.trim().parse::<usize>().ok())
                .unwrap_or(SYM_CACHE_DEFAULT_MIB)
                .saturating_mul(1 << 20),
            bytes: 0,
            evictions: 0,
            first_sightings: 0,
            clock: 0,
            build_on_first_sighting: false,
            band_mode: false,
            transient: std::collections::HashSet::new(),
            fresh: Vec::new(),
            collect_new: false,
            installed: 0,
            defer_frees: false,
            graveyard: Vec::new(),
            entry_cap: SYM_CACHE_CAP,
            window: false,
            refused_disabled: 0,
            refused_no_info: 0,
            refused_filters: 0,
            refused_group_blend: 0,
            refused_non_flat: 0,
            refused_inverse_mask: 0,
            refused_neutral_ctx: 0,
            refused_lit_silhouette: 0,
            refused_no_affine_projection: 0,
            refused_degenerate: 0,
            refused_non_similar: 0,
            refused_bad_scale: 0,
            refused_too_big: 0,
            refused_too_far: 0,
            refused_under_half_visible: 0,
        }
    }
}

impl SymbolSceneCache {
    /// Retention numbers for `frame_cost`: `(entries, bytes, budget_bytes,
    /// evictions, first_sightings_drawn_live)`.
    pub fn retention_stats(&self) -> (usize, usize, usize, u64, u64) {
        (
            self.entries.len(),
            self.bytes,
            self.budget_bytes,
            self.evictions,
            self.first_sightings,
        )
    }

    /// Every refusal counter, named, in the order `try_stamp_symbol` tests
    /// them — the diagnosis `docs/BIMARI5_LAG_PLAN.md` S2 asks for. A fixed
    /// order and a fixed set of names, so two snapshots (taken with this
    /// same method) can be zipped into a per-reason delta.
    pub fn refusal_counts(&self) -> Vec<(&'static str, u64)> {
        vec![
            ("disabled", self.refused_disabled),
            ("no_info", self.refused_no_info),
            ("filters", self.refused_filters),
            ("group_blend", self.refused_group_blend),
            ("non_flat", self.refused_non_flat),
            ("inverse_mask", self.refused_inverse_mask),
            ("neutral_ctx", self.refused_neutral_ctx),
            ("lit_silhouette", self.refused_lit_silhouette),
            ("no_affine_projection", self.refused_no_affine_projection),
            ("degenerate", self.refused_degenerate),
            ("non_similar", self.refused_non_similar),
            ("bad_scale", self.refused_bad_scale),
            ("too_big", self.refused_too_big),
            ("too_far", self.refused_too_far),
            ("under_half_visible", self.refused_under_half_visible),
        ]
    }
}

/// vello::Scene is not `Debug`, so spell the cache's out by hand.
impl std::fmt::Debug for SymbolSceneCache {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SymbolSceneCache")
            .field("entries", &self.entries.len())
            .field("enabled", &self.enabled)
            .field("builds", &self.builds)
            .field("stamps", &self.stamps)
            .finish()
    }
}

/// **One stamp built on a worker's cache, ready to install in another**
/// ([`DrawCache::drain_new_stamps`], [`DrawCache::install_stamps`]). Everything
/// a [`SymEntry`] holds: a stamp's bytes depend only on its key (E3), so the
/// scene is exactly what the installing cache would have built itself.
#[derive(Clone)]
pub struct NewStamp {
    pub key: SymKey,
    pub scene: Arc<crate::vello::Scene>,
    /// Approximate size of `scene` (see [`scene_bytes`]).
    pub bytes: usize,
    symbol: Arc<buzz_scene::Symbol>,
    anchor: buzz_geom::Point,
    lines: u64,
    silhouette: Option<Arc<Vec<buzz_geom::BezPath>>>,
}

impl SymbolSceneCache {
    /// Install `stamp` unless the key is already held: marked sighted (so a
    /// later miss for it is not drawn live as a first sighting), counted in
    /// `bytes`, and **transient** until a frame uses it. `true` when inserted.
    fn install(&mut self, stamp: NewStamp) -> bool {
        self.install_as(stamp, false)
    }

    /// [`Self::install`], optionally **retained** (S9): not marked transient.
    fn install_as(&mut self, stamp: NewStamp, retained: bool) -> bool {
        if self.entries.contains_key(&stamp.key) {
            return false;
        }
        self.clock += 1;
        self.sighted.insert(stamp.key, self.frame);
        self.bytes += stamp.bytes;
        if !retained {
            self.transient.insert(stamp.key);
        }
        self.entries.insert(
            stamp.key,
            SymEntry {
                scene: stamp.scene,
                symbol: stamp.symbol,
                anchor: stamp.anchor,
                used: self.frame,
                lines: stamp.lines,
                bytes: stamp.bytes,
                tick: self.clock,
                silhouette: stamp.silhouette,
            },
        );
        self.installed += 1;
        true
    }
}

/// What makes two instances share one cached encoding.
///
/// `depth` is in the key because symbol nesting is truncated at
/// [`MAX_SYMBOL_DEPTH`], so the same symbol drawn deeper can render less;
/// `scale_bits` is in it because the encoding is built at one zoom (the
/// flattening tolerance and baked stroke widths are zoom-specific). The camera
/// anchor is deliberately *not* in the key — the encoding is anchor-independent,
/// which is what makes panning free.
///
/// `sealed` is in it too, and for the same reason as `scale_bits`: it changes
/// what gets encoded, not just how it is placed. A symbol built while
/// [`LightDetail`] still allowed the seam seal, and later stamped once the
/// level dropped to [`LightDetail::Unsealed`], would bring the doubled seal
/// geometry back for free — the one thing that level exists to avoid.
///
/// `stroke_scale_bits` is in it too: it is baked into every stroke width the
/// child encodes (see [`DrawCtx::stroke_scale`]), and it depends on where the
/// instance sits, not only on its own scale — the same symbol stamped
/// straight on stage and stamped again nested inside a *different* stamped
/// parent divides out a different accumulated `s_q`, so they need different
/// compensation and must not share an entry.
///
/// `effect_bits` is the composed [`buzz_scene::ColorTransform`] the child is
/// encoded under — its four `multiply` and four `add` channels, each taken
/// exactly by `f32::to_bits` rather than hashed or quantised — so a symbol
/// under a colour effect (tint, alpha, brightness, or any Advanced mix of
/// them) can still be stamped: two instances only share an entry when their
/// effects are bit-for-bit the same, which is exactly when a live draw of one
/// would look identical to a live draw of the other. The identity effect
/// packs to the same bits every time, so a plain (uncoloured) symbol keys and
/// hits exactly as it did before this field existed.
///
/// `lit`, `light_print`, `light_depth_bits` and `stage_height_bits` are the
/// light a **lit stamp** was encoded under (`RIG_SCENE_LAG_PLAN` L3''):
/// `lit` keeps a lit stamp and an unlit one of the same symbol apart,
/// `light_print` is `LightRig::fingerprint()` of the rig resolved at the frame
/// (a different sun colour, sky or base is a different stamp), and the layer's
/// depth and the stage's height are the other two inputs of `LightRig::field`.
/// All four are `false` / `0` for an unlit stamp, so an unlit key is exactly
/// what it was before they existed, whatever rig the document carries.
///
/// `linear_bits` is `[0; 4]`, ignored, for a similarity placement — the
/// encoding never bakes in more than the quantised scale already in
/// `scale_bits`, so every rotation and mirroring of it shares one entry, as
/// before this field existed. For a **non-similar** placement (S2 in
/// `docs/BIMARI5_LAG_PLAN.md`) it holds the placement's own 2×2 linear part —
/// `a0, a1, a2, a3` from `Affine::as_coeffs`, each taken exactly by
/// `f64::to_bits` — because that whole matrix, not just a scale, is baked
/// into the child's geometry (see `try_stamp_symbol`): two instances only
/// share an entry when their linear parts are bit-for-bit the same, which is
/// exactly when a live draw of one would look identical to a live draw of the
/// other. `non_similar` keeps the two kinds of entry apart even in the
/// (astronomically unlikely) event a quantised-scale similarity and an exact
/// linear-part placement land on the same `scale_bits`.
#[derive(PartialEq, Eq, Hash, Clone, Copy, Debug)]
pub struct SymKey {
    fingerprint: u64,
    inner: u32,
    depth: u8,
    scale_bits: u64,
    sealed: bool,
    stroke_scale_bits: u64,
    effect_bits: [u32; 8],
    non_similar: bool,
    linear_bits: [u64; 4],
    lit: bool,
    light_print: u64,
    light_depth_bits: u64,
    stage_height_bits: u64,
}

/// Pack a [`buzz_scene::ColorTransform`]'s eight channels into exact bits for
/// [`SymKey`] — the same scheme `recoloured_image`'s cache key uses, so a
/// symbol's stamp-cache entry and its images' recolour-cache entry agree on
/// what "the same effect" means.
fn effect_key_bits(effect: &buzz_scene::ColorTransform) -> [u32; 8] {
    let mut bits = [0u32; 8];
    for i in 0..4 {
        bits[i] = effect.multiply[i].to_bits();
        bits[4 + i] = effect.add[i].to_bits();
    }
    bits
}

struct SymEntry {
    scene: Arc<crate::vello::Scene>,
    /// Guards against a fingerprint hash collision: a hit must point at the same
    /// symbol it was built from.
    symbol: Arc<buzz_scene::Symbol>,
    /// The symbol's own centre — `resolved_bounds.center()`, in the symbol's
    /// untransformed local space, whatever linear part `build_doc` carried
    /// into the geometry itself (identity for a similarity; the placement's
    /// own linear part for a non-similarity — see `try_stamp_symbol`). The
    /// stamp (`symbol_stamp`) is derived from this exact value, never a
    /// recomputed one, and never the linear-part-transformed anchor the
    /// build's own `RenderSplit` used to re-centre the transformed geometry.
    anchor: buzz_geom::Point,
    used: u64,
    /// The child's own `estimated_lines()`, measured when it was built. A
    /// stamp — hit or miss — adds this to the parent's own count rather than
    /// re-walking the cached scene; see `stamp_scene`.
    lines: u64,
    /// Approximate size of `scene`'s encoding, measured once at insert
    /// ([`scene_bytes`]).
    bytes: usize,
    /// LRU order: `SymbolSceneCache::clock` at the last hit or insert.
    tick: u64,
    /// **The symbol's filled paths, in the stamp's local space**, one per shape
    /// exactly as `draw_shape` handed them to `LightCache::add_silhouette`
    /// during the build (`RIG_SCENE_LAG_PLAN` L3''-3). `Some` for a lit
    /// similarity stamp only: a figure pass gathering its silhouette gets
    /// `placement * piece` for each on a hit, as the live draw would have
    /// given it. `None` for an unlit stamp (no figure is ever gathered there)
    /// and for a lit *non-similar* one, whose local space has the projection
    /// baked in (such an instance is drawn live while a silhouette is being
    /// gathered). Not counted in `bytes`: the retention accounting is the
    /// encoded scene's, unchanged.
    silhouette: Option<Arc<Vec<buzz_geom::BezPath>>>,
}

/// Approximate heap size of an encoded scene: its encoding streams' lengths
/// times their element sizes. Late-bound resources (images, gradients) are
/// not counted; they are shared, not copied, by a stamp.
fn scene_bytes(scene: &crate::vello::Scene) -> usize {
    let e = scene.encoding();
    std::mem::size_of_val(e.path_tags.as_slice())
        + std::mem::size_of_val(e.path_data.as_slice())
        + std::mem::size_of_val(e.draw_tags.as_slice())
        + std::mem::size_of_val(e.draw_data.as_slice())
        + std::mem::size_of_val(e.transforms.as_slice())
        + std::mem::size_of_val(e.styles.as_slice())
}

/// How many generations a `seen_off_screen` sighting survives, matching the
/// other caches. (Stamp entries themselves no longer age by generation: they
/// stay until the memory budget or [`SYM_CACHE_CAP`] evicts the least
/// recently used.)
const SYM_KEEP_FRAMES: u64 = 3;
/// Default memory budget for the stamp cache, in MiB
/// (`BUZZ_STAMP_CACHE_MB` overrides).
const SYM_CACHE_DEFAULT_MIB: usize = 512;
/// How many generations a first sighting waits for its second. Long, so a
/// scrub that returns to a frame is recognised.
const SYM_SIGHT_KEEP_FRAMES: u64 = 4096;
/// Ceiling on first-sighting records (keys are ~100 bytes each).
const SYM_SIGHT_CAP: usize = 1 << 16;
/// Ceiling on live entries. A zoom gesture rekeys everything each frame, so
/// without a cap three generations of full encodings would accumulate.
///
/// **256 thrashed on a real import.** Once `try_stamp_symbol` learned to stamp
/// scaled and mirrored placements (see its doc), the key space widened to
/// every `(symbol, inner frame, depth, quantised scale)` combination actually
/// on stage — 589 symbols, several scale buckets apiece from ordinary
/// authoring variety, and an animated `inner` frame per distinct playhead. A
/// cap of 256 evicted entries that were about to be reused the very next
/// frame, so every stamp became a rebuild. 4096 gives that variety headroom
/// without keeping every generation forever, which is what `end`'s ageing is
/// still for.
const SYM_CACHE_CAP: usize = 4096;

impl SymbolSceneCache {
    fn begin(&mut self) {
        self.frame = self.frame.wrapping_add(1);
    }

    /// Evict the least recently used **retained** entries (never an installed,
    /// not yet used one) until `bytes <= floor`.
    fn trim_retained(&mut self, floor: usize) {
        let mut order: Vec<(u64, SymKey)> = self
            .entries
            .iter()
            .filter(|(k, _)| !self.transient.contains(k))
            .map(|(k, e)| (e.tick, *k))
            .collect();
        order.sort_unstable_by_key(|(t, _)| *t);
        for (_, key) in order {
            if self.bytes <= floor {
                break;
            }
            if let Some(e) = self.entries.remove(&key) {
                self.bytes = self.bytes.saturating_sub(e.bytes);
                self.evictions += 1;
                if self.defer_frees {
                    self.graveyard.push(e.scene);
                }
            }
        }
    }

    fn end(&mut self) {
        let frame = self.frame;
        // Retention across frames: entries stay until the byte budget (or
        // the count ceiling) says otherwise, then the least recently used go
        // first — a scrub back to an earlier frame finds its stamps.
        if self.bytes > self.budget_bytes || self.entries.len() > self.entry_cap {
            // E6: installed-but-unused (transient) entries go before anything
            // a frame has used, each group least recently used first. With no
            // transient entry this is the plain LRU order.
            //
            // **Window mode (S9) reverses the groups:** the cache is full of
            // stamps a warm-up built for frames the playhead has left behind
            // (or has not reached), and what was installed ahead is what the
            // next frames are about to use; throwing it away first made the
            // encode thread rebuild it every frame.
            let window = self.window;
            let mut order: Vec<(bool, u64, SymKey)> = self
                .entries
                .iter()
                .map(|(k, e)| (self.transient.contains(k) == window, e.tick, *k))
                .collect();
            order.sort_unstable_by_key(|(retained, t, _)| (*retained, *t));
            // Window mode evicts below the budget, so the next sort is a tenth
            // of the budget away rather than the next stamp.
            let floor = if self.window {
                self.budget_bytes - self.budget_bytes / 10
            } else {
                self.budget_bytes
            };
            for (_, _, key) in order {
                if self.bytes <= floor && self.entries.len() <= self.entry_cap {
                    break;
                }
                if let Some(e) = self.entries.remove(&key) {
                    self.bytes = self.bytes.saturating_sub(e.bytes);
                    self.evictions += 1;
                    self.transient.remove(&key);
                    if self.defer_frees {
                        self.graveyard.push(e.scene);
                    }
                }
            }
        }
        // First sightings: age out on a long horizon, checked every 64
        // generations (or at once when over the count cap).
        if frame % 64 == 0 || self.sighted.len() > SYM_SIGHT_CAP {
            self.sighted
                .retain(|_, seen| frame.saturating_sub(*seen) < SYM_SIGHT_KEEP_FRAMES);
            if self.sighted.len() > SYM_SIGHT_CAP {
                let mut gens: Vec<u64> = self.sighted.values().copied().collect();
                gens.sort_unstable();
                let cut = gens[gens.len() / 2];
                self.sighted.retain(|_, g| *g > cut);
            }
        }
        // Same idea as `entries`' own ageing: a key not judged off-screen in
        // a while is not "static across frames" any more (a camera move or a
        // scrub could have carried it on-screen and back off again since),
        // so it starts over rather than staying "static" forever from one
        // stale sighting.
        self.seen_off_screen
            .retain(|_, seen| frame.saturating_sub(*seen) < SYM_KEEP_FRAMES);
    }
}

/// **Per-layer encoded scene fragments, reused across frames when a layer's
/// own content, lighting and view have not changed.** P2 in
/// `docs/IMPORT_PERF_PLAN.md`: a static layer — the common case, a
/// background that never moves — costs nothing beyond the first frame it is
/// asked for.
///
/// Built the same way [`SymbolSceneCache`] is, and for the same reason: an
/// entry is encoded once, in a space independent of whatever varies fastest,
/// and cheaply re-placed afterwards. For a symbol that is the instance's own
/// placement; for a layer here it is the story's own camera — see
/// [`try_stamp_layer`].
pub struct LayerSceneCache {
    entries: std::collections::HashMap<LayerKey, LayerEntry>,
    /// **Off by default** until it is pixel-faithful: on 2Bhatta2 frame 300
    /// (camera zoom 4.3) it fattened every outline, filled the wheel spokes
    /// solid and swelled thin artwork (5.6% of pixels wrong, worst 255/255).
    /// `BUZZ_LAYER_CACHE=1` turns it on for parity work; `set_layer_reuse`
    /// turns it on in a test. See `docs/IMPORT_PERF_PLAN.md` P2.
    enabled: bool,
    /// Generation, bumped per window frame like the other caches here.
    frame: u64,
    /// Total bytes of every cached fragment, approximated from its own
    /// `lines` — `vello::Encoding` exposes no byte count of its own. Tracked
    /// so [`Self::end`] can evict the least-recently-used entries down to
    /// [`LAYER_CACHE_BUDGET_BYTES`], P2's per-cache memory budget.
    bytes: usize,
    /// How many layer fragments were encoded this session, for tests and the
    /// HUD.
    pub builds: u64,
    /// How many times a cached fragment was stamped, likewise.
    pub stamps: u64,
}

impl Default for LayerSceneCache {
    fn default() -> Self {
        Self {
            entries: std::collections::HashMap::new(),
            enabled: std::env::var("BUZZ_LAYER_CACHE").is_ok(),
            frame: 0,
            bytes: 0,
            builds: 0,
            stamps: 0,
        }
    }
}

/// `vello::Scene` is not `Debug`, so spell the cache's out by hand — mirrors
/// [`SymbolSceneCache`]'s own `Debug`.
impl std::fmt::Debug for LayerSceneCache {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LayerSceneCache")
            .field("entries", &self.entries.len())
            .field("enabled", &self.enabled)
            .field("bytes", &self.bytes)
            .field("builds", &self.builds)
            .field("stamps", &self.stamps)
            .finish()
    }
}

/// What makes two frames' worth of one layer share a cached fragment. See
/// [`try_stamp_layer`] for what each field guards against.
#[derive(PartialEq, Eq, Hash, Clone, Copy, Debug)]
struct LayerKey {
    layer: u64,
    /// The layer's resolved content identity at this frame: the `Arc`
    /// pointers of its stored objects, folded together. Only reached for
    /// [`buzz_scene::ResolvedFrame::Stored`] — a tween has no stable identity
    /// to key on and is never cached; see [`try_stamp_layer`].
    content: u64,
    detail: u8,
    scale_bits: u64,
    /// The visible rect, quantised to a 64-px grid of the view — see
    /// [`try_stamp_layer`]. `(x0, y0, x1, y1)` in grid cells.
    cull: (i64, i64, i64, i64),
    /// [`FrameOptions::place`], folded into one number — see [`affine_hash`].
    /// Baked into the fragment at build time (it sits *inside* the camera in
    /// `draw_layer`'s own projection chain, so it cannot be re-applied on a
    /// hit the way the camera is), so a different value must be a different
    /// entry. Constant for almost every caller — `place` only moves while
    /// editing a symbol in place — so this costs real callers nothing.
    place_bits: u64,
    /// What this layer inherits from the layer it follows
    /// (`buzz_scene::LayerStack::inherited_transform`), folded the same way
    /// as `place_bits`. **Found by a real regression**: a parented layer
    /// whose own content never changes but whose parent moves was caching
    /// the first frame's placement and never re-encoding — the picture
    /// stopped following. `follows` is baked in for the same reason `place`
    /// is: it sits inside the camera in the projection chain, so only the
    /// camera itself can be replayed on a hit.
    follows_bits: u64,
}

/// Fold an affine's six coefficients into one key-shaped number. Not a
/// geometric measure of anything — just a cheap, exact (no quantisation)
/// distinguisher for [`LayerKey`], the same role `scale_bits` already plays
/// via a bare `.to_bits()`.
fn affine_hash(a: Affine) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for c in a.as_coeffs() {
        h ^= c.to_bits();
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

struct LayerEntry {
    scene: Arc<crate::vello::Scene>,
    /// The exact `Arc` pointers this entry was built from, kept so a hit can
    /// be verified rather than trusted from the hashed key alone — the same
    /// guard `SymEntry::symbol` gives `try_stamp_symbol`. `LayerKey::content`
    /// is a 64-bit fold of these same pointers, which a collision (or two
    /// unrelated documents whose freed and reallocated `Arc`s land at the
    /// same addresses — a real risk across `reset_caches`-free reuse of one
    /// `DrawCache`, e.g. switching documents in the open editor) could match
    /// by accident.
    objects: Vec<Arc<Object>>,
    used: u64,
    /// The fragment's own `estimated_lines()`, measured when it was built —
    /// added to the parent's line count on both a hit and a miss; see
    /// `layer_stamp`.
    lines: u64,
    /// This entry's approximate share of [`LayerSceneCache::bytes`].
    bytes: usize,
}

/// Do `a` and `b` hold exactly the same objects, in the same order? The
/// verification [`try_stamp_layer`] runs on a key hit — see
/// [`LayerEntry::objects`].
fn same_objects(a: &[Arc<Object>], b: &[Arc<Object>]) -> bool {
    a.len() == b.len() && a.iter().zip(b).all(|(x, y)| Arc::ptr_eq(x, y))
}

/// How many generations an unused layer fragment survives, matching
/// [`SYM_KEEP_FRAMES`].
const LAYER_KEEP_FRAMES: u64 = 3;

/// The memory cap [`LayerSceneCache::end`] evicts down to — P2's per-cache
/// budget in `docs/IMPORT_PERF_PLAN.md`.
const LAYER_CACHE_BUDGET_BYTES: usize = 256 * 1024 * 1024;

/// A deliberately generous per-line byte estimate for
/// [`LayerSceneCache::end`]'s budget — `vello::Encoding` exposes no byte
/// count of its own to measure instead. Erring high evicts a little early;
/// erring low would let the cache overrun the budget it exists to enforce.
const LAYER_BYTES_PER_LINE: usize = 64;

impl LayerSceneCache {
    fn begin(&mut self) {
        self.frame = self.frame.wrapping_add(1);
    }

    fn end(&mut self) {
        let frame = self.frame;
        let bytes = &mut self.bytes;
        self.entries.retain(|_, e| {
            let keep = frame.saturating_sub(e.used) < LAYER_KEEP_FRAMES;
            if !keep {
                *bytes = bytes.saturating_sub(e.bytes);
            }
            keep
        });
        // Still over the memory budget: drop the least-recently-used entries
        // until under it, same idiom as `SymbolSceneCache::end`'s count cap.
        while self.bytes > LAYER_CACHE_BUDGET_BYTES {
            let Some(oldest) = self
                .entries
                .iter()
                .min_by_key(|(_, e)| e.used)
                .map(|(k, _)| *k)
            else {
                break;
            };
            if let Some(e) = self.entries.remove(&oldest) {
                self.bytes = self.bytes.saturating_sub(e.bytes);
            }
        }
    }
}

/// Snap `rect` to a grid of `cell` document units on a side, outward —
/// [`try_stamp_layer`]'s "quantised to a 64-px grid of the view". `(0,0,0,0)`
/// for a degenerate cell, which only ever makes the cache miss, never wrong.
fn quantise_cull(rect: buzz_geom::Rect, cell: f64) -> (i64, i64, i64, i64) {
    if !(cell > 0.0) || !cell.is_finite() {
        return (0, 0, 0, 0);
    }
    (
        (rect.x0 / cell).floor() as i64,
        (rect.y0 / cell).floor() as i64,
        (rect.x1 / cell).ceil() as i64,
        (rect.y1 / cell).ceil() as i64,
    )
}

/// Fold a layer's stored objects' own `Arc` addresses together into one
/// identity for [`LayerKey::content`]. Two frames whose stored artwork is
/// the same objects, in the same order, hash equal; anything else — an edit,
/// a different keyframe — does not, which is exactly [`ResolvedFrame::Stored`]'s
/// own contract (see its doc).
fn layer_content_hash(layer_id: buzz_scene::LayerId, objects: &[Arc<Object>]) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    layer_id.0.hash(&mut hasher);
    for o in objects {
        (Arc::as_ptr(o) as usize as u64).hash(&mut hasher);
    }
    hasher.finish()
}

/// How finely `try_stamp_symbol` snaps an instance's scale before it is baked
/// into the child's encoding and its cache key: steps per octave. Large enough
/// that the snap is float noise (see the comment at its use), small enough
/// that rotations of one scale share an entry.
const SYMBOL_SCALE_STEPS_PER_OCTAVE: f64 = 1_048_576.0;

/// Whether a 2×2 linear part (kurbo coeffs `[a,b,c,d]`) is orthogonal — a pure
/// rotation, reflection or the identity, with no scale or shear. Only then do
/// baked stroke and seam-seal widths, which scale by the view zoom alone, stay
/// screen-correct after the instance's transform is folded in.
///
/// `eps` is a tolerance for "near-similarity after scale is factored out", not
/// for exact orthogonality: `try_stamp_symbol` calls this on `r = A_lin / s_q`,
/// the placement's linear part with its own scale quantised and divided back
/// out, and passes a tolerance wide enough to admit that quantisation's
/// residual (up to about ±1.1%) and a like amount of genuine anisotropy. A
/// caller after an exact test — this file has none left, but a future one
/// might — should pass an `eps` near machine precision instead.
fn is_orthogonal(coeffs: [f64; 6], eps: f64) -> bool {
    let [a, b, c, d, _, _] = coeffs;
    (a * a + b * b - 1.0).abs() < eps
        && (c * c + d * d - 1.0).abs() < eps
        && (a * c + b * d).abs() < eps
}

/// The transform that carries a child scene, encoded as `child_scale·(p −
/// anchor)` about the symbol centre, into the stage's render space.
///
/// Derivation: the stage needs `gpu_view ∘ S·(A·p − cam)` where `A` is the
/// instance's placement (projection ∘ document transform) and `S` is the
/// stage's own scale. A child whose only eligibility is an *exactly*
/// orthogonal `A` can be encoded at `child_scale = S` and stamped with `A`'s
/// own linear part, `A_lin`, which is what this used to do unconditionally.
///
/// `try_stamp_symbol` now also admits a placement that is a *similarity*,
/// `A_lin = s_q·r` with `r` near-orthogonal and `s_q` `A`'s scale quantised to
/// 1/32 octave. That child is encoded at `child_scale = S·s_q` instead, and
/// `r` carries the stamp in place of `A_lin` — because `r` and `A_lin` differ
/// by exactly the factor `s_q` already folded into `child_scale`:
/// `r·(S·s_q·(p − anchor)) = S·A_lin·(p − anchor)` for every `p`, so the two
/// ways of getting there land on the same point. `r = A_lin` exactly when
/// `s_q = 1`, which is every placement this took before quantisation existed.
/// Caller's choice which linear part `r` is: `A_lin` itself, or the quantised
/// one. Everything but the final compose is `f64`.
///
/// S2 (`docs/BIMARI5_LAG_PLAN.md`) admits a third case, a genuine
/// non-similarity: the child is encoded with `A_lin` baked into its own
/// geometry (`build_doc` in `try_stamp_symbol`) instead of factored out as a
/// scale, at `child_scale = S`, and `r` is left at the identity — the
/// geometry already carries the whole linear part, so the append adds only a
/// translation. This function's own formula for `t` does not change: `anchor`
/// here is always the symbol's untransformed centre (`SymEntry::anchor`),
/// never the `A_lin`-transformed point the child build's own `RenderSplit`
/// re-centred its geometry about, and `t = S·(A·anchor − cam)` holds exactly
/// regardless of which of the three cases above chose `r` and `child_scale` —
/// only their *product* with the geometry's own linear part is constrained
/// (it must equal `S·A_lin`), and `anchor` is a free constant either way.
fn symbol_stamp(
    gpu_view: Affine,
    a: Affine,
    r: [f64; 4],
    anchor: buzz_geom::Point,
    scale: f64,
    cam: buzz_geom::Point,
) -> Affine {
    let t = ((a * anchor) - cam) * scale;
    gpu_view * Affine::new([r[0], r[1], r[2], r[3], t.x, t.y])
}

impl DrawCache {
    pub fn new() -> Self {
        Self::default()
    }

    /// Open a **window frame**, which may draw several document frames.
    ///
    /// # Why this is not per document frame
    ///
    /// One frame on screen can be a dozen draws: the onion-skin ghosts either
    /// side, every keyframe under Edit Multiple Frames, the scene behind an
    /// opened symbol, and the live frame itself. Each of those used to open and
    /// close the cache on its own, and both caches evict anything not drawn
    /// within a few generations — so seven passes over one screen frame aged
    /// the first pass out before the last had finished, and every ghost rebuilt
    /// every blur from nothing. Six ghosts of a foggy scene cost a quarter of a
    /// second a frame, which is four frames a second to step through.
    ///
    /// The generation belongs to the *screen* frame. Everything drawn within
    /// one shares it, which is what the eviction budget was written for — the
    /// comment on `KEEP_FRAMES` says "enough to cover onion skinning" and this
    /// is what finally makes that true.
    pub fn begin(&mut self) {
        self.lights.begin();
        self.filters.begin();
        self.bounds.begin();
        self.layer_fx.begin();
        self.silhouette.begin();
        self.shadow_memo.begin();
        self.symbol_scenes.begin();
        self.layer_scenes.begin();
        self.slice_counter = 0;
        // L7: what each lighting level cost is a fact about one screen frame.
        self.clear_measured();
        self.last_pass_over = false;
    }

    pub fn end(&mut self) {
        self.lights.end();
        self.filters.end();
        self.bounds.end();
        self.layer_fx.end();
        self.silhouette.end();
        self.shadow_memo.end();
        self.symbol_scenes.end();
        self.layer_scenes.end();
    }

    /// Turn the layer-fx and silhouette memos on or off. Off, every layer's rim
    /// and filters are gathered and built afresh each frame; the picture is
    /// identical, only slower. For A/B tests (`BUZZ_NO_LAYER_FX_MEMO=1` sets the
    /// default for a process).
    #[doc(hidden)]
    pub fn set_layer_fx_memo(&mut self, on: bool) {
        self.layer_fx.enabled = on;
        self.silhouette.enabled = on;
    }

    /// **Test reading:** the ops last kept for `layer`'s rim and filters, so a
    /// test can see whether two frames shared them (`Arc::ptr_eq`).
    #[doc(hidden)]
    pub fn layer_fx_probe(&self, layer: buzz_scene::LayerId) -> Option<Arc<buzz_fx::Painted>> {
        self.layer_fx
            .entries
            .iter()
            .filter(|((l, _), _)| *l == layer)
            .max_by_key(|(_, entry)| (entry.used, entry.seq))
            .map(|(_, entry)| Arc::clone(&entry.fx))
    }

    /// **Test reading:** `(memo hits, memo misses)` for layer fx.
    #[doc(hidden)]
    pub fn layer_fx_counts(&self) -> (u64, u64) {
        (self.layer_fx.hits, self.layer_fx.misses)
    }

    /// Turn the shadow-fragment memo on or off. Off, every layer's shadow pass is
    /// encoded straight into the frame; the picture is identical, only slower.
    /// For A/B tests (`BUZZ_NO_SHADOW_MEMO=1` sets the default for a process).
    #[doc(hidden)]
    pub fn set_shadow_memo(&mut self, on: bool) {
        self.shadow_memo.enabled = on;
    }

    /// **Test reading:** `(fragments appended from the memo, passes encoded and
    /// kept)` for the shadow memo.
    #[doc(hidden)]
    pub fn shadow_memo_counts(&self) -> (u64, u64) {
        (self.shadow_memo.hits, self.shadow_memo.misses)
    }

    /// **Test reading:** silhouette work, for the lifetime of the cache:
    /// `(shapes the live walk visited, outlines replayed from a part's memo,
    /// parts recorded)`.
    #[doc(hidden)]
    pub fn silhouette_work(&self) -> (u64, u64, u64) {
        (
            self.silhouette.live,
            self.silhouette.replayed,
            self.silhouette.recorded,
        )
    }

    /// Turn the symbol-encoding cache on or off. Off, every instance is drawn
    /// live; the result is identical, only slower on instance-heavy documents.
    pub fn set_symbol_reuse(&mut self, on: bool) {
        self.symbol_scenes.enabled = on;
    }

    /// Build a stamp on a key's **first** sighting instead of drawing it live
    /// and building on the second (S2-rest (b)). Off by default; a warm-up
    /// sweep that pre-builds the whole document turns it on for its duration.
    pub fn set_build_on_first_sighting(&mut self, on: bool) {
        self.symbol_scenes.build_on_first_sighting = on;
    }

    /// **E6:** record every stamp this cache builds, for
    /// [`Self::drain_new_stamps`]. Off by default (a cache nobody drains must
    /// not collect).
    pub fn set_collect_new_stamps(&mut self, on: bool) {
        self.symbol_scenes.collect_new = on;
        if !on {
            self.symbol_scenes.fresh.clear();
        }
    }

    /// **E6: the stamps built since the last drain** (keys, scenes, bytes), for
    /// another cache to [install](Self::install_stamps). Entries evicted since
    /// they were built are skipped.
    pub fn drain_new_stamps(&mut self) -> Vec<NewStamp> {
        let sc = &mut self.symbol_scenes;
        let keys = std::mem::take(&mut sc.fresh);
        keys.into_iter()
            .filter_map(|key| {
                let e = sc.entries.get(&key)?;
                Some(NewStamp {
                    key,
                    scene: Arc::clone(&e.scene),
                    bytes: e.bytes,
                    symbol: Arc::clone(&e.symbol),
                    anchor: e.anchor,
                    lines: e.lines,
                    silhouette: e.silhouette.clone(),
                })
            })
            .collect()
    }

    /// **E6: install stamps built elsewhere.** Each is inserted only when its
    /// key is absent, marked sighted, counted in the byte total, and marked
    /// **transient**: [`Self::end`] evicts those before anything a frame has
    /// used. Answers how many were inserted.
    pub fn install_stamps(&mut self, stamps: Vec<NewStamp>) -> usize {
        stamps
            .into_iter()
            .map(|s| usize::from(self.symbol_scenes.install(s)))
            .sum()
    }

    /// **S9: install stamps a warm-up at document open built.** As
    /// [`Self::install_stamps`], except the stamps are **retained**, not
    /// transient: they are what the film needs, not a guess about the next few
    /// frames, so [`Self::end`] treats them as ordinary LRU entries.
    pub fn install_stamps_retained(&mut self, stamps: Vec<NewStamp>) -> usize {
        stamps
            .into_iter()
            .map(|s| usize::from(self.symbol_scenes.install_as(s, true)))
            .sum()
    }

    /// **S9: the stamp cache's limits** - the byte budget and the entry-count
    /// ceiling. The defaults (512 MiB, 4096 entries, `BUZZ_STAMP_CACHE_MB`)
    /// hold a few frames; a warmed film holds tens of thousands of stamps, so
    /// the warm-up raises both and the byte budget alone decides.
    pub fn set_stamp_limits(&mut self, budget_bytes: usize, max_entries: usize) {
        self.symbol_scenes.budget_bytes = budget_bytes;
        self.symbol_scenes.entry_cap = max_entries.max(1);
    }

    /// **S9: window mode** for a cache a warm-up filled (see
    /// [`SymbolSceneCache`]'s `window`): eviction goes down to 90% of the budget
    /// at a time, and [`Self::make_room_for_stamps`] works.
    pub fn set_stamp_window(&mut self, on: bool) {
        self.symbol_scenes.window = on;
    }

    /// **S9: make room for stamps built ahead** when the cache is over 90% of
    /// its budget: in window mode the least recently used retained stamps (the
    /// frames the playhead has left behind) go until it is at 80%. Answers
    /// whether the cache is now at or under 90%; always `false` outside window
    /// mode (a full cache then simply takes no more).
    pub fn make_room_for_stamps(&mut self) -> bool {
        let sc = &mut self.symbol_scenes;
        if !sc.window {
            return false;
        }
        let budget = sc.budget_bytes;
        if sc.bytes.saturating_mul(10) > budget.saturating_mul(9) {
            sc.trim_retained(budget - budget / 5);
        }
        sc.bytes.saturating_mul(10) <= budget.saturating_mul(9)
    }

    /// The stamp cache's byte budget.
    pub fn stamp_budget(&self) -> usize {
        self.symbol_scenes.budget_bytes
    }

    /// Bytes the held stamps account for (see [`scene_bytes`]).
    pub fn stamp_bytes(&self) -> usize {
        self.symbol_scenes.bytes
    }

    /// How many distinct stamps are held.
    pub fn stamp_count(&self) -> usize {
        self.symbol_scenes.entries.len()
    }

    /// **S9: take every held stamp out** (a zoom change makes them all
    /// unreachable, so they are dropped at once); the encodings come back so the
    /// caller can free them off its own thread.
    pub fn take_all_stamps(&mut self) -> Vec<Arc<crate::vello::Scene>> {
        let sc = &mut self.symbol_scenes;
        sc.transient.clear();
        sc.fresh.clear();
        sc.bytes = 0;
        sc.entries.drain().map(|(_, e)| e.scene).collect()
    }

    /// Keep evicted stamps' encodings for [`Self::take_graveyard`] instead of
    /// freeing them inside [`Self::end`]. Off by default (nobody would empty it).
    pub fn defer_frees(&mut self, on: bool) {
        self.symbol_scenes.defer_frees = on;
    }

    /// The encodings evicted since the last call, to be freed by the caller (see
    /// [`Self::defer_frees`]).
    pub fn take_graveyard(&mut self) -> Vec<Arc<crate::vello::Scene>> {
        std::mem::take(&mut self.symbol_scenes.graveyard)
    }

    /// **Let go of every held stamp** (S7, RSS): a pre-build worker hands its
    /// new stamps to the encode thread's cache and keeps no copy of its own.
    /// The stamps stay alive only where somebody else holds them.
    pub fn drop_stamps(&mut self) {
        let sc = &mut self.symbol_scenes;
        sc.entries.clear();
        sc.transient.clear();
        sc.fresh.clear();
        sc.bytes = 0;
    }

    /// Is the symbol-encoding cache on? (`BUZZ_NO_SYMBOL_CACHE` turns it off.)
    pub fn symbol_reuse(&self) -> bool {
        self.symbol_scenes.enabled
    }

    /// **E6:** adopt a copy of another cache's symbol table, so a worker's cache
    /// need not re-walk the library. Valid whatever it was built from: it is
    /// refreshed against the scene on first use, as always.
    pub fn adopt_symbols(&mut self, table: &SymbolTable) {
        self.symbols = table.clone();
    }

    /// The stamp held for `key`, if any (for tests and the HUD).
    pub fn stamp_scene(&self, key: &SymKey) -> Option<Arc<crate::vello::Scene>> {
        self.symbol_scenes.entries.get(key).map(|e| Arc::clone(&e.scene))
    }

    /// Is the stamp for `key` installed but not yet used by a frame?
    pub fn stamp_is_transient(&self, key: &SymKey) -> bool {
        self.symbol_scenes.transient.contains(key)
    }

    /// **E5:** share an abort flag with this cache. While it reads `true`,
    /// [`draw_layers`] stops at the next layer boundary, so a pass nobody wants
    /// any more ends within one layer. The aborted pass leaves the cache valid —
    /// everything it built is kept and the caller still runs [`Self::end`] —
    /// but the scene it drew is partial and must be thrown away, and the caller
    /// (see [`Self::is_aborted`]) must not judge it with `note_lines` /
    /// `reconsider`.
    pub fn set_abort(&mut self, flag: Arc<std::sync::atomic::AtomicBool>) {
        self.abort = Some(flag);
    }

    /// **E7:** make [`draw_layers`] draw only the stage objects numbered `k`
    /// modulo `n` (`Some((k, n))`, counted across the layers in paint order from
    /// the last [`Self::begin`]), or all of them (`None`). Used by a pre-build
    /// worker that takes one share of a frame's objects; the scene it draws is
    /// partial and must be thrown away.
    pub fn set_work_slice(&mut self, slice: Option<(usize, usize)>) {
        self.work_slice = slice.filter(|&(_, n)| n > 1);
    }

    /// Test hook: raise the flag given to [`Self::set_abort`] after `n` more
    /// symbol layers have been drawn inside a symbol's contents, stopping a
    /// pass in the middle of a stamp build. Does nothing without a flag.
    #[doc(hidden)]
    pub fn trip_abort_after_symbol_layers(&mut self, n: usize) {
        self.abort_trip = Some(n);
    }

    /// Checked once per layer of a symbol's own contents (E5 reaches inside a
    /// symbol, so a stamp build ends within one layer too).
    fn symbol_layer_aborted(&mut self) -> bool {
        if let Some(left) = self.abort_trip {
            if left == 0 {
                self.abort_trip = None;
                if let Some(flag) = &self.abort {
                    flag.store(true, std::sync::atomic::Ordering::Relaxed);
                }
            } else {
                self.abort_trip = Some(left - 1);
            }
        }
        self.is_aborted()
    }

    /// Has the flag given to [`Self::set_abort`] been raised?
    pub fn is_aborted(&self) -> bool {
        self.abort
            .as_ref()
            .is_some_and(|f| f.load(std::sync::atomic::Ordering::Relaxed))
    }

    /// Band mode: one frame is being drawn as several bands in one cache
    /// generation. Each band re-requests the symbols straddling it, and without
    /// this the 2nd-sighting rule would take band 2's request for a second
    /// sighting and stamp the straddler whole into it. On, a sighting recorded
    /// in the current generation does not count, so a frame's bands count as one
    /// sighting. Off (the default) whole-frame behaviour is unchanged.
    pub fn set_band_mode(&mut self, on: bool) {
        self.symbol_scenes.band_mode = on;
    }

    /// Turn the layer-encode cache on or off — see [`LayerSceneCache`] and
    /// `BUZZ_LAYER_CACHE`. Off, every layer is drawn live; the result is
    /// identical, only slower on a document with heavy static layers.
    pub fn set_layer_reuse(&mut self, on: bool) {
        self.layer_scenes.enabled = on;
    }

    /// (builds, stamps) of the layer cache so far — for a parity test to
    /// confirm the cache actually engaged rather than passing vacuously. See
    /// [`SymbolSceneCache`]'s own `symbol_cache_stats` equivalent,
    /// `Exporter::symbol_cache_stats`.
    pub fn layer_cache_stats(&self) -> (u64, u64) {
        (self.layer_scenes.builds, self.layer_scenes.stamps)
    }
}

/// Draw one frame, reusing lighting geometry between frames.
///
/// The caller owns the cache because the renderer is stateless by design: a
/// `SceneBuilder` lives for one frame, and geometry that took a boolean to
/// build must outlive it. See [`crate::lighting`].
pub fn draw_frame_lit(
    builder: &mut SceneBuilder<'_>,
    scene: &Scene,
    frame: u32,
    camera: Affine,
    options: &FrameOptions,
    lights: &mut LightCache,
) {
    // The filter cache lives for this one frame, so a blur is rebuilt each
    // time. Callers that draw the same document repeatedly — the window —
    // should hold a `DrawCache` and call `draw_frame_cached` instead.
    let mut cache = DrawCache {
        lights: std::mem::take(lights),
        filters: FilterCache::new(),
        bounds: BoundsCache::default(),
        animated: AnimatedMemo::default(),
        layer_fx: LayerFxMemo::default(),
        silhouette: SilhouetteMemo::default(),
        shadow_memo: ShadowMemo::default(),
        symbols: SymbolTable::default(),
        symbol_scenes: SymbolSceneCache::default(),
        lamp: None,
        detail: LightDetail::default(),
        encode: 0,
        ceiling: 0,
        pinned: None,
        lines: 0,
        measured_lines: [None; 4],
        measured_segments: [None; 4],
        step_ratio: [RatioMemo::default(); 4],
        last_pass_over: false,
        last_pass_detail: LightDetail::default(),
        scale: BudgetScale::default(),
        max_scale: BudgetScale::default(),
        shrink_eligible_since: None,
        pending_shrink: None,
        layer_scenes: LayerSceneCache::default(),
        abort: None,
        work_slice: None,
        slice_counter: 0,
        abort_trip: None,
    };
    draw_frame_cached(builder, scene, frame, camera, options, &mut cache);
    *lights = cache.lights;
}

/// Draw one frame, reusing both lighting and filter geometry between frames.
///
/// This is what the window calls. [`draw_frame_lit`] exists for callers that
/// only have a [`LightCache`]; it builds a filter cache for the one frame,
/// which is correct but rebuilds every blur each time.
pub fn draw_frame_cached(
    builder: &mut SceneBuilder<'_>,
    scene: &Scene,
    frame: u32,
    camera: Affine,
    options: &FrameOptions,
    cache: &mut DrawCache,
) {
    cache.begin();
    draw_frame_within(builder, scene, frame, camera, options, cache);
    cache.end();
}

/// Draw one frame **inside** a window frame already opened on the cache.
///
/// For callers that draw several document frames per screen frame — ghosts,
/// Edit Multiple Frames, the scene behind an opened symbol — so that all of
/// them share one cache generation. See [`DrawCache::begin`].
pub fn draw_frame_within(
    builder: &mut SceneBuilder<'_>,
    scene: &Scene,
    frame: u32,
    camera: Affine,
    options: &FrameOptions,
    cache: &mut DrawCache,
) {
    draw_layers(
        builder,
        scene,
        scene.layers(),
        frame,
        camera,
        options,
        cache,
    );
}

/// Draw one symbol's own contents, with nothing of the document around it.
///
/// For a **thumbnail**: the Library and the Assets panel identify a character
/// by its name, which means opening symbols to find out what they are. A
/// picture is the whole answer.
///
/// It is the symbol's layers rather than the document's, so nothing behind or
/// in front of the instance leaks into the picture, and the caller supplies a
/// camera that fits the artwork into whatever box it has.
pub fn draw_symbol(
    builder: &mut SceneBuilder<'_>,
    scene: &Scene,
    symbol: buzz_scene::SymbolId,
    frame: u32,
    camera: Affine,
    options: &FrameOptions,
    cache: &mut DrawCache,
) {
    let Some(symbol) = scene.library().get(symbol) else {
        return;
    };
    draw_layers(
        builder,
        scene,
        &symbol.layers,
        frame,
        camera,
        options,
        cache,
    );
}

/// Draw the **document's own timeline**, whatever symbol is open.
///
/// This is the context behind Animate's Edit in Place: the scene the symbol
/// was opened from, drawn where it stands so the artwork inside the symbol can
/// be judged against what surrounds it. The caller veils it afterwards; here
/// it is simply the other stack.
///
/// It draws nothing when the document itself is what is open, so a caller need
/// not check first.
pub fn draw_stage_context(
    builder: &mut SceneBuilder<'_>,
    scene: &Scene,
    frame: u32,
    camera: Affine,
    options: &FrameOptions,
    cache: &mut DrawCache,
) {
    if scene.edit_path().is_empty() {
        return;
    }
    // No begin/end: this is one pass of a screen frame the caller has already
    // opened on the cache. See `DrawCache::begin`.
    draw_layers(
        builder,
        scene,
        scene.stage_layers(),
        frame,
        camera,
        options,
        cache,
    );
}

/// Draw one layer stack — the stage's, or a symbol's.
///
/// Masks are resolved here rather than by the caller because the rule is
/// positional: a mask claims the unbroken run of masked layers below it, and
/// only the stack knows what "below" means.
fn draw_layers(
    builder: &mut SceneBuilder<'_>,
    scene: &Scene,
    layers: &buzz_scene::LayerStack,
    frame: u32,
    camera: Affine,
    options: &FrameOptions,
    cache: &mut DrawCache,
) {
    // Bring the per-symbol memo up to date once for the whole pass. Cheap when
    // the document has not changed, which is every pan, zoom and playback frame.
    // Every `has_additive_paint` and every scene-cache lookup below reaches the
    // walk only through here, so this is the one place that has to run first.
    cache.symbols.refresh(scene);

    // Which mask, if any, clips each layer, and whether that mask is in force.
    let masks = active_masks(layers, options.masks);
    let mut open: Option<OpenMask> = None;

    // The instant this pass draws. The frame itself for every pass but a
    // motion-blurred one, where it is part-way into the frame.
    let time = options.at(frame);

    // Resolve the light rig once for the whole stack, not once per layer.
    //
    // At the whole frame, deliberately: a light's keys are held across the
    // shutter rather than swept through it. What smears is the artwork the
    // light falls on; a lamp creeping a hundredth of a frame is not something
    // a viewer can see, and sweeping it would clone the whole rig per sample
    // to prove it.
    // The stage's own frame rate, not a nominal one: a strike's envelope is
    // measured in seconds, and a storm must run at the same speed in a 60 fps
    // document as in a 24 fps one. See `LightRig::resolved_at_rate`.
    let lights = Arc::new(
        scene
            .lights()
            .resolved_at_rate(frame, scene.stage().frame_rate.max(1.0))
            .into_owned(),
    );

    // **Where each light sits in this stack.** A light anchored above a layer
    // lights that layer and those below it and nothing above — so a layer is
    // lit by the lights at or above its own row. `front` is the timeline's
    // row order, front to back. An anchor that names no layer here (deleted,
    // or a symbol's own stack being edited) is read as the top: lights all.
    let front: std::collections::HashMap<buzz_scene::LayerId, usize> =
        layers.iter().enumerate().map(|(i, l)| (l.id, i)).collect();
    let anchors: Vec<Option<usize>> = lights
        .lights
        .iter()
        .map(|l| l.above.and_then(|id| front.get(&buzz_scene::LayerId(id)).copied()))
        .collect();
    let anchored = anchors.iter().any(Option::is_some);
    // One rig per distinct set of lights, shared by every layer lit by it.
    let mut rigs: std::collections::HashMap<Vec<bool>, Arc<buzz_scene::LightRig>> =
        std::collections::HashMap::new();
    // Which lights' pools and glooms have been laid. An anchored light's are
    // laid in its slot, before the first layer above it is drawn, so the air
    // it lights is the air beneath it.
    let mut pooled = vec![false; anchors.len()];

    // Depth ordering is opt-in on the stage. The masked run stays contiguous
    // either way, so the mask machinery below is untouched; only the order the
    // layers arrive in changes. With every depth equal, the two orders are
    // identical — see `LayerStack::depth_paint_order`.
    let ordered: Vec<&Arc<buzz_scene::Layer>> = if scene.stage().sort_by_depth {
        layers.drawable_at_by_depth(frame)
    } else {
        layers.drawable_at(frame).collect()
    };
    // **A layer rig is lit as one character** (LS in
    // `docs/PRODUCTION_SPEED_PLAN.md`): its part layers throw one shadow
    // between them, under the bottom-most part, instead of each part throwing
    // its own onto the parts drawn before it. Worked out once for the pass; a
    // document with no layer parenting (or an unlit pass) gets nothing here.
    let (follow_roots, rig_shadows) = if options.lit {
        rig_shadow_groups(layers, &ordered)
    } else {
        Default::default()
    };
    for layer in ordered.iter().copied() {
        // E5: a pass nobody wants any more ends at a layer boundary.
        if cache.is_aborted() {
            break;
        }
        // A mask layer's own artwork is never drawn in the finished picture:
        // it is a stencil, and Animate hides it for the same reason. It still
        // has to be *found*, which is what `active_masks` did.
        //
        // **A mask that claims nothing is still a mask.** It used to be drawn
        // as ordinary artwork, on the grounds that it was not clipping
        // anything — so a mask layer added before the layer under it had been
        // set to Masked splattered its stencil across the whole frame, opaque
        // and full size. That is a shape nobody drew to be seen, and on a
        // vignette or a torchlight cone it covers the film.
        //
        // The one case where a mask *should* show its own artwork is Animate's
        // editing rule: on the stage, an unlocked mask is not in force and its
        // contents are visible so they can be drawn. That is `WhenLocked`, and
        // it is the only reason this is not an unconditional skip.
        let is_stencil = layer.kind.is_mask()
            && (options.masks == MaskDisplay::Always || masks.values().any(|m| *m == layer.id));
        if is_stencil {
            continue;
        }

        let row = front.get(&layer.id).copied().unwrap_or(0);
        if anchored {
            // Lights anchored below this row: their pools go down now, under
            // it. Outside any mask — a pool is not the masked artwork's.
            let due: Vec<bool> = anchors
                .iter()
                .zip(&pooled)
                .map(|(a, done)| !done && a.is_some_and(|a| a > row))
                .collect();
            if due.iter().any(|d| *d) {
                close_mask(builder, open.take());
                let rig = lights.only(|i, _| due[i]);
                draw_gloom_bands(builder, scene, frame, camera, options, &rig);
                draw_light_pools(builder, scene, frame, camera, options, &rig);
                for (p, d) in pooled.iter_mut().zip(&due) {
                    *p |= *d;
                }
            }
        }

        // Masked layers arrive in one unbroken run per mask, so the clip is
        // opened once for the run rather than once per layer.
        let wanted = masks.get(&layer.id).copied();
        if wanted != open.as_ref().map(|o| o.mask) {
            close_mask(builder, open.take());
            if let Some(mask_id) = wanted
                && let Some(path) = mask_geometry(
                    scene,
                    layers,
                    mask_id,
                    time,
                    Affine::IDENTITY,
                    &scene
                        .camera_projection_at_depth(time, 0.0)
                        .unwrap_or_else(|| Projection::from_affine(camera))
                        // The mask travels with the stack it belongs to, or it
                        // would clip the wrong part of the stage.
                        .pre_affine(options.place),
                    builder.tolerance(),
                )
            {
                open = open_mask(builder, layers, mask_id, &masks, time, path);
            }
        }

        // Layer parenting: what this layer inherits from the layer it
        // follows. Resolved here because only the stack knows the chain.
        let follows = layers.inherited_transform(layer.id, time);
        // P2 in `docs/IMPORT_PERF_PLAN.md`: try the per-layer encode cache
        // first. It only ever adds a fast path — every gate it refuses on
        // falls through to the same live `draw_layer` this always called.
        // The lights at or above this row, or the whole rig when no light
        // has been placed in the stack — the same `Arc`, so nothing changes
        // for a document that never moved one.
        let lit_by = if anchored {
            let keep: Vec<bool> = anchors.iter().map(|a| a.is_none_or(|a| a <= row)).collect();
            rigs.entry(keep)
                .or_insert_with_key(|keep| {
                    Arc::new(lights.only(|i, _| keep[i]))
                })
                .clone()
        } else {
            lights.clone()
        };
        let role = shadow_role(&follow_roots, &rig_shadows, layer);
        if !try_stamp_layer(
            builder, scene, layers, layer, frame, camera, follows, options, &lit_by, cache,
        ) {
            draw_layer(
                builder, scene, layers, layer, frame, camera, follows, options, &lit_by, cache,
                role,
            );
        }
    }

    close_mask(builder, open);

    // Every light not already laid in its slot: the top-of-stack lights, and
    // any whose slot was never passed.
    let lights = if anchored {
        Arc::new(lights.only(|i, _| !pooled[i]))
    } else {
        lights
    };

    // **Last, over everything: the light itself.**
    //
    // A lamp's pool is light in the air, so it belongs to the frame and not to
    // any layer in it — drawn once, after all the artwork, over whatever the
    // artwork turned out to be. That is also why it is here rather than inside
    // `draw_layer`: one filled circle per lamp per frame, however many shapes it
    // falls across.
    // **The dark first, then the light.**
    //
    // Both belong to the frame rather than to any layer in it, and the order
    // between them is the whole point of having both: a pool laid over the
    // gloom cuts a hole through it, which is what a lamp in a dark room does.
    // The other way round the gloom would fall on the pool and the lamp would
    // be dimmed by the very darkness it is supposed to be beating.
    draw_gloom_bands(builder, scene, frame, camera, options, &lights);
    draw_light_pools(builder, scene, frame, camera, options, &lights);
}

/// Draw each gloom's wall of dark over the finished artwork.
///
/// See [`buzz_light::gloom_band`] for what one is and why the darkness is drawn
/// rather than folded into the artwork's colours. Nothing here is cached, for
/// the same reason a pool is not: it is a quad and a gradient, rebuilt each
/// frame for the cost of neither, which is what lets it follow a wall being
/// dragged.
///
/// Gated on `pools` along with the lamps' pools, and for the identical reason —
/// this is one statement about the *frame*, and Edit Multiple Frames would
/// otherwise stack six copies of the same darkness on one picture.
fn draw_gloom_bands(
    builder: &mut SceneBuilder<'_>,
    scene: &Scene,
    frame: u32,
    camera: Affine,
    options: &FrameOptions,
    lights: &buzz_scene::LightRig,
) {
    if !options.lit || !options.pools || !lights.is_active() {
        return;
    }

    // At the focal plane, like the stage itself and like a pool: a wall of dark
    // is not artwork on a layer and has no depth of its own to be moved by.
    let projection = scene
        .camera_projection_at_depth(frame, 0.0)
        .unwrap_or_else(|| Projection::from_affine(camera))
        .pre_affine(options.place);

    for light in lights.lights.iter().filter(|l| l.enabled) {
        let Some(band) = buzz_light::gloom_band(light) else {
            continue;
        };

        // The stops are already the colours to multiply by — opaque, ending at
        // white — so the alpha channel carries nothing and the blend is a plain
        // multiply. That is the same convention `Illumination::as_filter` uses
        // for a light, which is what makes the two land on the same arithmetic.
        let stops: Vec<buzz_scene::GradientStop> = band
            .ramp
            .iter()
            .map(|(at, surviving)| buzz_scene::GradientStop::new(*at, *surviving))
            .collect();
        let mut gradient = buzz_scene::Gradient::new(buzz_scene::GradientKind::Linear, stops);
        gradient.transform = band.ramp_transform();
        let paint = buzz_scene::Paint::Gradient(Arc::new(gradient));

        let quad = band.quad();
        let drawn = projection.map_path(&quad, builder.tolerance());
        // `SrcAtop`, so the dark lands on the picture and nowhere else. A gloom
        // stood off the stage reaches across it, and source-over would paint
        // its quad straight onto the transparency outside the frame.
        builder.fill_shape_atop_paint(
            &drawn,
            &paint,
            brush_projection(&projection, quad.bounding_box()),
            buzz_fx::Blend::Multiply,
        );
    }
}

/// Draw each lamp's pool of light over the finished artwork.
///
/// See [`buzz_light::light_pool`] for what a pool is and why a lamp needs one.
/// Nothing here is cached: a pool is a gradient and a circle, rebuilt each frame
/// for the cost of neither, which is what lets it follow a lamp being dragged.
fn draw_light_pools(
    builder: &mut SceneBuilder<'_>,
    scene: &Scene,
    frame: u32,
    camera: Affine,
    options: &FrameOptions,
    lights: &buzz_scene::LightRig,
) {
    if !options.lit || !options.pools || !lights.is_active() {
        return;
    }

    // At the focal plane, like the stage itself: a pool is not artwork on a
    // layer and has no depth of its own to be moved by.
    let projection = scene
        .camera_projection_at_depth(frame, 0.0)
        .unwrap_or_else(|| Projection::from_affine(camera))
        .pre_affine(options.place);

    for light in lights.lights.iter().filter(|l| l.enabled) {
        let Some(pool) = buzz_light::light_pool(light, 0.0) else {
            continue;
        };

        // The ramp, as the lamp's own colour fading to nothing. Alpha carries
        // how much light arrives, so the colour stays the light's at every
        // radius and only its presence changes — which is what keeps a warm
        // lamp warm all the way out instead of drifting towards grey.
        let stops: Vec<buzz_scene::GradientStop> = pool
            .ramp
            .iter()
            .map(|(at, arriving)| {
                buzz_scene::GradientStop::new(*at, light.color.multiply_alpha(*arriving))
            })
            .collect();
        let mut gradient = buzz_scene::Gradient::new(buzz_scene::GradientKind::Radial, stops);
        let disc = buzz_geom::Circle::new(pool.centre, pool.reach);
        gradient.fit_to(disc.bounding_box());
        let paint = buzz_scene::Paint::Gradient(Arc::new(gradient));

        let path = disc.to_path(builder.tolerance());
        let drawn = projection.map_path(&path, builder.tolerance());
        builder.fill_shape_paint_lit(
            &drawn,
            &paint,
            brush_projection(&projection, disc.bounding_box()),
        );
    }
}

/// A mask group being drawn, and how it is closed.
struct OpenMask {
    mask: buzz_scene::LayerId,
    /// The mask's geometry, kept only for an inverted mask: it is punched out
    /// of the group when the group closes.
    punch: Option<buzz_geom::BezPath>,
}

/// Per-side growth for [`grow_mask_path`], in device pixels — half the
/// clip's stroke width (the width itself is `2.0 * MASK_GROW_PX`).
///
/// Half a device pixel, the plan's first guess, is not enough: the masked
/// artwork's own edge is not at the bare geometric boundary. Every opaque
/// fill is sealed unconditionally (`docs/RENDER_SEAMS_PLAN.md` Step 1), which
/// already grows that edge `crate::SEAM_SEAL_PX / 2` outward on its own
/// (`fill_shape_paint_sealed`'s own doc), and the growth here has to clear
/// *that* — sealed edge plus an ordinary antialiasing kernel's width — not
/// the unsealed one the plan's diagnosis assumed. Chosen by measuring
/// `a_mask_the_same_size_as_its_artwork_shows_no_light_ring` (a red circle
/// masked by a black one exactly the same size — the adversarial case) at
/// three candidates built on that same half pixel:
///
/// | candidate | value | masked ring lightness | passes (≤ 178.2)? |
/// |---|---|---|---|
/// | `SEAM_SEAL_PX / 2 + 0.5` | 0.95 | 161.3 | **yes** |
///
/// The smallest candidate already passed, comfortably (the reference — an
/// ordinary singly antialiased 50%-coverage edge — is 170.2; 161.3 is
/// *darker*, i.e. more covered, than that), so the other two candidates were
/// never needed.
const MASK_GROW_PX: f64 = crate::SEAM_SEAL_PX / 2.0 + 0.5;

/// Grow a mask path by [`MASK_GROW_PX`] on each side, so the masked
/// artwork's own edge — usually drawn on the very same curve as the mask, an
/// eye white cut to the eye shape — does not have to fight the clip's
/// antialiasing for the rim pixel. Two antialiased edges on the same curve
/// multiply their coverage (0.5 × 0.5), so without this the rim reads a
/// quarter covered and the backdrop shows through: a white ring round the
/// eye.
///
/// Built the way [`append_silhouette`] merges a shape's fill with its stroke
/// outline: the path plus its own outline, stroked [`MASK_GROW_PX`]`* 2`
/// device pixels wide, both wound the same way ([`buzz_geom::wound_forward`])
/// so the non-zero fill rule unions them without a boolean pass. The outline
/// extends the filled region half its width to each side, so this grows the
/// mask by exactly [`MASK_GROW_PX`]. In practice the mask's own outline (the
/// eye line) is drawn above it and covers the sliver of masked artwork this
/// reveals beyond the mask, which is also why Animate's own masks show no
/// seam.
///
/// See `docs/RENDER_SEAMS_PLAN.md` Step 2.
fn grow_mask_path(path: &buzz_geom::BezPath, builder: &SceneBuilder<'_>) -> buzz_geom::BezPath {
    let view_scale = builder.view_scale().max(f64::MIN_POSITIVE);
    // **Never more than a small share of the mask itself**, in step with the
    // fill's own seal (`crate::seal_width_px`): a mask a few pixels tall — an
    // eye on a character standing far off — grown by the full amount lets the
    // masked eyeball spill a pixel past the eye on every side. The growth
    // exists to match the seal of the artwork drawn on the same curve, so it
    // shrinks exactly as that seal does, and vanishes when that seal does.
    let seal_share = {
        use buzz_geom::Shape as _;
        let bounds = path.control_box();
        let extent_px = bounds.width().max(bounds.height()) * view_scale;
        // The exact measure only for a mask small enough to need it.
        match crate::seal_width_px_lazy(extent_px, || {
            let area = path.area().abs() * view_scale * view_scale;
            let length = path.perimeter(builder.tolerance()) * view_scale;
            if length > 0.0 { 2.0 * area / length } else { 0.0 }
        }) {
            Some(seal) => seal / crate::SEAM_SEAL_PX,
            None => return path.clone(),
        }
    };
    let width = 2.0 * MASK_GROW_PX * seal_share / view_scale;
    let mut grown = buzz_geom::BezPath::new();
    for element in buzz_geom::wound_forward(path).elements() {
        grown.push(*element);
    }
    let outline = buzz_geom::outline_stroke(
        path,
        buzz_geom::StrokeStyle::new(width),
        builder.tolerance(),
    );
    for element in buzz_geom::wound_forward(&outline).elements() {
        grown.push(*element);
    }
    grown
}

/// Begin a mask group, whichever way round it works.
///
/// An ordinary mask is a clip and costs nothing to bound. An inverted one is a
/// group that gets a hole punched in it, and a group is a render target — so
/// it has to be bounded by what the masked layers actually draw. When they
/// draw nothing there is nothing to hide, and the group is skipped entirely.
fn open_mask(
    builder: &mut SceneBuilder<'_>,
    layers: &buzz_scene::LayerStack,
    mask: buzz_scene::LayerId,
    masks: &std::collections::BTreeMap<buzz_scene::LayerId, buzz_scene::LayerId>,
    at: impl buzz_scene::AtTime,
    path: buzz_geom::BezPath,
) -> Option<OpenMask> {
    let inverted = layers.get(mask).is_some_and(|l| l.kind.is_inverted_mask());
    if !inverted {
        let grown = grow_mask_path(&path, builder);
        builder.push_clip(&grown);
        return Some(OpenMask { mask, punch: None });
    }

    let covered = masks
        .iter()
        .filter(|(_, m)| **m == mask)
        .filter_map(|(id, _)| layers.get(*id))
        .filter_map(|l| l.bounds_at(at.frame()))
        .reduce(|a, b| a.union(b))?;

    // A margin, because a stroke is drawn about its path and a filter reaches
    // past the artwork it blurs; a group cut exactly to the bounds would clip
    // both off at the edge.
    builder.push_inverse_clip(covered.inflate(64.0, 64.0));
    // The same growth applies to the punch: it is the exact-path clip's
    // inverse, so the seam it would otherwise open is the same seam.
    let grown = grow_mask_path(&path, builder);
    Some(OpenMask {
        mask,
        punch: Some(grown),
    })
}

fn close_mask(builder: &mut SceneBuilder<'_>, open: Option<OpenMask>) {
    match open {
        Some(OpenMask {
            punch: Some(path), ..
        }) => builder.pop_inverse_clip(&path),
        Some(OpenMask { punch: None, .. }) => builder.pop_isolation(),
        None => {}
    }
}

/// Every layer that is currently clipped, and by which mask.
fn active_masks(
    layers: &buzz_scene::LayerStack,
    display: MaskDisplay,
) -> std::collections::BTreeMap<buzz_scene::LayerId, buzz_scene::LayerId> {
    let mut out = std::collections::BTreeMap::new();
    for group in layers.mask_groups() {
        let in_force = match display {
            MaskDisplay::Always => true,
            MaskDisplay::WhenLocked => layers.get(group.mask).is_some_and(|l| l.locked),
        };
        if !in_force {
            continue;
        }
        for masked in group.masked {
            out.insert(masked, group.mask);
        }
    }
    out
}

/// The mask layer's artwork, as one path in render space.
///
/// Every shape it holds contributes, filled — Animate masks with the mask
/// layer's *shapes*, ignoring their colour, and a mask made of three separate
/// blobs shows through all three. Strokes are ignored for the same reason
/// Animate ignores them: a mask is a region, and a stroke is a line.
///
/// # Symbols count
///
/// A mask layer holding a symbol instance is the ordinary case, not the odd
/// one: every Animate character on this machine masks its eyeballs with an
/// instance of the eye-white symbol, because the eye white *is* a symbol and
/// the animator dragged it onto the mask layer. `Object::flatten` skips
/// instances — it has no library to resolve them with — so those masks came
/// out empty, no clip was opened, and the eyeballs were drawn whole over the
/// face. [`mask_shapes`] goes through the library.
fn mask_geometry(
    scene: &Scene,
    layers: &buzz_scene::LayerStack,
    mask: buzz_scene::LayerId,
    at: impl buzz_scene::AtTime,
    place: Affine,
    projection: &Projection,
    tolerance: f64,
) -> Option<buzz_geom::BezPath> {
    let layer = layers.get(mask)?;
    let mut combined = buzz_geom::BezPath::new();
    // Built in document space and projected once, like everything else: a mask
    // on a tilted layer has to be foreshortened by exactly the same lens as the
    // artwork it clips, or it would clip the wrong region.
    let place = place * layers.inherited_transform(mask, at);
    let frame = at.frame();
    let elapsed = elapsed_on(layer, frame);

    let mut flat = Vec::new();
    for object in layers.resolved_at_rigged(mask, at).iter() {
        mask_shapes(scene, object, elapsed, place, 0, &mut flat);
    }
    for (transform, shape) in flat {
        if shape.fill.is_none() {
            continue;
        }
        for element in (transform * shape.path).elements() {
            combined.push(*element);
        }
    }

    if combined.elements().is_empty() {
        return None;
    }
    let mapped = projection.map_path(&combined, tolerance);
    (!mapped.elements().is_empty()).then_some(mapped)
}

/// How long the keyframe governing `frame` has been on screen — what a graphic
/// instance on the layer counts its own playhead from.
fn elapsed_on(layer: &buzz_scene::Layer, frame: u32) -> u32 {
    frame
        - layer
            .frames
            .keyframe_at(frame)
            .map(|k| k.start)
            .unwrap_or(0)
            .min(frame)
}

/// Every filled shape an object on a mask layer stands for, **instances
/// included** — the one difference from `Object::flatten`.
///
/// An instance is opened at the frame it would be drawn at, and each layer
/// inside it contributes its artwork where that layer's own parenting puts it.
/// A mask layer *inside* the instance is a stencil there and not a region here,
/// and a guide is never drawn anywhere, so both are passed over.
fn mask_shapes(
    scene: &Scene,
    object: &Object,
    elapsed: u32,
    parent: Affine,
    depth: usize,
    out: &mut Vec<(Affine, buzz_scene::ShapeData)>,
) {
    if !object.visible {
        return;
    }
    let world = parent * object.transform;
    match &object.kind {
        ObjectKind::Group(children) => {
            for child in children {
                mask_shapes(scene, child, elapsed, world, depth, out);
            }
        }
        ObjectKind::Instance(instance) => {
            if depth >= MAX_SYMBOL_DEPTH {
                return;
            }
            let Some(symbol) = scene.library().get(instance.symbol) else {
                return;
            };
            let inner = instance.resolve_frame(symbol.kind, elapsed, symbol.length());
            for layer in symbol.layers.drawable_at(inner) {
                if layer.kind.is_mask() || layer.kind == LayerKind::Guide {
                    continue;
                }
                let follows = symbol.layers.inherited_transform(layer.id, inner);
                let inner_elapsed = elapsed_on(layer, inner);
                for child in symbol.layers.resolved_at_rigged(layer.id, inner).iter() {
                    mask_shapes(scene, child, inner_elapsed, world * follows, depth + 1, out);
                }
            }
        }
        // Shapes, warps and posed rigs flatten as they always did.
        _ => object.flatten(parent, out),
    }
}

/// **One layer rig's cast shadow**, drawn once for the whole rig (LS).
///
/// A layer rig is a character cut into a layer per part, each following the
/// part it hangs from. Lit a layer at a time, every part threw its own shadow
/// onto the parts drawn before it — grey patches all over the body — where the
/// same character as one armature throws one. So the rig's parts are gathered
/// here and their union throws a single shadow, from the root's depth, drawn
/// beneath the bottom-most part; no part ever shadows another.
struct RigShadow<'a> {
    /// The root layer: the key the rig's shadow is remembered under.
    root: buzz_scene::LayerId,
    /// The root's depth, which the shadow is thrown from.
    depth: f64,
    /// The parts drawn this pass, in paint order. The first is the bottom-most.
    members: Vec<&'a Arc<buzz_scene::Layer>>,
}

/// How a layer's cast shadow is drawn.
#[derive(Clone, Copy)]
enum ShadowRole<'a> {
    /// Its own shadow pass, as every layer outside a layer rig.
    Own,
    /// A part of a layer rig whose shadow is drawn by the rig: none of its own.
    Member,
    /// The bottom-most part of a layer rig: draws the whole rig's shadow.
    Rig(&'a RigShadow<'a>),
}

/// Can this layer be part of a rig's one shadow? Masks are stencils and
/// guides are reference: they keep whatever they had.
fn casts_with_rig(layer: &buzz_scene::Layer) -> bool {
    !layer.kind.is_mask() && layer.kind != LayerKind::Guide
}

/// Every layer rig in the pass, with its parts in the order they are drawn,
/// keyed by root; and the layer-to-root map they were gathered by (see
/// [`buzz_scene::LayerStack::follow_roots`]). Both empty when nothing in the
/// stack follows anything.
#[allow(clippy::type_complexity)]
fn rig_shadow_groups<'a>(
    layers: &buzz_scene::LayerStack,
    ordered: &[&'a Arc<buzz_scene::Layer>],
) -> (
    std::collections::HashMap<buzz_scene::LayerId, buzz_scene::LayerId>,
    std::collections::HashMap<buzz_scene::LayerId, RigShadow<'a>>,
) {
    let roots = layers.follow_roots();
    let mut groups: std::collections::HashMap<buzz_scene::LayerId, RigShadow<'a>> =
        std::collections::HashMap::new();
    if roots.is_empty() {
        return (roots, groups);
    }
    for layer in ordered.iter().copied() {
        let Some(root) = roots.get(&layer.id).copied() else {
            continue;
        };
        if !casts_with_rig(layer) {
            continue;
        }
        groups
            .entry(root)
            .or_insert_with(|| RigShadow {
                root,
                depth: layers.get(root).map(|l| l.depth).unwrap_or(layer.depth),
                members: Vec::new(),
            })
            .members
            .push(layer);
    }
    (roots, groups)
}

/// Which [`ShadowRole`] `layer` draws its shadow in.
fn shadow_role<'a>(
    roots: &std::collections::HashMap<buzz_scene::LayerId, buzz_scene::LayerId>,
    groups: &'a std::collections::HashMap<buzz_scene::LayerId, RigShadow<'a>>,
    layer: &buzz_scene::Layer,
) -> ShadowRole<'a> {
    let Some(group) = roots.get(&layer.id).and_then(|root| groups.get(root)) else {
        return ShadowRole::Own;
    };
    if !casts_with_rig(layer) {
        return ShadowRole::Own;
    }
    match group.members.first() {
        Some(first) if first.id == layer.id => ShadowRole::Rig(group),
        _ => ShadowRole::Member,
    }
}

/// Draw one layer's artwork.
#[allow(
    clippy::too_many_arguments,
    reason = "one call site; a struct would only move the arguments"
)]
fn draw_layer(
    builder: &mut SceneBuilder<'_>,
    scene: &Scene,
    // The stack this layer belongs to — needed to resolve its frame through
    // the rig-relative path below, which only the stack can do (it is what
    // knows a layer's parent and that parent's rest pose).
    layers: &buzz_scene::LayerStack,
    layer: &buzz_scene::Layer,
    frame: u32,
    camera: Affine,
    // What this layer inherits from the layer it follows — Animate's Layer
    // Parenting. Identity for a layer that follows nothing, which is why a
    // document that uses none of this draws exactly as it did before.
    follows: Affine,
    options: &FrameOptions,
    // The light rig resolved at this frame, shared across every layer of the
    // pass. Resolved once by [`draw_layers`] rather than cloned per layer — on a
    // document with hundreds of layers that clone was pure per-frame waste.
    lights: &Arc<buzz_scene::LightRig>,
    cache: &mut DrawCache,
    // Whether this layer throws its own shadow, none (a layer rig's part), or
    // its whole layer rig's (the rig's bottom-most part). See [`RigShadow`].
    role: ShadowRole<'_>,
) {
    {
        // How this layer is projected onto the frame.
        //
        // The instant this layer is drawn at — the frame, or part-way into it
        // while a shutter is open. Everything below that varies *within* a
        // frame reads it; everything keyed to whole frames (the keyframe
        // lookup, a symbol's own timeline, a spring's integration) keeps taking
        // `frame`, because those genuinely do not move between two of them.
        let time = options.at(frame);

        // Layer depth draws artwork further from the camera smaller, and slides
        // it less as the camera pans; a **tilted** camera also foreshortens it,
        // turning the layer's rectangle into a trapezoid. Both fall out of the
        // same projection. A layer at or behind the camera, or turned past
        // edge-on, is skipped rather than drawn inside out.
        //
        // `camera` arrives already resolved for depth zero and is used as-is
        // when the shot is flat — which keeps every untilted document rendering
        // through exactly the affine it always did, down to the bit.
        let projection = if layer.depth == 0.0 && !scene.camera_has_tilt() {
            Projection::from_affine(camera)
        } else {
            match scene.camera_projection_at_depth(time, layer.depth) {
                Some(projection) => projection,
                None => return,
            }
        };

        // **Layer transparency rides the onion-skin road.** Both are the same
        // thing to the renderer — one alpha multiplying every colour on the
        // way out — so a dimmed layer needs no second mechanism, and a dimmed
        // layer *seen as a ghost* is dimmed once by each rather than by
        // whichever happened to be checked last.
        //
        // `layer_alpha` is off in the export's options, so this fades the
        // working view and never the film.
        let alpha = if options.layer_alpha {
            layer.alpha.clamp(0.0, 1.0)
        } else {
            1.0
        };
        let layer_alpha = match (options.ghost, alpha < 1.0) {
            (Some(ghost), true) => Some(ghost * alpha),
            (Some(ghost), false) => Some(ghost),
            (None, true) => Some(alpha),
            (None, false) => None,
        };

        // Guides are authoring aids: visible on stage, never exported.
        if layer.kind == LayerKind::Guide && !options.guides {
            return;
        }
        let outline = layer.outline || (options.ghost.is_some() && options.ghost_outlines);
        let tint = outline.then_some(layer.color);
        let faded = layer.kind == LayerKind::Guide;

        // Resolve tweens: what is drawn between two keyframes exists nowhere
        // in the document and has to be interpolated.
        // How this layer is lit. Worked out once here rather than per shape:
        // every shape on a layer shares its depth, and therefore its distance
        // from the light and the length of what it casts.
        // The rig, resolved at this frame by the caller (keyframed lights
        // contribute their state *now*). Static rigs resolve to themselves, so
        // the shading cache (keyed per light) keeps hitting.
        let rig: &buzz_scene::LightRig = lights;
        // A layer set to **Ignore lights** (captions, title cards) is drawn
        // exactly as the unlit render draws it: no shading, no crescents, no
        // rim, no cast shadow (F1).
        let lit = options.lit && rig.is_active() && tint.is_none() && !layer.unlit;
        let stage_height = scene.stage().size.height;
        // Whether this layer is lit at all is decided here; *how much* light
        // reaches each shape is worked out per shape, because a lamp's whole
        // character is that it varies across the stage. Deciding it once per
        // layer made two squares either side of a lamp exactly as bright as
        // each other, which is a sun pretending to be a lamp.
        let lighting = lit.then_some((stage_height, layer.depth));
        // Where this stack sits — identity unless a symbol is open in place —
        // and then layer parenting, both in the plane, before the lens.
        // A layer rig's one shadow is thrown in the plane, before any part's
        // parenting: each part brings its own `follows` into it.
        let placed = projection.pre_affine(options.place);
        let projection = placed.pre_affine(follows);

        // How this layer's shadows are thrown, if they are. Worked out once
        // here: it depends on the light and on how far this layer's artwork
        // stands above the surface catching it, and on nothing about any shape.
        // The cull below needs it and so does the shadow pass, and they must
        // agree — a caster culled away casts nothing.
        // Trimmed away at `LightDetail::Flat`, where nothing generated fits;
        // `None` here removes the shadow pass and the margin the cull below
        // grows for it in one go, exactly as an unlit layer does.
        //
        // A layer rig's part throws none of its own; its bottom-most part
        // throws the whole rig's, from the root's depth (LS).
        let shadow_depth = match role {
            ShadowRole::Rig(group) => group.depth,
            _ => layer.depth,
        };
        // A layer with **Cast shadow** off (set pieces) is still shaded and
        // tinted above; it only throws nothing. A layer rig follows its root.
        let casts_shadow = match role {
            ShadowRole::Rig(group) => layers.get(group.root).map_or(true, |root| root.casts_shadow),
            ShadowRole::Own => layer.casts_shadow,
            ShadowRole::Member => false,
        };
        let shadow = (lit && cache.detail().casts() && casts_shadow)
            .then(|| rig.key())
            .flatten()
            .and_then(|key| buzz_light::shadow_throw(key, scene.shadow_height(shadow_depth, key)));
        // The rig's parts as they stand now, for its one shadow: each with what
        // it inherits from the parts it follows and the instant into its own
        // keyframe.
        let rig_parts: Vec<RigPart<'_>> = match (role, &shadow) {
            (ShadowRole::Rig(group), Some(_)) => group
                .members
                .iter()
                .map(|member| RigPart {
                    follows: if member.id == layer.id {
                        follows
                    } else {
                        layers.inherited_transform(member.id, time)
                    },
                    resolved: layers.resolved_at_rigged(member.id, time),
                    elapsed: frame
                        - member
                            .frames
                            .keyframe_at(frame)
                            .map(|k| k.start)
                            .unwrap_or(0)
                            .min(frame),
                    layer: member,
                })
                .collect(),
            _ => Vec::new(),
        };

        // Culling is safe only where document-space bounds compare directly to
        // the viewport and nothing off-screen can reach into it: a flat camera,
        // a depth-zero layer (so no perspective moves the artwork), and no mask
        // (a mask needs its whole geometry). When any of these does not hold,
        // the layer draws everything, exactly as before.
        let cull = options
            .cull
            .filter(|_| {
                layer.depth == 0.0
                    && !scene.camera_has_tilt()
                    && !matches!(
                        layer.kind,
                        LayerKind::Mask | LayerKind::InverseMask | LayerKind::Masked
                    )
            })
            .map(|rect| {
                // **A key light casts shadows into view from artwork off the
                // edge, and the shadow itself says exactly how far off.**
                //
                // A caster whose shadow lands in `rect` is one for which
                // `shadow · caster` is in `rect`, so the casters that matter are
                // precisely those in `shadow⁻¹(rect)`. Taking that and the view
                // together is the smallest rectangle that can miss nothing.
                //
                // It used to be `rect` grown by its own longest side — nine
                // times the area, drawn every frame, whatever the light was
                // doing. A high sun throws a shadow a few units long and now
                // costs a few units of margin; a lamp *shrinks* the rectangle,
                // because its shadows splay outwards, so a lit lamp scene culls
                // to very nearly the view. That factor of nine, on every lit
                // frame of a document larger than the window, is most of what
                // made switching lighting on feel like switching responsiveness
                // off.
                match &shadow {
                    // A shadow on a **wall** is one affine for the whole
                    // layer, so the casters that matter are exactly those in
                    // its inverse image and nothing has to be guessed.
                    Some(buzz_light::ShadowThrow::Wall(shadow)) => {
                        rect.union(shadow.inverse().transform_rect_bbox(rect))
                    }
                    // A shadow on the **ground** is worked out per caster, so
                    // there is no one inverse to take. What bounds it instead
                    // is how far any caster on this layer could throw one:
                    // nothing here is taller than the layer's own artwork.
                    // A layer rig's shadow is as tall as the whole rig.
                    Some(throw) if !rig_parts.is_empty() => {
                        let tall = rig_parts
                            .iter()
                            .filter_map(|part| {
                                part.layer
                                    .bounds_at(frame)
                                    .map(|b| part.follows.transform_rect_bbox(b))
                            })
                            .reduce(|a, b| a.union(b))
                            .map(|b| b.height())
                            .unwrap_or(0.0);
                        let reach = throw.reach(tall);
                        rect.inflate(reach, reach)
                    }
                    Some(throw) => {
                        let tall = layer.bounds_at(frame).map(|b| b.height()).unwrap_or(0.0);
                        let reach = throw.reach(tall);
                        rect.inflate(reach, reach)
                    }
                    None => rect,
                }
            });

        let ctx = DrawCtx {
            // Set per object, where the layer's modifiers are evaluated.
            breath: None,
            scene,
            cull,
            stamp_view: options.stamp_view,
            lights: lights.clone(),
            frame,
            elapsed: frame
                - layer
                    .frames
                    .keyframe_at(frame)
                    .map(|k| k.start)
                    .unwrap_or(0)
                    .min(frame),
            tint,
            faded,
            ghost: layer_alpha,
            effect: ColorTransform::default(),
            adjust: None,
            gradient_map: None,
            blur: None,
            stage_frame: frame,
            synced: false,
            stage_size: scene.stage().size,
            depth: 0,
            lighting,
            light_print: if lit { rig.fingerprint() } else { 0 },
            // Per object below, where whether this artwork is its own figure
            // can be answered. This is the rig's answer for artwork that never
            // reaches that decision.
            shape_edges: lights.shape_edges(),
            layer_depth: layer.depth,
            projection,
            // No stamped child above this ctx yet; see `DrawCtx::stroke_scale`.
            stroke_scale: 1.0,
            in_stamp_build: false,
        };
        // Phase A3: a layer with a rig parent tweens in that parent's rest
        // frame rather than in the raw stored matrix, so a shearing or
        // non-uniformly scaled rig still turns as it was rigged. A layer with
        // no such parent resolves exactly as `resolved_at` always did.
        let resolved = layers.resolved_at_rigged(layer.id, time);

        // **Shadows first, and all of them, before any of this layer's
        // artwork.** A shadow falls on what is *behind* its caster: drawing
        // each one just before its own shape would let a character's shadow
        // land on the character standing next to it on the same layer, which
        // is never what a flat drawing means.
        //
        // The projection was worked out **once for the layer**, above. A layer
        // whose key light throws nothing — a sky, shadows switched off, a light
        // down on the surface — skips the whole pass here rather than
        // discovering it a shape at a time.
        //
        // **All of them into one group, and the tone put on the group.** A
        // shadow is the silhouette of what casts it, at one tone. Drawn a shape
        // at a time in black at the light's strength, the alphas compound
        // wherever two of a caster's shapes overlap — `1-(1-a)^n` — and a
        // character of a hundred shapes came out as a patchwork of a hundred
        // darknesses with every internal seam showing. Opaque inside a group
        // closed at the strength, the union is one tone however many shapes
        // made it. See `SceneBuilder::push_alpha_group`.
        // **A layer rig's one shadow** (LS): the bottom-most part draws the
        // union of every part's, thrown as one caster, before any part is
        // drawn — so no part lands in another's shadow.
        if let ShadowRole::Rig(group) = role
            && !rig_parts.is_empty()
            && let Some(shadow) = shadow
            && let Some(key) = rig.key()
        {
            let mut alpha = key.shadow_strength.clamp(0.0, 1.0);
            if ctx.faded {
                alpha *= FADE;
            }
            if let Some(ghost) = ctx.ghost {
                alpha *= ghost as f32;
            }
            draw_rig_shadow(
                builder,
                scene,
                group,
                &rig_parts,
                &shadow,
                key.shadow_strength,
                alpha,
                &ctx,
                placed,
                cache,
            );
        }
        if rig_parts.is_empty()
            && let Some(shadow) = shadow
            && let Some(key) = rig.key()
        {
            // The group carries everything that would have dimmed the fill:
            // the light's own shadow strength, a guide layer's fade, and a
            // ghost's alpha. Inside, every shadow is opaque.
            let mut alpha = key.shadow_strength.clamp(0.0, 1.0);
            if ctx.faded {
                alpha *= FADE;
            }
            if let Some(ghost) = ctx.ghost {
                alpha *= ghost as f32;
            }

            // **The whole pass, remembered** (L8 prime): while what is on the
            // layer, the light and the view hold still, the encoded pass is
            // appended instead of being built again. See [`ShadowMemo`] and
            // [`shadow_memo_signature`] for what "holds still" covers.
            let signature = if cache.shadow_memo.enabled {
                shadow_memo_signature(
                    scene,
                    layer,
                    &resolved,
                    &shadow,
                    key.shadow_strength,
                    alpha,
                    &ctx,
                    builder,
                    cache.detail(),
                    &mut cache.animated,
                )
            } else {
                None
            };
            let remembered = signature
                .as_ref()
                .and_then(|(hash, _)| cache.shadow_memo.get(layer.id, *hash));
            match (remembered, signature) {
                (Some((fragment, lines)), _) => {
                    builder.target_scene().append(&fragment, None);
                    builder.lines += lines;
                }
                (None, Some((hash, identity))) => {
                    // Encoded into a fragment through a builder with the
                    // frame's own view - the same split, clip and output scale,
                    // so its bytes are the ones the frame would have been
                    // given - then appended, and kept. A pass that draws
                    // nothing (no shadow in view) is kept too: empty.
                    let mut fragment = crate::vello::Scene::new();
                    let lines = {
                        let mut child = SceneBuilder {
                            scene: &mut fragment,
                            split: builder.split,
                            visible: builder.visible,
                            clip: builder.clip,
                            lines: 0,
                            device_scale: builder.device_scale,
                            last_stats: PathLineStats::default(),
                            runs: Vec::new(),
                            spare_runs: Vec::new(),
                        };
                        if let Some(cast) =
                            shadow_group_bounds(&resolved, &shadow, &ctx, &mut cache.bounds)
                        {
                            run_shadow_pass(
                                &mut child,
                                &resolved,
                                &shadow,
                                cast,
                                alpha,
                                &ctx,
                                &mut cache.bounds,
                            );
                        }
                        child.lines
                    };
                    builder.target_scene().append(&fragment, None);
                    builder.lines += lines;
                    cache.shadow_memo.insert(
                        layer.id,
                        hash,
                        Arc::new(fragment),
                        lines,
                        identity,
                        scene.library().clone(),
                    );
                }
                (None, None) => {
                    if let Some(cast) =
                        shadow_group_bounds(&resolved, &shadow, &ctx, &mut cache.bounds)
                    {
                        run_shadow_pass(
                            builder,
                            &resolved,
                            &shadow,
                            cast,
                            alpha,
                            &ctx,
                            &mut cache.bounds,
                        );
                    }
                }
            }
        }

        // A layer holding build-up paint is drawn into its own transparent
        // group. Additive compositing sums with the destination, and without
        // the group the destination would include the stage — a dark stroke on
        // a white background would sum to white and disappear. Inside, the sum
        // starts from nothing and means what it should.
        //
        // The group is skipped entirely when nothing on the layer is additive,
        // because every group is a render target and they are not free.
        let accumulates = resolved
            .iter()
            .any(|object| has_additive_paint(object, &cache.symbols, 0));

        if accumulates {
            builder.push_isolation(builder.clip_bounds());
        }

        // **The rim the key light lays around this layer's artwork**, if it
        // lays one. See `buzz_light::rim_glow`: a glow outside the silhouette,
        // in the light's colour and at the light's strength, so the edges come
        // up when the light does.
        //
        // Per *layer* rather than per shape, and from one union silhouette: a
        // character is a hundred shapes and glowing each of them would rim
        // every internal seam — the arm's edge against the body it is drawn
        // over — which reads as a drawing coming apart rather than as light.
        // The same reasoning, and the same shape, as the one cast shadow a
        // layer throws.
        //
        // Trimmed away with the crescents at `LightDetail::Fill`: a rim is
        // modelling, it costs a silhouette and a stack of strokes, and a frame
        // that will not fit the rasteriser has to give something up.
        let rim = (lit && cache.detail().models())
            .then(|| rig.key())
            .flatten()
            .and_then(|key| {
                let at = layer.bounds_at(frame).map(|b| b.center())?;
                buzz_light::rim_glow(key, at, layer.depth)
            });

        // Filters on the layer itself, which Animate does not have: the
        // whole layer is one subject, so a blurred background layer is one
        // effect rather than one per object on it.
        //
        // The rim is built through exactly this path, from a `Glow` with the
        // light's colour and reach, because a rim *is* Animate's Glow filter
        // laid by a light instead of by hand — same geometry, same bands, same
        // drawing code. Building it as a second mechanism would be two things
        // to keep looking alike.
        let needs_silhouette = !layer.filters.is_empty() || rim.is_some();
        let layer_fx: Option<Arc<buzz_fx::Painted>> = if needs_silhouette {
            // **Remembered between frames.** The silhouette gather and the
            // filter build over it are the whole cost of the rim on a
            // character stage, and they are a pure function of what is on the
            // layer: the same objects in the same poses, the same rim, the
            // same filters give the same ops. So the layer is asked for a
            // signature of exactly those (`layer_fx_signature`), and a layer
            // that has not changed skips the gather and the build outright.
            // `None` from the signature means something on the layer has no
            // identity to key on (freshly tweened artwork): build, don't keep.
            let evals: Vec<Option<buzz_scene::modifier::ModifierEval>> = resolved
                .iter()
                .map(|object| scene.modified_object_at(layer.id, object, time))
                .collect();
            let elapsed = layer
                .frames
                .keyframe_at(frame)
                .map(|k| time - k.start.min(frame) as f64)
                .unwrap_or(time);
            let signature = if cache.layer_fx.enabled {
                layer_fx_signature(
                    scene,
                    layer,
                    &resolved,
                    &evals,
                    elapsed,
                    ctx.stage_frame,
                    rim.as_ref(),
                    &mut cache.animated,
                )
            } else {
                None
            };
            let cached = signature
                .as_ref()
                .and_then(|(hash, _)| cache.layer_fx.get(layer.id, *hash));
            match cached {
                Some(hit) => Some(hit),
                None => {
                    let mut silhouette = buzz_geom::BezPath::new();
                    for (object, eval) in resolved.iter().zip(&evals) {
                        // The silhouette must be made from the same evaluated object
                        // that the paint pass draws. In particular, spring poses and
                        // wiggle/breath modifiers otherwise leave a character's
                        // shadow at its stored pose (the usual symptom is legs in a
                        // different position from the character).
                        match eval {
                            Some(eval) => append_silhouette(
                                eval.object.as_ref().unwrap_or(object),
                                eval.prepend,
                                Some(scene),
                                elapsed,
                                &mut silhouette,
                                0,
                                eval.breath.as_ref(),
                                ctx.stage_frame,
                                false,
                                Some(&mut cache.silhouette),
                            ),
                            None => append_silhouette(
                                object,
                                Affine::IDENTITY,
                                Some(scene),
                                elapsed,
                                &mut silhouette,
                                0,
                                None,
                                ctx.stage_frame,
                                false,
                                Some(&mut cache.silhouette),
                            ),
                        }
                    }
                    let mut filters: Vec<buzz_fx::Filter> =
                        Vec::with_capacity(layer.filters.len() + 1);
                    if let Some(rim) = rim {
                        filters.push(buzz_fx::Filter::new(buzz_fx::FilterKind::Glow {
                            x: rim.reach,
                            y: rim.reach,
                            // The strength is already in the colour's alpha, so that a
                            // rim which has fallen off across the stage arrives fainter
                            // rather than narrower.
                            strength: 1.0,
                            color: rim.color,
                            // Outside the line: what a hand-drawn rim is, and the only
                            // thing lighting does here that can be brighter than the
                            // picture around it.
                            inner: false,
                            knockout: false,
                            quality: buzz_fx::Quality::default(),
                        }));
                    }
                    // The layer's own filters last, so one set by hand still sits over
                    // the rim rather than under it.
                    filters.extend(layer.filters.iter().cloned());
                    let fx = Arc::new(buzz_fx::build(&filters, &silhouette));
                    if let Some((hash, identity)) = signature {
                        cache.layer_fx.insert(
                            layer.id,
                            hash,
                            Arc::clone(&fx),
                            identity,
                            scene.library().clone(),
                        );
                    }
                    Some(fx)
                }
            }
        } else {
            None
        };

        if let Some(fx) = &layer_fx {
            crate::filters::draw_ops(
                builder,
                &fx.behind,
                &ctx.projection,
                ctx.ghost.unwrap_or(1.0),
            );
        }

        let layer_ctx = match layer_fx.as_ref() {
            Some(fx) if fx.adjust.is_some() || fx.gradient_map.is_some() => DrawCtx {
                adjust: fx.adjust.or(ctx.adjust),
                gradient_map: fx.gradient_map.or(ctx.gradient_map),
                effect: layer.color_effect().compose(&ctx.effect),
                ..ctx.clone()
            },
            _ => DrawCtx {
                effect: layer.color_effect().compose(&ctx.effect),
                ..ctx.clone()
            },
        };

        let isolated_layer = layer.blend != buzz_scene::Blend::Normal;
        if isolated_layer {
            builder.push_blend(builder.clip_bounds(), layer.blend);
        }

        // Depth of field: a layer off the focus plane is blurred in proportion
        // to how far out of focus it is. The **geometric** approximation — it
        // reuses the per-shape blur the filter path already draws, so it costs
        // no new pipeline. `None` for a pinhole camera or a layer in focus, so
        // a document that sets no aperture is untouched.
        //
        // Resolved at `frame`, so a **focus pull** — focus keyed to travel from
        // one depth to another — softens this layer as the shot goes on.
        let dof_blur = scene.camera().dof_blur_at(time, layer.depth);

        if !layer_fx.as_ref().is_some_and(|fx| fx.hide_subject) {
            // A layer blur applies to every shape on the layer, and depth of
            // field adds to it — the wider of the two wins, so a blurred
            // background layer thrown out of focus is not blurred twice. The
            // same for every object here, so it is worked out once.
            let blur = combine_blur(layer_fx.as_ref().and_then(|fx| fx.blur), dof_blur);

            // **Everything placed before anything is drawn.**
            //
            // A figure may be several of the layer's objects — see
            // [`figure_groups`] — and which ones cannot be known until they
            // have all been placed and measured.
            let mut placed: Vec<Placed<'_>> = Vec::new();
            for (object, owner) in resolved.iter_owned() {
                // S7 (stop latency): placing a layer's objects (measuring each) can
                // take milliseconds on a crowded layer; an aborted pass stops here too.
                if cache.is_aborted() {
                    break;
                }
                // E7: a pre-build worker takes every `n`th object of the frame
                // (counted across layers), so the heaviest layer is shared too.
                if let Some((k, n)) = cache.work_slice {
                    let c = cache.slice_counter;
                    cache.slice_counter += 1;
                    if c % n != k {
                        continue;
                    }
                }
                // Live modifiers (a spring, a wiggle) are evaluated here — the
                // one place the window, the exporter and the headless tests all
                // pass through, so what is drawn is what is exported. Almost
                // every object has none and takes the cheap `None` path
                // unchanged.
                //
                // A spring re-poses the rig into an owned copy (no `Arc`
                // identity); a wiggle only prepends a transform and keeps the
                // original, so its symbol/bounds caches still hit.
                let (held, owner, prepend, breath) =
                    match scene.modified_object_at(layer.id, object, time) {
                        None => (None, owner, Affine::IDENTITY, None),
                        Some(eval) => {
                            let owner = if eval.object.is_some() { None } else { owner };
                            (eval.object, owner, eval.prepend, eval.breath)
                        }
                    };
                // Resolved through the library, because a character is usually
                // an instance and an instance's own bounds are a placeholder.
                let bounds = match owner {
                    Some(owner) => cache.bounds.resolved(owner, ctx.scene),
                    None => ctx.scene.resolved_bounds(held.as_ref().unwrap_or(object)),
                };
                placed.push(Placed {
                    held,
                    stored: object,
                    owner,
                    prepend,
                    breath,
                    region: buzz_scene::object::transform_rect(prepend, bounds),
                });
            }

            let groups = match layer_ctx.lights.edges {
                // Only the figure mode has any grouping to do; the others read
                // each object on its own and must not pay to find out that they
                // overlap.
                buzz_scene::EdgeMode::Figure => figure_groups(&placed, &layer_ctx),
                _ => (0..placed.len()).map(|i| vec![i]).collect(),
            };

            // One seal run for the layer's artwork: see
            // `SceneBuilder::begin_seal_run` and [`wants_seal_run`]. Skipped
            // for a layer of nothing but symbols, as `draw_symbol_contents`
            // skips one.
            let seals = placed.iter().any(|p| draws_own_art(p.object()))
                && wants_seal_run(
                    builder,
                    placed
                        .iter()
                        .map(|p| p.region)
                        .reduce(|a, b| a.union(b))
                        .unwrap_or(buzz_geom::Rect::ZERO),
                    Affine::IDENTITY,
                    &ctx.projection,
                );
            if seals {
                builder.begin_seal_run();
            }
            for group in groups {
                // S7 (stop latency): a pass nobody wants any more ends within an
                // object, not only at a layer boundary (one layer can be 40 ms).
                if cache.is_aborted() {
                    break;
                }
                let members: Vec<&Placed<'_>> = group.iter().map(|i| &placed[*i]).collect();

                // **Is this object its own figure already?**
                //
                // A lone shape's silhouette *is* its outline, so the two modes
                // agree on the picture — and the shape path draws the bands the
                // cheap way, recoloured into the fill, where the figure pass
                // must composite them over artwork whose colours it cannot
                // know. So a layer of four hundred props keeps the encode it
                // always had, and only artwork actually made of pieces pays for
                // being treated as one.
                //
                // A shape that overlaps another one is *not* alone, whatever it
                // is made of: the two of them are one body, and one body takes
                // one set of bands.
                let alone = members.len() == 1
                    && matches!(
                        members[0].object().kind,
                        ObjectKind::Shape(_) | ObjectKind::Warp(_)
                    );

                let mut object_ctx = layer_ctx.clone();
                object_ctx.blur = blur;
                object_ctx.shape_edges = match object_ctx.lights.edges {
                    buzz_scene::EdgeMode::Off => 0.0,
                    buzz_scene::EdgeMode::Shapes => object_ctx.lights.shape_edges(),
                    buzz_scene::EdgeMode::Figure if alone => object_ctx.lights.figure_edges(),
                    buzz_scene::EdgeMode::Figure => 0.0,
                };

                // **The light on the figure**, when the rig models figures
                // rather than shapes: the bands are measured against the
                // silhouette of everything the figure draws, so they go round
                // the character rather than round each piece of it. Opened
                // before the draw because the silhouette is gathered *from* the
                // draw, and closed after, because the bands lie over it. See
                // [`FigurePass`].
                //
                // Here, at the layer's own objects, rather than inside
                // `draw_object`: this is exactly the set of things that are one
                // figure. Everything below is a part of one.
                let figure = (!alone)
                    .then(|| FigurePass::open_over(&object_ctx, cache, &members))
                    .flatten();
                for member in &members {
                    if cache.is_aborted() {
                        break;
                    }
                    // A breath belongs to the one object that breathes, so it
                    // is set here and not on the group: two characters in one
                    // figure breathe at their own rates, as they should.
                    let member_ctx = match member.breath {
                        None => object_ctx.clone(),
                        breath => DrawCtx {
                            breath,
                            ..object_ctx.clone()
                        },
                    };
                    draw_object(
                        builder,
                        member.object(),
                        member.owner,
                        member.prepend,
                        &member_ctx,
                        cache,
                    );
                }
                if let Some(figure) = figure {
                    figure.close(builder, &object_ctx, cache);
                }
            }
            if seals {
                builder.end_seal_run();
            }
        }

        if let Some(fx) = &layer_fx {
            crate::filters::draw_ops(builder, &fx.over, &ctx.projection, ctx.ghost.unwrap_or(1.0));
        }

        if accumulates {
            builder.pop_isolation();
        }

        if isolated_layer {
            builder.pop_isolation();
        }
    }
}

/// Does this object, or anything inside it, paint additively?
///
/// The shallow stage-object tree — shapes, groups, rigs — is walked live. An
/// instance instead consults the [`SymbolTable`] memo, whose `additive` flag
/// already folds the symbol's whole subtree across every frame: without that,
/// this was a full recursive walk of the library for every stage object, every
/// frame — the O(document) cost the memo exists to kill.
fn has_additive_paint(object: &Object, symbols: &SymbolTable, depth: usize) -> bool {
    if depth >= MAX_SYMBOL_DEPTH {
        return false;
    }
    match &object.kind {
        ObjectKind::Shape(shape) => shape.blend.is_additive(),
        ObjectKind::Group(children) => children
            .iter()
            .any(|child| has_additive_paint(child, symbols, depth + 1)),
        ObjectKind::Warp(warp) => warp.shape.blend.is_additive(),
        ObjectKind::Armature(rig) => rig
            .parts
            .iter()
            .any(|part| has_additive_paint(&part.artwork, symbols, depth + 1)),
        ObjectKind::Instance(instance) => symbols
            .get(instance.symbol)
            .is_some_and(|info| info.flags.additive),
    }
}

/// Do two rectangles overlap, edges included? Inclusive on purpose: a shape
/// exactly touching the viewport edge is kept, never culled.
fn rects_overlap(a: buzz_geom::Rect, b: buzz_geom::Rect) -> bool {
    a.x0 <= b.x1 && a.x1 >= b.x0 && a.y0 <= b.y1 && a.y1 >= b.y0
}

/// Fold a depth-of-field blur into a layer's filter blur, taking the wider of
/// the two so a background layer that is both filter-blurred and out of focus is
/// blurred once rather than twice.
fn combine_blur(
    filter: Option<(f64, f64, buzz_fx::Quality)>,
    dof: Option<f64>,
) -> Option<(f64, f64, buzz_fx::Quality)> {
    match (filter, dof) {
        (Some((rx, ry, q)), Some(d)) => Some((rx.max(d), ry.max(d), q)),
        (Some(blur), None) => Some(blur),
        (None, Some(d)) => Some((d, d, buzz_fx::Quality::Medium)),
        (None, None) => None,
    }
}

/// State the draw walk carries down through groups and nested symbols.
///
/// Descending into an instance changes nearly all of it — the frame, the
/// colour effect, the depth — so it travels as one value rather than as a
/// growing argument list.
#[derive(Clone)]
struct DrawCtx<'a> {
    scene: &'a Scene,
    /// The visible rectangle in document space, **when it is safe to cull to
    /// it**. `Some` only on a flat, depth-zero, unmasked, unshadowed layer whose
    /// objects' document-space bounds compare directly to the viewport; `None`
    /// everywhere culling could hide something that projects, bleeds or shadows
    /// into view. Set per layer by [`draw_layer`]. When in doubt it is `None` and
    /// everything draws — culling too little only costs time, culling too much is
    /// a wrong picture.
    cull: Option<buzz_geom::Rect>,
    /// [`FrameOptions::stamp_view`], carried down unchanged from the layer
    /// this artwork is on. `None` means "use `cull`" — every caller, band or
    /// not — so [`try_stamp_symbol`]'s off-screen judgement is always against
    /// this pass's own visible rectangle. See H0 item 1 in
    /// `docs/IMPORT_PERF_PLAN.md`.
    stamp_view: Option<buzz_geom::Rect>,
    /// The light rig **resolved at the stage frame** — every keyframed light
    /// evaluated to concrete values. Read instead of `scene.lights()` so a
    /// keyframed sun lights and shadows at the frame being drawn, while a static
    /// rig resolves to the same values and keeps the shading cache warm. An
    /// `Arc` so the nested contexts of symbol drawing share it by a pointer bump.
    lights: Arc<buzz_scene::LightRig>,
    /// Which frame of *this* timeline is being drawn. A nested graphic symbol
    /// runs on its own frame number, not the stage's.
    frame: u32,
    /// How long the keyframe holding this artwork has been on screen.
    ///
    /// This, not `frame`, is what a graphic instance plays against: a symbol
    /// placed at frame 300 starts at *its* first frame, not three hundred
    /// frames into a cycle. Reading the timeline's own frame number instead
    /// puts every instance at an arbitrary point in its loop — which looks
    /// like animation, and is why it survived so long.
    elapsed: u32,
    /// Outline view: draw silhouettes in this colour instead of the artwork.
    tint: Option<Color>,
    /// Guide layer.
    faded: bool,
    /// Onion-skin ghost alpha.
    ghost: Option<f64>,
    /// Accumulated colour effect from every enclosing instance.
    effect: ColorTransform,
    /// Adjust Color from a filter on this object or its layer, applied to
    /// every colour inside it.
    adjust: Option<buzz_fx::ColorAdjust>,
    /// A duotone gradient map from a filter on this object or its layer,
    /// recolouring every colour inside it by brightness.
    gradient_map: Option<buzz_fx::GradientMap>,
    /// A blur inherited from a filter, applied to each shape as it is drawn.
    blur: Option<(f64, f64, buzz_fx::Quality)>,
    depth: usize,
    /// The frame the **stage** is on.
    ///
    /// Distinct from `frame`, which is the timeline being drawn — a graphic
    /// symbol runs on its own. The camera belongs to the document, so it is
    /// always read at this one; a symbol on its fourth frame must not be seen
    /// through the camera's fourth.
    stage_frame: u32,
    /// **Inside an instance locked to the root timeline.** Set once a
    /// [`buzz_scene::SymbolInstance::sync_to_root`] instance is entered and
    /// stays set for everything nested below it, so the whole subtree resolves
    /// its playhead to `stage_frame` rather than counting elapsed frames from
    /// where each instance was placed. This is what lines a character's lip
    /// sync up with the root dialogue at every level.
    synced: bool,
    /// The layer's own size, for the camera's arithmetic.
    stage_size: buzz_geom::Size,
    /// Whether this layer is lit, and what the lights need to know about it:
    /// the stage's height, for the sky's gradient, and the layer's depth.
    ///
    /// How much light reaches each *shape* is worked out per shape, because a
    /// lamp varies across the stage. Carried down through groups and nested
    /// symbols, so a shape inside a character inside a scene is lit by the
    /// same sun as everything else.
    lighting: Option<(f64, f64)>,
    /// `LightRig::fingerprint()` of [`Self::lights`], taken once per layer
    /// (`0` when the layer is unlit): the part of a lit stamp's [`SymKey`] that
    /// names the light, so a stamp is not hashed per instance.
    light_print: u64,
    /// **How strongly this artwork models its own edges**, which is the rig's
    /// modelling strength where the light draws shape by shape and zero where a
    /// [`FigurePass`] around the object is going to draw them instead.
    ///
    /// Set per object by [`draw_layer`], because the answer is a property of
    /// the object rather than of the rig: under [`buzz_scene::EdgeMode::Figure`]
    /// an object that *is* one shape is its own figure, and drawing its bands
    /// here — recoloured into the fill, one pass — is both the same picture and
    /// cheaper than compositing them over it afterwards.
    shape_edges: f32,
    /// **The breath deforming the artwork being drawn**, if it breathes.
    ///
    /// Carried on the context rather than folded into a transform because a
    /// breath is not one: the legs do not move, the ribs swell and the head
    /// rides on them, which is three behaviours over three bands of the body.
    /// See [`buzz_scene::Breath`].
    ///
    /// Here rather than in the modifier because the artwork a breath has to
    /// reach is usually inside a symbol, and the renderer is what expands one.
    breath: Option<buzz_scene::Breath>,
    /// The depth of the layer being drawn, which sets how far a lamp is from
    /// it and how long its shadow runs.
    layer_depth: f64,
    /// How this layer's plane lands on the frame: depth, tilt, the camera's
    /// pan and zoom, and layer parenting, all in one.
    ///
    /// **Everything is drawn through this and nothing else.** Geometry is
    /// accumulated in *document* space on the way down — an object's placement,
    /// a group's, a symbol's — and projected once at the leaf. Carrying a
    /// pre-multiplied "world" transform alongside was what let lighting
    /// geometry be drawn through the object's own placement twice, and it
    /// cannot happen now: there is one transform, and it is applied in one
    /// place.
    projection: Projection,
    /// **A multiplier on every stroke's drawn width**, `1.0` everywhere today.
    ///
    /// A drawn stroke's render width is `stroke.width ×
    /// stroke_placement_scale(doc) × builder.split.scale`: the stroke scales
    /// with the placement that carries it, as Animate's `scaleMode="normal"`
    /// strokes do. `try_stamp_symbol` encodes its cached child at `scale ×
    /// s_q` with the instance's own placement left out of `doc`, and stamps it
    /// with the residual `r = A / s_q`, whose scale Vello applies to the
    /// stroke on append — `s_q × |r|` is the instance's own scale *including
    /// the projection's* (camera zoom, layer depth), which a live stroke never
    /// carries. So the child's value is `stroke_scale / ps`, `ps` the
    /// projection's scale (S13a; see `try_stamp_symbol`): the stamped width
    /// then equals the live one.
    ///
    /// It used to be `ctx.stroke_scale / s_q` in a stamped child, when a live
    /// stroke ignored its placement's scale altogether (a 2-unit outline
    /// stayed 2 px wide in a symbol shrunk to a tenth — barish1's eyes went
    /// black at stage size); dividing `s_q` out kept the stamp in step with
    /// that. Kept as a field, and in `SymKey`, so a future stroke mode that
    /// does need compensating has one place to put it.
    stroke_scale: f64,
    /// **Inside the build of a symbol stamp** (E3 in `docs/BIMARI5_LAG_PLAN.md`).
    /// Set in the neutral context [`try_stamp_symbol`] builds a child with, and
    /// carried to everything nested below it. While set, a nested miss always
    /// builds — the second-sighting rule is skipped — so a stamp's bytes depend
    /// only on its [`SymKey`], whoever builds it and whatever was already cached.
    in_stamp_build: bool,
}

impl DrawCtx<'_> {
    /// Final colour for a fill or stroke, with every modifier applied.
    ///
    /// Instance effects come first because they are part of the artwork; the
    /// guide fade and ghost alpha are authoring overlays applied on top.
    fn colour(&self, c: Color) -> Color {
        let adjusted = match self.adjust {
            Some(adjust) => adjust.apply(c),
            None => c,
        };
        // The gradient map recolours by brightness, so it comes after any
        // brightness/contrast adjustment has settled what "bright" means.
        let mapped = match self.gradient_map {
            Some(map) => map.apply(adjusted),
            None => adjusted,
        };
        self.overlay(self.effect.apply(mapped))
    }

    /// Put document-space geometry where the lens says it goes.
    ///
    /// The single place a projection is applied. With no tilt it is an affine
    /// and the path keeps its curves; with tilt the path is flattened and
    /// mapped, and anything behind the camera is clipped away.
    fn project(&self, path: &buzz_geom::BezPath, tolerance: f64) -> buzz_geom::BezPath {
        self.projection.map_path(path, tolerance)
    }

    /// The path with this object's breath in it, if it breathes.
    ///
    /// A no-op — and no copy — for everything that does not, which is every
    /// object in almost every document.
    fn breathe(&self, path: buzz_geom::BezPath) -> buzz_geom::BezPath {
        match &self.breath {
            Some(breath) => breath.path(&path),
            None => path,
        }
    }

    /// Apply only the authoring overlays.
    ///
    /// Outline view draws in the layer's identifying colour, which is chrome
    /// rather than artwork — running it through an instance's tint would
    /// defeat the point of colour-coding layers.
    fn overlay(&self, c: Color) -> Color {
        let c = if self.faded { fade(c) } else { c };
        match self.ghost {
            Some(alpha) => c.multiply_alpha(alpha as f32),
            None => c,
        }
    }

    /// The same as [`Self::colour`], for a paint that may be a gradient.
    ///
    /// A colour effect, a filter's Adjust Color and the ghost alpha are all
    /// functions of one colour, so a gradient takes them stop by stop. That is
    /// what makes a tinted instance of a gradient-filled symbol tint the whole
    /// ramp rather than flatten it.
    fn paint(&self, paint: &buzz_scene::Paint) -> buzz_scene::Paint {
        // A picture's colours are its pixels, which `map_colors` leaves alone
        // (see `Paint::map_colors`). Its instance effect — Animate's
        // brightness, tint, alpha — is applied to a recoloured copy instead,
        // made once per picture and effect: barish1's night trees are dimmed
        // this way, and drew in full daylight without it.
        if let buzz_scene::Paint::Image(fill) = paint
            && !self.effect.is_identity()
        {
            return buzz_scene::Paint::Image(Box::new(recoloured_image(fill, &self.effect)));
        }
        paint.map_colors(|c| self.colour(c))
    }

    /// The projection expressed as an affine, for placing a brush.
    ///
    /// A brush transform is a matrix; a tilted camera is a projection, and the
    /// two are only the same thing when there is no tilt. Where the projection
    /// *is* affine — which is every document that has not tilted its camera —
    /// this is exact and free.
    ///
    /// Where it is not, an affine is fitted to three corners of the shape's own
    /// bounding box mapped through the real projection. That is exact at those
    /// three points and close between them, which for a ramp across one shape
    /// is not a visible difference; the alternative is dropping to a flat
    /// colour, which is. Recorded as a deviation in PROGRESS.md §7.
    fn brush_projection(&self, bounds: buzz_geom::Rect) -> Affine {
        brush_projection(&self.projection, bounds)
    }
}

/// A picture with a colour effect applied to its pixels —
/// `channel · multiply + add`, per channel, as Animate's colour effects do.
/// Cached by picture and effect (bounded), so a dimmed tree is recoloured
/// once, not every frame.
fn recoloured_image(fill: &buzz_scene::ImageFill, effect: &buzz_scene::ColorTransform) -> buzz_scene::ImageFill {
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex, OnceLock};
    type Key = (u64, [u32; 8]);
    static CACHE: OnceLock<Mutex<HashMap<Key, Arc<buzz_scene::ImageAsset>>>> = OnceLock::new();
    let asset = &fill.asset;
    let mut bits = [0u32; 8];
    for i in 0..4 {
        bits[i] = effect.multiply[i].to_bits();
        bits[4 + i] = effect.add[i].to_bits();
    }
    let key = (asset.blob_id(), bits);
    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    let hit = cache.lock().ok().and_then(|c| c.get(&key).cloned());
    let recoloured = hit.unwrap_or_else(|| {
        let mut pixels = asset.pixels.as_ref().clone();
        for px in pixels.chunks_exact_mut(4) {
            for i in 0..4 {
                let v = f32::from(px[i]) * effect.multiply[i] + effect.add[i] * 255.0;
                px[i] = v.round().clamp(0.0, 255.0) as u8;
            }
        }
        let made = Arc::new(buzz_scene::ImageAsset::from_pixels(
            asset.id,
            asset.name.clone(),
            asset.width,
            asset.height,
            Arc::new(pixels),
        ));
        if let Ok(mut c) = cache.lock() {
            // Bounded: a long film cycles through many effects on few
            // pictures, and nothing here is worth keeping past that.
            if c.len() >= 256 {
                c.clear();
            }
            c.insert(key, Arc::clone(&made));
        }
        made
    });
    let mut out = fill.clone();
    out.asset = recoloured;
    out
}

/// **The light on a whole figure**, rather than on each shape it is drawn from.
///
/// # What this fixes
///
/// A crescent is artwork minus a copy of itself shifted towards the light, and
/// until this existed the artwork in that sentence was always *one shape*. On a
/// drawing made of one shape that is exactly right. On a character — a group, a
/// rig, a symbol, a hundred shapes — it is a hundred separate figures: every
/// interior patch of shading picks up its own lit side and its own terminator,
/// so the drawing reads as a pile of separately outlined pieces instead of one
/// body turned towards the lamp, and stops looking like itself. That is the
/// report this comes from, and [`buzz_scene::EdgeMode`] is the choice it left
/// behind.
///
/// Here the bands are measured against the silhouette of **everything the
/// object draws**, taken from the draw itself: the pass switches the cache's
/// collector on, every filled path that goes by lands in it, and the crescents
/// are built from the lot. Nothing has to walk the object a second time, and
/// nothing can disagree with what was actually drawn — a posed rig, a tween, a
/// symbol on its own playhead, all of it arrives the same way.
///
/// # Cheaper, not dearer
///
/// One pair of booleans for a character instead of one pair per shape. The
/// silhouette itself is *concatenated*, not unioned — filled non-zero,
/// overlapping subpaths merge, which is what the booleans downstream read it as
/// anyway — so gathering it costs a path copy per shape and no boolean at all,
/// and only on the frames where something is actually built.
///
/// # No isolation group
///
/// The bands compose `SrcAtop`, which needs the backdrop under them to be the
/// figure. Every band is a crescent *inside* the silhouette, and the silhouette
/// is where the figure's own artwork is, so the backdrop there is that artwork —
/// no group required, which is the difference between one render target per lit
/// character per frame and none. Where a figure is semi-transparent the band
/// modulates what shows through it as well; that is the same trade the shape
/// path makes for an opaque fill, and it is worth a great deal more than it
/// costs.
struct FigurePass<'a> {
    owner: Option<&'a Arc<Object>>,
    /// What the artwork is made of, when the object's address does not say —
    /// see [`LightCache::ready`].
    content: u64,
    /// The object's placement, which is what the cache is keyed on.
    doc: Affine,
    /// Where the figure stands, in document space.
    ///
    /// The light over it is worked out from this at [`close`](Self::close) and
    /// not before: on a stage of four hundred props most figures have no band
    /// to draw on any one frame — the geometry is still being built — and
    /// measuring a lamp's falloff across each of them anyway was most of what
    /// the pass cost.
    region: buzz_geom::Rect,
    /// What the layer's lighting says about depth and the stage's height, kept
    /// for that measurement.
    lighting: (f64, f64),
    /// The key light, the direction it lies in, and — under
    /// [`buzz_scene::ShadeModel::Form`] — how square-on it strikes, `lz`.
    /// `None` under the Crescent model, which does not read it; see
    /// [`buzz_light::crescent_aim`].
    key: Option<(&'a buzz_light::Light, buzz_geom::Vec2, Option<f64>)>,
    /// The deepest gloom reaching the figure, the way its darkness arrives, and
    /// how deep it is here.
    gloom: Option<(&'a buzz_light::Light, buzz_geom::Vec2, f32)>,
    /// Geometry already in the cache, if it was there before the draw. When it
    /// is, no silhouette is collected at all.
    key_built: Option<Arc<buzz_light::ShadeGeometry>>,
    gloom_built: Option<Arc<buzz_light::ShadeGeometry>>,
    modelling: f32,
}

/// One top-level object of a layer, placed and measured, waiting to be drawn.
///
/// The layer's objects are all resolved before any of them is drawn, because a
/// figure may be several of them and which ones cannot be known until they have
/// all been placed. See [`figure_groups`].
struct Placed<'a> {
    /// A live modifier's re-posed copy, kept alive for as long as it is drawn
    /// from. `None` when the object was drawn as it is stored.
    held: Option<Object>,
    stored: &'a Object,
    owner: Option<&'a Arc<Object>>,
    prepend: Affine,
    /// The breath deforming this object's artwork, if it breathes.
    breath: Option<buzz_scene::Breath>,
    /// Where it lands in document space, resolved through the library.
    region: buzz_geom::Rect,
}

impl<'a> Placed<'a> {
    /// The object as it is actually drawn — the modifier's copy if there is one.
    fn object(&self) -> &Object {
        self.held.as_ref().unwrap_or(self.stored)
    }

    /// Its placement, which is what the crescent cache is keyed on.
    fn doc(&self) -> Affine {
        self.prepend * self.object().transform
    }

    /// Is this thing the size of the stage? A backdrop is not part of the
    /// figure standing in front of it, however much of it that figure covers.
    /// The same test [`FigurePass::open_over`] uses to refuse it an edge, and
    /// for the same reason.
    fn is_backdrop(&self, ctx: &DrawCtx<'_>) -> bool {
        let stage = ctx.scene.stage().stage_rect();
        self.region.width() >= stage.width() && self.region.height() >= stage.height()
    }
}

/// **Which of a layer's objects are one figure**, as groups of indices into
/// `placed`, in paint order.
///
/// # Why a layer is not simply one figure, nor one figure per object
///
/// `EdgeMode::Figure` measures a light's bands against the silhouette of the
/// figure so they go round the body rather than round each piece of it. It did
/// that per top-level object — which is right for a character drawn as a group,
/// a rig or a symbol, and wrong for one drawn as a limb per object on a layer.
/// There each limb was its own figure, so every place two limbs overlapped fell
/// between two sets of bands and came out as a pale seam across the joint.
///
/// A layer is not one figure either: that is a layer of separate props, which
/// is exactly the case the per-object rule got right and which the `Shapes`
/// mode is kept for.
///
/// **Objects that overlap are one figure.** A body's pieces touch — that is
/// what makes them a body — and props standing apart on a stage do not. A
/// backdrop is excluded however much it covers, because a figure in front of a
/// wall is not part of the wall.
///
/// # Cost
///
/// A sweep along x with an active list, not every pair against every other: a
/// layer of four hundred props is a stated case here, and it must not become
/// eighty thousand rectangle tests a frame. Objects are compared only while
/// their x ranges are still open, so a spread-out layer costs a sort and a walk.
///
/// And a figure rebuilds as a **unit** — move one of its pieces and its whole
/// silhouette is gathered again — so a group that swallowed a layer would put
/// that layer's entire geometry on every frame of a drag. See
/// [`MOST_PIECES_IN_A_FIGURE`].
fn figure_groups(placed: &[Placed<'_>], ctx: &DrawCtx<'_>) -> Vec<Vec<usize>> {
    let n = placed.len();
    if n <= 1 {
        return (0..n).map(|i| vec![i]).collect();
    }

    // Union-find over the objects, joined when two of them overlap.
    let mut parent: Vec<usize> = (0..n).collect();
    fn find(parent: &mut [usize], mut i: usize) -> usize {
        while parent[i] != i {
            parent[i] = parent[parent[i]];
            i = parent[i];
        }
        i
    }

    let joinable: Vec<bool> = placed
        .iter()
        .map(|p| p.region.width() > 0.0 && p.region.height() > 0.0 && !p.is_backdrop(ctx))
        .collect();

    let mut order: Vec<usize> = (0..n).filter(|i| joinable[*i]).collect();
    order.sort_by(|a, b| placed[*a].region.x0.total_cmp(&placed[*b].region.x0));

    let mut active: Vec<usize> = Vec::new();
    for i in order {
        let region = placed[i].region;
        // Anything whose right edge is behind this one's left edge cannot meet
        // it, nor anything further right, so it leaves the sweep for good.
        active.retain(|j| placed[*j].region.x1 >= region.x0);
        for &j in &active {
            let other = placed[j].region;
            if region.y0 <= other.y1 && other.y0 <= region.y1 {
                let (a, b) = (find(&mut parent, i), find(&mut parent, j));
                if a != b {
                    parent[a] = b;
                }
            }
        }
        active.push(i);
    }

    // Back into paint order: a figure is drawn where its first member was, and
    // its members in the order the layer stacks them.
    let mut groups: Vec<Vec<usize>> = Vec::new();
    let mut seen: std::collections::HashMap<usize, usize> = std::collections::HashMap::new();
    for i in 0..n {
        let root = find(&mut parent, i);
        match seen.get(&root) {
            Some(at) => groups[*at].push(i),
            None => {
                seen.insert(root, groups.len());
                groups.push(vec![i]);
            }
        }
    }

    // A run of pieces too long to be a body is a layer, and a layer is not a
    // figure. Broken back into one figure each, which is what it was before any
    // of this and what the per-object rule already got right.
    if groups.iter().any(|g| g.len() > MOST_PIECES_IN_A_FIGURE) {
        return groups
            .into_iter()
            .flat_map(|group| {
                if group.len() > MOST_PIECES_IN_A_FIGURE {
                    group.into_iter().map(|i| vec![i]).collect::<Vec<_>>()
                } else {
                    vec![group]
                }
            })
            .collect();
    }
    groups
}

/// **How many overlapping objects still read as one body.**
///
/// A figure is gathered and rebuilt as a unit: move one of its pieces and the
/// whole silhouette is collected again. That is the right trade for a character
/// — a body is a body, and it is a couple of dozen pieces — and the wrong one
/// for a layer of artwork that merely happens to touch. Six hundred blobs
/// scattered over a stage overlap their neighbours by chance, and joined into
/// one figure they put every path on the layer into every frame of a drag:
/// measured at 364 ms against a 12 ms warm frame, which is the hang this
/// number exists to prevent.
///
/// Two dozen with room to spare. A rigged biped is eleven parts; a generous
/// cut-out character with hair, hands and a prop or two is under twenty. Past
/// that, the pieces are a layer and get a figure each, exactly as they did
/// before overlapping meant anything — and artwork that really is one body in
/// more pieces than this can say so by being a group, which is one figure at
/// any size.
const MOST_PIECES_IN_A_FIGURE: usize = 32;

impl<'a> FigurePass<'a> {
    /// Decide whether this object is lit as a figure, and set up for it.
    ///
    /// `prepend` is whatever the caller is about to draw the object through —
    /// a live modifier's transform, or the identity — and the object's own
    /// transform is applied on top of it here, so the key moves when the figure
    /// moves.
    /// `members` is the figure: one object usually, and several where a layer's
    /// objects overlap and are therefore one body. See [`figure_groups`].
    ///
    /// The pass is keyed on the **first** member — its `Arc`, its placement —
    /// with every member's content and placement folded into the content hash,
    /// so a figure whose pieces move, or gains or loses one, rebuilds and one
    /// that is standing still does not.
    fn open_over(
        ctx: &'a DrawCtx<'a>,
        cache: &mut DrawCache,
        members: &[&Placed<'a>],
    ) -> Option<FigurePass<'a>> {
        let modelling = ctx.lights.figure_edges();
        let lead = *members.first()?;
        if modelling <= 0.01
            || !cache.detail().models()
            || !members.iter().any(|m| m.object().visible)
        {
            return None;
        }
        let (stage_height, depth) = ctx.lighting?;

        // Where the figure stands, and how much light is on it — everything it
        // is made of, taken together.
        let region = members
            .iter()
            .map(|m| m.region)
            .reduce(|a, b| a.union(b))
            .unwrap_or(lead.region);
        if !(region.width() > 0.0 && region.height() > 0.0) {
            return None;
        }
        let owner = lead.owner;

        let here = region.center();
        // **Which model the terminator comes from.** `Form` reads how
        // square-on the light stands (`crescent_aim`'s `lz`); `Crescent`
        // reads only the planar direction, exactly as before — see
        // `docs/HARD_LIGHT_PLAN.md`'s Stage A.
        let key = ctx.lights.key().and_then(|key| match ctx.lights.model {
            buzz_scene::ShadeModel::Form => buzz_light::crescent_aim(key, here, depth, modelling)
                .map(|aim| (key, aim.planar, Some(aim.lz))),
            buzz_scene::ShadeModel::Crescent => {
                buzz_light::crescent_direction(key, here, depth, modelling)
                    .map(|towards| (key, towards, None))
            }
        });

        // **No edge on a backdrop**, for the reason `draw_gloom_edge` gives: a
        // shape that runs past the sides of the frame has no edge in shot.
        let stage = ctx.scene.stage().stage_rect();
        let backdrop = region.width() >= stage.width() && region.height() >= stage.height();
        let gloom = (!backdrop)
            .then(|| {
                ctx.lights
                    .lights
                    .iter()
                    .filter_map(|light| buzz_light::gloom_at(light, here).map(|got| (light, got)))
                    .max_by(|a, b| a.1.1.total_cmp(&b.1.1))
            })
            .flatten()
            .map(|(light, (facing, deep))| (light, facing, deep));

        if key.is_none() && gloom.is_none() {
            return None;
        }

        let doc = lead.doc();
        // Every member's content **and** its placement: the cache is keyed on
        // the first of them, so anything that moves a later one has to move the
        // key too or the figure would keep the crescents of a pose it has left.
        let content = members.iter().fold(members.len() as u64, |acc, member| {
            let mut hash = acc.rotate_left(11).wrapping_mul(0x9E37_79B9_7F4A_7C15)
                ^ figure_content(member.object(), ctx, &mut cache.animated);
            for coefficient in member.doc().as_coeffs() {
                hash = hash.rotate_left(7) ^ (coefficient.to_bits() as u64);
            }
            // A breath deforms the artwork frame by frame without moving the
            // object, its `Arc` or its placement. With `elapsed` no longer
            // folded in for artwork that holds still, the breath has to be.
            if let Some(breath) = &member.breath {
                for number in [
                    breath.hips,
                    breath.shoulders,
                    breath.centre,
                    breath.rise,
                    breath.widen,
                ] {
                    hash = hash.rotate_left(7) ^ number.to_bits();
                }
            }
            hash
        });

        // What is already built. Only when *something* is missing does the
        // silhouette have to be gathered, which is what keeps a settled frame
        // from copying every path in the document.
        let key_built = key.and_then(|(light, towards, lz)| {
            cache.lights.ready(
                owner,
                LightCache::FIGURE,
                content,
                doc,
                towards,
                lz,
                light.softness,
            )
        });
        let gloom_built = gloom.and_then(|(light, facing, _)| {
            cache.lights.ready(
                owner,
                LightCache::FIGURE,
                content,
                doc,
                -facing,
                None,
                light.softness,
            )
        });
        if key.is_some() && key_built.is_none()
            || gloom.is_some() && gloom_built.is_none()
            // Tweened and posed artwork has no identity to cache against, so it
            // is gathered and built every frame — as its shapes' crescents were.
            || owner.is_none()
        {
            cache.lights.collect_silhouette();
        }

        Some(FigurePass {
            owner,
            content,
            doc,
            region,
            lighting: (stage_height, depth),
            key,
            gloom,
            key_built,
            gloom_built,
            modelling,
        })
    }

    /// Lay the bands over the figure that has just been drawn.
    fn close(self, builder: &mut SceneBuilder<'_>, ctx: &DrawCtx<'_>, cache: &mut DrawCache) {
        let silhouette = cache.lights.take_silhouette();

        // Whatever was cached, else built from the silhouette this draw
        // gathered, else nothing at all — which is the ordinary answer while a
        // light is being dragged and the geometry is still on its way.
        let build = |cache: &mut DrawCache,
                     had: Option<Arc<buzz_light::ShadeGeometry>>,
                     towards: buzz_geom::Vec2,
                     lz: Option<f64>,
                     softness: f64| {
            had.or_else(|| {
                silhouette.as_ref().map(|path| {
                    cache.lights.figure(
                        self.owner,
                        self.content,
                        path,
                        self.doc,
                        towards,
                        lz,
                        softness,
                    )
                })
            })
            .filter(|geometry| !geometry.is_empty())
        };

        let key = self.key.and_then(|(key, towards, lz)| {
            build(cache, self.key_built.clone(), towards, lz, key.softness)
                .map(|geometry| (key, towards, geometry))
        });
        let gloom = self.gloom.and_then(|(gloom, facing, deep)| {
            let alpha = (deep * self.modelling * 0.75).clamp(0.0, 1.0);
            (alpha > 0.02)
                .then(|| build(cache, self.gloom_built.clone(), -facing, None, gloom.softness))
                .flatten()
                .map(|geometry| (gloom, alpha, geometry))
        });
        if key.is_none() && gloom.is_none() {
            return;
        }

        // Only now is the light itself worked out. A lamp falling off across
        // the figure keeps its ramp, so the bands darken along the body as the
        // lamp does.
        let (stage_height, depth) = self.lighting;
        let field = ctx.lights.field(self.region, depth, stage_height);
        if field.is_neutral() {
            return;
        }
        let light = field.uniform();
        let lamp = field
            .disc()
            .map(|(centre, reach)| cache.lamp_paints(&field, centre, reach));

        if let Some((key, towards, geometry)) = key {
            let shade = geometry
                .shade
                .as_ref()
                .map(|s| ctx.project(s, builder.tolerance()));
            let highlight = geometry.highlight.as_ref().map(|h| {
                // The band's bounds in **document** space: the ramp runs along
                // the light's own direction, which is a fact about the scene
                // rather than about the lens.
                (ctx.project(h, builder.tolerance()), h.bounding_box())
            });
            lay_bands(
                builder,
                ctx,
                Bands {
                    shade: shade.as_ref(),
                    highlight: highlight.as_ref().map(|(path, band)| (path, *band)),
                    towards: Some(towards),
                    key: Some(key),
                    lamp: lamp.as_deref(),
                    light: &light,
                    modelling: self.modelling,
                },
            );
        }

        // The dark edge the deepest wall of gloom leaves, which is the other
        // half of what makes light read as light: the tint says how much light
        // there is, an edge says which way it is coming from. Its own aim, so
        // it shares nothing with the key light's entry.
        if let Some((gloom, alpha, geometry)) = gloom
            && let Some(edge) = &geometry.highlight
        {
            let drawn = ctx.project(edge, builder.tolerance());
            builder.fill_shape_atop(
                &drawn,
                gloom.color.multiply_alpha(alpha),
                buzz_fx::Blend::Multiply,
            );
        }
    }
}

/// **What a figure's artwork is made of**, beyond the object's own address.
///
/// An object's `Arc` is a complete answer for artwork that lives under it:
/// copy-on-write gives an edited group a new address, and the cache keys on
/// that. An **instance** does not — its artwork lives in the library and runs
/// on its own playhead, so a symbol being edited, or its next keyframe coming
/// round, changes the silhouette while the instance's address and placement sit
/// perfectly still.
///
/// So an object with an instance anywhere inside it folds in the library's
/// identity — and the frame it is playing **only if something animated is
/// inside it** ([`AnimatedMemo`]: an instance of a symbol with more than one
/// frame, at any depth). An instance of a one-frame symbol resolves to that
/// frame whatever the playhead says, so its silhouette cannot depend on
/// `elapsed`; folding `elapsed` in anyway rebuilt every figure whose parts are
/// symbol instances — every slimmed rig — on every frame of a held pose.
///
/// A **rig** is also keyed on its pose: every bone's numbers, every part's
/// artwork address and placement. For a rig stored in the document the object's
/// own `Arc` already says all of that, so this changes nothing there; it is what
/// stops a rig that is *not* behind a stored object from sharing a key with the
/// same rig in another pose. Everything that has no instance and is not a rig
/// answers zero and keys on its address alone, which is every shape and every
/// group of shapes — and is what keeps a settled document hitting the cache
/// frame after frame.
fn figure_content(object: &Object, ctx: &DrawCtx<'_>, animated: &mut AnimatedMemo) -> u64 {
    fn has_instance(object: &Object, depth: usize) -> bool {
        if depth >= MAX_SYMBOL_DEPTH {
            return true;
        }
        match &object.kind {
            ObjectKind::Instance(_) => true,
            ObjectKind::Group(children) => children.iter().any(|c| has_instance(c, depth + 1)),
            ObjectKind::Armature(rig) => rig
                .parts
                .iter()
                .any(|p| has_instance(&p.artwork, depth + 1)),
            ObjectKind::Shape(_) | ObjectKind::Warp(_) => false,
        }
    }
    fn mix(hash: u64, value: u64) -> u64 {
        (hash.rotate_left(13) ^ value).wrapping_mul(0x9E37_79B9_7F4A_7C15)
    }

    let mut hash = 0u64;
    if let ObjectKind::Armature(rig) = &object.kind {
        // The pose: every bone's numbers (not just the angle — a bone edited
        // in place moves the part without moving any angle), and every part's
        // artwork and where it sits. Cheap: no skin weights, no deformation.
        hash = mix(hash, rig.armature.len() as u64);
        for number in [rig.armature.root.x, rig.armature.root.y] {
            hash = mix(hash, number.to_bits());
        }
        for bone in &rig.armature.bones {
            hash = mix(hash, bone.parent.map_or(u64::MAX, |p| p as u64));
            for number in [
                bone.length,
                bone.rest_angle,
                bone.angle,
                bone.offset.x,
                bone.offset.y,
            ] {
                hash = mix(hash, number.to_bits());
            }
        }
        for part in &rig.parts {
            hash = mix(hash, Arc::as_ptr(&part.artwork) as usize as u64);
            for coefficient in part.artwork.transform.as_coeffs() {
                hash = mix(hash, coefficient.to_bits());
            }
            hash = mix(
                hash,
                match &part.binding {
                    buzz_scene::RigBinding::Rigid(bone) => *bone as u64,
                    buzz_scene::RigBinding::Skin(skin) => u64::MAX - skin.points.len() as u64,
                },
            );
        }
    }

    if !has_instance(object, 0) {
        return hash;
    }
    // The library's identity moves on any edit to any symbol, so it is always
    // part of the key for artwork that lives there.
    hash = mix(hash, ctx.scene.library().content_id() as u64);
    // `elapsed` is what a graphic instance plays against (and `stage_frame`
    // what a root-synced one does): only artwork with an animated symbol in it
    // can change its silhouette with either.
    if animated.object(ctx.scene, object, 0) {
        hash = mix(hash, ctx.elapsed as u64 + 1);
        hash = mix(hash, ctx.stage_frame as u64 + 1);
    }
    hash
}

/// **The dark edge one wall of dark leaves on one shape.**
///
/// The counterpart of the highlight: where the terminator says which side of a
/// figure the key light is *not* on, this says which side the darkness is
/// arriving from. A gloom without it is a wash — the figures standing in it go
/// down evenly and lose their form, which is the same complaint a flat tint
/// gets from a light.
///
/// # Why it goes through the crescent cache
///
/// It was drawn live first, as a clip and a punch: two fills and no geometry,
/// exact on the frame the wall moved. That is a lovely property and it cost
/// four encoded paths a shape — with the rim as well, five and a half times the
/// unlit encode over twelve hundred shapes, which `encode_cost` exists to
/// refuse and was right to. Lighting draws about one more outline per shape and
/// this is one more outline per shape.
///
/// Keyed on the gloom's own bearing, so it shares nothing with the key light's
/// entry and a wall being dragged does not throw away the terminators.
#[allow(clippy::too_many_arguments)]
fn draw_gloom_edge(
    builder: &mut SceneBuilder<'_>,
    ctx: &DrawCtx<'_>,
    cache: &mut DrawCache,
    owner: Option<&Arc<buzz_scene::Object>>,
    index: u16,
    placed: &buzz_geom::BezPath,
    doc: Affine,
    here: buzz_geom::Point,
) {
    use buzz_geom::Shape as _;

    // A gloom's edge is a crescent by another name — the same cache, the same
    // boolean — so it is given up with the rest of the modelling.
    if !cache.detail().models() {
        return;
    }
    let modelling = ctx.shape_edges;
    if modelling <= 0.01 {
        return;
    }

    // **No edge on a backdrop.**
    //
    // A shape that runs past the sides of the frame has no edge *in shot*: its
    // outline is the frame. A band along it is the edge of the paper going
    // dark, not a figure. Measured against the stage rather than the visible
    // rectangle, which is inflated for culling and moves with the zoom.
    let bounds = placed.bounding_box();
    let stage = ctx.scene.stage().stage_rect();
    if bounds.width() >= stage.width() && bounds.height() >= stage.height() {
        return;
    }

    // One gloom, the deepest here. Two walls overlapping on one edge would
    // darken it twice and read as dirt, which is the same reason the terminator
    // follows one key light rather than summing the rig.
    let Some((light, (facing, deep))) = ctx
        .lights
        .lights
        .iter()
        .filter_map(|light| buzz_light::gloom_at(light, here).map(|got| (light, got)))
        .max_by(|a, b| a.1.1.total_cmp(&b.1.1))
    else {
        return;
    };

    let alpha = (deep * modelling * 0.75).clamp(0.0, 1.0);
    if alpha <= 0.02 {
        return;
    }

    // The band belongs on the side the darkness arrives from, which is the side
    // the wall is on — so the direction handed to the cache is *back along* the
    // throw, and what is wanted from it is the near-side crescent.
    let geometry = cache
        .lights
        .crescents(owner, index, placed, doc, -facing, None, light.softness);
    let Some(edge) = &geometry.highlight else {
        return;
    };

    let drawn = ctx.project(edge, builder.tolerance());
    builder.fill_shape_atop(
        &drawn,
        light.color.multiply_alpha(alpha),
        buzz_fx::Blend::Multiply,
    );
}

/// [`DrawCtx::brush_projection`], for callers that have a projection and no
/// context — the light pools, which belong to the frame rather than to any one
/// layer's draw walk.
fn brush_projection(projection: &Projection, bounds: buzz_geom::Rect) -> Affine {
    if let Some(a) = projection.as_affine() {
        return a;
    }
    let (w, h) = (bounds.width(), bounds.height());
    // A shape with no extent in one axis gives no second point to fit
    // against, and dividing by it would put NaN into the matrix.
    if !(w.is_finite() && h.is_finite()) || w <= 0.0 || h <= 0.0 {
        return Affine::IDENTITY;
    }
    let o = bounds.origin();
    let corner = |p: buzz_geom::Point| projection.map_point(p);
    let (Some(p0), Some(px), Some(py)) = (
        corner(o),
        corner(buzz_geom::Point::new(o.x + w, o.y)),
        corner(buzz_geom::Point::new(o.x, o.y + h)),
    ) else {
        // Behind the camera. Nothing will be drawn anyway.
        return Affine::IDENTITY;
    };
    let cx = (px - p0) / w;
    let cy = (py - p0) / h;
    Affine::new([cx.x, cx.y, cy.x, cy.y, p0.x, p0.y])
}

/// Draw one object.
///
/// Two transforms travel together: `parent` carries the camera and is what
/// geometry is drawn through, while `doc` is the same accumulation **without**
/// the camera — document space, which is where the lights live. Deriving one
/// from the other would mean inverting the camera per shape, and a camera
/// scaled to nothing has no inverse.
fn draw_object(
    builder: &mut SceneBuilder<'_>,
    object: &Object,
    owner: Option<&Arc<Object>>,
    doc: Affine,
    ctx: &DrawCtx<'_>,
    cache: &mut DrawCache,
) {
    if !object.visible {
        return;
    }

    // **Off-screen shapes are skipped.** Only a bare shape — whose bounds are
    // exact, unlike an instance's placeholder — with no filter to bleed past its
    // edge, and only when `ctx.cull` is set (a safe layer, see `draw_layer`).
    // `doc` carries the full accumulation into document space at any nesting
    // depth, so the test is valid inside a symbol too; instances are never
    // culled by their own bounds, but each shape they contain is culled here.
    if let Some(cull) = ctx.cull
        && object.filters.is_empty()
    {
        // A bare shape has exact bounds; an instance (or a group) has a
        // placeholder, so its real extent is resolved through the library and
        // cached — culling an off-screen *character* skips its whole subtree,
        // which on an instance-heavy import is where the time goes. Only owned
        // objects (a layer's own, not tweened) are cached and culled; tweened
        // artwork changes every frame and is cheap to draw anyway.
        let bounds = match &object.kind {
            ObjectKind::Shape(_) => Some(object.bounds()),
            ObjectKind::Instance(_) | ObjectKind::Group(_) => {
                owner.map(|o| cache.bounds.resolved(o, ctx.scene))
            }
            _ => None,
        };
        if let Some(bounds) = bounds {
            // `world` is in **document** space (the geometry accumulation), but
            // the cull rectangle is the viewport in the camera's **shot** space.
            // Project the bounds the same way the artwork is projected before
            // comparing — with an animated camera the two spaces differ, and at
            // high zoom that difference exceeds the margin, which culled
            // on-screen artwork. When the bounds cannot be projected (edge-on or
            // behind the camera) nothing is culled.
            let world = buzz_scene::object::transform_rect(doc, bounds);
            if let Some(world) = ctx.projection.map_rect_bounds(world)
                && !rects_overlap(world, cull)
            {
                return;
            }
        }
    }

    // Filters and blend modes are the object's own, so they are applied here,
    // around everything it draws — including everything inside a group or a
    // symbol, which is what makes "blur this character" mean the character
    // rather than each of its pieces.
    if !object.filters.is_empty() || object.blend.needs_group() {
        draw_filtered(builder, object, owner, doc, ctx, cache);
        return;
    }

    draw_object_inner(builder, object, owner, doc, ctx, cache);
}

/// Draw an object that carries filters, a blend mode, or both.
///
/// Split out so the ordinary path — which is every object in almost every
/// document — stays a straight walk with nothing to skip past.
fn draw_filtered(
    builder: &mut SceneBuilder<'_>,
    object: &Object,
    owner: Option<&Arc<Object>>,
    doc: Affine,
    ctx: &DrawCtx<'_>,
    cache: &mut DrawCache,
) {
    // The subject's outline, in document space — the space the filter ops are
    // built in and projected from.
    let mut silhouette = buzz_geom::BezPath::new();
    append_silhouette(
        object,
        doc,
        Some(ctx.scene),
        ctx.elapsed as f64,
        &mut silhouette,
        ctx.depth,
        ctx.breath.as_ref(),
        ctx.stage_frame,
        ctx.synced,
        Some(&mut cache.silhouette),
    );
    let painted = buzz_fx::build(&object.filters, &silhouette);

    // A blend mode needs a group to blend against; so does anything the filter
    // paints outside the artwork, or the group would clip it away.
    let reach = object
        .filters
        .iter()
        .filter(|f| f.enabled)
        .map(|f| f.kind.reach())
        .fold(0.0f64, f64::max);
    let grouped = object.blend.needs_group();
    if grouped {
        let obj_bounds =
            buzz_scene::object::transform_rect(doc * object.transform, object.bounds())
                .inflate(reach + 2.0, reach + 2.0);
        let bounds = if obj_bounds.is_zero_area() {
            builder.clip_bounds()
        } else {
            obj_bounds.intersect(builder.clip_bounds())
        };
        builder.push_blend(bounds, object.blend);
    }

    let alpha = ctx.ghost.unwrap_or(1.0);
    crate::filters::draw_ops(builder, &painted.behind, &ctx.projection, alpha);

    if !painted.hide_subject {
        let inner = DrawCtx {
            adjust: painted.adjust.or(ctx.adjust),
            gradient_map: painted.gradient_map.or(ctx.gradient_map),
            blur: painted.blur.or(ctx.blur),
            ..ctx.clone()
        };
        draw_object_inner(builder, object, owner, doc, &inner, cache);
    }

    crate::filters::draw_ops(builder, &painted.over, &ctx.projection, alpha);

    if grouped {
        builder.pop_isolation();
    }
}

/// The artwork itself, with no filters around it.
fn draw_object_inner(
    builder: &mut SceneBuilder<'_>,
    object: &Object,
    owner: Option<&Arc<Object>>,
    doc: Affine,
    ctx: &DrawCtx<'_>,
    cache: &mut DrawCache,
) {
    let doc = doc * object.transform;

    // **An object may face its own way.** Rotated in space, it lies on a plane
    // of its own rather than in its layer's, so it is drawn through a
    // projection of its own — and so is everything inside it, which is what
    // makes turning a group turn the whole group rather than each piece about
    // its own middle.
    //
    // Everything below this point then proceeds exactly as before: the plane
    // it is drawn on has changed, and nothing else has.
    // **Which way round the object is, as the camera sees it.**
    //
    // Its own yaw *less the camera's*: a camera that has swung round to the far
    // side of a character sees its back without the character having turned at
    // all. With the shot facing straight on this is the object's own yaw, which
    // is what it has always been.
    let apparent_yaw = object.spatial.rotation_y
        - ctx
            .scene
            .camera()
            .state_at(ctx.stage_frame)
            .map(|s| s.yaw)
            .unwrap_or(0.0);

    // **Turned far enough round to be another drawing.** An animator does not
    // foreshorten a face to nothing to show its profile, they draw the profile;
    // so the nearest view in the turnaround is swapped in, and only the turn
    // *left over* after it foreshortens what is drawn. A back view on an object
    // facing exactly backwards therefore comes out square to the camera, and a
    // profile at ninety degrees is visible at all — the object's own plane is
    // edge-on there and has no width to draw in.
    if let Some((view, residual)) = object.turnaround.view_at(apparent_yaw) {
        let mut swapped = (**view).clone();
        swapped.spatial = buzz_scene::Spatial {
            rotation_y: residual,
            ..object.spatial
        };

        // **It turns about the front's transformation point, not its own.**
        //
        // The view is standing in for the front, so the two have to pivot on
        // the same physical spot or the drawing steps sideways at the moment
        // the swap happens — a head that turns about the neck on the way round
        // and about its own middle once it is there. The front's pivot is in the
        // front's coordinates; the view is placed relative to the front, so it
        // comes back through the view's own transform.
        let front_pivot = ctx.scene.pivot_local_of(object);
        swapped.pivot = buzz_scene::invert_affine(swapped.transform)
            .map(|inverse| inverse * front_pivot)
            .or(swapped.pivot);

        let swapped = Arc::new(swapped);
        draw_object(builder, &swapped, Some(&swapped), doc, ctx, cache);
        return;
    }

    let turned;
    let ctx = if object.spatial.is_flat() {
        ctx
    } else {
        // **Its transformation point**, not the middle of its box: a door
        // with its pivot on the hinge swings on the hinge.
        let pivot = doc * ctx.scene.pivot_local_of(object);
        let Some(projection) = ctx.scene.camera().projection_for_object(
            ctx.stage_frame,
            ctx.stage_size,
            ctx.layer_depth,
            pivot,
            &object.spatial,
        ) else {
            // Edge-on, or behind the camera: there is nothing to draw.
            return;
        };
        turned = DrawCtx {
            projection,
            ..ctx.clone()
        };
        &turned
    };

    match &object.kind {
        ObjectKind::Group(children) => {
            for child in children {
                draw_object(builder, child, Some(child), doc, ctx, cache);
            }
        }

        // Rigged artwork draws **posed**. The deformation happens here, at
        // draw time, from the angles the document actually stores — the same
        // arrangement tweens use, and for the same reason: a stored deformed
        // copy would be a second answer that could disagree with the first.
        ObjectKind::Armature(rig) => {
            for part in rig.posed() {
                // The posed parts are **memoised on the rig** (see
                // `ArmatureData::posed`): a held pose hands back the same
                // `Arc`s every frame, so they are an identity worth caching
                // against, exactly like any stored object. That only holds
                // when the rig itself is a stored object (`owner`); a tweened
                // or modifier-posed rig is a fresh object every frame, its
                // parts die with it, and keying a cache on them would only
                // defer builds that can never be hit — so those stay
                // owner-less and are built inline, as they always were.
                let part_owner = owner.map(|_| &part);
                draw_object(builder, &part, part_owner, doc, ctx, cache);
            }
        }

        ObjectKind::Warp(warp) => {
            draw_shape(builder, owner, 1, &warp.warped(), doc, ctx, cache);
        }

        ObjectKind::Instance(instance) => {
            if ctx.depth >= MAX_SYMBOL_DEPTH {
                return;
            }
            let Some(symbol) = ctx.scene.library().get(instance.symbol) else {
                // A dangling reference draws nothing rather than crashing; the
                // Library panel is where a missing symbol gets reported.
                return;
            };

            // Locked to the root, or inside something that is: the whole
            // subtree reads the stage's own frame. Otherwise a graphic follows
            // the parent playhead from where it was placed, and a movie clip
            // shows its first frame while authoring.
            let synced = ctx.synced || instance.sync_to_root;
            let inner = if synced {
                instance.resolve_to_root(ctx.stage_frame, symbol.length())
            } else {
                instance.resolve_frame(symbol.kind, ctx.elapsed, symbol.length())
            };

            let mut inner_ctx = ctx.clone();
            inner_ctx.frame = inner;
            inner_ctx.depth += 1;
            inner_ctx.synced = synced;
            // Compose rather than replace: a tinted symbol inside a faded one
            // must show both effects.
            inner_ctx.effect = instance.color.compose(&ctx.effect);

            // Reuse a cached encoding of the whole symbol when it is safe to; the
            // symbol is not cloned here, so `symbol` is still borrowed from the
            // scene, which is why the id and the reference both go in. The stamp
            // cache keys on `(symbol, inner)` and resolves the symbol's insides
            // the ordinary elapsed way, so a root-synced instance — whose insides
            // resolve to the stage frame instead — is drawn directly rather than
            // from a cache that never saw the sync.
            if !synced
                && try_stamp_symbol(
                    builder,
                    ctx,
                    &inner_ctx,
                    instance.symbol,
                    symbol,
                    inner,
                    doc,
                    cache,
                )
            {
                return;
            }

            draw_symbol_contents(builder, symbol, inner, doc, &inner_ctx, cache);
        }

        ObjectKind::Shape(shape) => draw_shape(builder, owner, 0, shape, doc, ctx, cache),
    }
}

/// Draw a symbol's own layer stack — the body of an instance, factored out so
/// that the live draw and the symbol-scene cache build walk it through the
/// *same* code and cannot drift apart.
///
/// `inner` is the symbol's own playhead (already resolved from the instance);
/// `doc` already includes the instance's placement; `inner_ctx` is the
/// instance's context (frame = `inner`, depth already advanced, colour effect
/// already composed). The old arm read `ctx.projection/tint/faded` here, which
/// are identical to `inner_ctx`'s — `inner_ctx` is a clone of `ctx` that only
/// changed frame, depth and effect — so reading them off `inner_ctx` is exactly
/// what the arm did.
fn draw_symbol_contents(
    builder: &mut SceneBuilder<'_>,
    symbol: &buzz_scene::Symbol,
    inner: u32,
    doc: Affine,
    inner_ctx: &DrawCtx<'_>,
    cache: &mut DrawCache,
) {
    // A symbol has its own layer stack, so it has its own masks. They
    // are always in force inside a symbol: the "only when locked" rule
    // is about editing the mask, and you are not editing this one —
    // you are looking at an instance of it placed somewhere else.
    let masks = active_masks(&symbol.layers, MaskDisplay::Always);
    let mut open: Option<OpenMask> = None;
    // One seal run for the whole symbol: its fills' seams are sealed beneath
    // its artwork rather than over the artwork drawn before each fill. See
    // `SceneBuilder::begin_seal_run`. The same here whether the symbol is
    // drawn live or encoded once for stamping, so the two stay identical.
    //
    // Only for a symbol with artwork of its own at this frame: a run costs a
    // copy of everything drawn inside it, and one holding nothing but other
    // symbols — a character's body, a scene's cast — would lift no seal and
    // copy its whole subtree for nothing.
    let bounds = cache
        .symbols
        .get(symbol.id)
        .map_or(buzz_geom::Rect::ZERO, |info| info.resolved_bounds);
    let seals = wants_seal_run(builder, bounds, doc, &inner_ctx.projection)
        && symbol.layers.drawable_at(inner).any(|layer| {
        !layer.kind.is_mask()
            && layer
                .frames
                .keyframe_at(inner)
                .is_some_and(|k| k.objects.iter().any(|o| draws_own_art(o)))
    });
    if seals {
        builder.begin_seal_run();
    }

    for layer in symbol.layers.drawable_at(inner) {
        // E5/E7: a pass nobody wants any more ends within a layer, inside a
        // symbol too; a stamp built while aborted is truncated and is never kept
        // (see `try_stamp_symbol`).
        if cache.symbol_layer_aborted() {
            break;
        }
        // Inside a symbol masks are always in force (see above), so a
        // mask layer here is always a stencil and never artwork.
        if layer.kind.is_mask() {
            continue;
        }

        let wanted = masks.get(&layer.id).copied();
        if wanted != open.as_ref().map(|o| o.mask) {
            close_mask(builder, open.take());
            if let Some(mask_id) = wanted
                && let Some(path) = mask_geometry(
                    inner_ctx.scene,
                    &symbol.layers,
                    mask_id,
                    inner,
                    doc,
                    &inner_ctx.projection,
                    builder.tolerance(),
                )
            {
                open = open_mask(builder, &symbol.layers, mask_id, &masks, inner, path);
            }
        }

        // An outline already in force wins — the stage layer's outline
        // toggle applies to everything it contains.
        let layer_ctx = DrawCtx {
            tint: inner_ctx
                .tint
                .or_else(|| layer.outline.then_some(layer.color)),
            faded: inner_ctx.faded || layer.kind == LayerKind::Guide,
            // Each layer inside the symbol counts from its own
            // keyframe, exactly as the stage's layers do.
            elapsed: inner
                - layer
                    .frames
                    .keyframe_at(inner)
                    .map(|k| k.start)
                    .unwrap_or(0)
                    .min(inner),
            ..inner_ctx.clone()
        };
        // A symbol's layers can follow each other too, which is
        // how a character symbol is rigged inside itself.
        let follows = symbol.layers.inherited_transform(layer.id, inner);
        for (child, owner) in symbol.layers.resolved_at_rigged(layer.id, inner).iter_owned() {
            // S7 (stop latency): see `draw_layer`'s object loop.
            if cache.is_aborted() {
                break;
            }
            draw_object(builder, child, owner, doc * follows, &layer_ctx, cache);
        }
    }

    close_mask(builder, open);
    if seals {
        builder.end_seal_run();
    }
}

/// A layer or symbol whose artwork spans fewer device pixels than this, on
/// its longest side, has its seals lifted beneath it
/// ([`SceneBuilder::begin_seal_run`]).
///
/// **Why only there.** The in-line seal eats half a device pixel of whatever
/// was drawn before it. That matters when the artwork's lines are only a pixel
/// or two wide on screen — a character standing far off, a face, an eye, a
/// mouth at stage size — and not at an ordinary size, where the in-line seal
/// is the look every existing document already has. And a run is not free:
/// its artwork is encoded into a scene of its own and copied into the frame
/// when it closes, which on a lamp-lit layer of four hundred shapes cost a
/// millisecond of every drag frame.
///
/// **Size, not scale.** barish1's mouth symbol is drawn at 0.94 device pixels
/// per unit, yet its teeth are 2.6 px tall: what decides whether line work is
/// thin on screen is how big the drawing is there, not how much its symbol
/// was scaled. 256 px is a figure a quarter of a 1080p frame tall or less —
/// barish1's whole face at stage size is about 105 px, its mouth about 30.
const SEAL_RUN_BELOW_PX: f64 = 256.0;

/// Should artwork whose own bounds are `bounds` (in its own units), placed by
/// `doc` and projected by `projection`, open a seal run? See
/// [`SEAL_RUN_BELOW_PX`]. The same answer live and stamped: a stamped child is
/// encoded at the instance's own (quantised) scale with an identity `doc`, so
/// `pixels_per_unit` carries what `doc` carries live.
fn wants_seal_run(
    builder: &SceneBuilder<'_>,
    bounds: buzz_geom::Rect,
    doc: Affine,
    projection: &Projection,
) -> bool {
    let det = |a: Affine| {
        let [a, b, c, d, _, _] = a.as_coeffs();
        (a * d - b * c).abs().sqrt()
    };
    let seen = projection.as_affine().map_or(1.0, det);
    let extent = bounds.width().max(bounds.height());
    extent > 0.0 && extent * builder.pixels_per_unit() * det(doc) * seen < SEAL_RUN_BELOW_PX
}

/// Does this object draw artwork itself, rather than only placing a symbol
/// that draws its own? What decides whether a seal run is worth opening: see
/// `draw_symbol_contents`.
fn draws_own_art(object: &Object) -> bool {
    !matches!(object.kind, ObjectKind::Instance(_))
}

/// **Try to append a cached encoding of this whole layer instead of walking
/// it.** P2 in `docs/IMPORT_PERF_PLAN.md`: a static layer — the common case,
/// artwork that never changes frame to frame — costs nothing beyond the
/// first time this hits.
///
/// Encodes in **pre-camera space**: the fragment is built with `camera` held
/// at [`Affine::IDENTITY`] and the real camera applied afterwards as the
/// [`vello::Scene::append`] transform (`layer_stamp`), exactly the trick
/// [`try_stamp_symbol`] already uses for an instance's own placement. That is
/// only valid while the camera is a plain affine — the same condition
/// [`draw_layer`] itself already requires to fold the camera straight into
/// `Projection::from_affine` rather than reaching for
/// [`buzz_scene::Scene::camera_projection_at_depth`] — so this refuses
/// whenever that does not hold, and everything else that would make a cached
/// fragment wrong: active lighting (a shadow or a rim depends on where the
/// light actually is), depth of field (the blur depends on distance from the
/// *real* camera), an outlined or onion-skinned layer, layer filters, a
/// non-normal blend, an unculled frame (there is nothing to quantise a key
/// from) and a tweened frame — built fresh every frame, with no stable
/// identity to key on; see [`buzz_scene::ResolvedFrame::iter_owned`].
///
/// When it returns `false` the caller draws the layer live through
/// [`draw_layer`], which is always correct — the cache only ever *adds* a
/// fast path, and every gate below that falls through leaves the live walk
/// to produce the picture.
#[allow(clippy::too_many_arguments)]
fn try_stamp_layer(
    builder: &mut SceneBuilder<'_>,
    scene: &Scene,
    layers: &buzz_scene::LayerStack,
    layer: &buzz_scene::Layer,
    frame: u32,
    camera: Affine,
    follows: Affine,
    options: &FrameOptions,
    lights: &Arc<buzz_scene::LightRig>,
    cache: &mut DrawCache,
) -> bool {
    if !cache.layer_scenes.enabled {
        return false;
    }
    if layer.depth != 0.0 || scene.camera_has_tilt() {
        return false;
    }
    if layer.kind == LayerKind::Guide || layer.outline {
        return false;
    }
    if options.ghost.is_some() {
        return false;
    }
    if options.layer_alpha && layer.alpha < 0.999 {
        return false;
    }
    if !layer.filters.is_empty() {
        return false;
    }
    if layer.blend != buzz_scene::Blend::Normal {
        return false;
    }
    if options.lit && lights.is_active() {
        return false;
    }
    let time = options.at(frame);
    if scene.camera().dof_blur_at(time, layer.depth).is_some() {
        return false;
    }
    let Some(cull) = options.cull else {
        return false;
    };
    let buzz_scene::ResolvedFrame::Stored(objects) = layers.resolved_at_rigged(layer.id, time)
    else {
        return false;
    };

    let scale = builder.split.scale;
    if !scale.is_finite() || scale <= 0.0 {
        return false;
    }
    // 64 view (device-independent) pixels, in document units at this zoom.
    let cell = 64.0 / scale;
    let key_rect = quantise_cull(cull, cell);
    let key = LayerKey {
        layer: layer.id.0,
        content: layer_content_hash(layer.id, objects),
        detail: cache.detail() as u8,
        scale_bits: scale.to_bits(),
        cull: key_rect,
        place_bits: affine_hash(options.place),
        follows_bits: affine_hash(follows),
    };
    let generation = cache.layer_scenes.frame;
    let anchor = builder.split.anchor;

    let hit = match cache.layer_scenes.entries.get_mut(&key) {
        // Verified against the live objects, not trusted from the hashed
        // key alone — see `LayerEntry::objects`.
        Some(entry) if same_objects(&entry.objects, objects) => {
            entry.used = generation;
            Some((Arc::clone(&entry.scene), entry.lines))
        }
        _ => None,
    };
    if let Some((child, lines)) = hit {
        cache.layer_scenes.stamps += 1;
        if std::env::var("BUZZ_DEBUG_LAYER_CACHE").is_ok() {
            eprintln!("layer-cache HIT: {:?} ({})", layer.name, layer.id.0);
        }
        layer_stamp(builder, &child, camera, anchor, scale, lines);
        return true;
    }

    // A miss: encode the layer once, pre-camera, culled to a rect 25% larger
    // than the current one (quantised, then grown) so a small pan still
    // hits next frame instead of rebuilding on every one.
    let snapped = buzz_geom::Rect::new(
        key_rect.0 as f64 * cell,
        key_rect.1 as f64 * cell,
        key_rect.2 as f64 * cell,
        key_rect.3 as f64 * cell,
    );
    let grown = snapped.inflate(snapped.width() * 0.125, snapped.height() * 0.125);
    let child_options = FrameOptions {
        cull: Some(grown),
        ..*options
    };

    let mut child_scene = crate::vello::Scene::new();
    let child_lines = {
        let split = RenderSplit {
            anchor,
            scale,
            // Unit scale, like `try_stamp_symbol`'s child: this fragment is
            // never drawn to a device directly, only appended into a parent
            // builder that already carries its own `gpu_view`.
            gpu_view: Affine::IDENTITY,
        };
        let clip = RenderClip::new(grown);
        let mut child = SceneBuilder {
            scene: &mut child_scene,
            split,
            visible: clip.bounds(),
            clip,
            lines: 0,
            device_scale: 1.0,
            last_stats: PathLineStats::default(),
            runs: Vec::new(),
            spare_runs: Vec::new(),
        };
        draw_layer(
            &mut child,
            scene,
            layers,
            layer,
            frame,
            // Pre-camera: see this function's own doc.
            Affine::IDENTITY,
            follows,
            &child_options,
            lights,
            cache,
            // A stamped layer is never lit (see the gate above), so it throws
            // no shadow of any kind.
            ShadowRole::Own,
        );
        child.lines
    };
    let child = Arc::new(child_scene);
    let bytes = (child_lines as usize).saturating_mul(LAYER_BYTES_PER_LINE);
    {
        let lc = &mut cache.layer_scenes;
        lc.builds += 1;
        lc.stamps += 1;
        lc.bytes += bytes;
        lc.entries.insert(
            key,
            LayerEntry {
                scene: Arc::clone(&child),
                objects: objects.to_vec(),
                used: generation,
                lines: child_lines,
                bytes,
            },
        );
    }
    if std::env::var("BUZZ_DEBUG_LAYER_CACHE").is_ok() {
        eprintln!(
            "layer-cache MISS/BUILD: {:?} ({}) bounds={:?} objects={} grown={:?} cull={:?} \
             anchor={:?} scale={} camera={:?} gpu_view={:?}",
            layer.name,
            layer.id.0,
            layer.bounds_at(frame),
            objects.len(),
            grown,
            cull,
            anchor,
            scale,
            camera.as_coeffs(),
            builder.split.gpu_view.as_coeffs(),
        );
    }
    layer_stamp(builder, &child, camera, anchor, scale, child_lines);
    true
}

/// Draw an instance from the symbol-scene cache when it is safe to, returning
/// whether it did.
///
/// When it returns `false` the caller draws the symbol live, which is always
/// correct — the cache only ever *adds* a fast path, and every gate below that
/// falls through leaves the live walk to produce the picture.
#[allow(clippy::too_many_arguments)]
fn try_stamp_symbol(
    builder: &mut SceneBuilder<'_>,
    ctx: &DrawCtx<'_>,
    inner_ctx: &DrawCtx<'_>,
    symbol_id: buzz_scene::SymbolId,
    symbol: &Arc<buzz_scene::Symbol>,
    inner: u32,
    doc: Affine,
    cache: &mut DrawCache,
) -> bool {
    if !cache.symbol_scenes.enabled {
        cache.symbol_scenes.refused_disabled += 1;
        return false;
    }
    // probe only (S13): `BUZZ_PROBE_STAMP_ONLY=<name>[,<name>]` stamps only those
    // symbols, `BUZZ_PROBE_STAMP_SKIP=<name>[,<name>]` never stamps those. Default
    // off (both unset) — a bisecting aid for pixel-parity hunts, nothing else.
    {
        static PROBE: std::sync::OnceLock<(Option<Vec<String>>, Option<Vec<String>>)> =
            std::sync::OnceLock::new();
        let (only, skip) = PROBE.get_or_init(|| {
            let list = |k: &str| {
                std::env::var(k)
                    .ok()
                    .map(|v| v.split(',').map(str::to_owned).collect::<Vec<_>>())
            };
            (list("BUZZ_PROBE_STAMP_ONLY"), list("BUZZ_PROBE_STAMP_SKIP"))
        });
        if only.is_some() || skip.is_some() {
            let hit = |l: &Vec<String>| l.iter().any(|n| *n == symbol.name);
            if only.as_ref().is_some_and(|l| !hit(l)) || skip.as_ref().is_some_and(hit) {
                cache.symbol_scenes.refused_disabled += 1;
                return false;
            }
        }
    }

    // The symbol's whole subtree must render position-independently, and the memo
    // must know it. Copy the few facts out so the memo's borrow ends here, before
    // the cache is borrowed mutably below.
    let (fingerprint, resolved_bounds, has_images) = {
        let Some(info) = cache.symbols.get(symbol_id) else {
            cache.symbol_scenes.refused_no_info += 1;
            return false;
        };
        if !info.cacheable_content() {
            if info.flags.filters {
                cache.symbol_scenes.refused_filters += 1;
            }
            if info.flags.group_blend {
                cache.symbol_scenes.refused_group_blend += 1;
            }
            if info.flags.non_flat {
                cache.symbol_scenes.refused_non_flat += 1;
            }
            if info.flags.inverse_mask {
                cache.symbol_scenes.refused_inverse_mask += 1;
            }
            return false;
        }
        (info.fingerprint, info.resolved_bounds, info.flags.images)
    };
    // The identity check on a hit compares against the *live* symbol, not the
    // memo's copy, so an entry left over from another document is rejected.
    let symbol_arc = Arc::clone(symbol);

    // **Lit stamps (`RIG_SCENE_LAG_PLAN` L3'').** A lit layer used to refuse
    // every instance here, so a rig of symbols was walked and encoded live,
    // shape by shape, every frame. It may stamp when the light cannot tell one
    // instance from another, which holds exactly when
    //
    // * `LightRig::field` gives the same `Illumination` wherever it is asked
    //   (`is_position_independent`: suns, a flat sky, no lamp — the field
    //   depends on position only through a lamp's falloff and a sky that mixes
    //   by height), so every shape of the symbol takes the same light at any
    //   placement and the stamp can carry it, encoded by the very
    //   `draw_shape` the live walk uses; and
    // * no shape inside draws bands of its own (`shape_edges == 0`: `Off`, or
    //   `Figure`, where an instance is never "alone" — the figure pass draws
    //   the bands round the whole figure, outside the stamp). That also makes
    //   `draw_gloom_edge`, `crescents` and the lamp-only branches of
    //   `draw_shape` unreachable inside a lit build.
    //
    // * no bitmap is inside (`SymFlags::images`): a picture takes the light as an
    //   isolation group sized to its box in the space it is encoded in, which for
    //   a rotated stamp is not the group the live walk lays.
    //
    // Anything else (a lamp, a mixed sky, `Shapes` edges, a picture) draws live,
    // as it always did, counted as `neutral_ctx`.
    let lit = ctx.lighting.is_some();

    // Colour, overlay and lighting must all be neutral; otherwise the cached
    // artwork — encoded once, shared by every instance — would be wrong here.
    if ctx.tint.is_some()
        || ctx.faded
        || ctx.ghost.is_some()
        || ctx.adjust.is_some()
        || ctx.gradient_map.is_some()
        || ctx.blur.is_some()
        || (lit
            && (!ctx.lights.is_position_independent()
                || ctx.shape_edges != 0.0
                || has_images))
        // **A breath is a deformation of the artwork**, so a cached encoding of
        // that artwork is the wrong shape — and it changes every frame, so
        // there is nothing here worth keeping anyway. Without this the symbol
        // would be stamped from the first frame's encoding and the character
        // would simply stop breathing.
        || ctx.breath.is_some()
    {
        cache.symbol_scenes.refused_neutral_ctx += 1;
        return false;
    }

    // The placement must be a *similarity* — a uniform scale composed with an
    // orthogonal linear part (rotation, optionally mirrored) — so a stroke
    // width, a seam seal or a flattening tolerance baked at one scale stays
    // correct once the instance's transform is folded in. A genuinely
    // stretched or skewed placement still draws live.
    let Some(proj_affine) = ctx.projection.as_affine() else {
        cache.symbol_scenes.refused_no_affine_projection += 1;
        return false;
    };
    let a = proj_affine * doc;
    // The scale a similarity carries is the square root of its linear part's
    // determinant — the factor a unit area maps to — whatever the rotation or
    // mirroring. A linear part with (numerically) no area cannot be factored
    // into a scale and an orthogonal part at all.
    let det = a.determinant();
    if det.abs() < 1e-12 {
        cache.symbol_scenes.refused_degenerate += 1;
        return false;
    }
    let s = det.abs().sqrt();

    // **Snapped to 2^-20 octave (S13a) — float-noise only.** The symbol-scene
    // cache keys on scale (see `SymKey`). `s` is a square root of a determinant,
    // so the *same* scale reached by different rotations (a limb swinging
    // through a hundred angles) differs in its last few bits; keying on those
    // raw would give every angle its own entry. Snapping the exponent to
    // 2^-20 octave (a step of 6.6e-7, at most 3.3e-7 off) folds that noise
    // together and nothing else. It used to be 1/32 octave (up to 1.1% off):
    // the child was then encoded at a scale 1% from the instance's, the append
    // applied the residual exactly, and yet big symbols drifted up to a pixel
    // from the live draw at 4x-40x (measured: Bimari5 f20 4x `houses`,
    // `newTree`, `lathaitFront` 0.16% of pixels over 2/255, 0.001% with the
    // exact scale). The encoding is now within float noise of the live one.
    let s_q = 2f64.powf((s.log2() * SYMBOL_SCALE_STEPS_PER_OCTAVE).round() / SYMBOL_SCALE_STEPS_PER_OCTAVE);
    if !s_q.is_finite() || s_q <= 0.0 {
        cache.symbol_scenes.refused_degenerate += 1;
        return false;
    }
    let [a0, a1, a2, a3, _, _] = a.as_coeffs();
    // The linear part with that quantised scale divided back out — near
    // orthogonal within the residual the quantisation above can leave, plus a
    // like amount of genuine anisotropy, rather than exactly. `is_orthogonal`
    // takes the wider tolerance that admits both; see its doc.
    let r_similarity = [a0 / s_q, a1 / s_q, a2 / s_q, a3 / s_q];
    // A similarity keeps the path above unchanged. A genuinely stretched or
    // skewed placement — S2 in `docs/BIMARI5_LAG_PLAN.md` — no longer draws
    // live: see the branch below, which keys on the placement's own exact
    // linear part instead of refusing.
    //
    // **Float-noise similarity test (non-similar default-on).** Decided on the
    // *unquantised* `a / s` (`s = √|det a|`), which is orthogonal to ~1e-16 for
    // a true similarity at any rotation or mirror, so `eps` 1e-9 admits only
    // float noise. Anything with real anisotropy (even 1%) keys on its exact
    // linear part below. (`r_similarity`, the stamp's own `a / s_q`, still
    // carries the 2^-20 snap's residual, which is why it is not the one tested.)
    let similar = is_orthogonal(
        [a0 / s, a1 / s, a2 / s, a3 / s, 0.0, 0.0],
        1e-9,
    );
    if !similar && !cache.symbol_scenes.non_similar {
        cache.symbol_scenes.refused_non_similar += 1;
        return false;
    }
    // A figure's silhouette is being gathered (or an enclosing stamp is being
    // built and will hand its own on): a lit non-similar stamp has the whole
    // projection baked into its local space, so its paths could only reach the
    // gatherer in document space through an inverse — not exact. The live walk
    // gives them exactly.
    if lit && !similar && cache.lights.collecting() {
        cache.symbol_scenes.refused_lit_silhouette += 1;
        return false;
    }

    let scale = builder.split.scale;
    if !scale.is_finite() || scale <= 0.0 {
        cache.symbol_scenes.refused_bad_scale += 1;
        return false;
    }
    // The point every instance of this symbol shares regardless of its own
    // placement — the stamp's translation is `scale · (a·anchor − cam)`
    // whatever linear part the child's own geometry carries (see the doc on
    // `symbol_stamp` and `SymKey::linear_bits`), so both branches below use
    // this same, untransformed value.
    let anchor = resolved_bounds.center();
    // The translation stays at the *stage's* scale, not the child's — it
    // places the child's own origin, and the child scale already accounts for
    // that once, inside the child's own geometry.
    let cam = builder.split.anchor;
    if (((a * anchor) - cam) * scale).hypot() > 1e7 {
        cache.symbol_scenes.refused_too_far += 1;
        return false;
    }
    let diag = resolved_bounds.width().hypot(resolved_bounds.height());

    // `r`: the linear part `stamp_scene` actually carries on append.
    // `child_scale`: the scalar `RenderSplit::scale` the child is encoded at.
    // `build_doc`: what `draw_symbol_contents` folds into the child's own
    // geometry while it is built (`Affine::IDENTITY` for a similarity, as
    // before; the placement's full linear part for a non-similarity).
    // `build_anchor`: the child build's own `RenderSplit::anchor` — `anchor`
    // itself for a similarity, or `anchor` carried through the same linear
    // part the geometry now bakes in, so the anchor subtraction re-centres
    // the *transformed* geometry the same well-conditioned way it always
    // re-centred the untransformed kind.
    let (r, child_scale, build_doc, build_anchor, linear_bits) = if similar {
        // The child is encoded at the stage's scale *and* the instance's
        // own, so its baked stroke widths and its flattening tolerance come
        // out exactly as a live draw at the combined scale would bake them.
        // See `symbol_stamp`.
        let child_scale = scale * s_q;
        // Keep the child's render-space coordinates, and the stamp's
        // translation, inside the well-conditioned range of f32.
        if child_scale * diag > 1e5 {
            cache.symbol_scenes.refused_too_big += 1;
            return false;
        }
        (r_similarity, child_scale, Affine::IDENTITY, anchor, [0u64; 4])
    } else {
        // **S2: bake the placement's whole linear part into the child's own
        // geometry**, the same way an *ordinary* nested transform already
        // does (`draw_object`'s `doc = doc * object.transform`) — so the
        // encoding is exactly what a live draw at this placement would
        // produce, translation aside. `stamp_scene` then carries only the
        // translation (`r = [1,0,0,1]`): the linear part is already in the
        // paths, the stroke widths (`stroke.width × stroke_placement_scale(doc)
        // × ctx.stroke_scale`, computed from this same `doc`) and the seam
        // seals, so nothing is left for the append to distort.
        //
        // Static placements repeat these same bits every frame and hit; a
        // tweened skew simply misses every frame, as a live-only placement
        // always has.
        let build_doc = Affine::new([a0, a1, a2, a3, 0.0, 0.0]);
        // A conservative (never-under) bound on how far this linear part can
        // stretch a point — `crate::max_stretch`'s own job for a stroke pen
        // transformed non-uniformly — used here for the same reason the
        // similarity branch bounds `child_scale * diag`: keeping the child's
        // own coordinates inside the well-conditioned range of f32.
        if scale * crate::max_stretch(build_doc) * diag > 1e5 {
            cache.symbol_scenes.refused_too_big += 1;
            return false;
        }
        let build_anchor = build_doc * anchor;
        (
            [1.0, 0.0, 0.0, 1.0],
            scale,
            build_doc,
            build_anchor,
            [a0.to_bits(), a1.to_bits(), a2.to_bits(), a3.to_bits()],
        )
    };

    // What every stroke the child encodes gets multiplied by.
    //
    // **The projection's own scale must be divided out (S13a).** A live stroke
    // is `width × stroke_placement_scale(doc) × stroke_scale × S`, where `doc`
    // is the *document-space* placement — the projection (camera pan/zoom and
    // layer depth) is applied to the path only and never widens the pen. The
    // stamp, though, carries `a = projection ∘ doc`: `s_q × |r| = √|det a| =
    // ps · stroke_placement_scale(doc)` (similarity) — or `build_doc` *is* `a`
    // (non-similarity) — so without this every stamped stroke came out `ps`
    // times the live width, `ps` being the projection's scale (0.39 for the
    // deep "well" layer at Bimari5 f20: the outline went 2.5× too thin).
    // Dividing `ps` out leaves exactly `w × sps(doc) × stroke_scale × S`.
    // `ps` is a property of the layer's projection alone, so every instance
    // on one layer shares these bits and the key does not fragment.
    let ps = proj_affine.determinant().abs().sqrt();
    if !ps.is_finite() || ps <= 0.0 {
        cache.symbol_scenes.refused_degenerate += 1;
        return false;
    }
    let child_stroke_scale = ctx.stroke_scale / ps;
    let key = SymKey {
        fingerprint,
        inner,
        depth: inner_ctx.depth.min(u8::MAX as usize) as u8,
        scale_bits: child_scale.to_bits(),
        sealed: cache.detail().seals(),
        stroke_scale_bits: child_stroke_scale.to_bits(),
        effect_bits: effect_key_bits(&inner_ctx.effect),
        non_similar: !similar,
        linear_bits,
        // The light a lit stamp is encoded under; all zero when unlit.
        lit,
        light_print: if lit { ctx.light_print } else { 0 },
        light_depth_bits: ctx.lighting.map_or(0, |(_, depth)| depth.to_bits()),
        stage_height_bits: ctx.lighting.map_or(0, |(height, _)| height.to_bits()),
    };
    let generation = cache.symbol_scenes.frame;

    // Looked up *before* the visibility gate below, so a mostly off-screen
    // instance can still take a hit it costs nothing new to keep — see
    // there. The pointer check rejects the rare case of two different
    // symbols colliding on the key.
    let hit = {
        let sc = &mut cache.symbol_scenes;
        sc.clock += 1;
        let tick = sc.clock;
        match sc.entries.get_mut(&key) {
            Some(entry) if Arc::ptr_eq(&entry.symbol, &symbol_arc) => {
                entry.used = generation;
                entry.tick = tick;
                let hit = (
                    Arc::clone(&entry.scene),
                    entry.anchor,
                    entry.lines,
                    entry.silhouette.clone(),
                );
                // E6: an installed stamp a frame has now used is retained like
                // any other.
                if !sc.transient.is_empty() {
                    sc.transient.remove(&key);
                }
                Some(hit)
            }
            _ => None,
        }
    };
    // Set below when a mostly-off-screen miss was also judged so last
    // generation: already a second sighting, so it builds without waiting.
    let mut static_miss = false;
    // Item 0 (E7): what the two sighting maps held for `key` before this call,
    // so a build cut short by the abort flag can leave them as it found them.
    let mut prev_off_screen: Option<Option<u64>> = None;
    let prev_sighting: Option<u64>;

    // **Do not stamp a mostly off-screen instance, unless it is already a
    // cache hit that fits this frame's line budget, or it is a miss that has
    // been asked for this way before.** A stamped child is encoded once,
    // unculled (`cull: None` in the neutral context below), and then
    // appended wherever every instance of it places it — including a huge
    // background symbol whose instance covers many times the visible stage.
    // The live path culls the parts of such a symbol that lie off-screen;
    // *building* it here would pay to encode, and to keep cached, the whole
    // thing for an instance that is mostly not seen. So a placement more
    // than half off the current cull rectangle still falls back to the live,
    // culled path on a **miss seen for the first time** — the same idiom
    // `draw_object` uses to cull a bare shape.
    //
    // A **hit** is different: the encoding already exists (another instance
    // of the same key paid for it), stamping it costs only the lines it is
    // already known to add — `entry.lines`, measured, not estimated — and
    // the picture comes out pixel-identical either way, since the GPU clips
    // the off-screen part of a stamp exactly as it would a live draw. So a
    // hit is taken whenever it still fits this frame's own line budget
    // (`builder.lines`, this frame's running total, against
    // [`line_budget`] — the same ceiling `DrawCache::reconsider` judges the
    // whole frame against); refused only when it would not, so an
    // already-heavy frame cannot be pushed over by an instance that is
    // barely on screen.
    //
    // A **miss seen mostly off-screen at this same key in the immediately
    // preceding generation too** is the third case: static across frames,
    // in `SymbolSceneCache::seen_off_screen`'s own words — the same camera,
    // the same placement, asked for again rather than a placement merely
    // passed through once (a single scrub frame, one band). A top-level
    // instance that never fits on screen — the very case S1 alone could not
    // reach, because it can never *become* a hit without building at least
    // once — builds here instead of redrawing all of its own artwork live
    // every single generation forever; every nested child under it keeps
    // reaching its own `try_stamp_symbol` call regardless; whichever of them
    // are visible were already being cached before this case existed.
    //
    // Judged against `stamp_view.or(cull)`, which is `cull` itself for every
    // caller today (see [`DrawCtx::stamp_view`]) — in band mode, one band's
    // own strip. A character straddling a band boundary is "mostly off" more
    // than one strip and is drawn live and culled in each, which costs a
    // little walking twice but keeps every band's own line count to what is
    // actually inside it. Judging this against the *frame's* whole visible
    // rectangle instead — so a straddling character passed once — was tried
    // and reverted: the stamp it then passed is appended *whole*, unculled,
    // into every band it touches, so more bands stop shrinking the largest
    // band's own lines and an over-budget band rasterises as black. See H0
    // item 1 in `docs/IMPORT_PERF_PLAN.md`.
    if let Some(view) = ctx.stamp_view.or(ctx.cull) {
        let world = buzz_scene::object::transform_rect(doc, resolved_bounds);
        if let Some(shot) = ctx.projection.map_rect_bounds(world)
            && shot.intersect(view).area() < shot.area() * 0.5
        {
            let within_budget = match &hit {
                Some((_, _, lines, _)) => builder.lines + lines <= line_budget(),
                None => false,
            };
            // Recorded regardless of the outcome below, so the *next*
            // generation can tell this one asked too — a single sighting
            // must not itself count as "asked before".
            let band_mode = cache.symbol_scenes.band_mode;
            let prev_off = cache.symbol_scenes.seen_off_screen.insert(key, generation);
            prev_off_screen = Some(prev_off);
            let seen_last_generation = prev_off
                // Band mode: this generation's own sighting (an earlier band of
                // the same frame) is not "the immediately preceding generation".
                .is_some_and(|g| {
                    generation.saturating_sub(g) <= 1 && !(band_mode && g == generation)
                });
            static_miss = hit.is_none() && seen_last_generation;
            if !within_budget && !static_miss {
                cache.symbol_scenes.refused_under_half_visible += 1;
                if std::env::var("BUZZ_DEBUG_SYMBOL_STAMP").is_ok() {
                    eprintln!(
                        "symbol-cache REFUSED (under_half_visible, {}): fingerprint={fingerprint} \
                         depth={} inner={inner} hit={}",
                        if hit.is_some() { "over budget" } else { "miss" },
                        inner_ctx.depth,
                        hit.is_some(),
                    );
                }
                return false;
            }
        }
    }

    if let Some((scene, anchor, lines, silhouette)) = hit {
        if lit && cache.lights.collecting() {
            // **The figure's silhouette on a hit** (L3''-3): the paths the
            // live walk would have handed to the figure pass, placed by the
            // instance's own document transform. A lit similarity stamp always
            // carries them; one that does not (never expected) draws live.
            match &silhouette {
                Some(pieces) => cache.lights.add_stamp_silhouette(doc, pieces),
                None => {
                    cache.symbol_scenes.refused_lit_silhouette += 1;
                    return false;
                }
            }
        }
        cache.symbol_scenes.stamps += 1;
        if lit {
            cache.symbol_scenes.stamps_lit += 1;
        }
        stamp_scene(builder, &scene, a, r, anchor, lines);
        return true;
    }

    // **A miss for a key never sighted before is drawn live** (S2-rest (b)):
    // building costs more than drawing live once, so a first visit must
    // never be slower than the live path. Only the sighting is recorded; the
    // key builds on its second request (a later generation, or a second
    // instance in this one). A static mostly-off-screen miss (above) is
    // already a second sighting and builds now.
    {
        let sc = &mut cache.symbol_scenes;
        let band_mode = sc.band_mode;
        let prev_sighted = sc.sighted.insert(key, generation);
        prev_sighting = prev_sighted;
        let sighted_before = prev_sighted.is_some_and(|g| {
            generation.saturating_sub(g) < SYM_SIGHT_KEEP_FRAMES
                // Band mode: this generation's own sighting is not an earlier one.
                && !(band_mode && g == generation)
        });
        // E3: inside a stamp build the rule is skipped, so nested misses
        // always build and the stamp's bytes depend only on its key.
        if !sighted_before && !static_miss && !sc.build_on_first_sighting && !ctx.in_stamp_build {
            sc.first_sightings += 1;
            return false;
        }
        // The sighting is kept: a key evicted later rebuilds on its next request.
    }

    // probe only (S13): `BUZZ_PROBE_STAMP_LOG=1` names every build.
    if std::env::var("BUZZ_PROBE_STAMP_LOG").is_ok() {
        eprintln!(
            "stamp-build name={:?} inner={inner} s={s:.6} s_q={s_q:.6} similar={similar} \
             S={} a={:?} anchor=({:.2},{:.2}) cull={:?}",
            symbol.name,
            format!("{scale} sps={:.6} doc={:?}", stroke_placement_scale(doc), doc.as_coeffs()),
            a.as_coeffs(),
            anchor.x,
            anchor.y,
            ctx.cull,
        );
    }
    // A miss: encode the whole symbol once, about its own centre, at this zoom,
    // through the *same* `draw_symbol_contents` the live path uses.
    let mut child_scene = crate::vello::Scene::new();
    // A lit build records every filled path it draws (see `SymEntry::silhouette`),
    // and keeps them out of any figure silhouette being gathered around it,
    // which wants them in document space, not in this stamp's local one.
    let outer_pieces = lit.then(|| cache.lights.begin_pieces());
    let child_lines = {
        let split = RenderSplit {
            anchor: build_anchor,
            scale: child_scale,
            gpu_view: Affine::IDENTITY,
        };
        // `resolved_bounds` is in the symbol's own *untransformed* local
        // space; the geometry this builder clips has already been carried
        // through `build_doc` (identity for a similarity, the placement's
        // full linear part for a non-similarity — see `draw_symbol_contents`
        // below), so the clip rectangle is `resolved_bounds` carried through
        // that same map, not the untransformed rectangle itself. Identical to
        // before for a similarity, where `build_doc` is the identity.
        let clip = RenderClip::new(buzz_scene::object::transform_rect(build_doc, resolved_bounds));
        // No viewport of its own to be tight about — this scene is stamped
        // wherever an instance places it, so its "visible" rectangle is the
        // same generous one `clip` already is.
        let mut child = SceneBuilder {
            scene: &mut child_scene,
            split,
            visible: clip.bounds(),
            clip,
            lines: 0,
            // `gpu_view` is `Affine::IDENTITY` above and is never touched by
            // `with_output_scale` — this child is never drawn to a device
            // directly, only stamped into a parent builder that already
            // carries its own `device_scale` — so unit scale is exact here.
            device_scale: 1.0,
            last_stats: PathLineStats::default(),
            runs: Vec::new(),
            spare_runs: Vec::new(),
        };
        let neutral = DrawCtx {
            cull: None,
            // Unculled like `cull` above, for the same reason: this child is
            // encoded once, position-independent, and a further instance
            // nested inside it must not be judged against where *this*
            // stamp happens to land — that would make the cached encoding
            // depend on the very placement it is meant to be shared across.
            stamp_view: None,
            // The light is kept (the gate above admitted only a rig that
            // lights every shape the same wherever it stands), so `draw_shape`
            // lights each shape of the child exactly as the live walk does;
            // `None` here when the layer is unlit, as `inner_ctx` already is.
            lighting: inner_ctx.lighting,
            // The composed effect from `inner_ctx`, not identity: this child
            // is encoded once and stamped by every instance that shares this
            // key, and the key now includes the effect (`SymKey::effect_bits`),
            // so every instance sharing this entry wants exactly this effect
            // baked in — a live draw with this same `inner_ctx` would apply
            // the identical effect through `DrawCtx::paint`/`colour`.
            effect: inner_ctx.effect,
            tint: None,
            faded: false,
            ghost: None,
            adjust: None,
            gradient_map: None,
            blur: None,
            projection: Projection::from_affine(Affine::IDENTITY),
            // A similarity's child is encoded at `child_scale = scale * s_q`
            // with `build_doc = IDENTITY`, and a live stroke scales with its
            // placement's `s`, of which `s_q` is the part baked into
            // `child_scale` and `|r|` the part the stamp's append applies:
            // nothing to compensate. A non-similarity's child is encoded at
            // `child_scale = scale` with `build_doc` carrying the placement's
            // whole linear part instead — `stroke_placement_scale(build_doc)`
            // then supplies exactly what a live stroke at this placement
            // would, and `r` is the identity on append: likewise nothing to
            // compensate. See `DrawCtx::stroke_scale`.
            stroke_scale: child_stroke_scale,
            // E3: everything nested in this build builds on a miss.
            in_stamp_build: true,
            ..inner_ctx.clone()
        };
        draw_symbol_contents(&mut child, symbol, inner, build_doc, &neutral, cache);
        child.lines
    };
    // The pieces of a lit build; only a similarity's are in a space the live
    // walk's document space can be rebuilt from (`placement * piece`).
    let pieces: Option<Vec<buzz_geom::BezPath>> = outer_pieces.map(|outer| cache.lights.end_pieces(outer));
    let silhouette = pieces
        .filter(|_| similar)
        .map(Arc::new);
    let scene = Arc::new(child_scene);
    // **A build cut short by the abort flag is truncated** (the nested
    // `draw_layers` stopped at its first layer), so it is never inserted,
    // collected or counted, and the sightings it recorded are put back: the
    // pass is being thrown away, and a later frame must not draw an
    // incomplete symbol from the cache.
    if cache.is_aborted() {
        let sc = &mut cache.symbol_scenes;
        match prev_sighting {
            Some(g) => sc.sighted.insert(key, g),
            None => sc.sighted.remove(&key),
        };
        if let Some(prev) = prev_off_screen {
            match prev {
                Some(g) => sc.seen_off_screen.insert(key, g),
                None => sc.seen_off_screen.remove(&key),
            };
        }
        stamp_scene(builder, &scene, a, r, anchor, child_lines);
        return true;
    }
    {
        let sc = &mut cache.symbol_scenes;
        sc.builds += 1;
        sc.stamps += 1;
        if lit {
            sc.stamps_lit += 1;
        }
        sc.clock += 1;
        let bytes = scene_bytes(&scene);
        if let Some(old) = sc.entries.insert(
            key,
            SymEntry {
                scene: Arc::clone(&scene),
                symbol: symbol_arc,
                anchor,
                used: generation,
                lines: child_lines,
                bytes,
                tick: sc.clock,
                silhouette: silhouette.clone(),
            },
        ) {
            sc.bytes = sc.bytes.saturating_sub(old.bytes);
        }
        sc.bytes += bytes;
        // A key built here is used here, whatever it was before.
        if !sc.transient.is_empty() {
            sc.transient.remove(&key);
        }
        if sc.collect_new {
            sc.fresh.push(key);
        }
    }
    // What the live walk would have given a figure gathering its silhouette
    // (a lit similarity build only: `silhouette` is `None` otherwise, and a
    // lit non-similar build is never reached while one is being gathered).
    if let Some(pieces) = &silhouette {
        cache.lights.add_stamp_silhouette(doc, pieces);
    }
    stamp_scene(builder, &scene, a, r, anchor, child_lines);
    true
}

/// Append a cached child scene into the stage at the instance's placement.
///
/// `r` is the linear part the stamp actually carries: `a`'s own coefficients
/// when the caller has none better, or `try_stamp_symbol`'s quantised-scale
/// `r` when the child was encoded at `child_scale` rather than at `a`'s full
/// scale. See `symbol_stamp`.
fn stamp_scene(
    builder: &mut SceneBuilder<'_>,
    child: &crate::vello::Scene,
    a: Affine,
    r: [f64; 4],
    anchor: buzz_geom::Point,
    lines: u64,
) {
    let stamp = symbol_stamp(
        builder.split.gpu_view,
        a,
        r,
        anchor,
        builder.split.scale,
        builder.split.anchor,
    );
    // Only glyph runs set encoding flags, and nothing here draws text; a child
    // with an unbalanced clip stack would corrupt the parent's. Both hold today
    // and these pin them for whoever adds text rendering.
    debug_assert_eq!(
        builder.target_scene().encoding().flags,
        0,
        "no glyph-run flags to carry"
    );
    debug_assert_eq!(
        child.encoding().n_open_clips,
        0,
        "child must be layer-balanced"
    );
    builder.target_scene().append(child, Some(stamp));
    // The child was already encoded at its final device scale (see
    // `try_stamp_symbol`), so its own `estimated_lines()` — recorded in
    // `SymEntry` when it was built — is exactly what this `append` costs the
    // parent; no rescaling. A hit adds the same stored number a miss just
    // measured.
    builder.lines += lines;
}

/// Append a layer fragment encoded in pre-camera space (see
/// [`try_stamp_layer`]) into the stage, with `camera` inserted exactly where
/// it belongs in the chain [`SceneBuilder::to_render_space`] applies to
/// ordinary geometry.
///
/// # Derivation
///
/// A live-drawn point reaches render space as `M · camera · p`, where
/// `M = scale(split.scale) · translate(-anchor)` is the anchor-then-scale
/// step every shape goes through and `camera` is folded into `p`'s own
/// projection before that. The fragment was instead built with `camera` held
/// at the identity, so its own encoded points are already `M · p`, each
/// tagged with the identity transform (`gpu_view` was `Affine::IDENTITY` for
/// the child; see `try_stamp_layer`).
///
/// [`vello::Scene::append`] can only pre-multiply a single transform onto
/// every op it copies, so the append transform has to carry the parent's
/// `gpu_view`, `camera`, *and* the change of basis between `M · p` (what the
/// fragment already has) and `p` (what `camera` needs to receive) —
/// `stamp = gpu_view · M · camera · M⁻¹`, so that
/// `stamp · (M · p) = gpu_view · M · camera · p`, the same point the live
/// path reaches. `M⁻¹` is written out directly (`translate(anchor) ·
/// scale(1/s)`) rather than through a general matrix inverse, since `M` is
/// exactly that by construction and a general inverse would only reintroduce
/// the numerical cancellation `to_render_space` is careful to avoid.
fn layer_stamp(
    builder: &mut SceneBuilder<'_>,
    child: &crate::vello::Scene,
    camera: Affine,
    anchor: buzz_geom::Point,
    scale: f64,
    lines: u64,
) {
    let m = Affine::scale(scale) * Affine::translate(-anchor.to_vec2());
    let m_inv = Affine::translate(anchor.to_vec2()) * Affine::scale(1.0 / scale);
    let stamp = builder.split.gpu_view * m * camera * m_inv;
    debug_assert_eq!(
        builder.target_scene().encoding().flags,
        0,
        "no glyph-run flags to carry"
    );
    debug_assert_eq!(
        child.encoding().n_open_clips,
        0,
        "child must be layer-balanced"
    );
    builder.target_scene().append(child, Some(stamp));
    // The fragment's own `estimated_lines()`, recorded in `LayerEntry` when
    // it was built — exactly what this `append` costs the parent, on both a
    // hit and the miss that just measured it. See `stamp_scene`.
    builder.lines += lines;
}

/// Collect an object's filled outlines into one path, through `transform`.
///
/// Concatenated rather than unioned: filled non-zero, overlapping subpaths
/// merge visually, which is what a silhouette means — and a union would be a
/// boolean per object per frame for a result nobody would be able to tell
/// apart. Strokes are included via outline so that line-art and stroked artwork
/// properly cast shadows and glow.
///
/// Each path is wound forward as it goes in ([`buzz_geom::turns_backwards`]):
/// merging under the non-zero rule only holds while they all turn the same way,
/// and drawn artwork's do not. Two wound opposite ways cancel where they
/// overlap, so without it a figure drawn in pieces comes back holed at every
/// joint — a filter with the joins missing, or a rim glow that traces the
/// insides of the figure as well as its outline.
fn append_silhouette(
    object: &Object,
    transform: Affine,
    scene: Option<&Scene>,
    time: f64,
    out: &mut buzz_geom::BezPath,
    depth: usize,
    breath: Option<&buzz_scene::Breath>,
    stage_frame: u32,
    synced: bool,
    mut memo: Option<&mut SilhouetteMemo>,
) {
    if !object.visible || depth >= MAX_SYMBOL_DEPTH {
        return;
    }
    let world = transform * object.transform;
    match &object.kind {
        ObjectKind::Shape(shape) => {
            if let Some(memo) = memo.as_deref_mut() {
                memo.live += 1;
            }
            // Breathing is evaluated in the same space as the paint pass:
            // transforms first, then the deformation. Applying it to local
            // coordinates before `world` changes the pose for rotated/scaled
            // characters and makes their shadow diverge from the artwork.
            let transformed = world * shape.path.clone();
            let path = breath.map(|b| b.path(&transformed)).unwrap_or(transformed);
            if shape.fill.is_some() {
                for element in buzz_geom::wound_forward(&path).elements() {
                    out.push(*element);
                }
            }
            if let Some(stroke) = &shape.stroke {
                if stroke.width > 0.0 {
                    let outline = buzz_geom::outline_stroke(
                        &path,
                        buzz_geom::StrokeStyle::new(stroke.width),
                        0.1,
                    );
                    for element in buzz_geom::wound_forward(&outline).elements() {
                        out.push(*element);
                    }
                }
            }
        }
        ObjectKind::Group(children) => {
            for child in children {
                append_silhouette(
                    child,
                    world,
                    scene,
                    time,
                    out,
                    depth + 1,
                    breath,
                    stage_frame,
                    synced,
                    memo.as_deref_mut(),
                );
            }
        }
        ObjectKind::Warp(warp) => {
            if let Some(memo) = memo.as_deref_mut() {
                memo.live += 1;
            }
            let warped = warp.warped();
            for element in buzz_geom::wound_forward(&(world * warped.path)).elements() {
                out.push(*element);
            }
        }
        ObjectKind::Armature(rig) => {
            for (slot, part) in rig.parts.iter().zip(rig.posed()) {
                // **A rigid part's outlines are remembered** (see
                // `SilhouetteMemo`): only where they are placed moves with the
                // pose. Skinned parts, parts with an animated symbol inside and
                // any gather under a breath are walked live, exactly as ever.
                if breath.is_none()
                    && matches!(slot.binding, buzz_scene::RigBinding::Rigid(_))
                    && let Some(scene) = scene
                    && let Some(memo) = memo.as_deref_mut()
                    && memo.enabled
                    && let Some(leaves) = memo.leaves_for(scene, &slot.artwork, depth + 1)
                {
                    // `append_silhouette(&part, world, ..)`, replayed: the
                    // part's placement, then each leaf's chain in the order
                    // the live walk multiplies it. Winding is decided on the
                    // **placed** path (a mirror flips it), per path.
                    if !part.visible || depth + 1 >= MAX_SYMBOL_DEPTH {
                        continue;
                    }
                    let placed = world * part.transform;
                    for leaf in leaves.iter() {
                        let mut at = placed;
                        for step in leaf.chain.iter() {
                            at = at * *step;
                        }
                        let path = at * leaf.path.clone();
                        match leaf.stroke {
                            None => {
                                for element in buzz_geom::wound_forward(&path).elements() {
                                    out.push(*element);
                                }
                            }
                            Some(width) => {
                                let outline = buzz_geom::outline_stroke(
                                    &path,
                                    buzz_geom::StrokeStyle::new(width),
                                    0.1,
                                );
                                for element in buzz_geom::wound_forward(&outline).elements() {
                                    out.push(*element);
                                }
                            }
                        }
                    }
                    memo.replayed += leaves.len() as u64;
                    continue;
                }
                append_silhouette(
                    &part,
                    world,
                    scene,
                    time,
                    out,
                    depth + 1,
                    breath,
                    stage_frame,
                    synced,
                    memo.as_deref_mut(),
                );
            }
        }
        ObjectKind::Instance(instance) => {
            let Some(scene) = scene else { return };
            let Some(symbol) = scene.library().get(instance.symbol) else {
                return;
            };
            let follows_root = synced || instance.sync_to_root;
            let frame = if follows_root {
                instance.resolve_to_root(stage_frame, symbol.length())
            } else {
                instance.resolve_frame(symbol.kind, time.floor() as u32, symbol.length())
            };
            for layer in symbol.layers.drawable_at(frame) {
                if !layer.visible || layer.kind.is_mask() {
                    continue;
                }
                let follows = symbol.layers.inherited_transform(layer.id, frame);
                // Match draw_symbol_contents exactly: nested layer modifiers
                // are not evaluated by that traversal, so evaluating them only
                // for the filter silhouette would make the shadow disagree
                // with the nested artwork. Top-level modifiers are handled by
                // the caller above, where the paint pass evaluates them too.
                // The rig-relative resolution has to match too, or a rigged
                // child's silhouette would drift from the artwork it is the
                // silhouette of.
                for child in symbol.layers.resolved_at_rigged(layer.id, frame).iter() {
                    append_silhouette(
                        child,
                        world * follows,
                        Some(scene),
                        layer
                            .frames
                            .keyframe_at(frame)
                            .map(|k| frame.saturating_sub(k.start) as f64)
                            .unwrap_or(frame as f64),
                        out,
                        depth + 1,
                        breath,
                        stage_frame,
                        follows_root,
                        memo.as_deref_mut(),
                    );
                }
            }
        }
    }
}

/// Paint one shape's fill and stroke, lit.
#[allow(
    clippy::too_many_arguments,
    reason = "one call path; a struct would only move the arguments"
)]
/// **How much a placement scales a stroke's width**: the square root of its
/// linear part's determinant, the one number a uniform pen can take from a
/// transform (the same one `buzz_scene::stamp` bakes a stroke's width with).
///
/// # The defect this exists for
///
/// A shape's stroke width is stored in the shape's own units, and the draw
/// used to hand it to the builder unscaled — in *document* units — while the
/// path itself went through the whole placement chain. So a symbol drawn at a
/// tenth of its size kept outlines ten times too wide for it. barish1's face
/// sits at about 0.13 on the stage, and its eyeballs at a further 0.44 inside
/// it: a 2-unit iris outline that Animate draws a quarter of a pixel wide was
/// drawn two pixels wide, so the brown iris went black, the sclera
/// disappeared behind the rim, and the teeth were lost under the lip lines.
/// Animate scales every `scaleMode="normal"` stroke — which is every stroke in
/// that file — with its symbol, and the blur branch of [`draw_shape`] always
/// did too, because it outlines the stroke in the shape's own space before
/// placing it.
///
/// A degenerate placement (zero scale) draws no width rather than a NaN.
fn stroke_placement_scale(doc: Affine) -> f64 {
    let [a, b, c, d, _, _] = doc.as_coeffs();
    let det = (a * d - b * c).abs().sqrt();
    if det.is_finite() { det } else { 1.0 }
}

/// How much `projection` magnifies a length near `at`: exact for an affine
/// (the usual case), the local scale for a tilted camera.
fn projection_scale(projection: &Projection, at: buzz_geom::Point) -> f64 {
    match projection.as_affine() {
        Some(a) => {
            let [a, b, c, d, _, _] = a.as_coeffs();
            (a * d - b * c).abs().sqrt()
        }
        None => projection.scale_at(at),
    }
}

fn draw_shape(
    builder: &mut SceneBuilder<'_>,
    owner: Option<&Arc<Object>>,
    index: u16,
    shape: &buzz_scene::ShapeData,
    doc: Affine,
    ctx: &DrawCtx<'_>,
    cache: &mut DrawCache,
) {
    // The shape where it sits in the document, and where the lens puts it.
    // Everything drawn below goes through the second; everything *measured* —
    // lighting, which is a property of the scene rather than of the view — uses
    // the first.
    //
    // **Breathed first**, so everything downstream — the light on it, its
    // silhouette, its shadow — is measured against the shape as it is actually
    // drawn. A breath is a deformation of the artwork rather than a placement
    // of it, so it lands here on the document-space path and not on `doc`.
    let placed = ctx.breathe(doc * shape.path.clone());
    let path = ctx.project(&placed, builder.tolerance());

    // Outline view: draw the silhouette in the layer colour instead of the
    // artwork, which is what the timeline's outline column does.
    if let Some(color) = ctx.tint {
        builder.stroke_hairline(&path, ctx.overlay(color), 1.0);
        return;
    }

    // Where this shape sits in the document, which is what the lights are
    // measured against.
    //
    // **The whole region it covers, not only its middle.** A lamp's light falls
    // off, and the falloff *is* the lamp: asked at one point, a lamp answers
    // with one colour, and a shape filled with it shows no falloff at all —
    // a wall came out flat, and the near side of a face was exactly as bright
    // as the far side of the same face. `LightRig::field` answers for the
    // region, carrying the ramp the renderer needs to lay the light per pixel.
    let region = placed.bounding_box();
    let here = region.center();
    // Its longest side where it is drawn — what the seam seal is weighed
    // against (`SceneBuilder::fill_shape_paint_sealed`), taken from the
    // bounds just measured rather than from another walk of the path.
    let extent = region.width().max(region.height()) * projection_scale(&ctx.projection, here);
    let field = ctx
        .lighting
        .map(|(stage_height, depth)| ctx.lights.field(region, depth, stage_height));
    // The single-colour answer, for everything that cannot carry a ramp: a
    // stroke, blurred artwork, and the cast shadow.
    let light = field.as_ref().map(|f| f.uniform());
    // Where a gradient's unit space lands. The gradient is stored in the
    // shape's own coordinates, so `doc` puts it in the document, and the
    // projection puts it on the frame — the same two steps the path took above.
    let brush_to_doc = ctx.brush_projection(placed.bounding_box()) * doc;

    // **The figure this shape is part of**, if one is being gathered.
    //
    // A figure's crescents are measured against the silhouette of everything it
    // draws, and this is where that silhouette comes from: the draw itself.
    // Fills only — a silhouette is a region, and a stroke is a line — and in
    // *document* space, which is the space the light is measured in and the
    // space the crescent cache is keyed on. Costs nothing at all unless a
    // figure is being collected, which is only while one is being built.
    if shape.fill.is_some() && cache.lights.collecting() {
        cache.lights.add_silhouette(&placed);
    }

    if let Some(fill) = &shape.fill {
        // **A bitmap takes the light as a blend, not as a colour.**
        //
        // Everything in the branch below lights artwork by rewriting its paint,
        // and a paint made of pixels has nothing to rewrite: `map_colors`
        // returns an image untouched, by design — recolouring thirty million
        // pixels would cost more than the frame they are drawn in. So an
        // imported drawing, a photograph, and anything Break Apart produces
        // went through the whole lit path and came out exactly as painted. No
        // tint, no shaded side, no highlight. On a document made of pictures
        // that is not "lighting is subtle here", it is the lights doing
        // nothing at all.
        //
        // The light is composited over the picture instead; see
        // [`draw_lit_composited`], which draws the same three things in the same
        // order and lands on the same colours.
        //
        // **A lamp needs the same treatment, for the same reason one step
        // removed.** Rewriting a fill's colours can only produce one colour per
        // stop, so a lamp falling off across the shape has nowhere to put its
        // falloff. Composited, the light is a *paint* — the lamp's own radial
        // ramp — and lands per pixel. So the composited path is taken whenever
        // the artwork has no colours to rewrite, or the light is not one colour
        // here.
        //
        // **A solid colour does not need it, and must not pay for it.** Vello
        // re-encodes a path for every fill, so laying the light over the artwork
        // costs that shape's whole outline a second time and a third. A solid
        // fill under a lamp is *exactly* a radial gradient of that colour lit at
        // each radius, so the light goes into the paint and the shape is drawn
        // once, as it always was. See [`lamp_lit`].
        let composited = field
            .as_ref()
            // A blurred shape is not drawn as itself — the blur replaces the
            // fill with a soft stack of copies — so there is nothing for the
            // light to sit on. That branch flattens the image to one colour and
            // lights *that*, below.
            .filter(|f| {
                let needs_compositing = fill.paint.is_image()
                    || (f.disc().is_some() && !matches!(fill.paint, buzz_scene::Paint::Solid(_)));
                needs_compositing && ctx.blur.is_none()
            });

        if let Some(field) = composited {
            draw_lit_composited(
                builder,
                LitShape {
                    owner,
                    index,
                    fill,
                    blend: shape.blend,
                    extent,
                    placed: &placed,
                    path: &path,
                    here,
                    doc,
                    brush_to_doc,
                    field,
                    // Artwork made of coloured regions shares its boundary with
                    // the artwork beside it and must not leave a pale line along
                    // the join; a bitmap's edges are exactly where it stops being
                    // opaque and must never be grown. See `fill_shape_paint_sealed`.
                    sealed: !fill.paint.is_image(),
                },
                ctx,
                cache,
            );
        } else {
            // The light reaches the fill first: this is the tint that makes a warm
            // key look warm and a blue sky fill look cold, before any geometry is
            // drawn on top of it. A gradient takes it stop by stop, so a lit ramp
            // stays a ramp.
            //
            // Under a lamp, a solid fill takes the light as a **ramp** rather
            // than as one colour: that is what puts the falloff on the pixels
            // instead of on the shape as a whole, and it costs one gradient and
            // no extra geometry.
            let lamp_fill = match (&field, &fill.paint) {
                (Some(f), buzz_scene::Paint::Solid(c)) => lamp_lit(f, *c, |i, c| i.apply(c)),
                _ => None,
            };
            let base = match (&lamp_fill, &light) {
                (Some((paint, _)), _) => paint.clone(),
                (None, Some(light)) => fill.paint.map_colors(|c| light.apply(c)),
                (None, None) => fill.paint.clone(),
            };
            // A lamp's ramp lives where the lamp stands, in document space, so
            // it takes the projection alone rather than the object's placement.
            let brush_to_doc = match &lamp_fill {
                Some((_, disc)) => ctx.brush_projection(*disc),
                None => brush_to_doc,
            };
            let paint = ctx.paint(&base);
            let color = match (&light, fill.paint.is_image()) {
                // **A blurred bitmap is drawn as one colour, so the light has a
                // colour to reach after all.** The blur below replaces the fill
                // with a soft stack of copies of a single tone — the image's
                // own mean — and `map_colors` left that tone unlit along with
                // the pixels it came from. Here there is nothing to composite
                // over, and nothing needs to be: it is a colour, so it takes
                // the light the way every other colour does.
                (Some(light), true) => light.apply(paint.color()),
                _ => paint.color(),
            };

            // A blur replaces the fill rather than joining it: the artwork *is*
            // the soft stack of copies, and drawing the sharp shape as well would
            // put a hard edge back in the middle of it.
            if let Some((rx, ry, quality)) = ctx.blur {
                let ops = cache.filters.blur(
                    owner,
                    index,
                    &shape.path,
                    color,
                    (rx, ry),
                    quality,
                    builder.tolerance(),
                );
                crate::filters::draw_ops(builder, &ops, &ctx.projection.pre_affine(doc), 1.0);
                if let Some(stroke) = &shape.stroke {
                    // A stroke under a blur is softened as its own outline, so a
                    // blurred drawing keeps its lines instead of losing them.
                    // `ctx.stroke_scale` in practice is always 1.0 here — a
                    // filtered shape makes its whole symbol ineligible for the
                    // stamp cache (`SymbolInfo::cacheable_content`) — but
                    // applied anyway so this cannot silently drift out of step
                    // with the drawn stroke below. See `DrawCtx::stroke_scale`.
                    let outline = buzz_geom::outline_stroke(
                        &shape.path,
                        buzz_geom::StrokeStyle::new(
                            (stroke.width * ctx.stroke_scale).max(f64::MIN_POSITIVE),
                        ),
                        builder.tolerance(),
                    );
                    let colour = ctx.colour(match &light {
                        Some(light) => light.apply(stroke.color()),
                        None => stroke.color(),
                    });
                    let ops =
                        buzz_fx::blur_ops(&outline, colour, (rx, ry), quality, builder.tolerance());
                    crate::filters::draw_ops(builder, &ops, &ctx.projection.pre_affine(doc), 1.0);
                }
                return;
            }
            // Build-up paint sums its opacity with what is under it. The enclosing
            // isolation group, opened by `draw_frame`, is what keeps that sum away
            // from the stage behind it.
            if shape.blend.is_additive() {
                // Build-up paint is *summing* into an isolation group, so growing
                // the edge would add to the accumulation there rather than merely
                // covering a seam — a darker rim round every stroke, which is the
                // one thing that mode exists to avoid.
                builder.fill_shape_paint_additive(&path, &paint, brush_to_doc);
            } else if cache.detail().seals() {
                // Sealed: artwork that shares a boundary with the shape beside it
                // must not leave a pale line along the join. See
                // `SceneBuilder::fill_shape_paint_sealed`.
                builder.fill_shape_paint_sealed_sized(&path, &paint, brush_to_doc, extent);
            } else {
                // `LightDetail::Unsealed`: a frame whose own artwork is too
                // dense gives up the seal's doubled segments rather than not
                // drawing at all. A hairline seam at an abutting edge is a
                // smaller loss than the frame.
                builder.fill_shape_paint(&path, &paint, brush_to_doc);
            }

            // Then the modelling: the terminator away from the key light, and the
            // glint towards it. Both are the artwork's own outline, offset, which
            // is what makes them read as form rather than as a filter over it.
            // `detail` first, and it is the cheapest of the four: below
            // `Full` the crescents are neither built nor drawn, so a frame that
            // cannot afford them does not pay the booleans either. See
            // [`LightDetail`].
            if cache.detail().models()
                && let Some(light) = &light
                && let Some(key) = ctx.lights.key()
                && let Some((towards, lz)) = match ctx.lights.model {
                    buzz_scene::ShadeModel::Form => {
                        buzz_light::crescent_aim(key, here, ctx.layer_depth, ctx.shape_edges)
                            .map(|aim| (aim.planar, Some(aim.lz)))
                    }
                    buzz_scene::ShadeModel::Crescent => buzz_light::crescent_direction(
                        key,
                        here,
                        ctx.layer_depth,
                        ctx.shape_edges,
                    )
                    .map(|towards| (towards, None)),
                }
            {
                // `towards` above is which way the light lies from here — all a
                // crescent takes from a light, and `None` when it draws none:
                // modelling off, or a light straight on, which has no direction in
                // the plane to shade along.
                //
                // `placed` is handed over rather than the shape, because the draw
                // above already put the path in document space. Transforming it a
                // second time inside the cache was a whole path copy per shape per
                // frame, paid on every *hit*, which is most of what lighting used
                // to cost once the geometry itself had settled.
                let modelling = ctx.shape_edges;
                let geometry = cache.lights.crescents(
                    owner,
                    index,
                    &placed,
                    doc,
                    towards,
                    lz,
                    key.softness,
                );

                // The crescents take the fill's paint stop by stop as well, so the
                // shaded side of a gradient-filled shape is that gradient darkened
                // rather than one flat colour laid over it.
                //
                // **Feathered where the fill is one colour**, which is most
                // artwork. A band filled with a single tone puts a hard edge
                // across the drawing wherever the terminator falls — on a shape
                // the size of a background that is a straight seam through the
                // middle of the picture, and it reads as a join rather than as
                // shading. Ramping from the shaded tone at the band's outer edge
                // to the lit one at the terminator makes the width `softness`
                // asks for the width the gradient actually takes, at no extra
                // layer: it is the same one fill, with a ramp in it.
                //
                // A fill made of a ramp or of pixels has no single pair of
                // colours to run between, so it keeps the flat band. A bitmap
                // does not come here at all — it is composited, and feathered
                // there.
                // Each band is **one fill**, as the artwork itself is. Under a
                // lamp it takes the same radial ramp the fill did, so the shaded
                // side of a wall darkens along the wall rather than taking one
                // value for the whole of it.
                if let Some(shade) = &geometry.shade {
                    let drawn = ctx.project(shade, builder.tolerance());
                    // **How sharply the shade arrives.** A hard light's
                    // terminator is a line: one step from lit to shaded, no
                    // ramp. A soft one arrives over the width of the band. At a
                    // softness of zero this is zero and the flat fill below is
                    // taken, which is exactly the hard edge asked for.
                    let feather = buzz_light::shade_feather(key.softness);
                    let band = shade.bounding_box();
                    match (&field, &fill.paint) {
                        (Some(f), buzz_scene::Paint::Solid(c))
                            if let Some((ramp, disc)) =
                                lamp_lit(f, *c, |i, c| i.apply_shaded(c)) =>
                        {
                            builder.fill_shape_paint(&drawn, &ramp, ctx.brush_projection(disc));
                        }
                        // **Feathered where the fill is one colour**, which is
                        // most artwork: full shade held across the band's outer
                        // part and falling to the lit colour the picture
                        // already is at the terminator, so the band ends in
                        // nothing rather than on a line. The ramp runs *away*
                        // from the light, which is the direction the band was
                        // built in.
                        (_, buzz_scene::Paint::Solid(c))
                            if feather > 1e-3
                                && let Some(ramp) = banded_ramp(
                                    light.apply_shaded(*c),
                                    light.apply(*c),
                                    band,
                                    -towards,
                                    1.0 - feather,
                                ) =>
                        {
                            builder.fill_shape_paint(&drawn, &ramp, ctx.brush_projection(band));
                        }
                        _ => {
                            let shaded =
                                ctx.paint(&fill.paint.map_colors(|c| light.apply_shaded(c)));
                            builder.fill_shape_paint(&drawn, &shaded, brush_to_doc);
                        }
                    }
                }
                if let Some(highlight) = &geometry.highlight {
                    let drawn = ctx.project(highlight, builder.tolerance());
                    // **How hard this light catches an edge.** At full the band
                    // is a wet, polished sheen; a drawing is usually matte, and
                    // laying the full glint on all of it was half of why a lit
                    // figure read as having a second, whiter drawing pasted
                    // over one side. See `buzz_light::Light::glint`.
                    let strength = modelling * key.glint();
                    let band = highlight.bounding_box();
                    match (&field, &fill.paint) {
                        (Some(f), buzz_scene::Paint::Solid(c))
                            if let Some((ramp, disc)) =
                                lamp_lit(f, *c, |i, c| i.highlight(c, key.color, strength)) =>
                        {
                            builder.fill_shape_paint(&drawn, &ramp, ctx.brush_projection(disc));
                        }
                        // **Feathered where the fill is one colour**, which is
                        // most artwork: the glint at the outer edge, fading to
                        // the lit colour the picture underneath already is, so
                        // the band ends in nothing rather than on a line. Both
                        // ends are opaque, so the brightest part of the band is
                        // exactly the colour the flat one was.
                        (_, buzz_scene::Paint::Solid(c))
                            if let Some(ramp) = glint_ramp(
                                light.highlight(*c, key.color, strength),
                                light.apply(*c),
                                band,
                                towards,
                            ) =>
                        {
                            builder.fill_shape_paint(&drawn, &ramp, ctx.brush_projection(band));
                        }
                        // A fill made of a ramp or of pixels has no single pair
                        // of colours to run between, so it keeps the flat band.
                        _ => {
                            let glint = ctx.paint(
                                &fill
                                    .paint
                                    .map_colors(|c| light.highlight(c, key.color, strength)),
                            );
                            builder.fill_shape_paint(&drawn, &glint, brush_to_doc);
                        }
                    }
                }
            }

            // **The dark edge a wall of dark leaves on a figure standing in
            // it**, which is the other half of what makes light read as light.
            //
            // The tint and the band overhead say *how much* light there is; an
            // edge says which way it is coming from, and without one a gloom is
            // a wash over the picture that takes every figure's form with it.
            //
            // Built through the same cache the terminator uses, keyed on the
            // gloom's own direction rather than the key light's — see
            // `draw_gloom_edge`. One more outline per shape, which is what the
            // rest of lighting costs and what `encode_cost` allows.
            draw_gloom_edge(builder, ctx, cache, owner, index, &placed, doc, here);
        }
    }

    if let Some(stroke) = &shape.stroke {
        // A stroke is lit but never shaded: a terminator across a one-pixel
        // line is noise, and an outline that changed width with the light
        // would read as a drawing mistake.
        let base = match &light {
            Some(light) => stroke.paint.map_colors(|c| light.apply(c)),
            None => stroke.paint.clone(),
        };
        let paint = ctx.paint(&base);
        if stroke.hairline {
            // A hairline is one pixel wide at every zoom, so its width is set
            // in screen space rather than document space and it cannot go
            // through the paint path. One pixel of a ramp is one colour anyway.
            builder.stroke_hairline(&path, paint.color(), 1.0);
        } else {
            // **Scaled with its placement**, as Animate draws a
            // `scaleMode="normal"` stroke (every stroke barish1 has): a symbol
            // shrunk to a tenth has outlines a tenth as wide. See
            // [`stroke_placement_scale`].
            builder.stroke_shape_paint(
                &path,
                &paint,
                stroke.width * stroke_placement_scale(doc) * ctx.stroke_scale,
                brush_to_doc,
            );
        }
    }
}

/// One shape lit by **compositing** rather than by recolouring, and the light
/// falling on it. See [`draw_lit_composited`].
struct LitShape<'a> {
    owner: Option<&'a Arc<Object>>,
    index: u16,
    fill: &'a buzz_scene::FillSpec,
    blend: buzz_scene::PaintBlend,
    /// The shape in **document** space — what the crescents are measured and
    /// built from, and what the cache is keyed on.
    /// Its longest side as drawn; see `SceneBuilder::fill_shape_paint_sealed`.
    extent: f64,
    placed: &'a buzz_geom::BezPath,
    /// The same shape where the lens puts it — what is actually drawn.
    path: &'a buzz_geom::BezPath,
    /// The middle of the shape in document space, which is where the light is
    /// evaluated.
    here: buzz_geom::Point,
    doc: Affine,
    brush_to_doc: Affine,
    /// The light over this shape — a ramp when a lamp falls off across it.
    field: &'a buzz_light::LightField,
    /// Whether the fill may be grown half a pixel to close the seam it shares
    /// with its neighbour. True for artwork made of coloured regions, false for
    /// a bitmap, whose edges are exactly where it stops being opaque.
    sealed: bool,
}

/// Light a shape by **compositing the light over the picture** rather than
/// folding it into the paint.
///
/// # Why this is not the ordinary path
///
/// [`draw_shape`] lights artwork with `Paint::map_colors`, which rewrites every
/// colour a fill is made of. That is exactly right for a solid or a gradient
/// under a light that arrives the same way everywhere — a lit ramp stays a ramp.
/// Two things it cannot do, and both of them arrive here instead:
///
/// * **A bitmap has no list of colours to rewrite.** Its colours are its pixels,
///   and `map_colors` returns it untouched by design, because rewriting a
///   four-megapixel photograph per frame would cost more than drawing it. So the
///   lit path ran over bitmaps and changed nothing about them.
/// * **A rewritten colour is one colour**, and a lamp is not. A lamp's light
///   falls off, and the falloff *is* the lamp; a shape whose fill was recoloured
///   by the light at its middle showed none of it, so a wall under a lamp came
///   out flat and the near side of a face was as bright as the far side. Laid
///   over the picture the light is a *paint*, and a lamp's paint is its own
///   radial falloff — see [`buzz_light::LightRig::field`]. The gradient lands the
///   light per pixel, which is the whole difference between a lamp that lights
///   the artwork and one that merely tints it.
///
/// Everything else is still recoloured, and encodes exactly what it always did:
/// a rig of suns and skies delivers one colour over any one shape, and paying
/// for a group and a gradient to say so would be waste.
///
/// # What is drawn, and why it lands on the same colours
///
/// The same three things, in the same order as the vector path:
///
/// 1. The picture, unlit, then the light laid over the whole of it — a multiply
///    by the light up to full, and a screen for anything above it. Multiplying
///    is what [`buzz_light::Illumination::apply`] does arithmetically, so this
///    is the same tint arrived at per pixel.
/// 2. The shaded crescent, carrying only the **ratio** from lit to ambient —
///    the picture beneath it is already lit, and multiplication composes, so it
///    lands on the ambient colour exactly.
/// 3. The highlight crescent, the light's own colour laid on at the `t` the
///    vector path mixes with.
///
/// # Isolated, and bounded
///
/// Every pass composes `SrcAtop` so it keeps the picture's alpha and cannot
/// paint into the transparent parts of a cut-out. That makes the *backdrop*
/// matter: outside a group the backdrop is everything already drawn, so the
/// light would tint the stage showing through the cut-out's own corners. The
/// group is bounded by what is drawn into it, because a group is a render
/// target and an unbounded one costs a full-viewport buffer.
///
/// # Where the two paths part company, and by how much
///
/// The lit body and the shaded band land on the vector path's colours exactly —
/// measured at 174 against 174 and 81 against 83 on a mid grey under the
/// default sun. Two things do differ, both in the highlight, both by a few
/// levels:
///
/// * **The glint mixes in the compositor's space, not in linear light.**
///   [`buzz_light::mix`] blends towards the light in linear light, because an
///   sRGB midpoint between two colours looks too dark. Laid over pixels the
///   blend is the compositor's, which is not linear, so the glint comes out
///   slightly the darker of the two.
/// * **The two crescents overlap at a corner, and there the vector path lets
///   the highlight win outright.** It is drawn second and opaque, so it
///   replaces the shade. A blend cannot replace: it composes with what is under
///   it, so at that corner the glint sits on the shaded pixel rather than the
///   lit one and is dimmer for it. Making them agree means subtracting one
///   crescent from the other, and that is a third boolean on every build of
///   every shape — paid on all artwork, to move two corners of a bitmap by a
///   few levels. Not taken; recorded here so the next reader knows it was
///   weighed rather than missed.
fn draw_lit_composited(
    builder: &mut SceneBuilder<'_>,
    it: LitShape<'_>,
    ctx: &DrawCtx<'_>,
    cache: &mut DrawCache,
) {
    let LitShape {
        owner,
        index,
        fill,
        blend,
        extent,
        placed,
        path,
        here,
        doc,
        brush_to_doc,
        field,
        sealed,
    } = it;

    let paint = ctx.paint(&fill.paint);
    let light = field.uniform();

    // A rig that is on but delivering full daylight must encode exactly what an
    // unlit document encodes: no group, no passes, no crescents.
    if field.is_neutral() {
        if blend.is_additive() {
            builder.fill_shape_paint_additive(path, &paint, brush_to_doc);
        } else if sealed && cache.detail().seals() {
            builder.fill_shape_paint_sealed_sized(path, &paint, brush_to_doc, extent);
        } else {
            builder.fill_shape_paint(path, &paint, brush_to_doc);
        }
        return;
    }

    // The crescents are worked out **before** anything is drawn, because the
    // group opened below has to be big enough to hold them: a crescent is the
    // artwork's own outline offset, so it can reach a little outside the shape.
    let modelling = ctx.shape_edges;
    let key = ctx.lights.key();
    let geometry = key
        .and_then(|key| {
            let (towards, lz) = match ctx.lights.model {
                buzz_scene::ShadeModel::Form => {
                    let aim = buzz_light::crescent_aim(key, here, ctx.layer_depth, modelling)?;
                    (aim.planar, Some(aim.lz))
                }
                buzz_scene::ShadeModel::Crescent => {
                    let towards =
                        buzz_light::crescent_direction(key, here, ctx.layer_depth, modelling)?;
                    (towards, None)
                }
            };
            Some((key, towards, lz))
        })
        .map(|(key, towards, lz)| {
            (
                key,
                // Which way the light lies from here, kept so the crescents can
                // be feathered along it below.
                towards,
                cache
                    .lights
                    .crescents(owner, index, placed, doc, towards, lz, key.softness),
            )
        });

    let shade = geometry
        .as_ref()
        .and_then(|(_, _, g)| g.shade.as_ref())
        .map(|s| ctx.project(s, builder.tolerance()));
    let highlight = geometry
        .as_ref()
        .and_then(|(_, _, g)| g.highlight.as_ref())
        .map(|h| ctx.project(h, builder.tolerance()));

    let mut bounds = path.bounding_box();
    for extra in [shade.as_ref(), highlight.as_ref()].into_iter().flatten() {
        bounds = bounds.union(extra.bounding_box());
    }

    // **The isolation group, when it is needed.**
    //
    // Every pass below composes `SrcAtop` so it keeps the picture's alpha and
    // cannot paint into the transparent parts of a cut-out. That makes the
    // *backdrop* matter: outside a group the backdrop is everything already
    // drawn, so the light would tint the stage showing through a cut-out's own
    // corners.
    //
    // An **opaque** fill has no such corners. Each pass is clipped to the shape
    // or to a band inside it, under the same fill rule the artwork was drawn
    // with, so the backdrop everywhere a pass can reach is that artwork and
    // nothing else. Skipping the group there takes one render target off every
    // solidly-filled lit shape, which is most of a drawing.
    let isolate = !crate::is_opaque(&fill.paint) || blend.is_additive();
    if isolate {
        builder.push_isolation(bounds);
    }

    if blend.is_additive() {
        builder.fill_shape_paint_additive(path, &paint, brush_to_doc);
    } else if sealed && cache.detail().seals() {
        // Artwork made of coloured regions shares a boundary with the artwork
        // beside it, and must not leave a pale line along the join.
        builder.fill_shape_paint_sealed_sized(path, &paint, brush_to_doc, extent);
    } else {
        // **Never sealed.** A bitmap's interesting edges are exactly where it
        // stops being opaque, and growing it half a pixel smears those border
        // pixels outwards — the fringe that makes a composite look pasted on.
        // `fill_shape_paint_sealed` already refuses an image for this reason;
        // saying so here keeps the two from drifting apart.
        builder.fill_shape_paint(path, &paint, brush_to_doc);
    }

    // **The light over the whole shape**: one multiply by what arrives, and a
    // screen for anything above full. A ramp when a lamp falls off across the
    // shape — which is what puts the falloff on the pixels rather than on the
    // shape as a whole — and one flat colour when nothing varies, in which case
    // this encodes exactly what it always did.
    let lamp = field
        .disc()
        .map(|(centre, reach)| cache.lamp_paints(field, centre, reach));
    match &lamp {
        None => {
            let filter = light.as_filter(true);
            builder.fill_shape_atop(path, filter.multiply, buzz_fx::Blend::Multiply);
            if let Some(screen) = filter.screen {
                builder.fill_shape_atop(path, screen, buzz_fx::Blend::Screen);
            }
        }
        Some(lamp) => {
            let to_doc = ctx.brush_projection(lamp.disc);
            builder.fill_shape_atop_paint(path, &lamp.multiply, to_doc, buzz_fx::Blend::Multiply);
            if let Some(screen) = &lamp.screen {
                builder.fill_shape_atop_paint(path, screen, to_doc, buzz_fx::Blend::Screen);
            }
        }
    }

    // Then the modelling, **feathered**. A crescent filled with one flat colour
    // puts a hard edge across the artwork wherever the terminator falls: on a
    // shape the size of a background that is a straight seam through the middle
    // of the picture, and it reads as a join rather than as shading. The ramp
    // runs along the light's own direction, from the full tone at the far edge
    // to nothing at the terminator, so the width `softness` asks for is the
    // width the gradient actually takes.
    // The two bands, each **one fill** laid over the picture. The shaded side
    // carries only the *ratio* from lit to ambient — the picture beneath it is
    // already lit, and multiplication composes, so it lands on the ambient
    // colour exactly — and takes it as a ramp of the light field rather than as
    // one colour, so a band lying across a lamp's falloff darkens with it.
    lay_bands(
        builder,
        ctx,
        Bands {
            shade: shade.as_ref(),
            highlight: highlight.as_ref().zip(
                geometry
                    .as_ref()
                    .and_then(|(_, _, g)| g.highlight.as_ref())
                    .map(|h| h.bounding_box()),
            ),
            towards: geometry.as_ref().map(|(_, t, _)| *t),
            key: geometry.as_ref().map(|(k, _, _)| *k),
            lamp: lamp.as_deref(),
            light: &light,
            modelling,
        },
    );

    if isolate {
        builder.pop_isolation();
    }
}

/// The two bands a key light lays over artwork **that is already drawn**, and
/// everything needed to colour them.
struct Bands<'a> {
    /// The shaded crescent, projected. `None` where there is none to draw.
    shade: Option<&'a buzz_geom::BezPath>,
    /// The glint, projected, with its **document-space** bounds — which is the
    /// space the ramp along the light's direction is built in.
    highlight: Option<(&'a buzz_geom::BezPath, buzz_geom::Rect)>,
    /// Which way the light lies, for the ramp.
    towards: Option<buzz_geom::Vec2>,
    key: Option<&'a buzz_light::Light>,
    /// The lamp's gradients, when one falls off across this artwork.
    lamp: Option<&'a LampPaints>,
    /// The light over the artwork as one value, for everything the ramp does
    /// not answer.
    light: &'a buzz_light::Illumination,
    modelling: f32,
}

/// **Lay the shaded side and the glint over artwork already on the frame.**
///
/// Shared by the two things that light by compositing rather than by
/// recolouring — a bitmap or a lamp-lit shape ([`draw_lit_composited`]) and a
/// whole figure ([`FigurePass`]) — so the two cannot drift into drawing the
/// same light two different ways.
///
/// The shaded band carries only the **ratio** from lit to ambient: the picture
/// beneath it is already lit and multiplication composes, so it lands on the
/// ambient colour exactly. The glint is **screened**, not laid over: light
/// falling on the artwork adds to the colour that is there rather than
/// replacing it, so a red coat catching a warm lamp stays red. See
/// [`buzz_light::Illumination::highlight_strength`], which the vector path
/// mixes with to the same place.
fn lay_bands(builder: &mut SceneBuilder<'_>, ctx: &DrawCtx<'_>, it: Bands<'_>) {
    if let Some(drawn) = it.shade {
        // The same feather the vector path lays, for the same reason: a hard
        // light's terminator is a line and a soft one's is a gradient, and the
        // two routes must not disagree about which. Multiplying by white is
        // multiplying by nothing, so the ramp runs from the shade filter at the
        // band's outer edge to white at the terminator.
        let feather = it.key.map(|k| buzz_light::shade_feather(k.softness));
        match (it.lamp, feather, it.towards) {
            (None, Some(feather), Some(towards)) if feather > 1e-3 => {
                let band = drawn.bounding_box();
                match banded_ramp(
                    it.light.shade_filter(),
                    Color::WHITE,
                    band,
                    -towards,
                    1.0 - feather,
                ) {
                    Some(paint) => builder.fill_shape_atop_paint(
                        drawn,
                        &paint,
                        ctx.brush_projection(band),
                        buzz_fx::Blend::Multiply,
                    ),
                    None => builder.fill_shape_atop(
                        drawn,
                        it.light.shade_filter(),
                        buzz_fx::Blend::Multiply,
                    ),
                }
            }
            (None, _, _) => {
                builder.fill_shape_atop(drawn, it.light.shade_filter(), buzz_fx::Blend::Multiply)
            }
            (Some(lamp), _, _) => builder.fill_shape_atop_paint(
                drawn,
                &lamp.shade,
                ctx.brush_projection(lamp.disc),
                buzz_fx::Blend::Multiply,
            ),
        }
    }
    if let (Some((drawn, band)), Some(key), Some(towards)) = (it.highlight, it.key, it.towards) {
        let strength = buzz_light::Illumination::highlight_strength(it.modelling * key.glint());
        let glint = key.color.multiply_alpha(strength);
        // Feathered exactly as the vector path's is — see `glint_ramp`. Here
        // the fade is in the *alpha*, because a photograph has no fill colour
        // to fade towards: the ramp goes from the glint to the same glint at
        // nothing, which is the picture underneath, untouched.
        let ramp = glint_ramp(glint, glint.multiply_alpha(0.0), band, towards);
        match ramp {
            Some(paint) => builder.fill_shape_atop_paint(
                drawn,
                &paint,
                ctx.brush_projection(band),
                buzz_fx::Blend::Screen,
            ),
            None => builder.fill_shape_atop(drawn, glint, buzz_fx::Blend::Screen),
        }
    }
}

/// **How much of the highlight stays at the full glint** before it starts to
/// fall away, as a fraction of the way across the lit side of the shape.
///
/// Not zero. A ramp that starts at the very outer edge has its brightest value
/// on one line of pixels, so the glint reads dimmer than the number asks for
/// and turning the slider up only widens the fade. A quarter at full, then a
/// fall across the rest, keeps the glint where it was and puts the softness
/// where the flat stripe used to be.
const GLINT_HOLD: f64 = 0.26;

/// **The glint, falling off around the form instead of stopping flat.**
///
/// # What was wrong with the flat band
///
/// The highlight crescent is the artwork's outline minus a copy of itself
/// shifted towards the light, and it was filled with **one tone, edge to
/// edge**. A real highlight is not like that: it is brightest where the
/// surface faces the light and dies away around the curve. Filled flat, the
/// band is a stripe of even brightness ending on a hard line, and on a face or
/// a limb it reads as a whiter drawing pasted over one side of the artwork
/// rather than as light falling on it. That is the report this comes from.
///
/// So the band is filled with a **ramp along the light's own direction**: full
/// where the shape faces the light, held for [`GLINT_HOLD`] of the way across,
/// then falling to `inner` at the far side. `inner` is whatever the picture
/// underneath already is, so the band dies into it rather than stopping.
///
/// **The same one fill**, with a gradient in it — no extra layer, no extra
/// path, no second boolean. The crescents are the most expensive thing the
/// renderer builds, and softening a light must not cost another one.
///
/// # Why it runs across the shape rather than across the band
///
/// The first version ramped over the band's own thickness, from the outline in
/// to the terminator. It is the more obvious reading of "feather the edge" and
/// it does not work: the band curls around the lit side of the shape, so its
/// outer edge is a *curve*, and a linear ramp anchored to the far corner of
/// its bounding box misses the band nearly everywhere — leaving the whole
/// highlight filled with the colour it was meant to fade to, which measures as
/// a light that has stopped putting any colour on the picture at all. A band
/// that wraps cannot be feathered along its own normal by a linear gradient.
/// It can be dimmed along its length by one, and that is the falloff the eye
/// is actually looking for.
///
/// `towards` points from the artwork towards the light and `band` is the
/// crescent's bounds in document space. `None` when there is no direction to
/// ramp along or no extent to ramp over; the caller then draws the flat band,
/// which is what it always drew.
fn glint_ramp(
    outer: Color,
    inner: Color,
    band: buzz_geom::Rect,
    towards: buzz_geom::Vec2,
) -> Option<buzz_scene::Paint> {
    banded_ramp(outer, inner, band, towards, GLINT_HOLD)
}

/// [`glint_ramp`], with the length of the flat part named by the caller.
///
/// `hold` is how much of the band stays at `outer` before it starts to fall
/// away, as a fraction of the band. The glint's is fixed; the shaded side's
/// comes from the light's softness, and at a softness of zero it is **1.0** —
/// the whole band at full tone, which is a terminator with no ramp in it at
/// all. That is what a hard light is. See [`buzz_light::shade_feather`].
fn banded_ramp(
    outer: Color,
    inner: Color,
    band: buzz_geom::Rect,
    towards: buzz_geom::Vec2,
    hold: f64,
) -> Option<buzz_scene::Paint> {
    let length = towards.hypot();
    if !length.is_finite() || length < 1e-9 {
        return None;
    }
    let dir = towards / length;

    // How far the band reaches from its own centre towards the light: the
    // support of its bounding box in that direction. The box rather than the
    // outline, because the support of a rectangle is two multiplications and
    // the support of a bezier outline is not — and the two agree where it
    // matters, which is the extreme.
    let centre = band.center();
    let support = (band.width() * dir.x).abs() * 0.5 + (band.height() * dir.y).abs() * 0.5;
    if !support.is_finite() || support <= 1e-6 {
        return None;
    }

    // Unit space runs from (-1, 0) to (1, 0), so the ramp's axis points *away*
    // from the light: offset 0 is the side facing it, offset 1 the far side.
    let axis = -dir;
    let half = support;
    let mid = centre;

    let mut gradient = buzz_scene::Gradient::new(
        buzz_scene::GradientKind::Linear,
        vec![
            buzz_scene::GradientStop::new(0.0, outer),
            buzz_scene::GradientStop::new(hold.clamp(0.0, 1.0), outer),
            buzz_scene::GradientStop::new(1.0, inner),
        ],
    );
    gradient.transform = Affine::translate(mid.to_vec2())
        * Affine::rotate(axis.y.atan2(axis.x))
        * Affine::scale_non_uniform(half, half);
    Some(buzz_scene::Paint::Gradient(Arc::new(gradient)))
}

/// The light across a lamp's reach, as a radial gradient in document space.
///
/// One stop per sample of the lamp's falloff, each turned into a colour to
/// composite by `tone` — the multiply factor, the screen overflow, whichever
/// pass is being laid. `disc` is the lamp's reach as a rectangle, which is what
/// puts the unit gradient where the lamp stands.
fn radial_light(
    ramp: &[(f64, buzz_light::Illumination)],
    disc: buzz_geom::Rect,
    tone: impl Fn(&buzz_light::Illumination) -> Color,
) -> buzz_scene::Paint {
    let stops: Vec<buzz_scene::GradientStop> = ramp
        .iter()
        .map(|(at, light)| buzz_scene::GradientStop::new(*at, tone(light)))
        .collect();
    let mut gradient = buzz_scene::Gradient::new(buzz_scene::GradientKind::Radial, stops);
    gradient.fit_to(disc);
    buzz_scene::Paint::Gradient(Arc::new(gradient))
}

/// **A solid colour lit by a lamp, as one paint.**
///
/// A lamp's light is radially symmetric about the point it stands over — see
/// [`buzz_light::Light::direct_at_radius`] — so a solid fill under one is
/// *exactly* a radial gradient of that colour, lit at each radius. Folding the
/// light into the paint this way is what puts the falloff on the pixels while
/// the shape is still drawn **once**.
///
/// # Why that matters more than it sounds
///
/// The obvious alternative is to draw the artwork and lay the light over it,
/// which is what a bitmap has to do. Vello re-encodes a path for every fill, and
/// there is no instancing: laying the light over the artwork costs that shape's
/// whole outline a second time, a third for a screen pass, and one more for
/// every band. Measured on a 28-layer Animate import — 615 thousand path
/// segments unlit — compositing every lit shape took the scene to **11.5
/// million**, and its path data from 9 MB to 171 MB, past the 128 MB a buffer
/// may bind. The GPU refused the frame and the process went with it.
///
/// `None` when no lamp varies across the region, which is every rig of suns and
/// skies: there is nothing to ramp and the caller keeps its solid colour.
fn lamp_lit(
    field: &buzz_light::LightField,
    base: Color,
    tone: impl Fn(&buzz_light::Illumination, Color) -> Color,
) -> Option<(buzz_scene::Paint, buzz_geom::Rect)> {
    let (centre, reach) = field.disc()?;
    let disc = buzz_geom::Circle::new(centre, reach).bounding_box();
    Some((
        radial_light(&field.ramp(), disc, |light| tone(light, base)),
        disc,
    ))
}

/// **A ramping lamp's three passes, built once and shared.**
///
/// The ramp spans the lamp rather than the shape (see
/// [`buzz_light::LightRig::field`]), so every shape one lamp reaches asks for
/// exactly these gradients. Building them per shape cost three allocations each
/// — measured at half the cost of drawing four hundred lit shapes — and produced
/// three identical answers over and over.
#[derive(Debug)]
struct LampPaints {
    /// Where the gradients sit, for the brush projection.
    disc: buzz_geom::Rect,
    /// The light up to full.
    multiply: buzz_scene::Paint,
    /// Whatever it has above full. `None` where it never exceeds it, which is
    /// most lamps over most of their reach.
    screen: Option<buzz_scene::Paint>,
    /// The step from lit to shaded, for the terminator.
    shade: buzz_scene::Paint,
}

impl LampPaints {
    fn build(field: &buzz_light::LightField, centre: buzz_geom::Point, reach: f64) -> Self {
        let disc = buzz_geom::Circle::new(centre, reach).bounding_box();
        let ramp = field.ramp();
        Self {
            disc,
            multiply: radial_light(&ramp, disc, |i| i.as_filter(true).multiply),
            // A second pass only where the lamp is actually brighter than full.
            screen: ramp
                .iter()
                .any(|(_, i)| i.as_filter(true).screen.is_some())
                .then(|| {
                    radial_light(&ramp, disc, |i| {
                        i.as_filter(true).screen.unwrap_or(Color::BLACK)
                    })
                }),
            shade: radial_light(&ramp, disc, |i| i.shade_filter()),
        }
    }
}

/// Draw every shadow this object throws, before the layer's artwork.
///
/// `shadow` is the projection worked out once for the whole layer by
/// [`buzz_light::shadow_transform`]: a shadow is the caster's own outline under
/// one affine, so there is nothing here to build and nothing to cache. That is
/// the change that lets a light be dragged — the shadows follow it exactly, on
/// every frame, however heavy the artwork, while the crescents catch up behind.
fn cast_shadows(
    builder: &mut SceneBuilder<'_>,
    object: &Object,
    doc: Affine,
    shadow: Affine,
    ctx: &DrawCtx<'_>,
) {
    let mut caster = Caster::default();
    cast_shadows_within(builder, &mut caster, object, doc, shadow, ctx, 0);
    caster.draw(builder);
}

/// **Where a layer's shadows land**, as the group they are drawn inside.
///
/// The union of what the layer actually draws, each caster thrown the way the
/// light throws *it*, and then through the lens. Every group is a render target, so it is trimmed to
/// what can be seen: a caster whose shadow falls right off the side of the
/// frame should not buy a buffer the size of the throw.
///
/// **Resolved bounds, not the object's own.** An instance's `bounds` is a
/// placeholder a few units across — the symbol's real extent lives in the
/// library — so measuring the layer that way gave a group a hair wide and
/// clipped every shadow in the document to nothing. `Scene::resolved_bounds`
/// is memoised across the library, so asking it per object is a lookup.
///
/// `None` when the layer draws nothing, or nothing whose shadow can be seen.
fn shadow_group_bounds(
    resolved: &buzz_scene::ResolvedFrame<'_>,
    shadow: &buzz_light::ShadowThrow,
    ctx: &DrawCtx<'_>,
    bounds: &mut BoundsCache,
) -> Option<buzz_geom::Rect> {
    let mut area: Option<buzz_geom::Rect> = None;
    for (object, owner) in resolved.iter_owned() {
        // Each caster thrown by **its own** affine, and the union of those: a
        // ground shadow is anchored at the feet of the figure that casts it, so
        // one box thrown once would be the shadow of the whole layer treated as
        // a single enormous caster.
        let b = caster_bounds(object, owner, ctx, bounds);
        let thrown = shadow.at(b).transform_rect_bbox(b);
        area = Some(match area {
            Some(a) => a.union(thrown),
            None => thrown,
        });
    }
    let mut area = area?;
    if let Some(cull) = ctx.cull {
        area = area.intersect(cull);
    }
    if !(area.width() > 0.0 && area.height() > 0.0) {
        return None;
    }
    ctx.projection.map_rect_bounds(area)
}

/// One caster's resolved bounds: from the [`BoundsCache`] when the object is a
/// stored one (a held rig is a lookup), measured through the scene otherwise.
/// Either way the number `Scene::resolved_bounds` gives.
fn caster_bounds(
    object: &Object,
    owner: Option<&Arc<Object>>,
    ctx: &DrawCtx<'_>,
    bounds: &mut BoundsCache,
) -> buzz_geom::Rect {
    match owner {
        Some(owner) => bounds.resolved(owner, ctx.scene),
        None => ctx.scene.resolved_bounds(object),
    }
}

/// **A layer's shadow pass**: the alpha group its shadows are drawn into, every
/// caster's silhouette thrown by the light inside it, and the group closed.
/// Shared by the live pass and the one a [`ShadowMemo`] fragment is encoded
/// with, so the two cannot differ.
fn run_shadow_pass(
    builder: &mut SceneBuilder<'_>,
    resolved: &buzz_scene::ResolvedFrame<'_>,
    shadow: &buzz_light::ShadowThrow,
    cast: buzz_geom::Rect,
    alpha: f32,
    ctx: &DrawCtx<'_>,
    bounds: &mut BoundsCache,
) {
    builder.push_alpha_group(cast, alpha);
    for (object, owner) in resolved.iter_owned() {
        // **Per caster**, because a shadow on the ground is anchored at the
        // feet of the thing that casts it - see [`buzz_light::ShadowThrow`]. A
        // wall shadow answers the same affine whatever it is asked about, so
        // this costs it nothing.
        //
        // Resolved bounds, so a character symbol is measured through the
        // library rather than by its placeholder box; the memo makes it a
        // lookup.
        let thrown = shadow.at(caster_bounds(object, owner, ctx, bounds));
        cast_shadows(builder, object, Affine::IDENTITY, thrown, ctx);
    }
    builder.pop_isolation();
}

/// One part of a layer rig as its one shadow sees it (LS): the layer, what it
/// inherits from the parts it follows, what is on it now, and how far into its
/// own keyframe the frame is (what a symbol on it plays from).
struct RigPart<'a> {
    layer: &'a Arc<buzz_scene::Layer>,
    follows: Affine,
    resolved: buzz_scene::ResolvedFrame<'a>,
    elapsed: u32,
}

/// **A layer rig's one shadow** (LS): remembered like any layer's pass (see
/// [`ShadowMemo`]), kept under the rig's root layer, which never throws a pass
/// of its own.
#[allow(clippy::too_many_arguments)]
fn draw_rig_shadow(
    builder: &mut SceneBuilder<'_>,
    scene: &Scene,
    group: &RigShadow<'_>,
    parts: &[RigPart<'_>],
    shadow: &buzz_light::ShadowThrow,
    strength: f32,
    alpha: f32,
    ctx: &DrawCtx<'_>,
    // The bottom-most part's projection before its own parenting: the plane
    // every part's `follows` is carried into.
    placed: Projection,
    cache: &mut DrawCache,
) {
    // Each part through the plane, with its own place in its own keyframe.
    let ctxs: Vec<DrawCtx<'_>> = parts
        .iter()
        .map(|part| DrawCtx {
            elapsed: part.elapsed,
            projection: placed,
            ..ctx.clone()
        })
        .collect();
    let signature = if cache.shadow_memo.enabled {
        rig_shadow_memo_signature(
            scene,
            parts,
            &ctxs,
            group.depth,
            shadow,
            strength,
            alpha,
            builder,
            cache.detail(),
            &mut cache.animated,
        )
    } else {
        None
    };
    let remembered = signature
        .as_ref()
        .and_then(|(hash, _)| cache.shadow_memo.get(group.root, *hash));
    match (remembered, signature) {
        (Some((fragment, lines)), _) => {
            builder.target_scene().append(&fragment, None);
            builder.lines += lines;
        }
        (None, Some((hash, identity))) => {
            // Encoded through a builder with the frame's own view, as a single
            // layer's pass is; see the shadow pass in `draw_layer`.
            let mut fragment = crate::vello::Scene::new();
            let lines = {
                let mut child = SceneBuilder {
                    scene: &mut fragment,
                    split: builder.split,
                    visible: builder.visible,
                    clip: builder.clip,
                    lines: 0,
                    device_scale: builder.device_scale,
                    last_stats: PathLineStats::default(),
                    runs: Vec::new(),
                    spare_runs: Vec::new(),
                };
                run_rig_shadow_pass(&mut child, parts, &ctxs, shadow, alpha, &mut cache.bounds);
                child.lines
            };
            builder.target_scene().append(&fragment, None);
            builder.lines += lines;
            cache.shadow_memo.insert(
                group.root,
                hash,
                Arc::new(fragment),
                lines,
                identity,
                scene.library().clone(),
            );
        }
        (None, None) => {
            run_rig_shadow_pass(builder, parts, &ctxs, shadow, alpha, &mut cache.bounds);
        }
    }
}

/// **A layer rig's shadow pass**: every part's casters gathered in the plane,
/// the rig's box round all of them, one throw for that box — so a ground
/// shadow is anchored at the rig's feet, as the same character's armature
/// is — and every part's silhouette drawn by it into one alpha group.
///
/// A caster that draws nothing (an empty part) has no box to add: it would
/// only drag the rig's middle towards wherever its empty group happens to sit.
fn run_rig_shadow_pass(
    builder: &mut SceneBuilder<'_>,
    parts: &[RigPart<'_>],
    ctxs: &[DrawCtx<'_>],
    shadow: &buzz_light::ShadowThrow,
    alpha: f32,
    bounds: &mut BoundsCache,
) {
    let Some(plane) = ctxs.first() else {
        return;
    };
    let mut boxes: Vec<buzz_geom::Rect> = Vec::new();
    for (part, ctx) in parts.iter().zip(ctxs) {
        for (object, owner) in part.resolved.iter_owned() {
            let b = caster_bounds(object, owner, ctx, bounds);
            if b.width() <= 0.0 && b.height() <= 0.0 {
                continue;
            }
            boxes.push(part.follows.transform_rect_bbox(b));
        }
    }
    let Some(rig_box) = boxes.iter().copied().reduce(|a, b| a.union(b)) else {
        return;
    };
    let thrown = shadow.at(rig_box);
    let Some(mut area) = boxes
        .iter()
        .map(|b| thrown.transform_rect_bbox(*b))
        .reduce(|a, b| a.union(b))
    else {
        return;
    };
    if let Some(cull) = plane.cull {
        area = area.intersect(cull);
    }
    if !(area.width() > 0.0 && area.height() > 0.0) {
        return;
    }
    let Some(cast) = plane.projection.map_rect_bounds(area) else {
        return;
    };
    builder.push_alpha_group(cast, alpha);
    for (part, ctx) in parts.iter().zip(ctxs) {
        for object in part.resolved.iter() {
            cast_shadows(builder, object, part.follows, thrown, ctx);
        }
    }
    builder.pop_isolation();
}

/// **What a layer's shadow pass depends on**, hashed - or `None` if something
/// cannot be identified (a freshly tweened object that is not a rig, a camera
/// with tilt) and the pass must be encoded and thrown away.
///
/// The pass reads the layer's resolved objects and nothing a modifier does to
/// them (it draws the stored artwork, as it always has), so the objects part is
/// [`layer_fx_signature`] with no modifier evaluations and no rim: each object's
/// stored `Arc` (or a rig's pose fingerprint), the library, whether anything
/// animated is inside (then the playhead), the layer's filters. On top of it,
/// everything else the encoding bakes in: the layer and its depth, the throw
/// (`shadow`, which carries the light's direction, length and adjustment - its
/// `Debug` form shows every field to the bit), the light's shadow strength and
/// the group's alpha, the projection's own affine (a camera move changes it),
/// the cull, the builder's view (anchor, scale, output transform, device scale,
/// clip, visible rectangle - all baked into render-space coordinates) and the
/// lighting detail. The returned `Weak`s pin every address hashed (see
/// [`FxEntry::_identity`]).
#[allow(clippy::too_many_arguments)]
fn shadow_memo_signature(
    scene: &Scene,
    layer: &buzz_scene::Layer,
    resolved: &buzz_scene::ResolvedFrame<'_>,
    shadow: &buzz_light::ShadowThrow,
    strength: f32,
    alpha: f32,
    ctx: &DrawCtx<'_>,
    builder: &SceneBuilder<'_>,
    detail: LightDetail,
    animated: &mut AnimatedMemo,
) -> Option<(u64, Vec<std::sync::Weak<Object>>)> {
    use std::hash::{Hash, Hasher};

    // A tilted camera has no affine form; the pass is drawn through it live.
    let projection = ctx.projection.as_affine()?;
    let evals: Vec<Option<buzz_scene::modifier::ModifierEval>> =
        resolved.iter().map(|_| None).collect();
    let (objects, identity) = layer_fx_signature(
        scene,
        layer,
        resolved,
        &evals,
        ctx.elapsed as f64,
        ctx.stage_frame,
        None,
        animated,
    )?;

    let mut h = std::collections::hash_map::DefaultHasher::new();
    objects.hash(&mut h);
    layer.depth.to_bits().hash(&mut h);
    hash_shadow_view(
        &mut h,
        shadow,
        strength,
        alpha,
        projection,
        ctx.cull,
        builder,
        detail,
    );
    Some((h.finish(), identity))
}

/// The half of a shadow pass's signature that is not the casters: the throw,
/// the tone, the view it is encoded through and the lighting detail. Shared by
/// [`shadow_memo_signature`] and [`rig_shadow_memo_signature`].
#[allow(clippy::too_many_arguments)]
fn hash_shadow_view(
    h: &mut std::collections::hash_map::DefaultHasher,
    shadow: &buzz_light::ShadowThrow,
    strength: f32,
    alpha: f32,
    projection: Affine,
    cull: Option<buzz_geom::Rect>,
    builder: &SceneBuilder<'_>,
    detail: LightDetail,
) {
    use std::hash::Hash;

    format!("{shadow:?}").hash(h);
    strength.to_bits().hash(h);
    alpha.to_bits().hash(h);
    for c in projection.as_coeffs() {
        c.to_bits().hash(h);
    }
    match cull {
        Some(cull) => {
            1u8.hash(h);
            for c in [cull.x0, cull.y0, cull.x1, cull.y1] {
                c.to_bits().hash(h);
            }
        }
        None => 0u8.hash(h),
    }
    let split = builder.split;
    for c in [split.anchor.x, split.anchor.y, split.scale, builder.device_scale] {
        c.to_bits().hash(h);
    }
    for c in split.gpu_view.as_coeffs() {
        c.to_bits().hash(h);
    }
    for r in [builder.clip.bounds(), builder.visible] {
        for c in [r.x0, r.y0, r.x1, r.y1] {
            c.to_bits().hash(h);
        }
    }
    detail.index().hash(h);
}

/// **What a layer rig's one shadow pass depends on** (LS), hashed: every
/// part's objects (as [`shadow_memo_signature`] hashes one layer's) with what
/// it inherits from the parts it follows, the root's depth, and the same view
/// and throw. `None` when anything in it cannot be identified.
#[allow(clippy::too_many_arguments)]
fn rig_shadow_memo_signature(
    scene: &Scene,
    parts: &[RigPart<'_>],
    ctxs: &[DrawCtx<'_>],
    depth: f64,
    shadow: &buzz_light::ShadowThrow,
    strength: f32,
    alpha: f32,
    builder: &SceneBuilder<'_>,
    detail: LightDetail,
    animated: &mut AnimatedMemo,
) -> Option<(u64, Vec<std::sync::Weak<Object>>)> {
    use std::hash::{Hash, Hasher};

    let plane = ctxs.first()?;
    let projection = plane.projection.as_affine()?;
    let mut h = std::collections::hash_map::DefaultHasher::new();
    let mut identity: Vec<std::sync::Weak<Object>> = Vec::new();
    parts.len().hash(&mut h);
    for (part, ctx) in parts.iter().zip(ctxs) {
        let evals: Vec<Option<buzz_scene::modifier::ModifierEval>> =
            part.resolved.iter().map(|_| None).collect();
        let (objects, held) = layer_fx_signature(
            scene,
            part.layer,
            &part.resolved,
            &evals,
            ctx.elapsed as f64,
            ctx.stage_frame,
            None,
            animated,
        )?;
        objects.hash(&mut h);
        for c in part.follows.as_coeffs() {
            c.to_bits().hash(&mut h);
        }
        identity.extend(held);
    }
    depth.to_bits().hash(&mut h);
    hash_shadow_view(
        &mut h,
        shadow,
        strength,
        alpha,
        projection,
        plane.cull,
        builder,
        detail,
    );
    Some((h.finish(), identity))
}

/// **Everything one caster blocks the light with**, gathered before any of it
/// is drawn.
///
/// # Why the shadow is one path and not one path per shape
///
/// A shadow used to be a black fill per shape of the caster. Every one of those
/// is antialiased on its own, so wherever two regions of the artwork share a
/// boundary — which on imported artwork is *every* boundary, Flash having
/// scan-converted a shape as a soup of edges in one pass — each covered about
/// half of every pixel along it, and half composited over half is three
/// quarters, not one. The missing quarter was the ground, and it traced every
/// internal border in the figure across its shadow as a pale line. That is the
/// "the shadows have inner outlines" report, and it is the conflation artifact:
/// a property of compositing paths separately rather than a bug in any of them.
///
/// Sealing each fill the way the artwork is sealed would work and would cost a
/// stroke per shape — it measurably doubled what a cast shadow encodes, which
/// on a dense document was enough to spend the whole lighting budget and trim
/// the shadows away altogether.
///
/// So the fills are concatenated and filled **once**. There are then no
/// internal boundaries to conflate, because the rasteriser scan-converts the
/// whole silhouette in one pass — which is what Flash did, and why its artwork
/// has no seams in it. It is also *cheaper* than what it replaces: one path per
/// caster instead of one per shape.
#[derive(Default)]
struct Caster {
    /// Every filled outline, thrown and projected, as one path.
    fills: buzz_geom::BezPath,
    /// The outlines, which cannot be folded into a fill. Each with the pen it
    /// is to be drawn with, already carried through the throw.
    strokes: Vec<(buzz_geom::BezPath, f64)>,
    /// Hairlines, which are a pixel wide at every zoom and so carry no width.
    hairlines: Vec<buzz_geom::BezPath>,
}

impl Caster {
    /// Add one shape's contribution.
    fn add(
        &mut self,
        builder: &SceneBuilder<'_>,
        shape: &buzz_scene::ShapeData,
        doc: Affine,
        shadow: Affine,
        ctx: &DrawCtx<'_>,
    ) {
        // Breathed where the artwork is and *then* thrown: a breathing
        // character's shadow breathes with it, which is what makes the shadow
        // read as its own.
        let cast = shadow * ctx.breathe(doc * shape.path.clone());
        let drawn = ctx.project(&cast, builder.tolerance());

        if shape.fill.is_some() {
            // **Wound forward**, because concatenating only unions under the
            // non-zero rule while every path turns the same way — two wound
            // opposite ways cancel where they overlap and leave a hole at the
            // join, which is the same defect this exists to remove. A path's
            // own holes survive; see [`buzz_geom::turns_backwards`].
            for element in buzz_geom::wound_forward(&drawn).elements() {
                self.fills.push(*element);
            }
        }

        // **And the outline, because a silhouette has no seams in it.**
        //
        // A shadow used to be the fills alone. In artwork drawn the way artwork
        // is actually drawn — a fill for each region with a line between them —
        // every one of those lines was a gap the ground showed through. It also
        // gives an unfilled outline — a rope, a wire, a stray line — the shadow
        // it should always have had, which filling its path could never do.
        let Some(stroke) = &shape.stroke else {
            return;
        };
        if stroke.hairline {
            // A hairline is a pixel wide however far away it is, and its shadow
            // is the same hairline: a seam, not a rope.
            self.hairlines.push(drawn);
            return;
        }
        // **The pen is thrown too.**
        //
        // The path is stroked after it has been projected, the way the
        // artwork's own strokes are — but a shadow's path has been through the
        // throw as well, and the throw scales. A ground shadow under a low
        // light stretches away from its caster, so the gap between two thrown
        // fills is wider than the gap between the fills themselves and a pen of
        // the drawn width no longer covers it: measured, the seam went from
        // bare ground to half-dark rather than closing.
        //
        // The *largest* scale the throw applies, not the average: a throw
        // squashes one axis and stretches the other, one pen cannot follow
        // both, and a seam sealed a little wide disappears inside the
        // silhouette while a seam sealed a little narrow is the light thread
        // this exists to remove.
        let [a, b, c, d, _, _] = shadow.as_coeffs();
        let spread = a.hypot(b).max(c.hypot(d)).max(1.0);
        self.strokes.push((drawn, stroke.width * spread));
    }

    /// Draw what was gathered.
    ///
    /// **Opaque.** The tone is on the group this is drawn inside, so that
    /// everything overlapping within one caster makes a silhouette rather than
    /// a darker patch. See the shadow pass in `draw_layer`.
    fn draw(self, builder: &mut SceneBuilder<'_>) {
        if !self.fills.elements().is_empty() {
            builder.fill_shape(&self.fills, Color::BLACK);
        }
        for (path, width) in &self.strokes {
            builder.stroke_transformed(path, Color::BLACK, *width, Affine::IDENTITY);
        }
        for path in &self.hairlines {
            builder.stroke_hairline(path, Color::BLACK, 1.0);
        }
    }
}

/// [`cast_shadows`], carrying how deep into nested symbols it has gone.
///
/// `depth_limit` is the *nesting* count and is nothing to do with layer depth,
/// which is the layer's distance from the camera. A symbol containing an
/// instance of itself would otherwise recurse until the stack ran out.
fn cast_shadows_within(
    builder: &SceneBuilder<'_>,
    caster: &mut Caster,
    object: &Object,
    doc: Affine,
    shadow: Affine,
    ctx: &DrawCtx<'_>,
    depth_limit: usize,
) {
    if !object.visible {
        return;
    }
    let doc = doc * object.transform;

    match &object.kind {
        ObjectKind::Shape(shape) => {
            if shape.fill.is_none() && shape.stroke.is_none() {
                // Nothing drawn is nothing to block the light with.
                return;
            }
            caster.add(builder, shape, doc, shadow, ctx);
        }
        ObjectKind::Group(children) => {
            for child in children {
                cast_shadows_within(builder, caster, child, doc, shadow, ctx, depth_limit);
            }
        }
        ObjectKind::Armature(rig) => {
            for part in rig.posed() {
                cast_shadows_within(builder, caster, &part, doc, shadow, ctx, depth_limit);
            }
        }
        ObjectKind::Warp(warp) => {
            let warped = warp.warped();
            if warped.fill.is_none() && warped.stroke.is_none() {
                return;
            }
            caster.add(builder, &warped, doc, shadow, ctx);
        }
        // **A symbol's shadow is the shadow of what it contains.**
        //
        // This used to do nothing, and the note here said so. That is a much
        // bigger hole than it reads as: a document imported from Animate is
        // *entirely* symbol instances, so "an instance casts nothing" means a
        // real film casts no shadows at all. Switching shadows on did visibly
        // nothing, which looks like the feature being broken rather than
        // unfinished.
        //
        // The walk is `draw_object`'s, reduced to what a shadow needs — no
        // masks, no colour effects, no filters, because a shadow is a
        // silhouette and none of those change its shape. What it does need is
        // the instance's own frame, so a symbol on its fourth frame casts the
        // shadow of the drawing on that frame rather than of its first.
        ObjectKind::Instance(instance) => {
            if depth_limit >= MAX_SYMBOL_DEPTH {
                return;
            }
            let Some(symbol) = ctx.scene.library().get(instance.symbol) else {
                return;
            };
            // Locked to the root, or nested inside something that is: the
            // shadow is cast from the stage's own frame, so it agrees with the
            // artwork drawn the same way.
            let synced = ctx.synced || instance.sync_to_root;
            let inner = if synced {
                instance.resolve_to_root(ctx.stage_frame, symbol.length())
            } else {
                instance.resolve_frame(symbol.kind, ctx.elapsed, symbol.length())
            };
            let synced_ctx;
            let ctx = if synced && !ctx.synced {
                synced_ctx = DrawCtx {
                    synced: true,
                    ..ctx.clone()
                };
                &synced_ctx
            } else {
                ctx
            };

            for layer in symbol.layers.drawable_at(inner) {
                // A mask layer is not artwork and casts nothing; the layers it
                // clips still cast their own.
                if layer.kind.is_mask() {
                    continue;
                }
                let follows = symbol.layers.inherited_transform(layer.id, inner);
                for child in symbol.layers.resolved_at_rigged(layer.id, inner).iter() {
                    cast_shadows_within(
                        builder,
                        caster,
                        child,
                        doc * follows,
                        shadow,
                        ctx,
                        depth_limit + 1,
                    );
                }
            }
        }
    }
}

/// Guide layers draw faintly, so they read as reference rather than artwork.
fn fade(color: Color) -> Color {
    color.multiply_alpha(FADE)
}

/// How far a guide layer's artwork is faded back. Named because the shadow pass
/// has to apply the same amount to a whole group rather than to one colour.
const FADE: f32 = 0.35;

#[cfg(test)]
mod tests {
    use super::*;
    use buzz_geom::{Camera, Point, Rect, Shape, Size};
    use buzz_scene::{LightKind, ShapeData};

    fn lit_scene() -> Scene {
        let mut scene = Scene::default();
        let layer = scene.add_layer("Art", buzz_scene::LayerKind::Normal);
        scene.add_shape(
            layer,
            ShapeData::filled(
                Rect::new(10.0, 10.0, 100.0, 100.0).to_path(1e-9),
                Color::WHITE,
            ),
        );
        // A sun low in the sky, so it actually casts a terminator and a shadow.
        scene.add_light(LightKind::Sun {
            azimuth: 0.4,
            elevation: 0.3,
        });
        assert!(scene.lights().is_active(), "the rig should be on");
        scene
    }

    /// A scene with one shape on-screen and one far off it, for culling.
    fn near_and_far_scene() -> Scene {
        let mut scene = Scene::default();
        let layer = scene.add_layer("Art", buzz_scene::LayerKind::Normal);
        // Near the origin, in view.
        scene.add_shape(
            layer,
            ShapeData::filled(
                Rect::new(10.0, 10.0, 40.0, 40.0).to_path(1e-9),
                Color::WHITE,
            ),
        );
        // Far away, well outside any reasonable viewport.
        scene.add_shape(
            layer,
            ShapeData::filled(
                Rect::new(50_000.0, 50_000.0, 50_040.0, 50_040.0).to_path(1e-9),
                Color::WHITE,
            ),
        );
        scene
    }

    fn encoded_paths(scene: &Scene, cull: Option<Rect>) -> u32 {
        let mut vello = crate::vello::Scene::new();
        let camera = Camera::new(Point::new(25.0, 25.0), 1.0, Size::new(400.0, 400.0));
        let mut builder = SceneBuilder::new(&mut vello, &camera);
        let mut cache = DrawCache::new();
        cache.begin();
        let options = FrameOptions {
            cull,
            ..FrameOptions::default()
        };
        draw_frame_within(
            &mut builder,
            scene,
            0,
            Affine::IDENTITY,
            &options,
            &mut cache,
        );
        vello.encoding().n_paths
    }

    /// **Culling changes nothing that is visible.** The stage culls to the same
    /// rectangle the render clip already bounds to, so a shape it skips is one
    /// the clip would have collapsed to nothing anyway — the parity guarantee
    /// for plan 2.4. What culling saves is the *work* of transforming, clipping
    /// and brushing that shape, which the perf gate (`never_hang.rs`) measures;
    /// here we pin that the encoded result is identical with and without it.
    #[test]
    fn culling_to_the_viewport_matches_no_cull() {
        let scene = near_and_far_scene();
        let uncalled = encoded_paths(&scene, None);
        // The clip in `encoded_paths` bounds a 400×400 view about (25,25); a cull
        // covering it (the near shape in, the far shape out) is what the stage
        // passes, and the far shape encodes nothing either way.
        let culled = encoded_paths(&scene, Some(Rect::new(-500.0, -500.0, 500.0, 500.0)));
        assert_eq!(
            culled, uncalled,
            "culling to the viewport must produce the identical encoding"
        );
        assert!(uncalled > 0, "the on-screen shape must be drawn");
    }

    #[test]
    fn rects_overlap_is_inclusive_at_the_edge() {
        let a = Rect::new(0.0, 0.0, 10.0, 10.0);
        assert!(
            rects_overlap(a, Rect::new(5.0, 5.0, 15.0, 15.0)),
            "overlapping"
        );
        assert!(
            rects_overlap(a, Rect::new(10.0, 0.0, 20.0, 10.0)),
            "edge-touching kept"
        );
        assert!(
            !rects_overlap(a, Rect::new(11.0, 0.0, 20.0, 10.0)),
            "clear of it"
        );
    }

    /// The plan's acceptance test for Wave 4.5: a **cold** cache draws the
    /// frame without building any geometry on the calling thread. The shapes
    /// come out unlit and the geometry is queued for an off-thread build.
    #[test]
    fn a_cold_cache_defers_rather_than_building() {
        let scene = lit_scene();
        let mut vello = crate::vello::Scene::new();
        let camera = Camera::new(Point::new(55.0, 55.0), 1.0, Size::new(400.0, 400.0));
        let mut builder = SceneBuilder::new(&mut vello, &camera);

        let mut cache = DrawCache::new();
        cache.begin();
        cache.lights.set_defer(true);

        let options = FrameOptions {
            lit: true,
            ..FrameOptions::default()
        };
        draw_frame_within(
            &mut builder,
            &scene,
            0,
            Affine::IDENTITY,
            &options,
            &mut cache,
        );

        assert!(
            cache.lights.is_empty(),
            "a deferred draw must not build geometry into the cache on this thread"
        );
        let misses = cache.lights.take_misses();
        assert!(
            !misses.is_empty(),
            "the geometry it could not draw should have been queued for later"
        );

        // And building those misses and installing them warms the cache, so the
        // next draw of the same frame is lit rather than deferred again.
        let built: Vec<_> = misses
            .into_iter()
            .map(crate::lighting::Miss::build)
            .collect();
        cache.lights.install(built);
        assert!(!cache.lights.is_empty(), "the built geometry is now cached");
    }

    /// The warm path: with defer off, the same draw builds inline, as it always
    /// did — so an ordinary edit lights on the spot with no unlit frame.
    #[test]
    fn a_warm_draw_builds_inline() {
        let scene = lit_scene();
        let mut vello = crate::vello::Scene::new();
        let camera = Camera::new(Point::new(55.0, 55.0), 1.0, Size::new(400.0, 400.0));
        let mut builder = SceneBuilder::new(&mut vello, &camera);

        let mut cache = DrawCache::new();
        cache.begin();
        // defer left off

        let options = FrameOptions {
            lit: true,
            ..FrameOptions::default()
        };
        draw_frame_within(
            &mut builder,
            &scene,
            0,
            Affine::IDENTITY,
            &options,
            &mut cache,
        );

        assert!(
            cache.lights.take_misses().is_empty(),
            "an inline draw queues nothing"
        );
        assert!(
            !cache.lights.is_empty(),
            "it should have built the geometry into the cache"
        );
    }

    /// `RENDER_FOLLOWUPS_PLAN.md` item 1: a rigged child now draws through
    /// [`buzz_scene::LayerStack::resolved_at_rigged`] — the same call
    /// [`draw_layer`] makes above — rather than through the plain
    /// `resolved_at` that ignored a non-similarity rig parent.
    ///
    /// Two layers: a parent whose rest pose is a **non-uniform** scale (2, 1)
    /// — not a similarity, so the bug this fixes would show up — and a child
    /// that follows it, its stored keyframes the usual `P_rest · C_rel(f)`
    /// product (`bake_rig_offsets`'s convention): identity at frame 0, a
    /// plain 90° turn at frame 1. At `t = 0.5` the drawn transform must be
    /// `F · Rotate(45°)` — the rig-relative rotation interpolated in the
    /// parent's own space and placed back into it — computed here by plain
    /// trigonometry, not by calling `interpolate_objects` a second time.
    #[test]
    fn a_rigged_child_tweens_in_its_parents_rest_frame() {
        use std::f64::consts::FRAC_PI_2;

        let f = Affine::scale_non_uniform(2.0, 1.0);

        let mut parent = buzz_scene::Layer::normal(buzz_scene::LayerId(1), "Parent");
        parent.rest_pose = Some(f);

        let mut child = buzz_scene::Layer::normal(buzz_scene::LayerId(2), "Child");
        child.follows = Some(parent.id);

        let shape = ShapeData::filled(Rect::new(0.0, 0.0, 1.0, 1.0).to_path(1e-9), Color::WHITE);
        let id = buzz_scene::ObjectId(1);
        let mut start = Object::shape(id, shape.clone());
        start.transform = f * Affine::IDENTITY;
        let mut end = Object::shape(id, shape);
        end.transform = f * Affine::rotate(FRAC_PI_2);

        child.frames.push_object(0, Arc::new(start));
        child.frames.insert_keyframe(1);
        child.frames.push_object(1, Arc::new(end));
        child.frames.set_tween(0, buzz_scene::Tween::classic());

        let mut layers = buzz_scene::LayerStack::new();
        layers.push_front(child.clone());
        layers.push_front(parent);

        let resolved = layers.resolved_at_rigged(child.id, 0.5_f64);
        let got: Vec<&Object> = resolved.iter().collect();
        assert_eq!(got.len(), 1);
        let got = got[0].transform;

        // Hand-computed: the rig-relative rotation halfway between 0° and 90°
        // is exactly 45° (no scale change either side), placed back through
        // the parent's rest pose.
        let expected = f * Affine::rotate(FRAC_PI_2 * 0.5);
        let (gc, ec) = (got.as_coeffs(), expected.as_coeffs());
        for i in 0..6 {
            assert!(
                (gc[i] - ec[i]).abs() < 1e-6,
                "coefficient {i}: {} vs {}",
                gc[i],
                ec[i]
            );
        }

        // The plain (non-rigged) path this replaces disagrees: interpolating
        // the stored product directly decomposes to a *uniform* 1.5x scale
        // turning through 45°, not the parent's own non-uniform scale — the
        // exact drift this phase fixes.
        let naive = child.frames.resolved_at(0.5_f64);
        let naive: Vec<&Object> = naive.iter().collect();
        let naive = naive[0].transform;
        assert!(
            (naive.as_coeffs()[0] - gc[0]).abs() > 1e-3,
            "the naive and rig-relative paths should disagree here"
        );
    }
}

#[cfg(test)]
mod symbol_table {
    use super::*;
    use buzz_geom::{Rect, Shape};
    use buzz_scene::{
        Layer, LayerId, Object, ObjectId, PaintBlend, ShapeData, Spatial, SymbolId, SymbolKind,
    };

    fn first_layer(scene: &Scene, symbol: SymbolId) -> LayerId {
        scene
            .library()
            .get(symbol)
            .unwrap()
            .layers
            .iter()
            .next()
            .unwrap()
            .id
    }

    /// A symbol whose one layer holds `object`.
    fn symbol_with(scene: &mut Scene, name: &str, object: Object) -> SymbolId {
        let id = scene.add_symbol(name, SymbolKind::Graphic, None);
        let layer = first_layer(scene, id);
        scene.library_mut().update(id, |s| {
            s.layers.update(layer, |l| {
                l.frames.push_object(0, Arc::new(object));
            });
        });
        id
    }

    fn shape(id: u64, rect: Rect) -> Object {
        Object::shape(
            ObjectId(id),
            ShapeData::filled(rect.to_path(1e-9), Color::WHITE),
        )
    }

    fn unit(id: u64) -> Object {
        shape(id, Rect::new(0.0, 0.0, 10.0, 10.0))
    }

    fn additive(id: u64) -> Object {
        Object::shape(
            ObjectId(id),
            ShapeData::filled(Rect::new(0.0, 0.0, 10.0, 10.0).to_path(1e-9), Color::WHITE)
                .with_blend(PaintBlend::Additive),
        )
    }

    /// The old algorithm: walk the whole library live. The memo must agree.
    fn brute(object: &Object, scene: &Scene, depth: usize) -> bool {
        if depth >= MAX_SYMBOL_DEPTH {
            return false;
        }
        match &object.kind {
            ObjectKind::Shape(s) => s.blend.is_additive(),
            ObjectKind::Group(children) => children.iter().any(|c| brute(c, scene, depth + 1)),
            ObjectKind::Warp(w) => w.shape.blend.is_additive(),
            ObjectKind::Armature(rig) => rig
                .parts
                .iter()
                .any(|p| brute(&p.artwork, scene, depth + 1)),
            ObjectKind::Instance(i) => scene.library().get(i.symbol).is_some_and(|sym| {
                sym.layers
                    .iter()
                    .any(|l| l.all_objects().any(|c| brute(c, scene, depth + 1)))
            }),
        }
    }

    #[test]
    fn additive_memo_matches_brute_force() {
        let mut scene = Scene::default();
        let plain = symbol_with(&mut scene, "plain", unit(1));
        let glow = symbol_with(&mut scene, "glow", additive(2));
        let character = scene.add_symbol("character", SymbolKind::Graphic, None);
        let clayer = first_layer(&scene, character);
        scene.library_mut().update(character, |s| {
            s.layers.update(clayer, |l| {
                l.frames
                    .push_object(0, Arc::new(Object::instance_of(ObjectId(10), plain)));
                l.frames
                    .push_object(0, Arc::new(Object::instance_of(ObjectId(11), glow)));
            });
        });

        let mut table = SymbolTable::default();
        table.refresh(&scene);

        let check = |table: &SymbolTable, scene: &Scene, id: SymbolId, oid: u64| {
            let memo = table.get(id).unwrap().flags.additive;
            let brute_v = brute(&Object::instance_of(ObjectId(oid), id), scene, 0);
            assert_eq!(memo, brute_v, "memo vs brute for {id:?}");
        };
        check(&table, &scene, plain, 900);
        check(&table, &scene, glow, 901);
        check(&table, &scene, character, 902);
        assert!(!table.get(plain).unwrap().flags.additive);
        assert!(table.get(glow).unwrap().flags.additive);
        assert!(
            table.get(character).unwrap().flags.additive,
            "the character inherits the glow's build-up paint"
        );

        // Add build-up paint deep inside the previously-plain part.
        let player = first_layer(&scene, plain);
        scene.library_mut().update(plain, |s| {
            s.layers.update(player, |l| {
                l.frames.push_object(0, Arc::new(additive(20)));
            });
        });
        table.refresh(&scene);
        check(&table, &scene, plain, 903);
        check(&table, &scene, character, 904);
        assert!(table.get(plain).unwrap().flags.additive, "now additive");
    }

    #[test]
    fn editing_a_nested_symbol_changes_the_parents_fingerprint_only() {
        let mut scene = Scene::default();
        let part = symbol_with(&mut scene, "part", unit(1));
        let character = symbol_with(
            &mut scene,
            "character",
            Object::instance_of(ObjectId(2), part),
        );
        let unrelated = symbol_with(&mut scene, "unrelated", unit(3));

        let mut table = SymbolTable::default();
        table.refresh(&scene);
        let fp_part = table.get(part).unwrap().fingerprint;
        let fp_char = table.get(character).unwrap().fingerprint;
        let fp_unrelated = table.get(unrelated).unwrap().fingerprint;

        // Edit the nested part — a new shape on its layer.
        let part_layer = first_layer(&scene, part);
        scene.library_mut().update(part, |s| {
            s.layers.update(part_layer, |l| {
                l.frames.push_object(0, Arc::new(unit(9)));
            });
        });
        table.refresh(&scene);

        assert_ne!(
            fp_part,
            table.get(part).unwrap().fingerprint,
            "part changed"
        );
        assert_ne!(
            fp_char,
            table.get(character).unwrap().fingerprint,
            "a nested edit must ripple up to the parent that instances it"
        );
        assert_eq!(
            fp_unrelated,
            table.get(unrelated).unwrap().fingerprint,
            "an untouched symbol keeps its fingerprint"
        );
    }

    #[test]
    fn safety_flags_are_transitive() {
        // A filter two levels deep.
        {
            let mut scene = Scene::default();
            let mut filtered = unit(1);
            filtered.filters = vec![buzz_fx::Filter::new(buzz_fx::FilterKind::blur())];
            let inner = symbol_with(&mut scene, "inner", filtered);
            let mid = symbol_with(&mut scene, "mid", Object::instance_of(ObjectId(2), inner));
            let outer = symbol_with(&mut scene, "outer", Object::instance_of(ObjectId(3), mid));
            let mut table = SymbolTable::default();
            table.refresh(&scene);
            assert!(
                table.get(outer).unwrap().flags.filters,
                "filter bubbles up two levels"
            );
            assert!(!table.get(outer).unwrap().cacheable_content());
        }
        // A group blend.
        {
            let mut scene = Scene::default();
            let mut blended = unit(1);
            blended.blend = buzz_fx::Blend::Add;
            let inner = symbol_with(&mut scene, "inner", blended);
            let outer = symbol_with(&mut scene, "outer", Object::instance_of(ObjectId(2), inner));
            let mut table = SymbolTable::default();
            table.refresh(&scene);
            assert!(table.get(outer).unwrap().flags.group_blend);
            assert!(!table.get(outer).unwrap().cacheable_content());
        }
        // An out-of-plane object.
        {
            let mut scene = Scene::default();
            let mut tilted = unit(1);
            tilted.spatial = Spatial {
                rotation_y: 0.5,
                ..Spatial::default()
            };
            let inner = symbol_with(&mut scene, "inner", tilted);
            let outer = symbol_with(&mut scene, "outer", Object::instance_of(ObjectId(2), inner));
            let mut table = SymbolTable::default();
            table.refresh(&scene);
            assert!(table.get(outer).unwrap().flags.non_flat);
            assert!(!table.get(outer).unwrap().cacheable_content());
        }
        // An inverse-mask layer.
        {
            let mut scene = Scene::default();
            let holed = scene.add_symbol("holed", SymbolKind::Graphic, None);
            scene.library_mut().update(holed, |s| {
                s.layers
                    .push_front(Layer::new(LayerId(999), "Hole", LayerKind::InverseMask));
            });
            let outer = symbol_with(&mut scene, "outer", Object::instance_of(ObjectId(2), holed));
            let mut table = SymbolTable::default();
            table.refresh(&scene);
            assert!(table.get(holed).unwrap().flags.inverse_mask);
            assert!(table.get(outer).unwrap().flags.inverse_mask, "bubbles up");
        }
        // Additive paint.
        {
            let mut scene = Scene::default();
            let glow = Object::shape(
                ObjectId(1),
                ShapeData::filled(Rect::new(0.0, 0.0, 10.0, 10.0).to_path(1e-9), Color::WHITE)
                    .with_blend(PaintBlend::Additive),
            );
            let inner = symbol_with(&mut scene, "inner", glow);
            let outer = symbol_with(&mut scene, "outer", Object::instance_of(ObjectId(2), inner));
            let mut table = SymbolTable::default();
            table.refresh(&scene);
            assert!(table.get(outer).unwrap().flags.additive);
            // Additive does not, by itself, make a symbol uncacheable.
            assert!(table.get(outer).unwrap().cacheable_content());
        }
        // A plain symbol is fully cacheable.
        {
            let mut scene = Scene::default();
            let plain = symbol_with(&mut scene, "plain", unit(1));
            let mut table = SymbolTable::default();
            table.refresh(&scene);
            let info = table.get(plain).unwrap();
            assert_eq!(info.flags, SymFlags::default());
            assert!(info.cacheable_content());
        }
    }

    #[test]
    fn resolved_bounds_cover_nested_instances_beyond_naive_bounds() {
        let mut scene = Scene::default();
        let part = symbol_with(&mut scene, "part", unit(1));
        // The character places the part a thousand units away — a naive measure
        // through the placeholder instance box would miss it entirely.
        let character = symbol_with(
            &mut scene,
            "character",
            Object::instance_of(ObjectId(2), part)
                .with_transform(Affine::translate((1000.0, 1000.0))),
        );

        let mut table = SymbolTable::default();
        table.refresh(&scene);
        let bounds = table.get(character).unwrap().resolved_bounds;
        assert!(
            bounds.x0 >= 500.0 && bounds.y0 >= 500.0,
            "resolved bounds must follow the instance to where it was placed, got {bounds:?}"
        );
        // The real 10-unit shape, not the ~2-unit placeholder box at the origin.
        assert!(
            (bounds.width() - 10.0).abs() < 1.0 && (bounds.height() - 10.0).abs() < 1.0,
            "and be the part's real size, got {bounds:?}"
        );
    }

    #[test]
    fn a_self_referencing_symbol_terminates() {
        let mut scene = Scene::default();
        let recur = scene.add_symbol("recur", SymbolKind::Graphic, None);
        let layer = first_layer(&scene, recur);
        // The symbol contains an instance of itself — a cycle.
        scene.library_mut().update(recur, |s| {
            s.layers.update(layer, |l| {
                l.frames
                    .push_object(0, Arc::new(Object::instance_of(ObjectId(1), recur)));
            });
        });

        let mut table = SymbolTable::default();
        table.refresh(&scene); // must not recurse forever
        // A cycle is treated conservatively: out of the cache.
        assert!(!table.get(recur).unwrap().cacheable_content());
    }
}

#[cfg(test)]
mod symbol_scene {
    use super::*;
    use buzz_geom::{Camera, Point, Rect, Shape, Size};
    use buzz_scene::{Object, ObjectId, ShapeData, SymbolId, SymbolKind};

    fn first_layer(scene: &Scene, symbol: SymbolId) -> buzz_scene::LayerId {
        scene
            .library()
            .get(symbol)
            .unwrap()
            .layers
            .iter()
            .next()
            .unwrap()
            .id
    }

    fn shape_object(id: u64, rect: Rect) -> Object {
        Object::shape(
            ObjectId(id),
            ShapeData::filled(rect.to_path(1e-9), Color::WHITE),
        )
    }

    /// A `part` symbol (one shape), a `character` that places the part `parts`
    /// times, and `chars` characters on a stage layer. Returns (scene, part,
    /// character).
    fn crowd(parts: usize, chars: usize) -> (Scene, SymbolId, SymbolId) {
        let mut scene = Scene::default();

        let part = scene.add_symbol("part", SymbolKind::Graphic, None);
        let pl = first_layer(&scene, part);
        scene.library_mut().update(part, |s| {
            s.layers.update(pl, |l| {
                l.frames.push_object(
                    0,
                    Arc::new(shape_object(1, Rect::new(0.0, 0.0, 10.0, 10.0))),
                );
            });
        });

        let character = scene.add_symbol("character", SymbolKind::Graphic, None);
        let cl = first_layer(&scene, character);
        scene.library_mut().update(character, |s| {
            for i in 0..parts {
                s.layers.update(cl, |l| {
                    l.frames.push_object(
                        0,
                        Arc::new(
                            Object::instance_of(ObjectId(100 + i as u64), part)
                                .with_transform(Affine::translate((0.0, i as f64 * 12.0))),
                        ),
                    );
                });
            }
        });

        let layer = scene.add_layer("Cast", LayerKind::Normal);
        for i in 0..chars {
            let x = (i % 10) as f64 * 30.0;
            let y = (i / 10) as f64 * 40.0;
            scene.add_instance_at(layer, 0, character, Affine::translate((x, y)));
        }
        (scene, part, character)
    }

    fn wide_camera() -> Camera {
        Camera::new(Point::new(150.0, 100.0), 0.5, Size::new(2000.0, 2000.0))
    }

    /// Draw once and report (builds, stamps, n_paths).
    fn render(
        scene: &Scene,
        camera: &Camera,
        options: &FrameOptions,
        reuse: bool,
    ) -> (u64, u64, u32) {
        let mut vello = crate::vello::Scene::new();
        let mut builder = SceneBuilder::new(&mut vello, camera);
        let mut cache = DrawCache::new();
        cache.set_symbol_reuse(reuse);
        cache.begin();
        draw_frame_within(
            &mut builder,
            scene,
            0,
            Affine::IDENTITY,
            options,
            &mut cache,
        );
        cache.end();
        (
            cache.symbol_scenes.builds,
            cache.symbol_scenes.stamps,
            vello.encoding().n_paths,
        )
    }

    #[test]
    fn stamp_matches_the_live_split_formula() {
        // For a handful of similarity placements — orthogonal, or a uniform
        // scale (optionally mirrored) composed with one — the stamped child
        // must land a point exactly where the live render split would, once
        // the scale is quantised and folded into the child's own encode.
        let cases = [
            (Affine::translate((37.0, -12.0)), Point::new(5.0, 9.0), 0.4),
            (Affine::rotate(0.7), Point::new(-20.0, 3.0), 2.5),
            (
                Affine::translate((100.0, 200.0)) * Affine::rotate(-1.2),
                Point::new(50.0, -50.0),
                1.0,
            ),
            (
                Affine::FLIP_Y * Affine::rotate(0.3),
                Point::new(8.0, 8.0),
                7.0,
            ),
            // Scaled: `A`'s linear part is a rotation at 0.79×.
            (
                Affine::translate((10.0, -30.0)) * Affine::rotate(0.9) * Affine::scale(0.79),
                Point::new(4.0, -6.0),
                1.5,
            ),
            // Scaled and mirrored: a similarity with a negative determinant.
            (
                Affine::translate((-40.0, 15.0)) * Affine::scale_non_uniform(-0.79, 0.79),
                Point::new(-2.0, 3.0),
                0.8,
            ),
        ];
        let gpu_view = Affine::translate((400.0, 300.0)) * Affine::rotate(0.2);
        let cam = Point::new(123.0, -45.0);

        for (a, anchor, scale) in cases {
            // The same factoring `try_stamp_symbol` does: quantise the
            // similarity's scale to 1/32 octave and divide it back out of the
            // linear part, so `r` is near-orthogonal and `r · s_q = A_lin`
            // exactly.
            let det = a.determinant();
            assert!(det.abs() > 1e-12, "test placement must be invertible");
            let s = det.abs().sqrt();
            let s_q = 2f64.powf((s.log2() * 32.0).round() / 32.0);
            let [a0, a1, a2, a3, _, _] = a.as_coeffs();
            let r = [a0 / s_q, a1 / s_q, a2 / s_q, a3 / s_q];
            assert!(
                is_orthogonal([r[0], r[1], r[2], r[3], 0.0, 0.0], 2.5e-2),
                "test A must be a similarity within the tolerance `try_stamp_symbol` admits"
            );

            let stamp = symbol_stamp(gpu_view, a, r, anchor, scale, cam);
            // The child encodes C(p) = child_scale·(p − anchor), at the
            // instance's own scale folded into the stage's.
            let child_scale = scale * s_q;
            let child = Affine::scale(child_scale) * Affine::translate(-anchor.to_vec2());
            // The live render space of the document point A·p is S·(A·p − cam),
            // then the GPU view.
            let live_split = gpu_view * Affine::scale(scale) * Affine::translate(-cam.to_vec2());
            for p in [
                Point::new(0.0, 0.0),
                Point::new(3.0, -7.0),
                Point::new(-11.0, 4.0),
            ] {
                let stamped = stamp * (child * p);
                let live = live_split * (a * p);
                assert!(
                    (stamped - live).hypot() < 1e-7,
                    "stamp diverged from the live split: {stamped:?} vs {live:?}"
                );
            }
        }
    }

    #[test]
    fn n_instances_build_once() {
        let (scene, _part, _character) = crowd(10, 50);
        let (builds, stamps, _paths) =
            render(&scene, &wide_camera(), &FrameOptions::default(), true);
        assert_eq!(
            builds, 2,
            "one encode for the character, one for the part — not 60k"
        );
        // S2-rest (b): the first character is drawn live (first sighting), and
        // its 10 parts stamp 9 times (the first part is live, the second part
        // builds). The second character builds (second request) and stamps its
        // 10 parts from the cache; the other 48 characters are hits. 9 + 10 +
        // 49 = 68.
        assert_eq!(stamps, 68);
    }

    #[test]
    fn scaled_and_mirrored_instances_build_once() {
        // `crowd`'s `part` is a single 10×10 shape, cacheable on its own —
        // skip the `character` indirection `n_instances_build_once` exercises
        // and place `part` directly, some uniformly scaled and some mirrored
        // at the same magnitude. `|det|` is 0.79² either way, so every
        // instance quantises to the same `s_q` and keys to the same cached
        // child scene; only the stamp's linear part tells them apart.
        let (mut scene, part, _character) = crowd(0, 0);
        let layer = scene.add_layer("Scaled", LayerKind::Normal);
        let n = 8;
        for i in 0..n {
            let x = i as f64 * 40.0;
            let placement = if i % 2 == 0 {
                Affine::scale(0.79)
            } else {
                Affine::scale_non_uniform(-0.79, 0.79)
            };
            scene.add_instance_at(layer, 0, part, Affine::translate((x, 0.0)) * placement);
        }
        let (builds, stamps, _paths) =
            render(&scene, &wide_camera(), &FrameOptions::default(), true);
        assert_eq!(
            builds, 1,
            "one encode for every scaled or mirrored instance of `part`"
        );
        // S2-rest (b): the first instance is drawn live (first sighting); the
        // second builds (its own draw counts as a stamp) and the other n-2
        // are reused straight from the cache — n-1 stamps in all.
        assert_eq!(stamps, n as u64 - 1);
    }

    // -- S1: stamping a symbol placed under a colour effect ------------------
    // `docs/BIMARI5_LAG_PLAN.md`, S1. `try_stamp_symbol` used to refuse any
    // instance whose composed colour effect was not identity, which meant a
    // background placed with a tint, alpha or Advanced effect — and every
    // nested child under it — was re-walked and re-encoded from scratch every
    // frame (Diagnosis §3). These prove a hit under a non-identity effect is
    // pixel-identical to the live draw, and that two instances of one symbol
    // under two different effects each get their own cache entry rather than
    // colliding.

    /// A device+queue for reading a [`crate::vello::Scene`] back as RGBA8, or
    /// `None` on a machine with no usable GPU. Mirrors the harness
    /// `tests/hard_light.rs` uses, kept local so this test needs no second
    /// crate and no `pub` surface just for a test helper.
    struct GpuHarness {
        gpu: crate::GpuContext,
    }

    impl GpuHarness {
        fn new() -> Option<Self> {
            match crate::GpuContext::new_blocking(&crate::GpuPreference::Automatic) {
                Ok(gpu) => Some(Self { gpu }),
                Err(e) => {
                    eprintln!("skipping symbol-effect stamp pixel test: no usable GPU ({e})");
                    None
                }
            }
        }

        /// Render `scene` at exactly `w`×`h` device pixels and read the
        /// result back as RGBA8. A fresh target every call, sized to the
        /// caller's own zoom, since the four zooms this test compares at do
        /// not share one output size.
        fn render_rgba(&mut self, scene: &crate::vello::Scene, w: u32, h: u32) -> Vec<u8> {
            let texture = self.gpu.device.create_texture(&crate::wgpu::TextureDescriptor {
                label: Some("sym-effect-cache-target"),
                size: crate::wgpu::Extent3d {
                    width: w,
                    height: h,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: crate::wgpu::TextureDimension::D2,
                format: crate::RENDER_FORMAT,
                usage: crate::wgpu::TextureUsages::STORAGE_BINDING
                    | crate::wgpu::TextureUsages::TEXTURE_BINDING
                    | crate::wgpu::TextureUsages::COPY_SRC,
                view_formats: &[],
            });
            let view = texture.create_view(&crate::wgpu::TextureViewDescriptor::default());
            let readback = self.gpu.device.create_buffer(&crate::wgpu::BufferDescriptor {
                label: Some("sym-effect-cache-readback"),
                size: (w * h * 4) as u64,
                usage: crate::wgpu::BufferUsages::COPY_DST | crate::wgpu::BufferUsages::MAP_READ,
                mapped_at_creation: false,
            });

            self.gpu
                .render(scene, &view, w, h, Color::from_rgb8(0x20, 0x20, 0x20))
                .expect("vello render");

            let mut encoder = self
                .gpu
                .device
                .create_command_encoder(&crate::wgpu::CommandEncoderDescriptor { label: None });
            encoder.copy_texture_to_buffer(
                crate::wgpu::TexelCopyTextureInfo {
                    texture: &texture,
                    mip_level: 0,
                    origin: crate::wgpu::Origin3d::ZERO,
                    aspect: crate::wgpu::TextureAspect::All,
                },
                crate::wgpu::TexelCopyBufferInfo {
                    buffer: &readback,
                    layout: crate::wgpu::TexelCopyBufferLayout {
                        offset: 0,
                        bytes_per_row: Some(w * 4),
                        rows_per_image: Some(h),
                    },
                },
                crate::wgpu::Extent3d {
                    width: w,
                    height: h,
                    depth_or_array_layers: 1,
                },
            );
            self.gpu.queue.submit([encoder.finish()]);

            let slice = readback.slice(..);
            slice.map_async(crate::wgpu::MapMode::Read, |r| r.expect("map readback"));
            self.gpu
                .device
                .poll(crate::wgpu::PollType::Wait {
                    submission_index: None,
                    timeout: Some(std::time::Duration::from_secs(30)),
                })
                .expect("poll device");
            let pixels = {
                let view = slice.get_mapped_range();
                view.to_vec()
            };
            readback.unmap();
            pixels
        }
    }

    /// Every pixel must match within 2/255, and at most 0.1% of pixels may
    /// even need that much slack — the tolerance a hit-vs-live comparison
    /// gets for float rounding in how a stamp's folded transform composes
    /// versus a live draw's, not for any real difference in the picture.
    fn assert_pixels_match(live: &[u8], stamped: &[u8], w: u32, h: u32, label: &str) {
        assert_eq!(
            live.len(),
            stamped.len(),
            "{label}: readback size mismatch"
        );
        let pixel_count = (w as usize) * (h as usize);
        let mut off = 0usize;
        let mut max_diff = 0i32;
        for px in 0..pixel_count {
            let base = px * 4;
            let mut diff = 0i32;
            for c in 0..4 {
                diff = diff.max((live[base + c] as i32 - stamped[base + c] as i32).abs());
            }
            max_diff = max_diff.max(diff);
            if diff > 2 {
                off += 1;
            }
        }
        let allowed = ((pixel_count as f64) * 0.001).ceil() as usize;
        assert!(
            off <= allowed,
            "{label}: {off}/{pixel_count} pixels differ by more than 2/255 \
             (max diff {max_diff}/255), allowed {allowed}"
        );
    }

    /// Draw one frame into a fresh [`crate::vello::Scene`] through the same
    /// walk the window and the exporter share, using a caller-supplied
    /// `cache` so its state — build and stamp counts, cached entries —
    /// persists across calls the way a real session's does. [`render`] above
    /// opens a fresh `DrawCache` every call, which is right for counting
    /// builds within *one* frame but cannot show a **cross-frame** hit; this
    /// is the same walk with that one difference.
    fn render_with_cache(
        scene: &Scene,
        camera: &Camera,
        options: &FrameOptions,
        cache: &mut DrawCache,
    ) -> crate::vello::Scene {
        let mut vs = crate::vello::Scene::new();
        let mut builder = SceneBuilder::new(&mut vs, camera);
        cache.begin();
        draw_frame_within(&mut builder, scene, 0, Affine::IDENTITY, options, cache);
        cache.end();
        vs
    }

    /// A `part` symbol with one nested child instance (of `child`) and one
    /// stroked-and-filled shape, placed once under `effect` on a `Stage`
    /// layer. Returns `(scene, part)`.
    fn symbol_under_effect_scene(effect: ColorTransform) -> (Scene, SymbolId) {
        let mut scene = Scene::default();

        let child = scene.add_symbol("child", SymbolKind::Graphic, None);
        let ccl = first_layer(&scene, child);
        scene.library_mut().update(child, |s| {
            s.layers.update(ccl, |l| {
                l.frames
                    .push_object(0, Arc::new(shape_object(1, Rect::new(0.0, 0.0, 6.0, 6.0))));
            });
        });

        let part = scene.add_symbol("part", SymbolKind::Graphic, None);
        let pl = first_layer(&scene, part);
        scene.library_mut().update(part, |s| {
            s.layers.update(pl, |l| {
                // A nested child instance.
                l.frames.push_object(
                    0,
                    Arc::new(
                        Object::instance_of(ObjectId(10), child)
                            .with_transform(Affine::translate((2.0, 2.0))),
                    ),
                );
                // A stroked-and-filled shape.
                let mut stroked_and_filled = ShapeData::filled(
                    Rect::new(-8.0, -8.0, 8.0, 8.0).to_path(1e-9),
                    Color::from_rgb8(0xC8, 0x3C, 0x3C),
                );
                stroked_and_filled.stroke = Some(buzz_scene::StrokeSpec::new(
                    Color::from_rgb8(0x14, 0x14, 0xC8),
                    1.5,
                ));
                l.frames
                    .push_object(1, Arc::new(Object::shape(ObjectId(11), stroked_and_filled)));
            });
        });

        let layer = scene.add_layer("Stage", LayerKind::Normal);
        let mut instance = Object::instance_of(ObjectId(100), part);
        if let ObjectKind::Instance(inst) = &mut instance.kind {
            inst.color = effect;
        }
        scene
            .add_object_at(layer, 0, instance)
            .expect("`part` is in the library");

        (scene, part)
    }

    #[test]
    fn stamped_symbol_under_a_colour_effect_matches_the_live_pixels() {
        let Some(mut gpu) = GpuHarness::new() else {
            return;
        };

        // Multiply and add both non-trivial in every channel, alpha below 1
        // — the general shape a tint, alpha or Advanced colour effect in
        // Animate produces.
        let effect = ColorTransform {
            multiply: [0.55, 0.80, 1.05, 0.65],
            add: [0.12, -0.04, 0.20, 0.0],
        };
        let (scene, _part) = symbol_under_effect_scene(effect);

        // (zoom, viewport in device pixels). The viewport grows with zoom so
        // the ~16-unit symbol stays framed with margin at every one — a
        // sharper picture of the same content, not a narrower crop of it —
        // while `zoom` alone still lands each case in its own quantised
        // `child_scale` bucket (`SymKey::scale_bits`).
        let cases: [(f64, u32); 4] = [(1.0, 64), (4.0, 128), (16.0, 512), (40.0, 1280)];

        for (zoom, viewport_px) in cases {
            let camera = Camera::new(
                Point::new(0.0, 0.0),
                zoom,
                Size::new(viewport_px as f64, viewport_px as f64),
            );
            let options = FrameOptions::default();

            // Cache enabled, drawn three times with the same `DrawCache`: the
            // first draw is only a first sighting (drawn live, S2-rest (b));
            // the second misses and builds; the third, at the identical key,
            // is a genuine cross-frame stamp hit.
            let mut cache = DrawCache::new();
            let _sighting = render_with_cache(&scene, &camera, &options, &mut cache);
            assert_eq!(
                cache.symbol_scenes.builds, 0,
                "zoom {zoom}: a first sighting is drawn live, not built"
            );
            let _first = render_with_cache(&scene, &camera, &options, &mut cache);
            let builds_after_first = cache.symbol_scenes.builds;
            let stamps_after_first = cache.symbol_scenes.stamps;
            assert!(
                builds_after_first > 0,
                "zoom {zoom}: the second draw must build the child at least once"
            );

            let stamped_vs = render_with_cache(&scene, &camera, &options, &mut cache);
            assert_eq!(
                cache.symbol_scenes.builds, builds_after_first,
                "zoom {zoom}: the second draw must not rebuild"
            );
            assert!(
                cache.symbol_scenes.stamps > stamps_after_first,
                "zoom {zoom}: the second draw must be a stamp hit"
            );

            // Cache disabled: every instance — and every nested child under
            // it — draws live.
            let mut live_cache = DrawCache::new();
            live_cache.symbol_scenes.enabled = false;
            let live_vs = render_with_cache(&scene, &camera, &options, &mut live_cache);

            let stamped_px = gpu.render_rgba(&stamped_vs, viewport_px, viewport_px);
            let live_px = gpu.render_rgba(&live_vs, viewport_px, viewport_px);
            assert_pixels_match(
                &live_px,
                &stamped_px,
                viewport_px,
                viewport_px,
                &format!("zoom {zoom}x"),
            );
        }
    }

    #[test]
    fn two_instances_of_one_symbol_under_different_effects_each_keep_their_own_colour() {
        let mut scene = Scene::default();
        let swatch = scene.add_symbol("swatch", SymbolKind::Graphic, None);
        let sl = first_layer(&scene, swatch);
        scene.library_mut().update(swatch, |s| {
            s.layers.update(sl, |l| {
                l.frames.push_object(
                    0,
                    Arc::new(shape_object(1, Rect::new(-6.0, -6.0, 6.0, 6.0))),
                );
            });
        });

        let layer = scene.add_layer("Stage", LayerKind::Normal);
        // Alpha stays 1 so the tint alone, not blending with the background,
        // decides the sampled colour.
        let red_effect = ColorTransform {
            multiply: [1.0, 0.15, 0.15, 1.0],
            add: [0.0; 4],
        };
        let blue_effect = ColorTransform {
            multiply: [0.15, 0.15, 1.0, 1.0],
            add: [0.0; 4],
        };

        let mut red = Object::instance_of(ObjectId(1), swatch)
            .with_transform(Affine::translate((-20.0, 0.0)));
        if let ObjectKind::Instance(inst) = &mut red.kind {
            inst.color = red_effect;
        }
        scene.add_object_at(layer, 0, red).expect("swatch is in the library");

        let mut blue = Object::instance_of(ObjectId(2), swatch)
            .with_transform(Affine::translate((20.0, 0.0)));
        if let ObjectKind::Instance(inst) = &mut blue.kind {
            inst.color = blue_effect;
        }
        scene.add_object_at(layer, 0, blue).expect("swatch is in the library");

        let camera = Camera::new(Point::new(0.0, 0.0), 2.0, Size::new(256.0, 128.0));
        let options = FrameOptions::default();
        let mut cache = DrawCache::new();
        // S2-rest (b): each key's first sighting is drawn live, so a first
        // draw records the sightings and the second one builds.
        let _sighting = render_with_cache(&scene, &camera, &options, &mut cache);
        assert_eq!(cache.symbol_scenes.builds, 0, "first sightings are drawn live");
        let vs = render_with_cache(&scene, &camera, &options, &mut cache);

        assert_eq!(
            cache.symbol_scenes.builds, 2,
            "each differently-effected instance of the same symbol builds its own entry"
        );
        assert_eq!(cache.symbol_scenes.stamps, 2);
        assert_eq!(
            cache.symbol_scenes.entries.len(),
            2,
            "two distinct cache entries, one per effect, not one colliding on the other"
        );

        let Some(mut gpu) = GpuHarness::new() else {
            return;
        };
        let px = gpu.render_rgba(&vs, 256, 128);
        // `render_split`: render-space = (doc − anchor) · zoom + viewport
        // centre. anchor is the camera centre (0,0), zoom 2, viewport centre
        // (128, 64).
        let sample = |x: u32, y: u32| -> [u8; 4] {
            let i = ((y * 256 + x) * 4) as usize;
            [px[i], px[i + 1], px[i + 2], px[i + 3]]
        };
        let red_sample = sample(128 - 40, 64);
        let blue_sample = sample(128 + 40, 64);
        assert!(
            red_sample[0] as i32 > red_sample[2] as i32 + 50,
            "the red instance must stay red: {red_sample:?}"
        );
        assert!(
            blue_sample[2] as i32 > blue_sample[0] as i32 + 50,
            "the blue instance must stay blue, not pick up the red instance's cached colour: \
             {blue_sample:?}"
        );
    }

    // -- S2: stamping a symbol under any static linear placement --------------
    // `docs/BIMARI5_LAG_PLAN.md`, S2. `try_stamp_symbol` used to refuse any
    // placement that was not a similarity (Diagnosis §3's `road1 copy` and
    // `newTree copy 4`), which meant a stretched, mirrored or rotated-and-
    // stretched background — and every nested child under it — was re-walked
    // and re-encoded from scratch every frame. These prove a stamp under the
    // placement's own exact linear part is pixel-identical to the live draw,
    // for a nested child and a stroked shape both.

    /// A `part` symbol with one nested child instance (of `child`) and one
    /// stroked-and-filled shape, placed once at `transform` on a `Stage`
    /// layer — the same content `symbol_under_effect_scene` builds for S1,
    /// placed instead of tinted, for S2. Returns `(scene, part)`.
    fn symbol_placed_scene(transform: Affine) -> (Scene, SymbolId) {
        let mut scene = Scene::default();

        let child = scene.add_symbol("child", SymbolKind::Graphic, None);
        let ccl = first_layer(&scene, child);
        scene.library_mut().update(child, |s| {
            s.layers.update(ccl, |l| {
                l.frames
                    .push_object(0, Arc::new(shape_object(1, Rect::new(0.0, 0.0, 6.0, 6.0))));
            });
        });

        let part = scene.add_symbol("part", SymbolKind::Graphic, None);
        let pl = first_layer(&scene, part);
        scene.library_mut().update(part, |s| {
            s.layers.update(pl, |l| {
                // A nested child instance.
                l.frames.push_object(
                    0,
                    Arc::new(
                        Object::instance_of(ObjectId(10), child)
                            .with_transform(Affine::translate((2.0, 2.0))),
                    ),
                );
                // A stroked-and-filled shape: the stroke width must scale with
                // this placement's own linear part exactly as a live draw's
                // would (`stroke_placement_scale`), not with some quantised
                // stand-in for it.
                let mut stroked_and_filled = ShapeData::filled(
                    Rect::new(-8.0, -8.0, 8.0, 8.0).to_path(1e-9),
                    Color::from_rgb8(0xC8, 0x3C, 0x3C),
                );
                stroked_and_filled.stroke = Some(buzz_scene::StrokeSpec::new(
                    Color::from_rgb8(0x14, 0x14, 0xC8),
                    1.5,
                ));
                l.frames
                    .push_object(1, Arc::new(Object::shape(ObjectId(11), stroked_and_filled)));
            });
        });

        let layer = scene.add_layer("Stage", LayerKind::Normal);
        let instance = Object::instance_of(ObjectId(100), part).with_transform(transform);
        scene
            .add_object_at(layer, 0, instance)
            .expect("`part` is in the library");

        (scene, part)
    }

    // -- S13a: exact stamps --------------------------------------------------
    // `docs/BIMARI5_LAG_PLAN.md`, S13a. Two root causes found on real Bimari5
    // frames, each reproduced below in a small fixture: (1) a stamp's stroke
    // widths carried the projection's own scale (camera zoom, layer depth),
    // which a live stroke never does; (2) the instance's scale was snapped to
    // 1/32 octave (up to 1.1% off) before it was baked into the child.

    /// A `bar` symbol whose frame 0 holds a stroked-and-filled square and a
    /// long, slightly tilted, heavily stroked bar (shallow edges, so a
    /// sub-pixel drift shows as a long ramp), placed once at `transform`.
    /// Everything is on frame 0, unlike [`symbol_placed_scene`]'s stroked
    /// shape. A speck at `speck_x` drags the symbol's centre away from the
    /// drawn part. Returns `(scene, bar)`.
    fn stroked_bar_scene(transform: Affine, speck_x: f64) -> (Scene, SymbolId) {
        let mut scene = Scene::default();
        let bar = scene.add_symbol("bar", SymbolKind::Graphic, None);
        let bl = first_layer(&scene, bar);
        scene.library_mut().update(bar, |s| {
            s.layers.update(bl, |l| {
                let mut square = ShapeData::filled(
                    Rect::new(-8.0, -8.0, 8.0, 8.0).to_path(1e-9),
                    Color::from_rgb8(0xC8, 0x3C, 0x3C),
                );
                square.stroke = Some(buzz_scene::StrokeSpec::new(
                    Color::from_rgb8(0x14, 0x14, 0xC8),
                    1.5,
                ));
                l.frames.push_object(0, Arc::new(Object::shape(ObjectId(11), square)));

                let mut tilted = buzz_geom::BezPath::new();
                tilted.move_to((-30.0, -3.0));
                tilted.line_to((30.0, 2.0));
                tilted.line_to((30.0, 6.0));
                tilted.line_to((-30.0, 1.0));
                tilted.close_path();
                let mut heavy = ShapeData::filled(tilted, Color::from_rgb8(0x20, 0x90, 0x30));
                heavy.stroke = Some(buzz_scene::StrokeSpec::new(Color::BLACK, 2.0));
                l.frames.push_object(0, Arc::new(Object::shape(ObjectId(12), heavy)));

                // A speck far to the right: it drags the symbol's centre (the
                // stamp's anchor) thousands of units from the drawn part, as
                // in the wide background symbols of a real scene, where the
                // visible edge sits far from the anchor the child is encoded
                // about.
                let speck = ShapeData::filled(
                    Rect::new(speck_x, 0.0, speck_x + 10.0, 10.0).to_path(1e-9),
                    Color::BLACK,
                );
                l.frames.push_object(0, Arc::new(Object::shape(ObjectId(13), speck)));
            });
        });
        let layer = scene.add_layer("Stage", LayerKind::Normal);
        scene
            .add_object_at(
                layer,
                0,
                Object::instance_of(ObjectId(100), bar).with_transform(transform),
            )
            .expect("`bar` is in the library");
        (scene, bar)
    }

    /// [`render_with_cache`] under a scene camera `place` — the affine a
    /// real document's camera / layer depth hands `draw_frame_within`, which
    /// is what makes the projection's scale differ from the stage's.
    fn render_with_cache_under(
        scene: &Scene,
        camera: &Camera,
        options: &FrameOptions,
        cache: &mut DrawCache,
        place: Affine,
    ) -> crate::vello::Scene {
        let mut vs = crate::vello::Scene::new();
        let mut builder = SceneBuilder::new(&mut vs, camera);
        cache.begin();
        draw_frame_within(&mut builder, scene, 0, place, options, cache);
        cache.end();
        vs
    }

    /// Stamped (third draw, a genuine hit) against live, at 1x/4x/16x/40x,
    /// for `scene` drawn under the scene camera `place`; `viewport(zoom)`
    /// frames the content. Also demands that a stamp really was taken.
    fn assert_stamp_matches_live_under(
        gpu: &mut GpuHarness,
        scene: &Scene,
        place: Affine,
        cases: &[(f64, u32)],
        label: &str,
    ) {
        for &(zoom, viewport_px) in cases {
            let camera = Camera::new(
                Point::new(0.0, 0.0),
                zoom,
                Size::new(viewport_px as f64, viewport_px as f64),
            );
            let options = FrameOptions::default();
            let mut cache = DrawCache::new();
            let _sighting = render_with_cache_under(scene, &camera, &options, &mut cache, place);
            let _built = render_with_cache_under(scene, &camera, &options, &mut cache, place);
            assert!(
                cache.symbol_scenes.builds > 0,
                "{label} zoom {zoom}: the second draw must build the child"
            );
            let builds = cache.symbol_scenes.builds;
            let stamped_vs = render_with_cache_under(scene, &camera, &options, &mut cache, place);
            assert_eq!(
                cache.symbol_scenes.builds, builds,
                "{label} zoom {zoom}: the third draw must be a pure hit"
            );

            let mut live_cache = DrawCache::new();
            live_cache.symbol_scenes.enabled = false;
            let live_vs = render_with_cache_under(scene, &camera, &options, &mut live_cache, place);

            let stamped_px = gpu.render_rgba(&stamped_vs, viewport_px, viewport_px);
            let live_px = gpu.render_rgba(&live_vs, viewport_px, viewport_px);
            assert_pixels_match(
                &live_px,
                &stamped_px,
                viewport_px,
                viewport_px,
                &format!("{label} zoom {zoom}x"),
            );
        }
    }

    /// **Root cause 1.** A live stroke is `width × stroke_placement_scale(doc)
    /// × S`: the projection (here a 0.4x scene camera, as a deep layer under a
    /// zoomed-out Bimari5 camera has) never widens the pen. The stamp's
    /// `s_q × |r|` *includes* that 0.4, so its strokes came out 0.4x the live
    /// width (the 2.5x-too-thin window-frame outline at Bimari5 f20) until
    /// `try_stamp_symbol` divided the projection's scale out of the child's
    /// stroke scale.
    #[test]
    fn a_stamp_under_a_shrinking_projection_keeps_the_live_stroke_width() {
        let Some(mut gpu) = GpuHarness::new() else {
            return;
        };
        // Placed at 2.5x, drawn under a 0.4x scene camera: the stamp's own
        // scale `s` is 1.0, the document placement's `sps` is 2.5.
        let (scene, _bar) = stroked_bar_scene(Affine::scale(2.5), 2_000.0);
        assert_stamp_matches_live_under(
            &mut gpu,
            &scene,
            Affine::scale(0.4),
            &[(1.0, 64), (4.0, 128), (16.0, 512), (40.0, 1280)],
            "0.4x projection",
        );
    }

    /// The same at a projection that *magnifies* (a zoomed-in camera, 2.2x),
    /// where the old stamp's strokes came out too thick.
    #[test]
    fn a_stamp_under_a_magnifying_projection_keeps_the_live_stroke_width() {
        let Some(mut gpu) = GpuHarness::new() else {
            return;
        };
        let (scene, _bar) = stroked_bar_scene(Affine::scale(0.5), 2_000.0);
        assert_stamp_matches_live_under(
            &mut gpu,
            &scene,
            Affine::scale(2.2),
            &[(1.0, 64), (4.0, 128), (16.0, 512), (40.0, 1280)],
            "2.2x projection",
        );
    }

    /// **Root cause 2.** A placement whose scale sits off the old 1/32-octave
    /// grid (1.1 is ~0.9% from the nearest grid point) must still stamp to the
    /// live pixels at every zoom: the child is now encoded at the instance's
    /// own scale, to float noise, so nothing is left for the append to make up.
    /// (With the 1/32-octave snap this fixture drifts by more than 2/255 over
    /// the allowed 0.1% of pixels — measured when the fix was made.)
    #[test]
    fn an_off_grid_scale_stamps_to_the_live_pixels() {
        let Some(mut gpu) = GpuHarness::new() else {
            return;
        };
        let (scene, _bar) = stroked_bar_scene(Affine::scale(1.1), 2_000.0);
        assert_stamp_matches_live_under(
            &mut gpu,
            &scene,
            Affine::IDENTITY,
            &[(1.0, 64), (4.0, 128), (16.0, 512), (40.0, 1280)],
            "off-grid scale",
        );
    }

    /// **Known live-path defect, not a stamp one (S13a finding).**
    /// `RenderClip::apply` *clamps* the endpoint of a straight `LineTo` that
    /// lies outside its 3x-visible bounds instead of intersecting the edge
    /// with them, so a long edge crossing the view is drawn with a different
    /// slope live — wrong against the true geometry. A stamp is encoded under
    /// a clip 3x the *symbol's* bounds, so its edge is the true one, and the
    /// two differ wherever a symbol edge runs past 3x the view (Bimari5 f900
    /// 16x/40x: 1.8% / 18% of pixels over 2/255, all of it this). Fixed by
    /// the exact live clip (S13c): the stamped and live pixels agree. The edge
    /// is +-1000 long so the 40x stamp stays under the 1e5 child-size refusal.
    #[test]
    fn a_long_edge_past_the_live_clip_margin_matches_the_live_pixels() {
        let Some(mut gpu) = GpuHarness::new() else {
            return;
        };
        let mut scene = Scene::default();
        let edge = scene.add_symbol("edge", SymbolKind::Graphic, None);
        let el = first_layer(&scene, edge);
        scene.library_mut().update(edge, |s| {
            s.layers.update(el, |l| {
                let mut band = buzz_geom::BezPath::new();
                band.move_to((-1000.0, -30.0));
                band.line_to((1000.0, 30.0));
                band.line_to((1000.0, 34.0));
                band.line_to((-1000.0, -26.0));
                band.close_path();
                let shape = ShapeData::filled(band, Color::BLACK);
                l.frames.push_object(0, Arc::new(Object::shape(ObjectId(11), shape)));
            });
        });
        let layer = scene.add_layer("Stage", LayerKind::Normal);
        scene
            .add_object_at(layer, 0, Object::instance_of(ObjectId(100), edge))
            .expect("`edge` is in the library");
        assert_stamp_matches_live_under(
            &mut gpu,
            &scene,
            Affine::IDENTITY,
            &[(4.0, 128), (40.0, 1280)],
            "long edge",
        );
    }

    /// Five angles of one scale, in one document, share their entries (the
    /// snap folds float noise only), so a limb swinging through many angles is
    /// not one entry each.
    #[test]
    fn rotations_of_one_scale_share_one_stamp_entry() {
        let (mut scene, part) = stroked_bar_scene(Affine::rotate(0.0) * Affine::scale(1.7), 2_000.0);
        let layer = scene.layers().iter().next().expect("the stage layer").id;
        for (i, angle) in [0.31, 1.07, 2.9, -2.2].into_iter().enumerate() {
            scene
                .add_object_at(
                    layer,
                    0,
                    Object::instance_of(ObjectId(200 + i as u64), part)
                        .with_transform(Affine::rotate(angle) * Affine::scale(1.7)),
                )
                .expect("`part` is in the library");
        }
        let camera = Camera::new(Point::new(0.0, 0.0), 4.0, Size::new(128.0, 128.0));
        let options = FrameOptions::default();
        let mut cache = DrawCache::new();
        let _ = render_with_cache(&scene, &camera, &options, &mut cache);
        let _ = render_with_cache(&scene, &camera, &options, &mut cache);
        assert_eq!(
            cache.symbol_scenes.entries.len(),
            1,
            "five angles of one scale must reuse the one entry"
        );
    }

    /// Build once at `transform`, then check the shared S1/S2 GPU proof: a
    /// stamped hit renders pixel-identical to `BUZZ_NO_SYMBOL_CACHE`-style
    /// live drawing, at 1×/4×/16×/40×, and that `try_stamp_symbol` actually
    /// took the non-similar path rather than silently falling back to live
    /// (which would also pass the pixel check, for the wrong reason).
    fn assert_non_similar_stamp_matches_the_live_pixels(gpu: &mut GpuHarness, transform: Affine) {
        let det = transform.determinant();
        assert!(det.abs() > 1e-12, "test placement must be invertible");
        let s = det.abs().sqrt();
        let s_q = 2f64.powf((s.log2() * 32.0).round() / 32.0);
        let [a0, a1, a2, a3, _, _] = transform.as_coeffs();
        let r = [a0 / s_q, a1 / s_q, a2 / s_q, a3 / s_q];
        assert!(
            !is_orthogonal([r[0], r[1], r[2], r[3], 0.0, 0.0], 2.5e-2),
            "test placement must be a genuine non-similarity, not S1's similarity path"
        );
        assert_stamp_takes_non_similar_path_and_matches_live(gpu, transform);
    }

    /// The shared GPU body: no assumption about how non-similar the placement
    /// is, only that the stamp path keys it as non-similar and matches live.
    fn assert_stamp_takes_non_similar_path_and_matches_live(
        gpu: &mut GpuHarness,
        transform: Affine,
    ) {
        let (scene, _part) = symbol_placed_scene(transform);
        let cases: [(f64, u32); 4] = [(1.0, 64), (4.0, 128), (16.0, 512), (40.0, 1280)];

        for (zoom, viewport_px) in cases {
            let camera = Camera::new(
                Point::new(0.0, 0.0),
                zoom,
                Size::new(viewport_px as f64, viewport_px as f64),
            );
            let options = FrameOptions::default();

            let mut cache = DrawCache::new();
            // S2-rest (b): the first draw is a first sighting (live); the
            // second builds.
            let _sighting = render_with_cache(&scene, &camera, &options, &mut cache);
            let _first = render_with_cache(&scene, &camera, &options, &mut cache);
            assert!(
                cache.symbol_scenes.builds > 0,
                "zoom {zoom}: the second draw must build the child at least once"
            );
            assert_eq!(
                cache.symbol_scenes.refused_non_similar, 0,
                "zoom {zoom}: S2 must take this placement via its own exact \
                 linear part, not refuse it"
            );
            assert!(
                !cache.symbol_scenes.entries.is_empty()
                    && cache.symbol_scenes.entries.keys().all(|k| k.non_similar),
                "zoom {zoom}: every entry must be keyed via the non-similar path"
            );
            let builds_after_first = cache.symbol_scenes.builds;

            let stamped_vs = render_with_cache(&scene, &camera, &options, &mut cache);
            assert_eq!(
                cache.symbol_scenes.builds, builds_after_first,
                "zoom {zoom}: the second draw must not rebuild"
            );

            let mut live_cache = DrawCache::new();
            live_cache.symbol_scenes.enabled = false;
            let live_vs = render_with_cache(&scene, &camera, &options, &mut live_cache);

            let stamped_px = gpu.render_rgba(&stamped_vs, viewport_px, viewport_px);
            let live_px = gpu.render_rgba(&live_vs, viewport_px, viewport_px);
            assert_pixels_match(
                &live_px,
                &stamped_px,
                viewport_px,
                viewport_px,
                &format!("zoom {zoom}x"),
            );
        }
    }

    /// A placement 1% off a similarity (well inside the old 2.5e-2 tolerance,
    /// which used to stamp it as a similarity with an approximate stroke
    /// width) now takes the exact linear-part path and matches live pixels.
    #[test]
    fn a_one_percent_anisotropic_placement_stamps_non_similar_and_matches_live() {
        let Some(mut gpu) = GpuHarness::new() else {
            return;
        };
        let transform = Affine::scale_non_uniform(1.5, 1.515);
        let s = transform.determinant().abs().sqrt();
        let [a0, a1, a2, a3, _, _] = transform.as_coeffs();
        assert!(
            is_orthogonal(
                [a0 / s_q_for(s), a1 / s_q_for(s), a2 / s_q_for(s), a3 / s_q_for(s), 0.0, 0.0],
                2.5e-2
            ),
            "fixture must be one the old tolerance called a similarity"
        );
        assert_stamp_takes_non_similar_path_and_matches_live(&mut gpu, transform);
    }

    fn s_q_for(s: f64) -> f64 {
        2f64.powf((s.log2() * 32.0).round() / 32.0)
    }

    #[test]
    fn a_non_uniform_stretch_with_a_stroke_matches_the_live_pixels() {
        let Some(mut gpu) = GpuHarness::new() else {
            return;
        };
        // Unequal x/y scale, no rotation or mirror — the `road1 copy` shape of
        // stretch in Diagnosis §3, simplified to an axis-aligned 2:1.
        assert_non_similar_stamp_matches_the_live_pixels(
            &mut gpu,
            Affine::scale_non_uniform(2.0, 1.0),
        );
    }

    #[test]
    #[ignore = "one AA-fringe column flips under a 0.002 px shift of the live draw itself (GPU float); full-frame parity passes; see docs/BIMARI5_LAG_PLAN.md S13"]
    fn a_mirrored_non_uniform_placement_matches_the_live_pixels() {
        let Some(mut gpu) = GpuHarness::new() else {
            return;
        };
        // The diagnosis's own `newTree copy 4`: mirrored (negative x scale)
        // and non-uniform.
        assert_non_similar_stamp_matches_the_live_pixels(
            &mut gpu,
            Affine::scale_non_uniform(-2.198, 2.052),
        );
    }

    /// **S2-fix Part 1, `docs/BIMARI5_LAG_PLAN.md`.** Renders the same
    /// content **live** (`symbol_scenes.enabled = false` on both sides — no
    /// stamp anywhere in this measurement) from one fixed camera at 1× zoom
    /// twice: once at `transform`, and once at
    /// `Affine::translate((dx, dy)) * transform` — the **placement itself**
    /// wrapped in an extra whole-pixel translation, not the camera. This
    /// matters: shifting the *camera* instead moves the subtraction
    /// `RenderSplit` does in `f64` on the CPU, before Vello's own transform
    /// argument is ever built (`camera.rs`'s rebasing, deliberately exact —
    /// measured separately and confirmed clean, 0/4096 px, 0 max diff, at
    /// these same three offsets). Wrapping the *placement* instead changes
    /// the actual `Affine` handed to Vello's own draw calls by exactly the
    /// translation a stamp's `append` would carry, which is the thing S2's
    /// hypothesis is actually about. At 1× zoom, no rotation, a document
    /// unit is exactly one device pixel, so a perfect rasteriser would
    /// satisfy `shifted[x + dx, y + dy] == base[x, y]` for every pixel whose
    /// shifted position is still on screen. Any difference beyond 2/255 here
    /// is the live path's own sensitivity to a shape's absolute position —
    /// its "self-shift envelope" — with no stamping involved at all.
    /// Returns `(pixels over 2/255, max diff/255)`, counted only over the
    /// region both renders cover.
    fn live_self_shift_envelope(
        gpu: &mut GpuHarness,
        transform: Affine,
        viewport_px: u32,
        dx: i32,
        dy: i32,
    ) -> (usize, i32) {
        let options = FrameOptions::default();
        let size = Size::new(viewport_px as f64, viewport_px as f64);
        let camera = Camera::new(Point::new(0.0, 0.0), 1.0, size);

        let (base_scene, _) = symbol_placed_scene(transform);
        let mut base_cache = DrawCache::new();
        base_cache.symbol_scenes.enabled = false;
        let base_vs = render_with_cache(&base_scene, &camera, &options, &mut base_cache);

        let shifted_transform = Affine::translate((dx as f64, dy as f64)) * transform;
        let (shifted_scene, _) = symbol_placed_scene(shifted_transform);
        let mut shifted_cache = DrawCache::new();
        shifted_cache.symbol_scenes.enabled = false;
        let shifted_vs = render_with_cache(&shifted_scene, &camera, &options, &mut shifted_cache);

        let base_px = gpu.render_rgba(&base_vs, viewport_px, viewport_px);
        let shifted_px = gpu.render_rgba(&shifted_vs, viewport_px, viewport_px);

        let w = viewport_px as i64;
        let h = viewport_px as i64;
        let mut off = 0usize;
        let mut max_diff = 0i32;
        for y in 0..h {
            let ys = y + dy as i64;
            if ys < 0 || ys >= h {
                continue;
            }
            for x in 0..w {
                let xs = x + dx as i64;
                if xs < 0 || xs >= w {
                    continue;
                }
                let base_idx = ((y * w + x) * 4) as usize;
                let shifted_idx = ((ys * w + xs) * 4) as usize;
                let mut diff = 0i32;
                for c in 0..4 {
                    diff = diff.max(
                        (base_px[base_idx + c] as i32 - shifted_px[shifted_idx + c] as i32).abs(),
                    );
                }
                max_diff = max_diff.max(diff);
                if diff > 2 {
                    off += 1;
                }
            }
        }
        (off, max_diff)
    }

    /// Measures [`live_self_shift_envelope`] for the mirrored fixture
    /// (`a_mirrored_non_uniform_placement_matches_the_live_pixels`'s own
    /// placement) at three different integer-pixel shifts, printed so a
    /// `--nocapture` run shows the raw numbers this step's decision rests on.
    #[test]
    fn mirrored_placement_live_self_shift_envelope() {
        let Some(mut gpu) = GpuHarness::new() else {
            return;
        };
        let transform = Affine::scale_non_uniform(-2.198, 2.052);
        for (dx, dy) in [(37, 23), (50, -31), (-44, 18)] {
            let (off, max_diff) = live_self_shift_envelope(&mut gpu, transform, 256, dx, dy);
            eprintln!(
                "self-shift envelope ({dx},{dy}): {off} px > 2/255, max diff {max_diff}/255"
            );
        }
    }

    #[test]
    fn a_rotated_non_uniform_placement_matches_the_live_pixels() {
        let Some(mut gpu) = GpuHarness::new() else {
            return;
        };
        // The diagnosis's own `road1 copy`: rotated 90° composed with an
        // unequal x/y scale.
        assert_non_similar_stamp_matches_the_live_pixels(
            &mut gpu,
            Affine::new([0.0, 0.268, -0.591, 0.0, 0.0, 0.0]),
        );
    }

    #[test]
    fn a_mostly_off_screen_hit_still_stamps_and_matches_the_live_pixels() {
        // Two instances of one plain (similarity, uniform-scale) symbol,
        // sharing one `SymKey`: the first sits fully inside the frame's cull
        // rectangle and must build; the second sits mostly outside it and,
        // before this fix, always fell back to a live, culled draw. It must
        // now take the hit instead — pixel-identical, since the GPU clips the
        // off-screen part either way — and the refusal counter must show it
        // was not refused.
        let mut scene = Scene::default();
        let part = scene.add_symbol("part", SymbolKind::Graphic, None);
        let pl = first_layer(&scene, part);
        scene.library_mut().update(part, |s| {
            s.layers.update(pl, |l| {
                let mut shape = ShapeData::filled(
                    Rect::new(-20.0, -20.0, 20.0, 20.0).to_path(1e-9),
                    Color::from_rgb8(0xC8, 0x3C, 0x3C),
                );
                shape.stroke = Some(buzz_scene::StrokeSpec::new(Color::from_rgb8(0x14, 0x14, 0xC8), 1.5));
                l.frames.push_object(0, Arc::new(Object::shape(ObjectId(1), shape)));
            });
        });

        let layer = scene.add_layer("Stage", LayerKind::Normal);
        // On screen, and inside the cull rectangle below: forces the build.
        scene.add_instance_at(layer, 0, part, Affine::translate((0.0, 0.0)));
        // Bounds become x: [25, 65] — only 5 of its 40 units of width (12.5%)
        // overlap the cull rectangle's x: [-30, 30], well under the 50% rule,
        // but still on screen at every zoom below, so a live draw and a
        // stamped one must show the identical (whole, unclipped-by-`cull`)
        // shape.
        scene.add_instance_at(layer, 0, part, Affine::translate((45.0, 0.0)));

        let cull = Some(Rect::new(-30.0, -30.0, 30.0, 30.0));
        // Viewports a multiple of 64 device pixels, like the other GPU tests
        // in this module: `wgpu`'s readback requires each row's byte count
        // (`width * 4`) to be a multiple of `COPY_BYTES_PER_ROW_ALIGNMENT`
        // (256), which 64 px already satisfies.
        let cases: [(f64, u32); 4] = [(1.0, 64), (4.0, 256), (16.0, 1024), (40.0, 2560)];

        let Some(mut gpu) = GpuHarness::new() else {
            return;
        };
        for (zoom, viewport_px) in cases {
            let camera = Camera::new(
                Point::new(0.0, 0.0),
                zoom,
                Size::new(viewport_px as f64, viewport_px as f64),
            );
            let options = FrameOptions {
                cull,
                ..FrameOptions::default()
            };

            let mut cache = DrawCache::new();
            // S2-rest (b): generation 1 is only a first sighting of the
            // on-screen key (drawn live, no build), and the mostly off-screen
            // instance is a plain first-time miss (refused, drawn live).
            let _sighting = render_with_cache(&scene, &camera, &options, &mut cache);
            assert_eq!(
                cache.symbol_scenes.builds, 0,
                "zoom {zoom}: a first sighting builds nothing"
            );
            let refused_after_sighting = cache.symbol_scenes.refused_under_half_visible;
            // Generation 2 builds the on-screen key; the mostly off-screen
            // instance then hits it.
            let vs = render_with_cache(&scene, &camera, &options, &mut cache);
            assert_eq!(
                cache.symbol_scenes.builds, 1,
                "zoom {zoom}: one build, shared by both instances"
            );
            assert_eq!(
                cache.symbol_scenes.stamps, 2,
                "zoom {zoom}: both instances stamp, including the mostly off-screen one"
            );
            assert_eq!(
                cache.symbol_scenes.refused_under_half_visible, refused_after_sighting,
                "zoom {zoom}: the mostly off-screen hit must not be refused"
            );

            let mut live_cache = DrawCache::new();
            live_cache.symbol_scenes.enabled = false;
            let live_vs = render_with_cache(&scene, &camera, &options, &mut live_cache);

            let stamped_px = gpu.render_rgba(&vs, viewport_px, viewport_px);
            let live_px = gpu.render_rgba(&live_vs, viewport_px, viewport_px);
            assert_pixels_match(
                &live_px,
                &stamped_px,
                viewport_px,
                viewport_px,
                &format!("zoom {zoom}x"),
            );
        }
    }

    #[test]
    fn a_mostly_off_screen_symbol_seen_twice_running_builds_and_matches_the_live_pixels() {
        // No on-screen sibling to force the build via a hit this time: the
        // top-level-instance case (`house8 copy 3` in Diagnosis §3) that S1
        // alone could not reach, because it can never *become* a hit without
        // building at least once. Asked for at the same mostly-off-screen
        // key twice running (two window frames of the same document frame —
        // an idle redraw, or a scrub that lands back on it) is static enough
        // to build; asked for once is not.
        let mut scene = Scene::default();
        let part = scene.add_symbol("part", SymbolKind::Graphic, None);
        let pl = first_layer(&scene, part);
        scene.library_mut().update(part, |s| {
            s.layers.update(pl, |l| {
                let mut shape = ShapeData::filled(
                    Rect::new(-20.0, -20.0, 20.0, 20.0).to_path(1e-9),
                    Color::from_rgb8(0xC8, 0x3C, 0x3C),
                );
                shape.stroke = Some(buzz_scene::StrokeSpec::new(
                    Color::from_rgb8(0x14, 0x14, 0xC8),
                    1.5,
                ));
                l.frames.push_object(0, Arc::new(Object::shape(ObjectId(1), shape)));
            });
        });

        let layer = scene.add_layer("Stage", LayerKind::Normal);
        // Mostly off the cull rectangle below, and nothing else shares its key.
        scene.add_instance_at(layer, 0, part, Affine::translate((45.0, 0.0)));

        let camera = Camera::new(Point::new(0.0, 0.0), 1.0, Size::new(64.0, 64.0));
        let options = FrameOptions {
            cull: Some(Rect::new(-30.0, -30.0, 30.0, 30.0)),
            ..FrameOptions::default()
        };

        let mut cache = DrawCache::new();
        // Generation 1: never seen before, so still refused — a plain miss.
        let _first = render_with_cache(&scene, &camera, &options, &mut cache);
        assert_eq!(cache.symbol_scenes.builds, 0, "a single sighting must not build");
        assert_eq!(cache.symbol_scenes.refused_under_half_visible, 1);

        // Generation 2: the same key, mostly off-screen again — static
        // enough to build now.
        let stamped_vs = render_with_cache(&scene, &camera, &options, &mut cache);
        assert_eq!(
            cache.symbol_scenes.builds, 1,
            "seen mostly off-screen twice running must build"
        );
        assert_eq!(
            cache.symbol_scenes.refused_under_half_visible, 1,
            "the second sighting must not add a refusal"
        );

        // Generation 3: now a genuine hit, via the already-tested hit rule.
        let _third = render_with_cache(&scene, &camera, &options, &mut cache);
        assert_eq!(
            cache.symbol_scenes.builds, 1,
            "the third sighting is a hit, not a rebuild"
        );

        let mut live_cache = DrawCache::new();
        live_cache.symbol_scenes.enabled = false;
        let live_vs = render_with_cache(&scene, &camera, &options, &mut live_cache);

        let Some(mut gpu) = GpuHarness::new() else {
            return;
        };
        let stamped_px = gpu.render_rgba(&stamped_vs, 64, 64);
        let live_px = gpu.render_rgba(&live_vs, 64, 64);
        assert_pixels_match(&live_px, &stamped_px, 64, 64, "generation 2's stamp");
    }

    /// **The measurements [`segment_ceiling`] was fitted to.**
    ///
    /// One dense film, eight output sizes, each level of [`LightDetail`]
    /// pinned, watching for the frame that never landed. What the GPU did is
    /// the authority; the formula has to agree with it in the direction that
    /// matters.
    ///
    /// **Every frame the rasteriser lost, the ceiling must refuse.** That is
    /// the safety property and the only hard one. The converse is not asserted
    /// everywhere: the ceiling refusing something that would in fact have
    /// rendered costs a document some shading, which is the mistake this is
    /// deliberately biased towards. It is asserted for the one size that
    /// prompted the fitting — an ordinary window, where a film's cast shadows
    /// have to survive.
    #[test]
    fn the_ceiling_refuses_every_frame_the_rasteriser_lost() {
        // (width, height, encoded segments, did the frame actually render)
        let measured = [
            (512.0, 512.0, 617_341u32, true),
            (512.0, 512.0, 930_972, true),
            (512.0, 512.0, 2_471_361, false),
            (1024.0, 1024.0, 930_972, true),
            (1024.0, 1024.0, 2_471_361, false),
            (1295.0, 855.0, 921_456, true),
            (1295.0, 855.0, 2_461_927, false),
            (1600.0, 1000.0, 921_536, true),
            (1920.0, 1200.0, 921_529, true),
            (2080.0, 1300.0, 921_539, false),
            (2560.0, 1440.0, 921_630, false),
            (3840.0, 2160.0, 921_633, false),
        ];
        for (w, h, segments, rendered) in measured {
            let ceiling = segment_ceiling(w * h);
            if !rendered {
                assert!(
                    ceiling < segments,
                    "{w}x{h}: {segments} segments were lost by the rasteriser                      and a ceiling of {ceiling} would have let them through"
                );
            }
        }

        // The report this was fitted for: a film's cast shadows must survive an
        // ordinary window. 921 k at a stage of roughly 1295 x 855.
        assert!(
            segment_ceiling(1295.0 * 855.0) >= 921_456,
            "an ordinary window must keep its cast shadows"
        );

        // An output size that is not a size must answer for an ordinary window,
        // not refuse everything. `egui::Rect::NOTHING` multiplies out to an
        // infinite area, and a ceiling of zero would trim a document that had
        // no trouble at all.
        let ordinary = segment_ceiling(REFERENCE_PIXELS);
        for bad in [f64::INFINITY, f64::NAN, 0.0, -1.0, f64::NEG_INFINITY] {
            assert_eq!(segment_ceiling(bad), ordinary, "for {bad}");
        }
        assert!(ordinary > 617_000, "and it must be a workable ceiling");

        // A bigger output must never be allowed *more*, or measuring it was
        // pointless.
        let mut last = u32::MAX;
        for pixels in [0.25e6, 1.0e6, 2.0e6, 4.0e6, 8.0e6] {
            let ceiling = segment_ceiling(pixels);
            assert!(ceiling < last, "the ceiling must fall as the output grows");
            last = ceiling;
        }
    }

    #[test]
    fn the_ladder_stops_at_flat_and_bands_take_over_and_climbs_back_one_at_a_time() {
        // A frame that stays over the ceiling steps down one level per call —
        // `Full -> NoModelling -> Flat` — and then holds, because the
        // automatic ladder stops at `Flat`: the seal is never traded for
        // budget, and a frame still over budget there draws in bands instead
        // (Phase 4d). See `docs/RENDER_SEAMS_PLAN.md` Step 1.
        let mut cache = DrawCache::new();
        let pixels = REFERENCE_PIXELS;
        let ceiling = segment_ceiling(pixels);
        let over = ceiling + 1;

        assert_eq!(cache.detail(), LightDetail::Full, "nothing given up yet");
        assert!(cache.reconsider(over, pixels), "must step down from Full");
        assert_eq!(cache.detail(), LightDetail::NoModelling);
        assert!(cache.reconsider(over, pixels), "must give up the shadows too");
        assert_eq!(cache.detail(), LightDetail::Flat);
        assert!(
            !cache.reconsider(over, pixels),
            "the ladder stops at Flat; the verdict must settle even though the frame is still over"
        );
        assert_eq!(cache.detail(), LightDetail::Flat);
        assert!(
            cache.over_budget(),
            "still over budget at Flat is what sends the caller to bands, not Unsealed"
        );

        // Now the load clears, and the ladder climbs back up — one step at a
        // time, and only once each level's own `room_to_step_up` threshold is
        // cleared, exactly as it does going down.
        let comfortably_under = |detail: LightDetail| detail.room_to_step_up(ceiling) / 2;

        assert!(
            cache.reconsider(comfortably_under(LightDetail::Flat), pixels),
            "must step up to shadows"
        );
        assert_eq!(cache.detail(), LightDetail::NoModelling);
        assert!(
            cache.reconsider(comfortably_under(LightDetail::NoModelling), pixels),
            "must step up to modelling"
        );
        assert_eq!(cache.detail(), LightDetail::Full);
        assert!(
            !cache.reconsider(0, pixels),
            "nowhere higher to go; the verdict must settle"
        );
        assert_eq!(cache.detail(), LightDetail::Full);
    }

    /// V3 in `docs/VRAM_ADAPTIVE_BUFFERS_PLAN.md`: an overflow grows
    /// [`DrawCache::scale`] towards [`DrawCache::set_max_scale`] before it
    /// ever touches [`LightDetail`] — the opposite order from
    /// [`the_ladder_stops_at_flat_and_bands_take_over_and_climbs_back_one_at_a_time`],
    /// which is exactly what a cache nobody has raised `max_scale` on (every
    /// existing caller before V3) still gets.
    #[test]
    fn an_overflow_grows_the_buffer_scale_before_it_trims_detail() {
        let mut cache = DrawCache::new();
        cache.set_max_scale(4);
        let pixels = REFERENCE_PIXELS;
        let ceiling_at_1 = segment_ceiling(pixels);
        let over = ceiling_at_1 + 1;

        assert_eq!(cache.detail(), LightDetail::Full, "nothing given up yet");
        assert_eq!(cache.scale(), 1, "starts at Vello's own upstream sizes");
        assert!(
            cache.reconsider(over, pixels),
            "must grow rather than trim detail — there is room to (max_scale 4)"
        );
        assert_eq!(
            cache.detail(),
            LightDetail::Full,
            "growing the buffer scale must never cost any quality"
        );
        assert_eq!(cache.scale(), 2, "doubled once towards max_scale");
        assert!(
            !cache.reconsider(over, pixels),
            "the same frame now fits at the grown scale; the verdict must settle"
        );
        assert_eq!(cache.detail(), LightDetail::Full);
        assert_eq!(cache.scale(), 2, "settled without growing further");
    }

    /// Growth stops at `max_scale`; a frame still over budget there falls
    /// back to the same detail ladder [`the_ladder_stops_at_flat_and_bands_take_over_and_climbs_back_one_at_a_time`]
    /// exercises, unchanged. See V3.
    #[test]
    fn once_at_max_scale_the_ladder_takes_over_exactly_as_before() {
        let mut cache = DrawCache::new();
        cache.set_max_scale(2);
        let pixels = REFERENCE_PIXELS;
        let ceiling_at_1 = segment_ceiling(pixels);
        // Over budget even once doubled to `max_scale` (2 x the scale-1
        // ceiling), so growth alone cannot save this frame.
        let over = ceiling_at_1 * 3;

        assert!(cache.reconsider(over, pixels), "grows once, to max_scale 2");
        assert_eq!(cache.scale(), 2);
        assert_eq!(
            cache.detail(),
            LightDetail::Full,
            "the grow step itself must not touch detail"
        );
        assert!(
            cache.reconsider(over, pixels),
            "already at max_scale; must step the ladder down instead"
        );
        assert_eq!(cache.scale(), 2, "scale cannot grow past max_scale");
        assert_eq!(
            cache.detail(),
            LightDetail::NoModelling,
            "stepped down exactly as it would with no room to grow at all"
        );
    }

    /// S4 in `docs/BIMARI5_LAG_PLAN.md`: a frame needing 8x reaches 8 in ONE
    /// reconsider (Bimari5's heaviest frame: 7.70 M lines, 2.86 M segments),
    /// then settles with no further growth.
    #[test]
    fn a_frame_needing_8x_reaches_8_in_one_reconsider() {
        let mut cache = DrawCache::new();
        cache.set_max_scale(16);
        let pixels = REFERENCE_PIXELS;
        cache.note_lines(7_700_000);
        assert!(cache.reconsider(2_860_000, pixels));
        assert_eq!(cache.scale(), 8, "one jump, not 1 -> 2 -> 4 -> 8");
        assert_eq!(cache.detail(), LightDetail::Full);
        assert!(!cache.reconsider(2_860_000, pixels), "fits at 8; settles");
        assert_eq!(cache.scale(), 8);
    }

    /// S4: 2.2x of the scale-1 budget goes to the power of two at or above
    /// 2.2 x 1.1 = 2.42, which is 4 (not 2, which would not fit).
    #[test]
    fn a_frame_needing_2_2x_grows_to_4() {
        let mut cache = DrawCache::new();
        cache.set_max_scale(16);
        let pixels = REFERENCE_PIXELS;
        let segments = (f64::from(segment_ceiling(pixels)) * 2.2) as u32;
        assert!(cache.reconsider(segments, pixels));
        assert_eq!(cache.scale(), 4);
        assert!(!cache.reconsider(segments, pixels));
        assert_eq!(cache.scale(), 4);
    }

    /// S4: the jump is clamped to `max_scale`.
    #[test]
    fn the_one_step_jump_is_clamped_at_max_scale() {
        let mut cache = DrawCache::new();
        cache.set_max_scale(4);
        cache.note_lines(7_700_000);
        assert!(cache.reconsider(2_860_000, REFERENCE_PIXELS));
        assert_eq!(cache.scale(), 4, "wanted 8, max_scale is 4");
    }

    /// S4: a frame that fits does not grow.
    #[test]
    fn a_frame_that_fits_does_not_grow() {
        let mut cache = DrawCache::new();
        cache.set_max_scale(16);
        let pixels = REFERENCE_PIXELS;
        cache.note_lines(1_000_000);
        let _ = cache.reconsider(segment_ceiling(pixels) / 2, pixels);
        assert_eq!(cache.scale(), 1);
    }

    /// V4 in `docs/VRAM_ADAPTIVE_BUFFERS_PLAN.md`: [`DrawCache::pending_shrink`]
    /// only ever offers the half scale back once a comfortable frame has
    /// held for [`DrawCache::SHRINK_IDLE`] straight, and
    /// [`DrawCache::commit_shrink`] then adopts it.
    #[test]
    fn shrinking_waits_out_the_idle_window_then_offers_half_the_scale() {
        let mut cache = DrawCache::new();
        cache.set_scale(4);
        let pixels = REFERENCE_PIXELS;
        // A trivial frame: comfortably under even the half-scale (2) budget.
        cache.reconsider(1, pixels);

        let start = Instant::now();
        cache.note_shrink_headroom(pixels, start);
        assert_eq!(
            cache.pending_shrink(),
            None,
            "must wait out the idle window first"
        );
        cache.note_shrink_headroom(pixels, start + Duration::from_secs(19));
        assert_eq!(cache.pending_shrink(), None, "19 s is not yet the 20 s window");
        cache.note_shrink_headroom(pixels, start + Duration::from_secs(20));
        assert_eq!(
            cache.pending_shrink(),
            Some(2),
            "offers half of the current scale (4) once the window has held"
        );

        cache.commit_shrink(2);
        assert_eq!(cache.scale(), 2, "adopts the shrunk scale");
        assert_eq!(
            cache.pending_shrink(),
            None,
            "committing clears the offer"
        );
    }

    /// A frame that stops fitting comfortably resets the idle window rather
    /// than offering a stale shrink — see V4.
    #[test]
    fn an_uncomfortable_frame_cancels_a_pending_shrink() {
        let mut cache = DrawCache::new();
        cache.set_scale(4);
        let pixels = REFERENCE_PIXELS;
        cache.reconsider(1, pixels);

        let start = Instant::now();
        // The first call only starts the idle window; it does not by itself
        // mean the window has already held.
        cache.note_shrink_headroom(pixels, start);
        assert_eq!(cache.pending_shrink(), None, "the idle window has only just started");
        cache.note_shrink_headroom(pixels, start + Duration::from_secs(21));
        assert_eq!(cache.pending_shrink(), Some(2), "comfortable and idle long enough");

        // A frame that would not comfortably fit at half scale — segments
        // right at the half-scale ceiling itself, well past 40% of it.
        let half_ceiling = segment_ceiling_at(pixels, 2);
        cache.reconsider(half_ceiling, pixels);
        cache.note_shrink_headroom(pixels, start + Duration::from_secs(42));
        assert_eq!(
            cache.pending_shrink(),
            None,
            "an uncomfortable frame must cancel the offer rather than keep it"
        );
    }

    #[test]
    fn unsealed_gives_up_about_half_of_flats_segments() {
        // `Unsealed` skips exactly the sub-pixel stroke `fill_shape_paint_sealed`
        // adds to every opaque fill (see `LightDetail`'s doc); nothing else
        // differs from `Flat`, which still seals. An opaque filled shape should
        // therefore cost roughly half as many segments unsealed as it does flat.
        let mut scene = Scene::default();
        let layer = scene.add_layer("Art", LayerKind::Normal);
        scene.add_shape(
            layer,
            ShapeData::filled(Rect::new(60.0, 60.0, 340.0, 340.0).to_path(1e-9), Color::WHITE),
        );
        let camera = Camera::new(Point::new(200.0, 200.0), 1.0, Size::new(400.0, 400.0));

        let measure = |detail: LightDetail| -> u32 {
            let mut vello = crate::vello::Scene::new();
            let mut builder = SceneBuilder::new(&mut vello, &camera);
            let mut cache = DrawCache::new();
            cache.pin_detail(Some(detail));
            cache.begin();
            draw_frame_within(
                &mut builder,
                &scene,
                0,
                Affine::IDENTITY,
                &FrameOptions::default(),
                &mut cache,
            );
            cache.end();
            builder.encoded_segments()
        };

        let flat = measure(LightDetail::Flat);
        let unsealed = measure(LightDetail::Unsealed);
        assert!(
            unsealed * 10 < flat * 6,
            "unsealed ({unsealed}) should be roughly half of flat's sealed cost ({flat})"
        );
    }

    #[test]
    fn a_repeated_redraw_reuses_and_an_edit_rebuilds() {
        let (mut scene, part, _character) = crowd(3, 5);
        let camera = wide_camera();
        let options = FrameOptions::default();
        let mut cache = DrawCache::new();
        cache.set_symbol_reuse(true);

        let go = |scene: &Scene, cache: &mut DrawCache| {
            let mut vello = crate::vello::Scene::new();
            let mut builder = SceneBuilder::new(&mut vello, &camera);
            cache.begin();
            draw_frame_within(&mut builder, scene, 0, Affine::IDENTITY, &options, cache);
            cache.end();
        };

        go(&scene, &mut cache);
        let after_first = cache.symbol_scenes.builds;
        assert_eq!(after_first, 2);

        go(&scene, &mut cache);
        assert_eq!(
            cache.symbol_scenes.builds, after_first,
            "an unchanged redraw stamps from the cache and encodes nothing new"
        );

        // Edit the nested part: its fingerprint, and the character's, both move.
        let pl = first_layer(&scene, part);
        scene.library_mut().update(part, |s| {
            s.layers.update(pl, |l| {
                l.frames
                    .push_object(0, Arc::new(shape_object(77, Rect::new(0.0, 0.0, 5.0, 5.0))));
            });
        });
        go(&scene, &mut cache);
        assert!(
            cache.symbol_scenes.builds > after_first,
            "a nested edit forces a fresh encode"
        );
    }

    #[test]
    fn aging_evicts_unused_entries() {
        // Re-expressed for S2-rest (c): entries no longer age out by
        // generation count; they are evicted, least recently used first, only
        // while the byte total is over `budget_bytes`.
        let (scene, _p, _c) = crowd(3, 3);
        let camera = wide_camera();
        let options = FrameOptions::default();
        let mut cache = DrawCache::new();
        cache.set_symbol_reuse(true);

        // Two draws: a first sighting, then the build.
        let _ = render_with_cache(&scene, &camera, &options, &mut cache);
        let _ = render_with_cache(&scene, &camera, &options, &mut cache);
        assert!(
            !cache.symbol_scenes.entries.is_empty(),
            "the draws cached something"
        );
        assert!(cache.symbol_scenes.bytes > 0, "and the byte total tracks it");

        // Generations pass with nothing drawn; within budget nothing goes.
        for _ in 0..(SYM_KEEP_FRAMES * 5) {
            cache.begin();
            cache.end();
        }
        assert!(
            !cache.symbol_scenes.entries.is_empty(),
            "entries not drawn for many generations survive while within the budget"
        );
        assert_eq!(cache.symbol_scenes.evictions, 0);

        // Squeeze the budget to nothing: the next `end` evicts them all.
        cache.symbol_scenes.budget_bytes = 0;
        cache.begin();
        cache.end();
        assert!(
            cache.symbol_scenes.entries.is_empty(),
            "over budget, unused entries are evicted"
        );
        assert_eq!(cache.symbol_scenes.bytes, 0, "and the byte total follows");
        assert!(cache.symbol_scenes.evictions > 0);
    }

    /// One plain, mostly-visible symbol on stage, for the retention tests.
    fn retention_scene(x: f64) -> Scene {
        let mut scene = Scene::default();
        let part = scene.add_symbol("part", SymbolKind::Graphic, None);
        let pl = first_layer(&scene, part);
        scene.library_mut().update(part, |s| {
            s.layers.update(pl, |l| {
                let mut shape = ShapeData::filled(
                    Rect::new(-8.0, -8.0, 8.0, 8.0).to_path(1e-9),
                    Color::from_rgb8(0xC8, 0x3C, 0x3C),
                );
                shape.stroke = Some(buzz_scene::StrokeSpec::new(
                    Color::from_rgb8(0x14, 0x14, 0xC8),
                    1.5,
                ));
                l.frames.push_object(0, Arc::new(Object::shape(ObjectId(1), shape)));
            });
        });
        let layer = scene.add_layer("Stage", LayerKind::Normal);
        scene.add_instance_at(layer, 0, part, Affine::translate((x, 0.0)));
        scene
    }

    #[test]
    fn an_entry_unused_for_ten_generations_is_still_a_hit() {
        let scene = retention_scene(0.0);
        let camera = Camera::new(Point::new(0.0, 0.0), 1.0, Size::new(64.0, 64.0));
        let options = FrameOptions::default();
        let mut cache = DrawCache::new();
        let _ = render_with_cache(&scene, &camera, &options, &mut cache); // sighting
        let _ = render_with_cache(&scene, &camera, &options, &mut cache); // build
        assert_eq!(cache.symbol_scenes.builds, 1);
        let stamps = cache.symbol_scenes.stamps;

        for _ in 0..10 {
            cache.begin();
            cache.end();
        }
        assert_eq!(cache.symbol_scenes.entries.len(), 1, "kept across 10 idle generations");

        let _ = render_with_cache(&scene, &camera, &options, &mut cache);
        assert_eq!(cache.symbol_scenes.builds, 1, "a revisit is a hit, not a rebuild");
        assert_eq!(cache.symbol_scenes.stamps, stamps + 1);
    }

    #[test]
    fn a_tiny_budget_evicts_the_lru_entry_and_the_result_still_matches_live() {
        // Two placements of the same symbol at two different zooms are two
        // keys. Build A, then B; with a budget of one entry's size, B's build
        // evicts A (the least recently used), and drawing A again — a fresh
        // build — still matches the live pixels.
        let scene = retention_scene(0.0);
        let options = FrameOptions::default();
        let cam_a = Camera::new(Point::new(0.0, 0.0), 1.0, Size::new(64.0, 64.0));
        let cam_b = Camera::new(Point::new(0.0, 0.0), 4.0, Size::new(64.0, 64.0));

        let mut cache = DrawCache::new();
        for cam in [&cam_a, &cam_b] {
            let _ = render_with_cache(&scene, cam, &options, &mut cache); // sighting
            let _ = render_with_cache(&scene, cam, &options, &mut cache); // build
        }
        assert_eq!(cache.symbol_scenes.entries.len(), 2, "both keys built");
        let one = cache.symbol_scenes.bytes / 2;
        assert!(one > 0);

        // Touch B last so A is the LRU, then shrink the budget to fit one.
        let _ = render_with_cache(&scene, &cam_b, &options, &mut cache);
        cache.symbol_scenes.budget_bytes = cache.symbol_scenes.bytes - 1;
        cache.begin();
        cache.end();
        assert_eq!(cache.symbol_scenes.entries.len(), 1, "one entry evicted");
        assert_eq!(cache.symbol_scenes.evictions, 1);
        assert!(cache.symbol_scenes.bytes <= cache.symbol_scenes.budget_bytes);
        let builds = cache.symbol_scenes.builds;

        // B (recently used) is still a hit.
        let _ = render_with_cache(&scene, &cam_b, &options, &mut cache);
        assert_eq!(cache.symbol_scenes.builds, builds, "the survivor is B, a hit");
        // A was evicted: its sighting is still recorded, so it rebuilds at once.
        let stamped_vs = render_with_cache(&scene, &cam_a, &options, &mut cache);
        assert_eq!(cache.symbol_scenes.builds, builds + 1, "the evicted A rebuilds");

        let mut live_cache = DrawCache::new();
        live_cache.symbol_scenes.enabled = false;
        let live_vs = render_with_cache(&scene, &cam_a, &options, &mut live_cache);
        let Some(mut gpu) = GpuHarness::new() else {
            return;
        };
        let stamped_px = gpu.render_rgba(&stamped_vs, 64, 64);
        let live_px = gpu.render_rgba(&live_vs, 64, 64);
        assert_pixels_match(&live_px, &stamped_px, 64, 64, "rebuilt after eviction");
    }

    #[test]
    fn a_first_sighting_is_drawn_live_and_the_second_builds_and_matches_live() {
        let scene = retention_scene(0.0);
        let options = FrameOptions::default();
        let Some(mut gpu) = GpuHarness::new() else {
            return;
        };
        for (zoom, px) in [(1.0, 64u32), (4.0, 128)] {
            let camera =
                Camera::new(Point::new(0.0, 0.0), zoom, Size::new(px as f64, px as f64));
            let mut cache = DrawCache::new();

            // First sighting: drawn live — nothing built, nothing inserted.
            let first_vs = render_with_cache(&scene, &camera, &options, &mut cache);
            assert_eq!(cache.symbol_scenes.builds, 0, "zoom {zoom}: no build on a first sighting");
            assert_eq!(cache.symbol_scenes.stamps, 0);
            assert!(cache.symbol_scenes.entries.is_empty(), "zoom {zoom}: nothing inserted");
            assert_eq!(cache.symbol_scenes.first_sightings, 1);

            // Second sighting (the next generation): built and inserted.
            let second_vs = render_with_cache(&scene, &camera, &options, &mut cache);
            assert_eq!(cache.symbol_scenes.builds, 1, "zoom {zoom}: built on the second sighting");
            assert_eq!(cache.symbol_scenes.entries.len(), 1);

            let mut live_cache = DrawCache::new();
            live_cache.symbol_scenes.enabled = false;
            let live_vs = render_with_cache(&scene, &camera, &options, &mut live_cache);

            let live_px = gpu.render_rgba(&live_vs, px, px);
            let first_px = gpu.render_rgba(&first_vs, px, px);
            let second_px = gpu.render_rgba(&second_vs, px, px);
            assert_pixels_match(&live_px, &first_px, px, px, &format!("zoom {zoom}x first sighting"));
            assert_pixels_match(&live_px, &second_px, px, px, &format!("zoom {zoom}x second sighting"));
        }
    }

    #[test]
    fn build_on_first_sighting_builds_a_first_miss_at_once_and_matches_live() {
        let scene = retention_scene(0.0);
        let options = FrameOptions::default();
        let Some(mut gpu) = GpuHarness::new() else {
            return;
        };
        for (zoom, px) in [(1.0, 64u32), (4.0, 128)] {
            let camera =
                Camera::new(Point::new(0.0, 0.0), zoom, Size::new(px as f64, px as f64));
            let mut cache = DrawCache::new();
            cache.set_build_on_first_sighting(true);
            let vs = render_with_cache(&scene, &camera, &options, &mut cache);
            assert_eq!(cache.symbol_scenes.builds, 1, "zoom {zoom}: built on the first miss");
            assert_eq!(cache.symbol_scenes.first_sightings, 0);
            assert_eq!(cache.symbol_scenes.entries.len(), 1);

            let mut live_cache = DrawCache::new();
            live_cache.symbol_scenes.enabled = false;
            let live_vs = render_with_cache(&scene, &camera, &options, &mut live_cache);
            let live_px = gpu.render_rgba(&live_vs, px, px);
            let stamped_px = gpu.render_rgba(&vs, px, px);
            assert_pixels_match(&live_px, &stamped_px, px, px, &format!("zoom {zoom}x first-miss build"));
        }
    }

    /// The encoding streams of a scene as one byte string, for exact comparison.
    fn encoding_bytes(vs: &crate::vello::Scene) -> Vec<u8> {
        fn pod<T>(v: &[T]) -> &[u8] {
            // SAFETY: only called on vello_encoding's `Pod` streams, read-only.
            unsafe { std::slice::from_raw_parts(v.as_ptr() as *const u8, std::mem::size_of_val(v)) }
        }
        let e = vs.encoding();
        let mut out = Vec::new();
        out.extend_from_slice(pod(e.path_tags.as_slice()));
        out.extend_from_slice(pod(e.path_data.as_slice()));
        out.extend_from_slice(pod(e.draw_tags.as_slice()));
        out.extend_from_slice(pod(e.draw_data.as_slice()));
        out.extend_from_slice(pod(e.transforms.as_slice()));
        out.extend_from_slice(pod(e.styles.as_slice()));
        out.extend_from_slice(
            format!("{} {} {:?}", e.n_clips, e.n_open_clips, e.resources.color_stops).as_bytes(),
        );
        out
    }

    /// A cache primed so the next draw builds the **top-level** stamp cold:
    /// its key is sighted (so it builds at once) but every entry is gone and
    /// every nested key is unsighted, so a nested miss has to build inside the
    /// stamp build on its own. E3 in `docs/BIMARI5_LAG_PLAN.md`.
    fn cold_top_state(scene: &Scene, camera: &Camera, options: &FrameOptions) -> DrawCache {
        let mut cache = DrawCache::new();
        let _ = render_with_cache(scene, camera, options, &mut cache); // sighting
        let sc = &mut cache.symbol_scenes;
        let top = sc.sighted.keys().map(|k| k.depth).min().expect("a sighting");
        sc.sighted.retain(|k, _| k.depth == top);
        sc.entries.clear();
        sc.bytes = 0;
        sc.seen_off_screen.clear();
        cache
    }

    #[test]
    fn a_cold_built_stamp_equals_the_same_stamp_built_with_nested_entries_present() {
        let (scene, _p, _c) = crowd(3, 1);
        let camera = Camera::new(Point::new(5.0, 17.0), 2.0, Size::new(128.0, 128.0));
        let options = FrameOptions::default();
        let mut cache = cold_top_state(&scene, &camera, &options);

        let _ = render_with_cache(&scene, &camera, &options, &mut cache);
        let top = cache.symbol_scenes.entries.keys().map(|k| k.depth).min().expect("built");
        assert!(
            cache.symbol_scenes.entries.keys().any(|k| k.depth > top),
            "the cold build must have built its nested stamp too"
        );
        let cold = encoding_bytes(
            &cache
                .symbol_scenes
                .entries
                .iter()
                .find(|(k, _)| k.depth == top)
                .expect("top entry")
                .1
                .scene,
        );

        // Drop only the top entry: the nested ones stay, so the rebuild stamps
        // them from the cache instead of building them.
        cache.symbol_scenes.entries.retain(|k, _| k.depth != top);
        cache.symbol_scenes.bytes = cache.symbol_scenes.entries.values().map(|e| e.bytes).sum();
        let builds = cache.symbol_scenes.builds;
        let _ = render_with_cache(&scene, &camera, &options, &mut cache);
        assert_eq!(cache.symbol_scenes.builds, builds + 1, "only the top rebuilt");
        let warm = encoding_bytes(
            &cache
                .symbol_scenes
                .entries
                .iter()
                .find(|(k, _)| k.depth == top)
                .expect("rebuilt top entry")
                .1
                .scene,
        );
        assert!(cold == warm, "a stamp's bytes must not depend on what was cached");
    }

    #[test]
    fn a_cold_built_nested_stamp_matches_the_live_pixels_at_1x_4x_and_16x() {
        let (scene, _p, _c) = crowd(2, 1);
        let options = FrameOptions::default();
        let Some(mut gpu) = GpuHarness::new() else {
            return;
        };
        for (zoom, px) in [(1.0, 64u32), (4.0, 128), (16.0, 384)] {
            let camera = Camera::new(Point::new(5.0, 11.0), zoom, Size::new(px as f64, px as f64));
            let mut cache = cold_top_state(&scene, &camera, &options);
            let cold_vs = render_with_cache(&scene, &camera, &options, &mut cache);
            assert!(
                cache.symbol_scenes.builds >= 2,
                "zoom {zoom}: the top stamp and its nested stamp were both built"
            );

            let mut live_cache = DrawCache::new();
            live_cache.symbol_scenes.enabled = false;
            let live_vs = render_with_cache(&scene, &camera, &options, &mut live_cache);
            let live_px = gpu.render_rgba(&live_vs, px, px);
            let cold_px = gpu.render_rgba(&cold_vs, px, px);
            assert_pixels_match(&live_px, &cold_px, px, px, &format!("zoom {zoom}x cold nested build"));
        }
    }

    /// A cache that builds a stamp on its first miss and records what it built,
    /// the way a pre-build worker's does (E6).
    fn collecting_cache() -> DrawCache {
        let mut cache = DrawCache::new();
        cache.set_build_on_first_sighting(true);
        cache.set_collect_new_stamps(true);
        cache
    }

    #[test]
    fn installed_stamps_are_hits_and_equal_the_stamps_built_locally() {
        let (scene, _p, _c) = crowd(2, 3);
        let camera = Camera::new(Point::new(5.0, 17.0), 2.0, Size::new(128.0, 128.0));
        let options = FrameOptions::default();

        // The worker's cache builds on first sighting and hands over what it built.
        let mut worker = collecting_cache();
        let _ = render_with_cache(&scene, &camera, &options, &mut worker);
        let stamps = worker.drain_new_stamps();
        assert!(stamps.len() >= 2, "the top stamp and its nested stamp were built");
        assert!(worker.drain_new_stamps().is_empty(), "a drain empties the list");
        let worker_bytes: Vec<(SymKey, Vec<u8>)> =
            stamps.iter().map(|s| (s.key, encoding_bytes(&s.scene))).collect();

        // A main cache that never saw the scene takes them.
        let mut main = DrawCache::new();
        let n = stamps.len();
        assert_eq!(main.install_stamps(stamps), n);
        assert_eq!(main.symbol_scenes.installed, n as u64);
        assert!(worker_bytes.iter().all(|(k, _)| main.stamp_is_transient(k)));
        assert_eq!(
            main.symbol_scenes.bytes,
            worker_bytes.iter().map(|(k, _)| main.symbol_scenes.entries[k].bytes).sum::<usize>(),
            "installed bytes are counted"
        );

        // The next frame hits them: nothing is built, nothing is drawn live.
        let _ = render_with_cache(&scene, &camera, &options, &mut main);
        assert_eq!(main.symbol_scenes.builds, 0, "an installed key is a hit, not a build");
        assert_eq!(main.symbol_scenes.first_sightings, 0, "and not a first sighting");
        assert!(main.symbol_scenes.stamps > 0);
        assert!(
            worker_bytes.iter().any(|(k, _)| !main.stamp_is_transient(k)),
            "a used stamp is retained"
        );

        // The same keys built locally have the very same bytes.
        let mut local = collecting_cache();
        let _ = render_with_cache(&scene, &camera, &options, &mut local);
        let local_stamps = local.drain_new_stamps();
        assert_eq!(local_stamps.len(), worker_bytes.len());
        for s in &local_stamps {
            let (_, theirs) = worker_bytes
                .iter()
                .find(|(k, _)| *k == s.key)
                .expect("the same keys are built");
            assert!(*theirs == encoding_bytes(&s.scene), "{:?}: bytes differ", s.key);
        }

        // Installing a key already held changes nothing.
        let again = local.drain_new_stamps();
        assert!(again.is_empty());
        let held = main.symbol_scenes.bytes;
        assert_eq!(main.install_stamps(local_stamps), 0);
        assert_eq!(main.symbol_scenes.bytes, held);
    }

    #[test]
    fn a_frame_drawn_after_installs_matches_the_live_pixels_at_1x_and_4x() {
        let (scene, _p, _c) = crowd(2, 3);
        let options = FrameOptions::default();
        let Some(mut gpu) = GpuHarness::new() else {
            return;
        };
        for (zoom, px) in [(1.0, 64u32), (4.0, 128)] {
            let camera = Camera::new(Point::new(5.0, 11.0), zoom, Size::new(px as f64, px as f64));
            let mut worker = collecting_cache();
            let _ = render_with_cache(&scene, &camera, &options, &mut worker);
            let mut main = DrawCache::new();
            assert!(main.install_stamps(worker.drain_new_stamps()) >= 2);
            let installed_vs = render_with_cache(&scene, &camera, &options, &mut main);
            assert_eq!(main.symbol_scenes.builds, 0, "zoom {zoom}: every stamp was installed");
            assert!(main.symbol_scenes.stamps > 0);

            let mut live_cache = DrawCache::new();
            live_cache.symbol_scenes.enabled = false;
            let live_vs = render_with_cache(&scene, &camera, &options, &mut live_cache);
            let live_px = gpu.render_rgba(&live_vs, px, px);
            let installed_px = gpu.render_rgba(&installed_vs, px, px);
            assert_pixels_match(
                &live_px,
                &installed_px,
                px,
                px,
                &format!("zoom {zoom}x after installs"),
            );
        }
    }

    fn pressure_key(i: u64) -> SymKey {
        SymKey {
            fingerprint: i,
            inner: 0,
            depth: 0,
            scale_bits: 0,
            sealed: true,
            stroke_scale_bits: 1.0f64.to_bits(),
            effect_bits: effect_key_bits(&ColorTransform::default()),
            non_similar: false,
            linear_bits: [0; 4],
            lit: false,
            light_print: 0,
            light_depth_bits: 0,
            stage_height_bits: 0,
        }
    }

    #[test]
    fn transient_stamps_are_evicted_before_retained_ones_under_budget_pressure() {
        let mut scene = Scene::default();
        let sym_id = scene.add_symbol("s", SymbolKind::Graphic, None);
        let sym = Arc::clone(scene.library().get(sym_id).unwrap());
        let dummy = Arc::new(crate::vello::Scene::new());

        let mut sc = SymbolSceneCache::default();
        sc.begin();
        // Two retained entries, oldest first; both bytes = 100.
        for i in 1..=2u64 {
            sc.clock += 1;
            sc.entries.insert(
                pressure_key(i),
                SymEntry {
                    scene: Arc::clone(&dummy),
                    symbol: Arc::clone(&sym),
                    anchor: Point::ORIGIN,
                    used: sc.frame,
                    lines: 0,
                    bytes: 100,
                    tick: sc.clock,
                    silhouette: None,
                },
            );
            sc.bytes += 100;
        }
        // An installed (transient) stamp: the *newest* by LRU order.
        let installed = sc.install(NewStamp {
            key: pressure_key(3),
            scene: Arc::clone(&dummy),
            bytes: 100,
            symbol: Arc::clone(&sym),
            anchor: Point::ORIGIN,
            lines: 0,
            silhouette: None,
        });
        assert!(installed);
        assert_eq!(sc.bytes, 300);
        assert!(sc.transient.contains(&pressure_key(3)));

        // 50 bytes over: plain LRU would drop key 1; the transient one goes.
        sc.budget_bytes = 250;
        sc.end();
        assert!(!sc.entries.contains_key(&pressure_key(3)), "the transient entry went first");
        assert!(sc.entries.contains_key(&pressure_key(1)));
        assert!(sc.entries.contains_key(&pressure_key(2)));
        assert!(sc.transient.is_empty());
        assert_eq!(sc.bytes, 200);

        // With nothing transient left the order is the plain LRU.
        sc.budget_bytes = 150;
        sc.end();
        assert!(!sc.entries.contains_key(&pressure_key(1)), "then the least recently used");
        assert!(sc.entries.contains_key(&pressure_key(2)));
    }

    /// S9: warm-up stamps are retained (never transient), the entry ceiling
    /// follows `set_stamp_limits`, and only the byte budget decides after it.
    #[test]
    fn warm_stamps_are_retained_and_the_entry_cap_is_raised_by_the_limits() {
        let mut scene = Scene::default();
        let sym_id = scene.add_symbol("s", SymbolKind::Graphic, None);
        let sym = Arc::clone(scene.library().get(sym_id).unwrap());
        let dummy = Arc::new(crate::vello::Scene::new());
        let stamp = |i: u64| NewStamp {
            key: pressure_key(i),
            scene: Arc::clone(&dummy),
            bytes: 10,
            symbol: Arc::clone(&sym),
            anchor: Point::ORIGIN,
            lines: 0,
            silhouette: None,
        };

        let mut cache = DrawCache::new();
        let n = SYM_CACHE_CAP + 100;
        let installed = cache.install_stamps_retained((1..=n as u64).map(stamp).collect());
        assert_eq!(installed, n);
        assert!(
            (1..=n as u64).all(|i| !cache.stamp_is_transient(&pressure_key(i))),
            "retained, not transient"
        );
        // The default ceiling would evict down to SYM_CACHE_CAP at the next `end`;
        // raised, nothing goes while the bytes fit the budget.
        cache.set_stamp_limits(10 * n, usize::MAX);
        cache.begin();
        cache.end();
        assert_eq!(cache.stamp_count(), n);
        assert_eq!(cache.stamp_bytes(), 10 * n);
        // Over the byte budget: the least recently used go, the rest stay.
        cache.set_stamp_limits(10 * (n - 5), usize::MAX);
        cache.begin();
        cache.end();
        assert_eq!(cache.stamp_count(), n - 5);
        assert!(cache.stamp_scene(&pressure_key(1)).is_none(), "oldest first");
        assert!(cache.stamp_scene(&pressure_key(n as u64)).is_some());
        // A zoom change drops everything at once and hands the encodings back.
        let taken = cache.take_all_stamps();
        assert_eq!(taken.len(), n - 5);
        assert_eq!((cache.stamp_count(), cache.stamp_bytes()), (0, 0));
    }

    /// S9 window mode: a full cache slides along the playback. `end` evicts to
    /// 90% of the budget, `make_room_for_stamps` frees the least recently used
    /// retained stamps (never an installed, unused one) down to 80%; outside
    /// window mode neither changes, and a full cache takes no more.
    #[test]
    fn a_window_cache_evicts_in_tenths_and_makes_room_for_stamps_built_ahead() {
        let mut scene = Scene::default();
        let sym_id = scene.add_symbol("s", SymbolKind::Graphic, None);
        let sym = Arc::clone(scene.library().get(sym_id).unwrap());
        let dummy = Arc::new(crate::vello::Scene::new());
        let stamp = |i: u64| NewStamp {
            key: pressure_key(i),
            scene: Arc::clone(&dummy),
            bytes: 10,
            symbol: Arc::clone(&sym),
            anchor: Point::ORIGIN,
            lines: 0,
            silhouette: None,
        };
        let filled = |window: bool| {
            let mut cache = DrawCache::new();
            cache.set_stamp_limits(1000, usize::MAX);
            cache.set_stamp_window(window);
            // 98 retained stamps (oldest = key 1) and 2 installed, unused ones.
            cache.install_stamps_retained((1..=98).map(stamp).collect());
            cache.install_stamps(vec![stamp(99), stamp(100)]);
            assert_eq!(cache.stamp_bytes(), 1000);
            cache
        };

        // Not a window: a full cache takes no more and `end` stops at the budget.
        let mut plain = filled(false);
        assert!(!plain.make_room_for_stamps(), "no room is made outside window mode");
        assert_eq!(plain.stamp_count(), 100);
        plain.set_stamp_limits(990, usize::MAX);
        plain.begin();
        plain.end();
        assert_eq!(plain.stamp_bytes(), 990, "evicts only down to the budget");

        // A window: room is made from the oldest retained stamps, at 80%.
        let mut window = filled(true);
        assert!(window.make_room_for_stamps());
        assert_eq!(window.stamp_bytes(), 800);
        assert!(window.stamp_scene(&pressure_key(1)).is_none(), "the oldest went first");
        assert!(window.stamp_scene(&pressure_key(20)).is_none() && window.stamp_scene(&pressure_key(21)).is_some());
        assert!(window.stamp_scene(&pressure_key(99)).is_some(), "an unused stamp is kept");
        assert!(window.stamp_scene(&pressure_key(100)).is_some());
        // Under 90% already: nothing is touched.
        assert!(window.make_room_for_stamps());
        assert_eq!(window.stamp_bytes(), 800);
        // And `end` over the budget goes to 90%, not to the budget.
        window.install_stamps_retained((200..230).map(stamp).collect());
        assert!(window.stamp_bytes() > 1000);
        window.begin();
        window.end();
        assert!(window.stamp_bytes() <= 900 && window.stamp_bytes() > 800, "{}", window.stamp_bytes());
    }

    /// S9: the size-once formula is exactly the jump `reconsider` makes (S4).
    #[test]
    fn fitting_scale_is_the_scale_reconsider_jumps_to() {
        let pixels = REFERENCE_PIXELS;
        for (lines, segments) in [
            (1_000_000u64, 100_000u32),
            (7_700_000, 1_000_000),
            (3_000_000, 4_000_000),
            (40_000_000, 100),
            (10, 10),
        ] {
            let mut cache = DrawCache::new();
            cache.set_max_scale(64);
            cache.note_lines(lines);
            let fit = fitting_scale(lines, segments, pixels, 64);
            if cache.reconsider(segments, pixels) {
                assert_eq!(
                    cache.scale(),
                    fit.max(2),
                    "lines {lines} segments {segments}: one jump to the fitting scale"
                );
            } else {
                assert_eq!(fit, 1, "a frame that fits the base buffers needs scale 1");
            }
        }
        assert_eq!(fitting_scale(40_000_000, 100, pixels, 4), 4, "clamped to max_scale");
    }

    /// S7 (stop latency): with `defer_frees` on, what `end` evicts is handed to the
    /// owner (`take_graveyard`) instead of being freed inside `end`; the eviction
    /// order and byte accounting are exactly as without it.
    #[test]
    fn deferred_frees_hand_the_evicted_encodings_to_the_owner() {
        let mut scene = Scene::default();
        let sym_id = scene.add_symbol("s", SymbolKind::Graphic, None);
        let sym = Arc::clone(scene.library().get(sym_id).unwrap());
        let mut run = |defer: bool| {
            let mut sc = SymbolSceneCache::default();
            sc.defer_frees = defer;
            sc.begin();
            let scenes: Vec<Arc<crate::vello::Scene>> =
                (0..3).map(|_| Arc::new(crate::vello::Scene::new())).collect();
            for i in 1..=3u64 {
                sc.clock += 1;
                sc.entries.insert(
                    pressure_key(i),
                    SymEntry {
                        scene: Arc::clone(&scenes[i as usize - 1]),
                        symbol: Arc::clone(&sym),
                        anchor: Point::ORIGIN,
                        used: sc.frame,
                        lines: 0,
                        bytes: 100,
                        tick: sc.clock,
                        silhouette: None,
                    },
                );
                sc.bytes += 100;
            }
            sc.budget_bytes = 150;
            sc.end();
            let mut kept: Vec<u64> = (1..=3).filter(|i| sc.entries.contains_key(&pressure_key(*i))).collect();
            kept.sort_unstable();
            // How many encodings only the test's own handle still holds: the cache
            // freed them (or never had them).
            let freed = scenes.iter().filter(|s| Arc::strong_count(s) == 1).count();
            (kept, sc.bytes, sc.evictions, sc.graveyard.len(), freed)
        };
        let (kept_a, bytes_a, ev_a, grave_a, freed_a) = run(false);
        let (kept_b, bytes_b, ev_b, grave_b, freed_b) = run(true);
        assert_eq!((kept_a, bytes_a, ev_a), (kept_b, bytes_b, ev_b), "same eviction either way");
        assert_eq!(grave_a, 0, "nothing is kept back without the flag");
        assert_eq!(grave_b, 2, "the two evicted encodings are handed over");
        // Without the flag the two evicted encodings were freed (only the test's
        // own handle is left); with it the graveyard still holds them.
        assert_eq!(freed_a, 2);
        assert_eq!(freed_b, 0);
    }

    /// S7 (RSS): a worker that lets go of its stamps keeps none of them alive.
    #[test]
    fn dropping_stamps_leaves_the_cache_empty_and_the_stamps_with_their_owners() {
        let (scene, _p, _c) = crowd(2, 1);
        let camera = Camera::new(Point::new(5.0, 17.0), 2.0, Size::new(128.0, 128.0));
        let options = FrameOptions::default();
        let mut worker = collecting_cache();
        let _ = render_with_cache(&scene, &camera, &options, &mut worker);
        let stamps = worker.drain_new_stamps();
        assert!(!stamps.is_empty(), "the worker built something");
        worker.drop_stamps();
        assert_eq!(worker.symbol_scenes.retention_stats().0, 0);
        assert_eq!(worker.symbol_scenes.retention_stats().1, 0);
        // The drained stamps are still whole (their `Arc`s are the only holders).
        assert!(stamps.iter().all(|s| Arc::strong_count(&s.scene) == 1));
    }

    #[test]
    fn a_used_installed_stamp_is_retained_and_an_unused_one_is_not() {
        let (scene, _p, _c) = crowd(2, 1);
        let camera = Camera::new(Point::new(5.0, 17.0), 2.0, Size::new(128.0, 128.0));
        let options = FrameOptions::default();
        let mut worker = collecting_cache();
        let _ = render_with_cache(&scene, &camera, &options, &mut worker);
        let mut main = DrawCache::new();
        main.install_stamps(worker.drain_new_stamps());
        let keys: Vec<SymKey> = main.symbol_scenes.entries.keys().copied().collect();
        assert!(keys.iter().all(|k| main.stamp_is_transient(k)));
        let _ = render_with_cache(&scene, &camera, &options, &mut main);
        assert!(
            keys.iter().any(|k| !main.stamp_is_transient(k)),
            "a stamp a frame used is no longer transient"
        );
    }

    #[test]
    fn a_symbol_requested_twice_in_one_generation_builds_on_the_second_request() {
        // Three instances of one symbol in one frame: the first is drawn live,
        // the second builds, the third is a hit, so a symbol placed many
        // times on a frame pays for at most one live draw before it is cached.
        let (mut scene, part, _character) = crowd(0, 0);
        let layer = scene.add_layer("Stage", LayerKind::Normal);
        for i in 0..3 {
            scene.add_instance_at(layer, 0, part, Affine::translate((i as f64 * 40.0, 0.0)));
        }
        let mut cache = DrawCache::new();
        let _ = render_with_cache(&scene, &wide_camera(), &FrameOptions::default(), &mut cache);
        assert_eq!(cache.symbol_scenes.first_sightings, 1);
        assert_eq!(cache.symbol_scenes.builds, 1);
        assert_eq!(cache.symbol_scenes.stamps, 2, "the building instance and one hit");
    }

    #[test]
    fn the_entry_count_is_capped() {
        let mut scene = Scene::default();
        let sym_id = scene.add_symbol("s", SymbolKind::Graphic, None);
        let sym = Arc::clone(scene.library().get(sym_id).unwrap());
        let dummy = Arc::new(crate::vello::Scene::new());

        let mut sc = SymbolSceneCache::default();
        sc.begin();
        for i in 0..(SYM_CACHE_CAP + 50) {
            sc.entries.insert(
                SymKey {
                    fingerprint: i as u64,
                    inner: 0,
                    depth: 0,
                    scale_bits: 0,
                    sealed: true,
                    stroke_scale_bits: 1.0f64.to_bits(),
                    effect_bits: effect_key_bits(&ColorTransform::default()),
                    non_similar: false,
                    linear_bits: [0; 4],
                    lit: false,
                    light_print: 0,
                    light_depth_bits: 0,
                    stage_height_bits: 0,
                },
                SymEntry {
                    scene: Arc::clone(&dummy),
                    symbol: Arc::clone(&sym),
                    anchor: Point::ORIGIN,
                    used: sc.frame,
                    lines: 0,
                    bytes: 0,
                    tick: i as u64,
                    silhouette: None,
                },
            );
        }
        sc.end();
        assert!(
            sc.entries.len() <= SYM_CACHE_CAP,
            "the cap bounds live entries at {SYM_CACHE_CAP}, got {}",
            sc.entries.len()
        );
    }

    /// For any scene the cache cannot take, it must build nothing, stamp nothing,
    /// and leave the encoding identical to drawing with the cache off — because
    /// the fallback *is* the live path.
    fn assert_ineligible(scene: &Scene, camera: &Camera, options: &FrameOptions) {
        let (builds, stamps, paths_on) = render(scene, camera, options, true);
        assert_eq!(builds, 0, "an ineligible scene must encode no child");
        assert_eq!(stamps, 0, "and stamp nothing");
        let (_, _, paths_off) = render(scene, camera, options, false);
        assert_eq!(
            paths_on, paths_off,
            "and encode exactly what the live path does"
        );
    }

    #[test]
    fn a_stretched_instance_stamps_via_its_own_linear_part() {
        // A uniform scale (or a uniform scale composed with a rotation or a
        // mirror) is a similarity and stamps via the quantised-scale path —
        // see `scaled_and_mirrored_instances_build_once`. An anisotropic
        // stretch is not a similarity — dividing out its quantised scale
        // leaves a linear part that is still not orthogonal — but S2
        // (`docs/BIMARI5_LAG_PLAN.md`) keys it on its own exact linear part
        // instead of refusing it: the stretched character and its
        // (identically stretched, since their own placement inside it is a
        // plain translation) `part` children each still build once rather
        // than being walked live every frame. See
        // `a_non_uniform_stretch_with_a_stroke_matches_the_live_pixels` for
        // the pixel proof.
        let (mut scene, _p, character) = crowd(3, 0);
        let layer = scene.add_layer("Stretched", LayerKind::Normal);
        scene.add_instance_at(layer, 0, character, Affine::scale_non_uniform(2.0, 1.3));
        // S2-rest (b): the first draw is a first sighting of the character
        // (drawn live; only its repeated `part` builds), the second builds it.
        let camera = wide_camera();
        let options = FrameOptions::default();
        let mut cache = DrawCache::new();
        let _sighting = render_with_cache(&scene, &camera, &options, &mut cache);
        let vs = render_with_cache(&scene, &camera, &options, &mut cache);
        let (builds, stamps) = (cache.symbol_scenes.builds, cache.symbol_scenes.stamps);
        let paths_on = vs.encoding().n_paths;
        assert_eq!(
            builds, 2,
            "one encode for the stretched character, one for its \
             (identically stretched) part"
        );
        assert!(stamps >= builds, "every build is also counted as a stamp");
        let (_, _, paths_off) = render(&scene, &wide_camera(), &FrameOptions::default(), false);
        assert_eq!(
            paths_on, paths_off,
            "stamping must encode exactly what the live path does"
        );
    }

    #[test]
    fn an_onion_ghost_falls_back() {
        let (scene, _p, _c) = crowd(3, 4);
        let ghosted = FrameOptions {
            ghost: Some(0.5),
            ..FrameOptions::default()
        };
        assert_ineligible(&scene, &wide_camera(), &ghosted);
    }

    #[test]
    fn a_symbol_with_a_filter_falls_back() {
        let mut scene = Scene::default();
        let part = scene.add_symbol("part", SymbolKind::Graphic, None);
        let pl = first_layer(&scene, part);
        scene.library_mut().update(part, |s| {
            s.layers.update(pl, |l| {
                let mut o = shape_object(1, Rect::new(0.0, 0.0, 10.0, 10.0));
                o.filters = vec![buzz_fx::Filter::new(buzz_fx::FilterKind::blur())];
                l.frames.push_object(0, Arc::new(o));
            });
        });
        let character = scene.add_symbol("character", SymbolKind::Graphic, None);
        let cl = first_layer(&scene, character);
        scene.library_mut().update(character, |s| {
            s.layers.update(cl, |l| {
                l.frames
                    .push_object(0, Arc::new(Object::instance_of(ObjectId(9), part)));
            });
        });
        let layer = scene.add_layer("Cast", LayerKind::Normal);
        for i in 0..4 {
            scene.add_instance_at(
                layer,
                0,
                character,
                Affine::translate((i as f64 * 30.0, 0.0)),
            );
        }
        assert_ineligible(&scene, &wide_camera(), &FrameOptions::default());
    }

    #[test]
    fn symbol_tween_and_parenting_cast_shadows_at_interpolated_positions() {
        let mut scene = Scene::default();
        let sym_id = scene.add_symbol("limb", SymbolKind::Graphic, None);
        let sym_layer = first_layer(&scene, sym_id);

        scene.library_mut().update(sym_id, |s| {
            s.layers.update(sym_layer, |l| {
                l.frames.insert_keyframe(0);
                l.frames.push_object(
                    0,
                    Arc::new(shape_object(1, Rect::new(0.0, 0.0, 20.0, 20.0))),
                );
                l.frames.insert_keyframe(10);
                l.frames.push_object(
                    10,
                    Arc::new(shape_object(1, Rect::new(100.0, 0.0, 120.0, 20.0))),
                );
                l.frames.set_tween(0, buzz_scene::Tween::classic());
            });
        });

        let main_layer = scene.add_layer("Character", LayerKind::Normal);
        scene.add_instance_at(main_layer, 0, sym_id, Affine::IDENTITY);
        scene.update_layer(main_layer, |l| {
            l.frames.insert_frame(10);
        });

        scene.add_light(buzz_scene::LightKind::Sun {
            azimuth: 0.5,
            elevation: 0.5,
        });

        let mut vello = crate::vello::Scene::new();
        let camera = Camera::new(Point::new(50.0, 50.0), 1.0, Size::new(400.0, 400.0));
        let mut builder = SceneBuilder::new(&mut vello, &camera);
        let mut cache = DrawCache::new();
        cache.begin();
        draw_frame_within(
            &mut builder,
            &scene,
            5,
            Affine::IDENTITY,
            &FrameOptions::default(),
            &mut cache,
        );

        assert!(
            vello.encoding().n_paths > 0,
            "must encode artwork and shadow paths for symbol tween"
        );
    }
}

#[cfg(test)]
mod layer_scene {
    use super::*;
    use buzz_geom::{Camera, Point, Rect, Shape, Size};
    use buzz_scene::ShapeData;

    fn scene_with_layer(shapes: usize) -> Scene {
        let mut scene = Scene::default();
        let layer = scene.add_layer("Background", LayerKind::Normal);
        for i in 0..shapes {
            let x = (i % 10) as f64 * 20.0;
            let y = (i / 10) as f64 * 20.0;
            let rect = Rect::new(x, y, x + 15.0, y + 15.0);
            scene.add_shape(layer, ShapeData::filled(rect.to_path(1e-9), Color::WHITE));
        }
        scene
    }

    fn wide_camera() -> Camera {
        Camera::new(Point::new(100.0, 100.0), 1.0, Size::new(800.0, 800.0))
    }

    fn options_with_cull(cull: Rect) -> FrameOptions {
        FrameOptions {
            cull: Some(cull),
            ..FrameOptions::default()
        }
    }

    #[test]
    fn layer_stamp_matches_the_live_split_formula() {
        // For every plain-affine camera this is eligible to cache under —
        // translation, rotation, scale, or a combination, not only a
        // similarity: `layer_stamp` conjugates *any* affine `camera` through
        // the fixed split `M`, unlike `symbol_stamp`, which bakes an
        // instance's own scale into the child's geometry and therefore does
        // need a similarity. See `try_stamp_layer`'s doc.
        let cases = [
            Affine::translate((37.0, -12.0)),
            Affine::rotate(0.7),
            Affine::translate((100.0, 200.0)) * Affine::rotate(-1.2),
            Affine::scale(2.5) * Affine::translate((5.0, -9.0)),
            Affine::scale_non_uniform(1.4, 0.6) * Affine::rotate(0.3),
        ];
        let gpu_view = Affine::translate((400.0, 300.0)) * Affine::rotate(0.2);
        let anchor = Point::new(12.0, -8.0);
        let scale = 1.7;

        for camera in cases {
            // The fragment's own encoded points are `M · p` — see
            // `try_stamp_layer`'s doc — built with the same split `layer_stamp`
            // is handed.
            let m = Affine::scale(scale) * Affine::translate(-anchor.to_vec2());
            let m_inv = Affine::translate(anchor.to_vec2()) * Affine::scale(1.0 / scale);
            let stamp = gpu_view * m * camera * m_inv;
            // The live render space of `camera · p` is `gpu_view · S ·
            // (camera · p − anchor)` — the same chain `to_render_space`
            // applies to ordinary geometry, with the camera folded into the
            // point beforehand exactly as `draw_layer`'s own projection does.
            let live_split =
                gpu_view * Affine::scale(scale) * Affine::translate(-anchor.to_vec2());
            for p in [
                Point::new(0.0, 0.0),
                Point::new(6.0, -3.0),
                Point::new(-10.0, 15.0),
            ] {
                let stamped = stamp * (m * p);
                let live = live_split * (camera * p);
                assert!(
                    (stamped - live).hypot() < 1e-7,
                    "layer_stamp diverged from the live split for camera {camera:?}: \
                     {stamped:?} vs {live:?}"
                );
            }
        }
    }

    #[test]
    fn a_static_layer_builds_once_across_a_moving_camera() {
        // The document camera moves every frame (P2's own motivation: 34-40
        // camera keys on the real files) while the cull rect the window
        // passes stays the same visible window — exactly what panning the
        // *story* camera looks like from `draw_layer`'s side. A layer whose
        // own content never changes must build once and stamp every frame
        // after.
        let scene = scene_with_layer(20);
        let camera = wide_camera();
        let options = options_with_cull(Rect::new(-200.0, -200.0, 1000.0, 1000.0));

        let mut cache = DrawCache::new();
        cache.set_layer_reuse(true);
        for story_camera in [
            Affine::IDENTITY,
            Affine::translate((3.0, 1.0)),
            Affine::translate((6.0, 2.0)),
            Affine::translate((9.0, 3.0)),
        ] {
            let mut vello = crate::vello::Scene::new();
            let mut builder = SceneBuilder::new(&mut vello, &camera);
            cache.begin();
            draw_frame_within(&mut builder, &scene, 0, story_camera, &options, &mut cache);
            cache.end();
            assert!(
                vello.encoding().n_paths > 0,
                "must still draw the layer's artwork"
            );
        }

        let (builds, stamps) = cache.layer_cache_stats();
        assert_eq!(
            builds, 1,
            "one encode for the whole static layer, not one per frame"
        );
        assert_eq!(stamps, 4, "every frame stamps the cached fragment");
    }

    #[test]
    fn disabling_the_layer_cache_draws_every_frame_live() {
        let scene = scene_with_layer(20);
        let camera = wide_camera();
        let options = options_with_cull(Rect::new(-200.0, -200.0, 1000.0, 1000.0));

        let mut cache = DrawCache::new();
        cache.set_layer_reuse(false);
        for _ in 0..3 {
            let mut vello = crate::vello::Scene::new();
            let mut builder = SceneBuilder::new(&mut vello, &camera);
            cache.begin();
            draw_frame_within(
                &mut builder,
                &scene,
                0,
                Affine::IDENTITY,
                &options,
                &mut cache,
            );
            cache.end();
        }

        let (builds, stamps) = cache.layer_cache_stats();
        assert_eq!(builds, 0, "the cache must never engage once turned off");
        assert_eq!(stamps, 0);
    }

    #[test]
    fn cached_and_live_draws_encode_the_same_geometry_as_the_cull_rect_moves() {
        // Sweeps the cull rect from squarely on the artwork to well past it,
        // crossing several 64-px grid cells and forcing misses and rebuilds
        // along the way.
        //
        // The cached path never draws *fewer* segments than the live one:
        // `try_stamp_layer` builds a miss culled to a rect 25% larger than
        // the exact one, so a hit can legitimately show a little geometry
        // just outside the exact cull a live draw would have dropped — which
        // is safe (`FrameOptions::cull`'s own doc: "culling too little only
        // costs time, culling too much is a wrong picture") but does mean
        // the two are not always segment-for-segment equal. Far enough from
        // every shape that even the grown rect misses, both must agree at
        // zero — that is the case an over-wide cache entry would get wrong.
        let scene = scene_with_layer(12);
        let camera = wide_camera();

        let mut with_cache = DrawCache::new();
        with_cache.set_layer_reuse(true);
        let mut without_cache = DrawCache::new();
        without_cache.set_layer_reuse(false);

        for cull_x in [0.0, 5.0, 90.0, 400.0, 4000.0] {
            let options =
                options_with_cull(Rect::new(cull_x - 200.0, -200.0, cull_x + 200.0, 200.0));

            let mut vello_a = crate::vello::Scene::new();
            let mut builder_a = SceneBuilder::new(&mut vello_a, &camera);
            with_cache.begin();
            draw_frame_within(
                &mut builder_a,
                &scene,
                0,
                Affine::IDENTITY,
                &options,
                &mut with_cache,
            );
            with_cache.end();

            let mut vello_b = crate::vello::Scene::new();
            let mut builder_b = SceneBuilder::new(&mut vello_b, &camera);
            without_cache.begin();
            draw_frame_within(
                &mut builder_b,
                &scene,
                0,
                Affine::IDENTITY,
                &options,
                &mut without_cache,
            );
            without_cache.end();

            let cached = vello_a.encoding().n_path_segments;
            let live = vello_b.encoding().n_path_segments;
            assert!(
                cached >= live,
                "cached draw must never encode less than the live one at cull_x={cull_x}: \
                 {cached} vs {live}"
            );
            if cull_x >= 4000.0 {
                assert_eq!(
                    cached, 0,
                    "far enough away that even the grown cull rect misses every shape, \
                     the cached path must draw nothing, not stale geometry"
                );
                assert_eq!(live, 0);
            }
        }
    }
}
