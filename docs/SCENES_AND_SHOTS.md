# Scenes, Setups and Shots — start here

How to split a film into parts in BuzzAnimate, and which of the three ways to
do it suits the film you are making. This page is the way in; the deeper
references are linked at the end.

---

## The words, once

| Word | What it is |
|---|---|
| **Scene** | One timeline with its own layers, like a scene in Animate. A document holds as many as you like, and the film plays them one after another. |
| **Normal scene** | An ordinary scene. If you never use setups or shots, every scene you have is this kind. |
| **Setup** | A scene you build once and *reuse*: background, props, lights, camera framing, characters standing in place. It is a source to copy from, so it is **never in the exported film**. |
| **Shot** | One cut of a dialogue film: a copy of a setup plus a stretch of the **master audio**. Frame 1 of a shot is where its slice of audio starts. |
| **Take** | Another version of the same shot (the same stretch of audio). Only the take you choose is in the film; the others are kept. |
| **Master audio** | The one dialogue track the whole sequence is cut from. |
| **Sequence strip** | The panel in the bottom dock (a tab beside the Timeline) that shows the master audio's waveform with your shots as blocks on top of it. *Window ▸ Sequence* if you can't see it. |

---

## Where the scene controls are

**The scene name above the stage.** The left end of the breadcrumb above the
stage shows the scene you're in. Click it to get the scene menu:

- **the list of scenes**: click one to switch to it. Setups show as
  `◆ Name (setup)` and shots as `Shot 2 · Name of its setup`, so you can tell
  them apart at a glance;
- **Add Scene**: a new, empty scene;
- **Duplicate Scene**: a copy of this one, with its set, cast, lights and
  animation, so the next beat starts from where this one is;
- **Rename Scene…**: the name turns into a text box. Enter keeps the new
  name and Escape cancels;
- **Move Up / Move Down**: change the running order. The film plays scenes in
  this order;
- **Delete Scene**: removes the scene you're in. ⚠ **You can't undo this, and
  it doesn't ask you to confirm.** Duplicate a scene first if you might want
  it back. You can't delete the last scene.

Inside a symbol, the scene name becomes a link that takes you back to the
scene's main timeline. The small **⏷** beside it opens the same scene menu.

**The Scene menu** at the top also has *Add Scene* and *Duplicate Scene*,
plus the Director's items (*Set the Scene…*, *Direct a Story…*,
*Scenery…*). Those three open the **Story panel**.

---

## Which way should I work?

| Your film is… | Use | Section |
|---|---|---|
| A few separate moments: a title, a walk, an ending | Plain **scenes** | [Way 1](#way-1--plain-scenes) |
| A conversation, where the same camera angles keep coming back and the voice track decides the cuts | **Setups + shots** | [Way 2](#way-2--setups-and-shots-a-dialogue-film) |
| A story you'd rather type than block by hand | **Direct a Story** | [Way 3](#way-3--let-the-director-make-the-scenes) |

You can mix them. A title as a Normal scene, then a dialogue sequence, then an
ending as a Normal scene plays in exactly that order.

---

## Way 1 — Plain scenes

1. Animate the first scene as usual.
2. Click the scene name above the stage and choose **Add Scene** (or
   **Duplicate Scene** to keep the set and characters).
3. **Rename Scene…** it to something you'll recognise, e.g. "02 Walk to the
   river".
4. Repeat. Use **Move Up / Move Down** to fix the order.
5. **Control ▸ Test Movie** plays the whole film. **File ▸ Export ▸ Export
   Video…** exports every scene in order.

That's all there is to it. None of the setup and shot rules below apply to
Normal scenes.

---

## Way 2 — Setups and shots (a dialogue film)

**Worked example:** a 40-second conversation between Ana and Ben, with three
camera angles: Ana close-up, Ben close-up, and a wide shot of the village.

### 1. Bring in the voice track

**Sequence ▸ Set Master Audio…** and pick the dialogue file (WAV, MP3, OGG, FLAC, M4A or AAC). Its
waveform appears across the Sequence strip.

> Already have the dialogue on a sound layer? **Sequence ▸ Use Existing Sound
> Layer ▸ ‹layer›** uses that layer instead. This item only appears on a
> Normal scene whose layer has its sound on frame 1.

### 2. Build each camera angle once, as a setup

For each angle:

1. **Add Scene**, and rename it (e.g. "Ana CU").
2. Arrange it: background, props, lights, Ana placed and framed. *Scene ▸ Set
   the Scene…* can lay the ground, backdrop, lights and cast for you.
3. **Sequence ▸ Make Setup from This Scene**.

You now have three setups: "Ana CU", "Ben CU" and "Village wide". In the scene
list they show with a ◆.

### 3. Cut the audio into shots

Pick one of these:

- **Automatic, one shot per spoken line:** **Sequence ▸ Cut on Dialogue ▸
  ‹a setup›**. The setup you pick is the default for every line. If you
  imported a subtitle file (*Commands ▸ Import Captions (.srt)…*) whose captions name
  the speakers, a line spoken by "Ana" goes to the setup with "Ana" in its
  name. It only fills audio that no shot covers yet, so running it again
  never re-cuts your work.
- **By hand:** **Sequence ▸ New Shot from Setup ▸ ‹a setup›** makes the first
  shot (from the playhead to the end of the audio). Then, with that shot
  open, put the playhead where the next cut should be and choose **Shot ▸
  Split at Playhead**, or press **S** with the pointer over the Sequence strip.

### 4. Give each shot the right angle

Right-click a shot's block in the Sequence strip ▸ **Try Another Setup ▸
‹setup›**. This makes a new take of that block's stretch of audio, built from
the setup you picked. The old take is kept, not thrown away.

### 5. Animate each shot

**Double-click** a shot block to open it. The Timeline counts from that shot's
frame 1, which is where its audio starts. Animate it like any scene. Run
lip-sync here (*Commands ▸ Lip Sync…*) and it only reads this shot's slice of
the dialogue.

### 6. Play and export

**Control ▸ Test Movie** or **File ▸ Export ▸ Export Video…**. The shots play in order
with the dialogue running straight across the cuts. Setups are left out.

### Things you can rely on

- **A shot never changes unless you edit that shot.** Editing a setup only
  affects shots you make from it *afterwards*. To pull a layer you added to
  the setup later into an existing shot, use **Shot ▸ Add Missing Setup
  Layers** (it only adds, never replaces).
- **Moving a cut is safe.** Drag the handle between two blocks to move the
  cut. Animation past a shot's new end is kept (greyed out) and comes back if
  you lengthen the shot again.
- **Takes are free.** **Shot ▸ Duplicate Shot** makes another take of the same
  audio. When a shot has more than one take, a **Take** switcher appears on
  its block; pick which one is in the film.
- **Characters stay consistent.** A character's *drawing* is its Cast symbol,
  shared by every shot, so a costume fix shows up everywhere. Their
  *animation* belongs to each shot.

### Cutting away and back

Village → Ana → village again? Open the second village shot and choose
**Shot ▸ Continue from Shot ▸ ‹the earlier village shot›** (or right-click
its block). Its frame 1 picks up the earlier shot's last pose. Only layers
still exactly as they were stamped are touched, so anything you've already
drawn is left alone.

### Handing off to Animate, or a final polish

**Sequence ▸ Flatten to Master Scene** builds a **new** Normal scene with every
shot laid end to end. Your sequence is kept.

---

## Way 3 — Let the Director make the scenes

**Scene ▸ Direct a Story…** opens the **Story panel**. Type what happens:

```
Night. Ana walks in from the left.
Ana talks to Ben for 4 seconds. Ben listens.

A storm. By the river.
Ben walks off right.
```

- **Direct this shot** stages, casts, blocks and frames the scene that is open
  from the words.
- **Direct the whole brief** makes **one scene per paragraph** (a blank line
  starts a new one), played in order and named after their own words.

What comes out is ordinary layers and keyframes that you can redraw, and
directing the open shot is one Ctrl+Z to take back. Each scene keeps its words: click a shot in the
Story panel's list to go to it and bring its words back into the box. The full
reference for what the Director understands is
[SCENES_AND_THE_DIRECTOR.md](SCENES_AND_THE_DIRECTOR.md).

---

## Menu reference

### Scene ▸ (top part)

| Item | Does |
|---|---|
| Add Scene | A new empty Normal scene |
| Duplicate Scene | A copy of this scene to carry on from |
| Set the Scene… | Story panel, **Set** section: ground, backdrop, light rig, cast |
| Direct a Story… | Story panel, ready to type |
| Scenery… | Story panel, **Scenery** section: trees, rocks and so on, from your own symbols if you like |

### Sequence ▸

| Item | Does | Greyed out / missing when |
|---|---|---|
| Set Master Audio… | Import the dialogue track | Refuses on a Setup or Shot scene (run it from a Normal scene) |
| Use Existing Sound Layer ▸ | Use a sound already in the scene | Missing unless a layer has sound on frame 1 |
| Make Setup from This Scene | Turn this scene into a reusable setup | — |
| New Shot from Setup ▸ | First shot from the playhead to the end of the audio | No setups yet |
| Cut on Dialogue ▸ | One shot per spoken line | No setups yet |
| Flatten to Master Scene | New Normal scene with all shots end to end | — |

### Shot ▸ (needs a shot open; double-click a block)

| Item | Does |
|---|---|
| Split at Playhead | Cut this shot in two at the playhead (**S** over the strip) |
| Try Another Setup ▸ | New take of this audio from another setup; old take kept |
| Duplicate Shot | Another take of the same audio |
| Add Missing Setup Layers | Bring in layers added to the setup since |
| Edit Setup | Open the setup this shot was made from |
| Continue from Shot ▸ | Start from an earlier shot's last pose (same setup only) |

### Sequence strip (mouse and keys)

| Do | To |
|---|---|
| Click a block | Select it |
| Double-click a block | Open that shot |
| Drag the handle between two blocks | Move the cut |
| Right-click a block | Duplicate Shot, Try Another Setup, Add Missing Setup Layers, Continue from Shot |
| **Take** switcher on a block | Choose which take is in the film |
| **S** with the pointer over the strip | Split at Playhead |

---

## Troubleshooting

- **The whole Shot menu is grey.** No shot is open. Double-click a block in
  the Sequence strip first.
- **New Shot from Setup / Cut on Dialogue are grey.** Make a setup first
  (*Sequence ▸ Make Setup from This Scene*).
- **I changed a setup and my shots didn't change.** That's intended: shots are
  copies. Use *Shot ▸ Add Missing Setup Layers* for new layers, or redo a shot
  from the setup with *Try Another Setup*.
- **My setup isn't in the exported video.** Setups never are. They're
  sources. Make a shot from it, or keep it as a Normal scene if it should
  play.
- **Undo didn't take back a cut or a new shot.** Sequence commands (setups,
  shots, takes, cuts) aren't on Undo. None of them delete anything, so a
  mistake just leaves an extra scene; delete it from the scene menu.
- **I deleted a scene by accident.** Delete Scene can't be undone. If the
  document was saved, reopen it without saving. *File ▸ Snapshots…* may also
  have an earlier copy.

---

## Going deeper

- [SEQUENCE_GUIDE.md](SEQUENCE_GUIDE.md): the setup/shot rules in full.
- [SCENES_AND_THE_DIRECTOR.md](SCENES_AND_THE_DIRECTOR.md): what *Set the
  Scene* builds, what *Direct a Story* understands, and the motion that runs
  by itself (breathing, sway, fire, storms).
- [SEQUENCE_SETUPS_PLAN.md](SEQUENCE_SETUPS_PLAN.md): the design behind
  setups and shots, for developers.
