# Cast characters start fresh (2026-11-02)

User: "make sure for all the characters I can select a button and all keyframes are
cleared for that character, so that character can be used in a new file from fresh."

Today Modify ▸ Symbol ▸ Clear All Keyframes… (and the Library right-click) clears
one symbol of the open document (`Scene::clear_symbol_keyframes`, keep a pose,
`nested`). A Cast character is an asset on disk (`AssetLibrary::user()`, folder
`buzz_ui::CAST_FOLDER`), so the keys it was saved with come along every time it is
cast into a new file.

## What to build

* **Cast panel, per character: "Start Fresh"** button (next to Cast / Delete), hover
  text "Clear every keyframe of this character, keeping one pose, so it starts clean
  in a new file". Opens a small confirm window: pose to keep = first frame (default)
  or the pose shown in the sheet; OK / Cancel.
* **Cast panel header: "Start Fresh: all…"** does the same for every character in the
  Cast folder (and its subfolders), with one confirm listing how many.
* What "fresh" means for one asset document: every library symbol reachable from its
  stage objects gets `clear_symbol_keyframes(id, keep, nested = true)`; the asset's
  own stage layers are cut to one keyframe at frame 0 holding the kept pose and a
  length of 1 frame; armature rigs keep the kept pose as their rest/only key; layer
  parenting, mouth/eye symbols' frames (lip-sync shapes are frames of the mouth
  symbol, NOT keys to clear: a mouth/eye/hand *swap* symbol's own timeline is left
  alone — skip symbols whose layers hold one drawing per frame used as a frame
  picker; find how `textsync::write_rig_mouth` / blink swaps identify them and
  exclude those), colours, names, tags unchanged. Audio guide layers removed.
* **Backup first**: before writing, copy the asset file to
  `<library root>/_cast_backup/<YYYY-MM-DD_HHMMSS>/<same relative path>` (outside the
  Cast folder so it is not listed). Status line says where. The asset is then saved
  in place, thumbnail refreshed, `cast_sheet` reset.
* Agent action `cast_start_fresh {name | all: true}`.

## Status

| Step | What | Who | Status |
|---|---|---|---|
| F1 | Build the above. Gate (on COPIES only: copy `AssetLibrary::user()` root's Cast folder to the scratchpad and open that as the library in a test/example; never write the user's real library during the gate): for every character in the copy, after Start Fresh: total keyframes on every layer of every reachable non-swap symbol == 1, stage layers 1 frame, mouth/eye swap symbols' frame counts unchanged, a render of frame 0 before vs after PSNR ≥ 40 dB when keep = first frame (sheet of 6 characters before/after looked at), placing it into a new empty document gives a 1-frame instance with no keys; backup file exists and loads. Headless App test for the button + confirm (no real mouse) | Sonnet → Opus review | built 2026-11-03, awaiting Opus review. `Scene::start_fresh` + `picker_symbols` (buzz-scene), `crates/buzz-app/src/cast_fresh.rs` (backup + save), Cast panel buttons + confirm, `cast_start_fresh` agent action. Gate on a copy of the 66 Cast files (`cast_fresh_gate` example): the real Cast already had 1 key everywhere (only stage layers were 1440 frames long), so a second run used INJECT=1 (12 frames, keys 0/4/8 on every stage + non-picker symbol layer): 2514 keys cleared, max keys after 1, stage 1 frame/1 key, picker symbols unchanged, frame-0 PSNR 99 dB (identical), backups byte-identical and load, placing into a new doc is clean; sheet looked at. Tests: `start_fresh_clears_keys_but_leaves_frame_pickers_alone`, `start_fresh_button_confirms_backs_up_and_clears_the_character` (headless build_ui + accesskit clicks), `cast_fresh::tests::the_stamp_is_a_folder_name`. Not done: pose-to-keep = a named pose from the sheet (confirm offers first frame or a frame number only). |
| F1 review | Opus 2026-11-03, `scratchpad/cast_fresh/out/sheet.png` looked at (before/after identical). **Accepted for hand testing.** The user's 66 Cast characters already had 1 key per symbol layer; only their stage timelines were 1440 frames long, which Start Fresh cuts to 1. The injected-keys run proves the clear and that mouth/eyelid charts are kept, but not that a wrong kept pose would be caught. Notes: picker detection is a heuristic (SingleFrame/first_frame ≠ 0, or names with mouth/lid/hand/viseme); `_cast_backup` shows in the Assets folder tree; keep-pose is a frame number, not a named pose; orphan half/shut eye symbols left in the library | Opus review | accepted with notes |
