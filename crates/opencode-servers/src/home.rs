//! Home's server and project selection (`home/machine.ts`, ported transition for transition):
//! which server Home focuses and which of its used projects is selected, checked against the
//! servers Home lists, with the selected project's worktree listing demand beside it.

use opencode_kit::servers::key::ServerKey;
use rune::machine::{Attempt, Effects};

/// `layout.home.selection` (`HomeProjectSelection`).
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct Selection {
    pub server: Option<ServerKey>,
    pub directory: Option<String>,
}

impl Selection {
    /// `selection(server, directory)` (`home/machine.ts:339-344`): no server is no selection, and
    /// an empty directory is none.
    pub fn new(server: Option<ServerKey>, directory: Option<String>) -> Selection {
        let Some(server) = server else { return Selection::default() };
        Selection { server: Some(server), directory: directory.filter(|d| !d.is_empty()) }
    }

    pub fn server(server: ServerKey) -> Selection {
        Selection { server: Some(server), directory: None }
    }

    /// `toggleHomeProjectSelection` (`shell/layout/helpers.ts:15-23`).
    pub fn toggle(&self, server: &ServerKey, directory: &str) -> Selection {
        if self.server.as_ref() == Some(server) && self.directory.as_deref() == Some(directory) {
            return Selection::server(server.clone());
        }
        Selection { server: Some(server.clone()), directory: Some(directory.to_owned()) }
    }
}

/// A used project (`HomeProjectItem`, `:6-9`).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Project {
    pub id: String,
    pub worktree: String,
}

/// A server Home lists (`HomeServerSnapshot`, `:11-18`).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ServerSnapshot {
    pub key: ServerKey,
    pub failed: bool,
    pub connected: bool,
    /// Its project list loaded, and an older build's directories for it are migrated.
    pub projects_ready: bool,
    pub used_projects: Vec<Project>,
}

/// `HomeEnvSnapshot` (`:20-27`).
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct Env {
    /// The stored layout, the stored servers and the plugins' servers have all loaded.
    pub hydrated: bool,
    pub stored: Selection,
    pub servers: Vec<ServerSnapshot>,
}

/// The selected project's worktree listing, once per connected server and project
/// (`HomeWorktreeSync`, `:29-38`).
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub enum Worktrees {
    #[default]
    Idle,
    Listing { attempt: Attempt, server: ServerKey, project_id: String },
    Listed { server: ServerKey, project_id: String },
}

/// A project add whose result selects the project, unless the user selects something first.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Adding {
    pub attempt: Attempt,
    pub server: ServerKey,
}

/// What every live state keeps (`HomeSelectionBase`, `:40-48`).
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct Base {
    pub issued: Attempt,
    /// The selection storage holds, as last read or written.
    pub stored: Selection,
    pub servers: Vec<ServerSnapshot>,
    pub adding: Option<Adding>,
    pub worktrees: Worktrees,
}

/// `HomeSelectionState` (`:50-70`):
/// - `Hydrating`: the layout or a server source hasn't loaded. Home shows the stored server when
///   listed, else the first, and writes nothing; a selection made meanwhile is `user`, which wins.
/// - `Verifying`: the server is listed; the directory waits for its projects to be kept or cleared.
/// - `Ready`: the server and its optional used project are checked against the loaded state.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum State {
    Hydrating { base: Base, user: Option<Selection> },
    Verifying { base: Base, server: ServerKey, directory: String },
    Ready { base: Base, server: ServerKey, project: Option<Project> },
    Disposed,
}

impl Default for State {
    /// `INITIAL_HOME_SELECTION_STATE` (`:110-118`).
    fn default() -> State {
        State::Hydrating { base: Base::default(), user: None }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Event {
    EnvSynced(Env),
    ServerFocused(ServerKey),
    ProjectToggled { server: ServerKey, directory: String },
    ProjectAdd { server: ServerKey, directories: Vec<String> },
    ProjectAdded { attempt: Attempt, project: Option<Project> },
    WorktreesListed { attempt: Attempt },
    Disposed,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Effect {
    SelectionPersist(Selection),
    ProjectAdd { attempt: Attempt, server: ServerKey, directories: Vec<String> },
    WorktreesList { attempt: Attempt, server: ServerKey, project_id: String },
    WorktreesRefresh { server: ServerKey, project_id: String },
}

impl State {
    fn base(&self) -> Option<&Base> {
        match self {
            State::Hydrating { base, .. } | State::Verifying { base, .. } | State::Ready { base, .. } => Some(base),
            State::Disposed => None,
        }
    }

    fn base_mut(&mut self) -> Option<&mut Base> {
        match self {
            State::Hydrating { base, .. } | State::Verifying { base, .. } | State::Ready { base, .. } => Some(base),
            State::Disposed => None,
        }
    }

    fn with_base(&self, base: Base) -> State {
        match self {
            State::Hydrating { user, .. } => State::Hydrating { base, user: user.clone() },
            State::Verifying { server, directory, .. } => State::Verifying { base, server: server.clone(), directory: directory.clone() },
            State::Ready { server, project, .. } => State::Ready { base, server: server.clone(), project: project.clone() },
            State::Disposed => State::Disposed,
        }
    }

    /// What Home shows as selected: always a listed server while there is one, and a used
    /// project's own path (`selectHomeSelection`, `:166-183`).
    pub fn selection(&self) -> Selection {
        match self {
            State::Disposed => Selection::default(),
            State::Verifying { server, directory, .. } => Selection { server: Some(server.clone()), directory: Some(directory.clone()) },
            State::Ready { server, project, .. } => Selection { server: Some(server.clone()), directory: project.as_ref().map(|p| p.worktree.clone()) },
            State::Hydrating { base, user } => {
                let target = user.as_ref().unwrap_or(&base.stored);
                let Some(entry) = base.servers.iter().find(|s| Some(&s.key) == target.server.as_ref()) else {
                    return base.servers.first().map(|s| Selection::server(s.key.clone())).unwrap_or_default();
                };
                let Some(dir) = &target.directory else { return Selection::server(entry.key.clone()) };
                Selection { server: Some(entry.key.clone()), directory: Some(find_project(entry, dir).map_or_else(|| dir.clone(), |p| p.worktree.clone())) }
            }
        }
    }
}

/// `transitionHomeSelection` (`home/machine.ts:120-164`).
pub fn step(state: &mut State, event: Event) -> Effects<Effect> {
    let Some(base) = state.base() else { return Effects::none() };
    match event {
        Event::Disposed => {
            *state = State::Disposed;
            Effects::none()
        }
        Event::EnvSynced(env) => synced(state, env),
        Event::ServerFocused(server) => choose(state, Selection::server(server)),
        Event::ProjectToggled { server, directory } => {
            let entry = base.servers.iter().find(|s| s.key == server);
            let Some((entry, project)) = entry.filter(|e| !e.failed).and_then(|e| Some((e, find_project(e, &directory)?))) else { return Effects::none() };
            let target = state.selection().toggle(&entry.key, &project.worktree);
            choose(state, target)
        }
        Event::ProjectAdd { server, directories } => {
            if directories.is_empty() {
                return Effects::none();
            }
            let Some(base) = state.base_mut() else { return Effects::none() };
            let attempt = Attempt(base.issued.0 + 1);
            base.issued = attempt;
            base.adding = Some(Adding { attempt, server: server.clone() });
            Effects::one(Effect::ProjectAdd { attempt, server, directories })
        }
        Event::ProjectAdded { attempt, project } => {
            let Some(adding) = base.adding.clone().filter(|a| a.attempt == attempt) else { return Effects::none() };
            match project {
                None => {
                    if let Some(base) = state.base_mut() {
                        base.adding = None;
                    }
                    Effects::none()
                }
                Some(project) => choose(state, Selection { server: Some(adding.server), directory: Some(project.worktree) }),
            }
        }
        Event::WorktreesListed { attempt } => {
            let Some(base) = state.base_mut() else { return Effects::none() };
            let Worktrees::Listing { attempt: a, server, project_id } = &base.worktrees else { return Effects::none() };
            if *a != attempt {
                return Effects::none();
            }
            let (server, project_id) = (server.clone(), project_id.clone());
            base.worktrees = Worktrees::Listed { server: server.clone(), project_id: project_id.clone() };
            Effects::one(Effect::WorktreesRefresh { server, project_id })
        }
    }
}

/// `synced` (`:189-212`).
fn synced(state: &mut State, env: Env) -> Effects<Effect> {
    let Some(base) = state.base() else { return Effects::none() };
    let stored = Selection::new(env.stored.server.clone(), env.stored.directory.clone());
    if let State::Hydrating { user, .. } = &*state {
        let next = Base { servers: env.servers, stored: stored.clone(), ..base.clone() };
        // Until everything loaded, the stored server may be missing only because its source
        // hasn't reported yet.
        if !env.hydrated {
            let next = State::Hydrating { base: next, user: user.clone() };
            return finish(state, next, Effects::none());
        }
        let target = user.clone().unwrap_or(stored);
        return place(state, next, target);
    }
    // Another part of the app wrote the selection, such as a session's "Show in Home".
    let external = env.stored != base.stored;
    let next = Base {
        servers: env.servers,
        stored: if external { stored } else { base.stored.clone() },
        adding: if external { None } else { base.adding.clone() },
        ..base.clone()
    };
    let target = if external { next.stored.clone() } else { state.selection() };
    place(state, next, target)
}

/// A selection the user made: it wins over a project add still running (`choose`, `:214-220`).
fn choose(state: &mut State, target: Selection) -> Effects<Effect> {
    let Some(base) = state.base() else { return Effects::none() };
    let base = Base { adding: None, ..base.clone() };
    if matches!(state, State::Hydrating { .. }) {
        let next = State::Hydrating { base, user: Some(Selection::new(target.server, target.directory)) };
        return finish(state, next, Effects::none());
    }
    place(state, base, target)
}

/// Checks `target` against the listed servers and writes what storage doesn't hold yet
/// (`place`, `:222-254`).
fn place(state: &mut State, base: Base, target: Selection) -> Effects<Effect> {
    // Home always lists a server; without one the selection stays as it is.
    let Some(first) = base.servers.first() else {
        let next = state.with_base(base);
        return finish(state, next, Effects::none());
    };
    let entry = base.servers.iter().find(|s| Some(&s.key) == target.server.as_ref());
    let (server, verify, project) = match (entry, &target.directory) {
        // The selected server is gone: its directory was its own, so only the first is kept.
        (None, _) => (first.key.clone(), None, None),
        (Some(e), None) => (e.key.clone(), None, None),
        (Some(e), Some(dir)) if !e.projects_ready => (e.key.clone(), Some(dir.clone()), None),
        (Some(e), Some(dir)) => (e.key.clone(), None, find_project(e, dir).cloned()),
    };
    let mut next = match verify {
        Some(directory) => State::Verifying { base, server, directory },
        None => State::Ready { base, server, project },
    };
    let persisted = next.selection();
    let Some(b) = next.base_mut() else { return Effects::none() };
    if persisted == b.stored {
        return finish(state, next, Effects::none());
    }
    b.stored = persisted.clone();
    finish(state, next, Effects::one(Effect::SelectionPersist(persisted)))
}

/// Starts the worktree demand when the selected used project changes or its server connects,
/// and drops it otherwise (`finish`, `:256-276`).
fn finish(state: &mut State, mut next: State, mut fx: Effects<Effect>) -> Effects<Effect> {
    let target = worktree_target(&next);
    if let Some(base) = next.base_mut() {
        let kept = match (&target, &base.worktrees) {
            (Some(_), Worktrees::Idle) => false,
            (Some((server, id)), Worktrees::Listing { server: s, project_id: p, .. } | Worktrees::Listed { server: s, project_id: p }) => s == server && p == id,
            (None, w) => *w == Worktrees::Idle,
        };
        if !kept {
            match target {
                None => base.worktrees = Worktrees::Idle,
                Some((server, project_id)) => {
                    let attempt = Attempt(base.issued.0 + 1);
                    base.issued = attempt;
                    base.worktrees = Worktrees::Listing { attempt, server: server.clone(), project_id: project_id.clone() };
                    fx.push(Effect::WorktreesList { attempt, server, project_id });
                }
            }
        }
    }
    *state = next;
    fx
}

/// `worktreeTarget` (`:278-288`).
fn worktree_target(state: &State) -> Option<(ServerKey, String)> {
    let current = state.selection();
    let entry = state.base()?.servers.iter().find(|s| Some(&s.key) == current.server.as_ref())?;
    let dir = current.directory.as_ref().filter(|_| entry.connected)?;
    Some((entry.key.clone(), find_project(entry, dir)?.id.clone()))
}

/// `findProject` (`:333-337`): matched by `sameDirectory` (`packages/util/src/path.ts:60-72`).
fn find_project<'a>(server: &'a ServerSnapshot, directory: &str) -> Option<&'a Project> {
    let key = same_directory_key(directory);
    server.used_projects.iter().find(|p| same_directory_key(&p.worktree) == key)
}

/// `comparablePath` (`packages/util/src/path.ts:51-57`): forward slashes for Windows paths, no
/// trailing slash except on roots.
pub fn comparable_path(path: &str) -> String {
    let windows = path.as_bytes().get(1) == Some(&b':') || path.starts_with("\\\\");
    let value = if windows { path.replace('\\', "/") } else { path.to_owned() };
    let trimmed = value.trim_end_matches('/');
    if trimmed.is_empty() && value.starts_with('/') {
        return "/".to_owned();
    }
    let b = trimmed.as_bytes();
    if b.len() == 2 && b[1] == b':' && b[0].is_ascii_alphabetic() {
        return format!("{trimmed}/");
    }
    trimmed.to_owned()
}

/// Normalization key for `containsDirectory` / `sameDirectory` (`packages/util/src/path.ts:60-72`):
/// applies `comparable_path` and lowercases Windows drive (`^[a-z]:/`) and UNC (`//`) paths so
/// `C:\Repos\x` and `c:\repos\x` compare equal while POSIX paths stay case-sensitive.
pub fn same_directory_key(path: &str) -> String {
    let key = comparable_path(path);
    let b = key.as_bytes();
    let windows_drive = b.len() >= 3 && b[0].is_ascii_alphabetic() && b[1] == b':' && b[2] == b'/';
    if windows_drive || key.starts_with("//") {
        key.to_lowercase()
    } else {
        key
    }
}
