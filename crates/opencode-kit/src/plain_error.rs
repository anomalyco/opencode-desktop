//! A transport error's plain words, for what the UI says. The raw text (`GET /x: timeout:
//! global`) is for logs.

/// Whether the server may simply be busy or briefly gone, so the same read is worth asking again.
/// A 4xx or an unreadable reply won't change on a retry.
pub fn is_transient(raw: &str) -> bool {
    let e = raw.to_lowercase();
    ["timeout", "timed out", "connection", "refused", "broken pipe", "unexpected eof", "stream ended", "http status: 502", "http status: 503", "http status: 504"]
        .iter()
        .any(|k| e.contains(k))
}

/// `raw` (with or without its `GET /path: ` prefix) in words a person can read; an error with
/// no known mapping comes back unchanged.
pub fn plain_error(raw: &str) -> String {
    let e = raw.to_lowercase();
    let has = |ks: &[&str]| ks.iter().any(|k| e.contains(k));
    let words = if has(&["timeout", "timed out"]) {
        "OpenCode didn't answer in time"
    } else if has(&["refused", "connection failed"]) {
        "OpenCode isn't running"
    } else if has(&["connection reset", "connection aborted", "broken pipe", "unexpected eof", "stream ended"]) {
        "OpenCode dropped the connection"
    } else if has(&["failed to lookup", "dns", "no route to host", "network is unreachable"]) {
        "Can't reach the OpenCode server"
    } else {
        return raw.to_owned();
    };
    words.to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn library_errors_become_plain_words() {
        assert_eq!(plain_error("GET /api/session: timeout: global"), "OpenCode didn't answer in time");
        assert_eq!(plain_error("io: Connection refused (os error 61)"), "OpenCode isn't running");
        assert_eq!(plain_error("events: stream ended"), "OpenCode dropped the connection");
        assert_eq!(plain_error("http status: 404"), "http status: 404");
    }

    #[test]
    fn only_a_busy_or_gone_server_is_worth_asking_again() {
        assert!(is_transient("GET /x: timeout: global"));
        assert!(is_transient("GET /x: io: Connection refused (os error 61)"));
        assert!(is_transient("GET /x: http status: 503"));
        assert!(!is_transient("GET /x: http status: 404"));
        assert!(!is_transient("GET /x: expected value at line 1"));
    }
}
