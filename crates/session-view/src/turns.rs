//! A turn's work as Desktop's timeline projects it (`session-ui/src/timeline/projection.ts`:
//! `groupContent`, `toolGroupType`, `renderable`, `groupMessages`), by Settings › Preferences ›
//! Timeline detail (`detail.rs`); `session-ui/src/tools/tool-renderer.tsx`
//! (`CurrentContextToolGroup`) labels a group.
//!
//! Each kind of activity stands on its own row, folds into the turn's group, or doesn't show.
//! What folds gathers across a turn's steps, up to the next row of its own: thoughts, tool calls,
//! notices and, where commentary folds, the text a later tool call follows. On its own row a call
//! shows as itself, plain reads share a line and edits share a file card. To-dos, empty parts and
//! questions still waiting on an answer never show, and a failed call shows wherever its kind is
//! hidden. A group reads "Used 5 Read, Shell": the tool count and each tool's name once; thoughts
//! alone read "Used 1 Thought", or "Thinking" while the live end of the turn; notices alone read
//! "Updates". With Desktop's default (Compact) thoughts and calls fold, and text, subagents and
//! answered questions stand on their own.

use rune::{El, col, el, row, text};

use opencode_kit::model::{Assistant, Attachment, Notice, Part, ToolStatus};

use crate::TimelineHost;
use crate::detail::{Category, Detail, Placement};
use crate::odometer::odometer;
use crate::palette::Pal;
use crate::style::{HOVER_FADE, UI, UI_MED, arrow, reasoning};

/// One entry of a turn: a step, or a notice that folds with its work.
#[derive(Clone, Copy, Debug)]
pub enum Entry<'a> {
    Step(&'a Assistant),
    Notice(&'a Notice),
    /// A message the turn passes over without showing (a hidden notice).
    Skip,
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

/// One timeline row of a turn's assistant work.
#[derive(Debug)]
pub enum Block<'a> {
    /// Folded thoughts, tool calls, notices and commentary.
    Group { key: String, steps: Vec<Step<'a>> },
    Text { key: String, body: &'a str, msg: usize, part: usize, live: bool },
    /// A subagent call. `session`: its child session, once the call names it.
    Agent { id: &'a str, agent: &'a str, title: &'a str, status: &'a ToolStatus, session: Option<&'a str>, metadata: &'a serde_json::Value },
    /// A question call, once answered, dismissed or failed (`question_row`).
    Question { id: &'a str, status: &'a ToolStatus, input: &'a serde_json::Value, metadata: &'a serde_json::Value },
    /// A thought on its own row (Thinking shows separately).
    Thought { key: String, step: Step<'a> },
    /// Calls on their own row (their kind shows separately, or a hidden kind's failure).
    Calls { key: String, kind: Calls, steps: Vec<Step<'a>> },
}

/// Folds consecutive assistant messages (a turn's steps) as Desktop's default timeline does.
/// `live`: the last one is still streaming.
pub fn fold<'a>(steps: &[&'a Assistant], live: bool) -> Vec<Block<'a>> {
    let entries: Vec<Entry<'a>> = steps.iter().map(|a| Entry::Step(a)).collect();
    fold_with(&entries, live, &Detail::default())
}

/// A turn's `entries` as rows, by `detail`. `live`: the last step is still streaming. A block's
/// `msg` is its entry's index.
pub fn fold_with<'a>(entries: &[Entry<'a>], live: bool, detail: &Detail) -> Vec<Block<'a>> {
    let commentary = commentary(entries, !live);
    let mut out: Vec<Block<'a>> = Vec::new();
    let mut group: Vec<Step<'a>> = Vec::new();
    let mut key: Option<String> = None;
    let flush = |out: &mut Vec<Block<'a>>, group: &mut Vec<Step<'a>>, key: &mut Option<String>| {
        if !group.is_empty() {
            let key = key.take().unwrap_or_else(|| String::from("c-"));
            out.push(Block::Group { key, steps: std::mem::take(group) });
        }
    };
    let last = entries.iter().rposition(|e| matches!(e, Entry::Step(_)));
    for (mi, e) in entries.iter().enumerate() {
        let a = match e {
            Entry::Notice(n) => {
                if group.is_empty() {
                    key = Some(format!("c-{}", n.id));
                }
                group.push(Step::Notice(n));
                continue;
            }
            Entry::Skip => continue,
            Entry::Step(a) => *a,
        };
        let live_msg = live && Some(mi) == last;
        // Keys count each kind apart, as the server's ordinals do: a thought arriving or
        // showing up in a refetched copy doesn't re-key the answer beside it.
        let (mut texts, mut thoughts) = (0, 0);
        for (pi, part) in a.parts.iter().enumerate() {
            let is_last = live_msg && pi + 1 == a.parts.len();
            let mut fold_in = |group: &mut Vec<Step<'a>>, first: String, step: Step<'a>| {
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
                    let placement = if commentary.contains(&(mi, pi)) { detail.placement(Category::Commentary) } else { Placement::Separate };
                    match placement {
                        Placement::Hidden => {}
                        Placement::Grouped => fold_in(&mut group, format!("c-{}-t{n}", a.id), Step::Text { body: text }),
                        Placement::Separate => {
                            flush(&mut out, &mut group, &mut key);
                            out.push(Block::Text { key: format!("t-{}-{n}", a.id), body: text, msg: mi, part: pi, live: is_last });
                        }
                    }
                }
                Part::Reasoning { text, started, completed } => {
                    let n = thoughts;
                    thoughts += 1;
                    if text.trim().is_empty() {
                        continue;
                    }
                    let secs = completed.map(|c| ((c - started) as f64 / 1000.0).round().max(0.0) as i64);
                    let step = Step::Thought { msg: &a.id, index: n, body: text, secs, started: *started };
                    match detail.placement(Category::Thinking) {
                        Placement::Hidden => {}
                        Placement::Grouped => fold_in(&mut group, format!("c-{}-{n}", a.id), step),
                        Placement::Separate => {
                            flush(&mut out, &mut group, &mut key);
                            out.push(Block::Thought { key: format!("th-{}-{n}", a.id), step });
                        }
                    }
                }
                Part::Tool { id, name, title, status, input, files, metadata } => {
                    let step = Step::Tool { id, name, title, status, files, input, metadata };
                    match name.as_str() {
                        "todowrite" => {}
                        "question" => {
                            if !matches!(status, ToolStatus::Pending | ToolStatus::Running) {
                                flush(&mut out, &mut group, &mut key);
                                out.push(Block::Question { id, status, input, metadata });
                            }
                        }
                        _ => {
                            let category = crate::detail::tool_category(name);
                            let failed = crate::detail::failed(name, status, metadata);
                            match (detail.placement(category), failed) {
                                (Placement::Hidden, false) => {}
                                (Placement::Grouped, _) => fold_in(&mut group, format!("c-{id}"), step),
                                (Placement::Separate, false) if category == Category::Subagents => {
                                    flush(&mut out, &mut group, &mut key);
                                    // `agent` today; older servers' `task` calls said `subagent_type`.
                                    let agent = input["agent"].as_str().or(input["subagent_type"].as_str()).unwrap_or("general");
                                    let session = opencode_kit::model::subagent_session(input, metadata);
                                    out.push(Block::Agent { id, agent, title, status, session, metadata });
                                }
                                (_, failed) => {
                                    flush(&mut out, &mut group, &mut key);
                                    let kind = if failed {
                                        Calls::One
                                    } else if grouped_read(&step) {
                                        Calls::Reads
                                    } else if file_tool(&step) {
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
        }
    }
    flush(&mut out, &mut group, &mut key);
    out
}

/// The turn's commentary (`turnCommentary`), by entry and part: text a later tool call in the
/// turn follows. A `settled` turn whose every text would be commentary keeps its last as the
/// answer, so it still ends with something to read.
fn commentary(entries: &[Entry], settled: bool) -> Vec<(usize, usize)> {
    let parts: Vec<(usize, usize, &Part)> = entries.iter().enumerate()
        .flat_map(|(mi, e)| match e {
            Entry::Step(a) => a.parts.iter().enumerate().map(|(pi, p)| (mi, pi, p)).collect::<Vec<_>>(),
            Entry::Notice(_) | Entry::Skip => Vec::new(),
        })
        .collect();
    let Some(last_tool) = parts.iter().rposition(|(_, _, p)| matches!(p, Part::Tool { .. })) else { return Vec::new() };
    let is_text = |p: &Part| matches!(p, Part::Text { text } if !text.trim().is_empty());
    let mut out: Vec<(usize, usize)> = parts[..last_tool].iter().filter(|(_, _, p)| matches!(p, Part::Text { .. })).map(|(mi, pi, _)| (*mi, *pi)).collect();
    let texts: Vec<(usize, usize)> = parts.iter().filter(|(_, _, p)| is_text(p)).map(|(mi, pi, _)| (*mi, *pi)).collect();
    if settled && texts.iter().all(|t| out.contains(t))
        && let Some(answer) = texts.last()
    {
        out.retain(|t| t != answer);
    }
    out
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

/// A group's label: `ui.messagePart.tools.used` ("Used {count} {title}") split around its count
/// and title as Desktop splits it, "Thinking" (`thinking`), or "Updates" alone, for notices and
/// commentary alone (in `before`).
#[derive(Debug, PartialEq)]
pub struct Label {
    pub count: Option<usize>,
    pub title: String,
    pub thinking: bool,
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

    fn plain(title: String, thinking: bool) -> Label {
        let text = title.clone();
        Label { count: None, title, thinking, before: String::new(), between: String::new(), after: String::new(), text }
    }

    /// `ui.messagePart.tools.used` for `count` and `title`, cut where its count and title stand.
    fn used(count: usize, title: String) -> Label {
        let text = opencode_i18n::plural!("ui.messagePart.tools.used", count, tools = title);
        let at = text.find(&title).unwrap_or(text.len());
        let before = text[..at].trim();
        let after = text.get(at + title.len()..).unwrap_or("").trim().to_owned();
        let digits = count.to_string();
        match before.find(&digits) {
            Some(n) => Label {
                count: Some(count),
                before: before[..n].trim().to_owned(),
                between: before[n + digits.len()..].trim().to_owned(),
                after,
                title,
                thinking: false,
                text,
            },
            None => Label { count: None, before: before.to_owned(), between: String::new(), after, title, thinking: false, text },
        }
    }
}

/// `busy`: the group is the live end of the turn.
pub fn label(steps: &[Step], busy: bool) -> Label {
    let mut names: Vec<&str> = Vec::new();
    let (mut tools, mut thoughts) = (0, 0);
    for s in steps {
        match s {
            Step::Tool { name, .. } => {
                tools += 1;
                let t = tool_title(name);
                if !names.contains(&t) {
                    names.push(t);
                }
            }
            Step::Thought { .. } => thoughts += 1,
            Step::Notice(_) | Step::Text { .. } => {}
        }
    }
    // Thoughts alone aren't finished work yet: the live group reads Thinking, not "Used 1 Thought".
    if busy && tools == 0 && thoughts > 0 && thoughts == steps.len() {
        return Label::plain(opencode_i18n::t!("ui.sessionTurn.status.thinking").into(), true);
    }
    if tools == 0 && thoughts == 0 {
        let updates = opencode_i18n::t!("ui.messagePart.context.updates");
        return Label { before: updates.into(), ..Label::plain(String::new(), false) }.with_text(updates);
    }
    if tools == 0 {
        return Label::used(thoughts, opencode_i18n::plural!("ui.messagePart.context.thought", thoughts));
    }
    // English lists join with commas only ("Read, Search, List"), as `@opencode/ui`'s list does.
    Label::used(tools, opencode_i18n::ui_list(&names))
}

impl Label {
    fn with_text(mut self, text: &str) -> Label {
        self.text = text.to_owned();
        self
    }
}

/// When the first thought of a group that reads Thinking started (ms), for its ticking time.
pub fn thinking_since(steps: &[Step], busy: bool) -> Option<i64> {
    if !label(steps, busy).thinking {
        return None;
    }
    // A thought the server hasn't stamped yet (a streamed part's first delta) has no start:
    // nothing ticks until it has one, rather than counting from the epoch.
    steps.iter().find_map(|st| match st {
        Step::Thought { started, .. } => Some((*started > 0).then_some(*started)),
        _ => None,
    }).flatten()
}

/// When the live turn's Thinking group started thinking (ms): the transcript is busy and its
/// last group is thoughts alone.
pub fn live_thinking_since(t: &opencode_kit::model::Transcript, detail: &Detail) -> Option<i64> {
    if !t.busy() {
        return None;
    }
    let steps: Vec<Entry> = t.messages.iter().rev()
        .map_while(|m| match m { opencode_kit::model::Message::Assistant(a) => Some(Entry::Step(a)), _ => None })
        .collect::<Vec<_>>().into_iter().rev().collect();
    match fold_with(&steps, true, detail).last()? {
        Block::Group { steps, .. } => thinking_since(steps, true),
        _ => None,
    }
}

/// How long a group has been thinking, as Desktop counts it (`ui.message.duration.*`): "3s",
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
    let detail = s.timeline_detail();
    let live = s.active_session_id().filter(|id| !s.is_session_stopped(id) && s.session_revert(id).is_none()).and_then(|id| {
        let since = s.with_transcript(id, |t| live_thinking_since(t, &detail)).flatten()?;
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

/// Between an open group's label and its steps (`--tool-content-gap` in `collapsed-tool-group`).
const GROUP_GAP: f32 = 8.0;

/// The folded row: its label, and when open, every step in order, inset 24 behind a hairline
/// rail (`collapsed-tool-group`), 8 under the label and 8 apart. Plain reads in a row share a
/// line, edits, writes and patches in a row share one card of files, and named skills in a row
/// share one "Loaded" line, as Desktop's `CurrentContextToolGroup` merges them. The row springs
/// open and shut on the timeline's growth, the timeline holding as Desktop's does
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
    // A card of files in the group holds headers that stick 8 under this one (Desktop's
    // `--sticky-accordion-top`, `message-part.css:1330-1332`).
    let files = steps.iter().any(file_tool);
    if open || shutting {
        trigger = trigger.fill(p.bg_base).sticky_top(0.0);
        trigger = if files {
            // Desktop's solid 8 pt under the label (`padding-bottom: var(--tool-content-gap)`
            // on the stuck trigger), which a stuck file header meets; the soft edge would wash
            // over that header. It presses and hovers as the label does.
            trigger.child(el().key("pad").pin(Some(0.0), Some(28.0), Some(0.0), None).h(GROUP_GAP).fill(p.bg_base))
        } else {
            // Steps scroll under it and soften into its fill rather than meeting a hard edge, and
            // shutting from there they slide away under it.
            trigger.stuck_shadow(rune::Shadow { y: 10.0, blur: 20.0, color: p.bg_base, ..Default::default() })
        };
    }
    let words = |key: &'static str, words: &str| (!words.is_empty()).then(|| text(words.to_owned()).key(key).font(UI).color(p.muted));
    trigger = match l.count {
        Some(n) => trigger
            .children(words("used", &l.before))
            .child(odometer(format!("{key}-count"), n.to_string(), n as f64, UI, p.text))
            .children(words("between", &l.between))
            .child(text(l.title.clone()).key(format!("title-{}", l.title)).font(UI_MED).color(p.text).truncate_fade())
            .children(words("after", &l.after)),
        // Thoughts alone, still live: Thinking shimmers in the title's 13/530 (`tool-renderer.tsx:634-636`).
        // Its time ticks beside it in the subtitle's muted 13/440 (`tool-renderer.tsx:640`).
        None if l.thinking => trigger.child(crate::working::shimmer(s, p, &l.title, UI_MED, 0.0).key("thinking")).children(
            thinking_since(steps, busy).map(|at| text(elapsed(s.now_ms() - at)).key("elapsed").font(UI.features("'tnum' 1")).color(p.muted)),
        ),
        // Notices and commentary alone: "Updates" in the prefix's ink (`context-tool-group-prefix`).
        None => trigger
            .children(words("updates", &l.before))
            .children((!l.title.is_empty()).then(|| text(l.title.clone()).key(format!("title-{}", l.title)).font(UI_MED).color(p.text).truncate_fade()))
            .children(words("after", &l.after)),
    };
    // The arrow shows while the pointer is over the label, and always while open.
    let a = el().key("arrow").w(16.0).h(24.0).ml(2.0).center().child(arrow(p, open));
    trigger = trigger.child(if open { a } else { a.group_opacity(0.0, 1.0) });
    let mut c = col().w_full().child(trigger);
    if open || shutting {
        // A file header in the steps sticks under the label and its 8 pt.
        let cx = crate::tools::Ctx { session, live, sticky: 28.0 + GROUP_GAP };
        // The row's clip reveals the steps as it grows, so they don't fade in as well.
        // 8 under the label (inside the steps, so a shut box is no taller than nothing), then the
        // rail 6 in from the steps' top and bottom.
        let mut list = col().key("steps").w_full().pt(GROUP_GAP).pl(12.0).gap(8.0).transition(rune::Transition::none());
        list = list.child(el().key("rail").pin(Some(-0.25), Some(14.0), None, Some(6.0)).w(0.5).fill(p.border));
        let mut i = 0;
        while i < steps.len() {
            let run = |f: fn(&Step) -> bool| steps[i..].iter().take_while(|st| f(st)).count();
            let (n, item) = match &steps[i] {
                st @ Step::Thought { .. } => (1, thought(s, p, st, live, fade)),
                Step::Notice(n) => (1, col().key(format!("notice-{}", n.id)).w_full().child(crate::turn_errors::notice(s, p, n, crate::compaction::Run::default()))),
                Step::Text { body } => (1, col().key(format!("text-{i}")).w_full().child(crate::timeline::markdown_view::<S>(p, body, fade, false))),
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
                    (1, step.children(crate::images::gallery(s, p, "step-images", files, false, None).map(|g| g.pl(22.0))))
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

/// A thought, showing open until pressed where Timeline detail expands thinking. Its key holds
/// it open or shut in a group and on its own row alike.
pub fn thought<S: TimelineHost>(s: &S, p: &Pal, st: &Step, live: bool, fade: f32) -> El {
    let Step::Thought { msg, index, body, secs, .. } = st else { return col() };
    let key = format!("r-{msg}-{index}");
    let open = crate::disclosure::is_open(s, &key, s.timeline_detail().expanded(Category::Thinking));
    reasoning::<S>(s, p, &key, body, *secs, live, open, fade).key(key)
}

/// Calls on their own row (`SessionAssistantContent`, `SessionReadToolGroup`,
/// `SessionFileToolGroup`), drawn as they draw in a group: one call, plain reads sharing a line,
/// or edits sharing a file card.
pub fn calls<S: TimelineHost>(s: &S, p: &Pal, session: &str, kind: Calls, steps: &[Step], live: bool) -> El {
    let cx = crate::tools::Ctx { session, live, sticky: 0.0 };
    let calls: Vec<crate::tools::Call> = steps.iter().filter_map(call).collect();
    let Some(first) = calls.first() else { return col() };
    match kind {
        Calls::Reads => crate::tools::cached(s, p, cx, &calls, || crate::tools::read_group(s, p, cx, &calls)).key(format!("reads-{}", first.id)),
        Calls::Files => col().key(format!("files-{}", first.id)).w_full().child(crate::tools::cached(s, p, cx, &calls, || crate::tools::file_group(s, p, cx, &calls))),
        Calls::One => {
            let files = match &steps[0] {
                Step::Tool { files, .. } => *files,
                _ => &[],
            };
            col().key(format!("step-{}", first.id)).w_full().child(crate::tools::view(s, p, cx, first))
                .children(crate::images::gallery(s, p, "step-images", files, false, None).map(|g| g.pl(22.0)))
        }
    }
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
                Block::Thought { key, .. } | Block::Calls { key, .. } => key.clone(),
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
        let live = label(steps, true);
        assert!(live.thinking && live.count.is_none() && live.title == "Thinking");
        // A live answer that hasn't streamed yet keeps its row.
        let b = step("m2", vec![tool("r", "read", ToolStatus::Completed), say("")]);
        assert_eq!(shape(&fold(&[&b], true)), ["c-r: Used 1 Read", "text "]);
    }
}
