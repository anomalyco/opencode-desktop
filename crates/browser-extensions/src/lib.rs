//! Pure-Rust (`#![forbid(unsafe_code)]`) MV3 browser extension verification, bounded unpacking,
//! SHA-256 tree integrity, compatibility classification, permissions review, and install staging.
//!
//! Mirrors upstream `packages/gui-plugins/src/browser/profile/{extensions,integrity,compatibility,review,install,sources,index}.ts`
//! with stricter guarantees for Rune Desktop (CEF 156):
//! - Manifest V2 extensions are honestly classified `Unsupported(ManifestV2)` (matching Chrome 156).
//! - CRX3 verification checks every RSA-SHA256 and ECDSA-P256-SHA256 proof against canonical SPKI
//!   DER and binds the developer public key to the requested 32-char listing ID.
//! - ZIP unpacking checks every entry before writing, verifies CRC-32 and exact inflated lengths,
//!   forbids symlinks, encrypted entries, ZIP64, duplicate/colliding paths, and Windows device names
//!   (including superscript `COM¹..³` / `LPT¹..³` via `browser_policy::is_windows_device_name`).
//! - Store updates refuse version downgrades.
//! - All persisted extension metadata flows through the app's `opencode-storage` port (`StoredExtension`).

#![forbid(unsafe_code)]

pub mod crx;
#[cfg(not(target_arch = "wasm32"))]
pub mod integrity;
#[cfg(not(target_arch = "wasm32"))]
pub mod manager;
pub mod manifest;
pub mod review;
#[cfg(not(target_arch = "wasm32"))]
pub mod sources;
#[cfg(not(target_arch = "wasm32"))]
pub mod zip;

use serde::{Deserialize, Serialize};

/// Where an installed extension came from (`profile/ipc.ts:82`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExtensionSource {
    Store,
    Browser,
    Folder,
}

/// Supported extension web stores (`profile/extensions.ts:231`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StoreKind {
    Chrome,
    Edge,
}

/// Extension record persisted in `opencode-storage` under `plugin.browser/extensions`
/// (`profile/index.ts:50-60`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoredExtension {
    pub id: String,
    pub directory: String,
    pub enabled: bool,
    pub source: ExtensionSource,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub local_key: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub digest: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub grants: Option<ReviewedGrants>,
}

/// Feature area an extension declares that this browser does not support (`profile/ipc.ts:23-38`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ExtensionFeature {
    Native,
    PageSync,
    Shortcuts,
    SidePanel,
    Devtools,
    Signin,
    Network,
    BrowserData,
    Settings,
    AddressBar,
    NewTab,
    Access,
    UserScripts,
    Other,
}

/// High-level compatibility verdict (`profile/ipc.ts:49-53`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CompatibilityLevel {
    Supported,
    Partial,
    Unsupported,
}

/// Why an extension is marked `Unsupported`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UnsupportedCause {
    /// Background script declares a fatal missing API (`readingList`, `tabGroups`, `gcm`, etc.).
    Startup,
    /// Manifest V2 extension refused by Chromium 156.
    ManifestV2,
}

/// Compatibility classification for UI badges and review dialogs (`profile/ipc.ts:49-53`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExtensionCompatibility {
    pub level: CompatibilityLevel,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cause: Option<UnsupportedCause>,
    #[serde(default)]
    pub missing: Vec<ExtensionFeature>,
}

/// Broad access categories summarized in the permissions review (`profile/ipc.ts:63`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ExtensionAccess {
    AllSites,
    SomeSites,
    Cookies,
    Activity,
    Clipboard,
}

/// Reviewed permission & host grant snapshot stored with an approved install (`profile/ipc.ts:67-71`).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReviewedGrants {
    #[serde(default)]
    pub access: Vec<ExtensionAccess>,
    #[serde(default)]
    pub hosts: Vec<String>,
    #[serde(default)]
    pub permissions: Vec<String>,
}

/// Summary of an installed extension for Settings › Browser › Extensions (`profile/ipc.ts:76-107`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProfileExtension {
    pub id: String,
    pub name: String,
    pub version: String,
    pub enabled: bool,
    pub source: ExtensionSource,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub popup: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub options: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default)]
    pub changed: bool,
    pub compatibility: ExtensionCompatibility,
    #[serde(default)]
    pub access: Vec<ExtensionAccess>,
}

/// An extension discovered in another installed Chromium browser profile (`profile/ipc.ts:239-251`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BrowserExtensionCandidate {
    pub id: String,
    pub name: String,
    pub version: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
    pub installed: bool,
    pub compatibility: ExtensionCompatibility,
    #[serde(default)]
    pub access: Vec<ExtensionAccess>,
}

/// Review payload presented to the user before approving an install or update (`profile/ipc.ts:159-171`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstallReview {
    pub unsupported: bool,
    pub token: String,
    pub id: String,
    pub source: ExtensionSource,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub store: Option<StoreKind>,
    pub name: String,
    pub version: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub replaces: Option<String>,
    pub compatibility: ExtensionCompatibility,
    pub grants: ReviewedGrants,
    pub sites: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub added: Option<ReviewedGrants>,
}

/// Failure codes for extension installation (`profile/ipc.ts:173-176`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InstallFailure {
    Manifest,
    Download,
    Load,
    Collision,
    Expired,
    Downgrade,
}

/// Result of staging or approving an extension install (`profile/ipc.ts:157-177`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum InstallResult {
    Installed { extension: ProfileExtension },
    Review(InstallReview),
    Failed { reason: InstallFailure, message: Option<String> },
}

/// Verified startup specification passed to `opencode-browser-engine --extension <id>:<sha256>:<dir>`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VerifiedStartupSpec {
    pub id: String,
    pub sha256: String,
    pub dir: String,
}

impl VerifiedStartupSpec {
    pub fn cli_arg(&self) -> String {
        format!("{}:{}:{}", self.id, self.sha256, self.dir)
    }
}
