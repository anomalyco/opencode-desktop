//! In-memory omnibox search index & frecency ranking matching upstream
//! `packages/gui-plugins/src/browser/history/{match.ts,search.ts}`.

use std::collections::HashMap;

use super::page::{FoundPage, PageVisit, bare, origin_of};

pub const MISS: u8 = 0;
pub const INSIDE: u8 = 1;
pub const START: u8 = 2;

const MARK: char = '\u{0001}';
const DAY_MS: f64 = 86_400_000.0;

#[derive(Clone, Debug)]
struct Term {
    inside: String,
    start: Option<String>,
}

/// Parsed user omnibox query (`parseQuery`).
#[derive(Clone, Debug)]
pub struct Query {
    pub words: Vec<String>,
    terms: Vec<Term>,
    prefix: Option<String>,
}

pub fn parse_query(text: &str) -> Option<Query> {
    let words: Vec<String> = text
        .trim()
        .to_lowercase()
        .split_whitespace()
        .map(|w| bare(w).to_owned())
        .filter(|w| !w.is_empty())
        .collect();
    if words.is_empty() {
        return None;
    }
    let prefix = (words.len() == 1).then(|| mark_words(&words[0]));
    let terms = words
        .iter()
        .map(|w| {
            let marked = mark_words(w);
            if let Some(inside) = marked.strip_prefix(MARK) {
                Term { inside: inside.to_owned(), start: Some(marked) }
            } else {
                Term { inside: marked, start: None }
            }
        })
        .collect();
    Some(Query { words, terms, prefix })
}

/// Inserts `\x01` before every word start (letter/digit following non-word char) so word-prefix
/// matches are a single substring scan.
pub fn search_text(url: &str, title: &str) -> String {
    let mut combined = String::with_capacity(url.len() + title.len() + 2);
    combined.push_str(&bare(url).to_lowercase());
    combined.push('\n');
    combined.push_str(&title.to_lowercase());
    let cleaned = combined.replace(MARK, "");
    mark_words(&cleaned)
}

fn mark_words(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 16);
    let mut in_word = false;
    for ch in text.chars() {
        let is_word = ch.is_alphanumeric();
        if is_word && !in_word {
            out.push(MARK);
        }
        out.push(ch);
        in_word = is_word;
    }
    out
}

pub fn match_quality(query: &Query, text: &str) -> u8 {
    let mut quality = START;
    for term in &query.terms {
        let Some(at) = text.find(&term.inside) else {
            return MISS;
        };
        let starts_here = at > 0 && text.as_bytes()[at - 1] == 1;
        let starts_word = match &term.start {
            None => false,
            Some(start_pat) => starts_here || text[at + 1..].contains(start_pat.as_str()),
        };
        if !starts_word {
            quality = INSIDE;
        }
    }
    quality
}

pub fn completes(query: &Query, text: &str) -> bool {
    query.prefix.as_deref().is_some_and(|p| text.starts_with(p))
}

/// `log2(1 + visits + typed)`: typed visits count twice (`match.ts::frequency`).
pub fn frequency(visits: i64, typed: i64) -> f64 {
    (1.0 + (visits.max(0) + typed.max(0)) as f64).log2()
}

/// Five-step recency decay (`match.ts::recency`): 1.0 within 4d -> 0.7 at 14d -> 0.5 at 31d ->
/// 0.3 at 90d -> 0.1 at 365d+.
pub fn recency(visited_ms: i64, now_ms: i64) -> f64 {
    let days = (now_ms - visited_ms) as f64 / DAY_MS;
    if days <= 4.0 {
        1.0
    } else if days <= 14.0 {
        0.7 + ((14.0 - days) * 0.3) / 10.0
    } else if days <= 31.0 {
        0.5 + ((31.0 - days) * 0.2) / 17.0
    } else if days <= 90.0 {
        0.3 + ((90.0 - days) * 0.2) / 59.0
    } else if days <= 365.0 {
        0.1 + ((365.0 - days) * 0.2) / 275.0
    } else {
        0.1
    }
}

#[derive(Clone, Debug)]
struct Entry {
    id: i64,
    text: String,
    host: String,
    frequency: f64,
    visited: i64,
}

#[derive(Clone, Debug)]
struct Ranked {
    id: i64,
    quality: u8,
    score: f64,
    visited: i64,
}

#[derive(Clone, Debug)]
struct HostSum {
    host: String,
    score: f64,
    page: i64,
    best: f64,
}

impl HostSum {
    /// Caps summed multi-page host score at 4x the host's single best page so one site spamming
    /// `history.pushState` paths cannot hijack row-1 inline autocomplete over a typed site.
    fn capped_score(&self) -> f64 {
        self.score.min(self.best * 4.0)
    }
}

/// Compact in-memory search index synchronized with `history.sqlite`.
#[derive(Default)]
pub struct PageIndex {
    entries: Vec<Entry>,
    slots: HashMap<i64, usize>,
}

impl PageIndex {
    pub fn size(&self) -> usize {
        self.entries.len()
    }

    pub fn set(&mut self, id: i64, page: &PageVisit) {
        let bare_lower = bare(&page.url).to_lowercase();
        let host_end = bare_lower.find(['/', '?', '#']).unwrap_or(bare_lower.len());
        let host = bare_lower[..host_end].to_owned();
        let entry = Entry {
            id,
            text: search_text(&page.url, &page.title),
            host,
            frequency: frequency(page.visits, page.typed),
            visited: page.visited,
        };
        if let Some(&slot) = self.slots.get(&id) {
            self.entries[slot] = entry;
        } else {
            let slot = self.entries.len();
            self.slots.insert(id, slot);
            self.entries.push(entry);
        }
    }

    pub fn remove(&mut self, id: i64) {
        let Some(slot) = self.slots.remove(&id) else { return };
        let Some(last) = self.entries.pop() else { return };
        if last.id != id {
            self.slots.insert(last.id, slot);
            self.entries[slot] = last;
        }
    }

    pub fn clear(&mut self) {
        self.entries.clear();
        self.slots.clear();
    }

    /// Returns IDs of the least-frecency / oldest pages to prune down to `keep`.
    pub fn least(&self, keep: usize, now_ms: i64) -> Vec<i64> {
        if self.entries.len() <= keep {
            return Vec::new();
        }
        let mut scored: Vec<(i64, f64, i64)> = self
            .entries
            .iter()
            .map(|e| (e.id, e.frequency * recency(e.visited, now_ms), e.visited))
            .collect();
        scored.sort_by(|a, b| a.1.total_cmp(&b.1).then_with(|| a.2.cmp(&b.2)));
        scored.into_iter().take(self.entries.len() - keep).map(|(id, _, _)| id).collect()
    }

    /// Ranks up to `limit` matching pages plus the best host-prefix front page when typing a single
    /// host prefix (`search.ts::findPages`).
    pub fn find_pages<FShown, FFront>(
        &self,
        text: &str,
        limit: usize,
        now_ms: i64,
        mut shown: FShown,
        mut front: FFront,
    ) -> Vec<FoundPage>
    where
        FShown: FnMut(i64) -> Option<(String, String)>,
        FFront: FnMut(&str) -> Option<(String, String)>,
    {
        if limit == 0 {
            return Vec::new();
        }
        let Some(query) = parse_query(text) else {
            return Vec::new();
        };
        let track_hosts = query.words.len() == 1 && !query.words[0].contains(['/', '?', '#']);
        let mut sums: HashMap<&str, HostSum> = HashMap::new();
        let mut top: Vec<Ranked> = Vec::with_capacity(limit + 1);

        for entry in &self.entries {
            let quality = match_quality(&query, &entry.text);
            if quality == MISS {
                continue;
            }
            let score = entry.frequency * recency(entry.visited, now_ms);
            if track_hosts && completes(&query, &entry.text) {
                sums.entry(entry.host.as_str())
                    .and_modify(|h| {
                        h.score += score;
                        if score > h.best {
                            h.page = entry.id;
                            h.best = score;
                        }
                    })
                    .or_insert_with(|| HostSum {
                        host: entry.host.clone(),
                        score,
                        page: entry.id,
                        best: score,
                    });
            }
            let cand = Ranked { id: entry.id, quality, score, visited: entry.visited };
            if top.len() == limit && !ranked_before(&cand, &top[limit - 1]) {
                continue;
            }
            let pos = top.iter().position(|other| ranked_before(&cand, other)).unwrap_or(top.len());
            top.insert(pos, cand);
            if top.len() > limit {
                top.pop();
            }
        }

        let mut pages: Vec<(FoundPage, u8)> = top
            .into_iter()
            .filter_map(|item| {
                let (url, title) = shown(item.id)?;
                Some((FoundPage { url, title, score: item.score }, item.quality))
            })
            .collect();

        let best_host = sums.into_values().max_by(|a, b| a.capped_score().total_cmp(&b.capped_score()));
        if let Some(host_sum) = best_host
            && let Some((best_url, _)) = shown(host_sum.page)
            && let Some(origin) = origin_of(&best_url)
        {
            let front_row = front(&host_sum.host);
            let site_url = front_row.as_ref().map(|(u, _)| u.clone()).unwrap_or_else(|| format!("{origin}/"));
            let site_title = front_row.map(|(_, t)| t).unwrap_or_default();
            pages.retain(|(p, _)| p.url != site_url);
            let site_page = FoundPage { url: site_url, title: site_title, score: host_sum.capped_score() };
            let insert_at = pages
                .iter()
                .position(|(p, q)| *q < START || p.score <= site_page.score)
                .unwrap_or(pages.len());
            pages.insert(insert_at, (site_page, START));
            pages.truncate(limit);
        }

        pages.into_iter().map(|(p, _)| p).collect()
    }
}

fn ranked_before(a: &Ranked, b: &Ranked) -> bool {
    if a.quality != b.quality {
        return a.quality > b.quality;
    }
    if a.score != b.score {
        return a.score > b.score;
    }
    if a.visited != b.visited {
        return a.visited > b.visited;
    }
    a.id < b.id
}
