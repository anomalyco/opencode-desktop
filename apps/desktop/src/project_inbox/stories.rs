//! Sidebar › Projects inbox: two project sessions, lumen's and harbor's, with tasks waiting on
//! you (one through its helper's question, one blocked), tasks done and unread (one that said
//! done, one that stopped without saying) and tasks still working. Some are subagents of the
//! project (t1191), some their own sessions the project started with `task_start` (t1213).

use opencode_kit::model::SessionInfo;
use rune::{El, Play, Tx, row};

use crate::fixture_ids::{MAIN, SIXTH};
use crate::palette::Pal;
use crate::stories::{Trigger, replay};
use crate::{App, Route, view};

const KEYS: &[Trigger] = &[
    replay(open),
    Trigger { key: 'a', label: "A task asks", run: |s, tx| ask(s, tx) },
    Trigger { key: 'f', label: "A working task finishes", run: |s, tx| finish(s, tx) },
    Trigger { key: 'x', label: "Mark the first done task read", run: |s, tx| read(s, tx) },
];

fn task(s: &mut App, id: &str, parent: &str, title: &str, idle_min: Option<i64>) {
    let now = s.now_ms;
    let project = s.session(parent).map(|p| (p.project.clone(), p.directory.clone())).unwrap_or_default();
    s.sessions.retain(|x| x.id != id);
    s.sessions.push(SessionInfo {
        id: id.into(),
        parent: Some(parent.into()),
        project: project.0,
        directory: project.1,
        title: title.into(),
        updated: now - idle_min.unwrap_or(1) * 60_000,
        created: now - 90 * 60_000,
        idle: idle_min.map(|m| now - m * 60_000),
        ..Default::default()
    });
}

/// A task with its own session, as `task_start` makes it, in `state` (`seerTask.state`).
fn own_task(s: &mut App, id: &str, project: &str, title: &str, state: &str, idle_min: Option<i64>) {
    task(s, id, project, title, idle_min);
    let now = s.now_ms;
    if let Some(x) = s.sessions.iter_mut().find(|x| x.id == id) {
        x.parent = None;
        x.metadata = serde_json::json!({ "seerTask": { "projectID": project, "state": state, "summary": title, "reportedAt": now - idle_min.unwrap_or(1) * 60_000 } });
    }
}

fn open(s: &mut App, _: &mut Play<App>) {
    seed(s);
}

/// The story's world, which the tests share.
pub(super) fn seed(s: &mut App) {
    s.finish_stream();
    let since = s.now_ms - 3 * 3_600_000;
    for (id, title) in [(MAIN, "Lumen"), (SIXTH, "Harbor")] {
        if let Some(x) = s.sessions.iter_mut().find(|x| x.id == id) {
            x.title = title.into();
            x.metadata = serde_json::json!({ "seerProject": { "id": id, "since": since } });
        }
    }
    task(s, "ses_ib01", MAIN, "Pick a retry budget for reconnects", None);
    task(s, "ses_ib02", MAIN, "Port the reconnect test to the fake clock", Some(4));
    task(s, "ses_ib03", "ses_ib01", "Bisect the jitter regression", None);
    task(s, "ses_ib09", MAIN, "Explain the backoff curve", Some(22));
    task(s, "ses_ib04", MAIN, "Profile the event stream parser", None);
    task(s, "ses_ib05", SIXTH, "Write the schema migration notes", None);
    task(s, "ses_ib06", SIXTH, "Migrate the loader tests", Some(64));
    task(s, "ses_ib07", SIXTH, "Retry uploads on a dropped socket", None);
    task(s, "ses_ib08", SIXTH, "Check the config fallbacks", None);
    own_task(s, "ses_ib10", MAIN, "Wait for the soak run", "working", Some(12));
    own_task(s, "ses_ib11", MAIN, "Bump the protocol version", "done", Some(2));
    own_task(s, "ses_ib12", SIXTH, "Tidy the upload errors", "stopped", Some(9));
    own_task(s, "ses_ib13", SIXTH, "Rotate the staging keys", "blocked", Some(15));
    own_task(s, "ses_ib14", MAIN, "Draft the reconnect docs", "working", None);
    // Its helper inherits the tag, as children do on the server.
    task(s, "ses_ib15", "ses_ib14", "Collect the error codes", None);
    let tag = s.session("ses_ib14").map(|x| x.metadata.clone()).unwrap_or_default();
    if let Some(x) = s.sessions.iter_mut().find(|x| x.id == "ses_ib15") {
        x.metadata = tag;
    }
    s.requests.retain(|(sid, _)| !sid.starts_with("ses_ib"));
    // A helper's question shows on its task.
    s.requests.insert(("ses_ib03".into(), "per_ib03".into()));
    s.requests.insert(("ses_ib05".into(), "frm_ib05".into()));
    s.running.retain(|id| !id.starts_with("ses_ib"));
    for id in ["ses_ib01", "ses_ib03", "ses_ib04", "ses_ib05", "ses_ib07", "ses_ib08", "ses_ib14", "ses_ib15"] {
        s.running.insert(id.into());
    }
    s.route = Route::Session("ses_ib02".into());
}

fn ask(s: &mut App, tx: &mut Tx) {
    tx.instant();
    s.requests.insert(("ses_ib04".into(), "per_ib04".into()));
}

fn finish(s: &mut App, tx: &mut Tx) {
    tx.instant();
    let now = s.now_ms;
    s.running.remove("ses_ib07");
    if let Some(x) = s.sessions.iter_mut().find(|x| x.id == "ses_ib07") {
        x.idle = Some(now);
        x.updated = now;
    }
}

fn read(s: &mut App, tx: &mut Tx) {
    let first = s.inbox_rows().iter().find(|r| r.kind == super::Kind::Done).map(|r| r.info.id.clone());
    if let Some(id) = first {
        s.mark_task_read(&id, tx);
    }
}

fn story_view(s: &App, p: &Pal) -> El {
    row().key("story-inbox").w(s.sidebar_w).h(640.0).fill(p.bg_deep).radius(10.0).clip_soft(0.0).child(view::sidebar(s, p))
}

rune_register::register!(crate::stories::STORY_LIST: crate::stories::Story = [
    crate::stories::Story { path: "Sidebar/Projects inbox", scene: |s| {
        seed(s);
        crate::stories::stage(s, story_view)
    }, knobs: &[], keys: KEYS, variants: &[] },
]);

rune_register::register!(crate::scenes::SCENE_LIST: crate::scenes::Scene = [("project-inbox", seed)]);
