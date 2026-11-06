//! Unix-socket sidecar launch and framing threads.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use rune::Waker;

use crate::installer::EngineStatus;
use crate::profile;
use crate::protocol::{EngineMsg, HostMsg};

/// The engine at `bin`, serving `sock` and keeping its sign-ins in `profile`.
pub(crate) fn engine_command(bin: &Path, sock: &Path, profile: &Path) -> Command {
    let mut cmd = Command::new(bin);
    cmd.arg("--socket").arg(sock).env(profile::PROFILE_ENV, profile);
    cmd
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn spawn_and_attach_bg(
    bin: PathBuf,
    sock: PathBuf,
    profile: PathBuf,
    mach_service: Option<String>,
    status: Arc<Mutex<EngineStatus>>,
    outbox: Arc<Mutex<Option<Sender<HostMsg>>>>,
    inbox_tx: Sender<EngineMsg>,
    waker: Option<Waker>,
) {
    let _ = std::fs::remove_file(&sock);
    // This app has no engine running (its socket didn't answer), so a held profile is an
    // engine on its way out, most likely the shared one from before per-app engines.
    profile::wait_until_free(&profile, profile::UNLOCK_WAIT);
    if let Err(e) = engine_command(&bin, &sock, &profile).spawn() {
        *status.lock().unwrap() =
            EngineStatus::Failed(format!("failed to start {}: {e}", bin.display()));
        if let Some(w) = waker {
            w();
        }
        return;
    }
    for _ in 0..80 {
        if let Ok(stream) = UnixStream::connect(&sock) {
            wire_stream(stream, mach_service, &status, &outbox, inbox_tx, waker);
            return;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    *status.lock().unwrap() =
        EngineStatus::Failed(format!("timed out waiting for {}", Path::display(&sock)));
    if let Some(w) = waker {
        w();
    }
}

pub(crate) fn wire_stream(
    stream: UnixStream,
    mach_service: Option<String>,
    status: &Arc<Mutex<EngineStatus>>,
    outbox: &Arc<Mutex<Option<Sender<HostMsg>>>>,
    inbox_tx: Sender<EngineMsg>,
    waker: Option<Waker>,
) {
    let Ok(reader_stream) = stream.try_clone() else {
        return;
    };
    let (tx, rx) = mpsc::channel::<HostMsg>();
    let _ = tx.send(HostMsg::Hello { mach_service, pid: std::process::id() });
    *outbox.lock().unwrap() = Some(tx);
    *status.lock().unwrap() = EngineStatus::Connecting;

    let outbox_writer = outbox.clone();
    std::thread::Builder::new()
        .name("rune-browser-tx".into())
        .spawn(move || {
            let mut writer = stream;
            while let Ok(msg) = rx.recv() {
                let is_shutdown = matches!(msg, HostMsg::Shutdown);
                if let Ok(mut line) = serde_json::to_string(&msg) {
                    line.push('\n');
                    if writer.write_all(line.as_bytes()).is_err() {
                        break;
                    }
                    let _ = writer.flush();
                }
                if is_shutdown {
                    break;
                }
            }
            *outbox_writer.lock().unwrap() = None;
        })
        .ok();

    let outbox_reader = outbox.clone();
    std::thread::Builder::new()
        .name("rune-browser-rx".into())
        .spawn(move || {
            let reader = BufReader::new(reader_stream);
            for line in reader.lines() {
                let Ok(line) = line else { break };
                if line.trim().is_empty() {
                    continue;
                }
                if let Ok(msg) = serde_json::from_str::<EngineMsg>(&line) {
                    let is_frame = matches!(msg, EngineMsg::CpuFrame { .. });
                    let _ = inbox_tx.send(msg);
                    if !is_frame
                        && let Some(w) = &waker
                    {
                        w();
                    }
                }
            }
            *outbox_reader.lock().unwrap() = None;
        })
        .ok();
}
