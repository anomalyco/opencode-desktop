//! `I18n`: a plugin's own catalogs (`locales/<id>.json`) in the interface language, and the
//! app's number, date and list formats. Every read records the language, so a surface redraws
//! when it changes.

use std::rc::Rc;

use mlua::{Lua, Table, Value};
use opencode_i18n::PluralCategory;
use opencode_plugin_spec as spec;
use rune_luau::Data;

use super::{lazy_signals, method};
use crate::plugin_host::luau::cx::{self, Dep};
use crate::plugin_host::luau::vm::Vmx;

/// Desktop's right-to-left languages (`RTL_LOCALES`).
const RTL: &[&str] = &["ar", "he", "ur", "pa", "fa", "dv"];

pub fn build(lua: &Lua, vmx: &Rc<Vmx>, t: &Table) -> mlua::Result<()> {
    let half = vmx.half.clone();
    method(lua, t, "T", move |_, (key, args): (String, Option<Table>)| {
        cx::read(Dep::Locale);
        Ok(translate(&half.catalogs.borrow(), &key, args.as_ref()))
    })?;
    let half = vmx.half.clone();
    method(lua, t, "Has", move |_, key: String| {
        cx::read(Dep::Locale);
        let catalogs = half.catalogs.borrow();
        Ok([opencode_i18n::locale().id(), "en"].iter().any(|l| catalogs.get(*l).is_some_and(|c| c.contains_key(&key))))
    })?;
    method(lua, t, "Locale", |lua, ()| {
        cx::read(Dep::Locale);
        locale_value(lua)
    })?;
    method(lua, t, "Direction", |_, ()| {
        cx::read(Dep::Locale);
        Ok(direction().as_str())
    })?;
    method(lua, t, "Plural", |_, count: f64| {
        cx::read(Dep::Locale);
        Ok(category(count))
    })?;
    method(lua, t, "Number", |lua, (value, format): (f64, Value)| {
        cx::read(Dep::Locale);
        let format: spec::NumberFormat = if format.is_nil() { Default::default() } else { Data::from_lua_value(lua, format)?.decode().map_err(|e| mlua::Error::runtime(format!("NumberFormat: {e}")))? };
        Ok(number(value, &format))
    })?;
    method(lua, t, "Date", |_, (ms, style): (f64, String)| {
        cx::read(Dep::Locale);
        let style = spec::DateStyle::parse(&style).ok_or_else(|| mlua::Error::runtime(format!("unknown DateStyle {style:?}")))?;
        Ok(date(ms as i64, style))
    })?;
    method(lua, t, "RelativeTime", |_, (value, unit): (f64, String)| {
        cx::read(Dep::Locale);
        use opencode_i18n::date::Unit as U;
        let unit = match spec::TimeUnit::parse(&unit).ok_or_else(|| mlua::Error::runtime(format!("unknown TimeUnit {unit:?}")))? {
            spec::TimeUnit::Second => U::Second,
            spec::TimeUnit::Minute => U::Minute,
            spec::TimeUnit::Hour => U::Hour,
            spec::TimeUnit::Day => U::Day,
            spec::TimeUnit::Week => U::Week,
            spec::TimeUnit::Month => U::Month,
            spec::TimeUnit::Year => U::Year,
        };
        Ok(opencode_i18n::date::relative(value.round() as i64, unit))
    })?;
    method(lua, t, "RelativeDay", |_, days: f64| {
        cx::read(Dep::Locale);
        Ok(opencode_i18n::date::relative_day(days.round() as i64))
    })?;
    method(lua, t, "List", |_, items: Vec<String>| {
        cx::read(Dep::Locale);
        Ok(opencode_i18n::list(&items))
    })?;
    lazy_signals(lua, vmx, t, "I18n", &["Changed"])
}

trait FromLuaValue {
    fn from_lua_value(lua: &Lua, v: Value) -> mlua::Result<Data>;
}

impl FromLuaValue for Data {
    fn from_lua_value(lua: &Lua, v: Value) -> mlua::Result<Data> {
        <Data as mlua::FromLua>::from_lua(v, lua)
    }
}

/// The interface language as a `LocaleInfo`.
pub fn locale_value(lua: &Lua) -> mlua::Result<Table> {
    let l = opencode_i18n::locale();
    let t = lua.create_table()?;
    t.raw_set("id", l.id())?;
    t.raw_set("tag", l.tag())?;
    t.raw_set("name", l.label())?;
    t.raw_set("direction", direction().as_str())?;
    Ok(t)
}

pub fn direction() -> spec::TextDirection {
    let l = opencode_i18n::locale();
    if l.rtl() || RTL.contains(&l.id()) { spec::TextDirection::Rtl } else { spec::TextDirection::Ltr }
}

fn category(count: f64) -> &'static str {
    match opencode_i18n::plural_category(count.abs().round() as u64) {
        PluralCategory::Zero => "zero",
        PluralCategory::One => "one",
        PluralCategory::Two => "two",
        PluralCategory::Few => "few",
        PluralCategory::Many => "many",
        _ => "other",
    }
}

/// Message `key` of the catalogs in the current language, else English, else the key, with
/// `{{name}}`s filled from `args` and a `count` picking the plural form.
fn translate(catalogs: &std::collections::BTreeMap<String, std::collections::BTreeMap<String, String>>, key: &str, args: Option<&Table>) -> String {
    let lang = opencode_i18n::locale().id();
    let count = args.and_then(|a| a.raw_get::<Option<f64>>("count").ok().flatten());
    let lookup = |k: &str| catalogs.get(lang).and_then(|c| c.get(k)).or_else(|| catalogs.get("en").and_then(|c| c.get(k))).cloned();
    let text = count
        .and_then(|n| lookup(&format!("{key}.{}", category(n))).or_else(|| lookup(&format!("{key}.other"))))
        .or_else(|| lookup(key))
        .unwrap_or_else(|| key.to_owned());
    fill(&text, args)
}

/// `{{ name }}` placeholders filled from `args`; a missing one reads empty.
fn fill(text: &str, args: Option<&Table>) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(open) = rest.find("{{") {
        out.push_str(&rest[..open]);
        let Some(close) = rest[open..].find("}}") else {
            out.push_str(&rest[open..]);
            return out;
        };
        let name = rest[open + 2..open + close].trim();
        if let Some(v) = args.and_then(|a| a.raw_get::<Value>(name).ok()) {
            match v {
                Value::Integer(i) => out.push_str(&opencode_i18n::integer(i as i64)),
                Value::Number(n) if n.fract() == 0.0 => out.push_str(&opencode_i18n::integer(n as i64)),
                Value::Nil => {}
                other => out.push_str(&other.to_string().unwrap_or_default()),
            }
        }
        rest = &rest[open + close + 2..];
    }
    out.push_str(rest);
    out
}

/// `value` in `format`: the app's grouping and digits for decimals and percents; currency and
/// compact as English writes them, with the language's separators.
pub fn number(value: f64, format: &spec::NumberFormat) -> String {
    let style = format.style.unwrap_or_default();
    let notation = format.notation.unwrap_or_default();
    match (style, notation) {
        (_, spec::NumberNotation::Compact) => compact(value),
        (spec::NumberStyle::Percent, _) => opencode_i18n::percent(value),
        (spec::NumberStyle::Currency, _) => currency(value, format.currency.as_deref().unwrap_or("USD"), format.max_fraction.unwrap_or(2) as u8),
        (spec::NumberStyle::Decimal, _) => opencode_i18n::decimal(value, format.max_fraction.unwrap_or(3).clamp(0, 20) as u8),
    }
}

/// `Intl.NumberFormat(locale, { notation: "compact", maximumFractionDigits: 0 })`, lowercased:
/// `950`, `1k`, `2k`, `1m` (999,999 rounds up to 1m).
fn compact(v: f64) -> String {
    const UNITS: [(f64, &str); 4] = [(1e12, "t"), (1e9, "b"), (1e6, "m"), (1e3, "k")];
    let sign = if v < 0.0 { "-" } else { "" };
    let v = v.abs();
    for (k, (scale, unit)) in UNITS.iter().enumerate() {
        let r = (v / scale).round();
        if r >= 1.0 {
            if r >= 1000.0 && k > 0 {
                let (up, upper) = UNITS[k - 1];
                return format!("{sign}{}{upper}", opencode_i18n::integer((v / up).round() as i64));
            }
            return format!("{sign}{}{unit}", opencode_i18n::integer(r as i64));
        }
    }
    format!("{sign}{}", opencode_i18n::integer(v.round() as i64))
}

/// `$1,234.50`, with the language's separators.
fn currency(v: f64, code: &str, fraction: u8) -> String {
    let symbol = match code {
        "USD" => "$",
        "EUR" => "€",
        "GBP" => "£",
        "JPY" => "¥",
        other => other,
    };
    let scale = 10f64.powi(i32::from(fraction));
    let cents = (v.abs() * scale).round();
    let sign = if v < 0.0 && cents > 0.0 { "-" } else { "" };
    let whole = (cents / scale).trunc();
    let digits = opencode_i18n::integer(whole as i64);
    if fraction == 0 {
        return format!("{sign}{symbol}{digits}");
    }
    let frac = (cents - whole * scale) as u64;
    let point = opencode_i18n::decimal(0.5, 1).chars().nth(1).unwrap_or('.');
    format!("{sign}{symbol}{digits}{point}{frac:0width$}", width = usize::from(fraction))
}

fn date(ms: i64, style: spec::DateStyle) -> String {
    use opencode_i18n::date::Style as S;
    let style = match style {
        spec::DateStyle::Time => S::Time,
        spec::DateStyle::MonthDay => S::MonthDay,
        spec::DateStyle::MonthDayYear => S::MonthDayYear,
        spec::DateStyle::DateTime => S::DateTime,
        spec::DateStyle::MonthDayTime => S::MonthDayTime,
        spec::DateStyle::MonthDayYearTime => S::MonthDayYearTime,
        spec::DateStyle::MonthDayTimeZone => S::MonthDayTimeZone,
    };
    let local = rune::LocalTime::at(ms);
    let secs = ms.div_euclid(1000) + i64::from(local.offset);
    let (days, rem) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    let (year, month, day) = civil(days);
    let at = opencode_i18n::date::Moment { year, month, day, hour: (rem / 3600) as u32, minute: (rem % 3600 / 60) as u32, offset: local.offset };
    opencode_i18n::date::format(style, at)
}

fn civil(days: i64) -> (i32, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    ((yoe + era * 400 + i64::from(m <= 2)) as i32, m as u32, d as u32)
}
