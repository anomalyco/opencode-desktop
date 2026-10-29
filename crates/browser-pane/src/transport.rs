//! Unix-socket sidecar launch, framing threads, and shell-shortcut filter.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use rune::{Key, KeyEvent, Mods, Waker};

use crate::installer::EngineStatus;
use crate::protocol::{EngineMsg, HostMsg};

pub(crate) fn spawn_and_attach_bg(
    bin: PathBuf,
    sock: PathBuf,
    mach_service: Option<String>,
    status: Arc<Mutex<EngineStatus>>,
    outbox: Arc<Mutex<Option<Sender<HostMsg>>>>,
    inbox_tx: Sender<EngineMsg>,
    waker: Option<Waker>,
) {
    let _ = std::fs::remove_file(&sock);
    if let Err(e) = Command::new(&bin).arg("--socket").arg(&sock).spawn() {
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

pub(crate) fn is_shell_shortcut(ev: &KeyEvent) -> bool {
    let m: Mods = ev.mods;
    if m.alt && !m.cmd && !m.ctrl && matches!(ev.key, Key::Up | Key::Down) {
        return true;
    }
    if m.ctrl && !m.cmd && !m.alt && ev.key == Key::Tab {
        return true;
    }
    match ev.key {
        Key::Command(c) => matches!(
            c.to_ascii_lowercase(),
            'k' | 'l' | 't' | 'n' | 'w' | 'b' | ',' | '[' | ']' | '1'..='9' | '=' | '+' | '-' | '_' | '0'
        ),
        _ => false,
    }
}

pub(crate) fn named_key_str(key: &Key) -> Option<&'static str> {
    match key {
        Key::Enter => Some("Enter"),
        Key::Backspace => Some("Backspace"),
        Key::Delete => Some("Delete"),
        Key::Tab => Some("Tab"),
        Key::Escape => Some("Escape"),
        Key::Left => Some("ArrowLeft"),
        Key::Right => Some("ArrowRight"),
        Key::Up => Some("ArrowUp"),
        Key::Down => Some("ArrowDown"),
        Key::Home => Some("Home"),
        Key::End => Some("End"),
        Key::PageUp => Some("PageUp"),
        Key::PageDown => Some("PageDown"),
        _ => None,
    }
}
