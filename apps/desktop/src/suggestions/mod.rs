//! The composer's suggestion machine ([`machine`], Desktop's `suggestions/machine.ts`) wired to
//! the app, as Desktop's `composer/editor/interaction.ts` wires it: typing, the composer's keys,
//! the + menu's Commands and Context, ↑/↓ and Shift+↑ become events, and the machine's effects
//! become edits of the draft, focus moves and the lists' highlight. The `@` list's rows are
//! `mentions`', the `/` list's `slash`'s and the history list's `history`'s.

pub mod history;
pub mod machine;

use rune::{Key, KeyEvent, TextEdit, Tx};

use crate::App;
use machine::{Effect, Event, Pick, Popover, Saved};
use session_view::composer::history::Entry;

#[derive(Default)]
pub struct Suggest {
    pub state: Popover,
    /// The search field of the commands menu and of the history list.
    pub search: TextEdit,
}

/// The entry as the draft, the caret at its end.
fn put_end(entry: &Entry, edit: &mut TextEdit) {
    entry.put(edit, entry.text.len());
}

/// A row id's list: the history list's rows are `history:<index>`.
fn history_index(id: &str) -> Option<usize> {
    id.strip_prefix("history:")?.parse().ok()
}

impl App {
    /// Steps the machine and carries out what it asks; whether it took the key.
    pub fn suggest(&mut self, event: Event<'_>, tx: &mut Tx) -> bool {
        let was = self.suggest.state.is_open();
        let step = machine::step(&mut self.suggest.state, event);
        let open = self.suggest.state.is_open();
        // The lists come, go and refilter in the frame of the key that did it.
        if was || open {
            tx.instant();
        }
        for effect in step.effects {
            self.apply_suggestion(effect, tx);
        }
        if open {
            self.refresh_suggestions(tx);
        }
        step.handled
    }

    fn apply_suggestion(&mut self, effect: Effect, tx: &mut Tx) {
        match effect {
            Effect::AddText(t) => self.composer.insert(t),
            Effect::RemoveTextBefore(t) => {
                let caret = self.composer.range().1;
                if self.composer.text[..caret].ends_with(t) {
                    self.composer.set_selection(caret - t.len(), caret);
                    self.composer.insert("");
                }
            }
            Effect::SetText(text) => put_end(&Entry { text, pills: Vec::new() }, &mut self.composer),
            Effect::Mention(key) => self.pick_mention(&key),
            Effect::Command { id, menu } => self.pick_command(&id, menu, tx),
            Effect::FocusEditor => {
                self.suggest.search.focused = false;
                self.composer.focused = true;
            }
            Effect::FocusSearch => {
                self.suggest.search = TextEdit::new("");
                self.suggest.search.focused = true;
                self.composer.focused = false;
            }
            Effect::Preview(id) => {
                if let Some(entry) = history_index(&id).and_then(|i| self.prompts.entries(self.add.shell()).get(i).cloned()) {
                    put_end(&entry, &mut self.composer);
                }
            }
            Effect::Apply { index, caret } => {
                if let Some(entry) = self.prompts.entries(self.add.shell()).get(index).cloned() {
                    let caret = caret.unwrap_or(entry.text.len());
                    entry.put(&mut self.composer, caret);
                    self.suggest.search.focused = false;
                    self.composer.focused = true;
                }
            }
            Effect::Restore { saved, focus } => {
                saved.entry.put(&mut self.composer, saved.caret);
                if focus {
                    self.suggest.search.focused = false;
                    self.composer.focused = true;
                }
            }
            Effect::Select(id) => {
                let pick = match &self.suggest.state {
                    Popover::History { .. } => history_index(&id).map(Pick::History),
                    Popover::Context(_) => Some(Pick::Mention(id)),
                    Popover::CommandInline(_) | Popover::CommandMenu(_) => Some(Pick::Command(id)),
                    Popover::Closed | Popover::Browsing { .. } => None,
                };
                if let Some(pick) = pick {
                    self.suggest(Event::Picked(pick), tx);
                }
            }
            Effect::Reveal(id) => {
                let (scroll, row) = match &self.suggest.state {
                    Popover::Context(_) => ("mentions-scroll", format!("mention-{id}")),
                    Popover::History { .. } => ("history-scroll", format!("history-row-{id}")),
                    _ => ("commands-scroll", format!("command-{id}")),
                };
                tx.scroll_into_view_in(scroll, row);
            }
        }
    }

    /// The rows the keys can highlight, in order: a stale file search's aren't.
    pub fn suggestion_ids(&self) -> Vec<String> {
        match &self.suggest.state {
            Popover::Context(list) => {
                let stale = !matches!(*self.source, crate::Source::Fixture) && self.mentions.search.files_stale(&list.query);
                self.mention_items().into_iter().filter(|i| !(stale && i.kind == crate::mentions::Kind::File && !i.recent)).map(|i| i.key).collect()
            }
            Popover::CommandInline(_) | Popover::CommandMenu(_) => self.slash_items().into_iter().map(|c| c.id).collect(),
            Popover::History { list, .. } => self.history_rows(&list.query).into_iter().map(|(id, _)| id).collect(),
            Popover::Closed | Popover::Browsing { .. } => Vec::new(),
        }
    }

    /// After anything that can change the open list's rows: the highlight follows them
    /// (`popover.results`), the `@` searches follow the query, and a mention with a space
    /// that finds nothing ends, so `@` can still start prose.
    pub fn refresh_suggestions(&mut self, tx: &mut Tx) {
        self.sync_mention_search(tx);
        if !self.suggest.state.is_open() {
            return;
        }
        let ids = self.suggestion_ids();
        let step = machine::step(&mut self.suggest.state, Event::Results(&ids));
        for effect in step.effects {
            self.apply_suggestion(effect, tx);
        }
        if let Popover::Context(list) = &self.suggest.state
            && list.query.chars().any(char::is_whitespace)
            && !self.mentions.search.sessions_pending(&list.query, self.mention_server())
            && self.mention_items().is_empty()
        {
            tx.instant();
            machine::step(&mut self.suggest.state, Event::Close);
            self.sync_mention_search(tx);
        }
    }

    /// The composer's text changed by typing (`input.changed`).
    pub fn suggest_input(&mut self, tx: &mut Tx) {
        let caret = self.composer.range().1.min(self.composer.text.len());
        let before = &self.composer.text[..caret];
        let pills: Vec<_> = self.composer.pills().map(|(r, _)| r).collect();
        let context = |open: bool| {
            crate::mentions::trigger(before, open).filter(|(at, _)| !pills.iter().any(|r| r.start <= *at && *at < r.end)).map(|(_, q)| q.to_owned())
        };
        let (closed, open) = (context(false), context(true));
        let command = self.composer.text.strip_prefix('/').filter(|q| !q.chars().any(char::is_whitespace)).map(str::to_owned);
        self.suggest(Event::Input { context: closed.as_deref(), context_open: open.as_deref(), command: command.as_deref() }, tx);
    }

    /// The composer's keys while a list is open, and Escape, Backspace and Ctrl+G always
    /// (`onKeyDown`); then PageUp and PageDown, which the lists don't page, stay with them.
    pub fn suggest_key(&mut self, ev: &KeyEvent, tx: &mut Tx) -> bool {
        let open = self.suggest.state.is_open();
        let key = keys(ev);
        if !open && !matches!(key, machine::Key::Escape | machine::Key::Backspace | machine::Key::Cancel) {
            return false;
        }
        let ids = if open { self.suggestion_ids() } else { Vec::new() };
        let composing = self.composer.is_composing();
        if self.suggest(Event::Key { key, ids: &ids, composing }, tx) {
            return true;
        }
        let plain = !(ev.mods.shift || ev.mods.alt || ev.mods.ctrl || ev.mods.cmd);
        open && plain && matches!(ev.key, Key::PageUp | Key::PageDown)
    }

    /// ↑ and ↓ recall sent prompts (`history.step`); Shift+↑ on an empty draft opens the
    /// history list (`history.open`).
    pub fn history_key(&mut self, ev: &KeyEvent, tx: &mut Tx) -> bool {
        let (up, down) = (ev.key == Key::Up, ev.key == Key::Down);
        if !(up || down) || ev.mods.alt || ev.mods.ctrl || ev.mods.cmd || !self.composer.focused || !self.has_composer() {
            return false;
        }
        // The menus and pickers use the arrows themselves while open.
        if self.add.open() || self.pickers.open.is_some() || self.switcher.open || self.settings.open {
            return false;
        }
        if self.composer.anchor != self.composer.focus || self.composer.is_composing() {
            return false;
        }
        if ev.mods.shift {
            return up && self.open_history(tx);
        }
        let entries = self.prompts.entries(self.add.shell()).len();
        let (len, caret) = (self.composer.text.len(), self.composer.focus);
        let draft = Saved { entry: Entry::of(&self.composer), caret };
        self.suggest(Event::HistoryStep { up, len, caret, entries, draft }, tx)
    }

    /// Shift+↑: the history list over an empty draft, its newest entry previewed.
    fn open_history(&mut self, tx: &mut Tx) -> bool {
        if self.suggest.state.is_open() || self.prompts.entries(self.add.shell()).is_empty() || !self.composer.text.is_empty() {
            return false;
        }
        let saved = Saved { entry: Entry::of(&self.composer), caret: self.composer.focus };
        let height = rune::memo::layout_size("composer").map(|[_, h]| h);
        self.suggest.search = TextEdit::new("");
        self.suggest.search.focused = true;
        self.composer.focused = false;
        self.suggest(Event::HistoryOpen { first: "history:0".into(), saved, height }, tx)
    }

    /// The + menu's Commands (`commands.open`).
    pub fn open_commands(&mut self, tx: &mut Tx) {
        let populated = !self.composer.text.trim().is_empty() || self.attach.any() || self.quote.any();
        let bare = self.composer.text == "/";
        self.suggest(Event::CommandsOpen { populated, bare }, tx);
    }

    /// The + menu's Context (`context.open`).
    pub fn open_context(&mut self, tx: &mut Tx) {
        self.suggest(Event::ContextOpen, tx);
    }

    /// The draft's shell mode changed (`mode.shell`, `mode.normal`, `mode.synced`): only a
    /// cancelled history preview has anything to do.
    pub fn suggest_mode(&mut self) {
        let shell = self.add.shell();
        for effect in machine::step(&mut self.suggest.state, Event::ModeSynced { shell }).effects {
            if let Effect::Restore { saved, .. } = effect {
                saved.entry.put(&mut self.composer, saved.caret);
            }
        }
    }

    /// Another draft replaced this one (`history.reset`): every list closes, and its searches.
    pub fn suggest_drop(&mut self) {
        machine::step(&mut self.suggest.state, Event::HistoryReset);
        let m = &mut *self.mentions;
        crate::mentions::search::step(&mut m.seq, &mut m.search, crate::mentions::search::Event::Closed);
    }

    /// Something was attached or a comment added (`draft.edited`): recalled history is now the draft.
    pub fn suggest_edited(&mut self) {
        machine::step(&mut self.suggest.state, Event::DraftEdited);
    }

    /// The search field of the commands menu or the history list changed (`popover.query`).
    pub fn suggest_query(&mut self, tx: &mut Tx) {
        let query = self.suggest.search.text.clone();
        self.suggest(Event::Query(&query), tx);
    }

    /// The pointer moved over a row (`popover.active`); a stale row isn't offered.
    pub fn suggest_pointed(&mut self, id: &str, tx: &mut Tx) {
        if self.suggest.state.active() == Some(id) || !self.suggestion_ids().iter().any(|x| x == id) {
            tx.live_only();
            return;
        }
        self.suggest(Event::Pointed(id), tx);
    }

    /// A row's press (`popover.select`).
    pub fn suggest_pick(&mut self, id: &str, tx: &mut Tx) {
        tx.instant();
        self.apply_suggestion(Effect::Select(id.to_owned()), tx);
    }

    /// Each frame: focus left the history list's search, for the draft (`popover.close`, the
    /// preview stays to edit) or anywhere else (`popover.dismiss`, the draft comes back), as
    /// Desktop's list hears `pointerdown` and `focusin` outside it.
    pub fn poll_suggestions(&mut self, tx: &mut Tx) -> bool {
        if !self.suggest.state.is_history() || self.suggest.search.focused {
            return false;
        }
        let in_draft = self.composer.focused;
        self.suggest(if in_draft { Event::Close } else { Event::Dismiss }, tx);
        true
    }
}

/// How the machine tells a key apart (`key.down`): Ctrl+N and Ctrl+P step as the arrows do.
fn keys(ev: &KeyEvent) -> machine::Key {
    use machine::Key as K;
    if let Some(step) = crate::popover::nav_step(ev, true) {
        return K::Step(step);
    }
    if crate::keymap::Bind::Stop.matches(ev) {
        return K::Cancel;
    }
    match &ev.key {
        Key::Escape => K::Escape,
        Key::Backspace => K::Backspace,
        Key::Tab => K::Tab,
        Key::Enter if !ev.mods.cmd && !ev.mods.ctrl => K::Enter,
        _ => K::Other,
    }
}
