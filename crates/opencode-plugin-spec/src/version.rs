//! Which plugins a host runs, by the `contract` their plugin.toml names.

use crate::{CONTRACT_MAJOR, CONTRACT_MINOR};

/// A `MAJOR.MINOR` contract version.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ContractVersion {
    major: u16,
    minor: u16,
}

/// Whether this host runs a plugin that targets a contract version.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Compat {
    /// Same major version, and a minor version this host implements.
    Supported,
    /// Same major version, newer minor version: load it, and log one warning. Calls to
    /// services or keys this host lacks warn and fall back.
    NewerMinor,
    /// Another major version: do not load it.
    Unsupported,
}

impl ContractVersion {
    /// The version this crate describes.
    pub const HOST: ContractVersion = ContractVersion { major: CONTRACT_MAJOR, minor: CONTRACT_MINOR };

    pub const fn new(major: u16, minor: u16) -> Self {
        ContractVersion { major, minor }
    }

    /// `"1.0"`; `None` for anything else, such as `"1"` or `"1.0.0"`.
    pub fn parse(s: &str) -> Option<Self> {
        let (major, minor) = s.trim().split_once('.')?;
        let digits = |p: &str| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit());
        if !digits(major) || !digits(minor) {
            return None;
        }
        Some(ContractVersion { major: major.parse().ok()?, minor: minor.parse().ok()? })
    }

    pub fn major(self) -> u16 {
        self.major
    }

    pub fn minor(self) -> u16 {
        self.minor
    }

    /// Whether a host at `self` runs a plugin that targets `plugin`.
    pub fn runs(self, plugin: ContractVersion) -> Compat {
        match (plugin.major == self.major, plugin.minor <= self.minor) {
            (false, _) => Compat::Unsupported,
            (true, true) => Compat::Supported,
            (true, false) => Compat::NewerMinor,
        }
    }
}

impl std::fmt::Display for ContractVersion {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}.{}", self.major, self.minor)
    }
}
