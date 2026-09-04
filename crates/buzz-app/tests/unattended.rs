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
        output: dir.join(out),
        height: Some(200),
        gpu: GpuPreference::Automatic,
    }
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

    let bytes = std::fs::metadata(&job.output)
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

    let decoded = image_size(&job.output).expect("a readable png");
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
        !job.output.exists(),
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
