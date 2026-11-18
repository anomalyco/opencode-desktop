//! URL query/path percent-encoding, pure list-highlight state machine, and generic upsert helpers.

const HEX: &[u8; 16] = b"0123456789ABCDEF";

/// Percent-encodes a URL query parameter value (RFC 3986 unreserved bytes pass through; all other
/// UTF-8 bytes become `%XX` uppercase hex). Matches `switcher::encode` byte-for-byte.
pub fn percent_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char);
            }
            _ => {
                out.push('%');
                out.push(HEX[(b >> 4) as usize] as char);
                out.push(HEX[(b & 0x0F) as usize] as char);
            }
        }
    }
    out
}

/// Encodes each non-empty `/`-separated segment of a workspace-relative path for `/api/fs/read/...`.
/// Matches `review::data::encode_fs_path` byte-for-byte.
pub fn encode_fs_path(path: &str) -> String {
    path.split('/')
        .filter(|seg| !seg.is_empty())
        .map(percent_encode)
        .collect::<Vec<_>>()
        .join("/")
}

/// The lit row of a list under a search field, as Desktop's typeahead keeps it
/// (`ui/src/forms/typeahead/controller.ts:14-22`, `ui/src/hooks/list-machine.ts:20-21`): until
/// a row is chosen the first row is lit; a chosen row holds while it's listed, and a new query
/// starts from the first row again (`search-list.tsx:261-265`), whatever was chosen before.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Highlight {
    #[default]
    First,
    Chosen {
        key: String,
        by: Lit,
    },
}

/// What chose the lit row (`ListHighlight.mode`): the pointer's row never scrolls the list
/// (`listScrollTarget`), and late results leave the list where it is for it (`search-list.tsx:189-207`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Lit {
    Keys,
    Pointer,
}

pub enum HighlightEvent<'a> {
    /// The field's text changed (`onInput`: `choose(undefined)`).
    QueryChanged,
    /// The arrows, a section jump or the host chose `key` (`setActive`).
    Keys(String),
    /// The pointer moved on `key` (`onMouseMove` with movement).
    Pointer(String),
    /// The list now holds `keys`: a chosen row that left it lets the first row light
    /// (`controller.ts:17-20`).
    Rows(&'a [&'a str]),
    /// The list opened, or the host starts it over.
    Reset,
}

impl Highlight {
    pub fn step(&mut self, event: HighlightEvent<'_>) {
        *self = match (std::mem::take(self), event) {
            (_, HighlightEvent::QueryChanged | HighlightEvent::Reset) => Highlight::First,
            (_, HighlightEvent::Keys(key)) => Highlight::Chosen { key, by: Lit::Keys },
            (_, HighlightEvent::Pointer(key)) => Highlight::Chosen {
                key,
                by: Lit::Pointer,
            },
            (Highlight::Chosen { key, by }, HighlightEvent::Rows(keys))
                if keys.contains(&key.as_str()) =>
            {
                Highlight::Chosen { key, by }
            }
            (_, HighlightEvent::Rows(_)) => Highlight::First,
        };
    }

    /// The lit row among `keys`: the chosen one while it's listed, else the first.
    pub fn resolve<'a>(&self, keys: impl Iterator<Item = &'a str> + Clone) -> Option<String> {
        let chosen = match self {
            Highlight::Chosen { key, .. } => keys.clone().find(|k| k == key),
            Highlight::First => None,
        };
        chosen.or_else(|| keys.clone().next()).map(str::to_owned)
    }

    /// Whether the pointer chose `key`.
    pub fn pointed(&self, key: &str) -> bool {
        matches!(self, Highlight::Chosen { key: k, by: Lit::Pointer } if k == key)
    }

    /// The keys chose `key`, as [`HighlightEvent::Keys`].
    pub fn set(&mut self, key: String) {
        self.step(HighlightEvent::Keys(key));
    }

    /// Back to the first row, as [`HighlightEvent::Reset`].
    pub fn clear(&mut self) {
        self.step(HighlightEvent::Reset);
    }
}

/// The index `step` from `at` (the first when none), wrapping around `len` items (0 when empty).
pub fn cycle(len: usize, at: Option<usize>, step: i64) -> usize {
    if len == 0 {
        return 0;
    }
    (at.unwrap_or(0) as i64 + step).rem_euclid(len as i64) as usize
}

/// Merges `incoming` items into `into` in place, replacing an existing item matched by `same_key`
/// whenever `version(&incoming) >= version(&existing)` and appending unmatched items.
///
/// Headless, zero-dep generalization of `upsert_sessions`:
/// `upsert_latest(into, incoming, |a, b| a.id == b.id, |s| s.updated)`.
pub fn upsert_latest<T, V: Ord>(
    into: &mut Vec<T>,
    incoming: impl IntoIterator<Item = T>,
    mut same_key: impl FnMut(&T, &T) -> bool,
    mut version: impl FnMut(&T) -> V,
) {
    for item in incoming {
        match into.iter_mut().find(|old| same_key(old, &item)) {
            Some(old) if version(&item) >= version(old) => *old = item,
            Some(_) => {}
            None => into.push(item),
        }
    }
}
