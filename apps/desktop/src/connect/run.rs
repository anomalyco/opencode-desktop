//! The connect dialog's effects against the server (Desktop's `providers/connect/controller.ts`):
//! each request runs as a job and its answer comes back as the dialog's next event. A window
//! that can't write answers a write with the read-only message instead.

use std::time::Duration;

use opencode_kit::client::Server;
use opencode_kit::integration::{AttemptStatus, MethodKind};
use rune::Tx;
use serde_json::Value;

use super::dialog::{CLAUDE, CLAUDE_CHECK_TIMEOUT_S, CONSOLE, Effect, Event, Snapshot};
use super::machine::{Catalog, Effect as SignIn, Event as Answer, Outcome, Signin};
use crate::App;
use crate::jobs::{Apply, READ_ONLY};

/// How long the Console's workspace providers may take to show after a sign-in.
const CONSOLE_SETTLE_TRIES: u32 = 20;

impl App {
    pub(super) fn run_connect(&mut self, effect: Effect, tx: &mut Tx) {
        match effect {
            Effect::Connect { session, provider, effect } => self.run_sign_in(session, provider, effect, tx),
            Effect::AccountActivate { session, attempt, provider, account } => {
                let name = self.provider_name(&provider);
                let label = self.account_label(&provider, &account);
                self.write_then(move |srv| srv.credential_activate(&account), move |s, tx, r| {
                    if r.is_ok() {
                        crate::status_toast::succeed(s, format!("{name} account switched"), format!("Now using {label}."));
                    }
                    s.settle_account(session, attempt, tx);
                });
            }
            Effect::AccountRemove { session, attempt, provider, account } => {
                let name = self.provider_name(&provider);
                let label = self.account_label(&provider, &account);
                self.write_then(move |srv| srv.credential_remove(&account), move |s, tx, r| {
                    if r.is_ok() {
                        crate::status_toast::succeed(s, format!("{label} removed"), format!("{name} will no longer use this account."));
                    }
                    s.settle_account(session, attempt, tx);
                });
            }
            Effect::ClaudeReplace(ids) => {
                for id in ids {
                    self.write_then(move |srv| srv.credential_remove(&id), |_, _, _| {});
                }
            }
            Effect::ClaudeTimer { session, check } => {
                self.job_after(Duration::from_secs_f64(CLAUDE_CHECK_TIMEOUT_S), move |_| {
                    Box::new(move |s: &mut App, tx: &mut Tx| s.connect_event(Event::ClaudeTimedOut { session, check }, tx)) as Apply
                });
            }
            Effect::ClaudeConfirm { session, check } => {
                let dir = self.connect_dir().map(str::to_owned);
                self.sync_catalogs(dir, move |s, tx, _| {
                    let problem = s.accounts_of(CLAUDE).problem;
                    let first = s.first_model(CLAUDE);
                    s.connect_event(Event::ClaudeConfirmed { session, check, problem, first }, tx);
                });
            }
            Effect::Reveal(provider) => {
                let keys: Vec<String> = self.connection_models(&provider).into_iter().map(|m| m.key).collect();
                self.pickers.prefs.set_visibility(keys.iter().map(String::as_str), true);
            }
            Effect::Connected(_) => {}
            Effect::Toast(provider) => {
                let name = self.provider_name(&provider);
                let description = if self.connection_models(&provider).is_empty() { String::new() } else { format!("{name} models are now available to use.") };
                crate::status_toast::succeed(self, format!("{name} connected"), description);
            }
            Effect::Apply(key) => self.apply_connected_model(&key, tx),
            Effect::Close => self.close_connect(tx),
        }
    }

    /// A write that answers `then` with its result; read-only windows answer at once.
    fn write_then<T: Send + 'static>(
        &self,
        work: impl FnOnce(&Server) -> Result<T, String> + Send + 'static,
        then: impl FnOnce(&mut App, &mut Tx, Result<T, String>) + Send + 'static,
    ) {
        if !self.can_write() {
            return self.later(Box::new(move |s: &mut App, tx: &mut Tx| then(s, tx, Err(READ_ONLY.into()))));
        }
        self.job(move |srv| {
            let r = work(srv);
            Box::new(move |s: &mut App, tx: &mut Tx| then(s, tx, r)) as Apply
        });
    }

    fn settle_account(&mut self, session: u32, attempt: u32, _tx: &mut Tx) {
        let dir = self.connect_dir().map(str::to_owned);
        self.sync_catalogs(dir, move |s, tx, _| s.connect_event(Event::AccountSettled { session, attempt }, tx));
    }

    fn run_sign_in(&mut self, session: u32, provider: String, effect: SignIn, tx: &mut Tx) {
        let dir = self.connect_dir().map(str::to_owned);
        let console = provider == CONSOLE;
        // Each answer comes back tagged with the provider and session it belongs to.
        let reply = {
            let provider = provider.clone();
            move |event: Answer, catalog: Option<Snapshot>| -> Apply {
                Box::new(move |s: &mut App, tx: &mut Tx| s.connect_event(Event::Result { session, provider, event, catalog }, tx))
            }
        };
        match effect {
            SignIn::Load { attempt } => self.job(move |srv| {
                let catalog = match srv.integration(&provider, dir.as_deref()) {
                    Ok(i) => Catalog { methods: i.connect_methods(), integration: Some(i) },
                    // One the server can't describe can still take an API key.
                    Err(_) => Catalog { integration: None, methods: vec![key_method()] },
                };
                reply(Answer::Loaded { attempt, catalog, manual: false }, None)
            }),
            SignIn::Start { attempt, method, answer } => {
                let answer = answer.map(Value::Object);
                let id = method.id.clone().unwrap_or_default();
                let kind = method.kind;
                let failed = move |e: String| Answer::StartFailed { attempt, error: if console && kind == MethodKind::Oauth { "Couldn't start sign-in. Check your server connection and try again.".into() } else { e } };
                self.write_then(
                    move |srv| match kind {
                        MethodKind::Command => srv.command_connect(&provider, dir.as_deref(), &id).map(|id| Answer::CommandStarted { attempt, id }),
                        MethodKind::External => srv.connect_external(&provider, dir.as_deref(), &id, answer.as_ref()).map(|_| Answer::ExternalSaved { attempt }),
                        _ => srv.oauth_connect(&provider, dir.as_deref(), &id, answer.as_ref()).map(|auth| Answer::OauthStarted { attempt, auth }),
                    },
                    move |s, tx, r| reply(r.unwrap_or_else(failed), None)(s, tx),
                );
            }
            SignIn::OpenBrowser { attempt_id, url } => {
                let opened = !crate::headless() && tx.open_url(&url).is_ok();
                self.later(reply(Answer::BrowserOpened { attempt_id, opened }, None));
            }
            SignIn::FormOpenBrowser { attempt, browser, url } => {
                let opened = !crate::headless() && tx.open_url(&url).is_ok();
                self.later(reply(Answer::FormBrowserOpened { attempt, browser, opened }, None));
            }
            SignIn::Poll { attempt, signin, delay } => {
                let command = matches!(signin, Signin::Command { .. });
                let every = if command || console { 0.5 } else { 1.0 };
                self.job_after(Duration::from_secs_f64(if delay { every } else { 0.0 }), move |srv| {
                    let outcome = poll(srv, &provider, dir.as_deref(), &signin, console);
                    reply(Answer::Polled { attempt, outcome }, None)
                });
            }
            SignIn::PollStop | SignIn::Complete { .. } => {}
            SignIn::Cancel { command, attempt_id } => self.write_then(
                move |srv| if command { srv.command_cancel(&provider, &attempt_id, dir.as_deref()) } else { srv.oauth_cancel(&provider, &attempt_id, dir.as_deref()) },
                |_, _, _| {},
            ),
            SignIn::Submit { attempt, signin, value, answer } => {
                let code = matches!(signin, Signin::Code { .. });
                let answer = answer.map(Value::Object);
                self.write_then(
                    move |srv| match &signin {
                        Signin::Code { auth, .. } => srv.oauth_complete(&provider, &auth.attempt, dir.as_deref(), &value),
                        _ => srv.connect_key(&provider, dir.as_deref(), &value, answer.as_ref()),
                    },
                    move |s, tx, r| {
                        let event = match r {
                            Ok(()) => Answer::SubmitSucceeded { attempt },
                            Err(e) if e.is_empty() && code => Answer::SubmitFailed { attempt, error: "Invalid authorization code".into() },
                            Err(error) => Answer::SubmitFailed { attempt, error },
                        };
                        reply(event, None)(s, tx);
                    },
                );
            }
            SignIn::Refresh { attempt, method } => {
                let wait = console && method.kind != MethodKind::Key;
                self.refresh_connection(dir, provider, session, attempt, wait.then_some(CONSOLE_SETTLE_TRIES));
            }
            SignIn::Sync => self.sync_catalogs(dir, |_, _, _| {}),
        }
    }

    /// Reads the catalogs after a sign-in. The Console's workspace providers can land after its
    /// grant, so its refresh (`tries`) reads again until they show or the tries run out.
    fn refresh_connection(&mut self, dir: Option<String>, provider: String, session: u32, attempt: u32, tries: Option<u32>) {
        self.sync_catalogs(dir.clone(), move |s, tx, ok| {
            if provider == CONSOLE && !ok {
                let error = "Your account is connected, but we couldn't load your models. Try again to refresh them.".into();
                return s.connect_event(Event::Result { session, provider, event: Answer::RefreshFailed { attempt, error }, catalog: None }, tx);
            }
            if let Some(n) = tries.filter(|n| *n > 0)
                && !s.console_loaded()
            {
                s.job_after(Duration::from_millis(500), move |_| {
                    Box::new(move |s: &mut App, _: &mut Tx| s.refresh_connection(dir, provider, session, attempt, Some(n - 1))) as Apply
                });
                return;
            }
            let catalog = Snapshot { pending: tries == Some(0) && !s.console_loaded(), first: s.first_model(&provider) };
            s.connect_event(Event::Result { session, provider, event: Answer::RefreshSucceeded { attempt }, catalog: Some(catalog) }, tx);
        });
    }

    /// A signed-in Console workspace lists its own providers beside the free `opencode` one.
    fn console_loaded(&self) -> bool {
        self.provider_infos(self.connect_dir()).unwrap_or_default().iter().any(|p| p.integration.as_deref() == Some(CONSOLE) && p.id != CONSOLE)
    }

    /// The first model the connection offers, `provider:id`, as its models step lights it.
    pub(crate) fn first_model(&self, provider: &str) -> Option<String> {
        super::steps::connection_groups(self, provider).first().and_then(|(_, models)| models.first()).map(|m| m.key.clone())
    }

    pub(crate) fn provider_name(&self, id: &str) -> String {
        self.integration_of(self.connect_dir(), id).map(|i| i.name.clone())
            .or_else(|| self.provider_infos(self.connect_dir()).and_then(|ps| ps.iter().find(|p| p.id == id).map(|p| p.name.clone())))
            .unwrap_or_else(|| id.to_owned())
    }

    fn account_label(&self, provider: &str, account: &str) -> String {
        self.integration_of(self.connect_dir(), provider)
            .and_then(|i| i.connections.iter().find(|c| c.id == account).map(|c| c.label.clone()))
            .unwrap_or_else(|| account.to_owned())
    }

    /// Continue on the models step: the composer that opened the dialog runs with the model;
    /// from Settings a new session starts with it.
    fn apply_connected_model(&mut self, key: &str, tx: &mut Tx) {
        use crate::pickers::PickerHost as _;
        let Some(m) = self.pickers.models.iter().find(|m| m.key == key).cloned() else { return };
        let scope = self.connect.open.as_ref().and_then(|o| o.scope.clone());
        if scope.is_none() {
            self.close_settings();
            self.new_session(tx);
        }
        let scope = scope.unwrap_or_else(|| self.picker_scope());
        let mut selection = self.selection();
        selection.model = opencode_kit::model::ModelRef { id: m.id, provider: m.provider, variant: String::new() };
        self.pickers.prefs.picked(key);
        self.pickers.selections.insert(scope, selection);
    }
}

fn key_method() -> opencode_kit::integration::Method {
    opencode_kit::integration::Method { kind: MethodKind::Key, id: None, label: Some("API key".into()), form: vec![] }
}

/// One status read of a browser sign-in or a command, in the words the dialog shows.
fn poll(srv: &Server, provider: &str, dir: Option<&str>, signin: &Signin, console: bool) -> Outcome {
    if let Signin::Command { attempt, .. } = signin {
        return match srv.command_status(provider, attempt, dir) {
            Ok(AttemptStatus::Pending(output)) => Outcome::Pending(output),
            Ok(AttemptStatus::Complete) => Outcome::Complete,
            Ok(AttemptStatus::Failed(message)) => Outcome::Failed(message),
            Ok(AttemptStatus::Expired) => Outcome::Failed("Authentication expired".into()),
            Err(e) => Outcome::Failed(e),
        };
    }
    let attempt = match signin {
        Signin::Browser { auth, .. } | Signin::Code { auth, .. } => auth.attempt.clone(),
        _ => return Outcome::Failed(String::new()),
    };
    let expired = if console { "This sign-in request has expired. Start again to get a new device code." } else { "Authorization expired" };
    match srv.oauth_status(provider, &attempt, dir) {
        Ok(AttemptStatus::Pending(_)) => Outcome::Pending(String::new()),
        Ok(AttemptStatus::Complete) => Outcome::Complete,
        Ok(AttemptStatus::Expired) => Outcome::Failed(expired.into()),
        Ok(AttemptStatus::Failed(m)) if console && m.contains("expired_token") => Outcome::Failed(expired.into()),
        Ok(AttemptStatus::Failed(m)) if console && m.contains("access_denied") => {
            Outcome::Failed("Access was denied in Console. Try again when you're ready to connect.".into())
        }
        Ok(AttemptStatus::Failed(m)) => Outcome::Failed(m),
        Err(_) if console => Outcome::Unreachable("Couldn't check authorization. Check your server connection and try again.".into()),
        Err(e) => Outcome::Unreachable(e),
    }
}
