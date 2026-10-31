//! Compactions in the timeline, as Desktop draws them (`message-content.tsx`,
//! `session-compaction.css`, `timeline-separator.tsx`).
//!
//! One a person asked for, one that failed, or any while `experimental.background_compaction`
//! is off is loud: a divider opens it, the summary follows, and a divider with the request's
//! usage ends it. With background compaction on, an automatic one is a quiet divider whose label
//! reads Compacting, then Compacted, and opens the summary. One the server admitted but hasn't
//! started is a divider of its own.

use std::collections::HashMap;
use std::f32::consts::FRAC_PI_2;
use std::sync::OnceLock;

use opencode_kit::client::Request;
use opencode_kit::model::{Compaction, CompactionStatus, Message, Tokens};
use serde_json::Value;
use rune::{
    AnimExt, Beat, Color, Curve, Cursor, Cycle, Easing, Effect, El, Icon, LoopSpec, Sequence, Spring, SpringPair, Transition, Tx, col, el, icon, row,
    text,
};

use crate::palette::Pal;
use crate::view::{BUBBLE, UI, UI_MED, markdown_tinted, readable};
use crate::{App, Route, Source};

/// A new automatic compaction waits this long before it shows (Desktop's grace).
const GRACE: f64 = 0.4;
/// One pass of the light along the lines and of the glint on the label, in seconds.
const CYCLE: f32 = 3.2;
/// The quiet divider's fade in.
static ENTER: [Beat; 2] = [Beat::new(0.0).opacity(0.0), Beat::new(0.32).easing(Easing::EASE_OUT)];
/// The quiet summary's fade in.
static OPEN: [Beat; 2] = [Beat::new(0.0).opacity(0.0), Beat::new(0.2).easing(Easing::EASE_OUT)];

static ARCHIVE: Icon = Icon {
    name: "archive",
    view: 16.0,
    body: r#"<path d="M2.5 5.5V13.5H13.5V5.5M2.5 5.5H1.5V2.5H14.5V5.5H13.5M2.5 5.5H13.5M6.5 7.5H9.5" stroke-linecap="round" stroke-linejoin="round" stroke="currentColor"/>"#,
};

#[derive(Default)]
pub struct Compactions {
    /// `experimental.background_compaction` by directory, from `/api/config`.
    pub background: HashMap<String, bool>,
    /// The setting where the server hasn't said (stories and tests set it).
    pub fallback: bool,
    /// When each compaction in the open session first showed, and how it enters.
    seen: HashMap<String, Seen>,
    /// When each quiet summary opened, on the app clock.
    opened: HashMap<String, f64>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct Seen {
    at: f64,
    enter: Option<Enter>,
    running: bool,
}

/// A quiet divider's entrance. `Grace`: a new one fades in once the grace is out, showing
/// Compacting. `Now`: one that ended inside the grace, or just before it showed, fades in at
/// once showing only Compacted. Without either, it was there already and shows as it is.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Enter {
    Grace,
    Now,
}

/// One row of a compaction in the timeline.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Piece {
    Quiet,
    Start,
    Summary,
    /// Compacting while it runs, then the divider that ends it.
    End,
    Error,
}

impl Piece {
    pub(crate) fn key(self, id: &str) -> String {
        let name = match self {
            Piece::Quiet => "quiet",
            Piece::Start => "start",
            Piece::Summary => "summary",
            Piece::End => "end",
            Piece::Error => "error",
        };
        format!("cmp-{id}-{name}")
    }
}

/// Whether `session`'s directory has background compaction on.
pub(crate) fn background(s: &App, session: &str) -> bool {
    let dir = s.session(session).map(|i| i.directory.as_str());
    dir.and_then(|d| s.compactions.background.get(d)).copied().unwrap_or(s.compactions.fallback)
}

/// Asks the server whether `session`'s directory has background compaction on, once.
pub(crate) fn ask(s: &App, session: &str) {
    let Some(dir) = s.session(session).map(|i| i.directory.clone()).filter(|d| !d.is_empty()) else { return };
    if let Source::Live { requests: Some(r), .. } = &s.source
        && !s.compactions.background.contains_key(&dir)
    {
        let _ = r.send(Request::Config(dir));
    }
}

/// `config.listed` (the reply to `Request::Config`) notes a directory's setting, and
/// `config.updated` asks again for every directory asked about. True when the setting moved.
pub(crate) fn apply_config(s: &mut App, ty: &str, data: &Value) -> bool {
    match ty {
        "config.listed" => {
            let Some(dir) = data["directory"].as_str() else { return false };
            let on = data["backgroundCompaction"] == true;
            s.compactions.background.insert(dir.to_owned(), on) != Some(on)
        }
        "config.updated" => {
            if let Source::Live { requests: Some(r), .. } = &s.source {
                for dir in s.compactions.background.keys() {
                    let _ = r.send(Request::Config(dir.clone()));
                }
            }
            false
        }
        _ => false,
    }
}

/// Desktop's choice: quiet only for an automatic compaction that hasn't failed, with
/// background compaction on.
pub(crate) fn quiet(s: &App, session: &str, c: &Compaction) -> bool {
    background(s, session) && !c.manual && c.status != CompactionStatus::Failed
}

/// The rows `c` takes, top to bottom.
pub(crate) fn pieces(s: &App, session: &str, c: &Compaction) -> Vec<Piece> {
    if quiet(s, session, c) {
        return vec![Piece::Quiet];
    }
    let mut v = vec![Piece::Start];
    if !summary(c).is_empty() {
        v.push(Piece::Summary);
    }
    v.push(Piece::End);
    if error(c).is_some() {
        v.push(Piece::Error);
    }
    v
}

/// After the frame's events: notes when each compaction in the open session first showed and
/// how it enters, and moves one that ended inside its grace to fading in as Compacted. Only the
/// open session's are kept, so one seen again later shows as it is, as Desktop's remount does.
/// True when what's drawn changed.
pub fn sync(s: &mut App) -> bool {
    let App { route, transcripts, compactions, now_ms, clock, .. } = s;
    let t = match route {
        Route::Session(id) => transcripts.get(id.as_str()),
        _ => None,
    };
    let list: Vec<&Compaction> = t.into_iter().flat_map(|t| &t.messages).filter_map(|m| if let Message::Compaction(c) = m { Some(c) } else { None }).collect();
    let before = compactions.seen.len();
    compactions.seen.retain(|id, _| list.iter().any(|c| &c.id == id));
    compactions.opened.retain(|id, _| list.iter().any(|c| &c.id == id));
    let mut changed = compactions.seen.len() != before;
    for c in list {
        let running = c.status == CompactionStatus::Running;
        match compactions.seen.get_mut(&c.id) {
            None => {
                let age = *now_ms - c.created;
                let enter = if running { (age < 400).then_some(Enter::Grace) } else { (age < 1_000).then_some(Enter::Now) };
                compactions.seen.insert(c.id.clone(), Seen { at: *clock, enter, running });
                changed = true;
            }
            Some(seen) if seen.running && !running => {
                seen.running = false;
                if seen.enter == Some(Enter::Grace) && *clock - seen.at < GRACE {
                    *seen = Seen { at: *clock, enter: Some(Enter::Now), running };
                }
                changed = true;
            }
            _ => {}
        }
    }
    changed
}

/// Piece `piece` of compaction `c`. `fade`: the timeline's word fade for a streaming summary.
pub(crate) fn view(s: &App, p: &Pal, c: &Compaction, piece: Piece, fade: f32) -> El {
    let running = c.status == CompactionStatus::Running;
    match piece {
        Piece::Quiet => quiet_view(s, p, c, fade),
        Piece::Start => divider(p, "Session compaction started"),
        Piece::Summary => col().w_full().child(markdown_tinted(p, summary(c), fade, running, p.text)),
        Piece::End if running => {
            let since = s.compactions.seen.get(&c.id).map_or(s.clock, |x| x.at);
            let title = crate::working::shimmer(s, p, "Compacting", UI_MED, since);
            let tool = row().key("tool").h(28.0).gap(8.0).center_y().child(icon(&ARCHIVE, 14.0).color(p.icon_muted)).child(title);
            col().key("running").w_full().pt(8.0).pb(8.0).role("status").label("Compacting").transition(Transition::none()).child(tool)
        }
        Piece::End => {
            let label = [outcome(c).to_owned(), usage(c).unwrap_or_default()].into_iter().filter(|x| !x.is_empty()).collect::<Vec<_>>().join(" · ");
            divider(p, &label).key("ended").transition(Transition::none())
        }
        Piece::Error => error_card(p, error(c).unwrap_or_default()),
    }
}

/// A compaction the server admitted but hasn't started.
pub(crate) fn queued(p: &Pal) -> El {
    divider(p, "Session compaction queued")
}

/// Desktop's timeline separator in its `py-2`: a hairline from nothing to the strong border on
/// each side of a faint label, which fades out where it's cut off.
pub(crate) fn divider(p: &Pal, label: &str) -> El {
    divider_of(p, text(label.to_owned()).key("label").font(UI).color(p.faint).truncate_fade())
}

/// The divider around any `words`, such as a label with its value (a moved session's folder).
pub(crate) fn divider_of(p: &Pal, words: El) -> El {
    let strong = p.q.border_strong;
    let clear = Color { a: 0.0, ..strong };
    // The lines keep some length when long words (a moved session's folder) fill the row.
    let line = |key: &'static str, angle: f32| el().key(key).h(1.0).grow(1.0).min_w(24.0).linear_gradient(angle, [(0.0, clear), (0.7, strong)]);
    let bar = row().key("separator").h(32.0).w_full().gap(12.0).center_y().child(line("start", FRAC_PI_2)).child(words).child(line("end", -FRAC_PI_2));
    col().w_full().pt(8.0).pb(8.0).child(bar)
}

fn summary(c: &Compaction) -> &str {
    if c.status == CompactionStatus::Failed { "" } else { c.summary.trim() }
}

/// What a failure says in its card: none for a cancel or an interruption, which the divider says.
fn error(c: &Compaction) -> Option<&str> {
    let (ty, message) = c.error.as_ref().filter(|_| c.status == CompactionStatus::Failed)?;
    (!matches!(ty.as_str(), "aborted" | "compaction.interrupted") && !message.is_empty()).then_some(message.as_str())
}

fn outcome(c: &Compaction) -> &'static str {
    match (c.status, c.error.as_ref().map(|e| e.0.as_str())) {
        (CompactionStatus::Failed, Some("aborted")) => "Session compaction cancelled",
        (CompactionStatus::Failed, Some("compaction.interrupted")) => "Session compaction interrupted",
        (CompactionStatus::Failed, _) => "Session compaction failed",
        (CompactionStatus::Completed, _) if c.native => "Session compacted by provider",
        _ => "Session compacted",
    }
}

/// The compaction request's own usage, "12.3K in · 1.1K out": none while it runs or when it
/// used nothing.
pub(crate) fn usage(c: &Compaction) -> Option<String> {
    let t: &Tokens = c.tokens.as_ref().filter(|_| c.status != CompactionStatus::Running)?;
    let input = t.input + t.cache_read + t.cache_write;
    let output = t.output + t.reasoning;
    (input + output > 0).then(|| format!("{} in · {} out", compact(input), compact(output)))
}

/// `Intl.NumberFormat` with compact notation and at most one fraction digit: 999, 1K, 12.3K, 1.5M.
pub(crate) fn compact(n: u64) -> String {
    let mut v = n as f64;
    let mut unit = 0;
    const UNITS: [&str; 5] = ["", "K", "M", "B", "T"];
    while unit + 1 < UNITS.len() && (v * 10.0).round() / 10.0 >= 1000.0 {
        v /= 1000.0;
        unit += 1;
    }
    if unit == 0 {
        return n.to_string();
    }
    let r = (v * 10.0).round() / 10.0;
    let s = if r.fract() == 0.0 { format!("{r:.0}") } else { format!("{r:.1}") };
    format!("{s}{}", UNITS[unit])
}

/// Desktop's error card: the danger accent down its left edge and the message in body text.
fn error_card(p: &Pal, message: &str) -> El {
    let accent = el().key("accent").pin(Some(0.0), Some(8.0), None, Some(8.0)).w(2.0).radius(2.0).fill(p.danger);
    let body = readable(p, text(message.to_owned()).font(BUBBLE).color(p.text).wrap_anywhere().w_full());
    row().key("error-card").w_full().pt(10.0).pb(10.0).pl(10.0).pr(12.0).child(accent).child(body)
}

// ---------------------------------------------------------------------------------------------
// The quiet divider

fn quiet_view(s: &App, p: &Pal, c: &Compaction, fade: f32) -> El {
    let running = c.status == CompactionStatus::Running;
    let seen = s.compactions.seen.get(&c.id).copied();
    let enter = seen.and_then(|x| x.enter);
    let at = seen.map_or(s.clock, |x| x.at);
    let delay = if enter == Some(Enter::Grace) { GRACE } else { 0.0 };
    // The light and the glint start with the fade in, after the grace.
    let start = at + delay;
    let body = summary(c);
    let expandable = !running && !body.is_empty();
    let key = format!("compaction-{}", c.id);
    let open = expandable && s.expanded.contains(&key);
    let lit = running && !s.reduced_motion;
    let mut divider = row()
        // A new key restarts the fade in, with everything inside at rest as Compacted.
        .key(if enter == Some(Enter::Now) { "divider-now" } else { "divider" })
        .h(32.0)
        .w_full()
        .gap(12.0)
        .center_y()
        .label(if running { "Compacting" } else { "Compacted" })
        .transition(Transition::none())
        .child(quiet_line(s, p, lit, start, false))
        .child(quiet_label(s, p, running, open, start))
        .child(quiet_line(s, p, lit, start, true));
    if enter.is_some() && s.clock - at < delay + 0.5 {
        divider = divider.looping(LoopSpec::sequence(Sequence::new(&ENTER).delay(delay)));
    }
    if expandable {
        let id = c.id.clone();
        divider = divider.role("button").hover_group().cursor(Cursor::Pointer).on_press_tx(move |s: &mut App, tx: &mut Tx| {
            tx.instant();
            if s.toggle_expanded(&key) {
                s.compactions.opened.insert(id.clone(), s.clock);
            }
        });
    }
    let mut out = col().w_full().child(col().w_full().pt(8.0).pb(8.0).child(divider));
    if open {
        let mut summary = col()
            .key("summary")
            .w_full()
            .pb(8.0)
            .transition(Transition::none())
            .child(markdown_tinted(p, body, fade, false, p.muted))
            .children(usage(c).map(|u| text(u).key("usage").font(UI).color(p.faint).mt(8.0)));
        if s.compactions.opened.get(&c.id).is_some_and(|t| s.clock - t < 0.3) {
            summary = summary.looping(LoopSpec::sequence(Sequence::new(&OPEN)));
        }
        out = out.child(summary);
    }
    out
}

/// A hairline that brightens when the compaction ends, with a light that gathers toward the label
/// while it runs. `end`: the line after the label, which mirrors the first.
fn quiet_line(s: &App, p: &Pal, lit: bool, start: f64, end: bool) -> El {
    let base = el()
        .key("base")
        .absolute(0.0, 0.0)
        .w_full()
        .h_full()
        .fill(p.q.border_strong)
        .opacity(if lit { 0.6 } else { 1.0 })
        .opacity_motion(Curve::tween(0.36, Easing::EASE_OUT));
    let mut line = el().key(if end { "end" } else { "start" }).h(1.0).grow(1.0).min_w(0.0).clip_soft(0.0).child(base);
    if lit {
        line = line.child(light(s, p, start, end));
    }
    line
}

/// The light: 40% of the line wide, it slides in from the outer end and off past the label in
/// the first 70% of each cycle, fading up as it comes and out as it reaches the label. It fades
/// out over 360 ms when the compaction ends.
fn light(s: &App, p: &Pal, start: f64, end: bool) -> El {
    let peak = Color { a: p.muted.a * 0.55, ..p.muted };
    let clear = Color { a: 0.0, ..p.muted };
    let (from, to) = if end { (1.0, -2.5) } else { (-1.0, 2.5) };
    let slide = LoopSpec { delay: loop_delay(s.clock, start), ..LoopSpec::slide(f64::from(CYCLE), [from, 0.0], [to, 0.0]).easing(travel()) };
    let beam = el()
        .key("beam")
        .pin((!end).then_some(0.0), Some(0.0), end.then_some(0.0), None)
        .w_frac(0.4)
        .h_full()
        .linear_gradient(if end { -FRAC_PI_2 } else { FRAC_PI_2 }, [(0.0, clear), (0.65, peak), (1.0, clear)])
        .looping(slide);
    el().key("light").absolute(0.0, 0.0).w_full().h_full().cycle(cycle(start, light_stops())).transition(fade_out(0.36)).child(beam)
}

/// "Compact" with its ending: "ing" rolls up and out, then "ed" rolls up into its place, in a box
/// as wide as the wider so nothing beside it moves. While it runs, a brighter copy warms the
/// label each time the lights land on it.
fn quiet_label(s: &App, p: &Pal, running: bool, open: bool, start: f64) -> El {
    let (stem, ing, ed) = ("Compact", "ing", "ed");
    let width = |w: &str| rune::text::measure(w, &[], UI, None).0;
    let ending_w = width(ing).max(width(ed)).ceil();
    let calm = s.reduced_motion;
    let (shift, blur) = if calm { (0.0, 0.0) } else { (3.0, 2.0) };
    let out = Curve::tween(0.2, Easing::cubic(0.4, 0.0, 1.0, 1.0));
    let into = Curve::tween(0.28, Easing::cubic(0.2, 0.8, 0.2, 1.0)).delay(0.08);
    let morph = |t: El, shown: bool, dy: f32, anim: rune::Anim| {
        t.absolute(0.0, 0.0)
            .opacity(if shown { 1.0 } else { 0.0 })
            .offset(0.0, if shown { 0.0 } else { dy })
            .blur(if shown { 0.0 } else { blur })
            .opacity_motion(anim)
            .offset_motion(anim)
            .blur_motion(anim)
    };
    let done = text(ed).key("done").font(UI).color(if open { p.muted } else { p.faint }).group_color(p.muted);
    let ending = el()
        .key("ending")
        .w(ending_w)
        .h(16.0)
        .child(morph(text(ing).key("running").font(UI).color(p.faint), running, -shift, out.into()))
        .child(morph(done, !running, shift, into.into()));
    let mut label = row().key("label").child(text(stem).key("stem").font(UI).color(p.faint)).child(ending);
    if running && !calm {
        let glint = row()
            .key("glint")
            .absolute(0.0, 0.0)
            .cycle(cycle(start, glint_stops()))
            .transition(fade_out(0.2))
            .child(text(stem).font(UI).color(p.muted))
            .child(el().w(ending_w).h(16.0).child(text(ing).font(UI).color(p.muted).absolute(0.0, 0.0)));
        label = label.child(glint);
    }
    label
}

/// Shows at once and fades out over about `secs` when removed.
fn fade_out(secs: f32) -> Transition {
    Transition { insertion: Effect::IDENTITY, removal: Effect::opacity(0.0), pop_layout: true, springs: SpringPair::new(Spring::NONE, Spring::new(secs, 0.0)) }
}

/// The wait that puts a loop mounting `now` in step with cycles counted from `start`.
fn loop_delay(now: f64, start: f64) -> f64 {
    let period = f64::from(CYCLE);
    if now < start {
        return start - now;
    }
    let d = (start - now).rem_euclid(period);
    if d > period - 1e-6 { 0.0 } else { d }
}

/// An opacity cycle with `stops` that starts at `start` on the app clock.
fn cycle(start: f64, stops: &'static [(f32, f32)]) -> Cycle {
    Cycle { period: CYCLE, phase: (start / f64::from(CYCLE)).rem_euclid(1.0) as f32, stops, still: 0.0, fps: 60.0 }
}

/// Points along `curve` from (`x0`, `y0`) to (`x1`, `y1`), the first left out.
fn eased(curve: Easing, (x0, y0): (f32, f32), (x1, y1): (f32, f32), n: usize) -> impl Iterator<Item = (f32, f32)> {
    (1..=n).map(move |i| {
        let u = i as f32 / n as f32;
        (x0 + (x1 - x0) * u, y0 + (y1 - y0) * curve.eval(u))
    })
}

/// The light's travel: one ease-in-out over the first 70% of the cycle, then out of sight.
fn travel() -> Easing {
    static STOPS: OnceLock<&'static [[f32; 2]]> = OnceLock::new();
    let stops = STOPS.get_or_init(|| {
        let mut v = vec![[0.0, 0.0]];
        v.extend(eased(Easing::cubic(0.4, 0.0, 0.6, 1.0), (0.0, 0.0), (0.7, 1.0), 28).map(|(x, y)| [x, y]));
        v.push([1.0, 1.0]);
        Box::leak(v.into_boxed_slice())
    });
    Easing::Piecewise(stops)
}

/// The light's opacity: up by 14% of the cycle on the travel's curve, held to 49.5%, out by 70%.
fn light_stops() -> &'static [(f32, f32)] {
    static STOPS: OnceLock<&'static [(f32, f32)]> = OnceLock::new();
    STOPS.get_or_init(|| {
        let mut v = vec![(0.0, 0.0)];
        v.extend(eased(Easing::cubic(0.4, 0.0, 0.6, 1.0), (0.0, 0.0), (0.14, 1.0), 8));
        v.extend([(0.495, 1.0), (0.7, 0.0), (1.0, 0.0)]);
        Box::leak(v.into_boxed_slice())
    })
}

/// The glint: dark to 49%, up to 0.75 by 57%, out by 82%.
fn glint_stops() -> &'static [(f32, f32)] {
    static STOPS: OnceLock<&'static [(f32, f32)]> = OnceLock::new();
    STOPS.get_or_init(|| {
        let mut v = vec![(0.0, 0.0), (0.49, 0.0)];
        v.extend(eased(Easing::cubic(0.2, 0.8, 0.2, 1.0), (0.49, 0.0), (0.57, 0.75), 8));
        v.extend(eased(Easing::cubic(0.4, 0.0, 0.6, 1.0), (0.57, 0.75), (0.82, 0.0), 10));
        v.push((1.0, 0.0));
        Box::leak(v.into_boxed_slice())
    })
}

#[cfg(test)]
#[path = "compaction_tests.rs"]
mod tests;
