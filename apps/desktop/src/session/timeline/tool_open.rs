//! A tool step's sidebar button (on its row while hovered) opens what it touched in the side
//! region: an edit or a patch in Changes, scrolled to that change and selected; a read or a
//! write in Files, at the lines read. A press on the row folds the step open inline, as
//! Desktop's does; the button follows how Desktop's file links open (`gui-plugins/src/file/renderer.tsx`, `LinkHandler`): the side region opens
//! if it was shut, the file's tab is added or selected, and the lines are selected and
//! scrolled into view. Here it opens at once, focus stays in the timeline, and the same click
//! or Esc puts the side region back how it was.

use rune::{Anim, El, Tx};
use rune_kit::diff::{FileDiff, LineKind, Side, parse};
use serde_json::Value;

use opencode_kit::model::ToolStatus;

use crate::App;
use crate::palette::Pal;
use crate::review::REVIEW_KEY;
#[allow(unused_imports)]
pub use crate::review::{Anchor, Located, locate};

/// Where a step opens.
#[derive(Clone, Debug, PartialEq)]
pub enum Target {
    /// Changes, at the change the step made.
    Change { path: String, anchor: Option<Anchor> },
    /// Files, with lines `start..=end` selected.
    File { path: String, lines: Option<(u32, u32)> },
}

impl Target {
    pub fn path(&self) -> &str {
        match self {
            Self::Change { path, .. } | Self::File { path, .. } => path,
        }
    }

    pub fn tooltip(&self) -> &'static str {
        match self {
            Self::Change { .. } => "Open change in sidebar",
            Self::File { lines: Some(_), .. } => "Open lines in sidebar",
            Self::File { .. } => "Open file in sidebar",
        }
    }
}

/// What a step opened, so the same click or Esc can put things back.
#[derive(Clone, Debug, PartialEq)]
pub struct Opened {
    pub session: String,
    pub step: String,
    pub tab: String,
    /// The tab showing before, or `None` when the side region was shut.
    pub before: Option<String>,
}

/// Where a finished `name` call with `input` and `metadata` opens; `directory` is the session's,
/// which paths are made relative to.
pub fn target(name: &str, status: &ToolStatus, input: &Value, metadata: &Value, directory: &str) -> Option<Target> {
    if *status != ToolStatus::Completed {
        return None;
    }
    let path_of = |v: &Value| v["filePath"].as_str().or(v["path"].as_str()).or(v["file"].as_str()).map(str::to_owned);
    match name {
        "read" => {
            let path = relative(&path_of(input)?, directory)?;
            let offset = input["offset"].as_u64().map(|n| n.max(1) as u32);
            let lines = offset.map(|a| (a, input["limit"].as_u64().map_or(a, |n| a + (n.max(1) as u32) - 1)));
            Some(Target::File { path, lines })
        }
        "write" => Some(Target::File { path: relative(&path_of(input)?, directory)?, lines: None }),
        "edit" | "multiedit" => {
            let raw = path_of(input)?;
            let path = relative(&raw, directory)?;
            let anchor = metadata_anchor(metadata, &path, directory).or_else(|| edit_anchor(input));
            Some(Target::Change { path, anchor })
        }
        "patch" | "apply_patch" => {
            let first = metadata["files"].as_array().and_then(|fs| fs.iter().find_map(|f| path_of(f).or(f["relativePath"].as_str().map(str::to_owned))));
            let raw = first.or_else(|| patch_files(input["patchText"].as_str()?).into_iter().next())?;
            let path = relative(&raw, directory)?;
            let anchor = metadata_anchor(metadata, &path, directory).or_else(|| input["patchText"].as_str().and_then(|t| patch_text_anchor(t, &path)));
            Some(Target::Change { path, anchor })
        }
        _ => None,
    }
}

/// `path` inside `directory`, relative to it; `None` for a path outside it.
pub(crate) fn relative(path: &str, directory: &str) -> Option<String> {
    let rel = if opencode_kit::path::is_absolute(path) {
        opencode_kit::path::strip_dir(path, directory)?
    } else {
        path.trim_start_matches("./")
    };
    let rel = crate::review::data::normalize_path(rel);
    (!rel.is_empty()).then_some(rel)
}

/// The first changed line of the step's own diff (`metadata.files[].patch`) for `path`.
fn metadata_anchor(metadata: &Value, path: &str, directory: &str) -> Option<Anchor> {
    let files = metadata["files"].as_array()?;
    let entry = files.iter().find(|f| {
        let p = f["file"].as_str().or(f["filePath"].as_str()).or(f["relativePath"].as_str()).unwrap_or("");
        relative(p, directory).as_deref() == Some(path)
    })?;
    let patch = entry["patch"].as_str().or(entry["diff"].as_str())?;
    parse(patch).iter().find_map(first_change)
}

fn first_change(fd: &FileDiff) -> Option<Anchor> {
    let lines = || fd.hunks.iter().flat_map(|h| &h.lines);
    let pick = |kind: LineKind, side: Side| {
        lines()
            .filter(|l| l.kind == kind)
            .find(|l| !l.text.trim().is_empty())
            .or_else(|| lines().find(|l| l.kind == kind))
            .map(|l| Anchor { side, text: l.text.clone(), near: l.number(side) })
    };
    pick(LineKind::Added, Side::New).or_else(|| pick(LineKind::Removed, Side::Old))
}

/// An edit's first new line, from its `oldString` and `newString`.
fn edit_anchor(input: &Value) -> Option<Anchor> {
    let (old, new) = (input["oldString"].as_str().unwrap_or(""), input["newString"].as_str().unwrap_or(""));
    let fresh = |a: &str, b: &str| {
        let seen: Vec<&str> = b.lines().collect();
        a.lines().find(|l| !l.trim().is_empty() && !seen.contains(l)).map(str::to_owned)
    };
    fresh(new, old)
        .map(|text| Anchor { side: Side::New, text, near: None })
        .or_else(|| fresh(old, new).map(|text| Anchor { side: Side::Old, text, near: None }))
}

/// The files an `apply_patch` patch text adds or updates, in order.
fn patch_files(text: &str) -> Vec<String> {
    text.lines()
        .filter_map(|l| l.strip_prefix("*** Update File: ").or_else(|| l.strip_prefix("*** Add File: ")))
        .map(|p| p.trim().to_owned())
        .collect()
}

/// The first `+` (or else `-`) line in `path`'s section of an `apply_patch` patch text.
fn patch_text_anchor(text: &str, path: &str) -> Option<Anchor> {
    let mut inside = false;
    let (mut added, mut removed) = (None, None);
    for l in text.lines() {
        if let Some(p) = l.strip_prefix("*** ") {
            if inside && (added.is_some() || removed.is_some()) {
                break;
            }
            let named = p.split_once(": ").map(|(_, f)| f.trim());
            inside = named.is_some_and(|f| f == path || f.ends_with(&format!("/{path}")));
            continue;
        }
        if !inside {
            continue;
        }
        if let Some(t) = l.strip_prefix('+').filter(|t| !t.trim().is_empty()) {
            added = Some(t.to_owned());
            break;
        }
        if removed.is_none()
            && let Some(t) = l.strip_prefix('-').filter(|t| !t.trim().is_empty())
        {
            removed = Some(t.to_owned());
        }
    }
    added
        .map(|text| Anchor { side: Side::New, text, near: None })
        .or_else(|| removed.map(|text| Anchor { side: Side::Old, text, near: None }))
}

/// The step's "open in sidebar" button, beside its row while hovered, when the step has somewhere
/// to open: a press on the row itself folds the step open inline, as Desktop's does. An edit,
/// write or patch shows in a card of file rows, which take none, as Desktop's don't.
pub fn action(s: &App, p: &Pal, session: &str, call: &session_view::tools::Call) -> Option<El> {
    let directory = s.session(session).map(|i| i.directory.as_str()).unwrap_or("");
    let (id, name, status, input, metadata) = (call.id, call.name, call.status, call.input, call.metadata);
    if matches!(name, "edit" | "multiedit" | "write" | "patch" | "apply_patch") {
        return None;
    }
    let t = target(name, status, input, metadata, directory)?;
    let step = id.to_owned();
    let ink = if s.tool_step_showing(id) { p.text } else { p.icon_muted };
    Some(rune::row()
        .key(format!("open-{id}"))
        .role("button")
        .label(format!("{} {}", crate::turns::tool_title(name), t.path()))
        .help(t.tooltip())
        .size(24.0, 24.0)
        .center()
        .radius(6.0)
        .hover(p.hover)
        .press(p.pressed)
        .hover_motion(Anim::INSTANT, Anim::INSTANT)
        // It shows while the pointer is anywhere over its tool's row and hides at once as it
        // leaves (`basic-tool.css:200-213`, no transition); hidden, it stays pressable, as
        // Desktop's tool actions do. On the button rather than its glyph, so the row's hover
        // reveals it, not only its own.
        .group_opacity(if session_view::style::hover_none() { 1.0 } else { 0.0 }, 1.0)
        .child(rune::icon(&rune_theme_opencode::icons::SIDEBAR, 14.0).color(ink))
        .on_press_tx({
            let (step, t) = (step.clone(), t.clone());
            move |s: &mut App, tx: &mut Tx| s.open_tool_step(&step, t.clone(), tx)
        })
        // Each click of a double-click presses too: the second shut what the first opened.
        .on_double_press_tx(move |s: &mut App, tx: &mut Tx| {
            if !s.tool_step_showing(&step) {
                s.open_tool_step(&step, t.clone(), tx);
            }
        }))
}

impl App {
    /// Where step `id` of the open session opens, as its row would.
    pub fn tool_step_target(&self, id: &str) -> Option<Target> {
        let sid = self.active_session_id()?;
        let directory = self.session(sid).map(|i| i.directory.as_str()).unwrap_or("");
        self.transcripts.get(sid)?.messages.iter().find_map(|m| match m {
            opencode_kit::model::Message::Assistant(a) => a.parts.iter().find_map(|p| match p {
                opencode_kit::model::Part::Tool { id: i, name, status, input, metadata, .. } if i == id => target(name, status, input, metadata, directory),
                _ => None,
            }),
            _ => None,
        })
    }

    /// Presses step `id`'s row.
    pub fn press_tool_step(&mut self, id: &str, tx: &mut Tx) {
        if let Some(t) = self.tool_step_target(id) {
            self.open_tool_step(id, t, tx);
        }
    }

    /// Whether step `step`'s view is what the side region shows now.
    pub fn tool_step_showing(&self, step: &str) -> bool {
        self.tool_open.as_ref().is_some_and(|o| o.step == step && self.showing(o))
    }

    fn showing(&self, o: &Opened) -> bool {
        self.active_session_id() == Some(o.session.as_str())
            && self.side_open()
            && self.resolve_side_region().and_then(|r| r.active.clone()).as_deref() == Some(o.tab.as_str())
    }

    /// Opens `target` for step `step`; pressed again while it shows, puts the side region back.
    pub fn open_tool_step(&mut self, step: &str, target: Target, tx: &mut Tx) {
        tx.instant();
        // The timeline narrows or widens with the side region; the pressed row stays put.
        tx.keep_on_screen(format!("tool-{step}"));
        if self.tool_step_showing(step) {
            self.dismiss_tool_step(tx);
            return;
        }
        let Some(sid) = self.active_session_id().map(str::to_owned) else { return };
        // A chain of steps opened one after another still goes back to where it began.
        let before = match self.tool_open.take().filter(|o| self.showing(o)) {
            Some(o) => o.before,
            None if self.side_open() => self.resolve_side_region().and_then(|r| r.active.clone()),
            None => None,
        };
        let tab = match target {
            Target::Change { path, anchor } => self.open_change(&sid, &path, anchor.as_ref(), tx),
            Target::File { path, lines } => {
                self.open_workspace_file_at(&path, false, lines, tx);
                crate::files::data::file_tab_key(&path)
            }
        };
        // Focus stays in the timeline, as a link's click leaves it.
        self.side.focused = false;
        *self.tool_open = Some(Opened { session: sid, step: step.to_owned(), tab, before });
    }

    /// Opens Changes at `path`'s change, or Files at its line when Changes doesn't list it.
    /// Returns the tab it opened.
    fn open_change(&mut self, sid: &str, path: &str, anchor: Option<&Anchor>, tx: &mut Tx) -> String {
        self.show_side_tab(sid, REVIEW_KEY, None);
        self.ensure_session_review(sid);
        let Some(sr) = self.review.sessions.get_mut(sid) else { return REVIEW_KEY.to_owned() };
        if let crate::review::Load::Loading { focus } = &mut sr.load {
            *focus = Some((path.to_owned(), anchor.cloned()));
            return REVIEW_KEY.to_owned();
        }
        if self.focus_review_change(sid, path, anchor, tx) {
            return REVIEW_KEY.to_owned();
        }
        let line = anchor.and_then(|a| a.near).map(|n| (n, n));
        self.open_workspace_file_at(path, false, line, tx);
        crate::files::data::file_tab_key(path)
    }

    /// Scrolls Changes to `path` and selects its change; false when Changes doesn't list it.
    pub fn focus_review_change(&mut self, sid: &str, path: &str, anchor: Option<&Anchor>, tx: &mut Tx) -> bool {
        crate::review::focus_change(self, sid, path, anchor, tx)
    }

    /// Esc while a step's view shows: puts the side region back. True when it did.
    /// A tool step's view is open in the side region (the escape stack's `ToolStep` layer).
    pub(crate) fn tool_view_open(&self) -> bool {
        self.tool_open.as_ref().is_some_and(|o| self.showing(o))
    }

    pub fn dismiss_tool_step(&mut self, tx: &mut Tx) -> bool {
        let Some(o) = self.tool_open.take() else { return false };
        if !self.showing(&o) {
            return false;
        }
        tx.instant();
        tx.keep_on_screen(format!("tool-{}", o.step));
        match o.before {
            Some(tab) => {
                self.select_side_tab(&tab, tx);
                self.side.focused = false;
            }
            None => {
                self.session_side_mut(&o.session).visibility.step(false);
                self.side.focused = false;
            }
        }
        true
    }
}
