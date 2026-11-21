//! Desktop host adapter for `opencode-feature-annotations::quote` (`features/annotations`).

pub use opencode_feature_annotations::quote::*;

mod card_stories;
mod stories;

use opencode_kit::client::Server;
use rune::Tx;
use rune_knobs::Knobs;

use crate::{App, Route, Source};

rune_register::register!(crate::knobs::GROUP_LIST: rune_knobs::Group = [place::GROUP]);

pub(crate) static QUOTE: opencode_kernel::Slot<Quotes> = opencode_kernel::Slot::new();

impl QuoteHost for App {
    fn quotes(&self) -> &Quotes {
        self.kernel.slots.get(&QUOTE)
    }

    fn quotes_mut(&mut self) -> &mut Quotes {
        self.kernel.slots.get_mut(&QUOTE)
    }

    fn quote_session(&self) -> Option<&str> {
        let Route::Session { id, .. } = &*self.route else { return None };
        let child = self.session(id).is_some_and(|i| i.parent.is_some());
        (!child && !self.kernel.slots.get(&crate::add_menu::ADD_MENU).shell() && !self.settings_open()).then_some(id.as_str())
    }

    fn quote_session_busy(&self) -> bool {
        matches!(&*self.route, Route::Session { id, .. } if self.busy(id))
    }

    fn quote_follow_up(&self) -> FollowUp {
        match self.settings.follow_up {
            crate::settings::FollowUp::Queue => FollowUp::Queue,
            crate::settings::FollowUp::Steer => FollowUp::Steer,
        }
    }

    fn can_send_quote(&self) -> bool {
        matches!(*self.route, Route::Session { .. })
            && self.composer_ext.attach.uploads.is_empty()
            && self.opening.is_none()
    }

    fn send_quote(&mut self, key: u64, tx: &mut Tx) -> Option<Option<String>> {
        App::send_quote(self, key, tx)
    }

    fn send_comment(&mut self, key: u64, tx: &mut Tx) -> Option<Option<String>> {
        App::send_comment(self, key, false, tx)
    }

    fn quote_send_result(&self, id: &str) -> Option<SendResult> {
        self.send_result(id)
    }

    fn delete_failed_quote_prompt(&mut self, id: &str) {
        self.outbox_action(id, "outbox-delete");
    }

    fn narrow(&self) -> bool {
        App::narrow(self)
    }

    fn knobs(&self) -> &Knobs {
        &self.diag.knobs
    }

    fn auto_focus_composer(&self, tx: &Tx) -> bool {
        crate::auto_focus_composer(tx)
    }

    fn set_composer_focused(&mut self, focused: bool) {
        self.composer.focused = focused;
    }

    fn live_server(&self) -> Option<Server> {
        match &*self.source {
            Source::Live { server, .. } => Some(server.clone()),
            Source::Fixture => None,
        }
    }

    fn active_directory(&self) -> String {
        self.composer_directory()
    }

    fn search_local_paths(&self, query: &str, limit: usize) -> Vec<String> {
        self.kernel.slots.get(&crate::mentions::MENTIONS).local_paths(query, limit)
    }

    fn active_line_comment_card(&self) -> Option<u64> {
        self.kernel.slots.get(&crate::line_comments::LINE_COMMENTS).active_card
    }

    fn open_line_comment_card(&mut self, key: u64, tx: &mut Tx) {
        App::open_line_comment_card(self, key, tx);
    }

    fn remove_line_comment_card(&mut self, key: u64) {
        App::remove_line_comment_card(self, key);
    }

    fn open_note_card(&mut self, origin: &str, href: &str, tx: &mut Tx) {
        #[cfg(not(target_arch = "wasm32"))]
        if origin == "browser" {
            self.reveal_browser_note_href(href, tx);
        }
        let _ = (origin, href, tx);
    }
}
