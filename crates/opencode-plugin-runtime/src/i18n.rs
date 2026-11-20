//! `I18n`: a plugin's own catalogs (`locales/<id>.json`) in the interface language, and the
//! app's number, date and list formats. Every read calls `on_read_locale`, so a host can record
//! the language dependency for surface redraws.

use std::cell::RefCell;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::rc::Rc;

use mlua::{Lua, LuaString, Table, Value};
use opencode_i18n::PluralCategory;
use opencode_plugin_spec as spec;

use crate::catalogs::Catalogs;
use crate::luau::{method, record, snapshot};

/// Desktop's right-to-left languages (`RTL_LOCALES`).
const RTL: &[&str] = &["ar", "he", "ur", "pa", "fa", "dv"];

/// Populates the `I18n` service methods on `t` (`T`, `Has`, `Locale`, `Direction`, `Plural`,
/// `Number`, `Date`, `RelativeTime`, `RelativeDay`, `List`). The caller attaches the lazy
/// `Changed` signal separately.
pub fn build(lua: &Lua, t: &Table, catalogs: Rc<RefCell<Catalogs>>, is_window: bool, on_read_locale: fn()) -> mlua::Result<()> {
    let texts: Rc<RefCell<Texts>> = Rc::default();
    let cats = catalogs.clone();
    let cache = texts.clone();
    method(lua, t, "T", move |lua, (key, args): (LuaString, Option<Table>)| {
        on_read_locale();
        let key = key.to_str()?;
        if args.is_some() {
            return lua.create_string(translate(&cats.borrow(), &key, args.as_ref()));
        }
        cache.borrow_mut().get_or(lua, |k| k.extend(b"T".iter().chain(key.as_bytes())), || translate(&cats.borrow(), &key, None))
    })?;
    let cats = catalogs;
    method(lua, t, "Has", move |_, key: String| {
        on_read_locale();
        let catalogs = cats.borrow();
        Ok([opencode_i18n::locale().id(), "en"].iter().any(|l| catalogs.get(l, &key).is_some()))
    })?;
    method(lua, t, "Locale", move |lua, ()| {
        on_read_locale();
        locale_value(lua)
    })?;
    method(lua, t, "Direction", move |_, ()| {
        on_read_locale();
        Ok(direction().as_str())
    })?;
    method(lua, t, "Plural", move |_, count: f64| {
        on_read_locale();
        Ok(category(count))
    })?;
    let cache = texts.clone();
    method(lua, t, "Number", move |lua, (value, format): (f64, Option<Table>)| {
        on_read_locale();
        let format = match &format {
            Some(t) => number_format(t)?,
            None => spec::NumberFormat::default(),
        };
        cache.borrow_mut().get_or(
            lua,
            |k| {
                k.push(b'N');
                k.extend(value.to_bits().to_le_bytes());
                k.extend([
                    format.style.map_or(0, |s| s as u8 + 1),
                    format.notation.map_or(0, |n| n as u8 + 1),
                    u8::from(format.locale_pattern == Some(true)),
                ]);
                k.extend(format.max_fraction.unwrap_or(-1).to_le_bytes());
                k.extend(format.currency.as_deref().unwrap_or_default().as_bytes());
            },
            || number(value, &format),
        )
    })?;
    let cache = texts;
    method(lua, t, "Date", move |lua, (ms, style): (f64, LuaString)| {
        on_read_locale();
        let style = style.to_str()?;
        let style = spec::DateStyle::parse(&style).ok_or_else(|| mlua::Error::runtime(format!("unknown DateStyle {:?}", &*style)))?;
        cache.borrow_mut().get_or(
            lua,
            |k| {
                k.extend([b'D', style as u8]);
                k.extend(ms.to_bits().to_le_bytes());
            },
            || date(ms as i64, style),
        )
    })?;
    if is_window {
        remembered(lua, t)?;
    }
    method(lua, t, "RelativeTime", move |_, (value, unit): (f64, String)| {
        on_read_locale();
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
    method(lua, t, "RelativeDay", move |_, days: f64| {
        on_read_locale();
        Ok(opencode_i18n::date::relative_day(days.round() as i64))
    })?;
    method(lua, t, "List", move |_, items: Vec<String>| {
        on_read_locale();
        Ok(opencode_i18n::list(&items))
    })
}

/// `T` without arguments, `Number` (plain, or with one `style` or `notation`) and `Date`
/// answered from tables in the VM: a render asks for the same few strings every time it runs,
/// and a table read costs far less than a call into the host. [`clear_vm`] empties them when
/// the interface language changes.
const REMEMBERED: &str = r#"
local hostT, hostN, hostD = ...
local texts, plain, numbers, dates, made = {}, {}, {}, {}, 0
local function made1()
    made += 1
    if made > 4096 then
        texts, plain, numbers, dates, made = {}, {}, {}, {}, 0
    end
end
local out = {}
function out.T(self, key, args)
    if args ~= nil or type(key) ~= "string" then
        return hostT(self, key, args)
    end
    local v = texts[key]
    if v == nil then
        v = hostT(self, key)
        made1()
        texts[key] = v
    end
    return v
end
function out.Number(self, value, format)
    if type(value) ~= "number" or value ~= value then
        return hostN(self, value, format)
    end
    local bucket = plain
    if format ~= nil then
        local style, notation = format.style, format.notation
        local by = if notation == nil then style elseif style == nil then notation else nil
        if by == nil or format.currency ~= nil or format.maxFraction ~= nil or format.localePattern ~= nil then
            return hostN(self, value, format)
        end
        bucket = numbers[by]
        if bucket == nil then
            bucket = {}
            numbers[by] = bucket
        end
    end
    local v = bucket[value]
    if v == nil then
        v = hostN(self, value, format)
        made1()
        bucket[value] = v
    end
    return v
end
function out.Date(self, ms, style)
    if type(ms) ~= "number" or ms ~= ms or type(style) ~= "string" then
        return hostD(self, ms, style)
    end
    local bucket = dates[style]
    if bucket == nil then
        bucket = {}
        dates[style] = bucket
    end
    local v = bucket[ms]
    if v == nil then
        v = hostD(self, ms, style)
        made1()
        bucket[ms] = v
    end
    return v
end
function out.clear()
    texts, plain, numbers, dates, made = {}, {}, {}, {}, 0
end
return out
"#;

/// Puts the remembering `T`, `Number` and `Date` in front of the host's on service `t`.
fn remembered(lua: &Lua, t: &Table) -> mlua::Result<()> {
    let host = (t.raw_get::<mlua::Function>("T")?, t.raw_get::<mlua::Function>("Number")?, t.raw_get::<mlua::Function>("Date")?);
    let out: Table = lua.load(REMEMBERED).set_name("=I18n").call(host)?;
    for name in ["T", "Number", "Date"] {
        t.raw_set(name, out.raw_get::<mlua::Function>(name)?)?;
    }
    lua.set_named_registry_value("opencode.i18n.clear", out.raw_get::<mlua::Function>("clear")?)
}

/// Clears the remembered `I18n` strings cached inside `script`'s Lua VM after a locale change.
pub fn clear_vm(script: &rune_luau::Script) {
    script.with_lua(|lua| {
        if let Ok(Some(clear)) = lua.named_registry_value::<Option<mlua::Function>>("opencode.i18n.clear") {
            clear.call::<()>(()).ok();
        }
    });
}

/// The VM's strings `T` (without arguments), `Number` and `Date` made in the current language,
/// by what they were made from: a surface that rebuilds asks for the same few again and again.
#[derive(Default)]
struct Texts {
    locale: String,
    made: HashMap<Box<[u8]>, LuaString>,
    key: Vec<u8>,
}

impl Texts {
    /// The string `make` gives for the key `key` writes, made once per language.
    fn get_or(&mut self, lua: &Lua, key: impl FnOnce(&mut Vec<u8>), make: impl FnOnce() -> String) -> mlua::Result<LuaString> {
        let locale = opencode_i18n::locale().id();
        if self.locale != locale || self.made.len() >= 4096 {
            self.made.clear();
            locale.clone_into(&mut self.locale);
        }
        self.key.clear();
        key(&mut self.key);
        if let Some(s) = self.made.get(self.key.as_slice()) {
            return Ok(s.clone());
        }
        let s = lua.create_string(make())?;
        self.made.insert(self.key.as_slice().into(), s.clone());
        Ok(s)
    }
}

/// A `NumberFormat` off its table, read field by field.
fn number_format(t: &Table) -> mlua::Result<spec::NumberFormat> {
    let pick = |field: &str| -> mlua::Result<Option<String>> { t.raw_get::<Option<String>>(field) };
    let bad = |what: &str, v: &str| mlua::Error::runtime(format!("NumberFormat: unknown {what} {v:?}"));
    let mut f = spec::NumberFormat::default();
    f.style = pick("style")?.map(|s| spec::NumberStyle::parse(&s).ok_or_else(|| bad("style", &s))).transpose()?;
    f.notation = pick("notation")?.map(|s| spec::NumberNotation::parse(&s).ok_or_else(|| bad("notation", &s))).transpose()?;
    f.currency = pick("currency")?;
    f.max_fraction = t.raw_get::<Option<f64>>("maxFraction")?.map(|n| n as i64);
    f.locale_pattern = t.raw_get::<Option<bool>>("localePattern")?;
    Ok(f)
}

/// The interface language as a `LocaleInfo`: built once per language, then copied.
pub fn locale_value(lua: &Lua) -> mlua::Result<Table> {
    let l = opencode_i18n::locale();
    let mut h = std::hash::DefaultHasher::new();
    l.id().hash(&mut h);
    let made = snapshot(lua, "locale", h.finish(), || {
        record(lua, 4, |info| {
            info.str(c"id", l.id());
            info.str(c"tag", l.tag());
            info.str(c"name", l.label());
            info.str(c"direction", direction().as_str());
        })
        .map(Value::Table)
    })?;
    match made {
        Value::Table(t) => Ok(t),
        _ => lua.create_table(),
    }
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

/// Message `key` of the catalogs in the current language, else English, else the app's own
/// messages (a plugin's `ctx.t` reaches the UI catalog, as Desktop's does), else the key, with
/// `{{name}}`s filled from `args` and a `count` picking the plural form.
fn translate(catalogs: &Catalogs, key: &str, args: Option<&Table>) -> String {
    let lang = opencode_i18n::locale().id();
    let count = args.and_then(|a| a.raw_get::<Option<f64>>("count").ok().flatten());
    // In a right-to-left language a message the plugin has only in English reads as an English unit,
    // its full stop and numbers in order, as the app's own untranslated messages do (rune-i18n).
    let rtl = direction() == spec::TextDirection::Rtl;
    let english = |t: std::borrow::Cow<'static, str>| if rtl { format!("\u{2066}{t}\u{2069}") } else { t.into_owned() };
    let own = |k: &str| catalogs.get(lang, k).map(|s| s.into_owned()).or_else(|| catalogs.get("en", k).map(english));
    let host_plural = |n: f64| opencode_i18n::lookup_plural(key, n.abs().round() as u64).map(str::to_owned);
    let text = count
        .and_then(|n| {
            own(&format!("{key}.{}", category(n)))
                .or_else(|| own(&format!("{key}.other")))
                .or_else(|| host_plural(n))
        })
        .or_else(|| own(key))
        .or_else(|| opencode_i18n::lookup(key).map(str::to_owned))
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
                // `mlua::Integer` is `i32` on wasm32 and `i64` on 64-bit targets.
                #[allow(clippy::unnecessary_cast)]
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
        (spec::NumberStyle::Currency, _) => currency(
            value,
            format.currency.as_deref().unwrap_or("USD"),
            format.max_fraction.unwrap_or(2) as u8,
            format.locale_pattern == Some(true),
        ),
        (spec::NumberStyle::Decimal, _) => opencode_i18n::decimal(value, format.max_fraction.unwrap_or(3).clamp(0, 20) as u8),
    }
}

/// `Intl.NumberFormat(locale, { notation: "compact", maximumFractionDigits: 0 })`, lowercased:
/// `999`, `1k` (from 999.5), `833k`, `999k`, `1m` (from 999,500), `2500t`. The unit is the
/// value's own thousands, moving up one when the rounding reaches 1,000 of it, as ICU does.
fn compact(v: f64) -> String {
    const UNITS: [&str; 5] = ["", "k", "m", "b", "t"];
    let sign = if v < 0.0 { "-" } else { "" };
    let v = v.abs();
    let mut k = (1..UNITS.len()).take_while(|&k| v >= 1000f64.powi(k as i32)).last().unwrap_or(0);
    let mut r = scaled(v, -3 * k as i32);
    if r >= 1000 && k + 1 < UNITS.len() {
        k += 1;
        r = scaled(v, -3 * k as i32);
    }
    // Compact notation groups only from five digits on (CLDR's minimum grouping digits of 2).
    let digits = if r >= 10_000 { opencode_i18n::integer(r as i64) } else { r.to_string() };
    format!("{sign}{digits}{}", UNITS[k])
}

/// `$1,234.50`, with the language's separators (`$0.01` from 0.005, `$10.00` from 9.995). When
/// `locale_pattern` is requested for USD, uses the interface language's localized currency pattern
/// (`$1,234.50`, `1.234,50 $`, `1 234,50 $US`).
fn currency(v: f64, code: &str, fraction: u8, locale_pattern: bool) -> String {
    if locale_pattern && code == "USD" {
        return opencode_i18n::t!("intl.currency.usd", value = opencode_i18n::fraction_digits(v, fraction, fraction));
    }
    let symbol = match code {
        "USD" => "$",
        "EUR" => "€",
        "GBP" => "£",
        "JPY" => "¥",
        other => other,
    };
    let sign = if v < 0.0 { "-" } else { "" };
    let scale = 10u128.pow(u32::from(fraction));
    let units = scaled(v.abs(), i32::from(fraction));
    let digits = opencode_i18n::integer((units / scale) as i64);
    if fraction == 0 {
        return format!("{sign}{symbol}{digits}");
    }
    let point = opencode_i18n::decimal(0.5, 1).chars().nth(1).unwrap_or('.');
    format!("{sign}{symbol}{digits}{point}{:0width$}", units % scale, width = usize::from(fraction))
}

/// `v` (not negative) in units of 10^-`fraction`, rounded half away from zero on its shortest
/// decimal form, as `Intl.NumberFormat` rounds a double: 9.995 at 2 is 1000, 999,500 at -3 is
/// 1000.
fn scaled(v: f64, fraction: i32) -> u128 {
    if !v.is_finite() {
        return 0;
    }
    // Rust prints a double's shortest round-trip digits, never in exponent form.
    let text = format!("{v}");
    let (int, frac) = text.split_once('.').unwrap_or((&text, ""));
    let digits: Vec<u8> = int.bytes().chain(frac.bytes()).map(|b| b - b'0').collect();
    let keep = int.len() as i32 + fraction;
    if keep < 0 {
        return 0;
    }
    let keep = keep as usize;
    let kept = (0..keep).fold(0u128, |n, i| n.saturating_mul(10).saturating_add(u128::from(digits.get(i).copied().unwrap_or(0))));
    kept.saturating_add(u128::from(digits.get(keep).is_some_and(|d| *d >= 5)))
}

fn date(ms: i64, style: spec::DateStyle) -> String {
    use opencode_i18n::date::Style as S;
    let local = rune::LocalTime::at(ms);
    if style == spec::DateStyle::TimeZone {
        return opencode_i18n::date::zone(local.offset);
    }
    let style = match style {
        spec::DateStyle::Time => S::Time,
        spec::DateStyle::MonthDay => S::MonthDay,
        spec::DateStyle::MonthDayYear => S::MonthDayYear,
        spec::DateStyle::DateTime => S::DateTime,
        spec::DateStyle::MonthDayTime => S::MonthDayTime,
        spec::DateStyle::MonthDayYearTime => S::MonthDayYearTime,
        spec::DateStyle::MonthDayTimeZone => S::MonthDayTimeZone,
        spec::DateStyle::TimeZone => unreachable!(),
    };
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
