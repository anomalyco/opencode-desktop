//! A `shader` node: an effect slot in Rune's painter stream that [`gpu::ShaderGpu`] paints, in
//! the host's compositor, with the node's WGSL. The view and the GPU side meet in a registry of
//! slots (one per shader on screen, by widget and path): the view writes each slot's source,
//! `values`, `fps` and pointer; the GPU side writes back a compile error or a freeze, which the
//! view shows as the `poster` (or `alt`) with the error under it.

pub mod gpu;

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::rc::Rc;

use opencode_widget::shader::DEFAULT_FPS;
use rune::{Drag, El, Phase, SurfaceFit, Tx, el, text};

use crate::Cx;

/// Effect marks this crate owns: the top 16 bits, `WS`.
const TAG: u32 = 0x5753 << 16;

/// Whether effect mark `mark` is a shader widget's.
pub fn is_mark(mark: u32) -> bool {
    mark & 0xFFFF_0000 == TAG
}

/// Where a slot stands, as the GPU side last found it.
#[derive(Clone, Debug, Default, PartialEq)]
pub enum Status {
    #[default]
    Running,
    /// Its WGSL didn't compile (or the device refused it).
    Failed(String),
    /// It ran over 50 ms a frame three frames in a row: stopped on its last frame.
    Frozen,
}

/// One shader on screen, as the view last built it.
#[derive(Clone, Debug)]
pub struct Slot {
    pub wgsl: Rc<str>,
    /// A hash of `wgsl`: the compiled pipeline's key, shared by every slot with that source.
    pub source: u64,
    pub values: [f32; 16],
    pub fps: f64,
    /// Logical pixels from the slot's top-left, and whether it's pressed.
    pub pointer: Option<([f32; 2], bool)>,
    pub status: Status,
}

#[derive(Default)]
struct Registry {
    marks: HashMap<String, u32>,
    slots: HashMap<u32, Slot>,
    next: u16,
}

/// Frames drawn with a shader on screen: a window whose widget animates only its shader draws
/// without building again, and this still says it draws.
pub(crate) static PAINTED: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// How many frames were drawn with a shader on screen.
pub fn painted() -> u64 {
    PAINTED.load(std::sync::atomic::Ordering::Relaxed)
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

/// The slots whose shader didn't compile: the scope each was built in (its widget) and why.
pub fn failures() -> Vec<(String, String)> {
    REGISTRY.with_borrow(|r| {
        r.marks
            .iter()
            .filter_map(|(key, m)| match &r.slots.get(m)?.status {
                Status::Failed(e) => Some((key.rsplit_once('/').map_or(key.as_str(), |(scope, _)| scope).to_owned(), e.clone())),
                _ => None,
            })
            .collect()
    })
}

/// Forgets the slots `keep` doesn't name (by mark), as the GPU side drops a slot off screen.
pub fn retain(keep: impl Fn(u32) -> bool) {
    REGISTRY.with_borrow_mut(|r| {
        r.slots.retain(|m, _| keep(*m));
        let slots = &r.slots;
        r.marks.retain(|_, m| slots.contains_key(m));
    });
}

/// Slot `key`'s mark, the slot written with this build's source, values and fps; its status.
fn show(key: &str, wgsl: &str, values: &[f64], fps: f64) -> (u32, Status) {
    REGISTRY.with_borrow_mut(|r| {
        let mark = match r.marks.get(key) {
            Some(m) => *m,
            None => {
                r.next = r.next.wrapping_add(1).max(1);
                let m = TAG | u32::from(r.next);
                r.marks.insert(key.to_owned(), m);
                m
            }
        };
        let mut h = std::collections::hash_map::DefaultHasher::new();
        wgsl.hash(&mut h);
        let source = h.finish();
        let mut v = opencode_widget::shader::Uniforms::default();
        v.set_values(values);
        let slot = r.slots.entry(mark).or_insert_with(|| Slot { wgsl: Rc::from(wgsl), source, values: v.values, fps, pointer: None, status: Status::Running });
        if slot.source != source {
            *slot = Slot { wgsl: Rc::from(wgsl), source, values: v.values, fps, pointer: slot.pointer, status: Status::Running };
        }
        slot.values = v.values;
        slot.fps = fps;
        (mark, slot.status.clone())
    })
}

/// A live program re-rendered with only shader `values` changed: slot `key` (its widget's scope
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

/// A shader node's box: `height` (its least, when it fills full screen) and `width`.
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

#[allow(clippy::too_many_arguments)]
pub(crate) fn view<S: 'static>(cx: &Cx<S>, path: &str, id: Option<&str>, size: Size, wgsl: &str, values: &[f64], fps: Option<f64>, alt: Option<&str>, poster: Option<&str>) -> El {
    let k = cx.look.kit;
    let fps = fps.unwrap_or(DEFAULT_FPS).clamp(0.0, opencode_widget::draw::MAX_FPS);
    let key = format!("{}/{path}", cx.look.scope);
    let (mark, status) = show(&key, wgsl, values, fps);
    let Size { height, width, fill } = size;
    let b = if fill { el().min_h(height as f32).grow(1.0) } else { el().h(height as f32) };
    let mut b = b.radius(crate::nodes::RADIUS).clip_rounded(crate::nodes::RADIUS).role("img").label(alt.unwrap_or("").to_owned());
    b = match width {
        Some(w) => b.w(w as f32).max_w(cx.look.width),
        None => b.w_full(),
    };
    if let Status::Failed(error) = status {
        // The poster, or the words it stands for, with why the shader didn't run under it.
        let picture = poster.and_then(|src| (cx.look.image)(src)).filter(|i| i.error().is_none());
        let b = match picture {
            Some(img) => b.pixels_fit(img.pixels(), SurfaceFit::Fill),
            None => b.fill(k.c.surface_weak).center().child(text(alt.unwrap_or("Shader").to_owned()).font(k.ty.ui).color(k.c.text_muted)),
        };
        let note = text(error).font(k.ty.mono).color(k.c.danger.fg).wrap().selectable(true);
        return rune::col().w_full().gap(4.0).child(b).child(note);
    }
    let b = b.effect(mark).effect_data([f32::from_bits(mark), 0.0, 0.0, 0.0]);
    let Some(hooks) = cx.hooks.cloned() else { return b };
    let (live, id, key) = (cx.live, id.map(str::to_owned), path.to_owned());
    let (h, sid) = (hooks.clone(), id.clone());
    b.on_pointer_move_local(move |s: &mut S, p, tx: &mut Tx| {
        if crate::captured(&h, s, sid.as_deref(), opencode_widget::PointerKind::Move, [p.x, p.y], tx) {
            return;
        }
        let down = with_slot(mark, |s| s.pointer.is_some_and(|(_, d)| d)).unwrap_or(false);
        point(mark, Some(([p.x, p.y], down)));
    })
    .on_hover_change(move |_: &mut S, inside, _tx: &mut Tx| {
        if !inside {
            point(mark, None);
        }
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
        // A shader with an id sends the pointer to the template as a canvas does: `move` only
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
        let event = opencode_widget::WidgetEvent::pointer(id.clone(), kind, f64::from(d.x), f64::from(d.y), f64::from(d.w), f64::from(d.h));
        hooks.send(s, event, tx);
    })
}
