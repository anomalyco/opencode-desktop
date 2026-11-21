//! The composer's suggestion machine ([`machine`], Desktop's `suggestions/machine.ts`) wired to
//! the host, as Desktop's `composer/editor/interaction.ts` wires it: typing, the composer's keys,
//! the + menu's Commands and Context, ↑/↓ and Shift+↑ become events, and the machine's effects
//! become edits of the draft, focus moves and the lists' highlight. The `@` list's rows come from
//! the host's mentions provider, the `/` list's from [`crate::slash`], and the history list's from
//! [`history`].

pub mod history;
pub mod machine;
pub mod popover;

use rune::{Key, KeyEvent, TextEdit, Tx};
use session_view::composer::history::Entry;

use crate::prompt_history::PromptHistory;
use machine::{Effect, Event, Pick, Popover, Saved};

pub use history::history_rows;
pub use popover::{PopoverRow, PopoverSpec, boundary, max_height, popup, suggest_search};

#[derive(Default)]
pub struct Suggest {
    pub state: Popover,
    /// The search field of the commands menu and of the history list.
    pub search: TextEdit,
}

/// Object-safe host boundary for the composer suggestion machine, keyboard routing, and history
/// popover.
pub trait SuggestionsHost {
    /// Read-only access to suggestion state (must not dirty `Tracked` cells).
    fn suggest(&self) -> &Suggest;
    /// Mutable access to suggestion state.
    fn suggest_mut(&mut self) -> &mut Suggest;

    /// Active composer editor (read).
    fn composer(&self) -> &TextEdit;
    /// Active composer editor (write).
    fn composer_mut(&mut self) -> &mut TextEdit;

    /// Sent prompt history store used by ↑/↓ recall and Shift+↑ history search.
    fn prompts(&self) -> &PromptHistory;

    /// Whether the composer is currently in shell (`!`) mode.
    fn shell_mode(&self) -> bool {
        false
    }
    /// Whether the open page has a composer (a draft or a non-subagent session).
    fn has_composer(&self) -> bool {
        true
    }
    /// Whether an open menu, picker, switcher, or settings overlay currently owns ↑/↓ keys.
    fn history_key_blocked(&self) -> bool {
        false
    }
    /// Whether the composer holds non-text attachments or quotes (makes `+ -> Commands` open the
    /// command menu with its own search field instead of inline `/`).
    fn composer_has_attachments_or_quotes(&self) -> bool {
        false
    }
    /// Whether picking or dismissing a popover should focus the composer editor.
    fn auto_focus_composer(&self, tx: &Tx) -> bool {
        !session_view::style::touch_controls() || tx.by_keys()
    }

    /// Selectable `@` mention row keys for `query`, filtering out stale non-recent file rows.
    fn mention_suggestion_ids(&self, query: &str) -> Vec<String> {
        let _ = query;
        Vec::new()
    }
    /// Selectable `/` slash command row IDs for the active query.
    fn slash_suggestion_ids(&self) -> Vec<String> {
        Vec::new()
    }
    /// Whether a `@` context query containing whitespace should auto-close (not waiting on
    /// pending session search results and currently matching zero mention rows).
    fn mention_whitespace_empty(&self, query: &str) -> bool {
        let _ = query;
        false
    }

    /// Inserts the picked `@` mention pill for `key`.
    fn pick_mention(&mut self, key: &str) {
        let _ = key;
    }
    /// Runs or inserts the picked `/` command `id` (`menu`: chosen from the commands menu).
    fn pick_command(&mut self, id: &str, menu: bool, tx: &mut Tx) {
        let _ = (id, menu, tx);
    }
    /// Synchronizes `@` mention catalog/file/session searches with the active query.
    fn sync_mention_search(&mut self, tx: &mut Tx) {
        let _ = tx;
    }
    /// Closes active `@` mention searches when the draft is replaced.
    fn close_mention_search(&mut self) {}
    /// Called when the draft matches a `/` command trigger (for example, waking dormant command
    /// plugins).
    fn on_command_trigger(&mut self) {}
}

/// The entry as the draft, the caret at its end.
fn put_end(entry: &Entry, edit: &mut TextEdit) {
    entry.put(edit, entry.text.len());
}

/// A row id's list: the history list's rows are `history:<index>`.
fn history_index(id: &str) -> Option<usize> {
    id.strip_prefix("history:")?.parse().ok()
}

/// The `@` before the caret and the query after it. Closed, the query is one run of non-space
/// characters; open, single spaces join words (and one may trail), so multi-word session titles
/// stay searchable. The `@` starts the text or follows whitespace.
pub fn mention_trigger(before: &str, open: bool) -> Option<(usize, &str)> {
    let at = before.rfind('@')?;
    if before[..at].chars().next_back().is_some_and(|c| !c.is_whitespace()) {
        return None;
    }
    let query = &before[at + 1..];
    let ok = if open {
        !query.starts_with(' ') && !query.contains("  ") && query.chars().all(|c| c == ' ' || !c.is_whitespace())
    } else {
        !query.chars().any(char::is_whitespace)
    };
    ok.then_some((at, query))
}

/// Steps the machine and carries out what it asks; whether it took the key.
pub fn suggest(host: &mut dyn SuggestionsHost, event: Event<'_>, tx: &mut Tx) -> bool {
    let was = host.suggest().state.is_open();
    let step = machine::step(&mut host.suggest_mut().state, event);
    let open = host.suggest().state.is_open();
    // The lists come, go and refilter in the frame of the key that did it.
    if was || open {
        tx.instant();
    }
    for effect in step.effects {
        apply_suggestion(host, effect, tx);
    }
    if open {
        refresh_suggestions(host, tx);
    }
    step.handled
}

fn apply_suggestion(host: &mut dyn SuggestionsHost, effect: Effect, tx: &mut Tx) {
    match effect {
        Effect::AddText(t) => host.composer_mut().insert(t),
        Effect::RemoveTextBefore(t) => {
            let composer = host.composer_mut();
            let caret = composer.range().1;
            if composer.text[..caret].ends_with(t) {
                composer.set_selection(caret - t.len(), caret);
                composer.insert("");
            }
        }
        Effect::SetText(text) => put_end(&Entry { text, pills: Vec::new() }, host.composer_mut()),
        Effect::Mention(key) => host.pick_mention(&key),
        Effect::Command { id, menu } => host.pick_command(&id, menu, tx),
        Effect::FocusEditor => {
            host.suggest_mut().search.focused = false;
            if host.auto_focus_composer(tx) {
                host.composer_mut().focused = true;
            }
        }
        Effect::FocusSearch => {
            let keep_keyboard = host.composer().focused || host.auto_focus_composer(tx);
            let s = host.suggest_mut();
            s.search = TextEdit::new("");
            s.search.focused = keep_keyboard;
            host.composer_mut().focused = false;
        }
        Effect::Preview(id) => {
            let shell = host.shell_mode();
            if let Some(entry) = history_index(&id).and_then(|i| host.prompts().entries(shell).get(i).cloned()) {
                put_end(&entry, host.composer_mut());
                // Show the beginning of each previewed prompt (`editor.tsx:125-133`).
                tx.scroll_to_instant("editor-scroll", 0.0, 0.0);
            }
        }
        Effect::Apply { index, caret } => {
            let shell = host.shell_mode();
            if let Some(entry) = host.prompts().entries(shell).get(index).cloned() {
                let caret = caret.unwrap_or(entry.text.len());
                entry.put(host.composer_mut(), caret);
                host.suggest_mut().search.focused = false;
                if host.auto_focus_composer(tx) {
                    host.composer_mut().focused = true;
                }
            }
        }
        Effect::Restore { saved, focus } => {
            saved.entry.put(host.composer_mut(), saved.caret);
            if focus {
                host.suggest_mut().search.focused = false;
                if host.auto_focus_composer(tx) {
                    host.composer_mut().focused = true;
                }
            }
        }
        Effect::Select(id) => {
            let pick = match &host.suggest().state {
                Popover::History { .. } => history_index(&id).map(Pick::History),
                Popover::Context(_) => Some(Pick::Mention(id)),
                Popover::CommandInline(_) | Popover::CommandMenu(_) => Some(Pick::Command(id)),
                Popover::Closed | Popover::Browsing { .. } => None,
            };
            if let Some(pick) = pick {
                suggest(host, Event::Picked(pick), tx);
            }
        }
        Effect::Reveal(id) => {
            let (scroll, row) = match &host.suggest().state {
                Popover::Context(_) => ("mentions-scroll", format!("mention-{id}")),
                Popover::History { .. } => ("history-scroll", format!("history-row-{id}")),
                _ => ("commands-scroll", format!("command-{id}")),
            };
            tx.scroll_into_view_in(scroll, row);
        }
    }
}

/// The rows the keys can highlight, in order: a stale file search's aren't.
pub fn suggestion_ids(host: &dyn SuggestionsHost) -> Vec<String> {
    match &host.suggest().state {
        Popover::Context(list) => host.mention_suggestion_ids(&list.query),
        Popover::CommandInline(_) | Popover::CommandMenu(_) => host.slash_suggestion_ids(),
        Popover::History { list, .. } => history_rows(host, &list.query).into_iter().map(|(id, _)| id).collect(),
        Popover::Closed | Popover::Browsing { .. } => Vec::new(),
    }
}

/// After anything that can change the open list's rows: the highlight follows them
/// (`popover.results`), the `@` searches follow the query, and a mention with a space
/// that finds nothing ends, so `@` can still start prose.
pub fn refresh_suggestions(host: &mut dyn SuggestionsHost, tx: &mut Tx) {
    host.sync_mention_search(tx);
    if !host.suggest().state.is_open() {
        return;
    }
    let ids = suggestion_ids(host);
    let step = machine::step(&mut host.suggest_mut().state, Event::Results(&ids));
    for effect in step.effects {
        apply_suggestion(host, effect, tx);
    }
    let auto_close = matches!(
        &host.suggest().state,
        Popover::Context(list) if list.query.chars().any(char::is_whitespace) && host.mention_whitespace_empty(&list.query)
    );
    if auto_close {
        tx.instant();
        machine::step(&mut host.suggest_mut().state, Event::Close);
        host.sync_mention_search(tx);
    }
}

/// The composer's text changed by typing (`input.changed`).
pub fn suggest_input(host: &mut dyn SuggestionsHost, tx: &mut Tx) {
    let (closed, open, command) = {
        let composer = host.composer();
        let caret = composer.range().1.min(composer.text.len());
        let before = &composer.text[..caret];
        let pills: Vec<_> = composer.pills().map(|(r, _)| r).collect();
        let context = |open: bool| {
            mention_trigger(before, open)
                .filter(|(at, _)| !pills.iter().any(|r| r.start <= *at && *at < r.end))
                .map(|(_, q)| q.to_owned())
        };
        let closed = context(false);
        let open = context(true);
        let command = composer
            .text
            .strip_prefix('/')
            .filter(|q| !q.chars().any(char::is_whitespace))
            .map(str::to_owned);
        (closed, open, command)
    };
    // The `/` list offers the commands a plugin enables.
    if command.is_some() {
        host.on_command_trigger();
    }
    suggest(
        host,
        Event::Input {
            context: closed.as_deref(),
            context_open: open.as_deref(),
            command: command.as_deref(),
        },
        tx,
    );
}

/// The composer's keys while a list is open, and Escape, Backspace and Ctrl+G always
/// (`onKeyDown`); then PageUp and PageDown, which the lists don't page, stay with them.
pub fn suggest_key(host: &mut dyn SuggestionsHost, ev: &KeyEvent, tx: &mut Tx) -> bool {
    let open = host.suggest().state.is_open();
    let key = keys(ev);
    if !open && !matches!(key, machine::Key::Escape | machine::Key::Backspace | machine::Key::Cancel) {
        return false;
    }
    let ids = if open { suggestion_ids(host) } else { Vec::new() };
    let composing = host.composer().is_composing();
    if suggest(host, Event::Key { key, ids: &ids, composing }, tx) {
        return true;
    }
    let plain = !(ev.mods.shift || ev.mods.alt || ev.mods.ctrl || ev.mods.cmd);
    open && plain && matches!(ev.key, Key::PageUp | Key::PageDown)
}

/// ↑ and ↓ recall sent prompts (`history.step`); Shift+↑ on an empty draft opens the
/// history list (`history.open`).
pub fn history_key(host: &mut dyn SuggestionsHost, ev: &KeyEvent, tx: &mut Tx) -> bool {
    let (up, down) = (ev.key == Key::Up, ev.key == Key::Down);
    if !(up || down) || ev.mods.alt || ev.mods.ctrl || ev.mods.cmd || !host.composer().focused || !host.has_composer() {
        return false;
    }
    // The menus and pickers use the arrows themselves while open.
    if host.history_key_blocked() {
        return false;
    }
    if host.composer().anchor != host.composer().focus || host.composer().is_composing() {
        return false;
    }
    if ev.mods.shift {
        return up && open_history(host, tx);
    }
    let entries = host.prompts().entries(host.shell_mode()).len();
    let (len, caret, draft) = {
        let composer = host.composer();
        (composer.text.len(), composer.focus, Saved { entry: Entry::of(composer), caret: composer.focus })
    };
    suggest(host, Event::HistoryStep { up, len, caret, entries, draft }, tx)
}

/// Shift+↑: the history list over an empty draft, its newest entry previewed.
fn open_history(host: &mut dyn SuggestionsHost, tx: &mut Tx) -> bool {
    if host.suggest().state.is_open() || host.prompts().entries(host.shell_mode()).is_empty() || !host.composer().text.is_empty() {
        return false;
    }
    let saved = {
        let composer = host.composer();
        Saved { entry: Entry::of(composer), caret: composer.focus }
    };
    let height = rune::memo::layout_size("composer").map(|[_, h]| h);
    let s = host.suggest_mut();
    s.search = TextEdit::new("");
    s.search.focused = true;
    host.composer_mut().focused = false;
    suggest(host, Event::HistoryOpen { first: "history:0".into(), saved, height }, tx)
}

/// The + menu's Commands (`commands.open`).
pub fn open_commands(host: &mut dyn SuggestionsHost, tx: &mut Tx) {
    let populated = !host.composer().text.trim().is_empty() || host.composer_has_attachments_or_quotes();
    let bare = host.composer().text == "/";
    suggest(host, Event::CommandsOpen { populated, bare }, tx);
}

/// The + menu's Context (`context.open`).
pub fn open_context(host: &mut dyn SuggestionsHost, tx: &mut Tx) {
    suggest(host, Event::ContextOpen, tx);
}

/// The draft's shell mode changed (`mode.shell`, `mode.normal`, `mode.synced`): only a
/// cancelled history preview has anything to do.
pub fn suggest_mode(host: &mut dyn SuggestionsHost) {
    let shell = host.shell_mode();
    let effects = machine::step(&mut host.suggest_mut().state, Event::ModeSynced { shell }).effects;
    for effect in effects {
        if let Effect::Restore { saved, .. } = effect {
            saved.entry.put(host.composer_mut(), saved.caret);
        }
    }
}

/// Another draft replaced this one (`history.reset`): every list closes, and its searches.
pub fn suggest_drop(host: &mut dyn SuggestionsHost) {
    machine::step(&mut host.suggest_mut().state, Event::HistoryReset);
    host.close_mention_search();
}

/// Something was attached or a comment added (`draft.edited`): recalled history is now the draft.
pub fn suggest_edited(host: &mut dyn SuggestionsHost) {
    if matches!(host.suggest().state, Popover::Browsing { .. }) {
        machine::step(&mut host.suggest_mut().state, Event::DraftEdited);
    }
}

/// The search field of the commands menu or the history list changed (`popover.query`).
pub fn suggest_query(host: &mut dyn SuggestionsHost, tx: &mut Tx) {
    let query = host.suggest().search.text.clone();
    suggest(host, Event::Query(&query), tx);
}

/// The pointer moved over a row (`popover.active`); a stale row isn't offered.
pub fn suggest_pointed(host: &mut dyn SuggestionsHost, id: &str, tx: &mut Tx) {
    if host.suggest().state.active() == Some(id) || !suggestion_ids(host).iter().any(|x| x == id) {
        tx.live_only();
        return;
    }
    suggest(host, Event::Pointed(id), tx);
}

/// A row's press (`popover.select`).
pub fn suggest_pick(host: &mut dyn SuggestionsHost, id: &str, tx: &mut Tx) {
    tx.instant();
    apply_suggestion(host, Effect::Select(id.to_owned()), tx);
}

/// Each frame: focus left the history list's search, for the draft (`popover.close`, the
/// preview stays to edit) or anywhere else (`popover.dismiss`, the draft comes back), as
/// Desktop's list hears `pointerdown` and `focusin` outside it.
pub fn poll_suggestions(host: &mut dyn SuggestionsHost, tx: &mut Tx) -> bool {
    let s = host.suggest();
    if !s.state.is_history() || s.search.focused {
        return false;
    }
    let in_draft = host.composer().focused;
    suggest(host, if in_draft { Event::Close } else { Event::Dismiss }, tx);
    true
}

/// How the machine tells a key apart (`key.down`): Ctrl+N and Ctrl+P step as the arrows do.
fn keys(ev: &KeyEvent) -> machine::Key {
    use machine::Key as K;
    if let Some(step) = session_view::popover::nav_step(ev, true) {
        return K::Step(step);
    }
    // `prompt.stop`'s other keys (^G) cancel the list; its Escape stays Escape.
    if ev.key != Key::Escape && opencode_ui::keymap::Bind::Stop.matches(ev) {
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
