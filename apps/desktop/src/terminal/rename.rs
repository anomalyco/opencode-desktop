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

use opencode_i18n::t;
use rune::machine::{Attempt, Effects, Seq};
use rune::{El, Key, KeyEvent, PointerPress, TextEdit, Tx, text_input};

use crate::App;
use crate::jobs::Apply;
use crate::palette::Pal;
use crate::panels::panel_key;
use crate::popup::{self, Item, Menu};

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
    Save(Field),
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
            (Edit::Arming { at, .. } | Edit::Editing { at }, Event::Submit | Event::Blur) => Effects::one(Effect::Save(at)),
            (edit, _) => {
                self.edit = edit;
                Effects::none()
            }
        }
    }
}

/// Turns tab `id`'s title into a field holding it, all of it selected.
pub(super) fn start(s: &mut App, id: &str, _source: Source, tx: &mut Tx) {
    let Some(sid) = s.active_session_id().map(str::to_owned) else { return };
    let ws = s.terminal_workspace_key_for_session(&sid);
    let Some(tab) = s.terminal.workspaces.get(&ws).and_then(|w| w.tabs.iter().find(|t| t.id == id)) else { return };
    let title = tab.title.clone();
    let effects = s.terminal.renaming.step(Event::Start { ws, id: id.to_owned(), title });
    run(s, effects, tx);
}

/// The label's context menu (`Menu.Context`): Rename, and Close.
pub(super) fn menu(s: &mut App, id: &str, press: PointerPress, tx: &mut Tx) {
    tx.instant();
    let menu = Menu::new(t!("plugin.terminal.tab.title")).item(Item::new("rename", t!("common.rename"))).item(Item::new("close", t!("common.close")));
    let id = id.to_owned();
    popup::open(s, menu, press, tx, move |s, pick, tx| match pick {
        Some("rename") => start(s, &id, Source::Menu, tx),
        Some("close") => s.close_side_tab(&panel_key(super::side_ops::PLUGIN, &id), tx),
        _ => {}
    });
}

/// What the rename field draws from, for a view that redraws only when it changes: the tab and
/// its field.
pub(crate) fn stamp(s: &App, h: &mut impl std::hash::Hasher) {
    use std::hash::Hash;
    s.terminal.renaming.field().map(|r| (&r.id, &r.field)).hash(h);
}

/// The field over tab `id`'s title while it's renamed: from past the glyph to the label's end.
pub(super) fn field(s: &App, p: &Pal, id: &str) -> Option<El> {
    let r = s.terminal.renaming.field().filter(|r| r.id == id)?;
    Some(
        text_input(&r.field, crate::view::UI)
            .key("terminal-rename")
            .label(t!("common.rename"))
            .spellcheck(s.settings.spellcheck)
            .single_line()
            .pin(Some(20.0), Some(0.0), Some(0.0), Some(0.0))
            .px(4.0)
            .radius(4.0)
            .fill(p.layer1)
            .color(p.text)
            .caret(p.text, p.selection).selection_style(p.field_selection)
            .on_edit(|s: &mut App| {
                let _ = s.terminal.renaming.step(Event::Typed);
                &mut s.terminal.renaming.field_mut().expect("renaming").field
            })
            .on_submit(|s: &mut App, tx: &mut Tx| {
                let effects = s.terminal.renaming.step(Event::Submit);
                run(s, effects, tx);
            })
            .on_key(|s: &mut App, ev: &KeyEvent, tx: &mut Tx| {
                if ev.key != Key::Escape {
                    return false;
                }
                tx.instant();
                let _ = s.terminal.renaming.step(Event::Cancel);
                true
            }),
    )
}

/// The armed frame, and a click away, which saves as the field's blur does.
pub(super) fn poll(s: &mut App, tx: &mut Tx) -> bool {
    let event = match &mut s.terminal.renaming.edit {
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
    let effects = s.terminal.renaming.step(event);
    run(s, effects, tx);
    true
}

fn run(s: &mut App, effects: Effects<Effect>, tx: &mut Tx) {
    for effect in effects {
        match effect {
            Effect::Arm => {
                tx.instant();
                tx.wake_after(std::time::Duration::ZERO);
            }
            Effect::Save(at) => save(s, at, tx),
        }
    }
}

/// Sets the terminal's title on the server, showing it at once; a field emptied clears it so
/// the shell's title shows again. The title already shown, or clearing one never set, sends
/// nothing; a failure puts the old title back and says why. Only the newest rename of a
/// terminal settles it.
fn save(s: &mut App, r: Field, tx: &mut Tx) {
    tx.instant();
    let typed = r.field.text.trim().to_owned();
    let Some(tab) = s.terminal.workspaces.get_mut(&r.ws).and_then(|w| w.tabs.iter_mut().find(|t| t.id == r.id)) else { return };
    let title = (!typed.is_empty()).then_some(typed);
    if title.as_deref() == Some(tab.title.as_str()) || (title.is_none() && !tab.custom_title) {
        return;
    }
    let before = (tab.title.clone(), tab.custom_title);
    if let Some(t) = &title {
        tab.title = t.clone();
        tab.custom_title = true;
    }
    let Some((srv, _)) = s.live_terminals() else { return };
    let attempt = s.terminal.renaming.seq.next();
    // A rename already in flight keeps the title it replaced, to put back if this one fails.
    let before = s.terminal.renaming.saving.get(&r.id).map_or(before, |(_, b)| b.clone());
    s.terminal.renaming.saving.insert(r.id.clone(), (attempt, before));
    let (ws, id) = (r.ws, r.id);
    s.job(move |_| {
        let answer = srv.terminal_rename(&id, title.as_deref());
        Box::new(move |s: &mut App, tx: &mut Tx| {
            if s.terminal.renaming.saving.get(&id).is_none_or(|(a, _)| *a != attempt) {
                return;
            }
            let Some((_, before)) = s.terminal.renaming.saving.remove(&id) else { return };
            tx.instant();
            let Some(tab) = s.terminal.workspaces.get_mut(&ws).and_then(|w| w.tabs.iter_mut().find(|t| t.id == id)) else { return };
            match answer {
                Ok(info) => {
                    tab.title = info.title;
                    tab.custom_title = info.user_title.is_some();
                }
                Err(e) => {
                    (tab.title, tab.custom_title) = before;
                    crate::status_toast::fail(s, t!("plugin.terminal.rename.failed"), e);
                }
            }
            s.queue_terminal_save();
        }) as Apply
    });
}
