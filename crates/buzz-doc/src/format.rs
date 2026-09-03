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
const ENTRY_SCENES: &str = "scenes.json";

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
    to_bytes_scenes(&[("Scene 1", scene)], 0)
}

/// The scene index for a multi-scene document: the scene names and which one
/// was active. Absent from a single-scene file — which is every file older
/// versions wrote — so its absence means "one scene".
#[derive(serde::Serialize, serde::Deserialize)]
struct ScenesDto {
    active: usize,
    names: Vec<String>,
}

/// The archive entry holding one scene's document JSON, and the media directory
/// its sounds and images live under. Scene 0 keeps the original names
/// (`document.json`, `media/`) so a single-scene file is byte-for-byte what it
/// always was and older readers still open it; later scenes are numbered.
fn scene_entry(index: usize) -> String {
    if index == 0 {
        ENTRY_DOCUMENT.to_string()
    } else {
        format!("scene-{index}.json")
    }
}

fn media_prefix(index: usize) -> String {
    if index == 0 {
        "media/".to_string()
    } else {
        format!("media/s{index}/")
    }
}

/// Serialise several named scenes into one `.buzz` archive.
pub fn to_bytes_scenes(scenes: &[(&str, &Scene)], active: usize) -> Result<Vec<u8>, DocError> {
    let mut buffer = Cursor::new(Vec::new());
    write_archive(&mut buffer, scenes, active)?;
    Ok(buffer.into_inner())
}

fn write_archive<W: Write + Seek>(
    writer: &mut W,
    scenes: &[(&str, &Scene)],
    active: usize,
) -> Result<(), DocError> {
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

    // The scene index — only when there is more than one, so a single-scene
    // document stays exactly the file it was and opens in older versions.
    if scenes.len() > 1 {
        let index = ScenesDto {
            active,
            names: scenes.iter().map(|(name, _)| name.to_string()).collect(),
        };
        zip.start_file(ENTRY_SCENES, deflated)?;
        zip.write_all(&serde_json::to_vec_pretty(&index)?)?;
    }

    let stored = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
    for (i, (_, scene)) in scenes.iter().enumerate() {
        zip.start_file(scene_entry(i), deflated)?;
        let dto = DocumentDto::from_scene(scene);
        zip.write_all(&serde_json::to_vec_pretty(&dto)?)?;
        write_media(&mut zip, scene, &media_prefix(i), stored)?;
    }

    zip.finish()?;
    Ok(())
}

/// Write a scene's sounds and images into the archive under `prefix`.
///
/// **Stored, not deflated.** MP3 and compressed WAV do not compress again;
/// deflating them costs time on every save and gives back a fraction of a
/// percent, and an uncompressed entry unzips to a playable file directly. A
/// painted bitmap that cannot be written is reported and skipped rather than
/// losing the whole save — the artwork keeps its place and shows a grey fill.
fn write_media<W: Write + Seek>(
    zip: &mut ZipWriter<W>,
    scene: &Scene,
    prefix: &str,
    stored: SimpleFileOptions,
) -> Result<(), DocError> {
    for sound in scene.sounds().iter() {
        zip.start_file(format!("{prefix}{}", sound.file_name()), stored)?;
        zip.write_all(&sound.data)?;
    }
    for image in scene.images().iter() {
        // A procedural texture travels as its recipe in the document JSON, so
        // there is nothing to put in the archive for it — see `ImageAssetDto`.
        if image.recipe.is_some() {
            continue;
        }
        match image.bytes_for_storage() {
            Ok(bytes) => {
                zip.start_file(format!("{prefix}{}", image.file_name()), stored)?;
                zip.write_all(&bytes)?;
            }
            Err(e) => tracing::warn!("could not store {}: {e}", image.name),
        }
    }
    Ok(())
}

/// Read the first (or only) scene from `.buzz` bytes.
pub fn from_bytes(bytes: &[u8]) -> Result<Scene, DocError> {
    Ok(from_bytes_scenes(bytes)?
        .0
        .into_iter()
        .next()
        .map(|(_, scene)| scene)
        .unwrap_or_else(Scene::empty))
}

/// Read every named scene from `.buzz` bytes, and which one was active. A file
/// with no scene index is a single-scene document — every file older versions
/// wrote — and comes back as one scene named "Scene 1".
pub fn from_bytes_scenes(bytes: &[u8]) -> Result<(Vec<(String, Scene)>, usize), DocError> {
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

    let (names, active): (Vec<String>, usize) = match archive.by_name(ENTRY_SCENES) {
        Ok(mut file) => {
            let mut json = String::new();
            file.read_to_string(&mut json)?;
            let index: ScenesDto = serde_json::from_str(&json)?;
            (index.names, index.active)
        }
        Err(_) => (vec!["Scene 1".to_string()], 0),
    };

    let mut scenes = Vec::with_capacity(names.len());
    for (i, name) in names.into_iter().enumerate() {
        let scene = read_scene(&mut archive, &scene_entry(i), &media_prefix(i))?;
        scenes.push((name, scene));
    }
    let active = active.min(scenes.len().saturating_sub(1));
    Ok((scenes, active))
}

/// Read one scene's document JSON and the media under `prefix`.
fn read_scene<R: Read + Seek>(
    archive: &mut ZipArchive<R>,
    doc_entry: &str,
    prefix: &str,
) -> Result<Scene, DocError> {
    let mut document = String::new();
    archive.by_name(doc_entry)?.read_to_string(&mut document)?;
    let dto: DocumentDto = serde_json::from_str(&document)?;

    // **Bitmaps are decoded before the scene is built**, because a fill refers
    // to one by id and resolves against it as the layers are read. Sounds can
    // be reunited afterwards — nothing points *into* them — but an image fill
    // with no image is a shape with no picture, so the library has to exist
    // first.
    let mut images = buzz_scene::ImageLibrary::default();
    for entry in &dto.images {
        // A procedural texture has no media entry to read: the recipe is the
        // file, and `to_scene_with_images` bakes it. A recipe naming a kind
        // this build does not know falls through to the media path and, finding
        // nothing, warns like any other missing image.
        if entry.recipe.as_ref().and_then(|r| r.to_recipe()).is_some() {
            continue;
        }
        let name = format!("{prefix}image-{}.{}", entry.id, entry.format);
        let mut bytes = Vec::new();
        match archive.by_name(&name) {
            Ok(mut file) => file.read_to_end(&mut bytes)?,
            Err(_) => {
                tracing::warn!("{name} is missing from the document; that image will be blank");
                continue;
            }
        };
        match buzz_scene::ImageAsset::decode(
            buzz_scene::ImageId(entry.id),
            entry.name.clone(),
            &bytes,
        ) {
            Ok(mut asset) => {
                // Whether it was painted is in the document, not in the PNG:
                // the decoder cannot know, and a stroke has to still be paint
                // when the file is reopened or it would stop fusing.
                asset.painted = entry.painted;
                images.insert(asset);
            }
            Err(e) => tracing::warn!("{name} could not be decoded: {e}"),
        }
    }

    let mut scene = dto.to_scene_with_images(images)?;

    // Reunite each sound with its bytes. A sound whose file is missing keeps
    // its entry — name, duration and every keyframe that references it — and
    // simply plays nothing. Dropping it instead would silently delete the
    // user's edits along with it.
    let names: Vec<(buzz_scene::SoundId, String)> = scene
        .sounds()
        .iter()
        .map(|s| (s.id, format!("{prefix}{}", s.file_name())))
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
    save_scenes(&[("Scene 1", scene)], 0, path)
}

/// Save several named scenes to `path`, atomically.
pub fn save_scenes(
    scenes: &[(&str, &Scene)],
    active: usize,
    path: impl AsRef<Path>,
) -> Result<(), DocError> {
    let bytes = to_bytes_scenes(scenes, active)?;
    write_atomic(path.as_ref(), &bytes)
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

/// Load the first (or only) scene from `path`.
pub fn load(path: impl AsRef<Path>) -> Result<Scene, DocError> {
    let mut bytes = Vec::new();
    File::open(path)?.read_to_end(&mut bytes)?;
    from_bytes(&bytes)
}

/// Load every named scene from `path`, and which one was active.
pub fn load_scenes(path: impl AsRef<Path>) -> Result<(Vec<(String, Scene)>, usize), DocError> {
    let mut bytes = Vec::new();
    File::open(path)?.read_to_end(&mut bytes)?;
    from_bytes_scenes(&bytes)
}

#[cfg(test)]
mod tests {
    /// **A bitmap survives a save and reopen**, pixels and placement both.
    ///
    /// The picture goes into `media/` once and the fill refers to it by id, so
    /// this is really two claims: that the file comes back byte-identical, and
    /// that the shape which was filled with it still is.
    #[test]
    fn a_bitmap_round_trips_through_the_container() {
        use buzz_scene::{FillSpec, ImageAsset, ImageFill, ImageId, LayerKind, ShapeData};

        // A small painted canvas, encoded as a real PNG.
        let mut canvas = ImageAsset::blank(ImageId(7), "Canvas", 8, 6);
        {
            let pixels = std::sync::Arc::make_mut(&mut canvas.pixels);
            for (i, chunk) in pixels.chunks_exact_mut(4).enumerate() {
                chunk.copy_from_slice(&[(i * 7) as u8, (i * 3) as u8, 200, 255]);
            }
        }
        let png = canvas.encode_png().expect("it encodes");
        let asset = ImageAsset::decode(ImageId(7), "Canvas", &png).expect("it decodes");
        let (width, height) = (asset.width, asset.height);
        let sample = asset.pixel(3, 2);

        let mut scene = Scene::empty();
        let stored = scene.images_mut().insert(asset);
        let layer = scene.add_layer("Art", LayerKind::Normal);
        let area = buzz_geom::Rect::new(40.0, 20.0, 140.0, 95.0);
        scene
            .add_shape(
                layer,
                ShapeData {
                    path: buzz_geom::Shape::to_path(&area, 1e-9),
                    fill: Some(FillSpec::image(ImageFill::new(stored, area))),
                    stroke: None,
                    blend: buzz_scene::PaintBlend::Normal,
                },
            )
            .expect("the image shape");

        let bytes = to_bytes(&scene).expect("it saves");
        let back = from_bytes(&bytes).expect("it opens");

        assert_eq!(back.images().len(), 1, "the bitmap was lost");
        let reopened = back
            .images()
            .get(ImageId(7))
            .expect("the bitmap should still be there");
        assert_eq!((reopened.width, reopened.height), (width, height));
        assert_eq!(
            reopened.pixel(3, 2),
            sample,
            "the pixels changed on the way through"
        );

        // And the shape still refers to it, in the same place.
        let fill = back
            .layers()
            .iter()
            .flat_map(|l| l.objects_at(0))
            .find_map(|o| match &o.kind {
                buzz_scene::ObjectKind::Shape(shape) => {
                    shape.fill.as_ref().and_then(|f| f.paint.image().cloned())
                }
                _ => None,
            })
            .expect("the fill should still be an image");
        assert_eq!(fill.asset.id, ImageId(7));
        let placed = fill.transform * buzz_geom::Point::new(0.0, 0.0);
        assert!(
            (placed.x - 40.0).abs() < 1e-9 && (placed.y - 20.0).abs() < 1e-9,
            "the picture moved: {placed:?}"
        );
    }

    /// A document whose `media/` entry has gone keeps the artwork and loses
    /// only the picture. Refusing to open would throw away everything else.
    #[test]
    fn a_missing_bitmap_leaves_the_artwork_standing() {
        use buzz_scene::{FillSpec, ImageAsset, ImageFill, ImageId, LayerKind, ShapeData};

        let canvas = ImageAsset::blank(ImageId(9), "Gone", 4, 4);
        let png = canvas.encode_png().expect("encodes");
        let asset = ImageAsset::decode(ImageId(9), "Gone", &png).expect("decodes");

        let mut scene = Scene::empty();
        let stored = scene.images_mut().insert(asset);
        let layer = scene.add_layer("Art", LayerKind::Normal);
        let area = buzz_geom::Rect::new(0.0, 0.0, 50.0, 50.0);
        scene
            .add_shape(
                layer,
                ShapeData {
                    path: buzz_geom::Shape::to_path(&area, 1e-9),
                    fill: Some(FillSpec::image(ImageFill::new(stored, area))),
                    stroke: None,
                    blend: buzz_scene::PaintBlend::Normal,
                },
            )
            .expect("the shape");

        // Save, then rebuild the archive without the media entry.
        let bytes = to_bytes(&scene).expect("saves");
        let mut stripped = Vec::new();
        {
            let mut source = zip::ZipArchive::new(Cursor::new(&bytes)).expect("readable");
            let mut out = zip::ZipWriter::new(Cursor::new(&mut stripped));
            for i in 0..source.len() {
                let mut entry = source.by_index(i).expect("an entry");
                let name = entry.name().to_string();
                if name.starts_with("media/") {
                    continue;
                }
                let mut data = Vec::new();
                entry.read_to_end(&mut data).expect("readable");
                out.start_file::<_, ()>(name, SimpleFileOptions::default())
                    .expect("writable");
                out.write_all(&data).expect("written");
            }
            out.finish().expect("finished");
        }

        let back = from_bytes(&stripped).expect("it should still open");
        let shapes: usize = back
            .layers()
            .iter()
            .flat_map(|l| l.objects_at(0))
            .filter(|o| matches!(o.kind, buzz_scene::ObjectKind::Shape(_)))
            .count();
        assert_eq!(shapes, 1, "the artwork was thrown away with the picture");
    }

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

    /// **Every scene survives a save and reopen**, in order, with its name and
    /// its own artwork — and the file remembers which one was active.
    #[test]
    fn several_scenes_round_trip_with_their_names() {
        let mut second = Scene::empty();
        let layer = second.add_layer("Only here", LayerKind::Normal);
        second.add_shape(
            layer,
            ShapeData::filled(Rect::new(0.0, 0.0, 4.0, 4.0).to_path(1e-9), Color::WHITE),
        );

        let first = sample();
        let scenes = [("Opening", &first), ("Chase", &second)];
        let bytes = to_bytes_scenes(&scenes, 1).unwrap();
        let (back, active) = from_bytes_scenes(&bytes).unwrap();
        assert_eq!(active, 1, "the active scene was not remembered");

        assert_eq!(back.len(), 2, "a scene was lost");
        assert_eq!(back[0].0, "Opening");
        assert_eq!(back[1].0, "Chase");
        assert_eq!(back[0].1.shape_count(), first.shape_count());
        assert_eq!(back[1].1.shape_count(), 1);
        assert!(
            back[1]
                .1
                .layers()
                .iter()
                .any(|l| l.name == "Only here"),
            "the second scene's layers were mixed up with the first's"
        );
    }

    /// A single-scene file — every file written before scenes existed — opens
    /// as one scene named "Scene 1". Backward compatibility.
    #[test]
    fn an_old_single_scene_file_opens_as_one_scene() {
        let scene = sample();
        let bytes = to_bytes(&scene).unwrap();
        let (back, active) = from_bytes_scenes(&bytes).unwrap();

        assert_eq!(back.len(), 1);
        assert_eq!(active, 0);
        assert_eq!(back[0].0, "Scene 1");
        assert_eq!(back[0].1.shape_count(), scene.shape_count());
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
            zip.start_file("mimetype", SimpleFileOptions::default())
                .unwrap();
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
