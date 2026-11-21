//! Settings › Providers (`settings/providers/providers.tsx`): the providers the server runs, each
//! with how it connects (API key, account, environment, config) and its accounts or Disconnect,
//! then the popular providers to connect and Show more providers, which opens the connect list.

use opencode_i18n::t;
use opencode_kit::integration::{Connection, Integration};
use opencode_ui::popup::PopupHost;
use opencode_ui::widgets::{self, DialogSize, Fit, badge, button, dialog_window, sized_modal};
use rune::machine::Effects;
use rune::{Color, Cx, El, Key, KeyEvent, TextEdit, Tx, col, el, icon, row, spacer, text};
use rune_kit::ButtonVariant;
use rune_theme_opencode::icons as ic;
use opencode_ui::palette::{Pal, kit};

use super::{BODY, NAME, catalog, header_with, page, section};
use crate::PROVIDER_REMOVAL_OVERLAY_ID;
use crate::host::{ProviderRow, SettingsHost};
use crate::jobs::{SettingsJob, SettingsRequest};

/// Desktop's popular providers, in its order (`catalog/order.ts`).
const POPULAR: [&str; 8] = ["opencode-go", "opencode", "anthropic", "github-copilot", "openai", "google", "openrouter", "vercel"];

/// Which of a provider's account menus is open.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Menu {
    Accounts,
    Options,
}

/// The Connected rows' account controls. At most one row's menu or rename field is open, named
/// by its provider.
#[derive(Default)]
pub enum Accounts {
    #[default]
    Idle,
    Menu { provider: String, menu: Menu },
    Renaming { provider: String, credential: String, field: Box<TextEdit> },
    Confirm(Removal),
}

#[derive(Clone, Debug)]
pub enum Removal {
    Provider { id: String, name: String, description: String },
    Account { credential: String, provider: String, label: String, last: bool },
}

pub enum Event {
    /// A row's account trigger or options button: opens its menu, or closes it when already open.
    Toggle { provider: String, menu: Menu },
    /// An item was picked, or the page changed: the open menu closes.
    Close,
    /// Rename in the open row's options: its active account becomes a field.
    Rename { credential: String, label: String },
    Cancel,
    Submit,
    Ask(Removal),
}

pub enum Effect {
    Rename { credential: String, label: String },
    Remove(Removal),
}

impl Accounts {
    pub fn step(&mut self, event: Event) -> Effects<Effect> {
        match (std::mem::take(self), event) {
            (_, Event::Ask(target)) => {
                *self = Accounts::Confirm(target);
                Effects::none()
            }
            (Accounts::Confirm(target), Event::Submit) => Effects::one(Effect::Remove(target)),
            (Accounts::Confirm(_), Event::Cancel | Event::Close) => Effects::none(),
            (Accounts::Menu { provider, menu }, Event::Toggle { provider: p, menu: m }) if provider == p && menu == m => Effects::none(),
            (_, Event::Toggle { provider, menu }) => {
                *self = Accounts::Menu { provider, menu };
                Effects::none()
            }
            (Accounts::Menu { provider, .. }, Event::Rename { credential, label }) => {
                let mut field = TextEdit::new(&label);
                field.focused = true;
                *self = Accounts::Renaming { provider, credential, field: Box::new(field) };
                Effects::none()
            }
            (Accounts::Menu { .. }, Event::Close) | (Accounts::Renaming { .. }, Event::Cancel) => Effects::none(),
            (Accounts::Renaming { credential, field, .. }, Event::Submit) => {
                let label = field.text.trim().to_owned();
                if label.is_empty() { Effects::none() } else { Effects::one(Effect::Rename { credential, label }) }
            }
            (state, _) => {
                *self = state;
                Effects::none()
            }
        }
    }

    pub fn blur(&mut self) {
        if let Accounts::Renaming { field, .. } = self {
            field.focused = false;
        }
    }

    fn open(&self, row: &str) -> Option<Menu> {
        match self {
            Accounts::Menu { provider, menu } if provider == row => Some(*menu),
            _ => None,
        }
    }

    pub(crate) fn field_mut(&mut self) -> Option<&mut TextEdit> {
        match self {
            Accounts::Renaming { field, .. } => Some(field.as_mut()),
            _ => None,
        }
    }
}

fn note(id: &str) -> Option<&'static str> {
    Some(match id {
        "opencode" => t!("dialog.provider.opencode.note"),
        "opencode-go" => t!("dialog.provider.opencodeGo.tagline"),
        "anthropic" => t!("dialog.provider.anthropic.note"),
        "claude-agent" => t!("dialog.provider.claudeAgent.note"),
        "openai" => t!("dialog.provider.openai.note"),
        "google" => t!("dialog.provider.google.note"),
        "openrouter" => t!("dialog.provider.openrouter.note"),
        "vercel" => t!("dialog.provider.vercel.note"),
        _ if id.starts_with("github-copilot") => t!("dialog.provider.copilot.note"),
        _ => return None,
    })
}

/// Steps the Connected rows' account state machine and executes resulting server effects.
pub fn step_accounts(s: &mut dyn SettingsHost, event: Event, tx: &mut Tx) {
    tx.instant();
    let was_confirm = matches!(s.settings_pages().accounts, Accounts::Confirm(_));
    let effects = s.settings_pages_mut().accounts.step(event);
    let is_confirm = matches!(s.settings_pages().accounts, Accounts::Confirm(_));
    if !was_confirm && is_confirm {
        let return_focus = tx.focused().map(str::to_owned);
        s.on_overlay_opened(PROVIDER_REMOVAL_OVERLAY_ID, return_focus);
    } else if was_confirm && !is_confirm {
        s.on_overlay_closed(PROVIDER_REMOVAL_OVERLAY_ID);
    }
    for effect in effects {
        match effect {
            Effect::Remove(Removal::Provider { id, name, .. }) => disconnect_provider(s, &id, &name),
            Effect::Remove(Removal::Account { credential, provider, .. }) => remove_account(s, &credential, &provider, tx),
            Effect::Rename { credential, label } => rename_account(s, credential, label),
        }
    }
}

fn ask_disconnect_provider(s: &mut dyn SettingsHost, id: &str, name: &str, tx: &mut Tx) {
    let providers = s.provider_rows(None);
    let count = providers.iter().filter(|pi| pi.integration.as_deref().unwrap_or(&pi.id) == id).count();
    let description = if id == "opencode" && count > 1 {
        let workspace = s
            .integrations(None)
            .unwrap_or_default()
            .iter()
            .find(|i| i.id == id)
            .and_then(|i| i.connections.iter().find(|c| c.credential))
            .map_or(t!("usage.workspace.fallback"), |c| c.label.as_str());
        opencode_i18n::plural!("settings.providers.disconnect.consoleDescription", count, workspace = workspace)
    } else {
        t!("settings.providers.disconnect.description").to_owned()
    };
    step_accounts(s, Event::Ask(Removal::Provider { id: id.to_owned(), name: name.to_owned(), description }), tx);
}

fn open_connect_from_settings(s: &mut dyn SettingsHost, provider: Option<&str>, tx: &mut Tx) {
    s.open_connect(provider, true, tx);
}

fn add_account(s: &mut dyn SettingsHost, id: &str, tx: &mut Tx) {
    step_accounts(s, Event::Close, tx);
    s.open_connect(Some(id), true, tx);
}

fn rename_key(s: &mut dyn SettingsHost, ev: &KeyEvent, tx: &mut Tx) -> bool {
    if ev.key != Key::Escape || ev.mods.cmd || ev.mods.ctrl || ev.mods.alt || ev.mods.shift {
        return false;
    }
    step_accounts(s, Event::Cancel, tx);
    true
}

/// `DialogConfirmProviderRemoval` (#64212), shared by Disconnect and Remove account.
pub fn confirmation<H: SettingsHost + 'static>(s: &H, p: &Pal, cx: &Cx<H>) -> Vec<El> {
    let Accounts::Confirm(target) = &s.settings_pages().accounts else { return vec![] };
    let (title, description, action) = match target {
        Removal::Provider { name, description, .. } => (t!("settings.providers.disconnect.title", provider = name), description.clone(), t!("common.disconnect")),
        Removal::Account { label, provider, last, .. } => {
            let description = if *last {
                t!("settings.providers.account.remove.finalDescription", provider = provider)
            } else {
                t!("settings.providers.account.remove.description", provider = provider)
            };
            (t!("settings.providers.account.remove.title", account = label), description, t!("dialog.provider.accounts.remove"))
        }
    };
    let cancel = |s: &mut H, tx: &mut Tx| step_accounts(s, Event::Cancel, tx);
    let card = widgets::overlay(p, col())
        .key("provider-removal")
        .role("alertdialog")
        .label(title.clone())
        .radius(12.0)
        .fill(p.layer1)
        .modal_tx(true, cancel)
        .child(col().padding(16.0).gap(4.0).child(text(title).font(NAME).color(p.text)).child(text(description).font(BODY).color(p.muted).wrap()))
        .child(
            row()
                .padding(16.0)
                .gap(8.0)
                .child(spacer())
                .child(button(t!("common.cancel"), ButtonVariant::Neutral).on_press_tx(cancel).build(kit(p)).autofocus())
                .child(button(action, ButtonVariant::Danger).on_press_tx(|s: &mut H, tx| step_accounts(s, Event::Submit, tx)).build(kit(p))),
        );
    sized_modal("provider-removal-modal", p, DialogSize::Normal, Fit::Content, dialog_window(cx, s.zoom()), card)
}

fn source(i: Option<&Integration>) -> &'static str {
    let Some(i) = i else { return t!("settings.providers.tag.other") };
    match i.connections.iter().find(|c| c.credential) {
        Some(c) if c.method == "key" => t!("provider.connect.method.apiKey"),
        Some(_) => t!("settings.providers.tag.account"),
        None if i.connections.iter().any(|c| !c.credential) => t!("settings.providers.tag.environment"),
        None => t!("settings.providers.tag.config"),
    }
}

pub fn view<H: SettingsHost + PopupHost + 'static>(s: &H, p: &Pal) -> El {
    let integrations = s.integrations(None).unwrap_or_default();
    let integration = |pi: &ProviderRow| integrations.iter().find(|i| i.id == pi.integration.as_deref().unwrap_or(&pi.id));
    let console = integrations.iter().find(|i| i.id == "opencode");
    let console_connected = console.is_some_and(|i| !i.connections.is_empty());
    let paid = |id: &str| s.models().iter().any(|m| m.provider == id && !m.free);
    let provider_rows = s.provider_rows(None);
    let mut connected: Vec<&ProviderRow> = provider_rows.iter().filter(|pi| pi.id != "opencode" || console_connected || paid(&pi.id)).collect();
    connected.sort_by_key(|pi| pi.id != "opencode-go");
    let rows: Vec<El> = connected.iter().map(|pi| connected_row(s, p, pi, integration(pi))).collect();
    let connected_list = if rows.is_empty() {
        catalog(p, "connected-providers", vec![el().py(16.0).child(text(t!("settings.providers.connected.empty")).font(super::NAME_REGULAR).color(p.muted))])
    } else {
        catalog(p, "connected-providers", rows)
    };
    let ids: Vec<&str> = connected.iter().map(|pi| pi.id.as_str()).collect();
    let mut popular: Vec<(String, String)> = integrations
        .iter()
        .filter(|i| POPULAR.contains(&i.id.as_str()))
        .filter(|i| {
            if i.id == "opencode" {
                i.connections.iter().find(|c| c.credential).is_none_or(|c| c.method != "oauth")
            } else {
                !ids.contains(&i.id.as_str())
            }
        })
        .map(|i| (i.id.clone(), i.name.clone()))
        .collect();
    popular.sort_by_key(|(id, _)| POPULAR.iter().position(|p| p == id));
    let more = text(t!("dialog.provider.viewAll"))
        .key("settings-providers-view-all")
        .role("button")
        .font(NAME)
        .color(p.accent)
        // Inside `.settings-section` (`gap: 16px`) plus `.settings-providers-view-all { margin-top: 20px }` (`settings.css`).
        .mt(36.0)
        .on_press_tx(|s: &mut H, tx: &mut Tx| open_connect_from_settings(s, None, tx));
    let body = col()
        .key("settings-providers")
        .w_full()
        .gap(32.0)
        .mt(8.0)
        .child(section(p, "connected-providers-section", Some(t!("settings.providers.section.connected")), connected_list))
        .child(
            col()
                .w_full()
                .child(section(
                    p,
                    "popular-providers-section",
                    Some(t!("settings.providers.section.popular")),
                    catalog(p, "popular-providers", popular.iter().map(|(id, name)| popular_row::<H>(p, id, name)).collect()),
                ))
                .child(more),
        );
    page(p, "settings-providers-page", header_with(p, t!("settings.providers.title"), t!("settings.providers.description"), super::servers::switcher(s, p)), body)
}

fn connected_row<H: SettingsHost + 'static>(s: &H, p: &Pal, pi: &ProviderRow, integration: Option<&Integration>) -> El {
    let lead = row()
        .grow(1.0)
        .shrink()
        .min_w(0.0)
        .gap(8.0)
        .center_y()
        .child(widgets::provider_mark(&pi.id, if p.dark { Color::WHITE } else { Color::BLACK }))
        .child(row().gap(8.0).center_y().child(text(pi.name.clone()).font(NAME).color(p.text).truncate()).child(badge(p, source(integration))));
    let accounts = integration.map(Integration::accounts).unwrap_or_default();
    let can_disconnect = integration.is_some_and(|i| i.connections.iter().any(|c| c.credential));
    let id = pi.integration.clone().unwrap_or_else(|| pi.id.clone());
    let trailing = if !accounts.is_empty() {
        let mut e = row().gap(0.0).center_y();
        if accounts.first().is_some_and(|a| a.status.is_some()) {
            let (again, warning) = (id.clone(), p.warning);
            e = e.child(
                button(t!("dialog.provider.needsSignIn"), ButtonVariant::GhostMuted)
                    .label_with(move |font, _| text(t!("dialog.provider.needsSignIn")).font(font).color(warning))
                    .on_press_tx(move |s: &mut H, tx: &mut Tx| open_connect_from_settings(s, Some(&again), tx))
                    .build(kit(p))
                    .key("settings-provider-sign-in"),
            );
        }
        e.child(account_menu(s, p, &pi.id, &id, &pi.name, integration))
    } else if can_disconnect {
        let (id, name) = (id.clone(), pi.name.clone());
        button(t!("common.disconnect"), ButtonVariant::GhostMuted)
            .on_press_tx(move |s: &mut H, tx: &mut Tx| ask_disconnect_provider(s, &id, &name, tx))
            .build(kit(p))
    } else {
        text(t!("settings.providers.connected.environmentDescription"))
            .key("settings-provider-env-hint")
            .font(super::NAME_REGULAR)
            .color(p.muted)
            .pr(12.0)
            .group_opacity(0.0, 1.0)
    };
    row()
        .key(format!("settings-provider-{}", pi.id))
        .w_full()
        .min_h(48.0)
        .py(10.0)
        .gap(16.0)
        .center_y()
        .hover_group()
        .child(lead)
        .child(trailing)
}

fn popular_row<H: SettingsHost + 'static>(p: &Pal, id: &str, name: &str) -> El {
    let mut main = row().gap(8.0).center_y().child(text(name.to_owned()).font(NAME).color(p.text));
    if id == "opencode" || id == "opencode-go" {
        main = main.child(badge(p, t!("dialog.provider.tag.recommended")));
    }
    let copy = col().grow(1.0).shrink().min_w(0.0).gap(6.0).child(main).children(note(id).map(|n| text(n).font(BODY).color(p.muted).wrap()));
    let open = id.to_owned();
    row()
        .key(format!("settings-popular-{id}"))
        .w_full()
        .py(16.0)
        .gap(16.0)
        .center_y()
        .child(
            row()
                .grow(1.0)
                .shrink()
                .min_w(0.0)
                .gap(8.0)
                .child(widgets::provider_mark(id, if id == "opencode" { if p.dark { Color::WHITE } else { Color::BLACK } } else { p.icon }))
                .child(copy),
        )
        .child(
            button(t!("common.connect"), ButtonVariant::Neutral)
                .icon(&ic::PLUS)
                .on_press_tx(move |s: &mut H, tx: &mut Tx| open_connect_from_settings(s, Some(&open), tx))
                .build(kit(p))
                .key(format!("settings-connect-{id}")),
        )
}

fn account_menu<H: SettingsHost + 'static>(s: &H, p: &Pal, provider: &str, id: &str, name: &str, integration: Option<&Integration>) -> El {
    let accounts = integration.map(Integration::accounts).unwrap_or_default();
    let active = accounts.first().map(|a| (a.id.clone(), a.label.clone()));
    let open = s.settings_pages().accounts.open(provider);
    if let (Accounts::Renaming { provider: row, credential, field }, Some((active_id, _))) = (&s.settings_pages().accounts, &active)
        && row == provider
        && credential == active_id
    {
        return rune_kit::TextInput::new(field)
            .label(t!("settings.providers.account.name"))
            .width(Some(240.0))
            .spellcheck(s.settings().spellcheck)
            .dir(rune::Direction::Auto)
            .on_edit(|s: &mut H| s.settings_pages_mut().account_rename_field_mut())
            .on_submit(|s: &mut H, tx: &mut Tx| step_accounts(s, Event::Submit, tx))
            .on_key(|s: &mut H, ev: &KeyEvent, tx: &mut Tx| rename_key(s, ev, tx))
            .build(kit(p));
    }
    let (menu_id, options_id) = (provider.to_owned(), provider.to_owned());
    let label = active.as_ref().map(|a| a.1.clone()).unwrap_or_default();
    let options = t!("dialog.provider.accounts.options", account = label);
    let trigger = button(label.clone(), ButtonVariant::GhostMuted)
        .label_with(move |font, ink| row().gap(6.0).center_y().child(text(label.clone()).font(font).color(ink).truncate()).child(icon(&ic::CHEVRON_DOWN, 14.0).color(ink)))
        .expanded(open == Some(Menu::Accounts))
        .on_press_tx(move |s: &mut H, tx: &mut Tx| step_accounts(s, Event::Toggle { provider: menu_id.clone(), menu: Menu::Accounts }, tx))
        .build(kit(p))
        .key(format!("settings-provider-accounts-{provider}"));
    let dots = el()
        .key(format!("settings-provider-options-{provider}"))
        .role("button")
        .label(options)
        .size(32.0, 32.0)
        .radius(6.0)
        .center()
        .fill(if open == Some(Menu::Options) { p.pressed } else { Color::TRANSPARENT })
        .hover(p.hover)
        .press(p.pressed)
        .on_press_tx(move |s: &mut H, tx: &mut Tx| step_accounts(s, Event::Toggle { provider: options_id.clone(), menu: Menu::Options }, tx))
        .child(icon(&ic::OUTLINE_DOTS, 16.0).color(p.icon_muted));
    let mut e = row().key("settings-provider-account-actions").gap(0.0).center_y().child(trigger).child(dots);
    if let Some(which) = open {
        e = e.child(menu::<H>(p, id, name, &accounts, which));
    }
    e
}

fn menu<H: SettingsHost + 'static>(p: &Pal, id: &str, name: &str, accounts: &[&Connection], which: Menu) -> El {
    let item = |key: String, label: String| {
        row()
            .key(key)
            .role("menuitem")
            .label(label.clone())
            .w_full()
            .h(28.0)
            .px(8.0)
            .gap(8.0)
            .center_y()
            .radius(4.0)
            .hover(p.hover)
            .child(text(label).font(super::NAME_REGULAR).color(p.text).truncate())
    };
    let mut list = col().w_full();
    match which {
        Menu::Accounts => {
            list = list.child(el().px(8.0).py(4.0).child(text(t!("settings.providers.account.group")).font(super::NAME_REGULAR).color(p.muted)));
            for (at, a) in accounts.iter().enumerate() {
                let (credential, provider, label) = (a.id.clone(), name.to_owned(), a.label.clone());
                list = list.child(
                    item(format!("settings-account-{}", a.id), a.label.clone())
                        .child(spacer())
                        .children((at == 0).then(|| icon(&ic::CHECK_SMALL, 14.0).color(p.icon)))
                        .on_press_tx(move |s: &mut H, tx: &mut Tx| activate_account(s, &credential, &provider, &label, tx)),
                );
            }
            let open = id.to_owned();
            list = list.child(el().w_full().h(0.5).my(4.0).fill(p.border)).child(
                item("settings-account-add".into(), t!("settings.providers.account.add").into())
                    .on_press_tx(move |s: &mut H, tx: &mut Tx| add_account(s, &open, tx)),
            );
        }
        Menu::Options => {
            let active = accounts.first().map(|a| (a.id.clone(), a.label.clone())).unwrap_or_default();
            let (rename_id, label) = (active.0.clone(), active.1.clone());
            let (remove_id, provider, last) = (active.0.clone(), name.to_owned(), accounts.len() == 1);
            list = list
                .child(item("settings-account-rename".into(), t!("common.rename").into()).on_press_tx(move |s: &mut H, tx: &mut Tx| {
                    step_accounts(s, Event::Rename { credential: rename_id.clone(), label: label.clone() }, tx);
                }))
                .child(item("settings-account-remove".into(), t!("dialog.provider.accounts.remove").into()).on_press_tx(move |s: &mut H, tx: &mut Tx| {
                    step_accounts(
                        s,
                        Event::Ask(Removal::Account { credential: remove_id.clone(), provider: provider.clone(), label: active.1.clone(), last }),
                        tx,
                    );
                }));
        }
    }
    widgets::overlay(p, col())
        .key("settings-provider-menu")
        .pin(None, Some(34.0), Some(0.0), None)
        .w(220.0)
        .padding(4.0)
        .radius(8.0)
        .fill(p.bg_base)
        .raise()
        .child(list)
}

fn require_writable(s: &mut dyn SettingsHost) -> bool {
    if !s.can_write() {
        s.toast_fail(t!("common.requestFailed").into(), t!("rune.jobs.readOnly").into());
        return false;
    }
    true
}

fn disconnect_provider(s: &mut dyn SettingsHost, id: &str, name: &str) {
    if !require_writable(s) {
        return;
    }
    s.spawn_job(SettingsJob::ActiveServer(SettingsRequest::DisconnectProvider {
        id: id.to_owned(),
        name: name.to_owned(),
    }));
}

fn activate_account(s: &mut dyn SettingsHost, credential: &str, provider: &str, label: &str, tx: &mut Tx) {
    tx.instant();
    let _ = s.settings_pages_mut().accounts.step(Event::Close);
    if !require_writable(s) {
        return;
    }
    s.spawn_job(SettingsJob::ActiveServer(SettingsRequest::ActivateAccount {
        credential: credential.to_owned(),
        provider: provider.to_owned(),
        label: label.to_owned(),
    }));
}

fn remove_account(s: &mut dyn SettingsHost, credential: &str, provider: &str, tx: &mut Tx) {
    tx.instant();
    let _ = s.settings_pages_mut().accounts.step(Event::Close);
    if !require_writable(s) {
        return;
    }
    let label = s
        .integrations(None)
        .unwrap_or_default()
        .iter()
        .flat_map(|i| i.connections.iter())
        .find(|c| c.id == credential)
        .map(|c| c.label.clone())
        .unwrap_or_default();
    s.spawn_job(SettingsJob::ActiveServer(SettingsRequest::RemoveAccount {
        credential: credential.to_owned(),
        provider: provider.to_owned(),
        label,
    }));
}

fn rename_account(s: &mut dyn SettingsHost, credential: String, label: String) {
    if !require_writable(s) {
        return;
    }
    s.spawn_job(SettingsJob::ActiveServer(SettingsRequest::RenameAccount { credential, label }));
}
