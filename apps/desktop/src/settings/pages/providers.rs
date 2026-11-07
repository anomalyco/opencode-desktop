//! Settings › Providers (`settings/providers/providers.tsx`): the providers the server runs, each
//! with how it connects (API key, account, environment, config) and its accounts or Disconnect,
//! then the popular providers to connect and Show more providers, which opens the connect list.

use opencode_kit::integration::Integration;
use rune::{Color, El, Tx, col, el, icon, row, spacer, text};
use rune_kit::ButtonVariant;
use rune_theme_opencode::icons as ic;

use super::{BODY, NAME, catalog, header, page, section};
use crate::App;
use crate::connect::catalog::ProviderInfo;
use crate::palette::{Pal, kit};
use crate::widgets::{self, badge, button};

/// Desktop's popular providers, in its order (`catalog/order.ts`).
const POPULAR: [&str; 8] = ["opencode-go", "opencode", "anthropic", "github-copilot", "openai", "google", "openrouter", "vercel"];

/// Which of a provider's account menus is open.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Menu {
    Accounts,
    Options,
}

fn note(id: &str) -> Option<&'static str> {
    Some(match id {
        "opencode" => "Curated models including Claude, GPT, Gemini and more",
        "opencode-go" => "Low cost subscription for everyone",
        "anthropic" => "Direct access to Claude models via API key",
        "claude-agent" => "Use your Claude plan or another Claude Code login",
        "openai" => "GPT models for fast, capable general AI tasks",
        "google" => "Gemini models for fast, structured responses",
        "openrouter" => "Access all supported models from one provider",
        "vercel" => "Unified access to AI models with smart routing",
        _ if id.starts_with("github-copilot") => "AI models for coding assistance via GitHub Copilot",
        _ => return None,
    })
}

/// How a provider connects, from the integration list as the TUI reads it: a credential is an
/// API key or an account, env connections are environment variables, and a connectionless
/// integration is config.
fn source(i: Option<&Integration>) -> &'static str {
    let Some(i) = i else { return "Other" };
    match i.connections.iter().find(|c| c.credential) {
        Some(c) if c.method == "key" => "API key",
        Some(_) => "Account",
        None if i.connections.iter().any(|c| !c.credential) => "Environment",
        None => "Config",
    }
}

pub fn view(s: &App, p: &Pal) -> El {
    let integrations = s.integrations(None).unwrap_or_default();
    let integration = |pi: &ProviderInfo| integrations.iter().find(|i| i.id == pi.integration.as_deref().unwrap_or(&pi.id));
    let console = integrations.iter().find(|i| i.id == "opencode");
    let console_connected = console.is_some_and(|i| !i.connections.is_empty());
    // The keyless OpenCode catalog isn't a connection: it shows once an account or paid model does.
    let paid = |id: &str| s.pickers.models.iter().any(|m| m.provider == id && !m.free);
    let mut connected: Vec<&ProviderInfo> = s.provider_infos(None).unwrap_or_default().iter()
        .filter(|pi| pi.id != "opencode" || console_connected || paid(&pi.id))
        .collect();
    connected.sort_by_key(|pi| pi.id != "opencode-go");
    let rows: Vec<El> = connected.iter().map(|pi| connected_row(s, p, pi, integration(pi))).collect();
    let connected_list = if rows.is_empty() {
        catalog(p, "connected-providers", vec![el().py(16.0).child(text("No connected providers").font(super::NAME_REGULAR).color(p.muted))])
    } else {
        catalog(p, "connected-providers", rows)
    };
    // The Console's row stays until its active credential is an account sign-in.
    let ids: Vec<&str> = connected.iter().map(|pi| pi.id.as_str()).collect();
    let mut popular: Vec<(String, String)> = integrations.iter()
        .filter(|i| POPULAR.contains(&i.id.as_str()))
        .filter(|i| if i.id == "opencode" { i.connections.iter().find(|c| c.credential).is_none_or(|c| c.method != "oauth") } else { !ids.contains(&i.id.as_str()) })
        .map(|i| (i.id.clone(), i.name.clone()))
        .collect();
    popular.sort_by_key(|(id, _)| POPULAR.iter().position(|p| p == id));
    let more = text("Show more providers").key("settings-providers-view-all").role("button").font(NAME).color(p.accent).mt(20.0)
        .on_press_tx(|s: &mut App, tx: &mut Tx| s.open_connect(None, true, tx));
    let body = col().key("settings-providers").w_full().gap(32.0).mt(8.0)
        .child(section(p, "connected-providers-section", Some("Connected providers"), connected_list))
        .child(col().w_full()
            .child(section(p, "popular-providers-section", Some("Popular providers"), catalog(p, "popular-providers", popular.iter().map(|(id, name)| popular_row(p, id, name)).collect())))
            .child(more));
    page(p, "settings-providers-page", header(p, "Providers", "Connect and manage model providers"), body)
}

/// A connected provider: its mark, name and how it connects; then its accounts, Disconnect, or
/// (for environment and config) a hint under the pointer.
fn connected_row(s: &App, p: &Pal, pi: &ProviderInfo, integration: Option<&Integration>) -> El {
    let lead = row().grow(1.0).shrink().min_w(0.0).gap(8.0).center_y()
        .child(widgets::provider_mark(&pi.id, if p.dark { Color::WHITE } else { Color::BLACK }))
        .child(row().gap(8.0).center_y().child(text(pi.name.clone()).font(NAME).color(p.text).truncate()).child(badge(p, source(integration))));
    let accounts = integration.map(Integration::accounts).unwrap_or_default();
    let can_disconnect = integration.is_some_and(|i| i.connections.iter().any(|c| c.credential));
    let id = pi.integration.clone().unwrap_or_else(|| pi.id.clone());
    let trailing = if !accounts.is_empty() {
        let mut e = row().gap(0.0).center_y();
        // The active account needs a fresh sign-in: the warning signs in again.
        if accounts[0].status.is_some() {
            let (again, warning) = (id.clone(), p.warning);
            e = e.child(button("Needs sign-in", ButtonVariant::GhostMuted)
                .label_with(move |font, _| text("Needs sign-in").font(font).color(warning))
                .on_press_tx(move |s: &mut App, tx: &mut Tx| s.open_connect(Some(&again), true, tx)).build(kit(p)).key("settings-provider-sign-in"));
        }
        e.child(account_menu(s, p, &id, &pi.name, integration))
    } else if can_disconnect {
        let (id, name) = (id.clone(), pi.name.clone());
        button("Disconnect", ButtonVariant::GhostMuted).on_press_tx(move |s: &mut App, tx: &mut Tx| s.disconnect_provider(&id, &name, tx)).build(kit(p))
    } else {
        text("Connected from your environment variables").key("settings-provider-env-hint").font(super::NAME_REGULAR).color(p.muted).pr(12.0).group_opacity(0.0, 1.0)
    };
    row().key(format!("settings-provider-{}", pi.id)).w_full().min_h(48.0).py(10.0).gap(16.0).center_y().hover_group()
        .child(lead).child(trailing)
}

/// A popular provider: its mark, name (Recommended for OpenCode's), what it offers, and Connect.
fn popular_row(p: &Pal, id: &str, name: &str) -> El {
    let mut main = row().gap(8.0).center_y().child(text(name.to_owned()).font(NAME).color(p.text));
    if id == "opencode" || id == "opencode-go" {
        main = main.child(badge(p, "Recommended"));
    }
    let copy = col().grow(1.0).shrink().min_w(0.0).gap(6.0).child(main).children(note(id).map(|n| text(n).font(BODY).color(p.muted).wrap()));
    let open = id.to_owned();
    row().key(format!("settings-popular-{id}")).w_full().py(16.0).gap(16.0).center_y()
        .child(row().grow(1.0).shrink().min_w(0.0).gap(8.0)
            .child(widgets::provider_mark(id, if id == "opencode" { if p.dark { Color::WHITE } else { Color::BLACK } } else { p.icon }))
            .child(copy))
        .child(button("Connect", ButtonVariant::Neutral).icon(&ic::PLUS)
            .on_press_tx(move |s: &mut App, tx: &mut Tx| s.open_connect(Some(&open), true, tx)).build(kit(p)).key(format!("settings-connect-{id}")))
}

/// The active account's menu (switch account, Add account) and its options (Rename, Remove).
fn account_menu(s: &App, p: &Pal, id: &str, name: &str, integration: Option<&Integration>) -> El {
    let accounts = integration.map(Integration::accounts).unwrap_or_default();
    let active = accounts.first().map(|a| (a.id.clone(), a.label.clone()));
    let open = s.settings_pages.menu.as_ref().filter(|(p, _)| p == id).map(|(_, m)| *m);
    if let (Some(renaming), Some((active_id, _))) = (&s.settings_pages.renaming, &active)
        && renaming == active_id
    {
        let credential = active_id.clone();
        return rune_kit::TextInput::new(&s.settings_pages.rename).label("Account name").width(Some(240.0))
            .on_edit(|s: &mut App| &mut s.settings_pages.rename)
            .on_submit(move |s: &mut App, tx: &mut Tx| s.rename_account(&credential, tx))
            // Escape drops the edit and keeps the name (`rename.ts`'s `cancelled`).
            .on_key(|s: &mut App, ev: &rune::KeyEvent, tx: &mut Tx| {
                if ev.key != rune::Key::Escape || ev.mods.cmd || ev.mods.ctrl || ev.mods.alt || ev.mods.shift {
                    return false;
                }
                tx.instant();
                s.settings_pages.renaming = None;
                true
            })
            .build(kit(p));
    }
    let (menu_id, options_id) = (id.to_owned(), id.to_owned());
    let label = active.as_ref().map(|a| a.1.clone()).unwrap_or_default();
    let trigger = button(label.clone(), ButtonVariant::GhostMuted)
        .label_with(move |font, ink| row().gap(6.0).center_y().child(text(label.clone()).font(font).color(ink).truncate()).child(icon(&ic::CHEVRON_DOWN, 14.0).color(ink)))
        .expanded(open == Some(Menu::Accounts))
        .on_press_tx(move |s: &mut App, tx: &mut Tx| s.toggle_provider_menu(&menu_id, Menu::Accounts, tx))
        .build(kit(p));
    let dots = el().key("settings-provider-options").role("button").label("Account options").size(32.0, 32.0).radius(6.0).center()
        .fill(if open == Some(Menu::Options) { p.pressed } else { Color::TRANSPARENT }).hover(p.hover).press(p.pressed)
        .on_press_tx(move |s: &mut App, tx: &mut Tx| s.toggle_provider_menu(&options_id, Menu::Options, tx))
        .child(icon(&ic::OUTLINE_DOTS, 16.0).color(p.icon_muted));
    let mut e = row().key("settings-provider-account-actions").gap(0.0).center_y().child(trigger).child(dots);
    if let Some(which) = open {
        e = e.child(menu(p, id, name, &accounts, which));
    }
    e
}

fn menu(p: &Pal, id: &str, name: &str, accounts: &[&opencode_kit::integration::Connection], which: Menu) -> El {
    let item = |key: String, label: String| row().key(key).role("menuitem").label(label.clone()).w_full().h(28.0).px(8.0).gap(8.0).center_y().radius(4.0)
        .hover(p.hover).child(text(label).font(super::NAME_REGULAR).color(p.text).truncate());
    let mut list = col().w_full();
    match which {
        Menu::Accounts => {
            list = list.child(el().px(8.0).py(4.0).child(text("Accounts").font(super::NAME_REGULAR).color(p.muted)));
            for (at, a) in accounts.iter().enumerate() {
                let (credential, provider, label) = (a.id.clone(), name.to_owned(), a.label.clone());
                list = list.child(item(format!("settings-account-{}", a.id), a.label.clone()).child(spacer()).children((at == 0).then(|| icon(&ic::CHECK_SMALL, 14.0).color(p.icon)))
                    .on_press_tx(move |s: &mut App, tx: &mut Tx| s.activate_account(&credential, &provider, &label, tx)));
            }
            let open = id.to_owned();
            list = list.child(el().w_full().h(0.5).my(4.0).fill(p.border))
                .child(item("settings-account-add".into(), "Add account".into())
                    .on_press_tx(move |s: &mut App, tx: &mut Tx| s.open_connect(Some(&open), true, tx)));
        }
        Menu::Options => {
            let active = accounts.first().map(|a| (a.id.clone(), a.label.clone())).unwrap_or_default();
            let (rename, label) = (active.clone(), active.1.clone());
            let (remove, provider, last) = (active.0.clone(), name.to_owned(), accounts.len() == 1);
            list = list
                .child(item("settings-account-rename".into(), "Rename".into()).on_press_tx(move |s: &mut App, tx: &mut Tx| {
                    tx.instant();
                    s.settings_pages.menu = None;
                    s.settings_pages.renaming = Some(rename.0.clone());
                    s.settings_pages.rename = rune::TextEdit::new(&label);
                    s.settings_pages.rename.focused = true;
                }))
                .child(item("settings-account-remove".into(), "Remove".into())
                    .on_press_tx(move |s: &mut App, tx: &mut Tx| s.remove_account(&remove, &provider, last, tx)));
        }
    }
    widgets::overlay(p, col()).key("settings-provider-menu").pin(None, Some(34.0), Some(0.0), None).w(220.0).padding(4.0).radius(8.0).fill(p.bg_base).raise().child(list)
}

impl App {
    fn toggle_provider_menu(&mut self, id: &str, which: Menu, tx: &mut Tx) {
        tx.instant();
        let same = self.settings_pages.menu.as_ref().is_some_and(|(p, m)| p == id && *m == which);
        self.settings_pages.menu = (!same).then(|| (id.to_owned(), which));
    }

    /// Removes every credential the integration holds (`disconnect.run`).
    fn disconnect_provider(&mut self, id: &str, name: &str, _tx: &mut Tx) {
        if !self.can_write() {
            return crate::status_toast::fail(self, "Request failed", crate::jobs::READ_ONLY);
        }
        let (id, name) = (id.to_owned(), name.to_owned());
        self.job(move |srv| {
            let result = srv.integration(&id, None).and_then(|i| {
                let creds: Vec<String> = i.connections.iter().filter(|c| c.credential).map(|c| c.id.clone()).collect();
                if creds.is_empty() {
                    return Err(format!("No removable credentials found for {name}"));
                }
                creds.iter().try_for_each(|c| srv.credential_remove(c))
            });
            Box::new(move |s: &mut App, _: &mut Tx| {
                match result {
                    Ok(()) => crate::status_toast::succeed(s, format!("{name} disconnected"), format!("{name} models are no longer available.")),
                    Err(e) => crate::status_toast::fail(s, "Request failed", e),
                }
                s.sync_catalogs(None, |_, _, _| {});
            }) as crate::jobs::Apply
        });
    }

    fn activate_account(&mut self, credential: &str, provider: &str, label: &str, tx: &mut Tx) {
        tx.instant();
        self.settings_pages.menu = None;
        let (credential, provider, label) = (credential.to_owned(), provider.to_owned(), label.to_owned());
        self.account_write(move |srv| srv.credential_activate(&credential), format!("{provider} account switched"), format!("Now using {label}."));
    }

    fn remove_account(&mut self, credential: &str, provider: &str, _last: bool, tx: &mut Tx) {
        tx.instant();
        self.settings_pages.menu = None;
        let (credential, provider) = (credential.to_owned(), provider.to_owned());
        let label = self.integrations(None).unwrap_or_default().iter().flat_map(|i| i.connections.iter()).find(|c| c.id == credential).map(|c| c.label.clone()).unwrap_or_default();
        self.account_write(move |srv| srv.credential_remove(&credential), format!("{label} removed"), format!("{provider} will no longer use this account."));
    }

    fn rename_account(&mut self, credential: &str, tx: &mut Tx) {
        tx.instant();
        let label = self.settings_pages.rename.text.trim().to_owned();
        self.settings_pages.renaming = None;
        if label.is_empty() {
            return;
        }
        let credential = credential.to_owned();
        self.account_write(move |srv| srv.credential_rename(&credential, &label), String::new(), String::new());
    }

    fn account_write(&mut self, work: impl FnOnce(&opencode_kit::client::Server) -> Result<(), String> + Send + 'static, title: String, description: String) {
        if !self.can_write() {
            return crate::status_toast::fail(self, "Request failed", crate::jobs::READ_ONLY);
        }
        self.job(move |srv| {
            let r = work(srv);
            Box::new(move |s: &mut App, _: &mut Tx| {
                match r {
                    Ok(()) if !title.is_empty() => crate::status_toast::succeed(s, title, description),
                    Ok(()) => {}
                    Err(e) => crate::status_toast::fail(s, "Request failed", e),
                }
                s.sync_catalogs(None, |_, _, _| {});
            }) as crate::jobs::Apply
        });
    }
}
