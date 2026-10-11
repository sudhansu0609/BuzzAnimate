# Ecosystem bridge — BuzzAnimate ⇄ Buzzcaf Studio ⇄ BuzzEdit

Written 2026-10-11. Request (user): "make sure Buzzcaf Studio is able to access
this app, and this app is able to access BuzzEdit, and all vice versa."

## Where things stand (checked 2026-10-11)

- **BuzzAnimate has no listener.** `ECOSYSTEM.md` §2 lists it with "Ports: none".
  Dexter can launch and close it (`dexter/data/apps.default.json` id
  `buzzanimate`), and that is all: no app can ask it anything.
- **Studio → BuzzEdit already works**: `BuzzcafStudio/backend/integrations/buzzedit_client.py`,
  `produce_video` (ECOSYSTEM §7a). **BuzzEdit → Studio** works too:
  `BuzzEdit/backend/integrations/buzzcaf_client.py` (`plan_visuals`).
- Every app finds every other through the **port ledger**
  (`%LOCALAPPDATA%\Buzzcaf\ports.json`, ECOSYSTEM §3) plus an identity check on
  its health route. Helpers: Python `buzzcaf_ports.py` (copies in the Studio's
  `backend/integrations/` and in `BuzzEdit/backend/`), Rust
  `BuzzCode/crates/cb-engine/src/ports.rs` (547 lines).
- BuzzAnimate already has a complete JSON tool surface:
  `crate::agent::catalog()` and `crate::agent::execute(&mut App, &AgentCall)`
  (`crates/buzz-app/src/agent.rs`), which the Assistant uses through a
  channel to the UI thread (`agent_bridge.rs`, `App::agent_pump`). It covers
  everything the user can do by hand (`docs/AI_AGENT_PLAN.md`).
- BuzzEdit routes used here: `GET /api/health`,
  `POST /api/projects/import_path {path}` (new project from a video),
  `POST /api/projects/{id}/media/import {paths, copy}`, `GET /api/projects/list`,
  transcription under `/api/transcription`.
- Library-level episode build: `buzz_app::episode::build_episode(play, casting,
  assets, numbers, opts)`.
- All four repos (BuzzAnimate, BuzzcafStudio, BuzzEdit, dexter) carry large
  uncommitted work: **every change is additive**; nothing is reformatted,
  reverted or committed.

## Status

| Step | What | State |
|---|---|---|
| E1 | BuzzAnimate local API: ports ledger, `/health`, `/api/catalog`, `/api/call`, `/api/state`, open/save/export/jobs | **done 2026-10-11** (`src/api.rs`, `src/ports.rs`; `tiny_http =0.12.0`, `humantime =2.4.0`). Evidence: `cargo test -p buzz-app --lib -- api::tests ports::tests` 16/16 — `api::tests::the_local_api_answers_publishes_and_drives_the_document` (health identity, ledger publish + withdraw on `api_shutdown`, state/catalog, `/api/call` new_layer = one `API: command` undo step + undo via API, 403 cross-origin, 409 quit, save, png + 2-frame mp4 export jobs `done`, two-line `/api/episode` → `.buzz`, and with `render` its preview job `done`, 404/405), `api::tests::only_this_machine_may_drive_the_api`, the 14 copied `ports::tests`; `agent_bridge::` 18/18 still pass; debug + release builds pass. Not verified: the real window (no GUI run), `/api/open` (only reachable through a background load; not tested). Deviations: `/api/call` undo/redo commands are not wrapped (wrapping would undo the undo); `/api/call` refused (409) while the Assistant is running; withdraw only removes an entry whose pid is ours; png export takes frame `from` of the scene being edited; episode previews are separate export jobs listed in the episode job's `render_jobs`. |
| E1-review | Review of E1 | **accepted with three fixes, 2026-10-11.** (1) `Origin: null` was allowed (my plan said so): any website can send it from a sandboxed iframe and drive open/save/export — now refused. (2) No `Host` check: a DNS-rebinding page could read answers same-origin — `host_allowed` now serves only 127.0.0.1 / localhost / [::1] on our port. Both covered in `api::tests::only_this_machine_may_drive_the_api`. (3) A **minimised** window gets no WM_PAINT, so requests would time out and exports/episode builds stall: `about_to_wait` now runs the frame directly every 30 ms while `ApiState::has_work()` or an export is running and the window is minimised. Not testable headlessly — **E7 must verify it in the real app.** Reran `api::tests ports::tests` 16/16. No CORS headers are sent (browser pages cannot read answers). |
| E2 | BuzzAnimate → BuzzEdit client: Send to BuzzEdit (export + import), transcribe audio for lip sync | **done 2026-10-11** (`src/estate.rs`; hooks in `app.rs` (`estate` field, `estate_pump` before `poll_tasks` and around it in `agent_poll_tasks`, dialog, dispatch), `agent.rs`, `agent_bridge.rs` (tool schemas), `api.rs` (`api_queue_export`, `api_job_state`; episode build made `pub(crate)`), `editor.rs` (no-op arm), buzz-ui `command.rs` + `panels.rs`; `ureq` 2.10 `json`, already in the lockfile via buzz-ai). Menu: File ▸ Send to BuzzEdit…, Commands ▸ Transcribe with BuzzEdit… (one dialog: asks BuzzEdit first, shows **Start BuzzEdit** when it is down). Tools: `send_to_buzzedit`, `buzzedit_projects`, `transcribe_with_buzzedit` (each `{started, job}` → `wait_for_jobs`; `start:true` launches `<estate>/BuzzEdit/START_APP.vbs` and waits up to 180 s). Routes used: `GET /api/health`, `GET /api/projects/list`, `POST /api/projects/import_path {path}`, `POST /api/projects/{id}/media/import {paths, copy:true}`, `POST /api/transcription/transcribe {project_id, precut:true}` (answers when Whisper is done; followed via `GET /api/transcription/status/trans_{id}`), `GET /api/transcription/{id}/transcript`. Evidence: `cargo test -p buzz-app --lib estate::` 6/6 — `an_app_is_known_by_its_name_not_by_its_port` (a `buzzcaf` health on BuzzEdit's ledger port is refused; ledger + scan; not-running and missing-launcher messages; estate root), `send_to_buzzedit_makes_a_project_or_adds_to_one` (both call shapes, bad id never sent, and with ffmpeg present the whole tool flow: MP4 on the export queue → media/import of that file), `a_buzzedit_transcript_becomes_captions_and_lip_sync` (5 words → 2 captions + Ana's lip-sync keys, `precut:true`), `an_app_that_is_not_running_is_said_so_and_offered_a_start` (agent error says `start:true`; menu dialog lands on Not running), `words_become_lines_at_pauses_and_sentence_ends`; `api::tests ports::tests agent_bridge:: agent::jobs` still pass (42/42 with estate); `cargo test -p buzz-ui --lib` 370/370 (one parallel-run flake in `timeline_panel::shift_click_sweeps…` on the first run, passes alone and on rerun); `cargo test -p buzz-app --test ai_agent_coverage` 41/41. Deviations: extra tool `buzzedit_projects` (the agent needs the list to pick a project); the three menu commands are routed **Excluded** for the agent (they open a dialog; the tools take args); discovery is ledger → preferred..+20 with a 250 ms connect probe and the health identity (no pid check, no `<APP>_URL`); a transcribed sound becomes its own BuzzEdit project, which is left there; the speaker must be named (lip sync matches the mouth symbol by name); default MP4 is beside the document, else `~/Videos/BuzzAnimate`; tests pass a temp ledger in an `Estate` value instead of setting `BUZZCAF_PORTS_FILE` (that env var is process-wide and `api::tests` sets it). The three new commands are added to the menu walk's `REACHES_OUTSIDE` (they would ask the machine's real apps); `app::tests::every_menu_command_runs_and_the_window_still_draws` was started but stopped after ~45 min still running, so it is **not verified**. `cargo build -p buzz-app` passes. Not verified: the real BuzzEdit (fake servers only), the dialog on screen (no GUI run), launching the real `.vbs`. |
| E3 | BuzzAnimate → Studio client: list projects, import a project's script and build the episode | **done 2026-10-11** (same files as E2). Menu: File ▸ Import Script from Studio… (project picker, optional folder; Start Buzzcaf Studio when down). Tools: `studio_projects`, `studio_build_episode {project, asset?, out_dir?, start?}`. Routes used: `GET /health` (must answer `app: "buzzcaf"`), `GET /api/projects`, `GET /api/projects/{id}/asset/script` (the Studio's real route for a project file: `{content}` for text). The build is `api::build_episode_files` — the function `POST /api/episode` runs — into `<estate>/BuzzcafStudio/backend/projects/<id>/animation/` (the Studio's default `PROJECTS_PATH`; elsewhere pass `out_dir`). Evidence: `estate::tests::a_studio_script_becomes_an_episode_in_the_project_folder` (list with `has_script`, two-line script → `.buzz` under `<project>/animation`, a missing project fails with the Studio's 404) and the Studio half of the not-running test. Deviations: no casting file and no `render` (only the `.buzz` + `report.md`). Not verified: a real Studio script — the build needs `## SCENE n — TITLE` headings and nothing converts a Studio script without them. |
| E23-review | Review of E2/E3 | **accepted with one fix, 2026-10-11.** Real Studio scripts are narrated videos in three shapes (`draft.md` `## [SCENE 01: …]` + `**V.O. (Narrator):**`; `buzzedit_script.md` `# SECTION` + inline `[broll: …]` directives; `short_script.md` `# Title` + `(pause)` cues) — none has `## SCENE n —`, so `studio_build_episode` built nothing ("no scenes"). Added `estate::studio_to_screenplay` (narrator V.O. lines, direction as action, directives stripped, `(pause)`/`(beat)` → Beat.; screenplays pass through) and wired it into the Studio build. Tests `estate::studio_script_tests::*` 2/2; `estate::` 8/8. Probe (temporary, removed) built the three real project scripts: 5, 3 and 1 scenes, `.buzz` + report each, 0 warnings. With no casting the narrator is captions only — map `NARRATOR` to a Cast rig in a casting file to get a talking presenter. Known: transcription leaves a BuzzEdit project behind per run; menu-walk test still unrun (>45 min). |
| E4 | BuzzEdit → BuzzAnimate: `integrations/buzzanimate_client.py` + `/api/buzzanimate/*` routes | **done 2026-10-11** (`BuzzEdit/backend/integrations/buzzanimate_client.py`, `routes/buzzanimate.py`, 3-line `include_router` in `main.py`). Routes: `GET /api/buzzanimate/status[?start=true]`, `POST /call {tool,args,start?}`, `POST /open {buzz_path,start?}`, `POST /render {buzz_path,project_id?,format?,start?}` → `{job_id}` (open → wait for `/api/state` path → `/api/export` beside the `.buzz` → poll job → `projects.import_media` into the project), `GET /render/{job_id}`. Not running → 503 naming `start: true`; `start:true` runs `<estate>/BuzzAnimate/BuzzAnimate.vbs` (wscript, no window) and polls health (default 600 s, `start_timeout` overrides). Evidence: `backend/tests/test_buzzanimate_bridge.py` 8/8 against a fake BuzzAnimate + temp ledger (not-running 503, ledger discovery + no `Origin` sent, 409 passthrough, impostor on the port refused, start launches + waits, missing launcher, render → project media incl. re-render, input checks); with `test_media_routes.py` + `test_port_ledger.py` 40/40. Deviations: `GET /status` answers 200 `{running:false,…}` rather than 503 (it is the status query); a re-render whose file is already linked counts as done. Not verified: the real BuzzAnimate, the real `.vbs`. |
| E5 | Studio → BuzzAnimate: `integrations/buzzanimate_client.py` + `/api/buzzanimate/*` routes + `animate` project step | **done 2026-10-11** (`BuzzcafStudio/backend/integrations/buzzanimate_client.py` — E4's client, only the `buzzcaf_ports` import and header differ; `app/api/buzzanimate_api.py`; 3 lines in `app/main.py`; 4 Dexter actions in `app/services/dexter_domains_extra.py`: `buzzanimate_status`, `buzzanimate_call`, `animate_start`, `animate_status`, domain `produce`). Routes: `GET /api/buzzanimate/status[?start=true]`, `POST /api/buzzanimate/call {tool,args,start?}`, `POST /api/projects/{id}/animate {casting_path?,scenes?,render?,to_buzzedit?,start?}` (background thread: the `script` asset's file → `/api/episode` `script_path`, `out_dir = <project>/animation`; waits for the episode job and, with render, each `render_jobs` MP4; `metadata["animation_output"] = {out_dir, script, files, videos, warnings, episode_job, buzzedit?}`; SSE `animation_progress`/`animation_ready`/`animation_failed`), `GET /api/projects/{id}/animate/status` (in-memory, else the stored output). Evidence: `backend/tests/test_buzzanimate_bridge.py` 7/7 (fake BuzzAnimate + temp ledger; not-running 503s; ledger discovery, no `Origin`; build into `<project>/animation` + stored output + events + restart read-back; `to_buzzedit` through the real `BuzzEditClient` pinned to a fake BuzzEdit: `import_path` for scene 1 then `media/import {copy:true}` for the rest; failed build → `animation_failed`; input checks 404/400; Dexter actions listed); with `test_dexter_gateway`, `test_health`, `test_ports`, `test_produce_video_gates`, `test_buzzedit_job_active` 56/56. Deviations: `to_buzzedit` implies `render`; a failed BuzzEdit hand-off is a warning on a `ready` run, not a failure; the script is passed by path (BuzzAnimate reads it; the `.buzz` takes the script file's stem); the client's `media/import` call uses `BuzzEditClient._post` (no public method exists). Not verified: the real BuzzAnimate/BuzzEdit, a real Studio script through a real build (E23-review probed that half). |
| E6 | Dexter registry health route + ECOSYSTEM.md (both copies) port map, health table, §7b | **done 2026-10-11** (`dexter/data/apps.default.json` `buzzanimate`: `"preferred_port": 8840`, `"health_path": "/health"` after `probes`, as the `buzzedit` record has them; probes kept. `dexter/backend/test_apps_registry.py` `HEALTH_ROUTES` gains `"buzzanimate": ("/health", "buzzanimate")`. `dexter/ECOSYSTEM.md` and `BuzzcafStudio/ECOSYSTEM.md` got the same text, applied by one script (Studio copy kept CRLF): §2 row (local API, **8840**), §3 health row (`buzzanimate`, `/health`), helper table Rust row (`BuzzAnimate/crates/buzz-app/src/ports.rs`, adapted copy), new §7b "BuzzAnimate bridge" (E1 routes + E4/E5 routes) before §8; the §7b block is identical in both (diffed)). Evidence: `python -m pytest test_apps_registry.py test_tools_apps.py test_buzzcaf_ports.py` 130 passed, 1 skipped, 2 failed — the 2 failures are `test_every_copy_of_this_helper_is_byte_identical…` (BuzzFin/BuzzcafCoffee/MastGaane copies missing on this machine) and the Node-helper twin, neither touched here; the ECOSYSTEM byte-identity test skips (it looks for `BuzzcafAI/`, which is now `BuzzcafStudio/`; the two copies had already drifted). Deviation: no `"ledger"` key — `apps_registry.ledger_id` defaults to the record id, which is `buzzanimate`. |
| E456-review | Review of E4/E5/E6 | **accepted with one fix, 2026-10-11.** BuzzEdit's CORS is `allow_origins=["*"]` (pre-existing), so any web page could have driven BuzzAnimate through the new `/api/buzzanimate/*` routes (open/save/export anywhere). Added a router-level `_this_machine_only` dependency to `BuzzEdit/backend/routes/buzzanimate.py` and `BuzzcafStudio/backend/app/api/buzzanimate_api.py`: no Origin or `http://127.0.0.1|localhost:*` only; `null` refused. Tests `test_only_this_machine_may_reach_buzzanimate_through_buzzedit` / `…_through_the_studio`; reran BuzzEdit bridge 9/9, Studio bridge 8/8. Checked: Studio and BuzzEdit both bind 127.0.0.1 by default; `/api/open` opens in a **new tab**, so an unsaved document is not lost (but a load already in progress makes it a silent no-op — the render job then times out). **Left for the user:** BuzzEdit's global `allow_origins=["*"]` still exposes the rest of BuzzEdit's API to any web page; tightening it needs the Electron `file://` origin handled. A future BuzzEdit UI button for these routes must call from Electron's main process (a `file://` page sends `Origin: null`). Dexter: 2 pre-existing failures in `test_buzzcaf_ports.py` (missing helper copies in BuzzFin/BuzzcafCoffee/MastGaane, Node copy drift). |
| E7 | End-to-end check with all three apps running | **done-verified 2026-10-11** (real apps, HTTP only, BuzzAnimate release exe built 10:55 and **minimised** throughout, so E1-review fix 3 is verified). (1) Studio `POST /api/projects/{id}/animate {render,to_buzzedit}` on a throwaway project `2026-10-11_Life3Baje_E7BridgeTestdeleteMe` (two-scene `draft.md`, narrator V.O.) → `ready` in ~20 s: `animation/draft.buzz` + `report.md`, previews `scene01_the_lane.mp4` (106 f) / `scene02_the_shop.mp4` (112 f), 1920×1080 h264 24 fps, captions visible; BuzzEdit project `6eba7486` created with both in its media pool. (2) BuzzEdit `POST /api/buzzanimate/render {buzz_path, project_id}` → `completed`, `draft.mp4` 9.08 s (= 218 f) linked into `6eba7486`. (3) BuzzAnimate `/api/call send_to_buzzedit {scope:scene, project}` → export job done (112 f, GPU), `sent_scene.mp4` in `6eba7486`'s media pool. **Found:** the caption casts a lit-scene shadow onto the ground (garbled dark copy under the text, see PRODUCTION_SPEED_PLAN fix F1); `/api/state` reports `dirty:true` right after `/api/open` (minor, unchecked). Not exercised: `transcribe_with_buzzedit` (needs a voiced sound). The two test projects (Studio + BuzzEdit `6eba7486`) are left in place for the user to look at. |
| review | Review | not started |

## Design

### E1 — BuzzAnimate's API (Rust, `crates/buzz-app`)

- **Ports:** copy `BuzzCode/crates/cb-engine/src/ports.rs` into
  `crates/buzz-app/src/ports.rs` as unchanged as it can be (adapt only what
  ties it to cb-engine's config; note each change in a header comment).
  Ledger name **`buzzanimate`**, preferred port **8840** (`BUZZANIMATE_PORT`
  overrides), span 20, bound on **127.0.0.1 only**. Publish once the listener
  answers `/health`; withdraw on clean exit. A failure to listen or publish
  is a status-line note, never fatal.
- **Server:** a small synchronous HTTP server on its own thread (`tiny_http`
  is the expected choice; justify any other). The UI thread does all document
  work: requests travel over a channel to `App::api_pump` (called each frame
  like `agent_pump`), and the server thread calls
  `ctx.request_repaint()` so an idle window still answers. Timeout 60 s per
  request with a clear error.
- **Security:** localhost only; refuse any request whose `Origin` header is
  not `null`/absent/`http://127.0.0.1:*`/`http://localhost:*` (a web page
  must not be able to drive the app). Bodies ≤ 16 MB.
- **Routes** (JSON in, JSON out):
  - `GET /health` → `{"app":"buzzanimate","status":"ok","port","pid","version"}`
  - `GET /api/state` → document path/title/dirty, current frame, frame count,
    fps, scene names, selection count.
  - `GET /api/catalog` → `agent::catalog()`.
  - `POST /api/call {tool, args}` → `agent::execute` result
    `{ok, value, error}`; one undo step per call, labelled `API: <tool>`.
    Calls that would ask the user (confirm) are refused with an error, never
    a hanging dialog.
  - `POST /api/open {path}`, `POST /api/save {path?}`.
  - `POST /api/export {path, format: mp4|mov|gif|webp|png, from?, to?}` →
    `{job_id}` on the existing background render queue;
    `GET /api/jobs`, `GET /api/jobs/{id}` → state, progress, output path,
    error.
  - `POST /api/episode {script_path | script_text, casting_path?, scenes?, out_dir, render?: bool}`
    → `{job_id}`: runs `episode::build_episode` (the `examples/episode_build.rs`
    flow) off the UI thread, writes the `.buzz` files and report to `out_dir`,
    and with `render` queues an MP4 per scene. Job result lists the files.
- **Tests:** a headless test that starts the server on an ephemeral port with a
  temp ledger (`BUZZCAF_PORTS_FILE`), pumps a headless `App`, and checks
  `/health` identity, ledger publish/withdraw, `/api/call` through to the
  document (and undo), a refused cross-origin request, an export job reaching
  `done` (small range), and `/api/episode` on a two-line script producing a
  `.buzz`.

### E2 — BuzzAnimate → BuzzEdit

- `crates/buzz-app/src/estate.rs`: discovery via the ledger + identity check
  (`buzzedit`, 8099, `/api/health`), using the ports helper; HTTP with the
  `ureq` already in the workspace.
- **File ▸ Send to BuzzEdit…** (and agent tool `send_to_buzzedit`): export the
  active scene or the whole document to MP4 on the queue, then
  `POST /api/projects/import_path {path}` (new project) or
  `/api/projects/{id}/media/import` (to a project picked from
  `/api/projects/list`). Status line reports the BuzzEdit project id. When
  BuzzEdit is not running: say so and offer to start it
  (`<estate>/BuzzEdit/START_APP.vbs`, estate root = BuzzAnimate's parent
  folder), then wait for its health route.
- **Lip sync from BuzzEdit transcription** (agent tool
  `transcribe_with_buzzedit`, menu under Commands): send a sound layer's file
  to BuzzEdit's transcription, receive word timings, write them as captions
  and run the existing Lip Sync from Captions. Read BuzzEdit's
  `routes/transcription.py` for the real request/response first.
- Tests: against a fake BuzzEdit (a tiny local server in the test) — discover,
  import call shape, not-running path.

### E3 — BuzzAnimate → Studio

- Discovery `buzzcaf`, 8099, `/health` (identity `app == "buzzcaf"`; BuzzEdit
  also prefers 8099, the identity check is what tells them apart).
- **File ▸ Import Script from Studio…** (agent tool `studio_projects`,
  `studio_build_episode`): list `GET /api/projects`, fetch the chosen
  project's script (find the Studio's real route for project files/script
  first), then the E1 episode build into
  `<project folder>/animation/`.
- Tests against a fake Studio server.

### E4 — BuzzEdit → BuzzAnimate (Python, `BuzzEdit/backend`)

- `integrations/buzzanimate_client.py`: `discover("buzzanimate", 8840,
  health_path="/health")` via its `buzzcaf_ports.py`; `status()`, `call(tool,
  args)`, `export(path, fmt, …)`, `job(id)`, `start()` (launch
  `<estate>/BuzzAnimate/BuzzAnimate.vbs`, poll health; note the cargo build
  on launch can take minutes).
- Routes, new router `routes/buzzanimate.py` at `/api/buzzanimate`:
  `GET /status`, `POST /call`, `POST /render {buzz_path, project_id?}` (open,
  export MP4, poll, then add to that BuzzEdit project's media), `POST /open
  {buzz_path}`.
- Tests: pytest with the client pointed at a fake BuzzAnimate.

### E5 — Studio → BuzzAnimate (Python, `BuzzcafStudio/backend`)

- `integrations/buzzanimate_client.py` (same shape as E4's).
- Routes `/api/buzzanimate/status`, `/api/buzzanimate/call`, and
  `POST /api/projects/{id}/animate {casting_path?, scenes?, render?}` →
  writes the project's script to its folder, calls `/api/episode`, polls,
  stores `animation_output` (files + MP4s) on the project, emits SSE
  `animation_progress` / `animation_ready` / `animation_failed`;
  `GET /api/projects/{id}/animate/status`.
- Optional hand-off: `POST /api/projects/{id}/animate {…, to_buzzedit: true}`
  imports the rendered MP4s into a BuzzEdit project through the existing
  `buzzedit_client`.
- Tests: pytest with fakes, in the Studio's existing test layout.

### E6 — Dexter + docs

- `dexter/data/apps.default.json` `buzzanimate`: add `"health_path": "/health"`,
  `"ledger": "buzzanimate"` (match how other records name these — read
  `test_apps_registry.py`), keep the process probes.
- ECOSYSTEM.md (both `dexter/` and `BuzzcafStudio/` copies — they have already
  drifted; add the same text to both): port map row (8840), health-route table
  row, a §7b "BuzzAnimate bridge" route table, and a Rust copy row for
  `ports.rs` in the helper table.
- Run Dexter's registry/ports tests.

### E7 — end to end

With all three running: Studio `animate` a two-line project → BuzzAnimate
builds and renders → MP4 lands in a BuzzEdit project; BuzzEdit
`/api/buzzanimate/render` on a `.buzz`; BuzzAnimate Send to BuzzEdit. Done
without the real mouse (HTTP only), and only while the user is not using the
machine, or with their go-ahead.

## Not in scope

- MidnightBuzz voices → BuzzAnimate (next round; see
  `docs/BUZZANIMATE_VS_ANIMATE.md` §6 item 2).
- A UI inside BuzzEdit's Electron front end or the Studio front end — the
  routes come first; buttons follow once the routes are proven.
