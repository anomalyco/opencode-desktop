//! The engine's socket (Unix domain, AF_UNIX on Windows too), launch and framing threads.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use rune::{NativeSurface, Waker};

use crate::installer::EngineStatus;
use crate::profile;
use crate::protocol::{EngineMsg, HostMsg};

#[cfg(unix)]
pub(crate) use std::os::unix::net::UnixStream;
#[cfg(windows)]
pub(crate) use uds_windows::UnixStream;

/// Each tab's surface by tab id, which frames are pushed to off the UI thread.
pub(crate) type Surfaces = Arc<Mutex<HashMap<u64, NativeSurface>>>;

/// The engine at `bin`, serving `sock` and keeping its sign-ins in `profile`. On Windows it
/// renders on the GPU Rune draws this app's windows with, so its frames open there.
pub(crate) fn engine_command(bin: &Path, sock: &Path, profile: &Path) -> Command {
    let mut cmd = Command::new(bin);
    cmd.arg("--socket").arg(sock).env(profile::PROFILE_ENV, profile);
    #[cfg(windows)]
    if let Some((high, low)) = rune::native_surface::windows::adapter_luid() {
        cmd.arg("--adapter-luid").arg(format!("{high},{low}"));
    }
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
    surfaces: Surfaces,
    waker: Option<Waker>,
) {
    let _ = std::fs::remove_file(&sock);
    // This app has no engine running (its socket didn't answer), so a held profile is an
    // engine on its way out, most likely the shared one from before per-app engines.
    profile::wait_until_free(&profile, profile::UNLOCK_WAIT);
    if let Err(e) = engine_command(&bin, &sock, &profile).spawn() {
        *status.lock().unwrap() =
            EngineStatus::Failed(format!("failed to start {}: {e}", bin.display()).into());
        if let Some(w) = waker {
            w();
        }
        return;
    }
    // Up to 20 s: the first start after an install can take several, while the OS scans the
    // engine's freshly unpacked files (Windows Defender does, for each new DLL).
    for _ in 0..400 {
        if let Ok(stream) = UnixStream::connect(&sock) {
            wire_stream(stream, mach_service, &status, &outbox, inbox_tx, surfaces, waker);
            return;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    *status.lock().unwrap() =
        EngineStatus::Failed(format!("timed out waiting for {}", Path::display(&sock)).into());
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
    surfaces: Surfaces,
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
                let Ok(msg) = serde_json::from_str::<EngineMsg>(&line) else { continue };
                // Straight to the tab's surface, which wakes the app itself.
                if matches!(msg, EngineMsg::SharedFrame { .. } | EngineMsg::CpuFrame { .. }) && !FIRST_FRAME.swap(true, std::sync::atomic::Ordering::Relaxed) {
                    rune::marks::mark("browser.page.first_frame");
                }
                if let EngineMsg::SharedFrame { tab_id, frame_id, width, height, texture, release_fence } = msg {
                    push_shared(&surfaces, tab_id, frame_id, width, height, texture, release_fence);
                    continue;
                }
                let is_frame = matches!(msg, EngineMsg::CpuFrame { .. });
                let _ = inbox_tx.send(msg);
                if !is_frame
                    && let Some(w) = &waker
                {
                    w();
                }
            }
            *outbox_reader.lock().unwrap() = None;
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
