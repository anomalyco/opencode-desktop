//! The MCPs dialog (`providers/connect/mcp-dialog.tsx`): the compact command palette's frame,
//! 480 pt wide with its search field leading, each server's status dot, name and switch (or
//! Sign in), and a failed server's error view with Copy details, Disable and Try again.

use opencode_kit::integration::{McpServer, McpStatus};
use rune::{Color, Cx, El, Key, KeyEvent, Spring, TextStyle, Transition, Tx, col, el, icon, row, spacer, text};
use rune_kit::{ButtonVariant, Switch};
use rune_theme_opencode::icons as ic;

use super::Detail;
use crate::App;
use crate::palette::{Pal, kit};
use crate::widgets::{self, BODY, LABEL, button};

const TITLE: TextStyle = TextStyle::new(15.0, 530.0).lh(20.0).tracking(-0.13);
const ERROR: TextStyle = TextStyle::new(12.0, 440.0).mono().lh(18.0);
/// `.mcp-status-action`: a small ghost-muted button at 11 on 16.
const ACTION: TextStyle = TextStyle::new(11.0, 440.0).lh(16.0).tracking(0.05);

/// The dialog's servers as listed: by name, a search narrowing them by name or status.
fn rows(s: &App) -> Vec<McpServer> {
    let Some(open) = &s.mcp.open else { return vec![] };
    let query = open.search.text.trim().to_lowercase();
    let mut list: Vec<McpServer> = s.mcp_servers(&open.dir).unwrap_or_default().to_vec();
    list.sort_by(|a, b| rune::text::compare_names(&a.name, &b.name));
    list.retain(|m| query.is_empty() || subsequence(&query, &m.name.to_lowercase()) || subsequence(&query, m.status.name()));
    list
}

/// fuzzysort's loosest match: every character of the query, in order.
fn subsequence(query: &str, hay: &str) -> bool {
    let mut chars = hay.chars();
    query.chars().all(|q| chars.any(|h| h == q))
}

/// The list's arrows and ↵: a failed server's row opens its error, any other's flips it.
pub fn list_key(s: &mut App, ev: &KeyEvent, tx: &mut Tx) -> bool {
    let names: Vec<String> = rows(s).into_iter().map(|m| m.name).collect();
    let Some(open) = s.mcp.open.as_mut() else { return false };
    if !matches!(open.detail, Detail::Closed) {
        return false;
    }
    match ev.key {
        Key::Down | Key::Up if !names.is_empty() => {
            let at = open.active.as_ref().and_then(|a| names.iter().position(|n| n == a)).unwrap_or(0);
            let n = names.len();
            open.active = Some(names[if ev.key == Key::Down { (at + 1) % n } else { (at + n - 1) % n }].clone());
            tx.instant();
            true
        }
        Key::Enter => {
            let Some(name) = open.active.clone().or_else(|| names.first().cloned()) else { return false };
            select(s, &name, tx);
            true
        }
        _ => false,
    }
}

fn select(s: &mut App, name: &str, tx: &mut Tx) {
    tx.instant();
    let Some(dir) = s.mcp.open.as_ref().map(|o| o.dir.clone()) else { return };
    let status = s.mcp_servers(&dir).and_then(|l| l.iter().find(|m| m.name == name)).map(|m| m.status.clone());
    match status {
        Some(McpStatus::Failed(_)) => s.open_mcp_detail(name),
        st => s.toggle_mcp(&dir, name, st != Some(McpStatus::Connected)),
    }
}

pub fn layer(s: &App, p: &Pal, cx: &Cx<App>) -> Vec<El> {
    let Some(open) = &s.mcp.open else { return vec![] };
    let zoom = s.zoom.max(0.5);
    let [win_w, win_h] = cx.layout_size("zoom").map_or([1280.0 / zoom, 820.0 / zoom], |[w, h]| [w, h]);
    let body = match &open.detail {
        Detail::Open { name, error, copied, .. } => detail(s, p, &open.dir, name, error, copied.as_deref() == Some(error.as_str())),
        // `max-width: 639px`: each row stands at least 44 tall, 8 in above and below (`search-list.css:291-297`).
        _ => list(s, p, &open.dir, win_w < 640.0 || cx.hover_none()),
    };
    // `<Dialog size="large" fit>` (`providers/connect/mcp-dialog.tsx`).
    let size = widgets::DialogSize::Large;
    let dialog = widgets::overlay(p, col())
        .key("mcp-dialog")
        .role("dialog")
        .label_static(opencode_i18n::t!("dialog.mcp.title"))
        .w(size.width(win_w))
        .max_h(size.height(win_h))
        .radius(12.0)
        .fill(p.bg_base)
        .clip_soft(0.0)
        .transition(Transition::none())
        .motion(Spring::NONE)
        .modal(true, |s: &mut App| s.close_mcp_dialog(&mut Tx::default()))
        .child(body);
    widgets::modal("mcp", p, dialog, Some(size.top(win_h)))
}

fn list(s: &App, p: &Pal, dir: &str, tall: bool) -> El {
    let open = s.mcp.open.as_ref().expect("open");
    let field = row().key("mcp-search-field").w_full().h(28.0).pr(6.0).gap(6.0).center_y()
        .child(el().pl(6.0).child(icon(&ic::MAGNIFYING_GLASS, 16.0).color(p.icon_muted)))
        .child(session_view::style::field(&open.search, crate::view::UI).key("mcp-search").single_line().grow(1.0).shrink().min_w(0.0)
            .color(p.text).placeholder(opencode_i18n::t!("dialog.mcp.search.placeholder"), p.text.alpha(p.text.a * 0.5)).caret(p.text, p.selection).selection_style(p.field_selection)
            .label_static(opencode_i18n::t!("dialog.mcp.search.placeholder"))
            .on_edit(|s: &mut App| &mut s.mcp.open.as_mut().expect("open").search)
            .on_input_change(|s: &mut App, tx: &mut Tx| {
                tx.instant();
                if let Some(o) = s.mcp.open.as_mut() {
                    o.active = None;
                }
            })
            .on_input_key(|s: &mut App, ev, tx| super::key(s, ev, tx)));
    // `.command-palette-compact`: 6 pt around, 17 below, the hairline 8 pt over the bottom.
    let search = col().key("mcp-search-row").w_full().pt(6.0).px(6.0).pb(8.0)
        .child(field).child(el().h(0.5).mt(8.5).mx(-6.0).fill(p.border));
    let servers = rows(s);
    let all = s.mcp_servers(dir).map_or(0, <[McpServer]>::len);
    let mut results = col().key("mcp-results").w_full().px(6.0).pb(6.0).gap(4.0);
    if servers.is_empty() {
        let words = if all > 0 { opencode_i18n::t!("dialog.mcp.search.empty") } else { opencode_i18n::t!("dialog.mcp.empty") };
        results = results.child(row().key("mcp-empty").w_full().min_h(120.0).center().child(text(words).font(crate::view::UI).color(p.muted)));
    }
    let active = open.active.clone().or_else(|| servers.first().map(|m| m.name.clone()));
    for m in &servers {
        results = results.child(server_row(s, p, dir, m, active.as_deref() == Some(m.name.as_str()), tall));
    }
    col().key("mcp-list").w_full().min_h(0.0).shrink()
        .child(search)
        // `SearchList`'s `MenuScroll` (`mcp-dialog.tsx:231`, `search-list.tsx:319`).
        .child(session_view::scroller::scroller("mcp-scroll").bar(session_view::scroller::Bar::MenuScroll).build::<App>(p, col().w_full().shrink().min_h(0.0), results))
}

/// One server: its dot and name, then View error and its switch, or Sign in.
fn server_row(s: &App, p: &Pal, dir: &str, m: &McpServer, active: bool, tall: bool) -> El {
    let (name, hover) = (m.name.clone(), m.name.clone());
    let r = row().key(format!("mcp-row-{}", m.name)).role("option").label(m.name.clone()).w_full();
    let r = if tall { r.min_h(44.0).py(8.0) } else { r.h(28.0) };
    r.px(6.0).gap(16.0).center_y().radius(6.0)
        .fill(if active { p.hover } else { Color::TRANSPARENT })
        .transition(Transition::none())
        .on_hover_change(move |s: &mut App, inside, tx: &mut Tx| {
            if inside && let Some(o) = s.mcp.open.as_mut() {
                tx.instant();
                o.active = Some(hover.clone());
            }
        })
        .on_press_tx(move |s: &mut App, tx: &mut Tx| select(s, &name, tx))
        .child(row().grow(1.0).shrink().min_w(0.0).gap(6.0).center_y()
            .child(widgets::status_dot(p, m.status.name()))
            .child(text(m.name.clone()).font(LABEL).color(if m.status == McpStatus::Disabled { p.muted } else { p.text }).shrink().truncate()))
        .child(trailing(s, p, dir, m))
}

/// The switch or Sign in a server row ends with, and View error before it when it failed; the
/// Settings page's rows end the same.
pub fn trailing(s: &App, p: &Pal, dir: &str, m: &McpServer) -> El {
    let busy = s.mcp_busy(dir, &m.name);
    let mut e = row().key("mcp-trailing").gap(8.0).center_y();
    if let McpStatus::Failed(_) = m.status {
        let (name, dir2) = (m.name.clone(), dir.to_owned());
        e = e.child(action(p, "mcp-view-error", opencode_i18n::t!("dialog.mcp.error.view"), &opencode_i18n::t!("dialog.mcp.error.viewLabel", name = m.name), false, move |s: &mut App, tx: &mut Tx| {
            tx.instant();
            if s.mcp.open.as_ref().is_some_and(|o| o.dir == dir2) {
                s.open_mcp_detail(&name);
            } else {
                s.open_mcp_dialog(dir2.clone(), Some(&name), tx);
            }
        }));
    }
    let (dir, name) = (dir.to_owned(), m.name.clone());
    if matches!(m.status, McpStatus::NeedsAuth(_)) && m.integration.is_some() {
        let label = if busy { opencode_i18n::t!("dialog.mcp.signingIn") } else { opencode_i18n::t!("dialog.mcp.signIn") };
        // `.mcp-status-action:last-child`: the label lines up with the switches above and below it.
        return e.child(action(p, "mcp-sign-in", label, &opencode_i18n::t!("dialog.mcp.signIn.open", name = m.name), busy, move |s: &mut App, tx: &mut Tx| {
            tx.instant();
            s.toggle_mcp(&dir, &name, true);
        }).mr(-5.0));
    }
    let on = m.status != McpStatus::Disabled;
    let pending = busy || m.status == McpStatus::Pending;
    // The switch's label holds only the name in an `sr-only` span, so it lays out empty, yet it
    // is still a flex item 8 pt before the control (`switch.css` gap), and the switch stands 32
    // wide (`mcp-dialog.tsx`, `extensions.tsx`).
    e.child(row().key("mcp-switch-root").pl(8.0).center_y().child(Switch::new(on).web().label(m.name.clone()).hide_label().disabled(pending)
        .on_toggle_tx(move |s: &mut App, tx: &mut Tx| {
            tx.instant();
            s.toggle_mcp(&dir, &name, !on);
        })
        .build(kit(p)).key("mcp-switch")))
}

/// `.mcp-status-action`: a quiet small ghost-muted button in a switch's weight.
fn action(p: &Pal, key: &str, label: &str, a11y: &str, busy: bool, press: impl Fn(&mut App, &mut Tx) + 'static) -> El {
    let e = row().key(key.to_owned()).role("button").label(a11y.to_owned()).h(20.0).px(5.0).center().radius(4.0)
        .child(text(label.to_owned()).font(ACTION).color(p.muted));
    if busy { e.opacity(0.5) } else { e.hover(p.hover).press(p.pressed).on_press_tx(press) }
}

/// The error view: back, the server's name, "Connection failed", the error and its actions.
fn detail(s: &App, p: &Pal, dir: &str, name: &str, error: &str, copied: bool) -> El {
    let busy = s.mcp_busy(dir, name) || s.mcp_servers(dir).and_then(|l| l.iter().find(|m| m.name == name)).is_some_and(|m| m.status == McpStatus::Pending);
    let back = el().key("mcp-back").role("button").label_static(opencode_i18n::t!("common.goBack")).size(20.0, 20.0).radius(4.0).center()
        .hover(p.hover).press(p.pressed)
        .on_press_tx(|s: &mut App, tx: &mut Tx| {
            tx.instant();
            s.close_mcp_detail();
        })
        .child(icon(&ic::ARROW_LEFT, 14.0).mirror_in_rtl().color(p.icon_muted));
    let close = el().key("mcp-close").role("button").label_static(opencode_i18n::t!("common.close")).size(20.0, 20.0).mt(-2.0).mr(-2.0).radius(4.0).center()
        .hover(p.hover).press(p.pressed)
        .on_press_tx(|s: &mut App, tx: &mut Tx| s.close_mcp_dialog(tx))
        .child(icon(&crate::widgets::DIALOG_CLOSE, 16.0).color(p.icon_muted));
    let header = row().key("mcp-error-header").w_full().pt(16.0).px(16.0).pb(12.0)
        .child(col().grow(1.0).shrink().gap(4.0)
            .child(row().gap(8.0).child(back).child(text(name.to_owned()).font(TITLE).color(p.text).wrap_anywhere().grow(1.0).shrink()))
            .child(text(opencode_i18n::t!("dialog.mcp.error.status")).font(BODY).color(p.muted)))
        .child(close);
    let details = opencode_i18n::t!("dialog.mcp.error.details", name = name, error = error);
    let (d1, n1, d2, n2) = (dir.to_owned(), name.to_owned(), dir.to_owned(), name.to_owned());
    let error_text = error.to_owned();
    let actions = row().key("mcp-error-actions").w_full().gap(8.0)
        .child(button(if copied { opencode_i18n::t!("common.copied") } else { opencode_i18n::t!("dialog.mcp.error.copy") }, ButtonVariant::GhostMuted).icon(if copied { &ic::CHECK } else { &ic::COPY })
            .on_press_tx(move |s: &mut App, tx: &mut Tx| {
                tx.copy(details.clone());
                s.mcp_copied(error_text.clone());
            }).build(kit(p)))
        .child(spacer())
        .child(button(opencode_i18n::t!("dialog.mcp.error.disable"), ButtonVariant::Ghost).disabled(busy).on_press_tx(move |s: &mut App, tx: &mut Tx| {
            tx.instant();
            s.toggle_mcp(&d1, &n1, false);
        }).build(kit(p)))
        .child(button(opencode_i18n::t!("common.retry"), ButtonVariant::Neutral).disabled(busy).on_press_tx(move |s: &mut App, tx: &mut Tx| {
            tx.instant();
            s.toggle_mcp(&d2, &n2, true);
        }).build(kit(p)));
    col().key("mcp-error").w_full()
        .child(header)
        .child(col().key("mcp-error-details").w_full().px(16.0).pb(16.0).gap(12.0)
            .child(col().w_full().py(8.0).px(12.0).radius(6.0).fill(p.layer3)
                .child(text(error.to_owned()).font(ERROR).color(p.text).wrap_anywhere().selectable(true).selection_style(p.field_selection)))
            .child(actions))
}
