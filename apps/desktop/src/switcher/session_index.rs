use super::*;

pub(super) struct Index {
    counts: (usize, usize),
    rows: Vec<Row>,
}

struct Row {
    id: String,
    title: String,
    description: String,
    project: String,
    updated: i64,
    archived: bool,
    search: String,
}

impl App {
    pub(super) fn session_matches(&self, q: &str, group: &'static str, archived: bool) -> Vec<Entry> {
        let mut index = self.switcher.index.borrow_mut();
        let counts = (self.sessions.len(), self.switcher.loaded.len());
        if index.as_ref().is_none_or(|i| i.counts != counts) {
            let mut labels = HashMap::new();
            let rows = self.merged_roots(&self.switcher.loaded).into_iter().map(|s| {
                let description = labels.entry((&s.project, &s.directory)).or_insert_with(|| self.project_label(s)).clone();
                let title = title_of(s);
                let search = format!("{}\0{}\0{}", title, description, s.id).to_lowercase();
                Row { id: s.id.clone(), title, description, project: s.project.clone(), updated: s.updated, archived: s.archived.is_some(), search }
            }).collect();
            *index = Some(Index { counts, rows });
        }
        let needle = q.to_lowercase();
        index.as_ref().unwrap().rows.iter().filter(|r| r.archived == archived && r.search.contains(&needle)).take(MATCHES).map(|r| Entry {
            key: format!("session:{}", r.id), action: Action::Session(r.id.clone()), title: r.title.clone(),
            project: r.project.clone(), description: r.description.clone(), hint: None, chord: None, group,
            updated: Some(r.updated), open: self.tabs.iter().any(|t| t.id == r.id),
        }).collect()
    }
}
