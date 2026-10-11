//! Document persistence, undo history and autosave.
//!
//! [`Document`] is what the editor actually holds: the current [`Scene`], the
//! undo [`History`], where the file lives, and the [`Autosave`] schedule. Every
//! mutation goes through [`Document::edit`], so no path can change the document
//! without also recording an undo step — the usual way undo quietly develops
//! holes.

pub mod assets;
pub mod autosave;
pub mod commands;
pub mod format;
pub mod history;
pub mod project;
pub mod sequence;
pub mod serial;
pub mod snapshots;
pub mod srt;
pub mod templates;

use std::path::{Path, PathBuf};

use buzz_scene::{ImportTarget, LayerId, LayerKind, Scene, SoundAsset, SoundId, SoundRef, SoundSync};

pub use assets::{Asset, AssetLibrary, rig_kind_of};
pub use autosave::{Autosave, AutosavePlan, Recovery, find_recoveries};
pub use commands::{CommandLibrary, SavedCommand};
pub use format::{DocError, EXTENSION, MIMETYPE, Meta};
pub use history::{History, UndoLabel};pub use project::{FilmError, PROJECT_EXTENSION, PROJECT_VERSION, Project, Shot};
pub use sequence::{SceneKey, SceneRole, ShotInfo};
pub use serial::{FORMAT_VERSION, SerialError};
pub use snapshots::SnapshotLibrary;
pub use templates::{Template, TemplateLibrary};

/// A scene's history as it stood before a run of edits; see
/// [`Document::run_checkpoint`].
pub struct RunCheckpoint {
    uid: u64,
    key: SceneKey,
    history: History,
    scene: Scene,
}

impl RunCheckpoint {
    /// Is `document` still the one this checkpoint was taken from (not closed
    /// or replaced by another since)?
    pub fn is_for(&self, document: &Document) -> bool {
        self.uid == document.uid
    }
}

/// An open document.
/// One scene of a document: an independent timeline with its own name and undo
/// history. A document has at least one, and edits, undo and view all address
/// whichever is [`Document::active_scene`].
struct SceneSlot {
    name: String,
    scene: Scene,
    history: History,
    /// Stable identity, independent of where this slot sits in `scenes`. See
    /// [`SceneKey`].
    key: SceneKey,
    /// Setup, Shot or (the default) Normal. See [`SceneRole`].
    role: SceneRole,
}

pub struct Document {
    /// Process-unique identity, so a caller can tell this document from a
    /// later one that replaced it (see [`RunCheckpoint`]).
    uid: u64,
    /// Never empty — a document always has at least one scene.
    scenes: Vec<SceneSlot>,
    active: usize,
    path: Option<PathBuf>,
    /// The file this document was translated from (e.g. an imported .fla),
    /// kept separate from `path` so Save still asks where to put a `.buzz`
    /// rather than overwriting the original.
    origin: Option<PathBuf>,
    autosave: Autosave,
    /// The next [`SceneKey`] to allocate. Always greater than every key any
    /// scene of this document has ever had, so a new shot never collides
    /// with one still named by `sequence`, `from_setup` or `take_of` even
    /// after the scene that had it is gone.
    next_scene_key: u32,
    /// The sequence's cut: shot keys, in film order. A shot's key not listed
    /// here is a take not in the cut — still open, still editable, exported
    /// nowhere. See "Setups are stamped, never linked" in
    /// `docs/SEQUENCE_SETUPS_PLAN.md`.
    sequence: Vec<SceneKey>,
    /// Where the master audio's sound keyframe lives, if the sequence has
    /// one set: which scene, and which layer on it.
    ///
    /// Not a bare sound reference, because a [`buzz_scene::SoundId`] is only
    /// meaningful inside the `SoundLibrary` of the scene that holds it —
    /// there is no document-wide sound namespace to point a reference at.
    master_audio: Option<(SceneKey, LayerId)>,
}

impl Default for Document {
    fn default() -> Self {
        Self::new(Scene::default())
    }
}

impl Document {
    /// A new, unsaved document with a single scene.
    pub fn new(scene: Scene) -> Self {
        // Work that has never been saved recovers from the application's own
        // directory rather than the system temp, which is swept.
        let autosave = Autosave::untitled(crate::autosave::recovery_dir());
        static NEXT_UID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        Self {
            uid: NEXT_UID.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
            scenes: vec![SceneSlot {
                name: "Scene 1".to_string(),
                scene,
                history: History::default(),
                key: SceneKey(0),
                role: SceneRole::Normal,
            }],
            active: 0,
            path: None,
            origin: None,
            autosave,
            next_scene_key: 1,
            sequence: Vec::new(),
            master_audio: None,
        }
    }

    /// A document from several named scenes, the first active.
    ///
    /// Every scene comes back `Normal`, with keys assigned by position —
    /// callers that need roles, stable keys or a sequence restored from a
    /// file use [`Self::open`] instead, which reads them from the manifest.
    pub fn from_scenes(scenes: Vec<(String, Scene)>) -> Self {
        let mut doc = Self::new(Scene::default());
        if scenes.is_empty() {
            return doc;
        }
        let count = scenes.len();
        doc.scenes = scenes
            .into_iter()
            .enumerate()
            .map(|(i, (name, scene))| {
                let mut history = History::default();
                history.mark_saved(scene.revision());
                SceneSlot {
                    name,
                    scene,
                    history,
                    key: SceneKey(i as u32),
                    role: SceneRole::Normal,
                }
            })
            .collect();
        doc.active = 0;
        doc.next_scene_key = count as u32;
        doc
    }

    /// Open a document from disk.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, DocError> {
        let path = path.as_ref().to_path_buf();
        let loaded = format::load_scenes_full(&path)?;
        let mut doc = Self::from_loaded_scenes(loaded);
        doc.path = Some(path.clone());
        doc.autosave = Autosave::beside(&path);
        Ok(doc)
    }

    /// Rebuild a document from every scene, role, key, the sequence's cut
    /// and the master audio that [`format::load_scenes_full`] read back —
    /// the same reconstruction [`Self::open`] does, for a caller that
    /// already has a [`format::LoadedScenes`] and wants a document with
    /// roles and the sequence honoured without adopting `path`/autosave the
    /// way opening a file does (see `buzz-app`'s headless renderer, which
    /// loads a document to direct or script over rather than to edit and
    /// save back to the same place by default).
    pub fn from_loaded_scenes(loaded: format::LoadedScenes) -> Self {
        let mut doc = Self::new(Scene::default());
        let next_scene_key = loaded.keys.iter().map(|k| k.0).max().map_or(0, |m| m + 1);
        doc.scenes = loaded
            .scenes
            .into_iter()
            .zip(loaded.keys)
            .zip(loaded.roles)
            .map(|(((name, scene), key), role)| {
                let mut history = History::default();
                history.mark_saved(scene.revision());
                SceneSlot {
                    name,
                    scene,
                    history,
                    key,
                    role,
                }
            })
            .collect();
        doc.next_scene_key = next_scene_key;
        doc.sequence = loaded.sequence;
        doc.master_audio = loaded.master_audio;
        doc.switch_scene(loaded.active);
        doc
    }

    fn active_slot(&self) -> &SceneSlot {
        &self.scenes[self.active]
    }

    fn active_slot_mut(&mut self) -> &mut SceneSlot {
        &mut self.scenes[self.active]
    }

    pub fn scene(&self) -> &Scene {
        &self.active_slot().scene
    }

    pub fn history(&self) -> &History {
        &self.active_slot().history
    }

    /// The names of every scene, in order.
    pub fn scene_names(&self) -> Vec<String> {
        self.scenes.iter().map(|s| s.name.clone()).collect()
    }

    /// Which scene is being edited.
    pub fn active_scene(&self) -> usize {
        self.active
    }

    /// **Every scene, in the order the film plays them.**
    ///
    /// Snapshots, for an export to take to its own thread. Copy-on-write, so
    /// this is a handful of pointer copies rather than a copy of the artwork.
    pub fn film(&self) -> Vec<Scene> {
        self.scenes.iter().map(|s| s.scene.clone()).collect()
    }

    /// How long the whole film is, in frames — every scene end to end, each
    /// counting its own looping section.
    ///
    /// What the export dialog's range runs over, so "all of it" means all of
    /// the film rather than all of whichever scene happened to be open.
    pub fn film_frames(&self) -> u32 {
        self.scenes
            .iter()
            .map(|s| s.scene.rendered_frame_count())
            .sum()
    }

    /// The first film frame of the scene at `index`.
    ///
    /// What turns "the frame I am looking at" into "the frame of the film I am
    /// looking at" — which is what an exported still, and a range typed
    /// against the film, both need.
    pub fn film_start_of(&self, index: usize) -> u32 {
        self.scenes
            .iter()
            .take(index)
            .map(|s| s.scene.rendered_frame_count())
            .sum()
    }

    /// **Scene indices, in the order the exported/previewed film actually
    /// plays them.** Setups are skipped, a take not in [`Self::sequence`] is
    /// skipped, and the shots that are in it play together, in that order,
    /// at the position their first sits among the document's `Normal`
    /// scenes. See `docs/SEQUENCE_SETUPS_PLAN.md` S6 and
    /// [`sequence::film_order`], which does the actual work.
    ///
    /// A document with only `Normal` scenes comes back as plain scene order,
    /// unchanged from before this existed.
    pub fn film_order(&self) -> Vec<usize> {
        let roles: Vec<SceneRole> = self.scenes.iter().map(|s| s.role.clone()).collect();
        let keys: Vec<SceneKey> = self.scenes.iter().map(|s| s.key).collect();
        sequence::film_order(&roles, &keys, &self.sequence)
    }

    /// **How long the scene at `index` plays in the film** — not always the
    /// same as its own timeline length. A `Shot`'s playing length is
    /// `audio_out - audio_in`: frames kept past a shortened shot's new end
    /// are not played (see `docs/SEQUENCE_SETUPS_PLAN.md`, "Nothing
    /// destructive by timing either"). A `Normal` or `Setup` scene plays its
    /// whole rendered length, exactly as before — this is what makes a
    /// document with only `Normal` scenes behave exactly as it always has.
    pub fn playing_length(&self, index: usize) -> u32 {
        let Some(slot) = self.scenes.get(index) else {
            return 0;
        };
        match &slot.role {
            SceneRole::Shot(info) => info.audio_out.saturating_sub(info.audio_in).max(1),
            _ => slot.scene.rendered_frame_count(),
        }
    }

    /// **The film, ready for a reel**: every scene that plays, in
    /// [`Self::film_order`], each paired with [`Self::playing_length`] —
    /// what `buzz_export::Reel::of_lengths` wants. Snapshots, like
    /// [`Self::film`]: `Scene::clone` is a handful of `Arc` copies, not a
    /// copy of the artwork.
    pub fn film_with_lengths(&self) -> Vec<(Scene, u32)> {
        self.film_order()
            .into_iter()
            .filter_map(|i| self.scenes.get(i).map(|s| (s.scene.clone(), self.playing_length(i))))
            .collect()
    }

    /// Add a new empty scene after the active one and switch to it. The stage
    /// size, frame rate and background carry over, as a new scene in Animate
    /// inherits the document's, so the shots match without setting them again.
    pub fn add_scene(&mut self) {
        let mut scene = Scene::default();
        *scene.stage_mut() = *self.scene().stage();
        let name = self.unique_scene_name("Scene");
        let key = self.allocate_scene_key();
        self.active += 1;
        self.scenes.insert(
            self.active,
            SceneSlot {
                name,
                scene,
                history: History::default(),
                key,
                role: SceneRole::Normal,
            },
        );
    }

    /// **Duplicate a scene**, whole, and switch to the copy.
    ///
    /// The set, the cast, the lights, the camera track and every keyframe come
    /// with it. This is the operation a conversation is built out of: two
    /// people standing in a room is the *same* scene beat after beat, and only
    /// the performance differs — so the second shot starts as a copy of the
    /// first and is then changed, rather than being staged again or pasted
    /// frame by frame onto the end of the one before it.
    ///
    /// The copy gets its **own history**, not the original's. Sharing one
    /// would mean undoing in the new scene reaching back into the old one's
    /// past and quietly rewriting a shot that was already finished.
    pub fn duplicate_scene(&mut self, index: usize) {
        let Some(source) = self.scenes.get(index) else {
            return;
        };
        // Named after what it came from, so a duplicated "Kitchen" is
        // "Kitchen 2" rather than "Scene 4" — the name is how a director finds
        // a shot in a list of twenty.
        let stem = source
            .name
            .trim_end_matches(|c: char| c.is_ascii_digit() || c.is_whitespace());
        let stem = if stem.is_empty() { "Scene" } else { stem };
        let name = self.unique_scene_name(stem);
        let scene = source.scene.clone();
        let role = source.role.clone();
        let key = self.allocate_scene_key();

        self.active = index + 1;
        self.scenes.insert(
            self.active,
            SceneSlot {
                name,
                scene,
                history: History::default(),
                key,
                role,
            },
        );
    }

    /// Move a scene to another position, and follow it.
    ///
    /// Scenes are an *ordered* list — they are the order the film plays in —
    /// so being able to say "this beat comes before that one" is part of
    /// having them at all.
    pub fn move_scene(&mut self, from: usize, to: usize) {
        if from >= self.scenes.len() || to >= self.scenes.len() || from == to {
            return;
        }
        let slot = self.scenes.remove(from);
        self.scenes.insert(to, slot);
        self.active = to;
    }

    /// A scene name not already taken, e.g. `Scene 3`.
    ///
    /// Numbering starts at two when the bare stem is itself a scene, because
    /// a scene called "Kitchen" *is* the first Kitchen: the copy beside it
    /// should be "Kitchen 2", not "Kitchen 1" sitting after it.
    fn unique_scene_name(&self, stem: &str) -> String {
        let taken = |name: &str| self.scenes.iter().any(|s| s.name == name);
        let first = if taken(stem) { 2 } else { 1 };
        for n in first.. {
            let candidate = format!("{stem} {n}");
            if !taken(&candidate) {
                return candidate;
            }
        }
        unreachable!()
    }

    /// Switch which scene is being edited. Out-of-range indices are ignored.
    pub fn switch_scene(&mut self, index: usize) {
        if index < self.scenes.len() {
            self.active = index;
        }
    }

    /// Rename a scene. A blank name, or one already taken, is refused.
    pub fn rename_scene(&mut self, index: usize, name: impl Into<String>) {
        let name = name.into();
        let taken = self
            .scenes
            .iter()
            .enumerate()
            .any(|(i, s)| i != index && s.name == name);
        if index < self.scenes.len() && !name.trim().is_empty() && !taken {
            self.scenes[index].name = name;
        }
    }

    /// Delete a scene. The last remaining scene cannot be deleted — a document
    /// always has one.
    pub fn delete_scene(&mut self, index: usize) {
        if self.scenes.len() <= 1 || index >= self.scenes.len() {
            return;
        }
        self.scenes.remove(index);
        self.active = self.active.min(self.scenes.len() - 1);
    }

    /// Hand out a fresh [`SceneKey`], never reused even after the scene that
    /// held it is gone.
    fn allocate_scene_key(&mut self) -> SceneKey {
        let key = SceneKey(self.next_scene_key);
        self.next_scene_key += 1;
        key
    }

    /// The stable key of the scene at `index`, if there is one there.
    pub fn scene_key(&self, index: usize) -> Option<SceneKey> {
        self.scenes.get(index).map(|s| s.key)
    }

    /// Which slot holds `key`, if any scene still does.
    pub fn index_of_key(&self, key: SceneKey) -> Option<usize> {
        self.scenes.iter().position(|s| s.key == key)
    }

    /// What kind of scene is at `index`: Normal, Setup or Shot.
    pub fn role(&self, index: usize) -> Option<&SceneRole> {
        self.scenes.get(index).map(|s| &s.role)
    }

    /// Change what kind of scene is at `index`. Out-of-range indices are
    /// ignored. Not an [`Self::edit`] — like a scene's name, its role is
    /// bookkeeping about the document rather than artwork inside a scene, so
    /// it is not itself undoable.
    pub fn set_role(&mut self, index: usize, role: SceneRole) {
        if let Some(slot) = self.scenes.get_mut(index) {
            slot.role = role;
        }
    }

    /// The sequence's cut, in film order: shot keys, in the order they play.
    /// A take not listed here is simply absent from the film — still open,
    /// still editable.
    pub fn sequence(&self) -> &[SceneKey] {
        &self.sequence
    }

    /// Replace the sequence's cut wholesale — e.g. after a reorder or a
    /// split. Keys naming scenes that no longer exist are left in place
    /// rather than silently dropped: nothing here is destructive on the
    /// caller's behalf.
    pub fn set_sequence(&mut self, sequence: Vec<SceneKey>) {
        self.sequence = sequence;
    }

    /// **Stamp a new Shot from a Setup.**
    ///
    /// An independent copy of the setup's layers, sliced to
    /// `audio_in..audio_out` of the master audio: every layer's frames are
    /// extended (never shortened, never truncated) to hold the last frame
    /// out to the shot's length. From this moment the shot owns everything
    /// in it — editing the setup afterwards never touches it. See "Setups
    /// are stamped, never linked" in `docs/SEQUENCE_SETUPS_PLAN.md`.
    ///
    /// Returns `None` if `setup` does not name a scene with the `Setup` role.
    pub fn stamp_shot(
        &mut self,
        setup: SceneKey,
        audio_in: u32,
        audio_out: u32,
    ) -> Option<SceneKey> {
        let setup_index = self.index_of_key(setup)?;
        if !matches!(self.scenes[setup_index].role, SceneRole::Setup) {
            return None;
        }
        let setup_name = self.scenes[setup_index].name.clone();
        let setup_revision = self.scenes[setup_index].scene.revision();

        // Cheap: `Scene::clone` shares its `LayerStack`'s `Arc`s rather than
        // copying artwork, and stays that way per layer until something
        // actually edits it.
        let mut scene = self.scenes[setup_index].scene.clone();
        let target_len = audio_out.saturating_sub(audio_in).max(1);
        let layer_ids: Vec<LayerId> = scene.stage_layers().iter().map(|l| l.id).collect();
        for id in layer_ids {
            scene.update_stage_layer(id, |layer| {
                // `insert_frame` only ever *extends* a span that is shorter
                // than requested — it holds the layer's last keyframe out to
                // the new length rather than deleting or reflowing anything.
                if layer.frames.length() < target_len {
                    layer.frames.insert_frame(target_len - 1);
                }
            });
        }

        let ordinal = 1 + self
            .scenes
            .iter()
            .filter(|s| {
                matches!(&s.role, SceneRole::Shot(info) if info.from_setup == Some(setup))
            })
            .count();
        let name = format!("{setup_name} – Shot {ordinal}");

        let insert_at = self
            .sequence
            .last()
            .and_then(|last| self.index_of_key(*last))
            .map(|i| i + 1)
            .unwrap_or(setup_index + 1);

        let mut history = History::default();
        history.mark_saved(scene.revision());
        let key = self.allocate_scene_key();
        let slot = SceneSlot {
            name,
            scene,
            history,
            key,
            role: SceneRole::Shot(ShotInfo {
                from_setup: Some(setup),
                setup_revision,
                audio_in,
                audio_out,
                take_of: None,
                continue_from: None,
                dialogue_layer: None,
            }),
        };

        if insert_at <= self.active {
            self.active += 1;
        }
        self.scenes.insert(insert_at, slot);
        self.sequence.push(key);
        self.retrim_shot(key);
        Some(key)
    }

    /// **Duplicate Shot**: an alternative take of an existing shot.
    ///
    /// Same slice of the master audio, its own copy of the layers, its own
    /// history. Not placed in [`Self::sequence`] — the original keeps
    /// playing until [`Self::set_active_take`] swaps which of the two is in
    /// the cut. Returns `None` if `key` does not name a Shot.
    pub fn duplicate_shot(&mut self, key: SceneKey) -> Option<SceneKey> {
        let index = self.index_of_key(key)?;
        let SceneRole::Shot(source_info) = self.scenes[index].role.clone() else {
            return None;
        };
        let source_name = self.scenes[index].name.clone();
        let scene = self.scenes[index].scene.clone();

        let name = self.unique_scene_name(&source_name);
        let mut history = History::default();
        history.mark_saved(scene.revision());
        let new_key = self.allocate_scene_key();
        let slot = SceneSlot {
            name,
            scene,
            history,
            key: new_key,
            role: SceneRole::Shot(ShotInfo {
                take_of: Some(key),
                continue_from: None,
                ..source_info
            }),
        };

        let insert_at = index + 1;
        if insert_at <= self.active {
            self.active += 1;
        }
        self.scenes.insert(insert_at, slot);
        // Deliberately not pushed to `self.sequence`: a fresh take is not in
        // the cut until `set_active_take` puts it there.
        Some(new_key)
    }

    /// Swap which of two takes sits in the cut. `original` keeps existing —
    /// only `Self::sequence` changes, so switching back is just calling this
    /// again the other way round.
    pub fn set_active_take(&mut self, original: SceneKey, take: SceneKey) {
        if let Some(pos) = self.sequence.iter().position(|k| *k == original) {
            self.sequence[pos] = take;
        }
    }

    // -- Audio is the timeline (docs/SEQUENCE_SETUPS_PLAN.md "The audio is
    // the timeline") ---------------------------------------------------------

    /// Where the master audio's sound keyframe lives, if one has been set:
    /// which scene, and which layer on it.
    pub fn master_audio(&self) -> Option<(SceneKey, LayerId)> {
        self.master_audio
    }

    /// **Set Master Audio.** Point the sequence's master audio at a sound
    /// already carried by a keyframe on `layer` (frame 0) of `scene` — the
    /// one dialogue/soundtrack track every shot's `audio_in`/`audio_out` are
    /// measured against. Every shot already stamped is left exactly as it
    /// is: this only changes what a *later* `Self::set_shot_range` or
    /// `Self::move_cut` copies in.
    pub fn set_master_audio(&mut self, scene: SceneKey, layer: LayerId) {
        self.master_audio = Some((scene, layer));
    }

    /// The frame rate `audio_in`/`audio_out` are counted in: the frame rate
    /// of the scene the master audio itself lives on.
    fn master_fps(&self) -> Option<f64> {
        let (scene_key, _) = self.master_audio?;
        let index = self.index_of_key(scene_key)?;
        Some(self.scenes[index].scene.stage().frame_rate)
    }

    /// The master audio's own clip — its sample rate and duration are on the
    /// returned asset; `Self::master_audio_frames` converts its duration to
    /// frames at the document's own fps. `None` if no master audio is set, or
    /// the layer it names no longer carries a sound at frame 0.
    pub fn master_sound(&self) -> Option<std::sync::Arc<SoundAsset>> {
        let (scene_key, layer_id) = self.master_audio?;
        let index = self.index_of_key(scene_key)?;
        let scene = &self.scenes[index].scene;
        let reference = scene.frame_sound(layer_id, 0)?;
        scene.sounds().get(reference.sound).cloned()
    }

    /// How many frames long the master audio is, at the frame rate of the
    /// scene it lives on.
    pub fn master_audio_frames(&self) -> Option<u32> {
        let fps = self.master_fps()?;
        Some(self.master_sound()?.duration_frames(fps))
    }

    /// Copy the master audio's clip into `shot_index`'s own `SoundLibrary` —
    /// `Arc`-shared bytes, not a byte copy — reusing the copy already there
    /// if `layer_id`'s frame 0 already carries one, so retrimming a shot
    /// already stamped never piles up duplicate library entries. `None` if
    /// there is no master audio, or its own keyframe carries no sound.
    fn ensure_master_sound_copied(&mut self, shot_index: usize, layer_id: LayerId) -> Option<SoundId> {
        if let Some(existing) = self.scenes[shot_index].scene.frame_sound(layer_id, 0) {
            return Some(existing.sound);
        }

        let (master_scene_key, master_layer_id) = self.master_audio?;
        let master_index = self.index_of_key(master_scene_key)?;
        let master_reference = self.scenes[master_index]
            .scene
            .frame_sound(master_layer_id, 0)?;
        let asset = self.scenes[master_index]
            .scene
            .sounds()
            .get(master_reference.sound)?
            .clone();

        let shot_scene = &mut self.scenes[shot_index].scene;
        Some(shot_scene.add_sound(
            &asset.name,
            asset.data.clone(),
            &asset.format,
            asset.sample_rate,
            asset.channels,
            asset.length,
        ))
    }

    /// **The one thing ever rewritten on an existing shot**: bring its
    /// Dialogue layer in line with its current `audio_in`/`audio_out`.
    ///
    /// Makes the shot's Dialogue layer if it has none yet, copies the master
    /// clip into the shot's own `SoundLibrary` if it is not there already,
    /// and trims it to `audio_in..audio_out` of the master audio — so the
    /// shot's frame 0 plays the master at `audio_in`. The layer is stretched
    /// or shrunk to span exactly the shot's length (`audio_out - audio_in`);
    /// every *other* layer on the shot is only ever extended to that length
    /// by holding its last frame, never shortened — frames past a shorter
    /// end are kept, not deleted. See "The audio is the timeline" in
    /// `docs/SEQUENCE_SETUPS_PLAN.md`.
    ///
    /// Called by `Self::stamp_shot`, `Self::set_shot_range` and
    /// `Self::move_cut`. Does nothing if `shot` does not name a Shot scene.
    fn retrim_shot(&mut self, shot: SceneKey) {
        let Some(index) = self.index_of_key(shot) else {
            return;
        };
        let (audio_in, audio_out, dialogue_layer) = match &self.scenes[index].role {
            SceneRole::Shot(info) => (info.audio_in, info.audio_out, info.dialogue_layer),
            _ => return,
        };
        let target_len = audio_out.saturating_sub(audio_in).max(1);

        // Every other layer: extended to the new length by holding its last
        // frame if it is now shorter, never shortened if it is now longer —
        // nothing here ever deletes a frame of the animator's own work.
        let other_layer_ids: Vec<LayerId> = self.scenes[index]
            .scene
            .stage_layers()
            .iter()
            .filter(|l| Some(l.id) != dialogue_layer)
            .map(|l| l.id)
            .collect();
        for id in other_layer_ids {
            self.scenes[index].scene.update_stage_layer(id, |layer| {
                if layer.frames.length() < target_len {
                    layer.frames.insert_frame(target_len - 1);
                }
            });
        }

        // Nothing to trim to yet, and this shot has never had a Dialogue
        // layer of its own either: leave it be rather than adding an empty
        // layer with nothing to play. A shot stamped before `Set Master
        // Audio` simply grows one the next time its range is touched once a
        // master audio exists (`Self::set_shot_range`, `Self::move_cut`).
        if dialogue_layer.is_none() && self.master_audio.is_none() {
            return;
        }

        // The Dialogue layer itself, made fresh if this shot has none yet.
        let layer_id = match dialogue_layer {
            Some(id) => id,
            None => {
                let id = self.scenes[index]
                    .scene
                    .add_stage_layer("Dialogue", LayerKind::Normal);
                if let SceneRole::Shot(info) = &mut self.scenes[index].role {
                    info.dialogue_layer = Some(id);
                }
                id
            }
        };

        let Some(sound_id) = self.ensure_master_sound_copied(index, layer_id) else {
            // No master audio set yet: the Dialogue layer exists (so the UI
            // has somewhere to show it), spans the shot's length, but there
            // is nothing to trim it to.
            self.scenes[index].scene.update_stage_layer(layer_id, |l| {
                if l.frames.length() < target_len {
                    l.frames.insert_frame(target_len - 1);
                }
                while l.frames.length() > target_len {
                    l.frames.remove_frame(target_len);
                }
            });
            return;
        };

        let fps = self.master_fps().unwrap_or(24.0).max(1.0);
        let trim_start = audio_in as f64 / fps;
        let trim_end = audio_out as f64 / fps;
        let reference = SoundRef {
            sound: sound_id,
            sync: SoundSync::Stream,
            volume: 1.0,
            loops: 1,
            trim_start: trim_start as f32,
            trim_end: Some(trim_end as f32),
        };

        let scene = &mut self.scenes[index].scene;
        scene.update_stage_layer(layer_id, |l| {
            if l.frames.length() < target_len {
                l.frames.insert_frame(target_len - 1);
            }
            while l.frames.length() > target_len {
                l.frames.remove_frame(target_len);
            }
        });
        scene.set_frame_sound(layer_id, 0, Some(reference));
    }

    /// Change a shot's slice of the master audio directly (not by moving a
    /// cut against its neighbour). Retrims the Dialogue layer and updates
    /// the shot's playing length the same way `Self::move_cut` does for a
    /// boundary between two shots. `audio_out` is clamped to at least
    /// `audio_in + 1`, so the shot never drops to zero frames. Returns false
    /// if `shot` does not name a Shot scene.
    pub fn set_shot_range(&mut self, shot: SceneKey, audio_in: u32, audio_out: u32) -> bool {
        let Some(index) = self.index_of_key(shot) else {
            return false;
        };
        let audio_out = audio_out.max(audio_in.saturating_add(1));
        match &mut self.scenes[index].role {
            SceneRole::Shot(info) => {
                info.audio_in = audio_in;
                info.audio_out = audio_out;
            }
            _ => return false,
        }
        self.retrim_shot(shot);
        true
    }

    /// **Move a cut**: the boundary between two adjacent shots in
    /// `Self::sequence` moves to master frame `at`. `left`'s `audio_out` and
    /// `right`'s `audio_in` both become `at`, and both shots' Dialogue
    /// layers are retrimmed to match — the only thing this, or anything
    /// else, ever rewrites on either shot; see `Self::retrim_shot`. Clamped
    /// so neither shot drops below one frame. Returns false if either key
    /// does not name a Shot scene.
    pub fn move_cut(&mut self, left: SceneKey, right: SceneKey, at: u32) -> bool {
        let Some(left_index) = self.index_of_key(left) else {
            return false;
        };
        let Some(right_index) = self.index_of_key(right) else {
            return false;
        };
        let left_audio_in = match &self.scenes[left_index].role {
            SceneRole::Shot(info) => info.audio_in,
            _ => return false,
        };
        let right_audio_out = match &self.scenes[right_index].role {
            SceneRole::Shot(info) => info.audio_out,
            _ => return false,
        };

        let min_at = left_audio_in.saturating_add(1);
        let max_at = right_audio_out.saturating_sub(1);
        let at = if min_at <= max_at {
            at.clamp(min_at, max_at)
        } else {
            min_at
        };

        if let SceneRole::Shot(info) = &mut self.scenes[left_index].role {
            info.audio_out = at;
        }
        if let SceneRole::Shot(info) = &mut self.scenes[right_index].role {
            info.audio_in = at;
        }
        self.retrim_shot(left);
        self.retrim_shot(right);
        true
    }

    /// **Add Missing Setup Layers.** The one exception to "a shot never hears
    /// from its setup again": copies across only the setup's layers whose
    /// *names* are not already on the shot. Never replaces or removes
    /// anything the shot already has.
    ///
    /// Returns how many layers were added; `0` if `shot` is not a Shot, its
    /// setup is gone, or there was nothing missing.
    pub fn add_missing_setup_layers(&mut self, shot: SceneKey) -> usize {
        let Some(shot_index) = self.index_of_key(shot) else {
            return 0;
        };
        let setup_key = match &self.scenes[shot_index].role {
            SceneRole::Shot(info) => info.from_setup,
            _ => None,
        };
        let Some(setup_key) = setup_key else {
            return 0;
        };
        let Some(setup_index) = self.index_of_key(setup_key) else {
            return 0;
        };

        let existing_names: std::collections::HashSet<String> = self.scenes[shot_index]
            .scene
            .stage_layers()
            .iter()
            .map(|l| l.name.clone())
            .collect();

        // A pruned copy of the setup, holding only the layers the shot does
        // not already have by name. Merging that in (rather than the whole
        // setup) is what keeps this add-only: nothing already on the shot is
        // ever a target of the merge.
        let mut diff = self.scenes[setup_index].scene.clone();
        let present: Vec<LayerId> = diff
            .stage_layers()
            .iter()
            .filter(|l| existing_names.contains(&l.name))
            .map(|l| l.id)
            .collect();
        for id in present {
            // The document's own timeline specifically, not whatever symbol
            // happens to be open for editing on this cloned scene.
            diff.edit_stage_layers().remove(id);
        }
        if diff.stage_layers().is_empty() {
            return 0;
        }

        let report = self.scenes[shot_index]
            .scene
            .merge(&diff, ImportTarget::Stage);
        report.layers
    }

    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    /// Attach the file this document was translated from (builder style).
    pub fn with_origin(mut self, origin: impl AsRef<Path>) -> Self {
        self.origin = Some(origin.as_ref().to_path_buf());
        self
    }

    pub fn origin(&self) -> Option<&Path> {
        self.origin.as_deref()
    }

    /// Name for the title bar, without the extension or the dirty marker:
    /// the stem of `path`, else the stem of `origin`, else `Untitled`.
    pub fn title(&self) -> String {
        self.path
            .as_ref()
            .or(self.origin.as_ref())
            .and_then(|p| p.file_stem())
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "Untitled".to_string())
    }

    /// Has the document changed since it was last saved?
    /// Treat the document as it stands as unmodified.
    ///
    /// For a document that was *made* rather than opened: a new file is not a
    /// changed one, and there is nothing to save yet. Without this a brand new
    /// document reports unsaved changes from the moment it appears, because
    /// building a scene bumps its revision.
    pub fn mark_clean(&mut self) {
        for slot in &mut self.scenes {
            let revision = slot.scene.revision();
            slot.history.mark_saved(revision);
        }
    }

    pub fn is_dirty(&self) -> bool {
        self.scenes
            .iter()
            .any(|s| s.history.is_dirty(s.scene.revision()))
    }

    /// Name for the title bar, with Animate's asterisk for unsaved changes.
    pub fn display_name(&self) -> String {
        let base = self.title();
        if self.is_dirty() {
            format!("{base}*")
        } else {
            base
        }
    }

    /// Apply an edit, recording it for undo.
    ///
    /// The single entry point for mutation. `label` names the step in the
    /// History panel and drives coalescing: consecutive edits sharing a label
    /// collapse into one undo step, so a drag is one Ctrl+Z rather than two
    /// hundred.
    pub fn edit(&mut self, label: impl Into<UndoLabel>, f: impl FnOnce(&mut Scene)) {
        let slot = &mut self.scenes[self.active];
        let before = slot.scene.clone();
        f(&mut slot.scene);

        // Nothing actually changed, so there is nothing to undo.
        if slot.scene.revision() == before.revision() {
            return;
        }
        slot.history.record(before, label);
    }

    /// Change view state on the scene without recording an undo step.
    ///
    /// Only one thing qualifies: which symbol is open for editing. It lives on
    /// [`Scene`] so that every panel and tool sees the same answer, but opening
    /// a symbol is navigation rather than an edit — it must not mark the
    /// document dirty, and Ctrl+Z must not take you back out of it.
    ///
    /// Nothing here may touch the artwork. `Scene`'s navigation methods do not
    /// bump the revision, so an accidental artwork change made through this
    /// path would go unrecorded; a debug assertion catches that.
    pub fn edit_view(&mut self, f: impl FnOnce(&mut Scene)) {
        let slot = &mut self.scenes[self.active];
        let before = slot.scene.revision();
        f(&mut slot.scene);
        debug_assert_eq!(
            before,
            slot.scene.revision(),
            "edit_view changed the document; use edit() so the change can be undone"
        );
    }

    /// End the current gesture so the next edit starts a fresh undo step.
    pub fn end_gesture(&mut self) {
        self.active_slot_mut().history.break_coalescing();
    }

    pub fn can_undo(&self) -> bool {
        self.active_slot().history.can_undo()
    }

    /// Remember where the active scene's history stands, to collapse a run of
    /// edits into one undo step afterwards with [`Self::collapse_run`].
    pub fn run_checkpoint(&self) -> RunCheckpoint {
        let slot = self.active_slot();
        RunCheckpoint {
            uid: self.uid,
            key: slot.key,
            history: slot.history.clone(),
            scene: slot.scene.clone(),
        }
    }

    /// Fold everything done to the checkpointed scene since `checkpoint` into
    /// one undo step named `label`. Returns false (and touches nothing) if the
    /// document was replaced or the scene is gone.
    pub fn collapse_run(&mut self, checkpoint: RunCheckpoint, label: impl Into<UndoLabel>) -> bool {
        if checkpoint.uid != self.uid {
            return false;
        }
        let Some(index) = self.index_of_key(checkpoint.key) else {
            return false;
        };
        let slot = &mut self.scenes[index];
        let changed = slot.history.edit_count() != checkpoint.history.edit_count()
            || slot.scene.revision() != checkpoint.scene.revision();
        slot.history
            .collapse_run(&checkpoint.history, checkpoint.scene, label, changed);
        true
    }

    pub fn can_redo(&self) -> bool {
        self.active_slot().history.can_redo()
    }

    pub fn undo(&mut self) -> bool {
        let slot = &mut self.scenes[self.active];
        match slot.history.undo(slot.scene.clone()) {
            Some(scene) => {
                slot.scene = scene;
                true
            }
            None => false,
        }
    }

    pub fn redo(&mut self) -> bool {
        let slot = &mut self.scenes[self.active];
        match slot.history.redo(slot.scene.clone()) {
            Some(scene) => {
                slot.scene = scene;
                true
            }
            None => false,
        }
    }

    /// Save to the current path. Fails if there is none — the caller should
    /// present Save As instead.
    pub fn save(&mut self) -> Result<(), DocError> {
        let Some(path) = self.path.clone() else {
            return Err(DocError::Io(std::io::Error::other(
                "this document has never been saved; use Save As",
            )));
        };
        self.save_as(path)
    }

    /// Save to `path` and adopt it as the document's location.
    pub fn save_as(&mut self, path: impl AsRef<Path>) -> Result<(), DocError> {
        let path = path.as_ref().to_path_buf();
        {
            let records: Vec<format::SceneRecord> = self
                .scenes
                .iter()
                .map(|s| format::SceneRecord {
                    name: s.name.as_str(),
                    scene: &s.scene,
                    key: s.key,
                    role: s.role.clone(),
                })
                .collect();
            format::save_scenes_full(
                &records,
                self.active,
                &self.sequence,
                self.master_audio,
                &path,
            )?;
        }

        self.mark_clean();
        self.autosave.retarget(&path);
        // The recovery copy is now redundant, and leaving it would prompt an
        // unnecessary recovery on next launch.
        let _ = self.autosave.discard_recovery();
        self.path = Some(path);
        Ok(())
    }

    /// Forget where this document came from: it becomes untitled.
    ///
    /// For a recovered autosave. The recovery file is *evidence of a crash*,
    /// not a document the user chose to keep, and Save must ask where to put
    /// the result rather than writing back over it — and the new document's
    /// own autosave must go somewhere else, or it would overwrite the very
    /// file it was recovered from.
    pub fn forget_path(&mut self) {
        self.path = None;
        self.origin = None;
        self.autosave
            .reset_to_untitled(crate::autosave::recovery_dir());
    }

    /// Build an autosave job if one is due.
    ///
    /// Returns a `Send` plan rather than writing here, so the caller can hand
    /// it to the background pool and keep editing.
    pub fn autosave_plan(&mut self) -> Option<AutosavePlan> {
        let revision = self.combined_revision();
        let refs: Vec<(&str, &Scene)> = self
            .scenes
            .iter()
            .map(|s| (s.name.as_str(), &s.scene))
            .collect();
        let meta = self.sequence_meta();
        self.autosave
            .plan_scenes(&refs, self.active, revision)
            .map(|plan| plan.with_sequence(meta))
    }

    /// A single fingerprint over every scene, so any edit anywhere makes an
    /// autosave due and an untouched document stays quiet.
    pub fn combined_revision(&self) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        self.scenes.len().hash(&mut hasher);
        for slot in &self.scenes {
            slot.scene.revision().hash(&mut hasher);
        }
        hasher.finish()
    }

    /// Remember the whole document for crash recovery. A crash then restores
    /// every scene, not only the one on screen.
    pub fn remember_for_crash(&self) {
        let refs: Vec<(&str, &Scene)> = self
            .scenes
            .iter()
            .map(|s| (s.name.as_str(), &s.scene))
            .collect();
        let plan = crate::autosave::crash_plan(
            &refs,
            self.active,
            self.combined_revision(),
            self.recovery_path(),
        );
        crate::autosave::remember_plan_for_crash(plan.with_sequence(self.sequence_meta()));
    }

    /// Setups, shots and the cut, for a recovery copy to carry.
    fn sequence_meta(&self) -> crate::autosave::SequenceMeta {
        crate::autosave::SequenceMeta {
            keys: self.scenes.iter().map(|s| s.key).collect(),
            roles: self.scenes.iter().map(|s| s.role.clone()).collect(),
            sequence: self.sequence.clone(),
            master_audio: self.master_audio,
        }
    }

    /// Where this document's recovery file would be written.
    pub fn recovery_path(&self) -> PathBuf {
        self.autosave.recovery_path()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use buzz_geom::Shape as _;
    use buzz_scene::{LayerKind, ShapeData};
    use kurbo::Rect;
    use peniko::Color;

    fn add_shape(scene: &mut Scene) {
        let layer = scene
            .layers()
            .iter()
            .next()
            .map(|l| l.id)
            .expect("a document always has a layer");
        scene.add_shape(
            layer,
            ShapeData::filled(Rect::new(0.0, 0.0, 10.0, 10.0).to_path(1e-9), Color::WHITE),
        );
    }

    #[test]
    fn a_new_document_is_clean_and_named_untitled() {
        let doc = Document::default();
        assert!(!doc.can_undo());
        assert_eq!(doc.display_name(), "Untitled*");
        assert!(doc.is_dirty(), "an unsaved document counts as dirty");
    }

    #[test]
    fn editing_records_an_undo_step() {
        let mut doc = Document::default();
        doc.edit("Draw", add_shape);

        assert!(doc.can_undo());
        assert_eq!(doc.history().next_undo_label(), Some("Draw"));
        assert_eq!(doc.scene().shape_count(), 1);

        assert!(doc.undo());
        assert_eq!(doc.scene().shape_count(), 0);
        assert!(doc.redo());
        assert_eq!(doc.scene().shape_count(), 1);
    }

    /// **A document keeps several scenes**, each with its own artwork, undo
    /// history and name — and switching between them is not an edit.
    #[test]
    fn scenes_are_independent_and_switching_is_not_an_edit() {
        let mut doc = Document::default();
        doc.edit("Draw", add_shape); // one shape on Scene 1
        assert_eq!(doc.scene_names(), vec!["Scene 1"]);

        doc.add_scene(); // now on Scene 2, empty
        assert_eq!(doc.active_scene(), 1);
        assert_eq!(doc.scene_names(), vec!["Scene 1", "Scene 2"]);
        assert_eq!(doc.scene().shape_count(), 0, "a new scene starts empty");

        doc.edit("Draw", add_shape);
        doc.edit("Draw", add_shape); // two shapes on Scene 2

        // Each scene has its own undo stack.
        assert!(doc.can_undo());
        doc.switch_scene(0);
        assert_eq!(doc.scene().shape_count(), 1, "Scene 1 is untouched");

        doc.switch_scene(1);
        assert_eq!(doc.scene().shape_count(), 2, "Scene 2 kept its own artwork");
    }

    /// Every scene survives a save and reopen, in order, with its name.
    #[test]
    fn scenes_round_trip_through_a_saved_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("film.buzz");

        let mut doc = Document::default();
        doc.edit("Draw", add_shape);
        doc.add_scene();
        doc.rename_scene(1, "Chase");
        doc.edit("Draw", add_shape);
        doc.edit("Draw", add_shape);
        doc.save_as(&path).unwrap();

        let reopened = Document::open(&path).unwrap();
        assert_eq!(reopened.scene_names(), vec!["Scene 1", "Chase"]);
        assert_eq!(reopened.active_scene(), 1, "the active scene is remembered");
        assert_eq!(reopened.scene().shape_count(), 2);
    }

    /// **Duplicating a scene copies the whole thing.** The operation a
    /// conversation is built out of: the next beat is the same room and the
    /// same two people, so it starts as a copy rather than as an empty stage
    /// or as frames pasted onto the end of the shot before it.
    #[test]
    fn duplicating_a_scene_copies_all_of_it_and_opens_the_copy() {
        let mut doc = Document::default();
        doc.rename_scene(0, "Kitchen");
        doc.edit("Draw", add_shape);
        doc.edit("Draw", add_shape);

        doc.duplicate_scene(0);

        assert_eq!(doc.active_scene(), 1, "the copy is opened for editing");
        assert_eq!(
            doc.scene_names(),
            vec!["Kitchen", "Kitchen 2"],
            "and is named after what it came from, next to it in the order"
        );
        assert_eq!(
            doc.scene().shape_count(),
            2,
            "the copy carries the whole set, not an empty stage"
        );

        // The two are now separate: changing the copy leaves the original.
        doc.edit("Draw", add_shape);
        assert_eq!(doc.scene().shape_count(), 3);
        doc.switch_scene(0);
        assert_eq!(doc.scene().shape_count(), 2, "the original is untouched");
    }

    /// The copy gets its **own** history. Sharing one would let an undo in the
    /// new shot reach back and rewrite the shot it was copied from.
    #[test]
    fn a_duplicated_scene_has_its_own_undo_history() {
        let mut doc = Document::default();
        doc.edit("Draw", add_shape);
        doc.duplicate_scene(0);

        // Fresh history: there is nothing of the original's past to undo.
        assert!(
            !doc.can_undo(),
            "the copy starts with a clean history of its own"
        );

        doc.edit("Draw", add_shape);
        assert!(doc.undo(), "and undoes its own edits");
        assert_eq!(doc.scene().shape_count(), 1);
        doc.switch_scene(0);
        assert_eq!(doc.scene().shape_count(), 1, "the original never moved");
    }

    /// Scenes are the order the film plays in, so that order has to be
    /// changeable.
    #[test]
    fn scenes_can_be_reordered() {
        let mut doc = Document::default();
        doc.rename_scene(0, "Opening");
        doc.add_scene();
        doc.rename_scene(1, "Chase");
        doc.add_scene();
        doc.rename_scene(2, "Ending");
        assert_eq!(doc.scene_names(), vec!["Opening", "Chase", "Ending"]);

        doc.move_scene(2, 0);
        assert_eq!(doc.scene_names(), vec!["Ending", "Opening", "Chase"]);
        assert_eq!(doc.active_scene(), 0, "the moved scene is followed");

        // Nonsense is refused rather than panicking or reshuffling.
        doc.move_scene(0, 9);
        doc.move_scene(9, 0);
        doc.move_scene(1, 1);
        assert_eq!(doc.scene_names(), vec!["Ending", "Opening", "Chase"]);
    }

    /// Duplicated scenes survive the file, in order, like any other.
    #[test]
    fn a_duplicated_scene_round_trips_through_a_saved_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("conversation.buzz");

        let mut doc = Document::default();
        doc.rename_scene(0, "Kitchen");
        doc.edit("Draw", add_shape);
        doc.duplicate_scene(0);
        doc.edit("Draw", add_shape);
        doc.save_as(&path).unwrap();

        let reopened = Document::open(&path).unwrap();
        assert_eq!(reopened.scene_names(), vec!["Kitchen", "Kitchen 2"]);
        assert_eq!(reopened.scene().shape_count(), 2);
    }

    /// The last scene cannot be deleted — a document always has one.
    #[test]
    fn the_final_scene_cannot_be_deleted() {
        let mut doc = Document::default();
        doc.delete_scene(0);
        assert_eq!(doc.scene_names().len(), 1, "a document is never scene-less");
    }

    /// An edit that changes nothing should not clutter the History panel.
    #[test]
    fn a_no_op_edit_records_nothing() {
        let mut doc = Document::default();
        doc.edit("Nothing", |_scene| {});
        assert!(
            !doc.can_undo(),
            "an edit that changed nothing must not create an undo step"
        );
    }

    #[test]
    fn a_drag_collapses_into_a_single_undo_step() {
        let mut doc = Document::default();
        for _ in 0..50 {
            doc.edit("Move", add_shape);
        }
        assert_eq!(doc.history().undo_depth(), 1);

        doc.end_gesture();
        doc.edit("Move", add_shape);
        assert_eq!(doc.history().undo_depth(), 2, "a new gesture is separate");
    }

    #[test]
    fn saving_and_reopening_preserves_the_document() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("drawing.buzz");

        let mut doc = Document::default();
        doc.edit("Draw", add_shape);
        doc.edit("Add layer", |s| {
            s.add_layer("Second", LayerKind::Normal);
        });
        doc.save_as(&path).unwrap();

        assert!(!doc.is_dirty(), "saving should clear the dirty flag");
        assert_eq!(doc.display_name(), "drawing");

        let reopened = Document::open(&path).unwrap();
        assert_eq!(reopened.scene().shape_count(), doc.scene().shape_count());
        assert_eq!(reopened.scene().layers().len(), doc.scene().layers().len());
        assert!(!reopened.is_dirty());
        assert!(
            !reopened.can_undo(),
            "a freshly opened document has no history"
        );
    }

    #[test]
    fn origin_names_an_unsaved_document() {
        let doc = Document::default().with_origin("C:/x/bhatta2.fla");
        assert!(doc.path().is_none());
        assert_eq!(doc.origin(), Some(Path::new("C:/x/bhatta2.fla")));
        assert_eq!(doc.title(), "bhatta2");
    }

    #[test]
    fn saved_path_wins_over_origin() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("drawing.buzz");

        let mut doc = Document::default().with_origin("C:/x/bhatta2.fla");
        doc.save_as(&path).unwrap();

        assert_eq!(doc.title(), "drawing");
        assert_eq!(
            doc.origin(),
            Some(Path::new("C:/x/bhatta2.fla")),
            "save_as leaves origin alone"
        );
    }

    #[test]
    fn editing_after_saving_marks_the_document_dirty_again() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("drawing.buzz");

        let mut doc = Document::default();
        doc.save_as(&path).unwrap();
        assert!(!doc.is_dirty());

        doc.edit("Draw", add_shape);
        assert!(doc.is_dirty());
        assert!(doc.display_name().ends_with('*'));

        // And undoing back to the saved state should read clean again.
        doc.undo();
        assert!(!doc.is_dirty(), "undo back to the saved revision is clean");
    }

    #[test]
    fn saving_without_a_path_asks_for_save_as() {
        let mut doc = Document::default();
        assert!(
            doc.save().is_err(),
            "an unsaved document has nowhere to save"
        );
    }

    #[test]
    fn save_then_save_again_uses_the_same_path() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("drawing.buzz");

        let mut doc = Document::default();
        doc.save_as(&path).unwrap();
        doc.edit("Draw", add_shape);
        doc.save().unwrap();

        assert_eq!(Document::open(&path).unwrap().scene().shape_count(), 1);
    }

    #[test]
    fn a_manual_save_clears_the_recovery_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("drawing.buzz");

        let mut doc = Document::default();
        doc.save_as(&path).unwrap();
        doc.edit("Draw", add_shape);

        // Force an autosave.
        if let Some(plan) = doc.autosave_plan() {
            plan.write().unwrap();
        }
        doc.save().unwrap();

        assert!(
            find_recoveries(dir.path()).is_empty(),
            "saving should remove the now-redundant recovery file"
        );
    }

    #[test]
    fn a_full_edit_save_crash_recover_cycle_works() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("drawing.buzz");

        // Work, save, then work more without saving.
        let mut doc = Document::default();
        doc.edit("Draw", add_shape);
        doc.save_as(&path).unwrap();
        for _ in 0..3 {
            doc.end_gesture();
            doc.edit("Draw", add_shape);
        }
        assert_eq!(doc.scene().shape_count(), 4);

        // Autosave catches the unsaved work, then the process "crashes".
        let plan = doc.autosave_plan().expect("a save should be due");
        plan.write().unwrap();
        drop(doc);

        // On restart the recovery is offered and holds the later work, while
        // the saved document still holds only what was explicitly saved.
        let recoveries = find_recoveries(dir.path());
        assert_eq!(recoveries.len(), 1);
        assert_eq!(recoveries[0].document.as_deref(), Some(path.as_path()));

        assert_eq!(format::load(&recoveries[0].path).unwrap().shape_count(), 4);
        assert_eq!(Document::open(&path).unwrap().scene().shape_count(), 1);
    }

    // --- Sequence: setups and shots (docs/SEQUENCE_SETUPS_PLAN.md) ---------

    /// **Roles, stable keys and the sequence's cut survive a save and
    /// reopen** through `Document::save_as`/`Document::open` themselves, not
    /// just the raw archive functions underneath them.
    #[test]
    fn document_roles_keys_and_sequence_round_trip_through_save_and_open() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("village.buzz");

        let mut doc = Document::default();
        doc.rename_scene(0, "Village");
        doc.set_role(0, SceneRole::Setup);
        let setup_key = doc.scene_key(0).unwrap();

        doc.add_scene();
        doc.rename_scene(1, "Village – Shot 1");
        let shot_key = doc.scene_key(1).unwrap();
        doc.set_role(
            1,
            SceneRole::Shot(ShotInfo {
                from_setup: Some(setup_key),
                setup_revision: 2,
                audio_in: 10,
                audio_out: 70,
                take_of: None,
                continue_from: None,
                dialogue_layer: None,
            }),
        );
        doc.set_sequence(vec![shot_key]);
        doc.save_as(&path).unwrap();

        let reopened = Document::open(&path).unwrap();
        assert_eq!(reopened.scene_key(0), Some(setup_key));
        assert_eq!(reopened.scene_key(1), Some(shot_key));
        assert_eq!(reopened.index_of_key(shot_key), Some(1));
        assert_eq!(reopened.role(0), Some(&SceneRole::Setup));
        match reopened.role(1) {
            Some(SceneRole::Shot(info)) => {
                assert_eq!(info.from_setup, Some(setup_key));
                assert_eq!(info.setup_revision, 2);
                assert_eq!(info.audio_in, 10);
                assert_eq!(info.audio_out, 70);
            }
            other => panic!("expected a Shot, got {other:?}"),
        }
        assert_eq!(reopened.sequence(), &[shot_key]);
    }

    /// **A crash must not turn a sequence into a pile of plain scenes.** The
    /// recovery copy — autosave and the every-frame crash guard both write it
    /// through `AutosavePlan` — carries every setup, shot and the cut, not only
    /// the scene names.
    #[test]
    fn a_recovery_copy_keeps_setups_shots_and_the_cut() {
        let mut doc = Document::default();
        doc.rename_scene(0, "Village");
        doc.set_role(0, SceneRole::Setup);
        let setup = doc.scene_key(0).unwrap();
        let shot = doc.stamp_shot(setup, 12, 60).expect("the setup exists");

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("recovery.buzz");
        let refs: Vec<(&str, &Scene)> =
            doc.scenes.iter().map(|s| (s.name.as_str(), &s.scene)).collect();
        crate::autosave::crash_plan(&refs, doc.active, 1, path.clone())
            .with_sequence(doc.sequence_meta())
            .write()
            .unwrap();

        let back = format::load_scenes_full(&path).unwrap();
        assert_eq!(back.roles[0], SceneRole::Setup);
        let at = back.keys.iter().position(|k| *k == shot).expect("the shot survived");
        match &back.roles[at] {
            SceneRole::Shot(info) => {
                assert_eq!((info.audio_in, info.audio_out), (12, 60));
                assert_eq!(info.from_setup, Some(setup));
            }
            other => panic!("the shot came back as {other:?}"),
        }
        assert_eq!(back.sequence, vec![shot]);
    }

    /// A document that never heard of setups and shots saves and reopens
    /// exactly as it always did: one Normal scene, key 0, no sequence.
    #[test]
    fn a_plain_document_opens_with_key_zero_and_a_normal_role() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("plain.buzz");

        let mut doc = Document::default();
        doc.edit("Draw", add_shape);
        doc.save_as(&path).unwrap();

        let reopened = Document::open(&path).unwrap();
        assert_eq!(reopened.scene_key(0), Some(SceneKey(0)));
        assert_eq!(reopened.role(0), Some(&SceneRole::Normal));
        assert!(reopened.sequence().is_empty());
    }

    /// **Stamping a shot copies the setup**, and from that moment editing the
    /// setup — adding art, moving a layer, changing frames — never touches
    /// the shot again.
    #[test]
    fn editing_a_setup_after_stamping_leaves_the_shot_untouched() {
        let mut doc = Document::default();
        doc.rename_scene(0, "Village");
        doc.set_role(0, SceneRole::Setup);
        doc.edit("Draw", add_shape);
        let setup_key = doc.scene_key(0).unwrap();

        let shot_key = doc.stamp_shot(setup_key, 10, 40).expect("the setup exists");
        let shot_index = doc.index_of_key(shot_key).unwrap();
        let shot_before = doc.film()[shot_index].clone();

        // Edit the setup: add art, move a layer, change frames.
        doc.switch_scene(0);
        doc.edit("Add art", add_shape);
        doc.edit("Add layer", |scene| {
            scene.add_layer("Extra", LayerKind::Normal);
        });
        doc.edit("Move a layer", |scene| {
            let ids: Vec<_> = scene.stage_layers().iter().map(|l| l.id).collect();
            scene.reorder_layer(ids[0], ids.len() - 1);
        });
        doc.edit("Change frames", |scene| {
            let id = scene.stage_layers().iter().next().unwrap().id;
            scene.update_stage_layer(id, |l| {
                l.frames.insert_frame(l.frames.length());
            });
        });

        let shot_after = doc.film()[shot_index].clone();
        assert_eq!(
            shot_after, shot_before,
            "editing the setup must never change a shot already stamped from it"
        );
    }

    /// **Stamping the same setup twice, at different audio ranges, gives two
    /// independent shots** — editing one never touches the other.
    #[test]
    fn stamping_twice_at_different_audio_ranges_gives_two_independent_shots() {
        let mut doc = Document::default();
        doc.rename_scene(0, "Village");
        doc.set_role(0, SceneRole::Setup);
        doc.edit("Draw", add_shape);
        let setup_key = doc.scene_key(0).unwrap();

        let shot_a = doc.stamp_shot(setup_key, 0, 20).unwrap();
        let shot_b = doc.stamp_shot(setup_key, 20, 50).unwrap();
        assert_ne!(shot_a, shot_b);
        assert_eq!(doc.sequence(), &[shot_a, shot_b]);

        let index_a = doc.index_of_key(shot_a).unwrap();
        let index_b = doc.index_of_key(shot_b).unwrap();

        match doc.role(index_a) {
            Some(SceneRole::Shot(info)) => assert_eq!((info.audio_in, info.audio_out), (0, 20)),
            other => panic!("expected a Shot, got {other:?}"),
        }
        match doc.role(index_b) {
            Some(SceneRole::Shot(info)) => assert_eq!((info.audio_in, info.audio_out), (20, 50)),
            other => panic!("expected a Shot, got {other:?}"),
        }

        doc.switch_scene(index_a);
        doc.edit("Draw", add_shape);
        assert_eq!(doc.film()[index_a].shape_count(), 2);
        assert_eq!(
            doc.film()[index_b].shape_count(),
            1,
            "stamping twice must give independent shots"
        );
    }

    /// **Duplicate Shot** makes an alternative take that leaves the original
    /// untouched, and not in the cut until `set_active_take` puts it there.
    #[test]
    fn duplicate_shot_makes_a_take_that_leaves_the_original_untouched() {
        let mut doc = Document::default();
        doc.rename_scene(0, "Village");
        doc.set_role(0, SceneRole::Setup);
        doc.edit("Draw", add_shape);
        let setup_key = doc.scene_key(0).unwrap();
        let shot_key = doc.stamp_shot(setup_key, 0, 20).unwrap();

        let take_key = doc.duplicate_shot(shot_key).expect("the shot exists");
        assert_ne!(take_key, shot_key);
        assert_eq!(
            doc.sequence(),
            &[shot_key],
            "a fresh take is not in the cut until it is made the active one"
        );

        let shot_index = doc.index_of_key(shot_key).unwrap();
        let take_index = doc.index_of_key(take_key).unwrap();

        doc.switch_scene(take_index);
        doc.edit("Draw", add_shape);
        assert_eq!(doc.film()[take_index].shape_count(), 2);
        assert_eq!(
            doc.film()[shot_index].shape_count(),
            1,
            "duplicating a shot must leave the original untouched"
        );

        match doc.role(take_index) {
            Some(SceneRole::Shot(info)) => assert_eq!(info.take_of, Some(shot_key)),
            other => panic!("expected a Shot, got {other:?}"),
        }

        doc.set_active_take(shot_key, take_key);
        assert_eq!(doc.sequence(), &[take_key]);
        assert!(
            doc.index_of_key(shot_key).is_some(),
            "the take that left the cut still exists"
        );
    }

    /// **Add Missing Setup Layers** only adds what the shot lacks by name,
    /// and does nothing once there is nothing left to add.
    #[test]
    fn add_missing_setup_layers_only_adds_what_is_missing_by_name() {
        let mut doc = Document::default();
        doc.rename_scene(0, "Village");
        doc.set_role(0, SceneRole::Setup);
        doc.edit("Add layer", |scene| {
            scene.add_layer("Background", LayerKind::Normal);
        });
        let setup_key = doc.scene_key(0).unwrap();
        let shot_key = doc.stamp_shot(setup_key, 0, 10).unwrap();

        // The setup gains another layer after the shot was already stamped.
        doc.switch_scene(0);
        doc.edit("Add layer", |scene| {
            scene.add_layer("Extra Prop", LayerKind::Normal);
        });

        let shot_index = doc.index_of_key(shot_key).unwrap();
        assert_eq!(doc.film()[shot_index].layers().len(), 2);

        let added = doc.add_missing_setup_layers(shot_key);
        assert_eq!(added, 1, "only the layer the shot lacks should be added");

        let names: Vec<String> = doc.film()[shot_index]
            .layers()
            .iter()
            .map(|l| l.name.clone())
            .collect();
        assert_eq!(names.len(), 3);
        assert!(names.contains(&"Layer_1".to_string()));
        assert!(names.contains(&"Background".to_string()));
        assert!(names.contains(&"Extra Prop".to_string()));

        assert_eq!(
            doc.add_missing_setup_layers(shot_key),
            0,
            "nothing left missing, so a second call adds nothing"
        );
    }

    // --- "The audio is the timeline" (docs/SEQUENCE_SETUPS_PLAN.md) --------

    /// Puts a clip on the active scene's stage timeline and points the
    /// document's master audio at it. `frames` is how long the clip covers
    /// at the scene's own frame rate (24fps by default) — long enough to
    /// hold every `audio_in`/`audio_out` a test slices out of it.
    fn stub_master_audio(doc: &mut Document, frames: u32) -> LayerId {
        let mut layer_id = None;
        doc.edit("Add master audio", |scene| {
            let layer = scene.add_stage_layer("Master Audio", LayerKind::Normal);
            let sound = scene.add_sound(
                "Dialogue Track",
                std::sync::Arc::new(vec![0u8; 16]),
                "wav",
                24,
                1,
                frames as u64,
            );
            scene.update_stage_layer(layer, |l| {
                if l.frames.length() < frames {
                    l.frames.insert_frame(frames - 1);
                }
            });
            scene.set_frame_sound(layer, 0, Some(SoundRef::stream(sound)));
            layer_id = Some(layer);
        });
        let scene_key = doc.scene_key(doc.active_scene()).unwrap();
        let layer_id = layer_id.unwrap();
        doc.set_master_audio(scene_key, layer_id);
        layer_id
    }

    /// **A stamped shot's Dialogue plays the master audio from `audio_in`** —
    /// frame 0 of the shot is `audio_in` of the master, which is the whole
    /// point of "the audio is the timeline".
    #[test]
    fn stamped_shot_dialogue_plays_master_from_audio_in() {
        let mut doc = Document::default();
        stub_master_audio(&mut doc, 1000);

        doc.add_scene();
        doc.rename_scene(1, "Village");
        doc.set_role(1, SceneRole::Setup);
        let setup_key = doc.scene_key(1).unwrap();

        let shot_key = doc.stamp_shot(setup_key, 30, 80).expect("the setup exists");
        let shot_index = doc.index_of_key(shot_key).unwrap();

        let dialogue_layer = match doc.role(shot_index) {
            Some(SceneRole::Shot(info)) => {
                info.dialogue_layer.expect("stamping makes a Dialogue layer")
            }
            other => panic!("expected a Shot, got {other:?}"),
        };

        let shot_scene = &doc.film()[shot_index];
        let sound_ref = shot_scene
            .frame_sound(dialogue_layer, 0)
            .expect("the Dialogue layer's frame 0 carries the master clip");
        assert_eq!(sound_ref.sync, SoundSync::Stream);
        assert!(
            (sound_ref.trim_start - 30.0 / 24.0).abs() < 1e-4,
            "trim_start should be audio_in in seconds, got {}",
            sound_ref.trim_start
        );
        assert!(
            (sound_ref.trim_end.unwrap() - 80.0 / 24.0).abs() < 1e-4,
            "trim_end should be audio_out in seconds, got {:?}",
            sound_ref.trim_end
        );
        let dialogue_len = shot_scene.layers().get(dialogue_layer).unwrap().frames.length();
        assert_eq!(dialogue_len, 50, "the Dialogue layer spans exactly the shot's length");
    }

    /// **The same setup stamped twice, at different audio ranges, plays a
    /// different moment of the master at each shot's frame 0.**
    #[test]
    fn frame_one_of_two_shots_of_the_same_setup_maps_to_different_master_times() {
        let mut doc = Document::default();
        stub_master_audio(&mut doc, 1000);

        doc.add_scene();
        doc.rename_scene(1, "Village");
        doc.set_role(1, SceneRole::Setup);
        let setup_key = doc.scene_key(1).unwrap();

        let shot_a = doc.stamp_shot(setup_key, 0, 20).unwrap();
        let shot_b = doc.stamp_shot(setup_key, 50, 90).unwrap();

        let trim_of = |doc: &Document, shot: SceneKey| -> f32 {
            let index = doc.index_of_key(shot).unwrap();
            let layer = match doc.role(index) {
                Some(SceneRole::Shot(info)) => info.dialogue_layer.unwrap(),
                other => panic!("expected a Shot, got {other:?}"),
            };
            doc.film()[index].frame_sound(layer, 0).unwrap().trim_start
        };

        let trim_a = trim_of(&doc, shot_a);
        let trim_b = trim_of(&doc, shot_b);
        assert!((trim_a - 0.0).abs() < 1e-4);
        assert!((trim_b - 50.0 / 24.0).abs() < 1e-4);
        assert_ne!(trim_a, trim_b, "frame 1 of each shot is a different master moment");
    }

    /// **Moving a cut retrims both shots and never touches anything but their
    /// Dialogue layer** — a shot's own artwork keeps every frame it had, even
    /// past a newly shortened end, and a shot that grows only ever has its
    /// other layers extended by holding, never a keyframe removed.
    #[test]
    fn move_cut_retrims_both_shots_and_leaves_other_layers_untouched() {
        let mut doc = Document::default();
        stub_master_audio(&mut doc, 1000);

        doc.add_scene();
        doc.rename_scene(1, "Village");
        doc.set_role(1, SceneRole::Setup);
        doc.edit("Draw", add_shape); // one distinguishing shape on the setup's own layer
        let setup_key = doc.scene_key(1).unwrap();
        let setup_layer = doc.film()[1].stage_layers().iter().next().unwrap().id;

        let shot_a = doc.stamp_shot(setup_key, 0, 30).unwrap();
        let shot_b = doc.stamp_shot(setup_key, 30, 80).unwrap();

        let index_a = doc.index_of_key(shot_a).unwrap();
        let index_b = doc.index_of_key(shot_b).unwrap();

        // Both shots' non-Dialogue ("Layer_1") layer was already extended to
        // the shot's own length at stamp time (the setup's own layer was
        // only 1 frame long).
        let layer_before_a = doc.film()[index_a]
            .stage_layers()
            .get(setup_layer)
            .unwrap()
            .as_ref()
            .clone();
        assert_eq!(layer_before_a.frames.length(), 30);

        // Move the boundary from 30 down to 10: shot A shrinks to 10 frames,
        // shot B grows from 50 to 70.
        assert!(doc.move_cut(shot_a, shot_b, 10));

        match doc.role(index_a) {
            Some(SceneRole::Shot(info)) => assert_eq!((info.audio_in, info.audio_out), (0, 10)),
            other => panic!("expected a Shot, got {other:?}"),
        }
        match doc.role(index_b) {
            Some(SceneRole::Shot(info)) => assert_eq!((info.audio_in, info.audio_out), (10, 80)),
            other => panic!("expected a Shot, got {other:?}"),
        }

        // Shot A's own artwork layer is untouched, byte-identical, even
        // though the shot itself is now only 10 frames — the 30 frames the
        // stamp originally extended it to are all still there.
        let layer_after_a = doc.film()[index_a]
            .stage_layers()
            .get(setup_layer)
            .unwrap()
            .as_ref()
            .clone();
        assert_eq!(
            layer_before_a, layer_after_a,
            "a shortened shot must not lose or change any frame of its own artwork"
        );
        assert_eq!(
            layer_after_a.frames.length(),
            30,
            "frames past the new, shorter end are kept, not deleted"
        );

        // Shot B's own artwork layer only ever grows by holding its last
        // frame — its single keyframe's objects are unchanged.
        let film = doc.film();
        let layer_after_b = film[index_b].stage_layers().get(setup_layer).unwrap();
        assert_eq!(layer_after_b.frames.length(), 70);
        assert_eq!(layer_after_b.frames.keyframe_count(), 1);

        // Both Dialogue layers were retrimmed to the new boundary.
        let dialogue_a = match doc.role(index_a) {
            Some(SceneRole::Shot(info)) => info.dialogue_layer.unwrap(),
            _ => unreachable!(),
        };
        let dialogue_b = match doc.role(index_b) {
            Some(SceneRole::Shot(info)) => info.dialogue_layer.unwrap(),
            _ => unreachable!(),
        };
        let ref_a = doc.film()[index_a].frame_sound(dialogue_a, 0).unwrap();
        let ref_b = doc.film()[index_b].frame_sound(dialogue_b, 0).unwrap();
        assert!((ref_a.trim_end.unwrap() - 10.0 / 24.0).abs() < 1e-4);
        assert!((ref_b.trim_start - 10.0 / 24.0).abs() < 1e-4);
        assert!((ref_b.trim_end.unwrap() - 80.0 / 24.0).abs() < 1e-4);
        assert_eq!(
            doc.film()[index_a].layers().get(dialogue_a).unwrap().frames.length(),
            10
        );
        assert_eq!(
            doc.film()[index_b].layers().get(dialogue_b).unwrap().frames.length(),
            70
        );
    }

    /// **`playing_length` and `film_order`/`film_with_lengths`**: a Shot's
    /// playing length is its slice of the master audio, not its own frame
    /// count; a Setup never appears in the film; shots play in `sequence`
    /// order. See `docs/SEQUENCE_SETUPS_PLAN.md` S6.
    #[test]
    fn film_with_lengths_skips_setups_and_uses_each_shots_playing_length() {
        let mut doc = Document::default();
        stub_master_audio(&mut doc, 1000);

        doc.add_scene();
        doc.rename_scene(1, "Village");
        doc.set_role(1, SceneRole::Setup);
        let setup_key = doc.scene_key(1).unwrap();

        let shot_a = doc.stamp_shot(setup_key, 0, 30).unwrap();
        let shot_b = doc.stamp_shot(setup_key, 30, 80).unwrap();
        let index_setup = doc.index_of_key(setup_key).unwrap();
        let index_a = doc.index_of_key(shot_a).unwrap();
        let index_b = doc.index_of_key(shot_b).unwrap();

        assert_eq!(doc.playing_length(index_a), 30, "audio_out - audio_in for shot A");
        assert_eq!(doc.playing_length(index_b), 50, "audio_out - audio_in for shot B");
        // A Normal scene plays its own full (rendered) length, exactly as
        // before there was such a thing as a Shot.
        let normal_length = doc.film()[0].rendered_frame_count();
        assert_eq!(doc.playing_length(0), normal_length);

        assert!(
            !doc.film_order().contains(&index_setup),
            "the setup itself never plays"
        );
        assert_eq!(
            doc.film_order(),
            vec![0, index_a, index_b],
            "the master's Normal scene, then the shots in sequence order"
        );

        let lengths: Vec<u32> = doc.film_with_lengths().iter().map(|(_, l)| *l).collect();
        assert_eq!(lengths, vec![normal_length, 30, 50]);
    }

    /// **Save, then open**: a shot's `dialogue_layer` and its trim survive.
    #[test]
    fn dialogue_layer_and_trims_round_trip_through_save_and_open() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("village.buzz");

        let mut doc = Document::default();
        stub_master_audio(&mut doc, 1000);

        doc.add_scene();
        doc.rename_scene(1, "Village");
        doc.set_role(1, SceneRole::Setup);
        let setup_key = doc.scene_key(1).unwrap();
        let shot_key = doc.stamp_shot(setup_key, 15, 45).unwrap();
        let shot_index = doc.index_of_key(shot_key).unwrap();
        let dialogue_layer = match doc.role(shot_index) {
            Some(SceneRole::Shot(info)) => info.dialogue_layer.unwrap(),
            other => panic!("expected a Shot, got {other:?}"),
        };

        doc.save_as(&path).unwrap();
        let reopened = Document::open(&path).unwrap();
        let reopened_index = reopened.index_of_key(shot_key).unwrap();

        match reopened.role(reopened_index) {
            Some(SceneRole::Shot(info)) => {
                assert_eq!(info.dialogue_layer, Some(dialogue_layer));
                assert_eq!((info.audio_in, info.audio_out), (15, 45));
            }
            other => panic!("expected a Shot, got {other:?}"),
        }

        let sound_ref = reopened.film()[reopened_index]
            .frame_sound(dialogue_layer, 0)
            .expect("the Dialogue layer's sound survives the round trip");
        assert!((sound_ref.trim_start - 15.0 / 24.0).abs() < 1e-4);
        assert!((sound_ref.trim_end.unwrap() - 45.0 / 24.0).abs() < 1e-4);
    }
}
