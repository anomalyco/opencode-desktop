//! Trigger, date, relative-time, status and ordering formatters for Automations
//! (packages/app/src/automations/format.ts, opencode#66393).

use std::cmp::Ordering;

use opencode_i18n::date::{self, Moment, Style, Unit, Width};
use opencode_i18n::{list, t};
use opencode_kit::model::{AutomationInfo, AutomationOutcome, AutomationStatus, AutomationTrigger};

/// Human-readable description of row.trigger (format.ts:triggerLabel).
pub fn trigger_label(row: &AutomationInfo) -> String {
    match &row.trigger {
        AutomationTrigger::At { at, .. } => {
            t!("automations.trigger.at", date = wall_clock(at, row))
        }
        AutomationTrigger::Every { every, .. } => {
            let interval = every.strip_prefix("1 ").unwrap_or(every.as_str());
            t!("automations.trigger.every", interval = interval)
        }
        AutomationTrigger::Cron { cron, .. } => {
            describe_cron(cron).unwrap_or_else(|| cron.clone())
        }
        AutomationTrigger::Event { server, event, .. } => {
            t!("automations.trigger.event", event = event, server = server)
        }
    }
}

/// The raw cron text of a cron trigger whose label is a description of it, for the inspector to
/// show beside the description. One with no description is already its own label.
pub fn described_cron(row: &AutomationInfo) -> Option<&str> {
    match &row.trigger {
        AutomationTrigger::Cron { cron, .. } if describe_cron(cron).is_some() => Some(cron.as_str()),
        _ => None,
    }
}

/// The most times of day a description lists: `0 9,13,17,21 * * *` reads as its four times, while
/// `0 6-22 * * *` keeps its cron text rather than seventeen.
const MAX_TIMES: usize = 6;

/// Covers the common cron shapes ("every weekday at 9"); anything else keeps its raw cron text
/// (format.ts:describeCron). Official describes one time of day only and leaves every other
/// schedule as cron text; this also reads several times a day, every N hours and a day of the
/// month, which are as common in what people ask for.
fn describe_cron(cron: &str) -> Option<String> {
    let fields: Vec<&str> = cron.split_whitespace().collect();
    if fields.len() != 5 {
        return None;
    }
    let minutes = parse_cron_field(fields[0], 0, 59, false)?;
    let hours = parse_cron_field(fields[1], 0, 23, false)?;
    let days = parse_cron_field(fields[2], 1, 31, false)?;
    let months = parse_cron_field(fields[3], 1, 12, false)?;
    let weekdays = parse_cron_field(fields[4], 0, 6, true)?;
    if !months.is_empty() {
        return None;
    }
    let clock = |hour: u32, minute: u32| date::format(Style::Time, Moment { year: 2000, month: 1, day: 1, hour, minute, offset: 0 });

    if !days.is_empty() {
        if let ([day], [hour], [minute], true) = (&days[..], &hours[..], &minutes[..], weekdays.is_empty()) {
            let day = opencode_i18n::integer(i64::from(*day));
            return Some(t!("rune.automations.trigger.monthly", day = day, time = clock(*hour, *minute)));
        }
        return None;
    }

    if let [minute] = minutes[..]
        && !hours.is_empty()
    {
        if weekdays.is_empty()
            && let Some(step) = even_step(&hours, 24).filter(|step| *step > 1)
        {
            return Some(t!("automations.trigger.hours", count = step, minute = format!("{minute:02}")));
        }
        if hours.len() > MAX_TIMES {
            return None;
        }
        let times: Vec<String> = hours.iter().map(|&hour| clock(hour, minute)).collect();
        let time = list(&times);
        if weekdays.is_empty() {
            return Some(t!("automations.trigger.daily", time = time));
        }
        if weekdays == [1, 2, 3, 4, 5] {
            return Some(t!("automations.trigger.weekdays", time = time));
        }
        if weekdays == [0, 6] {
            return Some(t!("automations.trigger.weekends", time = time));
        }
        let names: Vec<&str> = weekdays.iter().map(|&d| date::weekday_name(d, Width::Long)).collect();
        let days = list(&names);
        return Some(t!("automations.trigger.days", days = days, time = time));
    }

    if !hours.is_empty() || !weekdays.is_empty() {
        return None;
    }
    if let [minute] = minutes[..] {
        let minute = format!("{minute:02}");
        return Some(t!("automations.trigger.hourly", minute = minute));
    }
    even_step(&minutes, 60).map(|step| t!("automations.trigger.minutes", count = step))
}

/// The step of `values` when they run from 0 in equal steps through a cycle of `cycle` (`*/15`
/// minutes, `*/2` hours).
fn even_step(values: &[u32], cycle: u32) -> Option<u32> {
    let step = values.get(1)?.checked_sub(*values.first()?).filter(|step| *step > 0)?;
    let even = values[0] == 0
        && cycle.is_multiple_of(step)
        && values.len() == (cycle / step) as usize
        && values.iter().enumerate().all(|(i, &v)| v == i as u32 * step);
    even.then_some(step)
}

fn parse_cron_field(raw: &str, min: u32, max: u32, dow: bool) -> Option<Vec<u32>> {
    if raw == "*" {
        return Some(Vec::new());
    }
    let mut vals = Vec::new();
    for part in raw.split(',') {
        let (range_part, step) = match part.split_once('/') {
            Some((r, s)) => {
                let st: u32 = s.parse().ok()?;
                if st == 0 {
                    return None;
                }
                (r, st)
            }
            None => (part, 1),
        };
        let (start, end) = if range_part == "*" {
            (min, max)
        } else if let Some((a, b)) = range_part.split_once('-') {
            (parse_cron_atom(a, dow)?, parse_cron_atom(b, dow)?)
        } else {
            let v = parse_cron_atom(range_part, dow)?;
            if step == 1 { (v, v) } else { (v, max) }
        };
        if start < min || end > max || start > end {
            return None;
        }
        let mut cur = start;
        while cur <= end {
            vals.push(cur);
            cur = cur.saturating_add(step);
        }
    }
    vals.sort_unstable();
    vals.dedup();
    if vals.len() == (max - min + 1) as usize {
        return Some(Vec::new());
    }
    Some(vals)
}

fn parse_cron_atom(s: &str, dow: bool) -> Option<u32> {
    if dow {
        match s.to_ascii_uppercase().as_str() {
            "SUN" => return Some(0),
            "MON" => return Some(1),
            "TUE" => return Some(2),
            "WED" => return Some(3),
            "THU" => return Some(4),
            "FRI" => return Some(5),
            "SAT" => return Some(6),
            _ => {}
        }
    }
    let n: u32 = s.parse().ok()?;
    if dow && n == 7 { Some(0) } else { Some(n) }
}

fn wall_clock(at: &str, row: &AutomationInfo) -> String {
    if let Some(next) = row.next_due_at {
        return format_date(next, &row.tz);
    }
    if let Some(m) = parse_iso_wall_clock(at) {
        return date::format(Style::DateTime, m);
    }
    at.to_owned()
}

fn parse_iso_wall_clock(s: &str) -> Option<Moment> {
    let (d, t) = s.split_once('T')?;
    let mut dp = d.split('-');
    let year: i32 = dp.next()?.parse().ok()?;
    let month: u32 = dp.next()?.parse().ok()?;
    let day: u32 = dp.next()?.parse().ok()?;
    if dp.next().is_some() || !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    let time_only = t.trim_end_matches('Z').split(['+', '-']).next()?;
    let mut tp = time_only.split(':');
    let hour: u32 = tp.next()?.parse().ok()?;
    let minute: u32 = tp.next()?.parse().ok()?;
    if hour > 23 || minute > 59 {
        return None;
    }
    Some(Moment { year, month, day, hour, minute, offset: 0 })
}

/// Formats Unix milliseconds as medium date + short time (format.ts:formatDate).
pub fn format_date(at_ms: i64, _tz: &str) -> String {
    let secs = at_ms.div_euclid(1000);
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400) as u32;
    let (year, month, day) = civil_from_days(days);
    let moment = Moment {
        year,
        month,
        day,
        hour: rem / 3600,
        minute: (rem % 3600) / 60,
        offset: 0,
    };
    date::format(Style::DateTime, moment)
}

fn civil_from_days(z: i64) -> (i32, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u32;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = y + i64::from(m <= 2);
    (y as i32, m, d)
}

/// Compact duration until the next run ("5m", "2h", "3d") for the list pill chip
/// (`packages/app/src/automations/format.ts:108-120`, `opencode#66899`).
pub fn short_duration(at: i64, now_ms: i64) -> String {
    let minutes = (((at - now_ms) as f64) / 60_000.0).round().max(1.0) as i64;
    if minutes < 60 {
        format!("{minutes}m")
    } else if minutes < 1440 {
        let hours = ((minutes as f64) / 60.0).round().max(1.0) as i64;
        format!("{hours}h")
    } else {
        let days = ((minutes as f64) / 1440.0).round().max(1.0) as i64;
        format!("{days}d")
    }
}

/// Relative time matching Intl.RelativeTimeFormat(locale, { numeric: "auto" })
/// (format.ts:relativeTime).
pub fn relative_time(at_ms: i64, now_ms: i64) -> String {
    let seconds = (at_ms - now_ms) as f64 / 1000.0;
    let minutes = seconds / 60.0;
    let hours = minutes / 60.0;
    if seconds.abs() < 60.0 {
        return date::relative(0, Unit::Second);
    }
    if minutes.abs() < 60.0 {
        return date::relative(minutes.round() as i64, Unit::Minute);
    }
    if hours.abs() < 24.0 {
        return date::relative(hours.round() as i64, Unit::Hour);
    }
    let days = (hours / 24.0).round() as i64;
    date::relative_day(days)
        .map(str::to_owned)
        .unwrap_or_else(|| date::relative(days, Unit::Day))
}

/// Status string for the inspector subtitle (format.ts:statusLabel).
pub fn status_label(row: &AutomationInfo) -> &'static str {
    match (row.status, row.last_outcome) {
        (AutomationStatus::Paused, Some(AutomationOutcome::Failed)) => t!("automations.status.failed"),
        (AutomationStatus::Done, Some(AutomationOutcome::Expired)) => t!("automations.status.expired"),
        (AutomationStatus::Active, _) => t!("automations.status.active"),
        (AutomationStatus::Paused, _) => t!("automations.status.paused"),
        (AutomationStatus::Done, _) => t!("automations.status.done"),
    }
}

/// What a run that didn't simply fire says beside its time; a run that fired says nothing.
pub fn outcome_note(outcome: Option<AutomationOutcome>) -> Option<&'static str> {
    match outcome? {
        AutomationOutcome::Fired => None,
        AutomationOutcome::Late => Some(t!("rune.automations.outcome.late")),
        AutomationOutcome::Skipped => Some(t!("rune.automations.outcome.skipped")),
        AutomationOutcome::Disabled => Some(t!("rune.automations.outcome.disabled")),
        AutomationOutcome::Failed => Some(t!("automations.note.failed")),
        AutomationOutcome::Expired => Some(t!("automations.status.expired")),
    }
}

/// The session a run woke, from the first line of its output.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Delivery<'a> {
    Prompted(&'a str),
    Subagent(&'a str),
}

impl<'a> Delivery<'a> {
    pub fn session(self) -> &'a str {
        match self {
            Self::Prompted(id) | Self::Subagent(id) => id,
        }
    }
}

/// A run's output as the session it woke and what it logged after: the server writes
/// `Prompted <session>` or `Started subagent <session>` as the first line
/// (packages/core/src/automation.ts:217 `deliverySummary`). Output that starts any other way is
/// all log.
pub fn split_output(output: &str) -> (Option<Delivery<'_>>, &str) {
    let (first, rest) = output.split_once('\n').unwrap_or((output, ""));
    let session = |id: &'_ str| id.starts_with("ses_") && !id.contains(char::is_whitespace);
    let delivery = first
        .strip_prefix("Started subagent ")
        .filter(|id| session(id))
        .map(Delivery::Subagent)
        .or_else(|| first.strip_prefix("Prompted ").filter(|id| session(id)).map(Delivery::Prompted));
    match delivery {
        Some(delivery) => (Some(delivery), rest),
        None => (None, output),
    }
}

/// Row sort order: Active (earliest next_due_at first), then Paused, then Done; ties broken by
/// time.updated descending (format.ts:compareAutomations).
pub fn compare_automations(a: &AutomationInfo, b: &AutomationInfo) -> Ordering {
    let rank = |s: AutomationStatus| match s {
        AutomationStatus::Active => 0u8,
        AutomationStatus::Paused => 1u8,
        AutomationStatus::Done => 2u8,
    };
    rank(a.status)
        .cmp(&rank(b.status))
        .then_with(|| {
            if a.status == AutomationStatus::Active {
                a.next_due_at.unwrap_or(i64::MAX).cmp(&b.next_due_at.unwrap_or(i64::MAX))
            } else {
                Ordering::Equal
            }
        })
        .then_with(|| b.time.updated.cmp(&a.time.updated))
        .then_with(|| a.id.cmp(&b.id))
}
