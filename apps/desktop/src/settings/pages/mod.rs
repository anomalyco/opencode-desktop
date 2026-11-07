//! Settings pages built as Desktop's (`settings/settings.css`), each a self-contained view the
//! two-pane Settings shell hosts: Providers, Models and Extensions. Until that shell lands, a
//! plain list beside them picks the page, with General for the page Settings had before.

pub mod extensions;
pub mod models;
pub mod providers;

use std::collections::HashSet;

use rune::{Color, El, ScrollSpec, TextEdit, TextStyle, Tx, col, el, icon, row, text};
use rune_theme_opencode::icons as ic;

use crate::App;
use crate::palette::Pal;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Page {
    #[default]
    General,
    Providers,
    Models,
    Extensions,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ExtensionsTab {
    #[default]
    Mcps,
    Plugins,
    Skills,
}

/// What the pages hold while Settings is open.
#[derive(Default)]
pub struct State {
    pub page: Page,
    /// Models' search (`settings-filter-search`).
    pub search: TextEdit,
    /// Providers folded shut on Models.
    pub collapsed: HashSet<String>,
    pub tab: ExtensionsTab,
    /// The server's plugins and skills, once read.
    pub plugins: Option<Vec<String>>,
    pub skills: Option<Vec<String>>,
    /// A provider's account menu or options open on Providers: (provider, which).
    pub menu: Option<(String, providers::Menu)>,
    /// The account being renamed, and its field.
    pub renaming: Option<String>,
    pub rename: TextEdit,
}

/// `.settings-tab-title`: 15 on 20 at 640.
pub const TITLE: TextStyle = TextStyle::new(15.0, 640.0).lh(20.0);
/// The page's description, on the base 24 line.
pub const DESCRIPTION: TextStyle = TextStyle::new(13.0, 440.0).lh(24.0);
/// `.settings-section-title` and a list's names: 13 on 16 at 530.
pub const NAME: TextStyle = TextStyle::new(13.0, 530.0).lh(16.0);
pub const NAME_REGULAR: TextStyle = TextStyle::new(13.0, 440.0).lh(16.0).tracking(-0.04);
pub const BODY: TextStyle = TextStyle::new(13.0, 440.0).lh(20.0);
pub const LINE: TextStyle = TextStyle::new(13.0, 440.0).lh(24.0);

impl App {
    pub fn open_settings_page(&mut self, page: Page, tx: &mut Tx) {
        tx.instant();
        self.settings_pages.page = page;
        self.settings_pages.menu = None;
        match page {
            Page::Providers | Page::Models => self.sync_catalogs(None, |_, _, _| {}),
            Page::Extensions => {
                self.sync_mcp("");
                extensions::load(self);
            }
            Page::General => {}
        }
        if page == Page::Models {
            self.settings_pages.search.focused = true;
        }
    }
}

/// `.settings-tab-header`: the title and its description, an action at the end.
pub fn header(p: &Pal, title: &str, description: &str) -> El {
    row().key("settings-tab-header").w_full().gap(16.0)
        .child(col().grow(1.0).shrink().min_w(0.0).gap(4.0)
            .child(text(title.to_owned()).font(TITLE).color(p.text))
            .child(text(description.to_owned()).font(DESCRIPTION).color(p.muted).h(24.0).mt(-4.0)))
}

/// `.settings-section`: its title over its content, 16 pt apart.
pub fn section(p: &Pal, key: &str, title: Option<&str>, content: El) -> El {
    col().key(key.to_owned()).w_full().gap(16.0).children(title.map(|t| text(t.to_owned()).font(NAME).color(p.text))).child(content)
}

/// `[data-component="settings-list"][data-variant="catalog"]`: a `layer-01` card with 8 pt
/// corners, a 0.5 pt `border-muted` ring inside and 16 pt in from each side; its rows split by
/// 0.5 pt `border-base` rules.
pub fn catalog(p: &Pal, key: &str, rows: Vec<El>) -> El {
    let n = rows.len();
    let mut list = col().key(key.to_owned()).w_full().px(16.0).radius(8.0).fill(p.layer1).box_shadow(Pal::ring_in(p.border_muted));
    for (at, r) in rows.into_iter().enumerate() {
        list = list.child(r);
        if at + 1 < n {
            list = list.child(el().w_full().h(0.5).fill(p.border));
        }
    }
    list
}

/// A page's scrolling body beneath its header, as the page view the shell will host.
pub fn page(key: &str, header: El, body: El) -> El {
    col().key(key.to_owned()).w_full().grow(1.0).shrink().min_h(0.0)
        .child(col().w_full().px(4.0).child(header))
        .child(col().key(format!("{key}-scroll")).w_full().grow(1.0).shrink().min_h(0.0).px(4.0)
            .scroll(ScrollSpec { stick_bottom: false })
            .clip_fade(0.0, 24.0, 0.0, 0.0)
            .child(body.pb(80.0)))
}

/// The pages the list beside them offers, with Desktop's nav icons.
const NAV: [(Page, &str, &rune::Icon); 4] = [
    (Page::General, "General", &ic::SLIDERS),
    (Page::Providers, "Providers", &ic::PROVIDERS),
    (Page::Models, "Models", &ic::MODELS),
    (Page::Extensions, "Extensions", &ic::EXTENSIONS),
];

/// The list beside the pages (`.settings-nav-groups`): 28 pt rows, the open one on `layer-03`.
pub fn nav(s: &App, p: &Pal) -> El {
    let mut list = col().key("settings-nav").w(240.0).gap(4.0);
    for (page, label, glyph) in NAV {
        let on = s.settings_pages.page == page;
        list = list.child(row().key(format!("settings-nav-{}", label.to_lowercase())).role("tab").label(label).w_full().h(28.0).px(6.0).gap(6.0).center_y().radius(4.0)
            .fill(if on { p.layer3 } else { Color::TRANSPARENT })
            .hover(if on { p.layer3 } else { p.hover })
            .on_press_tx(move |s: &mut App, tx: &mut Tx| s.open_settings_page(page, tx))
            .child(icon(glyph, 16.0).color(if on { p.icon } else { p.icon_muted }))
            .child(text(label).font(NAME_REGULAR).color(if on { p.text } else { p.muted })));
    }
    list
}

/// The open page beside the list, laid out as Desktop's settings screen lays its column out.
pub fn view(s: &App, p: &Pal) -> Option<El> {
    let content = match s.settings_pages.page {
        Page::General => return None,
        Page::Providers => providers::view(s, p),
        Page::Models => models::view(s, p),
        Page::Extensions => extensions::view(s, p),
    };
    Some(row().key("settings-pages").w_full().grow(1.0).shrink().min_h(0.0).justify_center().pr(24.0)
        .child(col().w(328.0).pl(24.0).pr(64.0).child(nav(s, p)))
        .child(col().grow(1.0).shrink().min_w(0.0).max_w(728.0).min_h(0.0).child(content)))
}
