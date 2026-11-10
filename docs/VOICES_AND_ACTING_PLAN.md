# Voices and acting — plan

**Goal.** The creator performs every line himself. Each character gets a voice
of its own, designed from his voice (or cloned from a sample), and the
performance — his timing, his emotion — comes through in that voice. The
voiced lines then drive the character: lip sync from the audio, and eyes, face,
head and body acting from the line's emotion (angry, horrified, serious,
happy …).

Decisions (from the brief, 2026-11-10):

* **No stock voices.** Voices are *made*: the creator's own recording pushed
  through a voice design (pitch, formants, tone …), male or female, or through
  a cloned target voice. Text-to-speech is not the source.
* Lines are **Hinglish in Roman script** — so nothing may depend on an English
  phoneme dictionary; lip sync stays audio-driven.
* A line's emotion comes from a **script tag** — `RAJU: (angry) Kya bol raha
  hai?` — and where there is none, the local LLM fills it in from the scene;
  offline, neutral. Editable per line.
* Used from **both** the Producer (whole episode) and the editor (one scene).

## What already exists (and is reused, not rebuilt)

| Need | Already in the tree |
|---|---|
| Record from the microphone | `buzz_audio::mic::MicInput`, VAD `UtteranceCutter` |
| Decode / resample / play | `buzz_audio::Clip`, `resample`, `player` |
| Script → lines | `buzz_act::screenplay` (`Dialogue { speaker, voice, text }`) |
| Line timing in a recording | `buzz_act::audio_align` (`AudioPlan`, `audio.json`) |
| Mouth from audio | `buzz_audio::analyse_visemes`, `buzz_act::lipsync`, `textsync::write_rig_mouth` / `write_layer_rig_mouth` |
| Expression mouths | `buzz_ai::face::mouth_chart` — frames 10.. (Broad Smile, Frown, Angry Tight Mouth …) |
| Blinks, breathing | `episode.rs` (eye parts with open / half / shut symbols) |
| Body acting | `buzz_act::perform::Action` — Talk, React, Point, Nod, HeadShake, Shrug, Wave, Turn … |
| Face parts on a rig | `buzz_rig::pattern` slots: eyebrows, eye whites, pupils, mouth, head |
| Per-series casting | `<library>/Cast/<Series>/casting.json` (`my_cast.rs`) |

## Stages

### V1 — the voice engine (`crates/buzz-voice`) — *in progress*

Pure Rust DSP, no model, no download, deterministic and testable.

* **Pitch analysis** — YIN F0 per 10 ms hop, voiced/unvoiced.
* **TD-PSOLA** — grains at pitch marks, re-spaced: pitch and duration change
  independently, formants stay put.
* **Formant shift** — resample by the formant factor (moves formants *and*
  pitch), then PSOLA corrects pitch and duration back. Pitch and formants are
  therefore two independent knobs, which is what turns a man into a woman or a
  child rather than a chipmunk.
* **Intonation range** — every pitch mark's target F0 is
  `mean · (f0 / mean)^range`: flatter (bored, robotic) or livelier (excited)
  than the performance.
* **Tone** — low shelf (warmth/body), high shelf (brightness), nasal peak,
  telephone / radio band.
* **Texture** — breathiness (envelope-shaped noise), roughness / growl
  (sub-harmonic modulation + jitter), tremor / vibrato (pitch LFO), tremolo.
* **Space** — small-room reverb, gain, peak normalisation.
* **`VoiceProfile`** — all of the above as serialisable numbers, with presets:
  Man, Deep Man, Young Man, Old Man, Woman, Young Woman, Old Woman, Boy, Girl,
  Giant, Little Creature, Ghost/Whisper, Robot, Radio.
* **Emotion prosody** — `Emotion` (Neutral, Happy, Sad, Angry, Horrified /
  Scared, Serious, Surprised, Disgusted, Whisper, Excited, Tired) adds a small
  offset on top of a voice (pitch, range, tempo, breath, tremor), so a
  performance that was *slightly* angry can be pushed further — never applied
  blindly: the creator's own expression is the main signal.

### V2 — Voice Lab panel

Window ▸ Voices. Record a test line (or import a WAV), pick a preset, move the
sliders, A/B the original against the voice, save it as a named voice in
`<library>/Voices/<name>.voice.json`. Male/female presets one click away.

### V3 — voices on characters

A voice per cast member: `voices` map in the series' `casting.json` (member id
→ voice name), set from the Voice Lab or from the cast member's row in the
Producer. The editor's character panel shows and changes it for the selected
rig.

### V4 — record the lines (teleprompter)

Producer ▸ Record Lines, and in the editor for the current scene: every line of
the script in order, speaker, emotion tag, a record button per line (VAD trims
the take), retake, play back in the character's voice. Takes are kept as
`<out>/takes/<scene>_<line>.wav`; the converted line as
`<out>/voiced/<scene>_<line>.wav`. Because every line is its own take, the line
times are *known* — `audio.json` is written directly, no alignment guessing.
An existing single recording still goes through `audio_align` as before, then
is cut per line and converted per speaker.

### V5 — emotion

* Parse `(angry)`, `(horrified)`, `[happy]` … parentheticals in
  `screenplay.rs` into `Dialogue::emotion` (+ an intensity word: *slightly*,
  *very*).
* LLM fill-in for untagged lines in the breakdown step (one call per scene).
* Audio cross-check: pitch / energy of the take vs the speaker's average
  (loud + high → angry/excited; quiet + breathy → sad/whisper) adjusts
  intensity, never the label the script gave.

### V6 — acting from emotion

Per line, from its emotion and intensity:

| Emotion | Eyes | Brows | Mouth (between visemes) | Head / body |
|---|---|---|---|---|
| Angry | narrowed (half), fixed on the listener | down, in | Angry Tight Mouth on rests | lean in, Point, sharp head jabs on stresses |
| Horrified / Scared | wide (eye whites scaled up), pupils small, darting | up, in | open on rests | React (recoil), step back, hands up, tremble |
| Happy | squint smile | up | Broad / Small Smile on rests | Nod, open hands, bounce |
| Sad | half, looking down | up, in | Frown | head down, shoulders drop, slow |
| Serious | steady, slow blinks | level, slightly down | Neutral Closed | minimal, still head, slow Nod |
| Surprised | wide, blink-stop | high | O | small React back |
| Disgusted | half, look away | one up | Skeptical Side-Mouth | turn head away, HeadShake |

Eye movement generally: look at whoever is being addressed (the previous
speaker, else the nearest cast member), saccade on line starts, blink on
thought changes (line start, after a pause), more blinks when nervous. Written
as ordinary keyframes on the rig — editable, like everything `buzz_act` writes.

### V7 — cloning (needs the creator's go-ahead before anything is installed)

A **converter backend** behind one trait: `convert(take, target) -> wav`. The
first backend is a local zero-shot speech-to-speech model run as a sidecar
process (as ComfyUI and LM Studio already are), driven over a small local HTTP
API: it keeps the creator's timing and emotion and changes the timbre to a
reference sample (10–30 s of the target voice). A cloned voice is a
`VoiceProfile` whose `clone` field names the reference; the DSP chain still
runs after it, so a clone can be aged, roughened, put on a radio, etc. The
model runs in its own process with its own licence; nothing is linked into the
app.

## The rule every stage keeps

**A line belongs to one speaker.** Re-voicing, lip sync and (V6) acting are
applied per line, to that line's speaker only: `buzz_voice::voice_dialogue`
re-voices each span with its own speaker's profile; the Producer's build only
ever moves the speaker's mouth over the speaker's lines; and in the editor
`voices::place_line` writes the mouth with the *span* writers
(`textsync::write_rig_mouth_span` / `write_layer_rig_mouth_span`), which touch
only that line's frames — so a character's earlier lines, and every other
character, are left exactly as they were.

## Status

| Stage | State |
|---|---|
| V1 engine (`crates/buzz-voice`) | **done** 2026-11-10 — YIN, PSOLA, formant shift, intonation, tempo, breath, rasp, quaver, tremble, tone, phone, compression, room, level; 15 presets; emotions with Hinglish tags; `voice_dialogue` per-speaker; `examples/voice_demo.rs` |
| V2 Voice Lab — **Window ▸ Voices** panel (`voices_panel.rs`), own column on the far right | **done** — record (`buzz_audio::TakeRecorder`, native rate) / import, play original / voiced, presets, every slider, save / delete in `<library>/Voices/` |
| V3 voices on characters | **done** — characters on stage listed with a voice each (`<library>/Voices/characters.json`); per-series `Cast/<Series>/voices.json` wins in the Producer |
| V4 lines | **done** for the editor (Put line on timeline: `Voice: <name>` layer at the playhead, only that character's mouth) and the Producer (re-voiced `voiced/*.wav`, plan moved to the new times). Teleprompter of the script's lines: planned |
| V5 emotion tags in the script, LLM fill-in | planned (engine reads tags already: `Emotion::parse`) |
| V6 acting from emotion (eyes, brows, expression mouths, gestures) | planned |
| V7 cloning backend | planned — needs a go-ahead before anything is installed |
