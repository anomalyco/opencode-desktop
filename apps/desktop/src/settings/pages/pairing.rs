//! Settings › Pairing (Desktop's built-in pairing plugin, `gui-plugins/src/pairing`): Pair a
//! device (a code from this machine's server, `POST /api/pair`, and the address to reach it,
//! its remote access tunnel when on), Remote access (the background service's setting, through
//! `opencode service get|set remote`, as Desktop's main process changes it) and Keep screen
//! active (the display stays awake while the app runs; saved).

use std::sync::mpsc::Sender;

use rune::{El, TextStyle, Tx, col, row, text};
use serde_json::Value;

use super::{header, page};
use crate::App;
use crate::jobs::Apply;
use crate::palette::{Elevate as _, Pal, kit};
use crate::settings::{self, setting, switch};

const CODE: TextStyle = TextStyle::new(24.0, 530.0).lh(32.0).tracking(2.0);

/// The page's state: remote access as the service reads it, and the open pairing dialog.
#[derive(Default)]
pub struct State {
    /// `None` until the service answers (or while it can't).
    pub remote: Option<bool>,
    /// A change on its way to the service.
    pub remote_busy: bool,
    pub dialog: Option<Dialog>,
    /// The display is held awake (`awake::hold`).
    pub awake: bool,
}

/// The "Pair a device" dialog.
#[derive(Clone, Debug, PartialEq)]
pub enum Dialog {
    Loading,
    Ready { code: String, link: String, expires: f64 },
    Failed(String),
}

/// Pairing is the desktop's: its own server, its background service.
pub fn available() -> bool {
    true
}

/// Reads remote access from the service.
pub fn load(s: &App) {
    if crate::headless() || cfg!(test) {
        return;
    }
    service(s, &["get", "remote"], |s, out| {
        s.settings_pages.pairing.remote = out.ok().map(|o| o.trim() == "true");
    });
}

/// Runs `opencode service <args>` off the main thread and hands its output to `done`.
fn service(s: &App, args: &'static [&'static str], done: impl FnOnce(&mut App, Result<String, String>) + Send + 'static) {
    let out: Sender<Apply> = s.jobs_sender();
    let waker = s.waker.clone();
    std::thread::spawn(move || {
        let result = std::process::Command::new("opencode").arg("service").args(args).output().map_err(|e| e.to_string()).and_then(|o| {
            if o.status.success() { Ok(String::from_utf8_lossy(&o.stdout).into_owned()) } else { Err(String::from_utf8_lossy(&o.stderr).trim().to_owned()) }
        });
        let _ = out.send(Box::new(move |s: &mut App, tx: &mut Tx| {
            tx.instant();
            done(s, result);
        }) as Apply);
        if let Some(w) = waker {
            w();
        }
    });
}

fn set_remote(s: &mut App, on: bool) {
    let st = &mut s.settings_pages.pairing;
    if st.remote_busy {
        return;
    }
    st.remote_busy = true;
    st.remote = Some(on);
    let args: &'static [&'static str] = if on { &["set", "remote", "true"] } else { &["set", "remote", "false"] };
    service(s, args, move |s, out| {
        let st = &mut s.settings_pages.pairing;
        st.remote_busy = false;
        if let Err(e) = out {
            st.remote = Some(!on);
            crate::status_toast::say(s, "Could not update remote access. Try again.", e);
        }
    });
}

/// Asks the server for a pairing code and the addresses it answers on.
fn pair(s: &mut App) {
    s.settings_pages.pairing.dialog = Some(Dialog::Loading);
    let now = s.clock;
    s.job(move |srv| {
        let code = srv.write("POST", "/api/pair", None);
        let info = srv.get("/api/info").ok();
        let base = srv.base.clone();
        Box::new(move |s: &mut App, tx: &mut Tx| {
            tx.instant();
            if s.settings_pages.pairing.dialog.is_none() {
                return;
            }
            let urls: Vec<String> = info.as_ref().and_then(|v| v["urls"].as_array()).into_iter().flatten().filter_map(Value::as_str).map(str::to_owned).collect();
            let link = urls.iter().find(|u| u.starts_with("https://")).cloned().unwrap_or(base);
            s.settings_pages.pairing.dialog = Some(match code {
                Ok(v) => match v["code"].as_str() {
                    Some(code) => Dialog::Ready { code: code.to_owned(), link, expires: now + v["expires_in"].as_f64().unwrap_or(300.0) },
                    None => Dialog::Failed("Pairing is unavailable right now.".into()),
                },
                Err(e) => Dialog::Failed(e),
            });
        }) as Apply
    });
}

/// Keeps the display awake while the setting is on.
pub fn poll(s: &mut App, _: &mut Tx) -> bool {
    let want = s.settings.keep_screen_active && s.persists();
    if want != s.settings_pages.pairing.awake {
        s.settings_pages.pairing.awake = want;
        awake::hold(want);
    }
    false
}

pub fn view(s: &App, p: &Pal) -> El {
    let st = &s.settings_pages.pairing;
    let pair_button = crate::widgets::button("Pair device", rune_kit::ButtonVariant::Neutral)
        .disabled(!s.live())
        .on_press_tx(|s: &mut App, tx: &mut Tx| {
            tx.instant();
            pair(s);
        })
        .build(kit(p))
        .key("settings-pair-device");
    let remote = switch(p, "remote-access-switch", st.remote.unwrap_or(false));
    let remote = if st.remote.is_none() || st.remote_busy { remote.disabled(true).layer_opacity(0.5) } else {
        remote.on_press(|s: &mut App| {
            let on = !s.settings_pages.pairing.remote.unwrap_or(false);
            set_remote(s, on);
        })
    };
    let rows = vec![
        ("pairing", setting(p, "pairing", "Pair a device", "Connect a phone, tablet, or another computer from anywhere through an end-to-end encrypted tunnel.", pair_button)),
        ("settings-remote-access", setting(p, "settings-remote-access", "Remote access", "Paired devices reach this computer's OpenCode server over the internet. Turning it off disconnects them until you pair them again.", remote)),
        ("settings-keep-screen-active", setting(p, "settings-keep-screen-active", "Keep screen active", "Prevent this computer\u{2019}s display from sleeping while OpenCode is running.",
            switch(p, "keep-screen-active-switch", s.settings.keep_screen_active).on_press(|s: &mut App| s.change_settings(|x| x.keep_screen_active = !x.keep_screen_active)))),
    ];
    let list = settings::list(p, "pairing-list", super::rows(s, p, 20.0, rows));
    page(p, "settings-pairing-page", header(p, "Pairing", "Connect another device to this machine's OpenCode server."), list)
}

/// The open pairing dialog over the window.
pub fn layer(s: &App, p: &Pal) -> Vec<El> {
    let Some(dialog) = &s.settings_pages.pairing.dialog else { return Vec::new() };
    let close = crate::widgets::button("Done", rune_kit::ButtonVariant::Neutral).on_press_tx(|s: &mut App, tx: &mut Tx| {
        tx.instant();
        s.settings_pages.pairing.dialog = None;
    }).build(kit(p));
    let body = match dialog {
        Dialog::Loading => col().child(crate::widgets::status_row(p, "pairing-loading", "Getting a code.")),
        Dialog::Failed(e) => col().gap(8.0).child(text("Could not update pairing details. Try again.").font(crate::widgets::BODY).color(p.text)).child(text(e.clone()).font(crate::widgets::BODY).color(p.muted).wrap()),
        Dialog::Ready { code, link, expires } => {
            let left = (expires - s.clock).max(0.0) as i64;
            let link_copy = link.clone();
            col().gap(12.0)
                .child(text(code.clone()).font(CODE).color(p.text))
                .child(text(link.clone()).font(crate::widgets::BODY).color(p.muted).wrap())
                .child(text(format!("Expires in {}:{:02}", left / 60, left % 60)).font(crate::widgets::BODY).color(p.faint))
                .child(crate::widgets::button("Copy link", rune_kit::ButtonVariant::Ghost).on_press_tx(move |_: &mut App, tx: &mut Tx| tx.copy(link_copy.clone())).build(kit(p)))
        }
    };
    let surface = col().key("pairing-dialog").w(400.0).padding(20.0).gap(16.0).radius(12.0).fill(p.bg_base).elevate(p.overlay)
        .child(text("Pair a device").font(crate::widgets::LABEL).color(p.text))
        .child(text("Scan the code with the OpenCode app on another device, or copy the link.").font(crate::widgets::BODY).color(p.muted).wrap())
        .child(body)
        .child(row().w_full().justify_end().child(close));
    crate::widgets::modal("pairing", p, surface, None)
}

/// Holding the display awake: `SetThreadExecutionState` on Windows, a `caffeinate -d` on macOS
/// and `systemd-inhibit` on Linux for as long as it's held.
mod awake {
    #[cfg(target_os = "windows")]
    #[allow(unsafe_code)]
    pub fn hold(on: bool) {
        const ES_CONTINUOUS: u32 = 0x8000_0000;
        const ES_DISPLAY_REQUIRED: u32 = 0x0000_0002;
        #[link(name = "kernel32")]
        unsafe extern "system" {
            fn SetThreadExecutionState(flags: u32) -> u32;
        }
        // The main thread holds it, and it lasts until cleared or the app ends.
        // SAFETY: plain flags in, the previous state out.
        unsafe {
            SetThreadExecutionState(if on { ES_CONTINUOUS | ES_DISPLAY_REQUIRED } else { ES_CONTINUOUS });
        }
    }

    #[cfg(not(target_os = "windows"))]
    pub fn hold(on: bool) {
        use std::process::{Child, Command};
        use std::sync::Mutex;
        static HELD: Mutex<Option<Child>> = Mutex::new(None);
        let mut held = HELD.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(mut child) = held.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
        if on {
            let pid = std::process::id().to_string();
            *held = if cfg!(target_os = "macos") {
                Command::new("caffeinate").args(["-d", "-w", &pid]).spawn().ok()
            } else {
                Command::new("systemd-inhibit").args(["--what=idle", "--who=OpenCode", "--why=Keep screen active", "sleep", "infinity"]).spawn().ok()
            };
        }
    }
}
