//! Import session: a session export (JSON) picked from disk, imported into a project through
//! `experimental/session/import`: Desktop's `ProjectImportState`
//! (`home/projects/import-machine.ts:15-19`) and its driver (`home/projects/actions.tsx:61-152`).
//! One import at a time; while the transcript uploads, a toast says so; the imported session
//! opens only while the window still shows where the import was asked from.

use std::path::{Path, PathBuf};

use opencode_i18n::t;
use opencode_kit::model::SessionInfo;
use opencode_kit::servers::{Route, ServerKey};
use opencode_kit::wire_error::WireError;
use rune::machine::{Attempt, Effects, Seq};
use rune::{FileDialogOptions, Tx};
use serde_json::{Value, json};

use crate::host::ProjectsHost;
use crate::jobs::ProjectsJob;

/// `ProjectImportTarget`: where the import was asked from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Target {
    pub server: ServerKey,
    pub directory: String,
    pub origin: Route,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub enum State {
    #[default]
    Idle,
    /// The file picker is open.
    Picking { attempt: Attempt, target: Target },
    /// The transcript uploads; `toast` is the pending toast's id.
    Importing { attempt: Attempt, target: Target, toast: u64 },
}

#[derive(Default)]
pub struct Import {
    seq: Seq,
    pub state: State,
}

#[derive(Debug)]
pub enum Event {
    /// `import.requested`.
    Requested(Target),
    /// `picker.closed`: the file picked, `None` when the picker was cancelled.
    Picked { attempt: Attempt, file: Option<PathBuf> },
    /// `picker.failed`.
    PickerFailed { attempt: Attempt, why: String },
    /// `import.succeeded`; `here` is where the window is now.
    Imported { attempt: Attempt, session: Box<SessionInfo>, here: Route },
    /// `import.failed`.
    Failed { attempt: Attempt, why: String },
}

#[derive(Debug)]
pub enum Effect {
    /// `picker.open`.
    Pick { attempt: Attempt },
    /// `toast.pending.show` and `upload.start`.
    Upload { attempt: Attempt, target: Target, file: PathBuf },
    /// `toast.pending.dismiss`.
    Dismiss(u64),
    /// `toast.failed`.
    Fail(String),
    /// `session.open`.
    Open { target: Target, session: SessionInfo },
    /// `session.remember`: the imported session joins the lists without switching tabs.
    Remember { session: SessionInfo },
}

impl Import {
    /// Whether an import is under way (`selectProjectImporting`).
    pub fn importing(&self) -> bool {
        !matches!(self.state, State::Idle)
    }

    /// `transitionProjectImport`.
    pub fn step(&mut self, event: Event) -> Effects<Effect> {
        match (&self.state, event) {
            // A second Import session while the picker is open or a transcript uploads would run two.
            (State::Idle, Event::Requested(target)) => {
                let attempt = self.seq.next();
                self.state = State::Picking { attempt, target };
                Effects::one(Effect::Pick { attempt })
            }
            (State::Picking { attempt, target }, Event::Picked { attempt: a, file }) if *attempt == a => {
                let (attempt, target) = (*attempt, target.clone());
                self.state = State::Idle;
                match file {
                    Some(file) => Effects::one(Effect::Upload { attempt, target, file }),
                    None => Effects::none(),
                }
            }
            (State::Picking { attempt, .. }, Event::PickerFailed { attempt: a, why }) if *attempt == a => {
                self.state = State::Idle;
                Effects::one(Effect::Fail(why))
            }
            (State::Importing { attempt, toast, .. }, Event::Failed { attempt: a, why }) if *attempt == a => {
                let toast = *toast;
                self.state = State::Idle;
                Effects::two(Effect::Dismiss(toast), Effect::Fail(why))
            }
            // The reader moved on while the transcript uploaded: the import doesn't pull them away.
            (State::Importing { attempt, target, toast }, Event::Imported { attempt: a, session, here }) if *attempt == a => {
                let (target, toast) = (target.clone(), *toast);
                self.state = State::Idle;
                let land = if here == target.origin { Effect::Open { target, session: *session } } else { Effect::Remember { session: *session } };
                Effects::two(Effect::Dismiss(toast), land)
            }
            _ => Effects::none(),
        }
    }

    /// The upload started with its pending toast: the machine holds both.
    pub fn uploading(&mut self, attempt: Attempt, target: Target, toast: u64) {
        self.state = State::Importing { attempt, target, toast };
    }
}

/// Import session into `directory` of the active server (`importSession`).
pub fn import_session(host: &mut dyn ProjectsHost, directory: &str, tx: &mut Tx) {
    let target = Target {
        server: host.active_server(),
        directory: directory.to_owned(),
        origin: host.current_route(),
    };
    step_host(host, Event::Requested(target), tx);
}

/// Steps the import machine on `host` and runs all emitted effects.
pub fn step_host(host: &mut dyn ProjectsHost, event: Event, tx: &mut Tx) {
    let effects = host.project_state_mut().import.step(event);
    for effect in effects {
        match effect {
            Effect::Pick { attempt } => {
                let options = FileDialogOptions { title: t!("command.session.import").into(), ..Default::default() }
                    .filter("JSON", &["json"]);
                host.open_file_dialog(
                    options,
                    tx,
                    Box::new(move |h, result, tx| {
                        let ev = match result {
                            Ok(file) => Event::Picked { attempt, file },
                            Err(why) => Event::PickerFailed { attempt, why },
                        };
                        step_host(h, ev, tx);
                    }),
                );
            }
            Effect::Upload { attempt, target, file } => {
                let toast = host.toast_waiting(t!("toast.session.import.pending"));
                host.project_state_mut().import.uploading(attempt, target.clone(), toast);
                tx.instant();
                upload_session(host, attempt, target, &file, tx);
            }
            Effect::Dismiss(toast) => host.toast_dismiss(toast),
            Effect::Fail(why) => host.toast_fail(t!("common.requestFailed"), &why),
            Effect::Open { target, session } => {
                let id = session.id.clone();
                remember_session(host, session);
                host.open_session(&target.server, &id, tx);
            }
            Effect::Remember { session } => remember_session(host, session),
        }
    }
    tx.instant();
}

pub(crate) fn apply_uploaded(
    host: &mut dyn ProjectsHost,
    attempt: Attempt,
    result: Result<SessionInfo, WireError>,
    tx: &mut Tx,
) {
    let event = match result {
        Ok(session) => Event::Imported { attempt, session: Box::new(session), here: host.current_route() },
        Err(e) => Event::Failed { attempt, why: host.format_wire_error("", &e) },
    };
    step_host(host, event, tx);
}

fn remember_session(host: &mut dyn ProjectsHost, session: SessionInfo) {
    if let Some(sessions) = host.sessions_mut()
        && !sessions.iter().any(|x| x.id == session.id)
    {
        sessions.insert(0, session);
    }
}

/// Reads the export and imports it into `target`'s folder on a worker; what it reads that
/// isn't an export fails the import, as a server rejecting it does.
fn upload_session(host: &mut dyn ProjectsHost, attempt: Attempt, target: Target, path: &Path, tx: &mut Tx) {
    let parsed = std::fs::read_to_string(path)
        .map_err(|e| e.to_string())
        .and_then(|text| serde_json::from_str::<Value>(&text).map_err(|e| e.to_string()));
    let checked = match parsed {
        Ok(Value::Object(map)) if host.can_write() => Ok(map),
        Ok(Value::Object(_)) => Err(t!("rune.jobs.readOnly").to_owned()),
        Ok(_) => Err(t!("rune.home.import.notExport").to_owned()),
        Err(e) => Err(e),
    };
    let mut payload = match checked {
        Ok(map) => map,
        Err(why) => return step_host(host, Event::Failed { attempt, why }, tx),
    };
    payload.insert("location".into(), json!({ "directory": target.directory }));
    host.spawn_job(ProjectsJob::ImportSession { attempt, payload });
}
