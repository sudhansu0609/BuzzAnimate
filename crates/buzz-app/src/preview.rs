//! **See the render before it is written.**
//!
//! # The gap this closes
//!
//! Every setting in the Export dialog changes what comes out, and until this
//! existed there was exactly one way to find out what: export it. On a
//! fifteen-second film that is twenty seconds; on a long one it is a coffee
//! break, and the mistakes it catches are the cheap ones — the wrong range, the
//! wrong size, a guide layer somebody forgot to hide, a light left switched
//! off. Finding those *after* the encode is the whole of what makes exporting
//! feel like a commitment rather than a step.
//!
//! So: a strip of frames spread across the range being exported, rendered
//! **through the export's own pipeline** and shown in the dialog. Not the
//! stage's view of the document — the stage lights differently, shows guides,
//! honours layer transparency and draws the onion skin. The whole value of a
//! test render is that it is the same picture the file will hold, so it goes
//! through the same `FrameOptions` the exporter uses and reads the same reel.
//!
//! # Why the frames are spread rather than the first few
//!
//! The first frames of a film are the least informative part of it: the camera
//! has not moved, nobody has walked anywhere, and a cut has not happened. Six
//! frames spread evenly across the range cross every scene in a multi-scene
//! film, which is where the mistakes actually are.

use buzz_render::wgpu;
use buzz_scene::Scene;

/// How many frames a preview draws.
///
/// Six is enough to cross three shots and see a camera move; more is a grid
/// nobody reads and a wait long enough that a person would rather just export.
pub const SHOTS: usize = 6;

/// The longest edge of a preview picture, in pixels.
const SIZE: u32 = 320;

/// One rendered frame, and the texture the dialog draws it with.
pub struct Shot {
    /// Which frame of the *film* this is.
    pub frame: u32,
    pub id: egui::TextureId,
    #[allow(dead_code, reason = "held so the view stays alive")]
    texture: wgpu::Texture,
    #[allow(dead_code, reason = "held so the registered texture stays alive")]
    view: wgpu::TextureView,
    width: u32,
    height: u32,
}

impl Shot {
    /// How large the picture is, for the dialog to lay it out.
    pub fn size(&self) -> egui::Vec2 {
        egui::vec2(self.width as f32, self.height as f32)
    }
}

/// What to render, handed over when the user asks for it.
pub struct Request {
    pub scenes: Vec<Scene>,
    pub range: std::ops::Range<u32>,
    /// The size the export is going out at, so the preview has its aspect.
    pub width: u32,
    pub height: u32,
}

/// The test-render strip: what has been rendered, and what has been asked for.
#[derive(Default)]
pub struct Preview {
    shots: Vec<Shot>,
    wanted: Option<Request>,
    /// Textures to free once the renderer is reachable again.
    retired: Vec<egui::TextureId>,
    /// What the strip is showing, for the line under it.
    pub message: Option<String>,
}

impl Preview {
    /// Ask for a strip. Replaces whatever was there.
    pub fn request(&mut self, request: Request) {
        self.wanted = Some(request);
        self.message = Some("Rendering\u{2026}".to_string());
    }

    /// Is a render waiting to happen? The shell raises its frame rate for one.
    pub fn pending(&self) -> bool {
        self.wanted.is_some()
    }

    /// The frames, for the dialog to draw.
    pub fn shots(&self) -> &[Shot] {
        &self.shots
    }

    /// Throw the strip away — the document changed under it, or the dialog
    /// closed.
    pub fn clear(&mut self) {
        for shot in self.shots.drain(..) {
            self.retired.push(shot.id);
        }
        self.wanted = None;
        self.message = None;
    }

    /// **Render whatever was asked for**, once per frame, where the GPU is.
    ///
    /// The whole strip in one go rather than one per frame: six small pictures
    /// is a few milliseconds, and a strip that filled in one frame at a time
    /// would flicker its way into existence while the user watched.
    pub fn fulfil(
        &mut self,
        gpu: &mut buzz_render::GpuContext,
        egui_renderer: &mut egui_wgpu::Renderer,
        vello: &mut vello::Scene,
        cache: &mut buzz_render::document::DrawCache,
    ) {
        for id in self.retired.drain(..) {
            egui_renderer.free_texture(&id);
        }
        let Some(request) = self.wanted.take() else {
            return;
        };
        for shot in self.shots.drain(..) {
            egui_renderer.free_texture(&shot.id);
        }

        let reel = buzz_export::Reel::of(request.scenes.iter());
        if reel.is_empty() || request.range.is_empty() {
            self.message = Some("There is nothing in that range to render".into());
            return;
        }

        // The preview's own size, keeping the export's aspect. The long edge
        // is fixed so a square film and a Shorts film both fit the dialog.
        let aspect = request.width.max(1) as f64 / request.height.max(1) as f64;
        let (width, height) = if aspect >= 1.0 {
            (SIZE, ((SIZE as f64 / aspect).round() as u32).max(1))
        } else {
            (((SIZE as f64 * aspect).round() as u32).max(1), SIZE)
        };

        let last = request.range.end.saturating_sub(1);
        let span = last.saturating_sub(request.range.start);
        for i in 0..SHOTS {
            // Spread across the range, ends included: the first frame and the
            // last are the two most worth checking.
            let at = if SHOTS <= 1 {
                request.range.start
            } else {
                request.range.start + (span as u64 * i as u64 / (SHOTS as u64 - 1)) as u32
            };
            let Some((scene, local)) = reel.at_clamped(at) else {
                continue;
            };
            if let Some(shot) = self.draw(gpu, egui_renderer, vello, scene, local, at, width, height, cache)
            {
                self.shots.push(shot);
            }
        }

        self.message = Some(format!(
            "{} frame(s) of {}, at {}\u{00D7}{}",
            self.shots.len(),
            request.range.end - request.range.start,
            request.width,
            request.height,
        ));
    }

    /// One frame, through the export's pipeline.
    #[allow(clippy::too_many_arguments, reason = "a render takes what it takes")]
    fn draw(
        &mut self,
        gpu: &mut buzz_render::GpuContext,
        egui_renderer: &mut egui_wgpu::Renderer,
        vello: &mut vello::Scene,
        scene: &Scene,
        frame: u32,
        film_frame: u32,
        width: u32,
        height: u32,
        cache: &mut buzz_render::document::DrawCache,
    ) -> Option<Shot> {
        // **The stage framed exactly**, as the exporter frames it: the stage
        // centre in the middle of the picture, and one document unit to `zoom`
        // pixels. Not a fitted camera with a margin -- a preview with a border
        // the film will not have is a preview of a different picture.
        let stage = scene.stage().stage_rect();
        let camera = buzz_geom::Camera::new(
            stage.center(),
            width as f64 / stage.width().max(1e-6),
            buzz_geom::Size::new(width as f64, height as f64),
        );

        vello.reset();
        let mut builder = buzz_render::SceneBuilder::new(vello, &camera);
        // The stage's own colour under it, because an export is not
        // transparent unless it was asked to be.
        builder.fill_shape(&stage, scene.stage().background);
        buzz_render::document::draw_frame_cached(
            &mut builder,
            scene,
            frame,
            // The camera moves within the frame too, so it is asked for the
            // same instant the artwork is.
            scene.camera_transform(frame),
            // **The export's options, not the stage's.** The whole value of a
            // test render is that it is the picture the file will hold: lit,
            // masks clipping, and no authoring aid anywhere in it.
            &buzz_render::document::FrameOptions {
                lit: true,
                guides: false,
                masks: buzz_render::document::MaskDisplay::Always,
                ..buzz_render::document::FrameOptions::default()
            },
            cache,
        );
        drop(builder);

        let texture = gpu.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("export preview"),
            size: wgpu::Extent3d {
                width,
                height,
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
        gpu.render(vello, &view, width, height, scene.stage().background)
            .ok()?;

        let id =
            egui_renderer.register_native_texture(&gpu.device, &view, wgpu::FilterMode::Linear);
        Some(Shot {
            frame: film_frame,
            id,
            texture,
            view,
            width,
            height,
        })
    }
}
