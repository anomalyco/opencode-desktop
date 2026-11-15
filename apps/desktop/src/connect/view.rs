//! The connect dialog on screen (`providers/connect/dialog.tsx`): a normal-size surface on the
//! base fill with 12 pt corners; every step fits its content up to the normal height, and the top
//! edge stays where that height puts it. The list is the search dialog's
//! (`search-list.css` `.search-dialog`): a bare search under a compact header, a hairline across.

use rune::{Color, Cx, El, Key, KeyEvent, Spring, TextStyle, Transition, Tx, col, el, icon, row, spacer, text};
use rune_kit::TextInput;
use rune_theme_opencode::icons as ic;

use super::dialog::{self, Event, Frame};
use crate::App;
use crate::palette::{Pal, kit};
use crate::widgets::{self, BODY};

/// The search dialog's title and description: 13 and 12 on a 20 line.
const TITLE: TextStyle = TextStyle::new(13.0, 440.0).lh(20.0).tracking(-0.04);
const DESCRIPTION: TextStyle = TextStyle::new(12.0, 440.0).lh(20.0).tracking(-0.04);
/// A provider step's title (`dialog-header-title` at 15, tracking -0.13).
const STEP_TITLE: TextStyle = TextStyle::new(15.0, 530.0).lh(16.0).tracking(-0.13);
/// The list's group labels: 12 on 18 at 440, tracking 0.05, faint.
const GROUP: TextStyle = TextStyle::new(12.0, 440.0).lh(18.0).tracking(0.05);

/// The integrations the list offers, in the TUI's groups: connected first by name, then the
/// popular ones in their order, then the rest by name. MCP sign-ins live in the MCPs dialog.
const POPULAR: [&str; 6] = ["opencode", "opencode-go", "openai", "github-copilot", "anthropic", "google"];

pub struct Row {
    pub id: String,
    pub name: String,
    pub account: Option<String>,
    pub needs_sign_in: bool,
    /// Environment variables only: nothing to connect from here.
    pub disabled: bool,
}

pub fn groups(s: &App) -> Vec<(&'static str, Vec<Row>)> {
    let open = s.connect.open.as_ref();
    let query = open.map_or(String::new(), |o| o.search.text.trim().to_owned());
    let fold = crate::language::search_fold();
    let list = s.integrations(s.connect_dir()).unwrap_or_default();
    let rows: Vec<Row> = list.iter()
        .filter(|i| i.source.as_deref() != Some("mcp"))
        .filter(|i| fold.contains(&format!("{} {}", i.id, i.name), &query))
        .map(|i| Row {
            id: i.id.clone(),
            name: i.name.clone(),
            account: i.connections.first().map(|c| c.label.clone()),
            needs_sign_in: i.connections.first().is_some_and(|c| c.status.is_some()),
            disabled: i.connect_methods().is_empty() && i.accounts().is_empty(),
        })
        .collect();
    let connected = |r: &Row| r.account.is_some();
    let (mut connected_rows, rest): (Vec<Row>, Vec<Row>) = rows.into_iter().partition(connected);
    connected_rows.sort_by(|a, b| rune::text::compare_names(&a.name, &b.name));
    let (mut popular, mut services): (Vec<Row>, Vec<Row>) = rest.into_iter().partition(|r| POPULAR.contains(&r.id.as_str()));
    popular.sort_by_key(|r| POPULAR.iter().position(|p| *p == r.id));
    services.sort_by(|a, b| rune::text::compare_names(&a.name, &b.name));
    [(opencode_i18n::t!("dialog.provider.connected"), connected_rows), (opencode_i18n::t!("dialog.provider.group.popular"), popular), (opencode_i18n::t!("dialog.provider.group.services"), services)].into_iter().filter(|(_, r)| !r.is_empty()).collect()
}

/// The rows the arrows move through: every one but those connected from the environment only.
fn rows(s: &App) -> Vec<String> {
    groups(s).into_iter().flat_map(|(_, r)| r).filter(|r| !r.disabled).map(|r| r.id).collect()
}

/// ↑ ↓ and ↵ in the list.
pub fn picker_key(s: &mut App, ev: &KeyEvent, tx: &mut Tx) -> bool {
    let ids = rows(s);
    let Some(open) = s.connect.open.as_mut() else { return false };
    match ev.key {
        Key::Down | Key::Up if !ids.is_empty() => {
            let at = open.active.as_ref().and_then(|a| ids.iter().position(|i| i == a));
            let n = ids.len();
            let next = match (at, ev.key == Key::Down) {
                (None, true) => 0,
                (None, false) => n - 1,
                (Some(i), true) => (i + 1) % n,
                (Some(i), false) => (i + n - 1) % n,
            };
            open.active = Some(ids[next].clone());
            tx.instant();
            tx.scroll_into_view_in("connect-list", format!("provider-{}", ids[next]));
            true
        }
        Key::Enter => {
            let Some(id) = open.active.clone() else { return false };
            pick(s, &id, tx);
            true
        }
        _ => false,
    }
}

fn pick(s: &mut App, id: &str, tx: &mut Tx) {
    tx.instant();
    let accounts = s.accounts_of(id);
    s.connect_event(Event::Selected { provider: id.to_owned(), from_picker: true, accounts }, tx);
}

pub fn layer(s: &App, p: &Pal, cx: &Cx<App>) -> Vec<El> {
    let Some(open) = &s.connect.open else { return vec![] };
    let frame = dialog::frame(&open.dialog);
    let authorizing = open.dialog.provider.as_ref().and_then(|p| p.step.connect()).is_some_and(super::machine::authorizing);
    let mut surface = widgets::overlay(p, col())
        .key("connect-dialog")
        .role("dialog")
        .label(match open.dialog.provider.as_ref() {
            Some(provider) if frame != Frame::Models => opencode_i18n::t!("provider.connect.title", provider = s.provider_name(&provider.id)),
            _ => title(s),
        })
        .radius(12.0)
        .fill(p.bg_base)
        .clip_soft(0.0)
        .transition(Transition::none())
        .motion(Spring::NONE)
        // A stray press beside it mustn't drop a sign-in the user is finishing in the browser.
        .modal(true, move |s: &mut App| if !authorizing { s.close_connect(&mut Tx::default()) });
    surface = surface.child(header(s, p, frame));
    surface = match frame {
        Frame::Picker => surface.child(picker(s, p)),
        _ => surface.child(col().key("connect-focus-host").focusable(true).tab_index(-1).role("group").label(title(s)).w_full().shrink().min_h(0.0)
            .child(super::steps::body(s, p, frame))),
    };
    // `<Dialog size="normal" fit anchored>` (`providers/connect/dialog.tsx`): every step fits its
    // content within the normal height, and the top edge stays put, so a step only moves the bottom.
    widgets::sized_modal("connect", p, widgets::DialogSize::Normal, widgets::Fit::Anchored, widgets::dialog_window(cx, *s.zoom), surface)
}

/// The dialog's name: the list's, or the step's title.
pub fn title(s: &App) -> String {
    let Some(open) = &s.connect.open else { return String::new() };
    let Some(p) = &open.dialog.provider else { return opencode_i18n::t!("dialog.provider.title").into() };
    let name = step_name(s, &p.id);
    match dialog::body(&open.dialog) {
        dialog::Body::Models => opencode_i18n::t!("provider.connect.models.title", provider = name),
        _ => opencode_i18n::t!("provider.connect.title", provider = name),
    }
}

fn close_button(p: &Pal) -> El {
    el().key("connect-close").role("button").label_static(opencode_i18n::t!("common.close")).size(20.0, 20.0).radius(4.0).center()
        .hover(p.hover).press(p.pressed)
        .on_press_tx(|s: &mut App, tx: &mut Tx| s.close_connect(tx))
        .child(icon(&crate::widgets::DIALOG_CLOSE, 16.0).color(p.icon_muted))
}

fn step_name(s: &App, id: &str) -> String {
    if id == dialog::CONSOLE { opencode_i18n::t!("provider.connect.opencode.name").into() } else { s.provider_name(id) }
}

fn remote_label(s: &App, p: &Pal, id: &str) -> Option<El> {
    if id != dialog::CONSOLE || s.server_is_local() { return None; }
    let key = crate::servers::runtime::active(s);
    let name = crate::servers::registry::label_of(s, key);
    let label = opencode_i18n::t!("provider.connect.remote.label", server = name);
    Some(row().key("connect-remote").max_w(160.0).shrink().mr(12.0).center_y().gap(6.0)
        .help(opencode_i18n::t!("provider.connect.remote.description"))
        .child(icon(&ic::SERVER, 14.0).color(p.icon_muted)).child(text(label).font(TITLE).color(p.muted).truncate()))
}

fn crumb(content: El, key: &'static str, name: String, target: Option<dialog::BackTarget>) -> El {
    let el = content.key(key);
    match target {
        None => el,
        Some(target) => el.role("button").label(name).on_press_tx(move |s: &mut App, tx: &mut Tx| {
            tx.instant();
            s.connect_event(Event::Navigate(target), tx);
        }),
    }
}

fn header(s: &App, p: &Pal, frame: Frame) -> El {
    let open = s.connect.open.as_ref().expect("open");
    match frame {
        // `.search-dialog`: 10 pt above and at the end, 14 before, 6 below.
        Frame::Picker => row().key("connect-header").w_full().pt(10.0).pr(10.0).pb(6.0).pl(14.0)
            .child(col().grow(1.0).shrink().gap(2.0)
                .child(text(opencode_i18n::t!("dialog.provider.title")).font(TITLE).color(p.text))
                .child(text(opencode_i18n::t!("dialog.provider.description")).font(DESCRIPTION).color(p.muted)))
            .child(close_button(p)),
        Frame::Models => {
            let id = open.dialog.provider.as_ref().map(|p| p.id.clone()).unwrap_or_default();
            row().key("connect-header").w_full().pt(20.0).px(20.0).pb(16.0)
                .child(row().grow(1.0).shrink().h(16.0).gap(8.0).center_y()
                    .child(widgets::provider_mark(&id, p.icon))
                    .child(text(title(s)).font(STEP_TITLE).color(p.text)))
                .children(remote_label(s, p, &id)).child(close_button(p).mt(-2.0).mr(-2.0))
        }
        Frame::Provider | Frame::Accounts => {
            let provider = open.dialog.provider.as_ref().expect("provider frame");
            let root = opencode_i18n::t!("provider.connect.breadcrumb.root");
            let name = step_name(s, &provider.id);
            let target = dialog::back_target(&open.dialog).filter(|t| *t != dialog::BackTarget::Picker);
            let nav = row().role("navigation").label_static(opencode_i18n::t!("provider.connect.breadcrumb.label")).grow(1.0).shrink().min_w(0.0).gap(4.0).center_y()
                .child(crumb(text(root).font(TITLE).color(p.faint), "connect-crumb-root", root.into(), (!open.dialog.standalone).then_some(dialog::BackTarget::Picker)))
                .child(icon(&ic::CHEVRON_RIGHT, 14.0).mirror_in_rtl().color(p.icon_muted))
                .child(crumb(row().shrink().min_w(0.0).gap(6.0).center_y().child(widgets::provider_mark(&provider.id, p.icon)).child(text(name.clone()).font(widgets::BODY_MED).color(p.text).truncate()), "connect-crumb-provider", name, target));
            row().key("connect-header").w_full().padding(16.0).child(nav).children(remote_label(s, p, &provider.id)).child(close_button(p))
        }
    }
}

/// The searchable list (`ProviderPicker`).
fn picker(s: &App, p: &Pal) -> El {
    let open = s.connect.open.as_ref().expect("open");
    let field = TextInput::new(&open.search)
        .placeholder(opencode_i18n::t!("common.search.placeholder"))
        .label(opencode_i18n::t!("dialog.provider.search.placeholder"))
        .spellcheck(s.settings.spellcheck)
        .leading_icon(&ic::MAGNIFYING_GLASS)
        .width(None)
        .plain()
        .on_edit(|s: &mut App| &mut s.connect.open.as_mut().expect("open").search)
        .on_change(|s: &mut App, tx: &mut Tx| {
            tx.instant();
            if let Some(o) = s.connect.open.as_mut() {
                o.active = None;
            }
        })
        .on_key(|s: &mut App, ev, tx| super::key(s, ev, tx))
        .build(kit(p))
        .key("connect-search");
    // The hairline reaches across the body's 6 pt padding.
    let search = col().key("connect-search-row").w_full().gap(6.0).child(field)
        .child(el().key("connect-search-rule").h(0.5).mx(-6.0).fill(p.border_muted));
    let groups = groups(s);
    let mut list = col().key("connect-groups").w_full().gap(2.0).pb(32.0);
    if groups.is_empty() {
        list = list.child(row().key("connect-empty").w_full().h(96.0).center().child(text(opencode_i18n::t!("dialog.provider.empty")).font(BODY).color(p.muted)));
    }
    for (title, rows) in groups {
        let mut section = col().key(format!("connect-group-{title}")).w_full().gap(2.0)
            .child(el().px(8.0).pt(10.0).pb(2.0).child(text(title).font(GROUP).color(p.faint)));
        for r in rows {
            section = section.child(provider_row(p, &r, open.active.as_deref() == Some(r.id.as_str())));
        }
        list = list.child(section);
    }
    col().key("connect-picker").w_full().grow(1.0).shrink().min_h(0.0).px(6.0).pb(6.0).gap(2.0)
        .child(search)
        // No bar, as Desktop hides it (`[scrollbar-width:none]`, `providers/connect/dialog.tsx:338`).
        .child(session_view::scroller::scroller("connect-list").hidden(col().w_full().shrink().min_h(0.0).clip_fade(0.0, 0.0, 0.0, 40.0), list))
}

fn provider_row(p: &Pal, r: &Row, active: bool) -> El {
    let (id, hover_id) = (r.id.clone(), r.id.clone());
    let ink = if r.disabled { p.faint } else { p.text };
    let mut e = row().key(format!("provider-{}", r.id)).role("button").label(r.name.clone()).w_full().min_h(32.0)
        .py(6.0).pl(8.0).pr(6.0).gap(8.0).center_y().radius(6.0)
        .fill(if active { p.hover } else { Color::TRANSPARENT })
        .transition(Transition::none())
        .child(widgets::provider_mark(&r.id, p.icon).opacity(if r.disabled { 0.5 } else { 1.0 }))
        .child(text(r.name.clone()).font(BODY).color(ink).shrink().min_w(0.0).truncate());
    if r.disabled {
        return e.child(text(opencode_i18n::t!("dialog.provider.environmentOnly")).font(BODY).color(p.faint).shrink().truncate());
    }
    if let Some(account) = &r.account {
        e = e.child(widgets::badge(p, account.clone()).shrink().min_w(0.0));
    }
    if r.needs_sign_in {
        e = e.child(spacer()).child(text(opencode_i18n::t!("dialog.provider.needsSignIn")).key("provider-status").font(BODY).color(p.warning).pr(4.0));
    }
    e.on_hover_change(move |s: &mut App, inside, tx: &mut Tx| {
        if inside && let Some(o) = s.connect.open.as_mut() {
            tx.instant();
            o.active = Some(hover_id.clone());
        }
    })
    .on_press_tx(move |s: &mut App, tx: &mut Tx| pick(s, &id, tx))
}
