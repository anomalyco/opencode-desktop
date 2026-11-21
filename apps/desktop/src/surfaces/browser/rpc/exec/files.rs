//! Confined `files.upload` and `files.drop` executors for `experimental.browser` v4.
//!
//! Security boundary:
//! - Accepts ONLY server-supplied `command.files` bytes (`<= 8` files, `<= 5 MiB` total). Never
//!   reads arbitrary host filesystem paths named in tool arguments.
//! - Writes staged bytes strictly under `<profile>/opencode-page-uploads/<host_tab_id>/<file_id>/`
//!   with an ownership marker (`.opencode-page-uploads`) and Windows reserved-name sanitization.
//! - Engine re-verifies every path canonically before calling `DOM.setFileInputFiles` or
//!   `Input.dispatchDragEvent`.

use std::path::{Path, PathBuf};

use opencode_kit::browser_rpc::{Command, MAX_FILE_BYTES, Outcome, WireFile, failure_code, new_file_id};
use serde_json::{Value, json};

use super::super::session::SessionAttachment;
use super::input::quad_center;
use super::{ExecPhase, StepOutcome, cdp_err_msg, check_page_guards};
use crate::App;

const UPLOADS_DIR: &str = "opencode-page-uploads";
const UPLOADS_MARKER: &str = ".opencode-page-uploads";
const MAX_UPLOAD_FILES: usize = 8;

pub(super) struct UploadState {
    pub host_id: u64,
    pub stage: UploadStage,
}

pub(super) enum UploadStage {
    WaitUpload { cdp_req: u64 },
    DropScroll { bid: i64, staged: Vec<String>, cdp_req: u64 },
    DropCenter { staged: Vec<String>, cdp_req: u64 },
    WaitDrop { cdp_req: u64 },
}

impl UploadStage {
    pub fn cdp_req(&self) -> u64 {
        match self {
            Self::WaitUpload { cdp_req } | Self::DropScroll { cdp_req, .. } | Self::DropCenter { cdp_req, .. } | Self::WaitDrop { cdp_req } => *cdp_req,
        }
    }
}

pub(super) fn try_start(
    att: &mut SessionAttachment,
    app: &mut App,
    cmd: &Command,
) -> Option<StepOutcome> {
    match cmd.action_type()? {
        "files.upload" => Some(start_upload_or_drop(att, app, cmd, false)),
        "files.drop" => Some(start_upload_or_drop(att, app, cmd, true)),
        _ => None,
    }
}

fn start_upload_or_drop(
    att: &mut SessionAttachment,
    app: &mut App,
    cmd: &Command,
    is_drop: bool,
) -> StepOutcome {
    let action_name = if is_drop { "files.drop" } else { "files.upload" };
    let Some(wire_id) = cmd.tab_id() else {
        return StepOutcome::Done(Outcome::fail(failure_code::OPERATION_FAILED, "Missing tabID."));
    };
    let host_id = match att.resolve_tab(app, wire_id) {
        Ok(id) => id,
        Err(msg) => return StepOutcome::Done(Outcome::fail(failure_code::OPERATION_FAILED, msg)),
    };
    if let Err(o) = check_page_guards(app, host_id, action_name) {
        return StepOutcome::Done(o);
    }
    let Some(raw_ref) = cmd.action.get("ref").and_then(Value::as_str) else {
        return StepOutcome::Done(Outcome::fail(failure_code::OPERATION_FAILED, "Missing ref."));
    };
    let Some((bid, ref_session)) = att.refs.get(&host_id).and_then(|r| r.resolve(raw_ref)) else {
        return StepOutcome::Done(Outcome::fail(
            failure_code::STALE_REF,
            format!("Element ref {raw_ref} is stale or absent. Refresh the snapshot first."),
        ));
    };
    if ref_session.is_some() {
        return StepOutcome::Done(Outcome::fail(
            failure_code::OPERATION_FAILED,
            "Uploading or dropping files into a cross-origin frame is not supported yet.",
        ));
    }
    if cmd.files.is_empty() {
        return StepOutcome::Done(Outcome::fail(
            failure_code::OPERATION_FAILED,
            "Upload command has no server-transferred file bytes. Supply server-local paths to browser.files.upload/drop.",
        ));
    }
    if !app.workspace_panes.browser.host.engine_agent_files() {
        return StepOutcome::Done(Outcome::fail(
            failure_code::UNSUPPORTED,
            "File uploads and drops require an updated browser engine.",
        ));
    }
    let Some(profile_dir) = app.workspace_panes.browser.host.uploads_profile_dir() else {
        return StepOutcome::Done(Outcome::fail(
            failure_code::OPERATION_FAILED,
            "Browser profile directory is unavailable for confined upload staging.",
        ));
    };
    let staged = match stage_files(&profile_dir, host_id, &cmd.files) {
        Ok(paths) => paths,
        Err(msg) => return StepOutcome::Done(Outcome::fail(failure_code::OPERATION_FAILED, msg)),
    };

    app.workspace_panes.browser.host.set_agent_driven(host_id, true);
    if !is_drop {
        match app.workspace_panes.browser.host.upload_files(host_id, bid, staged) {
            Ok(cdp_req) => StepOutcome::Pending(ExecPhase::Upload(UploadState {
                host_id,
                stage: UploadStage::WaitUpload { cdp_req },
            })),
            Err(e) => StepOutcome::Done(Outcome::fail(failure_code::OPERATION_FAILED, e)),
        }
    } else {
        match app.workspace_panes.browser.host.send_typed_cdp(
            host_id,
            "DOM.scrollIntoViewIfNeeded",
            json!({ "backendNodeId": bid }),
            None,
        ) {
            Ok(cdp_req) => StepOutcome::Pending(ExecPhase::Upload(UploadState {
                host_id,
                stage: UploadStage::DropScroll { bid, staged, cdp_req },
            })),
            Err(e) => StepOutcome::Done(Outcome::fail(failure_code::OPERATION_FAILED, e)),
        }
    }
}

pub(super) fn step_upload(
    att: &SessionAttachment,
    app: &mut App,
    st: &mut UploadState,
) -> Option<Outcome> {
    let stage = std::mem::replace(&mut st.stage, UploadStage::WaitUpload { cdp_req: 0 });
    match stage {
        UploadStage::WaitUpload { cdp_req } | UploadStage::WaitDrop { cdp_req } => {
            let Some(resp) = app.workspace_panes.browser.host.take_cdp_response(cdp_req) else {
                st.stage = UploadStage::WaitUpload { cdp_req };
                return None;
            };
            match resp {
                Ok(_) => {
                    let tab = att.wire_tab(app, st.host_id)?;
                    Some(Outcome::ok(serde_json::to_value(tab).unwrap_or(Value::Null), Vec::new()))
                }
                Err(e) => Some(Outcome::fail(failure_code::OPERATION_FAILED, cdp_err_msg(&e))),
            }
        }
        UploadStage::DropScroll { bid, staged, cdp_req } => {
            let Some(resp) = app.workspace_panes.browser.host.take_cdp_response(cdp_req) else {
                st.stage = UploadStage::DropScroll { bid, staged, cdp_req };
                return None;
            };
            if let Err(e) = resp {
                return Some(Outcome::fail(failure_code::STALE_REF, cdp_err_msg(&e)));
            }
            match app.workspace_panes.browser.host.send_typed_cdp(
                st.host_id,
                "DOM.getContentQuads",
                json!({ "backendNodeId": bid }),
                None,
            ) {
                Ok(next_req) => {
                    st.stage = UploadStage::DropCenter { staged, cdp_req: next_req };
                    None
                }
                Err(e) => Some(Outcome::fail(failure_code::OPERATION_FAILED, e)),
            }
        }
        UploadStage::DropCenter { staged, cdp_req } => {
            let Some(resp) = app.workspace_panes.browser.host.take_cdp_response(cdp_req) else {
                st.stage = UploadStage::DropCenter { staged, cdp_req };
                return None;
            };
            let val = match resp {
                Ok(v) => v,
                Err(e) => return Some(Outcome::fail(failure_code::STALE_REF, cdp_err_msg(&e))),
            };
            let Some((x, y)) = quad_center(&val) else {
                return Some(Outcome::fail(
                    failure_code::STALE_REF,
                    "Drop target has no visible layout box. Refresh the snapshot first.",
                ));
            };
            match app.workspace_panes.browser.host.drop_files(st.host_id, x, y, staged) {
                Ok(next_req) => {
                    st.stage = UploadStage::WaitDrop { cdp_req: next_req };
                    None
                }
                Err(e) => Some(Outcome::fail(failure_code::OPERATION_FAILED, e)),
            }
        }
    }
}

/// Removes `<profile>/opencode-page-uploads/<host_id>` only when the ownership marker is present.
pub(in super::super) fn cleanup_tab_uploads(app: &App, host_id: u64) {
    let Some(profile_dir) = app.workspace_panes.browser.host.uploads_profile_dir() else { return };
    let root = profile_dir.join(UPLOADS_DIR);
    if !root.join(UPLOADS_MARKER).is_file() {
        return;
    }
    let _ = std::fs::remove_dir_all(root.join(host_id.to_string()));
}

fn stage_files(profile_dir: &Path, host_id: u64, files: &[WireFile]) -> Result<Vec<String>, String> {
    if files.is_empty() || files.len() > MAX_UPLOAD_FILES {
        return Err(format!("Expected 1..={MAX_UPLOAD_FILES} upload files."));
    }
    let root = profile_dir.join(UPLOADS_DIR);
    if let Ok(meta) = std::fs::symlink_metadata(&root)
        && (meta.file_type().is_symlink() || !root.join(UPLOADS_MARKER).is_file())
    {
        return Err("Refusing to write into an unmarked or symlinked opencode-page-uploads directory.".into());
    }
    std::fs::create_dir_all(&root).map_err(|e| format!("Could not create upload staging dir: {e}"))?;
    std::fs::write(root.join(UPLOADS_MARKER), b"")
        .map_err(|e| format!("Could not mark upload staging dir: {e}"))?;

    let mut total_bytes = 0usize;
    let mut staged = Vec::with_capacity(files.len());
    for file in files {
        let bytes = file.decode_bytes()?;
        total_bytes = total_bytes.saturating_add(bytes.len());
        if total_bytes > MAX_FILE_BYTES {
            return Err("Combined upload files exceed the 5 MiB transfer limit.".into());
        }
        let slot = new_file_id();
        let dir = root.join(host_id.to_string()).join(&slot);
        std::fs::create_dir_all(&dir).map_err(|e| format!("Could not create upload slot dir: {e}"))?;
        let dest: PathBuf = dir.join(sanitize_filename(&file.name));
        std::fs::write(&dest, &bytes).map_err(|e| format!("Could not write staged upload file: {e}"))?;
        staged.push(dest.to_string_lossy().into_owned());
    }
    Ok(staged)
}

fn sanitize_filename(raw: &str) -> String {
    let leaf = raw.rsplit(['/', '\\']).next().unwrap_or("upload");
    let cleaned: String = leaf
        .chars()
        .filter(|c| !c.is_control() && !matches!(c, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*'))
        .collect();
    let trimmed = cleaned.trim().trim_end_matches(['.', ' ']);
    let base = if trimmed.is_empty() || trimmed == "." || trimmed == ".." {
        "upload".to_owned()
    } else {
        trimmed.to_owned()
    };
    let stem = base.split('.').next().unwrap_or(&base);
    if browser_pane::policy::is_windows_device_name(stem) {
        format!("_{base}")
    } else {
        base
    }
}
