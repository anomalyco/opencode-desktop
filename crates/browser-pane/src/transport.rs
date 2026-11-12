//! The engine's socket (Unix domain, AF_UNIX on Windows too), launch and framing threads.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use rune::{NativeSurface, Waker};

use crate::installer::{ERR_STOPPED, EngineStatus};
use crate::profile;
use crate::protocol::{EngineMsg, HostMsg};

#[cfg(unix)]
pub(crate) use std::os::unix::net::UnixStream;
#[cfg(windows)]
pub(crate) use uds_windows::UnixStream;

/// Each tab's surface by tab id, which frames are pushed to off the UI thread.
pub(crate) type Surfaces = Arc<Mutex<HashMap<u64, NativeSurface>>>;

/// How long a new engine may take to open its socket: the first start after an install can take
/// several seconds while the OS scans the freshly unpacked files (Windows Defender does, for
/// each new DLL).
const LISTEN_TIMEOUT: Duration = Duration::from_secs(60);

/// What the app shares with the threads that talk to its engine.
#[derive(Clone)]
pub(crate) struct Link {
    pub status: Arc<Mutex<EngineStatus>>,
    pub outbox: Arc<Mutex<Option<Sender<HostMsg>>>>,
    pub inbox_tx: Sender<EngineMsg>,
    pub surfaces: Surfaces,
    pub waker: Option<Waker>,
}

impl Link {
    pub(crate) fn set(&self, status: EngineStatus) {
        *self.status.lock().unwrap() = status;
        self.wake();
    }

    fn wake(&self) {
        if let Some(w) = &self.waker {
            w();
        }
    }
}

/// The engine at `bin`, serving `sock` and keeping its sign-ins in `profile`, in the app's
/// language. On Windows it renders on the GPU Rune draws this app's windows with, so its frames
/// open there.
pub(crate) fn engine_command(bin: &Path, sock: &Path, profile: &Path) -> Command {
    let mut cmd = Command::new(bin);
    cmd.arg("--socket").arg(sock).arg("--lang").arg(chromium_locale(opencode_i18n::locale().id())).env(profile::PROFILE_ENV, profile);
    cmd.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null());
    #[cfg(windows)]
    if let Some((high, low)) = rune::native_surface::windows::adapter_luid() {
        cmd.arg("--adapter-luid").arg(format!("{high},{low}"));
    }
    cmd
}

/// Chromium's name for the app's language id, where they differ (package-windows.ps1 ships the
/// locales by the same names).
fn chromium_locale(id: &str) -> &str {
    match id {
        "en" => "en-US",
        "zh" => "zh-CN",
        "zht" => "zh-TW",
        "no" => "nb",
        other => other,
    }
}

/// Starts the engine and connects to it as soon as it listens (it says so on its output), on
/// the calling thread.
pub(crate) fn spawn_and_attach(bin: &Path, sock: &Path, profile: &Path, mach_service: Option<String>, link: &Link) {
    let _ = std::fs::remove_file(sock);
    // This app has no engine running (its socket didn't answer), so a held profile is an
    // engine on its way out, most likely the shared one from before per-app engines.
    profile::wait_until_free(profile, profile::UNLOCK_WAIT);
    let mut child = match engine_command(bin, sock, profile).spawn() {
        Ok(child) => child,
        Err(e) => return link.set(EngineStatus::Failed(format!("Couldn't start the browser: {e}").into())),
    };
    let Some(stdout) = child.stdout.take() else { return link.set(EngineStatus::Failed(ERR_STOPPED.into())) };
    let (said, heard) = mpsc::channel();
    let _ = std::thread::Builder::new().name("opencode-browser-listen".into()).spawn(move || {
        let mut line = String::new();
        let _ = said.send(BufReader::new(stdout).read_line(&mut line).is_ok_and(|n| n > 0) && line.trim() == "listening");
    });
    match heard.recv_timeout(LISTEN_TIMEOUT) {
        Ok(true) => {}
        // It exited (or hung) before it listened.
        _ => {
            let _ = child.kill();
            return link.set(EngineStatus::Failed(ERR_STOPPED.into()));
        }
    }
    rune::marks::mark("browser.engine.listening");
    match UnixStream::connect(sock) {
        Ok(stream) => wire_stream(stream, mach_service, link),
        Err(_) => link.set(EngineStatus::Failed(ERR_STOPPED.into())),
    }
}

pub(crate) fn wire_stream(stream: UnixStream, mach_service: Option<String>, link: &Link) {
    let Ok(reader_stream) = stream.try_clone() else {
        return;
    };
    let (tx, rx) = mpsc::channel::<HostMsg>();
    let _ = tx.send(HostMsg::Hello { mach_service, pid: std::process::id() });
    *link.outbox.lock().unwrap() = Some(tx);
    *link.status.lock().unwrap() = EngineStatus::Connecting;

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
        })
        .ok();

    let link = link.clone();
    std::thread::Builder::new()
        .name("rune-browser-rx".into())
        .spawn(move || {
            let reader = BufReader::new(reader_stream);
            for line in reader.lines() {
                let Ok(line) = line else { break };
                if line.trim().is_empty() {
                    continue;
                }
                let Ok(msg) = serde_json::from_str::<EngineMsg>(&line) else { continue };
                // Straight to the tab's surface, which wakes the app itself.
                if matches!(msg, EngineMsg::SharedFrame { .. } | EngineMsg::CpuFrame { .. }) && !FIRST_FRAME.swap(true, std::sync::atomic::Ordering::Relaxed) {
                    rune::marks::mark("browser.page.first_frame");
                }
                if let EngineMsg::SharedFrame { tab_id, frame_id, width, height, texture, release_fence } = msg {
                    push_shared(&link.surfaces, tab_id, frame_id, width, height, texture, release_fence);
                    continue;
                }
                let is_frame = matches!(msg, EngineMsg::CpuFrame { .. });
                let _ = link.inbox_tx.send(msg);
                if !is_frame {
                    link.wake();
                }
            }
            // The app shut the engine down (`shutdown_sidecar` empties the outbox first), or the
            // engine went away on its own: then the pane says so, and Retry starts it again.
            if link.outbox.lock().unwrap().take().is_some() {
                link.set(EngineStatus::Failed(ERR_STOPPED.into()));
            }
        })
        .ok();
}

/// The first page frame of the run, for the launch trace's `browser.page.first_frame`.
static FIRST_FRAME: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

#[cfg(windows)]
fn push_shared(surfaces: &Surfaces, tab_id: u64, frame_id: u64, width: u32, height: u32, texture: String, release_fence: String) {
    let surface = surfaces.lock().unwrap().get(&tab_id).cloned();
    if let Some(surface) = surface {
        let frame = rune::native_surface::windows::SharedTexture::named(texture, width, height)
            .release_fence(release_fence, frame_id);
        let _ = surface.push_shared(frame, frame_id);
    }
}

/// Only a Windows engine sends shared textures.
#[cfg(not(windows))]
fn push_shared(_: &Surfaces, _: u64, _: u64, _: u32, _: u32, _: String, _: String) {}
