// Desktop's pin board quads (`pin-quads.ts`), drawn into an offscreen layer as Desktop's canvas
// is: filled parallelograms whose coverage is each pixel's overlap with the quad along both edge
// directions, exact for axis-aligned rects. Shadows go first with MAX blending (the pipeline's).

struct Layer {
    // x, y: the region's origin in panel points; z, w: the layer's size in pixels.
    view: vec4<f32>,
    // x: pixels per point.
    scale: vec4<f32>,
};

@group(0) @binding(0) var<uniform> layer: Layer;

struct PinIn {
    @location(0) origin: vec2<f32>,
    @location(1) u: vec2<f32>,
    @location(2) v: vec2<f32>,
    @location(3) color: vec4<f32>,
};

struct PinOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) st: vec2<f32>,
    @location(1) span: vec2<f32>,
    @location(2) tint: vec4<f32>,
};

@vertex
fn pins_vs(@builtin(vertex_index) vi: u32, i: PinIn) -> PinOut {
    let corner = vec2<f32>(f32(vi & 1u), f32((vi >> 1u) & 1u));
    let scale = layer.scale.x;
    let du = i.u * scale;
    let dv = i.v * scale;
    let area = max(abs(du.x * dv.y - du.y * dv.x), 1e-4);
    // span is the quad's width across each pair of edges in pixels; the quad grows a pixel past
    // every edge so partly covered pixels get drawn.
    let span = vec2<f32>(area / max(length(dv), 1e-4), area / max(length(du), 1e-4));
    let st = mix(-1.0 / span, 1.0 + 1.0 / span, corner);
    let p = (i.origin - layer.view.xy) * scale + du * st.x + dv * st.y;
    var o: PinOut;
    o.pos = vec4<f32>(p.x / layer.view.z * 2.0 - 1.0, 1.0 - p.y / layer.view.w * 2.0, 0.0, 1.0);
    o.st = st;
    o.span = span;
    o.tint = i.color;
    return o;
}

@fragment
fn pins_fs(i: PinOut) -> @location(0) vec4<f32> {
    let d = i.st * i.span;
    let c = clamp(min(d + 0.5, i.span) - max(d - 0.5, vec2<f32>(0.0)), vec2<f32>(0.0), vec2<f32>(1.0));
    return i.tint * (c.x * c.y);
}
