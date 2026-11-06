//! A tool call's row inside a turn's open group, as Desktop draws it
//! (`session-ui/src/tools/tool-renderer.tsx`, `components/basic-tool.tsx`): a one-line trigger
//! (title, subtitle, `key=value` args), and under it, once pressed open, what the call did:
//! a shell's console, an edit's diff, a search's results, a failure's message.
//!
//! Each call shows in one of four phases, collapsed or expanded ([`Display`]). A call opens
//! while it runs only where Desktop lets it (`allowOpenWhilePending`: shell and execute), and
//! only when it has something to show. Opening and closing is instant, as a thought's is
//! (`disclosure::toggle`); the group's row carries the height change on its own fold spring.
//! The host may add a button that opens what the call touched in its side panel
//! ([`TimelineHost::tool_row_action`]).

mod console;
mod error;
mod files;
mod lists;

use opencode_kit::model::{self, ToolStatus};
use rune::{El, col, icon, row, text};
use serde_json::Value;

use crate::TimelineHost;
use crate::palette::Pal;
use crate::style::{INSTANT, UI, UI_MED, chevron, readable};

pub use console::strip_ansi;
pub use lists::read_group;

/// A tool call, as its part has it.
#[derive(Clone, Copy)]
pub struct Call<'a> {
    pub id: &'a str,
    pub name: &'a str,
    pub status: &'a ToolStatus,
    pub input: &'a Value,
    pub metadata: &'a Value,
}

/// Where a call is in its run.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    /// The model is still writing the call.
    Pending,
    Running,
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
    pub fn of<S: TimelineHost>(s: &S, c: &Call, opens: bool) -> Self {
        let phase = match c.status {
            ToolStatus::Pending => Phase::Pending,
            ToolStatus::Running => Phase::Running,
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

/// The key that holds call `id` open in [`TimelineHost::expanded`].
pub fn key(id: &str) -> String {
    format!("tool:{id}")
}

/// The key that holds part `part` of call `id` open: one file of a patch, a nested call of an
/// execute, its output.
pub fn sub_key(id: &str, part: impl std::fmt::Display) -> String {
    format!("tool:{id}/{part}")
}

/// The row of call `c` inside its group, with what it shows open under it.
pub fn view<S: TimelineHost>(s: &S, p: &Pal, c: &Call) -> El {
    let el = if *c.status == ToolStatus::Error && c.name != "execute" {
        error::card(s, p, c)
    } else {
        match c.name {
            "shell" | "bash" => console::shell(s, p, c),
            "execute" => console::execute(s, p, c),
            "edit" | "multiedit" => files::edit(s, p, c),
            "write" => files::write(s, p, c),
            "patch" | "apply_patch" => files::patch(s, p, c),
            "read" => lists::read(s, p, c),
            "list" | "glob" | "grep" => lists::search(s, p, c),
            "webfetch" => lists::webfetch(s, p, c),
            "websearch" => lists::websearch(s, p, c),
            "skill" => lists::skill(s, p, c),
            _ => lists::generic(s, p, c),
        }
    };
    col().key(format!("step-{}", c.id)).w_full().child(el)
}

/// A trigger's words: Desktop's `TriggerTitle`.
#[derive(Default)]
pub struct Trigger {
    /// Shimmers while the call runs; a title that isn't fixed (a provider's name) shows still.
    pub title: Title,
    pub subtitle: Option<String>,
    pub args: Vec<String>,
    /// "(3 matches)" after a dot, from a search's count.
    pub matches: Option<String>,
    /// The subtitle in the code font (a shell's command).
    pub code: bool,
    /// A link in the subtitle's place (a fetch's URL).
    pub link: Option<El>,
}

pub enum Title {
    Fixed(&'static str),
    Named(String),
}

impl Default for Title {
    fn default() -> Self {
        Title::Fixed("")
    }
}

impl Title {
    fn text(&self) -> &str {
        match self {
            Title::Fixed(t) => t,
            Title::Named(t) => t,
        }
    }
}

/// What a trigger does when pressed.
pub enum Press {
    /// Nothing: the call has nothing more to show.
    None,
    /// Opens and closes the call (or part of it) kept open under this key.
    Toggle(String),
}

/// A call's one-line trigger, keyed `tool-{id}`: its words, the chevron when it opens (on hover,
/// or while open), and the host's side-panel button on hover.
pub fn trigger<S: TimelineHost>(s: &S, p: &Pal, c: &Call, d: Display, t: Trigger, press: Press) -> El {
    let id = format!("tool-{}", c.id);
    let mut r = row().key(id.clone()).w_full().h(28.0).gap(6.0).center_y().hover_group().hover_motion(INSTANT, INSTANT);
    r = r.child(title(s, p, &t.title, d));
    if let Some(sub) = t.subtitle.filter(|x| !x.is_empty()) {
        let font = if t.code { crate::style::CODE.lh(16.0) } else { UI };
        r = r.child(readable(p, text(sub).key("subtitle").font(font).color(p.muted).truncate_fade()).shrink().min_w(0.0));
    }
    for (k, a) in t.args.into_iter().enumerate() {
        r = r.child(readable(p, text(a).key(format!("arg-{k}")).font(UI).color(p.muted).truncate_fade()).shrink().min_w(0.0));
    }
    if let Some(l) = t.link {
        r = r.child(l);
    }
    if let Some(m) = t.matches {
        r = r.child(text("·").key("dot").font(UI).color(p.faint)).child(text(m).key("matches").font(UI).color(p.faint));
    }
    let toggles = matches!(press, Press::Toggle(_));
    if toggles {
        let arrow = icon(chevron(d.open()), 14.0).key("chevron").color(p.icon_faint);
        r = r.child(if d.open() { arrow } else { arrow.group_opacity(0.0, 1.0) });
    }
    if let Some(action) = s.tool_row_action(p, c.id, c.name, c.status, c.input, c.metadata) {
        r = r.child(row().key("action").group_opacity(0.0, 1.0).child(action));
    }
    match press {
        Press::None => r,
        Press::Toggle(k) => r.role("button").label(t.title.text().to_owned()).on_press_tx(crate::disclosure::toggle::<S>(k, id)),
    }
}

fn title<S: TimelineHost>(s: &S, p: &Pal, t: &Title, d: Display) -> El {
    match t {
        Title::Fixed(words) if d.busy() => crate::working::shimmer(s, p, words, UI_MED, 0.0).key("title"),
        _ => text(t.text().to_owned()).key("title").font(UI_MED).color(p.text),
    }
}

/// What an open call shows, under its trigger: `rail` insets it 12, as Desktop insets a tool's
/// content; a shell's console and a file's diff run full width.
pub fn body(content: El, rail: bool, gap: f32) -> El {
    let b = col().key("body").w_full().pt(gap).pb(4.0).child(content);
    if rail { b.pl(12.0) } else { b }
}

/// A trigger, and `content` under it while open.
pub fn disclosure(trigger: El, d: Display, content: impl FnOnce() -> El) -> El {
    let c = col().w_full().child(trigger);
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

/// The directory a call ran in, relative to the session's, as a search's subtitle shows it.
pub fn directory<S: TimelineHost>(s: &S, path: &str) -> String {
    let dir = s.active_session_id().and_then(|id| s.session_directory(id));
    let rel = relative(path, dir);
    if rel.is_empty() { "/".to_owned() } else { rel.to_owned() }
}

/// "1 file", "3 files" (`ui.common.file`).
pub fn files_count(n: usize) -> String {
    format!("{n} {}", if n == 1 { "file" } else { "files" })
}
