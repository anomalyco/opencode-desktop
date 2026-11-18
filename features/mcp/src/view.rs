//! The MCPs dialog (`providers/connect/mcp-dialog.tsx`): the compact command palette's frame,
//! sized via [`DialogSize::Large`] with its search field leading, each server's status dot, name
//! and switch (or Sign in), and a failed server's error view with Copy details, Disable and Try
//! again.

use opencode_kit::integration::{McpServer, McpStatus};
use rune::{Color, Cx, El, Key, KeyEvent, Spring, TextStyle, Transition, Tx, col, el, icon, row, spacer, text};
use rune_kit::{ButtonVariant, Switch};
use rune_theme_opencode::icons as ic;
use opencode_ui::dialog::DialogSize;
use opencode_ui::palette::{Pal, kit};
use opencode_ui::scroller::{Bar, scroller};
use opencode_ui::style::{UI, field};

use crate::ui::{self, BODY, DIALOG_CLOSE, LABEL, TITLE, button, status_dot};
use crate::{Detail, Dialog, McpHost};

const ERROR: TextStyle = TextStyle::new(12.0, 440.0).mono().lh(18.0);
/// `.mcp-status-action`: a small ghost-muted button at 11 on 16.
const ACTION: TextStyle = TextStyle::new(11.0, 440.0).lh(16.0).tracking(0.05);

/// The dialog's servers as listed: by name, a search narrowing them by name or status.
pub fn rows(host: &dyn McpHost) -> Vec<McpServer> {
    let Some(open) = &host.mcp().open else { return Vec::new() };
    let query = open.search.text.trim().to_lowercase();
    let mut list: Vec<McpServer> = crate::servers(host, &open.dir).unwrap_or_default().to_vec();
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
pub fn list_key(host: &mut dyn McpHost, ev: &KeyEvent, tx: &mut Tx) -> bool {
    let names: Vec<String> = rows(host).into_iter().map(|m| m.name).collect();
    let Some(open) = host.mcp_mut().open.as_mut() else { return false };
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
            select(host, &name, tx);
            true
        }
        _ => false,
    }
}

/// Activates server `name` in the open dialog: opens its error view when failed, else toggles it.
pub fn select(host: &mut dyn McpHost, name: &str, tx: &mut Tx) {
    tx.instant();
    let Some(dir) = host.mcp().open.as_ref().map(|o| o.dir.clone()) else { return };
    let status = crate::servers(host, &dir).and_then(|l| l.iter().find(|m| m.name == name)).map(|m| m.status.clone());
    match status {
        Some(McpStatus::Failed(_)) => crate::open_detail(host, name),
        st => crate::toggle_server(host, &dir, name, st != Some(McpStatus::Connected)),
    }
}

/// The MCPs dialog modal layer (`DialogSize::Large` fit).
pub fn layer<H: McpHost + 'static>(s: &H, p: &Pal, cx: &Cx<H>) -> Vec<El> {
    if s.mcp().open.is_none() {
        return Vec::new();
    }
    let win = ui::dialog_window(s, cx);
    dialog(s, p, win, cx.hover_none())
}

/// Builds the MCPs dialog modal sized for `win`.
pub fn dialog<H: McpHost + 'static>(s: &H, p: &Pal, win: [f32; 2], hover_none: bool) -> Vec<El> {
    let Some(open) = &s.mcp().open else { return Vec::new() };
    let body = match &open.detail {
        Detail::Open { name, error, copied, .. } => detail(s, p, &open.dir, name, error, copied.as_deref() == Some(error.as_str())),
        // `max-width: 639px`: each row stands at least 44 tall, 8 in above and below (`search-list.css:291-297`).
        _ => list(s, p, open, win[0] < 640.0 || hover_none),
    };
    let (card, top) = build_dialog_card(p, win, body);
    let card = card.modal(true, |s: &mut H| crate::close_dialog(s, &mut Tx::default()));
    ui::modal("mcp", p, card, Some(top))
}

fn list<H: McpHost + 'static>(s: &H, p: &Pal, open: &Dialog, tall: bool) -> El {
    let search_input = search_input_shell(&open.search, p)
        .on_edit(|s: &mut H| s.mcp_mut().search_mut())
        .on_input_change(|s: &mut H, tx: &mut Tx| crate::clear_active(s, tx))
        .on_input_key(|s: &mut H, ev, tx| crate::key(s, ev, tx));
    let servers = rows(s);
    let total = crate::servers(s, &open.dir).map_or(0, <[McpServer]>::len);
    let active = open.active.clone().or_else(|| servers.first().map(|m| m.name.clone()));
    let row_els: Vec<El> = servers
        .iter()
        .map(|m| server_row(s, p, &open.dir, m, active.as_deref() == Some(m.name.as_str()), tall))
        .collect();
    let results = build_list_results(p, total, row_els);
    // `SearchList`'s `MenuScroll` (`mcp-dialog.tsx:231`, `search-list.tsx:319`).
    let scroll = scroller("mcp-scroll").bar(Bar::MenuScroll).build::<H>(p, col().w_full().shrink().min_h(0.0), results);
    build_list_frame(p, search_input, scroll)
}

fn server_row<H: McpHost + 'static>(s: &H, p: &Pal, dir: &str, m: &McpServer, active: bool, tall: bool) -> El {
    let (name, hover) = (m.name.clone(), m.name.clone());
    server_row_shell(p, m, active, tall, trailing(s, p, dir, m))
        .on_hover_change(move |s: &mut H, inside, tx: &mut Tx| crate::set_active_on_hover(s, &hover, inside, tx))
        .on_press_tx(move |s: &mut H, tx: &mut Tx| select(s, &name, tx))
}

/// The switch or Sign in a server row ends with, and View error before it when it failed; the
/// Settings page's rows end the same.
pub fn trailing<H: McpHost + 'static>(s: &H, p: &Pal, dir: &str, m: &McpServer) -> El {
    let busy = crate::busy(s, dir, &m.name);
    let view_error = matches!(m.status, McpStatus::Failed(_)).then(|| {
        let (name, dir2) = (m.name.clone(), dir.to_owned());
        action_shell(p, "mcp-view-error", opencode_i18n::t!("dialog.mcp.error.view"), &opencode_i18n::t!("dialog.mcp.error.viewLabel", name = m.name), false)
            .on_press_tx(move |s: &mut H, tx: &mut Tx| crate::open_error_from_row(s, &dir2, &name, tx))
    });
    let (dir, name) = (dir.to_owned(), m.name.clone());
    if matches!(m.status, McpStatus::NeedsAuth(_)) && m.integration.is_some() {
        let label = if busy { opencode_i18n::t!("dialog.mcp.signingIn") } else { opencode_i18n::t!("dialog.mcp.signIn") };
        let mut sign_in_btn = action_shell(p, "mcp-sign-in", label, &opencode_i18n::t!("dialog.mcp.signIn.open", name = m.name), busy);
        if !busy {
            sign_in_btn = sign_in_btn.on_press_tx(move |s: &mut H, tx: &mut Tx| crate::toggle_from_ui(s, &dir, &name, true, tx));
        }
        // `.mcp-status-action:last-child`: the label lines up with the switches above and below it.
        return build_trailing(view_error, sign_in_btn.mr(-5.0));
    }
    let on = m.status != McpStatus::Disabled;
    let pending = busy || m.status == McpStatus::Pending;
    // The switch's label holds only the name in an `sr-only` span, so it lays out empty, yet it
    // is still a flex item 8 pt before the control (`switch.css` gap), and the switch stands 32
    // wide (`mcp-dialog.tsx`, `extensions.tsx`).
    let switch = Switch::new(on)
        .web()
        .label(m.name.clone())
        .hide_label()
        .disabled(pending)
        .on_toggle_tx(move |s: &mut H, tx: &mut Tx| crate::toggle_from_ui(s, &dir, &name, !on, tx))
        .build(kit(p))
        .key("mcp-switch");
    build_trailing(view_error, row().key("mcp-switch-root").pl(8.0).center_y().child(switch))
}

fn detail<H: McpHost + 'static>(s: &H, p: &Pal, dir: &str, name: &str, error: &str, copied: bool) -> El {
    let busy = crate::detail_busy(s, dir, name);
    let back = back_button_shell(p).on_press_tx(|s: &mut H, tx: &mut Tx| crate::close_detail_from_ui(s, tx));
    let close = close_button_shell(p).on_press_tx(|s: &mut H, tx: &mut Tx| crate::close_dialog(s, tx));
    let details = opencode_i18n::t!("dialog.mcp.error.details", name = name, error = error);
    let (d1, n1, d2, n2) = (dir.to_owned(), name.to_owned(), dir.to_owned(), name.to_owned());
    let error_text = error.to_owned();
    let copy_btn = button(
        if copied { opencode_i18n::t!("common.copied") } else { opencode_i18n::t!("dialog.mcp.error.copy") },
        ButtonVariant::GhostMuted,
    )
    .icon(if copied { &ic::CHECK } else { &ic::COPY })
    .on_press_tx(move |s: &mut H, tx: &mut Tx| crate::copy_error(s, &details, error_text.clone(), tx))
    .build(kit(p));
    let disable_btn = button(opencode_i18n::t!("dialog.mcp.error.disable"), ButtonVariant::Ghost)
        .disabled(busy)
        .on_press_tx(move |s: &mut H, tx: &mut Tx| crate::toggle_from_ui(s, &d1, &n1, false, tx))
        .build(kit(p));
    let retry_btn = button(opencode_i18n::t!("common.retry"), ButtonVariant::Neutral)
        .disabled(busy)
        .on_press_tx(move |s: &mut H, tx: &mut Tx| crate::toggle_from_ui(s, &d2, &n2, true, tx))
        .build(kit(p));
    build_detail_layout(p, name, error, DetailButtons { back, close, copy_btn, disable_btn, retry_btn })
}

struct DetailButtons {
    back: El,
    close: El,
    copy_btn: El,
    disable_btn: El,
    retry_btn: El,
}

/// `<Dialog size="large" fit>` (`providers/connect/mcp-dialog.tsx`).
fn build_dialog_card(p: &Pal, win: [f32; 2], body: El) -> (El, f32) {
    let size = DialogSize::Large;
    let card = ui::overlay(p, col())
        .key("mcp-dialog")
        .role("dialog")
        .label_static(opencode_i18n::t!("dialog.mcp.title"))
        .w(size.width(win[0]))
        .max_h(size.height(win[1]))
        .radius(12.0)
        .fill(p.bg_base)
        .clip_soft(0.0)
        .transition(Transition::none())
        .motion(Spring::NONE)
        .child(body);
    (card, size.top(win[1]))
}

fn search_input_shell(edit: &rune::TextEdit, p: &Pal) -> El {
    field(edit, UI)
        .key("mcp-search")
        .single_line()
        .grow(1.0)
        .shrink()
        .min_w(0.0)
        .color(p.text)
        .placeholder(opencode_i18n::t!("dialog.mcp.search.placeholder"), p.text.alpha(p.text.a * 0.5))
        .caret(p.text, p.selection)
        .selection_style(p.field_selection)
        .label_static(opencode_i18n::t!("dialog.mcp.search.placeholder"))
}

fn build_list_frame(p: &Pal, search_input: El, scroll: El) -> El {
    let search_field = row()
        .key("mcp-search-field")
        .w_full()
        .h(28.0)
        .pr(6.0)
        .gap(6.0)
        .center_y()
        .child(el().pl(6.0).child(icon(&ic::MAGNIFYING_GLASS, 16.0).color(p.icon_muted)))
        .child(search_input);
    // `.command-palette-compact`: 6 pt around, 17 below, the hairline 8 pt over the bottom.
    let search = col()
        .key("mcp-search-row")
        .w_full()
        .pt(6.0)
        .px(6.0)
        .pb(8.0)
        .child(search_field)
        .child(el().h(0.5).mt(8.5).mx(-6.0).fill(p.border));
    col().key("mcp-list").w_full().min_h(0.0).shrink().child(search).child(scroll)
}

fn build_list_results(p: &Pal, total_servers: usize, rows: Vec<El>) -> El {
    let mut results = col().key("mcp-results").w_full().px(6.0).pb(6.0).gap(4.0);
    if rows.is_empty() {
        let words = if total_servers > 0 { opencode_i18n::t!("dialog.mcp.search.empty") } else { opencode_i18n::t!("dialog.mcp.empty") };
        results = results.child(row().key("mcp-empty").w_full().min_h(120.0).center().child(text(words).font(UI).color(p.muted)));
    }
    results.children(rows)
}

fn server_row_shell(p: &Pal, m: &McpServer, active: bool, tall: bool, trailing: El) -> El {
    let r = row().key(format!("mcp-row-{}", m.name)).role("option").label(m.name.clone()).w_full();
    let r = if tall { r.min_h(44.0).py(8.0) } else { r.h(28.0) };
    r.px(6.0)
        .gap(16.0)
        .center_y()
        .radius(6.0)
        .fill(if active { p.hover } else { Color::TRANSPARENT })
        .transition(Transition::none())
        .child(
            row()
                .grow(1.0)
                .shrink()
                .min_w(0.0)
                .gap(6.0)
                .center_y()
                .child(status_dot(p, m.status.name()))
                .child(text(m.name.clone()).font(LABEL).color(if m.status == McpStatus::Disabled { p.muted } else { p.text }).shrink().truncate()),
        )
        .child(trailing)
}

fn build_trailing(view_error: Option<El>, control: El) -> El {
    row().key("mcp-trailing").gap(8.0).center_y().children(view_error).child(control)
}

/// `.mcp-status-action`: a quiet small ghost-muted button in a switch's weight.
fn action_shell(p: &Pal, key: &str, label: &str, a11y: &str, busy: bool) -> El {
    let e = row()
        .key(key.to_owned())
        .role("button")
        .label(a11y.to_owned())
        .h(20.0)
        .px(5.0)
        .center()
        .radius(4.0)
        .child(text(label.to_owned()).font(ACTION).color(p.muted));
    if busy { e.opacity(0.5) } else { e.hover(p.hover).press(p.pressed) }
}

fn back_button_shell(p: &Pal) -> El {
    el().key("mcp-back")
        .role("button")
        .label_static(opencode_i18n::t!("common.goBack"))
        .size(20.0, 20.0)
        .radius(4.0)
        .center()
        .hover(p.hover)
        .press(p.pressed)
        .child(icon(&ic::ARROW_LEFT, 14.0).mirror_in_rtl().color(p.icon_muted))
}

fn close_button_shell(p: &Pal) -> El {
    el().key("mcp-close")
        .role("button")
        .label_static(opencode_i18n::t!("common.close"))
        .size(20.0, 20.0)
        .mt(-2.0)
        .mr(-2.0)
        .radius(4.0)
        .center()
        .hover(p.hover)
        .press(p.pressed)
        .child(icon(&DIALOG_CLOSE, 16.0).color(p.icon_muted))
}

fn build_detail_layout(p: &Pal, name: &str, error: &str, buttons: DetailButtons) -> El {
    let header = row()
        .key("mcp-error-header")
        .w_full()
        .pt(16.0)
        .px(16.0)
        .pb(12.0)
        .child(
            col()
                .grow(1.0)
                .shrink()
                .gap(4.0)
                .child(row().gap(8.0).child(buttons.back).child(text(name.to_owned()).font(TITLE).color(p.text).wrap_anywhere().grow(1.0).shrink()))
                .child(text(opencode_i18n::t!("dialog.mcp.error.status")).font(BODY).color(p.muted)),
        )
        .child(buttons.close);
    let actions = row()
        .key("mcp-error-actions")
        .w_full()
        .gap(8.0)
        .child(buttons.copy_btn)
        .child(spacer())
        .child(buttons.disable_btn)
        .child(buttons.retry_btn);
    col()
        .key("mcp-error")
        .w_full()
        .child(header)
        .child(
            col()
                .key("mcp-error-details")
                .w_full()
                .px(16.0)
                .pb(16.0)
                .gap(12.0)
                .child(
                    col()
                        .w_full()
                        .py(8.0)
                        .px(12.0)
                        .radius(6.0)
                        .fill(p.layer3)
                        .child(text(error.to_owned()).font(ERROR).color(p.text).wrap_anywhere().selectable(true).selection_style(p.field_selection)),
                )
                .child(actions),
        )
}
