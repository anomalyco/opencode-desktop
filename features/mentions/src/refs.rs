//! Session ids named in replies and prompts (`session_view::session_chips`): what each stands
//! for, a quiet one-time lookup for one the window hasn't listed, and the chip's right-click menu.

use std::cell::RefCell;
use std::collections::HashSet;

use opencode_kit::client::Request;
use rune::{Icon, PointerPress, Tx};
use rune_theme_opencode::icons::OUTLINE_COPY;
use session_view::session_chips::SessionRef;

use crate::{MentionsHost, title_of};

pub const COPY_ID: &str = "session-chip-copy-id";

/// One entry in a right-click context menu requested by a session chip.
pub struct ContextMenuItem {
    pub id: &'static str,
    pub label: &'static str,
    pub icon: &'static Icon,
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
/// looked up once, and stays text if the server hasn't it either.
pub fn session_ref_of(host: &dyn MentionsHost, id: &str) -> Option<SessionRef> {
    let Some(info) = host.session_info(id).or_else(|| host.mentions().found_session(id)) else {
        look_up_session(host, id);
        return None;
    };
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

/// The chip's menu at the pointer: Copy Session ID.
pub fn open_session_chip_menu(host: &mut dyn MentionsHost, id: &str, press: PointerPress, tx: &mut Tx) {
    let spec = ContextMenuSpec {
        label: opencode_i18n::t!("ui.message.moreActions"),
        min_width: 160.0,
        items: vec![ContextMenuItem {
            id: COPY_ID,
            label: opencode_i18n::t!("rune.session.chip.copyId"),
            icon: &OUTLINE_COPY,
        }],
    };
    let id = id.to_owned();
    tx.instant();
    host.open_context_menu(
        spec,
        press,
        tx,
        Box::new(move |picked, tx| {
            if picked == Some(COPY_ID) {
                tx.copy(id);
            }
        }),
    );
}
