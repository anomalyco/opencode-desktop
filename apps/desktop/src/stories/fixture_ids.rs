//! Ids in `assets/fixture.json` that stories, gates and tests name.

#![cfg_attr(not(feature = "stories"), allow(dead_code))]

pub const MAIN: &str = "ses_fx0001aaaaaaaaaaaaaaaaaa";
pub const CHILD: &str = "ses_fx0002aaaaaaaaaaaaaaaaaa";
pub const THIRD: &str = "ses_fx0003aaaaaaaaaaaaaaaaaa";
pub const SIXTH: &str = "ses_fx0006aaaaaaaaaaaaaaaaaa";
/// Kiln's tour of the tools: one call of every kind, and two that failed (`tools-*` scenes).
pub const TOUR: &str = "ses_fx0020aaaaaaaaaaaaaaaaaa";
/// The main session's first prompt.
pub const PROMPT: &str = "msg_001a162c32a80001fixture00";
/// The main session's last prompt.
pub const LAST_PROMPT: &str = "msg_001a162cb67e0006fixture00";
/// The row key of the main session's reply with the watchdog's code.
pub const REPLY_ROW: &str = "t-msg_001a162c36130003fixture00-0";
