//! Desktop host adapter for `opencode-feature-open-in` (`features/open-in`).
//!
//! Opening files or folders in local editors/terminals is native-only (`can_open` is always
//! `false` on `wasm32`); on `wasm32` the feature crate is omitted while `open_external` and
//! `App::open_in_path` (used by Copy path in the file viewer) remain active.

#[cfg(not(target_arch = "wasm32"))]
pub use opencode_feature_open_in::*;

#[cfg(not(target_arch = "wasm32"))]
use opencode_kernel::{Feature, KernelBuilder, Slot};
#[cfg(not(target_arch = "wasm32"))]
use rune::{Align, Anim, Cursor, PointerPress, el, icon, row};
use rune::{El, Tx};
#[cfg(not(target_arch = "wasm32"))]
use rune_kit::Loader;
#[cfg(not(target_arch = "wasm32"))]
use rune_theme_opencode::icons as ic;
#[cfg(not(target_arch = "wasm32"))]
use serde_json::Value;

use crate::App;
use crate::palette::Pal;
#[cfg(not(target_arch = "wasm32"))]
use crate::palette::kit;
#[cfg(not(target_arch = "wasm32"))]
use crate::popup::{self, Item, Menu, MenuButton as _};

#[cfg(target_arch = "wasm32")]
pub const FINDER: &str = "finder";

#[cfg(target_arch = "wasm32")]
pub fn open_external(tx: &mut Tx, url: &str) -> bool {
    tx.open_url(url).is_ok()
}

#[cfg(target_arch = "wasm32")]
pub fn finder_label(_: &App) -> &'static str {
    ""
}

#[cfg(target_arch = "wasm32")]
pub fn drag_item(_: &App, _: &str) -> Option<rune::DragItem> {
    None
}

#[cfg(target_arch = "wasm32")]
pub fn pick(_: &mut App, _: &str, _: &str, _: bool, _: &mut Tx) {}

#[cfg(target_arch = "wasm32")]
pub fn open_current(_: &mut App, _: &str, _: bool, _: bool) {}

#[cfg(not(target_arch = "wasm32"))]
pub(crate) static OPEN_IN: Slot<OpenIn> = Slot::new();

#[cfg(not(target_arch = "wasm32"))]
impl OpenInHost for App {
    fn open_in(&self) -> &OpenIn {
        self.kernel.slots.get(&OPEN_IN)
    }

    fn open_in_mut(&mut self) -> &mut OpenIn {
        self.kernel.slots.get_mut(&OPEN_IN)
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

/// Current selected Open-in app (`(label, icon, busy)`), or `None` on `wasm32`.
pub(crate) fn current_info(s: &App) -> Option<(&'static str, &'static str, bool)> {
    #[cfg(not(target_arch = "wasm32"))]
    {
        let open_in = s.kernel.slots.get(&OPEN_IN);
        Some((open_in.current_label(), icon_of(open_in.current()), open_in.busy()))
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = s;
        None
    }
}

/// Current Open-in app `(id, label)` when local opening is available and a non-file-manager app is
/// selected (for the header location card's folder submenu).
pub(crate) fn current_non_finder(s: &App) -> Option<(String, &'static str)> {
    #[cfg(not(target_arch = "wasm32"))]
    {
        if !s.can_open_in() {
            return None;
        }
        let open_in = s.kernel.slots.get(&OPEN_IN);
        let id = open_in.current();
        (id != FINDER).then(|| (id.to_owned(), open_in.current_label()))
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = s;
        None
    }
}

/// Installed `(id, label)` apps exposed to plugins (`Workspace:OpenInApps`).
pub(crate) fn apps_for_plugin(s: &App) -> Vec<(&'static str, &'static str)> {
    #[cfg(not(target_arch = "wasm32"))]
    {
        if s.can_open_in() { s.kernel.slots.get(&OPEN_IN).options() } else { Vec::new() }
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = s;
        Vec::new()
    }
}

/// Opens absolute `abs_path` in its system default app (no-op on `wasm32`).
pub(crate) fn open_default(s: &mut App, abs_path: String) {
    #[cfg(not(target_arch = "wasm32"))]
    s.kernel.slots.get_mut(&OPEN_IN).open_default(abs_path);
    #[cfg(target_arch = "wasm32")]
    let _ = (s, abs_path);
}

impl App {
    /// Opening only works where the server shares this machine's files.
    pub fn can_open_in(&self) -> bool {
        #[cfg(not(target_arch = "wasm32"))]
        {
            can_open(self)
        }
        #[cfg(target_arch = "wasm32")]
        {
            false
        }
    }

    /// The app last picked (Desktop's `plugin.file` `openInApp`).
    pub fn load_open_in(&mut self) {
        #[cfg(not(target_arch = "wasm32"))]
        load(self);
    }

    /// `rel` in the open session's directory.
    pub fn open_in_path(&self, rel: &str) -> Option<String> {
        #[cfg(not(target_arch = "wasm32"))]
        {
            resolve_path(self, rel)
        }
        #[cfg(target_arch = "wasm32")]
        {
            let dir = self.session(self.active_session_id()?)?.directory.as_str();
            Some(if dir.is_empty() {
                rel.to_owned()
            } else if rel.is_empty() {
                dir.to_owned()
            } else {
                format!("{}/{rel}", dir.trim_end_matches(['/', '\\']))
            })
        }
    }

    /// Opens `rel` in `app` (the file manager reveals it) and remembers the choice.
    pub fn open_in_app(&mut self, app: &str, rel: &str, remember: bool) {
        #[cfg(not(target_arch = "wasm32"))]
        open_app(self, app, rel, remember);
        #[cfg(target_arch = "wasm32")]
        let _ = (app, rel, remember);
    }

    /// [`App::open_in_app`]; with `reveal` false the file manager opens a folder `rel` rather than
    /// show it selected in its parent, as the toolbar's button and menu do.
    pub fn open_in_app_as(&mut self, app: &str, rel: &str, remember: bool, reveal: bool) {
        #[cfg(not(target_arch = "wasm32"))]
        open_app_as(self, app, rel, remember, reveal);
        #[cfg(target_arch = "wasm32")]
        let _ = (app, rel, remember, reveal);
    }

    /// `Workspace:OpenIn`: opens `path` (absolute, or inside the open session's folder) in `app`,
    /// where the server the window works against shares this machine's files; a file shows
    /// selected in the file manager.
    pub fn open_in_at(&mut self, app: &str, path: &str) {
        #[cfg(not(target_arch = "wasm32"))]
        open_at(self, app, path);
        #[cfg(target_arch = "wasm32")]
        let _ = (app, path);
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub fn poll_open_in(&mut self) -> bool {
        poll(self)
    }
}

/// Desktop's `OpenInAppMenuItemsV2`: "Open in" over each app with its mark (the current one
/// checked, from the toolbar), then Copy path.
#[cfg(not(target_arch = "wasm32"))]
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
    #[cfg(not(target_arch = "wasm32"))]
    {
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
    #[cfg(target_arch = "wasm32")]
    {
        let _ = rel;
        row
    }
}

/// The toolbar's split button for `rel` (`split-button-v2`): 28 pt tall with 6 pt corners and a
/// 1 pt `border-muted` ring; the remembered app's 16 pt mark opens it, the chevron lists the rest.
/// While a launch runs both halves fade to half and the mark turns to the loader. Nothing where
/// opening isn't possible.
pub fn button(s: &App, p: &Pal, key: &'static str, rel: &str) -> Option<El> {
    #[cfg(target_arch = "wasm32")]
    {
        let _ = (s, p, key, rel);
        None
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        if !s.can_open_in() {
            return None;
        }
        let label = s.kernel.slots.get(&OPEN_IN).current_label();
        let (rel_main, rel_menu) = (rel.to_owned(), rel.to_owned());
        let busy = s.kernel.slots.get(&OPEN_IN).busy();
        let chevron_key = format!("{key}-menu");
        let anchor = chevron_key.clone();
        let mark = match rune_theme_opencode::app_icons::named(icon_of(s.kernel.slots.get(&OPEN_IN).current()), p.dark) {
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
        let open = s.popup.open_at(&chevron_key);
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
                if s.kernel.slots.get(&OPEN_IN).busy() {
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
}

/// Kernel feature registration for Open in app detection and launch polling.
#[cfg(not(target_arch = "wasm32"))]
pub struct OpenInFeature;

#[cfg(not(target_arch = "wasm32"))]
impl Feature<App> for OpenInFeature {
    fn id(&self) -> &'static str {
        "feature.open_in"
    }

    fn register(&self, builder: &mut KernelBuilder<App>) {
        builder.register_slot(&OPEN_IN);
        builder.add_setup("open_in.waker", |s, w| s.kernel.slots.get_mut(&OPEN_IN).set_waker(w));
        // Precaution: shares `App::toast` with `inbox.sends`.
        builder
            .schedule(crate::tick::stage::INGEST_PAGES, "open_in.poll", |s, _, _| poll(s))
            .after("inbox.sends");
    }
}
