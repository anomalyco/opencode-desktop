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

/// Shared by every window for this launch. Only workers initialize it; polls only read it.
static HEX_INSTALLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();

#[derive(Default)]
pub struct Tips {
    state: queue::State,
    last_input: std::cell::Cell<Option<rune::time::Instant>>,
    last_route: std::cell::RefCell<Option<crate::Route>>,
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
fn run(s: &mut App, event: Event) -> bool {
    let mut changed = false;
    for effect in queue::step(&mut s.tips.state, event) {
        match effect {
            Effect::SyncMcp => s.sync_mcp(""),
            Effect::CheckHex => {
                let wake = (*s.waker).clone();
                if rune::task::spawn("tip-hex", move || {
                    HEX_INSTALLED.get_or_init(hex_client::installed);
                    if let Some(wake) = wake { wake(); }
                }).is_err() {
                    let _ = HEX_INSTALLED.set(false);
                }
            }
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
pub fn poll(s: &mut App, _tx: &mut Tx) -> bool {
    poll_with(s, false)
}

/// The window's focus changed (`view.rs`); gaining focus counts as user activity for a due tip.
pub fn on_focus(s: &mut App, focused: bool) -> bool {
    poll_with(s, focused)
}

fn poll_with(s: &mut App, focused: bool) -> bool {
    if !s.persists() {
        return false;
    }
    let input = rune::last_input();
    let had_input = s.tips.last_input.replace(input) != input;
    let route = rune::Tracked::untracked(&s.route);
    let route_changed = {
        let mut prev = s.tips.last_route.borrow_mut();
        match prev.as_ref() {
            Some(r) if r == route => false,
            Some(_) => {
                *prev = Some(route.clone());
                true
            }
            None => {
                *prev = Some(route.clone());
                false
            }
        }
    };
    let acted = focused || had_input || route_changed;
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
        hex_installed: if cfg!(target_arch = "wasm32") { Some(false) } else { HEX_INSTALLED.get().copied() },
        online: !crate::reconnect::down(s),
        open: !s.plugins.window_blurred.get() && s.toast.stack.page() == rune_kit::ToastPage::Shown,
        acted,
    };
    run(s, Event::Polled(poll))
}

/// The person closed toast `toast` (`onDismiss`): its tip is done.
pub fn dismissed(s: &mut App, toast: u64, _tx: &mut Tx) {
    run(s, Event::Handled { toast, history: load(), now: now_ms() });
}

/// The tip's Install: it's done, and its page opens.
pub fn installed(s: &mut App, toast: u64, tip: Tip, tx: &mut Tx) {
    dismissed(s, toast, tx);
    let _ = tx.open_url(tip.url());
}

fn force(s: &mut App, tip: Tip) {
    crate::scenes::open_scene(s);
    s.finish_stream();
    run(s, Event::Forced(tip));
}

rune_register::register!(crate::scenes::SCENE_LIST: crate::scenes::Scene = [
    ("tip-browse", |s| force(s, Tip::Browse)),
    ("tip-hex", |s| force(s, Tip::Hex)),
]);
