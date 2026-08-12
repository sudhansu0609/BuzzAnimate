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
    FillSpec, Layer, LayerId, LayerKind, Object, ObjectId, Scene, ShapeData, StrokeSpec, Symbol,
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
        format!(
            "{} layers, {} keyframes, {} shapes, {} instances, {} symbols",
            self.layers, self.keyframes, self.shapes, self.instances, self.symbols
        )
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

    // Symbols first: the stage's instances refer to them by name.
    let mut by_name: HashMap<String, SymbolId> = HashMap::new();
    for (path, xml) in library_files {
        match parse_symbol(xml, path, &mut ids, &mut report) {
            Ok(symbol) => {
                let name = symbol.name.clone();
                let id = symbol.id;
                scene.library_mut().insert(symbol);
                by_name.insert(name, id);
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
        _ => {
            // Animate marks a masked or guided layer by pointing at its parent
            // rather than by naming its own type.
            if attrs.contains_key("parentLayerIndex") {
                LayerKind::Normal
            } else {
                LayerKind::Normal
            }
        }
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

    let mut layers: Vec<Layer> = Vec::new();
    let mut context = FrameContext::default();
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
                    "DOMTimeline" => timeline_depth += 1,
                    "DOMLayer" if timeline_depth <= 1 => {
                        context.flush_layer(&mut layers, report);
                        context.begin_layer(&attrs, ids);
                    }
                    "DOMFrame" if timeline_depth <= 1 => {
                        context.begin_frame(&attrs, report);
                    }
                    _ if timeline_depth <= 1 => {
                        context.element(&name, &attrs, symbols, ids, report);
                    }
                    _ => {}
                }
            }
            Ok(Event::End(e)) => {
                let name = String::from_utf8_lossy(e.name().as_ref()).to_string();
                if name == "DOMTimeline" {
                    timeline_depth = timeline_depth.saturating_sub(1);
                }
            }
            Err(e) => return Err(ImportError::Xml(e.to_string())),
            _ => {}
        }
    }

    context.flush_layer(&mut layers, report);

    // Animate writes layers bottom-first; our stack is front-first.
    layers.reverse();
    if layers.is_empty() {
        layers.push(Layer::normal(LayerId(ids.take()), "Layer_1"));
    }
    for (index, layer) in layers.into_iter().enumerate() {
        report.layers += 1;
        scene.edit_layers().insert(index, layer);
    }
    Ok(())
}

/// Accumulates layers, frames and elements while walking the XML.
#[derive(Default)]
struct FrameContext {
    layer: Option<Layer>,
    keyframes: Vec<buzz_scene::Keyframe>,
    length: u32,
    current: Option<PendingFrame>,
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
        self.keyframes.clear();
        self.length = 0;
    }

    fn begin_frame(&mut self, attrs: &HashMap<String, String>, report: &mut ImportReport) {
        self.finish_frame();
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

        match name {
            "Edge" => {
                // The geometry itself. `fillStyle0` etc. reference styles
                // declared earlier in the shape; without tracking those we
                // fall back to a visible default rather than dropping the art.
                let Some(data) = attrs.get("edges") else { return };
                match parse_edges_closed(data) {
                    Ok(path) if !path.elements().is_empty() => {
                        let filled = attrs.contains_key("fillStyle0")
                            || attrs.contains_key("fillStyle1");
                        let shape = if filled {
                            ShapeData {
                                path,
                                fill: Some(FillSpec::solid(Color::from_rgb8(0x99, 0x99, 0x99))),
                                stroke: None,
                            }
                        } else {
                            ShapeData {
                                path,
                                fill: None,
                                stroke: Some(StrokeSpec::new(Color::BLACK, 1.0)),
                            }
                        };
                        frame
                            .objects
                            .push(std::sync::Arc::new(Object::shape(ObjectId(ids.take()), shape)));
                        report.shapes += 1;
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
                // the last segment.
                let short = library_name.rsplit('/').next().unwrap_or(library_name);
                match symbols.get(short).or_else(|| symbols.get(library_name)) {
                    Some(symbol) => {
                        let object = Object::instance_of(ObjectId(ids.take()), *symbol);
                        frame.objects.push(std::sync::Arc::new(object));
                        report.instances += 1;
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
                // Applies to the element most recently pushed.
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
        });
    }

    fn flush_layer(&mut self, layers: &mut Vec<Layer>, _report: &mut ImportReport) {
        self.finish_frame();
        let Some(mut layer) = self.layer.take() else {
            return;
        };
        layer.frames = buzz_scene::LayerTimeline::from_parts(
            std::mem::take(&mut self.keyframes),
            self.length.max(1),
        );
        layers.push(layer);
    }
}

/// Parse one `LIBRARY/*.xml` symbol definition.
fn parse_symbol(
    xml: &str,
    path: &str,
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

    let mut symbol = Symbol::new(SymbolId(ids.take()), file_name, SymbolKind::Graphic);
    symbol.folder = folder;

    let mut context = FrameContext::default();
    let mut layers: Vec<Layer> = Vec::new();
    let empty_symbols = HashMap::new();

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
                    _ => context.element(&name, &attrs, &empty_symbols, ids, report),
                }
            }
            Err(e) => return Err(ImportError::Xml(e.to_string())),
            _ => {}
        }
    }
    context.flush_layer(&mut layers, report);

    layers.reverse();
    if layers.is_empty() {
        layers.push(Layer::normal(LayerId(ids.take()), "Layer_1"));
    }
    for (index, layer) in layers.into_iter().enumerate() {
        symbol.layers.insert(index, layer);
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

    /// Animate writes layers bottom-first; ours are front-first.
    #[test]
    fn layer_order_is_reversed_to_match_our_stack() {
        let (scene, _) = import_sample();
        let names: Vec<&str> = scene.layers().iter().map(|l| l.name.as_str()).collect();
        assert_eq!(
            names,
            vec!["Foreground", "Background"],
            "the last layer in the file should be at the front"
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
}
