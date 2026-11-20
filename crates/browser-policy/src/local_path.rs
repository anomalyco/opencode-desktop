//! Local filesystem path policy: refuses UNC/SMB shares, Windows device namespaces, NTFS
//! alternate data streams, reserved device names, control/bidi characters, and traversal segments
//! BEFORE any filesystem call (`exists`, `metadata`, `canonicalize`) or OS file-manager reveal.

use crate::is_windows_device_name;

/// Returns `true` if `raw` contains control bytes, NUL, Unicode bidi overrides/isolates, UNC/SMB
/// prefixes, NT device prefixes, drive-relative paths, NTFS alternate data streams, or Windows
/// reserved device names. Pure string inspection — never touches the filesystem.
pub fn is_unc_or_device_path(raw: &str) -> bool {
    if raw.is_empty() || raw.len() > 4096 {
        return true;
    }
    if raw.chars().any(|c| {
        let n = c as u32;
        n <= 0x1f
            || (0x7f..=0x9f).contains(&n)
            || matches!(n, 0x061c | 0x200e | 0x200f | 0x202a..=0x202e | 0x2066..=0x2069)
    }) {
        return true;
    }
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return true;
    }
    // UNC (`\\server\share`, `//server/share`), Win32 device namespace (`\\?\`, `\\.\`), NT `\??\`.
    if trimmed.starts_with(r"\\")
        || trimmed.starts_with("//")
        || trimmed.starts_with(r"\??\")
        || trimmed.starts_with("/??/")
        || trimmed.starts_with(r"\/")
        || trimmed.starts_with(r"/\")
    {
        return true;
    }
    // `file://` URL with a non-empty, non-localhost authority (`file://127.0.0.1/share/x`).
    if let Some(after) = strip_file_scheme_slashes(trimmed)
        && !after.starts_with('/')
        && !after.starts_with('\\')
    {
        let host = after.split(['/', '\\', '?', '#']).next().unwrap_or("");
        if !host.eq_ignore_ascii_case("localhost") {
            return true;
        }
    }
    let normalized = trimmed.replace('\\', "/");
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
    // Reject Windows reserved device names (`CON`, `NUL`, `COM1`, `LPT1`, etc.) in any segment.
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
    false
}

/// Detects raw UNC, SMB, NT device, or remote `file://host/...` hrefs BEFORE link classification
/// collapses slashes or touches the filesystem.
pub fn is_raw_unc_href(href: &str) -> bool {
    let trimmed = href.trim();
    if trimmed.starts_with(r"\\")
        || trimmed.starts_with("//")
        || trimmed.starts_with(r"\??\")
        || trimmed.starts_with("/??/")
        || trimmed.starts_with(r"\/")
        || trimmed.starts_with(r"/\")
    {
        return true;
    }
    if let Some(after) = strip_file_scheme_slashes(trimmed) {
        if after.starts_with(r"\\")
            || after.starts_with("//")
            || after.starts_with(r"?\")
            || after.starts_with("?/")
            || after.starts_with(r".\")
            || after.starts_with("./")
        {
            return true;
        }
        if !after.starts_with('/') && !after.starts_with('\\') {
            let host = after.split(['/', '\\', '?', '#']).next().unwrap_or("");
            if !host.eq_ignore_ascii_case("localhost") {
                return true;
            }
        }
    }
    false
}

/// Validates that `raw` is a clean, absolute local filesystem path safe to reveal in the OS file
/// manager (never UNC, never device namespace, never quotes/controls/bidi, never `..` traversal).
///
/// On Windows, requires an explicit `<drive>:\` or `<drive>:/` prefix (callers must still verify
/// `GetDriveTypeW == DRIVE_FIXED` before touching the path). On Unix/macOS, requires a single
/// leading `/` (never `//`).
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
        if !normalized.starts_with('/') || normalized.starts_with("//") {
            return None;
        }
        Some(raw.to_owned())
    }
}

/// Returns `true` if `path` names a macOS executable bundle or script package that `/usr/bin/open`
/// without `-R` would launch instead of revealing in Finder.
pub fn is_macos_bundle_path(path: &str) -> bool {
    const BUNDLES: &[&str] = &[
        ".app",
        ".pkg",
        ".mpkg",
        ".bundle",
        ".prefpane",
        ".workflow",
        ".action",
        ".command",
        ".tool",
        ".dmg",
    ];
    let trimmed = path.trim_end_matches(['/', '\\']);
    let lower = trimmed.to_ascii_lowercase();
    BUNDLES.iter().any(|ext| lower.ends_with(ext))
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
