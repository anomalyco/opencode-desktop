//! Pure tab-strip placement, preview replacement, move, close, and `Recently closed` stack
//! (`packages/app/src/shell/state/session-tabs.ts`, `runtime/plugin/closed-tabs.ts`).

use std::collections::{HashMap, HashSet};

use serde_json::{Value, json};

use super::{SESSION_PANES, SessionPane};

pub const CLOSED_TABS_LIMIT: usize = 30;

/// Canonical stored tab key `"{plugin}:{id}"`.
pub fn panel_key(plugin: &str, id: &str) -> String {
    format!("{plugin}:{id}")
}

/// Splits `"{plugin}:{id}"` into `(plugin, id)`.
pub fn split_panel_key(key: &str) -> Option<(&str, &str)> {
    key.split_once(':')
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct SessionTabs {
    pub active: Option<String>,
    pub all: Vec<String>,
    pub panes: HashMap<String, SessionPane>,
    pub selected: HashMap<SessionPane, String>,
}

impl SessionTabs {
    pub fn to_json(&self) -> Value {
        let panes: serde_json::Map<String, Value> = self
            .panes
            .iter()
            .map(|(k, p)| (k.clone(), Value::String(p.name().into())))
            .collect();
        let selected: serde_json::Map<String, Value> = self
            .selected
            .iter()
            .map(|(p, k)| (p.name().into(), Value::String(k.clone())))
            .collect();
        json!({
            "active": self.active,
            "all": self.all,
            "panes": panes,
            "selected": selected,
        })
    }

    pub fn from_json(v: &Value) -> Self {
        let active = v["active"].as_str().map(str::to_owned);
        let all = v["all"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|x| x.as_str().map(str::to_owned))
            .collect();
        let panes = v["panes"]
            .as_object()
            .into_iter()
            .flatten()
            .filter_map(|(k, val)| Some((k.clone(), SessionPane::parse(val.as_str()?)?)))
            .collect();
        let selected = v["selected"]
            .as_object()
            .into_iter()
            .flatten()
            .filter_map(|(p, val)| Some((SessionPane::parse(p)?, val.as_str()?.to_owned())))
            .collect();
        Self { active, all, panes, selected }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct SessionTabState {
    pub tabs: SessionTabs,
    pub preview: Option<String>,
}

impl SessionTabState {
    pub fn to_json(&self) -> Value {
        json!({
            "tabs": self.tabs.to_json(),
            "preview": self.preview,
        })
    }

    pub fn from_json(v: &Value) -> Self {
        Self {
            tabs: SessionTabs::from_json(&v["tabs"]),
            preview: v["preview"].as_str().map(str::to_owned),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct ClosedSideTab {
    pub key: String,
    pub title: String,
    pub icon_name: Option<String>,
    pub file: Option<String>,
    pub detail: Option<String>,
    pub data: Option<String>,
    pub closed_ms: i64,
}

impl ClosedSideTab {
    pub fn to_json(&self) -> Value {
        json!({
            "key": self.key,
            "title": self.title,
            "iconName": self.icon_name,
            "file": self.file,
            "detail": self.detail,
            "data": self.data,
            "closedMs": self.closed_ms,
        })
    }

    pub fn from_json(v: &Value) -> Option<Self> {
        Some(Self {
            key: v["key"].as_str()?.to_owned(),
            title: v["title"].as_str()?.to_owned(),
            icon_name: v["iconName"].as_str().map(str::to_owned),
            file: v["file"].as_str().map(str::to_owned),
            detail: v["detail"].as_str().map(str::to_owned),
            data: v["data"].as_str().map(str::to_owned),
            closed_ms: v["closedMs"].as_i64().unwrap_or(0),
        })
    }
}

pub fn session_tab_pane(tabs: &SessionTabs, tab: &str) -> SessionPane {
    tabs.panes.get(tab).copied().unwrap_or(SessionPane::Side)
}

pub fn pane_tabs<'a>(tabs: &'a SessionTabs, pane: SessionPane) -> Vec<&'a str> {
    tabs.all
        .iter()
        .map(String::as_str)
        .filter(|t| session_tab_pane(tabs, t) == pane)
        .collect()
}

pub fn session_pane_tab(tabs: &SessionTabs, pane: SessionPane) -> Option<&str> {
    if let Some(active) = tabs.active.as_deref()
        && session_tab_pane(tabs, active) == pane
    {
        return Some(active);
    }
    tabs.selected.get(&pane).map(String::as_str)
}

pub fn place_session_tabs(
    previous: &SessionTabs,
    next: SessionTabs,
    target: Option<SessionPane>,
) -> SessionTabs {
    let kept: HashSet<&str> = previous.all.iter().map(String::as_str).collect();
    let has_next_panes = !next.panes.is_empty();

    let placed = |tab: &str, index: usize| -> SessionPane {
        if kept.contains(tab) {
            return if has_next_panes {
                session_tab_pane(&next, tab)
            } else {
                session_tab_pane(previous, tab)
            };
        }
        if let Some(t) = target {
            return t;
        }
        if let Some(replaced) = previous.all.get(index)
            && !next.all.contains(replaced)
        {
            return session_tab_pane(previous, replaced);
        }
        if let Some((plugin, _)) = split_panel_key(tab) {
            let prefix = format!("{plugin}:");
            if let Some(sibling) = previous.all.iter().rev().find(|k| k.starts_with(&prefix)) {
                return session_tab_pane(previous, sibling);
            }
        }
        SessionPane::Side
    };

    let mut panes = HashMap::new();
    for (idx, tab) in next.all.iter().enumerate() {
        let pane = placed(tab, idx);
        if pane != SessionPane::Side {
            panes.insert(tab.clone(), pane);
        }
    }

    let draft = SessionTabs {
        all: next.all.clone(),
        active: next.active.clone(),
        panes: panes.clone(),
        selected: HashMap::new(),
    };
    let active_pane = draft.active.as_deref().map(|a| session_tab_pane(&draft, a));

    let mut selected = HashMap::new();
    for pane in SESSION_PANES {
        if Some(pane) == active_pane {
            if let Some(act) = &draft.active {
                selected.insert(pane, act.clone());
            }
            continue;
        }
        let shown = session_pane_tab(previous, pane);
        let members = pane_tabs(&draft, pane);
        let Some(shown) = shown else {
            if !members.is_empty() && pane != SessionPane::Side {
                selected.insert(pane, members[0].to_owned());
            }
            continue;
        };
        if members.contains(&shown)
            || (pane == SessionPane::Side && !kept.contains(shown) && !draft.all.iter().any(|k| k == shown))
        {
            selected.insert(pane, shown.to_owned());
            continue;
        }
        let before = pane_tabs(previous, pane);
        let idx = before.iter().position(|t| *t == shown).unwrap_or(0);
        let neighbour = before[..idx]
            .iter()
            .rev()
            .chain(before.get(idx + 1..).into_iter().flatten())
            .copied()
            .find(|t| members.contains(t));
        if let Some(fb) = neighbour.or_else(|| members.first().copied()) {
            selected.insert(pane, fb.to_owned());
        }
    }

    SessionTabs {
        all: next.all,
        active: next.active,
        panes,
        selected,
    }
}

pub fn move_session_tab(
    current: &SessionTabs,
    tab: &str,
    pane: SessionPane,
    before: Option<&str>,
) -> SessionTabs {
    let mut rest: Vec<String> = current.all.iter().filter(|k| k.as_str() != tab).cloned().collect();
    let members: Vec<&str> = rest
        .iter()
        .map(String::as_str)
        .filter(|k| session_tab_pane(current, k) == pane)
        .collect();
    let anchor = before.and_then(|b| {
        if members.contains(&b) {
            Some(b)
        } else {
            members.first().copied()
        }
    });
    let pos = if let Some(a) = anchor {
        rest.iter().position(|k| k == a).unwrap_or(rest.len())
    } else if let Some(last) = members.last().copied() {
        rest.iter().position(|k| k == last).map_or(rest.len(), |i| i + 1)
    } else {
        rest.len()
    };
    rest.insert(pos, tab.to_owned());
    let mut panes: HashMap<String, SessionPane> = current
        .panes
        .iter()
        .filter(|(k, _)| k.as_str() != tab)
        .map(|(k, v)| (k.clone(), *v))
        .collect();
    if pane != SessionPane::Side {
        panes.insert(tab.to_owned(), pane);
    }
    place_session_tabs(
        current,
        SessionTabs {
            all: rest,
            active: Some(tab.to_owned()),
            panes,
            selected: HashMap::new(),
        },
        Some(pane),
    )
}

fn preview_slot<'a>(current: &'a SessionTabState, launchers: &HashSet<String>) -> Option<&'a str> {
    if let Some(act) = current.tabs.active.as_deref()
        && launchers.contains(act)
    {
        return Some(act);
    }
    current
        .preview
        .as_deref()
        .or_else(|| current.tabs.all.iter().map(String::as_str).find(|t| launchers.contains(*t)))
}

pub fn open_session_tab(
    current: &SessionTabState,
    tab: &str,
    launchers: &HashSet<String>,
    first: bool,
    target: Option<SessionPane>,
) -> SessionTabState {
    let preview = preview_slot(current, launchers).map(str::to_owned);
    if first {
        let mut all = vec![tab.to_owned()];
        all.extend(current.tabs.all.iter().filter(|k| k.as_str() != tab).cloned());
        let next = place_session_tabs(
            &current.tabs,
            SessionTabs { all, active: Some(tab.to_owned()), ..Default::default() },
            target,
        );
        return SessionTabState { tabs: next, preview };
    }

    let preview_idx = preview
        .as_deref()
        .and_then(|p| current.tabs.all.iter().position(|k| k == p));
    let existing_idx = current.tabs.all.iter().position(|k| k == tab);

    let all = if existing_idx.is_some() {
        if preview_idx.is_none() || preview.as_deref() == Some(tab) {
            current.tabs.all.clone()
        } else {
            current
                .tabs
                .all
                .iter()
                .filter(|k| Some(k.as_str()) != preview.as_deref())
                .cloned()
                .collect()
        }
    } else if let Some(pi) = preview_idx {
        current
            .tabs
            .all
            .iter()
            .enumerate()
            .map(|(i, k)| if i == pi { tab.to_owned() } else { k.clone() })
            .collect()
    } else {
        let mut v = current.tabs.all.clone();
        v.push(tab.to_owned());
        v
    };

    let next = place_session_tabs(
        &current.tabs,
        SessionTabs { all, active: Some(tab.to_owned()), ..Default::default() },
        target,
    );
    SessionTabState { tabs: next, preview: None }
}

pub fn preview_session_tab(
    current: &SessionTabState,
    tab: &str,
    launchers: &HashSet<String>,
    target: Option<SessionPane>,
) -> SessionTabState {
    let preview = preview_slot(current, launchers).map(str::to_owned);
    let preview_idx = preview
        .as_deref()
        .and_then(|p| current.tabs.all.iter().position(|k| k == p));
    let existing_idx = current.tabs.all.iter().position(|k| k == tab);

    let (all, next_preview) = if existing_idx.is_some() {
        if preview_idx.is_none() || preview.as_deref() == Some(tab) {
            (
                current.tabs.all.clone(),
                if preview.as_deref() == Some(tab) { Some(tab.to_owned()) } else { None },
            )
        } else {
            (
                current
                    .tabs
                    .all
                    .iter()
                    .filter(|k| Some(k.as_str()) != preview.as_deref())
                    .cloned()
                    .collect(),
                None,
            )
        }
    } else if let Some(pi) = preview_idx {
        (
            current
                .tabs
                .all
                .iter()
                .enumerate()
                .map(|(i, k)| if i == pi { tab.to_owned() } else { k.clone() })
                .collect(),
            Some(tab.to_owned()),
        )
    } else {
        let mut v = current.tabs.all.clone();
        v.push(tab.to_owned());
        (v, Some(tab.to_owned()))
    };

    let next = place_session_tabs(
        &current.tabs,
        SessionTabs { all, active: Some(tab.to_owned()), ..Default::default() },
        target,
    );
    SessionTabState { tabs: next, preview: next_preview }
}

pub fn close_session_tab(current: &SessionTabState, tab: &str) -> SessionTabState {
    let all: Vec<String> = current.tabs.all.iter().filter(|k| k.as_str() != tab).cloned().collect();
    let preview = current.preview.clone().filter(|p| p != tab);

    if current.tabs.active.as_deref() != Some(tab) {
        let next = place_session_tabs(
            &current.tabs,
            SessionTabs { all, active: current.tabs.active.clone(), ..Default::default() },
            None,
        );
        return SessionTabState { tabs: next, preview };
    }

    let pane = session_tab_pane(&current.tabs, tab);
    let members = pane_tabs(&current.tabs, pane);
    let idx = members.iter().position(|t| *t == tab).unwrap_or(0);
    let same_pane_next = idx
        .checked_sub(1)
        .and_then(|i| members.get(i).copied())
        .or_else(|| members.get(idx + 1).copied());
    let other_pane_next = SESSION_PANES
        .into_iter()
        .filter(|p| *p != pane)
        .filter_map(|p| session_pane_tab(&current.tabs, p))
        .find(|t| all.iter().any(|k| k == t));
    let active = same_pane_next
        .or(other_pane_next)
        .map(str::to_owned)
        .or_else(|| all.first().cloned());

    let next = place_session_tabs(
        &current.tabs,
        SessionTabs { all, active, ..Default::default() },
        None,
    );
    SessionTabState { tabs: next, preview }
}

pub fn push_closed_tab(list: &mut Vec<ClosedSideTab>, entry: ClosedSideTab) {
    list.retain(|x| x.key != entry.key);
    list.insert(0, entry);
    if list.len() > CLOSED_TABS_LIMIT {
        list.truncate(CLOSED_TABS_LIMIT);
    }
}

pub fn take_closed_tab(list: &mut Vec<ClosedSideTab>, key: Option<&str>) -> Option<ClosedSideTab> {
    let pos = match key {
        None => (!list.is_empty()).then_some(0),
        Some(k) => list.iter().position(|x| x.key == k),
    }?;
    Some(list.remove(pos))
}
