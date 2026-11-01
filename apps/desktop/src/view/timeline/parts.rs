use super::super::*;
use crate::fold::Fold;

/// Where a prompt's bubble sits in its run (see `user_bubble`), and where it lays out.
#[derive(Clone, Copy)]
pub(in crate::view) struct Place {
    pub(super) tight: bool,
    pub(super) above: bool,
    pub(super) below: bool,
    pub(super) fold: Fold,
}

/// 64ch of the bubble's 14 px Inter: Desktop caps a bubble at `min(82%, 64ch)`.
pub(crate) const BUBBLE_MAX: f32 = 565.0;

/// Whether prompt `b` joins the run of prompt `a` just above it: both have words and `b` brings
/// no files (Desktop's `user-runs.ts`).
pub(in crate::view) fn joins(a: &Message, b: &Message) -> bool {
    match (a, b) {
        (Message::User { text: x, .. }, Message::User { id, text: y }) => {
            !x.trim().is_empty() && !y.trim().is_empty() && model::user_files(id).is_empty()
        }
        _ => false,
    }
}

/// Whether pending steer `b` joins the run of the prompt (or steer) with text `a` above it.
pub(in crate::view) fn prompts_join(a: &str, b: &InboxItem) -> bool {
    !a.trim().is_empty() && !b.text.trim().is_empty() && b.files.is_empty() && model::user_files(&b.id).is_empty()
}

/// A prompt's bubble. A run of prompts sits 4 apart with the trailing corners tightened where
/// they meet (`above`, `below`); the run's last prompt has a copy line under it, and the others
/// show copy beside the bubble, all only while hovered.
///
/// `pending` (the session) draws an undelivered steer: muted, with Move to queue and Delete in
/// copy's place, and the run's last one marked Pending under its bubble at all times.
///
/// `quotes` are the prompt's quote cards: inside the bubble under its words, or in its place
/// when it has none.
pub(in crate::view) fn user_bubble(s: &App, p: &Pal, id: &str, body: &str, place: Place, glow: f32, pending: Option<&str>, quotes: Option<El>) -> El {
    let Place { tight, above, below, fold } = place;
    let (actions, pin_x) = match pending {
        Some(session) => (crate::queue::pending_actions(p, session, id, below), -50.0),
        None => crate::rewind::user_actions(s, p, id, body),
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
    let (shown, badges) = s.bubble_badges(p, id, body);
    let highlights = if pending.is_none() { s.find.text_highlights(p, &format!("u-{id}")) } else { Vec::new() };
    let words = readable(p, text(shown.clone()).key(format!("text-{id}")).font(BUBBLE).color(ink).wrap_anywhere().pills(badges).text_highlights(highlights));
    bubble = bubble.children(crate::fold::view(s, p, id, &shown, words, fold, quotes.is_some() && !body.trim().is_empty()));
    let mut face = match quotes {
        Some(cards) if body.trim().is_empty() => cards.key("bubble"),
        Some(cards) => bubble.w(342.0).pb(12.0).child(cards),
        None => bubble,
    };
    let actions = if below {
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

pub(crate) fn chevron(open: bool) -> &'static Icon {
    if open { &ic::CHEVRON_DOWN } else { &ic::CHEVRON_RIGHT }
}

pub(crate) fn reasoning(p: &Pal, key: &str, body: &str, secs: Option<i64>, open: bool, fade: f32) -> El {
    let label = match secs {
        Some(s) => format!("Thought for {s}s"),
        None => "Thinking".into(),
    };
    let mut c = col().w_full().child(
        row()
            .key("trigger")
            .role("button")
            .h(28.0)
            .gap(6.0)
            .center_y()
            // "Thinking" crossfades to "Thought for Ns" while the chevron slides over.
            .animate_layout()
            // Opening and closing is instant; only the streaming label change animates.
            .on_press_tx(crate::disclosure::toggle(key.to_owned(), format!("{key}/trigger")))
            .child(icon(&ic::BRAIN, 16.0).color(p.icon_muted))
            .child(text(label).key(if secs.is_some() { "thought" } else { "thinking" }).font(UI).color(p.muted))
            .child(icon(chevron(open), 14.0).color(p.icon_faint)),
    );
    if open {
        c = c.child(
            row().key("thought").pl(22.0).pt(4.0).child(readable(p, text(body.to_owned()).font(UI.lh(20.0)).color(p.muted).wrap().reveal(fade).w_full())),
        );
    }
    c
}

pub(in crate::view) fn turn_actions(s: &App, p: &Pal, busy: bool, session: &str, message: &Message) -> El {
    // These share a footprint and swap at once: a crossfade would lay the copy icon's outline
    // over the W in Working.
    let has_text = matches!(message, Message::Assistant(a) if a.parts.iter().any(|part| matches!(part, Part::Text { text } if !text.trim().is_empty())));
    let copy_key = format!("copy-{}", message.id());
    let mut button = row().key("copy").role("button").label("Copy response")
        .size(28.0, 28.0).pb(12.0).center().radius(4.0).hover(p.hover).press(p.pressed)
        .opacity(if busy || !has_text { 0.0 } else { 1.0 })
        .opacity_motion(INSTANT)
        .child(crate::copied::glyph(s, &copy_key, 16.0, p.muted));
    if !busy && has_text {
        let session = session.to_owned();
        let id = message.id().to_owned();
        button = button.on_press_tx(move |s: &mut App, tx: &mut Tx| {
            if let Some(Message::Assistant(a)) = s.transcripts.get(&session).and_then(|t| t.messages.iter().find(|m| m.id() == id)) {
                tx.copy(a.parts.iter().filter_map(|part| match part {
                    Part::Text { text } => Some(text.as_str()),
                    _ => None,
                }).collect::<Vec<_>>().join("\n\n"));
                s.mark_copied(copy_key.clone(), tx);
            }
        });
    }
    // One reserved tail: completion crossfades in place rather than first removing Working's
    // height and then adding actions. The scroll endpoint cannot reverse during this handoff.
    let interrupted = !busy && (matches!(message, Message::Assistant(a) if a.failure.as_ref().is_some_and(|f| super::interrupted(&f.kind))) || s.stops.has(session));
    el().w_full().h(crate::working::LANE).motion(crate::knobs::actions_tail(&s.knobs))
        .child(crate::working::row(s, p, session, busy).key("working").absolute(0.0, crate::working::LANE_PAD).opacity(if busy { 1.0 } else { 0.0 }).opacity_motion(INSTANT))
        .child(button.absolute(0.0, crate::working::LANE - 28.0))
        .children(interrupted.then(|| crate::stop::interrupted(p, has_text)))
        .children(crate::background::hint(s, p, session, busy))
}

// ---------------------------------------------------------------------------------------------
// Markdown

#[cfg(test)]
pub(in crate::view) fn markdown_view(p: &Pal, src: &str, fade: f32, streaming: bool) -> El {
    markdown_tinted(p, src, fade, streaming, p.text, super::CONTENT_W)
}

pub(in crate::view) fn markdown_view_find(s: &App, p: &Pal, row_key: &str, src: &str, fade: f32, streaming: bool) -> El {
    // A reply with a ```ui outline in it draws it as parts (`genui`); any other takes this path.
    if let Some(el) = crate::genui::reply(s, p, src, fade, streaming, s.find.row_hits(row_key)) {
        return el;
    }
    markdown_with_highlights(p, src, fade, streaming, p.text, crate::genui::width(s), s.find.markdown_highlights(p, row_key))
}

/// `markdown_view` with its body text in `color`, its diagrams at most `width` points wide.
pub(crate) fn markdown_tinted(p: &Pal, src: &str, fade: f32, streaming: bool, color: Color, width: f32) -> El {
    markdown_with_highlights(p, src, fade, streaming, color, width, Vec::new())
}

fn markdown_with_highlights(p: &Pal, src: &str, fade: f32, streaming: bool, color: Color, width: f32, highlights: Vec<(&[std::ops::Range<usize>], rune_kit::Highlight)>) -> El {
    markdown_styled(p, src, fade, streaming, markdown_style(p, color, width), highlights)
}

/// The timeline's Markdown look, its body text in `color`, its diagrams at most `width` points
/// wide (the column they sit in: wider ones lay out top-down or draw smaller).
pub(crate) fn markdown_style(p: &Pal, color: Color, width: f32) -> MarkdownStyle {
    let style = MarkdownStyle::from_kit(crate::palette::kit(p))
        .text(color)
        .muted(p.muted)
        .link(p.accent)
        .inline_code_bg(p.inline_code_bg)
        .code_block(p.layer1, p.border_muted)
        .quote_rule(p.layer3)
        .rule(p.border)
        .body_font(MD)
        .heading_font(1, H1)
        .heading_font(2, H2)
        .heading_font(3, H3)
        .code_block_font(CODE)
        .selection(p.selection)
        .diagram_max_width(width);
    match p.code {
        Some(code) => style.syntax_theme(code.syntax(rune_kit::syntax::SyntaxTheme::from_kit(crate::palette::kit(p)), p.diff_add, p.diff_del)),
        None => style,
    }
}

pub(crate) fn markdown_styled<H: rune_kit::markdown::HighlightItem>(p: &Pal, src: &str, fade: f32, streaming: bool, style: MarkdownStyle, highlights: Vec<H>) -> El {
    // Its texts join the timeline's selection group rather than forming one of their own, so
    // a drag runs on out of this reply into the rest of the timeline.
    Markdown::new(src).style(style).highlights(highlights).reveal(fade).streaming(streaming).selectable(true)
        .on_link_tx(|s: &mut App, url: &str, tx: &mut Tx| {
            if s.live() || cfg!(test) {
                let _ = tx.open_url(url);
            }
        })
        .build(crate::palette::kit(p)).selection_group(false)
}

/// Text people can select and copy, as Desktop's content is (`user-select: text`); the chrome
/// around it (triggers, labels, buttons) stays unselectable.
pub(crate) fn readable(p: &Pal, t: El) -> El {
    t.selectable(true).selection_color(p.selection)
}
