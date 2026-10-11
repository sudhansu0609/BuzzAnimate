//! F7.9: Whisper base.en on a short bundled clip (`tests/data/speech_short.wav`,
//! Windows SAPI voice, 22.05 kHz mono, ~5.5 s: "Ramu, bring the water to the
//! old man. The rain is coming tonight.").
//!
//! Ignored: needs the model (downloaded by the first run through the
//! registry, SHA-verified). Records numbers, not exact words: the word
//! overlap with the script and the timings are printed.

use std::path::Path;
use std::time::Instant;

const SCRIPT: &str = "Ramu, bring the water to the old man. The rain is coming tonight.";

fn words(s: &str) -> Vec<String> {
    s.split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(|w| w.to_lowercase())
        .collect()
}

#[test]
#[ignore = "downloads / needs the Whisper base.en ONNX model"]
fn whisper_transcribes_the_bundled_clip_with_timed_segments() {
    let t = Instant::now();
    buzz_ai::models::ensure(buzz_ai::models::ModelId::Whisper, |_, _| {}, || false)
        .expect("Whisper download + SHA-256 verification");
    eprintln!("ensure: {} ms", t.elapsed().as_millis());

    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data/speech_short.wav");
    let clip = buzz_audio::Clip::open(&path).expect("decode through buzz-audio");
    assert_eq!(clip.sample_rate, 22_050, "the clip exercises the resampler");

    let mut windows = 0;
    let tr = buzz_ai::transcribe::transcribe_clip(&clip, &mut |d, _| windows = d, &|| false)
        .expect("transcribe");
    eprintln!(
        "audio {:.2} s, load {} ms, run {} ms, provider {}, windows {windows}",
        tr.audio_seconds, tr.load_ms, tr.run_ms, tr.provider
    );
    for s in &tr.segments {
        eprintln!("  [{:.2} → {:.2}] {}", s.start, s.end, s.text);
    }
    let got = words(&tr.text());
    let want = words(SCRIPT);
    let hit = want.iter().filter(|w| got.contains(w)).count();
    let recall = hit as f64 / want.len() as f64;
    eprintln!("word recall {hit}/{} = {recall:.2}", want.len());
    assert!(!tr.segments.is_empty());
    assert!(recall >= 0.6, "word recall {recall:.2}");
    for s in &tr.segments {
        assert!(s.end > s.start && s.end <= tr.audio_seconds + 1.0, "{s:?}");
    }
    // A second run reuses the loaded model.
    let tr2 = buzz_ai::transcribe::transcribe_clip(&clip, &mut |_, _| {}, &|| false).unwrap();
    assert_eq!(tr2.load_ms, 0);
    eprintln!("warm run {} ms", tr2.run_ms);
    let srt = buzz_ai::transcribe::to_srt(&tr.segments, Some("Ramu"));
    eprintln!("{srt}");
}
