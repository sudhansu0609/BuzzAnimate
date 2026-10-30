# Improvements plan (2026-10-27)

User, 2026-10-27: "do the outpaint, then section 2 all and then fla export and then
lighting". Section 2 is the list of seven new features below, all aimed at the
standing goal: animating faster, easier and mostly automatic inside BuzzAnimate.
Outpaint comes first (`docs/TRACE_SPILL_AND_OUTPAINT_PLAN.md` § Round 2026-10-27);
FLA export X4–X6 and lighting B/C follow this plan.

What already exists and is **built on, not rebuilt**:
- `buzz-act/src/perform.rs` (walk/run/wave/sit/idle as pose functions + `apply`),
  `buzz-act/src/layer_rig.rs` (layer-parented rig view, `key_at`, `set_placement`).
- Copy Motion / Pose / Performance (`editor::{MotionClip, PoseClip, PerformanceClip}`,
  AI_LAYER_RIG_AND_MOTION_COPY_PLAN C1–C4, retarget by slot name).
- Motion Editor (`buzz-ui/src/motion_editor.rs`, one tween's easing as a bezier).
- Director (`buzz-act/src/direct.rs`) and LLM director (`buzz-ai/src/director.rs`).
- Storyboard S0–S3 (`buzz-app/src/storyboard.rs`, `storyboard_panel.rs`); S4/S5 todo.
- Export queue, 4 at once (`buzz-app/src/export_service.rs`), reel (`buzz-export/src/reel.rs`).

## Status

| Step | State | Date | Evidence |
|---|---|---|---|
| I3 Timing: Retime + Spacing | todo | | |
| I1 Animate Between Poses (in-betweens with overlap) | todo | | |
| I2 Motion library | todo | | |
| I4 Storyboard → animatic + episode with camera (= STORYBOARD_PLAN S4 + S5) | todo | | |
| I5 Notes → revisions | todo | | |
| I6 Render Episode + contact sheet + shot checks | todo | | |
| I7 Character check | todo | | |
| Review (planner) after each step | – | | |

Order of work is the table order: the small timing tool first, because I1 and I2
write keys that I3's tools then reshape.

## Rules for every step

- One undo step per command. Every command has a menu item, a palette entry and an
  agent verb (`buzz-app/src/agent.rs`), as the AI must be able to do what the user can.
- No `cargo fmt`. No new tests (user rule): verify with a probe example
  `crates/buzz-app/examples/zz_<step>_probe.rs` on ep1 data (headless, never saved
  over the user's files), and run the existing suites: `cargo test -p buzz-scene`,
  `-p buzz-act`, `-p buzz-ui -- --test-threads=1`, `-p buzz-app --lib`.
- Headless only: no real mouse or keyboard.
- Works on both rig kinds: layer-parented rigs (the default) and bone rigs.

## I3 — Timing: Retime and Spacing

**Retime** (Modify ▸ Timeline ▸ Retime…; agent `retime`): the selected frame range
(one or many layers; whole character when a rig layer is selected) is scaled to a
new length, about its first frame. Dialog: new length in frames, or percent, or
"to fit the sound under it". Keys are moved to `round(first + (f - first) * k)`;
two keys landing on one frame keep the later; tweens keep their easing. Frames after
the range shift by the length change on every layer of the selection (same rule as
Insert/Remove Frames). In the timeline: Alt-drag on the right edge of a selected
range does the same live (preview == commit).

**Spacing** (Modify ▸ Timeline ▸ Spacing ▸ Even / Ease In / Ease Out / Ease Both /
Favour First / Favour Last / Hold Ends …; agent `spacing`): writes the easing on
every tween that starts inside the selection. "Favour" = strong ease towards that
key (Animate's custom-ease bezier, the same `Easing` the Motion Editor writes).
"Hold Ends" turns the first and last 2 frames of each tween into holds (adds a key
at both ends, so the pose sits).

Probe: Lalten walk from ep1 retimed 50 % and 150 %; key frames listed; spacing
written on 6 tweens; one undo restores. Contact strip PNG of 12 frames before/after.

## I1 — Animate Between Poses

The animator keys two (or more) poses of a character; this adds the motion a
good animator would put between them. **Rule-based, deterministic, no LLM**
(seeded where random), so the preview equals the result.

Command: Animate ▸ Animate Between Poses… (agent `animate_between`), on a rig
character with ≥ 2 keys in the selected range (or the whole span between the
keys around the playhead). Dialog with live preview on the stage (scrubbing):
- **Ease** (default Ease Both): written as I3 spacing on each part's tween.
- **Overlap** (default 2 frames): every part's keys are offset by
  `depth_in_chain × overlap` frames (chest 0, shoulder 1×, elbow 2×, wrist 3×;
  head 1×, hair/scarf/ears tagged "follow" 3×). The last pose still lands in full:
  the offset key holds until it is reached.
- **Arcs** (default on): a part whose tween would travel in a straight line
  (translate-only part: hand IK target, character placement) gets a breakdown key
  at the middle frame, pushed off the straight path by `arc × distance` along the
  normal on the side of its parent's rotation.
- **Anticipation** (default off, 0–20 %): a key 3 frames before the move, the pose
  turned 10–20 % of the way the *other* way.
- **Settle** (default on): the last pose overshoots by 8 % at `end - 2`, back at `end`.
Each option is a checkbox + one slider. One undo step.

Works on layer-parented rigs (parts are layers; depth from the follow tree) and on
bone rigs (depth from the armature).

Probe: Guddu with two keyed poses 24 frames apart (arm up → arm down); before/after
strip of 12 frames; per-part key frames listed; the last frame's pose equals the
keyed pose (angle diff < 0.1°).

## I2 — Motion library

A reusable library of motions, the user's own and the built-ins, applied to any
Cast rig by slot name (C3's retarget).

- **Save Motion…** (Animate ▸ Motion Library ▸ Save Selection as Motion; agent
  `save_motion`): the selected character's frame range becomes a
  `PerformanceClip` saved as `<library>/Motions/<name>.motion.json` (the asset
  library root the Assets panel already uses), with a tag set (walk, run, talk,
  gesture, idle, turn, sit, custom), loop flag, and a 6-frame thumbnail strip PNG.
- **Built-ins** written into the same folder on first run, from `perform.rs`
  (walk, run, idle, wave, sit, point) applied to a reference slot rig, so they
  are ordinary clips.
- **Motion Library panel** (dock panel, next to Assets): thumbnails with play-on-
  hover, search, tags; double-click or drag onto a character = paste performance at
  the playhead (relative, stretch to a length typed in the panel, loops count).
  Agent `apply_motion {character, motion, frame, frames?, loops?}`.

Probe: save Lalten's 24-frame walk, apply to Munna and Raju (layer rigs) and to
one Cast bone rig: keys created, per-slot angles match within 1°, own placement
kept; strip PNGs.

## I4 — Storyboard → animatic and episode (STORYBOARD_PLAN S4 + S5)

Implemented as specified in `docs/STORYBOARD_PLAN.md` § S4 and § S5; this plan
only adds the camera rule, and the status lives in that plan.
- **Camera from the panels:** each panel's framing (`frame_subject`, wide /
  medium / close, the subject's box) becomes the shot's camera key at its first
  frame; a panel marked with a move (push in, pan to) gets a second key at its
  end, eased (I3 Ease Both). Cuts are hard (no tween across a panel boundary).
- Board edits (reorder, durations, framing) carry into `produce`.

Probe and review per STORYBOARD_PLAN S7 (ep1 scenes 4 and 5).

## I5 — Notes → revisions

The director writes notes the way they'd write them to an animator:
"sc5: Guddu looks left on line 3", "make Raju's entrance slower", "Munna should
laugh after the joke".

- **Notes panel** (Window ▸ Notes; agent `add_note`, `apply_notes`): one note per
  row, stored in the project as `notes.json` (text, scene, status open / applied /
  failed, the edit's summary, undo id).
- **Apply** sends each note with the scene's script lines, cast, the shot list
  and the timing (`script_timing`) to the LLM (`buzz-ai/src/director.rs`
  C11 path, the configured backend), which returns a JSON plan of **existing
  director verbs** plus a frame range (line 3 → its frames from the dialogue
  alignment). Validated like C11; shown as a preview (a summary row + a
  scrubbable before/after on the stage); **Accept** applies it as one undo step;
  the note row records what was done.
- New verbs only where a note needs one the director lacks: `retime` (I3),
  `animate_between` (I1), `apply_motion` (I2), `look_at` already exists.

Probe: 5 written notes on ep1 scene 5 against the configured backend (or the
offline fallback: rule parser for "looks left/right", "slower/faster", "laughs");
per note: the plan JSON, applied yes/no, before/after frame PNG.

## I6 — Render Episode, contact sheet, shot checks

**File ▸ Export ▸ Render Episode…** (agent `render_episode`): every scene of the
project (or a range) into the export queue (4 at once, existing service), then:
- **Contact sheet** per scene: first / middle / last frame of each shot, captioned
  (scene, shot, frames, characters), PNG pages beside the mp4s.
- **Shot checks** (cheap, on frames the render already produced, sampled every 6
  frames, plus the document): 
  - empty frame (nothing but the background for > 12 frames);
  - silent mouth: a character's dialogue sound plays but its mouth symbol does not
    change frame for > 1 s;
  - off frame: a speaking character's head box outside the camera frame;
  - frozen: no pixel change > 2 s while a sound plays;
  - render failure (from the queue).
- **Report** `report.html` + `report.json` beside the exports: one row per flag
  with a thumbnail and "scene/frame". In the app, a Render Report panel lists the
  flags; clicking one opens the scene at that frame.

Probe: ep1 scenes 4–5 rendered; sheet PNGs looked at; plant one fault (delete a
mouth key range in a scratch copy) and see it flagged.

## I7 — Character check

**Check ▸ Characters…** (agent `check_characters`), over the whole project
(or open scene): for each Cast character placed in any scene, compare against its
Cast file:
- **Colours:** each part's fill palette; a fill > ΔE 6 from every swatch of that
  part in the Cast file is flagged (catches stray recolours and broken wardrobe).
- **Parts:** a part symbol missing or added compared to the Cast rig.
- **Lines:** median stroke width per part vs Cast; > 25 % off flagged (scale
  aside: measured in the character's own space).
- **Size:** the character's height relative to the scene's other characters
  jumps > 15 % between shots of one scene.
Results in the same report panel as I6 ("go to"), with **Fix from Cast** for
colour flags (repaint those fills with the nearest Cast swatch, one undo step).

Probe: ep1 project: report JSON; plant one recolour + one missing hand in a
scratch copy and see both flagged; Fix from Cast restores the colour.
