use super::*;
use opencode_search::SearchArena;

pub(super) struct Index {
    revision: (u64, u64, usize),
    active: Bucket,
    archived: Bucket,
}

#[derive(Default)]
struct Bucket {
    rows: Vec<Row>,
    arena: SearchArena,
    last: Option<(String, Vec<usize>)>,
}

struct Row {
    id: String,
    title: String,
    description: String,
    project: String,
    updated: i64,
}

impl App {
    pub(super) fn session_matches(&self, q: &str, group: &'static str, archived: bool) -> Vec<Entry> {
        let mut cache = self.switcher.index.borrow_mut();
        let revision = (rune::Tracked::version(&self.sessions), rune::Tracked::version(&self.projects), self.switcher.loaded.len());
        if cache.as_ref().is_none_or(|i| i.revision != revision) {
            let mut labels = HashMap::new();
            let mut index = Index { revision, active: Bucket::default(), archived: Bucket::default() };
            for s in self.merged_roots(&self.switcher.loaded) {
                let description = labels.entry((&s.project, &s.directory)).or_insert_with(|| self.project_label(s)).clone();
                let title = title_of(s);
                let bucket = if s.archived.is_some() { &mut index.archived } else { &mut index.active };
                bucket.arena.push(&format!("{title}\0{description}\0{}", s.id));
                bucket.rows.push(Row { id: s.id.clone(), title, description, project: s.project.clone(), updated: s.updated });
            }
            *cache = Some(index);
        }
        let Some(index) = cache.as_mut() else { return Vec::new() };
        let bucket = if archived { &mut index.archived } else { &mut index.active };
        if bucket.last.as_ref().is_none_or(|r| r.0 != q) {
            let ids = if q.is_empty() {
                bucket.rows.iter().enumerate().filter(|(_, r)| !crate::view::is_automation(&r.title)).take(RECENT_SESSIONS).map(|(i, _)| i).collect()
            } else { bucket.arena.query(q, MATCHES) };
            bucket.last = Some((q.to_owned(), ids));
        }
        bucket.last.iter().flat_map(|(_, hits)| hits).map(|&i| {
            let r = &bucket.rows[i];
            Entry { key: format!("session:{}", r.id), action: Action::Session(r.id.clone()), title: r.title.clone(), project: r.project.clone(),
                description: r.description.clone(), hint: None, chord: None, group, updated: Some(r.updated), open: self.tabs.iter().any(|t| t.id == r.id) }
        }).collect()
    }
}
