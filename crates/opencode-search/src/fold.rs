use std::ops::Range;

use icu_normalizer::{ComposingNormalizerBorrowed, DecomposingNormalizerBorrowed};
use unicode_segmentation::UnicodeSegmentation;

/// A query chooses which distinctions it asks to keep. An unmarked query is the
/// projection kept in the index. A marked query verifies candidates against the source.
#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
pub struct SearchFold {
    accents: bool,
    vocalization: bool,
    hamza: bool,
    digits: bool,
}

pub(crate) fn vocalization(c: char) -> bool {
    matches!(c as u32, 0x591..=0x5bd | 0x5bf | 0x5c1..=0x5c2 | 0x5c4..=0x5c5 | 0x5c7 | 0x610..=0x61a | 0x64b..=0x65f | 0x670 | 0x6d6..=0x6ed)
}

fn accent(c: char) -> bool {
    matches!(c as u32, 0x300..=0x36f | 0x1ab0..=0x1aff | 0x1dc0..=0x1dff | 0xfe20..=0xfe2f)
}

impl SearchFold {
    pub fn for_query(s: &str) -> Self {
        let mut f = Self::default();
        if !s.is_ascii() {
            for c in s.chars().filter(|c| !c.is_ascii()) {
                f.vocalization |= vocalization(c);
                f.hamza |= matches!(c, 'أ' | 'آ' | 'إ' | 'ٱ');
                f.digits |= matches!(c, '٠'..='٩' | '۰'..='۹');
                f.accents |= accent(c)
                    || matches!(c.to_ascii_lowercase(), 'ø' | 'Ø' | 'ł' | 'Ł' | 'đ' | 'Đ')
                    || (c != 'İ'
                        && matches!(c as u32, 0xc0..=0x24f | 0x370..=0x52f | 0x1e00..=0x1fff)
                        && DecomposingNormalizerBorrowed::new_nfd()
                            .normalize_iter([c].into_iter())
                            .any(accent));
            }
        }
        f
    }

    pub fn fold(self, s: &str) -> String {
        if s.is_ascii() {
            return s.to_ascii_lowercase();
        }
        let mut out = String::with_capacity(s.len());
        for cluster in s.graphemes(true) {
            self.push(cluster, &mut out);
        }
        out
    }

    fn push(self, cluster: &str, out: &mut String) {
        if cluster.is_ascii() {
            out.extend(cluster.bytes().map(|b| char::from(b.to_ascii_lowercase())));
            return;
        }
        // Compose before folding, including half-width kana plus a voiced mark and
        // Arabic presentation forms. Keep a whole source cluster as one highlight.
        let normalized = ComposingNormalizerBorrowed::new_nfkc().normalize(cluster);
        for c in normalized.chars() {
            match c {
                'ß' | 'ẞ' => out.push_str("ss"),
                'İ' | 'ı' => out.push('i'),
                'ς' => out.push('σ'),
                'أ' | 'آ' | 'إ' | 'ٱ' if !self.hamza => out.push('ا'),
                'ي' | 'ى' => out.push('ی'),
                'ك' => out.push('ک'),
                'ہ' | 'ۀ' => out.push('ه'),
                'ך' => out.push('כ'),
                'ם' => out.push('מ'),
                'ן' => out.push('נ'),
                'ף' => out.push('פ'),
                'ץ' => out.push('צ'),
                '٠'..='٩' | '۰'..='۹' if !self.digits => {
                    out.push(char::from(b'0' + (c as u32 % 16) as u8))
                }
                'ـ'
                | '\u{200b}'..='\u{200f}'
                | '\u{202a}'..='\u{202e}'
                | '\u{2060}'..='\u{2069}'
                | '\u{fe00}'..='\u{fe0f}'
                | '\u{feff}'
                | '\u{e0100}'..='\u{e01ef}' => (),
                c if !self.vocalization && vocalization(c) => (),
                c if !self.accents && accent(c) => (),
                'ø' | 'Ø' if !self.accents => out.push('o'),
                'ł' | 'Ł' if !self.accents => out.push('l'),
                'đ' | 'Đ' | 'ð' | 'Ð' if !self.accents => out.push('d'),
                'œ' | 'Œ' if !self.accents => out.push_str("oe"),
                'æ' | 'Æ' if !self.accents => out.push_str("ae"),
                'þ' | 'Þ' if !self.accents => out.push_str("th"),
                'ħ' | 'Ħ' if !self.accents => out.push('h'),
                'ŧ' | 'Ŧ' if !self.accents => out.push('t'),
                c if !self.accents
                    && matches!(c as u32, 0xc0..=0x24f | 0x370..=0x52f | 0x1e00..=0x1fff) =>
                {
                    out.extend(
                        DecomposingNormalizerBorrowed::new_nfd()
                            .normalize_iter([c].into_iter())
                            .filter(|d| !accent(*d))
                            .flat_map(char::to_lowercase),
                    );
                }
                c => out.extend(c.to_lowercase()),
            }
        }
    }

    /// The fold with every character that isn't a letter or digit removed. Removing a fixed
    /// set of characters keeps substrings substrings, so when rendered text holds a query,
    /// the letters of its source hold the query's letters wherever markup inserts only
    /// punctuation. A cheap necessary condition before an exact match over rendered text.
    pub fn letters(self, s: &str) -> String {
        let mut out = self.fold(s);
        out.retain(char::is_alphanumeric);
        out
    }

    pub fn fold_with_map(self, s: &str) -> Folded {
        if s.is_ascii() {
            return Folded {
                text: s.to_ascii_lowercase(),
                spans: Vec::new(),
            };
        }
        let mut text = String::with_capacity(s.len());
        let mut spans = Vec::new();
        for (at, g) in s.grapheme_indices(true) {
            let start = text.len();
            self.push(g, &mut text);
            if start != text.len() {
                spans.push((start, at..at + g.len()));
            }
        }
        Folded { text, spans }
    }
}

#[derive(Debug)]
pub struct Folded {
    pub(crate) text: String,
    spans: Vec<(usize, Range<usize>)>,
}

impl Folded {
    pub fn text(&self) -> &str {
        &self.text
    }

    /// An expansion such as `ß` → `ss` still highlights exactly one source grapheme.
    pub fn source_range(&self, range: Range<usize>) -> Option<Range<usize>> {
        if range.is_empty() || self.text.get(range.clone()).is_none() {
            return None;
        }
        if self.spans.is_empty() {
            return Some(range);
        }
        let a = self.spans.partition_point(|(i, _)| *i <= range.start) - 1;
        let b = self.spans.partition_point(|(i, _)| *i < range.end) - 1;
        Some(self.spans[a].1.start..self.spans[b].1.end)
    }
}
