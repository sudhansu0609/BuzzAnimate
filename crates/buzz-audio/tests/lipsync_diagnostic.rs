//! **Lip sync diagnostic**: run lip sync over a real recording and print how
//! long each mouth shape is held. `ignore`d because the audio is the user's
//! own; run with
//! `LIPSYNC_AUDIO=path.mp3 cargo test -p buzz-audio --test lipsync_diagnostic -- --ignored --nocapture`.

use buzz_audio::analysis::{analyse_visemes, LipSyncOptions};

#[test]
#[ignore]
fn print_viseme_runs() {
    let path = std::env::var("LIPSYNC_AUDIO").unwrap_or_else(|_| {
        format!("{}/Documents/khayal3Baje/alaxmi/audios1/alaxmi_Marker 01.mp3", std::env::var("HOME").unwrap())
    });
    let clip = buzz_audio::Clip::open(std::path::Path::new(&path)).unwrap();
    let track = analyse_visemes(&clip, 24.0, &LipSyncOptions::default());
    let runs = track.runs();
    let mut lengths: Vec<u32> = runs.iter().map(|r| r.2).collect();
    lengths.sort();
    let voiced: Vec<u32> = runs.iter().filter(|r| r.1 != buzz_audio::analysis::Viseme::Rest).map(|r| r.2).collect();
    let mut voiced_sorted = voiced.clone();
    voiced_sorted.sort();
    println!("{path}: {} frames, {} runs", track.len(), runs.len());
    println!("median run {}, max {}, median non-rest run {}, max non-rest {}",
        lengths[lengths.len() / 2], lengths.last().unwrap(),
        voiced_sorted.get(voiced_sorted.len() / 2).copied().unwrap_or(0), voiced_sorted.last().copied().unwrap_or(0));
    let mut histogram = std::collections::BTreeMap::new();
    for f in 0..track.len() as u32 {
        *histogram.entry(track.at(f).label()).or_insert(0) += 1;
    }
    println!("histogram {histogram:?}");
    let long: Vec<u32> = voiced.iter().copied().filter(|l| *l > 6).collect();
    let speaking: u32 = voiced.iter().sum();
    println!("non-rest runs over 6 frames: {} covering {} of {} speaking frames", long.len(), long.iter().sum::<u32>(), speaking);
    println!("{:?}", runs.iter().take(60).map(|r| format!("{}x{}", r.1.label(), r.2)).collect::<Vec<_>>());
}
