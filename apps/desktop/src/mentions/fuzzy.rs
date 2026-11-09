//! The composer's matcher, for `@` mentions and `/` commands. A query finds a name when its
//! letters and digits appear in it in order, ignoring case, spaces and punctuation: `runedes`,
//! `rune des` and `RUNE-DESKTOP` all find "RUNE DESKTOP". A match ranks by its [`Tier`] (the
//! whole name, its start, a word's start, one run anywhere, a few pieces), then by quality:
//! fewer pieces, pieces that start words, a file's own name over its folders, earlier and shorter.

use std::cmp::Ordering;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Tier {
    /// The letters fall in a few pieces ("fla rec" in "Fix flaky reconnect").
    Scattered,
    /// One run inside a word ("connect" in "reconnect").
    Inside,
    /// One run from a word's start ("desk" in "RUNE DESKTOP").
    Word,
    /// The name starts with them ("runedes" for "RUNE DESKTOP").
    Prefix,
    /// The whole name.
    Exact,
}

/// How well a query found a name; greater is better.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Hit {
    pub tier: Tier,
    pub quality: i32,
}

impl Ord for Hit {
    fn cmp(&self, other: &Self) -> Ordering {
        self.tier.cmp(&other.tier).then(self.quality.cmp(&other.quality))
    }
}

impl PartialOrd for Hit {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// A name made ready to match many times: its letters and digits lowercased, which start a word
/// (after punctuation or a space, at a lower-to-upper case change, between letters and digits),
/// and where a path's last part begins.
#[derive(Clone, Debug, Default)]
pub struct Prepared {
    text: Box<[char]>,
    starts: Box<[bool]>,
    base: usize,
}

impl Prepared {
    pub fn new(name: &str) -> Self {
        let (mut text, mut starts, mut base) = (Vec::with_capacity(name.len()), Vec::with_capacity(name.len()), 0);
        let mut prev: Option<char> = None;
        for c in name.chars() {
            if c == '/' || c == '\\' {
                base = text.len();
            }
            if c.is_alphanumeric() {
                let start = prev.is_none_or(|p| !p.is_alphanumeric() || (p.is_lowercase() && c.is_uppercase()) || p.is_alphabetic() != c.is_alphabetic());
                for (i, l) in c.to_lowercase().enumerate() {
                    text.push(l);
                    starts.push(start && i == 0);
                }
            }
            prev = Some(c);
        }
        Prepared { text: text.into(), starts: starts.into(), base }
    }
}

/// A query made ready: its letters and digits, lowercased.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Needle(Box<[char]>);

impl Needle {
    pub fn new(query: &str) -> Self {
        Needle(query.chars().filter(|c| c.is_alphanumeric()).flat_map(char::to_lowercase).collect())
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Whatever this finds, `shorter` found too: typing on narrows the last key's matches.
    pub fn extends(&self, shorter: &Needle) -> bool {
        self.0.starts_with(&shorter.0)
    }
}

/// The needle's letters appear in `name` in order. What a longer needle finds, a shorter one
/// it extends finds too, so a list can narrow as the query grows.
pub fn contains(needle: &Needle, name: &Prepared) -> bool {
    let mut rest = name.text.iter();
    needle.0.iter().all(|c| rest.any(|x| x == c))
}

/// How well `needle` finds `name`; `None` when it doesn't. An empty needle finds everything.
pub fn score(needle: &Needle, name: &Prepared) -> Option<Hit> {
    let (n, h) = (&needle.0[..], &name.text[..]);
    if n.is_empty() {
        return Some(Hit { tier: Tier::Prefix, quality: 0 });
    }
    if !contains(needle, name) {
        return None;
    }
    let long = h.len().min(200) as i32;
    if h.len() == n.len() {
        return Some(Hit { tier: Tier::Exact, quality: 0 });
    }
    if h.starts_with(n) {
        return Some(Hit { tier: Tier::Prefix, quality: -long });
    }
    // The best single run: one from a word's start, then in the path's last part, then earliest.
    let mut run = None;
    for at in (1..=h.len() - n.len()).filter(|&at| h[at] == n[0] && h[at..].starts_with(n)) {
        let found = (name.starts[at], at >= name.base, std::cmp::Reverse(at));
        run = run.max(Some(found));
        if found.0 && found.1 {
            break;
        }
    }
    if let Some((word, own, std::cmp::Reverse(at))) = run {
        let quality = if own { 40 } else { 0 } - (at.min(100) as i32) - long / 4;
        return Some(Hit { tier: if word { Tier::Word } else { Tier::Inside }, quality });
    }
    scattered(n, name).map(|quality| Hit { tier: Tier::Scattered, quality: quality - long / 4 })
}

/// The needle in pieces, each the longest run left that fits after the last: a match only when
/// it takes a few pieces (two, or one for every three letters) and a one-letter piece starts a
/// word. Scattered single letters ("r…u…n…e" across a long path) are noise, not a match.
fn scattered(n: &[char], name: &Prepared) -> Option<i32> {
    let h = &name.text[..];
    let limit = 2.max(n.len().div_ceil(3));
    let (mut i, mut from, mut pieces, mut quality) = (0, 0, 0, 0i32);
    while i < n.len() {
        pieces += 1;
        if pieces > limit {
            return None;
        }
        // The longest run of the needle left, at a word's start when two are as long.
        let (mut len, mut at) = (0, 0);
        for x in from..h.len() {
            let run = h[x..].iter().zip(&n[i..]).take_while(|(a, b)| a == b).count();
            if run > len || (run == len && run > 0 && name.starts[x] && !name.starts[at]) {
                (len, at) = (run, x);
            }
        }
        if len == 0 {
            return None;
        }
        let start = name.starts[at];
        if len == 1 && !start {
            return None;
        }
        quality += if start { 20 } else { 0 } + if at >= name.base { 10 } else { 0 } - if i == 0 { at.min(50) as i32 } else { 0 };
        i += len;
        from = at + len;
    }
    Some(quality - pieces as i32 * 30)
}

#[cfg(test)]
#[path = "fuzzy_tests.rs"]
mod tests;
