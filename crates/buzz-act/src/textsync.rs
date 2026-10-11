//! **Lip sync from the words, for when there is no recording yet.**
//!
//! The audio lip sync ([`crate::lipsync`]) turns a soundtrack into mouth
//! shapes. A script has no soundtrack, but it has something a mouth can follow
//! well enough to be reviewed against: the letters. Romanised Hindi and English
//! are both spelled close enough to how they sound that vowel and lip-closing
//! letters give a plausible mouth, and a plausible mouth is exactly what a
//! rough cut needs — it shows who is talking, and when, and it is replaced by
//! the real thing the day the voices arrive.
//!
//! # The mapping
//!
//! | Letters | Shape |
//! |---|---|
//! | `a`, `aa` | [`Viseme::Ai`] |
//! | `e`, `i`, `ee` | [`Viseme::E`] |
//! | `o`, `au` | [`Viseme::O`] |
//! | `u`, `oo`, `w` | [`Viseme::U`] |
//! | `m`, `b`, `p` | [`Viseme::MBP`] |
//! | `f`, `v` | [`Viseme::FV`] |
//! | any other consonant | a short [`Viseme::E`], which is a half-open mouth |
//! | a space | a short [`Viseme::Rest`] |
//! | punctuation | a longer [`Viseme::Rest`] |
//!
//! A consonant next to a vowel disappears into it (the two are the same shape
//! once neighbours are merged), so what is left is what the eye can follow: the
//! open vowels, the closed lips, and the pauses.
//!
//! # Held, not flickered
//!
//! Every shape lasts at least **two frames**, because below that a mouth reads
//! as a flicker rather than as speech (the audio lip sync's `hold` is the same
//! number for the same reason). When a line has more shapes than it has frames
//! for, the least important are dropped first: the short consonants and the
//! word gaps. Shape lengths are even, so the shapes change on twos, which is
//! where the performance underneath them writes its own keys.
//!
//! # Writing it
//!
//! [`line_track`] lays a scene's lines out on one [`VisemeTrack`] — silent
//! wherever nobody is speaking — and the track is written by the same function
//! the audio lip sync uses, [`crate::lipsync::write_track`], for a character
//! whose mouth is on a layer of its own (the built-in puppet), or by
//! [`write_rig_mouth`] for a character whose mouth is a part of its armature
//! (the rig the image pipeline makes), which keys the part's frame on the rig's
//! own keyframes so that the mouth moves with the head.

use std::sync::Arc;

use buzz_audio::{Viseme, VisemeTrack};
use buzz_scene::{ObjectId, ObjectKind, Scene};

use crate::lipsync::LipSyncReport;

/// One piece of a token stream: a shape and how much of the line it wants.
type Token = (Viseme, f64);

/// The weight of a short consonant. Small, so it is the first thing dropped.
const CONSONANT: f64 = 0.45;
/// A vowel.
const VOWEL: f64 = 1.0;
/// A long vowel: `aa`, `ee`, `oo`.
const LONG_VOWEL: f64 = 1.6;
/// Lips pressed, or a lip on the teeth: a beat.
const CLOSURE: f64 = 0.8;
/// The gap between words.
const WORD_GAP: f64 = 0.3;
/// After a comma.
const COMMA: f64 = 0.8;
/// After a full stop, `?` or `!`.
const STOP: f64 = 1.2;
/// After `...`, `…` or a dash: a hesitation.
const HESITATION: f64 = 1.6;

fn lower(c: char) -> char {
    c.to_lowercase().next().unwrap_or(c)
}

/// The shapes of a line, in order, each with how much of the line it wants.
///
/// Adjacent shapes that are the same are one shape, and the line always ends on
/// a closed mouth.
fn tokens(text: &str) -> Vec<Token> {
    let chars: Vec<char> = text.chars().collect();
    let mut out: Vec<Token> = Vec::new();
    let mut i = 0usize;

    let push = |out: &mut Vec<Token>, shape: Viseme, weight: f64| match out.last_mut() {
        Some((last, w)) if *last == shape => *w += weight,
        _ => out.push((shape, weight)),
    };

    while i < chars.len() {
        let c = lower(chars[i]);
        let next = chars.get(i + 1).map(|n| lower(*n));

        if c.is_alphabetic() {
            match (c, next) {
                ('a', Some('a')) => {
                    push(&mut out, Viseme::Ai, LONG_VOWEL);
                    i += 2;
                    continue;
                }
                ('e', Some('e')) | ('i', Some('i')) => {
                    push(&mut out, Viseme::E, LONG_VOWEL);
                    i += 2;
                    continue;
                }
                ('o', Some('o')) => {
                    push(&mut out, Viseme::U, LONG_VOWEL);
                    i += 2;
                    continue;
                }
                ('a', Some('u')) => {
                    push(&mut out, Viseme::O, LONG_VOWEL * 0.8);
                    i += 2;
                    continue;
                }
                _ => {}
            }

            let (shape, weight) = match c {
                'a' => (Viseme::Ai, VOWEL),
                'e' | 'i' => (Viseme::E, VOWEL),
                'o' => (Viseme::O, VOWEL),
                'u' => (Viseme::U, VOWEL),
                'w' => (Viseme::U, CONSONANT + 0.15),
                'm' | 'b' | 'p' => (Viseme::MBP, CLOSURE),
                'f' | 'v' => (Viseme::FV, CLOSURE),
                _ => (Viseme::E, CONSONANT),
            };
            push(&mut out, shape, weight);
            i += 1;
            // The `h` of `kh`, `gh`, `bh`, `ch`, `sh`: aspiration, not a shape.
            if next == Some('h') && !matches!(c, 'a' | 'e' | 'i' | 'o' | 'u') {
                i += 1;
            }
        } else if c.is_ascii_digit() {
            push(&mut out, Viseme::E, CONSONANT);
            i += 1;
        } else if c.is_whitespace() {
            push(&mut out, Viseme::Rest, WORD_GAP);
            i += 1;
        } else {
            // Punctuation. A run of it is one pause, as long as its longest.
            let mut weight = match c {
                ',' | ';' | ':' => COMMA,
                '.' | '!' | '?' => STOP,
                '\u{2026}' | '\u{2014}' | '\u{2013}' => HESITATION,
                _ => 0.0,
            };
            let mut j = i + 1;
            while j < chars.len() && !chars[j].is_alphanumeric() && !chars[j].is_whitespace() {
                weight = weight.max(match chars[j] {
                    ',' | ';' | ':' => COMMA,
                    '.' | '!' | '?' => STOP,
                    '\u{2026}' | '\u{2014}' | '\u{2013}' => HESITATION,
                    _ => 0.0,
                });
                j += 1;
            }
            // `...` reads as a hesitation however the dots are typed.
            if chars[i..j].iter().filter(|c| **c == '.').count() >= 3 {
                weight = weight.max(HESITATION);
            }
            if weight > 0.0 {
                push(&mut out, Viseme::Rest, weight);
            }
            i = j;
        }
    }

    // A line starts speaking, it does not start closed.
    while matches!(out.first(), Some((Viseme::Rest, _))) {
        out.remove(0);
    }
    // And it ends closed.
    match out.last_mut() {
        Some((Viseme::Rest, w)) => *w = w.max(STOP),
        Some(_) => out.push((Viseme::Rest, STOP)),
        None => {}
    }
    out
}

/// Merge neighbours that have become the same shape.
fn merge_same(tokens: &mut Vec<Token>) {
    let mut merged: Vec<Token> = Vec::with_capacity(tokens.len());
    for (shape, weight) in tokens.drain(..) {
        match merged.last_mut() {
            Some((last, w)) if *last == shape => *w += weight,
            _ => merged.push((shape, weight)),
        }
    }
    *tokens = merged;
}

/// **The mouth shapes for a line of words, one per frame.**
///
/// `frames` is how long the line is on the timeline; the answer is exactly that
/// long. Every shape in it lasts at least two frames and the shapes change on
/// even frames, and it ends on [`Viseme::Rest`] whenever it has room to.
///
/// A line with no letters in it, or too short to hold a shape, is all
/// [`Viseme::Rest`]: a closed mouth is the honest answer when there is nothing
/// to say it with.
pub fn visemes_from_text(text: &str, frames: u32) -> Vec<Viseme> {
    let frames = frames as usize;
    if frames == 0 {
        return Vec::new();
    }
    let mut shapes = tokens(text);
    if shapes.is_empty() || frames < 2 {
        return vec![Viseme::Rest; frames];
    }
    // Two or three frames hold one shape: the one the line leans on most. The
    // track closes the mouth the moment the line is over.
    if frames < 4 {
        let heaviest = shapes
            .iter()
            .filter(|(shape, _)| *shape != Viseme::Rest)
            .max_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal))
            .map(|(shape, _)| *shape)
            .unwrap_or(Viseme::Rest);
        return vec![heaviest; frames];
    }

    // Too many shapes for the frames: drop the least important until each can
    // have its two. Never the closing rest.
    while shapes.len() * 2 > frames && shapes.len() > 1 {
        let last = shapes.len() - 1;
        let Some((drop, _)) = shapes
            .iter()
            .enumerate()
            .filter(|(i, _)| *i != last)
            .min_by(|a, b| a.1.1.partial_cmp(&b.1.1).unwrap_or(std::cmp::Ordering::Equal))
        else {
            break;
        };
        shapes.remove(drop);
        merge_same(&mut shapes);
    }
    // A closing rest that has been merged away would leave the mouth open.
    if shapes.len() == 1 && shapes[0].0 != Viseme::Rest && frames >= 4 {
        shapes.push((Viseme::Rest, STOP));
    }

    // Two frames each, and the rest by weight in pairs, so every change of shape
    // falls on an even frame.
    let count = shapes.len();
    let spare = frames.saturating_sub(count * 2);
    let pairs = spare / 2;
    let total_weight: f64 = shapes.iter().map(|(_, w)| *w).sum::<f64>().max(1e-9);

    let mut lengths: Vec<usize> = vec![2; count];
    let mut remainders: Vec<(usize, f64)> = Vec::with_capacity(count);
    let mut given = 0usize;
    for (i, (_, w)) in shapes.iter().enumerate() {
        let exact = pairs as f64 * w / total_weight;
        let whole = exact.floor() as usize;
        lengths[i] += whole * 2;
        given += whole;
        remainders.push((i, exact - whole as f64));
    }
    remainders.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    for (i, _) in remainders.into_iter().take(pairs.saturating_sub(given)) {
        lengths[i] += 2;
    }
    // An odd frame goes on the end, where it is the closing rest's.
    let used: usize = lengths.iter().sum();
    if frames > used {
        *lengths.last_mut().expect("at least one shape") += frames - used;
    } else if frames < used {
        // Fewer frames than a single shape wants: the shapes that fit.
        let mut over = used - frames;
        for len in lengths.iter_mut().rev() {
            let take = over.min(len.saturating_sub(1));
            *len -= take;
            over -= take;
            if over == 0 {
                break;
            }
        }
    }

    let mut out = Vec::with_capacity(frames);
    for ((shape, _), len) in shapes.iter().zip(&lengths) {
        out.extend(std::iter::repeat_n(*shape, *len));
    }
    out.truncate(frames);
    while out.len() < frames {
        out.push(Viseme::Rest);
    }
    out
}

/// One line to be mouthed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SpokenLine<'a> {
    /// The first frame of the line on the scene's timeline.
    pub start: u32,
    pub frames: u32,
    pub text: &'a str,
}

/// **One character's mouth for a whole scene**: the shapes of each of their
/// lines where they are spoken, and a closed mouth everywhere else.
///
/// A closed mouth rather than nothing, because a mouth that is not written is a
/// mouth that is not there: the track starts on frame zero with [`Viseme::Rest`]
/// so the character has a mouth before they say anything.
pub fn line_track(total_frames: u32, fps: f64, lines: &[SpokenLine<'_>]) -> VisemeTrack {
    let mut frames = vec![Viseme::Rest; total_frames as usize];
    for line in lines {
        let shapes = visemes_from_text(line.text, line.frames);
        for (offset, shape) in shapes.into_iter().enumerate() {
            if let Some(slot) = frames.get_mut(line.start as usize + offset) {
                *slot = shape;
            }
        }
    }
    VisemeTrack { frames, fps }
}

// -- a mouth that is part of an armature ----------------------------------------------

/// Why a rig's mouth could not be written.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum RigMouthError {
    #[error("there is no such object")]
    NoObject,
    #[error("that object is not a rig")]
    NotRigged,
    #[error("the rig has no mouth part with a frame for every shape")]
    NoMouth,
}

/// Which part of this rig is its mouth: a rigid or skinned instance of a
/// symbol with at least one frame for every shape. Preferring one whose symbol
/// is called a mouth.
pub fn find_rig_mouth(scene: &Scene, object: ObjectId) -> Option<usize> {
    let (_, found) = scene.find_object(object)?;
    let ObjectKind::Armature(rig) = &found.kind else {
        return None;
    };
    let mut fallback = None;
    for (index, part) in rig.parts.iter().enumerate() {
        let ObjectKind::Instance(instance) = &part.artwork.kind else {
            continue;
        };
        let Some(symbol) = scene.library().get(instance.symbol) else {
            continue;
        };
        if symbol.length() < Viseme::COUNT {
            continue;
        }
        if symbol.name.to_lowercase().contains("mouth") {
            return Some(index);
        }
        fallback.get_or_insert(index);
    }
    fallback
}

/// **Write a track onto a rig's mouth part.**
///
/// The mouth of an image-made rig is not an object on a layer: it is a part of
/// the armature, bound to the head, so that it turns when the head does. Its
/// shape is the part's `first_frame`, in single-frame mode, exactly as a mouth
/// instance on a layer would hold it, and it is keyed the way anything on a rig
/// is keyed: on the object's own keyframes. A key at every change of shape, and
/// the shape for its frame on every other key the rig has.
///
/// Call it **after** the performance is written. A key made here at a frame the
/// performance has no key on would carry the pose of the key before it, and the
/// tween would then run from that stale pose; the shapes from
/// [`visemes_from_text`] change on even frames, which are the performance's own
/// key frames, so in practice every key this writes lands on one that is
/// already there.
pub fn write_rig_mouth(
    scene: &mut Scene,
    object: ObjectId,
    track: &VisemeTrack,
    start: u32,
) -> Result<LipSyncReport, RigMouthError> {
    let (layer, found) = scene.find_object(object).ok_or(RigMouthError::NoObject)?;
    if !matches!(found.kind, ObjectKind::Armature(_)) {
        return Err(RigMouthError::NotRigged);
    }
    let part = find_rig_mouth(scene, object).ok_or(RigMouthError::NoMouth)?;

    let runs = track.runs();
    let silent = track.frames.iter().filter(|v| v.is_closed()).count() as u32;

    let last = start + track.len() as u32;
    scene.update_layer(layer, |l| {
        while l.frames.length() <= last {
            l.frames.insert_frame(l.frames.length());
        }
    });

    // A key at every change of shape.
    for (first, _, _) in &runs {
        scene.ensure_keyframe(layer, start + first);
    }

    // **And the right shape on every key the rig has**, not only on those. A
    // rig's key holds the whole armature, mouth part included, so a performance
    // key between two mouth keys carries a copy of the part as it was before
    // any of this: in loop mode, running through every shape. Every key on the
    // layer is told what the mouth is doing at its frame, and a key outside the
    // track is a closed mouth.
    let keys: Vec<u32> = scene
        .layers()
        .get(layer)
        .map(|l| {
            l.frames
                .keyframes()
                .iter()
                .filter(|k| k.objects.iter().any(|o| o.id == object))
                .map(|k| k.start)
                .collect()
        })
        .unwrap_or_default();
    for frame in keys {
        let shape = match frame.checked_sub(start) {
            Some(offset) => track.at(offset),
            None => Viseme::Rest,
        };
        let frame_of_shape = shape.frame();
        scene.update_object_at(frame, object, |target| {
            let ObjectKind::Armature(rig) = &mut target.kind else {
                return;
            };
            let Some(slot) = rig.parts.get_mut(part) else {
                return;
            };
            let mut artwork = (*slot.artwork).clone();
            if let ObjectKind::Instance(instance) = &mut artwork.kind {
                instance.first_frame = frame_of_shape;
                instance.loop_mode = buzz_scene::LoopMode::SingleFrame;
            }
            slot.artwork = Arc::new(artwork);
        });
    }

    Ok(LipSyncReport {
        keyframes: runs.len() as u32,
        frames: track.len() as u32,
        silent,
        message: format!(
            "Rig mouth: {} keys over {} frames ({} silent)",
            runs.len(),
            track.len(),
            silent
        ),
    })
}

// -- a mouth that is a part of a layer rig ---------------------------------------------

/// Which part of this **layer rig** is its mouth: the part, on its own layer,
/// that is an instance of a symbol with a frame for every shape — preferring
/// one whose symbol is called a mouth. [`find_rig_mouth`] for a rig made by
/// layer parenting.
pub fn find_layer_rig_mouth(scene: &Scene, rig: &crate::layer_rig::LayerRigView) -> Option<ObjectId> {
    let mut fallback = None;
    for &(_, _, id) in &rig.parts {
        let Some((_, object)) = scene.find_object(id) else {
            continue;
        };
        let ObjectKind::Instance(instance) = &object.kind else {
            continue;
        };
        let Some(symbol) = scene.library().get(instance.symbol) else {
            continue;
        };
        if symbol.length() < Viseme::COUNT {
            continue;
        }
        if symbol.name.to_lowercase().contains("mouth") {
            return Some(id);
        }
        fallback.get_or_insert(id);
    }
    fallback
}

/// [`write_rig_mouth`] for a layer rig's mouth part (`object`,
/// [`find_layer_rig_mouth`]): a key on its layer at every change of shape —
/// holding the tweened pose there ([`crate::layer_rig::key_at`]), so the mouth
/// keeps moving with the head — and the shape for its frame on every key the
/// layer has. Call it after the performance, as with the rig's.
pub fn write_layer_rig_mouth(
    scene: &mut Scene,
    object: ObjectId,
    track: &VisemeTrack,
    start: u32,
) -> Result<LipSyncReport, RigMouthError> {
    let (layer, found) = scene.find_object(object).ok_or(RigMouthError::NoObject)?;
    if !matches!(found.kind, ObjectKind::Instance(_)) {
        return Err(RigMouthError::NoMouth);
    }

    let runs = track.runs();
    let silent = track.frames.iter().filter(|v| v.is_closed()).count() as u32;

    let last = start + track.len() as u32;
    scene.update_layer(layer, |l| {
        while l.frames.length() <= last {
            l.frames.insert_frame(l.frames.length());
        }
    });
    for (first, _, _) in &runs {
        crate::layer_rig::key_at(scene, layer, start + first);
    }

    let keys: Vec<u32> = scene
        .layers()
        .get(layer)
        .map(|l| {
            l.frames
                .keyframes()
                .iter()
                .filter(|k| k.objects.iter().any(|o| o.id == object))
                .map(|k| k.start)
                .collect()
        })
        .unwrap_or_default();
    for frame in keys {
        let shape = match frame.checked_sub(start) {
            Some(offset) => track.at(offset),
            None => Viseme::Rest,
        };
        let frame_of_shape = shape.frame();
        scene.update_object_at(frame, object, |target| {
            if let ObjectKind::Instance(instance) = &mut target.kind {
                instance.first_frame = frame_of_shape;
                instance.loop_mode = buzz_scene::LoopMode::SingleFrame;
            }
        });
    }

    Ok(LipSyncReport {
        keyframes: runs.len() as u32,
        frames: track.len() as u32,
        silent,
        message: format!(
            "Layer rig mouth: {} keys over {} frames ({} silent)",
            runs.len(),
            track.len(),
            silent
        ),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn runs(shapes: &[Viseme]) -> Vec<(u32, Viseme, u32)> {
        VisemeTrack {
            frames: shapes.to_vec(),
            fps: 24.0,
        }
        .runs()
    }

    #[test]
    fn a_line_is_exactly_as_long_as_it_was_given() {
        for frames in [0u32, 1, 2, 3, 5, 8, 22, 29, 70, 133] {
            for text in ["Aa gaya sarkar.", "Haan.", "", "...", "Kya?", "Itna seriously kyun le raha hai?"] {
                let shapes = visemes_from_text(text, frames);
                assert_eq!(shapes.len(), frames as usize, "{text:?} in {frames}");
            }
        }
    }

    #[test]
    fn vowels_and_closed_lips_give_their_shapes() {
        // "Mera baap" has an m, an e, an a, a b, aa, p: closed, E, Ai, closed...
        let shapes = visemes_from_text("Mera baap", 60);
        let seen: std::collections::BTreeSet<&str> = shapes.iter().map(|v| v.label()).collect();
        assert!(seen.contains("MBP"), "{seen:?}");
        assert!(seen.contains("E"), "{seen:?}");
        assert!(seen.contains("Ai"), "{seen:?}");
        assert!(seen.contains("Rest"), "{seen:?}");
    }

    #[test]
    fn each_letter_group_maps_where_the_plan_says() {
        let only = |text: &str| -> std::collections::BTreeSet<&'static str> {
            visemes_from_text(text, 40)
                .iter()
                .filter(|v| **v != Viseme::Rest)
                .map(|v| v.label())
                .collect()
        };
        assert!(only("aa").contains("Ai"));
        assert!(only("ee").contains("E"));
        assert!(only("o").contains("O"));
        assert!(only("au").contains("O"));
        assert!(only("oo").contains("U"));
        assert!(only("w").contains("U"));
        assert!(only("fa").contains("FV"));
        assert!(only("va").contains("FV"));
        assert!(only("pa").contains("MBP"));
        assert!(only("ba").contains("MBP"));
        assert!(only("ma").contains("MBP"));
    }

    #[test]
    fn every_shape_lasts_at_least_two_frames() {
        for text in [
            "Main soch raha hoon kuch bada karun.",
            "Itna seriously kyun le raha hai?",
            "Haan... roz hi jaati hogi.",
            "Chalo!",
        ] {
            for frames in [8u32, 14, 22, 40, 77, 120] {
                let shapes = visemes_from_text(text, frames);
                let r = runs(&shapes);
                for (start, shape, len) in &r {
                    assert!(
                        *len >= 2,
                        "{text:?} in {frames}: {} lasted {len} frame(s) at {start}: {r:?}",
                        shape.label()
                    );
                }
            }
        }
    }

    #[test]
    fn shapes_change_on_even_frames() {
        let shapes = visemes_from_text("Main soch raha hoon kuch bada karun.", 83);
        for (start, _, _) in runs(&shapes) {
            assert_eq!(start % 2, 0, "a shape began on odd frame {start}");
        }
    }

    #[test]
    fn a_line_ends_closed() {
        for text in ["Aa gaya sarkar.", "Kya?", "Chalo!", "nahi"] {
            let shapes = visemes_from_text(text, 30);
            assert_eq!(
                *shapes.last().unwrap(),
                Viseme::Rest,
                "{text:?} ended open: {shapes:?}"
            );
        }
    }

    #[test]
    fn a_line_does_not_start_closed() {
        let shapes = visemes_from_text("  ...Haan.", 20);
        assert_ne!(shapes[0], Viseme::Rest);
    }

    #[test]
    fn a_line_with_no_letters_is_a_closed_mouth() {
        assert!(visemes_from_text("...", 12).iter().all(|v| *v == Viseme::Rest));
        assert!(visemes_from_text("", 12).iter().all(|v| *v == Viseme::Rest));
    }

    #[test]
    fn a_line_too_short_for_its_shapes_keeps_the_important_ones() {
        // Six frames hold three shapes; the vowel and the closed lips win over
        // the consonants and the gaps.
        let shapes = visemes_from_text("Mera kaam baap", 6);
        assert_eq!(shapes.len(), 6);
        let r = runs(&shapes);
        assert!(r.iter().all(|(_, _, len)| *len >= 2), "{r:?}");
        assert!(r.len() <= 3);
    }

    #[test]
    fn punctuation_inside_a_line_closes_the_mouth_for_a_moment() {
        let shapes = visemes_from_text("Haan... roz hi jaati hogi.", 90);
        let r = runs(&shapes);
        assert!(
            r.iter().filter(|(_, v, _)| *v == Viseme::Rest).count() >= 2,
            "the ellipsis and the word gaps should close the mouth: {r:?}"
        );
    }

    #[test]
    fn a_scene_track_is_closed_between_lines_and_open_for_them() {
        let lines = [
            SpokenLine {
                start: 10,
                frames: 20,
                text: "Aa gaya sarkar.",
            },
            SpokenLine {
                start: 60,
                frames: 30,
                text: "Meeting chal rahi hai.",
            },
        ];
        let track = line_track(100, 24.0, &lines);
        assert_eq!(track.len(), 100);
        assert!(track.frames[..10].iter().all(|v| *v == Viseme::Rest));
        assert!(track.frames[10..30].iter().any(|v| *v != Viseme::Rest));
        assert!(track.frames[30..60].iter().all(|v| *v == Viseme::Rest));
        assert!(track.frames[60..90].iter().any(|v| *v != Viseme::Rest));
        assert!(track.frames[90..].iter().all(|v| *v == Viseme::Rest));
    }

    #[test]
    fn a_line_that_runs_past_the_scene_is_clipped_not_a_panic() {
        let lines = [SpokenLine {
            start: 90,
            frames: 40,
            text: "Haan.",
        }];
        let track = line_track(100, 24.0, &lines);
        assert_eq!(track.len(), 100);
    }

    // -- writing --------------------------------------------------------------------------

    use buzz_geom::Affine;
    use buzz_scene::{LayerKind, Object, SymbolInstance};

    /// A scene with a mouth layer and the 10-frame placeholder mouth.
    #[test]
    fn a_text_track_goes_through_the_audio_lip_syncs_writer() {
        let mut scene = Scene::default();
        let layer = scene.add_layer("Mouth", LayerKind::Normal);
        let symbol = crate::lipsync::placeholder_mouth(&mut scene, "Mouth");
        let track = line_track(
            48,
            24.0,
            &[SpokenLine {
                start: 4,
                frames: 30,
                text: "Aa gaya sarkar.",
            }],
        );
        let report = crate::lipsync::write_track(&mut scene, &track, 0, layer, symbol, Affine::IDENTITY);
        assert!(report.keyframes >= 3, "{report:?}");
        assert!(report.silent > 0);

        // Every key holds a mouth showing one of the shapes.
        let target = scene.layers().get(layer).expect("layer").clone();
        let mut shown = std::collections::BTreeSet::new();
        for key in target.frames.keyframes() {
            for object in key.objects.iter() {
                let ObjectKind::Instance(i) = &object.kind else {
                    panic!("not an instance")
                };
                shown.insert(i.first_frame);
            }
        }
        assert!(shown.len() >= 3, "the mouth should change shape: {shown:?}");
    }

    /// A rig whose mouth is a bound part, like the image pipeline's.
    fn rig_with_mouth() -> (Scene, ObjectId, usize) {
        use crate::figure::{self, FigureSpec};
        let mut scene = Scene::default();
        let layer = scene.add_layer("Rig", LayerKind::Normal);
        let id = scene.next_object_id();
        let mut figure = figure::build(&FigureSpec::default(), id, || scene.next_object_id());
        let symbol = crate::lipsync::placeholder_mouth(&mut scene, "Rig_mouth");
        let mouth_id = scene.next_object_id();
        let mut mouth = Object::instance_of(mouth_id, symbol);
        if let ObjectKind::Instance(i) = &mut mouth.kind {
            *i = SymbolInstance::new(symbol);
        }
        let mut parts = 0;
        if let ObjectKind::Armature(rig) = &mut figure.kind {
            rig.bind_rigid(Arc::new(mouth), 2);
            parts = rig.parts.len() - 1;
        }
        let placed = scene.add_object(layer, figure).expect("placed");
        (scene, placed, parts)
    }

    #[test]
    fn a_rigs_mouth_part_is_found_by_its_symbol() {
        let (scene, rig, part) = rig_with_mouth();
        assert_eq!(find_rig_mouth(&scene, rig), Some(part));
    }

    #[test]
    fn a_rigs_mouth_is_keyed_on_the_rigs_own_keyframes() {
        let (mut scene, rig, part) = rig_with_mouth();
        let layer = scene.find_object(rig).expect("rig").0;
        scene.update_layer(layer, |l| {
            while l.frames.length() < 60 {
                l.frames.insert_frame(l.frames.length());
            }
        });
        let track = line_track(
            60,
            24.0,
            &[SpokenLine {
                start: 6,
                frames: 30,
                text: "Aa gaya sarkar.",
            }],
        );
        let report = write_rig_mouth(&mut scene, rig, &track, 0).expect("a rig mouth");
        assert!(report.keyframes >= 3, "{report:?}");

        // At each key, the part shows that frame's shape, single-frame.
        let target = scene.layers().get(layer).expect("layer").clone();
        let mut shapes = std::collections::BTreeSet::new();
        for key in target.frames.keyframes() {
            let object = key.objects.first().expect("the rig");
            let ObjectKind::Armature(a) = &object.kind else {
                panic!("not a rig")
            };
            let ObjectKind::Instance(i) = &a.parts[part].artwork.kind else {
                panic!("not an instance")
            };
            assert_eq!(i.loop_mode, buzz_scene::LoopMode::SingleFrame);
            assert_eq!(i.first_frame, track.at(key.start).frame(), "at {}", key.start);
            shapes.insert(i.first_frame);
        }
        assert!(shapes.len() >= 3, "{shapes:?}");
    }

    #[test]
    fn writing_a_mouth_onto_something_that_is_not_a_rig_says_so() {
        let mut scene = Scene::default();
        let layer = scene.add_layer("Rig", LayerKind::Normal);
        let id = scene
            .add_shape(
                layer,
                buzz_scene::ShapeData::filled(buzz_geom::BezPath::new(), peniko::Color::BLACK),
            )
            .expect("shape");
        let track = VisemeTrack {
            frames: vec![Viseme::Rest; 4],
            fps: 24.0,
        };
        assert_eq!(
            write_rig_mouth(&mut scene, id, &track, 0),
            Err(RigMouthError::NotRigged)
        );
    }
}
