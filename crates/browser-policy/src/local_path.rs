//! Pure string-level local filesystem path policy: refuses UNC/SMB shares, Windows device
//! namespaces, NTFS alternate data streams, reserved device names, control/bidi characters,
//! Unix autofs mounts, and traversal segments BEFORE any filesystem call (`exists`, `metadata`,
//! `canonicalize`, `GetDriveTypeW`, `ILCreateFromPathW`) or OS file-manager reveal.

use crate::is_windows_device_name;

/// Prefix used by inline-code file links (`opencode_ui::style::CODE_LINK`).
const CODE_LINK_PREFIX: &str = "opencode-code-path:";

/// Returns `true` if `raw` is unsafe to pass to any local filesystem API on this host OS.
/// Pure string inspection — never touches the filesystem.
pub fn is_unc_or_device_path(raw: &str) -> bool {
    is_unc_or_device_path_for(raw, cfg!(windows))
}

/// Platform-parameterized variant of [`is_unc_or_device_path`]: scopes Windows-specific checks
/// (drive-relative `C:foo`, NTFS ADS `:`, DOS reserved device names `CON`/`AUX`/`NUL`/…) to
/// Windows paths so legitimate Unix filenames like `2026-11-21T12:00:00.log` or `aux.py` work on
/// macOS and Linux.
pub fn is_unc_or_device_path_for(raw: &str, windows: bool) -> bool {
    if raw.is_empty() || raw.len() > 4096 || has_control_or_bidi(raw) {
        return true;
    }
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return true;
    }
    // UNC (`\\server\share`, `//server/share`), mixed slashes (`\/`, `/\`), Win32 device namespace
    // (`\\?\`, `\\.\`), and NT object namespace (`\??\`, `/??/`, `\??/`, `/??\`).
    if has_unc_or_device_prefix(trimmed) || is_unc_file_uri(trimmed) {
        return true;
    }
    // Never treat a `file:` URI as a local filesystem path on any platform.
    if trimmed.get(..5).is_some_and(|s| s.eq_ignore_ascii_case("file:")) {
        return true;
    }
    let normalized = trimmed.replace('\\', "/");
    if windows {
        let after_drive = if is_drive_prefix(&normalized) {
            // Drive-relative `C:foo` (missing `/` after `C:`) is unsafe/ambiguous.
            if normalized.as_bytes().get(2) != Some(&b'/') {
                return true;
            }
            &normalized[3..]
        } else {
            normalized.as_str()
        };
        // Reject any `:` after the initial Windows drive prefix (NTFS alternate data streams / URIs).
        if after_drive.contains(':') {
            return true;
        }
        // Reject Windows reserved device names (`CON`, `NUL`, `AUX`, `COM1`, `LPT1`, etc.) in any segment.
        for seg in after_drive.split('/') {
            if seg.is_empty() || seg == "." || seg == ".." {
                continue;
            }
            let clean = seg.trim_end_matches(['.', ' ']);
            let stem = clean.split('.').next().unwrap_or(clean);
            if is_windows_device_name(stem) {
                return true;
            }
        }
    } else if is_unix_autofs_path(&normalized) {
        return true;
    }
    false
}

/// Detects raw UNC, SMB, NT device, control/bidi-prefixed, percent-encoded UNC, or remote/UNC
/// `file:` hrefs (including `opencode-code-path:` links) BEFORE link classification collapses
/// slashes, invokes plugins, or touches the filesystem.
pub fn is_raw_unc_href(href: &str) -> bool {
    if is_raw_unc_href_inner(href) {
        return true;
    }
    if let Some(code) = href.trim().strip_prefix(CODE_LINK_PREFIX) {
        return is_raw_unc_href_inner(code);
    }
    false
}

fn is_raw_unc_href_inner(href: &str) -> bool {
    if has_control_or_bidi(href) {
        return true;
    }
    let trimmed = href.trim();
    if has_unc_or_device_prefix(trimmed) || is_unc_file_uri(trimmed) {
        return true;
    }
    let decoded = percent_decode_ascii(trimmed);
    if decoded != trimmed {
        if has_control_or_bidi(&decoded) {
            return true;
        }
        let dec_trim = decoded.trim();
        if has_unc_or_device_prefix(dec_trim) || is_unc_file_uri(dec_trim) {
            return true;
        }
    }
    false
}

/// Validates that `raw` is a clean, absolute local filesystem path candidate safe to pass to
/// [`crate::verify_local_path`] (never UNC, never device namespace, never quotes/controls/bidi,
/// never `..` traversal, never Unix autofs mounts).
///
/// On Windows, requires an explicit `<drive>:\` or `<drive>:/` prefix. On Unix/macOS, requires a
/// single leading `/` (never `//`). Callers MUST still run [`crate::verify_local_path`] before
/// touching disk so mapped drives, `subst`-to-UNC, symlinks/junctions, and network mounts are
/// rejected before any filesystem operation follows them.
pub fn safe_local_reveal_path(raw: &str) -> Option<String> {
    if raw.trim() != raw || raw.contains('"') || is_unc_or_device_path(raw) {
        return None;
    }
    let normalized = raw.replace('\\', "/");
    if normalized.split('/').any(|seg| seg == "..") {
        return None;
    }
    #[cfg(windows)]
    {
        if !is_drive_prefix(&normalized) || normalized.as_bytes().get(2) != Some(&b'/') {
            return None;
        }
        Some(raw.to_owned())
    }
    #[cfg(not(windows))]
    {
        if !normalized.starts_with('/') || normalized.starts_with("//") || is_unix_autofs_path(&normalized) {
            return None;
        }
        Some(raw.to_owned())
    }
}

/// Returns `true` if `normalized` (using `/` separators) points into a known Unix/macOS automount
/// or network-mount root (`/net`, `/Network`, `/misc`, `/afs`, `/run/user/*/gvfs`).
pub fn is_unix_autofs_path(normalized: &str) -> bool {
    const ROOTS: &[&str] = &["/net", "/network", "/misc", "/afs"];
    let lower = normalized.to_ascii_lowercase();
    if ROOTS
        .iter()
        .any(|r| lower == *r || lower.strip_prefix(r).is_some_and(|rest| rest.starts_with('/')))
    {
        return true;
    }
    // Linux GVFS network mounts (`/run/user/<uid>/gvfs/...`).
    if let Some(rest) = lower.strip_prefix("/run/user/")
        && let Some((_, tail)) = rest.split_once('/')
        && (tail == "gvfs" || tail.starts_with("gvfs/"))
    {
        return true;
    }
    false
}

/// Returns `true` if any path segment ends with a Windows Explorer shell namespace CLSID suffix
/// (for example `Folder.{20D04FE0-3AEA-1069-A2D8-08002B30309D}`).
pub fn is_clsid_folder_name(path: &str) -> bool {
    path.split(['/', '\\']).any(|seg| {
        let Some((_, rest)) = seg.rsplit_once(".{") else { return false };
        let Some(inner) = rest.strip_suffix('}') else { return false };
        inner.len() >= 32 && inner.chars().all(|c| c.is_ascii_hexdigit() || c == '-')
    })
}

/// Returns `true` if `path` names a macOS executable bundle, script package, or document package
/// that `/usr/bin/open` without `-R` would launch instead of revealing in Finder.
pub fn is_macos_bundle_path(path: &str) -> bool {
    const BUNDLES: &[&str] = &[
        ".app",
        ".appex",
        ".pkg",
        ".mpkg",
        ".bundle",
        ".prefpane",
        ".workflow",
        ".action",
        ".command",
        ".tool",
        ".dmg",
        ".scptd",
        ".service",
        ".osax",
        ".plugin",
        ".wdgt",
        ".saver",
        ".qlgenerator",
        ".xcodeproj",
        ".xcworkspace",
        ".playground",
        ".framework",
        ".kext",
        ".xpc",
        ".component",
        ".mdimporter",
        ".importer",
        ".vst",
        ".vst3",
        ".au",
        ".rtfd",
        ".pages",
        ".numbers",
        ".key",
    ];
    let trimmed = path.trim_end_matches(['/', '\\']);
    let lower = trimmed.to_ascii_lowercase();
    BUNDLES.iter().any(|ext| lower.ends_with(ext))
}

fn has_control_or_bidi(s: &str) -> bool {
    s.chars().any(|c| {
        let n = c as u32;
        n <= 0x1f
            || (0x7f..=0x9f).contains(&n)
            || matches!(n, 0x061c | 0x200e | 0x200f | 0x202a..=0x202e | 0x2066..=0x2069 | 0xfeff)
    })
}

fn has_unc_or_device_prefix(s: &str) -> bool {
    let b = s.as_bytes();
    if b.len() >= 2 && matches!(b[0], b'/' | b'\\') && matches!(b[1], b'/' | b'\\') {
        return true;
    }
    b.len() >= 4
        && matches!(b[0], b'/' | b'\\')
        && b[1] == b'?'
        && b[2] == b'?'
        && matches!(b[3], b'/' | b'\\')
}

fn is_unc_file_uri(s: &str) -> bool {
    let b = s.as_bytes();
    if b.len() < 6 || !b[..5].eq_ignore_ascii_case(b"file:") {
        return false;
    }
    // `file:\...` (backslash right after `file:`) is non-standard and UNC-prone on Windows.
    if b[5] == b'\\' {
        return true;
    }
    let Some(after) = strip_file_scheme_slashes(s) else {
        // `file:/...`: catch `file:/??/...`, `file:/\...`, etc.
        return has_unc_or_device_prefix(&s[5..]);
    };
    if after.is_empty() {
        return false;
    }
    if after.starts_with('\\') || has_unc_or_device_prefix(after) || is_dot_or_q_device(after) {
        return true;
    }
    if let Some(rest) = after.strip_prefix('/') {
        return rest.starts_with(['/', '\\'])
            || is_dot_or_q_device(rest)
            || has_unc_or_device_prefix(&format!("/{rest}"));
    }
    let host = after.split(['/', '\\', '?', '#']).next().unwrap_or("");
    if !host.eq_ignore_ascii_case("localhost") {
        return true;
    }
    let tail = &after[host.len()..];
    if tail.starts_with('\\') || has_unc_or_device_prefix(tail) {
        return true;
    }
    if let Some(rest) = tail.strip_prefix('/') {
        return rest.starts_with(['/', '\\']) || is_dot_or_q_device(rest);
    }
    false
}

fn is_dot_or_q_device(s: &str) -> bool {
    let b = s.as_bytes();
    (b.len() >= 2 && matches!(b[0], b'?' | b'.') && matches!(b[1], b'/' | b'\\'))
        || (b.len() >= 3 && b[0] == b'?' && b[1] == b'?' && matches!(b[2], b'/' | b'\\'))
}

fn percent_decode_ascii(s: &str) -> String {
    if !s.contains('%') {
        return s.to_owned();
    }
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%'
            && i + 2 < b.len()
            && let Ok(v) = u8::from_str_radix(&s[i + 1..i + 3], 16)
        {
            out.push(v);
            i += 3;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn is_drive_prefix(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() >= 2 && b[0].is_ascii_alphabetic() && b[1] == b':'
}

fn strip_file_scheme_slashes(s: &str) -> Option<&str> {
    let b = s.as_bytes();
    if b.len() >= 7
        && b[..5].eq_ignore_ascii_case(b"file:")
        && matches!(b[5], b'/' | b'\\')
        && matches!(b[6], b'/' | b'\\')
    {
        Some(&s[7..])
    } else {
        None
    }
}

/// Allow-lists ONLY non-redirecting local Windows reparse tags carried by ordinary files and
/// Windows 11 OneDrive / Cloud Filter folders:
/// - `IO_REPARSE_TAG_CLOUD`..`IO_REPARSE_TAG_CLOUD_F` (`(tag & !0x0000_F000) == 0x9000_001A`)
///   and legacy `IO_REPARSE_TAG_ONEDRIVE` (`0x8000_0021`): Windows Cloud Filter placeholders
///   (OneDrive Known Folder Move Desktop/Documents, iCloud, Dropbox, Google Drive).
/// - `IO_REPARSE_TAG_WOF` (`0x8000_0017`): Windows Overlay Filter transparent NTFS/WIM compression
///   (`Wof.sys`, e.g. `compact /exe:lzx`); stores local data metadata and never redirects paths.
/// - `IO_REPARSE_TAG_DEDUP` (`0x8000_0013`): NTFS Data Deduplication (`ddp.sys`) local volume
///   chunk-store metadata; never redirects path lookup.
///
/// Strictly refuses every name-surrogate / redirecting tag (bit 29 `0x2000_0000`: symlinks
/// `0xA000_000C`, mount points / junctions `0xA000_0003`, WSL symlinks `0xA000_001D`, global
/// reparse `0xA000_0019`), `IO_REPARSE_TAG_APPEXECLINK` (`0x8000_001B`), `IO_REPARSE_TAG_AF_UNIX`,
/// `IO_REPARSE_TAG_PROJFS`, `IO_REPARSE_TAG_STORAGE_SYNC`, and every unknown tag.
pub const fn is_allowed_local_reparse_tag(tag: u32) -> bool {
    const NAME_SURROGATE_BIT: u32 = 0x2000_0000;
    const IO_REPARSE_TAG_CLOUD_BASE: u32 = 0x9000_001A;
    const IO_REPARSE_TAG_CLOUD_MASK: u32 = 0x0000_F000;
    const IO_REPARSE_TAG_ONEDRIVE: u32 = 0x8000_0021;
    const IO_REPARSE_TAG_WOF: u32 = 0x8000_0017;
    const IO_REPARSE_TAG_DEDUP: u32 = 0x8000_0013;

    if (tag & NAME_SURROGATE_BIT) != 0 {
        return false;
    }
    (tag & !IO_REPARSE_TAG_CLOUD_MASK) == IO_REPARSE_TAG_CLOUD_BASE
        || matches!(tag, IO_REPARSE_TAG_ONEDRIVE | IO_REPARSE_TAG_WOF | IO_REPARSE_TAG_DEDUP)
}
