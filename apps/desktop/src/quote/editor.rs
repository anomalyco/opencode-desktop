//! The quote's comment editor, as Desktop draws it (`composer/editor/quote-comment.tsx` over
//! `LineCommentEditor`'s compact layout, `line-comment.tsx`, `line-comment.css`): 320 wide, the
//! quoted words over a growing field and one action button. Return saves the comment (Save, Add
//! comment or Done), Ctrl+Return sends the prompt with it (Send, or Steer or Queue while the
//! session works), and `@` lists files to mention. It stands where the Quote toolbar was when
//! it replaced it, else over its card, and keeps its bottom edge as the draft grows.

use rune::{Color, Cx, El, Icon, KeyEvent, Mods, Spring, TextStyle, Transition, Tx, col, el, icon, row, text};
use rune_theme_opencode::icons as ic;

use crate::App;
use crate::palette::{Elevate as _, Pal};
use crate::send_failed::SendResult;
use crate::quote::comment::{self, Anchor, Comment, Config, Event, FieldEffect, FieldEvent, FieldKey, Mention};

/// `quote-comment.tsx` `GAP`, `WIDTH`.
const GAP: f32 = 8.0;
const WIDTH: f32 = 320.0;
/// `line-comment-v2-textarea`: 13 at 440 on the 20 pt base line.
pub(crate) const FIELD: TextStyle = TextStyle::new(13.0, 440.0).lh(20.0).tracking(-0.04);
/// The alert under a failed send: 12 on the 16 pt compact line.
const ALERT: TextStyle = TextStyle::new(12.0, 440.0).lh(16.0);
/// `line-comment-v2-mention-path`: 12/16.
const PATH: TextStyle = TextStyle::new(12.0, 440.0).lh(16.0);

/// `quote-check.svg`, the compact action's tick.
static CHECK: Icon = Icon {
    name: "line-comment-check",
    view: 16.0,
    body: r#"<path d="M2.44444 8.22222L6 12.6667L13.5556 3.33333" stroke-linecap="square" stroke="currentColor"/>"#,
};

/// Opens `key`'s editor (`quoteComment.set`): from its card with no anchor, or where the
/// toolbar stood.
pub fn open(s: &mut App, key: u64, anchor: Option<Anchor>) {
    let Some(quote) = s.quote.items.iter().find(|q| q.key == key).and_then(|q| q.quote()) else { return };
    let saved = quote.comment.clone().unwrap_or_default();
    let q = &mut *s.quote;
    comment::step(&mut q.comment_seq, &mut q.comment, Event::Opened { key, saved, anchor });
    q.opening.set(0.0);
    s.composer.focused = false;
}

/// Closes the editor; Cancel and a save put focus back in the prompt.
pub fn close(s: &mut App, focus: bool) {
    let q = &mut *s.quote;
    comment::step(&mut q.comment_seq, &mut q.comment, Event::Closed);
    if focus {
        s.composer.focused = true;
    }
}

/// The quote's saved comment.
fn saved(s: &App, key: u64) -> &str {
    s.quote.items.iter().find(|q| q.key == key).and_then(|q| q.quote()).and_then(|q| q.comment.as_deref()).unwrap_or("")
}

/// `canDismissOutside`: nothing would be lost.
pub fn can_dismiss(s: &App) -> bool {
    match (&s.quote.comment, s.quote.comment.key(), s.quote.comment.draft()) {
        (Comment::Sending { .. }, ..) => false,
        (_, Some(key), Some(draft)) => draft.text.trim() == saved(s, key).trim(),
        _ => true,
    }
}

/// The primary action (`onSubmit`): the trimmed draft becomes the quote's comment, an empty one
/// clearing it, and focus goes back to the prompt.
fn save(s: &mut App) {
    let (Some(key), Some(draft)) = (s.quote.comment.key(), s.quote.comment.draft()) else { return };
    let comment = draft.text.trim().to_owned();
    if let Some(quote) = s.quote.items.iter_mut().find(|q| q.key == key).and_then(|q| q.comment.quote_mut()) {
        quote.comment = Some(comment);
    }
    close(s, true);
}

/// Where Ctrl+Return sends the prompt (`quoteSubmit.delivery`): steering or queueing, whichever
/// Return doesn't, while the session works; a plain send otherwise.
fn delivery(s: &App) -> &'static str {
    let working = matches!(&*s.route, crate::Route::Session { id, .. } if s.busy(id));
    match (working, s.settings.follow_up) {
        (false, _) => opencode_i18n::t!("session.quote.send"),
        (true, crate::settings::FollowUp::Queue) => opencode_i18n::t!("session.quote.steer"),
        (true, crate::settings::FollowUp::Steer) => opencode_i18n::t!("session.quote.queue"),
    }
}

/// `quoteSubmit.available`: the prompt can go now.
fn send_available(s: &App) -> bool {
    matches!(*s.route, crate::Route::Session { .. }) && s.attach.uploads.is_empty() && s.opening.is_none()
}

/// The alternate action (`quoteComment.send`): this quote goes with this comment, on its own, as
/// Ctrl+Return sends it (`App::send_quote`). The editor stays, read-only, until the send's
/// result comes back (`send_result`), as Desktop's waits for the request: it closes once the
/// server has the prompt or the outbox keeps it, and says so, the draft and quote kept, when
/// it failed.
fn send(s: &mut App, tx: &mut Tx) {
    let q = &mut *s.quote;
    for effect in comment::step(&mut q.comment_seq, &mut q.comment, Event::SendRequested) {
        let comment::Effect::Send { attempt, key, comment } = effect;
        if let Some(quote) = s.quote.items.iter_mut().find(|q| q.key == key).and_then(|q| q.comment.quote_mut()) {
            quote.comment = Some(comment);
        }
        let result = match s.send_quote(key, tx) {
            None => Some(SendResult::Failed),
            Some(None) => Some(SendResult::Accepted),
            Some(Some(id)) => {
                let q = &mut *s.quote;
                comment::step(&mut q.comment_seq, &mut q.comment, Event::SendOut { attempt, prompt: id.clone() });
                s.send_result(&id)
            }
        };
        if let Some(result) = result {
            settle(s, attempt, result);
        }
    }
}

/// A send's result (`send_failed::SendResult`), for the editor's send waiting on prompt `id`.
pub fn send_result(s: &mut App, id: &str, result: SendResult) {
    match s.quote.comment.send() {
        Some((attempt, Some(prompt))) if prompt == id => settle(s, attempt, result),
        _ => {}
    }
}

/// The send settles (`send.settled`): it went, so its quote leaves the composer; or it failed,
/// and the failed prompt goes too, so only the editor's send can try it again.
fn settle(s: &mut App, attempt: rune::machine::Attempt, result: SendResult) {
    let Some(key) = s.quote.comment.key() else { return };
    let prompt = s.quote.comment.send().and_then(|(_, p)| p.map(str::to_owned));
    let sent = result != SendResult::Failed;
    let q = &mut *s.quote;
    comment::step(&mut q.comment_seq, &mut q.comment, Event::SendSettled { attempt, sent });
    if sent {
        s.quote.remove(key);
    } else if let Some(id) = prompt {
        s.outbox_action(&id, "outbox-delete");
    }
}

fn config(s: &App) -> Config {
    let blank = s.quote.comment.draft().is_none_or(|d| d.text.trim().is_empty());
    Config { alternate: Some(!send_available(s)), submitting: s.quote.comment.sending(), allow_empty: true, blank }
}

fn field(s: &mut App, event: FieldEvent, tx: &mut Tx) -> bool {
    let q = &mut *s.quote;
    let Some(f) = q.comment.field_mut() else { return false };
    let (effects, handled) = comment::field_step(&mut q.comment_seq, f, event);
    for effect in effects {
        match effect {
            FieldEffect::Search { attempt, query, due } => search(s, attempt, query, due, tx),
            FieldEffect::Insert { path, token } => {
                if let Some(draft) = s.quote.comment.draft_mut() {
                    let text = format!("@{path} ");
                    draft.set_selection(token.start, token.end);
                    draft.insert(&text);
                }
            }
            FieldEffect::Submit { alternate: true } => send(s, tx),
            FieldEffect::Submit { alternate: false } => save(s),
            FieldEffect::Cancel => close(s, true),
        }
    }
    handled
}

/// The field's file search (`mention.items`).
fn search(s: &mut App, attempt: rune::machine::Attempt, query: String, due: f64, tx: &mut Tx) {
    tx.wake_at(due);
    if let Some(items) = search_paths(s, attempt, query, |s| &mut s.quote.mention_fetch) {
        field(s, FieldEvent::Loaded { attempt, items }, tx);
    }
}

/// A comment field's search for paths to mention (Desktop's `searchPaths`): the fixture's files
/// at once, or the server's search for files and folders, whose answer lands in `latest`.
pub(crate) fn search_paths(
    s: &mut App,
    attempt: rune::machine::Attempt,
    query: String,
    latest: impl FnOnce(&mut App) -> &mut rune::Latest<(rune::machine::Attempt, Option<Vec<String>>)>,
) -> Option<Vec<String>> {
    let crate::Source::Live { server, .. } = &*s.source else {
        let index = &s.mentions.file_index;
        let targets: Vec<[crate::mentions::fuzzysort::Target; 1]> = index.iter().map(|p| [crate::mentions::fuzzysort::Target::new(&crate::mentions::shown_path(p))]).collect();
        let objects: Vec<&[crate::mentions::fuzzysort::Target]> = targets.iter().map(|t| &t[..]).collect();
        return Some(crate::mentions::fuzzysort::go(&query.to_lowercase(), &objects).into_iter().map(|i| crate::mentions::shown_path(&index[i])).collect());
    };
    let (server, directory) = (server.clone(), s.composer_directory());
    latest(s).request(query, move |query| {
        let path = format!("/api/fs/find?location%5Bdirectory%5D={}&query={}", crate::switcher::encode(&directory), crate::switcher::encode(&query));
        (attempt, server.get(&path).ok().map(|v| crate::mentions::found_paths(&v).iter().map(|p| crate::mentions::shown_path(p)).collect()))
    });
    None
}

/// Held modifiers (`modifier.changed`): Mod alone shows the alternate send.
pub fn modifiers(s: &mut App, mods: Mods, tx: &mut Tx) -> bool {
    let held = (mods.ctrl || mods.cmd) && !mods.alt && !mods.shift;
    if s.quote.comment.field().is_none_or(|f| f.alternate == (held && config(s).alternate.is_some())) {
        return false;
    }
    let config = config(s);
    field(s, FieldEvent::Modifier { held, config }, tx);
    tx.instant();
    true
}

/// Each frame: an editor whose quote went closes, an unchanged one closes once its field lost
/// focus to a press elsewhere, and the file search's answer or time-out lands.
pub fn poll(s: &mut App, tx: &mut Tx) -> bool {
    let mut changed = false;
    let keys: Vec<u64> = s.quote.items.iter().map(|q| q.key).collect();
    if s.quote.comment.key().is_some_and(|k| !keys.contains(&k)) {
        let q = &mut *s.quote;
        comment::step(&mut q.comment_seq, &mut q.comment, Event::Retained(keys));
        changed = true;
    }
    if s.quote.comment.draft().is_some_and(|d| !d.focused) && can_dismiss(s) {
        close(s, false);
        changed = true;
    }
    if let Some((attempt, items)) = s.quote.mention_fetch.take() {
        field(s, items.map_or(FieldEvent::Failed { attempt }, |items| FieldEvent::Loaded { attempt, items }), tx);
        changed = true;
    }
    let now = tx.clock();
    let loading = s.quote.comment.field().and_then(|f| if let Mention::Loading { due, .. } = f.mention { Some((due, f.attempt)) } else { None });
    if let Some((due, attempt)) = loading
        && now + 1e-9 >= due
    {
        field(s, FieldEvent::Failed { attempt }, tx);
        changed = true;
    }
    changed
}

pub(crate) fn key_of(ev: &KeyEvent, tx: &Tx) -> FieldKey {
    if let Some(step) = crate::popover::nav_step(ev, true) {
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

/// The editor (`QuoteCommentEditor`).
pub fn view(s: &App, p: &Pal, cx: &mut Cx<App>) -> Option<El> {
    let key = s.quote.comment.key()?;
    let quote = s.quote.items.iter().find(|q| q.key == key)?.quote()?;
    let draft = s.quote.comment.draft()?;
    let f = s.quote.comment.field()?;
    let sending = s.quote.comment.sending();
    let [vw, vh] = cx.layout_size("zoom").map_or([f32::INFINITY, f32::INFINITY], |[w, h]| [w, h]);
    let [ox, oy] = cx.layout_with("zoomed", |r| [r[0], r[1]]).unwrap_or([0.0, 0.0]);
    let shell = |r: [f32; 4]| [r[0] - ox, r[1] - oy, r[2], r[3]];
    let width = WIDTH.min(vw - GAP * 2.0);
    let measured = cx.layout_size("quote-comment").map(|[_, h]| h).filter(|h| *h > 0.0);
    if s.quote.opening.get() == 0.0
        && let Some(h) = measured
    {
        s.quote.opening.set(h);
    }
    let opening = s.quote.opening.get();
    let height = measured.unwrap_or(0.0);
    let rect = cx.window_layout(&crate::quote_cards::chip_key(key)).map(shell).unwrap_or([GAP, vh - GAP, 0.0, 0.0]);
    let source = s.quote.comment.anchor().and_then(|a| (opening > 0.0).then(|| source_position(a, cx, shell, width, opening)).flatten());
    let left = source.map_or(rect[0], |(_, l)| l).max(GAP).min(vw - width - GAP);
    // The bottom edge stays where it opened as the draft grows upward.
    let above = rect[1] - opening - GAP;
    let desired = source.map_or(if above >= GAP { above } else { rect[1] + rect[3] + GAP }, |(t, _)| t);
    let bottom = (vh - GAP).min((GAP + opening).max(desired + opening));
    let available = (bottom - GAP).max(0.0);
    let top = GAP.max(bottom - height);

    let preview_lines = if s.quote.comment.anchor().is_some() { 2 } else { 4 };
    let preview = row()
        .key("line-comment-preview")
        .w_full()
        .child(el().key("rule").w(2.0).h_full().fill(p.layer3))
        .child(text(quote.text.clone()).key("quote").font(FIELD).color(p.muted).wrap().max_lines(preview_lines).dir(rune::Direction::Auto).pl(12.0).shrink().min_w(0.0).grow(1.0));
    let input = session_view::style::field(draft, FIELD)
        .key("quote-comment-field")
        .label_static(opencode_i18n::t!("ui.lineComment.submit"))
        .color(p.text)
        .w_full()
        .min_h(20.0)
        .max_h(160.0)
        .placeholder(opencode_i18n::t!("session.quote.optionalCommentPlaceholder"), p.faint)
        .caret(p.text, p.selection)
        .selection_style(p.field_selection)
        .spellcheck(s.settings.spellcheck)
        // `dir="auto"` and `unicode-bidi: plaintext` (`line-comment.tsx`): each line in its own direction.
        .dir(rune::Direction::Auto)
        .dir_by_paragraph()
        .on_edit(|s: &mut App| {
            // Read-only while its send settles.
            let q = &mut *s.quote;
            if q.comment.sending() {
                return &mut q.spare;
            }
            q.comment.draft_mut().unwrap_or(&mut q.spare)
        })
        .on_input_change(|s: &mut App, tx: &mut Tx| {
            let q = &mut *s.quote;
            comment::step(&mut q.comment_seq, &mut q.comment, Event::Edited);
            let token = s.quote.comment.draft().and_then(comment::token);
            let now = tx.clock();
            field(s, FieldEvent::Synced { token, now }, tx);
        })
        .on_input_key(|s: &mut App, ev, tx| {
            let key = key_of(ev, tx);
            if key == FieldKey::Other {
                return false;
            }
            tx.instant();
            let config = config(s);
            field(s, FieldEvent::Key { key, config }, tx)
        });
    let field_col = col().key("line-comment-field").grow(1.0).shrink().min_w(0.0).child(input).children(mention_list(p, &f.mention));
    let action = action(s, p, draft.text.trim().is_empty(), f.alternate, sending, key);
    let input_row = row().key("line-comment-input").w_full().gap(12.0).items_end().min_h(20.0).child(field_col).child(action);
    let shell_col = col().key("line-comment-shell").w_full().padding(12.0).gap(10.0).child(preview).child(input_row);
    let alert = s.quote.comment.failed().then(|| {
        row().key("quote-comment-failed").role("alert").w_full().px(12.0).pb(12.0).child(text(opencode_i18n::t!("session.quote.sendFailed")).font(ALERT).color(p.text).wrap())
    });
    // `MenuScroll` (`quote-comment.tsx:147`).
    let body = session_view::scroller::scroller("quote-comment-scroll").bar(session_view::scroller::Bar::MenuScroll).build::<App>(p, col().w_full().max_h(available), col().w_full().child(shell_col).children(alert));
    Some(
        col()
            .key("quote-comment")
            .absolute(left, top)
            .w(width)
            .radius(8.0)
            .fill(p.layer1)
            .elevate(p.floating)
            .clip_soft(0.0)
            // Unmeasured, it lays out unseen, so the field can take focus as it mounts.
            .opacity(if measured.is_some() { 1.0 } else { 0.0 })
            .opacity_motion(rune::Anim::INSTANT)
            .transition(Transition::none())
            .motion(Spring::NONE)
            .preserve_focus_on_press(true)
            .keeps_text_selection()
            .on_press(|_: &mut App| {})
            .child(body),
    )
}

/// `ComposerQuoteAnchor` and `quotePosition`: over a short or backward quote, else under it,
/// whichever fits in the timeline, centred on its visible lines; pinned to the timeline's
/// bottom edge when its text has scrolled out of the list.
fn source_position(a: &Anchor, cx: &mut Cx<App>, shell: impl Fn([f32; 4]) -> [f32; 4], width: f32, height: f32) -> Option<(f32, f32)> {
    let view = shell(cx.window_layout(&format!("scroll-{}", a.session))?);
    let (b_top, b_bottom, b_left, b_right) = (view[1], view[1] + view[3], view[0], view[0] + view[2]);
    let pinned = b_bottom - height - GAP;
    if cx.hover_none() { return Some((pinned, (b_left + b_right - width) * 0.5)); }
    let centred = |l: f32, r: f32| ((l + r - width) / 2.0).max(b_left + GAP).min((b_left + GAP).max(b_right - width - GAP));
    let Some(origin) = cx.window_layout(&a.path).map(&shell) else { return Some((pinned, centred(b_left, b_right))) };
    let rects: Vec<[f32; 4]> = a.rects.iter().map(|r| [origin[0] + r[0], origin[1] + r[1], r[2], r[3]]).collect();
    let (first, last) = (rects.first()?, rects.last()?);
    let (top, bottom) = (first[1], last[1] + last[3]);
    let visible: Vec<&[f32; 4]> = rects.iter().filter(|r| r[1] + r[3] > b_top.max(top) && r[1] < b_bottom.min(bottom)).collect();
    let shown: Vec<&[f32; 4]> = if visible.is_empty() { rects.iter().collect() } else { visible };
    let left = shown.iter().map(|r| r[0]).fold(f32::INFINITY, f32::min);
    let right = shown.iter().map(|r| r[0] + r[2]).fold(f32::NEG_INFINITY, f32::max);
    let above = top - height - GAP;
    let below = bottom + GAP;
    let fits_above = above >= b_top + GAP && top <= b_bottom;
    let fits_below = below + height <= b_bottom - GAP && bottom >= b_top;
    let sides = if a.short || a.backward { [(fits_above, above), (fits_below, below)] } else { [(fits_below, below), (fits_above, above)] };
    let y = sides.iter().find(|(fits, _)| *fits).map_or(pinned, |(_, y)| *y);
    Some((y, centred(left, right)))
}

/// The compact action (`line-comment-v2-submit`, a small `IconButton`): a tick in `ghost-muted`
/// over an empty draft or `neutral` over a written one; with Mod held, the send's arrow in the
/// `submit` look; a loader while the send settles. Its tooltip names the action and its keys.
fn action(s: &App, p: &Pal, empty: bool, alternate: bool, sending: bool, key: u64) -> El {
    let available = send_available(s);
    let primary = if !saved(s, key).trim().is_empty() {
        opencode_i18n::t!("common.save")
    } else if !empty {
        opencode_i18n::t!("session.quote.addComment")
    } else {
        opencode_i18n::t!("session.quote.done")
    };
    let alt_keys = [opencode_i18n::t!("common.key.ctrl").to_owned(), "⏎".to_owned()];
    let label = if alternate { delivery(s) } else { primary };
    let glyph = if sending {
        rune_kit::Loader::new().size(16.0).build(crate::palette::kit(p))
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
            .elevate(crate::status_toast::neutral_lift(p))
            .hover(p.hover)
            .press(p.pressed)
    };
    let disabled = sending || (alternate && !available);
    // The action showing and its keys; while the tick shows, the send on a row of its own under
    // it (`line-comment.tsx`, a `flex-col gap-1.5` of rows).
    let tip = if alternate {
        face.help(label).help_keys(alt_keys)
    } else {
        face.help(label).help_keys(["⏎".to_owned()]).help_row(delivery(s)).help_keys(alt_keys)
    };
    let b = tip.label(label).help_above().motion(Spring::NONE).child(glyph);
    if disabled {
        return b.opacity(if sending { 1.0 } else { 0.5 }).focusable(false);
    }
    b.on_press_tx(|s: &mut App, tx: &mut Tx| {
        tx.instant();
        let modified = {
            let m = tx.mods();
            (m.ctrl || m.cmd) && !m.alt && !m.shift
        };
        let config = config(s);
        field(s, FieldEvent::Clicked { modified, config }, tx);
    })
}

/// `line-comment-v2-mention-list`: up to ten paths under the field, 8 below it, 4 in, rows 4
/// apart on `bg-layer-02` in a 0.5 `border-muted` hairline; the lit row on `bg-layer-03`. A
/// row is the file's icon, then its folder (muted, cut at its end) and name.
fn mention_list(p: &Pal, mention: &Mention) -> Option<El> {
    mention_rows(p, mention, std::rc::Rc::new(field_event))
}

fn field_event(s: &mut App, event: FieldEvent, tx: &mut Tx) {
    field(s, event, tx);
}

/// [`mention_list`] whose rows step the field `act` reaches.
pub(crate) fn mention_rows(p: &Pal, mention: &Mention, act: std::rc::Rc<dyn Fn(&mut App, FieldEvent, &mut Tx)>) -> Option<El> {
    let items = mention.items();
    if items.is_empty() {
        return None;
    }
    let active = mention.active();
    let rows = items.iter().enumerate().map(|(i, path)| {
        let (dir, name) = match path.strip_suffix('/') {
            Some(_) => (path.as_str(), ""),
            None => path.rfind(['/', '\\']).map_or(("", path.as_str()), |at| (&path[..=at], &path[at + 1..])),
        };
        let (hover, pick) = (act.clone(), act.clone());
        row()
            .key(format!("mention-{i}"))
            .role("option")
            .label(path.clone())
            .w_full()
            .gap(8.0)
            .px(8.0)
            .py(6.0)
            .radius(4.0)
            .center_y()
            .fill(if active == Some(path.as_str()) { p.layer3 } else { Color::TRANSPARENT })
            .on_pointer_move_local(move |s: &mut App, _, tx: &mut Tx| {
                tx.instant();
                hover(s, FieldEvent::Hovered(i), tx);
            })
            .on_press_tx(move |s: &mut App, tx: &mut Tx| {
                tx.instant();
                pick(s, FieldEvent::Picked(i), tx);
            })
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
    });
    Some(
        // `line-comment-v2-mention-list`: a plain `overflow: auto` box's bar (`line-comment.css:294-296`).
        session_view::scroller::scroller("line-comment-mentions").build::<App>(
            p,
            col().w_full().mt(8.0).padding(4.0).max_h(180.0).radius(6.0).fill(p.layer2).box_shadow(Pal::ring_in(p.border_muted)),
            col().w_full().gap(4.0).children(rows),
        ),
    )
}
