//! The timeline's shell consoles following their shells, as Desktop's shell tool does
//! (`tool-renderer.tsx` `Output`, `shell-output.ts`): while a shell call's console shows and its
//! result doesn't carry the shell's final output (`tools::shell_follow`), its output is read from
//! the server, each second while the shell runs and once more when it stops (`tail`).
//! What was read stays for the last 32 shells, so a console shown again carries on from its
//! cursor, and a shell the server no longer knows is never asked for again. Each showing is a
//! generation of its own, so a read the previous one started lands nowhere.

use super::tail::{self, Page, Tail, TailEffect, TailEvent, Text};

/// Shells whose read output stays once their console is gone (`remember(outputs, …, 32)`).
const KEPT: usize = 32;

/// One shell's follower.
#[derive(Clone, Debug, PartialEq)]
enum Entry {
    /// Its console shows: the follower runs, in its generation.
    Shown { generation: u64, directory: String, tail: Tail },
    /// Its console is gone; what was read, and whether it was read to the end once the shell stopped.
    Kept { text: Text, complete: bool },
    /// The server no longer knows the shell.
    Missing { text: Text },
}

/// A read or wait the caller runs for shell `id`'s follower in `generation`.
#[derive(Clone, Debug, PartialEq)]
pub struct Effect {
    pub id: String,
    pub generation: u64,
    pub directory: String,
    pub effect: TailEffect,
}

#[derive(Default)]
pub struct Followers {
    /// Least recently shown first.
    entries: Vec<(String, Entry)>,
    issued: u64,
    /// Moves whenever any output read changes.
    version: u64,
}

impl Followers {
    /// Changes whenever any shell's output read does.
    pub fn version(&self) -> u64 {
        self.version
    }

    /// The output read from shell `id`, if any.
    pub fn output(&self, id: &str) -> Option<&str> {
        self.entries.iter().find(|(k, _)| k == id).map(|(_, e)| match e {
            Entry::Shown { tail, .. } => tail.text().output.as_str(),
            Entry::Kept { text, .. } | Entry::Missing { text } => text.output.as_str(),
        })
    }

    /// Whether any shell's output is followed: with none and none to show, `sync` does nothing.
    pub fn shows_any(&self) -> bool {
        self.entries.iter().any(|(_, e)| matches!(e, Entry::Shown { .. }))
    }

    /// The consoles showing now, each `(shell, directory, running)`: new ones start following,
    /// gone ones stop, and a shell that started or stopped running tells its follower.
    pub fn sync(&mut self, shown: &[(String, String, bool)]) -> Vec<Effect> {
        let mut out = Vec::new();
        for (id, entry) in &mut self.entries {
            if let Entry::Shown { tail, .. } = entry
                && !shown.iter().any(|(s, ..)| s == id)
            {
                *entry = match tail {
                    Tail::Missing { text } => Entry::Missing { text: std::mem::take(text) },
                    Tail::Complete { text } => Entry::Kept { text: std::mem::take(text), complete: true },
                    other => Entry::Kept { text: other.text().clone(), complete: false },
                };
            }
        }
        for (id, directory, running) in shown {
            let at = self.entries.iter().position(|(k, _)| k == id);
            let entry = at.map(|i| self.entries.remove(i).1);
            let (entry, effects) = match entry {
                Some(Entry::Shown { generation, directory, tail }) => {
                    let (tail, effects) = tail::step(tail, TailEvent::Running(*running));
                    (Entry::Shown { generation, directory, tail }, effects.into_iter().map(|e| (generation, e)).collect())
                }
                Some(Entry::Missing { text }) => (Entry::Missing { text }, Vec::new()),
                kept => {
                    self.issued += 1;
                    self.version += 1;
                    let (tail, effects) = match kept {
                        Some(Entry::Kept { text, complete }) => tail::resume(text, complete, *running),
                        _ => tail::start(*running),
                    };
                    (Entry::Shown { generation: self.issued, directory: directory.clone(), tail }, effects.into_iter().map(|e| (self.issued, e)).collect())
                }
            };
            out.extend(effects.into_iter().map(|(generation, effect)| Effect { id: id.clone(), generation, directory: directory.clone(), effect }));
            self.entries.push((id.clone(), entry));
        }
        // Kept output beyond the last 32 goes, oldest first; missing shells stay known.
        while self.entries.iter().filter(|(_, e)| matches!(e, Entry::Kept { .. })).count() > KEPT {
            let Some(i) = self.entries.iter().position(|(_, e)| matches!(e, Entry::Kept { .. })) else { break };
            self.entries.remove(i);
        }
        out
    }

    /// A read of shell `id` in `generation` landed: a page, `Err(true)` for a shell the server
    /// doesn't know, `Err(false)` for a failure.
    pub fn landed(&mut self, id: &str, generation: u64, attempt: u64, page: Result<Page, bool>) -> Vec<Effect> {
        let event = match page {
            Ok(page) => TailEvent::Read { attempt, page },
            Err(true) => TailEvent::ReadMissing { attempt },
            Err(false) => TailEvent::ReadFailed { attempt },
        };
        self.tell(id, generation, event)
    }

    /// A wait of shell `id`'s follower in `generation` ended.
    pub fn elapsed(&mut self, id: &str, generation: u64, attempt: u64) -> Vec<Effect> {
        self.tell(id, generation, TailEvent::WaitElapsed { attempt })
    }

    fn tell(&mut self, id: &str, generation: u64, event: TailEvent) -> Vec<Effect> {
        let Some((_, Entry::Shown { generation: g, directory, tail })) = self.entries.iter_mut().find(|(k, _)| k == id) else { return Vec::new() };
        if *g != generation {
            return Vec::new();
        }
        let before = tail.text().cursor;
        let (next, effects) = tail::step(std::mem::replace(tail, Tail::Complete { text: Text::default() }), event);
        if next.text().cursor != before {
            self.version += 1;
        }
        *tail = next;
        effects.into_iter().map(|effect| Effect { id: id.to_owned(), generation, directory: directory.clone(), effect }).collect()
    }
}
