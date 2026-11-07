//! A tool call's row inside a turn's open group, as Desktop draws it
//! (`session-ui/src/tools/tool-renderer.tsx`, `components/basic-tool.tsx`): a one-line trigger
//! (title, subtitle, `key=value` args), and under it, once pressed open, what the call did:
//! a shell's console, an edit's diff, a search's results, a failure's message.
//!
//! Each call shows in one of five phases, collapsed or expanded ([`Display`]). A call opens
//! while it runs only where Desktop lets it (`allowOpenWhilePending`: shell and execute), and
//! only when it has something to show. Opening and closing is instant, as a thought's is
//! (`disclosure::toggle`); the group's row carries the height change on its own fold spring.
//! The host may add a button beside a row that opens what the call touched in its side panel
//! ([`TimelineHost::tool_row_action`]).
//!
//! A row builds again only when its call, its open parts or its look change ([`memo`]): the
//! console's text, a diff's lines and their colours are worked out once per change, not once
//! per commit.

mod console;
mod details;
mod diff;
pub mod error;
mod execute;
mod files;
mod lists;
mod plain;

use std::borrow::Cow;
use std::hash::{Hash, Hasher};

use opencode_kit::model::{self, ToolStatus};
use rune::{El, col, el, row, text};
use serde_json::Value;

use crate::TimelineHost;
use crate::palette::Pal;
use crate::style::{INSTANT, TOOL_CODE, UI, UI_MED, arrow, readable};

pub use details::overlay as details_overlay;
pub use files::{group as file_group, is_file_tool};
pub use lists::{read_group, skill_group, skill_name};
pub use plain::strip_ansi;

/// A tool call, as its part has it.
#[derive(Clone, Copy)]
pub struct Call<'a> {
    pub id: &'a str,
    pub name: &'a str,
    pub status: &'a ToolStatus,
    pub input: &'a Value,
    pub metadata: &'a Value,
}

/// What the rows of one group share.
#[derive(Clone, Copy)]
pub struct Ctx<'a> {
    /// The session the calls are in: their paths are relative to its directory.
    pub session: &'a str,
    /// The turn is still running. A call a finished or stopped turn left pending or running is
    /// [`Phase::Stopped`]: it never shimmers, so an idle window draws nothing.
    pub live: bool,
}

/// Where a call is in its run.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    /// The model is still writing the call.
    Pending,
    Running,
    /// Pending or running when its turn ended: it never finished.
    Stopped,
    Completed,
    Errored,
}

/// Whether a call shows what it did under its trigger.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fold {
    Collapsed,
    Expanded,
}

/// How a call's row draws: its phase, and whether it is open.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Display {
    pub phase: Phase,
    pub fold: Fold,
}

impl Display {
    /// Call `c`'s display: open when its key is in `expanded` and `opens` says it may be.
    pub fn of<S: TimelineHost>(s: &S, cx: Ctx, c: &Call, opens: bool) -> Self {
        let phase = match c.status {
            ToolStatus::Pending if cx.live => Phase::Pending,
            ToolStatus::Running if cx.live => Phase::Running,
            ToolStatus::Pending | ToolStatus::Running => Phase::Stopped,
            ToolStatus::Completed => Phase::Completed,
            ToolStatus::Error => Phase::Errored,
        };
        let fold = if opens && s.expanded().contains(&key(c.id)) { Fold::Expanded } else { Fold::Collapsed };
        Display { phase, fold }
    }

    /// Still being written or running: its title shimmers.
    pub fn busy(self) -> bool {
        matches!(self.phase, Phase::Pending | Phase::Running)
    }

    pub fn open(self) -> bool {
        self.fold == Fold::Expanded
    }
}

/// Whether `status` is still under way in a turn that is `live`.
pub fn busy(status: &ToolStatus, live: bool) -> bool {
    live && matches!(status, ToolStatus::Pending | ToolStatus::Running)
}

/// The key that holds call `id` open in [`TimelineHost::expanded`].
pub fn key(id: &str) -> String {
    format!("tool:{id}")
}

/// The key that holds part `part` of call `id` open: one file of a patch, a nested call of an
/// execute, its output.
pub fn sub_key(id: &str, part: impl std::fmt::Display) -> String {
    format!("tool:{id}/{part}")
}

/// The row of call `c` inside its group, with what it shows open under it, built again only
/// when what it shows changes. A shimmering title isn't cached, so its sweep keeps the run's
/// rhythm.
pub fn view<S: TimelineHost>(s: &S, p: &Pal, cx: Ctx, c: &Call) -> El {
    let build = || {
        if *c.status == ToolStatus::Error && c.name != "execute" {
            return error::card(s, p, cx.session, c);
        }
        match c.name {
            "shell" | "bash" => console::shell(s, p, cx, c),
            "execute" => execute::execute(s, p, cx, c),
            "edit" | "multiedit" | "write" | "patch" | "apply_patch" => files::group(s, p, cx, &[*c]),
            "read" => lists::read(s, p, cx, c),
            "list" | "glob" | "grep" => lists::search(s, p, cx, c),
            "webfetch" => lists::webfetch(s, p, cx, c),
            "websearch" => lists::websearch(s, p, cx, c),
            "skill" => lists::skill_group(s, p, cx, &[*c]),
            _ => lists::generic(s, p, cx, c),
        }
    };
    cached(s, p, cx, &[*c], build)
}

/// `build` for `calls`, kept until one of them, the parts of them held open, their copy
/// buttons, the host's buttons beside them or the look changes.
pub fn cached<S: TimelineHost>(s: &S, p: &Pal, cx: Ctx, calls: &[Call], build: impl FnOnce() -> El) -> El {
    let Some(first) = calls.first() else { return col() };
    if !s.reduced_motion() && calls.iter().any(|c| busy(c.status, cx.live)) {
        return build();
    }
    let mut h = std::hash::DefaultHasher::new();
    (cx.session, cx.live, p.theme, p.dark, s.reduced_motion(), s.session_directory(cx.session)).hash(&mut h);
    let copied = crate::copied::current(s);
    for c in calls {
        (c.id, c.name, std::mem::discriminant(c.status), s.tool_row_action_version(c.id)).hash(&mut h);
        fingerprint(c.input, &mut h);
        fingerprint(c.metadata, &mut h);
        let own = key(c.id);
        let mut open: Vec<&String> = s.expanded().iter().filter(|k| k.starts_with(&own)).collect();
        open.sort();
        open.hash(&mut h);
        copied.as_deref().filter(|k| k.contains(c.id)).hash(&mut h);
    }
    // The memo keys its root with its own id, so the row keeps its keys one level down.
    rune::memo(format!("tm-{}", first.id), h.finish(), || col().w_full().child(build()))
}

/// A cheap stand-in for `v`'s content: its shape, numbers, and each string's length and ends.
/// Streamed output grows, and a tool's input or result is replaced, never edited in its middle
/// at the same length.
fn fingerprint(v: &Value, h: &mut impl Hasher) {
    match v {
        Value::Null => 0u8.hash(h),
        Value::Bool(b) => (1u8, b).hash(h),
        Value::Number(n) => (2u8, n.as_f64().map(f64::to_bits)).hash(h),
        Value::String(s) => {
            let b = s.as_bytes();
            (3u8, b.len(), &b[..b.len().min(32)], &b[b.len().saturating_sub(32)..]).hash(h);
        }
        Value::Array(a) => {
            (4u8, a.len()).hash(h);
            a.iter().for_each(|x| fingerprint(x, h));
        }
        Value::Object(o) => {
            (5u8, o.len()).hash(h);
            for (k, x) in o {
                k.hash(h);
                fingerprint(x, h);
            }
        }
    }
}

/// A trigger's words: Desktop's `TriggerTitle`.
#[derive(Default)]
pub struct Trigger {
    /// Shimmers while the call runs.
    pub title: Cow<'static, str>,
    /// Desktop's compact tools (shell, execute): a medium title, 6 pt gaps and a 16 pt arrow
    /// box. The rest set their title in the regular weight inside a group, 8 pt from the words
    /// after it.
    pub compact: bool,
    pub subtitle: Option<String>,
    pub args: Vec<String>,
    /// "(3 matches)" after a dot, from a search's count.
    pub matches: Option<String>,
    /// The subtitle in the code font (a shell's command).
    pub code: bool,
    /// A link in the subtitle's place (a fetch's URL).
    pub link: Option<El>,
}

/// What a trigger does when pressed.
pub enum Press {
    /// Nothing: the call has nothing more to show.
    None,
    /// Opens and closes the call (or part of it) kept open under this key.
    Toggle(String),
}

/// A call's trigger line: the trigger itself, keyed `tool-{id}` (its words, and the arrow when it
/// opens: on hover, or while open), and beside it, as siblings rather than inside the trigger's
/// button, the call's own `action` and the host's side-panel button, shown while the tool is
/// hovered (Desktop's `basic-tool-action`). Wrap it with [`disclosure`] or [`still`], which
/// make the tool the hover group those buttons follow.
pub fn trigger<S: TimelineHost>(s: &S, p: &Pal, cx: Ctx, c: &Call, d: Display, t: Trigger, press: Press, action: Option<El>) -> El {
    let id = format!("tool-{}", c.id);
    let gap = if t.compact { 6.0 } else { 8.0 };
    let mut words = row().key("words").gap(gap).center_y().shrink().min_w(0.0).child(title(s, p, &t.title, if t.compact { UI_MED } else { UI }, d));
    if let Some(sub) = t.subtitle.filter(|x| !x.is_empty()) {
        let font = if t.code { TOOL_CODE.lh(16.0) } else { UI };
        words = words.child(readable(p, text(sub).key("subtitle").font(font).color(p.faint).truncate_fade()).shrink().min_w(0.0));
    }
    for (k, a) in t.args.into_iter().enumerate() {
        words = words.child(readable(p, text(a).key(format!("arg-{k}")).font(UI).color(p.muted).truncate_fade()).shrink().min_w(0.0));
    }
    if let Some(l) = t.link {
        words = words.child(l);
    }
    if let Some(m) = t.matches {
        let dot = el().key("dot").size(6.0, 6.0).center().child(el().size(2.25, 2.25).radius(1.125).fill(p.icon_muted));
        words = words.child(dot).child(text(m).key("matches").font(UI).color(p.faint));
    }
    let mut button = row().key(id.clone()).h(28.0).gap(if t.compact { 6.0 } else { 0.0 }).center_y().shrink().min_w(0.0).child(words);
    if let Press::Toggle(k) = press {
        let a = el().key("arrow").w(if t.compact { 16.0 } else { 24.0 }).h(24.0).center().child(arrow(p, d.open()));
        button = button
            .child(if d.open() { a } else { a.group_opacity(0.0, 1.0) })
            .role("button")
            .label(t.title.into_owned())
            .expanded(d.open())
            .hover_group()
            .hover_motion(INSTANT, INSTANT)
            .on_press_tx(crate::disclosure::toggle::<S>(k, id));
    }
    let host = s.tool_row_action(p, cx.session, c, None);
    let actions: Vec<El> = action.into_iter().chain(host).collect();
    let line = row().key("line").w_full().h(28.0).gap(2.0).center_y();
    if actions.is_empty() {
        return line.child(button.grow(1.0));
    }
    // The buttons' glyphs rest hidden (`group_opacity`), the buttons themselves still pressable,
    // as Desktop's `basic-tool-action` is.
    line.child(button).child(row().key("actions").gap(2.0).center_y().children(actions))
}

fn title<S: TimelineHost>(s: &S, p: &Pal, t: &str, font: rune::TextStyle, d: Display) -> El {
    if d.busy() {
        return crate::working::shimmer(s, p, t, font, 0.0).key("title");
    }
    text(t.to_owned()).key("title").font(font).color(p.text)
}

/// A tool with nothing to open: its trigger line, the hover group its buttons follow.
pub fn still(line: El) -> El {
    col().w_full().hover_group().child(line)
}

/// What an open call shows, under its trigger: `rail` insets it 12, as Desktop insets a tool's
/// content; a shell's console and a file's diff run full width.
pub fn body(content: El, rail: bool, gap: f32) -> El {
    let b = col().key("body").w_full().pt(gap).child(content);
    if rail { b.pl(12.0) } else { b }
}

/// A trigger line, and `content` under it while open; the whole tool is the hover group its
/// buttons follow.
pub fn disclosure(line: El, d: Display, content: impl FnOnce() -> El) -> El {
    let c = col().w_full().hover_group().child(line);
    if d.open() { c.child(content()) } else { c }
}

/// The call's text result: what it streams while running, else what it returned.
pub fn output<'a>(c: &Call<'a>) -> &'a str {
    model::tool_output(c.status, c.metadata).unwrap_or("")
}

/// A string field of a call's input.
pub fn field<'a>(v: &'a Value, k: &str) -> Option<&'a str> {
    v[k].as_str().filter(|s| !s.is_empty())
}

/// A call's path: `filePath`, else `path`.
pub fn path<'a>(input: &'a Value) -> Option<&'a str> {
    field(input, "filePath").or_else(|| field(input, "path"))
}

/// The last part of a path.
pub fn file_name(path: &str) -> &str {
    path.trim_end_matches(['/', '\\']).rsplit(['/', '\\']).next().unwrap_or(path)
}

/// `path` inside the session's `directory`, relative to it, as Desktop's `relativizeProjectPath`.
pub fn relative<'a>(path: &'a str, directory: Option<&str>) -> &'a str {
    let Some(dir) = directory.filter(|d| !d.is_empty() && *d != "/" && *d != "\\") else { return path };
    if path == dir {
        return "";
    }
    let sep = if dir.contains('\\') { '\\' } else { '/' };
    let dir = dir.trim_end_matches(sep);
    path.strip_prefix(dir).and_then(|r| r.strip_prefix(sep)).unwrap_or(path)
}

/// `path` relative to session `session`'s directory.
pub fn session_relative<'a, S: TimelineHost>(s: &S, session: &str, path: &'a str) -> &'a str {
    relative(path, s.session_directory(session))
}

/// The directory a search ran in, relative to the session's, as its subtitle shows it; none
/// for the session's own directory. Desktop shows the searched directory's parent
/// (`displayDirectory(getDirectory(path))`), which names the wrong folder; this names the one
/// searched, as an intentional Rune design.
pub fn directory<S: TimelineHost>(s: &S, session: &str, path: &str) -> Option<String> {
    Some(session_relative(s, session, path).to_owned()).filter(|r| !r.is_empty())
}

/// "1 file", "3 files" (`ui.common.file`).
pub fn files_count(n: usize) -> String {
    format!("{n} {}", if n == 1 { "file" } else { "files" })
}
