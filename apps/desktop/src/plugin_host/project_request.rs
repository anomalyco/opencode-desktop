//! `Layout:OpenProject(serverKey, title)` (Desktop's `layout.project`, `ProjectRequestState` in
//! `runtime/plugin/host-layout-machine.ts`, ported transition for transition): a server that's
//! listed asks for a folder at once; one that isn't yet (a server the plugin just started) gets
//! `PROJECT_REQUEST_TTL_MS` to be listed, and a newer request replaces a waiting one. The folder
//! picked opens as a project on that server, with a draft in it.

use rune::Tx;
use rune::machine::Attempt;

use crate::App;

/// `PROJECT_REQUEST_TTL_MS`.
pub const PROJECT_REQUEST_TTL_MS: u64 = 30_000;

/// `ProjectRequestState`; the timer's due time rides along while it waits (the app's clock).
#[derive(Clone, Debug, Default, PartialEq)]
pub enum State {
    #[default]
    Idle,
    Waiting { attempt: Attempt, server: String, title: String, due: f64 },
}

pub enum Event {
    Request { server: String, title: String, listed: bool, now: f64 },
    Listed { server: String },
    Expire { attempt: Attempt },
}

#[derive(Debug, PartialEq)]
pub enum Effect {
    OpenPicker { server: String, title: String },
    /// The app's clock time the expiry is due.
    ScheduleExpire(f64),
}

impl State {
    fn attempt(&self, last: Attempt) -> Attempt {
        match self {
            State::Idle => last,
            State::Waiting { attempt, .. } => *attempt,
        }
    }

    /// The server a waiting request needs listed (`selectWaitingProjectServer`).
    pub fn waiting(&self) -> Option<&str> {
        match self {
            State::Waiting { server, .. } => Some(server),
            State::Idle => None,
        }
    }
}

/// `transitionProjectRequest`; `last` is the attempt the idle state carries in Desktop's.
pub fn step(state: &mut State, last: &mut Attempt, event: Event) -> Vec<Effect> {
    match event {
        Event::Request { server, title, listed: true, .. } => {
            *last = state.attempt(*last);
            *state = State::Idle;
            vec![Effect::OpenPicker { server, title }]
        }
        Event::Request { server, title, listed: false, now } => {
            let attempt = Attempt(state.attempt(*last).0 + 1);
            *last = attempt;
            let due = now + PROJECT_REQUEST_TTL_MS as f64 / 1000.0;
            *state = State::Waiting { attempt, server, title, due };
            vec![Effect::ScheduleExpire(due)]
        }
        Event::Listed { server } => match std::mem::take(state) {
            State::Waiting { attempt, server: waiting, title, .. } if waiting == server => {
                *last = attempt;
                vec![Effect::OpenPicker { server, title }]
            }
            other => {
                *state = other;
                Vec::new()
            }
        },
        Event::Expire { attempt } => {
            if matches!(state, State::Waiting { attempt: a, .. } if *a == attempt) {
                *last = attempt;
                *state = State::Idle;
            }
            Vec::new()
        }
    }
}

/// The server key a plugin names: `local` is the app's own.
fn server_key(key: &str) -> opencode_kit::servers::ServerKey {
    if key == opencode_kit::servers::ServerScope::LOCAL { opencode_kit::servers::ServerKey::sidecar() } else { opencode_kit::servers::ServerKey::new(key) }
}

fn listed(key: &str) -> bool {
    let key = server_key(key);
    crate::servers::runtime::listed().iter().any(|l| l.key == key)
}

/// `layout.project(server, title)`.
pub fn request(app: &mut App, server: String, title: String, tx: &mut Tx) {
    let listed = listed(&server);
    let now = *app.clock;
    let host = &mut *app.plugins;
    let effects = step(&mut host.project, &mut host.project_attempt, Event::Request { server, title, listed, now: now });
    run(app, effects, tx);
}

/// `poll`: a waiting request's server was listed, or its time ran out.
pub fn poll(app: &mut App, tx: &mut Tx) {
    // Every poll comes here: the host is written only for a request waiting on its server.
    if !matches!(app.plugins.project, State::Waiting { .. }) {
        return;
    }
    let host = &mut *app.plugins;
    let effects = match &host.project {
        State::Waiting { server, .. } if listed(server) => {
            let server = server.clone();
            step(&mut host.project, &mut host.project_attempt, Event::Listed { server })
        }
        State::Waiting { attempt, due, .. } if *app.clock >= *due => {
            let attempt = *attempt;
            step(&mut host.project, &mut host.project_attempt, Event::Expire { attempt })
        }
        _ => return,
    };
    run(app, effects, tx);
}

fn run(app: &mut App, effects: Vec<Effect>, tx: &mut Tx) {
    for effect in effects {
        match effect {
            Effect::ScheduleExpire(due) => tx.wake_at(due),
            Effect::OpenPicker { server, title } => open_picker(app, &server_key(&server), title, tx),
        }
    }
}

/// `useDirectoryPicker` (`projects::remote_picker`): this machine's folder dialog for a server
/// that shares its files, the server's own folder browser for another; the folder picked opens
/// as a project there, with a draft in it (`projects.add`, `tabs.newDraft`).
fn open_picker(app: &mut App, key: &opencode_kit::servers::ServerKey, title: String, tx: &mut Tx) {
    let title = if title.is_empty() { opencode_i18n::t!("command.project.open").to_owned() } else { title };
    let server = key.clone();
    crate::projects::remote_picker::pick(app, key, title, tx, move |s: &mut App, dir: String, tx: &mut Tx| {
        tx.instant();
        s.close_settings();
        crate::servers::runtime::switch(s, &server);
        s.open_project_dir(&dir, false, tx);
        *s.default_dir = crate::projects::clean_dir(&dir);
        s.open_draft_tab();
        tx.settle();
    });
}
