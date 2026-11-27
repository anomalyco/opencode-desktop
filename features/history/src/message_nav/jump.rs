//! Jump to message (Ctrl+Alt+G, ⌥⌘G on macOS), as Desktop's `DialogJumpToMessage`
//! (`session/commands/dialog-jump-to-message.tsx`, `jump-to-message.tsx`): the open session's
//! sent prompts, newest first, narrowed by what's typed, each a snippet around its match and when
//! it was sent. Choosing one puts it at the timeline's top as Previous and Next message do; one
//! the timeline hasn't loaded yet is paged back to first (`machine.rs`).

pub mod machine;

use std::time::Duration;

use opencode_kit::client::Server;
use opencode_kit::model::Message;
use opencode_search::query::{cycle, percent_encode};
use opencode_ui::widgets::{DialogSize, button, dialog_window};
use rune::{Color, Cx, El, Key, KeyEvent, LocalTime, Span, Spring, TextEdit, TextStyle, Transition, Tx, col, el, icon, row, text};
use rune_kit::ButtonVariant;
use rune_theme_opencode::icons as ic;
use serde_json::Value;
use session_view::palette::{Elevate as _, Pal, kit};
use session_view::scroller::{Bar, scroller};
use session_view::style::{INSTANT, UI, field};

use crate::message_nav::MessageNavHost;
use machine::{Effect, Event, State};

pub use machine::Prompt;

/// Overlay stack identifier for the Jump to message dialog (`opencode_kernel::OverlayStack`).
pub const JUMP_OVERLAY_ID: &str = "dialog.jump";

/// Off-thread request to page through all older user prompts for a session.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HistoryRequest {
    pub session: String,
    pub attempt: u32,
}

/// Result of [`fetch_history`], applied on the UI thread via [`apply_history_reply`].
#[derive(Clone, Debug)]
pub struct HistoryReply {
    pub attempt: u32,
    pub result: Result<Vec<Prompt>, String>,
}

/// Open-only dialog context carried inside [`State`] while the Jump to message dialog is open
/// (`dialog-jump-to-message.tsx`, `jump-to-message.tsx`).
#[derive(Clone, Debug, PartialEq, Eq)]
struct OpenDialog {
    session: String,
    active: Option<String>,
    restore_composer: bool,
}

#[derive(Default)]
pub struct JumpState {
    machine: State<OpenDialog>,
    pub search: TextEdit,
}

impl JumpState {
    pub fn open(&self) -> bool {
        self.machine.is_open()
    }

    pub fn is_focused(&self) -> bool {
        self.open() && self.search.focused
    }

    pub fn opening(&self) -> bool {
        self.machine.opening()
    }

    pub fn session(&self) -> &str {
        self.machine.open_data().map_or("", |d| d.session.as_str())
    }

    pub fn restore_composer(&self) -> bool {
        self.machine.open_data().is_some_and(|d| d.restore_composer)
    }

    fn active(&self) -> Option<&str> {
        self.machine.open_data().and_then(|d| d.active.as_deref())
    }

    fn set_active(&mut self, next: Option<String>) {
        if let Some(d) = self.machine.open_data_mut() {
            d.active = next;
        }
    }
}

/// A row: its prompt and the snippet shown (`jump-to-message.tsx` `rows`).
struct Row {
    prompt: Prompt,
    snippet: String,
}

/// Opens the dialog over the open session (`session.jumpToMessage`, disabled without one).
pub fn open(host: &mut dyn MessageNavHost, tx: &mut Tx) -> bool {
    let Some(sid) = host.displayed_session_id().or_else(|| host.active_session_id()).map(str::to_owned) else {
        return false;
    };
    tx.instant();
    let was_open = host.jump().open();
    let prev_restore = host.jump().restore_composer();
    if was_open {
        close(host, false);
    }
    let shell_restore = host.before_jump_open();
    let restore = if was_open { prev_restore } else { shell_restore };
    let return_focus = tx.focused().map(str::to_owned);
    {
        let j = host.jump_mut();
        j.search = TextEdit::new("");
        j.search.focused = true;
    }
    host.on_overlay_opened(JUMP_OVERLAY_ID, return_focus);
    step_jump(host, Event::Start(OpenDialog { session: sid, active: None, restore_composer: restore }), tx);
    let first = rows(host).first().map(|r| r.prompt.id.clone());
    host.jump_mut().set_active(first);
    true
}

/// Closes the Jump to message dialog, notifying `OverlayStack` and restoring composer focus when asked.
pub fn close(host: &mut dyn MessageNavHost, restore_focus: bool) {
    if !host.jump().open() {
        return;
    }
    let effects = host.jump_mut().machine.step(Event::Disposed);
    let restore_composer = effects
        .into_iter()
        .find_map(|effect| match effect {
            Effect::Closed(open) => Some(open.restore_composer),
            _ => None,
        })
        .unwrap_or(false);
    leave(host, restore_focus, restore_composer);
}

fn leave(host: &mut dyn MessageNavHost, restore_focus: bool, restore_composer: bool) {
    {
        let j = host.jump_mut();
        j.search.focused = false;
        j.search.cancel_composition();
    }
    host.on_overlay_closed(JUMP_OVERLAY_ID);
    if restore_focus {
        host.restore_composer_focus(restore_composer);
    }
}

fn step_jump(host: &mut dyn MessageNavHost, event: Event<OpenDialog>, tx: &mut Tx) {
    let effects = host.jump_mut().machine.step(event);
    for effect in effects {
        match effect {
            Effect::HistoryLoad { attempt } => load_history(host, attempt, tx),
            Effect::OpenLoad { .. } => {
                let sid = host.jump().session().to_owned();
                host.read_toward_place(&sid, tx);
            }
            Effect::Jump { id, open } => {
                leave(host, true, open.restore_composer);
                super::jump_to(host, &open.session, &id, tx);
            }
            Effect::Closed(open) => {
                leave(host, false, open.restore_composer);
            }
            // A read already on its way lands with an attempt the machine no longer takes.
            Effect::Cancel => {}
        }
    }
}

/// Older prompts for the picker only: paging them into the timeline would move the reader.
/// A session whose first page holds all of it has none.
fn load_history(host: &mut dyn MessageNavHost, attempt: u32, tx: &mut Tx) {
    let sid = host.jump().session().to_owned();
    if host.all_pages_loaded(&sid) || !host.is_live() {
        step_jump(host, Event::HistoryLoaded { attempt, history: Vec::new() }, tx);
        return;
    }
    if !host.spawn_history_load(HistoryRequest { session: sid, attempt }) {
        step_jump(host, Event::HistoryFailed { attempt }, tx);
    }
}

/// Executes [`HistoryRequest`] off the UI thread against the live [`Server`].
pub fn fetch_history(server: &Server, request: &HistoryRequest) -> HistoryReply {
    let attachment = opencode_i18n::t!("common.attachment");
    HistoryReply {
        attempt: request.attempt,
        result: history(server, &request.session, attachment),
    }
}

/// Applies a completed [`HistoryReply`] back onto `host` on the UI thread.
pub fn apply_history_reply(host: &mut dyn MessageNavHost, reply: HistoryReply, tx: &mut Tx) {
    if !host.jump().open() {
        return;
    }
    let event = match reply.result {
        Ok(history) => Event::HistoryLoaded { attempt: reply.attempt, history },
        Err(_) => Event::HistoryFailed { attempt: reply.attempt },
    };
    step_jump(host, event, tx);
    if host.jump().open() && host.jump().active().is_none() {
        let first = rows(host).first().map(|r| r.prompt.id.clone());
        host.jump_mut().set_active(first);
    }
}

/// Every prompt the dialog can list: the older history, then the timeline's own, which
/// win by id; none with no words, none still waiting to send, none past a revert.
pub fn prompts(host: &dyn MessageNavHost) -> Vec<Prompt> {
    let Some(open) = host.jump().machine.open_data() else {
        return Vec::new();
    };
    let sid = open.session.as_str();
    let mut all: Vec<Prompt> = host.jump().machine.history().to_vec();
    let shown = super::prompt_ids(host, sid);
    if let Some(t) = host.transcript(sid) {
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
    let revert = host.session_revert(sid);
    all.retain(|p| !p.text.trim().is_empty() && revert.is_none_or(|r| p.id.as_str() < r));
    all
}

/// The prompts whose words hold the query, newest first, each as a snippet: 16 before the
/// match (from a word's start) to 90 on or 16 past it, with `…` where it was cut.
fn rows(host: &dyn MessageNavHost) -> Vec<Row> {
    let needle = host.jump().search.text.trim().to_lowercase();
    let mut list = prompts(host);
    list.sort_by_key(|a| std::cmp::Reverse(a.created));
    list.into_iter()
        .filter_map(|prompt| {
            let words: Vec<char> = prompt.text.split_whitespace().collect::<Vec<_>>().join(" ").chars().collect();
            let lower: Vec<char> = words.iter().flat_map(|c| c.to_lowercase()).collect();
            let needle: Vec<char> = needle.chars().collect();
            // A letter that lowers to more than one keeps the text's own places only when none do.
            let at = if lower.len() != words.len() { needle.is_empty().then_some(0) } else { find(&lower, &needle) }?;
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

fn choose(host: &mut dyn MessageNavHost, id: &str, tx: &mut Tx) {
    if !host.jump().open() {
        return;
    }
    tx.instant();
    let sid = host.jump().session().to_owned();
    let loaded = host
        .transcript(&sid)
        .is_some_and(|t| t.messages.iter().any(|m| matches!(m, Message::User { id: x, .. } if x == id)));
    let now = host.clock();
    step_jump(host, Event::Selected { id: id.to_owned(), loaded, now }, tx);
}

/// Key handler while Jump to message is open: Escape closes, arrows cycle rows, Enter jumps.
pub fn key(host: &mut dyn MessageNavHost, ev: &KeyEvent, tx: &mut Tx) -> bool {
    if !host.jump().open() || host.jump().search.is_composing() {
        return false;
    }
    match ev.key {
        Key::Escape => {
            tx.instant();
            close(host, true);
            true
        }
        // While a prompt opens the list holds still (`disabled={props.opening}`).
        Key::Down | Key::Up | Key::Enter if host.jump().machine.opening() => true,
        Key::Down | Key::Up if !ev.mods.shift && !ev.mods.ctrl && !ev.mods.cmd && !ev.mods.alt => {
            tx.instant();
            let list = rows(host);
            if !list.is_empty() {
                let cur = host.jump().active().and_then(|id| list.iter().position(|r| r.prompt.id == id));
                let next = cycle(list.len(), cur, if ev.key == Key::Down { 1 } else { -1 });
                let id = list[next].prompt.id.clone();
                if next == 0 {
                    tx.scroll_to_y("jump-scroll", 0.0);
                } else {
                    tx.scroll_into_view_in("jump-scroll", format!("jump-row-{id}"));
                }
                host.jump_mut().set_active(Some(id));
            }
            true
        }
        Key::Enter if !ev.mods.shift => {
            let list = rows(host);
            let chosen = host
                .jump()
                .active()
                .and_then(|id| list.iter().find(|r| r.prompt.id == id))
                .or_else(|| list.first())
                .map(|r| r.prompt.id.clone());
            if let Some(id) = chosen {
                choose(host, &id, tx);
            }
            true
        }
        _ => false,
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
pub fn history(server: &Server, sid: &str, attachment: &str) -> Result<Vec<Prompt>, String> {
    let mut out = Vec::new();
    let mut cursor: Option<String> = None;
    loop {
        let after = cursor.as_deref().map(|c| format!("&cursor={}", percent_encode(c))).unwrap_or_default();
        let page = server.read(&format!("/api/session/{sid}/message?type=user&limit=200{after}")).map_err(|e| e.to_string())?;
        for m in page["data"].as_array().into_iter().flatten().filter(|m| m["type"] == "user") {
            out.push(Prompt {
                id: m["id"].as_str().unwrap_or_default().to_owned(),
                text: words(m, attachment),
                created: m["time"]["created"].as_i64().unwrap_or(0),
            });
        }
        cursor = page["cursor"]["next"].as_str().map(str::to_owned);
        if cursor.is_none() {
            return Ok(out);
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
pub fn poll(host: &mut dyn MessageNavHost, tx: &mut Tx) -> bool {
    if !host.jump().open() {
        return false;
    }
    let sid = host.jump().session().to_owned();
    if host.displayed_session_id().or_else(|| host.active_session_id()) != Some(sid.as_str()) {
        close(host, false);
        return true;
    }
    let (attempt, target, due) = match &host.jump().machine {
        State::Opening { attempt, target, due, .. } => (*attempt, target.clone(), *due),
        _ => return false,
    };
    let shown = super::prompt_ids(host, &sid).contains(&target);
    let held = host
        .transcript(&sid)
        .is_some_and(|t| t.messages.iter().any(|m| matches!(m, Message::User { id, .. } if *id == target)));
    let event = if shown {
        Event::Opened { attempt }
    } else if held || host.clock() >= due || host.all_pages_loaded(&sid) {
        // Loaded but not a prompt the timeline shows (a steer still waiting), or nothing older.
        Event::OpenFailed { attempt }
    } else {
        host.read_toward_place(&sid, tx);
        tx.wake_after(Duration::from_millis(100));
        return false;
    };
    step_jump(host, event, tx);
    true
}

// --- Thin non-generic callback helpers invoked by `view` ---

fn on_query_change(host: &mut dyn MessageNavHost, tx: &mut Tx) {
    tx.instant();
    let first = rows(host).first().map(|r| r.prompt.id.clone());
    host.jump_mut().set_active(first);
}

fn key_or_shortcut(host: &mut dyn MessageNavHost, ev: &KeyEvent, tx: &mut Tx) -> bool {
    key(host, ev, tx) || host.handle_shortcut(ev, tx)
}

fn clear_search(host: &mut dyn MessageNavHost, tx: &mut Tx) {
    tx.instant();
    {
        let j = host.jump_mut();
        j.search = TextEdit::new("");
        j.search.focused = true;
    }
    let first = rows(host).first().map(|r| r.prompt.id.clone());
    host.jump_mut().set_active(first);
}

fn on_row_hover(host: &mut dyn MessageNavHost, id: &str, inside: bool, tx: &mut Tx) {
    if !host.jump().open() || host.jump().machine.opening() {
        return;
    }
    tx.instant();
    host.jump_mut().set_active(inside.then(|| id.to_owned()));
}

fn on_retry(host: &mut dyn MessageNavHost, tx: &mut Tx) {
    tx.instant();
    step_jump(host, Event::Retry, tx);
}

// --- Display helpers ---

/// The first match of the query in `s`, in the accent colour (`SearchHighlight`).
fn highlighted(s: &str, q: &str, style: TextStyle, color: Color, accent: Color) -> El {
    let t = text(s.to_owned()).font(style).color(color).truncate_fade();
    let lower = s.to_lowercase();
    let needle = q.trim().to_lowercase();
    match lower.find(&needle) {
        Some(at) if !needle.is_empty() && lower.len() == s.len() => t.spans(vec![Span::new(at, at + needle.len()).color(accent)]),
        _ => t,
    }
}

/// `getRelativeTime` (`shell/time.ts`): "Just now", "5m ago", "3h ago", "2d ago", then the local
/// date ("Nov 3", with its year when not this year's).
fn relative_time(now: i64, then: i64) -> String {
    let s = (now - then).div_euclid(1000);
    let m = s.div_euclid(60);
    let h = s.div_euclid(3600);
    let d = s.div_euclid(86_400);
    if s < 60 {
        return opencode_i18n::t!("common.time.justNow").into();
    }
    if m < 60 {
        return opencode_i18n::t!("common.time.minutesAgo.short", count = m);
    }
    if h < 24 {
        return opencode_i18n::t!("common.time.hoursAgo.short", count = h);
    }
    if d < 7 {
        return opencode_i18n::t!("common.time.daysAgo.short", count = d);
    }
    let (at, today) = (LocalTime::at(then), LocalTime::at(now));
    let style = if at.year == today.year { opencode_i18n::date::Style::MonthDay } else { opencode_i18n::date::Style::MonthDayYear };
    opencode_i18n::date::format(style, session_view::turn_errors::moment(at))
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
const SECTION: TextStyle = TextStyle::new(12.0, 440.0).lh(18.0).tracking(0.05);
/// `.fork-dialog .search-list-trailing`: 11, tabular.
const TIME: TextStyle = TextStyle::new(11.0, 440.0).lh(16.0).tracking(0.05).features("'tnum' 1");
const EMPTY_H: f32 = 120.0;

fn search_input_shell(host: &dyn MessageNavHost, p: &Pal) -> El {
    field(&host.jump().search, UI)
        .key("jump-input")
        .label_static(opencode_i18n::t!("dialog.jump.title"))
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
    el().key("jump-clear")
        .role("button")
        .label_static(opencode_i18n::t!("common.clear"))
        .size(20.0, 20.0)
        .radius(4.0)
        .center()
        .hover(p.hover)
        .hover_motion(INSTANT, INSTANT)
        .child(icon(&ic::CLOSE, 16.0).color(p.icon_muted))
}

fn row_shell(p: &Pal, r: &Row, query: &str, is_active: bool, now_ms: i64) -> El {
    let at = r.prompt.created;
    let time = (at > 0).then(|| {
        let full = opencode_i18n::date::format(opencode_i18n::date::Style::MonthDayYearTime, session_view::turn_errors::moment(LocalTime::at(at)));
        text(relative_time(now_ms, at)).key("time").font(TIME).color(p.muted).help(full)
    });
    let line = highlighted(&r.snippet, query, UI, p.text, p.accent).key("text").grow(1.0).shrink().min_w(0.0);
    row()
        .key(format!("jump-row-{}", r.prompt.id))
        .role("option")
        .label(r.snippet.clone())
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
        .child(line)
        .children(time)
}

fn dialog_shell(p: &Pal, w: f32, scroll_h: f32, search: El, note: &'static str, retry: Option<El>, scroll: El) -> El {
    col()
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
        .child(col().key("jump-search").w_full().pt(6.0).px(6.0).child(search))
        .child(rune::hairline(p.border).key("jump-divider").w_full().mt(6.0).mb(2.0))
        .child(
            row()
                .key("jump-desc")
                .w_full()
                .h(SECTION_H)
                .px(14.0)
                .pt(10.0)
                .pb(2.0)
                .gap(8.0)
                .center_y()
                .child(text(note).font(SECTION).color(p.faint).truncate().grow(1.0).shrink().min_w(0.0))
                .children(retry),
        )
        .child(scroll)
}

/// Desktop's `JumpToMessage` (`jump-to-message.tsx`) in `<Dialog size="large" fit>`, as Fork from
/// message draws it: large, as tall as its rows up to the size's height.
pub fn view<H: MessageNavHost + 'static>(s: &H, p: &Pal, cx: &Cx<H>) -> [El; 2] {
    let host: &dyn MessageNavHost = s;
    let [win_w, win_h] = dialog_window(cx, host.zoom());
    let size = DialogSize::Large;
    let w = size.width(win_w);
    let max_h = size.height(win_h);
    let list_rows = rows(host);
    let any = !prompts(host).is_empty();
    let opening = host.jump().machine.opening();
    let status = host.jump().machine.status().map(|k| opencode_i18n::I18N.text(k));
    let active = host.jump().active().filter(|id| list_rows.iter().any(|r| r.prompt.id == *id));
    let query = host.jump().search.text.trim().to_owned();
    let typed = !host.jump().search.text.is_empty();
    let now_ms = host.now_ms();

    let search_input = search_input_shell(host, p)
        .on_edit(|s: &mut H| &mut s.jump_mut().search)
        .on_input_change(|s: &mut H, tx: &mut Tx| on_query_change(s, tx))
        .on_input_key(|s: &mut H, ev, tx| key_or_shortcut(s, ev, tx));
    let clear = (typed && !opening).then(|| clear_button_shell(p).on_press_tx(|s: &mut H, tx: &mut Tx| clear_search(s, tx)));
    let search = row()
        .key("jump-field")
        .h(28.0)
        .pl(8.0)
        .pr(6.0)
        .gap(8.0)
        .center_y()
        .child(icon(&ic::MAGNIFYING_GLASS, 16.0).color(p.icon_muted))
        .child(search_input)
        .children(clear);

    let (list, content_h) = if list_rows.is_empty() {
        let empty = status.unwrap_or(if any { opencode_i18n::t!("dialog.jump.noMatches") } else { opencode_i18n::t!("dialog.jump.empty") });
        let el = col().key("jump-results").w_full().child(col().key("jump-empty").w_full().h(EMPTY_H).center().child(text(empty).font(UI).color(p.muted)));
        (el, EMPTY_H)
    } else {
        let mut el = col().key("jump-results").w_full().px(6.0).pb(6.0).gap(ROW_GAP);
        for r in &list_rows {
            let is_active = active == Some(r.prompt.id.as_str());
            let (pick, hover_id) = (r.prompt.id.clone(), r.prompt.id.clone());
            let item = row_shell(p, r, &query, is_active, now_ms)
                .on_hover_change(move |s: &mut H, inside, tx: &mut Tx| on_row_hover(s, &hover_id, inside, tx))
                .on_press_tx(move |s: &mut H, tx: &mut Tx| choose(s, &pick, tx));
            el = el.child(item);
        }
        (el, list_rows.len() as f32 * (ROW_H + ROW_GAP) - ROW_GAP + 6.0)
    };
    let scroll_h = content_h.min(max_h - HEAD_H - SECTION_H).max(0.0);

    // The description, or the machine's note once there are rows to keep it beside; Retry after
    // a history that didn't load.
    let note = match status {
        Some(note) if !list_rows.is_empty() => note,
        _ => opencode_i18n::t!("dialog.jump.description"),
    };
    let retry = matches!(host.jump().machine, State::Failed { .. }).then(|| {
        button(opencode_i18n::t!("common.retry"), ButtonVariant::Neutral)
            .on_press_tx(|s: &mut H, tx: &mut Tx| on_retry(s, tx))
            .build(kit(p))
            .key("jump-retry")
    });
    let scroll = scroller("jump-scroll").bar(Bar::MenuScroll).build::<H>(p, col().role("listbox").w_full().h(scroll_h), list);
    let dialog = dialog_shell(p, w, scroll_h, search, note, retry, scroll).modal(true, |s: &mut H| close(s, true));
    let scrim = el().key("jump-scrim").absolute(0.0, 0.0).w_full().h_full().fill(p.scrim).transition(Transition::none());
    [scrim, dialog.absolute((win_w - w) / 2.0, size.top(win_h)).raise()]
}
