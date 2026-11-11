//! **Life and speech for the characters on the stage**
//! (SIMPLE_EPISODE_FLOW_PLAN E2 and E3).
//!
//! * E2: a Cast character put into the shot gets breath (the `Idle` chest rise on its layer rig) and
//!   blinks (the head's own `Eyelid` chart, keyed inside the head) from the playhead to the scene's
//!   end ([`add_life`]); Character > Add Life to All Characters runs it again on what is already there.
//!   Both write ordinary keys, so they can be edited, and running either again replaces them.
//! * E3: Sequence > Lip Sync All Characters ([`lip_sync_all`]) reads the lines of a dialogue recording
//!   (who says what, and when) and writes, for each character on the stage, the mouth and the `Talk`
//!   movement on their own lines and the breath in between. A narrator, and a speaker with no character
//!   on the stage, move nobody; they are listed in the report.

use std::collections::HashSet;

use buzz_act::audio_align::{self, AudioLine, Heard};
use buzz_act::layer_rig::{self, LayerRigView};
use buzz_act::perform::{self, Action, Performance};
use buzz_act::screenplay::{Item, ScriptScene, Voice};
use buzz_audio::{Clip, Viseme, VisemeTrack};
use buzz_doc::{Asset, AssetLibrary};
use buzz_geom::Affine;
use buzz_scene::{LayerId, LayerKind, ObjectId, Scene};

/// A Cast character on the stage: its folder (named like the character) and where it came from.
#[derive(Debug, Clone, PartialEq)]
pub struct StageCharacter {
    pub folder: LayerId,
    pub name: String,
    /// `Cast/<Series>/<Name>`.
    pub source: String,
}

/// Every Cast character lifted onto the stage, in layer order.
pub fn stage_characters(scene: &Scene) -> Vec<StageCharacter> {
    scene
        .layers()
        .iter()
        .filter(|l| l.kind == LayerKind::Folder)
        .filter_map(|l| {
            let source = l.cast_source.clone().filter(|s| !s.contains('|'))?;
            Some(StageCharacter { folder: l.id, name: l.name.clone(), source })
        })
        .collect()
}

impl StageCharacter {
    /// The names this character answers to: its folder's and the asset's.
    fn names(&self) -> Vec<String> {
        let asset = self.source.rsplit('/').next().unwrap_or("").to_string();
        vec![self.name.clone(), asset]
    }

    /// Does a script speaker (`DEEPAK`, `Mishra ji`) mean this character?
    pub fn is_speaker(&self, speaker: &str) -> bool {
        let want = normal(speaker);
        !want.is_empty() && self.names().iter().any(|n| normal(n) == want)
    }
}

fn normal(name: &str) -> String {
    name.split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty()).collect::<Vec<_>>().join(" ").to_lowercase()
}

/// The character's current rig (the pose showing at `frame`, else at frame 0).
fn rig_of(scene: &Scene, ch: &StageCharacter, frame: u32) -> Option<LayerRigView> {
    buzz_act::pose::rig_shown(scene, ch.folder, frame).or_else(|| buzz_act::pose::rig_shown(scene, ch.folder, 0))
}

/// Remove the keys a performance wrote over `from..to` on a rig's part layers (never frame 0, never a
/// blank key: a pose switch's blank is not ours), so a run again replaces rather than mixes.
fn clear_keys(scene: &mut Scene, view: &LayerRigView, from: u32, to: u32) {
    let layers: Vec<LayerId> = view.layers().collect();
    for layer in layers {
        let starts: Vec<u32> = scene
            .layers()
            .get(layer)
            .map(|l| l.frames.keyframes().iter().filter(|k| k.start > 0 && k.start >= from && k.start < to && !k.objects.is_empty()).map(|k| k.start).collect())
            .unwrap_or_default();
        scene.update_layer(layer, |l| {
            for s in starts {
                l.frames.clear_keyframe(s);
            }
        });
    }
}

/// Write `action` over `frames` on the rig (placement taken once, by the caller).
fn perform_on(scene: &mut Scene, body: ObjectId, placed: Affine, action: Action, frames: std::ops::Range<u32>, step: u32) -> bool {
    if frames.end <= frames.start + 2 {
        return false;
    }
    let performance = Performance { action, frames, amount: if action == Action::Idle { 0.9 } else { 1.0 }, tempo: 1.0, distance: 0.0, step };
    perform::apply_from(scene, body, &performance, placed).is_ok()
}

/// **Blink keys** for the head's `Eyelid` chart over `from..to`: half, shut, half, open for each blink,
/// on the blink times of this character (seeded by name), eyes open at `from`. Returns the keys written.
fn write_blinks(scene: &mut Scene, view: &LayerRigView, seed: &str, from: u32, to: u32, fps: f64) -> usize {
    let Some(face) = buzz_act::face::face_of(scene, view) else { return 0 };
    let Some((spot, frames)) = face.lid else { return 0 };
    let starts: Vec<u32> = crate::episode::blink_starts(seed, to, fps).into_iter().filter(|s| *s >= from).collect();
    let mut keys: Vec<(u32, u32)> = vec![(from, frames[0])];
    for b in &starts {
        for step in [0u32, 2, 4, 6] {
            if b + step < to {
                keys.push((b + step, frames[crate::episode::eye_state(&starts, b + step)]));
            }
        }
    }
    keys.sort_by_key(|k| k.0);
    keys.dedup_by_key(|k| k.0);
    buzz_act::face::clear_chart_keys(scene, spot, from, to);
    buzz_act::face::put_chart_keys(scene, &face, spot, &keys);
    keys.len()
}

/// What [`add_life`] did.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct LifeReport {
    /// The characters given life.
    pub characters: Vec<String>,
    pub breath_keys: usize,
    pub blink_keys: usize,
}

/// **Breath and blinks for every Cast character on the stage** from `from` to `to`.
pub fn add_life(scene: &mut Scene, from: u32, to: u32) -> LifeReport {
    let mut report = LifeReport::default();
    let fps = scene.stage().frame_rate.max(1.0);
    let to = to.max(from + 8);
    for ch in stage_characters(scene) {
        if life_of(scene, &ch, from, to, fps, &mut report) {
            report.characters.push(ch.name.clone());
        }
    }
    report
}

fn life_of(scene: &mut Scene, ch: &StageCharacter, from: u32, to: u32, fps: f64, report: &mut LifeReport) -> bool {
    let Some(view) = rig_of(scene, ch, from) else { return false };
    let body = view.body;
    let mut any = false;
    if let Some(placed) = layer_rig::placement(scene, body) {
        clear_keys(scene, &view, from, to);
        if perform_on(scene, body, placed, Action::Idle, from..to, 4) {
            report.breath_keys += ((to - from) / 4) as usize;
            any = true;
        }
    }
    let n = write_blinks(scene, &view, &ch.name, from, to, fps);
    report.blink_keys += n;
    any || n > 0
}

/// What [`cast_into_shot`] put on the stage.
#[derive(Debug, Clone, PartialEq)]
pub struct Cast {
    /// The character's folder (a combined Cast asset unpacked as a layer rig), `None` for a plain asset.
    pub folder: Option<LayerId>,
    /// The layers that arrived.
    pub layers: Vec<LayerId>,
    pub life: LifeReport,
}

/// Where the n-th character stands across the stage (fraction of its width).
const SLOTS: [f64; 5] = [0.5, 0.3, 0.7, 0.15, 0.85];

/// **Put a Cast character into the shot**: a combined asset is unpacked as a layer rig (its Front pose, a
/// folder named after it, a head of its own), stood on the ground in the next free place across the
/// stage, and, with `life`, given breath and blinks from frame `from` to the scene's end.
pub fn cast_into_shot(scene: &mut Scene, assets: &AssetLibrary, asset: &Asset, life: bool, from: u32) -> Result<Cast, String> {
    let already = stage_characters(scene).len();
    let (placed, folder) = crate::episode::place_cast(scene, assets, asset)?;
    let Some(bounds) = placed.bounds else {
        return Err(format!("{:?} has nothing to draw", asset.name));
    };
    let stage = scene.stage().stage_rect();
    let height = stage.height() * 0.62;
    let x = stage.x0 + stage.width() * SLOTS[already % SLOTS.len()];
    let feet = stage.y1 - stage.height() * 0.08;
    let (t, _) = crate::episode::stand_on(bounds, height, x, feet);
    crate::episode::transform_layers(scene, &placed.top, t);
    // Every layer that arrived lasts to the scene's end (a placed layer is as long as its symbol).
    let to = scene.frame_count().max(from + 24);
    for id in placed.all.iter().copied().chain(folder) {
        scene.update_layer(id, |l| {
            if l.frames.length() < to {
                l.frames.insert_frame(to - 1);
            }
        });
    }
    let mut report = LifeReport::default();
    if life && let Some(folder) = folder {
        let ch = StageCharacter { folder, name: scene.layers().get(folder).map(|l| l.name.clone()).unwrap_or_default(), source: String::new() };
        let fps = scene.stage().frame_rate.max(1.0);
        if life_of(scene, &ch, from, to, fps, &mut report) {
            report.characters.push(ch.name);
        }
    }
    Ok(Cast { folder, layers: placed.all, life: report })
}

// -- E3: lip sync all ----------------------------------------------------------------------

/// One spoken line found in the recording: who says it, what and when (seconds in the clip).
#[derive(Debug, Clone, PartialEq)]
pub struct SpokenLine {
    /// `None` when the speaker is not known.
    pub speaker: Option<String>,
    /// A voice over the picture (a narrator): nobody's mouth moves.
    pub narrator: bool,
    pub text: String,
    pub start: f64,
    pub end: f64,
}

/// **Find the script's lines in the recording**: its utterances are matched to the script's lines in order
/// (with the transcript when there is one), and each matched line carries its speaker.
pub fn find_lines(clip: &Clip, heard: &[Heard], scenes: &[&ScriptScene], path: &std::path::Path) -> (Vec<SpokenLine>, Vec<String>) {
    let plan = audio_align::plan(scenes, path, clip, if heard.is_empty() { None } else { Some(heard) });
    let mut lines = Vec::new();
    for AudioLine { scene, item, start, end } in &plan.lines {
        let Some(src) = scenes.iter().find(|s| s.number == *scene) else { continue };
        let Some(Item::Dialogue(d)) = src.items.get(*item) else { continue };
        let narrator = d.voice == Voice::VoiceOver || normal(&d.speaker) == "narrator";
        lines.push(SpokenLine { speaker: Some(d.speaker.clone()), narrator, text: d.text.clone(), start: *start, end: *end });
    }
    (lines, plan.notes)
}

/// What hearing the dialogue found.
#[derive(Debug, Clone)]
pub struct Hearing {
    pub lines: Vec<SpokenLine>,
    /// The mouth shapes in the recording, from its first frame.
    pub visemes: VisemeTrack,
    pub notes: Vec<String>,
    /// Seconds the hearing took.
    pub secs: f64,
    pub model: String,
}

/// **Hear the dialogue**: transcribe the recording locally (Whisper, when it is installed), find the
/// script's lines in it ([`find_lines`]) or, with no script, take what was heard as the lines and give each
/// to the character whose name it mentions; read the mouth shapes. Blocks: run it off the UI thread.
pub fn hear_dialogue(
    clip: &Clip,
    script: &buzz_act::screenplay::Screenplay,
    characters: &[String],
    fps: f64,
    progress: &mut dyn FnMut(usize, usize),
    cancel: &dyn Fn() -> bool,
) -> Result<Hearing, String> {
    let started = std::time::Instant::now();
    let mut notes = Vec::new();
    let mut model = "no transcript".to_string();
    let mut transcript: Vec<Heard> = Vec::new();
    if buzz_ai::transcribe::installed_whisper().is_ok() {
        match buzz_ai::transcribe::transcribe_clip(clip, progress, cancel) {
            Ok(t) => {
                model = format!("Whisper base.en, {}", t.provider);
                transcript = t.segments.into_iter().map(|s| Heard { start: s.start, end: s.end, text: s.text }).collect();
            }
            Err(e) => {
                if cancel() {
                    return Err("cancelled".into());
                }
                notes.push(format!("no transcript ({e}); the lines are found by their lengths"));
            }
        }
    } else {
        notes.push("Whisper is not installed (AI > Models); the lines are found by their lengths".to_string());
    }
    let scenes: Vec<&ScriptScene> = script.scenes.iter().collect();
    let mut lines: Vec<SpokenLine>;
    if scenes.is_empty() {
        // No script: what was heard is the line, spoken by the character it names.
        lines = transcript
            .iter()
            .map(|h| {
                let speaker = characters.iter().find(|c| normal(&h.text).split(' ').any(|w| normal(c).split(' ').next() == Some(w))).cloned();
                SpokenLine { narrator: speaker.is_none(), speaker, text: h.text.trim().to_string(), start: h.start, end: h.end }
            })
            .collect();
        notes.push("no script loaded in the Episode window: lines that do not name a character count as narration".to_string());
    } else {
        let (found, n) = find_lines(clip, &transcript, &scenes, std::path::Path::new(""));
        lines = found;
        notes.extend(n);
    }
    lines.sort_by(|a, b| a.start.total_cmp(&b.start));
    let visemes = buzz_audio::analyse_visemes(clip, fps, &buzz_audio::LipSyncOptions::default());
    Ok(Hearing { lines, visemes, notes, secs: started.elapsed().as_secs_f64(), model })
}

/// What Lip Sync All Characters did.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct LipSyncAllReport {
    pub lines: usize,
    /// Lines given to a character on the stage.
    pub matched: usize,
    pub narrator_lines: usize,
    /// Lines whose speaker has no character on the stage: `SPEAKER: first words`.
    pub unmatched: Vec<String>,
    pub characters: Vec<String>,
    /// Characters with a mouth (a `Mouth` layer in the head, or a mouth part) that were written.
    pub mouths: Vec<String>,
    pub warnings: Vec<String>,
}

/// The frames a line covers on the scene's timeline.
fn span_of(l: &SpokenLine, sound_start: u32, fps: f64) -> std::ops::Range<u32> {
    let a = sound_start + (l.start * fps).round() as u32;
    let b = sound_start + (l.end * fps).round() as u32;
    a..b.max(a + 2)
}

fn even(frame: u32) -> u32 {
    frame & !1
}

/// **Lip sync every character from the lines**: for each character on the stage the mouth over their own
/// lines (closed everywhere else), `Talk` over those lines and breath in between, blinks throughout.
/// `track` is the shapes heard in the recording from its first frame at `sound_start`. One scene edit:
/// the caller wraps it in one undo step. Writing again replaces what was written.
pub fn lip_sync_all(scene: &mut Scene, track: &VisemeTrack, sound_start: u32, lines: &[SpokenLine]) -> LipSyncAllReport {
    let fps = scene.stage().frame_rate.max(1.0);
    let mut report = LipSyncAllReport { lines: lines.len(), ..Default::default() };
    let chars = stage_characters(scene);
    let end = scene.frame_count().max(lines.iter().map(|l| span_of(l, sound_start, fps).end).max().unwrap_or(0) + 12);
    let mut claimed: HashSet<usize> = HashSet::new();
    let mut spans: Vec<Vec<std::ops::Range<u32>>> = vec![Vec::new(); chars.len()];
    for l in lines {
        if l.narrator {
            report.narrator_lines += 1;
            continue;
        }
        let who = l.speaker.clone().unwrap_or_default();
        let words: Vec<&str> = l.text.split_whitespace().take(5).collect();
        match chars.iter().position(|c| c.is_speaker(&who)) {
            Some(i) => {
                spans[i].push(span_of(l, sound_start, fps));
                claimed.insert(i);
                report.matched += 1;
            }
            None => report.unmatched.push(format!("{}: {}", if who.is_empty() { "?" } else { &who }, words.join(" "))),
        }
    }
    for (i, ch) in chars.iter().enumerate() {
        let Some(view) = rig_of(scene, ch, 0) else {
            report.warnings.push(format!("{}: not a rig, so it does not perform", ch.name));
            continue;
        };
        let body = view.body;
        let Some(placed) = layer_rig::placement(scene, body) else { continue };
        clear_keys(scene, &view, 0, end);
        // Talk over the character's lines (merging lines a breath apart), breath in the gaps.
        let mut talks: Vec<std::ops::Range<u32>> = Vec::new();
        for s in &spans[i] {
            let s = even(s.start.saturating_sub(2))..(s.end + 2).min(end);
            match talks.last_mut() {
                Some(last) if s.start <= last.end + 8 => last.end = last.end.max(s.end),
                _ => talks.push(s),
            }
        }
        talks.sort_by_key(|t| t.start);
        let mut cursor = 0u32;
        for t in &talks {
            if t.start > cursor + 3 {
                perform_on(scene, body, placed, Action::Idle, cursor..t.start, 4);
            }
            perform_on(scene, body, placed, Action::Talk, t.clone(), 2);
            cursor = t.end;
        }
        if cursor + 3 < end {
            perform_on(scene, body, placed, Action::Idle, cursor..end, 4);
        }
        write_blinks(scene, &view, &ch.name, 0, end, fps);
        if write_mouth(scene, &view, track, sound_start, &spans[i], end) {
            report.mouths.push(ch.name.clone());
        } else if !spans[i].is_empty() {
            report.warnings.push(format!("{}: no mouth found on the rig, so its lines are not lip-synced", ch.name));
        }
        report.characters.push(ch.name.clone());
    }
    report
}

/// The mouth of one character: the shapes heard over its own lines, Rest everywhere else.
fn write_mouth(scene: &mut Scene, view: &LayerRigView, track: &VisemeTrack, sound_start: u32, spans: &[std::ops::Range<u32>], end: u32) -> bool {
    let mut frames = vec![Viseme::Rest; end as usize + 1];
    for s in spans {
        for f in s.clone() {
            let Some(offset) = f.checked_sub(sound_start) else { continue };
            if let Some(slot) = frames.get_mut(f as usize) {
                *slot = track.at(offset);
            }
        }
    }
    let mine = VisemeTrack { frames, fps: track.fps };
    if let Some(face) = buzz_act::face::face_of(scene, view)
        && let Some(spot) = face.mouth
    {
        let keys = buzz_act::face::mouth_keys(scene, &face, &mine.runs());
        buzz_act::face::clear_chart_keys(scene, spot, 0, end + 1);
        buzz_act::face::put_chart_keys(scene, &face, spot, &keys);
        return true;
    }
    if let Some(mouth) = buzz_act::textsync::find_layer_rig_mouth(scene, view) {
        return buzz_act::textsync::write_layer_rig_mouth(scene, mouth, &mine, 0).is_ok();
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_script_speaker_is_matched_to_a_character_by_name() {
        let ch = StageCharacter { folder: LayerId(1), name: "MISHRA JI".into(), source: "Cast/Khayal3Baje/MISHRA JI".into() };
        assert!(ch.is_speaker("Mishra ji"));
        assert!(ch.is_speaker("MISHRA-JI"));
        assert!(!ch.is_speaker("MISHRA"));
        assert!(!ch.is_speaker(""));
    }
}
