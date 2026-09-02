//! Turning a string into vector glyph outlines.
//!
//! Everything in BuzzAnimate is `kurbo::BezPath`, text included: the Text tool
//! does not rasterise glyphs, it lays their *outlines* down as ordinary filled
//! artwork, so text scales, restyles and exports like anything else drawn. This
//! module is the one piece that reads a font and produces those outlines.
//!
//! # The font
//!
//! No font is bundled yet, so a font is loaded from the system at first use — a
//! short per-OS list of common faces, cached for the life of the process. If
//! none is found, [`outline`] returns `None` and the tool says so. Bundling a
//! licensed face in the app is the portability step; this is the stopgap that
//! makes the tool work now.

use std::sync::OnceLock;

use buzz_geom::BezPath;
use skrifa::{
    FontRef, GlyphId, MetadataProvider,
    instance::{LocationRef, Size},
    outline::{DrawSettings, OutlinePen},
};

/// The outlines of `content` at `size_px`, as one `BezPath` with its baseline at
/// the origin and text running to the right. `None` if there is no usable font
/// or the string has no drawable glyphs.
///
/// The winding matches the fill rule glyphs need (`NonZero`), so counters — the
/// holes in "o" and "a" — come out as holes when the path is filled.
pub fn outline(content: &str, size_px: f64) -> Option<BezPath> {
    if content.is_empty() {
        return None;
    }
    let bytes = system_font()?;
    let font = FontRef::new(bytes)
        .or_else(|_| FontRef::from_index(bytes, 0))
        .ok()?;
    let size = Size::new(size_px as f32);
    let location = LocationRef::default();
    let charmap = font.charmap();
    let glyphs = font.outline_glyphs();
    let metrics = font.glyph_metrics(size, location);

    let mut pen = OutlineToPath {
        path: BezPath::new(),
        origin_x: 0.0,
    };
    for ch in content.chars() {
        let gid = charmap.map(ch).unwrap_or(GlyphId::NOTDEF);
        if let Some(glyph) = glyphs.get(gid) {
            let _ = glyph.draw(DrawSettings::unhinted(size, location), &mut pen);
        }
        pen.origin_x += metrics.advance_width(gid).unwrap_or(0.0) as f64;
    }

    (!pen.path.elements().is_empty()).then_some(pen.path)
}

/// The advance width and a nominal line height of `content` at `size_px`, for
/// placing a caret or sizing a box. `(0, size_px)` when there is no font.
pub fn measure(content: &str, size_px: f64) -> (f64, f64) {
    let Some(bytes) = system_font() else {
        return (0.0, size_px);
    };
    let Ok(font) = FontRef::new(bytes).or_else(|_| FontRef::from_index(bytes, 0)) else {
        return (0.0, size_px);
    };
    let size = Size::new(size_px as f32);
    let charmap = font.charmap();
    let metrics = font.glyph_metrics(size, LocationRef::default());
    let width: f64 = content
        .chars()
        .map(|ch| {
            let gid = charmap.map(ch).unwrap_or(GlyphId::NOTDEF);
            metrics.advance_width(gid).unwrap_or(0.0) as f64
        })
        .sum();
    (width, size_px)
}

/// A pen that appends a glyph's contours onto one growing path, offset by the
/// running pen position and flipped in Y (fonts are Y-up, the stage is Y-down).
struct OutlineToPath {
    path: BezPath,
    origin_x: f64,
}

impl OutlineToPath {
    fn at(&self, x: f32, y: f32) -> (f64, f64) {
        (self.origin_x + x as f64, -(y as f64))
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

/// The system font bytes, found once and cached. `None` if no candidate exists.
fn system_font() -> Option<&'static [u8]> {
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

#[cfg(test)]
mod tests {
    use super::*;
    use buzz_geom::Shape as _;

    #[test]
    fn outlining_text_makes_a_path() {
        let Some(path) = outline("Hi", 48.0) else {
            eprintln!("skipping: no system font found");
            return;
        };
        assert!(!path.elements().is_empty(), "outline produced no geometry");
        let bb = path.bounding_box();
        assert!(bb.width() > 0.0 && bb.height() > 0.0, "text has no extent: {bb:?}");
    }

    #[test]
    fn empty_text_has_no_outline() {
        assert!(outline("", 48.0).is_none());
    }

    #[test]
    fn wider_strings_advance_further() {
        let (thin, _) = measure("i", 48.0);
        let (wide, _) = measure("wwww", 48.0);
        if thin == 0.0 && wide == 0.0 {
            eprintln!("skipping: no system font found");
            return;
        }
        assert!(wide > thin, "'wwww' ({wide}) should be wider than 'i' ({thin})");
    }
}
