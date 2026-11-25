//! Which sessions attach their browser besides the one on screen, and when a session another
//! window took comes back to this one (`packages/gui-plugins/src/browser/{model,prompt-gate}.ts`,
//! opencode#65969).

use std::collections::BTreeMap;

use opencode_kit::browser_rpc::{AttachEffect, AttachEvent, AttachMachine, AttachState, BlockReason};
use opencode_kit::model::BrowserGrant;
use opencode_kit::servers::ServerKey;
use rune::time::Instant;

use super::{BROWSER_RPC, BrowserRpcState, apply_batch_effects, session_directory};
use crate::App;

type Key = (ServerKey, String);

/// A subagent routed before stays attached while its root's tab does: official keeps a tab's
/// refs, the routed child's too, until the tab closes (`host-apis.tsx` `sessions`).
pub(super) fn keep_subagents(app: &App, active_server: &ServerKey, open_dirs: &mut BTreeMap<Key, String>) {
    let kept: Vec<Key> = app.kernel.slots.get(&BROWSER_RPC).routed_sessions.iter().filter(|k| k.0 == *active_server && !open_dirs.contains_key(*k)).cloned().collect();
    for key in kept {
        let root = app.panel_root(&key.1);
        if root != key.1 && open_dirs.contains_key(&(active_server.clone(), root.to_owned())) && let Some(dir) = Some(session_directory(app, &key.1, "")).filter(|d| !d.is_empty()) {
            open_dirs.insert(key, dir);
        }
    }
}

/// The open sessions that run although nobody has opened them in this window since the app
/// started, and so attach as the routed one does (`prompt-gate.ts` `attachesUnopened`): one the
/// server resumed, or that an automation, a finished job or a subagent woke, would run every
/// step without the browser tools until someone opened its tab. A session that holds the grant
/// has a pane already, this window's or another's, and keeps it: a window that only lists a tab
/// must not take the pane from the one that shows it. Details not read yet say nothing about a
/// grant, so the session waits for them.
pub(super) fn unopened(app: &App, open_dirs: &BTreeMap<Key, String>) -> Vec<Key> {
    open_dirs
        .keys()
        .filter(|k| app.running.contains(&k.1) && app.session(&k.1).is_some_and(|info| info.browser == BrowserGrant::Free))
        .cloned()
        .collect()
}

/// The sessions another window took and has let go since: the server revoked the grant that
/// window's attachment held, as when it closed (`model.ts` `session.released`).
pub(super) fn released(app: &App) -> Vec<Key> {
    app.kernel
        .slots
        .get(&BROWSER_RPC)
        .sessions
        .iter()
        .filter(|(_, att)| matches!(att.attach.state, AttachState::Blocked { reason: BlockReason::Replaced }))
        .filter(|(k, _)| app.session(&k.1).is_some_and(|info| info.browser == BrowserGrant::Free))
        .map(|(k, _)| k.clone())
        .collect()
}

/// What an open session's attachment does at this poll: it attaches when it has not, and one
/// another window took comes back when the person came to it here (`came`) or that window let
/// it go (`released`): never on focus, a press, a key or an agent's action, which would fail
/// the commands an agent has in flight in the window that holds it.
pub(super) fn wake(attach: &mut AttachMachine, came: bool, released: bool, now: Instant) -> Vec<AttachEffect> {
    let mut effects = attach.step(AttachEvent::EnsureAttached, now);
    if came {
        effects.extend(attach.step(AttachEvent::Take, now));
    }
    if released {
        effects.extend(attach.step(AttachEvent::Released, now));
    }
    effects
}

/// The session on screen changed: the one the person came to, if any (`model.ts`
/// `session.routed`, whose wake takes).
pub(super) fn came_on_screen(st: &mut BrowserRpcState, routed_now: Option<&Key>) -> Option<Key> {
    if st.on_screen.as_ref() == routed_now {
        return None;
    }
    st.on_screen = routed_now.cloned();
    st.on_screen.clone()
}

/// The person sends a prompt to `session_id` from this window, or opens its browser here: the
/// session's browser is this window's, so one another window took comes back (`model.ts`
/// `ready`, `open`). Never called for focus, a press, a key or an agent's action.
pub fn person_came(app: &mut App, session_id: &str) {
    let now = Instant::now();
    let st = app.kernel.slots.get_mut(&BROWSER_RPC);
    let Some((key, att)) = st.sessions.iter_mut().find(|(k, _)| k.1 == session_id) else { return };
    let effects = att.attach.step(AttachEvent::Take, now);
    if !effects.is_empty() {
        let batch = vec![(key.clone(), effects)];
        apply_batch_effects(app, batch, None);
    }
}
