//! Session ids named in replies and prompts (`session_view::session_chips`): what each stands
//! for, a quiet one-time lookup for one the window hasn't listed, and the chip's right-click menu.

use std::cell::RefCell;
use std::collections::HashSet;

use opencode_kit::client::Request;
use opencode_kit::mention::SessionRef;
use rune::{Icon, PointerPress, Tx};

use crate::{MentionsHost, title_of};

pub const OPEN_SESSION: &str = "session-ref-open";
pub const COPY_ID: &str = "session-chip-copy-id";
pub const COPY_TITLE: &str = "session-ref-copy-title";

/// One entry in a right-click context menu requested by a session reference.
pub struct ContextMenuItem {
    pub id: &'static str,
    pub label: &'static str,
    pub icon: Option<&'static Icon>,
}

/// Context menu specification passed to [`MentionsHost::open_context_menu`].
pub struct ContextMenuSpec {
    pub label: &'static str,
    pub min_width: f32,
    pub items: Vec<ContextMenuItem>,
}

/// The ids asked about once (a row asks on every build, even after a miss).
#[derive(Default)]
pub struct Lookups {
    asked: RefCell<HashSet<String>>,
}

impl Lookups {
    /// Marks `id` as looked up; returns `true` the first time `id` is seen.
    pub fn mark_asked(&self, id: &str) -> bool {
        self.asked.borrow_mut().insert(id.to_owned())
    }

    /// Clears recorded lookup state.
    pub fn clear(&mut self) {
        self.asked.get_mut().clear();
    }
}

/// What session `id` is, as loaded or as the `@` search found it; one neither knows is
/// looked up once, and stays text if the server hasn't it either. The active session itself
/// and archived sessions keep their raw IDs (`session-ui-provider.tsx:186-192`).
pub fn session_ref_of(host: &dyn MentionsHost, id: &str) -> Option<SessionRef> {
    if host.active_session_id() == Some(id) {
        return None;
    }
    let Some(info) = host.session_info(id).or_else(|| host.mentions().found_session(id)) else {
        look_up_session(host, id);
        return None;
    };
    if info.archived.is_some() {
        return None;
    }
    let parent = info.parent.as_deref().map(|p| host.session_info(p).map(title_of).unwrap_or_default());
    let busy = host.is_session_live(id);
    Some(SessionRef { title: info.title.clone(), parent, busy, updated: if busy { 0 } else { info.updated } })
}

fn look_up_session(host: &dyn MentionsHost, id: &str) {
    if !host.session_refs().mark_asked(id) {
        return;
    }
    host.send_request(Request::Lookup(id.to_owned()));
}

/// The session reference menu at the pointer (`SessionReferenceMenu`, anomalyco/opencode#63083):
/// Open Session (when not the active session), Copy Session ID, and Copy Session Title (when known
/// and unarchived).
pub fn open_session_chip_menu(host: &mut dyn MentionsHost, id: &str, press: PointerPress, tx: &mut Tx) {
    let is_current = host.active_session_id() == Some(id);
    let title = host
        .session_info(id)
        .or_else(|| host.mentions().found_session(id))
        .filter(|s| s.archived.is_none())
        .map(|s| s.title.trim())
        .filter(|t| !t.is_empty())
        .map(str::to_owned);
    let mut items = Vec::with_capacity(3);
    if !is_current {
        items.push(ContextMenuItem { id: OPEN_SESSION, label: opencode_i18n::t!("session.reference.open"), icon: None });
    }
    items.push(ContextMenuItem { id: COPY_ID, label: opencode_i18n::t!("command.session.copyID"), icon: None });
    if title.is_some() {
        items.push(ContextMenuItem { id: COPY_TITLE, label: opencode_i18n::t!("session.reference.copyTitle"), icon: None });
    }
    let spec = ContextMenuSpec {
        label: opencode_i18n::t!("ui.message.moreActions"),
        min_width: 160.0,
        items,
    };
    let id = id.to_owned();
    tx.instant();
    host.open_context_menu(
        spec,
        press,
        tx,
        Box::new(move |host, picked, tx| match picked {
            Some(OPEN_SESSION) => crate::open_mentioned(host, &id, tx),
            Some(COPY_ID) => {
                tx.copy(id.clone());
                host.toast_succeed(opencode_i18n::t!("common.copied").to_owned(), id);
            }
            Some(COPY_TITLE) => {
                if let Some(title) = title {
                    tx.copy(title.clone());
                    host.toast_succeed(opencode_i18n::t!("common.copied").to_owned(), title);
                }
            }
            _ => {}
        }),
    );
}
