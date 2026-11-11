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

