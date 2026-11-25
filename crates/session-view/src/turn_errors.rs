//! A turn's error, the Retry row and the session's notices, as Desktop draws them
//! (`session-timeline-row.tsx`: `Error`, `Retry`, `Notice`; `session-error.tsx`,
//! `session-retry.tsx`, `timeline-separator.tsx`).
//!
//! An error is a warning mark and its message, or for a spent allowance or budget one muted
//! line saying when it comes back, with Switch model for a budget. While the server waits to try
//! a step again, the Retry row says what failed and counts down to the next attempt in place.

use std::cell::Cell;

use opencode_kit::model::{Assistant, Notice, NoticeKind, Retry, StepError, capitalize};
use rune::{El, Link, LinkPaint, LocalTime, Span, Tx, col, icon, row, text};

use crate::TimelineHost;
use crate::palette::Pal;
use crate::style::{UI, UI_MED, readable};

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

pub use opencode_ui::style::moment;

/// `3:45 PM`, `15:45`: `Intl`'s `timeStyle: "short"` in the locale.
pub fn short_time(t: LocalTime) -> String {
    opencode_i18n::date::format(opencode_i18n::date::Style::Time, moment(t))
}

/// `Oct 31, 2026, 3:45 PM`: `dateStyle: "medium"` with a short time.
pub fn date_time(t: LocalTime) -> String {
    opencode_i18n::date::format(opencode_i18n::date::Style::DateTime, moment(t))
}

/// `Nov 1, 12:00 AM GMT-7`: a month, day, time and `shortOffset` zone.
pub fn budget_time(t: LocalTime) -> String {
    opencode_i18n::date::format(opencode_i18n::date::Style::MonthDayTimeZone, moment(t))
}

/// What a limit's row says, at `now` (epoch ms), with its action for a budget.
pub fn limit_copy(kind: Limit, at: Option<i64>, now: i64) -> (String, Option<&'static str>) {
    use opencode_i18n::t;
    match kind {
        Limit::Free | Limit::Go => {
            let go = kind == Limit::Go;
            let title = if go { t!("ui.sessionTurn.goLimit.title") } else { t!("ui.sessionTurn.freeLimit.title") };
            let Some(at) = at else { return (title.to_owned(), None) };
            let (t, today) = (LocalTime::at(at), LocalTime::at(now));
            let same_day = (t.year, t.month, t.day) == (today.year, today.month, today.day) && at - now < 86_400_000;
            let time = if same_day { short_time(t) } else { date_time(t) };
            (if go { t!("ui.sessionTurn.goLimit.lineReset", time = time) } else { t!("ui.sessionTurn.freeLimit.lineReset", time = time) }, None)
        }
        Limit::Budget => (
            match at {
                Some(at) => t!("ui.sessionTurn.budgetLimit.all", time = budget_time(LocalTime::at(at))),
                None => t!("ui.sessionTurn.budgetLimit.allUntilReset").to_owned(),
            },
            Some(t!("ui.sessionTurn.budgetLimit.useFreeModel")),
        ),
        Limit::ModelBudget => (
            match at {
                Some(at) => t!("ui.sessionTurn.budgetLimit.models", time = budget_time(LocalTime::at(at))),
                None => t!("ui.sessionTurn.budgetLimit.modelsUntilReset").to_owned(),
            },
            Some(t!("ui.sessionTurn.budgetLimit.switchModel")),
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
    icon(&rune_theme_opencode::icons::OUTLINE_HEXAGONAL_WARNING, 16.0).key("mark").color(p.danger)
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
                // A plain `<button>` (`timeline-next/row.tsx:1536`): the arrow.
                .cursor(rune::Cursor::Default)
                .on_link(|s: &mut S, _: &str, tx: &mut Tx| {
                    // A button: a middle click on it does nothing.
                    if tx.button() == rune::PointerButton::Middle {
                        return;
                    }
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
        opencode_i18n::t!("ui.sessionTurn.retry.geminiHot").to_owned()
    } else {
        unwrap_error(&r.message)
    };
    let secs = countdown(r, now);
    let info = if secs > 0 {
        opencode_i18n::plural!("ui.sessionTurn.retry.attemptWaiting", secs, attempt = r.attempt)
    } else {
        opencode_i18n::t!("ui.sessionTurn.retry.attemptRetryingNow", attempt = r.attempt)
    };
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
    matches!(n.kind, NoticeKind::Moved { .. } | NoticeKind::Model { .. } | NoticeKind::Agent { .. })
}

/// A notice row; a divider at `run`.
pub fn notice<S: TimelineHost>(s: &S, p: &Pal, n: &Notice, run: crate::compaction::Run) -> El {
    let faint_med = |key: &'static str, words: String| text(words).key(key).font(UI_MED).color(p.faint).truncate_fade().shrink().min_w(0.0);
    let faint = |key: &'static str, words: String| text(words).key(key).font(UI).color(p.faint).truncate_fade().shrink().min_w(0.0);
    // A notice that only tells leads with a faint bare mark, as the timeline's events do (t1582);
    // its words stay its name.
    let line = |m: crate::activity::event_row::Mark, words: &str, kids: Vec<El>| {
        row().key("notice").w_full().gap(8.0).center_y().min_h(20.0).role("status").label(words.to_owned())
            .child(crate::activity::event_row::mark(p, m, words)).children(kids)
    };
    use crate::activity::event_row::Mark;
    match &n.kind {
        // Switching the agent changes who answers next, so it draws as the model's separator does
        // (`timeline-next/row.tsx`'s `switched`).
        NoticeKind::Agent { agent, .. } => {
            let words = text(opencode_i18n::t!("ui.timeline.notice.agentSwitched", agent = capitalize(agent))).key("label").font(UI).color(p.faint);
            crate::compaction::divider_of(s, p, words, run)
        }
        NoticeKind::Instructions { sources } => {
            let label = opencode_i18n::t!("ui.sessionTimeline.notice.instructionsUpdated");
            let mut kids = vec![faint_med("label", label.into())];
            if !sources.is_empty() {
                kids.push(faint("sources", sources.join(", ")));
            }
            line(Mark::Updated, label, kids)
        }
        NoticeKind::Restart => {
            let label = opencode_i18n::t!("ui.sessionTimeline.notice.restart");
            line(Mark::Info, label, vec![faint_med("label", label.into())])
        }
        // Background work reporting back is an event (`noticeEvent`): a subagent's a row of its
        // own, a shell's counted ("Finished 1 shell").
        NoticeKind::Finished(f) if f.subagent => match crate::activity::events::notice_event(n) {
            Some(e) => crate::activity::event_row::subagent(s, p, session_of(s), &e),
            None => col(),
        },
        NoticeKind::Finished(_) => {
            let key = format!("ev-{}", n.id);
            let open = s.expanded().contains(&key);
            let events: Vec<_> = crate::activity::events::notice_event(n).into_iter().collect();
            crate::activity::event_row::view(
                s,
                p,
                session_of(s),
                &events,
                open,
                crate::disclosure::set::<S>(key.clone(), open, String::new()),
                crate::disclosure::set::<S>(key.clone(), true, String::new()),
            )
        }
        NoticeKind::Idle => col(),
        // Finished background Code Mode script (`timeline-next/row.tsx`, anomalyco/opencode#64741):
        // "Script finished" (or failed / cancelled) in its outcome's ink (`p.cancelled` maps to
        // `yellow-900` in light and `state-fg-warning` in dark, matching `outcomeNoticeColor`),
        // then the script's first line as the activity list names it. Official shows the job's
        // `cm_…` id there, which tells a reader nothing; the id stays out of the row.
        NoticeKind::Job { id, script: Some(outcome), .. } => {
            use opencode_kit::model::Outcome;
            let (label, m, ink) = match outcome {
                Outcome::Error => (opencode_i18n::t!("ui.sessionTimeline.notice.failed", actor = "Script"), Mark::Failed, p.danger),
                Outcome::Cancelled => (opencode_i18n::t!("ui.sessionTimeline.notice.cancelled", actor = "Script"), Mark::Stopped, p.cancelled),
                Outcome::Completed => (opencode_i18n::t!("ui.sessionTimeline.notice.finished", actor = "Script"), Mark::Completed, p.v2.text_code_accent),
            };
            let command = script_line(s, id);
            let words = command.as_ref().map_or_else(|| label.clone(), |c| format!("{label} {c}"));
            // The label keeps its width; a long command gives way, cut by its fade.
            let mut kids = vec![text(label).key("label").font(UI_MED).color(ink)];
            if let Some(c) = command {
                let c = text(c).key("command").dir(rune::Direction::Ltr).font(crate::style::TOOL_CODE.lh(16.0)).color(p.text).truncate_fade();
                kids.push(readable(p, c).shrink().min_w(0.0));
            }
            line(m, &words, kids)
        }
        NoticeKind::Text(t) | NoticeKind::Job { description: t, .. } => line(Mark::Info, t, vec![faint_med("label", t.clone())]),
        NoticeKind::Moved { directory } => {
            let words = row().key("words").gap(6.0).center_y().shrink().min_w(0.0)
                .child(text(opencode_i18n::t!("ui.sessionTimeline.notice.movedTo")).key("label").font(UI).color(p.faint))
                .child(faint("value", directory.clone()));
            crate::compaction::divider_of(s, p, words, run)
        }
        NoticeKind::Model { model, previous } => {
            let variant_only = previous.as_ref().is_some_and(|x| x.provider == model.provider && x.id == model.id);
            // A model's faster and slower speeds are models of their own ("gpt-5.5-fast"), so moving
            // between them is a change of speed (`timeline-next/row.tsx`, opencode#65749).
            let speed = previous.as_ref().and_then(|x| s.model_speed_switch(model, x));
            let (label, variant) = if let Some(speed) = speed {
                let label = match speed {
                    Some(speed) => opencode_i18n::t!("ui.sessionTimeline.notice.speedSwitched", speed = speed),
                    None => opencode_i18n::t!("ui.sessionTimeline.notice.speedStandard").to_owned(),
                };
                (label, None)
            } else if variant_only {
                let label = if model.variant.is_empty() || model.variant == "default" {
                    opencode_i18n::t!("ui.sessionTimeline.notice.variantDefault").to_owned()
                } else {
                    opencode_i18n::t!("ui.sessionTimeline.notice.variantSwitched", variant = model.variant)
                };
                (label, None)
            } else {
                (opencode_i18n::t!("ui.sessionTimeline.notice.modelSwitched", model = s.model_name(model)), Some(model.variant.as_str()).filter(|v| !v.is_empty() && *v != "default"))
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

/// The first non-blank line of the `execute` call that started script job `id`, on one line, as
/// the activity list names a running script (`subagents::jobs::scripts`). The notice carries only
/// the job's id and output; the call that went to the background holds the code, `metadata.jobID`.
fn script_line<S: TimelineHost>(s: &S, id: &str) -> Option<String> {
    use opencode_kit::model::{Message, Part};
    if id.is_empty() {
        return None;
    }
    s.with_transcript(session_of(s), |t| {
        t.messages.iter().rev().filter_map(|m| match m {
            Message::Assistant(a) => Some(a),
            _ => None,
        }).flat_map(|a| a.parts.iter().rev()).find_map(|p| match p {
            Part::Tool { name, input, metadata, .. } if name == "execute" && metadata["jobID"].as_str() == Some(id) => {
                input["code"].as_str()?.lines().map(str::trim).find(|l| !l.is_empty()).map(crate::tools::plain::one_line)
            }
            _ => None,
        })
    }).flatten()
}

/// The session a notice row draws in: the open one.
fn session_of<S: TimelineHost>(s: &S) -> &str {
    s.active_session_id().unwrap_or_default()
}
