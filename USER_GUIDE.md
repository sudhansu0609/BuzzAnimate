<p align="center">
  <img src="docs/images/banner.png" alt="BuzzAnimate Banner" width="100%">
</p>

# <img src="docs/images/logo-64.png" width="32" align="top"> BuzzAnimate — Complete User Guide & Feature Reference Manual

> **Welcome to BuzzAnimate!** This comprehensive guide is designed for first-time animators and artists transitioning from Adobe Animate / Flash. Every tool, panel, menu command, shortcut, and workflow is documented, illustrated with snapshots, and cross-referenced in an exhaustive alphabetical index.

---

## 📑 Table of Contents & Quick Navigation

1. [Introduction & Architectural Highlights](#1-introduction--architectural-highlights)
2. [First-Time Setup & Launching the App](#2-first-time-setup--launching-the-app)
3. [Visual Tour of the Workspace](#3-visual-tour-of-the-workspace)
4. [Canvas Navigation & Unbounded Zoom](#4-canvas-navigation--unbounded-zoom)
5. [The Vector Drawing Engine: Merge Shapes vs. Object Drawing](#5-the-vector-drawing-engine-merge-shapes-vs-object-drawing)
6. [Comprehensive 23-Tool Catalogue](#6-comprehensive-23-tool-catalogue)
7. [Advanced Brushes, Patterns & Gradients](#7-advanced-brushes-patterns--gradients)
8. [Timeline Mastery & Frame-by-Frame Animation](#8-timeline-mastery--frame-by-frame-animation)
9. [The 7 Layer Kinds & Layer Hierarchy](#9-the-7-layer-kinds--layer-hierarchy)
10. [Tweens & The Motion Editor](#10-tweens--the-motion-editor)
11. [Symbols, Instances & The Library](#11-symbols-instances--the-library)
12. [Rigging, FABRIK Inverse Kinematics & Warping](#12-rigging-fabrik-inverse-kinematics--warping)
13. [Studio Vector Lighting & Shadow Engine](#13-studio-vector-lighting--shadow-engine)
14. [Vector Filters & Blend Modes](#14-vector-filters--blend-modes)
15. [Spatial 3D Camera & Layer Parallax](#15-spatial-3d-camera--layer-parallax)
16. [Soundtrack, Waveforms & Automated Lip Sync](#16-soundtrack-waveforms--automated-lip-sync)
17. [Production Staging, Directing & Physics Modifiers](#17-production-staging-directing--physics-modifiers)
18. [Import & Export Pipelines](#18-import--export-pipelines)
19. [JavaScript Automation (JSFL API)](#19-javascript-automation-jsfl-api)
20. [Workspace Customization & Layouts](#20-workspace-customization--layouts)
21. [Complete Keyboard Shortcuts Reference](#21-complete-keyboard-shortcuts-reference)
22. [Troubleshooting & Pro Tips](#22-troubleshooting--pro-tips)
23. [Alphabetical Master Feature Index (A–Z)](#23-alphabetical-master-feature-index-az)

---

## 1. Introduction & Architectural Highlights

BuzzAnimate is a ground-up, GPU-accelerated vector animation suite built in Rust. It reconstructs the industry-standard workflow of Adobe Animate while overcoming decades-old limits inherited from 1996-era Flash.

```
┌───────────────────────────────────────────────────────────────────────────┐
│                          BUZZANIMATE ENGINE ARCHITECTURE                  │
├────────────────────────────────┬──────────────────────────────────────────┤
│ Feature                        │ Benefit for Animators                    │
├────────────────────────────────┼──────────────────────────────────────────┤
│ 🚀 Unbounded Zoom              │ Zoom into fine eyelashes or microscopic  │
│    (Verified to 2×10¹⁴%)       │ details without pixellation or caps.     │
├────────────────────────────────┼──────────────────────────────────────────┤
│ ⚡ Multi-Threaded Engine        │ Work-stealing thread pool utilizes all   │
│    (Interactive + Background)  │ CPU cores for instant responsiveness.    │
├────────────────────────────────┼──────────────────────────────────────────┤
│ 🎮 Compute-Shader Rendering    │ GPU rasterisation powered by Vello/wgpu. │
├────────────────────────────────┼──────────────────────────────────────────┤
│ 💡 Studio Vector Lighting       │ Suns, skies, lamps, gloom, and fires     │
│    & Cast Shadows              │ with real-time vector cast shadows.      │
├────────────────────────────────┼──────────────────────────────────────────┤
│ 🎥 Spatial 3D Camera           │ Pitch, yaw, and roll with genuine        │
│                                │ perspective projection & 2.5D parallax.  │
├────────────────────────────────┼──────────────────────────────────────────┤
│ 🦾 FABRIK Inverse Kinematics   │ Character skeletons, bone constraints,   │
│    & Pose Library              │ angle limits, and puppet warping.        │
├────────────────────────────────┼──────────────────────────────────────────┤
│ 👄 Automated Lip Sync          │ Audio waveform analysis into 10 visemes  │
│                                │ driving mouth symbol keyframes.          │
└────────────────────────────────┴──────────────────────────────────────────┘
```

---

## 2. First-Time Setup & Launching the App

### 2.1 Starting BuzzAnimate on Windows
- **Double-click `BuzzAnimate.bat`** in the application folder.
- The launcher verifies if changes have been made, compiles automatically if needed, and starts the application immediately.
- To create a permanent desktop icon, double-click **`Create Desktop Shortcut.bat`**.

```bat
:: Launcher Command-Line Options
BuzzAnimate.bat                        :: Opens a fresh, empty document
BuzzAnimate.bat "C:\Projects\Shot1.buzz" :: Opens an existing project
BuzzAnimate.bat --gpu NVIDIA           :: Forces a dedicated discrete GPU
BuzzAnimate.bat --integrated           :: Runs on power-efficient integrated GPU
BuzzAnimate.bat --script setup.js      :: Runs an automated startup script
BuzzAnimate.bat --dev                  :: Runs the debug build
```

### 2.2 Startup Console & GPU Adapter Table
When BuzzAnimate launches, a companion diagnostic console window opens. It scores every graphics adapter on your system and binds to the highest-performing hardware GPU:

```
   [0]   1110  NVIDIA GeForce RTX 5060 Ti     DiscreteGpu/Vulkan
   [1]    390  Intel(R) UHD Graphics 770      IntegratedGpu/Vulkan
-> [2]   1120  NVIDIA GeForce RTX 5060 Ti     DiscreteGpu/Dx12      <- selected
   [6]    n/a  Microsoft Basic Render Driver  Cpu/Dx12              <- disqualified
```
*(Closing the diagnostic console window will cleanly close the editor).*

---

## 3. Visual Tour of the Workspace

![BuzzAnimate Workspace Overview](docs/images/workspace_overview.png)

BuzzAnimate's user interface is partitioned into 6 functional zones:

```
┌─────────────────────────────────────────────────────────────────────────────────┐
│ 1. MENU BAR  (File, Edit, View, Insert, Modify, Control, Window, Help)           │
├───────┬────────────────────────────────────────────────────────┬────────────────┤
│ 2.    │ 3. STAGE & PASTEBOARD                                  │ 5. INSPECTOR & │
│ TOOL  │                                                        │    PROPERTIES  │
│ STRIP │  • White Stage: Exported camera boundaries             │  • Properties  │
│ (23   │  • Dark Grey Pasteboard: Offscreen drafting canvas     │  • Color       │
│ Tools)│  • Rulers, Snapping Guides & Light Gizmos              │  • Swatches    │
│       │  • Top-Right Stage Controls: Zoom presets & HUD        │  • Filters     │
├───────┴────────────────────────────────────────────────────────┤  • Lighting    │
│ 4. TIMELINE & PLAYBACK CONTROLS                                │  • Rigging     │
│  • Layers stack, Folders, Masks, Onion Skin, Looping section   │  • Sound / Lip │
│  • Frame ruler, Keyframes, Spans, Tweens, Audio Waveform       │  • Library     │
├────────────────────────────────────────────────────────────────┴────────────────┤
│ 6. DOCKABLE PANELS & BACKGROUND TASKS                                          │
└─────────────────────────────────────────────────────────────────────────────────┘
```

### Key UI Elements
1. **Menu Bar**: Complete file, editing, transformation, and document operations.
2. **Tool Strip**: Vertical toolbar grouped into selection, drawing, bone rigging, color, and navigation tools. Each button displays its single-letter keyboard shortcut.
3. **Stage & Pasteboard**: The active creative canvas. The white rectangle represents the printable/renderable frame. The surrounding dark gray pasteboard holds off-stage props, character turnarounds, and drafts.
4. **Timeline**: Frame sequencer displaying layers, keyframe spans, tweens, audio tracks, and onion skinning.
5. **Inspector / Property Docks**: Tabbed or stacked inspector panels for properties, swatches, layer depth, lighting, rigging, and sound.
6. **Task Bar & Notifications**: Real-time status for background video encoding, file imports, and auto-saves.

---

## 4. Canvas Navigation & Unbounded Zoom

BuzzAnimate eliminates the traditional 2,000% zoom ceiling found in older software. You can zoom to **2×10¹⁴%** without precision collapse or floating-point wobbles.

![Telemetry HUD and Debug Overlay](docs/images/workspace_debug_hud.png)

### Navigation Quick Controls
| Action | Input / Shortcut |
|---|---|
| **Pan Canvas** | `Spacebar + Drag` or `Middle-Click Drag` |
| **Smooth Zoom** | `Mouse Wheel Scroll` (centers directly under cursor) |
| **Step Zoom In** | `Ctrl + =` or `Z` tool click |
| **Step Zoom Out** | `Ctrl + -` or `Alt + Z` tool click |
| **100% Actual Size** | `Ctrl + 1` |
| **Show Frame (Stage)**| `Ctrl + 2` |
| **Fit in Window** | `Ctrl + 3` |
| **Show All Objects** | `Ctrl + Shift + W` |

> [!TIP]
> **Understanding the HUD**: Look at the top-right stage HUD overlay. It reports the active zoom generation (`gen`), ink coverage percentage, GPU draw time (`~0.9ms`), and screen precision down to sub-atomic scales.

---

## 5. The Vector Drawing Engine: Merge Shapes vs. Object Drawing

BuzzAnimate supports two distinct vector models. Understanding their interaction is essential for rapid drawing:

```
  MERGE SHAPE (Default)                 OBJECT DRAWING (Toggle 'J')
┌───────────────────────┐             ┌───────────────────────┐
│  Shape A    Shape B   │             │ ┌─────────┐           │
│   ┌────┐     ┌────┐   │             │ │ Shape A │ ┌─────────┤
│   │    ├─────┤    │   │             │ └─────────┤ │ Shape B │
│   └────┴─────┴────┘   │             │           └─┴─────────┘
│ Overlapping same-color│             │ Shapes remain independent │
│ shapes fuse. Different│             │ objects with bounding     │
│ colors cut each other!│             │ boxes and stacking order. │
└───────────────────────┘             └───────────────────────┘
```

### 1. Merge Shapes (Classic Flash Model)
- Shapes reside raw on the layer canvas.
- When two shapes of the **same color** touch, they seamlessly **fuse** into a single continuous polygon.
- When a shape of a **different color** is drawn over another, it **cuts away** the underlying geometry like a cookie cutter.
- **Carving with Lasso (`L`)**: Dragging the Lasso across a merge shape cuts the artwork along the drawn stroke!

### 2. Object Drawing Mode (Press `J` to toggle)
- Draws vector items inside protected containers with blue bounding boxes.
- Overlapping shapes do not merge or cut each other.
- Use **`Ctrl + B` (Break Apart)** to convert an Object Drawing back into Merge Shapes.

---

## 6. Comprehensive 23-Tool Catalogue

The toolbar is organized into logical functional blocks. The table below indexes every tool:

| Icon / Key | Tool Name | Shortcut | Group | Primary Purpose & Features |
|:---:|---|:---:|---|---|
| **V** | **Selection** | `V` | Select | Click to select fills or strokes. Double-click to select connected geometry. Drag fill edges to bend curves. |
| **A** | **Subselection** | `A` | Select | Direct vertex editing. Reveals cubic Bézier anchor points and tangent handles. |
| **L** | **Lasso** | `L` | Select | Freehand regional selection. Slices through merge artwork when dragged across it. |
| **G** | **Magic Wand** | `G` | Select | Click any color region to select all contiguous pixels/vectors within a set color tolerance. |
| **Q** | **Free Transform**| `Q` | Transform | Rotate, scale, skew, and reposition the pivot/origin point of objects, groups, and symbols. |
| **◑** | **Gradient Transform**| *(Menu)* | Transform | Move, scale, rotate, and adjust focal radius of linear and radial gradient fills. |
| **P** | **Pen** | `P` | Draw | Click to lay down straight anchor points; click-and-drag to create smooth Bézier arcs. |
| **T** | **Text** | `T` | Draw | Typesetting and title tool *(planned font shaping engine)*. |
| **N** | **Line** | `N` | Draw | Draws clean straight vector lines. Hold `Shift` to constrain to 45° increments. |
| **R** | **Rectangle** | `R` | Shapes | Draws rectangles and squares (`Shift`). Adjust Corner Radius in Properties for rounded boxes. |
| **O** | **Oval** | `O` | Shapes | Draws ellipses and perfect circles (`Shift`). |
| **☆** | **PolyStar** | *(Menu)* | Shapes | Draws regular polygons or stars. Customize vertex count and star-point depth in Properties. |
| **Y** | **Pencil** | `Y` | Freehand | Freehand strokes with 3 modes: **Straighten** (cleans lines), **Smooth** (cleans curves), **Ink** (raw). |
| **B** | **Brush** | `B` | Freehand | Rich painting with 7 brush models: Fluid, Normal, Pattern, Art, Soft, Effect, and Wave. |
| **M** | **Bone** | `M` | Rigging | Builds kinematic skeletons across shapes and symbols. Sets up FABRIK inverse kinematics. |
| **W** | **Asset Warp** | `W` | Rigging | Drops puppet warp pins onto vector artwork or images using Moving Least Squares (MLS). |
| **J** | **Motion Path** | `J` | Rigging | Draws a freehand vector curve; selected symbols follow and orient along the curve. |
| **K** | **Paint Bucket**| `K` | Color | Fills enclosed vector regions. Includes gap detection (Don't Close, Close Small/Medium/Large). |
| **S** | **Ink Bottle** | `S` | Color | Applies or changes stroke color, weight, and style to the outlines of existing shapes. |
| **I** | **Eyedropper** | `I` | Color | Samples fill color, stroke color, or stroke style directly from the stage. |
| **E** | **Eraser** | `E` | Edit | Erases strokes and fills. Modes: Erase Normal, Erase Fills, Erase Lines, Erase Inside. |
| **C** | **Camera** | `C` | View | Controls the cinematic 3D camera: pan, zoom, roll, pitch, and yaw. |
| **H** | **Hand** | `H` | View | Pans across the stage without affecting camera position or selection. |
| **Z** | **Zoom** | `Z` | View | Click to zoom in; `Alt + Click` to zoom out. Drag a marquee to zoom to a specific area. |

---

## 7. Advanced Brushes, Patterns & Gradients

The Brush tool (`B`) includes 7 specialized brush models selectable in the Properties panel:

```
┌─────────────┬─────────────────────────────────────────────────────────────┐
│ Brush Kind  │ Description & Best Use Case                                 │
├─────────────┼─────────────────────────────────────────────────────────────┤
│ 1. Fluid    │ Dynamic velocity-sensitive stroke. Thins when drawn fast    │
│             │ and fattens when drawn slow. Perfect for calligraphy & inking│
├─────────────┼─────────────────────────────────────────────────────────────┤
│ 2. Normal   │ Uniform-width marker stroke with optional start/end taper.  │
├─────────────┼─────────────────────────────────────────────────────────────┤
│ 3. Pattern  │ Stamps vector patterns repeatedly along the drawn stroke.   │
│             │ Built-ins: Dot, Dash, Leaf, Star, Arrow, Diamond, Custom.   │
├─────────────┼─────────────────────────────────────────────────────────────┤
│ 4. Art      │ Takes a single vector master shape and stretches it over    │
│             │ the entire length of your stroke.                           │
├─────────────┼─────────────────────────────────────────────────────────────┤
│ 5. Soft     │ Raster airbrush engine that renders soft-edge glowing strokes│
│             │ directly beside vector artwork.                             │
├─────────────┼─────────────────────────────────────────────────────────────┤
│ 6. Effect   │ Procedural scenery brush: paints falling snow, clouds,      │
│             │ silhouettes, or decorative string lights in a single stroke.│
├─────────────┼─────────────────────────────────────────────────────────────┤
│ 7. Wave     │ Animated flowing brush for smoke, flowing water, and hair.  │
│             │ Automatically advances its wave phase across frames!        │
└─────────────┴─────────────────────────────────────────────────────────────┘
```

### Custom Pattern Brush from Selection
1. Draw any vector shape on the stage (e.g., a custom stitch, leaf, or emblem).
2. Select it using the Selection tool (`V`).
3. Choose **`Modify ▸ Create Brush From Selection`**.
4. Switch to the Brush Tool (`B`), select **Pattern**, and pick **From Selection**. Your drawing is now an active brush stamp!

### Gradients & Gradient Transform
- **Fills**: Open the **Color** panel to select **Linear Gradient** or **Radial Gradient**.
- Add, remove, and slide color stops along the gradient ramp.
- Activate the **Gradient Transform tool (`◑`)** on any gradient-filled shape:
  - Drag the **center circle** to reposition the focal hotspot.
  - Drag the **outer ring** to rotate the gradient angle.
  - Drag the **square handle** to stretch or squash the gradient width.

---

## 8. Timeline Mastery & Frame-by-Frame Animation

The timeline at the bottom of the window controls playback and animation timing:

```
Layer 3: Eyes       [●] [  F6  ][   F5   ][  F7  ][   F5   ]
Layer 2: Head       [●] [             F6                   ] (Classic Tween ──────>) [ F6 ]
Layer 1: Audio      [●] [~v~~V~~v~~~V~~v~~~V~~v~~~V~~~v~~~~] (Soundtrack: "Dialogue.wav")
                        |----|----|----|----|----|----|----|
Frame:                  1    5    10   15   20   25   30
```

### Frame Hotkeys & Fundamentals
- **Frame (`F5`)**: Extends the previous drawing across time without changing it.
- **Remove Frame (`Shift + F5`)**: Deletes the selected frame span, shortening playback duration.
- **Keyframe (`F6`)**: Creates a new keyframe that duplicates the previous drawing so you can alter it.
- **Blank Keyframe (`F7`)**: Creates a blank frame to begin a completely fresh drawing.
- **Clear Keyframe (`Shift + F6`)**: Removes a keyframe, turning the cell back into an extended span.
- **Clear Frames (`Alt + Backspace`)**: Empties contents of the frame while preserving span length.
- **Cut / Copy / Paste Frames**: `Ctrl + Alt + X` / `Ctrl + Alt + C` / `Ctrl + Alt + V`.

### Playback & Onion Skinning
- **Play / Pause**: Press `Enter` or the transport play button.
- **Step Forward / Backward**: Press `.` (period) to step forward one frame; `,` (comma) to step back.
- **First / Last Frame**: Press `Home` / `End`.
- **Onion Skinning (`Alt + Shift + O`)**: Displays ghosted silhouettes of preceding and succeeding frames. Drag the green and blue range markers in the frame ruler to broaden or narrow the preview window.
- **Edit Multiple Frames**: Select and transform shapes across dozens of keyframes simultaneously!
- **Animate on Twos (`Modify ▸ On Twos`)**: Automatically holds every keyframe for 2 frames across the selected span, instantly achieving traditional cinematic 12fps cadence in a 24fps project.

---

## 9. The 7 Layer Kinds & Layer Hierarchy

BuzzAnimate supports 7 specialized layer types:

```
[+] Folder: Character
 ├── [Mask] Mask: Shadow_Cut
 ├── [Inverse Mask] InvMask: Hole_In_Wall  <-- BuzzAnimate Exclusive!
 ├── [Normal] Head
 ├── [Normal] Torso
 ├── [Guide] Sketch_Reference
 └── [Guided] Path_Follower
```

1. **Normal Layer**: Standard layer containing drawings, shapes, and symbol instances.
2. **Folder Layer**: Groups related layers together. Collapsible to organize complex scenes.
3. **Mask Layer**: Clips child layers indented beneath it. Only artwork falling *inside* the mask's shapes remains visible.
4. **Inverse Mask Layer** *(Exclusive to BuzzAnimate)*: Inverts standard masking — hides everything *inside* the mask shape while keeping everything *outside* visible! Essential for cutting dynamic holes, doorways, and silhouettes.
5. **Masked Layer**: A child layer governed by an active mask above it.
6. **Guide Layer**: Reference artwork layer (e.g. rough model sheets or rotoscope video). Displays faded on the stage and is **never exported** to the final render.
7. **Guided Layer**: A layer attached to a guide curve (e.g. for motion paths).

### Layer Parenting
- Drag a layer row and nest it under another layer to create a parent-child relationship (e.g., *Head* parented to *Neck*, parented to *Torso*).
- When the parent moves, rotates, or scales, the child follows automatically — **without requiring a skeletal bone rig**!

### Layer Depth & 2.5D Parallax
Open the **Layer Depth** panel (`Window ▸ Layer Depth`). It displays a side-view cross-section of your scene:
- Drag layers along the Z-axis to position them closer to or further from the camera.
- When the Camera pans or tilts, background layers automatically exhibit realistic parallax!

---

## 10. Tweens & The Motion Editor

BuzzAnimate offers three tween engines to automate in-between frames:

### 1. Classic Tween (`Insert ▸ Create Classic Tween`)
- Best for symbol instances.
- Place Symbol A on Keyframe 1, and the same Symbol A on Keyframe 20 in a new position/rotation. Right-click the span and choose **Create Classic Tween**. The timeline turns light purple with a connecting arrow.

### 2. Motion Tween (`Insert ▸ Create Motion Tween`)
- Object-based continuous tweening.
- Automatically records property changes (position, scale, rotation, color effect, filter intensity) at any frame without creating explicit keyframes.

### 3. Shape Tween (`Insert ▸ Create Shape Tween`)
- Organic vector morphing between Merge Shapes (e.g., a star morphing into a circle).
- Both start and end keyframes must contain raw, ungrouped vector shapes.

### The Motion Editor
Open `Window ▸ Motion Editor` to inspect the cubic Bézier acceleration curves of the active tween. Drag control handles to adjust ease-in, ease-out, bounce, and elastic timing.

---

## 11. Symbols, Instances & The Library

Symbols are reusable master assets stored inside the project **Library** (`Window ▸ Library`). Using symbols reduces file size and allows global asset updates across the entire production.

```
┌─────────────────────────────────────────────────────────────┐
│                      SYMBOL TYPES                           │
├───────────────┬─────────────────────────────────────────────┤
│ Graphic       │ Synced 1:1 with the main timeline.          │
│               │ Scrubbable on the stage; ideal for lip-sync,│
│               │ walk cycles, and character expressions.     │
├───────────────┼─────────────────────────────────────────────┤
│ MovieClip     │ Self-contained timeline that loops          │
│               │ independently of the parent scene.          │
├───────────────┼─────────────────────────────────────────────┤
│ Button        │ Interactive 4-frame asset:                  │
│               │ [Up] [Over] [Down] [Hit]                    │
└───────────────┴─────────────────────────────────────────────┘
```

### Creating and Editing Symbols
- **Convert Selection to Symbol**: Select shapes on the stage and press **`F8`**. Give it a name and choose the symbol type.
- **Create Empty Symbol**: Press **`Ctrl + F8`**.
- **Edit in Place**: Double-click any symbol instance on the stage or press **`Ctrl + E`**. The rest of the scene dims, allowing you to edit the symbol's internal timeline.
- **Return to Main Document**: Double-click empty canvas space or press **`Ctrl + F4`**.

### The Library Panel & Vector Thumbnails
- Every symbol in the Library displays a **live-rendered vector thumbnail**.
- Organize symbols into folders.
- **Swap Symbol (`Modify ▸ Swap Symbol`)**: Replace an instance on the stage with a different library symbol while preserving position, scale, and tweens.

---

## 12. Rigging, FABRIK Inverse Kinematics & Warping

BuzzAnimate includes a built-in character rigging and deformation suite (`Window ▸ Rigging`).

![Character Rigging and Stage Lighting](docs/images/character_with_lamp.png)

### Skeletons with the Bone Tool (`M`)
1. Place character limbs on consecutive layers or convert them to symbols.
2. Select the **Bone Tool (`M`)**.
3. Drag from the pelvis to the chest, shoulder to elbow, and elbow to wrist to build a kinematic chain.
4. **FABRIK IK Solver**: Grab the hand or foot and drag. The entire limb flexes and reaches naturally!
5. **Joint Limits & Pins**: In the Rigging panel, set rotation constraints (e.g. elbow limited to 0°–145°) and pin foot bones to prevent floor sliding.

### The Pose Library
- Click **Save Pose** in the Rigging panel to save character stances (e.g., "Idle", "Contact", "Recoil", "Jump").
- **Mirror Pose**: Instantly flips left and right limbs for effortless symmetric walk and run cycles.
- **Pose-to-Pose Keying**: BuzzAnimate automatically interpolates between stored library poses across the timeline!

### Asset Warp (`W`)
- Select the Asset Warp tool and click directly on vector drawings or imported bitmaps to drop mesh deformation pins.
- Drag pins to bend and squash artwork organically via Moving Least Squares (MLS) mathematics.

---

## 13. Studio Vector Lighting & Shadow Engine

BuzzAnimate features a specialized studio vector lighting system (`Window ▸ Lighting`). Lights are real-time vector mathematical primitives that calculate surface shading, highlights, and cast shadow polygons.

![Lighting Proof: Side-by-Side Comparison](docs/images/lighting_comparison_lit.png)
*(Above: A stage illuminated by vector lighting vs unlit flat artwork)*

### 5 Specialized Light Types
```
┌───────────┬─────────────────────────────────────────────────────────────────┐
│ Light     │ Characteristics & Behavior                                      │
├───────────┼─────────────────────────────────────────────────────────────────┤
│ 1. Sun    │ Directional sunlight across the entire stage. Aimed via an      │
│           │ intuitive interactive 360° celestial dial.                      │
├───────────┼─────────────────────────────────────────────────────────────────┤
│ 2. Sky    │ Hemispherical ambient fill illumination with no hard shadows.   │
├───────────┼─────────────────────────────────────────────────────────────────┤
│ 3. Lamp   │ Point light with adjustable origin, radius, color, and smooth   │
│           │ quadratic falloff.                                              │
├───────────┼─────────────────────────────────────────────────────────────────┤
│ 4. Gloom  │ Negative light / shadow wall that subtracts illumination.       │
│           │ Throws theatrical darkness into back corners.                   │
├───────────┼─────────────────────────────────────────────────────────────────┤
│ 5. Fire   │ Dynamic guttering hearth light. Fluctuates warm orange-yellow   │
│           │ intensity and shadow length automatically across frames!        │
└───────────┴─────────────────────────────────────────────────────────────────┘
```

![Lighting Falloff and Stage Gizmos](docs/images/lighting_falloff_editor.png)

### Working with Light Gizmos
- Press **`Ctrl + Shift + L`** to show/hide stage light gizmos.
- Drag a lamp's central handle to move it. Drag the outer radius ring to adjust falloff range.
- **Cast Shadows**: Check *Cast Shadows* on any lamp or sun. Shadow polygons are computed directly against vector artwork contours:

![Vector Cast Shadows](docs/images/vector_shadow_geometry.png)

- **Keyframed Lighting**: Click **Add Light Keyframe** in the Lighting panel to animate light position, color, and intensity along the timeline (e.g., a flashlight sweeping across a room or a flickering torch).

---

## 14. Vector Filters & Blend Modes

Select an object or an entire layer and open the **Filters** panel (`Window ▸ Filters`):

```
┌─────────────────┬─────────────────────────────────────────────────────────┐
│ Vector Filter   │ Adjustable Parameters                                   │
├─────────────────┼─────────────────────────────────────────────────────────┤
│ Blur            │ X Radius, Y Radius, Quality (Low, Medium, High).        │
├─────────────────┼─────────────────────────────────────────────────────────┤
│ Drop Shadow     │ Distance, Angle, Shadow Color, Blur, Strength, Knockout,│
│                 │ Inner Shadow, Hide Object.                              │
├─────────────────┼─────────────────────────────────────────────────────────┤
│ Glow            │ Blur, Strength, Color, Inner Glow, Knockout.            │
├─────────────────┼─────────────────────────────────────────────────────────┤
│ Bevel           │ Distance, Angle, Highlight Color, Shadow Color, Blur,   │
│                 │ Strength, Bevel Type (Inner, Outer, Full).              │
├─────────────────┼─────────────────────────────────────────────────────────┤
│ Adjust Color    │ Brightness, Contrast, Saturation, Hue Rotation (-180°..+180°).│
└─────────────────┴─────────────────────────────────────────────────────────┘
```

### 10 Blend Modes
Choose blend modes on any symbol or layer:
- **Normal**, **Layer**, **Darken**, **Multiply**, **Lighten**, **Screen**, **Overlay**, **Hard Light**, **Add**, and **Difference**.

---

## 15. Spatial 3D Camera & Layer Parallax

Select the **Camera tool (`C`)** to activate the cinematic stage camera.

```
       [Camera Viewport]
            /      \
           /  Roll  \      <-- Pitch & Yaw rotate the stage into 3D!
          /          \
  ┌──────┴────────────┴──────┐
  │  Stage in 3D Perspective │  <-- Objects turn as flat cards in depth!
  └──────────────────────────┘
```

### Camera Controls
- **Pan & Zoom**: Click and drag on the stage with the Camera tool to pan. Use the stage zoom slider or scroll wheel to zoom the lens.
- **Roll (Rotate)**: Drag the rotation wheel in the camera stage chrome to tilt the camera.
- **Pitch & Yaw (Spatial 3D)**: Rotate the stage in genuine 3D perspective. The camera renders the stage trapezoidally, and vector cards rotate in depth.
- **Camera Keyframing**: The timeline includes a dedicated **Camera Track**. Insert keyframes (`F6`) on the camera track to execute sweeping cinematic dollies, tracking shots, and dramatic zooms.

---

## 16. Soundtrack, Waveforms & Automated Lip Sync

BuzzAnimate features an integrated audio playback and phonetic lip-sync analysis engine.

### Importing Audio
- Select **`File ▸ Import ▸ Import Sound…`** or press **`Ctrl + R`**.
- Supported audio formats: **`.wav`, `.mp3`, `.ogg`, `.flac`, `.m4a`, `.aac`**.
- The soundtrack renders its full audio waveform directly on the timeline layer.
- **Detect Musical Beats**: Choose **`Control ▸ Detect Beats`** to mark rhythmic percussion beats with vertical ticks on the frame ruler.

### Automated Lip Sync in 3 Steps
1. Choose **`File ▸ New Mouth Symbol`**. BuzzAnimate creates a Graphic Symbol pre-populated with 10 labeled viseme mouth shapes:

```
Frame 0: Rest   (Closed/Relaxed)    Frame 5: L     (Tongue up)
Frame 1: Ai     (Open vowel)        Frame 6: WQ    (Pursed/tight)
Frame 2: E      (Wide vowel)        Frame 7: MBP   (Lips pressed)
Frame 3: O      (Round open)        Frame 8: FV    (Teeth on lip)
Frame 4: U      (Pursed vowel)      Frame 9: Etc   (Consonants: D,T,S,K)
```

2. Select **`File ▸ Lip Sync…`** (`Window ▸ Lip Sync`).
3. Select your dialogue audio track, choose the mouth symbol, and pick the destination character layer. Click **Confirm**.
4. BuzzAnimate analyzes the audio frequencies and automatically assigns corresponding mouth keyframes across the timeline!

---

## 17. Production Staging, Directing & Physics Modifiers

The **`buzz-act`** subsystem accelerates scene layout and automated character acting (`Modify ▸ Staging`):

### 1. Set the Scene (`Modify ▸ Staging ▸ Set the Scene…`)
Quickly builds a staged environment complete with ground plane, backdrop, and multi-point lighting.
- **Daylight**: High sun, blue ambient sky, short crisp shadows.
- **Sunset**: Low warm sun, amber horizon sky, elongated dramatic shadows.
- **Night**: Midnight sky with a focused practical warm lamp.
- **Interior**: Floor, wall plane, and interior ceiling luminaire.

### 2. Direct a Story (`Modify ▸ Staging ▸ Direct a Story…`)
Input a text script or narrative beat list. BuzzAnimate generates timed blocking and character placement directly on the timeline.

### 3. Procedural Physics Modifiers
- **Add Follow-Through (`Modify ▸ Add Follow-Through…`)**: Applies a damped-spring physics model to hair, antennae, tails, and loose clothing. When the character moves, secondary motion responds automatically!
- **Add Wiggle (`Modify ▸ Add Wiggle…`)**: Generates deterministic organic jitter for handheld camera shake, wind gusts, or breathing idle sway.
- **Bake Modifiers (`Modify ▸ Bake Modifiers`)**: Converts live physics calculations into editable keyframes.

---

## 18. Import & Export Pipelines

### 18.1 File Import Support
- **Adobe Animate Projects**: Reads uncompressed `.xfl` and modern `.fla` archives.
- **Adobe Flash SWF**: Extracts vector shapes, morphs, and symbol definitions from `.swf`.
- **Adobe Illustrator & Vector PDF**: Reads vector graphics, paths, and gradients from `.ai` and `.pdf`.
- **Raster Images & Sequences**: Imports PNG, JPEG, and numbered animation folders.
- **Video Reference Layer**: Imports MP4/MOV footage onto a Guide Layer for rotoscoping.

### 18.2 Export Formats & Background Render Queue
Open the Export dialog via **`File ▸ Export`**:

```
┌───────────────────────────────────────────────────────────────────────────┐
│                             EXPORT OPTIONS                                │
├───────────────────┬───────────────────────────────────────────────────────┤
│ Image (PNG)       │ Exports the current frame as high-res PNG (32-bit     │
│                   │ transparent RGBA or white background).               │
├───────────────────┼───────────────────────────────────────────────────────┤
│ PNG Sequence      │ Exports numbered image sequence (`frame_0001.png`...).│
├───────────────────┼───────────────────────────────────────────────────────┤
│ Video (MP4 / MOV) │ Hardware-accelerated GPU encoding via NVENC           │
│                   │ (`h264_nvenc`, `hevc_nvenc`, `av1_nvenc`) with        │
│                   │ software fallback. Muxes soundtrack automatically!    │
├───────────────────┼───────────────────────────────────────────────────────┤
│ ProRes 4444 (.mov)│ Broadcast-quality video preserving full 8-bit alpha   │
│                   │ transparency for downstream video compositing.        │
├───────────────────┼───────────────────────────────────────────────────────┤
│ Animated GIF/WebP │ Highly-optimized web animations with palette dithering│
│                   │ and transparency.                                     │
├───────────────────┼───────────────────────────────────────────────────────┤
│ Animate .fla      │ Exports project back out to Adobe Animate format.     │
└───────────────────┴───────────────────────────────────────────────────────┘
```

> [!NOTE]
> **Non-Blocking Background Tasks**: All video and sequence exports run on asynchronous background threads. Open the **Tasks panel (`Window ▸ Tasks`)** to monitor progress bars, cancel jobs, or queue multiple exports while continuing to animate without interruption.

---

## 19. JavaScript Automation (JSFL API)

BuzzAnimate includes a sandboxed ECMAScript engine that supports Adobe Animate's `fl` / `document` scripting API. Open the **Actions Panel (`F9`)**:

```javascript
// Example: Generate an animated grid of shapes
var doc = fl.getDocumentDOM();
var timeline = doc.getTimeline();

fl.trace("Automating layout on document: " + doc.name);

// Create a new layer
timeline.addNewLayer("Generated_Art");

// Draw 10 circles across the stage
for (var i = 0; i < 10; i++) {
    doc.addNewOval({
        left: 50 + (i * 45),
        top: 200,
        right: 90 + (i * 45),
        bottom: 240
    });
}

fl.trace("Created 10 vector circles successfully!");
```

- Press **`Ctrl + Enter`** inside the Actions panel to execute scripts.
- Use `fl.trace(message)` to print feedback to the Script Output area.

---

## 20. Workspace Customization & Layouts

### Docking, Floating & Tabs
- Every panel (Layers, Properties, Swatches, Rigging, Lighting, Depth, Library) can be docked to the Left, Right, Bottom, or torn off as an independent Floating Window.
- Combine panels into tabbed sections to conserve screen space.
- **Lock Layout (`Ctrl + Alt + L`)**: Freezes all panel borders and docks to prevent accidental dragging during intense drawing sessions.
- **Reset Workspace (`Window ▸ Reset Layout`)**: Restores all panels to default factory docking.
- **Command Palette (`Ctrl + K`)**: Press `Ctrl + K` to summon an instant spotlight search bar. Type any command or tool name and hit `Enter` to run it immediately.
- **Interface Theme**: Switch between **Studio Dark** and **Paper Light** in `Window ▸ Light Interface`.

---

## 21. Complete Keyboard Shortcuts Reference

### Tool Shortcuts
| Key | Tool | Key | Tool |
|:---:|---|:---:|---|
| **`V`** | Selection | **`Y`** | Pencil |
| **`A`** | Subselection | **`B`** | Brush |
| **`L`** | Lasso | **`M`** | Bone (Rigging) |
| **`G`** | Magic Wand | **`W`** | Asset Warp |
| **`Q`** | Free Transform | **`J`** | Motion Path / Toggle Object Drawing |
| **`P`** | Pen | **`K`** | Paint Bucket |
| **`T`** | Text | **`S`** | Ink Bottle |
| **`N`** | Line | **`I`** | Eyedropper |
| **`R`** | Rectangle | **`E`** | Eraser |
| **`O`** | Oval | **`C`** | Camera |
| **`H`** | Hand (Pan) | **`Z`** | Zoom |

### Timeline & Animation Shortcuts
| Shortcut | Command |
|---|---|
| **`F5`** | Insert Frame (extend span) |
| **`Shift + F5`** | Remove Frame |
| **`F6`** | Insert Keyframe |
| **`Shift + F6`** | Clear Keyframe |
| **`F7`** | Insert Blank Keyframe |
| **`Alt + Backspace`** | Clear Frames (empty span) |
| **`Ctrl + Alt + X`** | Cut Frames |
| **`Ctrl + Alt + C`** | Copy Frames |
| **`Ctrl + Alt + V`** | Paste Frames |
| **`Enter`** | Play / Pause Timeline |
| **`.` (Period)** | Step 1 Frame Forward |
| **`,` (Comma)** | Step 1 Frame Backward |
| **`Home` / `End`** | Go to First / Last Frame |
| **`Alt + Shift + O`** | Toggle Onion Skinning |

### Selection, Editing & Symbol Shortcuts
| Shortcut | Command |
|---|---|
| **`Ctrl + Z`** | Undo |
| **`Ctrl + Shift + Z` / `Ctrl + Y`** | Redo |
| **`Ctrl + X` / `Ctrl + C` / `Ctrl + V`** | Cut / Copy / Paste |
| **`Ctrl + D`** | Duplicate Selection |
| **`Ctrl + A`** | Select All |
| **`Ctrl + Shift + A`** | Deselect All |
| **`Ctrl + G`** | Group Selection |
| **`Ctrl + Shift + G`** | Ungroup Selection |
| **`Ctrl + B`** | Break Apart (symbols, groups, object drawing) |
| **`F8`** | Convert Selection to Symbol |
| **`Ctrl + F8`** | Create New Empty Symbol |
| **`Ctrl + E`** | Edit Symbol in Place |
| **`Ctrl + F4`** | Exit Symbol Editing (Return to Document) |
| **`Arrow Keys`** | Nudge Selection by 1 unit |
| **`Shift + Arrow Keys`** | Nudge Selection by 8 units |

### Stage, View & Workspace Shortcuts
| Shortcut | Command |
|---|---|
| **`Ctrl + K`** | Open Command Palette (Search any action) |
| **`Ctrl + 1`** | Zoom 100% (Actual Size) |
| **`Ctrl + 2`** | Show Frame (Fit stage boundaries) |
| **`Ctrl + 3`** | Fit in Window |
| **`Ctrl + Shift + W`** | Show All Objects |
| **`Ctrl + Alt + Shift + R`** | Toggle Rulers |
| **`Ctrl + '`** | Toggle Grid |
| **`Ctrl + ;`** | Toggle Snapping Guides |
| **`Ctrl + Shift + U`** | Toggle Object Snapping |
| **`Ctrl + Shift + L`** | Toggle Stage Light Gizmos |
| **`Ctrl + Alt + L`** | Lock / Unlock Workspace Layout |
| **`F9`** | Toggle Actions (JavaScript Scripting) Panel |

---

## 22. Troubleshooting & Pro Tips

### 1. Vector Drawing Not Merging?
- Check if **Object Drawing** is enabled. Press **`J`** to toggle back to Merge Shape mode, or select your drawing and press **`Ctrl + B` (Break Apart)**.

### 2. Bucket Tool Not Filling a Gap?
- If lines don't meet precisely, the paint bucket may refuse to fill. In the tool properties, set **Gap Detection** to **Close Medium Gaps** or **Close Large Gaps**.

### 3. GPU Performance & Shaders
- BuzzAnimate uses modern GPU compute pipelines. Ensure your graphics drivers are updated. On multi-GPU laptops, ensure the application is bound to the high-performance discrete GPU via `BuzzAnimate.bat --gpu NVIDIA`.

### 4. Automatic Crash Recovery
- BuzzAnimate writes incremental snapshots of your document in the background. If an unexpected power outage occurs, reopening the editor will detect recovery files and prompt you to restore your document with zero data loss.

---

## 23. Alphabetical Master Feature Index (A–Z)

### A
- [Actions Panel (`F9`)](#19-javascript-automation-jsfl-api)
- [Adjust Color Filter](#14-vector-filters--blend-modes)
- [Align & Distribute](#20-workspace-customization--layouts)
- [Animation on Twos / Threes](#8-timeline-mastery--frame-by-frame-animation)
- [Armatures & Skeletons](#12-rigging-fabrik-inverse-kinematics--warping)
- [Art Brush](#7-advanced-brushes-patterns--gradients)
- [Asset Warp Tool (`W`)](#12-rigging-fabrik-inverse-kinematics--warping)
- [Audio Formats (.wav, .mp3, .ogg, .flac)](#16-soundtrack-waveforms--automated-lip-sync)
- [Automated Lip Sync](#16-soundtrack-waveforms--automated-lip-sync)
- [Autosave & Crash Recovery](#22-troubleshooting--pro-tips)

### B
- [Bake Modifiers](#17-production-staging-directing--physics-modifiers)
- [Beat Detection](#16-soundtrack-waveforms--automated-lip-sync)
- [Bevel Filter](#14-vector-filters--blend-modes)
- [Blank Keyframes (`F7`)](#8-timeline-mastery--frame-by-frame-animation)
- [Blend Modes (Multiply, Screen, Add...)](#14-vector-filters--blend-modes)
- [Blur Filter](#14-vector-filters--blend-modes)
- [Bone Tool (`M`)](#12-rigging-fabrik-inverse-kinematics--warping)
- [Break Apart (`Ctrl + B`)](#5-the-vector-drawing-engine-merge-shapes-vs-object-drawing)
- [Brush Tool (`B`) & Types](#7-advanced-brushes-patterns--gradients)
- [Button Symbols](#11-symbols-instances--the-library)

### C
- [Camera 3D Perspective](#15-spatial-3d-camera--layer-parallax)
- [Camera Tool (`C`)](#15-spatial-3d-camera--layer-parallax)
- [Cast Shadows (Vector Calculations)](#13-studio-vector-lighting--shadow-engine)
- [Classic Tweens](#10-tweens--the-motion-editor)
- [Command Palette (`Ctrl + K`)](#20-workspace-customization--layouts)
- [Convert to Symbol (`F8`)](#11-symbols-instances--the-library)
- [Custom Pattern Brushes](#7-advanced-brushes-patterns--gradients)

### D
- [Daylight Staging](#17-production-staging-directing--physics-modifiers)
- [Direct a Story](#17-production-staging-directing--physics-modifiers)
- [Dockable Panels](#20-workspace-customization--layouts)
- [Drop Shadow Filter](#14-vector-filters--blend-modes)

### E
- [Edit in Place (`Ctrl + E`)](#11-symbols-instances--the-library)
- [Edit Multiple Frames](#8-timeline-mastery--frame-by-frame-animation)
- [Effect (Scenery) Brush](#7-advanced-brushes-patterns--gradients)
- [Eraser Tool (`E`)](#6-comprehensive-23-tool-catalogue)
- [Export Formats (PNG, MP4, GIF, WebP, ProRes)](#18-import--export-pipelines)
- [Eyedropper Tool (`I`)](#6-comprehensive-23-tool-catalogue)

### F
- [FABRIK Inverse Kinematics](#12-rigging-fabrik-inverse-kinematics--warping)
- [Filters Panel](#14-vector-filters--blend-modes)
- [Fire Light](#13-studio-vector-lighting--shadow-engine)
- [Fluid Brush](#7-advanced-brushes-patterns--gradients)
- [Folder Layers](#9-the-7-layer-kinds--layer-hierarchy)
- [Follow-Through Physics](#17-production-staging-directing--physics-modifiers)
- [Free Transform Tool (`Q`)](#6-comprehensive-23-tool-catalogue)

### G
- [Gap Detection (Paint Bucket)](#6-comprehensive-23-tool-catalogue)
- [Gloom (Negative Light)](#13-studio-vector-lighting--shadow-engine)
- [Glow Filter](#14-vector-filters--blend-modes)
- [Gradient Transform Tool (`◑`)](#7-advanced-brushes-patterns--gradients)
- [Graphic Symbols](#11-symbols-instances--the-library)
- [Guide Layers (Rotoscoping)](#9-the-7-layer-kinds--layer-hierarchy)

### H
- [Hand Tool (`H`) / Spacebar Pan](#4-canvas-navigation--unbounded-zoom)
- [HUD Telemetry Display](#4-canvas-navigation--unbounded-zoom)

### I
- [Importing .fla, .xfl, .swf, .pdf, .ai](#18-import--export-pipelines)
- [Ink Bottle Tool (`S`)](#6-comprehensive-23-tool-catalogue)
- [Inverse Mask Layers (Exclusive)](#9-the-7-layer-kinds--layer-hierarchy)

### J
- [JavaScript Automation (JSFL API)](#19-javascript-automation-jsfl-api)
- [Joint Constraints & Pinning](#12-rigging-fabrik-inverse-kinematics--warping)

### K
- [Keyframes (`F6`)](#8-timeline-mastery--frame-by-frame-animation)
- [Keyframed Lights](#13-studio-vector-lighting--shadow-engine)

### L
- [Lamp (Point Light)](#13-studio-vector-lighting--shadow-engine)
- [Lasso Tool (`L`)](#6-comprehensive-23-tool-catalogue)
- [Layer Depth (2.5D Parallax)](#9-the-7-layer-kinds--layer-hierarchy)
- [Layer Parenting](#9-the-7-layer-kinds--layer-hierarchy)
- [Library & Live Thumbnails](#11-symbols-instances--the-library)
- [Light Gizmos (`Ctrl + Shift + L`)](#13-studio-vector-lighting--shadow-engine)
- [Line Tool (`N`)](#6-comprehensive-23-tool-catalogue)
- [Lip Sync Dialog & Visemes](#16-soundtrack-waveforms--automated-lip-sync)
- [Looping Timeline Sections](#8-timeline-mastery--frame-by-frame-animation)

### M
- [Magic Wand Tool (`G`)](#6-comprehensive-23-tool-catalogue)
- [Mask Layers](#9-the-7-layer-kinds--layer-hierarchy)
- [Merge Shape Mode](#5-the-vector-drawing-engine-merge-shapes-vs-object-drawing)
- [Mirror Poses](#12-rigging-fabrik-inverse-kinematics--warping)
- [Motion Editor & Easing](#10-tweens--the-motion-editor)
- [Motion Path Tool (`J`)](#10-tweens--the-motion-editor)
- [Motion Tweens](#10-tweens--the-motion-editor)
- [Mouth Symbols (10 Visemes)](#16-soundtrack-waveforms--automated-lip-sync)
- [MovieClip Symbols](#11-symbols-instances--the-library)

### N
- [Normal Brush](#7-advanced-brushes-patterns--gradients)
- [Normal Layers](#9-the-7-layer-kinds--layer-hierarchy)
- [Nudge Selection (`Arrow Keys`)](#21-complete-keyboard-shortcuts-reference)
- [NVENC GPU Video Encoding](#18-import--export-pipelines)

### O
- [Object Drawing Mode (`J`)](#5-the-vector-drawing-engine-merge-shapes-vs-object-drawing)
- [Onion Skinning (`Alt + Shift + O`)](#8-timeline-mastery--frame-by-frame-animation)
- [Oval Tool (`O`)](#6-comprehensive-23-tool-catalogue)

### P
- [Paint Bucket Tool (`K`)](#6-comprehensive-23-tool-catalogue)
- [Pasteboard Canvas](#3-visual-tour-of-the-workspace)
- [Pattern Brush & Shapes](#7-advanced-brushes-patterns--gradients)
- [Pencil Tool (`Y`)](#6-comprehensive-23-tool-catalogue)
- [Pen Tool (`P`)](#6-comprehensive-23-tool-catalogue)
- [Perform (Automated Walks/Talks)](#17-production-staging-directing--physics-modifiers)
- [PolyStar Tool (`☆`)](#6-comprehensive-23-tool-catalogue)
- [Pose Library & Keying](#12-rigging-fabrik-inverse-kinematics--warping)
- [ProRes 4444 with Alpha](#18-import--export-pipelines)

### Q
- [Quick Start Guide](#2-first-time-setup--launching-the-app)

### R
- [Raster (Soft) Brush](#7-advanced-brushes-patterns--gradients)
- [Recognise Shape (`Modify ▸ Shape`)](#4-canvas-navigation--unbounded-zoom)
- [Rectangle Tool (`R`)](#6-comprehensive-23-tool-catalogue)
- [Reset Workspace](#20-workspace-customization--layouts)
- [Rigging Panel](#12-rigging-fabrik-inverse-kinematics--warping)

### S
- [Selection Tool (`V`)](#6-comprehensive-23-tool-catalogue)
- [Set the Scene](#17-production-staging-directing--physics-modifiers)
- [Shape Tweens](#10-tweens--the-motion-editor)
- [Sky (Ambient Light)](#13-studio-vector-lighting--shadow-engine)
- [Subselection Tool (`A`)](#6-comprehensive-23-tool-catalogue)
- [Sun (Directional Light)](#13-studio-vector-lighting--shadow-engine)
- [Swap Symbol](#11-symbols-instances--the-library)
- [Swatches & Color Palettes](#7-advanced-brushes-patterns--gradients)

### T
- [Tasks Panel (Background Exports)](#18-import--export-pipelines)
- [Text Tool (`T`)](#6-comprehensive-23-tool-catalogue)
- [Themes (Studio Dark / Paper Light)](#20-workspace-customization--layouts)
- [Timeline Spans & Cells](#8-timeline-mastery--frame-by-frame-animation)

### U
- [Unbounded Zoom (2×10¹⁴%)](#4-canvas-navigation--unbounded-zoom)
- [Undo / Redo (`Ctrl + Z` / `Ctrl + Y`)](#21-complete-keyboard-shortcuts-reference)

### V
- [Video Export (MP4, MOV, ProRes)](#18-import--export-pipelines)
- [Video Reference Layer (Rotoscoping)](#18-import--export-pipelines)
- [Visemes (Lip-Sync Phonemes)](#16-soundtrack-waveforms--automated-lip-sync)

### W
- [Wave Brush (Animated Flow)](#7-advanced-brushes-patterns--gradients)
- [Waveform Display](#16-soundtrack-waveforms--automated-lip-sync)
- [Wiggle Physics Modifier](#17-production-staging-directing--physics-modifiers)
- [Workspace Lock (`Ctrl + Alt + L`)](#20-workspace-customization--layouts)

### Z
- [Zoom Tool (`Z`)](#4-canvas-navigation--unbounded-zoom)
- [Zoom Presets (100%, Fit, Frame)](#4-canvas-navigation--unbounded-zoom)

---

*BuzzAnimate Documentation — Spilled Coffee Studios & Contributors.*
