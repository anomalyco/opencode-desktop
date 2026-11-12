//! Finished commands from the shell's OSC 133 marks (`A` prompt, `C` output starts, `D;<exit>`
//! done): each one's exit status and how long it ran, kept on its prompt's row.
//!
//! The emulator jumps between prompts by these marks itself but keeps no exit status, so the
//! output is scanned for them here, and [`crate::core::VtCore::feed`] stops at each so the
//! prompt's row is the cursor's when `A` arrives. Rows are known by the emulator's row id; a mark
//! also keeps the text of the row it first showed on, so an id reused for other text loses it.

use std::collections::{HashMap, VecDeque};

use rune::time::Instant;

/// Marks kept, oldest dropped first.
const KEPT: usize = 2_000;
/// The longest mark body read (`D;<exit>;aid=...`); longer ones are cut there.
const BODY_MAX: usize = 64;
const PREFIX: &[u8] = b"\x1b]133;";

/// A command that finished: its exit code (when the shell said it) and its run time.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CommandMark {
    pub exit: Option<i32>,
    pub ms: u32,
}

#[derive(Default)]
pub(crate) struct Marks {
    scan: Scan,
    prompt: Option<u128>,
    started: Option<Instant>,
    by_row: HashMap<u128, (CommandMark, Option<u64>)>,
    order: VecDeque<u128>,
}

impl Marks {
    /// The end of the next OSC 133 sequence in `bytes` (past its terminator) and its body.
    pub(crate) fn next(&mut self, bytes: &[u8]) -> Option<(usize, Vec<u8>)> {
        self.scan.next(bytes)
    }

    /// Takes mark `body`; `prompt_row` is the cursor's row id when it's a prompt's start.
    pub(crate) fn step(&mut self, body: &[u8], prompt_row: Option<u128>) {
        match body.first() {
            Some(b'A') => (self.prompt, self.started) = (prompt_row, None),
            Some(b'C') => self.started = Some(Instant::now()),
            Some(b'D') => {
                // A prompt left with no command (Enter alone) ends with no `C`: nothing ran.
                let (Some(row), Some(at)) = (self.prompt.take(), self.started.take()) else { return };
                let exit = std::str::from_utf8(body).ok().and_then(|b| b.split(';').nth(1)).and_then(|c| c.trim().parse().ok());
                let ms = at.elapsed().as_millis().min(u128::from(u32::MAX)) as u32;
                if self.by_row.insert(row, (CommandMark { exit, ms }, None)).is_none() {
                    self.order.push_back(row);
                }
                while self.order.len() > KEPT {
                    if let Some(old) = self.order.pop_front() {
                        self.by_row.remove(&old);
                    }
                }
            }
            _ => {}
        }
    }

    /// Row `id`'s mark, if it has one and its text (`hash`) is the text it was made on.
    pub(crate) fn on_row(&mut self, id: u128, hash: u64) -> Option<CommandMark> {
        let (mark, seen) = self.by_row.get_mut(&id)?;
        match seen {
            Some(h) if *h != hash => None,
            Some(_) => Some(*mark),
            None => {
                *seen = Some(hash);
                Some(*mark)
            }
        }
    }
}

/// Finds `ESC ] 133 ; <body> (BEL | ESC \)` across chunk boundaries, skipping to each ESC.
#[derive(Default)]
struct Scan {
    matched: usize,
    body: Option<Vec<u8>>,
    esc: bool,
}

impl Scan {
    fn next(&mut self, bytes: &[u8]) -> Option<(usize, Vec<u8>)> {
        let mut i = 0;
        while i < bytes.len() {
            if self.body.is_none() && self.matched == 0 {
                i += memchr::memchr(0x1b, &bytes[i..])?;
            }
            let b = bytes[i];
            i += 1;
            if let Some(body) = &mut self.body {
                if b == 0x07 || (self.esc && b == b'\\') {
                    self.esc = false;
                    return self.body.take().map(|body| (i, body));
                }
                if self.esc {
                    // ESC then something else: not a terminator, so not a mark.
                    (self.body, self.esc, self.matched) = (None, false, usize::from(b == 0x1b));
                    continue;
                }
                if b == 0x1b {
                    self.esc = true;
                } else if body.len() < BODY_MAX {
                    body.push(b);
                }
                continue;
            }
            if b == PREFIX[self.matched] {
                self.matched += 1;
                if self.matched == PREFIX.len() {
                    (self.matched, self.body) = (0, Some(Vec::new()));
                }
            } else {
                self.matched = usize::from(b == 0x1b);
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bodies(chunks: &[&[u8]]) -> Vec<String> {
        let mut m = Marks::default();
        let mut out = Vec::new();
        for chunk in chunks {
            let mut rest = *chunk;
            while let Some((end, body)) = m.next(rest) {
                out.push(String::from_utf8(body).unwrap());
                rest = &rest[end..];
            }
        }
        out
    }

    #[test]
    fn finds_marks_with_either_terminator_and_split_across_reads() {
        assert_eq!(bodies(&[b"x\x1b]133;A\x07$ ls\r\n\x1b]133;C\x1b\\out\x1b]13", b"3;D;2\x07"]), ["A", "C", "D;2"]);
    }

    #[test]
    fn other_sequences_are_not_marks() {
        assert_eq!(bodies(&[b"\x1b]0;title\x07\x1b[31mred\x1b]133;Bx\x1bq\x1b]133;D\x07"]), ["D"]);
    }

    #[test]
    fn a_command_is_marked_on_its_prompt_with_its_exit() {
        let mut m = Marks::default();
        m.step(b"A", Some(7));
        m.step(b"C", None);
        m.step(b"D;1", None);
        assert_eq!(m.on_row(7, 42).map(|k| k.exit), Some(Some(1)));
        assert_eq!(m.on_row(7, 43), None, "the row's id now holds other text");
        m.step(b"A", Some(8));
        m.step(b"D;0", None);
        assert_eq!(m.on_row(8, 1), None, "Enter alone ran nothing");
    }
}
