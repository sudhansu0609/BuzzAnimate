//! **What the window actually shows when a light is switched on.**
//!
//! The exporter's lighting tests pass, which proves the renderer. They do not
//! prove the *window*: the stage goes through `stage::build_scene`, a retained
//! `DrawCache`, and a deferred build, and none of that is on the export path.
//! A report of "the lights do nothing" is a report about this path, so this
//! reads the pixels back off the same encoding the window presents.

use buzz_app::editor::Editor;
use buzz_doc::Document;
use buzz_geom::{Point, Rect, Shape as _, Size};
use buzz_render::document::DrawCache;
use buzz_render::{GpuContext, GpuPreference, wgpu};
use buzz_scene::{LayerKind, LightKind, Scene, ShapeData};
use peniko::Color;

const W: u32 = 512;
const H: u32 = 512;
const BACKGROUND: Color = Color::from_rgb8(0x14, 0x16, 0x1A);
const ART: Color = Color::from_rgb8(0xC0, 0xC0, 0xC0);

struct Harness {
    gpu: GpuContext,
    texture: wgpu::Texture,
    view: wgpu::TextureView,
    readback: wgpu::Buffer,
}

impl Harness {
    fn new() -> Option<Self> {
        let gpu = match GpuContext::new_blocking(&GpuPreference::Automatic) {
            Ok(g) => g,
            Err(e) => {
                eprintln!("skipping stage lighting test: no usable GPU ({e})");
                return None;
            }
        };
        let texture = gpu.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("stage-lighting-target"),
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
            label: Some("stage-lighting-readback"),
            size: (W * H * 4) as u64,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        Some(Self {
            gpu,
            texture,
            view,
            readback,
        })
    }

    /// Encode the editor's stage exactly as the window does, render it, and
    /// read the pixels back.
    fn stage(&mut self, editor: &Editor, cache: &mut DrawCache) -> Vec<u8> {
        let mut vello = vello::Scene::new();
        buzz_app::stage::build_scene(
            &mut vello,
            editor,
            Rect::new(0.0, 0.0, W as f64, H as f64),
            1.0,
            cache,
        );
        self.present(&vello)
    }

    /// Render an encoding the caller already built and read the pixels back.
    ///
    /// Separate from [`Harness::stage`] because the window does **not** rebuild
    /// its encoding every frame: it keeps last frame's and re-renders it when
    /// nothing that shaped it changed. Reusing a stale encoding is precisely
    /// how a lighting change fails to reach the screen, so a test of that has to
    /// present the retained scene rather than a fresh one. See [`WindowSim`].
    /// [`Harness::stage`], through a stage rectangle of the caller's choosing.
    fn stage_through(&mut self, editor: &Editor, cache: &mut DrawCache, area: Rect) -> Vec<u8> {
        let mut vello = vello::Scene::new();
        buzz_app::stage::build_scene(&mut vello, editor, area, 1.0, cache);
        self.present(&vello)
    }

    fn present(&mut self, vello: &vello::Scene) -> Vec<u8> {
        self.gpu
            .render(vello, &self.view, W, H, BACKGROUND)
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

fn document() -> Editor {
    let mut scene = Scene::default();
    scene.stage_mut().background = Color::WHITE;
    scene.stage_mut().size = Size::new(550.0, 400.0);
    let layer = scene.add_layer("Art", LayerKind::Normal);
    scene.add_shape(
        layer,
        ShapeData::filled(Rect::new(200.0, 150.0, 350.0, 300.0).to_path(1e-9), ART),
    );
    let mut editor = Editor::new(Document::new(scene));
    editor.camera.viewport = Size::new(W as f64, H as f64);
    editor.camera.center = Point::new(275.0, 200.0);
    editor.camera.zoom = 0.8;
    editor
}

fn difference(a: &[u8], b: &[u8]) -> f64 {
    let mut moved = 0usize;
    for (p, q) in a.chunks_exact(4).zip(b.chunks_exact(4)) {
        let d = (p[0] as i32 - q[0] as i32).abs()
            + (p[1] as i32 - q[1] as i32).abs()
            + (p[2] as i32 - q[2] as i32).abs();
        if d > 8 {
            moved += 1;
        }
    }
    moved as f64 / (a.len() / 4).max(1) as f64
}

/// Warmth of the artwork: how far red runs ahead of blue.
fn warmth(pixels: &[u8]) -> f64 {
    let mut sum = 0.0;
    let mut n = 0usize;
    for px in pixels.chunks_exact(4) {
        // Only artwork, not the white stage or the dark pasteboard.
        let luma = 0.2126 * px[0] as f64 + 0.7152 * px[1] as f64 + 0.0722 * px[2] as f64;
        if (40.0..235.0).contains(&luma) {
            sum += px[0] as f64 - px[2] as f64;
            n += 1;
        }
    }
    if n == 0 { 0.0 } else { sum / n as f64 }
}

/// The report: switch a sun on and the stage must change.
#[test]
fn a_sun_changes_the_stage() {
    let Some(mut h) = Harness::new() else { return };
    let mut cache = DrawCache::default();
    let mut editor = document();

    let unlit = h.stage(&editor, &mut cache);
    editor.add_light(LightKind::sun());
    let lit = h.stage(&editor, &mut cache);

    let moved = difference(&unlit, &lit);
    assert!(
        moved > 0.01,
        "switching a sun on changed {:.3}% of the stage",
        moved * 100.0
    );
}

/// The report: change the light's colour and the artwork must change colour.
#[test]
fn the_lights_colour_reaches_the_stage() {
    let Some(mut h) = Harness::new() else { return };
    let mut cache = DrawCache::default();
    let mut editor = document();
    editor.add_light(LightKind::sun());

    let set = |editor: &mut Editor, c: Color| {
        editor.doc.edit("Light Colour", |scene| {
            let id = scene.lights().lights[0].id;
            scene.lights_mut().get_mut(id).expect("the sun").color = c;
        });
    };

    set(&mut editor, Color::from_rgb8(0xFF, 0x60, 0x20));
    let warm = h.stage(&editor, &mut cache);
    set(&mut editor, Color::from_rgb8(0x20, 0x60, 0xFF));
    let cold = h.stage(&editor, &mut cache);

    assert!(
        warmth(&warm) > warmth(&cold) + 8.0,
        "a warm sun ({:.1}) should be warmer than a cold one ({:.1})",
        warmth(&warm),
        warmth(&cold)
    );
}

/// The report: a lamp's height must change what is seen.
#[test]
fn a_lamps_height_changes_the_stage() {
    let Some(mut h) = Harness::new() else { return };
    let mut cache = DrawCache::default();
    let mut editor = document();
    editor.add_light(LightKind::Lamp {
        position: Point::new(120.0, 80.0),
        height: 60.0,
        radius: 320.0,
    });

    let set_height = |editor: &mut Editor, height: f64| {
        editor.doc.edit("Lamp Height", |scene| {
            let id = scene.lights().lights[0].id;
            let light = scene.lights_mut().get_mut(id).expect("the lamp");
            if let LightKind::Lamp {
                position, radius, ..
            } = light.kind
            {
                light.kind = LightKind::Lamp {
                    position,
                    height,
                    radius,
                };
            }
        });
    };

    let low = h.stage(&editor, &mut cache);
    set_height(&mut editor, 400.0);
    let high = h.stage(&editor, &mut cache);

    let moved = difference(&low, &high);
    assert!(
        moved > 0.005,
        "raising the lamp changed {:.3}% of the stage",
        moved * 100.0
    );
}

/// Not an assertion: dump what the stage looks like, so the picture can be
/// judged by eye rather than by a threshold.
#[test]
#[ignore = "diagnostic"]
fn dump_stage_pictures() {
    let Some(mut h) = Harness::new() else { return };
    let out = std::path::Path::new(&std::env::var("BUZZ_DUMP").unwrap_or_default()).to_path_buf();
    if out.as_os_str().is_empty() {
        return;
    }
    std::fs::create_dir_all(&out).expect("dump dir");

    let write = |name: &str, pixels: &[u8]| {
        let file = std::fs::File::create(out.join(name)).expect("create");
        let mut enc = png::Encoder::new(std::io::BufWriter::new(file), W, H);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        enc.write_header()
            .expect("header")
            .write_image_data(pixels)
            .expect("data");
    };

    let mut cache = DrawCache::default();
    let mut editor = document();
    write("00-unlit.png", &h.stage(&editor, &mut cache));

    editor.add_light(LightKind::sun());
    write("01-sun.png", &h.stage(&editor, &mut cache));

    editor.doc.edit("Warm", |scene| {
        let id = scene.lights().lights[0].id;
        scene.lights_mut().get_mut(id).expect("sun").color = Color::from_rgb8(0xFF, 0x60, 0x20);
    });
    write("02-sun-warm.png", &h.stage(&editor, &mut cache));

    let mut editor = document();
    editor.add_light(LightKind::Lamp {
        position: Point::new(160.0, 120.0),
        height: 80.0,
        radius: 320.0,
    });
    write("03-lamp.png", &h.stage(&editor, &mut cache));

    let mut cache = DrawCache::default();
    let mut editor = bitmap_document();
    editor.add_light(LightKind::sun());
    write("04-bitmap-sun.png", &h.stage(&editor, &mut cache));
    editor.doc.edit("Warm", |scene| {
        let id = scene.lights().lights[0].id;
        scene.lights_mut().get_mut(id).expect("sun").color = Color::from_rgb8(0xFF, 0x60, 0x20);
    });
    write("05-bitmap-warm.png", &h.stage(&editor, &mut cache));

    let mut cache = DrawCache::default();
    let mut editor = cutout_document();
    editor.add_light(LightKind::sun());
    write("06-cutout-sun.png", &h.stage(&editor, &mut cache));
}

// -- bitmap artwork ---------------------------------------------------------

/// A stage whose artwork is a **bitmap**, which is what an imported drawing,
/// a photograph and anything through Break Apart all are.
fn bitmap_document() -> Editor {
    use buzz_scene::{FillSpec, ImageAsset, ImageFill, ImageId};
    use std::sync::Arc;

    let area = Rect::new(200.0, 150.0, 350.0, 300.0);
    // **Opaque.** `vec![0xC0; n]` would set the alpha to 0xC0 as well, and a
    // three-quarters transparent grey over a white stage is not the colour this
    // is meant to compare against.
    let pixels: Vec<u8> = std::iter::repeat([0xC0u8, 0xC0, 0xC0, 0xFF])
        .take(8 * 8)
        .flatten()
        .collect();
    let asset = Arc::new(ImageAsset::from_pixels(
        ImageId(1),
        "Grey",
        8,
        8,
        Arc::new(pixels),
    ));

    let mut scene = Scene::default();
    scene.stage_mut().background = Color::WHITE;
    scene.stage_mut().size = Size::new(550.0, 400.0);
    let layer = scene.add_layer("Art", LayerKind::Normal);
    scene.add_shape(
        layer,
        ShapeData {
            path: area.to_path(1e-9),
            fill: Some(FillSpec::image(ImageFill::new(asset, area))),
            stroke: None,
            blend: buzz_scene::PaintBlend::Normal,
        },
    );

    let mut editor = Editor::new(Document::new(scene));
    editor.camera.viewport = Size::new(W as f64, H as f64);
    editor.camera.center = Point::new(275.0, 200.0);
    editor.camera.zoom = 0.8;
    editor
}

/// **The report, on bitmap artwork.** A sun must light a photograph as it
/// lights a drawn shape.
#[test]
fn a_sun_lights_bitmap_artwork() {
    let Some(mut h) = Harness::new() else { return };
    let mut cache = DrawCache::default();
    let mut editor = bitmap_document();

    let unlit = h.stage(&editor, &mut cache);
    editor.add_light(LightKind::sun());
    let lit = h.stage(&editor, &mut cache);

    let moved = difference(&unlit, &lit);
    assert!(
        moved > 0.01,
        "switching a sun on changed {:.3}% of a bitmap stage",
        moved * 100.0
    );
}

/// **The report, on bitmap artwork.** The light's colour must reach it.
#[test]
fn the_lights_colour_reaches_bitmap_artwork() {
    let Some(mut h) = Harness::new() else { return };
    let mut cache = DrawCache::default();
    let mut editor = bitmap_document();
    editor.add_light(LightKind::sun());

    let set = |editor: &mut Editor, c: Color| {
        editor.doc.edit("Light Colour", |scene| {
            let id = scene.lights().lights[0].id;
            scene.lights_mut().get_mut(id).expect("the sun").color = c;
        });
    };

    set(&mut editor, Color::from_rgb8(0xFF, 0x60, 0x20));
    let warm = h.stage(&editor, &mut cache);
    set(&mut editor, Color::from_rgb8(0x20, 0x60, 0xFF));
    let cold = h.stage(&editor, &mut cache);

    assert!(
        warmth(&warm) > warmth(&cold) + 8.0,
        "a warm sun ({:.1}) should be warmer than a cold one ({:.1}) on a bitmap",
        warmth(&warm),
        warmth(&cold)
    );
}

// -- artwork inside a symbol -------------------------------------------------

/// The same square, but held in a library symbol and placed as an instance —
/// which is what a character is.
fn symbol_document() -> Editor {
    use buzz_scene::{Object, ObjectId, SymbolKind};
    use std::sync::Arc;

    let mut scene = Scene::default();
    scene.stage_mut().background = Color::WHITE;
    scene.stage_mut().size = Size::new(550.0, 400.0);

    let symbol = scene.add_symbol("Character", SymbolKind::Graphic, None);
    let inner = scene
        .library()
        .get(symbol)
        .expect("the symbol")
        .layers
        .iter()
        .next()
        .expect("a layer")
        .id;
    scene.library_mut().update(symbol, |s| {
        s.layers.update(inner, |l| {
            l.frames.set_objects(
                0,
                vec![Arc::new(Object::shape(
                    ObjectId(9001),
                    ShapeData::filled(Rect::new(200.0, 150.0, 350.0, 300.0).to_path(1e-9), ART),
                ))],
            );
        });
    });

    let layer = scene.add_layer("Art", LayerKind::Normal);
    scene.add_instance_at(layer, 0, symbol, buzz_geom::Affine::IDENTITY);

    let mut editor = Editor::new(Document::new(scene));
    editor.camera.viewport = Size::new(W as f64, H as f64);
    editor.camera.center = Point::new(275.0, 200.0);
    editor.camera.zoom = 0.8;
    editor
}

/// **The report, on symbol artwork.** A character is a symbol, so if the light
/// stopped at the instance boundary nothing an animator lights would light.
#[test]
fn the_lights_colour_reaches_a_symbol() {
    let Some(mut h) = Harness::new() else { return };
    let mut cache = DrawCache::default();
    let mut editor = symbol_document();
    editor.add_light(LightKind::sun());

    let set = |editor: &mut Editor, c: Color| {
        editor.doc.edit("Light Colour", |scene| {
            let id = scene.lights().lights[0].id;
            scene.lights_mut().get_mut(id).expect("the sun").color = c;
        });
    };

    set(&mut editor, Color::from_rgb8(0xFF, 0x60, 0x20));
    let warm = h.stage(&editor, &mut cache);
    set(&mut editor, Color::from_rgb8(0x20, 0x60, 0xFF));
    let cold = h.stage(&editor, &mut cache);

    assert!(
        warmth(&warm) > warmth(&cold) + 8.0,
        "a warm sun ({:.1}) should be warmer than a cold one ({:.1}) inside a symbol",
        warmth(&warm),
        warmth(&cold)
    );
}

/// A bitmap that is **transparent outside a disc** — a cut-out, which is what
/// an imported character actually is. Its rectangle must stay invisible.
fn cutout_document() -> Editor {
    use buzz_scene::{FillSpec, ImageAsset, ImageFill, ImageId};
    use std::sync::Arc;

    let n = 64usize;
    let mut pixels = Vec::with_capacity(n * n * 4);
    for y in 0..n {
        for x in 0..n {
            let (dx, dy) = (x as f64 - 31.5, y as f64 - 31.5);
            let inside = dx * dx + dy * dy < 28.0 * 28.0;
            pixels.extend_from_slice(if inside {
                &[0xC0, 0xC0, 0xC0, 0xFF]
            } else {
                &[0, 0, 0, 0]
            });
        }
    }
    let asset = Arc::new(ImageAsset::from_pixels(
        ImageId(1),
        "Cutout",
        n as u32,
        n as u32,
        Arc::new(pixels),
    ));

    let area = Rect::new(200.0, 150.0, 350.0, 300.0);
    let mut scene = Scene::default();
    scene.stage_mut().background = Color::WHITE;
    scene.stage_mut().size = Size::new(550.0, 400.0);
    let layer = scene.add_layer("Art", LayerKind::Normal);
    scene.add_shape(
        layer,
        ShapeData {
            path: area.to_path(1e-9),
            fill: Some(FillSpec::image(ImageFill::new(asset, area))),
            stroke: None,
            blend: buzz_scene::PaintBlend::Normal,
        },
    );

    let mut editor = Editor::new(Document::new(scene));
    editor.camera.viewport = Size::new(W as f64, H as f64);
    editor.camera.center = Point::new(275.0, 200.0);
    editor.camera.zoom = 0.8;
    editor
}

/// **A cut-out keeps its transparency under a light.**
///
/// Every pass that lays light over a picture composes `SrcAtop` so it cannot
/// paint into the hole. Without that the light fills the cut-out's own corners
/// and the character gains a coloured rectangle — the artefact that makes
/// compositing look pasted on.
#[test]
fn a_cutouts_transparent_corners_stay_transparent() {
    let Some(mut h) = Harness::new() else { return };
    let mut cache = DrawCache::default();
    let mut editor = cutout_document();

    let unlit = h.stage(&editor, &mut cache);
    editor.doc.edit("Sky", |scene| {
        scene.lights_mut().base = Color::from_rgb8(0x30, 0x30, 0x30);
    });
    editor.add_light(LightKind::sky());
    let lit = h.stage(&editor, &mut cache);

    // A corner of the image's own rectangle, well outside the disc. The stage
    // is white there and must stay white — only the cast shadow may darken it,
    // and a sky casts none.
    let at = |pixels: &[u8], x: u32, y: u32| {
        let i = ((y * W + x) * 4) as usize;
        [pixels[i], pixels[i + 1], pixels[i + 2]]
    };
    // Document (205, 155) — inside the fill rectangle, outside the disc.
    let (x, y) = (
        ((205.0 - 275.0) * 0.8 + W as f64 / 2.0) as u32,
        ((155.0 - 200.0) * 0.8 + H as f64 / 2.0) as u32,
    );
    assert_eq!(
        at(&unlit, x, y),
        at(&lit, x, y),
        "a light changed a transparent corner of a cut-out"
    );
}

/// **A photograph of a flat colour must light like that colour.**
///
/// The vector path folds the light into the paint; the bitmap path composites
/// it over the pixels. They are different arithmetic in different places, and
/// the only thing that makes them one feature rather than two is that they
/// agree. A solid grey square and a bitmap of the same grey, lit by the same
/// sun, must come out the same picture.
#[test]
fn a_bitmap_lights_like_the_colour_it_is_made_of() {
    let Some(mut h) = Harness::new() else { return };

    let mut vector = document();
    vector.add_light(LightKind::sun());
    let mut cache = DrawCache::default();
    let drawn = h.stage(&vector, &mut cache);

    let mut bitmap = bitmap_document();
    bitmap.add_light(LightKind::sun());
    let mut cache = DrawCache::default();
    let photographed = h.stage(&bitmap, &mut cache);

    let mut worst = 0i32;
    let mut off = 0usize;
    for (p, q) in drawn.chunks_exact(4).zip(photographed.chunks_exact(4)) {
        let d = (0..3)
            .map(|i| (p[i] as i32 - q[i] as i32).abs())
            .max()
            .unwrap_or(0);
        worst = worst.max(d);
        // The two paths reach the same colour by different routes — one in
        // linear light on a `Color`, one through the compositor's transfer
        // curve — so they agree closely rather than bit for bit.
        if d > 10 {
            off += 1;
        }
    }
    let fraction = off as f64 / (drawn.len() / 4) as f64;
    // The body and the shaded band agree to within a level or two. What is left
    // is the glint: it mixes in the compositor's space rather than in linear
    // light, and at the corner where the two crescents overlap it sits on the
    // shaded pixel rather than the lit one. Both are named and weighed in
    // `draw_lit_composited`; together they are well under a hundredth of the frame.
    // Before the bitmap path existed this stood at 5%, with the light making no
    // difference to the picture at all.
    assert!(
        fraction < 0.01,
        "{:.3}% of the stage differs between a lit drawing and a lit photograph \
         of it (worst channel {worst})",
        fraction * 100.0
    );
}

/// **A saved document must open lit.**
///
/// Everything above proves a light works in the session that created it. A
/// document is saved and reopened far more often than it is created, and the
/// rig travels through its own DTO layer to get there — a separate piece of
/// code with its own version gate, which no in-memory test exercises at all.
#[test]
fn lights_survive_a_save_and_a_reopen() {
    let dir = std::env::temp_dir().join("buzz-lighting-roundtrip");
    std::fs::create_dir_all(&dir).expect("temp dir");
    let path = dir.join("lit.buzz");

    let mut editor = document();
    editor.add_light(LightKind::sun());
    editor.doc.edit("Colour", |scene| {
        let id = scene.lights().lights[0].id;
        let light = scene.lights_mut().get_mut(id).expect("the sun");
        light.color = Color::from_rgb8(0xFF, 0x60, 0x20);
        light.intensity = 1.5;
    });
    let before = editor.doc.scene().lights().clone();
    editor.doc.save_as(&path).expect("save");

    let reopened = Document::open(&path).expect("open");
    let after = reopened.scene().lights();

    assert!(after.enabled, "the rig came back switched off");
    assert_eq!(after.lights.len(), 1, "the sun did not come back");
    assert_eq!(&before, after, "the rig changed on the way through the file");
}

/// **The second frame must look like the first.**
///
/// Every test above draws one frame into a fresh cache. The running window
/// draws the same document over and over into a cache that is *warm*, and a
/// cache that is not lighting-aware serves what it stored on frame one for ever
/// after. That failure is invisible to a single-frame test and is the whole of
/// what the user sees: it lights, for one frame, and then stops.
///
/// Drawn five times, because the caches engage at different ages — the symbol
/// encoding on the second sight of an instance, the crescents on the frame
/// after they are built.
#[test]
fn lighting_survives_a_warm_cache() {
    let Some(mut h) = Harness::new() else { return };

    for (what, mut editor) in [
        ("loose artwork", document()),
        ("a symbol", symbol_document()),
        ("a bitmap", bitmap_document()),
    ] {
        editor.add_light(LightKind::sun());
        let mut cache = DrawCache::default();
        let first = h.stage(&editor, &mut cache);
        for frame in 2..=5 {
            let again = h.stage(&editor, &mut cache);
            let moved = difference(&first, &again);
            assert!(
                moved < 0.001,
                "{what}: frame {frame} differs from frame 1 by {:.3}% — a cache is \
                 serving artwork that is not lit the way the first frame lit it",
                moved * 100.0
            );
        }
    }
}

/// Artwork the way an **import** leaves it: shapes inside a group, and line
/// work with no fill at all.
fn imported_document(stroked: bool) -> Editor {
    use buzz_scene::{Object, ObjectId, StrokeSpec};
    use std::sync::Arc;

    let shape = if stroked {
        // Line work: a stroke and no fill, which is most of what a traced or
        // imported drawing is made of.
        ShapeData {
            path: Rect::new(200.0, 150.0, 350.0, 300.0).to_path(1e-9),
            fill: None,
            stroke: Some(StrokeSpec::new(ART, 6.0)),
            blend: buzz_scene::PaintBlend::Normal,
        }
    } else {
        ShapeData::filled(Rect::new(200.0, 150.0, 350.0, 300.0).to_path(1e-9), ART)
    };

    let mut scene = Scene::default();
    scene.stage_mut().background = Color::WHITE;
    scene.stage_mut().size = Size::new(550.0, 400.0);
    let layer = scene.add_layer("Art", LayerKind::Normal);
    scene.add_object(
        layer,
        Object {
            kind: buzz_scene::ObjectKind::Group(vec![Arc::new(Object::shape(
                ObjectId(4242),
                shape,
            ))]),
            ..Object::shape(ObjectId(4241), ShapeData::filled(Rect::ZERO.to_path(1e-9), ART))
        },
    );

    let mut editor = Editor::new(Document::new(scene));
    editor.camera.viewport = Size::new(W as f64, H as f64);
    editor.camera.center = Point::new(275.0, 200.0);
    editor.camera.zoom = 0.8;
    editor
}

/// **Grouped artwork, and line work, must light too.**
#[test]
fn the_lights_colour_reaches_grouped_and_stroked_artwork() {
    let Some(mut h) = Harness::new() else { return };

    for stroked in [false, true] {
        let what = if stroked { "line work" } else { "a group" };
        let mut editor = imported_document(stroked);
        editor.add_light(LightKind::sun());
        let mut cache = DrawCache::default();

        let set = |editor: &mut Editor, c: Color| {
            editor.doc.edit("Light Colour", |scene| {
                let id = scene.lights().lights[0].id;
                scene.lights_mut().get_mut(id).expect("the sun").color = c;
            });
        };

        set(&mut editor, Color::from_rgb8(0xFF, 0x60, 0x20));
        let warm = h.stage(&editor, &mut cache);
        set(&mut editor, Color::from_rgb8(0x20, 0x60, 0xFF));
        let cold = h.stage(&editor, &mut cache);

        assert!(
            warmth(&warm) > warmth(&cold) + 8.0,
            "{what}: a warm sun ({:.1}) should be warmer than a cold one ({:.1})",
            warmth(&warm),
            warmth(&cold)
        );
    }
}

// -- the user's own document -------------------------------------------------

/// Not a test: import a real file, report what its artwork is made of, and say
/// whether a light changes the picture. `BUZZ_FLA=<path>`.
#[test]
#[ignore = "diagnostic"]
fn diagnose_a_real_document() {
    use buzz_scene::{Object, ObjectKind, Paint};

    let Ok(path) = std::env::var("BUZZ_FLA") else {
        return;
    };
    let imported = buzz_app::import::read(std::path::Path::new(&path)).expect("import");
    eprintln!("summary: {}", imported.summary);
    for line in &imported.unsupported {
        eprintln!("  missing: {line}");
    }

    let scene = imported.scene;
    eprintln!(
        "stage {:?}  layers {}  symbols {}",
        scene.stage().size,
        scene.layers().len(),
        scene.library().len()
    );

    // What is actually on the stage, by kind and by what fills it.
    #[derive(Default, Debug)]
    struct Tally {
        groups: usize,
        instances: usize,
        armatures: usize,
        warps: usize,
        shapes: usize,
        solid_fill: usize,
        gradient_fill: usize,
        image_fill: usize,
        no_fill: usize,
        stroked: usize,
    }
    fn walk(object: &Object, scene: &Scene, t: &mut Tally, depth: usize) {
        if depth > 6 {
            return;
        }
        match &object.kind {
            ObjectKind::Group(children) => {
                t.groups += 1;
                for c in children {
                    walk(c, scene, t, depth + 1);
                }
            }
            ObjectKind::Instance(i) => {
                t.instances += 1;
                if let Some(symbol) = scene.library().get(i.symbol) {
                    for layer in symbol.layers.iter() {
                        for o in layer.frames.resolved_at(0).iter() {
                            walk(o, scene, t, depth + 1);
                        }
                    }
                }
            }
            ObjectKind::Armature(_) => t.armatures += 1,
            ObjectKind::Warp(_) => t.warps += 1,
            ObjectKind::Shape(s) => {
                t.shapes += 1;
                match s.fill.as_ref().map(|f| &f.paint) {
                    Some(Paint::Solid(_)) => t.solid_fill += 1,
                    Some(Paint::Gradient(_)) => t.gradient_fill += 1,
                    Some(Paint::Image(_)) => t.image_fill += 1,
                    None => t.no_fill += 1,
                }
                if s.stroke.is_some() {
                    t.stroked += 1;
                }
            }
        }
    }
    let mut tally = Tally::default();
    for layer in scene.layers().iter() {
        eprintln!(
            "  layer {:?} kind {:?} visible {} outline {} depth {}",
            layer.name, layer.kind, layer.visible, layer.outline, layer.depth
        );
        for o in layer.frames.resolved_at(0).iter() {
            walk(o, &scene, &mut tally, 0);
        }
    }
    eprintln!("{tally:#?}");

    // Now: does a light change the picture?
    let Some(mut h) = Harness::new() else { return };
    let mut editor = Editor::new(Document::new(scene));
    editor.camera.viewport = Size::new(W as f64, H as f64);
    let stage = editor.scene().stage().size;
    editor.camera.center = Point::new(stage.width / 2.0, stage.height / 2.0);
    editor.camera.zoom = (W as f64 / stage.width).min(H as f64 / stage.height) * 0.9;

    let mut cache = DrawCache::default();
    let unlit = h.stage(&editor, &mut cache);
    editor.add_light(LightKind::sun());
    eprintln!(
        "rig active: {}  lights: {}",
        editor.scene().lights().is_active(),
        editor.scene().lights().lights.len()
    );
    let lit = h.stage(&editor, &mut cache);
    eprintln!(
        "a sun changed {:.3}% of the stage",
        difference(&unlit, &lit) * 100.0
    );

    // Is the harness even seeing new pixels? Make the light absurd and look
    // again; and change something that has nothing to do with lighting at all.
    editor.doc.edit("Absurd", |scene| {
        let id = scene.lights().lights[0].id;
        let light = scene.lights_mut().get_mut(id).expect("the sun");
        light.color = Color::from_rgb8(0xFF, 0x00, 0x00);
        light.intensity = 4.0;
        scene.lights_mut().base = Color::from_rgb8(0xFF, 0x00, 0x00);
    });
    let absurd = h.stage(&editor, &mut cache);
    eprintln!("an absurd red light changed {:.3}%", difference(&unlit, &absurd) * 100.0);

    editor.doc.edit("Background", |scene| {
        scene.stage_mut().background = Color::from_rgb8(0x00, 0xFF, 0x00);
    });
    let green = h.stage(&editor, &mut cache);
    eprintln!("a green stage changed {:.3}%", difference(&absurd, &green) * 100.0);

    let sum = |p: &[u8]| p.iter().map(|&b| b as u64).sum::<u64>();
    eprintln!(
        "checksums: unlit={} lit={} absurd={} green={}",
        sum(&unlit), sum(&lit), sum(&absurd), sum(&green)
    );
    // A harness built from scratch, in case the first one is holding stale
    // pixels rather than the renderer producing them.
    if let Some(mut fresh) = Harness::new() {
        let mut cold = DrawCache::default();
        let again = fresh.stage(&editor, &mut cold);
        eprintln!("fresh harness checksum={} vs green={}", sum(&again), sum(&green));
    }

    if let Ok(out) = std::env::var("BUZZ_DUMP") {
        let out = std::path::Path::new(&out);
        std::fs::create_dir_all(out).ok();
        for (name, px) in [("fla-00-unlit.png", &unlit), ("fla-01-sun.png", &lit)] {
            let file = std::fs::File::create(out.join(name)).expect("create");
            let mut enc = png::Encoder::new(std::io::BufWriter::new(file), W, H);
            enc.set_color(png::ColorType::Rgba);
            enc.set_depth(png::BitDepth::Eight);
            enc.write_header()
                .expect("header")
                .write_image_data(px)
                .expect("data");
        }
    }
}

/// Not a test: import a real file, add a sun, and save it so the actual
/// application can be launched on it. `BUZZ_FLA=<in> BUZZ_OUT=<out.buzz>`.
#[test]
#[ignore = "diagnostic"]
fn save_a_real_document_lit() {
    let (Ok(fla), Ok(out)) = (std::env::var("BUZZ_FLA"), std::env::var("BUZZ_OUT")) else {
        return;
    };
    let imported = buzz_app::import::read(std::path::Path::new(&fla)).expect("import");
    let mut editor = Editor::new(Document::new(imported.scene));
    if std::env::var("BUZZ_NO_LIGHT").is_err() {
        editor.add_light(LightKind::sun());
        if let Ok(spec) = std::env::var("BUZZ_LAMP") {
            // "x,y" in document units.
            let mut it = spec.split(',').filter_map(|v| v.trim().parse::<f64>().ok());
            let (x, y) = (it.next().unwrap_or(960.0), it.next().unwrap_or(540.0));
            editor.doc.edit("Lamp", |scene| {
                let id = scene.lights().lights[0].id;
                let light = scene.lights_mut().get_mut(id).expect("the light");
                light.kind = LightKind::Lamp {
                    position: Point::new(x, y),
                    height: 200.0,
                    radius: 700.0,
                };
                light.color = Color::from_rgb8(0xFF, 0x8A, 0x20);
                light.intensity = 3.0;
                light.glow = 0.0;
                let rig = scene.lights_mut();
                rig.base = Color::from_rgb8(0x14, 0x1E, 0x40);
            });
        }
        if std::env::var("BUZZ_STRONG").is_ok() {
            editor.doc.edit("Strong", |scene| {
                let id = scene.lights().lights[0].id;
                let light = scene.lights_mut().get_mut(id).expect("the sun");
                light.color = Color::from_rgb8(0xFF, 0x8A, 0x20);
                light.intensity = 2.2;
                let rig = scene.lights_mut();
                rig.base = Color::from_rgb8(0x18, 0x28, 0x60);
                rig.modelling = 1.0;
            });
        }
        eprintln!("rig active: {}", editor.scene().lights().is_active());
    }
    editor.doc.save_as(&out).expect("save");
    eprintln!("wrote {out}");
}

/// **Switching a sun on must not simply dim the picture.**
///
/// This is the report that started it: on a real film, adding the default sun
/// moved 77% of the stage and changed nothing anyone could see — because what
/// it did was multiply every pixel by about 0.8, equally on all three channels.
/// Every pixel "changed"; the picture looked identical.
///
/// So "did anything change" is the wrong question, and the tests above that ask
/// it would all have passed on the broken defaults. The right question is
/// whether the light does what a light does: leave what it falls on about as
/// bright as it was, and put its effect into the difference between the lit
/// side and the shaded one.
#[test]
fn a_default_sun_lights_rather_than_dims() {
    let Some(mut h) = Harness::new() else { return };
    let mut cache = DrawCache::default();
    let mut editor = document();

    let unlit = h.stage(&editor, &mut cache);
    editor.add_light(LightKind::sun());
    let lit = h.stage(&editor, &mut cache);

    // **The middle of the square**, which is plainly lit: not the glint along
    // its top edge, not the shaded band down its left. Reading the brightest
    // pixels instead measures the highlight, which stays bright however much
    // the light dims everything around it — the first version of this test did
    // exactly that, and passed on the very defaults it was written to catch.
    let patch = |px: &[u8]| -> f64 {
        let (cx, cy) = (W / 2, H / 2 + 20);
        let mut sum = 0.0;
        let mut n = 0.0;
        for y in (cy - 8)..(cy + 8) {
            for x in (cx - 8)..(cx + 8) {
                let i = ((y * W + x) * 4) as usize;
                sum +=
                    0.2126 * px[i] as f64 + 0.7152 * px[i + 1] as f64 + 0.0722 * px[i + 2] as f64;
                n += 1.0;
            }
        }
        sum / n
    };

    let (was, now) = (patch(&unlit), patch(&lit));
    assert!(
        now > was * 0.95,
        "the lit side came out at {now:.0} against {was:.0} unlit — the sun is \
         dimming the artwork rather than lighting it"
    );

    // And it must leave its colour behind, not merely scale the brightness: a
    // warm sun that lights everything by an equal factor on all three channels
    // is a dimmer switch, and reads as one.
    assert!(
        warmth(&lit) > warmth(&unlit) + 4.0,
        "the sun left no colour behind: warmth {:.1} lit against {:.1} unlit",
        warmth(&lit),
        warmth(&unlit)
    );
}


// -- what a lamp does to a face ----------------------------------------------

/// Two squares, one where the lamp will stand and one across the stage: a face
/// and the wall behind it.
fn two_faces() -> Editor {
    let mut scene = Scene::default();
    scene.stage_mut().background = Color::WHITE;
    scene.stage_mut().size = Size::new(550.0, 400.0);
    let layer = scene.add_layer("Art", LayerKind::Normal);
    for x in [60.0, 350.0] {
        scene.add_shape(
            layer,
            ShapeData::filled(
                Rect::new(x, 150.0, x + 140.0, 290.0).to_path(1e-9),
                ART,
            ),
        );
    }
    let mut editor = Editor::new(Document::new(scene));
    editor.camera.viewport = Size::new(W as f64, H as f64);
    editor.camera.center = Point::new(275.0, 210.0);
    editor.camera.zoom = 0.85;
    editor
}

/// A lamp with **everything but its illumination turned off**.
///
/// This is what makes the tests below mean anything. A lamp does four separate
/// things — it lays a pool of glow, it models a crescent, it throws a shadow,
/// and it lights the artwork — and three of those move with the lamp whether or
/// not the fourth works at all. The first version of these tests measured the
/// picture with all four live, and passed against the very defect they were
/// written for: the pool brightened the near face, the highlight carried the
/// lamp's colour, and the illumination could stay switched off unnoticed.
///
/// Glow off, modelling off, shadows off. What is left is the light landing on
/// the artwork, and nothing else.
fn bare_lamp(editor: &mut Editor, at: Point) {
    editor.add_light(LightKind::Lamp {
        position: at,
        height: 120.0,
        radius: 300.0,
    });
    editor.doc.edit("Bare lamp", |scene| {
        let id = scene.lights().lights[0].id;
        let light = scene.lights_mut().get_mut(id).expect("the lamp");
        light.glow = 0.0;
        light.shadows = false;
        scene.lights_mut().modelling = 0.0;
    });
}

fn move_lamp(editor: &mut Editor, to: Point) {
    editor.doc.edit("Lamp", |scene| {
        let id = scene.lights().lights[0].id;
        let light = scene.lights_mut().get_mut(id).expect("the lamp");
        if let LightKind::Lamp { height, radius, .. } = light.kind {
            light.kind = LightKind::Lamp {
                position: to,
                height,
                radius,
            };
        }
    });
}

/// Mean luma of a patch of screen.
fn patch_luma(px: &[u8], cx: u32, cy: u32, half: u32) -> f64 {
    let mut sum = 0.0;
    let mut n = 0.0;
    for y in (cy - half)..(cy + half) {
        for x in (cx - half)..(cx + half) {
            let i = ((y * W + x) * 4) as usize;
            sum += 0.2126 * px[i] as f64 + 0.7152 * px[i + 1] as f64 + 0.0722 * px[i + 2] as f64;
            n += 1.0;
        }
    }
    sum / n
}

/// Where a document x lands on screen, for `two_faces`.
fn at_x(doc_x: f64) -> u32 {
    ((doc_x - 275.0) * 0.85 + W as f64 / 2.0) as u32
}
const FACE_Y: u32 = ((220.0 - 210.0) * 0.85 + 256.0) as u32;

/// **Carrying a lamp up to a face must light that face.**
///
/// The lamp used to be left out of the illumination entirely — it laid a pool
/// of glow over the frame and touched the artwork not at all. Moving it across
/// the stage therefore changed nothing about the figure it was moved towards,
/// which is the one thing a lamp is for.
#[test]
fn a_lamp_brought_closer_lights_the_near_face() {
    let Some(mut h) = Harness::new() else { return };
    let mut editor = two_faces();
    bare_lamp(&mut editor, Point::new(-500.0, 220.0));

    let mut cache = DrawCache::default();
    let away = patch_luma(&h.stage(&editor, &mut cache), at_x(130.0), FACE_Y, 10);
    move_lamp(&mut editor, Point::new(20.0, 220.0));
    let close = patch_luma(&h.stage(&editor, &mut cache), at_x(130.0), FACE_Y, 10);

    assert!(
        close > away + 12.0,
        "the face read {away:.0} with the lamp across the stage and {close:.0} with \
         it alongside — a lamp carried up to someone has to light them"
    );
}

/// **And the far side must not gain with it.** A lamp that lit everything
/// equally would be a sun wearing a lamp's icon; the falloff is what makes it
/// read as a lamp.
#[test]
fn a_lamp_lights_the_near_face_more_than_the_far_one() {
    let Some(mut h) = Harness::new() else { return };
    let mut editor = two_faces();
    bare_lamp(&mut editor, Point::new(20.0, 220.0));

    let mut cache = DrawCache::default();
    let lit = h.stage(&editor, &mut cache);
    let near = patch_luma(&lit, at_x(130.0), FACE_Y, 10);
    let far = patch_luma(&lit, at_x(420.0), FACE_Y, 10);

    assert!(
        near > far + 15.0,
        "the near face came out at {near:.0} and the far one at {far:.0} — a lamp \
         has to fall off with distance or it is a sun"
    );
}

/// **A lamp's colour must land on what it lights**, and not only in its glint.
#[test]
fn a_lamps_colour_reaches_the_artwork() {
    let Some(mut h) = Harness::new() else { return };
    let mut editor = two_faces();
    bare_lamp(&mut editor, Point::new(20.0, 220.0));

    let set = |editor: &mut Editor, c: Color| {
        editor.doc.edit("Lamp Colour", |scene| {
            let id = scene.lights().lights[0].id;
            scene.lights_mut().get_mut(id).expect("the lamp").color = c;
        });
    };

    let mut cache = DrawCache::default();
    set(&mut editor, Color::from_rgb8(0xFF, 0x60, 0x20));
    let warm = h.stage(&editor, &mut cache);
    set(&mut editor, Color::from_rgb8(0x20, 0x60, 0xFF));
    let cold = h.stage(&editor, &mut cache);

    assert!(
        warmth(&warm) > warmth(&cold) + 8.0,
        "a warm lamp ({:.1}) should be warmer than a cold one ({:.1})",
        warmth(&warm),
        warmth(&cold)
    );
}

/// **A lamp added the way an animator adds one must light what is on screen.**
///
/// Not a lamp built by hand with a reach chosen to suit the test — the one
/// `Insert ▸ Lamp` makes, on a film-sized stage. Its reach used to be a fixed
/// 320 units, which crosses the 550-wide stage this was built against and is a
/// sixth of a 1920 film: added to a real document it fell off to nothing before
/// it arrived at the character.
#[test]
fn a_lamp_added_to_a_film_sized_stage_lights_it() {
    let Some(mut h) = Harness::new() else { return };

    let mut scene = Scene::default();
    scene.stage_mut().background = Color::WHITE;
    scene.stage_mut().size = Size::new(1920.0, 1080.0);
    let layer = scene.add_layer("Art", LayerKind::Normal);
    // A figure near the middle of the shot.
    scene.add_shape(
        layer,
        ShapeData::filled(Rect::new(820.0, 400.0, 1100.0, 800.0).to_path(1e-9), ART),
    );
    let mut editor = Editor::new(Document::new(scene));
    editor.camera.viewport = Size::new(W as f64, H as f64);
    editor.camera.center = Point::new(960.0, 540.0);
    editor.camera.zoom = W as f64 / 1920.0;

    let mut cache = DrawCache::default();

    // Exactly what the menu does.
    editor.add_light(buzz_scene::LightKind::lamp(Point::new(0.0, 0.0)));
    // Glow, modelling and shadows all off, for the reason spelled out on
    // `bare_lamp`: each of them moves with the lamp whether or not the light
    // itself arrives, and measuring the picture with them live is how an
    // earlier version of this test passed against the very reach it was
    // written to catch.
    editor.doc.edit("Bare lamp", |scene| {
        let id = scene.lights().lights[0].id;
        let light = scene.lights_mut().get_mut(id).expect("the lamp");
        light.glow = 0.0;
        light.shadows = false;
        scene.lights_mut().modelling = 0.0;
    });
    let lit = h.stage(&editor, &mut cache);

    // **Against the fill light alone, not against the unlit document.** Adding
    // any light drops the picture from "full daylight" to the rig's fill, and
    // that drop happens whether or not the lamp reaches anything — so
    // comparing with the unlit picture measures the fill light and calls it a
    // lamp. Turning the lamp's own strength off leaves exactly the fill, and
    // the difference between the two is the lamp and nothing else.
    editor.doc.edit("Fill only", |scene| {
        let id = scene.lights().lights[0].id;
        scene.lights_mut().get_mut(id).expect("the lamp").intensity = 0.0;
    });
    let fill_only = h.stage(&editor, &mut cache);

    // The middle of the figure.
    let (cx, cy) = (
        (W / 2) as u32,
        ((600.0 - 540.0) * (W as f64 / 1920.0) + H as f64 / 2.0) as u32,
    );
    let (bare, lamp) = (
        patch_luma(&fill_only, cx, cy, 8),
        patch_luma(&lit, cx, cy, 8),
    );
    assert!(
        lamp > bare + 12.0,
        "the figure read {bare:.0} on the fill light alone and {lamp:.0} with the lamp          — a lamp added to a 1920-wide stage has to reach what is on screen"
    );
}

// -- a lamp across one shape -------------------------------------------------
//
// Everything above measures a lamp *between* shapes: two faces either side of
// it, a figure it is carried up to. All of it passed while the defect this
// section exists for was live, because the defect was inside one shape.
//
// The light used to be evaluated once per shape, at that shape's middle, and
// the resulting colour written into the fill. One colour per shape is exact for
// a sun — parallel rays deliver the same light everywhere — and for a lamp it
// throws away the only thing that makes a lamp a lamp. A wall came out flat. A
// face had no lit side and no shaded side. Carrying the lamp nearer moved one
// number per shape and nothing within any of them.
//
// So these measure *across a single shape*, which is where an animator looks.

/// A stage filled by one big shape, with a figure standing on it: a background
/// and a character, which is what a real frame is made of and what one small
/// rectangle in the middle of a white stage is not.
fn wall(fill: Color) -> Editor {
    let mut scene = Scene::default();
    scene.stage_mut().background = Color::WHITE;
    scene.stage_mut().size = Size::new(550.0, 400.0);
    let layer = scene.add_layer("Art", LayerKind::Normal);
    scene.add_shape(
        layer,
        ShapeData::filled(Rect::new(0.0, 0.0, 550.0, 400.0).to_path(1e-9), fill),
    );
    let mut editor = Editor::new(Document::new(scene));
    editor.camera.viewport = Size::new(W as f64, H as f64);
    editor.camera.center = Point::new(275.0, 200.0);
    editor.camera.zoom = 0.8;
    editor
}

/// Put a lamp on the stage at `at`, with **everything but its illumination
/// turned off** — the discipline [`bare_lamp`] spells out, and for the same
/// reason.
///
/// A lamp does four separate things and three of them move with it whether or
/// not the fourth works: the pool of glow is a radial ramp over the frame, the
/// terminator is a band that swings round, and the cast shadow points away. Any
/// of the three will make a sample beside the lamp brighter than one across the
/// stage. Written without this, every test below passed with the light on the
/// artwork switched off entirely — which is the defect they exist for.
///
/// Glow off, modelling off, shadows off. What is left is the light landing on
/// the surface, which is what these are about.
fn lamp_on(editor: &mut Editor, at: Point) {
    if editor.doc.scene().lights().lights.is_empty() {
        editor.add_light(LightKind::Lamp {
            position: at,
            height: 120.0,
            radius: 300.0,
        });
    }
    editor.doc.edit("Lamp", |scene| {
        let id = scene.lights().lights[0].id;
        let light = scene.lights_mut().get_mut(id).expect("the lamp");
        light.kind = LightKind::Lamp {
            position: at,
            height: 120.0,
            radius: 300.0,
        };
        light.glow = 0.0;
        light.shadows = false;
        scene.lights_mut().modelling = 0.0;
    });
}

/// Where a document x lands on screen, for [`wall`].
fn wall_x(doc_x: f64) -> u32 {
    ((doc_x - 275.0) * 0.8 + 256.0) as u32
}

/// Luma sampled along the wall **on the lamp's own row**, running away from it.
///
/// Along that line the distance to the lamp only grows, so the light may only
/// fall. Sampled on any other row it would rise to the point nearest the lamp
/// first — correct, and not a straight line to measure a falloff along.
fn away_from(pixels: &[u8], from_x: f64) -> Vec<f64> {
    (0..24)
        .map(|i| patch_luma(pixels, wall_x(from_x + i as f64 * 19.0), 256, 3))
        .collect()
}

/// **A lamp to one side of a shape lights that side and darkens the other.**
///
/// The report, in one line, and the thing nothing here used to measure. It is
/// asserted *within one shape*: the tests further up put two shapes either side
/// of a lamp, which a flat per-shape tint passes.
#[test]
fn a_lamp_lights_one_side_of_a_shape_and_darkens_the_other() {
    let Some(mut h) = Harness::new() else { return };
    let mut editor = wall(Color::from_rgb8(0xE8, 0xE4, 0xDC));
    lamp_on(&mut editor, Point::new(70.0, 200.0));

    let mut cache = DrawCache::default();
    let lit = h.stage(&editor, &mut cache);
    let near = patch_luma(&lit, wall_x(40.0), 256, 6);
    let far = patch_luma(&lit, wall_x(510.0), 256, 6);

    assert!(
        near > far + 60.0,
        "one wall, one lamp at its left edge: the near side read {near:.0} and \
         the far side {far:.0}. A lamp that lights a whole shape evenly is a \
         filter, not a lamp."
    );
}

/// **And it falls off smoothly**, rather than in steps.
///
/// Two ways of failing this, and both were live. A flat tint per shape gives a
/// *level* profile — the same value from one edge to the other, with a cliff
/// only where the shading band began. And a band filled with one tone gives a
/// **step**: measured at 64 levels between two samples 40 units apart, a seam
/// straight across the picture that reads as a join rather than as shading.
#[test]
fn a_lamps_falloff_is_a_gradient_rather_than_a_cliff() {
    let Some(mut h) = Harness::new() else { return };
    let mut editor = wall(Color::from_rgb8(0xE8, 0xE4, 0xDC));
    lamp_on(&mut editor, Point::new(70.0, 200.0));

    let mut cache = DrawCache::default();
    let profile = away_from(&h.stage(&editor, &mut cache), 78.0);

    // It only ever gets darker going away from the lamp.
    for pair in profile.windows(2) {
        assert!(
            pair[1] <= pair[0] + 2.0,
            "the wall brightens away from the lamp: {profile:?}"
        );
    }
    let total = profile[0] - profile[profile.len() - 1];
    let worst = profile
        .windows(2)
        .map(|p| p[0] - p[1])
        .fold(0.0f64, f64::max);
    assert!(
        total > 80.0,
        "a lamp at one edge of a wall has to fall off across it: {total:.0} \
         levels from end to end"
    );
    assert!(
        worst < total / 4.0,
        "the falloff steps: the worst of {worst:.0} levels between two samples \
         is a seam, not a gradient, against {total:.0} across the whole wall"
    );
}

/// **A lamp's colour lands on what it lights**, and lands hardest where the
/// lamp is nearest.
#[test]
fn a_lamps_colour_is_strongest_where_it_is_nearest() {
    let Some(mut h) = Harness::new() else { return };
    let set = |editor: &mut Editor, c: Color| {
        editor.doc.edit("Lamp Colour", |scene| {
            let id = scene.lights().lights[0].id;
            scene.lights_mut().get_mut(id).expect("the lamp").color = c;
        });
    };
    let mut editor = wall(Color::from_rgb8(0xC0, 0xC0, 0xC0));
    lamp_on(&mut editor, Point::new(70.0, 200.0));

    let mut cache = DrawCache::default();
    set(&mut editor, Color::from_rgb8(0xFF, 0x40, 0x10));
    let warm = h.stage(&editor, &mut cache);
    set(&mut editor, Color::from_rgb8(0x10, 0x40, 0xFF));
    let cold = h.stage(&editor, &mut cache);

    let redness = |px: &[u8], x: f64| {
        let row = 256;
        let i = ((row * W + wall_x(x)) * 4) as usize;
        px[i] as f64 - px[i + 2] as f64
    };
    assert!(
        redness(&warm, 40.0) > redness(&cold, 40.0) + 40.0,
        "a warm lamp and a cold one must not paint the same wall: {:.0} \
         against {:.0}",
        redness(&warm, 40.0),
        redness(&cold, 40.0)
    );
    // And the colour is a lamp's, so it is strongest under the lamp.
    assert!(
        redness(&warm, 40.0) > redness(&warm, 510.0) + 20.0,
        "the lamp's colour must fall off with it: {:.0} beside it against \
         {:.0} across the stage",
        redness(&warm, 40.0),
        redness(&warm, 510.0)
    );
}

/// **Strength reads all the way up.**
///
/// Turning a lamp up used to do almost nothing above its default: the light was
/// one flat tint per shape, and the default already put the artwork at the
/// brightness it was painted, so everything above it only bleached. With the
/// falloff on the pixels, turning it up carries the pool further out — which is
/// what the control is for and what it now measures.
#[test]
fn a_lamps_strength_reads_across_its_range() {
    let Some(mut h) = Harness::new() else { return };
    let mut editor = wall(Color::from_rgb8(0x60, 0x60, 0x64));
    lamp_on(&mut editor, Point::new(70.0, 200.0));

    let mut cache = DrawCache::default();
    let read = |h: &mut Harness, editor: &mut Editor, cache: &mut DrawCache, k: f32| {
        editor.doc.edit("Strength", |scene| {
            let id = scene.lights().lights[0].id;
            scene.lights_mut().get_mut(id).expect("the lamp").intensity = k;
        });
        // Out in the middle distance, where turning a lamp up is what carries
        // its light. Under the lamp itself everything saturates and every
        // setting looks alike, which is how a working control reads as a dead
        // one.
        patch_luma(&h.stage(editor, cache), wall_x(240.0), 256, 6)
    };

    let dim = read(&mut h, &mut editor, &mut cache, 0.4);
    let mid = read(&mut h, &mut editor, &mut cache, 1.3);
    let bright = read(&mut h, &mut editor, &mut cache, 3.5);
    assert!(
        mid > dim + 15.0 && bright > mid + 15.0,
        "a lamp's strength has to read across its range: {dim:.0} then \
         {mid:.0} then {bright:.0}"
    );
}

/// **Moving a lamp moves the light**, across a single shape.
#[test]
fn carrying_a_lamp_across_a_wall_carries_the_bright_side_with_it() {
    let Some(mut h) = Harness::new() else { return };
    let mut editor = wall(Color::from_rgb8(0xE8, 0xE4, 0xDC));
    lamp_on(&mut editor, Point::new(70.0, 200.0));

    let mut cache = DrawCache::default();
    let left = h.stage(&editor, &mut cache);
    lamp_on(&mut editor, Point::new(480.0, 200.0));
    let right = h.stage(&editor, &mut cache);

    let (l_near, l_far) = (
        patch_luma(&left, wall_x(40.0), 256, 6),
        patch_luma(&left, wall_x(510.0), 256, 6),
    );
    let (r_near, r_far) = (
        patch_luma(&right, wall_x(510.0), 256, 6),
        patch_luma(&right, wall_x(40.0), 256, 6),
    );
    assert!(
        l_near > l_far + 60.0 && r_near > r_far + 60.0,
        "the bright side must follow the lamp: with it on the left the wall \
         read {l_near:.0} against {l_far:.0}, and on the right {r_near:.0} \
         against {r_far:.0}"
    );
}

// ---------------------------------------------------------------------------
// The three reports: a light's colour that cannot be seen, and a light — or a
// gloom — that lights nothing the *second* time it is added.
// ---------------------------------------------------------------------------

/// How dark the picture is overall. A gloom's whole job.
fn mean_luma(pixels: &[u8]) -> f64 {
    let mut sum = 0.0;
    let mut n = 0usize;
    for px in pixels.chunks_exact(4) {
        sum += 0.2126 * px[0] as f64 + 0.7152 * px[1] as f64 + 0.0722 * px[2] as f64;
        n += 1;
    }
    if n == 0 { 0.0 } else { sum / n as f64 }
}

fn delete_first_light(editor: &mut Editor) {
    editor.doc.edit("Delete Light", |scene| {
        let id = scene.lights().lights[0].id;
        scene.lights_mut().remove(id);
    });
}

/// **The report: use a light once, cancel it, and the next one does nothing.**
#[test]
fn a_light_added_after_the_last_one_was_deleted_still_lights() {
    let Some(mut h) = Harness::new() else { return };
    let mut cache = DrawCache::default();
    let mut editor = document();

    let unlit = h.stage(&editor, &mut cache);

    editor.add_light(LightKind::sun());
    let first = h.stage(&editor, &mut cache);
    assert!(
        difference(&unlit, &first) > 0.02,
        "the first sun did nothing at all"
    );

    delete_first_light(&mut editor);
    let back = h.stage(&editor, &mut cache);
    assert!(
        difference(&unlit, &back) < 0.005,
        "deleting the only light must put the picture back"
    );

    editor.add_light(LightKind::sun());
    let second = h.stage(&editor, &mut cache);
    assert!(
        difference(&unlit, &second) > 0.02,
        "the second sun lit nothing: {:.3}% of the stage moved",
        difference(&unlit, &second) * 100.0
    );
}

/// The same report, for a lamp — which is the light an animator reaches for.
#[test]
fn a_lamp_added_after_the_last_one_was_deleted_still_lights() {
    let Some(mut h) = Harness::new() else { return };
    let mut cache = DrawCache::default();
    let mut editor = document();
    let unlit = h.stage(&editor, &mut cache);

    editor.add_light(LightKind::lamp(Point::new(160.0, 140.0)));
    let first = h.stage(&editor, &mut cache);
    assert!(difference(&unlit, &first) > 0.02, "the first lamp did nothing");

    delete_first_light(&mut editor);
    let _ = h.stage(&editor, &mut cache);

    editor.add_light(LightKind::lamp(Point::new(160.0, 140.0)));
    let second = h.stage(&editor, &mut cache);
    assert!(
        difference(&unlit, &second) > 0.02,
        "the second lamp lit nothing: {:.3}% moved",
        difference(&unlit, &second) * 100.0
    );
}

/// **The report: the darkness does not come back either.**
#[test]
fn a_gloom_added_after_the_last_one_was_deleted_still_darkens() {
    let Some(mut h) = Harness::new() else { return };
    let mut cache = DrawCache::default();
    let mut editor = document();

    editor.add_light(LightKind::lamp(Point::new(430.0, 110.0)));
    let lamp_only = h.stage(&editor, &mut cache);

    let gloom = editor
        .scene()
        .lights()
        .opposing_gloom(editor.camera.visible_doc_rect());
    editor.add_light(gloom);
    let first = h.stage(&editor, &mut cache);
    assert!(
        mean_luma(&first) < mean_luma(&lamp_only) - 4.0,
        "the first gloom did not darken the stage: {:.1} against {:.1}",
        mean_luma(&first),
        mean_luma(&lamp_only)
    );

    editor.doc.edit("Delete Light", |scene| {
        let id = scene.lights().lights[1].id;
        scene.lights_mut().remove(id);
    });
    let _ = h.stage(&editor, &mut cache);

    let gloom = editor
        .scene()
        .lights()
        .opposing_gloom(editor.camera.visible_doc_rect());
    editor.add_light(gloom);
    let second = h.stage(&editor, &mut cache);
    assert!(
        mean_luma(&second) < mean_luma(&lamp_only) - 4.0,
        "the second gloom did not darken the stage: {:.1} against {:.1}",
        mean_luma(&second),
        mean_luma(&lamp_only)
    );
}

/// **The report: a lamp's colour cannot be seen.**
///
/// The sun's colour is covered above. A lamp is the other half, and it reaches
/// the picture by two different routes — the ramp laid over the artwork and the
/// pool laid over the frame — so a lamp's colour failing is not the same defect
/// as a sun's.
#[test]
fn changing_a_lamps_colour_reaches_the_stage() {
    let Some(mut h) = Harness::new() else { return };
    let mut cache = DrawCache::default();
    let mut editor = document();
    editor.add_light(LightKind::lamp(Point::new(275.0, 200.0)));

    let set = |editor: &mut Editor, c: Color| {
        editor.doc.edit("Light Colour", |scene| {
            let id = scene.lights().lights[0].id;
            scene.lights_mut().get_mut(id).expect("the lamp").color = c;
        });
    };

    set(&mut editor, Color::from_rgb8(0xFF, 0x60, 0x20));
    let warm = h.stage(&editor, &mut cache);
    set(&mut editor, Color::from_rgb8(0x20, 0x60, 0xFF));
    let cold = h.stage(&editor, &mut cache);

    assert!(
        warmth(&warm) > warmth(&cold) + 8.0,
        "a warm lamp ({:.1}) should be warmer than a cold one ({:.1})",
        warmth(&warm),
        warmth(&cold)
    );
}

/// A gloom, like everything else, has to survive the window drawing the same
/// document over and over into a cache that is warm.
#[test]
fn a_gloom_survives_a_warm_cache() {
    let Some(mut h) = Harness::new() else { return };
    let mut cache = DrawCache::default();
    let mut editor = document();
    editor.add_light(LightKind::lamp(Point::new(430.0, 110.0)));
    let gloom = editor
        .scene()
        .lights()
        .opposing_gloom(editor.camera.visible_doc_rect());
    editor.add_light(gloom);

    let first = h.stage(&editor, &mut cache);
    for frame in 2..=5 {
        let again = h.stage(&editor, &mut cache);
        assert!(
            difference(&first, &again) < 0.001,
            "frame {frame} differs from frame 1 by {:.3}% \u{2014} the darkness is \
             not being drawn the same way twice",
            difference(&first, &again) * 100.0
        );
    }
}

/// **A light born while the stage has no area must still light the stage.**
///
/// The pixel half of `lighting_reports::a_light_added_before_the_stage_is_laid
/// _out_is_still_visible`: the reach was sized from a viewport that is empty
/// until the stage is laid out, so the lamp came out with the minimum reach of
/// forty units on a stage five hundred and fifty across.
#[test]
fn a_light_added_with_no_stage_area_still_lights_the_stage() {
    let Some(mut h) = Harness::new() else { return };
    let mut cache = DrawCache::default();
    let mut editor = document();
    let unlit = h.stage(&editor, &mut cache);

    // What the camera holds before the stage has been given any room.
    let restore = editor.camera.viewport;
    editor.camera.viewport = Size::new(0.0, 0.0);
    editor.add_light(LightKind::lamp(Point::ORIGIN));
    editor.camera.viewport = restore;

    let lit = h.stage(&editor, &mut cache);
    let moved = difference(&unlit, &lit);
    // Half the picture, not merely "something changed": a lamp with the minimum
    // reach of forty units still lays a small bright disc, and a test that only
    // asked whether pixels moved would call that a working light.
    assert!(
        moved > 0.5,
        "the lamp reached {:.1}% of the stage, so it was born too small to light the shot",
        moved * 100.0
    );
}

/// The same, for a wall of dark: born with no view it used to be one unit long
/// and one and a half wide.
#[test]
fn a_gloom_added_with_no_stage_area_still_darkens_the_stage() {
    let Some(mut h) = Harness::new() else { return };
    let mut cache = DrawCache::default();
    let mut editor = document();
    editor.add_light(LightKind::lamp(Point::new(430.0, 110.0)));
    let lamp_only = h.stage(&editor, &mut cache);

    let restore = editor.camera.viewport;
    editor.camera.viewport = Size::new(0.0, 0.0);
    editor.add_light(LightKind::gloom(Point::ORIGIN));
    editor.camera.viewport = restore;

    let darkened = h.stage(&editor, &mut cache);
    assert!(
        mean_luma(&darkened) < mean_luma(&lamp_only) - 4.0,
        "the gloom was a speck rather than a wall: {:.1} against {:.1}",
        mean_luma(&darkened),
        mean_luma(&lamp_only)
    );
}

/// Switch a light off in the panel and on again.
fn switch(editor: &mut Editor, on: bool) {
    editor.doc.edit("Light", |scene| {
        let id = scene.lights().lights[0].id;
        if let Some(light) = scene.lights_mut().get_mut(id) {
            light.enabled = on;
        }
    });
}

/// **The report, in the user's own words:** select a lamp, see its effect,
/// switch the lamp off, switch it on again — and the effect is gone.
///
/// Drawn through a cache that stays warm across the whole sequence, and with
/// the light left off long enough for its crescents to age out (`KEEP_FRAMES`
/// is three), because that is what a person switching a light off and looking
/// at the result actually does.
#[test]
fn a_lamp_switched_off_and_on_again_lights_the_same_way() {
    let Some(mut h) = Harness::new() else { return };
    let mut cache = DrawCache::default();
    let mut editor = document();
    let unlit = h.stage(&editor, &mut cache);

    editor.add_light(LightKind::lamp(Point::ORIGIN));
    let first = h.stage(&editor, &mut cache);
    assert!(
        difference(&unlit, &first) > 0.02,
        "the lamp did nothing the first time"
    );

    switch(&mut editor, false);
    let mut off = Vec::new();
    for _ in 0..6 {
        off = h.stage(&editor, &mut cache);
    }
    assert!(
        difference(&unlit, &off) < 0.005,
        "switching the lamp off must put the picture back"
    );

    switch(&mut editor, true);
    let again = h.stage(&editor, &mut cache);
    let drift = difference(&first, &again);
    assert!(
        drift < 0.005,
        "the lamp switched back on lit {:.3}% of the stage differently from the \
         first time it was on",
        drift * 100.0
    );
}

/// The same for a gloom: switched off and on again, the darkness must come back
/// exactly as it was.
#[test]
fn a_gloom_switched_off_and_on_again_darkens_the_same_way() {
    let Some(mut h) = Harness::new() else { return };
    let mut cache = DrawCache::default();
    let mut editor = document();
    editor.add_light(LightKind::lamp(Point::new(430.0, 110.0)));
    let lamp_only = h.stage(&editor, &mut cache);

    editor.add_light(LightKind::gloom(Point::ORIGIN));
    let first = h.stage(&editor, &mut cache);
    assert!(mean_luma(&first) < mean_luma(&lamp_only) - 4.0, "no darkness at all");

    editor.doc.edit("Light", |scene| {
        let id = scene.lights().lights[1].id;
        if let Some(light) = scene.lights_mut().get_mut(id) {
            light.enabled = false;
        }
    });
    for _ in 0..6 {
        let _ = h.stage(&editor, &mut cache);
    }

    editor.doc.edit("Light", |scene| {
        let id = scene.lights().lights[1].id;
        if let Some(light) = scene.lights_mut().get_mut(id) {
            light.enabled = true;
        }
    });
    let again = h.stage(&editor, &mut cache);
    assert!(
        difference(&first, &again) < 0.005,
        "the darkness came back different from how it went away"
    );
}

/// And the rig's own switch, which is the other way to turn the lighting off
/// and on again.
#[test]
fn the_rig_switched_off_and_on_again_lights_the_same_way() {
    let Some(mut h) = Harness::new() else { return };
    let mut cache = DrawCache::default();
    let mut editor = document();
    editor.add_light(LightKind::lamp(Point::ORIGIN));
    let first = h.stage(&editor, &mut cache);

    editor.doc.edit("Lighting", |scene| scene.lights_mut().enabled = false);
    for _ in 0..6 {
        let _ = h.stage(&editor, &mut cache);
    }
    editor.doc.edit("Lighting", |scene| scene.lights_mut().enabled = true);

    let again = h.stage(&editor, &mut cache);
    assert!(
        difference(&first, &again) < 0.005,
        "the rig switched back on lit {:.3}% of the stage differently",
        difference(&first, &again) * 100.0
    );
}

// ---------------------------------------------------------------------------
// The window's own frame loop, with the pixels read off it.
// ---------------------------------------------------------------------------

/// **`App::render`, reduced to the decisions that shape the stage encoding.**
///
/// Every test above calls `build_scene` once per frame, which is not what the
/// window does. The window keeps last frame's Vello encoding and **re-renders
/// it** whenever a stamp of its inputs matches; it builds crescents off-thread
/// when the cache is cold; and it only records what it could not light on a
/// frame where the light has come to rest. Those three together are where a
/// lighting change can fail to reach the screen while every single-frame test
/// passes, so this drives them in the same order `App::render` does and
/// presents whatever the window would actually have on screen.
struct WindowSim {
    editor: Editor,
    cache: DrawCache,
    vello: vello::Scene,
    build: Option<Build>,
    shade_aim: u64,
    stage_stale: bool,
    lights_generation: u64,
    last_rig: u64,
    stamp: Option<(u64, u32, u64, u64)>,
    /// Whether another frame is owed. The window sleeps when it is not.
    owed: bool,
}

struct Build {
    results: crossbeam_channel::Receiver<Vec<buzz_render::lighting::Built>>,
    abandon: std::sync::Arc<std::sync::atomic::AtomicBool>,
    aim: u64,
}

impl WindowSim {
    fn new(editor: Editor) -> Self {
        Self {
            editor,
            cache: DrawCache::default(),
            vello: vello::Scene::new(),
            build: None,
            shade_aim: 0,
            stage_stale: false,
            lights_generation: 0,
            last_rig: 0,
            stamp: None,
            owed: true,
        }
    }

    /// One window frame. Returns what is on screen at the end of it.
    fn frame(&mut self, h: &mut Harness) -> Vec<u8> {
        use std::sync::atomic::Ordering;
        self.owed = false;

        let (aim, rig) = {
            let resolved = self
                .editor
                .scene()
                .lights()
                .resolved_at(self.editor.current_frame);
            (resolved.aim(), resolved.fingerprint())
        };

        if let Some(build) = &self.build {
            if let Ok(built) = build.results.try_recv() {
                self.cache.lights.install(built);
                self.build = None;
                self.lights_generation += 1;
                self.owed = true;
            } else if build.aim != aim {
                build.abandon.store(true, Ordering::Relaxed);
                self.build = None;
                self.owed = true;
            } else {
                self.owed = true;
            }
        }

        // Kept in step with `app.rs`: a document whose lighting has been
        // trimmed back will never build a crescent, so calling it cold forever
        // would refuse the retained encoding on every frame.
        let cold = self.cache.lights.is_empty()
            && self.editor.scene().lights().is_active()
            && self.cache.detail() == buzz_render::document::LightDetail::Full;
        let building = self.build.is_some();
        let settled = aim == self.shade_aim;
        self.shade_aim = aim;
        self.cache.lights.set_inline_budget(if cold || building {
            std::time::Duration::ZERO
        } else {
            buzz_render::lighting::INLINE_BUDGET
        });
        self.cache.lights.set_queue(!building && settled);

        let stamp = (
            self.editor.scene().revision(),
            self.editor.current_frame,
            self.lights_generation,
            rig,
        );
        let stale_encoding = self.stage_stale && self.build.is_none();
        let reuse = !cold && !stale_encoding && self.stamp == Some(stamp);
        if !reuse {
            self.vello.reset();
            buzz_app::stage::build_scene(
                &mut self.vello,
                &self.editor,
                Rect::new(0.0, 0.0, W as f64, H as f64),
                1.0,
                &mut self.cache,
            );
            self.stamp = Some(stamp);
            self.stage_stale = self.cache.lights.is_stale();
        }

        let misses = self.cache.lights.take_misses();
        self.cache.lights.set_defer(false);
        if !misses.is_empty() && self.build.is_none() {
            let (send, results) = crossbeam_channel::bounded(1);
            let abandon = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
            let stop = std::sync::Arc::clone(&abandon);
            std::thread::spawn(move || {
                use rayon::prelude::*;
                let built = misses
                    .into_par_iter()
                    .filter(|_| !stop.load(Ordering::Relaxed))
                    .map(buzz_render::lighting::Miss::build)
                    .collect::<Vec<_>>();
                let _ = send.send(built);
            });
            self.build = Some(Build {
                results,
                abandon,
                aim,
            });
            self.owed = true;
        }

        if self.stage_stale {
            self.owed = true;
        }
        if rig != self.last_rig {
            self.last_rig = rig;
            self.owed = true;
        }

        h.present(&self.vello)
    }

    /// Draw for as long as the window would, and return what it finally shows.
    ///
    /// Generously bounded. The off-thread build is real work on a real pool, and
    /// this file runs beside a dozen other GPU tests — a tight bound turns "the
    /// machine was busy" into "the lighting is broken", which is the one thing a
    /// test of the lighting must not say.
    fn settle(&mut self, h: &mut Harness) -> Vec<u8> {
        let mut pixels = self.frame(h);
        for _ in 0..200 {
            if !self.owed {
                return pixels;
            }
            std::thread::sleep(std::time::Duration::from_millis(3));
            pixels = self.frame(h);
        }
        pixels
    }
}

/// **The report, driven through the window's own frame loop.**
///
/// Select a lamp and see its effect; switch the lamp off; switch it on again.
/// The single-frame tests above cannot see this failure, because they rebuild
/// the encoding every frame and build every crescent on the spot. The window
/// does neither.
#[test]
fn a_lamp_switched_off_and_on_again_reaches_the_window() {
    let Some(mut h) = Harness::new() else { return };
    let mut editor = document();
    editor.add_light(LightKind::lamp(Point::ORIGIN));
    let mut window = WindowSim::new(editor);

    let first = window.settle(&mut h);

    switch(&mut window.editor, false);
    let mut off = Vec::new();
    for _ in 0..6 {
        off = window.settle(&mut h);
    }
    assert!(
        difference(&first, &off) > 0.02,
        "switching the lamp off did not change the window"
    );

    switch(&mut window.editor, true);
    let again = window.settle(&mut h);
    let drift = difference(&first, &again);
    assert!(
        drift < 0.01,
        "the lamp was switched back on and the window shows something {:.2}% \
         different from the first time it was on",
        drift * 100.0
    );
}

/// The same for a wall of dark, which builds no geometry at all, so nothing in
/// the deferred machinery is keeping the window awake on its behalf.
#[test]
fn a_gloom_switched_off_and_on_again_reaches_the_window() {
    let Some(mut h) = Harness::new() else { return };
    let mut editor = document();
    editor.add_light(LightKind::lamp(Point::new(430.0, 110.0)));
    editor.add_light(LightKind::gloom(Point::ORIGIN));
    let mut window = WindowSim::new(editor);

    let first = window.settle(&mut h);

    fn set(window: &mut WindowSim, on: bool) {
        window.editor.doc.edit("Light", |scene| {
            let id = scene.lights().lights[1].id;
            if let Some(light) = scene.lights_mut().get_mut(id) {
                light.enabled = on;
            }
        });
    }

    set(&mut window, false);
    let mut off = Vec::new();
    for _ in 0..6 {
        off = window.settle(&mut h);
    }
    assert!(
        mean_luma(&off) > mean_luma(&first) + 4.0,
        "switching the gloom off did not lift the picture"
    );

    set(&mut window, true);
    let again = window.settle(&mut h);
    assert!(
        difference(&first, &again) < 0.01,
        "the darkness did not come back: {:.2}% of the window differs from the \
         first time it was on",
        difference(&first, &again) * 100.0
    );
}

/// Recolouring a light generates no crescents either, so the same question:
/// does the window ever show the new colour?
#[test]
fn recolouring_a_lamp_reaches_the_window() {
    let Some(mut h) = Harness::new() else { return };
    let mut editor = document();
    editor.add_light(LightKind::lamp(Point::new(275.0, 200.0)));
    let mut window = WindowSim::new(editor);

    fn set(window: &mut WindowSim, c: Color) {
        window.editor.doc.edit("Light Colour", |scene| {
            let id = scene.lights().lights[0].id;
            scene.lights_mut().get_mut(id).expect("the lamp").color = c;
        });
    }

    set(&mut window, Color::from_rgb8(0xFF, 0x60, 0x20));
    let warm = window.settle(&mut h);
    set(&mut window, Color::from_rgb8(0x20, 0x60, 0xFF));
    let cold = window.settle(&mut h);

    assert!(
        warmth(&warm) > warmth(&cold) + 8.0,
        "the window kept the warm lamp on screen after it was made cold: \
         {:.1} against {:.1}",
        warmth(&warm),
        warmth(&cold)
    );
}

/// **A stage area that is not a rectangle must not black the frame.**
///
/// The report: the app opens black, and it goes black again when the window is
/// maximised, and the lights do not work.
///
/// The window derives the stage's viewport offset from the rectangle egui gave
/// the central panel, and that rectangle is `egui::Rect::NOTHING` — infinities
/// — until the layout has been measured once. That is the first frame of a
/// session, and it is a frame again after a resize, before egui has measured
/// the new size. Applied, the infinity goes into the GPU transform and every
/// coordinate through it comes out NaN: nothing rasterises.
///
/// What is on screen then is a black stage. Because a light that did not work
/// would leave exactly that, it reads as the lighting being broken — but the
/// lighting is fine and no artwork was drawn at all.
#[test]
fn an_unmeasured_stage_area_still_draws_the_artwork() {
    let Some(mut h) = Harness::new() else { return };
    let mut editor = document();
    editor.add_light(LightKind::lamp(Point::ORIGIN));

    let mut cache = DrawCache::default();
    let good = h.stage_through(&editor, &mut cache, Rect::new(0.0, 0.0, W as f64, H as f64));

    // What `egui::Rect::NOTHING` scales to, and the other ways a layout that has
    // not settled can arrive.
    for (what, area) in [
        (
            "Rect::NOTHING",
            Rect::new(
                f64::INFINITY,
                f64::INFINITY,
                f64::NEG_INFINITY,
                f64::NEG_INFINITY,
            ),
        ),
        ("a NaN", Rect::new(f64::NAN, f64::NAN, f64::NAN, f64::NAN)),
    ] {
        let mut cache = DrawCache::default();
        let drawn = h.stage_through(&editor, &mut cache, area);
        let moved = difference(&good, &drawn);
        assert!(
            moved < 0.02,
            "{what} as a stage area left {:.1}% of the frame different from a \
             measured one: the stage went black",
            moved * 100.0
        );
    }
}

/// **The report:** switch the lighting off, switch it back on, and after that
/// the light's colour cannot be changed any more.
#[test]
fn a_lamp_recolours_after_the_rig_has_been_switched_off_and_on() {
    let Some(mut h) = Harness::new() else { return };
    let mut editor = document();
    editor.add_light(LightKind::lamp(Point::new(275.0, 200.0)));
    let mut window = WindowSim::new(editor);

    fn recolour(window: &mut WindowSim, c: Color) {
        window.editor.doc.edit("Light Colour", |scene| {
            let id = scene.lights().lights[0].id;
            scene.lights_mut().get_mut(id).expect("the lamp").color = c;
        });
    }
    fn rig(window: &mut WindowSim, on: bool) {
        window
            .editor
            .doc
            .edit("Lighting", |scene| scene.lights_mut().enabled = on);
    }

    // It works before the toggle.
    recolour(&mut window, Color::from_rgb8(0xFF, 0x60, 0x20));
    let warm_before = window.settle(&mut h);
    recolour(&mut window, Color::from_rgb8(0x20, 0x60, 0xFF));
    let cold_before = window.settle(&mut h);
    assert!(
        warmth(&warm_before) > warmth(&cold_before) + 8.0,
        "the colour did not reach the window even before the toggle"
    );

    // Off, a while, and on again.
    rig(&mut window, false);
    for _ in 0..6 {
        let _ = window.settle(&mut h);
    }
    rig(&mut window, true);
    let _ = window.settle(&mut h);

    // And it has to still work.
    recolour(&mut window, Color::from_rgb8(0xFF, 0x60, 0x20));
    let warm = window.settle(&mut h);
    recolour(&mut window, Color::from_rgb8(0x20, 0x60, 0xFF));
    let cold = window.settle(&mut h);
    assert!(
        warmth(&warm) > warmth(&cold) + 8.0,
        "after the rig was switched off and on, recolouring the lamp stopped \
         reaching the window: warm {:.1} against cold {:.1}",
        warmth(&warm),
        warmth(&cold)
    );
}


// ---------------------------------------------------------------------------
// **A frame the rasteriser cannot take.**
//
// The report these came from: a lamp added to an imported film, switched on,
// coloured red, and nothing on the stage changed at all — not when it moved,
// not when it was recoloured, not with the ambient set to pure red. The picture
// was byte-identical every time.
//
// Nothing was wrong with the light. Vello rasterises from a fixed buffer of
// 1 << 21 flattened lines, `render_to_texture` does not check whether the bump
// allocator failed, and that film's lit frame wanted 3.46 M. So the fine pass
// wrote nothing, the target kept the last frame that *had* landed — the unlit
// one — and the light appeared to do nothing.
//
// See `buzz_render::document::LightDetail`.
// ---------------------------------------------------------------------------

/// Artwork heavy enough to matter: `shapes` shapes of `segments` segments each,
/// spread across the stage so none of them is culled.
fn dense_document(shapes: usize, segments: usize) -> Editor {
    use buzz_geom::{BezPath, PathEl};

    let mut scene = Scene::default();
    scene.stage_mut().background = Color::WHITE;
    scene.stage_mut().size = Size::new(1000.0, 1000.0);
    let layer = scene.add_layer("Art", LayerKind::Normal);

    let across = (shapes as f64).sqrt().ceil() as usize;
    let step = 1000.0 / across as f64;
    for i in 0..shapes {
        let cx = (i % across) as f64 * step + step * 0.5;
        let cy = (i / across) as f64 * step + step * 0.5;
        // **Realistic density, not a scribble in a thimble.** Two earlier
        // versions of this fixture were pathological and both produced frames
        // the rasteriser silently dropped, so every test built on them was
        // measuring a blank: first a comb of alternating up-down lines, which
        // crosses every tile it touches dozens of times, then a polygon of the
        // same segment count crammed into a cell a few pixels across. Real
        // artwork is many overlapping shapes whose outlines carry a segment
        // every pixel or two, so that is what this draws — the shapes are wide
        // enough that their outlines are long enough to hold their segments.
        let r = step * 1.6;
        let mut path = BezPath::new();
        for s in 0..segments {
            let t = s as f64 / segments as f64 * std::f64::consts::TAU;
            let p = Point::new(cx + r * t.cos(), cy + r * t.sin());
            if s == 0 {
                path.push(PathEl::MoveTo(p));
            } else {
                path.push(PathEl::LineTo(p));
            }
        }
        path.push(PathEl::ClosePath);
        scene.add_shape(layer, ShapeData::filled(path, ART));
    }

    let mut editor = Editor::new(Document::new(scene));
    editor.camera.viewport = Size::new(W as f64, H as f64);
    editor.camera.center = Point::new(500.0, 500.0);
    editor.camera.zoom = W as f64 / 1000.0;
    editor
}

/// **The dense fixture has to actually render**, or every test built on it is
/// measuring a blank frame and passing for the wrong reason.
#[test]
fn a_dense_document_still_renders() {
    let Some(mut h) = Harness::new() else { return };
    let mut editor = dense_document(500, 400);
    let mut cache = DrawCache::default();
    let blank = {
        // A known picture to compare against: an empty stage.
        let empty = Editor::new(Document::new(Scene::default()));
        h.stage(&empty, &mut DrawCache::default())
    };
    let unlit = h.stage(&editor, &mut cache);
    assert!(
        difference(&blank, &unlit) > 0.2,
        "the fixture drew nothing at all"
    );

    editor.add_light(LightKind::sun());
    for _ in 0..3 {
        h.stage(&editor, &mut cache);
    }
    let lit = h.stage(&editor, &mut cache);
    assert!(
        difference(&unlit, &lit) > 0.05,
        "the fixture's lit frame never landed: it is showing the unlit one"
    );
}

/// Encode the stage the way the window does, and answer what it cost.
fn encode_stage(editor: &Editor, cache: &mut DrawCache) -> u32 {
    let mut vello = vello::Scene::new();
    buzz_app::stage::build_scene(
        &mut vello,
        editor,
        Rect::new(0.0, 0.0, W as f64, H as f64),
        1.0,
        cache,
    );
    vello.encoding().n_path_segments
}

/// **The report.** A document whose lit frame will not fit trims the light
/// rather than losing the frame — and what it hands the rasteriser is inside
/// what the rasteriser can take.
#[test]
fn a_frame_too_big_to_rasterise_trims_its_lighting_rather_than_vanishing() {
    use buzz_render::document::{LightDetail, segment_ceiling};

    let mut editor = dense_document(700, 600);
    let mut cache = DrawCache::default();

    // Unlit, this document is already most of what can be encoded, and nothing
    // is trimmed: there is no light to trim.
    let ceiling = segment_ceiling((W as f64) * (H as f64));
    let unlit = encode_stage(&editor, &mut cache);
    assert!(
        unlit < ceiling,
        "the fixture must fit unlit, or it is testing the wrong thing: {unlit} of {ceiling}"
    );
    assert_eq!(cache.detail(), LightDetail::Full, "nothing to give up");

    editor.add_light(LightKind::sun());
    let lit = encode_stage(&editor, &mut cache);
    assert_ne!(
        cache.detail(),
        LightDetail::Full,
        "a frame this size cannot carry its modelling and must say so"
    );
    assert!(
        lit <= ceiling,
        "what was handed to the rasteriser is still over the ceiling: {lit} of {ceiling}"
    );

    // **Not asserted here: that the trimmed frame then renders.**
    //
    // It is the property the whole mechanism exists for, and it is measured —
    // on the film the ceiling was fitted to, by `diagnose_a_saved_document`,
    // and on honest artwork by `a_dense_document_still_renders`. It cannot be
    // asserted *on this fixture*, and the reason is worth writing down: the
    // ceiling counts segments, and what a segment costs the rasteriser depends
    // on the artwork. A synthetic document dense enough to trip a ceiling
    // fitted to real drawing is, by construction, dearer per segment than the
    // thing it was fitted to — so it goes over while still under the count.
    // Asserting otherwise here would be demanding a guarantee a proxy cannot
    // give, and the honest place for that guarantee is the calibration test in
    // `buzz_render::document`.
}

/// And it settles: the level does not come and go frame after frame, which
/// would flicker the shading on and off for as long as the document was open.
#[test]
fn a_trimmed_frame_stays_trimmed() {
    let mut editor = dense_document(700, 600);
    editor.add_light(LightKind::sun());
    let mut cache = DrawCache::default();

    encode_stage(&editor, &mut cache);
    let settled = cache.detail();
    for _ in 0..8 {
        encode_stage(&editor, &mut cache);
        assert_eq!(cache.detail(), settled, "the level must not oscillate");
    }
}

/// A document small enough keeps everything, which is every document that was
/// working before this existed.
#[test]
fn an_ordinary_document_gives_up_nothing() {
    use buzz_render::document::LightDetail;

    let mut editor = document();
    editor.add_light(LightKind::sun());
    let mut cache = DrawCache::default();
    for _ in 0..3 {
        encode_stage(&editor, &mut cache);
    }
    assert_eq!(cache.detail(), LightDetail::Full);
}

/// **Switched off and on again, a trimmed document lights exactly as before.**
///
/// The report: with the light and its shadows on, switching them off and back
/// on does not bring them back. The switch-and-back tests that already exist
/// all run on a handful of shapes, where nothing is ever trimmed — so none of
/// them exercises the one thing a heavy document does differently, which is
/// change its own lighting level between the frame before and the frame after.
#[test]
fn a_trimmed_document_lights_the_same_after_a_switch_off_and_on() {
    let Some(mut h) = Harness::new() else { return };
    let mut editor = dense_document(500, 400);
    editor.add_light(LightKind::sun());
    let mut cache = DrawCache::default();

    // Settle: the first lit frame is the one that discovers it must trim.
    for _ in 0..3 {
        h.stage(&editor, &mut cache);
    }
    let lit = h.stage(&editor, &mut cache);
    let settled = cache.detail();
    eprintln!("lit:  {:?} encode {} ceiling {}", cache.detail(), cache.last_encode(), cache.last_ceiling());

    editor.doc.edit("Rig off", |s| s.lights_mut().enabled = false);
    let off = h.stage(&editor, &mut cache);
    eprintln!("off:  {:?} encode {} ceiling {}", cache.detail(), cache.last_encode(), cache.last_ceiling());
    assert!(
        difference(&lit, &off) > 0.02,
        "switching the rig off must change the picture, or this proves nothing"
    );

    editor.doc.edit("Rig on", |s| s.lights_mut().enabled = true);
    let back = h.stage(&editor, &mut cache);
    eprintln!("back: {:?} encode {} ceiling {}", cache.detail(), cache.last_encode(), cache.last_ceiling());
    let again = h.stage(&editor, &mut cache);
    eprintln!(
        "again: {:?} encode {} | back==off {} | again==off {} | again==lit {}",
        cache.detail(),
        cache.last_encode(),
        back == off,
        again == off,
        again == lit,
    );
    {
        let d = "B:/youtubeProjects/Buzzcaf_Media/BuzzAnimate/graphify-out/toggle";
        std::fs::create_dir_all(d).ok();
        for (n, px) in [("1-lit", &lit), ("2-off", &off), ("3-back", &back)] {
            let f = std::fs::File::create(format!("{d}/{n}.png")).unwrap();
            let mut e = png::Encoder::new(std::io::BufWriter::new(f), W, H);
            e.set_color(png::ColorType::Rgba);
            e.set_depth(png::BitDepth::Eight);
            e.write_header().unwrap().write_image_data(px).unwrap();
        }
    }
    assert_eq!(cache.detail(), settled, "the level must come back too");
    assert!(
        difference(&lit, &back) < 0.01,
        "switched back on, {:.1}% of the frame is still unlit",
        difference(&lit, &back) * 100.0
    );
}

/// The same for one light's shadows, which is their own switch.
///
/// On a document held at a trimmed level, since that is the state the report
/// was made from. Sparse artwork, because a shadow hidden behind the shape
/// beside it would make this pass whatever happened.
#[test]
fn shadows_come_back_after_their_switch_on_a_trimmed_document() {
    use buzz_render::document::LightDetail;

    let Some(mut h) = Harness::new() else { return };
    let mut editor = document();
    editor.add_light(LightKind::sun());
    editor.doc.edit("A low sun", |scene| {
        let id = scene.lights().lights[0].id;
        let light = scene.lights_mut().get_mut(id).expect("the sun");
        light.kind = LightKind::Sun {
            azimuth: 0.0,
            elevation: 0.5,
        };
        light.shadows = true;
    });
    let mut cache = DrawCache::default();
    cache.pin_detail(Some(LightDetail::NoModelling));
    let with = h.stage(&editor, &mut cache);

    let switch = |editor: &mut Editor, on: bool| {
        editor.doc.edit("Shadows", |scene| {
            let id = scene.lights().lights[0].id;
            scene.lights_mut().get_mut(id).expect("the sun").shadows = on;
        });
    };

    switch(&mut editor, false);
    let without = h.stage(&editor, &mut cache);
    assert!(
        difference(&with, &without) > 0.005,
        "switching the shadows off must change the picture, or this proves          nothing: {:.3}%",
        difference(&with, &without) * 100.0
    );

    switch(&mut editor, true);
    let back = h.stage(&editor, &mut cache);
    assert!(
        difference(&with, &back) < 0.002,
        "switched back on, {:.2}% of the frame has no shadow",
        difference(&with, &back) * 100.0
    );
}

/// **What a trimmed frame must still do: light the picture.**
///
/// Giving up the modelling is only acceptable because the part that says a
/// light is on — its colour on the artwork, and its pool — costs no geometry
/// and is never given up. If a trimmed frame stopped colouring the drawing, the
/// trim would be the very bug it exists to fix.
#[test]
fn a_trimmed_frame_still_takes_the_lights_colour() {
    use buzz_render::document::LightDetail;

    let Some(mut h) = Harness::new() else { return };
    for detail in [LightDetail::NoModelling, LightDetail::Flat] {
        let mut cache = DrawCache::default();
        cache.pin_detail(Some(detail));
        let mut editor = document();
        let unlit = h.stage(&editor, &mut cache);

        editor.add_light(LightKind::sun());
        editor.doc.edit("Warm", |scene| {
            let id = scene.lights().lights[0].id;
            let light = scene.lights_mut().get_mut(id).expect("the sun");
            light.color = Color::from_rgb8(0xFF, 0x7A, 0x10);
            light.intensity = 2.0;
        });
        let lit = h.stage(&editor, &mut cache);

        assert_eq!(cache.detail(), detail, "the pin must hold");
        assert!(
            warmth(&lit) > warmth(&unlit) + 8.0,
            "{detail:?} must still put the light's colour on the artwork: \
             {:.2} -> {:.2}",
            warmth(&unlit),
            warmth(&lit)
        );
    }
}

/// **A caster made of several shapes throws one shadow, not one per shape.**
///
/// The report: the shadows look like overlapping parts, some darker than
/// others. They were. Every shape cast its own, filled with black at the
/// light's shadow strength, and drawn one over another — so where two of a
/// character's shapes overlapped, and they overlap everywhere, the alphas
/// compounded: `1-(1-a)^n`. A figure of a hundred shapes came out as a
/// patchwork of a hundred different darknesses with every internal seam
/// showing, which is not a shadow, it is a stack of them.
///
/// A shadow is the silhouette of what casts it, at one tone.
#[test]
fn overlapping_shapes_cast_one_shadow_at_one_tone() {
    let Some(mut h) = Harness::new() else { return };

    // Two squares that overlap down the middle, well clear of the shadow they
    // will throw so the measurement is of the shadow alone.
    let mut scene = Scene::default();
    scene.stage_mut().background = Color::WHITE;
    scene.stage_mut().size = Size::new(550.0, 400.0);
    let layer = scene.add_layer("Art", LayerKind::Normal);
    // Coloured, not grey: then a grey pixel can only be the white stage with
    // shadow on it, and the artwork cannot be mistaken for its own shadow.
    let blue = Color::from_rgb8(0x20, 0x40, 0xC0);
    scene.add_shape(
        layer,
        ShapeData::filled(Rect::new(120.0, 60.0, 240.0, 180.0).to_path(1e-9), blue),
    );
    scene.add_shape(
        layer,
        ShapeData::filled(Rect::new(180.0, 60.0, 300.0, 180.0).to_path(1e-9), blue),
    );

    let mut editor = Editor::new(Document::new(scene));
    editor.camera.viewport = Size::new(W as f64, H as f64);
    editor.camera.center = Point::new(275.0, 200.0);
    editor.camera.zoom = 0.8;
    editor.add_light(LightKind::sun());
    editor.doc.edit("A low sun", |scene| {
        let id = scene.lights().lights[0].id;
        let light = scene.lights_mut().get_mut(id).expect("the sun");
        // Low and to one side, so the shadow lands clear of the artwork.
        light.kind = LightKind::Sun {
            azimuth: 0.0,
            elevation: 0.55,
        };
        light.shadows = true;
        light.shadow_strength = 0.5;
        // Nothing else may darken the picture, or the measurement is of the
        // rig rather than of the shadow.
        scene.lights_mut().base = Color::WHITE;
        scene.lights_mut().modelling = 0.0;
    });

    let px = h.stage(&editor, &mut DrawCache::default());

    // Black at `strength` over the white stage is one tone — 128 at a half.
    // Two of them stacked is 64. Edges ramp from the shadow's tone up to the
    // stage, so the *light* side of the range is antialiasing and says nothing;
    // anything **darker** than one shadow can only be two.
    let single = 255.0 * (1.0 - 0.5);
    let stacked = 255.0 * (1.0 - 0.5 * 1.5);
    let mut at_single = 0usize;
    let mut darker = 0usize;
    for px in px.chunks_exact(4) {
        let (r, g, b) = (px[0], px[1], px[2]);
        if r != g || g != b || r >= 0xF4 {
            continue;
        }
        if (r as f64 - single).abs() <= 6.0 {
            at_single += 1;
        } else if (r as f64) < single - 6.0 {
            darker += 1;
        }
    }
    assert!(at_single > 200, "no shadow was drawn at all: {at_single} pixels");
    assert!(
        darker * 200 < at_single,
        "{darker} pixels are darker than one shadow ({single:.0}) against          {at_single} at it — around {stacked:.0}, two shapes' shadows are          stacking where they overlap instead of making one silhouette"
    );
}

/// Not a test: open a real `.buzz` and report what each light does to the
/// window's own encoding. `BUZZ_DOC=<file.buzz>`, `BUZZ_DUMP=<dir>`.
#[test]
#[ignore = "diagnostic"]
fn diagnose_a_saved_document() {
    let Ok(path) = std::env::var("BUZZ_DOC") else {
        return;
    };
    let doc = Document::open(std::path::Path::new(&path)).expect("open");
    let mut editor = Editor::new(doc);
    let rig = editor.scene().lights().clone();
    eprintln!(
        "rig enabled {} active {} base {:?} modelling {} lights {}",
        rig.enabled,
        rig.is_active(),
        rig.base,
        rig.modelling,
        rig.lights.len()
    );
    for l in &rig.lights {
        eprintln!(
            "  {:?} enabled {} x{} shadows {} strength {} stands off {} {:?}",
            l.id, l.enabled, l.intensity, l.shadows, l.shadow_strength, l.standing_height, l.kind
        );
        if let buzz_scene::LightKind::Lamp { height, .. } = l.kind {
            let gap = height - l.standing_height;
            eprintln!(
                "    shadow scale = {height} / ({height} - {}) = {:.2} (capped at 2.0)",
                l.standing_height,
                (height / gap.max(1e-9)).clamp(1.0, 2.0)
            );
        }
    }

    let Some(mut h) = Harness::new() else { return };
    editor.camera.viewport = Size::new(W as f64, H as f64);
    let stage = editor.scene().stage().size;
    editor.camera.center = Point::new(stage.width / 2.0, stage.height / 2.0);
    editor.camera.zoom = (W as f64 / stage.width).min(H as f64 / stage.height) * 0.9;

    let mut cache = DrawCache::default();
    let dump = std::env::var("BUZZ_DUMP").ok();
    if let Some(d) = &dump {
        std::fs::create_dir_all(d).ok();
    }
    let shot = |h: &mut Harness, cache: &mut DrawCache, editor: &Editor, what: &str| {
        let px = h.stage(editor, cache);
        eprintln!(
            "  {what}: detail {:?} encode {} of {}",
            cache.detail(),
            cache.last_encode(),
            cache.last_ceiling()
        );
        if let Some(d) = &dump {
            let file = std::fs::File::create(
                std::path::Path::new(d).join(format!("{}.png", what.replace(' ', "-"))),
            )
            .expect("create");
            let mut enc = png::Encoder::new(std::io::BufWriter::new(file), W, H);
            enc.set_color(png::ColorType::Rgba);
            enc.set_depth(png::BitDepth::Eight);
            enc.write_header().expect("h").write_image_data(&px).expect("d");
        }
        px
    };

    editor.doc.edit("Rig off", |s| s.lights_mut().enabled = false);
    let unlit = shot(&mut h, &mut cache, &editor, "unlit");
    editor.doc.edit("Rig on", |s| s.lights_mut().enabled = true);
    // What each level would cost, and whether the frame survives it. A frame
    // that never landed shows the one before it, so each is compared with a
    // known blank.
    for detail in [
        buzz_render::document::LightDetail::Flat,
        buzz_render::document::LightDetail::NoModelling,
        buzz_render::document::LightDetail::Full,
    ] {
        cache.pin_detail(Some(buzz_render::document::LightDetail::Flat));
        editor.doc.edit("Rig off", |s| s.lights_mut().enabled = false);
        let blank = h.stage(&editor, &mut cache);
        editor.doc.edit("Rig on", |s| s.lights_mut().enabled = true);
        cache.pin_detail(Some(detail));
        let px = h.stage(&editor, &mut cache);
        eprintln!(
            "  pinned {detail:?}: encode {} -> {}",
            cache.last_encode(),
            if px == blank { "FRAME LOST" } else { "rendered" }
        );
    }
    cache.pin_detail(None);
    let lit = shot(&mut h, &mut cache, &editor, "lit");
    eprintln!("the light moved {:.3}% of the stage", difference(&unlit, &lit) * 100.0);
    eprintln!("warmth unlit {:.2} lit {:.2}", warmth(&unlit), warmth(&lit));

    // **The report: switch it off and on again.**
    let off = shot(&mut h, &mut cache, &editor, "toggle-1-off-rig");
    editor.doc.edit("Rig off", |s| s.lights_mut().enabled = false);
    let off = shot(&mut h, &mut cache, &editor, "toggle-2-rig-off");
    editor.doc.edit("Rig on", |s| s.lights_mut().enabled = true);
    let back = shot(&mut h, &mut cache, &editor, "toggle-3-rig-back");
    eprintln!(
        "rig off changed {:.2}%, back on differs from the first lit frame by {:.2}%",
        difference(&lit, &off) * 100.0,
        difference(&lit, &back) * 100.0
    );

    // And the same for one light's own switch, and for its shadows.
    let id = editor.scene().lights().lights[0].id;
    editor.doc.edit("Light off", |s| {
        s.lights_mut().get_mut(id).expect("l").enabled = false;
    });
    let l_off = shot(&mut h, &mut cache, &editor, "toggle-4-light-off");
    editor.doc.edit("Light on", |s| {
        s.lights_mut().get_mut(id).expect("l").enabled = true;
    });
    let l_back = shot(&mut h, &mut cache, &editor, "toggle-5-light-back");
    eprintln!(
        "light off changed {:.2}%, back on differs by {:.2}%",
        difference(&lit, &l_off) * 100.0,
        difference(&lit, &l_back) * 100.0
    );

    editor.doc.edit("Shadows off", |s| {
        s.lights_mut().get_mut(id).expect("l").shadows = false;
    });
    let s_off = shot(&mut h, &mut cache, &editor, "toggle-6-shadows-off");
    editor.doc.edit("Shadows on", |s| {
        s.lights_mut().get_mut(id).expect("l").shadows = true;
    });
    let s_back = shot(&mut h, &mut cache, &editor, "toggle-7-shadows-back");
    eprintln!(
        "shadows off changed {:.2}%, back on differs by {:.2}%",
        difference(&lit, &s_off) * 100.0,
        difference(&lit, &s_back) * 100.0
    );

    // And it must keep working, not merely land once.
    let z = editor.camera.zoom;
    editor.camera.zoom = z * 0.5;
    let out = shot(&mut h, &mut cache, &editor, "lit-zoomed-out");
    eprintln!(
        "zooming out moved {:.3}% (a frame that never landed moves nothing)",
        difference(&lit, &out) * 100.0
    );
}

/// Not a test: at what output size does each lighting level stop fitting?
/// `BUZZ_DOC=<file.buzz>`. A frame that never rendered leaves the previous
/// pixels, so each measurement is taken against a known blank.
#[test]
#[ignore = "diagnostic"]
fn diagnose_the_ceiling_against_output_size() {
    use buzz_render::document::LightDetail;

    let Ok(path) = std::env::var("BUZZ_DOC") else {
        return;
    };
    let Ok(gpu) = GpuContext::new_blocking(&GpuPreference::Automatic) else {
        return;
    };
    let mut gpu = gpu;

    let doc = Document::open(std::path::Path::new(&path)).expect("open");
    let mut editor = Editor::new(doc);

    for (w, h) in [
        (512u32, 512u32),
        (1024, 1024),
        (1295, 855),
        (1600, 1000),
        (1920, 1200),
        (2080, 1300),
        (2560, 1440),
        (3840, 2160),
    ] {
        let texture = gpu.device.create_texture(&wgpu::TextureDescriptor {
            label: None,
            size: wgpu::Extent3d {
                width: w,
                height: h,
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
        // Row pitch must be a multiple of 256 bytes.
        let row = (w * 4).div_ceil(256) * 256;
        let readback = gpu.device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: (row * h) as u64,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });

        editor.camera.viewport = Size::new(w as f64, h as f64);
        let stage = editor.scene().stage().size;
        editor.camera.center = Point::new(stage.width / 2.0, stage.height / 2.0);
        editor.camera.zoom = (w as f64 / stage.width).min(h as f64 / stage.height) * 0.9;

        let mut shot = |editor: &Editor, cache: &mut DrawCache| -> (u32, Vec<u8>) {
            let mut vello = vello::Scene::new();
            buzz_app::stage::build_scene(
                &mut vello,
                editor,
                Rect::new(0.0, 0.0, w as f64, h as f64),
                1.0,
                cache,
            );
            gpu.render(&vello, &view, w, h, BACKGROUND).expect("render");
            let mut enc = gpu
                .device
                .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
            enc.copy_texture_to_buffer(
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
                        bytes_per_row: Some(row),
                        rows_per_image: Some(h),
                    },
                },
                wgpu::Extent3d {
                    width: w,
                    height: h,
                    depth_or_array_layers: 1,
                },
            );
            gpu.queue.submit([enc.finish()]);
            let slice = readback.slice(..);
            slice.map_async(wgpu::MapMode::Read, |r| r.expect("map"));
            gpu.device
                .poll(wgpu::PollType::Wait {
                    submission_index: None,
                    timeout: Some(std::time::Duration::from_secs(60)),
                })
                .expect("poll");
            let px = slice.get_mapped_range().to_vec();
            readback.unmap();
            (vello.encoding().n_path_segments, px)
        };

        eprint!("  {w}x{h}:");
        for detail in [LightDetail::Flat, LightDetail::NoModelling, LightDetail::Full] {
            let mut cache = DrawCache::default();
            cache.pin_detail(Some(LightDetail::Flat));
            editor.doc.edit("Off", |s| s.lights_mut().enabled = false);
            let (_, blank) = shot(&editor, &mut cache);
            editor.doc.edit("On", |s| s.lights_mut().enabled = true);
            cache.pin_detail(Some(detail));
            let (segments, px) = shot(&editor, &mut cache);
            let ceiling = buzz_render::document::segment_ceiling((w as f64) * (h as f64));
            eprint!(
                "  {detail:?} {segments} {} (ceiling {ceiling} says {})",
                if px == blank { "LOST" } else { "ok" },
                if segments <= ceiling { "ok" } else { "trim" }
            );
        }
        eprintln!();
    }
}

/// Not a test: what each layer costs the encode, and what the modelling would
/// cost on top. `BUZZ_DOC=<file.buzz>`.
#[test]
#[ignore = "diagnostic"]
fn diagnose_where_the_segments_go() {
    use buzz_render::document::LightDetail;

    let Ok(path) = std::env::var("BUZZ_DOC") else {
        return;
    };
    let doc = Document::open(std::path::Path::new(&path)).expect("open");
    let mut editor = Editor::new(doc);
    editor.camera.viewport = Size::new(1295.0, 855.0);
    let stage = editor.scene().stage().size;
    editor.camera.center = Point::new(stage.width / 2.0, stage.height / 2.0);
    editor.camera.zoom = (1295.0 / stage.width).min(855.0 / stage.height) * 0.9;

    let area = Rect::new(0.0, 0.0, 1295.0, 855.0);
    let measure = |editor: &Editor, detail: LightDetail| -> u32 {
        let mut cache = DrawCache::default();
        cache.pin_detail(Some(detail));
        let mut vello = vello::Scene::new();
        buzz_app::stage::build_scene(&mut vello, editor, area, 1.0, &mut cache);
        vello.encoding().n_path_segments
    };

    let ids: Vec<_> = editor.scene().layers().iter().map(|l| l.id).collect();
    let names: Vec<String> = editor
        .scene()
        .layers()
        .iter()
        .map(|l| l.name.clone())
        .collect();

    editor.doc.edit("Hide all", |s| {
        for id in &ids {
            s.update_layer(*id, |l| l.visible = false);
        }
    });
    let empty = measure(&editor, LightDetail::Flat);

    let mut rows: Vec<(String, u32, u32)> = Vec::new();
    for (id, name) in ids.iter().zip(&names) {
        editor.doc.edit("Hide all", |s| {
            for other in &ids {
                s.update_layer(*other, |l| l.visible = false);
            }
        });
        editor.doc.edit("Show one", |s| {
            s.update_layer(*id, |l| l.visible = true);
        });
        let flat = measure(&editor, LightDetail::Flat).saturating_sub(empty);
        let full = measure(&editor, LightDetail::Full).saturating_sub(empty);
        rows.push((name.clone(), flat, full));
    }
    editor.doc.edit("Show all", |s| {
        for id in &ids {
            s.update_layer(*id, |l| l.visible = true);
        }
    });

    rows.sort_by_key(|(_, flat, _)| std::cmp::Reverse(*flat));
    eprintln!("  {:<32} {:>10} {:>12}", "layer", "artwork", "with model");
    for (name, flat, full) in &rows {
        if *flat == 0 && *full == 0 {
            continue;
        }
        eprintln!("  {name:<32} {flat:>10} {full:>12}");
    }
    let art: u32 = rows.iter().map(|(_, f, _)| f).sum();
    let modelled: u32 = rows.iter().map(|(_, _, f)| f).sum();
    eprintln!("  {:<32} {art:>10} {modelled:>12}", "-- sum of layers --");
    eprintln!(
        "  whole stage: flat {} shadows {} modelled {} (ceiling {})",
        measure(&editor, LightDetail::Flat),
        measure(&editor, LightDetail::NoModelling),
        measure(&editor, LightDetail::Full),
        buzz_render::document::segment_ceiling(1295.0 * 855.0),
    );

    // Does culling bite? Zoomed right in, nearly everything is off-screen and
    // the encode should collapse. `encode_cost` gates exactly this.
    let fit = editor.camera.zoom;
    for factor in [1.0, 2.0, 4.0, 8.0, 16.0] {
        editor.camera.zoom = fit * factor;
        // And what the ladder actually settles on there, judged frame by frame
        // the way the window does.
        let mut cache = DrawCache::default();
        for _ in 0..4 {
            let mut vello = vello::Scene::new();
            buzz_app::stage::build_scene(&mut vello, &editor, area, 1.0, &mut cache);
        }
        eprintln!(
            "  zoom x{factor}: flat {} shadows {} modelled {} -> settles on {:?}",
            measure(&editor, LightDetail::Flat),
            measure(&editor, LightDetail::NoModelling),
            measure(&editor, LightDetail::Full),
            cache.detail(),
        );
    }
    editor.camera.zoom = fit;
}

/// Not a test: drive a real `.buzz` through the window's own frame loop and
/// switch its lighting off and on again. `BUZZ_DOC=<file.buzz>`.
#[test]
#[ignore = "diagnostic"]
fn diagnose_the_switch_through_the_window() {
    let Ok(path) = std::env::var("BUZZ_DOC") else {
        return;
    };
    let Some(mut h) = Harness::new() else { return };
    let doc = Document::open(std::path::Path::new(&path)).expect("open");
    let mut editor = Editor::new(doc);
    editor.camera.viewport = Size::new(W as f64, H as f64);
    let stage = editor.scene().stage().size;
    editor.camera.center = Point::new(stage.width / 2.0, stage.height / 2.0);
    editor.camera.zoom = (W as f64 / stage.width).min(H as f64 / stage.height) * 0.9;

    let mut window = WindowSim::new(editor);
    let lit = window.settle(&mut h);
    eprintln!("  lit:  detail {:?}", window.cache.detail());

    window
        .editor
        .doc
        .edit("Rig off", |s| s.lights_mut().enabled = false);
    let off = window.settle(&mut h);
    eprintln!(
        "  off:  detail {:?}  moved {:.2}%",
        window.cache.detail(),
        difference(&lit, &off) * 100.0
    );

    window
        .editor
        .doc
        .edit("Rig on", |s| s.lights_mut().enabled = true);
    let back = window.settle(&mut h);
    eprintln!(
        "  back: detail {:?}  differs from the first lit frame by {:.2}%",
        window.cache.detail(),
        difference(&lit, &back) * 100.0
    );

    // And the light's own switch.
    let id = window.editor.scene().lights().lights[0].id;
    window.editor.doc.edit("Light off", |s| {
        s.lights_mut().get_mut(id).expect("l").enabled = false;
    });
    let l_off = window.settle(&mut h);
    window.editor.doc.edit("Light on", |s| {
        s.lights_mut().get_mut(id).expect("l").enabled = true;
    });
    let l_back = window.settle(&mut h);
    eprintln!(
        "  light off moved {:.2}%, back on differs by {:.2}%",
        difference(&lit, &l_off) * 100.0,
        difference(&lit, &l_back) * 100.0
    );

    // And undo, which puts the document's revision back to a number the
    // retained encoding has already seen.
    window.editor.doc.edit("Rig off", |s| s.lights_mut().enabled = false);
    let _ = window.settle(&mut h);
    window.editor.doc.undo();
    let undone = window.settle(&mut h);
    eprintln!(
        "  undo of the switch differs from the first lit frame by {:.2}%",
        difference(&lit, &undone) * 100.0
    );
}
