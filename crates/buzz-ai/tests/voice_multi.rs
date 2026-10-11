//! Stage V2/V5: multilingual Whisper on voice-command fixtures. No mic, no
//! network: the test skips cleanly when the model is not downloaded.
//!
//! `tests/fixtures/voice_en_hey_buzz.wav` is a Windows SAPI render (Zira,
//! 16 kHz mono) of "Hey Buzz, add a red ball to the scene." There is no Hindi
//! fixture: this machine has no Hindi SAPI voice, so Hindi is untested here.

use std::path::Path;

fn skip() -> bool {
    if buzz_ai::transcribe::multi_ready() {
        return false;
    }
    eprintln!("SKIPPED: Whisper multilingual is not downloaded (AI > Models)");
    true
}

#[test]
fn multilingual_whisper_hears_an_english_command_and_the_wake_matcher_splits_it() {
    if skip() {
        return;
    }
    let clip = buzz_audio::Clip::open(&Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/voice_en_hey_buzz.wav"))
        .expect("fixture");
    let samples = clip.resampled_mono(16_000);
    let t = std::time::Instant::now();
    let heard = buzz_ai::transcribe::transcribe_command(&samples, &|| false).expect("transcribe");
    eprintln!("heard {:?} in {} ms", heard, t.elapsed().as_millis());
    assert_eq!(heard.lang, "en");
    let lower = heard.text.to_lowercase();
    for w in ["red", "ball", "scene"] {
        assert!(lower.contains(w), "{lower}");
    }
    match buzz_ai::voice::match_wake(&heard.text) {
        buzz_ai::voice::WakeMatch::Command(c) => assert!(c.to_lowercase().contains("ball"), "{c}"),
        other => panic!("wake phrase not found in {:?}: {other:?}", heard.text),
    }
}
