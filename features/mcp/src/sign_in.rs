//! An MCP server's sign-in that also takes the redirect URL pasted from the browser
//! (`DialogMcpSignIn`, `mcp-sign-in/machine.ts`), for when the browser can't reach the machine
//! running the server. It polls the attempt until the server says how it went, sends a pasted
//! URL, reloads the server list once signed in, and ends an attempt still open when it closes.

use std::time::Duration;

use opencode_i18n::t;
use opencode_kit::integration::{Authorization, McpServer};
use rune::machine::{Attempt, Seq};
use rune::{Cx, El, Key, KeyEvent, Spring, TextEdit, TextStyle, Transition, Tx, col, el, icon, row, text};
use rune_kit::{ButtonVariant, TextInput};
use rune_theme_opencode::icons as ic;
use opencode_ui::dialog::DialogSize;
use opencode_ui::palette::{Pal, kit};

use crate::ui::{self, BODY, DIALOG_CLOSE, LABEL, TITLE, button, status_row};
use crate::{McpHost, McpJob, McpReply, McpRequest, SIGN_IN_OVERLAY_ID};

/// Between status polls while the attempt is pending or the server unreachable.
const POLL: Duration = Duration::from_secs(1);
/// How long the list reload after a sign-in may run before it offers a retry.
const SYNC_TIMEOUT: Duration = Duration::from_secs(30);

pub struct SignIn {
    pub dir: String,
    name: String,
    integration: String,
    attempt: String,
    url: String,
    state: State,
    pub(crate) field: TextEdit,
    /// Requests take numbers from here, so an answer whose number the state no longer holds is stale.
    issued: Seq,
}

impl SignIn {
    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn attempt(&self) -> &str {
        &self.attempt
    }

    /// Constructs a sign-in dialog already in the failed-auth state (for scene previews).
    pub fn failed_auth_preview(dir: String, name: String, integration: String, attempt: String, url: String, error: String) -> Self {
        Self {
            dir,
            name,
            integration,
            attempt,
            url,
            state: State::Failed(Failure::Auth(error)),
            field: TextEdit::default(),
            issued: Seq::default(),
        }
    }
}

/// `McpSignInState`, less `disposed`: closing the dialog drops it.
enum State {
    /// `accepted`: the server took a pasted URL and finishes the sign-in; the poll reports it.
    Waiting { poll: Attempt, accepted: bool, entry: Option<Entry>, unreachable: Option<String> },
    Submitting { poll: Attempt, submit: Attempt, unreachable: Option<String> },
    /// Signed in; the server list reloads before the dialog closes.
    Completing { sync: Attempt },
    Failed(Failure),
    Done,
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

/// `McpSignInPollOutcome`.
#[derive(Clone, Debug)]
pub enum PollOutcome {
    Pending,
    Complete,
    Failed(String),
    Expired,
    /// The status request threw; the attempt may still be open on the server until it expires.
    Unreachable(String),
}

/// `McpSignInEvent`, less `browser.open`, which the Open browser button does itself.
enum Event {
    Polled { poll: Attempt, outcome: PollOutcome },
    PasteChanged,
    PasteSubmitted,
    SubmitSucceeded { submit: Attempt },
    SubmitFailed { submit: Attempt, message: String },
    /// The attempt's lifetime ended before the server reported an outcome.
    WaitingTimedOut,
    SyncSucceeded { sync: Attempt },
    SyncFailed { sync: Attempt, message: String },
    SyncTimedOut { sync: Attempt },
    Retry,
    /// Esc, the close cross or a press outside.
    Disposed,
}

/// `McpSignInEffect`. The deadline is the attempt's lifetime, and a poll, submit or reload
/// answers with its number, so a stale one is dropped and `*.abort`, `*.clear` have nothing to do.
enum Effect {
    Poll { poll: Attempt, delay: Duration },
    Submit { submit: Attempt, code: String },
    /// Reloads the server list, reporting back within [`SYNC_TIMEOUT`].
    Sync { sync: Attempt },
    /// Reloads the list without reporting back, for a sign-in the server finishes after the
    /// dialog closed.
    Refresh,
    /// Ends the attempt on the server, so a late browser callback can't connect the server.
    Cancel,
    Close,
}

/// `transitionMcpSignIn`.
fn step(x: &mut SignIn, event: Event) -> Vec<Effect> {
    // Like the TUI, dismissing the dialog ends an attempt still waiting; an accepted paste or a
    // finished sign-in already saved its credential, which still has to reach the list.
    if let Event::Disposed = event {
        return match x.state {
            State::Waiting { accepted: true, .. } => vec![Effect::Refresh],
            State::Waiting { .. } | State::Submitting { .. } => vec![Effect::Cancel],
            _ => vec![],
        };
    }
    match (&mut x.state, event) {
        (State::Completing { sync }, Event::SyncSucceeded { sync: due }) if *sync == due => {
            x.state = State::Done;
            vec![Effect::Close]
        }
        (State::Completing { sync }, Event::SyncFailed { sync: due, message }) if *sync == due => {
            x.state = State::Failed(Failure::Sync(message));
            vec![]
        }
        (State::Completing { sync }, Event::SyncTimedOut { sync: due }) if *sync == due => {
            x.state = State::Failed(Failure::SyncTimedOut);
            vec![]
        }
        (State::Failed(Failure::Sync(_) | Failure::SyncTimedOut), Event::Retry) => sync_effect(x),
        (State::Waiting { poll, .. } | State::Submitting { poll, .. }, Event::Polled { poll: due, outcome }) if *poll == due => polled(x, outcome),
        (State::Waiting { unreachable, accepted, .. }, Event::WaitingTimedOut) => {
            // An accepted paste is the server's to finish; cancelling it could drop a sign-in
            // that is about to succeed.
            let cancel = !*accepted;
            x.state = State::Failed(unreachable.take().map_or(Failure::Expired, Failure::Auth));
            if cancel { vec![Effect::Cancel] } else { vec![] }
        }
        (State::Submitting { unreachable, .. }, Event::WaitingTimedOut) => {
            x.state = State::Failed(unreachable.take().map_or(Failure::Expired, Failure::Auth));
            vec![Effect::Cancel]
        }
        (State::Submitting { submit, unreachable, .. }, Event::SubmitSucceeded { submit: due }) if *submit == due => {
            // Polls at once, so the dialog moves on as soon as the server finishes.
            let (unreachable, poll) = (unreachable.take(), x.issued.next());
            x.state = State::Waiting { poll, accepted: true, entry: None, unreachable };
            vec![Effect::Poll { poll, delay: Duration::ZERO }]
        }
        (State::Submitting { poll, submit, unreachable }, Event::SubmitFailed { submit: due, message }) if *submit == due => {
            x.state = State::Waiting { poll: *poll, accepted: false, entry: Some(Entry::Rejected(message)), unreachable: unreachable.take() };
            vec![]
        }
        (State::Waiting { accepted: false, entry, .. }, Event::PasteChanged) => {
            *entry = None;
            vec![]
        }
        (State::Waiting { accepted: false, poll, entry, unreachable }, Event::PasteSubmitted) => {
            let code = x.field.text.trim().to_owned();
            if code.is_empty() {
                *entry = Some(Entry::Required);
                return vec![];
            }
            let (poll, unreachable, submit) = (*poll, unreachable.take(), x.issued.next());
            x.state = State::Submitting { poll, submit, unreachable };
            vec![Effect::Submit { submit, code }]
        }
        _ => vec![],
    }
}

/// A status poll's answer. A failed request says nothing of the attempt, which may still be
/// pending, so polling goes on until it expires.
fn polled(x: &mut SignIn, outcome: PollOutcome) -> Vec<Effect> {
    let failure = match outcome {
        PollOutcome::Complete => return sync_effect(x),
        PollOutcome::Expired => Failure::Expired,
        PollOutcome::Failed(m) => Failure::Auth(m),
        PollOutcome::Pending | PollOutcome::Unreachable(_) => {
            let next = x.issued.next();
            if let State::Waiting { poll, unreachable, .. } | State::Submitting { poll, unreachable, .. } = &mut x.state {
                *poll = next;
                *unreachable = match outcome {
                    PollOutcome::Unreachable(m) => Some(m),
                    _ => None,
                };
            }
            return vec![Effect::Poll { poll: next, delay: POLL }];
        }
    };
    x.state = State::Failed(failure);
    vec![]
}

/// The sign-in succeeded: the server list reloads before the dialog closes.
fn sync_effect(x: &mut SignIn) -> Vec<Effect> {
    let sync = x.issued.next();
    x.state = State::Completing { sync };
    vec![Effect::Sync { sync }]
}

/// Opens the sign-in dialog for server `name` of `dir` that the browser just started
/// (`startMcpSignIn`).
pub fn open(host: &mut dyn McpHost, dir: String, name: String, integration: String, auth: Authorization, tx: &mut Tx) {
    let lifetime = Duration::from_millis((auth.expires - auth.created).max(0) as u64);
    let mut field = TextEdit::new("");
    field.focused = true;
    let mut issued = Seq::default();
    let poll = issued.next();
    let attempt = auth.attempt.clone();
    host.mcp_mut().sign_in = Some(SignIn {
        dir,
        name,
        integration,
        attempt: auth.attempt,
        url: auth.url,
        state: State::Waiting { poll, accepted: false, entry: None, unreachable: None },
        field,
        issued,
    });
    let return_focus = tx.focused().map(str::to_owned);
    host.on_overlay_opened(SIGN_IN_OVERLAY_ID, return_focus);
    host.spawn_job(McpJob::Timer {
        delay: lifetime,
        reply: McpReply::SignInWaitingTimedOut { attempt },
    });
    run(host, vec![Effect::Poll { poll, delay: Duration::ZERO }], tx);
}

/// Feeds `event` to the open sign-in if it matches `attempt`, and runs what it asks for.
fn sign_in_step(host: &mut dyn McpHost, attempt: &str, event: Event, tx: &mut Tx) {
    let effects = {
        let Some(x) = host.mcp_mut().sign_in.as_mut().filter(|x| x.attempt == attempt) else { return };
        tx.instant();
        step(x, event)
    };
    run(host, effects, tx);
}

fn run(host: &mut dyn McpHost, effects: Vec<Effect>, tx: &mut Tx) {
    let Some((integration, attempt, dir)) = host.mcp().sign_in.as_ref().map(|x| (x.integration.clone(), x.attempt.clone(), x.dir.clone())) else {
        return;
    };
    for effect in effects {
        match effect {
            Effect::Poll { poll, delay } => host.spawn_job(McpJob::Server {
                delay,
                request: McpRequest::OauthStatus {
                    dir: dir.clone(),
                    integration: integration.clone(),
                    attempt: attempt.clone(),
                    poll,
                },
            }),
            Effect::Submit { submit, code } => host.spawn_job(McpJob::Server {
                delay: Duration::ZERO,
                request: McpRequest::OauthComplete {
                    dir: dir.clone(),
                    integration: integration.clone(),
                    attempt: attempt.clone(),
                    submit,
                    code,
                },
            }),
            Effect::Sync { sync } => {
                host.spawn_job(McpJob::Timer {
                    delay: SYNC_TIMEOUT,
                    reply: McpReply::SignInSyncTimedOut { attempt: attempt.clone(), sync },
                });
                host.spawn_job(McpJob::Server {
                    delay: Duration::ZERO,
                    request: McpRequest::SyncAfterSignIn {
                        dir: dir.clone(),
                        attempt: attempt.clone(),
                        sync,
                    },
                });
            }
            Effect::Refresh => crate::sync(host, &dir),
            Effect::Cancel => host.spawn_job(McpJob::Server {
                delay: Duration::ZERO,
                request: McpRequest::OauthCancel {
                    dir: dir.clone(),
                    integration: integration.clone(),
                    attempt: attempt.clone(),
                },
            }),
            Effect::Close => {
                host.mcp_mut().sign_in = None;
                host.on_overlay_closed(SIGN_IN_OVERLAY_ID);
                host.auto_focus_composer(tx);
            }
        }
    }
}

/// Esc, the close cross or a press outside: an attempt still waiting ends, and an accepted
/// paste or a finished sign-in still reaches the list.
pub fn close(host: &mut dyn McpHost, tx: &mut Tx) {
    let effects = {
        let Some(x) = host.mcp_mut().sign_in.as_mut() else { return };
        tx.instant();
        step(x, Event::Disposed)
    };
    run(host, effects, tx);
    host.mcp_mut().sign_in = None;
    host.on_overlay_closed(SIGN_IN_OVERLAY_ID);
    host.auto_focus_composer(tx);
}

/// Continue: sends the pasted URL, or asks for one.
pub fn submit(host: &mut dyn McpHost, tx: &mut Tx) {
    let Some(attempt) = host.mcp().sign_in.as_ref().map(|x| x.attempt.clone()) else { return };
    sign_in_step(host, &attempt, Event::PasteSubmitted, tx);
}

/// Retries syncing the server list after a sync error or timeout.
pub fn retry(host: &mut dyn McpHost, tx: &mut Tx) {
    let Some(attempt) = host.mcp().sign_in.as_ref().map(|x| x.attempt.clone()) else { return };
    sign_in_step(host, &attempt, Event::Retry, tx);
}

/// Clears field validation errors when the pasted redirect URL changes.
pub fn paste_changed(host: &mut dyn McpHost) {
    if let Some(x) = host.mcp_mut().sign_in.as_mut() {
        let _ = step(x, Event::PasteChanged);
    }
}

/// Opens the authorization URL again in the system browser.
pub fn open_browser(host: &mut dyn McpHost, url: &str, tx: &mut Tx) {
    host.open_external(url, tx);
}

/// Esc closes the sign-in dialog.
pub fn key(host: &mut dyn McpHost, ev: &KeyEvent, tx: &mut Tx) -> bool {
    if host.mcp().sign_in.is_none() {
        return false;
    }
    if ev.key == Key::Escape {
        close(host, tx);
        return true;
    }
    false
}

pub(crate) fn apply_waiting_timed_out(host: &mut dyn McpHost, attempt: &str, tx: &mut Tx) {
    sign_in_step(host, attempt, Event::WaitingTimedOut, tx);
}

pub(crate) fn apply_polled(host: &mut dyn McpHost, attempt: &str, poll: Attempt, outcome: PollOutcome, tx: &mut Tx) {
    sign_in_step(host, attempt, Event::Polled { poll, outcome }, tx);
}

pub(crate) fn apply_submitted(host: &mut dyn McpHost, attempt: &str, submit: Attempt, result: Result<(), String>, tx: &mut Tx) {
    let event = match result {
        Ok(()) => Event::SubmitSucceeded { submit },
        Err(message) => Event::SubmitFailed { submit, message },
    };
    sign_in_step(host, attempt, event, tx);
}

pub(crate) fn apply_synced(host: &mut dyn McpHost, dir: &str, attempt: &str, sync: Attempt, result: Result<Vec<McpServer>, String>, tx: &mut Tx) {
    let event = match result {
        Ok(list) => {
            crate::apply_listed(host, dir, list, tx);
            Event::SyncSucceeded { sync }
        }
        Err(message) => Event::SyncFailed { sync, message },
    };
    sign_in_step(host, attempt, event, tx);
}

pub(crate) fn apply_sync_timed_out(host: &mut dyn McpHost, attempt: &str, sync: Attempt, tx: &mut Tx) {
    sign_in_step(host, attempt, Event::SyncTimedOut { sync }, tx);
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
pub fn layer<H: McpHost + 'static>(s: &H, p: &Pal, cx: &Cx<H>) -> Vec<El> {
    if s.mcp().sign_in.is_none() {
        return Vec::new();
    }
    let win = ui::dialog_window(s, cx);
    dialog(s, p, win)
}

/// Builds the sign-in dialog modal sized for `win` (`DialogSize::Normal`).
pub fn dialog<H: McpHost + 'static>(s: &H, p: &Pal, win: [f32; 2]) -> Vec<El> {
    let Some(x) = &s.mcp().sign_in else { return Vec::new() };
    let failure = match &x.state {
        State::Failed(f) => Some(f),
        _ => None,
    };
    let can_retry = matches!(failure, Some(Failure::Sync(_) | Failure::SyncTimedOut));
    let form = matches!(x.state, State::Waiting { .. } | State::Submitting { .. });
    let open_browser_btn = form || matches!(failure, Some(Failure::Expired | Failure::Auth(_)));
    let locked = matches!(x.state, State::Submitting { .. } | State::Waiting { accepted: true, .. });

    let close_btn = close_button_shell(p).on_press_tx(|s: &mut H, tx: &mut Tx| close(s, tx));
    let open_btn = open_browser_btn.then(|| {
        let url = x.url.clone();
        button(t!("provider.connect.oauth.openBrowser"), ButtonVariant::Neutral)
            .icon(&ic::ARROW_UP_RIGHT)
            .on_press_tx(move |s: &mut H, tx: &mut Tx| open_browser(s, &url, tx))
            .build(kit(p))
            .key("mcp-sign-in-open-browser")
    });
    let retry_btn = can_retry.then(|| {
        button(t!("common.retry"), ButtonVariant::Neutral)
            .on_press_tx(|s: &mut H, tx: &mut Tx| retry(s, tx))
            .build(kit(p))
            .key("mcp-sign-in-retry")
    });
    let paste_controls = form.then(|| {
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
                .on_edit(|s: &mut H| s.mcp_mut().sign_in_field_mut())
                .on_change(|s: &mut H, _: &mut Tx| paste_changed(s))
                .on_submit(|s: &mut H, tx: &mut Tx| submit(s, tx));
        }
        if let Some(message) = &error {
            field = field.description(message);
        }
        let field = field.build(kit(p)).key("mcp-sign-in-field");
        let submit_btn = button(t!("common.continue"), ButtonVariant::Contrast)
            .disabled(locked)
            .on_press_tx(|s: &mut H, tx: &mut Tx| submit(s, tx))
            .build(kit(p))
            .key("mcp-sign-in-continue");
        PasteControls { field, submit_btn, error }
    });

    let card = build_card(x, p, Controls { close_btn, open_btn, retry_btn, paste_controls })
        .modal(true, |s: &mut H| close(s, &mut Tx::default()));
    // `<Dialog size="normal">` (`providers/connect/mcp-sign-in.tsx`).
    ui::fixed_modal("mcp-sign-in", p, DialogSize::Normal, win, card)
}

struct PasteControls {
    field: El,
    submit_btn: El,
    error: Option<String>,
}

struct Controls {
    close_btn: El,
    open_btn: Option<El>,
    retry_btn: Option<El>,
    paste_controls: Option<PasteControls>,
}

fn close_button_shell(p: &Pal) -> El {
    el().key("mcp-sign-in-close")
        .role("button")
        .label_static(t!("common.close"))
        .size(20.0, 20.0)
        .mt(-2.0)
        .mr(-2.0)
        .radius(4.0)
        .center()
        .hover(p.hover)
        .press(p.pressed)
        .child(icon(&DIALOG_CLOSE, 16.0).color(p.icon_muted))
}

fn build_card(x: &SignIn, p: &Pal, controls: Controls) -> El {
    let failure = match &x.state {
        State::Failed(f) => Some(f),
        _ => None,
    };
    let title = t!("dialog.mcp.signIn.open", name = x.name);
    let header = row()
        .key("mcp-sign-in-header")
        .w_full()
        .padding(16.0)
        .child(text(title.clone()).font(TITLE).color(p.text).wrap_anywhere().grow(1.0).shrink().min_w(0.0))
        .child(controls.close_btn);

    let status = match failure {
        Some(f) => text(failure_text(&x.name, f)).key("mcp-sign-in-failure").role("alert").font(BODY).color(p.danger).wrap().w_full(),
        None => status_row(p, "mcp-sign-in-waiting", t!("provider.connect.status.waiting")),
    };
    let mut body = col()
        .key("mcp-sign-in-body")
        .w_full()
        .px(12.0)
        .pb(12.0)
        .gap(20.0)
        .child(text(t!("dialog.mcp.signIn.description", name = x.name)).font(BODY).color(p.muted).wrap().w_full())
        .child(status);

    if controls.open_btn.is_some() || controls.retry_btn.is_some() {
        let actions = row()
            .key("mcp-sign-in-actions")
            .gap(8.0)
            .center_y()
            .children(controls.open_btn)
            .children(controls.retry_btn);
        body = body.child(actions);
    }

    if let Some(paste_controls) = controls.paste_controls {
        let label = t!("dialog.mcp.signIn.paste.label");
        let mut paste = col()
            .key("mcp-sign-in-form")
            .w_full()
            .gap(20.0)
            .child(col().w_full().gap(8.0).child(text(label).font(LABEL).color(p.text)).child(paste_controls.field));
        if let Some(e) = paste_controls.error {
            paste = paste.child(
                text(e)
                    .key("mcp-sign-in-error")
                    .role("alert")
                    .font(TextStyle::new(12.0, 440.0).lh(16.0))
                    .color(p.danger)
                    .wrap()
                    .w_full()
                    .mt(-16.0),
            );
        }
        body = body.child(paste.child(row().child(paste_controls.submit_btn)));
    }

    ui::overlay(p, col())
        .key("mcp-sign-in")
        .role("dialog")
        .label(title)
        .radius(6.0)
        .fill(p.layer1)
        .clip_soft(0.0)
        .transition(Transition::none())
        .motion(Spring::NONE)
        // `dialog-content`: past the dialog's height its content scrolls, the bar hidden.
        .child(opencode_ui::scroller::scroller("mcp-sign-in-content").hidden(col().w_full().shrink().min_h(0.0), col().w_full().child(header).child(body)))
}
