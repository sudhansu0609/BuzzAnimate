# Plan: document name, menus in the title bar, Test Movie, dockable Properties section

Planned by Opus (architect), 2026-09-23. Ground rules:
- Do not run `cargo fmt`.
- Commands go through `buzz_ui::command::Command` → `App::dispatch`
  (app.rs:5374) → `Editor::run`.
- Headless tests use the `Drive` harness pattern in app.rs:9666-9758
  (`App::new(GpuPreference::Automatic)`, `egui::Context::default()`,
  `ctx.run_ui(input, |ui| app.build_ui(ui))`). Never use the real mouse.

**Decisions taken for the user's open questions** (the user may override them):
- Ctrl+Enter (Cmd+Enter) is **Test Movie** everywhere except while typing in
  the Actions code editor, where it still runs the script.
- A Windows custom title bar in v1 accepts the loss of the Windows 11 Snap
  Layouts hover flyout on Maximise. Drag-to-edge snapping and Win+arrow still
  work. A `WM_NCHITTEST` subclass is a follow-up.
- Test Movie in v1 plays sound for the active scene only.
- The title shows the stem only (`bhatta2`).

**Order of work:** item 1, then 4, then 3, then 2. Item 2 is the window-level
change, so it goes last and can be reverted on its own. Verify each with
`cargo test -p buzz-doc -p buzz-ui -p buzz-app`.

---
## 1. Document name

### Findings
- `Document` (buzz-doc/src/lib.rs:44) has only `path: Option<PathBuf>`.
  `display_name()` (lib.rs:309-321) returns the file name with its extension,
  or `Untitled`, plus `*`.
- An imported document is built with `Document::new(imported.scene)` and no
  path, in `finish_open_imported` (app.rs:7294-7297) and at startup
  (main.rs:112). So a .fla opens as `Untitled`. That is intentional, so that
  Save cannot overwrite the .fla (app.rs:7267-7273).
- The name appears only in the status bar (`status_bar`, app.rs:4919). The OS
  title is a fixed `BuzzAnimate` (app.rs:966); `set_title` is never called.
- Save (app.rs:7318) already shows a .buzz Save As dialog when `path()` is
  None, suggesting `untitled.buzz`.
- Other places also fall back to `Untitled`: `save_snapshot`
  (app.rs:5421-5426), `export_fla_dialog` (app.rs:7165-7170) and the Save As
  suggestion (7323-7329).

### Decision
Add an **origin** to `Document`: the file it was translated from, separate
from its save `path`. The title is the stem of the path, else the stem of the
origin, else `Untitled`. Save on an imported document keeps the no-overwrite
rule, but suggests `<stem>.buzz` in the .fla's folder. After that save the
document has a `path`, so Ctrl+S is silent from then on.

### Steps
1. buzz-doc/src/lib.rs `struct Document`:
   - add `origin: Option<PathBuf>`, set to None in `new` and `from_scenes`;
   - add `with_origin(self, p)`, `origin()`, and
     `title() -> String` (the stem of path, else the stem of origin, else
     `Untitled`);
   - `display_name` becomes `title()` plus `*`;
   - `forget_path` also clears `origin`; `save_as` leaves it alone;
   - update the tests that expect `name.buzz` from `display_name`.
2. app.rs `finish_open_imported` (7297), and main.rs:112:
   `Document::new(..).with_origin(path)`.
3. app.rs: add `fn save_request(&self) -> crate::dialogs::Request`, used by
   `save` (7318):
   - the file name is `format!("{}.{}", doc.title(), buzz_doc::EXTENSION)`;
   - add `.directory(origin.parent())` when `path` is None and `origin` is
     Some;
   - add `#[cfg(test)] suggested_file_name()` and `suggested_directory()` to
     `dialogs.rs` `Request` (its builders are at 104-112).
4. The snapshot (5421-5426) and Export .fla (7165-7170) fallbacks use
   `self.editor.doc.title()`.
5. OS title: add `last_window_title: String` to `App`. In `render` (7484),
   after `run_ui`, set the title to `"{display_name} - BuzzAnimate"` whenever
   it changes.
6. Tests:
   - app.rs `opening_an_import_takes_its_name`: build an `import::Imported`
     from an empty scene and call `finish_open_imported("C:/x/bhatta2.fla", ..)`.
     Then `title()=="bhatta2"`, `path()` is None, and the suggested save is
     `bhatta2.buzz` in `C:/x`.
   - buzz-doc: `origin_names_an_unsaved_document` and
     `saved_path_wins_over_origin`.

---
## 2. Menus in the title bar

### Findings
- The bar with the icon and name is the **OS title bar**: native decorations
  (app.rs:965-995, plus `with_taskbar_icon` on Windows).
- The menu is its own egui row: `egui::Panel::top("menu")` (app.rs:1620-1636)
  calls `panels::menu_bar` (panels.rs:70), which wraps `egui::MenuBar`.
- `render` passes only `platform_output` on (7586) and reads `viewport_output`
  just for `repaint_delay`. egui `ViewportCommand`s are **ignored**, so window
  actions must be wired explicitly.
- Versions: winit 0.30.13; egui, egui-winit and egui-wgpu 0.35.

### Decision
Draw a custom title bar. The mode is `enum TitleBar { Custom, MacInset, Native }`,
stored on `App`:
- **Windows = Custom**: `with_decorations(false)` plus
  `WindowAttributesExtWindows::with_undecorated_shadow(true)`. The app draws
  the drag area and the minimise, maximise and close buttons, and handles
  resizing itself.
- **macOS = MacInset**:
  `with_titlebar_transparent(true).with_fullsize_content_view(true).with_title_hidden(true)`.
  The traffic lights stay native, with a 78 pt left inset and a bar at least
  28 pt tall.
- **Other Unix = Native**: nothing changes, except that the menu row shows the
  document name.

Window actions go through an App-owned queue that `render` drains, which keeps
them testable.

### Steps
1. The window attributes (app.rs:965):
   - on Windows, add `.with_decorations(false)`, and
     `.with_undecorated_shadow(true)` in the existing ext block (991-995);
   - on macOS, an ext block with the three setters;
   - `fn title_bar_mode() -> TitleBar`, chosen by cfg and stored as
     `App.title_bar`.
2. New `crates/buzz-app/src/title_bar.rs`, added as `mod title_bar;` in lib.rs:
   - `pub enum TitleBar`;
   - `pub enum WindowRequest { StartDrag, ToggleMaximize, Minimize, Close, Resize(winit::window::ResizeDirection) }`;
   - `#[derive(Default)] pub struct TitleBarRects { drag, minimize, maximize, close }`;
   - a pure `pub fn resize_direction(pos, size, border_px) -> Option<ResizeDirection>`
     that tests corners before edges.
3. `build_ui`: the body of `Panel::top("menu")` becomes
   `self.title_bar_row(ui, &mut commands, &template_names, can_undo, can_redo)`,
   laid out in `ui.horizontal` with `exact_size(32.0)` when the mode isn't
   Native. From left to right:
   - **MacInset**: `add_space(78)`.
   - **Custom**: the logo at 18 pt. `assets/logo-32.png` becomes a lazily
     made `egui::TextureHandle`, reusing the PNG decode factored out of
     `icon_from_png` (app.rs:144) as
     `decode_png_rgba(bytes) -> Option<(Vec<u8>,u32,u32)>`.
   - The unchanged `panels::menu_bar(..)`.
   - The rest of the width is a click-and-drag zone with `doc.display_name()`
     centred, stored in `title_rects.drag`. `drag_started` pushes `StartDrag`;
     `double_clicked` pushes `ToggleMaximize`.
   - **Custom**: three 46×32 buttons on the right, placed first with
     `right_to_left` so the menu cannot cover them (see the overflow note at
     app.rs:8668-8674). The glyphs are painted, the Close button hovers red,
     and each button's rect is recorded.
4. `App` gets `window_requests: Vec<WindowRequest>`, `title_rects` and
   `window_maximized`. Drain the requests in `render` after `run_ui`
   (~7521-7530):
   - `StartDrag` → `drag_window()`;
   - `ToggleMaximize` → `set_maximized(!is_maximized())`;
   - `Minimize` → `set_minimized(true)`;
   - `Close` → `self.request_quit()` (the `CloseRequested` path, app.rs:8209);
   - `Resize(d)` → `drag_resize_window(d)`.
5. Resize borders (Custom only), in `window_event` (8194):
   - track `last_cursor` on `CursorMoved`;
   - when not maximised, compute `resize_direction(pos, inner_size, 6*scale_factor)`
     and set the resize cursor;
   - on a left-button press with a direction, call `drag_resize_window` and
     **don't** forward the press to egui (move the `on_window_event` call at
     8199 below that early return);
   - on `Resized`, update `window_maximized`; the maximise glyph becomes
     Restore.
6. `mod title_bar_tests` in app.rs (the `Drive` pattern, with
   `title_bar = Custom`):
   - `menus_share_the_title_row` (`title_rects.drag.top()==0`, and the stage
     area starts at 32 or less);
   - `close_button_asks_to_quit`;
   - `double_click_on_the_bar_toggles_maximize`;
   - `dragging_the_bar_starts_a_window_drag`;
   - `resize_direction` unit tests (corners, edges, interior, scale 1.5);
   - `native_mode_draws_no_window_buttons`.

### Risks
- Snap Layouts: see the decision above.
- An undecorated window maximised through winit must respect the taskbar;
  check on a real machine.
- The border is in physical pixels; recheck after `ScaleFactorChanged`.
- Don't use `ctx.send_viewport_cmd`.

---
## 3. Test Movie (Control menu; Ctrl+Enter / Cmd+Enter)

### Findings
- The Control menu exists (panels.rs:407-450).
- **Shortcut conflict:** `RunScript` is already Ctrl+Enter (command.rs:707),
  consumed by `script_shortcuts` (app.rs:1378-1388), and exempt from the
  reachability test (app.rs:9474).
- There is already an export-look renderer: `Preview::draw` (preview.rs:182-268),
  with `FrameOptions{lit:true, guides:false, masks:Always, cull}`,
  `scene.camera_transform(frame)`, and `register_native_texture`. GPU work
  happens in `render` (`self.preview.fulfil`, app.rs:8001-8010).
- The film is `self.editor.doc.film()` (app.rs:6427).
  `buzz_export::Reel` gives `of`, `frames()` and `at_clamped`
  (reel.rs:63-113). Its `starts` field is private.
- Audio: `SoundBank::play/stop/seek/playing_frame` (sound.rs:256-294), with
  the audio-clock pattern in `Editor::advance_playback` (editor.rs:1211-1247).

### Decision
Test Movie is an **in-window modal overlay**: a full-window `egui::Area` at
`Order::Foreground` over a dark scrim. It renders through the **same code as
the export preview**, factored into a shared function. It plays the whole film,
loops, and follows the audio clock while sound plays. Esc, Ctrl+Enter or a
Close button ends it.

### Steps
1. command.rs:
   - add `TestMovie`, labelled "Test Movie", with shortcut `sc(ctrl, Key::Enter)`;
   - add it to the palette and ALL lists;
   - keep `RunScript` on the same chord, and update the duplicate-binding test
     (app.rs:9494) to allow this one shared chord, with the reason.
2. panels.rs:407: put `item(ui, Command::TestMovie, true, &mut raised); ui.separator();`
   at the top of the Control menu.
3. buzz-ui actions_panel: `pub const CODE_EDITOR_ID: &str`, used as the
   TextEdit id (grep for `TextEdit`).
4. app.rs `script_shortcuts` (1378): on Ctrl+Enter, if the focused id is the
   code editor, push `RunScript`; otherwise push `TestMovie`. Consume the key
   once. Add `TestMovie` to `handled_elsewhere` (9474).
5. preview.rs: extract lines 194-256 of `Preview::draw` into
   `pub(crate) fn render_export_frame(gpu, vello, scene, frame, width, height, cache, view) -> Option<()>`,
   and have `Preview::draw` call it.
6. buzz-export/src/reel.rs: `pub fn start_of(&self, scene_index: usize) -> Option<u32>`.
7. New `crates/buzz-app/src/test_movie.rs`, `pub struct TestMovie`:
   - fields: `scenes`, `active_index`, `frame`, `fps` (from `Reel::lead`),
     `accumulator`, `size_wanted`,
     `target: Option<(Texture, TextureView, TextureId, u32, u32)>`,
     `retired: Vec<TextureId>`, `drawn: Option<u32>`;
   - `open(scenes, active_index) -> Option<Self>`;
   - `advance(elapsed, audio_frame: Option<u32>)`, which follows the audio
     inside the active scene, otherwise the accumulator as at
     editor.rs:1249-1293, and always loops;
   - `frame()`, `frames()`, `needs_render()`;
   - `set_view_size(px_w, px_h)`, fitted to the stage aspect with the long edge
     capped at 1920;
   - `texture()`;
   - `fulfil(gpu, egui_renderer, vello, cache)`, which recreates the texture
     only when the size changes, uses `Reel::at_clamped`, and calls
     `render_export_frame`;
   - `close(self) -> Vec<TextureId>`.
8. app.rs:
   - add `test_movie: Option<TestMovie>` and `test_movie_retired`;
   - `dispatch(TestMovie)` toggles it. On open: stop editor playback, stop the
     sound, open the movie, and play the sound from 0. On close: stop the
     sound and retire the textures;
   - in `render`, before `run_ui`, advance the movie and skip
     `editor.advance_playback` while it is open;
   - next to `preview.fulfil`, fulfil the movie when `needs_render()`, then
     request a redraw, and free the retired textures;
   - `wants_frame` returns `Redraw::Now` while the movie is open.
9. app.rs `test_movie_overlay(ctx)`, called after `import_report_window`
   (7524) and at the end of `build_ui`, drawn once per frame:
   - a scrim, and the picture at the stage aspect (or "Rendering…" until the
     first frame lands);
   - the caption "Test Movie - <title> - frame N/M" and a Close button;
   - Esc is consumed and closes the movie;
   - stage input is blocked while the movie is open.
10. Tests:
    - `test_movie_advances_and_loops`;
    - `test_movie_follows_the_audio_clock`;
    - `ctrl_enter_opens_test_movie_outside_the_script_editor`;
    - `ctrl_enter_runs_the_script_when_the_code_editor_has_focus`;
    - `escape_closes_test_movie`;
    - `control_menu_lists_test_movie`;
    - optional, GPU-gated: `render_export_frame` equals `Exporter::render` on
      frame 0.

### Risks
- Sound plays for the active scene only (see the decision above).
- Full-size frames rendered on the UI thread may drop frames on big
  documents. The long edge is capped at 1920, and the audio clock keeps sync.

---
## 4. Floatable, dockable Properties section

### Findings
- The dock model already has what this needs:
  - `PanelId::Properties/Filters/ToolOptions` share `GROUP_INSPECTOR`
    (workspace.rs:862-888);
  - Library, Assets and Cast share `GROUP_ASSETS` on `Dock::RightOuter`
    (962-993);
  - `group_with` (1231), `dock_at` (1477), `float_at` (1502) and `move_to`
    (1519);
  - `reset_layout` restores `animate()` (808-819);
  - persistence is `workspace.save()` inside `DockRequests::apply`
    (app.rs:8611-8650).
- Dragging a single tab onto the Library section already works
  (`drag_docking`, app.rs:8494-8602; test at app.rs:9762).
- **Gaps:**
  - a) The section menu (8721-8826) acts on `section.front` only. `move_to`
    gives that panel a fresh group (1534), so Float on the inspector tears off
    only the front tab.
  - b) A grip-drag of a whole section to `DropTarget::Float` or `Column` pushes
    one request per panel, which splits the section.
  - c) A `DropTarget::Section` drop groups the panels in reverse order.
  - d) "Group With" only offers neighbours on the same side, so it can't
    reach the Library on `RightOuter`.
  - e) A floating window's Dock button sends `front` to `Dock::Right`, not to
    its home.

### Decision
Make every dock operation section-level in `Workspace`, and route both the
menu and drag-and-drop through it. No new layout fields are needed, so
persistence and Reset Layout keep working as they are.

### Steps
1. workspace.rs, each a no-op when `locked`:
   - `section_members(id) -> Vec<PanelId>`: same dock and group, ordered by
     `order`, or `[id]` when hidden;
   - `move_section(id, dock)`: `move_to(first)`, then `group_with` each later
     member after the previous one, then `select_tab(original front)`;
   - `float_section_at(id, pos)`: the same chaining, with `float_pos` and
     `float_size` copied to every member;
   - `dock_section_at(id, dock, before)`;
   - `group_section_with(id, target)`: chain after the target, then restore
     the front.
2. app.rs `DockRequests` (8297): add `section_moves`, `section_floats`,
   `section_docks` and `section_groups`, applied in `apply` before the
   per-panel lists and included in `touched`.
3. `drag_docking` release (8579-8600): when `dragged.0.len() > 1`, push one
   section request for `dragged.0[0]`.
4. The `section_header` menu (8721-8826):
   - the `Dock::CHOICES` items push `section_moves`, plus a sub-item "Move
     Tab Only" for the old behaviour;
   - "Group With" lists every visible section front on any dock except its
     own (a new `all_fronts: &[PanelId]`, built once in `build_ui`), each
     labelled with its dock, e.g. "Library (Far Right)";
   - Float pushes `section_floats`;
   - Dock pushes `section_moves(front, home)`, falling back to Right.
5. Tests:
   - workspace:
     - `the_inspector_floats_as_one_window`;
     - `the_inspector_tabs_in_with_the_library` (all 6 panels in one section,
       `dock_of(Properties)==RightOuter`, front is Properties);
     - `a_floated_inspector_docks_back_whole`;
     - `reset_layout_brings_the_inspector_home`;
     - `a_moved_inspector_survives_save_and_load`.
   - app.rs `dock_drag_tests`:
     - `grip_dragging_the_inspector_onto_the_library_tabs_it_there`;
     - `grip_dragging_the_inspector_onto_the_stage_floats_one_window`;
     - `group_with_menu_offers_the_library` (or test the `all_fronts` helper).

### Risks
Existing tests around app.rs:9795-10060 that expect per-panel menu behaviour
may need their expectations updated where they float or move a tabbed section.

---
## Status

Implemented in the order the plan states: 1, then 4, then 3, then 2. Verified
after each item with `cargo test -p buzz-doc -p buzz-ui -p buzz-app`. Every
run below shows the same pre-existing baseline failures, confirmed
unconnected to this work (see "Deviations" for item 4 and the note under the
table): `buzz-app --lib`'s `editor::tests::dragging_a_lamp_over_a_scene_stays_interactive`,
`editor::tests::dragging_a_selection_on_a_heavy_document_stays_interactive`,
`tools::tests::dragging_from_inside_the_selection_moves_it` (and other perf-budget
tests in that module are flaky under `-j4` load; they pass alone); `buzz-ui
--lib`'s 9 `workspace::{tests,group_tests,migration_tests}` failures around
`fill_gaps`/stranded-panel migration; and `buzz-ui`'s
`real_layout_migration::a_real_saved_layout_puts_the_story_panel_back_in_view`
(same root cause). None of these touch code this plan's items changed.

| Item | State | Tests added | Deviation |
|---|---|---|---|
| 1. Document name | done | `buzz-doc`: `origin_names_an_unsaved_document`, `saved_path_wins_over_origin`; `buzz-app`: `opening_an_import_takes_its_name` | None — implemented as written. |
| 4. Floatable, dockable Properties | done | `buzz-ui`: `the_inspector_floats_as_one_window`, `the_inspector_tabs_in_with_the_library`, `a_floated_inspector_docks_back_whole`, `reset_layout_brings_the_inspector_home`, `a_moved_inspector_survives_save_and_load`; `buzz-app` `dock_drag_tests`: `grip_dragging_the_inspector_onto_the_library_tabs_it_there`, `grip_dragging_the_inspector_onto_the_stage_floats_one_window`, `group_with_menu_offers_the_library` | `section_header` gained a `workspace: &Workspace` parameter (not only the renamed `all_fronts: &[PanelId]`) so "Group With" can look up each candidate's dock for its label — the plan didn't say where that lookup would come from. The "Move Tab Only" sub-item is its own submenu repeating all six `Dock::CHOICES`, rather than a single item, so the old per-tab behaviour stays fully reachable. Updated `dropping_a_section_between_two_others_docks_it_there` (an existing `dock_drag_tests` test), which had asserted the very fragmentation bug this item fixes ("a section of its own"); now asserts the whole section lands together, per the Risks note above. |
| 3. Test Movie | done | `buzz-app`: `test_movie::{opening_an_empty_document_gives_nothing_to_play, test_movie_advances_and_loops, test_movie_follows_the_audio_clock}`, `app::tests::{ctrl_enter_opens_test_movie_outside_the_script_editor, ctrl_enter_runs_the_script_when_the_code_editor_has_focus, escape_closes_test_movie}` | `control_menu_lists_test_movie` not added: no precedent anywhere in this codebase for driving a nested `egui::menu_button` popup open-then-click headlessly, and building that infrastructure was out of this brief's scope. `Command::TestMovie`'s wiring is instead covered by `every_shortcut_is_reachable_from_the_keyboard`, the two `ctrl_enter_*` tests, and `buzz-ui`'s shortcut/palette tests; the Control-menu line itself (`item(ui, Command::TestMovie, ...)` at the top of the menu) was added exactly as the plan states. The optional GPU-gated `render_export_frame` vs `Exporter::render` equality test was skipped, as the plan allows. Also fixed a second, pre-existing copy of the shortcut-collision test the grep for "app.rs:9494" didn't find on the first pass: `buzz-app/tests/break_apart.rs::ctrl_b_maps_to_break_apart_with_no_duplicate_shortcuts` needed the same `RunScript`/`TestMovie` exception as `buzz-ui`'s `shortcuts_do_not_collide`. |
| 2. Menus in the title bar | done | `buzz-app` `title_bar_tests`: `menus_share_the_title_row`, `close_button_asks_to_quit`, `double_click_on_the_bar_toggles_maximize`, `dragging_the_bar_starts_a_window_drag`, `native_mode_draws_no_window_buttons`, `resize_direction_{leaves_the_interior_alone, finds_the_four_edges, finds_the_four_corners, prefers_corners_over_edges, scales_the_border_with_the_display}` | `TitleBarRects`' four fields are `Option<egui::Rect>`, not bare `egui::Rect` — `egui::Rect` has no `Default` impl, so the literal field list couldn't satisfy `#[derive(Default)]`; `None` also reads more honestly as "not drawn this frame" than a bogus zero rect would. Added a `bounded_menu` helper not named in the plan: `egui::MenuBar` sets its own minimum width to *all* of whatever `Ui` it is handed regardless of how many menus are in it, so without capping the menu's own width short of the full row, the drag zone was left zero width every time (`ui.available_size().x <= 0`) — caught by `menus_share_the_title_row` failing before the fix. `menu_state` is a method, not the closure the plan implies, because a closure returning something borrowed from its own argument needs a higher-ranked lifetime bound `rustc` could not infer. Not driven on a real Windows session — Snap Layouts, drag-to-edge snapping, actual window-manager resize behaviour and the macOS inset are unverified beyond `cargo check` staying green there; only what a headless `egui::Context` can exercise (layout, queued `WindowRequest`s, `resize_direction`'s pure geometry) is test-covered. |

Every item's changed crates also pass `cargo check -p buzz-app` cleanly on
this Windows target; the macOS (`MacInset`) and other-Unix (`Native`) branches
in `app.rs` and `title_bar.rs` are `#[cfg]`-gated and were not compiled here.
