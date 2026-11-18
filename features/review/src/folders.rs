//! An untracked folder is one Git status row. Only opening it reads its entries; the panel
//! remembers a loaded listing across collapse (`gui-plugins/src/review/folder-machine.ts`).

use std::collections::HashMap;

use rune::Tx;
use serde_json::Value;

use crate::data::{ChangeList, insert_folder_nodes};
use crate::{ChangeMode, ReviewHost, ReviewJob, SessionReview};

#[derive(Default)]
pub(crate) enum Listing {
    #[default]
    Unopened,
    Loading,
    Ready { entries: usize, more: bool },
    Failed,
}

enum Event {
    Open,
    Close,
    Loaded(usize, bool),
    Failed,
}

impl Listing {
    /// A transition returns true only when its caller needs to issue a request.
    fn step(&mut self, event: Event) -> bool {
        match (&*self, event) {
            (Self::Unopened, Event::Open) => {
                *self = Self::Loading;
                return true;
            }
            (Self::Failed, Event::Close) => *self = Self::Unopened,
            (Self::Loading, Event::Loaded(entries, more)) => *self = Self::Ready { entries, more },
            (Self::Loading, Event::Failed) => *self = Self::Failed,
            _ => {}
        }
        false
    }
}

impl SessionReview {
    pub fn folder_label(&self, path: &str) -> Option<String> {
        match self.folders.get(path)? {
            Listing::Ready { entries, more: true } => Some(opencode_i18n::t!("plugin.review.folder.more", count = *entries)),
            Listing::Ready { entries, .. } => Some(entries.to_string()),
            Listing::Failed => Some(opencode_i18n::t!("plugin.review.folder.failed").to_owned()),
            _ => None,
        }
    }

    pub(crate) fn add_folder_nodes(&mut self) {
        insert_folder_nodes(&mut self.tree, self.folders.keys().map(String::as_str));
    }
}

/// Expands or collapses folder `path` in session `sid`'s change tree, issuing a server read
/// when an untracked folder is opened for the first time.
pub fn expand_folder(host: &mut dyn ReviewHost, sid: &str, path: &str, open: bool) {
    let request = {
        let Some(sr) = host.review_mut().sessions.get_mut(sid) else { return };
        sr.tree.set_expanded(path, open);
        sr.stamp += 1;
        sr.folders
            .get_mut(path)
            .and_then(|listing| listing.step(if open { Event::Open } else { Event::Close }).then_some((sr.mode, sr.generation)))
    };
    host.on_review_changed(sid);
    let Some((mode, generation)) = request else { return };
    let Some(root) = host.session_project_root(sid) else { return };
    let sep = if root.contains('\\') { "\\" } else { "/" };
    let dir = format!("{}{sep}{}", root.trim_end_matches(['/', '\\']), path.replace('/', sep));
    let mode_name = if mode == ChangeMode::Branch { "branch" } else { "working" };
    let url = format!(
        "/api/vcs/diff?location%5Bdirectory%5D={}&mode={mode_name}&untracked=folders",
        opencode_search::query::percent_encode(&dir),
    );
    host.spawn_job(ReviewJob::ReadFolder {
        session_id: sid.to_owned(),
        path: path.to_owned(),
        mode,
        generation,
        url,
    });
}

/// Toggles folder `path` in session `sid`'s change tree.
pub fn toggle_folder(host: &mut dyn ReviewHost, sid: &str, path: &str) {
    let Some(open) = host.review().sessions.get(sid).map(|sr| !sr.tree.is_expanded(path)) else { return };
    expand_folder(host, sid, path, open);
}

pub(crate) fn apply_loaded(
    host: &mut dyn ReviewHost,
    sid: &str,
    path: &str,
    mode: ChangeMode,
    generation: u64,
    result: Result<Value, String>,
    tx: &mut Tx,
) {
    let wrap = host.wrap_lines();
    let changed = {
        let review = host.review_mut();
        let (diff_mode, expand_mode) = (review.diff_mode, review.expand_mode);
        let Some(sr) = review.sessions.get_mut(sid) else { return };
        if sr.mode != mode || sr.generation != generation {
            return;
        }
        let Some(state) = sr.folders.get_mut(path) else { return };
        match result {
            Err(_) => {
                state.step(Event::Failed);
                if !sr.tree.is_expanded(path) {
                    state.step(Event::Close);
                }
            }
            Ok(rows) => {
                let rows = ChangeList::read(rows);
                state.step(Event::Loaded(rows.count, rows.more));
                let mut opened: HashMap<_, _> = sr.folders.keys().map(|f| (f.clone(), sr.tree.is_expanded(f))).collect();
                for folder in rows.folders {
                    opened.entry(folder.clone()).or_default();
                    sr.folders.entry(folder).or_default();
                }
                let offset = sr.meta.len();
                let mut meta = std::mem::take(&mut sr.meta);
                meta.extend(rows.meta.into_iter().map(|mut m| {
                    m.listed += offset;
                    m
                }));
                let mut listed = (0..meta.len()).collect::<Vec<_>>();
                listed.sort_unstable_by_key(|i| meta[*i].listed);
                let paths = listed.into_iter().map(|i| meta[i].path.clone()).collect();
                let mut files = sr.diffs.take_files();
                files.extend(rows.files);
                sr.land(files, meta, paths, diff_mode, expand_mode, wrap);
                for (folder, open) in opened {
                    sr.tree.set_expanded(&folder, open);
                }
            }
        }
        sr.stamp += 1;
        true
    };
    if changed {
        host.on_review_changed(sid);
        tx.instant();
    }
}
