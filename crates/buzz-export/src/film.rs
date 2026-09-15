//! Assembling a film from its shots.
//!
//! A film is built by the ordinary export queue — one segment per shot, each
//! encoded with the *same* preset — and then joined with ffmpeg's concat
//! demuxer copying the streams:
//!
//! ```text
//! ffmpeg -f concat -safe 0 -i list.txt -c copy -fflags +genpts out.mp4
//! ```
//!
//! # Why `-c copy` is safe here
//!
//! Concatenation by stream copy is only correct when every segment has matching
//! streams — same codec, resolution, frame rate, pixel format. That is *our*
//! guarantee, not a hope: the caller validates that every shot shares a frame
//! rate ([`buzz_doc::Project::validate`]) and encodes them all with one preset,
//! so the segments agree by construction. `+genpts` regenerates presentation
//! timestamps across the joins so the result plays without stutter at the seams.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{Context, Result, bail};

/// Join `segments`, in order, into `output` by stream copy.
///
/// Every segment must already have been encoded with identical settings — see
/// the module note. The list file the concat demuxer needs is written beside the
/// output and removed afterwards.
pub fn concat_segments(segments: &[PathBuf], output: &Path) -> Result<()> {
    if segments.is_empty() {
        bail!("a film needs at least one shot to assemble");
    }

    // The demuxer reads a text file of `file '...'` lines. It lives beside the
    // output so relative paths and permissions match the target.
    let dir = output.parent().unwrap_or_else(|| Path::new("."));
    let list_path = dir.join(format!(
        "buzz-concat-{}.txt",
        std::process::id().wrapping_add(segments.len() as u32)
    ));

    {
        let mut list = std::fs::File::create(&list_path)
            .with_context(|| format!("writing the concat list at {}", list_path.display()))?;
        for segment in segments {
            let abs = segment
                .canonicalize()
                .unwrap_or_else(|_| segment.clone());
            // Single quotes are the demuxer's escape; a quote inside a path is
            // written as `'\''`, which is the documented escape.
            let escaped = abs.to_string_lossy().replace('\'', r"'\''");
            writeln!(list, "file '{escaped}'").context("writing a concat list entry")?;
        }
    }

    let status = Command::new("ffmpeg")
        .arg("-hide_banner")
        .arg("-y")
        .args(["-f", "concat", "-safe", "0"])
        .args(["-i", &list_path.to_string_lossy()])
        .args(["-c", "copy"])
        .args(["-fflags", "+genpts"])
        .arg(output.as_os_str())
        .stdin(Stdio::null())
        .status();

    // Remove the list whatever happened, so a failed build leaves no litter.
    let _ = std::fs::remove_file(&list_path);

    let status = status.context("running ffmpeg to concatenate the film")?;
    if !status.success() {
        bail!("ffmpeg failed to concatenate the film ({status})");
    }
    if !output.exists() {
        bail!("ffmpeg reported success but wrote no film");
    }
    Ok(())
}
