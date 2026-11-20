//! Agent-specific engine handlers for `experimental.browser` v4 (`TypedCdpCall`, confined
//! `UploadFiles`, confined `DropFiles`).
//!
//! Security boundary:
//! - `TypedCdpCall` enforces `browser_policy::check_typed_cdp` and refuses extension documents.
//! - `UploadFiles` and `DropFiles` verify every path is a regular file strictly inside the
//!   marker-guarded `<profile>/opencode-page-uploads/<tab_id>/` staging directory (`<= 8` files,
//!   `<= 5 MiB` total, canonical symlink-free check) before invoking Chromium's `DOM.setFileInputFiles`
//!   or `Input.dispatchDragEvent`.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicI32, Ordering};

use cef::ImplBrowserHost;
use serde_json::{Value, json};

use crate::engine::ManagedTab;
use crate::handlers::CdpPending;
use crate::output::Output;
use crate::wire::EngineMsg;

pub const UPLOADS_DIR: &str = "opencode-page-uploads";
pub const UPLOADS_MARKER: &str = ".opencode-page-uploads";
const MAX_UPLOAD_FILES: usize = 8;
const MAX_UPLOAD_BYTES: u64 = 5 * 1024 * 1024;

pub fn remove_owned_uploads(profile_dir: &Path) {
    let root = profile_dir.join(UPLOADS_DIR);
    if root.join(UPLOADS_MARKER).is_file() {
        let _ = std::fs::remove_dir_all(root);
    }
}

pub fn remove_tab_uploads(profile_dir: &Path, tab_id: u64) {
    let root = profile_dir.join(UPLOADS_DIR);
    if root.join(UPLOADS_MARKER).is_file() {
        let _ = std::fs::remove_dir_all(root.join(tab_id.to_string()));
    }
}

pub fn handle_typed_cdp(
    t: &ManagedTab,
    tab_id: u64,
    request_id: u64,
    method: String,
    params: Value,
    session_id: Option<String>,
    next_cdp_id: &AtomicI32,
    out_tx: &Output,
) {
    let url = t.snap.lock().unwrap().url.clone();
    if browser_policy::extension_origin(&url) {
        send_err(
            out_tx,
            tab_id,
            request_id,
            "Agent browser tools cannot run DevTools methods on extension pages.",
        );
        return;
    }
    if let Err(err) = browser_policy::check_typed_cdp(&method, &params, true) {
        send_err(out_tx, tab_id, request_id, &err);
        return;
    }
    dispatch_cdp(t, tab_id, request_id, &method, params, session_id, next_cdp_id, out_tx);
}

pub fn handle_upload_files(
    t: &ManagedTab,
    profile_dir: &Path,
    tab_id: u64,
    request_id: u64,
    backend_node_id: i64,
    files: &[String],
    next_cdp_id: &AtomicI32,
    out_tx: &Output,
) {
    let url = t.snap.lock().unwrap().url.clone();
    if browser_policy::extension_origin(&url) {
        send_err(out_tx, tab_id, request_id, "Agent browser tools cannot upload files into extension pages.");
        return;
    }
    let verified = match verify_upload_paths(profile_dir, tab_id, files) {
        Ok(paths) => paths,
        Err(err) => {
            send_err(out_tx, tab_id, request_id, &err);
            return;
        }
    };
    dispatch_cdp(
        t,
        tab_id,
        request_id,
        "DOM.setFileInputFiles",
        json!({ "backendNodeId": backend_node_id, "files": verified }),
        None,
        next_cdp_id,
        out_tx,
    );
}

pub fn handle_drop_files(
    t: &ManagedTab,
    profile_dir: &Path,
    tab_id: u64,
    request_id: u64,
    x: f64,
    y: f64,
    files: &[String],
    next_cdp_id: &AtomicI32,
    out_tx: &Output,
) {
    let url = t.snap.lock().unwrap().url.clone();
    if browser_policy::extension_origin(&url) {
        send_err(out_tx, tab_id, request_id, "Agent browser tools cannot drop files onto extension pages.");
        return;
    }
    let verified = match verify_upload_paths(profile_dir, tab_id, files) {
        Ok(paths) => paths,
        Err(err) => {
            send_err(out_tx, tab_id, request_id, &err);
            return;
        }
    };
    let drag_data = json!({ "items": [], "files": verified, "dragOperationsMask": 1 });
    for kind in ["dragEnter", "dragOver"] {
        let msg_id = next_cdp_id.fetch_add(1, Ordering::Relaxed);
        let raw = json!({
            "id": msg_id,
            "method": "Input.dispatchDragEvent",
            "params": { "type": kind, "x": x, "y": y, "data": drag_data },
        })
        .to_string();
        let _ = t.host.send_dev_tools_message(Some(raw.as_bytes()));
    }
    dispatch_cdp(
        t,
        tab_id,
        request_id,
        "Input.dispatchDragEvent",
        json!({ "type": "drop", "x": x, "y": y, "data": drag_data }),
        None,
        next_cdp_id,
        out_tx,
    );
}

fn verify_upload_paths(profile_dir: &Path, tab_id: u64, files: &[String]) -> Result<Vec<String>, String> {
    if files.is_empty() || files.len() > MAX_UPLOAD_FILES {
        return Err(format!("Expected 1..={MAX_UPLOAD_FILES} staged upload files."));
    }
    let uploads_root = profile_dir.join(UPLOADS_DIR);
    if !uploads_root.join(UPLOADS_MARKER).is_file() {
        return Err("Upload staging directory is missing its ownership marker.".into());
    }
    let tab_dir = uploads_root.join(tab_id.to_string());
    let canonical_tab = std::fs::canonicalize(&tab_dir)
        .map_err(|_| "Tab upload staging directory does not exist.".to_owned())?;
    let mut total_bytes = 0u64;
    let mut out = Vec::with_capacity(files.len());
    for raw in files {
        let p = PathBuf::from(raw);
        let meta = std::fs::symlink_metadata(&p)
            .map_err(|_| "Staged upload file does not exist.".to_owned())?;
        if meta.file_type().is_symlink() || !meta.is_file() {
            return Err("Staged upload path must be a regular file, not a symlink or directory.".into());
        }
        let canon = std::fs::canonicalize(&p)
            .map_err(|_| "Could not canonicalize staged upload path.".to_owned())?;
        if !canon.starts_with(&canonical_tab) {
            return Err("Upload file path escapes the tab's confined staging directory.".into());
        }
        total_bytes = total_bytes.saturating_add(meta.len());
        if total_bytes > MAX_UPLOAD_BYTES {
            return Err("Staged upload files exceed the 5 MiB total transfer limit.".into());
        }
        out.push(canon.to_string_lossy().into_owned());
    }
    Ok(out)
}

#[allow(clippy::too_many_arguments)]
fn dispatch_cdp(
    t: &ManagedTab,
    tab_id: u64,
    request_id: u64,
    method: &str,
    params: Value,
    session_id: Option<String>,
    next_cdp_id: &AtomicI32,
    out_tx: &Output,
) {
    let msg_id = next_cdp_id.fetch_add(1, Ordering::Relaxed);
    t.cdp_id_map.lock().unwrap().insert(msg_id, CdpPending::Raw(request_id));
    let mut payload = json!({
        "id": msg_id,
        "method": method,
        "params": params,
    });
    if let Some(sid) = session_id {
        payload["sessionId"] = Value::String(sid);
    }
    let raw = payload.to_string();
    if t.host.send_dev_tools_message(Some(raw.as_bytes())) != 1 {
        t.cdp_id_map.lock().unwrap().remove(&msg_id);
        send_err(out_tx, tab_id, request_id, "send_dev_tools_message rejected");
    }
}

fn send_err(out_tx: &Output, tab_id: u64, request_id: u64, msg: &str) {
    let _ = out_tx.send(EngineMsg::CdpResponse {
        tab_id,
        request_id,
        ok: false,
        result: json!({ "message": msg }),
    });
}
