//! All entry points share one export per (server, session), from the first page to the save
//! dialog's answer (`app/src/session/commands/export-machine.ts`). No write on cancellation.

use std::collections::HashMap;
use opencode_kit::servers::ServerKey;
use opencode_i18n::t;
use rune::Tx;
use crate::{App, Source, status_toast};

type Target = (ServerKey, String);

#[derive(Clone, Copy, PartialEq)]
enum Phase { Fetch, Save }

enum Event { Request, Fetched, Finished }

#[derive(Default)]
pub(super) struct Exports(HashMap<Target, Phase>);

impl Exports {
    fn step(&mut self, target: &Target, event: Event) -> bool {
        match (self.0.get(target), event) {
            (None, Event::Request) => { self.0.insert(target.clone(), Phase::Fetch); }
            (Some(Phase::Fetch), Event::Fetched) => { self.0.insert(target.clone(), Phase::Save); }
            (Some(_), Event::Finished) => { self.0.remove(target); }
            _ => return false,
        }
        true
    }
}

impl App {
    pub(crate) fn exporting(&self, server: &ServerKey, id: &str) -> bool {
        !self.menus.exports.0.is_empty() && self.menus.exports.0.contains_key(&(server.clone(), id.to_owned()))
    }

    pub(crate) fn export_session_on(&mut self, server: &ServerKey, id: &str) {
        let target = (server.clone(), id.to_owned());
        if !self.menus.exports.step(&target, Event::Request) { return; }
        let client = crate::servers::runtime::with_server(self, server, |s| match &*s.source {
            Source::Live { server, .. } => Some(server.clone()),
            Source::Fixture => None,
        }).flatten();
        let Some(client) = client else { return failed(self, &target, t!("toast.session.export.failed.description").to_owned()); };
        let deliver = self.deliverer();
        let job = target.clone();
        if let Err(e) = rune::task::spawn("session-export", move || {
            let result = client.export_session(&job.1);
            deliver(Box::new(move |s: &mut App, tx: &mut Tx| match result {
                Ok((name, json)) if s.menus.exports.step(&job, Event::Fetched) => save(s, tx, job, name, json),
                Err(e) => failed(s, &job, e.to_string()),
                _ => {}
            }));
        }) { failed(self, &target, e.to_string()); }
    }
}

fn failed(s: &mut App, target: &Target, reason: String) {
    if s.menus.exports.step(target, Event::Finished) {
        status_toast::fail(s, t!("toast.session.export.failed.title"), reason);
    }
}

fn saved(s: &mut App, target: &Target, name: &str) {
    if s.menus.exports.step(target, Event::Finished) {
        status_toast::succeed(s, t!("toast.session.export.success.title"), t!("toast.session.export.success.description", filename = name));
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn save(_: &mut App, tx: &mut Tx, target: Target, name: String, json: String) {
    let opts = rune::FileDialogOptions { title: t!("desktop.dialog.saveFile").into(), file_name: Some(name.clone()), ..Default::default() };
    tx.save_file(opts, move |s: &mut App, answer, _| match answer {
        Ok(None) => { s.menus.exports.step(&target, Event::Finished); }
        Err(e) => failed(s, &target, e.0),
        Ok(Some(path)) => {
            let deliver = s.deliverer();
            let job = target.clone();
            if let Err(e) = rune::task::spawn("session-export-save", move || {
                let result = std::fs::write(&path, json);
                deliver(Box::new(move |s, _| match result {
                    Ok(()) => saved(s, &job, &name),
                    Err(e) => failed(s, &job, format!("{}: {e}", path.display())),
                }));
            }) { failed(s, &target, e.to_string()); }
        }
    });
}

#[cfg(target_arch = "wasm32")]
fn save(s: &mut App, _: &mut Tx, target: Target, name: String, json: String) {
    match opencode_kit::client::save_download(&name, &json) {
        Ok(_) => saved(s, &target, &name),
        Err(e) => failed(s, &target, e),
    }
}
