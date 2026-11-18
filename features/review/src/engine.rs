//! Non-generic changes engine entrypoints operating over `&mut dyn ReviewHost`: polling in-flight
//! diff/file fetches, debounced event invalidation, mode/style/expand switches, and job execution.

use opencode_code_view::data::review::{
    FetchedDiffs, ReviewFetchSpec, collapsed_folders, fetch_live_diffs, fetch_live_file_text, load_fixture_diffs,
};
use opencode_code_view::{ExpandMode, FoldEvent};
use opencode_kit::client::Server;
use opencode_kit::wire_error::WireKind;
use rune::Tx;
use rune::time::Instant;
use rune_kit::diff::{DiffMode, Side};
use serde_json::Value;

use crate::focus::{Anchor, focus_change};
use crate::folders::{self, expand_folder};
use crate::mobile::NarrowReview;
use crate::{ChangeMode, Load, Refresh, RefreshEffect, RefreshEvent, ReviewHost, ReviewJob, ReviewReply};

/// Executes a [`ReviewJob`] off the UI thread against the live [`Server`].
pub fn execute_job(server: &Server, job: ReviewJob) -> ReviewReply {
    match job {
        ReviewJob::ReadFolder { session_id, path, mode, generation, url } => {
            let result = server.get(&url).map_err(|e| e.to_string());
            ReviewReply::FolderLoaded { session_id, path, mode, generation, result }
        }
    }
}

/// Applies a completed [`ReviewReply`] back onto `host` on the UI thread.
pub fn apply_reply(host: &mut dyn ReviewHost, reply: ReviewReply, tx: &mut Tx) {
    match reply {
        ReviewReply::FolderLoaded { session_id, path, mode, generation, result } => {
            folders::apply_loaded(host, &session_id, &path, mode, generation, result, tx);
        }
    }
}

/// Lands the reads in flight; reads again after file events while the changes show.
pub fn poll(host: &mut dyn ReviewHost, tx: &mut Tx) -> bool {
    let mut changed = false;
    if let Some(fetched) = host.review().diff_fetch.take() {
        tx.instant();
        let sid = fetched.session_id.clone();
        if let Some((path, anchor)) = apply_fetched_diffs(host, fetched) {
            focus_change(host, &sid, &path, anchor.as_ref(), tx);
        }
        changed = true;
    }
    if let Some((session_id, path, content)) = host.review().file_fetch.take() {
        let updated = if let Some(sr) = host.review_mut().sessions.get_mut(&session_id) {
            tx.instant();
            // The ranges waiting on this file's text open as asked, or fold again without it.
            match content {
                Some(text) => {
                    sr.diffs.attach_file_text(&path, &text);
                }
                None => sr.diffs.text_failed(&path),
            }
            sr.stamp += 1;
            true
        } else {
            false
        };
        if updated {
            host.on_review_changed(&session_id);
            changed = true;
        }
    }
    let now = Instant::now();
    let effect = match host.review().refresh {
        Refresh::Idle => RefreshEffect::None,
        Refresh::Waiting(due) if now < due => RefreshEffect::WakeAfter(due - now),
        Refresh::Due | Refresh::Waiting(_) => host.review_mut().refresh.step(RefreshEvent::Poll(now)),
    };
    match effect {
        RefreshEffect::WakeAfter(delay) => tx.wake_after(delay),
        RefreshEffect::Run => {
            if let Some(sid) = host.active_session_id().map(str::to_owned)
                && host.should_refresh_on_poll()
            {
                let live = host.is_live();
                ensure_session(host, &sid);
                if !live {
                    changed = true;
                }
            }
        }
        RefreshEffect::None | RefreshEffect::WakeNow => {}
    }
    changed
}

/// Opens the review tab, and the side region with it (Desktop's `show`): added to the strip
/// when it isn't there, selected, focused, and its changes read.
pub fn show(host: &mut dyn ReviewHost, sid: &str, tx: &mut Tx) {
    host.show_review_tab(sid, true, tx);
    ensure_session(host, sid);
}

/// Records an incoming server event that may invalidate session diffs and schedules a debounced
/// or immediate refresh when the changes surface is visible.
pub fn note_event(host: &mut dyn ReviewHost, ty: &str, data: &Value) -> bool {
    if !matches!(
        ty,
        "filesystem.changed"
            | "session.execution.succeeded"
            | "session.step.ended"
            | "session.revert.committed"
    ) {
        return false;
    }
    host.review_mut().revision += 1;
    if !host.review_shown() {
        return false;
    }
    if ty == "filesystem.changed" && host.is_live() {
        if let Some(event_dir) = data["eventLocation"]["directory"].as_str()
            && let Some(cur_dir) = host.active_session_id().and_then(|sid| host.session_directory(sid))
            && !cur_dir.is_empty()
            && !same_directory(event_dir, cur_dir)
        {
            return false;
        }
        host.review_mut().refresh.step(RefreshEvent::Debounced(Instant::now()));
        return false;
    }
    host.review_mut().refresh.step(RefreshEvent::Immediate) == RefreshEffect::WakeNow
}

/// Ensures session `session_id`'s diffs are loaded for its current mode and revision.
pub fn ensure_session(host: &mut dyn ReviewHost, session_id: &str) {
    let directory = host.session_directory(session_id).unwrap_or_default().to_owned();
    let wrap = host.wrap_lines();
    let live_server = host.live_server().cloned();
    let (diff_mode, expand_mode, rev, mode, already_loaded) = {
        let review = host.review_mut();
        review.refresh.step(RefreshEvent::Clear);
        let diff_mode = review.diff_mode;
        let expand_mode = review.expand_mode;
        let rev = review.revision;
        let sr = review.sessions.entry(session_id.to_owned()).or_default();
        let mode = sr.mode;
        let already_loaded = sr.load.loaded_revision() == Some((mode, rev));
        if already_loaded {
            sr.diffs.set_mode(diff_mode);
            sr.diffs.set_expand_mode(expand_mode);
        }
        (diff_mode, expand_mode, rev, mode, already_loaded)
    };
    if already_loaded {
        return;
    }

    let Some(srv) = live_server else {
        let empty = Value::Null;
        let fixture = host.fixture_root().unwrap_or(&empty);
        let (files, meta, listed, cur_b, def_b, texts) = load_fixture_diffs(fixture, session_id, mode);
        {
            let sr = host.review_mut().sessions.entry(session_id.to_owned()).or_default();
            sr.branch_current = cur_b;
            sr.branch_default = def_b;
            sr.root_count = meta.len();
            sr.land(files, meta, listed, diff_mode, expand_mode, wrap);
            for (path, text) in texts {
                sr.diffs.attach_file_text(&path, &text);
            }
            let _ = sr.load.landed(mode, rev, false);
        }
        host.on_review_changed(session_id);
        return;
    };

    let ask = (session_id.to_owned(), mode, rev);
    if host.review().asked.as_ref() == Some(&ask) {
        return;
    }
    let spec = {
        let review = host.review_mut();
        review.asked = Some(ask);
        let (collapsed, active_path, find_query, narrow_open) = review
            .sessions
            .get(session_id)
            .map(|sr| {
                (
                    collapsed_folders(&sr.tree),
                    sr.active_path.clone(),
                    sr.diffs.find_query(),
                    sr.narrow.open_paths(),
                )
            })
            .unwrap_or_default();
        ReviewFetchSpec {
            session_id: session_id.to_owned(),
            directory,
            mode,
            revision: rev,
            diff_mode,
            expand_mode,
            collapsed,
            active_path,
            find_query,
            narrow_open,
        }
    };
    host.trace_review_asked(session_id, rev);
    host.review_mut()
        .diff_fetch
        .request(spec, move |spec| fetch_live_diffs(&srv, spec));
}

/// Forces a fresh read for session `session_id` (e.g. pressing Retry).
pub fn refresh(host: &mut dyn ReviewHost, session_id: &str) {
    host.review_mut().revision += 1;
    ensure_session(host, session_id);
}

/// Lands a read; returns the change a step asked to show once it did.
fn apply_fetched_diffs(host: &mut dyn ReviewHost, fetched: FetchedDiffs) -> Option<(String, Option<Anchor>)> {
    let swap = Instant::now();
    let diff_mode = host.review().diff_mode;
    let expand_mode = host.review().expand_mode;
    let wrap = host.wrap_lines();
    let prepared = match fetched.changes {
        Ok(prepared) => prepared,
        Err(e) => {
            let what = opencode_i18n::t!("rune.review.readFailed").to_owned();
            let offline = matches!(e.kind, WireKind::Unreachable(_) | WireKind::TimedOut);
            let why = host.format_wire_error(&what, &e);
            let changed = {
                let sr = host.review_mut().sessions.entry(fetched.session_id.clone()).or_default();
                if sr.mode == fetched.mode {
                    sr.load.failed_with(fetched.mode, fetched.revision, why, offline);
                    sr.stamp += 1;
                    true
                } else {
                    false
                }
            };
            if changed {
                host.on_review_changed(&fetched.session_id);
            }
            return None;
        }
    };
    let (reopen, file_count, focus) = {
        let sr = host.review_mut().sessions.entry(fetched.session_id.clone()).or_default();
        if sr.mode != fetched.mode {
            opencode_code_view::prepared::drop_off_ui(prepared);
            return None;
        }
        sr.branch_current = fetched.branch_current;
        sr.branch_default = fetched.branch_default;
        let reopen: Vec<_> = prepared.folders.iter().filter(|p| sr.tree.is_expanded(p)).cloned().collect();
        sr.generation += 1;
        let mut next_diffs = prepared.diffs.into_state();
        next_diffs.set_viewport([0.0, 0.0, sr.diffs.view_width(), 0.0]);
        next_diffs.carry_find_from(&sr.diffs);
        next_diffs.set_mode(diff_mode);
        next_diffs.set_expand_mode(expand_mode);
        next_diffs.keep_keys_rev(&sr.diffs);
        let old_folders = std::mem::replace(
            &mut sr.folders,
            prepared.folders.into_iter().map(|p| (p, folders::Listing::default())).collect(),
        );
        sr.root_count = prepared.count;
        let old_meta = std::mem::replace(&mut sr.meta, prepared.meta);
        let old_listed = std::mem::replace(&mut sr.listed, prepared.listed);
        let old_tree = std::mem::replace(&mut sr.tree, prepared.tree);
        let old_diffs = std::mem::replace(&mut sr.diffs, next_diffs);
        let mut next_narrow = NarrowReview::from_prepared(prepared.narrow);
        next_narrow.set_wrap(wrap);
        let old_narrow = std::mem::replace(&mut sr.narrow, next_narrow);
        sr.active_path = prepared.active_path;
        sr.stamp += 1;
        let file_count = sr.diffs.files().len();
        if !old_diffs.files().is_empty() || !old_meta.is_empty() {
            let old_diffs = old_diffs.into_prepared();
            let old_narrow = old_narrow.into_prepared();
            opencode_code_view::prepared::drop_off_ui((old_diffs, old_meta, old_listed, old_tree, old_folders, old_narrow));
        }
        let focus = sr.load.landed(fetched.mode, fetched.revision, fetched.no_git);
        (reopen, file_count, focus)
    };
    for folder in reopen {
        expand_folder(host, &fetched.session_id, &folder, true);
    }
    host.trace_review_landed(&fetched.session_id, fetched.revision, swap, file_count);
    host.on_review_changed(&fetched.session_id);
    focus
}

/// Compares `mode` in the routed session's review.
pub fn set_mode(host: &mut dyn ReviewHost, mode: ChangeMode) {
    let Some(sid) = host.active_session_id().map(str::to_owned) else { return };
    set_session_mode(host, &sid, mode);
}

/// Switches session `sid`'s change source (`git`, `branch`, or `turn`) and ensures its diffs load.
pub fn set_session_mode(host: &mut dyn ReviewHost, sid: &str, mode: ChangeMode) {
    let changed = {
        let review = host.review_mut();
        let sr = review.sessions.entry(sid.to_owned()).or_default();
        if sr.mode != mode {
            sr.mode = mode;
            sr.load.restart();
            sr.stamp += 1;
            review.asked = None;
            true
        } else {
            false
        }
    };
    if changed {
        host.on_review_changed(sid);
    }
    ensure_session(host, sid);
}

/// Sets the diff layout style (`DiffMode::Unified` or `DiffMode::Split`) across every session.
pub fn set_diff_mode(host: &mut dyn ReviewHost, mode: DiffMode) {
    if host.review().diff_mode == mode {
        return;
    }
    let review = host.review_mut();
    review.diff_mode = mode;
    for sr in review.sessions.values_mut() {
        sr.diffs.set_mode(mode);
        sr.stamp += 1;
    }
}

/// Steps the unchanged-context expand mode across every session, requesting full file text when
/// unfolding if needed.
pub fn set_expand_mode(host: &mut dyn ReviewHost, mode: ExpandMode) {
    let (next, event) = host.review().expand_mode.step(mode);
    let Some(event) = event else { return };
    host.review_mut().expand_mode = next;
    if event == FoldEvent::Unfold
        && let Some(sid) = host.active_session_id().map(str::to_owned)
    {
        request_missing_file_texts(host, &sid);
    }
    for sr in host.review_mut().sessions.values_mut() {
        sr.diffs.set_expand_mode(next);
        sr.stamp += 1;
    }
}

fn request_missing_file_texts(host: &mut dyn ReviewHost, session_id: &str) {
    let Some(srv) = host.live_server().cloned() else { return };
    let directory = host.session_directory(session_id).unwrap_or_default().to_owned();
    let missing = {
        let Some(sr) = host.review().sessions.get(session_id) else { return };
        let shown = sr
            .diffs
            .only()
            .map_or_else(|| (0..sr.diffs.files().len()).collect::<Vec<_>>(), |f| vec![f]);
        shown
            .into_iter()
            .find(|&f| !sr.diffs.has_full_text(f))
            .map(|f| sr.diffs.files()[f].path().to_owned())
    };
    if let Some(path) = missing {
        let sid = session_id.to_owned();
        host.review_mut()
            .file_fetch
            .request((sid, directory, path), move |(sid, dir, path)| {
                let content = fetch_live_file_text(&srv, &dir, &path);
                (sid, path, content)
            });
    }
}

/// Loads the whole text of file `f` of the active session's review, which a fold waits on
/// (`FoldEffect::FetchText`); without a server to ask, the fold gives up at once.
pub fn request_file_text(host: &mut dyn ReviewHost, f: usize) {
    let Some(sid) = host.active_session_id().map(str::to_owned) else { return };
    let Some(path) = host
        .review()
        .sessions
        .get(&sid)
        .and_then(|sr| sr.diffs.files().get(f))
        .map(|fd| fd.path().to_owned())
    else {
        return;
    };
    let Some(srv) = host.live_server().cloned() else {
        if let Some(sr) = host.review_mut().sessions.get_mut(&sid) {
            sr.diffs.text_failed(&path);
        }
        return;
    };
    let directory = host.session_directory(&sid).unwrap_or_default().to_owned();
    host.review_mut()
        .file_fetch
        .request((sid, directory, path), move |(sid, dir, path)| {
            let content = fetch_live_file_text(&srv, &dir, &path);
            (sid, path, content)
        });
}

/// Opens the review of `sid` (the MCP `open_diff` tool, a tool step's change), in `mode` and
/// `view` when given, at `path` near `line`, preserving existing composer/side focus.
pub fn open_for_session(
    host: &mut dyn ReviewHost,
    sid: &str,
    mode: Option<ChangeMode>,
    view: Option<DiffMode>,
    path: Option<&str>,
    line: Option<u32>,
    tx: &mut Tx,
) {
    if let Some(v) = view {
        set_diff_mode(host, v);
    }
    if let Some(m) = mode {
        let review = host.review_mut();
        let sr = review.sessions.entry(sid.to_owned()).or_default();
        if sr.mode != m {
            sr.mode = m;
            sr.load.restart();
            sr.stamp += 1;
            review.asked = None;
        }
    }
    host.show_review_tab(sid, false, tx);
    ensure_session(host, sid);
    if let Some(p) = path {
        let anchor = line.map(|n| Anchor { side: Side::New, text: String::new(), near: Some(n.max(1)) });
        if let Some(Load::Loading { focus }) = host.review_mut().sessions.get_mut(sid).map(|sr| &mut sr.load) {
            *focus = Some((p.to_owned(), anchor));
        } else {
            focus_change(host, sid, p, anchor.as_ref(), tx);
        }
    }
}

/// Applies `host.wrap_lines()` to every open narrow diff, mutating `review_mut()` only when one differs.
pub fn sync_narrow_wrap(host: &mut dyn ReviewHost) {
    let wrap = host.wrap_lines();
    if host.review().sessions.values().any(|sr| sr.narrow.wraps_otherwise(wrap)) {
        for sr in host.review_mut().sessions.values_mut() {
            sr.narrow.set_wrap(wrap);
        }
    }
}

fn same_directory(a: &str, b: &str) -> bool {
    opencode_kit::path::contains(a, b) && opencode_kit::path::contains(b, a)
}
