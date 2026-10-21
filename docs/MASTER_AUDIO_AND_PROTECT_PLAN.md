# Master Audio mode + Protect other keyframes

User, 2026-10-20:

> "I want a master audio mode: if master audio is set, it deletes or removes
> all other audios and only that audio remains for every scene, every
> character etc. It's to make sure I am using the right audio. There shall be
> a sound layer in each character and scene, also in any symbol tagged head."
>
> "Also a mode in which, if I change a symbol or a character, it shall not
> change anything in previous keyframes, maybe just the current keyframe. This
> is so I don't accidentally change other scenes when I change a character or
> object in the current scene."

## What already exists (measured 2026-10-20)

* **Sequence ▸ Set Master Audio…** (`editor.rs` `set_master_audio_from_file`
  ~6672, `set_master_audio_from_layer` ~6721). `Document::master_audio:
  Option<(SceneKey, LayerId)>` (`buzz-doc/src/lib.rs` ~96, saved in the
  container index, `format.rs`). Shot scenes (`SceneRole::Shot`) get a
  "Dialogue" stage layer trimmed to `audio_in..audio_out` by
  `Document::retrim_shot` (~700). Nothing removes *other* sounds, and normal
  (non-shot) scenes, symbols and heads get nothing.
* Episode build (`episode.rs` `attach_scene_sound` ~3645) puts a separate WAV
  **cut** per scene ("Dialogue scene N"), a different `SoundAsset` from the
  full recording.
* Sound lives on keyframes (`Keyframe::sound`), any layer, any timeline.
  Playback and export read only stage layers (`Scene::stage_cues`
  `buzz-scene/src/lib.rs` ~897), so sound inside a symbol is never heard.
* Symbols have `tags: Vec<String>` (`symbol.rs` ~377), mostly empty. Heads are
  found by name in practice (`buzz-ai/src/character.rs` ~3800: name contains
  "head").
* Each document scene has its **own library**, so editing a symbol in scene 3
  never changed scene 1. The real risk is *inside one scene*: one symbol is
  shared by every keyframe that shows it, so editing it at frame 300 changes
  frames 0–299 too.
* `Editor::auto_keyframe` (editor.rs ~122) exists. When it's on, stage edits at
  a mid-span frame create a keyframe at the playhead.
* Edit in Place (`editor.rs` ~8030) opens a symbol from a clicked instance.
  `Scene::editing`/`edit_places` (lib.rs ~290) record the path but not which
  instance was clicked.
* `Document::edit` (buzz-doc lib.rs ~977) snapshots the whole scene, including
  the `editing` view stack, so undo restores the stack that matches the
  artwork.
* Duplicating a symbol keeps object ids (`duplicate_library_symbol` ~8369
  shares the layer stack).

## Part M: Master Audio mode

### Behaviour

* **Sequence ▸ Master Audio Mode** toggle (checkmark). If no master audio is
  set, turning it on opens the Set Master Audio file dialog first.
* Turning it on (or changing the master while it's on) runs **Enforce**:
  1. **Find each scene's slice of the master.** Shot scenes use
     `audio_in..audio_out`. The master's own scene uses the full clip. For any
     other scene, if it carries a sound that **is a cut of the master**
     (located by envelope cross-correlation, see M2), the slice is that
     offset and length. If nothing matches, the scene gets the master from
     0:00 and the report says "Scene N: no audio to align — master placed from
     0:00".
  2. **Remove every other sound.** Clear every keyframe sound that isn't the
     master clip, on stage layers and inside every symbol, in every scene.
     Delete layers left with no artwork and no sound. Remove sounds nothing
     references from each scene's `SoundLibrary`.
  3. **One "Master Audio" stage layer per scene** carries the master clip,
     `Stream`, trimmed to the slice, and spans the slice's length. For shots
     this is the existing Dialogue layer (`retrim_shot`). It isn't renamed, so
     the "Dialogue" special case in `editor.rs` ~7342 still works.
  4. **A Master Audio guide layer inside every character and head symbol.**
     Character = tag `character` (case-insensitive), or library folder `Cast`
     / `Cast/...` (not `Cast parts`). Head = tag `head`, or a name containing
     "head" (case-insensitive, the convention `character.rs` already uses).
     The layer is `Layer::audio_guide = true`, locked, at the bottom of the
     symbol's stack, and carries the master clip trimmed so that symbol frame
     `t` hears the stage frame where it shows:
     `offset = scene_slice_start + s0 - f0`. Here `s0` is the stage keyframe
     start of the symbol's **first** visible use (nested uses compose), and
     `f0` is that instance's graphic `first_frame` (0 for movie clips). It
     spans the symbol's length. An unused symbol uses `s0 = f0 = 0`.
  5. **Confirm first.** "Master Audio mode will remove N other sounds from S
     scenes and Y symbols, and add the master to every scene, character and
     head. Each scene can be undone with Ctrl+Z." Apply as one undo step per
     scene ("Master Audio mode").
* **While it's on:**
  * Import Sound, drag-dropping a sound, the agent `sound` frame property and
    Properties ▸ Sound picking a non-master clip are **refused**. Status:
    "Master Audio mode is on — only <name> is allowed (Sequence ▸ Master Audio
    Mode)". Set Master Audio stays allowed and re-runs Enforce.
  * A status/timeline badge reads `♪ MASTER: <name>`.
  * **Sequence ▸ Re-apply Master Audio** reruns Enforce, for example after
    new characters are placed.
* `audio_guide` layers are **never heard and never exported**.
  `Scene::stage_cues` skips them, as do XFL export (doesn't write the layer's
  sound) and video export (it uses `stage_cues`). They exist so the waveform
  shows in the timeline while animating inside the character or head, for
  lip sync.
* Turning the mode off removes nothing and restores nothing. It only stops the
  refusals.

### Persistence

* `Document::master_audio_mode: bool`. It's saved in the container index next
  to `master_audio` (`format.rs`, `#[serde(default)]`, so old files load), and
  autosave meta carries it.
* `Layer::audio_guide: bool` (`#[serde(default)]`) in `buzz-scene` `layer.rs`
  and `buzz-doc` `serial.rs`.

## Part K: Protect other keyframes

### Behaviour

* **Edit ▸ Protect Other Keyframes** toggle, saved per person in `Workspace`
  (`#[serde(default)]`), like `drawing_modes`. A lock button on the edit bar or
  timeline header shows and toggles it. Scope setting: **This keyframe**
  (default) or **This keyframe and later** (a dropdown next to the toggle).
* **On the stage:** while it's on, `auto_keyframe` is forced on. Moving or
  changing a character at frame 300 of a keyframe that starts at 0 makes a
  keyframe at 300 first, so frames 0–299 don't change. The setting is
  remembered and restored when the mode is turned off.
* **Inside a symbol opened in place (copy-on-write):** the first edit that
  changes a symbol on the open path, while that symbol is also used anywhere
  else, forks the path, and only the clicked keyframe gets the change.
  * `Scene` gains view state `edit_origins: Vec<Option<EditOrigin>>`, one per
    `editing` entry, with `EditOrigin { layer, keyframe_start, object }`. This
    is the instance clicked in the parent timeline. It's `None` when the
    symbol was opened from the Library. Set by Edit in Place (~8030), cleared
    with `editing`.
  * `Document::protect_keyframes: Option<ProtectScope>` is view state, set by
    the editor from the workspace and not saved. `Document::edit`, when
    protect is set and the active scene has a non-empty editing path: run `f`
    as now. Then if any symbol on the path changed compared with `before`,
    call `Scene::fork_edit_path(&before, scope)`. That works out:
    * `k` = the first path index whose symbol has a reference count > 1
      (counting instance objects in every keyframe of the stage and of every
      symbol) in `before`. No shared index means no fork.
    * For `i in k..n`: new symbol `S_i'` = **after's** `S_i` (edited), and
      `S_i` = **before's** `S_i` (restored). Same name + " (f<1-based keyframe
      start>)", same folder, kind, registration and tags, empty `set`.
    * Repoint `o_k` in its parent (the stage, or `S_{k-1}`, which is private).
      For `i > k`, repoint `o_i` inside `S_{i-1}'`. Object ids are unchanged
      by duplication.
    * Scope **later**, when `k == 0` (parent is the stage): also repoint every
      stage instance of `S_0` in keyframes with `start >= o_0.keyframe_start`,
      on any stage layer.
    * Rewrite `editing[k..]` to the new ids.
    * All of this is one undo step with the edit, because it happens inside
      the same `edit`. Undo restores the old path from the snapshot.
    * Status line: "Protected: <name> copied as <name (f301)> for this
      keyframe".
  * Opened from the Library (origin `None`): no fork. The first edit shows a
    status warning: "<name> is used in N keyframes — open it from the stage to
    change only one".
* **Whole-character commands** in protect mode act on a copy for the current
  keyframe only: wardrobe recolour (Cast panel) and Swap Symbol (Swap repoints
  only the selected or current-keyframe instances, not every instance). Each
  deep-forks the character subtree once (all symbols reachable from the
  instance), repoints the current keyframe's instance, then applies the change
  to the copies. Stage K3.

## Constraints

* No new tests (user rule 2026-10-11). Existing suites must stay green:
  `cargo test -p buzz-scene -p buzz-doc -p buzz-ui`, plus `-p buzz-app --lib`
  and any `--test` that touches sound/sequence (`audio_lipsync_e2e`, sequence
  tests).
* Never run `cargo fmt`. Don't touch the uncommitted Drawing Mode work beyond
  what compiling needs.
* Verify headlessly (`Editor` probes in a scratch test that is not committed);
  no real mouse, no SendKeys.

## Status

| Step | What | State |
|---|---|---|
| M1 | `Layer::audio_guide` (+serial), `stage_cues`/XFL export skip it; `Document::master_audio_mode` (+format, autosave); `Document::edit_scene_at(index, label, f)` with history | done (2026-10-20): fields + serde defaults + save/autosave threading in; `cargo build -p buzz-app` ok |

| M2 | Enforce: slice finding (shot range / master scene / envelope correlation of an existing cut), remove other sounds + orphans + empty sound-only layers, stage Master Audio layer, character/head guide layers | built (2026-10-20): `buzz-doc/src/master_audio.rs` (`enforce_master_audio`, `master_audio_preview`, `MasterAudioReport`), `Scene::remove_stage_layer`; builds, `cargo test -p buzz-scene -p buzz-doc` 782 passed; behaviour/idempotency not yet probed (M4) |

| M3 | Editor/app: Sequence ▸ Master Audio Mode + Re-apply, confirm dialog, refusals (import/drop/agent/Properties), badge, Set Master Audio re-enforces while on | built (2026-10-20): commands + menu + click-again confirm on the status line + refusals + status-strip badge; `cargo build -p buzz-app` ok, `cargo test -p buzz-app --lib` 711 passed / 4 failed (pre-existing AI-panel test + 3 timing tests that pass when re-run alone). Not exercised in the app yet (M4) |
| M4 | Verify: probe on a 2-scene doc (shot + episode-style cut scene, a character + head symbol, a stray SFX): counts, slices, guide layers silent in `stage_cues`, refusal; suites green | done (2026-10-20): scratch probe (deleted) on 2 scenes, 10 s non-stationary master: preview == applied (4 sounds, 3 layers, 2 scenes, 2 symbols); scene 2 cut aligned at 4.0 s (match 1.00), one Master Audio layer Stream trim 4.0..7.0, 72 frames; stray SFX cleared with its art keyframe kept; sound-only layer and head SFX layer deleted; orphan sounds gone (library = master only); Raju (Cast) and Raju head each one locked bottom guide, trim 5.0 (4.0 s + 24 f @24 fps); `stage_cues` = master cue only; 2nd Enforce all zeros, no new undo step; undo restores scene 2's 4 sounds/cues; mode persists through save/open; Editor::import_sound and agent `set_property` frame sound refused with the status text. Timing: 10-min master + 3 cut scenes, Enforce 419 ms (dev profile, opt-level 1), all three cuts found exactly (60/200/450 s). No code fixes needed. `cargo test -p buzz-doc` 233 passed |
| K1 | `EditOrigin` view state set by Edit in Place; `Document::protect_keyframes`; `Scene::fork_edit_path` + `Document::edit` hook | done (2026-10-21): `EditOrigin`/`ProtectScope`/`ForkReport` (buzz-scene, re-exported by buzz-doc), `Scene::edit_origins` + `enter_symbol_in_place_from` (old `enter_symbol_in_place` = origin None), double-click Edit in Place records layer/keyframe start/object; `Document::{protect_keyframes, set_protect_keyframes, take_last_fork}`; fork runs only when a changed symbol sits at/inside the first shared one and every origin is found. Scratch probe (deleted), 6/6 through the real `enter_or_leave_at`: Head at kf 0/300/400, edit at 300 → kf 0/400 keep Head (unchanged), kf 300 → "Head (f301)" with the change, path rewritten, 2nd coalesced drag step no re-fork, one undo restores instance, path and library; refcount 1 no fork; FromHere repoints 300+400 not 0; protect off = global; Library-opened (origin None) no fork; nested Character(shared)>Head → both copied, chain repointed, original Character still → Head, undo restores path. `cargo test -p buzz-scene -p buzz-doc` 549+233 passed; `-p buzz-app --lib` 707 ok, 6 timing/load failures pass alone, AI-panel test pre-existing, `every_menu_command_runs_and_the_window_still_draws` not hung but ~6.4 s/command under load (cut off at 15 min cap) |
| K2 | Workspace toggle + scope, Edit menu + lock button, forced auto_keyframe, Library-opened warning, status lines | done (2026-10-21): `Workspace::{protect_keyframes, protect_from_here}` (serde default) + `protect_scope()`; commands `ToggleProtectKeyframes` / `ProtectThisKeyframe` / `ProtectFromHere` (ids `toggle_protect_keyframes`, `protect_scope_this_keyframe`, `protect_scope_from_here`; agent Route::Run; count test 204→207) in Edit menu (tick + Protect Scope submenu) and timeline header "🔒 Protect" + scope dropdown (`TimelineState::protect_keyframes`, `TimelineResponse::{toggle_protect_keyframes, set_protect_scope}`); `Editor::{set_protect_keyframes, set_protect_from_here, sync_protect_keyframes, note_protected_edits}` (sync at startup, toggle, adopt/swap_document/create_document; `note_protected_edits` runs each frame in `build_ui`): Auto Keyframe forced on + previous value restored, Toggle Auto Keyframe refused while on; fork status "Protected: Ana copied as Ana (f301) for this keyframe"; Library-opened warning once per entry. Scratch probe (deleted): toggle forces/restores auto keyframe (off and on cases), scope commands reach the doc, fork status exact, Library warning "Head is used in 4 keyframes — …" once, not repeated on the 2nd edit |
| K3 | Wardrobe recolour + Swap Symbol respect protect mode (deep fork for current keyframe) | done (2026-10-21; recolour N/A): `Scene::fork_instance_subtree(EditOrigin) -> Option<ForkReport>` (every reachable symbol copied once, BFS, cycle-safe, incl. turnaround views; " (f<start+1>)", empty `set`; nested instances in copies + the keyframe's stage instance repointed). Clear All Keyframes from a stage instance: `Editor::clear_keys_origin` set by the dialog opener (instance read at the playhead keyframe), `Editor::clear_symbol_keyframes_from` forks + clears the copy in one undo step when protect is on and refcount > 1; Library path unchanged. Swap Symbol was global (`Scene::swap_symbol` repoints every use), so in protect mode it now repoints only the first selected instance in the playhead keyframe (no copy needed; refuses if the two aren't instances at the playhead). Probe (deleted): clear at kf 300 → Ana keeps 2 keyframes, "Ana (f301)" has 1, kf 0 still Ana, Ana+Head copied, one undo restores; swap at 300 leaves kf 0. **Wardrobe recolour not built**: the Cast panel recolours the Cast asset file on disk (`App::recolour_character`), never stage instances, so there's no in-document command to protect — needs a plan decision. Review 2026-10-21: wardrobe recolour (`App::recolour_character`) rewrites the Cast asset file on disk and never touches stage instances, so there is nothing in-document to protect; marked N/A. |
| K4 | Verify: probe: shared head in keyframes at 0 and 300, edit in place at 300 → frame 0 unchanged, frame 300 changed, one undo restores both; refcount-1 path doesn't fork; scope later; suites green | done (2026-10-21): K1 probe 6/6 + K2/K3 probe 5/5 (fork at kf 300 leaves kf 0, refcount 1 no fork, FromHere, Library warning, Clear Keys + Swap in protect, one undo); `cargo test -p buzz-scene -p buzz-doc -p buzz-ui --tests` 549/233/375 + integration ok except pre-existing dock_columns ×3; ai_agent_coverage 41/41; never_hang.rs TimelineState literals fixed (focus_folders, protect_keyframes) |

## Part F: Clear All Keyframes in a symbol or character

User, 2026-10-20: "an option in the symbols or characters to clear all the
keyframes and just have normal frames."

* **Clear All Keyframes…** in three places: the Library symbol context menu
  (`buzz-ui/src/library_panel.rs` ~582), the stage instance right-click menu,
  and Modify ▸ Symbol. Each opens a small dialog:
  * **Keep pose:** *First frame* (default) or *Frame at playhead*. For an
    instance on stage, the playhead frame is the inner frame it shows now
    (`instance.resolve_frame`, as Edit in Place does at `editor.rs` ~8045).
  * **Include nested parts** (default on for characters, meaning tag
    `character` or folder `Cast`; off otherwise). It applies the same thing to
    every symbol reachable inside, each symbol once, cycle-safe.
* On every layer of the symbol: the kept pose's content becomes the single
  keyframe at frame 0, and every other keyframe becomes a normal frame
  (`Frames::clear_keyframe`, `buzz-scene/src/timeline.rs` ~474). Tweens and
  ease curves on the remaining keyframe are cleared. The layer **length is
  unchanged**, so the symbol keeps its duration. Keep frame 0's sound.
  Leave `audio_guide` layers alone. Labels on removed keyframes are dropped.
* One undo step, "Clear All Keyframes". Status: "<name>: 37 keyframes cleared
  on 12 layers (4 nested symbols)".
* **Protect Other Keyframes on** (Part K): from a stage instance, this works on
  a deep copy repointed for the current keyframe only (the K3 mechanism). From
  the Library it works on the symbol itself, after the K2 warning.
* Logic in buzz-scene: `Scene::clear_symbol_keyframes(id, keep: KeepPose,
  nested: bool) -> ClearReport`, so the agent can call it too (add an agent
  command `clear_symbol_keyframes {symbol, keep, nested}`).

| Step | What | State |
|---|---|---|
| F1 | `Scene::clear_symbol_keyframes` + Library/stage/Modify menu + dialog + agent command | built (2026-10-20): `buzz_scene::{KeepPose, ClearReport}` + `Scene::clear_symbol_keyframes` (nested = symbols reachable from the kept pose, each once, from First); `Command::ClearAllKeyframes` (id `clear_symbol_keyframes`, agent Route::Args `{symbol, keep: first\|playhead, nested?}`) in Library context menu, stage right-click (enabled on an instance), Modify ▸ Symbol; `buzz_ui::clear_keys_dialog` (keep First/at playhead, Include nested parts default = character); `Editor::clear_symbol_keyframes` one undo step + status. Protect-mode fork (Part K) not wired: K1–K3 not built. `cargo build -p buzz-app` ok; buzz-ui command-count test bumped 198→201 (was already 200 from M3) |
| F2 | Verify: probe on a walking character (keyframed limbs): every layer one keyframe, same length, pose = chosen frame, nested parts flattened, one undo restores | done (2026-10-20): scratch probe (deleted): Walker (Cast) 3 limb layers keyed 0/6/12/18 len 24 + audio-guide layer, nested Leg keyed 0/4/8 → Dot. Stage instance selected at playhead 12 → dialog playhead Some(12), nested default on; Library path on Leg → no playhead, nested off. Frame(12)+nested: report 11 keyframes / 4 layers / 2 nested symbols, status line matches; each limb 1 keyframe, len 24, objects == old frame-12 keyframe, tween default, label kept from frame 0; guide layer untouched; Leg 1 keyframe len 12; exactly 1 undo step, undo restores Walker+Leg. Agent (temp lib probe, deleted): `clear_symbol_keyframes` ok with report, bad `keep` refused, second call 'changed nothing'. `cargo test -p buzz-scene` 549 passed; `cargo test -p buzz-ui` green bar the flaky shift-click test (passes alone); `cargo test -p buzz-app --lib` 711 passed / 4 timing tests failed under load, all 4 pass re-run alone |
