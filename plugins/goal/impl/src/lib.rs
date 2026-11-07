//! Desktop's built-in Goal plugin (`packages/gui-plugins/src/goal`) as a native plugin: the
//! session's `/goal` as a pill above its composer, Goal, Goal paused or Goal met, and a popover
//! headed by the objective with Pause, Resume and Clear. The server's goal plugin owns the goal
//! and serves it over its RPC (`opencode.goal`): `get` reads it, `changed` keeps it current, and
//! the actions run `/goal pause|resume|clear` as the session's command.
//!
//! A built-in: this rlib declares it, `plugins/goal` exports it as a library, and
//! `opencode-desktop --features static-builtins` links it in.

mod machine;

use std::cell::RefCell;

use opencode_plugin::abi::BUTTON_GHOST;
use opencode_plugin::prelude::*;

use machine::{Action, COMMAND_TIMEOUT_MS, Effect, SNAPSHOT_TIMEOUT_MS, State, Status};

/// What a timer ends.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Timer {
    Snapshot,
    Retry,
    Command(String),
}

pub struct GoalPlugin {
    state: State,
    /// The routed session and its folder, as the pill last saw it; the next poll attaches to it.
    seen: RefCell<Option<(String, String)>>,
    waker: PluginWaker,
    /// Running timers: attempt, when it's due (ms) and what it ends.
    timers: Vec<(u64, i64, Timer)>,
}

impl WindowPlugin for GoalPlugin {
    fn setup(cx: &mut WindowSetupCx<Self>) -> Result<Self, PluginError> {
        let menu: Box<dyn Fn(&GoalPlugin, &mut ViewCx<GoalPlugin>, &Props) -> El> = Box::new(menu);
        cx.add_composer_pill_view(Contribution::composer_pill("goal"), show, Some(menu), |_, _| {});
        cx.add_server_events(Contribution::server_events("changed", "rpc.opencode.goal.changed"), |s, cx, _, data| {
            let v: serde_json::Value = serde_json::from_str(data).unwrap_or_default();
            let v = if v.get("sessionID").is_some() { v } else { v["data"].clone() };
            let Some(session) = v["sessionID"].as_str() else { return cx.unchanged() };
            let effects = s.state.changed(session, machine::Goal::parse(&v["goal"]));
            s.run(cx, effects);
        });
        let waker = cx.waker();
        Ok(GoalPlugin { state: State::default(), seen: RefCell::new(None), waker, timers: Vec::new() })
    }

    /// Attaches to the session the pill was drawn for, and ends timers that came due.
    fn poll(&mut self, cx: &mut PollCx) -> bool {
        let mut changed = false;
        let seen = self.seen.borrow().clone();
        if let Some((key, directory)) = seen
            && self.state.target().is_none_or(|t| t.key != key || t.directory != directory)
        {
            let effects = self.state.attach(&key, &directory);
            self.run(cx, effects);
            changed = true;
        }
        let now = cx.now_ms();
        let due: Vec<(u64, Timer)> = self.timers.iter().filter(|(_, at, _)| *at <= now).map(|(a, _, t)| (*a, t.clone())).collect();
        self.timers.retain(|(_, at, _)| *at > now);
        for (attempt, timer) in due {
            changed = true;
            let effects = match timer {
                Timer::Snapshot => self.state.snapshot_failed(attempt),
                Timer::Retry => self.state.retry_due(attempt),
                Timer::Command(key) => self.state.command_settled(&key, attempt, Some(None)),
            };
            self.run(cx, effects);
        }
        self.schedule(cx);
        changed
    }
}

/// The pill for the session (`ComposerPill.show`): the goal's status, its objective as the title.
fn show(s: &GoalPlugin, props: &Props) -> Option<PillView> {
    let routed = (props.session_id.clone(), props.directory.clone());
    if s.seen.borrow().as_ref() != Some(&routed) {
        *s.seen.borrow_mut() = Some(routed);
        s.waker.wake();
    }
    let goal = s.state.goal(&props.session_id)?;
    let (label, icon, tone) = match goal.status {
        Status::Active => ("Goal", "outline-steer", None),
        Status::Paused => ("Goal paused", "circle-exclamation", Some(Tone::Warning)),
        Status::Complete => ("Goal met", "circle-check", Some(Tone::Success)),
    };
    Some(PillView { label: label.into(), icon: icon.into(), tone, title: goal.objective.clone() })
}

/// The actions under the objective (`flex justify-end gap-1 px-1 pt-1`): Pause while active,
/// Resume while paused, and Clear, each waiting while one runs.
fn menu(s: &GoalPlugin, cx: &mut ViewCx<GoalPlugin>, props: &Props) -> El {
    let key = props.session_id.clone();
    let status = s.state.goal(&key).map(|g| g.status);
    let busy = s.state.busy(&key);
    let button = |cx: &mut ViewCx<GoalPlugin>, label: &'static str, icon: &'static str, action: Action| {
        let key = key.clone();
        ui::button_with(label, BUTTON_GHOST).small().lead_icon(icon).disabled(busy).key(format!("goal-{}", action.name())).on_press(cx.handler(move |s: &mut GoalPlugin, cx| {
            cx.close_popover();
            let effects = s.state.command(&key, action);
            s.run(cx, effects);
        }))
    };
    let mut r = row().key("goal-actions").w_full().gap(4.0).px(4.0).pt(4.0).child(spacer());
    if status == Some(Status::Active) {
        r = r.child(button(cx, "Pause", "stop", Action::Pause));
    }
    if status == Some(Status::Paused) {
        r = r.child(button(cx, "Resume", "refresh", Action::Resume));
    }
    r.child(button(cx, "Clear", "outline-trash", Action::Clear))
}

impl GoalPlugin {
    fn run(&mut self, cx: &mut ActionCx, effects: Vec<Effect>) {
        let now = cx.now_ms();
        for effect in effects {
            match effect {
                Effect::Cancel(attempt) => self.timers.retain(|(a, ..)| *a != attempt),
                Effect::ToastError(message) => cx.toast_error("Request failed", message.as_deref().unwrap_or("")),
                Effect::ScheduleRetry { attempt, delay } => self.timers.push((attempt, now + delay, Timer::Retry)),
                Effect::Fetch { attempt, session, directory } => {
                    self.timers.push((attempt, now + SNAPSHOT_TIMEOUT_MS, Timer::Snapshot));
                    let path = format!("/api/rpc/opencode.goal/get?location%5Bdirectory%5D={}", encode(&directory));
                    let body = serde_json::json!({ "input": { "sessionID": session } }).to_string();
                    cx.server_request("POST", &path, &body, move |s: &mut GoalPlugin, cx| {
                        if !s.timers.iter().any(|(a, ..)| *a == attempt) {
                            return cx.unchanged();
                        }
                        s.timers.retain(|(a, ..)| *a != attempt);
                        let effects = match cx.input_extra() {
                            "" => {
                                let v: serde_json::Value = serde_json::from_str(cx.input()).unwrap_or_default();
                                let output = v.get("output").or_else(|| v["data"].get("output")).cloned().unwrap_or_default();
                                s.state.snapshot_loaded(attempt, machine::Goal::parse(&output))
                            }
                            _ => s.state.snapshot_failed(attempt),
                        };
                        s.run(cx, effects);
                    });
                }
                Effect::Command { attempt, session, action } => {
                    self.timers.push((attempt, now + COMMAND_TIMEOUT_MS, Timer::Command(session.clone())));
                    let body = serde_json::json!({ "name": "goal", "text": action.name() }).to_string();
                    let key = session.clone();
                    cx.server_request("POST", &format!("/api/session/{session}/command"), &body, move |s: &mut GoalPlugin, cx| {
                        s.timers.retain(|(a, ..)| *a != attempt);
                        let failed = match cx.input_extra() {
                            "" => None,
                            e => Some(Some(e.to_owned())),
                        };
                        let effects = s.state.command_settled(&key, attempt, failed);
                        s.run(cx, effects);
                    });
                }
            }
        }
        self.schedule(cx);
    }

    fn schedule(&self, cx: &mut ActionCx) {
        if let Some(next) = self.timers.iter().map(|(_, at, _)| *at).min() {
            cx.wake_after((next - cx.now_ms()).max(0) as u64);
        }
    }
}

fn encode(s: &str) -> String {
    s.bytes().map(|b| if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) { (b as char).to_string() } else { format!("%{b:02X}") }).collect()
}

opencode_plugin::declare_plugin!(window = GoalPlugin);
