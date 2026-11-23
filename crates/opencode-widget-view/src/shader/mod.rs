//! A GPU canvas: an effect slot in Rune's painter stream that [`gpu::ShaderGpu`] paints, in the
//! host's compositor. A `wgsl` canvas it draws with the node's WGSL; a live program's `gpu: true`
//! canvas it shows as the run's replay drew it (`widgets-gpu.md`, [`opencode_widget::gpu`]). The
//! view and the GPU side meet in a registry of slots (one per GPU canvas on screen, by widget and
//! path): the view writes each `wgsl` slot's source, `values`, `fps`, `duration` and pointer; the
//! GPU side writes back a compile error or a freeze, which the view shows as the `poster` (or
//! `alt`) with the error under it, as it shows a live canvas the run stopped or can't draw. It
//! never renders at a zero width or height.

pub mod gpu;
mod live;

#[cfg(target_os = "macos")]
pub use live::{MetalShare, share_metal};
pub use opencode_widget::gpu::{Device as GpuDevice, device_limits as gpu_limits};

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::rc::Rc;
use std::sync::Arc;

use opencode_widget::gpu::Canvases;
use rune::{Drag, El, Phase, SurfaceFit, Tx, el, text};

use crate::Cx;

/// Effect marks this crate owns: the top 16 bits, `WS`.
const TAG: u32 = 0x5753 << 16;

/// Whether effect mark `mark` is a GPU canvas's.
pub fn is_mark(mark: u32) -> bool {
    mark & 0xFFFF_0000 == TAG
}

/// Where a slot stands, as the GPU side last found it.
#[derive(Clone, Debug, Default, PartialEq)]
pub enum Status {
    #[default]
    Running,
    /// It didn't run: its WGSL failed validation (`program`: the widget template's error), or the device
    /// refused WGSL that validated (a client error, never reported).
    Failed { message: String, program: bool },
    /// It ran over 50 ms a frame three frames in a row: stopped on its last frame.
    Frozen,
}

/// One GPU canvas on screen, as the view last built it.
#[derive(Clone, Debug)]
pub struct Slot {
    pub wgsl: Rc<str>,
    /// A hash of `wgsl`: the compiled pipeline's key, shared by every slot with that source.
    pub source: u64,
    pub values: [f32; 16],
    pub fps: f64,
    /// Seconds: it animates at `fps` until it drew a frame this old, then only on `values`.
    pub duration: Option<f64>,
    /// Logical pixels from the slot's top-left, and whether it's pressed.
    pub pointer: Option<([f32; 2], bool)>,
    pub status: Status,
}

#[derive(Default)]
struct Registry {
    marks: HashMap<String, u32>,
    slots: HashMap<u32, Slot>,
    /// Live GPU canvases on screen: their run's canvases, and the canvas's id.
    live: HashMap<u32, (Arc<Canvases>, String)>,
    next: u16,
}

impl Registry {
    /// Slot `key`'s mark, made the first time it's shown.
    fn mark(&mut self, key: &str) -> u32 {
        if let Some(m) = self.marks.get(key) {
            return *m;
        }
        self.next = self.next.wrapping_add(1).max(1);
        let m = TAG | u32::from(self.next);
        self.marks.insert(key.to_owned(), m);
        m
    }
}

/// Live slot `mark`: its run's canvases and the canvas's id.
pub fn live_slot(mark: u32) -> Option<(Arc<Canvases>, String)> {
    REGISTRY.with_borrow(|r| r.live.get(&mark).cloned())
}

thread_local! {
    static REGISTRY: RefCell<Registry> = RefCell::default();
    static CHANGED: Cell<bool> = const { Cell::new(false) };
}

/// Reads slot `mark`.
pub fn with_slot<R>(mark: u32, f: impl FnOnce(&Slot) -> R) -> Option<R> {
    REGISTRY.with_borrow(|r| r.slots.get(&mark).map(f))
}

/// The GPU side's verdict on slot `mark`; the view shows it from its next build.
pub fn set_status(mark: u32, status: Status) {
    REGISTRY.with_borrow_mut(|r| {
        if let Some(s) = r.slots.get_mut(&mark)
            && s.status != status
        {
            s.status = status;
            CHANGED.set(true);
        }
    });
}

/// Whether a slot's status changed since the last call: the host's poll builds the widget again.
pub fn take_changed() -> bool {
    CHANGED.replace(false)
}

/// The slots that failed: their widget's scope, why, and whether it's the program's error (WGSL
/// that failed validation) rather than this client's.
pub fn failures() -> Vec<(String, String, bool)> {
    REGISTRY.with_borrow(|r| {
        r.marks
            .iter()
            .filter_map(|(key, m)| match &r.slots.get(m)?.status {
                Status::Failed { message, program } => Some((key.rsplit_once('/').map_or(key.as_str(), |(scope, _)| scope).to_owned(), message.clone(), *program)),
                _ => None,
            })
            .collect()
    })
}

/// Forgets the slots `keep` doesn't name (by mark), as the GPU side drops a slot off screen.
pub fn retain(keep: impl Fn(u32) -> bool) {
    REGISTRY.with_borrow_mut(|r| {
        r.slots.retain(|m, _| keep(*m));
        r.live.retain(|m, _| keep(*m));
        let (slots, live) = (&r.slots, &r.live);
        r.marks.retain(|_, m| slots.contains_key(m) || live.contains_key(m));
    });
}

/// Slot `key`'s mark, the slot written with this build's source, values, fps and duration; its
/// status.
fn show(key: &str, wgsl: &str, values: &[f64], fps: f64, duration: Option<f64>) -> (u32, Status) {
    REGISTRY.with_borrow_mut(|r| {
        let mark = r.mark(key);
        let mut h = std::collections::hash_map::DefaultHasher::new();
        wgsl.hash(&mut h);
        let source = h.finish();
        let mut v = opencode_widget::shader::Uniforms::default();
        v.set_values(values);
        let slot = r.slots.entry(mark).or_insert_with(|| Slot { wgsl: Rc::from(wgsl), source, values: v.values, fps, duration, pointer: None, status: Status::Running });
        if slot.source != source {
            *slot = Slot { wgsl: Rc::from(wgsl), source, values: v.values, fps, duration, pointer: slot.pointer, status: Status::Running };
        }
        slot.values = v.values;
        slot.fps = fps;
        slot.duration = duration;
        (mark, slot.status.clone())
    })
}

/// A live program re-rendered with only GPU canvas `values` changed: slot `key` (its widget's scope
/// and the node's path) takes them now, without a build. Whether it had such a slot.
pub fn set_values(key: &str, values: &[f64]) -> bool {
    REGISTRY.with_borrow_mut(|r| {
        let Some(slot) = r.marks.get(key).and_then(|m| r.slots.get_mut(m)) else { return false };
        let mut v = opencode_widget::shader::Uniforms::default();
        v.set_values(values);
        slot.values = v.values;
        true
    })
}

/// A GPU canvas's box: `height` (its least, when it fills), `width`, and whether it fills.
pub(crate) struct Size {
    pub height: f64,
    pub width: Option<f64>,
    pub fill: bool,
}

fn point(mark: u32, at: Option<([f32; 2], bool)>) {
    REGISTRY.with_borrow_mut(|r| {
        if let Some(s) = r.slots.get_mut(&mark) {
            s.pointer = at;
        }
    });
}

/// `id`: the id its pointer events carry, when it takes input (`pointer: true`).
#[allow(clippy::too_many_arguments)]
pub(crate) fn view<S: 'static>(cx: &Cx<S>, path: &str, id: Option<&str>, size: Size, wgsl: &str, values: &[f64], motion: (f64, Option<f64>), alt: Option<&str>, poster: Option<&str>) -> El {
    let (fps, duration) = motion;
    let key = format!("{}/{path}", cx.look.scope);
    let (mark, status) = show(&key, wgsl, values, fps, duration.filter(|d| d.is_finite() && *d >= 0.0));
    let b = frame(cx, size, alt);
    if let Status::Failed { message: error, .. } = status {
        return placeholder(cx, b, poster, alt.unwrap_or("Shader"), Some(error));
    }
    slot(cx, b, path, id, mark)
}

/// A live program's GPU canvas (`gpu: true`): `canvas` its id, the run's texture once drawn; its
/// placeholder (the `poster`, else `alt`, else "GPU canvas not supported") where this client has
/// no GPU for it, with the error that stopped it under it.
#[allow(clippy::too_many_arguments)]
pub(crate) fn live_view<S: 'static>(cx: &Cx<S>, path: &str, id: Option<&str>, canvas: Option<&str>, size: Size, alt: Option<&str>, poster: Option<&str>) -> El {
    let b = frame(cx, size, alt);
    let (Some(canvases), Some(canvas)) = (cx.local.gpu(), canvas) else {
        return placeholder(cx, b, poster, alt.unwrap_or("GPU canvas not supported"), None);
    };
    match canvases.status(canvas) {
        opencode_widget::gpu::Status::Unavailable => placeholder(cx, b, poster, alt.unwrap_or("GPU canvas not supported"), None),
        opencode_widget::gpu::Status::Stopped(error) => placeholder(cx, b, poster, alt.unwrap_or("GPU canvas not supported"), Some(error)),
        opencode_widget::gpu::Status::Drawn => {
            let key = format!("{}/{path}", cx.look.scope);
            let mark = REGISTRY.with_borrow_mut(|r| {
                let m = r.mark(&key);
                r.live.insert(m, (canvases.clone(), canvas.to_owned()));
                m
            });
            slot(cx, b, path, id, mark)
        }
    }
}

/// A GPU canvas's box: its size, clipped to its rounded corners, labelled by `alt`.
fn frame<S>(cx: &Cx<S>, size: Size, alt: Option<&str>) -> El {
    let Size { height, width, fill } = size;
    let b = if fill { el().min_h(height as f32).grow(1.0) } else { el().h(height as f32) };
    let b = b.radius(crate::nodes::RADIUS).clip_rounded(crate::nodes::RADIUS).role("img").label(alt.unwrap_or("").to_owned());
    match width {
        Some(w) => b.w(w as f32).max_w(cx.look.width),
        None => b.w_full(),
    }
}

/// The poster in the box, or the words it stands for, with `error` under it.
fn placeholder<S>(cx: &Cx<S>, b: El, poster: Option<&str>, words: &str, error: Option<String>) -> El {
    let k = cx.look.kit;
    let picture = poster.and_then(|src| (cx.look.image)(src)).filter(|i| i.error().is_none());
    let b = match picture {
        Some(img) => b.pixels_fit(img.pixels(), SurfaceFit::Fill),
        None => b.fill(k.c.surface_weak).center().child(text(words.to_owned()).font(k.ty.ui).color(k.c.text_muted)),
    };
    let Some(error) = error else { return b };
    let note = text(error).font(k.ty.mono).color(k.c.danger.fg).wrap().selectable(true);
    rune::col().w_full().gap(4.0).child(b).child(note)
}

/// The box as effect slot `mark`, with its size kept for the GPU side and, when it takes input
/// (`id`), its pointer sent to the widget template.
fn slot<S: 'static>(cx: &Cx<S>, b: El, path: &str, id: Option<&str>, mark: u32) -> El {
    let b = b.effect(mark).effect_data([f32::from_bits(mark), 0.0, 0.0, 0.0]);
    let Some(hooks) = cx.hooks.cloned() else { return b };
    let (live, id, key) = (cx.live, id.map(str::to_owned), path.to_owned());
    let (h, sid, hkey) = (hooks.clone(), id.clone(), key.clone());
    let (sized, skey) = (hooks.local.clone(), key.clone());
    // Only where it sits moved (a scroll), or only the pointer the GPU reads: the view needn't
    // build again.
    let b = b.on_layout_tx(move |s: &mut S, [x, y, w, hgt], tx: &mut Tx| {
        let local = (sized)(s);
        local.set_origin(&skey, [x, y]);
        if local.size(&skey) == Some([w, hgt]) {
            tx.live_only();
            return;
        }
        local.set_size(&skey, [w, hgt]);
    });
    // In a stack, a GPU canvas that takes no input lets the pointer through to the layer below.
    if cx.layered && id.is_none() {
        return b;
    }
    b.on_pointer_move_local(move |s: &mut S, p, tx: &mut Tx| {
        if crate::captured(&h, s, sid.as_deref(), opencode_widget::PointerKind::Move, [p.x, p.y], tx) {
            return;
        }
        let down = with_slot(mark, |s| s.pointer.is_some_and(|(_, d)| d)).unwrap_or(false);
        point(mark, Some(([p.x, p.y], down)));
        // Hover: the widget template hears the pointer moving with no button held, once a frame.
        let (Some(id), false, true) = (&sid, down, live) else {
            tx.live_only();
            return;
        };
        let local = (h.local)(s);
        let Some(size) = local.size(&hkey) else {
            tx.live_only();
            return;
        };
        if local.moved_at(&hkey) == tx.clock() {
            tx.live_only();
            return;
        }
        local.set_moved(&hkey, tx.clock());
        let close = local.shader_close(&hkey);
        h.send(s, opencode_widget::WidgetEvent::hover(id.clone(), f64::from(p.x), f64::from(p.y), f64::from(size[0]), f64::from(size[1]), close), tx);
    })
    .on_hover_change(move |_: &mut S, inside, tx: &mut Tx| {
        if !inside {
            point(mark, None);
        }
        tx.live_only();
    })
    .on_drag(move |s: &mut S, d: &mut Drag, tx: &mut Tx| {
        let phase = match d.phase {
            Phase::Down => opencode_widget::PointerKind::Down,
            Phase::Move => opencode_widget::PointerKind::Move,
            Phase::Up => opencode_widget::PointerKind::Up,
        };
        if crate::captured(&hooks, s, id.as_deref(), phase, [d.x, d.y], tx) {
            return;
        }
        point(mark, Some(([d.x, d.y], d.phase != Phase::Up)));
        // One that takes input sends the pointer to the widget template as a 2D canvas does: `move` only
        // while down, at most once a frame.
        let (Some(id), true) = (&id, live) else { return };
        let kind = match d.phase {
            Phase::Down => opencode_widget::PointerKind::Down,
            Phase::Move => {
                let local = (hooks.local)(s);
                if local.moved_at(&key) == tx.clock() {
                    return;
                }
                local.set_moved(&key, tx.clock());
                opencode_widget::PointerKind::Move
            }
            Phase::Up => opencode_widget::PointerKind::Up,
        };
        let close = (hooks.local)(s).shader_close(&key);
        let event = opencode_widget::WidgetEvent::pointer(id.clone(), kind, f64::from(d.x), f64::from(d.y), f64::from(d.w), f64::from(d.h), close);
        hooks.send(s, event, tx);
    })
}
