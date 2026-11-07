//! Standalone `OpenCode Browser Engine` CEF 156 sidecar daemon.
//!
//! Listens on a private Unix domain socket (AF_UNIX on Windows too), streams GPU frames (2x
//! Retina `IOSurface`s over Mach ports on macOS, D3D11 shared textures on Windows), survives UI
//! hot-reloads (15s reconnect grace window), and exits cleanly after 60s with 0 tabs.

#[cfg(windows)]
mod d3d_frames;
mod engine;
mod handlers;
#[cfg(target_os = "macos")]
mod mac_app;
mod profile;
mod wire;

use std::io::{BufRead, BufReader, Write};
#[cfg(unix)]
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

#[cfg(windows)]
use uds_windows::{UnixListener, UnixStream};

use crate::engine::{CefRuntime, EngineManager};
use crate::wire::{EngineMsg, HostMsg};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.iter().any(|a| a == "--print-cef-dir") {
        if let Some(dir) = option_env!("OPENCODE_CEF_DIR") {
            println!("{dir}");
        } else if let Some(dir) = cef_dll_sys::get_cef_dir() {
            println!("{}", dir.display());
        }
        return;
    }

    let sock_path = args
        .iter()
        .position(|a| a == "--socket")
        .and_then(|i| args.get(i + 1))
        .map(PathBuf::from)
        .unwrap_or_else(default_socket_path);

    if let Some(parent) = sock_path.parent() {
        let _ = std::fs::create_dir_all(parent);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700));
        }
    }
    let _ = std::fs::remove_file(&sock_path);
    let listener = UnixListener::bind(&sock_path).expect("bind browser-engine unix socket");
    listener.set_nonblocking(true).expect("set_nonblocking");

    // The app's GPU (`HIGH,LOW` LUID halves): Chromium renders on it, so its frames open there.
    let adapter_luid = args
        .iter()
        .position(|a| a == "--adapter-luid")
        .and_then(|i| args.get(i + 1))
        .and_then(|v| v.split_once(','))
        .and_then(|(high, low)| Some((high.parse::<i32>().ok()?, low.parse::<u32>().ok()?)));
    let _cef = CefRuntime::initialize(adapter_luid);

    let (cmd_tx, cmd_rx) = mpsc::channel::<HostMsg>();
    let (out_tx, out_rx) = mpsc::channel::<EngineMsg>();
    let client_writer: Arc<Mutex<Option<UnixStream>>> = Arc::new(Mutex::new(None));
    let client_connected = Arc::new(AtomicBool::new(false));

    // Outbound JSON writer thread -> connected UI process.
    {
        let writer_slot = client_writer.clone();
        let connected = client_connected.clone();
        std::thread::Builder::new()
            .name("engine-sock-tx".into())
            .spawn(move || {
                while let Ok(msg) = out_rx.recv() {
                    let Ok(mut line) = serde_json::to_string(&msg) else {
                        continue;
                    };
                    line.push('\n');
                    let mut guard = writer_slot.lock().unwrap();
                    if let Some(stream) = guard.as_mut()
                        && (stream.write_all(line.as_bytes()).is_err() || stream.flush().is_err())
                    {
                        *guard = None;
                        connected.store(false, Ordering::SeqCst);
                    }
                }
            })
            .expect("spawn engine-sock-tx");
    }

    let mut manager = EngineManager::new(out_tx, adapter_luid);
    let mut disconnected_since: Option<Instant> = Some(Instant::now());
    let mut empty_since: Option<Instant> = Some(Instant::now());

    loop {
        if let Ok((stream, _)) = listener.accept() {
            let _ = stream.set_nonblocking(false);
            if let Ok(writer_clone) = stream.try_clone() {
                *client_writer.lock().unwrap() = Some(writer_clone);
                client_connected.store(true, Ordering::SeqCst);
                disconnected_since = None;

                let tx = cmd_tx.clone();
                let connected = client_connected.clone();
                std::thread::Builder::new()
                    .name("engine-sock-rx".into())
                    .spawn(move || {
                        let reader = BufReader::new(stream);
                        for line in reader.lines() {
                            let Ok(line) = line else { break };
                            if line.trim().is_empty() {
                                continue;
                            }
                            if let Ok(msg) = serde_json::from_str::<HostMsg>(&line) {
                                let _ = tx.send(msg);
                            }
                        }
                        connected.store(false, Ordering::SeqCst);
                    })
                    .ok();
            }
        }

        let mut keep_running = true;
        while let Ok(msg) = cmd_rx.try_recv() {
            if !manager.handle(msg) {
                keep_running = false;
                break;
            }
        }
        if !keep_running {
            break;
        }

        cef::do_message_loop_work();
        #[cfg(target_os = "macos")]
        mac_app::pump_run_loop(0.004);
        manager.poll_picks();
        manager.poll_frames();

        // Reconnect grace window: if UI exited without sending Shutdown, stay up 15s for hot-swap.
        if !client_connected.load(Ordering::SeqCst) {
            let since = *disconnected_since.get_or_insert_with(Instant::now);
            if since.elapsed() > Duration::from_secs(15) {
                break;
            }
        } else {
            disconnected_since = None;
        }

        // Idle eviction: exit cleanly after 60s with 0 open tabs.
        if manager.is_empty() {
            let since = *empty_since.get_or_insert_with(Instant::now);
            if since.elapsed() > Duration::from_secs(60) {
                break;
            }
        } else {
            empty_since = None;
        }

        std::thread::sleep(Duration::from_millis(4));
    }

    let _ = std::fs::remove_file(&sock_path);
}

#[cfg(unix)]
fn default_socket_path() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
    let uid = unsafe { libc::geteuid() };
    PathBuf::from(home)
        .join("Library/Caches/co.anomaly.opencode")
        .join(format!("browser-{uid}.sock"))
}

/// The app always passes `--socket`; this is for running the engine by hand.
#[cfg(windows)]
fn default_socket_path() -> PathBuf {
    std::env::temp_dir().join("co.anomaly.opencode").join("browser.sock")
}
