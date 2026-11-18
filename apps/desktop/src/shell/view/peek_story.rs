//! Sidebar › Peek: the closed sidebar peeking over a session from the window's start edge, then
//! sliding back. R replays it; P and L peek and leave by hand, and D docks from the peek.

use super::*;
use crate::layout_state::PeekEvent;
use crate::stories::{Story, Trigger, replay, stage};

const KEYS: &[Trigger] = &[
    replay(open),
    Trigger { key: 'p', label: "Peek", run: |s, tx| s.queue_peek(at_edge(true), tx) },
    Trigger { key: 'l', label: "Leave", run: leave },
    Trigger { key: 'd', label: "Dock from the peek", run: |s, tx| _ = s.toggle_sidebar(tx) },
];

fn at_edge(inside: bool) -> PeekEvent {
    PeekEvent::Edge { inside, can_peek: true, held: false }
}

fn open(s: &mut App, play: &mut rune::Play<'_, App>) {
    crate::open_scene(s);
    s.finish_stream();
    crate::scenes::side_tabs(s);
    *s.sidebar_open = false;
    play.instant();
    play.after(0.6, |s, tx| s.queue_peek(at_edge(true), tx));
    play.after(1.8, leave);
}

fn leave(s: &mut App, tx: &mut rune::Play<'_, App>) {
    s.queue_peek(at_edge(false), tx);
    s.queue_peek(PeekEvent::Corridor { inside: false, held: false }, tx);
}

/// A window's shell in small: the sidebar, the panel beside it, the peek's zones and the toggle.
fn view(s: &App, p: &Pal) -> El {
    let page = match &*s.route {
        Route::Session { id, .. } => session_page(s, p, id),
        _ => el(),
    };
    let panel = col()
        .key("peek-panel")
        .grow(1.0)
        .shrink()
        .ml(8.0)
        .mt(chrome().top())
        .mr(8.0)
        .mb(8.0)
        .radius(10.0)
        .fill(p.bg_base)
        .elevate(p.raised)
        .clip_soft(0.0)
        .child(page);
    row()
        .key("peek-window")
        .w(720.0)
        .h(520.0)
        .radius(10.0)
        .fill(p.bg_deep)
        .box_shadow(Pal::ring_out(p.ring))
        .clip_soft(0.0)
        .child(sidebar(s, p))
        .child(panel)
        .children(peek(s))
        .children(chrome().sidebar_header().then(|| sidebar_toggle(s, p, *s.sidebar_open, *s.sidebar_w)))
}

rune_register::register!(crate::stories::STORY_LIST: Story = [
    Story { path: "Sidebar/Peek", scene: |s| stage(s, view), knobs: &[super::PEEK_KNOBS], keys: KEYS, variants: &[] },
]);
