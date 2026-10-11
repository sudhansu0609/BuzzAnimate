//! **The stage shows what the export writes** — item B of
//! `docs/BARISH1_REGRESSION_PLAN.md`.
//!
//! The user saw barish1's night trees vanish *in the app*, while the export
//! path (`render_import`) drew them. The stage adds what the export does not:
//! `default_cull` (`stage.rs`) around the visible rectangle, the stage's own
//! lighting ladder, and the symbol stamps. So the stage is built exactly as
//! `App::render` builds it — `stage::build_scene` over a headless [`Editor`]
//! fit to the stage — rendered on the GPU, read back, and compared with the
//! exporter's picture of the same frame at the same size: the top 30 % of the
//! frame, where the trees stand, must match within 5 % of its pixels.
//!
//! Needs the user's own `barish1.fla` and a GPU; skips cleanly without
//! either.

use std::time::Duration;

use buzz_app::editor::Editor;
use buzz_app::stage;
use buzz_doc::Document;
use buzz_geom::{Rect, Size};
use buzz_render::document::DrawCache;
use buzz_render::{GpuContext, GpuPreference, wgpu};
use peniko::Color;

const FLA: &str = r"B:\youtubeProjects\Khayal3Baje\stories\oldStories\baarish\animate\barish1.fla";
const W: u32 = 1600;
const H: u32 = 900;
const PASTEBOARD: Color = Color::from_rgb8(0x2A, 0x2A, 0x2E);

/// Render `scene` into a fresh texture and read it back as RGBA8 rows.
fn render_stage(ctx: &mut GpuContext, scene: &vello::Scene) -> Vec<u8> {
    let texture = ctx.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("stage-parity"),
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
    ctx.render(scene, &view, W, H, PASTEBOARD).expect("stage render");
    let readback = ctx.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("stage-parity-readback"),
        size: u64::from(W * H * 4),
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder = ctx
        .device
        .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
    encoder.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture: &texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyBufferInfo {
            buffer: &readback,
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
    ctx.queue.submit([encoder.finish()]);
    let slice = readback.slice(..);
    slice.map_async(wgpu::MapMode::Read, |r| r.expect("map readback"));
    ctx.device
        .poll(wgpu::PollType::Wait {
            submission_index: None,
            timeout: Some(Duration::from_secs(30)),
        })
        .expect("poll device");
    let pixels = slice.get_mapped_range().to_vec();
    readback.unmap();
    pixels
}

#[test]
fn barish1_stage_draws_the_trees_the_export_draws() {
    if !std::path::Path::new(FLA).exists() {
        eprintln!("skipping: {FLA} is not on this machine");
        return;
    }
    let mut ctx = match GpuContext::new_blocking(&GpuPreference::Automatic) {
        Ok(ctx) => ctx,
        Err(e) => {
            eprintln!("skipping: no usable GPU ({e})");
            return;
        }
    };
    // V2 (`docs/VRAM_ADAPTIVE_BUFFERS_PLAN.md`): the window now starts at
    // `buffer_scale` 1 and grows on demand, but this harness renders the
    // "stage" side with its own raw `render_stage` below rather than through
    // `App`'s own grow-then-render sync — so it starts generous, exactly
    // what the window used to get immediately, to keep judging and
    // rendering against the same scale the exporter uses.
    ctx.start_generous();
    let mut exporter = match buzz_export::Exporter::new(&GpuPreference::Automatic) {
        Ok(e) => e,
        Err(e) => {
            eprintln!("skipping: no usable GPU ({e})");
            return;
        }
    };

    let imported = buzz_app::import::read(std::path::Path::new(FLA)).expect("import barish1");
    let scene = imported.scene.clone();
    let mut editor = Editor::new(Document::new(imported.scene));
    editor.camera.viewport = Size::new(f64::from(W), f64::from(H));
    let stage_rect = editor.scene().stage().stage_rect();
    // Fit, with no margin: the 16:9 stage fills the 16:9 viewport exactly, so
    // the stage's pixels and the export's line up one to one.
    editor.camera.fit_to_rect(stage_rect, 1.0);
    let area = Rect::new(0.0, 0.0, f64::from(W), f64::from(H));
    let settings = buzz_export::ExportSettings::scaled(&scene, f64::from(W) / stage_rect.width());
    assert_eq!((settings.width, settings.height), (W, H), "the export must match the stage size");

    let mut cache = DrawCache::default();
    // The budget the window renders against: the adapter's own, as
    // `App::init` sets it.
    cache.set_scale(ctx.buffer_scale);
    let rows = (H as f64 * 0.3) as u32;
    let mut report = Vec::new();
    // Fit, then zoomed 2x onto the top-left quarter of the stage, where the
    // trees are and where the cull and the stamps' "mostly off-screen" rule
    // have the most to decide.
    let quarter = Rect::new(
        stage_rect.x0,
        stage_rect.y0,
        stage_rect.x0 + stage_rect.width() / 2.0,
        stage_rect.y0 + stage_rect.height() / 2.0,
    );
    for (view, region) in [("fit", None), ("2x top-left", Some(quarter))] {
        editor.camera.fit_to_rect(region.unwrap_or(stage_rect), 1.0);
        let settings = buzz_export::ExportSettings { region, ..settings };
        for frame in [0u32, 300] {
            editor.current_frame = frame;
            let mut vello = vello::Scene::new();
            stage::build_scene(&mut vello, &editor, area, 1.0, &mut cache);
            assert!(
                !cache.over_budget(),
                "frame {frame} ({view}) went to band mode; this test compares a single pass"
            );
            let staged = render_stage(&mut ctx, &vello);
            let exported = exporter.render(&scene, frame, &settings).expect("export render");

            let (mut differing, mut total, mut sum) = (0u64, 0u64, 0u64);
            for y in 0..rows {
                for x in 0..W {
                    let i = ((y * W + x) * 4) as usize;
                    let a = &staged[i..i + 3];
                    let b = exported.pixel(x, y);
                    let diff = (0..3).map(|c| (a[c] as i32 - b[c] as i32).abs()).max().unwrap();
                    differing += u64::from(diff > 8);
                    sum += u64::from(a[0]) + u64::from(a[1]) + u64::from(a[2]);
                    total += 1;
                }
            }
            let pct = 100.0 * differing as f64 / total as f64;
            eprintln!(
                "frame {frame} ({view}): {pct:.2}% of the top 30% differs (stage mean {:.1})",
                sum as f64 / (3 * total) as f64
            );
            report.push((frame, view, pct));
        }
    }
    for (frame, view, pct) in report {
        assert!(
            pct <= 5.0,
            "frame {frame} ({view}): the stage's tree region differs from the export's in {pct:.2}% of pixels"
        );
    }
}
