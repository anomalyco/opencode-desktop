//! A terminal tab's inline rename (`TerminalTabLabel`, `inline-rename.ts`): a double-click on
//! its title or the label's context menu turns the title into a field over it, Enter or a click
//! away saves it on the server, Escape drops it. An emptied field gives back the shell's title.
//!
//! The edit is Desktop's `gui-plugins/src/terminal/rename-machine.ts` (`RenameState`): `Arming`
//! shows the field while focus settles (the double-click or the closing menu still moves it, so
//! a blur then doesn't end the edit), and the next frame arms it; `Editing` saves on blur. Each
//! save takes an attempt (`machine.ts` `Mutations.renames`), so only the newest answer for a
//! terminal lands.

use std::collections::HashMap;
use std::hash::{Hash, Hasher};

use opencode_i18n::t;
use opencode_kit::persistent_pty::PersistentPtyInfo;
use rune::machine::{Attempt, Effects, Seq};
use rune::{El, Key, KeyEvent, PointerPress, TextEdit, Tx};
use opencode_ui::palette::Pal;
use opencode_ui::style::UI;

use crate::{ContextMenuEntry, ContextMenuSpec, TerminalJob, TerminalSurfaceHost, ops, panel_key, persist};

/// What started a rename (`RenameSource`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    Pointer,
    Menu,
}

/// The tab being renamed and its field.
#[derive(Debug)]
pub struct Field {
    ws: String,
    id: String,
    field: TextEdit,
}

#[derive(Debug, Default)]
pub enum Edit {
    #[default]
    Idle,
    /// The field shows; its first frame arms it (`arming`). `edited`: typed before it armed, so
    /// arming leaves the text unselected.
    Arming { at: Field, attempt: Attempt, edited: bool },
    /// A blur saves (`editing`).
    Editing { at: Field },
}

/// The inline edit, and each terminal's rename in flight with the title it replaced.
#[derive(Debug, Default)]
pub struct Renames {
    seq: Seq,
    edit: Edit,
    saving: HashMap<String, (Attempt, (String, bool))>,
}

enum Event {
    Start { ws: String, id: String, title: String },
    /// The armed frame: the field holds focus from now on.
    Armed { attempt: Attempt },
    Typed,
    Submit,
    Blur,
    Cancel,
}

enum Effect {
    /// Arm the field on the next frame (`arm`).
    Arm,
    Save(Box<Field>),
}

impl Renames {
    fn field(&self) -> Option<&Field> {
        match &self.edit {
            Edit::Idle => None,
            Edit::Arming { at, .. } | Edit::Editing { at } => Some(at),
        }
    }

    fn field_mut(&mut self) -> Option<&mut Field> {
        match &mut self.edit {
            Edit::Idle => None,
            Edit::Arming { at, .. } | Edit::Editing { at } => Some(at),
        }
    }

    pub(crate) fn on_typed_field_mut(&mut self) -> Option<&mut TextEdit> {
        let _ = self.step(Event::Typed);
        self.field_mut().map(|f| &mut f.field)
    }

    fn needs_poll(&self) -> bool {
        !matches!(&self.edit, Edit::Idle) && !matches!(&self.edit, Edit::Editing { at } if at.field.focused)
    }

    /// `transitionRename`.
    fn step(&mut self, event: Event) -> Effects<Effect> {
        match (std::mem::take(&mut self.edit), event) {
            // A rename already open keeps its field, its draft and its blur handling.
            (Edit::Idle, Event::Start { ws, id, title }) => {
                let mut field = TextEdit::new(&title);
                field.set_selection(0, field.text.len());
                field.focused = true;
                self.edit = Edit::Arming { at: Field { ws, id, field }, attempt: self.seq.next(), edited: false };
                Effects::one(Effect::Arm)
            }
            (Edit::Arming { at, attempt, .. }, Event::Armed { attempt: a }) if attempt == a => {
                self.edit = Edit::Editing { at };
                Effects::none()
            }
            (Edit::Arming { at, attempt, .. }, Event::Typed) => {
                self.edit = Edit::Arming { at, attempt, edited: true };
                Effects::none()
            }
            (edit @ Edit::Arming { .. }, Event::Blur) => {
                self.edit = edit;
                Effects::none()
            }
            (Edit::Arming { .. } | Edit::Editing { .. }, Event::Cancel) => Effects::none(),
            (Edit::Arming { at, .. } | Edit::Editing { at }, Event::Submit | Event::Blur) => {
                Effects::one(Effect::Save(Box::new(at)))
            }
            (edit, _) => {
                self.edit = edit;
                Effects::none()
            }
        }
    }
}

/// Turns tab `id`'s title into a field holding it, all of it selected.
pub fn start(host: &mut dyn TerminalSurfaceHost, id: &str, _source: Source, tx: &mut Tx) {
    let Some(sid) = host.active_session_id().map(str::to_owned) else { return };
    let ws = ops::terminal_workspace_key_for_session(host, &sid);
    let Some(title) = host
        .terminal()
        .workspaces
        .get(&ws)
        .and_then(|w| w.tabs.iter().find(|t| t.id == id))
        .map(|t| t.title.clone())
    else {
        return;
    };
    let effects = host.terminal_mut().renaming.step(Event::Start { ws, id: id.to_owned(), title });
    run(host, effects, tx);
}

/// The label's context menu (`Menu.Context`): Rename, and Close.
pub fn menu(host: &mut dyn TerminalSurfaceHost, id: &str, press: PointerPress, tx: &mut Tx) {
    tx.instant();
    let spec = ContextMenuSpec {
        label: t!("plugin.terminal.tab.title"),
        entries: vec![
            ContextMenuEntry::Item { id: "rename", label: t!("common.rename"), enabled: true },
            ContextMenuEntry::Item { id: "close", label: t!("common.close"), enabled: true },
        ],
    };
    let id = id.to_owned();
    host.open_context_menu(
        spec,
        press,
        tx,
        Box::new(move |host, pick, tx| match pick {
            Some("rename") => start(host, &id, Source::Menu, tx),
            Some("close") => close(host, &id, tx),
            _ => {}
        }),
    );
}

/// The label menu's Close (`terminal().close(id)`, `terminal/model.ts:551-553`): ends the shell on
/// the server and the tab leaves as an exited terminal's does, so the panel stays open on a New
/// tab when it was the last, where closing the panel's tab would shut the panel.
fn close(host: &mut dyn TerminalSurfaceHost, id: &str, tx: &mut Tx) {
    let ws = host
        .terminal()
        .workspaces
        .iter()
        .find(|(_, w)| w.tabs.iter().any(|t| t.id == id))
        .map(|(k, _)| k.clone());
    match ws {
        Some(ws) => ops::close_terminal_tab(host, &ws, id, tx),
        None => host.close_side_tab(&panel_key(id), tx),
    }
}

/// What the rename field draws from, for a view that redraws only when it changes: the tab and
/// its field.
pub fn stamp(host: &dyn TerminalSurfaceHost, h: &mut impl Hasher) {
    host.terminal().renaming.field().map(|r| (&r.id, &r.field)).hash(h);
}

fn on_submit(host: &mut dyn TerminalSurfaceHost, tx: &mut Tx) {
    let effects = host.terminal_mut().renaming.step(Event::Submit);
    run(host, effects, tx);
}

fn on_key(host: &mut dyn TerminalSurfaceHost, ev: &KeyEvent, tx: &mut Tx) -> bool {
    if ev.key != Key::Escape {
        return false;
    }
    tx.instant();
    let _ = host.terminal_mut().renaming.step(Event::Cancel);
    true
}

/// The field over tab `id`'s title while it's renamed: from past the glyph to the label's end.
pub fn field<H: TerminalSurfaceHost + 'static>(s: &H, p: &Pal, id: &str) -> Option<El> {
    let r = s.terminal().renaming.field().filter(|r| r.id == id)?;
    Some(
        field_shell(p, &r.field, s.spellcheck())
            .on_edit(|s: &mut H| s.terminal_mut().rename_field_on_edit())
            .on_submit(|s: &mut H, tx: &mut Tx| on_submit(s, tx))
            .on_key(|s: &mut H, ev: &KeyEvent, tx: &mut Tx| on_key(s, ev, tx)),
    )
}

fn field_shell(p: &Pal, edit: &TextEdit, spellcheck: bool) -> El {
    opencode_ui::style::field(edit, UI)
        .key("terminal-rename")
        .label_static(t!("common.rename"))
        .spellcheck(spellcheck)
        .single_line()
        .pin(Some(20.0), Some(0.0), Some(0.0), Some(0.0))
        .px(4.0)
        .radius(4.0)
        .fill(p.layer1)
        .color(p.text)
        .caret(p.text, p.selection)
        .selection_style(p.field_selection)
}

/// The armed frame, and a click away, which saves as the field's blur does.
pub fn poll(host: &mut dyn TerminalSurfaceHost, tx: &mut Tx) -> bool {
    // Every poll comes here: the terminal is written only for an arming rename or a blur.
    if !host.terminal().renaming.needs_poll() {
        return false;
    }
    let event = match &mut host.terminal_mut().renaming.edit {
        // `arm`'s frame: focus that settled elsewhere comes back to the field first.
        Edit::Arming { at, attempt, edited } => {
            if !at.field.focused {
                at.field.focused = true;
                if !*edited {
                    at.field.set_selection(0, at.field.text.len());
                }
            }
            Event::Armed { attempt: *attempt }
        }
        Edit::Editing { at } if !at.field.focused => Event::Blur,
        Edit::Idle | Edit::Editing { .. } => return false,
    };
    let effects = host.terminal_mut().renaming.step(event);
    run(host, effects, tx);
    true
}

fn run(host: &mut dyn TerminalSurfaceHost, effects: Effects<Effect>, tx: &mut Tx) {
    for effect in effects {
        match effect {
            Effect::Arm => {
                tx.instant();
                tx.wake_after(std::time::Duration::ZERO);
            }
            Effect::Save(at) => save(host, *at, tx),
        }
    }
}

/// Sets the terminal's title on the server, showing it at once; a field emptied clears it so
/// the shell's title shows again. The title already shown, or clearing one never set, sends
/// nothing; a failure puts the old title back and says why. Only the newest rename of a
/// terminal settles it.
fn save(host: &mut dyn TerminalSurfaceHost, r: Field, tx: &mut Tx) {
    tx.instant();
    let typed = r.field.text.trim().to_owned();
    let title = (!typed.is_empty()).then_some(typed);
    let before = {
        let Some(tab) = ops::terminal_tab_mut(host, &r.ws, &r.id) else { return };
        if title.as_deref() == Some(tab.title.as_str()) || (title.is_none() && !tab.custom_title) {
            return;
        }
        let before = (tab.title.clone(), tab.custom_title);
        if let Some(t) = &title {
            tab.title = t.clone();
            tab.custom_title = true;
        }
        before
    };
    if ops::live_terminals(host).is_none() {
        return;
    }
    let attempt = {
        let renaming = &mut host.terminal_mut().renaming;
        let attempt = renaming.seq.next();
        // A rename already in flight keeps the title it replaced, to put back if this one fails.
        let before = renaming.saving.get(&r.id).map_or(before, |(_, b)| b.clone());
        renaming.saving.insert(r.id.clone(), (attempt, before));
        attempt
    };
    host.spawn_job(TerminalJob::Rename { ws: r.ws, pty_id: r.id, title, attempt });
}

pub(crate) fn apply_rename_reply(
    host: &mut dyn TerminalSurfaceHost,
    ws: &str,
    id: &str,
    attempt: Attempt,
    answer: Result<PersistentPtyInfo, String>,
    tx: &mut Tx,
) {
    if host.terminal().renaming.saving.get(id).is_none_or(|(a, _)| *a != attempt) {
        return;
    }
    let Some((_, before)) = host.terminal_mut().renaming.saving.remove(id) else { return };
    tx.instant();
    let failed = {
        let Some(tab) = ops::terminal_tab_mut(host, ws, id) else { return };
        match answer {
            Ok(info) => {
                tab.title = info.title;
                tab.custom_title = info.user_title.is_some();
                None
            }
            Err(e) => {
                (tab.title, tab.custom_title) = before;
                Some(e)
            }
        }
    };
    if let Some(e) = failed {
        host.toast_fail(t!("plugin.terminal.rename.failed").to_owned(), e);
    }
    persist::queue_terminal_save(host);
}
