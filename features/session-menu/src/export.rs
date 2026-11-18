//! All entry points share one export per (server, session), from the first page to the save
//! dialog's answer (`app/src/session/commands/export-machine.ts`). No write on cancellation.

use std::collections::HashMap;

use opencode_i18n::t;
use opencode_kit::servers::ServerKey;
use rune::Tx;

use crate::host::{SessionMenuHost, on_server};

pub type ExportTarget = (ServerKey, String);

#[derive(Clone, Copy, PartialEq, Eq)]
enum Phase {
    Fetch,
    Save,
}

enum Event {
    Request,
    Fetched,
    Finished,
}

#[derive(Default)]
pub struct Exports(HashMap<ExportTarget, Phase>);

impl Exports {
    pub fn is_exporting(&self, server: &ServerKey, id: &str) -> bool {
        !self.0.is_empty() && self.0.contains_key(&(server.clone(), id.to_owned()))
    }

    fn step(&mut self, target: &ExportTarget, event: Event) -> bool {
        match (self.0.get(target), event) {
            (None, Event::Request) => {
                self.0.insert(target.clone(), Phase::Fetch);
            }
            (Some(Phase::Fetch), Event::Fetched) => {
                self.0.insert(target.clone(), Phase::Save);
            }
            (Some(_), Event::Finished) => {
                self.0.remove(target);
            }
            _ => return false,
        }
        true
    }
}

/// Whether `(server, id)` already has an export in flight.
pub fn exporting(host: &dyn SessionMenuHost, server: &ServerKey, id: &str) -> bool {
    host.menus().exports.is_exporting(server, id)
}

/// Fetches session `id` from `server` and prompts to save (native) or downloads it (`wasm32`).
pub fn export_session_on(host: &mut dyn SessionMenuHost, server: &ServerKey, id: &str) {
    let target = (server.clone(), id.to_owned());
    if !host.menus_mut().exports.step(&target, Event::Request) {
        return;
    }
    let client = on_server(host, server, |h| h.live_server().cloned()).flatten();
    let Some(client) = client else {
        return failed(host, &target, t!("toast.session.export.failed.description").to_owned());
    };
    let Some(deliver) = host.deliverer() else {
        return failed(host, &target, t!("toast.session.export.failed.description").to_owned());
    };
    let job = target.clone();
    if let Err(e) = rune::task::spawn("session-export", move || {
        let result = client.export_session(&job.1);
        deliver(Box::new(move |h, tx| match result {
            Ok((name, json)) if h.menus_mut().exports.step(&job, Event::Fetched) => save(h, tx, job, name, json),
            Err(e) => failed(h, &job, e.to_string()),
            _ => {}
        }));
    }) {
        failed(host, &target, e.to_string());
    }
}

fn failed(host: &mut dyn SessionMenuHost, target: &ExportTarget, reason: String) {
    if host.menus_mut().exports.step(target, Event::Finished) {
        host.toast_fail(t!("toast.session.export.failed.title"), &reason);
    }
}

fn saved(host: &mut dyn SessionMenuHost, target: &ExportTarget, name: &str) {
    if host.menus_mut().exports.step(target, Event::Finished) {
        let desc = t!("toast.session.export.success.description", filename = name);
        host.toast_succeed(t!("toast.session.export.success.title"), &desc);
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn save(host: &mut dyn SessionMenuHost, tx: &mut Tx, target: ExportTarget, name: String, json: String) {
    let opts = rune::FileDialogOptions { title: t!("desktop.dialog.saveFile").into(), file_name: Some(name.clone()), ..Default::default() };
    host.save_file(
        opts,
        tx,
        Box::new(move |h, answer, _| match answer {
            Ok(None) => {
                h.menus_mut().exports.step(&target, Event::Finished);
            }
            Err(e) => failed(h, &target, e),
            Ok(Some(path)) => {
                let Some(deliver) = h.deliverer() else {
                    return failed(h, &target, t!("toast.session.export.failed.description").to_owned());
                };
                let job = target.clone();
                if let Err(e) = rune::task::spawn("session-export-save", move || {
                    let result = std::fs::write(&path, json);
                    deliver(Box::new(move |h, _| match result {
                        Ok(()) => saved(h, &job, &name),
                        Err(e) => failed(h, &job, format!("{}: {e}", path.display())),
                    }));
                }) {
                    failed(h, &target, e.to_string());
                }
            }
        }),
    );
}

#[cfg(target_arch = "wasm32")]
fn save(host: &mut dyn SessionMenuHost, _: &mut Tx, target: ExportTarget, name: String, json: String) {
    match opencode_kit::client::save_download(&name, &json) {
        Ok(_) => saved(host, &target, &name),
        Err(e) => failed(host, &target, e),
    }
}
