//! Reading foreign formats, and telling the user what came across.
//!
//! Three importers sit behind one entry point. Which one runs is decided by
//! the file's extension, because that is what the user chose in the dialog and
//! what they will blame if it goes wrong — sniffing content would be cleverer
//! and would make a mis-import harder to explain.
//!
//! # The fidelity report is the point
//!
//! Every importer returns a list of what it could not bring across. That list
//! is not a debug aid; it is the deliverable. An import that silently drops a
//! third of a file is worse than one that refuses, because the user finds out
//! later, in front of somebody else. So the report is surfaced in a dialog the
//! user has to dismiss whenever anything was lost.

use std::path::Path;

use buzz_scene::Scene;

/// A file read into a scene, with an account of what was lost.
#[derive(Debug)]
pub struct Imported {
    pub scene: Scene,
    /// One line per counted category, for the status bar.
    pub summary: String,
    /// What did not come across. Empty means a complete import.
    pub unsupported: Vec<String>,
}

/// What the user is shown after an import.
#[derive(Debug, Clone)]
pub struct ImportSummary {
    pub title: String,
    pub what_arrived: String,
    pub unsupported: Vec<String>,
    /// This is a *failure*, not a partial success.
    ///
    /// The same dialog serves both, because the question a user has is the
    /// same either way — what happened to my file? — and a failure that only
    /// writes a line into the status bar is one nobody reads. It is worded
    /// differently and it does not claim that everything else imported
    /// normally, because nothing did.
    pub failed: bool,
}

/// File extensions the importers understand, for the dialog's filter.
pub const IMPORTABLE: &[&str] = &["fla", "xfl", "swf", "pdf", "ai"];

/// Sound files, which File > Import also accepts.
///
/// **A sound is not read by any of the importers above** - it goes into the
/// library rather than onto the stage, so it takes a different route entirely
/// (`Editor::import_sound`). It still has to be *offered* here, because File >
/// Import is the command anybody reaches for with a dialogue track in hand, and
/// answering "BuzzAnimate cannot import .mp3 files" from a program with a sound
/// library, a waveform display and a lip-sync dialog is simply untrue. The
/// router below sends these to the sound path.
///
/// The same list the Import Sound dialog filters on, so the two cannot drift.
pub const AUDIBLE: &[&str] = &["wav", "mp3", "ogg", "flac", "m4a", "aac"];

/// Is this a sound file, and so bound for the library rather than the stage?
pub fn is_audio(path: &Path) -> bool {
    path.extension()
        .map(|e| e.to_string_lossy().to_ascii_lowercase())
        .is_some_and(|e| AUDIBLE.contains(&e.as_str()))
}

/// Read a file into a scene.
///
/// Errors are already phrased for a person: each importer explains what it
/// found and, where it can, what to do about it.
pub fn read(path: &Path) -> Result<Imported, String> {
    // A `.xfl` document is a folder, so a directory is not an error here.
    if path.is_dir() {
        return read_xfl(path);
    }

    let extension = path
        .extension()
        .map(|e| e.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();

    match extension.as_str() {
        "fla" | "xfl" => read_xfl(path),
        "swf" => read_swf(path),
        "pdf" | "ai" => read_pdf(path),
        "" => Err("that file has no extension, so there is no way to tell what it is".into()),
        // Handled by the caller before it ever gets here: a sound has no scene
        // to return. Reaching this arm means the routing was bypassed, and
        // saying so is more use than the general refusal below.
        other if AUDIBLE.contains(&other) => Err(format!(
            ".{other} is a sound rather than artwork. Bring it in with \
             File > Import Sound."
        )),
        other => Err(format!(
            "BuzzAnimate cannot import .{other} files. It reads .fla and .xfl \
             from Animate, .swf movies, .pdf or .ai artwork, and sound as \
             .wav, .mp3, .ogg, .flac, .m4a or .aac."
        )),
    }
}

fn read_xfl(path: &Path) -> Result<Imported, String> {
    let (scene, report) = buzz_import_xfl::import(path).map_err(|e| e.to_string())?;
    Ok(Imported {
        scene,
        summary: report.summary(),
        unsupported: report.unsupported,
    })
}

fn read_swf(path: &Path) -> Result<Imported, String> {
    let (scene, report) = buzz_import_swf::import(path).map_err(|e| e.to_string())?;
    Ok(Imported {
        scene,
        summary: report.summary(),
        unsupported: report.unsupported,
    })
}

fn read_pdf(path: &Path) -> Result<Imported, String> {
    let (scene, report) = buzz_import_pdf::import(path).map_err(|e| e.to_string())?;
    Ok(Imported {
        scene,
        summary: report.summary(),
        unsupported: report.unsupported,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn every_advertised_extension_is_actually_dispatched() {
        // A missing file is fine: what matters is that the extension is
        // recognised, so the error is about the file rather than the format.
        for extension in IMPORTABLE {
            let path = PathBuf::from(format!("does-not-exist.{extension}"));
            let error = read(&path).expect_err("the file does not exist");
            assert!(
                !error.contains("cannot import"),
                ".{extension} is offered in the dialog but not dispatched: {error}"
            );
        }
    }

    #[test]
    fn an_unknown_extension_says_what_is_supported() {
        let error = read(&PathBuf::from("drawing.psd")).unwrap_err();
        assert!(
            error.contains(".psd"),
            "it should name what was refused: {error}"
        );
        assert!(error.contains(".fla"), "and list what works: {error}");
    }

    #[test]
    fn a_file_with_no_extension_is_refused_clearly() {
        let error = read(&PathBuf::from("mystery")).unwrap_err();
        assert!(error.contains("no extension"), "{error}");
    }

    /// The dialog filter and the dispatcher must agree, or the user can pick a
    /// file the application then refuses.
    #[test]
    fn the_filter_list_has_no_duplicates_and_is_lowercase() {
        let mut seen = std::collections::BTreeSet::new();
        for extension in IMPORTABLE {
            assert_eq!(*extension, extension.to_ascii_lowercase());
            assert!(seen.insert(*extension), "{extension} is listed twice");
        }
    }

    /// Extensions arrive from the file system in whatever case the user's
    /// disk has them, and `.FLA` is still a `.fla`.
    #[test]
    fn extensions_are_matched_case_insensitively() {
        let error = read(&PathBuf::from("Movie.SWF")).unwrap_err();
        assert!(
            !error.contains("cannot import"),
            "an upper-case extension should still dispatch: {error}"
        );
    }
}
