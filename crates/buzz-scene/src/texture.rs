//! Procedural textures — seamless tiles generated in code.
//!
//! A texture here is not a new kind of paint. It is *baked to pixels*: each
//! generator returns one small, **seamless** RGBA tile, which becomes an
//! ordinary [`crate::ImageAsset`] worn as a tiling [`crate::ImageFill`]. So a
//! procedural texture renders and saves through the exact same path as an
//! imported photo — no new `Paint` variant, no renderer or format special case.
//! The cost is that it is pixels once made: re-tuning means re-applying, which
//! suits the app's paint-what-you-see, no-lag feel.
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
}

impl TextureKind {
    /// Every texture, in menu order.
    pub const ALL: [TextureKind; 6] = [
        TextureKind::Paper,
        TextureKind::Canvas,
        TextureKind::Noise,
        TextureKind::Checker,
        TextureKind::Dots,
        TextureKind::Stripes,
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
        }
    }
}

/// Bake `kind` into a `size`×`size` straight-alpha RGBA8 tile (`size*size*4`
/// bytes), interpolating between `bg` (0) and `fg` (1). `size` is clamped to a
/// sensible tile and, for the kinds whose period must divide it, rounded to a
/// multiple of 16.
pub fn tile(kind: TextureKind, size: u32, fg: Color, bg: Color) -> Vec<u8> {
    let n = size.clamp(16, 1024) & !15; // multiple of 16, so every period divides it
    let fg = fg.to_rgba8().to_u8_array();
    let bg = bg.to_rgba8().to_u8_array();
    let mut px = vec![0u8; (n * n * 4) as usize];
    for y in 0..n {
        for x in 0..n {
            let t = sample(kind, x, y, n);
            let out = ((y * n + x) * 4) as usize;
            for c in 0..4 {
                px[out + c] = lerp8(bg[c], fg[c], t);
            }
        }
    }
    px
}

/// The 0..=1 blend between background and foreground at one texel.
fn sample(kind: TextureKind, x: u32, y: u32, n: u32) -> f32 {
    match kind {
        TextureKind::Checker => {
            let cells = 8;
            let cx = x * cells / n;
            let cy = y * cells / n;
            ((cx + cy) & 1) as f32
        }
        TextureKind::Stripes => {
            let period = n / 8;
            ((x / period) & 1) as f32
        }
        TextureKind::Dots => {
            let spacing = (n / 4) as f32; // 4×4 dots, wraps because 4 divides n
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
            fbm(u, v, 0x51_A2)
        }
        TextureKind::Paper => {
            // Mostly background with a faint fibrous mottle.
            let u = x as f32 / n as f32;
            let v = y as f32 / n as f32;
            0.10 + 0.14 * fbm(u, v, 0x9E_37)
        }
        TextureKind::Canvas => {
            // Woven threads: warp and weft alternate which sits on top, each
            // thread brightest along its centre. Thread period divides n.
            let th = (n / 16) as f32;
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
    let mut sum = 0.0;
    let mut amp = 1.0;
    let mut per = 8u32;
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
        for kind in TextureKind::ALL {
            let n = 128u32;
            for i in 0..n {
                let eps = 1e-6;
                assert!(
                    (sample(kind, 0, i, n) - sample(kind, n, i, n)).abs() < eps,
                    "{} has a vertical seam at row {i}",
                    kind.label()
                );
                assert!(
                    (sample(kind, i, 0, n) - sample(kind, i, n, n)).abs() < eps,
                    "{} has a horizontal seam at column {i}",
                    kind.label()
                );
            }
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
