# 📖 BuzzAnimate Documentation Center

Welcome to the official documentation and learning hub for **BuzzAnimate**.

---

## 📚 Documentation Index

1. [**Complete User Guide & Feature Reference (`USER_GUIDE.md`)**](USER_GUIDE.md)
   - The master comprehensive handbook covering every tool, panel, menu command, and system in the app.
   - Includes real snapshot diagrams, detailed workflow guides, and an exhaustive **Alphabetical A–Z Master Index**.

2. [**5-Minute Quickstart Tutorial (`QUICKSTART.md`)**](QUICKSTART.md)
   - Step-by-step hands-on tutorial for first-time animators: drawing, keyframing a bouncing ball, classic tweening, lighting, and video export.

3. [**Keyboard & Mouse Shortcuts Cheatsheet (`SHORTCUTS.md`)**](SHORTCUTS.md)
   - Printable and searchable quick-reference card for all 23 single-key tools, timeline hotkeys, canvas navigation gestures, and editing chords.

4. [**Scenes, the Director, and Motion That Runs Itself (`SCENES_AND_THE_DIRECTOR.md`)**](SCENES_AND_THE_DIRECTOR.md)
   - How a shot builds itself: what *Set the Scene* arranges and the arithmetic behind it, what *Direct a Story* reads out of your prose and how it frames and cuts the shot, and every piece of the picture that keeps moving after you stop touching it — breathing, sway, drifting cloud, running water, fire and lightning.

5. [**Where the Time Goes — the Automation Audit (`AUTOMATION.md`)**](AUTOMATION.md)
   - What the program already does for you, ranked by the hand labour it removes — an honest list of what it still makes you do by hand, and a week-in-the-life audit for a narrated story channel.

---

## 🖼️ Included Visual Assets (`images/`)

> **The feature figures are generated, not screenshotted.** `modifier_blink_*`,
> `camera_move_*`, `camera_ease_*` and `line_weight_*` are rendered by the
> program itself from
> `crates/buzz-export/tests/doc_figures.rs`. A screenshot is correct on the day
> it is taken and quietly wrong ever after; these go stale only when the
> feature changes, and are put right with:
>
> ```
> cargo test -p buzz-export --test doc_figures -- --ignored --nocapture
> ```

- `banner.png`: Application logo banner
- `workspace_overview.png`: Annotated full-window workspace layout
- `workspace_debug_hud.png`: Real-time telemetry, zoom level, and rendering performance
- `stage_lighting_rig.png`: Multi-lamp interactive stage setup
- `lighting_comparison_lit.png`: Side-by-side comparison of lit vs flat vector scene
- `lighting_falloff_editor.png`: Studio lighting falloff rings and dial controls
- `vector_shadow_geometry.png`: Real-time vector cast shadow polygon calculations
- `character_with_lamp.png`: Character with dynamic practical lamp and shadow interaction
