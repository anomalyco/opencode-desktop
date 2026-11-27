//! Forking a session from a user message, and the `"Fork from message"` picker dialog, matching
//! OpenCode Desktop (`packages/app/src/session/fork.ts`,
//! `packages/app/src/session/commands/fork-dialog.tsx`).
//!
//! `POST /api/session/:id/fork` with `{"before": messageID}` creates a new root session holding
//! the settled history strictly before `messageID`. Once created, it opens as a tab with that
//! prompt back in its composer ready to edit and send. One fork runs at a time (`machine.rs`).

pub mod machine;

use std::time::Duration;

use opencode_kit::client::{Ticket, TicketKind, Write};
use opencode_kit::mention;
use opencode_kit::model::{Message, SessionInfo, Transcript};
use opencode_kit::wire_error::WireError;
use opencode_search::query::cycle;
use opencode_ui::widgets::{DialogSize, dialog_window};
use rune::machine::Attempt;
use rune::{Color, Cx, El, Key, KeyEvent, LocalTime, Spring, TextEdit, Transition, Tx, col, el, icon, row, text};
use rune_theme_opencode::icons as ic;
use session_view::palette::{Elevate as _, Pal};
use session_view::scroller::{Bar, scroller};
use session_view::style::{INSTANT, UI, activity_ring_sized, field};

use crate::rewind::{SavedDraft, draft_from_message_with};
use machine::{Effect, Event, Surface};

/// Overlay stack identifier for the `"Fork from message"` dialog (`opencode_kernel::OverlayStack`).
pub const FORK_OVERLAY_ID: &str = "dialog.fork";

#[derive(Clone, Debug)]
pub struct ForkableMessage {
    pub id: String,
    pub text: String,
    pub time: String,
}

/// `"Fork from message"` dialog lifecycle, replacing the parallel `open`, `active`,
/// `restore_composer`, and `slow` fields so stale modal state can never survive across close/reopen.
#[derive(Clone, Debug, Default)]
pub enum ForkModal {
    #[default]
    Closed,
    Open {
        active: Option<String>,
        restore_composer: bool,
        /// The fork on its way has taken longer than [`SLOW`]: its row says so.
        slow: bool,
    },
}

#[derive(Default)]
pub struct ForkState {
    pub modal: ForkModal,
    pub search: TextEdit,
    /// The fork on its way (`machine.rs`, `session/fork-machine.ts`).
    machine: machine::Fork,
    /// Fixture forks made so far, for their ids.
    made: u32,
}

/// How long a fork runs before its row says the server is still working on it.
const SLOW: f64 = 5.0;

/// Clock time at which a fork timing out at `due` crosses the [`SLOW`] wait threshold.
fn slow_at(due: f64) -> f64 {
    due - machine::TIMEOUT_SECS + SLOW
}

impl ForkState {
    /// The prompt a fork is on its way from: the picker holds still and shows it pending.
    pub fn pending(&self) -> Option<&str> {
        self.machine.pending()
    }

    pub fn is_open(&self) -> bool {
        matches!(self.modal, ForkModal::Open { .. })
    }

    pub fn is_focused(&self) -> bool {
        self.is_open() && self.search.focused
    }

    pub fn restore_composer(&self) -> bool {
        match self.modal {
            ForkModal::Open { restore_composer, .. } => restore_composer,
            ForkModal::Closed => false,
        }
    }

    pub fn active(&self) -> Option<&str> {
        match &self.modal {
            ForkModal::Open { active, .. } => active.as_deref(),
            ForkModal::Closed => None,
        }
    }
}

/// Object-safe host boundary for forking a session from a user message and the `"Fork from message"` dialog.
pub trait ForkHost: opencode_kernel::AppEnv + opencode_kernel::AppFeedback + opencode_kernel::AppOverlays {
    fn fork(&self) -> &ForkState;
    fn fork_mut(&mut self) -> &mut ForkState;

    /// Whether `session` is currently the active route (`Route::Session { id }` == `session`).
    fn is_session_open(&self, session: &str) -> bool {
        self.active_session_id() == Some(session)
    }

    /// Metadata for `session`, if known.
    fn session(&self, session: &str) -> Option<&SessionInfo>;
    /// Updates an existing session entry by ID or inserts `session` at the front of the list.
    fn upsert_session_front(&mut self, session: SessionInfo);
    /// Loaded transcript for `session`, if any.
    fn transcript(&self, session: &str) -> Option<&Transcript>;
    /// Inserts a newly synthesized local transcript (used for fixture/storybook forks).
    fn insert_transcript(&mut self, session: String, transcript: Transcript);

    /// Opens session `session` in the active view.
    fn open_session(&mut self, session: &str, tx: &mut Tx);
    /// Applies `draft` to `session`'s composer (live if active, or parked for when it opens).
    fn apply_draft(&mut self, session: &str, draft: SavedDraft);
    /// Display title for a `@session:<id>` mention pill when restoring a prompt draft.
    fn mention_title(&self, _session_id: &str) -> Option<String> {
        None
    }
    /// Resolves a skill name or id to its canonical skill id when restoring `@skill` pills.
    fn resolve_skill_id(&self, _name_or_id: &str) -> Option<&str> {
        None
    }

    /// Whether the host runs in fixture or storybook mode (forks are synthesized locally).
    fn is_simulated(&self) -> bool {
        false
    }
    /// Dispatches a tracked server write; returns `false` when the write could not be sent.
    fn send_write(&mut self, _write: Write) -> bool {
        false
    }
    /// Current status line text used as the fallback reason when [`Self::send_write`] returns `false`.
    fn status_text(&self) -> &str {
        ""
    }
    /// Formats a server [`WireError`] into a user-facing message.
    fn format_wire_error(&self, err: &WireError) -> String {
        err.to_string()
    }

    /// Prepares the shell before opening the Fork dialog (closes conflicting dialogs/pickers/menus,
    /// blurs the composer) and returns whether composer focus should be restored on dismiss.
    fn before_fork_open(&mut self) -> bool {
        false
    }
    /// Restores composer focus to `focused` when the Fork dialog closes with `restore_focus == true`.
    fn restore_composer_focus(&mut self, _focused: bool) {}
    /// Fallback handler for unhandled keys inside the Fork dialog search field.
    fn handle_shortcut(&mut self, _ev: &KeyEvent, _tx: &mut Tx) -> bool {
        false
    }
}

/// Server-style fork title (`projector.ts` `forkTitle`): `"Title (fork #1)"` → `"Title (fork #2)"`.
pub fn fork_title(title: &str) -> String {
    let base = if title.trim().is_empty() { "Untitled session" } else { title.trim() };
    if let Some((stem, rest)) = base.rsplit_once(" (fork #")
        && let Some(num) = rest.strip_suffix(')').and_then(|n| n.parse::<u32>().ok())
    {
        return format!("{stem} (fork #{})", num + 1);
    }
    format!("{base} (fork #1)")
}

fn is_root_session(host: &dyn ForkHost, session: &str) -> bool {
    host.session(session).is_none_or(|i| i.parent.is_none())
}

fn has_visible_user_message(host: &dyn ForkHost, session: &str) -> bool {
    let revert = host.session(session).and_then(|i| i.revert.as_deref());
    let Some(t) = host.transcript(session) else { return false };
    let shown = t.visible_len(revert);
    t.messages[..shown].iter().any(|m| matches!(m, Message::User { .. }))
}

/// Whether `/fork` and `⌘K → Fork from message` are enabled on the open session.
pub fn can_fork_session(host: &dyn ForkHost) -> bool {
    let Some(id) = host.active_session_id() else { return false };
    is_root_session(host, id) && has_visible_user_message(host, id)
}

/// Forks `session` before user message `message_id`, opens the new session, and puts the
/// message's prompt back into its composer (`useSessionFork`). One fork at a time: another
/// press waits for it. False when none started.
pub fn fork_from(host: &mut dyn ForkHost, session: &str, message_id: &str, tx: &mut Tx) -> bool {
    if !is_root_session(host, session) {
        return false;
    }
    let Some(body) = host.transcript(session).and_then(|t| {
        t.messages.iter().find_map(|m| match m {
            Message::User { id, text } if id == message_id => Some(text.clone()),
            _ => None,
        })
    }) else {
        return false;
    };
    let draft = {
        let h: &dyn ForkHost = host;
        draft_from_message_with(message_id, &body, |sid| h.mention_title(sid), |val| h.resolve_skill_id(val).map(str::to_owned))
    };
    let surface = match host.fork().modal {
        ForkModal::Open { .. } => Surface::Dialog,
        ForkModal::Closed => Surface::Inline,
    };
    let now = host.clock();
    let event = Event::Requested { surface, session: session.to_owned(), message: message_id.to_owned(), draft, now };
    step_fork(host, event, tx)
}

/// Steps the fork machine and carries out what it says. True when it changed anything.
fn step_fork(host: &mut dyn ForkHost, event: Event, tx: &mut Tx) -> bool {
    let effects = host.fork_mut().machine.step(event);
    let changed = !effects.is_empty();
    for effect in effects {
        match effect {
            Effect::Start { attempt, session, message } => {
                if let ForkModal::Open { slow, .. } = &mut host.fork_mut().modal {
                    *slow = false;
                }
                // The fixture and the storybook fork at once, as a server would.
                if host.is_simulated() {
                    let forked = create_fixture_fork(host, &session, &message);
                    step_fork(host, Event::Succeeded { attempt, forked }, tx);
                    continue;
                }
                let ticket = Ticket { kind: TicketKind::Fork, session: session.clone(), attempt: attempt.0 };
                let write = Write::Fork { session, before: Some(message) };
                if !host.send_write(Write::Tracked { ticket, write: Box::new(write) }) {
                    let why = host.status_text().to_owned();
                    step_fork(host, Event::Failed { attempt, why }, tx);
                }
            }
            Effect::Open { forked, draft } => {
                host.open_session(&forked, tx);
                host.apply_draft(&forked, draft);
            }
            Effect::CloseDialog => close(host, false),
            Effect::Fail { why } => {
                let why = why.split_once(": ").map_or(why.as_str(), |(_, why)| why).to_owned();
                host.toast_fail(opencode_i18n::t!("common.requestFailed").to_owned(), why);
            }
        }
    }
    changed
}

fn create_fixture_fork(host: &mut dyn ForkHost, session: &str, before_message: &str) -> String {
    let made = {
        let f = host.fork_mut();
        f.made += 1;
        f.made
    };
    let forked_id = format!("ses_fx_fork_{made:04}");
    let now = host.now_ms();
    let info = match host.session(session).cloned() {
        Some(parent) => SessionInfo {
            id: forked_id.clone(),
            parent: None,
            project: parent.project,
            title: fork_title(&parent.title),
            directory: parent.directory,
            updated: now,
            model: parent.model,
            cost: 0.0,
            idle: Some(now),
            viewed: Some(now),
            agent: parent.agent,
            created: now,
            outcome: None,
            revert: None,
            ..Default::default()
        },
        None => SessionInfo {
            id: forked_id.clone(),
            title: fork_title(""),
            updated: now,
            created: now,
            ..Default::default()
        },
    };
    let copied: Vec<Message> = host
        .transcript(session)
        .map(|t| {
            let end = t.visible_len(Some(before_message));
            t.messages[..end]
                .iter()
                .filter(|m| !matches!(m, Message::Assistant(a) if a.completed.is_none()))
                .cloned()
                .collect()
        })
        .unwrap_or_default();
    host.upsert_session_front(info);
    host.insert_transcript(forked_id.clone(), Transcript { session: forked_id.clone(), messages: copied });
    forked_id
}

/// Opens Desktop's `"Fork from message"` dialog for the open session.
pub fn open(host: &mut dyn ForkHost, tx: &mut Tx) -> bool {
    if !can_fork_session(host) {
        return false;
    }
    tx.instant();
    let (was_open, prev_restore) = match host.fork().modal {
        ForkModal::Open { restore_composer, .. } => (true, restore_composer),
        ForkModal::Closed => (false, false),
    };
    let shell_restore = host.before_fork_open();
    let restore_composer = if was_open { prev_restore } else { shell_restore };
    let return_focus = tx.focused().map(str::to_owned);
    let first = messages(host).first().map(|m| m.id.clone());
    let now = host.clock();
    let slow = match &host.fork().machine.state {
        machine::State::Forking { session, due, .. } if host.is_session_open(session) => now + 1e-9 >= slow_at(*due),
        machine::State::Forking { .. } | machine::State::Idle => false,
    };
    {
        let f = host.fork_mut();
        f.modal = ForkModal::Open { active: first, restore_composer, slow };
        f.search = TextEdit::new("");
        f.search.focused = true;
    }
    if !was_open {
        host.on_overlay_opened(FORK_OVERLAY_ID, return_focus);
    }
    true
}

/// Closes the `"Fork from message"` dialog, notifying `OverlayStack` and restoring composer focus when asked.
pub fn close(host: &mut dyn ForkHost, restore_focus: bool) {
    let restore = match host.fork().modal {
        ForkModal::Open { restore_composer, .. } => restore_composer,
        ForkModal::Closed => return,
    };
    {
        let f = host.fork_mut();
        // The picker's fork goes with it (`fork-machine.ts` `disposed`).
        if matches!(f.machine.state, machine::State::Forking { surface: Surface::Dialog, .. }) {
            drop(f.machine.step(Event::Left));
        }
        f.modal = ForkModal::Closed;
        f.search.focused = false;
        f.search.cancel_composition();
    }
    host.on_overlay_closed(FORK_OVERLAY_ID);
    if restore_focus {
        host.restore_composer_focus(restore);
    }
}

/// All non-empty user messages of the open session, newest first (`fork-dialog.tsx`).
pub fn messages(host: &dyn ForkHost) -> Vec<ForkableMessage> {
    let Some(session) = host.active_session_id() else { return Vec::new() };
    let revert = host.session(session).and_then(|i| i.revert.as_deref());
    let Some(t) = host.transcript(session) else { return Vec::new() };
    let shown = t.visible_len(revert);
    let mut out: Vec<ForkableMessage> = t.messages[..shown]
        .iter()
        .filter_map(|m| match m {
            Message::User { id, text } if !text.trim().is_empty() => {
                let preview: String = text.replace('\n', " ").chars().take(200).collect();
                Some(ForkableMessage { id: id.clone(), text: preview, time: short_time(id) })
            }
            _ => None,
        })
        .collect();
    out.reverse();
    out
}

/// Filtered messages matching `fork.search`.
pub fn filtered(host: &dyn ForkHost) -> Vec<ForkableMessage> {
    let q = host.fork().search.text.trim();
    let all = messages(host);
    if q.is_empty() {
        return all;
    }
    let fold = rune_kit::search::Fold::for_language(opencode_i18n::locale().tag());
    all.into_iter().filter(|m| fold.contains(&m.text, q)).collect()
}

/// Key handler while the Fork dialog is open: Escape closes, arrows cycle rows, Enter forks.
pub fn key(host: &mut dyn ForkHost, ev: &KeyEvent, tx: &mut Tx) -> bool {
    match host.fork().modal {
        ForkModal::Closed => return false,
        ForkModal::Open { .. } => {}
    }
    if host.fork().search.is_composing() {
        return false;
    }
    let list = filtered(host);
    match ev.key {
        Key::Escape => {
            tx.instant();
            close(host, true);
            true
        }
        // While a fork runs the list holds still (`fork-dialog.tsx:97-103`).
        Key::Down | Key::Up | Key::Enter if host.fork().pending().is_some() => true,
        Key::Down | Key::Up if !ev.mods.shift && !ev.mods.ctrl && !ev.mods.cmd && !ev.mods.alt => {
            tx.instant();
            if !list.is_empty() {
                let cur = match &host.fork().modal {
                    ForkModal::Open { active, .. } => active.as_ref().and_then(|id| list.iter().position(|m| &m.id == id)),
                    ForkModal::Closed => None,
                };
                let next = cycle(list.len(), cur, if ev.key == Key::Down { 1 } else { -1 });
                let id = list[next].id.clone();
                if next == 0 {
                    tx.scroll_to_y("fork-scroll", 0.0);
                } else {
                    tx.scroll_into_view_in("fork-scroll", format!("fork-row-{id}"));
                }
                match &mut host.fork_mut().modal {
                    ForkModal::Open { active, .. } => *active = Some(id),
                    ForkModal::Closed => {}
                }
            }
            true
        }
        Key::Enter if !ev.mods.shift => {
            tx.instant();
            let Some(session) = host.active_session_id().map(str::to_owned) else { return true };
            let chosen = match &host.fork().modal {
                ForkModal::Open { active, .. } => active.as_ref().and_then(|id| list.iter().find(|m| &m.id == id)),
                ForkModal::Closed => None,
            }
            .or_else(|| list.first())
            .map(|m| m.id.clone());
            if let Some(mid) = chosen {
                fork_from(host, &session, &mid, tx);
            }
            true
        }
        _ => false,
    }
}

/// A fork's `Incoming::Settled`: the session it made is remembered, and opens with the forked
/// prompt in its composer when this fork is still the one on its way.
pub fn settled(host: &mut dyn ForkHost, attempt: u32, result: Result<Option<SessionInfo>, WireError>, tx: &mut Tx) {
    let attempt = Attempt(attempt);
    let event = match result {
        Ok(Some(forked)) => {
            let id = forked.id.clone();
            host.upsert_session_front(forked);
            Event::Succeeded { attempt, forked: id }
        }
        Ok(None) => Event::Failed { attempt, why: String::new() },
        Err(e) => Event::Failed { attempt, why: host.format_wire_error(&e) },
    };
    step_fork(host, event, tx);
}

/// Each frame: a fork past its 30 s bound fails, and one whose session isn't open any more is
/// dropped. True when either happened.
pub fn poll(host: &mut dyn ForkHost, tx: &mut Tx) -> bool {
    let (session, due) = match &host.fork().machine.state {
        machine::State::Forking { session, due, .. } => (session.clone(), *due),
        machine::State::Idle => {
            return match host.fork().modal {
                ForkModal::Open { slow: true, .. } => {
                    match &mut host.fork_mut().modal {
                        ForkModal::Open { slow, .. } => *slow = false,
                        ForkModal::Closed => {}
                    }
                    true
                }
                ForkModal::Open { slow: false, .. } | ForkModal::Closed => false,
            };
        }
    };
    if !host.is_session_open(&session) {
        return step_fork(host, Event::Left, tx);
    }
    let now = host.clock();
    if now + 1e-9 >= due {
        return step_fork(host, Event::Due { now }, tx);
    }
    let at = slow_at(due);
    let next_slow = now + 1e-9 >= at;
    let changed = match host.fork().modal {
        ForkModal::Open { slow, .. } if slow != next_slow => {
            match &mut host.fork_mut().modal {
                ForkModal::Open { slow, .. } => *slow = next_slow,
                ForkModal::Closed => {}
            }
            true
        }
        ForkModal::Open { .. } | ForkModal::Closed => false,
    };
    let wait = if next_slow { due - now } else { at - now };
    tx.wake_after(Duration::from_secs_f64(wait.max(0.0)));
    changed
}

/// When a prompt was sent, as `toLocaleTimeString(undefined, { timeStyle: "short" })` reads it:
/// "6:03 AM" in the system's locale rather than the chosen language (`fork-dialog.tsx`).
fn short_time(id: &str) -> String {
    let ms = mention::user_created(id).or_else(|| {
        let hex = id.strip_prefix("msg_")?.get(..12)?;
        u64::from_str_radix(hex, 16).ok().map(|v| (v >> 12) as i64).filter(|&v| v > 1_500_000_000_000)
    });
    let moment = |ms| session_view::turn_errors::moment(LocalTime::at(ms));
    ms.map(|ms| opencode_i18n::date::format_in(opencode_i18n::system_locale(), opencode_i18n::date::Style::Time, moment(ms)))
        .unwrap_or_default()
}

// --- Thin non-generic callback helpers invoked by `view` ---

fn set_modal_active(host: &mut dyn ForkHost, next: Option<String>) {
    match &mut host.fork_mut().modal {
        ForkModal::Open { active, .. } => *active = next,
        ForkModal::Closed => {}
    }
}

fn on_query_change(host: &mut dyn ForkHost, tx: &mut Tx) {
    tx.instant();
    let first = filtered(host).first().map(|m| m.id.clone());
    set_modal_active(host, first);
}

fn key_or_shortcut(host: &mut dyn ForkHost, ev: &KeyEvent, tx: &mut Tx) -> bool {
    key(host, ev, tx) || host.handle_shortcut(ev, tx)
}

fn clear_search(host: &mut dyn ForkHost, tx: &mut Tx) {
    tx.instant();
    {
        let f = host.fork_mut();
        f.search = TextEdit::new("");
        f.search.focused = true;
    }
    let first = filtered(host).first().map(|m| m.id.clone());
    set_modal_active(host, first);
}

fn on_row_hover(host: &mut dyn ForkHost, id: &str, inside: bool, tx: &mut Tx) {
    if host.fork().pending().is_some() {
        return;
    }
    tx.instant();
    set_modal_active(host, inside.then(|| id.to_owned()));
}

fn on_row_press(host: &mut dyn ForkHost, sid: &str, mid: &str, tx: &mut Tx) {
    tx.instant();
    fork_from(host, sid, mid, tx);
}

/// The search block (`.command-palette-compact [data-slot="search-list-search"]`,
/// `shell/commands/dialog.css:145-158`): 6 above the 28-tall field, 6 to its 1 pt `::after`
/// hairline, 2 under it.
const HEAD_H: f32 = 6.0 + 28.0 + 6.0 + 1.0 + 2.0;
/// `fork-dialog-section` (`shell/search-list.css:249-263`): 10 above and 2 below its 18-tall line.
const SECTION_H: f32 = 10.0 + 18.0 + 2.0;
/// A row (`.fork-dialog [data-slot="search-list-row"]`, 32 tall), and the 2 between rows.
const ROW_H: f32 = 32.0;
const ROW_GAP: f32 = 2.0;
/// `fork-dialog-section`'s words: 12 at 440 on an 18 line, `text-faint`.
const SECTION: rune::TextStyle = rune::TextStyle::new(12.0, 440.0).lh(18.0).tracking(0.05);
/// `.fork-dialog .search-list-trailing`: 11, tabular.
const TIME: rune::TextStyle = rune::TextStyle::new(11.0, 440.0).lh(16.0).tracking(0.05).features("'tnum' 1");
/// `search-list-empty`.
const EMPTY_H: f32 = 120.0;

fn search_input_shell(host: &dyn ForkHost, p: &Pal) -> El {
    field(&host.fork().search, UI)
        .key("fork-input")
        .spellcheck(host.spellcheck())
        .single_line()
        .grow(1.0)
        .shrink()
        .min_w(0.0)
        .color(p.text)
        .placeholder(opencode_i18n::t!("common.search.placeholder"), p.text.alpha(p.text.a * 0.5))
        .caret(p.text, p.selection)
        .selection_style(p.field_selection)
}

fn clear_button_shell(p: &Pal) -> El {
    el().key("fork-clear")
        .role("button")
        .label_static(opencode_i18n::t!("common.clear"))
        .size(20.0, 20.0)
        .radius(4.0)
        .center()
        .hover(p.hover)
        .hover_motion(INSTANT, INSTANT)
        .child(icon(&ic::CIRCLE_XMARK, 16.0).color(p.icon_muted))
}

fn row_shell(p: &Pal, item: &ForkableMessage, is_active: bool, is_pending: bool, slow: bool) -> El {
    let trailing = if is_pending {
        // Past `SLOW` the wait says what it waits for (Desktop's loader says nothing).
        let words = slow.then(|| text(opencode_i18n::t!("rune.fork.slow")).key("slow").font(UI).color(p.muted));
        Some(row().key("pending").gap(6.0).center_y().children(words).child(activity_ring_sized(p, 12.0)))
    } else {
        (!item.time.is_empty()).then(|| text(item.time.clone()).key("time").font(TIME).color(p.muted))
    };
    // `search-list-row`: 8 in, 12 between the text and the time, 6 pt corners, the hover
    // overlay while lit; the text 13 at 440 in `text-base`, the time in `text-muted`.
    row()
        .key(format!("fork-row-{}", item.id))
        .role("option")
        .label(item.text.clone())
        .w_full()
        .h(ROW_H)
        .pl(8.0)
        .pr(6.0)
        .gap(12.0)
        .center_y()
        .radius(6.0)
        .fill(if is_active { p.hover } else { Color::TRANSPARENT })
        .transition(Transition::none())
        .motion(Spring::NONE)
        .child(text(item.text.clone()).key("text").font(UI).color(p.text).truncate().grow(1.0).shrink().min_w(0.0))
        .children(trailing)
}

fn dialog_shell(p: &Pal, w: f32, scroll_h: f32, search: El, scroll: El) -> El {
    col()
        .key("fork-dialog")
        .role("dialog")
        .label_static(opencode_i18n::t!("command.session.fork"))
        .w(w)
        .h(HEAD_H + SECTION_H + scroll_h)
        .radius(12.0)
        .fill(p.bg_base)
        .elevate(p.overlay)
        .clip_soft(0.0)
        .transition(Transition::none())
        .motion(Spring::NONE)
        .child(col().key("fork-search").w_full().pt(6.0).px(6.0).child(search))
        // `::after`: a 1 pt rule drawn at half its height (`scaleY(0.5)`) across the dialog, 6
        // under the field and 2 above the section (`commands/dialog.css:152-158`).
        .child(rune::hairline(p.border).key("fork-divider").w_full().mt(6.0).mb(2.0))
        .child(
            row()
                .key("fork-desc")
                .w_full()
                .h(SECTION_H)
                .px(14.0)
                .pt(10.0)
                .pb(2.0)
                .center_y()
                .child(text(opencode_i18n::t!("dialog.fork.description")).font(SECTION).color(p.faint)),
        )
        .child(scroll)
}

/// Desktop's `DialogFork` (`fork-dialog.tsx`, `search-list.css:243-289`) in
/// `<Dialog size="large" fit>`: large on bg-base with `--v2-elevation-overlay` and 12 pt corners,
/// as tall as its rows up to the size's height, growing down from its top edge.
pub fn view<H: ForkHost + 'static>(s: &H, p: &Pal, cx: &Cx<H>) -> [El; 2] {
    let host: &dyn ForkHost = s;
    let [win_w, win_h] = dialog_window(cx, host.zoom());
    let size = DialogSize::Large;
    let w = size.width(win_w);
    let max_h = size.height(win_h);
    let all = messages(host);
    let list_items = filtered(host);
    let (active, slow) = match &host.fork().modal {
        ForkModal::Open { active, slow, .. } => (
            active.as_ref().filter(|id| list_items.iter().any(|m| &m.id == *id)),
            *slow,
        ),
        ForkModal::Closed => (None, false),
    };
    let session = host.active_session_id().unwrap_or_default().to_owned();
    let typed = !host.fork().search.text.is_empty();

    // `text-input-v2` (base) in the fork dialog: 28 tall and transparent, the glyph 8 in and 8
    // from the text, the placeholder in `text-base` at half strength, and the clear button
    // (`circle-xmark`) once something is typed.
    let search_input = search_input_shell(host, p)
        .on_edit(|s: &mut H| &mut s.fork_mut().search)
        .on_input_change(|s: &mut H, tx: &mut Tx| on_query_change(s, tx))
        .on_input_key(|s: &mut H, ev, tx| key_or_shortcut(s, ev, tx));
    let clear = typed.then(|| clear_button_shell(p).on_press_tx(|s: &mut H, tx: &mut Tx| clear_search(s, tx)));
    let search = row()
        .key("fork-field")
        .h(28.0)
        .pl(8.0)
        .pr(if typed { 2.0 } else { 6.0 })
        .gap(8.0)
        .center_y()
        .child(icon(&ic::MAGNIFYING_GLASS, 16.0).color(p.icon_muted))
        .child(search_input)
        .children(clear);

    // `search-list-results` in the compact dialog: 6 in at the sides and foot, rows 2 apart.
    let (list, content_h) = if list_items.is_empty() {
        let empty = if all.is_empty() { opencode_i18n::t!("dialog.fork.empty") } else { opencode_i18n::t!("dialog.fork.search.empty") };
        let el = col().key("fork-results").w_full().child(col().key("fork-empty").w_full().h(EMPTY_H).center().child(text(empty).font(UI).color(p.muted)));
        (el, EMPTY_H)
    } else {
        // While a fork runs the rows hold still, and its row shows a loader for its time
        // (`fork-dialog.tsx:164-189`, `Loader class="size-3"`).
        let pending = host.fork().pending();
        let mut el = col().key("fork-results").w_full().px(6.0).pb(6.0).gap(ROW_GAP);
        for item in &list_items {
            let is_active = active == Some(&item.id);
            let is_pending = pending == Some(item.id.as_str());
            let (sid, mid, hover_id) = (session.clone(), item.id.clone(), item.id.clone());
            let r = row_shell(p, item, is_active, is_pending, slow)
                .on_hover_change(move |s: &mut H, inside, tx: &mut Tx| on_row_hover(s, &hover_id, inside, tx))
                .on_press_tx(move |s: &mut H, tx: &mut Tx| on_row_press(s, &sid, &mid, tx));
            el = el.child(r);
        }
        (el, list_items.len() as f32 * (ROW_H + ROW_GAP) - ROW_GAP + 6.0)
    };
    let scroll_h = content_h.min(max_h - HEAD_H - SECTION_H).max(0.0);
    // A `MenuScroll` with the app's `.search-list-scroll` (`fork-dialog.tsx:152`): no edge fades.
    let scroll = scroller("fork-scroll").bar(Bar::MenuScroll).build::<H>(p, col().role("listbox").w_full().h(scroll_h), list);
    let dialog = dialog_shell(p, w, scroll_h, search, scroll).modal(true, |s: &mut H| close(s, true));
    let scrim = el().key("fork-scrim").absolute(0.0, 0.0).w_full().h_full().fill(p.scrim).transition(Transition::none());
    [scrim, dialog.absolute((win_w - w) / 2.0, size.top(win_h)).raise()]
}
