# Symbol Encoding Cache — execution log

Working doc tracking the fix for Animate-import lag. Folds into
ARCHITECTURE.md Wave 16 when done, then this file is deleted.

## Why

An imported Animate document is instance-heavy: a symbol placed N times is
re-walked and re-encoded N times per frame in the Vello draw walk. Measured on
a 60k-shape fixture (`buzz-render/tests/encode_cost.rs`): ~37 ms/frame to
encode zoomed-to-fit, paid again on every pan/zoom/scrub/playback frame. That
is the "extremely laggy" import. Instance culling (already landed) fixes the
zoomed-in case; this cache fixes the general case.

## Mechanism

Encode each `(symbol, inner-frame)` once per frame-generation into its own
`vello::Scene`, then stamp it at each instance with
`Scene::append(child, Some(stamp))`. 300 encodes become 1 build + 300 appends.

## Steps

- [x] **1. Extract `draw_symbol_contents`** — pure refactor; live draw and the
  future cache build share one function so they cannot drift. No behavior
  change; full suite green.
- [ ] **2. `SymbolTable` memo** — per-symbol fingerprint (transitive),
  resolved bounds (all frames, via library), safety flags (filters,
  group_blend, non_flat, inverse_mask, additive). Cycle-safe DFS,
  revision-validated, Arc-pinned.
- [ ] **3. `has_additive_paint` → memo** — Instance arm consults the memo,
  killing the remaining per-frame library walk in `draw_layer`.
- [ ] **4. `SymbolSceneCache` + eligibility + stamp** — default OFF. Render-space
  child build, orthogonality-gated stamp via `append`.
- [ ] **5. GPU parity suite** — cache on vs off, tolerance ≤2 LSB / ≥99.9%
  exact, across translation/rotation/nesting/masks/gradients/looping/etc.
- [ ] **6. Perf gate** — `encode_cost.rs`: reuse-on encode ≥3× cheaper.
- [ ] **7. Flip default ON + `BUZZ_NO_SYMBOL_CACHE` hatch + docs.**

## Safety contract (why parity holds)

- **Eligibility** (else fall back to the live walk — always correct): no
  tint/faded/ghost/adjust/blur/lighting; composed colour effect identity;
  affine projection; **composed linear part orthogonal** (stroke & seam-seal
  widths only stay screen-correct under rotation/translation/reflection, since
  widths scale by view-zoom S alone, not by the instance transform); symbol
  content free of filters, group blends, non-flat spatial, inverse masks;
  size and translation within f32-safe bounds.
- **Stamp math** (f64 until the final append): child encodes `C(p)=S·(p−a_c)`;
  the stage needs `R(p)=gpu_view∘(S·(A·p−a))` where `A = projection∘doc`.
  Uniform S commutes with A's linear part, so
  `R = gpu_view ∘ [A_lin, S·(A·a_c − a)] ∘ C`.
- **Invalidation**: fingerprints are transitive, so editing a nested symbol
  invalidates every symbol that instances it (editing B forks B's Arc but not
  its parent's).

## Escape hatches
`BUZZ_NO_SYMBOL_CACHE=1` (this change), `BUZZ_NO_RETAIN=1`, `BUZZ_POLL=1` —
each pipeline stage independently bisectable.
