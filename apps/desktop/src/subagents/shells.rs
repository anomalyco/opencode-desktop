//! A session's background shells, as Desktop's running-work capsule lists them
//! (`activity-chips.tsx`, `requests/background.ts`): the server's running shells whose
//! `metadata.sessionID` is the session and that no running call of the session's current turn
//! holds, from `GET /api/shell` and the `shell.*` events. The list itself is the machine in
//! `activity.rs`; this runs its effects: Stops, end lookups and output reads on background
//! threads, and its timers on the app clock.

use std::collections::HashSet;
use std::sync::{Arc, Mutex};

use opencode_kit::model::{Message, Part, ToolStatus};
use rune::Tx;
use serde_json::Value;

use super::activity::{self, Activity, Effect, EndInfo, EndStatus, Event, Timer, Work};
use super::shell_follow::{self, Followers};
use super::shell_tail::{Page, TailEffect, TailEvent};
use crate::{App, Route, Source};

/// A running shell the server lists (`ShellInfo`).
#[derive(Clone, Debug, PartialEq)]
pub struct Shell {
    pub id: String,
    pub session: String,
    pub command: String,
    /// When it started, epoch ms.
    pub started: i64,
    /// The location it runs in, which its reads name.
    pub directory: String,
}

/// What a background request brought back, for the shell or attempt it was for.
enum Landed {
    Listed(String, Vec<Shell>),
    Output { id: String, attempt: u64, page: Result<Page, bool> },
    Ended { attempt: u64, info: Option<EndInfo> },
    Stopped { attempt: u64, ok: bool },
    /// A read for a timeline console's follower (`shell_follow`).
    Follow { id: String, generation: u64, attempt: u64, page: Result<Page, bool> },
}

/// A timer the list's machine started, due on the app clock (ms).
#[derive(Clone, Debug)]
enum Due {
    Machine { attempt: u64, timer: Timer },
    TailWait { attempt: u64 },
    FollowWait { id: String, generation: u64, attempt: u64 },
}

#[derive(Default)]
pub struct Shells {
    running: Vec<Shell>,
    listed: HashSet<String>,
    pub activity: Activity,
    /// The timeline consoles' followers.
    pub follow: Followers,
    /// The work the machine last heard of, so a change is told to it once.
    work: Work,
    timers: Vec<(i64, Due)>,
    landed: Arc<Mutex<Vec<Landed>>>,
    /// The second the run times last showed.
    ticked: i64,
}

/// The running shells of session `id` in the capsule, oldest first: those no running call of its
/// current turn holds (by shell id, or by command before the call names one).
pub fn running_for<'a>(s: &'a App, id: &str) -> Vec<&'a Shell> {
    let held: Vec<(Option<&str>, &str)> = s
        .transcripts
        .get(id)
        .and_then(|t| t.messages.iter().rev().find_map(|m| match m {
            Message::Assistant(a) if a.completed.is_none() => Some(a),
            _ => None,
        }))
        .map_or_else(Vec::new, |a| a.parts.iter().filter_map(|p| match p {
            Part::Tool { name, status: ToolStatus::Running, input, metadata, .. } if name == "shell" => Some((metadata["shellID"].as_str(), input["command"].as_str().unwrap_or(""))),
            _ => None,
        }).collect());
    let mut out: Vec<&Shell> = s.shells.running.iter()
        .filter(|sh| sh.session == id && !held.iter().any(|(sid, cmd)| *sid == Some(sh.id.as_str()) || (!cmd.is_empty() && *cmd == sh.command)))
        .collect();
    out.sort_by(|a, b| (a.started, &a.id).cmp(&(b.started, &b.id)));
    out
}

/// What runs in the open session's background now, as the machine sees it.
pub fn work(s: &App) -> Work {
    let Route::Session { id, .. } = &*s.route else { return Work::default() };
    Work {
        enabled: s.settings.background_activity != crate::settings::BackgroundActivity::Off,
        subagents: s.running_subagents(id).into_iter().map(|r| r.session).collect(),
        shells: running_for(s, id).into_iter().map(|sh| sh.id.clone()).collect(),
    }
}

/// Whether the capsule counts the shown shell as well: one that ended while its panel is open.
pub fn held(s: &App) -> bool {
    activity::held(&s.shells.activity, &work(s))
}

/// Tells the list's machine `event` and runs what it asks for.
pub fn dispatch(s: &mut App, event: Event) {
    let work = work(s);
    s.shells.work = work.clone();
    let state = std::mem::take(&mut s.shells.activity);
    let (next, effects) = activity::step(state, event, &work);
    s.shells.activity = next;
    for e in effects {
        run(s, e);
    }
}

/// Opens or shuts the list.
pub fn set_open(s: &mut App, open: bool) {
    dispatch(s, Event::Toggled(open));
}

/// Reads the running shells of `directory`, once per location until the stream reconnects.
pub fn list(s: &mut App, directory: &str) {
    if directory.is_empty() || !s.shells.listed.insert(directory.to_owned()) {
        return;
    }
    let Source::Live { server, .. } = &*s.source else { return };
    let path = format!("/api/shell?{}", location(directory));
    if crate::headless() {
        if let Ok(v) = server.read(&path) {
            let shells = parse_list(&v, directory);
            replace(&mut s.shells, directory, shells);
        }
        return;
    }
    let (server, landed, wake, dir) = (server.clone(), Arc::clone(&s.shells.landed), (*s.waker).clone(), directory.to_owned());
    spawn(move || {
        if let Ok(v) = server.get(&path) {
            let shells = parse_list(&v, &dir);
            push(&landed, wake.as_ref(), Landed::Listed(dir, shells));
        }
    });
}

/// After a reconnect every location is read again.
pub fn forget_lists(s: &mut App) {
    s.shells.listed.clear();
}

/// `shell.created`, `shell.exited` and `shell.deleted`.
pub fn event(s: &mut App, ty: &str, data: &Value) -> bool {
    match ty {
        "shell.created" => {
            let info = &data["info"];
            let session = info["metadata"]["sessionID"].as_str().unwrap_or("");
            let directory = data["location"]["directory"].as_str().map(str::to_owned)
                .or_else(|| s.session(session).map(|i| i.directory.clone()))
                .unwrap_or_default();
            let Some(sh) = parse(info, &directory).filter(|_| info["status"] == "running") else { return false };
            s.shells.running.retain(|r| r.id != sh.id);
            s.shells.running.push(sh);
            true
        }
        "shell.exited" | "shell.deleted" => {
            let Some(id) = data["id"].as_str() else { return false };
            let before = s.shells.running.len();
            s.shells.running.retain(|r| r.id != id);
            s.shells.running.len() != before
        }
        _ => false,
    }
}

/// Takes what landed, fires due timers, tells the machine when the work changed, and ticks the
/// run times each second while the list shows them.
pub fn poll(s: &mut App, tx: &mut Tx) -> bool {
    let before = s.shells.activity.clone();
    let landed = std::mem::take(&mut *s.shells.landed.lock().unwrap_or_else(|e| e.into_inner()));
    let mut changed = !landed.is_empty();
    let now = *s.now_ms;
    for l in landed {
        match l {
            Landed::Listed(dir, shells) => replace(&mut s.shells, &dir, shells),
            Landed::Output { id, attempt, page } => {
                if s.shells.activity.shown().is_some_and(|(sh, ..)| sh.id == id) {
                    let e = match page {
                        Ok(page) => TailEvent::Read { attempt, page },
                        Err(true) => TailEvent::ReadMissing { attempt },
                        Err(false) => TailEvent::ReadFailed { attempt },
                    };
                    dispatch(s, Event::Tail(e));
                }
            }
            Landed::Ended { attempt, info } => dispatch(s, Event::EndedSettled { attempt, info, now: now }),
            Landed::Follow { id, generation, attempt, page } => {
                let effects = s.shells.follow.landed(&id, generation, attempt, page);
                run_follow(s, effects);
            }
            Landed::Stopped { attempt, ok } => {
                if !ok {
                    crate::status_toast::fail(s, opencode_i18n::t!("common.requestFailed"), String::new());
                }
                dispatch(s, Event::StopSettled { attempt, stopped: ok, now: now });
            }
        }
    }
    // Every poll comes here: the shells are written only when a timer is due, a followed shell
    // shows or one stops showing, or the clock passes a second.
    let (due, later): (Vec<_>, Vec<_>) = if s.shells.timers.iter().any(|(at, _)| *at <= now) {
        std::mem::take(&mut s.shells.timers).into_iter().partition(|(at, _)| *at <= now)
    } else {
        (Vec::new(), Vec::new())
    };
    if !due.is_empty() {
        s.shells.timers = later;
    }
    for (_, d) in due {
        match d {
            Due::Machine { attempt, timer: Timer::Stopping } => dispatch(s, Event::StopTimedOut { attempt }),
            Due::Machine { attempt, timer: Timer::Ending } => dispatch(s, Event::EndedTimedOut { attempt, now: now }),
            Due::TailWait { attempt } => dispatch(s, Event::Tail(TailEvent::WaitElapsed { attempt })),
            Due::FollowWait { id, generation, attempt } => {
                let effects = s.shells.follow.elapsed(&id, generation, attempt);
                run_follow(s, effects);
            }
        }
    }
    if work(s) != s.shells.work {
        dispatch(s, Event::WorkSynced);
    }
    let shown = followed(s);
    if !shown.is_empty() || s.shells.follow.shows_any() {
        let effects = s.shells.follow.sync(&shown);
        run_follow(s, effects);
    }
    if let Some(next) = s.shells.timers.iter().map(|(at, _)| *at).min() {
        tx.wake_after(std::time::Duration::from_millis((next - now).max(0) as u64));
    }
    // The list's run times tick on whole seconds while it's open over running work.
    if s.shells.activity.is_open() && (!s.shells.work.shells.is_empty() || !s.shells.work.subagents.is_empty()) {
        let second = now.div_euclid(1000);
        tx.wake_after(std::time::Duration::from_millis(((second + 1) * 1000 - now) as u64));
        if s.shells.ticked != second {
            s.shells.ticked = second;
            changed = true;
        }
    }
    changed || s.shells.activity != before
}

/// The open shell consoles of the open session that read their shell, each
/// `(shell, directory, running)` (`tools::shell_follow`).
fn followed(s: &App) -> Vec<(String, String, bool)> {
    let Route::Session { id, .. } = &*s.route else { return Vec::new() };
    let Some(t) = s.transcripts.get(id) else { return Vec::new() };
    let directory = s.session(id).map_or("", |i| i.directory.as_str());
    let running = |shell: &str| s.shells.running.iter().any(|r| r.id == shell);
    let steps = t.messages.iter().filter_map(|m| match m {
        Message::Assistant(a) => Some(a),
        Message::Note { step, .. } => Some(&**step),
        _ => None,
    });
    steps
        .flat_map(|a| a.parts.iter())
        .filter_map(|p| match p {
            Part::Tool { id, name, status, input, metadata, .. } if name == "shell" => {
                let call = session_view::tools::Call { id, name, status, input, metadata };
                // A saved shell has nothing to follow; asking first spares `console_open`'s key on every poll.
                let (shell, live) = session_view::tools::shell_follow(&call, running)?;
                session_view::tools::console_open(s, &call).then(|| (shell.to_owned(), directory.to_owned(), live))
            }
            _ => None,
        })
        .collect()
}

/// Runs the reads and waits of the timeline consoles' followers.
fn run_follow(s: &mut App, effects: Vec<shell_follow::Effect>) {
    let now = *s.now_ms;
    for e in effects {
        match e.effect {
            TailEffect::Wait { attempt, ms } => s.shells.timers.push((now + ms, Due::FollowWait { id: e.id, generation: e.generation, attempt })),
            TailEffect::Read { attempt, cursor } => {
                let path = format!("/api/shell/{}/output?{}&cursor={cursor}", crate::switcher::encode(&e.id), location(&e.directory));
                let (id, generation) = (e.id, e.generation);
                request(s, move |server| {
                    let page = server.get(&path).map(|v| page_of(&v)).map_err(|e| e.contains("404"));
                    Landed::Follow { id, generation, attempt, page }
                });
            }
        }
    }
}

fn run(s: &mut App, effect: Effect) {
    let now = *s.now_ms;
    match effect {
        Effect::TimerStart { attempt, timer } => {
            let ms = if timer == Timer::Stopping { activity::STOP_TIMEOUT_MS } else { activity::END_TIMEOUT_MS };
            s.shells.timers.push((now + ms, Due::Machine { attempt, timer }));
        }
        Effect::TimerCancel(None) => s.shells.timers.retain(|(_, d)| !matches!(d, Due::Machine { .. })),
        Effect::TimerCancel(Some(a)) => s.shells.timers.retain(|(_, d)| !matches!(d, Due::Machine { attempt, .. } if *attempt == a)),
        Effect::Tail(TailEffect::Wait { attempt, ms }) => s.shells.timers.push((now + ms, Due::TailWait { attempt })),
        Effect::Tail(TailEffect::Read { attempt, cursor }) => {
            let Some((sh, ..)) = s.shells.activity.shown() else { return };
            let path = format!("/api/shell/{}/output?{}&cursor={cursor}", crate::switcher::encode(&sh.id), location(&sh.directory));
            let id = sh.id.clone();
            request(s, move |server| {
                let page = server.get(&path).map(|v| page_of(&v)).map_err(|e| e.contains("404"));
                Landed::Output { id, attempt, page }
            });
        }
        Effect::LoadEnded { attempt, id } => {
            let dir = s.shells.activity.shown().map(|(sh, ..)| sh.directory.clone()).unwrap_or_default();
            let path = format!("/api/shell/{}?{}", crate::switcher::encode(&id), location(&dir));
            request(s, move |server| {
                let info = server.get(&path).ok().map(|v| {
                    let d = v.get("data").unwrap_or(&v);
                    // A shell still running when asked was removed by a Stop.
                    let status = match d["status"].as_str() {
                        Some("exited") => EndStatus::Exited,
                        Some("timeout") => EndStatus::Timeout,
                        _ => EndStatus::Killed,
                    };
                    EndInfo { status, exit: d["exit"].as_i64(), completed: d["time"]["completed"].as_i64() }
                });
                Landed::Ended { attempt, info }
            });
        }
        Effect::RunStop { attempt, key, subagent } => {
            let dir = s.shells.running.iter().find(|r| r.id == key).map(|r| r.directory.clone()).unwrap_or_default();
            request(s, move |server| {
                let ok = if subagent {
                    server.interrupt(&key).is_ok()
                } else {
                    server.write("DELETE", &format!("/api/shell/{}?{}", crate::switcher::encode(&key), location(&dir)), None).is_ok()
                };
                Landed::Stopped { attempt, ok }
            });
        }
    }
}

/// Runs `work` against the live server on a thread, its result landing for the next poll.
fn request(s: &App, work: impl FnOnce(&opencode_kit::client::Server) -> Landed + Send + 'static) {
    let Source::Live { server, .. } = &*s.source else { return };
    // Headless snapshots have no event loop to wait on a thread: the read lands for the next frame.
    if crate::headless() {
        let l = work(server);
        push(&s.shells.landed, None, l);
        return;
    }
    let (server, landed, wake) = (server.clone(), Arc::clone(&s.shells.landed), (*s.waker).clone());
    spawn(move || {
        let l = work(&server);
        push(&landed, wake.as_ref(), l);
    });
}

fn location(directory: &str) -> String {
    format!("location%5Bdirectory%5D={}", crate::switcher::encode(directory))
}

fn page_of(v: &Value) -> Page {
    let d = v.get("data").unwrap_or(v);
    Page { output: d["output"].as_str().unwrap_or("").to_owned(), cursor: d["cursor"].as_u64().unwrap_or(0), size: d["size"].as_u64().unwrap_or(0) }
}

fn replace(shells: &mut Shells, directory: &str, list: Vec<Shell>) {
    shells.running.retain(|r| r.directory != directory);
    shells.running.extend(list);
}

fn parse_list(v: &Value, directory: &str) -> Vec<Shell> {
    let dir = v["location"]["directory"].as_str().unwrap_or(directory);
    v["data"].as_array().into_iter().flatten().filter(|i| i["status"] == "running").filter_map(|i| parse(i, dir)).collect()
}

fn parse(info: &Value, directory: &str) -> Option<Shell> {
    Some(Shell {
        id: info["id"].as_str()?.to_owned(),
        session: info["metadata"]["sessionID"].as_str().unwrap_or("").to_owned(),
        command: info["command"].as_str().unwrap_or("").to_owned(),
        started: info["time"]["started"].as_i64().unwrap_or(0),
        directory: directory.to_owned(),
    })
}

fn push(landed: &Mutex<Vec<Landed>>, wake: Option<&rune::Waker>, l: Landed) {
    landed.lock().unwrap_or_else(|e| e.into_inner()).push(l);
    if let Some(w) = wake {
        w();
    }
}

fn spawn(f: impl FnOnce() + Send + 'static) {
    let _ = std::thread::Builder::new().name("background-shells".into()).spawn(f);
}
