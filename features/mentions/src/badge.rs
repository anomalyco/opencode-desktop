//! Mentions as badges: one rounded chip per `@` mention, in the composer (Rune's `Mark::Pill`
//! atoms) and in a sent prompt's bubble (static pills over its text). Desktop colours agent and
//! file mentions' text (`syntax-type`, `syntax-property`, `syntax-keyword` for references) and
//! gives session mentions a raised chip; here every kind gets a chip, tinted by its colour.

use std::borrow::Cow;
use std::ops::Range;

use opencode_kit::mention::{self, UserMention};
use opencode_kit::model::SessionInfo;
use rune::Tx;

use crate::{MentionsHost, title_of};

pub use opencode_kit::mention::{
    mention_bubble as bubble, mention_kind_of as kind_of,
};

/// The mentions of a prompt sent from the composer, over its trimmed text (what a pending
/// steer shows until the server's copy replaces it).
pub fn echo<S: AsRef<str>>(text: &str, pills: &[(Range<usize>, S)]) -> Vec<UserMention> {
    let (lead, _, pills) = super::wire::inside_trimmed(text, pills);
    pills.filter_map(|(r, id)| Some(UserMention { range: r.start - lead..r.end - lead, kind: kind_of(id.as_ref())? })).collect()
}

/// A session's title for a mention of it: a loaded session's, or one the `@` search found.
pub fn mention_title(host: &dyn MentionsHost, id: &str) -> Option<String> {
    host.session_info(id).or_else(|| host.mentions().found_session(id)).map(title_of)
}

/// Records the composer's mentions for the prompt about to leave as message `id`, over the
/// text as typed (`Wire::display`), as its pending steer shows it.
pub fn echo_mentions(host: &dyn MentionsHost, id: &str) {
    let composer = host.composer();
    let pills: Vec<_> = composer.pills().collect();
    mention::set_user_mentions(id, echo(&composer.text, &pills));
}

/// Records `Wire::marks` for message `id`, whose echo shows `Wire::text` (session pills as
/// `@session:` tokens, so the bubble draws them as the server's copy will).
pub fn echo_wire_mentions(id: &str, marks: Vec<UserMention>) {
    mention::set_user_mentions(id, marks);
}

/// Opens the session a prompt's badge mentions. One the session list hasn't loaded gets a
/// placeholder in the open session's project, then its real info.
pub fn open_mentioned(host: &mut dyn MentionsHost, id: &str, tx: &mut Tx) {
    if host.session_info(id).is_none() {
        let found = host.mentions().found_session(id).cloned();
        let from = host.active_session_id().and_then(|open| host.session_info(open)).cloned().unwrap_or_default();
        host.push_session_info(found.unwrap_or(SessionInfo {
            id: id.to_owned(),
            project: from.project,
            directory: from.directory,
            ..Default::default()
        }));
        host.fetch_session_info(id);
    }
    host.open_session(id, tx);
}

/// Message `id`'s rendered bubble text, borrowing `body` when it holds no `@` mentions.
pub fn bubble_text<'a>(host: &dyn MentionsHost, id: &str, body: &'a str) -> Cow<'a, str> {
    if !body.contains('@') {
        return Cow::Borrowed(body);
    }
    Cow::Owned(bubble(body, &mention::user_mentions(id), |sid| mention_title(host, sid)).0)
}
