//! Language names in the current locale, as Desktop's `Intl.DisplayNames(locale, { type:
//! "language" })` gives them (`en-US` is "American English", "Englisch (Vereinigte Staaten)",
//! "англійська (Сполучені Штати)"), rebuilt from the parts `sync.ts` read out of Chromium:
//! a whole name where it has one, else the language's name with its script and region in the
//! locale's pattern, a part Chromium doesn't name keeping its code (`English (029)`).

use crate::t;

/// `tag`'s name, as `Intl.DisplayNames(locale, { type: "language" }).of(tag)`.
pub fn language_name(tag: &str) -> String {
    if let Some(name) = find(t!("intl.names.dialects"), tag) {
        return name.to_owned();
    }
    let mut parts = tag.split('-');
    let language = parts.next().unwrap_or(tag);
    // A language Chromium doesn't name leaves the whole tag, as its `fallback: "code"` does.
    let Some(base) = find(t!("intl.names.languages"), language) else { return tag.to_owned() };
    let subs: Vec<&str> = parts
        .map(|part| {
            let table = if is_script(part) { t!("intl.names.scripts") } else { t!("intl.names.regions") };
            find(table, part).unwrap_or(part)
        })
        .collect();
    let pattern = match subs.len() {
        0 => return base.to_owned(),
        1 => t!("intl.names.pattern"),
        2 => t!("intl.names.patternScript"),
        _ => return tag.to_owned(),
    };
    std::iter::once(base).chain(subs).enumerate().fold(pattern.to_owned(), |text, (i, part)| text.replace(&format!("{{{{{i}}}}}"), part))
}

/// The name of `code` in a `code<tab>name` table, one a line.
fn find(table: &'static str, code: &str) -> Option<&'static str> {
    table.lines().find_map(|line| line.split_once('\t').filter(|(c, _)| *c == code).map(|(_, name)| name))
}

fn is_script(part: &str) -> bool {
    part.len() == 4 && part.chars().next().is_some_and(|c| c.is_ascii_uppercase()) && part.chars().skip(1).all(|c| c.is_ascii_lowercase())
}
