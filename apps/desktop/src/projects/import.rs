//! Import session: a session export (JSON) picked from disk, imported into a project through
//! `experimental/session/import`: Desktop's `ProjectImportState`
//! (`home/projects/import-machine.ts:15-19`) and its driver (`home/projects/actions.tsx:61-152`).
//! One import at a time; while the transcript uploads, a toast says so; the imported session
//! opens only while the window still shows where the import was asked from.

use opencode_kit::model::SessionInfo;
use opencode_kit::servers::ServerKey;
use opencode_i18n::t;
use rune::Tx;
use rune::machine::{Attempt, Effects, Seq};

use crate::{App, Route};

/// `ProjectImportTarget`: where the import was asked from.
#[derive(Clone, Debug, PartialEq)]
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
    Picked { attempt: Attempt, file: Option<std::path::PathBuf> },
    /// `picker.failed`.
    PickerFailed { attempt: Attempt, why: String },
    /// `import.succeeded`; `here` is where the window is now.
    Imported { attempt: Attempt, session: SessionInfo, here: Route },
    /// `import.failed`.
    Failed { attempt: Attempt, why: String },
}

#[derive(Debug)]
pub enum Effect {
    /// `picker.open`.
    Pick { attempt: Attempt },
    /// `toast.pending.show` and `upload.start`.
    Upload { attempt: Attempt, target: Target, file: std::path::PathBuf },
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
                let land = if here == target.origin { Effect::Open { target, session } } else { Effect::Remember { session } };
                Effects::two(Effect::Dismiss(toast), land)
            }
            _ => Effects::none(),
        }
    }

    /// The upload started with its pending toast: the machine holds both.
    fn uploading(&mut self, attempt: Attempt, target: Target, toast: u64) {
        self.state = State::Importing { attempt, target, toast };
    }
}

impl App {
    /// Import session into `directory` of the active server (`importSession`).
    pub(crate) fn import_session(&mut self, directory: &str, tx: &mut Tx) {
        let target = Target { server: crate::servers::runtime::active(self).clone(), directory: directory.to_owned(), origin: self.route.clone() };
        self.import_step(Event::Requested(target), tx);
    }

    fn import_step(&mut self, event: Event, tx: &mut Tx) {
        for effect in self.project_state.import.step(event) {
            match effect {
                Effect::Pick { attempt } => {
                    let options = rune::FileDialogOptions { title: t!("command.session.import").into(), ..Default::default() }.filter("JSON", &["json"]);
                    tx.open_file(options, move |s: &mut App, result, tx: &mut Tx| {
                        let event = match result {
                            Ok(file) => Event::Picked { attempt, file },
                            Err(e) => Event::PickerFailed { attempt, why: e.to_string() },
                        };
                        s.import_step(event, tx);
                    });
                }
                Effect::Upload { attempt, target, file } => {
                    let toast = crate::status_toast::waiting(self, t!("toast.session.import.pending"));
                    self.project_state.import.uploading(attempt, target.clone(), toast);
                    tx.instant();
                    self.upload_session(attempt, target, &file, tx);
                }
                Effect::Dismiss(toast) => crate::status_toast::dismiss_id(self, toast),
                Effect::Fail(why) => crate::status_toast::fail(self, t!("common.requestFailed"), why),
                Effect::Open { target, session } => {
                    let id = session.id.clone();
                    self.remember_session(session);
                    crate::servers::runtime::with_server(self, &target.server, |s| s.open(&id, tx));
                }
                Effect::Remember { session } => self.remember_session(session),
            }
        }
        tx.instant();
    }

    fn remember_session(&mut self, session: SessionInfo) {
        if !self.sessions.iter().any(|x| x.id == session.id) {
            self.sessions.insert(0, session);
        }
    }

    /// Reads the export and imports it into `target`'s folder on a worker; what it reads that
    /// isn't an export fails the import, as a server rejecting it does.
    fn upload_session(&mut self, attempt: Attempt, target: Target, path: &std::path::Path, tx: &mut Tx) {
        let data = std::fs::read_to_string(path).map_err(|e| e.to_string()).and_then(|t| serde_json::from_str::<serde_json::Value>(&t).map_err(|e| e.to_string()));
        let data = match data {
            Ok(serde_json::Value::Object(map)) if self.can_write() => Ok(map),
            Ok(serde_json::Value::Object(_)) => Err(crate::jobs::read_only().to_owned()),
            Ok(_) => Err(t!("rune.home.import.notExport").to_owned()),
            Err(e) => Err(e),
        };
        let mut data = match data {
            Ok(map) => map,
            Err(why) => return self.import_step(Event::Failed { attempt, why }, tx),
        };
        data.insert("location".into(), serde_json::json!({ "directory": target.directory }));
        self.job(move |srv| {
            let made = srv.write("POST", "/api/experimental/session/import", Some(serde_json::Value::Object(data)));
            Box::new(move |s: &mut App, tx: &mut Tx| {
                let event = match made {
                    Ok(v) => Event::Imported { attempt, session: opencode_kit::model::parse_session(v.get("data").unwrap_or(&v)), here: s.route.clone() },
                    Err(why) => Event::Failed { attempt, why },
                };
                s.import_step(event, tx);
            }) as crate::jobs::Apply
        });
    }
}
