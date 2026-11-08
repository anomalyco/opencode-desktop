//! An MCP server's sign-in that also takes the redirect URL pasted from the browser
//! (`DialogMcpSignIn`, `mcp-sign-in/machine.ts`), for when the browser can't reach the machine
//! running the server. It polls the attempt until the server says how it went, sends a pasted
//! URL, reloads the server list once signed in, and ends an attempt still open when it closes.

use std::time::Duration;

use opencode_i18n::t;
use opencode_kit::integration::{AttemptStatus, Authorization};
use rune::{El, Key, KeyEvent, Spring, TextEdit, TextStyle, Transition, Tx, col, el, icon, row, text};
use rune_kit::{ButtonVariant, TextInput};
use rune_theme_opencode::icons as ic;

use crate::App;
use crate::jobs::Apply;
use crate::palette::{Pal, kit};
use crate::widgets::{self, BODY, LABEL, button};

/// Between status polls while the attempt is pending or the server unreachable.
const POLL: Duration = Duration::from_secs(1);
/// How long the list reload after a sign-in may run before it offers a retry.
const SYNC_TIMEOUT: Duration = Duration::from_secs(30);
/// `dialog-title`: 15 on 20 at 530.
const TITLE: TextStyle = TextStyle::new(15.0, 530.0).lh(20.0).tracking(-0.13);

pub struct SignIn {
    pub dir: String,
    name: String,
    integration: String,
    attempt: String,
    url: String,
    state: State,
    field: TextEdit,
    /// Requests take numbers from here, so an answer whose number the state no longer holds is stale.
    issued: u32,
}

enum State {
    /// `accepted`: the server took a pasted URL and finishes the sign-in; the poll reports it.
    Waiting { poll: u32, accepted: bool, entry: Option<Entry>, unreachable: Option<String> },
    Submitting { poll: u32, submit: u32, unreachable: Option<String> },
    /// Signed in; the server list reloads before the dialog closes.
    Completing { sync: u32 },
    Failed(Failure),
}

enum Entry {
    Required,
    Rejected(String),
}

enum Failure {
    Expired,
    Auth(String),
    /// Signed in, but the list didn't reload; a retry reloads it again.
    Sync(String),
    SyncTimedOut,
}

impl App {
    /// Opens the sign-in for server `name` of `dir` that the browser just started.
    pub(super) fn open_mcp_sign_in(&mut self, dir: String, name: String, integration: String, auth: Authorization) {
        let lifetime = Duration::from_millis((auth.expires - auth.created).max(0) as u64);
        let mut field = TextEdit::new("");
        field.focused = true;
        self.mcp.sign_in = Some(SignIn { dir, name, integration, attempt: auth.attempt.clone(), url: auth.url, state: State::Waiting { poll: 1, accepted: false, entry: None, unreachable: None }, field, issued: 1 });
        let attempt = auth.attempt;
        self.job_after(lifetime, move |_| Box::new(move |s: &mut App, tx: &mut Tx| timed_out(s, &attempt, tx)) as Apply);
        poll(self, 1, Duration::ZERO);
    }
}

/// The open sign-in, if it's attempt `attempt`.
fn current<'a>(s: &'a mut App, attempt: &str) -> Option<&'a mut SignIn> {
    s.mcp.sign_in.as_mut().filter(|x| x.attempt == attempt)
}

fn poll(s: &App, n: u32, delay: Duration) {
    let Some(x) = &s.mcp.sign_in else { return };
    let (id, attempt, dir) = (x.integration.clone(), x.attempt.clone(), x.dir.clone());
    s.job_after(delay, move |srv| {
        let status = srv.oauth_status(&id, &attempt, Some(dir.as_str()).filter(|d| !d.is_empty()));
        Box::new(move |s: &mut App, tx: &mut Tx| polled(s, &attempt, n, status, tx)) as Apply
    });
}

fn polled(s: &mut App, attempt: &str, n: u32, status: Result<AttemptStatus, String>, tx: &mut Tx) {
    let Some(x) = current(s, attempt) else { return };
    if !matches!(&x.state, State::Waiting { poll, .. } | State::Submitting { poll, .. } if *poll == n) {
        return;
    }
    tx.instant();
    let again = match status {
        // A failed request says nothing of the attempt, which may still be pending until it expires.
        Ok(AttemptStatus::Pending(_)) => Some(None),
        Err(e) => Some(Some(e)),
        Ok(AttemptStatus::Complete) => return sync(s),
        Ok(AttemptStatus::Failed(m)) => {
            x.state = State::Failed(Failure::Auth(m));
            None
        }
        Ok(AttemptStatus::Expired) => {
            x.state = State::Failed(Failure::Expired);
            None
        }
    };
    if let Some(failed) = again {
        x.issued += 1;
        let next = x.issued;
        if let State::Waiting { poll, unreachable, .. } | State::Submitting { poll, unreachable, .. } = &mut x.state {
            *poll = next;
            *unreachable = failed;
        }
        poll(s, next, POLL);
    }
}



/// The attempt's lifetime ended before the server said how it went. An accepted paste is the
/// server's to finish, so only an attempt without one is ended.
fn timed_out(s: &mut App, attempt: &str, tx: &mut Tx) {
    let Some(x) = current(s, attempt) else { return };
    let (unreachable, accepted) = match &x.state {
        State::Waiting { unreachable, accepted, .. } => (unreachable.clone(), *accepted),
        State::Submitting { unreachable, .. } => (unreachable.clone(), false),
        _ => return,
    };
    tx.instant();
    x.state = State::Failed(unreachable.map_or(Failure::Expired, Failure::Auth));
    if !accepted {
        cancel(s);
    }
}

/// Continue: sends the pasted URL, or asks for one.
fn submit(s: &mut App, tx: &mut Tx) {
    let Some(x) = s.mcp.sign_in.as_mut() else { return };
    let (polling, unreachable) = match &x.state {
        State::Waiting { poll, accepted: false, unreachable, .. } => (*poll, unreachable.clone()),
        _ => return,
    };
    tx.instant();
    let code = x.field.text.trim().to_owned();
    if code.is_empty() {
        if let State::Waiting { entry, .. } = &mut x.state {
            *entry = Some(Entry::Required);
        }
        return;
    }
    x.issued += 1;
    let n = x.issued;
    x.state = State::Submitting { poll: polling, submit: n, unreachable };
    let (id, attempt, dir) = (x.integration.clone(), x.attempt.clone(), x.dir.clone());
    s.job(move |srv| {
        let sent = srv.oauth_complete(&id, &attempt, Some(dir.as_str()).filter(|d| !d.is_empty()), &code);
        Box::new(move |s: &mut App, tx: &mut Tx| submitted(s, &attempt, n, sent, tx)) as Apply
    });
}

fn submitted(s: &mut App, attempt: &str, n: u32, sent: Result<(), String>, tx: &mut Tx) {
    let Some(x) = current(s, attempt) else { return };
    let (polling, unreachable) = match &x.state {
        State::Submitting { poll, submit, unreachable } if *submit == n => (*poll, unreachable.clone()),
        _ => return,
    };
    tx.instant();
    match sent {
        // Polls at once, so the dialog moves on as soon as the server finishes.
        Ok(()) => {
            x.issued += 1;
            let next = x.issued;
            x.state = State::Waiting { poll: next, accepted: true, entry: None, unreachable };
            poll(s, next, Duration::ZERO);
        }
        Err(m) => x.state = State::Waiting { poll: polling, accepted: false, entry: Some(Entry::Rejected(m)), unreachable },
    }
}

/// Reloads the location's server list, giving up after a while.
fn sync(s: &mut App) {
    let Some(x) = s.mcp.sign_in.as_mut() else { return };
    x.issued += 1;
    let n = x.issued;
    x.state = State::Completing { sync: n };
    let (attempt, dir) = (x.attempt.clone(), x.dir.clone());
    let late = attempt.clone();
    s.job_after(SYNC_TIMEOUT, move |_| {
        Box::new(move |s: &mut App, tx: &mut Tx| synced(s, &late, n, Err(None), tx)) as Apply
    });
    s.job(move |srv| {
        let list = srv.mcp_servers(Some(dir.as_str()).filter(|d| !d.is_empty()));
        Box::new(move |s: &mut App, tx: &mut Tx| {
            let done = match list {
                Ok(list) => {
                    s.mcp.servers.insert(dir.clone(), list);
                    s.mcp_synced(&dir, tx);
                    Ok(())
                }
                Err(e) => Err(Some(e)),
            };
            synced(s, &attempt, n, done, tx);
        }) as Apply
    });
}

/// The reload `n` ended: done closes the dialog; a failure, or `None` for too slow, offers a retry.
fn synced(s: &mut App, attempt: &str, n: u32, done: Result<(), Option<String>>, tx: &mut Tx) {
    let Some(x) = current(s, attempt) else { return };
    if !matches!(x.state, State::Completing { sync } if sync == n) {
        return;
    }
    tx.instant();
    match done {
        Ok(()) => s.mcp.sign_in = None,
        Err(Some(m)) => x.state = State::Failed(Failure::Sync(m)),
        Err(None) => x.state = State::Failed(Failure::SyncTimedOut),
    }
}

/// Ends the attempt on the server, so a late browser callback can't connect the server.
fn cancel(s: &App) {
    let Some(x) = &s.mcp.sign_in else { return };
    let (id, attempt, dir) = (x.integration.clone(), x.attempt.clone(), x.dir.clone());
    s.job(move |srv| {
        let _ = srv.oauth_cancel(&id, &attempt, Some(dir.as_str()).filter(|d| !d.is_empty()));
        Box::new(|_: &mut App, _: &mut Tx| {}) as Apply
    });
}

/// Esc, the close cross or a press outside: an attempt still waiting ends, and an accepted
/// paste or a finished sign-in still reaches the list.
fn close(s: &mut App, tx: &mut Tx) {
    let Some(x) = &s.mcp.sign_in else { return };
    tx.instant();
    match x.state {
        State::Waiting { accepted: true, .. } => s.sync_mcp(&x.dir.clone()),
        State::Waiting { .. } | State::Submitting { .. } => cancel(s),
        _ => {}
    }
    s.mcp.sign_in = None;
    s.composer.focused = s.has_composer() && !s.settings.open && s.mcp.open.is_none();
}

/// Esc closes it.
pub fn key(s: &mut App, ev: &KeyEvent, tx: &mut Tx) -> bool {
    if s.mcp.sign_in.is_none() {
        return false;
    }
    if ev.key == Key::Escape {
        close(s, tx);
        return true;
    }
    false
}

fn failure_text(name: &str, f: &Failure) -> String {
    match f {
        Failure::Expired => t!("provider.connect.status.failed", error = t!("provider.connect.oauth.expired")),
        Failure::Auth(m) => t!("provider.connect.status.failed", error = m),
        Failure::Sync(m) => t!("dialog.mcp.signIn.syncFailed", name = name, error = m),
        Failure::SyncTimedOut => t!("mcp.timedOut", name = name),
    }
}

/// The dialog (`Dialog size="normal"`): what to do, how it stands, Open browser or Try again, and
/// the redirect URL's field with Continue while the server waits.
pub fn layer(s: &App, p: &Pal) -> Vec<El> {
    let Some(x) = &s.mcp.sign_in else { return vec![] };
    let failure = match &x.state {
        State::Failed(f) => Some(f),
        _ => None,
    };
    let can_retry = matches!(failure, Some(Failure::Sync(_) | Failure::SyncTimedOut));
    let form = matches!(x.state, State::Waiting { .. } | State::Submitting { .. });
    let open_browser = form || matches!(failure, Some(Failure::Expired | Failure::Auth(_)));
    let locked = matches!(x.state, State::Submitting { .. } | State::Waiting { accepted: true, .. });

    let close_button = el().key("mcp-sign-in-close").role("button").label(t!("common.close")).size(20.0, 20.0).mt(-2.0).mr(-2.0).radius(4.0).center()
        .hover(p.hover).press(p.pressed)
        .on_press_tx(close)
        .child(icon(&widgets::DIALOG_CLOSE, 16.0).color(p.icon_muted));
    let title = t!("dialog.mcp.signIn.open", name = x.name);
    let header = row().key("mcp-sign-in-header").w_full().padding(16.0)
        .child(text(title.clone()).font(TITLE).color(p.text).wrap_anywhere().grow(1.0).shrink().min_w(0.0))
        .child(close_button);

    let status = match failure {
        Some(f) => text(failure_text(&x.name, f)).key("mcp-sign-in-failure").role("alert").font(BODY).color(p.danger).wrap().w_full(),
        None => widgets::status_row(p, "mcp-sign-in-waiting", t!("provider.connect.status.waiting")),
    };
    let mut body = col().key("mcp-sign-in-body").w_full().px(12.0).pb(12.0).gap(20.0)
        .child(text(t!("dialog.mcp.signIn.description", name = x.name)).font(BODY).color(p.muted).wrap().w_full())
        .child(status);
    if open_browser || can_retry {
        let mut actions = row().key("mcp-sign-in-actions").gap(8.0).center_y();
        if open_browser {
            let url = x.url.clone();
            actions = actions.child(button(t!("provider.connect.oauth.openBrowser"), ButtonVariant::Neutral).icon(&ic::ARROW_UP_RIGHT)
                .on_press_tx(move |_: &mut App, tx: &mut Tx| {
                    let _ = tx.open_url(&url);
                })
                .build(kit(p)).key("mcp-sign-in-open-browser"));
        }
        if can_retry {
            actions = actions.child(button(t!("common.retry"), ButtonVariant::Neutral).on_press_tx(|s: &mut App, tx: &mut Tx| {
                tx.instant();
                sync(s);
            }).build(kit(p)).key("mcp-sign-in-retry"));
        }
        body = body.child(actions);
    }
    if form {
        let error = match &x.state {
            State::Waiting { entry: Some(Entry::Required), .. } => Some(t!("dialog.mcp.signIn.paste.required").to_owned()),
            State::Waiting { entry: Some(Entry::Rejected(m)), .. } => Some(m.clone()),
            _ => None,
        };
        let label = t!("dialog.mcp.signIn.paste.label");
        let mut field = TextInput::new(&x.field)
            .placeholder(t!("dialog.mcp.signIn.paste.placeholder"))
            .label(label)
            .width(None)
            .spellcheck(false)
            .invalid(error.is_some());
        if !locked {
            field = field
                .on_edit(|s: &mut App| &mut s.mcp.sign_in.as_mut().expect("signing in").field)
                .on_change(|s: &mut App, _: &mut Tx| {
                    if let Some(State::Waiting { entry, .. }) = s.mcp.sign_in.as_mut().map(|x| &mut x.state) {
                        *entry = None;
                    }
                })
                .on_submit(submit);
        }
        let mut paste = col().key("mcp-sign-in-form").w_full().gap(20.0)
            .child(col().w_full().gap(8.0).child(text(label).font(LABEL).color(p.text)).child(field.build(kit(p)).key("mcp-sign-in-field")));
        if let Some(e) = error {
            paste = paste.child(text(e).key("mcp-sign-in-error").role("alert").font(rune::TextStyle::new(12.0, 440.0).lh(16.0)).color(p.danger).wrap().w_full().mt(-16.0));
        }
        body = body.child(paste.child(row().child(button(t!("common.continue"), ButtonVariant::Contrast).disabled(locked).on_press_tx(submit).build(kit(p)).key("mcp-sign-in-continue"))));
    }

    let dialog = widgets::overlay(p, col())
        .key("mcp-sign-in")
        .role("dialog")
        .label(title)
        .w(480.0)
        .h(368.0)
        .radius(6.0)
        .fill(p.layer1)
        .clip_soft(0.0)
        .transition(Transition::none())
        .motion(Spring::NONE)
        .modal(true, |s: &mut App| close(s, &mut Tx::default()))
        .child(header)
        .child(body);
    widgets::modal("mcp-sign-in", p, dialog, None)
}
