//! A question call in the timeline, as Desktop's `session-ui/src/tools/tool-renderer.tsx`
//! ("question") shows it. Answered, a "Questions  N answered" disclosure that starts open and
//! lists each question over its answer, multiple picks joined with commas and typed answers as
//! typed, everything wrapping. Dismissed, a quiet "Questions dismissed" at the right. Failed
//! another way, the error. While the dock asks, nothing: the dock is the question.

use opencode_kit::model::{ToolStatus, tool_error};
use rune::{El, col, icon, row, text};
use rune_theme_opencode::icons as ic;
use serde_json::Value;

use crate::App;
use crate::disclosure::toggle;
use crate::palette::Pal;
use crate::view::{UI, UI_MED, chevron, readable};

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
    let answers = (0..questions.len()).map(|i| got.get(i).map(|a| a.join(", ")).filter(|a| !a.is_empty()).unwrap_or_else(|| "(no answer)".into())).collect();
    (questions, Some(answers))
}

/// The error a failed call shows, without its "Error: " prefix.
fn error(status: &ToolStatus, metadata: &Value) -> Option<String> {
    let e = tool_error(metadata).filter(|_| *status == ToolStatus::Error)?;
    Some(e.replace("Error: ", "").trim().to_owned()).filter(|e| !e.is_empty())
}

pub fn view(s: &App, p: &Pal, id: &str, status: &ToolStatus, input: &Value, metadata: &Value) -> El {
    let key = format!("question-{id}");
    match error(status, metadata) {
        Some(e) if e.contains("dismissed this question") => row()
            .key(key)
            .w_full()
            .h(28.0)
            .center_y()
            .justify_end()
            .child(text("Questions dismissed").key("dismissed").font(UI).color(p.faint)),
        Some(e) => failed(s, p, &key, &e),
        None => answered(s, p, &key, input, metadata),
    }
}

fn answered(s: &App, p: &Pal, key: &str, input: &Value, metadata: &Value) -> El {
    let (questions, answers) = answers(input, metadata);
    let n = questions.len();
    let subtitle = match (&answers, n) {
        (_, 0) => String::new(),
        (Some(_), n) => format!("{n} answered"),
        (None, 1) => "1 question".into(),
        (None, n) => format!("{n} questions"),
    };
    // Answered, it starts open: `expanded` holds it while closed.
    let closed = format!("{key}-closed");
    let open = answers.is_some() && !s.expanded.contains(&closed);
    let mut trigger = row()
        .key("trigger")
        .role("button")
        .label(format!("Questions {subtitle}").trim().to_owned())
        .h(28.0)
        .gap(8.0)
        .center_y()
        .child(text("Questions").key("title").font(UI_MED).color(p.text));
    if !subtitle.is_empty() {
        trigger = trigger.child(text(subtitle).key("subtitle").font(UI).color(p.muted));
    }
    if answers.is_some() {
        trigger = trigger.on_press_tx(toggle(closed, format!("{key}/trigger"))).child(icon(chevron(open), 14.0).key("chevron").color(p.icon_faint));
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

/// Desktop's tool error card: the error's first clause, and all of it when opened.
fn failed(s: &App, p: &Pal, key: &str, e: &str) -> El {
    let open_key = format!("{key}-open");
    let open = s.expanded.contains(&open_key);
    let head = e.split(": ").next().unwrap_or("").trim();
    let summary = opencode_kit::model::capitalize(if head.is_empty() { "Failed" } else { head });
    let mut trigger = row()
        .key("trigger")
        .role("button")
        .label("Questions failed")
        .h(28.0)
        .gap(8.0)
        .center_y()
        .on_press_tx(toggle(open_key, format!("{key}/trigger")))
        .child(icon(&ic::WARNING, 16.0).color(p.danger))
        .child(text("Questions").key("title").font(UI_MED).color(p.text));
    if !open {
        trigger = trigger.child(text(summary).key("summary").font(UI).color(p.danger).truncate_fade().shrink());
    }
    trigger = trigger.child(icon(chevron(open), 14.0).key("chevron").color(p.icon_faint));
    let mut c = col().key(key.to_owned()).w_full().child(trigger);
    if open {
        c = c.child(row().key("error").w_full().pl(24.0).py(4.0).child(readable(p, text(e.to_owned()).font(UI.lh(20.0)).color(p.danger).wrap_anywhere().w_full())));
    }
    c
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn answers_pair_with_their_questions() {
        let input = json!({ "questions": [
            { "header": "Clock", "question": "How should the test control time?", "options": [] },
            { "header": "Cases", "question": "Which cases?", "options": [], "multiple": true },
            { "header": "Name", "question": "What's it called?", "options": [] },
        ]});
        assert_eq!(answers(&input, &json!(null)).1, None, "no answers while the dock asks");
        let (q, a) = answers(&input, &json!({ "answers": [["Fake clock"], ["Reset after success", "Cap at 30 s"], []] }));
        assert_eq!(q[1], "Which cases?");
        assert_eq!(a.unwrap(), ["Fake clock", "Reset after success, Cap at 30 s", "(no answer)"]);
    }
}
