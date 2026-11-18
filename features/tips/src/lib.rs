//! Paced one-time tips, as OpenCode Desktop shows them (`packages/app/src/tips/model.ts`,
//! `tips/queue.ts`): a persistent toast with an Install button, at most one a day and the first
//! an hour after the app was first seen, each until it's closed or its button is pressed. What
//! was shown and closed is kept in `opencode.global.dat`'s `tips.queue`, in Desktop's shape, so
//! a state seeded from Desktop carries it. Only a live window with a person at it shows them;
//! host scenes can call [`force`] to show one at once.

#![forbid(unsafe_code)]

pub mod queue;

use std::cell::Cell;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::sync::OnceLock;

use rune::Tx;

pub use queue::Tip;
use queue::{Effect, Event, History, Poll};

const GLOBAL: &str = "opencode.global.dat";
const KEY: &str = "tips.queue";

/// Shared by every window for this launch. Only workers initialize it; polls only read it.
static HEX_INSTALLED: OnceLock<bool> = OnceLock::new();

/// Opaque identity of the host's current route so tip pacing detects navigation between polls
/// without heap allocation or coupling to desktop's `Route` type.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct TipRoute(pub u64);

impl TipRoute {
    /// Derives a route key from any hashable host route value.
    pub fn from_hash(route: &(impl Hash + ?Sized)) -> Self {
        let mut hasher = DefaultHasher::new();
        route.hash(&mut hasher);
        Self(hasher.finish())
    }
}

/// Host state and callbacks required by paced one-time tips.
pub trait TipsHost: opencode_kernel::AppEnv {
    fn tips(&self) -> &Tips;
    fn tips_mut(&mut self) -> &mut Tips;
    /// Whether the server connection is up (replaces `!crate::reconnect::down(s)`).
    fn is_connected(&self) -> bool;
    /// Whether this window is focused and visible (eligible to display a tip toast).
    fn is_window_open(&self) -> bool;
    /// Opaque key for the currently shown route; changes count as user activity.
    fn route_key(&self) -> TipRoute;
    /// Whether the server's MCP list contains `browse`; `None` until the list has been fetched.
    fn browse_installed(&self) -> Option<bool>;
    /// Requests a refresh of the server's MCP list when the connection comes up.
    fn sync_mcp(&mut self);
    fn read_json(&self, ns: &str, key: &str) -> Option<serde_json::Value>;
    fn write_json(&self, ns: &str, key: &str, val: &serde_json::Value);
    fn show_tip_toast(&mut self, tip: Tip) -> u64;
    fn dismiss_toast(&mut self, id: u64);
    /// Opens the tip's install destination after the user presses its Install button.
    fn open_tip_target(&mut self, tip: Tip, tx: &mut Tx) {
        drop(tx.open_url(tip.url()));
    }
}

#[derive(Default)]
pub struct Tips {
    state: queue::State,
    last_input: Cell<Option<rune::time::Instant>>,
    last_route: Cell<Option<TipRoute>>,
}

fn load(host: &dyn TipsHost) -> History {
    host.read_json(GLOBAL, KEY).and_then(|v| serde_json::from_value(v).ok()).unwrap_or_default()
}

fn save(host: &dyn TipsHost, history: &History) {
    if let Ok(v) = serde_json::to_value(history) {
        host.write_json(GLOBAL, KEY, &v);
    }
}

fn now_ms() -> i64 {
    opencode_platform::wall_ms()
}

/// Steps the tips and carries out what they ask; true when a toast came or went.
fn run(host: &mut dyn TipsHost, event: Event) -> bool {
    let mut changed = false;
    let effects = queue::step(&mut host.tips_mut().state, event);
    for effect in effects {
        match effect {
            Effect::SyncMcp => host.sync_mcp(),
            Effect::CheckHex => {
                let wake = host.waker();
                if rune::task::spawn("tip-hex", move || {
                    HEX_INSTALLED.get_or_init(hex_client::installed);
                    if let Some(wake) = wake {
                        wake();
                    }
                })
                .is_err()
                {
                    let _ = HEX_INSTALLED.set(false).ok();
                }
            }
            Effect::Show { tip, history } => {
                let Some(claim) = queue::Claim::take() else { continue };
                if let Some(history) = history.filter(|_| host.persists()) {
                    save(&*host, &history);
                }
                let toast = host.show_tip_toast(tip);
                queue::step(&mut host.tips_mut().state, Event::Opened { tip, toast, claim });
                changed = true;
            }
            Effect::Hide(toast) => {
                host.dismiss_toast(toast);
                changed = true;
            }
            Effect::Write(history) if host.persists() => save(&*host, &history),
            Effect::Write(_) => {}
        }
    }
    changed
}

/// Each poll: what the pacing reads, from storage, the window and the server.
pub fn poll(host: &mut dyn TipsHost, _tx: &mut Tx) -> bool {
    poll_with(host, false)
}

/// The window's focus changed; gaining focus counts as user activity for a due tip.
pub fn on_focus(host: &mut dyn TipsHost, focused: bool) -> bool {
    poll_with(host, focused)
}

fn poll_with(host: &mut dyn TipsHost, focused: bool) -> bool {
    if !host.persists() {
        return false;
    }
    let input = rune::last_input();
    let had_input = host.tips().last_input.replace(input) != input;
    let route = host.route_key();
    let route_changed = match host.tips().last_route.replace(Some(route)) {
        Some(prev) => prev != route,
        None => false,
    };
    let acted = focused || had_input || route_changed;
    let now = now_ms();
    let mut history = load(&*host);
    // `model.ts`: the first launch is remembered as soon as the state loads.
    if history.first_seen == 0 {
        history.first_seen = now;
        save(&*host, &history);
    }
    let poll = Poll {
        clock: host.clock(),
        now,
        history,
        browse_installed: host.browse_installed(),
        hex_installed: if cfg!(target_arch = "wasm32") { Some(false) } else { HEX_INSTALLED.get().copied() },
        online: host.is_connected(),
        open: host.is_window_open(),
        acted,
    };
    run(host, Event::Polled(poll))
}

/// The person closed toast `toast` (`onDismiss`): its tip is done.
pub fn dismissed(host: &mut dyn TipsHost, toast: u64, _tx: &mut Tx) {
    let history = load(&*host);
    run(host, Event::Handled { toast, history, now: now_ms() });
}

/// The tip's Install: it's done, and its page opens.
pub fn installed(host: &mut dyn TipsHost, toast: u64, tip: Tip, tx: &mut Tx) {
    dismissed(host, toast, tx);
    host.open_tip_target(tip, tx);
}

/// Shows `tip` immediately outside normal pacing (for scene previews).
pub fn force(host: &mut dyn TipsHost, tip: Tip) -> bool {
    run(host, Event::Forced(tip))
}
