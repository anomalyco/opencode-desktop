use opencode_kit::model::SessionInfo;
use opencode_search::SearchArena;

use crate::entries::{Action, Entry, MATCHES, RECENT, RECENT_SESSIONS, project_label};
use crate::search_index::{self, Scope};
use crate::{SwitcherHost, SwitcherMode, is_automation, title_of};

/// The palette's sessions for `q`: the loaded roots from the shared index
/// (`search_index`), and the sessions the server found that aren't loaded, in one
/// order (score, then newest); a session in both shows its newer copy.
pub(crate) fn session_matches(host: &dyn SwitcherHost, q: &str, group: &'static str, archived: bool) -> Vec<Entry> {
    let limit = if q.trim().is_empty() { RECENT_SESSIONS.max(RECENT) } else { MATCHES };
    let sw = host.switcher();
    let fulltext = if !q.trim().is_empty() { host.palette_fulltext_rows() } else { Vec::new() };
    let mut hits: Vec<(i32, i64, &SessionInfo)> = Vec::new();
    let titles = sw.mode == SwitcherMode::All;
    let scope = if titles { Scope::CommandTitles } else { Scope::Palette { archived } };
    for (score, updated, id) in search_index::find_sessions(host, scope, q, limit) {
        let Some(s) = search_index::search_session(host, &id) else { continue };
        let s = sw.loaded.iter().find(|x| x.id == s.id && x.updated >= s.updated).unwrap_or(s);
        hits.push((score, updated, s));
    }
    // The server's finds are few: ranked here, against the same scores.
    let found: Vec<&SessionInfo> = sw.loaded.iter().filter(|s| (s.archived.is_some()) == archived && search_index::search_session(host, &s.id).is_none()).collect();
    if !found.is_empty() {
        if q.trim().is_empty() {
            hits.extend(found.iter().filter(|s| !is_automation(&s.title)).map(|s| (0, host.session_active_ms(s), *s)));
        } else {
            let mut arena = SearchArena::default();
            for s in &found {
                // Titles only, as the server matched them (`title LIKE`).
                arena.push(&title_of(s));
            }
            let context = |i: usize| Some((0, host.session_active_ms(found[i])));
            let ranked = if titles { arena.ranked_titles(q, MATCHES, context) } else { arena.ranked_scored(q, MATCHES, context) };
            hits.extend(ranked.into_iter().map(|(score, updated, i)| (score, updated, found[i])));
        }
        hits.sort_by(|a, b| b.0.cmp(&a.0).then(b.1.cmp(&a.1)));
        hits.truncate(limit);
    }
    // Then the sessions only their messages match (full text), in its order; each row shows
    // its line of context.
    for r in &fulltext {
        if !hits.iter().any(|(_, _, s)| s.id == r.session)
            && let Some(s) = search_index::search_session(host, &r.session).or(r.fallback.as_ref())
        {
            hits.push((0, host.session_active_ms(s), s));
        }
    }
    hits.into_iter()
        .map(|(_, _, s)| Entry {
            key: format!("session:{}", s.id),
            action: Action::Session(s.id.clone()),
            title: title_of(s),
            project: s.project.clone(),
            description: project_label(host, s),
            hint: None,
            chord: None,
            group,
            updated: Some(host.session_active_ms(s)),
            open: host.is_tab_open(None, &s.id),
        })
        .collect()
}
