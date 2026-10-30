# Storyboards, animatics and scene references (asked 2026-10-23)

The user: "can the AI make storyboards for our stories, and animations, and
scene initial setup references?" Then: panels in **both** looks (real scene
setup, and AI sketch where a character isn't built yet), prove it on **ep1**,
and "can the AI learn from some snaps of my videos how I place characters?"

The Producer (`docs/PRODUCER_PLAN.md`) already goes script → breakdown
(scenes, cast, places, entrances, props, lighting, mood, shot list) →
backgrounds → cast → animated episode → mp4. What's missing is a step where
the user **sees and edits the plan before it's animated**, and staging that
looks like **theirs**.

## Status

| Step | What | State |
|---|---|---|
| S0 | Learn the user's staging from snaps of their videos → `StagingStyle` | dataset reviewed 2026-10-23 (§ S0 dataset): ~25 frames with people, mostly wide; more close/two-shot frames asked for; build queued |
| S1 | Storyboard data (`storyboard.json`) from the breakdown | built 2026-10-23: `crates/buzz-app/src/storyboard.rs` (Board/Panel serde; `board_for`: one panel per camera cut + extra panels at entrances/exits/action-line gestures; `merge` keyed by scene+shot+line, user-changed fields kept via a `generated` snapshot, deleted ids in `removed`, user-added panels kept after their predecessor). Verified on ep1 4+5: offline breakdown → 3 panels; Gemma breakdown (one run before the GPU freeze) → 21 panels; a hand edit (Devanagari dialogue, notes, action) and a deletion survived a re-run. Not verified: user-added panels, reorder (no editor yet); gesture splits never fired on scenes 4/5 |
| S2 | Panels: real scene setup (A) and AI sketch (B) | **Framing on the subject, 2026-10-23:** a medium/close is no longer the episode camera (216 %, centred between partners): `frame_subject` renders the whole stage at the panel's frame with and without the subject's layers (body layer + its follow tree, `CastNote.body`), takes the changed pixels minus the even darkening of its cast shadow, and puts the camera on the head-and-chest centre, head 8 % from the top, down to ~knees (medium, 74 % of the figure) or head and shoulders (close, 36 %); wide stays the whole stage; kept inside the stage. The used camera goes back into `storyboard.json` (snapshot too, so not taken for an edit). Measured on ep1 4+5 (Gemma breakdown reused via `--breakdown`, no model asked; 18 medium/close of 21): subject centre within 0.2 % of the width of the middle on all 18, head room 7.8–8.9 %; all four pages looked at (Guddu, Munna, Lalten, Raju each centred; closes head and shoulders). Before this, A built + verified 2026-10-23: headless `build_episode` with the breakdown's shots injected into the casting, panel's key frame (middle of its lines; 60 % into an entrance/exit action), shot camera, captions hidden, 960×540 via `buzz_export::Exporter`; ep1 4/5 pages looked at: cast on the set, wides and mediums framed. B code path built (`sketch_prompt` + `backdrop::generate` Z-Image, cached), **never run against a live ComfyUI**; with ComfyUI down `--look b` falls back to A and the sheet says so in red (verified). No ControlNet/img2img layout steering yet (Z-Image workflow is text-only). Auto picks A when every on-screen speaker/cast member is a Cast rig and the set is built (all A on 4/5; B rule not exercised) |
| S3 | Board sheet export + Storyboard panel in the app (edit, reorder, regenerate) | export built + verified 2026-10-23: `write_sheets` (3×2 pages, caption block: scene/shot/beat/kind, look line, camera, frame, lines, action, dialogue with overflow count; Nirmala UI via `buzz_text`, Devanagari shaping checked by eye), PNG per page + one PDF (`write_pdf`, lopdf already in the workspace, JPEG pages; opened with pypdf: 4 pages). `examples/storyboard.rs <ep dir> --scenes 4,5 --out <dir> [--look auto\|a\|b] [--offline] [--breakdown <breakdown.json>]` (`--breakdown` reuses an earlier run's breakdown, no model asked). **App panel built + verified headlessly 2026-10-23:** Window ▸ Storyboard (`PanelId::Storyboard`, hidden, home Bottom; `crates/buzz-app/src/storyboard_panel.rs`): Make / Load / Export sheets, grid of thumbnails with captions, selected panel's kind / on / duration / look (Auto, A, B) / action / dialogue lines / notes, Up / Down, Add after, Split, Delete, Regenerate, Open scene. A worker thread keeps the built scenes (`Workbench`) and sends 320×180 thumbnails, so the panel never renders; every edit is saved to `storyboard.json` at once. Palette: Make Storyboard, Edit Storyboard…, Export Storyboard Sheets; agent (Route::Args): `storyboard_make`, `storyboard_edit`, `storyboard_export` (waited for). Verified by `examples/zz_storyboard_panel_probe.rs` on ep1 4+5 (Gemma breakdown reused, no model): make 21 panels (~11 s), notes/action/duration edit, move, split, add + delete a user panel, delete a made panel, kind→close + Regenerate (re-framed head and shoulders, looked at), export 4 pages + 4-page PDF; a second make kept the edits, the user's split panel and the deletion; Open scene switched the open ep1.buzz to “Scene 5 — RAJU ARRIVES”, frame 326; panel PNG looked at. Not kept: **reorder of made panels is undone by a re-make** (merge follows the fresh order; only user-added panels keep their place); the second half of a split has no picture until Regenerate / the next make; Look B from the panel never run (no ComfyUI); `storyboard_animate` belongs to S5 |
| S4 | Animatic mp4 | todo |
| S5 | Animate from the (edited) storyboard | todo |
| S6 | Scene setup references: location pictures + top-down layout map, as a guide layer | todo |
| S7 | Prove on ep1 (scenes 4, 5 first, then all) | todo |

## What exists (reuse)

* `crates/buzz-ai/src/breakdown.rs`: `SceneBrief { slugline, setting,
  location_key, background_prompt, present, absent, place, enter, props,
  lighting, shots: Vec<ShotBrief{at_line, kind, on, …}>, mood }`.
* `crates/buzz-app/src/episode.rs`: `SceneSet { ground, background, place:
  PlaceSpec{x, y, h, enter, cast, asset}, … }`, shots and cuts (P5), the
  headless scene build used by `examples/episode_build.rs` / `produce.rs`.
* `crates/buzz-ai/src/pose.rs`: `detect(img) -> Vec<Person>` (DWPose,
  133 whole-body keypoints, `character_height`). Works on cartoon pictures
  (used on the ep1 T-pose sheets).
* Vision LLM requests with images: `crates/buzz-ai/src/agent.rs`
  (`build_openai_request` / `build_ollama_request` with `images`).
* ComfyUI pictures: `crates/buzz-app/src/{comfy,backdrop}.rs` (zimage1_hq,
  1920×1080 in 60–90 s).
* Profiles beside: `buzz_rig::cut_style::CutStyle`, `head_style::HeadStyle`
  ("My Animate … style", serde, defaults) — `StagingStyle` follows that shape.
* Guide layers (non-rendering) for references.

## S0 — learn staging from snaps

**Input**: a folder of stills from the user's finished videos (aim 30–80,
across wide, medium, close, two-shots, three-plus groups, interiors and
exteriors). If the source FLA/.buzz of those videos exists, read exact
placement from it instead of guessing from pixels — preferred where available.

1. Per snap: `pose::detect` → each person's feet point (ankles/heel
   keypoints; when the feet are off-frame, mark the shot as cropped), head
   top, height as a fraction of the frame, facing (nose vs ears and shoulder
   order: left / right / front / back), overlap order (who is in front).
   Where DWPose finds nobody or few keypoints, ask the vision LLM for boxes
   and facing (JSON), marked lower confidence.
2. Classify each snap's shot from the tallest figure's visible extent: wide
   (whole body, h < 0.55), full, medium (cut at waist/knees), close (head and
   shoulders), extreme close; and by count: single, two-shot, group.
3. Learn, per shot kind and count: ground line (feet y) distribution, figure
   height, x positions (thirds or centre), spacing between people as a
   multiple of figure height, headroom, edge margin, how often people face
   each other vs camera, depth stagger (one nearer and bigger), how often a
   speaker is on the left. Medians plus spread.
4. Save `StagingStyle` ("My staging style", serde JSON, editable, with
   `animate_convention()`-like defaults when there are no snaps) — in
   `buzz-act` or `buzz-ai`, beside where staging decisions are made.
5. Check sheet: every snap with the detected figures, feet marks, shot kind
   and facing drawn on it (scratchpad `s0\`), so the planner and the user can
   see what it read.
6. Use it: the breakdown's default `place` (x, y, h), entrances' end marks,
   shot framing (P5 shot kinds → camera size/position) and S2's panels all take
   their numbers from `StagingStyle` instead of fixed constants. The breakdown
   prompt gets a short text summary of it (e.g. "two people talking: thirds,
   facing each other, spaced 0.9 figure heights").

### Profiles are per channel (user, 2026-10-23)

`dataset2` is the **Khayal 3 Baje** channel: night/dusk, moody, painted
village horror-story look. **Originals** (the Chirailiya series, ep1) is a
**comic sitcom**: illustrated, light-mood, colourful daylight backgrounds.
So `StagingStyle`, the background prompt style, the panel B look and the
lighting defaults are a **channel profile** (`Khayal 3 Baje`, `Originals`,
more later), chosen per project/episode (the Producer's `--series` /
`--style`), never one global style.

* **Khayal 3 Baje**: learned from `dataset2` (below).
* **Originals**: no reference frames yet. Until the user gives some, sitcom
  conventions: bright, even light; colourful illustrated sets; medium shots
  and two-shots for dialogue, characters cheated three-quarter toward camera,
  readable faces (figure 0.5–0.8 of frame height in mediums), clear
  left/right speaker sides kept across cuts, wides to establish the set and
  for group gags; little deep-shadow, no night unless the script says so.
  ep1 (S7) uses this profile.

### S0 dataset (`dataset2\dataset2`, Khayal 3 Baje, reviewed 2026-10-23)

341 stills, all frame-sized (224 × 1920×1080 PNG, 117 × 1280×720 JPG).
Contact sheets: session scratchpad `s0\sheet0–5.png`, `s0\people.png`.

* **~70 % are the user's asset library, not shots**: isolated props and
  buildings on white (house1–49, tree*, palmTree*, bush*, grass*, ground1–20,
  skies, lamps, tents, well, tapri, vehicles, phones) and ~150 empty
  background plates (`background1–117`, sideRoad*, OLDvILLAGE, RIVER…).
  Use: S6 references search these first (via `buzz-ai` asset_index) before
  generating; S2-B prompts and palettes take their look from them.
* **~25 frames have characters**: Diwali*, diwali1/11/1s, RECOVER_Tower4/4a/4d,
  background10/11/67/68/75/76, chhath1a/22/24/31/4/5, pret2A1, raja1/1a/3/31/3a/43.
  What they show (to be measured in S0, these are the eye-read starting
  points): wide and full shots; figures 0.15–0.45 of frame height, feet on a
  path or open ground in the lower 40 %; **depth staging** (one figure near
  and big, a group mid, small figures far — RECOVER_Tower4); **backs to
  camera** looking into the scene (background67, chhath24); characters set on
  the path that leads the eye; groups clustered and overlapping, sitting in
  circles round a fire/lamp (raja43); mostly **night/dusk**, lit by lamps,
  moon, torch.
* Few or no medium, close-up or dialogue two-shots: S0 learns wide staging
  from these; medium/close use conventions until the user adds frames.
* **`storyboardChatth.png` / `storyboardChatth1.png`**: the user's own
  storyboard frames are **full-colour painterly concept pictures** (soft
  illustration, sunset river, Chhath crowd), not pencil sketches. S2-B's look
  follows that: painted concept frame, with "pencil" as an option.

## S1 — storyboard data

`storyboard.json` next to `casting.json`: `Board { title, panels: Vec<Panel> }`,
`Panel { id, scene, shot, kind, on, from_line, to_line, duration_s, action,
dialogue: Vec<(speaker, text)>, camera: {move: hold|pan|push|pull, from, to},
place overrides (x, y, h, facing) per character, pose/gesture hint, look:
Real|Sketch|Auto, notes }`. Built from the breakdown's shots and lines plus
`StagingStyle`; one panel per shot, extra panels where a shot holds a big
action change (entrance, exit, gesture). Round-trips; the user's edits are kept
when the breakdown is re-run (panels keyed by scene + shot + line).

## S2 — panels

* **A, real scene setup**: build the panel's scene headless (as
  `episode_build` does), seek to the panel's key moment, render through the
  shot's camera at board size (e.g. 960×540). Same rigs, backgrounds, lights
  as the final.
* **B, AI sketch**: ComfyUI, the user's storyboard look (painted concept
  frame like `dataset2\storyboardChatth.png`; pencil as an option),
  prompt from the scene's `background_prompt`, the characters' `description`s,
  shot kind, action. Composition steered by the panel's layout: a pose
  skeleton / depth guide drawn from the `StagingStyle` positions (ControlNet
  if the workflow has it, else img2img from a rough real-layout render).
* **Auto** (default): A when every character in the panel has a Cast rig and
  the set exists; B otherwise; per panel, switchable.

## S3 — board sheet + Storyboard panel

* Export: pages of 6 panels (3×2) with the caption block under each
  (scene/shot, kind, action, dialogue, camera, duration), PNG per page and one
  PDF.
* App: **Window ▸ Storyboard** panel — grid of panels; edit any field; drag to
  reorder; add/delete/split; per panel Regenerate, look A/B; "Open scene"
  jumps to it. Command palette + agent actions (`storyboard_make`,
  `storyboard_edit`, `storyboard_export`, `storyboard_animate`).

## S4 — animatic

Panels held for their durations, the dialogue audio when there is any (P2
alignment), else the text timing; the camera move as a pan/zoom on the panel;
captions optional. mp4 via the existing export.

## S5 — animate from the board

`produce` takes `storyboard.json` when present: shots, durations, places,
facings and camera moves come from the board instead of the breakdown.
Unchanged boards give the same episode as today.

## S6 — scene setup references

Per `location_key`: 3–4 ComfyUI reference pictures (wide establishing,
reverse angle, key props close), and a **top-down layout map** (drawn, not
generated): set outline, props, each character's marks and entrance paths,
each shot's camera position and angle. Saved in the scene as a guide layer
(not in renders) and as PNGs beside the board.

## S7 — verify (planner, images looked at)

ep1 scenes 4 and 5, then the whole script: board sheets, a mixed A/B board
(remove one Cast rig to force B), the animatic, one board edit carried into
the produced episode, the reference sheets. S0: the snap check sheet, and the
same ep1 scene staged with and without `StagingStyle` side by side.

## Constraints

* No `cargo fmt`; no new tests (user rule 2026-10-11), run existing suites.
* Headless verification only; never real mouse/keyboard.
* Sketch panels must never replace real ones silently: the panel says which look it is.
