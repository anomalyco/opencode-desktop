//! A turn's work folds into one row, as Desktop's default ("compact") timeline does it:
//! `session-ui/src/timeline/projection.ts` (`groupContent`, `toolGroupType`, `renderable`) picks
//! what folds, and `session-ui/src/tools/tool-renderer.tsx` (`CurrentContextToolGroup`) labels it.
//!
//! Thoughts and tool calls fold together across a turn's steps, up to the next visible text.
//! Text, subagents and answered questions stay in the timeline; to-dos, empty parts and questions
//! still waiting on an answer don't show. A group reads "Used 5 Read, Shell": the tool count and
//! each tool's name once. A group of thoughts alone reads "Used 1 Thought", or "Thinking" while
//! it is the live end of the turn.

use rune::{El, col, el, row, text};

use opencode_kit::model::{Assistant, Attachment, Part, ToolStatus};

use crate::TimelineHost;
use crate::odometer::odometer;
use crate::palette::Pal;
use crate::style::{HOVER_FADE, UI, UI_MED, arrow, reasoning};

/// One step inside a group.
#[derive(Debug)]
pub enum Step<'a> {
    Thought { msg: &'a str, index: usize, body: &'a str, secs: Option<i64>, started: i64 },
    Tool { id: &'a str, name: &'a str, title: &'a str, status: &'a ToolStatus, files: &'a [Attachment], input: &'a serde_json::Value, metadata: &'a serde_json::Value },
}

/// One timeline row of a turn's assistant work.
#[derive(Debug)]
pub enum Block<'a> {
    /// Folded thoughts and tool calls.
    Group { key: String, steps: Vec<Step<'a>> },
    Text { key: String, body: &'a str, msg: usize, part: usize, live: bool },
    /// A subagent call. `session`: its child session, once the call names it.
    Agent { id: &'a str, agent: &'a str, title: &'a str, status: &'a ToolStatus, session: Option<&'a str>, metadata: &'a serde_json::Value },
    /// A question call, once answered, dismissed or failed (`question_row`).
    Question { id: &'a str, status: &'a ToolStatus, input: &'a serde_json::Value, metadata: &'a serde_json::Value },
}

/// Folds consecutive assistant messages (a turn's steps) into rows. `live`: the last one is
/// still streaming.
pub fn fold<'a>(steps: &[&'a Assistant], live: bool) -> Vec<Block<'a>> {
    let mut out = Vec::new();
    let mut group: Vec<Step<'a>> = Vec::new();
    let mut key: Option<(&'a str, Option<usize>)> = None;
    let flush = |out: &mut Vec<Block<'a>>, group: &mut Vec<Step<'a>>, key: &mut Option<(&'a str, Option<usize>)>| {
        if !group.is_empty() {
            let key = match key.take() {
                Some((id, Some(n))) => format!("c-{id}-{n}"),
                Some((id, None)) => format!("c-{id}"),
                None => String::from("c-"),
            };
            out.push(Block::Group { key, steps: std::mem::take(group) });
        }
    };
    for (mi, a) in steps.iter().enumerate() {
        let live_msg = live && mi + 1 == steps.len();
        // Keys count each kind apart, as the server's ordinals do: a thought arriving or
        // showing up in a refetched copy doesn't re-key the answer beside it.
        let (mut texts, mut thoughts) = (0, 0);
        for (pi, part) in a.parts.iter().enumerate() {
            let is_last = live_msg && pi + 1 == a.parts.len();
            let mut fold_in = |group: &mut Vec<Step<'a>>, first: (&'a str, Option<usize>), step: Step<'a>| {
                if group.is_empty() {
                    key = Some(first);
                }
                group.push(step);
            };
            match part {
                Part::Text { text } => {
                    let n = texts;
                    texts += 1;
                    // A live, still-empty answer keeps its row so the stream reveals into it.
                    if text.trim().is_empty() && !is_last {
                        continue;
                    }
                    flush(&mut out, &mut group, &mut key);
                    out.push(Block::Text { key: format!("t-{}-{n}", a.id), body: text, msg: mi, part: pi, live: is_last });
                }
                Part::Reasoning { text, started, completed } => {
                    let n = thoughts;
                    thoughts += 1;
                    if text.trim().is_empty() {
                        continue;
                    }
                    let secs = completed.map(|c| ((c - started) as f64 / 1000.0).round().max(0.0) as i64);
                    fold_in(&mut group, (&a.id, Some(n)), Step::Thought { msg: &a.id, index: n, body: text, secs, started: *started });
                }
                Part::Tool { id, name, title, status, input, files, metadata } => match name.as_str() {
                    "todowrite" => {}
                    "task" | "subagent" => {
                        flush(&mut out, &mut group, &mut key);
                        // `agent` today; older servers' `task` calls said `subagent_type`.
                        let agent = input["agent"].as_str().or(input["subagent_type"].as_str()).unwrap_or("general");
                        let session = opencode_kit::model::subagent_session(input, metadata);
                        out.push(Block::Agent { id, agent, title, status, session, metadata });
                    }
                    "question" => {
                        if !matches!(status, ToolStatus::Pending | ToolStatus::Running) {
                            flush(&mut out, &mut group, &mut key);
                            out.push(Block::Question { id, status, input, metadata });
                        }
                    }
                    _ => fold_in(&mut group, (id, None), Step::Tool { id, name, title, status, files, input, metadata }),
                },
            }
        }
    }
    flush(&mut out, &mut group, &mut key);
    out
}

/// A tool's name in a group's label (`toolTitle`).
pub fn tool_title(name: &str) -> &str {
    match name {
        "read" => "Read",
        "list" => "List",
        "glob" => "Glob",
        "grep" => "Grep",
        "browser" => "Browser",
        "webfetch" => "Webfetch",
        "websearch" => "Web Search",
        "subagent" | "task" => "Agent",
        "shell" | "bash" => "Shell",
        "execute" => "Execute",
        "edit" => "Edit",
        "write" => "Write",
        "patch" | "apply_patch" => "Patch",
        "question" => "Questions",
        "skill" => "Skill",
        _ => name,
    }
}

/// A group's label: "Used {count} {title}", or "Thinking" (`count` is `None`).
#[derive(Debug, PartialEq)]
pub struct Label {
    pub count: Option<usize>,
    pub title: String,
}

impl Label {
    pub fn text(&self) -> String {
        match self.count {
            Some(n) => format!("Used {n} {}", self.title),
            None => self.title.clone(),
        }
    }
}

/// `busy`: the group is the live end of the turn.
pub fn label(steps: &[Step], busy: bool) -> Label {
    let mut names: Vec<&str> = Vec::new();
    let mut tools = 0;
    for s in steps {
        if let Step::Tool { name, .. } = s {
            tools += 1;
            let t = tool_title(name);
            if !names.contains(&t) {
                names.push(t);
            }
        }
    }
    let thoughts = steps.len() - tools;
    // Thoughts alone aren't finished work yet: the live group reads Thinking, not "Used 1 Thought".
    if busy && tools == 0 {
        return Label { count: None, title: "Thinking".into() };
    }
    if tools == 0 {
        return Label { count: Some(thoughts), title: if thoughts == 1 { "Thought" } else { "Thoughts" }.into() };
    }
    // English lists join with commas only ("Read, Search, List").
    Label { count: Some(tools), title: names.join(", ") }
}

/// When the first thought of a group that reads Thinking started (ms), for its ticking time.
pub fn thinking_since(steps: &[Step], busy: bool) -> Option<i64> {
    if label(steps, busy).count.is_some() {
        return None;
    }
    steps.iter().find_map(|st| match st {
        Step::Thought { started, .. } => Some(*started),
        Step::Tool { .. } => None,
    })
}

/// When the live turn's Thinking group started thinking (ms): the transcript is busy and its
/// last group is thoughts alone.
pub fn live_thinking_since(t: &opencode_kit::model::Transcript) -> Option<i64> {
    if !t.busy() {
        return None;
    }
    let steps: Vec<&Assistant> = t.messages.iter().rev()
        .map_while(|m| match m { opencode_kit::model::Message::Assistant(a) => Some(a), _ => None })
        .collect::<Vec<_>>().into_iter().rev().collect();
    match fold(&steps, true).last()? {
        Block::Group { steps, .. } => thinking_since(steps, true),
        _ => None,
    }
}

/// How long a group has been thinking, as Desktop counts it (`ui.message.duration.*`): "3s",
/// "1m 5s", "1h 2m".
pub fn elapsed(ms: i64) -> String {
    let total = (ms / 1000).max(0);
    if total < 60 {
        return format!("{total}s");
    }
    if total < 3600 {
        return format!("{}m {}s", total / 60, total % 60);
    }
    format!("{}h {}m", total / 3600, total / 60 % 60)
}

/// Whole seconds a group started at `since` has been thinking at `now_ms`.
pub fn thinking_secs(since: i64, now_ms: i64) -> i64 {
    ((now_ms - since) / 1000).max(0)
}

thread_local! {
    /// The open session's thinking time last drawn: (session, start, seconds).
    static TICKED: std::cell::RefCell<Option<(String, i64, i64)>> = const { std::cell::RefCell::new(None) };
}

/// Redraws the open session's Thinking time when its second changes and wakes for the next
/// one; nothing wakes once the turn stops thinking.
pub fn poll<S: TimelineHost>(s: &S, tx: &mut rune::Tx) -> bool {
    let now_ms = s.now_ms();
    let live = s.active_session_id().filter(|id| !s.is_session_stopped(id) && s.session_revert(id).is_none()).and_then(|id| {
        let since = s.with_transcript(id, live_thinking_since).flatten()?;
        Some((id.to_owned(), since, thinking_secs(since, now_ms)))
    });
    if let Some((_, since, secs)) = &live {
        // Just past the next whole second.
        let next = since + (secs + 1) * 1000 - now_ms;
        tx.wake_after(std::time::Duration::from_millis(next.max(1) as u64));
    }
    TICKED.with_borrow_mut(|shown| (shown.as_ref() != live.as_ref()).then(|| *shown = live).is_some())
}

/// Step `st` as a tool call, when it is one.
fn call<'a>(st: &Step<'a>) -> Option<crate::tools::Call<'a>> {
    match st {
        Step::Tool { id, name, status, input, metadata, .. } => Some(crate::tools::Call { id, name, status, input, metadata }),
        Step::Thought { .. } => None,
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

/// The folded row: its label, and when open, every step in order, inset 24 behind a hairline
/// rail (`collapsed-tool-group`), 8 under the label and 8 apart. Plain reads in a row share a
/// line, edits, writes and patches in a row share one card of files, and named skills in a row
/// share one "Loaded" line, as Desktop's `CurrentContextToolGroup` merges them. The row springs
/// open and shut on the timeline's growth with its header held in place
/// (`disclosure::toggle_group`), and while open the header sticks to the timeline's top until
/// the group scrolls past. `busy`: the group is the live end of the turn; `live`: the turn is
/// still running, so a step still under way shimmers.
#[allow(clippy::too_many_arguments)]
pub fn group<S: TimelineHost>(s: &S, p: &Pal, session: &str, key: &str, steps: &[Step], busy: bool, live: bool, fade: f32) -> El {
    let open = s.expanded().contains(key);
    let shutting = !open && crate::disclosure::is_shutting(s, key);
    let l = label(steps, busy);
    let mut trigger = row()
        .key("trigger")
        .role("button")
        .label(l.text())
        .h(28.0)
        .gap(4.0)
        .center_y()
        .hover_group()
        // The arrow fades in under the pointer (`collapsible.css:26-29`).
        .hover_motion(HOVER_FADE, HOVER_FADE)
        // A step joining or the label changing crossfades the words while the arrow slides over.
        .animate_layout()
        .on_press_tx(crate::disclosure::toggle_group::<S>(key.to_owned()));
    if open || shutting {
        // Steps scroll under it and soften into its fill rather than meeting a hard edge, and
        // shutting from there they slide away under it.
        let soft = rune::Shadow { y: 10.0, blur: 20.0, color: p.bg_base, ..Default::default() };
        trigger = trigger.fill(p.bg_base).sticky_top(0.0).stuck_shadow(soft);
    }
    trigger = match l.count {
        Some(n) => trigger
            .child(text("Used").key("used").font(UI).color(p.muted))
            .child(odometer(format!("{key}-count"), n.to_string(), n as f64, UI, p.text))
            .child(text(l.title.clone()).key(format!("title-{}", l.title)).font(UI_MED).color(p.text).truncate_fade()),
        // Thoughts alone, still live: Thinking shimmers in the title's 13/530 (`tool-renderer.tsx:634-636`).
        // Its time ticks beside it in the subtitle's muted 13/440 (`tool-renderer.tsx:640`).
        None => trigger.child(crate::working::shimmer(s, p, &l.title, UI_MED, 0.0).key("thinking")).children(
            thinking_since(steps, busy).map(|at| text(elapsed(s.now_ms() - at)).key("elapsed").font(UI.features("'tnum' 1")).color(p.muted)),
        ),
    };
    // The arrow shows while the pointer is over the label, and always while open.
    let a = el().key("arrow").w(16.0).h(24.0).ml(2.0).center().child(arrow(p, open));
    trigger = trigger.child(if open { a } else { a.group_opacity(0.0, 1.0) });
    let mut c = col().w_full().child(trigger);
    if open || shutting {
        let cx = crate::tools::Ctx { session, live };
        // The row's clip reveals the steps as it grows, so they don't fade in as well.
        // 8 under the label (inside the steps, so a shut box is no taller than nothing), then the
        // rail 6 in from the steps' top and bottom.
        let mut list = col().key("steps").w_full().pt(8.0).pl(12.0).gap(8.0).transition(rune::Transition::none());
        list = list.child(el().key("rail").pin(Some(-0.25), Some(14.0), None, Some(6.0)).w(0.5).fill(p.border));
        let mut i = 0;
        while i < steps.len() {
            let run = |f: fn(&Step) -> bool| steps[i..].iter().take_while(|st| f(st)).count();
            let (n, item) = match &steps[i] {
                Step::Thought { msg, index, body, secs, .. } => {
                    let key = format!("r-{msg}-{index}");
                    (1, reasoning::<S>(s, p, &key, body, *secs, live, s.expanded().contains(&key), fade).key(key))
                }
                st @ Step::Tool { .. } if grouped_read(st) => {
                    let calls: Vec<crate::tools::Call> = steps[i..i + run(grouped_read)].iter().filter_map(call).collect();
                    let el = crate::tools::cached(s, p, cx, &calls, || crate::tools::read_group(s, p, cx, &calls));
                    (calls.len(), el.key(format!("reads-{}", calls[0].id)))
                }
                st @ Step::Tool { .. } if file_tool(st) => {
                    let calls: Vec<crate::tools::Call> = steps[i..i + run(file_tool)].iter().filter_map(call).collect();
                    let el = crate::tools::cached(s, p, cx, &calls, || crate::tools::file_group(s, p, cx, &calls));
                    (calls.len(), col().key(format!("files-{}", calls[0].id)).w_full().child(el))
                }
                st @ Step::Tool { .. } if named_skill(st) => {
                    let calls: Vec<crate::tools::Call> = steps[i..i + run(named_skill)].iter().filter_map(call).collect();
                    let el = crate::tools::cached(s, p, cx, &calls, || crate::tools::skill_group(s, p, cx, &calls));
                    (calls.len(), col().key(format!("skills-{}", calls[0].id)).w_full().child(el))
                }
                st @ Step::Tool { id, files, .. } => {
                    let step = col().key(format!("step-{id}")).w_full().child(crate::tools::view(s, p, cx, &call(st).expect("a tool step")));
                    (1, step.children(crate::images::gallery(s, p, "step-images", files, false).map(|g| g.pl(22.0))))
                }
            };
            list = list.child(item);
            i += n;
        }

        // Shutting, the steps stay laid out below a row that's only the header, and the row's
        // clip folds them away as its height springs down to it.
        // The box keeps the steps' path the same either way, so they don't remount on a press.
        let held = col().key("steps-box").w_full().pl(12.0).transition(rune::Transition::none());
        c = c.child(if open { held } else { held.h(0.0) }.child(list));
    }
    c
}

/// An edit, write or patch that didn't fail: it joins the file card of the steps beside it.
fn file_tool(st: &Step) -> bool {
    call(st).is_some_and(|c| crate::tools::is_file_tool(&c))
}

/// A skill call with a name that didn't fail: it joins the "Loaded" line of the skills beside it.
fn named_skill(st: &Step) -> bool {
    matches!(st, Step::Tool { name: "skill", status, .. } if **status != ToolStatus::Error) && call(st).is_some_and(|c| crate::tools::skill_name(&c).is_some())
}
#[cfg(test)]
mod tests {
    use super::*;

    fn tool(id: &str, name: &str, status: ToolStatus) -> Part {
        Part::Tool { id: id.into(), name: name.into(), status, title: String::new(), input: serde_json::Value::Null, files: vec![], metadata: serde_json::Value::Null }
    }
    fn thought(body: &str) -> Part {
        Part::Reasoning { text: body.into(), started: 0, completed: Some(2_000) }
    }
    fn say(body: &str) -> Part {
        Part::Text { text: body.into() }
    }
    fn step(id: &str, parts: Vec<Part>) -> Assistant {
        Assistant { id: id.into(), parts, ..Default::default() }
    }
    fn shape(blocks: &[Block]) -> Vec<String> {
        blocks
            .iter()
            .map(|b| match b {
                Block::Group { key, steps } => format!("{key}: {}", label(steps, false).text()),
                Block::Text { body, .. } => format!("text {body}"),
                Block::Agent { agent, .. } => format!("agent {agent}"),
                Block::Question { id, .. } => format!("question {id}"),
            })
            .collect()
    }

    #[test]
    fn a_turns_steps_fold_into_one_row() {
        // Kit's capture: thought, 3 tools, thought, 1 tool, thought, 1 tool, across three steps.
        let a = step("m1", vec![thought("look"), tool("c1", "read", ToolStatus::Completed), tool("c2", "read", ToolStatus::Completed), tool("c3", "shell", ToolStatus::Completed)]);
        let b = step("m2", vec![thought("hmm"), tool("c4", "read", ToolStatus::Completed)]);
        let c = step("m3", vec![thought("ok"), tool("c5", "shell", ToolStatus::Running)]);
        let blocks = fold(&[&a, &b, &c], true);
        assert_eq!(shape(&blocks), ["c-m1-0: Used 5 Read, Shell"]);
        let Block::Group { steps, .. } = &blocks[0] else { unreachable!() };
        assert_eq!(steps.len(), 8);
        // Still the live end of the turn: tools make it "Used", not "Thinking".
        assert_eq!(label(steps, true).text(), "Used 5 Read, Shell");
    }

    #[test]
    fn text_splits_groups_and_stays_visible() {
        let a = step("m1", vec![thought("plan"), say("I'll look."), tool("g", "grep", ToolStatus::Completed), tool("r", "read", ToolStatus::Completed)]);
        let b = step("m2", vec![say("Found it."), tool("e", "edit", ToolStatus::Error), tool("e2", "edit", ToolStatus::Completed)]);
        let c = step("m3", vec![say("Done.")]);
        assert_eq!(
            shape(&fold(&[&a, &b, &c], false)),
            ["c-m1-0: Used 1 Thought", "text I'll look.", "c-g: Used 2 Grep, Read", "text Found it.", "c-e: Used 2 Edit", "text Done."]
        );
    }

    #[test]
    fn an_answer_keeps_its_key_when_a_thought_joins_before_it() {
        let keys = |a: &Assistant| fold(&[a], false).iter().map(|b| match b {
            Block::Group { key, .. } | Block::Text { key, .. } => key.clone(),
            _ => String::new(),
        }).collect::<Vec<_>>();
        let live = step("m1", vec![say("Done.")]);
        let stored = step("m1", vec![thought("look"), say("Done.")]);
        assert_eq!(keys(&live), ["t-m1-0"]);
        assert_eq!(keys(&stored), ["c-m1-0", "t-m1-0"]);
    }

    #[test]
    fn subagents_and_answered_questions_stand_alone() {
        let a = step(
            "m1",
            vec![
                tool("r", "read", ToolStatus::Completed),
                tool("t", "task", ToolStatus::Running),
                tool("todo", "todowrite", ToolStatus::Completed),
                tool("g", "glob", ToolStatus::Completed),
                tool("q1", "question", ToolStatus::Running),
                tool("w", "write", ToolStatus::Completed),
                tool("q2", "question", ToolStatus::Completed),
                tool("x", "mcp_thing", ToolStatus::Completed),
            ],
        );
        assert_eq!(
            shape(&fold(&[&a], false)),
            ["c-r: Used 1 Read", "agent general", "c-g: Used 2 Glob, Write", "question q2", "c-x: Used 1 mcp_thing"]
        );
    }

    #[test]
    fn empty_parts_hide_and_live_thoughts_read_thinking() {
        let a = step("m1", vec![thought("  "), say(""), thought("first"), thought("second")]);
        let blocks = fold(&[&a], false);
        let Block::Group { key, steps } = &blocks[0] else { panic!("{blocks:?}") };
        assert_eq!(key, "c-m1-1");
        assert_eq!(label(steps, false).text(), "Used 2 Thoughts");
        assert_eq!(label(steps, true), Label { count: None, title: "Thinking".into() });
        // A live answer that hasn't streamed yet keeps its row.
        let b = step("m2", vec![tool("r", "read", ToolStatus::Completed), say("")]);
        assert_eq!(shape(&fold(&[&b], true)), ["c-r: Used 1 Read", "text "]);
    }
}
