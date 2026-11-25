//! The Settings screen (`settings/shell.tsx`, `navigation.tsx`, `settings.css`): the rail of
//! pages beside the open page. At 1008 pt and wider the two sit where Home's columns do, centred
//! in 1056 with 24 pt gutters (a 256 pt rail, and a 728 pt page column 28 past it whose cards
//! are Home's 720); from 800 the rail narrows to 240; below that the rail gives way to a bar with Back and the page menu. The
//! whole screen scrolls, the rail holding at its top, so the wheel anywhere moves the page.

use opencode_i18n::t;
use opencode_ui::focus::FocusExt;
use opencode_ui::popup::{self, Item, Menu, MenuButton as _, PopupHost};
use rune::{Color, Drag, El, Icon, Phase, PointerButton, Tx, col, icon, row, text};
use rune_theme_opencode::icons as ic;
use opencode_ui::palette::Pal;

use crate::host::SettingsHost;
use crate::pages::{self, Page};
use crate::search;

/// `tabs-v2-trigger`: 13 on 16 at 440, -0.04 tracking.
const TAB: rune::TextStyle = rune::TextStyle::new(13.0, 440.0).lh(16.0).tracking(-0.04);
const BACK: rune::TextStyle = rune::TextStyle::new(13.0, 440.0).lh(16.0);
/// A page row's hover (`settings-nav-groups` `tabs-v2-trigger-wrapper`: 120 ms ease-in-out).
const NAV_FADE: rune::Anim = rune::Anim::new(rune::Curve::tween(0.12, rune::Easing::EASE_IN_OUT));

/// `RootSettings`'s groups: client preferences, server pages, plugin pages and about.
pub fn groups(s: &dyn SettingsHost) -> Vec<Vec<Page>> {
    let client = vec![Page::General, Page::Appearance, Page::Notifications, Page::Shortcuts];
    let mut server = vec![Page::Providers, Page::Models, Page::Extensions, Page::Projects, Page::Worktrees, Page::Servers];
    let mut plugins = Vec::new();
    let mut last = Vec::new();
    for p in s.plugin_pages() {
        if p.in_about_group {
            last.push(Page::Plugin(p.id));
        } else if p.in_server_group {
            server.push(Page::Plugin(p.id));
        } else {
            plugins.push(Page::Plugin(p.id));
        }
    }
    last.extend([Page::Experimental, Page::About]);
    [client, server, plugins, last].into_iter().filter(|group| !group.is_empty()).collect()
}

/// A page's name in the rail.
pub fn label(s: &dyn SettingsHost, page: &Page) -> String {
    match page {
        Page::Plugin(id) => s.plugin_pages().into_iter().find(|p| p.id == *id).map_or_else(|| id.clone(), |p| p.title),
        Page::Servers if pages::servers::count(s) > 1 => t!("settings.tab.servers").to_owned(),
        other => other.tab_label().to_owned(),
    }
}

fn glyph(s: &dyn SettingsHost, page: &Page) -> &'static Icon {
    match page {
        Page::Plugin(id) => s.plugin_pages().into_iter().find(|p| p.id == *id).map_or(&ic::EXTENSIONS, |p| p.icon),
        other => other.icon(),
    }
}

/// The screen at `width` points: the rail and the page, or the narrow layout.
pub fn view<H: SettingsHost + PopupHost + 'static>(s: &H, p: &Pal, (width, height): (f32, f32)) -> El {
    // `.settings-panel` (`padding-inline: 4px`) is the rows' container.
    let narrow = width > 0.0 && width < 800.0;
    crate::note_panel_width(if narrow { width - 8.0 } else { 0.0 }, s.narrow_sm());
    let content = pages::view(s, p);
    // The whole screen scrolls with no bar, as Desktop hides it.
    let screen = |inner: El| {
        opencode_ui::scroller::scroller("settings").hidden(
            col()
                .role("region")
                .label_static(t!("command.category.settings"))
                .focusable(true)
                .tab_index(-1)
                .no_focus_ring()
                .w_full()
                .grow(1.0)
                .shrink()
                .min_h(0.0),
            inner,
        )
    };

    if narrow {
        // `--settings-mobile-inner-inset`: 8 in a narrow window, else 16 (`settings.css:33`).
        let inset = if s.narrow() { 8.0 } else { 16.0 };
        let field = col().key("settings-search-list").w_full().child(search::field(s, p, true));
        let field = if search::active(s) && !s.settings_pages().find.collapsed_narrow {
            field.child(search::results(s, p).max_h((height - 96.0).max(120.0)))
        } else {
            field
        };
        return col()
            .key("settings-page")
            .w_full()
            .grow(1.0)
            .shrink()
            .min_h(0.0)
            .child(narrow_bar(s, p, inset))
            .child(col().key("settings-sidebar").w_full().px(inset).py(8.0).child(field))
            // The page sits at the wide layout's path (`#0/#0/settings-content`), so a control keeps
            // keyboard focus while a layout switch or zoom moves the screen across 800.
            .child(screen(row().w_full().px(inset).child(row().w_full().child(col().key("settings-content").w_full().min_w(0.0).child(content)))));
    }
    // `settings.css` (opencode#65998): Settings lines up with Home. The layout is Home's 1056 box
    // centred with 24 of padding each side: the sidebar is Home's 256 projects column, and the
    // content starts 28 past it and runs 4 into the end gutter, so its panels' 4 of shadow room
    // sits in Home's 32 column gap and the cards span Home's 720 sessions column. From 800 to
    // 1007 the sidebar is 240 and the content has no margins.
    let wide = width <= 0.0 || width >= 1008.0;
    let (rail_w, pad_start, pad_end, margin) = if wide { (256.0, 0.0, 12.0, (28.0, -4.0)) } else { (240.0, 16.0, 24.0, (0.0, 0.0)) };
    let rail = col()
        .key("settings-sidebar")
        .w(rail_w)
        .pt(50.0)
        .pb(48.0)
        .pl(pad_start)
        .pr(pad_end)
        .sticky_top(0.0)
        .child(nav(s, p, rail_w - pad_start - pad_end, height));
    let page = col().key("settings-content").grow(1.0).shrink().min_w(0.0).max_w(728.0).ml(margin.0).mr(margin.1).child(content);
    col()
        .key("settings-page")
        .w_full()
        .grow(1.0)
        .shrink()
        .min_h(0.0)
        // `padding-inline: max(24px, calc((100% - 1056px) / 2 + 24px))`.
        .child(screen(row().w_full().px(24.0).justify_center().child(row().w_full().max_w(1056.0 - 48.0).items_start().child(rail).child(page))))
}

/// `.settings-nav`: Back, the search, then the page groups (or the search's results). A
/// project's pages have Back to projects and their tabs, without the search.
fn nav<H: SettingsHost + PopupHost + 'static>(s: &H, p: &Pal, width: f32, height: f32) -> El {
    let is_project = matches!(s.settings_pages().page, Page::Project(..));
    if let Page::Project(dir, _) = &s.settings_pages().page {
        let dir = dir.clone();
        let tabs = pages::project::Tab::ALL
            .iter()
            .fold(col().key("settings-nav-group-0").role("tablist").focus_group().w_full().gap(4.0), |c, tab| {
                c.child(nav_item(s, p, &Page::Project(dir.clone(), *tab)))
            });
        return col()
            .key("settings-nav")
            .w(width)
            .gap(16.0)
            .child(back_button::<H>(p, is_project).mb(28.0))
            .child(col().key("settings-nav-groups").w_full().child(tabs));
    }
    let nav = col().key("settings-nav").w(width).gap(16.0).child(back_button::<H>(p, is_project).mb(28.0));
    let room = |gap: f32| if height > 0.0 { (height - 50.0 - 48.0 - 28.0 - 28.0 - 16.0 - 28.0 - gap).max(120.0) } else { 480.0 };
    let field = col().key("settings-search-list").w_full().child(search::field(s, p, false));
    if search::active(s) {
        return nav.child(field.child(search::results(s, p).max_h(room(0.0))));
    }
    let nav = nav.child(field);
    let room = room(16.0);
    let groups = groups(s);
    let content_h: f32 = groups.iter().map(|g| g.len() as f32 * 28.0 + g.len().saturating_sub(1) as f32 * 4.0).sum::<f32>()
        + groups.len().saturating_sub(1) as f32 * 16.0;
    let gutter = if content_h > room + 0.5 { 8.0 } else { 0.0 };
    let mut list = col().key("settings-nav-list").role("tablist").focus_group().w_full().pr(gutter).gap(16.0);
    for (g, group) in groups.into_iter().enumerate() {
        let items = group.iter().fold(col().key(format!("settings-nav-group-{g}")).w_full().gap(4.0), |c, page| c.child(nav_item(s, p, page)));
        list = list.child(items);
    }
    let groups = opencode_ui::scroller::scroller("settings-nav-groups")
        .build::<H>(p, col().w_full().shrink().min_h(0.0).scroll_fade(rune::ScrollFade::feather(24.0)), list);
    nav.child(col().key("settings-nav-room").w_full().max_h(room).child(groups))
}

fn press_nav_item(host: &mut dyn SettingsHost, to: &Page, tx: &mut Tx) {
    if &host.settings_pages().page != to || search::active(host) {
        pages::open_page(host, to.clone(), tx);
    } else {
        tx.live_only();
    }
}

fn drag_nav_item(host: &mut dyn SettingsHost, to: &Page, d: &Drag, tx: &mut Tx) {
    let mouse = tx.button() == PointerButton::Primary && !opencode_ui::style::touch_controls();
    if d.phase == Phase::Down && mouse && (&host.settings_pages().page != to || search::active(host)) {
        pages::open_page(host, to.clone(), tx);
    } else {
        tx.live_only();
    }
}

fn nav_item<H: SettingsHost + 'static>(s: &H, p: &Pal, page: &Page) -> El {
    let on = &s.settings_pages().page == page;
    let ink = if on { p.text } else { p.muted };
    let label = label(s, page);
    let to = page.clone();
    let pressed = page.clone();
    row()
        .key(format!("settings-nav-{}", page.id()))
        .role("tab")
        .label(label.clone())
        .focusable(true)
        .focus_selected(on)
        .w_full()
        .h(28.0)
        .px(6.0)
        // `tabs-v2-trigger`'s gap is 8 (opencode#65998; measured next to official).
        .gap(8.0)
        .center_y()
        .radius(6.0)
        .fill(if on { p.layer3 } else { Color::TRANSPARENT })
        // Under the pointer a row that isn't the open page takes `bg-layer-01` and the base ink,
        // over 120 ms (`transition: background-color, color 120ms ease-in-out`).
        .hover(if on { p.layer3 } else { p.layer1 })
        .hover_group()
        .hover_motion(NAV_FADE, NAV_FADE)
        .on_press_tx(move |s: &mut H, tx: &mut Tx| press_nav_item(s, &to, tx))
        .on_drag(move |s: &mut H, d: &mut Drag, tx: &mut Tx| drag_nav_item(s, &pressed, d, tx))
        .child(icon(glyph(s, page), 16.0).color(ink).group_color(p.text))
        .child(text(label).font(TAB).color(ink).group_color(p.text).truncate())
}

fn press_back(host: &mut dyn SettingsHost, project: bool, tx: &mut Tx) {
    tx.instant();
    if project {
        pages::open_page(host, Page::Projects, tx);
    } else {
        crate::back_to_app(host, tx);
    }
}

/// `.settings-back`: the arrow and "Back to app" ("Back to projects" on a project's pages).
fn back_button<H: SettingsHost + 'static>(p: &Pal, project: bool) -> El {
    let words = if project { t!("settings.backToProjects") } else { t!("settings.backToApp") };
    row()
        .key("settings-back")
        .role("button")
        .label(words)
        .help(format!("{words} Esc"))
        .h(28.0)
        .px(6.0)
        .gap(8.0)
        .center_y()
        .radius(6.0)
        .self_start()
        .focus_default(p)
        .hover_group_focus()
        .hover(p.layer1)
        .on_press_tx(move |s: &mut H, tx: &mut Tx| press_back(s, project, tx))
        .child(icon(&ic::ARROW_LEFT, 16.0).mirror_in_rtl().color(p.muted).group_color(p.text))
        .child(text(words).font(BACK).color(p.muted).group_color(p.text))
}

fn build_narrow_menu(s: &dyn SettingsHost) -> Menu {
    let groups = groups(s);
    let selected = s.settings_pages().page.id().to_owned();
    let rows = groups.iter().map(|g| g.len() + 1).sum::<usize>() as f32;
    let mut menu = Menu::new(t!("command.category.settings")).align(rune::Align::End).gap(8.0).rows_before_scroll(rows);
    for (g, group) in groups.iter().enumerate() {
        if g > 0 {
            menu = menu.separator();
        }
        menu = menu.items(group.iter().map(|page| Item::new(page.id(), label(s, page)).icon(glyph(s, page)).checked(page.id() == selected)));
    }
    menu
}

fn pick_narrow_page(host: &mut dyn SettingsHost, picked: Option<&str>, tx: &mut Tx) {
    if let Some(id) = picked {
        pages::open_page(host, Page::parse(id), tx);
    }
}

/// `.settings-mobile-nav`: Back on the bar's start, the open page's menu at its end.
fn narrow_bar<H: SettingsHost + PopupHost + 'static>(s: &H, p: &Pal, inset: f32) -> El {
    let current = label(s, &s.settings_pages().page);
    let trigger = row()
        .key("settings-mobile-menu")
        .role("button")
        .label(current.clone())
        .h(28.0)
        .px(11.0)
        .gap(6.0)
        .center_y()
        .radius(6.0)
        .fill(p.bg_base)
        .box_shadow(Pal::ring_out(p.ring))
        .hover(p.hover)
        .menu_button("settings-mobile-menu", |s: &mut H, tx: &mut Tx| {
            let menu = build_narrow_menu(s);
            popup::open(s, menu, "settings-mobile-menu", tx, |s: &mut H, picked, tx| pick_narrow_page(s, picked, tx));
        })
        .child(text(current).font(TAB).color(p.text))
        .child(icon(&ic::CHEVRON_DOWN, 16.0).color(p.icon_muted));
    row()
        .key("settings-mobile-nav")
        .w_full()
        .px(inset)
        .py(8.0)
        .center_y()
        .justify_between()
        .fill(p.bg_base)
        .border_sides([0.0, 0.0, 0.5, 0.0], p.border_muted)
        .border_layout()
        .child(back_button::<H>(p, matches!(s.settings_pages().page, Page::Project(..))))
        .child(trigger)
}
