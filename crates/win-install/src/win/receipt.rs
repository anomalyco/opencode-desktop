//! Our installer's receipt, `<root>\install.json`: `{"app","artifact","channel","method":"direct","root"}`
//! (`anomalyco/release` `docs/protocol.md`, "Install methods"). The app updates itself only when
//! its install carries it; any other copy (a zip run in place, a package manager's) only links to
//! the setup. The setup writes it, and the updater carries it into each version it swaps in.

use std::path::Path;

use super::registry;
use super::scheme::same_path;

/// The receipt's file name in the install root.
pub const RECEIPT: &str = "install.json";
/// The release this install follows: `https://anoma.ly/release/opencode/desktop-rune/dev.json`
/// (`features/updater/src/protocol.rs` `APP_NAME`, `ARTIFACT_NAME`; `feed.rs` `CHANNEL`).
const APP: &str = "opencode";
const ARTIFACT: &str = "desktop-rune";
const CHANNEL: &str = "dev";

/// Writes the receipt for the install at `root`.
pub fn write_receipt(root: &Path) -> Result<(), String> {
    let receipt = serde_json::json!({ "app": APP, "artifact": ARTIFACT, "channel": CHANNEL, "method": "direct", "root": root.to_string_lossy() });
    let path = root.join(RECEIPT);
    let partial = root.join(format!("{RECEIPT}.partial"));
    std::fs::write(&partial, receipt.to_string()).and_then(|()| std::fs::rename(&partial, &path)).map_err(|e| {
        std::fs::remove_file(&partial).ok();
        format!("couldn't write {}: {e}", path.display())
    })
}

/// Whether `root` carries our receipt, naming this app, artifact and channel, the direct method
/// and `root` itself.
pub fn has_receipt(root: &Path) -> bool {
    let Ok(bytes) = std::fs::read(root.join(RECEIPT)) else { return false };
    let Ok(receipt) = serde_json::from_slice::<serde_json::Value>(&bytes) else { return false };
    let field = |name: &str| receipt.get(name).and_then(serde_json::Value::as_str);
    field("app") == Some(APP)
        && field("artifact") == Some(ARTIFACT)
        && field("channel") == Some(CHANNEL)
        && field("method") == Some("direct")
        && field("root").is_some_and(|r| same_path(Path::new(r), root))
}

/// Whether the setup installed `root`: an Apps & features entry of the app names it. Installs
/// from setups before the receipt have one, so the updater writes their receipt.
pub fn installed_by_setup(root: &Path) -> bool {
    registry::entry_at(root)
}
