# Symbol sets — pick a symbol from a menu, keyed at the playhead

Written 2026-10-10. Request (user, paraphrased from two messages): "a menu in
symbols that can have more symbols included. All the symbols are on the same
level and each has its own timeline from 0. From Properties I select a symbol
and it is put on stage at the same frame number, or I can choose. It's like
instant Swap, but all included in one. No going inside a symbol, no changing
frame numbers by hand."

So this is **not** a frame picker and **not** nested child symbols. It is a
named group of sibling library symbols (e.g. `Raju_idle`, `Raju_wave`,
`Raju_sit`) attached to one symbol, plus a Properties dropdown on any instance
of a member that swaps the instance to another member **from the playhead on**.

## Status

| Step | What | State |
|---|---|---|
| S1 | Model: `Symbol.set`, `Library::symbol_set / include_in_set / remove_from_set`, cleanup on symbol delete, merge remap | done 2026-10-10 — `symbol::tests::{a_symbol_set_is_found_from_the_owner_and_from_a_member, a_symbol_set_skips_missing_ids, include_in_set_refuses_self_duplicates_owners_and_other_sets, include_via_a_member_goes_to_the_owner, deleting_a_symbol_strips_it_from_sets}`, `merge::tests::a_symbol_set_is_remapped_through_a_merge` pass |
| S2 | Save/load: `SymbolDto.set` (optional, old files load) | done 2026-10-10 — FORMAT_VERSION 46→47 (file convention: every additive field bumps); `serial::tests::{a_symbol_set_round_trips, a_symbol_from_an_older_file_loads_with_no_set}` pass (buzz-doc lib 233/233) |
| S3 | `Scene::pick_from_set(frame, id, to, start)` — key at playhead, swap, set first frame | done 2026-10-10 — `tests::{pick_from_set_keys_at_the_playhead_and_continues_the_inner_frame, pick_from_set_clamps_a_play_once_instance, pick_from_set_from_start_and_on_an_existing_keyframe, pick_from_set_refuses_a_symbol_outside_the_set}` pass |
| S4 | Properties panel: **Set** dropdown, **Start** choice, **Include…** / remove menu | done 2026-10-10 — headless click-through via real `App::build_ui`: `app::symbol_set_tests::picking_a_set_member_in_properties_keys_it_at_the_playhead_and_undoes` passes (open Set, click wave → frame 6+ wave, 5 idle, undo restores). Set combo names the symbol shown *at the playhead*. Include…/Remove menus not click-tested |
| S5 | AI agent access to the same (read members, pick, include) | done 2026-10-10 — element fields `symbol_set` (read-only list), `pick_from_set` (name or {symbol,start}; reads symbol at playhead), `include_in_set` / `remove_from_set` (name; read the accepted names). Skipped in the generic every-field loop (write≠read shape), covered by `a_symbol_set_is_built_picked_and_undone_through_set_property`; `ai_agent_coverage` 41 passed, 0 failed, 1 ignored (pre-existing) |
| S6 | Tests green: buzz-scene, buzz-doc, buzz-ui, buzz-app lib + agent coverage | done 2026-10-10 — `cargo test -p buzz-scene` 542/0; `-p buzz-doc` 233/0; `-p buzz-ui` lib 370/0 (one earlier run had `timeline_panel::tests::shift_click_sweeps_a_range_and_ctrl_click_toggles_one` fail, passes alone and on rerun — flaky, not touched here); `-p buzz-ui --test dock_columns` 5/3 FAIL — Layers / Layer Depth heights, no instance selected so the new rows are not drawn; not caused by this change but not proven pre-existing either; `-p buzz-app --lib symbol_set_tests` 1/0; `--test ai_agent_coverage` 41/0 (1 ignored, pre-existing); `cargo build -p buzz-app` ok |
| review | Review: rerun tests, read the diff | **accepted with one fix, 2026-10-11.** Read `pick_from_set`, `Library` set functions and the panel code. Bug found and fixed: `instance_properties` (and the agent's `element` instance path) read the *earliest* copy of the instance and wrote every field back to the playhead's keyframe, so after a pick any other instance edit on that frame put the old symbol back. Both now read the copy at the playhead. New test `app::symbol_set_tests::editing_an_instance_after_a_pick_does_not_undo_the_pick` (fails with the old read, checked by reverting it) + a step in `a_symbol_set_is_built_picked_and_undone_through_set_property`. Reran: buzz-scene 542/0, buzz-doc 233/0, buzz-ui lib 370/0, buzz-app `symbol_set_tests` 2/0, `instance` 12/0, `propert` 1/0, `ai_agent_coverage` 41/0. Not run: full buzz-app lib suite, `dock_columns` (3 failures, untouched code). Not checked by eye in the real app. |

## Design

### S1 — the model (`crates/buzz-scene/src/symbol.rs`, library)

- `Symbol` gains `pub set: Vec<SymbolId>`: the *other* symbols included in
  this symbol's set. The symbol holding the list is the set's **owner**; the
  listed ones are **members**. Default empty.
- `Library::symbol_set(id) -> Vec<SymbolId>` — the full set an instance of
  `id` can pick from, owner first, then members in stored order:
  - if `id`'s own `set` is non-empty, `id` is the owner;
  - else the first symbol whose `set` contains `id` is the owner;
  - else the result is empty (no set — the panel hides the dropdown).
  Ids no longer in the library are skipped; no duplicates; owner never listed
  twice.
- `Library::include_in_set(owner, member) -> bool` — refuses (false): self,
  unknown ids, a member already in this set, a `member` that owns a set of its
  own, a `member` already in another owner's set (one set per symbol keeps the
  lookup unambiguous). `owner` may itself be a member of another set → include
  into *that* owner instead (resolve owner first), so the user can include from
  whichever member is on stage.
- `Library::remove_from_set(owner, member) -> bool`.
- Deleting a symbol from the library strips its id from every `set`.
- `merge.rs` (paste / import from another document): remap `set` through the
  symbol id map; drop ids that were not copied.
- Every `Symbol { .. }` struct literal in the workspace gets `set: Vec::new()`
  (or uses `Symbol::new`).

### S2 — save/load (`crates/buzz-doc/src/serial.rs`)

`SymbolDto.set: Vec<u64>`, `#[serde(default, skip_serializing_if = "Vec::is_empty")]`,
written in `from_symbol`, read in `to_symbol`. Follow the file's existing
convention on whether an additive optional field bumps the format version.
Tests: round trip keeps the set; a symbol without the field loads with an
empty set.

### S3 — the pick (`crates/buzz-scene/src/lib.rs`)

```rust
pub enum SetStart { SameFrame, FromStart }
pub fn pick_from_set(&mut self, frame: u32, id: ObjectId, to: SymbolId, start: SetStart) -> bool
```

1. Find the instance `id` (current timeline — the stage, or the open symbol).
   Refuse if `to` is not in `library().symbol_set(instance.symbol)` or equals
   the current symbol.
2. Work out the inner frame the instance shows **now**:
   `instance.resolve_frame(kind, elapsed, len)` with `elapsed = frame −
   start of the keyframe owning frame on the object's layer` (same as
   `elapsed_on` in `editor.rs` / `buzz-render`).
3. `ensure_keyframe_for(frame, id)` — **always**, regardless of auto-key, so
   frames before the playhead keep the old symbol. If the playhead is already
   on that keyframe, nothing is inserted and the whole keyframe changes.
4. `update_object_at(frame, id, …)`: `symbol = to`; `first_frame` =
   - `SameFrame`: the inner frame from step 2, fitted to the new length
     (`% len` for Loop, `min(len − 1)` for PlayOnce / SingleFrame / movie clip);
   - `FromStart`: 0.
   Keep transform, loop mode, colour effect, `sync_to_root` and the object id.
   With `sync_to_root` on, the frame follows the stage anyway — still write
   `first_frame` as above (harmless, and right if sync is later turned off).
5. Nothing else moves: playhead, sound layers, lip sync and other layers are
   untouched (that is "same audio and frame position").

One call = one edit; the caller wraps it in one undo step ("Pick Symbol").

### S4 — Properties panel (`crates/buzz-ui/src/panels.rs::instance_properties`)

Inside the existing **Symbol Instance** section, right under the Symbol/Swap
row:

- **Set** row (only when `symbol_set` is non-empty): a ComboBox showing the
  current symbol's name, listing every set member. Picking another calls
  `scene.pick_from_set(at.frame, id, chosen, start)` after the widgets are
  drawn (same collect-then-apply pattern as the rest of the function).
- **Start** row (only with a set): ComboBox `Same frame` / `From frame 1`,
  remembered in egui memory (`ui.data_mut`, persisted, keyed
  `"symbol-set-start"`), default `Same frame`.
- **Include…** menu button (always shown, next to `Swap…`): lists library
  symbols that `include_in_set` would accept for this instance's set owner;
  clicking includes it. When the set has members, a **Remove** submenu lists
  them (never the owner, never the symbol currently shown).
- Hover text explains: "Swap this instance to another symbol in its set, from
  this frame on. Earlier frames keep the current one."

Returns `edited = true` so the app records the undo step the same way as other
instance edits.

### S5 — AI agent (`crates/buzz-app/src/agent/props.rs`)

User rule: the AI can do everything the user can by hand
(docs/AI_AGENT_PLAN.md). Follow the existing "Properties/Instance" pattern:
read `symbol_set` (names), write `pick_from_set` (by name, optional
`start: "same_frame" | "from_start"`), `include_in_set` / `remove_from_set` (by
name). Keep `ai_agent_coverage` green.

### S6 — tests

- buzz-scene: set lookup from owner and from member; missing ids skipped;
  include refusals (self, duplicate, member that owns a set, member of another
  set); include via a member resolves to the owner; delete strips ids; merge
  remaps.
- `pick_from_set`: frames before the playhead keep the old symbol, the playhead
  and after show the new one; `SameFrame` continues the inner frame (Loop wraps
  into a shorter symbol, PlayOnce clamps); `FromStart` gives 0; picking on an
  existing keyframe inserts none; transform / colour / loop / sync / id kept;
  a symbol outside the set is refused.
- buzz-doc: round trip + old file.
- Headless UI (one `egui::Context`, see
  `verify-ui-headlessly` notes in `app.rs` tests): select an instance of a set
  member, open the Set combo, click another member → the instance's symbol at
  the playhead changed and an undo restores it.

## Not in scope

- A Library-panel context menu for building sets (Properties covers it; add
  later if asked).
- Thumbnails in the dropdown.
- Director / episode_build use of sets.
