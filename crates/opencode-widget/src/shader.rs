//! A GPU canvas's WGSL (a `canvas` with `wgsl`) as every client runs it (the contract's "GPU
//! canvases"): the standard `Widget` uniform header, the widget template's source defining `fn shade(frag: vec2f) -> vec4f`, and
//! the fragment entry point, plus the uniform block's byte layout. Shared by the Rune renderer and
//! the mobile bindings (which translate the same module with naga).

/// The contract's cap on a shader's source.
pub const MAX_SOURCE: usize = 16 * 1024;
/// Frames a shader may take over [`SLOW_FRAME_MS`] in a row before it stops.
pub const SLOW_FRAMES_TO_FREEZE: u32 = 3;
/// A frame slower than this counts against a shader.
pub const SLOW_FRAME_MS: f64 = 50.0;
/// `fps` when the node sets none.
pub const DEFAULT_FPS: f64 = 60.0;

/// Prepended to the widget template's source.
pub const HEADER: &str = "struct Widget { resolution: vec2f, time: f32, scale: f32, pointer: vec4f, values: array<vec4f, 4> }\n@group(0) @binding(0) var<uniform> widget: Widget;\n";

/// Appended after it: the fragment entry point.
pub const ENTRY: &str = "\n@fragment fn widget_main(@builtin(position) p: vec4f) -> @location(0) vec4f { return shade(p.xy); }\n";

/// A full-screen triangle, the vertex stage clients pair the fragment entry with.
pub const VERTEX: &str = "@vertex fn widget_vertex(@builtin(vertex_index) i: u32) -> @builtin(position) vec4f {\n  let uv = vec2f(f32((i << 1u) & 2u), f32(i & 2u));\n  return vec4f(uv * 2.0 - 1.0, 0.0, 1.0);\n}\n";

/// The fragment module for `source`: header, source, entry point. Refuses a source over
/// [`MAX_SOURCE`].
pub fn module(source: &str) -> Result<String, String> {
    if source.len() > MAX_SOURCE {
        return Err(format!("the shader is {} bytes, over the limit of {MAX_SOURCE}", source.len()));
    }
    let mut out = String::with_capacity(HEADER.len() + source.len() + ENTRY.len() + VERTEX.len());
    out.push_str(HEADER);
    out.push_str(source);
    out.push_str(ENTRY);
    Ok(out)
}

/// [`module`] with the full-screen [`VERTEX`] stage too, for a client that builds both from one
/// module (entry points `widget_vertex` and `widget_main`).
pub fn module_with_vertex(source: &str) -> Result<String, String> {
    let mut m = module(source)?;
    m.push_str(VERTEX);
    Ok(m)
}

/// The `Widget` uniform block, laid out as WGSL lays it out: 96 bytes.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Uniforms {
    pub resolution: [f32; 2],
    pub time: f32,
    pub scale: f32,
    /// x, y, down (0 or 1), 0.
    pub pointer: [f32; 4],
    /// The node's `values`, zero padded.
    pub values: [f32; 16],
}

impl Uniforms {
    pub const SIZE: usize = 96;

    /// Sets `values` from a node's list: up to 16 floats, row-major, zero padded.
    pub fn set_values(&mut self, values: &[f64]) {
        self.values = [0.0; 16];
        for (slot, v) in self.values.iter_mut().zip(values) {
            *slot = *v as f32;
        }
    }

    /// The block's bytes (little-endian, as GPUs read them).
    pub fn bytes(&self) -> [u8; Self::SIZE] {
        let mut out = [0u8; Self::SIZE];
        let floats = [self.resolution[0], self.resolution[1], self.time, self.scale]
            .into_iter()
            .chain(self.pointer)
            .chain(self.values);
        for (i, f) in floats.enumerate() {
            out[i * 4..i * 4 + 4].copy_from_slice(&f.to_le_bytes());
        }
        out
    }
}
