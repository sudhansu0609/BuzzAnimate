# BuzzAnimate standalone + co-pilot (phase 2, 2026-10-29)

Phase 1 (`docs/ANIMATED_CHANNELS_PLAN.md`, merged 2026-10-29) made the producer work end to end
from the Studio. The creator's next asks (2026-10-29):
1. **Everything works from BuzzAnimate itself, separately** (no Studio needed): his story -> verbatim
   screenplay with cast extraction -> produce.
2. **The tool supports him after the first pass**: he keeps directing scene by scene; each change
   rebuilds only what it touches.
3. **Audio comes as ~3-minute parts in order plus the full final episode file.**
4. **His own characters, never generic ones.**

Built in the worktree `B:\youtubeProjects\_worktrees\animate2\BuzzAnimate` (staged baseline = live,
including his uncommitted in-progress files). Merge 3-way into the live tree after review, when idle.
Copy his files, never edit them. Tests: 2-3 small ones per item, no long or render tests; visuals by
stills checked by Opus.

## Decisions (creator, 2026-10-29)
- **Cast sources (all three):** BuzzAnimate library `Characters/` (and its style subfolders), his FLA
  files (`B:\youtubeProjects\Khayal3Baje\latestLibrary\charactersNew1.fla`,
  `B:\youtubeProjects\Khayal3Baje\stories\morecharacters.fla`, `womanSample.fla`, `sampleNaming.fla`),
  and each story's own folder (`B:\youtubeProjects\Khayal3Baje\stories\<story>\animate\*.fla`).
- **Audio:** a folder of ~3-min parts in order (natural sort: part1, part2 ... or 01, 02 ...) plus the
  full final episode file.
- **Support controls, all four:** redo a background; swap character / pose; shots and angles; re-time
  lines / lip sync.

## Items
- **B1 Story -> screenplay in BuzzAnimate.** Port the Studio's verbatim converter
  (`B:\youtubeProjects\Buzzcaf_Media\BuzzcafStudio\backend\app\services\animate_screenplay.py`: units
  split in code, the model extracts the cast and labels only, code copies his sentences, verification
  that the spoken text is identical; cast matching that never merges two people on a shared role word or
  blank looks) into `buzz-ai` (Rust). LLM: OpenAI-compatible Gemini harness `http://127.0.0.1:8001/v1`
  (model `flash`) first, LM Studio fallback -- through buzz-ai's existing LLM client. Per-series cast
  registry in the asset library (`Cast/<Series>/cast_registry.json`). Output exactly the producer's
  format (`## SCENE n — TITLE`, bold `**EXT. PLACE — TIME**`, bold cues, inline `[Name: pose, position]`).
  UI: File ▸ AI ▸ Story to Screenplay… (story file in, screenplay + cast.json out, editable before
  producing); Produce Episode accepts a plain story and converts it first.
- **B2 His own cast.** A character catalogue from the three sources (FLA files imported through the
  existing importer into the library under `Characters/<source>/`, kept up to date by file mtime),
  indexed with the existing asset index (names, SigLIP, VLM descriptions). Casting: for each cast member
  (name, role, gender, age, look, aliases) shortlist his characters (name heuristics + SigLIP text
  similarity), the model picks with a reason; the choice is stored in the series registry and reused
  next episode. ComfyUI drawing only when no match and he allows it (flag). Rig readiness: report per
  cast character whether it has a usable mouth (layer rig mouth / mouth symbols like `mouthSide`,
  `mouthStraight`) and limbs for gestures; offer the existing AI Layer Rig action for those that do not.
  UI: a Casting dialog (cast member -> chosen character thumbnail + alternatives; change; lock).
- **B3 Audio parts + full file.** Inputs `--audio-parts <folder>` and `--audio <full.wav>` (and the
  dialog fields). Each part is transcribed (BuzzAnimate's transcription or the BuzzEdit transcription
  tool already wired in the assistant) and aligned to the screenplay lines -> part -> line/scene ranges
  (`parts.json`). `--part N` builds and renders only part N's scenes/lines for a quick preview; the final
  build aligns the full file and renders the whole episode. Report shows part boundaries and any lines a
  part did not cover.
- **B4 Co-pilot.** A Producer panel in the app for an episode's output folder: scenes -> shots -> lines.
  Actions (each writes an override into casting.json / `overrides.json` and rebuilds only the affected
  scene): background -- regenerate (new seed), edit prompt, pick another try, re-angle (Qwen); cast --
  swap a cast member's character (episode-wide or one scene), set a line's pose, move/re-stage (pins);
  shots -- change a shot's camera/angle, add/remove a cut at a line; lines -- nudge start/end, change
  speaker, redo lip sync for one line or one part. The same actions as assistant tools
  (`redo_background`, `set_background_prompt`, `pick_background_try`, `recast`, `set_pose`, `set_shot_camera`,
  `add_cut`, `remove_cut`, `retime_line`, `set_speaker`, `redo_lipsync`, `rebuild_scene`).
- **B5 Studio parity.** The Studio's "Produce animation" passes the parts folder + full file, and uses
  the same cast registry/casting results.

## Status
| Item | State | Notes |
|---|---|---|
| B1 Story -> screenplay in BuzzAnimate | done | buzz-ai/src/story.rs (+3 tests), examples/story_to_screenplay.rs, File>AI>Story to Screenplay dialog (produce_dialog.rs, app.rs, command.rs, panels.rs), Produce converts plain story (produce.rs); cargo check clean; no live LLM run |
| B2 His own cast (catalogue, casting, rig readiness) | done | my_cast.rs (catalogue, FLA sync by mtime, shortlist, casting.json, readiness; 3 tests), produce.rs cast step + report.md readiness, CLI --story-dir/--allow-generated-cast, buzz-ui casting_dialog.rs (File>AI>Casting, Produce dialog button); cargo check clean; no live LLM/FLA import run |
| B3 Audio parts + full file | done | produce_parts.rs (new), produce.rs (audio/build/render, --part, parts.json, report Parts), example CLI, produce_dialog + app.rs wiring; 3 unit tests ok, check clean; producer not run |
| B4 Co-pilot panel + assistant actions | done | copilot.rs (overrides.json, 14 actions, scoped rebuild, 3 tests ok), producer_panel.rs + Window>Producer (command.rs, panels.rs, app.rs), 14 assistant tools (agent.rs), produce.rs hooks, buzz-doc replace_scene; Overrides in report.md; check clean; producer not run; unit-command count test set to 226 (with Casting) |
| B2b/B4 review fixes | done | my_cast.rs: model casts from the WHOLE catalogue (id/name/folder/description, chunks of 120, winners re-asked), thumbnails of undescribed characters described once by LM Studio vision and cached as the index label; report.md cast line shows the Characters/ path (cast.json mine_path). copilot.rs/app.rs: action reloads the rebuilt .buzz into the same tab (playhead/camera kept). Picked try / re-angle kept as backgrounds/<key>.fixed.png (BgOverride.fixed) and used as-is by produce.rs; a new seed/prompt clears it. buzz-ui unit-command test runs in 0.5 s and passes (units + data hints = 228). 2 new tests; check clean; no live LLM/VLM/render |
| B7 Episode window + intuitive UI | done | File>AI>New Episode (buzz-ui episode_window.rs; 5 steps Story/Cast/Audio/Look/Produce, left rail with done ticks, Next off until valid; old Produce/Story dialogs have a Guided button), cast sheet reused from casting_dialog (no-mouth badge + Fix rig), per-series Look in assets/comfy_profiles/<Series>/episode_settings.json (episode_settings.rs, new ProduceRequest.angle_pictures), Producer panel now tree + selected item's actions only, last-change line, disabled reasons. 2 new tests; check clean; no producer run. Snapshots: B:\youtubeProjects\Buzzcaf_Media\Samples\animate_ui_snapshots\ (1_story..5_produce_done, 6-10_producer_*), example episode_ui_snapshots. Polish (lead review): Look step shows the series' ComfyUI flow + LoRAs with a Change... picker (comfy_profile::pictures_line/set_workflows), new-series box separate; Producer: 'Last change:' line, items named 'Scene 2, line 1', pose dropdown from the character's pose variants (+ Sit), Move = click/drag on the scene still with Left/Centre/Right/Door, Play someone else = thumbnail grid with search; scene still on top (copilot::view adds `still`, `poses`). 1 new test (pose_dropdown_lists_variants_with_standing_first) |
| B5 Studio parity | todo | |
| B6 Review (Opus) + stills + merge | merged 2026-10-29 (creator: 'merge all for now') | Live: BA story->screenplay verbatim (Forest Officer + 2 guards), FLA import 54 assets, casting picked his Characters/Bold Outline/forestOfficer with a reason, backgrounds sharp 211 / trace SSIM 0.917; UI snapshots reviewed + polished; Opus added LM Studio release after the cast step. NOT yet live: audio parts with his real audio, scene-3 cast test re-run, B5 Studio parity |

Each implementer updates its row (todo -> doing -> done, one-line note: files, tests, stills) after every
item, so a cut-off leaves a hand-off.
