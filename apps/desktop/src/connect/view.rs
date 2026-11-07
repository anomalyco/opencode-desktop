//! The connect dialog on screen (`providers/connect/dialog.tsx`): a 640 pt surface on the base
//! fill with 12 pt corners whose top stays where the provider list puts it; the list is 512 pt
//! tall, the models 560, and a provider's step fits its content. The list is the search dialog's
//! (`search-list.css` `.search-dialog`): a bare search under a compact header, a hairline across.

use rune::{Color, Cx, El, Key, KeyEvent, ScrollSpec, Spring, TextStyle, Transition, Tx, col, el, icon, row, spacer, text};
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
    let query = open.map_or(String::new(), |o| o.search.text.trim().to_lowercase());
    let list = s.integrations(s.connect_dir()).unwrap_or_default();
    let rows: Vec<Row> = list.iter()
        .filter(|i| i.source.as_deref() != Some("mcp"))
        .filter(|i| query.is_empty() || format!("{} {}", i.id, i.name).to_lowercase().contains(&query))
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
    connected_rows.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    let (mut popular, mut services): (Vec<Row>, Vec<Row>) = rest.into_iter().partition(|r| POPULAR.contains(&r.id.as_str()));
    popular.sort_by_key(|r| POPULAR.iter().position(|p| *p == r.id));
    services.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    [("Connected", connected_rows), ("Popular", popular), ("Services", services)].into_iter().filter(|(_, r)| !r.is_empty()).collect()
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
    let zoom = s.zoom.max(0.5);
    let [win_w, win_h] = cx.layout_size("zoom").map_or([1280.0 / zoom, 820.0 / zoom], |[w, h]| [w / zoom, h / zoom]);
    let top = ((win_h - 512.0) / 2.0).max(8.0);
    let frame = dialog::frame(&open.dialog);
    let authorizing = open.dialog.provider.as_ref().and_then(|p| p.step.connect()).is_some_and(super::machine::authorizing);
    let mut surface = widgets::overlay(p, col())
        .key("connect-dialog")
        .role("dialog")
        .label(title(s))
        .w((win_w - 16.0).min(640.0))
        .radius(12.0)
        .fill(p.bg_base)
        .clip_soft(0.0)
        .transition(Transition::none())
        .motion(Spring::NONE)
        // A stray press beside it mustn't drop a sign-in the user is finishing in the browser.
        .modal(true, move |s: &mut App| if !authorizing { s.close_connect(&mut Tx::default()) });
    surface = match frame {
        Frame::Picker => surface.h((win_h - 16.0).min(512.0)),
        Frame::Models => surface.h((win_h - 8.0 - top).min(560.0)),
        _ => surface.max_h((win_h - 8.0 - top).min(560.0)),
    };
    surface = surface.child(header(s, p, frame));
    surface = match frame {
        Frame::Picker => surface.child(picker(s, p)),
        _ => surface.child(super::steps::body(s, p, frame)),
    };
    widgets::modal("connect", p, surface, Some(top))
}

/// The dialog's name: the list's, or the step's title.
pub fn title(s: &App) -> String {
    let Some(open) = &s.connect.open else { return String::new() };
    let Some(p) = &open.dialog.provider else { return "Integrations".into() };
    let name = s.provider_name(&p.id);
    let connect = p.step.connect();
    match dialog::body(&open.dialog) {
        dialog::Body::Models => format!("Connected to {name}"),
        dialog::Body::Accounts => name,
        _ if p.id == dialog::CONSOLE && matches!(connect, Some(super::machine::State::Failed(..))) => "Connect to OpenCode".into(),
        dialog::Body::ConsoleSignIn { .. } => "Connecting to OpenCode Console".into(),
        _ if p.id == "anthropic" && connect.and_then(super::machine::method).is_some_and(|m| m.title().to_lowercase().contains("max")) => "Login with Anthropic".into(),
        _ if p.id == dialog::CONSOLE => "Connect OpenCode Console".into(),
        _ => format!("Connect {name}"),
    }
}

fn close_button(p: &Pal) -> El {
    el().key("connect-close").role("button").label("Close").size(20.0, 20.0).radius(4.0).center()
        .hover(p.hover).press(p.pressed)
        .on_press_tx(|s: &mut App, tx: &mut Tx| s.close_connect(tx))
        .child(icon(&crate::widgets::DIALOG_CLOSE, 16.0).color(p.icon_muted))
}

fn back_button(p: &Pal) -> El {
    el().key("connect-back").role("button").label("Navigate back").size(20.0, 20.0).my(-2.0).ml(-2.0).radius(4.0).center()
        .hover(p.hover).press(p.pressed)
        .on_press_tx(|s: &mut App, tx: &mut Tx| {
            tx.instant();
            s.connect_event(Event::Back, tx);
        })
        .child(icon(&ic::ARROW_LEFT, 14.0).color(p.icon_muted))
}

fn header(s: &App, p: &Pal, frame: Frame) -> El {
    let open = s.connect.open.as_ref().expect("open");
    let can_back = dialog::back_target(&open.dialog).is_some();
    match frame {
        // `.search-dialog`: 10 pt above and at the end, 14 before, 6 below.
        Frame::Picker => row().key("connect-header").w_full().pt(10.0).pr(10.0).pb(6.0).pl(14.0)
            .child(col().grow(1.0).shrink().gap(2.0)
                .child(text("Integrations").font(TITLE).color(p.text))
                .child(text("Connect services so OpenCode can use them.").font(DESCRIPTION).color(p.muted)))
            .child(close_button(p)),
        Frame::Accounts => {
            let mut lead = row().grow(1.0).shrink().min_w(0.0).gap(2.0).center_y();
            if can_back {
                lead = lead
                    .child(text("Integrations").key("connect-crumb").role("button").font(TITLE).color(p.faint)
                        .on_press_tx(|s: &mut App, tx: &mut Tx| {
                            tx.instant();
                            s.connect_event(Event::Back, tx);
                        }))
                    .child(icon(&ic::CHEVRON_RIGHT, 14.0).color(p.icon_muted));
            }
            row().key("connect-header").w_full().pt(10.0).pr(10.0).pb(6.0).pl(14.0)
                .child(lead.child(text(title(s)).font(TITLE).color(p.text).truncate()))
                .child(close_button(p))
        }
        Frame::Models => {
            let id = open.dialog.provider.as_ref().map(|p| p.id.clone()).unwrap_or_default();
            row().key("connect-header").w_full().pt(20.0).px(20.0).pb(16.0)
                .child(row().grow(1.0).shrink().h(16.0).gap(8.0).center_y()
                    .child(widgets::provider_mark(&id, p.icon))
                    .child(text(title(s)).font(STEP_TITLE).color(p.text)))
                .child(close_button(p).mt(-2.0).mr(-2.0))
        }
        Frame::Provider => {
            let id = open.dialog.provider.as_ref().map(|p| p.id.clone()).unwrap_or_default();
            // As tall as the title: the back button reaches 2 pt past it each way.
            let mut lead = row().grow(1.0).shrink().min_w(0.0).h(16.0).gap(8.0).center_y();
            if can_back {
                lead = lead.child(back_button(p));
            }
            row().key("connect-header").w_full().py(16.0).px(20.0)
                .child(lead.child(widgets::provider_mark(&id, p.icon)).child(text(title(s)).font(STEP_TITLE).color(p.text).truncate()))
                .child(close_button(p).mt(-2.0).mr(-2.0))
        }
    }
}

/// The searchable list (`ProviderPicker`).
fn picker(s: &App, p: &Pal) -> El {
    let open = s.connect.open.as_ref().expect("open");
    let field = TextInput::new(&open.search)
        .placeholder("Search")
        .label("Search providers")
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
        list = list.child(row().key("connect-empty").w_full().h(96.0).center().child(text("No providers found").font(BODY).color(p.muted)));
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
        .child(col().key("connect-list").w_full().grow(1.0).shrink().min_h(0.0)
            .scroll(ScrollSpec { stick_bottom: false })
            .clip_fade(0.0, 0.0, 0.0, 40.0)
            .child(list))
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
        return e.child(text("Environment only").font(BODY).color(p.faint).shrink().truncate());
    }
    if let Some(account) = &r.account {
        e = e.child(widgets::badge(p, account.clone()).shrink().min_w(0.0));
    }
    if r.needs_sign_in {
        e = e.child(spacer()).child(text("Needs sign-in").key("provider-status").font(BODY).color(p.warning).pr(4.0));
    }
    e.on_hover_change(move |s: &mut App, inside, tx: &mut Tx| {
        if inside && let Some(o) = s.connect.open.as_mut() {
            tx.instant();
            o.active = Some(hover_id.clone());
        }
    })
    .on_press_tx(move |s: &mut App, tx: &mut Tx| pick(s, &id, tx))
}
