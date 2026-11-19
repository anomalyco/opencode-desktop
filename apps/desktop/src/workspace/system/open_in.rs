//! Desktop host adapter for `opencode-feature-open-in` (`features/open-in`).

pub use opencode_feature_open_in::*;

use opencode_kernel::{Feature, KernelBuilder, Phase};
use rune::{Align, Anim, Cursor, El, PointerPress, Tx, el, icon, row};
use rune_kit::Loader;
use rune_theme_opencode::icons as ic;
use serde_json::Value;

use crate::App;
use crate::palette::{Pal, kit};
use crate::popup::{self, Item, Menu, MenuButton as _};

impl OpenInHost for App {
    fn open_in(&self) -> &OpenIn {
        &self.workspace_panes.open_in
    }

    fn open_in_mut(&mut self) -> &mut OpenIn {
        &mut self.workspace_panes.open_in
    }

    fn is_local_server(&self) -> bool {
        match &*self.source {
            crate::Source::Live { server, .. } => server.is_local(),
            crate::Source::Fixture => true,
        }
    }

    fn active_session_directory(&self) -> Option<&str> {
        Some(self.session(self.active_session_id()?)?.directory.as_str())
    }

    fn read_json(&self, ns: &str, key: &str) -> Option<Value> {
        crate::storage::json(ns, key)
    }

    fn write_json(&self, ns: &str, key: &str, val: &Value) {
        crate::storage::set_json(ns, key, val);
    }
}

impl App {
    /// Opening only works where the server shares this machine's files.
    pub fn can_open_in(&self) -> bool {
        can_open(self)
    }

    /// The app last picked (Desktop's `plugin.file` `openInApp`).
    pub fn load_open_in(&mut self) {
        load(self);
    }

    /// `rel` in the open session's directory.
    pub fn open_in_path(&self, rel: &str) -> Option<String> {
        resolve_path(self, rel)
    }

    /// Opens `rel` in `app` (the file manager reveals it) and remembers the choice.
    pub fn open_in_app(&mut self, app: &str, rel: &str, remember: bool) {
        open_app(self, app, rel, remember);
    }

    /// [`App::open_in_app`]; with `reveal` false the file manager opens a folder `rel` rather than
    /// show it selected in its parent, as the toolbar's button and menu do.
    pub fn open_in_app_as(&mut self, app: &str, rel: &str, remember: bool, reveal: bool) {
        open_app_as(self, app, rel, remember, reveal);
    }

    /// `Workspace:OpenIn`: opens `path` (absolute, or inside the open session's folder) in `app`,
    /// where the server the window works against shares this machine's files; a file shows
    /// selected in the file manager.
    pub fn open_in_at(&mut self, app: &str, path: &str) {
        open_at(self, app, path);
    }

    pub fn poll_open_in(&mut self) -> bool {
        poll(self)
    }
}

/// Desktop's `OpenInAppMenuItemsV2`: "Open in" over each app with its mark (the current one
/// checked, from the toolbar), then Copy path.
fn menu(s: &App, with_current: bool) -> Menu {
    let apps = menu_options(s, with_current)
        .into_iter()
        .map(|opt| Item::new(opt.item_id, opt.label).app_icon(opt.icon).checked(opt.checked));
    // menu-v2 content is at least 160 pt wide (`menu.css`).
    Menu::new(opencode_i18n::t!("plugin.file.open.in"))
        .min_width(160.0)
        .group(opencode_i18n::t!("plugin.file.open.in"))
        .items(apps)
        .separator()
        .item(Item::new("copy", opencode_i18n::t!("plugin.file.open.copyPath")).weak_icon(&ic::COPY))
}

/// Right-click on a tree row: the apps (the file manager revealing the row), and Copy path. A
/// file of this computer's drags out of the row too, to Explorer, a browser or a chat app, or
/// back into the composer (`rune::DragItem::within_app`).
pub fn context(row: El, rel: String) -> El {
    let dragged = rel.clone();
    row.drag_out(move |s: &App| drag_item(s, &dragged))
        .on_secondary_press(move |s: &mut App, press: PointerPress, tx: &mut Tx| {
            if !s.can_open_in() {
                return;
            }
            let rel = rel.clone();
            tx.instant();
            popup::open(s, menu(s, false), press, tx, move |s: &mut App, picked, tx| {
                tx.instant();
                if let Some(id) = picked {
                    pick(s, &rel, id, true, tx);
                }
            });
        })
}

/// The toolbar's split button for `rel` (`split-button-v2`): 28 pt tall with 6 pt corners and a
/// 1 pt `border-muted` ring; the remembered app's 16 pt mark opens it, the chevron lists the rest.
/// While a launch runs both halves fade to half and the mark turns to the loader. Nothing where
/// opening isn't possible.
pub fn button(s: &App, p: &Pal, key: &'static str, rel: &str) -> Option<El> {
    if !s.can_open_in() {
        return None;
    }
    let label = s.workspace_panes.open_in.current_label();
    let (rel_main, rel_menu) = (rel.to_owned(), rel.to_owned());
    let busy = s.workspace_panes.open_in.busy();
    let chevron_key = format!("{key}-menu");
    let anchor = chevron_key.clone();
    let mark = match rune_theme_opencode::app_icons::named(icon_of(s.workspace_panes.open_in.current()), p.dark) {
        _ if busy => Loader::new().size(14.0).build(kit(p)),
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
        .hover_motion(Anim::INSTANT, Anim::INSTANT)
        .role("button")
        .label(opencode_i18n::t!("plugin.file.open.ariaLabel", app = label))
        .help(opencode_i18n::t!("plugin.file.open.ariaLabel", app = label))
        .layer_opacity(if busy { 0.5 } else { 1.0 })
        .on_press_tx(move |s: &mut App, tx: &mut Tx| {
            tx.instant();
            open_current(s, &rel_main, false, false);
        })
        .child(mark);
    let open = s.dialogs.popup.open_at(&chevron_key);
    let menu_anchor = chevron_key.clone();
    let chev = el()
        .key(chevron_key)
        .size(20.0, 28.0)
        .center()
        .cursor(Cursor::Pointer)
        .fill(if open { p.hover } else { rune::Color::TRANSPARENT })
        .hover(p.hover)
        .press(p.pressed)
        .hover_motion(Anim::INSTANT, Anim::INSTANT)
        .role("button")
        .label_static(opencode_i18n::t!("plugin.file.open.menu"))
        .help(opencode_i18n::t!("plugin.file.open.menu"))
        .layer_opacity(if busy { 0.5 } else { 1.0 })
        .menu_button(menu_anchor, move |s: &mut App, tx: &mut Tx| {
            if s.workspace_panes.open_in.busy() {
                return;
            }
            let rel = rel_menu.clone();
            tx.instant();
            popup::open(s, menu(s, true).align(Align::End), anchor.clone(), tx, move |s: &mut App, picked, tx| {
                tx.instant();
                if let Some(id) = picked {
                    pick(s, &rel, id, false, tx);
                }
            });
        })
        .child(icon(&ic::CHEVRON_DOWN, 14.0).color(p.icon));
    Some(
        row()
            .key(key)
            .h(28.0)
            .radius(6.0)
            .clip_rounded(6.0)
            .box_shadow(rune::Shadow { spread: 1.0, color: p.border_muted, inset: true, ..rune::Shadow::default() })
            .center_y()
            .child(main)
            .child(chev),
    )
}

/// Kernel feature registration for Open in app detection and launch polling.
pub struct OpenInFeature;

impl Feature<App> for OpenInFeature {
    fn id(&self) -> &'static str {
        "feature.open_in"
    }

    fn register(&self, builder: &mut KernelBuilder<App>) {
        builder.add_setup("open_in.waker", |s, w| s.workspace_panes.open_in.set_waker(w));
        builder.add_system_ordered(
            Phase::Ingest,
            crate::tick::order::ingest::OPEN_IN,
            "open_in.poll",
            |s, _, _| poll(s),
        );
    }
}
