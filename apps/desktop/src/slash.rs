//! The `/` command popover over the composer, as in Desktop (`suggestions/machine.ts`,
//! `composer/model.ts`'s `slashCommands`): the server's commands for the composer's directory,
//! then the app's own (`commands.rs`, those with a `/name`), filtered by the draft's first word.
//! Picking an app command runs it at once; a server command fills in `/name `.

use rune::{Cx, El, Latest, Tx, Waker};
use serde_json::Value;

use crate::palette::Pal;
use crate::mentions::list::{Row, Spec, data_list, popup, suggest_search};
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
    /// An app command whose `/name` takes words (`slash.arguments`): picking it types `/name `.
    pub args: bool,
}

#[derive(Default)]
pub struct Slash {
    /// `/api/command` for `dir`.
    pub commands: Vec<Command>,
    dir: Option<String>,
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
    /// The query the open commands list filters by: the word after a draft's `/`, or what the
    /// commands menu's search field holds.
    pub fn slash_query(&self) -> Option<String> {
        self.suggest.state.commands().map(|l| l.query.clone())
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
            args: false,
        });
        let mut own: Vec<Item> = self.commands().into_iter().filter(|c| !matches!(c.run, crate::commands::Run::Plugin(_))).filter_map(|c| {
            let name = c.slash?;
            // Desktop's `/new` is `session.new`, which has no keys of its own (`use-session-commands.tsx:446-451`).
            let keys = c.keys.filter(|_| name != "new");
            Some(Item { id: format!("builtin.{}", c.id), name: name.into(), title: c.title.into(), description: c.description.into(), keys, builtin: Some(c.id), args: false })
        }).collect();
        // Plugin commands, hidden ones too (`/btw`), as Desktop's `slashCommands` lists them.
        own.extend(crate::plugin_host::slash_commands(self).into_iter().map(|c| Item {
            id: format!("builtin.{}", c.id),
            name: c.name.into(),
            title: c.title.into(),
            description: c.description.into(),
            keys: None,
            builtin: Some(c.id),
            args: c.args,
        }));
        own.sort_by_key(|c| ORDER.iter().position(|n| *n == c.name).unwrap_or(ORDER.len()));
        server.chain(own).collect()
    }

    /// The commands the query finds by name or title, best first, as Desktop's `commandList`
    /// filters them (`editor/interaction.ts:336-340`: fuzzysort over `trigger` and `title`); all
    /// of them, in order, for no query.
    pub fn slash_items(&self) -> Vec<Item> {
        let Some(query) = self.slash_query() else { return Vec::new() };
        use crate::mentions::fuzzysort::{Target, go};
        let list = self.slash_list();
        let needle = query.to_lowercase();
        if needle.is_empty() {
            return list;
        }
        let targets: Vec<[Target; 2]> = list.iter().map(|c| [Target::new(&c.name), Target::new(&c.title)]).collect();
        let objects: Vec<&[Target]> = targets.iter().map(|t| &t[..]).collect();
        let mut list: Vec<Option<Item>> = list.into_iter().map(Some).collect();
        go(&needle, &objects).into_iter().filter_map(|i| list[i].take()).collect()
    }

    pub fn slash_active(&self) -> Option<String> {
        self.suggest.state.commands().and_then(|l| l.active.id()).map(str::to_owned)
    }

    /// A picked command (`onSuggestionSelect`, then `draft.setText`): an app command that takes
    /// no words clears the draft, unless it came from the commands menu, and runs; any other
    /// fills in `/name `, before the draft's words when the menu picked it.
    pub fn pick_command(&mut self, id: &str, menu: bool, tx: &mut Tx) {
        let Some(item) = self.slash_list().into_iter().find(|c| c.id == id) else { return };
        if let Some(command) = item.builtin.filter(|_| !item.args) {
            if !menu {
                let len = self.composer.text.len();
                self.composer.set_selection(0, len);
                self.composer.insert("");
            }
            if !crate::plugin_host::run_command(self, command, tx) {
                self.run_command(command, tx);
            }
            return;
        }
        let draft = self.composer.text.trim().to_owned();
        let text = if menu && !draft.is_empty() { format!("/{} {draft}", item.name) } else { format!("/{} ", item.name) };
        let len = self.composer.text.len();
        self.composer.set_selection(0, len);
        self.composer.insert(&text);
    }

    /// A sent `/name` naming an app command and no server one runs it, and the draft clears; a
    /// command that takes words runs with the rest (`parseClientSlashCommand`).
    pub fn run_slash(&mut self, text: &str, tx: &mut Tx) -> bool {
        let Some((name, words)) = parse_slash(text.trim()) else { return false };
        if self.add.slash.commands.iter().any(|c| c.name == name) {
            return false;
        }
        let Some(item) = self.slash_list().into_iter().find(|c| c.builtin.is_some() && c.name == name) else { return false };
        if item.args {
            let Some(command) = item.builtin else { return false };
            let len = self.composer.text.len();
            self.composer.set_selection(0, len);
            self.composer.insert("");
            crate::plugin_host::run_slash(self, command, words, tx);
            return true;
        }
        if !words.is_empty() {
            return false;
        }
        self.pick_command(&item.id, false, tx);
        true
    }

    /// Takes in fetched commands, and asks for the composer directory's when a `/` needs them.
    pub fn poll_commands(&mut self, tx: &mut Tx) -> bool {
        let mut changed = false;
        if let Some((directory, list)) = self.add.slash.fetch.take()
            && self.add.slash.dir.as_deref() == Some(directory.as_str())
        {
            self.add.slash.commands = list;
            changed = true;
            if self.suggest.state.commands().is_some() {
                tx.instant();
                self.refresh_suggestions(tx);
            }
        }
        if self.slash_query().is_none() {
            return changed;
        }
        let Source::Live { server, .. } = &*self.source else { return changed };
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
    // The commands menu searches in a field of its own, its placeholder the slash (`editor.tsx:201-211`).
    let label = opencode_i18n::t!("ui.promptInput.commands");
    let search = matches!(s.suggest.state, crate::suggestions::machine::Popover::CommandMenu(_)).then(|| suggest_search(p, &s.suggest.search, label, "/"));
    let spec = Spec { key: "commands", label, empty: opencode_i18n::t!("ui.promptInput.noMatchingItems"), width, boundary: crate::mentions::list::boundary(cx), search };
    Some(popup(p, spec, rows, active.as_deref(), |s, id, tx| s.suggest_pointed(id, tx), |s, id, tx| s.suggest_pick(id, tx)))
}
