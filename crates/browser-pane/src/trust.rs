//! The trust rule's check of the unpacked files, before anything of them runs. A hash rule
//! (`Trust::Sha256`, `Trust::Local`) is already met by the archive's SHA-256; a publisher rule
//! checks every program and library: on Windows each one's Authenticode signature as it
//! unpacks (on a thread of its own, so it overlaps the rest of the download), on macOS the
//! bundle's Developer ID signature once it's whole.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Sender};
use std::thread::JoinHandle;

use crate::installer::{Distrust, Fault};
use crate::source::Trust;

pub(crate) struct Verifier {
    trust: Trust,
    queue: Option<Sender<PathBuf>>,
    worker: Option<JoinHandle<Result<(), Fault>>>,
}

impl Verifier {
    pub(crate) fn new(trust: &Trust) -> Self {
        let (queue, worker) = match trust {
            Trust::Publisher(name) if cfg!(windows) => {
                let (tx, rx) = mpsc::channel::<PathBuf>();
                let name = name.clone();
                let worker = std::thread::Builder::new()
                    .name("opencode-browser-verify".into())
                    .spawn(move || rx.into_iter().try_for_each(|file| signed_by(&file, &name)))
                    .ok();
                (worker.is_some().then_some(tx), worker)
            }
            _ => (None, None),
        };
        Self { trust: trust.clone(), queue, worker }
    }

    /// A file is whole: a publisher rule checks it if it's a program or library.
    pub(crate) fn file(&self, path: &Path) {
        let code = path.extension().and_then(|e| e.to_str()).is_some_and(|e| e.eq_ignore_ascii_case("exe") || e.eq_ignore_ascii_case("dll"));
        if let Some(queue) = self.queue.as_ref().filter(|_| code) {
            let _ = queue.send(path.to_owned());
        }
    }

    /// Every file is in `bundle`: the rule's answer.
    pub(crate) fn finish(mut self, bundle: &Path) -> Result<(), Fault> {
        drop(self.queue.take());
        if let Some(worker) = self.worker.take() {
            return worker.join().unwrap_or(Err(Fault::Untrusted(Distrust::Rule)));
        }
        match &self.trust {
            Trust::Publisher(team) if cfg!(target_os = "macos") => developer_id(bundle, team),
            Trust::Publisher(_) => Err(Fault::Untrusted(Distrust::Rule)),
            Trust::Local | Trust::Sha256(_) => Ok(()),
        }
    }
}

#[cfg(windows)]
fn signed_by(file: &Path, name: &str) -> Result<(), Fault> {
    let shown = file.file_name().map_or_else(|| file.display().to_string(), |n| n.to_string_lossy().into_owned());
    match authenticode::signer(file) {
        Ok(signer) if signer == name => Ok(()),
        Ok(signer) => Err(Fault::Untrusted(Distrust::Signer { file: shown, signer })),
        Err(_) => Err(Fault::Untrusted(Distrust::Unsigned { file: shown })),
    }
}

#[cfg(not(windows))]
fn signed_by(_file: &Path, _name: &str) -> Result<(), Fault> {
    Err(Fault::Untrusted(Distrust::Rule))
}

/// macOS: the bundle and everything in it is signed by Developer ID team `team`, unaltered.
fn developer_id(bundle: &Path, team: &str) -> Result<(), Fault> {
    let requirement = format!("=anchor apple generic and certificate leaf[subject.OU] = \"{team}\"");
    let ok = std::process::Command::new("/usr/bin/codesign")
        .args(["--verify", "--deep", "--strict", "-R"])
        .arg(requirement)
        .arg(bundle)
        .status()
        .is_ok_and(|s| s.success());
    if ok { Ok(()) } else { Err(Fault::Untrusted(Distrust::Unsigned { file: crate::installer::ENGINE_BUNDLE_NAME.to_owned() })) }
}
