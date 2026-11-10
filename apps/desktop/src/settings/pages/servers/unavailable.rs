//! `SettingsServerUnavailable` (`settings/servers/unavailable.tsx`): a per-server page while its
//! server can't be reached keeps its header and the switcher, and shows instead of its body
//! "Can't reach <server>", Retry (which probes every server and stays off until they answered or
//! 35 s passed, `ServerRetryState`) and Edit server (the server's own dialog for one added by
//! address, else Settings › Servers).


use opencode_kit::servers::settings::{self, RetryEffect, RetryEvent};
use rune::machine::Attempt;
use rune::{El, Tx, col, icon, row, text};
use rune_kit::ButtonVariant;
use rune_theme_opencode::icons as ic;

use super::inventory::{self, Kind};
use crate::App;
use crate::palette::Pal;
use opencode_i18n::t;

/// The Retry's probe and its timeout (`retry.run`'s effects), on the app's clock.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum Wait {
    #[default]
    Off,
    Armed { attempt: Attempt, waiter: u64, due: f64 },
}

const TITLE: rune::TextStyle = rune::TextStyle::new(13.0, 530.0).lh(16.0);
const DESCRIPTION: rune::TextStyle = rune::TextStyle::new(13.0, 440.0).lh(20.0);

/// The page `title` of a server that can't be reached, with its `description`.
pub fn view(s: &App, p: &Pal, key: &str, title: &str, description: &str) -> El {
    let server = inventory::selected(s);
    let retrying = s.settings_pages.server.retry.retrying();
    let retry = crate::widgets::button(t!("settings.server.unreachable.retry"), ButtonVariant::Neutral)
        .icon(&ic::REFRESH)
        .disabled(retrying)
        .on_press_tx(|s: &mut App, tx: &mut Tx| step(s, RetryEvent::Clicked, tx))
        .build(crate::palette::kit(p))
        .key("settings-server-retry");
    let http = match &server.kind {
        Kind::Http(h) => Some(h.url.clone()),
        _ => None,
    };
    let edit = crate::widgets::button(t!("settings.server.unreachable.edit"), ButtonVariant::GhostMuted)
        .on_press_tx(move |s: &mut App, tx: &mut Tx| match &http {
            Some(url) => crate::servers::dialog::open(s, Some(url), tx),
            None => s.open_settings_page(crate::settings::pages::Page::Servers, tx),
        })
        .build(crate::palette::kit(p))
        .key("settings-server-edit");
    let block = col().key("settings-server-unavailable").role("status").w_full().px(16.0).py(40.0).gap(8.0).items_center()
        .child(icon(&ic::OUTLINE_HEXAGONAL_WARNING, 16.0).color(p.icon_muted).mb(4.0))
        .child(text(opencode_i18n::t!("settings.server.unreachable.title", server = server.name.as_str()).to_owned()).font(TITLE).color(p.text).align(rune::Align::Center))
        .child(text(opencode_i18n::t!("settings.server.unreachable.description", server = server.name.as_str()).to_owned()).font(DESCRIPTION).color(p.muted).align(rune::Align::Center).wrap().max_w(360.0))
        .child(row().key("settings-server-unavailable-actions").mt(12.0).gap(8.0).justify_center().child(retry).child(edit));
    let header = super::super::header_with(p, title, description, super::switcher(s, p));
    super::super::page(p, key, header, crate::settings::list(p, "settings-server-unavailable-list", vec![block]))
}

/// Steps the Retry (`transitionServerRetry`) and runs its effects.
pub fn step(s: &mut App, event: RetryEvent, tx: &mut Tx) {
    let before = s.settings_pages.server.retry.retrying();
    for effect in settings::step(&mut s.settings_pages.server.retry, event) {
        match effect {
            RetryEffect::Run { attempt, timeout_ms } => {
                let due = s.clock + timeout_ms as f64 / 1000.0;
                // Without a server nothing answers: the timeout ends it.
                let waiter = crate::servers::health::retry(s, tx).unwrap_or(0);
                s.settings_pages.server.wait = Wait::Armed { attempt, waiter, due };
                tx.wake_at(due);
            }
            RetryEffect::TimeoutCancel => s.settings_pages.server.wait = Wait::Off,
        }
    }
    if before != s.settings_pages.server.retry.retrying() {
        tx.instant();
    }
}

/// `poll`: the Retry's probes answered (`retry.settled`) or its time ran out (`retry.timed-out`);
/// the page follows the server it shows (`server.synced`).
pub fn poll(s: &mut App, tx: &mut Tx) {
    let key = s.settings_pages.server.key.clone();
    step(s, RetryEvent::ServerSynced(key), tx);
    let Wait::Armed { attempt, waiter, due } = s.settings_pages.server.wait else { return };
    if crate::servers::health::resolved(waiter) {
        return step(s, RetryEvent::Settled { attempt }, tx);
    }
    if s.clock >= due {
        s.settings_pages.server.wait = Wait::Off;
        step(s, RetryEvent::TimedOut { attempt }, tx);
    }
}
