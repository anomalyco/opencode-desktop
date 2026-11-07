//! The `/` command popover over the composer, as in Desktop (`suggestions/machine.ts`,
//! `composer/model.ts`'s `slashCommands`): the server's commands for the composer's directory,
//! then the app's own (`commands.rs`, those with a `/name`), filtered by the draft's first word.
//! Picking an app command runs it at once; a server command fills in `/name `.

use rune::{Cx, El, Key, KeyEvent, Latest, Tx, Waker};
use serde_json::Value;

use crate::palette::Pal;
use crate::mentions::list::{Highlight, Row, Spec, data_list, nav_step, popup, step_key};
use crate::{App, Source};

#[derive(Clone, Debug, PartialEq)]
pub struct Command {
    pub name: String,
    pub description: String,
}

/// The app's `/` commands in the order Desktop registers them: the session's
/// (`use-session-commands.tsx:445-500`), Open file (`:515-522`), the terminal plugin after it
/// (`gui-plugins/src/terminal/renderer.tsx:90`), MCPs (`:655-664`), then the composer's
/// (`composer/commands.tsx:109-162`) and BTW (`gui-plugins/src/btw/renderer.tsx:47`).
const ORDER: [&str; 14] = ["new", "undo", "redo", "compact", "fork", "export", "open", "terminal", "mcp", "model", "connect", "reload", "agent", "btw"];

/// One row of the popover: a server command, or an app command (`builtin`, its id).
#[derive(Clone, Debug, PartialEq)]
pub struct Item {
    /// The row's id: the server command's name, or `builtin.<id>`.
    pub id: String,
    pub name: String,
    /// What the query matches besides the name: an app command's title.
    pub title: String,
    pub description: String,
    pub keys: Option<crate::keymap::Bind>,
    pub builtin: Option<&'static str>,
}

#[derive(Default)]
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
    fetch: Latest<(String, Vec<Command>)>,
}

impl Slash {
    pub fn set_waker(&mut self, waker: Waker) {
        self.fetch = Latest::new(waker);
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

    /// Every `/` command, as Desktop lists them (`composer/model.ts:261-291`): the server's, then
    /// the app's in Desktop's order ([`ORDER`]), each with its description only (none for
    /// `/new` and `/terminal`) and its keys.
    pub fn slash_list(&self) -> Vec<Item> {
        let server = self.add.slash.commands.iter().map(|c| Item {
            id: c.name.clone(),
            name: c.name.clone(),
            title: c.name.clone(),
            description: c.description.clone(),
            keys: None,
            builtin: None,
        });
        let mut own: Vec<Item> = self.commands().into_iter().filter_map(|c| {
            let name = c.slash?;
            // Desktop's `/new` is `session.new`, which has no keys of its own (`use-session-commands.tsx:446-451`).
            let keys = c.keys.filter(|_| name != "new");
            Some(Item { id: format!("builtin.{}", c.id), name: name.into(), title: c.title.into(), description: c.description.into(), keys, builtin: Some(c.id) })
        }).collect();
        own.sort_by_key(|c| ORDER.iter().position(|n| *n == c.name).unwrap_or(ORDER.len()));
        server.chain(own).collect()
    }

    /// The commands the query finds by name or title (`editor/interaction.ts:336-340`), best first.
    pub fn slash_items(&self) -> Vec<Item> {
        let Some(query) = self.slash_query() else { return Vec::new() };
        let needle = query.to_lowercase();
        let mut scored: Vec<(i64, Item)> = self.slash_list().into_iter()
            .filter_map(|c| Some((crate::mentions::fuzzy(&needle, &c.name).max(crate::mentions::fuzzy(&needle, &c.title))?, c)))
            .collect();
        scored.sort_by_key(|(s, _)| -s);
        scored.into_iter().map(|(_, c)| c).collect()
    }

    pub fn slash_active(&self) -> Option<String> {
        let items = self.slash_items();
        self.add.slash.active.resolve(&self.slash_query().unwrap_or_default(), items.iter().map(|c| c.id.as_str()))
    }

    fn slash_activate(&mut self, name: String) {
        let query = self.slash_query().unwrap_or_default();
        self.add.slash.active.set(name, query);
    }

    /// A server command replaces the draft with `/name ` (`replaceTrigger`); an app command
    /// clears it and runs (`command.trigger(id, "slash")`).
    pub fn pick_command(&mut self, id: &str, tx: &mut Tx) {
        let Some(item) = self.slash_list().into_iter().find(|c| c.id == id) else { return };
        let len = self.composer.text.len();
        self.composer.set_selection(0, len);
        self.add.slash.active.clear();
        match item.builtin {
            Some(command) => {
                self.composer.insert("");
                self.run_command(command, tx);
            }
            None => self.composer.insert(&format!("/{} ", item.name)),
        }
    }

    /// A sent `/name` naming an app command and no server one runs it, and the draft clears.
    pub fn run_slash(&mut self, text: &str, tx: &mut Tx) -> bool {
        let Some((name, "")) = parse_slash(text.trim()) else { return false };
        if self.add.slash.commands.iter().any(|c| c.name == name) {
            return false;
        }
        let Some(id) = self.slash_list().into_iter().find(|c| c.builtin.is_some() && c.name == name).map(|c| c.id) else { return false };
        self.pick_command(&id, tx);
        true
    }

    pub(crate) fn slash_key(&mut self, ev: &KeyEvent, tx: &mut Tx) -> bool {
        if self.slash_query().is_none() {
            return false;
        }
        if let Some(step) = nav_step(ev, true) {
            if let Some(active) = self.slash_active() {
                let items = self.slash_items();
                let keys = items.iter().map(|c| c.id.as_str());
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
                    self.pick_command(&name, tx);
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
        if let Some((directory, list)) = self.add.slash.fetch.take()
            && self.add.slash.dir.as_deref() == Some(directory.as_str())
        {
            self.add.slash.commands = list;
            changed = true;
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
        let server = server.clone();
        self.add.slash.fetch.request(directory, move |directory| {
            let path = format!("/api/command?location%5Bdirectory%5D={}", crate::switcher::encode(&directory));
            let list = server.get(&path).map(|v| commands(&v)).unwrap_or_default();
            (directory, list)
        });
        changed
    }

    /// A `/name args` draft naming a known command: the name and the arguments.
    pub fn slash_command(&self, text: &str) -> Option<(String, String)> {
        let (name, args) = parse_slash(text)?;
        self.add.slash.commands.iter().any(|c| c.name == name).then(|| (name.to_owned(), args.to_owned()))
    }
}

/// The `/` popover over the composer: each command's `/name`, its description, and its keys
/// in words at the end (`editor.tsx:1400-1402`: `keybind.join("+")`, muted).
pub fn commands_view(s: &App, p: &Pal, cx: &mut Cx<App>) -> Option<El> {
    s.slash_query()?;
    let width = cx.layout_size("composer").map_or(0.0, |[w, _]| w);
    let items = s.slash_items();
    let active = s.slash_active();
    let rows = items
        .iter()
        .map(|c| Row {
            id: c.id.clone(),
            key: format!("command-{}", c.id),
            label: format!("/{}", c.name),
            lead: None,
            description: c.description.clone(),
            trail: c.keys.filter(|b| b.chord().is_some()).map(|b| b.keycaps().join("+")),
        })
        .collect();
    let spec = Spec { key: "commands", label: "Commands", empty: "No matching items", width, boundary: crate::mentions::list::boundary(cx) };
    Some(popup(p, spec, rows, active.as_deref(), |s, name| s.slash_activate(name.to_owned()), |s, name, tx| s.pick_command(name, tx)))
}
