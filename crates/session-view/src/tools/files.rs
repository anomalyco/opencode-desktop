//! Edit, Write and Patch, as a group shows them (`CurrentFileToolGroup` over the `patch`
//! renderer): consecutive file tools merge into one card of file rows (`FileAccordionGroup`),
//! a row per path they changed. A row (`accordion-trigger`) reads the file's icon, its muted
//! directory and name, then "Created", "Deleted" or its `+N -M`, and a grabber; pressed, the
//! change opens flush under it. Several changes to one path share a row: one net diff when each
//! starts where the last ended (`patchFileGroups`), else their diffs one after another. Files
//! start shut. With no file known yet, the first tool's title alone.

use rune::{El, col, el, icon, row, text};
use rune_kit::diff::{FileDiff, parse};
use serde_json::Value;

use super::{Call, Ctx, Display, Press, Trigger, field, path, still, sub_key, trigger};
use crate::TimelineHost;
use crate::palette::Pal;
use crate::style::{INSTANT, UI, UI_MED, readable};

/// A file row's `+N -M` (`DiffChanges`, standard): 14 px mono, regular.
const COUNTS: rune::TextStyle = rune::TextStyle::new(14.0, 440.0).mono().lh(21.0);
/// A row's path: 13 px regular at 150% (`accordion-trigger`).
const PATH: rune::TextStyle = UI.lh(19.5);
/// `--radius-lg`: the card's outer corners.
const RADIUS: f32 = 8.0;

/// Whether `c` joins a run of file tools: an edit, write or patch that didn't fail.
pub fn is_file_tool(c: &Call) -> bool {
    *c.status != opencode_kit::model::ToolStatus::Error && matches!(c.name, "edit" | "multiedit" | "write" | "patch" | "apply_patch")
}

/// What a file's row says it did.
#[derive(Clone, Copy, PartialEq)]
enum Kind {
    Add,
    Update,
    Delete,
}

/// One change a call made to a file: its patch, or the texts before and after.
enum Source<'a> {
    Patch(&'a str),
    Texts(&'a str, &'a str),
}

impl Source<'_> {
    fn diff(&self, path: &str) -> FileDiff {
        match self {
            Source::Patch(p) => parse(p).into_iter().next().unwrap_or_else(|| FileDiff::from_texts(path, "", "")),
            Source::Texts(old, new) => FileDiff::from_texts(path, old, new),
        }
    }

    /// The whole file before and after, when the patch holds all of it (Desktop's
    /// `completePatchContents`): one hunk from the first line, from a producer that writes
    /// full context (a git diff, or a header ending in a tab).
    fn contents(&self) -> Option<(String, String)> {
        let Source::Patch(p) = self else { return None };
        let full = p.starts_with("diff --git ") || p.lines().any(|l| l.starts_with("--- ") && l.ends_with('\t'));
        let fd = parse(p).into_iter().next().filter(|_| full)?;
        let [h] = fd.hunks.as_slice() else { return None };
        if h.old_start + u32::from(h.old_count == 0) > 1 || h.new_start + u32::from(h.new_count == 0) > 1 {
            return None;
        }
        let side = |keep: rune_kit::diff::LineKind| h.lines.iter().filter(|l| l.kind != keep).map(|l| format!("{}\n", l.text)).collect::<String>();
        Some((side(rune_kit::diff::LineKind::Added), side(rune_kit::diff::LineKind::Removed)))
    }
}

/// One change to a path, as one call reported it.
struct Entry<'a> {
    call: Call<'a>,
    path: &'a str,
    kind: Kind,
    additions: usize,
    deletions: usize,
    source: Source<'a>,
}

/// A row: a path and every change the run made to it.
struct Item<'a> {
    path: &'a str,
    entries: Vec<Entry<'a>>,
}

impl Item<'_> {
    fn kind(&self) -> Kind {
        match (self.entries.first().map(|e| e.kind), self.entries.last().map(|e| e.kind)) {
            (_, Some(Kind::Delete)) => Kind::Delete,
            (Some(Kind::Add), _) => Kind::Add,
            _ => Kind::Update,
        }
    }

    /// The net diff when each change starts where the one before ended.
    fn chained(&self) -> Option<FileDiff> {
        if self.entries.len() < 2 {
            return None;
        }
        let contents: Vec<(String, String)> = self.entries.iter().map(|e| e.source.contents()).collect::<Option<_>>()?;
        contents.windows(2).all(|w| w[0].1 == w[1].0).then(|| FileDiff::from_texts(self.path, &contents[0].0, &contents[contents.len() - 1].1))
    }

    fn counts(&self) -> (usize, usize) {
        match self.chained() {
            Some(fd) => (fd.additions(), fd.deletions()),
            None => self.entries.iter().fold((0, 0), |(a, d), e| (a + e.additions, d + e.deletions)),
        }
    }

    fn diffs(&self) -> Vec<FileDiff> {
        match self.chained() {
            Some(fd) => vec![fd],
            None => self.entries.iter().map(|e| e.source.diff(e.path)).collect(),
        }
    }
}

/// The changes call `c` made: its `metadata.files`, else the edit or write its input describes.
fn entries<'a>(c: &Call<'a>) -> Vec<Entry<'a>> {
    if let Some(files) = c.metadata["files"].as_array().filter(|f| !f.is_empty()) {
        // Entries that aren't changes show nothing, and still replace what the input says.
        return files.iter().filter_map(|f| changed(c, f)).collect();
    }
    let Some(path) = path(c.input) else { return Vec::new() };
    let entry = |source, additions, deletions| Entry { call: *c, path, kind: Kind::Update, additions, deletions, source };
    match c.name {
        "edit" | "multiedit" => {
            let (Some(old), Some(new)) = (c.input["oldString"].as_str(), c.input["newString"].as_str()) else { return Vec::new() };
            let fd = FileDiff::from_texts(path, old, new);
            let (a, d) = (fd.additions(), fd.deletions());
            if a + d == 0 { Vec::new() } else { vec![entry(Source::Texts(old, new), a, d)] }
        }
        "write" => match field(c.input, "content") {
            Some(content) => vec![entry(Source::Texts("", content), content.split('\n').count() - usize::from(content.ends_with('\n')), 0)],
            None => Vec::new(),
        },
        _ => Vec::new(),
    }
}

/// One of `metadata.files`, when it is a change (`changedFileDiff`).
fn changed<'a>(c: &Call<'a>, f: &'a Value) -> Option<Entry<'a>> {
    let (path, patch) = (f["file"].as_str()?, f["patch"].as_str()?);
    let (additions, deletions) = (f["additions"].as_u64()? as usize, f["deletions"].as_u64()? as usize);
    let kind = match f["status"].as_str()? {
        "added" => Kind::Add,
        "deleted" => Kind::Delete,
        "modified" => Kind::Update,
        _ => return None,
    };
    (additions + deletions > 0).then_some(Entry { call: *c, path, kind, additions, deletions, source: Source::Patch(patch) })
}

/// A run of file tools: one card of a row per path they changed.
pub fn group<S: TimelineHost>(s: &S, p: &Pal, cx: Ctx, calls: &[Call]) -> El {
    let Some(first) = calls.first() else { return col() };
    let mut items: Vec<Item> = Vec::new();
    for e in calls.iter().flat_map(entries) {
        match items.iter_mut().find(|i| i.path == e.path) {
            Some(i) => i.entries.push(e),
            None => items.push(Item { path: e.path, entries: vec![e] }),
        }
    }
    if items.is_empty() {
        // Nothing changed yet: the first tool's title alone, its file count to come.
        let title = match first.name {
            "edit" | "multiedit" => "Edit",
            "write" => "Write",
            _ => "Patch",
        };
        let pending = calls.iter().find(|c| super::busy(c.status, cx.live)).unwrap_or(first);
        let d = Display::of(s, cx, pending, false);
        return still(trigger(s, p, cx, first, d, Trigger { title: title.into(), ..Default::default() }, Press::None, None));
    }
    let n = items.len();
    let mut card = col().key("files").w_full();
    let mut keyed: Vec<&str> = Vec::new();
    for (i, item) in items.iter().enumerate() {
        let owner = item.entries[0].call;
        // The first row a call shows carries its key, so a press on the host's button keeps it in view.
        let row_key = if keyed.contains(&owner.id) { format!("file-{}-{i}", first.id) } else { format!("tool-{}", owner.id) };
        keyed.push(owner.id);
        card = card.child(file_item(s, p, cx, first.id, item, row_key, i == 0, i + 1 == n));
    }
    // A file's errors sit flush under the card, as Desktop's `DiagnosticsDisplay` follows its
    // file's accordion.
    let errors = items.iter().filter_map(|item| {
        let owner = item.entries[0].call;
        diagnostics(p, owner.metadata, path(owner.input).unwrap_or(item.path))
    });
    col().w_full().child(card).children(errors)
}

/// One row of the card, and under it the change while open, and the file's errors.
#[allow(clippy::too_many_arguments)]
fn file_item<S: TimelineHost>(s: &S, p: &Pal, cx: Ctx, group: &str, item: &Item, row_key: String, first: bool, last: bool) -> El {
    let shown = super::session_relative(s, cx.session, item.path);
    let k = sub_key(group, format!("file:{shown}"));
    // Timeline detail's Edits › Collapse decides how a file shows until pressed.
    let open = crate::disclosure::is_open(s, &k, s.timeline_detail().expanded(crate::detail::Category::Edit));
    let owner = item.entries[0].call;
    let closed_bottom = last && !open;
    let top = if first { RADIUS } else { 0.0 };
    let bottom = if closed_bottom { RADIUS } else { 0.0 };
    let (dir, name) = match shown.rfind(['/', '\\']) {
        Some(at) => (&shown[..=at], &shown[at + 1..]),
        None => ("", shown),
    };
    let mut words = row().key("name").shrink().min_w(0.0).grow(1.0).center_y();
    if !dir.is_empty() {
        words = words.child(text(dir.to_owned()).key("dir").font(PATH).color(p.muted).truncate_head().shrink().min_w(0.0));
    }
    words = words.child(text(name.to_owned()).key("file").font(PATH).color(p.text).truncate_fade().shrink().min_w(0.0));
    let change = change(p, item);
    let toggle = || crate::disclosure::set::<S>(k.clone(), open, row_key.clone());
    let info = row()
        .key("info")
        .h_full()
        .gap(8.0)
        .center_y()
        .grow(1.0)
        .shrink()
        .min_w(0.0)
        .role("button")
        .label(shown.to_owned())
        .on_press_tx(toggle())
        .expanded(open)
        .child(icon(rune_theme_opencode::file_icons::file(name), 16.0).key("icon").color(p.text))
        .child(words);
    let marks = row()
        .key("marks")
        .h_full()
        .gap(16.0)
        .center_y()
        .children(change)
        .child(icon(&rune_theme_opencode::icons::CHEVRON_GRABBER_VERTICAL, 14.0).key("grabber").color(p.text));
    // The host's button sits beside the row's words, a sibling of its button rather than inside
    // it; the whole row presses as the button does, and counts as hovered for the host's button.
    let action = s.tool_row_action(p, cx.session, &owner, Some(item.path));
    let header = row()
        .key(row_key.clone())
        .w_full()
        .h(32.0)
        .px(12.0)
        .gap(20.0)
        .center_y()
        .fill(p.bg_base)
        .hover(p.hover)
        .border(1.0, p.border)
        .corners([top, top, bottom, bottom])
        .hover_group()
        .hover_motion(INSTANT, INSTANT)
        .on_press_tx(toggle())
        .child(info)
        .child(row().key("actions").h_full().gap(16.0).center_y().children(action).child(marks));    // A key holds no `/`, which separates the parts of a path.
    let mut out = col().key(format!("item-{}", shown.replace(['/', '\\'], ":"))).w_full().child(header);
    if !first {
        out = out.mt(-1.0);
    }
    if open {
        let corner = if last { RADIUS } else { 0.0 };
        let views = col().key("views").w_full().children(item.diffs().iter().map(|fd| super::diff::view(p, fd)));
        // Flush under the row, a hairline over the change (`edit-content`); the row's bottom
        // border is the content's top, which has none of its own (`border-top: 0`).
        out = out.child(
            col()
                .key("content")
                .w_full()
                .px(1.0)
                .pb(1.0)
                .border_sides([0.0, 1.0, 1.0, 1.0], p.border)
                .fill(p.layer1)
                .corners([0.0, 0.0, corner, corner])
                .clip_corners([0.0, 0.0, corner, corner])
                .child(el().key("hairline").w_full().h(0.5).fill(p.border_muted))
                .child(readable(p, views)),
        );
    }
    out
}

/// "Created", "Deleted" (13 px medium) or `+N -M`.
fn change(p: &Pal, item: &Item) -> Option<El> {
    let word = |w: &'static str, c| Some(text(w).key("change").font(UI_MED.lh(19.5)).color(c));
    match item.kind() {
        Kind::Add => word("Created", p.diff.add_text),
        Kind::Delete => word("Deleted", p.diff.del_text),
        Kind::Update => {
            let (a, d) = item.counts();
            (a + d > 0).then(|| {
                row().key("change").gap(8.0).center_y()
                    .child(text(format!("+{a}")).font(COUNTS).color(p.diff.add_text))
                    .child(text(format!("-{d}")).font(COUNTS).color(p.diff.del_text))
            })
        }
    }
}
/// A file's errors after the change (severity 1, the first three), as Desktop's
/// `DiagnosticsDisplay` lists them (`[data-component="diagnostics"]`): on the danger fill, 8 by
/// 12 in, under a 0.5 pt `state-border-danger` top border, square and without sides.
fn diagnostics(p: &Pal, metadata: &Value, file: &str) -> Option<El> {
    let list: Vec<&Value> = metadata["diagnostics"][file].as_array()?.iter().filter(|d| d["severity"] == 1).take(3).collect();
    if list.is_empty() {
        return None;
    }
    let font = rune::TextStyle::new(13.0, 400.0).mono().lh(19.5);
    let label = rune::TextStyle::new(13.0, 530.0).mono().lh(19.5).tracking(-0.5);
    let mut box_ = col()
        .key(format!("diagnostics-{}", file.replace(['/', '\\'], ":")))
        .w_full()
        .gap(4.0)
        .px(12.0)
        .py(8.0)
        .fill(p.danger_bg)
        .border_sides([0.5, 0.0, 0.0, 0.0], p.danger_border);
    for d in list {
        let start = &d["range"]["start"];
        let at = format!("[{}:{}]", start["line"].as_u64().unwrap_or(0) + 1, start["character"].as_u64().unwrap_or(0) + 1);
        box_ = box_.child(
            row().w_full().gap(6.0)
                .child(text("ERROR").font(label).color(p.danger))
                .child(text(at).font(font).color(p.danger))
                .child(readable(p, text(d["message"].as_str().unwrap_or("").to_owned()).font(font).color(p.danger).wrap_anywhere().max_lines(3)).shrink().min_w(0.0)),
        );
    }
    Some(box_)
}
