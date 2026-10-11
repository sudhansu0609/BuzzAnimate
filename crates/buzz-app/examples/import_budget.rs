//! How much a document's frames cost to encode, and whether they fit Vello.
//!
//! cargo run --release -p buzz-app --example import_budget -- input.fla [frame ...]
//!
//! For each frame and each cull (none, the stage, the stage plus a half-width
//! of pasteboard — roughly the editor at fit zoom) this runs the same
//! build-then-reconsider loop the exporter and the stage run, and reports what
//! the frame settled at: encoded segments against [`segment_ceiling`], the
//! detail level it had to drop to, how many symbols were encoded once and
//! stamped, and the warm per-frame encode time over a second of playback.
//!
//! A frame over the ceiling is one Vello will drop: it rasterises nothing and
//! the target keeps whatever it held, which is the "blank stage" an oversized
//! import shows. See `buzz_render::document::LightDetail`.

use buzz_render::document::{self, DrawCache, FrameOptions, segment_ceiling};

/// A GPU brought up only under `--features gpu-truth`, to read back Vello's
/// own counters for calibrating [`buzz_render::SceneBuilder::estimated_lines`]
/// against what it actually used. See Phase 4b in
/// `docs/FLA_IMPORT_BLANK_AND_LAG_PLAN.md`.
#[cfg(feature = "gpu-truth")]
struct Measurer {
    gpu: buzz_render::GpuContext,
    #[allow(dead_code, reason = "kept alive so the view stays valid")]
    texture: buzz_render::wgpu::Texture,
    view: buzz_render::wgpu::TextureView,
}

#[cfg(feature = "gpu-truth")]
impl Measurer {
    fn new(width: u32, height: u32) -> anyhow::Result<Self> {
        let gpu = buzz_render::GpuContext::new_blocking(&buzz_render::GpuPreference::default())?;
        let texture = gpu.device.create_texture(&buzz_render::wgpu::TextureDescriptor {
            label: Some("import-budget-gpu-truth"),
            size: buzz_render::wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: buzz_render::wgpu::TextureDimension::D2,
            format: buzz_render::RENDER_FORMAT,
            usage: buzz_render::wgpu::TextureUsages::STORAGE_BINDING
                | buzz_render::wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let view = texture.create_view(&buzz_render::wgpu::TextureViewDescriptor::default());
        Ok(Self { gpu, texture, view })
    }
}

fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let input = args
        .next()
        .ok_or_else(|| anyhow::anyhow!("usage: import_budget <file.fla> [frame ...]"))?;
    let mut frames: Vec<u32> = args.map(|v| v.parse()).collect::<Result<_, _>>()?;
    if frames.is_empty() {
        frames.push(0);
    }

    let started = std::time::Instant::now();
    let imported = buzz_app::import::read(std::path::Path::new(&input)).map_err(anyhow::Error::msg)?;
    println!("import: {:.0} ms — {}", started.elapsed().as_secs_f64() * 1e3, imported.summary);
    let scene = imported.scene;

    // The output a 1080p stage is shown at in the editor, roughly.
    let (w, h) = (960.0, 540.0);
    let stage = scene.stage().stage_rect();
    let camera = buzz_geom::Camera::new(stage.center(), w / stage.width(), buzz_geom::Size::new(w, h));
    let ceiling = segment_ceiling(w * h);
    println!("output {w}x{h}, ceiling {ceiling} segments");

    #[cfg(feature = "gpu-truth")]
    let mut measurer = Measurer::new(w as u32, h as u32)?;

    for &frame in &frames {
        for (label, grow) in [("no cull", None), ("stage", Some(1.0)), ("stage+pasteboard", Some(1.5))] {
            // The cull is in the camera's shot space, which is the stage's.
            let cull = grow.map(|g: f64| {
                stage.inflate(stage.width() * (g - 1.0) / 2.0, stage.height() * (g - 1.0) / 2.0)
            });
            let options = FrameOptions { lit: true, guides: false, cull, ..FrameOptions::default() };
            let mut vello = buzz_render::vello::Scene::new();
            let mut cache = DrawCache::default();

            // The loop every real caller runs: build, judge on both segments
            // and lines, rebuild if the level moved. Bounded well above the
            // number of levels. Returns the estimated line count alongside
            // the segments Vello's own encoding already counts, so the
            // ladder judges the same thing a real caller's `note_lines` would
            // feed `reconsider` — see Phase 4a/4c.
            let encode = |cache: &mut DrawCache, vello: &mut buzz_render::vello::Scene, f: u32| {
                cache.begin();
                let (segments, lines) = {
                    let mut builder = buzz_render::SceneBuilder::new(vello, &camera);
                    document::draw_frame_within(
                        &mut builder,
                        &scene,
                        f,
                        scene.camera_transform(f as f64),
                        &options,
                        cache,
                    );
                    (builder.encoded_segments(), builder.estimated_lines())
                };
                cache.end();
                (segments, lines)
            };
            let mut segments = 0;
            let mut lines = 0u64;
            for _ in 0..8 {
                let (s, l) = encode(&mut cache, &mut vello, frame);
                segments = s;
                lines = l;
                cache.note_lines(l);
                if !cache.reconsider(segments, w * h) {
                    break;
                }
            }
            // The settled encoding for `frame`, measured against the GPU's
            // own counters before the warm loop below moves `vello` on to
            // other frames.
            #[cfg(feature = "gpu-truth")]
            let truth = measurer.gpu.render_measured(
                &vello,
                &measurer.view,
                w as u32,
                h as u32,
                peniko::Color::TRANSPARENT,
            )?;

            // Warm: a second of playback through the cache the loop settled.
            let played = std::time::Instant::now();
            let fps = scene.stage().frame_rate.round().max(1.0) as u32;
            for f in frame..frame + fps {
                let (s, l) = encode(&mut cache, &mut vello, f);
                cache.note_lines(l);
                cache.reconsider(s, w * h);
            }
            let per_frame = played.elapsed().as_secs_f64() * 1e3 / fps as f64;
            // Over either budget no longer means the frame will not draw:
            // the stage falls back to drawing it in bands (Phase 4d), and
            // only a frame band mode's own cap cannot rescue genuinely will
            // not. See `RENDER_FOLLOWUPS_PLAN.md` item 3.
            let status = if !cache.over_budget() {
                String::new()
            } else if cache.bands_needed() >= 16 && cache.bands_would_still_overrun() {
                ", WILL NOT DRAW".to_string()
            } else {
                format!(", BANDS {}", cache.bands_needed())
            };
            println!(
                "frame {frame:>5} {label:<17} segments {segments:>8} ({:>3.0}% of ceiling{status}) detail {:?} \
                 symbols built {} stamped {}  warm {per_frame:.1} ms/frame  lines {lines:>8}",
                100.0 * segments as f64 / ceiling as f64,
                cache.detail(),
                cache.symbol_scenes.builds,
                cache.symbol_scenes.stamps,
            );
            #[cfg(feature = "gpu-truth")]
            {
                let ratio = if truth.lines > 0 {
                    lines as f64 / truth.lines as f64
                } else {
                    f64::NAN
                };
                println!(
                    "  lines est {lines} / actual {} (ratio {ratio:.2}){}",
                    truth.lines,
                    if truth.overflowed { " OVERFLOWED" } else { "" },
                );
            }
        }
    }
    Ok(())
}
