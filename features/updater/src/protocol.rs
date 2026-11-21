//! Anomaly Release protocol v1 (`https://anoma.ly/release`) manifest parser and update decision
//! rules. Refuses version downgrades (`Decision::DowngradeRefused`) when both running and manifest
//! versions parse as SemVer (including `-dev-<N>` / `-beta.<N>` prerelease ordinals).

use std::cmp::Ordering;

use serde_json::Value;

#[cfg_attr(not(any(target_os = "macos", windows)), allow(dead_code))]
pub const DEFAULT_SERVICE_URL: &str = "https://anoma.ly/release";
pub const APP_NAME: &str = "opencode";
pub const ARTIFACT_NAME: &str = "desktop-rune";
pub const TARGET_MACOS_ARM64: &str = "macos-arm64";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Target {
    pub url: String,
    pub size: u64,
    pub sha256: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Release {
    pub version: String,
    pub notes_url: Option<String>,
    pub rollout_percent: u8,
    pub required: bool,
    pub target: Option<Target>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Manifest {
    pub app: String,
    pub artifact: String,
    pub channel: String,
    pub sequence: u64,
    pub release: Release,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Decision {
    UpToDate,
    NoTarget,
    DowngradeRefused { version: String },
    RolloutDeferred { version: String },
    Install { version: String, target: Target },
}

pub fn manifest_url(service_base: &str, channel: &str) -> String {
    format!("{}/{APP_NAME}/{ARTIFACT_NAME}/{channel}.json", service_base.trim_end_matches('/'))
}

/// Resolves a manifest target URL (typically `/release/<app>/files/<sha256>/<name>`) against the
/// origin of `manifest_url`, refusing non-HTTPS URLs (except loopback `http://` in debug builds).
pub fn resolve_target_url(manifest_url: &str, target_url: &str) -> Result<String, String> {
    if !super::feed::allowed_update_url(manifest_url) {
        return Err(format!("refusing insecure manifest URL: {manifest_url}"));
    }
    if target_url.starts_with("https://") || target_url.starts_with("http://") {
        if !super::feed::allowed_update_url(target_url) {
            return Err(format!("refusing insecure target URL: {target_url}"));
        }
        return Ok(target_url.to_owned());
    }
    if !target_url.starts_with('/') || target_url.starts_with("//") {
        return Err(format!("unexpected relative target URL: {target_url}"));
    }
    let (scheme, rest) = manifest_url
        .split_once("://")
        .ok_or_else(|| format!("invalid manifest URL: {manifest_url}"))?;
    let host = rest
        .split('/')
        .next()
        .filter(|h| !h.is_empty())
        .ok_or_else(|| format!("missing host in manifest URL: {manifest_url}"))?;
    let resolved = format!("{scheme}://{host}{target_url}");
    if !super::feed::allowed_update_url(&resolved) {
        return Err(format!("refusing insecure target URL: {resolved}"));
    }
    Ok(resolved)
}

pub fn parse_manifest(text: &str, manifest_url: &str, target_name: &str) -> Result<Manifest, String> {
    let v: Value = serde_json::from_str(text).map_err(|e| format!("malformed manifest JSON: {e}"))?;
    if v.get("schema").and_then(Value::as_u64) != Some(1) {
        return Err("unsupported manifest schema".into());
    }
    let str_field = |obj: &Value, key: &str| -> Result<String, String> {
        obj.get(key)
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
            .ok_or_else(|| format!("manifest missing {key}"))
    };
    let app = str_field(&v, "app")?;
    let artifact = str_field(&v, "artifact")?;
    let channel = str_field(&v, "channel")?;
    let sequence = v.get("sequence").and_then(Value::as_u64).ok_or_else(|| "manifest missing sequence".to_owned())?;
    let rel = v.get("release").ok_or_else(|| "manifest missing release".to_owned())?;
    let version = str_field(rel, "version")?;
    let rollout_u64 = rel
        .get("rollout_percent")
        .and_then(Value::as_u64)
        .ok_or_else(|| "manifest missing rollout_percent".to_owned())?;
    let rollout_percent = u8::try_from(rollout_u64)
        .ok()
        .filter(|p| *p <= 100)
        .ok_or_else(|| "manifest rollout_percent out of range".to_owned())?;
    let required = match rel.get("required") {
        None => false,
        Some(Value::Bool(true)) => true,
        _ => return Err("manifest release.required must be true when present".into()),
    };
    let notes_url = rel.get("notes_url").and_then(Value::as_str).map(str::to_owned);
    let target = match rel.get("targets").and_then(|t| t.get(target_name)) {
        Some(t) => {
            let raw_url = str_field(t, "url")?;
            let url = resolve_target_url(manifest_url, &raw_url)?;
            let size = t.get("size").and_then(Value::as_u64).ok_or_else(|| "target missing size".to_owned())?;
            let sha256 = str_field(t, "sha256")?.to_ascii_lowercase();
            if sha256.len() != 64 || !sha256.bytes().all(|b| b.is_ascii_hexdigit()) {
                return Err("target sha256 must be 64 hex chars".into());
            }
            Some(Target { url, size, sha256 })
        }
        None => None,
    };
    Ok(Manifest {
        app,
        artifact,
        channel,
        sequence,
        release: Release { version, notes_url, rollout_percent, required, target },
    })
}

pub fn decide(
    manifest: &Manifest,
    expected_channel: &str,
    running_version: &str,
    installed_sha256: Option<&str>,
    rollout_bucket: u8,
) -> Result<Decision, String> {
    if manifest.app != APP_NAME {
        return Err(format!("wrong app in manifest: {}", manifest.app));
    }
    if manifest.artifact != ARTIFACT_NAME {
        return Err(format!("wrong artifact in manifest: {}", manifest.artifact));
    }
    if manifest.channel != expected_channel {
        return Err(format!("wrong channel in manifest: {}", manifest.channel));
    }
    let Some(target) = &manifest.release.target else {
        return Ok(Decision::NoTarget);
    };
    if version_is_older(&manifest.release.version, running_version) {
        return Ok(Decision::DowngradeRefused { version: manifest.release.version.clone() });
    }
    let already_running = match installed_sha256 {
        Some(sha) if !sha.is_empty() => sha.eq_ignore_ascii_case(&target.sha256),
        _ => manifest.release.version == running_version,
    };
    if already_running {
        return Ok(Decision::UpToDate);
    }
    if !manifest.release.required
        && (manifest.release.rollout_percent == 0 || rollout_bucket >= manifest.release.rollout_percent)
    {
        return Ok(Decision::RolloutDeferred { version: manifest.release.version.clone() });
    }
    Ok(Decision::Install { version: manifest.release.version.clone(), target: target.clone() })
}

fn parse_version(raw: &str) -> Option<([u64; 3], Option<&str>)> {
    let trimmed = raw.trim().strip_prefix(['v', 'V']).unwrap_or_else(|| raw.trim());
    let without_build = trimmed.split_once('+').map_or(trimmed, |(left, _)| left);
    let (core, pre) = match without_build.split_once('-') {
        Some((c, p)) if !p.is_empty() => (c, Some(p)),
        Some((c, _)) => (c, None),
        None => (without_build, None),
    };
    let mut parts = core.split('.');
    let major = parts.next()?.parse::<u64>().ok()?;
    let minor = parts.next()?.parse::<u64>().ok()?;
    let patch = parts.next()?.parse::<u64>().ok()?;
    parts.next().is_none().then_some(([major, minor, patch], pre))
}

fn compare_prerelease(a: &str, b: &str) -> Ordering {
    let mut ai = a.split(['.', '-']);
    let mut bi = b.split(['.', '-']);
    loop {
        match (ai.next(), bi.next()) {
            (Some(sa), Some(sb)) => {
                let ord = match (sa.parse::<u64>(), sb.parse::<u64>()) {
                    (Ok(na), Ok(nb)) => na.cmp(&nb),
                    _ => sa.cmp(sb),
                };
                if ord != Ordering::Equal {
                    return ord;
                }
            }
            (None, None) => return Ordering::Equal,
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
        }
    }
}

/// True when both `candidate` and `running` parse as SemVer (including `-dev-<N>` / `-beta.<N>`
/// prerelease tags) and `candidate` is strictly older than `running`.
pub fn version_is_older(candidate: &str, running: &str) -> bool {
    let (Some((c_core, c_pre)), Some((r_core, r_pre))) = (parse_version(candidate), parse_version(running)) else {
        return false;
    };
    match c_core.cmp(&r_core) {
        Ordering::Less => true,
        Ordering::Greater => false,
        Ordering::Equal => match (c_pre, r_pre) {
            (Some(_), None) => true,
            (None, Some(_)) | (None, None) => false,
            (Some(a), Some(b)) => compare_prerelease(a, b).is_lt(),
        },
    }
}
