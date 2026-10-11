# Series Ep1 — cast rigs and first scene

Written 2026-10-06. The user's request: "Do the rigging and setup, use any
background" for `B:\youtubeProjects\Originals\series\ep1`. Then: "swappable
mouths, eyes and extra views your rig notes call for — I need these as well.
Do whatever is required."

Source: `ep1/script.txt` (Episode 1 "Naye Padosi", 16 scenes) and
`ep1/raju34.txt` (the user's rig-pack notes: dedicated rigs per view,
swappable mouths, eyes, brows, expression presets, 3/4 front first). There
are five character folders of ChatGPT T-pose images.

## Status

| Step | What | State |
|---|---|---|
| R1 | `set_property element:<id> bone.<n>.angle` (read and set; keyed at the playhead like a bone drag) | done 2026-10-06 — `bone.<n>.angle` in `agent/props.rs` (degrees vs parent, `set_pose` clamps to limits, written on the playhead's keyframe via `update_object_where` so auto-key makes one; undo label "Pose"). Tests: `bone_angle_poses_a_bone_at_the_playhead_in_one_undo_step` + the generic every-field/bad-calls/inspect tests pass |
| R2 | Relaxed rest pose: arms down from the T-pose, saved as the rig's rest pose | done 2026-10-06 — bones are the 11-bone Biped (Hips, Chest, Head, Shoulder L, Elbow L, Shoulder R, Elbow R, Thigh L, Knee L, Thigh R, Knee R; Shoulder = upper arm, Elbow = forearm+hand). The example swings each Shoulder to 75 deg below horizontal on its own side with R1, then **bakes**: `set_rest_pose` alone only re-measures bones against the artwork as drawn (the arms would snap back to T-pose), so the saved document is reloaded, every part's artwork replaced by its posed copy, rest := pose, rebind. All 12 previews looked at: arms hang. Raju Back is already a relaxed image (kept as drawn). Woman1 Back: DWPose swapped the wrists between arms (forearms across the body); the example detects an elbow bent >60 deg and rebuilds through the same public stages with the wrists swapped back |
| R3 | `series_setup` example: one rig per view image, saved to Cast under the plan's names | done 2026-10-06 — `crates/buzz-app/examples/series_setup.rs`; all 12 views built (each 2-5 s) and saved to the real Cast as Guddu, Guddu 3-4, Guddu Back, Lalten, Lalten Back, Munna, Munna Back, Raju, Raju Back, Woman1, Woman1 3-4, Woman1 Back (none existed before; `--replace` overwrites only those names; the pipeline's own T-pose save goes to a scratch library, so no stray "Name 2"). Per-rig `.buzz` + preview PNG + report in `ep1/buzz/rigs/`. The bake also sets a face part that loops a multi-frame symbol (the viseme mouth) to hold frame 0 (Rest): left looping it flickers Rest/MBP on any timeline |
| R4 | Ep1 scene: Daylight + Village scenery, the five front rigs placed, scaled and performing; saved `.buzz` + preview PNGs | done 2026-10-06 — `series_setup --scene-only`: `ep1/buzz/ep1_scene5_setup.buzz` (144 frames; Lalten, Guddu, Woman1, Munna idle 1-144, Guddu talk 97-130, Raju talk 61-120 and walk 1-60 in from off-stage right, all at 486 px = 45% of 1080, soles on y 972 = 0.9), previews `ep1_scene5_f001/f030/f060/f120(.png 768 px snapshot, _hd.png 1280 px from the reloaded file)`, all looked at. Executor `bounds` of a rig over-state its width (instance placeholder boxes), so the example measures the rendered asset instead. The file is 254 MB: the format stores the whole rig in every keyframe (~1.5 MB x 72-102 keys x 5) |
| M1 | Mouth chart import: slice a labelled chart image into drawn mouths and make them the character's mouth-symbol frames | done 2026-10-06 — `buzz_ai::face::mouth_chart` (`read_chart`/`read_chart_file`, `frame_plan`, `build_layer`/`build_mouth_symbol`, `replace_mouth_frames`). Mouths are told from labels by **colour** (coloured ink = mouth, grey/black = lettering), pieces joined by a small gap, holes filled so teeth stay; labels come from the caller's list in reading order. All 19 mouths on Raju's chart cut whole and labelled right (centres within 30 px of where read by eye; preview contact sheet of all 20 frames viewed). Frames: 0..9 are the visemes in lip-sync order (Rest=Neutral Closed, Ai=A, E, O, U, L, WQ=U, MBP=M/B/P, FV=F/V, Etc=S/Z), then extras 10.. named by label (Small Smile, Broad Smile, Frown, I, TH, CH/J, Open Laugh, Angry Tight Mouth, Confused Half-Open, Skeptical Side-Mouth); every frame is a keyframe with its name as the label. Tests: `crates/buzz-ai/tests/mouth_chart.rs` (synthetic chart: slicing, order, scale, teeth, label-count error, frame plan, symbol, replace + `write_track`; `#[ignore]`d `real_raju_chart_imports_whole_and_labelled` writes the preview) + 2 unit tests in the module. **Shared-tracer defect worked around, not fixed:** `buzz_scene::bucket::trace_contours` keeps one outgoing edge per grid corner in a `HashMap`, so where two pixels of a region touch only at a corner one edge is overwritten, the loop dead-ends and is closed by a straight chord: a wedge of the wrong colour across the shape (3 such corners in the O mouth alone). `trace_mouth` posterises each mouth itself first and thickens every corner contact away before the tracer sees it. The proper fix (a multi-map plus a right-turn choice at a corner) would also help Trace Bitmap and the character pipeline |
| M2 | Raju gets his chart mouths; lip-sync uses them | command done 2026-10-06; **run on Raju's rig by `series_setup` 2026-10-06 (see Q-rebuild)** — `ai_mouth_chart {element: <rig id> \| character: <name>, path, labels?}` (`agent/dialogs.rs`, `Command::AiMouthChart`, agent only). Replaces the mouth symbol's frames in one undo step, keeping the symbol id (every instance and lip-sync keyframe carries on); sized to the current Rest frame and in its line weight; refuses unknown/missing/both args, an unknown character, a label-count mismatch, in words. Tests: 2 in `agent/dialogs.rs` (`mouth_chart_tests`: rig and name lookup; refusals change nothing; by name and by element, one undo step each; undo restores) and the generic `ai_agent_coverage` command tests pass. To finish for Raju: after R3 builds his rig, call `ai_mouth_chart {character:"Raju", path:"…03_29_03 AM.png"}` and then re-save him to Cast (the Cast asset still holds the old mouth until then) |
| M-review | Opus review 2026-10-06 | **accepted.** Viewed the 20-frame preview: all 19 chart mouths whole, labelled right, teeth/tongues kept. Found: shared tracer `trace_contours` drops an edge at pixel corner contacts (chords across shapes) — also hits the character pipeline; root fix delegated (T1). |
| T1 | Fix `buzz_scene::bucket::trace_contours` corner contacts; rebuild rigs with `--replace` after | **done** (left-turn join at corner contacts, 6 new exact-area tests; re-ran 47 bucket/trace/wand tests). Rig rebuild with `--replace` pending integration |
| R-review | Opus review 2026-10-06 | **accepted with defects.** Viewed scene f120 and the Guddu rig: 12 rigs in Cast, arms hang, Raju walks in and talks. Defects: pale joint/completion discs on torsos and hips, speckled check-pattern tracing, Woman1's hair washed out, mouth part looping (patched only in the saved assets), scene file 254 MB. Q1–Q4 (pipeline quality, then rebuild with --replace + Raju mouth chart) and F1 (file size) delegated. |
| F-review | Opus review 2026-10-06 | **accepted.** Format 46 rig-part table: scene 254 MB → 7.57 MB, load 11.8 s → 0.28 s, renders identical; re-ran buzz-doc 230/230. Needs a final release build so the user's app opens format-46 files. |
| Q1 | Pale discs on chest/hips, grey discs at the ankles and knees | **done 2026-10-06** (implementer; Opus review pending). Two causes, both in `buzz-ai/src/character.rs`. (1) `complete_parts_impl` flat-filled each part's bone capsule + joint circle with the part's dominant colour over **every** matte pixel it did not own, including real artwork of parts *below* it in the composite (Chest 72 / Hips 70 over thighs, shoulders, shirt; Foot over the trouser cuff): the "pale half-discs". Now a pixel is filled only if its visible owner paints over the part (higher depth, or same depth later in the list) = genuinely hidden, so the composite is the source pixels. (2) `trace_character_whole` put the joint cap (disc + exposed-arc outline) in the part's own shape list at the part's depth, so it lay over any lower part. Caps are now `TracedPart::caps`, bound by `assemble_character` as Rigid parts of the part's own bone at the **very back** of the rig's paint order (behind both parts the joint joins; invisible at rest), clipped to the character's silhouette (rays stop at the matte edge) and coloured from the pixels the part owns round its joint (palette-snapped), else the part's dominant colour. Tests: `rig_quality.rs` (`completion_never_fills_a_pixel_owned_by_a_part_it_would_paint_over`, `joint_caps_sit_behind_every_part_and_inside_the_silhouette`), `quality_redesign.rs` updated (cap now in `caps`; the fixed "40% of height" localisation proxy became "a fill lies inside the box of the part's own pixels"), `complete_parts.rs` (a part keeps all its own pixels; Hand L may own none). Side effect found and fixed: the old flat fill had hidden paper-coloured pockets of the matte (hands, feet), so `segment::matte` now takes the **paper-coloured halo** off an isnet matte (flood in from outside through pixels within 26 RGB of the page colour, at most 2% of the picture deep; `trim_paper_halo`, 3 tests) |
| Q2 | Checked/printed fabric traces as speckle | **done 2026-10-06.** Cause: the tracer drops regions under its speckle size but still draws an outline stroke round each dropped patch, and a pattern quantises into hundreds. `clean_composite_regions` (before the whole-character trace): (1) a non-ink pixel takes the most common non-ink colour in a 9 px window (erases threads up to ~4 px wide; a wider window flipped and merged the bands of a striped cloth), (2) every region under 2.5e-4 of the picture's pixels (~390 px at 1335 x 1178) merges into its longest-bordering neighbour (3 passes), (3) the picture is posterised to the palette so the tracer sees exactly these regions. **Kept:** ink-coloured regions (within 40 luminance of the line colour) at least 1.2% of the diagonal long are never merged however thin (outlines, moustache, brows); the head and face features are protected (only dither under 1e-5 of the picture merges). 3 unit tests beside the function. Result viewed on all 12 rigs: no speckle; plaids/prints become soft flat shapes (the check itself is lost), stripes stay readable |
| Q3 | Mouth part looping the 10-frame viseme symbol | **done 2026-10-06.** `assemble_character` builds the mouth instance `SingleFrame`, `first_frame` 0 (Rest); the example's bake patch stays as a guard. Test `the_mouth_part_holds_rest_instead_of_looping_the_visemes`. Found while applying Raju's chart: the mouth symbol's Rest frame was the whole traced mouth part (base fill + skin across a mask patch, 129 px on a 135 px face), so every placeholder viseme and the chart came out face-wide, and for Raju the part held no mouth at all (his mask took face-edge patches; the smile is in the Head's own shapes). `build_face_rig` now keeps only shapes inside the padded mouth-landmark box in the Rest frame, binds the rest to the head as `FaceRigSymbols::mouth_underlay`, and places/sizes the mouth from the landmarks when nothing traced lies inside (`restyle_mouth_visemes_at`). Test `the_mouth_symbol_is_the_width_of_the_mouth_and_not_of_the_patch_of_face_round_it` |
| Q4 | Eye whites larger than the drawn eyes | **done 2026-10-06 (sizing left as is).** `build_eye_white_symbol` size is landmark box x 1.2 and is **not** larger than the drawn eye (Guddu: 31 x 8 px against a drawn 33 x 33 round eye; it is flatter, not bigger). The big whites were holes: an eye part's disc mask (35 px round each contour point) took hair/skin at the temple that the Head's mask missed, and `assemble_character` drops everything an eye part drew, leaving those pixels unpainted (white wedges beside each eye). `part_masks` now keeps only what lies within 1.1 x the eye's width + 4 px of its centre; the rest goes back to the Head. Test `an_eye_part_owns_the_eye_and_not_the_temple_around_it`. **Open:** the synthetic eye is a flat lens inside a round drawn white on round-eyed characters |
| Q-rebuild | 12 rigs rebuilt with `--replace`; Raju's mouth chart applied | **done 2026-10-06.** `series_setup --replace` (rigs + scene), 12 views 0 failed; each preview viewed before and after. `series_setup` applies `ai_mouth_chart` to the Raju rig before his save (`MOUTH_CHARTS`, by the rig's **element**: by name it hit the library's `Raju_mouth`, a different symbol from the one his mouth instance uses, `Raju_mouth 1`, and left him at 10 frames). Raju's Cast asset: 20 mouth frames, instance SingleFrame at 0, chart mouths 53 px wide at the landmarks. Scene file now 4.1 MB (F1 landed) |
| Q-open | Seen on the rebuilt rigs, not in Q1-Q4 | (a) Hair washed out on front views (Guddu, Guddu 3-4): `trace_character_whole` recolours a near-ink fill to the part's dominant colour unless `region_is_dark` says the part's pixels under the fill's **bounding box** average dark, and a hair fill's box covers skin; test the mean luminance inside the shape instead. (b) Stray thin lines at T-pose shoulder height after the arms hang (Guddu, Munna Back, Raju, Lalten Back): strokes cut by owner are carried across background gaps (`cut_stroke_by_owner`) and stay with the chest. (c) A chart/placeholder mouth is drawn over the mouth the Head already holds when the Head's own shapes contain it (Raju). (d) Check/print patterns are flattened, not kept |
| Q-review | Opus review 2026-10-06 | **accepted.** Viewed scene f120: discs and speckle gone, Woman1 hair dark, faces clean, Raju's chart mouths in. Remaining: Guddu's hair washed orange → **fixed in review** (`region_is_dark` now measures inside the shape, not its bbox; buzz-ai lib 70 + quality tests pass), rigs rebuilding with --replace. Still open: Guddu left arm sticks out, white rim glow from the light rig, a stroke fragment at Raju's waist, plaids flattened. |
| S-review | Opus review 2026-10-06 | **accepted.** Viewed the sample sheet and 21 stills of 7 scenes; captions, entrances, talk, blinks and mouths are present. Rejected the sample casting (Munna Back as Chhotu); Chhotu is voice-only from the `shopa` stall's painted vendor. |
| V1 | Missing view images listed for the user (side; 3/4 for Lalten, Munna, Raju) | done: listed in ep1/buzz/README.md |
| S1 | Screenplay parser (scenes, sluglines, cues incl. V.O./O.S., dialogue, action, beats) | done 2026-10-06: `buzz_act::screenplay` (`Screenplay::parse`), 13 unit tests on excerpts, all pass |
| S2 | Timing model from text (no audio) | done 2026-10-06: `buzz_act::script_timing` (`Timing` struct holds every constant, `Timing::schedule`), 9 unit tests pass |
| S3 | Text-driven lip sync: visemes from romanised Hindi/English text, written with the existing mouth track writer | done 2026-10-06: `buzz_act::textsync` (`visemes_from_text`, `line_track`, `write_rig_mouth`; layer mouths go through `lipsync::write_track`), 16 unit tests pass |
| S4 | Shot builder: a scene's set (assets), cast (Cast rigs or assets), captions, talk/idle/walk performances, blinks, from a casting file | done 2026-10-06: `buzz_app::episode` (`Casting`, `build_scene`, `build_episode`, `report_markdown`); 7 unit tests + `tests/episode_build.rs` (20 headless tests: frames = summed timing, one `Subtitles` caption per line, rig mouth and eyes keyed on every rig key, entrance/exit, ground line and height, mirroring, uncast names skipped and reported). Casting also takes `puppet` (built-in character), per-scene `cast`/`asset`, `faces`, `aliases`. Blink: a puppet's eyes get the Blink modifier; a rig's eyes are inside its armature (a modifier there would squash the whole figure) so a rig blinks by swapping its eye parts for half-shut/shut copies on its own keys. **Rigs are slimmed on placement**: the image rigs hold ~4 MB of path data per keyframe (the first run wanted 35 GB to save one scene), so each rigid part becomes an instance of its own library symbol (`slim_rig`) |
| S5 | `episode_build` example: script + casting file → one `.buzz` (one document scene per script scene) + MP4 previews | done 2026-10-06: `crates/buzz-app/examples/episode_build.rs`; `cargo run -p buzz-app --example episode_build -- --script <script.txt> --casting <casting.json> --scenes 4,5,6 --out <dir> [--mp4] [--stills N] [--frames 400,600] [--height 540] [--assets <dir>] [--size 1920x1080]`. Writes `<out>/ep1.buzz`, `<out>/report.md`, `<out>/previews/sceneNN_title.mp4` (headless render) and `<out>/stills/*.png`. Run on the real script's scenes 4 and 5 with a scratch casting file (Guddu, Munna, Lalten, Raju, Munna Back as Chhotu): both built, previews rendered and viewed |
| S6 | Ep1 run | **done 2026-10-06 07:45**: `ep1/buzz/episode/ep1.buzz`, scenes 4,5,6,7,10,12,14 (7603 frames, 5m17s), stills viewed by Opus. Review fixes: `SceneSet.absent` (named-but-not-there; Raju out of 12, Amma out of 14); stall rescaled. MP4 previews done and checked by frame extraction: scenes 4, 5 (re-rendered, Raju enters right), 7, 10, 14 (09:51). Release app rebuilt; opens ep1.buzz (format 46). Tests: geom/scene/doc/act/ai/app all green apart from 3 pre-existing buzz-ui dock_columns failures; the Claude test was updated for medium effort. Morning summary: `ep1/buzz/README.md` |
| F1 | Small files: a rig keyed many times writes its artwork once (format 46) | **done 2026-10-06** — `buzz_doc::serial`: every distinct rig part (artwork + skin weights / rigid bone) goes in a per-document `rig_parts` table once; an armature key writes only `part_refs` (slot numbers) plus its bones/pose. Parts are told apart by allocation (same `Arc` + equal binding, no re-conversion) then by exact compact-JSON content (bytes compared, not hashed), so a rig keyed 100 times, a rig placed five times, or an old file whose copies are separate allocations all collapse to the distinct parts. Slots may refer only backwards (a rig inside a rig), so a hand-made cycle is refused in words. **Old files read unchanged**: `parts` inline is still understood (and may mix with refs); `FORMAT_VERSION` 45 -> 46 (older builds refuse a new file by version, as for every bump). Load builds each slot once, so keys share one allocation in memory (the real scene: 8004 part copies -> 115 allocations). **Real scene** (`ep1_scene5_setup.buzz`, 5 rigs x 144 frames, 99.9% of it `parts`): **253,950,368 B -> 7,573,485 B (254 MB -> 7.6 MB)**; load 11.8 s -> 0.28 s; save 19.4 s -> 0.57 s (6.8 s when converting an old file whose 8004 copies are all separate allocations); peak memory ~14.5 GB -> ~0.4 GB. Rebuilt with `series_setup --scene-only` into a scratch folder and also re-saved from the 254 MB file with `crates/buzz-doc/examples/resave.rs`; frames 60 and 120 render byte-identical from the old and the new file, frame 1 differs in 2 pixels by 1/255 (the same last-bit float noise the old writer's own save/load gives: 3 pixels). The 254 MB file in `ep1/buzz` is untouched. Tests (buzz-doc, 230 pass): `serial::rig_part_table_tests` x 12 (40 keys cost ~1 key; exact layer equality after load; shared allocations after load; equal-but-separate parts share a slot; parts differing in binding or colour do not; rig in a symbol and rig inside a rig; a v45-layout fixture `tests/fixtures/rig_keyed_v45.json` loads and shrinks on re-save; v45 + table mixed; out-of-range ref and self-referring slot refused; no table when no rig) and `format::tests` x 2 (100 keys through the real container < 3x one key; old-layout archive opens). **Known, not fixed:** serde_json parses floats without `float_roundtrip`, so any save/load can move a 17-digit number by its last bit (before and after this change; `features = ["float_roundtrip"]` on the workspace `serde_json` would make round trips bit-exact at the cost of rebuilding every crate); a multi-scene `.buzz` keeps one table per scene file, so the same cast in 16 scenes is written 16 times (a container-level table is the next step if episodes grow); skin weights (`Vec<Vec<(usize, f64)>>`) are still deep-copied per key in memory (zero rows in this scene: all its parts are rigid) |

## Image → view mapping (decided by looking at every image)

| Character | Front | 3/4 | Back | Other |
|---|---|---|---|---|
| Guddu | `guddu/Goofy Indian Character in T-Pose.png` | `guddu/Guddu in a T-Pose Pose Sheet.png` | `guddu/Back View Character in T-Pose.png` | — |
| Lalten | `lalten/Lalten in T-Pose Character Design.png` | — | `lalten/Cartoon Man in T-Pose with Scarf.png` | `Cheerful Cartoon Character in T-Pose.png` (second front, unused) |
| Munna | `munna/Anxious T-Pose Character in Checked Shirt.png` | — | `munna/Munna Back-View Character Turnaround.png` | `Worried Indian Character in T-Pose.png` (second front, unused) |
| Raju | `raju/ChatGPT Image Oct 6, 2026, 03_31_26 AM-1.png` | — | `raju/ChatGPT Image Oct 6, 2026, 03_31_27 AM-2.png` | mouth chart `03_29_03 AM.png`; hero `Friendly Indian Office-Goer Character.png` (not a T-pose, unused) |
| Woman1 | `woman1/Cartoon Indian Woman in Traditional Sari.png` | `woman1/Indian Sari Character in T-Pose.png` | `woman1/Back View of Woman in Mustard Sari.png` | — |

Cast names: `Guddu`, `Guddu 3-4`, `Guddu Back`, `Lalten`, `Lalten Back`, and so
on. "Woman1" keeps the folder's name until the user says who she is (likely
Amma).

## What already exists (don't rebuild)

- `ai_character_from_image {path, character_name}` (agent command): isnet →
  DWPose (incl. 68 face points) → SAM2 parts → LaMa completion → trace → rig,
  with the **face rig** from `buzz_ai::face` (C6): eye whites + pupils +
  eyelids (blink, look_at) and a mouth symbol whose frames are the visemes
  (Rest = the traced mouth, the rest restyled placeholders). Saves to Cast.
  `wait_for_jobs` waits.
- `inspect {what:"objects"}` reports every object's drawn `bounds` (added
  2026-10-06).
- JSFL: `document.setTheScene({setting, cast:0, horizon, frames})`,
  `document.layScenery("village", horizonY, backdrop)`, `perform`,
  `performFully`, `lipSync`, element `.matrix` get/set, `__objectId` on elements.
- `edit_list {target: element:<id>, list: "rig", op: "set_rest_pose"}`.

## R1–R4 (implementer A)

- **R1.** In `agent/props.rs` add the field `bone.<n>.angle` (degrees, the
  pose angle relative to the parent, as `Armature::pose()` gives) to the
  element target. Set writes the armature pose at the playhead through the
  **same editor path a bone drag uses** (find it: the Bone/Selection drag on
  an armature). One undo step, keyed if auto-keyframe is on or the frame is
  a keyframe, exactly as the drag behaves. Read returns the current angle.
  Add it to `PROPERTY_FIELDS` and the coverage tests.
- **R2.** In the example (not in the pipeline), after each rig is built:
  find the upper-arm bones by name (log the bone names the auto-rig gives;
  match "upper arm"/"shoulder", left/right), rotate each down by the angle
  that takes it from its T-pose direction to ~75° below horizontal (arms
  hanging, slightly out, mirrored per side), then `set_rest_pose`. Verify by
  rendering a snapshot and **looking at it**. Back views: same.
- **R3.** `crates/buzz-app/examples/series_setup.rs`:
  `cargo run --release -p buzz-app --example series_setup -- --ep <ep dir> --out <dir> [--replace]`.
  The mapping table above is a `const` in the example. For each image:
  `ai_character_from_image` + wait, R2, and the Cast name from the table.
  If a Cast asset of that exact name exists: without `--replace`, skip it and
  say so; with it, overwrite. **Never delete any other asset.**
- **R4.** One scene in a new document: `setTheScene({setting:"daylight",
  cast:0, frames:144, horizon:0.6})` + `layScenery("village", …)`. Place the
  five **front** rigs: feet on one ground line (~0.9 of stage height), a
  common figure height (~45% of stage height; Lalten slightly wider is fine),
  spread across x (Raju entering from the right). Performances (Scene 5 "Raju
  arrives"): Guddu, Munna and Lalten `idle` for the length; Raju `walk` in from
  off-stage right over frames 1–60 then `talk` 60–120; Guddu `talk` 96–130.
  Save `<out>/ep1_scene5_setup.buzz` and preview PNGs at frames 1, 60, 120
  (`snapshot`). Report bone names, rig ids, timings, failures.
- Tests: R1 in `ai_agent_coverage`. The example is verified by running it
  and **viewing** the PNGs.

## M1–M2 (implementer B)

- **M1.** `buzz_ai::face::mouth_chart`: read a chart image like Raju's (warm
  off-white background, mouths in rows, a text label under each). Find the
  mouth blobs (foreground connected components, labels excluded by size and
  their position under a blob). Assign each blob its label by **reading order
  against a given label list**, not OCR: the call takes `labels: &[&str]` in
  reading order. For Raju that is `["Neutral Closed","Small Smile","Broad Smile","Frown","A","E","I","O","U","M/B/P","F/V","L","TH","CH/J","S/Z","Open Laugh","Angry Tight Mouth","Confused Half-Open","Skeptical Side-Mouth"]`.
  Trace each blob (same tracer as the pipeline), normalise to one size and
  anchor (centre of the closed mouth's box), and produce frames.
  Viseme mapping onto the existing mouth symbol (`buzz_audio::Viseme`):
  Rest←Neutral Closed, MBP←M/B/P, FV←F/V, Ai←A, E←E, O←O, U←U, and any other
  viseme the enum has from the nearest label (L, TH, CH/J, S/Z, I). The
  expression mouths (smiles, frown, laugh, angry, confused, skeptical) become
  **extra named frames** after the visemes, so they can be keyed by hand. Keep
  the lip-sync frame order unchanged so `write_track` still works.
- **M2.** Agent command `ai_mouth_chart {element: <character rig id> | character: <name>, path, labels?}`
  (labels default to the list above) replaces that character's mouth-symbol
  frames in the document, as one undo step. Tests with a synthetic chart (drawn
  ellipses plus text boxes) and an `#[ignore]`d test on the real Raju chart
  that writes a preview PNG of all frames.

## V1 — images the user still needs to make

Side views for all five, and 3/4 front for Lalten, Munna and Raju: the notes
say 3/4 front carries 60–70% of dialogue shots. Same T-pose, same proportions.
A mouth chart for each character besides Raju.

## S1–S5 — script to scenes (implementer C)

The user is asleep for the night (2026-10-06): "use the script transcript to
assume the timings and movements; build whatever is needed". There is no
audio, so timing and mouths come from the text.

- **S1 parser** (`buzz_act::screenplay` or `buzz-doc`): the markdown
  screenplay form of `ep1/script.txt`:
  - `## SCENE n — TITLE` starts a scene.
  - `**EXT./INT. … — TIME**` is the slugline (location + time of day).
  - A line that is only `**NAME**` or `**NAME (V.O.)**`/`(O.S.)` is a cue;
    the following non-blank lines until a blank line are its dialogue.
  - `---` is a beat break; a line `Beat.`/`Another beat.`/`Long silence.`/
    `Silence.`/`A pause.` is a pause.
  - Anything else is action.
  - Stop at `# END`; everything after it is notes.
  Unit tests on excerpts of the real file (copy short excerpts into tests;
  don't read the user's folder from tests).
- **S2 timing** at 24 fps. Dialogue: `max(0.9 s, words × 0.36 s)`, plus
  0.35 s per `...`/`—` and 0.15 s at `?`/`!`. 0.3 s gap between lines. Pauses:
  Beat 0.8 s, Long silence 2.0 s, Silence/A pause 1.2 s. Action lines:
  `clamp(chars × 0.05 s, 1.0, 3.0)`; a beat break adds 0.5 s. Constants live
  in one struct so they can be tuned.
- **S3 text lip sync**: `visemes_from_text(text, frames) -> Vec<Viseme>` over
  `buzz_audio::Viseme`. Romanised letters → visemes: a/aa→Ai, e/i/ee→E,
  o/au→O, u/oo/w→U, m/b/p→MBP, f/v→FV, the rest consonants → a short
  neighbouring vowel-ish open (E) or Rest at word gaps; punctuation → Rest.
  Spread evenly over the line's frames, with at least 2 frames per shape. It
  writes through the same function the audio lip sync uses (`write_track` or
  equivalent), onto the character's mouth symbol from the face rig. Tests.
- **S4 shot builder**, driven by a **casting file** (JSON, written by the
  orchestrator, not the implementer) beside the script:
  ```json
  {"characters": {"GUDDU": {"cast": "Guddu"}, "CHHOTU": {"asset": "chaiwalNew"}, "NARRATOR": {"voice_only": true}},
   "scenes": {"4": {"setting": "daylight", "background": ["backgroundRoadA"], "props": [{"asset": "chaiTapri", "x": 0.7, "y": 0.62, "h": 0.5}], "place": {"GUDDU": {"x": 0.3}, "MUNNA": {"x": 0.42}}}}}
  ```
  Per scene: a new document scene (`AddScene`) named after the script scene;
  frames = summed timing.
  - **Set:** the background/props assets from the asset library
    (`placeAsset` / `Editor::place_asset_at`), scaled to the given fraction
    of stage height. If none are given, `setTheScene` + `layScenery("village")`.
  - **Cast:** Cast rigs placed by name, feet on the ground line, common
    height, at the given x (else spread). Assets for non-rigged characters.
  - **Performances:** the speaker `talk`s for each line, with text lip sync
    on rigs that have a face mouth. Everyone else `idle`s. A cue entering
    ("walks briskly toward them", "Raju arrives", "approaches") makes that
    character `walk` in from the nearer side first. Leaving ("runs toward",
    "flies away", "leaves") makes them `walk`/`run` off. Blink modifier on
    every rig.
  - **Captions:** a `Subtitles` layer, one keyed text per dialogue line
    (`NAME: line`), bottom centre, white with dark outline. V.O. lines are
    captioned in italics with no on-screen speaker.
  - **Unmatched cues:** a character with no cast/asset entry is skipped
    with a note in the report, never a failure.
- **S5** `crates/buzz-app/examples/episode_build.rs`:
  `-- --script <script.txt> --casting <casting.json> --scenes 4,5,6 --out <dir> [--mp4]`.
  It writes `<out>/ep1.buzz` and, with `--mp4`, one preview MP4 per scene
  through the headless render (`buzz_app::headless::render`). It also writes
  `<out>/report.md`: per scene, the frames, lines, who was cast, what was
  skipped, warnings.
- Everything goes through the same editor/JSFL paths a person or the
  Assistant uses (one undo step per scene build is fine).

### S1–S5 as built (2026-10-06)

* **Where:** parser `crates/buzz-act/src/screenplay.rs`, timing `crates/buzz-act/src/script_timing.rs`, text lip sync `crates/buzz-act/src/textsync.rs`, shot builder `crates/buzz-app/src/episode.rs`, example `crates/buzz-app/examples/episode_build.rs`, tests `crates/buzz-app/tests/episode_build.rs`.
* **Casting file** (see the module docs of `buzz_app::episode`): `characters` by cue name (`cast` = a Cast rig, `asset`, `puppet` = built-in character, `voice_only`; also `h`, `faces`, `aliases`, `folder`); `scenes` by script scene number (`"*"` for all): `setting`, `scenery`, `horizon`, `ground`, `lit`, `background[]`, `props[{asset,x,y(base),h,front}]`, `place{NAME:{x,y,h,enter,cast,asset}}`. A name with no entry is skipped and reported (its lines are still captioned).
* **Verified by eye** on scenes 4 and 5 of the real script with a scratch casting file: relaxed rigs standing on one ground line at a common height, Raju walking in from the right, talk gestures, mouths changing on the lines, blinks, white-on-dark captions.
* **Known limits:**
  * A rig's keyframe carries the whole armature. Image rigs hold megabytes of path data in their parts, so the builder moves each rigid part's drawing into a library symbol (`slim_rig`); the pipeline could make its rigs that way to begin with. Skinned parts are not slimmed.
  * A rig's eyes are part of its armature, so rigs blink by swapping the eye part for a half-shut/shut copy on their keys; the Blink modifier is used only on separate eye objects (the built-in puppet).
  * Timing is purely sequential and from text: scene 4 has fifteen seconds of action lines before the first spoken line. Everything is in `Timing`.
  * Characters face front: a walk is a leg cycle sliding sideways until side views exist (V1). `faces` mirrors a drawing that looks one way.
  * A line that overlaps one of its speaker's walks is captioned but not performed (the report says so).
  * Rendering the previews is slow on a loaded machine (scene 4, 1764 frames at 540p: 25-31 minutes when the GPU was shared; scene 5, 1085 frames: 2.7 minutes once, longer later). The built-in puppet makes a scene slower still.
