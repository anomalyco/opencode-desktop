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
        let titles = self.switcher.mode == SwitcherMode::All;
        let scope = if titles { Scope::CommandTitles } else { Scope::Palette { archived } };
        for (score, updated, id) in self.find_sessions(scope, q, limit) {
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
                    // Titles only, as the server matched them (`title LIKE`).
                    arena.push(&title_of(s));
                }
                let context = |i: usize| Some((0, found[i].active_ms()));
                let ranked = if titles { arena.ranked_titles(q, MATCHES, context) } else { arena.ranked_scored(q, MATCHES, context) };
                hits.extend(ranked.into_iter().map(|(score, updated, i)| (score, updated, found[i])));
            }
            hits.sort_by(|a, b| b.0.cmp(&a.0).then(b.1.cmp(&a.1)));
            hits.truncate(limit);
        }
        // Then the sessions only their messages match (full text), in its order; each row shows
        // its line of context.
        if !q.trim().is_empty() {
            for r in self.fulltext_rows(crate::fulltext::Surface::Palette) {
                if !hits.iter().any(|(_, _, s)| s.id == r.session)
                    && let Some(s) = self.search_session(&r.session).or(r.fallback.as_ref())
                {
                    hits.push((0, s.active_ms(), s));
                }
            }
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
