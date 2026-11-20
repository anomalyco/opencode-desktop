//! Agent-specific browser URL, extension-origin, URL-glob, and typed DevTools policy helpers
//! (`packages/plugin-browser/src/rpc.ts`, `packages/gui-plugins/src/browser/{chromium,policy}.ts`).

use crate::check_cdp;

/// Whether a URL or origin belongs to a browser extension (`packages/plugin-browser/src/rpc.ts:29-36`):
/// its pages, frames, and `blob:` URLs it creates. Agent tools must never navigate to, inspect,
/// evaluate in, or click inside extension documents because they run with `chrome.*` privileges.
pub fn extension_origin(input: &str) -> bool {
    let mut check = input.trim();
    while let Some(rest) = strip_ascii_prefix(check, "blob:") {
        check = rest.trim();
    }
    let Some((scheme, _)) = check.split_once(':') else { return false };
    matches!(
        scheme.to_ascii_lowercase().as_str(),
        "chrome-extension"
            | "moz-extension"
            | "safari-web-extension"
            | "ms-browser-extension"
            | "extension"
    )
}

/// Refusal message when an agent attempts to open an extension page (`rpc.ts:39-41`).
pub fn extension_page_refusal(url: &str) -> String {
    format!(
        "Agent browser tools cannot open extension pages such as {url}: an extension page runs with that extension's privileges. Ask the user to open it from the extensions menu."
    )
}

/// Matches a URL against a plain substring or `*` glob (`packages/plugin-browser/src/rpc.ts:52-65`).
pub fn matches_url(url: &str, pattern: &str) -> bool {
    if !pattern.contains('*') {
        return url.contains(pattern);
    }
    let mut parts = pattern.split('*');
    let first = parts.next().unwrap_or("");
    let mut middle: Vec<&str> = parts.collect();
    let last = middle.pop().unwrap_or("");
    if !url.starts_with(first) || !url.ends_with(last) {
        return false;
    }
    let mut offset = first.len();
    for part in middle {
        if part.is_empty() {
            continue;
        }
        let Some(found) = url[offset..].find(part) else {
            return false;
        };
        offset += found + part.len();
    }
    offset + last.len() <= url.len()
}

fn strip_ascii_prefix<'a>(input: &'a str, prefix: &str) -> Option<&'a str> {
    if input.len() >= prefix.len() && input.as_bytes()[..prefix.len()].eq_ignore_ascii_case(prefix.as_bytes()) {
        Some(&input[prefix.len()..])
    } else {
        None
    }
}

/// Validates a DevTools Protocol call issued exclusively by Rune's typed Rust `experimental.browser`
/// executors (never exposed to untrusted Luau plugins or raw `browser_cdp`). Extends [`check_cdp`]
/// with read-only diagnostics, hit-testing, drag interception, and profiling while strictly
/// forbidding `userGesture: true`, raw filesystem paths (`DOM.setFileInputFiles`,
/// `Input.dispatchDragEvent` with `files`), cookie mutations, `Browser.*`, and `Extensions.*`.
pub fn check_typed_cdp(method: &str, params: &serde_json::Value, engine_has_policy: bool) -> Result<(), String> {
    if matches!(method, "Runtime.evaluate" | "Runtime.callFunctionOn") {
        for flag in ["userGesture", "includeCommandLineAPI", "allowUnsafeEvalBlockedByCSP"] {
            if params
                .get(flag)
                .is_some_and(|v| !v.is_null() && v != &serde_json::Value::Bool(false))
            {
                return Err(format!("DevTools method '{method}' cannot enable {flag}."));
            }
        }
        for forbidden in ["executionContextId", "uniqueContextId"] {
            if params.get(forbidden).is_some_and(|v| !v.is_null()) {
                return Err(format!(
                    "DevTools method '{method}' cannot target '{forbidden}'."
                ));
            }
        }
        // Native Rust RPC executors may pin an explicit integer `contextId` on `Runtime.evaluate`
        // (after verifying the owning non-extension page frame).
        if method == "Runtime.evaluate"
            && let Some(cid) = params.get("contextId")
            && !cid.is_null()
        {
            if !engine_has_policy {
                return Err(
                    "DevTools method 'Runtime.evaluate' with contextId requires an updated browser engine that enforces URL policy."
                        .into(),
                );
            }
            if cid.as_i64().is_none_or(|n| n <= 0) {
                return Err("DevTools method 'Runtime.evaluate' requires a positive integer contextId.".into());
            }
            return Ok(());
        }
    }
    if check_cdp(method, params, engine_has_policy).is_ok() {
        return Ok(());
    }
    if !engine_has_policy {
        return Err(format!("DevTools method '{method}' requires an updated browser engine that enforces URL policy."));
    }
    match method {
        "Page.enable"
        | "Accessibility.getPartialAXTree"
        | "Runtime.enable"
        | "Runtime.releaseObjectGroup"
        | "Log.enable"
        | "Network.enable"
        | "Network.getResponseBody"
        | "Network.getRequestPostData"
        | "DOM.getNodeForLocation"
        | "Target.setAutoAttach"
        | "Target.detachFromTarget"
        | "Input.setInterceptDrags"
        | "Profiler.enable"
        | "Profiler.start"
        | "Profiler.stop"
        | "Profiler.disable"
        | "Tracing.start"
        | "Tracing.end"
        | "IO.read"
        | "IO.close"
        | "HeapProfiler.enable"
        | "HeapProfiler.takeHeapSnapshot"
        | "HeapProfiler.disable" => Ok(()),
        "Input.dispatchDragEvent" => {
            if params
                .get("data")
                .and_then(|d| d.get("files"))
                .and_then(|f| f.as_array())
                .is_some_and(|a| !a.is_empty())
            {
                return Err("DevTools method 'Input.dispatchDragEvent' cannot pass arbitrary host file paths.".into());
            }
            Ok(())
        }
        _ => Err(format!("DevTools method '{method}' is not permitted in the built-in browser.")),
    }
}

/// Whether `hostname` names a loopback address (`localhost`, `*.localhost`, `127.0.0.0/8` in
/// dotted/shorthand/decimal/hex/octal form, `::1`, or IPv4-mapped IPv6 loopback).
pub fn loopback_host(hostname: &str) -> bool {
    let h = hostname.trim().trim_end_matches('.');
    if h.eq_ignore_ascii_case("localhost") {
        return true;
    }
    if h.len() > 10 && h.as_bytes()[h.len() - 10..].eq_ignore_ascii_case(b".localhost") {
        return true;
    }
    let unbracketed = h.strip_prefix('[').and_then(|s| s.strip_suffix(']')).unwrap_or(h);
    if let Ok(v6) = unbracketed.parse::<std::net::Ipv6Addr>() {
        return v6.is_loopback()
            || v6.to_ipv4_mapped().is_some_and(|v4| v4.is_loopback())
            || v6.to_ipv4().is_some_and(|v4| v4.is_loopback());
    }
    parse_ipv4_u32(unbracketed).is_some_and(|ip| (ip >> 24) == 127)
}

/// Like [`loopback_host`], also matching unspecified addresses (`0.0.0.0`, `::`, `::ffff:0.0.0.0`).
pub fn is_loopback_or_unspecified_host(hostname: &str) -> bool {
    if loopback_host(hostname) {
        return true;
    }
    let h = hostname.trim().trim_end_matches('.');
    let unbracketed = h.strip_prefix('[').and_then(|s| s.strip_suffix(']')).unwrap_or(h);
    if let Ok(v6) = unbracketed.parse::<std::net::Ipv6Addr>() {
        return v6.is_unspecified() || v6.to_ipv4_mapped().is_some_and(|v4| v4.is_unspecified());
    }
    parse_ipv4_u32(unbracketed).is_some_and(|ip| ip == 0)
}

/// Parses WHATWG/BSD IPv4 forms (dotted, `127.1`, `2130706433`, `0x7f000001`, `0177.0.0.1`).
fn parse_ipv4_u32(input: &str) -> Option<u32> {
    if input.is_empty() {
        return None;
    }
    let parts: Vec<&str> = input.split('.').collect();
    if parts.is_empty() || parts.len() > 4 {
        return None;
    }
    let mut nums = Vec::with_capacity(parts.len());
    for part in &parts {
        if part.is_empty() {
            return None;
        }
        let n = if let Some(hex) = part.strip_prefix("0x").or_else(|| part.strip_prefix("0X")) {
            if hex.is_empty() { return None; }
            u64::from_str_radix(hex, 16).ok()?
        } else if part.len() > 1 && part.starts_with('0') {
            u64::from_str_radix(part, 8).ok()?
        } else {
            part.parse::<u64>().ok()?
        };
        nums.push(n);
    }
    let last = *nums.last()?;
    let limit = 1u64 << (8 * (5 - nums.len()));
    if last >= limit {
        return None;
    }
    let mut ip = last;
    for (i, &n) in nums[..nums.len() - 1].iter().enumerate() {
        if n > 255 {
            return None;
        }
        ip |= n << (8 * (3 - i));
    }
    u32::try_from(ip).ok()
}
