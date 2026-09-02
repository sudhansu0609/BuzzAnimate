//! The full-frame compositor: a raw-wgpu post-process chain.
//!
//! # Why this can exist at all
//!
//! `buzz-light` and `buzz-fx` build their effects as *geometry* because Vello
//! offers no shader hook inside its scene — a shaded crescent is a boolean, a
//! blur is a stack of bands. That argument is about **per-shape** effects, and
//! it does not reach here. The window renders Vello into a texture the
//! application owns and then blits that texture to the screen; that seam is
//! ours, and a full-frame pass slots into it. Bloom, grade, vignette and grain
//! are not vector operations, so they are done the way a compositor does them —
//! sampling the finished frame on the GPU.
//!
//! # One implementation, two devices
//!
//! The stage and the exporter both call [`Compositor::run`], from this one
//! crate, on their two different GPU devices. There is no second copy of the
//! grade maths to drift, which is the same parity guarantee `document.rs`
//! gives for the artwork walk.
//!
//! # The chain
//!
//! ```text
//! input ──▶ bright-pass ──▶ [Kawase blur]×N ──▶ bloom (half-res)
//!   │                                              │
//!   └────────────────── composite ◀───────────────┘ ──▶ output
//! ```
//!
//! The composite pass reads the artwork with `textureLoad` at integer pixel
//! coordinates, so with every effect off it returns the source byte-for-byte.
//! A disabled compositor is therefore an exact copy, which is what lets the
//! window skip it entirely and blit instead — and what the passthrough test
//! pins down.
//!
//! # A note against the design
//!
//! `ARCHITECTURE.md` specifies a *dual-Kawase* pyramid (down/up across several
//! resolutions). This ships the simpler **single half-resolution Kawase
//! ping-pong**: one bloom buffer, a handful of growing-offset blur steps. It
//! blooms softly and widely, is far less machinery to get right on the first
//! pass, and keeps the public API — `new`/`resize`/`run` — exactly as designed,
//! so a later upgrade to the pyramid changes only this file's internals.

use buzz_scene::PostSettings;
use peniko::Color;

/// The bloom buffer runs at half the frame's resolution: cheaper, and a wide
/// soft bloom does not need full detail.
const BLOOM_DIVISOR: u32 = 2;

/// Most Kawase steps we will ever run, and the size of the blur uniform ring.
const MAX_BLUR_STEPS: usize = 8;

/// Uniform dynamic offsets must clear the device's alignment; 256 covers every
/// desktop backend and is what wgpu reports as the common floor.
const BLUR_STRIDE: u64 = 256;

/// Flag bits shared with `compositor.wgsl`.
mod flag {
    pub const BLOOM: u32 = 1;
    pub const GRADE: u32 = 2;
    pub const VIGNETTE: u32 = 4;
    pub const GRAIN: u32 = 8;
    pub const POSTERISE: u32 = 16;
    pub const HALFTONE: u32 = 32;
    pub const HATCHING: u32 = 64;
}

/// Half-resolution bloom targets and their current size.
struct BloomTargets {
    a: wgpu::TextureView,
    b: wgpu::TextureView,
    /// Blur reading `a`, writing wherever the pass is pointed. Dynamic-offset
    /// uniform.
    blur_from_a: wgpu::BindGroup,
    blur_from_b: wgpu::BindGroup,
    width: u32,
    height: u32,
}

/// The post-process chain, owning its pipelines and intermediate targets.
pub struct Compositor {
    sampler: wgpu::Sampler,

    bright_pipeline: wgpu::RenderPipeline,
    blur_pipeline: wgpu::RenderPipeline,
    composite_pipeline: wgpu::RenderPipeline,

    io_layout: wgpu::BindGroupLayout,      // {tex, sampler, uniform}
    blur_layout: wgpu::BindGroupLayout,    // {tex, sampler, dynamic uniform}
    composite_layout: wgpu::BindGroupLayout, // {tex, tex, sampler, uniform}

    bright_uniform: wgpu::Buffer,
    blur_uniform: wgpu::Buffer,
    composite_uniform: wgpu::Buffer,

    bloom: Option<BloomTargets>,
}

impl Compositor {
    /// Build the pipelines. `format` is the format the composite pass writes —
    /// the surface format on the window, the export target's format offline.
    pub fn new(device: &wgpu::Device, format: wgpu::TextureFormat) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("compositor"),
            source: wgpu::ShaderSource::Wgsl(include_str!("compositor.wgsl").into()),
        });

        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("compositor-sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Nearest,
            ..Default::default()
        });

        let tex_entry = |binding: u32| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: true },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        };
        let sampler_entry = |binding: u32| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
            count: None,
        };
        let uniform_entry = |binding: u32, dynamic: bool| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: dynamic,
                min_binding_size: None,
            },
            count: None,
        };

        let io_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("compositor-io"),
            entries: &[tex_entry(0), sampler_entry(1), uniform_entry(2, false)],
        });
        let blur_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("compositor-blur"),
            entries: &[tex_entry(0), sampler_entry(1), uniform_entry(2, true)],
        });
        let composite_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("compositor-composite"),
            entries: &[
                tex_entry(0),
                tex_entry(1),
                sampler_entry(2),
                uniform_entry(3, false),
            ],
        });

        let bloom_format = wgpu::TextureFormat::Rgba16Float;
        let bright_pipeline = Self::pipeline(
            device,
            &shader,
            &io_layout,
            "fs_bright",
            bloom_format,
            "compositor-bright",
        );
        let blur_pipeline = Self::pipeline(
            device,
            &shader,
            &blur_layout,
            "fs_blur",
            bloom_format,
            "compositor-blur",
        );
        let composite_pipeline = Self::pipeline(
            device,
            &shader,
            &composite_layout,
            "fs_composite",
            format,
            "compositor-composite",
        );

        let bright_uniform = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("compositor-bright-u"),
            size: 16,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let blur_uniform = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("compositor-blur-u"),
            size: BLUR_STRIDE * MAX_BLUR_STEPS as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let composite_uniform = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("compositor-composite-u"),
            size: 96,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        Self {
            sampler,
            bright_pipeline,
            blur_pipeline,
            composite_pipeline,
            io_layout,
            blur_layout,
            composite_layout,
            bright_uniform,
            blur_uniform,
            composite_uniform,
            bloom: None,
        }
    }

    fn pipeline(
        device: &wgpu::Device,
        shader: &wgpu::ShaderModule,
        layout: &wgpu::BindGroupLayout,
        fs_entry: &str,
        format: wgpu::TextureFormat,
        label: &str,
    ) -> wgpu::RenderPipeline {
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some(label),
            bind_group_layouts: &[Some(layout)],
            immediate_size: 0,
        });
        device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some(label),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: shader,
                entry_point: Some("vs_fullscreen"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: shader,
                entry_point: Some(fs_entry),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
        })
    }

    /// Size (or resize) the bloom targets to the current frame. Cheap and
    /// idempotent when the size has not changed.
    pub fn resize(&mut self, device: &wgpu::Device, width: u32, height: u32) {
        let bw = (width / BLOOM_DIVISOR).max(1);
        let bh = (height / BLOOM_DIVISOR).max(1);
        if matches!(&self.bloom, Some(b) if b.width == bw && b.height == bh) {
            return;
        }

        let make = |label: &str| {
            let tex = device.create_texture(&wgpu::TextureDescriptor {
                label: Some(label),
                size: wgpu::Extent3d {
                    width: bw,
                    height: bh,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba16Float,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                    | wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            });
            tex.create_view(&wgpu::TextureViewDescriptor::default())
        };
        let a = make("compositor-bloom-a");
        let b = make("compositor-bloom-b");

        let blur_bg = |src: &wgpu::TextureView, label: &str| {
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some(label),
                layout: &self.blur_layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(src),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::Sampler(&self.sampler),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                            buffer: &self.blur_uniform,
                            offset: 0,
                            size: std::num::NonZeroU64::new(16),
                        }),
                    },
                ],
            })
        };
        let blur_from_a = blur_bg(&a, "compositor-blur-from-a");
        let blur_from_b = blur_bg(&b, "compositor-blur-from-b");

        self.bloom = Some(BloomTargets {
            a,
            b,
            blur_from_a,
            blur_from_b,
            width: bw,
            height: bh,
        });
    }

    /// Run the chain: `input` (the rendered artwork) to `output` (surface or
    /// export target), applying `post`. `frame_index` seeds the grain, so it
    /// must be the document frame for a reproducible export.
    ///
    /// The caller keeps `run` off the hot path when it is pointless: with
    /// [`PostSettings::is_identity`] the window blits directly instead. Called
    /// anyway, an identity `post` still produces an exact copy.
    #[allow(clippy::too_many_arguments)]
    pub fn run(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        input: &wgpu::TextureView,
        output: &wgpu::TextureView,
        width: u32,
        height: u32,
        post: &PostSettings,
        frame_index: u32,
    ) {
        self.resize(device, width, height);
        let bloom = self.bloom.as_ref().expect("resize populates bloom");

        let bloom_on = post.enabled && post.bloom.enabled && post.bloom.intensity > 0.0;

        // ---- bloom -----------------------------------------------------------
        // `final_bloom` names the target the blur chain left the result in; when
        // bloom is off it stays `a` (whatever it holds), and the composite pass
        // ignores it via the flag.
        let mut final_bloom = &bloom.a;
        if bloom_on {
            queue.write_buffer(
                &self.bright_uniform,
                0,
                &pack_bright(post.bloom.threshold),
            );

            // Bright-pass: input -> a.
            let bright_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("compositor-bright-bg"),
                layout: &self.io_layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(input),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::Sampler(&self.sampler),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: self.bright_uniform.as_entire_binding(),
                    },
                ],
            });
            Self::draw(encoder, &self.bright_pipeline, &bloom.a, &bright_bg, None);

            // Kawase ping-pong. Step i reads whichever target holds the current
            // result and writes the other.
            let steps = blur_steps(post.bloom.radius);
            let texel = [1.0 / bloom.width as f32, 1.0 / bloom.height as f32];
            for i in 0..steps {
                let offset = 0.5 + i as f32;
                queue.write_buffer(
                    &self.blur_uniform,
                    i as u64 * BLUR_STRIDE,
                    &pack_blur(texel, offset),
                );
                let reads_a = i % 2 == 0;
                let (src_bg, dest) = if reads_a {
                    (&bloom.blur_from_a, &bloom.b)
                } else {
                    (&bloom.blur_from_b, &bloom.a)
                };
                Self::draw(
                    encoder,
                    &self.blur_pipeline,
                    dest,
                    src_bg,
                    Some(i as u32 * BLUR_STRIDE as u32),
                );
                final_bloom = if reads_a { &bloom.b } else { &bloom.a };
            }
        }

        // ---- composite -------------------------------------------------------
        queue.write_buffer(
            &self.composite_uniform,
            0,
            &pack_composite(post, width, height, frame_index, bloom_on),
        );
        let composite_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("compositor-composite-bg"),
            layout: &self.composite_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(input),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(final_bloom),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: self.composite_uniform.as_entire_binding(),
                },
            ],
        });
        Self::draw(
            encoder,
            &self.composite_pipeline,
            output,
            &composite_bg,
            None,
        );
    }

    fn draw(
        encoder: &mut wgpu::CommandEncoder,
        pipeline: &wgpu::RenderPipeline,
        target: &wgpu::TextureView,
        bind_group: &wgpu::BindGroup,
        dynamic_offset: Option<u32>,
    ) {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("compositor-pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: target,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        pass.set_pipeline(pipeline);
        match dynamic_offset {
            Some(off) => pass.set_bind_group(0, bind_group, &[off]),
            None => pass.set_bind_group(0, bind_group, &[]),
        }
        pass.draw(0..3, 0..1);
    }
}

/// Kawase steps from the 0..1 radius: at least one, at most [`MAX_BLUR_STEPS`].
fn blur_steps(radius: f32) -> usize {
    let r = radius.clamp(0.0, 1.0);
    (1.0 + r * (MAX_BLUR_STEPS as f32 - 1.0)).round() as usize
}

fn pack_bright(threshold: f32) -> [u8; 16] {
    let mut b = [0u8; 16];
    b[0..4].copy_from_slice(&threshold.to_le_bytes());
    b
}

fn pack_blur(texel: [f32; 2], offset: f32) -> [u8; 16] {
    let mut b = [0u8; 16];
    b[0..4].copy_from_slice(&texel[0].to_le_bytes());
    b[4..8].copy_from_slice(&texel[1].to_le_bytes());
    b[8..12].copy_from_slice(&offset.to_le_bytes());
    b
}

/// The composite uniform, laid out to match `CompositeU` in the shader — six
/// 16-byte rows, with the `vec3` colour on its own aligned row.
fn pack_composite(
    post: &PostSettings,
    width: u32,
    height: u32,
    frame_index: u32,
    bloom_on: bool,
) -> [u8; 96] {
    let mut flags = 0u32;
    if bloom_on {
        flags |= flag::BLOOM;
    }
    if post.enabled && !post.grade.is_neutral() {
        flags |= flag::GRADE;
    }
    if post.enabled && post.vignette.enabled && post.vignette.amount > 0.0 {
        flags |= flag::VIGNETTE;
    }
    if post.enabled && post.grain.enabled && post.grain.amount > 0.0 {
        flags |= flag::GRAIN;
    }
    if post.enabled && post.posterise.enabled {
        flags |= flag::POSTERISE;
    }
    if post.enabled && post.halftone.enabled {
        flags |= flag::HALFTONE;
    }
    if post.enabled && post.hatching.enabled {
        flags |= flag::HATCHING;
    }

    let g = &post.grade;
    let v = &post.vignette;
    let gr = &post.grain;
    let [vr, vg, vb, _] = v.color.to_rgba8().to_u8_array();

    // Every field written at its exact byte offset, matching `CompositeU`'s
    // six 16-byte rows in the shader.
    let floats: [(usize, f32); 21] = [
        // row 0: resolution.xy (frame_index and flags are u32, written below)
        (0, width as f32),
        (4, height as f32),
        // row 1
        (16, post.bloom.intensity),
        (20, g.exposure),
        (24, g.contrast),
        (28, g.saturation),
        // row 2
        (32, g.temperature),
        (36, g.tint),
        (40, g.lift),
        (44, g.gamma),
        // row 3
        (48, g.gain),
        (52, v.amount),
        (56, v.softness),
        (60, gr.amount),
        // row 4
        (64, gr.size),
        // Two former padding slots now carry the stylise passes' one parameter
        // each; the shader names them poster_levels and halftone_scale.
        (68, post.posterise.levels.max(2) as f32),
        (72, post.halftone.scale.max(2.0)),
        (76, post.hatching.scale.max(2.0)),
        // row 5: vignette colour (vec3) on its own 16-byte-aligned row
        (80, vr as f32 / 255.0),
        (84, vg as f32 / 255.0),
        (88, vb as f32 / 255.0),
    ];

    let mut b = [0u8; 96];
    for (off, x) in floats {
        b[off..off + 4].copy_from_slice(&x.to_le_bytes());
    }
    // The two u32 fields share row 0 with the resolution.
    b[8..12].copy_from_slice(&frame_index.to_le_bytes());
    b[12..16].copy_from_slice(&flags.to_le_bytes());
    b
}

/// Kept for callers that want the compositor's neutral vignette colour.
pub const DEFAULT_VIGNETTE_COLOR: Color = Color::BLACK;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blur_steps_span_the_radius() {
        assert_eq!(blur_steps(0.0), 1);
        assert_eq!(blur_steps(1.0), MAX_BLUR_STEPS);
        assert!(blur_steps(0.5) > 1 && blur_steps(0.5) < MAX_BLUR_STEPS);
    }

    #[test]
    fn packing_sets_flags_only_for_active_passes() {
        let mut post = PostSettings::default();
        // Disabled master: nothing lights up even with a pass "on".
        post.vignette.enabled = true;
        let bytes = pack_composite(&post, 100, 100, 0, false);
        let flags = u32::from_le_bytes([bytes[12], bytes[13], bytes[14], bytes[15]]);
        assert_eq!(flags, 0, "master off must clear every flag");

        post.enabled = true;
        let bytes = pack_composite(&post, 100, 100, 0, false);
        let flags = u32::from_le_bytes([bytes[12], bytes[13], bytes[14], bytes[15]]);
        assert_eq!(flags, flag::VIGNETTE);
    }
}
