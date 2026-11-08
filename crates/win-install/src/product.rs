//! The identity a build installs under: what people see, and the ids Windows keys it by.

/// The app's executable in the install root.
pub const EXE: &str = "opencode-desktop.exe";
// TODO(IMPORTANT before release): the owner picks the Authenticode signing identity (the certificate's subject); set PUBLISHER to it, so Apps & features, the exe's version resource and the signature name the same publisher.
/// Who Apps & features and the exe's version resource say made the app.
pub const PUBLISHER: &str = "Anomaly";
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

    /// The install folder's name: the name without parentheses (`OpenCode Rune`).
    pub fn folder(&self) -> String {
        self.name.replace(['(', ')'], "").split_whitespace().collect::<Vec<_>>().join(" ")
    }

    /// Every scheme the install points at the app: OpenCode's and the app's own.
    pub fn schemes(&self) -> [&str; 2] {
        [OPENCODE_SCHEME, &self.scheme]
    }
}
