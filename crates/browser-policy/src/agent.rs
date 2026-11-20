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

pub(crate) fn check_dom_pierce_params(method: &str, params: &serde_json::Value) -> Result<(), String> {
    if params
        .get("pierce")
        .is_some_and(|v| !v.is_null() && v != &serde_json::Value::Bool(false))
    {
        return Err(format!("DevTools method '{method}' cannot enable pierce."));
    }
    Ok(())
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
        | "Page.createIsolatedWorld"
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
