//! An untracked folder is one Git status row. Only opening it reads its entries; the panel
//! remembers a loaded listing across collapse (`gui-plugins/src/review/folder-machine.ts`).

use std::collections::HashMap;

use rune_kit::FileNode;

use super::{ChangeMode, SessionReview, data::ChangeList};
use crate::{App, jobs::Apply};

#[derive(Default)]
pub(super) enum Listing {
    #[default]
    Unopened,
    Loading,
    Ready { entries: usize, more: bool },
    Failed,
}

enum Event { Open, Close, Loaded(usize, bool), Failed }

impl Listing {
    /// A transition returns true only when its caller needs to issue a request.
    fn step(&mut self, event: Event) -> bool {
        match (&*self, event) {
            (Self::Unopened, Event::Open) => { *self = Self::Loading; return true; }
            (Self::Failed, Event::Close) => *self = Self::Unopened,
            (Self::Loading, Event::Loaded(entries, more)) => *self = Self::Ready { entries, more },
            (Self::Loading, Event::Failed) => *self = Self::Failed,
            _ => {}
        }
        false
    }
}

impl SessionReview {
    pub(crate) fn folder_label(&self, path: &str) -> Option<String> {
        match self.folders.get(path)? {
            Listing::Ready { entries, more: true } => Some(opencode_i18n::t!("plugin.review.folder.more", count = *entries)),
            Listing::Ready { entries, .. } => Some(entries.to_string()),
            Listing::Failed => Some(opencode_i18n::t!("plugin.review.folder.failed").to_owned()),
            _ => None,
        }
    }

    pub(super) fn add_folder_nodes(&mut self) {
        for path in self.folders.keys() {
            if self.tree.get(path).is_some() { continue; }
            let mut parent = None;
            for end in path.match_indices('/').map(|(i, _)| i).chain(std::iter::once(path.len())) {
                let id = &path[..end];
                let name = id.rsplit('/').next().unwrap_or(id);
                self.tree.insert(parent, id, FileNode { dir: true, path: id.to_owned(), name: name.to_owned(), data: None });
                parent = Some(id);
            }
        }
        if !self.folders.is_empty() {
            self.tree.sort_by(|a, b| b.dir.cmp(&a.dir).then_with(|| rune::text::compare_names(&a.name, &b.name)));
        }
    }
}

impl App {
    pub(crate) fn expand_review_dir(&mut self, sid: &str, path: &str, open: bool) {
        let Some(sr) = self.review.sessions.get_mut(sid) else { return };
        sr.tree.set_expanded(path, open);
        sr.stamp += 1;
        let Some(listing) = sr.folders.get_mut(path) else { return };
        if !listing.step(if open { Event::Open } else { Event::Close }) { return; }
        let (sid, path, mode, generation) = (sid.to_owned(), path.to_owned(), sr.mode, sr.generation);
        let Some(info) = self.session(&sid) else { return };
        let root = self.project_for_session(info).map_or(&info.directory, |p| &p.directory);
        let sep = if root.contains('\\') { "\\" } else { "/" };
        let dir = format!("{}{sep}{}", root.trim_end_matches(['/', '\\']), path.replace('/', sep));
        let mode_name = if mode == ChangeMode::Branch { "branch" } else { "working" };
        let url = format!("/api/vcs/diff?location%5Bdirectory%5D={}&mode={mode_name}&untracked=folders", crate::switcher::encode(&dir));
        self.job(move |srv| {
            let result = srv.get(&url);
            Box::new(move |s: &mut App, tx: &mut rune::Tx| {
                let (diff_mode, expand_mode) = (s.review.diff_mode, s.review.expand_mode);
                let Some(sr) = s.review.sessions.get_mut(&sid) else { return };
                if sr.mode != mode || sr.generation != generation { return; }
                let Some(state) = sr.folders.get_mut(&path) else { return };
                match result {
                    Err(_) => { state.step(Event::Failed); if !sr.tree.is_expanded(&path) { state.step(Event::Close); } }
                    Ok(rows) => {
                        let rows = ChangeList::read(rows);
                        state.step(Event::Loaded(rows.count, rows.more));
                        let mut opened: HashMap<_, _> = sr.folders.keys().map(|f| (f.clone(), sr.tree.is_expanded(f))).collect();
                        for folder in rows.folders { opened.entry(folder.clone()).or_default(); sr.folders.entry(folder).or_default(); }
                        let offset = sr.meta.len();
                        let mut meta = std::mem::take(&mut sr.meta);
                        meta.extend(rows.meta.into_iter().map(|mut m| { m.listed += offset; m }));
                        let mut listed = (0..meta.len()).collect::<Vec<_>>();
                        listed.sort_unstable_by_key(|i| meta[*i].listed);
                        let paths = listed.into_iter().map(|i| meta[i].path.clone()).collect();
                        let mut files = sr.diffs.take_files();
                        files.extend(rows.files);
                        sr.land(files, meta, paths, diff_mode, expand_mode);
                        for (folder, open) in opened { sr.tree.set_expanded(&folder, open); }
                    }
                }
                sr.stamp += 1;
                tx.instant();
            }) as Apply
        });
    }
}
