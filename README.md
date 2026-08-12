# BuzzAnimate

GPU-accelerated vector animation. A from-scratch alternative to Adobe Animate,
targeting the three limits Animate inherited from 1996-era Flash:

| | Adobe Animate 2024 | BuzzAnimate |
|---|---|---|
| Maximum zoom | 2 000% | **no cap** — verified to 2×10¹⁴% |
| CPU usage | effectively single-threaded | **work-stealing pool across all cores** |
| Rasterisation | CPU | **GPU compute shaders (Vello)** |

Built clean-room. No decompilation of Adobe binaries and no reuse of Adobe
assets, icons, or trademarks. Formats read are published by Adobe (SWF,
AVM2/ABC), ISO standards (PDF), or plain XML (XFL).

---

## Status: Phases 0–3 complete

BuzzAnimate is now a working editor. It opens on an Animate-shaped window —
menu bar, tool strip, rulers around a white stage on a grey pasteboard, layers
and properties on the right, timeline along the bottom — and you can draw,
select, transform, edit anchor points, arrange, group, save, reopen and rely on
autosave.

Animate's **Merge Shape** model works as it should: overlapping fills of the
same colour fuse, a different colour cuts. Object Drawing is the alternative,
on the `J` toggle.

Phase 3 added the **timeline**: keyframes and frame spans with Animate's
F5/F6/F7 behaviour, a frame grid using Animate's drawing conventions, playback
that runs on wall-clock time rather than frames rendered, onion skinning, and
an animated **camera** whose keys interpolate — zoom geometrically, rotation by
the shortest way round.

Not yet implemented, and honestly marked as such in the toolbar: tweening,
gradients, Text, Lasso, Bézier pen authoring, multiple Scenes, clipboard. See
`PROGRESS.md` §7 for the full list.

---

## The engine, verified in Phase 0

Phase 0 exists to prove the three claims above before any authoring features are
built on top of them. All three are verified by automated tests against real
hardware, not asserted.

### Verified results

Measured on an i7-14700K (20C/28T) with an RTX 5060 Ti, via
`cargo test -p buzz-app --test headless_zoom --release -- --nocapture`:

```
 gen      zoom  precision    ink%  colours     encode       gpu  drawn/culled
   0  2e2%   3.64e-13   4.06%       48     0.21ms    10.8ms  43/181
   5  2e7%    3.64e-8   4.02%       47     0.10ms     0.8ms  70/154
  10  2e12%   3.64e-3   4.01%       54     0.10ms     0.9ms  70/154
  12  2e14%   3.64e-1   4.01%       54     0.08ms     0.8ms  72/152
```

Ink coverage is **constant at ~4% across thirteen decades of zoom** — rendering
quality is scale-invariant. Generation 0's 10.8 ms is one-off shader warmup; it
draws fewer items than generation 2, which takes 1.0 ms.

### The zoom mechanism

Animate's 2000% cap is a numeric limit, not a policy: Flash stores coordinates
as twips (1/20 px fixed point) and rasterises in `f32`. BuzzAnimate stores `f64`
and splits the view transform into three stages so nothing large ever reaches
the GPU:

```
1. CPU, f64:  q = p − anchor        well-conditioned; result is tiny
2. CPU, f64:  r = q × zoom          result is viewport-sized
3. GPU, f32:  s = rotate(r) + offset   unit scale; nothing large to lose
```

**Steps 1 and 2 must not be fused into one matrix.** A composed affine evaluates
`zoom·p − zoom·anchor`, which is the difference of two huge numbers and destroys
the answer. Both `buzz-geom` and `buzz-render` carry tests that fail if anyone
fuses them.

Applying the scale on the CPU (step 2) was not in the original design and was
added after measurement: leaving that multiply to the GPU cost **25 ms/frame at
1e12% zoom versus 0.9 ms**, and visibly degraded detail.

### Known precision limit

Rebasing removes catastrophic cancellation, but `f64` storage of an absolute
document coordinate leaves a floor:

```
precision_px ≈ |coordinate| × 2.22e-16 × zoom
```

For coordinates near 1e3 that is sub-pixel out to about **1e12%** — roughly ten
orders of magnitude past Animate — and degrades linearly rather than collapsing
after that. `Camera::screen_precision_px()` reports it live in the HUD.

---

## Running it

```sh
cargo run --release -p buzz-app
```

| Input | Action |
|---|---|
| Mouse wheel | Zoom about the cursor, unbounded |
| Drag | Pan |
| `R` | Reset / fit |
| `Esc` | Quit |
| HUD buttons | Jump to 2000% / 1e6 / 1e9 / 1e12, stress all cores |

Flags: `--gpu <name-or-index>` forces an adapter, `--integrated` prefers iGPU.
The adapter table is printed at startup.

### Why adapter selection is real code

This machine enumerates seven adapters, including `Microsoft Basic Render
Driver` — a CPU rasteriser that would make "the GPU build" slower than Animate,
silently. Every adapter is scored and the table is logged:

```
   [0]   1110  NVIDIA GeForce RTX 5060 Ti     DiscreteGpu/Vulkan
   [1]    390  Intel(R) UHD Graphics 770      IntegratedGpu/Vulkan
-> [2]   1120  NVIDIA GeForce RTX 5060 Ti     DiscreteGpu/Dx12      <- selected
   [6]    n/a  Microsoft Basic Render Driver  Cpu/Dx12              <- disqualified
```

---

## Layout

| Crate | Role |
|---|---|
| `buzz-geom` | `f64` geometry: rebasing camera, clipping, booleans, hit-testing, path editing |
| `buzz-jobs` | Two-pool work-stealing job system; per-worker CPU metrics |
| `buzz-render` | GPU adapter selection; Vello scene building |
| `buzz-scene` | Copy-on-write document model; Animate's six layer types; R-tree index |
| `buzz-doc` | `.buzz` format, undo history, autosave |
| `buzz-ui` | Theme, menus, shortcut map, tool catalogue, panels, snapping |
| `buzz-app` | Window, frame loop, editor state, tool behaviour, stage rendering |

Remaining crates (`buzz-scene`, `buzz-timeline`, `buzz-import-xfl`,
`buzz-avm`, …) arrive with their phases; empty placeholders would only be noise.

### Dependency pinning — read before upgrading

`vello 0.9` requires **wgpu ^29**, and `egui-wgpu 0.35` is the newest egui that
also uses wgpu 29. `egui 0.36` moved to wgpu 30.

Two majors of wgpu in one binary produce structurally distinct `Device` types
that cannot share a surface. **Do not bump egui past 0.35 until vello moves to
wgpu 30.** Verify with:

```sh
grep -A1 '^name = "wgpu"' Cargo.lock   # must list exactly one version
```

---

## Testing

```sh
cargo test --workspace            # 395 tests
cargo clippy --workspace --all-targets
cargo test -p buzz-app --test headless_zoom --release -- --nocapture
```

The headless zoom test drives the *same* encoding path the window uses, so what
is verified offscreen cannot drift from what is drawn. It skips cleanly when no
GPU is present.

---

## Roadmap

Phase 0 complete. Next: **Phase 1 — geometry and document core** (boolean ops,
copy-on-write scene graph, R-tree spatial index, `.buzz` format, undo).

Then: drawing tools and UI shell · timeline · symbols and tweens · importers
(`.fla`/`.xfl`, `.pdf`/`.ai`, `.swf`) · export (MP4 via NVENC, PNG, GIF, HTML5)
· rigging and IK · scripting and ActionScript.

**CP-1.1 done:** document-space clipping (`buzz_geom::RenderClip`) has replaced
Phase 0's culling, so shapes far larger than the viewport are now drawn
correctly instead of vanishing. Items drawn at 2×10¹⁴% went from 70 to 213 with
GPU time still 0.8–1.7 ms.
