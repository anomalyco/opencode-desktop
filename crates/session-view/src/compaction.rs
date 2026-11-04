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

use rune_knobs::{Group, Knob};

use crate::TimelineHost;
use crate::palette::Pal;
use crate::style::{BUBBLE, UI, UI_MED, markdown_tinted, readable};

/// A new automatic compaction waits this long before it shows (Desktop's grace).
const GRACE: f64 = 0.4;
/// One pass of the light along the lines and of the glint on the label, in seconds.
const CYCLE: f32 = 3.2;
/// The quiet divider's fade in.
static ENTER: [Beat; 2] = [Beat::new(0.0).opacity(0.0), Beat::new(0.32).easing(Easing::EASE_OUT)];

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
    pub seen: HashMap<String, Seen>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Seen {
    pub at: f64,
    pub enter: Option<Enter>,
    pub running: bool,
}

/// A quiet divider's entrance. `Grace`: a new one fades in once the grace is out, showing
/// Compacting. `Now`: one that ended inside the grace, or just before it showed, fades in at
/// once showing only Compacted. Without either, it was there already and shows as it is.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Enter {
    Grace,
    Now,
}

/// One row of a compaction in the timeline.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Piece {
    Quiet,
    Start,
    Summary,
    /// Compacting while it runs, then the divider that ends it.
    End,
    Error,
}

impl Piece {
    pub fn key(self, id: &str) -> String {
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
pub fn background<S: TimelineHost>(s: &S, session: &str) -> bool {
    let dir = s.session_directory(session);
    dir.and_then(|d| s.compactions_state().background.get(d)).copied().unwrap_or(s.compactions_state().fallback)
}

/// Asks the server whether `session`'s directory has background compaction on, once.
pub fn ask<S: TimelineHost>(s: &S, session: &str) {
    let Some(dir) = s.session_directory(session).map(str::to_owned).filter(|d| !d.is_empty()) else { return };
    if !s.compactions_state().background.contains_key(&dir) {
        s.send_request(Request::Config(dir));
    }
}

/// `config.listed` (the reply to `Request::Config`) notes a directory's setting, and
/// `config.updated` asks again for every directory asked about. True when the setting moved.
pub fn apply_config<S: TimelineHost>(s: &mut S, ty: &str, data: &Value) -> bool {
    match ty {
        "config.listed" => {
            let Some(dir) = data["directory"].as_str() else { return false };
            let on = data["backgroundCompaction"] == true;
            s.compactions_state_mut().background.insert(dir.to_owned(), on) != Some(on)
        }
        "config.updated" => {
            let dirs: Vec<String> = s.compactions_state().background.keys().cloned().collect();
            for dir in dirs {
                s.send_request(Request::Config(dir));
            }
            false
        }
        _ => false,
    }
}

/// Desktop's choice: quiet only for an automatic compaction that hasn't failed, with
/// background compaction on.
pub fn quiet<S: TimelineHost>(s: &S, session: &str, c: &Compaction) -> bool {
    background(s, session) && !c.manual && c.status != CompactionStatus::Failed
}

/// The rows `c` takes, top to bottom.
pub fn pieces<S: TimelineHost>(s: &S, session: &str, c: &Compaction) -> Vec<Piece> {
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
pub fn sync<S: TimelineHost>(s: &mut S) -> bool {
    let now_ms = s.now_ms();
    let clock = s.clock();
    let mut compactions = std::mem::take(s.compactions_state_mut());
    let t = s.active_session_id().and_then(|id| s.transcript(id));
    let before = compactions.seen.len();
    compactions.seen.retain(|id, _| {
        t.is_some_and(|t| t.messages.iter().any(|m| matches!(m, Message::Compaction(c) if &c.id == id)))
    });
    let mut changed = compactions.seen.len() != before;
    if let Some(t) = t {
        for m in &t.messages {
            let Message::Compaction(c) = m else { continue };
            let running = c.status == CompactionStatus::Running;
            match compactions.seen.get_mut(&c.id) {
                None => {
                    let age = now_ms - c.created;
                    let enter = if running { (age < 400).then_some(Enter::Grace) } else { (age < 1_000).then_some(Enter::Now) };
                    compactions.seen.insert(c.id.clone(), Seen { at: clock, enter, running });
                    changed = true;
                }
                Some(seen) if seen.running && !running => {
                    seen.running = false;
                    if seen.enter == Some(Enter::Grace) && clock - seen.at < GRACE {
                        *seen = Seen { at: clock, enter: Some(Enter::Now), running };
                    }
                    changed = true;
                }
                _ => {}
            }
        }
    }
    *s.compactions_state_mut() = compactions;
    changed
}

/// Whether `piece` of `c` draws as a divider.
pub fn is_divider(c: &Compaction, piece: Piece) -> bool {
    match piece {
        Piece::Quiet | Piece::Start => true,
        Piece::End => c.status != CompactionStatus::Running,
        Piece::Summary | Piece::Error => false,
    }
}

/// Piece `piece` of compaction `c`, a divider at `run`. `fade`: the timeline's word fade for a
/// streaming summary.
pub fn view<S: TimelineHost>(s: &S, p: &Pal, c: &Compaction, piece: Piece, fade: f32, run: Run) -> El {
    let running = c.status == CompactionStatus::Running;
    match piece {
        Piece::Quiet => quiet_view(s, p, c, run),
        Piece::Start => divider(s, p, "Session compaction started", run),
        Piece::Summary => col().w_full().child(markdown_tinted::<S>(p, summary(c), fade, running, p.text, crate::genui::width(s))),
        Piece::End if running => {
            let since = s.compactions_state().seen.get(&c.id).map_or(s.clock(), |x| x.at);
            let title = crate::working::shimmer(s, p, "Compacting", UI_MED, since);
            let tool = row().key("tool").h(28.0).gap(8.0).center_y().child(icon(&ARCHIVE, 14.0).color(p.icon_muted)).child(title);
            col().key("running").w_full().pt(8.0).pb(8.0).role("status").label("Compacting").transition(Transition::none()).child(tool)
        }
        Piece::End => {
            let label = [outcome(c).to_owned(), usage(c).unwrap_or_default()].into_iter().filter(|x| !x.is_empty()).collect::<Vec<_>>().join(" · ");
            divider(s, p, &label, run).key("ended").transition(Transition::none())
        }
        Piece::Error => error_card(p, error(c).unwrap_or_default()),
    }
}

/// A compaction the server admitted but hasn't started.
pub fn queued<S: TimelineHost>(s: &S, p: &Pal, run: Run) -> El {
    divider(s, p, "Session compaction queued", run)
}

/// Line to line, in points, for dividers stacked back to back.
pub const DIVIDER_GAP: Knob = Knob::new("Divider gap", 16.0, 48.0, 28.0).step(2.0).about("Line to line for stacked dividers");

pub const GROUP: Group = Group::new("Dividers", "dividers", &[DIVIDER_GAP]).under("Timeline");

/// The height of a divider's words, which its lines run through the middle of.
pub const WORDS: f32 = 16.0;

/// What sits beside a divider, on one side.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum Beside {
    /// Rows with no room of their own toward it, as a reply's text: the turn's usual 16.
    #[default]
    Other,
    /// A row that brings its own room: a prompt (its copy line under it), Working, the end.
    Spaced,
    /// Another divider: the two lines sit [`DIVIDER_GAP`] apart.
    Divider,
}

/// What sits above and below a divider.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Run {
    pub above: Beside,
    pub below: Beside,
}

/// The room over and under a divider's words at `run`: the turn's usual gap from other rows, and
/// [`DIVIDER_GAP`] line to line within a stack.
pub fn space<S: TimelineHost>(s: &S, run: Run) -> (f32, f32) {
    let half = (s.knobs().get(&DIVIDER_GAP).max(WORDS) - WORDS) / 2.0;
    let side = |b: Beside| match b {
        Beside::Other => 16.0,
        Beside::Spaced => 0.0,
        Beside::Divider => half,
    };
    (side(run.above), side(run.below))
}

/// Desktop's timeline separator: a hairline from nothing to the strong border on each side of a
/// faint label, which fades out where it's cut off.
pub fn divider<S: TimelineHost>(s: &S, p: &Pal, label: &str, run: Run) -> El {
    divider_of(s, p, text(label.to_owned()).key("label").font(UI).color(p.faint).truncate_fade(), run)
}

/// The divider around any `words`, such as a label with its value (a moved session's folder).
pub fn divider_of<S: TimelineHost>(s: &S, p: &Pal, words: El, run: Run) -> El {
    // The lines keep some length when long words (a moved session's folder) fill the row.
    let line = |end: bool| hairline(p, el().key(if end { "end" } else { "start" }).h(1.0).grow(1.0).min_w(24.0), end);
    col().w_full().child(divider_bar(line(false), words, line(true), space(s, run)).key("separator"))
}

/// A divider's row: `words` between its two lines, with `(over, under)` room around them.
fn divider_bar(start: El, words: El, end: El, (over, under): (f32, f32)) -> El {
    row().w_full().pt(over).pb(under).gap(12.0).center_y().child(start).child(words).child(end)
}

/// How far a divider's line runs from its outer end before it reaches full strength, as a
/// fraction of the line.
const RAMP: f32 = 0.7;

/// Paints `line` as every divider's hairline: from nothing at its outer end to the strong border
/// [`RAMP`] of the way in. `end`: the line after the words, which mirrors the first.
fn hairline(p: &Pal, line: El, end: bool) -> El {
    let strong = p.q.border_strong;
    let clear = Color { a: 0.0, ..strong };
    line.linear_gradient(if end { -FRAC_PI_2 } else { FRAC_PI_2 }, [(0.0, clear), (RAMP, strong)])
}

fn summary(c: &Compaction) -> &str {
    if c.status == CompactionStatus::Failed { "" } else { c.summary.trim() }
}

/// What a failure says in its card: none for a cancel or an interruption, which the divider says.
fn error(c: &Compaction) -> Option<&str> {
    let (ty, message) = c.error.as_ref().filter(|_| c.status == CompactionStatus::Failed)?;
    (!matches!(ty.as_str(), "aborted" | "compaction.interrupted") && !message.is_empty()).then_some(message.as_str())
}

pub fn outcome(c: &Compaction) -> &'static str {
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
pub fn usage(c: &Compaction) -> Option<String> {
    let t: &Tokens = c.tokens.as_ref().filter(|_| c.status != CompactionStatus::Running)?;
    let input = t.input + t.cache_read + t.cache_write;
    let output = t.output + t.reasoning;
    (input + output > 0).then(|| format!("{} in · {} out", compact(input), compact(output)))
}

/// `Intl.NumberFormat` with compact notation and at most one fraction digit: 999, 1K, 12.3K, 1.5M.
pub fn compact(n: u64) -> String {
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

fn quiet_view<S: TimelineHost>(s: &S, p: &Pal, c: &Compaction, run: Run) -> El {
    let running = c.status == CompactionStatus::Running;
    let clock = s.clock();
    let seen = s.compactions_state().seen.get(&c.id).copied();
    let enter = seen.and_then(|x| x.enter);
    let at = seen.map_or(clock, |x| x.at);
    let delay = if enter == Some(Enter::Grace) { GRACE } else { 0.0 };
    // The light and the glint start with the fade in, after the grace.
    let start = at + delay;
    let body = summary(c);
    let expandable = !running && !body.is_empty();
    let key = Piece::Quiet.key(&c.id);
    let open = expandable && s.expanded().contains(&key);
    let shutting = expandable && !open && crate::disclosure::is_shutting(s, &key);
    let lit = running && !s.reduced_motion();
    let words = quiet_label(s, p, running, open, start);
    let (over, under) = space(s, run);
    let mut divider = divider_bar(quiet_line(s, p, lit, start, false), words, quiet_line(s, p, lit, start, true), (over, under))
        // A new key restarts the fade in, with everything inside at rest as Compacted.
        .key(if enter == Some(Enter::Now) { "divider-now" } else { "divider" })
        .label(if running { "Compacting" } else { "Compacted" })
        .transition(Transition::none());
    if enter.is_some() && clock - at < delay + 0.5 {
        divider = divider.looping(LoopSpec::sequence(Sequence::new(&ENTER).delay(delay)));
    }
    if expandable {
        let id = c.id.clone();
        divider = divider.role("button").hover_group().cursor(Cursor::Pointer).on_press_tx(move |s: &mut S, tx: &mut Tx| press(s, tx, &id));
    }
    let mut bar = col().key("bar").w_full().child(divider);
    if open || shutting {
        // While open it sticks to the timeline's top until the summary scrolls past, so it shuts
        // from anywhere in it; the summary softens into its fill as a tool group's steps do.
        // Shutting from there, the summary slides away under it, never over it.
        let soft = rune::Shadow { y: 10.0, blur: 20.0, color: p.bg_base, ..Default::default() };
        bar = bar.fill(p.bg_base).sticky_top(0.0).stuck_shadow(soft);
    }
    let mut out = col().w_full().child(bar);
    if open || shutting {
        // The summary is finished text: the row's edge reveals it as it folds open, with no
        // word-by-word fade on top.
        let summary = col()
            .key("summary")
            .w_full()
            // The turn's usual gap under the divider's words, whatever row comes next.
            .pt((16.0 - under).max(0.0))
            .pb(8.0)
            .transition(Transition::none())
            .child(markdown_tinted::<S>(p, body, 0.0, false, p.muted, crate::genui::width(s)))
            .children(usage(c).map(|u| text(u).key("usage").font(UI).color(p.faint).mt(8.0)));
        // Shutting, the summary stays laid out below a row that's only the divider, and the
        // row's clip folds it away; the box keeps its path, so a press doesn't remount it.
        let held = col().key("summary-box").w_full().transition(Transition::none());
        out = out.child(if open { held } else { held.h(0.0) }.child(summary));
    }
    out
}

/// Opens or shuts quiet compaction `id`'s summary, folding on the tool group's spring (the
/// row's `fold_glide`). The divider holds where it was pressed, at the timeline's end too, so
/// the summary pushes what's below it down; pressed while stuck to the top, it stays there as
/// the summary folds away.
pub fn press<S: TimelineHost>(s: &mut S, tx: &mut Tx, id: &str) {
    let row = Piece::Quiet.key(id);
    if crate::disclosure::flip_fold(s, &row) {
        // Opening, the row's top holds: the divider turns sticky in the same commit, and a stuck
        // divider's drawn place can't say how far the timeline's end ran on.
        tx.keep_on_screen(row);
    } else {
        tx.keep_on_screen(format!("{row}/bar"));
    }
}

/// A divider's hairline that brightens when the compaction ends, with a light that gathers
/// toward the label while it runs. `end`: the line after the label, which mirrors the first.
fn quiet_line<S: TimelineHost>(s: &S, p: &Pal, lit: bool, start: f64, end: bool) -> El {
    let base = el()
        .key("base")
        .absolute(0.0, 0.0)
        .w_full()
        .h_full()
        .opacity(if lit { 0.6 } else { 1.0 })
        .opacity_motion(Curve::tween(0.36, Easing::EASE_OUT));
    let base = hairline(p, base, end);
    let mut line = el().key(if end { "end" } else { "start" }).h(1.0).grow(1.0).min_w(0.0).clip_soft(0.0).child(base);
    if lit {
        line = line.child(light(s, p, start, end));
    }
    line
}

/// The light's width, as a fraction of its line.
const BEAM: f32 = 0.4;
/// Where the light starts and ends each pass, in light widths from the line's outer end.
const SLIDE: (f32, f32) = (-1.0, 2.5);
/// Where the light is brightest, as a fraction of its width from its outer edge.
const PEAK: f32 = 0.65;

/// The light: [`BEAM`] of the line wide, it slides in from the outer end and off past the label
/// in the first 70% of each cycle, fading up as it comes and out as it reaches the label. It fades
/// out over 360 ms when the compaction ends.
fn light<S: TimelineHost>(s: &S, p: &Pal, start: f64, end: bool) -> El {
    let peak = Color { a: p.muted.a * 0.55, ..p.muted };
    let clear = Color { a: 0.0, ..p.muted };
    let (from, to) = if end { (-SLIDE.0, -SLIDE.1) } else { SLIDE };
    let slide = LoopSpec { delay: loop_delay(s.clock(), start), ..LoopSpec::slide(f64::from(CYCLE), [from, 0.0], [to, 0.0]).easing(travel()) };
    let beam = el()
        .key("beam")
        .pin((!end).then_some(0.0), Some(0.0), end.then_some(0.0), None)
        .w_frac(BEAM)
        .h_full()
        .linear_gradient(if end { -FRAC_PI_2 } else { FRAC_PI_2 }, [(0.0, clear), (PEAK, peak), (1.0, clear)])
        .looping(slide);
    el().key("light").absolute(0.0, 0.0).w_full().h_full().cycle(cycle(start, light_stops())).transition(fade_out(0.36)).child(beam)
}

/// "Compact" with its ending: "ing" rolls up and out, then "ed" rolls up into its place, in a box
/// as wide as the wider so nothing beside it moves. While it runs, a brighter copy warms the
/// label each time the lights land on it.
fn quiet_label<S: TimelineHost>(s: &S, p: &Pal, running: bool, open: bool, start: f64) -> El {
    let (stem, ing, ed) = ("Compact", "ing", "ed");
    let width = |w: &str| rune::text::measure(w, &[], UI, None).0;
    let ending_w = width(ing).max(width(ed)).ceil();
    let calm = s.reduced_motion();
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
    // The stem and its ending light up as one word, on hover and while open.
    let lit = |t: El| t.font(UI).color(if open { p.muted } else { p.faint }).group_color(p.muted);
    let done = lit(text(ed).key("done"));
    let ending = el()
        .key("ending")
        .w(ending_w)
        .h(16.0)
        .child(morph(text(ing).key("running").font(UI).color(p.faint), running, -shift, out.into()))
        .child(morph(done, !running, shift, into.into()));
    let mut label = row().key("label").child(lit(text(stem).key("stem"))).child(ending);
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

/// The light's opacity: up by 14% of the cycle on the travel's curve, held to 49.5%, out by 70%,
/// and never brighter than the line's own [`RAMP`] where its peak is, so the line's outer end
/// stays faint while the light crosses it.
fn light_stops() -> &'static [(f32, f32)] {
    static STOPS: OnceLock<&'static [(f32, f32)]> = OnceLock::new();
    STOPS.get_or_init(|| {
        let mut shape = vec![(0.0, 0.0)];
        shape.extend(eased(Easing::cubic(0.4, 0.0, 0.6, 1.0), (0.0, 0.0), (0.14, 1.0), 8));
        shape.extend([(0.495, 1.0), (0.7, 0.0), (1.0, 0.0)]);
        let shape = Cycle { period: 1.0, phase: 0.0, stops: Box::leak(shape.into_boxed_slice()), still: 0.0, fps: 60.0 };
        let travel = travel();
        let ramp = |t: f32| {
            let peak = BEAM * (SLIDE.0 + (SLIDE.1 - SLIDE.0) * travel.eval(t) + PEAK);
            (peak / RAMP).clamp(0.0, 1.0)
        };
        let mut v: Vec<(f32, f32)> = (0..=56).map(|i| i as f32 * 0.7 / 56.0).map(|t| (t, shape.sample(f64::from(t)) * ramp(t))).collect();
        v.push((1.0, 0.0));
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
