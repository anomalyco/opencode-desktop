//! A streaming reply's words each fade in once: text already shown never shows again from
//! nothing, whatever arrives after it.

use std::collections::HashMap;

use rune::{Env, Runtime};

use crate::story::{Host, poll, start_motion, stream, view};

const SUMMARY: &str = "## Objective\n- Make the deck the place Alex rehearses tonight's talk, with live editing and branch comparison.\n- Use the server and the particle demo as real workloads.\n\n## Relevant Files\n- `/work/deck/arena/combined.md`: talk script and must-teach list; later decisions replace Breakout.\n- `/work/deck/tui-deck.md`: original interactive deck, instruments, branch and scrub concepts.\n- `/work/deck/research/`: overnight reports incl. `ideas-map.md`, `lang.md`, `safety.md`, and fresh audits.\n- `/work/next/docs/research/dragging.md`: local research on dragging, `closest` / `between` / `vary`, and ranked adaptations.\n- `/work/deck-live/`: interpreter host, provenance/editor/live swap foundation; branch `deck-branch-compare` adds A/B.\n- `/work/deck-scrub-1/`: scrubber, single host wall clock, navigation author's workspace; latest fix on `deck-scrub-fix-1`.\n- `/work/next/`: worktree family anchor; use `git show next:<path>` for the current branch because the index can lag.\n\n1. First numbered step with **bold** words and a [link](https://example.com/a).\n2. Second step, *emphasis* across it.\n\nA closing paragraph that runs long enough to wrap onto a second line in the timeline's width, so a wrap is part of what streams.\n";

fn runtime() -> Runtime<Host> {
    let mut host = Host::default();
    start_motion(&mut host);
    let mut rt = Runtime::new(host, view, (900.0, 1400.0), Env::new(true, 1.0));
    rt.poll = Some(poll);
    rt.frame(0.0);
    for _ in 0..60 {
        rt.frame(1.0 / 60.0);
    }
    rt
}

/// Every text in the streaming reply's row, by path.
fn texts(rt: &Runtime<Host>) -> HashMap<String, String> {
    rt.elements()
        .into_iter()
        .filter(|e| e.path.contains("t-msg_m2"))
        .filter_map(|e| Some((e.path, e.text?)))
        .collect()
}

/// Streams `src` in `chunk`-byte pieces, `gap` frames apart, and lists every text that came back
/// from nothing (a new path showing more than the piece that just arrived) or lost its start.
fn restarts(src: &str, chunk: usize, gap: usize) -> Vec<String> {
    let mut rt = runtime();
    let mut seen = texts(&rt);
    let mut out = Vec::new();
    let mut at = 0;
    while at < src.len() {
        let mut end = (at + chunk).min(src.len());
        while !src.is_char_boundary(end) {
            end += 1;
        }
        let more = src[at..end].to_owned();
        at = end;
        rt.update(move |s, _| stream(s, &more));
        for _ in 0..gap {
            rt.frame(1.0 / 60.0);
        }
        let now = texts(&rt);
        for (path, text) in &now {
            match seen.get(path) {
                // A code span's padding isn't in the source.
                None if text.replace('\u{202f}', "").trim().len() > chunk => out.push(format!("new {path}: {text:?}")),
                Some(old) if !text.starts_with(old.as_str()) && !old.starts_with(text.as_str()) => {
                    out.push(format!("changed {path}: {old:?} -> {text:?}"))
                }
                _ => {}
            }
        }
        seen = now;
    }
    out
}

#[test]
fn streamed_text_never_shows_again_from_nothing() {
    for (chunk, gap) in [(7, 1), (24, 2), (80, 3)] {
        let r = restarts(SUMMARY, chunk, gap);
        assert!(r.is_empty(), "chunk {chunk}: {} restarts:\n{}", r.len(), r.iter().take(20).cloned().collect::<Vec<_>>().join("\n"));
    }
}
