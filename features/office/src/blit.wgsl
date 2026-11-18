// An Office page's Vello target drawn into its effect slot: the slot's rect samples the target one
// texel per device pixel, cut by the slot's clip (its page's rounded corners, the scroll viewport)
// as Rune's own quads are (`clip_alpha`, from `rune::render::CLIP_WGSL`).

struct Slot {
    // The slot's rect in the pass, device px: x, y, w, h.
    rect: vec4<f32>,
    // Its clip in window device px: x0, y0, x1, y1.
    clip: vec4<f32>,
    // The frame's window origin and size, device px.
    frame: vec4<f32>,
    // Its clip's fade and rounded-corner words.
    fade: vec4<u32>,
    // x: 1 when the pass blends sRGB-encoded values; y: the part of the target the rect shows (u, v).
    flags: vec4<f32>,
};

@group(0) @binding(0) var<uniform> slot: Slot;
@group(0) @binding(1) var page: texture_2d<f32>;
@group(0) @binding(2) var page_sampler: sampler;

struct Out {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs(@builtin(vertex_index) i: u32) -> Out {
    let corner = vec2<f32>(f32(i & 1u), f32((i >> 1u) & 1u));
    let p = slot.rect.xy + corner * slot.rect.zw;
    let ndc = vec2<f32>(p.x / slot.frame.z * 2.0 - 1.0, 1.0 - p.y / slot.frame.w * 2.0);
    var out: Out;
    out.position = vec4<f32>(ndc, 0.0, 1.0);
    out.uv = corner * slot.flags.yz;
    return out;
}

fn to_linear(c: vec3<f32>) -> vec3<f32> {
    return select(pow((c + 0.055) / 1.055, vec3<f32>(2.4)), c / 12.92, c <= vec3<f32>(0.04045));
}

@fragment
fn fs(in: Out) -> @location(0) vec4<f32> {
    let texel = textureSample(page, page_sampler, in.uv);
    let rgb = select(to_linear(texel.rgb), texel.rgb, slot.flags.x > 0.5);
    let cover = clip_alpha(in.position.xy + slot.frame.xy, slot.clip, slot.fade);
    let a = texel.a * cover;
    return vec4<f32>(rgb * a, a);
}
