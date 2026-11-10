//! The storybook's Long tool group story (Timeline › Long tool group): a turn that used 40
//! tools, its group near the timeline's end as in Kit's capture (t988). O opens or shuts it, the
//! end held as Desktop's pinned timeline keeps it; S scrolls into the open group so its header sticks to the top;
//! R replays open, scroll, then shut from the stuck header.

use opencode_kit::model::{Assistant, Message, Part, ToolStatus};
use rune::{Play, Tx};

use crate::App;
use crate::stories::{Trigger, replay, sid};

pub const KEYS: &[Trigger] = &[
    replay(open),
    Trigger { key: 'o', label: "Open or shut the group", run: |s, tx| flip(s, tx) },
    Trigger { key: 's', label: "Scroll into the group", run: |s, tx| scroll(s, tx) },
];

/// The group's row key (`turns::fold` keys a group by its first tool's id).
pub const GROUP: &str = "c-call_fold_00";
const TOOLS: usize = 40;

/// Ends session `session` with a prompt, a turn that used [`TOOLS`] tools, and its answer.
pub fn add_long_group(s: &mut App, session: &str) {
    let Some(t) = s.transcripts.get_mut(session) else { return };
    let names = ["read", "grep", "shell", "glob", "read", "execute", "skill"];
    let parts = (0..TOOLS)
        .map(|k| {
            let name = names[k % names.len()];
            Part::Tool {
                id: format!("call_fold_{k:02}"),
                name: name.into(),
                status: ToolStatus::Completed,
                title: format!("src/net/stream_{k:02}.rs"),
                input: serde_json::Value::Null,
                files: vec![],
                metadata: serde_json::Value::Null,
            }
        })
        .collect();
    t.messages.push(Message::User { id: "msg_zzfold0".into(), text: "Look through the reconnect code again before you merge.".into() });
    t.messages.push(Message::Assistant(Assistant { id: "msg_zzfold1".into(), completed: Some(1), parts, ..Default::default() }));
    let answer = vec![Part::Text { text: "Read it all again: the watchdog and the backoff hold up.".into() }];
    t.messages.push(Message::Assistant(Assistant { id: "msg_zzfold2".into(), completed: Some(2), parts: answer, ..Default::default() }));
}

fn open(s: &mut App, tx: &mut Play<App>) {
    s.finish_stream();
    let id = sid(s);
    add_long_group(s, &id);
    tx.after(0.6, |s, tx| flip(s, tx));
    tx.after(1.6, |s, tx| scroll(s, tx));
    tx.after(2.8, |s, tx| flip(s, tx));
}

/// Opens or shuts the group, as a press on its header does.
fn flip(s: &mut App, tx: &mut Tx) {
    crate::disclosure::toggle_group(GROUP.into())(s, tx);
}

/// Scrolls the open group's twentieth step to the middle of the timeline, under its stuck header.
fn scroll(s: &mut App, tx: &mut Tx) {
    if s.expanded.contains(GROUP) {
        tx.scroll_key_to("tool-call_fold_20", 160.0);
    }
}

const KNOBS: &[rune_knobs::Group] = &[rune_knobs::Group::new("Tool group", "stage", &[crate::disclosure::FOLD_SPEED, crate::disclosure::FOLD_FEATHER])];

rune_register::register!(crate::stories::STORY_LIST: crate::stories::Story = [
    crate::stories::Story::session("Timeline/Long tool group", KEYS).knobs(KNOBS),
]);

/// The fixture's tour session, which calls every kind of tool once, and the groups its turns
/// fold into (`turns::fold` keys a group by its first tool's id).
const TOUR_GROUPS: &[&str] = &["c-call_list_tour", "c-call_webfetch_tour", "c-call_edit_tour", "c-call_shell_tour", "c-call_shellerr_tour"];

/// The tour with its groups open: `group` alone, the session ending on the turn that holds it,
/// and `open` (tool rows, `tool:{id}`) open inside it. With no group, every group is open and
/// the whole session shows.
fn tour(s: &mut App, group: Option<&str>, open: &[&str]) {
    s.route = crate::Route::here(crate::fixture_ids::TOUR.into());
    let groups: Vec<&str> = group.map_or_else(|| TOUR_GROUPS.to_vec(), |g| vec![g]);
    s.expanded.extend(groups.iter().chain(open).map(|k| (*k).to_owned()));
    let Some(first) = group.and_then(|g| g.strip_prefix("c-")) else { return };
    if let Some(t) = s.transcripts.get_mut(crate::fixture_ids::TOUR)
        && let Some(at) = t.messages.iter().position(|m| matches!(m, Message::Assistant(a) if a.parts.iter().any(|p| matches!(p, Part::Tool { id, .. } if id == first))))
    {
        t.messages.truncate(at + 1);
    }
}

/// The tour's last turn still running: a shell streaming its output, a web search under way and
/// an edit the model is still writing.
fn tour_running(s: &mut App) {
    tour(s, Some("c-call_shell_tour"), &[]);
    let Some(t) = s.transcripts.get_mut(crate::fixture_ids::TOUR) else { return };
    let tool = |id: &str, name: &str, status, input: serde_json::Value, metadata: serde_json::Value| Part::Tool {
        id: id.into(),
        name: name.into(),
        status,
        title: String::new(),
        input,
        files: vec![],
        metadata,
    };
    let parts = vec![
        tool("call_live_shell", "shell", ToolStatus::Running, serde_json::json!({ "command": "kiln test --all" }), serde_json::json!({ "output": "running 126 tests\ntest cache::tests::evicts_oldest ... ok\ntest graph::tests::cycles_are_errors ... ok\n" })),
        tool("call_live_search", "websearch", ToolStatus::Running, serde_json::json!({ "query": "kiln doctor exit codes" }), serde_json::Value::Null),
        tool("call_live_edit", "edit", ToolStatus::Pending, serde_json::Value::Null, serde_json::Value::Null),
    ];
    t.messages.push(Message::Assistant(Assistant { id: "msg_zztourlive".into(), parts, ..Default::default() }));
    s.running.insert(crate::fixture_ids::TOUR.into());
    s.expanded.extend(["c-call_live_shell".to_owned(), "tool:call_live_shell".to_owned()]);
}

/// The tour's shell with a command that wraps over several lines, as in Kit's capture of a
/// console whose command and capped output both ran long.
fn long_command(s: &mut App) {
    let Some(t) = s.transcripts.get_mut(crate::fixture_ids::TOUR) else { return };
    let command = "D=/tmp/kiln-shots; perl -e 'alarm 60; exec @ARGV' \"/Applications/Browser.app/Contents/MacOS/Browser\" --headless=new --disable-gpu --no-first-run --user-data-dir=$(mktemp -d) --hide-scrollbars --force-device-scale-factor=1 --window-size=1600,3400 --screenshot=$D/full.png \"file://$D/index.html\" 2>&1 | tail -3; ls -la $D";
    for m in &mut t.messages {
        let Message::Assistant(a) = m else { continue };
        for part in &mut a.parts {
            if let Part::Tool { id, input, .. } = part
                && id == "call_shell_tour"
            {
                input["command"] = command.into();
            }
        }
    }
}

rune_register::register!(crate::scenes::SCENE_LIST: crate::scenes::Scene = [
    ("tools", |s| tour(s, None, &[])),
    ("tools-list", |s| tour(s, Some("c-call_list_tour"), &["tool:call_list_tour"])),
    ("tools-glob-grep", |s| tour(s, Some("c-call_list_tour"), &["tool:call_glob_tour", "tool:call_grep_tour"])),
    ("tools-read", |s| tour(s, Some("c-call_list_tour"), &[])),
    ("tools-web", |s| tour(s, Some("c-call_webfetch_tour"), &["tool:call_websearch_tour"])),
    ("tools-edit", |s| tour(s, Some("c-call_edit_tour"), &["tool:call_edit_tour/file:scripts/release.sh"])),
    ("tools-write", |s| tour(s, Some("c-call_edit_tour"), &["tool:call_edit_tour/file:ci/toolchain.env"])),
    ("tools-patch", |s| tour(s, Some("c-call_edit_tour"), &["tool:call_edit_tour/file:ci/build.yml", "tool:call_edit_tour/file:docs/toolchain.md"])),
    ("tools-shell", |s| tour(s, Some("c-call_shell_tour"), &["tool:call_shell_tour"])),
    ("tools-execute", |s| tour(s, Some("c-call_shell_tour"), &["tool:call_execute_tour", "tool:call_execute_tour/output"])),
    ("tools-execute-code", |s| {
        let open = format!("execute-details:{}:call_execute_tour", crate::fixture_ids::TOUR);
        tour(s, Some("c-call_shell_tour"), &[&open]);
    }),
    ("tools-shell-long", |s| {
        tour(s, Some("c-call_shell_tour"), &["tool:call_shell_tour"]);
        long_command(s);
    }),
    ("tools-error", |s| tour(s, Some("c-call_shellerr_tour"), &["tool:call_shellerr_tour"])),
    ("tools-running", tour_running),
]);
