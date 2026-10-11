//! **Every MP3 frame decodes, so a sound keeps its length** (S11 of
//! `docs/BIMARI5_LAG_PLAN.md`: "the audio seems a bit sped up").
//!
//! The fixture is 90 frames (frames 330..420) of Bimari5's 320 kbps, 44.1 kHz
//! stereo narration. Symphonia 0.5.5 rejected 40 of them with "invalid
//! main_data offset" (a granule whose data starts exactly at the end of the
//! frame's main data: silence, and legal), and `Clip::decode` skipped them, so
//! the whole film's sound came out 641 frames (16.8 s, 8%) short and ran
//! early. ffmpeg decodes every frame; so must we.

#[test]
fn a_320k_mp3_with_silent_granules_decodes_every_frame() {
    let bytes = include_bytes!("fixtures/silent_granules_320k.mp3");
    let clip = buzz_audio::Clip::decode(bytes, "silent granules").unwrap();
    assert_eq!(clip.sample_rate, 44_100);
    assert_eq!(clip.channels, 2);
    // 90 MPEG-1 Layer III frames of 1152 samples each.
    assert_eq!(clip.len(), 90 * 1152, "a dropped frame shortens the sound");
}
