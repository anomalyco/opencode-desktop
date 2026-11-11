//! Forking a session from a user message, and the `"Fork from message"` picker dialog, matching
//! OpenCode Desktop (`packages/app/src/session/fork.ts`,
//! `packages/app/src/session/commands/fork-dialog.tsx`).
//!
//! `POST /api/session/:id/fork` with `{"before": messageID}` creates a new root session holding
//! the settled history strictly before `messageID`. Once created, it opens as a tab with that
//! prompt back in its composer ready to edit and send. One fork runs at a time (`machine.rs`).

use crate::pickers::PickerHost as _;
use opencode_kit::client::{Ticket, TicketKind, Write};
use rune::machine::Attempt;
use opencode_kit::mention;
use opencode_kit::model::{Message, SessionInfo, Transcript};
use rune::{
    Color, Cx, El, Key, KeyEvent, ScrollSpec, Spring, TextEdit, Transition, Tx, col, el, icon, row,
    text, text_input,
};
use rune_theme_opencode::icons as ic;

use crate::mentions::list::cycle;
use crate::palette::{Elevate as _, Pal};
use crate::view::{INSTANT, UI};
use crate::{App, Route, Source};

#[derive(Clone, Debug)]
pub struct ForkableMessage {
    pub id: String,
    pub text: String,
    pub time: String,
}

mod machine;
use machine::{Effect, Event, Surface};

#[derive(Default)]
pub struct ForkState {
    pub open: bool,
    pub search: TextEdit,
    pub active: Option<String>,
    restore_composer: bool,
    /// The fork on its way (`machine.rs`, `session/fork-machine.ts`).
    machine: machine::Fork,
    /// Fixture forks made so far, for their ids.
    made: u32,
}

impl ForkState {
    /// The prompt a fork is on its way from: the picker holds still and shows it pending.
    pub fn pending(&self) -> Option<&str> {
        self.machine.pending()
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

impl App {
    /// Whether `/fork` and `⌘K → Fork from message` are enabled on the open session.
    pub fn can_fork_session(&self) -> bool {
        let Route::Session { id, .. } = &self.route else { return false };
        self.can_rewind_session(id) && !self.visible_user_ids(id).is_empty()
    }

    /// Forks `session` before user message `message_id`, opens the new session, and puts the
    /// message's prompt back into its composer (`useSessionFork`). One fork at a time: another
    /// press waits for it. False when none started.
    pub fn fork_from(&mut self, session: &str, message_id: &str, tx: &mut Tx) -> bool {
        if !self.can_rewind_session(session) {
            return false;
        }
        let Some(body) = self.transcripts.get(session).and_then(|t| {
            t.messages.iter().find_map(|m| match m {
                Message::User { id, text } if id == message_id => Some(text.clone()),
                _ => None,
            })
        }) else {
            return false;
        };
        let draft = self.draft_from_message(message_id, &body);
        let surface = if self.fork.open { Surface::Dialog } else { Surface::Inline };
        let event = Event::Requested { surface, session: session.to_owned(), message: message_id.to_owned(), draft, now: self.clock };
        self.step_fork(event, tx)
    }

    /// Steps the fork machine and carries out what it says. True when it changed anything.
    fn step_fork(&mut self, event: Event, tx: &mut Tx) -> bool {
        let effects = self.fork.machine.step(event);
        let changed = !effects.is_empty();
        for effect in effects {
            match effect {
                Effect::Start { attempt, session, message } => {
                    // The fixture and the storybook fork at once, as a server would.
                    if self.stage.is_some() || matches!(self.source, Source::Fixture) {
                        let forked = self.create_fixture_fork(&session, &message);
                        self.step_fork(Event::Succeeded { attempt, forked }, tx);
                        continue;
                    }
                    let ticket = Ticket { kind: TicketKind::Fork, session: session.clone(), attempt: attempt.0 };
                    let write = Write::Fork { session, before: Some(message) };
                    if !self.write(Write::Tracked { ticket, write: Box::new(write) }) {
                        let why = self.status.clone();
                        self.step_fork(Event::Failed { attempt, why }, tx);
                    }
                }
                Effect::Open { forked, draft } => {
                    self.open(&forked, tx);
                    self.apply_draft(&forked, draft);
                }
                Effect::CloseDialog => self.close_fork_dialog(false),
                Effect::Fail { why } => {
                    let why = why.split_once(": ").map_or(why.as_str(), |(_, why)| why).to_owned();
                    crate::status_toast::fail(self, opencode_i18n::t!("common.requestFailed"), why);
                }
            }
        }
        changed
    }

    fn create_fixture_fork(&mut self, session: &str, before_message: &str) -> String {
        self.fork.made += 1;
        let forked_id = format!("ses_fx_fork_{:04}", self.fork.made);
        let now = self.now_ms;
        let info = match self.session(session).cloned() {
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
        let copied: Vec<Message> = self
            .transcripts
            .get(session)
            .map(|t| {
                let end = t.visible_len(Some(before_message));
                t.messages[..end]
                    .iter()
                    .filter(|m| !matches!(m, Message::Assistant(a) if a.completed.is_none()))
                    .cloned()
                    .collect()
            })
            .unwrap_or_default();
        self.sessions.insert(0, info);
        self.transcripts.insert(forked_id.clone(), Transcript { session: forked_id.clone(), messages: copied });
        forked_id
    }

    /// Opens Desktop's `"Fork from message"` dialog for the open session.
    pub fn open_fork_dialog(&mut self, tx: &mut Tx) -> bool {
        if !self.can_fork_session() {
            return false;
        }
        tx.instant();
        let restore = if self.fork.open { self.fork.restore_composer } else { self.composer.focused };
        self.close_picker(false);
        self.close_switcher(false);
        self.close_project_switcher(false);
        self.close_message_menu();
        self.fork.restore_composer = restore;
        self.composer.focused = false;
        self.fork.open = true;
        self.fork.search = TextEdit::new("");
        self.fork.search.focused = true;
        self.fork.active = self.fork_messages().first().map(|m| m.id.clone());
        true
    }

    pub fn close_fork_dialog(&mut self, restore_focus: bool) {
        if !self.fork.open {
            return;
        }
        // The picker's fork goes with it (`fork-machine.ts` `disposed`).
        if matches!(self.fork.machine.state, machine::State::Forking { surface: Surface::Dialog, .. }) {
            self.fork.machine.step(Event::Left);
        }
        self.fork.open = false;
        self.fork.search.focused = false;
        self.fork.search.cancel_composition();
        if restore_focus {
            self.composer.focused = self.fork.restore_composer;
        }
    }

    /// All non-empty user messages of the open session, newest first (`fork-dialog.tsx`).
    pub fn fork_messages(&self) -> Vec<ForkableMessage> {
        let Route::Session { id: session, .. } = &self.route else { return Vec::new() };
        let revert = self.session(session).and_then(|i| i.revert.as_deref());
        let Some(t) = self.transcripts.get(session) else { return Vec::new() };
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
    pub fn fork_filtered(&self) -> Vec<ForkableMessage> {
        let q = self.fork.search.text.trim().to_lowercase();
        let all = self.fork_messages();
        if q.is_empty() {
            return all;
        }
        all.into_iter().filter(|m| m.text.to_lowercase().contains(&q)).collect()
    }

    pub fn fork_key(&mut self, ev: &KeyEvent, tx: &mut Tx) -> bool {
        if !self.fork.open {
            return false;
        }
        if self.fork.search.is_composing() {
            return false;
        }
        let list = self.fork_filtered();
        match ev.key {
            Key::Escape => {
                tx.instant();
                self.close_fork_dialog(true);
                true
            }
            // While a fork runs the list holds still (`fork-dialog.tsx:97-103`).
            Key::Down | Key::Up | Key::Enter if self.fork.pending().is_some() => true,
            Key::Down | Key::Up if !ev.mods.shift && !ev.mods.ctrl && !ev.mods.cmd && !ev.mods.alt => {
                tx.instant();
                if !list.is_empty() {
                    let cur = self.fork.active.as_ref().and_then(|id| list.iter().position(|m| &m.id == id));
                    let next = cycle(list.len(), cur, if ev.key == Key::Down { 1 } else { -1 });
                    let id = list[next].id.clone();
                    if next == 0 {
                        tx.scroll_to_y("fork-scroll", 0.0);
                    } else {
                        tx.scroll_into_view_in("fork-scroll", format!("fork-row-{id}"));
                    }
                    self.fork.active = Some(id);
                }
                true
            }
            Key::Enter if !ev.mods.shift => {
                tx.instant();
                let Route::Session { id: session, .. } = self.route.clone() else { return true };
                let chosen = self
                    .fork
                    .active
                    .as_ref()
                    .and_then(|id| list.iter().find(|m| &m.id == id))
                    .or_else(|| list.first())
                    .map(|m| m.id.clone());
                if let Some(mid) = chosen {
                    self.fork_from(&session, &mid, tx);
                }
                true
            }
            _ => false,
        }
    }
}

/// A fork's `Incoming::Settled`: the session it made is remembered, and opens with the forked
/// prompt in its composer when this fork is still the one on its way.
pub fn settled(s: &mut App, attempt: u32, result: Result<Option<SessionInfo>, String>, tx: &mut Tx) {
    let attempt = Attempt(attempt);
    let event = match result {
        Ok(Some(forked)) => {
            let id = forked.id.clone();
            match s.sessions.iter_mut().find(|x| x.id == id) {
                Some(slot) => *slot = forked,
                None => s.sessions.insert(0, forked),
            }
            Event::Succeeded { attempt, forked: id }
        }
        Ok(None) => Event::Failed { attempt, why: String::new() },
        Err(why) => Event::Failed { attempt, why },
    };
    s.step_fork(event, tx);
}

/// Each frame: a fork past its 30 s bound fails, and one whose session isn't open any more is
/// dropped. True when either happened.
pub fn poll(s: &mut App, tx: &mut Tx) -> bool {
    let machine::State::Forking { session, due, .. } = &s.fork.machine.state else { return false };
    if s.route != Route::here(session.clone()) {
        return s.step_fork(Event::Left, tx);
    }
    let (now, due) = (s.clock, *due);
    if now + 1e-9 >= due {
        return s.step_fork(Event::Due { now }, tx);
    }
    tx.wake_after(std::time::Duration::from_secs_f64(due - now));
    false
}

/// When a prompt was sent, as `toLocaleTimeString(undefined, { timeStyle: "short" })` reads it:
/// "6:03 AM" in the system's locale rather than the chosen language (fork-dialog.tsx).
fn short_time(id: &str) -> String {
    let ms = mention::user_created(id).or_else(|| {
        let hex = id.strip_prefix("msg_")?.get(..12)?;
        u64::from_str_radix(hex, 16).ok().map(|v| (v >> 12) as i64).filter(|&v| v > 1_500_000_000_000)
    });
    let moment = |ms| session_view::turn_errors::moment(rune::LocalTime::at(ms));
    ms.map(|ms| opencode_i18n::date::format_in(opencode_i18n::system_locale(), opencode_i18n::date::Style::Time, moment(ms))).unwrap_or_default()
}

/// The search block (`.fork-dialog [data-slot="search-list-search"]`): 6 above the 28-tall
/// field and 9 below it, the hairline at its foot.
const HEAD_H: f32 = 6.0 + 28.0 + 9.0;
/// `fork-dialog-section`: the description, 4 above and below its 20-tall line.
const SECTION_H: f32 = 28.0;
/// A row (`.fork-dialog [data-slot="search-list-row"]`), and the 2 between rows.
const ROW_H: f32 = 28.0;
const ROW_GAP: f32 = 2.0;
/// `search-list-empty`.
const EMPTY_H: f32 = 120.0;

/// Desktop's `DialogFork` (`fork-dialog.tsx`, `search-list.css:243-289`) in the compact palette
/// dialog (`dialog.css:141-153`): 480 wide on bg-base with `--v2-elevation-overlay` and 12 pt
/// corners, as tall as its rows up to 480, its top halfway down a 480-tall box but at least 48
/// from the window's.
pub fn view(s: &App, p: &Pal, cx: &Cx<App>) -> [El; 2] {
    let zoom = s.zoom.max(0.5);
    let [win_w, win_h] = cx
        .layout_size("zoom")
        .map_or([1280.0 / zoom, 820.0 / zoom], |[w, h]| [w / zoom, h / zoom]);
    let w = (win_w - 24.0).min(480.0);
    let max_h = (win_h - 96.0).min(480.0);
    let all = s.fork_messages();
    let filtered = s.fork_filtered();
    let active = s.fork.active.as_ref().filter(|id| filtered.iter().any(|m| &m.id == *id));
    let session = match &s.route {
        Route::Session { id: sid, .. } => sid.clone(),
        _ => String::new(),
    };

    // `text-input-v2` (base) in the fork dialog: 28 tall and transparent, the glyph 8 in and 8
    // from the text, the placeholder in `text-base` at half strength, and the clear button
    // (`circle-xmark`) once something is typed.
    let typed = !s.fork.search.text.is_empty();
    let search = row()
        .key("fork-field")
        .h(28.0)
        .pl(8.0)
        .pr(if typed { 2.0 } else { 6.0 })
        .gap(8.0)
        .center_y()
        .child(icon(&ic::MAGNIFYING_GLASS, 16.0).color(p.icon_muted))
        .child(
            text_input(&s.fork.search, UI)
                .key("fork-input")
                .single_line()
                .grow(1.0)
                .shrink()
                .min_w(0.0)
                .color(p.text)
                .placeholder(opencode_i18n::t!("common.search.placeholder"), p.text.alpha(p.text.a * 0.5))
                .caret(p.text, p.selection).selection_style(p.field_selection)
                .on_edit(|s: &mut App| &mut s.fork.search)
                .on_input_change(|s: &mut App, tx| {
                    tx.instant();
                    s.fork.active = s.fork_filtered().first().map(|m| m.id.clone());
                })
                .on_input_key(|s: &mut App, ev, tx| s.fork_key(ev, tx) || s.shortcut(ev, tx)),
        )
        .children(typed.then(|| {
            el().key("fork-clear")
                .role("button")
                .label(opencode_i18n::t!("common.clear"))
                .size(20.0, 20.0)
                .radius(4.0)
                .center()
                .hover(p.hover)
                .hover_motion(INSTANT, INSTANT)
                .on_press_tx(|s: &mut App, tx: &mut Tx| {
                    tx.instant();
                    s.fork.search = TextEdit::new("");
                    s.fork.search.focused = true;
                    s.fork.active = s.fork_filtered().first().map(|m| m.id.clone());
                })
                .child(icon(&ic::CIRCLE_XMARK, 16.0).color(p.icon_muted))
        }));

    // `search-list-results` in the compact dialog: 6 in at the sides and foot, rows 2 apart.
    let mut list = col().key("fork-results").w_full().px(6.0).pb(6.0).gap(ROW_GAP);
    let content_h = if filtered.is_empty() {
        let empty = if all.is_empty() { opencode_i18n::t!("dialog.fork.empty") } else { opencode_i18n::t!("dialog.fork.search.empty") };
        list = col().key("fork-results").w_full().child(col().key("fork-empty").w_full().h(EMPTY_H).center().child(text(empty).font(UI).color(p.muted)));
        EMPTY_H
    } else {
        // While a fork runs the rows hold still, and its row shows a loader for its time
        // (`fork-dialog.tsx:164-189`, `Loader class="size-3"`).
        let pending = s.fork.pending();
        for item in &filtered {
            let is_active = active == Some(&item.id);
            let (sid, mid, hover_id) = (session.clone(), item.id.clone(), item.id.clone());
            let trailing = if pending == Some(item.id.as_str()) {
                Some(session_view::style::activity_ring_sized(p, 12.0))
            } else {
                (!item.time.is_empty()).then(|| text(item.time.clone()).key("time").font(UI.features("\"tnum\"")).color(p.muted))
            };
            // `search-list-row`: 8 in, 12 between the text and the time, 6 pt corners, the hover
            // overlay while lit; the text 13 at 440 in `text-base`, the time in `text-muted`.
            let r = row()
                .key(format!("fork-row-{}", item.id))
                .role("option")
                .label(item.text.clone())
                .w_full()
                .h(ROW_H)
                .px(8.0)
                .gap(12.0)
                .center_y()
                .radius(6.0)
                .fill(if is_active { p.hover } else { Color::TRANSPARENT })
                .transition(Transition::none())
                .motion(Spring::NONE)
                .on_hover_change(move |s: &mut App, inside, tx: &mut Tx| {
                    if s.fork.pending().is_some() {
                        return;
                    }
                    tx.instant();
                    s.fork.active = inside.then(|| hover_id.clone());
                })
                .on_press_tx(move |s: &mut App, tx: &mut Tx| {
                    tx.instant();
                    s.fork_from(&sid, &mid, tx);
                })
                .child(text(item.text.clone()).key("text").font(UI).color(p.text).truncate().grow(1.0).shrink().min_w(0.0));
            list = list.child(r.children(trailing));
        }
        filtered.len() as f32 * (ROW_H + ROW_GAP) - ROW_GAP + 6.0
    };
    let scroll_h = content_h.min(max_h - HEAD_H - SECTION_H).max(0.0);

    let dialog = col()
        .key("fork-dialog")
        .role("dialog")
        .label(opencode_i18n::t!("command.session.fork"))
        .w(w)
        .h(HEAD_H + SECTION_H + scroll_h)
        .radius(12.0)
        .fill(p.bg_base)
        .elevate(p.overlay)
        .clip_soft(0.0)
        .transition(Transition::none())
        .motion(Spring::NONE)
        .modal(true, |s: &mut App| s.close_fork_dialog(true))
        .child(col().key("fork-search").w_full().pt(6.0).px(6.0).child(search))
        // `::after`: a 0.5 hairline in `border-muted` across the dialog at the block's foot.
        .child(el().key("fork-divider").w_full().h(0.5).mt(8.5).fill(p.border_muted))
        .child(
            row()
                .key("fork-desc")
                .w_full()
                .h(SECTION_H)
                .px(14.0)
                .center_y()
                .child(text(opencode_i18n::t!("dialog.fork.description")).font(UI.lh(20.0)).color(p.muted)),
        )
        .child(
            col()
                .key("fork-scroll")
                .role("listbox")
                .w_full()
                .h(scroll_h)
                // A `MenuScroll` with the app's `.search-list-scroll` (`fork-dialog.tsx:152`): no
                // edge fades.
                .scroll(ScrollSpec { stick_bottom: false })
                .child(list),
        );

    let scrim = el()
        .key("fork-scrim")
        .absolute(0.0, 0.0)
        .w_full()
        .h_full()
        .fill(p.scrim)
        .transition(Transition::none());
    [scrim, dialog.absolute((win_w - w) / 2.0, ((win_h - 480.0) / 2.0).max(48.0)).raise()]
}

