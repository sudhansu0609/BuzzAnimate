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

    // -- ground cover -------------------------------------------------------
    //
    // **Twelve textures for the floor of a shot**, six of grass and six of
    // dirt, because that is what a background is mostly made of and painting
    // one blade at a time is not animation. They are built from the same
    // seamless primitives as everything above — tileable value noise, and a
    // tileable cellular lattice for the ones made of stones — so a hillside
    // covers a field of any size with no seam and costs a handful of numbers
    // in the file.

    /// Fine dense turf: short blades, close together, mown.
    GrassLawn,
    /// Separated upright blades with the ground showing between them.
    GrassBlades,
    /// Rough grass in clumps, tall and uneven.
    GrassMeadow,
    /// Straw: pale strands lying over one another at an angle.
    GrassDry,
    /// Soft cushiony moss, dense and specked.
    GrassMoss,
    /// Broad rounded leaves scattered over the ground.
    GrassClover,

    /// Loamy soil, clumped, with darker matter through it.
    DirtSoil,
    /// Loose gravel: many small stones, each its own tone.
    DirtGravel,
    /// Fine sand with a faint ripple across it.
    DirtSand,
    /// Dry mud, cracked into plates.
    DirtCracked,
    /// Larger rounded pebbles packed together.
    DirtPebbles,
    /// Wet mud: slick dark patches with a sheen on the ridges.
    DirtMud,
}

impl TextureKind {
    /// The surface textures — what a drawing is *on*, or patterned with.
    pub const SURFACES: &'static [TextureKind] = &[
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

    /// The grasses.
    pub const GRASS: &'static [TextureKind] = &[
        TextureKind::GrassLawn,
        TextureKind::GrassBlades,
        TextureKind::GrassMeadow,
        TextureKind::GrassDry,
        TextureKind::GrassMoss,
        TextureKind::GrassClover,
    ];

    /// The bare ground.
    pub const DIRT: &'static [TextureKind] = &[
        TextureKind::DirtSoil,
        TextureKind::DirtGravel,
        TextureKind::DirtSand,
        TextureKind::DirtCracked,
        TextureKind::DirtPebbles,
        TextureKind::DirtMud,
    ];

    /// The three families, each under the heading a panel shows it beneath.
    ///
    /// Twenty-one buttons in one wrapped row is a wall rather than a menu, and
    /// "Lawn" next to "Checker" says nothing about which of them is ground
    /// cover. The grouping is the whole difference between a list you read and
    /// one you scan past.
    pub const GROUPS: [(&'static str, &'static [TextureKind]); 3] = [
        ("Surfaces", TextureKind::SURFACES),
        ("Grass", TextureKind::GRASS),
        ("Ground", TextureKind::DIRT),
    ];

    /// Every texture, in menu order.
    pub const ALL: [TextureKind; 21] = [
        TextureKind::Paper,
        TextureKind::Canvas,
        TextureKind::Noise,
        TextureKind::Checker,
        TextureKind::Dots,
        TextureKind::Stripes,
        TextureKind::Bricks,
        TextureKind::Wood,
        TextureKind::Hatch,
        TextureKind::GrassLawn,
        TextureKind::GrassBlades,
        TextureKind::GrassMeadow,
        TextureKind::GrassDry,
        TextureKind::GrassMoss,
        TextureKind::GrassClover,
        TextureKind::DirtSoil,
        TextureKind::DirtGravel,
        TextureKind::DirtSand,
        TextureKind::DirtCracked,
        TextureKind::DirtPebbles,
        TextureKind::DirtMud,
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
            TextureKind::GrassLawn => "Lawn",
            TextureKind::GrassBlades => "Blades",
            TextureKind::GrassMeadow => "Meadow",
            TextureKind::GrassDry => "Straw",
            TextureKind::GrassMoss => "Moss",
            TextureKind::GrassClover => "Clover",
            TextureKind::DirtSoil => "Soil",
            TextureKind::DirtGravel => "Gravel",
            TextureKind::DirtSand => "Sand",
            TextureKind::DirtCracked => "Cracked",
            TextureKind::DirtPebbles => "Pebbles",
            TextureKind::DirtMud => "Mud",
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
            // **The cellular kinds need a lattice to be cellular on.** One
            // cell across the tile is one stone the size of the tile, which is
            // not gravel; two is the fewest that reads as a scatter.
            TextureKind::GrassClover
            | TextureKind::DirtGravel
            | TextureKind::DirtCracked
            | TextureKind::DirtPebbles => 2,
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

            // Ground cover. The number is how coarse the feature is: a blade
            // is small and there are a great many of them, a pebble is large
            // and there are not.
            TextureKind::GrassLawn | TextureKind::GrassBlades | TextureKind::GrassMoss => 8,
            TextureKind::GrassMeadow | TextureKind::GrassDry | TextureKind::GrassClover => 4,
            TextureKind::DirtGravel | TextureKind::DirtSand => 8,
            TextureKind::DirtSoil | TextureKind::DirtCracked | TextureKind::DirtPebbles => 4,
            TextureKind::DirtMud => 2,
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

        // -- grass ----------------------------------------------------------
        //
        // What makes a patch of ground read as *grass* rather than as green
        // noise is that its features are **tall and thin**: a blade is one
        // texel across and twenty long. So every one of these is built from
        // noise whose lattice is finer across than it is along — see
        // [`value_noise_xy`] — and what separates them is how hard that field
        // is thresholded and what is laid under it.
        TextureKind::GrassLawn => {
            let (u, v) = unit(x, y, n);
            // Mown turf: the blades are short, dense and nearly even, so the
            // threshold is soft and a slow clump keeps it off being flat.
            let blade = fbm_xy(u, v, detail * 6, detail * 2, 0x4C_11);
            let clump = value_noise_xy(u, v, detail, detail, 0x77_E3);
            (0.16 + 0.92 * smoothstep(0.40, 0.86, blade) + 0.20 * (clump - 0.5)).clamp(0.0, 1.0)
        }
        TextureKind::GrassBlades => {
            let (u, v) = unit(x, y, n);
            // Separated blades: the same field thresholded **hard**, so what
            // is left is the blades themselves with the soil showing between
            // them rather than a continuous mat.
            let blade = fbm_xy(u, v, detail * 8, detail, 0x1D_44);
            let soil = value_noise_xy(u, v, detail * 2, detail * 2, 0x90_12);
            (0.05 + 0.14 * soil + 0.88 * smoothstep(0.55, 0.74, blade)).clamp(0.0, 1.0)
        }
        TextureKind::GrassMeadow => {
            let (u, v) = unit(x, y, n);
            // Rough grass: the blades are modulated by a much slower field, so
            // it grows in tufts with thin ground between them — which is what
            // separates a meadow from a lawn.
            let tuft = value_noise_xy(u, v, detail, detail, 0x2B_75);
            let blade = fbm_xy(u, v, detail * 5, detail * 2, 0x6E_09);
            (0.12 + 0.90 * smoothstep(0.30, 0.82, blade * (0.55 + 0.80 * tuft))).clamp(0.0, 1.0)
        }
        TextureKind::GrassDry => {
            let (u, _) = unit(x, y, n);
            // **Straw lies over, not up.** The strands run diagonally, which
            // is a shear of the tile — and a shear by a whole tile is one of
            // the few maps that takes the torus to itself, so it still wraps.
            // Taken in *integers* rather than as `u + v`: at the far edge the
            // float sum has lost the bits that decide the colour and the join
            // shows. `(x + y) % n` has not.
            let w = ((x + y) % n) as f32 / n as f32;
            let strand = fbm_xy(u, w, detail, detail * 8, 0x8A_3C);
            let litter = value_noise_xy(u, w, detail * 2, detail * 6, 0xC1_07);
            (0.20 + 0.16 * litter + 0.80 * smoothstep(0.50, 0.86, strand)).clamp(0.0, 1.0)
        }
        TextureKind::GrassMoss => {
            let (u, v) = unit(x, y, n);
            // Moss has no blades to speak of: it is a fine even cushion, so
            // this is round noise held in a narrow band, with dark specks
            // punched through it for the shade between the shoots.
            let cushion = fbm_at(u, v, detail * 3, 0x3F_71);
            let shoot = fbm_xy(u, v, detail * 10, detail * 6, 0x55_1A);
            (0.26 + 0.52 * cushion + 0.34 * smoothstep(0.52, 0.78, shoot)
                - 0.30 * smoothstep(0.74, 1.0, 1.0 - shoot))
            .clamp(0.0, 1.0)
        }
        TextureKind::GrassClover => {
            let (u, v) = unit(x, y, n);
            // Broad leaves rather than blades: one rounded leaf per cell of a
            // jittered lattice, each its own size and tone, over fine grass.
            let cell = cells(u, v, detail, 0x6C_2E);
            let leaf = 1.0 - smoothstep(0.26 + 0.16 * cell.id, 0.44 + 0.16 * cell.id, cell.near);
            let under = fbm_xy(u, v, detail * 5, detail * 2, 0x4A_88);
            (0.14 + 0.34 * under + 0.62 * leaf * (0.70 + 0.45 * cell.id)).clamp(0.0, 1.0)
        }

        // -- bare ground ------------------------------------------------------
        //
        // Dirt is the other half of a floor, and its features are the opposite
        // of grass's: round, and packed. Four of the six are built on
        // [`cells`], a seamless cellular lattice — a stone is "everywhere
        // nearer to this feature point than to any other", which is what makes
        // gravel look packed rather than sprinkled.
        TextureKind::DirtSoil => {
            let (u, v) = unit(x, y, n);
            // Loam: clumps at two scales, with darker matter punched through.
            let loam = fbm_at(u, v, detail * 2, 0x21_D5);
            let bits = value_noise(u, v, detail * 8, 0x4E_92);
            (0.28 + 0.62 * loam - 0.26 * smoothstep(0.78, 1.0, bits)).clamp(0.0, 1.0)
        }
        TextureKind::DirtGravel => {
            let (u, v) = unit(x, y, n);
            // Small stones, each its own tone, on grit. The stone body is the
            // middle of its cell; the gaps between cells are the grit.
            let cell = cells(u, v, detail * 2, 0x13_A7);
            let stone = 1.0 - smoothstep(0.20, 0.46, cell.near);
            let grit = value_noise(u, v, detail * 8, 0x33_B1);
            (0.10 + 0.22 * grit + 0.80 * stone * (0.45 + 0.60 * cell.id)).clamp(0.0, 1.0)
        }
        TextureKind::DirtSand => {
            let (u, v) = unit(x, y, n);
            // Fine grain, plus the ripple wind leaves across it. The ripple's
            // phase is wrapped **before** the sine, for the reason the wood
            // rings are: at the far edge an f32 has no bits left to hold the
            // fraction that decides the colour, and the seam is real.
            let grain = value_noise(u, v, detail * 12, 0x7B_40);
            let fine = value_noise(u, v, detail * 24, 0x1A_66);
            let phase = (v * detail as f32).rem_euclid(1.0);
            // **The wander has to beat the ripple.** At a third of a period the
            // bands stayed parallel and evenly spaced, and evenly spaced bands
            // are corduroy, not sand. Past a whole period the ripple wanders
            // further than it is wide, so it breaks up and drifts the way
            // wind-blown sand does.
            let wander = fbm_at(u, v, detail.max(2), 0x2C_88);
            let ripple = ((phase + wander * 1.15).rem_euclid(1.0) * std::f32::consts::TAU).sin();
            (0.46 + 0.09 * ripple + 0.30 * (grain - 0.5) + 0.22 * (fine - 0.5)).clamp(0.0, 1.0)
        }
        TextureKind::DirtCracked => {
            let (u, v) = unit(x, y, n);
            // Dry mud. A crack is where two cells meet — where the nearest and
            // the second-nearest feature points are the same distance away —
            // so the whole pattern falls out of the difference between them.
            let cell = cells(u, v, detail, 0x5D_C3);
            let crack = 1.0 - smoothstep(0.015, 0.11, cell.next - cell.near);
            let plate = 0.55 + 0.32 * cell.id + 0.18 * fbm_at(u, v, detail * 4, 0x99_02);
            (plate * (1.0 - crack)).clamp(0.0, 1.0)
        }
        TextureKind::DirtPebbles => {
            let (u, v) = unit(x, y, n);
            // Bigger stones, packed: each domed from its own middle outwards,
            // with a dark line in the gap where two of them meet.
            let cell = cells(u, v, detail, 0x0A_51);
            let gap = smoothstep(0.02, 0.10, cell.next - cell.near);
            let dome = 1.0 - smoothstep(0.0, 0.55, cell.near);
            (0.10 + 0.88 * gap * (0.30 + 0.52 * dome + 0.34 * (cell.id - 0.5))).clamp(0.0, 1.0)
        }
        TextureKind::DirtMud => {
            let (u, v) = unit(x, y, n);
            // Wet mud: broad slick patches that go dark, with a sheen along
            // the ridges standing out of them.
            let mud = fbm_at(u, v, detail * 3, 0x6F_A4);
            let wet = smoothstep(0.40, 0.68, fbm_at(u, v, detail.max(1), 0x1E_B8));
            let sheen = smoothstep(0.60, 0.82, mud);
            // Trodden: the fine grit that keeps a wet patch from being a blur.
            let grit = value_noise(u, v, detail * 10, 0x40_D9);
            ((0.20 + 0.70 * mud + 0.16 * (grit - 0.5)) * (1.0 - 0.62 * wet)
                + 0.38 * wet * sheen)
                .clamp(0.0, 1.0)
        }
    }
}

/// A texel's position in the unit square, which is where every noise-based kind
/// works. `x == n` gives exactly `1.0`, and every generator here is periodic
/// with period one, so the texel past the last column is the first one again.
fn unit(x: u32, y: u32, n: u32) -> (f32, f32) {
    (x as f32 / n as f32, y as f32 / n as f32)
}

/// What a cellular lattice says about one point: how far the nearest feature
/// point is, how far the second-nearest is, and a stable random number for the
/// nearest one.
///
/// Distances are in **cells**, so they do not change with the tile size: 0.5 is
/// half a cell however many texels that turns out to be.
struct Cell {
    /// Distance to the nearest feature point.
    near: f32,
    /// Distance to the second-nearest. `next - near` is zero exactly on the
    /// line where two cells meet, which is what draws a crack.
    next: f32,
    /// A stable `0..1` for the nearest cell, so each stone can be its own tone.
    id: f32,
}

/// **A seamless cellular ("Worley") lattice**: one jittered feature point per
/// cell of a `per`x`per` grid, with the neighbourhood wrapped, so it tiles the
/// way the value noise above does.
///
/// # Why the offsets are measured inside the cell
///
/// The obvious version compares absolute positions — the feature point at
/// `cell + jitter` against the sample at `u * per` — and it has a seam you can
/// see. At the far edge of a 128-cell lattice those numbers are around 128, an
/// f32 carries 24 bits, and the difference that decides the colour is the last
/// of them. Subtracting the cell first keeps every number in `-2..2`, where the
/// arithmetic at `u = 1` is *bit for bit* the arithmetic at `u = 0`. That is
/// the difference between a tile that wraps and one that nearly does.
fn cells(u: f32, v: f32, per: u32, seed: u32) -> Cell {
    let per = per.max(1);
    let (fx, fy) = (u * per as f32, v * per as f32);
    let (cx, cy) = (fx.floor(), fy.floor());
    // Where the sample sits *within* its own cell.
    let (lx, ly) = (fx - cx, fy - cy);
    let (cxi, cyi) = (cx as i64, cy as i64);

    let mut near = f32::INFINITY;
    let mut next = f32::INFINITY;
    let mut id = 0.0;
    for dy in -1..=1i64 {
        for dx in -1..=1i64 {
            let wx = (cxi + dx).rem_euclid(per as i64) as u32;
            let wy = (cyi + dy).rem_euclid(per as i64) as u32;
            // Held off the cell walls, so a feature point never lands exactly
            // on a boundary and two neighbours never coincide.
            let jx = 0.15 + 0.70 * lattice(wx, wy, seed);
            let jy = 0.15 + 0.70 * lattice(wx, wy, seed ^ 0x5A5A);
            let ox = dx as f32 + jx - lx;
            let oy = dy as f32 + jy - ly;
            let d = (ox * ox + oy * oy).sqrt();
            if d < near {
                next = near;
                near = d;
                id = lattice(wx, wy, seed ^ 0x1357);
            } else if d < next {
                next = d;
            }
        }
    }
    Cell { near, next, id }
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

/// The same, with a **different period in each axis** — how a grass grows.
///
/// A blade of grass is one texel across and twenty long, and a square lattice
/// cannot make that shape at any frequency: it gives blobs, and blobs of green
/// read as moss or as noise. A lattice that is fine across and coarse along
/// gives a feature that is thin and tall, which is a blade. Every grass kind
/// above is this function and a threshold.
///
/// Each axis wraps on its own period, so the tile is still seamless.
fn value_noise_xy(u: f32, v: f32, per_x: u32, per_y: u32, seed: u32) -> f32 {
    let (per_x, per_y) = (per_x.max(1), per_y.max(1));
    let fx = u * per_x as f32;
    let fy = v * per_y as f32;
    let x0 = fx.floor() as i64;
    let y0 = fy.floor() as i64;
    let tx = smooth(fx - x0 as f32);
    let ty = smooth(fy - y0 as f32);
    let wx = |i: i64| i.rem_euclid(per_x as i64) as u32;
    let wy = |i: i64| i.rem_euclid(per_y as i64) as u32;
    let (ax0, ax1) = (wx(x0), wx(x0 + 1));
    let (ay0, ay1) = (wy(y0), wy(y0 + 1));
    let v00 = lattice(ax0, ay0, seed);
    let v10 = lattice(ax1, ay0, seed);
    let v01 = lattice(ax0, ay1, seed);
    let v11 = lattice(ax1, ay1, seed);
    let a = v00 + (v10 - v00) * tx;
    let b = v01 + (v11 - v01) * tx;
    a + (b - a) * ty
}

/// [`fbm_at`], stretched: three octaves of [`value_noise_xy`], both periods
/// doubling together so the stretch is kept at every scale.
fn fbm_xy(u: f32, v: f32, per_x: u32, per_y: u32, seed: u32) -> f32 {
    let mut sum = 0.0;
    let mut amp = 1.0;
    let mut norm = 0.0;
    let (mut px, mut py) = (per_x.max(1), per_y.max(1));
    for o in 0..3 {
        sum += amp * value_noise_xy(u, v, px, py, seed.wrapping_add(o));
        norm += amp;
        amp *= 0.5;
        px *= 2;
        py *= 2;
    }
    sum / norm
}

/// Bilinear value noise on a `per`×`per` toroidal lattice, so it tiles: lattice
/// indices wrap mod `per`, making the left edge continue into the right and the
/// top into the bottom.
fn value_noise(u: f32, v: f32, per: u32, seed: u32) -> f32 {
    value_noise_xy(u, v, per, per, seed)
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

    /// **Every texture has to have a pattern in it**, not only the ones that
    /// were checked when they were written.
    ///
    /// A generator that clamps flat — a threshold nothing crosses, a noise
    /// scaled to nothing — bakes a tile of one colour, which is a solid fill
    /// that costs an image. It looks like a bug in the fill rather than in the
    /// texture, so it is worth one assertion per kind.
    #[test]
    fn every_kind_has_a_pattern_in_it() {
        for kind in TextureKind::ALL {
            let px = TextureRecipe::new(kind, FG, BG).bake(128);
            let lo = px.chunks_exact(4).map(|p| p[0]).min().unwrap();
            let hi = px.chunks_exact(4).map(|p| p[0]).max().unwrap();
            assert!(
                hi > lo + 8,
                "{} bakes a flat tile, {lo}..{hi}",
                kind.label()
            );
        }
    }

    /// The three families together are every texture, and no texture is in two
    /// of them — so a panel that shows the groups shows the whole menu.
    #[test]
    fn the_groups_cover_every_texture_exactly_once() {
        let mut listed: Vec<TextureKind> = Vec::new();
        for (_, family) in TextureKind::GROUPS {
            listed.extend_from_slice(family);
        }
        assert_eq!(
            listed.len(),
            TextureKind::ALL.len(),
            "the groups and the menu disagree on how many textures there are"
        );
        for kind in TextureKind::ALL {
            assert_eq!(
                listed.iter().filter(|k| **k == kind).count(),
                1,
                "{} is not in exactly one group",
                kind.label()
            );
        }
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
