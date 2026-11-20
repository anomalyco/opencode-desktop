//! Permissions review summarizer, registrable-site grouping, downgrade detection, and the
//! explicit install/review state machine (`enum InstallState` + `step`).
//!
//! Mirrors `packages/gui-plugins/src/browser/profile/review.ts` and `profile/index.ts:308-414`.

use std::cmp::Ordering;
use std::collections::HashSet;
use std::path::PathBuf;

use crate::manifest::{Manifest, compare_versions, covers_pattern, extension_access, review_hosts};
use crate::{
    ExtensionAccess, ExtensionSource, InstallFailure, InstallReview, ReviewedGrants, StoredExtension,
};

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
    let grants = ReviewedGrants { access: access.clone(), hosts: hosts.clone(), permissions: permissions.clone() };

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
        if added_access.is_empty() && added_hosts.is_empty() && added_perms.is_empty() {
            None
        } else {
            Some(ReviewedGrants {
                access: added_access,
                hosts: added_hosts,
                permissions: added_perms,
            })
        }
    });

    AccessReview { grants, sites, added }
}

/// Whether `new_version` is strictly older than `previous_version` (refused for store updates).
pub fn is_downgrade(previous_version: &str, new_version: &str) -> bool {
    compare_versions(new_version, previous_version) == Ordering::Less
}

/// Extracts a concise registrable site name from a Chrome host match pattern (`review.ts:17-29`).
fn summarize_pattern_site(pattern: &str) -> Option<String> {
    let (_, rest) = pattern.split_once("://")?;
    let authority = rest.split('/').next()?;
    let (wildcard, raw_host) = match authority.strip_prefix("*.") {
        Some(h) => (true, h),
        None => (false, authority),
    };
    let host = raw_host.split(':').next()?.trim().to_ascii_lowercase();
    if host.is_empty() || host == "*" || host.contains('*') {
        return None;
    }
    let domain = registrable_domain(&host);
    if let Some(d) = domain {
        Some(d)
    } else if wildcard {
        Some(format!("*.{host}"))
    } else {
        Some(host)
    }
}

/// Common multi-label public and private hosting suffixes so `a.github.io` / `b.github.io` and
/// `foo.co.uk` summarize accurately without a 200 KB PSL bundle (`review.ts:23-28`).
const MULTI_LABEL_SUFFIXES: &[&str] = &[
    "co.uk", "org.uk", "ac.uk", "gov.uk",
    "com.au", "net.au", "org.au", "edu.au", "gov.au",
    "co.jp", "ne.jp", "or.jp", "ac.jp", "go.jp",
    "co.nz", "co.kr", "co.in", "co.za", "co.il", "co.id",
    "com.br", "com.cn", "com.hk", "com.mx", "com.sg", "com.tw", "com.tr", "com.ar",
    "github.io", "gitlab.io", "pages.dev", "workers.dev", "vercel.app",
    "netlify.app", "herokuapp.com", "appspot.com", "web.app", "firebaseapp.com",
    "cloudfront.net", "amazonaws.com", "azurewebsites.net", "fly.dev", "onrender.com",
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
    let labels: Vec<&str> = trimmed.split('.').collect();
    if labels.len() < 2 {
        return None;
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
/// lifecycle (`AGENTS.md`: explicit `enum State` + `step(&mut State, Event) -> Effects`).
#[derive(Clone, Debug, Default)]
pub enum InstallState {
    #[default]
    Idle,
    /// Download / unpack / local copy is running on a worker thread.
    Staging { source: ExtensionSource },
    /// Staged copy verified and awaiting user confirmation via its single-use `token`.
    Reviewing {
        token: String,
        staged_dir: PathBuf,
        record: Box<StoredExtension>,
        review: Box<InstallReview>,
    },
    /// User approved the review token; atomic swap into managed directory is in flight.
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
    BeginStage { source: ExtensionSource },
    Staged {
        token: String,
        staged_dir: PathBuf,
        record: Box<StoredExtension>,
        review: Box<InstallReview>,
    },
    Approve { token: String },
    SwapSucceeded,
    Failed { reason: InstallFailure, message: Option<String> },
    Cancel,
    ExtensionRemoved { id: String },
}

/// Side effects returned by [`step`] for the caller to execute off the UI thread.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum InstallEffect {
    RemoveStagingDir(PathBuf),
    VerifyAndSwap { staged_dir: PathBuf, record: Box<StoredExtension> },
    RejectApproval(InstallFailure),
}

/// Pure state transition function for the extension install/review flow.
/// Guarantees illegal states (double-approval, stale-token swap, leaked staging dir) are impossible.
pub fn step(state: &mut InstallState, event: InstallEvent) -> Vec<InstallEffect> {
    let mut effects = Vec::new();
    match event {
        InstallEvent::BeginStage { source } => {
            if let InstallState::Reviewing { staged_dir, .. } = std::mem::take(state) {
                effects.push(InstallEffect::RemoveStagingDir(staged_dir));
            }
            *state = InstallState::Staging { source };
        }
        InstallEvent::Staged { token, staged_dir, record, review } => {
            if matches!(state, InstallState::Staging { .. }) {
                *state = InstallState::Reviewing { token, staged_dir, record, review };
            } else {
                effects.push(InstallEffect::RemoveStagingDir(staged_dir));
            }
        }
        InstallEvent::Approve { token } => match std::mem::take(state) {
            InstallState::Reviewing { token: expected, staged_dir, record, review } => {
                if review.compatibility.cause == Some(crate::UnsupportedCause::ManifestV2) {
                    effects.push(InstallEffect::RemoveStagingDir(staged_dir));
                    effects.push(InstallEffect::RejectApproval(InstallFailure::Load));
                    *state = InstallState::Failed {
                        reason: InstallFailure::Load,
                        message: Some("Manifest V2 extensions are not supported in Chromium 156.".into()),
                    };
                } else if expected == token {
                    effects.push(InstallEffect::VerifyAndSwap {
                        staged_dir: staged_dir.clone(),
                        record: record.clone(),
                    });
                    *state = InstallState::Swapping { staged_dir, record };
                } else {
                    effects.push(InstallEffect::RemoveStagingDir(staged_dir));
                    effects.push(InstallEffect::RejectApproval(InstallFailure::Expired));
                    *state = InstallState::Failed {
                        reason: InstallFailure::Expired,
                        message: None,
                    };
                }
            }
            other => {
                *state = other;
                effects.push(InstallEffect::RejectApproval(InstallFailure::Expired));
            }
        },
        InstallEvent::SwapSucceeded => {
            *state = InstallState::Idle;
        }
        InstallEvent::Failed { reason, message } => {
            match std::mem::take(state) {
                InstallState::Reviewing { staged_dir, .. }
                | InstallState::Swapping { staged_dir, .. } => {
                    effects.push(InstallEffect::RemoveStagingDir(staged_dir));
                }
                _ => {}
            }
            *state = InstallState::Failed { reason, message };
        }
        InstallEvent::Cancel => {
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
