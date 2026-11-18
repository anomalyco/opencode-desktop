// The egg's drawing in its slots (`gpu.rs`): a pin layer composited as Desktop's canvas is
// (`blit_*`), and the lodger, the flat wordmark, its shimmer and the paddle (`shape_*`), in device
// pixels. Colours arrive premultiplied and sRGB-encoded, as Desktop's 8-bit canvas and SVG colours
// are; a frame that blends in linear light gets them decoded here. Prepended: `rune::render::CLIP_WGSL`.

struct Slot {
    // x, y: the pass's window origin in device px; z, w: the pass's size.
    frame: vec4<f32>,
    // The slot's clip in window device px, and its packed fade and corner words.
    clip: vec4<f32>,
    fade: vec4<u32>,
    // x: 1 when the pass blends sRGB-encoded values; y, z (blit): the share of the layer's texture
    // this frame drew.
    flags: vec4<f32>,
    // Blit: the layer's rect in window device px.
    dest: vec4<f32>,
};

@group(0) @binding(0) var<uniform> slot: Slot;
@group(0) @binding(1) var layer_tex: texture_2d<f32>;
@group(0) @binding(2) var layer_sampler: sampler;

fn to_linear(c: vec3<f32>) -> vec3<f32> {
    return select(pow((c + 0.055) / 1.055, vec3<f32>(2.4)), c / 12.92, c <= vec3<f32>(0.04045));
}

// A premultiplied sRGB-encoded colour in the pass's blending space.
fn out_color(c: vec4<f32>) -> vec4<f32> {
    if (slot.flags.x > 0.5 || c.a <= 0.0) { return c; }
    return vec4<f32>(to_linear(c.rgb / c.a) * c.a, c.a);
}

fn ndc(p: vec2<f32>) -> vec4<f32> {
    let q = p - slot.frame.xy;
    return vec4<f32>(q.x / slot.frame.z * 2.0 - 1.0, 1.0 - q.y / slot.frame.w * 2.0, 0.0, 1.0);
}

// Blit ---------------------------------------------------------------------------------------

struct BlitOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn blit_vs(@builtin(vertex_index) vi: u32) -> BlitOut {
    let corner = vec2<f32>(f32(vi & 1u), f32((vi >> 1u) & 1u));
    var o: BlitOut;
    o.pos = ndc(slot.dest.xy + corner * slot.dest.zw);
    o.uv = corner * slot.flags.yz;
    return o;
}

@fragment
fn blit_fs(i: BlitOut) -> @location(0) vec4<f32> {
    let c = textureSample(layer_tex, layer_sampler, i.uv);
    return out_color(c) * clip_alpha(i.pos.xy + slot.frame.xy, slot.clip, slot.fade);
}

// Shapes -------------------------------------------------------------------------------------

const RECT: u32 = 0u;
const RRECT: u32 = 1u;
const CAPSULE: u32 = 2u;
const PIN: u32 = 3u;
const SHIMMER: u32 = 4u;
const SHADOW: u32 = 5u;
const RING: u32 = 6u;

struct ShapeIn {
    // The device-pixel box it may touch: x0, y0, x1, y1.
    @location(0) bounds: vec4<f32>,
    // Window device px to local: local = m * (p - t), m = (m00, m01, m10, m11).
    @location(1) m: vec4<f32>,
    // t.xy, the kind, and a spare.
    @location(2) t: vec4<f32>,
    // Kind-specific sizes: half extents, radius, and a width or sigma.
    @location(3) size: vec4<f32>,
    @location(4) color: vec4<f32>,
    @location(5) color2: vec4<f32>,
};

struct ShapeOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) @interpolate(flat) m: vec4<f32>,
    @location(1) @interpolate(flat) t: vec4<f32>,
    @location(2) @interpolate(flat) size: vec4<f32>,
    @location(3) @interpolate(flat) color: vec4<f32>,
    @location(4) @interpolate(flat) color2: vec4<f32>,
};

@vertex
fn shape_vs(@builtin(vertex_index) vi: u32, i: ShapeIn) -> ShapeOut {
    let corner = vec2<f32>(f32(vi & 1u), f32((vi >> 1u) & 1u));
    var o: ShapeOut;
    o.pos = ndc(mix(i.bounds.xy, i.bounds.zw, corner));
    o.m = i.m;
    o.t = i.t;
    o.size = i.size;
    o.color = i.color;
    o.color2 = i.color2;
    return o;
}

// A signed distance in local units and its local gradient.
struct Dist {
    d: f32,
    g: vec2<f32>,
};

fn sd_box(p: vec2<f32>, half: vec2<f32>, r: f32) -> Dist {
    let rr = min(r, min(half.x, half.y));
    let q = abs(p) - half + vec2<f32>(rr);
    let s = select(vec2<f32>(-1.0), vec2<f32>(1.0), p >= vec2<f32>(0.0));
    var out: Dist;
    if (q.x > 0.0 && q.y > 0.0) {
        out.d = length(q) - rr;
        out.g = s * normalize(q);
    } else if (q.x > q.y) {
        out.d = q.x - rr;
        out.g = vec2<f32>(s.x, 0.0);
    } else {
        out.d = q.y - rr;
        out.g = vec2<f32>(0.0, s.y);
    }
    return out;
}

// The parallelogram with corner `o` and edges `e1`, `e2`.
fn sd_para(p: vec2<f32>, o: vec2<f32>, e1: vec2<f32>, e2: vec2<f32>) -> Dist {
    let n1 = normalize(vec2<f32>(e1.y, -e1.x));
    let n2 = normalize(vec2<f32>(e2.y, -e2.x));
    let q = p - (o + (e1 + e2) * 0.5);
    let a1 = dot(q, n1);
    let a2 = dot(q, n2);
    let d1 = abs(a1) - abs(dot(e2, n1)) * 0.5;
    let d2 = abs(a2) - abs(dot(e1, n2)) * 0.5;
    var out: Dist;
    if (d1 > d2) {
        out.d = d1;
        out.g = n1 * sign(a1);
    } else {
        out.d = d2;
        out.g = n2 * sign(a2);
    }
    return out;
}

// Coverage from a local distance: its screen-space rate is the local gradient through `m`.
fn cover(s: Dist, m: vec4<f32>) -> f32 {
    let w = max(length(vec2<f32>(s.g.x * m.x + s.g.y * m.z, s.g.x * m.y + s.g.y * m.w)), 1e-5);
    return clamp(0.5 - s.d / w, 0.0, 1.0);
}

// A pixel's overlap with the box [-half, half] around `p`, exact for an axis-aligned box.
fn box_cover(p: vec2<f32>, half: vec2<f32>) -> f32 {
    let e = clamp(min(half, p + vec2<f32>(0.5)) - max(-half, p - vec2<f32>(0.5)), vec2<f32>(0.0), vec2<f32>(1.0));
    return e.x * e.y;
}

fn gaussian(x: f32, sigma: f32) -> f32 {
    return exp(-(x * x) / (2.0 * sigma * sigma)) / (sqrt(2.0 * 3.14159265) * sigma);
}

fn erf2(x: vec2<f32>) -> vec2<f32> {
    let s = sign(x);
    let a = abs(x);
    var r = 1.0 + (0.278393 + (0.230389 + 0.078108 * (a * a)) * a) * a;
    r = r * r;
    return s - s / (r * r);
}

fn shadow_x(x: f32, y: f32, sigma: f32, corner: f32, half: vec2<f32>) -> f32 {
    let delta = min(half.y - corner - abs(y), 0.0);
    let curved = half.x - corner + sqrt(max(0.0, corner * corner - delta * delta));
    let integral = 0.5 + 0.5 * erf2((x + vec2<f32>(-curved, curved)) * (sqrt(0.5) / sigma));
    return integral.y - integral.x;
}

// Rune's rounded-box shadow (`shader.wgsl` `shadow`), so the paddle casts the composer's.
fn shadow(p: vec2<f32>, half: vec2<f32>, sigma: f32, corner: f32) -> f32 {
    if (abs(p.x) <= half.x - 6.0 * sigma && abs(p.y) <= half.y - corner - 6.0 * sigma) {
        return 0.99863125;
    }
    let lo = p.y - half.y;
    let hi = p.y + half.y;
    let start = clamp(-3.0 * sigma, lo, hi);
    let end = clamp(3.0 * sigma, lo, hi);
    let step = (end - start) / 4.0;
    var y = start + step * 0.5;
    var v = 0.0;
    for (var i = 0; i < 4; i = i + 1) {
        v = v + shadow_x(p.x, p.y - y, sigma, corner, half) * gaussian(y, sigma) * step;
        y = y + step;
    }
    return v;
}

@fragment
fn shape_fs(i: ShapeOut) -> @location(0) vec4<f32> {
    let screen = i.pos.xy + slot.frame.xy;
    let d = screen - i.t.xy;
    let local = vec2<f32>(i.m.x * d.x + i.m.y * d.y, i.m.z * d.x + i.m.w * d.y);
    let kind = u32(i.t.z + 0.5);
    let half = i.size.xy;
    var col = vec4<f32>(0.0);
    if (kind == RECT) {
        col = i.color * box_cover(local, half);
    } else if (kind == SHIMMER) {
        // The mask's highlight: clear at color2.x, whole at color2.y, clear again at color2.z.
        let x = screen.x;
        let up = clamp((x - i.color2.x) / max(i.color2.y - i.color2.x, 1e-4), 0.0, 1.0);
        let down = clamp((i.color2.z - x) / max(i.color2.z - i.color2.y, 1e-4), 0.0, 1.0);
        col = i.color * box_cover(local, half) * min(up, down);
    } else if (kind == RRECT) {
        col = i.color * cover(sd_box(local, half, i.size.z), i.m);
    } else if (kind == CAPSULE) {
        // t.xy is one end and size.xy the other, in device px; size.w the half width.
        let a = i.t.xy;
        let ab = i.size.xy - a;
        let h = clamp(dot(screen - a, ab) / max(dot(ab, ab), 1e-6), 0.0, 1.0);
        col = i.color * clamp(i.size.w - length(screen - a - ab * h) + 0.5, 0.0, 1.0);
    } else if (kind == PIN) {
        // Desktop's pin: two side parallelograms under a 3.8-unit face with 0.45 corners, as one
        // group, so its opacity (color2.a) takes the whole once.
        let face = cover(sd_box(local, vec2<f32>(1.9), 0.45), i.m);
        let s1 = cover(sd_para(local, vec2<f32>(-1.9, -1.9), vec2<f32>(3.8, 0.0), vec2<f32>(1.7, 2.4)), i.m);
        let s2 = cover(sd_para(local, vec2<f32>(1.9, -1.9), vec2<f32>(1.7, 2.4), vec2<f32>(0.0, 3.8)), i.m);
        let sides = min(1.0, s1 + s2);
        let g = i.color * face + vec4<f32>(i.color2.rgb, 1.0) * sides * (1.0 - face);
        col = g * i.color2.a;
    } else if (kind == SHADOW) {
        col = i.color * shadow(local, half, max(i.size.w, 0.01), min(i.size.z, min(half.x, half.y)));
    } else if (kind == RING) {
        // A box-shadow spread of size.w around the box [-half, half] with corners size.z.
        let outer = cover(sd_box(local, half + vec2<f32>(i.size.w), i.size.z + i.size.w), i.m);
        let inner = cover(sd_box(local, half, i.size.z), i.m);
        col = i.color * max(outer - inner, 0.0);
    }
    return out_color(col) * clip_alpha(screen, slot.clip, slot.fade);
}
