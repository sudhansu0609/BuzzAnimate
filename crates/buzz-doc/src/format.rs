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

use buzz_scene::{LayerId, Scene};
use serde::{Deserialize, Serialize};
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipArchive, ZipWriter};

use crate::sequence::{SceneKey, SceneRole};
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
///
/// The sequence-setups fields (`keys`, `roles`, `sequence`, `master_audio`)
/// live here rather than in each scene's own `document.json` (`serial.rs`):
/// they are facts about the *document* — which scene is which, and the order
/// they cut in — not about any one scene's artwork. All default to "this
/// document has never heard of setups and shots": every scene `Normal`, keys
/// assigned by position, an empty sequence, no master audio. That is exactly
/// what every file written before this existed means.
#[derive(serde::Serialize, serde::Deserialize)]
struct ScenesDto {
    active: usize,
    names: Vec<String>,
    /// Stable per-scene identity, parallel to `names`. Rebuilt by position
    /// when absent, or when its length disagrees with `names` — belt and
    /// braces against a hand-edited or partially-old file.
    #[serde(default)]
    keys: Vec<SceneKey>,
    /// What kind of scene each one is, parallel to `names`.
    #[serde(default)]
    roles: Vec<SceneRole>,
    /// The sequence's cut: shot keys, in film order. A shot's key not listed
    /// here is a take not in the cut.
    #[serde(default)]
    sequence: Vec<SceneKey>,
    /// Where the master audio's sound keyframe lives: which scene, and which
    /// layer on it. Not a bare sound reference — a sound id only means
    /// anything inside the `SoundLibrary` of the scene that holds it.
    #[serde(default)]
    master_audio: Option<(SceneKey, LayerId)>,
    /// Master Audio mode: only the master clip may play anywhere. Absent
    /// means off, which is every file written before it existed.
    #[serde(default)]
    master_audio_mode: bool,
}

/// One scene plus the manifest metadata that travels with it: its stable key
/// and what kind of scene it is. What [`to_bytes_scenes_full`] and
/// [`save_scenes_full`] take instead of a bare `(name, Scene)` pair, so a
/// document with setups and shots round-trips them along with the artwork.
pub struct SceneRecord<'a> {
    pub name: &'a str,
    pub scene: &'a Scene,
    pub key: SceneKey,
    pub role: SceneRole,
}

/// Every scene read back from a `.buzz` archive, with the document-level
/// metadata that travelled alongside them in `scenes.json`. What
/// [`from_bytes_scenes_full`] and [`load_scenes_full`] return.
pub struct LoadedScenes {
    pub scenes: Vec<(String, Scene)>,
    pub active: usize,
    /// Parallel to `scenes`, one key per entry.
    pub keys: Vec<SceneKey>,
    /// Parallel to `scenes`, one role per entry.
    pub roles: Vec<SceneRole>,
    pub sequence: Vec<SceneKey>,
    pub master_audio: Option<(SceneKey, LayerId)>,
    pub master_audio_mode: bool,
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
///
/// Every scene comes back `Normal` with keys assigned by position — this is
/// what autosave and crash recovery use, which know nothing of setups and
/// shots. [`to_bytes_scenes_full`] carries roles, keys, the sequence and the
/// master audio too.
pub fn to_bytes_scenes(scenes: &[(&str, &Scene)], active: usize) -> Result<Vec<u8>, DocError> {
    let records: Vec<SceneRecord> = scenes
        .iter()
        .enumerate()
        .map(|(i, (name, scene))| SceneRecord {
            name,
            scene,
            key: SceneKey(i as u32),
            role: SceneRole::Normal,
        })
        .collect();
    to_bytes_scenes_full(&records, active, &[], None, false)
}

/// Serialise several scenes, with their roles and stable keys, the
/// sequence's cut and the master audio, into one `.buzz` archive.
pub fn to_bytes_scenes_full(
    scenes: &[SceneRecord],
    active: usize,
    sequence: &[SceneKey],
    master_audio: Option<(SceneKey, LayerId)>,
    master_audio_mode: bool,
) -> Result<Vec<u8>, DocError> {
    let mut buffer = Cursor::new(Vec::new());
    write_archive(&mut buffer, scenes, active, sequence, master_audio, master_audio_mode)?;
    Ok(buffer.into_inner())
}

fn write_archive<W: Write + Seek>(
    writer: &mut W,
    scenes: &[SceneRecord],
    active: usize,
    sequence: &[SceneKey],
    master_audio: Option<(SceneKey, LayerId)>,
    master_audio_mode: bool,
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

    // The scene index — only when there is more than one, or when there is
    // some other non-default fact to remember, so a plain single-scene
    // document stays exactly the file it was and opens in older versions.
    let needs_manifest = scenes.len() > 1
        || scenes.iter().any(|s| !matches!(s.role, SceneRole::Normal))
        || !sequence.is_empty()
        || master_audio.is_some()
        || master_audio_mode;
    if needs_manifest {
        let index = ScenesDto {
            active,
            names: scenes.iter().map(|s| s.name.to_string()).collect(),
            keys: scenes.iter().map(|s| s.key).collect(),
            roles: scenes.iter().map(|s| s.role.clone()).collect(),
            sequence: sequence.to_vec(),
            master_audio,
            master_audio_mode,
        };
        zip.start_file(ENTRY_SCENES, deflated)?;
        zip.write_all(&serde_json::to_vec_pretty(&index)?)?;
    }

    let stored = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
    for (i, record) in scenes.iter().enumerate() {
        zip.start_file(scene_entry(i), deflated)?;
        let dto = DocumentDto::from_scene(record.scene);
        zip.write_all(&serde_json::to_vec_pretty(&dto)?)?;
        write_media(&mut zip, record.scene, &media_prefix(i), stored)?;
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
///
/// Discards roles, keys, the sequence and the master audio — for autosave and
/// crash recovery, which only ever restore names and artwork. See
/// [`from_bytes_scenes_full`] for the rest.
pub fn from_bytes_scenes(bytes: &[u8]) -> Result<(Vec<(String, Scene)>, usize), DocError> {
    let loaded = from_bytes_scenes_full(bytes)?;
    Ok((loaded.scenes, loaded.active))
}

/// Read every scene from `.buzz` bytes, with the document-level metadata
/// (roles, stable keys, the sequence's cut, the master audio) that travelled
/// alongside them in `scenes.json`. A file with no scene index at all — every
/// file older versions wrote — comes back as one `Normal` scene named
/// "Scene 1", key `0`, no sequence, no master audio.
pub fn from_bytes_scenes_full(bytes: &[u8]) -> Result<LoadedScenes, DocError> {
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

    let (names, active, keys, roles, sequence, master_audio, master_audio_mode) = match archive.by_name(ENTRY_SCENES)
    {
        Ok(mut file) => {
            let mut json = String::new();
            file.read_to_string(&mut json)?;
            let index: ScenesDto = serde_json::from_str(&json)?;
            // A key or role list that disagrees in length with `names` is
            // treated the same as an absent one: made up by position rather
            // than trusted partially.
            let keys = if index.keys.len() == index.names.len() {
                index.keys
            } else {
                (0..index.names.len() as u32).map(SceneKey).collect()
            };
            let roles = if index.roles.len() == index.names.len() {
                index.roles
            } else {
                vec![SceneRole::Normal; index.names.len()]
            };
            (
                index.names,
                index.active,
                keys,
                roles,
                index.sequence,
                index.master_audio,
                index.master_audio_mode,
            )
        }
        Err(_) => (
            vec!["Scene 1".to_string()],
            0,
            vec![SceneKey(0)],
            vec![SceneRole::Normal],
            Vec::new(),
            None,
            false,
        ),
    };

    let mut scenes = Vec::with_capacity(names.len());
    for (i, name) in names.into_iter().enumerate() {
        let scene = read_scene(&mut archive, &scene_entry(i), &media_prefix(i))?;
        scenes.push((name, scene));
    }
    let active = active.min(scenes.len().saturating_sub(1));
    Ok(LoadedScenes {
        scenes,
        active,
        keys,
        roles,
        sequence,
        master_audio,
        master_audio_mode,
    })
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

/// Save several scenes, with their roles and stable keys, the sequence's cut
/// and the master audio, to `path`, atomically.
pub fn save_scenes_full(
    scenes: &[SceneRecord],
    active: usize,
    sequence: &[SceneKey],
    master_audio: Option<(SceneKey, LayerId)>,
    master_audio_mode: bool,
    path: impl AsRef<Path>,
) -> Result<(), DocError> {
    let bytes = to_bytes_scenes_full(scenes, active, sequence, master_audio, master_audio_mode)?;
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

/// Load every scene from `path`, with the document-level metadata (roles,
/// stable keys, the sequence's cut, the master audio) that travelled
/// alongside them.
pub fn load_scenes_full(path: impl AsRef<Path>) -> Result<LoadedScenes, DocError> {
    let mut bytes = Vec::new();
    File::open(path)?.read_to_end(&mut bytes)?;
    from_bytes_scenes_full(&bytes)
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
            back[1].1.layers().iter().any(|l| l.name == "Only here"),
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

    /// **Roles, stable keys, the sequence's cut and the master audio all
    /// survive a save and reopen**, alongside the names and artwork they
    /// travel with.
    #[test]
    fn roles_keys_sequence_and_master_audio_round_trip_through_the_container() {
        use crate::sequence::ShotInfo;

        let setup = sample();
        let mut shot = Scene::empty();
        let layer = shot.add_layer("Only here", LayerKind::Normal);
        shot.add_shape(
            layer,
            ShapeData::filled(Rect::new(0.0, 0.0, 4.0, 4.0).to_path(1e-9), Color::WHITE),
        );

        let setup_key = SceneKey(7);
        let shot_key = SceneKey(9);
        let shot_info = ShotInfo {
            from_setup: Some(setup_key),
            setup_revision: 3,
            audio_in: 10,
            audio_out: 70,
            take_of: None,
            continue_from: None,
            dialogue_layer: None,
        };
        let records = [
            SceneRecord {
                name: "Village",
                scene: &setup,
                key: setup_key,
                role: SceneRole::Setup,
            },
            SceneRecord {
                name: "Village – Shot 1",
                scene: &shot,
                key: shot_key,
                role: SceneRole::Shot(shot_info),
            },
        ];
        let master_audio = Some((shot_key, buzz_scene::LayerId(3)));
        let bytes =
            to_bytes_scenes_full(&records, 1, &[shot_key], master_audio, false).unwrap();


        let loaded = from_bytes_scenes_full(&bytes).unwrap();
        assert_eq!(loaded.active, 1);
        assert_eq!(loaded.scenes.len(), 2);
        assert_eq!(loaded.scenes[0].0, "Village");
        assert_eq!(loaded.scenes[1].0, "Village – Shot 1");
        assert_eq!(loaded.keys, vec![setup_key, shot_key]);
        assert_eq!(loaded.roles[0], SceneRole::Setup);
        assert_eq!(loaded.roles[1], SceneRole::Shot(shot_info));
        assert_eq!(loaded.sequence, vec![shot_key]);
        assert_eq!(loaded.master_audio, master_audio);
    }

    /// A manifest written before roles, keys, a sequence or master audio
    /// existed — just `active` and `names`, exactly what every file older
    /// versions wrote — still opens: every scene comes back `Normal`, with
    /// keys assigned by its position in the file.
    #[test]
    fn an_old_manifest_with_only_names_opens_as_all_normal_with_keys_by_position() {
        let first = sample();
        let mut second = Scene::empty();
        second.add_layer("Only here", LayerKind::Normal);

        let mut bytes = Vec::new();
        {
            let mut zip = ZipWriter::new(Cursor::new(&mut bytes));
            zip.start_file::<_, ()>(
                "mimetype",
                SimpleFileOptions::default().compression_method(CompressionMethod::Stored),
            )
            .unwrap();
            zip.write_all(MIMETYPE.as_bytes()).unwrap();

            let deflated =
                SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
            zip.start_file::<_, ()>("meta.json", deflated).unwrap();
            zip.write_all(&serde_json::to_vec(&Meta::default()).unwrap())
                .unwrap();

            // The old shape: only `active` and `names`, nothing this stage
            // added.
            zip.start_file::<_, ()>("scenes.json", deflated).unwrap();
            zip.write_all(br#"{"active":1,"names":["Opening","Chase"]}"#)
                .unwrap();

            zip.start_file::<_, ()>("document.json", deflated).unwrap();
            zip.write_all(&serde_json::to_vec(&DocumentDto::from_scene(&first)).unwrap())
                .unwrap();
            zip.start_file::<_, ()>("scene-1.json", deflated).unwrap();
            zip.write_all(&serde_json::to_vec(&DocumentDto::from_scene(&second)).unwrap())
                .unwrap();
            zip.finish().unwrap();
        }

        let loaded = from_bytes_scenes_full(&bytes).unwrap();
        assert_eq!(loaded.scenes.len(), 2);
        assert_eq!(loaded.active, 1);
        assert_eq!(
            loaded.keys,
            vec![SceneKey(0), SceneKey(1)],
            "keys should be made up by position"
        );
        assert_eq!(
            loaded.roles,
            vec![SceneRole::Normal, SceneRole::Normal],
            "every scene from an old file is Normal"
        );
        assert!(loaded.sequence.is_empty());
        assert!(loaded.master_audio.is_none());
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

    /// **The reported case, through the real container**: a rig keyed many
    /// times saves as about one copy of its artwork, not one per key, and
    /// comes back scene-for-scene equal.
    #[test]
    fn a_rig_keyed_many_times_saves_as_one_copy_of_its_artwork() {
        use crate::serial::test_support::keyed_rig_scene;

        let one = to_bytes(&keyed_rig_scene(1, 3000)).unwrap();
        let many_scene = keyed_rig_scene(100, 3000);
        let many = to_bytes(&many_scene).unwrap();
        assert!(
            many.len() < one.len() * 3,
            "100 keys took {} bytes against {} for one",
            many.len(),
            one.len()
        );

        let back = from_bytes(&many).unwrap();
        let before: Vec<_> = many_scene.stage_layers().iter().collect();
        let after: Vec<_> = back.stage_layers().iter().collect();
        assert_eq!(before.len(), after.len());
        for (a, b) in before.iter().zip(&after) {
            assert_eq!(**a, **b, "a layer changed on the way through the file");
        }
    }

    /// A `.buzz` written before the part table, every key carrying its rig in
    /// full, still opens and is the same scene.
    #[test]
    fn an_archive_with_the_rig_written_in_every_key_still_opens() {
        use crate::serial::test_support::{as_version_45, keyed_rig_scene};

        let scene = keyed_rig_scene(30, 200);
        let old_document =
            serde_json::to_vec_pretty(&as_version_45(DocumentDto::from_scene(&scene))).unwrap();

        let mut bytes = Vec::new();
        {
            let mut zip = ZipWriter::new(Cursor::new(&mut bytes));
            zip.start_file::<_, ()>(
                "mimetype",
                SimpleFileOptions::default().compression_method(CompressionMethod::Stored),
            )
            .unwrap();
            zip.write_all(MIMETYPE.as_bytes()).unwrap();
            let deflated =
                SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
            zip.start_file::<_, ()>("meta.json", deflated).unwrap();
            zip.write_all(&serde_json::to_vec(&Meta::default()).unwrap())
                .unwrap();
            zip.start_file::<_, ()>("document.json", deflated).unwrap();
            zip.write_all(&old_document).unwrap();
            zip.finish().unwrap();
        }

        let opened = from_bytes(&bytes).unwrap();
        let before: Vec<_> = scene.stage_layers().iter().collect();
        let after: Vec<_> = opened.stage_layers().iter().collect();
        assert_eq!(before.len(), after.len());
        for (a, b) in before.iter().zip(&after) {
            assert_eq!(**a, **b);
        }
        // And saved again it is the smaller layout.
        assert!(to_bytes(&opened).unwrap().len() < bytes.len());
    }
}
