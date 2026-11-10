//! Re-voice one recording through every preset, for listening.
//!
//! ```text
//! cargo run --release -p buzz-voice --example voice_demo -- take.wav out_dir
//! ```
//!
//! Writes `out_dir/<preset>.wav` and prints each one's measured pitch, so a
//! preset that went wrong shows up before anyone has to listen to it.

use buzz_voice::{Preset, render, track_pitch};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let input = args.next().ok_or("usage: voice_demo <take.wav|aiff> <out_dir>")?;
    let out = std::path::PathBuf::from(args.next().ok_or("missing out_dir")?);
    std::fs::create_dir_all(&out)?;
    let clip = buzz_audio::Clip::open(std::path::Path::new(&input))?;
    let take = clip.mono();
    let sr = clip.sample_rate;
    let before = track_pitch(&take, sr).mean().unwrap_or(0.0);
    println!("{input}: {:.2} s, {sr} Hz, pitch {before:.0} Hz", take.len() as f32 / sr as f32);
    for preset in Preset::ALL {
        let started = std::time::Instant::now();
        let voiced = render(&take, sr, &preset.profile());
        let ms = started.elapsed().as_millis();
        let pitch = track_pitch(&voiced, sr).mean().unwrap_or(0.0);
        let name = format!("{:?}.wav", preset);
        let spec = hound::WavSpec { channels: 1, sample_rate: sr, bits_per_sample: 16, sample_format: hound::SampleFormat::Int };
        let mut w = hound::WavWriter::create(out.join(&name), spec)?;
        for v in &voiced {
            w.write_sample((v.clamp(-1.0, 1.0) * 32767.0) as i16)?;
        }
        w.finalize()?;
        println!("  {:<22} pitch {pitch:>4.0} Hz  {:>5.2} s  ({ms} ms)", preset.label(), voiced.len() as f32 / sr as f32);
    }
    Ok(())
}
