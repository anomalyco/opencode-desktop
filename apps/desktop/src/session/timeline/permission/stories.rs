//! The storybook's Permission story (Requests › Permission): on the session stage, a request
//! arrives in the composer's place. Variants: a bash command, a long list of files, an action Desktop has no words for,
//! and a command long enough to wrap. A allows it once (the composer comes back), R asks again.
//! Light and dark follow the app's theme.

use rune::Play;
use serde_json::json;

use crate::App;
use crate::stories::{Trigger, replay};

const ID: &str = "per_story_permission";
const VARIANTS: &[(&str, usize)] = &[("Bash", 0), ("Long list", 1), ("No description", 2), ("Long command", 3)];
const KEYS: &[Trigger] = &[
    replay(open),
    Trigger { key: 'a', label: "Allow once", run: |s, tx| {
        tx.clear();
        s.reply_permission(ID, "once");
        s.morph(tx);
    } },
];

/// Variant `v`'s action and patterns.
pub(crate) fn asked(v: usize) -> (&'static str, Vec<String>) {
    match v {
        1 => ("edit", [
            "src/net/backoff.ts",
            "src/net/backoff.test.ts",
            "src/net/reconnect.ts",
            "src/net/socket.ts",
            "src/events/stream.ts",
            "src/events/stream.test.ts",
            "packages/session-ui/src/components/dock-prompt/permission-patterns-that-run-long.tsx",
            "docs/reconnect.md",
        ].map(String::from).into()),
        2 => ("github_create_issue", vec!["anomalyco/seer".into()]),
        3 => ("bash", vec!["git commit -am 'Backoff: cap at 30 s, reset after a success, keep jitter in range' && bun run typecheck && bun test test/net/backoff.test.ts --timeout 20000".into()]),
        _ => ("bash", vec!["bun test test/net/backoff.test.ts".into()]),
    }
}

/// The composer first; the request arrives a beat in, as `permission.asked` brings it.
fn open(s: &mut App, _: &mut Play<App>) {
    s.finish_stream();
    s.settings.auto_accept = false;
    s.pending.clear();
    s.permission_sent.0.clear();
    ask_soon(s);
}

fn ask_soon(s: &mut App) {
    let (action, resources) = asked(*s.fixture_run.variant);
    let ask = json!({ "id": ID, "sessionID": crate::stories::sid(s), "action": action, "resources": resources });
    *s.fixture_run.playback = [(*s.clock + 0.4, "permission.asked".into(), ask)].into();
    s.wake_at_playback();
}

rune_register::register!(crate::stories::STORY_LIST: crate::stories::Story = [
    crate::stories::Story::session("Requests/Permission", KEYS).variants(VARIANTS),
]);
