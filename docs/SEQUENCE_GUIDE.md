# Sequence: one setup, many shots, cut on the dialogue

> **Status (2026-09-27): built.** Every step below is implemented and covered
> by tests (`crates/buzz-app/tests/sequence_setups.rs`). Sequence commands
> (making setups, shots, takes, cuts) are not on Undo; none of them delete
> anything, so a mistaken one leaves an extra scene you can delete from the
> scene menu.

Use this when a film is a conversation: the same few camera setups (Ana facing
camera, Ben facing camera, the village wide) come back again and again, and the
dialogue audio decides when each one is on screen.

## The idea in one paragraph

You build each **setup** once. You import the dialogue once as the **master
audio**. Then you cut the audio into **shots**, and each shot is made *from* a
setup. A shot is its own copy: animate it freely, and nothing you do there, or
to the setup afterwards, changes any other shot. Each shot starts at its own
point in the audio, and that point is **frame 1 of the shot**.

## Three kinds of scene

| Kind | Use it for | In the exported film? |
|---|---|---|
| **Normal** | Ordinary Animate-style work: titles, one-off scenes, a master scene | Yes |
| **Setup** | A reusable arrangement: background, props, lights, camera framing, characters placed | No, it's a source |
| **Shot** | One cut: a setup + a stretch of the master audio | Yes, in sequence order |

Normal scenes work exactly as before. None of the rules below apply to them.

## Step by step

1. **Import the dialogue.** *Sequence ▸ Set Master Audio…* and pick the file.
   The Sequence strip (bottom dock, beside the Timeline) shows its waveform
   across the whole film.
2. **Build your setups.** Arrange a scene as usual (or use *Scene ▸ Set the
   Scene…*), then *Sequence ▸ Make Setup from This Scene*. Make one per camera
   position: "Ana CU", "Ben CU", "Village wide".
3. **Cut the audio into shots.**
   - Put the playhead where a new shot should begin and choose *Sequence ▸
     Split at Playhead* (or press **S** with the pointer over the Sequence
     strip), or
   - *Sequence ▸ Cut on Dialogue ▸ ‹a setup›*: one shot per spoken line, found
     from the audio (a quick breath does not get its own shot). Pick the
     setup new lines default to. With a subtitle (SRT) file imported and its
     captions on the master scene naming speakers, a line whose speaker's name
     is mentioned by one of your setups goes there instead — "Ana" goes to
     "Ana CU" even though you picked "Village wide" as the default. Never
     re-cuts a shot that is already in the sequence; it only fills the
     audio no shot covers yet.
4. **Give each shot a setup.** *Sequence ▸ New Shot from Setup* stamps the
   very first shot (from wherever the playhead sits to the end of the
   remaining audio). After that, right-click a shot block ▸ *Try Another
   Setup* to stamp a fresh take of that block's own slice from a different
   setup — the block switches to it, and the old take is kept, not thrown
   away. Use the same setup as often as you like: shot 3 and shot 4 can both
   be "Village wide", each with its own copy.
5. **Animate each shot.** Double-click a shot block to open it. The Timeline
   now counts from that shot's frame 1, which is where its audio starts. Run
   lip-sync here and it reads only this shot's slice of the audio.
6. **Play or export the film.** Shots play in order with the dialogue running
   continuously across the cuts. Setups are left out.

## Things you can rely on

- **Nothing is overwritten.** A shot never changes unless you edit that shot.
  Changing a setup only affects shots you stamp from it *afterwards*.
- **Moving a cut is safe.** Dragging a boundary between two shots changes
  where their audio starts and stops. Animation past a shot's new end is kept
  (greyed out) and comes back if you make the shot longer again. Frame 1 stays
  frame 1.
- **Try things without losing them.** *Shot ▸ Duplicate Shot* makes another
  **take** of the same stretch of audio. Try a different setup the same way:
  it makes a new take and keeps the old one. Switch which take is in the cut
  from the shot block.
- **Characters stay consistent.** A character's *drawing* is its Cast symbol,
  shared everywhere as in Animate, so a costume fix shows up in every shot.
  Their *animation* is always per shot.

## Handy extras

- **Continue from Shot**: when you cut away and back (village → Ana →
  village), *Shot ▸ Continue from Shot ▸ ‹an earlier shot of this setup›* (or
  right-click the shot's own block in the Sequence strip) sets this shot's
  frame 1 to that earlier shot's own last pose, layer by layer. Only a layer
  still exactly as it was stamped is touched; a layer you have already drawn
  on is left alone and named, so nothing you did is ever overwritten. The
  earlier shot itself never changes.
- **Add Missing Setup Layers**: if you added a layer to a setup later, this
  adds it to an existing shot. It only ever adds; it never replaces anything.
- **Flatten to Master Scene**: builds a *new* normal scene with every shot
  laid end to end, for an Animate hand-off or a final polish. The sequence
  is kept.

## Quick reference

| Do this | Where |
|---|---|
| Import dialogue | Sequence ▸ Set Master Audio… |
| Make a setup | Sequence ▸ Make Setup from This Scene |
| Cut a shot | Sequence ▸ Split at Playhead (S over the strip) |
| Auto-cut per line | Sequence ▸ Cut on Dialogue ▸ ‹a setup› |
| Make the first shot | Sequence ▸ New Shot from Setup |
| Give an existing shot a different setup | Right-click the shot block ▸ Try Another Setup |
| Another take | Shot ▸ Duplicate Shot |
| Open a shot | Double-click its block |
| Pull in new setup layers | Shot ▸ Add Missing Setup Layers |
| Pick up an earlier shot's pose | Shot ▸ Continue from Shot ▸ ‹a shot›, or right-click the block |
| One plain scene of everything | Sequence ▸ Flatten to Master Scene |
