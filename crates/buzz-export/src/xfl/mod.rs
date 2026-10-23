//! Writing a document back out as an Adobe Animate `.fla`.
//!
//! # Why this exists
//!
//! Import was one-way. A film could come *in* from Animate and never go back,
//! which makes this program somewhere work goes to be finished rather than a
//! place in a pipeline — and an animator with a studio, a client or a shelf of
//! Animate tooling cannot use a tool they cannot hand a file back from.
//!
//! # What a `.fla` is
//!
//! A ZIP of XML, which Adobe call XFL. Inside:
//!
//! * `DOMDocument.xml` — the stage, the library's index, and the timelines.
//! * `LIBRARY/<name>.xml` — one `DOMSymbolItem` per symbol.
//! * `META-INF/metadata.xml` — a marker Animate writes; harmless and expected.
//!
//! The geometry inside is the Edge format, which [`edge`] writes and
//! `buzz_import_xfl::edge` reads. The two are inverses and are tested against
//! each other, because a `.fla` that Animate opens *empty* is worse than one
//! it refuses: the refusal at least says something is wrong.
//!
//! # What travels, and what does not
//!
//! Travelling: the stage (size, colour, frame rate), layers with their names,
//! colours, visibility, locking and folders, **layer parenting**, keyframes
//! and their spans, classic and shape tweens with their eases, sounds on
//! keyframes (the clips in the library), shapes with solid fills and strokes,
//! groups, and symbols with their instances and transforms.
//!
//! Not yet travelling, and deliberately named rather than silently dropped —
//! [`FlaReport::skipped`] carries them out to the user: gradients, bitmaps,
//! filters, blend modes, the camera, and rigged or warped artwork. Those are written as their resolved artwork where they have any,
//! so the picture is right even where the editability is not.

mod edge;

use std::collections::BTreeMap;
use std::io::Write as _;
use std::path::Path;

use buzz_geom::Affine;
use buzz_scene::{Layer, LayerKind, Object, ObjectKind, Scene, Symbol, SymbolKind};
use peniko::Color;

/// What came of writing a `.fla`.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct FlaReport {
    /// Symbols written to `LIBRARY/`.
    pub symbols: usize,
    /// Objects written across every timeline.
    pub objects: usize,
    /// Things this writer cannot yet express, each named once.
    ///
    /// Reported rather than logged: an export that quietly drops a document's
    /// gradients is a trap, and the only honest version of a partial exporter
    /// is one that says what it left behind.
    pub skipped: Vec<String>,
}

impl FlaReport {
    fn skip(&mut self, what: &str) {
        let text = what.to_string();
        if !self.skipped.contains(&text) {
            self.skipped.push(text);
        }
    }

    /// A sentence for the status bar.
    pub fn summary(&self) -> String {
        let mut text = format!(
            "Exported {} object{} and {} symbol{}",
            self.objects,
            if self.objects == 1 { "" } else { "s" },
            self.symbols,
            if self.symbols == 1 { "" } else { "s" }
        );
        if !self.skipped.is_empty() {
            text.push_str(&format!(
                " \u{2014} not carried: {}",
                self.skipped.join(", ")
            ));
        }
        text
    }
}

/// Why a `.fla` could not be written.
#[derive(Debug, thiserror::Error)]
pub enum FlaError {
    #[error("writing {path}: {source}")]
    Io {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("packing the .fla: {0}")]
    Zip(#[from] zip::result::ZipError),
}

/// Write `scene` to `path` as a `.fla`.
pub fn export_fla(scene: &Scene, path: impl AsRef<Path>) -> Result<FlaReport, FlaError> {
    let path = path.as_ref();
    let (bytes, report) = fla_bytes(scene)?;
    std::fs::write(path, bytes).map_err(|source| FlaError::Io {
        path: path.display().to_string(),
        source,
    })?;
    Ok(report)
}

/// The `.fla` as bytes, for callers that are not writing to a file — the
/// round-trip tests, and anything that wants to hand it straight to a stream.
pub fn fla_bytes(scene: &Scene) -> Result<(Vec<u8>, FlaReport), FlaError> {
    let mut report = FlaReport::default();
    let mut buffer = Vec::new();
    {
        let mut zip = zip::ZipWriter::new(std::io::Cursor::new(&mut buffer));
        let options: zip::write::FileOptions<'_, ()> =
            zip::write::FileOptions::default().compression_method(zip::CompressionMethod::Deflated);

        // Animate looks for this first; without it the file is not recognised
        // as an XFL package at all.
        zip.start_file("META-INF/metadata.xml", options)?;
        zip.write_all(METADATA.as_bytes()).map_err(io("metadata"))?;

        // **Each symbol at its library path** (folder and name), unique in
        // the package: Animate files a symbol by that path, and two symbols
        // that share a name in different folders must not share an entry.
        let paths = library_paths(scene);
        for symbol in scene.library().iter() {
            zip.start_file(format!("LIBRARY/{}.xml", paths.of(symbol.id)), options)?;
            let xml = symbol_xml(scene, &paths, symbol, &mut report);
            zip.write_all(xml.as_bytes()).map_err(io("a symbol"))?;
            report.symbols += 1;
        }

        // **Each sound as its file, under `LIBRARY/`** at the item's `href`
        // (see `sound_media`), where Animate imports it from when it opens
        // the document.
        for sound in scene.sounds().iter() {
            let (Some(href), Some((bytes, _))) = (paths.sound_href(sound.id), paths.media.get(&sound.id)) else {
                continue;
            };
            zip.start_file(format!("LIBRARY/{href}"), options)?;
            zip.write_all(bytes).map_err(io("a sound"))?;
        }

        zip.start_file("DOMDocument.xml", options)?;
        let xml = document_xml(scene, &paths, &mut report);
        zip.write_all(xml.as_bytes())
            .map_err(io("DOMDocument.xml"))?;

        zip.finish()?;
    }
    Ok((buffer, report))
}

fn io(what: &str) -> impl Fn(std::io::Error) -> FlaError + '_ {
    move |source| FlaError::Io {
        path: what.to_string(),
        source,
    }
}

/// The XFL document type, which is the same in every `.fla` Adobe writes.
/// Animate uses it to recognise the package; a file without it is not one.
const FILETYPE_GUID: &str = "DD0DDBBF-5BEF-45B2-9F24-A3048D2A676F";

/// The format version, as Animate 2023 writes it. Claiming a much older XFL
/// would invite a reader to expect an older layout than this one.
const XFL_VERSION: &str = "23.0";
const MAJOR_VERSION: &str = "23";

#[cfg(target_os = "windows")]
const PLATFORM: &str = "Windows";
#[cfg(not(target_os = "windows"))]
const PLATFORM: &str = "Macintosh";

/// A per-file identifier, **derived from the document** rather than random.
///
/// Animate's is arbitrary, so anything of the right shape will do — and taking
/// it from the content means writing the same document twice gives the same
/// bytes, which is what makes an export diffable and a test repeatable.
fn file_guid(scene: &Scene) -> String {
    use std::hash::{Hash as _, Hasher as _};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    scene.stage().size.width.to_bits().hash(&mut hasher);
    scene.stage().size.height.to_bits().hash(&mut hasher);
    scene.layers().len().hash(&mut hasher);
    for symbol in scene.library().iter() {
        symbol.name.hash(&mut hasher);
    }
    let a = hasher.finish();
    // The 32 hex digits Animate writes, with no hyphens.
    format!("{a:016X}{:016X}", a.rotate_left(17) ^ 0x9E37_79B9_7F4A_7C15)
}

/// A library item's id in the two-part hex form Animate uses.
fn item_id(id: u64) -> String {
    format!("{:08x}-{:08x}", (id >> 32) as u32 | 0x6000_0000, id as u32)
}

/// A stable 64-bit hash of a name, for ids that must not change between
/// exports of the same document.
fn name_hash(name: &str) -> u64 {
    use std::hash::{Hash as _, Hasher as _};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    name.hash(&mut hasher);
    hasher.finish()
}

/// Every symbol's **library path** (`folder/sub/name`: the name Animate files
/// it under, references it by and stores it at in `LIBRARY/`) and the folders
/// those paths need.
///
/// Unique, ignoring case (Animate's library and the Windows file system both
/// ignore it): two symbols may share a name in different folders here, and
/// writing both to one `LIBRARY/` entry made the package unwritable.
struct LibraryPaths {
    by_symbol: BTreeMap<buzz_scene::SymbolId, String>,
    /// Every folder, parents before children.
    folders: Vec<String>,
    /// Every sound's library name: the same namespace as the symbols, so
    /// unique against them too.
    by_sound: BTreeMap<buzz_scene::SoundId, String>,
    /// Each sound's file as it goes into `LIBRARY/`, and its extension; a
    /// sound missing here is one Animate could not take (reported).
    media: BTreeMap<buzz_scene::SoundId, (Vec<u8>, &'static str)>,
}

impl LibraryPaths {
    fn of(&self, id: buzz_scene::SymbolId) -> &str {
        self.by_symbol.get(&id).map(String::as_str).unwrap_or("Symbol")
    }

    /// The sound's library name, if its file is written.
    fn sound(&self, id: buzz_scene::SoundId) -> Option<&str> {
        self.media.contains_key(&id).then(|| self.by_sound.get(&id).map(String::as_str))?
    }

    /// The sound's file under `LIBRARY/`: its name plus the extension of
    /// what is written.
    fn sound_href(&self, id: buzz_scene::SoundId) -> Option<String> {
        let name = self.sound(id)?;
        let (_, ext) = self.media.get(&id)?;
        Some(format!("{name}.{ext}"))
    }
}

fn library_paths(scene: &Scene) -> LibraryPaths {
    let mut taken = std::collections::BTreeSet::new();
    let mut by_symbol = BTreeMap::new();
    let mut folders = std::collections::BTreeSet::new();
    let folder_path = |folder: &str| -> Vec<String> {
        folder
            .split('/')
            .filter(|segment| !segment.trim().is_empty())
            .map(file_name)
            .collect()
    };
    for folder in scene.library().folders() {
        let segments = folder_path(folder);
        for end in 1..=segments.len() {
            folders.insert(segments[..end].join("/"));
        }
    }
    for symbol in scene.library().iter() {
        let segments = symbol.folder.as_deref().map(folder_path).unwrap_or_default();
        for end in 1..=segments.len() {
            folders.insert(segments[..end].join("/"));
        }
    }
    let folders_lower: std::collections::BTreeSet<String> =
        folders.iter().map(|f| f.to_lowercase()).collect();
    for symbol in scene.library().iter() {
        let segments = symbol.folder.as_deref().map(folder_path).unwrap_or_default();
        let prefix: String = segments.iter().map(|s| format!("{s}/")).collect();
        let base = file_name(&symbol.name);
        let mut path = format!("{prefix}{base}");
        let mut n = 2;
        while taken.contains(&path.to_lowercase()) || folders_lower.contains(&path.to_lowercase()) {
            path = format!("{prefix}{base} ({n})");
            n += 1;
        }
        taken.insert(path.to_lowercase());
        by_symbol.insert(symbol.id, path);
    }
    let mut by_sound = BTreeMap::new();
    let mut media = BTreeMap::new();
    for sound in scene.sounds().iter() {
        let base = file_name(&sound.name);
        let mut path = base.clone();
        let mut n = 2;
        while taken.contains(&path.to_lowercase()) || folders_lower.contains(&path.to_lowercase()) {
            path = format!("{base} ({n})");
            n += 1;
        }
        taken.insert(path.to_lowercase());
        by_sound.insert(sound.id, path);
        if let Some(file) = sound_media(sound) {
            media.insert(sound.id, file);
        }
    }
    LibraryPaths {
        by_symbol,
        folders: folders.into_iter().collect(),
        by_sound,
        media,
    }
}

const METADATA: &str = concat!(
    r#"<?xml version="1.0" encoding="UTF-8"?>"#,
    "\n",
    r#"<metadata xmlns="http://ns.adobe.com/xfl/2008/"/>"#,
    "\n"
);

// ---- the document -----------------------------------------------------------

fn document_xml(scene: &Scene, paths: &LibraryPaths, report: &mut FlaReport) -> String {
    let stage = scene.stage();
    let mut out = String::new();
    out.push_str(r#"<?xml version="1.0" encoding="UTF-8"?>"#);
    out.push('\n');
    // **Written to the shape Animate writes.**
    //
    // Compared attribute by attribute against a document Adobe saved (see
    // `tests::compare_against_a_real_fla`, which is how this list was arrived
    // at rather than guessed). The identifying attributes are not decoration:
    // Animate uses `filetypeGUID` to recognise the package as an XFL document
    // at all, and a reader that does not find what it expects in the header
    // has no reason to look any further.
    out.push_str(&format!(
        concat!(
            r#"<DOMDocument xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance" "#,
            r#"xmlns="http://ns.adobe.com/xfl/2008/" backgroundColor="{}" width="{}" "#,
            r#"height="{}" frameRate="{}" currentTimeline="1" xflVersion="{}" "#,
            r#"creatorInfo="BuzzAnimate" platform="{}" versionInfo="Saved by BuzzAnimate" "#,
            r#"majorVersion="{}" buildNumber="1" nextSceneIdentifier="2" "#,
            r#"filetypeGUID="{}" fileGUID="{}">"#,
        ),
        hex(stage.background),
        number(stage.size.width),
        number(stage.size.height),
        number(stage.frame_rate),
        XFL_VERSION,
        PLATFORM,
        MAJOR_VERSION,
        FILETYPE_GUID,
        file_guid(scene),
    ));
    out.push('\n');

    // The library's index. Animate resolves `href` against `LIBRARY/`.
    //
    // Empty `<folders/>` and `<media/>` are not written: Adobe leaves them out
    // when there is nothing in them, and matching that keeps the file to the
    // shape a reader has actually been given before.
    // The library's folders, parents before children, as Animate lists them.
    if !paths.folders.is_empty() {
        out.push_str("  <folders>\n");
        for folder in &paths.folders {
            out.push_str(&format!(
                "    <DOMFolderItem name=\"{}\" itemID=\"{}\"/>\n",
                escape(folder),
                item_id(name_hash(folder)),
            ));
        }
        out.push_str("  </folders>\n");
    }
    // **The sounds.** Each `DOMSoundItem` names its file under `LIBRARY/`
    // by `href` and nothing in `bin/`: Animate imports the file from there
    // as it opens the document, MP3 natively, and writes its own `bin/`
    // cache when the document is saved. Measured in Animate 2024 (X3): an
    // MP3 put straight into `bin/` without the waveform cache Animate
    // appends to its own was read as raw PCM (noise for three minutes);
    // the same file under `LIBRARY/` opened, played and exported back
    // byte for byte. A WAV opened the same from either place, and one at a
    // rate Animate does not store (16 kHz) was converted by Animate itself.
    let sounds: Vec<_> = scene
        .sounds()
        .iter()
        .filter_map(|sound| Some((sound, paths.sound(sound.id)?, paths.sound_href(sound.id)?)))
        .collect();
    if !sounds.is_empty() {
        out.push_str("  <media>\n");
        for (sound, name, href) in sounds {
            let mut attrs = format!(
                "name=\"{}\" itemID=\"{}\" href=\"{}\"",
                escape(name),
                item_id(sound.id.0),
                escape(&href),
            );
            // Animate's own description of the clip, for the rates it
            // stores; for any other it works them out on import.
            if let Some(format) = sound_format(sound) {
                attrs.push_str(&format!(" format=\"{format}\" sampleCount=\"{}\"", sound.length));
            }
            out.push_str(&format!("    <DOMSoundItem {attrs}/>\n"));
        }
        out.push_str("  </media>\n");
    }
    if scene.sounds().iter().any(|sound| paths.sound(sound.id).is_none()) {
        report.skip("sounds that are neither WAV nor MP3");
    }
    if scene.library().is_empty() {
        out.push_str("  <symbols/>\n");
    } else {
        out.push_str("  <symbols>\n");
        for symbol in scene.library().iter() {
            out.push_str(&format!(
                "    <Include href=\"{}.xml\" itemIcon=\"1\" loadImmediate=\"false\" itemID=\"{}\"/>\n",
                escape(paths.of(symbol.id)),
                item_id(symbol.id.0),
            ));
        }
        out.push_str("  </symbols>\n");
    }

    out.push_str("  <timelines>\n");
    out.push_str(&timeline_xml(
        scene,
        paths,
        "Scene 1",
        scene.stage_layers(),
        report,
        4,
    ));
    out.push_str("  </timelines>\n");
    // Animate writes this even with nothing in it.
    out.push_str("  <scripts/>\n");
    out.push_str("</DOMDocument>\n");
    out
}

fn symbol_xml(scene: &Scene, paths: &LibraryPaths, symbol: &Symbol, report: &mut FlaReport) -> String {
    let kind = match symbol.kind {
        SymbolKind::MovieClip => "movie clip",
        SymbolKind::Button => "button",
        SymbolKind::Graphic => "graphic",
    };
    let mut out = String::new();
    out.push_str(r#"<?xml version="1.0" encoding="UTF-8"?>"#);
    out.push('\n');
    out.push_str(&format!(
        r#"<DOMSymbolItem xmlns="http://ns.adobe.com/xfl/2008/" name="{}" itemID="{}" symbolType="{kind}">"#,
        escape(paths.of(symbol.id)),
        item_id(symbol.id.0),
    ));
    out.push('\n');
    out.push_str("  <timeline>\n");
    out.push_str(&timeline_xml(
        scene,
        paths,
        &symbol.name,
        &symbol.layers,
        report,
        4,
    ));
    out.push_str("  </timeline>\n");
    out.push_str("</DOMSymbolItem>\n");
    out
}

// ---- timelines ---------------------------------------------------------------

fn timeline_xml(
    scene: &Scene,
    paths: &LibraryPaths,
    name: &str,
    layers: &buzz_scene::LayerStack,
    report: &mut FlaReport,
    indent: usize,
) -> String {
    let pad = " ".repeat(indent);
    let mut out = String::new();
    // **Depth has to be switched on for Animate to honour it.** A layer's
    // depth is only read when the timeline says the document uses depth, so
    // exporting depths without this would put every layer back on the focal
    // plane — a picture quietly flattened rather than one visibly wrong.
    let depth_attr = if layers.iter().any(|l| l.depth != 0.0) {
        " layerDepthEnabled=\"true\""
    } else {
        ""
    };
    out.push_str(&format!(
        "{pad}<DOMTimeline name=\"{}\"{depth_attr}>\n{pad}  <layers>\n",
        escape(name)
    ));

    // Animate's layer order is top-first, which is ours: `LayerStack::iter`
    // already yields the timeline from the top down.
    let ordered: Vec<&std::sync::Arc<Layer>> = layers.iter().collect();
    // `parentLayerIndex` and the rigging links are written by *index*, so the
    // positions have to be known before any layer is written.
    let index_of: BTreeMap<buzz_scene::LayerId, usize> =
        ordered.iter().enumerate().map(|(i, l)| (l.id, i)).collect();

    // Which layers something hangs off, so only those are marked as rig
    // parents.
    let parents: std::collections::BTreeSet<buzz_scene::LayerId> =
        ordered.iter().filter_map(|l| l.follows).collect();
    // **Rigging indices are their own numbering**, not layer positions:
    // Animate numbers its rig parents 0, 1, 2... and a child's frame names its
    // parent by that number. Dense, in timeline order, as Animate writes them.
    let rig_index: BTreeMap<buzz_scene::LayerId, usize> = ordered
        .iter()
        .filter(|l| parents.contains(&l.id))
        .enumerate()
        .map(|(i, l)| (l.id, i))
        .collect();
    // **Which mask (or guide) governs each masked (or guided) layer.**
    // Masking is positional here (a mask owns the unbroken run of masked
    // layers below it, see `LayerStack::mask_groups`) but explicit in a
    // `.fla`: a masked layer is an ordinary layer whose `parentLayerIndex`
    // names its mask. `layerType="masked"` on its own is not something Animate
    // reads, and the layer opened unmasked.
    let mut governor: BTreeMap<buzz_scene::LayerId, usize> = BTreeMap::new();
    let mut mask_above: Option<usize> = None;
    let mut guide_above: Option<usize> = None;
    for (index, layer) in ordered.iter().enumerate() {
        match layer.kind {
            LayerKind::Mask | LayerKind::InverseMask => {
                mask_above = Some(index);
                guide_above = None;
            }
            LayerKind::Guide => {
                guide_above = Some(index);
                mask_above = None;
            }
            LayerKind::Masked => {
                if let Some(mask) = mask_above {
                    governor.insert(layer.id, mask);
                }
                guide_above = None;
            }
            LayerKind::Guided => {
                if let Some(guide) = guide_above {
                    governor.insert(layer.id, guide);
                }
                mask_above = None;
            }
            LayerKind::Normal | LayerKind::Folder => {
                mask_above = None;
                guide_above = None;
            }
        }
    }

    for layer in ordered.iter() {
        out.push_str(&layer_xml(
            scene,
            paths,
            layers,
            layer,
            &index_of,
            &rig_index,
            governor.get(&layer.id).copied(),
            report,
            indent + 4,
        ));
    }

    out.push_str(&format!("{pad}  </layers>\n{pad}</DOMTimeline>\n"));
    out
}

#[allow(clippy::too_many_arguments)]
fn layer_xml(
    scene: &Scene,
    paths: &LibraryPaths,
    layers: &buzz_scene::LayerStack,
    layer: &Layer,
    index_of: &BTreeMap<buzz_scene::LayerId, usize>,
    rig_index: &BTreeMap<buzz_scene::LayerId, usize>,
    governor: Option<usize>,
    report: &mut FlaReport,
    indent: usize,
) -> String {
    let pad = " ".repeat(indent);
    let mut attrs = format!(
        "name=\"{}\" color=\"{}\"",
        escape(&layer.name),
        hex(layer.color)
    );
    if !layer.visible {
        attrs.push_str(" visible=\"false\"");
    }
    if layer.locked {
        attrs.push_str(" locked=\"true\"");
    }
    if layer.outline {
        attrs.push_str(" outline=\"true\"");
    }
    match layer.kind {
        LayerKind::Folder => attrs.push_str(" layerType=\"folder\""),
        LayerKind::Mask => attrs.push_str(" layerType=\"mask\""),
        // A masked or guided layer is an ordinary layer pointing at its mask
        // or guide (below); one with nothing above to govern it is ordinary.
        LayerKind::Masked | LayerKind::Guided => {}
        LayerKind::Guide => attrs.push_str(" layerType=\"guide\""),
        LayerKind::InverseMask => {
            // Animate has no inverse mask; it comes back as an ordinary one
            // rather than as nothing.
            attrs.push_str(" layerType=\"mask\"");
            report.skip("inverse masks (written as ordinary masks)");
        }
        LayerKind::Normal => {}
    }
    // The folder a layer sits in — Animate's own `parentLayerIndex`, which is
    // a different thing from layer parenting. A masked or guided layer's
    // points at its mask or guide instead, which sits in the same folder.
    if let Some(at) = governor {
        attrs.push_str(&format!(" parentLayerIndex=\"{at}\""));
    } else if let Some(parent) = layer.parent
        && let Some(at) = index_of.get(&parent)
    {
        attrs.push_str(&format!(" parentLayerIndex=\"{at}\""));
    }
    // **Layer parenting**, which Animate writes as a rigging index on the
    // parent and a reference to it on the child's frames. Written the same way
    // round, so a character rigged here arrives in Animate still rigged.
    //
    // Only on a layer something actually follows. Animate puts it on rig
    // parents, not on every layer, and marking the whole timeline as rig nodes
    // would describe a character that is not there.
    let own_rig_index = rig_index.get(&layer.id).copied();
    if let Some(index) = own_rig_index {
        attrs.push_str(&format!(" layerRiggingIndex=\"{index}\""));
    }
    let follows = layer
        .follows
        .and_then(|id| rig_index.get(&id).map(|index| (id, *index)));

    // A folder holds layers, not frames: Animate writes it with none, and
    // drops any it is given.
    if layer.kind == LayerKind::Folder {
        return format!("{pad}<DOMLayer {attrs}/>\n");
    }

    let mut out = format!("{pad}<DOMLayer {attrs}>\n{pad}  <frames>\n");
    for keyframe in layer.frames.keyframes() {
        out.push_str(&frame_xml(
            scene,
            paths,
            layers,
            layer,
            keyframe,
            follows,
            own_rig_index.is_some(),
            report,
            indent + 4,
        ));
    }
    out.push_str(&format!("{pad}  </frames>\n{pad}</DOMLayer>\n"));
    out
}

#[allow(clippy::too_many_arguments)]
fn frame_xml(
    scene: &Scene,
    paths: &LibraryPaths,
    layers: &buzz_scene::LayerStack,
    layer: &Layer,
    keyframe: &buzz_scene::Keyframe,
    follows: Option<(buzz_scene::LayerId, usize)>,
    is_rig_parent: bool,
    report: &mut FlaReport,
    indent: usize,
) -> String {
    let pad = " ".repeat(indent);
    // How long the keyframe holds: up to the next one, or to the end of the
    // layer's span for the last. Animate stores the span, not the end.
    let next = layer
        .frames
        .keyframes()
        .iter()
        .map(|k| k.start)
        .filter(|start| *start > keyframe.start)
        .min()
        .unwrap_or_else(|| layer.frames.length());
    let duration = next.saturating_sub(keyframe.start).max(1);
    // A duration of one is left off, as Animate leaves it off.
    let mut attrs = format!("index=\"{}\"", keyframe.start);
    if duration > 1 {
        attrs.push_str(&format!(" duration=\"{duration}\""));
    }
    // **The tween**, as Animate writes a classic or shape tween on the frame
    // that starts it. Only where there is a next keyframe to tween to: on
    // the last one ours does nothing and Animate draws a broken tween.
    let has_next = layer.frames.keyframes().iter().any(|k| k.start > keyframe.start);
    let tween = has_next.then_some(keyframe.tween).filter(|t| t.is_active());
    let tweens_xml = match tween {
        Some(tween) => tween_attrs(&tween, keyframe.ease_curve.as_ref(), &mut attrs, indent + 2),
        None => {
            // `keyMode` is how Animate records what kind of keyframe this
            // is; 9728 is the ordinary one.
            attrs.push_str(" keyMode=\"9728\"");
            String::new()
        }
    };
    if let Some(label) = &keyframe.label {
        attrs.push_str(&format!(" name=\"{}\"", escape(label)));
    }
    // **Layer parenting, in Animate's convention.** Here a follower's
    // transform is absolute and only its parent's *motion* away from its rest
    // pose is inherited (`LayerStack::inherited_transform`); in a `.fla` a
    // rigged child's matrix is *relative to its parent*. Measured in Animate
    // 2024 (docs/FLA_EXPORT_PLAN.md X0): Animate draws a child at
    // `T(parent · (child · pivot)) · propagation · linear(child) · T(-pivot)`,
    // where `propagation` is the product of the `rigPropagationMatrix` values
    // down the chain, read from the file as written. So each rig parent
    // writes the linear part of its own written matrix as its propagation;
    // the chain's product is then the linear part of the parent's world, and
    // Animate's draw reduces to `parent_world · child`, exactly. The child is
    // written as `parent_world⁻¹ · drawn`: where the parent stands at this
    // key, and what the child is drawn as.
    //
    // Writing our absolute transforms as Animate's relative ones (as this
    // did) applied every parent twice and scattered rigged characters across
    // the stage.
    let at = keyframe.start;
    let world_of = |id: buzz_scene::LayerId| -> Option<Affine> {
        let first = layers.resolved_at_rigged(id, at).iter().next().map(|o| o.transform)?;
        Some(layers.inherited_transform(id, at) * first)
    };
    let mut relative = Affine::IDENTITY;
    if let Some((parent, parent_index)) = follows {
        let inherited = layers.inherited_transform(layer.id, at);
        match world_of(parent) {
            Some(parent_world) if parent_world.determinant().abs() > 1e-12 => {
                relative = parent_world.inverse() * inherited;
                attrs.push_str(&format!(" parentLayerIndex=\"{parent_index}\""));
            }
            // Nothing on the parent here: drawn where it is drawn. Animate
            // draws a child of an *empty* parent as if the parent were the
            // identity (measured, X1), so the link is still written, which
            // keeps the rig; only a parent that itself follows another is
            // left unlinked, as what Animate passes down through an empty
            // layer is unmeasured.
            None => {
                relative = inherited;
                let parent_follows = layers.iter().find(|l| l.id == parent).and_then(|l| l.follows);
                if parent_follows.is_none() {
                    attrs.push_str(&format!(" parentLayerIndex=\"{parent_index}\""));
                }
            }
            // A parent scaled to nothing: hanging off it would collapse the
            // child, so it is drawn where it is drawn, unlinked.
            Some(_) => relative = inherited,
        }
    }
    if is_rig_parent && let Some(first) = keyframe.objects.first() {
        let [a, b, c, d, _, _] = (relative * first.transform).as_coeffs();
        let fixed = |v: f64| (v * 65536.0).round() as i64;
        attrs.push_str(&format!(
            " rigPropagationMatrix='{{\"a\":{},\"b\":{},\"c\":{},\"d\":{},\"tx\":0,\"ty\":0}}'",
            fixed(a),
            fixed(b),
            fixed(c),
            fixed(d),
        ));
    }
    // **The sound starting here.** A Master Audio guide layer's is never
    // exported (nor reported as skipped: it was never meant to be heard).
    // The stage's Master Audio layer is an ordinary layer whose one key
    // streams the master clip, so it arrives as Animate's stream-synced
    // sound layer.
    let mut envelope_xml = String::new();
    if let Some(sound) = keyframe.sound.filter(|_| !layer.audio_guide) {
        match paths.sound(sound.sound) {
            Some(name) => envelope_xml = sound_attrs(&sound, name, &mut attrs, indent + 2),
            None => report.skip("sounds that are neither WAV nor MP3"),
        }
    }

    let mut out = format!("{pad}<DOMFrame {attrs}>\n{tweens_xml}{envelope_xml}{pad}  <elements>\n");
    let separate = keeps_shapes_apart(&keyframe.objects);
    for object in keyframe.objects.iter() {
        if relative == Affine::IDENTITY {
            out.push_str(&object_xml(scene, paths, object, separate, report, indent + 4));
        } else {
            let mut moved: Object = (**object).clone();
            moved.transform = relative * moved.transform;
            out.push_str(&object_xml(scene, paths, &moved, separate, report, indent + 4));
        }
    }
    out.push_str(&format!("{pad}  </elements>\n{pad}</DOMFrame>\n"));
    out
}

/// A tween's attributes onto a frame's, and its `<tweens>` element (custom
/// eases) if it has one. The inverse of `buzz_import_xfl`'s reading:
///
/// * classic and motion tweens are Animate's classic tween, `tweenType=
///   "motion"` (ours interpolate the same properties; the difference in
///   Animate is the authoring model), with whole extra turns as
///   `motionTweenRotate` + `motionTweenRotateTimes` and orient-to-path;
///   shape tweens are `tweenType="shape"`;
/// * the ease slider is `acceleration`, which is the slider **negated**:
///   Animate's own ease is -100 in .. 100 out (as ours), and the file's
///   `acceleration` is how much the tween speeds up, 100 for a full ease
///   in. Measured in Animate 2024 (X2): `acceleration="100"` opened as an
///   ease of -100 (JSFL `tweenEasing`) and drew a quarter of the way along
///   at the span's middle;
/// * a custom curve is `hasCustomEase` and a `<CustomEase>` of Animate's
///   `1 + 3n` points per property: the keyframe's `ease_curve` where the
///   Motion Editor drew one, else the tween's own cubic Bézier.
fn tween_attrs(
    tween: &buzz_scene::Tween,
    curve: Option<&buzz_scene::EasingSet>,
    attrs: &mut String,
    indent: usize,
) -> String {
    use buzz_scene::{Easing, TweenKind};
    let key_mode = if tween.kind == TweenKind::Shape { " keyMode=\"17922\"" } else { " keyMode=\"22017\"" };
    match tween.kind {
        TweenKind::Shape => attrs.push_str(" tweenType=\"shape\""),
        _ => {
            attrs.push_str(" tweenType=\"motion\"");
            if tween.extra_rotations != 0 {
                let way = if tween.extra_rotations > 0 { "clockwise" } else { "counter-clockwise" };
                attrs.push_str(&format!(
                    " motionTweenRotate=\"{way}\" motionTweenRotateTimes=\"{}\"",
                    tween.extra_rotations.unsigned_abs()
                ));
            }
            if tween.orient_to_path {
                attrs.push_str(" motionTweenOrientToPath=\"true\"");
            }
            attrs.push_str(" motionTweenSnap=\"true\"");
        }
    }
    let own = match tween.easing {
        Easing::Linear => None,
        Easing::Strength(amount) => {
            let amount = amount.round().clamp(-100.0, 100.0);
            if amount != 0.0 {
                attrs.push_str(&format!(" acceleration=\"{}\"", -amount as i64));
            }
            None
        }
        Easing::CubicBezier { .. } => Some(easing_points(tween.easing)),
    };
    let drawn = curve.filter(|c| !c.is_empty());
    let all: Option<Vec<buzz_geom::Point>> = drawn
        .and_then(|c| c.all.as_deref().map(<[_]>::to_vec))
        .or(own);
    let split = drawn.is_some_and(|c| c.position.is_some() || c.rotation.is_some() || c.scale.is_some());
    if all.is_none() && !split {
        attrs.push_str(key_mode);
        return String::new();
    }
    attrs.push_str(" hasCustomEase=\"true\"");
    // One curve per property, when the Motion Editor eased them apart: a
    // property it left alone eases by the whole tween's curve.
    let whole = all.unwrap_or_else(|| easing_points(tween.easing));
    let mut targets: Vec<(&str, Vec<buzz_geom::Point>)> = Vec::new();
    match drawn.filter(|_| split) {
        Some(drawn) => {
            attrs.push_str(" useSingleEaseCurve=\"false\"");
            let own_or_whole = |points: Option<&std::sync::Arc<[buzz_geom::Point]>>| {
                points.map(|p| p.to_vec()).unwrap_or_else(|| whole.clone())
            };
            targets.push(("position", own_or_whole(drawn.position.as_ref())));
            targets.push(("rotation", own_or_whole(drawn.rotation.as_ref())));
            targets.push(("scale", own_or_whole(drawn.scale.as_ref())));
            targets.push(("color", whole.clone()));
            targets.push(("filters", whole));
        }
        None => targets.push(("all", whole)),
    }
    attrs.push_str(key_mode);
    let pad = " ".repeat(indent);
    let mut out = format!("{pad}<tweens>\n");
    for (target, points) in targets {
        out.push_str(&format!("{pad}  <CustomEase target=\"{target}\">\n"));
        for point in points {
            if point.x == 0.0 && point.y == 0.0 {
                out.push_str(&format!("{pad}    <Point/>\n"));
            } else {
                out.push_str(&format!("{pad}    <Point x=\"{}\" y=\"{}\"/>\n", number(point.x), number(point.y)));
            }
        }
        out.push_str(&format!("{pad}  </CustomEase>\n"));
    }
    out.push_str(&format!("{pad}</tweens>\n"));
    out
}

/// An ease as Animate's custom-ease points (`1 + 3n`, `(0,0)` to `(1,1)`).
/// A Bézier is itself; the slider's curve (or a straight line) is fitted
/// with four cubic segments through its values and slopes, for a property
/// the Motion Editor left to the whole tween.
fn easing_points(easing: buzz_scene::Easing) -> Vec<buzz_geom::Point> {
    use buzz_geom::Point;
    match easing {
        buzz_scene::Easing::CubicBezier { x1, y1, x2, y2 } => {
            vec![Point::new(0.0, 0.0), Point::new(x1, y1), Point::new(x2, y2), Point::new(1.0, 1.0)]
        }
        easing => {
            const SEGMENTS: usize = 4;
            let slope = |t: f64| {
                let h = 1e-4;
                let (a, b) = ((t - h).max(0.0), (t + h).min(1.0));
                (easing.apply(b) - easing.apply(a)) / (b - a)
            };
            let mut points = vec![Point::new(0.0, 0.0)];
            for segment in 0..SEGMENTS {
                let t0 = segment as f64 / SEGMENTS as f64;
                let t1 = (segment + 1) as f64 / SEGMENTS as f64;
                let third = (t1 - t0) / 3.0;
                let (y0, y1) = (easing.apply(t0), easing.apply(t1));
                points.push(Point::new(t0 + third, y0 + slope(t0) * third));
                points.push(Point::new(t1 - third, y1 - slope(t1) * third));
                points.push(Point::new(t1, y1));
            }
            points
        }
    }
}

/// A sound's attributes onto its frame's, and its `<SoundEnvelope>` (the
/// volume), written as Animate writes them and as `buzz_import_xfl` reads
/// them back:
///
/// * `soundSync` (`event`, Animate's default, is left off);
/// * the loop checkbox as `soundLoopMode="loop"` with Animate's own
///   `soundLoop="32767"` (measured: without that count Animate reads no
///   loop), and a repeat count as `soundLoopMode="repeat"`;
/// * the in- and out-points in samples at 44.1 kHz (`inPoint44`,
///   `outPoint44`);
/// * the volume as one envelope point on both channels, `32768` being full.
fn sound_attrs(sound: &buzz_scene::SoundRef, name: &str, attrs: &mut String, indent: usize) -> String {
    use buzz_scene::SoundSync;
    attrs.push_str(&format!(" soundName=\"{}\"", escape(name)));
    match sound.sync {
        SoundSync::Event => {}
        SoundSync::Start => attrs.push_str(" soundSync=\"start\""),
        SoundSync::Stop => attrs.push_str(" soundSync=\"stop\""),
        SoundSync::Stream => attrs.push_str(" soundSync=\"stream\""),
    }
    match sound.loops {
        0 => attrs.push_str(" soundLoopMode=\"loop\" soundLoop=\"32767\""),
        1 => {}
        n => attrs.push_str(&format!(" soundLoopMode=\"repeat\" soundLoop=\"{n}\"")),
    }
    if sound.trim_start > 0.0 {
        attrs.push_str(&format!(" inPoint44=\"{}\"", (f64::from(sound.trim_start) * 44_100.0).round() as u64));
    }
    if let Some(end) = sound.trim_end {
        attrs.push_str(&format!(" outPoint44=\"{}\"", (f64::from(end) * 44_100.0).round() as u64));
    }
    let level = (f64::from(sound.volume.clamp(0.0, 1.0)) * 32_768.0).round() as u32;
    if level < 32_768 {
        attrs.push_str(" soundEffect=\"custom\"");
    }
    let pad = " ".repeat(indent);
    format!(
        "{pad}<SoundEnvelope>\n{pad}  <SoundEnvelopePoint level0=\"{level}\" level1=\"{level}\"/>\n{pad}</SoundEnvelope>\n"
    )
}

// ---- one object --------------------------------------------------------------

/// Must the shapes among these siblings be written as drawing objects?
///
/// **Animate merges raw shapes.** Every non-drawing-object shape on a frame
/// (or in a group) is one shape to Animate, which puts it beneath everything
/// else on the layer and resolves overlaps as holes: two overlapping shapes
/// opened as a ring, trees as hoops, and a shape stacked above a symbol
/// slipped under it. Our shapes are already resolved artwork in their own
/// stacking order, so beside anything else each one is written as a drawing
/// object, which Animate keeps whole and in order.
fn keeps_shapes_apart(objects: &[std::sync::Arc<Object>]) -> bool {
    objects.len() > 1 || objects.iter().any(|o| matches!(o.kind, ObjectKind::Armature(_)))
}

fn object_xml(
    scene: &Scene,
    paths: &LibraryPaths,
    object: &Object,
    separate: bool,
    report: &mut FlaReport,
    indent: usize,
) -> String {
    if !object.filters.is_empty() {
        report.skip("filters");
    }
    match &object.kind {
        ObjectKind::Shape(shape) => {
            report.objects += 1;
            shape_xml(shape, object.transform, object.drawing_object || separate, report, indent)
        }
        ObjectKind::Group(children) => {
            report.objects += 1;
            let pad = " ".repeat(indent);
            let mut out = format!("{pad}<DOMGroup>\n{pad}  <members>\n");
            let apart = keeps_shapes_apart(children);
            for child in children {
                // A group's own transform is carried into its members, because
                // Animate's DOMGroup has no matrix of its own.
                let mut moved: Object = (**child).clone();
                moved.transform = object.transform * moved.transform;
                out.push_str(&object_xml(scene, paths, &moved, apart, report, indent + 4));
            }
            out.push_str(&format!("{pad}  </members>\n{pad}</DOMGroup>\n"));
            out
        }
        ObjectKind::Instance(instance) => {
            report.objects += 1;
            let pad = " ".repeat(indent);
            let Some(symbol) = scene.library().get(instance.symbol) else {
                return String::new();
            };
            let kind = match symbol.kind {
                SymbolKind::MovieClip => "movie clip",
                SymbolKind::Button => "button",
                SymbolKind::Graphic => "graphic",
            };
            // A graphic instance carries how it plays; Animate writes it on
            // every one, and a reader that expects it finds nothing otherwise.
            // **And where it starts**: a held mouth or eye is a single-frame
            // instance on a chosen frame, and writing every one as a loop from
            // frame 0 opened each character with the same default face.
            let playing = if matches!(symbol.kind, SymbolKind::Graphic) {
                let mode = match instance.loop_mode {
                    buzz_scene::LoopMode::Loop => "loop",
                    buzz_scene::LoopMode::PlayOnce => "play once",
                    buzz_scene::LoopMode::SingleFrame => "single frame",
                };
                let first = if instance.first_frame > 0 {
                    format!(" firstFrame=\"{}\"", instance.first_frame)
                } else {
                    String::new()
                };
                format!(" loop=\"{mode}\"{first}")
            } else {
                String::new()
            };
            format!(
                "{pad}<DOMSymbolInstance libraryItemName=\"{}\" symbolType=\"{kind}\"{playing}>\n{}{}{pad}</DOMSymbolInstance>\n",
                escape(paths.of(symbol.id)),
                matrix_xml(object.transform, indent + 2),
                // **The joint goes back out too.** On a rigged character the
                // transformation point is the shoulder an arm swings from, and
                // leaving it out would hand Animate a character whose joints
                // had all been reset to the middle of their artwork.
                pivot_xml(object.pivot, indent + 2),
            )
        }
        // Rigged and warped artwork has no equivalent in a `.fla`: Animate's
        // own bones are a different model and its Asset Warp is not in the
        // file format at all. The *posed* artwork goes out, so the picture is
        // right and only the rig is lost — which is said out loud.
        ObjectKind::Armature(rig) => {
            report.skip("bone rigs (the posed artwork is written, the skeleton is not)");
            let mut out = String::new();
            for part in rig.posed() {
                let mut moved: Object = (*part).clone();
                moved.transform = object.transform * moved.transform;
                out.push_str(&object_xml(scene, paths, &moved, true, report, indent));
            }
            out
        }
        ObjectKind::Warp(warp) => {
            report.skip("warp handles (the warped artwork is written, the handles are not)");
            report.objects += 1;
            shape_xml(&warp.warped(), object.transform, object.drawing_object || separate, report, indent)
        }
    }
}

fn shape_xml(
    shape: &buzz_scene::ShapeData,
    transform: Affine,
    drawing_object: bool,
    report: &mut FlaReport,
    indent: usize,
) -> String {
    let pad = " ".repeat(indent);
    // Animate's DOMShape carries no matrix, so the object's transform is
    // applied to the geometry on the way out.
    let path = transform * shape.path.clone();
    let Some(edges) = edge::write_edges(&path) else {
        return String::new();
    };

    let fill = shape.fill.as_ref().and_then(|f| solid(&f.paint, report));
    let stroke = shape.stroke.as_ref().and_then(|s| {
        solid(&s.paint, report).map(|colour| (colour, if s.hairline { 0.1 } else { s.width }))
    });
    // A fill needs somewhere to be: an open outline is drawn as nothing.
    let fill = fill.filter(|_| edge::is_closed(&path));

    // **Overlapping outlines.** Animate fills a shape's outlines by parity:
    // where two of them overlap it leaves a hole (measured: two overlapping
    // squares in one shape opened as a ring). Ours fill by winding, so a
    // shape made of several outlines is resolved to the region it covers
    // before it goes out, and the outlines that remain never overlap. An
    // even-odd fill already means what Animate draws.
    let fill_path = match &shape.fill {
        Some(spec)
            if fill.is_some()
                && spec.rule == buzz_geom::FillMode::NonZero
                && subpath_count(&path) > 1 =>
        {
            let options = buzz_geom::boolean::options_for(std::slice::from_ref(&path));
            let resolved = buzz_geom::boolean(&path, &path, buzz_geom::BoolOp::Union, options);
            if resolved.elements().is_empty() { path.clone() } else { resolved }
        }
        _ => path.clone(),
    };
    let fill_path = if fill.is_some() { drop_slivers(&fill_path) } else { fill_path };
    let separate_fill = fill.is_some() && fill_path != path;
    let fill_edges = if separate_fill { edge::write_edges(&fill_path) } else { None };

    let has_fill = fill.is_some();
    let has_stroke = stroke.is_some();

    let drawing = if drawing_object { " isDrawingObject=\"true\"" } else { "" };
    let mut out = format!("{pad}<DOMShape{drawing}>\n");
    if let Some((colour, alpha)) = fill {
        out.push_str(&format!(
            "{pad}  <fills>\n{pad}    <FillStyle index=\"1\">\n{pad}      <SolidColor color=\"{colour}\"{}/>\n{pad}    </FillStyle>\n{pad}  </fills>\n",
            alpha_attr(alpha)
        ));
    }
    if let Some(((colour, alpha), width)) = stroke {
        out.push_str(&format!(
            "{pad}  <strokes>\n{pad}    <StrokeStyle index=\"1\">\n{pad}      <SolidStroke weight=\"{}\">\n{pad}        <fill>\n{pad}          <SolidColor color=\"{colour}\"{}/>\n{pad}        </fill>\n{pad}      </SolidStroke>\n{pad}    </StrokeStyle>\n{pad}  </strokes>\n",
            number(width),
            alpha_attr(alpha)
        ));
    }

    out.push_str(&format!("{pad}  <edges>\n"));
    if let Some(fill_edges) = &fill_edges {
        // The resolved fill and the drawn outline are no longer the same
        // edges, so each goes out on its own.
        out.push_str(&format!(
            "{pad}    <Edge fillStyle0=\"1\" edges=\"{}\"/>\n",
            escape(fill_edges)
        ));
        if has_stroke {
            out.push_str(&format!(
                "{pad}    <Edge strokeStyle=\"1\" edges=\"{}\"/>\n",
                escape(&edges)
            ));
        }
    } else {
        let mut attrs = String::new();
        if has_fill {
            attrs.push_str(" fillStyle0=\"1\"");
        }
        if has_stroke {
            attrs.push_str(" strokeStyle=\"1\"");
        }
        out.push_str(&format!(
            "{pad}    <Edge{attrs} edges=\"{}\"/>\n",
            escape(&edges)
        ));
    }
    out.push_str(&format!("{pad}  </edges>\n{pad}</DOMShape>\n"));
    out
}

/// Outlines enclosing less than this many square pixels are left out of a fill.
const SLIVER_AREA: f64 = 1.0;

/// A fill path without the outlines that enclose next to nothing.
///
/// **Slivers lose the whole fill.** Measured in Animate 2024
/// (docs/FLA_EXPORT_PLAN.md X1): Guddu's trouser leg, one large outline plus
/// three slivers of under half a square pixel left by resolving it, opened
/// with no fill at all; the same edges without the slivers opened whole.
/// What is dropped is too small to see. A path that is nothing but slivers is
/// kept as it is.
fn drop_slivers(path: &buzz_geom::BezPath) -> buzz_geom::BezPath {
    let mut parts: Vec<buzz_geom::BezPath> = Vec::new();
    for el in path.elements() {
        if parts.is_empty() || matches!(el, kurbo::PathEl::MoveTo(_)) {
            parts.push(buzz_geom::BezPath::new());
        }
        if let Some(part) = parts.last_mut() {
            part.push(*el);
        }
    }
    let kept: Vec<&buzz_geom::BezPath> = parts
        .iter()
        .filter(|part| kurbo::Shape::area(*part).abs() >= SLIVER_AREA)
        .collect();
    if kept.is_empty() || kept.len() == parts.len() {
        return path.clone();
    }
    let mut out = buzz_geom::BezPath::new();
    for part in kept {
        for el in part.elements() {
            out.push(*el);
        }
    }
    out
}

/// How many separate outlines a path is drawn from.
fn subpath_count(path: &buzz_geom::BezPath) -> usize {
    path.elements()
        .iter()
        .filter(|el| matches!(el, kurbo::PathEl::MoveTo(_)))
        .count()
}

/// A paint as a solid colour, or `None` if it is something a `.fla` cannot
/// carry — which is noted rather than passed over.
fn solid(paint: &buzz_scene::Paint, report: &mut FlaReport) -> Option<(String, f32)> {
    match paint {
        buzz_scene::Paint::Solid(colour) => Some((hex(*colour), colour.components[3])),
        buzz_scene::Paint::Gradient(_) => {
            report.skip("gradients");
            None
        }
        buzz_scene::Paint::Image(_) => {
            report.skip("bitmap fills");
            None
        }
    }
}

fn alpha_attr(alpha: f32) -> String {
    if alpha >= 1.0 {
        String::new()
    } else {
        format!(" alpha=\"{}\"", number(f64::from(alpha)))
    }
}

/// The transformation point, in the object's own space, as Animate writes it.
fn pivot_xml(pivot: Option<buzz_geom::Point>, indent: usize) -> String {
    let Some(at) = pivot else {
        return String::new();
    };
    let pad = " ".repeat(indent);
    format!(
        "{pad}<transformationPoint>
{pad}  <Point x=\"{}\" y=\"{}\"/>
{pad}</transformationPoint>
",
        number(at.x),
        number(at.y),
    )
}

fn matrix_xml(transform: Affine, indent: usize) -> String {
    let pad = " ".repeat(indent);
    let c = transform.as_coeffs();
    if c == Affine::IDENTITY.as_coeffs() {
        return String::new();
    }
    format!(
        "{pad}<matrix>\n{pad}  <Matrix a=\"{}\" b=\"{}\" c=\"{}\" d=\"{}\" tx=\"{}\" ty=\"{}\"/>\n{pad}</matrix>\n",
        number(c[0]),
        number(c[1]),
        number(c[2]),
        number(c[3]),
        number(c[4]),
        number(c[5]),
    )
}

// ---- sound files ---------------------------------------------------------------

/// A sound's file as it goes into `LIBRARY/`, with its extension: MP3 and
/// 8- or 16-bit PCM WAV exactly as imported (what Animate imports itself), a
/// WAV in any other sample format converted to 16-bit PCM, and `None` for
/// anything else, which Animate does not import.
fn sound_media(sound: &buzz_scene::SoundAsset) -> Option<(Vec<u8>, &'static str)> {
    let data = sound.data.as_slice();
    let is_wav = data.len() >= 12 && &data[0..4] == b"RIFF" && &data[8..12] == b"WAVE";
    if is_wav {
        let wav = Wav::read(data)?;
        if wav.tag == 1 && (wav.bits == 8 || wav.bits == 16) {
            return Some((data.to_vec(), "wav"));
        }
        return Some((wav.to_pcm16()?, "wav"));
    }
    let is_mp3 = data.starts_with(b"ID3") || (data.len() >= 2 && data[0] == 0xFF && (data[1] & 0xE0) == 0xE0);
    if is_mp3 || sound.format == "mp3" {
        return Some((data.to_vec(), "mp3"));
    }
    None
}

/// Animate's `format` for a clip at a rate it stores (`44kHz 16bit Stereo`),
/// or `None` for any other, which Animate works out for itself on import.
fn sound_format(sound: &buzz_scene::SoundAsset) -> Option<String> {
    let rate = match sound.sample_rate {
        5512 | 5513 => "5kHz",
        11025 => "11kHz",
        22050 => "22kHz",
        44100 => "44kHz",
        _ => return None,
    };
    let channels = match sound.channels {
        1 => "Mono",
        2 => "Stereo",
        _ => return None,
    };
    let data = sound.data.as_slice();
    let bits = match Wav::read(data) {
        Some(wav) if wav.tag == 1 && wav.bits == 8 => "8bit",
        _ => "16bit",
    };
    Some(format!("{rate} {bits} {channels}"))
}

/// The parts of a WAV file the export needs.
struct Wav<'a> {
    /// `1` integer PCM, `3` IEEE float (the extensible header's own sub-format
    /// read through).
    tag: u16,
    channels: u16,
    rate: u32,
    bits: u16,
    samples: &'a [u8],
}

impl<'a> Wav<'a> {
    fn read(data: &'a [u8]) -> Option<Self> {
        if data.len() < 12 || &data[0..4] != b"RIFF" || &data[8..12] != b"WAVE" {
            return None;
        }
        let u16_at = |at: usize| Some(u16::from_le_bytes(data.get(at..at + 2)?.try_into().ok()?));
        let u32_at = |at: usize| Some(u32::from_le_bytes(data.get(at..at + 4)?.try_into().ok()?));
        let mut at = 12;
        let mut format = None;
        let mut samples = None;
        while at + 8 <= data.len() {
            let id = &data[at..at + 4];
            let size = u32_at(at + 4)? as usize;
            let body = at + 8;
            let end = (body + size).min(data.len());
            if id == b"fmt " {
                let mut tag = u16_at(body)?;
                if tag == 0xFFFE {
                    tag = u16_at(body + 24)?;
                }
                format = Some((tag, u16_at(body + 2)?, u32_at(body + 4)?, u16_at(body + 14)?));
            } else if id == b"data" {
                samples = Some(&data[body..end]);
            }
            at = body + size + (size & 1);
        }
        let (tag, channels, rate, bits) = format?;
        Some(Self {
            tag,
            channels,
            rate,
            bits,
            samples: samples?,
        })
    }

    /// The same clip as a 16-bit PCM WAV file.
    fn to_pcm16(&self) -> Option<Vec<u8>> {
        let width = usize::from(self.bits / 8);
        if width == 0 || self.channels == 0 {
            return None;
        }
        let sample = |chunk: &[u8]| -> Option<f64> {
            Some(match (self.tag, self.bits) {
                (1, 8) => (f64::from(chunk[0]) - 128.0) / 128.0,
                (1, 16) => f64::from(i16::from_le_bytes([chunk[0], chunk[1]])) / 32768.0,
                (1, 24) => f64::from(i32::from_le_bytes([0, chunk[0], chunk[1], chunk[2]]) >> 8) / 8_388_608.0,
                (1, 32) => f64::from(i32::from_le_bytes(chunk.try_into().ok()?)) / 2_147_483_648.0,
                (3, 32) => f64::from(f32::from_le_bytes(chunk.try_into().ok()?)),
                (3, 64) => f64::from_le_bytes(chunk.try_into().ok()?),
                _ => return None,
            })
        };
        let mut pcm = Vec::with_capacity(self.samples.len() / width * 2);
        for chunk in self.samples.chunks_exact(width) {
            let value = (sample(chunk)? * 32767.0).round().clamp(-32768.0, 32767.0) as i16;
            pcm.extend_from_slice(&value.to_le_bytes());
        }
        let block = self.channels * 2;
        let mut out = Vec::with_capacity(44 + pcm.len());
        out.extend_from_slice(b"RIFF");
        out.extend_from_slice(&(36 + pcm.len() as u32).to_le_bytes());
        out.extend_from_slice(b"WAVEfmt ");
        out.extend_from_slice(&16u32.to_le_bytes());
        out.extend_from_slice(&1u16.to_le_bytes());
        out.extend_from_slice(&self.channels.to_le_bytes());
        out.extend_from_slice(&self.rate.to_le_bytes());
        out.extend_from_slice(&(self.rate * u32::from(block)).to_le_bytes());
        out.extend_from_slice(&block.to_le_bytes());
        out.extend_from_slice(&16u16.to_le_bytes());
        out.extend_from_slice(b"data");
        out.extend_from_slice(&(pcm.len() as u32).to_le_bytes());
        out.extend_from_slice(&pcm);
        Some(out)
    }
}

// ---- small conversions -------------------------------------------------------

/// A number as Animate writes them: no exponent, no trailing zeros.
fn number(value: f64) -> String {
    if !value.is_finite() {
        return "0".to_string();
    }
    if (value - value.round()).abs() < 1e-9 {
        return format!("{}", value.round() as i64);
    }
    let text = format!("{value:.6}");
    text.trim_end_matches('0').trim_end_matches('.').to_string()
}

fn hex(colour: Color) -> String {
    let [r, g, b, _] = colour.to_rgba8().to_u8_array();
    format!("#{r:02X}{g:02X}{b:02X}")
}

/// A symbol name as a file name. Animate's library allows characters a file
/// system does not, and a `/` in a symbol name would otherwise write outside
/// `LIBRARY/`.
fn file_name(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| match c {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '_',
            other => other,
        })
        .collect();
    // A run of dots is not a directory once the separators are gone, but it
    // still *reads* as one — and a package entry that looks like it climbs out
    // of `LIBRARY/` is not something to ship even when it does not.
    let cleaned = cleaned.replace("..", "_");
    let trimmed = cleaned.trim().trim_start_matches('.');
    if trimmed.is_empty() {
        "Symbol".to_string()
    } else {
        trimmed.to_string()
    }
}

fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            other => out.push(other),
        }
    }
    out
}

#[cfg(test)]
mod tests;
