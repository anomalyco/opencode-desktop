use super::*;
use std::fmt::Write as _;
use std::hash::{Hash, Hasher};

use opencode_search::SearchArena;

pub(super) struct Index {
    revision: (u64, u64, u64),
    active: Bucket,
    archived: Bucket,
}

#[derive(Default)]
struct Bucket {
    rows: Vec<Row>,
    arena: SearchArena,
    last: Option<(String, Vec<usize>)>,
}

/// Where a row's session is, read when the row is shown: the index holds no copy of it, and it
/// is built again whenever either list changes (`revision`).
#[derive(Clone, Copy)]
enum Row {
    Loaded(u32),
    Found(u32),
}

/// `compareSessionTime`'s id order, a UTF-16 code unit at a time: byte order for ASCII ids.
fn id_order(a: &str, b: &str) -> std::cmp::Ordering {
    if a.is_ascii() && b.is_ascii() { a.cmp(b) } else { a.encode_utf16().cmp(b.encode_utf16()) }
}

impl App {
    fn row_session(&self, row: Row) -> &SessionInfo {
        match row {
            Row::Loaded(i) => &self.sessions[i as usize],
            Row::Found(i) => &self.switcher.loaded[i as usize],
        }
    }

    /// `merged_roots` as rows: loaded roots and the server's matches, newest metadata winning,
    /// newest first.
    fn root_rows(&self) -> Vec<Row> {
        let found = &self.switcher.loaded;
        let by_id: HashMap<&str, usize> = found.iter().enumerate().map(|(i, s)| (s.id.as_str(), i)).collect();
        let mut beaten = vec![false; found.len()];
        let mut rows: Vec<(i64, Row)> = Vec::with_capacity(self.sessions.len() + found.len());
        for (i, s) in self.sessions.iter().enumerate().filter(|(_, s)| s.parent.is_none()) {
            if let Some(&f) = by_id.get(s.id.as_str()) {
                // A later copy wins a tie, as `merged_roots` keeps it.
                if found[f].updated >= s.updated {
                    continue;
                }
                beaten[f] = true;
            }
            rows.push((s.active_ms(), Row::Loaded(i as u32)));
        }
        rows.extend(found.iter().enumerate().filter(|(i, _)| !beaten[*i]).map(|(i, s)| (s.active_ms(), Row::Found(i as u32))));
        rows.sort_unstable_by(|a, b| b.0.cmp(&a.0).then_with(|| id_order(&self.row_session(a.1).id, &self.row_session(b.1).id)));
        rows.into_iter().map(|(_, r)| r).collect()
    }

    pub(super) fn session_matches(&self, q: &str, group: &'static str, archived: bool) -> Vec<Entry> {
        let mut cache = self.switcher.index.borrow_mut();
        let found = {
            let mut h = std::hash::DefaultHasher::new();
            self.switcher.loaded.iter().for_each(|s| (&s.id, s.updated).hash(&mut h));
            h.finish()
        };
        let revision = (rune::Tracked::version(&self.sessions), rune::Tracked::version(&self.projects), found);
        if cache.as_ref().is_none_or(|i| i.revision != revision) {
            let mut labels: HashMap<(&str, &str), String> = HashMap::new();
            let mut index = Index { revision, active: Bucket::default(), archived: Bucket::default() };
            let mut text = String::new();
            for row in self.root_rows() {
                let s = self.row_session(row);
                let description = labels.entry((&s.project, &s.directory)).or_insert_with(|| self.project_label(s));
                let bucket = if s.archived.is_some() { &mut index.archived } else { &mut index.active };
                text.clear();
                let title = if s.title.is_empty() { opencode_i18n::t!("rune.session.untitled") } else { s.title.as_str() };
                let _ = write!(text, "{title}\0{description}\0{}", s.id);
                bucket.arena.push(&text);
                bucket.rows.push(row);
            }
            *cache = Some(index);
        }
        let Some(index) = cache.as_mut() else { return Vec::new() };
        let bucket = if archived { &mut index.archived } else { &mut index.active };
        if bucket.last.as_ref().is_none_or(|r| r.0 != q) {
            let ids = if q.is_empty() {
                let rows = &bucket.rows;
                (0..rows.len()).filter(|&i| !crate::view::is_automation(&title_of(self.row_session(rows[i])))).take(RECENT_SESSIONS).collect()
            } else { bucket.arena.query(q, MATCHES) };
            bucket.last = Some((q.to_owned(), ids));
        }
        bucket.last.iter().flat_map(|(_, hits)| hits).map(|&i| {
            let s = self.row_session(bucket.rows[i]);
            Entry { key: format!("session:{}", s.id), action: Action::Session(s.id.clone()), title: title_of(s), project: s.project.clone(),
                description: self.project_label(s), hint: None, chord: None, group, updated: Some(s.updated), open: self.tabs.iter().any(|t| t.id == s.id) }
        }).collect()
    }
}
