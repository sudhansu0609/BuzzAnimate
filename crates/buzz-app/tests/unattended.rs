//! **A brief in, a film out, with nobody watching.**
//!
//! Everything needed to make a film without a person had been built and tested
//! separately — the director, the staging, the scenery, the performances, the
//! reel, the encoders — and none of it was reachable without opening a window
//! and clicking. This is the test that the join actually holds: prose on disk,
//! a picture on disk, no event loop in between.
//!
//! Rendered as a PNG rather than an MP4 so the test needs no ffmpeg and takes a
//! second. The video path is the same `run_export` call with a different
//! target, and is covered by the headless video tests in `buzz-export`.
//!
//! Skips with no GPU, like every other headless test.

use buzz_app::headless::{RenderJob, render};
use buzz_render::GpuPreference;

const BRIEF: &str = "\
Sunset. A forest.
Ana walks in from the left.
Ana talks to Ben. Ben listens.
Ben points at the door.
Ben walks off right.
";

fn have_gpu() -> bool {
    buzz_export::Exporter::new(&GpuPreference::Automatic).is_ok()
}

fn job(dir: &std::path::Path, brief: &str, out: &str) -> RenderJob {
    let path = dir.join("story.txt");
    std::fs::write(&path, brief).expect("write the brief");
    RenderJob {
        document: None,
        brief: Some(path),
        script: None,
        audio: Vec::new(),
        save: None,
        preview: None,
        output: Some(dir.join(out)),
        height: Some(200),
        gpu: GpuPreference::Automatic,
    }
}

/// A job that only runs a script, and only writes a document.
fn scripted(dir: &std::path::Path, source: &str) -> RenderJob {
    let path = dir.join("film.js");
    std::fs::write(&path, source).expect("write the script");
    RenderJob {
        document: None,
        brief: None,
        script: Some(path),
        audio: Vec::new(),
        save: Some(dir.join("film.buzz")),
        preview: None,
        output: None,
        height: None,
        gpu: GpuPreference::Automatic,
    }
}

/// Where a job put its film.
fn output_of(job: &RenderJob) -> &std::path::Path {
    job.output.as_deref().expect("this job renders")
}

/// **Six lines of prose become a picture**, with no window anywhere.
#[test]
fn a_brief_becomes_a_film() {
    if !have_gpu() {
        eprintln!("skipping: no usable GPU");
        return;
    }
    let dir = tempfile::tempdir().expect("temp dir");
    let job = job(dir.path(), BRIEF, "film.png");

    let message = render(&job).expect("the brief should render");
    assert!(
        message.contains("Directed"),
        "the report should say what was directed: {message}"
    );

    let bytes = std::fs::metadata(output_of(&job))
        .map(|m| m.len())
        .expect("the film should exist");
    assert!(bytes > 1_000, "the picture is only {bytes} bytes");
}

/// **A target height keeps the aspect and comes out even.**
///
/// The encoders refuse odd dimensions, and the last step of an overnight render
/// is the worst possible place to find that out.
#[test]
fn a_render_honours_the_height_it_was_given() {
    if !have_gpu() {
        eprintln!("skipping: no usable GPU");
        return;
    }
    let dir = tempfile::tempdir().expect("temp dir");
    let job = job(dir.path(), BRIEF, "film.png");
    render(&job).expect("renders");

    let decoded = image_size(output_of(&job)).expect("a readable png");
    assert_eq!(decoded.1, 200, "the height was not honoured: {decoded:?}");
    assert_eq!(decoded.0 % 2, 0, "an odd width: {decoded:?}");
}

/// **A brief nobody can read is refused, by name.**
///
/// Silence here is the worst outcome: an overnight job that produced nothing
/// and said nothing is one you find out about in the morning.
#[test]
fn a_brief_that_says_nothing_is_refused() {
    let dir = tempfile::tempdir().expect("temp dir");
    let job = job(dir.path(), "the quick brown fox\n", "film.png");
    let err = render(&job).expect_err("nothing directable");
    let said = err.to_string();
    assert!(
        said.contains("could be directed") && said.contains("story.txt"),
        "the reason should name the file: {said}"
    );
    // And carry the director's own complaint, so the fix is in the message.
    assert!(
        said.contains("named someone doing something"),
        "the reason should say what the parser wanted: {said}"
    );
    assert!(
        !output_of(&job).exists(),
        "a file was written for a brief that failed"
    );
}

/// **An unrenderable extension is refused before any work is done.**
#[test]
fn an_unknown_output_format_is_refused() {
    let dir = tempfile::tempdir().expect("temp dir");
    let job = job(dir.path(), BRIEF, "film.psd");
    let err = render(&job).expect_err("no psd encoder");
    assert!(err.to_string().contains("psd"), "{err}");
}

/// **A script builds a film, and the document is written.**
///
/// The gap this closes: `--script` used to need the window, so the most capable
/// half of the automation could not be reached by anything unattended. A brief
/// cannot rig a face or mask a sky; a script can, and now it can do it at
/// midnight.
#[test]
fn a_script_can_build_a_film_and_save_it() {
    let dir = tempfile::tempdir().expect("temp dir");
    let job = scripted(
        dir.path(),
        r#"
        var doc = fl.getDocumentDOM();
        doc.width = 960; doc.height = 540;
        doc.scenes.setLength(24);
        doc.setTheScene({setting: "daylight", frames: 24, lit: true});
        var ana = doc.addCharacter({name: "Ana", x: 300, y: 480, height: 260, frames: 24});
        doc.perform(ana.body, "walk", 0, 24);

        doc.scenes.add("Village");
        doc.scenes.setLength(24);
        var set = doc.setTheScene({setting: "night", frames: 24, clouds: true});
        doc.layScenery("village", set.horizonY, set.backdrop);
        fl.trace("scenes:", doc.scenes.count);
        "#,
    );

    let message = render(&job).expect("the script should run");
    assert!(
        message.contains("2 scene(s)"),
        "the report should say how many scenes: {message}"
    );

    let saved = job.save.as_deref().expect("a save path");
    let doc = buzz_doc::Document::open(saved).expect("the document should open");
    assert_eq!(doc.scene_names().len(), 2, "both shots should be in the file");

    // The face rig survived the round trip: the parenting is what makes it a
    // rig rather than three layers that happen to line up.
    let first = &doc.film()[0];
    let face = first
        .layers()
        .iter()
        .find(|l| l.name == "Ana Face")
        .expect("Ana has a face layer");
    assert!(
        face.follows.is_some(),
        "the face is not parented to anything"
    );
}

/// **A script that throws stops the job**, rather than saving half a film.
#[test]
fn a_failing_script_is_reported_by_name() {
    let dir = tempfile::tempdir().expect("temp dir");
    let job = scripted(dir.path(), "fl.getDocumentDOM().layScenery('swamp', 100, 0);");
    let err = render(&job).expect_err("no such scenery");
    let said = err.to_string();
    assert!(said.contains("film.js"), "the reason should name the file: {said}");
    assert!(said.contains("swamp"), "the reason should carry the complaint: {said}");
    assert!(
        !job.save.as_deref().expect("a save path").exists(),
        "a document was written for a script that failed"
    );
}

/// **A slice of a take, not the whole file.**
///
/// A dialogue recording is minutes long and a shot is seconds long, so the
/// useful unit is almost never the file. Cut on a sample *frame* -- cutting a
/// stereo file mid-frame swaps the channels for the rest of the clip.
#[test]
fn a_sound_can_be_sliced_on_the_way_in() {
    let dir = tempfile::tempdir().expect("temp dir");
    let wav = dir.path().join("take.wav");
    write_tone(&wav, 4.0);

    let mut job = scripted(
        dir.path(),
        r#"
        var doc = fl.getDocumentDOM();
        doc.scenes.setLength(48);
        var info = doc.sounds.info(0);
        fl.trace("seconds:", info.seconds.toFixed(2));
        if (info.seconds > 1.6) throw new Error("the slice was not taken: " + info.seconds);
        doc.setTheScene({setting: "daylight", frames: 48});
        doc.sounds.attach(0, {frame: 0});
        "#,
    );
    job.audio = vec![buzz_app::headless::AudioIn {
        path: wav,
        from: Some(1.0),
        length: Some(1.5),
    }];

    render(&job).expect("the sliced take should run");
    let doc = buzz_doc::Document::open(job.save.as_deref().expect("a save path"))
        .expect("the document should open");
    assert_eq!(doc.film()[0].sounds().len(), 1, "the take is not in the document");
}

/// A mono WAV of a steady tone, `seconds` long.
fn write_tone(path: &std::path::Path, seconds: f64) {
    let rate = 8_000u32;
    let frames = (rate as f64 * seconds) as usize;
    let data = (frames * 2) as u32;
    let mut out: Vec<u8> = Vec::with_capacity(44 + data as usize);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&rate.to_le_bytes());
    out.extend_from_slice(&(rate * 2).to_le_bytes());
    out.extend_from_slice(&2u16.to_le_bytes());
    out.extend_from_slice(&16u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data.to_le_bytes());
    for i in 0..frames {
        let t = i as f64 / rate as f64;
        let value = ((t * 220.0 * std::f64::consts::TAU).sin() * 12_000.0) as i16;
        out.extend_from_slice(&value.to_le_bytes());
    }
    std::fs::write(path, out).expect("write the tone");
}

/// The pixel size of a PNG, from its header.
fn image_size(path: &std::path::Path) -> Option<(u32, u32)> {
    let bytes = std::fs::read(path).ok()?;
    // IHDR is the first chunk: 8 bytes of signature, 4 length, 4 type, then
    // width and height as big-endian u32s.
    if bytes.len() < 24 {
        return None;
    }
    let w = u32::from_be_bytes(bytes[16..20].try_into().ok()?);
    let h = u32::from_be_bytes(bytes[20..24].try_into().ok()?);
    Some((w, h))
}
