//! The storybook's Subagents capsule story (Composer › Subagents capsule, 30 running): thirty
//! background agents run below the open session, some for minutes and some woken again hours
//! after they started, a Code Mode script runs in the background (another still in the
//! foreground, which the capsule leaves out), and the capsule's list is open over the composer. O opens and closes it, R replays.

use opencode_kit::model::SessionInfo;
use rune::Play;

use crate::App;
use crate::stories::{Trigger, replay, sid};

pub const KEYS: &[Trigger] = &[replay(open), Trigger { key: 'o', label: "Open or close the list", run: |s, _| { let next = !s.kernel.slots.get(&crate::subagents::SHELLS).activity.is_open(); crate::subagents::shells::set_open(s, next); } }];

const TASKS: &[(&str, &str)] = &[
    ("general", "Slack messages prototype"),
    ("general", "List motion tuning"),
    ("explore", "Sidebar gap and corners"),
    ("general", "Sidebar fade sections"),
    ("general", "Item switch autoresearch"),
    ("explore", "Dock holds the whole ask"),
    ("general", "Audio attachments"),
    ("general", "Sidebar bar and the Done section"),
    ("explore", "Apply the shortcuts everywhere"),
    ("general", "Drawing on images"),
    ("general", "Context for the current ask"),
    ("general", "Selectable content area"),
    ("general", "Merge, release and set up the menu bar app"),
    ("explore", "Lightbox close lands on its thumbnail"),
    ("general", "Sounds and settings"),
    ("general", "Item page consistency"),
    ("explore", "Task link chips"),
    ("general", "Multiplayer plan"),
    ("general", "⌘K to message mode, with a preview of the reply and the session it goes to"),
    ("general", "Recording bar prototypes"),
    ("explore", "Focus the item page"),
    ("general", "Bento grid and naming"),
    ("general", "Stream voice to the dictation app"),
    ("general", "Picker page prototypes"),
    ("explore", "Settings placement round"),
    ("general", "Status glyphs"),
    ("general", "Audit stale working cards"),
    ("explore", "Find every caller of reconnect()"),
    ("general", "Write tests for jittered backoff with a fake clock and a dropped socket"),
    ("general", "Profile the timeline's scroll path"),
];

fn child(k: usize) -> String {
    format!("ses_story_capsule_{k:02}")
}

/// Thirty agents and two scripts run, one in the background, and the list is open.
fn open(s: &mut App, _: &mut Play<App>) {
    s.finish_stream();
    run_thirty(s);
    let parent = sid(s);
    for (k, ago) in [(1, 95_000), (2, 12_000)] {
        let info = serde_json::json!({
            "id": format!("cm_story_script_{k}"),
            "kind": "codemode",
            "status": "running",
            "sessionID": parent,
            "time": { "started": *s.now_ms - ago },
        });
        crate::subagents::jobs::event(s, "experimental.job.updated", &serde_json::json!({ "info": info }));
    }
    // A script counts once its `execute` call has ended with the job's id (opencode#65303): the
    // first moved to the background; the second still runs in the foreground, so it isn't listed.
    name_script(s, &parent, "cm_story_script_1", "await reindex(workspace)");
    crate::subagents::shells::set_open(s, true);
}

/// Ends an `execute` call on `session` that moved job `job` to the background, running `code`.
fn name_script(s: &mut App, session: &str, job: &str, code: &str) {
    use opencode_kit::model::{Assistant, Message, Part, ToolStatus};
    let Some(t) = s.transcripts.get_mut(session) else { return };
    let call = Part::Tool {
        id: format!("call_{job}"),
        name: "execute".into(),
        status: ToolStatus::Completed,
        title: String::new(),
        input: serde_json::json!({ "code": code }),
        files: Vec::new(),
        metadata: serde_json::json!({ "jobID": job }),
    };
    match t.messages.iter_mut().rev().find_map(|m| if let Message::Assistant(a) = m { Some(a) } else { None }) {
        Some(a) => {
            a.parts.retain(|p| !matches!(p, Part::Tool { id, .. } if *id == format!("call_{job}")));
            a.parts.push(call);
        }
        None => t.messages.push(Message::Assistant(Assistant { id: "msg_story_script".into(), parts: vec![call], ..Default::default() })),
    }
}

/// Thirty agents run below the open session; every third was made hours ago and woke again a
/// few minutes back, so the list dates it from its last idle, not its creation.
pub fn run_thirty(s: &mut App) {
    let parent = sid(s);
    let from = s.session(&parent).cloned().unwrap_or_default();
    s.sessions.retain(|i| !i.id.starts_with("ses_story_capsule_"));
    for (k, (agent, title)) in TASKS.iter().enumerate() {
        let id = child(k);
        let ago = 20_000 + k as i64 * 47_000;
        let (created, idle) = if k % 3 == 2 { (*s.now_ms - ago * 40, Some(*s.now_ms - ago)) } else { (*s.now_ms - ago, None) };
        s.sessions.push(SessionInfo {
            id: id.clone(),
            parent: Some(parent.clone()),
            project: from.project.clone(),
            directory: from.directory.clone(),
            title: (*title).into(),
            agent: (*agent).into(),
            created,
            updated: *s.now_ms,
            idle,
            ..Default::default()
        });
        s.running.insert(id);
    }
}

rune_register::register!(crate::stories::STORY_LIST: crate::stories::Story = [
    crate::stories::Story::session("Composer/Subagents capsule, 30 running", KEYS),
]);
