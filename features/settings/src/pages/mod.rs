//! Settings' pages (`settings/pages.ts` and the plugins' `SettingsPage`s), each a view the
//! two-pane shell (`shell.rs`) hosts beside its rail, and the pieces they share:
//! `.settings-tab-header`, `.settings-tab-body`, sections, lists, rows and their controls.

pub mod about;
pub mod appearance;
pub mod experimental;
pub mod extensions;
pub mod general;
pub mod models;
pub mod notifications;
pub mod plugin_updates;
pub mod project;
pub mod project_extensions;
pub mod projects;
pub mod providers;
pub mod servers;
pub mod shortcuts;
pub mod timeline;
pub mod worktrees;

use opencode_i18n::t;
use opencode_ui::popup::{self, Menu, MenuButton as _, PopupHost};
use rune::{El, Icon, TextEdit, TextStyle, Tx, col, el, icon, row, rule, text};
use rune_kit::{Select, TextInput};
use rune_theme_opencode::icons as ic;
use opencode_ui::palette::{Pal, kit};
use opencode_ui::style::{UI, UI_MED};

use crate::host::SettingsHost;

/// A page of Settings: Desktop's root tabs (`SettingsRootTab`), a plugin's page, or a project's tab.
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
    Servers,
    Experimental,
    About,
    /// A plugin's own page (`SettingsPage` without a host page), by its contribution key.
    Plugin(String),
    /// A project's own pages (`SettingsProjectView`), by its folder.
    Project(String, project::Tab),
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
            Page::Servers => "servers",
            Page::Experimental => "experimental",
            Page::About => "about",
            Page::Plugin(id) => id,
            Page::Project(_, tab) => tab.id(),
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
            "servers" | "server" => Page::Servers,
            "experimental" => Page::Experimental,
            "about" => Page::About,
            // A project's page names no project; its list stands in.
            "project-general" | "project-workspaces" | "project-extensions" => Page::Projects,
            other => Page::Plugin(other.to_owned()),
        }
    }

    /// Its name (`pageLabels`), as settings search places a result.
    pub fn label(&self) -> &'static str {
        match self {
            Page::Providers => t!("settings.providers.title"),
            Page::Models => t!("settings.models.title"),
            Page::Extensions => t!("settings.tab.extensions"),
            Page::Projects => t!("settings.tab.projects"),
            Page::Worktrees => t!("settings.tab.workspaces"),
            Page::General => t!("settings.tab.preferences"),
            Page::Appearance => t!("settings.general.section.appearance"),
            Page::Notifications => t!("settings.tab.notifications"),
            Page::Shortcuts => t!("settings.shortcuts.title"),
            Page::Servers => t!("settings.section.server"),
            Page::Experimental => t!("settings.tab.experimental"),
            Page::About => t!("settings.tab.about"),
            Page::Plugin(_) => "",
            Page::Project(_, tab) => tab.label(),
        }
    }

    /// Its name in the rail (`rootClientTabs`, `serverTabs`), which calls Shortcuts by its short name.
    pub fn tab_label(&self) -> &'static str {
        match self {
            Page::Shortcuts => t!("settings.tab.shortcuts"),
            other => other.label(),
        }
    }

    /// It shows one server's settings (`serverTabs` and a project's pages): Settings' own server
    /// ([`servers::Scope`]), with the switcher, or the unavailable page while it can't load.
    pub fn per_server(&self) -> bool {
        matches!(self, Page::Providers | Page::Models | Page::Extensions | Page::Projects | Page::Worktrees | Page::Project(..))
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
            Page::Servers => &ic::SERVER,
            Page::Experimental => &ic::FLASK,
            Page::About => &ic::INFO,
            Page::Project(_, tab) => tab.icon(),
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
    pub plugins: Option<Vec<plugin_updates::PluginRow>>,
    /// Plugin update checks and installs on the Plugins tab.
    pub updates: plugin_updates::Machine,
    pub skills: Option<Vec<String>>,
    /// The Connected row whose account menu, options or rename field is open on Providers.
    pub accounts: providers::Accounts,
    /// Settings search in the rail (`search.rs`).
    pub find: crate::search::Find,
    /// Appearance's three font fields, as typed.
    pub fonts: appearance::Fields,
    /// About's wordmark, which scrambles each time the page shows.
    pub wordmark: about::Wordmark,
    pub shortcuts: shortcuts::State,
    pub projects: projects::State,
    pub worktrees: worktrees::State,
    pub servers: servers::State,
    /// The server the per-server pages show, and their Retry while it can't be reached.
    pub server: servers::Scope,
    /// Preferences' Timeline section.
    pub timeline: timeline::State,
    /// A project's own pages.
    pub project: project::State,
    /// Fallback target so provider rename `on_edit` never panics if fired after close.
    scratch_account_rename: TextEdit,
}

impl State {
    /// Takes focus out of every field on the pages.
    pub fn blur_fields(&mut self) {
        self.search.focused = false;
        self.accounts.blur();
        self.find.query.focused = false;
        self.fonts.blur();
        self.shortcuts.filter.focused = false;
        self.projects.filter.focused = false;
        self.project.name.focused = false;
        self.project.startup.focused = false;
        self.project.rename.focused = false;
    }

    pub(crate) fn account_rename_field_mut(&mut self) -> &mut TextEdit {
        self.accounts.field_mut().unwrap_or(&mut self.scratch_account_rename)
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

/// Shows `page` (Desktop's `surface.select`): resets ephemeral page UI state and kicks off loads
/// for pages that read from the server.
pub fn show_page(host: &mut dyn SettingsHost, page: Page) {
    let was_confirm = matches!(host.settings_pages().accounts, providers::Accounts::Confirm(_));
    let _ = host.settings_pages_mut().accounts.step(providers::Event::Close);
    if was_confirm {
        host.on_overlay_closed(crate::PROVIDER_REMOVAL_OVERLAY_ID);
    }
    host.settings_pages_mut().shortcuts.capture = None;
    host.settings_pages_mut().blur_fields();
    let key = host.settings_pages().server.key.clone();
    if page.per_server() && host.is_server_listed(&key) {
        host.switch_server(&key);
    }
    match &page {
        Page::Providers | Page::Models => host.sync_catalogs(None),
        Page::Extensions => {
            host.sync_mcp("");
            plugin_updates::dispatch(host, plugin_updates::Event::Reset);
            extensions::load(host);
        }
        Page::Appearance => {
            let fields = appearance::Fields::from(host.settings());
            host.settings_pages_mut().fonts = fields;
        }
        Page::Servers => servers::load(host),
        Page::Worktrees => {
            host.settings_pages_mut().worktrees.filter = worktrees::Filter::All;
            worktrees::load(host);
        }
        Page::Project(dir, tab) => {
            let dir = dir.clone();
            project::load(host, &dir, *tab);
        }
        _ => {}
    }
    if host.settings_pages().page == Page::About {
        host.settings_pages_mut().wordmark.activate(false);
    }
    host.settings_pages_mut().page = page;
}

/// Navigates to `page` inside Settings, scrolling to top and starting the About scramble when appropriate.
pub fn open_page(host: &mut dyn SettingsHost, page: Page, tx: &mut Tx) {
    tx.instant();
    tx.scroll_to_instant("settings", 0.0, 0.0);
    crate::search::leave(host);
    let about = page == Page::About;
    show_page(host, page);
    if about {
        host.settings_pages_mut().wordmark.activate(true);
    }
}

/// `.settings-tab-header`: the title and its description, an action at the end.
pub fn header(p: &Pal, title: &str, description: &str) -> El {
    header_with(p, title, description, None)
}

/// [`header`] with the page's actions at its end, on the title's line.
pub fn header_with(p: &Pal, title: &str, description: &str, actions: Option<El>) -> El {
    row()
        .key("settings-tab-header")
        .w_full()
        .gap(16.0)
        .child(
            col()
                .grow(1.0)
                .shrink()
                .min_w(0.0)
                .gap(4.0)
                .child(text(title.to_owned()).font(TITLE).color(p.text))
                .child(text(description.to_owned()).font(DESCRIPTION).color(p.muted).w_full().wrap()),
        )
        .children(actions.map(|a| row().key("settings-tab-header-actions").h(20.0).gap(8.0).center_y().self_start().child(a)))
}

/// [`header`] with an action centred on the title and description (`settings-tab-header--stacked`).
pub fn header_beside(p: &Pal, title: &str, description: &str, action: El) -> El {
    header(p, title, description).center_y().child(action)
}

/// `.settings-section`: its title over its content, 16 pt apart.
pub fn section(p: &Pal, key: &str, title: Option<&str>, content: El) -> El {
    col()
        .key(key.to_owned())
        .w_full()
        .gap(16.0)
        .children(title.map(|t| text(t.to_owned()).font(NAME).color(p.text)))
        .child(content)
}

/// `[data-component="settings-list"][data-variant="catalog"]`: a `layer-01` card with 8 pt
/// corners, a 0.5 pt `border-muted` ring inside and 16 pt in from each side; its rows split by
/// 0.5 pt `border-base` rules.
pub fn catalog(p: &Pal, key: &str, rows: Vec<El>) -> El {
    catalog_with(p, key, rows, true)
}

/// [`catalog`], its rules drawn by the card (`rules`) or by rows that hold their own.
pub fn catalog_with(p: &Pal, key: &str, rows: Vec<El>, rules: bool) -> El {
    let n = rows.len();
    let mut list = col().key(key.to_owned()).w_full().px(16.0).radius(8.0).fill(p.layer1).box_shadow(Pal::ring_in(p.border_muted));
    for (at, r) in rows.into_iter().enumerate() {
        list = list.child(r);
        if rules && at + 1 < n {
            list = list.child(rule(0.5, p.border));
        }
    }
    list
}

/// A page: its sticky header over its body (`.settings-panel`, 4 pt in for the cards' rings).
pub fn page(p: &Pal, key: &str, header: El, body: El) -> El {
    page_with(p, key, header, body, false)
}

/// [`page`]; `sectioned` gives the body Desktop's `settings-tab-body--sectioned` 32 pt top.
pub fn page_with(p: &Pal, key: &str, header: El, body: El, sectioned: bool) -> El {
    let fade = el()
        .key("settings-tab-header-fade")
        .pin(Some(0.0), None, Some(0.0), Some(-24.0))
        .h(24.0)
        .gradient(p.bg_base, p.bg_base.alpha(0.0));
    col()
        .key(key.to_owned())
        .w_full()
        .px(4.0)
        .child(
            col()
                .key("settings-tab-header-band")
                .w_full()
                .pt(if crate::compact() { 12.0 } else { 48.0 })
                .fill(p.bg_base)
                .sticky_top(0.0)
                .child(header)
                .child(fade),
        )
        .child(col().key(format!("{key}-body")).w_full().gap(32.0).pt(if sectioned { 32.0 } else { 24.0 }).pb(80.0).child(body))
}

/// Desktop's `Select` in a settings row.
pub fn select<H: SettingsHost + PopupHost + 'static>(
    s: &H,
    p: &Pal,
    key: &str,
    label: &str,
    options: Vec<(String, String)>,
    current: &str,
    pick: fn(&mut dyn SettingsHost, &str, &mut Tx),
) -> El {
    select_some(s, p, key, label, options.into_iter().map(|(id, title)| (id, title, true)).collect(), current, pick)
}

/// [`select`] whose choices can be disabled (`(id, title, enabled)`).
pub fn select_some<H: SettingsHost + PopupHost + 'static>(
    s: &H,
    p: &Pal,
    key: &str,
    label: &str,
    options: Vec<(String, String, bool)>,
    current: &str,
    pick: fn(&mut dyn SettingsHost, &str, &mut Tx),
) -> El {
    let shown = options.iter().find(|(id, _, _)| id == current).map_or_else(String::new, |(_, l, _)| l.clone());
    let anchor = format!("{key}-trigger");
    let label = label.to_owned();
    let current = current.to_owned();
    let open = s.popup().open_at(&anchor);
    let anchor_key = anchor.clone();
    Select::new(key.to_owned(), &[shown.as_str()])
        .current(Some(&shown))
        .label(label.clone())
        .open(open)
        .trigger(kit(p))
        .menu_button(anchor_key, move |s: &mut H, tx: &mut Tx| {
            let menu = Menu::picker_some(label.clone(), options.clone(), &current);
            popup::open(s, menu, anchor.clone(), tx, move |s: &mut H, picked, tx| {
                if let Some(id) = picked {
                    tx.instant();
                    pick(s, id, tx);
                }
            });
        })
}

/// A select over a closed choice (`prefs::Choice`).
pub fn choice<H: SettingsHost + PopupHost + 'static, C: crate::Choice>(
    s: &H,
    p: &Pal,
    key: &str,
    label: &str,
    current: C,
    pick: fn(&mut dyn SettingsHost, &str, &mut Tx),
) -> El {
    select(s, p, key, label, C::ALL.iter().map(|c| (c.name().to_owned(), c.label().to_owned())).collect(), current.name(), pick)
}

fn clear_filter_field(s: &mut dyn SettingsHost, field: fn(&mut State) -> &mut TextEdit, tx: &mut Tx) {
    tx.instant();
    let e = field(s.settings_pages_mut());
    *e = TextEdit::new("");
    e.focused = true;
}

/// `.settings-filter-search` (`SettingsSearchField`): 36 pt, `layer-02` at 60%, glass 16 pt in,
/// clear button once typed.
pub fn filter_field<H: SettingsHost + 'static>(
    p: &Pal,
    key: &str,
    placeholder: &str,
    edit: &TextEdit,
    field: fn(&mut State) -> &mut TextEdit,
) -> El {
    let typed = !edit.text.is_empty();
    let clear = typed.then(|| {
        row()
            .key("clear")
            .role("button")
            .label_static(t!("common.clear"))
            .size(20.0, 20.0)
            .center()
            .radius(4.0)
            .on_press_tx(move |s: &mut H, tx: &mut Tx| clear_filter_field(s, field, tx))
            .child(icon(&ic::CIRCLE_XMARK, 16.0).color(p.icon_faint))
    });
    row()
        .key(key.to_owned())
        .w_full()
        .h(36.0)
        .mx(1.0)
        .pr(10.0)
        .gap(8.0)
        .center_y()
        .radius(6.0)
        .fill(if edit.focused || typed { p.layer2 } else { p.search_bg })
        .hover(p.layer2)
        .child(el().pl(16.0).child(icon(&ic::MAGNIFYING_GLASS, 14.0).color(p.icon_muted)))
        .child(
            opencode_ui::style::field(edit, NAME_REGULAR)
                .key(format!("{key}-input"))
                .single_line()
                .grow(1.0)
                .shrink()
                .min_w(0.0)
                .color(p.text)
                .placeholder(placeholder.to_owned(), p.faint)
                .caret(p.text, p.selection)
                .selection_style(p.field_selection)
                .label(placeholder.to_owned())
                .on_edit(move |s: &mut H| field(s.settings_pages_mut()))
                .on_input_change(|_: &mut H, tx: &mut Tx| tx.instant()),
        )
        .children(clear)
}

/// `TextInput appearance="base"` at 220 pt, in its own font, as Appearance's font rows have it.
#[allow(clippy::too_many_arguments)]
pub fn text_field<H: SettingsHost + 'static>(
    p: &Pal,
    key: &str,
    label: &str,
    placeholder: &str,
    edit: &TextEdit,
    style: TextStyle,
    field: fn(&mut State) -> &mut TextEdit,
    change: fn(&mut dyn SettingsHost, &mut Tx),
) -> El {
    TextInput::new(edit)
        .placeholder(placeholder.to_owned())
        .label(label.to_owned())
        .width(Some(220.0))
        .text_style(style)
        .height(28.0)
        .on_edit(move |s: &mut H| field(s.settings_pages_mut()))
        .on_change(move |s: &mut H, tx: &mut Tx| change(s, tx))
        .build(kit(p))
        .key(key.to_owned())
}

/// `button-v2` ghost or ghost-muted with an optional leading icon.
pub fn ghost_button<H: SettingsHost + 'static>(
    p: &Pal,
    key: &str,
    label: &str,
    glyph: Option<&'static Icon>,
    muted: bool,
    enabled: bool,
    press: impl Fn(&mut dyn SettingsHost, &mut Tx) + 'static,
) -> El {
    let b = ghost_face(p, key, label, glyph, muted, enabled, false);
    if !enabled {
        return b;
    }
    b.on_press_tx(move |s: &mut H, tx: &mut Tx| {
        tx.instant();
        press(s, tx);
    })
}

/// [`ghost_button`] as a menu's trigger: while its menu is open it keeps the pressed fill.
#[allow(clippy::too_many_arguments)]
pub fn ghost_menu_button<H: SettingsHost + PopupHost + 'static>(
    p: &Pal,
    key: &str,
    label: &str,
    glyph: Option<&'static Icon>,
    muted: bool,
    enabled: bool,
    open: bool,
    press: impl Fn(&mut H, &mut Tx) + 'static,
) -> El {
    let b = ghost_face(p, key, label, glyph, muted, enabled, open);
    if !enabled {
        return b;
    }
    b.menu_button(key.to_owned(), move |s: &mut H, tx: &mut Tx| {
        tx.instant();
        press(s, tx);
    })
}

fn ghost_face(p: &Pal, key: &str, label: &str, glyph: Option<&'static Icon>, muted: bool, enabled: bool, open: bool) -> El {
    let ink = if !enabled { p.faint } else if muted && !open { p.muted } else { p.text };
    let glyph_ink = if !enabled { p.icon_faint } else if open { p.icon } else { p.icon_muted };
    let b = row()
        .key(key.to_owned())
        .role("button")
        .label(label.to_owned())
        .h(28.0)
        .px(if glyph.is_some() { 8.0 } else { 11.0 })
        .gap(6.0)
        .center_y()
        .radius(6.0)
        .children(glyph.map(|g| icon(g, 16.0).color(glyph_ink)))
        .child(text(label.to_owned()).font(if muted { UI } else { UI_MED }.features("\"tnum\"")).color(ink));
    if !enabled {
        return b.disabled(true);
    }
    let b = if open { b.fill(p.pressed) } else { b };
    b.hover(p.hover).press(p.pressed)
}

/// `SettingsSearchEmpty`: "No results for" and the query in quotes.
pub fn no_results(p: &Pal, key: &str, query: &str) -> El {
    row().key(key.to_owned()).w_full().h(28.0).pl(8.0).pr(6.0).gap(4.0).center_y().children(no_results_words(p, query))
}

/// `settings.search.empty`'s words, muted, around the query in quotes, which truncates.
pub fn no_results_words(p: &Pal, query: &str) -> Vec<El> {
    let quoted = t!("settings.search.empty.query", query = query.trim());
    opencode_i18n::parts(t!("settings.search.empty"))
        .filter_map(|part| match part {
            opencode_i18n::Part::Text(words) => (!words.trim().is_empty()).then(|| text(words.trim().to_owned()).font(NAME_REGULAR).color(p.muted)),
            opencode_i18n::Part::Arg(_) => Some(text(quoted.clone()).font(NAME_REGULAR).color(p.text).truncate()),
        })
        .collect()
}

/// The content of the open page: a per-server page of a server that can't be reached shows
/// `SettingsServerUnavailable` under its own header.
pub fn view<H: SettingsHost + PopupHost + 'static>(s: &H, p: &Pal) -> El {
    let page = &s.settings_pages().page;
    if page.per_server() && servers::inventory::selected(s).availability.pages_connection().is_none() {
        let (key, title, description) = match page {
            Page::Providers => ("settings-providers-page", t!("settings.providers.title"), t!("settings.providers.description")),
            Page::Models => ("settings-models-page", t!("settings.models.title"), t!("settings.models.description")),
            Page::Extensions => ("settings-extensions-page", t!("settings.tab.extensions"), t!("settings.extensions.description")),
            Page::Worktrees => ("settings-workspaces-page", t!("settings.tab.workspaces"), t!("settings.workspaces.description")),
            _ => ("settings-projects-page", t!("settings.projects.title"), t!("settings.projects.description")),
        };
        return servers::unavailable::view(s, p, key, title, description);
    }
    match page {
        Page::Providers => providers::view(s, p),
        Page::Models => models::view(s, p),
        Page::Extensions => extensions::view(s, p),
        Page::Projects => projects::view(s, p),
        Page::Worktrees => worktrees::view(s, p),
        Page::General => general::view(s, p),
        Page::Appearance => appearance::view(s, p),
        Page::Notifications => notifications::view(s, p),
        Page::Shortcuts => shortcuts::view(s, p),
        Page::Servers => servers::view(s, p),
        Page::Experimental => experimental::view(s, p),
        Page::About => about::view(s, p),
        Page::Plugin(id) => plugin_page(s, p, id),
        Page::Project(dir, tab) => project::view(s, p, dir, *tab),
    }
}

/// A plugin's own page: its title, description, optional header actions, and body.
fn plugin_page(s: &dyn SettingsHost, p: &Pal, id: &str) -> El {
    let (title, description, body, actions) = s.plugin_page_view(p, id).unwrap_or_else(|| (id.to_owned(), String::new(), el(), None));
    page_with(p, &format!("settings-{}-page", id.rsplit('.').next().unwrap_or(id)), header_with(p, &title, &description, actions), body, true)
}

/// `row` with the settings search's highlight behind it once a result revealed `target`.
pub fn revealable(s: &dyn SettingsHost, p: &Pal, target: &str, inset: f32, row: El) -> El {
    revealed(p, crate::search::reveal_of(s, target), inset, row)
}

/// [`revealable`] given the search's highlight on the row (`None` when it has none).
pub fn revealed(p: &Pal, lit: Option<bool>, inset: f32, row: El) -> El {
    let fade = rune::Curve::tween(1.8, rune::Easing::cubic(0.0, 0.0, 0.58, 1.0));
    let mark = el()
        .key("search-reveal")
        .pin(Some(-inset), Some(0.0), Some(-inset), Some(0.0))
        .radius(6.0)
        .fill(p.layer3)
        .opacity(if lit == Some(true) { 1.0 } else { 0.0 })
        .opacity_motion(if lit == Some(true) { rune::Anim::INSTANT } else { fade.into() });
    col().w_full().child(mark).child(row)
}

/// Rows keyed by their search target, each able to show the search's highlight.
pub fn rows(s: &dyn SettingsHost, p: &Pal, inset: f32, rows: Vec<(&str, El)>) -> Vec<El> {
    rows.into_iter().map(|(target, r)| revealable(s, p, target, inset, r)).collect()
}
