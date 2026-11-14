//! Automatic lip sync: turning the soundtrack into mouth keyframes.
//!
//! # How a mouth is animated, and what this automates
//!
//! An animator draws a mouth symbol whose frames are the shapes — closed, `ah`,
//! `oh`, `ee` and so on — then places one instance of it on a layer and sets
//! *which frame it shows* at each moment. That is Animate's model and it is
//! the model here: the instance's `first_frame` selects the shape.
//!
//! So lip sync is not drawing. It is choosing, for each frame of dialogue,
//! which frame of the mouth symbol to show. [`buzz_audio::analyse_visemes`]
//! decides the shapes; this module writes them onto the timeline as keyframes
//! and makes the whole run a single undo step.
//!
//! # It reads the root soundtrack, from wherever you are
//!
//! The dialogue lives on the document's own timeline. The mouth usually lives
//! several symbols down — inside the character, inside its head. Both are
//! true at once, and lip sync has to work across them: the analysis comes from
//! [`buzz_scene::Scene::stage_cues`], which ignores the edit path entirely, so
//! running it while inside the head symbol animates *that* mouth against the
//! *root* dialogue.

use std::sync::Arc;

use buzz_audio::{Clip, LipSyncOptions, Viseme, VisemeTrack, analyse_visemes};
use buzz_geom::Affine;
use buzz_scene::{LayerId, Object, ObjectId, ObjectKind, Scene, SymbolId, SymbolInstance};

/// What a lip-sync run produced.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct LipSyncReport {
    /// Keyframes written.
    pub keyframes: u32,
    /// Frames of dialogue covered.
    pub frames: u32,
    /// How many of those were silent, so the mouth is closed.
    pub silent: u32,
    /// A line for the status bar.
    pub message: String,
}

/// Why lip sync could not run.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum LipSyncError {
    #[error("there is no sound on the main timeline to sync to")]
    NoSound,
    #[error("choose a mouth symbol first")]
    NoMouth,
    #[error(
        "the mouth symbol has {found} frame(s); it needs {needed} — one per shape, \
         in Animate's order: rest, Ai, E, O, U, L, WQ, MBP, FV, etc"
    )]
    MouthTooShort { found: u32, needed: u32 },
    #[error("there is no layer to put the mouth on")]
    NoLayer,
}

/// Analyse the document's soundtrack and write mouth keyframes onto `layer`.
///
/// `mouth` is a symbol whose frames hold the shapes. Every keyframe written
/// holds one instance of it, showing the frame that shape lives on.
pub fn apply(
    scene: &mut Scene,
    clip: &Clip,
    sound_start: u32,
    layer: LayerId,
    mouth: SymbolId,
    placement: Affine,
    options: &LipSyncOptions,
) -> Result<LipSyncReport, LipSyncError> {
    let fps = scene.stage().frame_rate.max(0.01);

    let frames_in_symbol = scene
        .library()
        .get(mouth)
        .ok_or(LipSyncError::NoMouth)?
        .length();
    if frames_in_symbol < Viseme::COUNT {
        return Err(LipSyncError::MouthTooShort {
            found: frames_in_symbol,
            needed: Viseme::COUNT,
        });
    }
    if scene.layers().get(layer).is_none() {
        return Err(LipSyncError::NoLayer);
    }

    let track = analyse_visemes(clip, fps, options);
    if track.is_empty() {
        return Err(LipSyncError::NoSound);
    }

    Ok(write_track(
        scene,
        &track,
        sound_start,
        layer,
        mouth,
        placement,
    ))
}

/// **Write lip sync keys given as Animate visemes** — what a speech recogniser
/// (Rhubarb) hears: `keys` are `(frame from the sound's start, slot of
/// buzz_audio::ANIMATE_VISEMES)`, `length` the frames the dialogue covers.
/// Each slot shows the chart frame `chart` maps it to (Set Visemes), one
/// keyframe per change of frame; what an earlier sync left over the same
/// stretch is cleared first.
pub fn write_viseme_keys(
    scene: &mut Scene,
    keys: &[(u32, usize)],
    length: u32,
    sound_start: u32,
    layer: LayerId,
    mouth: SymbolId,
    placement: Affine,
    chart: &[u32; 12],
) -> LipSyncReport {
    // One key per change of the frame shown: two visemes drawn on the same
    // frame of the chart are one key.
    let mut shown: Vec<(u32, usize, u32)> = Vec::new();
    for (frame, slot) in keys {
        let chart_frame = chart[(*slot).min(11)];
        if shown.last().is_some_and(|(_, _, f)| *f == chart_frame) {
            continue;
        }
        shown.push((*frame, *slot, chart_frame));
    }
    let ids: Vec<ObjectId> = (0..shown.len()).map(|_| scene.next_object_id()).collect();
    let last_frame = sound_start + length;
    let rest_frame = chart[0];
    let silent = keys.iter().filter(|(_, slot)| *slot == 0 || chart[(*slot).min(11)] == rest_frame).count() as u32;
    scene.update_layer(layer, |target| {
        while target.frames.length() <= last_frame {
            target.frames.insert_frame(target.frames.length());
        }
        let stale: Vec<u32> = target
            .frames
            .keyframes()
            .iter()
            .map(|k| k.start)
            .filter(|s| *s > sound_start && *s <= last_frame)
            .collect();
        for start in stale {
            target.frames.clear_keyframe(start);
        }
        for ((frame, slot, chart_frame), id) in shown.iter().zip(&ids) {
            let at = sound_start + frame;
            target.frames.insert_keyframe(at);
            let mut instance = SymbolInstance::new(mouth);
            instance.first_frame = *chart_frame;
            instance.loop_mode = buzz_scene::LoopMode::SingleFrame;
            let object = Object {
                id: *id,
                name: Some(format!("Mouth {}", buzz_audio::ANIMATE_VISEMES[(*slot).min(11)])),
                transform: placement,
                kind: ObjectKind::Instance(instance),
                locked: false,
                visible: true,
                filters: Vec::new(),
                blend: buzz_scene::Blend::Normal,
                spatial: Default::default(),
                pivot: None,
                modifiers: Vec::new(),
                text: None,
                turnaround: Default::default(),
                drawing_object: false,
                brush: None,
            };
            target.frames.set_objects(at, vec![Arc::new(object)]);
        }
    });
    LipSyncReport {
        keyframes: shown.len() as u32,
        frames: length,
        silent,
        message: format!("Lip sync (Rhubarb): {} keyframes over {} frames", shown.len(), length),
    }
}

/// Write an already-analysed track onto a layer.
///
/// Separated from [`apply`] so the analysis can be reviewed — or replaced by a
/// hand-edited track — without redoing the placement, and so tests can drive
/// the timeline half without a decoder.
pub fn write_track(
    scene: &mut Scene,
    track: &VisemeTrack,
    sound_start: u32,
    layer: LayerId,
    mouth: SymbolId,
    placement: Affine,
) -> LipSyncReport {
    // One keyframe per *change* of shape, not one per frame. A keyframe on
    // every frame is unreadable in a timeline and impossible to adjust, and
    // the runs are exactly the held shapes the analysis worked to produce.
    let runs = track.runs();
    let silent = track.frames.iter().filter(|v| v.is_closed()).count() as u32;

    // Ids are allocated before the edit closure, because allocating one needs
    // the scene mutably and the closure already holds it.
    let ids: Vec<ObjectId> = (0..runs.len()).map(|_| scene.next_object_id()).collect();

    let last_frame = sound_start + track.len() as u32;
    // **The chart's own frames**: the visemes set on it (Animate's Set
    // Visemes), so a chart drawn in any order shows the right mouth. A chart
    // with none is taken to be drawn in this program's own order.
    let map = crate::face::stored_viseme_map(scene, mouth);
    scene.update_layer(layer, |target| {
        // The layer has to be long enough to hold the dialogue, or the later
        // keyframes would fall beyond its span and never be seen.
        while target.frames.length() <= last_frame {
            target.frames.insert_frame(target.frames.length());
        }
        // **A second sync replaces the first.** The keys an earlier run left
        // over this stretch are cleared before the new ones go in; added
        // between them instead, the old shapes kept showing through.
        let stale: Vec<u32> = target
            .frames
            .keyframes()
            .iter()
            .map(|k| k.start)
            .filter(|s| *s > sound_start && *s <= last_frame)
            .collect();
        for start in stale {
            target.frames.clear_keyframe(start);
        }

        for ((start, viseme, _), id) in runs.iter().zip(&ids) {
            let frame = sound_start + start;
            target.frames.insert_keyframe(frame);

            let mut instance = SymbolInstance::new(mouth);
            // The viseme *is* the frame of the mouth symbol. A graphic symbol
            // in single-frame mode shows exactly that frame and nothing else,
            // which is what a mouth shape is.
            instance.first_frame = map.map_or(viseme.frame(), |m| m[viseme.frame() as usize]);
            instance.loop_mode = buzz_scene::LoopMode::SingleFrame;

            let object = Object {
                id: *id,
                name: Some(format!("Mouth {}", viseme.label())),
                transform: placement,
                kind: ObjectKind::Instance(instance),
                locked: false,
                visible: true,
                filters: Vec::new(),
                blend: buzz_scene::Blend::Normal,
                spatial: Default::default(),
                pivot: None,
                modifiers: Vec::new(),
                text: None,
                turnaround: Default::default(),
                drawing_object: false,
                brush: None,
            };
            target.frames.set_objects(frame, vec![Arc::new(object)]);
        }
    });

    LipSyncReport {
        keyframes: runs.len() as u32,
        frames: track.len() as u32,
        silent,
        message: format!(
            "Lip sync: {} keyframes over {} frames ({} silent)",
            runs.len(),
            track.len(),
            silent
        ),
    }
}

/// Build a mouth symbol whose frames are the ten shapes, as a starting point.
///
/// A user with a mouth drawn for Animate imports it and uses that. A user
/// starting from nothing needs *something* on every frame, or lip sync has
/// nothing to show — so this makes a symbol of the right length with a labelled
/// placeholder on each frame, which can then be drawn over shape by shape.
pub fn placeholder_mouth(scene: &mut Scene, name: &str) -> SymbolId {
    let symbol = scene.add_symbol(name, buzz_scene::SymbolKind::Graphic, None);
    fill_placeholder_mouth(scene, symbol);
    symbol
}

/// The same shapes, drawn into a symbol that already exists.
///
/// Split out for [`crate::puppet`], which wants the mouth filed in the Cast
/// folder beside the eyes and the brows rather than at the root of the library
/// -- and, more to the point, wants to make it *once* for a whole cast. A
/// symbol is made by whoever is going to name and file it; this only draws.
pub fn fill_placeholder_mouth(scene: &mut Scene, symbol: SymbolId) {
    use buzz_geom::Shape as _;
    use buzz_scene::ShapeData;
    use peniko::Color;

    let Some(layer) = scene
        .library()
        .get(symbol)
        .and_then(|s| s.layers.iter().next())
        .map(|l| l.id)
    else {
        return;
    };

    // Openness stands in for the shape: a closed line for the closed mouths, a
    // wide ellipse for the open ones. It reads as a mouth at a glance, which
    // is what makes the timing reviewable before any drawing is done.
    let shapes: Vec<(Viseme, f64, f64)> = vec![
        (Viseme::Rest, 26.0, 3.0),
        (Viseme::Ai, 30.0, 26.0),
        (Viseme::E, 30.0, 14.0),
        (Viseme::O, 20.0, 22.0),
        (Viseme::U, 14.0, 14.0),
        (Viseme::L, 26.0, 18.0),
        (Viseme::WQ, 16.0, 12.0),
        (Viseme::MBP, 26.0, 4.0),
        (Viseme::FV, 24.0, 8.0),
        (Viseme::Etc, 26.0, 12.0),
    ];

    let ids: Vec<ObjectId> = (0..shapes.len()).map(|_| scene.next_object_id()).collect();

    scene.library_mut().update(symbol, |s| {
        s.layers.update(layer, |l| {
            for ((viseme, width, height), id) in shapes.iter().zip(&ids) {
                let frame = viseme.frame();
                while l.frames.length() <= frame {
                    l.frames.insert_frame(l.frames.length());
                }
                l.frames.insert_keyframe(frame);

                let mouth =
                    kurbo::Ellipse::new(buzz_geom::Point::ZERO, (width / 2.0, height / 2.0), 0.0)
                        .to_path(0.05);
                let object = Object {
                    id: *id,
                    name: Some(viseme.label().to_string()),
                    transform: Affine::IDENTITY,
                    kind: ObjectKind::Shape(ShapeData::filled(
                        mouth,
                        Color::from_rgb8(0x20, 0x20, 0x20),
                    )),
                    locked: false,
                    visible: true,
                    filters: Vec::new(),
                    blend: buzz_scene::Blend::Normal,
                    spatial: Default::default(),
                    pivot: None,
                    modifiers: Vec::new(),
                    text: None,
                    turnaround: Default::default(),
                    drawing_object: false,
                    brush: None,
                };
                l.frames.set_objects(frame, vec![Arc::new(object)]);
            }
        });
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use buzz_scene::{LayerKind, SoundRef, SymbolKind};

    fn dialogue_clip() -> Clip {
        // Speech-shaped: a burst, a pause, a burst.
        let rate = 44_100;
        let mut samples = Vec::new();
        let mut push = |seconds: f64, hz: f64, amplitude: f32| {
            let frames = (seconds * rate as f64) as usize;
            for i in 0..frames {
                let t = i as f64 / rate as f64;
                samples.push((t * hz * std::f64::consts::TAU).sin() as f32 * amplitude);
            }
        };
        push(0.4, 600.0, 0.7);
        push(0.3, 0.0, 0.0);
        push(0.4, 1_500.0, 0.7);

        Clip::new("Line", rate, 1, samples).expect("a clip")
    }

    fn document() -> (Scene, LayerId, SymbolId) {
        let mut scene = Scene::default();
        let layer = scene.add_layer("Mouth", LayerKind::Normal);
        let mouth = placeholder_mouth(&mut scene, "Mouth");
        (scene, layer, mouth)
    }

    #[test]
    fn a_placeholder_mouth_has_a_frame_for_every_shape() {
        let (scene, _, mouth) = document();
        let symbol = scene.library().get(mouth).expect("the mouth");
        assert!(
            symbol.length() >= Viseme::COUNT,
            "the mouth is {} frames, needs {}",
            symbol.length(),
            Viseme::COUNT
        );
    }

    #[test]
    fn lip_sync_writes_keyframes_showing_mouth_shapes() {
        let (mut scene, layer, mouth) = document();
        let clip = dialogue_clip();

        let report = apply(
            &mut scene,
            &clip,
            0,
            layer,
            mouth,
            Affine::translate((100.0, 100.0)),
            &LipSyncOptions::default(),
        )
        .expect("lip sync should run");

        assert!(report.keyframes > 2, "expected several shapes: {report:?}");
        assert!(report.silent > 0, "the pause should close the mouth");

        // Every keyframe holds one instance of the mouth, showing a frame.
        let target = scene.layers().get(layer).expect("the layer").clone();
        let mut shapes_seen = std::collections::BTreeSet::new();
        for keyframe in target.frames.keyframes() {
            for object in keyframe.objects.iter() {
                let ObjectKind::Instance(instance) = &object.kind else {
                    panic!("expected an instance, found {:?}", object.kind);
                };
                assert_eq!(instance.symbol, mouth);
                assert_eq!(instance.loop_mode, buzz_scene::LoopMode::SingleFrame);
                assert!(instance.first_frame < Viseme::COUNT);
                shapes_seen.insert(instance.first_frame);
            }
        }
        assert!(
            shapes_seen.len() > 1,
            "the mouth should change shape, saw {shapes_seen:?}"
        );
    }

    /// Keyframes go where the sound is, not at frame zero: dialogue starting
    /// on frame 30 must animate the mouth from frame 30.
    #[test]
    fn keyframes_start_where_the_sound_does() {
        let (mut scene, layer, mouth) = document();
        let clip = dialogue_clip();

        apply(
            &mut scene,
            &clip,
            30,
            layer,
            mouth,
            Affine::IDENTITY,
            &LipSyncOptions::default(),
        )
        .expect("lip sync");

        let target = scene.layers().get(layer).expect("the layer").clone();
        let first_with_mouth = target
            .frames
            .keyframes()
            .iter()
            .find(|k| !k.objects.is_empty())
            .expect("a keyframe with a mouth");
        assert!(
            first_with_mouth.start >= 30,
            "the mouth started at frame {} rather than 30",
            first_with_mouth.start
        );
    }

    #[test]
    fn the_layer_is_extended_to_hold_the_dialogue() {
        let (mut scene, layer, mouth) = document();
        let clip = dialogue_clip();
        let before = scene.layers().get(layer).expect("the layer").length();

        apply(
            &mut scene,
            &clip,
            0,
            layer,
            mouth,
            Affine::IDENTITY,
            &LipSyncOptions::default(),
        )
        .expect("lip sync");

        let after = scene.layers().get(layer).expect("the layer").length();
        assert!(after > before, "the layer should have been extended");
        assert!(
            after >= clip.duration_frames(scene.stage().frame_rate),
            "the layer is shorter than the dialogue"
        );
    }

    /// A mouth symbol with too few frames cannot show every shape, and the
    /// user needs to be told which and how many — not left with a mouth that
    /// silently shows the wrong thing.
    #[test]
    fn a_mouth_symbol_that_is_too_short_is_refused_with_the_numbers() {
        let mut scene = Scene::default();
        let layer = scene.add_layer("Mouth", LayerKind::Normal);
        let short = scene.add_symbol("Short", SymbolKind::Graphic, None);

        let error = apply(
            &mut scene,
            &dialogue_clip(),
            0,
            layer,
            short,
            Affine::IDENTITY,
            &LipSyncOptions::default(),
        )
        .expect_err("should be refused");

        match error {
            LipSyncError::MouthTooShort { found, needed } => {
                assert_eq!(needed, Viseme::COUNT);
                assert!(found < needed);
            }
            other => panic!("expected MouthTooShort, got {other:?}"),
        }
        assert!(error.to_string().contains("Ai"), "{error}");
    }

    /// **The arrangement the whole feature is for**: dialogue on the root,
    /// the mouth inside a character's head symbol, and lip sync run from in
    /// there.
    #[test]
    fn lip_sync_runs_inside_a_nested_symbol_against_the_root_dialogue() {
        let mut scene = Scene::default();

        // Dialogue on the root timeline.
        let sound = scene.add_sound("Line", Arc::new(vec![0; 4]), "wav", 44_100, 1, 44_100);
        let audio_layer = scene.add_layer("Audio", LayerKind::Normal);
        scene.set_frame_sound(audio_layer, 0, Some(SoundRef::stream(sound)));

        // A character containing a head, with the mouth layer inside the head.
        let mouth_symbol = placeholder_mouth(&mut scene, "Mouth");
        let head = scene.add_symbol("Head", SymbolKind::MovieClip, None);
        let character = scene.add_symbol("Character", SymbolKind::MovieClip, None);

        scene.enter_symbol(character);
        scene.enter_symbol(head);
        assert_eq!(scene.edit_path().len(), 2, "two symbols deep");

        // The mouth layer is the one open for editing, inside the head.
        let inner_layer = scene.layers().iter().next().expect("a layer").id;

        // The soundtrack is still the root's, from in here.
        assert_eq!(
            scene.stage_cues().len(),
            1,
            "the root dialogue is still cued"
        );

        let report = apply(
            &mut scene,
            &dialogue_clip(),
            0,
            inner_layer,
            mouth_symbol,
            Affine::IDENTITY,
            &LipSyncOptions::default(),
        )
        .expect("lip sync should run inside the head symbol");

        assert!(report.keyframes > 1);

        // The keyframes landed inside the head symbol, not on the root.
        let inside = scene.layers().get(inner_layer).expect("the layer").clone();
        assert!(
            inside
                .frames
                .keyframes()
                .iter()
                .any(|k| !k.objects.is_empty()),
            "the mouth should have been animated inside the head"
        );

        scene.edit_document();
        let root_layer = scene.layers().iter().next().expect("a root layer");
        assert!(
            root_layer
                .frames
                .keyframes()
                .iter()
                .all(|k| k.objects.is_empty()),
            "the root timeline should not have been touched"
        );
    }

    #[test]
    fn a_track_of_one_shape_writes_one_keyframe() {
        let (mut scene, layer, mouth) = document();
        let track = VisemeTrack {
            frames: vec![Viseme::Rest; 10],
            fps: 24.0,
        };

        let report = write_track(&mut scene, &track, 0, layer, mouth, Affine::IDENTITY);
        assert_eq!(report.keyframes, 1, "one run means one keyframe");
        assert_eq!(report.silent, 10);
    }

    /// **Set Visemes is obeyed**: with the animator's Animate mapping on the
    /// chart (Neutral on frame 4, Ah on 1, M on 5 — 1-based), the keys show
    /// those frames, not this program's own order.
    #[test]
    fn keys_land_on_the_frames_the_visemes_were_set_to() {
        let (mut scene, layer, mouth) = document();
        let mine = [4u32, 1, 7, 7, 3, 1, 5, 2, 2, 7, 9, 8].map(|f| f - 1);
        crate::face::set_animate_chart(&mut scene, mouth, mine);
        let track = VisemeTrack {
            frames: vec![Viseme::Rest, Viseme::Rest, Viseme::Ai, Viseme::Ai, Viseme::MBP, Viseme::MBP],
            fps: 24.0,
        };
        write_track(&mut scene, &track, 0, layer, mouth, Affine::IDENTITY);
        let shown = |frame: u32| {
            scene.layers().get(layer).and_then(|l| l.objects_at(frame).first().and_then(|o| o.instance()).map(|i| i.first_frame))
        };
        assert_eq!(shown(0), Some(3), "Neutral: the chart's frame 4");
        assert_eq!(shown(2), Some(0), "Ah: frame 1");
        assert_eq!(shown(4), Some(4), "M: frame 5");
        // Synced again with one shape: the earlier keys over the span are gone.
        let again = VisemeTrack { frames: vec![Viseme::Rest; 6], fps: 24.0 };
        write_track(&mut scene, &again, 0, layer, mouth, Affine::IDENTITY);
        let keys = scene.layers().get(layer).map(|l| l.frames.keyframes().iter().filter(|k| k.start <= 6).count()).unwrap_or(0);
        assert_eq!(keys, 1, "a second sync replaces the first, it does not add to it");
    }
}
