//! Ids in `assets/fixture.json` that stories, gates and tests name.

pub const MAIN: &str = "ses_fx0001aaaaaaaaaaaaaaaaaa";
pub const CHILD: &str = "ses_fx0002aaaaaaaaaaaaaaaaaa";
pub const THIRD: &str = "ses_fx0003aaaaaaaaaaaaaaaaaa";
pub const SIXTH: &str = "ses_fx0006aaaaaaaaaaaaaaaaaa";
/// Kiln's tour of the tools: one call of every kind, and two that failed (`tools-*` scenes).
pub const TOUR: &str = "ses_fx0020aaaaaaaaaaaaaaaaaa";
/// Kiln's sessions whose last step failed: a spent free allowance, Go limit, workspace budget,
/// model budget, and a plain error; one the server is retrying; and one with every notice.
#[cfg(test)]
pub const FREE_LIMIT: &str = "ses_fx0013aaaaaaaaaaaaaaaaaa";
#[cfg(test)]
pub const GO_LIMIT: &str = "ses_fx0014aaaaaaaaaaaaaaaaaa";
#[cfg(test)]
pub const BUDGET: &str = "ses_fx0015aaaaaaaaaaaaaaaaaa";
#[cfg(test)]
pub const MODEL_BUDGET: &str = "ses_fx0016aaaaaaaaaaaaaaaaaa";
#[cfg(test)]
pub const ERRORED: &str = "ses_fx0017aaaaaaaaaaaaaaaaaa";
#[cfg(test)]
pub const RETRYING: &str = "ses_fx0018aaaaaaaaaaaaaaaaaa";
#[cfg(test)]
pub const NOTICES: &str = "ses_fx0019aaaaaaaaaaaaaaaaaa";
/// The main session's first prompt.
pub const PROMPT: &str = "msg_001a162c32a80001fixture00";
/// The row key of the main session's reply with the watchdog's code.
pub const REPLY_ROW: &str = "t-msg_001a162c36130003fixture00-0";
