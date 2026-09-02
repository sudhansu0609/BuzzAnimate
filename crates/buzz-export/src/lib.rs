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
use buzz_geom::{Camera, Size};
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
        let gpu = GpuContext::new_blocking(preference).context("starting the GPU for export")?;
        Ok(Self {
            gpu,
            target: None,
            lights: buzz_render::document::DrawCache::new(),
            compositor: None,
            post_target: None,
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

    /// (builds, stamps) of the symbol cache so far — for a parity test to
    /// confirm the cache actually engaged rather than passing vacuously.
    pub fn symbol_cache_stats(&self) -> (u64, u64) {
        (
            self.lights.symbol_scenes.builds,
            self.lights.symbol_scenes.stamps,
        )
    }

    /// Drop all retained render geometry. The symbol table already refuses to
    /// serve one document's symbols for another, so this is not needed for
    /// correctness; a test uses it to render each unrelated document from a
    /// clean slate.
    pub fn reset_caches(&mut self) {
        self.lights = buzz_render::document::DrawCache::new();
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

        let mut vello_scene = buzz_render::vello::Scene::new();
        // Build, then let the cache judge what was built. A frame too big for
        // the rasteriser is not drawn at all — and an export has no next frame
        // to correct itself on, so it is encoded again with less of the light
        // rather than written out as whatever was in the target before. See
        // `buzz_render::document::DrawCache::reconsider`.
        for _ in 0..3 {
            let segments = {
                let mut builder = SceneBuilder::new(&mut vello_scene, &camera);
                if !settings.transparent {
                    builder.fill_shape(&stage, scene.stage().background);
                }
                document::draw_frame_cached(
                    &mut builder,
                    scene,
                    frame,
                    scene.camera_transform(frame),
                    options,
                    &mut self.lights,
                );
                builder.encoded_segments()
            };
            if !self
                .lights
                .reconsider(segments, (settings.width as f64) * (settings.height as f64))
            {
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
    fn apply_post(
        &mut self,
        frame: u32,
        post: &buzz_scene::PostSettings,
        width: u32,
        height: u32,
    ) {
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
            self.compositor = Some(buzz_render::Compositor::new(&self.gpu.device, RENDER_FORMAT));
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

        let slice = target.readback.slice(..);
        slice.map_async(wgpu::MapMode::Read, |r| {
            if let Err(e) = r {
                tracing::error!("export readback failed: {e:?}");
            }
        });
        self.gpu
            .device
            .poll(wgpu::PollType::Wait {
                submission_index: None,
                timeout: Some(std::time::Duration::from_secs(60)),
            })
            .context("waiting for the GPU to finish the frame")?;

        let mut pixels = Vec::with_capacity((width * height * 4) as usize);
        {
            let mapped = slice.get_mapped_range();
            let row_bytes = (width * 4) as usize;
            for row in 0..height as usize {
                let start = row * padded_row as usize;
                pixels.extend_from_slice(&mapped[start..start + row_bytes]);
            }
        }
        target.readback.unmap();

        if transparent {
            unpremultiply(&mut pixels);
        }

        Ok(Frame {
            width,
            height,
            pixels,
        })
    }
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
