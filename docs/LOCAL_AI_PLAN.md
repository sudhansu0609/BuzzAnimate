# Plan: local AI in BuzzAnimate

Requested 2026-09-24. **Priorities (user):** the AI integration first; **character
from PNG** (cut out parts → complete hidden parts → trace with equal-width
lines → colour → rig → poses) is the top priority within it. Then: identify
and rename assets, rig/re-rig existing characters, recognise poses and motions
(walk, run, hands, eyes), build a scene from a reference image, and
LLM-directed scene setup, placement, lip sync and movement.

Workflow: Opus plans; **one Sonnet implementer at a time, one chunk per run**;
Opus verifies each chunk (tests plus output images) before the next. Briefs must
be token-lean: exact files, grep before reading, build and test once per change.

## Status (keep current: update after every run and every review)

> **RESUMED 2026-09-26 by the user** (F7 round, see the F7 section). F1–F6 are done and reviewed; F7 rows below are kept current per item.
| Step | State | Date | Evidence |
|---|---|---|---|
| C0 foundation (LLM backends, model registry, ONNX) | done, verified on real models | 2026-09-24 | real downloads with SHA check; DirectML on RTX 5060 Ti |
| C1 matte and skeleton | partial | 2026-09-24 | arms/torso mostly right; legs weak (12–47% max error) |
| C2 part masks | redesign done; visually good; metrics unverified | 2026-09-24 | full-coverage label map (ai_review3/*/labels.png); IoU scorer broken |
| C3 completion | redesign done; one artefact | 2026-09-24 | vector joint caps hold when arms rotate 45°; a wrong-colour cap disc on OldmanfrontWalk's foot |
| C4 trace | redesign done; visually good; one artefact | 2026-09-24 | faces intact, uniform outlines, warm total 2.0 s; a stray dark band on pujari's hem; line_dist scorer saturates at 11.31 |
| Metrics fix + 2 artefacts | done; honest metrics show real misses | 2026-09-25 | line_dist was capped at 8·√2 and is now an exact EDT: mean 14.6 px, p95 63 px (target ≤ 1.5); C1 47%; C2 IoU 0.0 where the truth is fully occluded (a scorer quirk) and on real misses; the foot disc is fixed; the hem band is partly fixed, with the rest caused by `buzz_scene::trace` ColourLines fill colour (outside that run's scope); warm total 2.26 s |
| **PLAN PAUSED** (effective 2026-09-25, after the metrics-fix run; user request): Animate import performance comes first, see `docs/IMPORT_PERF_PLAN.md` | paused | 2026-09-24 | – |
| Note: the C5–C11 code came from the user's work with another model (see "Queued by the user"). It builds as of 2026-09-25; reviewed by Opus 2026-09-25, see "Opus review of C5–C11" | reviewed | 2026-09-25 | – |
| C5 assemble and rig, UI command | built; **failed review** | 2026-09-25 | assembles and saves; but fixed symbol ids overwrite existing Library symbols; rest render loses the face and misplaces parts (`scratchpad/ai_verify5/sheet.png`); armature not placed on stage from the UI |
| C6 faces | built; **failed review** | 2026-09-25 | generic eyes pasted over the head; the traced face (mustache, features) is gone; mouth not visible |
| C7 poses and motions | built; **failed review** | 2026-09-25 | walk/run/sit/point tear the rig apart (limbs detach, head rotates off); classifier tests are synthetic |
| C8 asset naming | built; **failed review** | 2026-09-25 | mock VLM only; thumbnails render on the UI thread with a new GPU exporter per symbol, unframed, nested symbols missing; also renames unrelated on-disk assets, outside undo |
| C9 re-rig existing characters | built; **failed review** | 2026-09-25 | the app uses a hard-coded proportional skeleton (no detection); armature added as a copy beside the original; pivots not set; timeline classifier labels the whole range once |
| C10 scene from image | built; **failed review** | 2026-09-25 | mock VLM only (the 75% recall is 3 of 4 made-up objects); SigLIP never called; poses a debug log; left-facing mirror shifts the object; blocks the UI thread |
| C11 LLM director | built; **failed review** | 2026-09-25 | mock LLM only; dialogue lines and gaze dropped (gaze is a debug log); blocks the UI thread |
| F1 fix: character ids + place on stage (review items 1, 2) | done, reviewed | 2026-09-25 | `Editor::create_ai_character` merges `assembled.scene` (ids renumbered) in one `doc.edit`, centred on the view; test `creating_an_ai_character_merges_instead_of_overwriting_the_library` (user symbols 1, 100–130 survive two creations; 2 armatures; one undo step each) |
| F2 fix: no UI freezes; C8 thumbnails and renames (items 5, 6); the character dialog's blocking `rfd` picker | done, reviewed | 2026-09-25 | `TaskKind::Ai` + `AiResult` channel for Scene from Image / Direct Story, commit in one `doc.edit`; Identify & Rename renders on a background thread with one Exporter, fitted framing, whole library, Cancel; on-disk asset rename removed; names deduped; picker threaded. Tests: 5 `ai_` app tests, `asset_naming` 5/5, thumbnail framing test ok. Unrelated: 9 `buzz-ui` workspace layout tests fail from separate uncommitted panel work |
| F3 fix: poses relative to rest (item 4) | done; **not enough** | 2026-09-25 | poses are now rest + delta (`scratchpad/f34/sheet.png`): idle good; walk/run/sit/point still tear, wave shows no change. Second cause below (F3b) |
| F4 fix: rest render keeps the traced face (item 3) | done, reviewed | 2026-09-25 | dialog, CLI and tests use `trace_character_whole`; rest render keeps mustache, pocket, colours; grey slab gone (`scratchpad/f34/sheet.png`). Left: generic googly eyes over the drawn eyes; black band at the kurta hem (F4b) |
| F3b fix: bone head offsets; armature from the detected skeleton | done, reviewed | 2026-09-25 | `Bone::offset` (serde default 0; old files load; FK, IK, pose_transform honour it) and `autorig::assemble_from_joints`; AI assembly and re-rig use the C1 joints. Renders (`scratchpad/f3b/wide.png`): walk/run/sit/point bend at shoulders, hips, knees, parts stay attached. buzz-rig 94, buzz-act, buzz-doc 189, buzz-ai, bone_rig all pass |
| F4b fix: face rig from the eye landmarks, bound to Head; kurta hem band; joint-cap gaps; wave forearm up | done, reviewed | 2026-09-25 | `scratchpad/f4b/review.png`: rest matches the source's face (eye size/colour from landmarks, drawn eyes de-doubled, mustache intact), no hem band (a large near-black ColourLines fill over Hips is recoloured unless the source is dark), hand/foot caps widened to 1.3× limb width, wave hand above the elbow, eyes stay on the head in point. Left: eye-white a little lighter than the source; point tilts the head strongly |
| F5a fix: C10 placement/mirror/pose report, C11 dialogue/targets/gaze | done, reviewed | 2026-09-25 | C10: bounds origin honoured, bottom-centre ground contact, mirror about the bounds' centre, pose recorded on `MatchedObjectReport.pose`. C11: `dialogue_cues` from `direct`'s Talk frames → `Editor::sync_directed_dialogue` (captions + per-speaker lip sync through the `.srt` join); lip sync needs a soundtrack, else captions only with a status; dead gaze loop removed (`direct` has no gaze control). Captions may add their own undo step |
| F5b fix: C9 detect the skeleton (render + DWPose), no duplicate drawing; C10 SigLIP embeddings | done; SigLIP real-model test not run | 2026-09-25 | Auto-Rig: Ai task renders the symbol, DWPose, joints mapped back, replaces the selected instance in place; parts carry every object of a layer. `buzz_ai::siglip` (224², (x/255−0.5)/0.5, pooled, L2) wired into Scene from Image when installed, status says when it isn't. Tests: 2 app tests + preprocessing test pass; `siglip_embed` ignored (model not downloaded) |
| F6a real-model runs (LM Studio: gemma-4-12b vision, qwen3.8-27b; bhatta2.fla) | done, reviewed | 2026-09-25 | harness `examples/ai_real_check.rs`, report `scratchpad/f6/report.json`. Fixed on the way: `llm.rs` connect 5 s / request 900 s + readable `LlmError::Timeout`; SigLIP crashed natively inside DirectML → loads on CPU (`onnx::pooled_cpu`), cosines 0.990 same / 0.786 different. **C8** 16 symbols, 0 parse failures, names good, but 364 s (reasoning model, ~3 min/batch). **C9** `sadhuback`: DWPose ok, 12/14 layers map right; palms → Hips (0 overlap), wrists → upper-arm slots. **C10** recall 0 / precision 0 on both frames, with and without SigLIP (labels are descriptions, library names are not; body-part symbols compete as candidates). **C11** 6/6 verbs right, 214 frames, 399 s; the model left `dialogue` empty so no cues. Ignored model tests 5/6 (ground-truth skeleton Foot R 78.6 > 75.5, known C1) |
| F7 next round | **in progress (resumed 2026-09-26); per-item rows below** | – | 1. C10 redesign: candidates = placeable top-level symbols only; SigLIP crop↔thumbnail as the primary score with a threshold, names/tags secondary; score recall against visible top-level instances. 2. C11: prompt to copy quoted speech into `dialogue`, plus a fallback that lifts quoted text from the brief. 3. C8/C10/C11 speed with reasoning models: ask for no/low reasoning where the server supports it, or recommend a non-reasoning model in AI Models; show elapsed time. 4. C9: hands/wrists to the forearm slot (the biped's Elbow slot carries the hand). 5. `buzz-export` Exporter readback panic when reused across different output sizes. 6. A bare Armature on a stage layer rendered blank in the harness (the app's merge path renders fine) — check. 7. Small brown dot below Oldman's left knee. |
| F6b end-to-end UI flow (App::new + build_ui, real ONNX models) | done, reviewed | 2026-09-25 | `character_from_image_end_to_end_through_the_ui` (ignored, needs models) passes on pujariFront_0 and OldmanfrontWalk_0: detect 1.5 s, process 1.7 s, create 0.07 s; 11 bones, Cast saved (temp library), +1 undo, eyes/mouth inside the head bounds. Bug fixed: `start_parts_processing` rebuilt the skeleton from 13 joints in a zeroed Person (face landmarks lost, Head written to the nose slot); now `apply_edited_joints` onto the detected skeleton (+3 unit tests). Renders `scratchpad/f6ui/sheet2.png`: pujari good; Oldman has a stray disc near the left foot and a blob above the head (F4c) |
| F4c fix: stray cap/blob artefacts on OldmanfrontWalk | done, reviewed | 2026-09-25 | disc: cap radius now ≤ half the bone length. Islands: `segment::drop_small_specks` (< 0.1% of the main body) + `PartMasks.props`: other disconnected components never reach a bone and become unbound "Prop N" layers in place (tests `matte_drops_a_tiny_speck_but_keeps_a_substantial_disconnected_blob`, `disconnected_prop_stays_out_of_every_bone_part_and_becomes_an_unbound_layer`). `scratchpad/f6d/om_sheet.png`: bowl stays put across poses. Left: a small brown dot below the left knee in every pose |
| F7.8 Claude API server backend (UI half) | done | 2026-09-26 | Models section: base URL, masked key field (password; hint `sk-local-… (stored)`), Save key (→ `ai.json`, owner-only, field cleared), Test connection (`GET /models` on a thread, ms + model count), Use this server, chat/vision model dropdowns from the server's list, "which backend each flow uses" table |
| F7.9 transcription (UI half) | done | 2026-09-26 | Transcribe section: library sound picker, speaker, Run (TaskKind::Ai, progress per 30 s window, Cancel), editable timed lines, Apply → `Editor::apply_transcript` (puts the sound on the timeline if it is not, then captions + the speaker's lip sync through `sync_directed_dialogue`) |
| F7.2 C11 dialogue | done; real runs pass | 2026-09-26 | Prompt + `fill_dialogue_from_brief` (straight, curly and spaced single quotes). **Found on the real Claude run and fixed:** `direct` read "Old Man" as two people ("Old", "The Man") and "Village." as a character, so the old man's line never became a cue — actors now go to `direct` by `director::stage_name` ("OldMan"), cues match by it, "village"/"field" are scenery words in `direct`'s NOT_NAMES, and overlapping talk ranges are clipped so captions never overlap. Tests `a_two_word_actor_keeps_his_line`, `single_quoted_speech_counts_but_apostrophes_do_not`. Real (`ai_real_check`, brief with two quoted lines): LM Studio qwen3.8-27b 68.7 s, 8 beats, verbs right, 2 cues ("The old man", "The Sadhu"); Claude sonnet 2.8 s, 7 beats, 2 cues (Old Man, Sadhu) |
| F7.4 C9 hands → forearm | done; real run good | 2026-09-26 | `rerig::is_hand_or_forearm` (wrist, palm, fist, finger, thumb, hand, forearm, elbow) gives the Elbow slot the name bonus; the upper-arm rule no longer claims them; a part with no overlap and no name goes to the nearest same-side bone, not slot 0 (Hips). **Also found on the real run and fixed:** layers named from the viewer's side put each arm's parts on the other arm's bones (the forearm flew off in wave) — `map_parts_to_slots` now maps both ways (names as written / L↔R swapped) and keeps the reading with more geometric overlap; "righ" typos (`righFoot`) count as right. Tests `hands_and_wrists_ride_on_the_forearm_slot`, `parts_named_from_the_other_side_still_land_on_their_own_limb`. Real (`ai_real_check --only-c9`, sadhuSide, DWPose 0.704): 15/15 layers on plausible bones, walk/wave stay attached (`scratchpad/f7/local/c9_sheet.png`: joints, rest, walk, wave). Left: the mustache sits a little low with a seam line in poses |
| F7.5 Exporter reuse across sizes | done | 2026-09-26 | Could not reproduce the panic with plain renders (`one_exporter_renders_frames_of_changing_sizes`: 550×400 → 256² → 1100×800 → 256² → 97×301 → 550×400, colour checked) nor on bhatta2 (`examples/exporter_reuse.rs`: stage ½, 12 symbol thumbnails, full 1920×1080, three rounds, one exporter, OK). The panic text points at `read_raw` calling `get_mapped_range` after a failed map (it only logged the map error). Fixed: the map result is received and checked, a failed/unfinished map or poll error is an `Err`, and the target is dropped so the next frame rebuilds it; `ai_real_check` now uses one exporter throughout |
| F7.6 bare Armature renders blank | done | 2026-09-26 | Two causes, found with `examples/armature_alone.rs` (sadhuback assembled into a rig, drawn alone): (1) the harness cloned bhatta2's scene **with its camera enabled**, which reframed the render off the rig (`render_symbol_alone` already disabled it; `render_armature` did not) — harness fixed; (2) `Scene::resolved_bounds` measured an Armature by its parts' placeholder instance bounds (-270..1228 wide instead of 230..627), so framing was wrong even without a camera — library fixed (`armature_resolved_bounds` in `buzz-scene`, used by both the memo table and `resolved_bounds_with`; test `an_armature_with_instance_parts_resolves_their_real_bounds`). After both: 135 509 opaque px, the whole back-view sadhu framed (`scratchpad/armature_alone.png`). Also found: plain `autorig::assemble` fits bones from the same placeholder bounds (tiny bones) when parts are instances; the app's Auto-Rig and the harness both go through `rerig_vector_character` → `assemble_from_joints` (bones from DWPose joints), so neither is affected; noted, not changed |
| F7.7 dot below Oldman's left knee | done | 2026-09-26 | It was a joint cap: DWPose guesses the far leg's ankle below where the foreshortened shin ends, and the Foot part got a cap disc there. `trace_character_whole` now caps only joints inside the matte (distance transform > 0.5). `ai_character --stage assemble` on OldmanfrontWalk_0 + `zz_buzz_sheet`: dot gone, nothing else changed (`scratchpad/f77/oldman_rest_after.png`). buzz-ai tests all pass; ignored model tests 6/7 (only the known `ground_truth_skeleton` Foot R 78.6 > 75.5) |
| F7.1 C10 redesign | done; recall target **not met** | 2026-09-26 | Candidates = `placeable_candidates` (105 of bhatta2's 589 symbols); ground truth = `visible_top_level_symbols` **through the frame's camera** (frame 0 is a close-up: 8 visible, not 11+); crops ↔ thumbnails SigLIP (`siglip_shortlist`, near-duplicate copies folded), then the vision model picks from a numbered `choice_sheet` (crop + top 6) via `confirm_with_vlm`; the app rule is "confirmed pick, else SigLIP best ≥ 0.80". **Found and fixed on the way:** every library thumbnail rendered blank because bhatta2's camera framed the thumbnail stage (`render_symbol_thumbnail` now disables the camera) — C8's names had come from old names only. Real, recall/precision (frame 0 / mid frame): **Claude sonnet** combined 0.375/0.50 and 0.385/0.42 (SigLIP alone 0.25/0.67, 0.15/0.67), 28–44 s; **LM Studio gemma-4-12b** combined 0.125/0.25 and 0.0/0.0, SigLIP alone 0.50/1.0 and 0.0/0.0 (gemma's confirmations are mostly wrong), 26–32 s. Below the > 0.5 target: near-identical character symbols (three boys, two orange-shirt old men) and scenery the VLM names differently from the library's split. Evidence `scratchpad/f7/{local,claude}/report.json`, `c10_*_choice_*.png`, `c10_*_reconstructed.png` |
| F7.O1 open item 1: C4 trace distance | done: the 14.6 px was the scorer | 2026-09-26 | `ai_verify` scored every traced stroke against the *silhouette* only, but most strokes are interior ink (eyes, mustache, collar, folds). New `source_edge_distance_field` (source ink luma < 90, colour steps > 60, matte boundary): mean 0.15–0.47 px, p95 1.00–1.41 px on all six fixtures (target ≤ 1.5 met); the old silhouette figure is printed beside it (mean 5–22 px). Max outliers 13–29 px remain (a few strokes with no source line under them). The dark fill on pujari's dhoti folds was not re-investigated beyond F4b. Evidence `scratchpad/f7open/verify.log`, `*/traced.png` |
| F7.O2 open item 2: C1 47% joint error | explained | 2026-09-26 | `scratchpad/f7open/pujariFront_0/overlay_joints.png`: the truth joints (magenta) are the rig's layer pivots, several off the drawn limb (the lifted leg's knee sits right of the dhoti, a shoulder above the collar), while DWPose's (cyan) sit on the anatomy; one real detector miss: both ankles on the visible foot (the lifted foot is hidden). So most of the 47% (legs, pujari) is the truth convention, not the detector; arms/torso 8.5–15% on 5/6 fixtures, ThakurBhai 46.9% (front view, as-labelled) |
| F7.O3 open item 3: C2 occluded parts | done | 2026-09-26 | `ai_verify` skips truth parts with no visible pixels in the IoU minimum and lists them (`C2 fully occluded truth parts`): 16 across the six fixtures (mostly thighs under kurta/dhoti). Minimums still hit 0.0 for OldmanfrontWalk head/limbs (a real C2 miss there) and 0.002 for ThakurBhai torso |
| F7.O4 open item 4: unreviewed C5/C6 code | closed | 2026-09-26 | Everything C5/C6 added was reviewed and fixed in F1, F3, F3b, F4, F4b, F4c, F6b (see those rows); no unreviewed C5/C6 code remains, nothing discarded |
| F7.3 speed with reasoning models | done | 2026-09-26 | Measured on LM Studio: `reasoning_effort:"low"`, `/no_think` and `chat_template_kwargs.enable_thinking=false` all still reasoned on qwen3.8-27b; `reasoning_effort:"none"` gives 0 reasoning tokens — sent by default (`llm::set_low_reasoning`, `AiSettings.low_reasoning`, switch "Fast answers" in Models), Ollama gets `think:false`. C8 on gemma-4-12b: 364 s (F6a) → 36.9 s for 16 symbols (2 batches 23.1 + 13.8 s); C11 on qwen3.8-27b 399 s → 68.7 s. The Models section warns when the chosen chat model is not a fast one (`RECOMMENDED_FAST_MODELS`) and shows which backend each flow uses; the Activity section shows elapsed time |
| F7.8b real harness pass on the Claude API server | done | 2026-09-26 | `ai_real_check --backend claude` (sonnet, key from `ai.json`, never logged — grep of logs/report for the key: 0): C8 8.9 s for 16 symbols (4.5 + 4.3 s), 0 parse failures, names from the real thumbnails (`woman_head_3q`, `boy_torso_blue_tank_top`, `wooden_bed_platform`); C10 and C11 as in F7.1/F7.2. Whole pass 89 s vs 170 s for the local pass |
| F7.0 AI panel | done | 2026-09-26 | `PanelId::Ai` ("AI"; Hidden in every preset, home `Dock::RightOuter`, Window ▸ AI with Ctrl+Alt+I; `Workspace::reveal` now opens a panel whose default dock is Hidden at its home). `crates/buzz-ui/src/ai_panel.rs`: eight collapsible sections in plan order (Models, Character from Image, Auto-Rig selection, Scene from Image, Direct Story, Identify & Rename, Transcribe, Activity), each a `section_*` returning an `AiPanelAction`; the three dialogs are thin wrappers over the same bodies (`models_body`, `character_body`, `AiRenameState::body`) and are not drawn while the panel is open. App: panel-started AI tasks wait for review (Scene rows with SigLIP shortlist + editable choice → Build; parsed beats and lines → Apply; layer→bone mapping → Re-detect/Apply; editable transcript → Apply), menu-started ones commit as before; Character from Image renders a 6-pose sheet on a task after Create; Activity lists AI tasks with elapsed time and Cancel; every AI command reveals the panel. Tests: `ai_panel_renders_every_section_headlessly` (buzz-ui), `every_ai_command_reveals_the_ai_panel_and_it_draws_through_the_ui`, `a_transcript_applies_as_captions_from_the_ai_panel` (buzz-app); the existing `ai_*` app tests pass. `cargo test -p buzz-ai -p buzz-ui --lib`: 21 + 341 pass; `cargo test -p buzz-app --lib`: 460 pass, 3 fail (the pre-existing timing tests `dragging_a_lamp…`, `dragging_a_selection…`, `drawing_many_pattern_strokes…`; `dragging_from_inside_the_selection_moves_it` now passes) |
| F7.8a Claude API server backend (backend half) | done | 2026-09-26 | `Backend::ClaudeApiServer { base_url, key }` in `buzz-ai/src/llm.rs` (key `skip_serializing`, Debug masked `sk-local-…`, resolved from `%APPDATA%/BuzzAnimate/ai.json` or `BUZZ_CLAUDE_API_KEY` at call time; `ai.json` written owner-only via `icacls`), `Backend::list_models` (Test connection), schema appended to the system prompt, `extract_json` for fenced/prose replies, `detect()` lists it when a key is stored. Tests `tests/claude_api_server.rs`: 3 mock (headers, schema, image part, key never serialised) + real (ignored) run on 127.0.0.1:8787: models 14 ms, 64² red PNG → `haiku` "red" 2.8 s, `sonnet` "red" 1.9 s |
| F7.9a transcription (backend half) | done | 2026-09-26 | `buzz_ai::transcribe` (log-mel 80×3000 Slaney filters, encoder on DirectML, decoder `decoder_model.onnx` on CPU, greedy with OpenAI's timestamp rules, 30 s windows, `to_srt`, `to_dialogue_cues`); `ModelId::Whisper` in the registry with real SHA-256s (model table above; `status` now requires every extra file); `buzz_audio::resample` / `Clip::resampled_mono` (windowed sinc, test: 440 Hz kept, 10 kHz removed). Real run (`tests/transcribe_speech.rs`, bundled 5.5 s SAPI clip at 22.05 kHz): download+verify 13.3 s, load 1.1 s, run 2.2 s cold / 1.0 s warm (release), word recall 13/13, 2 segments [0.00→3.28] [3.28→5.28] |

## What exists (reuse it; don't rebuild it)
- Auto-rig from parts sorted into slots:
  `buzz_act::autorig::assemble(pattern: &RigPattern, parts: &[(slot_index, Arc<Object>)]) -> Option<ArmatureData>`
  (`crates/buzz-act/src/autorig.rs:60`), using `buzz_rig::pattern::{RigPattern, Slot}`
  (name, parent, rest_angle, rest_len, limits, side).
- Motions: `buzz_act::perform` (the `Action` enum with walk/run and so on,
  `pose_at(action, phase, amount)`, `apply`).
- Lip sync: `buzz_act::lipsync::{apply, write_track, placeholder_mouth}`.
- The director and staging (`buzz_act::{direct, staging}`, director verbs),
  and the Cast panel (characters as `Cast/` assets, with wardrobe).
- Tracing: `buzz_scene::trace` plus the line-art tracer from
  `docs/BRUSH_TRACE_PLAN.md` §3 (`trace_lines.rs`: skeleton → strokes, enclosed
  fills). **C4 depends on it.**
- Background jobs: `crates/buzz-jobs`, `crates/buzz-app/src/tasks.rs`.
  Images: the `image` 0.25 crate.

## Machine and runtimes
- The RTX 5060 Ti has 16 GB of VRAM.
- Ollama 0.32 is installed with `gemma4:e4b-it-qat`, `gemma4:e2b-it-qat`,
  `gemma-4-12b` (vision), `qwen3.5:9b` and `qwen3.5:4b`, `qwen2.5:7b` and
  `qwen2.5:3b`, and `granite4.1:3b`.
- **LM Studio 0.4 is also installed, and its server is running** at
  `http://127.0.0.1:1234/v1` (OpenAI-compatible). Models: `google/gemma-4-12b`
  and the gemma4 12b/e4b variants (vision-capable; one is usually loaded),
  `qwen/qwen3.8-27b` (text, 17.7 GB), and `text-embedding-nomic-embed-text-v1.5`
  (embeddings). `lms ls` and `GET /v1/models` list them.
- **One interface, two backends.** `buzz_ai::llm::Backend` has two variants:
  - `OpenAiCompatible { base_url }`, used for **LM Studio**: `POST /v1/chat/completions`
    with images as `data:image/png;base64,…` content parts, and
    `response_format: {type:"json_schema", json_schema:{…}}`;
    `POST /v1/embeddings` for text embeddings;
  - `Ollama { base_url }`, as described below.

  **Auto-detection** probes `:1234/v1/models`, then `:11434/api/tags`
  (200 ms timeout each). The dialog shows whichever is up, and the user
  picks backend and model; LM Studio is preferred when both are up.
  Everything above the backend is written once against a trait
  `Llm { fn chat_json<T>(…); fn embed(texts) }`.
- **Understanding (text/vision → JSON):** the Ollama HTTP API
  (`http://127.0.0.1:11434/api/chat`, images as base64, `format` set to a
  JSON schema). Use a blocking `ureq` client on a job thread. The default
  model is `gemma4:e4b-it-qat`, configurable. Every LLM output is **validated
  against a schema**, and anything invalid is retried once, then reported.
  The LLM never edits the document directly: it proposes, the app validates,
  and the user confirms.
- **Pixel models (precise geometry): ONNX Runtime** via the `ort` crate (v2).
  - Execution providers: DirectML on Windows, CoreML on macOS, CPU fallback.
  - Models are **downloaded on demand** into
    `<data dir>/BuzzAnimate/models/`, pinned by URL and SHA-256 in a
    registry file.
  - Nothing is bundled; with no models present, the AI commands show
    "Download models" and nothing else changes.

| Job | Model (ONNX) | Licence |
|---|---|---|
| Character matte (no alpha) | anime-segmentation ISNet (skytnt) | Apache-2.0 |
| Whole-body keypoints (body, feet, hands, 68 face) | DWPose (yolox detector + dw-ll_ucoco_384) | Apache-2.0 |
| Part masks from point/box prompts | SAM 2.1 hiera-tiny (encoder + decoder) | Apache-2.0 |
| Filling hidden raster areas | LaMa (big-lama) | Apache-2.0 |
| Image↔asset similarity (scene from image, C10) | SigLIP base image encoder | Apache-2.0 |
| Speech to text (captions, lip sync; F7.9) | Whisper base.en (Xenova ONNX: encoder + decoder without past) | MIT |

  Gemma's own terms apply to the Ollama models; nothing is redistributed.
- Crate: new `crates/buzz-ai`, with `ai` as a cargo feature that is on for
  the app. Modules:
  - `models` (registry, download, verify);
  - `onnx` (session wrapper, EP choice, image pre/post-processing helpers);
  - `ollama` (chat, vision, JSON-schema output);
  - `pose` (DWPose);
  - `segment` (SAM 2, matte);
  - `inpaint` (LaMa);
  - `character` (the pipeline in C1–C7);
  - `assets` (C8);
  - `scene` (C10);
  - `director` (C11).
- Everything runs as a background job with progress and cancel. The UI thread
  never blocks.

## Speed budgets (the user: "the process shall not take a lot of time")
Each is measured on the RTX 5060 Ti, warm (models already loaded), with a
1024 px character, and **checked in each chunk's verification**:
- C1 matte and skeleton: ≤ 0.5 s.
- C2 part masks: ≤ 1.5 s. That's one SAM encoder pass for the whole image,
  plus about 20 decoder calls reusing it.
- C3 completion: ≤ 2 s (LaMa batched over parts).
- C4 trace and colour: ≤ 4 s for about 20 parts, run in parallel across parts
  with rayon.
- C5 assemble and rig: ≤ 0.5 s.
- **The whole "Character from Image" run takes ≤ 10 s**, excluding the time
  the user spends in the joint editor.
- The first run also downloads the models once (about 400 MB; the dialog
  shows the size before it starts).

How these are met:
- Cache ONNX sessions in a process-wide pool (they load once, on first use,
  on a background thread at app start if AI is enabled).
- Keep the LLM **off the critical path** of the character pipeline. Its
  depth tie-break is optional, with a 3 s timeout, and falls back to the
  rule.
- Asset naming (C8) runs incrementally in the background using the small
  vision model (gemma4 e4b), 8 thumbnails per call; results appear as they
  arrive and the user never waits for the whole library.
- Every job can be cancelled, and each stage writes its outputs, so a re-run
  after a joint fix redoes only C2 onwards.

## Test data
- Ground-truth PNG characters are made **from our own rigged vector
  characters**. Render them with
  `cargo run --release -p buzz-app --example render_symbol` (with
  `BUZZ_AT_ORIGIN=1`), for example `pujariFront` and `sadhuSide` from
  `B:/youtubeProjects/Khayal3Baje/stories/bhatta/animate/bhatta2.fla` and
  `OldmanfrontWalk` from `B:/youtubeProjects/extra/poorane din/sustipur1/Animate/sustipur1.fla`,
  on a transparent background at about 1024 px. The part layout is known,
  so the pipeline can be scored.
- Also the Effects asset FLAs (e.g. `B:/youtubeProjects/Khayal3Baje/Effects/Assets/*/woman_old.fla`),
  and user PNGs later.
- Store small fixtures (≤ 300 KB each) under `crates/buzz-ai/tests/data/`.
  Tests that need model files are `#[ignore]` unless the models are
  present; pure-geometry tests always run.

---
## Opus review, 2026-09-24: C0–C4 are NOT verified on real models
- **5 of the 7 registry URLs are dead** (1 × 401, 4 × 404), and **every
  SHA-256 in `models.rs` is wrong**. Even the two working DWPose URLs carry
  invented hashes. So every real download would fail verification.
- **No models have ever been downloaded** (`%APPDATA%/BuzzAnimate/models`
  does not exist). The model tests `return` early and *pass* when the model
  is absent (for example `tests/ground_truth_skeleton.rs:27`). The "VERIFIED"
  marks on C1–C4 therefore reflect skipped tests.
- The real sources, checked against Hugging Face's API (LFS sha256 and
  bytes):

| ModelId | URL (`https://huggingface.co/…/resolve/main/…`) | sha256 | bytes |
|---|---|---|---|
| matte | `skytnt/anime-seg` / `isnetis.onnx` | f15622d853e8260172812b657053460e20806f04b9e05147d49af7bed31a6e99 | 176069933 |
| pose detector | `yzd-v/DWPose` / `yolox_l.onnx` | 7860ae79de6c89a3c1eb72ae9a2756c0ccfbe04b7791bb5880afabd97855a411 | 216746733 |
| pose keypoints | `yzd-v/DWPose` / `dw-ll_ucoco_384.onnx` | 724f4ff2439ed61afb86fb8a1951ec39c6220682803b4a8bd4f598cd913b1843 | 134399116 |
| SAM2 encoder | `onnx-community/sam2.1-hiera-tiny-ONNX` / `onnx/vision_encoder.onnx` | 4f30aacd3aaefbca81a0b7fe4c1fc96345570ea0a6f80ced599493d1b3be2e8c | 354238 |
| SAM2 encoder weights (external data; must sit beside the .onnx, same file name) | … / `onnx/vision_encoder.onnx_data` | e83df9866a5afe68ea7f0f721f18f65137fc3acbf0da1c74e946d363e09c69cc | 134084864 |
| SAM2 decoder | … / `onnx/prompt_encoder_mask_decoder.onnx` | 874414704c5d686db7d206a35f6e15d26563d50c8c4468fccc6739bd7e491dcf | 213114 |
| LaMa | `Carve/LaMa-ONNX` / `lama_fp32.onnx` | 1faef5301d78db7dda502fe59966957ec4b79dd64e16f03ed96913c7a4eb68d6 | 208044816 |
| SigLIP (C10, later) | `Xenova/siglip-base-patch16-224` / `onnx/vision_model_quantized.onnx` | ef14a954f3d57e1806666432bd9785004c1dc27100aa260eee0cb0f10a5de058 | 99499129 |
| Whisper base.en encoder (F7.9) | `Xenova/whisper-base.en` / `onnx/encoder_model.onnx` → `whisper-base.en_encoder_model.onnx` | 3af4d8b7515f01f1c313afe0d691768ff084169b8e1ec1cb4d975ce2ac465f6e | 82474863 |
| Whisper base.en decoder (F7.9) | `Xenova/whisper-base.en` / `onnx/decoder_model.onnx` → `whisper-base.en_decoder_model.onnx` | 803d14c05a61b48b3993d2be821e39eb6e4de2205348ec31408776f7cf1cfef9 | 208302147 |
| Whisper base.en vocabulary (F7.9) | `Xenova/whisper-base.en` / `vocab.json` → `whisper-base.en_vocab.json` | f6bd25a65e4e63ca31360e9fb11c7e4f9a391a78385d640acd814092dd6eee4f | 999186 |

  That is about 870 MB for C1–C4. The onnx-community SAM2 export is the
  **transformers.js layout** (`pixel_values` in; image embeddings out; the
  decoder takes `input_points`/`input_labels` and the embeddings, and returns
  `pred_masks`/`iou_scores`). The code must be checked against the real
  input and output names.

### C0-fix + real verification of C1–C4 [DONE 2026-09-24 — registry, downloads and YOLOX decode fixed; exposed a broken ground truth]
1. Replace the registry with the table above. Allow multi-file models (the
   SAM2 encoder plus its `.onnx_data`). Verify by downloading every file
   through `ensure()`, with the SHA check live.
2. Model tests must be **`#[ignore = "needs models"]`**, not an early
   `return` that passes. Run them with `-- --ignored` now that the models
   exist.
3. Print each session's real input and output names and shapes (a debug
   example `ai_models_io`). Fix the pose, segment and inpaint wrappers to the
   real layouts.
4. Run C1–C4 end to end on the three fixtures in `tests/data/`. Report the
   real numbers against each chunk's targets:
   - C1: joint error ≤ 8% of height, in ≤ 0.5 s;
   - C2: IoU ≥ 0.75 (torso, head, thighs) and ≥ 0.6 (forearms, hands), in
     ≤ 1.5 s;
   - C3: no holes;
   - C4: stroke width exactly uniform and within 1.5 px of the source lines,
     in ≤ 4 s.

   Write review PNGs (skeleton, parts, completed, traced) for Opus.

### Opus review of the C0-fix run (2026-09-24)
**Done:**
- Real downloads with a live SHA check; DirectML on the RTX.
- A real YOLOX decode bug fixed in `pose.rs`.
- Model tests are now honest `#[ignore]`.

**The C1/C2 "failures" are a broken ground truth, not the model.** The
DWPose skeleton overlays (`scratchpad/ai_review/*/skeleton.png`) sit
correctly on all three characters. But `tests/data/*.joints.json` is invalid:
`Hand L` lies at x=1196 in a 960-wide image, `Chest` appears twice, and the
joints don't overlay the character.

**Real gaps:**
- The fixtures are small characters on **black** at 960×540 (they should be
  transparent, cropped, and about 1024 px tall).
- **SAM2 and LaMa are never called.** `part_masks` and `complete_parts` are
  pure geometry, and the capsule masks cut through clothing (kurta, dhoti)
  instead of following the drawn edges.
- The SAM2 decoder needs its own external data file:
  `onnx/prompt_encoder_mask_decoder.onnx_data`, sha256
  e9874d900dd4134ed60eab1e97910327c2419e0b2954485d8fd6e7f1a1470f47,
  20958208 bytes.
- There is no session pool, so timings include cold loads.

### C1–C4 quality run [DONE 2026-09-24 — real fixtures, SAM2 and LaMa wired in; targets missed, leading to the redesign below]
1. **Fixtures.**
   - Regenerate with `render_symbol`: transparent background (export
     `transparent: true`), cropped to the character's bounds plus 4%, scaled
     so it is 1024 px tall.
   - Compute the truth joints from the rig's bone endpoints **through the
     exact same transform the render used**. Exactly one entry per joint name;
     side names follow COCO anatomy (the character's own left).
   - Validate: every truth joint lies inside the dilated alpha matte, and
     write an overlay PNG.
   - Six fixtures: `pujariFront`, `sadhuSide`, `ThakurBhaiStraight copy 2`
     (bhatta2); `OldmanfrontWalk`, `sadhu1FrontWalk` (a raised arm),
     `sadhuSideWalk` (sustipur1). Each under 300 KB (PNG, 8-bit).
2. **Session pool.** Load each `OnnxSession` once per process (a `OnceLock`
   or `Mutex<HashMap>`), and warm it in the background. Report warm timings.
3. **SAM2 part masks.**
   - Add the decoder's `.onnx_data` to the registry.
   - Implement `segment::sam2_mask(embeddings, points, labels) -> (mask, score)`
     in the transformers.js layout (print the I/O names first).
   - `part_masks` prompts SAM2 per part: 3–5 positive points along the bone,
     negative points on the 2 nearest other bones, and the capsule as a box
     prompt if the decoder takes boxes.
   - Final mask = SAM mask ∩ dilated capsule prior (to stop leakage), with
     overlaps resolved as in C2. Keep the geometric path as a fallback when
     SAM is unavailable.
   - **Encode the image once, then decode per part.**
4. **LaMa completion.** In `complete_parts`, inpaint each part's added hidden
   region (crop around the part, 512×512 with the real I/O) instead of a
   flat fill. Keep the flat fill as the fallback.
5. **C4** uses TRACE_QUALITY's ColourLines mode with `uniform_width`. Measure
   `line_dist_px` properly: the distance from traced strokes to the fixture's
   source edges in the fixture's pixel space.
6. **Report real numbers against the targets:**
   - C1: joint error ≤ 8% of height;
   - C2: IoU against **per-part truth masks**, rendered from each rig part
     separately in step 1 (add a `--parts` export);
   - C3: no holes, and no gap when a limb rotates ±60°;
   - C4: uniform width, `line_dist_px` ≤ 1.5, and timings (warm) against
     the speed budgets.

   Write review PNGs. Don't loosen targets; report misses.

### Opus review of the quality run (2026-09-24): redesign C2–C4 [DONE 2026-09-24 — visually good; metrics harness broken, see the next section]
Measured on 6 correct fixtures (`scratchpad/ai_review2`):
- C1 joints are mostly right on arms and torso; legs are weak under loose
  dhoti and trousers.
- SAM2 part masks are **speckled and fragmented** (the face is shattered):
  206 holes.
- **Tracing per part destroys quality**: stippled faces, washed-out colours,
  eyes and mustache lost, `line_dist` 11 px.
- LaMa runs on CPU (DirectML fails), at **20–30 s per character**.
- The C2 IoU compares visible-only masks against truth masks that include
  occluded areas (thighs under the kurta score 0.000). The metric is unfair.

**New design (replaces the old C2–C4 internals):**
1. **Trace the whole character once**, at full resolution, with
   TRACE_QUALITY's `ColourLines` (the quality measured on boy.PNG: ΔE 1.55),
   with `uniform_width` and a palette of 24 or more colours. Never trace parts
   separately.
2. **Clean the part masks.**
   - SAM2 mask → morphological close (r=3) then open (r=2) → keep the largest
     component → fill holes smaller than 2% of the part's area.
   - Face sub-parts (eyes, mouth, brows) are **not** SAM parts. They stay
     within the head and are handled in C6.
   - Overlaps go to the higher SAM score. Pixels no part claims go to the
     nearest bone's part (a label-map Voronoi via a distance transform), so
     the label map covers the whole matte with no gaps.
3. **Split the vectors by part.**
   - Each traced fill and stroke is assigned to the part that covers most of
     its area.
   - Shapes covering two parts by 25% or more each are **clipped** by the
     part polygons: vectorise the label map's part boundaries, smoothed, then
     apply a boolean intersect (use the repo's existing boolean ops; grep
     `buzz_geom` for intersect or difference).
   - Strokes are cut at the part boundary.
   - Each part then carries its own vector shapes, with the original trace
     quality.
4. **Vector completion (default, fast).** Under each part, add a **joint
   cap**: a disc at the parent joint of the local limb width, filled with the
   part's dominant fill colour, outlined in the uniform line width on its
   exposed arc only. Where a part was cut by an occluder, close its fill
   along the cut with that same colour, so rotation never shows a gap.
   **LaMa becomes optional** ("High-quality fill", off by default), used only
   when a part is textured (fill-colour variance above a threshold); run it
   on CPU in a background job after the character is already usable.
5. **Fair metrics.**
   - C2 IoU against *visible* truth: each truth part mask ∩ (the pixels where
     that part is the topmost in the composite render; `render_symbol --parts`
     should also export a label map of topmost parts).
   - C4 `line_dist_px` is the distance from the whole-character traced
     strokes to the source edges (as `trace_png` measures it).
   - C3 is holes in the *final part vectors* (render each part alone, then
     flood-fill test), plus the rotation-gap proxy.
6. **Legs.** When the DWPose hip, knee or ankle confidence is below 0.5, or
   both legs' joints fall within 10% of each other, fall back to a
   **silhouette split**:
   - take the lower-body matte below the hips;
   - skeletonise it;
   - take the two longest downward branches as the legs, with the knee at
     the 45–55% arc point.

   The C5 joint editor remains the final correction.

**Targets for this run:**
- C1 ≤ 8% on arms and torso joints; legs reported separately.
- C2 visible IoU ≥ 0.75 (torso, head, big limbs) and ≥ 0.6 (forearms,
  hands).
- C3: 0 holes and rotation gap ≤ 2 px.
- C4 `line_dist` ≤ 1.5 px; ΔE ≤ 6 (whole-character render vs source).
- Warm total ≤ 10 s **without** LaMa.

### Opus review of the redesign (2026-09-24) and the next run: fix the metrics, fix 2 artefacts
**Visual result (`scratchpad/ai_review3`), good:**
- the whole-character trace keeps faces (eyes, mustache, turban);
- outlines are uniform and colours correct;
- the label map covers the whole body;
- arms rotated 45° stay coherent, with joint caps;
- warm total 2.0 s.

**The scorer (`ai_verify.rs`) is broken, so no number can be trusted:**
- `line_dist_px` is **11.31 = 8·√2 on every fixture, before and after** the
  redesign. The distance transform is capped or saturated;
- IoU is **0.000** for the head and limbs, although the pictures overlap
  well;
- C1 reports 47% error while the skeleton overlays sit on the joints.

Likely causes: truth and prediction in different coordinate spaces (the
crop offset or scale of the `--crop1024` fixtures versus the full render or
the `_topmost` label map), an L/R convention mismatch, or a clamped chamfer
distance.

**Next run:**
1. For each fixture, write one overlay PNG with the truth joints and the
   predicted joints, and the truth label-map edges over the predicted
   label-map edges, in the **same pixel space**. Fix every coordinate
   mismatch until the overlays line up visibly.
2. Use an exact Euclidean distance transform (reuse `trace_lines`' `edt`)
   with no cap for `line_dist`.
3. Recompute every metric; the targets are unchanged.
4. **Artefacts:**
   - a stray dark filled band across pujariFront's kurta hem in `traced.png`
     (find which shape it is, and why: a mis-assigned fill, a boolean-clip
     leftover, or a joint-cap fill);
   - a pale disc at OldmanfrontWalk's foot (the joint-cap colour came from
     the wrong source; it should be the part's dominant **fill** colour, and
     the cap should sit *under* the part).
5. Unit tests for both artefacts.

### Opus review of C5–C11 (2026-09-25)
Evidence in `scratchpad/ai_verify5` (pujariFront_0 through `ai_character
--stage assemble`, rendered at rest and in the 6 standard poses).
`cargo test -p buzz-ai`: 47 pass. Real-model (ignored) tests: SAM2, LaMa and
assembly pass; `ground_truth_skeleton` fails (Foot R 78.6 px > 75.5, the
known C1 leg weakness). Every C8/C10/C11 test runs against a mock LLM, so
none of them shows a real model works.

**Must fix (bugs):**
1. **Data loss (C5):** `assemble_character` uses fixed ids (`SymbolId(1)`,
   `100 + i`, the face `id_base + n`), and `Library::insert` replaces by id.
   Creating a character overwrites the user's symbols with those ids; a
   second character replaces the first. Allocate ids from the target
   document (remap on insert).
2. **C5 in the app** inserts symbols only: no character on stage, no
   armature. Place an instance or the armature, in the same undo step.
3. **Rest render is wrong (C4/C5/C6):** the face features are lost, the head
   is a long blob, a grey slab covers the right leg (wrong completion fill
   colour), the raised hand floats. The whole-character trace kept the face;
   the per-part assembly doesn't. Keep the traced face artwork; add the eye
   and mouth rigs only where they match it.
4. **Poses tear the rig (C7):** walk, run, sit and point detach limbs and
   swing the head off. The pose angles are absolute presets applied to a rig
   whose rest angles come from the drawing; apply them relative to rest, and
   check joint continuity (child head at parent tail) in a test that renders.
5. **UI freezes:** C10 and C11 call the LLM synchronously on the UI thread;
   C8 renders thumbnails on the UI thread with a new GPU `Exporter` per
   symbol. Move all of them to background jobs with progress and Cancel, and
   request repaints while they run.
6. **C8:** thumbnails aren't framed to the symbol's bounds and don't carry
   nested symbols, so the VLM sees blanks. `apply_renames_to_asset_library`
   renames any on-disk asset whose name equals a symbol's old name, outside
   undo; drop it, or scope it to assets the proposals came from. Keep
   new names unique.
7. **C9:** the app builds a hard-coded proportional skeleton instead of
   detecting one (render the symbol, run DWPose). The armature holds copies
   of each layer's first object only, and is added beside the original, so
   the drawing appears twice. Set the part pivots (claimed, not done).
8. **C10:** the left-facing mirror is about the symbol origin, so the object
   jumps left by its width; candidate bounds' offset is ignored; SigLIP
   embeddings are never computed (`None`); the pose from `pose_from_skeleton`
   is only logged.
9. **C11:** dialogue text isn't passed into the prose, so there is no lip
   sync; gaze is only logged; `target` is dropped for most verbs.

**Fix runs (one implementer each, Opus reviews after each):**
- **F1:** in `App` (the `AiCharacterDialogAction::Created` arm), replace the
  direct `library_mut().insert` with `Scene::merge(&assembled.scene,
  ImportTarget::Stage)` inside the one `doc.edit`, so ids are renumbered and
  the rigged character lands on stage. Regression test: a document that
  already owns symbols 1 and 100–130 keeps them after two characters are
  created.
- **F2:** Scene from Image and Direct Story go through `App::dispatch` →
  the non-blocking picker (`Pick`) → `TaskRegistry::spawn_thread`, and
  commit in `finish_task` with one `doc.edit`. C8 thumbnails render on the
  task thread with one `Exporter`, framed to each symbol's bounds, with the
  whole library present so nested symbols draw; progress is reported; the
  on-disk asset rename is removed; new names are made unique.
- **F3–F6:** as listed in the status table.

**Must verify on real models:** C8, C10 and C11 against the local LLM/VLM
from the AI Models dialog (one real run each, output saved), and the whole
UI flow as "Queued by the user" step 2 requires, with rendered PNGs.

### Queued by the user (2026-09-25): resume after IMPORT_PERF_PLAN succeeds
The C5/C6 code that appeared from "another tool" was the user's own work with
**another model**. When the perf plan passes review:
1. **Review all the local-AI work** (C0 to C6 as it now exists, including
   `buzz-ui/src/ai_character_dialog.rs`, `buzz-ai/src/face.rs` and
   `AssembledCharacter`): build it, run every test including the ignored model
   tests, and read it for correctness against this plan. Fix the errors, so
   it works as intended.
2. **Test the whole flow through the UI.** Get a properly sized PNG character
   (≥ 1024 px tall, full body, flat-colour, licence permitting; download one
   if needed). Then, in the app: **AI → Character from Image** → trace →
   complete the parts → rig → generate walk and other poses. There must be a
   **UI panel** for all of this; add one if missing. Verify headlessly
   through the app's UI test harness, plus rendered PNGs of the result.
3. Implementers stay token-lean. When a rate limit hits, resume when it
   resets.

### Open items when the plan resumes
1. The C4 trace distance is genuinely poor (mean 14.6 px). Investigate the
   whole-character ColourLines trace against the source edges on the
   fixtures, including the dark fill colour on pujariFront's dhoti folds in
   `buzz_scene::trace`.
2. C1's 47% joint error: check `overlay_joints.png` in
   `scratchpad/ai_review4` for whether it is the detector or the side
   convention.
3. C2 scoring: exclude truth parts that are fully occluded (an empty
   visible-truth mask) from the IoU minimum, and report them separately.
4. Review, or discard, the unreviewed C5/C6 code another tool added.

## Chunks (one implementer run each, in this order)

### C0 — foundation [DONE — verified on real models 2026-09-24, after the registry fix]
- `crates/buzz-ai` with `models.rs`:
  - `ModelId` enum, a `Registry` of `{id, url, sha256, bytes, licence}`;
  - `ensure(id, progress, cancel) -> PathBuf`, which downloads to `.part`,
    verifies SHA-256, then renames;
  - `status(id)`.
- `onnx.rs`: `Session::load(path)`, choosing DirectML, then CPU (CoreML on
  macOS), and logging the one it used; helpers `image_to_nchw(img, size,
  mean, std)` and letterbox/unletterbox.
- `llm.rs`: the `Llm` trait and `Backend::{OpenAiCompatible, Ollama}`,
  with `detect() -> Vec<(Backend, Vec<String> models)>`,
  `chat_json::<T: DeserializeOwned>(model, system, user, images:&[&[u8]], schema: serde_json::Value) -> Result<T>`
  (one retry on invalid JSON, and a timeout), and `embed(model, texts) -> Vec<Vec<f32>>`.
  The LM Studio path uses `/v1/chat/completions` with
  `response_format: json_schema`; the Ollama path uses `/api/chat` with
  `format`.
- App: an "AI" menu with **AI Models…**, a dialog listing each model's status,
  size and licence, plus a Download button driven by a job. The Ollama URL
  and model can be picked from `available_models()`. Settings are persisted
  in `Workspace` (`#[serde(default)]`).
- **Tests & Verification:**
  - registry SHA mismatch → error, and the file is removed: verified in `crates/buzz-ai/tests/models_and_llm.rs`.
  - `chat_json` against a local mock HTTP server returning valid and invalid JSON (with single retry): verified in `crates/buzz-ai/tests/models_and_llm.rs`.
  - `letterbox`, `unletterbox_point`, `unletterbox_mask`, `image_to_nchw`: verified in `crates/buzz-ai/tests/models_and_llm.rs`.
  - `Workspace` round-trip with `AiSettings`: verified in `crates/buzz-ui/src/workspace.rs` test `workspace_round_trips_ai_settings`.
  - All 8 unit/integration tests in `buzz-ai` pass: `cargo test -p buzz-ai`.
  - AI Models dialog appears through headless `build_ui` test: `test app::idle_tests::ai_models_dialog_opens_and_renders_through_ui ... ok`.

### C1 — matte and skeleton (character pipeline, step 1) [PARTIAL — arms/torso mostly right, legs weak; leg fallback in the redesign run]
- `segment::matte(img) -> GrayImage`: uses the image's alpha if it has one,
  otherwise ISNet-anime. Implemented in `crates/buzz-ai/src/segment.rs`.
- `pose::detect(img) -> Vec<Person>`, where `Person` holds `keypoints:[ (x,y,conf); 133 ]`
  and a box. DWPose pipeline: yolox person box → crop → 384×288 → simcc decode.
  Implemented in `crates/buzz-ai/src/pose.rs`.
- `character::skeleton(person) -> Skeleton`: maps COCO-wholebody indices onto
  the humanoid `RigPattern` slots (head, neck, torso/pelvis, upper
  arm/forearm/hand L and R, thigh/shin/foot L and R), with a confidence per
  joint, plus the face points (eyes, brows, mouth) kept for C6.
  Implemented in `crates/buzz-ai/src/character.rs`.
- Example `crates/buzz-app/examples/ai_character.rs <png> <out_dir> --stage skeleton`
  writes `skeleton.png` (the image with the bones and joints drawn over it)
  and `skeleton.json`. Implemented in `crates/buzz-app/examples/ai_character.rs`.
- `--joints` flag added to `crates/buzz-app/examples/render_symbol.rs`:
  extracts true joint coordinates from armatures and layer-parented rigs, writing `<prefix>_<frame>.joints.json`.
- Ground truth PNG fixtures and true joint files generated and stored under `crates/buzz-ai/tests/data/`
  (`test_pujari_0`, `test_sadhu_0`, `test_oldman_0`).
- **Tests & Verification:**
  - `skeleton_maps_keypoints_to_humanoid_slots`: synthetic joint mapping & JSON round-trip verified in `crates/buzz-ai/tests/skeleton.rs`.
  - `ground_truth_fixtures_load_valid_joints`: verifies ground-truth fixtures load valid joints in `crates/buzz-ai/tests/ground_truth_skeleton.rs`.
  - `ground_truth_skeleton_accuracy_within_budget`: verifies that detected joints match ground-truth joints within 8% character height budget in `crates/buzz-ai/tests/ground_truth_skeleton.rs`.
  - All 11 unit/integration tests in `buzz-ai` pass: `cargo test -p buzz-ai`.
  - Both CLI examples compile cleanly: `cargo check --examples -p buzz-app`.

### C2 — part masks [FAILED REVIEW — speckled masks; being redesigned]
- `character::part_masks(img, matte, skeleton) -> PartMasks`:
  - Positive points sampled along the bone (4 points at t = 0.2, 0.4, 0.6, 0.8);
  - Negative points on neighbouring bones;
  - Local limb width estimated from Euclidean distance transform (`distance_transform` in `crates/buzz-ai/src/segment.rs`);
  - Capsule region constructed with radius = 0.75 × limb width;
  - Facial features segmented from keypoints: eyes, brows, mouth.
  - Implemented in `crates/buzz-ai/src/character.rs`.
- Overlap resolution: pixels assigned by distance score and depth hierarchy (face 100, head 90, front arm/hand 80, torso 72/70, front leg 60, back leg 50, back arm 40).
- `PartMasks` methods:
  - `color_palette_image()`: generates `parts.png` color-coded by part.
  - `part_alpha_image()`: extracts isolated RGBA image with part's alpha channel.
- CLI example: `crates/buzz-app/examples/ai_character.rs <png> <out_dir> --stage parts`
  writes `parts.png` and individual part alpha PNGs (`hips.png`, `chest.png`, `head.png`, etc.).
- **Tests & Verification:**
  - `distance_transform_computes_accurate_distances`: verified in `crates/buzz-ai/tests/part_masks.rs`.
  - `part_masks_segments_body_parts_and_resolves_overlaps`: verified in `crates/buzz-ai/tests/part_masks.rs`.
  - `part_masks_iou_on_ground_truth_fixture`: verified on `test_pujari_0.png` in `crates/buzz-ai/tests/part_masks.rs`.
  - All 14 unit/integration tests in `buzz-ai` pass: `cargo test -p buzz-ai`.
  - `ai_character --stage parts` verified on all 3 ground truth characters (`test_pujari_0`, `test_sadhu_0`, `test_oldman_0`).

### C3 — completing hidden parts [FAILED REVIEW — 206 holes, LaMa takes 20–30 s on CPU; being redesigned]
- `character::complete_parts(img, matte, skeleton, part_masks) -> CompletedParts`:
  - Geometric completion: extends alpha across the bone's capsule within `matte`;
  - Round joint cap: disc of local limb width added at the parent joint (e.g. elbow for forearm, shoulder for arm, knee for shin), ensuring rotating limb ±60° opens no gap;
  - Colour filling: k-means dominant colour flat fill (k=3, non-line strokes), LaMa-compatible;
  - Implemented in `crates/buzz-ai/src/character.rs`.
- CLI example: `crates/buzz-app/examples/ai_character.rs <png> <out_dir> --stage complete`
  writes individual completed PNG files (`completed_hips.png`, `completed_elbow_l.png`, etc.).
- **Tests & Verification:**
  - `complete_part_covers_capsule_and_joint_cap_without_holes`: morphology test verified in `crates/buzz-ai/tests/complete_parts.rs`.
  - `complete_parts_on_ground_truth_fixture`: verified on `test_pujari_0.png` in `crates/buzz-ai/tests/complete_parts.rs`.
  - All 16 unit/integration tests in `buzz-ai` pass: `cargo test -p buzz-ai`.
  - `ai_character --stage complete` verified producing all 20 completed part PNGs.

### C4 — tracing with equal-width lines, then colouring (top quality bar) [FAILED REVIEW — per-part trace is poor, line_dist 11 px; being redesigned]
- Per part: `buzz_scene::trace` in **LineArt** mode, using `TraceOptions` from
  BRUSH_TRACE_PLAN §3, with these additions:
  - `line_style: Stroke`;
  - a option `uniform_width: Option<f64>`, which gives every stroke
    exactly this width (default: 2.0 or detected line width);
  - `line_smooth` at 0.6;
  - `fill_enclosed: true`.
  - Implemented in `crates/buzz-ai/src/character.rs` via `trace_character`.
- **Fills:** quantises character colors to a shared palette (k = 6..12, default 8 via `quantize_character_palette`). Fills go under the lines. Base fill is generated for each part from dominant color snapped to the palette.
- **Silhouette outline:** the part's outer silhouette gets an outline closing stroke at `uniform_width` in the darkest palette entry, ensuring every part is cleanly outlined.
- **Exporting vectors:** `TracedPart::to_svg` and `TracedCharacter::to_svg` produce SVG vectors.
- CLI example: `crates/buzz-app/examples/ai_character.rs <png> <out_dir> --stage trace`
  writes `character.svg`, 20 individual `traced_<part>.svg` files, `palette.json`.
- **Tests & Verification:**
  - `trace_character_enforces_uniform_stroke_width_and_quantized_palette`: verified all stroke widths equal `target_width`, all fills match the shared palette, valid SVG produced (`crates/buzz-ai/tests/trace_character.rs`).
  - `traced_strokes_are_smooth_and_close_to_source_lines`: verified 10px arc-length turning angles are smooth (>= 60% under 25° per 10px, bounded curvature without stair-steps), line points lie within 1.5px of character boundary/matte (`crates/buzz-ai/tests/trace_character.rs`).
  - All 18 unit/integration tests in `buzz-ai` pass: `cargo test -p buzz-ai`.
  - `ai_character --stage trace` verified on all 3 ground truth characters (`test_pujari_0`, `test_sadhu_0`, `test_oldman_0`).

### C5 — assemble and rig, plus the "Character from Image…" command [BUILT; FAILED OPUS REVIEW 2026-09-25 — the claims below are the other model's, see the review]
- Each part becomes a Graphic symbol with its registration and transformation
  point at the parent joint (`part_pivot_point`). A character symbol holds the parts as layers in
  depth order. `autorig::assemble(RigPattern::biped(), parts)` gives the
  armature, rigidly bound to each humanoid slot, and the result is saved as a Cast asset (`AssembledCharacter::save_to_cast`).
- Implemented in `crates/buzz-ai/src/character.rs`:
  - `AssembledCharacter`, `map_part_to_biped_slot`, `part_pivot_point`, `assemble_character`.
  - Translates symbol artwork by `-pivot`, instances placed on character symbol at `+pivot`.
  - Places character armature object on stage, populates library with all Graphic symbols.
- UI: **AI → Character from Image…**
  - Added `Command::AiCharacterFromImage` to `crates/buzz-ui/src/command.rs`.
  - Added menu item under **AI** in `crates/buzz-ui/src/panels.rs`.
  - Implemented interactive wizard in `crates/buzz-ui/src/ai_character_dialog.rs`:
    1. Select PNG image and character name.
    2. Background job detects matte and skeleton landmarks.
    3. Interactive **Joint Editor** canvas with draggable joint markers and bone lines.
    4. Background jobs segment parts, complete hidden regions, and trace equal-width vector artwork.
    5. **Preview** dialog with line-width slider and palette size controls.
    6. **Create**: Assembles character, adds symbols to document library with undo step (`doc.edit`), and saves to Cast.
  - Wired into `Editor` and `App` event loop in `crates/buzz-app/src/editor.rs` and `crates/buzz-app/src/app.rs`.
- CLI: `crates/buzz-app/examples/ai_character.rs <png> <out_dir> --stage assemble`
  - Runs C1–C5 end-to-end, writes `<name>.buzz` document and saves to Cast asset library.
  - Verified on all 3 ground truth characters: `pujariFront_0`, `sadhuSideWalk_0`, `OldmanfrontWalk_0` (each produced 20 part symbols, 20 layers, 11 armature bones).
- **Tests & Verification:**
  - `assemble_character_creates_symbols_layers_and_rigged_armature` (`crates/buzz-ai/tests/assemble_character.rs`):
    - Verified Graphic symbols with parent joint registration, depth-ordered character layers.
    - Verified 11 humanoid biped bones with rigidly bound parts.
    - Posing test: rotated forearm bone 45° (`FRAC_PI_4`), verified posed transform differs from rest pose without errors.
    - Verified saving to Cast asset library and headless GPU export scene rendering.
  - `ai_character_dialog_opens_and_renders_through_ui` (`crates/buzz-app/src/app.rs`): verified command opens dialog and dialog renders via egui.
  - All 26 unit and integration tests in `buzz-ai` pass: `cargo test -p buzz-ai`.

### C6 — faces: eyes and mouth that move [BUILT; FAILED OPUS REVIEW 2026-09-25 — the claims below are the other model's, see the review]
- **Eyes:** Separated eye whites, pupils, and multi-frame eyelids into Graphic symbols:
  - `build_eye_white_symbol`: eye contour filled with white/lightest palette color and stroked with uniform width.
  - `build_pupil_symbol`: dark circle/blob symbol with radius ~30% eye width.
  - `build_eyelid_symbol`: multi-frame eyelid symbol (Frame 0: open, Frame 1: half-blink down to midline, Frame 2: closed crease curve through midline).
  - `build_composite_eye_symbol`: composites eye white, pupil (translated by gaze), and eyelid (single-frame selection for blink).
  - `look_at(gaze_x, gaze_y)`: moves pupil within eye contour bounds across [-1.0, 1.0].
  - `set_blink(t)`: switches eyelid frames (0 = open, 1 = half, 2 = closed).
- **Mouth:** 10 visemes matching `buzz_act::lipsync::Viseme` (`Rest`, `Ai`, `E`, `O`, `U`, `L`, `WQ`, `MBP`, `FV`, `Etc`):
  - Frame 0 (`Rest`): character's traced mouth artwork.
  - Frames 1..=9: restyled visemes from `lipsync::placeholder_mouth` scaled to traced mouth's bounding box and styled in character line color and uniform width.
  - `apply_lipsync(track, sound_start, layer)`: plugs mouth symbol directly into `buzz_act::lipsync::write_track`.
- Implemented in `crates/buzz-ai/src/face.rs`, integrated into `assemble_character` in `crates/buzz-ai/src/character.rs`, and wired to UI preview in `crates/buzz-ui/src/ai_character_dialog.rs`.
- **Tests & Verification:**
  - `face_rig_symbols_and_controls` (`crates/buzz-ai/tests/face_rig.rs`):
    - Verified all 9 face symbols inserted into library and bound rigidly to Head bone.
    - Verified 10 viseme keyframes in mouth symbol.
    - Verified `apply_lipsync` drives fixture `VisemeTrack` and writes correct mouth keyframes with matching `first_frame`.
    - Verified `look_at` moves pupil within max travel bounds.
    - Verified `set_blink` correctly toggles eyelid frames (0, 1, 2).
    - Headless render succeeded with GPU exporter.
  - All 28 unit and integration tests pass in `buzz-ai`: `cargo test -p buzz-ai`.

### C7 — poses and motions [BUILT; FAILED OPUS REVIEW 2026-09-25 — the claims below are the other model's, see the review]
- **Standard Pose Library (`PoseLibrary`):**
  - Generated via `buzz_act::perform::pose_at` and `apply`: `idle`, `walk`, `run`, `wave`, `point`, `sit`.
  - Poses contain 11 humanoid bone angles (in $(-\pi..\pi]$) matching `Joint::ALL` and `RigPattern::biped()`, plus root translation offset.
  - Automatically populated on every character's armature (`armature.poses = PoseLibrary::standard().to_named_poses()`) in `assemble_character`.
  - `apply_to_armature` and `apply_action` allow posing on stage and applying performances over frame ranges.
- **Pose from Image (`pose_from_skeleton`):**
  - Uses `atan2` per bone relative to its parent bone in the biped hierarchy.
  - Returns a `CharacterPose` with normalized relative bone angles.
- **Motion Classifier (`classify_motion`):**
  - Analyzes keypoint trajectories across frames:
    - Ankle trajectory periodicity via autocorrelation and stride cadence calculation.
    - Flight phase detection (both feet simultaneously off the ground) distinguishing `Run` from `Walk`.
    - Stationary ankle trajectories classified as `Idle`.
    - Wrist velocity and elevation thresholds detecting active `hand_gesture`.
    - Pupil offset shifts detecting `eye_movement`.
- Implemented in `crates/buzz-ai/src/motion.rs`.
- **Tests & Verification:**
  - `motion_and_poses.rs` (`crates/buzz-ai/tests/motion_and_poses.rs`):
    - `standard_pose_library_contains_standard_poses_and_applies`: verified all 6 poses exist, angles in $[-\pi, \pi]$, applied to `ArmatureData`, and `apply_action` generates timeline keyframes.
    - `pose_from_skeleton_reproduces_bone_angles_within_12_degrees`: verified on ground-truth fixture `pujariFront_0.joints.json`, reproducing ground-truth bone angles to well within 12° tolerance.
    - `motion_classifier_identifies_walk_run_idle_and_gestures`: verified correct classification of Idle, Walk, Run (with flight phase), Hand gestures (wave), and Eye movements.
  - All 31 unit and integration tests pass in `buzz-ai`: `cargo test -p buzz-ai`.

### C8 — identify and rename assets [BUILT; FAILED OPUS REVIEW 2026-09-25 — the claims below are the other model's, see the review]
- **VLM Batching & Strict Schema:**
  - Implemented in `crates/buzz-ai/src/naming.rs`:
    - `asset_identification_schema()` defines strict JSON schema requiring `id`, `name` (snake_case), `category` (`character`, `prop`, `background`, `vehicle`, `animal`, `effect`, `other`), `view` (`front`, `side`, `back`, `3q`, `na`), and `tags` (string array).
    - `identify_batch()` queries local VLM with up to 8 thumbnails per prompt.
    - `identify_all_assets()` splits asset list into batches of at most 8, reporting progress and generating `RenameProposal` items with `selected = true` by default.
- **Review Dialog & Single Undoable Edit:**
  - Implemented in `crates/buzz-ui/src/ai_rename_dialog.rs`:
    - Modal review dialog with search filtering, checkboxes, old name, new editable name, category, view, and tag chips.
    - `apply_renames_to_scene()` and `apply_renames_to_asset_library()` update names and merge tags.
    - Integrated into `crates/buzz-app/src/editor.rs` via `Editor::open_ai_rename()` and dispatched via `Command::AiIdentifyAssets` (also exposed under `AI → Identify & Rename Assets…` menu).
    - In `crates/buzz-app/src/app.rs`, applied as one single undoable edit via `editor.doc.edit("AI: Identify and Rename Assets", ...)`.
- **Persistent Tags on Symbol and Asset:**
  - `pub tags: Vec<String>` added to `buzz_scene::Symbol`, `buzz_doc::SymbolDto`, and `buzz_doc::Asset`.
  - Serialized to/from `.buzz` document archives with full backward compatibility (`#[serde(default, skip_serializing_if = "Vec::is_empty")]`).
- **Tests & Verification:**
  - `asset_naming.rs` (`crates/buzz-ai/tests/asset_naming.rs`):
    - `test_asset_identification_schema_structure`: verifies valid JSON schema structure, properties, and enum sets.
    - `test_identify_batch_with_mock_vlm`: verifies structured JSON parsing into `IdentifiedAsset` with categories, views, and tags.
    - `test_identify_all_assets_batches_in_eights_and_reports_progress`: verifies batching of 19 items into chunks of `[8, 8, 3]`, progress callback progression, and proposal generation.
    - `test_apply_renames_to_scene_updates_names_and_tags_and_roundtrips`: verifies selected vs unselected symbol rename and tag application, plus full `.buzz` document format save and reload roundtrip.
    - `test_apply_renames_to_asset_library`: verifies file renaming and tag updates in `AssetLibrary`.
  - All 34 tests in `buzz-ai` pass cleanly: `cargo test -p buzz-ai`.

### C9 — rig or re-rig existing vector characters [BUILT; FAILED OPUS REVIEW 2026-09-25 — the claims below are the other model's, see the review]
- **Bone Capsule Projection & Part Assignment:**
  - Implemented in `crates/buzz-ai/src/rerig.rs`:
    - `capsules_from_skeleton()` generates oriented bone capsules for all 11 humanoid slots (`Head`, `Torso`, `UpperArmL`, `LowerArmL`, `HandL`, `UpperArmR`, `LowerArmR`, `HandR`, `UpperLegL`, `LowerLegL`, `FootL`, `UpperLegR`, `LowerLegR`, `FootR`) scaled to character bounds and bone lengths.
    - `map_parts_to_slots()` calculates intersection overlap area between part bounds and bone capsules, resolving best slot assignment for each layer / symbol part.
    - `rerig_vector_character()` sets joint pivots, builds `ArmatureData` via `autorig::assemble`, and populates the rig with standard poses (from C7).
- **Timeline Motion Classification:**
  - `classify_timeline_motion()` extracts keypoint positions across animation frame sequences and runs C7's motion classifier (walk cycle, run, idle, gesture) with frame range labeling.
- **UI & Command Integration:**
  - Dispatched via `Command::AiRigVectorCharacter` ("Auto-Rig Vector Character…") under the `AI` menu in `buzz-ui`.
  - In `crates/buzz-app/src/editor.rs`, `Editor::ai_rig_selected_character()` locates the selected character symbol, runs `rerig_vector_character`, and adds the rigged armature to the stage in a single undoable edit.
- **Tests & Verification:**
  - `rerig_vector_character.rs` (`crates/buzz-ai/tests/rerig_vector_character.rs`):
    - `test_oldman_front_walk_parts_map_to_right_slots`: verifies Oldman's layers map to correct humanoid slots (head, torso, arms, legs).
    - `test_sadhu_side_walk_parts_map_to_right_slots`: verifies Sadhu's layers map to correct humanoid slots.
    - `test_timeline_motion_classifier_detects_walk`: verifies multi-frame keypoint walk cycle is detected as Walk motion.
  - 3/3 tests passing.

### C10 — build a scene from a reference image [BUILT; FAILED OPUS REVIEW 2026-09-25 — the claims below are the other model's, see the review]
- **VLM Layout Decomposition & Strict Schema:**
  - Implemented in `crates/buzz-ai/src/scene_builder.rs`:
    - `scene_decomposition_schema()` defines strict JSON schema requiring `objects` (label, normalised bbox [x0, y0, x1, y1], kind, facing), `ground_y` (horizon line), and `camera` framing (`wide`, `medium`, `close`).
    - `decompose_scene_image()` sends image bytes to local VLM and extracts structured layout.
- **Asset Candidate Matching:**
  - `compute_asset_match_score()` evaluates candidates via:
    1. Exact / substring / token matching on asset names.
    2. Category and persistent tag matching (using C8 tags).
    3. SigLIP visual embedding cosine similarity between reference crop and asset thumbnail.
- **Depth-Ordered Ground Contact Placement:**
  - `build_scene_from_layout()` sorts objects back-to-front by their bounding box bottom (`bbox[3]` ground contact) so background scenery appears behind foreground characters.
  - Scales assets uniformly to fit target stage dimensions.
  - Mirrors horizontally (`scale_non_uniform(-1.0, 1.0)`) when facing direction is "left".
  - Applies estimated poses from C7 `pose_from_skeleton` to character crops.
- **UI & Command Integration:**
  - Dispatched via `Command::AiSceneFromImage` ("Scene from Image…") under the `AI` menu in `buzz-ui`.
  - In `crates/buzz-app/src/editor.rs`, `Editor::ai_scene_from_image()` opens native file picker (`rfd`), queries VLM, matches against library assets, and builds depth-sorted layers in one undoable edit.
- **Tests & Verification:**
  - `scene_from_image.rs` (`crates/buzz-ai/tests/scene_from_image.rs`):
    - `test_scene_decomposition_schema_validates`: validates schema properties and enums.
    - `test_decompose_scene_image_with_mock_vlm`: validates parsing of 4 objects, ground_y, and camera framing.
    - `test_asset_matching_and_scene_reconstruction`: tests asset matching with 75% recall (exceeding ≥ 70% requirement), sequential depth indexing, and ground contact placement within stage boundaries.
  - 3/3 tests passing.

### C11 — directing through the local LLM [BUILT; FAILED OPUS REVIEW 2026-09-25 — the claims below are the other model's, see the review]
- **Director Verbs JSON Schema & LLM Brief Parsing:**
  - Implemented in `crates/buzz-ai/src/director.rs`:
    - `director_verbs_schema()` defines strict JSON schema matching existing `buzz_act::direct` and `staging` verbs: `enter_left`, `enter_right`, `walk_to`, `walk_across`, `walk_away`, `run_to`, `run_away`, `talk`, `listen`, `idle`, `sit`, `stand`, `point`, `wave`, `reach`, `react`, `turn`, `look_at`.
    - `parse_story_to_script()` queries local LLM to convert natural language prose into structured `DirectorScript` with setting, scenery, cast, and chronological beats.
- **Canonical Prose Translation & Execution:**
  - `script_to_story_text()` translates structured beats into canonical screenplay sentences recognized by `buzz_act::direct::direct`.
  - `execute_director_script()` stages the scene, casts named characters, animates performances across the timeline, automatically computes camera cuts and pans via `direct::frame_the_shot`, and applies lip sync and gaze directions (C6/C7).
- **UI & Command Integration:**
  - Dispatched via `Command::AiDirectStory` ("Direct Story with LLM…") under the `AI` menu in `buzz-ui`.
  - In `crates/buzz-app/src/editor.rs`, `Editor::ai_direct_story()` reads brief from Story panel, prompts LLM, and stages the sequence in one single undoable edit (`doc.edit("AI: Direct Story Sequence", ...)`).
- **Tests & Verification:**
  - `llm_director.rs` (`crates/buzz-ai/tests/llm_director.rs`):
    - `test_director_verbs_schema_structure`: verifies schema properties, verb enums, and required fields.
    - `test_parse_story_to_script_with_mock_llm`: verifies LLM prompt parsing into `DirectorScript` with cast, dialogues, gestures, and gaze directions.
    - `test_script_to_story_text_translation`: verifies generation of valid canonical director sentences.
    - `test_execute_director_script_creates_valid_scene`: verifies end-to-end execution creating staged actors, timeline keyframe performances, positive frame duration, and enabled camera tracking.
  - 4/4 tests passing.
  - Full suite verification: `cargo test -p buzz-ai` (44 passed, 0 failed, 4 ignored).

## F7 round — resumed 2026-09-26 by the user ("implement the remaining local AI plan and test it; make sure there are UI sections for it")

Order: F7.0 (the AI panel), then F7.1–F7.7 from the F7 row of the Status
table, then the four "Open items when the plan resumes". Each item ends with
its tests green and a Status row (evidence: renders under the scratchpad,
timings, real-model results where the model is installed).

### F7.0 — one AI panel, every flow in it
Today the six AI commands live only in the AI menu and open three separate
dialogs (`ai_character_dialog.rs`, `ai_models_dialog.rs`, `ai_rename_dialog.rs`);
Scene from Image, Direct Story and Auto-Rig have no UI of their own beyond a
file picker and the Tasks panel. Add `PanelId::Ai` ("AI") to the workspace,
`Dock::Hidden` in Essentials (see `docs/WORKSPACE_DESIGN_PLAN.md`), home
`Dock::RightOuter`, shortcut in Window, and draw it in
`crates/buzz-ui/src/ai_panel.rs` as collapsible sections in this order:
1. **Models** — the `ai_models_dialog` body inline: each model's state
   (installed / downloading with progress / missing), Download, the LLM server
   URL and a "Test connection" button, the chosen chat and vision models, and
   a reasoning on/off switch (F7.3).
2. **Character from Image** — the `ai_character_dialog` steps inline: pick
   image, detect (joint overlay editable on the stage-sized preview), parts &
   trace, name, Create; the pose sheet (rest, walk, run, sit, point, wave)
   as thumbnails after creation.
3. **Auto-Rig selection** — one button plus the mapping table (layer →
   slot) after detection, with a Re-detect and an Apply.
4. **Scene from Image** — pick image, candidate list with SigLIP scores and
   the chosen symbol per object (editable dropdown), Build.
5. **Direct Story** — the brief text box, the cast, model timing (elapsed),
   the resulting beats and dialogue cues, Apply.
6. **Identify & Rename** — the `ai_rename_dialog` body inline.
7. **Activity** — the AI rows of the Tasks panel (elapsed, Cancel).
The dialogs stay as thin wrappers over the same section functions so the
menu commands keep working; each section is a `pub fn section_*(ui, state,
…) -> Action` in `ai_panel.rs`, and the dialogs call them.
Tests: `ai_panel_renders_every_section_headlessly`, every AI command reaches
its section (`Command::Ai* → PanelId::Ai` reveal), and the existing
`ai_*_through_ui` tests keep passing.

### F7.1–F7.7 (from the Status table's F7 row)
1. C10 redesign: candidates = placeable top-level symbols; SigLIP crop↔thumbnail
   primary score with a threshold; names/tags secondary; recall against
   visible top-level instances. Real-model run on bhatta2 frames recorded.
2. C11: prompt instructs the model to copy quoted speech into `dialogue`; a
   fallback lifts quoted text from the brief when the model leaves it empty.
3. Speed with reasoning models: request no/low reasoning where LM Studio
   supports it (`reasoning` / `think` off in the request), recommend a
   non-reasoning model in the Models section, show elapsed time.
4. C9: hands/wrists map to the forearm slot (the biped's Elbow slot carries
   the hand).
5. `buzz-export` Exporter readback panic when reused across output sizes.
6. A bare Armature on a stage layer renders blank in `ai_real_check`; check
   against the app's merge path and fix whichever is wrong.
7. The small brown dot below Oldman's left knee (cap or speck).

### F7.8 — a "Claude API server" backend beside LM Studio and Ollama
The user runs a local OpenAI-compatible server ("Claude API server") at
`http://127.0.0.1:8787` (`/v1` for the OpenAI SDK shape) that needs a key in
`Authorization: Bearer <key>` or `x-api-key: <key>`. Add
`LlmBackend::ClaudeApiServer { base_url, key }` in `buzz-ai/src/llm.rs` (same
chat/embeddings calls as OpenAiCompatible, plus the header), list it in the
Models section with a base-URL field, a masked key field, "Test connection"
(GET `/v1/models`), and the model dropdown filled from that endpoint. **The
key is a secret**: it is stored only in the user's config beside the dock
layout (`%APPDATA%/BuzzAnimate/ai.json`, mode-restricted where the OS allows),
never in the repo, tests, docs, snapshots or reports (mask it as
`sk-local-…` everywhere it is shown). The Models section also shows which
backend each AI flow will use.

### F7.9 — transcription (speech → captions and lip sync)
There is no speech-to-text in the app. Add `buzz_ai::transcribe` on a local
ONNX Whisper (Xenova/whisper-small.en or -base.en via the model registry with
real SHA-256s recorded in the plan's model table), taking a sound from the
Sound library (decode through `buzz-audio`, resample to 16 kHz mono), and
producing timed segments that become an `.srt`-shaped caption track and the
lip-sync source (the existing `.srt` join in `Editor::sync_directed_dialogue`).
UI: a "Transcribe" section in the AI panel (pick a library sound, language,
Run, progress, the resulting lines editable, Apply to captions / lip sync). A
test transcribes a short bundled clip (record numbers, not exact words).

### F7.10 — the end-to-end report, twice
`docs/AI_FEATURES_REPORT.md` with PNGs under `docs/images/ai/`: for each AI
feature (Character from Image, Auto-Rig, Scene from Image, Direct Story with
dialogue and lip sync, Identify & Rename, Transcribe) show the input, the
result, timings and the model used, **once with the local backend (LM Studio
+ ONNX) and once with the Claude API server backend**. Use the user's real
characters from `bhatta2.fla` / `barish1.fla` (rigged, drawn characters such
as ThakurBhaiSide, Ramu, OldManSideBody; and a full-body PNG rendered from
one of them for Character from Image), never stick figures. Every PNG comes
from the headless flows (`App::new` + `build_ui` + export, the snapshot
harness for panels). Keys are masked. The report ends with a table of what
works, what is partial, and what failed, with the evidence path per row.

### Then the "Open items when the plan resumes" (1–4 above)

### Acceptance
- `cargo test -p buzz-ai -p buzz-ui -p buzz-app` green; the ignored
  real-model tests run with `-- --include-ignored` where the models exist,
  results recorded (pass / measured error).
- `examples/ai_real_check.rs` re-run on bhatta2 with LM Studio: C8, C9, C10
  (recall > 0.5 on the two frames), C11 (dialogue cues present).
- Headless UI: `App::new` + `build_ui` opens the AI panel, walks Character
  from Image on `pujariFront_0.png`, creates the character, and the pose
  sheet PNG is written; screenshot of the panel via the Stage 0 harness of
  the workspace plan if it exists by then.

## Out of scope for now
- Generating new artwork with diffusion models (possible later with SD or
  Flux locally).
- Video motion capture beyond C7's classifier.
- Cloud APIs.
