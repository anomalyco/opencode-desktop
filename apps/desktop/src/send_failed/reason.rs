//! Only describe what the failed request proved. A failed response can follow a successful admission.
pub(super) fn status(error: &str) -> Option<u16> {
    error.split_once("http status: ")?.1.split_whitespace().next()?.parse().ok()
}

/// What an error says when it proves nothing about the prompt (a read that didn't parse, say).
pub(super) const NO_RECEIPT: &str = "no delivery receipt";

pub(super) fn words(error: &str) -> &'static str {
    // Something between the app and OpenCode answered (`opencode_kit::wire_error`): a captive
    // portal or a proxy. It isn't a refusal by OpenCode, so it never reads as one.
    if let Some((_, what)) = error.split_once(opencode_kit::wire_error::NOT_OPENCODE) {
        return if what.contains("redirect") {
            "the network sent it to a sign-in page"
        } else if what.contains("text/html") {
            "the network answered with a web page, not OpenCode"
        } else {
            "the answer didn't come from OpenCode"
        };
    }
    match status(error) {
        Some(401) => "sign in to OpenCode",
        Some(403) => "permission denied",
        Some(404) => "session not found",
        Some(408 | 504) => "server timed out",
        Some(409) => "the session rejected this message",
        Some(413) => "message is too large",
        Some(425 | 429 | 503) => "server is busy",
        Some(500..=599) => "server error",
        Some(400..=499) => "OpenCode rejected this message",
        _ => {
            let e = error.to_ascii_lowercase();
            if e.contains("timeout") || e.contains("timed out") {
                "server didn’t answer in time"
            } else if opencode_kit::plain_error::failed_for_sure(&e) {
                "can’t reach OpenCode"
            } else if opencode_kit::plain_error::is_transient(&e) || e.contains("eof") {
                "connection interrupted"
            } else {
                NO_RECEIPT
            }
        }
    }
}
