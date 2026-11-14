//! An MCP server's sign-in that also takes the redirect URL pasted from the browser
//! (`DialogMcpSignIn`, `mcp-sign-in/machine.ts`), for when the browser can't reach the machine
//! running the server. It polls the attempt until the server says how it went, sends a pasted
//! URL, reloads the server list once signed in, and ends an attempt still open when it closes.

use std::time::Duration;

use opencode_i18n::t;
use opencode_kit::integration::{AttemptStatus, Authorization};
use rune::machine::{Attempt, Seq};
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
    issued: Seq,
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
enum Outcome {
    Pending,
    Complete,
    Failed(String),
    Expired,
    /// The status request threw; the attempt may still be open on the server until it expires.
    Unreachable(String),
}

/// `McpSignInEvent`, less `browser.open`, which the Open browser button does itself.
enum Event {
    Polled { poll: Attempt, outcome: Outcome },
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
        (State::Failed(Failure::Sync(_) | Failure::SyncTimedOut), Event::Retry) => sync(x),
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
fn polled(x: &mut SignIn, outcome: Outcome) -> Vec<Effect> {
    let failure = match outcome {
        Outcome::Complete => return sync(x),
        Outcome::Expired => Failure::Expired,
        Outcome::Failed(m) => Failure::Auth(m),
        Outcome::Pending | Outcome::Unreachable(_) => {
            let next = x.issued.next();
            if let State::Waiting { poll, unreachable, .. } | State::Submitting { poll, unreachable, .. } = &mut x.state {
                *poll = next;
                *unreachable = match outcome {
                    Outcome::Unreachable(m) => Some(m),
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
fn sync(x: &mut SignIn) -> Vec<Effect> {
    let sync = x.issued.next();
    x.state = State::Completing { sync };
    vec![Effect::Sync { sync }]
}

impl App {
    /// Opens the sign-in for server `name` of `dir` that the browser just started
    /// (`startMcpSignIn`).
    pub(super) fn open_mcp_sign_in(&mut self, dir: String, name: String, integration: String, auth: Authorization) {
        let lifetime = Duration::from_millis((auth.expires - auth.created).max(0) as u64);
        let mut field = TextEdit::new("");
        field.focused = true;
        let mut issued = Seq::default();
        let poll = issued.next();
        self.mcp.sign_in = Some(SignIn { dir, name, integration, attempt: auth.attempt.clone(), url: auth.url, state: State::Waiting { poll, accepted: false, entry: None, unreachable: None }, field, issued });
        let attempt = auth.attempt;
        self.job_after(lifetime, move |_| Box::new(move |s: &mut App, tx: &mut Tx| sign_in_step(s, &attempt, Event::WaitingTimedOut, tx)) as Apply);
        run(self, vec![Effect::Poll { poll, delay: Duration::ZERO }]);
    }
}

/// Feeds `event` to the open sign-in if it's attempt `attempt`, and runs what it asks for.
fn sign_in_step(s: &mut App, attempt: &str, event: Event, tx: &mut Tx) {
    let Some(x) = s.mcp.sign_in.as_mut().filter(|x| x.attempt == attempt) else { return };
    tx.instant();
    let effects = step(x, event);
    run(s, effects);
}

fn run(s: &mut App, effects: Vec<Effect>) {
    let Some(x) = &s.mcp.sign_in else { return };
    let (id, attempt, dir) = (x.integration.clone(), x.attempt.clone(), x.dir.clone());
    let at = || Some(dir.clone()).filter(|d| !d.is_empty());
    for effect in effects {
        let (id, attempt, at) = (id.clone(), attempt.clone(), at());
        match effect {
            Effect::Poll { poll, delay } => s.job_after(delay, move |srv| {
                let outcome = match srv.oauth_status(&id, &attempt, at.as_deref()) {
                    Ok(AttemptStatus::Pending(_)) => Outcome::Pending,
                    Ok(AttemptStatus::Complete) => Outcome::Complete,
                    Ok(AttemptStatus::Failed(m)) => Outcome::Failed(m),
                    Ok(AttemptStatus::Expired) => Outcome::Expired,
                    Err(e) => Outcome::Unreachable(e),
                };
                Box::new(move |s: &mut App, tx: &mut Tx| sign_in_step(s, &attempt, Event::Polled { poll, outcome }, tx)) as Apply
            }),
            Effect::Submit { submit, code } => s.job(move |srv| {
                let event = match srv.oauth_complete(&id, &attempt, at.as_deref(), &code) {
                    Ok(()) => Event::SubmitSucceeded { submit },
                    Err(message) => Event::SubmitFailed { submit, message },
                };
                Box::new(move |s: &mut App, tx: &mut Tx| sign_in_step(s, &attempt, event, tx)) as Apply
            }),
            Effect::Sync { sync } => {
                let late = attempt.clone();
                s.job_after(SYNC_TIMEOUT, move |_| Box::new(move |s: &mut App, tx: &mut Tx| sign_in_step(s, &late, Event::SyncTimedOut { sync }, tx)) as Apply);
                let dir = dir.clone();
                s.job(move |srv| {
                    let list = srv.mcp_servers(at.as_deref());
                    Box::new(move |s: &mut App, tx: &mut Tx| {
                        let event = match list {
                            Ok(list) => {
                                s.mcp_listed(&dir, list, tx);
                                Event::SyncSucceeded { sync }
                            }
                            Err(message) => Event::SyncFailed { sync, message },
                        };
                        sign_in_step(s, &attempt, event, tx);
                    }) as Apply
                });
            }
            Effect::Refresh => s.sync_mcp(&dir),
            Effect::Cancel => s.job(move |srv| {
                let _ = srv.oauth_cancel(&id, &attempt, at.as_deref());
                Box::new(|_: &mut App, _: &mut Tx| {}) as Apply
            }),
            Effect::Close => s.mcp.sign_in = None,
        }
    }
}

/// Esc, the close cross or a press outside: an attempt still waiting ends, and an accepted
/// paste or a finished sign-in still reaches the list.
fn close(s: &mut App, tx: &mut Tx) {
    let Some(x) = s.mcp.sign_in.as_mut() else { return };
    tx.instant();
    let effects = step(x, Event::Disposed);
    run(s, effects);
    s.mcp.sign_in = None;
    s.composer.focused = s.has_composer() && !s.settings.open && s.mcp.open.is_none();
}

/// Continue: sends the pasted URL, or asks for one.
fn submit(s: &mut App, tx: &mut Tx) {
    let Some(attempt) = s.mcp.sign_in.as_ref().map(|x| x.attempt.clone()) else { return };
    sign_in_step(s, &attempt, Event::PasteSubmitted, tx);
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
pub fn layer(s: &App, p: &Pal, cx: &rune::Cx<App>) -> Vec<El> {
    let Some(x) = &s.mcp.sign_in else { return vec![] };
    let failure = match &x.state {
        State::Failed(f) => Some(f),
        _ => None,
    };
    let can_retry = matches!(failure, Some(Failure::Sync(_) | Failure::SyncTimedOut));
    let form = matches!(x.state, State::Waiting { .. } | State::Submitting { .. });
    let open_browser = form || matches!(failure, Some(Failure::Expired | Failure::Auth(_)));
    let locked = matches!(x.state, State::Submitting { .. } | State::Waiting { accepted: true, .. });

    let close_button = el().key("mcp-sign-in-close").role("button").label_static(t!("common.close")).size(20.0, 20.0).mt(-2.0).mr(-2.0).radius(4.0).center()
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
                let Some(attempt) = s.mcp.sign_in.as_ref().map(|x| x.attempt.clone()) else { return };
                sign_in_step(s, &attempt, Event::Retry, tx);
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
                    if let Some(x) = s.mcp.sign_in.as_mut() {
                        step(x, Event::PasteChanged);
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
        .radius(6.0)
        .fill(p.layer1)
        .clip_soft(0.0)
        .transition(Transition::none())
        .motion(Spring::NONE)
        .modal(true, |s: &mut App| close(s, &mut Tx::default()))
        // `dialog-content`: past the dialog's height its content scrolls, the bar hidden.
        .child(session_view::scroller::scroller("mcp-sign-in-content").hidden(col().w_full().shrink().min_h(0.0), col().w_full().child(header).child(body)));
    // `<Dialog size="normal">` (`providers/connect/mcp-sign-in.tsx`).
    widgets::sized_modal("mcp-sign-in", p, widgets::DialogSize::Normal, widgets::Fit::Fixed, widgets::dialog_window(cx, *s.zoom), dialog)
}

// `--scene mcp-sign-in-long`: a sign-in that failed with an error longer than the dialog, which
// scrolls in it.
rune_register::register!(crate::scenes::SCENE_LIST: crate::scenes::Scene = [
    ("mcp-sign-in-long", |s| {
        let error = (1..=30).map(|i| format!("at frame {i}: invalid_grant from https://auth.example.com/token")).collect::<Vec<_>>().join("\n");
        s.mcp.sign_in = Some(SignIn {
            dir: String::new(),
            name: "linear".into(),
            integration: "linear".into(),
            attempt: "msi_scene".into(),
            url: "https://auth.example.com/authorize".into(),
            state: State::Failed(Failure::Auth(error)),
            field: TextEdit::default(),
            issued: Seq::default(),
        });
    }),
]);
