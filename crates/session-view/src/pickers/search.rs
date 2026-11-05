//! The model list's search. Desktop's own (`providers/models/search.ts`) matches the query
//! against a model's name, its provider and its id, ignoring case and punctuation: `gpt5` finds
//! `GPT-5.5` as `gpt 5` does, and the picker lists what it finds by name.
//!
//! That plain substring still misses a slip of the fingers: `gemiin` finds nothing (t1389).
//! The letters of the query appearing in order count too, scored below every match Desktop
//! would make, so a typo reaches the model it meant without reordering the rest.

/// A field that starts with the query: Desktop's own best match.
const PREFIX: i64 = 20_000;
/// The query inside a field: any other match Desktop makes.
const CONTAINS: i64 = 10_000;
/// Letters in order, a slip of the fingers, stay under this, below every match Desktop makes.
const SCATTERED: i64 = 5_000;
/// Fields after the name (the provider, the id) rank a little lower than the name.
const FIELD: i64 = 100;

/// `normalizeModelSearch`: lowercased, with every run of punctuation or spaces as one space.
pub fn normalize(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    let mut gap = false;
    for c in value.to_lowercase().chars() {
        if c.is_alphanumeric() {
            if gap && !out.is_empty() {
                out.push(' ');
            }
            gap = false;
            out.push(c);
        } else {
            gap = true;
        }
    }
    out
}

/// How well `query` finds an entry with these fields (`name`, provider, id), best first;
/// `None` when it doesn't. Starts-with beats contains, and both beat letters in order.
pub fn score(query: &str, fields: &[&str]) -> Option<i64> {
    let needle = normalize(query);
    if needle.is_empty() {
        return Some(0);
    }
    // `compactModelSearch`: "gpt5" still finds "GPT-5.5".
    let tight: String = needle.replace(' ', "");
    let mut best: Option<i64> = None;
    for (n, field) in fields.iter().enumerate() {
        let near = -(n as i64) * FIELD;
        let text = normalize(field);
        let packed: String = text.replace(' ', "");
        let len = text.chars().count().min(200) as i64;
        let hit = if text.starts_with(&needle) || packed.starts_with(&tight) {
            Some(PREFIX + near + 200 - len)
        } else if let Some(at) = text.find(&needle).or_else(|| packed.find(&tight).filter(|_| !tight.is_empty())) {
            Some(CONTAINS + near + 200 - at as i64 * 2 - len)
        } else {
            scattered(&needle, &text).map(|s| (s + near).clamp(1, SCATTERED - 1))
        };
        best = best.max(hit);
    }
    best
}

/// Letters of `query` anywhere in `value`, in order: a run of them, and one at the start of a
/// word, count for more, as Rune's combobox scores them. The query's spaces are skipped, so
/// "gpt 5" still reaches "gpt55".
fn scattered(query: &str, value: &str) -> Option<i64> {
    let needle: Vec<char> = query.chars().filter(|c| !c.is_whitespace()).collect();
    let (mut at, mut score, mut run, mut word) = (0, 0, false, true);
    for c in value.chars() {
        if c == ' ' {
            run = false;
            word = true;
            continue;
        }
        if at < needle.len() && c == needle[at] {
            score += 10 + if run { 12 } else { 0 } + if word { 8 } else { 0 };
            run = true;
            at += 1;
        } else {
            run = false;
            score -= 1;
        }
        word = false;
    }
    (at == needle.len()).then_some(score)
}
