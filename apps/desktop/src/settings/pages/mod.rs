//! Settings' pages (`settings/pages.ts` and the plugins' `SettingsPage`s), each a view the
//! two-pane shell (`settings/shell.rs`) hosts beside its rail, and the pieces they share:
//! `.settings-tab-header`, `.settings-tab-body`, sections, lists, rows and their controls.

pub mod about;
pub mod appearance;
pub mod experimental;
pub mod extensions;
pub mod general;
pub mod models;
pub mod notifications;
pub mod pairing;
pub mod projects;
pub mod providers;
pub mod servers;
pub mod shortcuts;
pub mod timeline;
pub mod whats_new;
pub mod worktrees;

use rune::{Color, El, Icon, TextEdit, TextStyle, Tx, col, el, icon, row, text};
use rune_kit::{Select, TextInput};
use rune_theme_opencode::icons as ic;

use crate::App;
use crate::palette::{Pal, kit};
use crate::popup::{self, Menu};

/// A page of Settings: Desktop's root tabs (`SettingsRootTab`), a page a plugin adds (by its
/// id), or one built in here that Desktop's GUI plugins add there (Pairing, What's new).
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub enum Page {
    Providers,
    Models,
    Extensions,
    Projects,
    Worktrees,
    /// Preferences (`general`).
    #[default]
    General,
    Appearance,
    Notifications,
    Shortcuts,
    Pairing,
    Servers,
    WhatsNew,
    Experimental,
    About,
    /// A plugin's own page (`SettingsPage` without a host page), by its contribution key.
    Plugin(String),
}

impl Page {
    /// The tab id Desktop's route names it by (`?tab=`), for settings search and plugins.
    pub fn id(&self) -> &str {
        match self {
            Page::Providers => "providers",
            Page::Models => "models",
            Page::Extensions => "extensions",
            Page::Projects => "projects",
            Page::Worktrees => "workspaces",
            Page::General => "general",
            Page::Appearance => "appearance",
            Page::Notifications => "notifications",
            Page::Shortcuts => "shortcuts",
            Page::Pairing => "pairing",
            Page::Servers => "servers",
            Page::WhatsNew => "whats-new",
            Page::Experimental => "experimental",
            Page::About => "about",
            Page::Plugin(id) => id,
        }
    }

    /// The page tab `id` names; a plugin's page otherwise. `plugins` for the old Plugins page,
    /// which is Extensions' Plugins tab now.
    pub fn parse(id: &str) -> Page {
        match id {
            "providers" => Page::Providers,
            "models" => Page::Models,
            "extensions" | "plugins" => Page::Extensions,
            "projects" => Page::Projects,
            "workspaces" | "worktrees" => Page::Worktrees,
            "general" | "preferences" | "" => Page::General,
            "appearance" => Page::Appearance,
            "notifications" => Page::Notifications,
            "shortcuts" | "keybinds" => Page::Shortcuts,
            "pairing" => Page::Pairing,
            "servers" | "server" => Page::Servers,
            "whats-new" => Page::WhatsNew,
            "experimental" => Page::Experimental,
            "about" => Page::About,
            other => Page::Plugin(other.to_owned()),
        }
    }

    /// Its name in the rail (`pageLabels`).
    pub fn label(&self) -> &'static str {
        match self {
            Page::Providers => "Providers",
            Page::Models => "Models",
            Page::Extensions => "Extensions",
            Page::Projects => "Projects",
            Page::Worktrees => "Worktrees",
            Page::General => "Preferences",
            Page::Appearance => "Appearance",
            Page::Notifications => "Notifications",
            Page::Shortcuts => "Shortcuts",
            Page::Pairing => "Pairing",
            Page::Servers => "Server",
            Page::WhatsNew => "What's new",
            Page::Experimental => "Experimental",
            Page::About => "About",
            Page::Plugin(_) => "",
        }
    }

    /// Its rail icon (`pageIcons`).
    pub fn icon(&self) -> &'static Icon {
        match self {
            Page::Providers => &ic::PROVIDERS,
            Page::Models => &ic::MODELS,
            Page::Extensions | Page::Plugin(_) => &ic::EXTENSIONS,
            Page::Projects => &ic::FOLDER,
            Page::Worktrees => &ic::OUTLINE_WORKTREE,
            Page::General => &ic::SLIDERS,
            Page::Appearance => &ic::APPEARANCE,
            Page::Notifications => &ic::NOTIFICATIONS,
            Page::Shortcuts => &ic::KEYBOARD,
            Page::Pairing => &ic::OUTLINE_PAIR,
            Page::Servers => &ic::SERVER,
            Page::WhatsNew => &ic::OUTLINE_LIST_CHANGELOG,
            Page::Experimental => &ic::FLASK,
            Page::About => &ic::INFO,
        }
    }
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
    pub collapsed: models::Folds,
    pub tab: ExtensionsTab,
    /// The server's plugins and skills, once read.
    pub plugins: Option<Vec<String>>,
    pub skills: Option<Vec<String>>,
    /// A provider's account menu or options open on Providers: (provider, which).
    pub menu: Option<(String, providers::Menu)>,
    /// The account being renamed, and its field.
    pub renaming: Option<String>,
    pub rename: TextEdit,
    /// Settings search in the rail (`settings/search.rs`).
    pub find: super::search::Find,
    /// Appearance's three font fields, as typed.
    pub fonts: appearance::Fields,
    pub shortcuts: shortcuts::State,
    pub projects: projects::State,
    pub worktrees: worktrees::State,
    pub servers: servers::State,
    pub pairing: pairing::State,
    /// Preferences' Timeline section.
    pub timeline: timeline::State,
}

impl State {
    /// Takes focus out of every field on the pages.
    pub fn blur_fields(&mut self) {
        self.search.focused = false;
        self.rename.focused = false;
        self.find.query.focused = false;
        self.fonts.blur();
        self.shortcuts.filter.focused = false;
        self.projects.filter.focused = false;
    }
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
    /// Shows `page` (Desktop's `surface.select`): the screen scrolls back to its top, and a page
    /// that reads the server starts reading.
    pub fn show_settings_page(&mut self, page: Page) {
        self.settings_pages.menu = None;
        self.settings_pages.shortcuts.capture = None;
        self.settings_pages.blur_fields();
        match &page {
            Page::Providers | Page::Models => self.sync_catalogs(None, |_, _, _| {}),
            Page::Extensions => {
                self.sync_mcp("");
                extensions::load(self);
            }
            Page::Appearance => self.settings_pages.fonts = appearance::Fields::from(&self.settings),
            Page::Servers => servers::load(self),
            Page::Worktrees => worktrees::load(self),
            Page::Pairing => pairing::load(self),
            _ => {}
        }
        if page == Page::Models {
            self.settings_pages.search.focused = true;
        }
        self.settings_pages.page = page;
    }

    pub fn open_settings_page(&mut self, page: Page, tx: &mut Tx) {
        tx.instant();
        tx.scroll_to_instant("settings", 0.0, 0.0);
        super::search::leave(self);
        self.show_settings_page(page);
    }
}

/// `.settings-tab-header`: the title and its description, an action at the end.
pub fn header(p: &Pal, title: &str, description: &str) -> El {
    header_with(p, title, description, None)
}

/// [`header`] with the page's actions at its end, on the title's line.
pub fn header_with(p: &Pal, title: &str, description: &str, actions: Option<El>) -> El {
    row().key("settings-tab-header").w_full().gap(16.0)
        .child(col().grow(1.0).shrink().min_w(0.0).gap(4.0)
            .child(text(title.to_owned()).font(TITLE).color(p.text))
            .child(text(description.to_owned()).font(DESCRIPTION).color(p.muted)))
        .children(actions.map(|a| row().key("settings-tab-header-actions").h(20.0).gap(8.0).center_y().self_start().child(a)))
}

/// [`header`] with an action centred on the title and description (`settings-tab-header--stacked`,
/// where the action sits in the row itself rather than in `settings-tab-header-actions`).
pub fn header_beside(p: &Pal, title: &str, description: &str, action: El) -> El {
    header(p, title, description).center_y().child(action)
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

/// A page: its sticky header over its body (`.settings-panel`, 4 pt in for the cards' rings).
/// The whole screen scrolls (`settings-screen` at 800 pt and wider); the header holds to its
/// top on `bg-base`, a 24 pt fade below it, while the body passes under.
pub fn page(p: &Pal, key: &str, header: El, body: El) -> El {
    page_with(p, key, header, body, false)
}

/// [`page`]; `sectioned` gives the body Desktop's `settings-tab-body--sectioned` 32 pt top.
pub fn page_with(p: &Pal, key: &str, header: El, body: El, sectioned: bool) -> El {
    let fade = el().key("settings-tab-header-fade").pin(Some(0.0), None, Some(0.0), Some(-24.0)).h(24.0).gradient(p.bg_base, p.bg_base.alpha(0.0));
    col().key(key.to_owned()).w_full().px(4.0)
        .child(col().key("settings-tab-header-band").w_full().pt(48.0).fill(p.bg_base).sticky_top(0.0).child(header).child(fade))
        .child(col().key(format!("{key}-body")).w_full().gap(32.0).pt(if sectioned { 32.0 } else { 24.0 }).pb(80.0).child(body))
}

/// Desktop's `Select` in a settings row: the current choice and its chevron, opening the
/// choices with the current one checked below its end (`placement="bottom-end"`, 6 pt gutter).
pub fn select(s: &App, p: &Pal, key: &str, label: &str, options: Vec<(String, String)>, current: &str, pick: fn(&mut App, &str, &mut Tx)) -> El {
    let shown = options.iter().find(|(id, _)| id == current).map_or_else(String::new, |(_, l)| l.clone());
    let anchor = format!("{key}-trigger");
    let label = label.to_owned();
    let current = current.to_owned();
    let open = s.popup.open_at(&anchor);
    Select::new(key.to_owned(), &[shown.as_str()]).current(Some(&shown)).label(label.clone()).open(open).trigger(kit(p))
        .on_press_tx(move |s: &mut App, tx: &mut Tx| {
            let menu = Menu::picker(label.clone(), options.clone(), &current);
            popup::open(s, menu, anchor.clone(), tx, move |s: &mut App, picked, tx| {
                if let Some(id) = picked {
                    tx.instant();
                    pick(s, id, tx);
                }
            });
        })
}

/// A select over a closed choice (`prefs::Choice`).
pub fn choice<C: super::Choice>(s: &App, p: &Pal, key: &str, label: &str, current: C, pick: fn(&mut App, &str, &mut Tx)) -> El {
    select(s, p, key, label, C::ALL.iter().map(|c| (c.name().to_owned(), c.label().to_owned())).collect(), current.name(), pick)
}

/// `.settings-filter-search` (`SettingsSearchField`): 36 pt, `layer-02` at 60% (full under the
/// pointer or while typed in), the glass 16 pt in, a clear button once typed.
pub fn filter_field(s: &App, p: &Pal, key: &str, placeholder: &str, edit: &TextEdit, field: fn(&mut App) -> &mut TextEdit) -> El {
    let _ = s;
    let typed = !edit.text.is_empty();
    let clear = typed.then(|| {
        row().key("clear").role("button").label("Clear").size(20.0, 20.0).center().radius(4.0)
            .on_press_tx(move |s: &mut App, tx: &mut Tx| {
                tx.instant();
                let e = field(s);
                *e = TextEdit::new("");
                e.focused = true;
            })
            .child(icon(&ic::CIRCLE_XMARK, 16.0).color(p.icon_faint))
    });
    row().key(key.to_owned()).w_full().h(36.0).mx(1.0).pr(10.0).gap(8.0).center_y().radius(6.0)
        .fill(if edit.focused || typed { p.layer2 } else { p.search_bg })
        .hover(p.layer2)
        .child(el().pl(16.0).child(icon(&ic::MAGNIFYING_GLASS, 14.0).color(p.icon_muted)))
        .child(rune::text_input(edit, NAME_REGULAR).key(format!("{key}-input")).single_line().grow(1.0).shrink().min_w(0.0)
            .color(p.text).placeholder(placeholder.to_owned(), p.faint).caret(p.text, p.selection).label(placeholder.to_owned())
            .on_edit(field)
            .on_input_change(|_: &mut App, tx: &mut Tx| tx.instant()))
        .children(clear)
}

/// `TextInput appearance="base"` at 220 pt, in its own font, as Appearance's font rows have it.
pub fn text_field(p: &Pal, key: &str, label: &str, placeholder: &str, edit: &TextEdit, style: TextStyle, field: fn(&mut App) -> &mut TextEdit, change: fn(&mut App, &mut Tx)) -> El {
    TextInput::new(edit).placeholder(placeholder.to_owned()).label(label.to_owned()).width(Some(220.0)).text_style(style).height(28.0)
        .on_edit(field).on_change(change).build(kit(p)).key(key.to_owned())
}

/// `button-v2` ghost or ghost-muted with an optional leading icon (Add project, Add server,
/// Reset to defaults, History on GitHub).
pub fn ghost_button(p: &Pal, key: &str, label: &str, glyph: Option<&'static Icon>, muted: bool, enabled: bool, press: impl Fn(&mut App, &mut Tx) + 'static) -> El {
    let ink = if !enabled { p.faint } else if muted { p.muted } else { p.text };
    let b = row().key(key.to_owned()).role("button").label(label.to_owned()).h(28.0).px(if glyph.is_some() { 8.0 } else { 11.0 }).gap(6.0).center_y().radius(6.0)
        .children(glyph.map(|g| icon(g, 16.0).color(if enabled { p.icon_muted } else { p.icon_faint })))
        .child(text(label.to_owned()).font(super::super::view::UI).color(ink));
    if !enabled {
        return b.disabled(true);
    }
    b.hover(p.hover).press(p.pressed).on_press_tx(move |s: &mut App, tx: &mut Tx| {
        tx.instant();
        press(s, tx);
    })
}

/// `SettingsSearchEmpty`: "No results for" and the query in quotes.
pub fn no_results(p: &Pal, key: &str, query: &str) -> El {
    row().key(key.to_owned()).w_full().h(28.0).pl(8.0).pr(6.0).gap(4.0).center_y()
        .child(text("No results for").font(NAME_REGULAR).color(p.muted))
        .child(text(format!("\u{201c}{}\u{201d}", query.trim())).font(NAME_REGULAR).color(p.text).truncate())
}

/// The content of the open page.
pub fn view(s: &App, p: &Pal) -> El {
    match &s.settings_pages.page {
        Page::Providers => providers::view(s, p),
        Page::Models => models::view(s, p),
        Page::Extensions => extensions::view(s, p),
        Page::Projects => projects::view(s, p),
        Page::Worktrees => worktrees::view(s, p),
        Page::General => general::view(s, p),
        Page::Appearance => appearance::view(s, p),
        Page::Notifications => notifications::view(s, p),
        Page::Shortcuts => shortcuts::view(s, p),
        Page::Pairing => pairing::view(s, p),
        Page::Servers => servers::view(s, p),
        Page::WhatsNew => whats_new::view(s, p),
        Page::Experimental => experimental::view(s, p),
        Page::About => about::view(s, p),
        Page::Plugin(id) => plugin_page(s, p, id),
    }
}

/// A plugin's own page: its title, then what it draws.
fn plugin_page(s: &App, p: &Pal, id: &str) -> El {
    let (title, body) = crate::plugin_host::settings_page(s, p, id).unwrap_or_else(|| (id.to_owned(), el()));
    page(p, "settings-plugin-page", header(p, &title, ""), body)
}

/// `row` with the settings search's highlight behind it once a result revealed `target`
/// (`[data-search-target]::before`): `layer-03` reaching `inset` past the row each side, fading
/// out over 1.8 s.
pub fn revealable(s: &App, p: &Pal, target: &str, inset: f32, row: El) -> El {
    let Some(lit) = super::search::reveal_of(s, target) else { return row };
    let fade = rune::Curve::tween(1.8, rune::Easing::cubic(0.0, 0.0, 0.58, 1.0));
    let mark = el().key("search-reveal").pin(Some(-inset), Some(0.0), Some(-inset), Some(0.0)).radius(6.0).fill(p.layer3)
        .opacity(if lit { 1.0 } else { 0.0 }).opacity_motion(if lit { rune::Anim::INSTANT } else { fade.into() });
    col().w_full().child(mark).child(row)
}

/// Rows keyed by their search target, each able to show the search's highlight.
pub fn rows(s: &App, p: &Pal, inset: f32, rows: Vec<(&str, El)>) -> Vec<El> {
    rows.into_iter().map(|(target, r)| revealable(s, p, target, inset, r)).collect()
}

#[allow(dead_code)]
fn unused(_: Color) {}
