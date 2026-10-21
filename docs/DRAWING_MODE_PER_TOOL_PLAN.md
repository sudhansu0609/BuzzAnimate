# Merge / Object Drawing per tool and per brush

User, 2026-10-19: "when I draw two strokes with the Fluid brush and try to
select them, they are selected separately; with the Normal brush they are
selected together. Give every brush type, and the other drawing tools, an
option to keep strokes together or separate, like Animate's Object Drawing
mode."

## What was measured

Headless probe (`Editor::default()`, real pointer path, three crossing strokes,
Round/Square/Chisel/Ellipse tips, mouse and pen pressure, sizes 4–40): **Fluid
and Normal both merge to one object** in Merge Shape mode. Both go through
`ToolAction::AddShape` → `Editor::add_shape` → `lay_shape`. So the merge code is
not brush-specific. What does make strokes stay separate:

1. **Build up** (`BrushSettings::build_up`, shown for every brush). A non-Normal
   blend never merges (`merge_shape_into_layer`, guard at the top).
2. **Object Drawing** is one global checkbox at the bottom of the
   no-selection Properties panel (`panels.rs` ~3194), far from the tool options
   and not visibly tied to any tool.
3. **`J` does nothing.** `app.rs` ~2685 consumes the key and discards it.
4. Effect, Wave and captured-paint Pattern strokes go through
   `Editor::add_artwork` → `place_artwork`, which **ignores the mode**: every
   stroke becomes its own group.

## Design

Animate remembers Object Drawing **per tool**. Here, per tool and per brush
kind:

* `buzz_ui::style::ToolDrawingModes`: one `Option<DrawingMode>` per slot, for
  Pencil, Line, Pen, Rectangle, Oval, PolyStar, and each `BrushKind` (Fluid,
  Normal, Raster/Soft, Pattern, Art, Effect, Wave). `None` means follow
  `DrawStyle::drawing_mode` (the existing global field, still read by tests and
  the agent). Default: all `None`, except Effect and Wave = `Some(ObjectDrawing)`,
  which keeps today's behaviour for them.
* `DrawStyle::drawing_modes: ToolDrawingModes`.
* `DrawStyle::drawing_mode_for(tool, brush_kind) -> DrawingMode` and
  `set_drawing_mode_for(tool, brush_kind, mode)`. Non-drawing tools fall back to
  the global.
* `DrawStyle::set_all_drawing_modes(mode)`: global = mode, and every slot =
  `Some(mode)`. Used by the agent's `drawing_mode` field.

### Where the mode is read

* `Editor::add_shape`: `self.style.drawing_mode_for(self.tool(), self.style.brush.kind)`
  instead of `self.style.drawing_mode`.
* `Editor::paint_raster`: the Brush/Raster slot.
* `Editor::add_artwork`: the Brush slot for the current kind. In **Merge**
  mode the stroke **joins** what it lands on: find the topmost object on the
  layer/frame (visible, unlocked) whose bounds overlap the new pieces' bounds.
  If it is a plain `Group` of raw shapes (no symbol, rig or text), append the
  new pieces to that group's children, so stacking order never changes. If it
  is not, place the pieces as today, but always as a group, so the next stroke
  can join it. In Object mode: unchanged.
* `add_artwork_frames` (animated Wave): unchanged. Each frame is its own
  drawing.

### UI

* `tool_options_panel`: for Brush (each kind), Pencil, Line, Pen, Rectangle,
  Oval and PolyStar, a **Strokes** row with two selectable buttons, styled like
  the Symmetry buttons: **Merge together** and **Keep separate**. The Brush row
  reflects the current kind, so switching Fluid → Normal shows that brush's
  own setting. Hover text explains Animate's names (Merge Drawing / Object
  Drawing, `J`), that merged strokes are selected together, and, when Build up
  is on, that Build up strokes always stay separate.
* Remove the global Object Drawing checkbox from the Properties panel. It
  conflicts with per-tool settings.
* `J`: `Editor::toggle_drawing_mode()` toggles the current tool's slot, with a
  status line such as "Brush (Fluid): Keep separate". It does nothing for
  non-drawing tools.

### Persistence

Per person, like `brush_tips`: `Workspace::drawing_modes`
(`#[serde(default)]`), loaded in `Editor::new` and saved through a
`remember_drawing_modes()` that sits next to `remember_brush_tips()` at the
`app.rs` ~4522 call site.

## Constraints

* No new tests (user rule 2026-10-11). Existing suites must stay green.
* Never run `cargo fmt`.
* Tests set `style.drawing_mode` directly, and must keep working. That is why
  `None` falls back to the global field.

## Status

| Step | What | State |
|---|---|---|
| D1 | `ToolDrawingModes` + `DrawStyle` helpers + workspace persistence | done |
| D2 | Editor reads per-slot mode (add_shape, paint_raster) + `J` toggle | done |
| D3 | add_artwork join in Merge mode | done |
| D4 | Tool options Strokes row, remove Properties checkbox, agent setter | done |
| D5 | Verify: build, buzz-ui + buzz-app suites, probe | done (reviewer) |
| D6 | Object-drawn shapes are protected from later Merge strokes (`Object::drawing_object`) | done-verified 2026-10-19 (reviewer reran probe: 3 objects; probe: 2 Fluid separate + 2 Normal merged = 3 objects; Break Apart clears it on image paint only, plain shapes are a no-op there) |

## Verification

**D1–D4 review (2026-10-19).** Probe (`crates/buzz-app/tests/zz_probe_merge.rs`, scratch, not committed): two crossing strokes give 1 object in Merge and 2 in Keep separate for Brush Fluid, Normal, Soft, Effect and the Pencil. `J` on Fluid sets Fluid to Keep separate, leaves Normal on Merge, and the status reads "Brush (Fluid): Keep separate". Rerun alone, these pass: `drawing_many_pattern_strokes_in_a_row_stays_responsive`, `dragging_a_selection_on_a_heavy_document_stays_interactive`, `dragging_a_lamp_over_a_scene_stays_interactive`. They had failed only in the 75-minute run under load. `every_ai_command_reveals_the_ai_panel_and_it_draws_through_the_ui` also fails with the change stashed, so it was already failing. `dock_columns` (3) were failing before this change too.

**Found in review:** two Fluid strokes drawn with Keep separate, then two Normal strokes in Merge crossing them, ended as **1 object**: the Merge strokes swallowed the separate ones. Animate protects drawing objects from merge, so this needs D6.

## D6: protect drawing objects

* `buzz_scene::Object` gets `drawing_object: bool` (default false), saved and loaded with the document. The `.buzz` format must read old files without it.
* XFL import: `DOMShape isDrawingObject="true"` sets it. XFL export writes it back.
* Set it on every raw shape laid in Object Drawing: `lay_shape`'s non-merge path (`add_shape`, mirrors included) and `paint_raster`'s non-fused path. Groups never merge anyway.
* Merge never touches a drawing object. Skip them as candidates in `merge_shape_into_layer`, `merge_stroke_into_layer`, `cut_fills_along` and `fusable_paint`.
* Break Apart on a drawing object clears the flag, so it becomes merge paint, as in Animate. Do this only if Break Apart already handles raw shapes.
