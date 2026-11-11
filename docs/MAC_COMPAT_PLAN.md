# macOS compatibility plan

Goal: BuzzAnimate builds, starts and behaves on a Mac (Apple Silicon) the way
it does on Windows, without changing anything on Windows.

## Status

| Step | What | State | Date | Evidence |
|---|---|---|---|---|
| M0 | Local Mac type-check from Windows (`tools/mac-check.sh`) | done-verified (Opus) | 2026-11-05 | Reviewer re-ran: "mac check: OK", buzz-app lib/bin/tests/examples all checked; `Cargo.lock` unchanged; `bash -n` ok |
| M1 | GPU picker asks for Metal on macOS (**blocker**) | done-verified on Windows, Mac run pending | 2026-11-05 | Diff reviewed; Windows `adapter` tests 7 passed (real adapter still picked here); Metal path compiled, not run |
| M2 | ⌘Q goes through the app's own Quit (prompts), not winit's menu | compiled, Mac run pending | 2026-11-05 | `main.rs` `with_default_menu(false)` reviewed; mac-check ok; whether ⌘Q reaches egui needs a Mac (M8) |
| M3 | Film concat finds Homebrew ffmpeg | done-verified | 2026-11-05 | `film.rs` → `ffmpeg_cmd()`; `cargo test -p buzz-export --lib film` 5 passed |
| M4 | Logs, script data, Animate folders on macOS | done-verified on Windows, Mac run pending | 2026-11-05 | Diffs reviewed: Windows lookups unchanged; `buzz-script --lib` 43 passed, `jsfl` 2 passed; mac-check ok |
| M5 | ComfyUI on macOS: find Desktop (port 8000), start `main.py` | done-verified on Windows, Mac run pending | 2026-11-05 | Diff reviewed: Windows `.bat` path unchanged; `comfy` 5 passed; mac-check ok |
| M6 | `BuzzAnimate.app` bundle (Dock icon, mic permission), Intel-Mac message | revised in review, Mac run pending | 2026-11-05 | Review: a wrapper script that execs the binary loses the bundle identity (name, icon, mic text), so the binary is now copied into the bundle, `LSEnvironment BUZZ_HOME` points at the repo, and `main.rs` changes to `buzz_home()` on macOS when started outside it. README uses `bash tools/make-macos-app.sh` (exec bit not in git). `bash -n` ok; not run (needs a Mac) |
| M7 | macOS CI workflow file (not pushed until the user says so) | written, not pushed | 2026-11-05 | `.github/workflows/macos.yml` reviewed (workflow_dispatch, macos-14) |
| M8 | Checks only a real Mac can do | waiting for a Mac / CI | | |

## What was found (2026-11-05)

**Already Mac-ready** (from `d3e088f` and later): `BuzzAnimate.command` launcher
(executable bit set in git), ⌘ shortcuts via `Modifiers::COMMAND`, shortcut
labels via `ctx.format_shortcut`, native inset title bar (`title_bar.rs`
`MacInset`), Finder reveal (`open -R`), CoreML for ONNX, Mac font folders,
Homebrew ffmpeg lookup in `video.rs`, `sysctl hw.memsize`, `chmod 600` on the
AI key file, settings under `~/.config/BuzzAnimate` via the `HOME` fallbacks.
File pickers run on worker threads, and rfd 0.16 hops them onto the main
thread itself on macOS (`backend/macos/utils.rs run_on_main`), so they are safe.

**Compiles for macOS.** `cargo check --workspace --all-targets --target
aarch64-apple-darwin` passes with zero errors (warnings are the same ones
Windows has). Two things block a plain cross-check from Windows, both C build
scripts, not our code: `rquickjs-sys` needs a C compiler (stood in by a fake
`cc`/`ar` — `cargo check` never links), and `coreaudio-sys` needs Apple's SDK
(stood in by a copy of `cpal 0.15.3` whose macOS host is its own null host,
passed with `--config patch.crates-io.cpal.path=...`; `Cargo.lock` restored
afterwards). The real CoreAudio path is unchanged and only a Mac build checks it.

**Broken on a Mac at run time:**

1. **No GPU → the app cannot start.** `buzz-render/src/adapter.rs:233`
   enumerates `Backends::DX12 | Backends::VULKAN` only. A Mac has Metal, so the
   list is empty and `select` returns `NoAdapters`. The scorer already rates
   Metal (90); only the enumeration leaves it out.
2. **⌘Q skips the quit prompt.** winit 0.30 installs a default macOS menu whose
   Quit item sends `terminate:` (`winit/platform_impl/macos/menu.rs:71`). That
   ends the process via `applicationWillTerminate` without `CloseRequested`,
   so `App::request_quit` (export-running / unsaved prompts) never runs.
   The app's own `Command::Quit` is already `COMMAND+Q`.
3. **Film export loses ffmpeg when started from Finder.** `buzz-export/src/
   film.rs:56` runs bare `Command::new("ffmpeg")`; a Finder-launched app has no
   `/opt/homebrew/bin` on `PATH`. `video::ffmpeg_cmd()` already handles it.
4. **Release log, script data, Animate folders.** `main.rs:252` writes the log
   only under `LOCALAPPDATA` (no log at all on Mac). `buzz-script/src/shelf.rs:39`
   falls back to the temp folder (macOS clears it). `buzz-script/src/lib.rs`
   `dirs_next_config` is `APPDATA`-only. Animate's own folders are looked up
   under `LOCALAPPDATA`/`APPDATA` only (`buzz-script/src/lib.rs:237`,
   `buzz-app/src/jsfl_commands.rs:57` and `:479`); on a Mac Adobe keeps them in
   `~/Library/Application Support/Adobe`.
5. **ComfyUI.** `comfy.rs` can only start `run_nvidia_gpu.bat`, its default
   folder is a `B:\` path, and discovery scans 8188–8208 while ComfyUI Desktop
   for Mac listens on 8000.
6. **No Mac app.** Only the Terminal launcher exists: no Dock icon, and the
   microphone permission (lip-sync recording, `buzz-audio/src/mic.rs`) is
   granted to Terminal. Intel Macs cannot build: `ort-sys 2.0.0-rc.13` ships
   prebuilt ONNX Runtime for `aarch64-apple-darwin` only (`build/download/dist.tsv`).

**Left as is, on purpose:** Buzzcaf Studio / BuzzEdit launchers are `.vbs`
(`estate.rs`, already a clear error elsewhere); GPU-utilisation readout
(`buzz-jobs/clock.rs`) says "unavailable" off Windows; `nvidia-smi` probe just
fails; settings stay in `~/.config/BuzzAnimate` (moving them would split a
user's data across a dozen modules for no gain).

## Steps

### M0 — `tools/mac-check.sh`
Git-Bash script that repeats the check above: builds `fakecc`/`fakear` with
`rustc` into `target/mac-check/`, copies `cpal-0.15.3` from the cargo registry
into `target/mac-check/cpal`, rewrites its macOS cfgs to the null host and
drops its `coreaudio-rs` sections, backs up `Cargo.lock`, runs the check with
`CC_/CXX_/AR_aarch64_apple_darwin` and the `--config patch`, restores
`Cargo.lock` (also on failure, via `trap`), prints `error` lines and exits
non-zero if there were any. Run before any change that touches `cfg` code.

### M1 — Metal
`adapter.rs` `select`: enumerate `Backends::METAL` on macOS and keep
`DX12 | VULKAN` elsewhere; fix the doc comment.

### M2 — ⌘Q
`main.rs`: on macOS build the event loop with
`winit::platform::macos::EventLoopBuilderExtMacOS::with_default_menu(false)`.
With no menu, ⌘Q reaches the window as a key press and egui's `COMMAND+Q`
shortcut runs `Command::Quit` → `request_quit`, exactly as Ctrl+Q on Windows.
Trade-off: no ⌘H "Hide" item (egui had no Edit menu from winit anyway, so
⌘C/⌘V are unaffected).

### M3 — film ffmpeg
`film.rs`: `crate::video::ffmpeg_cmd()` instead of `Command::new("ffmpeg")`.

### M4 — folders
* `main.rs init_tracing`: macOS → `~/Library/Logs/BuzzAnimate/last.log`;
  other non-Windows → `$HOME/.config/BuzzAnimate/logs/last.log`.
* `shelf.rs overlay_root` and `buzz-script dirs_next_config`: same order as
  `buzz-doc/src/assets.rs::default_root` (APPDATA → XDG_CONFIG_HOME →
  HOME/.config), temp only last.
* Animate folders: on macOS also look under
  `$HOME/Library/Application Support/Adobe` (Commands/Configuration) and
  `$HOME/Library/Application Support/Adobe/Animate` (Shortcuts).

### M5 — ComfyUI
* Discovery also probes port 8000.
* Default folder: Windows keeps the `B:\` portable path; elsewhere `$HOME/ComfyUI`.
* Non-Windows start: if `<dir>/main.py` exists, run it with
  `<dir>/venv/bin/python`, `<dir>/.venv/bin/python` or `python3`, in `<dir>`,
  stdin/stdout/stderr null, own process group; else the existing clear error,
  reworded ("start ComfyUI yourself or set COMFYUI_DIR to a folder with main.py").

### M6 — Mac app
* `tools/make-macos-app.sh`: `cargo build --release -p buzz-app`, then write
  `BuzzAnimate.app` (repo root, git-ignored) with `Contents/MacOS/BuzzAnimate`
  = small script that `cd`s to the repo and `exec`s `target/release/buzzanimate "$@"`,
  `Contents/Info.plist` (name, `com.buzzcaf.buzzanimate`, icon,
  `NSMicrophoneUsageDescription`, `NSHighResolutionCapable`,
  `LSMinimumSystemVersion` 11.0), and `AppIcon.icns` made with `sips` +
  `iconutil` from `assets/logo-*.png`. Ad-hoc sign: `codesign -s - --force`.
* `BuzzAnimate.command`: on `uname -m` = `x86_64`, stop with a clear message
  (Apple Silicon needed; ONNX Runtime has no Intel-Mac build).
* README: one short macOS section.

### M7 — CI
`.github/workflows/macos.yml`, `workflow_dispatch` only (macOS minutes are
billed 10×): `macos-14` (Apple Silicon), install ffmpeg, `cargo check
--workspace --all-targets`, `cargo build --release -p buzz-app`, then
`cargo test` for the crates that need no GPU. Not pushed without the user's go-ahead.

### M8 — needs a real Mac (or CI for the build part)
Starts and renders on Metal; colours match Windows (CAMetalLayer colour space
on P3 screens); traffic lights do not cover the menu row; ⌘Q prompts with an
unsaved document; mic recording asks for permission; video export with
Homebrew ffmpeg; a Windows-made `.buzz` with absolute `B:\` media paths opens
(expected: media reported missing, not a crash).
