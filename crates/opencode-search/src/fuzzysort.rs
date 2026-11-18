//! fuzzysort 3.0.2, the matcher Desktop's lists filter with (`packages/ui/src/hooks/list-machine.ts:337-351`:
//! `fuzzysort.go(needle, items, { keys })`), ported line for line from `fuzzysort.js` so a query finds what
//! Desktop's finds and ranks it the same, ties included: results come out of the same binary heap
//! (`fastpriorityqueue`) Desktop's do. Strings are matched as JavaScript sees them, in UTF-16 units,
//! with Latin accents taken off first (`remove_accents`).

use std::borrow::Cow;

use icu_normalizer::DecomposingNormalizerBorrowed;
use icu_properties::CodePointMapData;
use icu_properties::props::Script;

const NEG: f64 = f64::NEG_INFINITY;

/// A target made ready (`prepare`): its lowercased units, which letters it holds, and where each
/// next word starts (`_nextBeginningIndexes`).
#[derive(Clone, Debug, Default)]
pub struct Target {
    lower: Vec<u16>,
    bitflags: u32,
    next: Vec<usize>,
    /// `!target`: an empty key, which matches nothing (`noTarget`).
    empty: bool,
}

impl Target {
    pub fn new(target: &str) -> Self {
        let info = lower_info(target);
        Target { lower: info.codes, bitflags: info.bitflags, next: next_beginnings(target), empty: target.is_empty() }
    }
}

/// `prepareSearch`: the trimmed query, and each distinct word of it when it holds a space.
struct Search {
    /// `_lower`.
    lower: Vec<u16>,
    /// `lowerCodes`.
    codes: Vec<u16>,
    contains_space: bool,
    bitflags: u32,
    spaces: Vec<Search>,
}

impl Search {
    fn new(search: &str) -> Self {
        let search = search.trim_matches(js_space);
        let info = lower_info(search);
        let mut spaces = Vec::new();
        if info.contains_space {
            let mut words: Vec<&str> = Vec::new();
            for word in search.split(js_space).filter(|w| !w.is_empty()) {
                if !words.contains(&word) {
                    words.push(word);
                }
            }
            spaces = words
                .into_iter()
                .map(|word| {
                    let i = lower_info(word);
                    // `_lower` is the word lowercased with its accents kept, as fuzzysort has it.
                    Search { lower: word.to_lowercase().encode_utf16().collect(), codes: i.codes, contains_space: false, bitflags: 0, spaces: Vec::new() }
                })
                .collect();
        }
        Search { lower: info.codes.clone(), codes: info.codes, contains_space: info.contains_space, bitflags: info.bitflags, spaces }
    }
}

/// `go(search, targets, { keys })`: which objects (each its keys' targets) the query finds, best first.
/// An empty query finds none (`noResults`); the lists don't filter then.
pub fn go(search: &str, objects: &[&[Target]]) -> Vec<usize> {
    if search.is_empty() {
        return Vec::new();
    }
    let s = Search::new(search);
    let mut q = Queue::default();
    'outer: for (i, keys) in objects.iter().enumerate() {
        let flags = keys.iter().filter(|t| !t.empty).fold(0, |f, t| f | t.bitflags);
        if s.bitflags & flags != s.bitflags {
            continue;
        }
        let mut best = vec![NEG; s.spaces.len()];
        let mut scores = Vec::with_capacity(keys.len());
        for t in keys.iter() {
            if t.empty {
                scores.push(NEG);
                continue;
            }
            let (hit, partial) = algorithm(&s, t, s.contains_space);
            let Some(hit) = hit else {
                scores.push(NEG);
                continue;
            };
            if s.contains_space {
                for (b, p) in best.iter_mut().zip(&partial) {
                    if *p > -1000.0 && *b > NEG {
                        let tmp = (*b + p) / 4.0;
                        if tmp > *b {
                            *b = tmp;
                        }
                    }
                    if *p > *b {
                        *b = *p;
                    }
                }
            }
            scores.push(hit.score);
        }
        let score = if s.contains_space {
            if best.contains(&NEG) {
                continue 'outer;
            }
            best.iter().sum()
        } else {
            if scores.iter().all(|s| *s == NEG) {
                continue;
            }
            let mut score = NEG;
            for &r in &scores {
                if r > -1000.0 && score > NEG {
                    let tmp = (score + r) / 4.0;
                    if tmp > score {
                        score = tmp;
                    }
                }
                if r > score {
                    score = r;
                }
            }
            score
        };
        q.add((score, i));
    }
    let mut out = vec![0; q.len];
    for slot in out.iter_mut().rev() {
        *slot = q.poll().map_or(0, |(_, i)| i);
    }
    out
}

struct Hit {
    score: f64,
    indexes: Vec<usize>,
}

/// `algorithm` with `allowSpaces` false: a query with a space goes word by word
/// (`algorithmSpaces`), with each word's share of the score when `partial`.
fn algorithm(s: &Search, t: &Target, partial: bool) -> (Option<Hit>, Vec<f64>) {
    if s.contains_space {
        return spaces(s, t, partial);
    }
    (strict(s, t, &t.next), Vec::new())
}

/// The next word start after `i`; past the end, the end (JavaScript's `undefined` there).
fn nx(next: &[usize], i: usize, end: usize) -> usize {
    next.get(i).copied().unwrap_or(end)
}

/// `algorithm`'s body: the query's letters in order, then a stricter pass that only takes runs and
/// word starts, then a substring, scored as fuzzysort scores them.
fn strict(s: &Search, t: &Target, next: &[usize]) -> Option<Hit> {
    let (q, h) = (&s.codes[..], &t.lower[..]);
    let (slen, tlen) = (q.len(), h.len());
    if slen == 0 || tlen == 0 {
        return None;
    }
    let mut simple = Vec::with_capacity(slen);
    let (mut si, mut ti) = (0, 0);
    loop {
        if q[si] == h[ti] {
            simple.push(ti);
            si += 1;
            if si == slen {
                break;
            }
        }
        ti += 1;
        if ti >= tlen {
            return None;
        }
    }

    let mut si = 0;
    let mut success = false;
    let mut matches: Vec<usize> = Vec::with_capacity(slen);
    let mut ti = if simple[0] == 0 { 0 } else { nx(next, simple[0] - 1, tlen) };
    let mut backtrack = 0;
    if ti != tlen {
        loop {
            if ti >= tlen {
                if si == 0 {
                    break;
                }
                backtrack += 1;
                if backtrack > 200 {
                    break;
                }
                si -= 1;
                let last = matches.pop().unwrap_or(0);
                ti = nx(next, last, tlen);
            } else if q[si] == h[ti] {
                matches.push(ti);
                si += 1;
                if si == slen {
                    success = true;
                    break;
                }
                ti += 1;
            } else {
                ti = nx(next, ti, tlen);
            }
        }
    }

    let mut sub = if slen <= 1 { None } else { index_of(h, &s.lower, simple[0]) };
    let is_sub = sub.is_some();
    let mut is_sub_begin = sub.is_some_and(|i| i == 0 || nx(next, i - 1, tlen) == i);
    if let Some(found) = sub
        && !is_sub_begin
    {
        let mut i = 0;
        while i < next.len() {
            if i > found && (0..slen).all(|k| h.get(i + k) == Some(&q[k])) {
                sub = Some(i);
                is_sub_begin = true;
                break;
            }
            i = next[i];
        }
    }

    let score_of = |m: &[usize]| {
        let mut score = 0.0;
        let mut groups = 0.0;
        for i in 1..slen {
            if m[i] as isize - m[i - 1] as isize != 1 {
                score -= m[i] as f64;
                groups += 1.0;
            }
        }
        let unmatched = m[slen - 1] as f64 - m[0] as f64 - (slen - 1) as f64;
        score -= (12.0 + unmatched) * groups;
        if m[0] != 0 {
            score -= (m[0] * m[0]) as f64 * 0.2;
        }
        if !success {
            score *= 1000.0;
        } else {
            let mut unique = 1;
            let mut i = nx(next, 0, tlen);
            while i < tlen {
                unique += 1;
                i = nx(next, i, tlen);
            }
            if unique > 24 {
                score *= ((unique - 24) * 10) as f64;
            }
        }
        let longer = (tlen as f64 - slen as f64) / 2.0;
        score -= longer;
        let bonus = 1.0 + (slen * slen) as f64;
        if is_sub {
            score /= bonus;
        }
        if is_sub_begin {
            score /= bonus;
        }
        score - longer
    };

    let best = match (success, sub) {
        (false, Some(at)) => (at..at + slen).collect(),
        (false, None) => simple,
        (true, Some(at)) if is_sub_begin => (at..at + slen).collect(),
        (true, _) => matches,
    };
    Some(Hit { score: score_of(&best), indexes: best })
}

/// `algorithmSpaces`: each word in turn, a run ending a word's match counting as a word start for
/// the next, docked when a word lands before the last; the whole query as one substring wins when
/// it scores better. `partial` lets some words miss and reports each word's share.
fn spaces(s: &Search, t: &Target, partial: bool) -> (Option<Hit>, Vec<f64>) {
    let n = s.spaces.len();
    let mut next = t.next.clone();
    let mut shares = vec![NEG; n];
    let (mut score, mut first, mut any) = (0.0, 0usize, false);
    let mut seen: Vec<usize> = Vec::new();
    for (i, word) in s.spaces.iter().enumerate() {
        let Some(hit) = strict(word, t, &next) else {
            if partial {
                continue;
            }
            return (None, shares);
        };
        any = true;
        if i != n - 1 && hit.indexes.windows(2).all(|w| w[1] as isize - w[0] as isize == 1) {
            let begin = hit.indexes[hit.indexes.len() - 1] + 1;
            let replace = nx(&next, begin - 1, t.lower.len());
            for k in (0..begin).rev() {
                if next.get(k) != Some(&replace) {
                    break;
                }
                next[k] = begin;
            }
        }
        score += hit.score / n as f64;
        shares[i] = hit.score / n as f64;
        if hit.indexes[0] < first {
            score -= (first - hit.indexes[0]) as f64 * 2.0;
        }
        first = hit.indexes[0];
        for j in hit.indexes {
            if !seen.contains(&j) {
                seen.push(j);
            }
        }
    }
    if partial && !any {
        return (None, shares);
    }
    if let Some(whole) = strict(s, t, &t.next)
        && whole.score > score
    {
        if partial {
            shares = vec![whole.score / n as f64; n];
        }
        return (Some(whole), shares);
    }
    (Some(Hit { score, indexes: seen }), shares)
}

/// `String.prototype.indexOf` over UTF-16 units.
fn index_of(h: &[u16], needle: &[u16], from: usize) -> Option<usize> {
    if needle.is_empty() {
        return Some(from.min(h.len()));
    }
    (from..=h.len().checked_sub(needle.len())?).find(|&i| h[i..].starts_with(needle))
}

struct LowerInfo {
    codes: Vec<u16>,
    bitflags: u32,
    contains_space: bool,
}

/// `prepareLowerInfo`: accents off, lowercased, and a bit for each letter, digits, other ASCII and
/// anything else it holds (a space sets none).
fn lower_info(s: &str) -> LowerInfo {
    let codes: Vec<u16> = remove_accents(s).to_lowercase().encode_utf16().collect();
    let (mut bitflags, mut contains_space) = (0u32, false);
    for &c in &codes {
        if c == 32 {
            contains_space = true;
            continue;
        }
        let bit = match c {
            97..=122 => c - 97,
            48..=57 => 26,
            0..=127 => 30,
            _ => 31,
        };
        bitflags |= 1 << bit;
    }
    LowerInfo { codes, bitflags, contains_space }
}

/// `prepareNextBeginningIndexes`: for each unit, where the next word starts (an uppercase letter
/// after a non-uppercase one, or a change between alphanumeric and not), or the end.
fn next_beginnings(target: &str) -> Vec<usize> {
    let units: Vec<u16> = remove_accents(target).encode_utf16().collect();
    let len = units.len();
    let (mut was_upper, mut was_alnum) = (false, false);
    let mut begins = Vec::new();
    for (i, &c) in units.iter().enumerate() {
        let upper = (65..=90).contains(&c);
        let alnum = upper || (97..=122).contains(&c) || (48..=57).contains(&c);
        if (upper && !was_upper) || !was_alnum || !alnum {
            begins.push(i);
        }
        was_upper = upper;
        was_alnum = alnum;
    }
    let mut next = Vec::with_capacity(len);
    let (mut last, mut at) = (begins.first().copied(), 0);
    for i in 0..len {
        match last {
            Some(l) if l > i => next.push(l),
            _ => {
                at += 1;
                last = begins.get(at).copied();
                next.push(last.unwrap_or(len));
            }
        }
    }
    next
}

/// `remove_accents`: Latin runs decomposed (NFD), then U+0300-U+036F dropped.
fn remove_accents(s: &str) -> Cow<'_, str> {
    if s.is_ascii() {
        return Cow::Borrowed(s);
    }
    let script = CodePointMapData::<Script>::new();
    let nfd = DecomposingNormalizerBorrowed::new_nfd();
    let (mut out, mut run) = (String::with_capacity(s.len()), String::new());
    for c in s.chars() {
        if script.get(c) == Script::Latin {
            run.push(c);
            continue;
        }
        if !run.is_empty() {
            out.push_str(&nfd.normalize(&run));
            run.clear();
        }
        out.push(c);
    }
    out.push_str(&nfd.normalize(&run));
    out.retain(|c| !('\u{300}'..='\u{36f}').contains(&c));
    Cow::Owned(out)
}

/// JavaScript's `\s` (and `trim`'s whitespace): Unicode spaces and the byte order mark.
fn js_space(c: char) -> bool {
    c.is_whitespace() || c == '\u{feff}'
}

/// fuzzysort's `fastpriorityqueue`: a min-heap on the score, whose order for equal scores is the
/// order Desktop's results take.
#[derive(Default)]
struct Queue {
    e: Vec<(f64, usize)>,
    len: usize,
}

impl Queue {
    fn add(&mut self, r: (f64, usize)) {
        let mut a = self.len;
        if a < self.e.len() {
            self.e[a] = r;
        } else {
            self.e.push(r);
        }
        self.len += 1;
        while a > 0 {
            let v = (a - 1) >> 1;
            if r.0 >= self.e[v].0 {
                break;
            }
            self.e[a] = self.e[v];
            a = v;
        }
        self.e[a] = r;
    }

    fn poll(&mut self) -> Option<(f64, usize)> {
        if self.len == 0 {
            return None;
        }
        let top = self.e[0];
        self.len -= 1;
        self.e[0] = self.e[self.len];
        let v = self.e[0];
        let (mut a, mut c) = (0, 1);
        while c < self.len {
            let s = c + 1;
            a = c;
            if s < self.len && self.e[s].0 < self.e[c].0 {
                a = s;
            }
            self.e[(a - 1) >> 1] = self.e[a];
            c = 1 + (a << 1);
        }
        while a > 0 {
            let f = (a - 1) >> 1;
            if v.0 >= self.e[f].0 {
                break;
            }
            self.e[a] = self.e[f];
            a = f;
        }
        self.e[a] = v;
        Some(top)
    }
}

/// `rankSessionTitle` (`composer/session-match.ts`): 0 for the whole title, 1 when the title starts
/// with the query, 2 when a later word does; each query word must start consecutive title words,
/// ignoring case, and a trailing space asks for one more word.
pub fn rank_session_title(title: &str, query: &str) -> Option<u8> {
    let lower = title.to_lowercase();
    let query = query.to_lowercase();
    if lower == query {
        return Some(0);
    }
    let words: Vec<&str> = query.split(' ').collect();
    let titles: Vec<&str> = lower.split(js_space).filter(|w| !w.is_empty()).collect();
    let start = (0..titles.len()).find(|&start| words.iter().enumerate().all(|(i, w)| titles.get(start + i).is_some_and(|t| t.starts_with(w))))?;
    Some(if start == 0 { 1 } else { 2 })
}
