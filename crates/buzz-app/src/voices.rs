//! **Character voices** in the app: the voices he has made, which character
//! speaks with which, and re-voicing a recording line by line (plan stages
//! V2–V4 of `docs/VOICES_AND_ACTING_PLAN.md`).
//!
//! * A voice is a [`VoiceProfile`] saved as `<library>/Voices/<name>.voice.json`
//!   — his, reusable across every series, like the rest of the library.
//! * Which character speaks with which voice is per series, beside the
//!   casting: `<library>/Cast/<Series>/voices.json`, speaker name (upper-case,
//!   as the script writes the cue) → voice name.
//! * [`revoice_plan`] takes the recording's line times (`audio.json`) and
//!   writes a re-voiced copy of every recording in which **each line is in its
//!   own speaker's voice and no one else's**, with the plan moved to the new
//!   times. The build then cuts, captions and lip-syncs against that copy, and
//!   since lip sync only ever moves the speaker's mouth over the speaker's own
//!   lines, each character speaks — and is voiced — only in its own segments.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use buzz_act::audio_align::{AudioLine, AudioPlan};
use buzz_act::screenplay::{Item, ScriptScene};
use buzz_act::layer_rig::CharacterRig;
use buzz_audio::Clip;
use buzz_scene::{LayerId, LayerKind, ObjectId, Scene};
use buzz_voice::{Emotion, Line, VoiceProfile};

/// `<library>/Voices`.
pub fn voices_dir(root: &Path) -> PathBuf {
    root.join("Voices")
}

/// A voice's file name: the name, with anything a file system would refuse
/// replaced.
fn file_for(root: &Path, name: &str) -> PathBuf {
    let safe: String = name
        .trim()
        .chars()
        .map(|c| if c.is_alphanumeric() || matches!(c, ' ' | '-' | '_' | '.') { c } else { '_' })
        .collect();
    voices_dir(root).join(format!("{}.voice.json", if safe.is_empty() { "Untitled" } else { safe.as_str() }))
}

/// Every saved voice, by name.
pub fn list(root: &Path) -> Vec<VoiceProfile> {
    let Ok(dir) = std::fs::read_dir(voices_dir(root)) else {
        return Vec::new();
    };
    let mut voices: Vec<VoiceProfile> = dir
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().ends_with(".voice.json"))
        .filter_map(|e| std::fs::read_to_string(e.path()).ok())
        .filter_map(|t| serde_json::from_str::<VoiceProfile>(&t).ok())
        .filter(|v| !v.name.trim().is_empty())
        .collect();
    voices.sort_by_key(|v| v.name.to_lowercase());
    voices
}

pub fn load(root: &Path, name: &str) -> Option<VoiceProfile> {
    let text = std::fs::read_to_string(file_for(root, name)).ok()?;
    serde_json::from_str(&text).ok()
}

pub fn save(root: &Path, voice: &VoiceProfile) -> Result<PathBuf> {
    let path = file_for(root, &voice.name);
    std::fs::create_dir_all(voices_dir(root))?;
    std::fs::write(&path, serde_json::to_string_pretty(voice)?)
        .with_context(|| format!("saving {}", path.display()))?;
    Ok(path)
}

pub fn delete(root: &Path, name: &str) -> Result<()> {
    std::fs::remove_file(file_for(root, name)).with_context(|| format!("deleting voice {name}"))
}

/// `<library>/Cast/<Series>/voices.json`.
pub fn cast_voices_path(root: &Path, series: &str) -> PathBuf {
    crate::my_cast::casting_path(root, series).with_file_name("voices.json")
}

fn speaker_key(name: &str) -> String {
    name.trim().to_uppercase()
}

/// Speaker → voice name for one series.
pub fn load_cast_voices(root: &Path, series: &str) -> BTreeMap<String, String> {
    std::fs::read_to_string(cast_voices_path(root, series))
        .ok()
        .and_then(|t| serde_json::from_str::<BTreeMap<String, String>>(&t).ok())
        .unwrap_or_default()
        .into_iter()
        .map(|(k, v)| (speaker_key(&k), v))
        .collect()
}

pub fn save_cast_voices(root: &Path, series: &str, map: &BTreeMap<String, String>) -> Result<()> {
    let path = cast_voices_path(root, series);
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let map: BTreeMap<String, String> = map
        .iter()
        .filter(|(_, v)| !v.trim().is_empty())
        .map(|(k, v)| (speaker_key(k), v.clone()))
        .collect();
    std::fs::write(&path, serde_json::to_string_pretty(&map)?).with_context(|| format!("saving {}", path.display()))
}

/// Give `speaker` the voice `voice` in `series` (`None` takes it away: the
/// recording as it is).
pub fn assign(root: &Path, series: &str, speaker: &str, voice: Option<&str>) -> Result<()> {
    let mut map = load_cast_voices(root, series);
    match voice {
        Some(v) => map.insert(speaker_key(speaker), v.to_string()),
        None => map.remove(&speaker_key(speaker)),
    };
    save_cast_voices(root, series, &map)
}

/// Every speaker of `series` with a voice, resolved to its profile: the
/// voices given to characters in the Voices panel, with the series' own
/// choices over them. A name whose voice file has gone is reported and left
/// in his own voice.
pub fn resolve(root: &Path, series: &str, notes: &mut Vec<String>) -> BTreeMap<String, VoiceProfile> {
    let mut chosen: BTreeMap<String, String> =
        load_editor_voices(root).into_iter().map(|(k, v)| (speaker_key(&k), v)).collect();
    chosen.extend(load_cast_voices(root, series));
    let mut out = BTreeMap::new();
    for (speaker, name) in chosen {
        match load(root, &name) {
            Some(profile) => {
                out.insert(speaker, profile);
            }
            None => notes.push(format!("{speaker}'s voice \"{name}\" is not in {}", voices_dir(root).display())),
        }
    }
    out
}

/// The speaker and emotion of a script item, when it is a line.
fn line_of(scenes: &[&ScriptScene], scene: u32, item: usize) -> Option<(String, Emotion, f32)> {
    let s = scenes.iter().find(|s| s.number == scene)?;
    let Some(Item::Dialogue(d)) = s.items.get(item) else {
        return None;
    };
    // V5 reads the script's tag into the line; until then a line is said as
    // he said it.
    Some((speaker_key(&d.speaker), Emotion::Neutral, 0.0))
}

/// What re-voicing did, for the report.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RevoiceReport {
    /// Lines re-voiced, per speaker.
    pub per_speaker: BTreeMap<String, usize>,
    /// Lines left as recorded (their speaker has no voice).
    pub as_recorded: usize,
    /// The re-voiced files written.
    pub files: Vec<PathBuf>,
    pub notes: Vec<String>,
}

/// **Re-voice every recording in `plan`, each line by its own speaker's
/// voice**, writing `<out>/voiced/<file>.wav`, and return the plan moved onto
/// those files and their new times.
///
/// Returns the plan unchanged when nobody has a voice. Every recording is
/// read once and written once; a scene with a recording of its own gets its
/// own voiced file, exactly as `scene_paths` had it.
pub fn revoice_plan(
    plan: &AudioPlan,
    scenes: &[&ScriptScene],
    voices: &BTreeMap<String, VoiceProfile>,
    out: &Path,
    say: &mut dyn FnMut(&str),
) -> Result<(AudioPlan, RevoiceReport)> {
    let mut report = RevoiceReport::default();
    if voices.is_empty() {
        return Ok((plan.clone(), report));
    }
    // Which file each scene's lines are in.
    let mut files: BTreeMap<PathBuf, Vec<u32>> = BTreeMap::new();
    let scene_numbers: std::collections::BTreeSet<u32> =
        plan.lines.iter().map(|l| l.scene).chain(plan.scene_spans.keys().copied()).collect();
    for n in scene_numbers {
        files.entry(plan.path_for(n).to_path_buf()).or_default().push(n);
    }

    let dir = out.join("voiced");
    std::fs::create_dir_all(&dir)?;
    let mut new_plan = AudioPlan { notes: plan.notes.clone(), ..AudioPlan::default() };
    for (path, numbers) in files {
        let clip = buzz_audio::Clip::open(&path).with_context(|| format!("opening {}", path.display()))?;
        let mono = clip.mono();
        let in_file: Vec<AudioLine> = plan.lines.iter().filter(|l| numbers.contains(&l.scene)).copied().collect();
        let lines: Vec<Line> = in_file
            .iter()
            .map(|l| {
                let (speaker, emotion, intensity) = line_of(scenes, l.scene, l.item).unwrap_or_default();
                Line { speaker, start: l.start, end: l.end, emotion, intensity }
            })
            .collect();
        for line in &lines {
            if voices.contains_key(&line.speaker) {
                *report.per_speaker.entry(line.speaker.clone()).or_default() += 1;
            } else {
                report.as_recorded += 1;
            }
        }
        say(&format!(
            "voicing {} ({} line(s), {} speaker(s) with a voice)",
            path.file_name().map(|f| f.to_string_lossy().to_string()).unwrap_or_default(),
            lines.len(),
            lines.iter().filter(|l| voices.contains_key(&l.speaker)).map(|l| &l.speaker).collect::<std::collections::BTreeSet<_>>().len()
        ));
        let voiced = buzz_voice::voice_dialogue(&mono, clip.sample_rate, &lines, voices);

        let stem = path.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_else(|| "recording".into());
        let written = dir.join(format!("{stem}.voiced.wav"));
        let out_clip = buzz_audio::Clip::new(&stem, clip.sample_rate, 1, voiced.samples.clone())?;
        std::fs::write(&written, buzz_act::audio_align::wav_bytes(&out_clip))
            .with_context(|| format!("writing {}", written.display()))?;
        report.files.push(written.clone());

        // `voice_dialogue` hands lines back in time order; match them to the
        // plan's lines the same way.
        let mut order: Vec<usize> = (0..in_file.len()).collect();
        order.sort_by(|a, b| in_file[*a].start.total_cmp(&in_file[*b].start));
        for (placed, &i) in voiced.lines.iter().zip(&order) {
            new_plan.lines.push(AudioLine { start: placed.start, end: placed.end, ..in_file[i] });
        }
        for n in &numbers {
            if let Some((a, b)) = plan.scene_spans.get(n) {
                new_plan.scene_spans.insert(*n, (voiced.map_time(*a), voiced.map_time(*b)));
            }
            if path != plan.path {
                new_plan.scene_paths.insert(*n, written.clone());
            }
        }
        if path == plan.path || new_plan.path.as_os_str().is_empty() {
            new_plan.path = written.clone();
        }
    }
    new_plan.lines.sort_by(|a, b| (a.scene, a.item).cmp(&(b.scene, b.item)));
    Ok((new_plan, report))
}

// ------------------------------------------------------------------ the editor: one line at a time

/// A character's mouth, as the mouth writers drive it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mouth {
    /// A bone rig whose mouth is one of its parts.
    Rig(ObjectId),
    /// A layer rig's mouth: an instance of the mouth symbol on a layer of its own.
    LayerRig(ObjectId),
}

/// A character on the stage, as the Voices panel lists it.
#[derive(Debug, Clone, PartialEq)]
pub struct StageCharacter {
    /// What the character is called — the rig's instance name, else its
    /// folder, else its layer. This is the name voices are assigned under.
    pub name: String,
    /// Any object of the rig, for selecting it.
    pub body: ObjectId,
    pub mouth: Option<Mouth>,
}

/// The name a rig goes by: the first of its folder, its layer and its
/// instance name that is not a generic one (`Layer 3`, `Figure`, `Rig`) — the
/// names a program gives rather than the ones a person does.
fn rig_name(scene: &Scene, layer: LayerId, object: Option<&buzz_scene::Object>) -> String {
    let generic = |name: &str| {
        let word: String = name.trim().trim_end_matches(|c: char| c.is_ascii_digit() || c == ' ').to_lowercase();
        word.is_empty() || matches!(word.as_str(), "layer" | "figure" | "rig" | "armature" | "character" | "symbol" | "body" | "root")
    };
    let layers = scene.layers();
    let own = layers.get(layer);
    let folder = own.and_then(|l| l.parent).and_then(|p| layers.get(p)).map(|p| p.name.clone());
    [folder, own.map(|l| l.name.clone()), object.and_then(|o| o.name.clone())]
        .into_iter()
        .flatten()
        .map(|n| n.trim().to_string())
        .find(|n| !generic(n))
        .unwrap_or_else(|| "Character".into())
}

/// The character `object` belongs to, if it is part of a rig.
pub fn character_of(scene: &Scene, object: ObjectId) -> Option<StageCharacter> {
    match buzz_act::layer_rig::rig_of(scene, object)? {
        CharacterRig::Armature(body) => {
            let (layer, found) = scene.find_object(body)?;
            let mouth = buzz_act::textsync::find_rig_mouth(scene, body).map(|_| Mouth::Rig(body));
            Some(StageCharacter { name: rig_name(scene, layer, Some(found)), body, mouth })
        }
        CharacterRig::Layers(view) => {
            let mouth = buzz_act::textsync::find_layer_rig_mouth(scene, &view).map(Mouth::LayerRig);
            Some(StageCharacter { name: rig_name(scene, view.root, None), body: view.body, mouth })
        }
    }
}

/// Every character on the stage at `frame`, each once, by name.
pub fn stage_characters(scene: &Scene, frame: u32) -> Vec<StageCharacter> {
    let mut out: Vec<StageCharacter> = Vec::new();
    for layer in scene.layers().iter() {
        for object in layer.frames.objects_at(frame) {
            let Some(found) = character_of(scene, object.id) else {
                continue;
            };
            if !out.iter().any(|c| c.body == found.body) {
                out.push(found);
            }
        }
    }
    // By name, so the panel's list does not reshuffle when a layer moves.
    out.sort_by_key(|c| c.name.to_lowercase());
    out
}

/// What placing a line did.
#[derive(Debug, Clone, PartialEq)]
pub struct LinePlaced {
    pub layer: LayerId,
    /// The frames the line covers.
    pub frames: std::ops::Range<u32>,
    /// Mouth keys written; `None` when the character has no mouth to drive.
    pub mouth_keys: Option<u32>,
    pub voiced: bool,
}

/// The layer a character's lines go on: `Voice: <name>`, made when there is
/// not one.
fn voice_layer(scene: &mut Scene, name: &str) -> LayerId {
    let title = format!("Voice: {name}");
    if let Some(l) = scene.stage_layers().iter().find(|l| l.name == title && l.kind != LayerKind::Folder) {
        return l.id;
    }
    scene.add_stage_layer(title, LayerKind::Normal)
}

/// **Put one line on the timeline for one character.**
///
/// The take is re-voiced with `voice` (said with `emotion`), added to the
/// document as a sound on the character's own `Voice: <name>` layer starting
/// at `at`, and the character's mouth — **this character's and nobody
/// else's** — is lip-synced over exactly that line's frames. Its other lines,
/// and every other character, are left as they were.
pub fn place_line(
    scene: &mut Scene,
    who: &StageCharacter,
    take: &Clip,
    voice: Option<&VoiceProfile>,
    emotion: Emotion,
    intensity: f32,
    at: u32,
) -> Result<LinePlaced> {
    let rate = take.sample_rate;
    let mono = take.mono();
    let samples = match voice {
        Some(v) => buzz_voice::render(&mono, rate, &v.for_line(emotion, intensity)),
        None => mono.to_vec(),
    };
    let label = format!("{} line", who.name);
    let clip = Clip::new(&label, rate, 1, samples)?;
    let fps = scene.stage().frame_rate.max(1.0);
    let frames = clip.duration_frames(fps).max(1);
    let bytes = std::sync::Arc::new(buzz_act::audio_align::wav_bytes(&clip));
    let sound = scene.add_sound(&label, bytes, "wav", rate, 1, clip.len() as u64);

    let layer = voice_layer(scene, &who.name);
    let end = at + frames;
    scene.update_stage_layer(layer, |l| {
        l.frames.insert_blank_keyframe(at);
        if let Some(k) = l.frames.keyframe_at_mut(at) {
            k.sound = Some(buzz_scene::SoundRef::stream(sound));
            k.label = Some(who.name.clone());
        }
        // A blank key where the line ends, so the stream stops there and the
        // next line can go straight after it on the same layer.
        if !l.frames.is_keyframe(end) {
            l.frames.insert_blank_keyframe(end);
        }
    });

    let mouth_keys = match who.mouth {
        Some(mouth) => {
            let track = buzz_audio::analyse_visemes(&clip, fps, &buzz_audio::LipSyncOptions::default());
            let report = match mouth {
                Mouth::Rig(rig) => buzz_act::textsync::write_rig_mouth_span(scene, rig, &track, at),
                Mouth::LayerRig(m) => buzz_act::textsync::write_layer_rig_mouth_span(scene, m, &track, at),
            }
            .map_err(|e| anyhow::anyhow!("{}: {e}", who.name))?;
            Some(report.keyframes)
        }
        None => None,
    };
    Ok(LinePlaced { layer, frames: at..end, mouth_keys, voiced: voice.is_some() })
}

/// `<library>/Voices/characters.json`: which voice each character speaks
/// with in the editor, by the name the Voices panel shows. The Producer's
/// per-series map ([`load_cast_voices`]) wins over this for an episode.
pub fn editor_voices_path(root: &Path) -> PathBuf {
    voices_dir(root).join("characters.json")
}

pub fn load_editor_voices(root: &Path) -> BTreeMap<String, String> {
    std::fs::read_to_string(editor_voices_path(root))
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default()
}

pub fn save_editor_voices(root: &Path, map: &BTreeMap<String, String>) -> Result<()> {
    std::fs::create_dir_all(voices_dir(root))?;
    let path = editor_voices_path(root);
    std::fs::write(&path, serde_json::to_string_pretty(map)?).with_context(|| format!("saving {}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use buzz_act::screenplay::Screenplay;
    use buzz_voice::Preset;

    /// A figure rig named `name` with a lip-sync mouth, alone on a layer.
    fn rig(scene: &mut Scene, name: &str) -> ObjectId {
        use buzz_act::figure::{self, FigureSpec};
        let layer = scene.add_layer(name, LayerKind::Normal);
        let id = scene.next_object_id();
        let mut figure = figure::build(&FigureSpec::default(), id, || scene.next_object_id());
        let symbol = buzz_act::lipsync::placeholder_mouth(scene, &format!("{name}_mouth"));
        let mouth = buzz_scene::Object::instance_of(scene.next_object_id(), symbol);
        if let buzz_scene::ObjectKind::Armature(r) = &mut figure.kind {
            r.bind_rigid(std::sync::Arc::new(mouth), 2);
        }
        scene.add_object(layer, figure).expect("placed")
    }

    fn speech(sr: u32, secs: f32) -> Clip {
        // A voiced buzz with syllable-like loudness, so the analysis opens and
        // closes the mouth.
        let n = (secs * sr as f32) as usize;
        let samples = (0..n)
            .map(|i| {
                let t = i as f32 / sr as f32;
                let syllables = (std::f32::consts::TAU * 3.0 * t).sin().max(0.0);
                (1..15).map(|k| (std::f32::consts::TAU * 120.0 * k as f32 * t).sin() / k as f32).sum::<f32>() * 0.3 * syllables
            })
            .collect();
        Clip::new("take", sr, 1, samples).unwrap()
    }

    /// **A line put on the timeline for Raju moves Raju's mouth and nobody
    /// else's**, on his own voice layer, over exactly that line's frames.
    #[test]
    fn a_placed_line_moves_only_its_own_characters_mouth() {
        let mut scene = Scene::default();
        let raju = rig(&mut scene, "Raju");
        let guddu = rig(&mut scene, "Guddu");
        let cast = stage_characters(&scene, 0);
        let names: Vec<&str> = cast.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, vec!["Guddu", "Raju"]);
        let who = cast.iter().find(|c| c.name == "Raju").unwrap().clone();
        assert_eq!(who.mouth, Some(Mouth::Rig(raju)));

        let guddu_layer = scene.find_object(guddu).unwrap().0;
        let guddu_keys_before = scene.layers().get(guddu_layer).unwrap().frames.keyframe_count();
        let take = speech(22_050, 1.5);
        let placed = place_line(&mut scene, &who, &take, Some(&Preset::OldMan.profile()), Emotion::Neutral, 0.0, 24).unwrap();

        assert!(placed.voiced);
        assert_eq!(placed.frames.start, 24);
        assert!(placed.frames.end >= 24 + 36, "1.5 s at 24 fps, slowed by the old man's tempo: {:?}", placed.frames);
        assert!(placed.mouth_keys.unwrap() >= 2, "Raju's mouth was keyed");
        let layer = scene.stage_layers().get(placed.layer).unwrap();
        assert_eq!(layer.name, "Voice: Raju");
        assert!(layer.frames.keyframe_at(24).and_then(|k| k.sound).is_some(), "the line's sound starts at the playhead");
        assert_eq!(
            scene.layers().get(guddu_layer).unwrap().frames.keyframe_count(),
            guddu_keys_before,
            "Guddu was not touched"
        );
        // Every Raju key inside the line is at or after the playhead.
        let raju_layer = scene.find_object(raju).unwrap().0;
        let raju_keys: Vec<u32> = scene.layers().get(raju_layer).unwrap().frames.keyframes().iter().map(|k| k.start).collect();
        assert!(raju_keys.iter().all(|k| *k == 0 || *k >= 24), "{raju_keys:?}");
    }

    #[test]
    fn voices_save_list_and_assign() {
        let lib = tempfile::tempdir().unwrap();
        let root = lib.path();
        let raju = VoiceProfile { name: "Raju".into(), ..Preset::YoungMan.profile() };
        let amma = VoiceProfile { name: "Amma".into(), ..Preset::OldWoman.profile() };
        save(root, &raju).unwrap();
        save(root, &amma).unwrap();
        let names: Vec<String> = list(root).into_iter().map(|v| v.name).collect();
        assert_eq!(names, vec!["Amma", "Raju"]);

        assign(root, "Chirailiya", "Raju", Some("Raju")).unwrap();
        assign(root, "Chirailiya", "amma ji", Some("Amma")).unwrap();
        let map = load_cast_voices(root, "Chirailiya");
        assert_eq!(map.get("RAJU").map(String::as_str), Some("Raju"));
        assert_eq!(map.get("AMMA JI").map(String::as_str), Some("Amma"));

        assign(root, "Chirailiya", "RAJU", None).unwrap();
        let mut notes = Vec::new();
        let resolved = resolve(root, "Chirailiya", &mut notes);
        assert_eq!(resolved.keys().cloned().collect::<Vec<_>>(), vec!["AMMA JI"]);
        assert!(notes.is_empty());
    }

    /// **The whole point**: two speakers in one recording, only one with a
    /// voice — his lines are re-voiced, the other's are left exactly as
    /// recorded, and the plan's times follow the re-voiced file.
    #[test]
    fn each_speaker_is_voiced_only_on_their_own_lines() {
        let dir = tempfile::tempdir().unwrap();
        let sr = 22_050u32;
        // Two "lines" of a 120 Hz buzz with a pause between.
        let tone = |secs: f32| -> Vec<f32> {
            (0..(secs * sr as f32) as usize)
                .map(|i| {
                    let t = i as f32 / sr as f32;
                    (1..20).map(|k| (std::f32::consts::TAU * 120.0 * k as f32 * t).sin() / k as f32).sum::<f32>() * 0.2
                })
                .collect()
        };
        let mut samples = tone(0.8);
        samples.extend(vec![0.0; (0.4 * sr as f32) as usize]);
        samples.extend(tone(0.8));
        let rec = dir.path().join("take.wav");
        let clip = buzz_audio::Clip::new("take", sr, 1, samples).unwrap();
        std::fs::write(&rec, buzz_act::audio_align::wav_bytes(&clip)).unwrap();

        let play = Screenplay::parse("## SCENE 1 \u{2014} X\n\n**RAJU**\nArre Guddu!\n\n**GUDDU**\nHaan bhai.\n");
        let scene = play.scene(1).expect("scene 1");
        let items: Vec<usize> = scene
            .items
            .iter()
            .enumerate()
            .filter(|(_, i)| matches!(i, Item::Dialogue(_)))
            .map(|(n, _)| n)
            .collect();
        assert_eq!(items.len(), 2, "two lines in the scene: {:?}", scene.items);
        let plan = AudioPlan {
            path: rec.clone(),
            lines: vec![
                AudioLine { scene: 1, item: items[0], start: 0.0, end: 0.8 },
                AudioLine { scene: 1, item: items[1], start: 1.2, end: 2.0 },
            ],
            scene_spans: [(1, (0.0, 2.0))].into_iter().collect(),
            ..AudioPlan::default()
        };
        let mut voices = BTreeMap::new();
        voices.insert("RAJU".to_string(), VoiceProfile { tempo: 0.8, ..Preset::Woman.profile() });

        let (moved, report) = revoice_plan(&plan, &[scene], &voices, dir.path(), &mut |_| {}).unwrap();

        assert_eq!(report.per_speaker.get("RAJU"), Some(&1));
        assert_eq!(report.as_recorded, 1, "Guddu has no voice and stays as recorded");
        assert!(moved.path.starts_with(dir.path().join("voiced")));
        let voiced = buzz_audio::Clip::open(&moved.path).unwrap();
        let mono = voiced.mono();
        let at = |a: f64, b: f64| mono[(a * sr as f64) as usize..((b * sr as f64) as usize).min(mono.len())].to_vec();
        let raju = &moved.lines[0];
        let guddu = &moved.lines[1];
        assert!((raju.end - raju.start - 1.0).abs() < 0.03, "Raju's slower voice lengthens his line");
        let raju_pitch = buzz_voice::track_pitch(&at(raju.start, raju.end), sr).mean().unwrap();
        let guddu_pitch = buzz_voice::track_pitch(&at(guddu.start, guddu.end), sr).mean().unwrap();
        assert!(raju_pitch > 165.0, "Raju re-voiced: {raju_pitch}");
        assert!((guddu_pitch - 120.0).abs() < 5.0, "Guddu untouched: {guddu_pitch}");
        assert!(guddu.start > raju.end, "Guddu's line moved after Raju's longer one");
        let (_, span_end) = moved.scene_spans[&1];
        assert!((span_end - guddu.end).abs() < 0.02, "the scene span follows the new times");
    }
}
