//! The drawn icon set: sixty line icons from the user's own sheet.
//!
//! # Why this is raster, when `icons.rs` explains why raster once failed
//!
//! The raster attempt `icons.rs` describes was a Sobel outline of an opaque
//! picture, sampled at whatever size the button happened to be — a traced
//! edge of an edge, blurred by the GPU's filtering. At 20 px it was a blob.
//!
//! This sheet is different at the source: its linework is the PNG's *alpha
//! channel*, a clean anti-aliased stroke with nothing else in it. It was cut
//! into `assets/icon_sheet.png` (a 10 x 6 grid of 128 px cells, the icon
//! centred in each) with the grid found by projection, not by hand.
//!
//! And the rendering is different: each icon is rasterised **for the exact
//! number of device pixels** it will occupy — area-averaged down from 128 px,
//! with the stroke first thickened so it lands at a pixel and a bit wide
//! rather than a faint 0.7 px — and drawn at a pixel-snapped rectangle with
//! nearest filtering, so the GPU never resamples it. One texture per
//! (icon, size), cached for the life of the context.
//!
//! The texture is white; the colour is the painter's tint. So one icon serves
//! every theme: [`ink`] is white on a dark theme and black on a light one.

use std::collections::HashMap;
use std::sync::OnceLock;

use egui::{Color32, ColorImage, Painter, Rect, TextureHandle, TextureOptions, pos2, vec2};

/// Side of one cell in the atlas, in pixels.
const CELL: usize = 128;
/// Cells per atlas row.
const COLUMNS: usize = 10;
/// The sheet's own stroke, measured on the atlas: about five and a half px.
const SOURCE_STROKE: f32 = 5.5;

/// One icon on the sheet, in the sheet's own reading order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SheetIcon {
    Brush,
    Pencil,
    Eraser,
    Fill,
    Eyedropper,
    Airbrush,
    Smudge,
    Marker,
    InkPen,
    Spray,

    Select,
    Lasso,
    MagicSelect,
    Crop,
    Transform,
    Rotate,
    Flip,
    Cut,
    Copy,
    Paste,

    Rectangle,
    Ellipse,
    Polygon,
    Line,
    Curve,
    Pen,
    Text,
    Shape,
    Star,
    Spiral,

    ColorPicker,
    ColorPanel,
    Gradient,
    Stroke,
    DashedLine,
    SmoothLine,
    PixelLine,
    Grid,
    Ruler,
    Perspective,

    Play,
    PreviousFrame,
    NextFrame,
    Record,
    OnionSkin,
    AddFrame,
    DeleteFrame,
    DuplicateFrame,
    Keyframe,
    MotionPath,

    NewLayer,
    AddLayer,
    DeleteLayer,
    MergeDown,
    MergeUp,
    Visibility,
    Lock,
    Unlock,
    Solo,
    FitToScreen,
}

impl SheetIcon {
    /// Every icon, in sheet order — its index is its cell.
    pub const ALL: [SheetIcon; 60] = {
        use SheetIcon::*;
        [
            Brush, Pencil, Eraser, Fill, Eyedropper, Airbrush, Smudge, Marker, InkPen, Spray,
            Select, Lasso, MagicSelect, Crop, Transform, Rotate, Flip, Cut, Copy, Paste,
            Rectangle, Ellipse, Polygon, Line, Curve, Pen, Text, Shape, Star, Spiral,
            ColorPicker, ColorPanel, Gradient, Stroke, DashedLine, SmoothLine, PixelLine, Grid,
            Ruler, Perspective, Play, PreviousFrame, NextFrame, Record, OnionSkin, AddFrame,
            DeleteFrame, DuplicateFrame, Keyframe, MotionPath, NewLayer, AddLayer, DeleteLayer,
            MergeDown, MergeUp, Visibility, Lock, Unlock, Solo, FitToScreen,
        ]
    };

    fn cell(self) -> usize {
        self as usize
    }
}

/// The resting ink for an icon: **white on a dark theme, black on a light
/// one** — the sheet is line art, and line art wants full contrast.
pub fn ink() -> Color32 {
    if crate::theme::theme().is_dark() {
        Color32::WHITE
    } else {
        Color32::BLACK
    }
}

/// The atlas, decoded once: coverage in `0..=1`, row-major.
fn atlas() -> &'static (usize, usize, Vec<f32>) {
    static ATLAS: OnceLock<(usize, usize, Vec<f32>)> = OnceLock::new();
    ATLAS.get_or_init(|| {
        let bytes: &[u8] = include_bytes!("../assets/icon_sheet.png");
        let mut decoder = png::Decoder::new(std::io::Cursor::new(bytes));
        decoder.set_transformations(png::Transformations::normalize_to_color8());
        let mut reader = decoder.read_info().expect("icon sheet is a valid PNG");
        let mut buf = vec![0; reader.output_buffer_size().expect("icon sheet has a size")];
        let info = reader.next_frame(&mut buf).expect("icon sheet decodes");
        let channels = info.color_type.samples();
        let (w, h) = (info.width as usize, info.height as usize);
        let data = buf[..w * h * channels]
            .chunks(channels)
            .map(|px| px[0] as f32 / 255.0)
            .collect();
        (w, h, data)
    })
}

/// `icon`'s coverage rasterised at `n` x `n` device pixels.
///
/// Public so a test can check an icon lands with a solid, crisp stroke at the
/// sizes the interface actually uses.
pub fn rasterise(icon: SheetIcon, n: usize) -> Vec<f32> {
    let n = n.max(1);
    let (w, _, data) = atlas();
    let (cx, cy) = ((icon.cell() % COLUMNS) * CELL, (icon.cell() / COLUMNS) * CELL);
    let src = |x: i32, y: i32| -> f32 {
        if x < 0 || y < 0 || x >= CELL as i32 || y >= CELL as i32 {
            0.0
        } else {
            data[(cy + y as usize) * w + cx + x as usize]
        }
    };

    // **Thicken to land on the pixel grid.** Scaled straight down, a 5.5 px
    // stroke at 128 is 0.7 px at 16 — a grey hairline. Aim for about 1.3
    // device pixels at button size, growing gently with the icon.
    let k = n as f32 / CELL as f32;
    let target = (n as f32 * 0.055).max(1.3);
    let radius = ((target / k - SOURCE_STROKE) * 0.5).clamp(0.0, 4.0);
    let mut thick = vec![0.0f32; CELL * CELL];
    let reach = radius.ceil() as i32;
    let offsets: Vec<(i32, i32, f32)> = (-reach..=reach)
        .flat_map(|dy| (-reach..=reach).map(move |dx| (dx, dy)))
        .filter_map(|(dx, dy)| {
            let weight = (radius + 0.5 - ((dx * dx + dy * dy) as f32).sqrt()).clamp(0.0, 1.0);
            (weight > 0.0).then_some((dx, dy, weight))
        })
        .collect();
    for y in 0..CELL as i32 {
        for x in 0..CELL as i32 {
            let mut v = 0.0f32;
            for &(dx, dy, weight) in &offsets {
                v = v.max(src(x + dx, y + dy) * weight);
            }
            thick[y as usize * CELL + x as usize] = v;
        }
    }

    // **Area-average down**: each output pixel is the exact mean of the
    // source area it covers, fractional edges included. No kernel ringing,
    // no blur beyond the pixel itself.
    let scale = CELL as f32 / n as f32;
    let spans: Vec<Vec<(usize, f32)>> = (0..n)
        .map(|i| {
            let (a, b) = (i as f32 * scale, (i + 1) as f32 * scale);
            (a.floor() as usize..(b.ceil() as usize).min(CELL))
                .map(|s| (s, ((s + 1) as f32).min(b) - (s as f32).max(a)))
                .filter(|(_, weight)| *weight > 0.0)
                .collect()
        })
        .collect();
    let mut out = vec![0.0f32; n * n];
    for (oy, ys) in spans.iter().enumerate() {
        for (ox, xs) in spans.iter().enumerate() {
            let mut sum = 0.0;
            for &(sy, wy) in ys {
                for &(sx, wx) in xs {
                    sum += thick[sy * CELL + sx] * wy * wx;
                }
            }
            // A gentle contrast lift so an edge half-covering a pixel reads
            // as ink rather than as grey.
            out[oy * n + ox] = ((sum / (scale * scale) - 0.04) / 0.82).clamp(0.0, 1.0);
        }
    }
    out
}

#[derive(Clone, Default)]
struct Cache(HashMap<(SheetIcon, usize), TextureHandle>);

fn texture(ctx: &egui::Context, icon: SheetIcon, n: usize) -> TextureHandle {
    let id = egui::Id::new("buzz-icon-sheet-cache");
    if let Some(handle) = ctx.data(|d| {
        d.get_temp::<Cache>(id)
            .and_then(|c| c.0.get(&(icon, n)).cloned())
    }) {
        return handle;
    }
    let coverage = rasterise(icon, n);
    let pixels = coverage
        .iter()
        .map(|&a| Color32::from_white_alpha((a * 255.0).round() as u8))
        .collect();
    let image = ColorImage::new([n, n], pixels);
    let handle = ctx.load_texture(format!("icon-{icon:?}-{n}"), image, TextureOptions::NEAREST);
    ctx.data_mut(|d| {
        d.get_temp_mut_or_default::<Cache>(id)
            .0
            .insert((icon, n), handle.clone());
    });
    handle
}

/// Draw `icon` centred in `rect`, in `color`, pixel-for-pixel.
pub fn paint(painter: &Painter, rect: Rect, icon: SheetIcon, color: Color32) {
    let ppp = painter.ctx().pixels_per_point();
    let side = rect.width().min(rect.height());
    let n = (side * ppp).round().max(1.0) as usize;
    // Snap the icon's corner to a device pixel, so a texel is a pixel.
    let size = n as f32 / ppp;
    let corner = rect.center() - vec2(size, size) * 0.5;
    let corner = pos2((corner.x * ppp).round() / ppp, (corner.y * ppp).round() / ppp);
    let tex = texture(painter.ctx(), icon, n);
    painter.image(
        tex.id(),
        Rect::from_min_size(corner, vec2(size, size)),
        Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
        color,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_cell_holds_an_icon() {
        for icon in SheetIcon::ALL {
            let px = rasterise(icon, 32);
            let ink: f32 = px.iter().sum();
            assert!(ink > 20.0, "{icon:?} is nearly empty at 32 px ({ink})");
        }
    }

    #[test]
    fn the_order_matches_the_enum() {
        for (i, icon) in SheetIcon::ALL.iter().enumerate() {
            assert_eq!(icon.cell(), i);
        }
    }

    #[test]
    fn at_button_size_the_stroke_is_solid_ink() {
        // A crisp icon has pixels at full coverage, not only greys.
        for n in [16, 20, 24] {
            for icon in SheetIcon::ALL {
                let px = rasterise(icon, n);
                let solid = px.iter().filter(|&&a| a > 0.75).count();
                assert!(solid >= 4, "{icon:?} at {n} px has no solid ink ({solid})");
            }
        }
    }
}

#[cfg(test)]
mod preview {
    use super::*;

    /// Writes every icon at 16, 20, 24 and 32 px, black on white, magnified
    /// 4x with nearest neighbour, to `buzz-icon-sheet-preview.png` in the temp
    /// directory — to look at, not to assert on.
    #[test]
    #[ignore]
    fn dump_preview() {
        let sizes = [16usize, 20, 24, 32];
        let zoom = 4;
        let pitch = 36 * zoom;
        let (w, h) = (SheetIcon::ALL.len() * pitch, sizes.len() * pitch);
        let mut img = vec![255u8; w * h];
        for (row, &n) in sizes.iter().enumerate() {
            for (col, icon) in SheetIcon::ALL.iter().enumerate() {
                let px = rasterise(*icon, n);
                for y in 0..n * zoom {
                    for x in 0..n * zoom {
                        let a = px[(y / zoom) * n + x / zoom];
                        img[(row * pitch + y) * w + col * pitch + x] = (255.0 * (1.0 - a)) as u8;
                    }
                }
            }
        }
        let path = std::env::temp_dir().join("buzz-icon-sheet-preview.png");
        let file = std::fs::File::create(&path).unwrap();
        let mut enc = png::Encoder::new(std::io::BufWriter::new(file), w as u32, h as u32);
        enc.set_color(png::ColorType::Grayscale);
        enc.write_header().unwrap().write_image_data(&img).unwrap();
        println!("{}", path.display());
    }
}
