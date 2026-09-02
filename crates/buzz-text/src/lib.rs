//! Turning a string into vector glyph outlines.
//!
//! Everything in BuzzAnimate is `kurbo::BezPath`, text included: the Text tool
//! does not rasterise glyphs, it lays their *outlines* down as ordinary filled
//! artwork, so text scales, restyles and exports like anything else drawn. This
//! module is the one piece that reads a font and produces those outlines.
//!
//! # Fonts
//!
//! [`available_fonts`] enumerates the faces installed on the system (plus any
//! dropped into an `assets/fonts` folder next to the app), so the Text tool can
//! offer a real picker — English, calligraphy, and Hindi/Devanagari faces all
//! appear by family name. [`outline`] takes an optional family name and renders
//! with it; `None` falls back to a common system default so text works before a
//! font is chosen.
//!
//! # Shaping
//!
//! Latin text is a straight glyph-per-character run, but Devanagari is not:
//! matras reorder, consonants form conjuncts, marks stack. So the outline path
//! runs the string through [`harfrust`] shaping first — it returns positioned
//! glyphs (id + advance + offset) that [`skrifa`] then outlines. The two crates
//! share the exact same `read_fonts::FontRef`, so one font loaded once drives
//! both.

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, OnceLock},
};

use buzz_geom::BezPath;
use harfrust::{ShaperData, UnicodeBuffer};
use skrifa::{
    FontRef, GlyphId, MetadataProvider,
    instance::{LocationRef, Size},
    outline::{DrawSettings, OutlinePen},
    raw::FileRef,
    string::StringId,
};

/// Line advance as a multiple of the em size, used to stack lines of text.
const LINE_SPACING: f64 = 1.25;

/// One font face the Text tool can draw with: a family name and, privately,
/// where its bytes live. `devanagari` flags faces that cover Hindi, so the UI
/// can surface them for users who want them.
#[derive(Debug, Clone)]
pub struct FontFace {
    /// The family name shown in the picker, e.g. "Nirmala UI" or "Segoe Script".
    pub family: String,
    /// True if the face has a glyph for अ (U+0905) — i.e. it can render Hindi.
    pub devanagari: bool,
    path: PathBuf,
    index: u32,
}

/// Every distinct font family installed on the system (and under `assets/fonts`),
/// sorted by name with one entry per family. Enumerated once and cached, because
/// it reads the name table of every font file on disk.
pub fn available_fonts() -> &'static [FontFace] {
    static FONTS: OnceLock<Vec<FontFace>> = OnceLock::new();
    FONTS.get_or_init(enumerate_fonts)
}

/// The outlines of `content` at `size_px` in the named `font` (or a system
/// default when `None`), as one `BezPath` with its first baseline at the origin
/// and text running to the right. Newlines start a new line below. `None` if
/// there is no usable font or the string has no drawable glyphs.
///
/// The winding matches the fill rule glyphs need (`NonZero`), so counters — the
/// holes in "o" and "a" — come out as holes when the path is filled.
pub fn outline(content: &str, size_px: f64, font: Option<&str>) -> Option<BezPath> {
    if content.is_empty() {
        return None;
    }
    let data = resolve_font(font)?;
    let font_ref = FontRef::from_index(data.bytes(), data.index).ok()?;
    let size = Size::new(size_px as f32);
    let location = LocationRef::default();
    let glyphs = font_ref.outline_glyphs();

    let shaper_data = ShaperData::new(&font_ref);
    let shaper = shaper_data.shaper(&font_ref).build();
    let scale = size_px / shaper.units_per_em() as f64;
    let line_height = size_px * LINE_SPACING;

    let mut pen = OutlineToPath { path: BezPath::new(), origin_x: 0.0, origin_y: 0.0 };
    for (line_index, line) in content.split('\n').enumerate() {
        let baseline = line_index as f64 * line_height;
        let mut cursor_x = 0.0;
        let shaped = shape_line(&shaper, line);
        for (info, pos) in shaped.glyph_infos().iter().zip(shaped.glyph_positions()) {
            pen.origin_x = cursor_x + pos.x_offset as f64 * scale;
            pen.origin_y = baseline - pos.y_offset as f64 * scale;
            if let Some(glyph) = glyphs.get(GlyphId::new(info.glyph_id)) {
                let _ = glyph.draw(DrawSettings::unhinted(size, location), &mut pen);
            }
            cursor_x += pos.x_advance as f64 * scale;
        }
    }

    (!pen.path.elements().is_empty()).then_some(pen.path)
}

/// The bounding advance width and total height of `content` at `size_px` in the
/// named `font`, for placing a caret or sizing a box. `(0, size_px)` when there
/// is no font. Width is the widest line; height covers every line.
pub fn measure(content: &str, size_px: f64, font: Option<&str>) -> (f64, f64) {
    let Some(data) = resolve_font(font) else {
        return (0.0, size_px);
    };
    let Ok(font_ref) = FontRef::from_index(data.bytes(), data.index) else {
        return (0.0, size_px);
    };
    let shaper_data = ShaperData::new(&font_ref);
    let shaper = shaper_data.shaper(&font_ref).build();
    let scale = size_px / shaper.units_per_em() as f64;
    let line_height = size_px * LINE_SPACING;

    let lines: Vec<&str> = content.split('\n').collect();
    let width = lines
        .iter()
        .map(|line| {
            let shaped = shape_line(&shaper, line);
            shaped
                .glyph_positions()
                .iter()
                .map(|pos| pos.x_advance as f64 * scale)
                .sum::<f64>()
        })
        .fold(0.0_f64, f64::max);
    let height = (lines.len().max(1) as f64 - 1.0) * line_height + size_px;
    (width, height)
}

/// Shape one line of text: run it through harfrust so complex scripts (Hindi)
/// reorder and combine correctly. `guess_segment_properties` picks the script,
/// direction and language from the characters, so Latin and Devanagari both work
/// without the caller declaring which.
fn shape_line(shaper: &harfrust::Shaper, line: &str) -> harfrust::GlyphBuffer {
    let mut buffer = UnicodeBuffer::new();
    buffer.push_str(line);
    buffer.guess_segment_properties();
    shaper.shape(buffer, &[])
}

/// A pen that appends a glyph's contours onto one growing path, offset by the
/// running pen position and flipped in Y (fonts are Y-up, the stage is Y-down).
struct OutlineToPath {
    path: BezPath,
    origin_x: f64,
    origin_y: f64,
}

impl OutlineToPath {
    fn at(&self, x: f32, y: f32) -> (f64, f64) {
        (self.origin_x + x as f64, self.origin_y - y as f64)
    }
}

impl OutlinePen for OutlineToPath {
    fn move_to(&mut self, x: f32, y: f32) {
        self.path.move_to(self.at(x, y));
    }
    fn line_to(&mut self, x: f32, y: f32) {
        self.path.line_to(self.at(x, y));
    }
    fn quad_to(&mut self, cx: f32, cy: f32, x: f32, y: f32) {
        let c = self.at(cx, cy);
        let p = self.at(x, y);
        self.path.quad_to(c, p);
    }
    fn curve_to(&mut self, c0x: f32, c0y: f32, c1x: f32, c1y: f32, x: f32, y: f32) {
        let c0 = self.at(c0x, c0y);
        let c1 = self.at(c1x, c1y);
        let p = self.at(x, y);
        self.path.curve_to(c0, c1, p);
    }
    fn close(&mut self) {
        self.path.close_path();
    }
}

/// Font bytes plus the face index within them (0 for a plain `.ttf`, the face
/// number for a `.ttc` collection). Bytes are either a cached system default or
/// a file read on demand and kept alive for the process.
struct ResolvedFont {
    data: FontData,
    index: u32,
}

impl ResolvedFont {
    fn bytes(&self) -> &[u8] {
        self.data.bytes()
    }
}

enum FontData {
    Static(&'static [u8]),
    Shared(Arc<Vec<u8>>),
}

impl FontData {
    fn bytes(&self) -> &[u8] {
        match self {
            FontData::Static(b) => b,
            FontData::Shared(v) => v,
        }
    }
}

/// Pick the bytes to draw with: the named family if it enumerated and its file
/// still reads, otherwise a common system default so text always works.
fn resolve_font(name: Option<&str>) -> Option<ResolvedFont> {
    if let Some(name) = name.filter(|n| !n.is_empty()) {
        if let Some(face) = available_fonts()
            .iter()
            .find(|f| f.family.eq_ignore_ascii_case(name))
        {
            if let Some(bytes) = load_bytes(&face.path) {
                return Some(ResolvedFont { data: FontData::Shared(bytes), index: face.index });
            }
        }
    }
    default_font().map(|b| ResolvedFont { data: FontData::Static(b), index: 0 })
}

/// Read a font file once and keep its bytes for the process, so re-editing text
/// in the same face doesn't re-read the file each time.
fn load_bytes(path: &Path) -> Option<Arc<Vec<u8>>> {
    static CACHE: OnceLock<Mutex<HashMap<PathBuf, Arc<Vec<u8>>>>> = OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    let mut map = cache.lock().ok()?;
    if let Some(bytes) = map.get(path) {
        return Some(bytes.clone());
    }
    let bytes = Arc::new(std::fs::read(path).ok()?);
    map.insert(path.to_path_buf(), bytes.clone());
    Some(bytes)
}

/// A common system font's bytes, found once and cached, used when no family is
/// named. `None` if no candidate exists.
fn default_font() -> Option<&'static [u8]> {
    static FONT: OnceLock<Option<Vec<u8>>> = OnceLock::new();
    FONT.get_or_init(|| {
        const CANDIDATES: &[&str] = &[
            // Windows
            "C:/Windows/Fonts/segoeui.ttf",
            "C:/Windows/Fonts/arial.ttf",
            "C:/Windows/Fonts/tahoma.ttf",
            "C:/Windows/Fonts/verdana.ttf",
            // macOS
            "/System/Library/Fonts/Supplemental/Arial.ttf",
            "/Library/Fonts/Arial.ttf",
            "/System/Library/Fonts/SFNSRounded.ttf",
            // Linux
            "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
            "/usr/share/fonts/truetype/liberation/LiberationSans-Regular.ttf",
            "/usr/share/fonts/TTF/DejaVuSans.ttf",
            "/usr/share/fonts/dejavu/DejaVuSans.ttf",
        ];
        CANDIDATES.iter().find_map(|path| std::fs::read(path).ok())
    })
    .as_deref()
}

/// Walk the OS font directories (and `assets/fonts`), read each file's family
/// name, and return one entry per family, sorted. A `.ttc` collection yields one
/// entry per face it holds.
fn enumerate_fonts() -> Vec<FontFace> {
    let mut faces = Vec::new();
    for dir in font_dirs() {
        collect_dir(&dir, &mut faces, 0);
    }
    faces.sort_by(|a, b| a.family.to_lowercase().cmp(&b.family.to_lowercase()));
    faces.dedup_by(|a, b| a.family.eq_ignore_ascii_case(&b.family));
    faces
}

/// The directories to scan for fonts on this OS, plus a project-local
/// `assets/fonts` folder so a user can drop in their own `.ttf` files.
fn font_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    #[cfg(target_os = "windows")]
    {
        dirs.push(PathBuf::from("C:/Windows/Fonts"));
        if let Ok(local) = std::env::var("LOCALAPPDATA") {
            dirs.push(PathBuf::from(local).join("Microsoft/Windows/Fonts"));
        }
    }
    #[cfg(target_os = "macos")]
    {
        dirs.push(PathBuf::from("/System/Library/Fonts"));
        dirs.push(PathBuf::from("/Library/Fonts"));
        if let Ok(home) = std::env::var("HOME") {
            dirs.push(PathBuf::from(home).join("Library/Fonts"));
        }
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    {
        dirs.push(PathBuf::from("/usr/share/fonts"));
        dirs.push(PathBuf::from("/usr/local/share/fonts"));
        if let Ok(home) = std::env::var("HOME") {
            dirs.push(PathBuf::from(&home).join(".fonts"));
            dirs.push(PathBuf::from(home).join(".local/share/fonts"));
        }
    }
    dirs.push(PathBuf::from("assets/fonts"));
    dirs
}

/// Recurse a directory (bounded depth), adding every font file's faces. Depth is
/// capped so a stray symlink loop can't spin forever.
fn collect_dir(dir: &Path, out: &mut Vec<FontFace>, depth: usize) {
    if depth > 6 {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_dir(&path, out, depth + 1);
        } else if is_font_file(&path) {
            read_faces(&path, out);
        }
    }
}

/// True for `.ttf` / `.otf` / `.ttc` files, case-insensitively.
fn is_font_file(path: &Path) -> bool {
    matches!(
        path.extension().and_then(|e| e.to_str()).map(str::to_ascii_lowercase).as_deref(),
        Some("ttf" | "otf" | "ttc")
    )
}

/// Read every face in a font file and push a [`FontFace`] for each. A plain font
/// is one face at index 0; a `.ttc` collection is several.
fn read_faces(path: &Path, out: &mut Vec<FontFace>) {
    let Ok(bytes) = std::fs::read(path) else {
        return;
    };
    match FileRef::new(&bytes) {
        Ok(FileRef::Font(font)) => {
            if let Some(face) = face_info(&font, path, 0) {
                out.push(face);
            }
        }
        Ok(FileRef::Collection(collection)) => {
            for index in 0..collection.len() {
                if let Ok(font) = collection.get(index) {
                    if let Some(face) = face_info(&font, path, index) {
                        out.push(face);
                    }
                }
            }
        }
        Err(_) => {}
    }
}

/// The family name and Devanagari coverage of one loaded face. `None` if it has
/// no readable family name.
fn face_info(font: &FontRef, path: &Path, index: u32) -> Option<FontFace> {
    let family = font
        .localized_strings(StringId::FAMILY_NAME)
        .english_or_first()
        .map(|s| s.to_string())?;
    if family.trim().is_empty() {
        return None;
    }
    let devanagari = font.charmap().map('\u{0905}').is_some();
    Some(FontFace { family, devanagari, path: path.to_path_buf(), index })
}

#[cfg(test)]
mod tests {
    use super::*;
    use buzz_geom::Shape as _;

    #[test]
    fn outlining_text_makes_a_path() {
        let Some(path) = outline("Hi", 48.0, None) else {
            eprintln!("skipping: no system font found");
            return;
        };
        assert!(!path.elements().is_empty(), "outline produced no geometry");
        let bb = path.bounding_box();
        assert!(bb.width() > 0.0 && bb.height() > 0.0, "text has no extent: {bb:?}");
    }

    #[test]
    fn empty_text_has_no_outline() {
        assert!(outline("", 48.0, None).is_none());
    }

    #[test]
    fn wider_strings_advance_further() {
        let (thin, _) = measure("i", 48.0, None);
        let (wide, _) = measure("wwww", 48.0, None);
        if thin == 0.0 && wide == 0.0 {
            eprintln!("skipping: no system font found");
            return;
        }
        assert!(wide > thin, "'wwww' ({wide}) should be wider than 'i' ({thin})");
    }

    #[test]
    fn extra_lines_add_height() {
        let (_, one) = measure("one", 48.0, None);
        let (_, three) = measure("one\ntwo\nthree", 48.0, None);
        if one == 48.0 && three == 48.0 {
            // No font: measure returned the (0, size_px) fallback for both.
            return;
        }
        assert!(three > one, "three lines ({three}) should be taller than one ({one})");
    }

    #[test]
    fn hindi_shapes_into_outlines() {
        let Some(face) = available_fonts().iter().find(|f| f.devanagari) else {
            eprintln!("skipping: no Devanagari font on this system");
            return;
        };
        // "नमस्ते" — has a matra and a conjunct, so it only renders right if the
        // text is actually shaped rather than laid out codepoint by codepoint.
        let path = outline("\u{0928}\u{092E}\u{0938}\u{094D}\u{0924}\u{0947}", 64.0, Some(&face.family));
        let path = path.expect("Hindi produced no outline");
        assert!(!path.elements().is_empty(), "Hindi outline is empty in {:?}", face.family);
    }

    #[test]
    fn enumeration_finds_a_family_that_outlines() {
        let fonts = available_fonts();
        if fonts.is_empty() {
            eprintln!("skipping: no fonts enumerated");
            return;
        }
        // Whatever the first family is, drawing with it by name should work.
        let name = fonts[0].family.clone();
        assert!(
            outline("Hi", 48.0, Some(&name)).is_some(),
            "named font {name:?} produced no outline"
        );
    }
}
