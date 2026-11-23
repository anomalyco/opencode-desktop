//! File viewer toolbars, picture metadata badges, Open-in split button, and the artifact `...` menu.

use opencode_ui::popup::{Item, Menu, MenuButton as _, PopupHost};
use rune::{Align, Anim, Color, Cursor, El, Icon, Shadow, TextStyle, Tx, el, icon, row, text};
use rune_kit::Loader;
use rune_theme_opencode::icons as ic;
use opencode_ui::palette::{Pal, kit};

use crate::{FilesHost, markdown, view};

const INSTANT: Anim = Anim::INSTANT;

/// `artifact-view.css`'s `@container (max-width: 600px)`.
pub(super) const META_MIN_W: f32 = 600.0;

fn tool_shell(p: &Pal, key: &'static str, glyph: &'static Icon, on: bool, label: &'static str, help: &str) -> El {
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
        .child(icon(glyph, 13.0).color(if on { p.icon } else { p.icon_muted }))
}

/// A toolbar button: `glyph`, filled while `on`, saying `help` on hover.
pub(super) fn tool<H: 'static>(
    p: &Pal,
    key: &'static str,
    glyph: &'static Icon,
    on: bool,
    label: &'static str,
    help: &str,
    press: impl Fn(&mut H, &mut Tx) + 'static,
) -> El {
    tool_shell(p, key, glyph, on, label, help).on_press_tx(press)
}

/// A picture's details in the toolbar (`artifact-metadata`): 13 px `text-faint` tabular figures,
/// 6 apart with a `·` between, 12 before the actions; shown while the pointer is on the toolbar
/// or focus is in it, and not at all in a view `META_MIN_W` wide or narrower.
pub(super) fn picture_meta(p: &Pal, items: Vec<String>) -> El {
    const META: TextStyle = TextStyle::new(13.0, 440.0).lh(16.0).features("'tnum' 1");
    let mut out = row()
        .key("files-meta")
        .gap(6.0)
        .mr(6.0)
        .center_y()
        .shrink()
        .min_w(0.0)
        .group_opacity(0.0, 1.0);
    for (i, item) in items.into_iter().enumerate() {
        if i > 0 {
            out = out.child(text("·").font(META).color(p.faint));
        }
        out = out.child(text(item).font(META).color(p.faint).shrink().min_w(0.0).truncate());
    }
    out
}

fn open_in_toolbar_menu(host: &mut dyn FilesHost, rel: &str, anchor: &str, tx: &mut Tx) {
    if host.open_in_current().is_some_and(|c| c.busy) {
        return;
    }
    let menu = view::open_in_menu(host, true).align(Align::End);
    let rel = rel.to_owned();
    tx.instant();
    host.open_context_menu(
        menu,
        anchor.to_owned().into(),
        tx,
        Box::new(move |h: &mut dyn FilesHost, picked: Option<&str>, tx: &mut Tx| {
            tx.instant();
            if let Some(id) = picked {
                h.open_in_pick(&rel, id, false, tx);
            }
        }),
    );
}

/// The toolbar's split button for `rel` (`split-button-v2`): 28 pt tall with 6 pt corners and a
/// 1 pt `border-muted` ring; the remembered app's 16 pt mark opens it, the chevron lists the rest.
pub(super) fn open_in_button<H: FilesHost + PopupHost + 'static>(
    s: &H,
    p: &Pal,
    key: &'static str,
    rel: &str,
) -> Option<El> {
    if !s.can_open_in() {
        return None;
    }
    let cur = s.open_in_current()?;
    let (rel_main, rel_menu) = (rel.to_owned(), rel.to_owned());
    let chevron_key = format!("{key}-menu");
    let anchor = chevron_key.clone();
    let mark = match rune_theme_opencode::app_icons::named(cur.icon, p.dark) {
        _ if cur.busy => Loader::new().size(14.0).build(kit(p)),
        Some(glyph) => icon(glyph, 16.0),
        None => el().size(16.0, 16.0),
    };
    let main = el()
        .key(format!("{key}-run"))
        .size(28.0, 28.0)
        .center()
        .cursor(Cursor::Pointer)
        .hover(p.hover)
        .press(p.pressed)
        .hover_motion(INSTANT, INSTANT)
        .role("button")
        .label(opencode_i18n::t!("plugin.file.open.ariaLabel", app = cur.label))
        .help(opencode_i18n::t!("plugin.file.open.ariaLabel", app = cur.label))
        .layer_opacity(if cur.busy { 0.5 } else { 1.0 })
        .on_press_tx(move |s: &mut H, tx: &mut Tx| {
            tx.instant();
            s.open_in_launch_current(&rel_main, false, false);
        })
        .child(mark);
    let open = s.popup().open_at(&chevron_key);
    let menu_anchor = chevron_key.clone();
    let chev = el()
        .key(chevron_key)
        .size(20.0, 28.0)
        .center()
        .cursor(Cursor::Pointer)
        .fill(if open { p.hover } else { Color::TRANSPARENT })
        .hover(p.hover)
        .press(p.pressed)
        .hover_motion(INSTANT, INSTANT)
        .role("button")
        .label_static(opencode_i18n::t!("plugin.file.open.menu"))
        .help(opencode_i18n::t!("plugin.file.open.menu"))
        .layer_opacity(if cur.busy { 0.5 } else { 1.0 })
        .menu_button(menu_anchor, move |s: &mut H, tx: &mut Tx| {
            open_in_toolbar_menu(s, &rel_menu, &anchor, tx);
        })
        .child(icon(&ic::CHEVRON_DOWN, 14.0).color(p.icon));
    Some(
        row()
            .key(key)
            .h(28.0)
            .radius(6.0)
            .clip_rounded(6.0)
            .box_shadow(Shadow { spread: 1.0, color: p.border_muted, inset: true, ..Shadow::default() })
            .center_y()
            .child(main)
            .child(chev),
    )
}

fn on_copy_content(host: &mut dyn FilesHost, sid: &str, path: &str, tx: &mut Tx) {
    let Some(text) = host
        .files()
        .sessions
        .get(sid)
        .and_then(|w| w.docs.get(path))
        .map(|d| d.text().to_owned())
    else {
        return;
    };
    tx.copy(text);
    host.toast_succeed(opencode_i18n::t!("common.copied").to_owned(), String::new());
}

fn plain_error(raw: &str) -> String {
    let e = raw.trim();
    let e = match e.rfind(" (os error ") {
        Some(i) if e.ends_with(')') => &e[..i],
        _ => e,
    };
    opencode_kit::plain_error::plain_error(e)
}

#[allow(clippy::too_many_arguments)]
fn open_artifact_menu(
    host: &mut dyn FilesHost,
    sid: &str,
    path: &str,
    name: &str,
    source: bool,
    previewable: bool,
    rendered: bool,
    tx: &mut Tx,
) {
    tx.instant();
    let menu = artifact_menu(host, path, source, previewable, rendered);
    let (sid, at, name) = (sid.to_owned(), path.to_owned(), name.to_owned());
    host.open_context_menu(
        menu,
        "files-artifact-menu".to_owned().into(),
        tx,
        Box::new(move |h: &mut dyn FilesHost, picked: Option<&str>, tx: &mut Tx| {
            tx.instant();
            match picked {
                Some("download") => {
                    let Some(text) = h
                        .files()
                        .sessions
                        .get(&sid)
                        .and_then(|w| w.docs.get(&at))
                        .map(|d| d.text().to_owned())
                    else {
                        return;
                    };
                    if let Err(e) = opencode_kit::client::save_download(&name, &text) {
                        h.toast_fail(opencode_i18n::t!("common.requestFailed").to_owned(), plain_error(&e));
                    }
                }
                Some("mode") => markdown::toggle_rendered(h, &sid, &at, tx),
                Some("open-browser") => crate::ops::open_html_in_browser(h, &sid, &at, tx),
                Some("reveal") => h.open_in_reveal(&at),
                Some("copy") => {
                    if let Some(full) = h.open_in_path(&at) {
                        tx.copy(full);
                        h.toast_succeed(opencode_i18n::t!("common.copied").to_owned(), String::new());
                    }
                }
                Some(id) if id.starts_with("app:") => h.open_in_pick(&at, id, false, tx),
                _ => {}
            }
        }),
    );
}

/// Desktop's `ArtifactToolbar`: 44 tall on `layer-01`, 20 in and 12 from the end, with no rule
/// under it. The file's name (13 / 16 at 530); then, 12 apart, a picture's size line and the
/// actions 8 apart: for a previewable file (`text`) its source / preview toggle and Copy
/// content, then the `...` menu (Download, the toggle, Reveal in the file manager, Copy path and
/// Open in).
#[allow(clippy::too_many_arguments)]
pub(super) fn artifact_toolbar<H: FilesHost + PopupHost + 'static>(
    p: &Pal,
    session_id: &str,
    path: &str,
    rendered: bool,
    previewable: bool,
    source: bool,
    menu_open: bool,
    meta: Option<Vec<String>>,
) -> El {
    const NAME: TextStyle = TextStyle::new(13.0, 530.0).lh(16.0);
    let name = opencode_kit::path::basename(path).to_owned();
    let mode_label = if rendered {
        opencode_i18n::t!("plugin.file.view.viewSource")
    } else {
        opencode_i18n::t!("plugin.file.view.viewPreview")
    };
    let mut actions = row().key("files-artifact-actions").gap(8.0).center_y();
    if previewable {
        let (sid, at) = (session_id.to_owned(), path.to_owned());
        actions = actions.child(action::<H>(
            p,
            "files-render-btn",
            if rendered { &ic::OUTLINE_CODE_BRACKETS } else { &ic::OUTLINE_EYE },
            mode_label,
            move |s: &mut H, tx: &mut Tx| markdown::toggle_rendered(s, &sid, &at, tx),
        ));
    }
    if source {
        let (sid, at) = (session_id.to_owned(), path.to_owned());
        actions = actions.child(action::<H>(
            p,
            "files-copy-content",
            &ic::OUTLINE_COPY,
            opencode_i18n::t!("plugin.file.view.copyContent"),
            move |s: &mut H, tx: &mut Tx| on_copy_content(s, &sid, &at, tx),
        ));
    }
    let (sid, at, menu_name) = (session_id.to_owned(), path.to_owned(), name.clone());
    actions = actions.child(
        menu_action::<H>(
            p,
            "files-artifact-menu",
            &ic::OUTLINE_DOTS,
            opencode_i18n::t!("plugin.file.view.actions"),
            move |s: &mut H, tx: &mut Tx| {
                open_artifact_menu(s, &sid, &at, &menu_name, source, previewable, rendered, tx);
            },
        )
        .fill(if menu_open { p.pressed } else { Color::TRANSPARENT }),
    );
    let trailing = row()
        .key("files-artifact-trailing")
        .gap(12.0)
        .center_y()
        .min_w(0.0)
        .children(meta.map(|m| picture_meta(p, m)))
        .child(actions);
    row()
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

/// The apps alone (`OpenInAppMenuItemsV2` with `copyPath={false}`), the current one checked:
/// an artifact toolbar's Open in submenu, its rows `app:<id>`.
fn open_in_apps_menu(host: &dyn FilesHost) -> Menu {
    let apps = host
        .open_in_menu_options(true)
        .into_iter()
        .map(|opt| Item::new(opt.item_id, opt.label).app_icon(opt.icon).checked(opt.checked));
    Menu::new(opencode_i18n::t!("plugin.file.open.in"))
        .min_width(160.0)
        .items(apps)
}

/// The `...` menu's rows.
fn artifact_menu(host: &dyn FilesHost, path: &str, source: bool, previewable: bool, rendered: bool) -> Menu {
    let mut menu = Menu::new(opencode_i18n::t!("plugin.file.view.actions"))
        .min_width(160.0)
        .align(Align::End);
    if source {
        menu = menu.item(Item::new("download", opencode_i18n::t!("plugin.file.view.download")));
    }
    if previewable {
        let label = if rendered {
            opencode_i18n::t!("plugin.file.view.viewSource")
        } else {
            opencode_i18n::t!("plugin.file.view.viewPreview")
        };
        menu = menu.item(Item::new("mode", label));
    }
    if host.opens_as_page(path) {
        menu = menu.item(Item::new("open-browser", opencode_i18n::t!("plugin.file.view.openInBrowser")));
    }
    if host.can_open_in() {
        menu = menu.item(Item::new(
            "reveal",
            opencode_i18n::t!("plugin.file.view.reveal", app = host.open_in_finder_label()),
        ));
    }
    menu = menu.item(Item::new("copy", opencode_i18n::t!("plugin.file.open.copyPath")));
    if host.can_open_in() {
        menu = menu.submenu("open-in", opencode_i18n::t!("plugin.file.open.in"), open_in_apps_menu(host));
    }
    menu
}

/// An `IconButton` ghost-muted large: 28 square, 6 round, its 16 pt glyph in icon-muted.
fn action<H: 'static>(
    p: &Pal,
    key: &'static str,
    glyph: &'static Icon,
    label: &'static str,
    press: impl Fn(&mut H, &mut Tx) + 'static,
) -> El {
    action_face(p, key, glyph, label).on_press_tx(press)
}

/// [`action`] as a menu button: its menu opens as the mouse button goes down.
fn menu_action<H: PopupHost + 'static>(
    p: &Pal,
    key: &'static str,
    glyph: &'static Icon,
    label: &'static str,
    open: impl Fn(&mut H, &mut Tx) + 'static,
) -> El {
    action_face(p, key, glyph, label).menu_button(key, open)
}

fn action_face(p: &Pal, key: &'static str, glyph: &'static Icon, label: &'static str) -> El {
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
        .child(icon(glyph, 16.0).color(p.icon_muted))
}
