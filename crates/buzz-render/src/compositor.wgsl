// The full-frame compositor: bloom, colour grade, vignette and grain.
//
// Three fragment programs share one full-screen-triangle vertex shader:
//
//   fs_bright   — the bloom bright-pass, into a half-resolution target
//   fs_blur     — a Kawase blur step, ping-ponged over the bloom target
//   fs_composite — the fused final pass: bloom add, grade, vignette, grain
//
// The composite pass reads the artwork with `textureLoad` at integer pixel
// coordinates, so with every effect off it returns the source byte-for-byte —
// the passthrough the disabled path relies on.

// ---------------------------------------------------------------------------
// Shared vertex shader
// ---------------------------------------------------------------------------

struct VsOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

// A single triangle that covers the whole clip rectangle. uv is 0..1 with the
// origin at the top-left, matching texture space.
@vertex
fn vs_fullscreen(@builtin(vertex_index) vi: u32) -> VsOut {
    var out: VsOut;
    let x = f32((vi << 1u) & 2u);
    let y = f32(vi & 2u);
    out.uv = vec2<f32>(x, y);
    out.pos = vec4<f32>(x * 2.0 - 1.0, 1.0 - y * 2.0, 0.0, 1.0);
    return out;
}

// ---------------------------------------------------------------------------
// Bright-pass
// ---------------------------------------------------------------------------

struct BrightU {
    threshold: f32,
    _pad0: f32,
    _pad1: f32,
    _pad2: f32,
};

@group(0) @binding(0) var src_tex: texture_2d<f32>;
@group(0) @binding(1) var src_samp: sampler;
@group(0) @binding(2) var<uniform> bright: BrightU;

fn luma(c: vec3<f32>) -> f32 {
    return dot(c, vec3<f32>(0.2126, 0.7152, 0.0722));
}

@fragment
fn fs_bright(in: VsOut) -> @location(0) vec4<f32> {
    let c = textureSample(src_tex, src_samp, in.uv).rgb;
    let l = luma(c);
    if (l <= bright.threshold) {
        return vec4<f32>(0.0, 0.0, 0.0, 1.0);
    }
    // Keep the colour, scaled by how far above the threshold it sits, so a
    // bright red highlight blooms red rather than white.
    let k = (l - bright.threshold) / max(l, 1e-4);
    return vec4<f32>(c * k, 1.0);
}

// ---------------------------------------------------------------------------
// Kawase blur step
// ---------------------------------------------------------------------------

struct BlurU {
    texel: vec2<f32>,
    offset: f32,
    _pad: f32,
};

@group(0) @binding(0) var blur_tex: texture_2d<f32>;
@group(0) @binding(1) var blur_samp: sampler;
@group(0) @binding(2) var<uniform> blur: BlurU;

@fragment
fn fs_blur(in: VsOut) -> @location(0) vec4<f32> {
    let o = blur.texel * blur.offset;
    var sum = textureSample(blur_tex, blur_samp, in.uv + vec2<f32>(o.x, o.y));
    sum += textureSample(blur_tex, blur_samp, in.uv + vec2<f32>(-o.x, o.y));
    sum += textureSample(blur_tex, blur_samp, in.uv + vec2<f32>(o.x, -o.y));
    sum += textureSample(blur_tex, blur_samp, in.uv + vec2<f32>(-o.x, -o.y));
    return sum * 0.25;
}

// ---------------------------------------------------------------------------
// Composite — the fused final pass
// ---------------------------------------------------------------------------

// Flag bits, matching the Rust side.
const FLAG_BLOOM: u32 = 1u;
const FLAG_GRADE: u32 = 2u;
const FLAG_VIGNETTE: u32 = 4u;
const FLAG_GRAIN: u32 = 8u;

struct CompositeU {
    resolution: vec2<f32>,
    frame_index: u32,
    flags: u32,

    bloom_intensity: f32,
    exposure: f32,
    contrast: f32,
    saturation: f32,

    temperature: f32,
    tint: f32,
    lift: f32,
    gamma: f32,

    gain: f32,
    vignette_amount: f32,
    vignette_softness: f32,
    grain_amount: f32,

    grain_size: f32,
    _pad0: f32,
    _pad1: f32,
    _pad2: f32,

    vignette_color: vec3<f32>,
    _pad3: f32,
};

@group(0) @binding(0) var comp_src: texture_2d<f32>;
@group(0) @binding(1) var comp_bloom: texture_2d<f32>;
@group(0) @binding(2) var comp_samp: sampler;
@group(0) @binding(3) var<uniform> comp: CompositeU;

fn apply_grade(c_in: vec3<f32>) -> vec3<f32> {
    var c = c_in;
    // Exposure, in stops.
    c = c * exp2(comp.exposure);
    // White balance: warm pushes red up and blue down; tint moves green.
    c.r = c.r * (1.0 + comp.temperature * 0.2);
    c.b = c.b * (1.0 - comp.temperature * 0.2);
    c.g = c.g * (1.0 + comp.tint * 0.2);
    // Lift / gamma / gain.
    c = comp.gain * (c + comp.lift * (vec3<f32>(1.0) - c));
    c = pow(max(c, vec3<f32>(0.0)), vec3<f32>(1.0 / max(comp.gamma, 1e-3)));
    // Contrast about mid-grey.
    c = (c - vec3<f32>(0.5)) * comp.contrast + vec3<f32>(0.5);
    // Saturation.
    let l = luma(c);
    c = mix(vec3<f32>(l), c, comp.saturation);
    return c;
}

// A stable hash in 0..1 from an integer cell and a seed. Depends only on its
// inputs, so the same frame renders identically every time.
fn hash_cell(cell: vec2<u32>, seed: u32) -> f32 {
    var n = cell.x * 1973u + cell.y * 9277u + seed * 26699u;
    n = (n << 13u) ^ n;
    n = n * (n * n * 15731u + 789221u) + 1376312589u;
    return f32(n & 0x7fffffffu) / f32(0x7fffffff);
}

@fragment
fn fs_composite(in: VsOut) -> @location(0) vec4<f32> {
    let px = vec2<i32>(i32(in.pos.x), i32(in.pos.y));
    let src = textureLoad(comp_src, px, 0);
    var color = src.rgb;

    if ((comp.flags & FLAG_BLOOM) != 0u) {
        let b = textureSample(comp_bloom, comp_samp, in.uv).rgb;
        color = color + b * comp.bloom_intensity;
    }

    if ((comp.flags & FLAG_GRADE) != 0u) {
        color = apply_grade(color);
    }

    if ((comp.flags & FLAG_VIGNETTE) != 0u) {
        // Distance from centre, normalised so a corner is ~1.
        let d = distance(in.uv, vec2<f32>(0.5)) / 0.7071;
        let inner = 1.0 - comp.vignette_softness;
        let v = smoothstep(inner, 1.0, d) * comp.vignette_amount;
        color = mix(color, comp.vignette_color, v);
    }

    if ((comp.flags & FLAG_GRAIN) != 0u) {
        let size = max(comp.grain_size, 1.0);
        let cell = vec2<u32>(
            u32(in.pos.x / size),
            u32(in.pos.y / size),
        );
        let g = hash_cell(cell, comp.frame_index + 1u) * 2.0 - 1.0;
        color = color + vec3<f32>(g * comp.grain_amount);
    }

    // Alpha is passed through untouched, so a disabled compositor is an exact
    // copy of its input.
    return vec4<f32>(clamp(color, vec3<f32>(0.0), vec3<f32>(1.0)), src.a);
}
