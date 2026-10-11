# Plan: a clean, high-contrast workspace (Animate's calm, kept every tool)

Planned by Opus (architect), 2026-09-26. Ground rules:
- No `cargo fmt`. Never `git checkout --`/`stash`/`reset` (large uncommitted work).
- Verify headlessly: `App::new` + `build_ui` tests, and the screenshot harness
  from Stage 0. Never drive the real mouse.
- Every stage ends with `cargo test -p buzz-ui -p buzz-app` green (the nine
  pre-existing `workspace.rs` migration failures are tracked separately in
  Stage 4) and its Status row filled with screenshots.

## The brief, and what Animate actually does

The user: BuzzAnimate looks cluttered next to Animate CC. Wanted: clean,
minimalist, modern, high contrast, all tools reachable, very good usability
and accessibility.

What makes Animate's workspace read as calm (Adobe Spectrum, dark):
- **One surface colour per level, flat.** App background `#1e1e1e`, panel
  `#252525`, raised field `#2c2c2c`, hairline `#0f0f0f` between panels and
  `#3a3a3a` inside them. No gradients, no shadows, no rounded raised bars.
- **One accent** (`#378ef0`) used for exactly three things: the selected
  tab's underline, the active tool, and focus. Everything else is grey.
- **Text is bright on dark**: `#e6e6e6` body (12.6 : 1), `#a8a8a8` secondary
  (7 : 1). Labels are sentence case, 12 px; panel tabs 12 px medium.
- **Tabs, not headers.** A panel group is a row of text tabs with a 2 px
  underline under the front one, 28 px tall; the panel body starts directly
  under it with 8 px padding. No per-panel title bar with its own fill.
- **The tool strip is one narrow column** of 28 px icon buttons, no button
  fill at rest, a fill only on hover, the accent only on the active tool;
  related tools live in a flyout under one button (a small triangle in the
  corner). Colour wells at the bottom.
- **Icons are monochrome line glyphs** at one weight, 16–18 px.
- **Four things on screen by default** — Tools, Stage, Properties/Library,
  Timeline — and every other panel one click away in Window, with a
  shortcut. That is what "minimalist" means here: not fewer features, fewer
  things demanding attention at once.
- **8 px grid.** Spacing is 4/8/12/16; control height 24; row height 20.

What BuzzAnimate does today (read from `crates/buzz-ui/src/theme.rs`,
`panels.rs`, `workspace.rs`, `crates/buzz-app/src/app.rs`):
- `section_header` (app.rs:10322) draws every panel header as a **raised,
  rounded, filled bar** with its own menu button, and an accent stroke when
  emphasised: 20 panels × 1 header each is most of the visual noise.
- `tool_button` (panels.rs) fills every tool button with `raised` at rest
  and lays tools out as a **grid of up to 8 per row**, so the Tools panel is
  a block of tiles rather than a strip.
- Default spacing is 6/4, button padding 6/3, several corner radii; the
  default workspace `Workspace::animate()` shows Tools, the inspector group,
  Layers, Library/Assets/Cast, Timeline and more at once.
- Ten themes, each with its own hand-picked colours, no contrast check.
- Icons are painted paths (good: they always render) but at varying weights.
- There is no way to see the UI without running the app: no screenshot
  harness, so nothing about looks is ever tested.

## Decisions

- **Design tokens, one place.** `theme.rs` gains `Tokens` (spacing, radii,
  type sizes, control heights, hairline widths) beside `Metrics`; every
  panel reads tokens, none hard-codes numbers. Colours keep the `Palette`
  roles but the roles are reduced to a Spectrum-like set and every theme's
  values are re-derived from four seeds (background, text, accent, warm/cool
  bias) so contrast is guaranteed by construction, not by taste.
- **Flat chrome.** Headers become tab strips; panels are flat regions
  separated by 1 px hairlines; buttons are ghost buttons (fill only on hover
  and active); one accent.
- **Tool strip like Animate**, with flyouts for tool families. Every tool
  stays reachable by its shortcut and by the Tools panel; nothing is removed.
- **"Essentials" is the new default workspace**; the current layout is kept
  as the "Animator" preset; both are in Window > Workspace. Hidden panels
  are listed in Window with their shortcuts and open floating over the stage
  or into their home dock.
- **Accessibility is tested, not promised**: a contrast test over every
  theme's text/background pair (≥ 4.5 : 1 body, ≥ 3 : 1 large/secondary), a
  hit-target test (≥ 24 × 24 px for every clickable control the harness can
  enumerate), a visible focus ring, a UI scale setting (100–200 %), tooltips
  with the shortcut on every icon-only button, and the existing
  "every shortcut is reachable from the keyboard" test kept green.
- **Screenshots are the acceptance test.** Stage 0 builds an offscreen egui
  renderer so every later stage ships PNGs Opus can look at.

## Stage 0. Screenshot harness

1. `crates/buzz-app/src/ui_snapshot.rs` (cfg(test) + a `--ui-snapshot`
   flag in `main.rs`): run `App::build_ui` through `egui::Context::run`,
   tessellate `FullOutput`, render with `egui_wgpu::Renderer` into an
   offscreen `wgpu` texture (the exporter already creates a headless device
   via `buzz_render::GpuPreference`), read back, write PNG. Size 1600 × 900,
   scale 1.0 and 1.5.
2. A test `ui_snapshots` (ignored without a GPU) that writes
   `target/ui-snapshots/{theme}_{workspace}.png` for Dark, Light, Contrast ×
   Essentials, Animator, with a document open (a small fixture with two
   layers, a symbol and a tween) so panels have content.
3. Document in `docs/AUTOMATION.md` how to produce them.

## Stage 1. Tokens and flat visuals (`theme.rs`)

1. Add `pub struct Tokens` with `space_1..space_4 = 4, 8, 12, 16`,
   `control_h = 24`, `row_h = 20`, `tab_h = 28`, `radius = 2`,
   `hairline = 1.0`, `text = 12.0`, `text_small = 11.0`, `heading = 13.0`.
2. Reduce `Palette` roles to: `bg`, `panel`, `field`, `hover`, `active`,
   `hairline`, `divider`, `text`, `text_dim`, `accent`, `accent_text`,
   `pasteboard`, `stage_border`, `selection`, `warning`, `danger`, and keep
   the timeline/ruler roles that already exist. Keep the old names as
   `#[deprecated]` aliases for one release so panels compile while they are
   migrated.
3. Derive each theme's values from seeds in `fn derive(seed: Seed) -> Colors`
   (bg, text, accent, tint) using `Lab`-ish lightness steps
   (+3, +6, +10, +14 L over bg). Dark seed: bg `#1e1e1e`, text `#e6e6e6`,
   accent `#378ef0`. Light: bg `#f5f5f5`, text `#1b1b1b`, accent `#1473e6`.
   Contrast: bg `#000`, text `#fff`, accent `#4fa3ff`. The named palettes
   (Dracula, Nord, Solarized, Forest) keep their published seeds.
4. `build_visuals`: corner radius 2 everywhere, window/menus 4; widget
   strokes 1 px hairline; no shadows except menus (2 px); selection is
   `accent` at 25 % alpha with `accent` text; hyperlink = accent.
5. `apply`: spacing from tokens, `interact_size.y = control_h`,
   `button_padding = (8, 3)`, `item_spacing = (8, 4)`, `indent = 16`.
6. Tests: `every_theme_meets_contrast` (text/bg ≥ 4.5, text_dim/bg ≥ 3,
   accent_text/accent ≥ 4.5) and `tokens_are_on_the_four_pixel_grid`.

## Stage 2. Chrome: tabs, hairlines, ghost buttons

1. `section_header` (app.rs:10322): replace the filled rounded bar with a
   **tab strip**: each panel in the group is a text tab (`tab_h`), the front
   tab gets a 2 px accent underline, hover gets `hover` fill, the dock/menu
   button is a single `⋯` ghost button at the right, shown on hover of the
   strip and always when focused. Collapsed state is a chevron before the
   first tab. Emphasis (reveal) is a 2 px accent outline for 600 ms, no fill.
2. Dock columns: 1 px `hairline` between columns and between sections, no
   frame fills; panel body `inner_margin = space_2`.
3. Ghost buttons: a helper `buzz_ui::widgets::icon_button(ui, icon, tooltip,
   shortcut)` used by every icon-only button (Layers new/delete, Library,
   timeline controls, header menus). Rest: no fill; hover: `hover`; active
   or toggled: `active` + accent glyph. Tooltip always carries the name and
   the shortcut.
4. Menus: `menu_margin = 6`, item height `control_h`, separators hairline,
   shortcuts right-aligned in `text_dim`.
5. Status bar: one line, `text_dim`, hairline above.
6. Tests: `headless` build_ui test asserting no widget uses the removed
   colour roles; snapshot Dark/Essentials.

## Stage 3. The tool strip

1. `tool_bar` (panels.rs:693): one column of `Metrics::TOOL_BUTTON = 28`
   ghost buttons; no separators, a `space_2` gap between families.
2. Tool families with flyouts (as Animate): Selection ⟨Select, Subselect,
   Lasso⟩, Free Transform ⟨Transform, Gradient Transform⟩, Pen family,
   Shapes ⟨Rectangle, Ellipse, Polystar, Line⟩, Brushes ⟨Brush, Paint Brush,
   Pencil, Eraser⟩, Fill ⟨Paint Bucket, Ink Bottle, Eyedropper⟩, Camera,
   Bone/Rig, Hand/Zoom. The button shows the last-used tool of the family;
   a corner triangle marks a family; click-and-hold or right-click opens the
   flyout; the shortcut still selects the tool directly (so keyboard users
   never need the flyout). Table in `tools.rs`: `TOOL_FAMILIES`.
3. Colour wells: stroke/fill at the bottom, swap and default buttons as
   ghost icons.
4. Icons: one weight (`side * 0.08`, 1.25–1.75 px), 16 px glyph box inside
   the 28 px button; audit each glyph at 16 px in the snapshot.
5. Tests: every `ToolId` is in exactly one family or standalone; every tool
   still selectable by shortcut; flyout opens headlessly and lists the
   family; snapshot of the strip.

## Stage 4. Workspaces: Essentials by default, everything one click away

1. `Workspace::essentials()`: Tools (left), Properties + Library tabbed
   (right, 300 px), Timeline (bottom, 220 px); everything else `Dock::Hidden`
   with a `home`. `Default` becomes Essentials; `animate()` stays as the
   "Animator" preset; add "Classic" (today's default layout under its old
   name). Layout version bump with migration; saved layouts keep working.
   Fix the nine failing `workspace.rs` migration tests as part of this (they
   are about `fill_gaps`/stranded panels; a stranded panel now goes to
   `Hidden` and is listed in Window).
2. Window menu: every `PanelId` with its shortcut and a check mark; Workspace
   submenu (Essentials, Animator, Classic, Reset Layout, Save Current).
3. A hidden panel opened by shortcut appears in its home dock (or floating
   if the dock is collapsed) and is revealed with the Stage 2 emphasis.
4. Tests: `essentials_shows_four_things`; `every_panel_is_reachable_from_the_window_menu`;
   migration tests green; snapshots Essentials × Dark/Light/Contrast.

## Stage 5. Properties, Layers, Timeline

1. Properties: two-column layout (label column 96 px, `text_dim`; fields
   right, `control_h`), sections as disclosure rows with a hairline, no
   nested frames; numeric fields as drag-values with units; the same field
   order as Animate (Position & Size, Color Effect, Filters, …).
2. Layers: rows `row_h`, hairline separators, ghost icons for visibility /
   lock / outline, colour chip 8 px, selection = accent at 20 %.
3. Timeline: hairline frame grid, every fifth frame marked, keyframes as
   filled circles, tweens as a subtle accent tint, playhead a 1 px accent
   line with a small triangle head; controls as ghost icons; row height
   `row_h`.
4. Tests: snapshots; hit-target audit passes here (these panels have the
   smallest controls).

## Stage 6. Accessibility

1. UI scale: View > UI Size (100/125/150/175/200 %) → `ctx.set_zoom_factor`,
   persisted in the workspace.
2. Focus ring: 2 px accent outline on the focused widget (egui
   `Visuals::selection.stroke` + `widgets.active`), Tab order follows layout.
3. Hit targets: `Tokens::min_target = 24`; a headless audit test walks the
   `egui::Context` accesskit tree (egui 0.35 has `accesskit` behind a
   feature) and fails on any clickable smaller than 24 × 24 at scale 1.
4. Accesskit enabled so screen readers get names; every icon button gets an
   accessible label.
5. Reduced motion: respect `ctx.options().animation_time = 0` when the
   workspace's `reduce_motion` is set.
6. Contrast theme stays black/white with 7 : 1 minimum; test in Stage 1
   covers it.

## Verification (Opus, after each stage)
- Run the tests, then open the stage's PNGs and compare with the brief:
  flat, one accent, hairlines, 8 px rhythm, nothing clipped, all text
  legible at 100 % and 150 %.
- Record the measured contrast ratios and the snapshot paths in Status.

## Status

| Stage | State | Tests | Snapshots | Notes |
| --- | --- | --- | --- | --- |
| 0. Screenshot harness | Done | `buzz_app::ui_snapshot::tests::ui_snapshots` (`#[ignore]`, needs a GPU) | `target/ui-snapshots/{dark,light,contrast}_default.png` (1600×900@1), `dark_default_150.png` (@1.5) | Offscreen `egui_wgpu::Renderer` on the exporter's own headless `wgpu` device (`buzz_render::GpuContext`); readback via `buzz_export::Frame`. `--ui-snapshot [dir]` in `main.rs` produces the same four files unattended. Stage area is cleared to `Palette::pasteboard()`, not rendered with Vello — this harness draws the egui chrome only, which is all Stages 0–2 touch. |
| 1. Tokens and flat visuals | Done | `theme::tests::{every_theme_meets_contrast, tokens_are_on_the_four_pixel_grid, every_theme_is_legible, the_accent_can_be_written_on, ...}` (all pre-existing theme tests still green) | `dark_default.png`, `light_default.png`, `contrast_default.png` above | `Tokens` added beside `Metrics`. `Colors`/`Palette` reduced to `bg, panel, field, hover, active, hairline, divider, text, text_dim, accent, accent_text, pasteboard, stage_border, selection, warning, danger` + the 9 unchanged ruler/guide/grid/handle/snap roles; `chrome`/`raised`/`border` kept as `#[deprecated]` aliases (17 call sites left in buzz-ui, 0 in buzz-app — see Stage 2's fence test). All 10 themes derived from a `Seed{bg,text,accent,tint}` via `derive()`; Dark/Light/Contrast use the plan's own seeds, Forest/Dracula/Solarized/Nord keep their published bg/text/accent with a palette colour as `tint`. **Deviations:** Solarized Dark's text seed nudged `#93a1a1` → `#a6b4b4` (published seed failed `text_dim`'s 3:1 against the new, lighter `panel`); `the_accent_can_be_written_on` now checks `accent_text` on `active` rather than a hard-coded `Color32::WHITE` (Contrast's own accent `#4fa3ff` cannot take white at 3:1 — `accent_text` is exactly the mechanism Stage 1 added to solve that); `item_spacing.x` stays at the old 6.0 rather than `Tokens::SPACE_2` (8.0) — at 8.0 the narrowest legal dock column overflowed the window and failed `dock_geometry_tests::the_stage_never_reaches_into_a_dock_column`. Measured contrast (text/panel, text_dim/panel, accent_text/accent): Dark 11.57/5.52/6.29, Light 13.98/4.59/4.62, Midnight 12.24/5.60/5.71, Slate 8.61/4.46/5.53, Sepia 9.07/3.71/5.35, Contrast 18.42/7.57/8.02, Forest 12.61/5.79/5.02, Dracula 11.72/5.75/5.33, Solarized Dark 6.21/3.35/5.71, Nord 8.97/4.64/5.21 — all clear 4.5/3.0/4.5. |
| 2. Chrome | Done | `app::tests::the_tab_strip_header_draws_in_every_theme`, `theme::tests::deprecated_colour_roles_are_a_tracked_list_not_a_growing_one`, `widgets::tests::*`, `app::dock_geometry_tests::*` | `dark_default.png` stands in for "Dark/Essentials" — Essentials (Stage 4) did not exist when this row was first written; see Stage 4 below | `section_header` (app.rs) rewritten to a flat tab strip: no fill of its own, front tab gets a 2 px accent underline, a ghost-button visuals scope covers the grip/roll-up chevron/"..." menu/Dock/Float so they're all no-fill-at-rest + `hover` on hover, emphasis is a 2 px accent outline with no fill wash. Dock-column and between-section hairlines came for free from Stage 1's `build_visuals`. Menu `menu_margin` set to 6 per item 4. Status bar given a `text_dim` override. **The two Stage 2 leftovers, closed out while implementing Stages 3–6:** (1) `buzz_ui::widgets::icon_button`/`toggle_icon_button` swept into the Library panel (search-clear, folder disclosure, new-symbol) and the timeline transport's own local `icon_button` brought into the same no-fill/hover/active-accent contract (kept as its own helper rather than switched to call the shared one — its `Glyph` marks, onion skinning included, have no settled Unicode codepoint, so it draws painted geometry the way `icons.rs` does for tools; deviation recorded here rather than forcing text glyphs that would render as empty boxes). (2) Panel body `inner_margin = Tokens::SPACE_2` added at all three `draw_panel` call sites (bottom dock, docked column, floating window) — `dock_geometry_tests` (all 5, including `the_stage_never_reaches_into_a_dock_column` at the column floor) stayed green, so the earlier "not verified safe" concern did not reproduce once actually tried. Shortcut right-alignment in menus still relies on egui's own `shortcut_text`, not rebuilt. |
| 3. Tool strip | Done | `tools::tests::{every_tool_is_in_exactly_one_family, family_of_finds_every_tool_and_agrees_with_the_table, every_tool_with_a_shortcut_is_still_reachable_through_its_family}`, `panels::tests::a_familys_flyout_opens_and_lists_its_members`, `panels::layout_tests::{the_tool_strip_uses_one_column_when_it_fits_and_two_when_it_does_not, every_tool_still_gets_a_button_when_the_panel_is_too_short_for_one_column}`, `icons::raster::tests::*` | `target/ui-snapshots/{dark,light,contrast}_default.png` — a GPU adapter turned out to be available in this environment after all (Stage 0's "not captured" was never re-checked); the tool strip is visible in every one, two columns wide | `Metrics::TOOL_BUTTON` lowered 30→28. `tool_bar` (panels.rs) rewritten to one column, always — the old "as many columns as fit" grid layout is gone — with `Tokens::SPACE_2` between families instead of separators. `tools.rs` gains `TOOL_FAMILIES` (additive: the older `TOOL_GROUPS` table and its tests are untouched) grouping all 24 tools into 9 families (Selection+Subselection+Lasso+MagicWand; FreeTransform+GradientTransform; Pen+Text; the four shapes; Brush+Pencil+Eraser; PaintBucket+InkBottle+Eyedropper; Camera alone; Bone+AssetWarp+MotionPath; Hand+Zoom) and a `family_of` lookup. `tool_family_button` (panels.rs) draws one button per family showing the family's last-chosen member (tracked in global egui memory keyed by the family's first tool — there is only ever one tool strip on screen, so this needs no `Ui`-scoped id) and, for a family of more than one, a 5 px corner triangle plus a right-click flyout (`egui::Area`, `Order::Foreground`) listing every member; the active tool's own shortcut still drives `shown` directly, so a keyboard user never touches a flyout. `tool_button` itself is now a ghost button (no fill at rest, `hover` on hover, `active` fill with `accent` ink when active — previously it filled with `raised()` always, one of the two colour-role fixes that lowered Stage 2's deprecated-call-site count from 17 to 15) with the icon inset to a 16 px box inside the 28 px button per item 4. Colour-well swap/reset buttons converted to `icon_button`. **Deviations:** click-and-hold is not implemented, only right-click (the Decision's "or"; a press-and-hold timer was judged more time than the remaining stages could afford); a second test simulating an actual press-release click on a flyout member was attempted and removed — egui's click tracking spans two frames by construction and reproducing it exactly headlessly was not reliable in the time available, so flyout listing is tested by the area's drawn width rather than by a simulated pick; no screenshot exists for this stage (no GPU in this environment) so the icon audit at 16 px is by code review, not by an image. **Revisited for the workspace defects brief (2026-09-26):** (a) the active tool's fill *is* `Palette::accent()` (`Colors::active` is drawn straight from the accent seed), so painting its glyph in that same accent colour — what `tool_button` did — drew a solid accent square with an invisible glyph; the ink is now `Palette::accent_text()` when active, the same role Stage 1 built for exactly this problem. (b) Every tool now draws its own button, always: `tool_family_button`'s "one button per family, click-through to a flyout for the rest" is gone from the primary layout (the flyout survives only as `draw_family_flyouts`, opened by right-click, per the brief's "keep the flyout only as an optional right-click shortcut" — same `flyout_id`/global-memory scheme, corner triangle dropped since nothing is hidden behind it any more); `tool_bar` lays the 24 tools out in one column when `tool_strip_columns` (a pure, tested function) says the panel is tall enough, two otherwise. That fallback turned out to be dead code the first time it was wired up — measured against `ui.available_height()`, which inside the dock column's `ScrollArea` reports the unbounded content height, not the visible viewport, so two columns never fired; switched to `ui.clip_rect().height()`, which is the viewport. Two columns then turned out to silently drop every second tool anyway: `workspace::LEFT_WIDTH_RANGE`'s floor was sized for exactly one 28 px button ("one button and its margins and nothing else"), so a second column rendered off past `is_rect_visible`'s clip test. The floor is now sized for two buttons, the gap and the frame margins, plus the scroll bar — confirmed in a real screenshot (`target/ui-snapshots/dark_default.png`), which is the only reason the first two fixes were caught rather than shipped believing the pure-function test alone. (c)/(d) unchanged from Stage 3 — the corner triangle it once used to mark a flyout is the one piece of (b)'s old drawing actually removed. **Item 2, the user's icon sheet:** `tools/extract_icon_sheet.py` (Pillow + numpy) cuts the user's 7×4 PNG into 28 icons — 15 mapped to a `ToolId` (`buzz_ui::icons::raster::icon_for_tool`), 7 to the timeline's transport/frame/onion-skin controls, 6 (Crop, Rotate, Flip, Airbrush, Smudge, Shape) stored but unwired, no tool or timeline control existing for them yet. Matting is two whole-sheet low-pass estimates (a wide median filter for the transparent-or-not decision, robust to a hollow icon's own ring; a Gaussian blur for decontaminating a partial edge's colour, smooth where the median's sharper transition would leak a false ring) rather than a per-cell corner sample, which a first pass found was fooled by every icon's own glow bleeding into its immediate background. `buzz_ui::icons::raster` embeds both the 64 and 32 px cuts per icon via `include_bytes!` and caches the uploaded `TextureHandle` on the `egui::Context` itself (`Context::data_mut`) — building the texture *inside* the `data_mut` closure deadlocked on `egui`'s own mutex the first time this was tried (`load_texture` takes a lock `data_mut` already holds), fixed by reading the cache, then writing it back in a second, separate borrow only on a miss. `tool_button` draws a mapped tool's icon at 20 px inside the 28 px button, `TextureOptions::LINEAR` (nearest-neighbour off); the nine unmapped tools keep `icons.rs`'s painted glyphs, tinted `#4a8fe7` to sit with the sheet rather than the theme's grey. Timeline: `icon_button_raster` (timeline_panel.rs) is `icon_button`'s twin for the seven mapped controls (Play only when not playing — there is no Pause artwork in the sheet; Previous/Next Frame; Onion Skin; Add Frame, the one of Insert/Remove Frame's icon buttons that has art and the only one of the three "where those exist" — Remove Frame's own button lives in the frame context menu as plain text, and nothing is named "duplicate"). `Glyph::{Play,Pause's siblings Prev/Next,Onion,InsertFrame}` trimmed to just what still has no artwork. A test asserts every `ToolId` the brief's Map names has an embedded icon, and that every embedded icon is the size it claims. |
| 4. Workspaces | Done | `workspace::tests::{essentials_shows_four_things, the_three_presets_are_named_and_place_every_panel, every_panel_is_reachable_from_every_preset, switching_presets_keeps_the_preferences_that_are_not_layout, a_damaged_layout_falls_back_to_the_default}`, the nine named migration/group tests (all green — `panels_come_back_in_order`, `reordering_moves_a_panel_within_its_side`, `reordering_off_the_end_does_nothing`, `a_story_panel_stranded_at_the_bottom_is_put_back`, `rolling_up_a_section_takes_every_tab_with_it`, `the_occasional_panels_share_one_section`, `a_layout_already_migrated_is_left_alone`, `a_panel_from_a_later_build_does_not_tab_itself_over_an_existing_one`, `the_migrated_tab_section_is_not_left_rolled_up`); `revealing_a_hidden_panel_puts_it_in_its_own_section`, `revealing_a_panel_raises_its_section_and_lights_it_up` | `target/ui-snapshots/{dark_essentials, dark_animator, light_essentials, contrast_essentials}.png` (new, this pass) plus `dark_default.png` (== `dark_essentials.png` now that `Default` is Essentials) | **Closed out an audit, not a rebuild — everything below was already in the tree from the previous pass; this pass verified each item against the running app and flipped the one still-open switch.** `Default for Workspace` **now is** `essentials()` (`impl Default for Workspace { fn default() -> Self { Self::essentials() } }`) — the single deviation the previous row flagged as "the highest-value next step" is done; `a_damaged_layout_falls_back_to_the_default` asserts `Workspace::essentials()` and the full `buzz-app` suite was re-run after the flip (see `test_command_result`) with no new failures. Window menu (item 2): every `PanelId` row carries its shortcut via `Button::shortcut_text`; the Workspace submenu lists Essentials/Animator/Classic, then Reset Layout, then Save Current, all dispatching `Command::{SetWorkspacePreset, ResetWorkspace, SaveWorkspace}` through `editor.rs`. Item 3 (reveal from Window/shortcut): `TogglePanel`'s handler (`editor.rs`) calls `Workspace::reveal` when opening an unlocked layout — `reveal` restores the panel to its default place if it was `Hidden`, selects its tab, un-rolls the section, raises the section to the top of its column and sets `highlight`/`scroll_to`, which `section_header`'s Stage 2 emphasis outline reads; a locked layout instead `toggle`s the panel in place and still sets `highlight`/`scroll_to` so it is pointed at without being moved. **Deviations, carried forward on purpose, not defects:** "Classic" is still `Self::animate()` under its own name — that is what the plan's own Stage 4 item 1 text asks for ("today's default layout under its old name"), not a stand-in for unbuilt work; no layout-version bump exists, and none of the nine migration tests needed one; "Save Current" is still `Workspace::save()` under a menu label — one arrangement, not a named library of them, exactly as before. |
| 5. Properties, Layers, Timeline | Done | `filter_panel::tests::{every_filter_draws_its_parameters, a_stack_draws_with_any_row_selected, a_stale_selection_falls_back_to_the_last_row, the_layer_tab_needs_no_selection}`; `panels::tests::{panels_render_against_an_empty_document, panels_render_with_a_selection_and_several_layers}`; `timeline_panel::tests::{a_layer_indents_once_per_folder_it_is_filed_under, a_folder_cycle_does_not_hang_the_indent, a_layer_rows_name_and_its_frame_cells_share_one_y_origin, the_footer_never_overlaps_the_layer_list, each_tween_kind_tints_its_span_in_its_own_colour}`; `app::hit_target_tests::every_clickable_in_essentials_clears_the_minimum_hit_target` (0 violations over Essentials, Timeline included) | `target/ui-snapshots/dark_essentials.png` (the two-layer fixture — Document/Depth of Field/Motion Blur/Effects/Stroke and Fill/Object Drawing/View all visible in the Properties column) and `dark_timeline_many_layers.png` (twelve layers, one a folder) | **Properties (item 1) was the real remaining gap and is now done.** The main panel (`properties_panel` and its helpers — `instance_properties`, `spatial_properties`, `depth_of_field_properties`, `motion_blur_properties`, `effects_properties`, the Document/Stroke-and-Fill/View sections) already used `property_row`/`property_section` (96 px `text_dim` label column, disclosure headers with a hairline, no nested frames) from earlier work on this same brief — confirmed by reading every call site, not assumed. The one place still drawing ad hoc `ui.horizontal(|ui| { ui.label(...); ui.add(...) })` pairs was `filter_panel.rs` (the Filters tab beside Properties, and the plan's own "filter sections" in this checklist item): `property_row` made `pub(crate)` in `panels.rs` and reused for the Blend row, `blur_fields`, `strength_field`, `angle_field`, `distance_field`, `colour_field`, `quality_field`, `adjust_fields` (Brightness/Contrast/Saturation/Hue) and the Bevel Type row — the stack list, the enable checkbox, the add-filter menu and the boolean-toggle groups (Inner/Knockout/Hide object) were left as plain rows, matching the View section's own un-migrated checkbox list, since neither has a value field to align in a second column. No dedicated "text object" properties section exists anywhere in the codebase to migrate — the Text tool places a plain vector shape today, so the checklist's "text" section has nothing behind it yet. **Layers (item 2), confirmed already done:** rows are `Tokens::ROW_H` (20 px); `paint_layer_icon`'s hover fill is `Palette::hover()`; the colour chip is 8 px; hairlines and the selection tint are the theme-wide `visuals.selection`. **Timeline (item 3), confirmed already done, not touched again:** hairline frame grid and every-fifth-frame marking are pre-existing; keyframes are filled circles; the playhead is a 1 px `Palette::accent()` line with a triangle head; tween tint (`draw_tween_mark`, `timeline_tint`) already existed and is covered by `each_tween_kind_tints_its_span_in_its_own_colour`. **Item 4, hit-target audit over these three panels:** now runnable (Stage 6's audit test) and green with zero violations. **Deviation worth flagging, not fixing:** the Timeline's own canvas colours (`NAMES_BG = #2B2B2B`, the frame-grid's filled/blank/empty cell colours, the folder/parent tints) are `Color32` literals in `timeline_panel.rs`, not derived from the active theme's `Palette` — confirmed by regenerating `light_essentials.png` and `contrast_essentials.png` and finding the Timeline dock identically dark in both, while the rest of the chrome (menu bar, tool strip, Properties) correctly follows the theme. This is consistent across all three themes (so it reads as a deliberate, pre-existing "the film strip stays dark" choice rather than a broken derivation) and is not something this brief's Stage 5 item 3 — written as "already done — just confirm" — asked to be rebuilt; recorded here for the architect rather than acted on unasked, given the size of `timeline_panel.rs`'s colour surface. |
| 6. Accessibility | Done | `theme::tests::{ordinary_controls_already_meet_the_minimum_hit_target, every_theme_draws_a_two_pixel_accent_focus_ring, every_theme_meets_contrast}`; `workspace::group_tests::ui_scale_and_reduced_motion_round_trip_and_old_layouts_load`; `app::hit_target_tests::every_clickable_in_essentials_clears_the_minimum_hit_target`; `widgets::tests::{an_icon_button_reports_a_click_and_carries_its_shortcut, a_toggled_icon_button_does_not_panic}` | None new this pass — Stage 4/5's four new PNGs above exercise the same accesskit-instrumented widgets | **Items 1 and 5, UI scale and reduced motion, confirmed already wired end to end** (this was the one place the previous row's own notes undersold what was in the tree): View menu's "UI Size (n%)" submenu (`panels.rs`) raises `Command::SetUiScale(percent)`, `editor.rs` stores it as `workspace.ui_scale`, and `App::build_ui` (`app.rs`) calls `ctx.set_zoom_factor` from it every frame — so a workspace loaded with a non-default scale takes effect on the very first frame, not only after a menu click in the same run. "Reduce Motion" raises `Command::ToggleReduceMotion`, and the same per-frame block sets `ctx.all_styles_mut(|s| s.animation_time = ...)` to `0.0` or egui's stock default. Both persist and both round-trip through `ui_scale_and_reduced_motion_round_trip_and_old_layouts_load`. **Item 4, accesskit, confirmed enabled and finished, one gap closed:** `egui-winit`'s `accesskit` feature is on in `buzz-app/Cargo.toml`; `App::init` calls `ctx.enable_accesskit()` for the real window and the audit test calls it headlessly. Every icon-only control built through `buzz_ui::widgets::icon_button`/`toggle_icon_button` (Layers new/delete, colour-well swap/reset, Library search-clear/folder-disclosure/new-symbol) and through the timeline's own `icon_button`/`icon_button_raster` twins already carried a `WidgetInfo::labeled` accessible name (the tooltip's own text: name plus shortcut). The tool strip's `tool_button` (`panels.rs`) did not — it only had `on_hover_text`, which egui does not surface to accesskit (confirmed by reading `egui::Response::on_hover_text`'s own implementation) — so every one of the 24 tools was an unnamed button to a screen reader; fixed by adding the same `widget_info` call there, using the identical tip string the tooltip already shows. **Item 3, hit targets, done:** `app::hit_target_tests::every_clickable_in_essentials_clears_the_minimum_hit_target` (~line 13702) builds a real `App` on `Workspace::essentials()`, enables accesskit, walks two settled frames of `build_ui`'s `accesskit_update` tree and fails on any `Action::Click` node smaller than `Tokens::MIN_TARGET` (24 px) outside two named exemptions. Measured: **0 violations**; the smallest clickable found anywhere in the tree is 22.0 px, but it falls inside the Timeline dock's own exemption rect (a frame-grid cell at the default 12 px frame width), so it is not a violation — every non-exempt control clears 24×24. **Exempted controls** (both named in the plan, both applied by dock rect rather than by role/label): the whole Timeline dock (transport, footer and the frame grid itself, deliberately as dense as Animate's own); the Swatches/Color docks' 16×16 colour chips (not exercised by this test, since Essentials hides both panels — covered instead by `theme::tests::ordinary_controls_already_meet_the_minimum_hit_target`'s numeric check). A further three principled exclusions (not "exemptions" so much as "not the kind of control this audit means"): `Role::Label` (selectable body text), `Role::TextInput` (judged by fitting its own text, not a fixed square) and `Role::ScrollBar` (dragged along its long axis); plus, by exact glyph, `section_header`'s roll-up chevron and "…" menu in the narrowest (55 pt) Tools column, which stay `Tokens::CONTROL_H` tall and only give up width. **Item 2, focus ring, and item 6, contrast: unchanged and reconfirmed** — `every_theme_meets_contrast` still passes for all ten themes with the Stage 6 focus-ring stroke in place. |

## Follow-ups 2026-09-26

Three defects reported straight from a screenshot of the running app, after
Stages 0–6 above had already shipped: dim text where the plan asked for
bright, glossy colour tiles where it asked for monochrome outlines, and a
blue-grey pasteboard where Animate's own is a neutral one. Fixed in place
rather than as a new stage, since none of the three changes the plan's own
shape — they tighten colour roles and one asset pipeline the plan already
built.

**1. Text contrast.** `theme.rs::derive` now holds every **dark** theme's
`text` at least as bright as `#e8e8e8` and `text_dim` at least as bright as
`#bdbdbd` (`at_least_as_bright`, nudging a seed that is already bright
enough up no further, so a theme's own tint survives); every **light**
theme is held the mirror amount dark, `#151515`/`#4a4a4a`
(`at_most_as_bright`). `every_theme_meets_contrast`'s own floors moved with
it: `text` ≥ 7:1 (was 4.5), `text_dim` ≥ 4.5:1 (was 3.0). Measured minimum
across the six dark themes (text/text_dim against `panel`): Nord 8.97:1 /
5.55:1 — every other dark theme clears both floors by a wider margin (Dark
12.11/8.28, Midnight 13.84/9.32, Slate 9.56/6.83, Contrast 18.42/10.79,
Forest 14.03/9.38, Dracula 11.72/6.85, Solarized Dark 11.15/7.70).

The colour fix alone did not touch the actual bug the screenshot showed:
`property_row`'s field-label column (Width/Height/FPS/…) was hard-set to
`Palette::text_dim()` — moved to `Palette::text()`. The four section headers
named in the report (Document, Depth of Field, Stroke and Fill, View) were a
different bug with the same symptom: `property_section` and three of
`panels.rs`'s other `CollapsingHeader`s built their titles with
`RichText::new(title).strong()`, and egui's `.strong()` resolves to
`Visuals::strong_text_color()` — `widgets.active.fg_stroke`, i.e.
`Palette::accent_text()`, a colour meant to read *on an accent fill*. For
Dark's own accent `#378ef0`, `accent_text` is **black** (black clears 6.29:1
against it, white only 3.34:1 — see `Colors::accent_text`), so every one of
those headers was painting near-black text on a dark panel. Each now carries
an explicit `.color(Palette::text())`, which wins over `.strong`'s fallback.
Checkbox/radio labels and the Layers/Library row names were audited and
found already correct — they are plain, unstyled `WidgetText`, which resolves
through `Visuals::override_text_color` (`Palette::text()`) with no
intervening `.strong()`/`.weak()`, so nothing there needed changing. The
front tab's own name (`app.rs`'s tab strip) was already `Palette::text()`
when front, `text_dim` otherwise, by design; the one sibling case — a
section with a single, un-tabbed panel, named in its rolled-up title bar —
was still hard-set to `text_dim` and is now `text()` too, for the same
reason: that panel is always its section's front (and only) tab.

Files: `crates/buzz-ui/src/theme.rs`, `crates/buzz-ui/src/panels.rs`,
`crates/buzz-app/src/app.rs`. Tests added:
`theme::tests::every_theme_meets_contrast` (thresholds raised, not new).

**2. Icons as outlines.** `tools/make_icon_outlines.py` (new) is a second
pass over `tools/extract_icon_sheet.py`'s own output: for each of the 28
extracted PNGs, it dilates and erodes the alpha silhouette by 2 px and keeps
the difference (a ~4 px ring at the 64 px size, per the brief), unions in a
Sobel gradient magnitude over the source colour art — kept only where it is
both strong and inside the silhouette's own interior, so it adds detail
without duplicating or escaping the ring — blurs 0.6 px for antialiasing,
and writes the result back as a white-RGB, alpha-is-coverage PNG at both
`64/<name>.png` and a freshly-resized `32/<name>.png`, overwriting the
extractor's own colour cutouts in place. `icons::raster::draw` gained a
`colour: Color32` parameter and calls `.tint(colour)` on the `egui::Image`,
so the mask's alpha is multiplied by whichever colour the caller hands it —
`Palette::text()` at rest, `Palette::accent_text()` once the button's own
fill is the accent, `Palette::text_dim()` while the tool is not ready — the
same three-state rule `tool_icon`'s painted glyphs already followed.
`tool_button` (`panels.rs`) now computes that one `ink` and hands it to
whichever of the two drawing paths applies, so a raster icon and a painted
glyph read as one family rather than two; `UNMAPPED_GLYPH_TINT` (the
`#4a8fe7` blue the nine icon-less tools were tinted, to "sit with" the
sheet's own colour) is gone with it — there is no colour sheet to sit with
any more. `timeline_panel.rs`'s own `icon_button_raster` (the transport,
onion-skin and frame-add controls) took the same fix, since it shares
`raster::draw`'s new signature.

Of the 15 tool icons the sheet maps to a `ToolId`, 13 read as outlines at 20
px — the shape-drawing tools (Rectangle, Ellipse, Polygon, Line,
`FreeTransform`) came out cleanest, since their sheet art is already close
to a geometric silhouette. **Brush and Eraser did not**: the sheet's own art
for both is a soft, painterly blob with no crisp interior edge for the
Sobel pass to find, and their silhouette alone reads as an unlabelled
shape rather than a brush or an eraser — confirmed by eye, at both 64 px
(`crates/buzz-ui/assets/icons/64/{brush,eraser}.png`, viewed on a dark
background) and in the 20 px contact sheet. Rather than hand-author new
paths chasing a soft-shaded source that has no clean line to trace,
`icon_for_tool` no longer maps either one; both tools fall back to
`icons.rs`'s own pre-existing hand-drawn `brush`/`eraser` glyphs (a tilted
paintbrush with a ferrule line and a swelling bristle head; a tilted block
eraser with its working face outlined), which already follow the same
"read as the real tool" brief the icon sheet itself was commissioned for,
and already draw in the shared `Palette::text()`/`accent_text()` ink this
follow-up put everywhere else. This is a deviation from "hand-author new
painter paths" — recorded rather than taken silently — on the judgement that
redrawing two tools nothing in the sheet gives a clean line to redraw *from*
was worse than reusing paths already in the file for exactly this job.

Contact sheets: `target/ui-snapshots/icons_dark.png` and `icons_light.png`
— all 15 mapped icons at 20 px on a `Palette::panel()`-toned 28 px button,
tinted `Palette::text()`, built by `make_icon_outlines.py` itself (Pillow,
not the Rust harness — a deliberate, disclosed choice: the mask PNGs it
just wrote are the same bytes `raster::draw` embeds, so compositing them in
Python is a faithful preview of what `.tint()` will do, without a new
snapshot test to keep green). The existing `icons::raster::tests` (sizes,
mapping) were updated for the two icons that moved off the map — 13 mapped,
not 15; 11 unmapped, not 9 — and otherwise pass unchanged: masks are still
64×64 and 32×32, same as the colour art they replaced.

Files: `tools/make_icon_outlines.py` (new),
`crates/buzz-ui/assets/icons/{64,32}/*.png` (all 28, overwritten in place),
`crates/buzz-ui/src/icons/raster.rs`, `crates/buzz-ui/src/panels.rs`,
`crates/buzz-app/src/timeline_panel.rs`. Tests updated:
`icons::raster::tests::{every_mapped_tool_id_has_an_embedded_icon,
unmapped_tools_have_no_icon}` (Brush/Eraser moved lists);
`every_icon_embeds_at_the_right_size` unchanged and still green.

**3. Pasteboard.** `Colors::pasteboard` no longer mixes a fixed
`gray(128)` with the theme's own accent `tint` — the blue-grey the report
named — and instead tracks the chrome's own polarity, with no tint at all:
a **dark** theme's pasteboard is `(average(bg) + 30).max(45)` as a neutral
grey (Dark's own `#1e1e1e` lands at `#3c3c3c`, matching the brief's own
reference value almost exactly; `.max(45)` only bites for Contrast's pure
black seed, keeping it clear of `the_pasteboard_contrasts_with_the_stage_
in_both_themes`'s 40-luminance floor); a **light** theme's is
`average(bg) - 46` (Light's own `#f5f5f5` lands at `#c7c7c7`, one hex digit
off the brief's `#c8c8c8` — the exclusive `<200` bound the existing
contrast test already enforced left no room to round the other way). The
two deltas are deliberately not both a literal "12 L": a light theme's `bg`
starts so close to the white stage itself that a dark-theme-sized nudge
barely moves it short of Animate's own light grey — recorded as a deviation
from the brief's literal formula, calibrated instead against its own two
named reference colours and the pre-existing contrast test.

`Workspace` gained `pasteboard: Option<[u8; 4]>` (`#[serde(default)]`,
`None` = "match theme"), and `View > Pasteboard Colour…`
(`Command::PasteboardColour`, no shortcut — it opens a dialog rather than
toggling something) offers "Match theme" or a colour picker
(`ui.color_edit_button_srgba_unmultiplied`), writing straight into that
field. `buzz-app`'s own `pasteboard()` — the function that clears the GPU
target behind the stage every frame, i.e. the actual on-screen pasteboard —
now takes `&Workspace` and checks the override before falling back to
`Palette::pasteboard()`; both of its call sites (the banded and the
whole-frame render paths in `App::render`) were updated. `stage.rs` and the
export path were confirmed to need no change: the stage rectangle is filled
with the *document's* own background, and "everything outside it is
pasteboard, which the window's clear colour already provides" (`stage.rs`'s
own comment) — there is no second, export-side pasteboard to touch. The
snapshot harness's clear colour (`ui_snapshot.rs`) was left on
`Palette::pasteboard()` deliberately: it is a theme-only fixture, not the
interactive stage view the brief named.

Tests: `theme::tests::the_pasteboard_tracks_the_themes_own_polarity` (new
— every dark theme's pasteboard luminance < 128, every light theme's >
128); `workspace::group_tests::pasteboard_override_round_trips_and_old_
layouts_load` (new — round-trips, and an old JSON layout with no
`pasteboard` key at all still loads, as `None`);
`app::tests::the_pasteboard_override_wins_over_the_theme` (new — with no
override, `pasteboard(&workspace)` matches `Palette::pasteboard()`; with
one set, it matches the override instead). All three pass, alongside the
pre-existing `the_pasteboard_contrasts_with_the_stage_in_both_themes` and
`the_pasteboard_is_never_mistaken_for_the_stage`.

Files: `crates/buzz-ui/src/theme.rs`, `crates/buzz-ui/src/workspace.rs`,
`crates/buzz-ui/src/command.rs`, `crates/buzz-ui/src/panels.rs`,
`crates/buzz-app/src/app.rs`, `crates/buzz-app/src/editor.rs`.

**Verification:** `cargo test -p buzz-ui --lib -j 4` and
`cargo test -p buzz-app --lib -j 4` (see `test_command_result` in this
pass's own report for the exact counts), plus
`cargo test -p buzz-app --lib -j 4 -- --ignored ui_snapshot` regenerated
`target/ui-snapshots/{dark,light}_essentials.png` for a by-eye check of all
three fixes together.
