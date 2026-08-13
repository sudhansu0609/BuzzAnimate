//! The BuzzAnimate document model.
//!
//! # Copy-on-write, and why the whole architecture rests on it
//!
//! A [`Scene`] is an immutable snapshot built from `Arc`s. Cloning one copies
//! pointers, not artwork, and editing a clone touches only the objects that
//! actually changed — [`Arc::make_mut`] clones a node just when another
//! snapshot still shares it.
//!
//! Three properties fall out of that, and they are the reason for the design:
//!
//! 1. **The renderer never locks.** It holds a snapshot and reads it freely
//!    while the document thread builds the next one. No mutex sits between
//!    editing and drawing, so an edit cannot stall a frame and a frame cannot
//!    stall an edit.
//! 2. **Undo is nearly free.** Old snapshots *are* the history. Undo is
//!    swapping back to a previous `Scene`, not replaying inverse operations —
//!    which is where undo systems usually accumulate bugs.
//! 3. **Background work is safe by construction.** Autosave, thumbnails and
//!    index rebuilds take a snapshot and work from it with no coordination.
//!
//! # Revisions
//!
//! Every edit bumps [`Scene::revision`]. Derived data such as
//! [`SpatialIndex`] records the revision it was built from, so a consumer can
//! tell whether what it holds is current instead of trusting it blindly.

pub mod camera_track;
pub mod index;
pub mod layer;
pub mod merge;
pub mod object;
pub mod rig;
pub mod sound;
pub mod symbol;
pub mod timeline;
pub mod tween;

use std::sync::Arc;

use buzz_geom::{Affine, Point, Rect, Size};
use peniko::Color;
use serde::{Deserialize, Serialize};

pub use camera_track::{CameraKey, CameraTrack, MAX_TILT};
pub use index::{IndexEntry, SpatialIndex};
pub use layer::{Layer, LayerHeight, LayerId, LayerKind, LayerStack, MaskGroup};
pub use merge::{ImportTarget, MergeReport};
pub use object::{FillSpec, Object, ObjectId, ObjectKind, PaintBlend, ShapeData, StrokeSpec};
pub use rig::{ArmatureData, RigBinding, RigPart, WarpData};
pub use sound::{SoundAsset, SoundCue, SoundId, SoundLibrary, SoundRef, SoundSync};
pub use buzz_fx::{BevelKind, Blend, ColorAdjust, Filter, FilterKind, Quality};
pub use buzz_light::{Light, LightId, LightKind, LightRig};
pub use symbol::{
    ColorEffect, ColorTransform, Library, LoopMode, Symbol, SymbolId, SymbolInstance, SymbolKind,
};
pub use timeline::{FrameKind, Keyframe, LayerTimeline, ResolvedFrame, TweenSpan};
pub use tween::{Easing, Tween, TweenKind};

/// Stage setup, matching Animate's Document Properties dialog.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct StageProperties {
    /// Stage size in document units. Animate's default is 550×400.
    pub size: Size,
    pub background: Color,
    pub frame_rate: f64,
}

impl Default for StageProperties {
    fn default() -> Self {
        Self {
            size: Size::new(550.0, 400.0),
            background: Color::WHITE,
            frame_rate: 24.0,
        }
    }
}

impl StageProperties {
    /// The stage rectangle, with its origin at the top left as in Animate.
    pub fn stage_rect(&self) -> Rect {
        Rect::new(0.0, 0.0, self.size.width, self.size.height)
    }
}

/// Hands out identifiers that stay unique for the life of a document.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct IdAllocator {
    next: u64,
}

impl Default for IdAllocator {
    fn default() -> Self {
        // Start at 1 so 0 can mean "none" in file formats.
        Self { next: 1 }
    }
}

impl IdAllocator {
    fn take(&mut self) -> u64 {
        let id = self.next;
        self.next += 1;
        id
    }

    /// Ensure future ids do not collide with one already in use.
    ///
    /// Importers assign ids from the source file, so the allocator has to be
    /// told about them or the next new object would reuse an existing id.
    pub fn reserve_above(&mut self, used: u64) {
        self.next = self.next.max(used + 1);
    }
}

/// An immutable snapshot of the document.
#[derive(Debug, Clone)]
pub struct Scene {
    /// Private so every change goes through [`Scene::stage_mut`] and bumps the
    /// revision. As a public field it was possible to resize the stage without
    /// invalidating derived data or recording an undo step — a silent bug that
    /// only shows up later as a stale index.
    stage: StageProperties,
    /// The animated camera. Off until the user enables it. Private for the
    /// same reason as `stage`.
    camera: CameraTrack,
    /// Reusable symbols, organised into folders. Private for the same reason.
    library: Library,
    /// Imported sounds. Private for the same reason as the library.
    sounds: SoundLibrary,
    /// The lights. Private for the same reason: changing one has to bump the
    /// revision, or the renderer would keep drawing yesterday's shadows.
    lights: LightRig,
    layers: LayerStack,
    ids: IdAllocator,
    revision: u64,
    /// Symbols currently open for editing, outermost first.
    ///
    /// # Why this lives on the scene
    ///
    /// Animate's symbol editing mode redirects the *whole* authoring surface:
    /// the stage draws the symbol's contents, the timeline shows its frames,
    /// F6 inserts a keyframe on its layers, and selection addresses its
    /// objects. [`Scene::layers`] answering "the timeline the user is looking
    /// at" makes every panel and tool follow along at once. Threading a
    /// context parameter through seventy call sites would be the same
    /// behaviour with seventy more places to forget it.
    ///
    /// # Why it is not document state
    ///
    /// It is never serialised, never bumps the revision, and is not part of
    /// [`PartialEq`] — which symbol you happen to have open is not an edit,
    /// so it must not mark the document dirty or land in the undo history.
    /// [`Scene::stage_layers`] reaches the document's own timeline regardless
    /// of context, and that is what saving uses.
    editing: Vec<SymbolId>,
}

impl PartialEq for Scene {
    fn eq(&self, other: &Self) -> bool {
        self.stage == other.stage
            && self.camera == other.camera
            && self.library == other.library
            && self.sounds == other.sounds
            && self.lights == other.lights
            && self.layers == other.layers
            && self.ids == other.ids
            && self.revision == other.revision
    }
}

impl Default for Scene {
    fn default() -> Self {
        let mut scene = Self {
            stage: StageProperties::default(),
            camera: CameraTrack::new(),
            library: Library::new(),
            sounds: SoundLibrary::default(),
            lights: LightRig::default(),
            layers: LayerStack::new(),
            ids: IdAllocator::default(),
            revision: 0,
            editing: Vec::new(),
        };
        // Animate starts every document with one layer named "Layer_1".
        scene.add_layer("Layer_1", LayerKind::Normal);
        scene
    }
}

impl Scene {
    /// An empty document with no layers at all.
    ///
    /// Importers want this; the editor wants [`Scene::default`].
    pub fn empty() -> Self {
        Self {
            stage: StageProperties::default(),
            camera: CameraTrack::new(),
            library: Library::new(),
            sounds: SoundLibrary::default(),
            lights: LightRig::default(),
            layers: LayerStack::new(),
            ids: IdAllocator::default(),
            revision: 0,
            editing: Vec::new(),
        }
    }

    /// Which edit this snapshot represents.
    pub fn revision(&self) -> u64 {
        self.revision
    }

    /// The timeline the user is currently authoring.
    ///
    /// In symbol editing mode this is the open symbol's own layer stack, so
    /// the stage, the timeline and every tool address the symbol's contents
    /// without knowing that they are doing so. Use [`Scene::stage_layers`]
    /// when you specifically mean the document's main timeline.
    pub fn layers(&self) -> &LayerStack {
        match self.editing.last() {
            // A symbol deleted while open falls back to the main timeline
            // rather than showing nothing.
            Some(id) => self.library.get(*id).map_or(&self.layers, |s| &s.layers),
            None => &self.layers,
        }
    }

    /// The document's own timeline, whatever is open for editing.
    ///
    /// Saving, and anything that has to see the whole document rather than the
    /// current view, comes here.
    pub fn stage_layers(&self) -> &LayerStack {
        &self.layers
    }

    /// Symbols open for editing, outermost first. Empty on the main timeline.
    ///
    /// The breadcrumb above the stage is built from this.
    pub fn edit_path(&self) -> &[SymbolId] {
        &self.editing
    }

    /// Which symbol is being edited, if any.
    pub fn editing_symbol(&self) -> Option<SymbolId> {
        self.editing.last().copied()
    }

    /// Open a symbol for editing.
    ///
    /// Does not bump the revision: opening a symbol is navigation, not an
    /// edit, and marking the document dirty for it would be wrong.
    pub fn enter_symbol(&mut self, id: SymbolId) -> bool {
        if self.library.get(id).is_none() {
            return false;
        }
        // Re-entering a symbol already on the path would let a user walk a
        // cycle forever; jump back to that level instead.
        if let Some(index) = self.editing.iter().position(|s| *s == id) {
            self.editing.truncate(index + 1);
        } else {
            self.editing.push(id);
        }
        true
    }

    /// Step out one level, towards the main timeline.
    pub fn exit_symbol(&mut self) -> bool {
        self.editing.pop().is_some()
    }

    /// Return all the way to the main timeline.
    pub fn edit_document(&mut self) {
        self.editing.clear();
    }

    pub fn stage(&self) -> &StageProperties {
        &self.stage
    }

    /// Mutable stage properties. Bumps the revision.
    pub fn stage_mut(&mut self) -> &mut StageProperties {
        self.revision += 1;
        &mut self.stage
    }

    pub fn camera(&self) -> &CameraTrack {
        &self.camera
    }

    /// Mutable camera track. Bumps the revision, so camera moves are undoable.
    pub fn camera_mut(&mut self) -> &mut CameraTrack {
        self.revision += 1;
        &mut self.camera
    }

    pub fn library(&self) -> &Library {
        &self.library
    }

    /// Mutable library. Bumps the revision, so library edits are undoable.
    pub fn library_mut(&mut self) -> &mut Library {
        self.revision += 1;
        &mut self.library
    }

    // -- lighting ------------------------------------------------------------

    /// The document's lights.
    pub fn lights(&self) -> &LightRig {
        &self.lights
    }

    /// Mutable lights. Bumps the revision, so moving a light is undoable and
    /// invalidates whatever the renderer had cached.
    pub fn lights_mut(&mut self) -> &mut LightRig {
        self.revision += 1;
        &mut self.lights
    }

    /// Add a light, with a fresh id and a name that is not taken.
    pub fn add_light(&mut self, kind: LightKind) -> LightId {
        let id = LightId(self.ids.take());
        let base = kind.label();
        let mut name = base.to_string();
        for n in 2..10_000 {
            if !self.lights.lights.iter().any(|l| l.name == name) {
                break;
            }
            name = format!("{base} {n}");
        }

        let light = Light::new(id, name, kind);
        let rig = self.lights_mut();
        rig.lights.push(light);
        // Adding the first light switches the rig on: an animator who asks for
        // a sun means to see one, and a light that does nothing until a second
        // switch is found is a bug report waiting to happen.
        rig.enabled = true;
        id
    }

    /// How far artwork on `layer` stands above the surface its shadow falls
    /// on, for a given light.
    ///
    /// Flat artwork has no thickness, so the light's standing height is the
    /// base; a layer pushed towards the camera really is further in front of
    /// the background, and its shadow lengthens by exactly that much.
    pub fn shadow_height(&self, layer_depth: f64, light: &Light) -> f64 {
        let receiver = self.receiving_depth();
        (light.standing_height + (receiver - layer_depth)).max(0.0)
    }

    /// The depth of the surface shadows fall on: the furthest layer back.
    pub fn receiving_depth(&self) -> f64 {
        self.stage_layers()
            .iter()
            .map(|l| l.depth)
            .fold(0.0f64, f64::max)
    }

    // -- sound ---------------------------------------------------------------

    /// Imported sounds.
    pub fn sounds(&self) -> &SoundLibrary {
        &self.sounds
    }

    /// Mutable sound library. Bumps the revision, so importing is undoable.
    pub fn sounds_mut(&mut self) -> &mut SoundLibrary {
        self.revision += 1;
        &mut self.sounds
    }

    /// Import a sound, giving it a unique name and a fresh id.
    pub fn add_sound(
        &mut self,
        name: &str,
        data: std::sync::Arc<Vec<u8>>,
        format: &str,
        sample_rate: u32,
        channels: u16,
        length: u64,
    ) -> SoundId {
        let id = SoundId(self.ids.take());
        let name = self.sounds.unique_name(name);
        self.sounds_mut().insert(SoundAsset {
            id,
            name,
            data,
            format: format.to_ascii_lowercase(),
            sample_rate,
            channels,
            length,
        });
        id
    }

    /// Attach a sound to the keyframe governing `frame` on `layer`.
    ///
    /// Animate attaches sound to a *keyframe*, not to a layer, so one layer
    /// can carry a whole scene's effects. Returns whether there was a keyframe
    /// to attach it to.
    pub fn set_frame_sound(
        &mut self,
        layer: LayerId,
        frame: u32,
        sound: Option<SoundRef>,
    ) -> bool {
        let mut attached = false;
        self.update_layer(layer, |l| {
            if let Some(keyframe) = l.frames.keyframe_at_mut(frame) {
                keyframe.sound = sound;
                attached = true;
            }
        });
        attached
    }

    /// The sound on the keyframe governing `frame`.
    pub fn frame_sound(&self, layer: LayerId, frame: u32) -> Option<SoundRef> {
        self.layers()
            .get(layer)?
            .frames
            .keyframe_at(frame)
            .and_then(|k| k.sound)
    }

    /// Every sound on the **document's own timeline**, with the frame it
    /// starts on.
    ///
    /// # Why this reads the stage timeline, always
    ///
    /// This is what plays. It deliberately ignores which symbol is open for
    /// editing, so the dialogue on the root timeline keeps sounding when you
    /// step into a character to animate its walk — and when you step from
    /// there into its head to animate the mouth. The sound belongs to the
    /// document; the symbol you are inside is a view of it.
    ///
    /// It is the same distinction saving makes, for the same reason.
    pub fn stage_cues(&self) -> Vec<SoundCue> {
        let mut cues = Vec::new();
        for layer in self.stage_layers().iter() {
            // A hidden layer is still heard: hiding a layer hides *artwork*,
            // and an animator hides layers constantly while working. Losing
            // the soundtrack because a layer was hidden would be surprising in
            // exactly the way this whole design is trying to avoid.
            for keyframe in layer.frames.keyframes() {
                let Some(sound) = keyframe.sound else { continue };
                if sound.sync == SoundSync::Stop {
                    continue;
                }
                cues.push(SoundCue {
                    sound: sound.sound,
                    start_frame: keyframe.start,
                    volume: sound.volume,
                    sync: sound.sync,
                });
            }
        }
        cues.sort_by_key(|c| c.start_frame);
        cues
    }

    /// Sounds placed anywhere, including inside symbols, for the Library
    /// panel's use count.
    pub fn sound_usage(&self) -> std::collections::BTreeMap<SoundId, usize> {
        let mut counts = std::collections::BTreeMap::new();
        let stage = self.stage_layers().iter();
        let nested = self.library.iter().flat_map(|s| s.layers.iter());
        for layer in stage.chain(nested) {
            for keyframe in layer.frames.keyframes() {
                if let Some(sound) = keyframe.sound {
                    *counts.entry(sound.sound).or_insert(0) += 1;
                }
            }
        }
        counts
    }

    /// Add a symbol, giving it a unique name and a fresh id.
    pub fn add_symbol(
        &mut self,
        name: impl Into<String>,
        kind: SymbolKind,
        folder: Option<&str>,
    ) -> SymbolId {
        let id = SymbolId(self.ids.take());
        let name = self.library.unique_name(&name.into());
        let mut symbol = Symbol::new(id, name, kind);
        symbol.folder = folder.map(|f| f.trim_matches('/').to_string());
        // A symbol always needs one layer, or there is nowhere to draw.
        symbol.layers.push_front(Layer::normal(LayerId(self.ids.take()), "Layer_1"));
        self.library.insert(symbol);
        self.bump();
        id
    }

    /// Place an instance of `symbol` on a layer at `frame`.
    pub fn add_instance_at(
        &mut self,
        layer: LayerId,
        frame: u32,
        symbol: SymbolId,
        transform: Affine,
    ) -> Option<ObjectId> {
        // An instance of a symbol that is not in the library would draw
        // nothing and save as a dangling reference.
        self.library.get(symbol)?;
        let id = ObjectId(self.ids.take());
        let object = Object::instance_of(id, symbol).with_transform(transform);
        self.add_object_at(layer, frame, object)
    }

    /// Bounds of a placed instance, resolved through the library.
    ///
    /// [`Object::bounds`] cannot do this because an object has no way to reach
    /// the library, so it returns a placeholder; anything that needs real
    /// extents for an instance comes here.
    pub fn instance_bounds(&self, object: &Object) -> Option<Rect> {
        let instance = object.instance()?;
        let symbol = self.library.get(instance.symbol)?;
        let local = symbol.bounds()?;
        Some(object::transform_rect(object.transform, local))
    }

    /// Bounds of an object, resolving instances through the library.
    pub fn resolved_bounds(&self, object: &Object) -> Rect {
        match &object.kind {
            ObjectKind::Instance(_) => self
                .instance_bounds(object)
                .unwrap_or_else(|| object.bounds()),
            ObjectKind::Group(children) => children
                .iter()
                .map(|c| {
                    object::transform_rect(object.transform, self.resolved_bounds(c))
                })
                .reduce(|a, b| a.union(b))
                .unwrap_or_else(|| object.bounds()),
            // Rigged artwork measures itself, posed, and needs no library.
            ObjectKind::Shape(_) | ObjectKind::Armature(_) | ObjectKind::Warp(_) => object.bounds(),
        }
    }

    /// How many times each symbol is used, for the Library panel.
    ///
    /// Counts instances on the stage *and* inside other symbols, because a
    /// symbol used only inside another is still in use — deleting it would
    /// break that one.
    pub fn symbol_usage(&self) -> std::collections::BTreeMap<SymbolId, usize> {
        let mut counts = std::collections::BTreeMap::new();

        fn walk(object: &Object, counts: &mut std::collections::BTreeMap<SymbolId, usize>) {
            match &object.kind {
                ObjectKind::Instance(i) => *counts.entry(i.symbol).or_insert(0) += 1,
                ObjectKind::Group(children) => {
                    for c in children {
                        walk(c, counts);
                    }
                }
                // A symbol rigged to an armature is still in use, and deleting
                // it would leave the rig drawing nothing.
                ObjectKind::Armature(rig) => {
                    for part in &rig.parts {
                        walk(&part.artwork, counts);
                    }
                }
                ObjectKind::Shape(_) | ObjectKind::Warp(_) => {}
            }
        }

        // The document's own timeline, not the one being authored: a use count
        // has to cover the whole file however you are navigating it.
        let stage = self.stage_layers().iter();
        let nested = self.library.iter().flat_map(|s| s.layers.iter());
        for layer in stage.chain(nested) {
            for object in layer.all_objects() {
                walk(object, &mut counts);
            }
        }
        counts
    }

    /// Mutable access that bumps the revision.
    ///
    /// All edits go through here so no path can change the document without
    /// invalidating derived data.
    pub fn edit_layers(&mut self) -> &mut LayerStack {
        self.revision += 1;
        self.active_layers_mut()
    }

    /// The document's own timeline, mutably, whatever is open for editing.
    ///
    /// Loading a file comes here: it is rebuilding the document, not editing
    /// whatever the previous document happened to have open.
    pub fn edit_stage_layers(&mut self) -> &mut LayerStack {
        self.revision += 1;
        &mut self.layers
    }

    /// Mutable access to the timeline being authored, without bumping.
    ///
    /// Private because callers that skip the bump would leave derived data
    /// stale; every public path either bumps first or calls [`Scene::bump`].
    fn active_layers_mut(&mut self) -> &mut LayerStack {
        if let Some(id) = self.editing.last().copied()
            && self.library.get(id).is_some()
        {
            return self
                .library
                .layers_mut(id)
                .expect("the symbol was just confirmed to exist");
        }
        &mut self.layers
    }

    fn bump(&mut self) {
        self.revision += 1;
    }

    /// Add a layer at the top, as Animate does.
    pub fn add_layer(&mut self, name: impl Into<String>, kind: LayerKind) -> LayerId {
        let id = LayerId(self.ids.take());
        let mut layer = Layer::new(id, name, kind);
        // By position rather than by id: see `layer::default_color`.
        layer.color = crate::layer::default_color(self.layers().len());
        self.active_layers_mut().push_front(layer);
        self.bump();
        id
    }

    pub fn remove_layer(&mut self, id: LayerId) -> Option<Arc<Layer>> {
        let removed = self.active_layers_mut().remove(id);
        if removed.is_some() {
            // Anything that followed it is released rather than left pointing
            // at a layer that is gone: a dangling link resolves to identity, so
            // the artwork would be right, but the Parent column would show a
            // name for a layer the user cannot find.
            let orphans = self.layers().followers_of(id);
            for orphan in orphans {
                self.active_layers_mut().update(orphan, |l| l.follows = None);
            }
            self.bump();
        }
        removed
    }

    /// Edit a layer in place. Returns false if it does not exist.
    pub fn update_layer(&mut self, id: LayerId, f: impl FnOnce(&mut Layer)) -> bool {
        let ok = self.active_layers_mut().update(id, f);
        if ok {
            self.bump();
        }
        ok
    }

    /// Place a shape on a layer at `frame`.
    ///
    /// The artwork lands on the keyframe governing that frame, so drawing on
    /// frame 7 of a span beginning at frame 5 edits frame 5 — Animate's
    /// behaviour.
    pub fn add_shape_at(
        &mut self,
        layer: LayerId,
        frame: u32,
        shape: ShapeData,
    ) -> Option<ObjectId> {
        let id = ObjectId(self.ids.take());
        let object = Arc::new(Object::shape(id, shape));
        let mut placed = false;
        self.active_layers_mut()
            .update(layer, |l| placed = l.push_object_at(frame, object));
        placed.then(|| {
            self.bump();
            id
        })
    }

    /// Place a shape on frame 0. Convenience for tests and importers.
    pub fn add_shape(&mut self, layer: LayerId, shape: ShapeData) -> Option<ObjectId> {
        self.add_shape_at(layer, 0, shape)
    }

    /// Place an already-built object on a layer at `frame`.
    pub fn add_object_at(&mut self, layer: LayerId, frame: u32, object: Object) -> Option<ObjectId> {
        let id = object.id;
        self.ids.reserve_above(id.0);
        let mut placed = false;
        self.active_layers_mut()
            .update(layer, |l| placed = l.push_object_at(frame, Arc::new(object)));
        placed.then(|| {
            self.bump();
            id
        })
    }

    /// Place an already-built object on frame 0.
    pub fn add_object(&mut self, layer: LayerId, object: Object) -> Option<ObjectId> {
        self.add_object_at(layer, 0, object)
    }

    /// Frames in the document: the longest layer, and at least one.
    pub fn frame_count(&self) -> u32 {
        self.layers()
            .frame_count()
            .max(self.camera().last_frame() + 1)
            .max(1)
    }

    /// Duration in seconds at the document's frame rate.
    pub fn duration_seconds(&self) -> f64 {
        let fps = if self.stage().frame_rate > 0.0 {
            self.stage().frame_rate
        } else {
            24.0
        };
        self.frame_count() as f64 / fps
    }

    /// The camera transform for `frame`, or identity when the camera is off.
    pub fn camera_transform(&self, frame: u32) -> Affine {
        self.camera().transform_at(frame, self.stage().size)
    }

    /// The camera transform for artwork on a layer at `depth`.
    ///
    /// `None` when the layer sits at or behind the camera and should not be
    /// drawn at all.
    pub fn camera_transform_at_depth(&self, frame: u32, depth: f64) -> Option<Affine> {
        self.camera()
            .transform_at_depth(frame, self.stage().size, depth)
    }

    /// How a layer at `depth` is projected onto the frame.
    ///
    /// A homography rather than an affine, because a tilted camera turns a
    /// layer's rectangle into a trapezoid. With no tilt it *is* an affine —
    /// exactly [`Self::camera_transform_at_depth`] — so the render path takes
    /// the same route it always did for the documents that do not use this.
    ///
    /// `None` when the layer is at or behind the camera.
    pub fn camera_projection_at_depth(
        &self,
        frame: u32,
        depth: f64,
    ) -> Option<buzz_geom::Projection> {
        self.camera()
            .projection_at_depth(frame, self.stage().size, depth)
    }

    /// Does the shot tilt at all?
    ///
    /// Lets the renderer and the editor take the flat path when nothing in the
    /// document has ever asked for perspective.
    pub fn camera_has_tilt(&self) -> bool {
        self.camera().has_tilt()
    }

    /// Move a point from where the user sees it into a layer's own
    /// coordinates.
    ///
    /// Depth draws a layer's artwork somewhere other than where its geometry
    /// says it is, so a click has to be moved the same way in reverse before it
    /// can be tested against that geometry. Without this, clicking a layer
    /// pushed into the distance selects nothing — the artwork is on screen but
    /// the hit test is looking where it used to be.
    ///
    /// Returns the point unchanged for a layer on the focal plane, which is
    /// every layer in a document that does not use depth.
    pub fn view_to_layer(&self, frame: u32, depth: f64, point: Point) -> Option<Point> {
        if depth == 0.0 && !self.camera_has_tilt() {
            return Some(point);
        }

        // Through the projection, not the affine: with the camera tilted, the
        // artwork is not merely scaled but foreshortened, and a click has to be
        // carried back through the same perspective it was drawn with — or
        // tilted artwork is visible and unclickable.
        let with_depth = self.camera_projection_at_depth(frame, depth)?;
        // Relative to the depth-zero transform, because that is the space the
        // rest of the editor already works in.
        let base = buzz_geom::Projection::from_affine(self.camera_transform(frame));
        let combined = with_depth.inverse()?.then(&base);
        combined.map_point(point)
    }

    /// Allocate an id without attaching anything to the document yet.
    pub fn next_object_id(&mut self) -> ObjectId {
        ObjectId(self.ids.take())
    }

    /// Guarantee future ids exceed `used`.
    ///
    /// Loading a document and importing a `.fla` both bring their own ids. The
    /// allocator has to be raised past them or the next new object would
    /// silently collide with something already in the file.
    pub fn reserve_ids_above(&mut self, used: u64) {
        self.ids.reserve_above(used);
    }

    /// Find an object and the layer holding it.
    pub fn find_object(&self, id: ObjectId) -> Option<(LayerId, &Arc<Object>)> {
        self.layers()
            .iter()
            .find_map(|l| l.find_object(id).map(|o| (l.id, o)))
    }

    /// Edit an object in place, wherever on the current timeline it lives.
    ///
    /// Searches every keyframe rather than only the one under the playhead:
    /// panels address an object by id, and an object selected on frame 5 is
    /// still that object when the playhead has moved on. Returns false if no
    /// such object exists, and bumps the revision only when one was found.
    pub fn update_object(&mut self, id: ObjectId, f: impl FnOnce(&mut Object)) -> bool {
        let Some((layer_id, _)) = self.find_object(id) else {
            return false;
        };
        let mut f = Some(f);
        let mut changed = false;
        self.active_layers_mut().update(layer_id, |layer| {
            for keyframe in layer.frames.keyframes_mut() {
                let objects = Arc::make_mut(&mut keyframe.objects);
                if let Some(object) = objects.iter_mut().find(|o| o.id == id) {
                    // `make_mut` here is the copy-on-write step: the object is
                    // only cloned if a snapshot elsewhere still holds it.
                    if let Some(f) = f.take() {
                        f(Arc::make_mut(object));
                        changed = true;
                    }
                    break;
                }
            }
        });
        if changed {
            self.bump();
        }
        changed
    }

    /// Edit an object **on the keyframe that owns `frame`**.
    ///
    /// # Why this exists next to [`Self::update_object`]
    ///
    /// Pressing F6 duplicates a keyframe by cloning the `Arc` around its
    /// objects, so the *same object id* legitimately appears on several
    /// keyframes of one layer. `update_object` edits the first one it finds,
    /// which is the earliest — so moving artwork with the playhead on frame 12
    /// would silently change frame 0 instead, and leave frame 12 exactly as it
    /// was. Anything that knows where the playhead is should come here.
    ///
    /// Falls back to searching every keyframe when the object is not on the
    /// one owning `frame`: an object selected on another layer, or held on a
    /// keyframe beyond the span, should still be editable rather than
    /// mysteriously immovable.
    pub fn update_object_at(
        &mut self,
        frame: u32,
        id: ObjectId,
        f: impl FnOnce(&mut Object),
    ) -> bool {
        let Some((layer_id, _)) = self.find_object(id) else {
            return false;
        };
        let mut f = Some(f);
        let mut changed = false;

        self.active_layers_mut().update(layer_id, |layer| {
            if let Some(keyframe) = layer.frames.keyframe_at_mut(frame) {
                let objects = Arc::make_mut(&mut keyframe.objects);
                if let Some(object) = objects.iter_mut().find(|o| o.id == id)
                    && let Some(f) = f.take()
                {
                    f(Arc::make_mut(object));
                    changed = true;
                    return;
                }
            }

            for keyframe in layer.frames.keyframes_mut() {
                let objects = Arc::make_mut(&mut keyframe.objects);
                if let Some(object) = objects.iter_mut().find(|o| o.id == id) {
                    if let Some(f) = f.take() {
                        f(Arc::make_mut(object));
                        changed = true;
                    }
                    break;
                }
            }
        });

        if changed {
            self.bump();
        }
        changed
    }

    pub fn remove_object(&mut self, id: ObjectId) -> Option<Arc<Object>> {
        let layer_id = self.find_object(id).map(|(l, _)| l)?;
        let mut removed = None;
        self.active_layers_mut().update(layer_id, |l| removed = l.remove_object(id));
        if removed.is_some() {
            self.bump();
        }
        removed
    }

    /// Total leaf shapes across every layer and every keyframe.
    pub fn shape_count(&self) -> usize {
        self.layers()
            .iter()
            .flat_map(|l| l.all_objects())
            .map(|o| o.shape_count())
            .sum()
    }

    /// Leaf shapes visible at `frame`.
    pub fn shape_count_at(&self, frame: u32) -> usize {
        self.layers()
            .iter()
            .flat_map(|l| l.objects_at(frame).iter())
            .map(|o| o.shape_count())
            .sum()
    }

    /// Bounds of all artwork across every frame, ignoring the stage rectangle.
    pub fn content_bounds(&self) -> Option<Rect> {
        self.layers().iter().filter_map(|l| l.bounds()).reduce(|a, b| a.union(b))
    }

    /// Everything the user could reasonably want framed: the stage plus any
    /// artwork sitting out on the pasteboard.
    pub fn fit_bounds(&self) -> Rect {
        match self.content_bounds() {
            Some(content) => content.union(self.stage().stage_rect()),
            None => self.stage().stage_rect(),
        }
    }

    /// Build the entries for a spatial index, in paint order.
    ///
    /// Depth increases towards the front, matching the convention in
    /// `buzz_geom::hit` where later entries are on top.
    pub fn index_entries_at(&self, frame: u32) -> Vec<IndexEntry> {
        let mut entries = Vec::new();
        let mut depth = 0usize;
        for layer in self.layers().drawable_at(frame) {
            // A followed layer's artwork is drawn somewhere other than where
            // its geometry says it is, so the index has to agree — otherwise a
            // parented limb is visible but unclickable.
            let follows = self.layers().inherited_transform(layer.id, frame);
            for object in layer.objects_at(frame) {
                if !object.visible {
                    continue;
                }
                entries.push(IndexEntry {
                    object: object.id,
                    layer: layer.id,
                    bounds: crate::object::transform_rect(follows, object.bounds()),
                    depth,
                });
                depth += 1;
            }
        }
        entries
    }

    /// Index entries for frame 0.
    pub fn index_entries(&self) -> Vec<IndexEntry> {
        self.index_entries_at(0)
    }

    /// Build a spatial index for `frame`.
    ///
    /// Cheap enough to call directly for small documents; for large ones run it
    /// on the background pool and hand the result over when it is ready.
    pub fn build_index_at(&self, frame: u32) -> SpatialIndex {
        SpatialIndex::build(self.index_entries_at(frame), self.revision)
    }

    pub fn build_index(&self) -> SpatialIndex {
        self.build_index_at(0)
    }

    /// Every shape visible at `frame`, with its resolved world transform.
    ///
    /// The camera transform is *not* applied here: the renderer needs to know
    /// where artwork sits in document space for hit-testing and culling, and
    /// applies the camera separately.
    pub fn flatten_at(&self, frame: u32) -> Vec<(Affine, ShapeData)> {
        let mut out = Vec::new();
        for layer in self.layers().drawable_at(frame) {
            for object in layer.objects_at(frame) {
                object.flatten(Affine::IDENTITY, &mut out);
            }
        }
        out
    }

    pub fn flatten_for_render(&self) -> Vec<(Affine, ShapeData)> {
        self.flatten_at(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use buzz_geom::{BezPath, Point, Shape as _};
    use kurbo::Rect as KRect;

    fn square(x: f64, y: f64, size: f64) -> BezPath {
        KRect::new(x, y, x + size, y + size).to_path(1e-9)
    }

    fn scene_with_shapes(n: u64) -> (Scene, LayerId) {
        let mut scene = Scene::default();
        let layer = scene.layers().iter().next().unwrap().id;
        for i in 0..n {
            scene.add_shape(
                layer,
                ShapeData::filled(square(i as f64 * 20.0, 0.0, 10.0), Color::WHITE),
            );
        }
        (scene, layer)
    }

    #[test]
    fn a_new_document_matches_animates_defaults() {
        let scene = Scene::default();
        assert_eq!(scene.stage().size, Size::new(550.0, 400.0));
        assert_eq!(scene.stage().frame_rate, 24.0);
        assert_eq!(scene.layers().len(), 1, "Animate starts with one layer");
        assert_eq!(scene.layers().iter().next().unwrap().name, "Layer_1");
    }

    #[test]
    fn every_edit_bumps_the_revision() {
        let mut scene = Scene::default();
        let layer = scene.layers().iter().next().unwrap().id;

        let r0 = scene.revision();
        scene.add_shape(layer, ShapeData::filled(square(0.0, 0.0, 10.0), Color::WHITE));
        let r1 = scene.revision();
        assert!(r1 > r0);

        scene.update_layer(layer, |l| l.name = "renamed".into());
        assert!(scene.revision() > r1);
    }

    #[test]
    fn a_failed_edit_does_not_bump_the_revision() {
        let mut scene = Scene::default();
        let before = scene.revision();
        assert!(!scene.update_layer(LayerId(9999), |l| l.name = "nope".into()));
        assert_eq!(scene.revision(), before);
        assert!(scene.remove_layer(LayerId(9999)).is_none());
        assert_eq!(scene.revision(), before);
    }

    #[test]
    fn ids_are_unique_across_layers_and_objects() {
        let (mut scene, layer) = scene_with_shapes(5);
        let extra = scene.add_layer("Another", LayerKind::Normal);
        let shape = scene
            .add_shape(extra, ShapeData::filled(square(0.0, 0.0, 5.0), Color::WHITE))
            .unwrap();

        assert_ne!(layer.0, extra.0);
        assert!(scene.find_object(shape).is_some());
    }

    /// Importers assign their own ids; the allocator must not later reuse them.
    #[test]
    fn imported_ids_are_reserved() {
        let mut scene = Scene::empty();
        let layer = scene.add_layer("L", LayerKind::Normal);

        let imported = Object::shape(
            ObjectId(5000),
            ShapeData::filled(square(0.0, 0.0, 10.0), Color::WHITE),
        );
        scene.add_object(layer, imported).unwrap();

        let fresh = scene.next_object_id();
        assert!(
            fresh.0 > 5000,
            "a new id ({}) must not collide with an imported one",
            fresh.0
        );
    }

    /// The property the whole architecture depends on.
    #[test]
    fn a_snapshot_is_unaffected_by_later_edits() {
        let (mut scene, layer) = scene_with_shapes(10);
        let snapshot = scene.clone();
        let before = snapshot.shape_count();

        scene.add_shape(layer, ShapeData::filled(square(999.0, 0.0, 10.0), Color::WHITE));
        scene.update_layer(layer, |l| l.name = "changed".into());

        assert_eq!(snapshot.shape_count(), before, "the snapshot changed");
        assert_eq!(scene.shape_count(), before + 1);
        assert_eq!(snapshot.layers().get(layer).unwrap().name, "Layer_1");
        assert!(snapshot.revision() < scene.revision());
    }

    /// Undo as snapshot swapping, which is the point of the design.
    #[test]
    fn undo_is_just_restoring_an_earlier_snapshot() {
        let (mut scene, layer) = scene_with_shapes(3);
        let history: Vec<Scene> = (0..4)
            .map(|i| {
                let snap = scene.clone();
                scene.add_shape(
                    layer,
                    ShapeData::filled(square(i as f64, 100.0, 5.0), Color::WHITE),
                );
                snap
            })
            .collect();

        assert_eq!(scene.shape_count(), 7);
        let restored = history[0].clone();
        assert_eq!(restored.shape_count(), 3, "undo should return to 3 shapes");
        // And the later snapshots are still intact and distinct.
        assert_eq!(history[2].shape_count(), 5);
    }

    /// Snapshotting is `O(1)`, not `O(objects)`.
    ///
    /// Cloning a `Scene` clones the single `Arc` wrapping the layer list, so
    /// nothing per-object happens at all — the individual object refcounts are
    /// untouched. Copy-on-write only descends into a layer, and then into an
    /// object, at the moment one is actually edited.
    #[test]
    fn cloning_a_scene_shares_structure_wholesale() {
        let (scene, layer) = scene_with_shapes(50);
        let object = Arc::clone(scene.layers().get(layer).unwrap().objects_at(0).first().unwrap());
        let before = Arc::strong_count(&object);

        let clones: Vec<Scene> = (0..20).map(|_| scene.clone()).collect();

        assert_eq!(
            Arc::strong_count(&object),
            before,
            "snapshotting should not touch per-object refcounts at all"
        );
        assert_eq!(clones.len(), 20);
        assert_eq!(clones[0].shape_count(), 50);
    }

    /// ...and copy-on-write descends only when an edit forces it.
    #[test]
    fn editing_a_snapshot_forks_only_the_touched_layer() {
        let (mut scene, layer) = scene_with_shapes(50);
        let snapshot = scene.clone();

        let layer_before = Arc::as_ptr(snapshot.layers().get(layer).unwrap());
        scene.update_layer(layer, |l| l.name = "edited".into());
        let layer_after = Arc::as_ptr(scene.layers().get(layer).unwrap());

        assert_ne!(
            layer_before, layer_after,
            "the edited layer should have been forked"
        );
        assert_eq!(
            Arc::as_ptr(snapshot.layers().get(layer).unwrap()),
            layer_before,
            "the snapshot must still point at the original layer"
        );
    }

    /// Editing one object must leave its neighbours as the same allocations.
    #[test]
    fn an_edit_touches_only_what_changed() {
        let (mut scene, layer) = scene_with_shapes(20);
        let untouched_before =
            Arc::as_ptr(scene.layers().get(layer).unwrap().objects_at(0).last().unwrap());

        scene.update_layer(layer, |l| {
            let objects = l.frames.objects_at_mut(0).expect("frame 0 exists");
            let first = Arc::make_mut(&mut objects[0]);
            first.transform = Affine::translate((5.0, 5.0));
        });

        let untouched_after =
            Arc::as_ptr(scene.layers().get(layer).unwrap().objects_at(0).last().unwrap());
        assert_eq!(
            untouched_before, untouched_after,
            "an unrelated object was reallocated"
        );
    }

    #[test]
    fn snapshotting_a_large_scene_is_cheap() {
        let (scene, _) = scene_with_shapes(20_000);

        let start = std::time::Instant::now();
        let snapshots: Vec<Scene> = (0..100).map(|_| scene.clone()).collect();
        let elapsed = start.elapsed();

        assert_eq!(snapshots.len(), 100);
        // 100 snapshots of a 20k-object scene. Generous, to catch an accidental
        // deep copy rather than to police small drift.
        assert!(
            elapsed.as_millis() < 500,
            "100 snapshots of 20k objects took {elapsed:?}; \
             cloning is probably deep-copying"
        );
    }

    #[test]
    fn hidden_layers_and_objects_are_excluded_from_rendering() {
        let mut scene = Scene::empty();
        let visible = scene.add_layer("Visible", LayerKind::Normal);
        let hidden = scene.add_layer("Hidden", LayerKind::Normal);

        scene.add_shape(visible, ShapeData::filled(square(0.0, 0.0, 10.0), Color::WHITE));
        let ghost = scene
            .add_shape(hidden, ShapeData::filled(square(50.0, 0.0, 10.0), Color::WHITE))
            .unwrap();
        scene.update_layer(hidden, |l| l.visible = false);

        assert_eq!(scene.flatten_for_render().len(), 1);
        assert!(
            !scene.index_entries().iter().any(|e| e.object == ghost),
            "a hidden layer's objects must not be indexed"
        );
    }

    #[test]
    fn folders_contribute_no_artwork_to_the_render() {
        let mut scene = Scene::empty();
        let folder = scene.add_layer("Folder", LayerKind::Folder);
        // Even if something were somehow attached to a folder.
        scene.add_shape(folder, ShapeData::filled(square(0.0, 0.0, 10.0), Color::WHITE));
        assert!(scene.flatten_for_render().is_empty());
    }

    #[test]
    fn index_depth_increases_towards_the_front() {
        let mut scene = Scene::empty();
        let back = scene.add_layer("Back", LayerKind::Normal);
        let front = scene.add_layer("Front", LayerKind::Normal);

        let back_shape = scene
            .add_shape(back, ShapeData::filled(square(0.0, 0.0, 10.0), Color::WHITE))
            .unwrap();
        let front_shape = scene
            .add_shape(front, ShapeData::filled(square(0.0, 0.0, 10.0), Color::WHITE))
            .unwrap();

        let entries = scene.index_entries();
        let depth_of = |id: ObjectId| entries.iter().find(|e| e.object == id).unwrap().depth;
        assert!(
            depth_of(front_shape) > depth_of(back_shape),
            "the layer above must index at a greater depth"
        );
    }

    #[test]
    fn the_index_matches_the_snapshot_revision() {
        let (mut scene, layer) = scene_with_shapes(10);
        let index = scene.build_index();
        assert!(index.is_current_for(scene.revision()));

        scene.add_shape(layer, ShapeData::filled(square(0.0, 500.0, 10.0), Color::WHITE));
        assert!(
            !index.is_current_for(scene.revision()),
            "the index should now report itself stale"
        );
    }

    /// The index must be buildable off the UI thread from a snapshot, with no
    /// locking and no coordination. That is the whole reason the document model
    /// is immutable.
    #[test]
    fn an_index_can_be_rebuilt_on_another_thread_while_editing_continues() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<Scene>();
        assert_send_sync::<SpatialIndex>();

        let (mut scene, layer) = scene_with_shapes(5_000);
        let snapshot = scene.clone();
        let snapshot_revision = snapshot.revision();

        // Hand the snapshot to a worker...
        let worker = std::thread::spawn(move || snapshot.build_index());

        // ...and keep editing the document meanwhile.
        for i in 0..100 {
            scene.add_shape(
                layer,
                ShapeData::filled(square(i as f64, 900.0, 4.0), Color::WHITE),
            );
        }

        let index = worker.join().expect("index build panicked");

        assert_eq!(index.len(), 5_000, "the index describes the snapshot");
        assert!(index.is_current_for(snapshot_revision));
        assert!(
            !index.is_current_for(scene.revision()),
            "concurrent edits should leave the index detectably stale"
        );
        assert_eq!(scene.shape_count(), 5_100);
    }

    #[test]
    fn index_queries_find_the_right_objects() {
        let (scene, _) = scene_with_shapes(200);
        let index = scene.build_index();
        assert_eq!(index.len(), 200);

        let hits = index.query_point(Point::new(45.0, 5.0));
        assert_eq!(hits.len(), 1, "expected exactly one shape at x=45");
    }

    #[test]
    fn removing_an_object_finds_it_on_any_layer() {
        let mut scene = Scene::empty();
        let a = scene.add_layer("A", LayerKind::Normal);
        let b = scene.add_layer("B", LayerKind::Normal);
        scene.add_shape(a, ShapeData::filled(square(0.0, 0.0, 10.0), Color::WHITE));
        let target = scene
            .add_shape(b, ShapeData::filled(square(20.0, 0.0, 10.0), Color::WHITE))
            .unwrap();

        assert!(scene.remove_object(target).is_some());
        assert!(scene.find_object(target).is_none());
        assert_eq!(scene.shape_count(), 1);
        assert!(scene.remove_object(target).is_none(), "removing twice is a no-op");
    }

    #[test]
    fn fit_bounds_always_includes_the_stage() {
        let scene = Scene::default();
        assert_eq!(scene.fit_bounds(), scene.stage().stage_rect());

        let (mut scene, layer) = scene_with_shapes(0);
        scene.add_shape(
            layer,
            ShapeData::filled(square(-500.0, -500.0, 10.0), Color::WHITE),
        );
        let fit = scene.fit_bounds();
        assert!(fit.x0 <= -500.0, "pasteboard artwork should be included");
        assert!(
            fit.x1 >= scene.stage().size.width,
            "the stage should still be included"
        );
    }

    /// The Properties panel edits an instance by id, so the edit has to reach
    /// the object wherever on the layer it lives — not only under the playhead.
    #[test]
    fn update_object_reaches_an_object_on_any_keyframe() {
        let mut scene = Scene::default();
        let layer = scene.layers().iter().next().unwrap().id;
        let symbol = scene.add_symbol("Hero", SymbolKind::Graphic, None);

        // Put the instance on a later keyframe, away from frame 0.
        scene.update_layer(layer, |l| {
            l.frames.insert_frame(20);
            l.frames.insert_keyframe(10);
        });
        let id = scene
            .add_instance_at(layer, 10, symbol, Affine::IDENTITY)
            .expect("the instance is placed");

        let before = scene.revision();
        assert!(scene.update_object(id, |o| {
            if let ObjectKind::Instance(i) = &mut o.kind {
                i.first_frame = 3;
                i.loop_mode = LoopMode::PlayOnce;
            }
        }));
        assert!(scene.revision() > before, "an edit must bump the revision");

        let (_, object) = scene.find_object(id).expect("still there");
        let instance = object.instance().expect("still an instance");
        assert_eq!(instance.first_frame, 3);
        assert_eq!(instance.loop_mode, LoopMode::PlayOnce);
    }

    /// Layer colours exist to tell layers apart, so two layers in one document
    /// must not share one until the palette genuinely runs out. Indexing by id
    /// broke this: ids are shared with objects, so a document with seven
    /// shapes per layer strode the ids by eight and gave every layer the same
    /// colour.
    #[test]
    fn layers_get_distinct_colours_even_when_objects_consume_ids() {
        let mut scene = Scene::default();

        for _ in 0..5 {
            let layer = scene.layers().iter().next().unwrap().id;
            // Seven shapes, which is what strode the ids by eight.
            for i in 0..7 {
                scene.add_shape(
                    layer,
                    ShapeData::filled(square(i as f64, 0.0, 5.0), Color::WHITE),
                );
            }
            scene.add_layer("Another", LayerKind::Normal);
        }

        let colours: Vec<[u8; 4]> = scene
            .layers()
            .iter()
            .map(|l| l.color.to_rgba8().to_u8_array())
            .collect();
        let distinct: std::collections::BTreeSet<_> = colours.iter().collect();

        assert_eq!(
            distinct.len(),
            colours.len(),
            "every layer should have its own colour, got {colours:?}"
        );
    }

    /// A no-op must stay a no-op: bumping for an object that does not exist
    /// would invalidate the spatial index and mark the document dirty for
    /// nothing.
    #[test]
    fn update_object_on_a_missing_object_changes_nothing() {
        let (mut scene, _) = scene_with_shapes(3);
        let before = scene.revision();

        assert!(!scene.update_object(ObjectId(9999), |o| o.visible = false));
        assert_eq!(scene.revision(), before);
    }

    /// **The defect `update_object_at` exists for.** F6 duplicates a keyframe
    /// by cloning the `Arc` around its objects, so one id appears on several
    /// keyframes. Editing "the object" without saying *when* changed the
    /// earliest keyframe — so posing a rig or dragging a shape on frame 12
    /// silently damaged frame 0 and appeared to do nothing.
    #[test]
    fn editing_an_object_lands_on_the_keyframe_the_playhead_is_in() {
        let mut scene = Scene::default();
        let layer = scene.layers().iter().next().expect("a layer").id;
        let id = scene
            .add_shape(
                layer,
                ShapeData::filled(
                    kurbo::Rect::new(0.0, 0.0, 10.0, 10.0).to_path(1e-9),
                    Color::WHITE,
                ),
            )
            .expect("a shape");

        // Extend the span, then duplicate the keyframe at frame 10.
        scene.update_layer(layer, |l| {
            for _ in 0..10 {
                l.frames.insert_frame(0);
            }
            l.frames.insert_keyframe(10);
        });

        assert!(scene.update_object_at(10, id, |o| {
            o.transform = Affine::translate((100.0, 0.0));
        }));

        let at = |frame: u32| {
            scene
                .layers()
                .iter()
                .flat_map(|l| l.objects_at(frame).iter())
                .find(|o| o.id == id)
                .map(|o| o.transform.translation().x)
                .expect("the shape")
        };

        assert_eq!(at(10), 100.0, "the edited frame did not change");
        assert_eq!(at(0), 0.0, "frame 0 was changed instead");
    }

    /// An object that is not on the keyframe owning the frame — because the
    /// playhead is somewhere else entirely — is still editable rather than
    /// mysteriously immovable.
    #[test]
    fn editing_falls_back_when_the_playhead_is_off_the_object() {
        let mut scene = Scene::default();
        let layer = scene.layers().iter().next().expect("a layer").id;
        let id = scene
            .add_shape(
                layer,
                ShapeData::filled(
                    kurbo::Rect::new(0.0, 0.0, 10.0, 10.0).to_path(1e-9),
                    Color::WHITE,
                ),
            )
            .expect("a shape");

        // Frame 500 is far beyond the one-frame span.
        assert!(scene.update_object_at(500, id, |o| o.visible = false));
        assert!(
            scene
                .layers()
                .iter()
                .flat_map(|l| l.objects_at(0).iter())
                .any(|o| o.id == id && !o.visible)
        );
    }

    /// Snapshots must not see later edits — the guarantee the whole
    /// copy-on-write model rests on.
    #[test]
    fn update_object_does_not_reach_into_an_existing_snapshot() {
        let (mut scene, _) = scene_with_shapes(3);
        let id = scene.layers().iter().next().unwrap().all_objects().next().unwrap().id;

        let snapshot = scene.clone();
        assert!(scene.update_object(id, |o| o.visible = false));

        let (_, before) = snapshot.find_object(id).expect("in the snapshot");
        let (_, after) = scene.find_object(id).expect("in the scene");
        assert!(before.visible, "the snapshot must be untouched");
        assert!(!after.visible, "the live scene must have changed");
    }
}
