# Scenes, the Director, and Motion That Runs Itself

**How a shot builds itself in BuzzAnimate** — what *Set the Scene* actually
arranges, what *Direct a Story* reads out of your prose, and which parts of the
picture keep moving after you stop touching them.

Everything below produces **ordinary layers, shapes, lights and keyframes**.
There is no "generated scene" object, nothing re-runs behind your back, and one
Ctrl+Z takes the whole thing away. It is a starting point in the same sense a
template is, and the first thing you do to it is draw over it.

---

## Part 1 — Set the Scene

**Scene ▸ Set the Scene…**

### What it is actually doing

Not handing you clip art. The thing that costs an afternoon before any
animation exists is the **arrangement**: a horizon at a believable height, a
ground plane the characters stand *on* rather than in front of, a key light and
a fill that agree with each other, and a cast placed at plausible distances and
sizes.

None of that is drawing. All of it is arithmetic, and all of it has one
obviously right answer that everybody types out by hand.

### The horizon is the one number everything follows

Set the **Horizon** slider and the rest falls out of it:

| Follows from the horizon | How |
|---|---|
| Where sky meets ground | The line itself |
| Where the cast's feet go | On the ground, staggered back from it |
| How large each figure is drawn | Smaller the further back they stand |
| How high a practical lamp hangs | Above the heads, measured from the line |
| Where the water's near bank sits | A third of the way down the ground |

Getting characters to stand *on* something rather than float above it is most
of what separates a set scene from a pile of shapes.

### The cast

Up to six. They are spread across the stage and **staggered in depth** — each
one further back is drawn a little smaller and a little higher up, which is the
whole of perspective on a flat stage and is what stops two characters reading as
cut-outs at the same distance.

They also **face each other**: the first looks right, the rest look back at it.
Two people in a shot who both face the camera are two people in a photograph.

Each gets **its own layer**, so their performances can be timed independently.

And **everybody breathes.** A set scene is a held pose until somebody animates
it, and a held pose that does not move is a picture of a character rather than a
character standing there. Each of the cast gets a live *Breathe* (see Part 3) at
a slightly different rate, so a crowd of six does not inhale together. It costs
no keyframes, survives a re-time, and a walk written over it walks *and*
breathes rather than choosing.

### The settings

| Setting | What you get |
|---|---|
| **Daylight** | A high sun, a blue sky, short hard shadows |
| **Sunset** | A low raking sun, a warm sky, shadows running long |
| **Night** | A dark sky and one warm practical lamp doing the work |
| **Interior** | A wall, a floor, and a practical lamp — no sky at all |
| **Storm** | A near-black sky that **strikes**, with cloud running over it |

### The light rig

Ticking **Light it** builds a three-point rig, in the two dimensions that mean
anything on flat artwork:

- **A key** with a direction — a sun outdoors, a practical lamp indoors and at
  night, and the lightning itself in a storm.
- **A fill** (a sky light) taking the *other* end of the picture's range: cool
  against a warm key, dim at night, barely there in a storm.
- **A rim** on the key. This is the point of lighting a set scene: everything
  else lighting does happens *inside* the silhouette, and the rim is the one
  thing that puts an edge of the key's colour around a figure and separates
  them from the ground behind.

There is deliberately no fourth light for each new idea. A spot and an area
light mean nothing on flat artwork; adding one would be adding a lamp with extra
numbers.

### Cloud in the sky

Tick **Cloud in the sky** and you get five cumulus above the horizon, each
drifting across and looping, **each at its own speed and height**.

A cloud here is three to five overlapping discs in one path with a flat base —
one shape, so it is one fill to draw and one object to move. The higher ones are
smaller, paler and slower, which is parallax, is free, and is most of what makes
a flat sky read as deep. Each also **billows** slowly as it goes; without that a
cloud is a cut-out sliding across a card, which is exactly what it looks like.

A **Storm** always gets cloud whether you ticked it or not. There is no such
thing as lightning out of a clear sky.

An **Interior** never gets cloud however hard it is asked. A cloud on a wall is
a stain.

### Water in front

Tick **Water in front** and you get a river across the near ground.

The whole trick is that **nothing in it is drawn moving.** It is a flat band of
water colour with highlight streaks lying on it, and those streaks slide across
at nine different speeds — near ones fast and long, far ones slow and short. The
eye reads the relative motion between them as a surface flowing, and it does it
convincingly enough that this is how water is done in cel animation and in
nearly every game background ever shipped.

Each streak also bobs a little as it goes. A streak that only slides is a
scratch on the glass; the bob is the difference between "this is moving" and
"this is liquid".

### None of it costs a keyframe

Read that again, because it is the part that matters: **the sky and the water
move without a single key on the timeline.** They are live modifiers (see Part
3), so:

- Re-time the shot and the sky is still crossing at the right speed.
- Export two frames or twenty thousand; the setting up is identical.
- The loop is seamless, because it is a wrap rather than a pair of keys.

---

## Part 2 — Direct a Story

**Scene ▸ Direct a Story…**

### What you type

```
Night. Ana walks in from the left.
Ana talks to Ben for 4 seconds. Ben listens.

A storm. By the river.
Ben walks off right.
```

### What "understanding" means here, honestly

**There is no language model in this and nothing is learned.** The parser is a
keyword grammar. It knows:

- the **setting words** — *night, midnight, moonlit* → Night; *sunset, dusk,
  evening, golden hour* → Sunset; *inside, room, kitchen, office* → Interior;
  *day, morning, noon, sunny* → Daylight; and **storm, thunder, lightning,
  tempest, downpour** → Storm, which is checked first because a "stormy night"
  is a storm and matching *night* first would take the lightning out of it.
- a few dozen **verbs** sorted into four actions: walk, run, talk, wait.
- the **direction words** — "in from the left", "off right", "to Ben",
  "across".
- **durations** — "for 3 seconds".
- **scenery words** — *river, stream, lake, sea, shore, water* put water in the
  shot; *cloud, overcast* put cloud in the sky.
- that a **capitalised word it has no other explanation for** is somebody's
  name, and that "a man" casts an unnamed one who is "the man" thereafter.

That covers the way people actually write a brief — subject, verb, colour — and
it **fails loudly rather than cleverly**: every sentence it could not read is
listed in the report afterwards, because a director who silently skips a line of
the script is worse than one who asks.

### The schedule

Sentences run in story order. Each actor has a clock, and a sentence starts when
everyone in it is free. Write **"meanwhile"** and it starts alongside the
previous one instead.

Two rules stop an automatic scene giving itself away:

- **Someone spoken *to* listens** — an idle over the same frames. A character
  frozen solid while being talked at is the most obvious tell there is.
- **Everyone left standing idles quietly to the end of the shot.** Breathing,
  not stopped.

### The camera

A staged, performed scene with a locked-off camera is a stage play seen from row
H. It is not wrong, and it is not a film. So the director frames it, on three
rules:

- **Somebody talking is who the shot is about.** The camera comes in on them,
  centred and closer. Between two speakers it **cuts** — keys on adjacent
  frames — because a camera that drifts across the room during a conversation is
  a camera nobody asked for.
- **Somebody walking is followed.** Keys at both ends of the beat, so the camera
  pans with them. The movement is the point of that beat.
- **Nothing else moves the camera.** An idle holds whatever framing it
  inherited, and a shot with nobody talking or walking gets one wide key and
  stays there.

It **never frames tighter than the figure**. The zoom comes from the actor's own
height, so a close shot is close *on them* rather than by some number of pixels,
and it is bounded so nobody's head leaves the frame. Guessing a framing that
decapitates somebody is worse than not framing at all.

### Shots

**A blank line starts a new shot.** So does a setting on a line of its own:

```
Ana waits by the river.

Night.
Ben walks in from the left.
```

…is two shots — a scene each, played in order, each named after its own words.
A later shot that does not restate the setting keeps the one in force.

---

## Part 3 — Motion that runs itself

Everything in Parts 1 and 2 that keeps moving does so through **live
modifiers**: rules evaluated when a frame is drawn rather than baked into keys.

**Filters panel ▸ Live Motion ▸ +**

| Modifier | What it does | Reach for it when |
|---|---|---|
| **Breathe** | The chest rises and falls about the drawing's own feet | Any character on a held pose |
| **Blink** | The lid falls and lifts every few seconds | The eye artwork on any character |
| **Turn** | Carries a face's features round a cylinder so it turns | A grouped head, with no other view drawn |
| **Sway** | The drawing bends downwind from its base, in gusts | Trees, grass, banners, hanging signs |
| **Drift** | A steady move that loops | Clouds, water, a street behind a window |
| **Wiggle** | A deterministic wander | Idle sway, a breeze, a handheld shake |
| **Spring** | Damped follow-through on a bone chain | Hair, tails, coats |
| **Look At** | Turns to face a point | Eyes and heads that track |
| **Squash & Stretch** | Stretches along the direction of motion | Selling weight and speed |

### Why live and not baked

A modifier is **deterministic in (object, frame)** and is not written to
keyframes. That means:

- **Re-time the animation and it re-follows.** Nothing to re-bake.
- **Its cost does not grow with the length of the film.** One setting per
  object, whether the shot is two seconds or two minutes.
- **The same maths as the bakers.** The spring and the wiggle here are the *same*
  solvers `Scene ▸ Add Follow-Through` and `Add Wiggle` use when you ask them to
  bake. "Live" and "bake to keyframes" are two deliveries of one calculation.

### Breathe

A held pose in animation is never *still*. A drawing that does not move between
two keys reads as a picture of a character rather than as a character standing
there — and the cheapest thing that fixes it, the thing every animator draws by
hand on a hold, is a breath.

- **Rate** is in breaths per minute: 14 at rest, 30 and up after running.
- **Depth** scales it; 1.0 is a comfortable resting breath.

It is anchored at the **bottom of the drawing**, so the feet stay on the ground
and the motion goes into the chest. It is about two per cent of scale, and
nobody ever notices it consciously — they notice its absence immediately. The
phase is seeded from the object, so a crowd does not breathe in unison, which is
the one thing that would make it visible.

The curve is not a sine. A breath fills quickly and empties slowly; a pure sine
reads as a machine.

### Blink

The other half of the job Breathe does, and on a face the larger half. Nobody
consciously sees a blink either — but a character who holds a stare for eight
seconds while talking is unnerving in a way nobody can name, and that is the
trap a puppet built for limited animation falls into: its eyes are one drawing
that nothing ever touches.

Applied to the **eye artwork**, not to the character — like Sway on a tree, the
lid falls on whatever drawing it is given.

- **Rate** is in blinks per minute: 12 at rest, and much past 20 reads as nerves.
- **Duration** is how long one blink takes; 0.16s is a real one, four frames at 24.

The lid **falls faster than it lifts** — about a third of the blink is the close
and two thirds the open — which is what a real eyelid does and what separates a
blink from a pulse. The bottom edge is held so the eye closes downward; pinching
it shut about the middle reads as a wince.

The interval is **jittered inside a slot** rather than fixed, because a blink on
a metronome is worse than no blink at all: the eye becomes a ticking clock in
the corner of the shot. And roughly one in six comes as a **double**, because a
blink that is always single is a regularity an audience reads as mechanical
without being able to say why.

Like everything else here it is deterministic in (object, frame), so a re-timed
shot blinks identically — and seeded per object, so a cast never blinks together.

### Sway

A **shear**, not a rotation: the bottom stays planted and the lean grows with
height, which is what a trunk does. A rotated tree pivots its roots out of the
ground.

- **Lean** is how far the top goes at a full gust, as a share of the drawing's
  own height — 0.1 is a stiff pine, 0.35 a willow.
- **Hz** is the gust rate; around 0.2 is a breeze.

The gust is **biased downwind** rather than centred, because wind is: it lulls
back towards upright and gusts one way, instead of waving the tree evenly to
both sides like a metronome. It is seeded from the object, so a row of trees
planted from the same drawing does not sway as one object — which is exactly
what gives a painted background away.

The drawing also shortens very slightly as it bends, because the top of a
leaning trunk really is nearer the ground. Without that the crown swings along a
visibly wrong arc and the tree looks rubbery rather than woody.

### Drift

- **dx / dy** — document units per second.
- **wrap** — how far it travels before it starts again. `0` never loops.
- **start** — how far into that loop it already is, `0..1`.

The **start** is the field that makes a *field* of drifting things possible.
Without it, five clouds on one loop are five clouds in a queue crossing the sky
in formation. (Offsetting where each one is *placed* does not fix it — the wrap
then sends the ones placed further along off the far side and holds them there
for most of the loop. The phase has to be inside the wrap.)

Set the wrap **wider than the stage plus the drawing**, or the thing pops into
existence in mid-air on the far side.

---

## Part 4 — Lights that will not hold still

Two presets, both in **Insert ▸ Light** and in the Lights panel. Neither costs a
keyframe; both move every frame on their own. **Scrub the timeline to see
them.**

### 🔥 Fire

A lamp with a hearth colour, a hard gutter and a tighter reach. It is a preset
rather than a fourth kind of light because everything a fire is, a lamp already
has — a place on the stage, a falloff, a pool in the air. The only things that
make it fire are the colour and the fact that it will not hold still.

The gutter moves:

- **the brightness**, two rates at once, so there is no period for the eye to
  find;
- **the colour**, redder as it drops, because the dim part of a fire is the
  ember colour and not a dimmer version of the flame;
- **the pool**, but less — a flame's reach is steadier than its brightness;
- **the shadow it throws.** Half of why a hearth reads as a hearth is that the
  shadows on the wall behind you jump with the flame. A steady shadow under a
  moving light says "filter" as loudly as a flat tint does.

The shadow's *shape* deliberately does not move. That follows the light's
position, and moving it would rebuild every boolean in the shot on every frame.
What moves is how dark it is, which is free.

### ⚡ Storm

A dark sky that strikes. Every few seconds:

1. a **stepped leader** — brief, faint, gone before the eye has settled;
2. a **beat of nothing**;
3. the **return stroke** — six or seven times the light, arriving in two frames
   and dying away over a third of a second, flickering on the way down as
   further strokes follow the same channel.

Then dark again, for a random count, and again.

**Turn the light itself right down first** — the ⚡ preset does this for you. A
flash only reads against the dark, and a storm set over a daylit stage is a
light going slightly brighter now and then.

Put it on a **sky**: a sheet of lightning has no direction, so it lights the
whole stage at once, which is what makes the frame go white rather than one side
of every figure. On a *sun* it flashes with a direction and the shading and
shadows snap with it — worth having for a close shot, too strong for a wide one.

Higher on the **Lightning** slider is both more often and harder: a tenth is a
storm on the horizon, full is overhead.

It is deterministic noise seeded from the light, so it is the same storm on
every machine and in every export, never twice the same strike, and there are no
keys to re-time. It composes with the flicker, so a torch carried through a
storm gutters and is lit by the sky at the same time.

### Softening what a light does to an edge

Two controls on every light, next to each other:

- **Softness** — how *wide* the shaded edge is. Narrow reads as a hard light.
- **Edge highlight** — how *hard* the lit side lands. Full is a wet, polished
  sheen; a drawing is usually matte, so the default sits near two thirds. Zero
  leaves the lit side its own colour, with only the shaded side to model it.

The highlight band also **falls off around the form** rather than stopping flat:
it is brightest where the shape faces the light and dies away around the curve,
which is what a real highlight does. Filled flat, the band was a stripe of even
brightness ending on a hard line — and on a face or a limb that reads as a
whiter drawing pasted over one side of the artwork rather than as light falling
on it.

---

## Part 5 — The ground under all of it

**Colour panel ▸ Texture** now carries twenty-one seamless procedural textures
in three families:

- **Surfaces** — Paper, Canvas, Noise, Checker, Dots, Stripes, Bricks, Wood,
  Hatch
- **Grass** — Lawn, Blades, Meadow, Straw, Moss, Clover
- **Ground** — Soil, Gravel, Sand, Cracked, Pebbles, Mud

Each takes **your** two colours — the fill and the stroke — so a texture adopts
the palette you are working in rather than dictating one. Each tiles with no
seam at any size, and each is stored as a handful of numbers rather than an
embedded image, so a hillside covered in grass costs nothing in the file.

They are **re-editable after they are applied**: change a colour, coarsen the
pattern, push the contrast, and every shape wearing that texture changes at
once.

---

## Cheat sheet

| I want… | Do this |
|---|---|
| A shot arranged and lit, now | Scene ▸ Set the Scene… |
| The same, from a written brief | Scene ▸ Direct a Story… |
| A sky that moves | Set the Scene ▸ **Cloud in the sky** |
| A river that runs | Set the Scene ▸ **Water in front** |
| Lightning | Setting **Storm**, or Insert ▸ Light ▸ ⚡ Storm |
| A hearth | Insert ▸ Light ▸ 🔥 Fire |
| A character who is alive on a hold | Live Motion ▸ **Breathe** (the staged cast already has it) |
| Trees in wind | Live Motion ▸ **Sway** |
| Anything scrolling past | Live Motion ▸ **Drift** |
| A softer lit edge | Lights ▸ **Edge highlight**, down |
| Grass or dirt | Colour ▸ Texture ▸ Grass / Ground |
