//! The editor's sound: what is loaded, what is audible, and when.
//!
//! # The rule this module exists to keep
//!
//! **The document's soundtrack plays wherever you are working.** Step into a
//! character symbol to animate its walk and the dialogue keeps sounding; step
//! from there into the head symbol to animate the mouth and it *still* keeps
//! sounding, at the same frame, from the same take.
//!
//! That falls out of one decision, made in one place:
//! [`buzz_scene::Scene::stage_cues`] reads the document's own timeline rather
//! than the timeline being edited. Everything here simply hands those cues to
//! the player and never asks what symbol is open. There is no code path that
//! could get it wrong, because there is no code path that knows.
//!
//! # Decoded audio is cached, not stored
//!
//! The document keeps sounds as the files they came from. Playing them needs
//! samples, and decoding a three-minute MP3 takes long enough to hear as a
//! stutter, so the decoded form is cached here — outside the document, keyed
//! by sound id, shared with the audio thread by `Arc`.

use std::collections::BTreeMap;
use std::sync::Arc;

use buzz_audio::{Clip, Player};
use buzz_scene::{Scene, SoundId};

/// One sound waiting to be decoded, owned so it can cross to a worker thread.
pub struct Undecoded {
    pub id: SoundId,
    pub data: Arc<Vec<u8>>,
    pub name: String,
}

/// Decode one queued sound. Pure and self-contained — a whole batch runs in
/// parallel with no shared state, which is what lets [`SoundBank::take_undecoded`]
/// hand its result to a rayon `par_iter` off the UI thread.
pub fn decode_undecoded(u: &Undecoded) -> (SoundId, Result<Clip, String>) {
    if u.data.is_empty() {
        return (
            u.id,
            Err("the audio is missing from this document".to_string()),
        );
    }
    match Clip::decode(&u.data, &u.name) {
        Ok(clip) => (u.id, Ok(clip)),
        Err(e) => (u.id, Err(format!("{e:#}"))),
    }
}

/// Everything the editor needs to make a document audible.
pub struct SoundBank {
    /// Decoded audio, keyed by the document's sound id.
    clips: BTreeMap<SoundId, Arc<Clip>>,
    /// Sounds that would not decode, so the failure is reported once rather
    /// than retried on every frame.
    failed: BTreeMap<SoundId, String>,
    player: Player,
    /// The revision the player's cues were built from, so they are rebuilt
    /// when the document changes and not on every frame.
    cued_revision: Option<u64>,
}

impl SoundBank {
    pub fn new(fps: f64) -> Self {
        Self {
            clips: BTreeMap::new(),
            failed: BTreeMap::new(),
            player: Player::new(fps),
            cued_revision: None,
        }
    }

    pub fn player(&self) -> &Player {
        &self.player
    }

    pub fn player_mut(&mut self) -> &mut Player {
        &mut self.player
    }

    /// A decoded clip, if it is loaded.
    pub fn clip(&self, id: SoundId) -> Option<&Arc<Clip>> {
        self.clips.get(&id)
    }

    /// Why a sound is silent, if it is.
    pub fn failure(&self, id: SoundId) -> Option<&str> {
        self.failed.get(&id).map(|s| s.as_str())
    }

    pub fn is_empty(&self) -> bool {
        self.clips.is_empty()
    }

    /// Decode anything in the document that is not decoded yet.
    ///
    /// Cheap to call every frame: it does nothing once the document's sounds
    /// are loaded, and a sound that failed is not retried.
    pub fn sync_with(&mut self, scene: &Scene) {
        for asset in scene.sounds().iter() {
            if self.clips.contains_key(&asset.id) || self.failed.contains_key(&asset.id) {
                continue;
            }
            if asset.data.is_empty() {
                // A document whose media entry was missing: the sound is
                // known, named and referenced, but has no audio to play.
                self.failed.insert(
                    asset.id,
                    "the audio is missing from this document".to_string(),
                );
                continue;
            }

            match Clip::decode(&asset.data, &asset.name) {
                Ok(clip) => {
                    tracing::info!(
                        "decoded {} ({:.2}s, {} Hz)",
                        asset.name,
                        clip.duration_seconds(),
                        clip.sample_rate
                    );
                    self.clips.insert(asset.id, Arc::new(clip));
                }
                Err(e) => {
                    tracing::warn!("could not decode {}: {e:#}", asset.name);
                    self.failed.insert(asset.id, format!("{e:#}"));
                }
            }
        }

        // Drop clips whose sound has been deleted, so an undo that removes a
        // three-minute take gives the memory back.
        self.clips.retain(|id, _| scene.sounds().get(*id).is_some());
        self.failed
            .retain(|id, _| scene.sounds().get(*id).is_some());
    }

    /// Rebuild what the player will sound, from the **document's** timeline.
    ///
    /// Called whenever the document changes. It deliberately never consults
    /// the edit path: see this module's documentation.
    pub fn rebuild_cues(&mut self, scene: &Scene) {
        let cues = scene
            .stage_cues()
            .into_iter()
            .filter_map(|cue| {
                let clip = self.clips.get(&cue.sound)?;
                Some(buzz_audio::player::Cue {
                    clip: Arc::clone(clip),
                    start_frame: cue.start_frame,
                    volume: cue.volume,
                    // Stop never reaches the player at all — `stage_cues`
                    // drops it — so the remaining three map straight across.
                    sync: match cue.sync {
                        buzz_scene::SoundSync::Stream => buzz_audio::player::CueSync::Stream,
                        buzz_scene::SoundSync::Start => buzz_audio::player::CueSync::Start,
                        buzz_scene::SoundSync::Event | buzz_scene::SoundSync::Stop => {
                            buzz_audio::player::CueSync::Event
                        }
                    },
                })
            })
            .collect();

        self.player.set_fps(scene.stage().frame_rate);
        self.player.set_cues(cues);
        self.cued_revision = Some(scene.revision());
    }

    /// Rebuild the cues if the document has moved on since they were built.
    ///
    /// **The synchronous path.** Decodes any new sound inline, so a caller that
    /// is about to play or analyse audio has it ready. Used for user-initiated,
    /// one-or-few-sound actions — importing a sound, attaching one, pressing
    /// play, running lip sync — and by the tests. The **per-frame** refresh the
    /// window does is [`Self::refresh_cues`], which never decodes; see there.
    pub fn refresh(&mut self, scene: &Scene) {
        if self.cued_revision != Some(scene.revision()) {
            self.sync_with(scene);
            self.rebuild_cues(scene);
        }
    }

    /// Rebuild the cues from **already-decoded** clips — no decoding here.
    ///
    /// This is what the window calls every frame. Decoding a large document's
    /// sounds inline on the frame that first sees them was a freeze that scaled
    /// with the soundtrack; that work now happens off the UI thread (the App
    /// drives [`Self::take_undecoded`] → [`Self::install_decoded`]). Until a clip
    /// lands its layer simply has no waveform and its cue no sound, which is a
    /// blank frame or two, not a hang.
    pub fn refresh_cues(&mut self, scene: &Scene) {
        if self.cued_revision != Some(scene.revision()) {
            self.clips.retain(|id, _| scene.sounds().get(*id).is_some());
            self.failed.retain(|id, _| scene.sounds().get(*id).is_some());
            self.rebuild_cues(scene);
        }
    }

    /// The document's sounds that are neither decoded nor known-bad, ready to be
    /// decoded off the UI thread.
    ///
    /// The audio bytes are shared by `Arc`, so this is pointer copies, not a
    /// copy of the audio.
    pub fn take_undecoded(&self, scene: &Scene) -> Vec<Undecoded> {
        scene
            .sounds()
            .iter()
            .filter(|asset| {
                !self.clips.contains_key(&asset.id) && !self.failed.contains_key(&asset.id)
            })
            .map(|asset| Undecoded {
                id: asset.id,
                data: Arc::clone(&asset.data),
                name: asset.name.clone(),
            })
            .collect()
    }

    /// Put decoded clips (and decode failures) built off-thread back into the
    /// bank, then rebuild the cues — the document revision has *not* moved since
    /// they were queued, so [`Self::refresh_cues`] would not do it.
    ///
    /// A sound deleted while it was decoding is dropped rather than installed.
    pub fn install_decoded(
        &mut self,
        results: Vec<(SoundId, Result<Clip, String>)>,
        scene: &Scene,
    ) {
        for (id, result) in results {
            if scene.sounds().get(id).is_none() {
                continue;
            }
            match result {
                Ok(clip) => {
                    self.clips.insert(id, Arc::new(clip));
                }
                Err(why) => {
                    self.failed.insert(id, why);
                }
            }
        }
        self.clips.retain(|id, _| scene.sounds().get(*id).is_some());
        self.failed.retain(|id, _| scene.sounds().get(*id).is_some());
        self.rebuild_cues(scene);
    }

    /// Start playing from `frame`.
    pub fn play(&mut self, scene: &Scene, frame: u32) {
        self.refresh(scene);
        if !self.player.has_sound() {
            return;
        }
        if let Err(e) = self.player.play(frame) {
            tracing::warn!("could not start audio: {e:#}");
        }
    }

    pub fn stop(&mut self) {
        self.player.pause();
    }

    /// Move the sound to a frame without starting it.
    pub fn seek(&mut self, frame: u32) {
        self.player.seek(frame);
    }

    /// Where the audio has actually reached, while playing.
    ///
    /// The playhead follows *this* rather than the other way round: the audio
    /// clock is the one the audience hears, and a dropped video frame must not
    /// nudge the dialogue.
    pub fn playing_frame(&self) -> Option<u32> {
        self.player
            .is_playing()
            .then(|| self.player.position_frame())?
    }

    /// The clip a document's first streamed sound refers to — what lip sync
    /// analyses and what the timeline draws a waveform for.
    ///
    /// "The root audio layer", in the user's terms: it comes from the stage
    /// timeline whatever symbol is open, so lip sync inside a character's head
    /// symbol still works from the dialogue on the root.
    pub fn stage_track(&self, scene: &Scene) -> Option<(SoundId, u32, Arc<Clip>)> {
        scene
            .stage_cues()
            .into_iter()
            .find(|cue| cue.sync.follows_timeline())
            .or_else(|| scene.stage_cues().into_iter().next())
            .and_then(|cue| {
                let clip = self.clips.get(&cue.sound)?;
                Some((cue.sound, cue.start_frame, Arc::clone(clip)))
            })
    }
}

impl std::fmt::Debug for SoundBank {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SoundBank")
            .field("clips", &self.clips.len())
            .field("failed", &self.failed.len())
            .field("player", &self.player)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use buzz_scene::{LayerKind, SoundRef, SymbolKind};

    /// A one-second tone as a real WAV file, which is what an import produces.
    fn wav(seconds: f64) -> Vec<u8> {
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate: 44_100,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut out = std::io::Cursor::new(Vec::new());
        {
            let mut writer = hound::WavWriter::new(&mut out, spec).expect("writer");
            let frames = (seconds * 44_100.0) as usize;
            for i in 0..frames {
                let t = i as f64 / 44_100.0;
                let v = (t * 440.0 * std::f64::consts::TAU).sin() * 0.6;
                writer.write_sample((v * 32_000.0) as i16).expect("sample");
            }
            writer.finalize().expect("finalize");
        }
        out.into_inner()
    }

    /// A document with dialogue on the root timeline and a character symbol
    /// containing a head symbol — the arrangement the feature is for.
    fn document_with_dialogue() -> (Scene, SoundId, buzz_scene::SymbolId, buzz_scene::SymbolId) {
        let mut scene = Scene::default();

        let sound = scene.add_sound("Dialogue", Arc::new(wav(1.0)), "wav", 44_100, 1, 44_100);

        // The audio layer, on the root.
        let audio_layer = scene.add_layer("Audio", LayerKind::Normal);
        assert!(scene.set_frame_sound(audio_layer, 0, Some(SoundRef::stream(sound))));

        // A character on the root, with a head symbol inside it.
        let head = scene.add_symbol("Head", SymbolKind::MovieClip, None);
        let character = scene.add_symbol("Character", SymbolKind::MovieClip, None);
        let character_layer = scene
            .library()
            .get(character)
            .expect("the character")
            .layers
            .iter()
            .next()
            .expect("a layer")
            .id;
        scene.library_mut().update(character, |symbol| {
            symbol.layers.update(character_layer, |layer| {
                layer.frames.set_objects(
                    0,
                    vec![Arc::new(buzz_scene::Object {
                        id: buzz_scene::ObjectId(500),
                        name: None,
                        transform: buzz_geom::Affine::IDENTITY,
                        kind: buzz_scene::ObjectKind::Instance(buzz_scene::SymbolInstance::new(
                            head,
                        )),
                        locked: false,
                        visible: true,
                        filters: Vec::new(),
                        blend: Default::default(),
                        spatial: Default::default(),
                        pivot: None,
                        modifiers: Vec::new(),
                        text: None,
                    })],
                );
            });
        });

        let stage_layer = scene.layers().iter().next().expect("a layer").id;
        scene.add_instance_at(stage_layer, 0, character, buzz_geom::Affine::IDENTITY);

        (scene, sound, character, head)
    }

    #[test]
    fn a_documents_sounds_are_decoded_once() {
        let (scene, sound, _, _) = document_with_dialogue();
        let mut bank = SoundBank::new(24.0);

        bank.sync_with(&scene);
        let clip = bank.clip(sound).expect("the dialogue should have decoded");
        assert!((clip.duration_seconds() - 1.0).abs() < 0.05);
        assert!(bank.failure(sound).is_none());

        // Again: no work, same clip.
        let pointer = Arc::as_ptr(clip);
        bank.sync_with(&scene);
        assert_eq!(
            Arc::as_ptr(bank.clip(sound).expect("still there")),
            pointer,
            "the clip should not have been decoded twice"
        );
    }

    /// The per-frame path decodes **nothing** — that is what keeps a big
    /// import's soundtrack off the UI thread. `refresh_cues` reports the audio
    /// as still undecoded, and only `install_decoded` (fed by the background
    /// worker) brings a clip to life. This is the pin for plan 1.3.
    #[test]
    fn the_per_frame_path_never_decodes_and_the_background_install_does() {
        let (scene, sound, _, _) = document_with_dialogue();
        let mut bank = SoundBank::new(24.0);

        // The per-frame refresh must not decode.
        bank.refresh_cues(&scene);
        assert!(
            bank.clip(sound).is_none(),
            "refresh_cues must not decode on the UI thread"
        );

        // The document reports exactly the one undecoded sound, ready to hand to
        // a worker thread.
        let batch = bank.take_undecoded(&scene);
        assert_eq!(batch.len(), 1);
        assert_eq!(batch[0].id, sound);

        // Decoding (as the worker would) and installing brings it to life.
        let results = batch.iter().map(decode_undecoded).collect();
        bank.install_decoded(results, &scene);
        assert!(
            bank.clip(sound).is_some(),
            "the installed clip should be playable"
        );
        // And its cue is rebuilt even though the revision never moved.
        assert!(bank.player().has_sound(), "the cue should be live");

        // Now the sound is decoded, so nothing is left to hand out.
        assert!(bank.take_undecoded(&scene).is_empty());
    }

    /// **The request, tested.** The root audio is cued the same wherever the
    /// user is working: on the root, inside the character, and inside the head
    /// symbol nested within it.
    #[test]
    fn root_audio_is_cued_the_same_inside_nested_symbols() {
        let (mut scene, sound, character, head) = document_with_dialogue();
        let mut bank = SoundBank::new(24.0);
        bank.sync_with(&scene);

        let cues_on_root = scene.stage_cues();
        assert_eq!(cues_on_root.len(), 1, "the dialogue should be cued");
        assert_eq!(cues_on_root[0].sound, sound);

        // Step inside the character.
        assert!(scene.enter_symbol(character));
        assert_eq!(
            scene.stage_cues(),
            cues_on_root,
            "the root dialogue must still be cued inside the character"
        );

        // And inside the head, nested one deeper.
        assert!(scene.enter_symbol(head));
        assert_eq!(scene.edit_path().len(), 2, "two symbols deep");
        assert_eq!(
            scene.stage_cues(),
            cues_on_root,
            "the root dialogue must still be cued two symbols deep"
        );

        // The bank agrees: the same track is the one to animate against.
        let (track, start, clip) = bank.stage_track(&scene).expect("a track");
        assert_eq!(track, sound);
        assert_eq!(start, 0);
        assert!(clip.duration_seconds() > 0.9);

        // Back on the root, nothing has changed.
        scene.edit_document();
        assert_eq!(scene.stage_cues(), cues_on_root);
    }

    /// A sound on a layer inside a *symbol* is not the document's soundtrack:
    /// it belongs to that symbol's own timeline and does not play on the root.
    #[test]
    fn a_sound_inside_a_symbol_is_not_part_of_the_stage_track() {
        let (mut scene, _, character, _) = document_with_dialogue();
        let effect = scene.add_sound("Effect", Arc::new(wav(0.2)), "wav", 44_100, 1, 8_820);

        let inner_layer = scene
            .library()
            .get(character)
            .expect("the character")
            .layers
            .iter()
            .next()
            .expect("a layer")
            .id;
        scene.library_mut().update(character, |symbol| {
            symbol.layers.update(inner_layer, |layer| {
                if let Some(keyframe) = layer.frames.keyframe_at_mut(0) {
                    keyframe.sound = Some(SoundRef::event(effect));
                }
            });
        });

        let cues = scene.stage_cues();
        assert_eq!(cues.len(), 1, "only the root's dialogue plays: {cues:?}");
        assert_ne!(cues[0].sound, effect);
    }

    #[test]
    fn cues_are_rebuilt_when_the_document_changes() {
        let (mut scene, _, _, _) = document_with_dialogue();
        let mut bank = SoundBank::new(24.0);
        bank.refresh(&scene);
        assert!(bank.player().has_sound());

        // Remove the sound from its keyframe: the player should fall silent.
        let audio_layer = scene
            .layers()
            .iter()
            .find(|l| l.name == "Audio")
            .expect("the audio layer")
            .id;
        scene.set_frame_sound(audio_layer, 0, None);

        bank.refresh(&scene);
        assert!(
            !bank.player().has_sound(),
            "the player should have nothing left to sound"
        );
    }

    #[test]
    fn a_sound_that_cannot_be_decoded_is_reported_once_and_not_retried() {
        let mut scene = Scene::default();
        let id = scene.add_sound(
            "Broken",
            Arc::new(b"this is not audio".to_vec()),
            "wav",
            44_100,
            1,
            100,
        );

        let mut bank = SoundBank::new(24.0);
        bank.sync_with(&scene);

        assert!(bank.clip(id).is_none());
        assert!(bank.failure(id).is_some(), "the failure should be recorded");

        // A second pass must not try again — and must not panic.
        bank.sync_with(&scene);
        assert!(bank.failure(id).is_some());
    }

    /// A document saved without its media entry keeps the sound, its name and
    /// every keyframe that refers to it, and simply plays nothing.
    #[test]
    fn a_sound_with_no_audio_is_silent_rather_than_lost() {
        let mut scene = Scene::default();
        let id = scene.add_sound("Missing", Arc::new(Vec::new()), "wav", 44_100, 1, 44_100);

        let mut bank = SoundBank::new(24.0);
        bank.sync_with(&scene);

        assert!(bank.clip(id).is_none());
        assert!(bank.failure(id).is_some());
        assert_eq!(scene.sounds().len(), 1, "the sound is still in the library");
    }

    #[test]
    fn deleting_a_sound_gives_its_memory_back() {
        let (mut scene, sound, _, _) = document_with_dialogue();
        let mut bank = SoundBank::new(24.0);
        bank.sync_with(&scene);
        assert!(bank.clip(sound).is_some());

        scene.sounds_mut().remove(sound);
        bank.sync_with(&scene);
        assert!(
            bank.clip(sound).is_none(),
            "the decoded audio should be gone"
        );
        assert!(bank.is_empty());
    }
}
