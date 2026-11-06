//! Edit, Write and Patch: each file a call changed is an accordion row (its icon, directory and
//! name, and its change counts) that opens to the change itself, as Desktop's
//! `ToolFileAccordion` and `patch` renderer do. Files start shut.

use rune::{Color, El, col, el, icon, row, text};
use rune_kit::diff::{FileDiff, LineKind, parse};
use rune_kit::syntax::SyntaxTheme;
use serde_json::Value;

use super::{Call, Display, Press, Title, Trigger, body, field, files_count, path, sub_key, trigger};
use crate::TimelineHost;
use crate::palette::Pal;
use crate::style::{CODE, INSTANT, UI, UI_SMALL, chevron, readable};

/// A file row's change words: Desktop's 12 px medium.
const SMALL_MED: rune::TextStyle = rune::TextStyle::new(12.0, 530.0).lh(16.0);

/// An edit: a still "Edit · 1 file" title over its file's row; open, the diff, and the file's
/// errors under it.
pub fn edit<S: TimelineHost>(s: &S, p: &Pal, c: &Call) -> El {
    let d = Display::of(s, c, true);
    let input_path = path(c.input).unwrap_or("");
    let diff = changed_files(c.metadata).into_iter().next().or_else(|| {
        let (old, new) = (field(c.input, "oldString")?, c.input["newString"].as_str()?);
        let fd = FileDiff::from_texts(input_path, old, new);
        (fd.additions() + fd.deletions() > 0).then_some(fd)
    });
    let shown = diff.as_ref().map_or(input_path, |fd| fd.path());
    let shown = if shown.is_empty() { input_path } else { shown };
    let count = usize::from(!shown.is_empty());
    let title = Trigger { title: Title::Fixed("Edit"), subtitle: (count > 0).then(|| files_count(count)), ..Default::default() };
    let mut out = col().w_full().child(trigger(s, p, c, d, title, Press::None));
    if shown.is_empty() {
        return out;
    }
    let change = match &diff {
        Some(fd) if !d.busy() => Change::Counts(fd.additions(), fd.deletions()),
        _ => Change::None,
    };
    let k = super::key(c.id);
    let open = s.expanded().contains(&k);
    out = out.child(file_row(s, p, c, 0, &relative_file(s, shown), change, open, k));
    if open {
        let view = match &diff {
            Some(fd) => diff_view(p, fd),
            None => col(),
        };
        out = out.child(body(view, false, 4.0));
    }
    out.children(diagnostics(p, c.metadata, input_path))
}

/// A write: its file's row; open, the file it wrote.
pub fn write<S: TimelineHost>(s: &S, p: &Pal, c: &Call) -> El {
    let (Some(file), Some(content)) = (path(c.input), field(c.input, "content")) else {
        // Nothing written yet: the call's title alone, as Desktop shows a write still being written.
        let d = Display::of(s, c, false);
        return trigger(s, p, c, d, Trigger { title: Title::Fixed("Write"), ..Default::default() }, Press::None);
    };
    let k = super::key(c.id);
    let open = s.expanded().contains(&k);
    let header = file_row(s, p, c, 0, &relative_file(s, file), Change::None, open, k);
    let mut out = col().w_full().child(header);
    if open {
        let lang = FileDiff::from_texts(file, "", "").language().map(str::to_owned);
        out = out.child(body(code_view(p, content, lang.as_deref()), false, 4.0));
    }
    out.children(diagnostics(p, c.metadata, file))
}

/// A patch: a row per file it changed ("Created", "Deleted", or its counts); each opens to its
/// diff. With no changed file known yet, "Patch" alone.
pub fn patch<S: TimelineHost>(s: &S, p: &Pal, c: &Call) -> El {
    let files = changed_files(c.metadata);
    if files.is_empty() {
        let d = Display::of(s, c, false);
        return trigger(s, p, c, d, Trigger { title: Title::Fixed("Patch"), ..Default::default() }, Press::None);
    }
    let mut out = col().w_full();
    for (n, fd) in files.iter().enumerate() {
        let k = sub_key(c.id, n);
        let open = s.expanded().contains(&k);
        let change = match fd.status {
            rune_kit::diff::FileStatus::Added => Change::Created,
            rune_kit::diff::FileStatus::Deleted => Change::Deleted,
            _ => Change::Counts(fd.additions(), fd.deletions()),
        };
        out = out.child(file_row(s, p, c, n, &relative_file(s, fd.path()), change, open, k));
        if open {
            out = out.child(body(diff_view(p, fd), false, 4.0).key(format!("diff-{n}")));
        }
    }
    out
}

/// What a file row says it did.
enum Change {
    None,
    Created,
    Deleted,
    Counts(usize, usize),
}

/// The changed files a call's `metadata.files` lists (Desktop's `changedFileDiff`): each entry's
/// patch, when it has one and it changed something.
fn changed_files(metadata: &Value) -> Vec<FileDiff> {
    metadata["files"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|f| f["additions"].as_u64().unwrap_or(0) + f["deletions"].as_u64().unwrap_or(0) > 0)
        .filter_map(|f| {
            let patch = f["patch"].as_str()?;
            let mut fd = parse(patch).into_iter().next()?;
            let file = f["file"].as_str().unwrap_or("");
            if !file.is_empty() {
                fd.new_path = Some(file.to_owned());
            }
            fd.status = match f["status"].as_str() {
                Some("added") => rune_kit::diff::FileStatus::Added,
                Some("deleted") => rune_kit::diff::FileStatus::Deleted,
                _ => fd.status,
            };
            Some(fd)
        })
        .collect()
}

fn relative_file<S: TimelineHost>(s: &S, path: &str) -> String {
    let dir = s.active_session_id().and_then(|id| s.session_directory(id));
    super::relative(path, dir).to_owned()
}

/// A file's accordion row: icon, muted directory, name, what changed and a chevron. The first
/// file's row of a write or a patch carries the call's key, `tool-{id}`, and the host's\n/// side-panel button.
#[allow(clippy::too_many_arguments)]
fn file_row<S: TimelineHost>(s: &S, p: &Pal, c: &Call, n: usize, path: &str, change: Change, open: bool, k: String) -> El {
    // An edit's still title row carries the call's key and button, so its file row doesn't.
    let first = n == 0 && c.name != "edit" && c.name != "multiedit";
    let (dir, name) = match path.rfind('/') {
        Some(at) => (&path[..=at], &path[at + 1..]),
        None => ("", path),
    };
    let row_key = if first { format!("tool-{}", c.id) } else { format!("file-{}-{n}", c.id) };
    let mut words = row().key("name").gap(0.0).shrink().min_w(0.0).center_y();
    if !dir.is_empty() {
        words = words.child(text(dir.to_owned()).key("dir").font(UI).color(p.muted).truncate_head().shrink().min_w(0.0));
    }
    words = words.child(text(name.to_owned()).key("file").font(UI).color(p.text).truncate_fade());
    let mut r = row()
        .key(row_key.clone())
        .w_full()
        .h(32.0)
        .gap(8.0)
        .center_y()
        .role("button")
        .label(path.to_owned())
        .hover_group()
        .hover_motion(INSTANT, INSTANT)
        .on_press_tx(crate::disclosure::toggle::<S>(k, row_key))
        .child(icon(rune_kit::file_icon(name), 16.0).key("icon").color(p.icon_muted))
        .child(words)
        .child(el().key("spacer").grow(1.0));
    r = match change {
        Change::None => r,
        Change::Created => r.child(text("Created").key("change").font(SMALL_MED).color(p.diff_add)),
        Change::Deleted => r.child(text("Deleted").key("change").font(SMALL_MED).color(p.diff_del)),
        Change::Counts(add, del) => r.child(
            row().key("change").gap(4.0)
                .child(text(format!("+{add}")).font(SMALL_MED).color(p.diff_add))
                .child(text(format!("-{del}")).font(SMALL_MED).color(p.diff_del)),
        ),
    };
    if first
        && let Some(action) = s.tool_row_action(p, c.id, c.name, c.status, c.input, c.metadata)
    {
        r = r.child(row().key("action").group_opacity(0.0, 1.0).child(action));
    }
    let arrow = icon(chevron(open), 14.0).key("chevron").color(p.icon_faint);
    r.child(if open { arrow } else { arrow.group_opacity(0.0, 1.0) })
}

/// A file's errors after the change (severity 1, the first three), as Desktop's
/// `DiagnosticsDisplay` lists them.
fn diagnostics(p: &Pal, metadata: &Value, file: &str) -> Option<El> {
    let list: Vec<&Value> = metadata["diagnostics"][file].as_array()?.iter().filter(|d| d["severity"] == 1).take(3).collect();
    if list.is_empty() {
        return None;
    }
    let mut box_ = col().key("diagnostics").w_full().gap(4.0).px(12.0).py(8.0).mt(4.0).radius(6.0).fill(p.danger.alpha(0.08));
    for d in list {
        let start = &d["range"]["start"];
        let at = format!("[{}:{}]", start["line"].as_u64().unwrap_or(0) + 1, start["character"].as_u64().unwrap_or(0) + 1);
        box_ = box_.child(
            row().w_full().gap(6.0)
                .child(text("ERROR").font(SMALL_MED.mono()).color(p.danger))
                .child(text(at).font(UI_SMALL.mono()).color(p.danger))
                .child(readable(p, text(d["message"].as_str().unwrap_or("").to_owned()).font(UI_SMALL.mono()).color(p.danger).wrap_anywhere().max_lines(3)).shrink().min_w(0.0)),
        );
    }
    Some(box_)
}

/// The theme a file's code colours by: the timeline's code theme where it has one.
fn syntax(p: &Pal) -> SyntaxTheme {
    let base = SyntaxTheme::from_kit(crate::palette::kit(p));
    match p.code {
        Some(code) => code.syntax(base, p.diff_add, p.diff_del),
        None => base,
    }
}

/// One line of code, coloured as `lang`.
fn code_line(theme: &SyntaxTheme, lang: Option<&str>, line: &str, ink: Color) -> El {
    let found = rune_kit::syntax::highlight(lang, line);
    text(line.to_owned()).font(CODE).color(ink).spans(theme.spans(&found, 0)).wrap_anywhere()
}

/// `t` of the way from `base` to `hue` in OKLab, as Changes tints its lines.
fn tint(base: Color, hue: Color, t: f32) -> Color {
    let [a, b] = [base.to_oklab(), hue.to_oklab()];
    Color::from_oklab([0, 1, 2, 3].map(|i| if i == 3 { a[3] } else { a[i] + (b[i] - a[i]) * t }))
}

/// The width a gutter of `digits`-digit line numbers takes.
fn gutter(digits: usize) -> f32 {
    (digits.max(2) as f32 * 7.8 + 16.0).ceil()
}

/// A diff in one column: each hunk's lines under a quiet separator, added and removed lines
/// tinted, a line number in the gutter (the new side's, the old side's for a removed line).
pub fn diff_view(p: &Pal, fd: &FileDiff) -> El {
    let theme = syntax(p);
    let lang = fd.language().map(str::to_owned);
    let most = fd.hunks.iter().map(|h| (h.old_start + h.old_count).max(h.new_start + h.new_count)).max().unwrap_or(1);
    let w = gutter(most.to_string().len());
    let mut out = col().key("diff").w_full().radius(6.0).border(0.5, p.border).fill(p.bg_base).clip_rounded(6.0).py(4.0);
    for (hi, h) in fd.hunks.iter().enumerate() {
        if hi > 0 || h.old_start > 1 || h.new_start > 1 {
            out = out.child(
                row().key(format!("hunk-{hi}")).w_full().h(24.0).px(12.0).center_y().fill(p.layer1)
                    .child(text(h.header()).font(UI_SMALL).color(p.faint).truncate_fade()),
            );
        }
        for (li, l) in h.lines.iter().enumerate() {
            let (tint, number, mark) = match l.kind {
                LineKind::Added => (Some(tint(p.bg_base, p.diff_add, if p.dark { 0.13 } else { 0.095 })), l.new, "+"),
                LineKind::Removed => (Some(tint(p.bg_base, p.diff_del, if p.dark { 0.13 } else { 0.095 })), l.old, "-"),
                _ => (None, l.new, " "),
            };
            let num = number.map(|n| n.to_string()).unwrap_or_default();
            let mut line = row()
                .key(format!("l-{hi}-{li}"))
                .w_full()
                .min_h(20.0)
                .child(row().w(w).pr(8.0).justify_end().child(text(num).font(CODE).color(p.faint)))
                .child(el().w(14.0).child(text(mark).font(CODE).color(p.faint)))
                .child(readable(p, code_line(&theme, lang.as_deref(), &l.text, p.text)).grow(1.0).min_w(0.0).pr(12.0));
            if let Some(t) = tint {
                line = line.fill(t);
            }
            out = out.child(line);
        }
    }
    out
}

/// A file's text, its lines numbered, as Desktop shows what a write wrote.
fn code_view(p: &Pal, content: &str, lang: Option<&str>) -> El {
    let theme = syntax(p);
    let lines: Vec<&str> = content.strip_suffix('\n').unwrap_or(content).split('\n').collect();
    let w = gutter(lines.len().to_string().len());
    let mut out = col().key("code").w_full().radius(6.0).border(0.5, p.border).fill(p.bg_base).clip_rounded(6.0).py(4.0);
    for (n, l) in lines.iter().enumerate() {
        out = out.child(
            row().key(format!("l-{n}")).w_full().min_h(20.0)
                .child(row().w(w).pr(8.0).justify_end().child(text((n + 1).to_string()).font(CODE).color(p.faint)))
                .child(readable(p, code_line(&theme, lang, l, p.text)).grow(1.0).min_w(0.0).pr(12.0)),
        );
    }
    out
}

