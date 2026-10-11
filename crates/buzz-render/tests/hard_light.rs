//! **The terminator from geometry, on actual pixels.**
//! `docs/HARD_LIGHT_PLAN.md`, Stage A.
//!
//! `buzz-light`'s own tests prove the crescent geometry in isolation; this
//! proves the renderer draws it. A grey disc, a hard sun close to its side
//! (`lz \u{2248} 0.15`, a little past grazing — see `ELEVATION`) under
//! [`ShadeModel::Form`]: the near side must read brighter than the far side,
//! the split must sit near the disc's own centre, and a hard light's
//! terminator must be a much narrower transition than a soft one's — the
//! whole report Stage A exists to fix (`buzz-light/src/geometry.rs`'s
//! `shade_width`, before this, could never darken more than a sliver at any
//! softness).
//!
//! Skips rather than fails without a GPU, exactly as
//! `buzz-app/tests/stage_lighting.rs` does, and for the same reason: this is a
//! report about the renderer, not about the machine it happens to run on.

use buzz_geom::{Camera, Point, Shape as _, Size};
use buzz_render::document::{self, DrawCache, FrameOptions};
use buzz_render::{GpuContext, GpuPreference, SceneBuilder, vello, wgpu};
use buzz_scene::{LayerKind, Light, LightId, LightKind, Scene, ShadeModel, ShapeData};
use peniko::Color;

// 256 rather than 300: `bytes_per_row` (`W * 4`) must respect wgpu's copy
// alignment, and 256 is the one round number both square and aligned.
const W: u32 = 256;
const H: u32 = 256;
const BACKGROUND: Color = Color::from_rgb8(0x10, 0x10, 0x10);
const DISC: Color = Color::from_rgb8(0xB0, 0xB0, 0xB0);

/// The disc's own placement, in document units — chosen so its equator (the
/// row through its centre) spans almost the whole frame, and its edges still
/// sit inside it, so the picture is recognisably a lit circle rather than a
/// blob filling the canvas.
const CENTER: Point = Point::new(128.0, 128.0);
const RADIUS: f64 = 110.0;

struct Harness {
    gpu: GpuContext,
    texture: wgpu::Texture,
    view: wgpu::TextureView,
    readback: wgpu::Buffer,
}

fn harness() -> Option<Harness> {
    let gpu = match GpuContext::new_blocking(&GpuPreference::Automatic) {
        Ok(g) => g,
        Err(e) => {
            eprintln!("skipping hard light test: no usable GPU ({e})");
            return None;
        }
    };
    let texture = gpu.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("hard-light-target"),
        size: wgpu::Extent3d {
            width: W,
            height: H,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: buzz_render::RENDER_FORMAT,
        usage: wgpu::TextureUsages::STORAGE_BINDING
            | wgpu::TextureUsages::TEXTURE_BINDING
            | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
    let readback = gpu.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("hard-light-readback"),
        size: (W * H * 4) as u64,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    Some(Harness {
        gpu,
        texture,
        view,
        readback,
    })
}

impl Harness {
    /// Encode the scene through the same walk the window and the exporter
    /// share (`buzz_render::document`), render it, and read the pixels back.
    fn render(&mut self, scene: &Scene) -> Vec<u8> {
        let camera = Camera::new(CENTER, 1.0, Size::new(W as f64, H as f64));
        let mut vs = vello::Scene::new();
        let mut builder = SceneBuilder::new(&mut vs, &camera);
        let options = FrameOptions {
            lit: true,
            ..FrameOptions::default()
        };
        let mut cache = DrawCache::new();
        cache.begin();
        document::draw_frame_within(
            &mut builder,
            scene,
            0,
            buzz_geom::Affine::IDENTITY,
            &options,
            &mut cache,
        );
        cache.end();

        self.gpu
            .render(&vs, &self.view, W, H, BACKGROUND)
            .expect("vello render");

        let mut encoder = self
            .gpu
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: &self.texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &self.readback,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(W * 4),
                    rows_per_image: Some(H),
                },
            },
            wgpu::Extent3d {
                width: W,
                height: H,
                depth_or_array_layers: 1,
            },
        );
        self.gpu.queue.submit([encoder.finish()]);

        let slice = self.readback.slice(..);
        slice.map_async(wgpu::MapMode::Read, |r| r.expect("map readback"));
        self.gpu
            .device
            .poll(wgpu::PollType::Wait {
                submission_index: None,
                timeout: Some(std::time::Duration::from_secs(30)),
            })
            .expect("poll device");
        let pixels = {
            let view = slice.get_mapped_range();
            view.to_vec()
        };
        self.readback.unmap();
        pixels
    }
}

/// The sun's elevation, chosen so `lz = sin(elevation)` comes out a little
/// over `0.1` — near enough a side light that `form_shade_width` still
/// darkens most of the disc, and enough past grazing incidence (`lz = 0`
/// exactly, `N\u{00b7}L = 0`, no direct light at all) that the shaded side
/// reads as genuinely dimmer than the lit one rather than merely un-glinted.
/// See `buzz-light`'s own `a_side_light_darkens_half_the_form` for the
/// pure-geometry version of this at `lz = 0` exactly, where there is no
/// render to confuse the measurement.
///
/// **A sun, not a lamp**, so the picture is the crescent's own geometry and
/// nothing else: a lamp whose brightness varies across the disc is drawn
/// with its own radial falloff ramp (`LightRig::field`'s lamp treatment),
/// which has nothing to do with `softness` and would swamp the very thing
/// [`a_hard_terminator_is_narrower_than_a_soft_one`] measures. A sun's light
/// is the same everywhere, so the only ramp across the disc is the one this
/// stage adds.
const ELEVATION: f64 = 0.1488; // asin(0.148)

/// A grey disc under a rig with one sun to its left, low enough to be nearly
/// a side light. `model` and `softness` are the only things that vary
/// between the tests below.
fn scene(model: ShadeModel, softness: f64) -> Scene {
    let mut scene = Scene::default();
    scene.stage_mut().background = BACKGROUND;
    scene.stage_mut().size = Size::new(W as f64, H as f64);

    let layer = scene.add_layer("Art", LayerKind::Normal);
    let circle = kurbo::Circle::new(CENTER, RADIUS);
    scene.add_shape(layer, ShapeData::filled(circle.to_path(1e-6), DISC));

    scene.lights_mut().enabled = true;
    scene.lights_mut().model = model;
    scene.lights_mut().modelling = 1.0;
    // A near-black fill, rather than the mid-grey default meant for a
    // document with nothing yet to contrast it against, so the shaded side
    // and the lit one read distinctly apart.
    scene.lights_mut().base = Color::from_rgb8(0x18, 0x18, 0x1C);
    let mut sun = Light::new(
        LightId(1),
        "Key",
        LightKind::Sun {
            // `azimuth.cos()` negative points the sun's planar direction to
            // the left of the picture — see `Light::screen_towards`.
            azimuth: std::f64::consts::PI,
            elevation: ELEVATION,
        },
    );
    sun.softness = softness;
    sun.intensity = 3.0;
    sun.glint = 1.0;
    scene.lights_mut().lights.push(sun);

    scene
}

fn luma(px: &[u8]) -> f64 {
    0.2126 * px[0] as f64 + 0.7152 * px[1] as f64 + 0.0722 * px[2] as f64
}

/// Mean luma over `[x0, x1) x [y0, y1)`.
fn mean_luma(pixels: &[u8], x0: u32, x1: u32, y0: u32, y1: u32) -> f64 {
    let mut sum = 0.0;
    let mut n = 0usize;
    for y in y0..y1 {
        for x in x0..x1 {
            let i = ((y * W + x) * 4) as usize;
            sum += luma(&pixels[i..i + 4]);
            n += 1;
        }
    }
    sum / n.max(1) as f64
}

/// Mean luma per column, averaged down the disc's equatorial band — the
/// brightness profile across the terminator.
fn column_profile(pixels: &[u8], y0: u32, y1: u32) -> Vec<f64> {
    (0..W)
        .map(|x| mean_luma(pixels, x, x + 1, y0, y1))
        .collect()
}

/// The equatorial band a column profile is measured over: the middle tenth
/// of the disc's own height, comfortably clear of its curved top and bottom.
fn equator() -> (u32, u32) {
    let half = (RADIUS * 0.1).round() as u32;
    let mid = CENTER.y.round() as u32;
    (mid.saturating_sub(half), mid + half)
}

/// **Deep in the lit side**, clear of the narrow glint right at the edge
/// nearest the light — a fifth of the diameter in from the disc's own left
/// edge, which `HIGHLIGHT_SHARE` (30% of the shaded width, itself under half
/// the diameter) never reaches.
fn lit_reference_x() -> u32 {
    (CENTER.x - RADIUS * 0.6) as u32
}

/// **Inside the Form model's shaded side, and outside the Crescent model's.**
///
/// Not simply "deep in shade": at `lz` this close to zero, Form's shaded
/// side reaches nearly to the disc's own centre, but the Crescent model's
/// hard-light floor (`HARD_SHADE_WIDTH`, `0.3` of the diameter) shades only
/// the fifth of the disc nearest its far edge. `0.28` of the radius past
/// centre sits inside Form's band and outside Crescent's, which is what lets
/// `the_crescent_model_still_gives_a_narrow_hard_band` tell the two apart at
/// the same point `a_side_light_darkens_the_far_side_of_a_disc` calls shaded.
fn shade_reference_x() -> u32 {
    (CENTER.x + RADIUS * 0.28) as u32
}

/// **The full-shade plateau, however soft the feather is.**
///
/// The shaded band's colour ramps from full shade at its *outer* edge — the
/// disc's own far edge, here — to the lit colour at the terminator, over
/// [`buzz_light::shade_feather(softness)`] of the band's own width. That
/// outer anchor does not move with softness, only how far the ramp reaches
/// in from it, so a point close to the disc's edge reads as full shade at
/// any softness — unlike [`shade_reference_x`], which sits inside a soft
/// light's ramp and cannot be reused across it.
fn shade_edge_x() -> u32 {
    (CENTER.x + RADIUS - 4.0) as u32
}

/// Where the profile crosses `target`, searching only `x0..x1` — column
/// indices, in the profile's own units — to sub-pixel precision by linear
/// interpolation. Kept away from the disc's edges and, for a shaded-side
/// crossing, away from the narrow glint on the lit side, both of which are
/// transitions of their own that would otherwise be found first.
fn crossing(profile: &[f64], target: f64, x0: usize, x1: usize) -> f64 {
    for w in x0..x1.min(profile.len() - 1) {
        let (a, b) = (profile[w], profile[w + 1]);
        if (a - target) * (b - target) <= 0.0 && a != b {
            let t = (target - a) / (b - a);
            return w as f64 + t;
        }
    }
    (x0 + x1) as f64 / 2.0
}

/// **A hard side light darkens most of the disc**, which the Crescent model
/// can never do at any softness — see `buzz-light`'s own
/// `a_side_light_darkens_half_the_form`. The sun lies to the left of the
/// disc, so the near (left) side must read brighter than the far (right)
/// side, and the shaded side's own edge must fall near the disc's centre
/// column — see `form_shade_width`, which puts it there at `lz` this close
/// to zero.
#[test]
fn a_side_light_darkens_the_far_side_of_a_disc() {
    let Some(mut h) = harness() else { return };
    let scene = scene(ShadeModel::Form, 0.05);
    let pixels = h.render(&scene);
    let (y0, y1) = equator();

    let lit_x = lit_reference_x();
    let shade_x = shade_edge_x();
    let lit = mean_luma(&pixels, lit_x, lit_x + 4, y0, y1);
    let shade = mean_luma(&pixels, shade_x - 4, shade_x + 4, y0, y1);
    assert!(
        lit > shade * 1.6,
        "the lit side ({lit:.1}) should be well over the shaded side ({shade:.1})"
    );

    // The shaded side's own boundary — not the narrow glint's — is what
    // `form_shade_width` places near the disc's centre at this `lz`. Search
    // from just past the glint rightward, so the glint's own edge is not
    // mistaken for it.
    let profile = column_profile(&pixels, y0, y1);
    let mid = (lit + shade) / 2.0;
    let search_from = lit_x as usize + 4;
    let search_to = (shade_x + 4) as usize;
    let at = crossing(&profile, mid, search_from, search_to);
    let offset_pct = ((at - CENTER.x).abs() / (RADIUS * 2.0)) * 100.0;
    // The plan's own fixture puts this within 6% at `lz = 0` exactly, where
    // `form_shade_width` is precisely `0.5`. Measured here at `lz \u{2248} 0.15`
    // — moved off zero so there is direct light for the shaded side to lose,
    // see `ELEVATION` — it comes out a little over 7%: `form_shade_width`
    // itself is `0.426` there, and a lune's own boundary does not move
    // linearly with the shift that builds it. 9% keeps the assertion honest
    // about that gap while still failing if the split drifted towards an
    // edge instead of the middle.
    assert!(
        offset_pct < 9.0,
        "the shaded side's edge sat {offset_pct:.1}% of the diameter from the disc's \
         centre, expected it near the middle for a light this close to the side"
    );
}

/// **The Crescent model is unaffected.** A softness of 0.05 there is the
/// narrowest band the old floor allows, not most of the disc — this is the
/// picture Stage A must not have changed.
#[test]
fn the_crescent_model_still_gives_a_narrow_hard_band() {
    let Some(mut h) = harness() else { return };
    let scene = scene(ShadeModel::Crescent, 0.05);
    let pixels = h.render(&scene);
    let (y0, y1) = equator();

    let lit_x = lit_reference_x();
    let shade_x = shade_reference_x();
    let lit = mean_luma(&pixels, lit_x, lit_x + 4, y0, y1);
    let shade = mean_luma(&pixels, shade_x - 4, shade_x, y0, y1);
    assert!(
        lit < shade * 1.3,
        "the old model's hard-light floor should leave a point this far in from the \
         edge almost as bright as the lit side, not darken most of the disc: \
         {lit:.1} against {shade:.1}"
    );
}

/// **A hard light's terminator is a much narrower transition than a soft
/// one's**, which is the whole point of moving the width off the softness
/// slider: softness is left to do only the feathering it was always meant to.
#[test]
fn a_hard_terminator_is_narrower_than_a_soft_one() {
    let Some(mut h) = harness() else { return };
    let (y0, y1) = equator();
    let lit_x = lit_reference_x();
    // The full-shade plateau, not `shade_reference_x`: a soft light's ramp
    // reaches most of the way across the band, and that point sits inside
    // it rather than past it. See `shade_edge_x`.
    let shade_x = shade_edge_x();

    let width_at = |h: &mut Harness, softness: f64| -> f64 {
        let scene = scene(ShadeModel::Form, softness);
        let pixels = h.render(&scene);
        let lit = mean_luma(&pixels, lit_x, lit_x + 4, y0, y1);
        let shade = mean_luma(&pixels, shade_x - 4, shade_x + 4, y0, y1);
        let (from, to) = (lit - (lit - shade) * 0.1, lit - (lit - shade) * 0.9);
        let profile = column_profile(&pixels, y0, y1);
        let search_from = lit_x as usize + 4;
        let search_to = shade_x as usize + 4;
        (crossing(&profile, to, search_from, search_to)
            - crossing(&profile, from, search_from, search_to))
        .abs()
    };

    let hard = width_at(&mut h, 0.05);
    let soft = width_at(&mut h, 0.9);
    assert!(
        soft > hard * 3.0,
        "a soft light's transition ({soft:.1}px) should be much wider than a hard \
         one's ({hard:.1}px)"
    );
}
