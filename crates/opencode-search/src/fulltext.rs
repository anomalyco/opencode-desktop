//! Full text's side of the storage contract (`notes/resilience-status.md`, "From SEARCH"): the
//! storage lane answers a quoted `fts_query` with up to 250 newest matching rows; this ranks
//! them, keeps at most two a session, and cuts each row's snippet with its marks.
//! Integer scores only, so equal inputs always order the same.

use std::ops::Range;

use crate::SearchFold;

/// What a row matched in: weighs it (`better-search.md` 4.4, tier 2).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Kind {
    SessionTitle,
    Draft,
    UserPrompt,
    AssistantReply,
    Tool,
}

impl Kind {
    fn weight(self) -> i32 {
        match self {
            Kind::SessionTitle => 8_000,
            Kind::Draft => 6_500,
            Kind::UserPrompt => 5_000,
            Kind::AssistantReply => 3_500,
            Kind::Tool => 2_500,
        }
    }
}

/// One row the storage lane returned, with its text as stored.
#[derive(Clone, Debug)]
pub struct Hit {
    pub session: String,
    /// The message, part or draft it is in; `None` for a session's title.
    pub entity: Option<String>,
    pub kind: Kind,
    pub title: String,
    pub body: String,
    pub updated_ms: i64,
    /// In the window's open project.
    pub active_project: bool,
}

/// A ranked row: its index in the hits, and how many more of its session's rows it stands for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Ranked {
    pub hit: usize,
    pub more: usize,
}

fn recency(now_ms: i64, updated_ms: i64) -> i32 {
    const HOUR: i64 = 3_600_000;
    match now_ms.saturating_sub(updated_ms) {
        age if age < HOUR => 2_500,
        age if age < 24 * HOUR => 1_800,
        age if age < 7 * 24 * HOUR => 1_000,
        age if age < 30 * 24 * HOUR => 400,
        _ => 0,
    }
}

/// The query, folded as each surface folds it, in one phrase.
fn phrase(query: &str) -> (SearchFold, String) {
    let words: Vec<&str> = query.split_whitespace().collect();
    let mode = SearchFold::for_query(query.trim());
    (mode, mode.fold(&words.join(" ")))
}

pub fn score(hit: &Hit, query: &str, now_ms: i64) -> i32 {
    let (mode, needle) = phrase(query);
    let mut score = hit.kind.weight() + recency(now_ms, hit.updated_ms);
    if !needle.is_empty() {
        if mode.fold(&hit.title).contains(&needle) {
            score += 6_000;
        } else if mode.fold(&hit.body).contains(&needle) {
            score += 3_000;
        }
    }
    if hit.active_project {
        score += 3_000;
    }
    score
}

/// Best first, at most `per_session` rows a session (its best ones); the last row kept for a
/// session counts the ones left out. Ties go to the newer row, then the session id, then the entity.
pub fn rank(hits: &[Hit], query: &str, now_ms: i64, per_session: usize) -> Vec<Ranked> {
    let mut order: Vec<(i32, usize)> = hits.iter().enumerate().map(|(i, h)| (score(h, query, now_ms), i)).collect();
    order.sort_unstable_by(|(sa, a), (sb, b)| {
        let (ha, hb) = (&hits[*a], &hits[*b]);
        sb.cmp(sa).then(hb.updated_ms.cmp(&ha.updated_ms)).then_with(|| ha.session.cmp(&hb.session)).then_with(|| ha.entity.cmp(&hb.entity))
    });
    let mut kept: Vec<Ranked> = Vec::new();
    let mut seen: std::collections::HashMap<&str, (usize, usize)> = std::collections::HashMap::new();
    for (_, i) in order {
        let entry = seen.entry(hits[i].session.as_str()).or_insert((0, usize::MAX));
        if entry.0 < per_session.max(1) {
            entry.0 += 1;
            entry.1 = kept.len();
            kept.push(Ranked { hit: i, more: 0 });
        } else {
            kept[entry.1].more += 1;
        }
    }
    kept
}

/// A row's line of context: `text` from the body (line breaks as spaces), the query's words
/// marked in it, and whether it was cut before or after.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Snippet {
    pub text: String,
    pub marks: Vec<Range<usize>>,
    pub cut_before: bool,
    pub cut_after: bool,
}

/// About `width` characters of `body` around the query's first match: a third before it, the
/// rest after. `None` when no word of the query is in it.
pub fn snippet(body: &str, query: &str, width: usize) -> Option<Snippet> {
    let mode = SearchFold::for_query(query.trim());
    let hay = mode.fold_with_map(body);
    let mut marks: Vec<Range<usize>> = Vec::new();
    for word in query.split_whitespace() {
        let needle = mode.fold(word);
        if needle.is_empty() {
            continue;
        }
        let mut from = 0;
        while let Some(at) = hay.text()[from..].find(&needle) {
            let at = from + at;
            marks.extend(hay.source_range(at..at + needle.len()));
            from = at + needle.len();
        }
    }
    marks.sort_by_key(|r| (r.start, r.end));
    marks.dedup_by(|b, a| {
        if b.start <= a.end {
            a.end = a.end.max(b.end);
            true
        } else {
            false
        }
    });
    let first = marks.first()?.clone();
    let chars: Vec<usize> = body.char_indices().map(|(i, _)| i).chain(std::iter::once(body.len())).collect();
    let at = chars.partition_point(|&i| i < first.start);
    let lead = width / 3;
    let start_char = at.saturating_sub(lead);
    let end_char = (start_char + width).min(chars.len() - 1);
    let (start, end) = (chars[start_char], chars[end_char]);
    let text: String = body[start..end].chars().map(|c| if c == '\n' || c == '\r' || c == '\t' { ' ' } else { c }).collect();
    let marks = marks
        .into_iter()
        .filter(|r| r.start < end && r.end > start)
        .map(|r| r.start.max(start) - start..r.end.min(end) - start)
        .collect();
    Some(Snippet { text, marks, cut_before: start > 0, cut_after: end < body.len() })
}
