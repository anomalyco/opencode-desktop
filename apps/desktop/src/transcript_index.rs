//! Find's index of a session's searchable rows (`crate::find`): each row's text, copied once
//! per transcript shape, its letters (to rule rows out with one scan) and, for a reply, the text
//! its Markdown shows (`rune_kit::markdown::Rendered`), so a query never parses a reply. It is
//! built on a worker, reusing every row whose text didn't change, and searched there too when a
//! query matches more rows than a key may match in its own frame.

use std::collections::HashMap;
use std::ops::Range;
use std::sync::Arc;

use rune_kit::markdown::{Case, Rendered};
use rune_kit::search::Fold;

/// Frees x on another thread: a dense query's matches or a large index take tens of
/// milliseconds to free, which a key's frame mustn't spend.
pub(crate) fn retire<T: Send + 'static>(x: T) {
    #[cfg(not(target_arch = "wasm32"))]
    let _ = std::thread::Builder::new().name("find-free".into()).spawn(move || drop(x));
    #[cfg(target_arch = "wasm32")]
    drop(x);
}

/// Rows a key matches itself; past this, the rows on screen now and the rest on the worker.
pub(crate) const KEY_BUDGET: usize = 256;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum Kind {
    /// A user bubble: plain text as shown.
    Bubble,
    /// A reply's text part, drawn as Markdown.
    Reply,
    /// A reply with an outline (```` ```ui ````) while a plugin draws them: matched in the words it shows.
    Outline,
}

pub(crate) struct SourceRow {
    body: Range<u32>,
    id: Range<u32>,
    /// Which text part of its message (a bubble's is 0).
    n: u32,
    kind: Kind,
    sig: u64,
}

/// The searchable rows of a transcript as the UI thread copied them, in timeline order.
#[derive(Default)]
pub(crate) struct Source {
    pub shape: u64,
    text: String,
    ids: String,
    rows: Vec<SourceRow>,
}

impl Source {
    pub fn new(shape: u64) -> Self {
        Source { shape, ..Source::default() }
    }

    pub fn push(&mut self, id: &str, n: u32, kind: Kind, body: &str, sig: u64) {
        let b = self.text.len() as u32;
        self.text.push_str(body);
        let i = self.ids.len() as u32;
        self.ids.push_str(id);
        self.rows.push(SourceRow { body: b..self.text.len() as u32, id: i..self.ids.len() as u32, n, kind, sig });
    }

    pub fn len(&self) -> usize {
        self.rows.len()
    }

    pub fn body(&self, row: usize) -> &str {
        let r = &self.rows[row].body;
        &self.text[r.start as usize..r.end as usize]
    }

    fn id(&self, row: usize) -> &str {
        let r = &self.rows[row].id;
        &self.ids[r.start as usize..r.end as usize]
    }

    pub fn kind(&self, row: usize) -> Kind {
        self.rows[row].kind
    }

    /// The timeline's key for the row: `u-{id}` for a bubble, `t-{id}-{n}` for a reply's text.
    pub fn key(&self, row: usize) -> String {
        match self.rows[row].kind {
            Kind::Bubble => format!("u-{}", self.id(row)),
            Kind::Reply | Kind::Outline => format!("t-{}-{}", self.id(row), self.rows[row].n),
        }
    }
}

pub(crate) struct Index {
    pub source: Arc<Source>,
    /// Each row's letters (`SearchFold::letters` of what it shows), end to end.
    letters: String,
    ends: Vec<usize>,
    rendered: Vec<Option<Arc<Rendered>>>,
    by_key: HashMap<Box<str>, u32>,
}

impl Index {
    /// The index of `source`, taking each unchanged row's letters and rendered text from `prev`.
    pub fn build(source: Arc<Source>, prev: Option<&Index>) -> Index {
        let fold = opencode_search::SearchFold::default();
        let old: HashMap<(&str, u32, Kind), usize> = prev.map(|p| (0..p.source.len()).map(|i| ((p.source.id(i), p.source.rows[i].n, p.source.rows[i].kind), i)).collect()).unwrap_or_default();
        let mut letters = String::new();
        let mut ends = Vec::with_capacity(source.len());
        let mut rendered = Vec::with_capacity(source.len());
        let mut by_key = HashMap::with_capacity(source.len());
        for i in 0..source.len() {
            let row = &source.rows[i];
            let reuse = prev.zip(old.get(&(source.id(i), row.n, row.kind))).filter(|(p, j)| p.source.rows[**j].sig == row.sig);
            match reuse {
                Some((p, &j)) => {
                    letters.push_str(&p.letters[p.start(j)..p.ends[j] - 1]);
                    rendered.push(p.rendered[j].clone());
                }
                None => {
                    let body = source.body(i);
                    let shows = (row.kind == Kind::Reply).then(|| Arc::new(Rendered::new(body)));
                    match &shows {
                        Some(r) => r.texts().for_each(|t| {
                            letters.push_str(&fold.letters(t));
                            letters.push('\n');
                        }),
                        None => letters.push_str(&fold.letters(body)),
                    }
                    rendered.push(shows);
                }
            }
            letters.push('\n');
            ends.push(letters.len());
            by_key.insert(source.key(i).into_boxed_str(), i as u32);
        }
        Index { source, letters, ends, rendered, by_key }
    }

    fn start(&self, row: usize) -> usize {
        if row == 0 { 0 } else { self.ends[row - 1] }
    }

    pub fn row_of(&self, key: &str) -> Option<usize> {
        self.by_key.get(key).map(|&r| r as usize)
    }

    /// Rows that can hold the query: those whose letters hold its letters, and outlines (whose
    /// words come from a plugin). Every row for a query with no letters.
    pub fn candidates(&self, query: &str) -> Vec<usize> {
        self.candidates_within(query, usize::MAX).unwrap_or_default()
    }

    /// [`Self::candidates`] when there are at most `limit`; `None` as soon as there are more, so
    /// a key learns that the worker must match them without finding them all.
    pub fn candidates_within(&self, query: &str, limit: usize) -> Option<Vec<usize>> {
        let needle = opencode_search::SearchFold::default().letters(query);
        let n = self.source.len();
        if needle.is_empty() {
            return (n <= limit).then(|| (0..n).collect());
        }
        let finder = memchr::memmem::Finder::new(needle.as_bytes());
        let letters = self.letters.as_bytes();
        let mut out = Vec::new();
        let (mut row, mut at) = (0, 0);
        while row < n {
            // An outline can't be ruled out; else the next row whose letters hold the needle,
            // searched from where the last one ended (one hit a row, however many it holds).
            if self.source.rows[row].kind != Kind::Outline {
                let Some(found) = finder.find(&letters[at..]) else {
                    out.extend((row..n).filter(|&r| self.source.rows[r].kind == Kind::Outline));
                    break;
                };
                let next = row + self.ends[row..].partition_point(|&e| e <= at + found);
                out.extend((row..next).filter(|&r| self.source.rows[r].kind == Kind::Outline));
                row = next;
            }
            if row >= n {
                break;
            }
            out.push(row);
            if out.len() > limit {
                return None;
            }
            at = self.ends[row];
            row += 1;
        }
        (out.len() <= limit).then_some(out)
    }

    /// Row `row`'s matches as source byte ranges of its text (several where a match spans markup).
    pub fn matches(&self, row: usize, query: &str, fold: Fold) -> Vec<Vec<Range<usize>>> {
        let body = self.source.body(row);
        match (self.source.kind(row), &self.rendered[row]) {
            (Kind::Bubble, _) => fold.find(body, query).into_iter().map(|r| vec![r]).collect(),
            (Kind::Reply, Some(r)) => r.find(body, query, Case::Folded(fold)),
            (Kind::Reply, None) => rune_kit::markdown::find(body, query, Case::Folded(fold)),
            (Kind::Outline, _) => crate::genui::find(body, query),
        }
    }

    /// Every row's matches among `rows`, in timeline order.
    pub fn search(&self, rows: &[usize], query: &str, fold: Fold) -> Results {
        Results::new(rows.iter().filter_map(|&r| {
            let ranges = self.matches(r, query, fold);
            (!ranges.is_empty()).then(|| (self.source.key(r), ranges))
        }))
    }
}

/// One row's matches.
pub(crate) struct RowHit {
    pub key: Box<str>,
    pub ranges: Vec<Vec<Range<usize>>>,
    /// Its first match's place among all the matches.
    pub first: usize,
    /// A hash of its ranges, for the row's memo.
    pub version: u64,
}

/// A query's matches by row, in timeline order.
#[derive(Default)]
pub(crate) struct Results {
    pub hits: Vec<RowHit>,
    by_key: HashMap<Box<str>, usize>,
    pub total: usize,
}

impl Results {
    pub fn new(rows: impl Iterator<Item = (String, Vec<Vec<Range<usize>>>)>) -> Results {
        let mut out = Results::default();
        for (key, ranges) in rows {
            let key = key.into_boxed_str();
            out.by_key.insert(key.clone(), out.hits.len());
            let version = crate::find::row_hash(&ranges, None);
            let first = out.total;
            out.total += ranges.len();
            out.hits.push(RowHit { key, ranges, first, version });
        }
        out
    }

    pub fn get(&self, key: &str) -> Option<&RowHit> {
        self.by_key.get(key).map(|&i| &self.hits[i])
    }

    /// Match `n`'s row and its place in that row.
    pub fn locate(&self, n: usize) -> Option<(&RowHit, usize)> {
        if n >= self.total {
            return None;
        }
        let i = self.hits.partition_point(|h| h.first <= n) - 1;
        Some((&self.hits[i], n - self.hits[i].first))
    }

    /// Where match `local` of row `key` is among all of them.
    pub fn place(&self, key: &str, local: usize) -> Option<usize> {
        self.get(key).filter(|h| local < h.ranges.len()).map(|h| h.first + local)
    }
}
