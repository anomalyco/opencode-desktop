//! Reference read-only [`TimelineHost`] and storybook scenes for `crates/session-view` (`p255`).
//!
//! Stores its [`Transcript`] in a [`RefCell`] and feeds virtual rows through
//! [`TimelineHost::with_transcript`], mirroring how Seer (`apps/sketch/src/session_view.rs`) holds
//! its live and fixture transcripts.

use std::cell::RefCell;

use opencode_kit::model::{Message, Transcript};
use rune::{Cx, El, Key, KeyEvent, Play, Tx, Waker, col, el};
use rune_knobs::Knobs;

use crate::fixture::{SESSION_ID, transcript};
use crate::palette::{Pal, pal};
use crate::style::INSTANT;
use crate::{TimelineHost, ViewState};

pub const DIAGRAM_SRC: &str = "graph LR\n  Kit[opencode-kit] --> View[session-view]\n  View --> Desktop[opencode-desktop]\n  View --> Seer[seer-sketch]\n";

/// Standalone read-only host state for scenes, stories, and integration tests.
pub struct Host {
    pub knobs: Knobs,
    pub clock: f64,
    pub now_ms: i64,
    pub base_ms: i64,
    pub zoom: f32,
    pub fold_user: bool,
    pub variant: usize,
    pub theme: String,
    pub talk: RefCell<Option<Transcript>>,
    pub waker: Option<Waker>,
    pub view: ViewState,
}

impl Default for Host {
    fn default() -> Self {
        let base_ms = 1_700_000_000_000;
        Self {
            knobs: Knobs::default(),
            clock: 0.0,
            now_ms: base_ms,
            base_ms,
            zoom: 1.0,
            fold_user: true,
            variant: 0,
            theme: String::new(),
            talk: RefCell::new(Some(transcript(base_ms, false))),
            waker: None,
            view: ViewState::default(),
        }
    }
}

impl Host {
    /// Initializes a host from CLI flags (`--zoom`, `--theme`).
    pub fn from_args() -> Self {
        let zoom = arg("--zoom")
            .and_then(|z| z.parse::<f32>().ok())
            .map_or(1.0, |z| z.clamp(0.5, 3.0));
        let mut h = Self { zoom, ..Self::default() };
        if let Some(th) = arg("--theme")
            && th != "dark"
            && th != "light"
        {
            h.theme = th;
        }
        h
    }

    pub fn reload(&mut self, streaming: bool) {
        self.fold_user = self.variant != 2;
        self.view.expanded.clear();
        self.view.shutting.clear();
        *self.talk.borrow_mut() = Some(transcript(self.now_ms, streaming));
    }

    pub fn scroll_key() -> String {
        format!("scroll-{SESSION_ID}")
    }
}

impl TimelineHost for Host {
    fn knobs(&self) -> &Knobs {
        &self.knobs
    }
    fn clock(&self) -> f64 {
        self.clock
    }
    fn now_ms(&self) -> i64 {
        self.now_ms
    }
    fn zoom(&self) -> f32 {
        self.zoom
    }
    fn active_session_id(&self) -> Option<&str> {
        Some(SESSION_ID)
    }
    fn with_transcript<R>(&self, session: &str, f: impl FnOnce(&Transcript) -> R) -> Option<R> {
        let borrow = self.talk.borrow();
        borrow.as_ref().filter(|t| t.session == session).map(f)
    }
    fn fold_user_messages(&self) -> bool {
        self.fold_user
    }
    crate::delegate_view_state!(view);
}

pub fn setup(s: &mut Host, waker: Waker) {
    let headless = std::env::args().any(|a| a == "--snap" || a == "--trace" || a == "--frames");
    s.waker = Some(waker.clone());
    s.view.setup(waker, headless);
}

pub fn poll(s: &mut Host, now: f64, tx: &mut Tx) -> bool {
    s.clock = now;
    s.now_ms = s.base_ms + (now * 1000.0) as i64;
    crate::poll(s, now, tx)
}

pub fn on_key(s: &mut Host, ev: &KeyEvent, tx: &mut Tx) -> bool {
    if ev.key == Key::Escape && s.view.diagram.is_open() {
        crate::diagram::close(s, tx);
        return true;
    }
    false
}

pub fn is_dark(cx: &Cx<Host>) -> bool {
    match arg("--theme").as_deref() {
        Some("light") => false,
        Some("dark") => true,
        _ => cx.system_dark(),
    }
}

/// Renders the read-only session timeline inside a centered panel stage, scaled by `s.zoom`.
pub fn view(s: &Host, cx: &mut Cx<Host>) -> El {
    crate::themes::set_active(&s.theme);
    let p: &Pal = pal(is_dark(cx));
    let body = s
        .with_transcript(SESSION_ID, |t| crate::read_only(s, p, SESSION_ID, Some(t)))
        .unwrap_or_else(|| crate::read_only(s, p, SESSION_ID, None));
    let card = col()
        .key("session-stage")
        .w_full()
        .max_w(820.0)
        .h_full()
        .radius(10.0)
        .fill(p.bg_base)
        .border(0.5, p.ring)
        .shadow(1.0, 2.0, p.shadow)
        .clip_soft(0.0)
        .child(body);
    let inner = col()
        .key("stage-bg")
        .w_full()
        .h_full()
        .fill(p.bg_deep)
        .padding(16.0)
        .center()
        .child(card)
        .children(crate::overlays(s, p, cx));
    let sized = if (s.zoom - 1.0).abs() > 1e-4 {
        let [w, h] = cx.layout_size("zoom").unwrap_or([1280.0, 820.0]);
        if w > 0.0 && h > 0.0 {
            el().w(w / s.zoom).h(h / s.zoom)
        } else {
            el().w_full().h_full()
        }
    } else {
        el().w_full().h_full()
    };
    el().key("zoom")
        .w_full()
        .h_full()
        .center()
        .hover_motion_within(INSTANT, INSTANT)
        .child(sized.key("zoomed").scale(s.zoom).child(inner))
}

pub const READ_ONLY_VARIANTS: &[(&str, usize)] = &[
    ("Settled", 0),
    ("Streaming", 1),
    ("Fold off", 2),
];

pub fn open_read_only(s: &mut Host, _tx: &mut Play<Host>) {
    s.reload(s.variant == 1);
}

pub fn toggle_fold(s: &mut Host, tx: &mut Tx) {
    let last = s.with_transcript(SESSION_ID, |t| {
        t.messages.iter().rev().find_map(|m| match m {
            Message::User { id, text } if text.lines().count() > crate::fold::OVER => Some(id.clone()),
            _ => None,
        })
    }).flatten();
    if let Some(id) = last {
        crate::fold::flip(s, &id, false, tx);
    }
}

pub fn open_scrollbar(s: &mut Host, tx: &mut Play<Host>) {
    s.reload(false);
    tx.after(0.4, |_, tx| tx.scroll_page(Host::scroll_key(), -2));
    tx.after(2.4, |_, tx| tx.scroll_to_bottom(Host::scroll_key()));
}

pub fn open_diagram_story(s: &mut Host, tx: &mut Play<Host>) {
    s.reload(false);
    s.view.diagram = Default::default();
    tx.after(0.4, |s, tx| {
        crate::diagram::open_source(s, DIAGRAM_SRC, tx);
    });
}

pub const SCENES: &[(&str, fn(&mut Host))] = &[
    ("read-only", |s| s.reload(false)),
    ("Session view/Read-only", |s| s.reload(s.variant == 1)),
    ("Session view/Scroll bar", |s| s.reload(false)),
    ("Session view/Diagram", |s| s.reload(false)),
];

fn arg(flag: &str) -> Option<String> {
    let mut args = std::env::args();
    while let Some(a) = args.next() {
        if a == flag {
            return args.next();
        }
    }
    None
}

#[cfg(test)]
#[path = "story_tests.rs"]
mod tests;
