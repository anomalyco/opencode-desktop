//! Connect links from `opencode pair`: `<server>/auth/connect/<code>`, where the code is
//! single-use and short-lived. Redeeming one asks the server for JSON and gets back a session
//! token, which is then the server's password (`Server::with_password`).

use std::time::Duration;

use serde_json::Value;

/// A pasted connect link split into the server's address and its code.
#[derive(Debug, PartialEq)]
pub struct Link {
    pub server: String,
    pub code: String,
}

/// Reads a connect link, or None when `value` isn't one. The server is the link's origin.
pub fn parse(value: &str) -> Option<Link> {
    let value = value.trim();
    let (scheme, rest) = value.split_once("://")?;
    if scheme != "http" && scheme != "https" {
        return None;
    }
    let rest = rest.split(['?', '#']).next().unwrap_or("");
    let (host, path) = rest.split_once('/')?;
    if host.is_empty() || host.contains('@') {
        return None;
    }
    let code = path.strip_prefix("auth/connect/")?.trim_end_matches('/');
    if code.is_empty() || !code.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') {
        return None;
    }
    Some(Link { server: format!("{scheme}://{host}"), code: code.to_owned() })
}

/// Trades the link's code for a session token. The code works once: a second try, or one after
/// it expires, is refused.
pub fn redeem(link: &Link) -> Result<String, String> {
    let agent: ureq::Agent = ureq::Agent::config_builder().timeout_global(Some(Duration::from_secs(10))).http_status_as_error(false).build().into();
    let mut resp = agent
        .get(format!("{}/auth/connect/{}", link.server, link.code))
        .header("Accept", "application/json")
        .call()
        .map_err(|e| format!("Can't reach {}: {e}", link.server))?;
    match resp.status().as_u16() {
        200 => {}
        401 => return Err("That link has expired or was already used. Make a new one with `opencode pair`.".into()),
        n => return Err(format!("{} answered {n}", link.server)),
    }
    let text = resp.body_mut().read_to_string().map_err(|e| format!("Can't read {}'s answer: {e}", link.server))?;
    let body: Value = serde_json::from_str(&text).map_err(|e| format!("{} sent something unexpected: {e}", link.server))?;
    body["token"].as_str().filter(|t| !t.is_empty()).map(str::to_owned).ok_or_else(|| format!("{} sent no token", link.server))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};

    fn answer_once(reply: &'static str) -> (String, std::thread::JoinHandle<String>) {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let h = std::thread::spawn(move || {
            let (mut s, _) = listener.accept().unwrap();
            let mut buf = [0u8; 4096];
            let n = s.read(&mut buf).unwrap_or(0);
            s.write_all(reply.as_bytes()).unwrap();
            String::from_utf8_lossy(&buf[..n]).into_owned()
        });
        (base, h)
    }

    #[test]
    fn reads_a_connect_link() {
        assert_eq!(
            parse("  https://abc.example.com/auth/connect/Xy_9-z?x=1\n"),
            Some(Link { server: "https://abc.example.com".into(), code: "Xy_9-z".into() })
        );
        assert_eq!(parse("http://127.0.0.1:49374/auth/connect/abc").unwrap().server, "http://127.0.0.1:49374");
        assert_eq!(parse("http://127.0.0.1:4096"), None);
        assert_eq!(parse("http://127.0.0.1:4096/connect#abc"), None, "the old link shape");
        assert_eq!(parse("ftp://x/auth/connect/abc"), None);
        assert_eq!(parse("http://u:p@x/auth/connect/abc"), None);
        assert_eq!(parse("http://x/auth/connect/a b"), None);
    }

    #[test]
    fn redeems_the_code_for_a_token() {
        let (base, h) = answer_once("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 17\r\n\r\n{\"token\":\"tok_1\"}");
        let link = Link { server: base, code: "abc".into() };
        assert_eq!(redeem(&link).unwrap(), "tok_1");
        let req = h.join().unwrap();
        assert!(req.starts_with("GET /auth/connect/abc HTTP/1.1"), "{req}");
        assert!(req.to_ascii_lowercase().contains("accept: application/json"), "{req}");
    }

    #[test]
    fn a_spent_code_says_so() {
        let (base, _h) = answer_once("HTTP/1.1 401 Unauthorized\r\nContent-Length: 0\r\n\r\n");
        let err = redeem(&Link { server: base, code: "abc".into() }).unwrap_err();
        assert!(err.contains("expired"), "{err}");
    }
}
