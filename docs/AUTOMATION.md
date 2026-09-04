<div align="center">
  <img src="images/banner.png" alt="BuzzAnimate Banner" width="100%">
</div>

# Where the Time Goes

**An honest audit of what BuzzAnimate already does for you, and what it still
makes you do by hand.**

The point of this program is that the parts of animation which are *arithmetic
rather than drawing* should not cost an animator a day. This document lists
what has been taken off the animator, and — more usefully — what has not.

Everything in Part 1 is shipped and testable today. Everything in Part 2 was
verified as **absent** by reading the source, not assumed.

---

## Part 1 — What is already automatic

Ranked by how much hand labour each one removes from a real shot.

| # | Feature | The labour it replaces | Where |
|---|---|---|---|
| 1 | **Paint Through** (ink & paint) | Colouring every region of every frame by hand. Colouring is half the labour of drawn animation and almost none of the craft | `Control ▸ Paint Through` |
| 2 | **Direct a Story** | Staging, casting, blocking, timing and framing a shot from a brief | `Insert ▸ Scene ▸ Direct a Story…` |
| 3 | **Set the Scene** | Ground, backdrop, a self-consistent light rig, and a cast standing at plausible sizes | `Insert ▸ Scene ▸ Set the Scene…` |
| 4 | **Perform** (Walk / Run / Talk / Idle) | Keying a cycle by hand, every time, on every character | `Insert ▸ Scene ▸ Perform…` |
| 5 | **Live Motion** (7 modifiers) | The keyframes that keep a held drawing alive — breath, wind, drift, springs | Filters panel ▸ Live Motion |
| 6 | **Automated lip sync** | Choosing a mouth shape per frame against a dialogue track | `File ▸ Lip Sync…` |
| 7 | **Auto-rigging** | Laying bones along drawn artwork by hand | Rigging panel |
| 8 | **Retarget Performance** | Re-animating the same walk on every member of a cast | `Insert ▸ Scene ▸ Retarget Performance` |
| 9 | **Effect brushes** (15 kinds) | Drawing rain, snow, skylines, treelines, string lights stroke by stroke | Brush ▸ Effect |
| 10 | **On Twos / On Threes** | Halving or thirding the drawings a shot needs, cell by cell | `Control ▸ On Twos` |
| 11 | **Follow-through & wiggle bakes** | Hand-animating secondary motion on hair, tails and coats | `Insert ▸ Scene ▸ Add Follow-Through…` |
| 12 | **Beat detection** | Marking the music on the ruler by ear | `File ▸ Detect Beats` |
| 13 | **Shape recognition, smooth, straighten** | Cleaning up a drawn wobble | `Modify ▸ Shape` |
| 14 | **Export presets & background render queue** | Re-typing encoder settings; waiting on a blocked UI | Tasks panel |
| 15 | **JSFL-style scripting** (52 host calls) | Any repetitive document edit | Actions panel, or `--script` |

**The through-line:** every one of these leaves behind **ordinary layers,
shapes, keyframes and poses**. Nothing generated stays generated. That is what
lets automation compound instead of becoming a wall you have to work around.

---

## Part 2 — Where the time still goes

Ranked by hours saved per finished minute of film, highest first.

### 2.1 A brief cannot become a film without a person clicking

**The gap.** `buzzanimate --script brief.js` runs a script, and the script API
has `document.direct(story)`. But the 52 host calls include **no export, no
scene management, no lighting, no lip sync, and no staging beyond `direct`** —
so a script can build a shot and then has no way to render it, and no way to
build the second shot. `Editor::direct_sequence` already turns a multi-shot
brief into one scene per shot, and `ExportPreset` already carries every encoder
choice; neither is reachable from a script.

**Why it is the biggest gap.** It is the difference between "the program helps
you animate" and "you hand it a brief at midnight and read the mp4 at
breakfast".

**What it needs.** Roughly six host calls — `directSequence`, `setScene`,
`addLight`, `lipSync`, `exportWith(preset)`, `saveAs` — plus a
`--render <preset>` flag on the binary. Everything underneath them exists and
is tested; this is wiring, not new machinery.

---

### 2.2 The director knows who speaks and when, and nothing uses it

**The gap.** `direct` writes `Talk` beats with exact frame ranges per actor. Lip
sync is a completely separate manual path: import a sound, make a mouth symbol,
open the dialog, pick the character, pick the mouth, run it — **once per
character, per shot**. The two systems never meet, and the director's parser has
no notion of a quoted line of dialogue at all.

**What it needs.**
1. Dialogue in the brief — `Ana: "We should go."` — parsed into a per-actor
   line with a duration.
2. A per-actor audio cue, either an imported take or (later) synthesised.
3. `Talk` beats auto-fitted to that line's real length rather than a default,
   and `analyse_visemes` run on it automatically against an auto-created mouth
   symbol.

**Why it is worth it.** Dialogue is what most short films *are*, and today every
second of it is hand-wired. The frame ranges the automation would need are
already computed and thrown away.

**Shipped.** `File ▸ Lip Sync from Captions` reads the caption layer's frame
labels for the cast, matches each speaker to a mouth symbol of that name,
slices the viseme track to each line's own frames and writes it onto that
character's layer — closing the mouth at the end of every line. Two people
talking, each animated only while they are talking.

**What remains of §2.2** is the other half: the *director* still does not write
`Talk` beats fitted to a real line's length, and nothing creates the mouth
symbols or places them on a character. The dialogue-to-mouth path is done; the
dialogue-to-*performance* path is not.

---

### 2.3 The director understands four verbs

**The gap.** `Walk`, `Run`, `Talk`, `Idle`. Every other beat a writer types —
*sits*, *stands up*, *turns*, *points*, *picks it up*, *falls*, *waves*,
*reacts* — lands in `ignored` and is handed back to the writer to rephrase.

**Why this is the cheapest win on the list.** `perform::pose_at` is a readable
table of curves, written to be read and extended. Each new action is one more
arm of that match, and each one directly multiplies how much of a brief lands
without a person. Six more actions — **sit, stand, turn, point, reach, react** —
would cover most of what people actually write in a brief.

**The honest constraint.** These are hand-authored curves, not a solver. Adding
one costs an animator's judgement, not an algorithm. That is a feature, but it
does mean the vocabulary grows deliberately.

---

### 2.4 Scenery is arranged but never populated

**The gap.** `Set the Scene` builds ground, backdrop, lights and cast. The 15
effect brushes that would fill it — pine trees, leafy trees, grass, buildings,
street lamps, string lights, rain, snow, stars, cloud — are **hand-drawn only**.
`buzz-act` never references `effect_brush`.

**What it needs.** Setting-aware scenery in the staging recipe: a `Night`
exterior lays a treeline or a skyline along the horizon and a lamp strip along
the path; a storm lays rain; a daylight exterior lays grass. Plus prose triggers
in the director — *forest*, *city*, *street*, *rain* — matching the existing
`clouds` / `water` mechanism, which is already exactly this pattern and already
works.

**Why it is worth it.** After the director runs, the largest remaining *drawing*
task is the background, and the brushes that would paint it already exist.

---

### 2.5 Nothing does a timing pass

**The gap.** `perform` and `direct` write poses on evenly spaced keys. Nothing
applies easing, nothing puts a generated performance on twos, and nothing
retimes a cut to land on a detected beat. There is a Motion Editor and an ease
model for tweens; generated work never touches them.

**Worse for the camera.** `CameraKey` has **no easing field at all**, and
`CameraTrack::state_at` interpolates position and rotation on a linear `t`
(`camera_track.rs:377`). Zoom is at least geometric, so a push-in does not
visibly accelerate — but every pan the director plans starts and stops dead.

**What it needs.** An "ease the generated keys" pass (slow in and out of every
hold), an option to expose a performance on twos as it is written, and — once
`Detect Beats` has run — snapping the director's cuts to the nearest beat.

**Why it matters.** Linear keys are the most reliable tell that a shot was
generated. This is a small amount of arithmetic standing between "obviously
automatic" and "looks animated".

---

### 2.6 Turnarounds exist and the director cannot reach them

**The gap.** The director *does* turn people the right way — `ActorState::face`
mirrors the placement about the character's own feet, so somebody walking left
faces left. But a mirror is not a turnaround. A drawn back, profile or
three-quarter view is chosen from `object.spatial.rotation_y`
(`buzz-render/src/document.rs:2310`), and **nothing in `buzz-act` ever sets
`rotation_y`** — so a character with a full turnaround installed by hand will
still be shown front-on, mirrored, for every frame the director writes.

**What it needs.** `Move::Exit` / `Enter` / `Toward` already know the direction
of travel; setting a yaw instead of (or as well as) a mirror is the whole
change, and `Turnaround::view_at` then picks the drawing itself.

**Half of this has since shipped from the other end.** The `Turn` modifier
turns a grouped face from a yaw with no second drawing at all — so a director
that set a yaw would now get a real turn even on a cast that has no turnaround
drawn. The director still does not set one.

---

### 2.7 A crowd is placed one person at a time

**The gap.** `Add Person` adds one. The modifiers are already phase-seeded per
object precisely so that a crowd does not breathe or sway in unison — the hard
part of a crowd is solved, and there is no command that makes one.

**What it needs.** "Add N extras" scattered in depth with size falloff, each
handed a Breathe. Everything it would call already exists.

---

### 2.8 Export cannot be reached without a human

**Correction to a natural assumption:** export is *not* per-scene. `Reel` lays
every scene end to end and `export_service` walks all of them, carrying each
scene's sound cues to the right film frame (`export_service.rs:403`). A
multi-scene document already renders as one continuous film with a correct mix.

**What is actually missing** is smaller: no way to export at all without the
UI, no "render each scene as its own file", and no queue you can fill and walk
away from. All three fall out of 2.1.

---

## Part 3 — The animator's view: a narrated story channel

Parts 1 and 2 audit the program. This part asks a narrower and more useful
question: **you make narrated animated stories for YouTube, weekly. Where does
your week actually go, and what does this tool do about it?**

That genre has a particular shape, and it is not the shape of a feature film:

- **The narration comes first.** You record the voice-over, and then everything
  — every shot length, every cut, every mouth — is fitted to audio that already
  exists and cannot move.
- **The animation is limited on purpose.** Puppets, held poses, camera moves
  over near-stills. Nobody is animating a run cycle frame by frame at that
  cadence, and the audience does not expect it. What sells it is that the held
  poses are not *dead*.
- **Reuse is the whole economy.** The same cast, the same rooms, the same title
  card, week after week. An episode that starts from a blank document has
  already lost.

### 3.1 What the program already does for that week

| The job | What does it today |
|---|---|
| Mouths against the narration | **Lip Sync** — visemes analysed from the track, a keyframe per mouth shape |
| Held poses that are not dead | **Live Motion** — Breathe, Sway, Drift running for free on every held drawing |
| Blocking the script you already wrote | **Direct a Story** — the script is the brief; you get staging, casting, blocking and cuts |
| Cast and sets surviving between episodes | **Assets panel** — a library on disk (`%APPDATA%/BuzzAnimate/assets`), outside any one document |
| An episode skeleton | **Save as Template / New from Template** |
| Depth on a near-still shot | **Layer Depth** parallax + a keyable **camera** with pitch and yaw |
| One walk, whole cast | **Retarget Performance**; **Swap Symbol** to recast between episodes |
| A cycle that does not need keying to length | **Loop region** on the timeline |
| Halving the drawings | **On Twos / On Threes** |
| Delivery | Presets at 1080p, square, and **1080×1920 for Shorts**; background render queue |
| Music under it | Multiple sound layers mix down correctly through the reel, with per-cue volume |

That is a genuinely strong base for limited animation. The gaps below are what
is left.

### 3.2 The seven things that still cost a day each week

Ranked by hours per episode.

**1. The voice-over does not drive the timeline.** ~~Nothing in the program
reads a narration and lays out the film to match.~~ **Shipped** —
`buzz_audio::detect_phrases` finds where the voice speaks and where it breathes
(adaptive threshold, gaps under a quarter second ignored), and
`File ▸ Fit to Narration` stretches the film to cover it and puts a blank
keyframe at the start of every line. Re-running after a re-record keeps what was
drawn to the lines that did not move. What is *still* missing is the transcript
half: no captions in, no per-line dialogue routed to a character (see §2 and §3
below).

**2. Nothing blinks.** ~~After breathing, a blink is the single most-noticed cue
that a drawing is alive, and there is no blink anywhere in the codebase.~~
**Shipped** — `Blink` is a live modifier alongside Breathe: jittered interval,
a lid that falls faster than it lifts, one blink in six a double, seeded per
object. Filters panel ▸ Live Motion ▸ Blink.

**3. No captions, in or out.** ~~There is no SRT import to text layers and no
SRT export.~~ **Shipped** — `buzz_doc::srt` reads and writes SubRip, and
`File ▸ Import/Export Captions` puts the words on a Captions layer keyed to
their own timecodes and takes them back off again.

**Note the direction was wrong when this was written.** "SRT out, then in" is
backwards: the program knows the *timings* and not the *words*, so an export
built from what it had would have produced perfectly-timed empty cues. Import
is what carries information the document did not already have — and it is what
makes §2 possible, because a cue that says `Ana:` is dialogue attached to a
name.

**4. Camera moves are manual and interpolate linearly.** **Shipped** — every
`CameraKey` now carries an `Easing`, applied in `state_at`, and `Camera ▸ Move`
writes six named moves (*push in*, *pull out*, *pan left/right*, *reveal*,
*drift*) from the playhead to the end of the scene, already eased. The drift is
left linear on purpose. Note this only fixes the camera half of 2.5; generated
*poses* are still on evenly spaced linear keys.

**5. No text animation.** Titles, chapter cards, the emphasised word that pops
on screen — `buzz-text` outlines and measures glyphs and stops there. Every
title is hand-keyed, every episode, from scratch.

**6. Episode scaffolding is a flat template.** `New from Template` gives you a
document. What this genre wants is "new episode: keep the cast, the sets and the
title card, clear the timeline and the narration" — which is a different
operation, and the Assets panel is most of the machinery for it.

**7. No fades or ducking.** `SoundCue` carries a single scalar `volume` and no
envelope. Music under narration has to be pre-ducked in another program and
re-imported whenever a line changes.

### 3.3 If you only built four things

For this animator, in this order:

1. ~~**Auto-blink**~~ — **shipped**.
2. ~~**Narration-driven timing**~~ — **shipped** as `Fit to Narration`.
3. ~~**Named camera moves, eased**~~ — **shipped**.
4. ~~**SRT out** (then in)~~ — **shipped**, and in the right order: import
   first, because that is the direction carrying information the document did
   not have.

The queue is now led by **the director's four verbs** (§2.3) — the cheapest
remaining win by a wide margin, since `perform::pose_at` is a readable table of
curves written to be extended, and every beat a writer types that is not walk,
run, talk or wait is currently handed back to them to rephrase.

Also shipped since this audit was written, from the requests side rather than
this list: the **`Turn` modifier** (a face turns from one drawing, §2.6), and
**Trace Bitmap** (a picture becomes editable artwork).

---

## Part 4 — The short answer

**No, it is not all we can do.** What is automated is the *inside* of a shot:
staging it, casting it, blocking it, framing it, keeping it alive, and colouring
it. What is not automated is the **ends** —

- getting a brief *in* without a person driving the dialogs (2.1, 2.3),
- getting the film *out* without a person clicking Export (2.1, 2.8),
- and the two passes that make automated work stop looking automated:
  **dialogue** (2.2) and **timing** (2.5).

Ordered by hours-saved per hour-spent building it, the queue is:

1. **Script/CLI reach to export and staging** (2.1) — unlocks unattended work at all
2. **Dialogue → lip sync through the director** (2.2)
3. **Six more verbs** (2.3) — cheapest, most visible
4. **An easing and cadence pass on generated keys** (2.5)
5. **Scenery from the effect brushes** (2.4)
6. Turnarounds, crowds, batch export (2.6–2.8)

And from the animator's side of the desk (Part 3), what is left is mostly
**scenery** (2.4), **a timing pass on generated poses** (2.5) and the
**script/CLI reach** that would let a brief run unattended (2.1). Auto-blink,
eased named camera moves, narration-driven timing, captions in and out, and
dialogue routed to each character's mouth have all shipped since this audit was
written, along with the head turn and bitmap tracing.

---

*See also: [`SCENES_AND_THE_DIRECTOR.md`](SCENES_AND_THE_DIRECTOR.md) for how
the automated parts work, [`USER_GUIDE.md`](USER_GUIDE.md) §17 for how to drive
them, and `PROGRESS.md` in the repository root for the engineering roadmap.*
