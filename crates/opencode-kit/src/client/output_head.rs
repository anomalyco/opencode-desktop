//! A legacy server puts result text inside its JSON page. Only offer a prefix if the JSON
//! structure identifies it as a tool's text; the page could instead contain a prompt, an
//! image, or a different message. Never search for a field name inside arbitrary strings.

use serde::{Deserialize, de::IgnoredAny};
use std::sync::Arc;

use crate::wire_error::WireKind;

const HEAD: usize = 2048;
const PATH: &[&str] = &[
    "data", "[]", "content", "[]", "state", "content", "[]", "text",
];

/// Look near the beginning of an interrupted page. Use serde for complete tokens (including
/// escaped object keys). An incomplete sequence inside a string contributes no decoded bytes.
pub(super) fn partial(bytes: &[u8]) -> Option<WireKind> {
    let start = Cursor {
        bytes: &bytes[..bytes.len().min(64 << 10)],
        pos: 0,
    }
    .locate(PATH)?;
    // A lower bound need not count the whole discarded range. Check first that the token
    // actually crosses the cut; reading the first few KiB of a *complete* item isn't proof.
    let mut de = serde_json::Deserializer::from_slice(&bytes[start..]);
    if !IgnoredAny::deserialize(&mut de).is_err_and(|e| e.is_eof()) {
        return None;
    }
    let raw = &bytes[start + 1..];
    let mut saved = String::new();
    let mut total = 0u64;
    let mut pos = 0;
    while pos < raw.len() {
        let n = raw[pos..]
            .iter()
            .position(|&b| b < 32 || matches!(b, b'"' | b'\\'))
            .unwrap_or(raw.len() - pos);
        if n > 0 {
            let slice = &raw[pos..pos + n];
            let plain = match std::str::from_utf8(slice) {
                Ok(s) => s,
                Err(e) if e.error_len().is_none() && pos + n == raw.len() => {
                    std::str::from_utf8(&slice[..e.valid_up_to()]).ok()?
                }
                Err(_) => return None,
            };
            append(&mut saved, &mut total, plain);
            pos += n;
        }
        match raw.get(pos) {
            None => break,
            Some(b'\\') => {
                let Some(&esc) = raw.get(pos + 1) else { break };
                let mut width = if esc == b'u' { 6 } else { 2 };
                if width == 6
                    && raw
                        .get(pos + 2..pos + 6)
                        .and_then(|b| std::str::from_utf8(b).ok())
                        .and_then(|s| u16::from_str_radix(s, 16).ok())
                        .is_some_and(|u| (0xd800..=0xdbff).contains(&u))
                {
                    width = 12;
                }
                let Some(s) = raw.get(pos..pos + width) else {
                    break;
                };
                let mut quoted = [0u8; 14];
                quoted[0] = b'"';
                quoted[1..=width].copy_from_slice(s);
                quoted[width + 1] = b'"';
                let decoded = match esc {
                    b'n' => "\n",
                    b'r' => "\r",
                    b't' => "\t",
                    b'/' => "/",
                    b'\\' => "\\",
                    b'"' => "\"",
                    b'b' => "\x08",
                    b'f' => "\x0c",
                    b'u' => {
                        let decoded: String = serde_json::from_slice(&quoted[..width + 2]).ok()?;
                        append(&mut saved, &mut total, &decoded);
                        pos += width;
                        continue;
                    }
                    _ => return None,
                };
                append(&mut saved, &mut total, decoded);
                pos += width;
            }
            _ => return None,
        }
    }
    (!saved.is_empty()).then(|| WireKind::PartialOutput {
        head: Arc::from(saved),
        at_least: total,
    })
}

fn append(head: &mut String, total: &mut u64, s: &str) {
    let n = s.floor_char_boundary(HEAD.saturating_sub(*total as usize).min(s.len()));
    head.push_str(&s[..n]);
    *total += s.len() as u64;
}

struct Cursor<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl Cursor<'_> {
    fn peek(&mut self) -> Option<u8> {
        while self.bytes.get(self.pos)?.is_ascii_whitespace() {
            self.pos += 1;
        }
        self.bytes.get(self.pos).copied()
    }
    fn eat(&mut self, b: u8) -> bool {
        if self.peek() == Some(b) {
            self.pos += 1;
            true
        } else {
            false
        }
    }
    fn locate(&mut self, path: &[&str]) -> Option<usize> {
        if self.pos >= 64 << 10 {
            return None;
        }
        let Some((&key, rest)) = path.split_first() else {
            if self.peek() != Some(b'"') {
                return None;
            }
            // Pick only the unfinished field at the end of the bounded search window.
            return serde_json::from_slice::<IgnoredAny>(&self.bytes[self.pos..])
                .is_err_and(|e| e.is_eof())
                .then_some(self.pos);
        };
        let array = key == "[]";
        if !self.eat(if array { b'[' } else { b'{' }) {
            return None;
        }
        let mut kind = None;
        loop {
            let name = if array {
                None
            } else {
                let mut it = serde_json::Deserializer::from_slice(&self.bytes[self.pos..])
                    .into_iter::<String>();
                let name = it.next()?.ok()?;
                self.pos += it.byte_offset();
                if !self.eat(b':') {
                    return None;
                }
                Some(name)
            };
            self.peek()?;
            let at = self.pos;
            if name.as_deref() == Some("type") {
                let mut de = serde_json::Deserializer::from_slice(&self.bytes[at..]);
                kind = <&str>::deserialize(&mut de).ok();
            }
            let appropriate = match path {
                ["state", ..] => kind == Some("tool"),
                ["text"] => kind == Some("text"),
                _ => true,
            };
            if (array || name.as_deref() == Some(key))
                && appropriate
                && let Some(hit) = self.locate(rest)
            {
                return Some(hit);
            }
            let mut it =
                serde_json::Deserializer::from_slice(&self.bytes[at..]).into_iter::<IgnoredAny>();
            it.next()?.ok()?;
            self.pos = at + it.byte_offset();
            if !self.eat(b',') {
                return None;
            }
        }
    }
}
