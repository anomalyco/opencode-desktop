//! A cell's text without a heap allocation for nearly every cell ([`Grapheme`]).

use std::sync::Arc;

/// The grapheme cluster a cell shows: up to 15 bytes of UTF-8 inline (every scalar value, and
/// most combining and emoji sequences), a longer one (a family emoji, stacked marks) shared.
/// Extracting a full 200x60 screen allocated a `String` for each of its 12,000 cells.
#[derive(Clone, PartialEq, Eq, Hash)]
pub enum Grapheme {
    Inline { len: u8, bytes: [u8; 15] },
    Long(Arc<str>),
}

impl Grapheme {
    pub const EMPTY: Grapheme = Grapheme::Inline { len: 0, bytes: [0; 15] };

    pub fn as_str(&self) -> &str {
        match self {
            Grapheme::Inline { len, bytes } => std::str::from_utf8(&bytes[..usize::from(*len)]).unwrap_or_default(),
            Grapheme::Long(s) => s,
        }
    }

    /// The cluster of `chars`, as libghostty-vt hands a cell's codepoints over.
    pub fn from_chars(chars: impl IntoIterator<Item = char>) -> Grapheme {
        let mut bytes = [0u8; 15];
        let mut len = 0usize;
        let mut chars = chars.into_iter();
        for ch in chars.by_ref() {
            let n = ch.len_utf8();
            if len + n > bytes.len() {
                let mut s = String::from(std::str::from_utf8(&bytes[..len]).unwrap_or_default());
                s.push(ch);
                s.extend(chars);
                return Grapheme::Long(s.into());
            }
            ch.encode_utf8(&mut bytes[len..]);
            len += n;
        }
        Grapheme::Inline { len: len as u8, bytes }
    }
}

impl Default for Grapheme {
    fn default() -> Self {
        Grapheme::EMPTY
    }
}

impl std::ops::Deref for Grapheme {
    type Target = str;
    fn deref(&self) -> &str {
        self.as_str()
    }
}

impl std::fmt::Debug for Grapheme {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.as_str().fmt(f)
    }
}

impl std::fmt::Display for Grapheme {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl PartialEq<str> for Grapheme {
    fn eq(&self, other: &str) -> bool {
        self.as_str() == other
    }
}

impl PartialEq<&str> for Grapheme {
    fn eq(&self, other: &&str) -> bool {
        self.as_str() == *other
    }
}

impl From<&str> for Grapheme {
    fn from(s: &str) -> Self {
        Grapheme::from_chars(s.chars())
    }
}
