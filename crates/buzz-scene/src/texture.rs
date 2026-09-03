//! Procedural textures — seamless tiles generated in code.
//!
//! A texture here is not a new kind of paint. It is *baked to pixels*: each
//! generator returns one small, **seamless** RGBA tile, which becomes an
//! ordinary [`crate::ImageAsset`] worn as a tiling [`crate::ImageFill`]. So a
//! procedural texture renders and saves through the exact same path as an
//! imported photo — no new `Paint` variant, no renderer or format special case.
//! **The recipe travels with the tile.** The pixels are still what renders, but
//! the [`TextureRecipe`] that made them is kept on the asset, so a texture can
//! be re-tuned after it has been applied — a different colour, coarser bricks,
//! more contrast — and every shape wearing it changes at once. The tile is
//! re-baked from the recipe, which is also all a saved file needs to keep: a
//! procedural texture costs a handful of numbers rather than an embedded PNG.
//!
//! Every tile is built to wrap: opposite edges meet, so `Extend::Repeat` in the
//! sampler shows no seam however large the shape. The two colours come from the
//! caller (the style's fill and stroke), so a texture adopts the chosen palette
//! rather than dictating one.

use peniko::Color;

/// The built-in procedural textures. Each bakes to a seamless tile.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TextureKind {
    /// Soft, low-contrast fibre — a sheet of paper.
    Paper,
    /// A woven over-under thread grid — artist's canvas.
    Canvas,
    /// Cloudy fractal value noise.
    Noise,
    /// A two-colour checkerboard.
    Checker,
    /// A regular grid of filled circles.
    Dots,
    /// Even vertical stripes.
    Stripes,
    /// Courses of brick, each row offset half a brick from the one below.
    Bricks,
    /// Grain and rings, stretched along one axis the way a sawn plank is.
    Wood,
    /// Diagonal pen hatching.
    Hatch,
}

impl TextureKind {
    /// Every texture, in menu order.
    pub const ALL: [TextureKind; 9] = [
        TextureKind::Paper,
        TextureKind::Canvas,
        TextureKind::Noise,
        TextureKind::Checker,
        TextureKind::Dots,
        TextureKind::Stripes,
        TextureKind::Bricks,
        TextureKind::Wood,
        TextureKind::Hatch,
    ];

    /// A short human label for a button or menu.
    pub fn label(self) -> &'static str {
        match self {
            TextureKind::Paper => "Paper",
            TextureKind::Canvas => "Canvas",
            TextureKind::Noise => "Noise",
            TextureKind::Checker => "Checker",
            TextureKind::Dots => "Dots",
            TextureKind::Stripes => "Stripes",
            TextureKind::Bricks => "Bricks",
            TextureKind::Wood => "Wood",
            TextureKind::Hatch => "Hatch",
        }
    }

    /// **The fewest periods this pattern can wrap with.**
    ///
    /// Two, for every pattern built on alternation — a checker, a stripe, the
    /// over-and-under of a weave, the offset courses of a wall. All of them
    /// decide each period by a parity, so an *odd* number of periods across the
    /// tile leaves the last one next to another just like it: a seam. Two is
    /// even, and every larger detail is a power of two, so this is the whole of
    /// the rule.
    pub fn min_detail(self) -> u32 {
        match self {
            TextureKind::Checker
            | TextureKind::Stripes
            | TextureKind::Canvas
            | TextureKind::Bricks => 2,
            _ => 1,
        }
    }

    /// How many periods of the pattern span one tile, before the recipe says
    /// otherwise.
    ///
    /// The kinds differ because their features do: eight courses of brick reads
    /// like a wall, eight rings reads like nothing at all.
    pub fn default_detail(self) -> u32 {
        match self {
            TextureKind::Checker | TextureKind::Stripes | TextureKind::Hatch => 8,
            TextureKind::Dots | TextureKind::Bricks => 4,
            TextureKind::Canvas => 16,
            // The cloudy kinds are fractal rather than periodic; detail sets
            // the lattice they start from.
            TextureKind::Paper | TextureKind::Noise | TextureKind::Wood => 8,
        }
    }
}

/// **How a procedural texture was made** — everything needed to make it again.
///
/// Kept on the [`crate::ImageAsset`] the tile became, which is what makes a
/// texture re-editable rather than a one-way bake: change a colour or coarsen
/// the pattern and the same asset is re-baked, so every shape already wearing it
/// follows. It is also all a saved document needs to store.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TextureRecipe {
    pub kind: TextureKind,
    /// The pattern's colour — the ink, the brick, the thread.
    pub fg: Color,
    /// What it sits on — the paper, the mortar, the gap.
    pub bg: Color,
    /// How many periods of the pattern span one tile.
    ///
    /// Clamped to a **power of two** by [`Self::detail`], because a period that
    /// does not divide the tile leaves a seam down the join — and a seam is the
    /// one thing a tiling texture may never have.
    pub detail: u32,
    /// How far apart the two colours are pushed. `1.0` is the pattern as
    /// generated; below that it fades towards its own average, above it hardens
    /// towards two flat colours.
    ///
    /// The pivot is the tile's **own average**, not the halfway point between
    /// the two colours. Paper and canvas live down near their background — a
    /// faint mottle, mostly bg — so pushing them apart about a fixed midpoint
    /// drives every texel off the same end and flattens the very patterns the
    /// control exists to strengthen.
    pub contrast: f64,
}

impl TextureRecipe {
    /// A texture of this kind in these two colours, at its natural detail.
    pub fn new(kind: TextureKind, fg: Color, bg: Color) -> Self {
        Self {
            kind,
            fg,
            bg,
            detail: kind.default_detail(),
            contrast: 1.0,
        }
    }

    /// The detail actually used: a power of two from 1 to 16, so the pattern's
    /// period always divides the tile and the tile always wraps.
    pub fn detail(&self) -> u32 {
        let wanted = self.detail.clamp(1, 16);
        // Round down to a power of two: 5 courses of brick across a tile would
        // not meet at the join.
        let power = 1 << (31 - wanted.leading_zeros());
        power.max(self.kind.min_detail())
    }

    /// The contrast actually used, kept somewhere a picture can survive.
    pub fn contrast(&self) -> f64 {
        if self.contrast.is_finite() {
            self.contrast.clamp(0.0, 4.0)
        } else {
            1.0
        }
    }

    /// Bake this recipe into a `size`x`size` straight-alpha RGBA8 tile.
    pub fn bake(&self, size: u32) -> Vec<u8> {
        let n = size.clamp(16, 1024) & !15; // multiple of 16, so every period divides it
        let detail = self.detail();

        // Two passes, because contrast pivots on the tile's own average and
        // there is no way to know that without generating it first.
        let mut blend = vec![0.0f32; (n * n) as usize];
        let mut total = 0.0f64;
        for y in 0..n {
            for x in 0..n {
                let t = sample(self.kind, x, y, n, detail);
                blend[(y * n + x) as usize] = t;
                total += t as f64;
            }
        }
        let mean = (total / blend.len() as f64) as f32;

        let fg = self.fg.to_rgba8().to_u8_array();
        let bg = self.bg.to_rgba8().to_u8_array();
        let contrast = self.contrast() as f32;
        let mut px = vec![0u8; (n * n * 4) as usize];
        for (i, raw) in blend.iter().enumerate() {
            let t = (mean + (raw - mean) * contrast).clamp(0.0, 1.0);
            let out = i * 4;
            for c in 0..4 {
                px[out + c] = lerp8(bg[c], fg[c], t);
            }
        }
        px
    }
}

/// Bake `kind` into a `size`×`size` straight-alpha RGBA8 tile (`size*size*4`
/// bytes), interpolating between `bg` (0) and `fg` (1). `size` is clamped to a
/// sensible tile and, for the kinds whose period must divide it, rounded to a
/// multiple of 16.
pub fn tile(kind: TextureKind, size: u32, fg: Color, bg: Color) -> Vec<u8> {
    TextureRecipe::new(kind, fg, bg).bake(size)
}

/// The 0..=1 blend between background and foreground at one texel.
///
/// `detail` is how many periods of the pattern span the tile, and is always a
/// power of two, so every period divides `n` and the tile wraps.
fn sample(kind: TextureKind, x: u32, y: u32, n: u32, detail: u32) -> f32 {
    match kind {
        TextureKind::Checker => {
            let cells = detail;
            let cx = x * cells / n;
            let cy = y * cells / n;
            ((cx + cy) & 1) as f32
        }
        TextureKind::Stripes => {
            let period = (n / detail).max(1);
            ((x / period) & 1) as f32
        }
        TextureKind::Bricks => {
            // Courses of brick, every other one shifted half a brick along, with
            // mortar between. A brick is twice as wide as it is tall, which is
            // what makes it read as brickwork rather than as a grid.
            let course = (n / detail).max(1);
            let brick = (course * 2).min(n);
            // Row and column are taken **within the tile**: the shifted x has to
            // be wrapped back before it names a brick, or the course that runs
            // off the right edge is a different brick from the one that comes
            // back on the left, and the wall has a seam down it.
            let row = (y / course) % detail.max(1);
            let shift = if row & 1 == 0 { 0 } else { brick / 2 };
            let along = (x + shift) % n;
            let bx = along % brick;
            let by = y % course;
            let mortar = (course / 8).max(1);
            if bx < mortar || by < mortar {
                0.0
            } else {
                // A little variation brick to brick, so a wall is not a stencil.
                0.72 + 0.28 * lattice(along / brick, row, 0xB2_1C)
            }
        }
        TextureKind::Hatch => {
            // Diagonal pen strokes. The period is measured along x + y, which
            // wraps in both directions because the period divides the tile.
            let period = (n / detail).max(1);
            let along = (x + y) % period;
            let width = (period / 2).max(1);
            let edge = 1.0;
            let d = along as f32;
            // Soft on both sides of the stroke so the lines are not stepped.
            let inside = smoothstep(-edge, edge, d) * (1.0 - smoothstep(width as f32 - edge, width as f32 + edge, d));
            inside.clamp(0.0, 1.0)
        }
        TextureKind::Wood => {
            // Rings: noise stretched hard along the grain, then folded into
            // bands. The stretch is what turns a cloud into a plank.
            let u = x as f32 / n as f32;
            let v = y as f32 / n as f32;
            let wander = fbm(u, v, 0x77_A1);
            // The ring phase is wrapped into 0..1 *before* the wander is added
            // and before the sine. Multiplying v by the ring count and trusting
            // the sine's own period is a seam in practice rather than in theory:
            // at v = 1 the argument is eight turns further along, and an f32 has
            // no bits left there to hold the fraction that decides the colour.
            let ring = (v * detail as f32).rem_euclid(1.0);
            let phase = (ring + wander * 0.6).rem_euclid(1.0);
            let rings = (phase * std::f32::consts::TAU).sin();
            // Plus the fine grain that runs along the board.
            let grain = value_noise(u, v, (detail * 4).max(8), 0x31_F0);
            (0.45 + 0.35 * rings + 0.20 * grain).clamp(0.0, 1.0)
        }
        TextureKind::Dots => {
            let spacing = (n / detail).max(1) as f32;
            let radius = spacing * 0.32;
            let dx = (x as f32 % spacing) - spacing * 0.5;
            let dy = (y as f32 % spacing) - spacing * 0.5;
            let d = (dx * dx + dy * dy).sqrt();
            // A one-texel soft edge so dots aren't jagged.
            (1.0 - smoothstep(radius - 1.0, radius + 1.0, d)).clamp(0.0, 1.0)
        }
        TextureKind::Noise => {
            let u = x as f32 / n as f32;
            let v = y as f32 / n as f32;
            fbm_at(u, v, detail, 0x51_A2)
        }
        TextureKind::Paper => {
            // Mostly background with a faint fibrous mottle.
            let u = x as f32 / n as f32;
            let v = y as f32 / n as f32;
            0.10 + 0.14 * fbm_at(u, v, detail, 0x9E_37)
        }
        TextureKind::Canvas => {
            // Woven threads: warp and weft alternate which sits on top, each
            // thread brightest along its centre. Thread period divides n.
            let th = (n / detail).max(1) as f32;
            let fx = (x as f32 % th) / th;
            let fy = (y as f32 % th) / th;
            let warp = (std::f32::consts::PI * fx).sin();
            let weft = (std::f32::consts::PI * fy).sin();
            let over_warp = ((x as f32 / th) as u32 + (y as f32 / th) as u32) & 1 == 0;
            let shade = if over_warp { warp } else { weft };
            0.18 + 0.55 * shade
        }
    }
}

/// Fractal value noise in 0..1, seamless over the unit square: three octaves of
/// tileable value noise whose lattice periods (8, 16, 32) all divide the tile.
fn fbm(u: f32, v: f32, seed: u32) -> f32 {
    fbm_at(u, v, 8, seed)
}

/// The same, starting from a `per`x`per` lattice — how a fractal kind takes its
/// detail. `per` is a power of two, so every octave's lattice still tiles.
fn fbm_at(u: f32, v: f32, per: u32, seed: u32) -> f32 {
    let mut sum = 0.0;
    let mut amp = 1.0;
    let mut per = per.max(1);
    let mut norm = 0.0;
    for o in 0..3 {
        sum += amp * value_noise(u, v, per, seed.wrapping_add(o));
        norm += amp;
        amp *= 0.5;
        per *= 2;
    }
    sum / norm
}

/// Bilinear value noise on a `per`×`per` toroidal lattice, so it tiles: lattice
/// indices wrap mod `per`, making the left edge continue into the right and the
/// top into the bottom.
fn value_noise(u: f32, v: f32, per: u32, seed: u32) -> f32 {
    let fx = u * per as f32;
    let fy = v * per as f32;
    let x0 = fx.floor() as i64;
    let y0 = fy.floor() as i64;
    let tx = smooth(fx - x0 as f32);
    let ty = smooth(fy - y0 as f32);
    let wrap = |i: i64| i.rem_euclid(per as i64) as u32;
    let (ax0, ax1) = (wrap(x0), wrap(x0 + 1));
    let (ay0, ay1) = (wrap(y0), wrap(y0 + 1));
    let v00 = lattice(ax0, ay0, seed);
    let v10 = lattice(ax1, ay0, seed);
    let v01 = lattice(ax0, ay1, seed);
    let v11 = lattice(ax1, ay1, seed);
    let a = v00 + (v10 - v00) * tx;
    let b = v01 + (v11 - v01) * tx;
    a + (b - a) * ty
}

/// A stable pseudo-random value in 0..1 for a lattice point, via splitmix64.
fn lattice(x: u32, y: u32, seed: u32) -> f32 {
    let mut z = (u64::from(x) << 32) ^ u64::from(y) ^ (u64::from(seed) << 16);
    z = z.wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^= z >> 31;
    (z >> 40) as f32 / (1u32 << 24) as f32
}

/// Smoothstep easing 0..1 for noise interpolation (no sharp lattice creases).
fn smooth(t: f32) -> f32 {
    t * t * (3.0 - 2.0 * t)
}

/// Smoothstep between two edges, clamped.
fn smoothstep(edge0: f32, edge1: f32, x: f32) -> f32 {
    if edge1 <= edge0 {
        return if x < edge0 { 0.0 } else { 1.0 };
    }
    let t = ((x - edge0) / (edge1 - edge0)).clamp(0.0, 1.0);
    smooth(t)
}

/// Blend two bytes by `t` in 0..=1.
fn lerp8(a: u8, b: u8, t: f32) -> u8 {
    let t = t.clamp(0.0, 1.0);
    (a as f32 + (b as f32 - a as f32) * t).round().clamp(0.0, 255.0) as u8
}

#[cfg(test)]
mod tests {
    use super::*;

    const FG: Color = Color::from_rgb8(0x20, 0x30, 0x40);
    const BG: Color = Color::from_rgb8(0xE8, 0xE0, 0xD0);

    #[test]
    fn every_kind_bakes_a_full_tile() {
        for kind in TextureKind::ALL {
            let px = tile(kind, 128, FG, BG);
            assert_eq!(px.len(), 128 * 128 * 4, "{} wrong size", kind.label());
            assert!(px.iter().any(|&b| b != 0), "{} is all zero", kind.label());
            // Every pixel is fully opaque (both colours are).
            assert!(
                px.chunks_exact(4).all(|p| p[3] == 0xFF),
                "{} left transparent texels",
                kind.label()
            );
        }
    }

    #[test]
    fn tiles_are_seamless() {
        // Seamless means the tile repeats with period `n`: the texel one step
        // *past* the last column is the first column again, and likewise down.
        // Sampling at the out-of-range index `n` (which `sample` handles by plain
        // arithmetic) is exactly that next texel, so it must equal index 0. This
        // is the real seam test — comparing the two *edges* of one tile would
        // wrongly flag high-frequency patterns like the checker, whose edges are
        // meant to differ.
        //
        // Every detail a recipe can ask for is checked, not just the default:
        // detail is what sets each pattern's period, so it is exactly the knob
        // that could introduce a seam.
        for kind in TextureKind::ALL {
            let n = 128u32;
            for wanted in 1u32..=16 {
                // Through the recipe, so the test asks for every detail a user
                // can ask for and checks what the recipe actually allows.
                let detail = TextureRecipe {
                    detail: wanted,
                    ..TextureRecipe::new(kind, FG, BG)
                }
                .detail();
                for i in 0..n {
                    let eps = 1e-6;
                    assert!(
                        (sample(kind, 0, i, n, detail) - sample(kind, n, i, n, detail)).abs() < eps,
                        "{} at detail {detail} (asked {wanted}) has a vertical seam at row {i}",
                        kind.label()
                    );
                    assert!(
                        (sample(kind, i, 0, n, detail) - sample(kind, i, n, n, detail)).abs() < eps,
                        "{} at detail {detail} (asked {wanted}) has a horizontal seam at column {i}",
                        kind.label()
                    );
                }
            }
        }
    }

    #[test]
    fn a_recipe_clamps_detail_to_something_that_can_wrap() {
        // Not a power of two: rounded down to one.
        let mut r = TextureRecipe::new(TextureKind::Hatch, FG, BG);
        r.detail = 5;
        assert_eq!(r.detail(), 4);
        r.detail = 1000;
        assert_eq!(r.detail(), 16, "and bounded");
        r.detail = 0;
        assert_eq!(r.detail(), 1, "and never nothing");
    }

    #[test]
    fn an_alternating_pattern_never_gets_an_odd_number_of_periods() {
        for kind in TextureKind::ALL {
            let mut r = TextureRecipe::new(kind, FG, BG);
            r.detail = 1;
            let got = r.detail();
            assert!(
                got >= kind.min_detail(),
                "{} needs at least {} periods, got {got}",
                kind.label(),
                kind.min_detail()
            );
        }
    }

    #[test]
    fn contrast_pushes_the_two_colours_apart_and_together() {
        let flat = TextureRecipe {
            contrast: 0.0,
            ..TextureRecipe::new(TextureKind::Checker, FG, BG)
        }
        .bake(64);
        // With no contrast every texel is the midpoint, so the tile is flat.
        let first = &flat[0..3];
        assert!(
            flat.chunks_exact(4).all(|px| px[0..3] == *first),
            "no contrast should leave one flat colour"
        );

        let hard = TextureRecipe {
            contrast: 4.0,
            ..TextureRecipe::new(TextureKind::Paper, FG, BG)
        }
        .bake(64);
        let spread = |px: &[u8]| {
            let lo = px.chunks_exact(4).map(|p| p[0]).min().unwrap();
            let hi = px.chunks_exact(4).map(|p| p[0]).max().unwrap();
            hi - lo
        };
        let normal = TextureRecipe::new(TextureKind::Paper, FG, BG).bake(64);
        assert!(
            spread(&hard) > spread(&normal),
            "more contrast should spread the tile wider"
        );
    }

    #[test]
    fn a_nonsense_contrast_reads_as_the_pattern_as_generated() {
        let mut r = TextureRecipe::new(TextureKind::Noise, FG, BG);
        r.contrast = f64::NAN;
        assert_eq!(r.contrast(), 1.0);
        r.contrast = -5.0;
        assert_eq!(r.contrast(), 0.0);
    }

    #[test]
    fn the_new_kinds_all_draw_something() {
        for kind in [TextureKind::Bricks, TextureKind::Wood, TextureKind::Hatch] {
            let px = TextureRecipe::new(kind, FG, BG).bake(128);
            let lo = px.chunks_exact(4).map(|p| p[0]).min().unwrap();
            let hi = px.chunks_exact(4).map(|p| p[0]).max().unwrap();
            assert!(
                hi > lo + 8,
                "{} should have some pattern in it, got {lo}..{hi}",
                kind.label()
            );
        }
    }

    #[test]
    fn two_colours_actually_appear() {
        // A checker must contain texels near both fg and bg.
        let px = tile(TextureKind::Checker, 64, FG, BG);
        let near = |p: &[u8], c: [u8; 4]| {
            (0..3).all(|i| (p[i] as i32 - c[i] as i32).abs() < 24)
        };
        let fg = FG.to_rgba8().to_u8_array();
        let bg = BG.to_rgba8().to_u8_array();
        assert!(px.chunks_exact(4).any(|p| near(p, fg)), "no foreground");
        assert!(px.chunks_exact(4).any(|p| near(p, bg)), "no background");
    }
}
