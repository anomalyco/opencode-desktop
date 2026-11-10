//! Dictation through Hex: the mic in the composer's toolbar, or ⇧⌘M, records and puts the words
//! at the caret, ready to edit. A press starts and a second press (or the stop) finishes;
//! holding ⇧⌘M past `HOLD` talks until it comes up. Esc throws the recording away. Send or ⏎
//! while it listens finishes and sends with the words, as the iOS app's Send does; otherwise
//! nothing is sent.
//!
//! It looks like the OpenCode iOS app's composer: while it listens the mic is a quiet stop beside
//! Send, which shows a spinner while the words come; "Preparing dictation…", the words so far,
//! and what went wrong sit under the toolbar. `hex_client::dictate` runs each dictation on its
//! own thread over whatever the running Hex offers (its capture API, on the microphone chosen in
//! Hex, with the words so far where its model streams; on a Windows Hex from before that API, a
//! live stream from the app's own microphone, which also sends the words so far). Hex pastes nothing,
//! so the composer keeps its focus. An installed Hex that isn't running starts on the press.
//!
//! The mic shows while Hex is there (on Windows always, so a press without Hex offers its
//! download). Tests, headless runs and the storybook use a fake Hex unless `SEER_HEX` names an
//! endpoint file, so they never open the microphone.

use std::path::PathBuf;
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex};

use hex_client::{Cmd, Fix, Heard, Presence, Problem};
use rune::machine::{Attempt, Seq};
use rune::{El, Icon, Key, KeyEvent, Tx, el, icon, row, text};

use crate::App;
use crate::palette::Pal;
use crate::view::{UI_SMALL, activity_ring_sized};

/// Who Hex says is recording.
const SOURCE: &str = "OpenCode";
/// Holding the shortcut longer than this is push-to-talk: the recording ends when it comes up.
const HOLD: f64 = 0.3;
/// "Preparing dictation…" shows once Hex has taken this long to start, so a quick start shows
/// nothing but the stop.
const PREPARING_AFTER: f64 = 0.25;
/// Seconds between looks for Hex while idle.
const LOOK_SECS: f64 = 2.0;
/// The fake's transcription time.
const FAKE_TRANSCRIBE: f64 = 0.6;
/// The fake's start when the storybook asks for a slow one.
const FAKE_SLOW_START: f64 = 1.2;
pub const FAKE_WORDS: &str = "keep the retry budget as it is and add a test for the reconnect";

const MIC: Icon = Icon {
    name: "outline-mic",
    view: 16.0,
    body: r#"<path d="M8 1.5C6.89543 1.5 6 2.39543 6 3.5V8C6 9.10457 6.89543 10 8 10C9.10457 10 10 9.10457 10 8V3.5C10 2.39543 9.10457 1.5 8 1.5Z" stroke="currentColor"/><path d="M3.5 7.5V8C3.5 10.4853 5.51472 12.5 8 12.5C10.4853 12.5 12.5 10.4853 12.5 8V7.5" stroke="currentColor" stroke-linecap="square"/><path d="M8 12.5V14.5" stroke="currentColor" stroke-linecap="square"/>"#,
};

enum Backend {
    /// In-process: ready after `Dictation::prepare`, words `FAKE_TRANSCRIBE` after the stop.
    Fake,
    /// The real Hex: its endpoint file, and its data folder (where Windows' Hex is installed).
    Hex { file: PathBuf, support: PathBuf },
}

impl Backend {
    fn detect() -> Backend {
        let headless = crate::headless() || crate::arg("--scene").is_some() || crate::arg("--fixture").is_some();
        if std::env::var_os("SEER_HEX").is_none() && (cfg!(test) || headless) {
            return Backend::Fake;
        }
        let support = crate::dirs::hex_support().unwrap_or_default();
        Backend::Hex { file: hex_client::discovery(&support), support }
    }

    fn presence(&self) -> Presence {
        match self {
            Backend::Fake => Presence::Running,
            Backend::Hex { file, support } => hex_client::presence(file, support),
        }
    }
}

/// Where a dictation is, the machine every press, key and word from Hex goes through ([`step`]).
/// A press goes `Idle` (or `Failed`) → `Starting` → `Listening` → `Finishing` → `Idle` with the
/// words at the caret; Esc goes back to `Idle` from any of the three, and Hex or the microphone
/// refusing goes to `Failed`. Desktop has no dictation of its own; the phases are the iOS
/// app's, and each carries only what it needs: a recording's attempt fences what an earlier
/// one's thread still says, and a held shortcut's press time lives only while the microphone
/// is open.
enum Phase {
    Idle,
    /// Pressed: Hex is starting, or taking the microphone. `slow` once that has taken
    /// `PREPARING_AFTER`, so "Preparing dictation…" shows.
    Starting { rec: Rec, slow: bool, hold: Option<f64> },
    /// Hex is listening; `heard` is the words so far, when its model sends them. `hold`: when
    /// the shortcut that started it went down, while it is still held.
    Listening { rec: Rec, heard: Option<Words>, hold: Option<f64> },
    /// Stopped at `stopped`; the words are coming, and go out with a send when Send or ⏎ (`Some`,
    /// true for ⌘⏎) finished it. The words so far stay until they land.
    Finishing { rec: Rec, heard: Option<Words>, stopped: f64, send: Option<bool> },
    /// What went wrong, until the next press or send (as iOS keeps it).
    Failed(Problem),
}

/// Words so far: `stable` won't change, `tentative` may.
#[derive(Clone)]
struct Words {
    stable: String,
    tentative: String,
}

/// One recording: its attempt, when the press started it, and how to reach its Hex thread
/// (`None` with the fake).
struct Rec {
    attempt: Attempt,
    since: f64,
    cmd: Option<Sender<Cmd>>,
}

pub struct Dictation {
    backend: Backend,
    /// Where Hex was at the last look, and when that was.
    presence: Presence,
    looked: f64,
    phase: Phase,
    /// What Hex's threads heard, tagged with their recording.
    heard: Arc<Mutex<Vec<(Attempt, Heard)>>>,
    seq: Seq,
    /// Seconds the fake takes to start.
    prepare: f64,
}

impl Default for Dictation {
    fn default() -> Self {
        Self::with(Backend::detect())
    }
}

impl Dictation {
    fn with(backend: Backend) -> Dictation {
        let presence = backend.presence();
        Dictation { backend, presence, looked: f64::NEG_INFINITY, phase: Phase::Idle, heard: Arc::default(), seq: Seq::default(), prepare: 0.0 }
    }

    /// The storybook's Hex, whatever the app runs on.
    pub fn fake() -> Dictation {
        Self::with(Backend::Fake)
    }

    /// The real Hex behind this endpoint file (tests point it at a stand-in).
    #[cfg(test)]
    pub fn at(file: PathBuf) -> Dictation {
        let support = file.parent().map(PathBuf::from).unwrap_or_default();
        Self::with(Backend::Hex { file, support })
    }

    fn rec(&self) -> Option<&Rec> {
        match &self.phase {
            Phase::Starting { rec, .. } | Phase::Listening { rec, .. } | Phase::Finishing { rec, .. } => Some(rec),
            Phase::Idle | Phase::Failed(_) => None,
        }
    }

    /// The microphone is open (or opening): starting or listening.
    pub fn recording(&self) -> bool {
        matches!(self.phase, Phase::Starting { .. } | Phase::Listening { .. })
    }

    /// A recording is on: preparing, listening or finishing.
    pub fn active(&self) -> bool {
        self.rec().is_some()
    }

    pub fn error(&self) -> Option<&str> {
        match &self.phase {
            Phase::Failed(p) => Some(&p.says),
            _ => None,
        }
    }

    /// The mic shows while Hex is there, or always where a press offers its download.
    fn shows_mic(&self) -> bool {
        self.presence != Presence::Missing || hex_client::MIC_WITHOUT_HEX || self.active()
    }

    /// The fake starts slowly from now on, as a Hex loading its model would.
    pub fn slow_fake(&mut self) {
        self.prepare = FAKE_SLOW_START;
    }

    #[cfg(test)]
    pub fn preparing_shown(&self) -> bool {
        matches!(self.phase, Phase::Starting { slow: true, .. })
    }

    fn fail(&mut self, why: Problem) {
        self.cancel_thread();
        self.phase = Phase::Failed(why);
    }

    fn cancel_thread(&self) {
        if let Some(cmd) = self.rec().and_then(|r| r.cmd.as_ref()) {
            let _ = cmd.send(Cmd::Cancel);
        }
    }
}

/// What reaches the machine.
enum Event {
    /// The mic or the shortcut: starts, or finishes the recording that listens. `held`: the
    /// shortcut went down, so a long hold is push-to-talk.
    Toggle { now: f64, held: bool },
    /// The shortcut came up.
    Release { now: f64 },
    /// Send or ⏎ (`alternate` for ⌘⏎).
    Submit { now: f64, alternate: bool },
    Escape,
    /// Hex's thread (or the fake) heard something for `attempt`.
    Heard { attempt: Attempt, heard: Heard },
}

/// What the caller does about it.
enum Effect {
    /// Start a recording: the Hex thread for `attempt`, or the fake.
    Start(Attempt),
    /// Tell the recording's Hex thread to stop (finish) and hand over the words.
    Stop,
    /// Put the words at the caret, then send them when `Some` (true for ⌘⏎).
    Words { words: String, send: Option<bool> },
}

/// Whether the machine took the event.
fn step(d: &mut Dictation, event: Event) -> (bool, Option<Effect>) {
    match (std::mem::replace(&mut d.phase, Phase::Idle), event) {
        (Phase::Idle | Phase::Failed(_), Event::Toggle { now, held }) => {
            d.presence = d.backend.presence();
            if d.presence == Presence::Missing {
                d.phase = Phase::Failed(if hex_client::MIC_WITHOUT_HEX { Problem::not_installed() } else { Problem::of(hex_client::NOT_RUNNING) });
                return (true, None);
            }
            let attempt = d.seq.next();
            let rec = Rec { attempt, since: now, cmd: None };
            let hold = held.then_some(now);
            let fake_ready = matches!(d.backend, Backend::Fake) && d.prepare <= 0.0;
            d.phase = if fake_ready { Phase::Listening { rec, heard: None, hold } } else { Phase::Starting { rec, slow: false, hold } };
            (true, Some(Effect::Start(attempt)))
        }
        (Phase::Starting { rec, .. }, Event::Toggle { now, .. }) => finish(d, rec, None, now, None),
        (Phase::Listening { rec, heard, .. }, Event::Toggle { now, .. }) => finish(d, rec, heard, now, None),
        (Phase::Starting { rec, .. }, Event::Submit { now, alternate }) => finish(d, rec, None, now, Some(alternate)),
        (Phase::Listening { rec, heard, .. }, Event::Submit { now, alternate }) => finish(d, rec, heard, now, Some(alternate)),
        // A hold past `HOLD` was push-to-talk, so the recording finishes as the key comes up.
        (Phase::Starting { rec, hold: Some(at), .. }, Event::Release { now }) if now - at >= HOLD => finish(d, rec, None, now, None),
        (Phase::Listening { rec, heard, hold: Some(at) }, Event::Release { now }) if now - at >= HOLD => finish(d, rec, heard, now, None),
        (Phase::Starting { rec, slow, hold: Some(_) }, Event::Release { .. }) => {
            d.phase = Phase::Starting { rec, slow, hold: None };
            (true, None)
        }
        (Phase::Listening { rec, heard, hold: Some(_) }, Event::Release { .. }) => {
            d.phase = Phase::Listening { rec, heard, hold: None };
            (true, None)
        }
        // While the words come, the mic and Send wait for them.
        (phase @ Phase::Finishing { .. }, Event::Toggle { .. } | Event::Submit { .. }) => {
            d.phase = phase;
            (true, None)
        }
        // A send clears what went wrong, as iOS's does, and goes ahead.
        (Phase::Failed(_), Event::Submit { .. }) => (false, None),
        (phase @ (Phase::Starting { .. } | Phase::Listening { .. } | Phase::Finishing { .. }), Event::Escape) => {
            d.phase = phase;
            d.cancel_thread();
            d.phase = Phase::Idle;
            (true, None)
        }
        (phase, Event::Heard { attempt, heard }) => {
            if d_attempt(&phase) != Some(attempt) {
                d.phase = phase;
                return (false, None);
            }
            hear(d, phase, heard)
        }
        (phase, _) => {
            d.phase = phase;
            (false, None)
        }
    }
}

/// Into `Finishing`: the microphone stops and the words are coming.
fn finish(d: &mut Dictation, rec: Rec, heard: Option<Words>, now: f64, send: Option<bool>) -> (bool, Option<Effect>) {
    d.phase = Phase::Finishing { rec, heard, stopped: now, send };
    (true, Some(Effect::Stop))
}

fn d_attempt(phase: &Phase) -> Option<Attempt> {
    match phase {
        Phase::Starting { rec, .. } | Phase::Listening { rec, .. } | Phase::Finishing { rec, .. } => Some(rec.attempt),
        Phase::Idle | Phase::Failed(_) => None,
    }
}

/// One thing the recording's thread heard, moving the phase on.
fn hear(d: &mut Dictation, phase: Phase, h: Heard) -> (bool, Option<Effect>) {
    d.phase = match (phase, h) {
        (Phase::Starting { rec, hold, .. }, Heard::Ready) => Phase::Listening { rec, heard: None, hold },
        (Phase::Listening { rec, hold, .. }, Heard::Partial { stable, tentative }) => Phase::Listening { rec, heard: Some(Words { stable, tentative }), hold },
        (Phase::Finishing { rec, stopped, send, .. }, Heard::Partial { stable, tentative }) => Phase::Finishing { rec, heard: Some(Words { stable, tentative }), stopped, send },
        (phase, Heard::Words(words)) => {
            let send = match phase {
                Phase::Finishing { send, .. } => send,
                _ => None,
            };
            return (true, Some(Effect::Words { words, send }));
        }
        (_, Heard::Failed(why)) => Phase::Failed(why),
        (phase, _) => phase,
    };
    (true, None)
}

pub fn start(s: &mut App) {
    if s.dictation.active() {
        return;
    }
    let now = s.clock;
    run(s, Event::Toggle { now, held: false }, None);
}

/// Stops the microphone; the words land at the caret when Hex has them.
pub fn stop(s: &mut App) {
    if s.dictation.recording() {
        let now = s.clock;
        run(s, Event::Toggle { now, held: false }, None);
    }
}

pub fn cancel(s: &mut App) {
    run(s, Event::Escape, None);
}

/// Steps the machine and carries out its effect; whether it took the event.
fn run(s: &mut App, event: Event, tx: Option<&mut Tx>) -> bool {
    let (took, effect) = step(&mut s.dictation, event);
    match effect {
        Some(Effect::Start(attempt)) => {
            let d = &mut s.dictation;
            if let Backend::Hex { file, support } = &d.backend {
                let (cmd, rx) = mpsc::channel();
                let (file, support, heard) = (file.clone(), support.clone(), d.heard.clone());
                std::thread::spawn(move || {
                    hex_client::dictate(&file, &support, SOURCE, &rx, &mut |h| heard.lock().unwrap_or_else(|e| e.into_inner()).push((attempt, h)));
                });
                if let Phase::Starting { rec, .. } | Phase::Listening { rec, .. } = &mut d.phase {
                    rec.cmd = Some(cmd);
                }
            }
        }
        Some(Effect::Stop) => {
            if let Some(cmd) = s.dictation.rec().and_then(|r| r.cmd.as_ref()) {
                let _ = cmd.send(Cmd::Stop);
            }
        }
        Some(Effect::Words { words, send }) => {
            insert(s, &words);
            if let (Some(alternate), Some(tx)) = (send, tx) {
                s.submit(alternate, tx);
            }
        }
        None => {}
    }
    took
}

/// Send or ⏎ (`alternate` for ⌘⏎): while it listens, finishes and sends with the words, as the
/// iOS app's Send does; while the words come, waits for them. True when dictation took it.
pub fn submit(s: &mut App, alternate: bool) -> bool {
    let now = s.clock;
    if let Phase::Failed(_) = s.dictation.phase {
        s.dictation.phase = Phase::Idle;
        return false;
    }
    run(s, Event::Submit { now, alternate }, None)
}

fn shortcut(ev: &KeyEvent) -> bool {
    crate::keymap::Bind::Dictate.matches(ev)
}

/// ⇧⌘M starts or finishes; while a recording is on, Esc throws it away.
pub fn key(s: &mut App, ev: &KeyEvent, tx: &mut Tx) -> bool {
    if shortcut(ev) {
        if !tx.key_repeat() {
            tx.instant();
            let now = tx.clock();
            run(s, Event::Toggle { now, held: true }, Some(tx));
        }
        return true;
    }
    if ev.key == Key::Escape && s.dictation.active() {
        tx.instant();
        cancel(s);
        return true;
    }
    false
}

/// The shortcut's M coming up (with ⌘ still down, or after it): a hold past `HOLD` was
/// push-to-talk, so the recording finishes.
pub fn key_up(s: &mut App, ev: &KeyEvent, tx: &mut Tx) -> bool {
    let m = matches!(&ev.key, Key::Command('m')) || matches!(&ev.key, Key::Text(t) if t.eq_ignore_ascii_case("m"));
    let held = matches!(s.dictation.phase, Phase::Starting { hold: Some(_), .. } | Phase::Listening { hold: Some(_), .. });
    if !m || !held {
        return false;
    }
    let now = tx.clock();
    let was = s.dictation.recording();
    run(s, Event::Release { now }, Some(tx));
    if was && !s.dictation.recording() {
        tx.instant();
    }
    true
}

/// Each frame: looks for Hex, follows Hex's thread, and puts Hex's words at the caret.
pub fn poll(s: &mut App, now: f64, tx: &mut Tx) -> bool {
    let d = &mut s.dictation;
    let mut changed = false;
    if !d.active() && now - d.looked >= LOOK_SECS {
        d.looked = now;
        let presence = d.backend.presence();
        changed |= presence != d.presence;
        d.presence = presence;
    }
    let Some(rec) = d.rec() else { return changed };
    let (attempt, since, fake) = (rec.attempt, rec.since, rec.cmd.is_none());
    let mut heard: Vec<Heard> = d.heard.lock().unwrap_or_else(|e| e.into_inner()).drain(..).filter(|(a, _)| *a == attempt).map(|(_, h)| h).collect();
    let starting = matches!(d.phase, Phase::Starting { .. });
    // The fake answers on its own clock: ready after `prepare`, words `FAKE_TRANSCRIBE` after
    // the stop.
    let words_due = match &d.phase {
        Phase::Finishing { stopped, .. } if fake => Some(stopped + FAKE_TRANSCRIBE),
        _ => None,
    };
    if fake && starting && now - since >= d.prepare {
        heard.push(Heard::Ready);
    }
    if words_due.is_some_and(|t| now >= t) {
        heard.push(Heard::Words(std::env::var("SEER_FAKE_WORDS").unwrap_or_else(|_| FAKE_WORDS.into())));
    }
    // Hex's thread can't wake the app, so look again soon; and on time for the slow-start line
    // and the fake's next step.
    let mut next = now + 0.1;
    if starting {
        next = next.min(since + PREPARING_AFTER);
        if fake {
            next = next.min(since + d.prepare);
        }
    }
    if let Some(t) = words_due {
        next = next.min(t);
    }
    tx.wake_at(next.max(now + 0.001));
    if let Phase::Starting { slow, .. } = &mut d.phase
        && !*slow
        && now - since >= PREPARING_AFTER
    {
        *slow = true;
        changed = true;
    }
    for h in heard {
        changed = true;
        run(s, Event::Heard { attempt, heard: h }, Some(tx));
    }
    changed
}

/// The words at the caret, replacing a selection, with a space from the text either side.
fn insert(s: &mut App, words: &str) {
    let words = words.trim();
    if words.is_empty() {
        return;
    }
    let c = &mut s.composer;
    let (start, end) = (c.anchor.min(c.focus), c.anchor.max(c.focus));
    let before = c.text[..start.min(c.text.len())].chars().next_back().is_some_and(|ch| !ch.is_whitespace());
    let after = c.text.get(end..).and_then(|t| t.chars().next()).is_some_and(|ch| !ch.is_whitespace());
    let mut put = String::new();
    if before {
        put.push(' ');
    }
    put.push_str(words);
    if after {
        put.push(' ');
    }
    c.insert(&put);
}

/// What sits before Send, as in the iOS app's composer: the mic, a quiet stop while it listens,
/// and a spinner in the stop while the words come. Nothing while Hex isn't there (except on
/// Windows, where the mic offers Hex's download).
pub fn toolbar(s: &App, p: &Pal) -> Option<El> {
    let d = &s.dictation;
    if !d.shows_mic() {
        return None;
    }
    let finishing = matches!(d.phase, Phase::Finishing { .. });
    // The mic and the stop swap at once on the press, as iOS's do.
    let still = rune::Spring::NONE;
    let mut b = el()
        .key("mic")
        .role("button")
        .size(28.0, 28.0)
        .radius(6.0)
        .center()
        .motion(still)
        .motion_intent(rune::MotionIntent::CUT)
        .preserve_focus_on_press(true)
        // Desktop's composer has no Tab stop after its variant select: Tab goes on to the
        // titlebar, so the mic keeps out of the Tab order (its shortcut still toggles it).
        .tab_index(-1)
        // Inside the composer's clip its keyboard ring sits 2 in (`editor.css:70-73`).
        .focus_ring(2.0, -2.0, p.v2.border_focus);
    b = match &d.phase {
        Phase::Idle | Phase::Failed(_) => b.label(opencode_i18n::t!("rune.command.prompt.dictate")).help(opencode_i18n::t!("rune.command.prompt.dictate")).help_keys(crate::keymap::Bind::Dictate.keycaps()).child(icon(&MIC, 16.0).color(p.icon_muted).motion(still)),
        Phase::Finishing { .. } => b.label(opencode_i18n::t!("rune.dictation.finishing")).fill(p.layer2).child(activity_ring_sized(p, 14.0)),
        // iOS's stop.fill in a tertiary fill; the same square as the run's Stop.
        Phase::Starting { .. } | Phase::Listening { .. } => b.label(opencode_i18n::t!("rune.dictation.stop")).help(opencode_i18n::t!("rune.dictation.stop")).help_keys(crate::keymap::Bind::Dictate.keycaps()).fill(p.layer2)
            .child(el().key("stop-glyph").size(10.0, 10.0).radius(2.0).fill(p.text).motion(still)),
    };
    if !finishing {
        b = b.hover(p.hover).press(p.pressed).on_press_tx(|s: &mut App, tx: &mut Tx| {
            tx.instant();
            let now = tx.clock();
            run(s, Event::Toggle { now, held: false }, Some(tx));
        });
    }
    // Its slot rides the composer's edge on the panel's spring; the button's own swap lands at once.
    Some(el().key("mic-slot").size(28.0, 28.0).child(b))
}

/// Under the toolbar, as in iOS: "Preparing dictation…" while Hex is slow to start, the words so
/// far while it listens, or what went wrong with what to do about it.
pub fn status(s: &App, p: &Pal) -> Option<El> {
    let line = row().key("dictation-status").px(16.0).pb(12.0).gap(8.0).center_y();
    match &s.dictation.phase {
        Phase::Starting { slow: true, .. } => Some(line.child(activity_ring_sized(p, 12.0)).child(text(opencode_i18n::t!("rune.dictation.preparing")).font(UI_SMALL).color(p.muted))),
        Phase::Listening { heard: Some(w), .. } | Phase::Finishing { heard: Some(w), .. } => heard_line(line, p, w),
        Phase::Failed(why) => {
            // In a narrow composer the reason gives way, so its fix stays whole.
            let line = line.child(text(why.says.clone()).font(UI_SMALL).color(p.danger).truncate());
            Some(match why.fix {
                Some(fix) => line.child(fix_button(p, fix)),
                None => line,
            })
        }
        _ => None,
    }
}

/// The words so far: the settled text, cut at its start when the line is full, then the faint
/// tail Hex may still change.
fn heard_line(line: El, p: &Pal, w: &Words) -> Option<El> {
    if w.stable.is_empty() && w.tentative.is_empty() {
        return None;
    }
    let words = row().key("dictation-heard").shrink()
        .child(text(w.stable.clone()).font(UI_SMALL).color(p.muted).truncate_head())
        .child(text(w.tentative.clone()).font(UI_SMALL).color(p.faint));
    Some(line.child(words))
}

/// What a problem offers: Hex's download, Hex's window, or the microphone privacy page.
fn fix_button(p: &Pal, fix: Fix) -> El {
    row().key("dictation-fix").role("button").label(fix.label().to_owned()).h(20.0).px(4.0).center_y().radius(4.0).hover(p.hover).press(p.pressed)
        .preserve_focus_on_press(true)
        .on_press_tx(move |s: &mut App, tx: &mut Tx| {
            tx.instant();
            // The fake (storybook, headless) opens nothing.
            let Backend::Hex { support, .. } = &s.dictation.backend else { return };
            match fix {
                Fix::Install => {
                    let _ = tx.open_url(hex_client::DOWNLOAD);
                }
                Fix::OpenHex => {
                    if let Err(why) = hex_client::launch(&support.clone(), true) {
                        s.dictation.fail(Problem { says: why, fix: None });
                    }
                }
                Fix::MicSettings => hex_client::open_mic_settings(),
            }
        })
        .child(text(fix.label()).font(UI_SMALL).color(p.text))
}

// The storybook's "Composer › Dictation", on the fake Hex.

pub const STORY_KEYS: &[crate::stories::Trigger] = &[
    crate::stories::replay(story_open),
    crate::stories::Trigger { key: 's', label: "Start", run: |s, tx| {
        tx.instant();
        s.dictation.prepare = 0.0;
        start(s);
    } },
    crate::stories::Trigger { key: 'p', label: "Slow start", run: |s, tx| {
        tx.instant();
        cancel(s);
        s.dictation.slow_fake();
        start(s);
    } },
    crate::stories::Trigger { key: 'w', label: "Words so far", run: |s, tx| {
        tx.instant();
        if !s.dictation.recording() {
            s.dictation.prepare = 0.0;
            start(s);
        }
        let attempt = s.dictation.seq.current();
        s.dictation.heard.lock().unwrap_or_else(|e| e.into_inner()).extend([
            (attempt, Heard::Ready),
            (attempt, Heard::Partial { stable: "keep the retry budget as it is".into(), tentative: " and add a".into() }),
        ]);
    } },
    crate::stories::Trigger { key: 'f', label: "Finish", run: |s, tx| {
        tx.instant();
        stop(s);
    } },
    crate::stories::Trigger { key: 'e', label: "Next error", run: |s, tx| {
        tx.instant();
        let next = match s.dictation.error() {
            Some(hex_client::NOT_RUNNING) => hex_client::BUSY,
            Some(hex_client::BUSY) => hex_client::OUTDATED,
            Some(hex_client::OUTDATED) => hex_client::NOT_INSTALLED,
            Some(hex_client::NOT_INSTALLED) => hex_client::MIC_BLOCKED,
            _ => hex_client::NOT_RUNNING,
        };
        cancel(s);
        s.dictation.fail(Problem::of(next));
    } },
];

/// A draft with the caret after it, then the loop iOS shows: the stop while it listens, the
/// spinner while the words come, the words at the caret.
fn story_open(s: &mut App, tx: &mut rune::Play<App>) {
    s.dictation = Dictation::fake();
    s.composer = rune::TextEdit::new("Looks right. Before you merge,");
    start(s);
    tx.after(1.6, |s, tx| {
        tx.instant();
        stop(s);
    });
}

rune_register::register!(crate::stories::STORY_LIST: crate::stories::Story = [
    crate::stories::Story::composer("Composer/Dictation", STORY_KEYS),
]);
