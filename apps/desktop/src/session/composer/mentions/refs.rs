//! Session ids named in replies and prompts (`session_view::session_chips`): what each stands
//! for, a quiet one-time lookup for one the window hasn't listed, and the chip's right-click menu.

use std::cell::RefCell;
use std::collections::HashSet;

use opencode_kit::client::Request;
use rune::{PointerPress, Tx};
use session_view::TimelineHost as _;
use session_view::session_chips::SessionRef;

use crate::popup::{Item, Menu};
use crate::{App, Source};

const COPY_ID: &str = "session-chip-copy-id";

/// The ids asked about once (a row asks on every build, even after a miss).
#[derive(Default)]
pub struct Lookups {
    asked: RefCell<HashSet<String>>,
}

impl App {
    /// What session `id` is, as loaded or as the `@` search found it; one neither knows is
    /// looked up once, and stays text if the server hasn't it either.
    pub fn session_ref_of(&self, id: &str) -> Option<SessionRef> {
        let Some(info) = self.session(id).or_else(|| self.mentions.found_session(id)) else {
            self.look_up_session(id);
            return None;
        };
        let parent = info.parent.as_deref().map(|p| self.session(p).map(crate::view::title_of).unwrap_or_default());
        let busy = self.is_session_live(id);
        Some(SessionRef { title: info.title.clone(), parent, busy, updated: if busy { 0 } else { info.updated } })
    }

    fn look_up_session(&self, id: &str) {
        let refs = &self.session_refs;
        if !refs.asked.borrow_mut().insert(id.to_owned()) {
            return;
        }
        if let Source::Live { requests: Some(r), .. } = &*self.source {
            let _ = r.send(Request::Lookup(id.to_owned()));
        }
    }

    /// The chip's menu at the pointer: Copy Session ID.
    pub fn open_session_chip_menu(&mut self, id: &str, press: PointerPress, tx: &mut Tx) {
        let menu = Menu::new(opencode_i18n::t!("ui.message.moreActions")).min_width(160.0).item(
            Item::new(COPY_ID, opencode_i18n::t!("rune.session.chip.copyId")).icon(&rune_theme_opencode::icons::OUTLINE_COPY),
        );
        let id = id.to_owned();
        tx.instant();
        crate::popup::open(self, menu, press, tx, move |_: &mut App, picked: Option<&str>, tx: &mut Tx| {
            if picked == Some(COPY_ID) {
                tx.copy(id);
            }
        });
    }
}
