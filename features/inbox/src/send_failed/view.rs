//! Per-message send status footer for unsent, checking, slow, and failed prompts.
//!
//! State lookup and action handlers operate on `&dyn SendFailedHost` / `&mut dyn SendFailedHost`.
//! Generic `H: SendFailedHost + 'static` bounds appear strictly on `rune::El` view wrappers and
//! delegate 1-line to non-generic functions.

use std::hash::{DefaultHasher, Hash, Hasher};

use rune::{El, Tx, row, text};
use session_view::palette::Pal;
use session_view::style::UI_SMALL;

use super::{AfterCheck, Phase, SendFailedHost, State};

struct FooterSpec {
    label: String,
    actions: FooterActions,
}

enum FooterActions {
    None,
    OutboxRetry,
    LocalFailed { can_edit: bool },
}

fn footer_spec(host: &dyn SendFailedHost, id: &str) -> Option<FooterSpec> {
    let unsent = host.unsent();
    let state = unsent.state(id)?;
    if let Some(sent) = unsent.durable(id) {
        let label = match &sent.phase {
            Phase::Sending { then: Some(AfterCheck::Edit), .. } | Phase::Unconfirmed { then: Some(AfterCheck::Edit), .. } => "Checking delivery before editing".into(),
            Phase::Sending { then: Some(AfterCheck::Delete), .. } | Phase::Unconfirmed { then: Some(AfterCheck::Delete), .. } => "Checking delivery before deleting".into(),
            // Past `SLOW_SEND` it says why the wait goes on: the server hasn't answered yet.
            Phase::Sending { then: None, .. } if unsent.slow(id) => opencode_i18n::t!("rune.send.slow").into(),
            Phase::Sending { then: None, .. } => "Sending…".into(),
            Phase::Waiting { why } if why == "waiting for connection" => "Not sent · waiting for connection".into(),
            Phase::Waiting { why } if !why.is_empty() => format!("Not sent · {why} · retrying"),
            Phase::Waiting { .. } => "Not sent · waiting to send".to_owned(),
            Phase::Unconfirmed { why, .. } if why.is_empty() => "Checking delivery…".into(),
            Phase::Unconfirmed { why, .. } => format!("Checking delivery · {why}"),
            Phase::Failed { why, .. } => format!("Not sent · {why}"),
            // The session's own status line says what it waits for.
            Phase::Accepted | Phase::Preparing => return None,
        };
        let actions = if state == State::Failed { FooterActions::OutboxRetry } else { FooterActions::None };
        return Some(FooterSpec { label, actions });
    }
    let label = match state {
        State::Unconfirmed => opencode_i18n::t!("rune.send.unconfirmed").to_owned(),
        State::Failed => opencode_i18n::t!("rune.send.failed").to_owned(),
        _ => return None,
    };
    let actions = if state == State::Failed {
        FooterActions::LocalFailed { can_edit: host.composer().text.is_empty() && !host.composer_has_attachments() }
    } else {
        FooterActions::None
    };
    Some(FooterSpec { label, actions })
}

fn on_outbox_retry(host: &mut dyn SendFailedHost, id: &str, tx: &mut Tx) {
    tx.instant();
    super::outbox_action(host, id, "outbox-retry");
}

fn on_retry_send(host: &mut dyn SendFailedHost, id: &str, tx: &mut Tx) {
    tx.instant();
    super::retry_send(host, id);
}

fn on_edit_failed_send(host: &mut dyn SendFailedHost, id: &str, tx: &mut Tx) {
    tx.instant();
    super::edit_failed_send(host, id);
}

/// Fingerprint of [`footer`] for timeline row memoization (`TimelineHost::user_status_version`).
pub fn footer_version(host: &dyn SendFailedHost, id: &str) -> u64 {
    let unsent = host.unsent();
    if let Some(s) = unsent.durable(id) {
        let mut version = DefaultHasher::new();
        // Past `SLOW_SEND` the footer says why the wait goes on: a new version.
        (s.state(), s.then(), s.why(), unsent.slow(id)).hash(&mut version);
        return version.finish() | 8;
    }
    match unsent.state(id) {
        Some(State::Unconfirmed) => 1,
        Some(State::Failed) => {
            if host.composer().text.is_empty() && !host.composer_has_attachments() {
                2
            } else {
                3
            }
        }
        _ => 0,
    }
}

/// Status line under a user prompt bubble while sending, waiting, checking delivery, or failed.
pub fn footer<H: SendFailedHost + 'static>(host: &H, p: &Pal, id: &str) -> Option<El> {
    let spec = footer_spec(host, id)?;
    let mut line = row().key("send-status").h(20.0).gap(8.0).center_y().child(text(spec.label).font(UI_SMALL).color(p.faint));
    match spec.actions {
        FooterActions::None => {}
        FooterActions::OutboxRetry => {
            let id = id.to_owned();
            line = line.child(action::<H>(p, "retry-send", opencode_i18n::t!("rune.send.retry"), move |s, tx| on_outbox_retry(s, &id, tx)));
        }
        FooterActions::LocalFailed { can_edit } => {
            let retry = id.to_owned();
            line = line.child(action::<H>(p, "retry-send", opencode_i18n::t!("rune.send.retry"), move |s, tx| on_retry_send(s, &retry, tx)));
            if can_edit {
                let edit = id.to_owned();
                line = line.child(action::<H>(p, "edit-send", opencode_i18n::t!("common.edit"), move |s, tx| on_edit_failed_send(s, &edit, tx)));
            }
        }
    }
    Some(line)
}

fn action<H: SendFailedHost + 'static>(p: &Pal, key: &str, label: &str, run: impl Fn(&mut H, &mut Tx) + 'static) -> El {
    row()
        .key(key.to_owned())
        .role("button")
        .label(label.to_owned())
        .help(label.to_owned())
        .h(20.0)
        .px(4.0)
        .center_y()
        .radius(4.0)
        .hover(p.hover)
        .press(p.pressed)
        .on_press_tx(move |s: &mut H, tx: &mut Tx| run(s, tx))
        .child(text(label.to_owned()).font(UI_SMALL).color(p.muted))
}
