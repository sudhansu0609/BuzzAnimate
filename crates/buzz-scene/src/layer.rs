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
        matches!(self, Self::Normal | Self::Mask | Self::Masked | Self::Guided)
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
    pub parent: Option<LayerId>,

    /// The eye column.
    pub visible: bool,
    /// The padlock column. Locked layers cannot be selected or edited.
    pub locked: bool,
    /// The outline column: draw artwork as outlines in the layer colour.
    pub outline: bool,
    /// Tint used for outline view and selection highlights.
    pub color: Color,
    pub height: LayerHeight,
    /// Folders only: whether children are hidden in the timeline. Purely a UI
    /// state — it never affects rendering.
    pub collapsed: bool,

    /// The layer's frames. Artwork lives in keyframes, not on the layer.
    pub frames: LayerTimeline,
}

/// Animate cycles through these when creating layers.
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
            visible: true,
            locked: false,
            outline: false,
            height: LayerHeight::Normal,
            collapsed: false,
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
            if layer.kind != LayerKind::Mask {
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

    /// Longest layer, which is the document's frame count.
    pub fn frame_count(&self) -> u32 {
        self.layers.iter().map(|l| l.length()).max().unwrap_or(1)
    }

    /// Layers the user can currently select on.
    pub fn selectable(&self) -> impl Iterator<Item = &Arc<Layer>> {
        self.paint_order().filter(|l| {
            l.is_editable() && self.is_effectively_visible(l.id) && !self.is_effectively_locked(l.id)
        })
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

    #[test]
    fn only_folders_are_excluded_from_stage_painting() {
        assert!(LayerKind::Normal.paints_on_stage());
        assert!(LayerKind::Guide.paints_on_stage(), "guides show while authoring");
        assert!(!LayerKind::Folder.paints_on_stage());
    }

    #[test]
    fn guides_are_never_exported() {
        assert!(LayerKind::Normal.paints_to_output());
        assert!(LayerKind::Mask.paints_to_output());
        assert!(LayerKind::Masked.paints_to_output());
        assert!(LayerKind::Guided.paints_to_output());
        assert!(!LayerKind::Guide.paints_to_output(), "guides are authoring aids");
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
        assert!(!s.reorder(LayerId(404), 0), "unknown layer should report failure");
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
        let mut a = stack(&[
            (1, "A", LayerKind::Normal),
            (2, "B", LayerKind::Normal),
        ]);
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
}
