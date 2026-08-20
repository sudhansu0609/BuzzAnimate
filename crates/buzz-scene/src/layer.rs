//! Layers, modelled on Adobe Animate.
//!
//! # Ordering
//!
//! Layers are stored **front to back**: index 0 is the topmost row in the
//! timeline and paints in front of everything below it. That matches what the
//! user sees, at the cost of one reversal at render time — see
//! [`LayerStack::paint_order`], which yields back to front so later entries
//! paint on top. Hit-testing uses the same convention (later = on top), so the
//! two agree by construction.
//!
//! # Masking is positional
//!
//! Animate does not store an explicit link from a mask to what it masks. A
//! [`LayerKind::Mask`] layer masks the run of [`LayerKind::Masked`] layers
//! immediately below it, stopping at the first layer that is not masked. The
//! same applies to [`LayerKind::Guide`] and [`LayerKind::Guided`]. Reproducing
//! that positional rule is what makes an imported `.fla` behave correctly, so
//! it is implemented here rather than invented as an explicit relationship.

use std::sync::Arc;

use buzz_geom::Affine;
use peniko::Color;
use serde::{Deserialize, Serialize};

use crate::object::{Object, ObjectId};
use crate::timeline::{FrameKind, LayerTimeline};

/// Stable identity for a layer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct LayerId(pub u64);

/// Animate's six layer types.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum LayerKind {
    /// An ordinary drawing layer.
    #[default]
    Normal,
    /// Holds other layers. Draws nothing itself.
    Folder,
    /// Its artwork defines what is visible on the masked layers beneath it.
    Mask,
    /// The same, inverted: its artwork defines what is **hidden**.
    ///
    /// Animate has no such layer — a hole is cut there by drawing the mask as
    /// a shape with a hole in it, which means redrawing the mask whenever the
    /// artwork under it moves. This is the same region used the other way
    /// round, and it is the honest way to do a spotlight in reverse: a
    /// character walking behind a foreground element, a scratch-off, smoke
    /// eating a title.
    InverseMask,
    /// Clipped by the nearest mask layer above.
    Masked,
    /// Reference geometry. Visible while authoring, never exported.
    Guide,
    /// Follows a motion guide on the guide layer above.
    Guided,
}

impl LayerKind {
    /// Does this layer paint its own artwork into the output?
    ///
    /// Folders have no artwork, and guides are authoring aids that Animate
    /// deliberately excludes from published output.
    pub fn paints_to_output(self) -> bool {
        matches!(
            self,
            Self::Normal | Self::Mask | Self::InverseMask | Self::Masked | Self::Guided
        )
    }

    /// Does this layer clip the run of masked layers below it?
    ///
    /// Both kinds of mask do; they differ only in which side of the region
    /// survives. Every positional rule reads this rather than naming the two
    /// kinds, so a third would not have to be chased through the code.
    pub fn is_mask(self) -> bool {
        matches!(self, Self::Mask | Self::InverseMask)
    }

    /// Does the mask hide what it covers rather than reveal it?
    pub fn is_inverted_mask(self) -> bool {
        matches!(self, Self::InverseMask)
    }

    /// Is this layer visible on the stage while authoring?
    ///
    /// Guides are: that is their whole purpose.
    pub fn paints_on_stage(self) -> bool {
        !matches!(self, Self::Folder)
    }

    /// Can this layer hold artwork at all?
    pub fn holds_artwork(self) -> bool {
        !matches!(self, Self::Folder)
    }

    /// Name shown in the layer properties dialog.
    pub fn display_name(self) -> &'static str {
        match self {
            Self::Normal => "Normal",
            Self::Folder => "Folder",
            Self::Mask => "Mask",
            Self::InverseMask => "Inverse Mask",
            Self::Masked => "Masked",
            Self::Guide => "Guide",
            Self::Guided => "Guided",
        }
    }
}

/// Row height in the timeline, as Animate offers it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum LayerHeight {
    #[default]
    Normal,
    Double,
    Triple,
}

impl LayerHeight {
    pub fn percent(self) -> u16 {
        match self {
            Self::Normal => 100,
            Self::Double => 200,
            Self::Triple => 300,
        }
    }
}

/// A single layer.
#[derive(Debug, Clone, PartialEq)]
pub struct Layer {
    pub id: LayerId,
    pub name: String,
    pub kind: LayerKind,
    /// Enclosing folder, if any.
    ///
    /// This is timeline *nesting* — the folder the layer is filed under. It is
    /// not [`Self::follows`], which is a different relationship entirely.
    pub parent: Option<LayerId>,

    /// The layer this one **follows**: Animate's Layer Parenting.
    ///
    /// When the followed layer's artwork moves, this layer's artwork moves with
    /// it — a head layer parented to a body layer, an arm to a shoulder. It is
    /// how a character is rigged without bones, and it is what Animate's Parent
    /// column in the timeline sets.
    ///
    /// Deliberately separate from [`Self::parent`]: a layer can be filed in a
    /// folder and follow a layer in a different one, and Animate keeps the two
    /// apart for exactly that reason.
    pub follows: Option<LayerId>,

    /// This layer's own transform **at the moment it became a rig parent**.
    ///
    /// Layer parenting propagates a parent's motion *away from its rest pose*,
    /// so the whole feature turns on what "rest" means. It used to mean "this
    /// layer's first keyframe" — which is fine for a character that is already
    /// animated and useless for one that is not: with a single keyframe, now
    /// *is* rest, the motion is always the identity, and moving a wrist left
    /// its palm behind. Parenting did nothing until you had animated, which is
    /// the wrong way round, because parenting is what you set up *before* you
    /// animate.
    ///
    /// Recorded when a link is made, so it stays put while the artwork moves.
    /// `None` on a layer nothing follows, and on documents written before this
    /// existed — those fall back to the first keyframe, which is what they were
    /// saved expecting.
    pub rest_pose: Option<Affine>,

    /// The eye column.
    pub visible: bool,
    /// The padlock column. Locked layers cannot be selected or edited.
    pub locked: bool,
    /// The outline column: draw artwork as outlines in the layer colour.
    pub outline: bool,
    /// How solid this layer is drawn **while working**, `0.0`–`1.0`.
    ///
    /// Animate's layer transparency, and like Animate's it is an authoring aid
    /// rather than a property of the film: the export draws every layer at full
    /// strength. That is the whole use of it — dimming a reference layer to
    /// draw over, or fading the foreground to see what is behind it, without
    /// changing a frame of what is delivered. A layer meant to be genuinely
    /// see-through in the film wants an alpha on its artwork instead.
    ///
    /// Hiding a layer is a different thing and stays a different thing:
    /// [`Self::visible`] takes it out of the export too.
    pub alpha: f64,
    /// Tint used for outline view and selection highlights.
    pub color: Color,
    pub height: LayerHeight,
    /// How far this layer sits from the camera, in document units.
    ///
    /// Zero is the focal plane, where artwork renders at its natural size.
    /// Positive is further away: smaller, and slower to slide as the camera
    /// pans. Negative is nearer: larger, and faster. This is Animate's Layer
    /// Depth, and it is what produces parallax.
    ///
    /// Depth does **not** reorder drawing by default. Paint order is the layer
    /// order in the timeline, exactly as in Animate — a layer pushed into the
    /// distance keeps its place in the stack, so pushing a foreground layer
    /// back shrinks it without sending it behind anything. The stage's opt-in
    /// `sort_by_depth` changes that, ordering layers by this value instead; see
    /// [`LayerStack::depth_paint_order`].
    pub depth: f64,
    /// Folders only: whether children are hidden in the timeline. Purely a UI
    /// state — it never affects rendering.
    pub collapsed: bool,

    /// Filters applied to everything on this layer, as one subject.
    ///
    /// **Animate has no layer filters** — there, filters belong to a movie clip
    /// instance, and blurring a whole layer means selecting it all and
    /// converting it to a symbol first. These are geometry rather than a
    /// cached raster surface (see `buzz-fx`), so a layer costs no more than an
    /// object does, and "blur the background" is a thing animators ask for
    /// constantly. Recorded as a deviation, with the reason.
    pub filters: Vec<buzz_fx::Filter>,

    /// The layer's frames. Artwork lives in keyframes, not on the layer.
    pub frames: LayerTimeline,
}

/// Animate cycles through these when creating layers.
/// The colour a layer gets when it is the `index`-th in the document.
///
/// Indexed by *position*, not by id. Ids are shared with objects, so a
/// document with seven shapes per layer strides the ids by eight and hands
/// every layer the same colour — which defeats the point of a colour meant to
/// tell layers apart, in the timeline chips and in the Layer Depth view alike.
pub fn default_color(index: usize) -> Color {
    DEFAULT_COLORS[index % DEFAULT_COLORS.len()]
}

const DEFAULT_COLORS: [Color; 8] = [
    Color::from_rgb8(0x00, 0x99, 0xFF),
    Color::from_rgb8(0xFF, 0x66, 0x00),
    Color::from_rgb8(0x00, 0xCC, 0x66),
    Color::from_rgb8(0xCC, 0x33, 0xCC),
    Color::from_rgb8(0xFF, 0xCC, 0x00),
    Color::from_rgb8(0x00, 0xCC, 0xCC),
    Color::from_rgb8(0xFF, 0x33, 0x66),
    Color::from_rgb8(0x99, 0x66, 0xFF),
];

impl Layer {
    pub fn new(id: LayerId, name: impl Into<String>, kind: LayerKind) -> Self {
        Self {
            color: DEFAULT_COLORS[(id.0 as usize) % DEFAULT_COLORS.len()],
            id,
            name: name.into(),
            kind,
            parent: None,
            follows: None,
            rest_pose: None,
            visible: true,
            locked: false,
            outline: false,
            alpha: 1.0,
            height: LayerHeight::Normal,
            // On the focal plane, so a new layer is unaffected by perspective.
            depth: 0.0,
            collapsed: false,
            filters: Vec::new(),
            frames: LayerTimeline::new(),
        }
    }

    pub fn normal(id: LayerId, name: impl Into<String>) -> Self {
        Self::new(id, name, LayerKind::Normal)
    }

    /// Can the user select or modify anything on this layer?
    ///
    /// Animate requires a layer to be both visible and unlocked. A hidden layer
    /// is not clickable even though its objects still exist.
    pub fn is_editable(&self) -> bool {
        self.visible && !self.locked && self.kind.holds_artwork()
    }

    /// Should this layer's artwork be drawn at `frame`?
    pub fn is_drawable_at(&self, frame: u32) -> bool {
        self.visible && self.kind.paints_on_stage() && !self.objects_at(frame).is_empty()
    }

    /// Artwork shown at `frame`, in paint order.
    pub fn objects_at(&self, frame: u32) -> &[Arc<Object>] {
        self.frames.objects_at(frame)
    }

    /// What the timeline should draw for `frame`.
    pub fn frame_kind(&self, frame: u32) -> FrameKind {
        self.frames.frame_kind(frame)
    }

    /// How many frames this layer occupies.
    pub fn length(&self) -> u32 {
        self.frames.length()
    }

    /// Add an object to the keyframe governing `frame`.
    pub fn push_object_at(&mut self, frame: u32, object: Arc<Object>) -> bool {
        self.frames.push_object(frame, object)
    }

    /// Remove an object from wherever on this layer it appears.
    pub fn remove_object(&mut self, id: ObjectId) -> Option<Arc<Object>> {
        self.frames.remove_object(id)
    }

    /// Find an object anywhere on this layer, across all keyframes.
    pub fn find_object(&self, id: ObjectId) -> Option<&Arc<Object>> {
        self.frames.all_objects().find(|o| o.id == id)
    }

    /// Every object on the layer, across all keyframes.
    pub fn all_objects(&self) -> impl Iterator<Item = &Arc<Object>> {
        self.frames.all_objects()
    }

    /// Bounds of the artwork shown at `frame`.
    pub fn bounds_at(&self, frame: u32) -> Option<buzz_geom::Rect> {
        self.objects_at(frame)
            .iter()
            .map(|o| o.bounds())
            .reduce(|a, b| a.union(b))
    }

    /// Bounds of everything on the layer, across every frame.
    pub fn bounds(&self) -> Option<buzz_geom::Rect> {
        self.all_objects()
            .map(|o| o.bounds())
            .reduce(|a, b| a.union(b))
    }
}

/// A mask and the layers it clips.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MaskGroup {
    pub mask: LayerId,
    /// Clipped layers, front to back, as stored.
    pub masked: Vec<LayerId>,
    /// The mask hides what it covers instead of revealing it.
    pub inverted: bool,
}

/// The ordered stack of layers in a scene or symbol timeline.
///
/// Front to back: index 0 is the top row in the timeline.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct LayerStack {
    layers: Arc<Vec<Arc<Layer>>>,
}

impl LayerStack {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn len(&self) -> usize {
        self.layers.len()
    }

    pub fn is_empty(&self) -> bool {
        self.layers.is_empty()
    }

    /// Front to back, as shown in the timeline.
    pub fn iter(&self) -> impl Iterator<Item = &Arc<Layer>> {
        self.layers.iter()
    }

    /// Back to front, the order artwork must be painted in.
    ///
    /// Later entries paint on top, which is the same convention
    /// `buzz_geom::hit` uses, so a hit test built from this order agrees with
    /// what the user sees.
    pub fn paint_order(&self) -> impl Iterator<Item = &Arc<Layer>> {
        self.layers.iter().rev()
    }

    pub fn get(&self, id: LayerId) -> Option<&Arc<Layer>> {
        self.layers.iter().find(|l| l.id == id)
    }

    pub fn index_of(&self, id: LayerId) -> Option<usize> {
        self.layers.iter().position(|l| l.id == id)
    }

    /// Insert at the front (top of the timeline), where Animate puts new layers.
    pub fn push_front(&mut self, layer: Layer) {
        Arc::make_mut(&mut self.layers).insert(0, Arc::new(layer));
    }

    /// Insert at a specific row.
    pub fn insert(&mut self, index: usize, layer: Layer) {
        let layers = Arc::make_mut(&mut self.layers);
        let index = index.min(layers.len());
        layers.insert(index, Arc::new(layer));
    }

    pub fn remove(&mut self, id: LayerId) -> Option<Arc<Layer>> {
        let layers = Arc::make_mut(&mut self.layers);
        let index = layers.iter().position(|l| l.id == id)?;
        Some(layers.remove(index))
    }

    /// Move a layer to a new row, as dragging it in the timeline would.
    pub fn reorder(&mut self, id: LayerId, to_index: usize) -> bool {
        let layers = Arc::make_mut(&mut self.layers);
        let Some(from) = layers.iter().position(|l| l.id == id) else {
            return false;
        };
        let to = to_index.min(layers.len().saturating_sub(1));
        if from == to {
            return true;
        }
        let layer = layers.remove(from);
        layers.insert(to, layer);
        true
    }

    /// Edit one layer in place, cloning it only if another snapshot shares it.
    pub fn update(&mut self, id: LayerId, f: impl FnOnce(&mut Layer)) -> bool {
        let layers = Arc::make_mut(&mut self.layers);
        let Some(slot) = layers.iter_mut().find(|l| l.id == id) else {
            return false;
        };
        f(Arc::make_mut(slot));
        true
    }

    /// Direct children of a folder, front to back.
    pub fn children_of(&self, folder: LayerId) -> Vec<LayerId> {
        self.layers
            .iter()
            .filter(|l| l.parent == Some(folder))
            .map(|l| l.id)
            .collect()
    }

    /// Is this layer inside a collapsed or hidden folder?
    ///
    /// Animate hides a folder's contents on the stage when the folder itself is
    /// hidden, so visibility has to be resolved through ancestors rather than
    /// read off the layer alone.
    pub fn is_effectively_visible(&self, id: LayerId) -> bool {
        let Some(mut layer) = self.get(id) else {
            return false;
        };
        if !layer.visible {
            return false;
        }
        // Bounded by the layer count, so a corrupt parent cycle cannot hang.
        for _ in 0..self.layers.len() {
            let Some(parent_id) = layer.parent else {
                return true;
            };
            let Some(parent) = self.get(parent_id) else {
                return true;
            };
            if !parent.visible {
                return false;
            }
            layer = parent;
        }
        true
    }

    // -- layer parenting ------------------------------------------------------

    /// What a layer's artwork inherits from the layer it follows.
    ///
    /// Animate's Layer Parenting: move the body and the head goes with it. The
    /// inherited transform is the **motion** of every layer above this one in
    /// the follow chain — how far each has travelled from its own first
    /// keyframe — composed outermost first.
    ///
    /// Motion, not position, is what is inherited. A head drawn in the right
    /// place must stay in the right place the moment it is parented; inheriting
    /// the body's absolute transform would fling it across the stage as soon as
    /// the link was made, which is not what parenting means to an animator.
    pub fn inherited_transform(&self, id: LayerId, frame: u32) -> Affine {
        let mut chain = Vec::new();
        let mut current = self.get(id).and_then(|l| l.follows);
        // Bounded by the layer count: a corrupt file can hold a follow cycle,
        // and this must terminate rather than hang the renderer.
        for _ in 0..self.layers.len() {
            let Some(next) = current else { break };
            if chain.contains(&next) {
                break;
            }
            chain.push(next);
            current = self.get(next).and_then(|l| l.follows);
        }

        // Outermost first: the grandparent's motion applies to the parent's,
        // and both apply to this layer.
        let mut out = Affine::IDENTITY;
        for followed in chain.iter().rev() {
            out *= self.motion_of(*followed, frame);
        }
        out
    }

    /// How far a layer's artwork has moved from where it started.
    ///
    /// # Which transform is "the layer's"
    ///
    /// A layer has no transform of its own — its objects do. This takes the
    /// **first object on the layer** as the thing that represents it, because
    /// layer parenting is used with one symbol per layer: that is how a rig is
    /// built in Animate, and the Parent column exists to serve it. A layer
    /// holding loose artwork can still be followed; it is the first object that
    /// leads.
    ///
    /// Recorded as a deviation rather than hidden: Animate tracks a
    /// transformation for the layer itself.
    pub fn motion_of(&self, id: LayerId, frame: u32) -> Affine {
        let Some(layer) = self.get(id) else {
            return Affine::IDENTITY;
        };
        let anchor = |at: u32| {
            layer
                .frames
                .resolved_at(at)
                .iter()
                .next()
                .map(|object| object.transform)
        };
        let Some(now) = anchor(frame) else {
            return Affine::IDENTITY;
        };
        // The pose the link was made at, when there is one. Falling back to the
        // first keyframe keeps documents written before rest poses existed
        // behaving as they did — and those are exactly the documents whose
        // parenting only ever showed up once they were animated.
        let rest = match layer.rest_pose {
            Some(recorded) => recorded,
            None => {
                let Some(rest_frame) = layer.frames.keyframes().first().map(|k| k.start) else {
                    return Affine::IDENTITY;
                };
                let Some(rest) = anchor(rest_frame) else {
                    return Affine::IDENTITY;
                };
                rest
            }
        };

        // A rest pose scaled to nothing has no inverse; a layer like that
        // simply passes nothing on, rather than scattering its children.
        let c = rest.as_coeffs();
        let determinant = c[0] * c[3] - c[1] * c[2];
        if determinant.abs() < 1e-12 {
            return Affine::IDENTITY;
        }
        now * rest.inverse()
    }

    /// May `layer` follow `target` without making a cycle?
    ///
    /// A cycle would be a layer that follows itself through some chain, and the
    /// renderer would have nothing sensible to draw. Refusing the link is
    /// friendlier than resolving it arbitrarily.
    pub fn can_follow(&self, layer: LayerId, target: LayerId) -> bool {
        if layer == target || self.get(target).is_none() {
            return false;
        }
        // Walk up from the target: if this layer is already above it, linking
        // would close the loop.
        let mut current = Some(target);
        for _ in 0..self.layers.len() {
            let Some(id) = current else { return true };
            if id == layer {
                return false;
            }
            current = self.get(id).and_then(|l| l.follows);
        }
        false
    }

    /// Every layer that follows `id`, directly.
    pub fn followers_of(&self, id: LayerId) -> Vec<LayerId> {
        self.layers
            .iter()
            .filter(|l| l.follows == Some(id))
            .map(|l| l.id)
            .collect()
    }

    /// Locked directly, or inside a locked folder.
    pub fn is_effectively_locked(&self, id: LayerId) -> bool {
        let Some(mut layer) = self.get(id) else {
            return true;
        };
        if layer.locked {
            return true;
        }
        for _ in 0..self.layers.len() {
            let Some(parent_id) = layer.parent else {
                return false;
            };
            let Some(parent) = self.get(parent_id) else {
                return false;
            };
            if parent.locked {
                return true;
            }
            layer = parent;
        }
        false
    }

    /// Resolve which layers each mask clips.
    ///
    /// A mask claims the unbroken run of `Masked` layers directly beneath it.
    /// The run stops at the first layer that is not `Masked`, which is exactly
    /// Animate's rule and what `.fla` files rely on.
    pub fn mask_groups(&self) -> Vec<MaskGroup> {
        let mut groups = Vec::new();
        for (i, layer) in self.layers.iter().enumerate() {
            if !layer.kind.is_mask() {
                continue;
            }
            let mut masked = Vec::new();
            for below in self.layers.iter().skip(i + 1) {
                if below.kind != LayerKind::Masked {
                    break;
                }
                masked.push(below.id);
            }
            groups.push(MaskGroup {
                mask: layer.id,
                masked,
                inverted: layer.kind.is_inverted_mask(),
            });
        }
        groups
    }

    /// Which mask, if any, clips this layer.
    pub fn mask_for(&self, id: LayerId) -> Option<LayerId> {
        self.mask_groups()
            .into_iter()
            .find(|g| g.masked.contains(&id))
            .map(|g| g.mask)
    }

    /// The guide layer a guided layer follows, by the same positional rule.
    pub fn guide_for(&self, id: LayerId) -> Option<LayerId> {
        let index = self.index_of(id)?;
        if self.layers[index].kind != LayerKind::Guided {
            return None;
        }
        // Walk upwards past sibling guided layers to the guide above them.
        for candidate in self.layers[..index].iter().rev() {
            match candidate.kind {
                LayerKind::Guided => continue,
                LayerKind::Guide => return Some(candidate.id),
                _ => return None,
            }
        }
        None
    }

    /// Layers whose artwork should be drawn at `frame`, back to front.
    pub fn drawable_at(&self, frame: u32) -> impl Iterator<Item = &Arc<Layer>> {
        self.paint_order()
            .filter(move |l| l.is_drawable_at(frame) && self.is_effectively_visible(l.id))
    }

    /// Back to front, ordered by layer depth rather than by the timeline.
    ///
    /// Furthest from the camera (largest depth) is painted first. A mask and its
    /// run of masked layers move as **one unit** and keep their relative order,
    /// so the mask still owns an unbroken run — the invariant the render walk
    /// relies on. The sort is **stable**, so layers at equal depth keep the
    /// timeline's order; with every depth equal the result is exactly
    /// [`Self::paint_order`], which is what makes the feature free for any
    /// document that leaves depth alone.
    pub fn depth_paint_order(&self) -> Vec<&Arc<Layer>> {
        // Units in timeline (front-to-back) order, each tagged with the depth it
        // sorts by — the mask's, for a mask group.
        let mut units: Vec<(f64, Vec<&Arc<Layer>>)> = Vec::new();
        let mut i = 0;
        while i < self.layers.len() {
            let layer = &self.layers[i];
            if layer.kind.is_mask() {
                let mut unit = vec![layer];
                let mut j = i + 1;
                while j < self.layers.len() && self.layers[j].kind == LayerKind::Masked {
                    unit.push(&self.layers[j]);
                    j += 1;
                }
                units.push((layer.depth, unit));
                i = j;
            } else {
                units.push((layer.depth, vec![layer]));
                i += 1;
            }
        }

        // Reverse to paint order (back to front), inside units and out, so the
        // pre-sort sequence is exactly `paint_order()`. Then a stable sort by
        // depth descending puts the furthest unit first and leaves equal depths
        // in that paint order — byte-identical output when nothing uses depth.
        units.reverse();
        for (_, unit) in &mut units {
            unit.reverse();
        }
        units.sort_by(|a, b| b.0.total_cmp(&a.0));
        units.into_iter().flat_map(|(_, unit)| unit).collect()
    }

    /// Drawable layers at `frame`, depth-ordered. The depth-sorted sibling of
    /// [`Self::drawable_at`], used when the stage's `sort_by_depth` is on.
    pub fn drawable_at_by_depth(&self, frame: u32) -> Vec<&Arc<Layer>> {
        self.depth_paint_order()
            .into_iter()
            .filter(|l| l.is_drawable_at(frame) && self.is_effectively_visible(l.id))
            .collect()
    }

    /// Longest layer, which is the document's frame count.
    pub fn frame_count(&self) -> u32 {
        self.layers.iter().map(|l| l.length()).max().unwrap_or(1)
    }

    /// Layers the user can currently select on.
    pub fn selectable(&self) -> impl Iterator<Item = &Arc<Layer>> {
        self.paint_order().filter(|l| {
            l.is_editable()
                && self.is_effectively_visible(l.id)
                && !self.is_effectively_locked(l.id)
        })
    }

    /// Layers the user can select on, **in the order they are painted**.
    ///
    /// `by_depth` mirrors the stage's `sort_by_depth`. A hit test walks these
    /// back to front and keeps the last match, so it only finds what is
    /// actually on top if it walks them in the order they were drawn. With
    /// depth sorting on, [`Self::selectable`] walks the timeline's order
    /// instead — so a layer pushed to the back of the shot but sitting high in
    /// the timeline won the click, and the artwork visibly in front of it could
    /// not be selected at all.
    ///
    /// With depth sorting off, or with every depth equal, this is exactly
    /// [`Self::selectable`].
    pub fn selectable_in_paint_order(&self, by_depth: bool) -> Vec<&Arc<Layer>> {
        if !by_depth {
            return self.selectable().collect();
        }
        self.depth_paint_order()
            .into_iter()
            .filter(|l| {
                l.is_editable()
                    && self.is_effectively_visible(l.id)
                    && !self.is_effectively_locked(l.id)
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stack(kinds: &[(u64, &str, LayerKind)]) -> LayerStack {
        let mut s = LayerStack::new();
        // Build bottom-up so the listed order ends up as the visual order.
        for (id, name, kind) in kinds.iter().rev() {
            s.push_front(Layer::new(LayerId(*id), *name, *kind));
        }
        s
    }

    #[test]
    fn new_layers_go_to_the_top_like_animate() {
        let mut s = LayerStack::new();
        s.push_front(Layer::normal(LayerId(1), "Layer 1"));
        s.push_front(Layer::normal(LayerId(2), "Layer 2"));

        assert_eq!(s.index_of(LayerId(2)), Some(0), "newest should be on top");
        assert_eq!(s.index_of(LayerId(1)), Some(1));
    }

    #[test]
    fn paint_order_is_back_to_front() {
        let s = stack(&[
            (1, "top", LayerKind::Normal),
            (2, "middle", LayerKind::Normal),
            (3, "bottom", LayerKind::Normal),
        ]);
        let painted: Vec<u64> = s.paint_order().map(|l| l.id.0).collect();
        assert_eq!(
            painted,
            vec![3, 2, 1],
            "the bottom layer must paint first so the top layer lands on top"
        );
    }

    /// With every depth equal, the depth order is exactly the timeline order —
    /// the guarantee that makes the feature free for documents that ignore it.
    #[test]
    fn depth_order_with_equal_depths_matches_paint_order() {
        let s = stack(&[
            (1, "top", LayerKind::Normal),
            (2, "middle", LayerKind::Normal),
            (3, "bottom", LayerKind::Normal),
        ]);
        let plain: Vec<u64> = s.paint_order().map(|l| l.id.0).collect();
        let by_depth: Vec<u64> = s.depth_paint_order().iter().map(|l| l.id.0).collect();
        assert_eq!(by_depth, plain);
    }

    /// A layer pushed into the distance paints behind a nearer one, whatever the
    /// timeline says.
    #[test]
    fn depth_order_puts_the_furthest_layer_first() {
        let mut s = stack(&[
            (1, "near-on-top", LayerKind::Normal),
            (2, "far-below", LayerKind::Normal),
        ]);
        // Make the top layer nearer (small depth) and the bottom one far.
        s.update(LayerId(1), |l| l.depth = -100.0);
        s.update(LayerId(2), |l| l.depth = 500.0);

        let by_depth: Vec<u64> = s.depth_paint_order().iter().map(|l| l.id.0).collect();
        // Furthest (id 2, depth 500) paints first; nearest (id 1) last, on top.
        assert_eq!(by_depth, vec![2, 1]);
    }

    /// A mask and its masked run move together, keeping the run unbroken.
    #[test]
    fn depth_order_keeps_a_mask_with_its_run() {
        let mut s = stack(&[
            (1, "faraway", LayerKind::Normal),
            (2, "mask", LayerKind::Mask),
            (3, "masked", LayerKind::Masked),
        ]);
        // Push the plain layer far away so it must sort behind the mask group,
        // and give the mask group a nearer depth.
        s.update(LayerId(1), |l| l.depth = 900.0);
        s.update(LayerId(2), |l| l.depth = 0.0);

        let ids: Vec<u64> = s.depth_paint_order().iter().map(|l| l.id.0).collect();
        // The far plain layer paints first; then the mask group, mask and its
        // masked layer still adjacent.
        assert_eq!(ids, vec![1, 3, 2]);
        let mask_pos = ids.iter().position(|&i| i == 2).unwrap();
        let masked_pos = ids.iter().position(|&i| i == 3).unwrap();
        assert_eq!(
            mask_pos.abs_diff(masked_pos),
            1,
            "the mask and its masked layer must stay adjacent"
        );
    }

    #[test]
    fn only_folders_are_excluded_from_stage_painting() {
        assert!(LayerKind::Normal.paints_on_stage());
        assert!(
            LayerKind::Guide.paints_on_stage(),
            "guides show while authoring"
        );
        assert!(!LayerKind::Folder.paints_on_stage());
    }

    #[test]
    fn guides_are_never_exported() {
        assert!(LayerKind::Normal.paints_to_output());
        assert!(LayerKind::Mask.paints_to_output());
        assert!(LayerKind::Masked.paints_to_output());
        assert!(LayerKind::Guided.paints_to_output());
        assert!(
            !LayerKind::Guide.paints_to_output(),
            "guides are authoring aids"
        );
        assert!(!LayerKind::Folder.paints_to_output());
    }

    /// Animate's positional rule, which imported `.fla` files depend on.
    #[test]
    fn a_mask_claims_the_run_of_masked_layers_below_it() {
        let s = stack(&[
            (1, "Mask", LayerKind::Mask),
            (2, "Masked A", LayerKind::Masked),
            (3, "Masked B", LayerKind::Masked),
            (4, "Normal", LayerKind::Normal),
            (5, "Orphan masked", LayerKind::Masked),
        ]);

        let groups = s.mask_groups();
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].mask, LayerId(1));
        assert_eq!(
            groups[0].masked,
            vec![LayerId(2), LayerId(3)],
            "the run must stop at the first non-masked layer"
        );

        assert_eq!(s.mask_for(LayerId(2)), Some(LayerId(1)));
        assert_eq!(
            s.mask_for(LayerId(5)),
            None,
            "a masked layer below a normal layer has no mask"
        );
    }

    #[test]
    fn multiple_masks_resolve_independently() {
        let s = stack(&[
            (1, "Mask A", LayerKind::Mask),
            (2, "Masked A", LayerKind::Masked),
            (3, "Mask B", LayerKind::Mask),
            (4, "Masked B", LayerKind::Masked),
        ]);
        let groups = s.mask_groups();
        assert_eq!(groups.len(), 2);
        assert_eq!(groups[0].masked, vec![LayerId(2)]);
        assert_eq!(groups[1].masked, vec![LayerId(4)]);
    }

    /// An inverse mask claims its run by exactly the same positional rule,
    /// and says which way round it works.
    #[test]
    fn an_inverse_mask_claims_the_run_below_it_and_is_marked_inverted() {
        let s = stack(&[
            (1, "Hole", LayerKind::InverseMask),
            (2, "Masked A", LayerKind::Masked),
            (3, "Masked B", LayerKind::Masked),
            (4, "Normal", LayerKind::Normal),
        ]);
        let groups = s.mask_groups();
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].masked, vec![LayerId(2), LayerId(3)]);
        assert!(groups[0].inverted, "the group must know it is inverted");
        assert_eq!(s.mask_for(LayerId(2)), Some(LayerId(1)));
    }

    /// And an ordinary mask is not marked inverted, which is the half of the
    /// pair that a wrong default would break silently.
    #[test]
    fn an_ordinary_mask_is_not_inverted() {
        let s = stack(&[
            (1, "Mask", LayerKind::Mask),
            (2, "Masked", LayerKind::Masked),
        ]);
        assert!(!s.mask_groups()[0].inverted);
    }

    #[test]
    fn a_mask_with_nothing_beneath_it_has_an_empty_group() {
        let s = stack(&[(1, "Mask", LayerKind::Mask)]);
        let groups = s.mask_groups();
        assert_eq!(groups.len(), 1);
        assert!(groups[0].masked.is_empty());
    }

    #[test]
    fn guided_layers_find_the_guide_above_them() {
        let s = stack(&[
            (1, "Guide", LayerKind::Guide),
            (2, "Guided A", LayerKind::Guided),
            (3, "Guided B", LayerKind::Guided),
            (4, "Normal", LayerKind::Normal),
        ]);
        assert_eq!(s.guide_for(LayerId(2)), Some(LayerId(1)));
        assert_eq!(
            s.guide_for(LayerId(3)),
            Some(LayerId(1)),
            "a second guided layer follows the same guide"
        );
        assert_eq!(s.guide_for(LayerId(4)), None);
    }

    #[test]
    fn hiding_a_folder_hides_its_children() {
        let mut s = LayerStack::new();
        s.push_front(Layer::new(LayerId(1), "Folder", LayerKind::Folder));
        let mut child = Layer::normal(LayerId(2), "Inside");
        child.parent = Some(LayerId(1));
        s.insert(1, child);

        assert!(s.is_effectively_visible(LayerId(2)));
        s.update(LayerId(1), |l| l.visible = false);
        assert!(
            !s.is_effectively_visible(LayerId(2)),
            "a child of a hidden folder must be hidden"
        );
    }

    #[test]
    fn locking_a_folder_locks_its_children() {
        let mut s = LayerStack::new();
        s.push_front(Layer::new(LayerId(1), "Folder", LayerKind::Folder));
        let mut child = Layer::normal(LayerId(2), "Inside");
        child.parent = Some(LayerId(1));
        s.insert(1, child);

        assert!(!s.is_effectively_locked(LayerId(2)));
        s.update(LayerId(1), |l| l.locked = true);
        assert!(s.is_effectively_locked(LayerId(2)));
    }

    /// A corrupt file could contain a parent cycle; resolution must terminate.
    #[test]
    fn a_parent_cycle_does_not_hang() {
        let mut s = LayerStack::new();
        s.push_front(Layer::new(LayerId(1), "A", LayerKind::Folder));
        s.push_front(Layer::new(LayerId(2), "B", LayerKind::Folder));
        s.update(LayerId(1), |l| l.parent = Some(LayerId(2)));
        s.update(LayerId(2), |l| l.parent = Some(LayerId(1)));

        // The result is arbitrary; not hanging is the point.
        let _ = s.is_effectively_visible(LayerId(1));
        let _ = s.is_effectively_locked(LayerId(2));
    }

    #[test]
    fn locked_and_hidden_layers_are_not_selectable() {
        let mut s = stack(&[
            (1, "A", LayerKind::Normal),
            (2, "B", LayerKind::Normal),
            (3, "C", LayerKind::Normal),
        ]);
        s.update(LayerId(1), |l| l.locked = true);
        s.update(LayerId(2), |l| l.visible = false);

        let selectable: Vec<u64> = s.selectable().map(|l| l.id.0).collect();
        assert_eq!(selectable, vec![3]);
    }

    #[test]
    fn reordering_moves_a_layer_to_the_requested_row() {
        let mut s = stack(&[
            (1, "A", LayerKind::Normal),
            (2, "B", LayerKind::Normal),
            (3, "C", LayerKind::Normal),
        ]);
        assert!(s.reorder(LayerId(3), 0));
        let order: Vec<u64> = s.iter().map(|l| l.id.0).collect();
        assert_eq!(order, vec![3, 1, 2]);

        // Out-of-range clamps rather than panicking.
        assert!(s.reorder(LayerId(3), 99));
        assert_eq!(s.iter().last().unwrap().id, LayerId(3));
        assert!(
            !s.reorder(LayerId(404), 0),
            "unknown layer should report failure"
        );
    }

    #[test]
    fn folder_children_are_listed() {
        let mut s = LayerStack::new();
        s.push_front(Layer::new(LayerId(1), "Folder", LayerKind::Folder));
        for id in [2u64, 3] {
            let mut child = Layer::normal(LayerId(id), format!("Child {id}"));
            child.parent = Some(LayerId(1));
            s.insert(1, child);
        }
        let mut children = s.children_of(LayerId(1));
        children.sort();
        assert_eq!(children, vec![LayerId(2), LayerId(3)]);
    }

    #[test]
    fn layer_heights_match_animates_options() {
        assert_eq!(LayerHeight::Normal.percent(), 100);
        assert_eq!(LayerHeight::Double.percent(), 200);
        assert_eq!(LayerHeight::Triple.percent(), 300);
    }

    /// Editing one layer must not deep-copy the others.
    #[test]
    fn updating_one_layer_leaves_the_rest_shared() {
        let mut a = stack(&[(1, "A", LayerKind::Normal), (2, "B", LayerKind::Normal)]);
        let snapshot = a.clone();

        let untouched_before = Arc::as_ptr(snapshot.get(LayerId(2)).unwrap());
        a.update(LayerId(1), |l| l.name = "renamed".into());
        let untouched_after = Arc::as_ptr(a.get(LayerId(2)).unwrap());

        assert_eq!(
            untouched_before, untouched_after,
            "an unrelated layer should still be the same allocation"
        );
        assert_eq!(snapshot.get(LayerId(1)).unwrap().name, "A");
        assert_eq!(a.get(LayerId(1)).unwrap().name, "renamed");
    }
    // -- layer parenting -----------------------------------------------------

    /// A layer holding one square, keyed at frame 0 and moved by `(dx, dy)` at
    /// frame 10, with a classic tween between them.
    fn moving_layer(id: u64, name: &str, dx: f64, dy: f64) -> Layer {
        use crate::object::{Object, ObjectId, ShapeData};
        use buzz_geom::{Rect, Shape as _};

        let mut layer = Layer::normal(LayerId(id), name);
        let art = || {
            Arc::new(Object::shape(
                ObjectId(id * 100),
                ShapeData::filled(Rect::new(0.0, 0.0, 20.0, 20.0).to_path(1e-9), Color::BLACK),
            ))
        };
        layer.frames.set_objects(0, vec![art()]);
        layer.frames.insert_keyframe(10);
        layer.frames.set_objects(
            10,
            vec![Arc::new(
                Object::shape(
                    ObjectId(id * 100),
                    ShapeData::filled(Rect::new(0.0, 0.0, 20.0, 20.0).to_path(1e-9), Color::BLACK),
                )
                .with_transform(Affine::translate((dx, dy))),
            )],
        );
        layer
    }

    /// The point of the feature: move the body, and the head goes with it.
    #[test]
    fn a_following_layer_inherits_the_motion_of_the_one_it_follows() {
        let mut s = LayerStack::new();
        s.push_front(moving_layer(1, "Body", 60.0, 0.0));
        s.push_front(Layer::normal(LayerId(2), "Head"));
        s.update(LayerId(2), |l| l.follows = Some(LayerId(1)));

        // At the first keyframe the body has not moved, so nothing is inherited
        // — a layer must not jump the moment it is parented.
        let at_rest = s.inherited_transform(LayerId(2), 0);
        assert_eq!(at_rest, Affine::IDENTITY);

        // At frame 10 the body has travelled 60 to the right, and so has the
        // head — without anything having been keyed on the head at all.
        let moved = s.inherited_transform(LayerId(2), 10);
        assert_eq!(moved, Affine::translate((60.0, 0.0)));
    }

    /// Motion accumulates down a chain: hand follows arm follows body.
    #[test]
    fn motion_accumulates_down_a_chain() {
        let mut s = LayerStack::new();
        s.push_front(moving_layer(1, "Body", 10.0, 0.0));
        s.push_front(moving_layer(2, "Arm", 0.0, 5.0));
        s.push_front(Layer::normal(LayerId(3), "Hand"));
        s.update(LayerId(2), |l| l.follows = Some(LayerId(1)));
        s.update(LayerId(3), |l| l.follows = Some(LayerId(2)));

        let hand = s.inherited_transform(LayerId(3), 10);
        assert_eq!(hand, Affine::translate((10.0, 5.0)));

        // The arm inherits only the body's motion; its own is already in its
        // artwork and must not be applied twice.
        assert_eq!(
            s.inherited_transform(LayerId(2), 10),
            Affine::translate((10.0, 0.0))
        );
    }

    /// A layer that follows nothing is untouched, which is what keeps every
    /// document that predates the feature drawing exactly as it did.
    #[test]
    fn a_layer_that_follows_nothing_inherits_nothing() {
        let s = stack(&[(1, "a", LayerKind::Normal), (2, "b", LayerKind::Normal)]);
        assert_eq!(s.inherited_transform(LayerId(1), 7), Affine::IDENTITY);
    }

    #[test]
    fn a_layer_cannot_follow_itself_or_close_a_loop() {
        let mut s = LayerStack::new();
        s.push_front(Layer::normal(LayerId(1), "a"));
        s.push_front(Layer::normal(LayerId(2), "b"));
        s.push_front(Layer::normal(LayerId(3), "c"));

        assert!(!s.can_follow(LayerId(1), LayerId(1)), "itself");
        assert!(s.can_follow(LayerId(2), LayerId(1)));
        s.update(LayerId(2), |l| l.follows = Some(LayerId(1)));
        s.update(LayerId(3), |l| l.follows = Some(LayerId(2)));

        // 1 following 3 would close 1 -> 3 -> 2 -> 1.
        assert!(!s.can_follow(LayerId(1), LayerId(3)));
        assert!(!s.can_follow(LayerId(1), LayerId(2)));
    }

    /// A corrupt file can hold a cycle anyway; resolving it must terminate.
    #[test]
    fn a_follow_cycle_does_not_hang() {
        let mut s = LayerStack::new();
        s.push_front(Layer::normal(LayerId(1), "a"));
        s.push_front(Layer::normal(LayerId(2), "b"));
        s.update(LayerId(1), |l| l.follows = Some(LayerId(2)));
        s.update(LayerId(2), |l| l.follows = Some(LayerId(1)));

        let _ = s.inherited_transform(LayerId(1), 3);
        let _ = s.inherited_transform(LayerId(2), 3);
    }

    /// Following a layer that has since been deleted is harmless.
    #[test]
    fn following_a_missing_layer_inherits_nothing() {
        let mut s = LayerStack::new();
        s.push_front(Layer::normal(LayerId(1), "a"));
        s.update(LayerId(1), |l| l.follows = Some(LayerId(99)));
        assert_eq!(s.inherited_transform(LayerId(1), 4), Affine::IDENTITY);
    }

    #[test]
    fn followers_are_found_so_a_deleted_layer_can_release_them() {
        let mut s = LayerStack::new();
        s.push_front(Layer::normal(LayerId(1), "body"));
        s.push_front(Layer::normal(LayerId(2), "head"));
        s.push_front(Layer::normal(LayerId(3), "hat"));
        s.update(LayerId(2), |l| l.follows = Some(LayerId(1)));
        s.update(LayerId(3), |l| l.follows = Some(LayerId(1)));

        let mut found = s.followers_of(LayerId(1));
        found.sort_by_key(|l| l.0);
        assert_eq!(found, vec![LayerId(2), LayerId(3)]);
    }
}
