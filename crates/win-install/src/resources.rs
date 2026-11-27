//! The resources a Windows executable carries, for build scripts: its icon (Explorer, the
//! taskbar, Alt+Tab, a pinned shortcut), its version information (Properties › Details, the
//! uninstall entry's size and version) and its manifest (common controls 6 for task dialogs,
//! `asInvoker`, so Windows never guesses a setup wants an administrator).
//!
//! Call [`embed`] from `build.rs` when `CARGO_CFG_TARGET_OS` is `windows`; it compiles with the
//! Windows SDK's `rc.exe`.

use std::path::Path;

/// What one executable embeds.
pub struct Resources<'a> {
    icon: &'a Path,
    manifest: &'a Path,
    description: &'a str,
    product: &'a str,
    file_name: &'a str,
    version: &'a str,
}

impl<'a> Resources<'a> {
    /// `icon` is an `.ico` holding every size the shell asks for; `manifest` the application
    /// manifest; `file_name` the executable's name; `version` the build's version string.
    pub fn new(icon: &'a Path, manifest: &'a Path, file_name: &'a str, version: &'a str) -> Self {
        Resources { icon, manifest, description: "", product: "", file_name, version }
    }

    /// The file's description (Task Manager's name for it) and the product it belongs to.
    pub fn names(mut self, description: &'a str, product: &'a str) -> Self {
        self.description = description;
        self.product = product;
        self
    }
}

/// Compiles the resources into the executable being built.
pub fn embed(r: &Resources) -> Result<(), String> {
    println!("cargo:rerun-if-changed={}", r.icon.display());
    println!("cargo:rerun-if-changed={}", r.manifest.display());
    let mut res = winresource::WindowsResource::new();
    res.set_icon(&r.icon.to_string_lossy()).set_manifest_file(&r.manifest.to_string_lossy());
    version(&mut res, r.file_name, r.version, r.description, r.product);
    res.compile().map_err(|e| format!("windows resources: {e}"))
}

/// Compiles the icon and version information (Properties > Details) into the library being built;
/// Windows reads the application manifest from the exe.
///
/// The icon is the windows': a window's icons load from the module its window code is linked
/// into (`rune::set_app_icon_resource`: winit's `Icon::from_resource` passes that module's own
/// handle to `LoadImageW`), which is the library when the app is one. The exe's icon alone leaves
/// every window without one: the taskbar then draws the exe's icon, and Alt+Tab, which asks only
/// the window, the system's default program icon. `icon` is the exe's `.ico`, under the same
/// resource id (1).
pub fn embed_library(icon: &Path, file_name: &str, version_text: &str, description: &str, product: &str) -> Result<(), String> {
    println!("cargo:rerun-if-changed={}", icon.display());
    let mut res = winresource::WindowsResource::new();
    res.set_icon(&icon.to_string_lossy());
    version(&mut res, file_name, version_text, description, product);
    res.compile().map_err(|e| format!("windows resources: {e}"))
}

fn version(res: &mut winresource::WindowsResource, file_name: &str, version: &str, description: &str, product: &str) {
    let packed = pack(version);
    res.set("FileDescription", description)
        .set("ProductName", product)
        .set("CompanyName", crate::PUBLISHER)
        .set("OriginalFilename", file_name)
        .set("InternalName", file_name.trim_end_matches(".exe").trim_end_matches(".dll"))
        .set("FileVersion", version)
        .set("ProductVersion", version)
        .set("LegalCopyright", "")
        .set_version_info(winresource::VersionInfo::FILEVERSION, packed)
        .set_version_info(winresource::VersionInfo::PRODUCTVERSION, packed);
}

/// `major.minor.patch.build` packed for `VS_FIXEDFILEINFO`: `1.2.3` is 1.2.3.0, and a preview's
/// trailing number is its build (`0.0.0-dev-42` is 0.0.0.42, `0.0.0-pr-12-7` is 0.0.0.7).
pub fn pack(version: &str) -> u64 {
    let (core, pre) = version.split_once('-').unwrap_or((version, ""));
    let mut parts = core.split('.').map(|p| p.parse::<u16>().unwrap_or(0));
    let mut next = || u64::from(parts.next().unwrap_or(0));
    let (major, minor, patch) = (next(), next(), next());
    let digits: String = pre.chars().rev().take_while(char::is_ascii_digit).collect::<Vec<_>>().into_iter().rev().collect();
    let build = u64::from(digits.parse::<u16>().unwrap_or(0));
    (major << 48) | (minor << 32) | (patch << 16) | build
}
