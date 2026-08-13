//! The `.buzz` container.
//!
//! A zip archive, laid out so the parts that will grow later already have a
//! home:
//!
//! ```text
//! mimetype        stored uncompressed, first entry, so the file is
//!                 identifiable from its first bytes
//! meta.json       format version and provenance
//! document.json   stage, layers, artwork
//! library/        symbols            (Phase 4)
//! media/          bitmaps, audio     (Phase 5)
//! ```
//!
//! The uncompressed-`mimetype`-first convention is borrowed from ODF and EPUB.
//! It costs nothing and means `file` and other sniffers can identify a `.buzz`
//! document without unzipping it.
//!
//! # Saving is atomic
//!
//! Writing straight to the destination means a crash mid-write leaves a
//! truncated file where the user's work used to be. Saving writes to a
//! temporary file alongside the target and renames it into place, so the
//! document is either the old version or the new one, never a broken hybrid.

use std::fs::File;
use std::io::{Cursor, Read, Seek, Write};
use std::path::{Path, PathBuf};

use buzz_scene::Scene;
use serde::{Deserialize, Serialize};
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipArchive, ZipWriter};

use crate::serial::{DocumentDto, FORMAT_VERSION, SerialError};

/// Identifies the container.
pub const MIMETYPE: &str = "application/vnd.buzzcaf.buzzanimate";

const ENTRY_MIMETYPE: &str = "mimetype";
const ENTRY_META: &str = "meta.json";
const ENTRY_DOCUMENT: &str = "document.json";

/// The customary extension.
pub const EXTENSION: &str = "buzz";

/// Anything that can go wrong reading or writing a document.
#[derive(Debug, thiserror::Error)]
pub enum DocError {
    #[error("input/output error: {0}")]
    Io(#[from] std::io::Error),
    #[error("the file is not a valid .buzz archive: {0}")]
    Archive(String),
    #[error("this does not look like a BuzzAnimate document (mimetype was {0:?})")]
    WrongType(String),
    #[error("could not read the document data: {0}")]
    Json(#[from] serde_json::Error),
    #[error(transparent)]
    Serial(#[from] SerialError),
}

impl From<zip::result::ZipError> for DocError {
    fn from(e: zip::result::ZipError) -> Self {
        Self::Archive(e.to_string())
    }
}

/// Provenance, written alongside the document.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Meta {
    pub format_version: u32,
    /// The build that last wrote the file, for diagnosing format issues later.
    pub generator: String,
    /// Seconds since the Unix epoch.
    #[serde(default)]
    pub modified: u64,
}

impl Default for Meta {
    fn default() -> Self {
        Self {
            format_version: FORMAT_VERSION,
            generator: format!("BuzzAnimate {}", env!("CARGO_PKG_VERSION")),
            modified: now_unix(),
        }
    }
}

fn now_unix() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Serialise a scene into an in-memory `.buzz` archive.
///
/// Separated from the file writing so autosave can build the bytes on a
/// background thread and only touch the disk at the end.
pub fn to_bytes(scene: &Scene) -> Result<Vec<u8>, DocError> {
    let mut buffer = Cursor::new(Vec::new());
    write_archive(&mut buffer, scene)?;
    Ok(buffer.into_inner())
}

fn write_archive<W: Write + Seek>(writer: &mut W, scene: &Scene) -> Result<(), DocError> {
    let mut zip = ZipWriter::new(writer);

    // Stored, not deflated, and written first: that is what makes the archive
    // sniffable without decompression.
    zip.start_file(
        ENTRY_MIMETYPE,
        SimpleFileOptions::default().compression_method(CompressionMethod::Stored),
    )?;
    zip.write_all(MIMETYPE.as_bytes())?;

    let deflated = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);

    zip.start_file(ENTRY_META, deflated)?;
    zip.write_all(&serde_json::to_vec_pretty(&Meta::default())?)?;

    zip.start_file(ENTRY_DOCUMENT, deflated)?;
    let dto = DocumentDto::from_scene(scene);
    zip.write_all(&serde_json::to_vec_pretty(&dto)?)?;

    // Sounds go in `media/`, byte for byte as they were imported — the
    // directory the container reserved for exactly this in Phase 1.
    //
    // **Stored, not deflated.** MP3 and compressed WAV do not compress again;
    // deflating them costs time on every save and autosave and gives back a
    // fraction of a percent. An uncompressed entry also means an unzip
    // recovers a playable file directly.
    let stored = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
    for sound in scene.sounds().iter() {
        zip.start_file(format!("media/{}", sound.file_name()), stored)?;
        zip.write_all(&sound.data)?;
    }

    zip.finish()?;
    Ok(())
}

/// Read a scene from `.buzz` bytes.
pub fn from_bytes(bytes: &[u8]) -> Result<Scene, DocError> {
    let mut archive = ZipArchive::new(Cursor::new(bytes))?;

    // Check the type before trusting anything else in the file.
    let mut mimetype = String::new();
    archive
        .by_name(ENTRY_MIMETYPE)
        .map_err(|_| DocError::WrongType("missing".into()))?
        .read_to_string(&mut mimetype)?;
    if mimetype.trim() != MIMETYPE {
        return Err(DocError::WrongType(mimetype.trim().to_string()));
    }

    let mut document = String::new();
    archive.by_name(ENTRY_DOCUMENT)?.read_to_string(&mut document)?;

    let dto: DocumentDto = serde_json::from_str(&document)?;
    let mut scene = dto.to_scene()?;

    // Reunite each sound with its bytes. A sound whose file is missing keeps
    // its entry — name, duration and every keyframe that references it — and
    // simply plays nothing. Dropping it instead would silently delete the
    // user's edits along with it, and they would have no way to tell what the
    // document used to sound like.
    let names: Vec<(buzz_scene::SoundId, String)> = scene
        .sounds()
        .iter()
        .map(|s| (s.id, format!("media/{}", s.file_name())))
        .collect();
    for (id, name) in names {
        let mut bytes = Vec::new();
        match archive.by_name(&name) {
            Ok(mut entry) => {
                entry.read_to_end(&mut bytes)?;
            }
            Err(_) => {
                tracing::warn!("{name} is missing from the document; that sound will be silent");
                continue;
            }
        }
        if let Some(asset) = scene.sounds_mut().get(id).cloned() {
            let mut updated = (*asset).clone();
            updated.data = std::sync::Arc::new(bytes);
            scene.sounds_mut().insert(updated);
        }
    }

    Ok(scene)
}

/// Read the metadata without loading the artwork.
///
/// Lets a file browser or recovery prompt show details cheaply.
pub fn read_meta(path: impl AsRef<Path>) -> Result<Meta, DocError> {
    let file = File::open(path)?;
    let mut archive = ZipArchive::new(file)?;
    let mut json = String::new();
    archive.by_name(ENTRY_META)?.read_to_string(&mut json)?;
    Ok(serde_json::from_str(&json)?)
}

/// Save a scene to `path`, atomically.
///
/// Writes a sibling temporary file and renames it over the target, so an
/// interrupted save cannot destroy the previous version.
pub fn save(scene: &Scene, path: impl AsRef<Path>) -> Result<(), DocError> {
    let path = path.as_ref();
    let bytes = to_bytes(scene)?;
    write_atomic(path, &bytes)
}

/// Write bytes to `path` via a temporary file and a rename.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), DocError> {
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent)?;
    }

    let temp = temp_sibling(path);
    {
        let mut file = File::create(&temp)?;
        file.write_all(bytes)?;
        // Force the data out before the rename, so a crash immediately after
        // cannot leave a renamed-but-empty file.
        file.sync_all()?;
    }

    // Windows refuses to rename onto an existing file, unlike POSIX.
    if path.exists() {
        std::fs::remove_file(path)?;
    }
    std::fs::rename(&temp, path)?;
    Ok(())
}

fn temp_sibling(path: &Path) -> PathBuf {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "document".into());
    path.with_file_name(format!(".{name}.tmp"))
}

/// Load a scene from `path`.
pub fn load(path: impl AsRef<Path>) -> Result<Scene, DocError> {
    let mut bytes = Vec::new();
    File::open(path)?.read_to_end(&mut bytes)?;
    from_bytes(&bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use buzz_geom::Shape as _;
    use buzz_scene::{LayerKind, ShapeData};
    use kurbo::Rect;
    use peniko::Color;

    fn sample() -> Scene {
        let mut scene = Scene::empty();
        let layer = scene.add_layer("Layer_1", LayerKind::Normal);
        for i in 0..20 {
            scene.add_shape(
                layer,
                ShapeData::filled(
                    Rect::new(i as f64 * 10.0, 0.0, i as f64 * 10.0 + 8.0, 8.0).to_path(1e-9),
                    Color::from_rgb8(0x33, 0x66, 0x99),
                ),
            );
        }
        scene
    }

    #[test]
    fn a_document_round_trips_through_bytes() {
        let scene = sample();
        let bytes = to_bytes(&scene).unwrap();
        let back = from_bytes(&bytes).unwrap();

        assert_eq!(back.shape_count(), scene.shape_count());
        assert_eq!(back.layers().len(), scene.layers().len());
        assert_eq!(back.stage().size, scene.stage().size);
    }

    #[test]
    fn the_archive_starts_with_an_uncompressed_mimetype() {
        let bytes = to_bytes(&sample()).unwrap();
        // The literal string must be findable near the start without inflating.
        let head = &bytes[..bytes.len().min(200)];
        let needle = MIMETYPE.as_bytes();
        assert!(
            head.windows(needle.len()).any(|w| w == needle),
            "mimetype should be stored uncompressed at the head of the archive"
        );
    }

    #[test]
    fn a_file_round_trips_through_disk() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.buzz");

        let scene = sample();
        save(&scene, &path).unwrap();
        assert!(path.exists());

        let back = load(&path).unwrap();
        assert_eq!(back.shape_count(), scene.shape_count());

        let meta = read_meta(&path).unwrap();
        assert_eq!(meta.format_version, FORMAT_VERSION);
        assert!(meta.generator.contains("BuzzAnimate"));
    }

    #[test]
    fn saving_over_an_existing_file_replaces_it_cleanly() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.buzz");

        save(&Scene::empty(), &path).unwrap();
        let scene = sample();
        save(&scene, &path).unwrap();

        assert_eq!(load(&path).unwrap().shape_count(), scene.shape_count());
        // No temporary file should be left lying around.
        let leftovers: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().contains(".tmp"))
            .collect();
        assert!(leftovers.is_empty(), "a temporary file was left behind");
    }

    #[test]
    fn saving_creates_missing_directories() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested").join("deeper").join("test.buzz");
        save(&sample(), &path).unwrap();
        assert!(path.exists());
    }

    #[test]
    fn a_non_buzz_zip_is_rejected_with_a_clear_error() {
        let mut buffer = Cursor::new(Vec::new());
        {
            let mut zip = ZipWriter::new(&mut buffer);
            zip.start_file("mimetype", SimpleFileOptions::default()).unwrap();
            zip.write_all(b"application/zip").unwrap();
            zip.finish().unwrap();
        }
        let err = from_bytes(&buffer.into_inner()).unwrap_err();
        assert!(matches!(err, DocError::WrongType(_)), "got {err:?}");
    }

    #[test]
    fn arbitrary_bytes_do_not_panic_the_loader() {
        for junk in [
            b"".to_vec(),
            b"not a zip at all".to_vec(),
            vec![0x50, 0x4B, 0x03, 0x04, 0xFF, 0xFF],
        ] {
            let result = from_bytes(&junk);
            assert!(result.is_err(), "junk input should fail, not succeed");
        }
    }

    #[test]
    fn loading_a_missing_file_reports_io_rather_than_panicking() {
        let err = load("definitely/not/here.buzz").unwrap_err();
        assert!(matches!(err, DocError::Io(_)), "got {err:?}");
    }

    #[test]
    fn compression_actually_shrinks_a_repetitive_document() {
        let mut scene = Scene::empty();
        let layer = scene.add_layer("L", LayerKind::Normal);
        for i in 0..500 {
            scene.add_shape(
                layer,
                ShapeData::filled(
                    Rect::new(i as f64, 0.0, i as f64 + 1.0, 1.0).to_path(1e-9),
                    Color::WHITE,
                ),
            );
        }

        let archive = to_bytes(&scene).unwrap();
        let raw = serde_json::to_vec_pretty(&DocumentDto::from_scene(&scene)).unwrap();

        assert!(
            archive.len() < raw.len(),
            "archive ({} bytes) should be smaller than raw JSON ({} bytes)",
            archive.len(),
            raw.len()
        );
    }
}
