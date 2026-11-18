//! Anchor matching and scrolling to a changed hunk inside a session's review diff.

use rune::Tx;
use rune_kit::diff::{FileDiff, LineKind, LineRange, LineRef, Side};

use crate::ReviewHost;

/// One line a tool step or navigation target changed: its text on `side`, and its number when known.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Anchor {
    pub side: Side,
    pub text: String,
    pub near: Option<u32>,
}

/// The change's lines in a file's diff: `start..=end` on `side` to select, and `top`, the line
/// to scroll to (a few lines of context above).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Located {
    pub side: Side,
    pub start: u32,
    pub end: u32,
    pub top: u32,
}

/// Finds `anchor` among `fd`'s changed lines: the matching line nearest its number, then the run
/// of changed lines it starts. With no match, its number alone.
pub fn locate(fd: &FileDiff, anchor: &Anchor) -> Option<Located> {
    let kind = if anchor.side == Side::New { LineKind::Added } else { LineKind::Removed };
    let side = anchor.side;
    let mut best: Option<(usize, usize, u32)> = None;
    for exact in [true, false] {
        for (hi, h) in fd.hunks.iter().enumerate() {
            for (li, l) in h.lines.iter().enumerate() {
                let hit = l.kind == kind && if exact { l.text == anchor.text } else { l.text.trim() == anchor.text.trim() };
                let Some(n) = l.number(side).filter(|_| hit) else { continue };
                let far = |n: u32| anchor.near.map_or(0, |a| a.abs_diff(n));
                if best.is_none_or(|(_, _, b)| far(n) < far(b)) {
                    best = Some((hi, li, n));
                }
            }
        }
        if best.is_some() {
            break;
        }
    }
    let Some((hi, li, start)) = best else {
        let n = anchor.near?;
        return Some(Located { side, start: n, end: n, top: n.saturating_sub(3).max(1) });
    };
    let lines = &fd.hunks[hi].lines;
    let end = lines[li..]
        .iter()
        .take_while(|l| l.kind == kind)
        .filter_map(|l| l.number(side))
        .last()
        .unwrap_or(start);
    // Up to three lines above the change, as Files scrolls a line into view.
    let top = lines[..li]
        .iter()
        .rev()
        .filter_map(|l| l.number(side))
        .take(3)
        .last()
        .unwrap_or(start);
    Some(Located { side, start, end, top })
}

/// Scrolls Changes to `path` and selects its change; returns `false` when Changes doesn't list it.
pub fn focus_change(host: &mut dyn ReviewHost, sid: &str, path: &str, anchor: Option<&Anchor>, tx: &mut Tx) -> bool {
    let Some(sr) = host.review_mut().sessions.get_mut(sid) else { return false };
    let Some(file) = sr.diffs.file_index(path) else { return false };
    sr.show(path);
    sr.tree.reveal(path, tx);
    let found = anchor.and_then(|a| locate(&sr.diffs.files()[file], a));
    match found {
        Some(at) => {
            sr.diffs.scroll_to_line(file, LineRef::new(at.side, at.top), tx);
            let (start, end) = (LineRef::new(at.side, at.start), LineRef::new(at.side, at.end));
            sr.diffs.set_selection(Some(LineRange { file, start, end }));
        }
        None => sr.diffs.scroll_to_file(file, tx),
    }
    true
}
