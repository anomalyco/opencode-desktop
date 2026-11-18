//! The composer's suggestion lists and prompt recall as one machine, Desktop's
//! `transitionComposer` (`packages/app/src/composer/suggestions/machine.ts`). At most one is
//! open, and each carries only what it needs: the `@` context list, the `/` commands (inline
//! under a draft that is `/` and a word, or as a menu with its own search over a written
//! draft), the Shift+↑ history search, or plain ↑/↓ browsing of sent prompts, which shows no
//! list. [`step`] returns what the composer does next; the caller carries it out
//! (`super::apply_suggestion`).
//!
//! The draft's shell mode belongs to the shell machine (`session_view::composer::mode`); this one
//! only hears that it changed ([`Event::ModeSynced`]). Its `drag` half is the dropzone
//! ([`crate::dropzone`]), and its `focus` half is the composer's real focus, which Rune keeps.

use rune::machine::Effects;
use session_view::composer::history::Entry;

/// The draft that recall replaced (`ComposerSavedDraft`), put back when ↓ passes the newest
/// entry or a history preview is cancelled.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Saved {
    pub entry: Entry,
    pub caret: usize,
}

/// The highlighted row (`activeID` with `chosen`): the first row while results arrive and
/// reorder, until the keys or the pointer choose one, which stays while the results hold it.
#[derive(Clone, Debug, Default, PartialEq)]
pub enum Active {
    #[default]
    None,
    First(String),
    Chosen(String),
}

impl Active {
    pub fn id(&self) -> Option<&str> {
        match self {
            Active::None => None,
            Active::First(id) | Active::Chosen(id) => Some(id),
        }
    }
}

/// An open list: what it's filtered by and its highlight.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct List {
    pub query: String,
    pub active: Active,
}

impl List {
    fn of(query: &str) -> List {
        List { query: query.to_owned(), active: Active::None }
    }
}

/// `ComposerPopoverState`.
#[derive(Clone, Debug, Default, PartialEq)]
pub enum Popover {
    #[default]
    Closed,
    /// ↑ and ↓ recall sent prompts into the draft, `index` from the newest, with no list open.
    Browsing { index: usize, saved: Saved },
    Context(List),
    /// The draft is `/` and a word.
    CommandInline(List),
    /// The + menu's Commands over a written draft: the list has a search field of its own.
    CommandMenu(List),
    /// Shift+↑: sent prompts with a search field; the highlighted one previews in the draft,
    /// which keeps the composer's `height` from before the first preview.
    History { list: List, saved: Saved, height: Option<f32> },
}

impl Popover {
    /// A list shows (`isComposerPopoverOpen`); browsing shows none.
    pub fn list(&self) -> Option<&List> {
        match self {
            Popover::Context(l) | Popover::CommandInline(l) | Popover::CommandMenu(l) => Some(l),
            Popover::History { list, .. } => Some(list),
            Popover::Closed | Popover::Browsing { .. } => None,
        }
    }

    fn list_mut(&mut self) -> Option<&mut List> {
        match self {
            Popover::Context(l) | Popover::CommandInline(l) | Popover::CommandMenu(l) => Some(l),
            Popover::History { list, .. } => Some(list),
            Popover::Closed | Popover::Browsing { .. } => None,
        }
    }

    pub fn is_open(&self) -> bool {
        self.list().is_some()
    }

    pub fn active(&self) -> Option<&str> {
        self.list().and_then(|l| l.active.id())
    }

    pub fn is_history(&self) -> bool {
        matches!(self, Popover::History { .. })
    }

    /// The commands list, inline or as a menu.
    pub fn commands(&self) -> Option<&List> {
        match self {
            Popover::CommandInline(l) | Popover::CommandMenu(l) => Some(l),
            _ => None,
        }
    }

    pub fn context(&self) -> Option<&List> {
        match self {
            Popover::Context(l) => Some(l),
            _ => None,
        }
    }

    /// The composer's height a history preview holds.
    pub fn held_height(&self) -> Option<f32> {
        match self {
            Popover::History { height, .. } => *height,
            _ => None,
        }
    }
}

/// A key the composer heard (`key.down`), as the machine tells them apart.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Key {
    Escape,
    /// Ctrl+G.
    Cancel,
    Backspace,
    Tab,
    Enter,
    /// ↓ or Ctrl+N (1), ↑ or Ctrl+P (-1).
    Step(i64),
    Other,
}

/// What was picked from a list (`popover.select`).
#[derive(Clone, Debug, PartialEq)]
pub enum Pick {
    /// A sent prompt, by its index from the newest.
    History(usize),
    /// A command row.
    Command(String),
    /// A context row: a skill, agent, session or file.
    Mention(String),
}

pub enum Event<'a> {
    /// Typing changed the draft (`input.changed`). `context` reads the `@` before the caret by
    /// the rule a closed list uses, and `context_open` by an open one's (single spaces join
    /// words); `command` is the word after a draft's leading `/` when the draft is just that.
    Input { context: Option<&'a str>, context_open: Option<&'a str>, command: Option<&'a str> },
    /// The draft changed some other way (an attachment, a comment): recall is now the draft.
    DraftEdited,
    /// The + menu's Commands (`commands.open`). `populated`: the draft holds something; `bare`:
    /// it is just the `/` an earlier open typed.
    CommandsOpen { populated: bool, bare: bool },
    ContextOpen,
    /// Shift+↑ on an empty draft (`history.open`), the newest entry first.
    HistoryOpen { first: String, saved: Saved, height: Option<f32> },
    /// Plain ↑ or ↓ (`history.step`): the draft's length and caret, how many entries there are,
    /// and the draft as it is, kept if browsing starts.
    HistoryStep { up: bool, len: usize, caret: usize, entries: usize, draft: Saved },
    /// Another draft replaced this one, or a send emptied it (`history.reset`).
    HistoryReset,
    /// The search field of the commands menu or the history list (`popover.query`).
    Query(&'a str),
    /// The selectable rows, in order (`popover.results`).
    Results(&'a [String]),
    /// The pointer moved over a row (`popover.active`).
    Pointed(&'a str),
    /// Close the list (`popover.close`): a send, or a press in the draft.
    Close,
    /// A press or focus outside the list (`popover.dismiss`): a history preview puts the draft back.
    Dismiss,
    Picked(Pick),
    Key { key: Key, ids: &'a [String], composing: bool },
    /// Something else set the draft's shell mode (`mode.shell`, `mode.normal`, `mode.synced`).
    ModeSynced { shell: bool },
}

#[derive(Clone, Debug, PartialEq)]
pub enum Effect {
    /// Puts `/` or `@` at the caret (`draft.addText`).
    AddText(&'static str),
    /// Takes `/` or `@` back from before the caret (`draft.removeTextBefore`).
    RemoveTextBefore(&'static str),
    /// The draft is just this (`draft.setText`).
    SetText(String),
    /// The row's pill replaces the `@query` before the caret (`mention.add`).
    Mention(String),
    /// A command row was picked, from the menu or inline (`draft.setText`, or the app's own
    /// command running).
    Command { id: String, menu: bool },
    FocusEditor,
    /// The list's own search field takes the keys (`focus.command-search`).
    FocusSearch,
    /// The entry shows in the draft while highlighted (`history.preview`).
    Preview(String),
    /// The entry becomes the draft, the caret at `caret` or its end (`history.apply`).
    Apply { index: usize, caret: Option<usize> },
    /// The saved draft comes back (`history.restore`).
    Restore { saved: Saved, focus: bool },
    /// Tab or Return on the highlighted row: the caller picks it (`suggestion.select`).
    Select(String),
    /// Brings the highlighted row into view.
    Reveal(String),
}

/// What a step did: its effects, and whether it took the key (`handled`).
#[derive(Debug, Default, PartialEq)]
pub struct Step {
    pub effects: Effects<Effect>,
    pub handled: bool,
}

fn did(effects: Effects<Effect>) -> Step {
    Step { effects, handled: false }
}

fn took(effects: Effects<Effect>) -> Step {
    Step { effects, handled: true }
}

pub fn step(state: &mut Popover, event: Event<'_>) -> Step {
    match event {
        Event::Input { context, context_open, command } => input(state, context, context_open, command),
        Event::DraftEdited => {
            if matches!(state, Popover::Browsing { .. }) {
                *state = Popover::Closed;
            }
            Step::default()
        }
        Event::CommandsOpen { populated, bare } => commands_open(state, populated, bare),
        Event::ContextOpen => context_open(state),
        Event::HistoryOpen { first, saved, height } => history_open(state, first, saved, height),
        Event::HistoryStep { up, len, caret, entries, draft } => history_step(state, up, len, caret, entries, draft),
        Event::HistoryReset | Event::Close => {
            *state = Popover::Closed;
            Step::default()
        }
        Event::Dismiss => match std::mem::take(state) {
            Popover::History { saved, .. } => did(Effects::one(Effect::Restore { saved, focus: false })),
            Popover::Browsing { index, saved } => {
                *state = Popover::Browsing { index, saved };
                Step::default()
            }
            _ => Step::default(),
        },
        Event::Query(query) => {
            let Some(list) = state.list_mut() else { return Step::default() };
            // The history list previews again once the results come; the others refilter.
            *list = List::of(query);
            Step::default()
        }
        Event::Results(ids) => results(state, ids),
        Event::Pointed(id) => pointed(state, id),
        Event::Picked(pick) => picked(state, pick),
        Event::Key { key, ids, composing } => key_down(state, key, ids, composing),
        Event::ModeSynced { shell } => mode_synced(state, shell),
    }
}

fn input(state: &mut Popover, context: Option<&str>, context_open: Option<&str>, command: Option<&str>) -> Step {
    // An open mention keeps single spaces, so multi-word session titles stay searchable.
    let context = if matches!(state, Popover::Context(_)) { context_open } else { context };
    *state = match (context, command) {
        (Some(query), _) => Popover::Context(List::of(query)),
        (None, Some(query)) => Popover::CommandInline(List::of(query)),
        // Typing leaves the commands menu, whose search field takes its own input, and makes
        // recalled history the draft.
        (None, None) => Popover::Closed,
    };
    Step::default()
}

fn commands_open(state: &mut Popover, populated: bool, bare: bool) -> Step {
    // A second press closes the commands, taking back the slash the first one typed.
    if let Popover::CommandInline(_) | Popover::CommandMenu(_) = state {
        let typed = matches!(state, Popover::CommandInline(_)) && bare;
        *state = Popover::Closed;
        return did(if typed {
            Effects::two(Effect::SetText(String::new()), Effect::FocusEditor)
        } else {
            Effects::one(Effect::FocusEditor)
        });
    }
    if !populated {
        *state = Popover::CommandInline(List::default());
        return did(Effects::two(Effect::AddText("/"), Effect::FocusEditor));
    }
    *state = Popover::CommandMenu(List::default());
    did(Effects::one(Effect::FocusSearch))
}

fn context_open(state: &mut Popover) -> Step {
    // A second press closes the list, taking back the @ it typed while nothing follows it.
    if let Popover::Context(list) = state {
        let typed = list.query.is_empty();
        *state = Popover::Closed;
        return did(if typed {
            Effects::two(Effect::RemoveTextBefore("@"), Effect::FocusEditor)
        } else {
            Effects::one(Effect::FocusEditor)
        });
    }
    *state = Popover::Context(List::default());
    did(Effects::two(Effect::AddText("@"), Effect::FocusEditor))
}

fn history_open(state: &mut Popover, first: String, saved: Saved, height: Option<f32>) -> Step {
    if state.is_open() {
        return Step::default();
    }
    // Searching from a recalled entry still cancels back to the draft the user wrote.
    let saved = match std::mem::take(state) {
        Popover::Browsing { saved, .. } => saved,
        _ => saved,
    };
    *state = Popover::History { list: List { query: String::new(), active: Active::First(first.clone()) }, saved, height };
    took(Effects::one(Effect::Preview(first)))
}

fn history_step(state: &mut Popover, up: bool, len: usize, caret: usize, entries: usize, draft: Saved) -> Step {
    if state.is_open() {
        return Step::default();
    }
    let browsing = match state {
        Popover::Browsing { index, .. } => Some(*index),
        _ => None,
    };
    if !session_view::composer::history::can_step(up, len, caret, browsing.is_some()) {
        return Step::default();
    }
    if up {
        let index = browsing.map_or(0, |i| i + 1);
        if index >= entries {
            return Step::default();
        }
        let saved = match std::mem::take(state) {
            Popover::Browsing { saved, .. } => saved,
            _ => draft,
        };
        *state = Popover::Browsing { index, saved };
        return took(Effects::one(Effect::Apply { index, caret: Some(0) }));
    }
    let Some(index) = browsing else { return Step::default() };
    if index == 0 {
        let Popover::Browsing { saved, .. } = std::mem::take(state) else { unreachable!("browsing above") };
        return took(Effects::one(Effect::Restore { saved, focus: true }));
    }
    if index > entries {
        return Step::default();
    }
    if let Popover::Browsing { index: at, .. } = state {
        *at = index - 1;
    }
    took(Effects::one(Effect::Apply { index: index - 1, caret: None }))
}

fn results(state: &mut Popover, ids: &[String]) -> Step {
    let history = state.is_history();
    let Some(list) = state.list_mut() else { return Step::default() };
    // Until the user picks a row, the highlight follows the first row as results arrive and
    // reorder; the history list keeps its row while the results hold it.
    let kept = match &list.active {
        Active::Chosen(id) => ids.contains(id).then(|| Active::Chosen(id.clone())),
        Active::First(id) if history => ids.contains(id).then(|| Active::First(id.clone())),
        _ => None,
    };
    let active = kept.unwrap_or_else(|| ids.first().map_or(Active::None, |id| Active::First(id.clone())));
    if active.id() == list.active.id() {
        return Step::default();
    }
    list.active = active;
    match (history, list.active.id()) {
        (true, Some(id)) => did(Effects::one(Effect::Preview(id.to_owned()))),
        _ => Step::default(),
    }
}

fn pointed(state: &mut Popover, id: &str) -> Step {
    let history = state.is_history();
    let Some(list) = state.list_mut() else { return Step::default() };
    if list.active == Active::Chosen(id.to_owned()) {
        return Step::default();
    }
    list.active = Active::Chosen(id.to_owned());
    if history { did(Effects::one(Effect::Preview(id.to_owned()))) } else { Step::default() }
}

fn picked(state: &mut Popover, pick: Pick) -> Step {
    match pick {
        Pick::History(index) => {
            if !state.is_history() {
                return Step::default();
            }
            *state = Popover::Closed;
            took(Effects::one(Effect::Apply { index, caret: None }))
        }
        Pick::Command(id) => {
            let menu = matches!(state, Popover::CommandMenu(_));
            *state = Popover::Closed;
            did(Effects::two(Effect::Command { id, menu }, Effect::FocusEditor))
        }
        Pick::Mention(key) => {
            *state = Popover::Closed;
            did(Effects::two(Effect::Mention(key), Effect::FocusEditor))
        }
    }
}

fn key_down(state: &mut Popover, key: Key, ids: &[String], composing: bool) -> Step {
    if let Popover::History { .. } = state
        && matches!(key, Key::Escape | Key::Cancel)
    {
        let Popover::History { saved, .. } = std::mem::take(state) else { unreachable!("history above") };
        return took(Effects::one(Effect::Restore { saved, focus: true }));
    }
    let Some(active) = state.list().map(|l| l.active.id().map(str::to_owned)) else { return Step::default() };
    // Backspace on an empty search backs out of the commands menu, as deleting the slash
    // closes inline commands.
    let menu_backs_out = key == Key::Backspace && matches!(state, Popover::CommandMenu(l) if l.query.is_empty());
    // Tab keeps the previewed entry as the draft.
    let closes = matches!(key, Key::Cancel | Key::Escape) || menu_backs_out || (state.is_history() && key == Key::Tab);
    if closes {
        *state = Popover::Closed;
        return took(Effects::one(Effect::FocusEditor));
    }
    if key == Key::Tab || (key == Key::Enter && !composing) {
        return took(active.map_or(Effects::none(), |id| Effects::one(Effect::Select(id))));
    }
    let Key::Step(direction) = key else { return Step::default() };
    if ids.is_empty() {
        return Step::default();
    }
    let current = active.and_then(|id| ids.iter().position(|x| *x == id));
    let len = ids.len() as i64;
    let next = match current {
        // The history list stops at its ends; the others wrap.
        Some(at) if state.is_history() => (at as i64 + direction).clamp(0, len - 1),
        Some(at) => (at as i64 + direction).rem_euclid(len),
        None if direction > 0 => 0,
        None => len - 1,
    } as usize;
    let id = ids[next].clone();
    let history = state.is_history();
    if let Some(list) = state.list_mut() {
        list.active = Active::Chosen(id.clone());
    }
    took(if history {
        Effects::two(Effect::Preview(id.clone()), Effect::Reveal(id))
    } else {
        Effects::one(Effect::Reveal(id))
    })
}

fn mode_synced(state: &mut Popover, shell: bool) -> Step {
    match state {
        Popover::Closed => Step::default(),
        // Entering shell mode cancels a history preview: its entry belongs to the prompt history.
        Popover::History { .. } if shell => {
            let Popover::History { saved, .. } = std::mem::take(state) else { unreachable!("history above") };
            did(Effects::one(Effect::Restore { saved, focus: false }))
        }
        // Leaving it keeps a list a typed `@` or `/` opened.
        Popover::Context(_) | Popover::CommandInline(_) | Popover::CommandMenu(_) if !shell => Step::default(),
        _ => {
            *state = Popover::Closed;
            Step::default()
        }
    }
}
