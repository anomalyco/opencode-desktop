//! Privacy-safe structured page diagnostics for renderer crashes and load failures (`UP-110`
//! `#65096`, `packages/gui-plugins/src/browser/page-log.ts`).
//!
//! Never logs URL paths, queries, fragments, credentials, page titles, or page text — only the
//! normalized origin (or non-web scheme category), termination status, exit code (plus hex
//! NTSTATUS on Windows), open duration, loading state, and `agent_driven` flag.

use std::time::Instant;

use cef::{TerminationStatus, sys};
use serde_json::json;

use crate::output::Output;
use crate::wire::EngineMsg;

/// Per-tab timing anchor for structured diagnostics.
pub struct TabDiagnostics {
    opened_at: Instant,
}

impl Default for TabDiagnostics {
    fn default() -> Self {
        Self { opened_at: Instant::now() }
    }
}

impl TabDiagnostics {
    pub fn open_ms(&self) -> u64 {
        u64::try_from(self.opened_at.elapsed().as_millis()).unwrap_or(u64::MAX)
    }

    pub fn report_renderer_terminated(
        &self,
        tab_id: u64,
        status: TerminationStatus,
        error_code: i32,
        error_string: &str,
        url: &str,
        loading: bool,
        agent_driven: bool,
        out_tx: &Output,
    ) {
        let reason = termination_reason(status);
        let origin = log_origin(url);
        let open_ms = self.open_ms();
        let error_label = sanitize_error_token(error_string);
        let mut fields = json!({
            "cause": "renderer-gone",
            "reason": reason,
            "exitCode": error_code,
            "origin": origin,
            "agentDriven": agent_driven,
            "openMs": open_ms,
            "loading": loading,
        });
        if cfg!(windows) {
            fields["exitCodeHex"] = json!(exit_code_hex(error_code));
        }
        if let Some(tok) = error_label {
            fields["error"] = json!(tok);
        }
        let _ = out_tx.send(EngineMsg::PageDiagnostic {
            tab_id,
            level: "error".into(),
            event: "browser page ended".into(),
            fields,
        });
    }

    pub fn report_load_failed(
        &self,
        tab_id: u64,
        code: i32,
        error_text: &str,
        failed_url: &str,
        loading: bool,
        agent_driven: bool,
        out_tx: &Output,
    ) {
        let origin = log_origin(failed_url);
        let open_ms = self.open_ms();
        let mut fields = json!({
            "code": code,
            "origin": origin,
            "agentDriven": agent_driven,
            "openMs": open_ms,
            "loading": loading,
        });
        if let Some(tok) = sanitize_error_token(error_text) {
            fields["error"] = json!(tok);
        }
        let _ = out_tx.send(EngineMsg::PageDiagnostic {
            tab_id,
            level: "warn".into(),
            event: "browser page load failed".into(),
            fields,
        });
    }
}

/// Maps a CEF `TerminationStatus` to the upstream `page-log.ts` reason vocabulary.
pub fn termination_reason(status: TerminationStatus) -> &'static str {
    use sys::cef_termination_status_t as T;
    match *status.as_ref() {
        T::TS_ABNORMAL_TERMINATION => "abnormal-exit",
        T::TS_PROCESS_WAS_KILLED => "killed",
        T::TS_PROCESS_CRASHED => "crashed",
        T::TS_PROCESS_OOM => "oom",
        T::TS_LAUNCH_FAILED => "launch-failed",
        T::TS_INTEGRITY_FAILURE => "integrity-failure",
        _ => "unknown",
    }
}

/// Windows NTSTATUS / exit code formatted as `0xC0000005` (`page-log.ts:99`).
pub fn exit_code_hex(code: i32) -> String {
    format!("0x{:08X}", code as u32)
}

/// Only accepts bounded Chromium-style uppercase error token names (never free-form messages/paths).
fn sanitize_error_token(raw: &str) -> Option<&str> {
    let s = raw.trim();
    if !s.is_empty()
        && s.len() <= 64
        && s.bytes().all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'_')
    {
        Some(s)
    } else {
        None
    }
}

/// Privacy-safe origin classifier matching upstream `page-log.ts:58-96`.
/// Never preserves credentials, path, query, or fragment.
pub fn log_origin(address: &str) -> String {
    let trimmed = address.trim();
    if trimmed.is_empty() {
        return "none".into();
    }
    if let Some(orig) = browser_policy::destination_origin(trimmed) {
        return orig;
    }
    let lower = trimmed.to_ascii_lowercase();
    if let Some(rest) = lower.strip_prefix("blob:") {
        let inner = log_origin(rest);
        if inner.starts_with("http://") || inner.starts_with("https://") {
            return format!("blob:{inner}");
        }
        return "blob".into();
    }
    if lower.starts_with("file:") {
        return "file".into();
    }
    if lower.starts_with("oc-preview:") || lower.starts_with("workspace:") {
        return "preview".into();
    }
    if lower.starts_with("artifact:") {
        return "artifact".into();
    }
    if lower.starts_with("about:") {
        return "about".into();
    }
    if lower.starts_with("data:") {
        return "data".into();
    }
    if lower.starts_with("chrome-extension:") {
        return "extension".into();
    }
    if lower.starts_with("chrome-error:") {
        return "error-page".into();
    }
    if lower.starts_with("http://") || lower.starts_with("https://") {
        // Credentialed or malformed HTTP(S) URL: never log the authority.
        return "invalid".into();
    }
    "other".into()
}
