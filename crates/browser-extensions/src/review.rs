//! Permissions review summarizer, registrable-site grouping, strict downgrade detection, and the
//! explicit install/review state machine (`enum InstallState` + `step`).
//!
//! Mirrors `packages/gui-plugins/src/browser/profile/review.ts` and `profile/index.ts:308-414`,
//! with stricter guarantees:
//! - Surfaces `proxy`, `declarativeNetRequest*`, `management`, `externally_connectable`, and
//!   `web_accessible_resources` in grants and update deltas.
//! - Preserves `*.` wildcards on two-label and three-label public/private suffixes (`*.co.uk`,
//!   `*.github.io`, `*.s3.amazonaws.com`), and restricts `sites` to sanitized ASCII hostnames `<= 64`
//!   chars.
//! - `InstallState` prevents preempting `Swapping`, tags `Staging` with a monotonic generation so
//!   late results from cancelled installs cannot hijack a newer review, enforces constant-time
//!   single-use tokens with a 10-minute TTL (`REVIEW_TTL_MS`), and only lets `SwapSucceeded` /
//!   `SwapFailed` transition out of `Swapping`.

use std::cmp::Ordering;
use std::collections::HashSet;
use std::path::PathBuf;

use crate::crx::constant_time_eq;
use crate::manifest::{Manifest, compare_versions, covers_pattern, extension_access, review_hosts};
use crate::{
    ExtensionAccess, ExtensionSource, InstallFailure, InstallReview, ReviewedGrants, StoredExtension,
};

/// Default review token lifetime (10 minutes in milliseconds).
pub const REVIEW_TTL_MS: u64 = 10 * 60 * 1000;

const MAX_SITE_CHARS: usize = 64;

/// Result of summarizing an extension's grants for the install review dialog (`review.ts:10-38`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AccessReview {
    pub grants: ReviewedGrants,
    pub sites: Vec<String>,
    pub added: Option<ReviewedGrants>,
}

/// Summarizes an extension's manifest permissions and host patterns for user review, plus any
/// newly requested grants when updating an existing installation (`review.ts:10-38`).
pub fn review_access(manifest: &Manifest, previous: Option<&ReviewedGrants>) -> AccessReview {
    let hosts = review_hosts(manifest);
    let host_set: HashSet<&str> = hosts.iter().map(String::as_str).collect();
    let permissions: Vec<String> = manifest
        .permissions
        .iter()
        .filter(|p| !host_set.contains(p.as_str()))
        .cloned()
        .collect();
    let access = extension_access(manifest);
    let externally_connectable = manifest.externally_connectable.clone();
    let web_accessible_resources = manifest.web_accessible_resources.clone();
    let grants = ReviewedGrants {
        access: access.clone(),
        hosts: hosts.clone(),
        permissions: permissions.clone(),
        externally_connectable: externally_connectable.clone(),
        web_accessible_resources: web_accessible_resources.clone(),
    };

    let mut site_set = HashSet::new();
    for pattern in &hosts {
        if let Some(site) = summarize_pattern_site(pattern) {
            site_set.insert(site);
        }
    }
    let mut sites: Vec<String> = site_set.into_iter().collect();
    sites.sort();

    let added = previous.and_then(|prev| {
        let added_access: Vec<ExtensionAccess> = access
            .iter()
            .copied()
            .filter(|&item| {
                !prev.access.contains(&item)
                    && !(item == ExtensionAccess::SomeSites && prev.access.contains(&ExtensionAccess::AllSites))
            })
            .collect();
        let added_hosts: Vec<String> = hosts
            .iter()
            .filter(|item| !prev.hosts.iter().any(|g| g == *item || covers_pattern(g, item)))
            .cloned()
            .collect();
        let added_perms: Vec<String> = permissions
            .iter()
            .filter(|item| !prev.permissions.contains(item))
            .cloned()
            .collect();
        let added_ext_conn: Vec<String> = externally_connectable
            .iter()
            .filter(|item| !prev.externally_connectable.contains(item))
            .cloned()
            .collect();
        let added_war: Vec<String> = web_accessible_resources
            .iter()
            .filter(|item| !prev.web_accessible_resources.contains(item))
            .cloned()
            .collect();
        if added_access.is_empty()
            && added_hosts.is_empty()
            && added_perms.is_empty()
            && added_ext_conn.is_empty()
            && added_war.is_empty()
        {
            None
        } else {
            Some(ReviewedGrants {
                access: added_access,
                hosts: added_hosts,
                permissions: added_perms,
                externally_connectable: added_ext_conn,
                web_accessible_resources: added_war,
            })
        }
    });

    AccessReview { grants, sites, added }
}

/// Whether `new_version` is strictly older than `previous_version` (or either version is invalid,
/// failing closed on malformed version strings).
pub fn is_downgrade(previous_version: &str, new_version: &str) -> bool {
    matches!(compare_versions(new_version, previous_version), Some(Ordering::Less) | None)
}

/// Extracts a concise, ASCII-sanitized (`<= 64` chars) registrable site name from a Chrome host
/// match pattern (`review.ts:17-29`). Preserves `*.` wildcards on public and private suffixes.
fn summarize_pattern_site(pattern: &str) -> Option<String> {
    let (_, rest) = pattern.split_once("://")?;
    let authority = rest.split('/').next()?;
    let (wildcard, raw_host) = match authority.strip_prefix("*.") {
        Some(h) => (true, h),
        None => (false, authority),
    };
    let host_only = match raw_host.split_once(':') {
        Some((h, port)) if port == "*" || (!port.is_empty() && port.bytes().all(|b| b.is_ascii_digit())) => h,
        Some(_) => return None,
        None => raw_host,
    };
    let host = host_only.trim().trim_end_matches('.').to_ascii_lowercase();
    if !is_safe_ascii_host(&host) {
        return None;
    }
    let domain = registrable_domain(&host);
    let label = if let Some(d) = domain {
        d
    } else if wildcard {
        format!("*.{host}")
    } else {
        host
    };
    Some(cap_site_label(&label))
}

fn is_safe_ascii_host(host: &str) -> bool {
    if host.is_empty() || host.len() > 253 || host == "*" || host.contains('*') {
        return false;
    }
    if host.bytes().any(|b| !(b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'.' || b == b'-')) {
        return false;
    }
    host.split('.').all(|label| {
        !label.is_empty()
            && label.len() <= 63
            && !label.starts_with('-')
            && !label.ends_with('-')
    })
}

fn cap_site_label(site: &str) -> String {
    if site.len() <= MAX_SITE_CHARS {
        return site.to_owned();
    }
    // Preserve the right-most registrable end, matching `host-display.ts`.
    let tail_len = MAX_SITE_CHARS.saturating_sub(1);
    format!("…{}", &site[site.len() - tail_len..])
}

/// Common two- and three-label public and private hosting suffixes so `*.co.uk`, `*.github.io`,
/// `*.s3.amazonaws.com`, `a.github.io`, and `foo.co.uk` summarize accurately without a 200 KB PSL
/// bundle (`review.ts:23-28`).
const MULTI_LABEL_SUFFIXES: &[&str] = &[
    // Public suffixes (2-label)
    "co.uk", "org.uk", "ac.uk", "gov.uk", "me.uk", "net.uk",
    "com.au", "net.au", "org.au", "edu.au", "gov.au",
    "co.jp", "ne.jp", "or.jp", "ac.jp", "go.jp",
    "co.nz", "org.nz", "net.nz", "govt.nz",
    "co.kr", "or.kr", "go.kr",
    "co.in", "org.in", "net.in", "gov.in",
    "co.za", "org.za", "gov.za",
    "co.il", "org.il", "co.id", "or.id",
    "com.br", "org.br", "net.br", "gov.br",
    "com.cn", "org.cn", "net.cn", "gov.cn",
    "com.hk", "org.hk", "com.tw", "org.tw", "com.sg", "org.sg",
    "com.mx", "org.mx", "com.ar", "com.tr", "com.pl", "com.ua",
    "com.es", "com.fr", "com.de", "com.my", "com.ph", "com.vn",
    // Private multi-tenant hosting suffixes (2- and 3-label)
    "github.io", "gitlab.io", "pages.dev", "workers.dev", "r2.dev", "deno.dev",
    "vercel.app", "netlify.app", "herokuapp.com", "appspot.com", "web.app",
    "firebaseapp.com", "cloudfront.net", "amazonaws.com", "s3.amazonaws.com",
    "azurewebsites.net", "fly.dev", "onrender.com", "blogspot.com", "wordpress.com",
    "tumblr.com", "myshopify.com", "ngrok.io", "ngrok-free.app", "trycloudflare.com",
];

fn registrable_domain(host: &str) -> Option<String> {
    let trimmed = host.trim_end_matches('.');
    if trimmed.is_empty()
        || trimmed == "localhost"
        || trimmed.starts_with('[')
        || trimmed.bytes().all(|b| b.is_ascii_digit() || b == b'.')
    {
        return None;
    }
    // If the host itself is a known multi-label public or private suffix (e.g. `co.uk`,
    // `github.io`, `s3.amazonaws.com`), it is NOT a registrable single site — return `None`
    // so wildcards (`*.co.uk`, `*.github.io`) stay visible as broad wildcards!
    if MULTI_LABEL_SUFFIXES.contains(&trimmed) {
        return None;
    }
    let labels: Vec<&str> = trimmed.split('.').collect();
    if labels.len() < 2 {
        return None;
    }
    if labels.len() >= 4 {
        let last_three = format!(
            "{}.{}.{}",
            labels[labels.len() - 3],
            labels[labels.len() - 2],
            labels[labels.len() - 1]
        );
        if MULTI_LABEL_SUFFIXES.contains(&last_three.as_str()) {
            return Some(format!("{}.{last_three}", labels[labels.len() - 4]));
        }
    }
    if labels.len() >= 3 {
        let last_two = format!("{}.{}", labels[labels.len() - 2], labels[labels.len() - 1]);
        if MULTI_LABEL_SUFFIXES.contains(&last_two.as_str()) {
            return Some(format!("{}.{last_two}", labels[labels.len() - 3]));
        }
    }
    Some(format!("{}.{}", labels[labels.len() - 2], labels[labels.len() - 1]))
}

/// Explicit state machine governing the one-at-a-time extension staging and permission review
/// lifecycle (`AGENTS.md`: explicit `enum State` + `step(&mut State, Event, now_ms) -> Effects`).
#[derive(Clone, Debug, Default)]
pub enum InstallState {
    #[default]
    Idle,
    /// Download / unpack / local copy is running on a worker thread with monotonic `generation`.
    Staging {
        generation: u64,
        source: ExtensionSource,
    },
    /// Staged copy verified and awaiting user confirmation via its single-use `token` before
    /// `expires_at_ms`.
    Reviewing {
        token: String,
        expires_at_ms: u64,
        staged_dir: PathBuf,
        record: Box<StoredExtension>,
        review: Box<InstallReview>,
    },
    /// User approved the review token; atomic swap into managed directory is in flight (cannot be
    /// preempted by `BeginStage` or `Cancel`).
    Swapping {
        staged_dir: PathBuf,
        record: Box<StoredExtension>,
    },
    /// Last install attempt failed; carries reason for UI display until dismissed or retried.
    Failed {
        reason: InstallFailure,
        message: Option<String>,
    },
}

/// Input events driving [`InstallState`].
#[derive(Clone, Debug)]
pub enum InstallEvent {
    BeginStage {
        generation: u64,
        source: ExtensionSource,
    },
    Staged {
        generation: u64,
        token: String,
        staged_dir: PathBuf,
        record: Box<StoredExtension>,
        review: Box<InstallReview>,
    },
    StageFailed {
        generation: u64,
        reason: InstallFailure,
        message: Option<String>,
    },
    Approve {
        token: String,
    },
    SwapSucceeded,
    SwapFailed {
        reason: InstallFailure,
        message: Option<String>,
    },
    Cancel,
    ExtensionRemoved {
        id: String,
    },
}

/// Side effects returned by [`step`] for the caller to execute off the UI thread.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum InstallEffect {
    RemoveStagingDir(PathBuf),
    VerifyAndSwap {
        staged_dir: PathBuf,
        record: Box<StoredExtension>,
    },
    RejectApproval(InstallFailure),
}

/// Pure state transition function for the extension install/review flow.
/// Guarantees illegal states (double-approval, expired/stale-token swap, preempting an in-flight
/// swap, stale staging hijack, leaked staging dir) are impossible.
pub fn step(state: &mut InstallState, event: InstallEvent, now_ms: u64) -> Vec<InstallEffect> {
    let mut effects = Vec::new();
    match event {
        InstallEvent::BeginStage { generation, source } => {
            if matches!(state, InstallState::Swapping { .. }) {
                return effects;
            }
            if let InstallState::Reviewing { staged_dir, .. } = std::mem::take(state) {
                effects.push(InstallEffect::RemoveStagingDir(staged_dir));
            }
            *state = InstallState::Staging { generation, source };
        }
        InstallEvent::Staged { generation, token, staged_dir, record, review } => {
            if matches!(state, InstallState::Staging { generation: g, .. } if *g == generation) {
                *state = InstallState::Reviewing {
                    token,
                    expires_at_ms: now_ms.saturating_add(REVIEW_TTL_MS),
                    staged_dir,
                    record,
                    review,
                };
            } else {
                effects.push(InstallEffect::RemoveStagingDir(staged_dir));
            }
        }
        InstallEvent::StageFailed { generation, reason, message } => {
            if matches!(state, InstallState::Staging { generation: g, .. } if *g == generation) {
                *state = InstallState::Failed { reason, message };
            }
        }
        InstallEvent::Approve { token } => match std::mem::take(state) {
            InstallState::Reviewing { token: expected, expires_at_ms, staged_dir, record, review } => {
                if matches!(
                    review.compatibility.cause,
                    Some(
                        crate::UnsupportedCause::ManifestV2
                            | crate::UnsupportedCause::DisallowedPermission
                    )
                ) {
                    effects.push(InstallEffect::RemoveStagingDir(staged_dir));
                    effects.push(InstallEffect::RejectApproval(InstallFailure::Load));
                    *state = InstallState::Failed {
                        reason: InstallFailure::Load,
                        message: Some(if review.compatibility.cause == Some(crate::UnsupportedCause::ManifestV2) {
                            "Manifest V2 extensions are not supported in Chromium 156.".into()
                        } else {
                            "This extension requires a disallowed browser permission.".into()
                        }),
                    };
                } else if now_ms > expires_at_ms
                    || !constant_time_eq(expected.as_bytes(), token.as_bytes())
                {
                    effects.push(InstallEffect::RemoveStagingDir(staged_dir));
                    effects.push(InstallEffect::RejectApproval(InstallFailure::Expired));
                    *state = InstallState::Failed {
                        reason: InstallFailure::Expired,
                        message: None,
                    };
                } else {
                    effects.push(InstallEffect::VerifyAndSwap {
                        staged_dir: staged_dir.clone(),
                        record: record.clone(),
                    });
                    *state = InstallState::Swapping { staged_dir, record };
                }
            }
            other => {
                *state = other;
                effects.push(InstallEffect::RejectApproval(InstallFailure::Expired));
            }
        },
        InstallEvent::SwapSucceeded => {
            if matches!(state, InstallState::Swapping { .. }) {
                *state = InstallState::Idle;
            }
        }
        InstallEvent::SwapFailed { reason, message } => {
            if let InstallState::Swapping { staged_dir, .. } = state {
                effects.push(InstallEffect::RemoveStagingDir(staged_dir.clone()));
                *state = InstallState::Failed { reason, message };
            }
        }
        InstallEvent::Cancel => {
            if matches!(state, InstallState::Swapping { .. }) {
                return effects;
            }
            if let InstallState::Reviewing { staged_dir, .. } = std::mem::take(state) {
                effects.push(InstallEffect::RemoveStagingDir(staged_dir));
            }
            *state = InstallState::Idle;
        }
        InstallEvent::ExtensionRemoved { id } => {
            if let InstallState::Reviewing { record, .. } = state
                && record.id == id
                && let InstallState::Reviewing { staged_dir, .. } = std::mem::take(state)
            {
                effects.push(InstallEffect::RemoveStagingDir(staged_dir));
                *state = InstallState::Idle;
            }
        }
    }
    effects
}
