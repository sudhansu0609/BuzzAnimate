# Documents: title shows the file name, several open documents, multi-file import (2026-09-27)

User: "When I save a file with a name, the name of the file shall show on top;
if it's not saved it shall show Untitled, as in Animate. Also I shall be able
to open more than one file at a time, also import more than one file at a
time."

## Status

| # | Item | Status |
|---|---|---|
| D1 | Title shows the saved file's name (repro first) | DONE — see below |
| D2 | Several documents open at once, as document tabs | QUEUED |
| D2b | Several documents shown side by side (tiled stages), incl. several .fla opened at once | QUEUED (after D2) |
| D3 | Import several files at once | DONE — see below |

## D1 — title (repro first)

What the code already does (Opus, read 2026-09-27): `Document::title()` =
stem of `path`, else stem of `origin` (an imported .fla), else "Untitled";
`display_name()` adds `*` when dirty. Both the in-window title
(`app.rs title_drag_zone`, ~3380) and the OS title (`"{display_name} -
BuzzAnimate"`, ~9919) use it. `save_to` → `Document::save_as` sets `self.path`.
So on paper Save should retitle. **Find why the user still sees "Untitled"**:
- write a headless App test: new doc → `save_to(tmp/"Scene A.buzz")` →
  `doc.display_name() == "Scene A"` and the drawn title text (probe the
  painter/text or the `last_window_title`) == "Scene A - BuzzAnimate";
- check every save route: Ctrl+S on a new doc (`ask_for_path` → `Pick::SaveAs`
  completion handler; does it call `save_to` on the *same* document?), Save
  As, save after opening a .fla (origin), autosave recovery
  (`reset_to_untitled`), the multi-scene path;
- check whether the title bar row is only repainted on input, and whether a
  cached title string (`last_window_title`, the title row) goes stale.
Fix the real cause; keep the test. Animate shows `name.fla` in the document
tab; here the title shows `name` (stem), with the `.buzz` extension added in
the document tab (D2).

### D1 — done (2026-09-27)

**Data was never the bug.** `Document::save_as` / `save_to` retitle the
document correctly on every route read (Ctrl+S, Save As, an import's
origin) — a headless test driving `save_to` directly confirmed
`display_name()` flips right away.

**Root cause: a starved repaint, exactly the suspected mechanism.**
`app.rs::build_ui` draws the title row (`title_bar_row` → `title_drag_zone`)
near the *top* of the frame, and only *after* the rest of the frame is built
does it call `poll_picker()` / `poll_tasks()` — the code that actually
applies an async Save As or Open. So the frame that a completed Save As
lands on paints the title from the *pre-save* document; that alone is a
one-frame lag, harmless in an immediate-mode UI, ordinarily invisible
because *something* asks for another frame right after. The actual bug:
once the picker (and every task) goes idle, nothing does. The existing
`if !self.exports.is_idle() || !self.tasks.is_empty() || self.picker.busy()
{ request_repaint_after(100ms) }` only fires while something is *still*
busy — the instant a Save As resolves and nothing else is running, that
condition is false, no repaint is requested at all (confirmed with
`egui::Context::run_ui`'s own `FullOutput.viewport_output[ROOT].repaint_delay`,
which the app's real render loop reads to decide whether to wake up again —
`Duration::MAX` means "never"). egui then goes fully idle, and the stale
"Untitled" the last frame painted sits on screen until unrelated input
(a mouse move, a keystroke) forces a redraw — precisely "the title bar row
is only repainted on input".

Fix (`app.rs`, in `build_ui`, right before the existing
`request_repaint_after` check): note whether the picker or the task queue
was busy *before* this frame's polls ran, and if either just went idle,
`ctx.request_repaint()` (immediate, not `_after`) on top of the existing
condition. Applied to both `picker` and the new `picker_many` (D3).

Test: `app::tests::saving_as_asks_for_a_repaint_so_the_title_does_not_go_stale`
(`crates/buzz-app/src/app.rs`) — resolves a `Pending::resolved(Pick::SaveAs,
…)` (new `#[cfg(test)]` constructor on `dialogs::Pending`) mid-`build_ui`,
after settling the Tools column's own scrollbar-flash animation on real time
(warm-up frames; a raw `RawInput::default()` loop with no elapsed time never
lets that settle, and would otherwise mask the very signal this test checks
— confirmed by bisecting `build_ui` panel by panel). Fails without the fix
(confirmed by temporarily reverting it), passes with it.

Files: `crates/buzz-app/src/app.rs` (`build_ui`'s poll block),
`crates/buzz-app/src/dialogs.rs` (`Pending::resolved`, test-only).

## D2 — several documents open (document tabs, as Animate)

- `App` holds `documents: Vec<OpenDocument>`, one active. `OpenDocument` =
  the `Document` + its per-document view state (camera/zoom/pan,
  current_frame, selection, active layer, scene index, timeline scroll,
  sequence strip state, light/camera selection). The global state (workspace,
  tool, style/brush, panels) stays shared, as in Animate.
- Switching tabs swaps the active document in and out of the `Editor` (there
  is already `Editor::adopt(doc)`; add `take_document()` / view-state
  save/restore) — or refactor Editor's per-doc fields into a struct. Choose the
  smaller safe change and document it.
- A **document tab strip** above the stage (Animate's): one tab per open
  document showing `name.buzz` (or `Untitled-N`, numbered like Animate) plus
  `*` when dirty, a close ×, middle-click to close, the active tab highlighted.
  Ctrl+Tab / Ctrl+Shift+Tab cycle.
- File ▸ Open opens in a **new tab** (a multi-select dialog opens several);
  re-opening an already-open path switches to its tab. File ▸ New opens a new
  tab. Close (Ctrl+W) closes the active tab and asks to save if dirty; quitting
  asks for every dirty document.
- Autosave and the crash guard cover **every** open document (each has its
  own recovery file already via `Document`'s autosave; make sure background
  tabs still tick and are remembered for crash).
- Per-document caches (thumbnails, waveforms, draw caches, sound bank) must be
  keyed by document or reset on switch, so one tab never shows another's
  artwork.
- Tests: open two files → two tabs, switching preserves each one's frame,
  zoom and selection; editing tab A never dirties tab B; closing a dirty tab
  prompts; re-open focuses the existing tab; undo stays per document.

## D2b — side by side (user, 2026-09-27)

> "I want to open more than one Animate FLA file at once, as well all of them
> show up on screen side by side."

- **Open accepts several files in one go, any mix of `.buzz`, `.fla`, `.xfl`,
  `.swf`** (the formats Open already handles). Each becomes its own document.
  A multi-FLA open imports them **in parallel on the job pool**, with progress
  per file in the Tasks panel; tabs appear as each finishes.
- **Tile view**: *Window ▸ Arrange ▸ Tile Side by Side / Tile Grid / Tabs*.
  The stage area splits into one **viewport per open document** (2 → left and
  right, 3–4 → 2×2 grid; more than 4 → grid with scrolling or the first four
  plus tabs). Each viewport has a slim header: the document name + `*`, and a
  close button. Every viewport keeps its own zoom, pan and current frame.
- **One focused viewport** (accent outline). Clicking into a viewport focuses
  it, and the Timeline, Layers, Properties, Library and Sequence panels show
  the focused document. Tools act on the focused viewport. Scrolling or panning
  a viewport doesn't focus it.
- **Drawing cost**: one shared `GpuContext`, **one `DrawCache` per document**.
  A viewport that didn't change (no edit, no playback, no pan) reuses its last
  rendered texture instead of re-encoding; only the focused viewport and any
  playing one redraw every frame. Lay this out so the VRAM plan's adaptive
  buffers (`docs/VRAM_ADAPTIVE_BUFFERS_PLAN.md`) still apply: the buffers are
  shared, not per viewport.
- *Optional, later:* play all tiled viewports together (sync playheads), and
  copy/drag layers or symbols between side-by-side documents.
- Tests: open 3 files → 3 documents, tile arrangement gives 3 viewport rects
  covering the stage area without overlap; focusing viewport 2 switches the
  panels' document; an edit in one leaves the others clean; an unchanged
  viewport is not re-encoded (counter on the draw cache).

## D3 — import several files at once

- The Import dialogs (`app.rs import_dialog` ~9233, `dialogs.rs` `Ask`)
  switch to `pick_files` (multi-select) for Import to Stage / Import to
  Library / Import Sound / Import Image.
- Stage: several images land on the stage staggered (or on consecutive
  keyframes if the user holds Shift? no, keep it simple: staggered, each its
  own object). Library: all added. Several .fla/.swf/.xfl: each merged in turn,
  one undo step per file (or one for the batch; say which). Sounds: each on its
  own layer. Image sequences keep their existing dedicated route.
- One status line summarising (`Imported 5 files, 1 failed: name (reason)`);
  a failure does not stop the rest.
- Tests: importing 3 images to the library adds 3 items; one bad file among
  good ones reports the failure and imports the rest.

### D3 — done (2026-09-27)

- **Multi-select added** without touching every existing single-file picker:
  `dialogs.rs` gained `Request::open_files()` (built the same as
  `open_file()`) and `PendingMany<P>` — the same shape as `Pending<P>`, for a
  dialog that answers with `Vec<PathBuf>` (`rfd`'s `pick_files()`, via a new
  `Request::show_many`). `App` holds a second picker, `picker_many`, polled
  in `build_ui` next to `poll_picker()`. `import_dialog` (Import to
  Stage/Library), `import_image_dialog` and `import_sound_dialog` now ask
  through it; every other dialog (Open, Save As, Fill With Image, video
  reference, captions, …) is untouched.
- **Stage / Library (`.fla`/`.xfl`/`.swf`/images/sounds mixed in the "import
  anything" filter):** `start_batch_import` queues every path
  (`import_queue: VecDeque<(ImportTarget, PathBuf)>`) and merges **one at a
  time** — `advance_import_queue` starts the next only once `finish_merge`
  (or a read/merge failure) reports the previous one done, so only one
  read-and-merge pipeline is ever in flight. **One undo step per file**
  (chosen over one for the whole batch: each file already went through its
  own `Document::edit("Import", …)`, so this fell out of reusing the
  existing single-file pipeline unchanged rather than needing a new
  mechanism). A file that is audio goes to `import_sound_from` as before —
  synchronous, so that branch now also advances the queue itself, or a
  batch of mixed types would stall after the first sound.
- **Import Image:** `import_images_from` places each on the stage, each its
  own object, offset from the last by a small diagonal `stagger_offset` (24pt
  steps) so a batch does not land stacked exactly on top of itself — a
  single file is unaffected (delegates straight to the unchanged
  `import_image_from`/`Editor::import_image`). New `Editor::import_image_at`
  is `import_image`'s body with the placement offset made explicit.
- **Import Sound:** `import_sounds_from` loops `Editor::import_sound`, which
  already calls `add_stage_layer` per call — "each on its own layer" needed
  no change there, only the loop and the summary line.
- **One status line:** `App::import_batch_summary(total, failed)` —
  `"Imported {total} files"` or `"Imported {total} files, {n} failed: name
  (reason), …"` — shared by the image/sound loops and by
  `finish_import_batch` (the merge queue's own version, fed from
  `ImportBatch { total, failed }`). A single file never sees this: it keeps
  its own existing one-line message, unchanged.
- **Bug found and fixed along the way, in `Editor::import_image`
  (`editor.rs`):** a failed decode calls `self.doc.undo()` unconditionally,
  on the assumption stated in its own comment that "the failed `add_image`
  left the revision bumped" — but `add_image` only bumps the revision in
  `images_mut()`, reached *after* `ImageAsset::decode` succeeds; a decode
  *failure* never touches it. So `Document::edit` recorded no undo step for
  that call at all, and the unconditional `undo()` afterwards popped
  whichever *real* edit came immediately before it instead. Invisible with a
  single import (nothing precedes it to lose); a multi-file batch is the
  first place in the codebase that imports two images back to back, and hit
  it immediately (one bad file ate the good one right before it). Fixed by
  only calling `undo()` when the revision actually changed.
- Tests (`crates/buzz-app/src/app.rs`, `app::tests`):
  `importing_three_images_at_once_adds_three_to_the_library`,
  `one_bad_image_among_good_ones_reports_the_failure_and_imports_the_rest`,
  `a_batch_of_images_lands_staggered_not_stacked`. Existing
  `crates/buzz-app/tests/bitmap_tools.rs` (including
  `a_file_that_is_not_a_picture_is_refused_cleanly`) still pass, confirming
  the `undo()` fix does not change single-file behaviour.
- Files: `crates/buzz-app/src/dialogs.rs` (`Request::open_files`/`show_many`,
  `PendingMany`), `crates/buzz-app/src/app.rs` (`picker_many`, `import_queue`,
  `import_batch`/`ImportBatch`, `ask_for_paths`, `poll_picker_many`,
  `start_batch_import`, `advance_import_queue`, `finish_import_batch`,
  `import_images_from`, `import_sounds_from`, `import_batch_summary`,
  `stagger_offset`), `crates/buzz-app/src/editor.rs`
  (`import_image`/`import_image_at`).
- Not touched, on purpose: image sequences (`sequence_folder_dialog`, its
  own dedicated route, per spec) and `fill_with_image_dialog` (a single fill
  target, not listed among D3's four dialogs).

## Review log

- 2026-09-27 Opus, D1+D3: accepted. D1 root cause: title drawn before the async Save As/Open was polled, and nothing scheduled another frame once the picker went idle, so the old title stayed until unrelated input; fix = immediate repaint on busy→idle. Repro test fails with the fix reverted. D3 multi-select import (stage/library/sound/image), one undo per file; also fixed a latent bug where a failed image import undid the previous edit. app:: 72 pass, dialogs 7, buzz-doc 213, bitmap_tools 20.
