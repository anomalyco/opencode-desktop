//! Asks the server to watch each open file, as Desktop's file model does
//! (`packages/app/src/workspaces/files/model.tsx`, `watch` and `unwatch`): when a file tab opens
//! and each time one shows (the server keeps a watch 24 hours from the latest ask), for every
//! open file tab when the stream connects or comes back (a restarted server forgets its
//! watches), and no more when the last tab holding it closes or its session is dropped. The
//! server then publishes `filesystem.changed` for it. A server keeps at most 256 watches a
//! folder and drops the one that would expire first for a new one: that file hears nothing
//! more until its tab shows again. Errors are ignored, as Desktop's are (an older server has no
//! such route). Live servers only; a path outside the session's folder isn't watched.

use std::collections::BTreeSet;

use serde_json::json;

use crate::{App, Source};

/// `(folder, path)`: the sessions whose open tabs hold its watch.
pub(crate) type Watches = std::collections::HashMap<(String, String), BTreeSet<String>>;

/// `sid`'s tab for `path` opened or showed: (re)arm its watch.
pub(crate) fn watch(s: &mut App, sid: &str, path: &str) {
    let Some(directory) = directory(s, sid, path) else { return };
    s.files.watches.entry((directory.clone(), path.to_owned())).or_default().insert(sid.to_owned());
    send(s, "POST", &directory, path);
}

/// `sid`'s tab for `path` closed: the watch stops once no session's tab holds it.
pub(crate) fn unwatch(s: &mut App, sid: &str, path: &str) {
    let Some(directory) = directory(s, sid, path) else { return };
    let key = (directory.clone(), path.to_owned());
    let Some(holders) = s.files.watches.get_mut(&key) else { return };
    holders.remove(sid);
    if holders.is_empty() {
        s.files.watches.remove(&key);
        send(s, "DELETE", &directory, path);
    }
}

/// `sid`'s open files were dropped (its session closed): their watches stop.
pub(crate) fn forget_session(s: &mut App, sid: &str) {
    let held: Vec<(String, String)> = s.files.watches.iter().filter(|(_, sids)| sids.contains(sid)).map(|(k, _)| k.clone()).collect();
    for (directory, path) in held {
        let key = (directory.clone(), path.clone());
        if let Some(holders) = s.files.watches.get_mut(&key) {
            holders.remove(sid);
            if holders.is_empty() {
                s.files.watches.remove(&key);
                send(s, "DELETE", &directory, &path);
            }
        }
    }
}

/// The stream connected or came back: every open file tab's watch again, the shown one first.
pub(crate) fn connected(s: &mut App) {
    for (sid, path) in open_tabs(s) {
        watch(s, &sid, &path);
    }
}

/// Every session's open file tabs, the shown one first.
pub(crate) fn open_tabs(s: &App) -> Vec<(String, String)> {
    let shown = s.active_file_tab().and_then(|(sid, path)| Some((sid, path?)));
    let mut tabs: Vec<(String, String)> = shown.iter().cloned().collect();
    for (sid, st) in s.side.sessions.iter() {
        for key in &st.tab_state.tabs.all {
            if let Some(path) = key.strip_prefix("file:").and_then(super::data::file_tab_path)
                && shown.as_ref() != Some(&(sid.clone(), path.clone()))
            {
                tabs.push((sid.clone(), path));
            }
        }
    }
    tabs
}

/// The folder `sid`'s file `path` is in, when the server can watch it.
fn directory(s: &App, sid: &str, path: &str) -> Option<String> {
    if !matches!(*s.source, Source::Live { .. }) || path.is_empty() || opencode_kit::path::is_absolute(path) {
        return None;
    }
    s.session(sid).map(|i| i.directory.clone()).filter(|d| !d.is_empty())
}

fn send(s: &App, method: &'static str, directory: &str, path: &str) {
    let Source::Live { server, .. } = &*s.source else { return };
    let (server, path) = (server.clone(), path.to_owned());
    let url = format!("/api/experimental/fs/watch?location%5Bdirectory%5D={}", crate::switcher::encode(directory));
    let _ = rune::task::spawn("file-watch", move || {
        let _ = server.write(method, &url, Some(json!({ "path": path })));
    });
}
