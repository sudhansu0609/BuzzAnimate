# AI actions without the AI panel (2026-11-02)

User: "when I do some AI action I do not want the whole AI panel to open up with
all settings etc, it's taking space; just the action shall happen with a progress
bar or its window shall open up."

Today every AI command calls `workspace.reveal(PanelId::Ai)`, which docks the
whole panel (assistant, models, character, rig, scene, story, identify,
transcribe sections) and squeezes the stage.

## Rule

* An AI menu command / shortcut **never reveals the AI panel**. The panel opens
  only through Window ▸ AI (Ctrl+Alt+I) or the voice push-to-talk (unchanged).
* A command that just runs (Split Picture into Layers, Remove Object, Rig Layers,
  Scene from Image's analysis, Direct Story's parse, Auto-Rig's analysis,
  Transcribe) shows a **small floating progress card** at the bottom-right of the
  stage: task name, progress bar (animated when the length is unknown), elapsed
  seconds, Stop when cancellable. It goes away when the task ends; the result
  goes to the status line as today. More than one running task = one card,
  one row each (reuse `tasks_panel::running_row`'s look). The card is not drawn
  while the Tasks panel is open (it would be a copy).
* A command that needs the user's input (settings or a review) opens **its own
  compact window** (`egui::Window`, closable, remembers its position), drawing
  the same body the panel section draws:
  * AI Models… → `ai_models_dialog` window (already exists; currently suppressed
    while the panel is open — keep that suppression only when the panel is open).
  * Character from Image… → `ai_character_dialog` window (exists, same rule).
  * Identify Assets → the rename window (exists, same rule).
  * Scene from Image review → window "Scene from Image" with
    `ai_panel::section_scene`'s body.
  * Direct Story review → window "Direct Story" with `section_story`'s body.
  * Auto-Rig mapping review → window "Auto-Rig" with `section_auto_rig`'s body.
  * Transcript review → window "Transcript" with `section_transcribe`'s body.
  When the AI panel is already open, results keep going into the panel as today
  (no second copy in a window).
* Results that were shown only because `ai_from_panel` held the task id keep
  that behaviour (the panel asked, the panel shows the answer).

## Status

| Step | What | Who | Status |
|---|---|---|---|
| Q1 | Remove the `reveal(PanelId::Ai)` calls on AI commands and AI results (app.rs dispatch ~8697–8715, results ~10372–10416, editor.rs ~6071–6080); add the progress card; add the four review windows reusing the section bodies (factor each section body into a `pub fn …_body` if the section wrapper gets in the way); route window actions through the same `AiPanelAction` handling the panel uses. Existing tests that assert the panel is revealed by a command are updated to assert it is NOT revealed and the window/card is shown instead. Gate: headless App test (no real mouse) for Split Picture (card shown, panel closed), Scene from Image review (window open, panel closed), Auto-Rig review, Direct Story; `cargo build --release -p buzz-app --bin buzzanimate` clean; `cargo test -p buzz-ui` and the touched buzz-app tests pass | Sonnet → Opus review | built, awaiting review: reveal calls removed (app.rs dispatch+results, editor.rs); `ai_panel::ai_progress_card` (bottom-right, one `running_row` per AI task, hidden when panel or Tasks open) and `ai_review_windows` (Scene from Image / Direct Story / Auto-Rig / Transcript windows over the section bodies, shown when a review waits and the panel is closed, closing drops the review); actions go through `ai_panel_action`. Tests: `cargo test -p buzz-ui` lib 377 pass (tests/dock_columns has 3 layout failures, not touched by Q1); buzz-app `ai_commands_never_reveal_the_ai_panel_and_it_draws_through_the_ui`, `quiet_ai_actions_show_a_card_and_review_windows_not_the_panel`, `a_transcript_applies_as_captions_from_the_ai_panel` pass. Note: a menu Scene from Image commits at once (no review), so its window only shows for a panel-started run whose panel was closed. |
| Q1 review | Opus 2026-11-02: grep confirms no `reveal(PanelId::Ai)` left outside tests and voice_bridge; card and windows called from `build_ui` (app.rs ~4066/4072). **Accepted for hand testing.** Notes: Stop and window buttons not clicked in the test; closing a review window leaves the app-side pending data until the next run; buzz-ui `tests/dock_columns` 3 layout failures not shown to come from Q1 (Q1 touched only ai_panel.rs/tasks_panel.rs) — triage separately | Opus review | accepted with notes |
| Q2 | (user 2026-11-03: "when it is tracing or doing any operation a progress bar shall be shown in mid screen that can be moved to the side") The Q1 card becomes a **movable progress window for every long operation**, not only AI: Trace Bitmap (TaskKind::Trace), Even Out Lines, Optimize Shapes, Split Picture, Remove Object, Generate Background, Lip Sync All, Add Life, export/render, sound decode, any `TaskKind`. It opens **centred on the stage** the first time, is draggable by its title bar, and **remembers where the user dropped it** (workspace setting, per session and saved); one row per running task (name, what it is doing now, progress bar — animated when unknown — elapsed seconds, Stop when cancellable); closes itself when nothing runs; a small "hide" (—) collapses it to a slim bar it can be restored from; still shown when the Tasks panel is open only if that panel is not visible on screen. Operations that run on the UI thread today and freeze the window while working must report into it too (move them off-thread where cheap; otherwise at least show the window before starting). Gate: headless App tests — trace starts → window centred on the stage; move it → next task opens where it was left; two tasks → two rows; hide/restore | Sonnet → Opus review | built, awaiting review: `ai_panel::progress_window` replaces the Q1 card (draggable egui Window, centred on the stage first time, drop saved in `Workspace.progress_window_pos`, one `running_row` per task, dash hides to a slim bar, closes when idle). `App::progress_rows` = every TaskKind except Thumbnails/AssetScan/Warm, plus a Sound row while `sound_decode` runs; Tasks panel open suppresses it. Trace, Even Out Lines, Optimize, Split Picture, Remove Object, Generate Background, Lip Sync All, Export already ran as tasks (off-thread). Add Life (`scene_life::add_life`) still runs on the UI thread. Test: `the_progress_window_is_centred_draggable_remembered_and_one_row_per_task`. |
| Q2 review | Opus 2026-11-03: **accepted for hand testing.** "Working" window for every long task kind (trace, even lines, optimize, split, remove, generate background, lip sync all, export, open/import, sound decode), centred on the stage first, drop position saved in the workspace, one row per task with Stop, "—" hides to a slim bar, nothing when idle; headless test covers centre/drag/remember/rows/hide; Q1 tests and buzz-ui lib (377) pass. Notes: Add Life still runs on the UI thread (one edit, unmeasured); hidden while the Tasks panel is open; sound row shows 0 s elapsed; drag end detected by window move + release (egui drag_stopped did not fire) | Opus review | accepted with notes |
