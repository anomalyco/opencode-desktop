//! Jump to message (Ctrl+Alt+G, ⌥⌘G on macOS), as Desktop's `DialogJumpToMessage`
//! (`session/commands/dialog-jump-to-message.tsx`, `jump-to-message.tsx`): the open session's
//! sent prompts, newest first, narrowed by what's typed, each a snippet around its match and when
//! it was sent. Choosing one puts it at the timeline's top as Previous and Next message do; one
//! the timeline hasn't loaded yet is paged back to first (`machine.rs`).

use crate::pickers::PickerHost as _;
use opencode_kit::model::Message;
use rune::{Color, Cx, El, Key, KeyEvent, Spring, TextEdit, Transition, Tx, col, el, icon, row, text};
use rune_theme_opencode::icons as ic;
use serde_json::Value;

use crate::mentions::list::cycle;
use crate::palette::{Elevate as _, Pal};
use crate::view::{INSTANT, UI};
use crate::{App, Route, Source};

mod machine;
use machine::{Effect, Event, Prompt, State};

#[derive(Default)]
pub struct JumpState {
    machine: State,
    pub search: TextEdit,
    active: Option<String>,
    /// The session the dialog lists, and whether the composer had focus when it opened.
    session: String,
    restore_composer: bool,
}

impl JumpState {
    pub fn open(&self) -> bool {
        self.machine.is_open()
    }
}

/// A row: its prompt and the snippet shown (`jump-to-message.tsx` `rows`).
struct Row {
    prompt: Prompt,
    snippet: String,
}

impl App {
    /// Opens the dialog over the open session (`session.jumpToMessage`, disabled without one).
    pub fn open_jump_dialog(&mut self, tx: &mut Tx) -> bool {
        let Route::Session { id, .. } = &*self.route else { return false };
        let sid = self.displayed_session(id).unwrap_or(id).to_owned();
        tx.instant();
        let restore = if self.jump.open() { self.jump.restore_composer } else { self.composer.focused };
        self.close_jump_dialog(false);
        self.close_fork_dialog(false);
        self.close_picker(false);
        self.close_switcher(false);
        self.close_project_switcher(false);
        self.close_message_menu();
        self.jump.restore_composer = restore;
        self.composer.focused = false;
        self.jump.session = sid;
        self.jump.search = TextEdit::new("");
        self.jump.search.focused = true;
        self.step_jump(Event::Start, tx);
        self.jump.active = self.jump_rows().first().map(|r| r.prompt.id.clone());
        true
    }

    pub fn close_jump_dialog(&mut self, restore_focus: bool) {
        if !self.jump.open() {
            return;
        }
        self.jump.machine.step(Event::Disposed);
        self.leave_jump(restore_focus);
    }

    fn leave_jump(&mut self, restore_focus: bool) {
        self.jump.search.focused = false;
        self.jump.search.cancel_composition();
        if restore_focus {
            self.composer.focused = self.jump.restore_composer;
        }
    }

    fn step_jump(&mut self, event: Event, tx: &mut Tx) {
        for effect in self.jump.machine.step(event) {
            match effect {
                Effect::HistoryLoad { attempt } => self.load_jump_history(attempt, tx),
                Effect::OpenLoad { .. } => {
                    let sid = self.jump.session.clone();
                    self.read_toward_place(&sid, tx);
                }
                Effect::Jump { id } => {
                    self.leave_jump(true);
                    let sid = self.jump.session.clone();
                    crate::message_nav::jump_to(self, &sid, &id, tx);
                }
                // A read already on its way lands with an attempt the machine no longer takes.
                Effect::Cancel => {}
            }
        }
    }

    /// Older prompts for the picker only: paging them into the timeline would move the reader.
    /// A session whose first page holds all of it has none.
    fn load_jump_history(&mut self, attempt: u32, tx: &mut Tx) {
        let sid = self.jump.session.clone();
        let all_loaded = self.pages.first_read_done(&sid) && !self.pages.more_above(&sid);
        let server = match &*self.source {
            Source::Live { server, .. } if !all_loaded => Some(server.clone()),
            _ => None,
        };
        let Some(server) = server else { return self.step_jump(Event::HistoryLoaded { attempt, history: Vec::new() }, tx) };
        let deliver = self.deliverer();
        let attachment = opencode_i18n::t!("common.attachment").to_owned();
        let spawned = rune::task::spawn("jump-history", move || {
            let result = history(&server, &sid, &attachment);
            deliver(Box::new(move |s: &mut App, tx: &mut Tx| {
                let event = match result {
                    Ok(history) => Event::HistoryLoaded { attempt, history },
                    Err(_) => Event::HistoryFailed { attempt },
                };
                s.step_jump(event, tx);
                if s.jump.active.is_none() {
                    s.jump.active = s.jump_rows().first().map(|r| r.prompt.id.clone());
                }
            }));
        });
        if spawned.is_err() {
            self.step_jump(Event::HistoryFailed { attempt }, tx);
        }
    }

    /// Every prompt the dialog can list: the older history, then the timeline's own, which
    /// win by id; none with no words, none still waiting to send, none past a revert.
    fn jump_prompts(&self) -> Vec<Prompt> {
        let sid = &self.jump.session;
        let mut all: Vec<Prompt> = self.jump.machine.history().to_vec();
        let shown = crate::message_nav::prompt_ids(self, sid);
        if let Some(t) = self.transcripts.get(sid) {
            for m in &t.messages {
                if let Message::User { id, text } = m
                    && shown.contains(id)
                {
                    let p = Prompt { id: id.clone(), text: text.clone(), created: created(id) };
                    match all.iter_mut().find(|x| x.id == *id) {
                        Some(slot) => *slot = p,
                        None => all.push(p),
                    }
                }
            }
        }
        let revert = self.session(sid).and_then(|i| i.revert.clone());
        all.retain(|p| !p.text.trim().is_empty() && revert.as_ref().is_none_or(|r| p.id < *r));
        all
    }

    /// The prompts whose words hold the query, newest first, each as a snippet: 16 before the
    /// match (from a word's start) to 90 on or 16 past it, with `…` where it was cut.
    fn jump_rows(&self) -> Vec<Row> {
        let needle = self.jump.search.text.trim().to_lowercase();
        let mut prompts = self.jump_prompts();
        prompts.sort_by(|a, b| b.created.cmp(&a.created));
        prompts
            .into_iter()
            .filter_map(|prompt| {
                let words: Vec<char> = prompt.text.split_whitespace().collect::<Vec<_>>().join(" ").chars().collect();
                let lower: Vec<char> = words.iter().flat_map(|c| c.to_lowercase()).collect();
                let needle: Vec<char> = needle.chars().collect();
                // A letter that lowers to more than one keeps the text's own places only when none do.
                let at = if lower.len() != words.len() { Some(0).filter(|_| needle.is_empty()) } else { find(&lower, &needle) }?;
                let prefix = at.saturating_sub(16);
                let space = words.iter().skip(prefix).position(|c| *c == ' ').map(|i| i + prefix);
                let start = match space {
                    Some(sp) if prefix > 0 && sp < at => sp + 1,
                    _ => prefix,
                };
                let end = words.len().min((start + 90).max(at + needle.len() + 16));
                let body: String = words[start..end].iter().collect();
                let snippet = format!("{}{body}{}", if start > 0 { "…" } else { "" }, if end < words.len() { "…" } else { "" });
                Some(Row { prompt, snippet })
            })
            .collect()
    }

    fn choose_jump(&mut self, id: &str, tx: &mut Tx) {
        tx.instant();
        let loaded = self.transcripts.get(&self.jump.session).is_some_and(|t| t.messages.iter().any(|m| matches!(m, Message::User { id: x, .. } if x == id)));
        self.step_jump(Event::Selected { id: id.to_owned(), loaded, now: *self.clock }, tx);
    }

    pub fn jump_key(&mut self, ev: &KeyEvent, tx: &mut Tx) -> bool {
        if !self.jump.open() || self.jump.search.is_composing() {
            return false;
        }
        match ev.key {
            Key::Escape => {
                tx.instant();
                self.close_jump_dialog(true);
                true
            }
            // While a prompt opens the list holds still (`disabled={props.opening}`).
            Key::Down | Key::Up | Key::Enter if self.jump.machine.opening() => true,
            Key::Down | Key::Up if !ev.mods.shift && !ev.mods.ctrl && !ev.mods.cmd && !ev.mods.alt => {
                tx.instant();
                let rows = self.jump_rows();
                if !rows.is_empty() {
                    let cur = self.jump.active.as_ref().and_then(|id| rows.iter().position(|r| &r.prompt.id == id));
                    let next = cycle(rows.len(), cur, if ev.key == Key::Down { 1 } else { -1 });
                    let id = rows[next].prompt.id.clone();
                    if next == 0 {
                        tx.scroll_to_y("jump-scroll", 0.0);
                    } else {
                        tx.scroll_into_view_in("jump-scroll", format!("jump-row-{id}"));
                    }
                    self.jump.active = Some(id);
                }
                true
            }
            Key::Enter if !ev.mods.shift => {
                let rows = self.jump_rows();
                let chosen = self.jump.active.as_ref().and_then(|id| rows.iter().find(|r| &r.prompt.id == id)).or_else(|| rows.first()).map(|r| r.prompt.id.clone());
                if let Some(id) = chosen {
                    self.choose_jump(&id, tx);
                }
                true
            }
            _ => false,
        }
    }
}

/// The first place `needle` starts in `hay`.
fn find(hay: &[char], needle: &[char]) -> Option<usize> {
    if needle.is_empty() {
        return Some(0);
    }
    hay.windows(needle.len()).position(|w| w == needle)
}

/// When prompt `id` was sent: as read with it, else from its id's time.
fn created(id: &str) -> i64 {
    opencode_kit::model::user_created(id)
        .or_else(|| opencode_kit::mention::user_created(id))
        .or_else(|| {
            let hex = id.strip_prefix("msg_")?.get(..12)?;
            u64::from_str_radix(hex, 16).ok().map(|v| (v >> 12) as i64).filter(|&v| v > 1_500_000_000_000)
        })
        .unwrap_or(0)
}

/// Every sent prompt of session `sid`, 200 a page (`loadHistory`): its words, then its files,
/// agents and skills that aren't mentions in them.
fn history(server: &opencode_kit::client::Server, sid: &str, attachment: &str) -> Result<Vec<Prompt>, String> {
    let mut rows = Vec::new();
    let mut cursor: Option<String> = None;
    loop {
        let after = cursor.as_deref().map(|c| format!("&cursor={}", crate::switcher::encode(c))).unwrap_or_default();
        let page = server.read(&format!("/api/session/{sid}/message?type=user&limit=200{after}")).map_err(|e| e.to_string())?;
        for m in page["data"].as_array().into_iter().flatten().filter(|m| m["type"] == "user") {
            rows.push(Prompt { id: m["id"].as_str().unwrap_or_default().to_owned(), text: words(m, attachment), created: m["time"]["created"].as_i64().unwrap_or(0) });
        }
        cursor = page["cursor"]["next"].as_str().map(str::to_owned);
        if cursor.is_none() {
            return Ok(rows);
        }
    }
}

fn words(m: &Value, attachment: &str) -> String {
    let list = |k: &str| m[k].as_array().into_iter().flatten().filter(|x| !x["mention"].as_bool().unwrap_or(false));
    let mut labels = vec![m["text"].as_str().unwrap_or_default().to_owned()];
    labels.extend(list("files").map(|f| f["name"].as_str().filter(|n| !n.is_empty()).unwrap_or(attachment).to_owned()));
    labels.extend(list("agents").filter_map(|a| a["name"].as_str()).map(|n| format!("@{n}")));
    labels.extend(list("skills").filter_map(|a| a["name"].as_str()).map(str::to_owned));
    labels.retain(|l| !l.is_empty());
    labels.join("\n")
}

/// Each frame: a dialog whose session isn't open any more closes, and a prompt being opened
/// lands once the timeline holds it, or fails when there's nothing older left or its time is up.
pub fn poll(s: &mut App, tx: &mut Tx) -> bool {
    if !s.jump.open() {
        return false;
    }
    let sid = s.jump.session.clone();
    if !matches!(&*s.route, Route::Session { id, .. } if s.displayed_session(id).unwrap_or(id) == sid) {
        s.close_jump_dialog(false);
        return true;
    }
    let State::Opening { attempt, target, due, .. } = &s.jump.machine else { return false };
    let (attempt, target, due) = (*attempt, target.clone(), *due);
    let shown = crate::message_nav::prompt_ids(s, &sid).contains(&target);
    let held = s.transcripts.get(&sid).is_some_and(|t| t.messages.iter().any(|m| matches!(m, Message::User { id, .. } if *id == target)));
    let event = if shown {
        Event::Opened { attempt }
    } else if held || *s.clock >= due || (s.pages.first_read_done(&sid) && !s.pages.more_above(&sid)) {
        // Loaded but not a prompt the timeline shows (a steer still waiting), or nothing older.
        Event::OpenFailed { attempt }
    } else {
        s.read_toward_place(&sid, tx);
        tx.wake_after(std::time::Duration::from_millis(100));
        return false;
    };
    s.step_jump(event, tx);
    true
}

/// The search block: 6 above the 28-tall field and 9 below it.
const HEAD_H: f32 = 6.0 + 28.0 + 9.0;
/// The description and its Retry, 4 above and below a 20-tall line.
const SECTION_H: f32 = 28.0;
const ROW_H: f32 = 28.0;
const ROW_GAP: f32 = 2.0;
const EMPTY_H: f32 = 120.0;

/// Desktop's `JumpToMessage` in the compact palette dialog, as Fork from message draws it
/// (`fork-dialog`): 480 wide, as tall as its rows up to 480.
pub fn view(s: &App, p: &Pal, cx: &Cx<App>) -> [El; 2] {
    let zoom = s.zoom.max(0.5);
    let [win_w, win_h] = cx.layout_size("zoom").map_or([1280.0 / zoom, 820.0 / zoom], |[w, h]| [w, h]);
    let w = (win_w - 24.0).min(480.0);
    let max_h = (win_h - 96.0).min(480.0);
    let rows = s.jump_rows();
    let any = !s.jump_prompts().is_empty();
    let opening = s.jump.machine.opening();
    let status = s.jump.machine.status().map(|k| opencode_i18n::I18N.text(k));
    let active = s.jump.active.as_ref().filter(|id| rows.iter().any(|r| &r.prompt.id == *id));
    let query = s.jump.search.text.trim().to_owned();

    let typed = !s.jump.search.text.is_empty();
    let search = row()
        .key("jump-field")
        .h(28.0)
        .pl(8.0)
        .pr(6.0)
        .gap(8.0)
        .center_y()
        .child(icon(&ic::MAGNIFYING_GLASS, 16.0).color(p.icon_muted))
        .child(
            session_view::style::field(&s.jump.search, UI)
                .key("jump-input")
                .label_static(opencode_i18n::t!("dialog.jump.title"))
                .single_line()
                .grow(1.0)
                .shrink()
                .min_w(0.0)
                .color(p.text)
                .placeholder(opencode_i18n::t!("common.search.placeholder"), p.text.alpha(p.text.a * 0.5))
                .caret(p.text, p.selection)
                .selection_style(p.field_selection)
                .on_edit(|s: &mut App| &mut s.jump.search)
                .on_input_change(|s: &mut App, tx| {
                    tx.instant();
                    s.jump.active = s.jump_rows().first().map(|r| r.prompt.id.clone());
                })
                .on_input_key(|s: &mut App, ev, tx| s.jump_key(ev, tx) || s.shortcut(ev, tx)),
        )
        .children((typed && !opening).then(|| {
            el().key("jump-clear")
                .role("button")
                .label_static(opencode_i18n::t!("common.clear"))
                .size(20.0, 20.0)
                .radius(4.0)
                .center()
                .hover(p.hover)
                .hover_motion(INSTANT, INSTANT)
                .on_press_tx(|s: &mut App, tx: &mut Tx| {
                    tx.instant();
                    s.jump.search = TextEdit::new("");
                    s.jump.search.focused = true;
                    s.jump.active = s.jump_rows().first().map(|r| r.prompt.id.clone());
                })
                .child(icon(&ic::CIRCLE_XMARK, 16.0).color(p.icon_muted))
        }));

    let mut list = col().key("jump-results").w_full().px(6.0).pb(6.0).gap(ROW_GAP);
    let content_h = if rows.is_empty() {
        let empty = status.unwrap_or(if any { opencode_i18n::t!("dialog.jump.noMatches") } else { opencode_i18n::t!("dialog.jump.empty") });
        list = col().key("jump-results").w_full().child(col().key("jump-empty").w_full().h(EMPTY_H).center().child(text(empty).font(UI).color(p.muted)));
        EMPTY_H
    } else {
        for r in &rows {
            let is_active = active == Some(&r.prompt.id);
            let (pick, hover_id) = (r.prompt.id.clone(), r.prompt.id.clone());
            let at = r.prompt.created;
            let time = (at > 0).then(|| {
                let full = opencode_i18n::date::format(opencode_i18n::date::Style::MonthDayYearTime, session_view::turn_errors::moment(rune::LocalTime::at(at)));
                text(crate::switcher::relative_time(*s.now_ms, at)).key("time").font(UI.features("\"tnum\"")).color(p.muted).help(full)
            });
            let line = crate::switcher::highlighted(&r.snippet, &query, UI, p.text, p.accent).key("text").grow(1.0).shrink().min_w(0.0);
            let item = row()
                .key(format!("jump-row-{}", r.prompt.id))
                .role("option")
                .label(r.snippet.clone())
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
                    if s.jump.machine.opening() {
                        return;
                    }
                    tx.instant();
                    s.jump.active = inside.then(|| hover_id.clone());
                })
                .on_press_tx(move |s: &mut App, tx: &mut Tx| s.choose_jump(&pick, tx))
                .child(line);
            list = list.child(item.children(time));
        }
        rows.len() as f32 * (ROW_H + ROW_GAP) - ROW_GAP + 6.0
    };
    let scroll_h = content_h.min(max_h - HEAD_H - SECTION_H).max(0.0);

    // The description, or the machine's note once there are rows to keep it beside; Retry after
    // a history that didn't load.
    let note = match status {
        Some(note) if !rows.is_empty() => note,
        _ => opencode_i18n::t!("dialog.jump.description"),
    };
    let retry = matches!(s.jump.machine, State::Failed { .. }).then(|| {
        crate::widgets::button(opencode_i18n::t!("common.retry"), rune_kit::ButtonVariant::Neutral)
            .on_press_tx(|s: &mut App, tx: &mut Tx| {
                tx.instant();
                s.step_jump(Event::Retry, tx);
            })
            .build(crate::palette::kit(p))
            .key("jump-retry")
    });
    let dialog = col()
        .key("jump-dialog")
        .role("dialog")
        .label_static(opencode_i18n::t!("dialog.jump.title"))
        .w(w)
        .h(HEAD_H + SECTION_H + scroll_h)
        .radius(12.0)
        .fill(p.bg_base)
        .elevate(p.overlay)
        .clip_soft(0.0)
        .transition(Transition::none())
        .motion(Spring::NONE)
        .modal(true, |s: &mut App| s.close_jump_dialog(true))
        .child(col().key("jump-search").w_full().pt(6.0).px(6.0).child(search))
        .child(el().key("jump-divider").w_full().h(0.5).mt(8.5).fill(p.border_muted))
        .child(
            row()
                .key("jump-desc")
                .w_full()
                .h(SECTION_H)
                .px(14.0)
                .gap(8.0)
                .center_y()
                .child(text(note).font(UI.lh(20.0)).color(p.muted).truncate().grow(1.0).shrink().min_w(0.0))
                .children(retry),
        )
        .child(session_view::scroller::scroller("jump-scroll").bar(session_view::scroller::Bar::MenuScroll).build::<App>(p, col().role("listbox").w_full().h(scroll_h), list));
    let scrim = el().key("jump-scrim").absolute(0.0, 0.0).w_full().h_full().fill(p.scrim).transition(Transition::none());
    [scrim, dialog.absolute((win_w - w) / 2.0, ((win_h - 480.0) / 2.0).max(48.0)).raise()]
}
