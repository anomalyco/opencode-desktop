//! The `/` command popover over the composer, as in Desktop (`suggestions/machine.ts`): the
//! server's commands for the composer's directory, filtered by the draft's first word.

use std::sync::mpsc::{Receiver, Sender, channel};

use rune::{El, Key, KeyEvent, Tx};
use serde_json::Value;

use crate::palette::Pal;
use crate::mentions::list::{Highlight, Row, Spec, data_list, nav_step, popup, step_key};
use crate::{App, Source};

#[derive(Clone, Debug, PartialEq)]
pub struct Command {
    pub name: String,
    pub description: String,
}

pub struct Slash {
    /// `/api/command` for `dir`.
    pub commands: Vec<Command>,
    dir: Option<String>,
    /// The highlighted command and the query it was picked under.
    active: Highlight,
    /// The composer's text when Escape closed the popover.
    pub(crate) closed: Option<String>,
    /// Whether the popover showed after the last edit.
    shown: bool,
    fetched_tx: Sender<(String, Vec<Command>)>,
    fetched: Receiver<(String, Vec<Command>)>,
}

impl Default for Slash {
    fn default() -> Self {
        let (fetched_tx, fetched) = channel();
        Slash { commands: Vec::new(), dir: None, active: Highlight::default(), closed: None, shown: false, fetched_tx, fetched }
    }
}

/// `/api/command`'s `{ data: [{ name, description }] }`, or the fixture's list.
pub fn commands(v: &Value) -> Vec<Command> {
    data_list(v)
        .filter_map(|c| {
            Some(Command { name: c["name"].as_str()?.to_owned(), description: c["description"].as_str().unwrap_or("").to_owned() })
        })
        .collect()
}

/// `parseSlashCommand`: the name after a leading `/` and the trimmed rest.
pub fn parse_slash(text: &str) -> Option<(&str, &str)> {
    let rest = text.strip_prefix('/')?;
    let end = rest.find(char::is_whitespace).unwrap_or(rest.len());
    Some((&rest[..end], rest[end..].trim()))
}

impl App {
    /// The `/query` the command popover filters by: the whole draft is `/` and a word, the
    /// caret at its end.
    pub fn slash_query(&self) -> Option<String> {
        if self.add.shell || !self.composer.focused || self.add.slash.closed.as_deref() == Some(self.composer.text.as_str()) {
            return None;
        }
        let text = &self.composer.text;
        let query = text.strip_prefix('/')?;
        let caret = self.composer.range();
        (caret.0 == caret.1 && caret.1 == text.len() && !query.contains(char::is_whitespace)).then(|| query.to_owned())
    }

    /// After the composer's text changes: the command popover opens, closes and refilters with
    /// this frame.
    pub fn slash_changed(&mut self, tx: &mut Tx) {
        let shown = self.slash_query().is_some();
        if shown || std::mem::replace(&mut self.add.slash.shown, shown) {
            tx.instant();
        }
        self.add.slash.shown = shown;
    }

    pub fn slash_items(&self) -> Vec<&Command> {
        let Some(query) = self.slash_query() else { return Vec::new() };
        if query.is_empty() {
            return self.add.slash.commands.iter().collect();
        }
        let mut scored: Vec<(i64, &Command)> =
            self.add.slash.commands.iter().filter_map(|c| crate::mentions::fuzzy(&query, &c.name).map(|s| (s, c))).collect();
        scored.sort_by_key(|(s, _)| -s);
        scored.into_iter().map(|(_, c)| c).collect()
    }

    pub fn slash_active(&self) -> Option<String> {
        let items = self.slash_items();
        self.add.slash.active.resolve(&self.slash_query().unwrap_or_default(), items.iter().map(|c| c.name.as_str()))
    }

    fn slash_activate(&mut self, name: String) {
        let query = self.slash_query().unwrap_or_default();
        self.add.slash.active.set(name, query);
    }

    /// Replaces the draft with `/name ` (`replaceTrigger`).
    pub fn pick_command(&mut self, name: &str) {
        let len = self.composer.text.len();
        self.composer.set_selection(0, len);
        self.composer.insert(&format!("/{name} "));
        self.add.slash.active.clear();
    }

    pub(crate) fn slash_key(&mut self, ev: &KeyEvent, tx: &mut Tx) -> bool {
        if self.slash_query().is_none() {
            return false;
        }
        if let Some(step) = nav_step(ev, true) {
            if let Some(active) = self.slash_active() {
                let items = self.slash_items();
                let keys = items.iter().map(|c| c.name.as_str());
                if let Some(name) = step_key(keys, Some(&active), step, tx, "commands-scroll", "command-") {
                    self.slash_activate(name);
                }
            }
            return true;
        }
        match ev.key {
            Key::Escape => {
                tx.instant();
                self.add.slash.closed = Some(self.composer.text.clone());
                true
            }
            Key::Tab | Key::Enter if self.composer.preedit.is_none() && !ev.mods.cmd => match self.slash_active() {
                Some(name) => {
                    tx.instant();
                    self.pick_command(&name);
                    true
                }
                None => false,
            },
            _ => false,
        }
    }

    /// Takes in fetched commands, and asks for the composer directory's when a `/` needs them.
    pub fn poll_commands(&mut self) -> bool {
        let mut changed = false;
        for (directory, list) in self.add.slash.fetched.try_iter() {
            if self.add.slash.dir.as_deref() == Some(directory.as_str()) {
                self.add.slash.commands = list;
                changed = true;
            }
        }
        if self.slash_query().is_none() {
            return changed;
        }
        let Source::Live { server, .. } = &self.source else { return changed };
        let directory = self.composer_directory();
        if self.add.slash.dir.as_deref() == Some(directory.as_str()) {
            return changed;
        }
        self.add.slash.dir = Some(directory.clone());
        let (server, wake, out) = (server.clone(), self.waker.clone(), self.add.slash.fetched_tx.clone());
        std::thread::spawn(move || {
            let path = format!("/api/command?location%5Bdirectory%5D={}", crate::switcher::encode(&directory));
            let list = server.get(&path).map(|v| commands(&v)).unwrap_or_default();
            let _ = out.send((directory, list));
            if let Some(w) = wake {
                w();
            }
        });
        changed
    }

    /// A `/name args` draft naming a known command: the name and the arguments.
    pub fn slash_command(&self, text: &str) -> Option<(String, String)> {
        let (name, args) = parse_slash(text)?;
        self.add.slash.commands.iter().any(|c| c.name == name).then(|| (name.to_owned(), args.to_owned()))
    }
}

/// The `/` popover over the composer: each command's `/name` and description.
pub fn commands_view(s: &App, p: &Pal, width: f32) -> Option<El> {
    s.slash_query()?;
    let items = s.slash_items();
    let active = s.slash_active();
    let rows = items
        .iter()
        .map(|c| Row { id: c.name.clone(), key: format!("command-{}", c.name), label: format!("/{}", c.name), lead: None, description: c.description.clone() })
        .collect();
    let spec = Spec { key: "commands", label: "Commands", empty: "No matching commands", width };
    Some(popup(s.zoom, p, spec, rows, active.as_deref(), |s, name| s.slash_activate(name.to_owned()), |s, name| s.pick_command(name)))
}
