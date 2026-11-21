//! A turn's work as Desktop's New timeline projects it (`session-ui/src/timeline-next/projection.ts`:
//! `turnRows`, `groupContent`, `groupNotices`; `rules.ts`: `contentShows`, `contentGroup`), by
//! Settings › Preferences › Timeline detail (`detail.rs`).
//!
//! Each kind of activity stands on its own row, folds into the turn's activity group, or doesn't
//! show, a failure included. What folds gathers across a turn's steps, up to the next row of its
//! own: thoughts, tool calls, notices and, where commentary folds, the text a later tool call
//! follows. Each subagent event, a launch, a follow-up or a report, is a row of its own in
//! timeline order, never counted (anomalyco/opencode#64017). Background shells are events: where
//! their setting groups them their launch joins the group, else launches one after another share
//! one events row ("Started 2 shells"), as reports one after another do; a report right after the
//! group that launched it finishes there. The thought the model is writing stands at
//! the turn's end as Thinking; finished, it settles where thoughts go. A group names what the
//! agent did (`activity::label`); one of thoughts alone is those thoughts, without a group.

mod view;

pub use view::{calls, events as events_row, group, subagent as subagent_row, thought, widget};

use opencode_kit::model::{Assistant, Attachment, Notice, Part, ToolStatus};

use crate::activity::{events, rules};
use crate::detail::{Category, Detail, Placement};

/// One entry of a turn: a step, or a notice that folds with its work.
#[derive(Clone, Copy, Debug)]
pub enum Entry<'a> {
    Step(&'a Assistant),
    Notice(&'a Notice),
    /// A message the turn passes over without showing (a hidden notice).
    Skip,
    /// The run ended (an idle marker), unshown: a wake-up (a watch, a subagent, a job) can start
    /// another run in the same turn, and each run keeps its own answer ([`commentary`]).
    Idle,
}

/// One step inside a group, or on its own row.
#[derive(Debug)]
pub enum Step<'a> {
    Thought { msg: &'a str, index: usize, body: &'a str, secs: Option<i64>, started: i64 },
    Tool { id: &'a str, name: &'a str, title: &'a str, status: &'a ToolStatus, files: &'a [Attachment], input: &'a serde_json::Value, metadata: &'a serde_json::Value },
    /// A notice folded into the turn's group.
    Notice(&'a Notice),
    /// Commentary folded into the group: text a later tool call in the turn follows.
    Text { body: &'a str },
}

/// How calls on their own row draw.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Calls {
    /// One call as itself.
    One,
    /// Plain reads sharing a line.
    Reads,
    /// Edits, writes and patches sharing a file card.
    Files,
}

/// Where a group's step comes from: its entry (the turn's message, counted from the turn's
/// first) and part, so an open group's virtual rows find a step again from the transcript
/// without folding the turn. `n`: a thought's index among its message's thoughts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Loc {
    pub entry: u32,
    pub part: u32,
    pub n: u32,
}

impl Loc {
    /// A notice's entry has no part.
    const NOTICE: u32 = u32::MAX;

    /// The step this names in the turn's `messages` (one entry each, as [`fold_with`] read them).
    pub fn step<'a>(self, messages: &'a [opencode_kit::model::Message]) -> Option<Step<'a>> {
        use opencode_kit::model::Message;
        let m = messages.get(self.entry as usize)?;
        if self.part == Loc::NOTICE {
            return match m {
                Message::Notice(n) => Some(Step::Notice(n)),
                _ => None,
            };
        }
        let a = match m {
            Message::Assistant(a) => a,
            Message::Note { step, .. } => &**step,
            _ => return None,
        };
        Some(match a.parts.get(self.part as usize)? {
            Part::Text { text } => Step::Text { body: text },
            Part::Reasoning { text, started, completed } => Step::Thought {
                msg: &a.id,
                index: self.n as usize,
                body: text,
                secs: completed.map(|c| ((c - started) as f64 / 1000.0).round().max(0.0) as i64),
                started: *started,
            },
            Part::Tool { id, name, title, status, input, files, metadata } => Step::Tool { id, name, title, status, files, input, metadata },
        })
    }
}

/// One timeline row of a turn's assistant work.
#[derive(Debug)]
pub enum Block<'a> {
    /// Folded thoughts, tool calls, notices and commentary; `at`: each step's [`Loc`].
    Group { key: String, steps: Vec<Step<'a>>, at: Vec<Loc> },
    Text { key: String, body: &'a str, msg: usize, part: usize, live: bool },
    /// Background shells started together, or reporting back one after another, as one events
    /// row (`groupContent`'s `events`, `groupNotices`): "Started 2 shells".
    Events { key: String, steps: Vec<Step<'a>> },
    /// One subagent event, a call or a report, on a row of its own (`groupContent`'s
    /// `subagent`, anomalyco/opencode#64017).
    Subagent { key: String, step: Step<'a> },
    /// A question call, once answered, dismissed or failed (`question_row`).
    Question { id: &'a str, status: &'a ToolStatus, input: &'a serde_json::Value, metadata: &'a serde_json::Value },
    /// A thought on its own row: Thinking separately, or (`live`) the thought the model is writing,
    /// which stands at the end of the turn in place of the status line (`TimelineRow.Thinking`).
    Thought { key: String, step: Step<'a>, live: bool },
    /// Calls on their own row (their kind shows separately).
    Calls { key: String, kind: Calls, steps: Vec<Step<'a>> },
    /// An agent widget (a `widget` call of assistant message `message`), always on its own row
    /// where the agent posted it, whatever Timeline detail says of tools: the host draws it in
    /// the call's place ([`crate::tools::widget`]).
    Widget { key: String, message: &'a str, step: Step<'a> },
}

/// Whether every step is a thought, or every step a notice: notices never join a run of
/// thoughts (`projection.ts`'s `joins`).
fn all_thoughts(group: &[Step]) -> bool {
    !group.is_empty() && group.iter().all(|s| matches!(s, Step::Thought { .. }))
}

fn all_notices(group: &[Step]) -> bool {
    !group.is_empty() && group.iter().all(|s| matches!(s, Step::Notice(_)))
}

/// Folds consecutive assistant messages (a turn's steps) as Desktop's timeline does, by the
/// default Timeline detail. `live`: the last one is still streaming.
pub fn fold<'a>(steps: &[&'a Assistant], live: bool) -> Vec<Block<'a>> {
    let entries: Vec<Entry<'a>> = steps.iter().map(|a| Entry::Step(a)).collect();
    fold_with(&entries, live, &Detail::default())
}

/// Whether an events block holds only reports (notices), or only launches (calls): the two never
/// share a row (`projection.ts`'s `joins`, `groupContent`).
fn events_of_notices(steps: &[Step]) -> bool {
    steps.iter().all(|s| matches!(s, Step::Notice(_)))
}

/// Whether the open activity group started the work `e` is about (`launches`).
fn launched(group: &[Step], e: &events::Event) -> bool {
    let Some(key) = events::work_key(e) else { return false };
    group.iter().any(|st| matches!(st, Step::Tool { .. }) && crate::activity::label::step_event(st).is_some_and(|l| events::work_key(&l) == Some(key)))
}

/// A turn's `entries` as rows, by `detail`. `live`: the last step is still streaming. A block's
/// `msg` is its entry's index.
pub fn fold_with<'a>(entries: &[Entry<'a>], live: bool, detail: &Detail) -> Vec<Block<'a>> {
    let commentary = commentary(entries, !live);
    let mut out: Vec<Block<'a>> = Vec::new();
    let mut group: Vec<Step<'a>> = Vec::new();
    let mut at: Vec<Loc> = Vec::new();
    let mut key: Option<String> = None;
    let flush = |out: &mut Vec<Block<'a>>, group: &mut Vec<Step<'a>>, at: &mut Vec<Loc>, key: &mut Option<String>| {
        if !group.is_empty() {
            let key = key.take().unwrap_or_else(|| String::from("c-"));
            out.push(Block::Group { key, steps: std::mem::take(group), at: std::mem::take(at) });
        }
    };
    // An events block joins the one before it only while nothing came between.
    let push_event = |out: &mut Vec<Block<'a>>, key: String, step: Step<'a>| {
        let notice = matches!(step, Step::Notice(_));
        match out.last_mut() {
            Some(Block::Events { steps, .. }) if events_of_notices(steps) == notice => steps.push(step),
            _ => out.push(Block::Events { key, steps: vec![step] }),
        }
    };
    let last = entries.iter().rposition(|e| matches!(e, Entry::Step(_)));
    for (mi, e) in entries.iter().enumerate() {
        let a = match e {
            Entry::Notice(n) => {
                // Background work reporting back (`groupNotices`): a subagent's report is a row of
                // its own; a shell's, right after the group that started it, finishes there, else
                // it is an event of its own.
                if let Some(ev) = events::notice_event(n) {
                    if ev.kind == events::Kind::Subagent {
                        flush(&mut out, &mut group, &mut at, &mut key);
                        out.push(Block::Subagent { key: format!("a-{}", n.id), step: Step::Notice(n) });
                    } else if !ev.failed && launched(&group, &ev) {
                        group.push(Step::Notice(n));
                        at.push(Loc { entry: mi as u32, part: Loc::NOTICE, n: 0 });
                    } else {
                        flush(&mut out, &mut group, &mut at, &mut key);
                        push_event(&mut out, format!("c-ev-{}", n.id), Step::Notice(n));
                    }
                    continue;
                }
                if all_thoughts(&group) {
                    flush(&mut out, &mut group, &mut at, &mut key);
                }
                if group.is_empty() {
                    key = Some(format!("c-{}", n.id));
                }
                group.push(Step::Notice(n));
                at.push(Loc { entry: mi as u32, part: Loc::NOTICE, n: 0 });
                continue;
            }
            Entry::Skip | Entry::Idle => continue,
            Entry::Step(a) => *a,
        };
        let live_msg = live && Some(mi) == last;
        // Keys count each kind apart, as the server's ordinals do: a thought arriving or
        // showing up in a refetched copy doesn't re-key the answer beside it.
        let (mut texts, mut thoughts) = (0, 0);
        for (pi, part) in a.parts.iter().enumerate() {
            let is_last = live_msg && pi + 1 == a.parts.len();
            // The group's key is its first step's: only that one formats it.
            let fold_in = |group: &mut Vec<Step<'a>>, at: &mut Vec<Loc>, key: &mut Option<String>, first: &dyn Fn() -> String, step: Step<'a>, n: usize| {
                if group.is_empty() {
                    *key = Some(first());
                }
                group.push(step);
                at.push(Loc { entry: mi as u32, part: pi as u32, n: n as u32 });
            };
            match part {
                Part::Text { text } => {
                    let n = texts;
                    texts += 1;
                    // A live, still-empty answer keeps its row so the stream reveals into it.
                    if text.trim().is_empty() && !is_last {
                        continue;
                    }
                    let placement = if commentary.binary_search(&(mi, pi)).is_ok() { detail.placement(Category::Commentary) } else { Placement::Separate };
                    match placement {
                        Placement::Hidden => {}
                        Placement::Grouped => fold_in(&mut group, &mut at, &mut key, &|| format!("c-{}-t{n}", a.id), Step::Text { body: text }, 0),
                        Placement::Separate => {
                            flush(&mut out, &mut group, &mut at, &mut key);
                            out.push(Block::Text { key: format!("t-{}-{n}", a.id), body: text, msg: mi, part: pi, live: is_last });
                        }
                    }
                }
                Part::Reasoning { text, started, completed } => {
                    let n = thoughts;
                    thoughts += 1;
                    if detail.placement(Category::Thinking) == Placement::Hidden {
                        continue;
                    }
                    let secs = completed.map(|c| ((c - started) as f64 / 1000.0).round().max(0.0) as i64);
                    let step = Step::Thought { msg: &a.id, index: n, body: text, secs, started: *started };
                    // The thought the model is writing stands at the end of the turn, even before
                    // its first words (`textShows`, `stepEvent`'s `reasoning`).
                    if is_last && completed.is_none() {
                        flush(&mut out, &mut group, &mut at, &mut key);
                        out.push(Block::Thought { key: format!("th-{}-{n}", a.id), step, live: true });
                        continue;
                    }
                    if text.trim().is_empty() {
                        continue;
                    }
                    match detail.placement(Category::Thinking) {
                        Placement::Grouped => {
                            if all_notices(&group) {
                                flush(&mut out, &mut group, &mut at, &mut key);
                            }
                            fold_in(&mut group, &mut at, &mut key, &|| format!("c-{}-{n}", a.id), step, n)
                        }
                        _ => {
                            flush(&mut out, &mut group, &mut at, &mut key);
                            out.push(Block::Thought { key: format!("th-{}-{n}", a.id), step, live: false });
                        }
                    }
                }
                Part::Tool { id, name, title, status, input, files, metadata } => {
                    let step = Step::Tool { id, name, title, status, files, input, metadata };
                    let rule = rules::rule(name, input);
                    // A call stopped before the model finished writing it did nothing.
                    let empty = input.as_object().is_none_or(|o| o.is_empty());
                    if rule.hidden || (rules::stopped(status, metadata) && empty) {
                        continue;
                    }
                    let failed = rules::failed(name, status, metadata);
                    // A widget shows once the model wrote its call.
                    if name == "widget" {
                        if !matches!(status, ToolStatus::Pending) {
                            flush(&mut out, &mut group, &mut at, &mut key);
                            out.push(Block::Widget { key: format!("wg-{}-{id}", a.id), message: &a.id, step });
                        }
                        continue;
                    }
                    // Working on a widget template shows as one quiet line of its own, as the TUI's.
                    if name == "widget_template" && !failed {
                        flush(&mut out, &mut group, &mut at, &mut key);
                        out.push(Block::Calls { key: format!("p-{id}"), kind: Calls::One, steps: vec![step] });
                        continue;
                    }
                    if rule.alone {
                        if !matches!(status, ToolStatus::Pending | ToolStatus::Running) {
                            flush(&mut out, &mut group, &mut at, &mut key);
                            out.push(Block::Question { id, status, input, metadata });
                        }
                        continue;
                    }
                    let placement = detail.placement(rule.category);
                    // Every subagent event is its own row, wherever its setting places it, unless
                    // hidden: they are never counted (`contentGroup`'s `subagent`).
                    if rule.category == Category::Subagents {
                        if placement != Placement::Hidden {
                            flush(&mut out, &mut group, &mut at, &mut key);
                            out.push(Block::Subagent { key: format!("a-{id}"), step });
                        }
                        continue;
                    }
                    match placement {
                        // A failure shows only where its category does (`contentShows`).
                        Placement::Hidden => {}
                        // Work that runs on joins the activity group where its setting groups
                        // it; a launch that failed keeps its own row, as failures do.
                        Placement::Grouped if rule.layout != rules::Layout::Event || !failed => fold_in(&mut group, &mut at, &mut key, &|| format!("c-{id}"), step, 0),
                        _ if rule.layout == rules::Layout::Event => {
                            flush(&mut out, &mut group, &mut at, &mut key);
                            push_event(&mut out, format!("c-ev-{id}"), step);
                        }
                        _ => {
                            flush(&mut out, &mut group, &mut at, &mut key);
                            let kind = if grouped_read(&step) {
                                Calls::Reads
                            } else if !failed && file_tool(&step) {
                                Calls::Files
                            } else {
                                Calls::One
                            };
                            match out.last_mut() {
                                Some(Block::Calls { kind: k, steps, .. }) if *k == kind && kind != Calls::One => steps.push(step),
                                _ => out.push(Block::Calls { key: format!("p-{id}"), kind, steps: vec![step] }),
                            }
                        }
                    }
                }
            }
        }
    }
    flush(&mut out, &mut group, &mut at, &mut key);
    out
}

/// The turn's commentary (`turnCommentary`, `session-ui/src/timeline/commentary.ts`,
/// anomalyco/opencode#64917), by entry and part, run by run: a prompt starts a turn, but wake-ups
/// from watches, subagents and jobs add idle-bounded runs to it, and each run keeps its own answer
/// out of the activity group. A run an idle marker closed is settled; the turn's last run is
/// settled once the turn is (`settled`). Sorted, for `binary_search`.
fn commentary(entries: &[Entry], settled: bool) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let mut from = 0;
    for (i, e) in entries.iter().enumerate() {
        if matches!(e, Entry::Idle) {
            run_commentary(entries, from..i, true, &mut out);
            from = i + 1;
        }
    }
    run_commentary(entries, from..entries.len(), settled, &mut out);
    out
}

/// One run's commentary (`runCommentary`): provider `native.phase` labels take precedence over
/// tool order (`commentary` always groups, `final_answer` never groups); otherwise text before
/// the run's last tool call is commentary. A `settled` run whose every visible text would be
/// commentary keeps its last as the answer, so an interrupted or tool-ending run still leaves
/// something to read.
fn run_commentary(entries: &[Entry], run: std::ops::Range<usize>, settled: bool, out: &mut Vec<(usize, usize)>) {
    use opencode_kit::model::TextPhase;
    let parts = || entries[run.clone()].iter().zip(run.clone()).flat_map(|(e, mi)| {
        let step = match e {
            Entry::Step(a) => Some(*a),
            Entry::Notice(_) | Entry::Skip | Entry::Idle => None,
        };
        step.into_iter().flat_map(move |a| a.parts.iter().enumerate().map(move |(pi, p)| ((mi, pi), p, a.text_phase(pi))))
    });
    let last_tool = parts().rfind(|(_, p, _)| matches!(p, Part::Tool { .. })).map(|(at, _, _)| at);
    let start = out.len();
    for (at, p, phase) in parts() {
        if !matches!(p, Part::Text { .. }) {
            continue;
        }
        let is_commentary = phase == Some(TextPhase::Commentary)
            || (phase != Some(TextPhase::FinalAnswer) && last_tool.is_some_and(|lt| at < lt));
        if is_commentary {
            out.push(at);
        }
    }
    if start == out.len() {
        return;
    }
    let slice = &out[start..];
    let has_answer = parts().any(|(at, p, _)| {
        matches!(p, Part::Text { text } if !text.trim().is_empty()) && slice.binary_search(&at).is_err()
    });
    if settled && !has_answer {
        let last_visible = parts().rfind(|(_, p, _)| matches!(p, Part::Text { text } if !text.trim().is_empty())).map(|(at, _, _)| at);
        if let Some(answer) = last_visible
            && let Some(i) = out[start..].iter().position(|t| *t == answer)
        {
            out.remove(start + i);
        }
    }
}

/// A tool's name in a group's label (`toolTitle`).
pub fn tool_title(name: &str) -> &str {
    use opencode_i18n::t;
    match name {
        "read" => t!("ui.tool.read"),
        "list" => t!("ui.tool.list"),
        "glob" => t!("ui.tool.glob"),
        "grep" => t!("ui.tool.grep"),
        "browser" => t!("ui.tool.browser"),
        "webfetch" => t!("ui.tool.webfetch"),
        "websearch" => t!("ui.tool.websearch"),
        "subagent" | "task" => t!("ui.tool.agent.default"),
        "shell" | "bash" => t!("ui.tool.shell"),
        "execute" => t!("ui.tool.execute"),
        "edit" => t!("ui.messagePart.title.edit"),
        "write" => t!("ui.messagePart.title.write"),
        "patch" | "apply_patch" => t!("ui.tool.patch"),
        "question" => t!("ui.tool.questions"),
        "skill" => t!("ui.tool.skill"),
        _ => name,
    }
}

/// The label of a group with no calls and no work that runs on (`usedLabel`):
/// `ui.messagePart.tools.used` ("Used {count} {title}") over its thoughts, split around its
/// count and title as Desktop splits it, or "Updates" alone for notices and commentary (in
/// `before`).
#[derive(Debug, PartialEq)]
pub struct Label {
    pub count: Option<usize>,
    pub title: String,
    /// The words before the count, between it and the title, and after the title.
    pub before: String,
    pub between: String,
    pub after: String,
    /// The whole label, as its button names it.
    pub text: String,
}

impl Label {
    pub fn text(&self) -> String {
        self.text.clone()
    }

    /// `ui.messagePart.tools.used` for `count` and `title`, cut where its count and title stand.
    fn used(count: usize, title: String) -> Label {
        let text = opencode_i18n::plural!("ui.messagePart.tools.used", count, tools = title);
        let at = text.find(&title).unwrap_or(text.len());
        let before = text[..at].trim();
        let after = text.get(at + title.len()..).unwrap_or("").trim().to_owned();
        let digits = count.to_string();
        match before.find(&digits) {
            Some(n) => Label { count: Some(count), before: before[..n].trim().to_owned(), between: before[n + digits.len()..].trim().to_owned(), after, title, text },
            None => Label { count: None, before: before.to_owned(), between: String::new(), after, title, text },
        }
    }
}

/// "Used 2 Thoughts", or "Updates" (`usedLabel`).
pub fn label(steps: &[Step]) -> Label {
    let thoughts = steps.iter().filter(|s| matches!(s, Step::Thought { .. })).count();
    if thoughts == 0 {
        let updates = opencode_i18n::t!("ui.messagePart.context.updates");
        return Label { count: None, title: String::new(), before: updates.into(), between: String::new(), after: String::new(), text: updates.into() };
    }
    Label::used(thoughts, opencode_i18n::plural!("ui.messagePart.context.thought", thoughts))
}

/// How long something took, as Desktop counts it (`ui.message.duration.*`): "3s",
/// "1m 5s", "1h 2m".
pub fn elapsed(ms: i64) -> String {
    use opencode_i18n::{integer, t};
    let total = (ms / 1000).max(0);
    if total < 60 {
        return t!("ui.message.duration.seconds", count = integer(total));
    }
    if total < 3600 {
        return t!("ui.message.duration.minutesSeconds", minutes = integer(total / 60), seconds = integer(total % 60));
    }
    t!("ui.message.duration.hoursMinutes", hours = integer(total / 3600), minutes = integer(total / 60 % 60))
}

/// Step `st` as a tool call, when it is one.
fn call<'a>(st: &Step<'a>) -> Option<crate::tools::Call<'a>> {
    match st {
        Step::Tool { id, name, status, input, metadata, .. } => Some(crate::tools::Call { id, name, status, input, metadata }),
        Step::Thought { .. } | Step::Notice(_) | Step::Text { .. } => None,
    }
}

/// A read that shares a line with the reads beside it: not failed, not an image, and loading no
/// instructions (`currentToolGroupedRead`).
fn grouped_read(st: &Step) -> bool {
    let Step::Tool { name: "read", status, input, metadata, .. } = st else { return false };
    let image = crate::tools::path(input).is_some_and(|p| {
        let p = p.to_ascii_lowercase();
        [".png", ".jpg", ".jpeg", ".gif", ".webp", ".svg", ".avif", ".bmp", ".ico"].iter().any(|e| p.ends_with(e))
    });
    let loaded = metadata["loaded"].as_array().is_some_and(|a| a.iter().any(|v| v.is_string()));
    **status != ToolStatus::Error && !image && !loaded
}


/// An edit, write or patch that didn't fail: it joins the file card of the steps beside it.
fn file_tool(st: &Step) -> bool {
    call(st).is_some_and(|c| crate::tools::is_file_tool(&c))
}

/// A skill call with a name that didn't fail: it joins the "Loaded" line of the skills beside it.
fn named_skill(st: &Step) -> bool {
    matches!(st, Step::Tool { name: "skill", status, .. } if **status != ToolStatus::Error) && call(st).is_some_and(|c| crate::tools::skill_name(&c).is_some())
}
