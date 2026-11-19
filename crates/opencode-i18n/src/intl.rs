//! Numbers, plural categories and lists as Desktop's `Intl.NumberFormat`, `Intl.PluralRules`
//! and `Intl.ListFormat` give them, from the symbols, rules and separators `sync.ts` reads out of
//! Desktop's own Electron. Chromium's ICU data is cut down, so some locales write these as en-US
//! or root does there, and so do these.

use crate::{PluralCategory, t};

/// `n` with the locale's digits, grouping and minus.
pub fn integer(n: i64) -> String {
    signed(n < 0, local(&group(&n.unsigned_abs().to_string())))
}

/// `n` rounded half away from zero to at most `max_fraction` fraction digits, trailing zeros
/// dropped, as `Intl.NumberFormat(locale, { maximumFractionDigits })` writes it.
pub fn decimal(n: f64, max_fraction: u8) -> String {
    fraction_digits(n, 0, max_fraction)
}

/// `decimal` with at least `min_fraction` fraction digits, zeros added, as
/// `Intl.NumberFormat(locale, { minimumFractionDigits, maximumFractionDigits })` writes it.
pub fn fraction_digits(n: f64, min_fraction: u8, max_fraction: u8) -> String {
    if !n.is_finite() {
        return n.to_string();
    }
    // The shortest digits that read back as `n`, as Intl rounds from.
    let shortest = n.abs().to_string();
    let (whole, fraction) = shortest.split_once('.').unwrap_or((&shortest, ""));
    let keep = fraction.len().min(max_fraction as usize);
    let mut digits: Vec<u8> = whole.bytes().chain(fraction.bytes().take(keep)).map(|b| b - b'0').collect();
    if fraction.as_bytes().get(keep).is_some_and(|&b| b >= b'5') {
        let carry = digits.iter_mut().rev().try_for_each(|d| {
            *d = (*d + 1) % 10;
            if *d == 0 { Ok(()) } else { Err(()) }
        });
        if carry.is_ok() {
            digits.insert(0, 1);
        }
    }
    let point = digits.len() - keep;
    let text: String = digits.iter().map(|d| char::from(b'0' + d)).collect();
    let (whole, fraction) = text.split_at(point);
    let trimmed = fraction.trim_end_matches('0');
    let fraction = format!("{trimmed:0<width$}", width = (min_fraction as usize).max(trimmed.len()));
    let body = if fraction.is_empty() {
        group(whole)
    } else {
        format!("{}{}{fraction}", group(whole), t!("intl.number.decimal"))
    };
    signed(n < 0.0, local(&body))
}

/// `items` as `Intl.ListFormat(locale, { style: "long", type: "conjunction" })` joins them.
pub fn list<S: AsRef<str>>(items: &[S]) -> String {
    let Some((last, rest)) = items.split_last() else { return String::new() };
    let last = last.as_ref();
    let Some((first, middle)) = rest.split_first() else { return last.to_owned() };
    let lower = last.to_lowercase();
    let hebrew = last.chars().next().is_some_and(|c| ('\u{5d0}'..='\u{5ea}').contains(&c));
    let i_sound = lower.starts_with('i') || (lower.starts_with("hi") && !lower[2..].starts_with(['a', 'e']));
    let (pair, end) = match () {
        _ if hebrew => (t!("intl.list.pair.hebrew"), t!("intl.list.end.hebrew")),
        _ if i_sound => (t!("intl.list.pair.i"), t!("intl.list.end.i")),
        _ => (t!("intl.list.pair"), t!("intl.list.end")),
    };
    let mut out = first.as_ref().to_owned();
    if middle.is_empty() {
        return out + pair + last;
    }
    for (at, item) in middle.iter().enumerate() {
        out.push_str(if at == 0 { t!("intl.list.start") } else { t!("intl.list.middle") });
        out.push_str(item.as_ref());
    }
    out + end + last
}

/// `n`'s CLDR category in the current locale, as `Intl.PluralRules(locale).select(n)`.
pub fn plural_category(n: u64) -> PluralCategory {
    category(t!("intl.plural"), n)
}

/// `n`'s category under the rules the locale's relative times use.
pub(crate) fn relative_category(n: u64) -> PluralCategory {
    category(t!("intl.relative.plural"), n)
}

/// A rule table's category for `n`: a letter for each of 0–199, then 1000000's; past 199 the
/// last two digits decide, but for whole millions.
fn category(table: &str, n: u64) -> PluralCategory {
    let at = match n {
        0..200 => n as usize,
        _ if n.is_multiple_of(1_000_000) => 200,
        _ => 100 + (n % 100) as usize,
    };
    match table.as_bytes().get(at) {
        Some(b'z') => PluralCategory::Zero,
        Some(b'o') => PluralCategory::One,
        Some(b't') => PluralCategory::Two,
        Some(b'f') => PluralCategory::Few,
        Some(b'm') => PluralCategory::Many,
        _ => PluralCategory::Other,
    }
}

/// ASCII digits split into the locale's groups (`1,234,567`, Indian `12,34,567`), unless there
/// are too few to group (Spanish leaves `1234`).
fn group(digits: &str) -> String {
    let (primary, secondary) = match t!("intl.number.grouping").split_once(',') {
        Some((p, s)) => (p.parse().unwrap_or(3), s.parse().unwrap_or(3)),
        None => {
            let p = t!("intl.number.grouping").parse().unwrap_or(3);
            (p, p)
        }
    };
    let min: usize = t!("intl.number.minGrouping").parse().unwrap_or(1);
    if digits.len() < primary + min {
        return digits.to_owned();
    }
    let separator = t!("intl.number.group");
    let (head, tail) = digits.split_at(digits.len() - primary);
    let mut groups = vec![tail];
    let mut rest = head;
    while rest.len() > secondary {
        let (before, last) = rest.split_at(rest.len() - secondary);
        groups.push(last);
        rest = before;
    }
    groups.push(rest);
    groups.reverse();
    groups.join(separator)
}

/// ASCII digits in the locale's own.
fn local(text: &str) -> String {
    let digits: Vec<char> = t!("intl.number.digits").chars().collect();
    if digits.len() != 10 || digits[0] == '0' {
        return text.to_owned();
    }
    text.chars().map(|c| c.to_digit(10).map_or(c, |d| digits[d as usize])).collect()
}

fn signed(negative: bool, body: String) -> String {
    if !negative {
        return body;
    }
    t!("intl.number.negative", value = body)
}
