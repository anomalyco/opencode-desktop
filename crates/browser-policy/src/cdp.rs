//! DevTools Protocol allow-list and parameter guards shared between desktop and browser-engine.

/// Validates a DevTools Protocol call against the 22-method allow-list: 17 protocol calls used by
/// OpenCode Desktop's fixed actions (`packages/gui-plugins/src/browser/chromium.ts:592,796-880,
/// 912,938,974,1003,1026,1205,1267-1389,1469-1484,1698-1722`) plus 5 read-only `DOM` inspection
/// methods (`DOM.getDocument`, `DOM.getBoxModel`, `DOM.getContentQuads`, `DOM.querySelector`,
/// `DOM.querySelectorAll`) used by Rune's tools. Enforced in both the app and the sidecar.
pub fn check_cdp(method: &str, params: &serde_json::Value, engine_has_policy: bool) -> Result<(), String> {
    if !engine_has_policy {
        return match method {
            "Accessibility.getFullAXTree" | "Page.captureScreenshot" => Ok(()),
            "Runtime.evaluate" => check_runtime_params(method, params),
            _ => Err(format!("DevTools method '{method}' requires an updated browser engine that enforces URL policy.")),
        };
    }
    match method {
        "Accessibility.getFullAXTree"
        | "DOM.enable"
        | "DOM.getDocument"
        | "DOM.describeNode"
        | "DOM.getBoxModel"
        | "DOM.getContentQuads"
        | "DOM.scrollIntoViewIfNeeded"
        | "DOM.focus"
        | "DOM.getFrameOwner"
        | "DOM.querySelector"
        | "DOM.querySelectorAll"
        | "Input.insertText"
        | "Page.getFrameTree"
        | "Page.getLayoutMetrics"
        | "Page.captureScreenshot"
        | "Page.handleJavaScriptDialog"
        | "Runtime.releaseObject" => Ok(()),
        "DOM.resolveNode" => check_no_context_params(method, params),
        "Input.dispatchMouseEvent" => check_mouse_params(params),
        "Runtime.evaluate" | "Runtime.callFunctionOn" => check_runtime_params(method, params),
        "Input.dispatchKeyEvent" => check_key_params(params),
        _ => Err(format!("DevTools method '{method}' is not permitted in the built-in browser.")),
    }
}

fn check_mouse_params(params: &serde_json::Value) -> Result<(), String> {
    if let Some(btn) = params.get("button").and_then(serde_json::Value::as_str)
        && btn.eq_ignore_ascii_case("middle")
    {
        return Err("DevTools method 'Input.dispatchMouseEvent' does not permit middle-click paste.".into());
    }
    if let Some(btns) = params.get("buttons").and_then(serde_json::Value::as_i64)
        && (btns & 4) != 0
    {
        return Err("DevTools method 'Input.dispatchMouseEvent' does not permit middle-button paste.".into());
    }
    Ok(())
}

fn check_no_context_params(method: &str, params: &serde_json::Value) -> Result<(), String> {
    for field in ["contextId", "executionContextId", "uniqueContextId"] {
        if params.get(field).is_some_and(|v| !v.is_null()) {
            return Err(format!(
                "DevTools method '{method}' cannot target an explicit execution context ('{field}')."
            ));
        }
    }
    Ok(())
}

fn check_runtime_params(method: &str, params: &serde_json::Value) -> Result<(), String> {
    for flag in ["includeCommandLineAPI", "userGesture", "allowUnsafeEvalBlockedByCSP"] {
        if let Some(v) = params.get(flag)
            && !v.is_null()
            && v != &serde_json::Value::Bool(false)
        {
            return Err(format!("DevTools method '{method}' cannot enable {flag}."));
        }
    }
    check_no_context_params(method, params)
}

fn check_bounded_key_int(params: &serde_json::Value, field: &str, max: i64) -> Result<u64, String> {
    let Some(val) = params.get(field) else { return Ok(0) };
    if val.is_null() {
        return Ok(0);
    }
    let Some(n) = val.as_i64() else {
        return Err(format!(
            "DevTools method 'Input.dispatchKeyEvent' requires '{field}' to be an integer in 0..={max}."
        ));
    };
    if !(0..=max).contains(&n) {
        return Err(format!(
            "DevTools method 'Input.dispatchKeyEvent' requires '{field}' to be in 0..={max}."
        ));
    }
    Ok(n as u64)
}

fn check_key_params(params: &serde_json::Value) -> Result<(), String> {
    if params.get("commands").is_some_and(|v| !v.is_null()) {
        return Err("DevTools method 'Input.dispatchKeyEvent' does not permit editing commands.".into());
    }
    let modifiers = check_bounded_key_int(params, "modifiers", 15)?;
    let wvk = check_bounded_key_int(params, "windowsVirtualKeyCode", 255)?;
    let nvk = check_bounded_key_int(params, "nativeVirtualKeyCode", 255)?;
    let shortcuts = (modifiers & 6) != 0;
    let ctrl = (modifiers & 2) != 0;
    let shift = (modifiers & 8) != 0;
    let key = params.get("key").and_then(serde_json::Value::as_str).unwrap_or("");
    let code = params.get("code").and_then(serde_json::Value::as_str).unwrap_or("");
    let is_cvx = matches!(key, "c" | "C" | "v" | "V" | "x" | "X")
        || code.eq_ignore_ascii_case("KeyC")
        || code.eq_ignore_ascii_case("KeyV")
        || code.eq_ignore_ascii_case("KeyX")
        || matches!(wvk, 67 | 86 | 88)
        || matches!(nvk, 67 | 86 | 88);
    let is_insert = key.eq_ignore_ascii_case("Insert")
        || code.eq_ignore_ascii_case("Insert")
        || wvk == 45
        || nvk == 45;
    let is_del_or_f10 = key.eq_ignore_ascii_case("Delete")
        || code.eq_ignore_ascii_case("Delete")
        || key.eq_ignore_ascii_case("F10")
        || code.eq_ignore_ascii_case("F10")
        || matches!(wvk, 46 | 121)
        || matches!(nvk, 46 | 121);
    let is_menu = key.eq_ignore_ascii_case("ContextMenu")
        || code.eq_ignore_ascii_case("ContextMenu")
        || wvk == 93
        || nvk == 93;
    if (shortcuts && is_cvx) || (is_insert && (shift || ctrl)) || (shift && is_del_or_f10) || is_menu {
        return Err("DevTools method 'Input.dispatchKeyEvent' cannot send clipboard or context-menu shortcuts.".into());
    }
    Ok(())
}
