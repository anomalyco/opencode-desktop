//! A question call in the timeline, as Desktop's `session-ui/src/tools/tool-renderer.tsx`
//! ("question") shows it. Answered, a "Questions  N answered" disclosure that starts open and
//! lists each question over its answer, multiple picks joined with commas and typed answers as
//! typed, everything wrapping. Dismissed, a quiet "Questions dismissed" at the right. Failed
//! another way, the error. While the dock asks, nothing: the dock is the question.

use opencode_kit::model::{ToolStatus, tool_error};
use rune::{El, col, row, text};
use serde_json::Value;

use crate::TimelineHost;
use crate::disclosure::toggle;
use crate::palette::Pal;
use crate::style::{UI, UI_MED, arrow, readable};

/// Each question's text, and its answer once the call has them (`None` before).
pub fn answers(input: &Value, metadata: &Value) -> (Vec<String>, Option<Vec<String>>) {
    let questions: Vec<String> = input["questions"].as_array().into_iter().flatten().filter_map(|q| q["question"].as_str().map(str::to_owned)).collect();
    let got: Vec<Vec<String>> = metadata["answers"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|a| a.as_array()?.iter().map(|v| v.as_str().map(str::to_owned)).collect::<Option<Vec<_>>>())
        .collect();
    if got.is_empty() {
        return (questions, None);
    }
    let answers = (0..questions.len()).map(|i| got.get(i).map(|a| a.join(", ")).filter(|a| !a.is_empty()).unwrap_or_else(|| opencode_i18n::t!("ui.question.answer.none").into())).collect();
    (questions, Some(answers))
}

/// The error a failed call shows, without its "Error: " prefix.
fn error(status: &ToolStatus, metadata: &Value) -> Option<String> {
    let e = tool_error(metadata).filter(|_| *status == ToolStatus::Error)?;
    Some(e.replace("Error: ", "").trim().to_owned()).filter(|e| !e.is_empty())
}

pub fn view<S: TimelineHost>(s: &S, p: &Pal, session: &str, id: &str, status: &ToolStatus, input: &Value, metadata: &Value) -> El {
    let key = format!("question-{id}");
    match error(status, metadata) {
        Some(e) if e.contains("dismissed this question") => row()
            .key(key)
            .w_full()
            .h(28.0)
            .center_y()
            .justify_end()
            .child(text(opencode_i18n::t!("ui.messagePart.questions.dismissed")).key("dismissed").font(UI).color(p.faint)),
        // Failed another way: the tool error card every other tool's failure shows.
        Some(_) => col().key(key).w_full().child(crate::tools::error::card(s, p, session, &crate::tools::Call { id, name: "question", status, input, metadata })),
        None => answered(s, p, &key, input, metadata),
    }
}

fn answered<S: TimelineHost>(s: &S, p: &Pal, key: &str, input: &Value, metadata: &Value) -> El {
    let (questions, answers) = answers(input, metadata);
    let n = questions.len();
    let subtitle = match (&answers, n) {
        (_, 0) => String::new(),
        (Some(_), n) => opencode_i18n::t!("ui.question.subtitle.answered", count = n),
        (None, n) => format!("{n} {}", opencode_i18n::plural_text(opencode_i18n::plural_key!("ui.common.question"), n as u64)),
    };
    // Answered, it starts open: `expanded` holds it while closed.
    let closed = format!("{key}-closed");
    let open = answers.is_some() && !s.expanded().contains(&closed);
    let mut trigger = row()
        .key("trigger")
        .role("button")
        .label(format!("{} {subtitle}", opencode_i18n::t!("ui.tool.questions")).trim().to_owned())
        .expanded(open)
        .h(28.0)
        .gap(8.0)
        .center_y()
        .child(text(opencode_i18n::t!("ui.tool.questions")).key("title").font(UI_MED).color(p.text));
    if !subtitle.is_empty() {
        trigger = trigger.child(text(subtitle).key("subtitle").font(UI).color(p.muted));
    }
    if answers.is_some() {
        let a = arrow(p, open);
        // The arrow fades in and out over 0.15 s ease (`collapsible.css:27-44`).
        trigger = trigger.hover_group().hover_motion(crate::style::HOVER_FADE, crate::style::HOVER_FADE).on_press_tx(toggle::<S>(closed, format!("{key}/trigger"))).child(if open { a } else { a.group_opacity(0.0, 1.0) });
    }
    let mut c = col().key(key.to_owned()).w_full().child(trigger);
    if let (true, Some(answers)) = (open, answers) {
        let items = questions.into_iter().zip(answers).enumerate().map(|(i, (q, a))| {
            col()
                .key(format!("qa-{i}"))
                .w_full()
                .gap(2.0)
                .child(readable(p, text(q).key("q").font(UI.lh(20.0)).color(p.muted).wrap().w_full()))
                .child(readable(p, text(a).key("a").font(UI.lh(20.0)).color(p.text).wrap_anywhere().w_full()))
        });
        c = c.child(col().key("answers").w_full().pl(12.0).py(8.0).gap(12.0).children(items));
    }
    c
}

