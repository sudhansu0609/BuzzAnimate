//! **Group Empty Layers**, and keeping one layer pinned to the top.
//!
//! Group Empty Layers sorts each folder's own layers so the ones with artwork
//! (or sound) come first and the empty ones after, so a timeline's blank rows
//! sit together instead of scattered between the ones in use. It only
//! *reorders*, and only among siblings: nothing is deleted, nothing moves into
//! or out of a folder, a folder keeps everything filed in it, and the layers
//! with something on them keep their order — so the picture cannot change,
//! as an empty layer paints nothing. Unlike Arrange Layers, which deletes
//! empty layers and files the rest into new folders.
//!
//! A mask or guide layer keeps the masked or guided layers under it: they
//! move together, as one, filled if any of them is.

use std::collections::HashMap;
use std::sync::Arc;

use crate::layer::{Layer, LayerKind, LayerStack};
use crate::{LayerId, Scene};

impl Scene {
    /// **Group Empty Layers** on the timeline being edited: in every folder,
    /// and at the top level, the layers holding artwork or sound first and
    /// the empty ones after, each keeping its order. Returns how many rows
    /// moved; the revision is bumped only when one did.
    ///
    /// A light that sat above an empty layer is moved onto the first layer
    /// with artwork that was below it, so it lights the same artwork as
    /// before. A stack whose folders are not each followed by their own
    /// layers (not one the timeline makes) is left as it is.
    pub fn group_empty_layers(&mut self) -> usize {
        let in_symbol = self.editing_symbol().is_some_and(|id| self.library.get(id).is_some());
        let rows: Vec<Arc<Layer>> = self.active_layers_mut().iter().cloned().collect();
        let Some(order) = grouped_order(&rows) else {
            return 0;
        };
        let moved = order.iter().enumerate().filter(|(i, r)| *i != **r).count();
        if moved == 0 {
            return 0;
        }

        if !in_symbol {
            let index: HashMap<LayerId, usize> = rows.iter().enumerate().map(|(i, l)| (l.id, i)).collect();
            let holds = |l: &Layer| l.kind != LayerKind::Folder && !is_empty(l);
            for light in &mut self.lights.lights {
                let Some(&at) = light.above.and_then(|a| index.get(&LayerId(a))) else {
                    continue;
                };
                if holds(&rows[at]) {
                    continue;
                }
                if let Some(next) = rows[at..].iter().find(|l| holds(l)) {
                    light.above = Some(next.id.0);
                }
            }
        }

        let stack = LayerStack::from_rows(order.into_iter().map(|i| rows[i].clone()).collect());
        *self.active_layers_mut() = stack;
        self.bump();
        moved
    }

    /// Put a stage layer first in the stack, out of any folder: where the
    /// master audio is kept, just under the camera. Returns whether it moved.
    pub fn pin_layer_to_top(&mut self, id: LayerId) -> bool {
        let at_top = self.layers.index_of(id) == Some(0) && self.layers.get(id).is_some_and(|l| l.parent.is_none());
        if at_top || self.layers.get(id).is_none() {
            return false;
        }
        let moved = self.layers.move_to(id, 0, None);
        if moved {
            self.bump();
        }
        moved
    }
}

/// No artwork and no sound anywhere, and not an audio guide.
fn is_empty(layer: &Layer) -> bool {
    layer.is_empty_layer() && !layer.audio_guide
}

/// The new order of `rows`, as indices into it, or `None` when the stack is
/// not laid out folder-then-contents.
fn grouped_order(rows: &[Arc<Layer>]) -> Option<Vec<usize>> {
    let known: HashMap<LayerId, usize> = rows.iter().enumerate().map(|(i, l)| (l.id, i)).collect();
    let parent = |i: usize| rows[i].parent.filter(|p| known.contains_key(p));
    let mut children: HashMap<Option<LayerId>, Vec<usize>> = HashMap::new();
    for i in 0..rows.len() {
        children.entry(parent(i)).or_default().push(i);
    }

    // Folder-then-contents, as it is now: anything else is left alone.
    let mut now = Vec::with_capacity(rows.len());
    emit(None, &children, rows, &mut now);
    if now != (0..rows.len()).collect::<Vec<_>>() {
        return None;
    }

    // Empty: a layer with nothing on it, or a folder of only such layers.
    let mut empty = vec![false; rows.len()];
    for i in (0..rows.len()).rev() {
        empty[i] = match rows[i].kind {
            LayerKind::Folder => children.get(&Some(rows[i].id)).is_none_or(|c| c.iter().all(|&c| empty[c])),
            _ => is_empty(&rows[i]),
        };
    }

    for siblings in children.values_mut() {
        // Units: a layer, with the masked or guided layers right under it.
        let mut units: Vec<Vec<usize>> = Vec::new();
        for &i in siblings.iter() {
            let follows = matches!(rows[i].kind, LayerKind::Masked | LayerKind::Guided);
            match units.last_mut() {
                Some(unit) if follows => unit.push(i),
                _ => units.push(vec![i]),
            }
        }
        let (full, blank): (Vec<_>, Vec<_>) = units.into_iter().partition(|u| u.iter().any(|&i| !empty[i]));
        *siblings = full.into_iter().chain(blank).flatten().collect();
    }

    let mut order = Vec::with_capacity(rows.len());
    emit(None, &children, rows, &mut order);
    Some(order)
}

/// `parent`'s layers in order, each folder followed by its own.
fn emit(parent: Option<LayerId>, children: &HashMap<Option<LayerId>, Vec<usize>>, rows: &[Arc<Layer>], out: &mut Vec<usize>) {
    for &i in children.get(&parent).map_or(&[][..], |c| c.as_slice()) {
        out.push(i);
        if rows[i].kind == LayerKind::Folder {
            emit(Some(rows[i].id), children, rows, out);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Something on the layer: what it is does not matter here.
    fn draw(scene: &mut Scene, layer: LayerId) {
        let object = crate::Object::instance_of(scene.next_object_id(), crate::SymbolId(1));
        scene.add_object_at(layer, 0, object);
    }

    fn names(scene: &Scene) -> Vec<String> {
        scene.layers().iter().map(|l| l.name.clone()).collect()
    }

    /// Empty layers gather after the full ones, folder by folder, nothing
    /// deleted and nothing changing folder; the full ones keep their order.
    #[test]
    fn empty_layers_gather_after_the_full_ones_in_each_folder() {
        let mut scene = Scene::default();
        let first = scene.layers().iter().next().unwrap().id;
        scene.update_layer(first, |l| l.name = "e1".into());
        // Added on top each time: the stack reads top-first as the names say.
        let a = scene.add_layer("a", LayerKind::Normal);
        let e2 = scene.add_layer("e2", LayerKind::Normal);
        let b = scene.add_layer("b", LayerKind::Normal);
        draw(&mut scene, a);
        draw(&mut scene, b);
        let folder = scene.add_layer("folder", LayerKind::Folder);
        let inner_empty = scene.add_layer("in-e", LayerKind::Normal);
        let inner_full = scene.add_layer("in-f", LayerKind::Normal);
        draw(&mut scene, inner_full);
        // Filed into the folder: in-e, then in-f after it.
        scene.edit_open_layers(|stack| {
            let at = stack.index_of(folder).unwrap() + 1;
            stack.move_to(inner_empty, at, Some(folder));
            let at = stack.index_of(inner_empty).unwrap() + 1;
            stack.move_to(inner_full, at, Some(folder));
        });
        // folder [in-e, in-f] ... top-level: folder, b, e2, a, e1
        assert_eq!(names(&scene), ["folder", "in-e", "in-f", "b", "e2", "a", "e1"]);
        let light = scene.add_light(crate::LightKind::Sun { azimuth: 0.0, elevation: 0.5 });
        scene.place_light(light, Some(e2));

        let moved = scene.group_empty_layers();
        assert!(moved > 0);
        assert_eq!(names(&scene), ["folder", "in-f", "in-e", "b", "a", "e2", "e1"]);
        assert_eq!(scene.layers().get(inner_empty).unwrap().parent, Some(folder), "still in its folder");
        assert_eq!(scene.light_slot(light), Some(a), "the light still lights `a` and below");
        assert_eq!(scene.group_empty_layers(), 0, "already grouped");
    }

    /// A masked layer stays under its mask, and an empty folder counts as empty.
    #[test]
    fn a_mask_keeps_its_layers_and_an_empty_folder_is_empty() {
        let mut scene = Scene::default();
        let first = scene.layers().iter().next().unwrap().id;
        scene.update_layer(first, |l| l.name = "full".into());
        draw(&mut scene, first);
        let masked = scene.add_layer("masked", LayerKind::Masked);
        draw(&mut scene, masked);
        scene.add_layer("mask", LayerKind::Mask);
        scene.add_layer("empty folder", LayerKind::Folder);
        assert_eq!(names(&scene), ["empty folder", "mask", "masked", "full"]);
        scene.group_empty_layers();
        assert_eq!(names(&scene), ["mask", "masked", "full", "empty folder"], "the mask carries its layer, empty mask or not");
    }

    #[test]
    fn a_layer_is_pinned_to_the_top() {
        let mut scene = Scene::default();
        let audio = scene.layers().iter().next().unwrap().id;
        let other = scene.add_layer("other", LayerKind::Normal);
        assert_eq!(scene.layers().index_of(other), Some(0));
        assert!(scene.pin_layer_to_top(audio));
        assert_eq!(scene.layers().index_of(audio), Some(0));
        assert!(!scene.pin_layer_to_top(audio), "already there");
    }
}

impl Scene {
    /// **Remove Folder, Keep Layers**: each folder in `folders` (every folder
    /// on the timeline being edited, when `None`) is taken away and what was
    /// filed in it goes up a level — into the folder above, or onto the
    /// timeline itself — **in the same place in the stack**, so nothing moves
    /// on the stage. A hidden or locked folder leaves its layers hidden or
    /// locked; a light that sat above a folder sits above its first layer.
    /// Returns how many folders went; bumps the revision when any did.
    pub fn remove_folders(&mut self, folders: Option<&[LayerId]>) -> usize {
        let in_symbol = self.editing_symbol().is_some_and(|id| self.library.get(id).is_some());
        let stack = self.active_layers_mut();
        let rows: Vec<Arc<Layer>> = stack.iter().cloned().collect();
        let going: Vec<LayerId> = rows
            .iter()
            .filter(|l| l.kind == LayerKind::Folder && folders.is_none_or(|f| f.contains(&l.id)))
            .map(|l| l.id)
            .collect();
        if going.is_empty() {
            return 0;
        }
        let by_id: HashMap<LayerId, Arc<Layer>> = rows.iter().map(|l| (l.id, l.clone())).collect();
        // The nearest ancestor that stays, and what hiding and locking the
        // ones that go passed down.
        let settle = |mut parent: Option<LayerId>| {
            let (mut hidden, mut locked) = (false, false);
            while let Some(p) = parent.filter(|p| going.contains(p)) {
                let folder = &by_id[&p];
                hidden |= !folder.visible;
                locked |= folder.locked;
                parent = folder.parent.filter(|q| by_id.contains_key(q));
            }
            (parent, hidden, locked)
        };
        let kept: Vec<Arc<Layer>> = rows
            .iter()
            .filter(|l| !going.contains(&l.id))
            .map(|l| {
                let (parent, hidden, locked) = settle(l.parent.filter(|p| by_id.contains_key(p)));
                if parent == l.parent && !hidden && !locked {
                    return l.clone();
                }
                let mut layer = (**l).clone();
                layer.parent = parent;
                layer.visible &= !hidden;
                layer.locked |= locked;
                Arc::new(layer)
            })
            .collect();
        let gone = going.len();
        if !in_symbol {
            for light in &mut self.lights.lights {
                let Some(above) = light.above.map(LayerId).filter(|a| going.contains(a)) else {
                    continue;
                };
                let at = rows.iter().position(|l| l.id == above).unwrap_or(0);
                light.above = rows[at..].iter().find(|l| !going.contains(&l.id)).map(|l| l.id.0);
            }
        }
        *self.active_layers_mut() = LayerStack::from_rows(kept);
        self.bump();
        gone
    }
}
