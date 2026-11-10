//! Anomaly Release protocol v1 (`https://anoma.ly/release`) manifest parser and update decision
//! rules (`anomalyco/release/docs/protocol.md`). Versions are display text and never ordered:
//! an install updates whenever the channel's current release differs from the running build.

use serde_json::Value;

#[cfg_attr(not(any(target_os = "macos", windows)), allow(dead_code))]
pub const DEFAULT_SERVICE_URL: &str = "https://anoma.ly/release";
pub const APP_NAME: &str = "opencode-rune";
pub const ARTIFACT_NAME: &str = "desktop";
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
    RolloutDeferred { version: String },
    Install { version: String, target: Target },
}

pub fn manifest_url(service_base: &str, channel: &str) -> String {
    format!("{}/{APP_NAME}/{ARTIFACT_NAME}/{channel}.json", service_base.trim_end_matches('/'))
}

/// Resolves a manifest target URL (typically `/release/<app>/files/<sha256>/<name>`) against the
/// origin of `manifest_url`.
pub fn resolve_target_url(manifest_url: &str, target_url: &str) -> Result<String, String> {
    if target_url.starts_with("https://") || target_url.starts_with("http://") {
        return Ok(target_url.to_owned());
    }
    if !target_url.starts_with('/') {
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
    Ok(format!("{scheme}://{host}{target_url}"))
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
