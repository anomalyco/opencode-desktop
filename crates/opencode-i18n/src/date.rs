//! Dates and times in the current locale, as Desktop's `Intl.DateTimeFormat` and
//! `Intl.RelativeTimeFormat` calls write them: month and weekday names, day periods, the locale's
//! digits, hour cycle and calendar (Thai's Buddhist years, Persian's solar months), from the CLDR
//! patterns `sync.ts` writes to `locales/intl`.

use crate::{I18N, Key, PluralKey, key, plural_key, t};

/// A wall-clock moment in its own time zone, as the platform reports local time.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Moment {
    pub year: i32,
    /// 1–12.
    pub month: u32,
    /// 1–31.
    pub day: u32,
    /// 0–23.
    pub hour: u32,
    pub minute: u32,
    /// Seconds east of UTC.
    pub offset: i32,
}

impl Moment {
    /// 0 for Sunday through 6 for Saturday.
    pub fn weekday(&self) -> u32 {
        (days_from_civil(self.year, self.month, self.day) + 4).rem_euclid(7) as u32
    }

    /// Days since 1970-01-01 of its calendar date.
    pub fn days(&self) -> i64 {
        days_from_civil(self.year, self.month, self.day)
    }
}

/// The `Intl.DateTimeFormat` options Desktop formats with.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Style {
    /// `{ timeStyle: "short" }`: `3:45 PM`, `15:45`.
    Time,
    /// `{ month: "short", day: "numeric" }`: `Nov 3`, `3. Nov.`.
    MonthDay,
    /// `{ day: "numeric", month: "short", year: "numeric" }`: `Nov 3, 2026`.
    MonthDayYear,
    /// `{ dateStyle: "medium", timeStyle: "short" }`: `Nov 3, 2026, 3:45 PM`, `03.11.2026, 15:45`.
    DateTime,
    /// `{ month: "short", day: "numeric", hour: "numeric", minute: "2-digit" }`: `Nov 3, 3:45 PM`.
    MonthDayTime,
    /// `MonthDayTime` with `year: "numeric"`: `Nov 3, 2026, 3:45 PM`.
    MonthDayYearTime,
    /// `MonthDayTime` with `timeZoneName: "shortOffset"`: `Nov 3, 3:45 PM GMT-7`.
    MonthDayTimeZone,
}

/// A name's width: `Nov`, `November`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Width {
    Short,
    Long,
}

/// A `Intl.RelativeTimeFormat` unit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Unit {
    Second,
    Minute,
    Hour,
    Day,
    Week,
    Month,
    Year,
}

/// `at` in `style`, as `new Intl.DateTimeFormat(locale, style).format(at)` writes it.
pub fn format(style: Style, at: Moment) -> String {
    format_in(I18N.locale(), style, at)
}

/// `at` in `style` in the locale at index `locale` (`crate::locales`), whatever the chosen one:
/// `format_in(crate::system_locale(), …)` is Desktop's `toLocaleTimeString(undefined, …)`.
pub fn format_in(locale: usize, style: Style, at: Moment) -> String {
    // The system's locale need not be the one the app shows: its messages may still be in the bundle.
    #[cfg(bundle_assets)]
    crate::bundle::load(locale, crate::locales()[locale].id());
    let text = |key| I18N.text_in(locale, key);
    let (pattern, months, periods) = style_keys(style);
    let (year, month, day) = calendar_date(locale, at);
    let digits = |s: String| digits(locale, s);
    let mut out = String::new();
    let mut rest = text(pattern);
    while let Some(open) = rest.find("{{") {
        out.push_str(&rest[..open]);
        let Some(close) = rest[open..].find("}}") else { break };
        let token = &rest[open + 2..open + close];
        rest = &rest[open + close + 2..];
        let twelve = match at.hour % 12 { 0 => 12, h => h };
        match token {
            "month" => out.push_str(months.map_or("", |k| nth(text(k), month))),
            "dayPeriod" => out.push_str(periods.map_or("", |k| nth(text(k), at.hour + 1))),
            "zone" => out.push_str(&zone_in(locale, at.offset)),
            "y" => out.push_str(&digits(year.to_string())),
            "d" => out.push_str(&digits(day.to_string())),
            "dd" => out.push_str(&digits(format!("{day:02}"))),
            "m" => out.push_str(&digits(at.minute.to_string())),
            "mm" => out.push_str(&digits(format!("{:02}", at.minute))),
            "h" => out.push_str(&digits(twelve.to_string())),
            "hh" => out.push_str(&digits(format!("{twelve:02}"))),
            "K" => out.push_str(&digits((at.hour % 12).to_string())),
            "KK" => out.push_str(&digits(format!("{:02}", at.hour % 12))),
            "H" => out.push_str(&digits(at.hour.to_string())),
            "HH" => out.push_str(&digits(format!("{:02}", at.hour))),
            "k" => out.push_str(&digits(if at.hour == 0 { 24 } else { at.hour }.to_string())),
            "kk" => out.push_str(&digits(format!("{:02}", if at.hour == 0 { 24 } else { at.hour }))),
            _ => {}
        }
    }
    out.push_str(rest);
    out
}

/// The name of `month` (1–12) of the locale's calendar on its own (`{ month: "short" }`).
pub fn month_name(month: u32, width: Width) -> &'static str {
    let names = match width {
        Width::Short => t!("intl.date.months.short"),
        Width::Long => t!("intl.date.months.long"),
    };
    nth(names, month)
}

/// The name of `weekday` (0 for Sunday) on its own (`{ weekday: "short" }`).
pub fn weekday_name(weekday: u32, width: Width) -> &'static str {
    let names = match width {
        Width::Short => t!("intl.date.weekdays.short"),
        Width::Long => t!("intl.date.weekdays.long"),
    };
    nth(names, weekday % 7 + 1)
}

/// `yesterday`, `today` or `tomorrow` for -1, 0 and 1, as `{ numeric: "auto" }` names those days.
pub fn relative_day(days: i64) -> Option<&'static str> {
    match days {
        -1 => Some(t!("intl.relative.day.yesterday")),
        0 => Some(t!("intl.relative.day.today")),
        1 => Some(t!("intl.relative.day.tomorrow")),
        _ => None,
    }
}

/// `n` units from now, `3 days ago` or `in 3 days`, as `{ numeric: "always" }` writes it.
pub fn relative(n: i64, unit: Unit) -> String {
    let (past, future) = unit_keys(unit);
    let key = if n < 0 { past } else { future };
    let count = n.unsigned_abs();
    let template = I18N.plural_form(key, crate::intl::relative_category(count));
    crate::render(template, &[("count", &crate::integer(count as i64))])
}

/// Days since 1970-01-01 of a proleptic Gregorian date (Howard Hinnant's `days_from_civil`).
pub fn days_from_civil(year: i32, month: u32, day: u32) -> i64 {
    let (m, d) = (month as i64, day as i64);
    let y = year as i64 - i64::from(m <= 2);
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * (m + if m > 2 { -3 } else { 9 }) + 2) / 5 + d - 1;
    era * 146_097 + yoe * 365 + yoe / 4 - yoe / 100 + doy - 719_468
}

/// A style's pattern, and the month names and day periods it writes (when it writes them).
fn style_keys(style: Style) -> (Key, Option<Key>, Option<Key>) {
    match style {
        Style::Time => (key!("intl.date.time"), None, Some(key!("intl.date.time.dayPeriods"))),
        Style::MonthDay => (key!("intl.date.monthDay"), Some(key!("intl.date.monthDay.months")), None),
        Style::MonthDayYear => (key!("intl.date.monthDayYear"), Some(key!("intl.date.monthDayYear.months")), None),
        Style::DateTime => (
            key!("intl.date.dateTime"),
            Some(key!("intl.date.dateTime.months")),
            Some(key!("intl.date.dateTime.dayPeriods")),
        ),
        Style::MonthDayTime => (
            key!("intl.date.monthDayTime"),
            Some(key!("intl.date.monthDayTime.months")),
            Some(key!("intl.date.monthDayTime.dayPeriods")),
        ),
        Style::MonthDayYearTime => (
            key!("intl.date.monthDayYearTime"),
            Some(key!("intl.date.monthDayYearTime.months")),
            Some(key!("intl.date.monthDayYearTime.dayPeriods")),
        ),
        Style::MonthDayTimeZone => (
            key!("intl.date.monthDayTimeZone"),
            Some(key!("intl.date.monthDayTimeZone.months")),
            Some(key!("intl.date.monthDayTimeZone.dayPeriods")),
        ),
    }
}

fn unit_keys(unit: Unit) -> (PluralKey, PluralKey) {
    match unit {
        Unit::Second => (plural_key!("intl.relative.second.past"), plural_key!("intl.relative.second.future")),
        Unit::Minute => (plural_key!("intl.relative.minute.past"), plural_key!("intl.relative.minute.future")),
        Unit::Hour => (plural_key!("intl.relative.hour.past"), plural_key!("intl.relative.hour.future")),
        Unit::Day => (plural_key!("intl.relative.day.past"), plural_key!("intl.relative.day.future")),
        Unit::Week => (plural_key!("intl.relative.week.past"), plural_key!("intl.relative.week.future")),
        Unit::Month => (plural_key!("intl.relative.month.past"), plural_key!("intl.relative.month.future")),
        Unit::Year => (plural_key!("intl.relative.year.past"), plural_key!("intl.relative.year.future")),
    }
}

/// The `n`th (1-based) of a `|`-separated list.
fn nth(list: &'static str, n: u32) -> &'static str {
    list.split('|').nth(n.saturating_sub(1) as usize).unwrap_or("")
}

/// `text` with its ASCII digits in the locale's own (`۱۴۰۵`, `১২`).
fn digits(locale: usize, text: String) -> String {
    let local: Vec<char> = I18N.text_in(locale, key!("intl.date.digits")).chars().collect();
    if local.len() != 10 || local[0] == '0' {
        return text;
    }
    text.chars().map(|c| c.to_digit(10).map_or(c, |d| local[d as usize])).collect()
}

/// The `shortOffset` zone: `GMT+0`, `GMT-7`, `GMT+5:30`.
pub fn zone(offset: i32) -> String {
    zone_in(I18N.locale(), offset)
}

/// `zone` in the locale at index `locale`.
fn zone_in(locale: usize, offset: i32) -> String {
    let text = |key| I18N.text_in(locale, key);
    if offset == 0 {
        return text(key!("intl.date.zone.utc")).to_owned();
    }
    let sign = text(if offset < 0 { key!("intl.date.zone.minus") } else { key!("intl.date.zone.plus") });
    let (hours, minutes) = (offset.unsigned_abs() / 3600, offset.unsigned_abs() % 3600 / 60);
    let amount = if minutes == 0 {
        hours.to_string()
    } else {
        format!("{hours}{}{minutes:02}", text(key!("intl.date.zone.separator")))
    };
    let offset = format!("{sign}{}", digits(locale, amount));
    crate::render(text(key!("intl.date.zone")), &[("offset", &offset as &dyn std::fmt::Display)])
}

/// `at`'s year, month and day in the locale's calendar.
fn calendar_date(locale: usize, at: Moment) -> (i32, u32, u32) {
    match I18N.text_in(locale, key!("intl.date.calendar")) {
        "buddhist" => (at.year + 543, at.month, at.day),
        "persian" => persian(at.days()),
        _ => (at.year, at.month, at.day),
    }
}

/// ICU's arithmetic Persian calendar (`persncal.cpp`), from days since 1970-01-01.
fn persian(days: i64) -> (i32, u32, u32) {
    const MONTH_START: [i64; 12] = [0, 31, 62, 93, 124, 155, 186, 216, 246, 276, 306, 336];
    let since = days + 2_440_588 - 1_948_320;
    let year = 1 + (33 * since + 3).div_euclid(12_053);
    let farvardin1 = 365 * (year - 1) + (8 * year + 21).div_euclid(33);
    let day_of_year = since - farvardin1;
    let month = if day_of_year < 216 { day_of_year / 31 } else { (day_of_year - 6) / 30 };
    (year as i32, month as u32 + 1, (day_of_year - MONTH_START[month as usize] + 1) as u32)
}
