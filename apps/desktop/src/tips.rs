//! Paced one-time tips, as OpenCode Desktop shows them (`packages/app/src/tips/model.ts`,
//! `tips/queue.ts`): a persistent toast with an Install button, at most one a day and the first
//! an hour after the app was first seen, each until it's closed or its button is pressed. What
//! was shown and closed is kept in `opencode.global.dat`'s `tips.queue`, in Desktop's shape, so
//! a state seeded from Desktop carries it. Only a live window with a person at it shows them;
//! the `tip-browse` and `tip-hex` scenes show one at once.

pub mod queue;

use rune::Tx;

use crate::App;
use queue::{Effect, Event, History, Poll, Tip};

const GLOBAL: &str = "opencode.global.dat";
const KEY: &str = "tips.queue";

#[derive(Default)]
pub struct Tips {
    state: queue::State,
}

fn load() -> History {
    crate::storage::json(GLOBAL, KEY).and_then(|v| serde_json::from_value(v).ok()).unwrap_or_default()
}

fn save(history: &History) {
    if let Ok(v) = serde_json::to_value(history) {
        crate::storage::set_json(GLOBAL, KEY, &v);
    }
}

fn now_ms() -> i64 {
    crate::wall_ms()
}

/// Steps the tips and carries out what they ask; true when a toast came or went.
fn run(s: &mut App, event: Event, tx: &mut Tx) -> bool {
    let mut changed = false;
    for effect in queue::step(&mut s.tips.state, event) {
        match effect {
            Effect::WakeAt(due) => tx.wake_at(due),
            Effect::SyncMcp => s.sync_mcp(""),
            Effect::Show { tip, history } => {
                let Some(claim) = queue::Claim::take() else { continue };
                if let Some(history) = history.filter(|_| s.persists()) {
                    save(&history);
                }
                let toast = crate::status_toast::tip(s, tip);
                queue::step(&mut s.tips.state, Event::Opened { tip, toast, claim });
                changed = true;
            }
            Effect::Hide(toast) => {
                crate::status_toast::dismiss_id(s, toast);
                changed = true;
            }
            Effect::Write(history) if s.persists() => save(&history),
            Effect::Write(_) => {}
        }
    }
    changed
}

/// Each poll: what the pacing reads, from storage, the window and the server.
pub fn poll(s: &mut App, tx: &mut Tx) -> bool {
    if !s.persists() {
        return false;
    }
    let now = now_ms();
    let mut history = load();
    // `model.ts`: the first launch is remembered as soon as the state loads.
    if history.first_seen == 0 {
        history.first_seen = now;
        save(&history);
    }
    let poll = Poll {
        clock: *s.clock,
        now,
        history,
        browse_installed: s.mcp_servers("").map(|list| list.iter().any(|m| m.name == "browse")),
        online: !crate::reconnect::down(s),
        open: !s.plugins.window_blurred.get() && s.toast.stack.page() == rune_kit::ToastPage::Shown,
    };
    run(s, Event::Polled(poll), tx)
}

/// The person closed toast `toast` (`onDismiss`): its tip is done.
pub fn dismissed(s: &mut App, toast: u64, tx: &mut Tx) {
    run(s, Event::Handled { toast, history: load(), now: now_ms() }, tx);
}

/// The tip's Install: it's done, and its page opens.
pub fn installed(s: &mut App, toast: u64, tip: Tip, tx: &mut Tx) {
    dismissed(s, toast, tx);
    let _ = tx.open_url(tip.url());
}

fn force(s: &mut App, tip: Tip) {
    crate::scenes::open_scene(s);
    s.finish_stream();
    run(s, Event::Forced(tip), &mut Tx::default());
}

rune_register::register!(crate::scenes::SCENE_LIST: crate::scenes::Scene = [
    ("tip-browse", |s| force(s, Tip::Browse)),
    ("tip-hex", |s| force(s, Tip::Hex)),
]);
