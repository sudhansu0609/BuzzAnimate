# Animated channels: Khayal3Baje and Originals end to end (2026-10-29)

The creator's ask: the agents read the whole script, write prompts for backgrounds and
set-ups, generate them with **his own ComfyUI flows and LoRAs**, use the characters and
poses he marked and placed, and do lip sync and basic pose movement automatically.
Originals gets easy-going illustrated backgrounds whose objects (houses, trees, shops)
are traced into **symbols on their own layers** for animated effects. Done in BuzzAnimate.

Built in worktrees `B:\youtubeProjects\_worktrees\animate\{BuzzAnimate,BuzzcafStudio}`
(staged baseline = the live state, including BuzzAnimate's 51 uncommitted in-progress
files; the feature is the unstaged diff + new files). Merge into the live trees only after
review, 3-way per file, when nothing is being produced. The live BuzzAnimate binary and
its `target/` are never touched: the worktree builds into its own `target/`.

## Decisions (creator, 2026-10-29)
- **Placement and poses come from all three:** pose pictures in per-character folders,
  his placements on the BuzzAnimate stage (kept on rebuild), and script tags
  `[Name: pose, position]`.
- **Khayal3Baje voice:** one narrator (him, `NARRATOR (V.O.)`, heard never seen) plus some
  quoted dialogue that the speaking character lip-syncs. During narration the characters
  act (idle / react / walk), mouths closed.
- **`sudhansujibMax` LoRA = the Khayal3Baje art style:** on every Khayal3Baje background
  and every ComfyUI-drawn Khayal3Baje character.
- **Originals world:** a 2010s semi-rural small town in North India (smartphones, bikes,
  small shops of that decade), easy-going illustrated look.
- **His workflows are copied, never edited** in `C:\Users\singh\Documents\ComfyUI\user\default\workflows`.
- **Tests:** 2-3 small ones per item, no long or real-render tests; prove visuals by stills.

## What already exists (do not rebuild)
Producer (`docs/PRODUCER_HOWTO.md`, `PRODUCER_PLAN.md`): screenplay parse + Gemma breakdown
(`buzz-ai/src/breakdown.rs`, narrator/V.O. supported), casting.json, cast from character
folders (`buzz-app/src/cast_build.rs`), ComfyUI backgrounds (`buzz-app/src/backdrop.rs`,
one fixed Z-Image API workflow bound by node ids 70/66/69/9) traced into Sky/Far/Mid/Ground
plates, audio alignment, lip sync, blinks, breathing, talk/idle/walk, entrances, captions,
shots, lights, sets from own assets (`asset_sets.rs`), `build_episode` in
`buzz-app/src/episode.rs`, CLI `cargo run --release -p buzz-app --example produce`.
Local AI: SAM 2 + ISNet mattes (`buzz-ai/src/segment.rs`), DWPose (`pose.rs`), VLM object
boxes (`scene_builder.rs`), outpaint (`outpaint.rs`).

## Items
- **AN1 Series ComfyUI profiles.** Copy the creator's workflows into
  `assets/comfy_profiles/<Series>/` (Khayal3Baje: `urbanScenesKhayal3baje.json` for
  backgrounds and characters -- Juggernaut XL + `sudhansujibMax-000037` LoRA;
  `secondurbanScenesKhayal3baje.json` as the alternate; `Horror_View_Change_StrictLoRA.json`
  noted for later angle changes). Convert UI-format to API-format once (ComfyUI
  `/object_info` when ComfyUI runs; keep the converted copy next to the original copy) and
  bind by node **class and wiring**, not ids: the CLIPTextEncode feeding KSampler `positive`
  (prompt), the one feeding `negative`, EmptyLatentImage (size), KSampler (seed), SaveImage /
  "Image Save" (prefix); the LoRA stays as saved. A profile json per series names its
  background and character workflows, base size and style text. `backdrop::workflow_path`
  and `cast_build` pick the series profile, else the current Z-Image default.
- **AN2 Originals world + look.** The Chirailiya / Originals preset becomes "a semi-rural
  small town in North India, 2010s" with an easy-going illustrated style (existing comic
  Z-Image path), wherever the world string and style presets live.
- **AN3 Background objects as symbols.** After a background is generated: the VLM lists
  objects (house, tree, shop, pole, cart, wall...) with boxes; SAM 2 masks each; each mask
  is traced into its own library symbol on its own layer at its depth (between the plates),
  named `<Object> N`; the hole it leaves in its plate is filled (outpaint / surrounding
  colour) so moving the symbol never shows a hole. On for Originals by default, a flag for
  any series.
- **AN4 Placement and poses.** (a) Pose pictures: files in a character folder named by pose
  (`sitting.png`, `scared.png`, `pointing.png`) become that rig's pose variants; the
  breakdown picks a pose per line/beat. (b) Script tags `[Name: pose, position]` (position:
  left / centre / right / door / x%) parsed in the breakdown and written to casting, beating
  the model's choice. (c) Stage: before `--redo build`, read the existing `.buzz`; character
  instances he moved or posed (transform/pose differ from what the last build wrote, kept as
  a hash in `produce.json`) are written back into casting as pinned overrides and survive the
  rebuild.
- **AN5 Narrator format check (Khayal3Baje).** Narrator lines: nobody's mouth moves,
  characters idle/react; quoted lines: the speaker lip-syncs. Verify with a short script and
  stills; fix only what is wrong.
- **ST1 Studio hand-off.** Khayal3Baje and Originals workflows get an "Animation Screenplay"
  step (output `screenplay`): the script rewritten as a screenplay (sluglines, `NARRATOR
  (V.O.)` for narration, `NAME:` for quoted lines, `[Name: pose, position]` tags) without
  changing a spoken word. A "Produce animation" action runs the BuzzAnimate producer CLI on
  the project (script, the recording or narration audio, the series character folder, style,
  series, world) as a background job holding the one-video slot, then puts the `.mp4` in the
  project for the normal publish steps. These channels no longer get BuzzEdit stills plans.

## Corrections and additions (creator, 2026-10-29, later)
- **His models are Z-Image Turbo with LoRAs trained on Z-Image Turbo**, not SDXL. Khayal3Baje's
  style LoRA is `zImageTurboKhayal3Bajev1.14` (newest; flow `Z-Image-Turbo-LoRA.json` uses its
  step-4000 file, `image_z_image_turboLora2.json` the final one; `zimageKhayal3Bajev1.1` is the
  older one in `image_z_imageLora.json`). Base `z_image_turbo_bf16` (B:) / `zImageTurbo_turbo`,
  text encoder `qwen_3_4b`, vae `ae`. The LoRAs were parked on H: and are now **copied** back into
  the B: portable `models/loras/` (H: copies kept). The urbanScenes SDXL flows stay copied but are
  not the default.
- **AN1b** Khayal3Baje profile -> Z-Image LoRA flow (copy `Z-Image-Turbo-LoRA.json` and
  `image_z_image_turboLora2.json`; default = turboLora2 with the final v1.14 LoRA), bound like the
  others. Any "Image Save" (WAS) node gets its `output_path` rebound into the run's output folder
  (it currently writes into the creator's `B:\youtubeProjects\Khayal3Baje\stories\kalwa\pics`).
- **AN7 Definition.** Every generated picture is upscaled before tracing (pattern of his
  `B:\youtubeProjects\Khayal3Baje\zturboupscale.json`: ImageUpscaleWithModel then ImageScaleBy to
  the target; 4x-UltraSharp is not installed -> RealESRGAN_x4plus / x2plus from B:), then checked:
  sharpness (variance of the Laplacian on a downscaled grey copy) above a threshold, no people
  where the plate must be empty (the existing people check), size at least the trace size. A
  failed check regenerates with a new seed (max 2), then keeps the best and flags it in the report.
- **AN8 Trace quality.** After tracing, render the vector at the picture's size and compare with
  the picture (mean colour error / SSIM on a downscaled copy); below threshold, retrace with the
  next tuned option set (`docs/TRACE_TUNING_PLAN.md`, `TRACE_QUALITY_PLAN.md`), keep the best,
  record the score per background in the report and `backgrounds.json`.
- **AN9 Prompts and angles per shot.** The breakdown gives each shot its own camera (wide / medium
  / close / low angle / high angle / over-the-shoulder / reverse) chosen from what happens in the
  shot; one location keeps one look (the same location description, seed family and lighting) and
  each shot renders its angle of it. Prompts are written from the scene (place, time, weather,
  mood, era/world, the series style text) -- specific nouns, the angle words, nobody in frame for
  plates -- and logged per shot. Same-location angle consistency: first try Z-Image with the shared
  description + angle + seed family; his `multiple_scene_angles-v1.0.json` (a subgraph flow that
  re-angles one reference picture) is copied and wired as the consistency path only if its models
  are installed (report which are missing otherwise).

- **AN10 Speaker-aware lip sync and stillness** (creator, 2026-10-29; replaces AN5). From the
  recording, the audio step finds each line's span and therefore its speaker (a character, or
  the narrator). Only that character lip-syncs, from the audio, inside that span. Everyone not
  speaking stays still: mouth closed, no talk gestures; only natural blinks and faint breathing
  (a `still_extras` switch can turn even those off). Narration: no mouth moves, characters do
  only what the action lines say. Movement comes from the script: an action line that says a
  character stands, sits, points, turns or walks triggers that move at that moment (pose
  variants from AN4 where present); otherwise the pose holds. Verified with a short narrated
  script with two characters and one quoted line: stills show only the speaker's mouth open in
  the quoted span, closed everywhere else; report lists every lip-sync span with its speaker.

## Status
| Item | State | Notes |
|---|---|---|
| AN1 Series ComfyUI profiles | done | `buzz-app/src/comfy_profile.rs` (UI->API via /object_info input_order, `.api.json` cache, binder by wiring), assets/comfy_profiles/Khayal3Baje/ (3 copies + profile.json), backdrop::generate + cast_build::generate_sheet use it, falls back to Z-Image with a warning; 2 tests |
| AN2 Originals world + look | done | Chirailiya default world in `buzz-ui/produce_dialog.rs::default_world` (+ hint, agent text, HOWTO); comic look unchanged |
| AN3 Background objects as symbols | done (not run live) | `buzz-app/src/bg_objects.rs` (VLM boxes -> SAM 2 masks -> traced symbols, plate by bottom edge, hole fill), `backdrop::save_asset_with_objects`, backgrounds step (`objects`/`object_notes` in backgrounds.json + report.md), `Profile.objects_as_symbols`, `ProduceRequest.objects`, `--objects`/`--no-objects`; default on for Chirailiya/Originals; 2 tests |
| AN4 Placement and poses (folders, tags, stage) | done | cast_build pose variants; breakdown `parse_tags`/`apply_stage` (tags beat the model); stage_pins wired in produce.rs build step (hashes in produce.json `written`, pins.json); 2 tests |
| AN5 Narrator format check | merged into AN10 | |
| AN10 Speaker-aware lip sync + stillness + script-driven moves | done | episode.rs: speaker-only mouths, `still_extras`, action-line stand/sit/point/turn moves, `lipsync` report section; per-shot background_for + pose-variant swap left as TODO; 1 test |
| ST1 Studio hand-off | done | Studio worktree: animate_bridge.py + animate_bridge_api.py, screenplay step in 2 workflows, not_animated condition, AnimateProduce.tsx; 2 tests + tsc ok |
| ST2 Cast extraction + verbatim screenplay | done (not run live) | Studio worktree: app/services/animate_screenplay.py (code splits units, model extracts cast + labels only, code copies his sentences into BuzzAnimate's `## SCENE`/bold-cue format and verifies spoken text identical), per-channel cast registry knowledge/cast/<Brand>.json, production/cast.json, workflow `_screenplay_step`, animate_bridge writes description.txt for cast without a folder (producer reads none; look also in the first action line); 3 tests; live 2026-10-29 on his first-person sample: 37 s on Gemini Flash, verified verbatim; Opus fixed cast merging (shared role words / blank looks merged two guards) |
| AN1b Khayal3Baje -> Z-Image LoRA flow; Image Save path rebound | done (verified) | profile.json -> image_z_image_turboLora2 1280x720; comfy_profile rebinds Image Save output_path; 3 comfy_profile tests pass |
| AN7 Definition (upscale + sharpness/people/size gate) | done | definition.rs (SHARPNESS_MIN 40, upscale_in_place, never fails run), backdrop.generate_checked (3 tries, keep best, definition_flag in backgrounds.json) |
| AN8 Trace quality gate (render-compare, retrace) | done | definition::trace_checked (MAE<=28, SSIM>=0.55), trace_score/trace_tries/trace_notes per background in backgrounds.json |
| AN9 Prompts and angles per shot | backdrop/produce half done; build side with other implementer | backdrop.rs: angle_key (wide = location, else loc__camera), plan_angles (wide + top 3, max 4), `background_for(&Value backgrounds_json, location: &str, camera: &str) -> Option<String>` (falls back to wide); produce backgrounds step draws each angle from shot.background_prompt + seed_family |
| AN11 Per-shot angle cuts + re-staging + pose swaps | done | episode.rs: angle pictures on own layers shown only in their span (`hide_during`), `angle_restage` (reverse mirror, over_shoulder, low/high ground shift by horizon_row, pinned kept), `pose_beats` variant rigs at action lines, Angles section in report.md; 2 tests |
| AN12 Test fixes: camera-framed medium/close, Qwen angle flow, memory release, missing-model precheck | done (not run live) | backdrop.plan_angles = reverse/low/high/over_shoulder only (medium/close use the wide); `generate_angle` + `comfy_profile::qwen_angle_graph` (his multiple_scene_angles subgraph settings built in code, copy + `angle_workflow` in Khayal3Baje profile); `comfy_profile::missing_models` precheck (up front in report + log, skip cleanly); produce.rs `release_lm_studio` after breakdown/end, `ComfyClient::free_memory` at end; 2 new tests |
| AN13 Live-test fixes: captions, over-shoulder framing, pose fallback, prompt nouns | done (not run live) | captions: profile.json `captions` (Khayal3Baje false), `comfy_profile::captions_enabled`, `--captions on/off`, `BuildOptions.captions`; angle shots keep camera x1.0 (`apply_angle_framing`, one shot list); tagged pose with no picture -> rig's own Sit held until Stand, else Skipped reason (`PlaceSpec.pose`); breakdown `set_nouns` lead the background prompt, cap 70 words; 3 new tests |
| AN6 Review (Opus) + stills + merge | done | 2026-10-29: live tests (Khayal LoRA backgrounds sharp 47-57, Qwen over-shoulder = same courtyard, speaker-only lip sync, first-person sample converted verbatim in 37 s + animated); merged 28 BuzzAnimate + 16 Studio files 3-way (live unchanged since baseline); test leftovers kept out; polish left: 'standing' as default pose, white oval under feet, trace blotches in dark skies, vehicles |

Each implementer updates its row (todo -> doing -> done, one-line note: files, tests,
stills) after every item, so a cut-off leaves a hand-off.
