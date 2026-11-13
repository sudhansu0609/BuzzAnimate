//! **The mouth keeps up with the voice.** Lip sync held one shape for twenty
//! frames and more through connected speech ("it gives the same mouth shapes
//! for too many frames even when the audio progresses"): every voiced stretch
//! between two consonant dips took a single vowel. The fixture is eight
//! seconds of real narration, mono 22 kHz.

use buzz_audio::analysis::{LipSyncOptions, Viseme, analyse_visemes};

fn track() -> buzz_audio::VisemeTrack {
    let bytes = include_bytes!("fixtures/narration_8s.mp3");
    let clip = buzz_audio::Clip::decode(bytes, "narration").unwrap();
    analyse_visemes(&clip, 24.0, &LipSyncOptions::default())
}

#[test]
fn speech_changes_shape_at_syllable_rate() {
    let track = track();
    let runs = track.runs();
    let mut speaking: Vec<u32> = runs.iter().filter(|r| r.1 != Viseme::Rest).map(|r| r.2).collect();
    speaking.sort();
    let median = speaking[speaking.len() / 2];
    assert!(median <= 6, "median spoken run {median} frames: {runs:?}");
    let longest = *speaking.last().unwrap();
    assert!(longest <= 10, "one shape held {longest} frames through speech: {runs:?}");

    let shapes: std::collections::BTreeSet<&str> = track.frames.iter().map(|v| v.label()).collect();
    assert!(shapes.len() >= 4, "only {shapes:?}");
}

#[test]
fn no_shape_flickers_for_a_single_frame() {
    let runs = track().runs();
    // The first and last runs may be cut by the clip's own edges.
    for (start, shape, length) in &runs[1..runs.len() - 1] {
        assert!(*length >= 2, "{shape:?} for one frame at {start}");
    }
}
