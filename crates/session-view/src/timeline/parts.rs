use opencode_kit::model::{self, InboxItem, Message, Part, Transcript};
use rune::{Color, El, TextStyle, Tx, col, el, row, text};

use crate::TimelineHost;
use crate::fold::Fold;
use crate::palette::Pal;
use crate::style::{BUBBLE, INSTANT, interrupted_label};
pub use crate::style::{BUBBLE_MAX, markdown_style, markdown_tinted, markdown_with_highlights, readable};

/// Where a prompt's bubble sits in its run (see `user_bubble`), and where it lays out.
#[derive(Clone, Copy)]
pub struct Place {
    pub tight: bool,
    pub above: bool,
    pub below: bool,
    pub fold: Fold,
}

/// Whether prompt `b` joins the run of prompt `a` just above it: both have words and `b` brings
/// no files (Desktop's `user-runs.ts`).
pub fn joins(a: &Message, b: &Message) -> bool {
    match (a, b) {
        (Message::User { text: x, .. }, Message::User { id, text: y }) => {
            !x.trim().is_empty() && !y.trim().is_empty() && !super::attachments::any(id)
        }
        _ => false,
    }
}

/// Whether pending steer `b` joins the run of the prompt (or steer) with text `a` above it.
pub fn prompts_join(a: &str, b: &InboxItem) -> bool {
    !a.trim().is_empty() && !b.text.trim().is_empty() && b.files.is_empty() && !super::attachments::any(&b.id)
}

/// A read-only host's corner button for a user prompt: Copy message (`copied::glyph`), as
/// [`user_menu_button`] lays it out.
pub fn default_user_menu<S: TimelineHost>(s: &S, p: &Pal, id: &str, body: &str) -> Option<El> {
    if body.trim().is_empty() {
        return None;
    }
    let copy_key = format!("copy-u-{id}");
    let copied = body.to_owned();
    let glyph = crate::copied::glyph(s, &copy_key, 14.0, p.icon_muted);
    Some(
        user_menu_button(p, format!("copy-{id}"), opencode_i18n::t!("ui.message.copyMessage"), false)
            .on_press_tx(move |s: &mut S, tx: &mut Tx| {
                tx.copy(copied.clone());
                crate::copied::mark_copied(s, copy_key.clone(), tx);
            })
            .child(glyph),
    )
}

/// The bubble's corner button (`Menu.Trigger` as a small ghost-muted `IconButton`,
/// `icon-button.css`): 20 pt with 4 pt corners, the overlay's hover and press fills, and the
/// pressed fill while its menu is `open`, which no hover or press adds to, as
/// `:not([data-expanded])` keeps Desktop's hover off it. The host adds the glyph and the press.
pub fn user_menu_button(p: &Pal, key: String, label: &str, open: bool) -> El {
    let b = row().key(key).role("button").label(label.to_owned()).size(20.0, 20.0).center().radius(4.0);
    if open { b.fill(p.pressed) } else { b.hover(p.hover).press(p.pressed).hover_motion(INSTANT, INSTANT) }
}

/// The meta line under a prompt (`user-message-meta-wrap`): 12/18 at 440, tracking 0.05, faint
/// (`message-part.css:329-338`).
const PROMPT_META: TextStyle = TextStyle::new(12.0, 440.0).lh(18.0).tracking(0.05);

/// The chevron's inset from the bubble's top and trailing edges: 8 plus half of what a 21 pt
/// line has over the 20 pt button (`message-part.css:507-511`).
const MENU_INSET: f32 = 8.0 + (21.0 - 20.0) / 2.0;

/// The model a prompt ran with, for its meta line: the last its turn's replies named, else the
/// last one named before it (`projection.ts`'s `indexUserContext`), and a variant other than
/// default after it.
pub fn prompt_meta<S: TimelineHost>(s: &S, t: &Transcript, message: Option<usize>) -> Vec<(String, bool)> {
    let model_of = |m: &Message| match m {
        Message::Assistant(a) => a.model.clone().filter(|m| !m.id.is_empty() || !m.provider.is_empty()),
        _ => None,
    };
    let turn = message.and_then(|i| {
        let end = t.messages[i + 1..].iter().position(|m| matches!(m, Message::User { .. })).map_or(t.messages.len(), |n| i + 1 + n);
        t.messages[i + 1..end].iter().rev().find_map(model_of)
    });
    let before = t.messages[..message.unwrap_or(t.messages.len())].iter().rev().find_map(model_of);
    // A narrow window shows the agent and model in the composer instead (`message-content.tsx:339`).
    let Some(m) = turn.or(before).filter(|_| !crate::style::narrow()) else { return Vec::new() };
    let mut items = vec![(s.model_name(&m), false)];
    if !m.variant.is_empty() && m.variant != "default" {
        let mut v = m.variant.chars();
        items.push((v.next().map(|c| c.to_uppercase().chain(v).collect()).unwrap_or_default(), true));
    }
    items
}

/// A prompt's bubble, as Desktop's `user-message` (`message-part.css:10-567`): 14/21 text on
/// `layer-02`, 8 by 12 inside, 10 pt corners, at most `min(82%, 64ch)` wide. A run of prompts
/// sits 4 apart with the trailing corners tightened where they meet (`above`, `below`).
///
/// Its actions open from a chevron in its top trailing corner, shown while the bubble is
/// hovered (or its menu is open) over a 16 pt fade of the bubble's colour; under it a 20 pt
/// line 8 below names the model (`meta`) while the bubble is hovered. A run's earlier prompts
/// fold that line away.
///
/// `pending` (the session) draws an undelivered steer: on `layer-01`, with Pending in view at
/// all times under it. Pending sits in the meta room, outside the bubble, so it never rides the
/// bubble's flight in from the composer.
///
/// `quotes` are the prompt's quote cards: inside the bubble above its words, or in its place
/// when it has none.
#[allow(clippy::too_many_arguments)]
pub fn user_bubble<S: TimelineHost>(s: &S, p: &Pal, session: &str, id: &str, body: &str, place: Place, glow: f32, pending: bool, quotes: Option<El>, meta: &[(String, bool)]) -> El {
    let Place { tight, above, below, fold } = place;
    let menu = s.user_menu(p, session, id, body, pending);
    let open = s.user_menu_open_on(id);
    let (top, end) = (if above { 4.0 } else { 10.0 }, if below { 4.0 } else { 10.0 });
    let fill = if pending { p.layer1 } else { p.layer2 };
    let mut bubble = col()
        .key("bubble")
        .max_w(BUBBLE_MAX)
        .shrink()
        .px(12.0)
        .py(8.0)
        .corners([10.0, top, end, 10.0])
        .fill(fill)
        .hover_group()
        .hover_motion(INSTANT, INSTANT)
        // The prompt's own direction, its trailing corners and menu with it
        // (`message-content.tsx:683`'s `dir="auto"`).
        .dir(rune::Direction::Auto);
    if glow > 0.0 {
        // A quiet surface lift under the text, with no glow rim.
        let lift = p.lift;
        bubble = bubble.child(
            el().key("glow").absolute(0.0, 0.0).w_full().h_full().corners([10.0, top, end, 10.0])
                .fill(lift.alpha(0.1))
                .opacity(glow)
                .opacity_motion(INSTANT),
        );
    }
    let ink = if pending { p.muted } else { p.text };
    // Mentions read as badges; a badge copies as its text.
    let (shown, badges, links) = crate::user_parts::bubble_badges(s, p, id, body);
    let highlights = if !pending { s.find_text_highlights(p, &format!("u-{id}")) } else { Vec::new() };
    let mut words = text(shown.clone()).key(format!("text-{id}")).font(BUBBLE).color(ink).wrap_anywhere().pills(badges).text_highlights(highlights);
    if !links.is_empty() {
        for link in links {
            words = words.link(link);
        }
        words = words
            .on_link(|s: &mut S, link: &str, tx: &mut Tx| {
                if let Some(sid) = crate::user_parts::session_of_link(link) {
                    s.open_session(sid, tx);
                }
            })
            .on_link_secondary(|s: &mut S, link: &str, press, tx: &mut Tx| {
                if let Some(sid) = crate::user_parts::session_of_link(link) {
                    s.session_chip_menu(sid, press, tx);
                }
            });
    }
    let words = crate::fold::view(s, p, id, &shown, readable(p, words), fold, quotes.is_some() && !body.trim().is_empty());
    let mut face = match quotes {
        Some(cards) if body.trim().is_empty() => cards.key("bubble"),
        Some(cards) => bubble.w(342.0).pt(12.0).child(cards).children(words),
        None => bubble.children(words),
    };
    if let Some(button) = menu {
        // The bubble's colour fades in over 16 pt behind the chevron, so words under the corner
        // never compete with it (`message-part.css:499-525`).
        let fade = el().key("menu-fade").w(16.0).h(20.0).linear_gradient(std::f32::consts::FRAC_PI_2, [(0.0, fill.alpha(0.0)), (1.0, fill)]);
        let corner = row().key("menu").pin(None, Some(MENU_INSET), Some(MENU_INSET), None).h(20.0).corners([4.0; 4])
            .child(fade)
            .child(el().key("menu-under").size(20.0, 20.0).fill(fill).child(button));
        face = face.child(if open { corner.opacity(1.0).opacity_motion(INSTANT) } else { corner.group_opacity(0.0, 1.0) });
    }
    // A right-click anywhere on the bubble opens the chevron's menu, at the pointer.
    let mut face = crate::images::menu::on_message::<S>(face, session, id);
    let status = s.user_status(p, id);
    // The meta line, right under the bubble: the model, then Pending on a steer, which stays
    // in view while the model shows only under the pointer (`message-part.css:271-458`).
    if status.is_none() && !below && (!meta.is_empty() || pending) {
        let mut line = row().key("meta").pin(None, None, Some(0.0), Some(-28.0)).h(20.0).gap(4.0).center_y().a11y_hidden();
        let mut model = row().key("meta-model").gap(4.0).center_y();
        for (k, (label, variant)) in meta.iter().enumerate() {
            if k > 0 && !variant {
                model = model.child(text("\u{00b7}").key(format!("sep{k}")).font(PROMPT_META).color(p.faint));
            }
            model = model.child(text(label.clone()).key(format!("item{k}")).font(PROMPT_META).color(p.faint));
        }
        if pending && !meta.is_empty() {
            model = model.child(text("\u{00b7}").key("pending-sep").font(PROMPT_META).color(p.faint));
        }
        line = line.child(model);
        if pending {
            // Pending itself draws in the meta room below, outside the bubble, so it stays put
            // while the bubble flies in from the composer; an unseen copy here keeps the model
            // left of it.
            line = line.child(pending_label(p).key("pending-room").opacity(0.0));
        }
        // As Desktop's copy wrapper, the whole line shows only under the pointer, sent or
        // pending, so it comes and goes unseen as the model it names lands or changes, and a
        // steer's delivery never crossfades it into view.
        face = face.child(if open { line.opacity(1.0).opacity_motion(INSTANT) } else { line.group_opacity(0.0, 1.0) });
    }
    let c = col()
        .w_full()
        .pt(if above { 4.0 } else if tight { 8.0 } else { 16.0 })
        .child(row().w_full().justify_end().child(row().w_frac(0.82).justify_end().child(face)));
    match status {
        Some(status) => c.child(row().key("actions").w_full().h(20.0).mt(8.0).justify_end().child(status)),
        // The meta line's room (`user-message-copy-wrapper`: 8 above, 20 tall), folded away in
        // a run's earlier prompts.
        None => {
            let mut room = el().key("meta-room").w_full().h(if below { 0.0 } else { 28.0 });
            if pending && !below {
                // A label, so it shows and goes at once, as Desktop's delivery swaps it
                // (message-part.css:401-410).
                let label = pending_label(p).key("pending").transition(rune::Transition::none());
                room = room.child(row().key("pending-line").w_full().h(20.0).mt(8.0).justify_end().center_y().a11y_hidden().child(label));
            }
            c.child(room)
        }
    }
}

/// A steer's Pending, under its bubble's trailing edge.
fn pending_label(p: &Pal) -> El {
    text(opencode_i18n::t!("ui.message.pending")).font(PROMPT_META).color(p.faint)
}

/// The hover group a reply's footer shares with the reply text above it (separate virtual
/// rows), as Desktop shows the footer while its text part is hovered.
pub const REPLY_HOVER: &str = "reply-hover";

/// The footer's "Model · 1s" (`text-part-meta`): 12/18 at weight 440 in `text-faint`.
const META: TextStyle = TextStyle::new(12.0, 440.0).lh(18.0).tracking(0.05);

/// Whether part `part` of message `message` is the reply text the turn's footer copies: the
/// last text of the last reply with any (`copyContentID`).
pub fn footer_text(t: &Transcript, message: usize, part: usize) -> bool {
    let text = |p: &Part| matches!(p, Part::Text { text } if !text.trim().is_empty());
    let last = t.messages.iter().rposition(|m| matches!(m, Message::Assistant(a) if a.parts.iter().any(text)));
    last == Some(message)
        && matches!(&t.messages[message], Message::Assistant(a) if a.parts.iter().rposition(text) == Some(part))
}

/// The footer's items after Copy (`text-part-meta`): the model's name (and a variant other
/// than default, with no separator), then how long the turn took, from its prompt to its last
/// finished step (`SessionTurn.duration`), as "1s" or "1m 5s".
fn reply_meta<S: TimelineHost>(s: &S, t: &Transcript, message: usize) -> Vec<(String, bool)> {
    let mut items = Vec::new();
    let model = t.messages[..=message].iter().rev().find_map(|m| match m {
        Message::Assistant(a) => a.model.clone(),
        _ => None,
    });
    // A narrow window leaves out the model (`message-content.tsx:1172`).
    if let Some(m) = model.filter(|m| (!m.id.is_empty() || !m.provider.is_empty()) && !crate::style::narrow()) {
        items.push((s.model_name(&m), false));
        if !m.variant.is_empty() && m.variant != "default" {
            let mut v = m.variant.chars();
            items.push((v.next().map(|c| c.to_uppercase().chain(v).collect()).unwrap_or_default(), true));
        }
    }
    let prompt = t.messages[..message].iter().rposition(|m| matches!(m, Message::User { .. }));
    let started = prompt.and_then(|i| match &t.messages[i] {
        Message::User { id, .. } => model::user_created(id),
        _ => None,
    });
    let first_step = t.messages[prompt.map_or(0, |i| i + 1)..=message].iter().find_map(|m| match m {
        Message::Assistant(a) if a.created > 0 => Some(a.created),
        _ => None,
    });
    let done = t.messages[prompt.map_or(0, |i| i + 1)..].iter().rev().find_map(|m| match m {
        Message::Assistant(a) => a.completed,
        _ => None,
    });
    if let (Some(done), Some(from)) = (done, started.or(first_step)) {
        let total = ((done - from).max(0) as f64 / 1000.0).round() as i64;
        items.push((
            if total < 60 {
                opencode_i18n::t!("ui.message.duration.seconds", count = opencode_i18n::integer(total))
            } else {
                opencode_i18n::t!("ui.message.duration.minutesSeconds", minutes = opencode_i18n::integer(total / 60), seconds = opencode_i18n::integer(total % 60))
            },
            false,
        ));
    }
    items
}

/// The turn's tail: Working while it runs (unless it waits on a question or permission, `asking`,
/// as Desktop's `showWorking`), then the reply's Copy footer, with room of its own over the
/// timeline's tail when it ends the timeline (`last`).
pub fn turn_actions<S: TimelineHost>(s: &S, p: &Pal, busy: bool, asking: bool, last: bool, session: &str, message: &Message) -> El {
    // These share a footprint and swap at once: a crossfade would lay the copy icon's outline
    // over the W in Working.
    let has_text = matches!(message, Message::Assistant(a) if a.parts.iter().any(|part| matches!(part, Part::Text { text } if !text.trim().is_empty())));
    let copy_key = format!("copy-{}", message.id());
    use crate::working::{COPY, COPY_AT, COPY_GLYPH};
    let mut button = row().key("copy").role("button").label(opencode_i18n::t!("ui.message.copyResponse"))
        .size(COPY, COPY).center().radius(4.0).hover(p.hover).press(p.pressed)
        .opacity(if busy || !has_text { 0.0 } else { 1.0 })
        .opacity_motion(INSTANT)
        .child(crate::copied::glyph(s, &copy_key, COPY_GLYPH, p.muted));
    if !busy && has_text {
        let session = session.to_owned();
        let id = message.id().to_owned();
        button = button.on_press_tx(move |s: &mut S, tx: &mut Tx| {
            let copied = s
                .with_transcript(&session, |t| {
                    t.messages.iter().find(|m| m.id() == id).and_then(|m| match m {
                        Message::Assistant(a) => Some(
                            a.parts
                                .iter()
                                .filter_map(|part| match part {
                                    Part::Text { text } => Some(text.as_str()),
                                    _ => None,
                                })
                                .collect::<Vec<_>>()
                                .join("\n\n"),
                        ),
                        _ => None,
                    })
                })
                .flatten();
            if let Some(body) = copied {
                tx.copy(body);
                crate::copied::mark_copied(s, copy_key.clone(), tx);
            }
        });
    }
    // One reserved tail: completion crossfades in place rather than first removing Working's
    // height and then adding actions. The scroll endpoint cannot reverse during this handoff.
    let interrupted = !busy && (matches!(message, Message::Assistant(a) if a.failure.as_ref().is_some_and(|f| super::interrupted(&f.kind))) || s.is_session_stopped(session));
    // "Model · 1s" after Copy, 4 apart, the separator 4 from each item; a variant follows its
    // model with no separator.
    let items = (!busy && has_text)
        .then(|| s.with_transcript(session, |t| t.messages.iter().position(|m| m.id() == message.id()).map(|i| reply_meta(s, t, i)).unwrap_or_default()))
        .flatten()
        .unwrap_or_default();
    let meta = |items: Vec<(String, bool)>| {
        let mut r = row().key("meta").h(COPY).gap(4.0).center_y().a11y_hidden();
        for (k, (label, variant)) in items.into_iter().enumerate() {
            if k > 0 && !variant {
                r = r.child(text("\u{00b7}").key(format!("sep{k}")).font(META).color(p.faint));
            }
            r = r.child(text(label).key(format!("item{k}")).font(META).color(p.faint));
        }
        r.group_opacity(0.0, 1.0)
    };
    // The Sources experiment's chip leads a finished turn's footer that searched the web: it
    // stays in view, 16 under the reply, and Copy follows it 8 on (`text-part-footer`).
    let sources = (!busy && has_text && s.knobs().get(&crate::knobs::SOURCES_ENABLED) > 0.5)
        .then(|| s.with_transcript(session, |t| t.messages.iter().position(|m| m.id() == message.id()).and_then(|i| crate::sources_chip::of(t, i))).flatten())
        .flatten();
    // A finished reply's footer (`text-part-copy-wrapper`: 8 under it, 20 tall) sits over the
    // Working lane, which Desktop's timeline tail keeps empty under it (`virtualizer.tsx:152`).
    // Input waiting under the turn lands at once, so the footer doesn't push it down meanwhile.
    let footer = if busy || !last || !(has_text || interrupted) {
        0.0
    } else if sources.is_some() && !interrupted {
        16.0 + COPY
    } else {
        COPY_AT.1 + COPY
    };
    let working = busy && !asking;
    let lane = el().w_full().h(footer + crate::working::LANE).token_motion(crate::knobs::actions_tail(s.knobs()))
        .hover_group_shared(REPLY_HOVER)
        .hover_motion(INSTANT, INSTANT)
        .child(crate::working::row(s, p, session, working).key("working").absolute(0.0, crate::working::LANE_PAD).opacity(if working { 1.0 } else { 0.0 }).opacity_motion(INSTANT));
    if interrupted {
        // A stopped turn's footer runs to the reply's end edge, "Model · 3s · Interrupted"
        // after Copy (`text-part-copy-wrapper[data-interrupted]`, `message-part.css:652-655`).
        // Interrupted stays in view and rises out of Working's place as the run settles (Kit's);
        // Copy and the rest show under the pointer, as Desktop's footer does.
        let mut tail = row().key("tail").pin(None, Some(COPY_AT.1), Some(0.0), None).h(COPY).gap(4.0).center_y();
        if has_text {
            tail = tail.child(button.group_opacity(0.0, 1.0));
            if !items.is_empty() {
                tail = tail.child(meta(items)).child(text("\u{00b7}").key("sep-interrupted").font(META).color(p.faint).group_opacity(0.0, 1.0));
            }
        }
        return lane.child(tail.child(interrupted_label(p))).children(s.turn_tail_extra(p, session, busy));
    }
    let (dx, y) = match &sources {
        Some(c) => (chip_width(c) + 8.0, 16.0),
        None => (0.0, COPY_AT.1),
    };
    let lane = match &sources {
        Some(c) => lane.child(crate::sources_chip::chip(s, p, c).absolute(0.0, 16.0)),
        None => lane,
    };
    // Copy and the meta show only while the reply or its footer is hovered, at once, as
    // Desktop's do; Kit's Working-to-Copy swap still plays under the pointer.
    lane.child(button.absolute(COPY_AT.0 + dx, y).group_opacity(0.0, 1.0))
        .children((!items.is_empty()).then(|| meta(items).absolute(COPY_AT.0 + dx + COPY + 4.0, y)))
        .children(s.turn_tail_extra(p, session, busy))
}

/// The Sources chip's width: 2 in, its tiles (or the globe), 6, its words, 8 out.
fn chip_width(c: &crate::sources_chip::Chip) -> f32 {
    let label = match c.count {
        0 => opencode_i18n::t!("ui.sources.none").to_owned(),
        n => opencode_i18n::plural!("ui.sources.count", n),
    };
    let icons = if c.sites.is_empty() { 18.0 } else { 16.0 + 13.0 * (c.sites.len() - 1) as f32 };
    2.0 + icons + 6.0 + rune::text::measure(&label, &[], TextStyle::new(12.0, 440.0).lh(16.0).tracking(0.05), None).0 + 8.0
}

// ---------------------------------------------------------------------------------------------
// Markdown

pub fn markdown_view<S: TimelineHost>(p: &Pal, src: &str, fade: f32, streaming: bool) -> El {
    markdown_tinted::<S>(p, src, fade, streaming, p.text, crate::style::content_w())
}

/// A reply's markdown, `message` being the reply it is: a right-click on it (or on one of its
/// images) opens the reply's menu.
pub fn markdown_view_find<S: TimelineHost>(s: &S, p: &Pal, row_key: &str, src: &str, fade: f32, streaming: bool, message: Option<&str>) -> El {
    // Session ids read as chips, except while Find marks the text as written.
    let hits = s.find_row_hits(row_key);
    let chips = if hits.is_none() { crate::session_chips::chips(s, src) } else { None };
    let tips = if chips.is_some() { crate::session_chips::tips(s, src) } else { Vec::new() };
    let src = chips.as_deref().unwrap_or(src);
    // A reply with a ```ui outline in it draws it as parts (`genui`); any other takes this path.
    let el = crate::genui::reply(s, p, src, fade, streaming, hits, &tips).unwrap_or_else(|| {
        let media = crate::images::markdown::MdMedia { message: message.map(std::rc::Rc::from), ..crate::images::md_media(s) };
        markdown_with_highlights::<S>(p, src, fade, streaming, p.text, crate::genui::width(s), s.find_markdown_highlights(p, row_key), crate::copied::current(s), Some(media), &tips)
    });
    match message {
        Some(id) => crate::images::menu::on_message::<S>(el, "", id),
        None => el,
    }
}

/// Convenience wrapper with explicit body colour and diagram max width.
pub fn markdown_colored<S: TimelineHost>(p: &Pal, src: &str, fade: f32, streaming: bool, color: Color, width: f32) -> El {
    markdown_tinted::<S>(p, src, fade, streaming, color, width)
}
