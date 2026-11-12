//! The identity a build installs under: what people see, and the ids Windows keys it by.

/// The app's executable in the install root.
pub const EXE: &str = "opencode-desktop.exe";
/// Who Apps & features and the exe's version resource (CompanyName) say made the app: `OpenCode`,
/// as OpenCode Desktop shows in both. A display name only; the signature the updater trusts is
/// the certificate's (`updater::windows::EXPECTED_PUBLISHER` in the app).
pub const PUBLISHER: &str = "OpenCode";
pub const HOMEPAGE: &str = "https://opencode.ai";
/// The scheme OpenCode Desktop registers. The install takes it too, so `opencode://open?project=…`
/// opens the project here; OpenCode Desktop takes it back each time it starts.
pub const OPENCODE_SCHEME: &str = "opencode";
/// The id every build's own id starts with (`anomaly.opencode-rune.dev`, `….pr.12`).
const FAMILY: &str = "anomaly.opencode-rune";

/// One build's identity: its name, its AppUserModelID and its own URL scheme, from the same
/// environment variables the release workflows set for every artifact of a build.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Product {
    name: String,
    id: String,
    scheme: String,
    version: String,
}

impl Product {
    /// The build's identity from its compile-time environment (`OPENCODE_RUNE_WINDOW_TITLE`,
    /// `OPENCODE_RUNE_BUNDLE_ID`, `OPENCODE_RUNE_BUILD_CHANNEL`, `OPENCODE_RUNE_URL_SCHEME`). A
    /// local build (no channel, or `local`) has its own id and scheme, so it never takes a
    /// release's notifications, links or taskbar button.
    pub fn from_build(title: Option<&str>, bundle_id: Option<&str>, channel: Option<&str>, scheme: Option<&str>, version: &str) -> Product {
        fn set(v: Option<&str>) -> Option<&str> {
            v.filter(|v| !v.is_empty())
        }
        let local = set(channel).is_none_or(|c| c == "local");
        Product {
            name: set(title).unwrap_or("OpenCode (Rune)").to_owned(),
            id: set(bundle_id).unwrap_or(if local { "anomaly.opencode-rune.dev" } else { "anomaly.opencode-rune" }).to_owned(),
            scheme: set(scheme).unwrap_or(if local { "opencode-rune-dev" } else { "opencode-rune" }).to_ascii_lowercase(),
            version: version.to_owned(),
        }
    }

    /// What people see: the Start menu entry, Apps & features, the toasts' sender.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The AppUserModelID: the taskbar button, the toasts, and the uninstall entry's key.
    pub fn id(&self) -> &str {
        &self.id
    }

    /// The app's own URL scheme: toast clicks (`<scheme>://notification/…`) and the app's links.
    pub fn scheme(&self) -> &str {
        &self.scheme
    }

    /// What every build of the app shares in its id (`anomaly.opencode-rune`), as they share
    /// the app's data folders.
    pub fn family(&self) -> &str {
        FAMILY
    }

    pub fn version(&self) -> &str {
        &self.version
    }

    /// The same build under another AppUserModelID: a harness run's scratch identity, so its
    /// taskbar button, jump list and toasts (and any registration a proof writes for them) are
    /// its own, never an install's or another run's.
    pub fn with_id(mut self, id: impl Into<String>) -> Product {
        self.id = id.into();
        self
    }

    /// The COM class of the app's toast activator (`{xxxxxxxx-xxxx-4xxx-8xxx-xxxxxxxxxxxx}`): one
    /// per AppUserModelID, the same in every build of it, from the id's FNV-1a hash. The install
    /// registers it (`CustomActivator`, `LocalServer32`); the app serves it while it runs.
    pub fn toast_activator(&self) -> String {
        let hash = self.id.bytes().fold(0x6c62_272e_07bb_0142_62b8_2175_6295_c58d_u128, |h, b| (h ^ u128::from(b)).wrapping_mul(0x0000_0000_0100_0000_0000_0000_0000_013b));
        let h = (hash & !(0xf << 76) | (0x4 << 76)) & !(0x3 << 62) | (0x2 << 62);
        format!("{{{:08x}-{:04x}-{:04x}-{:04x}-{:012x}}}", h >> 96, (h >> 80) & 0xffff, (h >> 64) & 0xffff, (h >> 48) & 0xffff, h & 0xffff_ffff_ffff)
    }
    /// The install folder's name: the name without parentheses (`OpenCode Rune`).
    pub fn folder(&self) -> String {
        self.name.replace(['(', ')'], "").split_whitespace().collect::<Vec<_>>().join(" ")
    }

}
