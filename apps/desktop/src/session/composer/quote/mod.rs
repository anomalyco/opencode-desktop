//! Desktop host adapter for `opencode-feature-annotations::quote` (`features/annotations`).

pub use opencode_feature_annotations::quote::*;

mod card_stories;
mod stories;

use opencode_kit::client::Server;
use rune::Tx;
use rune_knobs::Knobs;

use crate::{App, Route, Source};

rune_register::register!(crate::knobs::GROUP_LIST: rune_knobs::Group = [place::GROUP]);

pub(crate) fn map_send_result(r: crate::send_failed::SendResult) -> SendResult {
    match r {
        crate::send_failed::SendResult::Accepted => SendResult::Accepted,
        crate::send_failed::SendResult::Failed => SendResult::Failed,
        crate::send_failed::SendResult::Queued => SendResult::Queued,
    }
}

impl QuoteHost for App {
    fn quotes(&self) -> &Quotes {
        &self.quote
    }

    fn quotes_mut(&mut self) -> &mut Quotes {
        &mut self.quote
    }

    fn quote_session(&self) -> Option<&str> {
        let Route::Session { id, .. } = &*self.route else { return None };
        let child = self.session(id).is_some_and(|i| i.parent.is_some());
        (!child && !self.add.shell() && !self.settings.open).then_some(id.as_str())
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
            && self.attach.uploads.is_empty()
            && self.opening.is_none()
    }

    fn send_quote(&mut self, key: u64, tx: &mut Tx) -> Option<Option<String>> {
        App::send_quote(self, key, tx)
    }

    fn quote_send_result(&self, id: &str) -> Option<SendResult> {
        self.send_result(id).map(map_send_result)
    }

    fn delete_failed_quote_prompt(&mut self, id: &str) {
        self.outbox_action(id, "outbox-delete");
    }

    fn narrow(&self) -> bool {
        App::narrow(self)
    }

    fn spellcheck(&self) -> bool {
        self.settings.spellcheck
    }

    fn knobs(&self) -> &Knobs {
        &self.knobs
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
        self.mentions.local_paths(query, limit)
    }

    fn active_line_comment_card(&self) -> Option<u64> {
        self.line_comments.active_card
    }

    fn open_line_comment_card(&mut self, key: u64, tx: &mut Tx) {
        App::open_line_comment_card(self, key, tx);
    }

    fn remove_line_comment_card(&mut self, key: u64) {
        App::remove_line_comment_card(self, key);
    }
}
