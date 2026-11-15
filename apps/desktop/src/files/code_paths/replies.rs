//! Reply-path checks are reads: discovering a path during a view starts work, but isn't a
//! reason to run the view again. Only an answer that turns plain code into a link is one.
use std::{cell::{Cell, RefCell}, collections::{HashMap, HashSet}, rc::Rc, sync::mpsc::{Receiver, Sender, channel}};
use opencode_kit::client::Server;
use rune::{Waker, time::Instant};
use super::{FOUND_FOR, MISSING_FOR, reply_path_exists};

enum Check { Checking { attempt: u64, was_link: bool }, Answer { exists: bool, at: Instant } }

#[derive(Default)]
struct Paths { entries: HashMap<String, Check>, rev: u64 }

struct Answer { sid: String, attempt: u64, entries: Vec<(String, bool)> }

pub(in crate::files) struct Replies {
    sessions: RefCell<HashMap<String, Paths>>,
    attempt: Cell<u64>,
    out: Sender<Answer>,
    answers: Receiver<Answer>,
}

impl Default for Replies {
    fn default() -> Self {
        let (out, answers) = channel();
        Self { sessions: Default::default(), attempt: Cell::new(0), out, answers }
    }
}

impl Replies {
    pub(in crate::files) fn retain(&self, keep: impl Fn(&str) -> bool) { self.sessions.borrow_mut().retain(|sid, _| keep(sid)); }
    pub(in crate::files) fn version(&self, sid: &str) -> u64 { self.sessions.borrow().get(sid).map_or(0, |p| p.rev) }

    pub(super) fn read(&self, sid: &str, codes: Vec<String>, server: &Server, directory: impl FnOnce() -> String, wake: Option<&Waker>) -> Option<Rc<HashSet<String>>> {
        let now = Instant::now();
        let attempt = self.attempt.get().wrapping_add(1);
        let mut ask = Vec::new();
        let mut found = HashSet::new();
        let mut sessions = self.sessions.borrow_mut();
        if !sessions.contains_key(sid) { sessions.insert(sid.to_owned(), Paths::default()); }
        let paths = sessions.get_mut(sid).unwrap();
        for code in codes {
            match paths.entries.get(&code) {
                Some(Check::Checking { .. }) => continue,
                Some(Check::Answer { exists, at }) if now.duration_since(*at) < if *exists { FOUND_FOR } else { MISSING_FOR } => {
                    if *exists { found.insert(code); }
                }
                stale => {
                    let was_link = matches!(stale, Some(Check::Answer { exists: true, .. }));
                    paths.entries.insert(code.clone(), Check::Checking { attempt, was_link });
                    ask.push(code);
                }
            }
        }
        if !ask.is_empty() {
            self.attempt.set(attempt);
            let (out, wake, server, directory, session) = (self.out.clone(), wake.cloned(), server.clone(), directory(), sid.to_owned());
            let started = rune::task::spawn("reply-code-links", move || {
                let entries = ask.into_iter().map(|code| { let exists = reply_path_exists(&server, &directory, &code); (code, exists) }).collect();
                if out.send(Answer { sid: session, attempt, entries }).is_ok() && let Some(wake) = wake { wake(); }
            });
            if started.is_err() { paths.entries.retain(|_, c| !matches!(c, Check::Checking { attempt: a, .. } if *a == attempt)); }
        }
        (!found.is_empty()).then(|| Rc::new(found))
    }

    pub(in crate::files) fn land(&self, shown: Option<&str>) -> bool {
        let mut changed = false;
        for answer in self.answers.try_iter() {
            let mut sessions = self.sessions.borrow_mut();
            let Some(paths) = sessions.get_mut(&answer.sid) else { continue };
            let mut links = false;
            let now = Instant::now();
            for (code, exists) in answer.entries {
                let Some(state) = paths.entries.get_mut(&code) else { continue };
                let Check::Checking { attempt, was_link } = state else { continue };
                if *attempt != answer.attempt { continue; }
                // Another retained row may still draw the previous positive answer. A negative
                // revalidation must remove that link even if this particular row drew plain code.
                links |= exists || *was_link;
                *state = Check::Answer { exists, at: now };
            }
            if links { paths.rev = paths.rev.wrapping_add(1); changed |= shown == Some(&answer.sid); }
        }
        changed
    }
}
