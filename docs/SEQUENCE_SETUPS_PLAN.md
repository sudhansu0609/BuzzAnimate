# Sequence: reusable setups, shots cut on the dialogue (2026-09-27)

## What the user asked (paraphrased)

A conversation between a few characters: shot 1 has character A facing camera,
shot 2 character B, shot 3 the village, shot 4 back to the village, and so on.
Don't make me copy/paste frames to set scenes up again. The **same setup should
be reusable**. The **audio is the timeline**: each shot starts at a specific
point in the audio, and that point is **frame 1 of that shot** — so a
character's frame 1 in shot 3 is a different moment of the audio than their
frame 1 in shot 1. Keep a **master scene** that works exactly like Animate, where
none of these rules apply. Improve on the idea, plan it, build it.

## Design

Three kinds of scene in one document (scenes already exist: `Document`
`SceneSlot`s, the scene menu, `buzz_export::Reel` plays them in order):

| Kind | What it is | In the exported film? |
|---|---|---|
| **Normal** (the master) | Today's scene. Nothing linked, no rules. | Yes, in scene order, as now |
| **Setup** | A reusable arrangement: background, props, lights, camera framing, characters placed. Its own timeline holds ambient motion (live modifiers, loops). | **No**: it's a source, not a shot |
| **Shot** | One cut of the film: *which setup* + *which stretch of the master audio*. Frame 1 = the audio in-point. | Yes, in sequence order |

### Setups are stamped, never linked (user correction, 2026-09-27)

> User: "I want exactly what is said. I want to use the same setup at different
> audio timings to generate different animation and still be able to see and
> use the previous one. Don't want to overwrite any."

So there is **no live link** and **no propagation**. That supersedes an
earlier draft of this plan, which had linked layers that a setup edit would
rewrite.

- **New Shot from Setup** stamps a complete, independent copy of the setup's
  layers into a new Shot scene. The copy is cheap because layers are `Arc`, and
  it is copy-on-write per layer from then on. From that moment the shot owns
  everything in it.
- Editing a setup affects **only shots made from it afterwards**. Existing
  shots are never modified by anything except direct edits made in that shot.
- The same setup can be stamped any number of times at different audio slices.
  Each shot's animation (lip-sync, acting) is generated in its own shot time,
  frame 1 = its `audio_in`. Every earlier shot stays in the sequence, openable,
  editable, and exported.
- Each shot records `from_setup` (and the setup's revision it was stamped
  from), for display only: the strip's colour and label. The single explicit
  exception is **Shot ▸ Add Missing Setup Layers**, which only *adds* layers the
  setup has gained since the stamp. It never replaces or deletes anything.
- **Nothing destructive by timing either**: moving a cut or shortening a shot
  changes only its `audio_in/out` and playing length. Frames beyond the new end
  are kept (not played, shown greyed past the end marker) and return if the shot
  is lengthened. Frame 1 always stays frame 1.
- **Duplicate Shot** makes an alternative take of a shot: same slice, its own
  copy, so a new version never overwrites the old one. The strip can switch
  which take is in the cut.

### The audio is the timeline

- **Master audio**: one dialogue/soundtrack for the whole sequence, imported
  once (Sequence ▸ Set Master Audio…).
- Each shot carries `audio_in`/`audio_out` (frames on the master). Its
  **Dialogue** layer is a linked-to-master sound keyframe trimmed to that slice
  (sound in/out points already exist: `Waveform::level_start/level_end`), so
  the shot's frame 1 plays the master at `audio_in`. The shot's length =
  `audio_out − audio_in`.
- **Moving a cut** (dragging the boundary between shots 2 and 3) retrims both
  shots' audio and lengths. The shots' local animation keeps its shot-relative
  timing, so frame 1 is still frame 1.

### The Sequence strip (new panel, docks at the bottom beside the Timeline)

The master audio's waveform across the whole film, with the shots as coloured
blocks on it (colour = setup, label = setup name). You can:
- click a block to open that shot; the playhead moves in film time;
- press **Split at Playhead** (S) to cut a shot in two;
- drag a boundary to move a cut;
- pick a shot's setup from a dropdown (village → Ana close-up), or make a new
  setup from the current shot;
- read a ruler showing film timecode, while the Timeline shows shot frames.

### Improvements over the original idea

1. **Cut on the lines.** *Sequence ▸ Cut on Dialogue* uses the existing phrase
   detector (`buzz_audio::detect_phrases`, used by Fit to Narration) to propose
   one shot per spoken line. Assign a setup per line and a conversation's
   shot/reverse-shot is laid out in seconds. With a subtitle/SRT file that
   names speakers (already importable), each line goes straight to that
   speaker's setup.
2. **Lip-sync per shot, automatically.** Auto lip-sync on a shot reads *its*
   audio slice and keys the character's mouth from shot frame 1. Only this shot
   changes, and the setup and every other shot are untouched.
3. **Continue performance.** Cutting away and back (village, Ana, **village**)
   usually wants the second village shot to pick up where the first left off.
   The per-shot option *Continue from shot N* seeds this shot's local layers with
   shot N's last pose at frame 1.
4. **One character, everywhere.** Characters are Cast symbols, so a wardrobe or
   rig fix propagates through every setup and shot. (This already holds for
   symbols. Stamping copies the *placement* of the Cast symbol, not the symbol.
   Note: this is the one thing shared across shots: a character's **drawing**
   (the symbol), as in Animate. Animation is never shared.)
5. **Bake / hand-off.** *Sequence ▸ Flatten to Master Scene* makes a **new** scene that turns the sequence into one ordinary Animate-style scene (frames
   pasted end to end, which is the thing the user no longer wants to do by
   hand), for XFL/Animate hand-off or a final hand-polish.
6. **The master stays plain.** Normal scenes never get links, chain icons or
   audio rules. Mixing is allowed: a normal title scene, then the sequence.

## Model and format

- `Document`: stable `SceneKey(u32)` per scene (serialized; old files get keys
  by index). `SceneRole { Normal, Setup, Shot(ShotInfo) }`, with
  `ShotInfo { setup: SceneKey, audio_in: u32, audio_out: u32, continue_from:
  Option<SceneKey> }`. Document-level `master_audio: Option<SoundRef>` (the same
  sound reference type layers use) and `sequence: Vec<SceneKey>` (shot order).
  Serialized with serde defaults, so old files are all Normal.
- No per-layer link. `ShotInfo` gains `from_setup: SceneKey`, `setup_revision: u64`, `takes` handled by duplicate shots with `take_of: Option<SceneKey>` and `ShotInfo.active_take`.
- **Stamping** (`buzz-doc`): `Document::stamp_shot(setup, audio_in, audio_out) -> SceneKey`, a new scene whose layers are clones of the setup's, with frames extended to the shot length (holding the last frame). One undo step at document level. Setup edits never touch shots.
- Export (`Reel`): Setup scenes are skipped. Shots play in `sequence` order at
  their position among normal scenes.

## Stages

| # | Stage | Status |
|---|---|---|
| S1 | Model + serial: SceneKey, SceneRole, ShotInfo (from_setup, revision, audio_in/out, take_of), master_audio, sequence; round-trip + old-file tests | DONE — crates/buzz-doc/src/sequence.rs (types), lib.rs (Document fields/accessors), format.rs (`ScenesDto` manifest fields, `SceneRecord`/`LoadedScenes`, `*_full` read/write fns); tests: format::tests::roles_keys_sequence_and_master_audio_round_trip_through_the_container, format::tests::an_old_manifest_with_only_names_opens_as_all_normal_with_keys_by_position, tests::document_roles_keys_and_sequence_round_trip_through_save_and_open, tests::a_plain_document_opens_with_key_zero_and_a_normal_role. `master_audio` stored as `Option<(SceneKey, LayerId)>` — see notes below. |
| S2 | Stamping: New Shot from Setup (independent copy), Duplicate Shot (take), Add Missing Setup Layers (add-only); test that editing a setup leaves existing shots byte-identical | DONE — crates/buzz-doc/src/lib.rs (`stamp_shot`, `duplicate_shot`, `set_active_take`, `add_missing_setup_layers`); tests: tests::editing_a_setup_after_stamping_leaves_the_shot_untouched, tests::stamping_twice_at_different_audio_ranges_gives_two_independent_shots, tests::duplicate_shot_makes_a_take_that_leaves_the_original_untouched, tests::add_missing_setup_layers_only_adds_what_is_missing_by_name. `add_missing_setup_layers` reuses `buzz_scene::Scene::merge` on a name-pruned clone of the setup for safe id remapping; it currently lands on whichever timeline is open for editing on the destination shot (an existing `merge()` limitation, not new here). |
| S3 | Audio: master audio, per-shot trimmed Dialogue layer, frame 1 = audio_in, moving a cut retrims both shots; tests | DONE — crates/buzz-doc/src/sequence.rs (`ShotInfo.dialogue_layer: Option<LayerId>`, `#[serde(default)]`); lib.rs (`Document::set_master_audio`, `master_audio` getter, `master_sound`, `master_audio_frames`, `master_fps`, `ensure_master_sound_copied`, `retrim_shot`, `set_shot_range`, `move_cut`; `stamp_shot` now calls `retrim_shot`); format.rs and the `document_roles_keys_and_sequence_round_trip_through_save_and_open` test literal updated for the new field. Tests (crates/buzz-doc/src/lib.rs): tests::stamped_shot_dialogue_plays_master_from_audio_in, tests::frame_one_of_two_shots_of_the_same_setup_maps_to_different_master_times, tests::move_cut_retrims_both_shots_and_leaves_other_layers_untouched, tests::dialogue_layer_and_trims_round_trip_through_save_and_open. A shot with no `dialogue_layer` and no master audio set gets no Dialogue layer at all (nothing to trim to yet) — it grows one the next time `set_shot_range`/`move_cut` touches it once a master audio exists; this kept S2's layer-count tests (e.g. `add_missing_setup_layers_only_adds_what_is_missing_by_name`) passing unchanged. |
| S4 | Commands: New Setup (from current scene), New Shot from Setup, Try Another Setup (stamps a NEW take from a different setup; the old take is kept), Split at Playhead, Edit Setup, Duplicate Shot, Add Missing Setup Layers, Flatten to Master Scene (a copy; the sequence is kept) | DONE — crates/buzz-ui/src/command.rs (`Command::{SetMasterAudio, SetMasterAudioFromLayer, MakeSetupFromScene, NewShotFromSetup, SplitAtPlayhead, TryAnotherSetup, DuplicateShot, AddMissingSetupLayers, EditSetup, FlattenToMasterScene}`, labels, palette entries for the payload-free ones); crates/buzz-app/src/editor.rs (new "sequence" section: `set_master_audio_from_file`/`_from_layer`, `make_setup_from_scene`, `new_shot_from_setup` (stamps at the film playhead \u{2192} remaining master audio, or past the cut so far), `film_playhead`/`scrub_film_to`/`shot_at_film_frame`, `split_shot_at_playhead` (retrims the left shot via `set_shot_range`, stamps the right fresh from the same setup, reorders `sequence` to put it right after), `try_another_setup` (stamps, marks `take_of`, swaps into the original's slot), `duplicate_shot`, `add_missing_setup_layers`, `edit_setup`, `flatten_to_master_scene` (new Normal scene; each shot's layers `Scene::merge`d in and shifted/trimmed to its own stretch via `LayerTimeline::from_parts`; `sequence` untouched), `playing_length`/`shot_end_marker`, `sequence_setups`/`sequence_menu_info`/`sequence_view`/`apply_sequence`); `Command::run` dispatch and `App::dispatch`'s file-dialog interception (`Pick::SetMasterAudio`) in editor.rs/app.rs. Every command here is bookkeeping about the document (roles, sequence, master audio pointer) rather than artwork inside a scene — like `set_role`/`AddScene`/`DuplicateScene` before them, none of it goes through `Document::edit`, so none of it is on the undo stack; each still writes its own status message. Tests: crates/buzz-app/tests/sequence_setups.rs (`new_shot_from_setup_stamps_a_shot_spanning_the_master_audio`, `split_at_playhead_keeps_the_left_shots_layers_and_stamps_a_fresh_right_shot`, `try_another_setup_stamps_a_new_take_and_keeps_the_old_one`, `flatten_to_master_scene_makes_a_new_normal_scene_and_leaves_the_sequence_unchanged`, `playback_stops_at_the_shots_playing_length_not_the_scenes_frame_count`). |
| S5 | UI: Sequence strip panel (shots on the master waveform, takes switcher, kept-frames past end greyed); scene menu shows kinds | DONE — crates/buzz-ui/src/sequence_panel.rs (new: `SequenceView`/`ShotBlock`/`SequenceResponse`/`sequence_panel` — master waveform, coloured/labelled shot blocks, active-shot outline, takes switcher (a block with >1 take), film playhead + timecode ruler, drag-a-boundary \u{2192} `move_cut`, click \u{2192} select, double-click \u{2192} open, right-click \u{2192} Duplicate Shot/Try Another Setup \u{25b8} list/Add Missing Setup Layers/Edit Setup, `S` while the strip has the pointer's attention \u{2192} `split_at_playhead`); crates/buzz-ui/src/workspace.rs (`PanelId::Sequence`, tabbed beside Timeline in `Dock::Bottom` \u{2014} not front \u{2014} in the default layout; `VERSION_SEQUENCE_PANEL` migration adds it `Hidden`, home `Bottom`, to an existing saved layout rather than surprise-showing it); crates/buzz-ui/src/timeline_panel.rs (`TimelineState.shot_end`: a greyed overlay + marker line past a Shot's own playing length, drawn last over every row — small and additive, the parked-timeline items untouched); crates/buzz-ui/src/panels.rs (`SequenceMenuInfo`, and the "Sequence"/"Shot" menus, beside Camera); crates/buzz-app/src/{app.rs, editor.rs} wire it all together (`App::sequence_panel`, `Editor::sequence_view`/`apply_sequence`, `Editor::playing_length` used in `advance_playback`'s two wrap sites so playback stops/loops at the shot's own length rather than `Scene::frame_count`; `Editor::scene_role_label` marks the scene menu's rows, `\u{25c6} Village (setup)` / `Shot 3 \u{b7} Village`, a Normal scene unmarked). Tests: sequence_panel.rs's own `clicking_a_block_selects_it`, `dragging_a_boundary_reports_move_cut` (pure, no window); `playback_stops_at_the_shots_playing_length_not_the_scenes_frame_count` in sequence_setups.rs covers the Timeline/playback half. Verified: `cargo test -p buzz-ui --lib` (354 pass), `cargo test -p buzz-app --lib app::` (65 pass), `cargo test -p buzz-app --test sequence_setups` (5 pass), `cargo build -p buzz-app`. |
| S6 | Export: Reel skips setups, orders shots; film audio continuous across cuts (test: sample continuity at a cut) | DONE — crates/buzz-doc/src/sequence.rs (`pub fn film_order(roles, keys, sequence) -> Vec<usize>`, the pure rule: Setups dropped, a Shot not in `sequence` dropped, the Shots that are in it play together in `sequence`'s order inserted where the first of them sits among the Normal scenes); lib.rs (`Document::film_order`, `Document::playing_length` (Shot → `max(1, audio_out-audio_in)`; Normal/Setup → `rendered_frame_count()`, not raw `frame_count()` — needed so a looping Normal scene still behaves exactly as before, see `editor.rs::the_films_length_agrees_with_what_the_exporter_will_render`), `Document::film_with_lengths -> Vec<(Scene, u32)>`, `Document::from_loaded_scenes` (extracted from `Document::open`, so headless gets the same reconstruction)); crates/buzz-export/src/reel.rs (`Reel::of_lengths`: each scene contributes exactly the given length, truncating a longer playlist or holding the last frame of a shorter one; `Reel::of` unchanged). Every Reel-construction site rerouted through `film_with_lengths`/`of_lengths`: buzz-app/src/{export_service.rs, preview.rs, test_movie.rs, headless.rs}, editor.rs's own length-agreement test, and app.rs's three scene-gathering call sites (Test Movie, preview, export) — `TestMovie`'s "active scene" index is now mapped through `film_order` so the audio-follow behaviour still finds the right scene when a sequence reorders the film. `headless.rs::build` switched from names-only `load_scenes` to `load_scenes_full` + `Document::from_loaded_scenes`, closing the S1 review note. Tests: buzz-export/src/reel.rs `sequence_tests::{film_skips_setups_and_plays_shots_in_sequence_order, an_inactive_take_is_skipped_from_the_film, a_shortened_shot_contributes_only_its_playing_length, dialogue_is_continuous_across_a_cut}` plus `tests::of_lengths_truncates_or_holds_to_the_length_given`/`of_lengths_with_a_scenes_own_length_matches_of`; buzz-doc/src/sequence.rs `film_order_tests::*`; buzz-doc/src/lib.rs `tests::film_with_lengths_skips_setups_and_uses_each_shots_playing_length`; buzz-app/src/headless.rs `tests::headless_load_of_a_sequence_matches_the_in_app_film_length`. Verified: `cargo test -p buzz-doc --lib` (210 pass), `cargo test -p buzz-export --lib reel` (13 pass), `cargo build -p buzz-app`, `cargo test -p buzz-app --lib export` (7 pass, includes editor.rs's looping length-agreement test), `cargo test -p buzz-app --lib headless` (5 pass), `cargo test -p buzz-app --lib test_movie` (5 pass, includes app.rs's Test Movie open/close tests). `buzz-app` briefly failed to build mid-session from unrelated, concurrent S4/S5 work-in-progress elsewhere in app.rs/editor.rs/buzz-ui (non-exhaustive `Command`/`PanelId` matches, a `TimelineState.shot_end` field, a missing `sequence_panel` method) — waited for it to settle rather than touch those files; all green once it did. |
| S7 | Cut on Dialogue (phrases/SRT speakers → shots); per-shot lip-sync on the slice | DONE — crates/buzz-app/src/editor.rs: `Editor::cut_on_dialogue(default_setup)` (Sequence ▸ Cut on Dialogue ▸ ‹setup›, mirroring New Shot from Setup's own submenu rather than a modal dialog) decodes the master clip, runs `buzz_audio::detect_phrases`, merges phrases shorter than ~0.75s into the one before them, computes the master's uncovered stretches from the shots already in `Document::sequence` (never the covered ones), cuts each uncovered stretch at the phrase starts inside it, and stamps one shot per resulting segment — a speaker named on the master scene's own **Captions** layer (`Editor::master_caption_lines`, the same label-on-a-keyframe reading `lip_sync_from_captions` uses) sends its segment to the first setup whose name mentions that speaker (`name_mentions`, already used for mouth matching), everything else to `default_setup`. `stamp_shot` always appends to the cut, so the new shots' keys are re-sorted back into audio-time order afterwards. S7b: `Editor::run_lip_sync` read `SoundBank::stage_track`, which handed `analyse_visemes` the *whole, untrimmed* clip — so a shot's own trimmed Dialogue layer (or any other trimmed sound) had its mouths searched for outside the section that actually plays. Fixed generically, not by checking for a Shot role: new `buzz_audio::Clip::trimmed(start, end)` cuts a clip to two in-clip times; new `SoundBank::stage_track_trimmed` (crates/buzz-app/src/sound.rs) hands back that cut clip when the cue carries a trim, `Arc`-shared otherwise; `run_lip_sync` now calls it instead of `stage_track`. Mouths still key from the cue's own `start_frame` (already correct — a shot's Dialogue keyframe sits at its own frame 0). Command::CutOnDialogue(SceneKey) added to buzz-ui/src/command.rs (label, no shortcut, not in the palette — payload-bearing, like NewShotFromSetup); panels.rs's Sequence menu gets a "Cut on Dialogue" submenu beside "New Shot from Setup". Tests: crates/buzz-audio/src/lib.rs `tests::trimmed_keeps_only_the_section_asked_for`; crates/buzz-app/tests/sequence_setups.rs `cut_on_dialogue_leaves_existing_shots_untouched_and_fills_only_uncovered_ranges` (a hand-cut shot stays byte-identical; every shot, old and new, tiles the film with no gap or overlap), `cut_on_dialogue_sends_a_captioned_line_to_the_speakers_own_setup` (a captioned line goes to the setup whose name mentions the speaker; an uncaptioned line goes to the default), `lip_sync_on_a_trimmed_shot_keys_the_first_mouth_change_at_the_shots_own_frame` (silence-then-tone master, shot trimmed to start inside the silence — confirmed to fail at the untrimmed frame before the fix, pass at the trimmed one after). Verified: `cargo test -p buzz-audio --lib trimmed` (1 pass), `cargo test -p buzz-app --test sequence_setups` (9 pass), `cargo test -p buzz-app --lib lip` (4 pass), `cargo build -p buzz-app`. |
| S8 | Continue from shot N | DONE — crates/buzz-app/src/editor.rs: `Editor::continue_from_shot(shot, source)` — for every stage layer present by name on both `shot` and `source` except Dialogue: skipped (name reported) if `shot`'s own setup has no layer of that name to compare against, or if `shot`'s frame 0 no longer equals that setup layer's frame 0 (`LayerTimeline::frame_contents`, structural `Vec<Arc<Object>>` equality — true by construction until something actually edits frame 0, since stamping only ever shares or extends, never rewrites, a keyframe's objects); otherwise `shot`'s frame 0 is set (`LayerTimeline::set_objects`) to `source`'s own resolved artwork (`LayerTimeline::resolved_at`, so a tween running through the last frame is baked rather than read as its start) at `source`'s own last playing frame (`Document::playing_length(source) - 1`). One `Document::edit` covering every qualifying layer (one undo step), switching to `shot` first if it was not already active; `source` is only ever read via `Document::film()` clones, never edited; `ShotInfo.continue_from` recorded via `set_role` afterwards (bookkeeping, like `take_of`, not on the undo stack). `Editor::earlier_shots_same_setup(shot)` lists shots before `shot` in `Document::sequence` sharing its `from_setup`, by scene name (already unique and already formatted `"<setup> – Shot N"` by `stamp_shot`) — feeds both the Shot menu's new "Continue from Shot" submenu (`SequenceMenuInfo.continue_from_choices`, buzz-ui/src/panels.rs) and the Sequence strip block's own right-click (`ShotBlock.continue_from_choices`, `SequenceResponse.continue_from`, buzz-ui/src/sequence_panel.rs). `Command::ContinueFromShot(SceneKey, SceneKey)` added to buzz-ui/src/command.rs (label, no shortcut, not in the palette — payload-bearing, like TryAnotherSetup); dispatched in `Editor::run` and `Editor::apply_sequence`. Tests: crates/buzz-app/tests/sequence_setups.rs `continue_from_shot_seeds_frame_zero_and_skips_a_layer_already_animated` (two independent stamps of one setup; shot 1 given a distinctive keyframe at its own last frame on two layers; shot 2's "Extra" layer hand-drawn on before Continue from Shot runs — after it, the other layer's frame 0 matches shot 1's last-frame pose exactly, "Extra" keeps the user's own drawing untouched and is named in the skip list, shot 1 is byte-identical layer for layer, and `continue_from` is recorded). Verified: `cargo test -p buzz-app --test sequence_setups` (9 pass), `cargo build -p buzz-app`. |
| S9 | Short user guide `docs/SEQUENCE_GUIDE.md`: how to set master audio, make setups, cut shots, stamp, takes, lip-sync per shot, export, flatten (user asked, 2026-09-27) | DONE — checked against shipped menus 2026-09-27; banner set to built |
| R | Opus review after each stage (targeted cargo tests; no snapshot renders) | DONE — see Review log |

Queue position: **top priority** (user, 2026-09-27: "ignore .. focus on the scenes part"). The timeline batch is parked (code landed, untested). J1 stays last.

## Review log

- 2026-09-27 Opus, S1+S2: reviewed `sequence.rs`, `Document::{stamp_shot,duplicate_shot,set_active_take,add_missing_setup_layers}`, manifest persistence. Core test `editing_a_setup_after_stamping_leaves_the_shot_untouched` compares the whole shot scene before/after four kinds of setup edit — sound. **Found + fixed (Opus):** autosave and the per-frame crash guard still wrote the names-only manifest, so a recovered sequence lost every role/shot/cut. `AutosavePlan` now carries `SequenceMeta` (`with_sequence`, `crash_plan`, `remember_plan_for_crash`); test `a_recovery_copy_keeps_setups_shots_and_the_cut`. `cargo test -p buzz-doc --lib` 203 pass. Accepted. Note for S6: `buzz-app/src/headless.rs` still loads via names-only `load_scenes`.
- 2026-09-27 Opus, S3: reviewed. Trim = `SoundRef.trim_start/trim_end` (seconds) from audio_in/out ÷ fps; sound Arc-shared into the shot's library; `retrim_shot` sizes only the Dialogue layer, other layers extended-by-hold, never truncated. 207 buzz-doc tests pass. Accepted. **Consequence carried into S5/S6:** a shortened shot keeps frames past its end, so a shot's *playing* length is `audio_out − audio_in`, NOT `Scene::frame_count()` — playback end, the timeline's end marker (greyed frames past it) and the Reel must all use the shot length. Add `Document::shot_length(key)`/`playing_length(index)` (Normal/Setup → frame_count).
- 2026-09-27 Opus, S6: reviewed `Reel::of_lengths` (exact per-scene length, hold-last when short), `film_order`/`playing_length` (Normal/Setup use rendered_frame_count so all-Normal docs are unchanged), headless now loads roles. Implementer: buzz-doc 210, reel 13, app export 7 / headless 5 / test_movie 5 pass. Accepted. Open question for a later check: a non-dialogue sound keyed past a shortened shot's end should not leak into the next shot (film_frame_of past the length returns None, so it should be dropped — not yet covered by a test).
- 2026-09-27 Opus, S4+S5: re-ran `cargo test -p buzz-app --test sequence_setups` (5 pass) and `buzz-ui --lib sequence_panel` (2 pass); implementer also ran buzz-ui 354 and app:: 65 pass. Accepted. Notes: sequence commands are bookkeeping, not on the undo stack — acceptable because none destroy anything (they add scenes / change which take is cut); Flatten keeps each shot's layers as their own set (not merged by name) — a polish target. Guide step 4 updated by implementer to match what was built.
- 2026-09-27 Opus, S7+S8: re-ran `--test sequence_setups` 9 pass. Lip-sync ignored sound trims before (fixed for all trimmed sounds via `SoundBank::stage_track_trimmed` / `Clip::trimmed`; test fails at the untrimmed frame without it). Cut on Dialogue fills only uncovered audio; Continue from Shot skips user-drawn layers. Accepted.
- 2026-09-27 Opus: `timeline_panel::tests::the_footer_never_overlaps_the_layer_list` failed once in a parallel `buzz-ui --lib` run, then passed 3 parallel + 1 serial full runs (354) — intermittent, likely a shared-global (theme) race between parallel tests; not caused by sequence work. Follow-up: make it deterministic.
- Known issue: `S` over the Sequence strip also matches the Ink Bottle tool shortcut (tools.rs) — may switch tool as well as split; follow-up to consume the key in the strip.
