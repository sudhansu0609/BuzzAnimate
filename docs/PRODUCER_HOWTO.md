# Producer — how to use it, and where the proofs are

Script + recording + character art in, a finished `.buzz` and `.mp4` out.
Everything below is relative to the repo root `B:\youtubeProjects\Buzzcaf_Media\BuzzAnimate`.

## 1. What you need before you start

| Thing | Where / what |
|---|---|
| Script | one text file, screenplay style (sluglines `INT./EXT.`, `NAME:` lines). Example: `B:\youtubeProjects\Originals\series\ep1\script.txt` |
| Recording (optional) | one WAV of the dialogue (`--audio`), or one WAV per scene in a folder. Without it lines are timed from the text. |
| Character art (optional) | one sub-folder per character, named after the character (`ep1\guddu\`, `ep1\raju\` …) with T-pose pictures. Charts, side profiles and hero shots are skipped automatically. A character with no folder is drawn by ComfyUI. |
| LM Studio | running on port 1234 with a Gemma model loaded (the breakdown step; 10–13 min for a 16-scene script). Not running → offline breakdown (no LLM). |
| ComfyUI | the portable install `B:\ComfyUI_windows_portable_nvidia_cu118_or_cpu\ComfyUI_windows_portable`. `--launch-comfy` (or the dialog's tick box) starts it for you (~50 s). |
| Style | `comic` (Originals / Chirailiya), `inked` (default, flat cel + bold ink), `painterly` (Khayal3Baje look), or free text. |

## 2. Run it from the app

1. Open `target\release\buzzanimate.exe`.
2. **File ▸ AI ▸ Produce Episode…**
3. Fill in: Script, Audio (optional), Characters folder (optional), Style, Series name, World (e.g. `rural Bihar, India, late 1990s` — filled in for you when the series is Chirailiya: a semi-rural small town in North India, 2010s), Scenes (blank = all, or `4,5` / `4-6`), Output folder, tick "Generate backgrounds with ComfyUI" and "Start ComfyUI if it is not running".
4. **Produce**. It runs as a task in the Tasks panel (Cancel works at every step, including mid-LLM call). When it finishes the `.buzz` opens in the app.

## 3. Run it from the command line

```text
cargo run --release -p buzz-app --example produce -- ^
  --script "B:\youtubeProjects\Originals\series\ep1\script.txt" ^
  --audio  scratchpad\produce\p7\scenes4_5.wav ^
  --characters "B:\youtubeProjects\Originals\series\ep1" ^
  --style comic --series Chirailiya --world "rural Bihar, India, late 1990s" ^
  --scenes 4,5 --out scratchpad\produce\p7\run ^
  --mp4 --height 540 --stills 4 --launch-comfy
```

Useful flags: `--assets <library>` (save rigs/backgrounds into another library; the default is your real one), `--offline` (no LLM), `--no-backgrounds`, `--size 1920x1080`, `--fresh` (redo everything), `--redo <step>` (parse, breakdown, cast, backgrounds, audio, build, render, report — that step and every later one), `--cancel-after <s>`.

A second run with the same inputs skips every finished step (`produce.json` in the output folder remembers them).

## 4. Or ask the assistant

The AI panel has the tool `produce_episode {script, out, audio?, characters?, style?, series?, world?}` — "produce episode 1 scenes 4 and 5 from … with the comic style".

## 5. What comes out (`<out>\`)

| File | What |
|---|---|
| `breakdown.json` | scenes, who is in each, where they stand, entrances, time of day, location keys, background prompts, lighting, shot cues |
| `casting.json` | the same as a casting the episode builder consumes — edit this and `--redo build` to change staging by hand |
| `cast.json`, `cast\` | which picture became which rig (views, spill numbers, skipped pictures and why) |
| `backgrounds.json`, `backgrounds\*.png` | generated pictures (one per location key, cached by prompt hash, `.tryN.png` = regenerations after the people check) |
| `audio.json` | every line's start/end in the recording |
| `<series>.buzz` | the episode: one scene per timeline, cast as layer rigs, lip sync, blinks, breathing, captions, title cards, shots, lights, props |
| `scenes\*.mp4`, `<series>.mp4` | with `--mp4` (joined, with audio) |
| `stills\` | with `--stills N`: N frames per scene |
| `report.md` | the human-readable report: timings per step, cast, backgrounds, props placed / not placed, lines timed, cuts |

## 6. Fixing things by hand afterwards

* **A rig is wrong** → delete it from `Cast\` in the library and `--redo cast`; or build one picture yourself: AI ▸ Character from Image (select the picture on stage first), "↺ Reset to recommended" if settings were changed.
* **A cut arm/hand you made yourself** → Lasso the part, F8, put its transformation point on the joint, Modify ▸ Rig ▸ Extend Part Under Neighbours.
* **Background wrong** → delete `backgrounds\<key>-*.png` and `--redo backgrounds` (new seed), or change the prompt in `casting.json` (`background_prompt`).
* **Staging / shots / lights** → edit `casting.json` (`place`, `shots`, `lighting`, `props` with x/y) and `--redo build`.
* **Lines mis-timed** → check `audio.json`; `--redo audio` after fixing the recording.

## 7. Proofs (all under `scratchpad\`)

| Step | What to look at |
|---|---|
| **P7 / P8 finished episode** | `produce\p7\run\Chirailiya.mp4` (90 s, audio), `produce\p7\run\Chirailiya.buzz`, `produce\p7\run\report.md`, stills `produce\p7\run\stills\scene04_the_three_berozgaars_f0481.png`, `…f0801.png`, `scene05_raju_arrives_f0111.png`, `…f0555.png` |
| P7 inputs | `produce\p7\scenes4_5.wav` (Windows-TTS recording), `produce\p7\truth4_5.json` (true line times), `produce\p7\tts_multi.ps1`, scratch library `produce\p7\assets\` (old rigs in `produce\p7\cast_old_p7\`) |
| P7 run logs | `produce\p7\run1.log` … `run4_props.log`, `produce\p8_proof.log` (final rerun, 731 s), `produce\p8_suites.log` (all suites) |
| P1 breakdown | `produce\p1\llm.json` (Gemma, 16 scenes / 24 characters), `produce\p1\casting_llm.json`, `produce\p1\offline.json`, `produce\p1\llm_raw.json` |
| P3 backgrounds | `produce\p3\stall_inked\{bg,traced,compare,pan}.png` + `plates\`, `produce\p3\night\…` (moon glow sky), `produce\p3\stall\…`, tuning evidence `produce\p3\tune\` |
| P4 cast build | `produce\p4\real\cast_report.json` + `previews\*_poses.png` (the 12 Ep1 pictures), `produce\p4\out\previews\Conductor_poses.png` (ComfyUI-drawn character), `produce\p4\out\characters\Conductor.png` |
| P5 shots + lights | `produce\p5\shots.json`, `produce\p5\ep_list\stills\` (medium cuts), `produce\p5\ep\stills\` (push-in) |
| P8-A hang + spill + rim | `produce\p8a\guddu_side.log` (the kurbo hang), `produce\p8a\{Raju,Raju_3-4,Conductor}_{before,after}.png`, `produce\p8a\a2_peel_hand_4x.png`, `a2_peel_tpose.png`, `a2_matte_arm.png`, `produce\p8a\a3_before\`, `a3_after2\` |
| P8-B posed rigs | before: `produce\p8b\base\raju\raju_posed.png`, `base\guddu\guddu_posed.png`; after: `produce\p8b\final2\{raju,guddu,woman1}\*_posed.png`; crops `produce\p8b\final\raju\c_{hip,knee,shoulder,farleg}_3x.png`, `produce\p8b\d9\guddu\c_torso_2x.png`, `produce\p8b\final2\guddu_shoulders_d9_vs_e2_2x.png` (the open shoulder hump) |
| P8-C producer | `produce\p8c\shots.json` (speaker-change cuts), `produce\p8c\cancel\` (0.22 s cancel), `produce\p8c\alt\out\stills\` (prop layout experiment) |
| Extend Part (X1) | `x1\{hidden,rotated}_after.png`, `x1dbg\` |
| Chain borrow (B1b′) | `b4b2\raju_posed.png` |

Plan and status tables: `docs\PRODUCER_PLAN.md` (P1–P8), `docs\TRACE_SPILL_AND_OUTPAINT_PLAN.md` (character pipeline rows A–X).

## 8. Known limits (as of 2026-10-14)

* Front-view arms: when an arm drops, its wide root rises ~20 px above the shoulder line as a rounded hump (structural; needs the arm root rebuilt as a disc + a sleeve-cap part). Guddu's right shoulder keeps a small tab.
* Woman1 spill max 3.6 px (paper between hair strands); generated sheets keep a few grey dashes where the rim stroke is as dark as ink.
* Counter items (kettle, glasses, bag) are not placed on generated backgrounds unless you give them x/y in `casting.json`.
* The cancelled LLM worker keeps its socket open until the server answers.
* The Produce dialog has been verified headlessly, not clicked in the real window.
