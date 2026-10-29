//! The storybook's Frosted surfaces story (Timeline › Frosted surfaces): the two places Desktop
//! frosts what's under it, each over a reply's words, light and dark side by side. The jump to
//! latest button blurs the words at 2 pt under bg-base at 92%; the drop overlay blurs them at
//! 1.5 pt behind its edge mask and fades in and out over 200 ms. D drags a file over and away.

use std::cell::Cell;

use rune::{El, Play, TextStyle, col, el, row, text};

use crate::App;
use crate::palette::{Pal, pal};
use crate::stories::{Trigger, replay};

pub const KEYS: &[Trigger] = &[replay(open), Trigger { key: 'd', label: "Drag a file over or away", run: toggle }];

const W: f32 = 310.0;
const H: f32 = 200.0;
const BODY: TextStyle = TextStyle::new(14.0, 440.0).lh(22.0);
const WORDS: &str = "The server sends a heartbeat every 10 seconds, but stream.ts only reconnects on an error event. When the machine sleeps, the TCP connection goes half-open: the browser never sees an error, so the client waits forever. Treat silence as failure: if no event or heartbeat arrives for 25 seconds, close the stream and reconnect with backoff.";

thread_local! {
    /// A file drag is over the story's page.
    static DRAGGING: Cell<bool> = const { Cell::new(false) };
}

/// The drag comes over the page, then leaves it.
fn open(_: &mut App, tx: &mut Play<App>) {
    DRAGGING.set(false);
    tx.after(0.6, toggle);
    tx.after(2.2, toggle);
}

fn toggle(_: &mut App, tx: &mut Play<App>) {
    tx.animated();
    DRAGGING.set(!DRAGGING.get());
}

pub fn view(s: &App, _: &Pal) -> El {
    row()
        .key("story-frosted")
        .gap(16.0)
        .child(column(s, pal(false), "light"))
        .child(column(s, pal(true), "dark"))
}

fn column(s: &App, p: &Pal, name: &str) -> El {
    let jump = page(p, "jump").child(
        row().pin(Some(0.0), None, Some(0.0), Some(16.0)).center().child(crate::jump::face(p, s.knobs.get(&crate::knobs::CAPSULE_RADIUS))),
    );
    let drop = page(p, "drop").child(crate::dropzone::layer(p, (W, H), DRAGGING.get(), s.reduced_motion));
    col().key(format!("frosted-{name}")).gap(12.0).child(jump).child(drop)
}

/// A page of a reply's words, the surface each frosts.
fn page(p: &Pal, key: &str) -> El {
    el().key(key)
        .size(W, H)
        .radius(10.0)
        .fill(p.bg_base)
        .border(0.5, p.ring)
        .clip_rounded(10.0)
        .child(col().w_full().px(16.0).py(12.0).child(text(format!("{WORDS} {WORDS}")).font(BODY).color(p.text).wrap().w_full()))
}

rune_register::register!(crate::stories::STORY_LIST: crate::stories::Story = [
    crate::stories::Story { path: "Timeline/Frosted surfaces", scene: |s| crate::stories::watch(s, view), knobs: &[], keys: KEYS, variants: &[] },
]);
