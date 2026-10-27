//! A turn's work folds into one row, as Desktop's default ("compact") timeline does it:
//! `session-ui/src/timeline/projection.ts` (`groupContent`, `toolGroupType`, `renderable`) picks
//! what folds, and `session-ui/src/tools/tool-renderer.tsx` (`CurrentContextToolGroup`) labels it.
//!
//! Thoughts and tool calls fold together across a turn's steps, up to the next visible text.
//! Text, subagents and answered questions stay in the timeline; to-dos, empty parts and questions
//! still waiting on an answer don't show. A group reads "Used 5 Read, Shell": the tool count and
//! each tool's name once. A group of thoughts alone reads "Used 1 Thought", or "Thinking" while
//! it is the live end of the turn.

use rune::{El, col, icon, row, text};

use opencode_kit::model::{Assistant, Attachment, Part, ToolStatus};
use rune_theme_opencode::icons as ic;

use crate::App;
use crate::palette::Pal;
use crate::view::{UI, UI_MED, chevron, reasoning};

/// One step inside a group.
#[derive(Debug)]
pub enum Step<'a> {
    Thought { key: String, body: &'a str, secs: Option<i64> },
    Tool { id: &'a str, name: &'a str, title: &'a str, status: &'a ToolStatus, files: &'a [Attachment] },
}

/// One timeline row of a turn's assistant work.
#[derive(Debug)]
pub enum Block<'a> {
    /// Folded thoughts and tool calls.
    Group { key: String, steps: Vec<Step<'a>> },
    Text { key: String, body: &'a str, live: bool },
    /// A subagent call. `session`: its child session, once the call names it. `background`: the
    /// call returned at once and the child runs on.
    Agent { id: &'a str, agent: &'a str, title: &'a str, status: &'a ToolStatus, session: Option<&'a str>, background: bool },
    /// A tool that shows on its own (an answered question).
    Tool(Step<'a>),
}

/// Folds consecutive assistant messages (a turn's steps) into rows. `live`: the last one is
/// still streaming.
pub fn fold<'a>(steps: &[&'a Assistant], live: bool) -> Vec<Block<'a>> {
    let mut out = Vec::new();
    let mut group: Vec<Step<'a>> = Vec::new();
    let mut key = String::new();
    let flush = |out: &mut Vec<Block<'a>>, group: &mut Vec<Step<'a>>, key: &str| {
        if !group.is_empty() {
            out.push(Block::Group { key: format!("c-{key}"), steps: std::mem::take(group) });
        }
    };
    for (mi, a) in steps.iter().enumerate() {
        let live_msg = live && mi + 1 == steps.len();
        for (pi, part) in a.parts.iter().enumerate() {
            let is_last = live_msg && pi + 1 == a.parts.len();
            let mut fold_in = |group: &mut Vec<Step<'a>>, first: String, step: Step<'a>| {
                if group.is_empty() {
                    key = first;
                }
                group.push(step);
            };
            match part {
                Part::Text { text } => {
                    // A live, still-empty answer keeps its row so the stream reveals into it.
                    if text.trim().is_empty() && !is_last {
                        continue;
                    }
                    flush(&mut out, &mut group, &key);
                    out.push(Block::Text { key: format!("t-{}-{pi}", a.id), body: text, live: is_last });
                }
                Part::Reasoning { text, started, completed } => {
                    if text.trim().is_empty() {
                        continue;
                    }
                    let secs = completed.map(|c| ((c - started) as f64 / 1000.0).round().max(1.0) as i64);
                    let k = format!("{}-{pi}", a.id);
                    fold_in(&mut group, k.clone(), Step::Thought { key: format!("r-{k}"), body: text, secs });
                }
                Part::Tool { id, name, title, status, input, files, metadata } => match name.as_str() {
                    "todowrite" => {}
                    "task" | "subagent" => {
                        flush(&mut out, &mut group, &key);
                        // `agent` today; older servers' `task` calls said `subagent_type`.
                        let agent = input["agent"].as_str().or(input["subagent_type"].as_str()).unwrap_or("general");
                        let session = opencode_kit::model::subagent_session(input, metadata);
                        let background = input["background"] == true || (*status == ToolStatus::Completed && metadata["status"] == "running");
                        out.push(Block::Agent { id, agent, title, status, session, background });
                    }
                    "question" => {
                        if !matches!(status, ToolStatus::Pending | ToolStatus::Running) {
                            flush(&mut out, &mut group, &key);
                            out.push(Block::Tool(Step::Tool { id, name, title, status, files }));
                        }
                    }
                    _ => fold_in(&mut group, id.clone(), Step::Tool { id, name, title, status, files }),
                },
            }
        }
    }
    flush(&mut out, &mut group, &key);
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

fn tool_icon(name: &str) -> &'static rune::Icon {
    match name {
        "shell" | "bash" | "execute" => &ic::CONSOLE,
        "grep" | "glob" => &ic::MAGNIFYING_GLASS,
        "read" | "edit" | "write" | "patch" | "apply_patch" => &ic::CODE_LINES,
        "todowrite" | "todoread" => &ic::CHECKLIST,
        _ => &ic::FILE_TREE,
    }
}

pub fn tool_row(p: &Pal, id: &str, name: &str, title: &str, status: &ToolStatus) -> El {
    let color = if *status == ToolStatus::Error { p.danger } else { p.muted };
    row()
        .key(format!("tool-{id}"))
        .h(28.0)
        .gap(8.0)
        .center_y()
        .child(icon(tool_icon(name), 14.0).color(p.icon_muted))
        .child(text(tool_title(name).to_owned()).font(UI_MED).color(color))
        .child(text(title.to_owned()).font(UI).color(p.faint).truncate())
}

/// The folded row: its label and chevron, and when open, every step in order. Opening and
/// closing is instant, as a thought's own disclosure is.
pub fn group(s: &App, p: &Pal, key: &str, steps: &[Step], busy: bool, fade: f32) -> El {
    let open = s.expanded.contains(key);
    let l = label(steps, busy);
    let k = key.to_owned();
    let mut trigger = row()
        .key("trigger")
        .role("button")
        .label(l.text())
        .h(28.0)
        .gap(4.0)
        .center_y()
        // A step joining or the label changing crossfades the words while the chevron slides over.
        .animate_layout()
        .on_press_tx(move |s: &mut App, tx: &mut rune::Tx| {
            tx.instant();
            if !s.expanded.remove(&k) {
                s.expanded.insert(k.clone());
            }
        });
    trigger = match l.count {
        Some(n) => trigger
            .child(text("Used").key("used").font(UI).color(p.muted))
            .child(text(n.to_string()).key(format!("n{n}")).font(UI).color(p.text))
            .child(text(l.title.clone()).key(format!("title-{}", l.title)).font(UI_MED).color(p.text).truncate()),
        None => trigger.child(text(l.title.clone()).key("thinking").font(UI).color(p.muted)),
    };
    trigger = trigger.child(icon(chevron(open), 14.0).key("chevron").color(p.icon_faint));
    let mut c = col().w_full().pb(12.0).child(trigger);
    if open {
        let mut list = col().key("steps").w_full().pl(4.0);
        for step in steps {
            list = list.child(match step {
                Step::Thought { key, body, secs } => {
                    reasoning(p, key, body, *secs, s.expanded.contains(key), fade).key(key.clone()).pb(0.0)
                }
                Step::Tool { id, name, title, status, files } => {
                    let r = tool_row(p, id, name, title, status);
                    match crate::attachments::gallery(s, p, "step-images", files, false) {
                        Some(g) => col().key(format!("step-{id}")).w_full().child(r).child(g.pl(22.0)),
                        None => r,
                    }
                }
            });
        }
        c = c.child(list);
    }
    c
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
                Block::Tool(Step::Tool { name, .. }) => format!("tool {name}"),
                Block::Tool(_) => unreachable!(),
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
            ["c-r: Used 1 Read", "agent general", "c-g: Used 2 Glob, Write", "tool question", "c-x: Used 1 mcp_thing"]
        );
    }

    #[test]
    fn empty_parts_hide_and_live_thoughts_read_thinking() {
        let a = step("m1", vec![thought("  "), say(""), thought("first"), thought("second")]);
        let blocks = fold(&[&a], false);
        let Block::Group { key, steps } = &blocks[0] else { panic!("{blocks:?}") };
        assert_eq!(key, "c-m1-2");
        assert_eq!(label(steps, false).text(), "Used 2 Thoughts");
        assert_eq!(label(steps, true), Label { count: None, title: "Thinking".into() });
        // A live answer that hasn't streamed yet keeps its row.
        let b = step("m2", vec![tool("r", "read", ToolStatus::Completed), say("")]);
        assert_eq!(shape(&fold(&[&b], true)), ["c-r: Used 1 Read", "text "]);
    }
}
