use opencode_kit::model::{self, InboxItem, Message, Part, Transcript};
use rune::{Color, El, TextStyle, Tx, col, el, row, text};

use crate::TimelineHost;
use crate::focus::FocusExt as _;
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
    /// A reply's work sits right above it: the room it stands under it (`rhythm::L`, less a
    /// footer's room), else 0.
    pub reply: f32,
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
    let b = row().key(key).role("button").label(label.to_owned()).size(20.0, 20.0).center().radius(4.0).cursor(rune::Cursor::Pointer).focus_inset(p);
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
/// `quotes` are the prompt's comment cards: inside the bubble under its words (`message-content.tsx:697-699`,
/// `message-part.css:1931-1967`), or in its place when it has none.
#[allow(clippy::too_many_arguments)]
pub fn user_bubble<S: TimelineHost>(s: &S, p: &Pal, session: &str, id: &str, body: &str, place: Place, glow: f32, pending: bool, quotes: Option<El>, meta: &[(String, bool)]) -> El {
    let Place { tight, above, below, reply, fold } = place;
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
        .hover_group_focus()
        // The bubble stays in the timeline's direction, so its tightened corners always face the
        // timeline's outer wall; its lines read their own way inside it (`dir_by_paragraph`).
        .hover_motion(INSTANT, INSTANT);
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
    let (shown, badges, links, paths) = crate::user_parts::bubble_badges(s, p, id, body);
    let highlights = if !pending { s.find_text_highlights(p, &format!("u-{id}")) } else { Vec::new() };
    // File and folder badges, and (when the prompt contains RTL text) the paths, calls, flags
    // and file names typed around them, each keep their own left-to-right order in the line.
    // Pure-LTR text needs no LRI/PDI isolates around ASCII tokens (`message-part.css:176,192-198`),
    // avoiding unnecessary run splits that perturb kerning and subpixel glyph placement.
    let units: Vec<_> = if has_rtl(&shown) {
        rune::text::ltr_units(&shown)
            .into_iter()
            .filter(|u| !badges.iter().any(|(r, _)| r.start < u.end && u.start < r.end))
            .collect()
    } else {
        Vec::new()
    };
    let mut words = text(shown.clone()).key(format!("text-{id}")).font(BUBBLE).color(ink).wrap_anywhere().pills(badges).text_highlights(highlights);
    for r in paths.into_iter().chain(units) {
        words = words.isolate(r, rune::Direction::Ltr);
    }
    if !links.is_empty() {
        for link in links {
            words = words.link(link);
        }
        words = words
            .on_link(|s: &mut S, link: &str, tx: &mut Tx| {
                // A chip, not an address: a middle click on it does nothing.
                if tx.button() == rune::PointerButton::Middle {
                    return;
                }
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
    // Each line reads in the direction of its own words, inside the bubble's
    // (`message-part.css:176`, `:224`: `unicode-bidi: plaintext`).
    let words = readable(p, words).dir_by_paragraph();
    let words = crate::fold::view(s, p, id, &shown, words, fold, quotes.is_some() && !body.trim().is_empty());
    let mut face = match quotes {
        Some(cards) if body.trim().is_empty() => cards.key("bubble"),
        Some(cards) => bubble.w(342.0).pb(12.0).children(words).child(cards),
        None => bubble.children(words),
    };
    if let Some(button) = menu {
        // The bubble's colour fades in over 16 pt behind the chevron, so words under the corner
        // never compete with it (`message-part.css:499-525`).
        // It stands at the end of the first line, on its right when that line reads left to right
        // and on its left when it reads right to left, so it never covers the prompt's first word.
        let rtl = shown.lines().next().and_then(rune::Direction::of) == Some(rune::Direction::Rtl);
        let (from, to) = if rtl { (fill, fill.alpha(0.0)) } else { (fill.alpha(0.0), fill) };
        let fade = el().key("menu-fade").w(16.0).h(20.0).linear_gradient(std::f32::consts::FRAC_PI_2, [(0.0, from), (1.0, to)]);
        let under = el().key("menu-under").size(20.0, 20.0).fill(fill).child(button);
        let corner = if rtl { row().pin(Some(MENU_INSET), Some(MENU_INSET), None, None).child(under).child(fade) } else { row().pin(None, Some(MENU_INSET), Some(MENU_INSET), None).child(fade).child(under) };
        let corner = corner.key("menu").unmirrored().dir(rune::Direction::Ltr).h(20.0).corners([4.0; 4]);
        face = face.child(if open || crate::style::hover_none() { corner.opacity(1.0).opacity_motion(INSTANT) } else { corner.group_opacity(0.0, 1.0) });
    }
    // A right-click anywhere on the bubble opens the chevron's menu, at the pointer.
    let mut face = crate::images::menu::on_message::<S>(face, session, id);
    let status = s.user_status(p, id);
    // The meta line, right under the bubble: the model, then Pending on a steer, which stays
    // in view while the model shows only under the pointer (`message-part.css:271-458`).
    // The absolute anchor keeps its place even when the first model arrives a commit
    // after delivery. An empty anchor carries only the offset, no label or layout space.
    // A pending label is drawn either way; Smooth timeline also keeps an empty anchor in place
    // for the model that arrives a commit after delivery.
    if status.is_none() && !below && (s.smooth_timeline() || !meta.is_empty() || pending) {
        let mut line = row().key("meta").pin(None, None, Some(0.0), Some(-28.0)).h(20.0).gap(4.0).center_y().a11y_hidden();
        for (k, (label, variant)) in meta.iter().enumerate() {
            if k > 0 && !variant {
                line = line.child(text("\u{00b7}").key(format!("sep{k}")).font(PROMPT_META).color(p.faint));
            }
            line = line.child(text(label.clone()).key(format!("item{k}")).font(PROMPT_META).color(p.faint));
        }
        // Offset the model, not the line's layout. It can return to the trailing edge
        // without landing on the outgoing Pending glyphs under a stationary pointer.
        if pending {
            let width = |t: &str| rune::text::measure(t, &[], PROMPT_META, None).0;
            let room = width(opencode_i18n::t!("ui.message.pending")) + width("·") + 8.0;
            let sign = if opencode_i18n::direction() == opencode_i18n::Direction::Rtl { 1.0 } else { -1.0 };
            line = line.offset(sign * room, 0.0);
            if !meta.is_empty() {
                line = line.child(text("·").key("pending-sep").font(PROMPT_META).color(p.faint).pin(None, None, Some(-4.0 - width("·")), None));
            }
        }
        use rune::AnimExt as _;
        line = line.offset_motion(if s.smooth_timeline() { rune::tok::motion::SNAP.delay(rune::Transition::opacity().springs.exit.duration) } else { INSTANT });
        // Only the hover line moves sideways; the status stays at the bubble's trailing edge.
        face = face.child(if open { line.opacity(1.0).opacity_motion(INSTANT) } else { line.group_opacity(0.0, 1.0) });
    }
    let c = col()
        .w_full()
        .pt(if above { 4.0 } else if tight { 8.0 } else if reply > 0.0 { reply } else { 16.0 })
        .child(row().w_full().justify_end().child(row().w_frac(0.82).justify_end().child(face)));
    match status {
        Some(status) => c.child(row().key("actions").w_full().h(20.0).mt(8.0).justify_end().child(status)),
        // The meta line's room (`user-message-copy-wrapper`: 8 above, 20 tall), folded away in
        // a run's earlier prompts.
        None => {
            let mut room = el().key("meta-room").w_full().h(if below { 0.0 } else { 28.0 });
            if !pending && s.smooth_timeline() { room = room.child_exit(rune::Transition::opacity()); }
            if pending && !below {
                room = room.child(pending_label(p).key("pending").pin(None, Some(9.0), Some(0.0), None).a11y_hidden().transition(rune::Transition::none()));
            }
            c.child(room)
        }
    }
}

/// A steer's Pending, under its bubble's trailing edge.
fn pending_label(p: &Pal) -> El {
    text(opencode_i18n::t!("ui.message.pending")).font(PROMPT_META).color(p.faint)
}

/// Whether `s` contains any right-to-left script character or bidi override/isolate mark
/// (UAX #9 R / AL / RLE / RLO / RLI). Pure-LTR messages already resolve at bidi level 0 under
/// `dir_by_paragraph`, so isolating ASCII `ltr_units` only splits shaping runs unnecessarily.
fn has_rtl(s: &str) -> bool {
    !s.is_ascii()
        && s.chars().any(|c| {
            matches!(
                c as u32,
                0x0590..=0x08FF
                    | 0x200F
                    | 0x202B
                    | 0x202E
                    | 0x2067
                    | 0xFB1D..=0xFDFF
                    | 0xFE70..=0xFEFF
                    | 0x10800..=0x10FFF
                    | 0x1E800..=0x1EFFF
            )
        })
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

/// Whether part `part` of message `message` ends a turn a later prompt follows: the last text of
/// the turn's last reply with any (`session-timeline-row.tsx:310-323`'s `copyContentID`), whose
/// footer sits under it in its own row ([`turn_footer`]). The turn the timeline ends on keeps its
/// footer in the actions row ([`turn_actions`]).
pub fn turn_footer_text(t: &Transcript, message: usize, part: usize) -> bool {
    let text = |p: &Part| matches!(p, Part::Text { text } if !text.trim().is_empty());
    let Some(Message::Assistant(a)) = t.messages.get(message) else { return false };
    if a.parts.iter().rposition(text) != Some(part) {
        return false;
    }
    let Some(next) = t.messages[message + 1..].iter().position(|m| matches!(m, Message::User { .. })) else { return false };
    !t.messages[message + 1..message + 1 + next].iter().any(|m| matches!(m, Message::Assistant(a) if a.parts.iter().any(text)))
}

/// The message index of the latest turn that has a finished reply to copy
/// (`session-timeline-row.tsx`'s `latestCopyTurn`, anomalyco/opencode#64640): computed once per
/// timeline build by walking backwards from the last non-live message, so each row check is `O(1)`
/// even in a 10,000-message session.
pub fn latest_copy_message<S: TimelineHost>(s: &S, t: &Transcript) -> Option<usize> {
    let busy = s.timeline_busy(t) || s.is_session_live(&t.session);
    let live = if busy {
        t.messages.iter().rposition(|m| matches!(m, Message::User { .. })).unwrap_or(0)
    } else {
        t.messages.len()
    };
    let text = |p: &Part| matches!(p, Part::Text { text } if !text.trim().is_empty());
    t.messages.get(..live)?.iter().rposition(|m| matches!(m, Message::Assistant(a) if a.parts.iter().any(text)))
}

/// A finished turn's footer under its reply's last text (`text-part-copy-wrapper`): 8 under the
/// text, 20 tall, Copy (the part's text) then "Model · 1s". When `keep_copy` is true (the latest
/// finished turn with a reply), Copy stays visible while metadata waits for hover or keyboard
/// focus; earlier turns hide the whole footer until hover or focus (anomalyco/opencode#64640,
/// `packages/session-ui/src/components/message-part.css`).
pub fn turn_footer<S: TimelineHost>(s: &S, p: &Pal, t: &Transcript, message: usize, part: usize, keep_copy: bool) -> El {
    use crate::working::{COPY, COPY_AT, COPY_GLYPH};
    let Message::Assistant(a) = &t.messages[message] else { return el() };
    let body = match a.parts.get(part) {
        Some(Part::Text { text }) => text.clone(),
        _ => String::new(),
    };
    let code_tail = if ends_with_code(&body) { 16.0 } else { 0.0 };
    let copy_key = format!("copy-{}-{part}", a.id);
    let key = copy_key.clone();
    let done = crate::copied::is_copied(s, &copy_key);
    let label = if done { opencode_i18n::t!("ui.message.copied") } else { opencode_i18n::t!("ui.message.copyResponse") };
    let button = row().key("copy").role("button").label_static(label).help(label).help_above().help_open(done)
        .size(COPY, COPY).center().radius(4.0).cursor(rune::Cursor::Pointer).focus_inset(p).hover(p.hover).press(p.pressed).hover_motion(INSTANT, INSTANT)
        .on_press_tx(move |s: &mut S, tx: &mut Tx| {
            tx.copy(body.clone());
            crate::copied::mark_copied(s, key.clone(), tx);
        })
        .child(crate::copied::glyph(s, &copy_key, COPY_GLYPH, p.icon_muted))
        .ml(COPY_AT.0);
    let interrupted = a.failure.as_ref().is_some_and(|f| super::interrupted(&f.kind));
    let mut items = reply_meta(s, t, message);
    if interrupted {
        items.push((opencode_i18n::t!("ui.message.interrupted").to_owned(), false));
    }
    let mut meta = row().key("meta").h(COPY).gap(4.0).center_y().a11y_hidden();
    for (k, (label, variant)) in items.into_iter().enumerate() {
        if k > 0 && !variant {
            meta = meta.child(text("\u{00b7}").key(format!("sep{k}")).font(META).color(p.faint));
        }
        meta = meta.child(text(label).key(format!("item{k}")).font(META).color(p.faint));
    }
    let wrapper = row().key("reply-footer").h(COPY).mt(COPY_AT.1 + code_tail).gap(4.0).center_y();
    let wrapper = if interrupted { wrapper.w_full().child(rune::spacer()) } else { wrapper };
    if keep_copy {
        wrapper.child(button).child(crate::style::hover_reveal(meta))
    } else {
        wrapper.child(crate::style::hover_reveal(row().key("reply-footer-items").h(COPY).gap(4.0).center_y().child(button).child(meta)))
    }
}

/// The footer's items after Copy (`text-part-meta`): the model's name (and a variant other
/// than default, with no separator), then how long the turn took, from its prompt to its last
/// finished step (`SessionTurn.duration`), as "1s" or "1m 5s", then its tokens per second
/// once every step has streamed (`SessionTurn.tokensPerSecond`), as "42.0 tok/s".
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
    // The turn runs to the next prompt: its own last step's end, not the session's.
    let from = prompt.map_or(0, |i| i + 1);
    let end = t.messages[message..].iter().position(|m| matches!(m, Message::User { .. })).map_or(t.messages.len(), |n| message + n);
    let done = t.messages[from..end].iter().rev().find_map(|m| match m {
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
    // The turn's tokens per second through its last step (`session-timeline-row.tsx:550-564`), to
    // one decimal ("42.0 tok/s", `message-content.tsx:1204-1213`; the TUI's `toFixed(1)`,
    // `routes/session/index.tsx:2065-2067`).
    let last = t.messages[message..end].iter().rposition(|m| matches!(m, Message::Assistant(_))).map_or(message, |n| message + n);
    if let Some(tps) = model::turn::tokens_per_second(&t.messages, last) {
        items.push((opencode_i18n::t!("ui.message.tokensPerSecond", value = opencode_i18n::fraction_digits(tps, 1, 1)), false));
    }
    items
}

/// The turn's tail: the status line while it runs (`StatusLane`: Working, or what it waits on,
/// `working::status`; the thought the model is writing stands in for it), then the reply's Copy
/// footer, with room of its own over the timeline's tail when it ends the timeline (`last`).
/// `asking`, the session waits on a question or permission, only builds the row again.
// Public timeline row helper kept stable for callers across apps and features.
#[allow(clippy::too_many_arguments)]
pub fn turn_actions<S: TimelineHost>(s: &S, p: &Pal, busy: bool, _asking: bool, last: bool, session: &str, index: usize, message: &Message) -> El {
    // These share a footprint and swap at once: a crossfade would lay the copy icon's outline
    // over the W in Working.
    let last_text = match message {
        Message::Assistant(a) => a.parts.iter().rev().find_map(|part| match part {
            Part::Text { text } if !text.trim().is_empty() => Some(text.as_str()),
            _ => None,
        }),
        _ => None,
    };
    let has_text = last_text.is_some();
    let code_tail = if last_text.is_some_and(ends_with_code) { 16.0 } else { 0.0 };
    let copy_key = format!("copy-{}", message.id());
    use crate::working::{COPY, COPY_AT, COPY_GLYPH};
    let mut button = row().key("copy").role("button").label_static(opencode_i18n::t!("ui.message.copyResponse"))
        .size(COPY, COPY).center().radius(4.0)
        .opacity(if busy || !has_text { 0.0 } else { 1.0 })
        .opacity_motion(INSTANT)
        .child(crate::copied::glyph(s, &copy_key, COPY_GLYPH, p.icon_muted));
    if !busy && has_text {
        let done = crate::copied::is_copied(s, &copy_key);
        let label = if done { opencode_i18n::t!("ui.message.copied") } else { opencode_i18n::t!("ui.message.copyResponse") };
        button = button.label_static(label).help(label).help_above().help_open(done)
            .cursor(rune::Cursor::Pointer).focus_inset(p).hover(p.hover).press(p.pressed).hover_motion(INSTANT, INSTANT);
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
    } else {
        button = button.inert().a11y_hidden();
    }
    // One reserved tail: completion crossfades in place rather than first removing Working's
    // height and then adding actions. The scroll endpoint cannot reverse during this handoff.
    let interrupted = !busy && (matches!(message, Message::Assistant(a) if a.failure.as_ref().is_some_and(|f| super::interrupted(&f.kind))) || s.is_session_stopped(session));
    // "Model · 1s" after Copy, 4 apart, the separator 4 from each item; a variant follows its
    // model with no separator.
    // The message's place, from its row (`index`): not a walk of the transcript for each row.
    let at = |t: &Transcript| t.messages.get(index).filter(|m| m.id() == message.id()).map(|_| index).or_else(|| t.messages.iter().position(|m| m.id() == message.id()));
    let items = (!busy && has_text)
        .then(|| s.with_transcript(session, |t| at(t).map(|i| reply_meta(s, t, i)).unwrap_or_default()))
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
        crate::style::hover_reveal(r)
    };
    // The Sources experiment's chip leads a finished turn's footer that searched the web: it
    // stays in view, 16 under the reply, and Copy follows it 8 on (`text-part-footer`).
    let sources = (!busy && has_text && s.knobs().get(&crate::knobs::SOURCES_ENABLED) > 0.5)
        .then(|| s.with_transcript(session, |t| at(t).and_then(|i| crate::sources_chip::of(t, i))).flatten())
        .flatten();
    // A finished reply's footer (`text-part-copy-wrapper`: 8 under it, 20 tall) sits over the
    // Working lane, which Desktop's timeline tail keeps empty under it (`virtualizer.tsx:152`).
    // Input waiting under the turn lands at once, so the footer doesn't push it down meanwhile.
    let copy_y = COPY_AT.1 + code_tail;
    let footer = if busy || !last || !(has_text || interrupted) {
        0.0
    } else if sources.is_some() && !interrupted {
        16.0 + COPY
    } else {
        copy_y + COPY
    };
    // A thought stands in for plain "Working", but a wait ("Waiting for approval" / "Waiting for answer")
    // still shows so the user sees what to act on (`message-timeline.tsx:491-496`).
    let working = busy && (s.session_waiting(session).is_some() || s.working_label_for(session).is_some() || !crate::working::thinking(s, session));
    let lane = el().w_full().h(footer + crate::working::LANE).token_motion(crate::knobs::actions_tail(s.knobs()))
        .hover_group_shared(REPLY_HOVER)
        .hover_group_focus()
        .hover_motion(INSTANT, INSTANT)
        .child(crate::working::row(s, p, session, working).key("working").absolute(0.0, crate::working::LANE_TEXT_TOP).opacity(if working { 1.0 } else { 0.0 }).opacity_motion(INSTANT));
    if interrupted {
        // A stopped turn's footer runs to the reply's end edge, "Model · 3s · Interrupted"
        // after Copy (`text-part-copy-wrapper[data-interrupted]`, `message-part.css:652-655`).
        // Interrupted stays in view and rises out of Working's place as the run settles.
        let mut tail = row().key("tail").pin(None, Some(copy_y), Some(0.0), None).h(COPY).gap(4.0).center_y();
        if has_text {
            tail = tail.child(button);
            if !items.is_empty() {
                tail = tail.child(meta(items)).child(crate::style::hover_reveal(text("\u{00b7}").key("sep-interrupted").font(META).color(p.faint)));
            }
        }
        return lane.child(tail.child(interrupted_label(p))).children(s.turn_tail_extra(p, session, busy));
    }
    let (x, y) = match &sources {
        Some(c) => (chip_width(c) + 8.0, 16.0),
        None => (COPY_AT.0, copy_y),
    };
    let lane = match &sources {
        Some(c) => lane.child(crate::sources_chip::chip(s, p, c).absolute(0.0, 16.0)),
        None => lane,
    };
    let button = button.absolute(x, y);
    lane.child(button)
        .children((!items.is_empty()).then(|| meta(items).absolute(x + COPY + 4.0, y)))
        .children(s.turn_tail_extra(p, session, busy))
}

/// Whether a Markdown reply ends with a fenced code block (`pre` inside `[data-component="markdown-code"]`
/// keeps its `margin-bottom: 24px`, collapsing with `[data-slot="text-part-copy-wrapper"]`'s `margin-top: 8px`
/// to `24px`, i.e. 16 pt below `COPY_AT.1`, `markdown.css:486-488`).
fn ends_with_code(src: &str) -> bool {
    let mut fence: Option<(u8, usize)> = None;
    let mut last_code = false;
    for line in src.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let bytes = trimmed.as_bytes();
        let mark = bytes[0];
        let run = bytes.iter().take_while(|&&b| b == mark).count();
        if let Some((open, len)) = fence {
            if mark == open && run >= len && bytes[run..].iter().all(|b| b.is_ascii_whitespace()) {
                fence = None;
                last_code = true;
            }
            continue;
        }
        if (mark == b'`' || mark == b'~') && run >= 3 && (mark == b'~' || !bytes[run..].contains(&b'`')) {
            fence = Some((mark, run));
            last_code = true;
        } else {
            last_code = false;
        }
    }
    last_code
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
/// `prompt` is the direction of the prompt the reply answers, which it reads in until its own
/// words arrive.
// Public markdown row helper kept stable for callers across apps and features.
#[allow(clippy::too_many_arguments)]
pub fn markdown_view_find<S: TimelineHost>(s: &S, p: &Pal, row_key: &str, src: &str, fade: f32, streaming: bool, message: Option<&str>, prompt: Option<rune::Direction>) -> El {
    // Session ids read as chips, except while Find marks the text as written.
    let hits = s.find_row_hits(row_key);
    let chips = if hits.is_none() { crate::session_chips::chips(s, src) } else { None };
    let tips = if chips.is_some() { crate::session_chips::tips(s, src) } else { Vec::new() };
    let src = chips.as_deref().unwrap_or(src);
    let links = s.code_links(src, streaming);
    // A reply with a ```ui outline in it draws it as parts (`genui`); any other takes this path.
    let media = crate::images::markdown::MdMedia { message: message.map(std::rc::Rc::from), scope: Some(std::rc::Rc::from(row_key)), ..crate::images::md_media(s) };
    let render = |body: &str, section_hits, highlights| {
        crate::genui::reply(s, p, row_key, body, fade, streaming, section_hits, &tips, links.clone()).unwrap_or_else(|| {
            crate::style::markdown_styled_wrap::<S, _>(
                p,
                body,
                fade,
                streaming,
                markdown_style(p, p.text, crate::genui::width(s)),
                highlights,
                crate::copied::current(s),
                Some(media.clone()),
                &tips,
                prompt,
                links.clone(),
                crate::style::CodeWrap::of(s, row_key),
            )
        })
    };
    let el = crate::video::reply(s, p, row_key, src, streaming, |range, suffix| {
        let marks = crate::genui::hits_in(hits, &range);
        let active = marks.iter().position(|(_, _, on)| *on);
        let ranges: Vec<_> = marks.iter().map(|(_, m, _)| m.clone()).collect();
        let highlights = marks.iter().map(|(i, m, on)| (m.as_slice(), crate::style::markdown_mark(p, *i, *on))).collect();
        let owned;
        let segment = if suffix.is_empty() {
            &src[range.clone()]
        } else {
            owned = format!("{}{suffix}", &src[range.clone()]);
            owned.as_str()
        };
        let seg_scope = format!("{row_key}-v-{}", range.start);
        // Only segments with a generated UI block need its own Find ranges.
        if let Some(view) = crate::genui::reply(s, p, &seg_scope, segment, fade, streaming, hits.map(|_| (ranges.as_slice(), active)), &tips, links.clone()) {
            return view;
        }
        crate::style::markdown_styled_wrap::<S, _>(
            p,
            segment,
            fade,
            streaming,
            markdown_style(p, p.text, crate::genui::width(s)),
            highlights,
            crate::copied::current(s),
            Some(media.clone()),
            &tips,
            prompt,
            links.clone(),
            crate::style::CodeWrap::of(s, &seg_scope),
        )
    }).unwrap_or_else(|| render(src, hits, s.find_markdown_highlights(p, row_key)));
    match message {
        Some(id) => crate::images::menu::on_message::<S>(el, "", id),
        None => el,
    }
}

/// Convenience wrapper with explicit body colour and diagram max width.
pub fn markdown_colored<S: TimelineHost>(p: &Pal, src: &str, fade: f32, streaming: bool, color: Color, width: f32) -> El {
    markdown_tinted::<S>(p, src, fade, streaming, color, width)
}
