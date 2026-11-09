//! The Settings screen (`settings/shell.tsx`, `navigation.tsx`, `settings.css`): the rail of
//! pages beside the open page. At 1048 pt and wider the two sit centred in 1048 (a 328 pt rail,
//! 24 in and 64 before the page, and a 720 pt page column); from 800 the rail narrows to 240
//! with 24 pt gutters; below that the rail gives way to a bar with Back and the page menu. The
//! whole screen scrolls, the rail holding at its top, so the wheel anywhere moves the page.

use rune::{Color, El, Tx, col, el, icon, row, text};
use rune_theme_opencode::icons as ic;

use super::pages::{self, Page};
use super::search;
use crate::App;
use crate::palette::Pal;
use crate::popup::{self, Menu};
use opencode_i18n::t;

/// `tabs-v2-trigger`: 13 on 16 at 440, -0.04 tracking.
const TAB: rune::TextStyle = rune::TextStyle::new(13.0, 440.0).lh(16.0).tracking(-0.04);

/// The rail's groups (`RootSettings`'s `groups`): the server's pages; the app's, with plugin
/// pages (Pairing among them) before Server; then What's new, Experimental and About.
pub fn groups(s: &App) -> Vec<Vec<Page>> {
    let plugins = crate::plugin_host::settings_page_ids(s);
    let (about, own): (Vec<_>, Vec<_>) = plugins.into_iter().partition(|(_, about)| *about);
    let mut client = vec![Page::General, Page::Appearance, Page::Notifications, Page::Shortcuts];
    client.extend(own.into_iter().map(|(id, _)| Page::Plugin(id)));
    client.push(Page::Servers);
    let mut last: Vec<Page> = about.into_iter().map(|(id, _)| Page::Plugin(id)).collect();
    if pages::whats_new::available() {
        last.insert(0, Page::WhatsNew);
    }
    last.extend([Page::Experimental, Page::About]);
    vec![vec![Page::Providers, Page::Models, Page::Extensions, Page::Projects, Page::Worktrees], client, last]
}

/// A page's name in the rail.
pub fn label(s: &App, page: &Page) -> String {
    match page {
        Page::Plugin(id) => crate::plugin_host::settings_page_title(s, id).unwrap_or_else(|| id.clone()),
        Page::Servers if pages::servers::count(s) > 1 => t!("settings.tab.servers").to_owned(),
        other => other.tab_label().to_owned(),
    }
}

fn glyph(s: &App, page: &Page) -> &'static rune::Icon {
    match page {
        Page::Plugin(id) => crate::plugin_host::settings_page_icon(s, id).unwrap_or(&ic::EXTENSIONS),
        other => other.icon(),
    }
}

/// The screen at `width` points: the rail and the page, or the narrow layout.
pub fn view(s: &App, p: &Pal, (width, height): (f32, f32)) -> El {
    // `.settings-panel` (`padding-inline: 4px`) is the rows' container.
    let narrow = width > 0.0 && width < 800.0;
    crate::settings::note_panel_width(if narrow { width - 8.0 } else { 0.0 });
    let content = pages::view(s, p);
    let screen = col().key("settings").role("region").label(t!("command.category.settings")).w_full().grow(1.0).shrink().min_h(0.0)
        .scroll(rune::ScrollSpec { stick_bottom: false });
    if narrow {
        // `--settings-mobile-inner-inset`: 8 in a narrow window, else 16 (`settings.css:33`).
        let inset = if s.narrow() { 8.0 } else { 16.0 };
        // `.settings-sidebar` collapses to the search field under the bar, `8 inset` round it.
        let field = col().key("settings-search-list").w_full().child(search::field(s, p));
        let field = if search::active(s) { field.child(search::results(s, p).max_h((height - 96.0).max(120.0))) } else { field };
        return col().key("settings-page").w_full().grow(1.0).shrink().min_h(0.0)
            .child(narrow_bar(s, p, inset))
            .child(col().key("settings-sidebar").w_full().px(inset).py(8.0).child(field))
            .child(screen.child(col().w_full().px(inset).child(content)));
    }
    let wide = width <= 0.0 || width >= 1048.0;
    let (rail_w, pad_start, pad_end, gutter) = if wide { (328.0, 24.0, 64.0, 0.0) } else { (240.0, 16.0, 24.0, 24.0) };
    // `.settings-nav` is 240 wide at most (`settings.css:70-78`): all of the narrow rail's 200.
    let rail = col().key("settings-sidebar").w(rail_w).pt(50.0).pb(48.0).pl(pad_start).pr(pad_end).sticky_top(0.0).child(nav(s, p, rail_w - pad_start - pad_end, height));
    let page = col().key("settings-content").grow(1.0).shrink().min_w(0.0).max_w(720.0).child(content);
    col().key("settings-page").w_full().grow(1.0).shrink().min_h(0.0)
        .child(screen.child(row().w_full().px(gutter).justify_center().child(row().w_full().max_w(1048.0).items_start().child(rail).child(page))))
}

/// `.settings-nav`: Back, the search, then the page groups (or the search's results). A
/// project's pages have Back to projects and their tabs, without the search.
fn nav(s: &App, p: &Pal, width: f32, height: f32) -> El {
    if let Page::Project(dir, _) = &s.settings_pages.page {
        let tabs = pages::project::Tab::ALL.iter().fold(col().key("settings-nav-group-0").role("tablist").focus_group().w_full().gap(4.0), |c, tab| c.child(nav_item(s, p, &Page::Project(dir.clone(), *tab))));
        return col().key("settings-nav").w(width).gap(16.0).child(back_button(p, matches!(s.settings_pages.page, Page::Project(..))).mb(28.0)).child(col().key("settings-nav-groups").w_full().child(tabs));
    }
    let nav = col().key("settings-nav").w(width).gap(16.0).child(back_button(p, matches!(s.settings_pages.page, Page::Project(..))).mb(28.0));
    // The results, or the page groups, scroll in what the rail leaves them: the screen less its
    // insets, Back and the field (and the nav's 16 after it, which the results don't take).
    let room = |gap: f32| if height > 0.0 { (height - 50.0 - 48.0 - 28.0 - 28.0 - 16.0 - 28.0 - gap).max(120.0) } else { 480.0 };
    // `.settings-search` holds the field and its results (`search.tsx:124`), so they meet without
    // the nav's gap.
    let field = col().key("settings-search-list").w_full().child(search::field(s, p));
    if search::active(s) {
        return nav.child(field.child(search::results(s, p).max_h(room(0.0))));
    }
    let nav = nav.child(field);
    let room = room(16.0);
    let groups = groups(s);
    // An overflowing list gives Chromium's 8 pt scroll bar its own room at its end
    // (`styles/scrollbar.css:25-28`), so its triggers are 8 narrower: 192 in the narrow rail.
    let content_h: f32 = groups.iter().map(|g| g.len() as f32 * 28.0 + g.len().saturating_sub(1) as f32 * 4.0).sum::<f32>() + groups.len().saturating_sub(1) as f32 * 16.0;
    let gutter = if content_h > room + 0.5 { 8.0 } else { 0.0 };
    // One Tab stop for the rail, its open page, as Kobalte's `Tabs`; the arrows move in it.
    let mut list = col().key("settings-nav-list").role("tablist").focus_group().w_full().pr(gutter).gap(16.0);
    for (g, group) in groups.into_iter().enumerate() {
        let items = group.iter().fold(col().key(format!("settings-nav-group-{g}")).w_full().gap(4.0), |c, page| c.child(nav_item(s, p, page)));
        list = list.child(items);
    }
    // `.settings-nav-groups` (`settings.css:84-146`): each edge fades in by what's hidden behind
    // it, up to 24.
    let groups = col().key("settings-nav-groups").w_full().shrink().min_h(0.0)
        .scroll(rune::ScrollSpec { stick_bottom: false })
        .scroll_fade(rune::ScrollFade::feather(24.0))
        .child(list);
    nav.child(col().key("settings-nav-room").w_full().max_h(room).child(groups))
}

/// A page's trigger: 28 pt, 6 in, 4 pt corners, muted; the open one on `layer-03` in the base ink.
fn nav_item(s: &App, p: &Pal, page: &Page) -> El {
    let on = &s.settings_pages.page == page;
    let ink = if on { p.text } else { p.muted };
    let label = label(s, page);
    let to = page.clone();
    row().key(format!("settings-nav-{}", page.id())).role("tab").label(label.clone()).focusable(true).focus_selected(on)
        .w_full().h(28.0).px(6.0).gap(8.0).center_y().radius(4.0)
        .fill(if on { p.layer3 } else { Color::TRANSPARENT })
        .hover(if on { p.layer3 } else { p.hover })
        .on_press_tx(move |s: &mut App, tx: &mut Tx| s.open_settings_page(to.clone(), tx))
        .child(icon(glyph(s, page), 16.0).color(ink))
        .child(text(label).font(TAB).color(ink).truncate())
}

/// `.settings-back`: the arrow and "Back to app" ("Back to projects" on a project's pages), muted
/// until hovered.
fn back_button(p: &Pal, project: bool) -> El {
    let words = if project { t!("settings.backToProjects") } else { t!("settings.backToApp") };
    row().key("settings-back").role("button").label(words).help(format!("{words} Esc")).h(28.0).px(6.0).gap(6.0).center_y().radius(4.0).self_start()
        .hover_group()
        .hover(p.layer3)
        .on_press_tx(move |s: &mut App, tx: &mut Tx| {
            tx.instant();
            if project {
                s.open_settings_page(Page::Projects, tx);
            } else {
                s.close_settings();
            }
        })
        .child(icon(&ic::ARROW_LEFT, 14.0).color(p.muted).group_color(p.text))
        .child(text(words).font(TAB).color(p.muted).group_color(p.text))
}

/// `.settings-mobile-nav`: Back on the bar's start, the open page's menu at its end.
fn narrow_bar(s: &App, p: &Pal, inset: f32) -> El {
    let current = label(s, &s.settings_pages.page);
    let trigger = row().key("settings-mobile-menu").role("button").label(current.clone()).h(28.0).px(11.0).gap(6.0).center_y().radius(6.0)
        .fill(p.bg_base).box_shadow(Pal::ring_out(p.ring)).hover(p.hover)
        .on_press_tx(|s: &mut App, tx: &mut Tx| {
            let groups = groups(s);
            let selected = s.settings_pages.page.id().to_owned();
            let mut menu = Menu::new(t!("command.category.settings")).align(rune::Align::End);
            for (g, group) in groups.iter().enumerate() {
                if g > 0 {
                    menu = menu.separator();
                }
                menu = menu.items(group.iter().map(|page| popup::Item::new(page.id(), label(s, page)).icon(glyph(s, page)).checked(page.id() == selected)));
            }
            popup::open(s, menu, "settings-mobile-menu", tx, |s: &mut App, picked, tx| {
                if let Some(id) = picked {
                    s.open_settings_page(Page::parse(id), tx);
                }
            });
        })
        .child(text(current).font(TAB).color(p.text))
        .child(icon(&ic::CHEVRON_DOWN, 16.0).color(p.icon_muted));
    row().key("settings-mobile-nav").w_full().px(inset).py(8.0).pb(8.5).center_y().justify_between().fill(p.bg_base)
        .child(back_button(p, matches!(s.settings_pages.page, Page::Project(..))))
        .child(trigger)
        .child(el().key("rule").pin(Some(0.0), None, Some(0.0), Some(0.0)).h(0.5).fill(p.border_muted))
}
