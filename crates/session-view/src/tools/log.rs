//! A console only shapes the text its reader can see. A short block of adjacent lines is one
//! virtual row so even a log made entirely of newlines doesn't need a key per byte. Split a
//! long physical line too: a minified result with no newlines must never reach a single shaper.

use std::{borrow::Cow, rc::Rc};

use rune::{Color, El, GrowthSpec, TextStyle, col, text};

use crate::{TimelineHost, palette::Pal, style::readable};

pub const INLINE: usize = 2 << 20;
// An oversized result is a preview card, not a second scroller. One real line from each
// end fits beside the explanation and action; even a single minified line stays bounded.
const EDGE: usize = 256;

pub enum Excerpt<'a> {
    Whole(&'a str),
    Ends {
        first: &'a str,
        last: &'a str,
        bytes: usize,
    },
}

impl<'a> Excerpt<'a> {
    pub fn new(raw: &'a str) -> Self {
        if raw.len() <= INLINE {
            return Self::Whole(raw);
        }
        let head = &raw[..raw.floor_char_boundary(EDGE)];
        let tail = &raw[raw.ceil_char_boundary(raw.len() - EDGE)..];
        let first = head.split_once('\n').map_or(head, |(line, _)| line);
        let tail = tail.strip_suffix('\n').unwrap_or(tail);
        let last = tail.rsplit_once('\n').map_or(tail, |(_, line)| line);
        Self::Ends {
            first,
            last,
            bytes: raw.len(),
        }
    }
}

/// `visible` belongs under a scroll viewport. Text, links, wrapping and selection use the
/// existing short-output renderer. Only a user scroll changes which chunks are materialised.
pub fn visible<S: TimelineHost>(
    p: &Pal,
    raw: &str,
    font: TextStyle,
    ink: Color,
    linked: bool,
    height: f32,
) -> El {
    let show = move |p: &Pal, line: &str| {
        let line = if linked {
            super::plain::clean(line)
        } else {
            Cow::Borrowed(line)
        };
        let el = if linked {
            super::plain::linked::<S>(p, &line, font, ink)
        } else {
            text(line.into_owned())
                .font(font)
                .color(ink)
                .wrap_anywhere()
        };
        readable(p, el).w_full().min_h(height)
    };
    let raw = raw.strip_suffix('\n').unwrap_or(raw);
    if raw.len() < 2048 && raw.bytes().filter(|&b| b == b'\n').take(16).count() < 16 {
        return show(p, raw);
    }
    let mut ends = Vec::<u32>::new();
    let mut start = 0;
    while start < raw.len() {
        let cut = raw.floor_char_boundary((start + 1024).min(raw.len()));
        let part = &raw[start..cut];
        // Keep logical lines together unless one line alone exceeds the shaping budget.
        let mut lf = part.match_indices('\n').take(4).map(|(i, _)| i + 1);
        let n = lf
            .by_ref()
            .nth(3)
            .or_else(|| {
                (cut < raw.len())
                    .then(|| part.rfind('\n').map(|i| i + 1))
                    .flatten()
            })
            .unwrap_or(part.len());
        start += n;
        ends.push(start as u32);
    }
    let (raw, p) = (Rc::<str>::from(raw), *p);
    col()
        .w_full()
        .key("log-rows")
        .growth(GrowthSpec::default())
        .virtual_rows(
            ends.len(),
            height * 4.0,
            |_: &S, i| Cow::Owned(i.to_string()),
            move |_: &S, i| {
                let begin = if i == 0 { 0 } else { ends[i - 1] as usize };
                let slice = &raw[begin..ends[i] as usize];
                show(&p, slice.strip_suffix('\n').unwrap_or(slice))
            },
        )
        .virtual_rows_version(0)
}
