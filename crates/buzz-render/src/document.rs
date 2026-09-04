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

use crate::SceneBuilder;
use crate::filters::FilterCache;
use crate::lighting::LightCache;
use std::sync::Arc;

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
    /// The visible rectangle in **document space**, for culling.
    ///
    /// **Display-only.** The window passes the viewport rect so artwork far off
    /// the edge of a huge document is skipped rather than transformed, brushed
    /// and encoded into a scene that clips it away anyway. The exporter and the
    /// thumbnail renderer pass `None`: they render the whole stage (or a whole
    /// symbol), so their output is byte-for-byte what it was before culling
    /// existed — the parity guarantee is that this field is the *only* thing
    /// that differs, and they never set it. The cull is applied conservatively;
    /// see [`DrawCtx::cull`].
    pub cull: Option<buzz_geom::Rect>,
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
            place: Affine::IDENTITY,
            subframe: 0.0,
            cull: None,
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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LightDetail {
    /// Everything: tint, pool, cast shadows, terminator and highlight.
    #[default]
    Full,
    /// No terminator and no highlight. Tint, pool and cast shadows stand.
    NoModelling,
    /// Tint and pool only.
    Flat,
}

impl LightDetail {
    /// May a shape draw its terminator and highlight?
    fn models(self) -> bool {
        self == Self::Full
    }

    /// May a shape cast a shadow?
    fn casts(self) -> bool {
        self != Self::Flat
    }

    fn down(self) -> Self {
        match self {
            Self::Full => Self::NoModelling,
            Self::NoModelling | Self::Flat => Self::Flat,
        }
    }

    fn up(self) -> Self {
        match self {
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
    fn room_to_step_up(self, ceiling: u32) -> u32 {
        match self {
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
    (CEILING_CONSTANT / linear.sqrt()) as u32
}

/// What an unmeasured output is taken to be: an ordinary full-HD window. Also
/// the middle of the range [`segment_ceiling`] will answer for at all, since a
/// degenerate one-pixel viewport should not be granted an unbounded frame.
const REFERENCE_PIXELS: f64 = 1920.0 * 1080.0;

/// The fitted constant of [`segment_ceiling`]. See there for the measurements.
const CEILING_CONSTANT: f64 = 33_000_000.0;

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
    /// second pass at the latest for any given frame, because a level is only
    /// ever stepped one at a time and there are three of them.
    ///
    /// Judging *after* the encode rather than guessing before it is the whole
    /// point: the number is Vello's own count of what it will rasterise, so it
    /// cannot disagree with what was drawn. The cost is one wasted encode on
    /// the frame where the verdict changes, and the verdict is remembered, so
    /// every frame after it builds once.
    ///
    /// A frame still over the ceiling at [`LightDetail::Flat`] is a frame whose
    /// *artwork* will not fit, which no amount of trimming the lights can
    /// mend; the level stops there rather than pretending otherwise.
    pub fn reconsider(&mut self, segments: u32, output_pixels: f64) -> bool {
        self.encode = segments;
        self.ceiling = segment_ceiling(output_pixels);
        if self.pinned.is_some() {
            return false;
        }
        let was = self.detail;
        if segments > self.ceiling {
            self.detail = self.detail.down();
        } else if segments < self.detail.room_to_step_up(self.ceiling) {
            self.detail = self.detail.up();
        }
        if self.detail == was {
            return false;
        }
        // The pass just judged is being thrown away, so the shading it recorded
        // as owed is owed for shapes the replacement will not shade. See
        // `LightCache::discard_frame`.
        self.lights.discard_frame();
        true
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
    entries: std::collections::HashMap<usize, (Arc<Object>, buzz_geom::Rect, u64)>,
    frame: u64,
}

impl BoundsCache {
    fn begin(&mut self) {
        self.frame = self.frame.wrapping_add(1);
    }

    fn end(&mut self) {
        let frame = self.frame;
        self.entries
            .retain(|_, (_, _, used)| frame.saturating_sub(*used) < 3);
    }

    /// The resolved parent-space bounds of `owner`, computed once and reused.
    fn resolved(&mut self, owner: &Arc<Object>, scene: &Scene) -> buzz_geom::Rect {
        let key = Arc::as_ptr(owner) as usize;
        let frame = self.frame;
        if let Some(entry) = self.entries.get_mut(&key) {
            entry.2 = frame;
            return entry.1;
        }
        let bounds = scene.resolved_bounds(owner);
        self.entries.insert(key, (Arc::clone(owner), bounds, frame));
        bounds
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
    };

    fn or(self, other: SymFlags) -> SymFlags {
        SymFlags {
            filters: self.filters || other.filters,
            group_blend: self.group_blend || other.group_blend,
            non_flat: self.non_flat || other.non_flat,
            inverse_mask: self.inverse_mask || other.inverse_mask,
            additive: self.additive || other.additive,
        }
    }
}

/// A per-symbol memo of [`SymbolInfo`], rebuilt only when the document changes.
///
/// The interactive view camera is not part of the scene, so panning, zooming and
/// playback never bump [`Scene::revision`] — the table is built once on import
/// and reused for every frame the user then works through. It is rebuilt in full
/// on any edit, which is a single depth-first pass over the library.
#[derive(Debug, Default)]
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
/// The bounds mirror `Scene::resolved_bounds_within` exactly, but read a nested
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
            object.bounds()
        }
        // A posed rig, like a warp, measures itself — its bounds do not come
        // from unioning its parts (which are the undeformed source).
        ObjectKind::Warp(warp) => {
            flags.additive |= warp.shape.blend.is_additive();
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
    /// Generation, bumped per window frame like the lighting cache.
    frame: u64,
    /// How many child scenes were encoded this session, for tests and the HUD.
    pub builds: u64,
    /// How many times a cached scene was stamped, likewise.
    pub stamps: u64,
}

impl Default for SymbolSceneCache {
    fn default() -> Self {
        Self {
            entries: std::collections::HashMap::new(),
            enabled: true,
            frame: 0,
            builds: 0,
            stamps: 0,
        }
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

/// What makes two instances share one cached encoding.
///
/// `depth` is in the key because symbol nesting is truncated at
/// [`MAX_SYMBOL_DEPTH`], so the same symbol drawn deeper can render less;
/// `scale_bits` is in it because the encoding is built at one zoom (the
/// flattening tolerance and baked stroke widths are zoom-specific). The camera
/// anchor is deliberately *not* in the key — the encoding is anchor-independent,
/// which is what makes panning free.
#[derive(PartialEq, Eq, Hash, Clone, Copy, Debug)]
struct SymKey {
    fingerprint: u64,
    inner: u32,
    depth: u8,
    scale_bits: u64,
}

struct SymEntry {
    scene: Arc<crate::vello::Scene>,
    /// Guards against a fingerprint hash collision: a hit must point at the same
    /// symbol it was built from.
    symbol: Arc<buzz_scene::Symbol>,
    /// The symbol centre the child was encoded about; the stamp is derived from
    /// this exact value, never a recomputed one.
    anchor: buzz_geom::Point,
    used: u64,
}

/// How many generations an unused entry survives, matching the other caches.
const SYM_KEEP_FRAMES: u64 = 3;
/// Ceiling on live entries. A zoom gesture rekeys everything each frame, so
/// without a cap three generations of full encodings would accumulate.
const SYM_CACHE_CAP: usize = 256;

impl SymbolSceneCache {
    fn begin(&mut self) {
        self.frame = self.frame.wrapping_add(1);
    }

    fn end(&mut self) {
        let frame = self.frame;
        self.entries
            .retain(|_, e| frame.saturating_sub(e.used) < SYM_KEEP_FRAMES);
        // If still over budget, drop the least-recently-used until at the cap.
        while self.entries.len() > SYM_CACHE_CAP {
            if let Some(oldest) = self
                .entries
                .iter()
                .min_by_key(|(_, e)| e.used)
                .map(|(k, _)| *k)
            {
                self.entries.remove(&oldest);
            } else {
                break;
            }
        }
    }
}

/// Whether a 2×2 linear part (kurbo coeffs `[a,b,c,d]`) is orthogonal — a pure
/// rotation, reflection or the identity, with no scale or shear. Only then do
/// baked stroke and seam-seal widths, which scale by the view zoom alone, stay
/// screen-correct after the instance's transform is folded in.
fn is_orthogonal(coeffs: [f64; 6], eps: f64) -> bool {
    let [a, b, c, d, _, _] = coeffs;
    (a * a + b * b - 1.0).abs() < eps
        && (c * c + d * d - 1.0).abs() < eps
        && (a * c + b * d).abs() < eps
}

/// The transform that carries a child scene, encoded as `S·(p − anchor)` about
/// the symbol centre, into the stage's render space.
///
/// Derivation: the stage needs `gpu_view ∘ S·(A·p − cam)` where `A` is the
/// instance's placement (projection ∘ document transform). Since the uniform
/// scale `S` commutes with `A`'s linear part, that is
/// `gpu_view ∘ [A_lin, S·(A·anchor − cam)] ∘ (S·(p − anchor))` — the bracket is
/// this stamp. Everything but the final compose is `f64`.
fn symbol_stamp(
    gpu_view: Affine,
    a: Affine,
    anchor: buzz_geom::Point,
    scale: f64,
    cam: buzz_geom::Point,
) -> Affine {
    let t = ((a * anchor) - cam) * scale;
    let c = a.as_coeffs();
    gpu_view * Affine::new([c[0], c[1], c[2], c[3], t.x, t.y])
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
        self.symbol_scenes.begin();
    }

    pub fn end(&mut self) {
        self.lights.end();
        self.filters.end();
        self.bounds.end();
        self.symbol_scenes.end();
    }

    /// Turn the symbol-encoding cache on or off. Off, every instance is drawn
    /// live; the result is identical, only slower on instance-heavy documents.
    pub fn set_symbol_reuse(&mut self, on: bool) {
        self.symbol_scenes.enabled = on;
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
        symbols: SymbolTable::default(),
        symbol_scenes: SymbolSceneCache::default(),
        lamp: None,
        detail: LightDetail::default(),
        encode: 0,
        ceiling: 0,
        pinned: None,
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

    // Depth ordering is opt-in on the stage. The masked run stays contiguous
    // either way, so the mask machinery below is untouched; only the order the
    // layers arrive in changes. With every depth equal, the two orders are
    // identical — see `LayerStack::depth_paint_order`.
    let ordered: Vec<&Arc<buzz_scene::Layer>> = if scene.stage().sort_by_depth {
        layers.drawable_at_by_depth(frame)
    } else {
        layers.drawable_at(frame).collect()
    };
    for layer in ordered {
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

        // Masked layers arrive in one unbroken run per mask, so the clip is
        // opened once for the run rather than once per layer.
        let wanted = masks.get(&layer.id).copied();
        if wanted != open.as_ref().map(|o| o.mask) {
            close_mask(builder, open.take());
            if let Some(mask_id) = wanted
                && let Some(path) = mask_geometry(
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
        draw_layer(
            builder, scene, layer, frame, camera, follows, options, &lights, cache,
        );
    }

    close_mask(builder, open);

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
        builder.push_clip(&path);
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
    Some(OpenMask {
        mask,
        punch: Some(path),
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
fn mask_geometry(
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

    for object in layer.frames.resolved_at(at).iter() {
        let mut flat = Vec::new();
        object.flatten(place, &mut flat);
        for (transform, shape) in flat {
            if shape.fill.is_none() {
                continue;
            }
            for element in (transform * shape.path).elements() {
                combined.push(*element);
            }
        }
    }

    if combined.elements().is_empty() {
        return None;
    }
    let mapped = projection.map_path(&combined, tolerance);
    (!mapped.elements().is_empty()).then_some(mapped)
}

/// Draw one layer's artwork.
#[allow(
    clippy::too_many_arguments,
    reason = "one call site; a struct would only move the arguments"
)]
fn draw_layer(
    builder: &mut SceneBuilder<'_>,
    scene: &Scene,
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
        let lit = options.lit && rig.is_active() && tint.is_none();
        let stage_height = scene.stage().size.height;
        // Whether this layer is lit at all is decided here; *how much* light
        // reaches each shape is worked out per shape, because a lamp's whole
        // character is that it varies across the stage. Deciding it once per
        // layer made two squares either side of a lamp exactly as bright as
        // each other, which is a sun pretending to be a lamp.
        let lighting = lit.then_some((stage_height, layer.depth));
        // Where this stack sits — identity unless a symbol is open in place —
        // and then layer parenting, both in the plane, before the lens.
        let projection = projection.pre_affine(options.place).pre_affine(follows);

        // How this layer's shadows are thrown, if they are. Worked out once
        // here: it depends on the light and on how far this layer's artwork
        // stands above the surface catching it, and on nothing about any shape.
        // The cull below needs it and so does the shadow pass, and they must
        // agree — a caster culled away casts nothing.
        // Trimmed away at `LightDetail::Flat`, where nothing generated fits;
        // `None` here removes the shadow pass and the margin the cull below
        // grows for it in one go, exactly as an unlit layer does.
        let shadow = (lit && cache.detail().casts())
            .then(|| rig.key())
            .flatten()
            .and_then(|key| buzz_light::shadow_transform(key, scene.shadow_height(layer.depth, key)));

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
                match shadow {
                    Some(shadow) => rect.union(shadow.inverse().transform_rect_bbox(rect)),
                    None => rect,
                }
            });

        let ctx = DrawCtx {
            scene,
            cull,
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
            stage_size: scene.stage().size,
            depth: 0,
            lighting,
            layer_depth: layer.depth,
            projection,
        };
        let resolved = layer.frames.resolved_at(time);

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
        if let Some(shadow) = shadow
            && let Some(key) = rig.key()
            && let Some(cast) = shadow_group_bounds(&resolved, shadow, &ctx)
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
            builder.push_alpha_group(cast, alpha);
            for object in resolved.iter() {
                cast_shadows(builder, object, Affine::IDENTITY, key, shadow, &ctx);
            }
            builder.pop_isolation();
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
            // Bounded to the layer's own artwork: an unbounded group would
            // cost a full-viewport buffer whatever the layer contains.
            //
            // Through the camera, because that is where the artwork actually
            // lands — a layer moved by depth or by a camera pan would
            // otherwise be clipped against where its geometry used to be.
            let bounds = layer
                .bounds_at(frame)
                .and_then(|b| ctx.projection.map_rect_bounds(b))
                .map(|b| b.inflate(2.0, 2.0))
                .unwrap_or_else(|| builder.clip_bounds());
            builder.push_isolation(bounds.intersect(builder.clip_bounds()));
        }

        // **The rim the key light lays around this layer's artwork**, if it
        // lays one. See `buzz_light::rim_glow`: a glow outside the silhouette,
        // in the light's colour and at the light's strength, so the edges come
        // up when the light does.
        //
        // Per *layer* rather than per shape, and from one union silhouette: a
        // character is a hundred shapes and glowing each of them would rim
        // every internal seam \u2014 the arm's edge against the body it is drawn
        // over \u2014 which reads as a drawing coming apart rather than as light.
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
        // laid by a light instead of by hand \u2014 same geometry, same bands, same
        // drawing code. Building it as a second mechanism would be two things
        // to keep looking alike.
        let needs_silhouette = !layer.filters.is_empty() || rim.is_some();
        let layer_fx = needs_silhouette.then(|| {
            let mut silhouette = buzz_geom::BezPath::new();
            for object in resolved.iter() {
                append_silhouette(object, Affine::IDENTITY, &mut silhouette);
            }
            let mut filters: Vec<buzz_fx::Filter> = Vec::with_capacity(layer.filters.len() + 1);
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
            buzz_fx::build(&filters, &silhouette)
        });

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
                ..ctx.clone()
            },
            _ => ctx.clone(),
        };

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
            for (object, owner) in resolved.iter_owned() {
                let mut object_ctx = layer_ctx.clone();
                // A layer blur applies to every shape on the layer, and depth of
                // field adds to it — the wider of the two wins, so a blurred
                // background layer thrown out of focus is not blurred twice.
                object_ctx.blur = combine_blur(layer_fx.as_ref().and_then(|fx| fx.blur), dof_blur);

                // Live modifiers (a spring, a wiggle) are evaluated here — the one
                // place the window, the exporter and the headless tests all pass
                // through, so what is drawn is what is exported. Almost every
                // object has none and takes the cheap `None` path unchanged.
                match scene.modified_object_at(layer.id, object, time) {
                    None => draw_object(builder, object, owner, Affine::IDENTITY, &object_ctx, cache),
                    Some(eval) => {
                        // A spring re-poses the rig into an owned copy (no `Arc`
                        // identity); a wiggle only prepends a transform and keeps
                        // the original, so its symbol/bounds caches still hit.
                        let drawn = eval.object.as_ref().unwrap_or(object);
                        let owner = if eval.object.is_some() { None } else { owner };
                        draw_object(builder, drawn, owner, eval.prepend, &object_ctx, cache);
                    }
                }
            }
        }

        if let Some(fx) = &layer_fx {
            crate::filters::draw_ops(builder, &fx.over, &ctx.projection, ctx.ghost.unwrap_or(1.0));
        }

        if accumulates {
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
    let modelling = ctx.lights.modelling;
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
        .crescents(owner, index, placed, doc, -facing, light.softness);
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
    append_silhouette(object, doc * object.transform, &mut silhouette);
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
        let bounds = ctx
            .projection
            .map_rect_bounds(buzz_scene::object::transform_rect(doc, object.bounds()))
            .map(|b| b.inflate(reach + 2.0, reach + 2.0))
            .unwrap_or_else(|| builder.clip_bounds());
        builder.push_blend(bounds.intersect(builder.clip_bounds()), object.blend);
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
                // Posed rig artwork is rebuilt from the bones every frame, so
                // like a tween it has no identity worth caching against.
                draw_object(builder, &part, None, doc, ctx, cache);
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

            // A graphic follows the parent playhead — from where it was
            // placed, not from the start of the film; a movie clip shows its
            // first frame while authoring.
            let inner = instance.resolve_frame(symbol.kind, ctx.elapsed, symbol.length());

            let mut inner_ctx = ctx.clone();
            inner_ctx.frame = inner;
            inner_ctx.depth += 1;
            // Compose rather than replace: a tinted symbol inside a faded one
            // must show both effects.
            inner_ctx.effect = instance.color.compose(&ctx.effect);

            // Reuse a cached encoding of the whole symbol when it is safe to; the
            // symbol is not cloned here, so `symbol` is still borrowed from the
            // scene, which is why the id and the reference both go in.
            if try_stamp_symbol(
                builder,
                ctx,
                &inner_ctx,
                instance.symbol,
                symbol,
                inner,
                doc,
                cache,
            ) {
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

    for layer in symbol.layers.drawable_at(inner) {
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
            tint: inner_ctx.tint.or_else(|| layer.outline.then_some(layer.color)),
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
        for (child, owner) in layer.frames.resolved_at(inner).iter_owned() {
            draw_object(builder, child, owner, doc * follows, &layer_ctx, cache);
        }
    }

    close_mask(builder, open);
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
        return false;
    }

    // The symbol's whole subtree must render position-independently, and the memo
    // must know it. Copy the few facts out so the memo's borrow ends here, before
    // the cache is borrowed mutably below.
    let (fingerprint, resolved_bounds) = {
        let Some(info) = cache.symbols.get(symbol_id) else {
            return false;
        };
        if !info.cacheable_content() {
            return false;
        }
        (info.fingerprint, info.resolved_bounds)
    };
    // The identity check on a hit compares against the *live* symbol, not the
    // memo's copy, so an entry left over from another document is rejected.
    let symbol_arc = Arc::clone(symbol);

    // Colour, overlay and lighting must all be neutral; otherwise the cached
    // artwork — encoded once, shared by every instance — would be wrong here.
    if ctx.tint.is_some()
        || ctx.faded
        || ctx.ghost.is_some()
        || ctx.adjust.is_some()
        || ctx.gradient_map.is_some()
        || ctx.blur.is_some()
        || ctx.lighting.is_some()
        || !inner_ctx.effect.is_identity()
    {
        return false;
    }

    // The placement must be an orthogonal affine, so baked stroke widths stay
    // screen-correct and the stamp is an exact conjugation of the render split.
    let Some(proj_affine) = ctx.projection.as_affine() else {
        return false;
    };
    let a = proj_affine * doc;
    if !is_orthogonal(a.as_coeffs(), 1e-6) {
        return false;
    }

    let scale = builder.split.scale;
    if !scale.is_finite() || scale <= 0.0 {
        return false;
    }
    // Keep the child's render-space coordinates, and the stamp's translation,
    // inside the well-conditioned range of f32.
    let diag = resolved_bounds.width().hypot(resolved_bounds.height());
    if scale * diag > 1e5 {
        return false;
    }
    let anchor = resolved_bounds.center();
    let cam = builder.split.anchor;
    if (((a * anchor) - cam) * scale).hypot() > 1e7 {
        return false;
    }

    let key = SymKey {
        fingerprint,
        inner,
        depth: inner_ctx.depth.min(u8::MAX as usize) as u8,
        scale_bits: scale.to_bits(),
    };
    let generation = cache.symbol_scenes.frame;

    // A hit: stamp the cached encoding. The pointer check rejects the rare case
    // of two different symbols colliding on the key.
    let hit = match cache.symbol_scenes.entries.get_mut(&key) {
        Some(entry) if Arc::ptr_eq(&entry.symbol, &symbol_arc) => {
            entry.used = generation;
            Some((Arc::clone(&entry.scene), entry.anchor))
        }
        _ => None,
    };
    if let Some((scene, anchor)) = hit {
        cache.symbol_scenes.stamps += 1;
        stamp_scene(builder, &scene, a, anchor);
        return true;
    }

    // A miss: encode the whole symbol once, about its own centre, at this zoom,
    // through the *same* `draw_symbol_contents` the live path uses.
    let mut child_scene = crate::vello::Scene::new();
    {
        let split = RenderSplit {
            anchor,
            scale,
            gpu_view: Affine::IDENTITY,
        };
        let clip = RenderClip::new(resolved_bounds);
        let mut child = SceneBuilder {
            scene: &mut child_scene,
            split,
            clip,
        };
        let neutral = DrawCtx {
            cull: None,
            lighting: None,
            effect: ColorTransform::default(),
            tint: None,
            faded: false,
            ghost: None,
            adjust: None,
            gradient_map: None,
            blur: None,
            projection: Projection::from_affine(Affine::IDENTITY),
            ..inner_ctx.clone()
        };
        draw_symbol_contents(&mut child, symbol, inner, Affine::IDENTITY, &neutral, cache);
    }
    let scene = Arc::new(child_scene);
    {
        let sc = &mut cache.symbol_scenes;
        sc.builds += 1;
        sc.stamps += 1;
        sc.entries.insert(
            key,
            SymEntry {
                scene: Arc::clone(&scene),
                symbol: symbol_arc,
                anchor,
                used: generation,
            },
        );
    }
    stamp_scene(builder, &scene, a, anchor);
    true
}

/// Append a cached child scene into the stage at the instance's placement.
fn stamp_scene(
    builder: &mut SceneBuilder<'_>,
    child: &crate::vello::Scene,
    a: Affine,
    anchor: buzz_geom::Point,
) {
    let stamp = symbol_stamp(
        builder.split.gpu_view,
        a,
        anchor,
        builder.split.scale,
        builder.split.anchor,
    );
    // Only glyph runs set encoding flags, and nothing here draws text; a child
    // with an unbalanced clip stack would corrupt the parent's. Both hold today
    // and these pin them for whoever adds text rendering.
    debug_assert_eq!(builder.scene.encoding().flags, 0, "no glyph-run flags to carry");
    debug_assert_eq!(child.encoding().n_open_clips, 0, "child must be layer-balanced");
    builder.scene.append(child, Some(stamp));
}

/// Collect an object's filled outlines into one path, through `transform`.
///
/// Concatenated rather than unioned: filled non-zero, overlapping subpaths
/// merge visually, which is what a silhouette means — and a union would be a
/// boolean per object per frame for a result nobody would be able to tell
/// apart. Strokes are left out for the same reason a mask ignores them: a
/// silhouette is a region, and a stroke is a line.
fn append_silhouette(object: &Object, transform: Affine, out: &mut buzz_geom::BezPath) {
    let mut flat = Vec::new();
    object.flatten(transform, &mut flat);
    for (place, shape) in flat {
        if shape.fill.is_none() {
            continue;
        }
        for element in (place * shape.path).elements() {
            out.push(*element);
        }
    }
}

/// Paint one shape's fill and stroke, lit.
#[allow(
    clippy::too_many_arguments,
    reason = "one call path; a struct would only move the arguments"
)]
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
    let placed = doc * shape.path.clone();
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
                    || (f.disc().is_some()
                        && !matches!(fill.paint, buzz_scene::Paint::Solid(_)));
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
                    let outline = buzz_geom::outline_stroke(
                        &shape.path,
                        buzz_geom::StrokeStyle::new(stroke.width.max(f64::MIN_POSITIVE)),
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
            } else {
                // Sealed: artwork that shares a boundary with the shape beside it
                // must not leave a pale line along the join. See
                // `SceneBuilder::fill_shape_paint_sealed`.
                builder.fill_shape_paint_sealed(&path, &paint, brush_to_doc);
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
                && let Some(towards) =
                    buzz_light::crescent_direction(key, here, ctx.layer_depth, ctx.lights.modelling)
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
                let modelling = ctx.lights.modelling;
                let geometry = cache
                    .lights
                    .crescents(owner, index, &placed, doc, towards, key.softness);

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
                    match (&field, &fill.paint) {
                        (Some(f), buzz_scene::Paint::Solid(c))
                            if let Some((ramp, disc)) =
                                lamp_lit(f, *c, |i, c| i.apply_shaded(c)) =>
                        {
                            builder.fill_shape_paint(&drawn, &ramp, ctx.brush_projection(disc));
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
            builder.stroke_shape_paint(&path, &paint, stroke.width, brush_to_doc);
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
        } else if sealed {
            builder.fill_shape_paint_sealed(path, &paint, brush_to_doc);
        } else {
            builder.fill_shape_paint(path, &paint, brush_to_doc);
        }
        return;
    }

    // The crescents are worked out **before** anything is drawn, because the
    // group opened below has to be big enough to hold them: a crescent is the
    // artwork's own outline offset, so it can reach a little outside the shape.
    let modelling = ctx.lights.modelling;
    let key = ctx.lights.key();
    let geometry = key
        .and_then(|key| {
            let towards =
                buzz_light::crescent_direction(key, here, ctx.layer_depth, modelling)?;
            Some((key, towards))
        })
        .map(|(key, towards)| {
            (
                key,
                // Which way the light lies from here, kept so the crescents can
                // be feathered along it below.
                towards,
                cache
                    .lights
                    .crescents(owner, index, placed, doc, towards, key.softness),
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
    } else if sealed {
        // Artwork made of coloured regions shares a boundary with the artwork
        // beside it, and must not leave a pale line along the join.
        builder.fill_shape_paint_sealed(path, &paint, brush_to_doc);
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
    if let Some(drawn) = &shade {
        match &lamp {
            None => builder.fill_shape_atop(drawn, light.shade_filter(), buzz_fx::Blend::Multiply),
            Some(lamp) => builder.fill_shape_atop_paint(
                drawn,
                &lamp.shade,
                ctx.brush_projection(lamp.disc),
                buzz_fx::Blend::Multiply,
            ),
        }
    }
    if let (Some(drawn), Some((key, towards, built))) = (&highlight, &geometry) {
        let strength = buzz_light::Illumination::highlight_strength(modelling * key.glint());
        let glint = key.color.multiply_alpha(strength);
        // Feathered exactly as the vector path's is — see `glint_ramp`. Here
        // the fade is in the *alpha*, because a bitmap has no fill colour to
        // fade towards: the ramp goes from the glint to the same glint at
        // nothing, which is the picture underneath, untouched.
        let ramp = built.highlight.as_ref().and_then(|h| {
            let band = h.bounding_box();
            glint_ramp(glint, glint.multiply_alpha(0.0), band, *towards).map(|paint| (paint, band))
        });
        match ramp {
            Some((paint, band)) => builder.fill_shape_atop_paint(
                drawn,
                &paint,
                ctx.brush_projection(band),
                buzz_fx::Blend::Normal,
            ),
            None => builder.fill_shape_atop(drawn, glint, buzz_fx::Blend::Normal),
        }
    }

    if isolate {
        builder.pop_isolation();
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
            buzz_scene::GradientStop::new(GLINT_HOLD, outer),
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
    key: &buzz_light::Light,
    shadow: Affine,
    ctx: &DrawCtx<'_>,
) {
    cast_shadows_within(builder, object, doc, key, shadow, ctx, 0);
}

/// **Where a layer's shadows land**, as the group they are drawn inside.
///
/// The union of what the layer actually draws, thrown by the shadow affine, and
/// then through the lens. Every group is a render target, so it is trimmed to
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
    shadow: Affine,
    ctx: &DrawCtx<'_>,
) -> Option<buzz_geom::Rect> {
    let mut area: Option<buzz_geom::Rect> = None;
    for object in resolved.iter() {
        let b = ctx.scene.resolved_bounds(object);
        area = Some(match area {
            Some(a) => a.union(b),
            None => b,
        });
    }
    let mut area = shadow.transform_rect_bbox(area?);
    if let Some(cull) = ctx.cull {
        area = area.intersect(cull);
    }
    if !(area.width() > 0.0 && area.height() > 0.0) {
        return None;
    }
    ctx.projection.map_rect_bounds(area)
}

/// One shape's shadow: its outline, put where the light throws it.
///
/// The projection is folded into the placement rather than applied after it, so
/// the path is copied once. `shadow * doc` is the same thing as shadowing the
/// already-placed path — the shadow is taken in document space — for one path
/// transform instead of two.
fn draw_shadow(
    builder: &mut SceneBuilder<'_>,
    path: &buzz_geom::BezPath,
    doc: Affine,
    _key: &buzz_light::Light,
    shadow: Affine,
    ctx: &DrawCtx<'_>,
) {
    // **Opaque.** The tone is on the group this is drawn inside, so that shapes
    // overlapping within one caster make a silhouette rather than a darker
    // patch. See the shadow pass in `draw_layer`.
    let cast = (shadow * doc) * path.clone();
    let drawn = ctx.project(&cast, builder.tolerance());
    builder.fill_shape(&drawn, Color::BLACK);
}

/// [`cast_shadows`], carrying how deep into nested symbols it has gone.
///
/// `depth_limit` is the *nesting* count and is nothing to do with layer depth,
/// which is the layer's distance from the camera. A symbol containing an
/// instance of itself would otherwise recurse until the stack ran out.
fn cast_shadows_within(
    builder: &mut SceneBuilder<'_>,
    object: &Object,
    doc: Affine,
    key: &buzz_light::Light,
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
            if shape.fill.is_none() {
                // An unfilled outline has nothing to block the light with.
                return;
            }
            draw_shadow(builder, &shape.path, doc, key, shadow, ctx);
        }
        ObjectKind::Group(children) => {
            for child in children {
                cast_shadows_within(builder, child, doc, key, shadow, ctx, depth_limit);
            }
        }
        ObjectKind::Armature(rig) => {
            for part in rig.posed() {
                cast_shadows_within(builder, &part, doc, key, shadow, ctx, depth_limit);
            }
        }
        ObjectKind::Warp(warp) => {
            let warped = warp.warped();
            if warped.fill.is_none() {
                return;
            }
            draw_shadow(builder, &warped.path, doc, key, shadow, ctx);
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
            let inner = instance.resolve_frame(symbol.kind, ctx.elapsed, symbol.length());

            for layer in symbol.layers.drawable_at(inner) {
                // A mask layer is not artwork and casts nothing; the layers it
                // clips still cast their own, which is a simplification — the
                // shadow of a masked drawing should be the shadow of the part
                // that survives the mask. Recorded in PROGRESS.md §7.
                if layer.kind.is_mask() {
                    continue;
                }
                for child in layer.objects_at(inner) {
                    cast_shadows_within(builder, child, doc, key, shadow, ctx, depth_limit + 1);
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
            ShapeData::filled(Rect::new(10.0, 10.0, 40.0, 40.0).to_path(1e-9), Color::WHITE),
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
        draw_frame_within(&mut builder, scene, 0, Affine::IDENTITY, &options, &mut cache);
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
        assert!(rects_overlap(a, Rect::new(5.0, 5.0, 15.0, 15.0)), "overlapping");
        assert!(rects_overlap(a, Rect::new(10.0, 0.0, 20.0, 10.0)), "edge-touching kept");
        assert!(!rects_overlap(a, Rect::new(11.0, 0.0, 20.0, 10.0)), "clear of it");
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
        draw_frame_within(&mut builder, &scene, 0, Affine::IDENTITY, &options, &mut cache);

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
        draw_frame_within(&mut builder, &scene, 0, Affine::IDENTITY, &options, &mut cache);

        assert!(
            cache.lights.take_misses().is_empty(),
            "an inline draw queues nothing"
        );
        assert!(
            !cache.lights.is_empty(),
            "it should have built the geometry into the cache"
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
        Object::shape(ObjectId(id), ShapeData::filled(rect.to_path(1e-9), Color::WHITE))
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
            ObjectKind::Armature(rig) => {
                rig.parts.iter().any(|p| brute(&p.artwork, scene, depth + 1))
            }
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

        assert_ne!(fp_part, table.get(part).unwrap().fingerprint, "part changed");
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
            assert!(table.get(outer).unwrap().flags.filters, "filter bubbles up two levels");
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
        Object::shape(ObjectId(id), ShapeData::filled(rect.to_path(1e-9), Color::WHITE))
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
                l.frames
                    .push_object(0, Arc::new(shape_object(1, Rect::new(0.0, 0.0, 10.0, 10.0))));
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
        draw_frame_within(&mut builder, scene, 0, Affine::IDENTITY, options, &mut cache);
        cache.end();
        (
            cache.symbol_scenes.builds,
            cache.symbol_scenes.stamps,
            vello.encoding().n_paths,
        )
    }

    #[test]
    fn stamp_matches_the_live_split_formula() {
        // For a handful of orthogonal placements, the stamped child must land a
        // point exactly where the live render split would.
        let cases = [
            (Affine::translate((37.0, -12.0)), Point::new(5.0, 9.0), 0.4),
            (Affine::rotate(0.7), Point::new(-20.0, 3.0), 2.5),
            (
                Affine::translate((100.0, 200.0)) * Affine::rotate(-1.2),
                Point::new(50.0, -50.0),
                1.0,
            ),
            (Affine::FLIP_Y * Affine::rotate(0.3), Point::new(8.0, 8.0), 7.0),
        ];
        let gpu_view = Affine::translate((400.0, 300.0)) * Affine::rotate(0.2);
        let cam = Point::new(123.0, -45.0);

        for (a, anchor, scale) in cases {
            assert!(is_orthogonal(a.as_coeffs(), 1e-9), "test A must be orthogonal");
            let stamp = symbol_stamp(gpu_view, a, anchor, scale, cam);
            // The child encodes C(p) = S·(p − anchor).
            let child = Affine::scale(scale) * Affine::translate(-anchor.to_vec2());
            // The live render space of the document point A·p is S·(A·p − cam),
            // then the GPU view.
            let live_split = gpu_view * Affine::scale(scale) * Affine::translate(-cam.to_vec2());
            for p in [Point::new(0.0, 0.0), Point::new(3.0, -7.0), Point::new(-11.0, 4.0)] {
                let stamped = stamp * (child * p);
                let live = live_split * (a * p);
                assert!(
                    (stamped - live).hypot() < 1e-9,
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
        assert_eq!(builds, 2, "one encode for the character, one for the part — not 60k");
        // 50 character stamps, plus the 10 part stamps of the single character
        // build (the other 49 characters are whole-scene stamps that never walk
        // the parts again).
        assert_eq!(stamps, 60);
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
        let (scene, _p, _c) = crowd(3, 3);
        let camera = wide_camera();
        let options = FrameOptions::default();
        let mut cache = DrawCache::new();
        cache.set_symbol_reuse(true);

        let mut vello = crate::vello::Scene::new();
        let mut builder = SceneBuilder::new(&mut vello, &camera);
        cache.begin();
        draw_frame_within(&mut builder, &scene, 0, Affine::IDENTITY, &options, &mut cache);
        cache.end();
        assert!(!cache.symbol_scenes.entries.is_empty(), "the draw cached something");

        // Generations pass with nothing drawn; entries age out.
        for _ in 0..SYM_KEEP_FRAMES {
            cache.begin();
            cache.end();
        }
        assert!(
            cache.symbol_scenes.entries.is_empty(),
            "entries not drawn for a few generations are dropped"
        );
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
                },
                SymEntry {
                    scene: Arc::clone(&dummy),
                    symbol: Arc::clone(&sym),
                    anchor: Point::ORIGIN,
                    used: sc.frame,
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
        assert_eq!(paths_on, paths_off, "and encode exactly what the live path does");
    }

    #[test]
    fn a_scaled_instance_falls_back() {
        // A non-orthogonal placement (uniform 2x) is out of scope for v1.
        let (mut scene, _p, character) = crowd(3, 0);
        let layer = scene.add_layer("Scaled", LayerKind::Normal);
        scene.add_instance_at(layer, 0, character, Affine::scale(2.0));
        assert_ineligible(&scene, &wide_camera(), &FrameOptions::default());
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
            scene.add_instance_at(layer, 0, character, Affine::translate((i as f64 * 30.0, 0.0)));
        }
        assert_ineligible(&scene, &wide_camera(), &FrameOptions::default());
    }
}
