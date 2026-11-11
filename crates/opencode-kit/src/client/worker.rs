//! The connection's threads: the fetch worker answering `Request`s, the writer running
//! `Write`s and the event stream, each waking the window when something lands.

use std::ops::ControlFlow;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::Duration;

use rune::Waker;

use super::{Incoming, Request, Server, Write, encode_component, list, save_download};
use crate::model;

/// How long the event stream may stay silent before it's dropped and reopened, as Desktop's
/// `defaultIdleTimeout`. The server writes a heartbeat every 15 s. A connection that died while
/// the Mac slept just goes quiet, and without a limit its read blocks forever: no reconnect, so
/// no resync, and sessions that finished meanwhile stay running.
const EVENTS_IDLE: Duration = Duration::from_secs(45);

#[cfg(not(target_arch = "wasm32"))]
static RETRY: (std::sync::Mutex<u64>, std::sync::Condvar) = (std::sync::Mutex::new(0),std::sync::Condvar::new());
#[cfg(target_arch = "wasm32")]
pub(super) use super::web::{read_events, wake_reconnects};
#[cfg(not(target_arch = "wasm32"))]
pub(super) fn wake_reconnects() {
    *RETRY.0.lock().unwrap() += 1;
    RETRY.1.notify_all();
}

/// Starts the worker, writer and event threads (the first loads already happened). Returns the
/// request and write senders and the inbox.
pub fn connect(server: Server, wake: Waker) -> (Sender<Request>, Sender<Write>, Receiver<Incoming>) {
    connect_with_idle(server, wake, EVENTS_IDLE)
}

/// [`connect`], dropping and reopening the event stream once it has been silent for `idle`
/// (heartbeats included) instead of [`EVENTS_IDLE`]: a slow or dead link (a tunnel that still
/// accepts but never answers) is noticed sooner.
pub fn connect_with_idle(server: Server, wake: Waker, idle: Duration) -> (Sender<Request>, Sender<Write>, Receiver<Incoming>) {
    let (inbox_tx, inbox) = channel();
    let (req_tx, write_tx) = spawn_workers(&server, &inbox_tx, &wake);
    #[cfg(not(target_arch = "wasm32"))]
    std::thread::Builder::new()
        .name("opencode-events".into())
        .spawn(move || read_events(&server, idle, &inbox_tx, &wake))
        .expect("spawn event thread");
    #[cfg(target_arch = "wasm32")]
    read_events(&server, idle, &inbox_tx, &wake);
    (req_tx, write_tx, inbox)
}

/// The event stream, reopened with backoff after each drop, until `inbox` closes.
#[cfg(not(target_arch = "wasm32"))]
pub(super) fn read_events(server: &Server, idle: Duration, inbox_tx: &Sender<Incoming>, wake: &Waker) {
    retry_events(server,idle,inbox_tx,wake,250);
}

/// How long a stream whose server refuses connections waits before trying on its own: the
/// reach watcher ([`super::reach`]) wakes it the moment the server accepts again, so this is only
/// a backstop.
#[cfg(not(target_arch = "wasm32"))]
const PARKED: Duration = Duration::from_secs(30);

#[cfg(not(target_arch = "wasm32"))]
fn retry_events(server:&Server,idle:Duration,inbox_tx:&Sender<Incoming>,wake:&Waker, mut backoff:u64) {
    let mut resumed = false;
    // The failure the window last heard of: the same one again tells it nothing, so it isn't woken.
    let mut said: Option<String> = None;
    loop {
        let mut opened = false;
        let generation = *RETRY.0.lock().unwrap();
        if let Err(e) = server.events(idle, inbox_tx, wake, resumed, &mut opened) {
            let news = opened || said.as_deref() != Some(e.as_str());
            // Sent either way: a closed inbox is how this thread learns its window went.
            if inbox_tx.send(Incoming::Status(e.clone())).is_err() {
                return;
            }
            if news {
                wake();
            }
            said = Some(e);
            server.watch_reach(None);
        }
        if opened {
            backoff = 250;
        }
        resumed = true;
        let wait = if server.refusing() { PARKED } else { Duration::from_millis(backoff) };
        let _wait = RETRY.1.wait_timeout_while(RETRY.0.lock().unwrap(),wait,|g| *g == generation).unwrap();
        backoff = (backoff * 2).min(3000);
    }
}

/// Whether a held write may go: its gate's answer (`false` when the gate is gone), or at once.
fn released(gate: &mut Option<std::sync::mpsc::Receiver<bool>>) -> bool {
    gate.take().is_none_or(|g| g.recv().unwrap_or(false))
}

/// The fetch and write threads, answering into `inbox_tx`.
pub(super) fn spawn_workers(server: &Server, inbox_tx: &Sender<Incoming>, wake: &Waker) -> (Sender<Request>, Sender<Write>) {
    let (req_tx, req_rx) = channel::<Request>();
    let (write_tx, write_rx) = channel::<Write>();

    {
        let server = server.clone();
        let inbox_tx = inbox_tx.clone();
        let wake = wake.clone();
        rune::task::serve("opencode-write", write_rx, move |w| {
                    #[cfg(not(target_arch = "wasm32"))]
                    let (mut gate, w) = match w {
                        Write::Gated { gate, write } => (Some(gate), *write),
                        w => (None, w),
                    };
                    #[cfg(target_arch = "wasm32")]
                    let mut gate: Option<std::sync::mpsc::Receiver<bool>> = None;
                    // Every gated write but a plain send waits for its row first: they change the server
                    // before they post the prompt (a session made, a revert committed).
                    if !matches!(w, Write::Send { .. }) && !released(&mut gate) {
                        return ControlFlow::Continue(());
                    }
                    let (ticket, w) = match w {
                        Write::Tracked { ticket, write } => (Some(ticket), *write),
                        w => (None, w),
                    };
                    let mut forked = None;
                    let prompt_id = match &w {
                        Write::Send { session, prompt }
                        | Write::SendMovingHome { session, prompt }
                        | Write::SendFromRevert { session, prompt }
                        | Write::StartAs { session, prompt, .. }
                        | Write::StartInNewWorktree { session, prompt, .. } => {
                            prompt.id.clone().map(|id| (session.clone(), id))
                        }
                        _ => None,
                    };
                    // A dock's reply: its request, so the dock hears it was taken.
                    let request_id = match &w {
                        Write::Permission { request, .. } => Some(request.clone()),
                        Write::Form { form, .. } | Write::CancelForm { form, .. } => Some(form.clone()),
                        _ => None,
                    };
                    // A rename, delete or restore: its answer goes back to the machine that sent it
                    // (`client.session.write`), failures included.
                    let session_write = match &w {
                        Write::Rename { session, title } => Some(serde_json::json!({ "kind": "rename", "sessionID": session, "title": title })),
                        Write::Delete(session) => Some(serde_json::json!({ "kind": "delete", "sessionID": session })),
                        Write::Unarchive(session) => Some(serde_json::json!({ "kind": "unarchive", "sessionID": session })),
                        _ => None,
                    };
                    let result = match w {
                        Write::Prompt { session, text } => server.send_prompt(&session, &text),
                        Write::Permission { session, request, decision } => server.reply_permission(&session, &request, &decision),
                        Write::Form { session, form, answer } => server.reply_form(&session, &form, answer),
                        Write::CancelForm { session, form } => server.cancel_form(&session, &form),
                        Write::Interrupt(session) => server.stop(&session).map(|interrupted| {
                            if let Some(interrupted) = interrupted {
                                let _ = inbox_tx.send(Incoming::Event {
                                    ty: "session.interrupt.acknowledged".into(),
                                    data: serde_json::json!({ "sessionID": session, "interrupted": interrupted }),
                                });
                            }
                        }),
                        Write::Create { directory, text } => server.create_session(&directory).and_then(|info| {
                            server.send_prompt(&info.id, &text)?;
                            let _ = inbox_tx.send(Incoming::Created(info));
                            Ok(())
                        }),
                        Write::Send { session, prompt } => {
                            // A send makes its request while its row commits, and posts it once it has.
                            let request = server.prepare(&session, &prompt);
                            if !released(&mut gate) {
                                return ControlFlow::Continue(());
                            }
                            server.send_prepared(&session, &prompt, request).map(|_| ())
                        }
                        Write::SendMovingHome { session, prompt } => server.send_moving_home(&session, &prompt).map(|_| ()),
                        Write::Start { directory, prompt } => server.create_session(&directory).and_then(|mut info| {
                            server.send(&info.id, &prompt)?;
                            // The reply to the create came before the switches.
                            if let Some(agent) = &prompt.agent {
                                info.agent.clone_from(agent);
                            }
                            if let Some(m) = &prompt.model {
                                info.model = m.clone();
                            }
                            let _ = inbox_tx.send(Incoming::Created(info));
                            Ok(())
                        }),
                        Write::StartAs { session, directory, prompt } => server.start_as(&session, &directory, &prompt).map(|info| {
                            let _ = inbox_tx.send(Incoming::Created(info));
                        }),
                        Write::StartInNewWorktree { session, project, from, branch, prompt } => {
                            let made = || {
                                let _ = inbox_tx.send(Incoming::Event { ty: "worktree.updated".into(), data: serde_json::json!({ "projectID": project }) });
                            };
                            server.start_in_new_worktree(&session, &project, &from, branch.as_deref(), &prompt, made).map(|info| {
                                let _ = inbox_tx.send(Incoming::Created(info));
                            })
                        }
                        Write::Cancel { session, item } => server.cancel_inbox(&session, &item),
                        Write::Deliver { session, item, delivery } => server.set_delivery(&session, &item, delivery),
                        Write::Shell { session, id, command } => server.shell(&session, id.as_deref(), &command),
                        Write::Rename { session, title } => server.rename_session(&session, &title),
                        Write::Delete(session) => server.delete_session(&session),
                        Write::Archive(session) => server.archive(&session),
                        Write::Unarchive(session) => server.unarchive(&session),
                        Write::View { session, idle } => server.view(&session, idle),
                        Write::Metadata { session, metadata } => server.set_metadata(&session, metadata),
                        Write::Export(session) => server.export_session(&session).and_then(|(name, json)| save_download(&name, &json)).map(|path| {
                            let _ = inbox_tx.send(Incoming::Status(format!("Saved session to {path}")));
                        }),
                        Write::Background { session, quiet } => server.background(&session).or_else(|e| if quiet { Ok(()) } else { Err(e) }),
                        Write::Compact(session) => server.compact(&session),
                        // `command.config.reload.done` and `.failed` (`composer/commands.tsx:75-82`).
                        Write::Reload => server.write("POST", "/api/location/reload", None)
                            .map(|_| { let _ = inbox_tx.send(Incoming::Status("Configuration reloaded".into())); })
                            .map_err(|e| format!("Couldn't reload configuration: {e}")),
                        Write::OpenProject { directory, start_session } => server.open_project(&directory).and_then(|(location, projects)| {
                            let session = if start_session { Some(server.create_session(&location.directory)?) } else { None };
                            let _ = inbox_tx.send(Incoming::ProjectOpened { location, projects, session });
                            Ok(())
                        }),
                        Write::RevertStage { session, message, settle } => {
                            let res = if settle { server.revert_to(&session, &message) } else { server.revert_stage(&session, &message) };
                            res.map(|_| ())
                        }
                        Write::RevertClear(session) => server.revert_clear(&session),
                        Write::RevertCommit(session) => server.revert_commit(&session),
                        Write::SendFromRevert { session, prompt } => server.send_from_revert(&session, &prompt).map(|_| ()),
                        Write::Fork { session, before } => server.fork_session(&session, before.as_deref()).map(|session| forked = Some(session)),
                        Write::Tracked { .. } => Err("a tracked write can't track another".into()),
                        #[cfg(not(target_arch = "wasm32"))]
                        Write::Gated { .. } => Err("a gated write can't hold another".into()),
                    };
                    if let Some(ticket) = ticket {
                        if inbox_tx.send(Incoming::Settled { ticket, result: result.map(|()| forked) }).is_err() {
                            return ControlFlow::Break(());
                        }
                        wake();
                        return ControlFlow::Continue(());
                    }
                    if result.is_ok() && let Some(id) = &request_id {
                        let _ = inbox_tx.send(Incoming::Event { ty: "client.request.replied".into(), data: serde_json::json!({ "requestID": id }) });
                    }
                    if result.is_ok() && let Some((session,id)) = &prompt_id {
                        let _ = inbox_tx.send(Incoming::Event { ty: "client.prompt.accepted".into(), data: serde_json::json!({"sessionID":session,"id":id}) });
                    }
                    if let Some(mut data) = session_write {
                        data["ok"] = serde_json::json!(result.is_ok());
                        if let Err(e) = &result {
                            data["error"] = serde_json::json!(e);
                        }
                        if inbox_tx.send(Incoming::Event { ty: "client.session.write".into(), data }).is_err() {
                            return ControlFlow::Break(());
                        }
                        wake();
                        return ControlFlow::Continue(());
                    }
                    if let Err(e) = result {
                        let told = match prompt_id {
                            Some((session, id)) => inbox_tx.send(Incoming::SendFailed { session, id, error: e }),
                            None => inbox_tx.send(Incoming::Status(e)),
                        };
                        if told.is_err() {
                            return ControlFlow::Break(());
                        }
                    }
                    wake();
                    ControlFlow::Continue(())
            })
            .expect("spawn write thread");
    }

    {
        let server = server.clone();
        let inbox_tx = inbox_tx.clone();
        let wake = wake.clone();
        let send = move |m: Incoming| {
            let ok = inbox_tx.send(m).is_ok();
            wake();
            ok
        };
        let sessions = |server: &Server| server.get("/api/session?limit=120").map(|v| model::parse_sessions(&v));
        rune::task::serve("opencode-fetch", req_rx, move |r| {
                    let ok = match r {
                        Request::Session(id) => match server.read(&format!("/api/session/{id}/message?limit=80&order=desc")) {
                            Ok(v) => send(Incoming::Messages { session: id, messages: model::parse_messages(&v) }),
                            Err(e) => send(Incoming::Status(e)),
                        },
                        Request::Sessions => match sessions(&server) {
                            Ok(list) => send(Incoming::Sessions(list)),
                            Err(e) => send(Incoming::Status(e)),
                        },
                        Request::ProjectSessions(project) => {
                            match server.get(&format!("/api/session?project={}&archived=false&limit=200", encode_component(&project))) {
                                Ok(v) => send(Incoming::Sessions(model::parse_sessions(&v))),
                                Err(e) => send(Incoming::Status(e)),
                            }
                        }
                        Request::Projects => match server.projects() {
                            Ok(list) => send(Incoming::Projects(list)),
                            Err(e) => send(Incoming::Status(e)),
                        },
                        Request::Info(id) => match server.get(&format!("/api/session/{id}")) {
                            Ok(v) => send(Incoming::Info(model::parse_session(v.get("data").unwrap_or(&v)))),
                            Err(e) => send(Incoming::Status(e)),
                        },
                        // A miss (deleted, another server's) says so quietly, not as a status.
                        Request::Lookup(id) => match server.get(&format!("/api/session/{id}")) {
                            Ok(v) => send(Incoming::Info(model::parse_session(v.get("data").unwrap_or(&v)))),
                            Err(_) => send(Incoming::Event { ty: "session.lookup.missing".into(), data: serde_json::json!({ "sessionID": id }) }),
                        },
                        Request::Active => match server.get("/api/session/active") {
                            Ok(v) => {
                                let map = v.get("data").unwrap_or(&v);
                                send(Incoming::Active(map.as_object().into_iter().flatten().map(|(k, _)| k.clone()).collect()))
                            }
                            Err(e) => send(Incoming::Status(e)),
                        },
                        Request::Pending(id) => send(Incoming::Pending { items: server.pending(&id), session: id }),
                        Request::Config(dir) => {
                            // A server without it leaves compactions loud, as Desktop does.
                            let v = server.get(&format!("/api/config?location%5Bdirectory%5D={}", encode_component(&dir)));
                            let on = v.is_ok_and(|v| model::background_compaction(&v));
                            send(Incoming::Event { ty: "config.listed".into(), data: serde_json::json!({ "directory": dir, "backgroundCompaction": on }) })
                        }
                        Request::Worktrees { project, root } => match server.get(&format!("/api/worktree?projectID={}", encode_component(&project))) {
                            Ok(v) => send(Incoming::Event {
                                ty: "worktree.listed".into(),
                                data: serde_json::json!({ "projectID": project, "directories": model::parse_worktrees(&v, &root) }),
                            }),
                            Err(e) => send(Incoming::Status(e)),
                        },
                        Request::Branches { directory, search } => {
                            let data = match server.branches(&directory, &search) {
                                Ok(branches) => serde_json::json!({ "directory": directory, "search": search, "branches": branches }),
                                Err(_) => serde_json::json!({ "directory": directory, "search": search, "failed": true }),
                            };
                            send(Incoming::Event { ty: "vcs.branch.listed".into(), data })
                        }
                        Request::Inbox(id) => match server.read(&format!("/api/session/{id}/inbox")) {
                            Ok(v) => send(Incoming::Event {
                                ty: "session.inbox.listed".into(),
                                data: serde_json::json!({ "sessionID": id, "items": list(&v) }),
                            }),
                            Err(e) => send(Incoming::Status(e)),
                        },
                        // As Desktop's family sync, a failed read leaves the family as it was.
                        Request::Children(id) => match server.children(&id) {
                            Ok(sessions) => send(Incoming::Children { parent: id, sessions }),
                            Err(_) => true,
                        },
                    };
                    if !ok {
                        return ControlFlow::Break(());
                    }
                    // The channel blocks when idle. User-requested reads should drain without
                    // a timer between them: opening a session queues several reads together.
                    ControlFlow::Continue(())
            })
            .expect("spawn fetch thread");
    }

    (req_tx, write_tx)
}
