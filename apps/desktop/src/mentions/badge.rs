//! Mentions as badges: one rounded chip per `@` mention, in the composer (Rune's `Mark::Pill`
//! atoms) and in a sent prompt's bubble (static pills over its text). Desktop colours agent and
//! file mentions' text (`syntax-type`, `syntax-property`, `syntax-keyword` for references) and
//! gives session mentions a raised chip; here every kind gets a chip, tinted by its colour.

use std::ops::Range;

#[cfg(test)]
use opencode_kit::mention::MentionKind;
use opencode_kit::mention::UserMention;
#[cfg(test)]
use rune::PillStyle;
use rune::Tx;

use crate::App;
use crate::view::title_of;

pub use session_view::user_parts::{
    mention_bubble as bubble, mention_kind_of as kind_of,
    mention_pill_style as pill_style,
};
#[cfg(test)]
use session_view::user_parts::mention_ink as ink;
#[cfg(test)]
use session_view::user_parts::{mention_neutral as neutral, mention_tint as tint};

/// The mentions of a prompt sent from the composer, over its trimmed text (what a pending
/// steer shows until the server's copy replaces it).
pub fn echo<S: AsRef<str>>(text: &str, pills: &[(Range<usize>, S)]) -> Vec<UserMention> {
    let (lead, _, pills) = super::wire::inside_trimmed(text, pills);
    pills.filter_map(|(r, id)| Some(UserMention { range: r.start - lead..r.end - lead, kind: kind_of(id.as_ref())? })).collect()
}

impl App {
    /// A session's title for a mention of it: a loaded session's, or one the `@` search found.
    pub fn mention_title(&self, id: &str) -> Option<String> {
        self.session(id).or_else(|| self.mentions.found_session(id)).map(title_of)
    }

    /// Records the composer's mentions for the prompt about to leave as message `id`, over the
    /// text as typed (`Wire::display`), as its pending steer shows it.
    pub fn echo_mentions(&self, id: &str) {
        let pills: Vec<_> = self.composer.pills().collect();
        opencode_kit::mention::set_user_mentions(id, echo(&self.composer.text, &pills));
    }

    /// Records `Wire::marks` for message `id`, whose echo shows `Wire::text` (session pills as
    /// `@session:` tokens, so the bubble draws them as the server's copy will).
    pub fn echo_wire_mentions(&self, id: &str, marks: Vec<UserMention>) {
        opencode_kit::mention::set_user_mentions(id, marks);
    }

    /// Opens the session a prompt's badge mentions. One the session list hasn't loaded gets a
    /// placeholder in the open session's project, then its real info.
    pub fn open_mentioned(&mut self, id: &str, tx: &mut Tx) {
        if self.session(id).is_none() {
            let found = self.mentions.found_session(id).cloned();
            let here = match &self.route {
                crate::Route::Session(open) => self.session(open).cloned(),
                _ => None,
            };
            let from = here.unwrap_or_default();
            self.sessions.push(found.unwrap_or(opencode_kit::model::SessionInfo {
                id: id.to_owned(),
                project: from.project,
                directory: from.directory,
                ..Default::default()
            }));
            self.fetch_info(id);
        }
        self.open(id, tx);
    }

    /// Message `id`'s rendered bubble text, borrowing `body` when it holds no `@` mentions.
    pub fn bubble_text<'a>(&self, id: &str, body: &'a str) -> std::borrow::Cow<'a, str> {
        if !body.contains('@') {
            return std::borrow::Cow::Borrowed(body);
        }
        std::borrow::Cow::Owned(bubble(body, &opencode_kit::mention::user_mentions(id), |sid| self.mention_title(sid)).0)
    }
}

#[cfg(test)]
#[path = "badge_tests.rs"]
mod tests;
