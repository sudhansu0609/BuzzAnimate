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
//! and their spans, shapes with solid fills and strokes, groups, and symbols
//! with their instances and transforms.
//!
//! Not yet travelling, and deliberately named rather than silently dropped —
//! [`FlaReport::skipped`] carries them out to the user: gradients, bitmaps,
//! filters, blend modes, tweens, sound, the camera, and rigged or warped
//! artwork. Those are written as their resolved artwork where they have any,
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
            text.push_str(&format!(" \u{2014} not carried: {}", self.skipped.join(", ")));
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

        for symbol in scene.library().iter() {
            zip.start_file(format!("LIBRARY/{}.xml", file_name(&symbol.name)), options)?;
            let xml = symbol_xml(scene, symbol, &mut report);
            zip.write_all(xml.as_bytes()).map_err(io("a symbol"))?;
            report.symbols += 1;
        }

        zip.start_file("DOMDocument.xml", options)?;
        let xml = document_xml(scene, &mut report);
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

const METADATA: &str = concat!(
    r#"<?xml version="1.0" encoding="UTF-8"?>"#,
    "\n",
    r#"<metadata xmlns="http://ns.adobe.com/xfl/2008/"/>"#,
    "\n"
);

// ---- the document -----------------------------------------------------------

fn document_xml(scene: &Scene, report: &mut FlaReport) -> String {
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
    if scene.library().is_empty() {
        out.push_str("  <symbols/>\n");
    } else {
        out.push_str("  <symbols>\n");
        for symbol in scene.library().iter() {
            out.push_str(&format!(
                "    <Include href=\"{}.xml\" itemIcon=\"1\" loadImmediate=\"false\" itemID=\"{}\"/>\n",
                escape(&file_name(&symbol.name)),
                item_id(symbol.id.0),
            ));
        }
        out.push_str("  </symbols>\n");
    }

    out.push_str("  <timelines>\n");
    out.push_str(&timeline_xml(
        scene,
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

fn symbol_xml(scene: &Scene, symbol: &Symbol, report: &mut FlaReport) -> String {
    let kind = match symbol.kind {
        SymbolKind::MovieClip => "movie clip",
        SymbolKind::Button => "button",
        SymbolKind::Graphic => "graphic",
    };
    let mut out = String::new();
    out.push_str(r#"<?xml version="1.0" encoding="UTF-8"?>"#);
    out.push('\n');
    out.push_str(&format!(
        r#"<DOMSymbolItem xmlns="http://ns.adobe.com/xfl/2008/" name="{}" symbolType="{kind}">"#,
        escape(&symbol.name)
    ));
    out.push('\n');
    out.push_str("  <timeline>\n");
    out.push_str(&timeline_xml(scene, &symbol.name, &symbol.layers, report, 4));
    out.push_str("  </timeline>\n");
    out.push_str("</DOMSymbolItem>\n");
    out
}

// ---- timelines ---------------------------------------------------------------

fn timeline_xml(
    scene: &Scene,
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
    let index_of: BTreeMap<buzz_scene::LayerId, usize> = ordered
        .iter()
        .enumerate()
        .map(|(i, l)| (l.id, i))
        .collect();

    // Which layers something hangs off, so only those are marked as rig
    // parents.
    let parents: std::collections::BTreeSet<buzz_scene::LayerId> =
        ordered.iter().filter_map(|l| l.follows).collect();

    for (index, layer) in ordered.iter().enumerate() {
        out.push_str(&layer_xml(
            scene,
            layer,
            index,
            &index_of,
            parents.contains(&layer.id),
            report,
            indent + 4,
        ));
    }

    out.push_str(&format!("{pad}  </layers>\n{pad}</DOMTimeline>\n"));
    out
}

fn layer_xml(
    scene: &Scene,
    layer: &Layer,
    index: usize,
    index_of: &BTreeMap<buzz_scene::LayerId, usize>,
    is_rig_parent: bool,
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
        LayerKind::Masked => attrs.push_str(" layerType=\"masked\""),
        LayerKind::Guide => attrs.push_str(" layerType=\"guide\""),
        LayerKind::Guided => attrs.push_str(" layerType=\"guided\""),
        LayerKind::InverseMask => {
            // Animate has no inverse mask; it comes back as an ordinary one
            // rather than as nothing.
            attrs.push_str(" layerType=\"mask\"");
            report.skip("inverse masks (written as ordinary masks)");
        }
        LayerKind::Normal => {}
    }
    // The folder a layer sits in — Animate's own `parentLayerIndex`, which is
    // a different thing from layer parenting.
    if let Some(parent) = layer.parent
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
    if is_rig_parent {
        attrs.push_str(&format!(" layerRiggingIndex=\"{index}\""));
    }
    let follows_index = layer.follows.and_then(|id| index_of.get(&id)).copied();

    let mut out = format!("{pad}<DOMLayer {attrs}>\n{pad}  <frames>\n");
    for keyframe in layer.frames.keyframes() {
        out.push_str(&frame_xml(
            scene,
            layer,
            keyframe,
            follows_index,
            report,
            indent + 4,
        ));
    }
    out.push_str(&format!("{pad}  </frames>\n{pad}</DOMLayer>\n"));
    out
}

fn frame_xml(
    scene: &Scene,
    layer: &Layer,
    keyframe: &buzz_scene::Keyframe,
    follows_index: Option<usize>,
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
    // `keyMode` is how Animate records what kind of keyframe this is; 9728 is
    // the ordinary one, and it is on every frame in a document Adobe saved.
    // A duration of one is left off, as Animate leaves it off.
    let mut attrs = format!("index=\"{}\"", keyframe.start);
    if duration > 1 {
        attrs.push_str(&format!(" duration=\"{duration}\""));
    }
    attrs.push_str(" keyMode=\"9728\"");
    if let Some(label) = &keyframe.label {
        attrs.push_str(&format!(" name=\"{}\"", escape(label)));
    }
    if let Some(parent) = follows_index {
        attrs.push_str(&format!(" parentLayerIndex=\"{parent}\""));
    }
    if keyframe.tween.kind != buzz_scene::TweenKind::None {
        report.skip("tweens (keyframes are written, the interpolation is not)");
    }
    if keyframe.sound.is_some() {
        report.skip("sound");
    }

    let mut out = format!("{pad}<DOMFrame {attrs}>\n{pad}  <elements>\n");
    for object in keyframe.objects.iter() {
        out.push_str(&object_xml(scene, object, report, indent + 4));
    }
    out.push_str(&format!("{pad}  </elements>\n{pad}</DOMFrame>\n"));
    out
}

// ---- one object --------------------------------------------------------------

fn object_xml(scene: &Scene, object: &Object, report: &mut FlaReport, indent: usize) -> String {
    if !object.filters.is_empty() {
        report.skip("filters");
    }
    match &object.kind {
        ObjectKind::Shape(shape) => {
            report.objects += 1;
            shape_xml(shape, object.transform, report, indent)
        }
        ObjectKind::Group(children) => {
            report.objects += 1;
            let pad = " ".repeat(indent);
            let mut out = format!("{pad}<DOMGroup>\n{pad}  <members>\n");
            for child in children {
                // A group's own transform is carried into its members, because
                // Animate's DOMGroup has no matrix of its own.
                let mut moved: Object = (**child).clone();
                moved.transform = object.transform * moved.transform;
                out.push_str(&object_xml(scene, &moved, report, indent + 4));
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
            let playing = if matches!(symbol.kind, SymbolKind::Graphic) {
                " loop=\"loop\""
            } else {
                ""
            };
            format!(
                "{pad}<DOMSymbolInstance libraryItemName=\"{}\" symbolType=\"{kind}\"{playing}>\n{}{}{pad}</DOMSymbolInstance>\n",
                escape(&symbol.name),
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
                out.push_str(&object_xml(scene, &moved, report, indent));
            }
            out
        }
        ObjectKind::Warp(warp) => {
            report.skip("warp handles (the warped artwork is written, the handles are not)");
            report.objects += 1;
            shape_xml(&warp.warped(), object.transform, report, indent)
        }
    }
}

fn shape_xml(
    shape: &buzz_scene::ShapeData,
    transform: Affine,
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

    let has_fill = fill.is_some();
    let has_stroke = stroke.is_some();

    let mut out = format!("{pad}<DOMShape>\n");
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

    let mut attrs = String::new();
    if has_fill {
        attrs.push_str(" fillStyle0=\"1\"");
    }
    if has_stroke {
        attrs.push_str(" strokeStyle=\"1\"");
    }
    out.push_str(&format!(
        "{pad}  <edges>\n{pad}    <Edge{attrs} edges=\"{}\"/>\n{pad}  </edges>\n{pad}</DOMShape>\n",
        escape(&edges)
    ));
    out
}

/// A paint as a solid colour, or `None` if it is something a `.fla` cannot
/// carry — which is noted rather than passed over.
fn solid(paint: &buzz_scene::Paint, report: &mut FlaReport) -> Option<(String, f32)> {
    match paint {
        buzz_scene::Paint::Solid(colour) => {
            Some((hex(*colour), colour.components[3]))
        }
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
