//! Forking a session from a user message, and the `"Fork from message"` picker dialog, matching
//! OpenCode Desktop (`packages/app/src/session/fork.ts`,
//! `packages/app/src/session/commands/fork-dialog.tsx`).
//!
//! `POST /api/session/:id/fork` with `{"before": messageID}` creates a new root session holding
//! the settled history strictly before `messageID`. Once created, it opens as a tab with that
//! prompt back in its composer ready to edit and send.

use opencode_kit::client::Write;
use opencode_kit::mention;
use opencode_kit::model::{Message, SessionInfo, Transcript};
use rune::{
    Color, Cx, El, Key, KeyEvent, ScrollSpec, Spring, TextEdit, Transition, Tx, col, el, icon, row,
    text, text_input,
};
use rune_theme_opencode::icons as ic;

use crate::mentions::list::cycle;
use crate::palette::Pal;
use crate::rewind::SavedDraft;
use crate::view::{INSTANT, UI, UI_SMALL};
use crate::{App, Route, Source};

#[derive(Clone, Debug)]
pub struct ForkableMessage {
    pub id: String,
    pub text: String,
    pub time: String,
}

#[derive(Default)]
pub struct ForkState {
    pub open: bool,
    pub search: TextEdit,
    pub active: Option<String>,
    restore_composer: bool,
    pending: Vec<(String, Option<String>, SavedDraft)>,
    seq: u32,
}

impl ForkState {
    pub fn clear_session(&mut self, session: &str) {
        self.pending.retain(|(from, _, _)| from != session);
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
        self.can_undo_session()
    }

    /// Forks `session` before user message `message_id`, opens the new session, and puts the
    /// message's prompt back into its composer (`useSessionFork`).
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

        let restored = self.draft_from_message(message_id, &body);
        let fixture = self.stage.is_some() || matches!(self.source, Source::Fixture);
        if fixture {
            self.close_fork_dialog(false);
            let forked_id = self.create_fixture_fork(session, message_id);
            self.open(&forked_id, tx);
            self.apply_draft(&forked_id, restored);
            return true;
        }

        if !self.write(Write::Fork { session: session.to_owned(), before: Some(message_id.to_owned()) }) {
            let why = self.status.clone();
            crate::status_toast::fail(self, "Request failed", why);
            return false;
        }
        self.fork.pending.push((session.to_owned(), Some(message_id.to_owned()), restored));
        self.close_fork_dialog(false);
        true
    }

    fn create_fixture_fork(&mut self, session: &str, before_message: &str) -> String {
        self.fork.seq += 1;
        let forked_id = format!("ses_fx_fork_{:04}", self.fork.seq);
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
        self.fork.open = false;
        self.fork.search.focused = false;
        self.fork.search.preedit = None;
        if restore_focus {
            self.composer.focused = self.fork.restore_composer;
        }
    }

    /// All non-empty user messages of the open session, newest first (`fork-dialog.tsx`).
    pub fn fork_messages(&self) -> Vec<ForkableMessage> {
        let Route::Session(session) = &self.route else { return Vec::new() };
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
        if self.fork.search.preedit.is_some() {
            return false;
        }
        let list = self.fork_filtered();
        match ev.key {
            Key::Escape => {
                tx.instant();
                self.close_fork_dialog(true);
                true
            }
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
                let Route::Session(session) = self.route.clone() else { return true };
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

/// Applies `Incoming::Forked`: remembers the new session, opens it, and puts the forked prompt in
/// its composer.
pub fn applied(s: &mut App, from: String, before: Option<String>, forked: SessionInfo, tx: &mut Tx) {
    let forked_id = forked.id.clone();
    match s.sessions.iter_mut().find(|x| x.id == forked_id) {
        Some(slot) => *slot = forked,
        None => s.sessions.insert(0, forked),
    }
    let idx = s.fork.pending.iter().position(|(f, b, _)| *f == from && *b == before);
    let draft = idx.map(|i| s.fork.pending.remove(i).2);
    s.close_fork_dialog(false);
    s.open(&forked_id, tx);
    if let Some(draft) = draft {
        s.apply_draft(&forked_id, draft);
    }
}

fn short_time(id: &str) -> String {
    let ms = mention::user_created(id).or_else(|| {
        let hex = id.strip_prefix("msg_")?.get(..12)?;
        u64::from_str_radix(hex, 16).ok().map(|v| (v >> 12) as i64).filter(|&v| v > 1_500_000_000_000)
    });
    let Some(ms) = ms else { return String::new() };
    let tod = ms.div_euclid(1000).rem_euclid(86_400);
    format!("{:02}:{:02}", tod / 3600, (tod % 3600) / 60)
}

pub fn view(s: &App, p: &Pal, cx: &Cx<App>) -> [El; 2] {
    let zoom = s.zoom.max(0.5);
    let [win_w, win_h] = cx
        .layout_size("zoom")
        .map_or([1280.0 / zoom, 820.0 / zoom], |[w, h]| [w / zoom, h / zoom]);
    let (w, h) = ((win_w - 32.0).clamp(280.0, 560.0), (win_h - 64.0).clamp(200.0, 380.0));
    let all = s.fork_messages();
    let filtered = s.fork_filtered();
    let active = s
        .fork
        .active
        .as_ref()
        .filter(|id| filtered.iter().any(|m| &m.id == *id))
        .or_else(|| filtered.first().map(|m| &m.id));
    let session = match &s.route {
        Route::Session(sid) => sid.clone(),
        _ => String::new(),
    };

    let search = row()
        .key("fork-field")
        .h(36.0)
        .px(12.0)
        .gap(8.0)
        .center_y()
        .radius(6.0)
        .fill(p.search_bg)
        .hover(p.layer2)
        .hover_motion(INSTANT, INSTANT)
        .child(icon(&ic::MAGNIFYING_GLASS, 16.0).color(p.icon_muted))
        .child(
            text_input(&s.fork.search, UI.lh(20.0))
                .key("fork-input")
                .grow(1.0)
                .shrink()
                .h(20.0)
                .placeholder("Search", p.faint)
                .caret(p.text, p.selection)
                .on_edit(|s: &mut App| &mut s.fork.search)
                .on_input_change(|s: &mut App, tx| {
                    tx.instant();
                    s.fork.active = s.fork_filtered().first().map(|m| m.id.clone());
                })
                .on_input_key(|s: &mut App, ev, tx| s.fork_key(ev, tx) || s.shortcut(ev, tx)),
        );

    let mut list = col().key("fork-results").w_full().px(6.0).pb(8.0).gap(1.0);
    if filtered.is_empty() {
        let empty = if all.is_empty() { "No messages to fork from" } else { "No messages found" };
        list = list.child(col().key("fork-empty").w_full().min_h(120.0).center().child(text(empty).font(UI).color(p.muted)));
    } else {
        for item in &filtered {
            let is_active = active == Some(&item.id);
            let (sid, mid, hover_id) = (session.clone(), item.id.clone(), item.id.clone());
            let mut r = row()
                .key(format!("fork-row-{}", item.id))
                .role("option")
                .label(item.text.clone())
                .w_full()
                .h(36.0)
                .px(12.0)
                .gap(12.0)
                .center_y()
                .radius(6.0)
                .fill(if is_active { p.row_active } else { Color::TRANSPARENT })
                .transition(Transition::none())
                .on_hover_change(move |s: &mut App, inside, tx: &mut Tx| {
                    tx.instant();
                    s.fork.active = inside.then(|| hover_id.clone());
                })
                .on_press_tx(move |s: &mut App, tx: &mut Tx| {
                    tx.instant();
                    s.fork_from(&sid, &mid, tx);
                })
                .child(text(item.text.clone()).font(UI).color(p.text).truncate_fade().grow(1.0).shrink().min_w(0.0));
            if !item.time.is_empty() {
                r = r.child(text(item.time.clone()).font(UI_SMALL).color(p.faint));
            }
            list = list.child(r);
        }
    }

    let dialog = col()
        .key("fork-dialog")
        .role("dialog")
        .label("Fork from message")
        .w(w)
        .h(h)
        .radius(12.0)
        .fill(p.bg_base)
        .border(0.5, p.ring)
        .shadow(8.0, 16.0, p.shadow_far)
        .shadow(4.0, 8.0, p.shadow)
        .clip_soft(0.0)
        .transition(Transition::none())
        .motion(Spring::NONE)
        .modal(true, |s: &mut App| s.close_fork_dialog(true))
        .child(col().key("fork-search").w_full().padding(6.0).child(search))
        .child(
            row()
                .key("fork-desc")
                .w_full()
                .h(28.0)
                .px(18.0)
                .center_y()
                .child(text("Choose a message to fork from").font(UI_SMALL).color(p.muted)),
        )
        .child(
            col()
                .key("fork-scroll")
                .role("listbox")
                .w_full()
                .grow(1.0)
                .shrink()
                .scroll(ScrollSpec { stick_bottom: false })
                .clip_fade(0.0, 0.0, 0.0, 24.0)
                .child(list),
        );

    let scrim = el()
        .key("fork-scrim")
        .absolute(0.0, 0.0)
        .w_full()
        .h_full()
        .fill(p.scrim)
        .transition(Transition::none());
    let modal = el()
        .key("fork-modal")
        .absolute(0.0, 0.0)
        .w_full()
        .h_full()
        .center()
        .transition(Transition::none())
        .child(dialog)
        .raise();
    [scrim, modal]
}

#[cfg(test)]
#[path = "fork_tests.rs"]
mod tests;
