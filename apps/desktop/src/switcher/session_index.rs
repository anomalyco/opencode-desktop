use super::*;

use opencode_search::SearchArena;

use crate::switcher::search_index::Scope;

impl App {
    /// The palette's sessions for `q`: the loaded roots from the shared index
    /// (`crate::session_search`), and the sessions the server found that aren't loaded, in one
    /// order (score, then newest); a session in both shows its newer copy.
    pub(super) fn session_matches(&self, q: &str, group: &'static str, archived: bool) -> Vec<Entry> {
        let limit = if q.trim().is_empty() { RECENT_SESSIONS.max(RECENT) } else { MATCHES };
        let mut hits: Vec<(i32, i64, &SessionInfo)> = Vec::new();
        for (score, updated, id) in self.find_sessions(Scope::Palette { archived }, q, limit) {
            let Some(s) = self.search_session(&id) else { continue };
            let s = self.switcher.loaded.iter().find(|x| x.id == s.id && x.updated >= s.updated).unwrap_or(s);
            hits.push((score, updated, s));
        }
        // The server's finds are few: ranked here, against the same scores.
        let found: Vec<&SessionInfo> = self.switcher.loaded.iter().filter(|s| (s.archived.is_some()) == archived && self.search_session(&s.id).is_none()).collect();
        if !found.is_empty() {
            if q.trim().is_empty() {
                hits.extend(found.iter().filter(|s| !crate::view::is_automation(&s.title)).map(|s| (0, s.active_ms(), *s)));
            } else {
                let mut arena = SearchArena::default();
                for s in &found {
                    arena.push(&format!("{}\0{}{}{}", title_of(s), self.project_label(s), opencode_search::EXACT, s.id));
                }
                hits.extend(arena.ranked_scored(q, MATCHES, |i| Some((0, found[i].active_ms()))).into_iter().map(|(score, updated, i)| (score, updated, found[i])));
            }
            hits.sort_by(|a, b| b.0.cmp(&a.0).then(b.1.cmp(&a.1)));
            hits.truncate(limit);
        }
        hits.into_iter()
            .map(|(_, _, s)| Entry {
                key: format!("session:{}", s.id),
                action: Action::Session(s.id.clone()),
                title: title_of(s),
                project: s.project.clone(),
                description: self.project_label(s),
                hint: None,
                chord: None,
                group,
                updated: Some(s.updated),
                open: self.tabs.iter().any(|t| t.id == s.id),
            })
            .collect()
    }
}
