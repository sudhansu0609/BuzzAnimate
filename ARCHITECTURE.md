# BuzzAnimate — Architecture: the engine waves

**Written:** 2026-08-15 · **Designs:** Waves 4–15 · **Format versions:** 19, 20, 21

The forward design. [`PROGRESS.md`](PROGRESS.md) records what was built and why;
[`IMPROVEMENTS.md`](IMPROVEMENTS.md) says what to build next and in what order; this
file says **how**, in enough detail to start typing.

Waves 1–3 (`IMPROVEMENTS.md` Part I) are the assembly work — clipboard, thumbnails,
pose library, scene templates. They need no architecture: the pieces exist and the
work is wiring. Everything below is new machinery, so it gets a design first.

| | |
|---|---|
| **Part II — the engine waves** | 4 Foundations · 5 Background export · 6 Compositor · 7 Raster layers · 8 Asset watcher · 9 2.5D · 10 Film · 10b Camera angles |
| **Part III — the delight waves** | 11 Animation feel · 12 Procedural motion · 13 Drawing delight · 14 Pro output · 15 Command & control |

> Every file:line below was read out of the source while this was written. Where a
> design says "reuse X", X exists today and is named.

---

## 0. The one rule

> **The window must never stop responding. Not for a script, not for an export, not
> for a heavy first frame, not for a file dialog.**

This is the constraint the whole of Part II is arranged around, because it is the one
the user stated first and the one that is easiest to lose a feature at a time. It is
written as six rules, and the rules are testable.

### The six rules

1. **No closure on the UI thread may exceed ~4 ms.** At 60 fps a frame is 16.7 ms and
   egui must lay out, tessellate and encode inside it. Anything that *can* exceed 4 ms
   goes through the `TaskRegistry` (§4) with progress and cancel, or through
   `JobSystem::run(Pool::Interactive)` when the answer is needed this frame and is
   provably sub-frame.
2. **Owned snapshot in, message out.** Background work receives an owned `Scene` — a
   copy-on-write `Arc` tree, so the clone is pointer copies — and reports back only
   over a channel. No background thread ever holds a reference into live document
   state. This is exactly what autosave does today (`buzz-doc/src/autosave.rs:188`),
   and it is the reason autosave has never raced.
3. **Cancel is observed within 100 ms.** Every task loop checks its `CancelToken` at
   least that often. A cancel button that takes eight seconds to be noticed is not a
   cancel button.
4. **No blocking OS dialog on the UI thread. Ever.** There are seven `rfd` call sites
   today and each one freezes the window for as long as the user browses.
5. **Commit on complete.** A finished background job's result is applied on the next
   frame through `Document::edit`, so it lands as one ordinary undo step and the
   history reads the way the user's actions read.
6. **No work on rayon's global pool.** `buzz-geom`'s booleans, the IK solver and the
   export's PNG encoding all use the global pool today, so they contend with each
   other and with nothing scheduling them. They move onto `JobSystem`'s two pools.

### What breaks the rule today

Read out of the source. This list is the acceptance criterion for Wave 4 — when every
row is struck through, the rule holds.

| Where | What happens | Cost |
|---|---|---|
| `buzz-app/src/editor.rs:2452` `run_script` | Clones the scene, runs `buzz_script::run` synchronously, commits | A five-second script freezes the window for five seconds (§7-32) |
| `buzz-app/src/app.rs:3056` → `buzz-render/src/document.rs` → `buzz-light/src/geometry.rs` | Every shading crescent, cast shadow and blur band is built before the first frame appears, single-threaded | **305 ms** on a heavy scene (§7-155), and paid again whenever a light moves, because `LightCache::begin` (`buzz-render/src/lighting.rs:74`) clears the whole cache on any rig change |
| `buzz-app/src/app.rs` — 7 `rfd::FileDialog` sites | Modal, on the UI thread | The window is dead while the file browser is open |
| `buzz-app/src/app.rs` — import and `Document::open` | Parse and build on the UI thread | A large `.fla` freezes the window for its whole read |
| `buzz-app/src/export_job.rs` | Cancel flag read once per 16-frame batch; `read_back` can block 60 s | Cancel is not felt for seconds |

Note what is **not** on the list: the export's rendering. It already runs on its own
thread with its own GPU device. That part was built right.

---

# Part II — the engine waves

## Wave 4 — Foundations: the task registry (M)

**Nothing else in Part II is safe to build until long work has somewhere to live.**

### Decision

A `TaskRegistry` owned by `App`, holding every long-running job; `buzz-jobs` gains one
primitive and nothing else.

*Rejected: putting job handles inside `JobSystem`.* Its two pools are rayon pools,
sized for data-parallel bursts. A minutes-long export would squat on one of the six
background workers and starve autosave — the only other thing that uses that pool.
Tasks and pools are different lifetimes and want different types.

### Shapes

```rust
// crates/buzz-jobs/src/lib.rs — the only addition to this crate
#[derive(Clone, Default)]
pub struct CancelToken(Arc<AtomicBool>);

impl CancelToken {
    pub fn cancel(&self);
    pub fn is_cancelled(&self) -> bool;
}
```

```rust
// crates/buzz-app/src/tasks.rs — new
pub struct TaskId(u64);

pub enum TaskKind {
    Export, ConcatFilm, Import, Open, Script, AssetScan, Thumbnails, Resample,
}

pub struct TaskProgress { pub done: u64, pub total: u64, pub detail: String }
pub enum TaskOutcome { Finished, Cancelled, Failed(String) }

/// Handed to every task closure: how to report, and how to be stopped.
pub struct TaskCtx { pub cancel: CancelToken, pub progress: ProgressSink }

pub struct Task {
    pub id: TaskId,
    pub kind: TaskKind,
    pub label: String,
    pub cancel: CancelToken,
    pub progress: Arc<Mutex<TaskProgress>>,
    pub started: Instant,
    join: Option<JoinHandle<()>>,          // owned thread; pool jobs carry None
    done: Receiver<TaskOutcome>,
}

pub struct TaskRegistry { tasks: Vec<Task>, next: u64 }

impl TaskRegistry {
    pub fn spawn_thread(&mut self, kind: TaskKind, label: String,
        f: impl FnOnce(TaskCtx) -> TaskOutcome + Send + 'static) -> TaskId;
    pub fn spawn_pool(&mut self, jobs: &JobSystem, pool: Pool, kind: TaskKind,
        label: String, f: impl FnOnce(TaskCtx) -> TaskOutcome + Send + 'static) -> TaskId;
    pub fn cancel(&mut self, id: TaskId);
    /// Drains what finished. Called once per frame from `App::update`.
    pub fn poll(&mut self) -> Vec<(TaskId, TaskKind, TaskOutcome)>;
    pub fn running(&self) -> impl Iterator<Item = &Task>;
    /// Tasks that must not be lost to a quit — exports and film assembly.
    pub fn quit_blockers(&self) -> Vec<&Task>;
}
```

**Why the registry lives on `App` and not on `Editor`.** This is the structural fix
for a real bug. Today the export job is a field of `App` (`app.rs:210`) but its
progress dialog is a field of `Editor` (`editor.rs:107`). Opening a document builds a
fresh `Editor`, so the dialog state is destroyed while the job keeps running: progress
vanishes, the Cancel button becomes unreachable, and the completion message lands in
the *new* document's status bar. Work that outlives a document must be owned by
something that outlives a document.

### The five refits

**1 · Scripts off the UI thread.** `run_script` (`editor.rs:2452`) already clones the
scene before running — so it is one `spawn_thread` away. `buzz-script` gains
`RunOptions { interrupt: Option<CancelToken> }`, checked from the interpreter's fuel
counter every N instructions. While a script runs the editor shows an input-gating
overlay with a live Cancel; the *window* keeps painting, the *document* is briefly
read-only. That is the honest semantic — a script is a transaction — and it is very
different from a frozen window. Closes §7-32.

**2 · The 305 ms first frame.** Two changes, both of which §7-155 already proposes.
*(a)* Build shading geometry in parallel: the crescent/shadow work in
`buzz-light/src/geometry.rs` is per shape × per light and embarrassingly parallel, run
under `JobSystem::run(Pool::Interactive)`. *(b)* Never block first paint: when
`LightCache` is cold, **paint the frame unlit**, start the build, and
`request_repaint` when it lands. `LightCache` gains a `Pending` state. Separately,
stop clearing the whole cache when the rig fingerprint changes
(`buzz-render/src/lighting.rs:74`) — key entries **per light**, so touching one lamp
does not rebuild the sun. That last change is also what makes keyframed lights
affordable in Wave 9a. Closes §7-154, §7-155.

**3 · Import and open in the background.** Parse on a `TaskKind::Open` thread, deliver
the built `Document` over the channel, install it on poll. The importers already
produce a report of what they could not bring across; progress rides the same channel.

**4 · File dialogs off the thread.** New `crates/buzz-app/src/dialogs.rs`:

```rust
pub fn spawn_file_dialog(purpose: DialogPurpose, filters: &[(&str, &[&str])])
    -> Receiver<Option<PathBuf>>;
```

`std::thread::spawn` the blocking `rfd` call, send the answer down a channel, and let
`App` hold `pending_dialog: Option<(DialogPurpose, Receiver<..>)>` and route the reply
in `update`. *Rejected `rfd`'s async API:* it wants an executor in a codebase that has
none, where a thread and a channel is twenty lines and matches how everything else
here already talks.

**5 · Export cancel latency.** Check the token every frame rather than every sixteen,
and wait for readback in 10 ms `device.poll` slices with a cancel check between them.

### Testing

`TaskRegistry` is pure `std` and tests without a GPU: spawn → progress → cancel →
poll; a panicking task reports `Failed` rather than poisoning the registry;
`quit_blockers` counts exports and not thumbnails. `buzz-script` gets an
infinite-loop-plus-token test that must return within a deadline. A render test
asserts that a cold `LightCache` still paints (unlit) without building geometry on the
calling thread.

### Size and dependencies

**M.** Depends on nothing. **Everything else depends on it.**

---

## Wave 5 — Background export: queue, presets, and a Tasks panel (M)

Video export already works — NVENC with software fallback, frames piped to the ffmpeg
already on the machine, soundtrack muxed in
(`crates/buzz-export/src/video.rs`, tested by
`crates/buzz-export/tests/headless_video.rs`). What is missing is everything *around*
it: you cannot start a second one, the progress dialog dies with the document, and
quitting throws away a half-written file.

### Decision

A strictly **serial** export queue on `App`, each job a `TaskRegistry` thread reusing
today's `ExportJob`.

*Rejected: parallel exports.* Every job builds its own second wgpu device and Vello
renderer (`buzz-export/src/lib.rs:153`). Running four at once quadruples VRAM and
fights NVENC's session limit to finish the same total work no sooner. Serial is not a
limitation here; it is the correct scheduling.

### Shapes

```rust
// crates/buzz-app/src/export_service.rs — new
pub enum ExportTarget {
    PngSequence(PathBuf),
    Video(PathBuf),
    Gif(PathBuf, GifSettings),
    Webp(PathBuf, WebpSettings),
}

pub struct ExportRequest {
    pub scene: Scene,               // owned snapshot, as ExportJob already takes
    pub audio: Vec<AudioTrack>,
    pub settings: ExportSettings,
    pub video: Option<VideoSettings>,
    pub target: ExportTarget,
    pub range: FrameRange,
    pub label: String,
}

pub struct ExportQueue { pending: VecDeque<ExportRequest>, active: Option<TaskId> }

impl App {
    /// In `update`: nothing active and something pending → start the next.
    fn pump_export_queue(&mut self);
}
```

The one-slot gate at `app.rs:2413` is deleted. "Export while exporting" now means
"joins the queue".

### The Tasks panel

A dockable panel (`crates/buzz-ui/src/tasks_panel.rs`) listing every running task —
kind, label, progress bar, elapsed, Cancel — and finished exports with "Reveal in
folder". It is **global**, so `File ▸ New` mid-export changes nothing about it. The
export dialog on the document becomes configure-and-enqueue only.

Its rows come from `TaskRegistry::running()`, so imports, thumbnail batches and asset
scans appear there too. One place to look for "what is this program doing".

### Quit protection

On a close request, `quit_blockers()` non-empty raises:

> **Export "shot3.mp4" is 37% done.** Keep waiting · Cancel export and quit

Cancelling deletes the `.part` file. A successful export keeps today's write-to-`.part`
then rename, which is the same atomic-write discipline autosave uses.

### Presets

```rust
// crates/buzz-export/src/preset.rs — new
#[derive(Serialize, Deserialize, Clone)]
pub struct ExportPreset {
    pub name: String,
    pub target_kind: TargetKind,
    pub settings: ExportSettings,
    pub video: Option<VideoSettings>,
    pub gif: Option<GifSettings>,
}
```

Stored **app-level**, in `export_presets.json` beside the dock layout.

*Rejected: presets in the document.* A preset encodes a delivery target — "YouTube
1080p", "GIF preview" — which belongs to the person and outlives any one film, exactly
like the workspace and the theme. It also means no format bump. Ships with built-ins:
YouTube 1080p, Master (HEVC, high quality), GIF preview 480p.

### GIF and WebP (closes CP-6.3)

One ffmpeg invocation with a split filter, so the frames are piped once:

```
-filter_complex "[0:v]split[a][b];[a]palettegen=stats_mode=diff[p];[b][p]paletteuse=dither=bayer:bayer_scale=3"
```

`stats_mode=diff` weights the palette towards what actually changes between frames,
which is what makes an animated GIF of a character on a flat background look right.
Animated WebP via `libwebp_anim`, single pass. New `crates/buzz-export/src/gif.rs`,
built like `video.rs`.

*Rejected: gifski.* Better dithering, but it is a new dependency with its own thread
pool when ffmpeg is already shipped, already tested, and already piped to.

### Testing

Headless, skipping without a GPU: enqueue two small exports and assert they ran
serially and both files exist; cancel the active one and assert the `.part` is gone
and the next started; a GIF golden checked with `ffprobe` for stream kind and frame
count. Preset serde round-trip and `quit_blockers` are plain unit tests.

### Size and dependencies

**M.** Depends on Wave 4.

---

## Wave 6 — The compositor: bloom, grain, vignette, grade (M)

### Decision

A raw-wgpu post-pass chain in `buzz-render`, inserted between Vello's output and the
blit to the screen.

This is possible because of a detail of how the window already draws: Vello renders
into a texture **the application owns**, which is then blitted to the swapchain and
has egui drawn over it (`buzz-app/src/app.rs`). That seam is ours. So although §7-46
and §7-52 correctly say *Vello offers no shader hook* — which is why lighting and
filters are built as geometry — that argument does not reach a **full-frame** pass
after Vello has finished. Per-shape effects still cannot be shaders; per-frame effects
can.

*Rejected: effects inside the Vello scene.* Bloom and grain are not vector operations;
expressing them as geometry is what already makes blur expensive (§7-52).

### Shapes

```rust
// crates/buzz-render/src/compositor.rs + compositor.wgsl — new
pub struct Compositor { /* pipelines, bind groups, ping-pong targets, bloom chain */ }

impl Compositor {
    pub fn new(device: &wgpu::Device, format: wgpu::TextureFormat) -> Self;
    pub fn resize(&mut self, device: &wgpu::Device, width: u32, height: u32);
    pub fn run(&mut self, encoder: &mut wgpu::CommandEncoder,
               input: &wgpu::TextureView, output: &wgpu::TextureView,
               post: &PostSettings, frame_index: u32);
}
```

Format- and device-parameterised on purpose, so the wgpu 29 → 30 migration that
arrives with vello 0.10 does not touch this API.

**The pass chain.** Bright-pass → dual-Kawase down/up (3–4 steps) → additive bloom;
then **one fused final pass** doing grade (exposure, contrast, saturation,
temperature/tint, lift-gamma-gain) → vignette → grain. One pass rather than four
because each extra full-screen pass is another read and write of the whole frame.

Grain is a hash of `(pixel, frame_index)` — **deterministic**, so an export is
reproducible and the test can assert two renders of the same frame are identical. With
`post.enabled == false` the chain is a straight passthrough, and a test asserts it is
bit-identical.

### Where the settings live

```rust
// crates/buzz-scene — on StageProperties (lib.rs:78)
pub struct PostSettings {
    pub enabled: bool,
    pub bloom: BloomSettings,
    pub grain: GrainSettings,
    pub vignette: VignetteSettings,
    pub grade: GradeSettings,
}
```

**Per scene**, because it is the look of the film and you must be able to see it on the
stage while you work. Export presets carry only `post_override: UseScene | Disabled`,
never a second copy of the settings.

That is the parity guarantee, and it is worth stating plainly: **the stage and the
export run the same `Compositor::run`**, from the same crate, on their two different
devices. There is no second implementation to drift.

### Depth of field, the cheap half

`FrameOptions` (`buzz-render/src/document.rs:34`) gains
`dof: Option<DofParams { focus_distance: f32, aperture: f32 }>`. Where the draw walk
already picks a projection from layer depth, it also sets the **existing** per-shape
blur hook (`document.rs:729`, consumed at the shape) from
`aperture × |1 − depth_scale(depth)|`, using `CameraTrack.focal_distance`.

Zero new plumbing, and it ships in days. It is the *geometric* band blur, so it is an
approximation of an approximation — the honest version arrives in Wave 9c. Closes the
cheap half of §7-29.

### Format

**Version 18 → 19.** `PostSettingsDto` with `#[serde(default)]`, so a v18 file loads
with effects off. Back-compat test copies the strip-the-field pattern already in
`buzz-doc/src/serial.rs` (the test that removes `alpha` to simulate a pre-v18 file).

### UI

An **Effects** section in the document properties: per-pass enable and sliders.
`View ▸ Preview Effects` (on by default) turns the preview off on a weak GPU without
changing what exports.

### Testing

Headless GPU (`crates/buzz-render/tests/compositor.rs`), skipping without an adapter:
vignette darkens corners relative to centre; bloom bleeds past a bright dot's edge;
the same frame index twice reads back identical; disabled is bit-identical to input.
Plus the v18 → v19 default-load round trip.

### Size and dependencies

**M.** No hard dependency — the seam exists today — so it can run alongside Wave 5.

---

## Wave 7 — Raster layers (L)

The largest piece, and the one that makes the program an all-in-one drawing tool
rather than a vector tool that can also stamp bitmaps.

### What is wrong with painting today

The soft brush works and looks right, but every stroke becomes **its own `ImageAsset`
and its own rectangle object** (`buzz-app/src/editor.rs:1326`). So:

- A thousand strokes is a thousand objects and a thousand library entries (§7-164).
- `ImageLibrary` is a `BTreeMap` on `Scene`, deep-cloned on every `Document::edit` —
  fine at a hundred strokes, a real per-keystroke cost at ten thousand.
- Nothing ever removes an image, so undone strokes still ship inside the `.buzz`.
- Any pixel change makes a new identity, so Vello re-uploads the **whole** image
  (§7-167).
- The in-progress stroke rebuilds its whole canvas and a whole throwaway `ImageAsset`
  on **every pointer move** — 7.8 ms for a full-width sweep (§7-166).
- Strokes cannot merge into each other, which is what painting *is* (§7-164).

### Decision

**256-pixel tiles, straight-alpha RGBA8, one `Arc` per tile, sparse map, hung on a new
`LayerKind::Raster`. Undo is tile copy-on-write.**

*Rejected: one buffer per layer.* `Document::edit` snapshots the scene before every
change, so the first stroke after each snapshot forks the whole buffer — 8.3 MB at
1080p, and **~830 MB across a hundred undo steps**. This is precisely the objection
already recorded against full-canvas painting in §7-164, and it is fatal.

*Rejected: a stroke journal (replay from a log).* Replay cost grows without bound,
and erasers and filters force checkpoints anyway, so the memory returns by the back
door.

Tiles answer both, and they answer the GPU upload problem with the same mechanism: one
change, two open defects closed.

### Shapes

```rust
// crates/buzz-scene/src/raster_layer.rs — new
pub const TILE: u32 = 256;

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct TileCoord { pub x: i32, pub y: i32 }

#[derive(Clone)]
pub struct Tile {
    pixels: Arc<[u8]>,   // TILE * TILE * 4, straight alpha
    identity: u64,       // from the same global counter ImageAsset uses
}

#[derive(Clone, Default)]
pub struct RasterCanvas {
    tiles: BTreeMap<TileCoord, Tile>,   // sparse: untouched space costs nothing
    pub resolution: f32,                // canvas pixels per document unit; default 1.0
}

impl RasterCanvas {
    pub fn composite_stroke(&mut self, stroke: &Canvas, color: Color, mode: BrushMode);
    pub fn apply_filter(&mut self, filter: &RasterFilter, region: Option<Rect>);
    pub fn tile(&self, at: TileCoord) -> Option<&Tile>;
    pub fn sample(&self, x: f64, y: f64) -> [u8; 4];   // for the Magic Wand
}

pub enum BrushMode { Paint, Erase }
pub enum RasterFilter {
    GaussianBlur(f32),
    BrightnessContrast(f32, f32),
    HueSaturate(f32, f32, f32),
}
```

Every design question, answered once:

**Tile size and format.** 256 px, RGBA8, **straight** alpha — matching `ImageAsset`'s
existing convention (`buzz-scene/src/image.rs:40`), because two alpha conventions in
one renderer is a bug waiting to be written. 256 KB a tile: big enough that a
BTreeMap stays small, small enough that a dab forks a quarter-megabyte and not eight.
The existing one-byte-per-pixel coverage `Canvas` (`buzz-scene/src/raster.rs:107`)
survives untouched as the **per-stroke intermediate** — it is the right shape for
accumulating a stamp path, and its `max`-blend is what stops beading.

**How a layer becomes paintable.** `LayerKind::Raster`, plus
`canvas: Option<RasterCanvas>` on `Layer`, with the invariant `kind == Raster ⇔
canvas.is_some()` enforced by a `Layer::raster(name)` constructor. A raster layer holds
no objects.
*Rejected: a canvas on any layer.* Tools, hit-testing, selection and undo semantics
all fork in two if a layer can be both. Krita and Photoshop separate the kinds, and
they separate them for this reason.
*Rejected: a raster object kind.* That is what exists now, and it is §7-164.

**Undo.** Tile copy-on-write. `Document::edit` clones the `Scene`; cloning a
`RasterCanvas` copies `Arc` pointers. The stroke then `Arc::make_mut`s only the tiles
it touched, each taking a new identity. A hundred undo steps of small dabs is
~100 × 256 KB ≈ **25 MB**, against 830 MB for the whole-canvas design. The 600 ms
history coalescing already in place means a continuous drag is one step.

**GPU upload.** One `peniko::Image` per tile, keyed by the tile's identity — the exact
discipline `ImageAsset.identity` already uses as a blob id. `buzz-render` gains a small
`TileBlobCache: HashMap<u64, peniko::Image>`. Untouched tiles keep their `Arc` and
their identity, so Vello's blob cache hits and **only dirty tiles upload**. Closes
§7-167. No per-tile `ImageAsset` — blobs are built straight from the tile `Arc`s, so
the library does not grow.

**Retargeting the brush.** Input still builds the bbox-sized coverage `Canvas`. The
commit path changes from "new asset + new rectangle" to a single `Document::edit`
calling `composite_stroke` — one undo entry per stroke. The eraser becomes
`BrushMode::Erase` (destination-out on alpha), which is the first time erasing paint
has been possible at all. Closes §7-164.

**The preview.** Stop rebuilding an `ImageAsset` per pointer move. The in-progress
stroke draws as a **transient overlay** — one stroke-bbox-sized image with its own
throttled identity, living outside the document and discarded on commit. Closes
§7-166.

**Resolution and zoom.** `resolution` is per canvas, chosen when the layer is made
(0.5 / 1 / 2 / 4 px per document unit, default 1) and rendered through a
`scale(1/resolution)`. Zooming past it goes soft, as paint does. An explicit
`Layer ▸ Resample Raster Layer…` doubles it as a `TaskKind::Resample` background job.
*Rejected: re-rasterising to match zoom.* Krita does not; it would mean the artwork
changes when you look closer, and it buys the author nothing. Answers §7-165.

**Filters.** Whole-layer CPU operations in `buzz-scene/src/raster_ops.rs`, parallel
across tiles on `Pool::Interactive`, each application one `Document::edit`. A
whole-canvas filter forks every tile once — that is the honest cost, and it is bounded
by history depth.

**Tablet pressure.** Adopt `octotablet` behind a `tablet` cargo feature, isolated in
`crates/buzz-app/src/tablet.rs`; pressure drives radius and flow. Without the feature
or the hardware, every sample is 1.0 exactly as now. winit 0.30 supplies nothing on
Windows, so this is purely additive. Closes §7-25.

### Format

**Version 19 → 20.** `RasterLayerDto { resolution, tiles: Vec<TileDto { x, y, png }>}`
— tiles PNG-compressed inside the `.buzz`, reusing the encoder already there. Empty
tiles are absent rather than stored. `#[serde(default)]` and the strip-field
back-compat test as always.

### UI

"New Raster Layer" in the Layers panel; brush and eraser act on the active raster
layer and keep today's vector behaviour elsewhere, with the status bar saying which
mode is live; layer properties gain resolution and Resample;
`Layer ▸ Filter ▸ {Blur, Brightness/Contrast, Hue/Saturation}` enable on raster layers.

### Testing

CPU-only, in `buzz-scene`: a stroke crossing a tile boundary lands in both tiles and
the boundary rows are continuous — **the seam regression test**; clone-then-stroke
leaves `Arc::ptr_eq` true for untouched tiles (the undo-cost proof); `composite_stroke`
matches the old `Canvas` stamp semantics against a golden; a filter forks every tile
exactly once. Headless GPU: a three-tile canvas reads back equal to the CPU composite,
and blob identities are unchanged for tiles away from the stroke. Serde: tile PNG round
trip, and a v19 file loads.

### Size and dependencies

**L.** Depends lightly on Wave 4 (filters and resample as tasks). Independent of 5
and 6.

---

## Wave 8 — The asset pipeline: watched folders (S)

### Decision

A `notify` watcher whose events do nothing but **schedule a debounced rescan**;
`AssetLibrary::rescan` (`buzz-doc/src/assets.rs:143`) stays the single source of truth.

*Rejected: applying file events incrementally.* Renames arrive as pairs that can be
split, editors write through temp files, and a copy of a hundred assets is an event
storm. Rescan is already milliseconds and is already correct.

### Shapes

```rust
// crates/buzz-doc/src/watch.rs — new
pub struct AssetWatcher { /* watcher + receiver */ }

impl AssetWatcher {
    pub fn new(roots: &[PathBuf]) -> notify::Result<Self>;
    /// Drains events; true if anything relevant changed.
    pub fn poll_dirty(&self) -> bool;
}
```

`App` polls per frame; dirty sets a deadline 500 ms out, pushed back by further events.
On the deadline, a `TaskKind::AssetScan` on `Pool::Background` builds a **fresh**
`AssetLibrary` off-thread and it is swapped in on completion — never mutated in place
while the panel is drawing it. New files feed the Wave-1 thumbnail cache (keyed by
path, mtime and size).

### UI

A "watching" dot and a pause toggle in the Assets panel; extra watched folders in
preferences. Closes §7-83.

### Testing

A temp-directory integration test: drop a file, assert dirty within the deadline and
present after the rescan; write rapidly and assert exactly one rescan is scheduled; an
unreadable root still reports through the existing `last_error` rather than looking
empty.

### Size and dependencies

**S.** Depends on Wave 4 and on Wave 1's thumbnails.

---

## Wave 9 — 2.5D: keyframed lights, depth sorting, real depth of field

### 9a · Keyframed lights (M)

Today `LightRig` is a plain field on `Scene` (`buzz-scene/src/lib.rs:180`) with no
notion of frames, so a sun cannot swing through a shot the way the camera can (§7-47).

**Decision:** a per-light `LightTrack` mirroring `CameraTrack`, with a **per-light,
frame-resolved** fingerprint and cache.

*Rejected: keyframing the whole rig as one unit.* Lights would not be able to animate
on independent timings, and it preserves the all-or-nothing cache clear that already
costs 305 ms.

```rust
// crates/buzz-scene/src/light_track.rs — new
pub struct LightKey {
    pub frame: u32,
    pub position: Point, pub intensity: f32, pub color: Color, pub radius: f32,
    // …the animatable fields of Light
}

pub struct LightTrack { pub enabled: bool, keys: Vec<LightKey> }

// Light gains:  pub track: Option<LightTrack>

impl LightTrack {
    pub fn state_at(&self, frame: u32, base: &Light) -> Light;   // as CameraTrack::state_at
}

impl LightRig {
    pub fn resolved_at(&self, frame: u32) -> Cow<'_, LightRig>;  // Borrowed when nothing animates
    pub fn light_fingerprint_at(&self, index: usize, frame: u32) -> u64;
}
```

**The cache-miss problem, and its answer.** `LightRig::fingerprint()`
(`buzz-light/src/lib.rs:286`) is the key the shading cache is built on. If lights
animate, a moving light's fingerprint changes every frame, so its crescents rebuild
every frame — by construction, and that cannot be designed away. What can be designed
away is everything else:

1. **Per-light keys** mean a static light keeps hitting the cache while its neighbour
   animates. Today one moving light rebuilds *all* of them.
2. **Parallel geometry** from Wave 4 makes one light's rebuild a fraction of the old
   single-threaded whole-rig cost.
3. **Quantise** the resolved state in the hash — position to 0.1 unit, intensity to
   1/256 — so scrubbing back and forth near a key re-hits rather than thrashing.

The regression test for this is precise: **animating light A must not evict light B.**

**Serialisation:** `LightKeyDto` copied from `CameraKeyDto`
(`buzz-doc/src/serial.rs:710`), `#[serde(default)]` on `track`.

**UI:** selecting a light shows its channel in the timeline, reusing the camera row's
widgets; the on-stage gizmos (`buzz-app/src/lights.rs`) gain a keyframe diamond;
dragging a gizmo writes a key at the playhead the way the camera does. Closes §7-47.

### 9b · Depth sorting (S)

`Layer.depth` (`buzz-scene/src/layer.rs:188`) drives parallax and projection but
explicitly does **not** reorder drawing, so two layers that cross in space still draw
one wholly in front of the other (§7-60, §7-65).

**Decision:** an opt-in `sort_by_depth: bool` on `StageProperties`, sorting **mask
groups** after grouping, plus a stable per-object `Spatial.z` sort within a layer.

*Rejected: always on.* Existing documents rely on paint order being the timeline's;
silently reordering them would change films that are already finished.

Paint order is decided in one place — `paint_order()` (`layer.rs:360`) feeding
`drawable_at`, walked at `buzz-render/src/document.rs:290`, with objects in stored
keyframe order at `document.rs:650`. The sort goes in between, and it has one hard
constraint: **masking assumes a mask owns an unbroken run of layers below it**
(`document.rs`). So the runs are built first and sorted **as units**; a mask and the
layers it clips move together or the mask stops meaning anything.

Stable sort throughout, so ties keep the timeline's order — which makes the feature
free for anyone who leaves every depth at zero, and gives the opt-out an exact
definition: identical output.

### 9c · Depth of field, the honest half (M)

Wave 6 ships geometric DOF by reusing the per-shape blur hook. This replaces it when
enabled: bucket layers into K depth slices (4–6), render each through the existing
pipeline into the compositor's ping-pong targets, blur each by its circle of confusion,
and composite back to front.

This is the classic multi-plane camera blur, and it is the honest model for a layer
tool: Vello emits no per-pixel depth buffer, so a true per-pixel defocus is not
available at any price. `aperture` becomes a keyable field on `CameraTrack`, following
the same DTO pattern. Closes §7-29 properly.

### Format

**Version 20 → 21** — light tracks, `sort_by_depth`, and camera aperture in one bump.

### Testing

Interpolation is pure: endpoints, midpoint, a single key, and `track: None` returning
the base light. A static rig's fingerprint is constant across frames. The per-light
cache survives a sibling animating. Paint order: a masked run stays contiguous after
sorting; equal depths are byte-identical to unsorted; opting out is byte-identical to
today. A golden render of two overlapping rectangles with swapped depths flips.

### Size and dependencies

**9a M · 9b S · 9c M.** 9a and 9b depend on Wave 4; 9c depends on Wave 6.

---

## Wave 10 — The film: `.buzzproj` (M)

### Decision

**A lightweight project file listing member `.buzz` shots, with the film produced by
the Wave 5 export queue plus ffmpeg's concat demuxer.**

The alternative deserves an honest weighing, because it is the obvious one.
*`Vec<Scene>` inside one `.buzz`* buys in-app scene switching and a single portable
file. It costs a format break that makes `History`, autosave, crash recovery and every
panel scene-indexed — an L+ change touching nearly everything, for a stated goal that
is "multiple shots rendered as one film". That goal is **export orchestration**, and
the project file solves it at size M with **no document-model change at all**, reusing
the queue and registry verbatim. Shots stay independently editable, independently
openable, and independently versionable in git, and `Scene::extract`/`Scene::merge`
already move content between them. Nothing here forecloses the single-file design if
it is ever wanted for a different reason.

```rust
// crates/buzz-doc/src/project.rs — new. Its own JSON versioning, NOT the .buzz format.
#[derive(Serialize, Deserialize)]
pub struct Project { pub version: u32, pub name: String, pub shots: Vec<Shot> }

#[derive(Serialize, Deserialize)]
pub struct Shot {
    pub path: PathBuf,              // relative to the .buzzproj
    pub range: Option<FrameRange>,
    pub angle: Option<String>,      // a named camera angle — see Wave 10b
    pub enabled: bool,
}
```

**Exporting a film.** Validate first: every shot must encode with the *same* preset,
because `-c copy` concatenation requires matching streams. A frame-rate mismatch
between documents is surfaced as a warning with a re-encode escape hatch rather than a
corrupt file. Then enqueue one `ExportRequest` per shot to a temporary segment, and a
final `TaskKind::ConcatFilm` task running

```
ffmpeg -f concat -safe 0 -i list.txt -c copy -fflags +genpts out.mp4
```

Timestamp trouble is avoided because *we* encoded every segment with identical
settings; audio gaps are avoided by encoding a silent track into any segment whose shot
has no sound.

**UI:** a small Project panel — reorderable shot list, enable checkboxes, double-click
to open a shot, "Export Film…". `File ▸ New/Open Project`, and recent projects.

**Testing:** serde and relative-path resolution round-trip; a headless film test
(skipped without a GPU) concatenating two four-frame shots and checking with `ffprobe`
that the duration is the sum and there is one video stream; the validator rejecting
mismatched frame rates.

**Size: M.** Depends on Wave 5.

---

## Wave 10b — Camera angles: stage once, shoot from anywhere (S–M)

The direct answer to *"I have to set the scene up again for a different angle, and that
takes time as well."*

### The idea

A staged scene is furniture, characters, lights and depth. **An angle is a camera
state, not a new scene.** The camera already carries everything an angle needs —
centre, zoom, rotation, pitch, yaw, focal distance — and already projects the stage in
real perspective with per-layer parallax. What is missing is the ability to *name* a
camera state and come back to it.

```rust
// crates/buzz-scene/src/camera_track.rs — alongside CameraTrack
pub struct NamedAngle { pub name: String, pub state: CameraKey }

// CameraTrack gains:  pub angles: Vec<NamedAngle>
```

Three things fall out of that one field:

1. **An Angles panel.** Save the current camera as "Wide", "Close on Ana", "Reverse".
   Click to jump. Nothing about the scene changes — you are moving the camera, not
   rebuilding the set.
2. **"Cut to angle at playhead"** writes a stepped camera key, so an entire
   multi-angle sequence — wide, close, reverse, back to wide — lives on **one
   timeline of one staged scene**. The cuts are keyframes; the staging is done once.
3. **`Shot.angle`** in `.buzzproj` (above). The same staged `.buzz` becomes N shots at
   N angles in the finished film, re-staging nothing.

Scene templates (Wave 3) carry the angle list with them, so a template can arrive with
its coverage already blocked out.

### The honest limit

This must be written down rather than discovered: **flat art seen edge-on is flat.**
The believable envelope for 2.5D cards is moderate pitch and yaw — enough for a camera
to discover a set rather than slide past it, not enough for a true reverse angle on a
character built from one flat drawing. A genuine reverse needs a second drawing, which
is what a turnaround in the pose library (Wave 2) is for.

Two things widen that envelope, and both are already in this plan: **depth sorting**
(9b), so cards that cross in space resolve correctly instead of one drawing flatly in
front, and **parallax**, which already works. An angle change reads as a new view
mostly because of what moves *relative to what* — which is depth doing the work.

**UI:** Angles panel (dockable, thumbnail per angle from the Wave-1 cache);
`Camera ▸ Save Angle…`, `Camera ▸ Cut to Angle at Playhead`.

**Testing:** save/apply round-trips the camera state exactly; "cut at playhead"
produces a stepped key (no interpolation across the cut); angles survive the format
round-trip; a template carries them.

**Size: S–M.** Serialises in the v21 bump alongside Wave 9. Best after 9b, which is
what makes a re-angle worth looking at.

---

## Dependencies, formats, and what closes

```
Wave 4  Foundations ──┬──► Wave 5  Export service ──► Wave 10  Film ──► Wave 10b Angles
                      │                                                      ▲
                      ├──► Wave 7  Raster layers                             │
                      ├──► Wave 8  Asset watcher                             │
                      └──► Wave 9a Lights, 9b Depth sort ────────────────────┘

Wave 6  Compositor ──────► Wave 9c  Sliced depth of field
```

Waves 6 and 7 can proceed alongside 5. Wave 4 gates everything.

| Format | Wave | Adds |
|---|---|---|
| **19** | 6 | `PostSettings` on the stage |
| **20** | 7 | Raster layers and their tiles |
| **21** | 9 + 10b | Light tracks · `sort_by_depth` · camera aperture · named angles |

`.buzzproj` versions on its own, independently of the document format. Each bump gets a
DTO with `#[serde(default)]` and a strip-the-field back-compat test, following the
pattern already in `buzz-doc/src/serial.rs`.

**§7 rows these close:** 25 (tablet pressure) · 29 (depth of field) · 32 (scripts on
the UI thread) · 47 (keyframed lights) · 83 (assets folder not watched) · 154 (lighting
single-threaded) · 155 (305 ms first frame) · 164, 165, 166, 167 (the four raster
limits). §7-60 and §7-65 are addressed by 9b's opt-in design rather than closed
outright, since the default stays as it is.

---

## What could go wrong

Written before the work, so none of it is a surprise.

1. **VRAM per export.** Each job owns a full GPU context. The serial queue bounds this
   to one; assert it, and drop the `Exporter` — device included — between jobs.
2. **Tile seams under Vello's antialiasing.** Adjacent tile images at fractional
   transforms can show a one-pixel seam. Plan A: snap the canvas group to integer
   device pixels when the transform is axis-aligned. Plan B, designed in from the
   start: duplicate one pixel of shared edge per tile. Either way the headless seam
   readback test is what decides.
3. **ffmpeg concat.** Mismatched stream parameters break `-c copy`. Prevented by
   same-preset validation, `+genpts`, and silent-audio padding.
4. **The wgpu 29 pin.** `egui 0.35` and `vello 0.9` must agree on wgpu, so the
   `Compositor` is format- and device-parameterised — a future migration changes its
   construction, not its API.
5. **Script cancellation needs a safepoint.** Without the interpreter fuel hook the
   worst case is an abandoned detached thread, so the fuel counter is not optional in
   Wave 4.
6. **`notify` on network drives** misses events and storms. The debounce-then-rescan
   design reconciles regardless; fall back to `PollWatcher` where the platform watcher
   is unreliable.
7. **Raster filter undo memory.** A whole-canvas filter forks every tile once per
   application. History depth caps it, but a byte budget on `History` is the real
   answer and is noted as future work.
8. **`octotablet` and winit's window handle.** Feature-gated, degrading to constant
   pressure, so it can never be the reason the program does not start.

---

# Part III — the delight waves

Part II makes the program fast, safe and capable. Part III is what makes it *good* —
and each one is grounded in a subsystem that already exists, so none is a moonshot.

## Wave 11 — Animation feel (M)

The review-and-timing toolkit, which is where animation quality actually comes from.

- **Motion trails and arcs.** Sample a selected object's transform across ±N frames —
  the tween path already evaluates at any frame — and draw the arc on the stage with a
  tick per frame. Bunched ticks are slow, spread ticks are fast, and a lumpy arc is the
  single most common reason motion looks wrong. Dragging a tick retimes it.
- **Audio scrubbing.** Play an ~80 ms window at the playhead while it is dragged.
  `buzz-audio` already decodes and plays; a scrub is a seek plus a short play. Nothing
  else makes lip sync and timing-to-music as fast.
- **Video reference layer.** Import a video with the ffmpeg already depended on,
  decode to an `ImageAsset` sequence on a Guide-kind layer — which is already excluded
  from export. That is rotoscoping, for the cost of a decode loop.
- **Frame labels and beat markers.** Comments on the ruler, plus markers detected from
  the soundtrack by onset detection in `buzz-audio`, so cutting to music is visual.

## Wave 12 — Procedural motion: the modifier stack (L)

**The differentiator.** A `Vec<Modifier>` on an object or a bone, evaluated at draw
time and **deterministic in `(object id, frame)`** so exports are reproducible and
nothing is ever baked:

- `Wiggle { amplitude, frequency }` — camera shake, idle sway, a held pose that
  breathes instead of freezing.
- `Spring { stiffness, damping }` on a bone — **automatic follow-through and overlap**
  for hair, cloth, tails and antennae, evaluated as a damped response to the keyframed
  motion. Fixed-step, cached per contiguous frame range, invalidated by the owning
  span's edit.
- `LookAt { target }` — eyes and heads that track a point.
- `AutoSquashStretch { amount }` — driven by velocity.

Secondary animation is the most labour-intensive thing an animator does by hand and the
first thing cut when time is short. Getting it for free is the largest single lever in
this document on "faster **and** more beautiful".

## Wave 13 — Drawing delight (M)

- **Pull-string stabiliser** for brush and pencil, beyond the current smoothing — the
  difference between a shaky line and a confident one.
- **Symmetry drawing**, mirror X/Y/radial: duplicate the in-progress path through an
  `Affine`, which the path pipeline makes nearly free.
- **Perspective guides** — vanishing-point rulers, snapped like the existing guides.
- **Gap-aware paint bucket**: close gaps up to a tolerance before flooding, which is
  Animate's Gap Size and the reason its bucket feels forgiving.
- **Gradient maps and paper-texture fills** as paint variants.

## Wave 14 — Pro output (M)

- **True motion blur at export.** Render K sub-frame samples and average them on the
  GPU. The Exporter owns its device, and tweens and the camera already interpolate at
  fractional frames, so this is an accumulation buffer and little else — and it is
  quality nothing in the Animate world ships. The stage shows a cheap compositor
  approximation.
- **Alpha video** — ProRes 4444 or VP9+alpha WebM through the same ffmpeg pipe — so
  the work composites into other tools.
- **Render region**: export a marquee rather than the whole stage.
- **Stylisation passes** in the compositor: posterise, halftone, hatching.

## Wave 15 — Command and control (S)

- **Command palette** on `Ctrl+K`, fuzzy-matching the existing `Command` catalogue.
  The catalogue, the labels and the shortcuts are all already data
  (`buzz-ui/src/command.rs:14`), so this is a search box over a list that exists.
- **Shortcut editor** — same argument: the map is already data.
- **Saved commands** menu for Actions-panel scripts, which takes the sting out of
  §7-30 (a script lives in the panel, not in the document).
- **Named version snapshots** — checkpoints through the autosave machinery, so "before
  I changed the ending" is a thing you can return to.

## The horizon

Named so that leaving them out is a decision rather than an oversight.

- **ML-assisted inbetweening and colourisation.** Research-grade, heavy dependencies,
  and a very different kind of correctness argument. Not now; not never.
- **HTML5 runtime export** (CP-6.4) — already on the roadmap, unchanged by this plan.
- **Multiple scenes in one file** — deliberately superseded by `.buzzproj` (Wave 10)
  unless a reason appears that the project file cannot serve.
- **Collaborative review** — comments pinned to a frame, for a director rather than an
  animator.

## Suggested order after Part II

```
15 (S, immediate) → 11 → 13 → 14 → 12
```

The modifier stack goes last: it is the biggest, and it is much better with keyframed
lights and a motion editor already in place.

---

## How this file is kept

Same rule as the rest of the record: **a design that is not written down has not been
decided.** When a wave is built, `PROGRESS.md` §4 gets a section saying what was built
and *why it was built that way*, the §7 rows it closes are struck through, and the wave
here is marked shipped with a pointer to that section. Where the built thing differs
from this design — and it will — the difference and its reason go in §4, and this file
is corrected. If this file and `PROGRESS.md` ever disagree, `PROGRESS.md` is right.
