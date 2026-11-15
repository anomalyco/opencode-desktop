//! The open file: its toolbar, find bar, and either the shared [`CodeViewer`] or, for Markdown,
//! the rendered page ([`super::markdown`]).

use rune::{Anim, Color, Cursor, El, Icon, TextEdit, TextStyle, Tx, col, el, icon, row, text};
use rune_theme_opencode::icons as ic;

use super::SessionFiles;
use super::markdown::{self, READ_ICON};
use crate::App;
use crate::code_view::CodeViewer;
use crate::code_view::find_bar::{FindBar, find_bar, over};
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

/// A picture's details in the toolbar (`artifact-metadata`): 13 px `text-faint` tabular figures,
/// 6 apart with a `·` between, 12 before the actions; shown while the pointer is on the toolbar
/// or focus is in it, and not at all in a view `META_MIN_W` wide or narrower.
fn picture_meta(p: &Pal, items: Vec<String>) -> El {
    const META: TextStyle = TextStyle::new(13.0, 440.0).lh(16.0).features("'tnum' 1");
    let mut out = row().key("files-meta").gap(6.0).mr(6.0).center_y().shrink().min_w(0.0).group_opacity(0.0, 1.0);
    for (i, item) in items.into_iter().enumerate() {
        if i > 0 {
            out = out.child(text("·").font(META).color(p.faint));
        }
        out = out.child(text(item).font(META).color(p.faint).shrink().min_w(0.0).truncate());
    }
    out
}

/// `artifact-view.css`'s `@container (max-width: 600px)`.
const META_MIN_W: f32 = 600.0;

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
            .child(text(opencode_i18n::t!("plugin.file.command.open")).font(UI_MED).color(p.text))
            .child(el().h(8.0))
            .child(el().h(20.0).center().child(text(opencode_i18n::t!("plugin.file.selectToOpen")).font(UI).color(p.muted)));
    };

    let doc = ws.docs.get(path);
    let rendered = markdown::shows_rendered(ws, path);
    let previewable = (markdown::is_markdown(path) || super::image::is_svg(path) || ws.tables.contains_key(path)) && doc.is_some_and(|d| d.ready() && !d.missing() && !d.binary());
    let picture = super::image::showing(ws, path);
    // A picture or a video shows on its stage, with no lines to find or wrap.
    let lines = picture.is_none() && !ws.videos.contains_key(path);
    let wrap_on = doc.is_some_and(|d| d.wrap());
    let find_on = doc.is_some_and(|d| d.find_open());
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
        .hover_group_focus()
        .hover_motion(INSTANT, INSTANT)
        .child(crumb)
        .children(picture.filter(|_| s.files.stage_size.get()[0] > META_MIN_W).map(|img| picture_meta(p, img.meta())))
        .children(previewable.then(|| {
            let (glyph, label) = if rendered { (&ic::CODE_LINES, opencode_i18n::t!("plugin.file.view.viewSource")) } else { (&READ_ICON, opencode_i18n::t!("plugin.file.view.viewPreview")) };
            tool(p, "files-render-btn", glyph, false, label, label, at(|s, sid, path, tx| s.toggle_rendered(sid, path, tx)))
        }))
        .children(lines.then(|| tool(p, "files-find-btn", &ic::MAGNIFYING_GLASS, find_on, opencode_i18n::t!("rune.files.find"), opencode_i18n::t!("rune.files.find"), at(|s, sid, path, tx| {
            tx.instant();
            if let Some(d) = s.files.sessions.get_mut(sid).and_then(|w| w.docs.get_mut(path)) {
                let open = !d.find_open();
                d.set_find_open(open);
            }
        })).help_keys(crate::keymap::Bind::Find.keycaps())))
        .children((!rendered && lines).then(|| {
            tool(p, "files-wrap-btn", &WRAP_ICON, wrap_on, opencode_i18n::t!("plugin.review.settings.wrapLines.title"), opencode_i18n::t!("plugin.review.settings.wrapLines.title"), at(|s, sid, path, tx| {
                tx.instant();
                let wrap = s.files.sessions.get(sid).and_then(|w| w.docs.get(path)).is_some_and(|d| d.wrap());
                s.set_file_wrap(sid, path, !wrap);
            }))
        }))
        .child(tool(p, "files-copy-path", &ic::COPY, false, opencode_i18n::t!("plugin.file.open.copyPath"), opencode_i18n::t!("plugin.file.open.copyPath"), move |_s: &mut App, tx: &mut Tx| {
            tx.copy(path_copy.clone());
        }))
        .children(crate::open_in::button(s, p, "files-open-in", path));

    let out = col()
        .key("files-main")
        .grow(1.0)
        .min_w(0.0)
        .h_full()
        .child(toolbar)
        .child(el().w_full().h(0.5).fill(p.border_muted));
    out.child(code_area(s, p, session_id, ws, path, false))
}

/// The open file under the panel's own header (`ui.codeView`): its find bar while open, then a
/// plugin's viewer, the rendered Markdown or the code. `artifact` puts a Markdown file's
/// source / preview toggle above it, as Desktop's artifact toolbar does.
pub(crate) fn code_area(s: &App, p: &Pal, session_id: &str, ws: &SessionFiles, path: &str, artifact: bool) -> El {
    let doc = ws.docs.get(path);
    let rendered = markdown::shows_rendered(ws, path);
    let previewable = (markdown::is_markdown(path) || super::image::is_svg(path) || ws.tables.contains_key(path)) && doc.is_some_and(|d| d.ready() && !d.missing() && !d.binary());
    // A picture shows on its stage, which has no lines to find or wrap.
    let picture = super::image::showing(ws, path);
    let clip = ws.videos.get(path);
    let mut out = col().key("files-code-area").grow(1.0).min_w(0.0).min_h(0.0).w_full();
    // A plugin's file viewer (`FileViewer`) shows the kinds of file it takes.
    let plugin_view = crate::plugin_host::file_viewer(s, p, session_id, &s.link_directory(session_id), path);
    // `SessionFileView`: a text file's code starts 12 below the strip (`mt-3`), under no toolbar;
    // an artifact (Markdown, SVG, a picture, another binary file, a kind a plugin views) has its
    // `ArtifactToolbar`.
    let toolbar = artifact && (previewable || picture.is_some() || clip.is_some() || plugin_view.is_some() || doc.is_some_and(|d| d.binary()));
    if artifact && !toolbar {
        out = out.pt(12.0);
    }
    if toolbar {
        let text = doc.filter(|_| previewable).map(|d| d.text().to_owned());
        out = out.child(artifact_toolbar(s, p, session_id, path, rendered, text, picture.filter(|_| s.files.stage_size.get()[0] > META_MIN_W).map(|img| img.meta())));
    }
    // Pierre's bar floats over the view it searches, rendered or source.
    let bar = doc.filter(|d| d.find_open()).map(|d| {
        let (active, total) = ws.file_matches(path,d);
        let bar = FindBar { key: "files-find-bar", top: -4.0, edit: file_find_input, changed: |s, at, tx| {
            tx.instant();
            let (sid, path) = at.split_once('\n').unwrap_or_default();
            if let Some(w) = s.files.sessions.get_mut(sid) && w.shown_table(path).is_none() && let Some(d) = w.docs.get_mut(path) {
                d.sync_find_query();
            }
            s.sync_rendered_find(sid, path, tx);
        }, step: |s, at, forward, tx| {
            let (sid, path) = at.split_once('\n').unwrap_or_default();
            s.file_find_step(sid, path, forward, tx);
        }, close: |s, at, tx| {
            tx.instant();
            let (sid, path) = at.split_once('\n').unwrap_or_default();
            if let Some(d) = s.files.sessions.get_mut(sid).and_then(|w| w.docs.get_mut(path)) {
                d.set_find_open(false);
            }
        }, target: format!("{session_id}\n{path}") };
        find_bar(p, &d.find_input, total, active, s.settings.spellcheck, bar)
    });

    if let Some(view) = plugin_view {
        return out.child(view);
    }
    if let Some(table) = ws.shown_table(path) {
        let painter = if doc.is_some_and(|d| d.find_open()) { &ws.table_find } else { &super::table_find::TableFind::Closed };
        return out.child(over(super::table_view::view(session_id, path, p, table.clone(), painter.painter(p)), bar));
    }
    if let Some(img) = picture {
        return out.child(super::image::stage(s, p, img));
    }
    // One that can't play shows as another binary file, as Desktop's `decode.failed` does.
    if let Some(v) = clip.filter(|v| !matches!(v.player.state(), rune::PlayerState::Failed(_))) {
        return out.child(super::video::stage(s, p, v));
    }
    if rendered && markdown::is_markdown(path) && let Some(d) = doc {
        return out.child(over(markdown::rendered(p, session_id, path, d, ws.md.get(path), ws.code_paths.get(path), artifact), bar));
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
    .notes(crate::line_comments::host::Notes::new(session_id, crate::line_comments::Surface::Files))
    .build(s, p);

    out.child(over(viewer, bar))
}

/// The find field of file `at` (`<session>\n<path>`).
fn file_find_input<'a>(s: &'a mut App, at: &str) -> &'a mut TextEdit {
    let (sid, path) = at.split_once('\n').unwrap_or_default();
    let has = s.files.sessions.get(sid).is_some_and(|w| w.docs.contains_key(path));
    if has {
        &mut s.files.sessions.get_mut(sid).unwrap().docs.get_mut(path).unwrap().find_input
    } else {
        &mut s.files.empty_doc.find_input
    }
}
/// Desktop's `ArtifactToolbar`: 44 tall on `layer-01`, 20 in and 12 from the end, with no rule
/// under it. The file's name (13 / 16 at 530); then, 12 apart, a picture's size line and the
/// actions 8 apart: for a previewable file (`text`) its source / preview toggle and Copy
/// content, then the `...` menu (Download, the toggle, Reveal in the file manager, Copy path and
/// Open in).
fn artifact_toolbar(s: &App, p: &Pal, session_id: &str, path: &str, rendered: bool, text_body: Option<String>, meta: Option<Vec<String>>) -> El {
    const NAME: TextStyle = TextStyle::new(13.0, 530.0).lh(16.0);
    let name = opencode_kit::path::basename(path).to_owned();
    let previewable = text_body.is_some();
    let mode_label = if rendered { opencode_i18n::t!("plugin.file.view.viewSource") } else { opencode_i18n::t!("plugin.file.view.viewPreview") };
    let mut actions = row().key("files-artifact-actions").gap(8.0).center_y();
    if previewable {
        let (sid, at) = (session_id.to_owned(), path.to_owned());
        actions = actions.child(action(p, "files-render-btn", if rendered { &ic::OUTLINE_CODE_BRACKETS } else { &ic::OUTLINE_EYE }, mode_label, move |s: &mut App, tx: &mut Tx| s.toggle_rendered(&sid, &at, tx)));
        let body = text_body.clone().unwrap_or_default();
        actions = actions.child(action(p, "files-copy-content", &ic::OUTLINE_COPY, opencode_i18n::t!("plugin.file.view.copyContent"), move |s: &mut App, tx: &mut Tx| {
            tx.copy(body.clone());
            crate::status_toast::succeed(s, opencode_i18n::t!("common.copied"), "");
        }));
    }
    let (sid, at, menu_name) = (session_id.to_owned(), path.to_owned(), name.clone());
    let open = s.popup.open_at("files-artifact-menu");
    actions = actions.child(
        action(p, "files-artifact-menu", &ic::OUTLINE_DOTS, opencode_i18n::t!("plugin.file.view.actions"), move |s: &mut App, tx: &mut Tx| {
            tx.instant();
            let (sid, at, body, name) = (sid.clone(), at.clone(), text_body.clone(), menu_name.clone());
            crate::popup::open(s, artifact_menu(s, previewable, rendered), "files-artifact-menu".to_owned(), tx, move |s: &mut App, picked, tx| {
                tx.instant();
                match picked {
                    Some("download") => {
                        if let Err(e) = opencode_kit::client::save_download(&name, body.as_deref().unwrap_or_default()) {
                            crate::status_toast::fail(s, opencode_i18n::t!("common.requestFailed"), crate::status_toast::plain(&e));
                        }
                    }
                    Some("mode") => s.toggle_rendered(&sid, &at, tx),
                    Some("reveal") => s.open_in_app_as(crate::open_in::FINDER, &at, false, true),
                    Some("copy") => {
                        if let Some(full) = s.open_in_path(&at) {
                            tx.copy(full);
                            crate::status_toast::succeed(s, opencode_i18n::t!("common.copied"), "");
                        }
                    }
                    Some(id) => {
                        if let Some(app) = id.strip_prefix("app:") {
                            s.open_in_app_as(app, &at, true, false);
                        }
                    }
                    None => {}
                }
            });
        })
        .fill(if open { p.pressed } else { Color::TRANSPARENT }),
    );
    let trailing = row().key("files-artifact-trailing").gap(12.0).center_y().min_w(0.0).children(meta.map(|m| picture_meta(p, m))).child(actions);
    row()
        .key("files-artifact-toolbar")
        .w_full()
        .h(44.0)
        .pl(20.0)
        .pr(12.0)
        .gap(12.0)
        .center_y()
        .fill(p.layer1)
        .hover_group_focus()
        .hover_motion(INSTANT, INSTANT)
        .child(text(name).font(NAME).color(p.text).grow(1.0).shrink().min_w(0.0).truncate())
        .child(trailing)
}

/// The `...` menu's rows.
fn artifact_menu(s: &App, previewable: bool, rendered: bool) -> crate::popup::Menu {
    use crate::popup::{Item, Menu};
    let mut menu = Menu::new(opencode_i18n::t!("plugin.file.view.actions")).min_width(160.0).align(rune::Align::End);
    if previewable {
        menu = menu.item(Item::new("download", opencode_i18n::t!("plugin.file.view.download")));
        let label = if rendered { opencode_i18n::t!("plugin.file.view.viewSource") } else { opencode_i18n::t!("plugin.file.view.viewPreview") };
        menu = menu.item(Item::new("mode", label));
    }
    if s.can_open_in() {
        menu = menu.item(Item::new("reveal", opencode_i18n::t!("plugin.file.view.reveal", app = crate::open_in::finder_label(s))));
    }
    menu = menu.item(Item::new("copy", opencode_i18n::t!("plugin.file.open.copyPath")));
    if s.can_open_in() {
        menu = menu.submenu("open-in", opencode_i18n::t!("plugin.file.open.in"), crate::open_in::apps_menu(s));
    }
    menu
}

/// An `IconButton` ghost-muted large: 28 square, 6 round, its 16 pt glyph in icon-muted.
fn action(p: &Pal, key: &'static str, glyph: &'static Icon, label: &'static str, press: impl Fn(&mut App, &mut Tx) + 'static) -> El {
    el().key(key)
        .size(28.0, 28.0)
        .radius(6.0)
        .center()
        .cursor(Cursor::Pointer)
        .hover(p.hover)
        .press(p.pressed)
        .hover_motion(INSTANT, INSTANT)
        .role("button")
        .label(label)
        .help(label)
        .on_press_tx(press)
        .child(icon(glyph, 16.0).color(p.icon_muted))
}
