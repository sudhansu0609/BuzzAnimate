//! Window, surface and frame loop.
//!
//! # Frame structure
//!
//! ```text
//! Vello  -> intermediate Rgba8Unorm texture   (compute; needs STORAGE_BINDING)
//! blit   -> surface texture                   (a surface cannot be a storage target)
//! egui   -> surface texture, LoadOp::Load     (overlay on top of the artwork)
//! ```
//!
//! The intermediate texture is not an optimisation to remove later: Vello's
//! fine rasteriser writes through a storage binding, which swapchain textures
//! generally do not allow.

use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use buzz_geom::{Camera, Point, Rect, Shape, Size, Vec2};
use buzz_jobs::{JobSystem, Pool, Snapshot, Utilisation};
use buzz_render::{GpuContext, GpuPreference};
// Always the wgpu that vello re-exports — never a second copy.
use buzz_render::wgpu;
use peniko::Color;
use vello::Scene;
use winit::application::ApplicationHandler;
use winit::event::{ElementState, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::ActiveEventLoop;
use winit::keyboard::Key;
use winit::window::{Window, WindowId};

use crate::demo::{CullStats, ZoomTarget};
use crate::hud::{self, HudActions, HudState};

/// Background of the stage.
const BACKGROUND: Color = Color::from_rgb8(0x14, 0x16, 0x1A);

/// How much one wheel notch multiplies the zoom.
const WHEEL_ZOOM_STEP: f64 = 1.18;

/// Utilisation is averaged over this window so the bars stay readable.
const UTILISATION_WINDOW: Duration = Duration::from_millis(250);

/// Margin used when framing the artwork.
const FIT_MARGIN: f64 = 1.15;

/// Live state once the window and GPU exist.
struct Active {
    window: Arc<Window>,
    surface: wgpu::Surface<'static>,
    surface_config: wgpu::SurfaceConfiguration,
    gpu: GpuContext,
    blitter: wgpu::util::TextureBlitter,
    /// Vello's compute target, rebuilt when the window resizes.
    target: Option<TargetTexture>,

    egui_ctx: egui::Context,
    egui_state: egui_winit::State,
    egui_renderer: egui_wgpu::Renderer,

    scene: Scene,
    camera: Camera,

    cursor: Point,
    panning: bool,

    last_frame: Instant,
    frame_ms: f32,
    util_since: Snapshot,
    util_at: Instant,
    utilisation: Option<Utilisation>,
    /// From the previous frame: the HUD is built before the scene is encoded.
    last_cull: CullStats,
}

/// Vello's render target and the size it was built for.
struct TargetTexture {
    #[allow(dead_code, reason = "kept alive so the view stays valid")]
    texture: wgpu::Texture,
    view: wgpu::TextureView,
    width: u32,
    height: u32,
}

/// The application.
pub struct App {
    active: Option<Active>,
    jobs: Arc<JobSystem>,
    /// `Arc` so a frame can hold it without borrowing `self`.
    artwork: Arc<ZoomTarget>,
    bounds: Rect,
    preference: GpuPreference,
}

impl App {
    pub fn new(preference: GpuPreference) -> Self {
        let artwork = Arc::new(ZoomTarget::default());
        let bounds = artwork
            .items
            .iter()
            .map(|i| i.path.bounding_box())
            .reduce(|a, b| a.union(b))
            .unwrap_or(Rect::new(0.0, 0.0, 1.0, 1.0));

        Self {
            active: None,
            jobs: Arc::new(JobSystem::new()),
            artwork,
            bounds,
            preference,
        }
    }

    fn init(&mut self, event_loop: &ActiveEventLoop) -> Result<Active> {
        let attrs = Window::default_attributes()
            .with_title("BuzzAnimate — Phase 0")
            .with_inner_size(winit::dpi::LogicalSize::new(1600.0, 950.0));
        let window = Arc::new(
            event_loop
                .create_window(attrs)
                .context("creating the window")?,
        );

        let gpu = GpuContext::new_blocking(&self.preference)?;
        println!("{}", gpu.selection.report());

        let surface = gpu
            .instance
            .create_surface(window.clone())
            .context("creating the surface")?;

        let caps = surface.get_capabilities(&gpu.selection.adapter);

        // Prefer a non-sRGB surface format. Vello writes colours that are
        // already sRGB-encoded into a UNORM texture; blitting those through an
        // sRGB view would apply the transfer function a second time and wash
        // the whole stage out.
        let format = caps
            .formats
            .iter()
            .copied()
            .find(|f| !f.is_srgb())
            .unwrap_or(caps.formats[0]);

        let size = window.inner_size();
        let surface_config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            width: size.width.max(1),
            height: size.height.max(1),
            present_mode: wgpu::PresentMode::AutoVsync,
            desired_maximum_frame_latency: 2,
            alpha_mode: caps.alpha_modes[0],
            view_formats: vec![],
        };
        surface.configure(&gpu.device, &surface_config);

        let blitter = wgpu::util::TextureBlitter::new(&gpu.device, format);

        let egui_ctx = egui::Context::default();
        let egui_state = egui_winit::State::new(
            egui_ctx.clone(),
            egui::ViewportId::ROOT,
            &window,
            None,
            None,
            None,
        );
        let egui_renderer =
            egui_wgpu::Renderer::new(&gpu.device, format, egui_wgpu::RendererOptions::default());

        let mut camera = Camera::new(
            Point::ORIGIN,
            1.0,
            Size::new(size.width as f64, size.height as f64),
        );
        camera.fit_to_rect(self.bounds, FIT_MARGIN);

        Ok(Active {
            window,
            surface,
            surface_config,
            gpu,
            blitter,
            target: None,
            egui_ctx,
            egui_state,
            egui_renderer,
            scene: Scene::new(),
            camera,
            cursor: Point::ORIGIN,
            panning: false,
            last_frame: Instant::now(),
            frame_ms: 0.0,
            util_since: self.jobs.sample(Pool::Interactive),
            util_at: Instant::now(),
            utilisation: None,
            last_cull: CullStats::default(),
        })
    }
}

impl Active {
    /// Rebuild Vello's target if the surface size changed.
    fn ensure_target(&mut self) {
        let (w, h) = (self.surface_config.width, self.surface_config.height);
        if matches!(&self.target, Some(t) if t.width == w && t.height == h) {
            return;
        }

        let texture = self.gpu.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("vello-target"),
            size: wgpu::Extent3d {
                width: w,
                height: h,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: buzz_render::RENDER_FORMAT,
            // STORAGE_BINDING is what forces the intermediate texture: Vello's
            // fine rasteriser writes through it, and surfaces cannot.
            usage: wgpu::TextureUsages::STORAGE_BINDING | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        self.target = Some(TargetTexture {
            texture,
            view,
            width: w,
            height: h,
        });
    }

    fn resize(&mut self, width: u32, height: u32) {
        if width == 0 || height == 0 {
            return;
        }
        self.surface_config.width = width;
        self.surface_config.height = height;
        self.surface
            .configure(&self.gpu.device, &self.surface_config);
        self.camera.viewport = Size::new(width as f64, height as f64);
    }

    fn refresh_metrics(&mut self, jobs: &JobSystem) {
        let now = Instant::now();
        self.frame_ms = now.duration_since(self.last_frame).as_secs_f32() * 1000.0;
        self.last_frame = now;

        if now.duration_since(self.util_at) >= UTILISATION_WINDOW {
            self.utilisation = Some(jobs.utilisation_since(Pool::Interactive, &self.util_since));
            self.util_since = jobs.sample(Pool::Interactive);
            self.util_at = now;
        }
    }

    fn apply(&mut self, actions: HudActions, artwork: &ZoomTarget, bounds: Rect, jobs: &Arc<JobSystem>) {
        if actions.reset_view || actions.fit_view {
            self.camera.fit_to_rect(bounds, FIT_MARGIN);
        }
        if let Some(pct) = actions.goto_zoom_percent {
            // Recentre so the artwork stays in frame when jumping levels.
            self.camera.center = artwork.center;
            self.camera.set_zoom_percent(pct as f64);
        }
        if actions.stress_cores {
            let jobs = Arc::clone(jobs);
            jobs.clone().spawn(Pool::Interactive, move || {
                use rayon::prelude::*;
                jobs.run(Pool::Interactive, || {
                    (0..2048u64)
                        .into_par_iter()
                        .map(|i| {
                            let mut acc = i | 1;
                            for k in 0..300_000u64 {
                                acc = acc.wrapping_mul(6364136223846793005).wrapping_add(k | 1);
                                acc ^= acc >> 17;
                            }
                            acc
                        })
                        .reduce(|| 0, |a, b| a.wrapping_add(b))
                });
            });
        }
    }

    fn render(&mut self, artwork: &ZoomTarget, jobs: &Arc<JobSystem>, bounds: Rect) -> Result<()> {
        self.refresh_metrics(jobs);

        // ---- HUD ----------------------------------------------------------
        let raw_input = self.egui_state.take_egui_input(&self.window);
        let state = HudState {
            adapter: self.gpu.selection.chosen.info.name.clone(),
            backend: format!("{:?}", self.gpu.selection.chosen.info.backend),
            frame_ms: self.frame_ms,
            interactive_threads: jobs.interactive_threads(),
            background_threads: jobs.background_threads(),
            utilisation: self.utilisation.clone(),
            generation: artwork.generation_at_zoom(self.camera.zoom),
            generations: artwork.generations,
            scene_items: artwork.items.len(),
            drawn: self.last_cull.drawn,
            culled: self.last_cull.culled(),
            visible_generations: self.last_cull.generation_range(),
        };

        let camera = self.camera;
        let egui_ctx = self.egui_ctx.clone();
        // `begin_pass`/`end_pass` rather than `run_ui`: the HUD is built from
        // `egui::Window`s, which attach to a `Context`, not to a root `Ui`.
        egui_ctx.begin_pass(raw_input);
        let actions = hud::draw(&egui_ctx, &camera, &state);
        let output = egui_ctx.end_pass();

        self.apply(actions, artwork, bounds, jobs);
        self.egui_state
            .handle_platform_output(&self.window, output.platform_output);
        let paint_jobs = self
            .egui_ctx
            .tessellate(output.shapes, output.pixels_per_point);

        // ---- Vector artwork -------------------------------------------------
        // Shared with the headless zoom test, so what is verified offscreen is
        // exactly what the window draws.
        self.last_cull = artwork.encode(&mut self.scene, &self.camera);

        use wgpu::CurrentSurfaceTexture as Cst;
        let frame = match self.surface.get_current_texture() {
            // Suboptimal still presents correctly; reconfiguring mid-resize
            // would just cause flicker.
            Cst::Success(f) | Cst::Suboptimal(f) => f,
            Cst::Outdated | Cst::Lost => {
                let (w, h) = (self.surface_config.width, self.surface_config.height);
                self.resize(w, h);
                return Ok(());
            }
            // Transient: skip this frame and try again on the next one.
            Cst::Timeout | Cst::Occluded => return Ok(()),
            other => {
                return Err(anyhow::anyhow!("acquiring surface texture: {other:?}"));
            }
        };
        let surface_view = frame
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());

        let (w, h) = (self.surface_config.width, self.surface_config.height);
        self.ensure_target();

        // Disjoint field borrows: `gpu` mutably, `target` and `scene` shared.
        let target = &self.target.as_ref().expect("ensure_target ran").view;
        self.gpu.render(&self.scene, target, w, h, BACKGROUND)?;

        let mut encoder =
            self.gpu
                .device
                .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                    label: Some("buzz-frame"),
                });

        self.blitter
            .copy(&self.gpu.device, &mut encoder, target, &surface_view);

        // ---- egui overlay ----------------------------------------------------
        for (id, delta) in &output.textures_delta.set {
            self.egui_renderer
                .update_texture(&self.gpu.device, &self.gpu.queue, *id, delta);
        }

        let screen = egui_wgpu::ScreenDescriptor {
            size_in_pixels: [w, h],
            pixels_per_point: output.pixels_per_point,
        };
        let user_buffers = self.egui_renderer.update_buffers(
            &self.gpu.device,
            &self.gpu.queue,
            &mut encoder,
            &paint_jobs,
            &screen,
        );

        {
            let pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("egui"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &surface_view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        // Load, not Clear: the artwork is already there.
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            self.egui_renderer
                .render(&mut pass.forget_lifetime(), &paint_jobs, &screen);
        }

        self.gpu
            .queue
            .submit(user_buffers.into_iter().chain([encoder.finish()]));
        frame.present();

        for id in &output.textures_delta.free {
            self.egui_renderer.free_texture(id);
        }

        Ok(())
    }
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.active.is_some() {
            return;
        }
        match self.init(event_loop) {
            Ok(active) => self.active = Some(active),
            Err(e) => {
                eprintln!("BuzzAnimate failed to start: {e:?}");
                event_loop.exit();
            }
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        let bounds = self.bounds;
        let Some(active) = self.active.as_mut() else {
            return;
        };

        // egui gets first refusal on input so the HUD stays clickable.
        let response = active.egui_state.on_window_event(&active.window, &event);
        let egui_has_pointer = response.consumed;
        if response.repaint {
            active.window.request_redraw();
        }

        match event {
            WindowEvent::CloseRequested => event_loop.exit(),

            WindowEvent::Resized(size) => {
                active.resize(size.width, size.height);
                active.window.request_redraw();
            }

            WindowEvent::CursorMoved { position, .. } => {
                let new = Point::new(position.x, position.y);
                if active.panning {
                    let d = new - active.cursor;
                    active.camera.pan_screen(Vec2::new(d.x, d.y));
                    active.window.request_redraw();
                }
                active.cursor = new;
            }

            WindowEvent::MouseInput { state, button, .. } => {
                if matches!(button, MouseButton::Middle | MouseButton::Left) && !egui_has_pointer {
                    active.panning = state == ElementState::Pressed;
                }
            }

            WindowEvent::MouseWheel { delta, .. } if !egui_has_pointer => {
                let notches = match delta {
                    MouseScrollDelta::LineDelta(_, y) => y as f64,
                    MouseScrollDelta::PixelDelta(p) => p.y / 60.0,
                };
                if notches != 0.0 {
                    // Unbounded in both directions — no 2000% ceiling here.
                    active
                        .camera
                        .zoom_by_at(WHEEL_ZOOM_STEP.powf(notches), active.cursor);
                    active.window.request_redraw();
                }
            }

            WindowEvent::KeyboardInput { event, .. } if event.state == ElementState::Pressed => {
                match event.logical_key.as_ref() {
                    Key::Named(winit::keyboard::NamedKey::Escape) => event_loop.exit(),
                    Key::Character("r" | "R") => active.camera.fit_to_rect(bounds, FIT_MARGIN),
                    _ => {}
                }
                active.window.request_redraw();
            }

            WindowEvent::RedrawRequested => {
                let artwork = Arc::clone(&self.artwork);
                let jobs = Arc::clone(&self.jobs);
                if let Err(e) = active.render(&artwork, &jobs, bounds) {
                    eprintln!("frame failed: {e:?}");
                }
            }

            _ => {}
        }
    }

    fn about_to_wait(&mut self, _event_loop: &ActiveEventLoop) {
        // Redraw continuously: the HUD shows live CPU utilisation.
        if let Some(active) = &self.active {
            active.window.request_redraw();
        }
    }
}
