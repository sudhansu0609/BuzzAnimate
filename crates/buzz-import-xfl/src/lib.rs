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
    Object, ObjectId, Paint, Scene, ShapeData, StrokeSpec, Symbol, SymbolId, SymbolKind, Tween,
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
    #[error("could not find DOMDocument.xml; this does not look like an Animate document")]
    MissingDocument,
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
    pub layers: usize,
    pub keyframes: usize,
    pub shapes: usize,
    pub groups: usize,
    pub instances: usize,
    pub symbols: usize,
    pub tweens: usize,
    /// Keyframes on Animate's camera layer.
    pub camera_keys: usize,
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

    import_fla_bytes(&bytes)
}

/// Import from `.fla` bytes.
pub fn import_fla_bytes(bytes: &[u8]) -> Result<(Scene, ImportReport), ImportError> {
    let mut archive = zip::ZipArchive::new(std::io::Cursor::new(bytes))?;

    let document = read_entry(&mut archive, "DOMDocument.xml")
        .ok_or(ImportError::MissingDocument)?;

    // Symbol definitions live in LIBRARY/, one file each.
    let library_names: Vec<String> = (0..archive.len())
        .filter_map(|i| archive.by_index(i).ok().map(|f| f.name().to_string()))
        .filter(|name| {
            let lower = name.to_ascii_lowercase();
            lower.starts_with("library/") && lower.ends_with(".xml")
        })
        .collect();

    let mut library_files = Vec::new();
    for name in library_names {
        if let Some(text) = read_entry(&mut archive, &name) {
            library_files.push((name, text));
        }
    }

    build(&document, &library_files)
}

/// Import from an uncompressed `.xfl` folder.
pub fn import_xfl_folder(folder: &Path) -> Result<(Scene, ImportReport), ImportError> {
    let document_path = folder.join("DOMDocument.xml");
    if !document_path.exists() {
        return Err(ImportError::MissingDocument);
    }
    let document = std::fs::read_to_string(&document_path)?;

    let mut library_files = Vec::new();
    let library_dir = folder.join("LIBRARY");
    if library_dir.is_dir() {
        collect_library(&library_dir, &library_dir, &mut library_files)?;
    }

    build(&document, &library_files)
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
        } else if path.extension().is_some_and(|e| e.eq_ignore_ascii_case("xml")) {
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

fn read_entry<R: std::io::Read + std::io::Seek>(
    archive: &mut zip::ZipArchive<R>,
    name: &str,
) -> Option<String> {
    let mut file = archive.by_name(name).ok()?;
    let mut text = String::new();
    file.read_to_string(&mut text).ok()?;
    Some(text)
}

/// Assemble a scene from the document and its library files.
fn build(
    document: &str,
    library_files: &[(String, String)],
) -> Result<(Scene, ImportReport), ImportError> {
    let mut scene = Scene::empty();
    let mut report = ImportReport::default();
    let mut ids = IdSource::default();

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
        match parse_symbol(xml, path, id, &by_name, &mut ids, &mut report) {
            Ok(symbol) => {
                scene.library_mut().insert(symbol);
                report.symbols += 1;
            }
            Err(e) => report.note_unsupported(format!("symbol {path}: {e}")),
        }
    }

    parse_document(document, &mut scene, &by_name, &mut ids, &mut report)?;

    // The importer allocates its own ids, so the document's allocator has to
    // be raised past them or a new object would collide with an imported one.
    scene.reserve_ids_above(ids.next);
    Ok((scene, report))
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
    let hex = attrs.get(color_key).map(String::as_str).unwrap_or("#000000");
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
    ids: &mut IdSource,
    report: &mut ImportReport,
) -> Result<(), ImportError> {
    let mut reader = quick_xml::Reader::from_str(xml);
    reader.config_mut().trim_text(true);
    // Reject mismatched or unclosed tags. A truncated file should be reported,
    // not half-imported into a document that looks complete.
    reader.config_mut().check_end_names = true;

    let mut layers: Vec<PendingLayer> = Vec::new();
    let mut context = FrameContext::default();
    let mut camera = CameraCapture::default();
    let mut open_elements = 0usize;
    // Only the first timeline is the main one; symbol timelines live in
    // LIBRARY and are parsed separately.
    let mut timeline_depth = 0usize;

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
                }
                // The one closing tag that matters: a shape is only a shape
                // once every one of its edges has been read.
                if name == "DOMShape" && timeline_depth <= 1 {
                    context.finish_shape(ids, report);
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

    resolve_layer_parents(&mut layers);

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
fn resolve_layer_parents(layers: &mut [PendingLayer]) {
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

    bake_rig_offsets(layers);
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
/// Reconciling the two by baking the parent's **rest** pose into the child is
/// exact for one level and wrong for two, because matrices do not commute: by
/// the shin the two products disagree, and a leg comes out stretched. So the
/// chain is composed here **per keyframe** instead:
///
/// ```text
/// child_world(f) = parent_world(f) * child_relative(f)
/// ```
///
/// Parents are baked before their children, so `parent_world` is read straight
/// off the parent's own already-absolute keyframes. The rig link is then
/// dropped: the artwork carries the pose, and propagating the parent's motion
/// a second time would double every move.
fn bake_rig_offsets(layers: &mut [PendingLayer]) {
    use std::collections::HashMap;

    let follows: HashMap<LayerId, LayerId> = layers
        .iter()
        .filter_map(|p| p.layer.follows.map(|f| (p.layer.id, f)))
        .collect();
    if follows.is_empty() {
        return;
    }

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

    for index in order {
        let Some(parent) = layers[index].layer.follows else {
            continue;
        };
        let Some(parent_index) = layers.iter().position(|p| p.layer.id == parent) else {
            layers[index].layer.follows = None;
            continue;
        };

        // What the parent is doing at each of this layer's keyframes, read
        // from the parent as it now stands — already absolute.
        let at_parent = |frame: u32| -> Affine {
            layers[parent_index]
                .layer
                .frames
                .resolved_at(frame)
                .iter()
                .next()
                .map(|object| object.transform)
                .unwrap_or(Affine::IDENTITY)
        };

        let moved: Vec<buzz_scene::Keyframe> = layers[index]
            .layer
            .frames
            .keyframes()
            .iter()
            .map(|keyframe| {
                let parent_now = at_parent(keyframe.start);
                let objects: Vec<std::sync::Arc<Object>> = keyframe
                    .objects
                    .iter()
                    .map(|object| {
                        let mut copy = (**object).clone();
                        copy.transform = parent_now * copy.transform;
                        std::sync::Arc::new(copy)
                    })
                    .collect();
                buzz_scene::Keyframe {
                    objects: std::sync::Arc::new(objects),
                    ..keyframe.clone()
                }
            })
            .collect();

        let length = layers[index].layer.frames.length();
        layers[index].layer.frames = buzz_scene::LayerTimeline::from_parts(moved, length);
        // The pose is in the artwork now; keeping the link would move it twice.
        layers[index].layer.follows = None;
    }
}

/// Accumulates layers, frames and elements while walking the XML.
#[derive(Default)]
struct FrameContext {
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
        let mut push = |path: buzz_geom::BezPath,
                        fill: Option<Paint>,
                        stroke: Option<(Paint, f64)>| {
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
                    }),
                    stroke: stroke.map(|(paint, width)| StrokeSpec {
                        paint,
                        width,
                        hairline: false,
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

        // Counted after the closure has finished with `report`.
        report.shapes += made;
        if gaps > 0 {
            report.note_unsupported("a fill outline that does not close on itself");
        }
    }

    fn begin_frame(&mut self, attrs: &HashMap<String, String>, report: &mut ImportReport) {
        self.finish_frame();
        // Which layer this one follows, by rigging index. Animate keeps it on
        // the frame because a rig can be re-parented part way through a shot;
        // our model has one link per layer, so the first one stated wins.
        if self.rig_parent.is_none() {
            self.rig_parent = attrs
                .get("parentLayerIndex")
                .and_then(|v| v.trim().parse::<u32>().ok());
        }
        let start = attr_u32(attrs, "index", 0);
        let duration = attr_u32(attrs, "duration", 1).max(1);
        let tween = parse_tween(attrs);
        if tween.is_active() {
            report.tweens += 1;
        }
        self.current = Some(PendingFrame {
            start,
            duration,
            label: attrs.get("name").cloned().filter(|s| !s.is_empty()),
            tween,
            objects: Vec::new(),
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

        match name {
            // A new shape starts a new set of style tables — and finishes any
            // shape still open, so a file that nests one inside a group still
            // draws both.
            "DOMShape" => {
                self.finish_shape(ids, report);
                self.styles.begin_shape();
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
            "BitmapFill" => {
                report.note_unsupported("a bitmap fill, imported as a flat colour");
                self.styles.set_color(Color::from_rgb8(0x80, 0x80, 0x80));
            }

            "Edge" => {
                // Collected, not drawn. A shape's outlines are spread across
                // its edges and only make sense once all of them are in —
                // `finish_shape` is where they become artwork.
                let Some(data) = attrs.get("edges") else { return };
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
            "DOMBitmapInstance" => report.note_unsupported("bitmap"),
            "DOMStaticText" | "DOMDynamicText" | "DOMInputText" => {
                report.note_unsupported("text")
            }
            "DOMVideoInstance" => report.note_unsupported("video"),
            "DOMSoundItem" => report.note_unsupported("sound"),
            "Matrix" => {
                // **A gradient's matrix is its own.** It says where the ramp
                // runs, and it is claimed here before anything else can mistake
                // it for a placement.
                if self.styles.place_gradient(parse_matrix(attrs)) {
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
            _ => {}
        }
    }

    fn finish_frame(&mut self) {
        let Some(frame) = self.current.take() else {
            return;
        };
        self.length = self.length.max(frame.start + frame.duration);
        self.keyframes.push(buzz_scene::Keyframe {
            start: frame.start,
            objects: std::sync::Arc::new(frame.objects),
            label: frame.label,
            tween: frame.tween,
            // Imported formats carry no sound yet (PROGRESS §7).
            sound: None,
        });
    }

    fn flush_layer(&mut self, layers: &mut Vec<PendingLayer>, _report: &mut ImportReport) {
        self.finish_frame();
        let Some(mut layer) = self.layer.take() else {
            return;
        };
        layer.frames = buzz_scene::LayerTimeline::from_parts(
            std::mem::take(&mut self.keyframes),
            self.length.max(1),
        );
        layers.push(PendingLayer {
            layer,
            parent_index: self.parent_index.take(),
            rig_index: self.rig_index.take(),
            rig_parent: self.rig_parent.take(),
        });
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
    for full in [library_name_of(xml), Some(from_path)].into_iter().flatten() {
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

    let mut context = FrameContext::default();
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
                    _ => context.element(&name, &attrs, symbols, ids, report),
                }
            }
            Ok(Event::End(e)) if e.name().as_ref() == b"DOMShape" => {
                context.finish_shape(ids, report);
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
    resolve_layer_parents(&mut layers);

    if layers.is_empty() {
        layers.push(PendingLayer {
            layer: Layer::normal(LayerId(ids.take()), "Layer_1"),
            parent_index: None,
            rig_index: None,
            rig_parent: None,
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
                ("LIBRARY/parts/character.xml".to_string(), CHARACTER.to_string()),
                ("LIBRARY/parts/torso.xml".to_string(), part("torso")),
                ("LIBRARY/parts/arm.xml".to_string(), part("arm")),
                ("LIBRARY/hero.xml".to_string(), HERO_SYMBOL.to_string()),
            ],
        )
        .unwrap();

        assert!(
            !report.unsupported.iter().any(|u| u.contains("unknown symbol")),
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

        // The link is read and then *spent*: the child's pose is composed
        // with the parent's, frame by frame, and the artwork carries it from
        // there. Keeping the link as well would move the head twice.
        assert_eq!(head.follows, None, "the rig is baked, not left live");
        assert_eq!(body.follows, None);

        // Relative (10, -20) on a body at (300, 400) is (310, 380) absolute.
        let placed = head.objects_at(0)[0].transform.as_coeffs();
        assert!(
            (placed[4] - 310.0).abs() < 1e-9 && (placed[5] - 380.0).abs() < 1e-9,
            "the parent's rest pose should be baked in, got {:?}",
            (placed[4], placed[5])
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
        assert!((keys[2].zoom - 2.0).abs() < 1e-9, "zoom was {}", keys[2].zoom);
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
        assert!(report.unsupported[0].contains("x50"), "{:?}", report.unsupported);
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
        std::fs::write(&path, [0xD0u8, 0xCF, 0x11, 0xE0, 0xA1, 0xB1, 0x1A, 0xE1, 0, 0]).unwrap();

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
            Err(ImportError::MissingDocument)
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
        assert_eq!(scene.layers().len(), 1, "a document needs somewhere to draw");
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

        let first = shapes[0].fill.as_ref().expect("the first edge is filled").color();
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
        assert_eq!(stroke.color().to_rgba8().to_u8_array(), [0x00, 0xFF, 0x00, 0xFF]);
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
        assert_eq!(g.stops()[1].color.to_rgba8().to_u8_array()[..3], [255, 0, 0]);

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
            shapes[0].fill.as_ref().unwrap().color().to_rgba8().to_u8_array(),
            [0x11, 0x22, 0x33, 0xFF]
        );
        // The second shape declares no styles, so it falls back to the visible
        // default rather than inheriting the first shape's blue.
        assert_ne!(
            shapes[1].fill.as_ref().unwrap().color().to_rgba8().to_u8_array(),
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
}
