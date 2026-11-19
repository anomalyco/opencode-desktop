//! A connected integration's accounts step (`AccountsView`): pick the active account, rename or
//! remove one from its options menu, or add another.

use std::time::Duration;

use opencode_kit::integration::Connection;
use rune::{El, Key, KeyEvent, TextEdit, Tx, col, el, icon, row, spacer, text};
use rune_kit::{ButtonVariant, TextInput};
use rune_theme_opencode::icons as ic;
use opencode_ui::palette::{Pal, kit};

use super::{open_url, provider_id};
use crate::catalog;
use crate::dialog::{AccountAction, Event, Step};
use crate::machine as conn;
use crate::run;
use crate::ui::{self, BODY};
use crate::{ConnectHost, ConnectJob, ConnectRequest};

/// Saves the account's new name (`AccountNameInput`), then leaves the field.
fn rename_account(host: &mut dyn ConnectHost, credential: &str, tx: &mut Tx) {
    let label = host.connect().dialog().map(|o| o.rename.text.trim().to_owned()).unwrap_or_default();
    crate::connect_event(host, Event::RenameClosed, tx);
    if label.is_empty() {
        return;
    }
    if !host.can_write() {
        host.toast_fail(opencode_i18n::t!("common.requestFailed").to_owned(), run::read_only());
        return;
    }
    let dir = crate::connect_dir(host).map(str::to_owned);
    host.spawn_job(ConnectJob::Server {
        delay: Duration::ZERO,
        request: ConnectRequest::AccountRename { dir, credential: credential.to_owned(), label },
    });
}

fn rename_key(host: &mut dyn ConnectHost, ev: &KeyEvent, tx: &mut Tx) -> bool {
    if ev.key != Key::Escape {
        return false;
    }
    crate::connect_event(host, Event::RenameClosed, tx);
    true
}

/// A connection that needs a fresh sign-in has its own link or signs in again.
fn choose_account(host: &mut dyn ConnectHost, a: &Connection, tx: &mut Tx) {
    tx.instant();
    match &a.status {
        Some(st) if st.url.is_some() => open_url(host, st.url.as_deref().unwrap_or_default(), tx),
        Some(_) => crate::connect_event(host, Event::AccountAdd, tx),
        None => crate::connect_event(host, Event::ActivateRequested(a.id.clone()), tx),
    }
}

fn toggle_menu(host: &mut dyn ConnectHost, id: &str, tx: &mut Tx) {
    tx.instant();
    if let Some(o) = host.connect_mut().dialog_mut() {
        o.menu = if o.menu.as_deref() == Some(id) { None } else { Some(id.to_owned()) };
    }
}

fn start_rename(host: &mut dyn ConnectHost, id: &str, tx: &mut Tx) {
    let pid = provider_id(host);
    let label = catalog::integration_of(host, crate::connect_dir(host), &pid)
        .and_then(|i| i.connections.iter().find(|c| c.id == id).map(|c| c.label.clone()))
        .unwrap_or_default();
    crate::connect_event(host, Event::RenameStarted(id.to_owned()), tx);
    if let Some(o) = host.connect_mut().dialog_mut() {
        o.menu = None;
        o.rename = TextEdit::new(&label);
        o.rename.focused = true;
    }
}

fn prompt_remove(host: &mut dyn ConnectHost, id: &str, tx: &mut Tx) {
    crate::connect_event(host, Event::RemovePrompted(id.to_owned()), tx);
    if let Some(o) = host.connect_mut().dialog_mut() {
        o.menu = None;
    }
}

fn cancel_remove(host: &mut dyn ConnectHost, tx: &mut Tx) {
    crate::connect_event(host, Event::RemoveCancelled, tx);
}

fn confirm_remove(host: &mut dyn ConnectHost, id: &str, tx: &mut Tx) {
    crate::connect_event(host, Event::RemoveConfirmed(id.to_owned()), tx);
}

fn add_account(host: &mut dyn ConnectHost, tx: &mut Tx) {
    crate::connect_event(host, Event::AccountAdd, tx);
}

pub(super) fn view<H: ConnectHost + 'static>(s: &H, p: &Pal) -> El {
    let Some(o) = s.connect().dialog() else { return col() };
    let Some(Step::Accounts { action, .. }) = o.dialog.provider.as_ref().map(|p| &p.step) else { return col() };
    let id = provider_id(s);
    let busy = matches!(action, AccountAction::Activating { .. } | AccountAction::Removing { .. });
    let active = match action {
        AccountAction::Activating { account, .. } => Some(account.clone()),
        _ => o.dialog.provider.as_ref().and_then(|p| p.accounts.active.clone()),
    };
    let mut list: Vec<Connection> = catalog::integration_of(s, crate::connect_dir(s), &id)
        .map(|i| i.accounts().into_iter().cloned().collect())
        .unwrap_or_default();
    list.sort_by(|a, b| rune::text::compare_names(&a.label, &b.label).then(a.id.cmp(&b.id)));
    let mut rows = col()
        .key("provider-accounts")
        .role("radiogroup")
        .label_static(opencode_i18n::t!("dialog.provider.accounts.connected"))
        .gap(2.0);
    for a in list {
        if *action == AccountAction::Confirming(a.id.clone()) {
            let confirm = a.id.clone();
            rows = rows.child(
                row()
                    .min_h(28.0)
                    .pl(28.0)
                    .pr(2.0)
                    .gap(6.0)
                    .center_y()
                    .radius(6.0)
                    .child(text(opencode_i18n::t!("dialog.provider.accounts.removeConfirm", account = a.label)).role("alert").font(BODY).color(p.text).grow(1.0).shrink().truncate())
                    .child(ui::button(opencode_i18n::t!("common.cancel"), ButtonVariant::GhostMuted).on_press_tx(|s: &mut H, tx: &mut Tx| cancel_remove(s, tx)).build(kit(p)))
                    .child(
                        ui::button(opencode_i18n::t!("dialog.provider.accounts.remove"), ButtonVariant::Neutral)
                            .on_press_tx(move |s: &mut H, tx: &mut Tx| confirm_remove(s, &confirm, tx))
                            .build(kit(p)),
                    ),
            );
            continue;
        }
        if *action == AccountAction::Renaming(a.id.clone()) {
            let credential = a.id.clone();
            // `account-name.tsx`'s InlineInput is `dir="auto"`.
            let field = TextInput::new(&o.rename)
                .label(opencode_i18n::t!("settings.providers.account.name"))
                .width(None)
                .plain()
                .spellcheck(s.spellcheck())
                .dir(rune::Direction::Auto)
                .on_edit(|s: &mut H| s.connect_mut().rename_mut())
                .on_submit(move |s: &mut H, tx: &mut Tx| rename_account(s, &credential, tx))
                .on_key(|s: &mut H, ev, tx| rename_key(s, ev, tx))
                .build(kit(p));
            rows = rows.child(
                row()
                    .key(format!("account-rename-{}", a.id))
                    .w_full()
                    .min_h(28.0)
                    .pl(24.0)
                    .pr(8.0)
                    .center_y()
                    .radius(6.0)
                    .fill(p.hover)
                    .child(field),
            );
            continue;
        }
        let is_active = active.as_deref() == Some(a.id.as_str());
        let pick = a.clone();
        let mut line = row()
            .key(format!("account-{}", a.id))
            .role("radio")
            .label(a.label.clone())
            .grow(1.0)
            .shrink()
            .min_h(28.0)
            .pl(6.0)
            .pr(8.0)
            .gap(6.0)
            .center_y()
            .child(el().size(16.0, 16.0).center().children(is_active.then(|| icon(&ic::CHECK_SMALL, 14.0).color(p.icon))))
            .child(text(a.label.clone()).font(BODY).color(p.text).shrink().truncate());
        if a.status.is_some() {
            line = line.child(spacer()).child(text(opencode_i18n::t!("dialog.provider.needsSignIn")).font(BODY).color(p.warning));
        }
        if !busy {
            line = line.on_press_tx(move |s: &mut H, tx: &mut Tx| choose_account(s, &pick, tx));
        }
        let menu_id = a.id.clone();
        let options = el()
            .key(format!("account-options-{}", a.id))
            .role("button")
            .label(opencode_i18n::t!("dialog.provider.accounts.options", account = a.label))
            .size(24.0, 24.0)
            .mr(4.0)
            .radius(4.0)
            .center()
            .hover(p.hover)
            .press(p.pressed)
            .on_press_tx(move |s: &mut H, tx: &mut Tx| toggle_menu(s, &menu_id, tx))
            .child(icon(&ic::OUTLINE_DOTS, 16.0).color(p.icon_muted));
        let mut account_row = row().w_full().min_h(28.0).radius(6.0).center_y().hover(p.hover).child(line).child(options);
        if o.menu.as_deref() == Some(a.id.as_str()) {
            account_row = account_row.child(account_menu::<H>(p, &a.id));
        }
        rows = rows.child(account_row);
    }
    let can_add = !busy && o.dialog.provider.as_ref().and_then(|p| p.step.connect()).and_then(conn::catalog).is_some_and(|c| !c.methods.is_empty());
    let mut add = row()
        .key("provider-account-add")
        .role("button")
        .label_static(opencode_i18n::t!("dialog.provider.accounts.add"))
        .w_full()
        .h(28.0)
        .pl(6.0)
        .pr(8.0)
        .gap(6.0)
        .center_y()
        .radius(6.0)
        .hover(p.hover)
        .press(p.pressed)
        .child(el().size(16.0, 16.0).center().child(icon(&ic::PLUS, 16.0).color(p.icon_muted)))
        .child(text(opencode_i18n::t!("dialog.provider.accounts.add")).font(BODY).color(p.muted));
    add = if can_add { add.on_press_tx(|s: &mut H, tx: &mut Tx| add_account(s, tx)) } else { add.opacity(0.5) };
    col()
        .key("connect-accounts")
        .w_full()
        .gap(2.0)
        .child(el().h(0.5).mt(4.0).mb(6.0).mx(-6.0).fill(p.border_muted))
        .child(rows)
        .child(add)
}

fn account_menu<H: ConnectHost + 'static>(p: &Pal, id: &str) -> El {
    let (rename, remove) = (id.to_owned(), id.to_owned());
    let item = |key: &str, label: &str| {
        row()
            .key(key.to_owned())
            .role("menuitem")
            .label(label.to_owned())
            .w_full()
            .h(28.0)
            .px(8.0)
            .center_y()
            .radius(4.0)
            .hover(p.hover)
            .child(text(label.to_owned()).font(BODY).color(p.text))
    };
    ui::overlay(p, col())
        .key("account-menu")
        .pin(None, Some(28.0), Some(4.0), None)
        .w(160.0)
        .padding(4.0)
        .radius(8.0)
        .fill(p.bg_base)
        .raise()
        .child(item("account-rename", opencode_i18n::t!("common.rename")).on_press_tx(move |s: &mut H, tx: &mut Tx| start_rename(s, &rename, tx)))
        .child(
            item("account-remove", opencode_i18n::t!("dialog.provider.accounts.remove"))
                .on_press_tx(move |s: &mut H, tx: &mut Tx| prompt_remove(s, &remove, tx)),
        )
}
