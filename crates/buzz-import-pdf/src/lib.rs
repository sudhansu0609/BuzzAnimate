//! Import PDF and Adobe Illustrator artwork as editable vectors.
//!
//! # One parser for two formats
//!
//! Illustrator has written PDF internally since version 9, wrapped in its own
//! container with a private stream Illustrator itself reads back. The *drawing*
//! is ordinary PDF, so one content-stream interpreter covers both. Files from
//! Illustrator 8 and earlier are PostScript, a different language altogether;
//! those are detected and refused with a message that says so rather than
//! producing an empty document.
//!
//! # What this reads
//!
//! Vector artwork: the path-construction and path-painting operators, the
//! graphics state that governs them, colour in Gray, RGB and CMYK, and Form
//! XObjects, which Illustrator uses heavily. Every page becomes a keyframe on
//! one layer, so a multi-page document arrives as a sequence you can step
//! through.
//!
//! Text and images are **reported, not silently dropped** — see
//! [`ImportReport::unsupported`]. Text needs the font subsystem that BuzzAnimate
//! does not have yet, and turning glyphs into paths without it would produce
//! artwork the user cannot edit back into words.
//!
//! # Coordinates
//!
//! PDF puts the origin at the bottom-left with y increasing upwards; the stage
//! puts it at the top-left with y increasing downwards. Every page therefore
//! carries a flip derived from its MediaBox, applied once as the base
//! transform, so imported artwork lands the right way up at the right place.
//!
//! # Precision
//!
//! `lopdf` decodes PDF reals as `f32`, and everything here widens to `f64` at
//! once and stays there. That is not a loss: PDF's own syntax specifies about
//! five significant decimal digits for reals, well inside `f32`, so the file
//! never carried more precision than arrives. Arithmetic — matrix composition,
//! Bézier construction — all happens in `f64`.

use std::collections::BTreeMap;
use std::path::Path;

use buzz_geom::{Affine, BezPath, Point};
use buzz_scene::{FillSpec, Layer, LayerId, Object, ObjectId, Scene, ShapeData, StrokeSpec};
use lopdf::{Document as PdfDocument, Object as PdfObject, ObjectId as PdfObjectId};
use peniko::Color;

/// Why an import failed outright.
#[derive(Debug, thiserror::Error)]
pub enum ImportError {
    #[error("input/output error: {0}")]
    Io(#[from] std::io::Error),
    #[error("this file could not be read as a PDF: {0}")]
    Pdf(String),
    #[error(
        "this is a PostScript-based Illustrator file (version 8 or earlier), \
         which is a different format from the PDF that later versions write. \
         Open it in Illustrator and re-save it with \"Create PDF Compatible \
         File\" turned on."
    )]
    LegacyPostScript,
    #[error("the document has no pages")]
    NoPages,
    #[error("the document is encrypted and cannot be read without its password")]
    Encrypted,
}

impl From<lopdf::Error> for ImportError {
    fn from(e: lopdf::Error) -> Self {
        // lopdf reports a missing decryption key through its ordinary error
        // type; saying "encrypted" is far more use than the raw message.
        let text = e.to_string();
        if text.to_lowercase().contains("encrypt") {
            return Self::Encrypted;
        }
        Self::Pdf(text)
    }
}

/// What came across, and what did not.
#[derive(Debug, Default, Clone)]
pub struct ImportReport {
    pub pages: usize,
    pub paths: usize,
    pub fills: usize,
    pub strokes: usize,
    /// Features present in the file that this importer does not handle.
    pub unsupported: Vec<String>,
}

impl ImportReport {
    fn note_unsupported(&mut self, what: &str) {
        // One line per kind with a count, rather than thousands of repeats.
        if let Some(existing) = self
            .unsupported
            .iter_mut()
            .find(|e| e.as_str() == what || e.starts_with(&format!("{what} (x")))
        {
            let count = existing
                .rsplit_once(" (x")
                .and_then(|(_, n)| n.trim_end_matches(')').parse::<usize>().ok())
                .unwrap_or(1);
            *existing = format!("{what} (x{})", count + 1);
        } else {
            self.unsupported.push(what.to_string());
        }
    }

    pub fn is_complete(&self) -> bool {
        self.unsupported.is_empty()
    }

    pub fn summary(&self) -> String {
        format!(
            "{} pages, {} paths ({} filled, {} stroked)",
            self.pages, self.paths, self.fills, self.strokes
        )
    }
}

/// Import a `.pdf` or `.ai` file.
pub fn import(path: impl AsRef<Path>) -> Result<(Scene, ImportReport), ImportError> {
    let bytes = std::fs::read(path.as_ref())?;
    import_bytes(&bytes)
}

/// Import from bytes.
pub fn import_bytes(bytes: &[u8]) -> Result<(Scene, ImportReport), ImportError> {
    // A pre-v9 Illustrator file is PostScript and begins with the PS banner.
    // A PDF begins with `%PDF-`. Checking the front of the file lets us say
    // which of the two this is instead of failing with a parse error.
    if bytes.starts_with(b"%!PS") {
        return Err(ImportError::LegacyPostScript);
    }

    let doc = PdfDocument::load_mem(bytes)?;
    build(&doc)
}

fn build(doc: &PdfDocument) -> Result<(Scene, ImportReport), ImportError> {
    let mut report = ImportReport::default();
    let mut scene = Scene::empty();
    let mut ids = IdSource::default();

    let pages: BTreeMap<u32, PdfObjectId> = doc.get_pages();
    if pages.is_empty() {
        return Err(ImportError::NoPages);
    }

    // The first page sets the stage size, which is what a user expects when
    // importing a document that is mostly one page.
    let first = media_box(doc, *pages.values().next().expect("checked non-empty"));
    scene.stage_mut().size = buzz_geom::Size::new(first.2 - first.0, first.3 - first.1);

    let mut layer = Layer::normal(LayerId(ids.take()), "Artwork");
    let mut keyframes: Vec<buzz_scene::Keyframe> = Vec::new();

    for (index, (_number, page_id)) in pages.iter().enumerate() {
        let (x0, y0, _x1, y1) = media_box(doc, *page_id);

        // PDF is y-up from the bottom-left; the stage is y-down from the
        // top-left. One flip, applied to everything on the page.
        let base = Affine::new([1.0, 0.0, 0.0, -1.0, -x0, y1 - y0 + y0]);

        let mut objects = Vec::new();
        match doc.get_and_decode_page_content(*page_id) {
            Ok(content) => {
                let mut interp = Interpreter::new(doc, &mut ids, &mut report);
                interp.run(&content.operations, base, *page_id, 0, &mut objects);
            }
            Err(e) => {
                report.note_unsupported(&format!("a page whose content could not be decoded ({e})"));
            }
        }

        report.pages += 1;
        keyframes.push(buzz_scene::Keyframe {
            start: index as u32,
            objects: std::sync::Arc::new(objects),
            label: None,
            tween: buzz_scene::Tween::default(),
        });
    }

    layer.frames = buzz_scene::LayerTimeline::from_parts(keyframes, pages.len().max(1) as u32);
    scene.edit_stage_layers().insert(0, layer);
    scene.reserve_ids_above(ids.peek());

    Ok((scene, report))
}

/// A page's MediaBox as `(x0, y0, x1, y1)`, falling back to US Letter.
///
/// MediaBox is an *inheritable* attribute: a page may not carry one and expect
/// to take its parent's. Walking up the page tree is therefore not optional —
/// without it, real documents silently import at the wrong size.
fn media_box(doc: &PdfDocument, page_id: PdfObjectId) -> (f64, f64, f64, f64) {
    const LETTER: (f64, f64, f64, f64) = (0.0, 0.0, 612.0, 792.0);

    let mut current = Some(page_id);
    // Bounded, so a file whose page tree loops cannot hang the importer.
    for _ in 0..32 {
        let Some(id) = current else { break };
        let Ok(dict) = doc.get_dictionary(id) else {
            break;
        };

        if let Ok(value) = dict.get_deref(b"MediaBox", doc)
            && let Ok(array) = value.as_array()
            && array.len() == 4
        {
            let n = |i: usize| array[i].as_float().unwrap_or(0.0) as f64;
            let (a, b, c, d) = (n(0), n(1), n(2), n(3));
            // PDF allows the corners in either order; normalise so width and
            // height cannot come out negative.
            return (a.min(c), b.min(d), a.max(c), b.max(d));
        }

        current = match dict.get(b"Parent") {
            Ok(PdfObject::Reference(parent)) => Some(*parent),
            _ => None,
        };
    }
    LETTER
}

/// Hands out object and layer ids for the scene being built.
#[derive(Default)]
struct IdSource(u64);

impl IdSource {
    fn take(&mut self) -> u64 {
        self.0 += 1;
        self.0
    }
    fn peek(&self) -> u64 {
        self.0
    }
}

/// PDF's graphics state, or the part of it that reaches vector artwork.
#[derive(Clone)]
struct GraphicsState {
    transform: Affine,
    fill: Color,
    stroke: Color,
    line_width: f64,
    /// PDF's `CA`/`ca`, which multiply the colours' own alpha.
    fill_alpha: f32,
    stroke_alpha: f32,
}

impl GraphicsState {
    fn new(transform: Affine) -> Self {
        Self {
            transform,
            // PDF's initial colour is black for both fill and stroke.
            fill: Color::BLACK,
            stroke: Color::BLACK,
            line_width: 1.0,
            fill_alpha: 1.0,
            stroke_alpha: 1.0,
        }
    }
}

/// How deep Form XObjects may nest before we stop following them.
const MAX_FORM_DEPTH: usize = 12;

/// Walks a content stream, building paths.
struct Interpreter<'a> {
    doc: &'a PdfDocument,
    ids: &'a mut IdSource,
    report: &'a mut ImportReport,
}

impl<'a> Interpreter<'a> {
    fn new(doc: &'a PdfDocument, ids: &'a mut IdSource, report: &'a mut ImportReport) -> Self {
        Self { doc, ids, report }
    }

    /// Interpret one content stream.
    ///
    /// `resources` names the object whose `/Resources` dictionary resolves
    /// XObject lookups, which is the page for page content and the form itself
    /// for a form's content.
    fn run(
        &mut self,
        operations: &[lopdf::content::Operation],
        base: Affine,
        resources: PdfObjectId,
        depth: usize,
        out: &mut Vec<std::sync::Arc<Object>>,
    ) {
        let mut state = GraphicsState::new(base);
        let mut stack: Vec<GraphicsState> = Vec::new();

        // The path being constructed, in *user space* — it is transformed at
        // painting time, because PDF applies the CTM in force when the path is
        // painted, not when each segment is added.
        let mut path = BezPath::new();
        let mut start = Point::ZERO;
        let mut current = Point::ZERO;
        let mut in_text = false;

        for op in operations {
            let n = |i: usize| -> f64 {
                op.operands
                    .get(i)
                    .and_then(|o| o.as_float().ok())
                    .unwrap_or(0.0) as f64
            };

            match op.operator.as_str() {
                // -- graphics state ------------------------------------------
                "q" => stack.push(state.clone()),
                "Q" => {
                    if let Some(previous) = stack.pop() {
                        state = previous;
                    }
                }
                "cm" if op.operands.len() >= 6 => {
                    let m = Affine::new([n(0), n(1), n(2), n(3), n(4), n(5)]);
                    state.transform *= m;
                }
                "w" => state.line_width = n(0),
                "gs" => {
                    // An ExtGState can carry alpha, which is common enough in
                    // Illustrator output to be worth following.
                    self.apply_ext_gstate(&op.operands, resources, &mut state);
                }

                // -- colour ---------------------------------------------------
                "g" => state.fill = gray(n(0), state.fill_alpha),
                "G" => state.stroke = gray(n(0), state.stroke_alpha),
                "rg" => state.fill = rgb(n(0), n(1), n(2), state.fill_alpha),
                "RG" => state.stroke = rgb(n(0), n(1), n(2), state.stroke_alpha),
                "k" => state.fill = cmyk(n(0), n(1), n(2), n(3), state.fill_alpha),
                "K" => state.stroke = cmyk(n(0), n(1), n(2), n(3), state.stroke_alpha),
                // `sc`/`scn` set a colour in whatever space `cs` selected. The
                // operand count identifies the family for the three device
                // spaces, which is what nearly every file uses; a named
                // pattern or separation has no numeric operands and is
                // reported rather than guessed at.
                "sc" | "scn" => match op.operands.len() {
                    1 => state.fill = gray(n(0), state.fill_alpha),
                    3 => state.fill = rgb(n(0), n(1), n(2), state.fill_alpha),
                    4 => state.fill = cmyk(n(0), n(1), n(2), n(3), state.fill_alpha),
                    _ => self.report.note_unsupported("a pattern or separation fill"),
                },
                "SC" | "SCN" => match op.operands.len() {
                    1 => state.stroke = gray(n(0), state.stroke_alpha),
                    3 => state.stroke = rgb(n(0), n(1), n(2), state.stroke_alpha),
                    4 => state.stroke = cmyk(n(0), n(1), n(2), n(3), state.stroke_alpha),
                    _ => self.report.note_unsupported("a pattern or separation stroke"),
                },

                // -- path construction ----------------------------------------
                "m" => {
                    start = Point::new(n(0), n(1));
                    current = start;
                    path.move_to(start);
                }
                "l" => {
                    current = Point::new(n(0), n(1));
                    path.line_to(current);
                }
                "c" if op.operands.len() >= 6 => {
                    let (c1, c2, end) = (
                        Point::new(n(0), n(1)),
                        Point::new(n(2), n(3)),
                        Point::new(n(4), n(5)),
                    );
                    ensure_started(&mut path, current);
                    path.curve_to(c1, c2, end);
                    current = end;
                }
                // `v` uses the current point as the first control point.
                "v" if op.operands.len() >= 4 => {
                    let (c2, end) = (Point::new(n(0), n(1)), Point::new(n(2), n(3)));
                    ensure_started(&mut path, current);
                    path.curve_to(current, c2, end);
                    current = end;
                }
                // `y` uses the end point as the second control point.
                "y" if op.operands.len() >= 4 => {
                    let (c1, end) = (Point::new(n(0), n(1)), Point::new(n(2), n(3)));
                    ensure_started(&mut path, current);
                    path.curve_to(c1, end, end);
                    current = end;
                }
                "h" => {
                    if !path.is_empty() {
                        path.close_path();
                        current = start;
                    }
                }
                "re" if op.operands.len() >= 4 => {
                    let (x, y, w, h) = (n(0), n(1), n(2), n(3));
                    path.move_to(Point::new(x, y));
                    path.line_to(Point::new(x + w, y));
                    path.line_to(Point::new(x + w, y + h));
                    path.line_to(Point::new(x, y + h));
                    path.close_path();
                    start = Point::new(x, y);
                    current = start;
                }

                // -- path painting --------------------------------------------
                // The operator says whether to fill, stroke, or both, and
                // whether to close first. Every one of them ends the path.
                "f" | "F" | "f*" | "S" | "s" | "B" | "B*" | "b" | "b*" | "n" => {
                    let operator = op.operator.as_str();
                    if matches!(operator, "s" | "b" | "b*") && !path.is_empty() {
                        path.close_path();
                    }
                    let fills = matches!(operator, "f" | "F" | "f*" | "B" | "B*" | "b" | "b*");
                    let strokes = matches!(operator, "S" | "s" | "B" | "B*" | "b" | "b*");

                    if !path.is_empty() && (fills || strokes) {
                        self.emit(&path, &state, fills, strokes, out);
                    }
                    path = BezPath::new();
                }
                // `W`/`W*` set a clip from the current path. The clip applies
                // at the *next* painting operator, which has already been
                // handled above; recording it is honest about what was lost.
                "W" | "W*" => self.report.note_unsupported("a clipping path"),

                // -- things we knowingly do not read --------------------------
                "BT" => {
                    in_text = true;
                    self.report.note_unsupported("text");
                }
                "ET" => in_text = false,
                "Do" => self.handle_xobject(&op.operands, &state, resources, depth, out),
                "sh" => self.report.note_unsupported("a gradient (shading)"),
                "BI" => self.report.note_unsupported("an inline image"),
                _ => {}
            }

            // Text-positioning operators inside BT/ET are numerous; noting
            // each one would bury the report under noise. One "text" entry per
            // block is the useful signal.
            let _ = in_text;
        }
    }

    /// Turn the constructed path into a scene object.
    fn emit(
        &mut self,
        path: &BezPath,
        state: &GraphicsState,
        fills: bool,
        strokes: bool,
        out: &mut Vec<std::sync::Arc<Object>>,
    ) {
        // Baked into the geometry rather than left on the object's transform:
        // PDF paths carry no reusable identity, and a flat path is what every
        // editing tool in the application expects to work on.
        let placed = state.transform * path.clone();

        let mut shape = ShapeData {
            path: placed,
            fill: None,
            stroke: None,
        };
        if fills {
            shape.fill = Some(FillSpec::solid(state.fill));
            self.report.fills += 1;
        }
        if strokes {
            // Line width is in user space, so the transform scales it. Using
            // the average of the two axis scales keeps a stroke sensible under
            // a non-uniform matrix, where a single width cannot be exact.
            let scale = transform_scale(state.transform);
            let width = (state.line_width * scale).max(0.0);
            shape.stroke = Some(StrokeSpec::new(state.stroke, width));
            self.report.strokes += 1;
        }

        self.report.paths += 1;
        out.push(std::sync::Arc::new(Object::shape(
            ObjectId(self.ids.take()),
            shape,
        )));
    }

    /// Follow a `Do` operator into a Form XObject.
    ///
    /// Illustrator wraps most artwork in these, so not following them would
    /// import a great many files as blank.
    fn handle_xobject(
        &mut self,
        operands: &[PdfObject],
        state: &GraphicsState,
        resources: PdfObjectId,
        depth: usize,
        out: &mut Vec<std::sync::Arc<Object>>,
    ) {
        let Some(name) = operands.first().and_then(|o| o.as_name().ok()) else {
            return;
        };

        let Some(stream_id) = self.lookup_xobject(name, resources) else {
            self.report.note_unsupported("a form or image that could not be resolved");
            return;
        };

        let Ok(stream) = self.doc.get_object(stream_id).and_then(|o| o.as_stream()) else {
            return;
        };

        let subtype = stream.dict.get(b"Subtype").ok().and_then(|o| o.as_name().ok());
        match subtype {
            Some(b"Image") => {
                self.report.note_unsupported("an embedded image");
                return;
            }
            Some(b"Form") => {}
            _ => return,
        }

        if depth >= MAX_FORM_DEPTH {
            // A form that draws itself is a cycle; a bound is the only defence
            // against a file that contains one.
            self.report.note_unsupported("a form nested too deeply to follow");
            return;
        }

        // A form carries its own matrix, applied before its content.
        let mut inner = state.transform;
        if let Ok(matrix) = stream.dict.get(b"Matrix").and_then(|o| o.as_array())
            && matrix.len() == 6
        {
            let v = |i: usize| matrix[i].as_float().unwrap_or(0.0) as f64;
            inner *= Affine::new([v(0), v(1), v(2), v(3), v(4), v(5)]);
        }

        // `decode_content` needs the filters applied first.
        let decoded = stream.decompressed_content().unwrap_or_else(|_| stream.content.clone());
        let Ok(content) = lopdf::content::Content::decode(&decoded) else {
            self.report.note_unsupported("a form whose content could not be decoded");
            return;
        };

        self.run(&content.operations, inner, stream_id, depth + 1, out);
    }

    /// Find an XObject by name in the resource dictionary.
    fn lookup_xobject(&self, name: &[u8], resources: PdfObjectId) -> Option<PdfObjectId> {
        let dict = self.doc.get_dictionary(resources).ok()?;
        let resources = dict.get_deref(b"Resources", self.doc).ok()?.as_dict().ok()?;
        let xobjects = resources.get_deref(b"XObject", self.doc).ok()?.as_dict().ok()?;
        match xobjects.get(name).ok()? {
            PdfObject::Reference(id) => Some(*id),
            _ => None,
        }
    }

    /// Read alpha out of an ExtGState.
    fn apply_ext_gstate(
        &mut self,
        operands: &[PdfObject],
        resources: PdfObjectId,
        state: &mut GraphicsState,
    ) {
        let Some(name) = operands.first().and_then(|o| o.as_name().ok()) else {
            return;
        };
        let Some(gs) = (|| {
            let dict = self.doc.get_dictionary(resources).ok()?;
            let resources = dict.get_deref(b"Resources", self.doc).ok()?.as_dict().ok()?;
            let states = resources.get_deref(b"ExtGState", self.doc).ok()?.as_dict().ok()?;
            states.get_deref(name, self.doc).ok()?.as_dict().ok()
        })() else {
            return;
        };

        if let Ok(a) = gs.get(b"ca").and_then(|o| o.as_float()) {
            state.fill_alpha = a.clamp(0.0, 1.0);
            state.fill = state.fill.multiply_alpha(state.fill_alpha);
        }
        if let Ok(a) = gs.get(b"CA").and_then(|o| o.as_float()) {
            state.stroke_alpha = a.clamp(0.0, 1.0);
            state.stroke = state.stroke.multiply_alpha(state.stroke_alpha);
        }
        if let Ok(w) = gs.get(b"LW").and_then(|o| o.as_float()) {
            state.line_width = w as f64;
        }
    }
}

/// A `move_to` is required before any curve; a content stream that omits it is
/// malformed, but refusing to draw is worse than starting where we are.
fn ensure_started(path: &mut BezPath, current: Point) {
    if path.is_empty() {
        path.move_to(current);
    }
}

/// How much a transform scales, as one number.
///
/// The geometric mean of the two axis lengths: for a uniform scale it is that
/// scale exactly, and for a non-uniform one it is the value that preserves
/// area, which is the least wrong single answer for a stroke width.
fn transform_scale(t: Affine) -> f64 {
    let c = t.as_coeffs();
    let sx = (c[0] * c[0] + c[1] * c[1]).sqrt();
    let sy = (c[2] * c[2] + c[3] * c[3]).sqrt();
    (sx * sy).sqrt()
}

fn channel(v: f64) -> u8 {
    (v.clamp(0.0, 1.0) * 255.0).round() as u8
}

fn gray(v: f64, alpha: f32) -> Color {
    let c = channel(v);
    Color::from_rgba8(c, c, c, 255).multiply_alpha(alpha)
}

fn rgb(r: f64, g: f64, b: f64, alpha: f32) -> Color {
    Color::from_rgba8(channel(r), channel(g), channel(b), 255).multiply_alpha(alpha)
}

/// PDF's DeviceCMYK, by the conversion the specification itself gives.
///
/// This is the naive formula, and it is the right choice here: a
/// colour-managed conversion needs the output profile, which an authoring tool
/// targeting a screen does not have. Being predictable beats being subtly
/// wrong in a way the user cannot correct.
fn cmyk(c: f64, m: f64, y: f64, k: f64, alpha: f32) -> Color {
    Color::from_rgba8(
        channel((1.0 - c) * (1.0 - k)),
        channel((1.0 - m) * (1.0 - k)),
        channel((1.0 - y) * (1.0 - k)),
        255,
    )
    .multiply_alpha(alpha)
}

#[cfg(test)]
mod tests {
    use super::*;
    use buzz_geom::Shape as _;

    /// Build a single-page PDF around a content stream, uncompressed so the
    /// test reads as the PDF it is.
    fn pdf_with(content: &str, media_box: &str) -> Vec<u8> {
        let mut out = String::from("%PDF-1.7\n");
        let mut offsets = Vec::new();

        let objects = [
            "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
            format!("<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox {media_box} >>"),
            "<< /Type /Page /Parent 2 0 R /Contents 4 0 R /Resources << >> >>".to_string(),
            format!("<< /Length {} >>\nstream\n{content}\nendstream", content.len()),
        ];

        for (i, body) in objects.iter().enumerate() {
            offsets.push(out.len());
            out.push_str(&format!("{} 0 obj\n{body}\nendobj\n", i + 1));
        }

        let xref = out.len();
        out.push_str(&format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1));
        for offset in &offsets {
            out.push_str(&format!("{offset:010} 00000 n \n"));
        }
        out.push_str(&format!(
            "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n",
            objects.len() + 1
        ));
        out.into_bytes()
    }

    fn shapes(scene: &Scene) -> Vec<ShapeData> {
        scene
            .stage_layers()
            .iter()
            .flat_map(|l| l.all_objects())
            .filter_map(|o| match &o.kind {
                buzz_scene::ObjectKind::Shape(s) => Some(s.clone()),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn a_filled_rectangle_arrives_as_a_filled_path() {
        let pdf = pdf_with("1 0 0 rg\n10 20 100 50 re\nf", "[0 0 612 792]");
        let (scene, report) = import_bytes(&pdf).expect("the PDF parses");

        assert_eq!(report.pages, 1);
        assert_eq!(report.paths, 1);
        assert_eq!(report.fills, 1);
        assert_eq!(report.strokes, 0);

        let shapes = shapes(&scene);
        assert_eq!(shapes.len(), 1);
        let fill = shapes[0].fill.expect("it is filled");
        assert_eq!(fill.color.to_rgba8().to_u8_array()[..3], [255, 0, 0]);
    }

    /// The single most common way to get an import subtly wrong: PDF measures
    /// y upwards from the bottom, the stage measures it downwards from the
    /// top, so artwork lands mirrored unless the page is flipped.
    #[test]
    fn the_page_is_flipped_so_artwork_is_not_upside_down() {
        // A rectangle sitting near the *bottom* of a 792-high page.
        let pdf = pdf_with("0 0 100 50 re\nf", "[0 0 612 792]");
        let (scene, _) = import_bytes(&pdf).unwrap();

        let bounds = shapes(&scene)[0].path.bounding_box();
        assert!(
            bounds.y0 > 700.0,
            "a shape at the bottom of the page must arrive near the bottom \
             of the stage, not the top; got {bounds:?}"
        );
        assert!((bounds.height() - 50.0).abs() < 1e-6, "and keep its height");
        assert!((bounds.width() - 100.0).abs() < 1e-6);
    }

    #[test]
    fn the_stage_takes_its_size_from_the_media_box() {
        let pdf = pdf_with("0 0 10 10 re\nf", "[0 0 300 200]");
        let (scene, _) = import_bytes(&pdf).unwrap();
        assert_eq!(scene.stage().size.width, 300.0);
        assert_eq!(scene.stage().size.height, 200.0);
    }

    /// MediaBox is inheritable, and here it lives only on the Pages node. A
    /// reader that looks at the page alone would silently use a default size.
    #[test]
    fn a_media_box_inherited_from_the_page_tree_is_found() {
        let pdf = pdf_with("0 0 10 10 re\nf", "[0 0 400 500]");
        let (scene, _) = import_bytes(&pdf).unwrap();
        assert_eq!(
            (scene.stage().size.width, scene.stage().size.height),
            (400.0, 500.0),
            "the page has no MediaBox of its own; it must inherit"
        );
    }

    #[test]
    fn curves_come_across_as_curves_not_polygons() {
        let pdf = pdf_with("10 10 m\n20 20 30 30 40 40 c\nS", "[0 0 612 792]");
        let (scene, report) = import_bytes(&pdf).unwrap();

        assert_eq!(report.strokes, 1);
        let path = &shapes(&scene)[0].path;
        let curves = path
            .elements()
            .iter()
            .filter(|e| matches!(e, kurbo::PathEl::CurveTo(..)))
            .count();
        assert_eq!(curves, 1, "the cubic must survive as a cubic");
    }

    #[test]
    fn the_graphics_state_stack_restores_colour_and_transform() {
        // Red inside q/Q, then a second shape that must still be black.
        let pdf = pdf_with(
            "q\n1 0 0 rg\n0 0 10 10 re\nf\nQ\n0 0 10 10 re\nf",
            "[0 0 612 792]",
        );
        let (scene, _) = import_bytes(&pdf).unwrap();
        let shapes = shapes(&scene);
        assert_eq!(shapes.len(), 2);

        assert_eq!(shapes[0].fill.unwrap().color.to_rgba8().to_u8_array()[..3], [255, 0, 0]);
        assert_eq!(
            shapes[1].fill.unwrap().color.to_rgba8().to_u8_array()[..3],
            [0, 0, 0],
            "Q must restore the colour set before q"
        );
    }

    #[test]
    fn a_transform_moves_the_artwork() {
        let pdf = pdf_with("1 0 0 1 100 0 cm\n0 0 10 10 re\nf", "[0 0 612 792]");
        let (scene, _) = import_bytes(&pdf).unwrap();
        let bounds = shapes(&scene)[0].path.bounding_box();
        assert!(
            (bounds.x0 - 100.0).abs() < 1e-6,
            "the cm translation should apply; got {bounds:?}"
        );
    }

    #[test]
    fn cmyk_black_is_black_and_cmyk_white_is_white() {
        let pdf = pdf_with("0 0 0 1 k\n0 0 10 10 re\nf\n0 0 0 0 k\n0 0 10 10 re\nf", "[0 0 612 792]");
        let (scene, _) = import_bytes(&pdf).unwrap();
        let shapes = shapes(&scene);
        assert_eq!(shapes[0].fill.unwrap().color.to_rgba8().to_u8_array()[..3], [0, 0, 0]);
        assert_eq!(shapes[1].fill.unwrap().color.to_rgba8().to_u8_array()[..3], [255, 255, 255]);
    }

    #[test]
    fn fill_and_stroke_together_produce_one_shape_with_both() {
        let pdf = pdf_with("1 0 0 rg\n0 0 1 RG\n5 w\n0 0 10 10 re\nB", "[0 0 612 792]");
        let (scene, report) = import_bytes(&pdf).unwrap();

        assert_eq!(report.paths, 1, "B is one path, not two");
        assert_eq!((report.fills, report.strokes), (1, 1));

        let shape = &shapes(&scene)[0];
        assert!(shape.fill.is_some() && shape.stroke.is_some());
        assert_eq!(shape.stroke.unwrap().width, 5.0);
    }

    /// `n` ends a path without painting it. Emitting it anyway would fill
    /// every clipping rectangle in the document with black.
    #[test]
    fn a_path_ended_with_n_draws_nothing() {
        let pdf = pdf_with("0 0 100 100 re\nn", "[0 0 612 792]");
        let (scene, report) = import_bytes(&pdf).unwrap();
        assert_eq!(report.paths, 0);
        assert!(shapes(&scene).is_empty());
    }

    #[test]
    fn text_is_reported_rather_than_silently_dropped() {
        let pdf = pdf_with("BT\n/F1 12 Tf\n(hello) Tj\nET", "[0 0 612 792]");
        let (_, report) = import_bytes(&pdf).unwrap();

        assert!(!report.is_complete());
        assert!(
            report.unsupported.iter().any(|u| u.starts_with("text")),
            "the user must be told the text did not come across: {:?}",
            report.unsupported
        );
    }

    #[test]
    fn a_postscript_illustrator_file_is_refused_with_an_explanation() {
        let err = import_bytes(b"%!PS-Adobe-3.0\n% Illustrator 8\n").unwrap_err();
        assert!(matches!(err, ImportError::LegacyPostScript));
        assert!(
            err.to_string().contains("PDF Compatible"),
            "the message should say how to fix it: {err}"
        );
    }

    #[test]
    fn rubbish_is_refused_rather_than_panicking() {
        for bytes in [
            b"not a pdf at all".as_slice(),
            b"%PDF-1.7\ntruncated".as_slice(),
            &[0xFF; 64],
        ] {
            let _ = import_bytes(bytes);
        }
    }

    /// Every page becomes a keyframe, so a multi-page document can be stepped
    /// through rather than arriving stacked on top of itself.
    #[test]
    fn each_page_becomes_its_own_keyframe() {
        // Two pages, each with one rectangle at a different height.
        let content_a = "0 0 10 10 re\nf";
        let content_b = "0 100 10 10 re\nf";
        let mut out = String::from("%PDF-1.7\n");
        let mut offsets = Vec::new();
        let objects = [
            "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
            "<< /Type /Pages /Kids [3 0 R 5 0 R] /Count 2 /MediaBox [0 0 200 200] >>".to_string(),
            "<< /Type /Page /Parent 2 0 R /Contents 4 0 R >>".to_string(),
            format!("<< /Length {} >>\nstream\n{content_a}\nendstream", content_a.len()),
            "<< /Type /Page /Parent 2 0 R /Contents 6 0 R >>".to_string(),
            format!("<< /Length {} >>\nstream\n{content_b}\nendstream", content_b.len()),
        ];
        for (i, body) in objects.iter().enumerate() {
            offsets.push(out.len());
            out.push_str(&format!("{} 0 obj\n{body}\nendobj\n", i + 1));
        }
        let xref = out.len();
        out.push_str(&format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1));
        for offset in &offsets {
            out.push_str(&format!("{offset:010} 00000 n \n"));
        }
        out.push_str(&format!(
            "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n",
            objects.len() + 1
        ));

        let (scene, report) = import_bytes(out.as_bytes()).expect("two-page PDF parses");
        assert_eq!(report.pages, 2);

        let layer = scene.stage_layers().iter().next().unwrap();
        assert_eq!(layer.frames.keyframe_count(), 2, "one keyframe per page");
        assert_eq!(layer.frames.objects_at(0).len(), 1);
        assert_eq!(layer.frames.objects_at(1).len(), 1);
    }

    /// Ids handed out by the importer must not be reissued by later editing,
    /// or a user's first edit could collide with imported artwork.
    #[test]
    fn the_scene_does_not_reuse_imported_object_ids() {
        let pdf = pdf_with("0 0 10 10 re\nf\n20 20 10 10 re\nf", "[0 0 612 792]");
        let (mut scene, _) = import_bytes(&pdf).unwrap();

        let used: std::collections::BTreeSet<u64> = scene
            .stage_layers()
            .iter()
            .flat_map(|l| l.all_objects())
            .map(|o| o.id.0)
            .collect();
        assert_eq!(used.len(), 2);

        let layer = scene.stage_layers().iter().next().unwrap().id;
        let fresh = scene
            .add_shape(
                layer,
                ShapeData::filled(kurbo::Rect::new(0.0, 0.0, 1.0, 1.0).to_path(1e-9), Color::WHITE),
            )
            .unwrap();
        assert!(!used.contains(&fresh.0), "the allocator reissued an imported id");
    }
}
