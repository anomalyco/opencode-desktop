//! URL policy enforced inside the sidecar (`RequestHandler` and `ResourceRequestHandler`),
//! matching `crates/browser-pane/src/policy.rs` and OpenCode Desktop (`policy.ts`,
//! `pane.ts:103-107`, `chromium.ts:432-446`, `contained-file.ts:17-18`).

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refusal {
    Credentials,
    Workspace,
    Web,
    Blocked,
}

impl Refusal {
    pub fn code(self) -> &'static str {
        match self {
            Self::Credentials => "browser.address.credentials",
            Self::Workspace => "browser.address.workspace",
            Self::Web => "browser.address.web",
            Self::Blocked => "ERR_BLOCKED_BY_CLIENT",
        }
    }
}

pub fn destination_origin(input: &str) -> Option<String> {
    let trimmed = input.trim();
    let (scheme, rest) = split_http_scheme(trimmed)?;
    let authority = rest.split(['/', '\\', '?', '#']).next().unwrap_or("");
    if authority.is_empty() || authority.contains('@') || authority.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return None;
    }
    let host_port = parse_host_port(authority)?;
    Some(format!("{scheme}://{host_port}"))
}

pub fn is_http_with_credentials(input: &str) -> bool {
    let trimmed = input.trim();
    let Some((_, rest)) = split_http_scheme(trimmed) else { return false };
    let authority = rest.split(['/', '\\', '?', '#']).next().unwrap_or("");
    if authority.is_empty() || authority.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return false;
    }
    let Some((_, host_port)) = authority.rsplit_once('@') else { return false };
    parse_host_port(host_port).is_some()
}

pub fn is_file_scheme(input: &str) -> bool {
    let trimmed = input.trim();
    trimmed.len() >= 5 && trimmed.as_bytes()[..5].eq_ignore_ascii_case(b"file:")
}

pub fn local_file_path(input: &str) -> Option<String> {
    let trimmed = input.trim();
    if trimmed.len() < 8 || !trimmed.as_bytes()[..7].eq_ignore_ascii_case(b"file://") {
        return None;
    }
    let after = &trimmed[7..];
    let raw_path = after.split(['?', '#']).next().unwrap_or(after);
    if !raw_path.starts_with('/') || raw_path.starts_with("//") {
        return None;
    }
    if contains_escaped_sep_or_nul(raw_path) {
        return None;
    }
    let decoded = percent_decode(raw_path)?;
    if decoded.chars().any(|c| (c as u32) <= 0x1f) {
        return None;
    }
    let normalized = decoded.replace('\\', "/");
    if cfg!(windows) {
        resolve_windows_file_path(&normalized)
    } else {
        resolve_unix_file_path(&normalized)
    }
}

pub fn file_url_within(input: &str, roots: &[String]) -> bool {
    if roots.is_empty() {
        return false;
    }
    let Some(target) = local_file_path(input) else { return false };
    roots.iter().any(|root| path_within_root(&target, root))
}

pub fn allowed_destination(input: &str, roots: &[String]) -> bool {
    destination_origin(input).is_some() || file_url_within(input, roots)
}

pub fn refusal(input: &str, roots: &[String]) -> Option<Refusal> {
    let trimmed = input.trim();
    if trimmed.eq_ignore_ascii_case("about:blank") || allowed_destination(trimmed, roots) {
        return None;
    }
    if is_http_with_credentials(trimmed) {
        return Some(Refusal::Credentials);
    }
    Some(if roots.is_empty() { Refusal::Web } else { Refusal::Workspace })
}

pub fn frame_guard(url: &str, is_main_frame: bool, roots: &[String]) -> Option<Refusal> {
    let trimmed = url.trim();
    if trimmed.eq_ignore_ascii_case("about:blank") {
        return None;
    }
    if is_main_frame {
        if allowed_destination(trimmed, roots) {
            return None;
        }
        return Some(refusal(trimmed, roots).unwrap_or(Refusal::Blocked));
    }
    if !is_file_scheme(trimmed) || file_url_within(trimmed, roots) {
        return None;
    }
    Some(if roots.is_empty() { Refusal::Web } else { Refusal::Workspace })
}

pub fn subresource_allowed(url: &str, roots: &[String]) -> bool {
    !is_file_scheme(url) || file_url_within(url, roots)
}

pub fn check_cdp(method: &str, params: &serde_json::Value) -> Result<(), String> {
    let trimmed = method.trim();
    match trimmed {
        "Accessibility.getFullAXTree"
        | "DOM.enable"
        | "DOM.getDocument"
        | "DOM.describeNode"
        | "DOM.resolveNode"
        | "DOM.getBoxModel"
        | "DOM.getContentQuads"
        | "DOM.scrollIntoViewIfNeeded"
        | "DOM.focus"
        | "DOM.getFrameOwner"
        | "DOM.querySelector"
        | "DOM.querySelectorAll"
        | "Input.dispatchMouseEvent"
        | "Input.insertText"
        | "Page.getFrameTree"
        | "Page.getLayoutMetrics"
        | "Page.captureScreenshot"
        | "Page.handleJavaScriptDialog"
        | "Runtime.releaseObject" => Ok(()),
        "Runtime.evaluate" | "Runtime.callFunctionOn" => {
            if params.get("includeCommandLineAPI").and_then(serde_json::Value::as_bool) == Some(true) {
                return Err(format!("DevTools method '{trimmed}' cannot enable includeCommandLineAPI."));
            }
            Ok(())
        }
        "Input.dispatchKeyEvent" => {
            if params.get("commands").is_some_and(|v| !v.is_null()) {
                return Err("DevTools method 'Input.dispatchKeyEvent' does not permit editing commands.".into());
            }
            let modifiers = params.get("modifiers").and_then(serde_json::Value::as_u64).unwrap_or(0);
            let shortcuts = (modifiers & 6) != 0;
            let ctrl = (modifiers & 2) != 0;
            let shift = (modifiers & 8) != 0;
            let key = params.get("key").and_then(serde_json::Value::as_str).unwrap_or("");
            let vk = params
                .get("windowsVirtualKeyCode")
                .and_then(serde_json::Value::as_u64)
                .or_else(|| params.get("nativeVirtualKeyCode").and_then(serde_json::Value::as_u64))
                .unwrap_or(0);
            if (shortcuts && (matches!(key, "c" | "C" | "v" | "V" | "x" | "X") || matches!(vk, 67 | 86 | 88)))
                || ((key.eq_ignore_ascii_case("Insert") || vk == 45) && (shift || ctrl))
                || (shift && (key.eq_ignore_ascii_case("Delete") || vk == 46 || key.eq_ignore_ascii_case("F10") || vk == 121))
            {
                return Err("DevTools method 'Input.dispatchKeyEvent' cannot send clipboard or context-menu shortcuts.".into());
            }
            Ok(())
        }
        _ => Err(format!("DevTools method '{trimmed}' is not permitted in the built-in browser.")),
    }
}

fn split_http_scheme(input: &str) -> Option<(&'static str, &str)> {
    if input.len() >= 7 && input.as_bytes()[..7].eq_ignore_ascii_case(b"http://") {
        return Some(("http", &input[7..]));
    }
    if input.len() >= 8 && input.as_bytes()[..8].eq_ignore_ascii_case(b"https://") {
        return Some(("https", &input[8..]));
    }
    None
}

fn parse_host_port(authority: &str) -> Option<String> {
    if authority.starts_with('[') {
        let end = authority.find(']')?;
        let ipv6 = &authority[1..end];
        if ipv6.is_empty() || !ipv6.chars().all(|c| c.is_ascii_hexdigit() || c == ':' || c == '.') {
            return None;
        }
        let tail = &authority[end + 1..];
        if tail.is_empty() {
            return Some(authority[..=end].to_ascii_lowercase());
        }
        let port_str = tail.strip_prefix(':')?;
        let port: u16 = port_str.parse().ok()?;
        return Some(format!("[{}]:{port}", ipv6.to_ascii_lowercase()));
    }
    let (host, port) = match authority.split_once(':') {
        Some((h, "")) => (h, None),
        Some((h, p)) => (h, Some(p.parse::<u16>().ok()?)),
        None => (authority, None),
    };
    if host.is_empty() || host.contains(':') || host.chars().any(|c| matches!(c, '/' | '\\' | '?' | '#' | '@' | '[' | ']') || c.is_whitespace() || c.is_control()) {
        return None;
    }
    match port {
        Some(p) => Some(format!("{}:{p}", host.to_ascii_lowercase())),
        None => Some(host.to_ascii_lowercase()),
    }
}

fn contains_escaped_sep_or_nul(raw: &str) -> bool {
    let bytes = raw.as_bytes();
    let mut i = 0;
    while i + 2 < bytes.len() {
        if bytes[i] == b'%' {
            let pair = [bytes[i + 1].to_ascii_lowercase(), bytes[i + 2].to_ascii_lowercase()];
            if matches!(&pair, b"2f" | b"5c" | b"00") {
                return true;
            }
            i += 3;
        } else {
            i += 1;
        }
    }
    false
}

fn percent_decode(raw: &str) -> Option<String> {
    let bytes = raw.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            if i + 2 >= bytes.len() {
                return None;
            }
            let hi = (bytes[i + 1] as char).to_digit(16)?;
            let lo = (bytes[i + 2] as char).to_digit(16)?;
            out.push(((hi << 4) | lo) as u8);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

fn resolve_windows_file_path(path: &str) -> Option<String> {
    let bytes = path.as_bytes();
    if bytes.len() < 3 || bytes[0] != b'/' || !bytes[1].is_ascii_alphabetic() || bytes[2] != b':' {
        return None;
    }
    if bytes.len() > 3 && bytes[3] != b'/' {
        return None;
    }
    let drive = bytes[1] as char;
    let rest = if bytes.len() > 3 { &path[4..] } else { "" };
    let mut stack: Vec<&str> = Vec::new();
    for seg in rest.split('/') {
        if seg.is_empty() || seg == "." {
            continue;
        }
        if seg == ".." {
            stack.pop()?;
            continue;
        }
        if !valid_windows_segment(seg) {
            return None;
        }
        stack.push(seg);
    }
    if stack.is_empty() {
        Some(format!("{drive}:"))
    } else {
        Some(format!("{drive}:/{}", stack.join("/")))
    }
}

fn valid_windows_segment(seg: &str) -> bool {
    if seg.contains(':') || seg.ends_with('.') || seg.ends_with(' ') {
        return false;
    }
    let stem = seg.split('.').next().unwrap_or(seg);
    !matches!(
        stem.to_ascii_lowercase().as_str(),
        "con" | "conin$" | "conout$" | "prn" | "aux" | "nul"
            | "com1" | "com2" | "com3" | "com4" | "com5" | "com6" | "com7" | "com8" | "com9"
            | "lpt1" | "lpt2" | "lpt3" | "lpt4" | "lpt5" | "lpt6" | "lpt7" | "lpt8" | "lpt9"
    )
}

fn resolve_unix_file_path(path: &str) -> Option<String> {
    let rest = path.strip_prefix('/')?;
    let mut stack: Vec<&str> = Vec::new();
    for seg in rest.split('/') {
        if seg.is_empty() || seg == "." {
            continue;
        }
        if seg == ".." {
            stack.pop()?;
            continue;
        }
        stack.push(seg);
    }
    Some(format!("/{}", stack.join("/")))
}

fn normalize_root_lex(root: &str) -> Option<String> {
    let trimmed = root.trim();
    if trimmed.is_empty() {
        return None;
    }
    let stripped = trimmed
        .strip_prefix(r"\\?\")
        .or_else(|| trimmed.strip_prefix("//?/"))
        .unwrap_or(trimmed)
        .replace('\\', "/");
    let stripped = if stripped.len() >= 3 && stripped.as_bytes()[0] == b'/' && stripped.as_bytes()[1].is_ascii_alphabetic() && stripped.as_bytes()[2] == b':' {
        &stripped[1..]
    } else {
        stripped.as_str()
    };
    let clean = if stripped == "/" { "/" } else { stripped.trim_end_matches('/') };
    if clean.is_empty() {
        return None;
    }
    Some(if cfg!(windows) { clean.to_ascii_lowercase() } else { clean.to_owned() })
}

fn lex_within(target: &str, root_lex: &str) -> bool {
    let cmp_target = if cfg!(windows) { target.to_ascii_lowercase() } else { target.to_owned() };
    if root_lex == "/" {
        return cmp_target.starts_with('/');
    }
    cmp_target == root_lex
        || (cmp_target.len() > root_lex.len()
            && cmp_target.starts_with(root_lex)
            && cmp_target.as_bytes()[root_lex.len()] == b'/')
}

fn path_within_root(target_lex: &str, root: &str) -> bool {
    let Some(root_lex) = normalize_root_lex(root) else { return false };
    let Ok(canon_root) = std::fs::canonicalize(root) else { return false };
    let canon_root_str = canon_root.to_string_lossy();
    let lex_ok = lex_within(target_lex, &root_lex)
        || normalize_root_lex(&canon_root_str).is_some_and(|cr| lex_within(target_lex, &cr));
    if !lex_ok {
        return false;
    }
    let os_target = if cfg!(windows) { target_lex.replace('/', "\\") } else { target_lex.to_owned() };
    let mut cur: &std::path::Path = std::path::Path::new(&os_target);
    loop {
        if std::fs::symlink_metadata(cur).is_ok() {
            let Ok(canon_cur) = std::fs::canonicalize(cur) else { return false };
            return canon_cur.starts_with(&canon_root);
        }
        let Some(parent) = cur.parent() else { return false };
        if parent == cur || parent.as_os_str().is_empty() {
            return false;
        }
        cur = parent;
    }
}
