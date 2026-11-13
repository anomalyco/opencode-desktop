use std::ops::Range;

use memchr::memmem::Finder;

use crate::{Folded, SearchFold};

#[derive(Debug, PartialEq, Eq)]
pub struct SlabMatch {
    pub row: usize,
    pub bytes: Range<usize>,
}

/// One contiguous scan over the loaded document. ASCII rows need no offset map;
/// keep a source copy for non-ASCII rows only, for a query with explicit accents.
#[derive(Default)]
pub struct SlabFind {
    text: String,
    ends: Vec<usize>,
    mapped: Vec<(usize, Box<str>, Folded)>,
}

impl SlabFind {
    pub fn push(&mut self, row: &str) {
        let start = self.text.len();
        if row.is_ascii() {
            self.text.push_str(row);
            self.text[start..].make_ascii_lowercase();
        } else {
            let projection = SearchFold::default().fold_with_map(row);
            self.text.push_str(projection.text());
            self.mapped.push((self.ends.len(), row.into(), projection));
        }
        self.text.push('\n');
        self.ends.push(self.text.len());
    }

    pub fn find(&self, query: &str, mut sink: impl FnMut(SlabMatch)) {
        let mode = SearchFold::for_query(query);
        let needle = mode.fold(query);
        if needle.is_empty() {
            return;
        }
        let finder = Finder::new(needle.as_bytes());
        // A strict query is not necessarily present in the loose projection.
        if mode != SearchFold::default() {
            for (row, source, _) in &self.mapped {
                let text = mode.fold_with_map(source);
                let mut last_end = 0;
                for p in finder.find_iter(text.text().as_bytes()) {
                    if let Some(bytes) = text.source_range(p..p + needle.len())
                        && bytes.start >= last_end
                    {
                        last_end = bytes.end;
                        sink(SlabMatch { row: *row, bytes });
                    }
                }
            }
            return;
        }
        let mut row = 0;
        let mut mapped = 0;
        let mut prev = None;
        for p in finder.find_iter(self.text.as_bytes()) {
            if self.ends[row] <= p {
                row += 1;
                if self.ends[row] <= p {
                    row += self.ends[row..].partition_point(|end| *end <= p);
                }
            }
            if p + needle.len() >= self.ends[row] {
                continue;
            } // no cross-row matches
            let from = p - self.ends.get(row.wrapping_sub(1)).copied().unwrap_or(0);
            let range = from..from + needle.len();
            mapped += self.mapped[mapped..].partition_point(|r| r.0 < row);
            let range = if let Some((id, _, map)) = self.mapped.get(mapped)
                && *id == row
            {
                let Some(r) = map.source_range(range) else {
                    continue;
                };
                if prev == Some((row, r.start)) {
                    continue;
                }
                prev = Some((row, r.start));
                r
            } else {
                range
            };
            sink(SlabMatch { row, bytes: range });
        }
    }
}
