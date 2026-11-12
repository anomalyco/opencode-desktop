//! What the UI says about a failure: what happened, why, what to do next, and the details to
//! copy (the call, the server, its status and its own words), as one model every surface shows
//! (`status_toast::error`, the Files panel's failed read). Built from the client's typed error
//! (`opencode_kit::wire_error::WireError`), so OpenCode's own explanation is kept and a captive
//! portal is named for what it is.
//!
//! Wording: plain English, short, no blame; where Desktop's title is right it stays (the caller
//! passes it as `what`).

use opencode_kit::wire_error::{WireError, WireKind};

#[derive(Clone, Debug, PartialEq)]
pub struct UserError {
    /// The headline: what didn't happen ("Couldn't rename session").
    pub what: String,
    /// Why, naming the server and its own words when it gave them.
    pub why: String,
    /// What to do now, when the button isn't it.
    pub next: Option<String>,
    /// What "Copy details" puts on the clipboard.
    pub details: String,
}

impl UserError {
    /// `e`, from a call to `server` (its name as the app shows it), headlined `what`.
    pub fn from_wire(what: &str, server: &str, e: &WireError) -> UserError {
        let (why, next): (String, Option<&str>) = match &e.kind {
            WireKind::Unreachable(_) => (format!("{server} isn't answering."), Some("Check that it's running and reachable, then try again.")),
            WireKind::TimedOut => (format!("{server} didn't answer in time."), Some("It may still be working on it. Try again in a moment.")),
            WireKind::Dropped(_) => (format!("The connection to {server} dropped before it answered."), Some("It may have gone through: check before you try again.")),
            WireKind::Tls(detail) => (format!("The secure connection to {server} failed ({detail})."), Some("Check the server's certificate.")),
            WireKind::Redirected { location, .. } => (
                match location {
                    Some(to) => format!("Something on the network answered instead of {server}, and sent the app to {to}."),
                    None => format!("Something on the network answered instead of {server}."),
                },
                Some("If this network has a sign-in page, sign in, then try again."),
            ),
            WireKind::NotOpenCode { what, .. } => (format!("Something on the network answered instead of {server}: {what}."), Some("If this network has a sign-in page, sign in, then try again.")),
            WireKind::Status { status: 401, .. } => (format!("{server} asks this app to sign in again."), Some("Sign in to the server, then try again.")),
            WireKind::Status { status: 403, message, .. } => (message.clone().unwrap_or_else(|| format!("{server} doesn't allow this.")), None),
            WireKind::Status { status, message: Some(m), .. } if *status >= 500 => (format!("{server} hit an error: {}", stopped(m)), Some("Try again; if it keeps failing, check the server's logs.")),
            WireKind::Status { status, message: None, .. } if *status >= 500 => (format!("{server} hit an error (HTTP {status})."), Some("Try again; if it keeps failing, check the server's logs.")),
            WireKind::Status { message: Some(m), .. } => (sentence(m), None),
            WireKind::Status { status, message: None, .. } => (format!("{server} refused it (HTTP {status})."), None),
            WireKind::Shape(m) | WireKind::Other(m) => (sentence(m), None),
        };
        let details = format!("Action: {what}\nServer: {server}\nCall: {} {}\nError: {}", e.method, e.path, e.kind);
        UserError { what: what.to_owned(), why, next: next.map(str::to_owned), details }
    }

    /// The toast's description: why, then what to do.
    pub fn description(&self) -> String {
        match &self.next {
            Some(next) => format!("{} {next}", self.why),
            None => self.why.clone(),
        }
    }
}

/// `m` ending with a stop, as the middle of a sentence.
fn stopped(m: &str) -> String {
    let m = m.trim();
    if m.ends_with(['.', '!', '?']) { m.to_owned() } else { format!("{m}.") }
}

/// `m` as a sentence: capitalised, ending with a stop.
fn sentence(m: &str) -> String {
    let m = m.trim();
    let mut out: String = m.chars().take(1).flat_map(char::to_uppercase).chain(m.chars().skip(1)).collect();
    if !out.ends_with(['.', '!', '?']) {
        out.push('.');
    }
    out
}
