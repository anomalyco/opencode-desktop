//! Slack permalinks in prompts (`session_view::slack`): each read once from the live server
//! (`GET /api/experimental/console/slack/message`), or from `fixture.json`'s `slack` in a fixture
//! run, and kept for the window's life. A read that fails leaves the chip alone, with no card.

use opencode_kit::slack::{Lookup, Reason};

use crate::{App, Source};

mod stories;

impl App {
    /// Starts the reads bubbles asked for; true when one answered at once.
    pub(crate) fn poll_slack_links(&mut self) -> bool {
        let mut changed = false;
        for url in self.slack_links.take_fetches() {
            if matches!(*self.source, Source::Live { .. }) {
                self.job(move |server| {
                    let lookup = server.slack_message(&url).unwrap_or_else(|e| Lookup::Unavailable(Reason::Failed(e.to_string())));
                    Box::new(move |s: &mut App, _| {
                        s.slack_links.answer(&url, lookup);
                    })
                });
            } else {
                let lookup = opencode_kit::slack::lookup(&self.fixture["slack"][url.as_str()]).unwrap_or(Lookup::Unavailable(Reason::NotFound));
                changed |= self.slack_links.answer(&url, lookup);
            }
        }
        changed
    }
}
