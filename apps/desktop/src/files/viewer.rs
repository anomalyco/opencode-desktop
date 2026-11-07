//! The open file: its toolbar, find bar, and either the shared [`CodeViewer`] or, for Markdown,
//! the rendered page ([`super::markdown`]).

use rune::{Anim, Color, Cursor, El, Icon, TextEdit, TextStyle, Tx, col, el, icon, row, text, text_input};
use rune_theme_opencode::icons as ic;

use super::SessionFiles;
use super::markdown::{self, READ_ICON};
use crate::App;
use crate::code_view::CodeViewer;
use crate::palette::Pal;
use crate::view::{UI, UI_MED, UI_SMALL};

const INSTANT: Anim = Anim::INSTANT;
const MONO_TINY: TextStyle = TextStyle::new(11.0, 450.0).mono().lh(15.0).features("'tnum' 1");

static WRAP_ICON: Icon = Icon {
    name: "files-wrap-lines",
    view: 16.0,
    body: r#"<path d="M2.5 4.5H13.5M2.5 8.5H11C12.38 8.5 13.5 9.62 13.5 11C13.5 12.38 12.38 13.5 11 13.5H8.5M10 11.75L8.25 13.5L10 15.25M2.5 12.5H6" stroke="currentColor" stroke-width="1.25" stroke-linecap="round" stroke-linejoin="round" fill="none"/>"#,
};

/// Desktop's `file-tree` icon: a folder with a pane inside.
static PLACEHOLDER_ICON: Icon = Icon {
    name: "files-placeholder-tree",
    view: 20.0,
    body: r#"<path d="M18 18V5H9.5L7.5 2H2L2 18H5M18 18H5M18 18V8.5H5V18" stroke="currentColor" stroke-linecap="square" fill="none"/>"#,
};

/// A toolbar button: `glyph`, filled while `on`, saying `help` on hover.
fn tool(p: &Pal, key: &'static str, glyph: &'static Icon, on: bool, label: &'static str, help: &str, press: impl Fn(&mut App, &mut Tx) + 'static) -> El {
    el().key(key)
        .size(25.0, 25.0)
        .radius(5.0)
        .center()
        .cursor(Cursor::Pointer)
        .fill(if on { p.row_active } else { Color::TRANSPARENT })
        .hover(p.hover)
        .hover_motion(INSTANT, INSTANT)
        .role("button")
        .label(label)
        .help(help.to_owned())
        .on_press_tx(press)
        .child(icon(glyph, 13.0).color(if on { p.icon } else { p.icon_muted }))
}

#[allow(clippy::too_many_arguments)]
pub(super) fn files_main(
    s: &App,
    p: &Pal,
    session_id: &str,
    proj_title: &str,
    ws: &SessionFiles,
    active_path: Option<&str>,
    placeholder: bool,
    sidebar_open: bool,
) -> El {
    let Some(path) = active_path.filter(|_| !placeholder) else {
        // `SessionFilePanelV2Empty` at the panel's top: the 20 px `file-tree` icon 8 px above a
        // 13/500 title and the 13/440 hint, 8 px apart, in `text-weak`.
        return col()
            .key("files-placeholder")
            .grow(1.0)
            .min_w(0.0)
            .h_full()
            .padding(24.0)
            .items_center()
            .child(icon(&PLACEHOLDER_ICON, 20.0).color(p.muted))
            .child(el().h(16.0))
            .child(text("Open file").font(UI_MED).color(p.text))
            .child(el().h(8.0))
            .child(el().h(20.0).center().child(text("Select a file to open").font(UI).color(p.muted)));
    };

    let doc = ws.docs.get(path);
    let rendered = markdown::shows_rendered(ws, path);
    let markdown_doc = markdown::is_markdown(path) && doc.is_some_and(|d| d.ready && !d.missing && !d.binary);
    let wrap_on = doc.is_some_and(|d| d.wrap());
    let find_on = doc.is_some_and(|d| d.find_open);
    let sel_badge = doc.filter(|_| !rendered).and_then(|d| d.selection()).map(|sel| {
        let (a, b) = sel.ordered();
        if a == b { format!("L{a}") } else { format!("L{a}–{b}") }
    });

    let (dir_part, base_name) = opencode_kit::path::split(path);
    let dir_part = Some(dir_part.to_owned()).filter(|d| !d.is_empty());
    let mut crumb = row().key("files-crumb").grow(1.0).shrink().min_w(0.0).gap(3.0).center_y();
    if !sidebar_open && dir_part.is_none() {
        crumb = crumb.child(text(format!("{proj_title}/")).font(UI_SMALL).color(p.faint).shrink().min_w(0.0).truncate_fade());
    }
    if let Some(d) = dir_part {
        crumb = crumb.child(text(d).font(UI_SMALL).color(p.muted).shrink().min_w(0.0).truncate_fade());
    }
    crumb = crumb.child(text(base_name.to_owned()).font(UI_MED).color(p.text).shrink().min_w(0.0).truncate_fade());
    if let Some(lbl) = sel_badge {
        crumb = crumb.child(el().px(5.0).py(1.0).radius(4.0).fill(p.layer1).child(text(lbl).font(MONO_TINY).color(p.muted)));
    }

    let at = |f: fn(&mut App, &str, &str, &mut Tx)| {
        let (sid, path) = (session_id.to_owned(), path.to_owned());
        move |s: &mut App, tx: &mut Tx| f(s, &sid, &path, tx)
    };
    let path_copy = path.to_owned();

    let toolbar = row()
        .key("files-toolbar")
        .w_full()
        .h(crate::review::chrome::BAR_H)
        // The tree toggle floats over the panel's corner; a closed tree's toolbar starts past it.
        .pl(if sidebar_open { 12.0 } else { crate::review::chrome::TOGGLE_INSET })
        .pr(12.0)
        .gap(6.0)
        .center_y()
        .fill(p.bg_base)
        .child(crumb)
        .children(markdown_doc.then(|| {
            let (glyph, label) = if rendered { (&ic::CODE_LINES, "Show source") } else { (&READ_ICON, "Show rendered") };
            tool(p, "files-render-btn", glyph, false, label, label, at(|s, sid, path, tx| s.toggle_rendered(sid, path, tx)))
        }))
        .child(tool(p, "files-find-btn", &ic::MAGNIFYING_GLASS, find_on, "Find in file", "Find in file", at(|s, sid, path, tx| {
            tx.instant();
            if let Some(d) = s.files.sessions.get_mut(sid).and_then(|w| w.docs.get_mut(path)) {
                d.find_open = !d.find_open;
                d.find_input.focused = d.find_open;
            }
        })).help_keys(crate::keymap::Bind::Find.keycaps()))
        .children((!rendered).then(|| {
            tool(p, "files-wrap-btn", &WRAP_ICON, wrap_on, "Wrap lines", "Wrap lines", at(|s, sid, path, tx| {
                tx.instant();
                let wrap = s.files.sessions.get(sid).and_then(|w| w.docs.get(path)).is_some_and(|d| d.wrap());
                s.set_file_wrap(sid, path, !wrap);
            }))
        }))
        .child(tool(p, "files-copy-path", &ic::COPY, false, "Copy path", "Copy path", move |_s: &mut App, tx: &mut Tx| {
            tx.copy(path_copy.clone());
        }))
        .children(crate::open_in::button(s, p, "files-open-in", path));

    let mut out = col()
        .key("files-main")
        .grow(1.0)
        .min_w(0.0)
        .h_full()
        .child(toolbar)
        .child(el().w_full().h(0.5).fill(p.border_muted));

    if let Some(d) = doc
        && d.find_open
    {
        let view = ws.md.get(path);
        let (total, active) = match view.filter(|_| rendered) {
            Some(v) => (v.matches.len(), v.active),
            None if rendered => (0, 0),
            None => (d.match_count(), d.active_match_index()),
        };
        out = out.child(file_find_bar(p, session_id, path, d, total, active, s.settings.spellcheck)).child(el().w_full().h(0.5).fill(p.border_muted));
    }

    // A plugin's file viewer (`FileViewer`) shows the kinds of file it takes.
    let text = doc.filter(|d| d.ready && !d.missing && !d.binary).map_or("", |d| d.text());
    if let Some(view) = crate::plugin_host::file_viewer(s, p, session_id, &s.link_directory(session_id), path, text) {
        return out.child(view);
    }
    if rendered && let Some(d) = doc {
        return out.child(markdown::rendered(p, session_id, path, d, ws.md.get(path)));
    }

    let (sid_get, path_get) = (session_id.to_owned(), path.to_owned());
    let (sid_mut, path_mut) = (session_id.to_owned(), path.to_owned());
    let viewer = CodeViewer::new(
        format!("file-code-{session_id}-{path}"),
        move |s: &App| s.files.sessions.get(&sid_get).and_then(|w| w.docs.get(&path_get)).unwrap_or(&s.files.empty_doc),
        move |s: &mut App| {
            let has = s.files.sessions.get(&sid_mut).is_some_and(|w| w.docs.contains_key(&path_mut));
            if has { s.files.sessions.get_mut(&sid_mut).unwrap().docs.get_mut(&path_mut).unwrap() } else { &mut s.files.empty_doc }
        },
    )
    .build(s, p);

    out.child(viewer)
}

fn file_find_bar(p: &Pal, session_id: &str, path: &str, doc: &crate::code_view::CodeDocState, total: usize, active: usize, spellcheck: bool) -> El {
    let cur = if total == 0 { 0 } else { active + 1 };
    let (sid_edit, path_edit) = (session_id.to_owned(), path.to_owned());
    let (sid_sub, path_sub) = (session_id.to_owned(), path.to_owned());
    let (sid_close, path_close) = (session_id.to_owned(), path.to_owned());

    row()
        .key("files-find-bar")
        .w_full()
        .h(32.0)
        .px(10.0)
        .gap(8.0)
        .center_y()
        .fill(p.layer1)
        .child(icon(&ic::MAGNIFYING_GLASS, 12.0).color(p.faint))
        .child(
            text_input(&doc.find_input, UI_SMALL)
                .key("files-find-input")
                .spellcheck(spellcheck)
                .placeholder("Find in file…", p.faint)
                .color(p.text)
                .grow(1.0)
                .shrink()
                .on_edit(move |s: &mut App| {
                    let has = s.files.sessions.get(&sid_edit).is_some_and(|w| w.docs.contains_key(&path_edit));
                    if has {
                        &mut s.files.sessions.get_mut(&sid_edit).unwrap().docs.get_mut(&path_edit).unwrap().find_input
                    } else {
                        &mut s.files.empty_doc.find_input
                    }
                })
                .on_submit(move |s: &mut App, tx: &mut Tx| s.file_find_step(&sid_sub, &path_sub, true, tx)),
        )
        .children((!doc.find_input.text.is_empty()).then(|| text(format!("{cur}/{total}")).font(MONO_TINY).color(p.muted)))
        .child(
            el().key("files-find-close")
                .size(20.0, 20.0)
                .radius(4.0)
                .center()
                .cursor(Cursor::Pointer)
                .hover(p.hover)
                .hover_motion(INSTANT, INSTANT)
                .role("button")
                .label("Close find")
                .on_press_tx(move |s: &mut App, tx: &mut Tx| {
                    tx.instant();
                    if let Some(d) = s.files.sessions.get_mut(&sid_close).and_then(|w| w.docs.get_mut(&path_close)) {
                        d.find_open = false;
                        d.find_input = TextEdit::new("");
                        d.sync_find_query();
                    }
                })
                .child(icon(&ic::XMARK_SMALL, 11.0).color(p.icon_muted)),
        )
}
