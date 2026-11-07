//! Desktop's built-in Pairing plugin (`packages/gui-plugins/src/pairing`) as a native plugin:
//! Settings › Pairing (Pair a device, Remote access, Keep screen active) and the Pair device
//! command. Pair a device turns remote access on, waits for this machine's server to advertise
//! its tunnel, then shows a code from it (`POST /api/pair`) as a QR code and a link, refreshed as
//! it expires. Remote access is the background service's setting, changed through `opencode
//! service` as Desktop's main process changes it, one command at a time; Keep screen active
//! holds the display awake while the app runs, and is remembered.
//!
//! A built-in: this rlib declares it, `plugins/pairing` exports it as a library, and
//! `opencode-desktop --features static-builtins` links it in.

mod awake;
mod machine;
mod page;
mod qr;
mod service;

use std::cell::Cell;
use std::collections::VecDeque;

use opencode_plugin::prelude::*;

use machine::{Effect, Event, State, Wait};

/// Whether the display is held awake, remembered across launches (`keepScreenActive`).
const KEEP_SCREEN: Store<bool> = Store::new("keepScreenActive");

/// How long "Copied" shows after the link is copied.
const COPY_FEEDBACK_MS: i64 = 2_000;

pub struct Pairing {
    /// The page's state, made when the page first draws (as Desktop's page mounts).
    state: Option<State>,
    /// The page drew without a state: the next poll makes one.
    wanted: Cell<bool>,
    waker: PluginWaker,
    /// Running attempts: when each times out (ms), and what its timeout reports.
    timers: Vec<(u64, i64, Timeout)>,
    service: service::Queue,
    awake: awake::Blocker,
    /// The link copied and until when "Copied" shows.
    copied: Option<(String, i64)>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Timeout {
    Read,
    Change,
    Tunnel,
    Code,
    Wait(Wait),
}

impl WindowPlugin for Pairing {
    fn setup(cx: &mut WindowSetupCx<Self>) -> Result<Self, PluginError> {
        let page = Contribution::settings_page("pairing", "Pairing")
            .description("Connect another device to this machine's OpenCode server.")
            .icon("outline-pair")
            .entry("pairing", "Pairing", "", "pair device qr phone remote")
            .entry(
                "settings-remote-access",
                "Remote access",
                "Paired devices reach this computer's OpenCode server over the internet. Turning it off disconnects them until you pair them again.",
                "internet tunnel opentunnel",
            )
            .entry("settings-keep-screen-active", "Keep screen active", "Prevent this computer's display from sleeping while OpenCode is running.", "display sleep awake local");
        cx.add_settings_page(page, page::page);
        cx.add_dialog(Contribution::dialog(page::DIALOG, "Pair a device"), page::dialog, |s, cx| s.dispatch(cx, Event::DialogClosed));
        cx.add_command(CommandSpec::new("open", "Pair device"), |_, cx| cx.open_settings("pairing"));
        let mut awake = awake::Blocker::default();
        // The display stays awake from launch when the setting is on (`main.ts`).
        if KEEP_SCREEN.get() == Some(true) {
            awake.hold_at_setup(cx);
        }
        let waker = cx.waker();
        Ok(Pairing { state: None, wanted: Cell::new(false), waker, timers: Vec::new(), service: service::Queue::default(), awake, copied: None })
    }

    /// Makes the page's state once it drew, ends what timed out, and ticks the dialog's countdown.
    fn poll(&mut self, cx: &mut PollCx) -> bool {
        let mut changed = false;
        if self.state.is_none() && self.wanted.get() {
            let (state, effects) = State::new(true);
            self.state = Some(state);
            self.start(cx, effects);
            changed = true;
        }
        let now = cx.now_ms();
        let due: Vec<(u64, Timeout)> = self.timers.iter().filter(|(_, at, _)| *at <= now).map(|(a, _, t)| (*a, *t)).collect();
        self.timers.retain(|(_, at, _)| *at > now);
        changed |= !due.is_empty();
        for (attempt, timeout) in due {
            let event = match timeout {
                Timeout::Read => Event::ReadDone { attempt, enabled: None },
                Timeout::Change => Event::ChangeDone { attempt, ok: false },
                Timeout::Tunnel => Event::TunnelMissing { attempt },
                Timeout::Code => Event::CodeFailed { attempt },
                Timeout::Wait(kind) => Event::WaitEnded { kind, attempt },
            };
            self.dispatch(cx, event);
        }
        if self.state.as_ref().is_some_and(|s| matches!(s.dialog, machine::Dialog::Ready { .. })) {
            self.dispatch(cx, Event::Tick { now });
            changed = true;
        }
        if self.copied.as_ref().is_some_and(|(_, until)| *until <= now) {
            self.copied = None;
            changed = true;
        }
        self.schedule(cx);
        changed
    }
}

impl Pairing {
    /// The page drew: the first time, its state is made on the next poll.
    pub(crate) fn want_state(&self) {
        if self.state.is_none() && !self.wanted.replace(true) {
            self.waker.wake();
        }
    }

    pub(crate) fn dispatch(&mut self, cx: &mut ActionCx, event: Event) {
        let Some(state) = self.state.as_mut() else { return };
        let effects = state.apply(event);
        self.start(cx, effects);
    }

    /// Runs `effects`, and the effects of the events some answer at once.
    fn start(&mut self, cx: &mut ActionCx, effects: Vec<Effect>) {
        let now = cx.now_ms();
        let mut queue: VecDeque<Effect> = effects.into();
        while let Some(effect) = queue.pop_front() {
            let mut at_once = |s: &mut Pairing, event: Event| {
                if let Some(state) = s.state.as_mut() {
                    queue.extend(state.apply(event));
                }
            };
            match effect {
                Effect::Abort(attempt) => self.timers.retain(|(a, ..)| *a != attempt),
                Effect::OpenDialog => cx.open_dialog(page::DIALOG),
                Effect::Wait { kind, attempt, delay } => self.timers.push((attempt, now + delay, Timeout::Wait(kind))),
                Effect::ReadRemote { attempt, timeout } => {
                    self.timers.push((attempt, now + timeout, Timeout::Read));
                    self.service.push(cx, service::Job::Read { attempt });
                }
                Effect::SetRemote { attempt, enabled, restart, timeout } => {
                    self.timers.push((attempt, now + timeout, Timeout::Change));
                    self.service.push(cx, service::Job::Set { attempt, enabled, restart });
                }
                // The blocker is the plugin's own, so what it holds answers at once.
                Effect::ReadScreen { attempt, .. } => {
                    let held = self.awake.held();
                    at_once(self, Event::ScreenReadDone { attempt, enabled: Some(held) });
                }
                Effect::SetScreen { attempt, enabled, .. } => {
                    let ok = self.awake.hold(cx, enabled);
                    if ok {
                        KEEP_SCREEN.set(&enabled);
                    }
                    at_once(self, Event::ScreenChangeDone { attempt, ok });
                }
                Effect::PollTunnel { attempt, timeout } => {
                    self.timers.push((attempt, now + timeout, Timeout::Tunnel));
                    cx.server_request("GET", "local:/api/info", "", move |s: &mut Pairing, cx| {
                        let v: serde_json::Value = serde_json::from_str(cx.input()).unwrap_or_default();
                        let urls = v.get("urls").or_else(|| v["data"].get("urls")).and_then(|u| u.as_array()).cloned().unwrap_or_default();
                        // The server listens over plain HTTP, so its HTTPS address is its tunnel.
                        let tunnel = urls.iter().filter_map(|u| u.as_str()).find_map(origin_https);
                        let event = match tunnel {
                            Some(tunnel) if cx.input_extra().is_empty() => Event::TunnelFound { attempt, tunnel },
                            _ => Event::TunnelMissing { attempt },
                        };
                        s.dispatch(cx, event);
                    });
                }
                Effect::FetchCode { attempt, timeout } => {
                    self.timers.push((attempt, now + timeout, Timeout::Code));
                    cx.server_request("POST", "local:/api/pair", "", move |s: &mut Pairing, cx| {
                        let v: serde_json::Value = serde_json::from_str(cx.input()).unwrap_or_default();
                        let v = if v.get("code").is_some() { v } else { v["data"].clone() };
                        let event = match (cx.input_extra(), v["code"].as_str()) {
                            ("", Some(code)) => Event::CodeLoaded { attempt, code: code.to_owned(), expires_in: v["expires_in"].as_i64().unwrap_or(300), now: cx.now_ms() },
                            _ => Event::CodeFailed { attempt },
                        };
                        s.dispatch(cx, event);
                    });
                }
            }
        }
        self.schedule(cx);
    }

    fn schedule(&self, cx: &mut ActionCx) {
        let now = cx.now_ms();
        let timers = self.timers.iter().map(|(_, at, _)| *at).min();
        let tick = self.state.as_ref().is_some_and(|s| matches!(s.dialog, machine::Dialog::Ready { .. })).then_some(now + 1000);
        let copied = self.copied.as_ref().map(|(_, until)| *until);
        if let Some(at) = [timers, tick, copied].into_iter().flatten().min() {
            cx.wake_after((at - now).max(0) as u64);
        }
    }

    pub(crate) fn service_done(&mut self, cx: &mut ActionCx, done: service::Done) {
        let event = match done {
            service::Done::Read { attempt, enabled } => Event::ReadDone { attempt, enabled },
            service::Done::Set { attempt, ok } => Event::ChangeDone { attempt, ok },
        };
        self.dispatch(cx, event);
        self.service.next(cx);
    }

    pub(crate) fn copy_link(&mut self, cx: &mut ActionCx, url: &str) {
        cx.copy_text(url);
        self.copied = Some((url.to_owned(), cx.now_ms() + COPY_FEEDBACK_MS));
        self.schedule(cx);
    }
}

/// `https://host[:port]` of `url`, when it's an HTTPS address.
fn origin_https(url: &str) -> Option<String> {
    let rest = url.strip_prefix("https://")?;
    let host = rest.split(['/', '?', '#']).next().filter(|h| !h.is_empty())?;
    Some(format!("https://{host}"))
}

opencode_plugin::declare_plugin!(window = Pairing);
