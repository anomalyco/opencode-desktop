use super::*;

pub(super) struct Index {
    counts: (usize, usize),
    rows: Vec<Row>,
    search: String,
}

struct Row {
    id: String,
    title: String,
    description: String,
    project: String,
    updated: i64,
    archived: bool,
    end: usize,
}

impl App {
    pub(super) fn session_matches(&self, q: &str, group: &'static str, archived: bool) -> Vec<Entry> {
        let mut index = self.switcher.index.borrow_mut();
        let counts = (self.sessions.len(), self.switcher.loaded.len());
        if index.as_ref().is_none_or(|i| i.counts != counts) {
            let mut labels = HashMap::new();
            let mut search = String::new();
            let rows = self.merged_roots(&self.switcher.loaded).into_iter().map(|s| {
                let description = labels.entry((&s.project, &s.directory)).or_insert_with(|| self.project_label(s)).clone();
                let title = title_of(s);
                search.push_str(&format!("{}\0{}\0{}\0", title, description, s.id).to_lowercase());
                Row { id: s.id.clone(), title, description, project: s.project.clone(), updated: s.updated, archived: s.archived.is_some(), end: search.len() }
            }).collect();
            *index = Some(Index { counts, rows, search });
        }
        let needle = q.to_lowercase();
        if needle.contains('\0') { return Vec::new(); }
        let finder = memchr::memmem::Finder::new(needle.as_bytes());
        let index = index.as_ref().unwrap();
        let mut offset = 0;
        let mut next = 0;
        // One contiguous search skips nonmatches without starting a matcher for every row.
        std::iter::from_fn(|| {
            if !needle.is_empty() {
                let hit = offset + finder.find(&index.search.as_bytes()[offset..])?;
                next += index.rows[next..].partition_point(|r| r.end <= hit);
            }
            let row = index.rows.get(next)?;
            offset = row.end;
            next += 1;
            Some(row)
        }).filter(|r| r.archived == archived).take(if q.is_empty() { RECENT_SESSIONS } else { MATCHES }).map(|r| Entry {
            key: format!("session:{}", r.id), action: Action::Session(r.id.clone()), title: r.title.clone(),
            project: r.project.clone(), description: r.description.clone(), hint: None, chord: None, group,
            updated: Some(r.updated), open: self.tabs.iter().any(|t| t.id == r.id),
        }).collect()
    }
}
