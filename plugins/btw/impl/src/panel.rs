//! A side question's tab (`panel.tsx`): the question as a prompt with its agent, model and time,
//! then Working while it runs, the answer as a reply with Copy and its agent, model and duration,
//! or why it failed with Retry. Its label in the strip is the question beside the comment icon.

use opencode_plugin::abi::BUTTON_OUTLINE;
use opencode_plugin::prelude::*;

use crate::Btw;
use crate::machine::{ChatModel, Failure, Info, Phase};

/// `md:max-w-session`: the timeline's width.
const SESSION_W: f32 = 744.0;
/// A prompt bubble's widest (`min(82%, 64ch)` at 14 px Inter, as the timeline's).
const BUBBLE_MAX: f32 = 565.0;

pub fn label(s: &Btw, cx: &mut ViewCx<Btw>, props: &Props) -> El {
    let saved = crate::store::load(&props.session_id);
    let title = match s.state.tab(&saved, &props.session_id, &props.path) {
        crate::machine::TabView::Ready(q) => q,
        crate::machine::TabView::Missing => "/btw".to_owned(),
    };
    // A label of its own keeps the trigger's ink, selected or not (`flex items-center gap-1.5`).
    let p = cx.pal();
    row()
        .shrink()
        .min_w(0.0)
        .gap(6.0)
        .center_y()
        .child(icon("comment", 16.0).color(p.muted))
        .child(text(title).font(13.0, 440.0).line_height(20.0).color(p.muted).truncate().shrink().min_w(0.0))
}

pub fn render(s: &Btw, cx: &mut ViewCx<Btw>, props: &Props) -> El {
    let (session, id) = (props.session_id.clone(), props.path.clone());
    let saved = crate::store::load(&session);
    let (phase, info) = s.state.view(&saved, &session, &id);
    let copied: Vec<&'static str> = s.copied.iter().filter(|(c, _)| *c == id).map(|(_, k)| *k).collect();
    let model = info.model.as_ref().map(|m| model_label(cx, m)).unwrap_or_default();
    let stamp = info.created.map(|ms| short_time(ms, cx.utc_offset(ms))).unwrap_or_default();
    let version = (&phase, &info, &copied, &model, &stamp, cx.dark());
    cx.memo(&format!("btw-{session}-{id}"), version, |cx| {
        let body = col().key("btw-content").w_full().max_w(SESSION_W).px(24.0).pt(24.0).pb(32.0);
        let body = if matches!(phase, Phase::Missing) {
            body
        } else {
            let agent = capitalize(&info.agent);
            let body = body.child(prompt(cx, &id, &info, &agent, &model, &stamp, copied.contains(&"q")));
            match &phase {
                Phase::Running => body.child(row().key("btw-working").role("status").h(36.0).pt(12.0).center_y().child(ui::shimmer("Working").font(13.0, 530.0).line_height(16.0))),
                Phase::Failed(f) => body.child(failed(cx, &session, &id, f)),
                Phase::Answered { answer, duration } => body.child(answered(cx, &id, answer, &agent, &model, *duration, copied.contains(&"a"))),
                Phase::Missing => body,
            }
        };
        let p = cx.pal();
        col()
            .key("session-btw-panel")
            .w_full()
            .h_full()
            .fill(p.bg_base)
            .child(col().key("btw-scroll").w_full().grow(1.0).min_h(0.0).scroll_y().items_center().child(body))
    })
}

/// The question (`user-message`): its bubble at the end, and under it, while hovered, its
/// agent, model and time with Copy message.
fn prompt(cx: &mut ViewCx<Btw>, id: &str, info: &Info, agent: &str, model: &str, stamp: &str, copied: bool) -> El {
    let p = cx.pal();
    let faint = cx.token("text-text-faint").unwrap_or(p.faint);
    let layer2 = cx.token("background-bg-layer-02").unwrap_or(p.layer2);
    let bubble = row()
        .key("btw-question")
        .max_w(BUBBLE_MAX)
        .shrink()
        .px(12.0)
        .py(8.0)
        .radius(10.0)
        .fill(layer2)
        .child(text(info.question.clone()).font(14.0, 440.0).line_height(21.0).color(p.text).wrap().selectable());
    let user_meta = [agent, model].into_iter().filter(|s| !s.is_empty()).collect::<Vec<_>>().join("\u{a0}\u{b7}\u{a0}");
    let mut meta = row().key("btw-question-meta").min_w(0.0).shrink().center_y();
    let meta_text = |t: String| text(t).font(12.0, 440.0).line_height(18.0).tracking(0.05).color(faint).no_wrap();
    if !user_meta.is_empty() {
        meta = meta.child(meta_text(user_meta.clone()).truncate().shrink().min_w(0.0));
    }
    if !user_meta.is_empty() && !stamp.is_empty() {
        meta = meta.child(meta_text("\u{a0}\u{b7}\u{a0}".into()));
    }
    if !stamp.is_empty() {
        meta = meta.child(meta_text(stamp.to_owned()));
    }
    let (question, cid) = (info.question.clone(), id.to_owned());
    let tip = if copied { "Copied" } else { "Copy message" };
    let copy = icon_button(cx, "btw-copy-question", copied, tip).on_press(cx.handler(move |s: &mut Btw, cx| {
        cx.copy_text(&question);
        mark(s, &cid, "q");
    }));
    let footer = row().key("btw-question-footer").w_full().h(20.0).mt(8.0).gap(8.0).center_y().reveal_on_hover().child(spacer()).child(meta).child(copy);
    col()
        .key(format!("btw-prompt-{id}"))
        .w_full()
        .items_end()
        .hover_group()
        .child(row().w_full().child(spacer()).child(bubble))
        .child(footer)
}

/// The answer (`text-part`): Markdown at 14, and under it, while hovered, Copy answer with its
/// agent, model and duration.
fn answered(cx: &mut ViewCx<Btw>, id: &str, answer: &str, agent: &str, model: &str, duration: Option<i64>, copied: bool) -> El {
    let p = cx.pal();
    let faint = cx.token("text-text-faint").unwrap_or(p.faint);
    let dur = duration.filter(|d| *d >= 0).map(|ms| {
        let total = (ms as f64 / 1000.0).round() as i64;
        if total < 60 { format!("{total}s") } else { format!("{}m {}s", total / 60, total % 60) }
    });
    let meta = [agent.to_owned(), model.to_owned(), dur.unwrap_or_default()].into_iter().filter(|s| !s.is_empty()).collect::<Vec<_>>().join(" \u{b7} ");
    let (text_answer, cid) = (answer.to_owned(), id.to_owned());
    let tip = if copied { "Copied" } else { "Copy answer" };
    let copy = icon_button(cx, "btw-copy-answer", copied, tip).ml(-2.0).on_press(cx.handler(move |s: &mut Btw, cx| {
        cx.copy_text(&text_answer);
        mark(s, &cid, "a");
    }));
    let mut footer = row().key("btw-answer-footer").h(20.0).mt(8.0).gap(4.0).center_y().reveal_on_hover().child(copy);
    if !meta.is_empty() {
        footer = footer.child(text(meta).font(12.0, 440.0).line_height(18.0).tracking(0.05).color(faint).no_wrap().truncate().shrink().min_w(0.0));
    }
    col().key("btw-answer").w_full().mt(24.0).hover_group().child(ui::markdown(answer.to_owned()).line_height(25.2).w_full()).child(footer)
}

/// Why it failed, and Retry (`flex flex-col items-start gap-3 pt-4`).
fn failed(cx: &mut ViewCx<Btw>, session: &str, id: &str, failure: &Failure) -> El {
    let p = cx.pal();
    let weak = cx.token("text-weak").unwrap_or(p.faint);
    let detail = match failure {
        Failure::Empty => Some("The model replied without any text.".to_owned()),
        Failure::Timeout => Some("The model took too long to reply.".to_owned()),
        Failure::Error(m) => m.clone(),
        Failure::Interrupted => None,
    };
    let mut words = col().gap(4.0).child(text("Couldn't answer that question").font(13.0, 440.0).line_height(19.5).color(weak));
    if let Some(d) = detail {
        words = words.child(text(d).font(13.0, 440.0).line_height(19.5).color(weak).wrap());
    }
    let (session, id) = (session.to_owned(), id.to_owned());
    let retry = ui::button_with("Retry", BUTTON_OUTLINE).small().key("btw-retry").on_press(cx.handler(move |s: &mut Btw, cx| s.retry(cx, &session, &id)));
    col().key("btw-failed").w_full().gap(12.0).pt(16.0).child(words).child(row().child(retry))
}

/// Desktop's `IconButton size="normal" variant="ghost-muted"` with the copy glyph (a check once
/// copied) at 14, its tooltip above.
fn icon_button(cx: &mut ViewCx<Btw>, key: &'static str, copied: bool, tip: &'static str) -> El {
    let p = cx.pal();
    row()
        .key(key)
        .role("button")
        .label(tip)
        .help(tip)
        .help_above()
        .size(24.0, 24.0)
        .center()
        .radius(6.0)
        .hover(p.hover)
        .press(p.pressed)
        .instant_hover()
        .child(icon(if copied { "check" } else { "outline-copy" }, 14.0).color(p.icon_muted))
}

fn mark(s: &mut Btw, id: &str, which: &'static str) {
    if id.is_empty() {
        return;
    }
    if !s.copied.iter().any(|(c, k)| c == id && *k == which) {
        s.copied.push((id.to_owned(), which));
    }
}

/// The model's name in the catalog (else its id), with a variant other than default after it
/// (`ui.message.modelVariant`).
fn model_label(cx: &ViewCx<Btw>, m: &ChatModel) -> String {
    let name = cx.model(&m.provider, &m.id).name.unwrap_or_else(|| m.id.clone());
    if m.variant.is_empty() || m.variant == "default" {
        return name;
    }
    format!("{name} ({})", capitalize(&m.variant))
}

fn capitalize(s: &str) -> String {
    let mut c = s.chars();
    c.next().map(|f| f.to_uppercase().chain(c).collect()).unwrap_or_default()
}

/// `Intl.DateTimeFormat(locale, { timeStyle: "short" })` in English: "3:05 PM".
fn short_time(ms: i64, offset_s: i64) -> String {
    let local = ms / 1000 + offset_s;
    let minutes = local.rem_euclid(86_400) / 60;
    let (h, m) = (minutes / 60, minutes % 60);
    let h12 = if h % 12 == 0 { 12 } else { h % 12 };
    format!("{h12}:{m:02}\u{202f}{}", if h < 12 { "AM" } else { "PM" })
}
