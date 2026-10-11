# BuzzAnimate vs Adobe Animate — where it saves time, where it doesn't, what to build next

Written 2026-10-11 from the source, the docs (`docs/AUTOMATION.md`,
`docs/USER_GUIDE.md`, the plan files) and the Chirailiya Ep1 build. Claims
about Animate are about Animate 2024 (installed on this machine). Where
something has not been measured, it says so.

## 1. The short version

Animate is a **drawing and timing tool**: every shot is staged, keyed,
lip-synced and exported by a person. BuzzAnimate does the same drawing and
timing — the tools are deliberately Animate-shaped, so nothing has to be
relearned — and then **automates the shot itself**: a script and a cast of
rigs become staged, acted, lip-synced, lit scenes, and an AI agent can drive
every menu and panel. The Ep1 build (2026-10-06) turned a screenplay plus one
image per character into 7 animated scenes overnight, with no one at the
keyboard.

Where Animate is still ahead: maturity and stability, HTML5 Canvas / AS3
interactivity, text animation, and the Adobe ecosystem.

## 2. Things BuzzAnimate does that Animate does not do at all

| Feature | What it does | What it replaces in Animate |
|---|---|---|
| **Script → episode** (`buzz_app::episode`, `casting.json`) | A screenplay becomes scenes: sets, cast, entrances, talk/idle acting, blinks, captions and text lip sync, one scene per file plus a report | Days of staging and keying per episode |
| **Direct a Story** (Insert ▸ Scene) | A brief is staged, cast, blocked, timed and framed | Manual staging and blocking of every shot |
| **Set the Scene** | Ground, backdrop, a consistent light rig and a cast at plausible sizes | Building every set by hand |
| **Perform** — walk, run, talk, idle; **Retarget Performance** | Cycles keyed for you, then copied to every member of the cast | Keying each cycle on each character |
| **Live Motion modifiers** — breathe, blink, wind, drift, spring, turn… | A held drawing stays alive with no keys at all, until you bake it | Hundreds of "keep it alive" keyframes |
| **AI agent with full app access** (Assistant; `docs/AI_AGENT_PLAN.md`) | "Make the sky darker and move Raju left on frame 40" is done through the same code the menus use, as one undo step | Nothing comparable |
| **Vector lighting and shadows** — sun, sky, lamp, gloom; hard light | Colours, highlights and shadow direction follow where the light is dragged | Painting light and shadow onto every drawing |
| **Paint Through** | Colours every region of every frame | Bucket-filling frame by frame |
| **Auto-rig + pose library** from one image (DWPose) | Bones laid along drawn artwork; standard poses that tween | Hand-placed bones |
| **Character from an image, Trace Bitmap modes** (Ink / Ink+colour) | A ChatGPT T-pose PNG becomes a layered, rigged vector character | Hours of tracing per character |
| **Cast panel + wardrobe recolour** | Characters are reusable assets; a recolour reaches every part and pose | Editing every symbol's colours |
| **Effect brushes** (15 kinds) | Rain, snow, skylines, treelines, string lights in one stroke | Stroke-by-stroke drawing |
| **Fit to Narration, Captions in/out, Lip Sync from Captions** | The voice-over lays out the timeline; an SRT drives the mouths | Manual timing to a voice track |
| **Sequence: setups, shots, cut on dialogue** | A reusable setup is stamped into shots, cut on who is speaking | Duplicating scenes and re-cutting by hand |
| **Symbol sets** (new) | Pick `Raju_wave` from a dropdown and it starts at the playhead, keyed | Swap + manual keyframes, or the Frame Picker on one symbol |
| **Drag frames across layers** (new) | Move a frame range sideways or onto other layers; the old place becomes a blank keyframe | Animate does this too — parity |
| **Background render queue, NVENC** | MP4/MOV on the GPU with sound muxed, while you keep working | Media Encoder round trips |
| **Unbounded zoom** (f64 geometry, scale-invariant rendering) | Draw a fly's eye inside a city shot | Animate's zoom cap |
| **Headless CLI** (`--brief`, `--render`, `--script`) | Briefs and renders without opening the window | — |
| **Local API + estate bridge** (port 8840, `src/api.rs`, `src/estate.rs`) | Studio script → episode → render → BuzzEdit project, each app able to call the others; Whisper transcript → captions + lip sync | Export, copy, import, re-time by hand |
| **Text as vector outlines, Devanagari shaped** (`buzz-text`) | Hindi/English titles that scale and export like artwork | Animate's text; Devanagari needs care |

## 3. How it saves time — where the hours go in one episode

Ordered by hours removed per episode for a narrated, dialogue-driven series
like Chirailiya (qualitative; not timed against a stopwatch):

1. **Lip sync** — text or caption driven, routed to each character's own
   mouth chart (Raju has 19 mouths). In Animate: Auto Lip-Sync helps on one
   symbol, but routing every line to the right character, in every shot, is
   manual.
2. **Staging and acting** — entrances, talk/idle, blinks and breathing come
   from the script. In Animate every one of those is keyframes.
3. **Colouring and lighting** — Paint Through + lights instead of filling and
   shading every frame.
4. **Characters** — from image to rig in one pass, reusable across episodes
   through the Cast panel.
5. **Rendering** — GPU encoding on a queue instead of a blocked app or Media
   Encoder.

## 4. Quality — what is actually better, and what is only "as good"

- **Better, measurably:** rendering is scale-invariant (ink coverage constant
  across 13 decades of zoom, README Phase 0); geometry is f64, so precision
  holds to ~1e12% zoom. Vector lighting and filters are rendered, not painted.
- **Parity, measured:** FLA import is checked pixel-for-pixel against Animate's
  own MP4 at high zoom before any speed cache is turned on
  (memory: quality over speed on import).
- **Not better yet:** tracing quality (spill outside outlines is an open plan,
  `docs/TRACE_SPILL_AND_OUTPAINT_PLAN.md`); generated poses use evenly spaced
  linear keys (no timing pass — `AUTOMATION.md` §2.5), which can look
  mechanical; lit rigs lag on heavy scenes (ep1 lag notes).

## 5. Where Animate is still ahead — honestly

- HTML5 Canvas / WebGL / AS3 export and interactivity (buttons, code).
- Text: the Text tool works (Devanagari shaped), but nothing animates text yet (titles, popping words).
- Audio: no fades or ducking on a sound cue (BuzzEdit can duck, see §6).
- Maturity: years of production use, tutorials and stability; BuzzAnimate
  still has open bug plans and a 30-minute menu sweep test.
- Ecosystem: After Effects, Character Animator, Media Encoder, cloud libraries.

## 6. How to make production very fast — ranked by hours saved per hour built

1. **Wire the estate together** (E1–E6 built and unit-tested 2026-10-11;
   E7, the real three-app run, not done: `docs/ECOSYSTEM_BRIDGE_PLAN.md`). The Studio writes the script, BuzzAnimate
   animates it, and BuzzEdit finishes it (captions, music with ducking, colour,
   thumbnails, Shorts), each app able to call the others. This removes every
   copy-a-file-between-apps step.
2. **Real voices → audio lip sync.** MidnightBuzz already runs XTTS/RVC: give
   each cast member a voice, generate every line, and drive the mouths from
   the audio instead of from text. The Whisper half exists (Commands ▸
   Transcribe with BuzzEdit → captions + lip-sync keys); what's missing is
   generating the voices per cast member. Today Ep1 has no audio at all.
3. **A timing pass on generated keys** (`AUTOMATION.md` §2.5) — ease in/out,
   holds and anticipation on director keys. The single biggest jump in how
   "automated" the result looks.
4. **Gesture sets per character, chosen by the director.** Symbol sets now
   give each character `idle / wave / point / sit…` siblings; let the episode
   builder pick one per line from the dialogue's verb and emotion.
5. **Scenery from ComfyUI → Trace.** Generate a background in BuzzEdit's
   ComfyUI, trace it into layered vector parts, file it as an asset set.
6. **Text animation** — titles, chapter cards, emphasised words.
7. **Parallel scene renders** — a headless `--serve` instance per scene, or a
   render farm over the queue, so 7 scenes render at once on the 28 threads.
8. **Fix the known drags**: lit-rig lag, trace spill, the slow test sweep —
   every bug that makes you wait is time taken from production.
