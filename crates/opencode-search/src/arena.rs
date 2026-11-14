use std::cmp::Reverse;
use std::ops::Range;

use memchr::memmem::Finder;

use crate::SearchFold;

/// The least score of a row that holds the query as a substring: a subsequence scores less.
pub const SUBSTRING: i32 = 5_000;

/// Dense folded strings and a transposed character mask: one AND discards up to
/// 64 rows. IDs are insertion-order indices into the caller's immutable snapshot.
#[derive(Default)]
pub struct SearchArena {
    data: String,
    ends: Vec<usize>,
    masks: Vec<[u64; 64]>,
    // Non-ASCII rows only: a query that specifies accents can ask for the original.
    originals: Vec<(usize, Box<str>)>,
    first_letters: Option<(usize, Box<[Vec<usize>; 26]>)>,
}

fn mask(s: &str) -> u64 {
    s.bytes().fold(0, |m, b| {
        m | 1
            << match b {
                b'a'..=b'z' => b - b'a',
                b'0'..=b'9' => 26 + b - b'0',
                _ => 36 + b % 28,
            }
    })
}

fn boundary(c: u8) -> bool {
    matches!(c, b' ' | b'/' | b'\\' | b'-' | b'_' | b'.' | 0)
}

impl SearchArena {
    pub fn push(&mut self, text: &str) -> usize {
        self.first_letters = None;
        let id = self.ends.len();
        if id % 64 == 0 {
            self.masks.push([0; 64]);
        }
        let start = self.data.len();
        if text.is_ascii() {
            self.data.push_str(text);
            self.data[start..].make_ascii_lowercase();
        } else {
            self.data.push_str(&SearchFold::default().fold(text));
            if SearchFold::for_query(text) != SearchFold::default() {
                self.originals.push((id, text.into()));
            }
        }
        self.ends.push(self.data.len());
        let mut m = mask(&self.data[start..]);
        while m != 0 {
            self.masks[id / 64][m.trailing_zeros() as usize] |= 1 << (id % 64);
            m &= m - 1;
        }
        id
    }

    /// Release builder slack before publishing a snapshot to readers.
    pub fn compact(&mut self) {
        self.data.shrink_to_fit();
        self.ends.shrink_to_fit();
        self.masks.shrink_to_fit();
        self.originals.shrink_to_fit();
    }

    /// Build once with immutable rows, not when the list opens. Keep only the
    /// best `limit` rows per letter. Other scopes and larger limits still use
    /// full ranking; an exclusion after truncation would lose valid hits.
    pub fn prepare_first_letters(&mut self, limit: usize) {
        self.first_letters = Some((
            limit,
            Box::new(std::array::from_fn(|i| {
                self.ranked(&char::from(b'a' + i as u8).to_string(), limit, |_| {
                    Some((0, 0))
                })
            })),
        ));
    }

    pub fn allocated_bytes(&self) -> usize {
        self.data.capacity()
            + self.ends.capacity() * size_of::<usize>()
            + self.masks.capacity() * 512
            + self.originals.capacity() * size_of::<(usize, Box<str>)>()
            + self.originals.iter().map(|r| r.1.len()).sum::<usize>()
            + self.first_letters.as_ref().map_or(0, |(_, lists)| {
                size_of_val(lists.as_ref())
                    + lists
                        .iter()
                        .map(|v| v.capacity() * size_of::<usize>())
                        .sum::<usize>()
            })
    }

    pub fn query(&self, query: &str, limit: usize) -> Vec<usize> {
        if let [b] = query.trim().as_bytes()
            && b.is_ascii_alphabetic()
            && let Some((prepared, first)) = &self.first_letters
            && limit <= *prepared
        {
            let rows = &first[usize::from(b.to_ascii_lowercase() - b'a')];
            return rows[..limit.min(rows.len())].to_vec();
        }
        self.ranked(query, limit, |_| Some((0, 0)))
    }

    /// `context` excludes rows outside the surface's scope, or adds a context boost
    /// and recency tie break. Keep at most 2K candidates even for a one-letter query.
    pub fn ranked(
        &self,
        query: &str,
        limit: usize,
        context: impl Fn(usize) -> Option<(i32, i64)>,
    ) -> Vec<usize> {
        self.ranked_scored(query, limit, context).into_iter().map(|t| t.2).collect()
    }

    /// [`Self::ranked`] with each row's score and recency, best first, so results from
    /// several arenas merge in one order.
    pub fn ranked_scored(
        &self,
        query: &str,
        limit: usize,
        context: impl Fn(usize) -> Option<(i32, i64)>,
    ) -> Vec<(i32, i64, usize)> {
        self.ranked_scored_in(query, limit, usize::MAX, context)
    }

    /// [`Self::ranked_scored`] over each row's first `fields` `\0`-separated fields only (a
    /// surface that matches titles, of rows that hold `title\0project`).
    pub fn ranked_scored_in(
        &self,
        query: &str,
        limit: usize,
        fields: usize,
        context: impl Fn(usize) -> Option<(i32, i64)>,
    ) -> Vec<(i32, i64, usize)> {
        let limit = limit.min(self.ends.len());
        if limit == 0 {
            return Vec::new();
        }
        let raw = query.trim();
        let mode = SearchFold::for_query(raw);
        let loose = SearchFold::default().fold(raw);
        if !raw.is_empty() && loose.is_empty() && mode == SearchFold::default() {
            return Vec::new();
        }
        let q = if mode == SearchFold::default() {
            loose.as_str().into()
        } else {
            std::borrow::Cow::Owned(mode.fold(raw))
        };
        let finder = Finder::new(q.as_bytes());
        let mut bits = [0usize; 64];
        let mut count = 0;
        let mut m = mask(&loose);
        while m != 0 {
            bits[count] = m.trailing_zeros() as usize;
            count += 1;
            m &= m - 1;
        }
        let mut best = Vec::with_capacity(limit.saturating_mul(2));
        for (block, masks) in self.masks.iter().enumerate() {
            let mut candidates = u64::MAX;
            for &bit in &bits[..count] {
                candidates &= masks[bit];
                if candidates == 0 {
                    break;
                }
            }
            while candidates != 0 {
                let id = block * 64 + candidates.trailing_zeros() as usize;
                if id >= self.ends.len() {
                    break;
                }
                candidates &= candidates - 1;
                let Some((boost, updated)) = context(id) else {
                    continue;
                };
                let mut hay = &self.data
                    [self.ends.get(id.wrapping_sub(1)).copied().unwrap_or(0)..self.ends[id]];
                let precise;
                if mode != SearchFold::default()
                    && let Ok(i) = self.originals.binary_search_by_key(&id, |r| r.0)
                {
                    precise = mode.fold(&self.originals[i].1);
                    hay = &precise;
                }
                if fields != usize::MAX {
                    let cut = hay.match_indices('\0').nth(fields.wrapping_sub(1)).map_or(hay.len(), |(i, _)| i);
                    hay = &hay[..cut];
                }
                let score = if q.is_empty() {
                    0
                } else if let Some(pos) = finder.find(hay.as_bytes()) {
                    if hay == q {
                        30_000
                    } else {
                        let base = if pos == 0 {
                            20_000
                        } else if boundary(hay.as_bytes()[pos - 1]) {
                            15_000
                        } else {
                            10_000
                        };
                        base - (pos.saturating_mul(2) + hay.len()).min(4_999) as i32
                    }
                } else if let Some(score) = fuzzy(hay, &q) {
                    score
                } else {
                    continue;
                };
                best.push((Reverse(score.saturating_add(boost)), Reverse(updated), id));
                if best.len() == limit * 2 {
                    best.select_nth_unstable(limit);
                    best.truncate(limit);
                }
            }
        }
        best.sort_unstable();
        best.truncate(limit);
        best.into_iter().map(|(Reverse(score), Reverse(updated), id)| (score, updated, id)).collect()
    }

    /// Only visible rows pay for byte maps, never the full list on a key.
    pub fn highlights(source: &str, query: &str) -> Vec<Range<usize>> {
        let mode = SearchFold::for_query(query.trim());
        let hay = mode.fold_with_map(source);
        let needle = mode.fold(query.trim());
        if needle.is_empty() {
            return Vec::new();
        }
        if let Some(pos) = hay.text().find(&needle) {
            return hay
                .source_range(pos..pos + needle.len())
                .into_iter()
                .collect();
        }
        let mut spans: Vec<Range<usize>> = Vec::new();
        if subsequence(hay.text(), &needle, |r| {
            if let Some(r) = hay.source_range(r) {
                if let Some(last) = spans.last_mut()
                    && last.end >= r.start
                {
                    last.end = last.end.max(r.end);
                } else {
                    spans.push(r);
                }
            }
        })
        .is_none()
        {
            spans.clear();
        }
        spans
    }
}

/// What a row may match loosely: one `\0` field at a time, so a query's letters never gather
/// across fields (`title\0project`).
fn fuzzy(hay: &str, q: &str) -> Option<i32> {
    hay.split('\0').filter_map(|field| subsequence(field, q, |_| ())).max()
}

fn subsequence(hay: &str, q: &str, mut found: impl FnMut(Range<usize>)) -> Option<i32> {
    let mut at = 0;
    let mut score = 1000i32;
    for c in q.chars() {
        let gap = hay[at..].find(c)?;
        at += gap;
        score = score.saturating_add(
            12 + if gap == 0 { 16 } else { 0 }
                + if at == 0 || boundary(hay.as_bytes()[at - 1]) {
                    14
                } else {
                    0
                },
        );
        score = score.saturating_sub(gap.min(1000) as i32);
        found(at..at + c.len_utf8());
        at += c.len_utf8();
    }
    Some(score.min(4999))
}
