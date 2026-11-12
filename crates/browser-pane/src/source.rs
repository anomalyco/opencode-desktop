//! Where a new engine comes from and which engines this build accepts: the two inputs a release
//! sets, [`SOURCE`] and [`TRUST`], and the release this platform picks from the source's index.
//!
//! The index has the release service's manifest shape (`apps/browser-engine/scripts/
//! package-windows.ps1` writes one): `release.version`, and per platform
//! `release.targets.<os>-<arch>` with the archive's `url` (relative to the index, or absolute),
//! `size` and `sha256`.

use std::io::Read;
use std::path::PathBuf;
use std::time::Duration;

use serde_json::Value;

use crate::installer::{ERR_DOWNLOAD, ERR_NO_SOURCE, Version, err_blocked, err_untrusted};

// TODO(IMPORTANT before release): the engine's delivery and trust root are decided when the app
// moves into the opencode repo: set OPENCODE_BROWSER_ENGINE_SOURCE (the index's URL) and
// OPENCODE_BROWSER_ENGINE_TRUST (`publisher:<signer>` or `sha256:<hex>,...`) for the release build.
// Until then a development build reads the defaults below.
/// The index's URL: `https://`, `http://` (loopback only, unless the trust rule is a publisher or
/// hashes), or `file:`. Set when the app is built (`OPENCODE_BROWSER_ENGINE_SOURCE`).
pub const SOURCE: &str = match option_env!("OPENCODE_BROWSER_ENGINE_SOURCE") {
    Some(s) => s,
    None => DEV_SOURCE,
};
/// Which archives install (`OPENCODE_BROWSER_ENGINE_TRUST`, set when the app is built):
/// - `publisher:<name>`: every program and library in it is signed by `<name>` (Windows: a
///   trusted Authenticode signature whose certificate names it; macOS: the Developer ID team).
/// - `sha256:<hex>[,<hex>...]`: the archive is one of these.
/// - `local`: whatever a loopback or `file:` source lists, checked against the index's size and
///   SHA-256 (a development build serving its own package).
pub const TRUST: &str = match option_env!("OPENCODE_BROWSER_ENGINE_TRUST") {
    Some(t) => t,
    None => DEV_TRUST,
};

/// Windows: `bun apps/browser-engine/scripts/serve.ts` serves a local package here.
#[cfg(windows)]
const DEV_SOURCE: &str = "http://127.0.0.1:47963/index.json";
#[cfg(windows)]
const DEV_TRUST: &str = "local";
/// macOS: the one engine published so far, pinned by its hash.
#[cfg(target_os = "macos")]
const DEV_SOURCE: &str = "https://anoma.ly/release/opencode-rune/browser-engine/stable.json";
#[cfg(target_os = "macos")]
const DEV_TRUST: &str = "sha256:92291cae38cec0cf280e64ac3eb43cb77e0b44cdd3f304279a050fa8c69cfa18";
#[cfg(not(any(windows, target_os = "macos")))]
const DEV_SOURCE: &str = "";
#[cfg(not(any(windows, target_os = "macos")))]
const DEV_TRUST: &str = "";

/// This platform's target in the index.
const TARGET: &str = if cfg!(all(windows, target_arch = "aarch64")) {
    "windows-arm64"
} else if cfg!(windows) {
    "windows-x64"
} else if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
    "macos-arm64"
} else {
    "unsupported"
};

/// How an archive earns its install.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Trust {
    Local,
    Sha256(Vec<String>),
    Publisher(String),
}

/// The release the index offers this platform.
#[derive(Clone, Debug)]
pub(crate) struct Release {
    pub version: Version,
    pub url: String,
    pub size: u64,
    pub sha256: String,
    pub trust: Trust,
}

/// The source and trust rule in force: the build's, or in a development build
/// `OPENCODE_BROWSER_ENGINE_SOURCE` and `OPENCODE_BROWSER_ENGINE_TRUST` from the environment (a
/// harness's stand-in on another port).
fn inputs() -> (String, String) {
    let runtime = |name: &str| cfg!(debug_assertions).then(|| std::env::var_os(name)).flatten().and_then(|v| v.into_string().ok());
    (
        runtime("OPENCODE_BROWSER_ENGINE_SOURCE").unwrap_or_else(|| SOURCE.to_owned()),
        runtime("OPENCODE_BROWSER_ENGINE_TRUST").unwrap_or_else(|| TRUST.to_owned()),
    )
}

/// Whether this build has a source at all (else the pane says the browser isn't available).
pub(crate) fn configured() -> bool {
    !inputs().0.trim().is_empty()
}

/// Reads the index and picks this platform's release: one the build accepts (its engine
/// revision) from a source its trust rule allows.
pub(crate) fn latest() -> Result<Release, String> {
    let (source, trust) = inputs();
    let source = source.trim();
    if source.is_empty() {
        return Err(ERR_NO_SOURCE.into());
    }
    let trust = parse_trust(&trust).ok_or_else(|| err_untrusted("this build has no trust rule"))?;
    if trust == Trust::Local && !is_local(source) {
        return Err(err_untrusted("a remote source needs a publisher or hashes to trust"));
    }
    let index: Value = serde_json::from_slice(&fetch_small(source)?).map_err(|_| ERR_DOWNLOAD.to_owned())?;
    let release = &index["release"];
    let version = release["version"].as_str().and_then(Version::parse).ok_or_else(|| ERR_NO_SOURCE.to_owned())?;
    let target = &release["targets"][TARGET];
    let (Some(url), Some(size), Some(sha256)) = (target["url"].as_str(), target["size"].as_u64(), target["sha256"].as_str()) else {
        return Err(ERR_NO_SOURCE.into());
    };
    if !version.accepted() {
        return Err(ERR_NO_SOURCE.into());
    }
    let sha256 = sha256.to_ascii_lowercase();
    if let Trust::Sha256(allowed) = &trust
        && !allowed.contains(&sha256)
    {
        return Err(err_untrusted("its hash isn't one this build accepts"));
    }
    Ok(Release { version, url: resolve(source, url), size, sha256, trust })
}

fn parse_trust(rule: &str) -> Option<Trust> {
    let rule = rule.trim();
    if rule == "local" {
        return Some(Trust::Local);
    }
    if let Some(hashes) = rule.strip_prefix("sha256:") {
        let list: Vec<String> = hashes.split(',').map(|h| h.trim().to_ascii_lowercase()).filter(|h| h.len() == 64).collect();
        return (!list.is_empty()).then_some(Trust::Sha256(list));
    }
    rule.strip_prefix("publisher:").map(str::trim).filter(|p| !p.is_empty()).map(|p| Trust::Publisher(p.to_owned()))
}

/// A `file:` source, or `http://` on this machine.
fn is_local(url: &str) -> bool {
    if url.starts_with("file:") {
        return true;
    }
    let Some(rest) = url.strip_prefix("http://") else { return false };
    let host = rest.split(['/', '?']).next().unwrap_or_default();
    let host = host.rsplit_once(':').filter(|(h, port)| !h.is_empty() && port.bytes().all(|b| b.is_ascii_digit())).map_or(host, |(h, _)| h);
    matches!(host, "127.0.0.1" | "localhost" | "[::1]")
}

/// `url` as the index names it, against the index's own URL.
fn resolve(index: &str, url: &str) -> String {
    if url.contains("://") || url.starts_with("file:") {
        return url.to_owned();
    }
    if let Some(path) = url.strip_prefix('/') {
        let origin_end = index.find("://").map_or(0, |i| i + 3);
        let origin = index[origin_end..].find('/').map_or(index, |slash| &index[..origin_end + slash]);
        return format!("{origin}/{path}");
    }
    let dir = index.rfind('/').map_or(index, |slash| &index[..=slash]);
    format!("{dir}{url}")
}

/// A `file:` URL's path.
pub(crate) fn file_path(url: &str) -> Option<PathBuf> {
    let rest = url.strip_prefix("file:")?;
    let rest = rest.strip_prefix("//").unwrap_or(rest);
    // `file:///C:/x` on Windows.
    let rest = if cfg!(windows) { rest.strip_prefix('/').filter(|r| r.as_bytes().get(1) == Some(&b':')).unwrap_or(rest) } else { rest };
    Some(PathBuf::from(percent_decode(rest)))
}

fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let hex = |b: u8| (b as char).to_digit(16);
        if bytes[i] == b'%'
            && let (Some(h), Some(l)) = (bytes.get(i + 1).copied().and_then(hex), bytes.get(i + 2).copied().and_then(hex))
        {
            out.push((h * 16 + l) as u8);
            i += 3;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// The index, whole (it's a few hundred bytes; at most 1 MB is read).
fn fetch_small(url: &str) -> Result<Vec<u8>, String> {
    let mut body = Vec::new();
    if let Some(path) = file_path(url) {
        std::fs::File::open(path).and_then(|f| f.take(1 << 20).read_to_end(&mut body)).map_err(|_| ERR_NO_SOURCE.to_owned())?;
        return Ok(body);
    }
    let agent = crate::download::agent(Duration::from_secs(15));
    let mut resp = match agent.get(url).call() {
        Ok(resp) => resp,
        Err(ureq::Error::StatusCode(404)) => return Err(ERR_NO_SOURCE.into()),
        Err(ureq::Error::StatusCode(code)) if crate::download::blocked(code) => return Err(err_blocked(code)),
        Err(_) => return Err(ERR_DOWNLOAD.into()),
    };
    resp.body_mut().as_reader().take(1 << 20).read_to_end(&mut body).map_err(|_| ERR_DOWNLOAD.to_owned())?;
    Ok(body)
}
