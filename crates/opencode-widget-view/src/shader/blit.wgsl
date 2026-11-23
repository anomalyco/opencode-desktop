// A shader widget's last frame drawn into its effect slot: one texel per device pixel, cut by the
// slot's clip (its rounded corners, the scroll viewport) as Rune's own quads are (`clip_alpha`,
// from `rune::render::CLIP_WGSL`). The texture holds the template's premultiplied, sRGB-encoded
// output, written as is to an sRGB-blended frame and decoded for a linear one.

struct Slot {
    // The slot's rect in the pass, device px: x, y, w, h.
    rect: vec4<f32>,
    // Its clip in window device px: x0, y0, x1, y1.
    clip: vec4<f32>,
    // The pass's window origin and size, device px.
    frame: vec4<f32>,
    fade: vec4<u32>,
    // x: 1 when the pass blends sRGB-encoded values; y: 1 when the texture's alpha is ignored
    // (a GPU canvas configured `alphaMode: "opaque"`).
    flags: vec4<f32>,
};

@group(0) @binding(0) var<uniform> slot: Slot;
@group(0) @binding(1) var shot: texture_2d<f32>;
@group(0) @binding(2) var shot_sampler: sampler;

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
    out.uv = corner;
    return out;
}

fn to_linear(c: vec3<f32>) -> vec3<f32> {
    return select(pow((c + 0.055) / 1.055, vec3<f32>(2.4)), c / 12.92, c <= vec3<f32>(0.04045));
}

@fragment
fn fs(in: Out) -> @location(0) vec4<f32> {
    let texel = textureSample(shot, shot_sampler, in.uv);
    let a = select(clamp(texel.a, 0.0, 1.0), 1.0, slot.flags.y > 0.5);
    let straight = select(texel.rgb / max(a, 1e-4), vec3<f32>(0.0), a <= 0.0);
    let rgb = select(to_linear(clamp(straight, vec3<f32>(0.0), vec3<f32>(1.0))) * a, texel.rgb, slot.flags.x > 0.5);
    let cover = clip_alpha(in.position.xy + slot.frame.xy, slot.clip, slot.fade);
    return vec4<f32>(rgb * cover, a * cover);
}
