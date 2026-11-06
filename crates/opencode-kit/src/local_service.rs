//! Discovers the local OpenCode background service when `OPENCODE_SERVER_PASSWORD` is unset, where
//! OpenCode's own clients look (`@opencode/client` `service-probe.ts`, Desktop's
//! `background-service.ts`): the service's registration `<state>/opencode/service.json` (`url`,
//! `password`), with `<state>` = `XDG_STATE_HOME`, else `<home>/.local/state` on every platform
//! and `<home>` = `USERPROFILE` on Windows; `service-local.json` there for Desktop's local channel
//! (`OPENCODE_DESKTOP_SERVER_CHANNEL=local`). The CLI's own config (`hostname`, `port`,
//! `password` in `OPENCODE_CONFIG_DIR`, else `<config>/opencode/service.json`) comes after, for a
//! service that hasn't registered.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::client::{DEFAULT_SERVER, Server};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LocalService {
    pub base: String,
    pub port: u16,
    pub password: Option<String>,
}

/// Parses a `service.json` written by `opencode serve --service`: the registration (`url`) or the
/// CLI's config (`hostname`, `port`).
pub fn parse_service_json(text: &str) -> Option<LocalService> {
    let v: Value = serde_json::from_str(text).ok()?;
    let (raw_host, port) = match v.get("url").and_then(Value::as_str) {
        Some(url) => {
            let authority = url.trim().strip_prefix("http://")?.split('/').next()?;
            let (host, port) = authority.rsplit_once(':')?;
            (host, port.parse::<u64>().ok()?)
        }
        None => (v.get("hostname").and_then(Value::as_str).unwrap_or("127.0.0.1"), v.get("port")?.as_u64()?),
    };
    let port = u16::try_from(port).ok().filter(|p| *p > 0)?;
    let host = match raw_host.trim() {
        "" | "0.0.0.0" | "localhost" | "127.0.0.1" => "127.0.0.1",
        "::1" | "[::1]" => "[::1]",
        _ => return None,
    };
    let password = v
        .get("password")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned);
    Some(LocalService { base: format!("http://{host}:{port}"), port, password })
}

/// The files to try, in order, from an environment lookup (see the module docs).
fn candidate_paths(var: impl Fn(&str) -> Option<OsString>) -> Vec<PathBuf> {
    let set = |k: &str| var(k).filter(|v| !v.is_empty()).map(PathBuf::from);
    let home = rune::dirs::home_from(&var);
    let xdg = |k: &str, fallback: &[&str]| set(k).or_else(|| Some(fallback.iter().fold(home.clone()?, |p, part| p.join(part))));
    let local = var("OPENCODE_DESKTOP_SERVER_CHANNEL").is_some_and(|c| c == "local");
    let registration = if local { "service-local.json" } else { "service.json" };
    [
        xdg("XDG_STATE_HOME", &[".local", "state"]).map(|d| d.join("opencode").join(registration)),
        set("OPENCODE_CONFIG_DIR").map(|d| d.join("service.json")),
        xdg("XDG_CONFIG_HOME", &[".config"]).map(|d| d.join("opencode").join("service.json")),
    ]
    .into_iter()
    .flatten()
    .collect()
}

pub fn read_local_service_from(paths: &[PathBuf]) -> Option<LocalService> {
    paths.iter().find_map(|p| read_one(p))
}

fn read_one(path: &Path) -> Option<LocalService> {
    let text = std::fs::read_to_string(path).ok()?;
    parse_service_json(&text)
}

/// Reads the first valid `service.json` where OpenCode's clients look.
pub fn read_local_service() -> Option<LocalService> {
    if cfg!(test) {
        return None;
    }
    read_local_service_from(&candidate_paths(|k| std::env::var_os(k)))
}

fn local_port(base: &str) -> Option<u16> {
    let (scheme, rest) = base.trim_end_matches('/').split_once("://")?;
    if scheme != "http" {
        return None;
    }
    let host_port = rest.split('/').next()?;
    let (host, port) = host_port.rsplit_once(':')?;
    if !matches!(host, "127.0.0.1" | "localhost" | "[::1]") {
        return None;
    }
    port.parse::<u16>().ok()
}

/// Returns the `service.json` password when `base` targets the same local port.
pub fn password_for_base(base: &str) -> Option<String> {
    let port = local_port(base)?;
    let svc = read_local_service()?;
    (svc.port == port).then_some(svc.password).flatten()
}

fn probe_usable(server: &Server) -> bool {
    if !server.reachable() {
        return false;
    }
    let mut req = Server::agent(Some(std::time::Duration::from_millis(500)))
        .get(format!("{}/api/project", server.base));
    if let Some(auth) = &server.auth {
        req = req.header("Authorization", auth);
    }
    req.call()
        .ok()
        .and_then(|r| r.into_body().read_to_string().ok())
        .and_then(|s| serde_json::from_str::<Value>(&s).ok())
        .is_some()
}

fn discover_with_service(default: &Server, default_base: &str, svc: Option<LocalService>) -> Server {
    if default.base != default_base {
        return default.clone();
    }
    let Some(svc) = svc.filter(|s| s.base != default_base) else {
        return default.clone();
    };
    if probe_usable(default) {
        return default.clone();
    }
    let mut candidate = Server::new(&svc.base);
    if let Some(pw) = &svc.password {
        candidate = candidate.with_password(pw);
    }
    if default.media_refs {
        candidate = candidate.with_media_refs();
    }
    if candidate.reachable() {
        return candidate;
    }
    default.clone()
}

/// Returns `default` when usable; otherwise, when `default` is `http://127.0.0.1:4096`, checks
/// whether a local OpenCode Desktop sidecar in `service.json` is listening on another port.
pub fn discover_local_server(default: &Server) -> Server {
    discover_with_service(default, DEFAULT_SERVER, read_local_service())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_local_service_json() {
        let svc = parse_service_json(r#"{"hostname":"127.0.0.1","port":4096,"password":" secret "}"#).unwrap();
        assert_eq!(svc.base, "http://127.0.0.1:4096");
        assert_eq!(svc.port, 4096);
        assert_eq!(svc.password.as_deref(), Some("secret"));
    }

    #[test]
    fn parses_passwordless_and_rejects_remote_hosts() {
        let open = parse_service_json(r#"{"hostname":"localhost","port":43120,"password":""}"#).unwrap();
        assert_eq!(open.base, "http://127.0.0.1:43120");
        assert_eq!(open.password, None);
        assert!(parse_service_json(r#"{"hostname":"example.com","port":4096}"#).is_none());
        assert!(parse_service_json(r#"{"hostname":"127.0.0.1","port":0}"#).is_none());
    }

    #[test]
    fn local_port_matches_loopback_http_only() {
        assert_eq!(local_port("http://127.0.0.1:4096"), Some(4096));
        assert_eq!(local_port("http://localhost:4096/"), Some(4096));
        assert_eq!(local_port("https://host.example.ts.net:4096"), None);
    }

    #[test]
    fn falls_back_to_service_json_when_default_port_is_unauthorized() {
        use std::io::{Read, Write};
        use std::net::TcpListener;

        let blocked = TcpListener::bind("127.0.0.1:0").unwrap();
        let blocked_base = format!("http://{}", blocked.local_addr().unwrap());
        let sidecar = TcpListener::bind("127.0.0.1:0").unwrap();
        let sidecar_port = sidecar.local_addr().unwrap().port();
        let sidecar_base = format!("http://127.0.0.1:{sidecar_port}");

        std::thread::spawn(move || {
            for stream in blocked.incoming().take(2).flatten() {
                let mut s = stream;
                let _ = s.set_read_timeout(Some(std::time::Duration::from_millis(50)));
                let mut buf = [0u8; 512];
                let n = s.read(&mut buf).unwrap_or(0);
                if n > 0 {
                    let _ = s.write_all(b"HTTP/1.1 401 Unauthorized\r\nContent-Length: 0\r\n\r\n");
                }
            }
        });

        let default = Server::new(&blocked_base).with_media_refs();
        let svc = LocalService {
            base: sidecar_base.clone(),
            port: sidecar_port,
            password: Some("sidecar-pw".into()),
        };
        let picked = discover_with_service(&default, &blocked_base, Some(svc));
        assert_eq!(picked.base, sidecar_base);
        assert!(picked.media_refs);
        assert!(picked.auth.is_some());
    }
}
