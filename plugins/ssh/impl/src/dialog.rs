//! Add SSH server and Connect to <host> (`dialog.tsx`, `dialog-machine.ts`): the target and an
//! optional name, then each sign-in prompt the connect raises (a password, a key's passphrase, a
//! host's fingerprint to trust), until the server is up. The dialog follows the main side's state
//! for its server; closing it while the attempt runs cancels it.

use opencode_plugin::abi::{BUTTON_CONTRAST, BUTTON_NEUTRAL, SHADOW_OVERLAY};
use opencode_plugin::prelude::*;

use crate::SshWindow;
use crate::controller::with;
use crate::machine::{Config, Event, Prompt, Record, Stage};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Mode {
    Add,
    Connect,
    /// Only an attempt's prompts (it started without the dialog).
    Prompt,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Status {
    Idle,
    Active { prompted: bool, challenge: Option<Prompt>, answered: Option<String> },
    Completed,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct DialogState {
    pub id: String,
    pub mode: Mode,
    pub status: Status,
    /// Opened from the palette: once up, the window connects to it (Desktop opens a project there).
    pub open_project: bool,
}

pub struct Fields {
    pub target: EditorHandle,
    pub name: EditorHandle,
    pub response: EditorHandle,
}

impl SshWindow {
    /// Opens the dialog for a new server (`add`), a known one (`connect`), or its prompts.
    pub fn open_dialog(&mut self, cx: &mut ActionCx, mode: Mode, config: Option<Config>, open_project: bool) {
        let id = config.as_ref().map_or_else(crate::token, |c| c.id.clone());
        cx.set_editor_text(self.fields.target, config.as_ref().map_or("", |c| c.target.as_str()));
        cx.set_editor_text(self.fields.name, config.as_ref().map_or("", |c| c.name.as_str()));
        cx.set_editor_text(self.fields.response, "");
        let status = if mode == Mode::Prompt { Status::Active { prompted: true, challenge: None, answered: None } } else { Status::Idle };
        self.dialog = Some(DialogState { id, mode, status, open_project });
        cx.open_dialog(crate::DIALOG);
        self.sync(cx);
    }

    /// Follows the main side's state for the dialog's server (`synced`).
    pub fn sync(&mut self, cx: &mut ActionCx) {
        let Some(core) = self.core() else { return };
        let Some(d) = self.dialog.as_mut() else { return };
        let item = core.borrow().state.servers.iter().find(|r| r.config.id == d.id).cloned();
        let Status::Active { prompted, challenge, answered } = &mut d.status else { return };
        if item.as_ref().is_some_and(|i| i.stage() == Stage::Ready) {
            let (id, open_project) = (d.id.clone(), d.open_project);
            d.status = Status::Completed;
            cx.close_dialog();
            if open_project {
                cx.connect_server(&id);
            }
            return;
        }
        if let Some(prompt) = item.as_ref().and_then(Record::prompt) {
            if challenge.as_ref().map(|c| &c.id) != Some(&prompt.id) {
                *challenge = Some(prompt.clone());
                *prompted = true;
                *answered = None;
                cx.set_editor_text(self.fields.response, "");
            }
            return;
        }
        // The prompt this window answered stays, disabled, while the server checks the answer.
        let keep = challenge.as_ref().is_some_and(|c| answered.as_deref() == Some(c.id.as_str())) && item.as_ref().is_some_and(|i| i.stage() == Stage::Connecting);
        if !keep {
            *challenge = None;
        }
    }

    fn submit(&mut self, cx: &mut ActionCx) {
        let Some(core) = self.core() else { return cx.toast_error("Request failed", "SSH is unavailable") };
        let (target, name) = (cx.editor_text(self.fields.target), cx.editor_text(self.fields.name));
        let Some(d) = self.dialog.as_mut() else { return };
        let item = core.borrow().state.servers.iter().find(|r| r.config.id == d.id).cloned();
        let busy = matches!(d.status, Status::Active { .. }) && item.as_ref().is_some_and(|i| i.stage() == Stage::Connecting);
        if busy || target.trim().is_empty() || matches!(&d.status, Status::Active { challenge: Some(_), .. }) {
            return;
        }
        d.status = Status::Active { prompted: false, challenge: None, answered: None };
        let config = Config { id: d.id.clone(), target, name };
        let replace = item.is_some_and(|i| i.stage() == Stage::Incompatible);
        with(&core, cx, |c, cx| c.dispatch(cx, Event::Start { config, replace, background: false }));
        self.sync(cx);
    }

    fn respond(&mut self, cx: &mut ActionCx) {
        let Some(core) = self.core() else { return };
        let value = cx.editor_text(self.fields.response);
        let Some(d) = self.dialog.as_mut() else { return };
        let Status::Active { challenge: Some(prompt), answered, .. } = &mut d.status else { return };
        if answered.as_deref() == Some(prompt.id.as_str()) || (!prompt.confirm && value.is_empty()) {
            return;
        }
        *answered = Some(prompt.id.clone());
        let event = Event::Respond { id: d.id.clone(), prompt: prompt.id.clone(), value: if prompt.confirm { "yes".into() } else { value } };
        with(&core, cx, |c, cx| c.dispatch(cx, event));
    }

    /// The dialog went away: its attempt goes with it.
    pub fn dismissed(&mut self, cx: &mut ActionCx) {
        let Some(d) = self.dialog.take() else { return };
        if let (Status::Active { .. }, Some(core)) = (&d.status, self.core()) {
            with(&core, cx, |c, cx| c.dispatch(cx, Event::Cancel { id: d.id }));
        }
    }
}

/// What the dialog shows (`viewSshDialog`).
struct View {
    connect_to: bool,
    show_form: bool,
    inputs_disabled: bool,
    incompatible: bool,
    prompt: Option<Prompt>,
    waiting: bool,
    error: Option<&'static str>,
    respond: bool,
    primary_disabled: bool,
    primary: &'static str,
}

fn view(d: &DialogState, item: Option<&Record>, target_empty: bool, response_empty: bool) -> View {
    let (prompted, challenge, answered) = match &d.status {
        Status::Active { prompted, challenge, answered } => (*prompted, challenge.clone(), answered.clone()),
        _ => (false, None, None),
    };
    let active = matches!(d.status, Status::Active { .. });
    let stage = item.map(Record::stage);
    let incompatible = stage == Some(Stage::Incompatible);
    let settled = d.status == Status::Completed;
    let error = item.and_then(Record::error).filter(|_| active || d.mode != Mode::Add);
    let busy = active && error.is_none() && stage.unwrap_or(Stage::Connecting) == Stage::Connecting;
    let waiting = busy || (active && error.is_none() && challenge.as_ref().is_some_and(|c| answered.as_deref() == Some(c.id.as_str()))) || settled;
    let error = (!incompatible).then_some(error.map(crate::command::Code::message)).flatten();
    let (respond, primary_disabled, primary) = match &challenge {
        Some(c) => (true, waiting || (!c.confirm && response_empty), if waiting { "Connecting over SSH\u{2026}" } else if c.confirm { "Trust and connect" } else { "Continue" }),
        None => (false, busy || target_empty || settled, if busy { "Connecting over SSH\u{2026}" } else if incompatible { "Update and reconnect" } else if d.mode == Mode::Add { "Add server" } else { "Connect" }),
    };
    View {
        connect_to: prompted || d.mode != Mode::Add,
        show_form: d.mode != Mode::Prompt && !incompatible && (!prompted || (error.is_some() && challenge.is_none())),
        inputs_disabled: busy || challenge.is_some() || settled,
        incompatible,
        prompt: challenge,
        waiting,
        error,
        respond,
        primary_disabled,
        primary,
    }
}

pub fn render(s: &SshWindow, cx: &mut ViewCx<SshWindow>, _: &Props) -> El {
    let Some(d) = s.dialog.clone() else { return cx.nothing() };
    let item = s.core().and_then(|c| c.borrow().state.servers.iter().find(|r| r.config.id == d.id).cloned());
    let target = cx.editor_text(s.fields.target);
    let name = cx.editor_text(s.fields.name);
    let v = view(&d, item.as_ref(), target.trim().is_empty(), cx.editor_text(s.fields.response).is_empty());
    let p = cx.pal();
    let host = crate::name(&Config { id: String::new(), target: if v.connect_to && target.is_empty() { item.as_ref().map(|i| i.config.target.clone()).unwrap_or_default() } else { target }, name });
    let title = if v.connect_to { format!("Connect to {host}") } else { "Add SSH server".into() };
    let header = row().w_full().padding(16.0).center_y().child(text(title).font(13.0, 530.0).line_height(16.0).tracking(-0.04).color(p.text));
    let mut body = col().w_full().gap(24.0);
    let label = |t: &'static str| text(t).font(13.0, 530.0).line_height(16.0).color(p.text);
    if v.show_form {
        let target_field = ui::field(s.fields.target, "ssh user@example.com").large().invalid(v.error.is_some()).disabled(v.inputs_disabled).w_full().key("ssh-target");
        let target_field = target_field.on_press(cx.handler(|s: &mut SshWindow, cx| s.submit(cx)));
        let name_field = ui::field(s.fields.name, "Localhost").large().disabled(v.inputs_disabled).w_full().key("ssh-name").on_press(cx.handler(|s: &mut SshWindow, cx| s.submit(cx)));
        body = body.child(col().w_full().gap(8.0).child(label("Host or SSH command")).child(target_field)).child(col().w_full().gap(8.0).child(label("Server name (optional)")).child(name_field));
    }
    if v.incompatible {
        body = body.child(
            col()
                .w_full()
                .gap(8.0)
                .child(text("Server update required").font(14.0, 530.0).line_height(20.0).color(p.text))
                .child(text(crate::command::Code::Version.message()).font(13.0, 440.0).line_height(20.0).color(p.muted).wrap()),
        );
    }
    if let Some(prompt) = &v.prompt {
        let mut block = col().w_full().gap(8.0).child(text(prompt.text.clone()).mono().font(12.0, 440.0).line_height(16.0).wrap().selectable().max_h(200.0).scroll_y().color(p.text).key("ssh-prompt"));
        if !prompt.confirm {
            block = block.child(ui::field(s.fields.response, "").large().secure().disabled(v.waiting).w_full().key("ssh-response").on_press(cx.handler(|s: &mut SshWindow, cx| s.respond(cx))));
        }
        body = body.child(block);
    }
    if let Some(e) = v.error {
        let danger = cx.token("state-fg-danger").unwrap_or(p.danger);
        body = body.child(text(e).font(11.0, 440.0).line_height(16.0).wrap().color(danger).key("ssh-error"));
    }
    let cancel = ui::button_with("Cancel", BUTTON_NEUTRAL).key("ssh-cancel").on_press(cx.handler(|s: &mut SshWindow, cx| {
        s.dismissed(cx);
        cx.close_dialog();
    }));
    let primary = ui::button_with(v.primary, BUTTON_CONTRAST).disabled(v.primary_disabled).key("ssh-primary");
    let respond = v.respond;
    let primary = primary.on_press(cx.handler(move |s: &mut SshWindow, cx| if respond { s.respond(cx) } else { s.submit(cx) }));
    col()
        .key("ssh-dialog")
        .w(480.0)
        .fill(p.layer1)
        .radius(6.0)
        .shadow(SHADOW_OVERLAY)
        .clip()
        .child(header)
        .child(el().w_full().h(0.5).fill(p.border_strong))
        .child(col().w_full().px(16.0).pt(16.0).pb(8.0).child(body))
        .child(row().w_full().padding(16.0).gap(8.0).child(spacer()).child(cancel).child(primary))
}
