//! Getting a finished animation *out*: PNG images and PNG sequences.
//!
//! # One picture of the document, not two
//!
//! Every frame is rendered through `buzz_render::document`, the same walk the
//! window uses, onto the GPU, through the same Vello renderer. Nothing here
//! knows how to draw a shape. That is deliberate and it is the whole design:
//! an exporter with its own drawing code eventually disagrees with the screen,
//! and the user finds out after the render finishes rather than while they are
//! working.
//!
//! Chrome cannot leak in for the same reason — rulers, guides, selection
//! handles and onion skins are drawn by the application in screen space and
//! are not part of that walk.
//!
//! # What comes out
//!
//! [`Frame`] holds **unpremultiplied** RGBA8, which is what PNG stores. Vello
//! composites in premultiplied alpha, so a transparent export is divided back
//! out on the way here; an opaque one is unaffected, since dividing by 1 is
//! the identity.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use buzz_geom::{Camera, Point, Rect, Size};
use buzz_render::{GpuContext, GpuPreference, RENDER_FORMAT, SceneBuilder, document, wgpu};
use buzz_scene::Scene;
use peniko::Color;
use rayon::prelude::*;

/// Rows in a texture-to-buffer copy must start on a 256-byte boundary.
///
/// A 550-pixel-wide stage is 2200 bytes per row, which is *not* a multiple of
/// 256 — so the copy is padded and the padding stripped on the way out. Every
/// interesting document width hits this; only the round powers of two that
/// test suites like to use avoid it.
const COPY_ALIGNMENT: u32 = 256;

pub mod film;
pub mod gif;
pub mod preset;
pub mod reel;
pub mod video;
pub mod xfl;

pub use film::concat_segments;
pub use preset::{ExportPreset, PresetFormat};
pub use reel::Reel;
pub use xfl::{FlaError, FlaReport, export_fla, fla_bytes};

pub use gif::{
    AnimatedFormat, AnimatedReport, Dither, GifSettings, WebpSettings, export_gif, export_webp,
};
pub use video::{
    AudioTrack, VideoCodec, VideoContainer, VideoReport, VideoSettings, export_video,
    ffmpeg_available,
};

/// What to render, and how large.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ExportSettings {
    pub width: u32,
    pub height: u32,
    /// Export with no background, so the artwork can be composited elsewhere.
    ///
    /// Off by default: Animate's Export Image writes the stage as it looks,
    /// and a file that silently lost its background would surprise anyone who
    /// did not ask for it.
    pub transparent: bool,
    /// Render only this rectangle of the stage (in document units), scaled to
    /// fill `width`×`height`. `None` frames the whole stage — the usual case.
    /// A render region, for re-rendering a corner without redoing the frame.
    pub region: Option<buzz_geom::Rect>,
}

impl ExportSettings {
    /// The stage at 100%: one document unit to one pixel, which is what
    /// Animate's Export Image offers first.
    pub fn for_stage(scene: &Scene) -> Self {
        let size = scene.stage().size;
        Self {
            width: size.width.round().max(1.0) as u32,
            height: size.height.round().max(1.0) as u32,
            transparent: false,
            region: None,
        }
    }

    /// The stage scaled by a factor — 2.0 for a retina still, 0.5 for a
    /// contact sheet.
    pub fn scaled(scene: &Scene, factor: f64) -> Self {
        let base = Self::for_stage(scene);
        let scale = |v: u32| ((v as f64 * factor).round().max(1.0)) as u32;
        Self {
            width: scale(base.width),
            height: scale(base.height),
            ..base
        }
    }

    /// The scale this size represents relative to the stage.
    ///
    /// Taken from the width, and the height follows it: an export whose aspect
    /// ratio differs from the stage's shows more or less of the pasteboard
    /// rather than stretching the artwork, which is what Animate does and what
    /// anybody would expect from a camera.
    fn zoom(&self, scene: &Scene) -> f64 {
        let stage_width = scene.stage().size.width;
        if stage_width <= 0.0 {
            return 1.0;
        }
        self.width as f64 / stage_width
    }
}

/// One rendered frame: unpremultiplied RGBA8, row-major, no padding.
#[derive(Debug, Clone, PartialEq)]
pub struct Frame {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u8>,
}

impl Frame {
    /// The pixel at `(x, y)` as `[r, g, b, a]`.
    pub fn pixel(&self, x: u32, y: u32) -> [u8; 4] {
        let i = ((y * self.width + x) * 4) as usize;
        [
            self.pixels[i],
            self.pixels[i + 1],
            self.pixels[i + 2],
            self.pixels[i + 3],
        ]
    }

    /// Encode as PNG into memory.
    pub fn to_png(&self) -> Result<Vec<u8>> {
        let mut out = Vec::new();
        {
            let mut encoder = png::Encoder::new(&mut out, self.width, self.height);
            encoder.set_color(png::ColorType::Rgba);
            encoder.set_depth(png::BitDepth::Eight);
            let mut writer = encoder.write_header().context("writing the PNG header")?;
            writer
                .write_image_data(&self.pixels)
                .context("writing PNG pixels")?;
        }
        Ok(out)
    }

    /// Write a PNG file.
    ///
    /// Written to a temporary file beside the target and renamed into place,
    /// so an interrupted export cannot leave a half-written image where the
    /// user expects a finished one — the same rule `.buzz` saves follow.
    pub fn write_png(&self, path: &Path) -> Result<()> {
        let encoded = self.to_png()?;
        let temp = path.with_extension("png.part");
        std::fs::write(&temp, &encoded).with_context(|| format!("writing {}", temp.display()))?;
        std::fs::rename(&temp, path)
            .with_context(|| format!("renaming into {}", path.display()))?;
        Ok(())
    }
}

/// A GPU brought up for exporting, with a render target sized to the job.
///
/// Held across frames on purpose: bringing up a device costs tens of
/// milliseconds, which would dominate a 500-frame sequence.
pub struct Exporter {
    gpu: GpuContext,
    target: Option<Target>,
    /// Lighting geometry, kept across the frames of a sequence: consecutive
    /// frames share most of their artwork, so most of the shadows are the
    /// same shadows.
    lights: buzz_render::document::DrawCache,
    /// The full-frame compositor and its own render target, built the first
    /// time a film with effects is exported and then reused. Kept off the plain
    /// path so a document with no look allocates nothing extra.
    compositor: Option<buzz_render::Compositor>,
    post_target: Option<PostTarget>,
    /// On by default; `set_cull(false)` is the only way left to get a truly
    /// unculled render out of `draw_into_target`; once culling is on, leaving
    /// a caller's own `FrameOptions::cull` at `None` no longer means "render
    /// everything" here, it means "fill it in". A parity test that wants to
    /// compare the two needs a way to ask for the old behaviour honestly.
    cull: bool,
}

/// A second render target the compositor writes into, sized to the frame.
struct PostTarget {
    width: u32,
    height: u32,
    #[allow(dead_code, reason = "kept alive so the view stays valid")]
    texture: wgpu::Texture,
    view: wgpu::TextureView,
}

struct Target {
    width: u32,
    height: u32,
    texture: wgpu::Texture,
    view: wgpu::TextureView,
    readback: wgpu::Buffer,
    /// Bytes per row in the readback buffer, including alignment padding.
    padded_row: u32,
}

impl Exporter {
    /// Bring up a GPU for exporting.
    pub fn new(preference: &GpuPreference) -> Result<Self> {
        let mut gpu = GpuContext::new_blocking(preference).context("starting the GPU for export")?;
        // V2/V5 (`docs/VRAM_ADAPTIVE_BUFFERS_PLAN.md`): an export has no
        // frame budget and must never degrade quality, so unlike the window
        // (which now starts at `buffer_scale` `1` and only grows on demand)
        // this context starts generous, at the adapter's own
        // `max_buffer_scale` — exactly what `GpuContext::new` used to set
        // `buffer_scale` to directly, before V2 made the window's own start
        // small.
        gpu.start_generous();
        let mut lights = buzz_render::document::DrawCache::new();
        // P2 (`docs/IMPORT_PERF_PLAN.md`): adopt this exporter's own GPU
        // context's buffer scale, captured once, rather than leaving the
        // cache at `DrawCache::default()`'s scale of `1` — which would judge
        // every export against Vello's upstream buffer size and band-split
        // far more than P1's bigger transient buffers actually need.
        lights.set_scale(gpu.buffer_scale);
        Ok(Self {
            gpu,
            target: None,
            lights,
            compositor: None,
            post_target: None,
            cull: true,
        })
    }

    /// The adapter in use, for the log and the progress dialog.
    pub fn adapter_summary(&self) -> String {
        self.gpu.adapter_summary()
    }

    /// The largest square this GPU will render in one pass.
    pub fn max_dimension(&self) -> u32 {
        self.gpu.device.limits().max_texture_dimension_2d
    }

    /// Turn the symbol-encoding cache on or off for this exporter. Exposed so a
    /// parity test can render the same document both ways; the picture must be
    /// the same either way.
    pub fn set_symbol_reuse(&mut self, on: bool) {
        self.lights.set_symbol_reuse(on);
    }

    /// Turn the per-layer encode cache on or off for this exporter — P2 in
    /// `docs/IMPORT_PERF_PLAN.md`. Exposed for the same reason
    /// `set_symbol_reuse` is: a parity test can render both ways, and a test
    /// that means to isolate a *different* cache (the symbol one) can hold
    /// this one off so it cannot short-circuit the layer walk that other
    /// cache's own stamping is measured through.
    pub fn set_layer_reuse(&mut self, on: bool) {
        self.lights.set_layer_reuse(on);
    }

    /// Pin this exporter's own `DrawCache` to `scale` rather than whatever
    /// `Self::new` picked from the real adapter's own limits. P2 in
    /// `docs/IMPORT_PERF_PLAN.md`: the budget belongs to the cache, not the
    /// process, so a fixture calibrated to overrun the ceiling at Vello's
    /// upstream buffer size (`scale: 1`) stays meaningful regardless of which
    /// GPU runs the suite — without reaching for the process-wide
    /// `vello_encoding::set_buffer_scale`, which only ever rises and would
    /// race every other GPU context in the same test binary.
    pub fn set_budget_scale(&mut self, scale: u32) {
        self.lights.set_scale(scale);
    }

    /// Turn culling to the output rectangle on or off. On by default. Exposed
    /// so a parity test can ask `draw_into_target` for the old, genuinely
    /// unculled behaviour: once culling is on, a caller's own
    /// `FrameOptions::cull` left at `None` is filled in rather than honoured,
    /// so `None` alone can no longer say "render everything, undisturbed."
    pub fn set_cull(&mut self, on: bool) {
        self.cull = on;
    }

    /// (builds, stamps) of the symbol cache so far — for a parity test to
    /// confirm the cache actually engaged rather than passing vacuously.
    pub fn symbol_cache_stats(&self) -> (u64, u64) {
        (
            self.lights.symbol_scenes.builds,
            self.lights.symbol_scenes.stamps,
        )
    }

    /// Was the last frame drawn judged over either budget — the segment
    /// ceiling or the line budget — even at `LightDetail::Flat`, the bottom
    /// of the automatic ladder? Exposed so a test of `Self::render_bands` can
    /// assert its fixture actually reaches that path rather than passing
    /// vacuously through the ordinary single-pass one. See Phase 4d.
    pub fn lights_over_budget(&self) -> bool {
        self.lights.over_budget()
    }

    /// Drop all retained render geometry. The symbol table already refuses to
    /// serve one document's symbols for another, so this is not needed for
    /// correctness; a test uses it to render each unrelated document from a
    /// clean slate.
    pub fn reset_caches(&mut self) {
        let mut lights = buzz_render::document::DrawCache::new();
        lights.set_scale(self.gpu.buffer_scale);
        self.lights = lights;
    }

    /// Render one frame.
    pub fn render(
        &mut self,
        scene: &Scene,
        frame: u32,
        settings: &ExportSettings,
    ) -> Result<Frame> {
        // An export always shows the finished picture: masks clip and lights
        // light, whatever the stage happens to be showing while the document is
        // being worked on — and no authoring aid reaches the film.
        self.render_with(
            scene,
            frame,
            settings,
            &document::FrameOptions {
                lit: true,
                // Reference geometry is for the person drawing, not for the
                // audience. See `FrameOptions::guides`.
                guides: false,
                ..document::FrameOptions::default()
            },
        )
    }

    /// Render one frame with options of the caller's choosing.
    ///
    /// For the passes that are *not* the film: the working view, which honours
    /// each layer's transparency, and the tests that have to prove the two
    /// differ.
    pub fn render_with(
        &mut self,
        scene: &Scene,
        frame: u32,
        settings: &ExportSettings,
        options: &document::FrameOptions,
    ) -> Result<Frame> {
        // **Motion blur.** A shutter that is open for part of the frame records
        // everything that happens while it is, so the frame is drawn at several
        // instants across it and the results are added up.
        //
        // Gated on `lit`, exactly as the compositor is and for the same reason:
        // `lit` is the film's own flag, and the working-view and comparison
        // passes must stay the raw artwork at a clean instant.
        match scene.camera().shutter_offsets() {
            Some(offsets) if options.lit => {
                self.render_blurred(scene, frame, settings, options, &offsets)
            }
            _ => self.render_at(scene, frame, settings, options, 0.0),
        }
    }

    /// Draw the frame at every instant the shutter is open and add them up.
    ///
    /// # Why the samples are added on the CPU
    ///
    /// The obvious place is the GPU — accumulate into a float target and never
    /// read a sample back. That needs a new pipeline and a new intermediate
    /// format in the compositor; this needs neither, and an export is already
    /// paying the cost of drawing the frame `n` times, next to which the
    /// read-backs are not what makes it slow.
    ///
    /// # The post chain runs once, at the end
    ///
    /// A shutter integrates *light*; the film's look is the response to what it
    /// collected. So the samples are drawn raw, added, uploaded back into the
    /// target — which is a copy destination precisely so the compositor's
    /// result can go back into it — and only then graded, bloomed and grained.
    /// Doing it per sample would average the grain away, which is the one part
    /// of the look that is meant to be different on every frame.
    fn render_blurred(
        &mut self,
        scene: &Scene,
        frame: u32,
        settings: &ExportSettings,
        options: &document::FrameOptions,
        offsets: &[f64],
    ) -> Result<Frame> {
        // Each sample is the frame as it would ordinarily be drawn — lit and
        // shadowed, because that is part of the picture the shutter collects.
        // Only the compositor is held back, and it runs once at the end.
        let mut raw = options.clone();

        let mut sum: Vec<f32> = Vec::new();
        let weight = 1.0 / offsets.len() as f32;
        let (mut width, mut height) = (0u32, 0u32);

        for &offset in offsets {
            raw.subframe = offset;
            self.draw_into_target(scene, frame, settings, &raw)?;
            let sample = self.read_raw()?;
            if sum.is_empty() {
                width = sample.width;
                height = sample.height;
                sum = vec![0.0; sample.pixels.len()];
            }
            // **Added as light, not as bytes.** A shutter integrates what
            // arrives at the film, and the bytes are gamma-encoded, so adding
            // them directly makes every smear darker than the thing that made
            // it — a white shape crossing black comes out grey rather than
            // half-bright. Alpha is a coverage fraction and is already linear,
            // so it is added as it is.
            for (i, (acc, byte)) in sum.iter_mut().zip(sample.pixels.iter()).enumerate() {
                let value = if i % 4 == 3 {
                    *byte as f32 / 255.0
                } else {
                    srgb_to_linear(*byte)
                };
                *acc += weight * value;
            }
        }

        if sum.is_empty() {
            return self.render_at(scene, frame, settings, options, 0.0);
        }

        let mut pixels = vec![0u8; sum.len()];
        for (i, (out, acc)) in pixels.iter_mut().zip(sum.iter()).enumerate() {
            *out = if i % 4 == 3 {
                (acc * 255.0).round().clamp(0.0, 255.0) as u8
            } else {
                linear_to_srgb(*acc)
            };
        }

        self.upload(&pixels, width, height)?;

        let post = scene.stage().post;
        if options.lit && !post.is_identity() {
            self.apply_post(frame, &post, settings.width, settings.height);
        }
        self.read_back(settings.transparent)
    }

    /// One clean frame, at one instant.
    fn render_at(
        &mut self,
        scene: &Scene,
        frame: u32,
        settings: &ExportSettings,
        options: &document::FrameOptions,
        subframe: f64,
    ) -> Result<Frame> {
        let options = document::FrameOptions {
            subframe,
            ..options.clone()
        };
        self.draw_into_target(scene, frame, settings, &options)?;

        // The finished picture carries the document's look. Gated on `lit`,
        // which is the film's own flag — the working-view and comparison passes
        // render `lit: false` and must stay the raw artwork. The compositor is
        // the *same* `Compositor::run` the window calls, so the film matches the
        // stage frame for frame.
        let post = scene.stage().post;
        if options.lit && !post.is_identity() {
            self.apply_post(frame, &post, settings.width, settings.height);
        }

        self.read_back(settings.transparent)
    }

    /// Draw one instant of one frame into the render target, and stop there.
    fn draw_into_target(
        &mut self,
        scene: &Scene,
        frame: u32,
        settings: &ExportSettings,
        options: &document::FrameOptions,
    ) -> Result<()> {
        let limit = self.max_dimension();
        if settings.width == 0 || settings.height == 0 {
            bail!("an exported image needs a width and a height");
        }
        if settings.width > limit || settings.height > limit {
            // Refused rather than silently truncated. Tiling across several
            // passes is the real fix and is not built yet, so the message says
            // what the machine can actually do.
            bail!(
                "{} x {} is larger than this GPU can render in one pass ({limit} x {limit}); \
                 export smaller, or in pieces",
                settings.width,
                settings.height
            );
        }

        self.ensure_target(settings.width, settings.height)?;

        // The camera frames the stage exactly: the stage centre in the middle
        // of the image, and one document unit to `zoom` pixels. A render region
        // frames that rectangle instead, scaled to fill the output.
        let stage = scene.stage().stage_rect();
        let (center, zoom) = match settings.region {
            Some(r) => (r.center(), settings.width as f64 / r.width().max(1e-6)),
            None => (stage.center(), settings.zoom(scene)),
        };
        let camera = Camera::new(
            center,
            zoom,
            Size::new(settings.width as f64, settings.height as f64),
        );

        // **Cull to the output.** Before this the exporter passed `cull: None`
        // and encoded the document however far it ran past the frame, which is
        // fine for a small render but for a large import means paying to
        // encode artwork nobody will ever see — the same cost `stage.rs` no
        // longer pays. The rectangle is the one the camera above frames: the
        // render region if there is one, the stage otherwise, in the camera's
        // shot space (the same space the stage's own cull uses). Grown by 2%
        // on each side, because a stroke or a seam seal can poke a hair past
        // an object's own fill bounds. Only filled in when the caller left
        // `cull` unset — a caller with its own opinion (the working-view pass,
        // a comparison test) keeps it. Gated on `self.cull` too: with it off,
        // `None` means what it always did, "render everything, undisturbed" —
        // `culling_to_the_viewport_matches_no_cull` is the test that needs
        // that honoured exactly.
        let mut options = *options;
        if self.cull && options.cull.is_none() {
            let region = settings.region.unwrap_or(stage);
            options.cull = Some(region.inflate(region.width() * 0.02, region.height() * 0.02));
        }
        let options = &options;

        let mut vello_scene = buzz_render::vello::Scene::new();
        // Build, then let the cache judge what was built. A frame too big for
        // the rasteriser is not drawn at all — and an export has no next frame
        // to correct itself on, so it is encoded again with less of the light
        // rather than written out as whatever was in the target before. See
        // `buzz_render::document::DrawCache::reconsider`.
        for _ in 0..4 {
            let (segments, lines) = {
                let mut builder = SceneBuilder::new(&mut vello_scene, &camera);
                if !settings.transparent {
                    builder.fill_shape(&stage, scene.stage().background);
                }
                document::draw_frame_cached(
                    &mut builder,
                    scene,
                    frame,
                    // The camera moves within the frame too, so it is asked
                    // for the same instant the artwork is.
                    scene.camera_transform(options.at(frame)),
                    options,
                    &mut self.lights,
                );
                (builder.encoded_segments(), builder.estimated_lines())
            };
            // Noted before `reconsider` judges the pass, so the ladder can
            // give up modelling, shadows and finally the seam seal for a
            // frame whose *lines* overrun the GPU's buffer well under the
            // segment ceiling — see Phase 4c.
            self.lights.note_lines(lines);
            if std::env::var("BUZZ_DEBUG_LAYER_CACHE").is_ok() {
                eprintln!(
                    "draw_into_target: segments={segments} lines={lines} detail_before={:?}",
                    self.lights.detail()
                );
            }
            if !self
                .lights
                .reconsider(segments, (settings.width as f64) * (settings.height as f64))
            {
                if std::env::var("BUZZ_DEBUG_LAYER_CACHE").is_ok() {
                    eprintln!("draw_into_target: settled detail={:?}", self.lights.detail());
                }
                break;
            }
        }

        let background = if settings.transparent {
            Color::TRANSPARENT
        } else {
            // Outside the stage rectangle is pasteboard, which is not part of
            // the artwork; it takes the stage colour so an export is never
            // fringed by the editor's grey.
            scene.stage().background
        };

        // **The segment ceiling is not a safe trigger by itself** (Phase 4,
        // "Status after Phases 1-3"): Vello's line buffer can overflow, and
        // silently drop the whole frame, well under it on curvy, heavily
        // stroked artwork. The ladder above already gave up everything it
        // automatically can, down to `Flat`; a document still over either
        // budget there is one whose *artwork* does not fit a single sealed
        // pass at any lighting level, and the only way left to guarantee it
        // draws is to draw less of it at a time — not to trade away the seam
        // seal for a little more headroom. See `Self::render_bands` and
        // `docs/RENDER_SEAMS_PLAN.md` Step 1.
        if self.lights.over_budget() {
            self.render_bands(scene, frame, settings, options, &camera, stage, background)?;
        } else {
            let target = self.target.as_ref().expect("target just ensured");
            self.gpu
                .render(
                    &vello_scene,
                    &target.view,
                    settings.width,
                    settings.height,
                    background,
                )
                .context("rendering the frame")?;
        }

        Ok(())
    }

    /// Draw `frame` in horizontal strips, each culled to its own rows and
    /// rendered as its own Vello pass, so a frame the ladder still calls
    /// over budget at `Flat` never has to fit one GPU pass all at once — see
    /// Phase 4d in `docs/FLA_IMPORT_BLANK_AND_LAG_PLAN.md`.
    ///
    /// The level `draw_into_target`'s ladder settled on is **held**: a band
    /// encodes at that level without reconsidering, because reconsidering a
    /// fraction of the artwork against the whole frame's budget would answer
    /// a different question than the one already answered.
    fn render_bands(
        &mut self,
        scene: &Scene,
        frame: u32,
        settings: &ExportSettings,
        options: &document::FrameOptions,
        camera: &Camera,
        stage: Rect,
        background: Color,
    ) -> Result<()> {
        let mut n = self.lights.bands_needed();
        loop {
            let row_bytes = settings.width as usize * 4;
            let mut buffer = vec![0u8; row_bytes * settings.height as usize];
            let mut any_band_over = false;

            for i in 0..n {
                let y0 = (u64::from(settings.height) * u64::from(i) / u64::from(n)) as u32;
                let y1 = (u64::from(settings.height) * u64::from(i + 1) / u64::from(n)) as u32;
                if y1 <= y0 {
                    continue;
                }

                // The document-space rect the export camera frames those
                // rows to, grown by 2% for the same reason the whole-frame
                // cull is: a stroke or a seam seal can poke a hair past an
                // object's own fill bounds.
                let top_left = camera.screen_to_doc(Point::new(0.0, f64::from(y0)));
                let bottom_right =
                    camera.screen_to_doc(Point::new(f64::from(settings.width), f64::from(y1)));
                let band = Rect::new(
                    top_left.x.min(bottom_right.x),
                    top_left.y.min(bottom_right.y),
                    top_left.x.max(bottom_right.x),
                    top_left.y.max(bottom_right.y),
                );
                let band = band.inflate(band.width() * 0.02, band.height() * 0.02);

                let mut band_options = *options;
                band_options.cull = Some(band);
                // `stamp_view` stays `None` (inherited from `*options`), so
                // `try_stamp_symbol`'s off-screen rule falls back to `cull`
                // above — this band's own strip. Setting it to the *frame's*
                // whole cull instead was tried and reverted: a symbol that
                // then passed the rule was appended whole, unclipped to any
                // one band, into every band it touched, so more bands stopped
                // shrinking the largest band's own lines. See H0 item 1 in
                // `docs/IMPORT_PERF_PLAN.md` and
                // `buzz_render::document::FrameOptions::stamp_view`.

                let mut band_scene = buzz_render::vello::Scene::new();
                let lines = {
                    let mut builder = SceneBuilder::new(&mut band_scene, camera);
                    if !settings.transparent {
                        builder.fill_shape(&stage, scene.stage().background);
                    }
                    document::draw_frame_cached(
                        &mut builder,
                        scene,
                        frame,
                        scene.camera_transform(options.at(frame)),
                        &band_options,
                        &mut self.lights,
                    );
                    builder.estimated_lines()
                };
                // V2 (`docs/VRAM_ADAPTIVE_BUFFERS_PLAN.md`): this cache's own
                // scale, not the thread-local `vello_encoding` reads.
                if lines > self.lights.line_budget() {
                    any_band_over = true;
                }

                let target = self.target.as_ref().expect("target just ensured");
                self.gpu
                    .render(
                        &band_scene,
                        &target.view,
                        settings.width,
                        settings.height,
                        background,
                    )
                    .context("rendering a band")?;
                let sample = self.read_raw()?;
                for y in y0..y1 {
                    let start = y as usize * row_bytes;
                    buffer[start..start + row_bytes]
                        .copy_from_slice(&sample.pixels[start..start + row_bytes]);
                }
            }

            if any_band_over && n < 64 {
                n = (n * 2).min(64);
                continue;
            }

            // Put the stitched picture back into the target exactly as
            // `render_blurred` does, so the compositor and read-back that
            // follow see it as an ordinary finished frame.
            self.upload(&buffer, settings.width, settings.height)?;
            return Ok(());
        }
    }

    /// Make sure the render target matches the requested size.
    fn ensure_target(&mut self, width: u32, height: u32) -> Result<()> {
        if self
            .target
            .as_ref()
            .is_some_and(|t| t.width == width && t.height == height)
        {
            return Ok(());
        }

        let padded_row = width * 4;
        let padded_row = padded_row.div_ceil(COPY_ALIGNMENT) * COPY_ALIGNMENT;

        let texture = self.gpu.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("export-target"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: RENDER_FORMAT,
            usage: wgpu::TextureUsages::STORAGE_BINDING
                | wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_SRC
                // The compositor's result is copied back into this texture so
                // read-back stays unchanged, which needs it as a copy target.
                | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let readback = self.gpu.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("export-readback"),
            size: (padded_row as u64) * (height as u64),
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });

        self.target = Some(Target {
            width,
            height,
            texture,
            view,
            readback,
            padded_row,
        });
        Ok(())
    }

    /// Run the compositor over the just-rendered frame and copy its result back
    /// into the main target, so read-back is unchanged.
    fn apply_post(&mut self, frame: u32, post: &buzz_scene::PostSettings, width: u32, height: u32) {
        // A matching second target for the compositor's output.
        if !matches!(&self.post_target, Some(p) if p.width == width && p.height == height) {
            let texture = self.gpu.device.create_texture(&wgpu::TextureDescriptor {
                label: Some("export-post-target"),
                size: wgpu::Extent3d {
                    width,
                    height,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: RENDER_FORMAT,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
                view_formats: &[],
            });
            let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
            self.post_target = Some(PostTarget {
                width,
                height,
                texture,
                view,
            });
        }
        if self.compositor.is_none() {
            self.compositor = Some(buzz_render::Compositor::new(
                &self.gpu.device,
                RENDER_FORMAT,
            ));
        }

        let target = self.target.as_ref().expect("a target has been made");
        let post_target = self.post_target.as_ref().expect("just ensured");
        let compositor = self.compositor.as_mut().expect("just ensured");

        let mut encoder = self
            .gpu
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("export-compositor"),
            });
        compositor.run(
            &self.gpu.device,
            &self.gpu.queue,
            &mut encoder,
            &target.view,
            &post_target.view,
            width,
            height,
            post,
            frame,
        );
        // Copy the composited image back into the main target so read-back and
        // everything downstream sees the finished frame.
        encoder.copy_texture_to_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &post_target.texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyTextureInfo {
                texture: &target.texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
        );
        self.gpu.queue.submit([encoder.finish()]);
    }

    /// Copy the rendered texture back to the CPU and strip the row padding.
    fn read_back(&mut self, transparent: bool) -> Result<Frame> {
        let mut frame = self.read_raw()?;
        if transparent {
            unpremultiply(&mut frame.pixels);
        }
        Ok(frame)
    }

    /// The target's pixels exactly as the GPU left them — premultiplied, and
    /// gamma-encoded. What the shutter adds up, and what [`Self::read_back`]
    /// then straightens the alpha of.
    fn read_raw(&mut self) -> Result<Frame> {
        let target = self.target.as_ref().expect("a target has been made");
        let (width, height, padded_row) = (target.width, target.height, target.padded_row);

        let mut encoder = self
            .gpu
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("export-readback"),
            });
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: &target.texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &target.readback,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(padded_row),
                    rows_per_image: Some(height),
                },
            },
            wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
        );
        self.gpu.queue.submit([encoder.finish()]);

        // **The map's own result decides whether the bytes are read** (F7.5,
        // `docs/LOCAL_AI_PLAN.md`). This used to log a failed map and call
        // `get_mapped_range` anyway, which panics — "Buffer with
        // 'export-readback' label has been destroyed" was what one exporter
        // reused across very different sizes and scenes eventually hit. A
        // failed or unfinished map is now an `Err`, and the target is dropped
        // so the next frame builds a fresh one instead of inheriting a
        // buffer left mid-map.
        let readback = target.readback.clone();
        let slice = readback.slice(..);
        let (tx, rx) = std::sync::mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |r| {
            let _ = tx.send(r);
        });
        let polled = self.gpu.device.poll(wgpu::PollType::Wait {
            submission_index: None,
            timeout: Some(std::time::Duration::from_secs(60)),
        });
        let mapped_ok = match (polled, rx.try_recv()) {
            (Ok(_), Ok(Ok(()))) => Ok(()),
            (Err(e), _) => Err(format!("waiting for the GPU to finish the frame: {e}")),
            (_, Ok(Err(e))) => Err(format!("export read-back failed: {e}")),
            (_, Err(_)) => Err("export read-back did not complete".to_string()),
        };
        if let Err(msg) = mapped_ok {
            tracing::error!("{msg}");
            self.target = None;
            bail!(msg);
        }

        let mut pixels = Vec::with_capacity((width * height * 4) as usize);
        {
            let mapped = slice.get_mapped_range();
            let row_bytes = (width * 4) as usize;
            for row in 0..height as usize {
                let start = row * padded_row as usize;
                pixels.extend_from_slice(&mapped[start..start + row_bytes]);
            }
        }
        readback.unmap();

        Ok(Frame {
            width,
            height,
            pixels,
        })
    }

    /// Put a finished picture back into the render target.
    ///
    /// The target is a copy destination already — that is how the compositor's
    /// result gets back into it — so the accumulated frame can take the same
    /// road and be graded by the same pass as any other.
    fn upload(&mut self, pixels: &[u8], width: u32, height: u32) -> Result<()> {
        let target = self.target.as_ref().expect("a target has been made");
        if target.width != width || target.height != height {
            bail!("the accumulated frame is not the size of the target");
        }
        self.gpu.queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &target.texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            pixels,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(width * 4),
                rows_per_image: Some(height),
            },
            wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
        );
        self.gpu.queue.submit([]);
        Ok(())
    }
}

/// One gamma-encoded byte as linear light.
fn srgb_to_linear(byte: u8) -> f32 {
    let c = byte as f32 / 255.0;
    if c <= 0.040_45 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

/// Linear light back to a gamma-encoded byte.
fn linear_to_srgb(linear: f32) -> u8 {
    let c = linear.clamp(0.0, 1.0);
    let encoded = if c <= 0.003_130_8 {
        c * 12.92
    } else {
        1.055 * c.powf(1.0 / 2.4) - 0.055
    };
    (encoded * 255.0).round().clamp(0.0, 255.0) as u8
}

/// Vello composites in premultiplied alpha; PNG stores straight alpha.
///
/// Only meaningful for a transparent export — over an opaque background every
/// alpha is 255 and this divides by one.
fn unpremultiply(pixels: &mut [u8]) {
    for pixel in pixels.chunks_exact_mut(4) {
        let a = pixel[3];
        if a == 0 {
            // Fully transparent: the colour carries no information, and
            // leaving whatever the compositor left there produces coloured
            // fringes when the file is scaled by something else.
            pixel[0] = 0;
            pixel[1] = 0;
            pixel[2] = 0;
            continue;
        }
        if a == 255 {
            continue;
        }
        for channel in &mut pixel[..3] {
            *channel = ((*channel as u32 * 255 + a as u32 / 2) / a as u32).min(255) as u8;
        }
    }
}

/// Export a single image — Animate's File ▸ Export ▸ Export Image.
///
/// Brings up its own GPU, so it is a one-line call from the application. Use
/// [`Exporter`] directly when exporting more than one image.
pub fn export_image(
    scene: &Scene,
    frame: u32,
    path: &Path,
    settings: &ExportSettings,
    preference: &GpuPreference,
) -> Result<()> {
    let mut exporter = Exporter::new(preference)?;
    let rendered = exporter.render(scene, frame, settings)?;
    rendered.write_png(path)
}

/// What a sequence export produced.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SequenceReport {
    pub files: Vec<PathBuf>,
    /// Frames rendered, which is `files.len()` unless something was skipped.
    pub frames: u32,
}

/// How many frames are rendered before the batch is encoded and written.
///
/// The GPU renders one frame at a time; PNG encoding is pure CPU and goes
/// across every core. Batching is what lets both happen without holding the
/// whole sequence in memory — a 500-frame 1080p sequence would be 4 GB.
const BATCH: usize = 16;

/// Export a range of frames as `name0000.png`, `name0001.png`, …
///
/// `progress` is called with the number of frames finished so far, so a dialog
/// can show something moving. Returning `false` from it cancels the export;
/// what has already been written stays on disk, since half a sequence is
/// usually still worth having.
pub fn export_sequence(
    reel: &Reel<'_>,
    frames: std::ops::Range<u32>,
    directory: &Path,
    base_name: &str,
    settings: &ExportSettings,
    preference: &GpuPreference,
    mut progress: impl FnMut(u32, u32) -> bool,
) -> Result<SequenceReport> {
    if frames.is_empty() {
        bail!("there are no frames in that range to export");
    }
    if reel.is_empty() {
        bail!("there are no scenes to export");
    }

    // **The range is in film frames, not timeline frames.** A film is its
    // scenes end to end, and a scene with a looping section is longer than its
    // own timeline — the whole point of both features being that they are in
    // the finished file. So the numbering the user asked for is resolved
    // through the reel before anything is rendered, and it says which scene as
    // well as which frame. For a single scene with no loop this is exactly
    // what it always was: frame `n` is frame `n`.
    std::fs::create_dir_all(directory)
        .with_context(|| format!("creating {}", directory.display()))?;

    let mut exporter = Exporter::new(preference)?;
    let total = frames.len() as u32;
    let mut report = SequenceReport::default();

    // Numbered by their place in the film, drawn from wherever the reel says —
    // so a looped section writes several files from one timeline frame, and a
    // second scene carries on the numbering rather than restarting it.
    let numbers: Vec<u32> = frames.collect();
    for batch in numbers.chunks(BATCH) {
        let mut rendered = Vec::with_capacity(batch.len());
        for &index in batch {
            let (scene, frame) = reel
                .at_clamped(index)
                .expect("a non-empty reel always has a frame to hold on");
            rendered.push((index, exporter.render(scene, frame, settings)?));
        }

        // Encode and write across every core. This is the part CP-6.1 promised
        // would use the whole machine, and it is the part that can: rendering
        // is one GPU, encoding is `n` independent images.
        let written: Result<Vec<PathBuf>> = rendered
            .par_iter()
            .map(|(frame, image)| {
                let path = directory.join(format!("{base_name}{frame:04}.png"));
                image.write_png(&path)?;
                Ok(path)
            })
            .collect();

        report.files.extend(written?);
        report.frames = report.files.len() as u32;

        if !progress(report.frames, total) {
            break;
        }
    }

    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use buzz_geom::{Rect, Shape as _};
    use buzz_scene::{LayerKind, ShapeData};

    fn document() -> Scene {
        let mut scene = Scene::default();
        let layer = scene.add_layer("Art", LayerKind::Normal);
        scene.add_shape(
            layer,
            ShapeData::filled(
                Rect::new(0.0, 0.0, 550.0, 400.0).to_path(1e-9),
                Color::from_rgb8(0xFF, 0x00, 0x00),
            ),
        );
        scene
    }

    /// F7.5 (`docs/LOCAL_AI_PLAN.md`): one `Exporter` reused across very
    /// different output sizes — the whole stage, 256² thumbnails, a large
    /// frame, and back — must render every one at its own size with the right
    /// pixels, not panic on a destroyed read-back buffer.
    #[test]
    fn one_exporter_renders_frames_of_changing_sizes() {
        let Ok(mut exporter) = Exporter::new(&GpuPreference::Automatic) else {
            eprintln!("no GPU; skipping");
            return;
        };
        let scene = document();
        for (w, h) in [(550, 400), (256, 256), (1100, 800), (256, 256), (97, 301), (550, 400)] {
            let settings = ExportSettings { width: w, height: h, ..ExportSettings::for_stage(&scene) };
            let frame = exporter.render(&scene, 0, &settings).expect("renders");
            assert_eq!((frame.width, frame.height), (w, h));
            assert_eq!(frame.pixels.len(), (w * h * 4) as usize);
            let [r, g, b, _] = frame.pixel(w / 2, h / 2);
            assert!(r > 200 && g < 40 && b < 40, "{w}x{h}: centre is {r},{g},{b}");
        }
    }

    #[test]
    fn stage_settings_match_the_document() {
        let scene = document();
        let settings = ExportSettings::for_stage(&scene);
        assert_eq!((settings.width, settings.height), (550, 400));
        assert!(!settings.transparent, "a background is the default");

        let double = ExportSettings::scaled(&scene, 2.0);
        assert_eq!((double.width, double.height), (1100, 800));
        assert_eq!(double.zoom(&scene), 2.0);
    }

    /// The alignment case every real document hits: 550 x 4 is 2200 bytes,
    /// which is not a multiple of 256.
    #[test]
    fn a_stage_width_that_needs_row_padding_is_handled() {
        let padded = (550u32 * 4).div_ceil(COPY_ALIGNMENT) * COPY_ALIGNMENT;
        assert_eq!(padded, 2304);
        assert!(padded > 550 * 4, "this width really does need padding");
    }

    #[test]
    fn unpremultiplying_recovers_the_original_colour() {
        // Half-transparent pure red, premultiplied: 128 in the red channel.
        let mut pixels = vec![128, 0, 0, 128];
        unpremultiply(&mut pixels);
        assert_eq!(pixels[0], 255, "red should come back to full");
        assert_eq!(pixels[3], 128, "alpha is untouched");

        // Opaque pixels are left exactly alone.
        let mut opaque = vec![10, 20, 30, 255];
        unpremultiply(&mut opaque);
        assert_eq!(opaque, vec![10, 20, 30, 255]);

        // A fully transparent pixel loses its colour rather than keeping a
        // value that would fringe when scaled.
        let mut clear = vec![90, 90, 90, 0];
        unpremultiply(&mut clear);
        assert_eq!(clear, vec![0, 0, 0, 0]);
    }

    #[test]
    fn a_frame_encodes_to_a_real_png() {
        let frame = Frame {
            width: 2,
            height: 2,
            pixels: vec![
                255, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255, 255, 255, 255, 255,
            ],
        };
        let png = frame.to_png().expect("encode");
        assert_eq!(&png[..8], b"\x89PNG\r\n\x1a\n", "PNG magic");

        let decoder = png::Decoder::new(std::io::Cursor::new(&png));
        let mut reader = decoder.read_info().expect("read info");
        let mut buffer = vec![0; reader.output_buffer_size().expect("size")];
        let info = reader.next_frame(&mut buffer).expect("decode");
        assert_eq!((info.width, info.height), (2, 2));
        assert_eq!(&buffer[..info.buffer_size()], &frame.pixels[..]);
    }

    #[test]
    fn writing_leaves_no_part_file_behind() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("still.png");
        let frame = Frame {
            width: 1,
            height: 1,
            pixels: vec![1, 2, 3, 255],
        };
        frame.write_png(&path).expect("write");

        assert!(path.exists());
        assert!(
            !path.with_extension("png.part").exists(),
            "the temporary file should have been renamed away"
        );
    }

    #[test]
    fn pixels_can_be_addressed_by_coordinate() {
        let frame = Frame {
            width: 2,
            height: 2,
            pixels: vec![1, 1, 1, 255, 2, 2, 2, 255, 3, 3, 3, 255, 4, 4, 4, 255],
        };
        assert_eq!(frame.pixel(0, 0), [1, 1, 1, 255]);
        assert_eq!(frame.pixel(1, 0), [2, 2, 2, 255]);
        assert_eq!(frame.pixel(0, 1), [3, 3, 3, 255]);
    }
}
