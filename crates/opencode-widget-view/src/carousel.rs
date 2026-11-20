//! A carousel: one page at a time, sliding in from the side it came from. It pages by its
//! arrows, its page dots, Left and Right while it has focus, a sideways trackpad swipe (one
//! page per gesture) and a drag (a mouse's, or a finger's swipe). Without an `id` it pages
//! locally; with one it also sends `change` with the new page.

use opencode_widget::{Node, WidgetEvent};
use rune::{Drag, El, Key, KeyEvent, Phase, Transition, Tx, WheelEvent, col, el, icon, row};
use rune_kit::Kit;

use crate::Cx;
use crate::local::Came;

/// How far a drag travels sideways before its release turns the page, in points.
const DRAG_PAGE: f32 = 40.0;
/// How far a page slides as it comes in.
const SLIDE: f32 = 28.0;

pub(crate) fn view<S: 'static>(cx: &Cx<S>, id: Option<&str>, children: &[Node], path: &str) -> El {
    let k = cx.look.kit;
    let count = children.len();
    if count == 0 {
        return el();
    }
    let pager = cx.local.pager(path);
    let page = pager.page.min(count - 1);
    let enter = match pager.came {
        Came::FromRight => Transition::offset(SLIDE, 0.0).combined(Transition::opacity()),
        Came::FromLeft => Transition::offset(-SLIDE, 0.0).combined(Transition::opacity()),
        Came::Still => Transition::opacity(),
    };
    let shown = crate::nodes::node(cx, &children[page], &format!("{path}.{page}")).transition(Transition::asymmetric(enter, Transition::opacity()));
    let mut stage = el().key("stage").w_full().clip_rounded(crate::nodes::RADIUS).child(shown);
    let Some(hooks) = cx.hooks.cloned().filter(|_| count >= 2) else { return col().w_full().child(stage) };
    let live = cx.live;
    // Every way of paging goes through here: the local page moves, and a carousel with an id
    // tells the template.
    let go = {
        // A read-only carousel still pages, here only.
        let (hooks, key, id) = (hooks.clone(), path.to_owned(), id.map(str::to_owned).filter(|_| live));
        move |s: &mut S, to: usize, tx: &mut Tx| {
            if (hooks.local)(s).go(&key, to, count)
                && let Some(id) = &id
            {
                hooks.send(s, WidgetEvent::change(id.clone(), to as u64), tx);
            }
        }
    };
    let go = std::rc::Rc::new(go);
    {
        let (local, key, go) = (hooks.local.clone(), path.to_owned(), go.clone());
        stage = stage.on_wheel(move |s: &mut S, ev: WheelEvent, tx: &mut Tx| {
            let (take, turned) = (local)(s).wheel(&key, &ev, count);
            if let Some(to) = turned {
                go(s, to, tx);
            }
            take
        });
    }
    {
        let go = go.clone();
        // A finger swipes it after the usual slop, so a vertical pan still scrolls the page.
        stage = stage.on_drag(move |s: &mut S, d: &mut Drag, tx: &mut Tx| {
            if d.phase != Phase::Up || d.cancelled() {
                return;
            }
            if d.dx <= -DRAG_PAGE && page + 1 < count {
                go(s, page + 1, tx);
            } else if d.dx >= DRAG_PAGE && page > 0 {
                go(s, page - 1, tx);
            }
        });
    }
    let arrow = |left: bool| {
        let to = if left { page.checked_sub(1) } else { Some(page + 1).filter(|p| *p < count) };
        let name = if left { &cx.look.words.previous } else { &cx.look.words.next };
        arrow(k, name, left, to.is_some()).on_press_tx({
            let go = go.clone();
            move |s: &mut S, tx: &mut Tx| {
                if let Some(to) = to {
                    go(s, to, tx);
                }
            }
        })
    };
    // Each arrow rides its edge, centred down the stage.
    let side = |key: &'static str, left: bool| {
        let (l, r) = if left { (Some(8.0), None) } else { (None, Some(8.0)) };
        el().key(key).pin(l, Some(0.0), r, Some(0.0)).center().child(arrow(left))
    };
    stage = stage.child(side("prev", true)).child(side("next", false));
    let dots = row().key("dots").w_full().justify_center().gap(6.0).py(2.0).children((0..count).map(|i| {
        let on = i == page;
        let go = go.clone();
        el().key(format!("d{i}"))
            .size(if on { 16.0 } else { 6.0 }, 6.0)
            .radius(3.0)
            .fill(if on { k.c.text_base } else { k.c.border_strong })
            .token_motion(k.motion.control)
            .role("tab")
            .checked(on)
            .label(format!("{} / {count}", i + 1))
            .on_press_tx(move |s: &mut S, tx: &mut Tx| go(s, i, tx))
    }));
    let keys = go.clone();
    col()
        .w_full()
        .gap(6.0)
        .role("group")
        .label(format!("{} / {count}", page + 1))
        .focusable(true)
        .on_key(move |s: &mut S, ev: &KeyEvent, tx: &mut Tx| match ev.key {
            Key::Left if page > 0 => {
                keys(s, page - 1, tx);
                true
            }
            Key::Right if page + 1 < count => {
                keys(s, page + 1, tx);
                true
            }
            _ => false,
        })
        .child(stage)
        .child(dots)
}

/// A round arrow over the stage's edge; faint and inert at the first or last page.
fn arrow(k: &Kit, name: &str, left: bool, enabled: bool) -> El {
    let glyph = icon(k.glyph.caret_right, 12.0).color(k.c.icon_base).icon_rotation(if left { std::f32::consts::PI } else { 0.0 });
    let mut b = el()
        .size(28.0, 28.0)
        .radius(14.0)
        .center()
        .fill(k.c.surface_float)
        .role("button")
        .label(name.to_owned())
        .child(glyph)
        .opacity(if enabled { 1.0 } else { 0.0 })
        .disabled(!enabled);
    for sh in &k.elevation.raised {
        b = b.box_shadow(*sh);
    }
    if enabled { k.interactive(b) } else { b.inert() }
}
