//! A question call in the timeline, as Desktop's `session-ui/src/tools/tool-renderer.tsx`
//! ("question") shows it, but for answered calls (see [`answered`]). Answered, each answer
//! after its question's short header, the pairs flowing and wrapping as one block, multiple picks
//! joined with commas and typed answers as typed. Dismissed, a quiet "Questions dismissed" at the
//! right. Failed another way, the error. While the dock asks, nothing: the dock is the question.

use opencode_kit::model::{ToolStatus, tool_error};
use rune::{El, col, row, text};
use serde_json::Value;

use crate::TimelineHost;
use crate::palette::Pal;
use crate::style::{UI, UI_MED, readable};

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
        None => answered(p, &key, input, metadata),
    }
}

/// Room around the pairs: with their 20 pt lines, the first and last line's text sits
/// 6 in, as in a 28 pt trigger, so the rhythm spaces the block as it spaces every other line of
/// activity (`timeline::rhythm`'s `LINE_INSET`).
const PAD_Y: f32 = 4.0;
/// Between two pairs on a line, and between wrapped lines of pairs.
const PAIR_GAP: f32 = 16.0;
const LINE_GAP: f32 = 4.0;
/// Between a pair's header and its answer.
const HEADER_GAP: f32 = 6.0;
/// The widest a header (or a question shown in place of a missing one) grows before it fades.
const HEADER_W: f32 = 240.0;
/// The narrowest an answer goes beside its header (or its own width, when shorter); with less
/// room it goes under it.
const ANSWER_MIN_W: f32 = 96.0;
/// The most lines an answer takes before it ends in "…".
const ANSWER_LINES: usize = 3;

/// Whether the call came back answered (not dismissed, not failed): the timeline's rhythm spaces
/// the pairs as lines of activity, the dismissed line and the error card as they always were.
pub(crate) fn is_answered(status: &ToolStatus, metadata: &Value) -> bool {
    error(status, metadata).is_none()
}

// Rune-only divergence, asked for by the owner: Desktop shows an answered call as a
// "Questions  N answered" disclosure listing each question over its answer
// (`tool-renderer.tsx` "question"), which stood far taller than the answers it holds. Here each
// answer is one pair on a flowing line, its question's short header muted before it, pairs
// 16 pt apart and wrapping as whole pairs; nothing to open or close. The question itself is
// each pair's tooltip, and the pair's accessible name is "<question>: <answer>".
//
// Long answers: a pair wider than the line takes a line of its own and its answer wraps beside
// the header, hanging under its own first word, for at most three lines; past that it ends in
// "…". Copying the answer, and its accessible name, keep the whole of it. Where the line leaves
// less than 96 pt beside the header (a very narrow column), the answer goes under it instead.
// A header longer than 240 pt, or than the line, fades out.
fn answered(p: &Pal, key: &str, input: &Value, metadata: &Value) -> El {
    let (questions, answers) = answers(input, metadata);
    let Some(answers) = answers else {
        // Finished without answers to show: the title and how many it asked, as one quiet line.
        let n = questions.len();
        let mut line = row().key(key.to_owned()).w_full().h(28.0).gap(8.0).center_y()
            .child(text(opencode_i18n::t!("ui.tool.questions")).key("title").font(UI_MED).color(p.text));
        if n > 0 {
            let count = format!("{n} {}", opencode_i18n::plural_text(opencode_i18n::plural_key!("ui.common.question"), n as u64));
            line = line.child(text(count).key("subtitle").font(UI).color(p.muted));
        }
        return line;
    };
    let headers: Vec<Option<String>> = input["questions"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|q| q["question"].is_string())
        .map(|q| q["header"].as_str().map(str::trim).filter(|h| !h.is_empty()).map(str::to_owned))
        .collect();
    let font = UI.lh(20.0);
    let pairs = questions.into_iter().zip(answers).enumerate().map(|(i, (q, a))| {
        // No header: the question itself in its place.
        let header = headers.get(i).cloned().flatten().unwrap_or_else(|| q.clone());
        let narrowest = rune::text::measure(&a, &[], font, None).0.min(ANSWER_MIN_W);
        row()
            .key(format!("qa-{i}"))
            .label(format!("{q}: {a}"))
            .help(q)
            .items_start()
            .flex_wrap()
            .gaps(0.0, HEADER_GAP)
            .shrink()
            .child(readable(p, text(header).key("h").font(font).color(p.muted).truncate_fade().max_w(HEADER_W)))
            .child(readable(p, text(a).key("a").font(font).color(p.text).wrap_anywhere().max_lines(ANSWER_LINES).grow(1.0).min_w(narrowest)))
    });
    // The caller sets the row's own top padding (the rhythm's room), so the pairs sit inside it.
    col().key(key.to_owned()).w_full().child(row().key("pairs").w_full().py(PAD_Y).flex_wrap().gaps(LINE_GAP, PAIR_GAP).children(pairs))
}
