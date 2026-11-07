use opencode_kit::model::{self, InboxItem, Message, Part, Transcript};
use rune::{Color, El, TextStyle, Tx, col, el, row, text};

use crate::TimelineHost;
use crate::fold::Fold;
use crate::palette::Pal;
use crate::style::{BUBBLE, CONTENT_W, INSTANT, interrupted_label};
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
            !x.trim().is_empty() && !y.trim().is_empty() && model::user_files(id).is_empty()
        }
        _ => false,
    }
}

/// Whether pending steer `b` joins the run of the prompt (or steer) with text `a` above it.
pub fn prompts_join(a: &str, b: &InboxItem) -> bool {
    !a.trim().is_empty() && !b.text.trim().is_empty() && b.files.is_empty() && model::user_files(&b.id).is_empty()
}

/// Read-only hover action for a user prompt: Copy message (`copied::glyph`).
pub fn default_user_actions<S: TimelineHost>(s: &S, p: &Pal, id: &str, body: &str) -> (El, f32) {
    let mut actions = row().key("user-actions").gap(6.0).center_y().group_opacity(0.0, 1.0);
    if !body.trim().is_empty() {
        let copy_key = format!("copy-u-{id}");
        let copied = body.to_owned();
        let glyph = crate::copied::glyph(s, &copy_key, 14.0, p.icon_muted);
        actions = actions.child(
            row()
                .key(format!("copy-{id}"))
                .role("button")
                .label("Copy message")
                .help("Copy message")
                .size(20.0, 20.0)
                .center()
                .radius(4.0)
                .hover(p.hover)
                .press(p.pressed)
                .hover_motion(INSTANT, INSTANT)
                .on_press_tx(move |s: &mut S, tx: &mut Tx| {
                    tx.copy(copied.clone());
                    crate::copied::mark_copied(s, copy_key.clone(), tx);
                })
                .child(glyph),
        );
    }
    (actions, -24.0)
}

/// A prompt's bubble. A run of prompts sits 4 apart with the trailing corners tightened where
/// they meet (`above`, `below`); the run's last prompt has a copy line under it, and the others
/// show copy beside the bubble, all only while hovered.
///
/// `pending` (the session) draws an undelivered steer: muted, with Move to queue and Delete in
/// copy's place, and the run's last one marked Pending under its bubble at all times.
///
/// `quotes` are the prompt's quote cards: inside the bubble above its words, or in its place
/// when it has none.
#[allow(clippy::too_many_arguments)]
pub fn user_bubble<S: TimelineHost>(s: &S, p: &Pal, id: &str, body: &str, place: Place, glow: f32, pending: Option<&str>, quotes: Option<El>) -> El {
    let Place { tight, above, below, fold } = place;
    let (actions, pin_x) = match pending {
        Some(session) => s.pending_user_actions(p, session, id, below),
        None => s.delivered_user_actions(p, id, body),
    };
    let (top, end) = (if above { 4.0 } else { 10.0 }, if below { 4.0 } else { 10.0 });
    let mut bubble = col()
        .key("bubble")
        .max_w(BUBBLE_MAX)
        .shrink()
        .px(12.0)
        .py(8.0)
        .corners([10.0, top, end, 10.0])
        .fill(if pending.is_some() { p.layer1 } else { p.layer2 });
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
    let ink = if pending.is_some() { p.muted } else { p.text };
    // Mentions read as badges; a badge copies as its text.
    let (shown, badges, links) = crate::user_parts::bubble_badges(s, p, id, body);
    let highlights = if pending.is_none() { s.find_text_highlights(p, &format!("u-{id}")) } else { Vec::new() };
    let mut words = text(shown.clone()).key(format!("text-{id}")).font(BUBBLE).color(ink).wrap_anywhere().pills(badges).text_highlights(highlights);
    if !links.is_empty() {
        for link in links {
            words = words.link(link);
        }
        words = words.on_link(|s: &mut S, link: &str, tx: &mut Tx| {
            if let Some(sid) = crate::user_parts::session_of_link(link) {
                s.open_session(sid, tx);
            }
        });
    }
    let words = crate::fold::view(s, p, id, &shown, readable(p, words), fold, quotes.is_some() && !body.trim().is_empty());
    let mut face = match quotes {
        Some(cards) if body.trim().is_empty() => cards.key("bubble"),
        Some(cards) => bubble.w(342.0).pt(12.0).child(cards).children(words),
        None => bubble.children(words),
    };
    let status = s.user_status(p, id);
    let actions = if status.is_some() {
        status
    } else if below {
        face = face.child(actions.pin(Some(pin_x), None, None, Some(8.5)));
        None
    } else {
        Some(actions)
    };
    let c = col()
        .w_full()
        .pt(if above { 4.0 } else if tight { 8.0 } else { 16.0 })
        .hover_group()
        .hover_motion(INSTANT, INSTANT)
        .child(row().w_full().justify_end().child(row().w_frac(0.82).justify_end().child(face)));
    c.children(actions.map(|actions| row().key("actions").w_full().h(20.0).mt(4.0).justify_end().child(actions)))
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
    if let Some(m) = model.filter(|m| !m.id.is_empty() || !m.provider.is_empty()) {
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
        items.push((if total < 60 { format!("{total}s") } else { format!("{}m {}s", total / 60, total % 60) }, false));
    }
    items
}

pub fn turn_actions<S: TimelineHost>(s: &S, p: &Pal, busy: bool, session: &str, message: &Message) -> El {
    // These share a footprint and swap at once: a crossfade would lay the copy icon's outline
    // over the W in Working.
    let has_text = matches!(message, Message::Assistant(a) if a.parts.iter().any(|part| matches!(part, Part::Text { text } if !text.trim().is_empty())));
    let copy_key = format!("copy-{}", message.id());
    use crate::working::{COPY, COPY_AT, COPY_GLYPH};
    let mut button = row().key("copy").role("button").label("Copy response")
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
    let meta = (!busy && has_text && !interrupted)
        .then(|| s.with_transcript(session, |t| t.messages.iter().position(|m| m.id() == message.id()).map(|i| reply_meta(s, t, i)).unwrap_or_default()))
        .flatten()
        .filter(|items| !items.is_empty())
        .map(|items| {
            let mut r = row().key("meta").absolute(COPY_AT.0 + COPY + 4.0, COPY_AT.1).h(COPY).gap(4.0).center_y().a11y_hidden();
            for (k, (label, variant)) in items.into_iter().enumerate() {
                if k > 0 && !variant {
                    r = r.child(text("\u{00b7}").key(format!("sep{k}")).font(META).color(p.faint));
                }
                r = r.child(text(label).key(format!("item{k}")).font(META).color(p.faint));
            }
            r.group_opacity(0.0, 1.0)
        });
    // Copy and the meta show only while the reply or its footer is hovered, at once, as
    // Desktop's do; Kit's Working-to-Copy swap still plays under the pointer.
    el().w_full().h(crate::working::LANE).token_motion(crate::knobs::actions_tail(s.knobs()))
        .hover_group_shared(REPLY_HOVER)
        .hover_motion(INSTANT, INSTANT)
        .child(crate::working::row(s, p, session, busy).key("working").absolute(0.0, crate::working::LANE_PAD).opacity(if busy { 1.0 } else { 0.0 }).opacity_motion(INSTANT))
        .child(button.absolute(COPY_AT.0, COPY_AT.1).group_opacity(0.0, 1.0))
        .children(meta)
        .children(interrupted.then(|| interrupted_label(p, has_text)))
        .children(s.turn_tail_extra(p, session, busy))
}

// ---------------------------------------------------------------------------------------------
// Markdown

pub fn markdown_view<S: TimelineHost>(p: &Pal, src: &str, fade: f32, streaming: bool) -> El {
    markdown_tinted::<S>(p, src, fade, streaming, p.text, CONTENT_W)
}

pub fn markdown_view_find<S: TimelineHost>(s: &S, p: &Pal, row_key: &str, src: &str, fade: f32, streaming: bool) -> El {
    // A reply with a ```ui outline in it draws it as parts (`genui`); any other takes this path.
    if let Some(el) = crate::genui::reply(s, p, src, fade, streaming, s.find_row_hits(row_key)) {
        return el;
    }
    markdown_with_highlights::<S>(p, src, fade, streaming, p.text, crate::genui::width(s), s.find_markdown_highlights(p, row_key), crate::copied::current(s), Some(crate::images::md_media(s)))
}

/// Convenience wrapper with explicit body colour and diagram max width.
pub fn markdown_colored<S: TimelineHost>(p: &Pal, src: &str, fade: f32, streaming: bool, color: Color, width: f32) -> El {
    markdown_tinted::<S>(p, src, fade, streaming, color, width)
}
