//! Import Adobe Animate documents.
//!
//! Handles both shapes Animate saves in:
//!
//! * **`.fla`** — a zip container holding an XFL structure. This is what
//!   Animate has written since CS5.
//! * **`.xfl`** — the same structure as a folder on disk, which is what you
//!   get from "Save as Uncompressed Document".
//!
//! Legacy `.fla` files from CS4 and earlier are OLE2 compound documents with
//! undocumented binary records; those are detected and refused with a clear
//! message rather than parsed into nonsense.
//!
//! # Layout
//!
//! ```text
//! DOMDocument.xml     stage, timelines, layers, frames
//! LIBRARY/*.xml       one file per symbol
//! bin/                binary media (bitmaps, audio)
//! ```
//!
//! # Fidelity
//!
//! Animate has two decades of features and this reads the structural core:
//! stage properties, layers and their types, keyframes and spans, shapes,
//! groups, symbol instances and tweens. Anything not understood is **recorded
//! in an [`ImportReport`] rather than silently dropped**, so a user can see
//! what did not come across instead of hunting for it.

pub mod edge;

use std::collections::HashMap;
use std::io::Read;
use std::path::Path;

use buzz_geom::Affine;
use buzz_scene::{
    FillSpec, Gradient, GradientKind, GradientSpread, GradientStop, Layer, LayerId, LayerKind,
    Object, ObjectId, Paint, Scene, ShapeData, SoundId, SoundRef, SoundSync, StrokeSpec, Symbol,
    SymbolId, SymbolKind, Tween,
};
use peniko::Color;
use quick_xml::events::Event;

pub use edge::{EdgeError, parse_edges, parse_edges_closed};

/// Why an import failed outright.
#[derive(Debug, thiserror::Error)]
pub enum ImportError {
    #[error("input/output error: {0}")]
    Io(#[from] std::io::Error),
    #[error("not a valid .fla archive: {0}")]
    Archive(String),
    #[error(
        "this is a legacy .fla from Flash CS4 or earlier, which uses an \
         undocumented binary format. Open it in Animate and re-save it, or use \
         File > Save As and choose Uncompressed Document (.xfl)."
    )]
    LegacyBinaryFla,
    /// No `DOMDocument.xml` in the archive — and **what was there instead**.
    ///
    /// The bare message was a dead end: told that a file does not look like an
    /// Animate document, there is nothing to do but believe it. Naming the
    /// entries that *are* in the archive turns it into something actionable —
    /// a `.fla` saved by a different tool, an archive of something else
    /// entirely, or a nested folder are all obvious at a glance and
    /// indistinguishable without it.
    #[error(
        "could not find DOMDocument.xml, so this does not look like an Animate \
         document. The archive holds: {0}"
    )]
    MissingDocument(String),
    /// It is there and could not be read as text.
    ///
    /// Distinct from [`Self::MissingDocument`] because the two want opposite
    /// things done about them, and reporting this one as "not an Animate
    /// document" — which is what silently ignoring the read failure did — sends
    /// the user looking for a problem with the wrong file.
    #[error("DOMDocument.xml is in the archive but could not be read as text: {0}")]
    UnreadableDocument(String),
    #[error("malformed XML: {0}")]
    Xml(String),
}

impl From<quick_xml::Error> for ImportError {
    fn from(e: quick_xml::Error) -> Self {
        Self::Xml(e.to_string())
    }
}

impl From<zip::result::ZipError> for ImportError {
    fn from(e: zip::result::ZipError) -> Self {
        Self::Archive(e.to_string())
    }
}

/// What came across, and what did not.
///
/// Returned alongside the scene so the editor can tell the user plainly.
#[derive(Debug, Default, Clone)]
pub struct ImportReport {
    /// The first scene's own name, as Animate's Scene panel shows it.
    pub scene_name: Option<String>,
    /// **Every scene after the first**, each its own timeline and camera over
    /// the shared library. Animate keeps them apart; reading them all into
    /// one timeline played Scene 2's layers and camera moves over Scene 1
    /// (barish1: 36 layers and 53 camera keys where Scene 1 has 16 and 3).
    pub more_scenes: Vec<(String, Scene)>,
    pub layers: usize,
    pub keyframes: usize,
    pub shapes: usize,
    pub groups: usize,
    pub instances: usize,
    pub symbols: usize,
    pub tweens: usize,
    /// Keyframes on Animate's camera layer.
    pub camera_keys: usize,
    /// Layers whose rig parent named a *different* layer on some keyframes
    /// than on others (`parentLayerIndex` changing mid-timeline). `follows`
    /// holds one layer, so the keyframes that name a different one are baked
    /// to their absolute pose at that frame instead of propagating from
    /// either parent's rest pose — see `bake_rig_offsets`.
    pub retargeted_rig_layers: usize,
    /// Features present in the file that this importer does not handle.
    pub unsupported: Vec<String>,
}

impl ImportReport {
    fn note_unsupported(&mut self, what: impl Into<String>) {
        let what = what.into();
        // One line per kind, with a count, rather than thousands of repeats.
        if let Some(existing) = self
            .unsupported
            .iter_mut()
            .find(|e| e.starts_with(&what) || e.split(" (").next() == Some(what.as_str()))
        {
            let count = existing
                .rsplit_once(" (x")
                .and_then(|(_, n)| n.trim_end_matches(')').parse::<usize>().ok())
                .unwrap_or(1);
            *existing = format!("{what} (x{})", count + 1);
        } else {
            self.unsupported.push(what);
        }
    }

    pub fn is_complete(&self) -> bool {
        self.unsupported.is_empty()
    }

    /// A short summary for the status bar.
    pub fn summary(&self) -> String {
        let mut line = format!(
            "{} layers, {} keyframes, {} shapes, {} instances, {} symbols",
            self.layers, self.keyframes, self.shapes, self.instances, self.symbols
        );
        // Only when there is a camera: most documents have none, and a
        // permanent "0 camera keys" teaches the reader to skip the line.
        if self.camera_keys > 0 {
            line.push_str(&format!(", {} camera keys", self.camera_keys));
        }
        line
    }
}

/// Import a `.fla` file or an `.xfl` folder.
pub fn import(path: impl AsRef<Path>) -> Result<(Scene, ImportReport), ImportError> {
    let path = path.as_ref();

    if path.is_dir() {
        return import_xfl_folder(path);
    }

    let mut bytes = Vec::new();
    std::fs::File::open(path)?.read_to_end(&mut bytes)?;

    // A legacy .fla is an OLE2 compound document, which starts with this
    // signature. Detecting it lets us say something useful instead of failing
    // with "not a zip".
    const OLE2_MAGIC: [u8; 8] = [0xD0, 0xCF, 0x11, 0xE0, 0xA1, 0xB1, 0x1A, 0xE1];
    if bytes.len() >= 8 && bytes[..8] == OLE2_MAGIC {
        return Err(ImportError::LegacyBinaryFla);
    }

    // The `.fla`'s own folder, so a sound whose `.dat` cache is missing can
    // still be found at `sourceExternalFilepath`, which Animate writes
    // relative to the document.
    import_fla_bytes_from(&bytes, path.parent())
}

/// Import from `.fla` bytes.
pub fn import_fla_bytes(bytes: &[u8]) -> Result<(Scene, ImportReport), ImportError> {
    import_fla_bytes_from(bytes, None)
}

/// [`import_fla_bytes`], with the folder the `.fla` itself lives in, so a
/// `sourceExternalFilepath` fallback has somewhere to resolve against. `None`
/// when the bytes did not come from a file on disk — the fallback then simply
/// finds nothing, same as any other missing media.
fn import_fla_bytes_from(
    bytes: &[u8],
    base_dir: Option<&Path>,
) -> Result<(Scene, ImportReport), ImportError> {
    let mut archive = zip::ZipArchive::new(std::io::Cursor::new(bytes))?;

    // **Found by name, not assumed at the root.** Animate writes
    // `DOMDocument.xml` at the top of the archive, but a `.fla` that has been
    // through a zip tool — repacked, or built from an unzipped folder — often
    // carries the whole document inside a directory, and `by_name` on the bare
    // name then finds nothing at all. The file is the same file; only the
    // prefix differs, so the prefix is what is searched for and then carried
    // through to the library.
    let document_entry = (0..archive.len())
        .filter_map(|i| archive.by_index(i).ok().map(|f| f.name().to_string()))
        .filter(|name| {
            name.rsplit('/')
                .next()
                .is_some_and(|leaf| leaf.eq_ignore_ascii_case("DOMDocument.xml"))
        })
        // The shallowest one, so a document that happens to hold another
        // document deeper down still opens as itself.
        .min_by_key(|name| name.matches('/').count());

    let Some(document_entry) = document_entry else {
        return Err(ImportError::MissingDocument(archive_listing(&mut archive)));
    };

    let document = read_entry(&mut archive, &document_entry)?
        .ok_or_else(|| ImportError::MissingDocument(archive_listing(&mut archive)))?;

    // Symbol definitions live in LIBRARY/, one file each — beside the document,
    // wherever that turned out to be.
    let prefix = match document_entry.rfind('/') {
        Some(at) => &document_entry[..=at],
        None => "",
    };
    let wanted = format!("{}library/", prefix.to_ascii_lowercase());
    let library_names: Vec<String> = (0..archive.len())
        .filter_map(|i| archive.by_index(i).ok().map(|f| f.name().to_string()))
        .filter(|name| {
            let lower = name.to_ascii_lowercase();
            lower.starts_with(&wanted) && lower.ends_with(".xml")
        })
        .collect();

    let mut library_files = Vec::new();
    for name in library_names {
        // **A symbol that will not read is reported, not fatal.** Losing one
        // damaged symbol costs that symbol; refusing the document costs the
        // film. The same judgement §4 records for malformed XML inside one.
        match read_entry(&mut archive, &name) {
            Ok(Some(text)) => library_files.push((name, text)),
            Ok(None) => {}
            Err(e) => tracing::warn!("skipping library entry {name}: {e}"),
        }
    }

    build_with_media(&document, &library_files, base_dir, |root, name| {
        // `bin/` sits beside `LIBRARY/`, holding the sound and bitmap caches
        // Animate keeps under its own generated names — see [`MediaRoot`].
        let sub = match root {
            MediaRoot::Library => "LIBRARY",
            MediaRoot::Bin => "bin",
        };
        let entry = format!("{prefix}{sub}/{name}");
        let mut file = match archive.by_name(&entry) {
            Ok(file) => file,
            Err(zip::result::ZipError::FileNotFound) => return Ok(None),
            Err(error) => return Err(error.into()),
        };
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes)?;
        Ok(Some(bytes))
    })
}

/// Import from an uncompressed `.xfl` folder.
pub fn import_xfl_folder(folder: &Path) -> Result<(Scene, ImportReport), ImportError> {
    let document_path = folder.join("DOMDocument.xml");
    if !document_path.exists() {
        // The same courtesy the archive path gets: say what is there instead.
        let listing = std::fs::read_dir(folder)
            .map(|entries| {
                let mut names: Vec<String> = entries
                    .flatten()
                    .map(|e| e.file_name().to_string_lossy().to_string())
                    .collect();
                names.sort();
                names.truncate(8);
                if names.is_empty() {
                    "nothing at all".to_string()
                } else {
                    names.join(", ")
                }
            })
            .unwrap_or_else(|e| format!("(unreadable: {e})"));
        return Err(ImportError::MissingDocument(listing));
    }
    // Decoded rather than required to be UTF-8, so a byte order mark does not
    // reach the parser as content.
    let document = decode_xml(&std::fs::read(&document_path)?)?;

    let mut library_files = Vec::new();
    let library_dir = folder.join("LIBRARY");
    if library_dir.is_dir() {
        collect_library(&library_dir, &library_dir, &mut library_files)?;
    }

    build_with_media(&document, &library_files, Some(folder), |media_root, name| {
        let relative = std::path::PathBuf::from(name.replace('\\', "/"));
        if relative.components().any(|part| !matches!(part,
            std::path::Component::Normal(_) | std::path::Component::CurDir)) {
            return Err(ImportError::Archive(format!("invalid media path: {name}")));
        }
        // `bin/` sits beside `LIBRARY/`, holding the sound and bitmap caches
        // Animate keeps under its own generated names — see [`MediaRoot`].
        let unresolved = match media_root {
            MediaRoot::Library => library_dir.clone(),
            MediaRoot::Bin => folder.join("bin"),
        };
        if !unresolved.is_dir() {
            return Ok(None);
        }
        let root = unresolved.canonicalize()?;
        let path = root.join(relative);
        if !path.exists() { return Ok(None); }
        let path = path.canonicalize()?;
        if !path.starts_with(&root) {
            return Err(ImportError::Archive(format!("media escapes library: {name}")));
        }
        Ok(Some(std::fs::read(path)?))
    })
}

fn collect_library(
    root: &Path,
    dir: &Path,
    out: &mut Vec<(String, String)>,
) -> Result<(), ImportError> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            collect_library(root, &path, out)?;
        } else if path
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("xml"))
        {
            // Keep the path relative to LIBRARY so folder structure survives.
            let relative = path
                .strip_prefix(root)
                .unwrap_or(&path)
                .to_string_lossy()
                .replace('\\', "/");
            out.push((relative, std::fs::read_to_string(&path)?));
        }
    }
    Ok(())
}

/// Read one archive entry as text.
///
/// `Ok(None)` means it is not in the archive; `Err` means it is and could not
/// be read. Collapsing those two into one answer is what made a damaged
/// document report itself as not being an Animate document at all.
///
/// **The bytes are decoded rather than required to be UTF-8.** A UTF-8 byte
/// order mark is stripped, because `quick_xml` treats it as content and the
/// document then begins with a character no parser expects; and a UTF-16 file —
/// which Animate does not write, but which a conversion tool might — is decoded
/// from its mark instead of being refused as invalid UTF-8.
fn read_entry<R: std::io::Read + std::io::Seek>(
    archive: &mut zip::ZipArchive<R>,
    name: &str,
) -> Result<Option<String>, ImportError> {
    let mut file = match archive.by_name(name) {
        Ok(file) => file,
        Err(zip::result::ZipError::FileNotFound) => return Ok(None),
        Err(e) => return Err(ImportError::Archive(e.to_string())),
    };
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)
        .map_err(|e| ImportError::UnreadableDocument(e.to_string()))?;
    decode_xml(&bytes).map(Some)
}

/// Turn XML bytes into text, honouring a byte order mark.
fn decode_xml(bytes: &[u8]) -> Result<String, ImportError> {
    // UTF-16, which is rare and is still a real file when it appears.
    if bytes.len() >= 2 {
        let wide = match (bytes[0], bytes[1]) {
            (0xFF, 0xFE) => Some(false),
            (0xFE, 0xFF) => Some(true),
            _ => None,
        };
        if let Some(big_endian) = wide {
            let units: Vec<u16> = bytes[2..]
                .chunks_exact(2)
                .map(|c| {
                    if big_endian {
                        u16::from_be_bytes([c[0], c[1]])
                    } else {
                        u16::from_le_bytes([c[0], c[1]])
                    }
                })
                .collect();
            return String::from_utf16(&units)
                .map_err(|e| ImportError::UnreadableDocument(e.to_string()));
        }
    }

    let without_bom = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(bytes);
    String::from_utf8(without_bom.to_vec())
        .map_err(|e| ImportError::UnreadableDocument(e.to_string()))
}

/// A few entry names from the archive, for a message that says what was found.
fn archive_listing<R: std::io::Read + std::io::Seek>(archive: &mut zip::ZipArchive<R>) -> String {
    let mut names: Vec<String> = (0..archive.len().min(400))
        .filter_map(|i| archive.by_index(i).ok().map(|f| f.name().to_string()))
        // Directory entries say nothing useful about what the file is.
        .filter(|n| !n.ends_with('/'))
        .collect();
    if names.is_empty() {
        return "nothing at all".to_string();
    }
    names.sort();
    let total = names.len();
    names.truncate(8);
    let listed = names.join(", ");
    if total > 8 {
        format!("{listed}, and {} more", total - 8)
    } else {
        listed
    }
}

/// Assemble a scene from the document and its library files.
#[cfg(test)]
fn build(
    document: &str,
    library_files: &[(String, String)],
) -> Result<(Scene, ImportReport), ImportError> {
    build_with_media(document, library_files, None, |_, _| Ok(None))
}

type BitmapMap = HashMap<String, buzz_scene::ImageFill>;

/// A `DOMSoundItem`'s name (what a frame's `soundName` refers to) to the
/// [`SoundId`] it was imported as.
type SoundMap = HashMap<String, SoundId>;

/// Which of the two folders a media file lives under.
///
/// Animate keeps both beside `DOMDocument.xml`: `LIBRARY/` mirrors the
/// original bitmaps and any audio saved back out under its own name, while
/// `bin/` holds Animate's own generated cache of everything, named after an
/// opaque `...HRef` rather than the asset. A bitmap's `bin/` copy is
/// re-encoded and not generally decodable as the original format, so bitmaps
/// are read from `LIBRARY/`; a sound's `bin/` copy is the original MP3
/// bitstream verbatim (Findings §5), so sounds are read from `bin/`.
enum MediaRoot {
    Library,
    Bin,
}

fn build_with_media(
    document: &str,
    library_files: &[(String, String)],
    base_dir: Option<&Path>,
    mut read_media: impl FnMut(MediaRoot, &str) -> Result<Option<Vec<u8>>, ImportError>,
) -> Result<(Scene, ImportReport), ImportError> {
    let mut scene = Scene::empty();
    let mut report = ImportReport::default();
    let mut ids = IdSource::default();
    // Decode each bitmap once; stage and symbol placements share its pixels.
    let mut bitmaps = BitmapMap::new();
    let mut media_reader = quick_xml::Reader::from_str(document);
    loop {
        match media_reader.read_event()? {
            Event::Eof => break,
            Event::Start(e) | Event::Empty(e) if e.name().as_ref() == b"DOMBitmapItem" => {
                let attrs = attributes(&e);
                let Some(name) = attrs.get("name") else { continue };
                let href = attrs.get("href").unwrap_or(name);
                let bytes = match read_media(MediaRoot::Library, href) {
                    Ok(Some(bytes)) => bytes,
                    // **No copy under `LIBRARY/`: Animate's own cache in
                    // `bin/`.** Most bitmaps in a real file exist only there
                    // (barish1: 18 of 21), and drawing them as nothing left
                    // grey placeholder slabs across every tree and bush.
                    Ok(None) => match attrs
                        .get("bitmapDataHRef")
                        .map(|dat| read_media(MediaRoot::Bin, dat))
                    {
                        Some(Ok(Some(dat))) => match decode_bitmap_dat(&dat) {
                            Some(DatBitmap::Encoded(bytes)) => bytes,
                            Some(DatBitmap::Pixels { width, height, rgba }) => {
                                let asset = buzz_scene::ImageAsset::from_pixels(
                                    buzz_scene::ImageId(ids.take()),
                                    name.clone(),
                                    width,
                                    height,
                                    std::sync::Arc::new(rgba),
                                );
                                // The size Animate places it at, in twips, when
                                // the cache holds it downsampled.
                                let w = attrs
                                    .get("frameRight")
                                    .and_then(|v| v.parse::<f64>().ok())
                                    .map_or(width as f64, |t| t / 20.0);
                                let h = attrs
                                    .get("frameBottom")
                                    .and_then(|v| v.parse::<f64>().ok())
                                    .map_or(height as f64, |t| t / 20.0);
                                let asset = scene.images_mut().insert(asset);
                                let mut fill = buzz_scene::ImageFill::new(
                                    asset,
                                    buzz_geom::Rect::new(0.0, 0.0, w, h),
                                );
                                fill.smooth = attr_bool(&attrs, "allowSmoothing", false);
                                bitmaps.insert(name.clone(), fill);
                                continue;
                            }
                            None => {
                                report.note_unsupported(format!("bitmap media unreadable: {name}"));
                                continue;
                            }
                        },
                        _ => {
                            report.note_unsupported(format!("bitmap media unavailable: {name}"));
                            continue;
                        }
                    },
                    Err(error) => {
                        report.note_unsupported(format!("bitmap media {name}: {error}"));
                        continue;
                    }
                };
                match buzz_scene::ImageAsset::decode(buzz_scene::ImageId(ids.take()), name, &bytes) {
                    Ok(asset) => {
                        let rect = buzz_geom::Rect::new(0.0, 0.0, asset.width as f64, asset.height as f64);
                        let asset = scene.images_mut().insert(asset);
                        let mut fill = buzz_scene::ImageFill::new(asset, rect);
                        fill.smooth = attr_bool(&attrs, "allowSmoothing", false);
                        bitmaps.insert(name.clone(), fill);
                    }
                    Err(error) => report.note_unsupported(format!("bitmap media {name}: {error}")),
                }
            }
            _ => {}
        }
    }

    // **Sounds, the same way, once.** A `DOMSoundItem` only ever appears in
    // the document's own library manifest (never inside a symbol's LIBRARY
    // file), so one pass here finds every one of them; frames that name a
    // sound, wherever they are, look it up in `sounds` below.
    let mut sounds = SoundMap::new();
    let mut sound_reader = quick_xml::Reader::from_str(document);
    loop {
        match sound_reader.read_event()? {
            Event::Eof => break,
            Event::Start(e) | Event::Empty(e) if e.name().as_ref() == b"DOMSoundItem" => {
                let attrs = attributes(&e);
                let Some(name) = attrs.get("name").cloned() else { continue };
                match import_sound_bytes(&attrs, base_dir, &mut read_media) {
                    Ok(Some((bytes, format))) => {
                        match buzz_audio::Clip::decode(&bytes, &name) {
                            Ok(clip) => {
                                let id = scene.add_sound(
                                    &name,
                                    std::sync::Arc::new(bytes),
                                    &format,
                                    clip.sample_rate,
                                    clip.channels,
                                    clip.len() as u64,
                                );
                                sounds.insert(name, id);
                            }
                            Err(error) => {
                                report.note_unsupported(format!("sound {name}: could not decode ({error})"))
                            }
                        }
                    }
                    Ok(None) => report.note_unsupported(format!("sound media unavailable: {name}")),
                    Err(error) => report.note_unsupported(format!("sound {name}: {error}")),
                }
            }
            _ => {}
        }
    }

    // **Two passes over the library, and the reason matters.**
    //
    // A symbol's timeline contains instances of *other* symbols — a character
    // is a torso holding an arm holding a hand — and in a real document most
    // of them are defined in a file that has not been read yet. Parsing each
    // symbol against the symbols already parsed therefore resolves almost
    // nothing: an Animate document of any size imported with every nested
    // instance dropped and a page of "instance of unknown symbol" against it.
    //
    // So names and ids are collected first, from every file, and only then are
    // the timelines read — by which point every symbol can see every other,
    // whatever order the archive happens to store them in.
    let mut by_name: HashMap<String, SymbolId> = HashMap::new();
    let mut pending: Vec<(&String, &String, SymbolId)> = Vec::new();
    for (path, xml) in library_files {
        let id = SymbolId(ids.take());
        for key in library_keys(xml, path) {
            // First writer wins: a name that appears twice keeps the symbol
            // whose own file is named after it, rather than a later
            // like-named one from another folder.
            by_name.entry(key).or_insert(id);
        }
        pending.push((path, xml, id));
    }

    for (path, xml, id) in pending {
        match parse_symbol(xml, path, id, &by_name, &bitmaps, &sounds, &mut ids, &mut report) {
            Ok(symbol) => {
                scene.library_mut().insert(symbol);
                report.symbols += 1;
            }
            Err(e) => report.note_unsupported(format!("symbol {path}: {e}")),
        }
    }

    // Every scene shares the library just read; each gets its own timeline.
    let names = timeline_names(document);
    let shared = scene.clone();
    parse_document(document, &mut scene, &by_name, &bitmaps, &sounds, &mut ids, &mut report, 0)?;
    report.scene_name = names.first().cloned();
    for (index, name) in names.iter().enumerate().skip(1) {
        let mut more = shared.clone();
        parse_document(document, &mut more, &by_name, &bitmaps, &sounds, &mut ids, &mut report, index)?;
        report.more_scenes.push((name.clone(), more));
    }

    // The importer allocates its own ids, so the document's allocator has to
    // be raised past them or a new object would collide with an imported one.
    scene.reserve_ids_above(ids.next);
    for (_, more) in &mut report.more_scenes {
        more.reserve_ids_above(ids.next);
    }
    Ok((scene, report))
}

/// The name of every top-level `<DOMTimeline>` — Animate's scenes — in order.
fn timeline_names(xml: &str) -> Vec<String> {
    let mut reader = quick_xml::Reader::from_str(xml);
    let mut depth = 0usize;
    let mut names = Vec::new();
    loop {
        match reader.read_event() {
            Ok(Event::Start(e)) if e.name().as_ref() == b"DOMTimeline" => {
                if depth == 0 {
                    let attrs = attributes(&e);
                    names.push(attrs.get("name").cloned().unwrap_or_else(|| format!("Scene {}", names.len() + 1)));
                }
                depth += 1;
            }
            Ok(Event::Empty(e)) if e.name().as_ref() == b"DOMTimeline" && depth == 0 => {
                let attrs = attributes(&e);
                names.push(attrs.get("name").cloned().unwrap_or_else(|| format!("Scene {}", names.len() + 1)));
            }
            Ok(Event::End(e)) if e.name().as_ref() == b"DOMTimeline" => depth = depth.saturating_sub(1),
            Ok(Event::Eof) | Err(_) => break,
            _ => {}
        }
    }
    if names.is_empty() {
        names.push("Scene 1".to_string());
    }
    names
}

/// Bytes for a `DOMSoundItem`, and the format tag to save them back out under.
///
/// **The `.dat` behind `soundDataHRef` is tried first, and it is usually
/// enough on its own.** Unlike a bitmap's `bin/` cache, which Animate
/// re-encodes into a format nothing else reads, a sound's cache is the
/// **original MPEG bitstream**, byte for byte — confirmed by the frame sync
/// this checks for (Findings §5). `sourceExternalFilepath` is only the
/// fallback, for the archive whose cache went missing or was never an MP3 to
/// begin with.
fn import_sound_bytes(
    attrs: &HashMap<String, String>,
    base_dir: Option<&Path>,
    read_media: &mut impl FnMut(MediaRoot, &str) -> Result<Option<Vec<u8>>, ImportError>,
) -> Result<Option<(Vec<u8>, String)>, ImportError> {
    if let Some(href) = attrs.get("soundDataHRef") {
        if let Some(bytes) = read_media(MediaRoot::Bin, href)? {
            if is_mpeg_frame_sync(&bytes) {
                return Ok(Some((bytes, "mp3".to_string())));
            }
        }
    }
    if let (Some(base), Some(path)) = (base_dir, attrs.get("sourceExternalFilepath")) {
        let resolved = base.join(path.replace('\\', "/"));
        if let Ok(bytes) = std::fs::read(&resolved) {
            let format = resolved
                .extension()
                .map(|e| e.to_string_lossy().to_ascii_lowercase())
                .unwrap_or_else(|| "mp3".to_string());
            return Ok(Some((bytes, format)));
        }
    }
    Ok(None)
}

/// Whether bytes begin with an MPEG audio frame sync: `0xFF` followed by a
/// byte with its top three bits set. Every MP3 frame starts with this
/// eleven-bit sync word, and Animate's sound cache starts the file with one
/// rather than wrapping it in a container.
fn is_mpeg_frame_sync(bytes: &[u8]) -> bool {
    bytes.len() >= 2 && bytes[0] == 0xFF && (bytes[1] & 0xE0) == 0xE0
}

/// Hands out ids for imported objects.
#[derive(Debug, Default)]
struct IdSource {
    next: u64,
}

impl IdSource {
    fn take(&mut self) -> u64 {
        self.next += 1;
        self.next
    }
}

/// Attributes of an XML start tag, as a map.
fn attributes(e: &quick_xml::events::BytesStart<'_>) -> HashMap<String, String> {
    e.attributes()
        .filter_map(|a| a.ok())
        .filter_map(|a| {
            let key = String::from_utf8_lossy(a.key.as_ref()).to_string();
            // Animate writes `<?xml version="1.0" encoding="UTF-8"?>`, so 1.0
            // normalisation is the right rule for whitespace in attributes.
            let value = a
                .normalized_value(quick_xml::XmlVersion::Explicit1_0)
                .ok()?
                .to_string();
            Some((key, value))
        })
        .collect()
}

fn attr_f64(attrs: &HashMap<String, String>, key: &str, default: f64) -> f64 {
    attrs
        .get(key)
        .and_then(|v| v.parse::<f64>().ok())
        .unwrap_or(default)
}

fn attr_u32(attrs: &HashMap<String, String>, key: &str, default: u32) -> u32 {
    attrs
        .get(key)
        .and_then(|v| v.parse::<u32>().ok())
        .unwrap_or(default)
}

/// A style's `index` attribute. XFL numbers styles from one.
fn attr_index(attrs: &HashMap<String, String>) -> u32 {
    attrs
        .get("index")
        .and_then(|v| v.trim().parse::<u32>().ok())
        .unwrap_or(1)
}

fn attr_bool(attrs: &HashMap<String, String>, key: &str, default: bool) -> bool {
    match attrs.get(key).map(String::as_str) {
        Some("true") | Some("1") => true,
        Some("false") | Some("0") => false,
        _ => default,
    }
}

/// Animate writes colours as `#RRGGBB` with a separate alpha attribute.
fn parse_color(attrs: &HashMap<String, String>, color_key: &str, alpha_key: &str) -> Color {
    let hex = attrs
        .get(color_key)
        .map(String::as_str)
        .unwrap_or("#000000");
    let hex = hex.trim_start_matches('#');
    let value = u32::from_str_radix(hex, 16).unwrap_or(0);
    let alpha = attrs
        .get(alpha_key)
        .and_then(|v| v.parse::<f64>().ok())
        .unwrap_or(1.0)
        .clamp(0.0, 1.0);

    Color::from_rgba8(
        ((value >> 16) & 0xFF) as u8,
        ((value >> 8) & 0xFF) as u8,
        (value & 0xFF) as u8,
        (alpha * 255.0).round() as u8,
    )
}

/// Animate's `<Matrix>` element.
fn parse_matrix(attrs: &HashMap<String, String>) -> Affine {
    Affine::new([
        attr_f64(attrs, "a", 1.0),
        attr_f64(attrs, "b", 0.0),
        attr_f64(attrs, "c", 0.0),
        attr_f64(attrs, "d", 1.0),
        attr_f64(attrs, "tx", 0.0),
        attr_f64(attrs, "ty", 0.0),
    ])
}

/// Map Animate's layer type attribute onto our layer kinds.
fn parse_layer_kind(attrs: &HashMap<String, String>) -> LayerKind {
    match attrs.get("layerType").map(String::as_str) {
        Some("folder") => LayerKind::Folder,
        Some("mask") => LayerKind::Mask,
        Some("guide") => LayerKind::Guide,
        // `masked` and `guided` matter: the positional rule that resolves
        // masking reads these kinds. Importing a masked layer as Normal breaks
        // the run beneath its mask, and the mask silently clips nothing.
        Some("masked") => LayerKind::Masked,
        Some("guided") => LayerKind::Guided,
        _ => LayerKind::Normal,
    }
}

fn parse_tween(attrs: &HashMap<String, String>) -> Tween {
    let kind = match attrs.get("tweenType").map(String::as_str) {
        Some("motion") => buzz_scene::TweenKind::Classic,
        Some("shape") => buzz_scene::TweenKind::Shape,
        _ => buzz_scene::TweenKind::None,
    };
    let ease = attr_f64(attrs, "acceleration", 0.0);
    Tween {
        kind,
        easing: if ease.abs() < f64::EPSILON {
            buzz_scene::Easing::Linear
        } else {
            buzz_scene::Easing::Strength(ease)
        },
        extra_rotations: attrs
            .get("motionTweenRotateTimes")
            .and_then(|v| v.parse::<i32>().ok())
            .unwrap_or(0),
        orient_to_path: attr_bool(attrs, "motionTweenOrientToPath", false),
    }
}

/// A `<DOMFrame>`'s `soundName` and its sync/loop attributes, resolved
/// against the sounds already imported. `None` when the frame names no
/// sound, or names one whose media did not come across — a dangling
/// `soundName` should not point at nothing.
fn parse_sound_ref(attrs: &HashMap<String, String>, sounds: &SoundMap) -> Option<SoundRef> {
    let name = attrs.get("soundName")?;
    let sound = *sounds.get(name)?;
    let sync = match attrs.get("soundSync").map(String::as_str) {
        Some("start") => SoundSync::Start,
        Some("stop") => SoundSync::Stop,
        Some("stream") => SoundSync::Stream,
        // "event" is Animate's default and what it omits when writing.
        _ => SoundSync::Event,
    };
    let loops = match attrs.get("soundLoopMode").map(String::as_str) {
        // The loop checkbox: Animate does not say how many times, only that
        // it does not stop — which is what zero means to `SoundRef::loops`.
        Some("loop") => 0,
        Some("repeat") => attr_u32(attrs, "soundLoop", 1).max(1),
        _ => 1,
    };
    Some(SoundRef {
        sound,
        sync,
        volume: 1.0,
        loops,
        trim_start: 0.0,
        trim_end: None,
    })
}

/// A cubic-Bézier equivalent for one of Animate's named `<Ease method="…">`
/// presets, as the four points `buzz_scene::eval_bezier_ease` expects:
/// `(0,0)`, two control points, `(1,1)`. Our own [`buzz_scene::Easing`] has no
/// vocabulary of named curves to map onto — only `Linear`, the acceleration
/// slider and an arbitrary Bézier — so every named preset becomes the
/// equivalent single-segment Bézier instead, the same well-known control
/// points CSS's `cubic-bezier()` uses for the same names. `None` for
/// `"none"`/anything unrecognised: nothing to override with, so the property
/// keeps easing by the tween's own slider.
fn bezier_points_for_ease_method(method: &str) -> Option<[buzz_geom::Point; 4]> {
    let (x1, y1, x2, y2) = match method {
        "quadIn" => (0.11, 0.0, 0.5, 0.0),
        "quadOut" => (0.5, 1.0, 0.89, 1.0),
        "quadInOut" => (0.455, 0.03, 0.515, 0.955),
        "cubicIn" => (0.55, 0.055, 0.675, 0.19),
        "cubicOut" => (0.215, 0.61, 0.355, 1.0),
        "cubicInOut" => (0.645, 0.045, 0.355, 1.0),
        "quartIn" => (0.895, 0.03, 0.685, 0.22),
        "quartOut" => (0.165, 0.84, 0.44, 1.0),
        "quartInOut" => (0.77, 0.0, 0.175, 1.0),
        "quintIn" => (0.755, 0.05, 0.855, 0.06),
        "quintOut" => (0.23, 1.0, 0.32, 1.0),
        "quintInOut" => (0.86, 0.0, 0.07, 1.0),
        "sineIn" => (0.47, 0.0, 0.745, 0.715),
        "sineOut" => (0.39, 0.575, 0.565, 1.0),
        "sineInOut" => (0.445, 0.05, 0.55, 0.95),
        "expoIn" => (0.95, 0.05, 0.795, 0.035),
        "expoOut" => (0.19, 1.0, 0.22, 1.0),
        "expoInOut" => (1.0, 0.0, 0.0, 1.0),
        "circIn" => (0.6, 0.04, 0.98, 0.335),
        "circOut" => (0.075, 0.82, 0.165, 1.0),
        "circInOut" => (0.785, 0.135, 0.15, 0.86),
        _ => return None,
    };
    Some([
        buzz_geom::Point::new(0.0, 0.0),
        buzz_geom::Point::new(x1, y1),
        buzz_geom::Point::new(x2, y2),
        buzz_geom::Point::new(1.0, 1.0),
    ])
}

/// A Motion Tween span longer than this bakes one keyframe per **property
/// key** (with a linear classic tween between) rather than one per frame —
/// the difference between a handful of keyframes and thousands for a span
/// that turns out to hold a single unanimated pose, which real files do: an
/// object converted to a Motion Tween and never actually moved still writes
/// its full span length.
const MOTION_BAKE_DENSE_LIMIT: u32 = 2000;

/// One property's motion curve — Animate's `AnimationCore`, evaluated by
/// piecewise-linear interpolation between its keyframes' `(timevalue, anchor
/// value)` pairs.
///
/// Animate also writes `previous`/`next` Bézier handles on every keyframe,
/// but on both evidence files this plan was measured against — the two
/// rotating heads with a "hold, ease into a turn, hold, ease back" curve
/// included — every single one exactly repeats its own keyframe's anchor,
/// keyframe after keyframe, leaving no curvature to reverse-engineer a handle
/// convention from. Piecewise-linear reproduces every curve measured exactly;
/// a real hand-drawn handle would be an approximation, the same trade a
/// straight `Easing::Linear` already is for the rare classic tween whose
/// acceleration this importer cannot read.
#[derive(Debug, Default, Clone)]
struct MotionCurve {
    /// `(timevalue in ticks, value)`, sorted by tick.
    keys: Vec<(f64, f64)>,
}

impl MotionCurve {
    fn value_at(&self, tick: f64) -> f64 {
        let Some(&(first_t, first_v)) = self.keys.first() else {
            return 0.0;
        };
        if tick <= first_t {
            return first_v;
        }
        let &(last_t, last_v) = self.keys.last().unwrap();
        if tick >= last_t {
            return last_v;
        }
        for pair in self.keys.windows(2) {
            let (t0, v0) = pair[0];
            let (t1, v1) = pair[1];
            if tick >= t0 && tick <= t1 {
                if (t1 - t0).abs() < 1e-9 {
                    return v1;
                }
                return v0 + (v1 - v0) * (tick - t0) / (t1 - t0);
            }
        }
        last_v
    }
}

/// A Motion Tween's `AnimationCore`: every property it animates, plus how
/// many ticks its span covers.
#[derive(Debug, Default)]
struct MotionObject {
    ticks_total: f64,
    properties: HashMap<String, MotionCurve>,
}

impl MotionObject {
    fn get(&self, id: &str) -> Option<&MotionCurve> {
        self.properties.get(id)
    }

    /// `id`'s value at `tick`, or `default` when the property is absent —
    /// which is Animate's own rest value for it: `100` for the two scales,
    /// `0` for everything else this reads.
    fn value_at(&self, id: &str, tick: f64, default: f64) -> f64 {
        match self.properties.get(id) {
            Some(curve) if !curve.keys.is_empty() => curve.value_at(tick),
            _ => default,
        }
    }
}

/// Parse the inner text of a `<motionObjectXML>` element: Animate's
/// `AnimationCore`, a small nested document nothing else in this file reads,
/// parsed with a reader of its own rather than folded into the flat
/// one-level state machine the rest of the importer uses — `PropertyContainer`
/// nests several layers deep, and that machine tracks only one.
///
/// The properties that matter for v1 — `Motion_X`, `Motion_Y`, `Rotation_Z`,
/// `Skew_X`, `Skew_Y`, `Scale_X`, `Scale_Y` — are read by `id`, wherever in
/// the container tree they sit; `Depth` and the colour/filter containers are
/// not modelled and are named in the report when they hold anything.
fn parse_motion_object(xml: &str, report: &mut ImportReport) -> MotionObject {
    let mut reader = quick_xml::Reader::from_str(xml);
    let mut object = MotionObject::default();
    let mut current_property: Option<String> = None;
    loop {
        let (name, attrs, is_empty) = match reader.read_event() {
            Ok(Event::Eof) => break,
            Ok(Event::Start(e)) => (
                String::from_utf8_lossy(e.name().as_ref()).to_string(),
                attributes(&e),
                false,
            ),
            Ok(Event::Empty(e)) => (
                String::from_utf8_lossy(e.name().as_ref()).to_string(),
                attributes(&e),
                true,
            ),
            Err(_) => break,
            _ => continue,
        };
        match name.as_str() {
            "AnimationCore" => object.ticks_total = attr_f64(&attrs, "duration", 0.0),
            "Property" => current_property = attrs.get("id").cloned(),
            "Keyframe" => {
                if let Some(id) = current_property.clone()
                    && let Some(anchor) = attrs.get("anchor")
                    && let Some((_, value)) = anchor.split_once(',')
                    && let Ok(value) = value.trim().parse::<f64>()
                {
                    let tick = attr_f64(&attrs, "timevalue", 0.0);
                    object
                        .properties
                        .entry(id)
                        .or_default()
                        .keys
                        .push((tick, value));
                }
            }
            // A container with something in it to convert: named once, not
            // per instance — `note_unsupported` already collapses repeats.
            "PropertyContainer" if !is_empty => match attrs.get("id").map(String::as_str) {
                Some("Colors") => report.note_unsupported("motion tween colour effects"),
                Some("Filters") => report.note_unsupported("motion tween filters"),
                _ => {}
            },
            _ => {}
        }
    }
    for curve in object.properties.values_mut() {
        curve
            .keys
            .sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
    }
    object
}

/// Convert a Motion Tween into ordinary keyframes.
///
/// # The convention, measured against sustipur1's `fatherFront copy 3`
///
/// Every property at its rest value must reproduce the `DOMSymbolInstance`'s
/// own `<Matrix>` exactly — the check this plan asks for — and it does, with
/// no separate "base matrix" needed at all beyond the instance's own
/// placement: `Scale_X`/`Scale_Y` are **signed** percentages (a mirror is a
/// negative one, not a separate flag), `Rotation_Z` adds into *both* skews
/// the same way `extra_rotations` does in `buzz-scene`'s own tween model
/// (Phase A), and `Skew_X`/`Skew_Y` add on top of that, one skew each:
///
/// ```text
/// skewX = radians(Rotation_Z + Skew_X)      skewY = radians(Rotation_Z + Skew_Y)
/// a = (Scale_X/100)·cos(skewY)   b = (Scale_X/100)·sin(skewY)
/// c = -(Scale_Y/100)·sin(skewX)  d =  (Scale_Y/100)·cos(skewX)
/// tx = instance.tx + Motion_X    ty = instance.ty + Motion_Y
/// ```
///
/// `Motion_X`/`Motion_Y` are 0 at every span's own rest keyframe in both
/// evidence files, so `instance.tx, instance.ty` is exactly the anchor
/// `Motion_X`/`Motion_Y` measures its own move from.
fn bake_motion_object(
    frame: &PendingFrame,
    motion: &MotionObject,
    report: &mut ImportReport,
) -> Vec<buzz_scene::Keyframe> {
    let Some(base) = frame.objects.first() else {
        return Vec::new();
    };
    let base_coeffs = base.transform.as_coeffs();
    let frame_count = frame.duration.max(1);
    let ticks_per_frame = if frame_count > 0 && motion.ticks_total > 0.0 {
        motion.ticks_total / frame_count as f64
    } else {
        1.0
    };

    if let Some(depth) = motion.get("Depth")
        && depth.keys.iter().any(|&(_, v)| v.abs() > 1e-6)
    {
        report.note_unsupported("motion tween Z-depth (3D)");
    }

    let compose = |tick: f64| -> Affine {
        let motion_x = motion.value_at("Motion_X", tick, 0.0);
        let motion_y = motion.value_at("Motion_Y", tick, 0.0);
        let rotation = motion.value_at("Rotation_Z", tick, 0.0);
        let skew_x = motion.value_at("Skew_X", tick, 0.0);
        let skew_y = motion.value_at("Skew_Y", tick, 0.0);
        let scale_x = motion.value_at("Scale_X", tick, 100.0) / 100.0;
        let scale_y = motion.value_at("Scale_Y", tick, 100.0) / 100.0;
        let angle_x = (rotation + skew_x).to_radians();
        let angle_y = (rotation + skew_y).to_radians();
        Affine::new([
            scale_x * angle_y.cos(),
            scale_x * angle_y.sin(),
            -scale_y * angle_x.sin(),
            scale_y * angle_x.cos(),
            base_coeffs[4] + motion_x,
            base_coeffs[5] + motion_y,
        ])
    };

    let one_keyframe = |start: u32, tick: f64, label: Option<String>, sound: Option<SoundRef>, tween: Tween| {
        let mut object = (**base).clone();
        object.transform = compose(tick);
        buzz_scene::Keyframe {
            start,
            objects: std::sync::Arc::new(vec![std::sync::Arc::new(object)]),
            label,
            tween,
            ease_curve: None,
            sound,
        }
    };

    // A span with no real property animation bakes to a single held
    // keyframe, whatever its length — the common case, an object converted
    // to a Motion Tween and never actually touched. A long span with real
    // motion bakes one keyframe per property key, classic-tweened linearly
    // between, instead of one per frame — see `MOTION_BAKE_DENSE_LIMIT`.
    let mut key_frames: Vec<(u32, f64)> = motion
        .properties
        .values()
        .flat_map(|c| c.keys.iter().map(|&(t, _)| t))
        .map(|t| {
            let index = (t / ticks_per_frame).round() as u32;
            (index.min(frame_count.saturating_sub(1)), t)
        })
        .collect();
    key_frames.sort_by(|a, b| a.0.cmp(&b.0));
    key_frames.dedup_by_key(|pair| pair.0);

    if frame_count > MOTION_BAKE_DENSE_LIMIT {
        // No property changes at all: one held keyframe for the whole span,
        // whatever its length — an object converted to a Motion Tween and
        // never actually touched, which real files do at spans thousands of
        // frames long.
        if key_frames.len() <= 1 {
            let tick = key_frames.first().map(|&(_, t)| t).unwrap_or(0.0);
            return vec![one_keyframe(frame.start, tick, frame.label.clone(), frame.sound, Tween::default())];
        }
        let last = key_frames.len() - 1;
        return key_frames
            .into_iter()
            .enumerate()
            .map(|(i, (index, tick))| {
                one_keyframe(
                    frame.start + index,
                    tick,
                    if i == 0 { frame.label.clone() } else { None },
                    if i == 0 { frame.sound } else { None },
                    if i == last { Tween::default() } else { Tween::classic() },
                )
            })
            .collect();
    }

    (0..frame_count)
        .map(|i| {
            one_keyframe(
                frame.start + i,
                i as f64 * ticks_per_frame,
                if i == 0 { frame.label.clone() } else { None },
                if i == 0 { frame.sound } else { None },
                Tween::default(),
            )
        })
        .collect()
}

/// Parse `DOMDocument.xml` into the scene's main timeline.
/// Animate's camera layer, read as it goes past.
///
/// The camera is a layer of type `camera` holding one instance of a symbol
/// called `__Camera__` per keyframe, and that symbol is not in the library —
/// it is Animate's own. Read naively it produced a page of "instance of
/// unknown symbol __Camera__" and the document imported without its moves,
/// which for a documentary shot on a camera move is most of the animation.
///
/// The matrix on the instance places the *camera*, so the view is its inverse:
/// a camera scaled to 0.5 shows half as much stage, which is a zoom of 2.
#[derive(Default)]
struct CameraCapture {
    /// Inside a camera layer.
    active: bool,
    /// Whether the file said the camera is switched on.
    enabled: bool,
    frame: u32,
    /// Whether the span starting at `frame` is tweened.
    tweened: bool,
    /// A `__Camera__` instance is waiting for its `<Matrix>`.
    pending: bool,
    /// Each key, and whether the span after it moves.
    keys: Vec<(buzz_scene::CameraKey, bool)>,
}

impl CameraCapture {
    fn element(&mut self, name: &str, attrs: &HashMap<String, String>, stage: buzz_geom::Size) {
        match name {
            "DOMSymbolInstance" => {
                if attrs.get("libraryItemName").map(String::as_str) != Some("__Camera__") {
                    return;
                }
                // Recorded now, at rest: a camera keyframe with no matrix of
                // its own means the camera sits at the middle of the stage.
                self.keys.push((
                    buzz_scene::CameraKey::new(
                        self.frame,
                        buzz_geom::Point::new(stage.width / 2.0, stage.height / 2.0),
                    ),
                    self.tweened,
                ));
                self.pending = true;
            }
            "Matrix" if self.pending => {
                self.pending = false;
                let Some((key, _)) = self.keys.last_mut() else {
                    return;
                };
                let c = parse_matrix(attrs).as_coeffs();
                let scale = (c[0] * c[0] + c[1] * c[1]).sqrt();
                key.center = buzz_geom::Point::new(c[4], c[5]);
                // The inverse, and guarded: a degenerate camera matrix would
                // otherwise divide by zero and take the whole view with it.
                key.zoom = if scale > 1e-9 { 1.0 / scale } else { 1.0 };
                key.rotation = c[1].atan2(c[0]);
            }
            _ => {}
        }
    }
}

fn parse_document(
    xml: &str,
    scene: &mut Scene,
    symbols: &HashMap<String, SymbolId>,
    bitmaps: &BitmapMap,
    sounds: &SoundMap,
    ids: &mut IdSource,
    report: &mut ImportReport,
    which: usize,
) -> Result<(), ImportError> {
    let mut reader = quick_xml::Reader::from_str(xml);
    reader.config_mut().trim_text(true);
    // Reject mismatched or unclosed tags. A truncated file should be reported,
    // not half-imported into a document that looks complete.
    reader.config_mut().check_end_names = true;

    let mut layers: Vec<PendingLayer> = Vec::new();
    let mut context = FrameContext {
        bitmaps: bitmaps.clone(),
        sounds: sounds.clone(),
        ..Default::default()
    };
    let mut camera = CameraCapture::default();
    let mut open_elements = 0usize;
    // Symbol timelines live in LIBRARY and are parsed separately. Here each
    // top-level `<DOMTimeline>` is one of Animate's scenes, and only the
    // `which`-th is read: the others are skipped whole.
    let mut timeline_depth = 0usize;
    let mut timelines_seen = 0usize;
    let mut skipping = false;

    loop {
        let event = reader.read_event();
        // Track nesting so an unclosed document is caught: quick-xml reports
        // EOF rather than an error for a file that simply stops.
        match &event {
            Ok(Event::Start(_)) => open_elements += 1,
            Ok(Event::End(_)) => open_elements = open_elements.saturating_sub(1),
            _ => {}
        }

        match event {
            Ok(Event::Eof) => {
                if open_elements > 0 {
                    return Err(ImportError::Xml(format!(
                        "the document ends with {open_elements} unclosed element(s); \
                         it is probably truncated"
                    )));
                }
                break;
            }
            Ok(Event::Start(ref e)) | Ok(Event::Empty(ref e))
                if skipping || (timeline_depth == 0 && e.name().as_ref() == b"DOMTimeline") =>
            {
                // Another scene's timeline: counted, and nested timelines
                // tracked so its end is found, but nothing in it is read.
                let is_start = matches!(event, Ok(Event::Start(_)));
                if e.name().as_ref() == b"DOMTimeline" {
                    if timeline_depth == 0 {
                        let mine = timelines_seen == which;
                        timelines_seen += 1;
                        if is_start {
                            skipping = !mine;
                            timeline_depth += 1;
                            if mine {
                                let attrs = attributes(e);
                                camera.enabled = attr_bool(&attrs, "cameraLayerEnabled", false);
                            }
                        }
                    } else if is_start {
                        timeline_depth += 1;
                    }
                }
            }
            Ok(Event::Start(e)) | Ok(Event::Empty(e)) => {
                let name = String::from_utf8_lossy(e.name().as_ref()).to_string();
                let attrs = attributes(&e);

                match name.as_str() {
                    "DOMDocument" => {
                        let stage = scene.stage_mut();
                        stage.size = buzz_geom::Size::new(
                            attr_f64(&attrs, "width", 550.0),
                            attr_f64(&attrs, "height", 400.0),
                        );
                        stage.frame_rate = attr_f64(&attrs, "frameRate", 24.0);
                        stage.background = parse_color(&attrs, "backgroundColor", "__none");
                    }
                    "DOMTimeline" => {
                        timeline_depth += 1;
                        if timeline_depth == 1 {
                            camera.enabled = attr_bool(&attrs, "cameraLayerEnabled", false);
                        }
                    }
                    "DOMLayer" if timeline_depth <= 1 => {
                        context.flush_layer(&mut layers, report);
                        // The camera layer is not artwork: Animate hides it,
                        // and importing it as a layer would put an empty one
                        // at the top of every document that has a camera.
                        camera.active =
                            attrs.get("layerType").map(String::as_str) == Some("camera");
                        if !camera.active {
                            context.begin_layer(&attrs, ids);
                        }
                    }
                    "DOMFrame" if timeline_depth <= 1 => {
                        if camera.active {
                            camera.frame = attr_u32(&attrs, "index", 0);
                            camera.tweened =
                                attrs.get("tweenType").map(String::as_str) == Some("motion");
                        } else {
                            context.begin_frame(&attrs, report);
                        }
                    }
                    // **A Motion Tween's own nested document.** Parsed with a
                    // separate reader over the raw text between here and its
                    // closing tag, rather than folded into this flat
                    // one-level state machine — see `parse_motion_object`.
                    "motionObjectXML" if timeline_depth <= 1 && !camera.active => {
                        let start = reader.buffer_position() as usize;
                        if let Some(rel_end) =
                            xml.get(start..).and_then(|rest| rest.find("</motionObjectXML>"))
                        {
                            let motion = parse_motion_object(&xml[start..start + rel_end], report);
                            context.begin_motion_object(motion);
                        }
                    }
                    _ if timeline_depth <= 1 => {
                        if camera.active {
                            camera.element(&name, &attrs, scene.stage().size);
                        } else {
                            context.element(&name, &attrs, symbols, ids, report);
                        }
                    }
                    _ => {}
                }
            }
            Ok(Event::End(e)) => {
                let name = String::from_utf8_lossy(e.name().as_ref()).to_string();
                if name == "DOMTimeline" {
                    timeline_depth = timeline_depth.saturating_sub(1);
                    if timeline_depth == 0 {
                        skipping = false;
                    }
                    continue;
                }
                if skipping {
                    continue;
                }
                // The one closing tag that matters: a shape is only a shape
                // once every one of its edges has been read.
                if name == "DOMShape" && timeline_depth <= 1 {
                    context.finish_shape(ids, report);
                }
                // A custom ease's points are only complete once its own
                // closing tag arrives.
                if name == "CustomEase" && timeline_depth <= 1 {
                    context.end_custom_ease();
                }
            }
            Err(e) => return Err(ImportError::Xml(e.to_string())),
            _ => {}
        }
    }

    context.flush_layer(&mut layers, report);

    // The camera, if the document had one. `enabled` follows the file: a
    // document that switched its camera off should not open with it on.
    if !camera.keys.is_empty() {
        let track = scene.camera_mut();
        track.enabled = camera.enabled;

        // **A camera keyframe that is not tweened holds until the next one.**
        // Ours interpolate between every pair of keys, which is right for a
        // tweened move and wrong for the other nineteen spans in a real film:
        // a shot that should sit still for ten seconds and then cut was
        // instead drifting the whole way, arriving at the next shot's framing
        // just as the next shot began. A held span is written as a second key
        // with the same values at its last frame, so the hold is in the track
        // rather than in a rule somebody has to remember.
        for (index, (key, tweened)) in camera.keys.iter().enumerate() {
            report.camera_keys += 1;
            track.set_key(*key);

            let Some((next, _)) = camera.keys.get(index + 1) else {
                continue;
            };
            if !tweened && next.frame > key.frame + 1 {
                let mut hold = *key;
                hold.frame = next.frame - 1;
                track.set_key(hold);
            }
        }
    }

    resolve_layer_parents(&mut layers, report);

    // **The file's order is the timeline's order, top first.** This was read
    // backwards for five phases and the list was being reversed, which put
    // every document inside out: the sky in front of the artwork, a
    // background drawn over the characters standing on it, and — quietly
    // worse — every mask *below* the layers it claims, so masking did
    // nothing at all.
    //
    // The camera layer settles it. Animate keeps it pinned to the top of the
    // timeline and writes it as the first `<DOMLayer>`; and in every real file
    // a mask layer is written immediately *before* the layers it masks, which
    // is where Animate shows it — above them.
    if layers.is_empty() {
        layers.push(PendingLayer {
            layer: Layer::normal(LayerId(ids.take()), "Layer_1"),
            parent_index: None,
            rig_index: None,
            rig_parent: None,
            rig_parent_by_frame: Vec::new(),
            rig_prop_by_frame: Vec::new(),
        });
    }
    for (index, pending) in layers.into_iter().enumerate() {
        report.layers += 1;
        scene.edit_layers().insert(index, pending.layer);
    }
    Ok(())
}

/// A layer, plus the links it was written with, which cannot be resolved
/// until every layer has been seen and given an id.
struct PendingLayer {
    layer: Layer,
    /// The layer's own `parentLayerIndex`: the folder it sits in, or the mask
    /// that claims it. An index into the layer list in document order.
    parent_index: Option<usize>,
    /// This layer's `layerRiggingIndex`, which is how children name it.
    rig_index: Option<u32>,
    /// The rigging index of the layer this one follows, from its frames.
    rig_parent: Option<u32>,
    /// Each keyframe's own rig parent, frame by frame — see
    /// `FrameContext::rig_parent_by_frame`.
    rig_parent_by_frame: Vec<(u32, Option<u32>)>,
    /// Each keyframe's own `rigPropagationMatrix` — see `rig_compose`.
    rig_prop_by_frame: Vec<(u32, Option<Affine>)>,
}

/// Turn Animate's `parentLayerIndex` into our `parent` links.
///
/// # Why only folders
///
/// Animate uses one attribute for two different relationships: a layer inside
/// a **folder** points at the folder, and a **masked or guided** layer points
/// at the mask or guide governing it. Our model only has the first — masked
/// and guided layers resolve positionally, by a mask claiming the unbroken run
/// of layers beneath it, which is Animate's own rule and what the rest of the
/// engine already implements.
///
/// So a pointer at a folder becomes a `parent`, and a pointer at a **mask**
/// becomes the masked *kind* — which is the same relationship written the way
/// our positional rule can read it. Honouring the mask pointer as parenting
/// would put a masked layer inside its own mask and break that resolution.
///
/// # `layerType="masked"` is not what Animate writes
///
/// The format has the value, and current Animate does not use it: a masked
/// layer is an ordinary layer whose `parentLayerIndex` points at the mask
/// above it, exactly as a layer in a folder points at the folder. Waiting for
/// `layerType="masked"` meant every mask in every real document claimed
/// nothing and clipped nothing.
fn resolve_layer_parents(layers: &mut [PendingLayer], report: &mut ImportReport) {
    let ids: Vec<LayerId> = layers.iter().map(|p| p.layer.id).collect();
    let is_folder: Vec<bool> = layers
        .iter()
        .map(|p| p.layer.kind == LayerKind::Folder)
        .collect();
    let is_mask: Vec<bool> = layers.iter().map(|p| p.layer.kind.is_mask()).collect();

    for (index, pending) in layers.iter_mut().enumerate() {
        let Some(parent) = pending.parent_index else {
            continue;
        };
        // A layer cannot contain itself, and an index past the end is a
        // corrupt file rather than a relationship.
        if parent == index || parent >= ids.len() {
            continue;
        }
        if is_folder[parent] {
            pending.layer.parent = Some(ids[parent]);
        } else if is_mask[parent] && pending.layer.kind == LayerKind::Normal {
            pending.layer.kind = LayerKind::Masked;
        }
    }

    // **Layer Parenting is a different attribute, on the frame.** Animate's
    // rig links a layer to the one it follows with `layerRiggingIndex` on the
    // parent and `parentLayerIndex` on the child's *frame* — the link can
    // change over time, which is why it lives there. It is not the layer's own
    // `parentLayerIndex`, which is the folder it sits in.
    //
    // Without this a rigged character arrives in pieces: every part's matrix
    // is relative to the part it hangs off, so a head with a small offset from
    // its torso is drawn at that small offset from the *origin* instead, and
    // the character comes apart across the frame.
    let by_rig: HashMap<u32, LayerId> = layers
        .iter()
        .filter_map(|p| p.rig_index.map(|r| (r, p.layer.id)))
        .collect();
    for pending in layers.iter_mut() {
        let Some(parent) = pending.rig_parent else {
            continue;
        };
        let Some(id) = by_rig.get(&parent) else {
            continue;
        };
        if *id != pending.layer.id {
            pending.layer.follows = Some(*id);
        }
    }

    bake_rig_offsets(layers, report);
}

/// Turn Animate's *relative* rig transforms into absolute ones, frame by
/// frame.
///
/// # Two conventions for the same picture
///
/// In a rigged Animate character a part's matrix is written **relative to the
/// part it hangs off**: a head on a torso is `(49, -156)`, not a position; a
/// shin is a rotation about the knee, in the thigh's own space. Animate draws
/// it as `parent_now * child_now`, straight down the chain.
///
/// Our model does something different and deliberately so — a child's
/// transform is absolute, and layer parenting propagates only the parent's
/// *motion* away from its rest pose, which is what Animate's editor does when
/// you parent two layers that already sit where you want them.
///
/// # Keeping the rig, not just the picture
///
/// This used to compose the whole chain per keyframe —
/// `child_world(f) = parent_world(f) * child_relative(f)` — and then **drop the
/// rig link**, on the grounds that propagating the parent's motion again would
/// move the child twice. That produced the right picture and threw the rig
/// away: an imported character arrived as a stack of unrelated layers, so
/// moving the torso left the head behind, and the parenting an animator had
/// built in Animate was simply not there.
///
/// What is baked in is the parent's **rest** pose instead, and the link is
/// kept. That is exact at every depth, which is worth showing because the note
/// this replaces claimed otherwise. Writing `motion_of(P, f) = P(f)·P(rest)⁻¹`
/// for what our model propagates, and baking `C_stored(f) = P_rest·C_rel(f)`:
///
/// ```text
/// drawn C(f) = motion_of(P,f) · C_stored(f)
///            = P(f)·P(rest)⁻¹ · P_rest·C_rel(f)
///            = P(f)·C_rel(f)                     — which is what Animate draws
/// ```
///
/// And one level further down, where the old note expected it to come apart —
/// `motion_of(C,f) = C_stored(f)·C_stored(rest)⁻¹ = P_rest·C_rel(f)·C_rel(rest)⁻¹·P_rest⁻¹`,
/// so with `G_stored(f) = C_rest_world·G_rel(f)` the `P_rest` terms cancel and
/// `drawn G(f) = P(f)·C_rel(f)·G_rel(f)`. The rest pose that gets baked in is
/// the parent's *stored* transform at its own first keyframe — already the
/// composed rest chain, because parents are baked before their children — so
/// each level cancels the one above it.
///
/// The upshot: the character arrives posed exactly as Animate drew it **and**
/// still rigged, so dragging a parent layer carries its children.
fn bake_rig_offsets(layers: &mut [PendingLayer], report: &mut ImportReport) {
    use std::collections::HashMap;

    let follows: HashMap<LayerId, LayerId> = layers
        .iter()
        .filter_map(|p| p.layer.follows.map(|f| (p.layer.id, f)))
        .collect();
    if follows.is_empty() {
        return;
    }

    // Every rigged layer's own rigging index, so a keyframe's
    // `parentLayerIndex` can be resolved to the layer it actually names —
    // which is not always the one `follows` points at; see below.
    let by_rig: HashMap<u32, LayerId> = layers
        .iter()
        .filter_map(|p| p.rig_index.map(|r| (r, p.layer.id)))
        .collect();

    // How deep each layer hangs, so parents are baked before their children.
    // Bounded by the layer count, so a cycle in a corrupt file cannot spin.
    let count = layers.len();
    let depth = |id: LayerId| -> usize {
        let mut seen = Vec::new();
        let mut current = follows.get(&id).copied();
        for _ in 0..count {
            let Some(next) = current else { break };
            if seen.contains(&next) {
                break;
            }
            seen.push(next);
            current = follows.get(&next).copied();
        }
        seen.len()
    };

    let mut order: Vec<usize> = (0..layers.len()).collect();
    order.sort_by_key(|i| depth(layers[*i].layer.id));

    // **Animate's world, reproduced exactly at every keyframe.** Each child
    // keyframe's world is `rig_compose(parent_world, child, pivot, parent's
    // accumulated propagation)` — measured, see `rig_compose`. What is stored
    // is that world with the runtime's own inherited motion divided back out,
    // so `inherited_transform · stored` (what the renderer draws) lands on it.
    let position = |layers: &[PendingLayer], id: LayerId| layers.iter().position(|p| p.layer.id == id);
    let first_transform = |frames: &buzz_scene::LayerTimeline, at: u32| -> Option<Affine> {
        frames.resolved_at(at).iter().next().map(|o| o.transform)
    };
    // The runtime's inherited motion for layer `id` at `at` — the same
    // product `LayerStack::inherited_transform` draws with, over the layers
    // baked so far.
    fn inherited(layers: &[PendingLayer], id: LayerId, at: u32) -> Affine {
        let mut chain = Vec::new();
        let mut current = layers.iter().find(|p| p.layer.id == id).and_then(|p| p.layer.follows);
        for _ in 0..layers.len() {
            let Some(next) = current else { break };
            if chain.contains(&next) {
                break;
            }
            chain.push(next);
            current = layers.iter().find(|p| p.layer.id == next).and_then(|p| p.layer.follows);
        }
        let mut out = Affine::IDENTITY;
        for followed in chain.iter().rev() {
            let Some(layer) = layers.iter().find(|p| p.layer.id == *followed) else { continue };
            let Some(now) = layer.layer.frames.resolved_at(at).iter().next().map(|o| o.transform) else {
                continue;
            };
            let rest = layer.layer.rest_pose.unwrap_or_else(|| parent_rest_pose(&layer.layer.frames));
            if rest.determinant().abs() < 1e-12 {
                continue;
            }
            out *= now * rest.inverse();
        }
        out
    }
    fn own_prop(p: &PendingLayer, at: u32) -> Affine {
        p.rig_prop_by_frame
            .iter()
            .rev()
            .find(|(start, _)| *start <= at)
            .and_then(|(_, m)| *m)
            .unwrap_or(Affine::IDENTITY)
    }
    // Accumulated propagation per layer, per keyframe start: a root passes on
    // its own; a child passes on its parent's times its own.
    let mut props: HashMap<LayerId, Vec<(u32, Affine)>> = layers
        .iter()
        .map(|p| {
            let v = p.layer.frames.keyframes().iter().map(|k| (k.start, own_prop(p, k.start))).collect();
            (p.layer.id, v)
        })
        .collect();
    let prop_at = |props: &HashMap<LayerId, Vec<(u32, Affine)>>, id: LayerId, at: u32| -> Affine {
        props
            .get(&id)
            .and_then(|v| v.iter().rev().find(|(s, _)| *s <= at).or(v.first()))
            .map(|(_, m)| *m)
            .unwrap_or(Affine::IDENTITY)
    };

    for index in order {
        let Some(parent) = layers[index].layer.follows else {
            continue;
        };
        let Some(parent_index) = position(layers, parent) else {
            layers[index].layer.follows = None;
            continue;
        };
        // The pose the runtime measures the parent's motion from.
        let parent_rest = parent_rest_pose(&layers[parent_index].layer.frames);
        layers[parent_index].layer.rest_pose = Some(parent_rest);
        let chosen_rig_index = layers[parent_index].rig_index;

        let mut retargeted_this_layer = false;
        let mut child_props = Vec::new();
        let keyframes = layers[index].layer.frames.keyframes().to_vec();
        let mut moved = Vec::with_capacity(keyframes.len());
        for keyframe in &keyframes {
            let s = keyframe.start;
            // Which layer this keyframe hangs off: usually `follows`, rarely a
            // different rigged layer named on the keyframe itself (Findings §4).
            let named = layers[index]
                .rig_parent_by_frame
                .iter()
                .rev()
                .find(|(start, _)| *start <= s)
                .and_then(|(_, n)| *n);
            let hung_off = match named {
                Some(n) if Some(n) != chosen_rig_index => by_rig.get(&n).copied().filter(|id| *id != parent),
                _ => None,
            };
            if hung_off.is_some() {
                retargeted_this_layer = true;
            }
            let from = hung_off.unwrap_or(parent);
            let Some(from_index) = position(layers, from) else {
                moved.push(keyframe.clone());
                continue;
            };
            // A parent with nothing drawn here (a blank key before the rig
            // appears) is taken at its rest pose, its first drawn one.
            let parent_world = inherited(layers, from, s)
                * first_transform(&layers[from_index].layer.frames, s)
                    .unwrap_or_else(|| parent_rest_pose(&layers[from_index].layer.frames));
            let parent_prop = prop_at(&props, from, s);
            child_props.push((s, parent_prop * own_prop(&layers[index], s)));
            // The runtime draws this keyframe under its `follows` chain.
            let runtime = inherited(layers, layers[index].layer.id, s);
            let undo = if runtime.determinant().abs() > 1e-12 { runtime.inverse() } else { Affine::IDENTITY };
            let objects: Vec<std::sync::Arc<Object>> = keyframe
                .objects
                .iter()
                .map(|object| {
                    let mut copy = (**object).clone();
                    let world = rig_compose(parent_world, copy.transform, copy.pivot, parent_prop);
                    copy.transform = undo * world;
                    std::sync::Arc::new(copy)
                })
                .collect();
            moved.push(buzz_scene::Keyframe {
                objects: std::sync::Arc::new(objects),
                ..keyframe.clone()
            });
        }

        let length = layers[index].layer.frames.length();
        layers[index].layer.frames = buzz_scene::LayerTimeline::from_parts(moved, length);
        props.insert(layers[index].layer.id, child_props);
        // **The link stays**, so dragging a parent layer in the editor still
        // carries its children.
        if retargeted_this_layer {
            report.retargeted_rig_layers += 1;
        }
    }
}

/// An XFL `<Color>` colour effect as a multiply-then-add transform.
///
/// Animate writes the preset it came from (`tintColor` + `tintMultiplier`,
/// or `brightness`) and, for Advanced, the raw per-channel `…Multiplier`
/// (default 1) and `…Offset` (-255..255, default 0). The preset is applied
/// first; any raw channel value the file also gives overrides it.
fn parse_color_effect(attrs: &HashMap<String, String>) -> buzz_scene::ColorTransform {
    let get = |k: &str| attrs.get(k).and_then(|v| v.parse::<f32>().ok());
    let mut multiply = [1.0f32; 4];
    let mut add = [0.0f32; 4];
    if let Some(amount) = get("tintMultiplier") {
        let tint = parse_color(attrs, "tintColor", "__no_alpha__");
        let rgba = tint.to_rgba8();
        for (i, c) in [rgba.r, rgba.g, rgba.b].into_iter().enumerate() {
            multiply[i] = 1.0 - amount;
            add[i] = c as f32 / 255.0 * amount;
        }
    }
    if let Some(bright) = get("brightness") {
        for i in 0..3 {
            multiply[i] = 1.0 - bright.abs();
            add[i] = bright.max(0.0);
        }
    }
    for (i, ch) in ["red", "green", "blue", "alpha"].into_iter().enumerate() {
        if let Some(m) = get(&format!("{ch}Multiplier")) {
            multiply[i] = m;
        }
        if let Some(o) = get(&format!("{ch}Offset")) {
            add[i] = o / 255.0;
        }
    }
    buzz_scene::ColorTransform { multiply, add }
}

/// A bitmap from Animate's `bin/*.dat` cache.
enum DatBitmap {
    /// A JPEG or PNG kept as it came, for `ImageAsset::decode`.
    Encoded(Vec<u8>),
    /// Decoded pixels, straight-alpha RGBA8.
    Pixels { width: u32, height: u32, rgba: Vec<u8> },
}

/// Decode one of Animate's cached bitmaps.
///
/// Two lossless layouts share a 24-byte header: a `u16` tag, the row stride in
/// bytes, width and height (`u16` each), then the frame's left, right, top and
/// bottom in twips (`u32` each), all little-endian.
/// - `0x0503`, 32-bit: a has-alpha byte and a compressed byte, then the pixels
///   as **premultiplied A, R, G, B**.
/// - `0x0303`, 8-bit palette: a has-alpha byte, a `u16` colour count, that
///   many colours as A, R, G, B, a compressed byte, then one index per pixel
///   over `stride`-byte rows.
///
/// The pixels are one zlib stream cut into chunks, each a `u16` length and
/// then that many bytes, ending at a zero length. Anything else, JPEG
/// included, starts with its own file signature and is handed back whole.
fn decode_bitmap_dat(bytes: &[u8]) -> Option<DatBitmap> {
    if bytes.starts_with(&[0xFF, 0xD8]) || bytes.starts_with(&[0x89, b'P', b'N', b'G']) {
        return Some(DatBitmap::Encoded(bytes.to_vec()));
    }
    let u16_at = |o: usize| -> Option<u16> { Some(u16::from_le_bytes([*bytes.get(o)?, *bytes.get(o + 1)?])) };
    let tag = u16_at(0)?;
    let stride = u16_at(2)? as usize;
    let width = u16_at(4)? as usize;
    let height = u16_at(6)? as usize;
    if width == 0 || height == 0 {
        return None;
    }
    let inflate = |mut offset: usize| -> Option<Vec<u8>> {
        let mut zipped = Vec::new();
        loop {
            let length = u16_at(offset)? as usize;
            offset += 2;
            if length == 0 {
                break;
            }
            zipped.extend_from_slice(bytes.get(offset..offset + length)?);
            offset += length;
        }
        let mut out = Vec::new();
        std::io::Read::read_to_end(&mut flate2::read::ZlibDecoder::new(&zipped[..]), &mut out).ok()?;
        Some(out)
    };
    let mut rgba = vec![0u8; width * height * 4];
    match tag {
        0x0503 => {
            let has_alpha = *bytes.get(0x18)? != 0;
            let raw = inflate(0x1A)?;
            if raw.len() < width * height * 4 {
                return None;
            }
            for (out, px) in rgba.chunks_exact_mut(4).zip(raw.chunks_exact(4)) {
                let a = px[0];
                let unpremultiply = |c: u8| -> u8 {
                    if a == 0 || a == 255 { c } else { ((c as u32 * 255 + a as u32 / 2) / a as u32).min(255) as u8 }
                };
                out.copy_from_slice(&[
                    unpremultiply(px[1]),
                    unpremultiply(px[2]),
                    unpremultiply(px[3]),
                    if has_alpha { a } else { 255 },
                ]);
            }
        }
        0x0303 => {
            let count = u16_at(0x19)? as usize;
            let palette_at = 0x1B;
            let palette = bytes.get(palette_at..palette_at + count * 4)?;
            let raw = inflate(palette_at + count * 4 + 1)?;
            if stride < width || raw.len() < stride * height {
                return None;
            }
            for y in 0..height {
                for x in 0..width {
                    let index = raw[y * stride + x] as usize;
                    let out = &mut rgba[(y * width + x) * 4..][..4];
                    if index < count {
                        let c = &palette[index * 4..][..4];
                        out.copy_from_slice(&[c[1], c[2], c[3], c[0]]);
                    }
                }
            }
        }
        _ => return None,
    }
    Some(DatBitmap::Pixels { width: width as u32, height: height as u32, rgba })
}

/// A frame's `rigPropagationMatrix`, `{"a":…,"b":…,"c":…,"d":…,"tx":…,"ty":…}`
/// in 16.16 fixed point. Only its linear part is used — see `rig_compose`.
fn parse_rig_propagation(text: &str) -> Option<Affine> {
    let field = |key: &str| -> Option<f64> {
        let at = text.find(&format!("\"{key}\""))? + key.len() + 2;
        let rest = text[at..].trim_start().strip_prefix(':')?.trim_start();
        let end = rest.find(|c: char| !(c == '-' || c.is_ascii_digit())).unwrap_or(rest.len());
        rest[..end].parse::<f64>().ok()
    };
    Some(Affine::new([
        field("a")? / 65536.0,
        field("b")? / 65536.0,
        field("c")? / 65536.0,
        field("d")? / 65536.0,
        0.0,
        0.0,
    ]))
}

/// Where Animate's layer parenting draws a child, given its parent's world
/// matrix `parent`, the child's stored (parent-relative) matrix `child`, the
/// child's transformation point `pivot` in its own space, and `propagation`,
/// the parent's accumulated `rigPropagationMatrix`.
///
/// **Measured in Adobe Animate 2024**, not guessed: barish1's rigged
/// characters were detached from their parents inside Animate (which keeps
/// every part where it is drawn and rewrites its matrix in world space), and
/// every part's world matrix matched this:
/// - the child's **transformation point** (its joint) is placed by the
///   parent's whole matrix, scale included;
/// - the child's **own shape** is not scaled by the parent. It is carried by
///   the propagation matrix Animate stores on each rigged keyframe
///   (`rigPropagationMatrix`), accumulated down the chain: a root passes on
///   its own; a child passes on its parent's times its own. Usually that is
///   just the parent's turn (a half turn for a mirrored parent), but not
///   always: barish1's wrists carry a skew their palms inherit.
///
/// ```text
/// world = T(parent · (child · pivot)) · propagation · linear(child) · T(-pivot)
/// ```
///
/// Composing the full matrices instead shrank every part hung under a scaled
/// parent (the kurta man's torso came out at 0.53 × 0.57 of its size) and
/// pulled heads and hands off their joints.
fn rig_compose(parent: Affine, child: Affine, pivot: Option<buzz_geom::Point>, propagation: Affine) -> Affine {
    let pivot = pivot.unwrap_or(buzz_geom::Point::ORIGIN);
    let [a, b, c, d, ..] = child.as_coeffs();
    let joint = parent * (child * pivot);
    let [pa, pb, pc, pd, ..] = propagation.as_coeffs();
    let shape = Affine::new([pa, pb, pc, pd, 0.0, 0.0]) * Affine::new([a, b, c, d, 0.0, 0.0]);
    Affine::translate(joint.to_vec2()) * shape * Affine::translate(-pivot.to_vec2())
}

/// The pose a rig's children are baked against: the parent's own artwork on
/// its rest frame.
///
/// # The first keyframe **that has an object**
///
/// A parent layer occasionally opens on a blank keyframe — a held clear
/// before the rig appears — and reading the rest pose from that gave every
/// child an identity matrix to rig against instead of the parent's real
/// pose (Findings §4). So this looks past blank keyframes to the first one
/// that actually draws something.
///
/// # The first **symbol instance**, not just the first object
///
/// When that keyframe holds more than one object, Animate rigs a child
/// against the instance — not against, say, a loose shape drawn on the same
/// layer for reference. Falls back to the first object when the keyframe
/// holds no instance at all.
fn parent_rest_pose(frames: &buzz_scene::LayerTimeline) -> Affine {
    let Some(rest) = frames
        .keyframes()
        .iter()
        .find(|k| !k.objects.is_empty())
        .map(|k| k.start)
    else {
        return Affine::IDENTITY;
    };
    let resolved = frames.resolved_at(rest);
    let objects: Vec<&Object> = resolved.iter().collect();
    objects
        .iter()
        .find(|o| o.instance().is_some())
        .or_else(|| objects.first())
        .map(|o| o.transform)
        .unwrap_or(Affine::IDENTITY)
}

/// Accumulates layers, frames and elements while walking the XML.
#[derive(Default)]
struct FrameContext {
    bitmaps: BitmapMap,
    /// Every sound in the library, by the name a frame's `soundName` refers
    /// to it by.
    sounds: SoundMap,
    layer: Option<Layer>,
    keyframes: Vec<buzz_scene::Keyframe>,
    length: u32,
    current: Option<PendingFrame>,
    /// The `parentLayerIndex` of the layer being read, kept beside it because
    /// it cannot be resolved until every layer has been seen and given an id.
    parent_index: Option<usize>,
    /// The layer's `layerRiggingIndex`, and the rigging index of whatever its
    /// frames say it follows. Both wait for the same reason.
    rig_index: Option<u32>,
    rig_parent: Option<u32>,
    /// Style tables for the `DOMShape` currently being read.
    ///
    /// XFL declares fills and strokes once per shape and then has each edge
    /// reference them by index, so the tables have to be accumulated before
    /// the edges that use them can be coloured.
    styles: ShapeStyleTable,
    /// The boundary pieces of the `DOMShape` currently being read.
    ///
    /// Held for the same reason as the styles, and a stronger one: a fill's
    /// outline is spread across several edges and none of them is a shape on
    /// its own.
    edges: Vec<edge::EdgeRecord>,
    /// An instance has just been pushed and is waiting for its `<Matrix>`.
    placing: bool,
    /// A `<DOMShape>` has opened and its own `<matrix>`, if it has one, has
    /// not arrived yet. Only a shape inside a `<DOMGroup>` (or a drawing
    /// object) carries one; it comes before the shape's fills and edges.
    shape_wants_matrix: bool,
    /// The open shape's own placement: its edges are in its own space.
    shape_matrix: Option<Affine>,
    /// The open shape says `isDrawingObject="true"`.
    shape_drawing_object: bool,
    /// A `<transformationPoint>` is open and the `<Point>` inside it belongs
    /// to the object just pushed. `<Point>` appears elsewhere in XFL, so it
    /// is only taken when something has asked for it.
    reading_pivot: bool,
    /// An instance's `<color>` is open: the `<Color>` inside it is that
    /// instance's colour effect (alpha, tint, brightness, advanced).
    reading_color: bool,
    /// The open frame's `<SoundEnvelope>` has given its volume already.
    /// `<SoundEnvelopePoint>` can appear more than once, to shape the volume
    /// over the clip's length; only the first is read for now, so later ones
    /// must not overwrite it.
    envelope_taken: bool,
    /// A `<CustomEase>` is open, and which property it eases —
    /// `target="all" | "position" | "rotation" | "scale" | "color"` — so its
    /// `<Point>` children (which are `<Point>` the same way a
    /// `<transformationPoint>`'s is, and must not be confused with it) are
    /// collected into the right curve.
    reading_custom_ease: Option<String>,
    /// Points accumulated for the open `<CustomEase>`, in file order: the
    /// curve's own start, then two control points and an end point per
    /// segment. Built by `<Point>` and committed by `end_custom_ease`.
    custom_ease_points: Vec<buzz_geom::Point>,
    /// Each finished keyframe's own `parentLayerIndex`, alongside
    /// `keyframes` — `bake_rig_offsets` reads this to find the rare layer
    /// that re-parents mid-timeline, which `rig_parent` above (the layer's
    /// single, first-stated link) cannot represent.
    rig_parent_by_frame: Vec<(u32, Option<u32>)>,
    /// Each finished keyframe's own `rigPropagationMatrix`, the same way.
    rig_prop_by_frame: Vec<(u32, Option<Affine>)>,
}

/// Flash's gradients are declared in a fixed square 32 768 twips across —
/// 1 638.4 pixels, running from −819.2 to +819.2 — and the matrix in the file
/// maps *that* onto the artwork. Our own unit space is −1 to 1, so this is the
/// factor between them.
///
/// It is the single number that decides whether an imported gradient is the
/// right size, which is why it is named rather than written inline.
const XFL_GRADIENT_HALF_BOX: f64 = 819.2;

/// The fills and strokes declared by one `DOMShape`.
#[derive(Default)]
struct ShapeStyleTable {
    fills: HashMap<u32, Paint>,
    /// Index to (paint, weight).
    strokes: HashMap<u32, (Paint, f64)>,
    /// Which style the elements now arriving belong to.
    ///
    /// `SolidColor` appears under both `FillStyle` and `StrokeStyle`, so the
    /// enclosing style has to be remembered to know which table to write to.
    /// The document is walked as a flat event stream, so this is how nesting
    /// is recovered.
    current: Option<StyleSlot>,
    /// The gradient being read, if one is open.
    ///
    /// Built up as its parts arrive and written to the style table after each,
    /// because this is a flat walk that never sees a closing tag. XFL puts the
    /// `<Matrix>` before the `<GradientEntry>` list, so the placement is
    /// usually known before the first stop — but nothing here depends on that
    /// order.
    gradient: Option<Gradient>,
    /// The stops as the *file* gave them.
    ///
    /// Kept apart from the gradient's own list because [`Gradient::set_stops`]
    /// pads a list of fewer than two up to two, so a gradient that has been
    /// committed once already holds stops the file never wrote — and reading
    /// them back to append the next one counts the padding as real.
    gradient_stops: Vec<GradientStop>,
    /// A gradient is open and has not yet been given its matrix.
    ///
    /// `<Matrix>` means several different things in XFL depending on what
    /// encloses it; this is how the gradient's own is told apart from the one
    /// that places a symbol instance.
    gradient_wants_matrix: bool,
    /// A `<BitmapFill>` is open and waiting for its `<Matrix>`: the image laid
    /// across its natural size, before the fill's own placement.
    bitmap_wants_matrix: Option<buzz_scene::ImageFill>,
}

#[derive(Clone, Copy)]
enum StyleSlot {
    Fill(u32),
    Stroke(u32),
}

impl ShapeStyleTable {
    fn begin_shape(&mut self) {
        self.fills.clear();
        self.strokes.clear();
        self.current = None;
        self.end_gradient();
    }

    fn end_gradient(&mut self) {
        self.bitmap_wants_matrix = None;
        self.gradient = None;
        self.gradient_stops.clear();
        self.gradient_wants_matrix = false;
    }

    /// Record a solid colour against whichever style is being read.
    fn set_color(&mut self, color: Color) {
        self.set_paint(Paint::Solid(color));
    }

    /// Record a paint against whichever style is being read.
    fn set_paint(&mut self, paint: Paint) {
        match self.current {
            Some(StyleSlot::Fill(index)) => {
                self.fills.insert(index, paint);
            }
            Some(StyleSlot::Stroke(index)) => {
                // Keep whatever weight `SolidStroke` already recorded.
                let width = self.strokes.get(&index).map(|(_, w)| *w).unwrap_or(1.0);
                self.strokes.insert(index, (paint, width));
            }
            None => {}
        }
    }

    fn set_stroke_width(&mut self, width: f64) {
        if let Some(StyleSlot::Stroke(index)) = self.current {
            let paint = self
                .strokes
                .get(&index)
                .map(|(p, _)| p.clone())
                .unwrap_or(Paint::Solid(Color::BLACK));
            self.strokes.insert(index, (paint, width));
        }
    }

    /// Open a bitmap fill: the picture at its natural size until its own
    /// `<Matrix>` arrives. Animate tiles a bitmap fill unless it is marked
    /// `bitmapIsClipped`.
    fn begin_bitmap(&mut self, mut fill: buzz_scene::ImageFill, clipped: bool) {
        self.end_gradient();
        fill.tile = !clipped;
        self.set_paint(Paint::Image(Box::new(fill.clone())));
        self.bitmap_wants_matrix = Some(fill);
    }

    /// Claim a `<Matrix>` for the open bitmap fill, if there is one. XFL writes
    /// it in twips per image pixel, like SWF: `a="20"` is the picture at 1:1.
    fn place_bitmap(&mut self, matrix: Affine) -> bool {
        let Some(mut fill) = self.bitmap_wants_matrix.take() else {
            return false;
        };
        let [a, b, c, d, e, f] = matrix.as_coeffs();
        let placed = Affine::new([a / 20.0, b / 20.0, c / 20.0, d / 20.0, e, f]);
        fill.transform = placed * fill.transform;
        self.set_paint(Paint::Image(Box::new(fill)));
        true
    }

    /// Open a gradient. Its stops and its matrix arrive as later elements.
    fn begin_gradient(&mut self, kind: GradientKind, spread: GradientSpread, focal: f64) {
        self.gradient_stops.clear();
        let mut g = Gradient::new(kind, Vec::new());
        g.spread = spread;
        g.focal = focal;
        // Until the matrix arrives the gradient stands at unit size, which is
        // 2 pixels across. A file that omits the matrix is malformed, and a
        // tiny ramp is a visible symptom rather than a silent one — but Flash's
        // own default is the full gradient box, so that is what is used.
        g.transform = Affine::scale(XFL_GRADIENT_HALF_BOX);
        self.gradient = Some(g);
        self.gradient_wants_matrix = true;
        self.commit_gradient();
    }

    /// Place the open gradient with the matrix from the file.
    ///
    /// Returns whether the matrix was claimed, so the caller knows not to treat
    /// it as an instance placement.
    fn place_gradient(&mut self, m: Affine) -> bool {
        if !self.gradient_wants_matrix {
            return false;
        }
        self.gradient_wants_matrix = false;
        if let Some(g) = &mut self.gradient {
            // The file's matrix maps Flash's gradient square; ours maps the
            // unit one. Scaling first turns the second into the first, and then
            // the file's matrix puts it on the artwork.
            g.transform = m * Affine::scale(XFL_GRADIENT_HALF_BOX);
        }
        self.commit_gradient();
        true
    }

    /// Add a stop to the open gradient.
    ///
    /// Falls back to treating the colour as solid when no gradient is open,
    /// which is what a `<GradientEntry>` outside one would otherwise be: lost.
    fn add_gradient_stop(&mut self, color: Color, ratio: f64) {
        if self.gradient.is_none() {
            self.set_color(color);
            return;
        }
        self.gradient_stops.push(GradientStop::new(ratio, color));
        let stops = self.gradient_stops.clone();
        if let Some(g) = &mut self.gradient {
            g.set_stops(stops);
        }
        self.commit_gradient();
    }

    /// Write the gradient as it currently stands into the style table.
    ///
    /// Done after every part because this walk never sees a closing tag: the
    /// style has to be correct after each element in case that element was the
    /// last one.
    fn commit_gradient(&mut self) {
        if let Some(g) = self.gradient.clone() {
            self.set_paint(Paint::Gradient(std::sync::Arc::new(g)));
        }
    }
}

/// XFL's `spreadMethod`, which is Animate's Extend / Reflect / Repeat.
fn parse_spread(attrs: &HashMap<String, String>) -> GradientSpread {
    match attrs.get("spreadMethod").map(String::as_str) {
        Some("reflect") => GradientSpread::Reflect,
        Some("repeat") => GradientSpread::Repeat,
        // "extend" is Animate's default and what it omits when writing.
        _ => GradientSpread::Pad,
    }
}

struct PendingFrame {
    start: u32,
    duration: u32,
    label: Option<String>,
    tween: Tween,
    objects: Vec<std::sync::Arc<Object>>,
    sound: Option<SoundRef>,
    /// Per-property custom eases from this frame's `<tweens>`, if it had any.
    ease_curve: Option<buzz_scene::EasingSet>,
    /// This frame's own `parentLayerIndex`, unlike `FrameContext::rig_parent`
    /// which keeps only the first one a layer states. Most layers never
    /// re-parent, so this is the same value on every one of their keyframes;
    /// `bake_rig_offsets` reads it to catch the few that do.
    rig_parent: Option<u32>,
    /// This frame's `rigPropagationMatrix`: the linear part Animate passes on
    /// to the layers hung off this one (see `rig_compose`).
    rig_prop: Option<Affine>,
    /// This frame's `<motionObjectXML>` (Animate's Motion Tween), parsed —
    /// `None` for every other kind of frame. `finish_frame` bakes it into
    /// ordinary per-frame keyframes instead of pushing this one frame as is.
    motion_object: Option<MotionObject>,
}

impl FrameContext {
    fn begin_layer(&mut self, attrs: &HashMap<String, String>, ids: &mut IdSource) {
        let name = attrs
            .get("name")
            .cloned()
            .unwrap_or_else(|| "Layer".to_string());
        let mut layer = Layer::new(LayerId(ids.take()), name, parse_layer_kind(attrs));
        layer.visible = attr_bool(attrs, "visible", true);
        layer.locked = attr_bool(attrs, "locked", false);
        layer.outline = attr_bool(attrs, "outline", false);
        if let Some(color) = attrs.get("color") {
            let map = HashMap::from([("c".to_string(), color.clone())]);
            layer.color = parse_color(&map, "c", "__none");
        }
        self.layer = Some(layer);
        self.parent_index = attrs
            .get("parentLayerIndex")
            .and_then(|v| v.trim().parse::<usize>().ok());
        // How this layer is named by the children that follow it.
        self.rig_index = attrs
            .get("layerRiggingIndex")
            .and_then(|v| v.trim().parse::<u32>().ok());
        self.rig_parent = None;
        self.keyframes.clear();
        self.length = 0;
    }

    /// Turn the edges collected since the last `DOMShape` into artwork.
    ///
    /// One object per fill and one per stroke, rather than one per `<Edge>`:
    /// a fill's boundary is spread across the edges, so an edge on its own is
    /// a fragment of an outline and not a shape at all.
    ///
    /// **Fills first, then strokes**, which is Animate's own order and the
    /// reason a drawn line sits on top of the colour it encloses rather than
    /// half under it.
    fn finish_shape(&mut self, ids: &mut IdSource, report: &mut ImportReport) {
        let records = std::mem::take(&mut self.edges);
        if records.is_empty() {
            return;
        }
        let Some(frame) = self.current.as_mut() else {
            return;
        };

        let mut made = 0usize;
        let mut push =
            |path: buzz_geom::BezPath, fill: Option<Paint>, stroke: Option<(Paint, f64)>| {
                if path.elements().is_empty() {
                    return;
                }
                frame.objects.push(std::sync::Arc::new(Object::shape(
                    ObjectId(ids.take()),
                    ShapeData {
                        path,
                        fill: fill.map(|paint| FillSpec {
                            paint,
                            rule: buzz_geom::FillMode::NonZero,
                            swatch: None,
                        }),
                        stroke: stroke.map(|(paint, width)| StrokeSpec {
                            paint,
                            width,
                            hairline: false,
                            swatch: None,
                        }),
                        // No source format expresses build-up paint, so imported
                        // artwork always composites normally.
                        blend: buzz_scene::PaintBlend::Normal,
                    },
                )));
                made += 1;
            };

        let (fills, gaps) = edge::assemble_fills_counted(&records);
        for (index, path) in fills {
            // A fill referencing a style the file never declared still has to
            // be visible, or artwork silently vanishes.
            let paint = self
                .styles
                .fills
                .get(&index)
                .cloned()
                .unwrap_or(Paint::Solid(Color::from_rgb8(0x99, 0x99, 0x99)));
            push(path, Some(paint), None);
        }

        for (index, path) in edge::assemble_strokes(&records) {
            let (paint, width) = self
                .styles
                .strokes
                .get(&index)
                .cloned()
                .unwrap_or((Paint::Solid(Color::BLACK), 1.0));
            push(path, None, Some((paint, width)));
        }

        // The shape's own placement, when it has one (see `shape_matrix`).
        if let Some(matrix) = self.shape_matrix.take()
            && let Some(frame) = self.current.as_mut()
        {
            let from = frame.objects.len().saturating_sub(made);
            for object in &mut frame.objects[from..] {
                let object = std::sync::Arc::make_mut(object);
                object.transform = matrix * object.transform;
            }
        }
        // A drawing object stays out of later Merge strokes.
        if std::mem::take(&mut self.shape_drawing_object)
            && let Some(frame) = self.current.as_mut()
        {
            let from = frame.objects.len().saturating_sub(made);
            for object in &mut frame.objects[from..] {
                std::sync::Arc::make_mut(object).drawing_object = true;
            }
        }
        self.shape_wants_matrix = false;
        // Counted after the closure has finished with `report`.
        report.shapes += made;
        if gaps > 0 {
            report.note_unsupported("a fill outline that does not close on itself");
        }
    }

    fn begin_frame(&mut self, attrs: &HashMap<String, String>, report: &mut ImportReport) {
        self.finish_frame(report);
        // This frame's own rig parent, whatever it says.
        let frame_rig_parent = attrs
            .get("parentLayerIndex")
            .and_then(|v| v.trim().parse::<u32>().ok());
        // Which layer this one follows, by rigging index. Animate keeps it on
        // the frame because a rig can be re-parented part way through a shot;
        // our model has one link per layer, so the first one stated wins —
        // `frame_rig_parent` above keeps every frame's own, for
        // `bake_rig_offsets` to catch the rare one that changes.
        if self.rig_parent.is_none() {
            self.rig_parent = frame_rig_parent;
        }
        let start = attr_u32(attrs, "index", 0);
        let duration = attr_u32(attrs, "duration", 1).max(1);
        let tween = parse_tween(attrs);
        if tween.is_active() {
            report.tweens += 1;
        }
        self.envelope_taken = false;
        self.reading_custom_ease = None;
        self.custom_ease_points.clear();
        self.current = Some(PendingFrame {
            start,
            duration,
            label: attrs.get("name").cloned().filter(|s| !s.is_empty()),
            tween,
            objects: Vec::new(),
            sound: parse_sound_ref(attrs, &self.sounds),
            ease_curve: None,
            rig_parent: frame_rig_parent,
            rig_prop: attrs.get("rigPropagationMatrix").and_then(|v| parse_rig_propagation(v)),
            motion_object: None,
        });
        report.keyframes += 1;
    }

    fn element(
        &mut self,
        name: &str,
        attrs: &HashMap<String, String>,
        symbols: &HashMap<String, SymbolId>,
        ids: &mut IdSource,
        report: &mut ImportReport,
    ) {
        let Some(frame) = self.current.as_mut() else {
            return;
        };

        // **An instance claims only the matrix that immediately follows it.**
        // `<matrix>` is the first child of `<DOMSymbolInstance>`, so anything
        // else arriving first means this instance has no matrix of its own and
        // sits at the origin. Letting the claim stand was the worse bug: the
        // next `<Matrix>` in the file is usually a *gradient's*, whose scale is
        // a fraction of a percent, and the instance collapsed to a point — a
        // lantern, a bed and a cot vanishing from a shot while the layer
        // reported them present.
        if !matches!(name, "matrix" | "Matrix" | "DOMSymbolInstance") {
            self.placing = false;
        }

        // **And a gradient claims only the matrix immediately inside it**, by
        // exactly the same discipline and for exactly the same reason.
        //
        // Adobe writes `<matrix>` as the first child of a gradient, before the
        // `<GradientEntry>` list, so anything else arriving first means this
        // gradient has none. Leaving the claim armed was the same bug the
        // comment above describes, running the other way: a gradient without a
        // matrix would swallow the matrix of the *next symbol instance* in the
        // file, and that instance — and every one after it whose gradient did
        // the same — collapsed to the origin.
        if !matches!(name, "matrix" | "Matrix") {
            self.styles.gradient_wants_matrix = false;
        }

        match name {
            // A new shape starts a new set of style tables — and finishes any
            // shape still open, so a file that nests one inside a group still
            // draws both.
            "DOMShape" => {
                self.finish_shape(ids, report);
                self.styles.begin_shape();
                self.shape_wants_matrix = true;
                self.shape_matrix = None;
                self.shape_drawing_object =
                    attrs.get("isDrawingObject").is_some_and(|v| v.trim() == "true");
            }
            // Past the point where a shape's own matrix could come.
            "fills" | "strokes" | "edges" => {
                self.shape_wants_matrix = false;
            }

            "FillStyle" => {
                self.styles.current = Some(StyleSlot::Fill(attr_index(attrs)));
                self.styles.end_gradient();
            }
            "StrokeStyle" => {
                self.styles.current = Some(StyleSlot::Stroke(attr_index(attrs)));
                self.styles.end_gradient();
            }
            // Every stroke kind carries its weight the same way; the dash and
            // stipple variants differ only in decoration we cannot draw yet.
            "SolidStroke" | "DashedStroke" | "DottedStroke" | "RaggedStroke" | "StippleStroke"
            | "HatchedStroke" => {
                self.styles.set_stroke_width(attr_f64(attrs, "weight", 1.0));
                if name != "SolidStroke" {
                    report.note_unsupported("a decorated stroke, imported as a plain one");
                }
            }
            "SolidColor" => {
                self.styles.end_gradient();
                self.styles.set_color(parse_color(attrs, "color", "alpha"));
            }
            "LinearGradient" => {
                self.styles
                    .begin_gradient(GradientKind::Linear, parse_spread(attrs), 0.0);
            }
            "RadialGradient" => {
                // Animate's focal point, which slides the hot spot along the
                // ramp's own axis. Written as a ratio of the radius, which is
                // exactly what our unit space wants.
                let focal = attr_f64(attrs, "focalPointRatio", 0.0);
                self.styles
                    .begin_gradient(GradientKind::Radial, parse_spread(attrs), focal);
            }
            "GradientEntry" => {
                // **The ratio is the stop's place on the ramp.** It was ignored
                // while gradients were flattened to one colour, because an
                // average does not care where its terms sit. It matters now: a
                // file whose ratios are 0, 0.2 and 1 draws nothing like the
                // same gradient with them evenly spread.
                let ratio = attr_f64(attrs, "ratio", 0.0);
                self.styles
                    .add_gradient_stop(parse_color(attrs, "color", "alpha"), ratio);
            }
            "BitmapFill" => match attrs.get("bitmapPath").and_then(|p| self.bitmaps.get(p)) {
                Some(fill) => {
                    let fill = fill.clone();
                    self.styles.begin_bitmap(fill, attr_bool(attrs, "bitmapIsClipped", false));
                }
                None => {
                    report.note_unsupported("a bitmap fill whose picture is missing, imported as a flat colour");
                    self.styles.set_color(Color::from_rgb8(0x80, 0x80, 0x80));
                }
            },

            "Edge" => {
                // Collected, not drawn. A shape's outlines are spread across
                // its edges and only make sense once all of them are in —
                // `finish_shape` is where they become artwork.
                let Some(data) = attrs.get("edges") else {
                    return;
                };
                let index = |k: &str| attrs.get(k).and_then(|v| v.trim().parse::<u32>().ok());
                match edge::parse_segments(data) {
                    Ok(segments) if !segments.is_empty() => {
                        self.edges.push(edge::EdgeRecord {
                            fill_left: index("fillStyle1"),
                            fill_right: index("fillStyle0"),
                            stroke: index("strokeStyle"),
                            segments,
                        });
                    }
                    Ok(_) => {}
                    Err(e) => report.note_unsupported(format!("edge data ({e})")),
                }
            }
            "DOMSymbolInstance" => {
                let Some(library_name) = attrs.get("libraryItemName") else {
                    return;
                };
                // Animate stores the library path; the symbol's own name is
                // the last segment. The full path is tried first — two folders
                // may each hold a "head", and the bare name would pick
                // whichever was read first.
                let short = library_name.rsplit('/').next().unwrap_or(library_name);
                match symbols.get(library_name).or_else(|| symbols.get(short)) {
                    Some(symbol) => {
                        let mut object = Object::instance_of(ObjectId(ids.take()), *symbol);
                        // **How this instance plays.** Animate keeps it on the
                        // instance, not on the symbol: the same drawing placed
                        // twice can loop in one shot and hold a single pose in
                        // the next, and half a rigged character is placed as a
                        // held pose. Ignoring these two attributes ran every
                        // held pose through its whole timeline and started
                        // every cycle at its first frame — the drawing was
                        // right and the pose was wrong.
                        if let buzz_scene::ObjectKind::Instance(placed) = &mut object.kind {
                            placed.first_frame = attr_u32(attrs, "firstFrame", 0);
                            placed.loop_mode = match attrs.get("loop").map(String::as_str) {
                                Some("single frame") => buzz_scene::LoopMode::SingleFrame,
                                Some("play once") => buzz_scene::LoopMode::PlayOnce,
                                // Animate's default, and what it writes for
                                // everything else.
                                _ => buzz_scene::LoopMode::Loop,
                            };
                        }
                        frame.objects.push(std::sync::Arc::new(object));
                        report.instances += 1;
                        // The next `<Matrix>` places this instance. Nothing
                        // else may claim it — see the `Matrix` arm.
                        self.placing = true;
                    }
                    None => report
                        .note_unsupported(format!("instance of unknown symbol {library_name}")),
                }
            }
            "DOMGroup" => report.groups += 1,
            "DOMBitmapInstance" => {
                let Some(name) = attrs.get("libraryItemName") else { return };
                let Some(fill) = self.bitmaps.get(name) else {
                    report.note_unsupported(format!("bitmap instance unavailable: {name}"));
                    return;
                };
                let rect = buzz_geom::Rect::new(0.0, 0.0, fill.asset.width as f64, fill.asset.height as f64);
                let mut object = Object::shape(ObjectId(ids.take()), ShapeData {
                    path: buzz_geom::Shape::to_path(&rect, 1e-9),
                    fill: Some(FillSpec::image(fill.clone())),
                    stroke: None,
                    blend: buzz_scene::PaintBlend::Normal,
                });
                object.visible = attr_bool(attrs, "isVisible", true);
                frame.objects.push(std::sync::Arc::new(object));
                report.shapes += 1;
                self.placing = true;
            }
            "DOMStaticText" | "DOMDynamicText" | "DOMInputText" => report.note_unsupported("text"),
            "DOMVideoInstance" => report.note_unsupported("video"),
            // `DOMSoundItem` is a library item, read by the sound pass in
            // `build_with_media` before any frame is open — never a stage
            // instance, so there is nothing to place here.
            //
            // **A custom ease curve, drawn by hand in the Motion Editor.**
            // `target` says which property it governs — `all` covers most of
            // them — and its points arrive as the `<Point>` children that
            // follow, collected until `</CustomEase>` closes it (see
            // `end_custom_ease`).
            "CustomEase" => {
                self.reading_custom_ease =
                    Some(attrs.get("target").cloned().unwrap_or_else(|| "all".into()));
                self.custom_ease_points.clear();
            }
            "Point" if self.reading_custom_ease.is_some() => {
                self.custom_ease_points.push(buzz_geom::Point::new(
                    attr_f64(attrs, "x", 0.0),
                    attr_f64(attrs, "y", 0.0),
                ));
            }
            // **A named ease preset, per property.** Distinct from the
            // acceleration slider (`parse_tween`'s `Easing::Strength`, which
            // is written on the `DOMFrame` itself): this is the Motion
            // Editor's dropdown of named curves — `quadInOut` and the rest —
            // applied to one property rather than the whole tween. Mapped
            // onto an equivalent Bézier, the same shape a `<CustomEase>`
            // would carry, since our model has no named-preset vocabulary of
            // its own to match it against.
            "Ease" => {
                if let Some(method) = attrs.get("method")
                    && let Some(points) = bezier_points_for_ease_method(method)
                {
                    let target = attrs.get("target").map(String::as_str).unwrap_or("all");
                    self.set_ease_target(target, std::sync::Arc::from(points));
                }
            }
            // **The transformation point an animator placed.**
            //
            // Animate writes it beside the matrix on anything that can be
            // transformed, and it is not decoration: on a rigged character it
            // is the *joint* — the shoulder an arm swings from. Ignoring it
            // put every joint back at the middle of its artwork, so an
            // imported character had to be re-rigged by hand before it could
            // be posed. It travels in the object's own space, which is where
            // we store it too.
            "transformationPoint" => {
                self.reading_pivot = true;
            }
            // **The instance's colour effect.** Dropping it drew every faded
            // or tinted symbol at full strength: barish1's rain, set to 36%
            // alpha, came out solid white over the whole scene.
            "color" => {
                self.reading_color = true;
            }
            "Color" if self.reading_color => {
                self.reading_color = false;
                if let Some(last) = frame.objects.last_mut()
                    && let buzz_scene::ObjectKind::Instance(placed) =
                        &mut std::sync::Arc::make_mut(last).kind
                {
                    placed.color = parse_color_effect(attrs);
                }
            }
            "Point" if self.reading_pivot => {
                self.reading_pivot = false;
                let x = attrs
                    .get("x")
                    .and_then(|v| v.parse::<f64>().ok())
                    .unwrap_or(0.0);
                let y = attrs
                    .get("y")
                    .and_then(|v| v.parse::<f64>().ok())
                    .unwrap_or(0.0);
                if let Some(last) = frame.objects.last_mut() {
                    std::sync::Arc::make_mut(last).pivot = Some(buzz_geom::Point::new(x, y));
                }
            }
            "Matrix" if self.shape_wants_matrix => {
                // **A grouped shape's own placement.** Animate writes the
                // group's matrix on the `<DOMGroup>` and again, already
                // composed, on each member shape; its edges are in that
                // shape's own space. Ignoring it drew every grouped shape at
                // its raw coordinates: barish1's left eyeball sat at the
                // symbol's corner, clipped by the eye-white mask to a sliver.
                self.shape_wants_matrix = false;
                self.shape_matrix = Some(parse_matrix(attrs));
            }
            "Matrix" => {
                // **A gradient's matrix is its own.** It says where the ramp
                // runs, and it is claimed here before anything else can mistake
                // it for a placement.
                if self.styles.place_gradient(parse_matrix(attrs)) {
                    return;
                }
                // A bitmap fill's matrix lays its picture down; also its own.
                if self.styles.place_bitmap(parse_matrix(attrs)) {
                    return;
                }
                // **Otherwise, only the one that places an instance.**
                // `<Matrix>` appears in several places in XFL, and the others
                // are not placements at all: a bitmap fill carries one to say
                // how its image is laid down. Taken as a placement, a
                // gradient's matrix moved whichever object happened to be last
                // — which is how a hut ends up across the stage from its
                // village. Anything not expecting a matrix ignores this one.
                if !self.placing {
                    return;
                }
                self.placing = false;
                if let Some(last) = frame.objects.last_mut() {
                    let transform = parse_matrix(attrs);
                    std::sync::Arc::make_mut(last).transform = transform;
                }
            }
            // **A frame's volume, if it was shaped.** Animate can fade a
            // sound across its own span with several of these; only the
            // first is read for now, which is the clip's starting level and a
            // fair stand-in for a curve we do not yet model.
            "SoundEnvelopePoint" if !self.envelope_taken => {
                self.envelope_taken = true;
                if let Some(sound) = frame.sound.as_mut() {
                    let level0 = attr_f64(attrs, "level0", 32_768.0);
                    let level1 = attr_f64(attrs, "level1", level0);
                    sound.volume = (((level0 + level1) / 2.0) / 32_768.0).clamp(0.0, 1.0) as f32;
                }
            }
            _ => {}
        }
    }

    fn finish_frame(&mut self, report: &mut ImportReport) {
        // A well-formed file has already closed every `<CustomEase>` by now;
        // this only matters for one that reads truncated, so its points are
        // not simply dropped.
        self.end_custom_ease();
        let Some(frame) = self.current.take() else {
            return;
        };
        self.length = self.length.max(frame.start + frame.duration);
        self.rig_parent_by_frame.push((frame.start, frame.rig_parent));
        self.rig_prop_by_frame.push((frame.start, frame.rig_prop));

        // **Bake, don't model.** A Motion Tween's animation lives in its
        // `AnimationCore` property curves, not in anything our own tween
        // model can run — so it is converted into ordinary keyframes right
        // here, once, rather than carried as a kind of tween the renderer
        // would have to understand forever after.
        if let Some(motion) = &frame.motion_object {
            for keyframe in bake_motion_object(&frame, motion, report) {
                self.keyframes.push(keyframe);
            }
            return;
        }

        self.keyframes.push(buzz_scene::Keyframe {
            start: frame.start,
            objects: std::sync::Arc::new(frame.objects),
            label: frame.label,
            tween: frame.tween,
            ease_curve: frame.ease_curve,
            sound: frame.sound,
        });
    }

    /// Commit the open `<CustomEase>`'s points onto the pending frame, keyed
    /// by the target it named. Fewer than four points is not a curve — `1 +
    /// 3n` from `(0,0)` to `(1,1)` needs at least one full segment — so it is
    /// dropped rather than kept as something that cannot be evaluated.
    fn end_custom_ease(&mut self) {
        let Some(target) = self.reading_custom_ease.take() else {
            return;
        };
        let points = std::mem::take(&mut self.custom_ease_points);
        if points.len() < 4 {
            return;
        }
        self.set_ease_target(&target, std::sync::Arc::from(points));
    }

    /// Write one property's ease curve onto the open frame's [`EasingSet`],
    /// creating it on first use. `position`, `rotation` and `scale` are their
    /// own fields; everything else — `all`, `color`, `filters` — falls back
    /// to `all`, which is the minimum acceptable version this plan asks for.
    fn set_ease_target(&mut self, target: &str, points: std::sync::Arc<[buzz_geom::Point]>) {
        let Some(frame) = self.current.as_mut() else {
            return;
        };
        let set = frame
            .ease_curve
            .get_or_insert_with(buzz_scene::EasingSet::default);
        match target {
            "position" => set.position = Some(points),
            "rotation" => set.rotation = Some(points),
            "scale" => set.scale = Some(points),
            _ => set.all = Some(points),
        }
    }

    /// Attach a parsed `<motionObjectXML>` to the frame it belongs to.
    /// `finish_frame` bakes it once the frame closes.
    fn begin_motion_object(&mut self, motion: MotionObject) {
        if let Some(frame) = self.current.as_mut() {
            frame.motion_object = Some(motion);
        }
    }

    fn flush_layer(&mut self, layers: &mut Vec<PendingLayer>, report: &mut ImportReport) {
        self.finish_frame(report);
        let Some(mut layer) = self.layer.take() else {
            return;
        };
        let mut keyframes = std::mem::take(&mut self.keyframes);
        link_tweened_objects(&mut keyframes);
        layer.frames = buzz_scene::LayerTimeline::from_parts(keyframes, self.length.max(1));
        layers.push(PendingLayer {
            layer,
            parent_index: self.parent_index.take(),
            rig_index: self.rig_index.take(),
            rig_parent: self.rig_parent.take(),
            rig_parent_by_frame: std::mem::take(&mut self.rig_parent_by_frame),
            rig_prop_by_frame: std::mem::take(&mut self.rig_prop_by_frame),
        });
    }
}

/// **Give a classic tween's far keyframe the ids of the objects it tweens
/// from.** Our tweens pair an object with its counterpart on the next
/// keyframe by id (F6 carries ids forward), while this importer gave every
/// object on every keyframe a fresh one, so nothing ever paired: every
/// imported classic tween held its first pose and jumped at the next key
/// (barish1's digging swing, 2Bhatta2's walks). Animate tweens the instances
/// on the two keyframes in order, so they are paired in order here, instance
/// with instance; the chain carries through, so a pose tweened 0 -> 12 -> 26
/// keeps one id throughout. Shape tweens pair by what the drawings are
/// instead (`match_drawings`) and are left alone.
fn link_tweened_objects(keyframes: &mut [buzz_scene::Keyframe]) {
    use buzz_scene::tween::TweenKind;
    for index in 0..keyframes.len().saturating_sub(1) {
        if !matches!(keyframes[index].tween.kind, TweenKind::Classic | TweenKind::Motion) {
            continue;
        }
        let from: Vec<(ObjectId, bool)> = keyframes[index]
            .objects
            .iter()
            .map(|o| (o.id, o.instance().is_some()))
            .collect();
        let next = &mut keyframes[index + 1];
        let mut objects: Vec<std::sync::Arc<Object>> = next.objects.as_ref().clone();
        let mut changed = false;
        for ((id, from_is_instance), object) in from.iter().zip(objects.iter_mut()) {
            if *from_is_instance == object.instance().is_some() && object.id != *id {
                std::sync::Arc::make_mut(object).id = *id;
                changed = true;
            }
        }
        if changed {
            next.objects = std::sync::Arc::new(objects);
        }
    }
}

/// The name a `DOMSymbolInstance` would use to refer to this file's symbol.
///
/// Animate writes the *library path* — `characters/hero` — in both the
/// symbol's own `name` attribute and the file's position under `LIBRARY/`, and
/// the two do not always agree: the file name is escaped for the file system
/// while the attribute is not. Both are offered as keys, along with the last
/// segment of each, since a document written before the symbol was moved into
/// a folder refers to it by the bare name.
fn library_keys(xml: &str, path: &str) -> Vec<String> {
    let from_path = path
        .trim_start_matches("LIBRARY/")
        .trim_start_matches("library/")
        .trim_end_matches(".xml")
        .to_string();

    let mut keys: Vec<String> = Vec::new();
    for full in [library_name_of(xml), Some(from_path)]
        .into_iter()
        .flatten()
    {
        let short = full.rsplit('/').next().unwrap_or(&full).to_string();
        for key in [full.clone(), short] {
            if !key.is_empty() && !keys.contains(&key) {
                keys.push(key);
            }
        }
    }
    keys
}

/// The `name` attribute of the file's `DOMSymbolItem`, without reading the
/// timeline underneath it.
fn library_name_of(xml: &str) -> Option<String> {
    let mut reader = quick_xml::Reader::from_str(xml);
    reader.config_mut().trim_text(true);
    loop {
        match reader.read_event() {
            Ok(Event::Start(e)) | Ok(Event::Empty(e)) => {
                if e.name().as_ref() == b"DOMSymbolItem" {
                    return attributes(&e).get("name").cloned();
                }
            }
            Ok(Event::Eof) | Err(_) => return None,
            _ => {}
        }
    }
}

/// Parse one `LIBRARY/*.xml` symbol definition.
///
/// The id is handed in rather than taken here: every symbol's id is allocated
/// before any timeline is read, so a symbol can hold an instance of one whose
/// file comes later in the archive.
fn parse_symbol(
    xml: &str,
    path: &str,
    id: SymbolId,
    symbols: &HashMap<String, SymbolId>,
    bitmaps: &BitmapMap,
    sounds: &SoundMap,
    ids: &mut IdSource,
    report: &mut ImportReport,
) -> Result<Symbol, ImportError> {
    let mut reader = quick_xml::Reader::from_str(xml);
    reader.config_mut().trim_text(true);

    // The library path becomes the folder, so an organised Animate library
    // arrives organised rather than flattened.
    let trimmed = path
        .trim_start_matches("LIBRARY/")
        .trim_start_matches("library/")
        .trim_end_matches(".xml");
    let (folder, file_name) = match trimmed.rsplit_once('/') {
        Some((f, n)) => (Some(f.to_string()), n.to_string()),
        None => (None, trimmed.to_string()),
    };

    let mut symbol = Symbol::new(id, file_name, SymbolKind::Graphic);
    symbol.folder = folder;

    let mut context = FrameContext {
        bitmaps: bitmaps.clone(),
        sounds: sounds.clone(),
        ..Default::default()
    };
    let mut layers: Vec<PendingLayer> = Vec::new();

    loop {
        match reader.read_event() {
            Ok(Event::Eof) => break,
            Ok(Event::Start(e)) | Ok(Event::Empty(e)) => {
                let name = String::from_utf8_lossy(e.name().as_ref()).to_string();
                let attrs = attributes(&e);
                match name.as_str() {
                    "DOMSymbolItem" => {
                        if let Some(n) = attrs.get("name") {
                            let short = n.rsplit('/').next().unwrap_or(n);
                            symbol.name = short.to_string();
                        }
                        symbol.kind = match attrs.get("symbolType").map(String::as_str) {
                            Some("button") => SymbolKind::Button,
                            Some("graphic") => SymbolKind::Graphic,
                            // Animate's default when unspecified.
                            _ => SymbolKind::MovieClip,
                        };
                    }
                    "DOMLayer" => {
                        context.flush_layer(&mut layers, report);
                        context.begin_layer(&attrs, ids);
                    }
                    "DOMFrame" => context.begin_frame(&attrs, report),
                    "motionObjectXML" => {
                        let start = reader.buffer_position() as usize;
                        if let Some(rel_end) =
                            xml.get(start..).and_then(|rest| rest.find("</motionObjectXML>"))
                        {
                            let motion = parse_motion_object(&xml[start..start + rel_end], report);
                            context.begin_motion_object(motion);
                        }
                    }
                    _ => context.element(&name, &attrs, symbols, ids, report),
                }
            }
            Ok(Event::End(e)) if e.name().as_ref() == b"DOMShape" => {
                context.finish_shape(ids, report);
            }
            Ok(Event::End(e)) if e.name().as_ref() == b"CustomEase" => {
                context.end_custom_ease();
            }
            Err(e) => {
                // **Animate itself writes broken symbol files.** Real
                // documents on this machine contain `<DOMShape` with no
                // closing bracket, immediately followed by `</DOMShape>` —
                // the puppet-warp shapes, saved damaged. Refusing the file
                // threw away every other frame in the symbol along with the
                // bad one, and the symbol then vanished from every scene that
                // used it. What reads is kept, and the truncation is named.
                report.note_unsupported(format!(
                    "damaged XML in the symbol {}, read as far as it goes ({e})",
                    symbol.name
                ));
                break;
            }
            _ => {}
        }
    }
    context.flush_layer(&mut layers, report);

    // A symbol's timeline has layer folders and masks just as the document's
    // does, and the same top-first order.
    resolve_layer_parents(&mut layers, report);

    if layers.is_empty() {
        layers.push(PendingLayer {
            layer: Layer::normal(LayerId(ids.take()), "Layer_1"),
            parent_index: None,
            rig_index: None,
            rig_parent: None,
            rig_parent_by_frame: Vec::new(),
            rig_prop_by_frame: Vec::new(),
        });
    }
    for (index, pending) in layers.into_iter().enumerate() {
        symbol.layers.insert(index, pending.layer);
    }
    Ok(symbol)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Zip a set of named entries into `.fla` bytes.
    fn zip_up(entries: &[(&str, &[u8])]) -> Vec<u8> {
        let mut buffer = std::io::Cursor::new(Vec::new());
        {
            let mut zip = zip::ZipWriter::new(&mut buffer);
            for (name, bytes) in entries {
                zip.start_file::<_, ()>(*name, Default::default()).unwrap();
                use std::io::Write as _;
                zip.write_all(bytes).unwrap();
            }
            zip.finish().unwrap();
        }
        buffer.into_inner()
    }

    /// **A `.fla` whose contents sit inside a folder still opens.**
    ///
    /// Animate writes `DOMDocument.xml` at the top of the archive, but a `.fla`
    /// that has been repacked — zipped from an unzipped folder, which is how
    /// one gets moved between machines — carries everything one level down.
    /// Looking only at the root found nothing and reported the file as not
    /// being an Animate document, which is both wrong and a dead end.
    #[test]
    fn a_repacked_fla_with_everything_in_a_folder_still_opens() {
        let bytes = zip_up(&[
            ("MyFilm/DOMDocument.xml", MINIMAL_DOCUMENT.as_bytes()),
            ("MyFilm/LIBRARY/hero.xml", HERO_SYMBOL.as_bytes()),
        ]);

        let (scene, report) = import_fla_bytes(&bytes).expect("it should open");
        assert_eq!(scene.stage().size, buzz_geom::Size::new(640.0, 480.0));
        assert_eq!(
            scene.library().len(),
            1,
            "the library beside the document should have been found too: {}",
            report.summary()
        );
    }

    /// A byte order mark is stripped rather than handed to the parser, which
    /// treats it as content and then fails on a document that begins with a
    /// character no parser expects.
    #[test]
    fn a_utf8_byte_order_mark_does_not_stop_the_parse() {
        let mut document = vec![0xEF, 0xBB, 0xBF];
        document.extend_from_slice(MINIMAL_DOCUMENT.as_bytes());

        let bytes = zip_up(&[
            ("DOMDocument.xml", &document),
            ("LIBRARY/hero.xml", HERO_SYMBOL.as_bytes()),
        ]);

        let (scene, _) = import_fla_bytes(&bytes).expect("a BOM is not a failure");
        assert_eq!(scene.stage().size, buzz_geom::Size::new(640.0, 480.0));
    }

    /// **The message names what was in the archive.** Told only that a file
    /// does not look like an Animate document there is nothing to do but
    /// believe it; told what the archive actually holds, the answer is usually
    /// obvious.
    #[test]
    fn a_missing_document_says_what_the_archive_held_instead() {
        let bytes = zip_up(&[
            ("readme.txt", b"nothing to see"),
            ("art/drawing.png", b"\x89PNG"),
        ]);

        let Err(e) = import_fla_bytes(&bytes) else {
            panic!("an archive with no document should not open")
        };
        let message = e.to_string();
        assert!(message.contains("readme.txt"), "{message}");
        assert!(message.contains("drawing.png"), "{message}");
    }

    /// One damaged symbol costs that symbol, not the film. The document still
    /// opens and everything else in it survives.
    #[test]
    fn a_library_entry_that_will_not_decode_is_skipped_not_fatal() {
        // A lone continuation byte: not valid UTF-8 anywhere.
        let broken: &[u8] = &[0x3C, 0x3F, 0x78, 0x6D, 0x6C, 0xFF, 0xFE_u8.wrapping_add(1)];
        let bytes = zip_up(&[
            ("DOMDocument.xml", MINIMAL_DOCUMENT.as_bytes()),
            ("LIBRARY/hero.xml", HERO_SYMBOL.as_bytes()),
            ("LIBRARY/broken.xml", broken),
        ]);

        let (scene, _) = import_fla_bytes(&bytes).expect("one bad symbol is not fatal");
        assert_eq!(
            scene.library().len(),
            1,
            "the good symbol should still be there"
        );
    }

    /// A document that is present and cannot be decoded says *that*, rather
    /// than claiming the file is not an Animate document — which sent the user
    /// looking for a problem with the wrong file.
    #[test]
    fn an_undecodable_document_is_not_reported_as_missing() {
        let bytes = zip_up(&[("DOMDocument.xml", &[0xF8, 0xA1, 0xA1, 0xA1])]);

        let Err(e) = import_fla_bytes(&bytes) else {
            panic!("it should not have opened")
        };
        assert!(
            matches!(e, ImportError::UnreadableDocument(_)),
            "expected an unreadable document, got: {e}"
        );
    }

    const MINIMAL_DOCUMENT: &str = r##"<?xml version="1.0" encoding="UTF-8"?>
<DOMDocument xmlns="http://ns.adobe.com/xfl/2008/" width="640" height="480"
             frameRate="30" backgroundColor="#336699">
  <timelines>
    <DOMTimeline name="Scene 1">
      <layers>
        <DOMLayer name="Foreground" visible="false" locked="true">
          <frames>
            <DOMFrame index="0" duration="5" tweenType="motion" acceleration="50">
              <elements>
                <DOMSymbolInstance libraryItemName="hero">
                  <matrix><Matrix a="2" d="2" tx="100" ty="50"/></matrix>
                </DOMSymbolInstance>
              </elements>
            </DOMFrame>
            <DOMFrame index="5" duration="5"/>
          </frames>
        </DOMLayer>
        <DOMLayer name="Background" color="#4FFF4F" visible="true" locked="false">
          <frames>
            <DOMFrame index="0" duration="10" name="start">
              <elements>
                <DOMShape>
                  <edges>
                    <Edge fillStyle0="1" edges="!0 0|2000 0|2000 2000|0 2000|0 0"/>
                  </edges>
                </DOMShape>
              </elements>
            </DOMFrame>
          </frames>
        </DOMLayer>
      </layers>
    </DOMTimeline>
  </timelines>
</DOMDocument>"##;

    const HERO_SYMBOL: &str = r##"<?xml version="1.0" encoding="UTF-8"?>
<DOMSymbolItem xmlns="http://ns.adobe.com/xfl/2008/" name="hero" symbolType="graphic">
  <timeline>
    <DOMTimeline name="hero">
      <layers>
        <DOMLayer name="body">
          <frames>
            <DOMFrame index="0" duration="1">
              <elements>
                <DOMShape>
                  <edges><Edge fillStyle0="1" edges="!0 0|400 0|400 400|0 400|0 0"/></edges>
                </DOMShape>
              </elements>
            </DOMFrame>
          </frames>
        </DOMLayer>
      </layers>
    </DOMTimeline>
  </timeline>
</DOMSymbolItem>"##;

    fn import_sample() -> (Scene, ImportReport) {
        build(
            MINIMAL_DOCUMENT,
            &[("LIBRARY/hero.xml".to_string(), HERO_SYMBOL.to_string())],
        )
        .expect("the sample should import")
    }

    #[test]
    fn stage_properties_come_across() {
        let (scene, _) = import_sample();
        assert_eq!(scene.stage().size, buzz_geom::Size::new(640.0, 480.0));
        assert_eq!(scene.stage().frame_rate, 30.0);
        assert_eq!(
            scene.stage().background.to_rgba8().to_u8_array(),
            [0x33, 0x66, 0x99, 255]
        );
    }

    #[test]
    fn layers_arrive_with_their_properties() {
        let (scene, report) = import_sample();
        assert_eq!(report.layers, 2);
        assert_eq!(scene.layers().len(), 2);

        let foreground = scene.layers().iter().next().unwrap();
        assert_eq!(foreground.name, "Foreground");
        assert!(!foreground.visible, "hidden layers stay hidden");
        assert!(foreground.locked);

        let background = scene.layers().iter().last().unwrap();
        assert_eq!(background.name, "Background");
        assert!(background.visible);
    }

    /// **Animate writes layers top-first**, and so do we — the order in the
    /// file is the order down the timeline.
    ///
    /// This was read backwards for five phases, and the list was reversed on
    /// the way in. Every document imported inside out: skies over artwork,
    /// backgrounds over the characters standing on them, and every mask
    /// underneath the layers it was supposed to clip. The camera layer is the
    /// proof — Animate pins it to the top of the timeline and writes it
    /// first.
    #[test]
    fn layer_order_matches_the_file() {
        let (scene, _) = import_sample();
        let names: Vec<&str> = scene.layers().iter().map(|l| l.name.as_str()).collect();
        assert_eq!(
            names,
            vec!["Foreground", "Background"],
            "the first layer in the file is the front one"
        );
    }

    #[test]
    fn shapes_are_parsed_from_edge_data() {
        let (scene, report) = import_sample();
        assert!(report.shapes >= 1);

        let background = scene.layers().iter().last().unwrap();
        let objects = background.objects_at(0);
        assert_eq!(objects.len(), 1);

        // 2000 twips is 100 pixels.
        let bounds = objects[0].bounds();
        assert!((bounds.width() - 100.0).abs() < 0.01, "got {bounds:?}");
    }

    #[test]
    fn frame_spans_and_labels_survive() {
        let (scene, report) = import_sample();
        assert!(report.keyframes >= 3);

        let background = scene.layers().iter().last().unwrap();
        assert_eq!(background.length(), 10, "duration 10 means ten frames");
        assert_eq!(
            background.frames.keyframe_at(0).unwrap().label.as_deref(),
            Some("start")
        );
    }

    #[test]
    fn tweens_are_imported_with_their_easing() {
        let (scene, report) = import_sample();
        assert_eq!(report.tweens, 1);

        let foreground = scene.layers().iter().next().unwrap();
        let tween = foreground.frames.tween_at(0);
        assert_eq!(tween.kind, buzz_scene::TweenKind::Classic);
        assert!(matches!(tween.easing, buzz_scene::Easing::Strength(a) if (a - 50.0).abs() < 1e-9));
    }

    /// Hand-computed: x moves 0 → 100, rotating 0° → 90° with a flip
    /// (`Scale_X` 100 → -100) over 11 frames (0..=10). Checked at 0, 5 and 10
    /// against matrices worked out by hand from `bake_motion_object`'s own
    /// documented convention.
    #[test]
    fn a_motion_tween_bakes_matrices_matching_hand_computed_ones() {
        const DOCUMENT: &str = r##"<?xml version="1.0" encoding="UTF-8"?>
<DOMDocument xmlns="http://ns.adobe.com/xfl/2008/" width="200" height="200">
  <timelines><DOMTimeline name="Scene 1"><layers>
    <DOMLayer name="Art"><frames>
      <DOMFrame index="0" duration="11" tweenType="motion object" isMotionObject="true">
        <motionObjectXML><AnimationCore TimeScale="24000" Version="1" duration="11000">
          <TimeMap strength="0" type="Quadratic"/>
          <metadata><Settings xformPtXOffsetPct="0.5" xformPtYOffsetPct="0.5"/></metadata>
          <PropertyContainer id="root">
            <PropertyContainer id="Basic_Motion">
              <Property id="Motion_X"><Keyframe anchor="0,0" next="0,0" previous="0,0" timevalue="0"/><Keyframe anchor="0,100" next="0,100" previous="0,100" timevalue="10000"/></Property>
              <Property id="Motion_Y"><Keyframe anchor="0,0" next="0,0" previous="0,0" timevalue="0"/></Property>
              <Property id="Rotation_Z"><Keyframe anchor="0,0" next="0,0" previous="0,0" timevalue="0"/><Keyframe anchor="0,90" next="0,90" previous="0,90" timevalue="10000"/></Property>
              <Property id="Depth"><Keyframe anchor="0,0" next="0,0" previous="0,0" timevalue="0"/></Property>
            </PropertyContainer>
            <PropertyContainer id="Transformation">
              <Property id="Skew_X"><Keyframe anchor="0,0" next="0,0" previous="0,0" timevalue="0"/></Property>
              <Property id="Skew_Y"><Keyframe anchor="0,0" next="0,0" previous="0,0" timevalue="0"/></Property>
              <Property id="Scale_X"><Keyframe anchor="0,100" next="0,100" previous="0,100" timevalue="0"/><Keyframe anchor="0,-100" next="0,-100" previous="0,-100" timevalue="10000"/></Property>
              <Property id="Scale_Y"><Keyframe anchor="0,100" next="0,100" previous="0,100" timevalue="0"/></Property>
            </PropertyContainer>
            <PropertyContainer id="Colors"/>
            <PropertyContainer id="Filters"/>
          </PropertyContainer>
        </AnimationCore></motionObjectXML>
        <elements>
          <DOMSymbolInstance libraryItemName="part">
            <matrix><Matrix tx="0" ty="0"/></matrix>
          </DOMSymbolInstance>
        </elements>
      </DOMFrame>
    </frames></DOMLayer>
  </layers></DOMTimeline></timelines>
</DOMDocument>"##;

        let (scene, report) = build(
            DOCUMENT,
            &[("LIBRARY/part.xml".to_string(), RIG_PART.to_string())],
        )
        .unwrap();

        let layer = scene.layers().iter().next().unwrap();
        let at = |frame: u32| layer.objects_at(frame)[0].transform.as_coeffs();

        let c0 = at(0);
        for (k, want) in [1.0, 0.0, 0.0, 1.0, 0.0, 0.0].into_iter().enumerate() {
            assert!((c0[k] - want).abs() < 1e-6, "frame 0 coeff {k}: {c0:?}");
        }

        let c5 = at(5);
        let want5 = [
            0.0,
            0.0,
            -std::f64::consts::FRAC_1_SQRT_2,
            std::f64::consts::FRAC_1_SQRT_2,
            50.0,
            0.0,
        ];
        for k in 0..6 {
            assert!((c5[k] - want5[k]).abs() < 1e-6, "frame 5 coeff {k}: {c5:?} vs {want5:?}");
        }

        let c10 = at(10);
        for (k, want) in [0.0, -1.0, -1.0, 0.0, 100.0, 0.0].into_iter().enumerate() {
            assert!((c10[k] - want).abs() < 1e-6, "frame 10 coeff {k}: {c10:?}");
        }

        assert!(report.tweens == 0, "a baked motion tween carries no classic/shape tween count");
    }

    /// bhatta2/sustipur1's real Motion Tweens hold a static, unanimated pose
    /// on spans thousands of frames long — an object converted to a Motion
    /// Tween and never actually touched. `bake_motion_object` must not write
    /// one keyframe per frame for that: a single held one is exact and
    /// leaves the layer's own length untouched.
    #[test]
    fn a_long_unanimated_motion_tween_span_bakes_to_one_held_keyframe() {
        const DOCUMENT: &str = r##"<?xml version="1.0" encoding="UTF-8"?>
<DOMDocument xmlns="http://ns.adobe.com/xfl/2008/" width="200" height="200">
  <timelines><DOMTimeline name="Scene 1"><layers>
    <DOMLayer name="Art"><frames>
      <DOMFrame index="0" duration="4641" tweenType="motion object" isMotionObject="true">
        <motionObjectXML><AnimationCore TimeScale="24000" Version="1" duration="4641000">
          <PropertyContainer id="root">
            <PropertyContainer id="Basic_Motion">
              <Property id="Motion_X"><Keyframe anchor="0,0" next="0,0" previous="0,0" timevalue="0"/></Property>
              <Property id="Motion_Y"><Keyframe anchor="0,0" next="0,0" previous="0,0" timevalue="0"/></Property>
              <Property id="Rotation_Z"><Keyframe anchor="0,-8.94977" next="0,-8.94977" previous="0,-8.94977" timevalue="0"/></Property>
            </PropertyContainer>
            <PropertyContainer id="Transformation">
              <Property id="Scale_X"><Keyframe anchor="0,-168.153" next="0,-168.153" previous="0,-168.153" timevalue="0"/></Property>
              <Property id="Scale_Y"><Keyframe anchor="0,168.153" next="0,168.153" previous="0,168.153" timevalue="0"/></Property>
            </PropertyContainer>
          </PropertyContainer>
        </AnimationCore></motionObjectXML>
        <elements>
          <DOMSymbolInstance libraryItemName="part">
            <matrix><Matrix a="-1.66104125976562" b="0.261581420898438" c="0.261581420898438" d="1.66104125976562" tx="388.05" ty="209"/></matrix>
          </DOMSymbolInstance>
        </elements>
      </DOMFrame>
    </frames></DOMLayer>
  </layers></DOMTimeline></timelines>
</DOMDocument>"##;

        let (scene, _) = build(
            DOCUMENT,
            &[("LIBRARY/part.xml".to_string(), RIG_PART.to_string())],
        )
        .unwrap();

        let layer = scene.layers().iter().next().unwrap();
        assert_eq!(
            layer.frames.keyframes().len(),
            1,
            "an unanimated span should bake to one held keyframe, not one per frame"
        );

        // The frame-0 identity check this plan asks for: at rest, the baked
        // matrix must equal the instance's own written `<Matrix>` exactly —
        // the real numbers from sustipur1's `fatherFront copy 3`
        // (`EyebrowRight`), which is where `bake_motion_object`'s convention
        // was verified against a real file.
        let c = layer.objects_at(0)[0].transform.as_coeffs();
        let want = [
            -1.66104125976562,
            0.261581420898438,
            0.261581420898438,
            1.66104125976562,
            388.05,
            209.0,
        ];
        // The file's own `Scale_X`/`Scale_Y`/`Rotation_Z` are themselves
        // written to 3 decimal places (a UI rounding, not a measurement),
        // so reconstructing from them cannot land closer than that lets it —
        // 1e-4 here is tight for the coefficients themselves, not a loose
        // pass.
        for k in 0..6 {
            assert!(
                (c[k] - want[k]).abs() < 1e-4,
                "coefficient {k}: baked {c:?}, instance wrote {want:?}"
            );
        }
    }

    /// The sample from sustipur1's `pradhanOldman__assets__/Symbol 1.xml`: a
    /// `<CustomEase target="all">` with a bare `<Point/>` for its start.
    #[test]
    fn a_custom_ease_curve_produces_that_easing_on_the_keyframe() {
        const DOCUMENT: &str = r##"<?xml version="1.0" encoding="UTF-8"?>
<DOMDocument xmlns="http://ns.adobe.com/xfl/2008/" width="200" height="200">
  <timelines><DOMTimeline name="Scene 1"><layers>
    <DOMLayer name="Art"><frames>
      <DOMFrame index="0" duration="10" tweenType="motion" hasCustomEase="true">
        <tweens>
          <CustomEase target="all">
            <Point/>
            <Point x="0.462061801615505" y="0.0309006166962531"/>
            <Point x="0.731165356194802" y="0.505253119561007"/>
            <Point x="1" y="1"/>
          </CustomEase>
        </tweens>
        <elements>
          <DOMShape><edges><Edge fillStyle1="1" edges="!0 0|400 0|400 400|0 400|0 0"/></edges></DOMShape>
        </elements>
      </DOMFrame>
      <DOMFrame index="10" duration="1"/>
    </frames></DOMLayer>
  </layers></DOMTimeline></timelines>
</DOMDocument>"##;

        let (scene, _) = build(DOCUMENT, &[]).unwrap();
        let layer = scene.layers().iter().next().unwrap();
        let curve = layer
            .frames
            .keyframe_at(0)
            .unwrap()
            .ease_curve
            .clone()
            .expect("a custom ease should have been captured");
        let all = curve.all.expect("target=\"all\" should be set");
        assert_eq!(all.len(), 4);
        assert!(
            all[0].x.abs() < 1e-9 && all[0].y.abs() < 1e-9,
            "a bare <Point/> means (0,0), got {:?}",
            all[0]
        );
        assert!((all[1].x - 0.462061801615505).abs() < 1e-9);
        assert!((all[3].x - 1.0).abs() < 1e-9 && (all[3].y - 1.0).abs() < 1e-9);
    }

    /// bhatta2's `fatherBackPose.xml`: a preset named ease rather than a
    /// hand-drawn curve.
    #[test]
    fn a_named_ease_method_produces_an_equivalent_bezier_on_the_keyframe() {
        const DOCUMENT: &str = r##"<?xml version="1.0" encoding="UTF-8"?>
<DOMDocument xmlns="http://ns.adobe.com/xfl/2008/" width="200" height="200">
  <timelines><DOMTimeline name="Scene 1"><layers>
    <DOMLayer name="Art"><frames>
      <DOMFrame index="0" duration="10" tweenType="motion" easeMethodName="quadInOut">
        <tweens><Ease target="all" method="quadInOut"/></tweens>
        <elements>
          <DOMShape><edges><Edge fillStyle1="1" edges="!0 0|400 0|400 400|0 400|0 0"/></edges></DOMShape>
        </elements>
      </DOMFrame>
      <DOMFrame index="10" duration="1"/>
    </frames></DOMLayer>
  </layers></DOMTimeline></timelines>
</DOMDocument>"##;

        let (scene, _) = build(DOCUMENT, &[]).unwrap();
        let layer = scene.layers().iter().next().unwrap();
        let curve = layer
            .frames
            .keyframe_at(0)
            .unwrap()
            .ease_curve
            .clone()
            .expect("a named ease method should have been mapped onto a curve");
        let all = curve.all.expect("target=\"all\" should be set");
        assert_eq!(all.len(), 4);
        assert!(all[0].x.abs() < 1e-9 && all[0].y.abs() < 1e-9);
        assert!((all[3].x - 1.0).abs() < 1e-9 && (all[3].y - 1.0).abs() < 1e-9);
        // Not the identity line: it is a real ease, not a pass-through.
        assert!(all[1].y.abs() > 1e-6 || all[2].y.abs() > 1e-6);
    }

    #[test]
    fn symbols_are_imported_into_the_library() {
        let (scene, report) = import_sample();
        assert_eq!(report.symbols, 1);

        let hero = scene.library().find_by_name("hero").expect("hero symbol");
        assert_eq!(hero.kind, SymbolKind::Graphic);
        assert!(hero.bounds().is_some(), "the symbol should contain artwork");
    }

    #[test]
    fn instances_are_linked_to_their_symbol() {
        let (scene, report) = import_sample();
        assert_eq!(report.instances, 1);

        let hero = scene.library().find_by_name("hero").unwrap().id;
        let usage = scene.symbol_usage();
        assert_eq!(usage.get(&hero), Some(&1));
    }

    #[test]
    fn an_instance_matrix_is_applied() {
        let (scene, _) = import_sample();
        let foreground = scene.layers().iter().next().unwrap();
        let instance = &foreground.objects_at(0)[0];
        let c = instance.transform.as_coeffs();
        assert!((c[0] - 2.0).abs() < 1e-9, "scale x was {}", c[0]);
        assert!((c[4] - 100.0).abs() < 1e-9, "translate x was {}", c[4]);
    }

    /// The library folder structure must survive, not flatten.
    #[test]
    fn library_folders_are_preserved() {
        let (scene, _) = build(
            MINIMAL_DOCUMENT,
            &[(
                "LIBRARY/characters/heroes/hero.xml".to_string(),
                HERO_SYMBOL.to_string(),
            )],
        )
        .unwrap();

        let hero = scene.library().find_by_name("hero").unwrap();
        assert_eq!(hero.folder.as_deref(), Some("characters/heroes"));
        assert_eq!(hero.path(), "characters/heroes/hero");
        assert!(
            scene.library().folders().any(|f| f == "characters"),
            "parent folders should exist too"
        );
    }

    /// A symbol built out of other symbols — which is what a rigged character
    /// is — must find them whatever order the library is read in.
    ///
    /// This is the defect that made a real Animate document import as a page
    /// of "instance of unknown symbol": each symbol was parsed against only
    /// the symbols parsed before it, so a torso holding an arm found nothing
    /// unless the arm's file happened to come first. Here the container is
    /// read *first*, and both its parts are referenced by their library path.
    #[test]
    fn a_symbol_finds_the_symbols_inside_it_whatever_the_order() {
        const CHARACTER: &str = r##"<?xml version="1.0" encoding="UTF-8"?>
<DOMSymbolItem xmlns="http://ns.adobe.com/xfl/2008/" name="parts/character" symbolType="movie clip">
  <timeline>
    <DOMTimeline name="character">
      <layers>
        <DOMLayer name="Layer 1">
          <frames>
            <DOMFrame index="0" duration="1">
              <elements>
                <DOMSymbolInstance libraryItemName="parts/torso"/>
                <DOMSymbolInstance libraryItemName="parts/arm"/>
                <DOMSymbolInstance libraryItemName="hero"/>
              </elements>
            </DOMFrame>
          </frames>
        </DOMLayer>
      </layers>
    </DOMTimeline>
  </timeline>
</DOMSymbolItem>"##;

        let part = |name: &str| {
            format!(
                r##"<DOMSymbolItem xmlns="http://ns.adobe.com/xfl/2008/" name="parts/{name}"
                     symbolType="graphic"><timeline><DOMTimeline name="{name}"><layers>
                     <DOMLayer name="Layer 1"><frames><DOMFrame index="0" duration="1"/>
                     </frames></DOMLayer></layers></DOMTimeline></timeline></DOMSymbolItem>"##
            )
        };

        let (scene, report) = build(
            MINIMAL_DOCUMENT,
            &[
                (
                    "LIBRARY/parts/character.xml".to_string(),
                    CHARACTER.to_string(),
                ),
                ("LIBRARY/parts/torso.xml".to_string(), part("torso")),
                ("LIBRARY/parts/arm.xml".to_string(), part("arm")),
                ("LIBRARY/hero.xml".to_string(), HERO_SYMBOL.to_string()),
            ],
        )
        .unwrap();

        assert!(
            !report
                .unsupported
                .iter()
                .any(|u| u.contains("unknown symbol")),
            "every nested instance should resolve: {:?}",
            report.unsupported
        );

        let character = scene.library().find_by_name("character").unwrap();
        let inside = character.layers.iter().next().unwrap().objects_at(0);
        assert_eq!(inside.len(), 3, "all three instances should be kept");

        // And each points at the right symbol, not merely at *a* symbol.
        let torso = scene.library().find_by_name("torso").unwrap().id;
        assert!(
            inside
                .iter()
                .any(|o| o.instance().map(|i| i.symbol) == Some(torso)),
            "the torso instance should reference the torso symbol"
        );
    }

    /// **A fill's outline is spread across several edges**, and each edge is a
    /// fragment rather than a shape.
    ///
    /// This is how Animate really writes artwork: a soup of two-point pieces,
    /// each saying which fill is on its left and which on its right. Read as
    /// one closed outline per `<Edge>`, a bush arrives as several hundred
    /// slivers — which is what a real document looked like before the pieces
    /// were reassembled.
    #[test]
    fn a_fill_spread_across_edges_is_reassembled_into_one_outline() {
        // A 100x100 square, written as four separate one-segment edges, in a
        // deliberately unhelpful order and with one of them the wrong way
        // round (its fill on the right instead of the left).
        const SOUP: &str = r##"<?xml version="1.0" encoding="UTF-8"?>
<DOMDocument xmlns="http://ns.adobe.com/xfl/2008/" width="550" height="400">
  <timelines>
    <DOMTimeline name="Scene 1">
      <layers>
        <DOMLayer name="Art">
          <frames>
            <DOMFrame index="0" duration="1">
              <elements>
                <DOMShape>
                  <fills>
                    <FillStyle index="1"><SolidColor color="#3366CC"/></FillStyle>
                  </fills>
                  <edges>
                    <Edge fillStyle1="1" edges="!2000 0|2000 2000"/>
                    <Edge fillStyle0="1" edges="!0 0|2000 0"/>
                    <Edge fillStyle1="1" edges="!0 2000|0 0"/>
                    <Edge fillStyle1="1" edges="!2000 2000|0 2000"/>
                  </edges>
                </DOMShape>
              </elements>
            </DOMFrame>
          </frames>
        </DOMLayer>
      </layers>
    </DOMTimeline>
  </timelines>
</DOMDocument>"##;

        let (scene, _) = build(SOUP, &[]).unwrap();
        let objects = scene.layers().iter().next().unwrap().objects_at(0);
        assert_eq!(
            objects.len(),
            1,
            "one fill, one object \u{2014} not one per edge: {:?}",
            objects.len()
        );

        // 2000 twips is 100 pixels, and the square is closed and whole.
        let bounds = objects[0].bounds();
        assert!(
            (bounds.width() - 100.0).abs() < 0.01 && (bounds.height() - 100.0).abs() < 0.01,
            "the four fragments should close into the whole square, got {bounds:?}"
        );
    }

    /// A rigged character's parts are stored *relative* to the part they hang
    /// off, and have to be made absolute on the way in.
    #[test]
    fn a_rigged_layer_follows_its_parent_and_keeps_its_place() {
        const RIG: &str = r##"<?xml version="1.0" encoding="UTF-8"?>
<DOMSymbolItem xmlns="http://ns.adobe.com/xfl/2008/" name="figure" symbolType="graphic">
  <timeline>
    <DOMTimeline name="figure">
      <layers>
        <DOMLayer name="head" layerRiggingIndex="9">
          <frames>
            <DOMFrame index="0" duration="1" parentLayerIndex="5">
              <elements>
                <DOMSymbolInstance libraryItemName="part">
                  <matrix><Matrix tx="10" ty="-20"/></matrix>
                </DOMSymbolInstance>
              </elements>
            </DOMFrame>
          </frames>
        </DOMLayer>
        <DOMLayer name="body" layerRiggingIndex="5">
          <frames>
            <DOMFrame index="0" duration="1">
              <elements>
                <DOMSymbolInstance libraryItemName="part">
                  <matrix><Matrix tx="300" ty="400"/></matrix>
                </DOMSymbolInstance>
              </elements>
            </DOMFrame>
          </frames>
        </DOMLayer>
      </layers>
    </DOMTimeline>
  </timeline>
</DOMSymbolItem>"##;

        const PART: &str = r##"<DOMSymbolItem xmlns="http://ns.adobe.com/xfl/2008/" name="part"
             symbolType="graphic"><timeline><DOMTimeline name="part"><layers>
             <DOMLayer name="Layer_1"><frames><DOMFrame index="0" duration="1"/>
             </frames></DOMLayer></layers></DOMTimeline></timeline></DOMSymbolItem>"##;

        let (scene, _) = build(
            MINIMAL_DOCUMENT,
            &[
                ("LIBRARY/figure.xml".to_string(), RIG.to_string()),
                ("LIBRARY/part.xml".to_string(), PART.to_string()),
            ],
        )
        .unwrap();

        let figure = scene.library().find_by_name("figure").unwrap();
        let head = figure.layers.iter().find(|l| l.name == "head").unwrap();
        let body = figure.layers.iter().find(|l| l.name == "body").unwrap();

        // **The link is kept.** What is baked into the artwork is the parent's
        // *rest* pose, which is exactly the part layer parenting does not
        // propagate — so the head arrives where Animate drew it and still
        // follows the body. Spending the link instead produced the right
        // picture and no rig: moving the body left the head behind.
        assert_eq!(head.follows, Some(body.id), "the rig is kept, not spent");
        assert_eq!(body.follows, None, "the body hangs off nothing");

        // Relative (10, -20) on a body at (300, 400) is (310, 380) absolute.
        let placed = head.objects_at(0)[0].transform.as_coeffs();
        assert!(
            (placed[4] - 310.0).abs() < 1e-9 && (placed[5] - 380.0).abs() < 1e-9,
            "the parent's rest pose should be baked in, got {:?}",
            (placed[4], placed[5])
        );
    }

    /// **An imported rig draws where Animate draws it, on every frame.**
    ///
    /// The one that matters: a parent that *moves*. Baking the parent's rest
    /// pose into the child and keeping the link has to reproduce Animate's
    /// `parent_world(f) · child_relative(f)` once our own layer parenting has
    /// added the parent's motion back on top — at two levels down, which is
    /// where the arithmetic was previously assumed to come apart.
    #[test]
    fn an_imported_rig_draws_where_animate_draws_it_on_every_frame() {
        const RIG: &str = r##"<?xml version="1.0" encoding="UTF-8"?>
<DOMSymbolItem xmlns="http://ns.adobe.com/xfl/2008/" name="figure" symbolType="graphic">
  <timeline>
    <DOMTimeline name="figure">
      <layers>
        <DOMLayer name="hand" layerRiggingIndex="7">
          <frames>
            <DOMFrame index="0" duration="11" parentLayerIndex="9">
              <elements>
                <DOMSymbolInstance libraryItemName="part">
                  <matrix><Matrix tx="5" ty="7"/></matrix>
                </DOMSymbolInstance>
              </elements>
            </DOMFrame>
          </frames>
        </DOMLayer>
        <DOMLayer name="arm" layerRiggingIndex="9">
          <frames>
            <DOMFrame index="0" duration="11" parentLayerIndex="5">
              <elements>
                <DOMSymbolInstance libraryItemName="part">
                  <matrix><Matrix tx="10" ty="-20"/></matrix>
                </DOMSymbolInstance>
              </elements>
            </DOMFrame>
          </frames>
        </DOMLayer>
        <DOMLayer name="body" layerRiggingIndex="5">
          <frames>
            <DOMFrame index="0" duration="10">
              <elements>
                <DOMSymbolInstance libraryItemName="part">
                  <matrix><Matrix tx="300" ty="400"/></matrix>
                </DOMSymbolInstance>
              </elements>
            </DOMFrame>
            <DOMFrame index="10" duration="1">
              <elements>
                <DOMSymbolInstance libraryItemName="part">
                  <matrix><Matrix tx="380" ty="455"/></matrix>
                </DOMSymbolInstance>
              </elements>
            </DOMFrame>
          </frames>
        </DOMLayer>
      </layers>
    </DOMTimeline>
  </timeline>
</DOMSymbolItem>"##;

        const PART: &str = r##"<DOMSymbolItem xmlns="http://ns.adobe.com/xfl/2008/" name="part"
             symbolType="graphic"><timeline><DOMTimeline name="part"><layers>
             <DOMLayer name="Layer_1"><frames><DOMFrame index="0" duration="1"/>
             </frames></DOMLayer></layers></DOMTimeline></timeline></DOMSymbolItem>"##;

        let (scene, _) = build(
            MINIMAL_DOCUMENT,
            &[
                ("LIBRARY/figure.xml".to_string(), RIG.to_string()),
                ("LIBRARY/part.xml".to_string(), PART.to_string()),
            ],
        )
        .unwrap();

        let figure = scene.library().find_by_name("figure").unwrap();
        let layer = |name: &str| figure.layers.iter().find(|l| l.name == name).unwrap();
        let (body, arm, hand) = (layer("body"), layer("arm"), layer("hand"));

        assert_eq!(arm.follows, Some(body.id), "the arm follows the body");
        assert_eq!(hand.follows, Some(arm.id), "the hand follows the arm");

        // Where each part is actually drawn: its own transform, carried by
        // whatever its chain of parents is doing on that frame.
        let drawn = |l: &buzz_scene::Layer, frame: u32| -> (f64, f64) {
            let follows = figure.layers.inherited_transform(l.id, frame);
            let own = l.objects_at(frame)[0].transform;
            let c = (follows * own).as_coeffs();
            (c[4], c[5])
        };

        // Animate's answer, composed straight down the chain.
        // Frame 0:  body (300,400) · arm (10,-20) · hand (5,7)
        for (name, l, want) in [
            ("body", body, (300.0, 400.0)),
            ("arm", arm, (310.0, 380.0)),
            ("hand", hand, (315.0, 387.0)),
        ] {
            let (x, y) = drawn(l, 0);
            assert!(
                (x - want.0).abs() < 1e-6 && (y - want.1).abs() < 1e-6,
                "at rest the {name} should be drawn at {want:?}, got {:?}",
                (x, y)
            );
        }

        // Frame 10: the body has moved to (380, 455). Everything hanging off
        // it moves by the same (80, 55) and no more.
        for (name, l, want) in [
            ("body", body, (380.0, 455.0)),
            ("arm", arm, (390.0, 435.0)),
            ("hand", hand, (395.0, 442.0)),
        ] {
            let (x, y) = drawn(l, 10);
            assert!(
                (x - want.0).abs() < 1e-6 && (y - want.1).abs() < 1e-6,
                "once the body moves the {name} should be drawn at {want:?}, got {:?}",
                (x, y)
            );
        }
    }

    const RIG_PART: &str = r##"<DOMSymbolItem xmlns="http://ns.adobe.com/xfl/2008/" name="part"
         symbolType="graphic"><timeline><DOMTimeline name="part"><layers>
         <DOMLayer name="Layer_1"><frames><DOMFrame index="0" duration="1"/>
         </frames></DOMLayer></layers></DOMTimeline></timeline></DOMSymbolItem>"##;

    /// A parent layer that opens on a **blank** keyframe: the rig has to look
    /// past it to the first keyframe that actually draws something, or every
    /// child bakes against the identity matrix instead of the parent's real
    /// pose (Findings §4).
    #[test]
    fn a_parent_whose_first_keyframe_is_blank_is_rigged_from_its_first_real_one() {
        const RIG: &str = r##"<?xml version="1.0" encoding="UTF-8"?>
<DOMSymbolItem xmlns="http://ns.adobe.com/xfl/2008/" name="figure" symbolType="graphic">
  <timeline>
    <DOMTimeline name="figure">
      <layers>
        <DOMLayer name="head" layerRiggingIndex="9">
          <frames>
            <DOMFrame index="0" duration="2" parentLayerIndex="5">
              <elements>
                <DOMSymbolInstance libraryItemName="part">
                  <matrix><Matrix tx="10" ty="-20"/></matrix>
                </DOMSymbolInstance>
              </elements>
            </DOMFrame>
          </frames>
        </DOMLayer>
        <DOMLayer name="body" layerRiggingIndex="5">
          <frames>
            <DOMFrame index="0" duration="1"/>
            <DOMFrame index="1" duration="1">
              <elements>
                <DOMSymbolInstance libraryItemName="part">
                  <matrix><Matrix tx="300" ty="400"/></matrix>
                </DOMSymbolInstance>
              </elements>
            </DOMFrame>
          </frames>
        </DOMLayer>
      </layers>
    </DOMTimeline>
  </timeline>
</DOMSymbolItem>"##;

        let (scene, _) = build(
            MINIMAL_DOCUMENT,
            &[
                ("LIBRARY/figure.xml".to_string(), RIG.to_string()),
                ("LIBRARY/part.xml".to_string(), RIG_PART.to_string()),
            ],
        )
        .unwrap();

        let figure = scene.library().find_by_name("figure").unwrap();
        let head = figure.layers.iter().find(|l| l.name == "head").unwrap();
        let placed = head.objects_at(0)[0].transform.as_coeffs();
        assert!(
            (placed[4] - 310.0).abs() < 1e-9 && (placed[5] - 380.0).abs() < 1e-9,
            "the rest pose should come from the body's first *drawn* keyframe (300,400), \
             not its blank one, got {:?}",
            (placed[4], placed[5])
        );
    }

    /// A parent keyframe holding several objects rigs against the **symbol
    /// instance** among them, not whichever object happens to be drawn first.
    #[test]
    fn a_parent_keyframe_with_several_objects_rigs_from_the_symbol_instance() {
        const RIG: &str = r##"<?xml version="1.0" encoding="UTF-8"?>
<DOMSymbolItem xmlns="http://ns.adobe.com/xfl/2008/" name="figure" symbolType="graphic">
  <timeline>
    <DOMTimeline name="figure">
      <layers>
        <DOMLayer name="head" layerRiggingIndex="9">
          <frames>
            <DOMFrame index="0" duration="1" parentLayerIndex="5">
              <elements>
                <DOMSymbolInstance libraryItemName="part">
                  <matrix><Matrix tx="10" ty="-20"/></matrix>
                </DOMSymbolInstance>
              </elements>
            </DOMFrame>
          </frames>
        </DOMLayer>
        <DOMLayer name="body" layerRiggingIndex="5">
          <frames>
            <DOMFrame index="0" duration="1">
              <elements>
                <DOMShape>
                  <edges><Edge fillStyle1="1" edges="!0 0|400 0|400 400|0 400|0 0"/></edges>
                </DOMShape>
                <DOMSymbolInstance libraryItemName="part">
                  <matrix><Matrix tx="300" ty="400"/></matrix>
                </DOMSymbolInstance>
              </elements>
            </DOMFrame>
          </frames>
        </DOMLayer>
      </layers>
    </DOMTimeline>
  </timeline>
</DOMSymbolItem>"##;

        let (scene, _) = build(
            MINIMAL_DOCUMENT,
            &[
                ("LIBRARY/figure.xml".to_string(), RIG.to_string()),
                ("LIBRARY/part.xml".to_string(), RIG_PART.to_string()),
            ],
        )
        .unwrap();

        let figure = scene.library().find_by_name("figure").unwrap();
        let head = figure.layers.iter().find(|l| l.name == "head").unwrap();
        let placed = head.objects_at(0)[0].transform.as_coeffs();
        assert!(
            (placed[4] - 310.0).abs() < 1e-9 && (placed[5] - 380.0).abs() < 1e-9,
            "the rest pose should come from the instance (300,400), not the \
             decorative shape drawn before it, got {:?}",
            (placed[4], placed[5])
        );
    }

    /// A child whose rig parent changes mid-timeline: `follows` can only hold
    /// the one link (the first parent it named), so the keyframe naming a
    /// *different* parent bakes to its absolute pose at that frame instead.
    #[test]
    fn a_child_that_changes_rig_parent_mid_timeline_bakes_the_other_keyframe_absolute() {
        const RIG: &str = r##"<?xml version="1.0" encoding="UTF-8"?>
<DOMSymbolItem xmlns="http://ns.adobe.com/xfl/2008/" name="figure" symbolType="graphic">
  <timeline>
    <DOMTimeline name="figure">
      <layers>
        <DOMLayer name="head" layerRiggingIndex="9">
          <frames>
            <DOMFrame index="0" duration="1" parentLayerIndex="5">
              <elements>
                <DOMSymbolInstance libraryItemName="part">
                  <matrix><Matrix tx="10" ty="-20"/></matrix>
                </DOMSymbolInstance>
              </elements>
            </DOMFrame>
            <DOMFrame index="1" duration="1" parentLayerIndex="6">
              <elements>
                <DOMSymbolInstance libraryItemName="part">
                  <matrix><Matrix tx="1" ty="1"/></matrix>
                </DOMSymbolInstance>
              </elements>
            </DOMFrame>
          </frames>
        </DOMLayer>
        <DOMLayer name="body" layerRiggingIndex="5">
          <frames>
            <DOMFrame index="0" duration="2">
              <elements>
                <DOMSymbolInstance libraryItemName="part">
                  <matrix><Matrix tx="300" ty="400"/></matrix>
                </DOMSymbolInstance>
              </elements>
            </DOMFrame>
          </frames>
        </DOMLayer>
        <DOMLayer name="prop" layerRiggingIndex="6">
          <frames>
            <DOMFrame index="0" duration="1">
              <elements>
                <DOMSymbolInstance libraryItemName="part">
                  <matrix><Matrix tx="0" ty="0"/></matrix>
                </DOMSymbolInstance>
              </elements>
            </DOMFrame>
            <DOMFrame index="1" duration="1">
              <elements>
                <DOMSymbolInstance libraryItemName="part">
                  <matrix><Matrix tx="500" ty="600"/></matrix>
                </DOMSymbolInstance>
              </elements>
            </DOMFrame>
          </frames>
        </DOMLayer>
      </layers>
    </DOMTimeline>
  </timeline>
</DOMSymbolItem>"##;

        let (scene, report) = build(
            MINIMAL_DOCUMENT,
            &[
                ("LIBRARY/figure.xml".to_string(), RIG.to_string()),
                ("LIBRARY/part.xml".to_string(), RIG_PART.to_string()),
            ],
        )
        .unwrap();

        let figure = scene.library().find_by_name("figure").unwrap();
        let head = figure.layers.iter().find(|l| l.name == "head").unwrap();
        let body = figure.layers.iter().find(|l| l.name == "body").unwrap();

        // `follows` keeps the first parent it named — the body.
        assert_eq!(head.follows, Some(body.id));

        // Frame 0 names the body it already follows: baked normally, against
        // the body's rest pose (300,400).
        let at0 = head.objects_at(0)[0].transform.as_coeffs();
        assert!(
            (at0[4] - 310.0).abs() < 1e-9 && (at0[5] - 380.0).abs() < 1e-9,
            "frame 0 should still bake against the body, got {:?}",
            (at0[4], at0[5])
        );

        // Frame 1 names the prop instead — a different rigged layer. Baked to
        // its **absolute** pose there: the prop's own frame-1 transform
        // (500,600), composed with the head's local (1,1), since there is no
        // `follows` link to propagate a rest-relative motion through.
        let at1 = head.objects_at(1)[0].transform.as_coeffs();
        assert!(
            (at1[4] - 501.0).abs() < 1e-9 && (at1[5] - 601.0).abs() < 1e-9,
            "frame 1 should bake absolute against the prop (500,600) + (1,1), got {:?}",
            (at1[4], at1[5])
        );

        assert_eq!(
            report.retargeted_rig_layers, 1,
            "one layer changed rig parent mid-timeline"
        );
    }

    /// **A joint placed in Animate is still a joint after importing.**
    ///
    /// The transformation point is not decoration on a rigged character: it is
    /// the shoulder an arm swings from. Dropped on the way in, every joint went
    /// back to the middle of its artwork and the character had to be re-rigged
    /// by hand before it could be posed.
    #[test]
    fn a_transformation_point_survives_the_import() {
        const WITH_POINT: &str = r##"<?xml version="1.0" encoding="UTF-8"?>
<DOMSymbolItem xmlns="http://ns.adobe.com/xfl/2008/" name="arm" symbolType="graphic">
  <timeline>
    <DOMTimeline name="arm">
      <layers>
        <DOMLayer name="Layer_1">
          <frames>
            <DOMFrame index="0" duration="1">
              <elements>
                <DOMSymbolInstance libraryItemName="part">
                  <matrix><Matrix tx="100" ty="50"/></matrix>
                  <transformationPoint>
                    <Point x="12.5" y="-40"/>
                  </transformationPoint>
                </DOMSymbolInstance>
              </elements>
            </DOMFrame>
          </frames>
        </DOMLayer>
      </layers>
    </DOMTimeline>
  </timeline>
</DOMSymbolItem>"##;

        const PART: &str = r##"<DOMSymbolItem xmlns="http://ns.adobe.com/xfl/2008/" name="part"
             symbolType="graphic"><timeline><DOMTimeline name="part"><layers>
             <DOMLayer name="Layer_1"><frames><DOMFrame index="0" duration="1"/>
             </frames></DOMLayer></layers></DOMTimeline></timeline></DOMSymbolItem>"##;

        let (scene, _) = build(
            MINIMAL_DOCUMENT,
            &[
                ("LIBRARY/arm.xml".to_string(), WITH_POINT.to_string()),
                ("LIBRARY/part.xml".to_string(), PART.to_string()),
            ],
        )
        .unwrap();

        let arm = scene.library().find_by_name("arm").unwrap();
        let placed = arm.layers.iter().next().unwrap().objects_at(0)[0].clone();
        assert_eq!(
            placed.pivot,
            Some(buzz_geom::Point::new(12.5, -40.0)),
            "the joint should have come across as it was placed"
        );

        // And it is where Animate draws it: the point is in the object's own
        // space, so its place on the stage is through the instance's matrix.
        let world = scene.pivot_of(&placed);
        assert!(
            (world.x - 112.5).abs() < 1e-9 && (world.y - 10.0).abs() < 1e-9,
            "the joint landed at {world:?}"
        );
    }

    /// Animate's camera layer becomes our camera, not an error.
    #[test]
    fn the_camera_layer_is_imported_as_the_camera() {
        const WITH_CAMERA: &str = r##"<?xml version="1.0" encoding="UTF-8"?>
<DOMDocument xmlns="http://ns.adobe.com/xfl/2008/" width="1920" height="1080" frameRate="24">
  <timelines>
    <DOMTimeline name="Scene 1" cameraLayerEnabled="true">
      <layers>
        <DOMLayer name="Camera" layerType="camera">
          <frames>
            <DOMFrame index="0" duration="50">
              <elements>
                <DOMSymbolInstance libraryItemName="__Camera__" name="___camera___instance">
                  <matrix><Matrix tx="960" ty="540"/></matrix>
                </DOMSymbolInstance>
              </elements>
            </DOMFrame>
            <DOMFrame index="50">
              <elements>
                <DOMSymbolInstance libraryItemName="__Camera__" name="___camera___instance">
                  <matrix><Matrix a="0.5" d="0.5" tx="400" ty="300"/></matrix>
                </DOMSymbolInstance>
              </elements>
            </DOMFrame>
          </frames>
        </DOMLayer>
        <DOMLayer name="Art">
          <frames><DOMFrame index="0" duration="1"/></frames>
        </DOMLayer>
      </layers>
    </DOMTimeline>
  </timelines>
</DOMDocument>"##;

        let (scene, report) = build(WITH_CAMERA, &[]).unwrap();

        assert!(
            !report.unsupported.iter().any(|u| u.contains("__Camera__")),
            "the camera is not an unknown symbol: {:?}",
            report.unsupported
        );
        assert_eq!(report.camera_keys, 2);
        assert_eq!(
            scene.layers().len(),
            1,
            "the camera layer is the camera, not a layer of artwork"
        );

        let camera = scene.camera();
        assert!(camera.enabled, "the file said the camera is on");
        let keys = camera.keys();
        assert_eq!(keys[0].center, buzz_geom::Point::new(960.0, 540.0));
        assert!((keys[0].zoom - 1.0).abs() < 1e-9);

        // **The first span is not tweened, so it holds.** Written as a second
        // key with the same values at the span's last frame, which is how a
        // track that interpolates everything expresses a cut.
        assert_eq!(keys.len(), 3, "a held span gains a key at its end");
        assert_eq!(keys[1].frame, 49);
        assert!((keys[1].zoom - 1.0).abs() < 1e-9, "the hold must not drift");

        // A camera scaled to half shows half the stage: a zoom of two.
        assert_eq!(keys[2].frame, 50);
        assert!(
            (keys[2].zoom - 2.0).abs() < 1e-9,
            "zoom was {}",
            keys[2].zoom
        );
        assert_eq!(keys[2].center, buzz_geom::Point::new(400.0, 300.0));
    }

    /// A *tweened* camera span must not gain a hold: it moves the whole way.
    #[test]
    fn a_tweened_camera_span_keeps_moving() {
        const TWEENED: &str = r##"<?xml version="1.0" encoding="UTF-8"?>
<DOMDocument xmlns="http://ns.adobe.com/xfl/2008/" width="1920" height="1080">
  <timelines>
    <DOMTimeline name="Scene 1" cameraLayerEnabled="true">
      <layers>
        <DOMLayer name="Camera" layerType="camera">
          <frames>
            <DOMFrame index="0" duration="30" tweenType="motion">
              <elements>
                <DOMSymbolInstance libraryItemName="__Camera__">
                  <matrix><Matrix tx="960" ty="540"/></matrix>
                </DOMSymbolInstance>
              </elements>
            </DOMFrame>
            <DOMFrame index="30">
              <elements>
                <DOMSymbolInstance libraryItemName="__Camera__">
                  <matrix><Matrix a="0.5" d="0.5" tx="960" ty="540"/></matrix>
                </DOMSymbolInstance>
              </elements>
            </DOMFrame>
          </frames>
        </DOMLayer>
      </layers>
    </DOMTimeline>
  </timelines>
</DOMDocument>"##;

        let (scene, _) = build(TWEENED, &[]).unwrap();
        assert_eq!(scene.camera().keys().len(), 2, "no hold on a tweened span");
        let middle = scene.camera().state_at(15).expect("a state halfway");
        assert!(
            middle.zoom > 1.2 && middle.zoom < 1.8,
            "the move should be underway at halfway, not held: {}",
            middle.zoom
        );
    }

    /// Animate writes damaged symbol files, and one bad frame must not cost
    /// the whole symbol.
    #[test]
    fn a_symbol_with_broken_xml_keeps_what_reads() {
        // `<DOMShape` with no closing bracket, exactly as Animate saved it.
        const DAMAGED: &str = r##"<?xml version="1.0" encoding="UTF-8"?>
<DOMSymbolItem xmlns="http://ns.adobe.com/xfl/2008/" name="hurt" symbolType="graphic">
  <timeline>
    <DOMTimeline name="hurt">
      <layers>
        <DOMLayer name="Layer 1">
          <frames>
            <DOMFrame index="0" duration="1">
              <elements>
                <DOMShape>
                  <edges><Edge fillStyle0="1" edges="!0 0|2000 0|2000 2000|0 2000|0 0"/></edges>
                </DOMShape>
              </elements>
            </DOMFrame>
            <DOMFrame index="1" duration="1">
              <elements>
                <DOMShape
                </DOMShape>
              </elements>
            </DOMFrame>
          </frames>
        </DOMLayer>
      </layers>
    </DOMTimeline>
  </timeline>
</DOMSymbolItem>"##;

        let (scene, report) = build(
            MINIMAL_DOCUMENT,
            &[("LIBRARY/hurt.xml".to_string(), DAMAGED.to_string())],
        )
        .unwrap();

        let hurt = scene
            .library()
            .find_by_name("hurt")
            .expect("the symbol should survive its bad frame");
        assert!(
            hurt.bounds().is_some(),
            "the artwork before the damage should still be there"
        );
        assert!(
            report.unsupported.iter().any(|u| u.contains("damaged XML")),
            "and the damage should be named: {:?}",
            report.unsupported
        );
    }

    /// Anything unsupported must be reported, not silently dropped.
    #[test]
    fn unsupported_features_are_reported() {
        let with_bitmap = MINIMAL_DOCUMENT.replace(
            "<DOMSymbolInstance libraryItemName=\"hero\">",
            "<DOMBitmapInstance libraryItemName=\"photo.png\"/><DOMStaticText/>\
             <DOMSymbolInstance libraryItemName=\"hero\">",
        );
        let (_, report) = build(&with_bitmap, &[]).unwrap();

        assert!(!report.is_complete());
        assert!(
            report.unsupported.iter().any(|u| u.contains("bitmap")),
            "bitmaps should be reported: {:?}",
            report.unsupported
        );
        assert!(report.unsupported.iter().any(|u| u.contains("text")));
    }

    #[test]
    fn repeated_unsupported_features_are_counted_not_repeated() {
        let mut report = ImportReport::default();
        for _ in 0..50 {
            report.note_unsupported("bitmap");
        }
        assert_eq!(report.unsupported.len(), 1, "one line, with a count");
        assert!(
            report.unsupported[0].contains("x50"),
            "{:?}",
            report.unsupported
        );
    }

    #[test]
    fn ids_do_not_collide_with_later_edits() {
        let (mut scene, _) = import_sample();
        let existing: Vec<u64> = scene
            .layers()
            .iter()
            .flat_map(|l| l.all_objects().map(|o| o.id.0))
            .collect();
        let fresh = scene.next_object_id();
        assert!(
            !existing.contains(&fresh.0),
            "a new object collided with an imported one"
        );
    }

    #[test]
    fn a_legacy_binary_fla_is_refused_with_advice() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("old.fla");
        // OLE2 compound document signature.
        std::fs::write(
            &path,
            [0xD0u8, 0xCF, 0x11, 0xE0, 0xA1, 0xB1, 0x1A, 0xE1, 0, 0],
        )
        .unwrap();

        match import(&path) {
            Err(ImportError::LegacyBinaryFla) => {}
            other => panic!("expected a clear legacy error, got {other:?}"),
        }
    }

    #[test]
    fn a_document_without_dom_document_is_refused() {
        let mut buffer = std::io::Cursor::new(Vec::new());
        {
            let mut zip = zip::ZipWriter::new(&mut buffer);
            zip.start_file("something.txt", zip::write::SimpleFileOptions::default())
                .unwrap();
            use std::io::Write;
            zip.write_all(b"not an animate document").unwrap();
            zip.finish().unwrap();
        }
        assert!(matches!(
            import_fla_bytes(&buffer.into_inner()),
            Err(ImportError::MissingDocument(_))
        ));
    }

    #[test]
    fn malformed_xml_is_an_error_not_a_panic() {
        assert!(build("<DOMDocument><unclosed>", &[]).is_err());
    }

    #[test]
    fn an_empty_document_still_yields_one_layer() {
        let (scene, _) = build(
            r##"<DOMDocument width="100" height="100"><timelines><DOMTimeline/></timelines></DOMDocument>"##,
            &[],
        )
        .unwrap();
        assert_eq!(
            scene.layers().len(),
            1,
            "a document needs somewhere to draw"
        );
    }

    #[test]
    fn an_xfl_folder_imports_the_same_as_an_archive() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("DOMDocument.xml"), MINIMAL_DOCUMENT).unwrap();
        let library = dir.path().join("LIBRARY");
        std::fs::create_dir_all(&library).unwrap();
        std::fs::write(library.join("hero.xml"), HERO_SYMBOL).unwrap();

        let (scene, report) = import(dir.path()).unwrap();
        assert_eq!(scene.layers().len(), 2);
        assert_eq!(report.symbols, 1);
        assert_eq!(report.instances, 1);
    }

    #[test]
    fn a_zipped_fla_imports() {
        let mut buffer = std::io::Cursor::new(Vec::new());
        {
            use std::io::Write;
            let mut zip = zip::ZipWriter::new(&mut buffer);
            let options = zip::write::SimpleFileOptions::default();
            zip.start_file("DOMDocument.xml", options).unwrap();
            zip.write_all(MINIMAL_DOCUMENT.as_bytes()).unwrap();
            zip.start_file("LIBRARY/hero.xml", options).unwrap();
            zip.write_all(HERO_SYMBOL.as_bytes()).unwrap();
            zip.finish().unwrap();
        }

        let (scene, report) = import_fla_bytes(&buffer.into_inner()).unwrap();
        assert_eq!(scene.layers().len(), 2);
        assert_eq!(report.symbols, 1);
        assert!(report.summary().contains("2 layers"));
    }

    /// Animate writes layers bottom-first and points a nested layer at its
    /// folder by *index into that list*, so the link has to be resolved before
    /// the list is reversed.
    const NESTED_LAYERS: &str = r##"<?xml version="1.0" encoding="UTF-8"?>
<DOMDocument xmlns="http://ns.adobe.com/xfl/2008/" width="550" height="400">
  <timelines>
    <DOMTimeline name="Scene 1">
      <layers>
        <DOMLayer name="Folder" layerType="folder"/>
        <DOMLayer name="Inside" parentLayerIndex="0"/>
        <DOMLayer name="Outside"/>
      </layers>
    </DOMTimeline>
  </timelines>
</DOMDocument>"##;

    #[test]
    fn a_layer_inside_a_folder_arrives_inside_that_folder() {
        let (scene, _) = build(NESTED_LAYERS, &[]).unwrap();

        let folder = scene
            .layers()
            .iter()
            .find(|l| l.name == "Folder")
            .expect("the folder came across");
        let inside = scene.layers().iter().find(|l| l.name == "Inside").unwrap();
        let outside = scene.layers().iter().find(|l| l.name == "Outside").unwrap();

        assert_eq!(folder.kind, LayerKind::Folder);
        assert_eq!(
            inside.parent,
            Some(folder.id),
            "parentLayerIndex must survive the bottom-first-to-top-first flip"
        );
        assert_eq!(outside.parent, None);
    }

    /// Animate overloads `parentLayerIndex`: a **masked** layer points at its
    /// mask with the same attribute a nested layer uses for its folder — and
    /// it does *not* write `layerType="masked"`, whatever the format allows.
    /// This is copied from a real file: the mask first, the layer it claims
    /// after it, pointing back at it.
    const MASKED_LAYERS: &str = r##"<?xml version="1.0" encoding="UTF-8"?>
<DOMDocument xmlns="http://ns.adobe.com/xfl/2008/" width="550" height="400">
  <timelines>
    <DOMTimeline name="Scene 1">
      <layers>
        <DOMLayer name="TheMask" layerType="mask" outline="true"/>
        <DOMLayer name="Masked" parentLayerIndex="0"/>
      </layers>
    </DOMTimeline>
  </timelines>
</DOMDocument>"##;

    #[test]
    fn a_masked_layer_is_not_nested_inside_its_mask() {
        let (scene, _) = build(MASKED_LAYERS, &[]).unwrap();

        let masked = scene.layers().iter().find(|l| l.name == "Masked").unwrap();
        let mask = scene.layers().iter().find(|l| l.name == "TheMask").unwrap();

        assert_eq!(mask.kind, LayerKind::Mask);
        assert_eq!(
            masked.kind,
            LayerKind::Masked,
            "a layer pointing at a mask is masked, whatever its layerType says"
        );
        assert_eq!(
            masked.parent, None,
            "a mask is not a folder; masking is resolved positionally"
        );

        // And the positional rule does the real work: the mask sits above the
        // layer it claims, which is what the renderer reads.
        assert_eq!(
            scene.layers().mask_for(masked.id),
            Some(mask.id),
            "the mask should still claim the layer beneath it"
        );
    }

    /// A corrupt or hand-edited file must not be able to make a layer its own
    /// parent, or point outside the list.
    const BAD_PARENTS: &str = r##"<?xml version="1.0" encoding="UTF-8"?>
<DOMDocument xmlns="http://ns.adobe.com/xfl/2008/" width="550" height="400">
  <timelines>
    <DOMTimeline name="Scene 1">
      <layers>
        <DOMLayer name="SelfParent" layerType="folder" parentLayerIndex="0"/>
        <DOMLayer name="OffTheEnd" parentLayerIndex="99"/>
        <DOMLayer name="NotANumber" parentLayerIndex="banana"/>
      </layers>
    </DOMTimeline>
  </timelines>
</DOMDocument>"##;

    /// XFL declares fills once per shape and has each edge reference one by
    /// index. Ignoring the table imports every file as flat grey, which is
    /// what this guards against.
    const STYLED_SHAPES: &str = r##"<?xml version="1.0" encoding="UTF-8"?>
<DOMDocument xmlns="http://ns.adobe.com/xfl/2008/" width="550" height="400">
  <timelines>
    <DOMTimeline name="Scene 1">
      <layers>
        <DOMLayer name="Art">
          <frames>
            <DOMFrame index="0" duration="1">
              <elements>
                <DOMShape>
                  <fills>
                    <FillStyle index="1"><SolidColor color="#3366CC"/></FillStyle>
                    <FillStyle index="2"><SolidColor color="#FF0000" alpha="0.5"/></FillStyle>
                  </fills>
                  <strokes>
                    <StrokeStyle index="1">
                      <SolidStroke weight="4">
                        <fill><SolidColor color="#00FF00"/></fill>
                      </SolidStroke>
                    </StrokeStyle>
                  </strokes>
                  <edges>
                    <Edge fillStyle1="1" edges="!0 0|2000 0|2000 2000|0 2000|0 0"/>
                    <Edge fillStyle0="2" edges="!0 0|400 0|400 400|0 400|0 0"/>
                    <Edge strokeStyle="1" edges="!0 0|2000 0"/>
                  </edges>
                </DOMShape>
              </elements>
            </DOMFrame>
          </frames>
        </DOMLayer>
      </layers>
    </DOMTimeline>
  </timelines>
</DOMDocument>"##;

    fn shapes_of(scene: &Scene) -> Vec<ShapeData> {
        scene
            .layers()
            .iter()
            .flat_map(|l| l.all_objects())
            .filter_map(|o| match &o.kind {
                buzz_scene::ObjectKind::Shape(s) => Some(s.clone()),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn an_edge_takes_the_colour_of_the_fill_style_it_references() {
        let (scene, _) = build(STYLED_SHAPES, &[]).unwrap();
        let shapes = shapes_of(&scene);
        assert_eq!(shapes.len(), 3);

        let first = shapes[0]
            .fill
            .as_ref()
            .expect("the first edge is filled")
            .color();
        assert_eq!(
            first.to_rgba8().to_u8_array(),
            [0x33, 0x66, 0xCC, 0xFF],
            "fillStyle1=1 must resolve to the declared blue, not a grey default"
        );
    }

    /// `fillStyle0` names the fill on the other side of the edge, but it is
    /// still that fill's colour.
    #[test]
    fn a_fill_referenced_by_fill_style_zero_is_resolved_too() {
        let (scene, _) = build(STYLED_SHAPES, &[]).unwrap();
        let second = shapes_of(&scene)[1].fill.as_ref().expect("filled").color();
        let [r, g, b, a] = second.to_rgba8().to_u8_array();
        assert_eq!([r, g, b], [0xFF, 0x00, 0x00]);
        assert_eq!(a, 128, "the alpha attribute must be honoured");
    }

    #[test]
    fn a_stroke_style_supplies_both_colour_and_weight() {
        let (scene, _) = build(STYLED_SHAPES, &[]).unwrap();
        let shapes = shapes_of(&scene);
        let stroke = shapes[2]
            .stroke
            .as_ref()
            .expect("the third edge is stroked");
        assert_eq!(
            stroke.color().to_rgba8().to_u8_array(),
            [0x00, 0xFF, 0x00, 0xFF]
        );
        assert_eq!(stroke.width, 4.0);
    }

    /// A gradient now arrives as a gradient, with its stops where the file put
    /// them — not as the average colour it used to be flattened to.
    #[test]
    fn a_linear_gradient_fill_arrives_as_a_gradient() {
        const GRADIENT: &str = r##"<?xml version="1.0" encoding="UTF-8"?>
<DOMDocument xmlns="http://ns.adobe.com/xfl/2008/" width="550" height="400">
  <timelines><DOMTimeline name="Scene 1"><layers>
    <DOMLayer name="Art"><frames><DOMFrame index="0" duration="1"><elements>
      <DOMShape>
        <fills>
          <FillStyle index="1">
            <LinearGradient>
              <matrix><Matrix a="0.1" d="0.1" tx="50" ty="50"/></matrix>
              <GradientEntry color="#000000" ratio="0"/>
              <GradientEntry color="#FF0000" ratio="0.25"/>
              <GradientEntry color="#FFFFFF" ratio="1"/>
            </LinearGradient>
          </FillStyle>
        </fills>
        <edges><Edge fillStyle1="1" edges="!0 0|2000 0|2000 2000|0 2000|0 0"/></edges>
      </DOMShape>
    </elements></DOMFrame></frames></DOMLayer>
  </layers></DOMTimeline></timelines>
</DOMDocument>"##;

        let (scene, _) = build(GRADIENT, &[]).unwrap();
        let shapes = shapes_of(&scene);
        let fill = shapes[0].fill.as_ref().expect("filled");
        let g = fill.paint.gradient().expect("it should be a gradient");

        assert_eq!(g.kind, GradientKind::Linear);
        assert_eq!(g.stops().len(), 3);
        // **The ratios are the point.** These were thrown away entirely while
        // gradients were averaged, and a file whose middle stop sits at a
        // quarter draws nothing like one where it sits halfway.
        assert!((g.stops()[1].offset - 0.25).abs() < 1e-9, "{:?}", g.stops());
        assert_eq!(
            g.stops()[1].color.to_rgba8().to_u8_array()[..3],
            [255, 0, 0]
        );

        // The matrix maps Flash's 1638.4-pixel gradient box, so a scale of 0.1
        // puts the ramp's end 81.92 pixels from its centre at (50, 50).
        let h = g.handles();
        assert!((h.center.x - 50.0).abs() < 1e-6, "centre {:?}", h.center);
        assert!(
            (h.end.x - (50.0 + 81.92)).abs() < 1e-6,
            "the gradient box should be 1638.4px wide, got end {:?}",
            h.end
        );
    }

    /// The spread mode and a radial gradient's focal point both come across.
    #[test]
    fn a_radial_gradient_keeps_its_spread_and_focal_point() {
        const GRADIENT: &str = r##"<?xml version="1.0" encoding="UTF-8"?>
<DOMDocument xmlns="http://ns.adobe.com/xfl/2008/" width="550" height="400">
  <timelines><DOMTimeline name="Scene 1"><layers>
    <DOMLayer name="Art"><frames><DOMFrame index="0" duration="1"><elements>
      <DOMShape>
        <fills>
          <FillStyle index="1">
            <RadialGradient spreadMethod="reflect" focalPointRatio="0.5">
              <matrix><Matrix a="0.1" d="0.1"/></matrix>
              <GradientEntry color="#FF0000" ratio="0"/>
              <GradientEntry color="#0000FF" ratio="1"/>
            </RadialGradient>
          </FillStyle>
        </fills>
        <edges><Edge fillStyle1="1" edges="!0 0|2000 0|2000 2000|0 2000|0 0"/></edges>
      </DOMShape>
    </elements></DOMFrame></frames></DOMLayer>
  </layers></DOMTimeline></timelines>
</DOMDocument>"##;

        let (scene, _) = build(GRADIENT, &[]).unwrap();
        let shapes = shapes_of(&scene);
        let fill = shapes[0].fill.as_ref().expect("filled");
        let g = fill.paint.gradient().expect("it should be a gradient");

        assert_eq!(g.kind, GradientKind::Radial);
        assert_eq!(g.spread, GradientSpread::Reflect);
        assert!((g.focal - 0.5).abs() < 1e-9, "focal was {}", g.focal);
    }

    /// **A gradient without a matrix must not swallow the next instance's.**
    ///
    /// Adobe writes `<matrix>` as a gradient's first child, so a gradient that
    /// has none never gets one — but the claim used to stay armed, and the next
    /// `<Matrix>` in the file belongs to a symbol instance. Every instance
    /// after such a gradient collapsed to the origin, which on a real film is
    /// most of the artwork piled into one corner.
    ///
    /// This is the mirror of the bug the `Matrix` arm's comment describes, and
    /// it was introduced by the fix for the other direction.
    #[test]
    fn a_gradient_without_a_matrix_does_not_steal_the_next_instances() {
        const NO_MATRIX: &str = r##"<?xml version="1.0" encoding="UTF-8"?>
<DOMDocument xmlns="http://ns.adobe.com/xfl/2008/" width="550" height="400">
  <symbols><Include href="hero.xml"/></symbols>
  <timelines><DOMTimeline name="Scene 1"><layers>
    <DOMLayer name="Art"><frames><DOMFrame index="0" duration="1"><elements>
      <DOMShape>
        <fills>
          <FillStyle index="1">
            <RadialGradient>
              <GradientEntry color="#FFDD55" ratio="0"/>
              <GradientEntry color="#FF8800" ratio="1"/>
            </RadialGradient>
          </FillStyle>
        </fills>
        <edges><Edge fillStyle1="1" edges="!0 0|2000 0|2000 2000|0 2000|0 0"/></edges>
      </DOMShape>
      <DOMSymbolInstance libraryItemName="hero">
        <matrix><Matrix a="1" d="1" tx="320" ty="240"/></matrix>
      </DOMSymbolInstance>
    </elements></DOMFrame></frames></DOMLayer>
  </layers></DOMTimeline></timelines>
</DOMDocument>"##;

        let (scene, _) = build(
            NO_MATRIX,
            &[("LIBRARY/hero.xml".to_string(), HERO_SYMBOL.to_string())],
        )
        .unwrap();

        let layers = scene.layers();
        let objects = layers.iter().next().expect("one layer").objects_at(0);
        let instance = objects
            .iter()
            .find(|o| matches!(o.kind, buzz_scene::ObjectKind::Instance(_)))
            .expect("the instance should be placed");
        let c = instance.transform.as_coeffs();
        assert!(
            (c[4] - 320.0).abs() < 1e-9 && (c[5] - 240.0).abs() < 1e-9,
            "the gradient swallowed the instance's matrix: {c:?}"
        );

        // And the gradient itself is still a gradient — disarming the claim
        // must not have thrown the fill away.
        let shape = objects
            .iter()
            .find_map(|o| match &o.kind {
                buzz_scene::ObjectKind::Shape(s) => Some(s),
                _ => None,
            })
            .expect("the gradient-filled shape");
        assert!(
            shape.fill.as_ref().is_some_and(|f| f.paint.is_gradient()),
            "the gradient fill was lost"
        );
    }

    /// A gradient **with** a matrix still gets it, and only it — the claim is
    /// spent on the first one and the instance that follows keeps its own.
    /// Both halves in one file, which is the arrangement a real film has.
    #[test]
    fn one_gradient_with_a_matrix_and_one_without_both_behave() {
        const MIXED: &str = r##"<?xml version="1.0" encoding="UTF-8"?>
<DOMDocument xmlns="http://ns.adobe.com/xfl/2008/" width="550" height="400">
  <symbols><Include href="hero.xml"/></symbols>
  <timelines><DOMTimeline name="Scene 1"><layers>
    <DOMLayer name="Art"><frames><DOMFrame index="0" duration="1"><elements>
      <DOMShape>
        <fills>
          <FillStyle index="1">
            <LinearGradient>
              <matrix><Matrix a="0.1" d="0.1" tx="10" ty="10"/></matrix>
              <GradientEntry color="#000000" ratio="0"/>
              <GradientEntry color="#FFFFFF" ratio="1"/>
            </LinearGradient>
          </FillStyle>
          <FillStyle index="2">
            <RadialGradient>
              <GradientEntry color="#FF0000" ratio="0"/>
              <GradientEntry color="#0000FF" ratio="1"/>
            </RadialGradient>
          </FillStyle>
        </fills>
        <edges>
          <Edge fillStyle1="1" edges="!0 0|2000 0|2000 2000|0 2000|0 0"/>
          <Edge fillStyle1="2" edges="!4000 0|6000 0|6000 2000|4000 2000|4000 0"/>
        </edges>
      </DOMShape>
      <DOMSymbolInstance libraryItemName="hero">
        <matrix><Matrix a="1" d="1" tx="150" ty="250"/></matrix>
      </DOMSymbolInstance>
    </elements></DOMFrame></frames></DOMLayer>
  </layers></DOMTimeline></timelines>
</DOMDocument>"##;

        let (scene, _) = build(
            MIXED,
            &[("LIBRARY/hero.xml".to_string(), HERO_SYMBOL.to_string())],
        )
        .unwrap();

        let layers = scene.layers();
        let objects = layers.iter().next().expect("one layer").objects_at(0);

        let instance = objects
            .iter()
            .find(|o| matches!(o.kind, buzz_scene::ObjectKind::Instance(_)))
            .expect("the instance should be placed");
        let c = instance.transform.as_coeffs();
        assert!(
            (c[4] - 150.0).abs() < 1e-9 && (c[5] - 250.0).abs() < 1e-9,
            "the instance took the wrong matrix: {c:?}"
        );

        // The gradient that *did* have a matrix is placed by it: a scale of 0.1
        // over Flash's 1638.4-pixel box puts the ramp's end 81.92 from centre.
        let placed = objects
            .iter()
            .find_map(|o| match &o.kind {
                buzz_scene::ObjectKind::Shape(s) => s
                    .fill
                    .as_ref()
                    .and_then(|f| f.paint.gradient())
                    .filter(|g| g.kind == GradientKind::Linear),
                _ => None,
            })
            .expect("the linear gradient");
        let h = placed.handles();
        assert!((h.center.x - 10.0).abs() < 1e-6, "centre {:?}", h.center);
        assert!(
            (h.end.x - (10.0 + 81.92)).abs() < 1e-6,
            "the gradient with a matrix lost it: end {:?}",
            h.end
        );
    }

    /// **The gradient's matrix must not place an instance.** It is the bug the
    /// `Matrix` arm's comment describes, from the other side: now that a
    /// gradient claims its own matrix, an instance that follows one must still
    /// get its own rather than the gradient's fraction-of-a-percent scale.
    #[test]
    fn a_gradients_matrix_is_not_taken_as_a_placement() {
        const MIXED: &str = r##"<?xml version="1.0" encoding="UTF-8"?>
<DOMDocument xmlns="http://ns.adobe.com/xfl/2008/" width="550" height="400">
  <symbols><Include href="hero.xml"/></symbols>
  <timelines><DOMTimeline name="Scene 1"><layers>
    <DOMLayer name="Art"><frames><DOMFrame index="0" duration="1"><elements>
      <DOMShape>
        <fills>
          <FillStyle index="1">
            <LinearGradient>
              <matrix><Matrix a="0.05" d="0.05" tx="10" ty="10"/></matrix>
              <GradientEntry color="#000000" ratio="0"/>
              <GradientEntry color="#FFFFFF" ratio="1"/>
            </LinearGradient>
          </FillStyle>
        </fills>
        <edges><Edge fillStyle1="1" edges="!0 0|2000 0|2000 2000|0 2000|0 0"/></edges>
      </DOMShape>
      <DOMSymbolInstance libraryItemName="hero">
        <matrix><Matrix a="1" d="1" tx="200" ty="100"/></matrix>
      </DOMSymbolInstance>
    </elements></DOMFrame></frames></DOMLayer>
  </layers></DOMTimeline></timelines>
</DOMDocument>"##;

        let (scene, _) = build(
            MIXED,
            &[("LIBRARY/hero.xml".to_string(), HERO_SYMBOL.to_string())],
        )
        .unwrap();
        let layers = scene.layers();
        let objects = layers.iter().next().expect("one layer").objects_at(0);
        let instance = objects
            .iter()
            .find(|o| matches!(o.kind, buzz_scene::ObjectKind::Instance(_)))
            .expect("the instance should be placed");
        let c = instance.transform.as_coeffs();
        assert!(
            (c[4] - 200.0).abs() < 1e-9 && (c[5] - 100.0).abs() < 1e-9,
            "the instance took the wrong matrix: {c:?}"
        );
        assert!(
            (c[0] - 1.0).abs() < 1e-9,
            "the instance collapsed to the gradient's scale: {c:?}"
        );
    }

    /// Each shape gets its own style table, so a later shape must not pick up
    /// an earlier one's colours by index.
    #[test]
    fn style_tables_do_not_leak_between_shapes() {
        const TWO_SHAPES: &str = r##"<?xml version="1.0" encoding="UTF-8"?>
<DOMDocument xmlns="http://ns.adobe.com/xfl/2008/" width="550" height="400">
  <timelines><DOMTimeline name="Scene 1"><layers>
    <DOMLayer name="Art"><frames><DOMFrame index="0" duration="1"><elements>
      <DOMShape>
        <fills><FillStyle index="1"><SolidColor color="#112233"/></FillStyle></fills>
        <edges><Edge fillStyle1="1" edges="!0 0|400 0|400 400|0 400|0 0"/></edges>
      </DOMShape>
      <DOMShape>
        <edges><Edge fillStyle1="1" edges="!0 0|400 0|400 400|0 400|0 0"/></edges>
      </DOMShape>
    </elements></DOMFrame></frames></DOMLayer>
  </layers></DOMTimeline></timelines>
</DOMDocument>"##;

        let (scene, _) = build(TWO_SHAPES, &[]).unwrap();
        let shapes = shapes_of(&scene);
        assert_eq!(shapes.len(), 2);

        assert_eq!(
            shapes[0]
                .fill
                .as_ref()
                .unwrap()
                .color()
                .to_rgba8()
                .to_u8_array(),
            [0x11, 0x22, 0x33, 0xFF]
        );
        // The second shape declares no styles, so it falls back to the visible
        // default rather than inheriting the first shape's blue.
        assert_ne!(
            shapes[1]
                .fill
                .as_ref()
                .unwrap()
                .color()
                .to_rgba8()
                .to_u8_array(),
            [0x11, 0x22, 0x33, 0xFF],
            "the second shape must not inherit the first shape's fill table"
        );
    }

    #[test]
    fn nonsense_parent_indexes_are_ignored_rather_than_believed() {
        let (scene, _) = build(BAD_PARENTS, &[]).unwrap();
        for layer in scene.layers().iter() {
            assert_eq!(layer.parent, None, "{} should have no parent", layer.name);
        }
    }

    // -- sound (Phase E) ------------------------------------------------------

    /// A tiny MP3, committed rather than generated: `buzz-audio` has no
    /// encoder, only decoders (its own doc comment says so), so there is
    /// nothing in the workspace to make one with at test time.
    const TINY_MP3: &[u8] = include_bytes!("../tests/data/tiny.mp3");

    #[test]
    fn a_sound_in_the_bin_cache_becomes_a_library_asset_and_a_frame_cue() {
        const DOCUMENT: &str = r##"<?xml version="1.0" encoding="UTF-8"?>
<DOMDocument xmlns="http://ns.adobe.com/xfl/2008/" width="200" height="200" frameRate="24">
  <media>
    <DOMSoundItem name="Beep.mp3" href="Beep.mp3" soundDataHRef="Beep.dat"
                  sourceExternalFilepath="../audios/Beep.mp3"/>
  </media>
  <timelines>
    <DOMTimeline name="Scene 1">
      <layers>
        <DOMLayer name="sound">
          <frames>
            <DOMFrame index="0" duration="1" soundName="Beep.mp3" soundSync="stream">
              <SoundEnvelope>
                <SoundEnvelopePoint level0="16384" level1="16384"/>
              </SoundEnvelope>
              <elements/>
            </DOMFrame>
          </frames>
        </DOMLayer>
      </layers>
    </DOMTimeline>
  </timelines>
</DOMDocument>"##;

        let bytes = zip_up(&[
            ("DOMDocument.xml", DOCUMENT.as_bytes()),
            ("bin/Beep.dat", TINY_MP3),
        ]);
        let (scene, report) = import_fla_bytes(&bytes).expect("the sound should import");

        assert_eq!(scene.sounds().len(), 1, "{:?}", report);
        let asset = scene.sounds().iter().next().expect("one sound asset");
        assert_eq!(asset.name, "Beep.mp3");
        assert_eq!(asset.format, "mp3");
        assert!(asset.sample_rate > 0 && asset.length > 0);

        let layer = scene
            .layers()
            .iter()
            .find(|l| l.name == "sound")
            .expect("the sound layer");
        let cue = layer
            .frames
            .keyframe_at(0)
            .and_then(|k| k.sound)
            .expect("the frame should carry the cue");
        assert_eq!(cue.sound, asset.id);
        assert_eq!(cue.sync, buzz_scene::SoundSync::Stream);
        assert!(
            (cue.volume - 0.5).abs() < 1e-6,
            "16384/32768 should read as half volume, got {}",
            cue.volume
        );
    }

    #[test]
    fn a_sound_whose_cache_is_missing_falls_back_to_its_external_file() {
        const DOCUMENT: &str = r##"<?xml version="1.0" encoding="UTF-8"?>
<DOMDocument xmlns="http://ns.adobe.com/xfl/2008/" width="200" height="200" frameRate="24">
  <media>
    <DOMSoundItem name="Beep.mp3" href="Beep.mp3" soundDataHRef="NotThere.dat"
                  sourceExternalFilepath="audios/Beep.mp3"/>
  </media>
  <timelines>
    <DOMTimeline name="Scene 1">
      <layers>
        <DOMLayer name="sound"><frames><DOMFrame index="0" duration="1"/></frames></DOMLayer>
      </layers>
    </DOMTimeline>
  </timelines>
</DOMDocument>"##;

        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("DOMDocument.xml"), DOCUMENT).unwrap();
        std::fs::create_dir_all(dir.path().join("audios")).unwrap();
        std::fs::write(dir.path().join("audios/Beep.mp3"), TINY_MP3).unwrap();

        let (scene, _) = import(dir.path()).expect("the fallback file should import");
        assert_eq!(scene.sounds().len(), 1);
        let asset = scene.sounds().iter().next().unwrap();
        assert_eq!(asset.format, "mp3");
    }

    #[test]
    fn a_sound_with_no_media_anywhere_is_reported_not_fatal() {
        const DOCUMENT: &str = r##"<?xml version="1.0" encoding="UTF-8"?>
<DOMDocument xmlns="http://ns.adobe.com/xfl/2008/" width="200" height="200">
  <media>
    <DOMSoundItem name="Gone.mp3" href="Gone.mp3" soundDataHRef="Gone.dat"/>
  </media>
  <timelines><DOMTimeline name="Scene 1"><layers/></DOMTimeline></timelines>
</DOMDocument>"##;

        let (scene, report) = build(DOCUMENT, &[]).expect("a missing sound is not fatal");
        assert_eq!(scene.sounds().len(), 0);
        assert!(
            report
                .unsupported
                .iter()
                .any(|u| u.contains("sound media unavailable: Gone.mp3")),
            "{:?}",
            report.unsupported
        );
    }

    /// bhatta2 is a real production file kept outside the repository (see
    /// `docs/IMPORT_FIDELITY_PLAN.md`'s Evidence files); this only runs where
    /// it happens to be, so its absence on another machine or in CI is not a
    /// failure.
    #[test]
    fn bhatta2_imports_its_three_sounds_and_they_are_not_silent() {
        let path = Path::new("B:/youtubeProjects/Khayal3Baje/stories/bhatta/animate/bhatta2.fla");
        if !path.exists() {
            eprintln!("skipping: {} is not on this machine", path.display());
            return;
        }
        let (scene, report) = import(path).expect("bhatta2 should import");
        assert_eq!(scene.sounds().len(), 3, "{:?}", report.unsupported);
        assert!(
            !report.unsupported.iter().any(|u| u.starts_with("sound")),
            "sound should no longer be reported as unsupported: {:?}",
            report.unsupported
        );

        let mut cues = 0;
        for layer in scene.layers().iter() {
            cues += layer.frames.keyframes().iter().filter(|k| k.sound.is_some()).count();
        }
        for symbol in scene.library().iter() {
            for layer in symbol.layers.iter() {
                cues += layer.frames.keyframes().iter().filter(|k| k.sound.is_some()).count();
            }
        }
        assert_eq!(cues, 15, "15 frames name a sound across the document and its symbols");

        // Not literally the first 0.5s of each clip: two of the three are
        // dialogue "marker" recordings that lead with a real, silent pause
        // before the line starts, so that would assert on the pause rather
        // than on whether the sound decoded. Checked non-silent overall
        // instead, which still catches a decode that produced only zeroes.
        for asset in scene.sounds().iter() {
            let clip = buzz_audio::Clip::decode(&asset.data, &asset.name)
                .unwrap_or_else(|e| panic!("{} should decode: {e}", asset.name));
            let heard = clip.samples.iter().any(|s| s.abs() > 1e-4);
            assert!(heard, "{} should not be silent", asset.name);
        }
    }

    /// The zlib stream Animate cuts into `u16`-length chunks, ending at zero.
    fn chunked_zlib(raw: &[u8]) -> Vec<u8> {
        use std::io::Write;
        let mut z = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
        z.write_all(raw).unwrap();
        let zipped = z.finish().unwrap();
        let mut out = Vec::new();
        for piece in zipped.chunks(3) {
            out.extend_from_slice(&(piece.len() as u16).to_le_bytes());
            out.extend_from_slice(piece);
        }
        out.extend_from_slice(&0u16.to_le_bytes());
        out
    }

    fn dat_header(tag: u16, stride: u16, w: u16, h: u16) -> Vec<u8> {
        let mut out = Vec::new();
        for v in [tag, stride, w, h] {
            out.extend_from_slice(&v.to_le_bytes());
        }
        for v in [0u32, w as u32 * 20, 0, h as u32 * 20] {
            out.extend_from_slice(&v.to_le_bytes());
        }
        out
    }

    /// Animate's 32-bit cache: premultiplied ARGB comes out straight RGBA.
    #[test]
    fn a_lossless_dat_bitmap_decodes_to_straight_rgba() {
        let mut dat = dat_header(0x0503, 8, 2, 1);
        dat.extend_from_slice(&[1, 1]);
        // Opaque red, then half-transparent white (premultiplied to 128).
        dat.extend(chunked_zlib(&[255, 255, 0, 0, 128, 128, 128, 128]));
        let Some(DatBitmap::Pixels { width, height, rgba }) = decode_bitmap_dat(&dat) else {
            panic!("should decode");
        };
        assert_eq!((width, height), (2, 1));
        assert_eq!(rgba, vec![255, 0, 0, 255, 255, 255, 255, 128]);
    }

    /// Animate's 8-bit cache: a palette of ARGB colours and an index per pixel.
    #[test]
    fn a_palette_dat_bitmap_decodes_through_its_colours() {
        let mut dat = dat_header(0x0303, 4, 2, 2);
        dat.push(0);
        dat.extend_from_slice(&2u16.to_le_bytes());
        dat.extend_from_slice(&[255, 10, 20, 30, 255, 200, 100, 50]);
        dat.push(1);
        // Rows padded to a 4-byte stride.
        dat.extend(chunked_zlib(&[0, 1, 9, 9, 1, 0, 9, 9]));
        let Some(DatBitmap::Pixels { rgba, .. }) = decode_bitmap_dat(&dat) else {
            panic!("should decode");
        };
        assert_eq!(&rgba[..8], &[10, 20, 30, 255, 200, 100, 50, 255]);
        assert_eq!(&rgba[8..], &[200, 100, 50, 255, 10, 20, 30, 255]);
    }

    /// An instance's `<color>` is its colour effect, not decoration.
    #[test]
    fn an_instance_keeps_its_alpha_and_tint() {
        let alpha: HashMap<String, String> = [("alphaMultiplier".to_string(), "0.359375".to_string())].into();
        let fx = parse_color_effect(&alpha);
        assert_eq!(fx.multiply, [1.0, 1.0, 1.0, 0.359375]);
        assert_eq!(fx.add, [0.0; 4]);

        let tint: HashMap<String, String> = [
            ("tintMultiplier".to_string(), "0.5".to_string()),
            ("tintColor".to_string(), "#FF0000".to_string()),
        ]
        .into();
        let fx = parse_color_effect(&tint);
        assert_eq!(fx.multiply, [0.5, 0.5, 0.5, 1.0]);
        assert_eq!(fx.add, [0.5, 0.0, 0.0, 0.0]);
    }

    /// Layer parenting as Adobe Animate 2024 itself draws it — numbers read
    /// back from Animate for barish1's kurta man, by detaching his head from
    /// its torso in Animate (which keeps it where it is drawn): the joint rides
    /// the torso's whole matrix, but the head keeps its own 0.1766 scale.
    #[test]
    fn a_rigged_part_sits_where_animate_draws_it() {
        let torso_world = Affine::new([0.5723, 0.0, 0.0, 0.5723, -844.85, 314.85]);
        let head_local = Affine::new([0.176_574_7, 0.0, 0.0, 0.176_574_7, 5.55, -147.9]);
        let pivot = buzz_geom::Point::new(726.3, 1101.5);
        let world = rig_compose(torso_world, head_local, Some(pivot), Affine::IDENTITY).as_coeffs();
        assert!((world[0] - 0.1766).abs() < 1e-3 && (world[3] - 0.1766).abs() < 1e-3, "{world:?}");
        assert!((world[4] - -896.5).abs() < 0.5 && (world[5] - 147.0).abs() < 0.5, "{world:?}");

        // A wrist whose propagation carries a skew hands it on to its palm:
        // majdoorside's right palm, again as Animate draws it.
        let arm_prop = Affine::new([62990.0 / 65536.0, -17872.0 / 65536.0, 17872.0 / 65536.0, 62990.0 / 65536.0, 0.0, 0.0]);
        let wrist_prop = parse_rig_propagation(r#"{"a":67736,"b":8272,"c":8273,"d":96640,"tx":-819,"ty":-3083}"#).unwrap();
        let palm_local = Affine::new([-0.085_434, -0.086_365, -0.094_070, 0.062_134, -209.7, -277.1]);
        let palm = rig_compose(Affine::IDENTITY, palm_local, None, arm_prop * wrist_prop).as_coeffs();
        for (got, want) in palm[..4].iter().zip([-0.132, -0.106, -0.064, 0.101]) {
            assert!((got - want).abs() < 2e-3, "{palm:?}");
        }
    }

    const TWO_SCENES_WITH_A_TWEEN: &str = r##"<?xml version="1.0" encoding="UTF-8"?>
<DOMDocument xmlns="http://ns.adobe.com/xfl/2008/" width="640" height="480" frameRate="24">
  <timelines>
    <DOMTimeline name="Scene 1">
      <layers>
        <DOMLayer name="mover">
          <frames>
            <DOMFrame index="0" duration="10" tweenType="motion">
              <elements>
                <DOMSymbolInstance libraryItemName="part">
                  <matrix><Matrix tx="0" ty="0"/></matrix>
                </DOMSymbolInstance>
              </elements>
            </DOMFrame>
            <DOMFrame index="10">
              <elements>
                <DOMSymbolInstance libraryItemName="part">
                  <matrix><Matrix tx="100" ty="0"/></matrix>
                </DOMSymbolInstance>
              </elements>
            </DOMFrame>
          </frames>
        </DOMLayer>
        <DOMLayer name="grouped">
          <frames>
            <DOMFrame index="0" duration="11">
              <elements>
                <DOMGroup>
                  <matrix><Matrix a="2" d="2" tx="300" ty="200"/></matrix>
                  <members>
                    <DOMShape>
                      <matrix><Matrix a="2" d="2" tx="300" ty="200"/></matrix>
                      <fills><FillStyle index="1"><SolidColor color="#FF0000"/></FillStyle></fills>
                      <edges><Edge fillStyle1="1" edges="!0 0|200 0!200 0|200 200!200 200|0 200!0 200|0 0"/></edges>
                    </DOMShape>
                  </members>
                </DOMGroup>
              </elements>
            </DOMFrame>
          </frames>
        </DOMLayer>
      </layers>
    </DOMTimeline>
    <DOMTimeline name="Village">
      <layers>
        <DOMLayer name="only in scene two">
          <frames>
            <DOMFrame index="0" duration="3">
              <elements>
                <DOMSymbolInstance libraryItemName="part">
                  <matrix><Matrix tx="7" ty="7"/></matrix>
                </DOMSymbolInstance>
              </elements>
            </DOMFrame>
          </frames>
        </DOMLayer>
      </layers>
    </DOMTimeline>
  </timelines>
</DOMDocument>"##;

    fn two_scenes() -> (Scene, ImportReport) {
        build(TWO_SCENES_WITH_A_TWEEN, &[("LIBRARY/part.xml".to_string(), RIG_PART.to_string())]).unwrap()
    }

    /// An imported classic tween moves between its keyframes, instead of
    /// holding the first pose and jumping at the second.
    #[test]
    fn an_imported_classic_tween_moves_between_its_keyframes() {
        let (scene, _) = two_scenes();
        let mover = scene.layers().iter().find(|l| l.name == "mover").unwrap();
        let x = |f: u32| mover.frames.resolved_at(f).iter().next().unwrap().transform.as_coeffs()[4];
        assert!((x(0) - 0.0).abs() < 1e-9 && (x(10) - 100.0).abs() < 1e-9);
        assert!((x(5) - 50.0).abs() < 1e-6, "halfway should be at 50, got {}", x(5));
    }

    /// A shape inside a group is placed by its own matrix, applied once.
    #[test]
    fn a_grouped_shape_is_placed_by_its_own_matrix() {
        let (scene, _) = two_scenes();
        let grouped = scene.layers().iter().find(|l| l.name == "grouped").unwrap();
        let shape = &grouped.objects_at(0)[0];
        let placed = shape.transform * buzz_geom::Point::new(10.0, 10.0);
        // Edge (200, 200) twips = (10, 10) px, then scale 2 and move (300, 200).
        assert!((placed.x - 320.0).abs() < 1e-6 && (placed.y - 220.0).abs() < 1e-6, "{placed:?}");
    }

    /// Each of Animate's scenes is its own timeline, not merged into the first.
    #[test]
    fn each_animate_scene_imports_as_its_own_scene() {
        let (scene, report) = two_scenes();
        assert!(scene.layers().iter().all(|l| l.name != "only in scene two"));
        assert_eq!(report.scene_name.as_deref(), Some("Scene 1"));
        assert_eq!(report.more_scenes.len(), 1);
        let (name, village) = &report.more_scenes[0];
        assert_eq!(name, "Village");
        assert!(village.layers().iter().any(|l| l.name == "only in scene two"));
        assert!(village.layers().iter().all(|l| l.name != "mover"));
    }
}
