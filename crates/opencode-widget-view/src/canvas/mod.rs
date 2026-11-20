//! A `canvas` node: its box (full width unless `width`), what its template drew in it, and
//! what it hears: its size and whether it shows (which drive drawing, see [`state`]), and the
//! pointer, which a canvas with an `id` sends to `update` (`move` at most once a frame).

pub(crate) mod paint;
pub mod state;

use opencode_widget::draw::Pointer;
use opencode_widget::{PointerKind, WidgetEvent};
use rune::{Drag, El, Phase, Tx, el, text};

use crate::Cx;
use state::{Event, Sight};

#[allow(clippy::too_many_arguments)]
pub(crate) fn view<S: 'static>(cx: &Cx<S>, path: &str, id: Option<&str>, height: f64, width: Option<f64>, alt: Option<&str>, fill: bool) -> El {
    let k = cx.look.kit;
    let canvas = cx.local.canvas(path);
    let size = canvas.and_then(|c| c.size);
    // Filling (full screen), `height` is its least: it takes the height left.
    let b = if fill { el().min_h(height as f32).grow(1.0) } else { el().h(height as f32) };
    let mut b = b.role("img").label(alt.unwrap_or("").to_owned()).clip_rounded(0.0);
    b = match width {
        Some(w) => b.w(w as f32).max_w(cx.look.width),
        None => b.w_full(),
    };
    if let (Some(c), Some(size)) = (canvas, size) {
        b = b.children(paint::paint(k, c.commands(), size, cx.look.scale, cx.look.image));
        if let Some(error) = c.error() {
            let note = text(error.to_owned()).font(k.ty.caption()).color(k.c.danger.fg).truncate();
            b = b.child(el().pin(Some(6.0), None, Some(6.0), Some(4.0)).child(note).inert());
        }
    }
    let Some(hooks) = cx.hooks.cloned() else { return b };
    let lens = hooks.local.clone();
    let key = path.to_owned();
    b = b.on_layout(move |s: &mut S, [_, _, w, h]| (lens)(s).canvas_event(&key, Event::Resized([w, h])));
    let (lens, key) = (hooks.local.clone(), path.to_owned());
    b = b.on_visible(move |s: &mut S, on, _tx: &mut Tx| (lens)(s).canvas_event(&key, Event::Seen(if on { Sight::Shown } else { Sight::Hidden })));
    // Hover tells `draw` where the pointer is; it isn't an event.
    let (lens, key, h, cid) = (hooks.local.clone(), path.to_owned(), hooks.clone(), id.map(str::to_owned));
    b = b.on_pointer_move_local(move |s: &mut S, p, tx: &mut Tx| {
        if crate::captured(&h, s, cid.as_deref(), PointerKind::Move, [p.x, p.y], tx) {
            return;
        }
        let down = (lens)(s).canvas(&key).and_then(|c| c.pointer).is_some_and(|p| p.down);
        (lens)(s).canvas_event(&key, Event::Pointer(Some(Pointer { x: f64::from(p.x), y: f64::from(p.y), down })));
    });
    let (lens, key) = (hooks.local.clone(), path.to_owned());
    b = b.on_hover_change(move |s: &mut S, inside, _tx: &mut Tx| {
        if !inside {
            (lens)(s).canvas_event(&key, Event::Pointer(None));
        }
    });
    let live = cx.live;
    let (id, key) = (id.map(str::to_owned), path.to_owned());
    b.on_drag(move |s: &mut S, d: &mut Drag, tx: &mut Tx| {
        let (x, y) = (f64::from(d.x), f64::from(d.y));
        let (w, h) = (f64::from(d.w), f64::from(d.h));
        let phase = match d.phase {
            Phase::Down => PointerKind::Down,
            Phase::Move => PointerKind::Move,
            Phase::Up => PointerKind::Up,
        };
        if crate::captured(&hooks, s, id.as_deref(), phase, [d.x, d.y], tx) {
            return;
        }
        let down = d.phase != Phase::Up;
        let local = (hooks.local)(s);
        local.canvas_event(&key, Event::Pointer(Some(Pointer { x, y, down })));
        let kind = match d.phase {
            Phase::Down => PointerKind::Down,
            Phase::Move => {
                // `move` goes at most once a frame.
                let Some(c) = local.canvas(&key) else { return };
                if c.moved == tx.clock() {
                    return;
                }
                PointerKind::Move
            }
            Phase::Up => PointerKind::Up,
        };
        if kind == PointerKind::Move {
            local.canvas_moved(&key, tx.clock());
        }
        if let (Some(id), true) = (&id, live) {
            hooks.send(s, WidgetEvent::pointer(id.clone(), kind, x, y, w, h), tx);
        }
    })
}
