//! A turn's error, the Retry row and the session's notices, as Desktop draws them
//! (`session-timeline-row.tsx`: `Error`, `Retry`, `Notice`; `session-error.tsx`,
//! `session-retry.tsx`, `timeline-separator.tsx`).
//!
//! An error is a warning mark and its message, or for a spent allowance or budget one muted
//! line saying when it comes back, with Switch model for a budget. While the server waits to try
//! a step again, the Retry row says what failed and counts down to the next attempt in place.

use std::cell::Cell;

use opencode_kit::model::{Assistant, Notice, NoticeKind, Retry, StepError, capitalize};
use rune::{El, Icon, Link, LinkPaint, LocalTime, Span, Tx, col, icon, row, text};

use crate::TimelineHost;
use crate::palette::Pal;
use crate::style::{UI, UI_MED, readable};

/// Desktop's `outline-hexagonal-warning`.
static HEX_WARNING: Icon = Icon {
    name: "outline-hexagonal-warning",
    view: 16.0,
    body: r#"<path d="M10.6667 1.33325H5.33272L1.33334 5.33325L1.33342 10.6917L5.3327 14.691L10.6667 14.6909L14.6911 10.6917V5.33255L10.6667 1.33325Z" stroke="currentColor"/><path d="M8 10.6667H8.00667" stroke="currentColor" stroke-linecap="square"/><path d="M8 7.99992V5.33325" stroke="currentColor" stroke-linecap="square"/>"#,
};

/// Desktop drops a provider's wait past a week as bogus.
const MAX_RETRY_AFTER_MS: i64 = 7 * 86_400_000;

/// A spent allowance or budget, which reads as a notice rather than an error.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Limit {
    /// OpenCode Zen's daily free messages.
    Free,
    /// An OpenCode Go subscription's usage limit.
    Go,
    /// An OpenCode Console budget on all spend: paid models pause.
    Budget,
    /// A budget on some models: those pause.
    ModelBudget,
}

/// The limit a step's error is, and when it resets (epoch ms), if known.
pub fn limit(a: &Assistant) -> Option<(Limit, Option<i64>)> {
    let e = a.failure.as_ref()?;
    Some(match e.kind.as_str() {
        "provider.free-limit" | "provider.go-limit" => {
            let kind = if e.kind == "provider.go-limit" { Limit::Go } else { Limit::Free };
            let base = a.completed.unwrap_or(a.created);
            let at = e.retry_after_ms.filter(|d| (0..=MAX_RETRY_AFTER_MS).contains(d)).filter(|_| base > 0).map(|d| base + d);
            (kind, at)
        }
        "provider.budget-limit" | "provider.model-budget-limit" => {
            let kind = if e.kind == "provider.budget-limit" { Limit::Budget } else { Limit::ModelBudget };
            (kind, resets_at(e))
        }
        _ => return None,
    })
}

/// OpenCode Console names when a budget resets in its refusal's body, as `resetsAt`.
fn resets_at(e: &StepError) -> Option<i64> {
    let body = e.body.as_deref()?;
    let at = body.find("\"resetsAt\"")? + "\"resetsAt\"".len();
    let rest = body[at..].trim_start().strip_prefix(':')?.trim_start().strip_prefix('"')?;
    parse_iso(&rest[..rest.find('"')?])
}

/// `2026-11-01T00:00:00Z` or with `.000` and a `+hh:mm` offset, in epoch ms.
pub fn parse_iso(t: &str) -> Option<i64> {
    let n = |r: std::ops::Range<usize>| t.get(r)?.parse::<i64>().ok();
    let (y, mo, d, h, mi, sec) = (n(0..4)?, n(5..7)?, n(8..10)?, n(11..13)?, n(14..16)?, n(17..19)?);
    let mut rest = &t[19..];
    if let Some(frac) = rest.strip_prefix('.') {
        rest = frac.trim_start_matches(|c: char| c.is_ascii_digit());
    }
    let offset = match rest {
        "Z" | "" => 0,
        o => {
            let sign = if o.starts_with('-') { -1 } else { 1 };
            let hm = o.get(1..)?;
            let (oh, om) = hm.split_once(':').unwrap_or((hm.get(..2)?, hm.get(2..).unwrap_or("0")));
            sign * (oh.parse::<i64>().ok()? * 60 + om.parse::<i64>().ok()?) * 60
        }
    };
    Some((days_from_civil(y, mo, d) * 86_400 + h * 3600 + mi * 60 + sec - offset) * 1000)
}

fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * (m + if m > 2 { -3 } else { 9 }) + 2) / 5 + d - 1;
    era * 146_097 + yoe * 365 + yoe / 4 - yoe / 100 + doy - 719_468
}

const MONTHS: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];

/// `3:45 PM`, as `Intl` formats `timeStyle: "short"` in English.
pub fn short_time(t: LocalTime) -> String {
    let h = match t.hour % 12 { 0 => 12, h => h };
    format!("{h}:{:02} {}", t.minute, if t.hour < 12 { "AM" } else { "PM" })
}

/// `Oct 31, 2026, 3:45 PM`: `dateStyle: "medium"` with a short time.
pub fn date_time(t: LocalTime) -> String {
    format!("{} {}, {}, {}", MONTHS[t.month as usize - 1], t.day, t.year, short_time(t))
}

/// `Nov 1, 12:00 AM GMT-7`: a month, day, time and `shortOffset` zone.
pub fn budget_time(t: LocalTime) -> String {
    let zone = match t.offset {
        0 => "GMT".to_owned(),
        o => {
            let (sign, a) = (if o < 0 { '-' } else { '+' }, o.abs());
            let (h, m) = (a / 3600, a % 3600 / 60);
            if m == 0 { format!("GMT{sign}{h}") } else { format!("GMT{sign}{h}:{m:02}") }
        }
    };
    format!("{} {}, {} {zone}", MONTHS[t.month as usize - 1], t.day, short_time(t))
}

/// What a limit's row says, at `now` (epoch ms), with its action for a budget.
pub fn limit_copy(kind: Limit, at: Option<i64>, now: i64) -> (String, Option<&'static str>) {
    match kind {
        Limit::Free | Limit::Go => {
            let (title, reset) = if kind == Limit::Go {
                ("You've reached your OpenCode Go usage limit.", "You've reached your OpenCode Go usage limit. It resets at")
            } else {
                ("You've used today's free messages.", "You've used today's free messages. They reset at")
            };
            let Some(at) = at else { return (title.to_owned(), None) };
            let (t, today) = (LocalTime::at(at), LocalTime::at(now));
            let same_day = (t.year, t.month, t.day) == (today.year, today.month, today.day) && at - now < 86_400_000;
            (format!("{reset} {}.", if same_day { short_time(t) } else { date_time(t) }), None)
        }
        Limit::Budget => (
            match at {
                Some(at) => format!("Workspace budget reached · paid models paused until {} · free models still work", budget_time(LocalTime::at(at))),
                None => "Workspace budget reached · paid models paused until it resets · free models still work".to_owned(),
            },
            Some("Use a free model"),
        ),
        Limit::ModelBudget => (
            match at {
                Some(at) => format!("Model budget reached · this model is paused until {}", budget_time(LocalTime::at(at))),
                None => "Model budget reached · this model is paused until it resets".to_owned(),
            },
            Some("Switch model"),
        ),
    }
}

/// Desktop's `unwrapErrorMessage`: a provider's JSON error envelope as `type: message`.
pub fn unwrap_error(message: &str) -> String {
    let text = message.strip_prefix("Error:").unwrap_or(message).trim();
    let read = |v: &str| -> Option<serde_json::Value> {
        let first: serde_json::Value = serde_json::from_str(v).ok()?;
        match first.as_str() {
            Some(inner) => serde_json::from_str(inner.trim()).ok(),
            None => Some(first),
        }
    };
    let json = read(text).or_else(|| {
        let (a, b) = (text.find('{')?, text.rfind('}')?);
        (b > a).then(|| read(&text[a..=b])).flatten()
    });
    let Some(env) = json.filter(|j| j.is_object()) else { return message.to_owned() };
    let e = &env["error"];
    let field = |k: &str| e[k].as_str().filter(|x| !x.is_empty());
    match (field("type"), field("message")) {
        (Some(t), Some(m)) => format!("{t}: {m}"),
        (None, Some(m)) => m.to_owned(),
        (Some(t), None) => t.to_owned(),
        _ => field("code")
            .or_else(|| env["message"].as_str())
            .or_else(|| e.as_str())
            .map_or_else(|| message.to_owned(), str::to_owned),
    }
}

fn full_len(words: &str, action: &str) -> usize {
    words.len() + " · ".len() + action.len()
}

/// A message with what's before its first colon in bold, as `SessionErrorMessage` sets it.
fn titled(p: &Pal, message: &str, wrap: bool) -> El {
    let mut t = text(message.to_owned()).key("message").font(UI).color(p.text);
    if let Some(at) = message.find(':') {
        t = t.spans(vec![Span::new(0, at + 1).weight(530.0)]);
    }
    if wrap { readable(p, t.wrap().grow(1.0).shrink().min_w(0.0)) } else { readable(p, t.truncate_fade().shrink().min_w(0.0)) }
}

fn mark(p: &Pal) -> El {
    // On the first line's middle, as `items-start` puts it beside 16 pt lines.
    icon(&HEX_WARNING, 16.0).key("mark").color(p.danger)
}

/// The turn's error row: a limit's notice, or the warning mark and the error.
pub fn error<S: TimelineHost>(s: &S, p: &Pal, a: &Assistant) -> El {
    if let Some((kind, at)) = limit(a) {
        let (words, action) = limit_copy(kind, at, s.now_ms());
        let mut line = text(words.clone()).key("words").font(UI).color(p.muted).wrap().w_full();
        if let Some(action) = action.filter(|_| s.can_switch_model()) {
            // The action flows on in the same paragraph, as Desktop's inline button does.
            let full = format!("{words} · {action}");
            let at = full.len() - action.len();
            line = text(full).key("words").font(UI).color(p.muted).wrap().w_full()
                .spans(vec![Span::new(at, full_len(&words, action)).weight(530.0).color(p.text)])
                .link(Link::new(at..full_len(&words, action), "switch-model").role("button").label(action).hover(LinkPaint::new().underline(p.text)))
                .on_link(|s: &mut S, _: &str, tx: &mut Tx| {
                    tx.instant();
                    s.switch_model(tx);
                });
        }
        return row().key("limit").w_full().role("status").label(words).child(readable(p, line));
    }
    let message = unwrap_error(a.failure.as_ref().map_or(a.error.as_deref().unwrap_or(""), |f| f.message.as_str()));
    row().key("error").w_full().gap(8.0).role("alert").label(message.clone()).child(mark(p)).child(titled(p, &message, true))
}

/// The Retry row's lines: what failed, and the attempt with its countdown at `now`.
pub fn retry_copy(r: &Retry, now: i64) -> (String, String) {
    let message = if r.message.contains("exceeded your current quota") && r.message.contains("gemini") {
        "gemini is way too hot right now".to_owned()
    } else {
        unwrap_error(&r.message)
    };
    let secs = countdown(r, now);
    let info = if secs > 0 { format!("Attempt {} - retrying in {secs}s", r.attempt) } else { format!("Attempt {} - retrying", r.attempt) };
    (message, info)
}

/// Whole seconds to the next attempt, as Desktop rounds them.
fn countdown(r: &Retry, now: i64) -> i64 {
    ((r.at - now) as f64 / 1000.0).round().max(0.0) as i64
}

/// While the server waits to try the step again: what failed, and a countdown that changes in
/// place.
pub fn retry<S: TimelineHost>(s: &S, p: &Pal, r: &Retry) -> El {
    let (message, info) = retry_copy(r, s.now_ms());
    let body = col().key("body").grow(1.0).shrink().min_w(0.0)
        .child(row().key("what").w_full().child(titled(p, &message, false)))
        .child(text(info).key("info").font(UI).color(p.faint).pt(4.0));
    row().key("retry").w_full().gap(8.0).role("status").child(mark(p)).child(body)
}

thread_local! {
    /// The countdown the open session last drew.
    static SHOWN: Cell<Option<(u32, i64)>> = const { Cell::new(None) };
}

/// Redraws the open session's countdown when its number changes, and wakes for the next one.
pub fn poll<S: TimelineHost>(s: &S, tx: &mut Tx) -> bool {
    let Some(id) = s.active_session_id() else { return false };
    let now_ms = s.now_ms();
    let retrying = s
        .with_transcript(id, |t| if t.busy() { t.retrying().map(|r| (r.attempt, r.at, countdown(r, now_ms))) } else { None })
        .flatten();
    let now = retrying.map(|(attempt, _, count)| (attempt, count));
    let changed = SHOWN.replace(now) != now;
    if let Some((_, at, _)) = retrying
        && at > now_ms
    {
        // The next half second past `at`, where the rounded count drops.
        let left = (at - now_ms - 500).rem_euclid(1000) + 1;
        tx.wake_after(std::time::Duration::from_millis(left as u64));
    }
    changed
}

/// Whether notice `n` draws as a divider.
pub fn is_divider(n: &Notice) -> bool {
    matches!(n.kind, NoticeKind::Moved { .. } | NoticeKind::Model { .. })
}

/// A notice row; a divider at `run`.
pub fn notice<S: TimelineHost>(s: &S, p: &Pal, n: &Notice, run: crate::compaction::Run) -> El {
    let faint_med = |key: &'static str, words: String| text(words).key(key).font(UI_MED).color(p.faint).truncate_fade().shrink().min_w(0.0);
    let faint = |key: &'static str, words: String| text(words).key(key).font(UI).color(p.faint).truncate_fade().shrink().min_w(0.0);
    let line = |kids: Vec<El>| row().key("notice").w_full().gap(8.0).center_y().min_h(20.0).pt(12.0).pb(4.0).children(kids);
    match &n.kind {
        NoticeKind::Agent { agent, previous } => {
            let data = match previous {
                Some(prev) if !prev.is_empty() => format!("{} → {}", capitalize(prev), capitalize(agent)),
                _ => capitalize(agent),
            };
            line(vec![faint_med("label", "Agent changed".into()), faint("data", data)])
        }
        NoticeKind::Instructions { sources } => {
            let mut kids = vec![faint_med("label", "Instructions updated".into())];
            if !sources.is_empty() {
                kids.push(faint("sources", sources.join(", ")));
            }
            line(kids)
        }
        NoticeKind::Restart => line(vec![faint_med("label", "Continuing after restart".into())]),
        NoticeKind::Text(t) => line(vec![faint_med("label", t.clone())]),
        NoticeKind::Moved { directory } => {
            let words = row().key("words").gap(6.0).center_y().shrink().min_w(0.0)
                .child(text("Moved to").key("label").font(UI).color(p.faint))
                .child(faint("value", directory.clone()));
            crate::compaction::divider_of(s, p, words, run)
        }
        NoticeKind::Model { model, previous } => {
            let variant_only = previous.as_ref().is_some_and(|x| x.provider == model.provider && x.id == model.id);
            let (label, variant) = if variant_only {
                let v = if model.variant.is_empty() || model.variant == "default" { "Default".to_owned() } else { model.variant.clone() };
                (format!("Switched variant to {v}"), None)
            } else {
                (format!("Switched to {}", s.model_name(model)), Some(model.variant.as_str()).filter(|v| !v.is_empty() && *v != "default"))
            };
            let mut words = row().key("words").gap(6.0).center_y().shrink().min_w(0.0).child(faint("label", label));
            if let Some(v) = variant {
                words = words
                    .child(rune::el().key("dot").size(2.25, 2.25).radius(2.0).fill(p.faint))
                    .child(text(capitalize(v)).key("variant").font(UI).color(p.faint));
            }
            crate::compaction::divider_of(s, p, words, run)
        }
    }
}
