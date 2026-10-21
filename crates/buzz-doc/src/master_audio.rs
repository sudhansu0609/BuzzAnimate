//! **Master Audio mode: Enforce.**
//!
//! Makes the master clip the only sound in the document: every scene gets its
//! slice of the master on one stage layer, every other sound is removed, and
//! every character and head symbol gets a silent guide layer carrying the
//! master so its waveform shows while animating inside it. See
//! `docs/MASTER_AUDIO_AND_PROTECT_PLAN.md` Part M (step M2).

use std::collections::{BTreeSet, HashMap};
use std::sync::Arc;

use buzz_scene::{
    Layer, LayerId, LayerKind, LayerStack, LayerTimeline, Object, ObjectKind, Scene, SoundAsset,
    SoundId, SoundRef, SoundSync, Symbol, SymbolId, SymbolInstance,
};

use crate::{Document, SceneRole};

/// The undo label every scene's Enforce step is recorded under.
pub const MASTER_AUDIO_LABEL: &str = "Master Audio mode";

/// Name of the stage layer (non-shot scenes) and of the guide layer inside
/// character and head symbols.
pub const MASTER_AUDIO_LAYER: &str = "Master Audio";

/// A scene's own cut is accepted as a piece of the master only at or above
/// this normalised envelope correlation.
const MIN_CORRELATION: f64 = 0.8;

/// Envelope rate for alignment, in values per second.
const ENVELOPE_HZ: u32 = 100;

/// What Enforce did (or, from [`Document::master_audio_preview`], would do).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct MasterAudioReport {
    /// Keyframe sounds cleared, on stage layers and inside symbols.
    pub sounds_removed: usize,
    /// Layers deleted because removing their sound left them empty, plus
    /// duplicate guide layers.
    pub layers_removed: usize,
    /// Scenes that changed.
    pub scenes_touched: usize,
    /// Character/head symbols whose Master Audio guide layer was added or
    /// brought up to date.
    pub symbols_given_guide: usize,
    /// Per-scene notes (alignment results, unused library sounds removed).
    pub notes: Vec<String>,
}

/// The master clip and where it lives.
struct MasterContext {
    asset: Arc<SoundAsset>,
    scene_index: usize,
    layer: LayerId,
    /// Frame rate of the master's own scene: what shot `audio_in`/`audio_out`
    /// are counted in.
    fps: f64,
}

/// Which stage layer carries the master in a scene, and how it is found.
enum Target {
    /// A shot: its Dialogue layer, made if missing.
    Shot {
        dialogue: Option<LayerId>,
        audio_in: u32,
        audio_out: u32,
    },
    /// The master's own scene: the master layer itself.
    Master(LayerId),
    /// Any other scene: a "Master Audio" layer, made if missing.
    Other,
}

/// The part of the master a scene plays, in seconds into the master.
struct Slice {
    start: f64,
    end: Option<f64>,
    /// Frames the stage layer spans, at least 1.
    frames: u32,
}

/// One scene's planned result.
struct PlannedScene {
    work: Scene,
    sounds_removed: usize,
    layers_removed: usize,
    symbols_given_guide: usize,
    notes: Vec<String>,
}

/// The master clip's envelope, decoded at most once per Enforce.
#[derive(Default)]
struct Aligner {
    master_envelope: Option<Option<Vec<f32>>>,
}

impl Aligner {
    fn master_envelope(&mut self, master: &SoundAsset) -> Option<&[f32]> {
        if self.master_envelope.is_none() {
            self.master_envelope = Some(decode_envelope(master));
        }
        self.master_envelope.as_ref().unwrap().as_deref()
    }
}

/// Is `asset` the master clip (the same bytes, shared or copied)?
fn is_master_clip(asset: &SoundAsset, master: &SoundAsset) -> bool {
    Arc::ptr_eq(&asset.data, &master.data)
        || (asset.data.len() == master.data.len()
            && asset.sample_rate == master.sample_rate
            && asset.channels == master.channels
            && asset.length == master.length
            && asset.data == master.data)
}

fn scene_fps(scene: &Scene) -> f64 {
    let fps = scene.stage().frame_rate;
    if fps.is_finite() && fps > 0.0 { fps } else { 24.0 }
}

/// Does this symbol get a Master Audio guide layer: a character (tag, or
/// filed under `Cast`) or a head (tag, or "head" in its name)?
fn wants_guide(symbol: &Symbol) -> bool {
    symbol
        .tags
        .iter()
        .any(|t| t.eq_ignore_ascii_case("character") || t.eq_ignore_ascii_case("head"))
        || symbol
            .folder
            .as_deref()
            .is_some_and(|f| f == "Cast" || f.starts_with("Cast/"))
        || symbol.name.to_lowercase().contains("head")
}

/// ~100 Hz RMS envelope of a clip's mono mix, or `None` if it can't be decoded.
fn decode_envelope(asset: &SoundAsset) -> Option<Vec<f32>> {
    let clip = buzz_audio::Clip::decode(&asset.data, &asset.name).ok()?;
    let mono = clip.mono();
    let window = (clip.sample_rate / ENVELOPE_HZ).max(1) as usize;
    Some(
        mono.chunks(window)
            .map(|c| (c.iter().map(|x| x * x).sum::<f32>() / c.len() as f32).sqrt())
            .collect(),
    )
}

/// Where `cut` sits inside `master`, by normalised cross-correlation of their
/// envelopes: the best lag (in envelope steps) and its correlation.
fn best_offset(master: &[f32], cut: &[f32]) -> Option<(usize, f64)> {
    let m = cut.len();
    if m < 10 || master.len() < m {
        return None;
    }
    let mean_c = cut.iter().map(|&x| x as f64).sum::<f64>() / m as f64;
    let centred: Vec<f64> = cut.iter().map(|&x| x as f64 - mean_c).collect();
    let norm_c = centred.iter().map(|x| x * x).sum::<f64>().sqrt();
    if norm_c < 1e-9 {
        return None;
    }
    let mut prefix = vec![0.0f64; master.len() + 1];
    let mut prefix_sq = vec![0.0f64; master.len() + 1];
    for (i, &x) in master.iter().enumerate() {
        let x = x as f64;
        prefix[i + 1] = prefix[i] + x;
        prefix_sq[i + 1] = prefix_sq[i] + x * x;
    }
    let mut best: Option<(usize, f64)> = None;
    for lag in 0..=master.len() - m {
        let sum = prefix[lag + m] - prefix[lag];
        let sq = prefix_sq[lag + m] - prefix_sq[lag];
        let var = sq - sum * sum / m as f64;
        if var <= 1e-12 {
            continue;
        }
        // `centred` sums to zero, so its dot with the window equals its dot
        // with the window minus the window's mean.
        let dot: f64 = centred
            .iter()
            .zip(&master[lag..lag + m])
            .map(|(c, &x)| c * x as f64)
            .sum();
        let r = dot / (norm_c * var.sqrt());
        if best.is_none_or(|(_, b)| r > b) {
            best = Some((lag, r));
        }
    }
    best
}

fn format_time(seconds: f64) -> String {
    let s = seconds.max(0.0);
    let minutes = (s / 60.0).floor() as u64;
    format!("{minutes}:{:04.1}", s - minutes as f64 * 60.0)
}

/// Every symbol instance in a layer stack, with the keyframe start it sits
/// on. Hidden layers and hidden objects are left out: only *visible* uses
/// decide where a symbol is first seen.
fn instances_in(stack: &LayerStack) -> Vec<(u32, SymbolInstance)> {
    fn walk(object: &Object, start: u32, out: &mut Vec<(u32, SymbolInstance)>) {
        if !object.visible {
            return;
        }
        match &object.kind {
            ObjectKind::Instance(inst) => out.push((start, inst.clone())),
            ObjectKind::Group(children) => {
                for child in children {
                    walk(child, start, out);
                }
            }
            _ => {}
        }
    }
    let mut out = Vec::new();
    for layer in stack.iter() {
        if !layer.visible || layer.audio_guide {
            continue;
        }
        for keyframe in layer.frames.keyframes() {
            for object in keyframe.objects.iter() {
                walk(object, keyframe.start, &mut out);
            }
        }
    }
    out
}

/// For every symbol shown in the scene: the stage frame of its first visible
/// use, and its **anchor** — the stage frame its own frame 0 corresponds to
/// (`s0 - f0`, nested uses composing). Unused symbols are absent.
fn symbol_anchors(scene: &Scene) -> HashMap<SymbolId, (i64, i64)> {
    let library = scene.library();
    let f0 = |inst: &SymbolInstance| -> i64 {
        match library.get(inst.symbol) {
            Some(s) if s.kind.follows_parent_timeline() => inst.first_frame as i64,
            _ => 0,
        }
    };
    let mut anchors: HashMap<SymbolId, (i64, i64)> = HashMap::new();
    let offer = |anchors
: &mut HashMap<SymbolId, (i64, i64)>, id: SymbolId, visible: i64, anchor: i64| -> bool {
        match anchors.get(&id) {
            Some(&(v, _)) if v <= visible => false,
            _ => {
                anchors.insert(id, (visible, anchor));
                true
            }
        }
    };
    for (start, inst) in instances_in(scene.stage_layers()) {
        let start = start as i64;
        let anchor = if inst.sync_to_root { 0 } else { start - f0(&inst) };
        offer(&mut anchors, inst.symbol, start, anchor);
    }
    let nested: HashMap<SymbolId, Vec<(u32, SymbolInstance)>> = library
        .iter()
        .map(|s| (s.id, instances_in(&s.layers)))
        .collect();
    for _ in 0..=library.len() {
        let mut changed = false;
        let known: Vec<(SymbolId, (i64, i64))> = anchors.iter().map(|(k, v)| (*k, *v)).collect();
        for (parent, (parent_visible, parent_anchor)) in known {
            let Some(children) = nested.get(&parent) else {
                continue;
            };
            for (start, inst) in children {
                let visible = (parent_anchor + *start as i64).max(parent_visible);
                let anchor = if inst.sync_to_root {
                    0
                } else {
                    parent_anchor + *start as i64 - f0(inst)
                };
                changed |= offer(&mut anchors, inst.symbol, visible, anchor);
            }
        }
        if !changed {
            break;
        }
    }
    anchors
}

/// Set a layer's span to exactly `frames` (at least 1).
fn set_length(layer: &mut Layer, frames: u32) {
    let frames = frames.max(1);
    if layer.frames.length() < frames {
        layer.frames.insert_frame(frames - 1);
    }
    while layer.frames.length() > frames {
        if !layer.frames.remove_frame(frames) {
            break;
        }
    }
}

/// Clear every keyframe sound in `stack` that `keep` refuses, returning how
/// many were cleared and which layers lost one.
fn clear_sounds(
    stack: &mut LayerStack,
    mut keep: impl FnMut(&Layer, u32, &SoundRef) -> bool,
) -> (usize, Vec<LayerId>) {
    let mut removed = 0;
    let mut touched = Vec::new();
    let plan: Vec<(LayerId, Vec<u32>)> = stack
        .iter()
        .map(|layer| {
            let starts = layer
                .frames
                .keyframes()
                .iter()
                .filter(|k| k.sound.as_ref().is_some_and(|s| !keep(layer, k.start, s)))
                .map(|k| k.start)
                .collect::<Vec<_>>();
            (layer.id, starts)
        })
        .filter(|(_, starts)| !starts.is_empty())
        .collect();
    for (id, starts) in plan {
        removed += starts.len();
        touched.push(id);
        stack.update(id, |l| {
            for keyframe in l.frames.keyframes_mut() {
                if starts.contains(&keyframe.start) {
                    keyframe.sound = None;
                }
            }
        });
    }
    (removed, touched)
}

/// Do two scenes differ in anything Enforce touches?
fn scenes_differ(a: &Scene, b: &Scene) -> bool {
    fn arcs_differ<'a, T: PartialEq + 'a>(
        a: impl Iterator<Item = &'a Arc<T>>,
        b: impl Iterator<Item = &'a Arc<T>>,
    ) -> bool {
        let a: Vec<&Arc<T>> = a.collect();
        let b: Vec<&Arc<T>> = b.collect();
        a.len() != b.len()
            || a
                .iter()
                .zip(&b)
                .any(|(x, y)| !Arc::ptr_eq(x, y) && ***x != ***y)
    }
    arcs_differ(a.stage_layers().iter(), b.stage_layers().iter())
        || arcs_differ(a.library().iter(), b.library().iter())
        || arcs_differ(a.sounds().iter(), b.sounds().iter())
}

impl Document {
    /// The master clip, read straight from the master scene's stage.
    fn master_context(&self) -> Option<MasterContext> {
        let (scene_key, layer) = self.master_audio?;
        let scene_index = self.index_of_key(scene_key)?;
        let scene = &self.scenes[scene_index].scene;
        let reference = scene
            .stage_layers()
            .get(layer)?
            .frames
            .keyframe_at(0)?
            .sound?;
        let asset = scene.sounds().get(reference.sound)?.clone();
        Some(MasterContext {
            asset,
            scene_index,
            layer,
            fps: scene_fps(scene),
        })
    }

    fn master_target(&self, index: usize, ctx: &MasterContext) -> Target {
        // The master's own scene keeps the master whole, even if it is a shot:
        // trimming it would cut the very clip every other scene is sliced from.
        if index == ctx.scene_index {
            return Target::Master(ctx.layer);
        }
        if let SceneRole::Shot(info) = &self.scenes[index].role {
            return Target::Shot {
                dialogue: info.dialogue_layer,
                audio_in: info.audio_in,
                audio_out: info.audio_out,
            };
        }
        Target::Other
    }

    /// **What Master Audio mode would do**, without changing anything — the
    /// counts for the confirm dialog.
    pub fn master_audio_preview(&self) -> MasterAudioReport {
        let mut report = MasterAudioReport::default();
        let Some(ctx) = self.master_context() else {
            report
                .notes
                .push("No master audio is set (Sequence ▸ Set Master Audio…)".to_string());
            return report;
        };
        let mut aligner = Aligner::default();
        for index in 0..self.scenes.len() {
            let target = self.master_target(index, &ctx);
            let source = &self.scenes[index].scene;
            let planned = plan_scene(index, source, &ctx, target, &mut aligner);
            if scenes_differ(&planned.work, source) {
                report.scenes_touched += 1;
            }
            report.sounds_removed += planned.sounds_removed;
            report.layers_removed += planned.layers_removed;
            report.symbols_given_guide += planned.symbols_given_guide;
            report.notes.extend(planned.notes);
        }
        report
    }

    /// **Enforce Master Audio mode**: give every scene its slice of the master
    /// on one stage layer, remove every other sound (and the layers and
    /// library entries left with nothing), and give every character and head
    /// symbol a silent Master Audio guide layer. One undo step per scene,
    /// labelled [`MASTER_AUDIO_LABEL`]. Running it again changes nothing.
    pub fn enforce_master_audio(&mut self) -> Result<MasterAudioReport, String> {
        let ctx = self
            .master_context()
            .ok_or_else(|| "No master audio is set (Sequence ▸ Set Master Audio…)".to_string())?;
        let mut report = MasterAudioReport::default();
        let mut aligner = Aligner::default();
        for index in 0..self.scenes.len() {
            // A shot's Dialogue layer is brought in line by `retrim_shot`
            // first; its result is folded into this scene's one undo step.
            let mut retrimmed = None;
            if index != ctx.scene_index && matches!(self.scenes[index].role, SceneRole::Shot(_)) {
                let key = self.scenes[index].key;
                let before = self.scenes[index].scene.clone();
                self.retrim_shot(key);
                retrimmed = Some(std::mem::replace(&mut self.scenes[index].scene, before));
            }
            let target = self.master_target(index, &ctx);
            let planned = {
                let source = retrimmed.as_ref().unwrap_or(&self.scenes[index].scene);
                plan_scene(index, source, &ctx, target, &mut aligner)
            };
            report.sounds_removed += planned.sounds_removed;
            report.layers_removed += planned.layers_removed;
            report.symbols_given_guide += planned.symbols_given_guide;
            report.notes.extend(planned.notes);
            if scenes_differ(&planned.work, &self.scenes[index].scene) {
                report.scenes_touched += 1;
                let work = planned.work;
                self.edit_scene_at(index, MASTER_AUDIO_LABEL, move |scene| *scene = work);
            }
        }
        Ok(report)
    }
}

/// Work out one scene's slice of the master.
fn find_slice(
    index: usize,
    scene: &Scene,
    ctx: &MasterContext,
    target: &Target,
    aligner: &mut Aligner,
    notes: &mut Vec<String>,
) -> Slice {
    let fps = scene_fps(scene);
    let master = &ctx.asset;
    let master_seconds = master.duration_seconds();
    match target {
        Target::Shot {
            audio_in,
            audio_out,
            ..
        } => Slice {
            start: *audio_in as f64 / ctx.fps,
            end: Some(*audio_out as f64 / ctx.fps),
            frames: audio_out.saturating_sub(*audio_in).max(1),
        },
        Target::Master(_) => Slice {
            start: 0.0,
            end: None,
            frames: master.duration_frames(fps).max(1),
        },
        Target::Other => {
            let stage = scene.stage_layers();
            let sound_on = |layer: &Layer| {
                layer
                    .frames
                    .keyframes()
                    .iter()
                    .filter_map(|k| k.sound.map(|s| (k.start, s)))
                    .collect::<Vec<_>>()
            };
            // Already carrying the master (a previous Enforce, or placed by
            // hand): its trim is the slice. The "Master Audio" layer first.
            let mut layers: Vec<&Arc<Layer>> = stage.iter().filter(|l| !l.audio_guide).collect();
            layers.sort_by_key(|l| l.name != MASTER_AUDIO_LAYER);
            for layer in &layers {
                for (start, sound) in sound_on(layer) {
                    let Some(asset) = scene.sounds().get(sound.sound) else {
                        continue;
                    };
                    if sound.sync == SoundSync::Stop || !is_master_clip(asset, master) {
                        continue;
                    }
                    // The stored trims are reused as they are, so a second
                    // Enforce writes back exactly what the first one did.
                    let begin = (sound.trim_start as f64 - start as f64 / fps).max(0.0);
                    let end = sound.trim_end.map_or(master_seconds, |e| e as f64);
                    let frames = ((end - begin) * fps).round().max(1.0) as u32;
                    return Slice {
                        start: begin,
                        end: Some(end),
                        frames,
                    };
                }
            }
            // A cut of the master: find where it sits by envelope correlation.
            let mut best: Option<(f64, Slice)> = None;
            for layer in &layers {
                for (start, sound) in sound_on(layer) {
                    if sound.sync == SoundSync::Stop {
                        continue;
                    }
                    let Some(asset) = scene.sounds().get(sound.sound) else {
                        continue;
                    };
                    let Some(cut) = decode_envelope(asset) else {
                        continue;
                    };
                    let Some(master_env) = aligner.master_envelope(master) else {
                        continue;
                    };
                    let Some((lag, r)) = best_offset(master_env, &cut) else {
                        continue;
                    };
                    if r < MIN_CORRELATION || best.as_ref().is_some_and(|(b, _)| *b >= r) {
                        continue;
                    }
                    let lag_s = lag as f64 / ENVELOPE_HZ as f64;
                    let cut_end = sound
                        .trim_end
                        .map_or(asset.duration_seconds(), |e| e as f64);
                    let begin = (lag_s + sound.trim_start as f64 - start as f64 / fps).max(0.0);
                    let end = (lag_s + cut_end).min(master_seconds.max(begin));
                    let frames = ((end - begin) * fps).round().max(1.0) as u32;
                    best = Some((
                        r,
                        Slice {
                            start: begin,
                            end: Some(begin + frames as f64 / fps),
                            frames,
                        },
                    ));
                }
            }
            if let Some((r, slice)) = best {
                notes.push(format!(
                    "Scene {}: aligned to the master at {} (match {:.2})",
                    index + 1,
                    format_time(slice.start),
                    r
                ));
                return slice;
            }
            notes.push(format!(
                "Scene {}: no audio to align — master placed from 0:00",
                index + 1
            ));
            let frames = stage.frame_count().max(1);
            Slice {
                start: 0.0,
                end: Some(frames as f64 / fps),
                frames,
            }
        }
    }
}

/// Plan one scene's Enforce on a copy of it. Nothing in `source` changes.
fn plan_scene(
    index: usize,
    source: &Scene,
    ctx: &MasterContext,
    target: Target,
    aligner: &mut Aligner,
) -> PlannedScene {
    let master = ctx.asset.clone();
    let mut notes = Vec::new();
    let slice = find_slice(index, source, ctx, &target, aligner, &mut notes);
    let fps = scene_fps(source);
    let mut work = source.clone();

    // The stage layer that will carry the master, if it exists already.
    let existing_target = match &target {
        Target::Shot { dialogue, .. } => dialogue.filter(|id| work.stage_layers().get(*id).is_some()),
        Target::Master(id) => Some(*id).filter(|id| work.stage_layers().get(*id).is_some()),
        Target::Other => work
            .stage_layers()
            .iter()
            .find(|l| l.name == MASTER_AUDIO_LAYER && !l.audio_guide && l.kind != LayerKind::Folder)
            .map(|l| l.id),
    };

    // -- 2. Remove every other sound. ---------------------------------------
    let master_ids: BTreeSet<SoundId> = work
        .sounds()
        .iter()
        .filter(|a| is_master_clip(a, &master))
        .map(|a| a.id)
        .collect();

    let mut sounds_removed = 0;
    let mut layers_removed = 0;

    // Stage: only the target's frame-0 master survives (rewritten below).
    let mut stage = work.stage_layers().clone();
    let (removed, touched) = clear_sounds(&mut stage, |layer, start, sound| {
        Some(layer.id) == existing_target && start == 0 && master_ids.contains(&sound.sound)
    });
    sounds_removed += removed;
    let mut empties = Vec::new();
    for id in touched {
        if Some(id) == existing_target {
            continue;
        }
        if let Some(layer) = stage.get(id)
            && layer.kind != LayerKind::Folder
            && layer.is_empty_layer()
            && !stage.is_depended_on(id)
        {
            empties.push(id);
        }
    }
    // Write the cleared sounds back through the scene so it records a change.
    for layer in stage.iter() {
        let current = work.stage_layers().get(layer.id).cloned();
        if current.as_ref().is_some_and(|c| !Arc::ptr_eq(c, layer)) {
            let replacement = (**layer).clone();
            work.update_stage_layer(layer.id, |l| *l = replacement);
        }
    }
    for id in empties {
        if work.remove_stage_layer(id).is_some() {
            layers_removed += 1;
        }
    }

    // Symbols: only master clips on guide layers survive.
    let symbol_ids: Vec<SymbolId> = work.library().iter().map(|s| s.id).collect();
    for id in &symbol_ids {
        let Some(symbol) = work.library().get(*id) else {
            continue;
        };
        let mut stack = symbol.layers.clone();
        let (removed, touched) = clear_sounds(&mut stack, |layer, _, sound| {
            layer.audio_guide && master_ids.contains(&sound.sound)
        });
        if removed == 0 {
            continue;
        }
        sounds_removed += removed;
        for layer_id in touched {
            if stack.len() <= 1 {
                break;
            }
            if let Some(layer) = stack.get(layer_id)
                && layer.kind != LayerKind::Folder
                && layer.is_empty_layer()
                && !stack.is_depended_on(layer_id)
            {
                stack.remove(layer_id);
                layers_removed += 1;
            }
        }
        work.library_mut().update(*id, |s| s.layers = stack);
    }

    // -- 3. One stage layer carrying the master. ---------------------------
    let target_id = match existing_target {
        Some(id) => id,
        None => {
            let name = match target {
                Target::Shot { .. } => "Dialogue",
                _ => MASTER_AUDIO_LAYER,
            };
            work.add_stage_layer(name, LayerKind::Normal)
        }
    };
    let current = work
        .stage_layers()
        .get(target_id)
        .and_then(|l| l.frames.keyframe_at(0).and_then(|k| k.sound));
    let sound_id = match current.filter(|s| master_ids.contains(&s.sound)) {
        Some(s) => s.sound,
        None => match master_ids.iter().next() {
            Some(id) => *id,
            None => work.add_sound(
                &master.name,
                master.data.clone(),
                &master.format,
                master.sample_rate,
                master.channels,
                master.length,
            ),
        },
    };
    let volume = current
        .filter(|s| s.sound == sound_id)
        .map_or(1.0, |s| s.volume);
    let reference = SoundRef {
        sound: sound_id,
        sync: SoundSync::Stream,
        volume,
        loops: 1,
        trim_start: slice.start as f32,
        trim_end: slice.end.map(|e| e as f32),
    };
    let frames = slice.frames;
    work.update_stage_layer(target_id, |l| {
        set_length(l, frames);
        if let Some(keyframe) = l.frames.keyframe_at_mut(0) {
            keyframe.sound = Some(reference);
        }
    });

    // -- 4. Guide layers inside characters and heads. ----------------------
    // Measured from the trim actually written, so a re-run lands on the same
    // `f32` values.
    let slice_start = reference.trim_start as f64;
    let anchors = symbol_anchors(&work);
    let master_seconds = master.duration_seconds();
    let mut symbols_given_guide = 0;
    for id in &symbol_ids {
        let Some(symbol) = work.library().get(*id).cloned() else {
            continue;
        };
        if !wants_guide(&symbol) {
            continue;
        }
        let anchor = anchors.get(id).map_or(0, |&(_, a)| a);
        let trim_start = (slice_start + anchor as f64 / fps)
.clamp(0.0, master_seconds.max(0.0));
        let length = symbol
            .layers
            .iter()
            .filter(|l| !l.audio_guide)
            .map(|l| l.frames.length())
            .max()
            .unwrap_or(1)
            .max(1);
        let trim_end = (trim_start + length as f64 / fps).min(master_seconds.max(trim_start));

        let mut stack = symbol.layers.clone();
        let guides: Vec<LayerId> = stack.iter().filter(|l| l.audio_guide).map(|l| l.id).collect();
        for extra in guides.iter().skip(1) {
            stack.remove(*extra);
            layers_removed += 1;
        }
        let guide_id = match guides.first() {
            Some(id) => *id,
            None => {
                let id = LayerId(work.next_object_id().0);
                let mut layer = Layer::new(id, MASTER_AUDIO_LAYER, LayerKind::Normal);
                layer.color = buzz_scene::layer::default_color(stack.len());
                stack.insert(stack.len(), layer);
                id
            }
        };
        let mut timeline = LayerTimeline::new();
        timeline.insert_frame(length - 1);
        if let Some(keyframe) = timeline.keyframe_at_mut(0) {
            keyframe.sound = Some(SoundRef {
                sound: sound_id,
                sync: SoundSync::Stream,
                volume: 1.0,
                loops: 1,
                trim_start: trim_start as f32,
                trim_end: Some(trim_end as f32),
            });
        }
        stack.update(guide_id, |l| {
            l.name = MASTER_AUDIO_LAYER.to_string();
            l.kind = LayerKind::Normal;
            l.audio_guide = true;
            l.locked = true;
            l.parent = None;
            l.follows = None;
            l.frames = timeline;
        });
        let bottom = stack.len().saturating_sub(1);
        stack.reorder(guide_id, bottom);
        if stack != symbol.layers {
            symbols_given_guide += 1;
            work.library_mut().update(*id, |s| s.layers = stack);
        }
    }

    // -- Unreferenced sounds leave the library. -----------------------------
    let mut used: BTreeSet<SoundId> = BTreeSet::new();
    let stacks = std::iter::once(work.stage_layers().clone())
        .chain(work.library().iter().map(|s| s.layers.clone()))
        .collect::<Vec<_>>();
    for stack in &stacks {
        for layer in stack.iter() {
            for keyframe in layer.frames.keyframes() {
                if let Some(sound) = keyframe.sound {
                    used.insert(sound.sound);
                }
            }
        }
    }
    let unused: Vec<SoundId> = work
        .sounds()
        .iter()
        .map(|a| a.id)
        .filter(|id| !used.contains(id))
        .collect();
    if !unused.is_empty() {
        notes.push(format!(
            "Scene {}: {} unused library sound(s) removed",
            index + 1,
            unused.len()
        ));
        let sounds = work.sounds_mut();
        for id in unused {
            sounds.remove(id);
        }
    }

    PlannedScene {
        work,
        sounds_removed,
        layers_removed,
        symbols_given_guide,
        notes,
    }
}
