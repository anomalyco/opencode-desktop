//! Reference read-only [`TimelineHost`] and storybook scenes for `crates/session-view` (`p255`).
//!
//! Stores its [`Transcript`] in a [`RefCell`] and feeds virtual rows through
//! [`TimelineHost::with_transcript`], mirroring how Seer (`apps/sketch/src/session_view.rs`) holds
//! its live and fixture transcripts.

use std::cell::RefCell;

use opencode_kit::model::{Attachment, Message, Part, Transcript};
use rune::{Cx, El, Key, KeyEvent, Play, TextEdit, Tx, Waker, col, el};
use rune_knobs::Knobs;

use crate::composer::attachments::push_unique;
use crate::composer::history::{Entry, PromptHistory, arrow_up};
use crate::composer::{ComposerHost, dock, is_stop_shortcut, land};
use crate::arrivals::{Arrivals, Seen};
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
    pub show_composer: bool,
    pub busy: bool,
    pub edit: TextEdit,
    pub files: Vec<Attachment>,
    pub history: PromptHistory<String>,
    pub theme: String,
    pub talk: RefCell<Option<Transcript>>,
    pub waker: Option<Waker>,
    pub view: ViewState,
    pub arrivals: Arrivals,
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
            show_composer: false,
            busy: false,
            edit: TextEdit::new(""),
            files: Vec::new(),
            history: PromptHistory::default(),
            theme: String::new(),
            talk: RefCell::new(Some(transcript(base_ms, false))),
            waker: None,
            view: ViewState::default(),
            arrivals: Arrivals::default(),
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
        self.show_composer = false;
        self.fold_user = self.variant != 2;
        self.busy = streaming;
        self.view.expanded.clear();
        *self.talk.borrow_mut() = Some(transcript(self.now_ms, streaming));
    }

    pub fn open_composer(&mut self) {
        self.reload(self.variant == 1);
        self.show_composer = true;
        self.edit = TextEdit::new("");
        self.files.clear();
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
    fn active_session_id(&self) -> Option<&str> {
        Some(SESSION_ID)
    }
    fn with_transcript<R>(&self, session: &str, f: impl FnOnce(&Transcript) -> R) -> Option<R> {
        let borrow = self.talk.borrow();
        borrow.as_ref().filter(|t| t.session == session).map(f)
    }
    fn is_session_live(&self, session: &str) -> bool {
        session == SESSION_ID && self.busy
    }
    fn fold_user_messages(&self) -> bool {
        self.fold_user
    }
    fn user_glow_at(&self, id: &str) -> Option<f64> {
        self.arrivals.glow_at(id)
    }
    crate::delegate_view_state!(view);
}

impl ComposerHost for Host {
    fn composer_edit(&self) -> &TextEdit {
        &self.edit
    }
    fn composer_edit_mut(&mut self) -> &mut TextEdit {
        &mut self.edit
    }
    fn composer_busy(&self) -> bool {
        self.busy
    }
    fn composer_send(&mut self, tx: &mut Tx) {
        let text = self.edit.text.trim().to_owned();
        if text.is_empty() && self.files.is_empty() {
            return;
        }
        let mid = opencode_kit::client::message_id();
        let entry = Entry::of(&self.edit);
        let files = std::mem::take(&mut self.files);
        if !files.is_empty() {
            opencode_kit::model::set_user_files(&mid, files);
        }
        if let Some(t) = self.talk.borrow_mut().as_mut() {
            t.messages.push(Message::User { id: mid.clone(), text });
        }
        let focused = self.edit.focused;
        self.edit = TextEdit::new("");
        self.edit.focused = focused;
        self.history.add(entry, false);
        land(self, SESSION_ID, &mid, "prompt", tx);
    }
    fn composer_stop(&mut self, tx: &mut Tx) {
        self.busy = false;
        tx.instant();
    }
    fn composer_attachments(&self) -> &[Attachment] {
        &self.files
    }
    fn composer_remove_attachment(&mut self, id: &str) {
        self.files.retain(|f| crate::images::identity(f) != id);
    }
    fn composer_paste_attachment(&mut self, a: Attachment) -> bool {
        push_unique(&mut self.files, a);
        true
    }
    fn composer_on_key(&mut self, ev: &KeyEvent, tx: &mut Tx) -> bool {
        if self.busy && is_stop_shortcut(ev) {
            self.composer_stop(tx);
            return true;
        }
        if let Some(up) = arrow_up(ev) {
            let scope = SESSION_ID.to_owned();
            if self.history.step(&mut self.edit, &scope, false, up) {
                tx.instant();
                return true;
            }
        }
        false
    }
}

pub fn setup(s: &mut Host, waker: Waker) {
    let headless = std::env::args().any(|a| a == "--snap" || a == "--trace" || a == "--frames");
    s.waker = Some(waker.clone());
    s.view.setup(waker, headless);
}

pub fn poll(s: &mut Host, now: f64, tx: &mut Tx) -> bool {
    s.clock = now;
    s.now_ms = s.base_ms + (now * 1000.0) as i64;
    s.arrivals.poll(&s.knobs, now, tx) | crate::poll(s, now, tx)
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

/// Renders the read-only session timeline inside a centered panel stage, at `s.zoom` (`Cx::zoom`).
pub fn view(s: &Host, cx: &mut Cx<Host>) -> El {
    cx.zoom(s.zoom);
    crate::themes::set_active(&s.theme);
    let p: &Pal = pal(is_dark(cx));
    let body = if s.show_composer {
        s.with_transcript(SESSION_ID, |t| crate::timeline_area(s, p, SESSION_ID, Some(t), Vec::new()))
            .unwrap_or_else(|| crate::timeline_area(s, p, SESSION_ID, None, Vec::new()))
    } else {
        s.with_transcript(SESSION_ID, |t| crate::read_only(s, p, SESSION_ID, Some(t)))
            .unwrap_or_else(|| crate::read_only(s, p, SESSION_ID, None))
    };
    let card = col()
        .key("session-stage")
        .w_full()
        .max_w(820.0)
        .h_full()
        .radius(10.0)
        .fill(p.bg_base)
        .box_shadow(Pal::ring_out(p.ring))
        .shadow(1.0, 2.0, p.shadow)
        .clip_soft(0.0)
        .child(body)
        .children(s.show_composer.then(|| dock(s, p)));
    let inner = col()
        .key("stage-bg")
        .w_full()
        .h_full()
        .fill(p.bg_deep)
        .padding(16.0)
        .center()
        .child(card)
        .children(crate::overlays(s, p, cx));
    el().key("zoom")
        .w_full()
        .h_full()
        .center()
        .hover_motion_within(INSTANT, INSTANT)
        .child(el().key("zoomed").w_full().h_full().child(inner))
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

pub fn open_composer_story(s: &mut Host, _tx: &mut Play<Host>) {
    s.open_composer();
}

/// The motion story's start: a reply just begun streaming.
pub fn start_motion(s: &mut Host) {
    s.fold_user = true;
    s.view.expanded.clear();
    *s.talk.borrow_mut() = Some(crate::fixture::motion(s.now_ms));
}

/// Adds `more` to the streaming reply's text.
pub fn stream(s: &mut Host, more: &str) {
    if let Some(t) = s.talk.borrow_mut().as_mut()
        && let Some(Message::Assistant(a)) = t.messages.last_mut()
        && let Some(Part::Text { text }) = a.parts.last_mut()
    {
        text.push_str(more);
    }
}

/// Ends the turn and adds a prompt sent from another client, which glows as it lands.
pub fn arrive(s: &mut Host) {
    let at = s.now_ms;
    if let Some(t) = s.talk.borrow_mut().as_mut() {
        let seen = Seen::of(t);
        if let Some(Message::Assistant(a)) = t.messages.last_mut() {
            a.completed = Some(at);
        }
        t.messages.extend(opencode_kit::model::parse_messages(&serde_json::json!({"data": [
            {"id": "msg_m3", "type": "user", "time": {"created": at}, "text": "Nice. Does Seer get it too?"},
        ]})));
        s.arrivals.landed(seen.as_ref(), t, s.clock);
    }
}

/// The motion story: the reply streams in a chunk at a time, the tool group opens and shuts,
/// and a prompt arrives from another client. R replays.
pub fn open_motion(s: &mut Host, tx: &mut Play<Host>) {
    start_motion(s);
    for (i, more) in crate::fixture::MOTION_STREAM.iter().enumerate() {
        tx.after(0.6 + 0.5 * i as f64, move |s, _| stream(s, more));
    }
    tx.after(3.0, |s, tx| crate::disclosure::toggle_group::<Host>("c-prt_m1".into())(s, &mut **tx));
    tx.after(4.2, |s, tx| crate::disclosure::toggle_group::<Host>("c-prt_m1".into())(s, &mut **tx));
    tx.after(5.2, |s, _| arrive(s));
}

pub const SCENES: &[(&str, fn(&mut Host))] = &[
    ("read-only", |s| s.reload(false)),
    ("composer", |s| s.open_composer()),
    ("Session view/Motion", start_motion),
    ("Session view/Read-only", |s| s.reload(s.variant == 1)),
    ("Session view/Composer", |s| s.open_composer()),
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

