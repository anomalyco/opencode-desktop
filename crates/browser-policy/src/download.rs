//! Download filename sanitization, bidi stripping, executable-type detection, and URL allow-list
//! shared identically by `browser-pane` (app) and `opencode-browser-engine` (sidecar).

/// Maximum character count for a sanitized download filename (preserving the trailing extension).
pub const MAX_DOWNLOAD_NAME_CHARS: usize = 120;
/// Maximum character count for a reported download MIME type.
const MAX_DOWNLOAD_MIME_CHARS: usize = 128;
/// Suffix appended when neutralizing an executable filename for a human Save dialog or disk path.
const NEUTRALIZE_SUFFIX: &str = ".download";

/// Returns `true` for Unicode bidirectional formatting controls and BOM/ZWNBSP that can spoof
/// filenames (for example `invoice_\u{202E}fdp.exe`).
pub fn is_bidi_control(c: char) -> bool {
    matches!(
        c,
        '\u{061C}'
            | '\u{200E}'
            | '\u{200F}'
            | '\u{202A}'..='\u{202E}'
            | '\u{2066}'..='\u{2069}'
            | '\u{FEFF}'
    )
}

/// Sanitizes an untrusted download filename (`suggested_name`) for safe UI display and cross-OS
/// filesystem storage:
/// - Bounds input scanning before allocating,
/// - Strips Unicode bidi overrides/isolates/marks (`U+202A..=U+202E`, `U+2066..=U+2069`,
///   `U+200E`, `U+200F`, `U+061C`, `U+FEFF`),
/// - Replaces filesystem-unsafe characters (`< > : " / \ | ? *`) and control characters with `_`,
/// - Drops leading spaces and trailing dots/spaces,
/// - Preserves at most the trailing `MAX_DOWNLOAD_NAME_CHARS` (120) characters (keeping the real
///   extension intact),
/// - Replaces empty, `"."`, or `".."` with `"file"`,
/// - Prefixes `_` onto Windows reserved device names (`CON`, `NUL.txt`, `COM1`, `COM¹`, etc.).
pub fn sanitize_download_name(raw: &str) -> String {
    // Bound work on hostile multi-megabyte `Content-Disposition` filenames while keeping the tail
    // (where the extension lives).
    let bounded = tail_bytes(raw, 4096);
    let cleaned: String = bounded
        .chars()
        .filter(|&c| !is_bidi_control(c))
        .map(|c| {
            if matches!(c, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*') || c.is_control() {
                '_'
            } else {
                c
            }
        })
        .collect();
    let trimmed = cleaned.trim_start_matches(' ').trim_end_matches(['.', ' ']);
    let chars: Vec<char> = trimmed.chars().collect();
    let tail: String = chars[chars.len().saturating_sub(MAX_DOWNLOAD_NAME_CHARS)..]
        .iter()
        .collect();
    let tail = tail.trim_start_matches(' ').trim_end_matches(['.', ' ']);
    if tail.is_empty() || tail == "." || tail == ".." {
        return "file".to_owned();
    }
    let stem = tail.split('.').next().unwrap_or_default().trim_end();
    if crate::is_windows_device_name(stem) {
        let mut out = String::with_capacity(tail.len().min(MAX_DOWNLOAD_NAME_CHARS) + 1);
        out.push('_');
        if tail.chars().count() >= MAX_DOWNLOAD_NAME_CHARS {
            // Drop the first character after prefixing `_` (e.g. `CON...exe` -> `_ON...exe`)
            // so the trailing extension is never truncated.
            for c in tail.chars().skip(1) {
                out.push(c);
            }
        } else {
            out.push_str(tail);
        }
        return out.trim_end_matches(['.', ' ']).to_owned();
    }
    tail.to_owned()
}

/// Sanitizes and bounds a reported download MIME type for wire transport and UI display.
pub fn sanitize_download_mime(raw: &str) -> String {
    let clean: String = raw
        .chars()
        .filter(|&c| !is_bidi_control(c) && !c.is_control())
        .take(MAX_DOWNLOAD_MIME_CHARS)
        .collect();
    let trimmed = clean.trim();
    if trimmed.is_empty() {
        "application/octet-stream".to_owned()
    } else {
        trimmed.to_owned()
    }
}

/// Returns `true` when `name` ends in an OS executable, script, installer, bundle, shortcut, or
/// disk-image extension that must never be written silently or suggested un-neutralized.
pub fn is_dangerous_download_extension(name: &str) -> bool {
    let clean = sanitize_download_name(name);
    let Some((stem, ext)) = clean.rsplit_once('.') else {
        return false;
    };
    if stem.is_empty() || ext.is_empty() {
        return false;
    }
    let ext = ext.to_ascii_lowercase();
    matches!(
        ext.as_str(),
        // Windows executables, installers, scripts, shortcuts, disk images
        "exe"
            | "bat"
            | "cmd"
            | "com"
            | "cpl"
            | "pif"
            | "scr"
            | "ps1"
            | "psm1"
            | "psd1"
            | "ps1xml"
            | "msi"
            | "msp"
            | "mst"
            | "msix"
            | "msixbundle"
            | "appx"
            | "appxbundle"
            | "lnk"
            | "url"
            | "scf"
            | "inf"
            | "ins"
            | "isp"
            | "reg"
            | "msc"
            | "vbs"
            | "vbe"
            | "vb"
            | "wsf"
            | "wsh"
            | "hta"
            | "jse"
            | "chm"
            | "sct"
            | "wsc"
            | "application"
            | "gadget"
            | "diagcab"
            | "settingcontent-ms"
            | "library-ms"
            | "searchconnector-ms"
            | "shb"
            | "shs"
            | "dll"
            | "sys"
            | "drv"
            | "ocx"
            | "jar"
            | "jnlp"
            | "iso"
            | "img"
            | "vhd"
            | "vhdx"
            // macOS bundles, installers, scripts, internet/terminal launchers
            | "app"
            | "dmg"
            | "pkg"
            | "mpkg"
            | "command"
            | "tool"
            | "action"
            | "workflow"
            | "terminal"
            | "scpt"
            | "applescript"
            | "osa"
            | "inetloc"
            | "webloc"
            | "fileloc"
            // Linux / Unix scripts, packages, desktop launchers
            | "sh"
            | "bash"
            | "zsh"
            | "csh"
            | "ksh"
            | "run"
            | "desktop"
            | "appimage"
            | "deb"
            | "rpm"
    )
}

/// Returns `true` when `mime` denotes a directly-executable binary, installer, or OS shell script.
pub fn is_dangerous_download_mime(mime: &str) -> bool {
    let base = mime.split(';').next().unwrap_or_default().trim().to_ascii_lowercase();
    matches!(
        base.as_str(),
        "application/x-msdownload"
            | "application/x-msdos-program"
            | "application/x-dosexec"
            | "application/x-executable"
            | "application/x-mach-binary"
            | "application/x-elf"
            | "application/x-sharedlib"
            | "application/x-msi"
            | "application/x-ms-shortcut"
            | "application/x-bat"
            | "application/x-csh"
            | "application/x-sh"
            | "application/x-shellscript"
            | "text/x-shellscript"
            | "application/hta"
            | "application/x-apple-diskimage"
            | "application/java-archive"
            | "application/x-java-jnlp-file"
            | "application/x-powershell"
            | "application/vnd.microsoft.portable-executable"
    )
}

/// Returns `true` when either the sanitized filename extension or the MIME type is executable.
pub fn is_dangerous_download(name: &str, mime: &str) -> bool {
    is_dangerous_download_extension(name) || is_dangerous_download_mime(mime)
}

/// Returns a sanitized filename with `.download` appended whenever the filename or MIME type is
/// dangerous, staying within `MAX_DOWNLOAD_NAME_CHARS`.
pub fn neutralize_download_name(raw: &str, mime: &str) -> String {
    let clean = sanitize_download_name(raw);
    if !is_dangerous_download(&clean, mime) {
        return clean;
    }
    let max_prefix = MAX_DOWNLOAD_NAME_CHARS - NEUTRALIZE_SUFFIX.len();
    let chars: Vec<char> = clean.chars().collect();
    let prefix: String = chars[chars.len().saturating_sub(max_prefix)..].iter().collect();
    let prefix = prefix.trim_start_matches(' ').trim_end_matches(['.', ' ']);
    let prefix = if prefix.is_empty() || prefix == "." || prefix == ".." { "file" } else { prefix };
    let stem = prefix.split('.').next().unwrap_or_default().trim_end();
    if crate::is_windows_device_name(stem) {
        let mut out = String::with_capacity(prefix.len() + NEUTRALIZE_SUFFIX.len() + 1);
        out.push('_');
        if prefix.chars().count() >= max_prefix {
            for c in prefix.chars().skip(1) {
                out.push(c);
            }
        } else {
            out.push_str(prefix);
        }
        let trimmed = out.trim_end_matches(['.', ' ']);
        return format!("{trimmed}{NEUTRALIZE_SUFFIX}");
    }
    format!("{prefix}{NEUTRALIZE_SUFFIX}")
}

/// Strict allow-list for download URLs (`can_download`, `on_before_download`, and `SaveImage`).
///
/// Allows only:
/// - `http:` / `https:` without embedded credentials,
/// - `data:` URLs,
/// - `blob:` URLs whose inner origin is `null/` or credential-free `http:`/`https:`.
///
/// Unconditionally rejects `file:`, `oc-preview:`, `chrome-extension:`, `chrome:`, `devtools:`,
/// credentialed `http(s):`, `blob:chrome-extension:`, `blob:oc-preview:`, and `blob:file:`.
pub fn download_url_allowed(url: &str) -> bool {
    let trimmed = url.trim();
    if trimmed.is_empty() {
        return false;
    }
    if crate::destination_origin(trimmed).is_some() {
        return true;
    }
    let lower = trimmed.to_ascii_lowercase();
    if lower.starts_with("data:") {
        return true;
    }
    if let Some(rest) = trimmed.strip_prefix("blob:").or_else(|| {
        lower.starts_with("blob:").then(|| &trimmed[5..])
    }) {
        let rest_lower = rest.to_ascii_lowercase();
        return rest_lower.starts_with("null/") || crate::destination_origin(rest).is_some();
    }
    false
}

fn tail_bytes(s: &str, max_bytes: usize) -> &str {
    if s.len() <= max_bytes {
        return s;
    }
    let mut start = s.len() - max_bytes;
    while start < s.len() && !s.is_char_boundary(start) {
        start += 1;
    }
    &s[start..]
}
