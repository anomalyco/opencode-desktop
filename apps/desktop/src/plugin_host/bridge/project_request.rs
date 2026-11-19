//! `Layout:OpenProject(serverKey, title)` (Desktop's `layout.project`, `ProjectRequestState` in
//! `runtime/plugin/host-layout-machine.ts`): desktop wiring over
//! `opencode_plugin_runtime::machines::ProjectRequest`.

pub use opencode_plugin_runtime::machines::{
    ProjectRequest as State, ProjectRequestEffect as Effect, ProjectRequestEvent as Event,
    step_project_request as step,
};
use rune::Tx;

use crate::App;

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
    let effects = step(&mut host.project, &mut host.project_attempt, Event::Request { server, title, listed, now });
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
    crate::projects::remote_picker::pick_with(app, key, title, tx, move |h, dir, tx| {
        tx.instant();
        h.close_settings();
        h.switch_server(&server);
        crate::projects::open_project_dir(h, &dir, false, tx);
        h.open_draft_in(&server, &dir, tx);
        tx.settle();
    });
}
