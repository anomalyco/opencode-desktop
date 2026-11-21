//! The quote's comment editor, as Desktop draws it (`composer/editor/quote-comment.tsx` over
//! `LineCommentEditor`'s compact layout, `line-comment.tsx`, `line-comment.css`): 320 wide, the
//! quoted words over a growing field and one action button. Return saves the comment (Save, Add
//! comment or Done), Ctrl+Return sends the prompt with it (Send, or Steer or Queue while the
//! session works), and `@` lists files to mention. It stands where the Quote toolbar was when
//! it replaced it, else over its card, and keeps its bottom edge as the draft grows.

use std::rc::Rc;

use opencode_kit::client::Server;
use rune::{Color, Cx, El, Icon, KeyEvent, Mods, Shadow, Spring, TextEdit, TextStyle, Transition, Tx, col, el, icon, row, text};
use rune_theme_opencode::icons as ic;
use serde_json::Value;
use session_view::palette::{Elevate as _, Pal, kit};

use super::QuoteHost;
use super::cards::chip_key;
use super::comment::{self, Anchor, Comment, Config, Event, FieldEffect, FieldEvent, FieldKey, Mention};
use super::place::{self, Place, Placing};

/// `quote-comment.tsx` `GAP`, `WIDTH`.
const GAP: f32 = 8.0;
const WIDTH: f32 = 320.0;
/// `line-comment-v2-textarea`: 13 at 440 on the 20 pt base line.
const FIELD: TextStyle = TextStyle::new(13.0, 440.0).lh(20.0).tracking(-0.04);
/// The alert under a failed send: 12 on the 16 pt compact line.
const ALERT: TextStyle = TextStyle::new(12.0, 440.0).lh(16.0);
/// `line-comment-v2-mention-path`: 12/16.
const PATH: TextStyle = TextStyle::new(12.0, 440.0).lh(16.0);

/// `quote-check.svg`, the compact action's tick.
pub static CHECK: Icon = Icon {
    name: "line-comment-check",
    view: 16.0,
    body: r#"<path d="M2.44444 8.22222L6 12.6667L13.5556 3.33333" stroke-linecap="square" stroke="currentColor"/>"#,
};

pub use opencode_runtime::SendResult;

/// Follow-up delivery preference while a session works (`Settings > Follow-up`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum FollowUp {
    #[default]
    Queue,
    Steer,
}

/// Opens `key`'s editor (`quoteComment.set`): from its card with no anchor, or where the
/// toolbar stood.
pub fn open(host: &mut dyn QuoteHost, key: u64, anchor: Option<Anchor>) {
    let Some(saved) = host
        .quotes()
        .items
        .iter()
        .find(|q| q.key == key)
        .and_then(|q| q.quote())
        .map(|q| q.comment.clone().unwrap_or_default())
    else {
        return;
    };
    let q = host.quotes_mut();
    comment::step(&mut q.comment_seq, &mut q.comment, Event::Opened { key, saved, anchor });
    q.opening.set(0.0);
    q.placing.set(Placing::Unplaced);
    host.set_composer_focused(false);
}

/// Closes the editor; Cancel and a save put focus back in the prompt.
pub fn close(host: &mut dyn QuoteHost, focus: bool) {
    let q = host.quotes_mut();
    comment::step(&mut q.comment_seq, &mut q.comment, Event::Closed);
    if focus && !session_view::style::touch_controls() {
        host.set_composer_focused(true);
    }
}

/// The quote's saved comment.
fn saved(host: &dyn QuoteHost, key: u64) -> &str {
    host.quotes()
        .items
        .iter()
        .find(|q| q.key == key)
        .and_then(|q| q.quote())
        .and_then(|q| q.comment.as_deref())
        .unwrap_or("")
}

/// `canDismissOutside`: nothing would be lost.
pub fn can_dismiss(host: &dyn QuoteHost) -> bool {
    let q = host.quotes();
    match (&q.comment, q.comment.key(), q.comment.draft()) {
        (Comment::Sending { .. }, ..) => false,
        (_, Some(key), Some(draft)) => draft.text.trim() == saved(host, key).trim(),
        _ => true,
    }
}

/// The primary action (`onSubmit`): the trimmed draft becomes the quote's comment, an empty one
/// clearing it, and focus goes back to the prompt.
fn save(host: &mut dyn QuoteHost) {
    let (key, comment) = {
        let q = host.quotes();
        let (Some(key), Some(draft)) = (q.comment.key(), q.comment.draft()) else { return };
        (key, draft.text.trim().to_owned())
    };
    if let Some(quote) = host.quotes_mut().items.iter_mut().find(|q| q.key == key).and_then(|q| q.comment.quote_mut()) {
        quote.comment = Some(comment);
    }
    close(host, true);
}

/// Where Ctrl+Return sends the prompt (`quoteSubmit.delivery`): steering or queueing, whichever
/// Return doesn't, while the session works; a plain send otherwise.
fn delivery(host: &dyn QuoteHost) -> &'static str {
    match (host.quote_session_busy(), host.quote_follow_up()) {
        (false, _) => opencode_i18n::t!("session.quote.send"),
        (true, FollowUp::Queue) => opencode_i18n::t!("session.quote.steer"),
        (true, FollowUp::Steer) => opencode_i18n::t!("session.quote.queue"),
    }
}

/// `quoteSubmit.available`: the prompt can go now.
fn send_available(host: &dyn QuoteHost) -> bool {
    host.can_send_quote()
}

/// The alternate action (`quoteComment.send`): this quote goes with this comment, on its own, as
/// Ctrl+Return sends it (`QuoteHost::send_quote`). The editor stays, read-only, until the send's
/// result comes back (`send_result`), as Desktop's waits for the request: it closes once the
/// server has the prompt or the outbox keeps it, and says so, the draft and quote kept, when
/// it failed.
fn send(host: &mut dyn QuoteHost, tx: &mut Tx) {
    let effects = {
        let q = host.quotes_mut();
        comment::step(&mut q.comment_seq, &mut q.comment, Event::SendRequested)
    };
    for effect in effects {
        let comment::Effect::Send { attempt, key, comment } = effect;
        if let Some(quote) = host.quotes_mut().items.iter_mut().find(|q| q.key == key).and_then(|q| q.comment.quote_mut()) {
            quote.comment = Some(comment);
        }
        let result = match host.send_quote(key, tx) {
            None => Some(SendResult::Failed),
            Some(None) => Some(SendResult::Accepted),
            Some(Some(id)) => {
                let q = host.quotes_mut();
                comment::step(&mut q.comment_seq, &mut q.comment, Event::SendOut { attempt, prompt: id.clone() });
                host.quote_send_result(&id)
            }
        };
        if let Some(result) = result {
            settle(host, attempt, result);
        }
    }
}

/// A send's result (`SendResult`), for the editor's send waiting on prompt `id`.
pub fn send_result(host: &mut dyn QuoteHost, id: &str, result: SendResult) {
    match host.quotes().comment.send() {
        Some((attempt, Some(prompt))) if prompt == id => settle(host, attempt, result),
        _ => {}
    }
}

/// The send settles (`send.settled`): it went, so its quote leaves the composer; or it failed,
/// and the failed prompt goes too, so only the editor's send can try it again.
fn settle(host: &mut dyn QuoteHost, attempt: rune::machine::Attempt, result: SendResult) {
    let (Some(key), prompt) = (
        host.quotes().comment.key(),
        host.quotes().comment.send().and_then(|(_, p)| p.map(str::to_owned)),
    ) else {
        return;
    };
    let sent = result != SendResult::Failed;
    {
        let q = host.quotes_mut();
        comment::step(&mut q.comment_seq, &mut q.comment, Event::SendSettled { attempt, sent });
    }
    if sent {
        host.quotes_mut().remove(key);
    } else if let Some(id) = prompt {
        host.delete_failed_quote_prompt(&id);
    }
}

fn config(host: &dyn QuoteHost) -> Config {
    let q = host.quotes();
    let blank = q.comment.draft().is_none_or(|d| d.text.trim().is_empty());
    Config { alternate: Some(!send_available(host)), submitting: q.comment.sending(), allow_empty: true, blank }
}

fn field(host: &mut dyn QuoteHost, event: FieldEvent, tx: &mut Tx) -> bool {
    if host.quotes().comment.field().is_none() {
        return false;
    }
    let (effects, handled) = {
        let q = host.quotes_mut();
        let Some(f) = q.comment.field_mut() else { return false };
        comment::field_step(&mut q.comment_seq, f, event)
    };
    for effect in effects {
        match effect {
            FieldEffect::Search { attempt, query, due } => search(host, attempt, query, due, tx),
            FieldEffect::Insert { path, token } => {
                if let Some(draft) = host.quotes_mut().comment.draft_mut() {
                    let text = format!("@{path} ");
                    draft.set_selection(token.start, token.end);
                    draft.insert(&text);
                }
            }
            FieldEffect::Submit { alternate: true } => send(host, tx),
            FieldEffect::Submit { alternate: false } => save(host),
            FieldEffect::Cancel => close(host, true),
        }
    }
    handled
}

/// The field's file search (`mention.items`).
fn search(host: &mut dyn QuoteHost, attempt: rune::machine::Attempt, query: String, due: f64, tx: &mut Tx) {
    tx.wake_at(due);
    match prepare_path_search(host, &query) {
        PathSearch::Immediate(items) => {
            field(host, FieldEvent::Loaded { attempt, items }, tx);
        }
        PathSearch::Live { server, directory } => {
            request_path_search(&mut host.quotes_mut().mention_fetch, server, directory, attempt, query);
        }
    }
}

/// Resolved destination for a comment field's `@` path search.
pub enum PathSearch {
    Immediate(Vec<String>),
    Live { server: Server, directory: String },
}

/// Prepares a comment field's search for paths to mention (Desktop's `searchPaths`): the
/// fixture's files at once, or the server's search for files and folders.
pub fn prepare_path_search(host: &dyn QuoteHost, query: &str) -> PathSearch {
    match host.live_server() {
        Some(server) => PathSearch::Live { server, directory: host.active_directory() },
        None => PathSearch::Immediate(host.search_local_paths(query, 50)),
    }
}

/// Dispatches an async `/api/fs/find` search into `latest`.
pub fn request_path_search(
    latest: &mut rune::Latest<(rune::machine::Attempt, Option<Vec<String>>)>,
    server: Server,
    directory: String,
    attempt: rune::machine::Attempt,
    query: String,
) {
    latest.request(query, move |query| {
        let path = format!(
            "/api/fs/find?location%5Bdirectory%5D={}&query={}",
            opencode_search::query::percent_encode(&directory),
            opencode_search::query::percent_encode(&query),
        );
        (attempt, server.get(&path).ok().map(|v| found_paths(&v)))
    });
}

fn found_paths(v: &Value) -> Vec<String> {
    v.get("data")
        .unwrap_or(v)
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|f| f["path"].as_str().or(f.as_str()))
        .map(|p| p.strip_prefix("./").or_else(|| p.strip_prefix(".\\")).unwrap_or(p).to_owned())
        .collect()
}

/// Held modifiers (`modifier.changed`): Mod alone shows the alternate send.
pub fn modifiers(host: &mut dyn QuoteHost, mods: Mods, tx: &mut Tx) -> bool {
    let held = (mods.ctrl || mods.cmd) && !mods.alt && !mods.shift;
    let cfg = config(host);
    if host.quotes().comment.field().is_none_or(|f| f.alternate == (held && cfg.alternate.is_some())) {
        return false;
    }
    field(host, FieldEvent::Modifier { held, config: cfg }, tx);
    tx.instant();
    true
}

/// Each frame: an editor whose quote went closes, an unchanged one closes once its field lost
/// focus to a press elsewhere, and the file search's answer or time-out lands.
pub fn poll(host: &mut dyn QuoteHost, tx: &mut Tx) -> bool {
    let mut changed = false;
    let missing = {
        let q = host.quotes();
        q.comment.key().is_some_and(|k| !q.items.iter().any(|item| item.key == k))
    };
    if missing {
        let q = host.quotes_mut();
        let keys: Vec<u64> = q.items.iter().map(|q| q.key).collect();
        comment::step(&mut q.comment_seq, &mut q.comment, Event::Retained(keys));
        changed = true;
    }
    if host.quotes().comment.draft().is_some_and(|d| !d.focused) && can_dismiss(host) {
        close(host, false);
        changed = true;
    }
    if let Some((attempt, items)) = host.quotes().mention_fetch.take() {
        field(host, items.map_or(FieldEvent::Failed { attempt }, |items| FieldEvent::Loaded { attempt, items }), tx);
        changed = true;
    }
    let now = tx.clock();
    let loading = host.quotes().comment.field().and_then(|f| {
        if let Mention::Loading { due, .. } = f.mention {
            Some((due, f.attempt))
        } else {
            None
        }
    });
    if let Some((due, attempt)) = loading
        && now + 1e-9 >= due
    {
        field(host, FieldEvent::Failed { attempt }, tx);
        changed = true;
    }
    changed
}

pub fn key_of(ev: &KeyEvent, tx: &Tx) -> FieldKey {
    if let Some(step) = session_view::popover::nav_step(ev, true) {
        return FieldKey::Step(step as i32);
    }
    let m = ev.mods;
    match ev.key {
        rune::Key::Escape => FieldKey::Escape,
        rune::Key::Enter if !m.shift => FieldKey::Enter { modified: (m.ctrl || m.cmd) && !m.alt, repeat: tx.key_repeat() },
        rune::Key::Tab if !(m.alt || m.ctrl || m.cmd) => FieldKey::Tab,
        _ => FieldKey::Other,
    }
}

struct ViewMeasure {
    vw: f32,
    vh: f32,
    origin: [f32; 2],
    measured: Option<f32>,
    chip: Option<[f32; 4]>,
    scroll: Option<[f32; 4]>,
    dock: Option<[f32; 4]>,
    anchor_origin: Option<[f32; 4]>,
    reduced_motion: bool,
    hover_none: bool,
}

struct EditorLayout {
    width: f32,
    band: [f32; 4],
    available: f32,
    place: Place,
    measured: bool,
    preview_lines: usize,
}

fn resolve_editor_layout(host: &dyn QuoteHost, m: &ViewMeasure) -> EditorLayout {
    let q = host.quotes();
    let [ox, oy] = m.origin;
    let shell = |r: [f32; 4]| [r[0] - ox, r[1] - oy, r[2], r[3]];
    let width = WIDTH.min(m.vw - GAP * 2.0);
    if q.opening.get() == 0.0
        && let Some(h) = m.measured
    {
        q.opening.set(h);
    }
    let opening = q.opening.get();
    let height = m.measured.unwrap_or(0.0);
    let rect = m.chip.map(shell).unwrap_or([GAP, m.vh - GAP, 0.0, 0.0]);
    let header = if host.narrow() { 0.0 } else { session_view::style::HEADER_H };
    let was = q.placing.get();
    let spot = q
        .comment
        .anchor()
        .and_then(|a| (opening > 0.0).then(|| source_position(a, m, shell, width, opening, header, was)).flatten());
    // The bottom edge stays where it opened as the draft grows upward. By its text it's drawn in
    // the timeline's band, so it goes out under the header or the composer with the text.
    let (band, left, bottom, available, place) = match spot {
        Some(spot) => {
            let bottom = spot.top + opening;
            ([0.0, spot.band[0], m.vw, spot.band[1]], spot.left, bottom, (bottom - spot.band[0] - GAP).max(opening), spot.place)
        }
        None => {
            let above = rect[1] - opening - GAP;
            let desired = if above >= GAP { above } else { rect[1] + rect[3] + GAP };
            let bottom = (m.vh - GAP).min((GAP + opening).max(desired + opening));
            ([0.0, 0.0, m.vw, m.vh], rect[0], bottom, (bottom - GAP).max(0.0), Place::Card)
        }
    };
    let left = left.max(GAP).min(m.vw - width - GAP);
    let top = if place == Place::Card { GAP.max(bottom - height) } else { bottom - height };
    let mut next = was;
    for effect in place::step(&mut next, place::Event::Placed { place, at: [left - band[0], top - band[1]] }) {
        place::AT.with(|at| match effect {
            place::Effect::Glide(to) if !m.reduced_motion => at.set(to),
            place::Effect::Land(to) | place::Effect::Glide(to) => at.snap(to),
        });
    }
    q.placing.set(next);
    let preview_lines = if q.comment.anchor().is_some() { 2 } else { 4 };
    EditorLayout { width, band, available, place, measured: m.measured.is_some(), preview_lines }
}

fn on_edit_draft(host: &mut dyn QuoteHost) -> &mut TextEdit {
    // Read-only while its send settles.
    let q = host.quotes_mut();
    if q.comment.sending() {
        return &mut q.spare;
    }
    q.comment.draft_mut().unwrap_or(&mut q.spare)
}

fn on_input_change(host: &mut dyn QuoteHost, tx: &mut Tx) {
    {
        let q = host.quotes_mut();
        comment::step(&mut q.comment_seq, &mut q.comment, Event::Edited);
    }
    let token = host.quotes().comment.draft().and_then(comment::token);
    let now = tx.clock();
    field(host, FieldEvent::Synced { token, now }, tx);
}

fn on_input_key(host: &mut dyn QuoteHost, ev: &KeyEvent, tx: &mut Tx) -> bool {
    let key = key_of(ev, tx);
    if key == FieldKey::Other {
        return false;
    }
    tx.instant();
    let cfg = config(host);
    field(host, FieldEvent::Key { key, config: cfg }, tx)
}

fn on_action_press(host: &mut dyn QuoteHost, tx: &mut Tx) {
    tx.instant();
    let modified = {
        let m = tx.mods();
        (m.ctrl || m.cmd) && !m.alt && !m.shift
    };
    let cfg = config(host);
    field(host, FieldEvent::Clicked { modified, config: cfg }, tx);
}

fn on_rested(host: &mut dyn QuoteHost, tx: &mut Tx) {
    let mut placing = host.quotes().placing.get();
    place::step(&mut placing, place::Event::Rested);
    host.quotes().placing.set(placing);
    tx.live_only();
}

fn on_field_event(host: &mut dyn QuoteHost, event: FieldEvent, tx: &mut Tx) {
    field(host, event, tx);
}

fn preview_shell(p: &Pal, quote_text: &str, preview_lines: usize) -> El {
    row()
        .key("line-comment-preview")
        .w_full()
        .child(el().key("rule").w(2.0).h_full().fill(p.layer3))
        .child(
            text(quote_text.to_owned())
                .key("quote")
                .font(FIELD)
                .color(p.muted)
                .wrap()
                .max_lines(preview_lines)
                .dir(rune::Direction::Auto)
                .pl(12.0)
                .shrink()
                .min_w(0.0)
                .grow(1.0),
        )
}

fn input_shell(host: &dyn QuoteHost, p: &Pal, draft: &TextEdit) -> El {
    session_view::style::field(draft, FIELD)
        .key("quote-comment-field")
        .label_static(opencode_i18n::t!("ui.lineComment.submit"))
        .color(p.text)
        .w_full()
        .min_h(20.0)
        .max_h(160.0)
        .placeholder(opencode_i18n::t!("session.quote.optionalCommentPlaceholder"), p.faint)
        .caret(p.text, p.selection)
        .selection_style(p.field_selection)
        .spellcheck(host.spellcheck())
        // `dir="auto"` and `unicode-bidi: plaintext` (`line-comment.tsx`): each line in its own direction.
        .dir(rune::Direction::Auto)
        .dir_by_paragraph()
}

fn alert_shell(p: &Pal, failed: bool) -> Option<El> {
    failed.then(|| {
        row()
            .key("quote-comment-failed")
            .role("alert")
            .w_full()
            .px(12.0)
            .pb(12.0)
            .child(text(opencode_i18n::t!("session.quote.sendFailed")).font(ALERT).color(p.text).wrap())
    })
}

/// The editor (`QuoteCommentEditor`).
pub fn view<H: QuoteHost + 'static>(s: &H, p: &Pal, cx: &mut Cx<H>) -> Option<El> {
    let q = s.quotes();
    let key = q.comment.key()?;
    let quote = q.items.iter().find(|item| item.key == key)?.quote()?;
    let draft = q.comment.draft()?;
    let f = q.comment.field()?;
    let sending = q.comment.sending();
    let [vw, vh] = cx.layout_size("zoom").unwrap_or([f32::INFINITY, f32::INFINITY]);
    let origin = cx.layout_with("zoomed", |r| [r[0], r[1]]).unwrap_or([0.0, 0.0]);
    let measured = cx.layout_size("quote-comment").map(|[_, h]| h).filter(|h| *h > 0.0);
    let chip = cx.window_layout(&chip_key(key));
    let opening = if q.opening.get() == 0.0 { measured.unwrap_or(0.0) } else { q.opening.get() };
    let (scroll, dock, anchor_origin) = match q.comment.anchor().filter(|_| opening > 0.0) {
        Some(a) => (
            cx.window_layout(&format!("scroll-{}", a.session)),
            cx.window_layout("composer-dock").or_else(|| cx.window_layout("dock")),
            cx.window_layout(&a.path),
        ),
        None => (None, None, None),
    };
    let layout = resolve_editor_layout(
        s,
        &ViewMeasure {
            vw,
            vh,
            origin,
            measured,
            chip,
            scroll,
            dock,
            anchor_origin,
            reduced_motion: cx.is_reduced_motion(),
            hover_none: cx.hover_none(),
        },
    );

    let preview = preview_shell(p, &quote.text, layout.preview_lines);
    let input = input_shell(s, p, draft)
        .on_edit(|s: &mut H| on_edit_draft(s))
        .on_input_change(|s: &mut H, tx: &mut Tx| on_input_change(s, tx))
        .on_input_key(|s: &mut H, ev, tx| on_input_key(s, ev, tx));
    let mentions = mention_rows::<H>(p, &f.mention, Rc::new(|s: &mut H, e: FieldEvent, tx: &mut Tx| on_field_event(s, e, tx)));
    let field_col = col().key("line-comment-field").grow(1.0).shrink().min_w(0.0).child(input).children(mentions);
    let (action_el, enabled) = action_shell(s, p, draft.text.trim().is_empty(), f.alternate, sending, key);
    let action = if enabled {
        action_el.on_press_tx(|s: &mut H, tx: &mut Tx| on_action_press(s, tx))
    } else {
        action_el
    };
    let input_row = row().key("line-comment-input").w_full().gap(12.0).items_end().min_h(20.0).child(field_col).child(action);
    let shell_col = col().key("line-comment-shell").w_full().padding(12.0).gap(10.0).child(preview).child(input_row);
    let alert = alert_shell(p, q.comment.failed());
    // `MenuScroll` (`quote-comment.tsx:147`).
    let body = session_view::scroller::scroller("quote-comment-scroll")
        .bar(session_view::scroller::Bar::MenuScroll)
        .build::<H>(p, col().w_full().max_h(layout.available), col().w_full().child(shell_col).children(alert));
    let editor = col()
        .key("quote-comment")
        // Placed by its live offset, which glides when it changes side.
        .absolute(0.0, 0.0)
        .live_offset(place::AT.with(|at| at.smooth(place::spring(s.knobs()))))
        .on_drawn_layout_tx(|s: &mut H, _, tx: &mut Tx| on_rested(s, tx))
        .w(layout.width)
        .radius(8.0)
        .fill(p.layer1)
        .elevate(p.floating)
        .clip_soft(0.0)
        // Unmeasured, it lays out unseen, so the field can take focus as it mounts.
        .opacity(if layout.measured { 1.0 } else { 0.0 })
        .opacity_motion(rune::Anim::INSTANT)
        .transition(Transition::none())
        .motion(Spring::NONE)
        .preserve_focus_on_press(true)
        .keeps_text_selection()
        .on_press(|_: &mut H| {})
        .child(body);
    let layer = el().key("quote-comment-band").absolute(layout.band[0], layout.band[1]);
    let layer = if layout.place == Place::Card {
        layer.w_full().h_full()
    } else {
        layer.w(layout.band[2]).h(layout.band[3]).clip_rounded(0.0)
    };
    Some(layer.child(editor))
}

/// Where the editor stands by its quote's text: its top and left for its opening height, and
/// the timeline's band under the header and over the composer, `[top, height]`, it's drawn in.
struct Spot {
    top: f32,
    left: f32,
    place: Place,
    band: [f32; 2],
}

/// `ComposerQuoteAnchor` and `quotePosition`: over a short or backward quote, else under it,
/// whichever fits in the timeline, centred on its visible lines. Where neither fits it keeps the
/// side it's on and scrolls with its text, out of the band when the text goes (Kit, t1564);
/// while the text's row isn't laid out it waits past the band's edge the text left by.
fn source_position(
    a: &Anchor,
    m: &ViewMeasure,
    shell: impl Fn([f32; 4]) -> [f32; 4],
    width: f32,
    height: f32,
    header: f32,
    was: Placing,
) -> Option<Spot> {
    let view = shell(m.scroll?);
    let (b_top, b_bottom, b_left, b_right) = (view[1], view[1] + view[3], view[0], view[0] + view[2]);
    // The timeline runs on under the composer.
    let dock = m.dock.map(&shell).map_or(b_bottom, |d| d[1]);
    let band = [b_top + header, (b_bottom.min(dock) - b_top - header).max(0.0)];
    let (high, low) = (band[0] + GAP, band[0] + band[1] - GAP - height);
    let spot = |top: f32, left: f32, place: Place| Some(Spot { top, left, place, band });
    if m.hover_none {
        return spot(low.max(high), (b_left + b_right - width) * 0.5, Place::Pinned);
    }
    let centred = |l: f32, r: f32| ((l + r - width) / 2.0).max(b_left + GAP).min((b_left + GAP).max(b_right - width - GAP));
    let preferred = if a.short || a.backward { Place::Above } else { Place::Below };
    let side = match was.place() {
        Some(side @ (Place::Above | Place::Below)) => side,
        _ => preferred,
    };
    let Some(origin) = m.anchor_origin.map(&shell) else {
        let left = was.at().map_or_else(|| centred(b_left, b_right), |[x, _]| x);
        let gone_up = was.at().is_some_and(|[_, y]| y < band[1] / 2.0);
        return spot(if gone_up { band[0] - height * 2.0 - GAP } else { band[0] + band[1] + GAP }, left, side);
    };
    let rects: Vec<[f32; 4]> = a.rects.iter().map(|r| [origin[0] + r[0], origin[1] + r[1], r[2], r[3]]).collect();
    let (first, last) = (rects.first()?, rects.last()?);
    let (top, bottom) = (first[1], last[1] + last[3]);
    let visible: Vec<&[f32; 4]> = rects.iter().filter(|r| r[1] + r[3] > b_top.max(top) && r[1] < b_bottom.min(bottom)).collect();
    let shown: Vec<&[f32; 4]> = if visible.is_empty() { rects.iter().collect() } else { visible };
    let left = shown.iter().map(|r| r[0]).fold(f32::INFINITY, f32::min);
    let right = shown.iter().map(|r| r[0] + r[2]).fold(f32::NEG_INFINITY, f32::max);
    let y = |place: Place| if place == Place::Above { top - height - GAP } else { bottom + GAP };
    let other = if preferred == Place::Above { Place::Below } else { Place::Above };
    let fits = |place: Place| (high..=low).contains(&y(place));
    let place = [preferred, other].into_iter().find(|&p| fits(p)).unwrap_or(side);
    spot(y(place), centred(left, right), place)
}

/// `--v2-elevation-button-neutral`: a drop and the 0.5px ring outside the button, and in dark the
/// 0.5px lit top edge (`theme.css`).
pub fn neutral_lift(p: &Pal) -> [Shadow; 4] {
    let (y, blur) = if p.dark { (1.0, 2.0) } else { (1.0, 1.5) };
    let top = if p.dark { Shadow { y: -0.5, color: Color::WHITE.alpha(0.1), ..Shadow::default() } } else { Shadow::default() };
    [Shadow { y, blur, color: p.q.neutral_shadow, ..Shadow::default() }, Pal::ring_out(p.q.neutral_ring), top, Shadow::default()]
}

/// The compact action (`line-comment-v2-submit`, a small `IconButton`): a tick in `ghost-muted`
/// over an empty draft or `neutral` over a written one; with Mod held, the send's arrow in the
/// `submit` look; a loader while the send settles. Its tooltip names the action and its keys.
fn action_shell(host: &dyn QuoteHost, p: &Pal, empty: bool, alternate: bool, sending: bool, key: u64) -> (El, bool) {
    let available = send_available(host);
    let primary = if !saved(host, key).trim().is_empty() {
        opencode_i18n::t!("common.save")
    } else if !empty {
        opencode_i18n::t!("session.quote.addComment")
    } else {
        opencode_i18n::t!("session.quote.done")
    };
    let alt_keys = [opencode_i18n::t!("common.key.ctrl").to_owned(), "⏎".to_owned()];
    let label = if alternate { delivery(host) } else { primary };
    let glyph = if sending {
        rune_kit::Loader::new().size(16.0).build(kit(p))
    } else if alternate {
        icon(&ic::ARROW_UP, 16.0).color(p.q.submit_text)
    } else {
        icon(&CHECK, 16.0).color(if empty { p.icon_muted } else { p.text })
    };
    let face = if alternate || sending {
        session_view::composer::submit_button(p, true).key("quote-comment-submit").size(20.0, 20.0).radius(4.0)
    } else if empty {
        el().key("quote-comment-submit").role("button").size(20.0, 20.0).radius(4.0).center().hover(p.hover).press(p.pressed)
    } else {
        el().key("quote-comment-submit")
            .role("button")
            .size(20.0, 20.0)
            .radius(4.0)
            .center()
            .fill(if p.dark && p.theme == 0 { Color::WHITE.alpha(0.06) } else { p.q.neutral })
            .elevate(neutral_lift(p))
            .hover(p.hover)
            .press(p.pressed)
    };
    let disabled = sending || (alternate && !available);
    // The action showing and its keys; while the tick shows, the send on a row of its own under
    // it (`line-comment.tsx`, a `flex-col gap-1.5` of rows).
    let tip = if alternate {
        face.help(label).help_keys(alt_keys)
    } else {
        face.help(label).help_keys(["⏎".to_owned()]).help_row(delivery(host)).help_keys(alt_keys)
    };
    let b = tip.label(label).help_above().motion(Spring::NONE).child(glyph);
    if disabled {
        (b.opacity(if sending { 1.0 } else { 0.5 }).focusable(false), false)
    } else {
        (b, true)
    }
}

fn mention_row_shell(p: &Pal, i: usize, path: &str, active: bool) -> El {
    let (dir, name) = match path.strip_suffix('/') {
        Some(_) => (path, ""),
        None => path.rfind(['/', '\\']).map_or(("", path), |at| (&path[..=at], &path[at + 1..])),
    };
    row()
        .key(format!("mention-{i}"))
        .role("option")
        .label(path.to_owned())
        .w_full()
        .gap(8.0)
        .px(8.0)
        .py(6.0)
        .radius(4.0)
        .center_y()
        .fill(if active { p.layer3 } else { Color::TRANSPARENT })
        .child(icon(rune_theme_opencode::file_icons::file(path), 16.0).color(p.text))
        .child(
            row()
                .key("path")
                .shrink()
                .min_w(0.0)
                .center_y()
                .child(text(dir.to_owned()).font(PATH).color(p.muted).truncate().shrink().min_w(0.0))
                .children((!name.is_empty()).then(|| text(name.to_owned()).font(PATH).color(p.text))),
        )
}

pub type FieldAct<H> = Rc<dyn Fn(&mut H, FieldEvent, &mut Tx)>;

/// `line-comment-v2-mention-list`: up to ten paths under the field, 8 below it, 4 in, rows 4
/// apart on `bg-layer-02` in a 0.5 `border-muted` hairline; the lit row on `bg-layer-03`. A
/// row is the file's icon, then its folder (muted, cut at its end) and name.
pub fn mention_rows<H: 'static>(
    p: &Pal,
    mention: &Mention,
    act: FieldAct<H>,
) -> Option<El> {
    let items = mention.items();
    if items.is_empty() {
        return None;
    }
    let active = mention.active();
    let rows = items.iter().enumerate().map(|(i, path)| {
        let (hover, pick) = (Rc::clone(&act), Rc::clone(&act));
        mention_row_shell(p, i, path, active == Some(path.as_str()))
            .on_pointer_move_local(move |s: &mut H, _, tx: &mut Tx| {
                tx.instant();
                hover(s, FieldEvent::Hovered(i), tx);
            })
            .on_press_tx(move |s: &mut H, tx: &mut Tx| {
                tx.instant();
                pick(s, FieldEvent::Picked(i), tx);
            })
    });
    Some(
        // `line-comment-v2-mention-list`: a plain `overflow: auto` box's bar (`line-comment.css:294-296`).
        session_view::scroller::scroller("line-comment-mentions").build::<H>(
            p,
            col().w_full().mt(8.0).padding(4.0).max_h(180.0).radius(6.0).fill(p.layer2).border(0.5, p.border_muted).border_layout(),
            col().w_full().gap(4.0).children(rows),
        ),
    )
}
