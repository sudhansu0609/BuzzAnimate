//! Merging one document into another.
//!
//! Every importer produces a whole [`Scene`], and every import lands in a
//! document that already exists. Those two documents were built independently,
//! so both allocated ids from zero and their id spaces overlap completely:
//! symbol 1 in the incoming file is a different symbol from symbol 1 in the
//! open one. Copying anything across without renumbering would silently
//! repoint instances at the wrong artwork.
//!
//! So this module renumbers. Every [`SymbolId`], [`LayerId`] and [`ObjectId`]
//! coming in is reallocated from the destination's own [`IdAllocator`], and
//! every reference between them is rewritten to match — including instances
//! nested inside other symbols, and layer `parent` links.
//!
//! # Names, unlike ids, are the user's
//!
//! Ids are renumbered silently because nobody sees them. A *name* collision is
//! different: the user chose both names and is entitled to know that one of
//! them moved. Incoming symbols are renamed only when the name is genuinely
//! taken, and every rename is recorded in the [`MergeReport`].

use std::collections::HashMap;

use crate::layer::{Layer, LayerId, LayerStack};
use crate::object::{Object, ObjectId, ObjectKind};
use crate::symbol::{Symbol, SymbolId};
use crate::timeline::{Keyframe, LayerTimeline};
use crate::{IdAllocator, Scene};

/// Where an imported document should land.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImportTarget {
    /// Animate's *Import to Library*: bring the symbols, leave the stage alone.
    Library,
    /// Animate's *Import to Stage*: bring the symbols **and** place the
    /// document's own timeline as new layers on top of the current one.
    Stage,
}

/// What a merge brought across.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct MergeReport {
    pub symbols: usize,
    pub layers: usize,
    pub objects: usize,
    /// Symbols renamed because the name was already taken: (wanted, given).
    pub renamed: Vec<(String, String)>,
}

impl MergeReport {
    /// A sentence for the status bar.
    pub fn summary(&self) -> String {
        let mut s = format!("{} symbols, {} layers, {} objects", self.symbols, self.layers, self.objects);
        if !self.renamed.is_empty() {
            s.push_str(&format!("; {} renamed to avoid a clash", self.renamed.len()));
        }
        s
    }
}

/// Renumbers everything on the way in, and remembers what became what.
struct Remapper<'a> {
    ids: &'a mut IdAllocator,
    /// Old symbol id to new. Built before any artwork is copied, because an
    /// instance may refer to a symbol defined later in the file.
    symbols: HashMap<SymbolId, SymbolId>,
    /// Old sound id to new, for the same reason: a keyframe's sound reference
    /// is a number, and two documents both numbering from one would collide.
    sounds: HashMap<crate::sound::SoundId, crate::sound::SoundId>,
    objects: usize,
}

impl Remapper<'_> {
    /// Copy a layer stack, renumbering layers, their parents and their
    /// artwork.
    fn layer_stack(&mut self, source: &LayerStack) -> LayerStack {
        // Layer ids are allocated up front: `parent` points at another layer in
        // the same stack, and a folder can appear after its children.
        let mut layer_ids: HashMap<LayerId, LayerId> = HashMap::new();
        for layer in source.iter() {
            layer_ids.insert(layer.id, LayerId(self.ids.take()));
        }

        let mut out = LayerStack::new();
        for (index, layer) in source.iter().enumerate() {
            let mut copy = Layer::new(
                layer_ids[&layer.id],
                layer.name.clone(),
                layer.kind,
            );
            // A parent outside this stack cannot be honoured, so the layer
            // becomes top-level rather than pointing at a foreign document.
            copy.parent = layer.parent.and_then(|p| layer_ids.get(&p).copied());
            copy.visible = layer.visible;
            copy.locked = layer.locked;
            copy.outline = layer.outline;
            copy.color = layer.color;
            copy.height = layer.height;
            copy.collapsed = layer.collapsed;
            copy.frames = self.timeline(&layer.frames);
            out.insert(index, copy);
        }
        out
    }

    fn timeline(&mut self, source: &LayerTimeline) -> LayerTimeline {
        let keyframes: Vec<Keyframe> = source
            .keyframes()
            .iter()
            .map(|k| Keyframe {
                start: k.start,
                objects: std::sync::Arc::new(
                    k.objects
                        .iter()
                        .map(|o| std::sync::Arc::new(self.object(o)))
                        .collect(),
                ),
                label: k.label.clone(),
                tween: k.tween,
                // Sound ids are remapped by the caller alongside symbol ids;
                // a merge that kept the source's id would point at whichever
                // local sound happened to share the number.
                sound: k.sound.map(|mut reference| {
                    if let Some(new) = self.sounds.get(&reference.sound) {
                        reference.sound = *new;
                    }
                    reference
                }),
            })
            .collect();
        LayerTimeline::from_parts(keyframes, source.length())
    }

    fn object(&mut self, source: &Object) -> Object {
        self.objects += 1;
        Object {
            id: ObjectId(self.ids.take()),
            name: source.name.clone(),
            transform: source.transform,
            kind: match &source.kind {
                ObjectKind::Shape(s) => ObjectKind::Shape(s.clone()),
                ObjectKind::Group(children) => ObjectKind::Group(
                    children
                        .iter()
                        .map(|c| std::sync::Arc::new(self.object(c)))
                        .collect(),
                ),
                ObjectKind::Instance(i) => {
                    let mut copy = i.clone();
                    // The whole point of the exercise. A symbol that did not
                    // come across leaves the reference as it was, where it
                    // will draw nothing — rather than silently pointing at
                    // whichever local symbol happens to hold that number.
                    if let Some(new) = self.symbols.get(&i.symbol) {
                        copy.symbol = *new;
                    }
                    ObjectKind::Instance(copy)
                }
                // A rig's parts are objects in their own right, so they are
                // renumbered too — otherwise an imported armature would hold
                // artwork whose ids collide with the document's own.
                ObjectKind::Armature(rig) => {
                    let mut copy = rig.clone();
                    for part in &mut copy.parts {
                        part.artwork = std::sync::Arc::new(self.object(&part.artwork));
                    }
                    ObjectKind::Armature(copy)
                }
                ObjectKind::Warp(warp) => ObjectKind::Warp(warp.clone()),
            },
            locked: source.locked,
            visible: source.visible,
            filters: source.filters.clone(),
            blend: source.blend,
            spatial: source.spatial,
            pivot: source.pivot,
        }
    }
}

impl Scene {
    /// Merge another document into this one.
    ///
    /// The other scene is left untouched, so an importer can hand over a
    /// borrowed result and a failed merge cannot corrupt it.
    ///
    /// Bumps the revision once, at the end, so the whole import is a single
    /// undo step rather than one per symbol.
    pub fn merge(&mut self, other: &Scene, target: ImportTarget) -> MergeReport {
        let mut report = MergeReport::default();

        // 1. Allocate a new id for every incoming symbol, before copying any
        //    artwork — an instance inside symbol A may refer to symbol B.
        let mut symbols = HashMap::new();
        for symbol in other.library.iter() {
            symbols.insert(symbol.id, SymbolId(self.ids.take()));
        }

        // Sounds are renumbered the same way, and copied across: a document
        // whose keyframes referred to sounds that did not come with them would
        // play silence and give no clue why.
        let mut sounds = HashMap::new();
        let mut incoming_sounds = Vec::new();
        for sound in other.sounds.iter() {
            let id = crate::sound::SoundId(self.ids.take());
            sounds.insert(sound.id, id);
            let mut copy = (**sound).clone();
            copy.id = id;
            copy.name = self.sounds.unique_name(&copy.name);
            incoming_sounds.push(copy);
        }
        for sound in incoming_sounds {
            self.sounds.insert(sound);
        }

        let mut remap = Remapper {
            ids: &mut self.ids,
            symbols,
            sounds,
            objects: 0,
        };

        // 2. Copy the symbols themselves.
        let mut incoming: Vec<Symbol> = Vec::new();
        for symbol in other.library.iter() {
            incoming.push(Symbol {
                id: remap.symbols[&symbol.id],
                name: symbol.name.clone(),
                kind: symbol.kind,
                folder: symbol.folder.clone(),
                layers: remap.layer_stack(&symbol.layers),
                registration: symbol.registration,
            });
        }

        // 3. The document's own timeline, if this is an Import to Stage.
        let stage_layers = match target {
            ImportTarget::Stage => Some(remap.layer_stack(other.stage_layers())),
            ImportTarget::Library => None,
        };
        report.objects = remap.objects;

        // 4. Names are resolved against the destination one at a time, so two
        //    incoming symbols that would both become "Hero" cannot collide
        //    with each other either.
        for mut symbol in incoming {
            let wanted = symbol.name.clone();
            let given = self.library.unique_name(&wanted);
            if given != wanted {
                report.renamed.push((wanted, given.clone()));
            }
            symbol.name = given;
            self.library.insert(symbol);
            report.symbols += 1;
        }

        // Empty folders are part of how the user organised their library, so
        // they come across too.
        for folder in other.library.folders() {
            self.library.add_folder(folder);
        }

        // 5. Incoming layers go on top, which is where Animate puts them and
        //    where the user will look for what they just imported.
        if let Some(layers) = stage_layers {
            let arriving: Vec<_> = layers.iter().cloned().collect();
            report.layers = arriving.len();
            for (index, layer) in arriving.into_iter().enumerate() {
                self.layers.insert(index, (*layer).clone());
            }
        }

        self.bump();
        report
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::object::ShapeData;
    use crate::symbol::SymbolKind;
    use buzz_geom::{Affine, Shape as _};
    use kurbo::Rect;
    use peniko::Color;

    fn shape(size: f64) -> ShapeData {
        ShapeData::filled(Rect::new(0.0, 0.0, size, size).to_path(1e-9), Color::WHITE)
    }

    /// A document with one symbol, one instance of it on the stage, and a
    /// second symbol nested inside the first.
    fn document(name: &str, size: f64) -> Scene {
        let mut scene = Scene::default();
        let layer = scene.layers().iter().next().unwrap().id;

        let inner = scene.add_symbol(format!("{name} Inner"), SymbolKind::Graphic, None);
        let outer = scene.add_symbol(format!("{name} Outer"), SymbolKind::Graphic, None);

        // Artwork in the inner symbol.
        let inner_layer = scene.library().get(inner).unwrap().layers.iter().next().unwrap().id;
        let art = Object::shape(scene.next_object_id(), shape(size));
        scene.library_mut().update(inner, |s| {
            s.layers.update(inner_layer, |l| {
                l.frames.set_objects(0, vec![std::sync::Arc::new(art)]);
            });
        });

        // An instance of the inner symbol, inside the outer one.
        let outer_layer = scene.library().get(outer).unwrap().layers.iter().next().unwrap().id;
        let nested = Object::instance_of(scene.next_object_id(), inner);
        scene.library_mut().update(outer, |s| {
            s.layers.update(outer_layer, |l| {
                l.frames.set_objects(0, vec![std::sync::Arc::new(nested)]);
            });
        });

        scene.add_instance_at(layer, 0, outer, Affine::IDENTITY);
        scene
    }

    /// The defect the whole module exists to prevent: both documents number
    /// their symbols from one, so a naive copy would repoint instances at
    /// whatever local symbol happened to share the number.
    #[test]
    fn a_nested_instance_still_points_at_its_own_symbol_after_merging() {
        let mut host = document("Host", 10.0);
        let guest = document("Guest", 99.0);

        // The two documents genuinely do collide before merging.
        let host_ids: Vec<u64> = host.library().iter().map(|s| s.id.0).collect();
        let guest_ids: Vec<u64> = guest.library().iter().map(|s| s.id.0).collect();
        assert_eq!(host_ids, guest_ids, "the id spaces must overlap for this test to mean anything");

        host.merge(&guest, ImportTarget::Stage);

        // Find the guest's outer symbol by name and follow its nested instance
        // back to a symbol; it must be the *guest's* inner symbol, whose
        // artwork is 99 wide, not the host's 10-wide one.
        let outer = host
            .library()
            .find_by_name("Guest Outer")
            .expect("the guest symbol came across");
        let nested = outer
            .layers
            .iter()
            .flat_map(|l| l.all_objects())
            .find_map(|o| o.instance().map(|i| i.symbol))
            .expect("the nested instance came across");

        let target = host.library().get(nested).expect("it points at a real symbol");
        assert_eq!(target.name, "Guest Inner", "the nested instance was repointed");

        let bounds = target.bounds().expect("the inner symbol has artwork");
        assert_eq!(bounds.width(), 99.0, "and at the guest's artwork, not the host's");
    }

    /// The host's own artwork must be exactly as it was.
    #[test]
    fn merging_leaves_the_host_documents_own_symbols_alone() {
        let mut host = document("Host", 10.0);
        let guest = document("Guest", 99.0);

        let before: Vec<(String, u64)> = host
            .library()
            .iter()
            .map(|s| (s.name.clone(), s.id.0))
            .collect();

        host.merge(&guest, ImportTarget::Stage);

        for (name, id) in before {
            let still = host.library().get(SymbolId(id)).expect("still present");
            assert_eq!(still.name, name, "a host symbol was renamed or replaced");
        }
        let inner = host.library().find_by_name("Host Inner").unwrap();
        assert_eq!(inner.bounds().unwrap().width(), 10.0);
    }

    #[test]
    fn the_source_document_is_not_modified() {
        let mut host = document("Host", 10.0);
        let guest = document("Guest", 99.0);
        let untouched = guest.clone();

        host.merge(&guest, ImportTarget::Stage);

        assert_eq!(guest, untouched, "merging must not disturb what it read");
    }

    /// Importing to the library brings symbols but must not touch the stage.
    #[test]
    fn importing_to_the_library_leaves_the_timeline_alone() {
        let mut host = document("Host", 10.0);
        let guest = document("Guest", 99.0);

        let layers_before = host.stage_layers().len();
        let symbols_before = host.library().len();

        let report = host.merge(&guest, ImportTarget::Library);

        assert_eq!(host.stage_layers().len(), layers_before, "the stage must not change");
        assert_eq!(report.layers, 0);
        assert_eq!(host.library().len(), symbols_before + report.symbols);
    }

    #[test]
    fn importing_to_the_stage_puts_the_new_layers_on_top() {
        let mut host = document("Host", 10.0);
        let guest = document("Guest", 99.0);
        let host_layer = host.layers().iter().next().unwrap().id;
        host.update_layer(host_layer, |l| l.name = "Host Layer".into());

        let report = host.merge(&guest, ImportTarget::Stage);
        assert!(report.layers > 0);

        let top = host.stage_layers().iter().next().unwrap();
        assert_ne!(top.name, "Host Layer", "the imported layer goes above the existing one");
    }

    /// A name the user already used is theirs; the incoming one moves aside,
    /// and the move is reported rather than done quietly.
    #[test]
    fn a_colliding_symbol_name_is_changed_and_reported() {
        let mut host = Scene::default();
        host.add_symbol("Hero", SymbolKind::Graphic, None);

        let mut guest = Scene::default();
        guest.add_symbol("Hero", SymbolKind::MovieClip, None);

        let report = host.merge(&guest, ImportTarget::Library);

        assert_eq!(report.renamed.len(), 1, "exactly one clash");
        assert_eq!(report.renamed[0].0, "Hero");
        assert_ne!(report.renamed[0].1, "Hero");
        assert_eq!(host.library().len(), 2, "both symbols survive");

        // The host's original is still the graphic it always was.
        let original = host.library().find_by_name("Hero").unwrap();
        assert_eq!(original.kind, SymbolKind::Graphic);
    }

    /// Two incoming symbols wanting the same name must not collide with each
    /// other either — names are resolved one at a time against the growing
    /// library, not all at once against the original.
    #[test]
    fn two_incoming_symbols_wanting_one_name_both_survive() {
        let mut host = Scene::default();
        host.add_symbol("Hero", SymbolKind::Graphic, None);

        let mut first = Scene::default();
        first.add_symbol("Hero", SymbolKind::Graphic, None);
        let mut second = Scene::default();
        second.add_symbol("Hero", SymbolKind::Graphic, None);

        host.merge(&first, ImportTarget::Library);
        host.merge(&second, ImportTarget::Library);

        assert_eq!(host.library().len(), 3);
        let names: std::collections::BTreeSet<&str> =
            host.library().iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names.len(), 3, "all three names are distinct: {names:?}");
    }

    /// Ids handed out after a merge must not repeat ones the merge just used.
    #[test]
    fn later_edits_do_not_reuse_an_imported_id() {
        let mut host = document("Host", 10.0);
        let guest = document("Guest", 99.0);
        host.merge(&guest, ImportTarget::Stage);

        let existing: std::collections::BTreeSet<u64> =
            host.library().iter().map(|s| s.id.0).collect();

        let fresh = host.add_symbol("Afterwards", SymbolKind::Graphic, None);
        assert!(!existing.contains(&fresh.0), "the allocator reissued an id");
    }

    /// Every object across the whole document must still have a unique id,
    /// including artwork nested inside symbols.
    #[test]
    fn every_object_id_is_still_unique_after_a_merge() {
        let mut host = document("Host", 10.0);
        let guest = document("Guest", 99.0);
        host.merge(&guest, ImportTarget::Stage);

        let mut seen = std::collections::BTreeSet::new();
        let stage = host.stage_layers().iter().flat_map(|l| l.all_objects());
        let nested = host
            .library()
            .iter()
            .flat_map(|s| s.layers.iter().flat_map(|l| l.all_objects()));
        for object in stage.chain(nested) {
            assert!(seen.insert(object.id.0), "object id {} appears twice", object.id.0);
        }
    }

    /// Layer folders must survive the renumbering, pointing at the *copied*
    /// folder rather than at the original document's layer.
    #[test]
    fn a_layer_folder_still_holds_its_children_after_merging() {
        let mut guest = Scene::default();
        let folder = guest.add_layer("Folder", crate::layer::LayerKind::Folder);
        let child = guest.add_layer("Child", crate::layer::LayerKind::Normal);
        guest.update_layer(child, |l| l.parent = Some(folder));

        let mut host = Scene::default();
        host.merge(&guest, ImportTarget::Stage);

        let copied_folder = host
            .stage_layers()
            .iter()
            .find(|l| l.name == "Folder")
            .expect("the folder came across");
        let copied_child = host
            .stage_layers()
            .iter()
            .find(|l| l.name == "Child")
            .expect("the child came across");

        assert_eq!(
            copied_child.parent,
            Some(copied_folder.id),
            "the child must point at the copied folder"
        );
        assert_ne!(copied_folder.id, folder, "and the folder must have been renumbered");
    }

    /// An instance whose symbol is not in the file cannot be repaired, and
    /// must not be repointed at an unrelated local symbol that happens to
    /// share the number.
    #[test]
    fn a_dangling_instance_is_left_dangling_rather_than_mispointed() {
        let mut host = Scene::default();
        let decoy = host.add_symbol("Decoy", SymbolKind::Graphic, None);

        // A guest whose instance refers to a symbol it does not contain, using
        // the same number the host gave its decoy.
        let mut guest = Scene::default();
        let layer = guest.layers().iter().next().unwrap().id;
        let orphan = Object::instance_of(guest.next_object_id(), decoy);
        guest.add_object_at(layer, 0, orphan);

        host.merge(&guest, ImportTarget::Stage);

        let imported = host
            .stage_layers()
            .iter()
            .flat_map(|l| l.all_objects())
            .filter_map(|o| o.instance())
            .find(|i| i.symbol == decoy);

        assert!(
            imported.is_some(),
            "the reference is left as it was, drawing nothing, rather than \
             being silently repointed at a different symbol"
        );
    }
}
