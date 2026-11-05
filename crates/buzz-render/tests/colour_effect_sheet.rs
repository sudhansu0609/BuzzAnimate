//! **Object colour effects on pixels** (`docs/OBJECT_COLOR_EFFECT_PLAN.md`, O1).
//! One symbol placed six times with None / Brightness -50 / +50 / Tint red 50 % /
//! Alpha 30 % / Advanced; the sheet is written to the PNG named by
//! `BUZZ_O1_SHEET` (default: temp dir). Skips without a GPU.

use buzz_geom::{Affine, Camera, Point, Shape as _, Size};
use buzz_render::document::{self, DrawCache, FrameOptions};
use buzz_render::{GpuContext, GpuPreference, SceneBuilder, vello, wgpu};
use buzz_scene::{ColorTransform, LayerKind, ObjectKind, Scene, ShapeData, SymbolKind};
use peniko::Color;
use std::sync::Arc;

const W: u32 = 768;
const H: u32 = 256;

#[test]
fn colour_effects_change_the_pixels() {
    let mut gpu = match GpuContext::new_blocking(&GpuPreference::Automatic) {
        Ok(g) => g,
        Err(e) => {
            eprintln!("skipping: no GPU ({e})");
            return;
        }
    };
    let mut scene = Scene::default();
    scene.stage_mut().background = Color::from_rgb8(0x40, 0x40, 0x40);
    scene.stage_mut().size = Size::new(W as f64, H as f64);
    let layer = scene.add_layer("Cast", LayerKind::Normal);
    let symbol = scene.add_symbol("Hero", SymbolKind::Graphic, None);
    let inner = scene
        .library()
        .get(symbol)
        .and_then(|s| s.layers.iter().next())
        .map(|l| l.id)
        .unwrap();
    let art = scene.next_object_id();
    let art = buzz_scene::Object::shape(
        art,
        ShapeData::filled(
            kurbo::Rect::new(-40.0, -40.0, 40.0, 40.0).to_path(1e-9),
            Color::from_rgb8(0x30, 0xA0, 0x50),
        ),
    );
    scene.library_mut().update(symbol, |s| {
        s.layers.update(inner, |l| {
            l.frames.set_objects(0, vec![Arc::new(art)]);
        });
    });
    let effects = [
        ColorTransform::default(),
        ColorTransform::brightness(-0.5),
        ColorTransform::brightness(0.5),
        ColorTransform::tint(Color::from_rgb8(255, 0, 0), 0.5),
        ColorTransform::alpha(0.3),
        ColorTransform { multiply: [1.0, 0.4, 1.0, 1.0], add: [0.0, 0.0, 0.3, 0.0] },
    ];
    for (i, fx) in effects.iter().enumerate() {
        let id = scene
            .add_instance_at(layer, 0, symbol, Affine::translate((64.0 + 128.0 * i as f64, 128.0)))
            .unwrap();
        scene.update_object_at(0, id, |o| {
            if let ObjectKind::Instance(inst) = &mut o.kind {
                inst.color = *fx;
            }
        });
    }

    let texture = gpu.device.create_texture(&wgpu::TextureDescriptor {
        label: None,
        size: wgpu::Extent3d { width: W, height: H, depth_or_array_layers: 1 },
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
        label: None,
        size: (W * H * 4) as u64,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let camera = Camera::new(Point::new(W as f64 / 2.0, H as f64 / 2.0), 1.0, Size::new(W as f64, H as f64));
    let mut vs = vello::Scene::new();
    let mut builder = SceneBuilder::new(&mut vs, &camera);
    let mut cache = DrawCache::new();
    cache.begin();
    document::draw_frame_within(&mut builder, &scene, 0, Affine::IDENTITY, &FrameOptions::default(), &mut cache);
    cache.end();
    gpu.render(&vs, &view, W, H, Color::from_rgb8(0x40, 0x40, 0x40)).expect("render");
    let mut enc = gpu.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
    enc.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo { texture: &texture, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
        wgpu::TexelCopyBufferInfo {
            buffer: &readback,
            layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(W * 4), rows_per_image: Some(H) },
        },
        wgpu::Extent3d { width: W, height: H, depth_or_array_layers: 1 },
    );
    gpu.queue.submit([enc.finish()]);
    let slice = readback.slice(..);
    slice.map_async(wgpu::MapMode::Read, |r| r.expect("map"));
    gpu.device
        .poll(wgpu::PollType::Wait { submission_index: None, timeout: Some(std::time::Duration::from_secs(30)) })
        .expect("poll");
    let px = slice.get_mapped_range().to_vec();
    readback.unmap();

    let at = |i: usize| {
        let o = ((128 * W as usize) + 64 + 128 * i) * 4;
        [px[o] as i32, px[o + 1] as i32, px[o + 2] as i32]
    };
    let base = at(0);
    let lum = |c: [i32; 3]| c[0] + c[1] + c[2];
    assert!(lum(at(1)) < lum(base), "brightness -50 darkens");
    assert!(lum(at(2)) > lum(base), "brightness +50 lightens");
    assert!(at(3)[0] > base[0] + 30, "red tint raises red");
    assert!(lum(at(4)) < lum(base) && lum(at(4)) > 3 * 0x40, "alpha 30 blends toward the background");
    assert!(at(5)[2] > base[2] && at(5)[1] < base[1], "advanced moves channels separately");

    let path = std::env::var("BUZZ_O1_SHEET")
        .unwrap_or_else(|_| std::env::temp_dir().join("o1_sheet.png").to_string_lossy().into_owned());
    // The render target is RGBA8.
    image::save_buffer(&path, &px, W, H, image::ColorType::Rgba8).expect("write sheet");
    eprintln!("sheet: {path}");
}
