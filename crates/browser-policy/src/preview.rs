//! Shared `oc-preview://` policy, URL parsing, scope confinement, artifact & remote workspace
//! path validation, MIME mapping, and HTTP Range parsing (`#63002`: `scheme.ts`, `preview.ts`,
//! `preview-path.ts`, `disk/preview-scope.ts`, `artifact-link.ts`, `profile/range.ts`).
//!
//! Pure, `#![forbid(unsafe_code)]`, and `wasm32`-safe so `opencode-browser-engine`,
//! `crates/browser-pane`, and `apps/desktop` enforce the exact same rules.

use crate::{
    contains_escaped_sep_or_nul, is_safe_workspace_root, is_windows_device_name, percent_decode,
    valid_windows_segment,
};

/// Isolated custom scheme for local workspace, remote workspace, and artifact previews.
pub const PREVIEW_SCHEME: &str = "oc-preview";
/// Display-only scheme for remote server workspace previews (`workspace://<session_id>/<path>`).
pub const WORKSPACE_SCHEME: &str = "workspace";
/// Display-only scheme for artifact previews (`artifact://<art_id>/<path>`).
pub const ARTIFACT_SCHEME: &str = "artifact";
/// Hard cap on local and remote workspace preview responses (128 MiB).
pub const MAX_PREVIEW_FILE_BYTES: u64 = 128 * 1024 * 1024;
/// Hard cap on artifact preview responses (32 MiB).
pub const MAX_ARTIFACT_BYTES: u64 = 32 * 1024 * 1024;

/// Strict CSP applied to artifact previews (`profile/serve.ts:38-39`).
pub const ARTIFACT_CSP: &str = "sandbox allow-scripts allow-same-origin; default-src 'self'; script-src 'self' 'unsafe-inline'; style-src 'self' 'unsafe-inline'; img-src 'self' data: blob:; font-src 'self' data:; media-src 'self' data: blob:; connect-src 'self'; object-src 'none'; base-uri 'none'; form-action 'none'; frame-ancestors 'self'";

/// Strict CSP applied to local and remote workspace previews (stricter than upstream `#63002`:
/// confines network/subresources/WebSockets/pop-ups/downloads/top-nav to the preview origin).
pub const WORKSPACE_PREVIEW_CSP: &str = "sandbox allow-scripts allow-same-origin; default-src 'self'; script-src 'self' 'unsafe-inline'; style-src 'self' 'unsafe-inline'; img-src 'self' data: blob:; font-src 'self' data:; media-src 'self' data: blob:; connect-src 'self' data: blob:; worker-src 'self' blob:; child-src 'self' blob:; object-src 'none'; base-uri 'none'; form-action 'none'; frame-ancestors 'self'";

/// Chromium's `Connection-Allowlist` header blocking speculative network/WebRTC outside own origin.
pub const CONNECTION_ALLOWLIST: &str = "(response-origin);webrtc=block";

const ARTIFACT_ROUTE: &str = "/api/experimental/artifacts/";

pub fn is_preview_scheme(input: &str) -> bool {
    let t = input.trim();
    t.len() >= 11 && t.as_bytes()[..11].eq_ignore_ascii_case(b"oc-preview:")
}

/// Whether `url` uses a banned raw scheme (`file:` or `oc-preview:`), including nested prefixes
/// such as `view-source:file:`, `filesystem:file:`, `blob:file:`, or `blob:oc-preview:`.
pub fn has_banned_scheme(url: &str) -> bool {
    let mut cur = url.trim();
    loop {
        if is_preview_scheme(cur) || crate::is_file_scheme(cur) {
            return true;
        }
        let next = ["view-source:", "filesystem:", "blob:"].iter().find_map(|p| {
            (cur.len() >= p.len() && cur.as_bytes()[..p.len()].eq_ignore_ascii_case(p.as_bytes()))
                .then(|| cur[p.len()..].trim())
        });
        match next {
            Some(rest) => cur = rest,
            None => return false,
        }
    }
}

pub fn is_workspace_scheme(input: &str) -> bool {
    let t = input.trim();
    t.len() >= 10 && t.as_bytes()[..10].eq_ignore_ascii_case(b"workspace:")
}

pub fn is_artifact_scheme(input: &str) -> bool {
    let t = input.trim();
    t.len() >= 9 && t.as_bytes()[..9].eq_ignore_ascii_case(b"artifact:")
}

pub fn is_html_path(path: &str) -> bool {
    let base = path.rsplit(['/', '\\']).next().unwrap_or(path);
    let lower = base.to_ascii_lowercase();
    lower.ends_with(".html") || lower.ends_with(".htm")
}

/// Returns whether a workspace root path uses Windows drive/UNC syntax (used to apply Windows
/// filename rules for remote Windows workspaces regardless of client OS, without rejecting valid
/// colons on remote Unix workspaces from a Windows client).
pub fn root_is_windows(root: &str) -> bool {
    let t = root.trim();
    let b = t.as_bytes();
    (b.len() >= 2 && b[0].is_ascii_alphabetic() && b[1] == b':')
        || t.starts_with(r"\\")
        || t.starts_with("//")
}

/// Stable, session-scoped display prefix for remote workspace files (`preview-path.ts:12-14`).
pub fn workspace_prefix(session_id: &str) -> String {
    format!("{WORKSPACE_SCHEME}://{}/", session_id.to_ascii_lowercase())
}

/// Encodes each path segment for URLs while preserving `/` separators.
pub fn encode_rel_path(path: &str) -> String {
    path.split('/')
        .map(encode_path_segment)
        .collect::<Vec<_>>()
        .join("/")
}

pub fn encode_path_segment(seg: &str) -> String {
    let mut out = String::with_capacity(seg.len());
    for b in seg.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'!' | b'~' | b'*' | b'\'' | b'(' | b')') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// Builds a `file:///` display URL from a local workspace root and workspace-relative path.
pub fn local_display_url(root: &str, rel_path: &str, suffix: &str) -> String {
    let norm = root.replace('\\', "/");
    let clean = norm.trim_end_matches('/');
    let encoded_root = clean
        .split('/')
        .enumerate()
        .map(|(i, seg)| {
            if i == 0 && seg.len() == 2 && seg.as_bytes()[0].is_ascii_alphabetic() && seg.as_bytes()[1] == b':' {
                seg.to_owned()
            } else {
                encode_path_segment(seg)
            }
        })
        .collect::<Vec<_>>()
        .join("/");
    let encoded_rel = encode_rel_path(rel_path);
    let prefix = if encoded_root.starts_with('/') {
        format!("file://{encoded_root}")
    } else {
        format!("file:///{encoded_root}")
    };
    if encoded_rel.is_empty() {
        format!("{prefix}{suffix}")
    } else {
        format!("{prefix}/{encoded_rel}{suffix}")
    }
}

/// Builds the isolated internal preview URL `oc-preview://<grant_host>/<encoded_rel>`.
pub fn preview_url(grant_host: &str, rel_path: &str, suffix: &str) -> String {
    let encoded = if rel_path.is_empty() { String::new() } else { encode_rel_path(rel_path) };
    format!("{PREVIEW_SCHEME}://{grant_host}/{encoded}{suffix}")
}

/// Computes a portable workspace-relative path from an already-normalized absolute `target` and
/// workspace `root`, returning `None` if `target` is outside `root` or fails preview path rules.
pub fn relative_to_root(target: &str, root: &str) -> Option<String> {
    let t = target.replace('\\', "/");
    let r = root.replace('\\', "/");
    let r = r.trim_end_matches('/');
    if r.is_empty() || t.len() <= r.len() {
        return None;
    }
    let is_win = root_is_windows(root);
    let matches_prefix = if is_win {
        t.as_bytes()[..r.len()].eq_ignore_ascii_case(r.as_bytes())
    } else {
        &t[..r.len()] == r
    };
    if !matches_prefix || t.as_bytes().get(r.len()) != Some(&b'/') {
        return None;
    }
    let rel = &t[r.len() + 1..];
    preview_path_valid(rel, is_win).then(|| rel.to_owned())
}

/// Computes a sanitized response MIME type (plus UTF-8 charset when applicable).
/// When `prefer_server` is `false` (remote workspace previews), derives MIME strictly from the
/// file extension (`mime_for_path`) and NEVER trusts `server_ct` (`serve.ts:78`, `review-rd281.md`).
pub fn sanitize_response_mime(server_ct: &str, path: &str, prefer_server: bool) -> String {
    let (ext_mime, ext_charset) = mime_for_path(path);
    if !prefer_server {
        return match ext_charset {
            Some(cs) => format!("{ext_mime}; charset={cs}"),
            None => ext_mime.to_owned(),
        };
    }
    let base = server_ct
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    let valid_token = !base.is_empty()
        && base.contains('/')
        && base.len() <= 128
        && base.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'/' | b'-' | b'+' | b'.'));
    if valid_token && base != "application/octet-stream" {
        let is_textual = base.starts_with("text/")
            || base.ends_with("javascript")
            || base.ends_with("+xml")
            || base == "application/json";
        if is_textual {
            let cs = ext_charset.unwrap_or("utf-8");
            return format!("{base}; charset={cs}");
        }
        return base;
    }
    if ext_mime != "application/octet-stream" {
        return match ext_charset {
            Some(cs) => format!("{ext_mime}; charset={cs}"),
            None => ext_mime.to_owned(),
        };
    }
    "application/octet-stream".into()
}

/// Validates a portable workspace-relative path (`preview-path.ts:2-9`), rejecting drive letters,
/// empty/dot segments, backslashes, control characters, and (when `windows_rules` is true)
/// Windows colons, trailing dots/spaces, and DOS device names including superscript `COM¹..³`/`LPT¹..³`.
pub fn preview_path_valid(path: &str, windows_rules: bool) -> bool {
    if path.is_empty() {
        return false;
    }
    let bytes = path.as_bytes();
    if bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' {
        return false;
    }
    path.split('/').all(|part| {
        !part.is_empty()
            && part != "."
            && part != ".."
            && !part.chars().any(|c| c == '\\' || c.is_control())
            && (!windows_rules || valid_windows_segment(part))
    })
}

/// Parses an unnormalized `prefix`-based address (e.g. `workspace://<sid>/...` or
/// `oc-preview://<host>/...`) BEFORE URL normalization so alternate authorities, dot segments,
/// trailing slashes, and encoded separators (`%2f`, `%5c`, `%00`) are rejected (`preview-path.ts:17-31`).
pub fn parse_prefixed_path(address: &str, prefix: &str, windows_rules: bool) -> Option<String> {
    if address.len() < prefix.len() || !address.as_bytes()[..prefix.len()].eq_ignore_ascii_case(prefix.as_bytes()) {
        return None;
    }
    let rest = &address[prefix.len()..];
    let encoded = rest.split(['?', '#']).next().unwrap_or(rest);
    if encoded.is_empty() || encoded.starts_with('/') || encoded.ends_with('/') || encoded.contains('\\') {
        return None;
    }
    if contains_escaped_sep_or_nul(encoded) {
        return None;
    }
    let decoded = percent_decode(encoded)?;
    preview_path_valid(&decoded, windows_rules).then_some(decoded)
}

/// Splits an `oc-preview://<grant_host>/<rel>[?q][#h]` URL into `(decoded_rel, query_and_hash_suffix)`,
/// strictly requiring exact `grant_host` with no userinfo, port, double-slash, or traversal.
pub fn parse_preview_url(url: &str, grant_host: &str, windows_rules: bool) -> Option<(String, String)> {
    let prefix = format!("{PREVIEW_SCHEME}://{grant_host}/");
    let rel = parse_prefixed_path(url, &prefix, windows_rules)?;
    let rest = &url[prefix.len()..];
    let suffix = rest.find(['?', '#']).map(|i| &rest[i..]).unwrap_or("");
    Some((rel, suffix.to_owned()))
}

/// Whether a preview of `document` inside `root` may read `path` (`disk/preview-scope.ts:9-45`).
///
/// - Entry document (`entry == true && path == document && is_html_path(document)`) is allowed once
///   so an explicit preview of `.well-known/index.html` works without opening sibling dotfiles.
/// - Subresources and linked pages must remain inside `document`'s folder or subfolders, under a
///   safe workspace directory (never `/`, drive root, UNC root, `$HOME`, `Desktop`/`Documents`/
///   `Downloads`/`AppData`/`Library`), with zero dot-segments or secret filenames.
pub fn preview_allowed(root: &str, document: &str, path: &str, entry: bool, home: Option<&str>) -> bool {
    let win_rules = root_is_windows(root);
    let doc_norm = document.replace('\\', "/");
    let path_norm = path.replace('\\', "/");
    let doc: Vec<&str> = doc_norm.split('/').collect();
    let file: Vec<&str> = path_norm.split('/').collect();
    if doc.is_empty() || file.is_empty() {
        return false;
    }
    let bad_part = |part: &str| {
        part.is_empty()
            || part == "."
            || part == ".."
            || (part.len() >= 2 && part.as_bytes()[0].is_ascii_alphabetic() && part.as_bytes()[1] == b':')
            || part.chars().any(char::is_control)
            || (win_rules && !valid_windows_segment(part))
    };
    if doc.iter().any(|p| bad_part(p)) || file.iter().any(|p| bad_part(p)) {
        return false;
    }
    let folder = &doc[..doc.len() - 1];
    let dir = if folder.is_empty() {
        root.trim_end_matches(['/', '\\']).to_owned()
    } else {
        format!("{}/{}", root.trim_end_matches(['/', '\\']), folder.join("/"))
    };
    // Always enforce workspace-root safety BEFORE the one-shot entry HTML early return so neither
    // local nor remote previews can target `/`, drive roots, UNC roots, or broad home/system folders.
    if !is_safe_workspace_root(root, home) || !is_safe_workspace_root(&dir, home) {
        return false;
    }
    let same = doc == file;
    if entry && same && is_html_path(&doc_norm) {
        return true;
    }
    if file.len() <= folder.len() || !folder.iter().zip(file.iter()).all(|(a, b)| a == b) {
        return false;
    }
    !file.iter().any(|part| is_hidden_or_secret_segment(part, win_rules))
}

fn is_hidden_or_secret_segment(part: &str, win_rules: bool) -> bool {
    if part.starts_with('.') {
        return true;
    }
    let lower = part.to_ascii_lowercase();
    matches!(
        lower.as_str(),
        "id_rsa" | "id_dsa" | "id_ecdsa" | "id_ed25519" | "credentials"
    ) || lower.starts_with("secrets.")
        || lower.ends_with(".pem")
        || lower.ends_with(".key")
        || lower.ends_with(".pfx")
        || lower.ends_with(".p12")
        || (win_rules && is_windows_device_name(lower.split('.').next().unwrap_or(&lower)))
}

/// Parsed artifact target (`artifact://<id>/<encoded_path>`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ArtifactItem {
    pub id: String,
    /// Percent-encoded relative path inside the artifact (may be empty for root `/`).
    pub path: String,
}

fn parse_artifact_parts(value: &str) -> Option<ArtifactItem> {
    if value.chars().any(|c| matches!(c, '?' | '#') || c.is_whitespace() || (c as u32) <= 0x1f) {
        return None;
    }
    let (id, rest) = value.split_once('/')?;
    if !is_valid_artifact_id(id) {
        return None;
    }
    if rest.is_empty() {
        return Some(ArtifactItem { id: id.to_owned(), path: String::new() });
    }
    let decoded = parse_prefixed_path(rest, "", true)?;
    Some(ArtifactItem { id: id.to_owned(), path: encode_rel_path(&decoded) })
}

pub fn is_valid_artifact_id(id: &str) -> bool {
    let Some(suffix) = id.strip_prefix("art_") else { return false };
    !suffix.is_empty() && suffix.chars().all(|c| c.is_ascii_alphanumeric())
}

pub fn artifact_view_path(item: &ArtifactItem) -> String {
    format!("{ARTIFACT_ROUTE}{}/view/{}", item.id, item.path)
}

/// Parses `artifact://art_<id>/<path>` (`artifact-link.ts:45-47`).
pub fn artifact_address(address: &str) -> Option<ArtifactItem> {
    let rest = address.strip_prefix("artifact://")?;
    parse_artifact_parts(rest)
}

/// Recognizes an artifact view link on `server_url` (or relative `/api/experimental/artifacts/...`)
/// and normalizes it to `artifact://<id>/<path>` (`artifact-link.ts:22-43`).
pub fn artifact_link(href: &str, server_url: &str) -> Option<String> {
    let rest = if let Some(r) = href.strip_prefix(ARTIFACT_ROUTE) {
        r
    } else {
        let server_origin = crate::destination_origin(server_url)?;
        let href_origin = crate::destination_origin(href)?;
        if !server_origin.eq_ignore_ascii_case(&href_origin) {
            return None;
        }
        let server_after = server_url.trim().split_once("://")?.1;
        let server_path = server_after
            .find('/')
            .map(|i| server_after[i..].split(['?', '#']).next().unwrap_or(""))
            .unwrap_or("")
            .trim_end_matches('/');
        let href_after = href.trim().split_once("://")?.1;
        let href_path = href_after.find('/').map(|i| &href_after[i..])?;
        let prefix = format!("{server_path}{ARTIFACT_ROUTE}");
        href_path.strip_prefix(&prefix)?
    };
    let (id, tail) = rest.split_once("/view")?;
    let path = match tail {
        "" => "",
        s => s.strip_prefix('/')?,
    };
    let item = parse_artifact_parts(&format!("{id}/{path}"))?;
    Some(format!("{ARTIFACT_SCHEME}://{}/{}", item.id, item.path))
}

/// Translates `oc-preview://<host>/<path>[?q][#h]` for an artifact grant into `(display_url, server_route)`,
/// stripping query & fragment so page scripts cannot inject server query parameters (`artifact-link.ts:50-57`).
pub fn artifact_resource(url: &str, host: &str, id: &str) -> Option<(String, String)> {
    let prefix = format!("{PREVIEW_SCHEME}://{host}/");
    if url.len() < prefix.len() || !url.as_bytes()[..prefix.len()].eq_ignore_ascii_case(prefix.as_bytes()) {
        return None;
    }
    let raw = url[prefix.len()..].split(['?', '#']).next().unwrap_or("");
    let item = parse_artifact_parts(&format!("{id}/{raw}"))?;
    let display = format!("{ARTIFACT_SCHEME}://{}/{}", item.id, item.path);
    let route = artifact_view_path(&item);
    Some((display, route))
}

/// Strict subresource/network allowlist for ANY request inside a preview `CefRequestContext`
/// (including Web Workers, `sendBeacon`, WebSockets, and prefetch). Allows ONLY the preview's
/// own `oc-preview://<grant_host>/...` origin plus inline `data:`, same-origin `blob:`, and `about:blank`/`about:srcdoc`.
pub fn preview_subresource_allowed(url: &str, grant_host: &str) -> bool {
    let t = url.trim();
    if t.eq_ignore_ascii_case("about:blank") || t.eq_ignore_ascii_case("about:srcdoc") {
        return true;
    }
    if t.len() >= 5 && t.as_bytes()[..5].eq_ignore_ascii_case(b"data:") {
        return true;
    }
    let own_origin = format!("{PREVIEW_SCHEME}://{grant_host}");
    if let Some(rest) = strip_ascii_prefix(t, &own_origin) {
        return rest.is_empty() || rest.starts_with('/');
    }
    let own_blob = format!("blob:{own_origin}/");
    if strip_ascii_prefix(t, &own_blob).is_some() {
        return true;
    }
    false
}

/// Whether `initiator` (Chromium's `request_initiator`) matches `oc-preview://<grant_host>`.
/// Returns `false` for missing/empty/`"null"` initiators; browser-initiated top-level navigations
/// are handled separately by `preview/context.rs`.
pub fn preview_initiator_allowed(initiator: Option<&str>, grant_host: &str) -> bool {
    let Some(raw) = initiator.map(str::trim).filter(|s| !s.is_empty()) else {
        return false;
    };
    let expected = format!("{PREVIEW_SCHEME}://{grant_host}");
    raw.eq_ignore_ascii_case(&expected)
}

fn parse_ascii_u64(s: &str) -> Option<u64> {
    if s.is_empty() || !s.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    s.parse::<u64>().ok()
}

/// Validates and canonicalizes a single HTTP `Range` request header (`bytes=<start>-<end>`,
/// `bytes=<start>-`, or `bytes=-<suffix>`) before forwarding to a remote preview host.
/// Rejects multi-ranges, signs, whitespace, CRLF, or malformed syntax.
pub fn normalize_range_header(raw: Option<&str>) -> Option<String> {
    let trimmed = raw?.trim();
    if trimmed.chars().any(char::is_control) {
        return None;
    }
    let spec = strip_ascii_prefix(trimmed, "bytes=")?;
    let (first_str, last_str) = spec.split_once('-')?;
    if last_str.contains(',') {
        return None;
    }
    match (first_str.is_empty(), last_str.is_empty()) {
        (false, true) => {
            let start = parse_ascii_u64(first_str)?;
            Some(format!("bytes={start}-"))
        }
        (false, false) => {
            let start = parse_ascii_u64(first_str)?;
            let end = parse_ascii_u64(last_str)?;
            (start <= end).then(|| format!("bytes={start}-{end}"))
        }
        (true, false) => {
            let suffix = parse_ascii_u64(last_str)?;
            (suffix > 0).then(|| format!("bytes=-{suffix}"))
        }
        (true, true) => None,
    }
}

/// Strictly parses a `206 Partial Content` `Content-Range: bytes <start>-<end>/<total>` header.
pub fn parse_content_range(header: Option<&str>) -> Option<(u64, u64, u64)> {
    let trimmed = header?.trim();
    let rest = strip_ascii_prefix(trimmed, "bytes ")?;
    let (range_part, total_str) = rest.split_once('/')?;
    let (start_str, end_str) = range_part.split_once('-')?;
    let start = parse_ascii_u64(start_str)?;
    let end = parse_ascii_u64(end_str)?;
    let total = parse_ascii_u64(total_str)?;
    if total == 0 || start > end || end >= total {
        return None;
    }
    Some((start, end, total))
}

fn strip_ascii_prefix<'a>(s: &'a str, prefix: &str) -> Option<&'a str> {
    if s.len() >= prefix.len() && s.as_bytes()[..prefix.len()].eq_ignore_ascii_case(prefix.as_bytes()) {
        Some(&s[prefix.len()..])
    } else {
        None
    }
}

/// Parsed HTTP single-byte-range response plan (`profile/range.ts:2-14`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RangeSpec {
    Full { length: u64 },
    Partial { start: u64, end: u64, length: u64, content_range: String, header: String },
    Unsatisfiable { content_range: String },
}

pub fn parse_range(header: Option<&str>, size: u64) -> RangeSpec {
    let Some(raw) = header else {
        return RangeSpec::Full { length: size };
    };
    let trimmed = raw.trim();
    let Some(spec) = strip_ascii_prefix(trimmed, "bytes=") else {
        return RangeSpec::Unsatisfiable { content_range: format!("bytes */{size}") };
    };
    let Some((first_str, last_str)) = spec.split_once('-') else {
        return RangeSpec::Unsatisfiable { content_range: format!("bytes */{size}") };
    };
    if last_str.contains(',') || (first_str.is_empty() && last_str.is_empty()) {
        return RangeSpec::Unsatisfiable { content_range: format!("bytes */{size}") };
    }
    if size == 0 {
        return RangeSpec::Unsatisfiable { content_range: "bytes */0".into() };
    }
    let (start, end) = match (first_str.is_empty(), last_str.is_empty()) {
        (false, true) => {
            let Some(s) = parse_ascii_u64(first_str) else {
                return RangeSpec::Unsatisfiable { content_range: format!("bytes */{size}") };
            };
            (s, size - 1)
        }
        (false, false) => {
            let (Some(s), Some(e)) = (parse_ascii_u64(first_str), parse_ascii_u64(last_str)) else {
                return RangeSpec::Unsatisfiable { content_range: format!("bytes */{size}") };
            };
            (s, e.min(size - 1))
        }
        (true, false) => {
            let Some(suffix) = parse_ascii_u64(last_str) else {
                return RangeSpec::Unsatisfiable { content_range: format!("bytes */{size}") };
            };
            if suffix == 0 {
                return RangeSpec::Unsatisfiable { content_range: format!("bytes */{size}") };
            }
            (size.saturating_sub(suffix), size - 1)
        }
        (true, true) => return RangeSpec::Unsatisfiable { content_range: format!("bytes */{size}") },
    };
    if start > end || start >= size {
        return RangeSpec::Unsatisfiable { content_range: format!("bytes */{size}") };
    }
    let length = end - start + 1;
    RangeSpec::Partial {
        start,
        end,
        length,
        content_range: format!("bytes {start}-{end}/{size}"),
        header: format!("bytes={start}-{end}"),
    }
}

/// Returns `(mime_type, optional_charset)` for a workspace file path (always paired with `nosniff`).
pub fn mime_for_path(path: &str) -> (&'static str, Option<&'static str>) {
    let base = path.rsplit(['/', '\\']).next().unwrap_or(path);
    let ext = base.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase());
    match ext.as_deref() {
        Some("html" | "htm") => ("text/html", Some("utf-8")),
        Some("css") => ("text/css", Some("utf-8")),
        Some("js" | "mjs" | "cjs") => ("text/javascript", Some("utf-8")),
        Some("json" | "map") => ("application/json", Some("utf-8")),
        Some("svg") => ("image/svg+xml", Some("utf-8")),
        Some("png") => ("image/png", None),
        Some("jpg" | "jpeg") => ("image/jpeg", None),
        Some("gif") => ("image/gif", None),
        Some("webp") => ("image/webp", None),
        Some("avif") => ("image/avif", None),
        Some("ico") => ("image/x-icon", None),
        Some("bmp") => ("image/bmp", None),
        Some("woff") => ("font/woff", None),
        Some("woff2") => ("font/woff2", None),
        Some("ttf") => ("font/ttf", None),
        Some("otf") => ("font/otf", None),
        Some("wasm") => ("application/wasm", None),
        Some("mp4" | "m4v") => ("video/mp4", None),
        Some("webm") => ("video/webm", None),
        Some("ogg" | "ogv") => ("video/ogg", None),
        Some("mp3") => ("audio/mpeg", None),
        Some("wav") => ("audio/wav", None),
        Some("pdf") => ("application/pdf", None),
        Some("txt" | "log") => ("text/plain", Some("utf-8")),
        Some("md" | "markdown") => ("text/markdown", Some("utf-8")),
        Some("csv") => ("text/csv", Some("utf-8")),
        Some("xml") => ("application/xml", Some("utf-8")),
        Some("yaml" | "yml") => ("text/yaml", Some("utf-8")),
        _ => ("application/octet-stream", None),
    }
}

/// Sanitizes an upstream artifact `Content-Type` header (`profile/serve.ts:37`),
/// returning `(mime_type, optional_charset)`.
pub fn sanitize_content_type(raw: &str) -> (String, Option<String>) {
    let trimmed = raw.trim();
    let (essence, charset) = match trimmed.split_once(';') {
        Some((m, rest)) => {
            let c = rest.trim();
            if c.eq_ignore_ascii_case("charset=utf-8") {
                (m.trim(), Some("utf-8".to_owned()))
            } else {
                return ("application/octet-stream".into(), None);
            }
        }
        None => (trimmed, None),
    };
    let Some((ty, sub)) = essence.split_once('/') else {
        return ("application/octet-stream".into(), None);
    };
    let valid_part = |p: &str| {
        !p.is_empty() && p.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'.' | b'+' | b'-'))
    };
    if valid_part(ty) && valid_part(sub) {
        (format!("{}/{}", ty.to_ascii_lowercase(), sub.to_ascii_lowercase()), charset)
    } else {
        ("application/octet-stream".into(), None)
    }
}
