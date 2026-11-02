//! Sent prompts' flights, measured one by one: each leaves where its words were typed, travels
//! one monotonic path at its own size, never from above, the left edge or off the right, and
//! comes to rest where the settled layout puts it, with no snap at the end.
//!
//! The matrix crosses how a prompt goes (send, steer, two steers, ⌘⏎ then delivery), what it
//! follows (a reply, a prompt, a tool row, a compaction), its words (one word to very long,
//! images, a quote, mentions), and the window (wide, narrow, zoomed 1.6). The suite flies a
//! slice of it. `FLIGHTS=all` flies all of it, `FLIGHTS=<filter>,…` the cases whose names hold a
//! filter; `FLIGHTS_OUT` names a file for the table and `FLIGHTS_TABLE` prints it.

use rune::TextEdit;
use serde_json::json;

use super::{Circ, Length, Run, SID};
use opencode_kit::model::Message;

#[derive(Clone, Copy, Debug, PartialEq)]
enum How {
    Send,
    Steer,
    /// A second steer, under the first.
    SteerTwice,
    /// ⌘⏎ into the queue, then the server delivers it: the flight out of the queue.
    Delivered,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum After {
    Reply,
    Prompt,
    Tool,
    Compaction,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Words {
    Word,
    Line,
    Lines,
    Long,
    /// A crash report pasted whole, longer than the composer shows: it arrives in place.
    Paste,
    Image,
    Quote,
    QuoteOnly,
    Mention,
}

#[derive(Clone, Copy, Debug)]
struct Case {
    how: How,
    after: After,
    words: Words,
    narrow: bool,
    zoomed: bool,
}

impl Case {
    fn name(&self) -> String {
        let mut s = format!("{:?}/after-{:?}/{:?}", self.how, self.after, self.words).to_lowercase();
        s += if self.narrow { "/narrow" } else { "/wide" };
        if self.zoomed { s += "/zoomed"; }
        s
    }

    fn body(&self) -> String {
        let words = match self.words {
            Words::Word => "ok",
            Words::Line | Words::Image | Words::Quote => "Can you also check the reconnect jitter?",
            Words::Lines => "Three things before you merge:\n- keep the backoff capped at 30 s\n- log every retry with its attempt number\n- and add a test that drops the socket twice in a row",
            Words::Long => "The reconnect still feels wrong after sleep. When the lid opens the stream sits silent for the full 25 seconds before the watchdog notices, and in that window the sidebar shows stale sessions and the composer happily accepts prompts that go nowhere. Could we probe the socket the moment the system wakes instead, and only fall back to the watchdog when the wake notification never comes? Also please make sure the backoff resets only after a message actually arrives, not on open, because a server that accepts and immediately drops us would otherwise loop at the floor delay forever. Add tests for both, with a fake clock, and keep one slow end-to-end test on real timers so a regression shows up somewhere.",
            Words::QuoteOnly => "",
            Words::Mention => "",
            Words::Paste => return crate::stories::long_paste(),
        };
        words.into()
    }
}

/// Every combination; `everyday` is the slice the suite runs by default.
fn matrix() -> Vec<Case> {
    let mut v = Vec::new();
    let all = [Words::Word, Words::Line, Words::Lines, Words::Long, Words::Paste, Words::Image, Words::Quote, Words::QuoteOnly, Words::Mention];
    for zoomed in [false, true] {
        for narrow in [false, true] {
            for how in [How::Send, How::Steer, How::SteerTwice, How::Delivered] {
                for words in all {
                    v.push(Case { how, after: After::Reply, words, narrow, zoomed });
                }
                if how == How::Send {
                    for after in [After::Prompt, After::Tool, After::Compaction] {
                        for words in [Words::Line, Words::Lines] {
                            v.push(Case { how, after, words, narrow, zoomed });
                        }
                    }
                }
            }
        }
    }
    v
}

/// One of each way to send in each window: what the suite runs unless `FLIGHTS` asks for more.
fn everyday(c: &Case) -> bool {
    matches!((c.how, c.after, c.words),
        (How::Send, After::Reply, Words::Line | Words::Mention | Words::QuoteOnly)
        | (How::Send, After::Compaction, Words::Line)
        | (How::Steer, After::Reply, Words::Lines | Words::Paste)
        | (How::SteerTwice, After::Reply, Words::Image)
        | (How::Delivered, After::Reply, Words::Line | Words::Paste))
}

/// One frame of the flying prompt: its outer rect and its first words' rect.
#[derive(Clone, Copy, Debug)]
struct Frame {
    k: usize,
    rect: [f32; 4],
    target: [f32; 4],
    words: Option<[f32; 4]>,
}

impl Frame {
    /// Where it would sit this frame if it weren't flying, on screen.
    fn rest(&self) -> [f32; 4] {
        self.target
    }

    /// How far it is from that, on each axis.
    fn off(&self) -> [f32; 2] {
        let rest = self.rest();
        [self.rect[0] - rest[0], self.rect[1] - rest[1]]
    }
}

struct Measured {
    /// Where its words started: the composer's words (or the queue row's label).
    source: Option<[f32; 4]>,
    frames: Vec<Frame>,
    settled: Option<[f32; 4]>,
    window: (f32, f32),
    /// The frame Enter (or the delivery) landed on.
    sent_at: usize,
    /// Its source showed all its words, so it flies; otherwise it arrives in place.
    flies: bool,
}

fn setup(case: &Case) -> Run {
    let busy = case.how != How::Send;
    let length = if case.after == After::Tool { Length::Short } else { Length::Long };
    let circ = Circ { length, narrow: case.narrow, zoomed: case.zoomed, unplayed: busy, ..Circ::default() };
    let mut r = Run::new(circ);
    match case.after {
        After::Reply | After::Tool => {}
        After::Prompt => r.act("a prompt above", |rt| rt.update(|s, _| {
            if let Some(t) = s.transcripts.get_mut(SID) {
                t.messages.push(Message::User { id: "msg_zzzzabove".into(), text: "And while you're there, look at the retry cap.".into() });
            }
        })),
        After::Compaction => {
            r.event_now("session.compaction.started", json!({ "sessionID": SID, "eventID": "evt_flightcompaction", "reason": "auto" }));
            r.play(0.3);
            let tokens = json!({ "input": 10_412, "output": 1_034, "reasoning": 96, "cache": { "read": 1_880, "write": 0 } });
            r.event_now("session.compaction.ended", json!({ "sessionID": SID, "reason": "auto", "text": "## Goal\nKeep reconnects alive.", "tokens": tokens }));
        }
    }
    if busy {
        r.start_stream();
        r.play(1.2);
    }
    r.play(1.0);
    r
}

fn compose(r: &mut Run, case: &Case, body: &str) {
    let words = case.words;
    let body = body.to_owned();
    r.act("compose", |rt| rt.update(move |s, _| {
        s.composer = if words == Words::Mention { crate::mention_stories::draft() } else { TextEdit::new(body) };
        s.composer.focused = true;
        if matches!(words, Words::Quote | Words::QuoteOnly) {
            s.quote.push(opencode_kit::model::Quote { text: "The server sends a heartbeat every 10 seconds".into(), comment: None });
        }
    }));
    if words == Words::Image {
        r.act("attach", |rt| rt.update(|s, _| {
            let a = opencode_kit::model::Attachment { mime: "image/png".into(), name: "shot.png".into(), bytes: super::png().into(), media: None };
            s.attach.add(a.clone());
            s.attach.image(&a).unwrap().wait().unwrap();
        }));
    }
    r.play(0.8);
}

fn find(els: &[rune::ElementInfo], key: &str) -> Option<[f32; 4]> {
    els.iter().find(|e| !e.exiting && e.key.as_deref() == Some(key)).map(|e| e.rect)
}

/// The rows `u-…` on screen.
fn rows(r: &Run) -> Vec<String> {
    r.rt.elements().into_iter().filter(|e| !e.exiting).filter_map(|e| e.key.filter(|k| k.starts_with("u-msg"))).collect()
}

/// The flying prompt `id`'s frame: its bubble (quote cards alone stand in for one).
fn frame(r: &Run, id: &str) -> Option<Frame> {
    let els = r.rt.elements();
    let row = format!("u-{id}");
    let inside = |e: &&rune::ElementInfo| !e.exiting && e.path.split('/').any(|p| p == row);
    let face = els.iter().filter(inside).find(|e| e.key.as_deref() == Some("bubble"))?;
    let words = els.iter().filter(inside).find(|e| e.key.as_deref() == Some(&format!("text-{id}")))
        .or_else(|| els.iter().filter(inside).find(|e| e.key.as_deref() == Some("title") && e.path.contains("/quote-0/")))
        .map(|e| e.rect);
    // Its row isn't flying: it maps the bubble's layout onto the screen, scroll and zoom included.
    let row = els.iter().find(|e| !e.exiting && e.key.as_deref() == Some(&row))?;
    let k = r.rt.state.zoom;
    let t = face.target;
    let at = |axis: usize| row.rect[axis] + (t[axis] - row.target[axis]) * k;
    Some(Frame { k: r.gate.rec.frames, rect: face.rect, target: [at(0), at(1), t[2] * k, t[3] * k], words })
}

fn measure(case: Case) -> Measured {
    let mut r = setup(&case);
    let window = r.circ.size();
    if case.how == How::SteerTwice {
        compose(&mut r, &case, "First, keep the cap at 30 s.");
        r.enter(false);
        r.play(1.2);
    }
    compose(&mut r, &case, &case.body());
    let before = rows(&r);
    // Words leave where they were typed; a prompt of only a quote leaves from its chip.
    let mut source = if case.words == Words::QuoteOnly {
        r.rt.elements().into_iter().find(|e| !e.exiting && e.key.as_deref() == Some("title") && e.path.contains("/quote-chip-")).map(|e| e.rect)
    } else {
        find(&r.rt.elements(), "prompt")
    };
    let mut id: Option<String> = None;
    let mut flies = crate::view::shows_whole_prompt();
    if case.how == How::Delivered {
        r.enter(true);
        r.play(1.2);
        let queued = r.rt.state.inboxes.get(SID).and_then(|i| i.queued().last()).map(|i| i.id.clone());
        let Some(q) = queued else { return Measured { source: None, frames: vec![], settled: None, window, sent_at: 0, flies } };
        let els = r.rt.elements();
        source = els.iter().find(|e| !e.exiting && e.key.as_deref() == Some("label") && e.path.contains(&format!("queued-{q}"))).map(|e| e.rect);
        flies = crate::arrivals::flies_from_queue(&case.body());
        r.event_now("session.inbox.delivered", json!({ "sessionID": SID, "inboxID": q }));
    } else {
        r.enter(false);
    }
    let sent_at = r.gate.rec.frames - 1;
    let mut frames = Vec::new();
    for _ in 0..(2.0 / super::DT) as usize {
        if id.is_none() {
            id = rows(&r).into_iter().find(|k| !before.contains(k)).map(|k| k.trim_start_matches("u-").to_owned());
        }
        if let Some(f) = id.as_deref().and_then(|id| frame(&r, id)) {
            frames.push(Frame { k: f.k - 1, ..f });
        }
        r.step();
    }
    r.idle(1.0);
    let settled = id.as_deref().and_then(|id| frame(&r, id)).map(|f| f.rect);
    Measured { source, frames, settled, window, sent_at, flies }
}

/// What's wrong with one flight; empty when it flew as it should. Each frame is judged by its
/// offset from where it would sit that frame, so scrolling under it doesn't count as flying.
fn judge(m: &Measured) -> Vec<String> {
    let mut out = Vec::new();
    let (Some(first), Some(last)) = (m.frames.first(), m.frames.last()) else {
        out.push("no bubble to fly".into());
        return out;
    };
    if first.k > m.sent_at + 1 {
        out.push(format!("shows {} frames after the send", first.k - m.sent_at));
    }
    let o0 = first.off();
    if !m.flies {
        let worst = m.frames.iter().map(|f| f.off()[0].abs().max(f.off()[1].abs())).fold(0.0, f32::max);
        if worst > 1.0 { out.push(format!("should arrive in place, but sits {worst:.0} pt off its rest")); }
        return out;
    }
    if o0[0].abs() < 2.0 && o0[1].abs() < 2.0 {
        out.push(format!("didn't fly: first frame {:?} is its rest", first.rect.map(|v| v.round())));
        return out;
    }
    // Leaves where its words were (a quote alone, where its chip was).
    if let (Some(src), Some(words)) = (m.source, first.words) {
        let (dx, dy) = (words[0] - src[0], words[1] - src[1]);
        if dx.abs() > 6.0 || dy.abs() > 6.0 {
            out.push(format!("leaves {dx:+.0},{dy:+.0} pt off where its words were typed"));
        }
    }
    // From the right side: below its rest, left of it, inside the window.
    let r0 = first.rect;
    if o0[1] < -1.0 { out.push(format!("comes from above ({:.0} pt over its rest)", -o0[1])); }
    if o0[0] > 1.0 { out.push(format!("comes in from the right of its rest ({:+.0} pt)", o0[0])); }
    if r0[0] < 1.0 { out.push(format!("enters from the left edge (x {:.0})", r0[0])); }
    if r0[0] + r0[2] > m.window.0 + 1.0 { out.push(format!("enters from off the right (right edge {:.0} > {:.0})", r0[0] + r0[2], m.window.0)); }
    // At its own size the whole way.
    let worst = m.frames.iter().map(|f| (f.rect[2] - last.rect[2]).abs().max((f.rect[3] - last.rect[3]).abs())).fold(0.0, f32::max);
    if worst > 1.0 { out.push(format!("changes size in flight by {worst:.1} pt")); }
    if (last.rect[2] - last.target[2]).abs() > 0.5 || (last.rect[3] - last.target[3]).abs() > 0.5 {
        out.push(format!("flies at {:.0}×{:.0}, its layout says {:.0}×{:.0}", last.rect[2], last.rect[3], last.target[2], last.target[3]));
    }
    // One monotonic path on screen, whatever scrolls under it, that never passes its rest.
    for axis in 0..2 {
        let name = ["x", "y"][axis];
        let total = last.rect[axis] - first.rect[axis];
        let back: f32 = m.frames.windows(2).map(|w| w[1].rect[axis] - w[0].rect[axis]).filter(|d| d * total.signum() < 0.0).map(f32::abs).sum();
        if back > 1.0 { out.push(format!("turns back {back:.1} pt on {name}")); }
        let over = m.frames.iter().map(|f| f.off()[axis]).filter(|o| o * o0[axis] < 0.0).map(f32::abs).fold(0.0, f32::max);
        if o0[axis].abs() > 1.0 && over > 1.0 { out.push(format!("overshoots {over:.1} pt on {name}")); }
    }
    // No snap at the end: the last step is no bigger than the steps before it.
    let steps: Vec<f32> = m.frames.windows(2).map(|w| (w[1].rect[0] - w[0].rect[0]).abs().max((w[1].rect[1] - w[0].rect[1]).abs())).collect();
    if let Some(i) = steps.iter().rposition(|&s| s > 0.05) {
        let tail = steps[i];
        let before = if i > 0 { steps[i - 1] } else { 0.0 };
        if tail > 2.0 && tail > 3.0 * before.max(0.3) { out.push(format!("snaps {tail:.1} pt onto its rest")); }
    }
    let ol = last.off();
    if ol[0].abs() > 0.5 || ol[1].abs() > 0.5 { out.push(format!("still {:+.0},{:+.0} pt from its rest after 2 s", ol[0], ol[1])); }
    if let Some(rest) = m.settled
        && ((rest[2] - last.rect[2]).abs() > 0.5 || (rest[3] - last.rect[3]).abs() > 0.5)
    {
        out.push(format!("settles at {:.0}×{:.0} after flying at {:.0}×{:.0}", rest[2], rest[3], last.rect[2], last.rect[3]));
    }
    out
}

fn row_of(m: &Measured, case: &Case, problems: &[String]) -> String {
    let f = m.frames.first();
    let fmt = |r: Option<[f32; 4]>| r.map_or("-".into(), |r| format!("{:.0},{:.0} {:.0}×{:.0}", r[0], r[1], r[2], r[3]));
    format!("{:<52} src {:<22} first {:<24} words {:<22} rest {:<24} {}\n", case.name(), fmt(m.source), fmt(f.map(|f| f.rect)), fmt(f.and_then(|f| f.words)), fmt(m.settled),
        if problems.is_empty() { "ok".into() } else { problems.join("; ") })
}

#[test]
fn sent_prompts_fly_in_from_the_composer() {
    let filter = std::env::var("FLIGHTS").ok();
    let cases: Vec<Case> = matrix().into_iter().filter(|c| match &filter {
        Some(f) => f == "all" || f.split(',').any(|f| c.name().contains(f)),
        None => everyday(c),
    }).collect();
    let jobs = std::sync::Mutex::new(cases.into_iter().enumerate().collect::<Vec<_>>());
    let out = std::sync::Mutex::new(Vec::new());
    let threads = std::thread::available_parallelism().map_or(4, |n| n.get()).min(12);
    std::thread::scope(|scope| {
        for _ in 0..threads {
            std::thread::Builder::new().stack_size(64 << 20).spawn_scoped(scope, || loop {
                let Some((i, case)) = jobs.lock().unwrap().pop() else { break };
                let m = measure(case);
                let problems = judge(&m);
                if std::env::var_os("FLIGHTS_FRAMES").is_some() {
                    let lines: String = m.frames.iter().map(|f| format!("  {} rect {:?} off {:?} words {:?}\n", f.k, f.rect.map(|v| v.round()), f.off().map(|v| v.round()), f.words.map(|w| w.map(|v| v.round())))).collect();
                    eprintln!("{} src {:?}\n{lines}", case.name(), m.source);
                }
                out.lock().unwrap().push((i, row_of(&m, &case, &problems), problems.len()));
            }).unwrap();
        }
    });
    let mut rows = out.into_inner().unwrap();
    rows.sort_by_key(|r| r.0);
    let failing = rows.iter().filter(|r| r.2 > 0).count();
    let table: String = rows.iter().map(|r| r.1.as_str()).collect();
    if let Ok(path) = std::env::var("FLIGHTS_OUT") {
        std::fs::write(path, &table).unwrap();
    }
    if std::env::var_os("FLIGHTS_TABLE").is_some() {
        eprint!("{table}");
    }
    assert!(failing == 0, "{failing} of {} flights wrong:\n{}", rows.len(), rows.iter().filter(|r| r.2 > 0).map(|r| r.1.as_str()).collect::<String>());
}
