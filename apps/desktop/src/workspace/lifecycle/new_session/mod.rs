//! Starting a session from the new-session page. Send shows the new session at once, its prompt
//! the first message with the start's status line under it, while the start runs its phases
//! (Desktop's `new-session/machine.ts`): a new worktree is made first, then the folder's MCP
//! toggles in flight settle, then the server creates the session and takes the prompt
//! (`Write::StartAs`, through the outbox). The id is chosen here, as Desktop does, so nothing
//! swaps when the server's copy lands. A phase that fails or runs out of time goes back to the
//! new-session page with the words in the composer.
use std::collections::HashMap;

use crate::pickers::PickerHost as _;
use opencode_kit::client::{Prompt, Request, Write};
use opencode_kit::model::SessionInfo;
use rune::Tx;
use rune::machine::{Attempt, Seq};

use crate::send_failed::Sent;
use crate::{App, Route, Source};

/// Outlasts the 10 minutes `POST /api/worktree` may take to run the project's setup script, so
/// the phase only ends a create whose answer stalled after that (`NEW_SESSION_WORKTREE_TIMEOUT_MS`).
const WORKTREE_SECS: f64 = 11.0 * 60.0;
/// Waiting for the folder's MCP toggles (60 s each) to settle (`NEW_SESSION_MCP_TIMEOUT_MS`).
const MCP_SECS: f64 = 2.0 * 60.0;

/// The prompt a start waits to send: its outbox row (Preparing from Enter, so a kill while the
/// worktree is made or the MCP servers settle loses nothing; Desktop holds it in memory).
#[derive(Clone)]
struct Held {
    id: String,
}

/// A session on screen that the server may not have yet (`NewSessionStartState`). It shows
/// while its worktree is made (`worktree`), while its folder's MCP toggles settle (`mcp`) and
/// while the server creates it (`session`), and either lands (`started`: it takes the server's
/// details) or rolls back (`tab.rollback`: the draft tab comes back with the words). Rune's
/// session phase is the outbox's journaled `Write::StartAs`, which waits out a lost connection
/// rather than timing out.
#[derive(Clone)]
enum Start {
    /// Making a worktree of `project` from the checkout `project_directory`.
    Worktree { attempt: Attempt, draft: Option<String>, project_directory: String, held: Held, due: f64 },
    /// Waiting for the MCP toggles in flight in `directory` (`waiting`); `failed` once one of
    /// them failed. `created`: the worktree this start made.
    Mcp { attempt: Attempt, draft: Option<String>, held: Held, directory: String, created: bool, waiting: Vec<Attempt>, failed: bool, due: f64 },
    /// Being created in `directory`, in place of draft tab `draft`.
    Creating { directory: String, draft: Option<String>, created: bool },
}

/// Where a start's session runs.
pub(crate) enum Place {
    Existing(String),
    New { project: String, from: String, branch: Option<String> },
}

enum Event {
    /// `toggling`: the MCP toggles in flight in an existing folder.
    Begin { place: Place, draft: Option<String>, held: Held, toggling: Vec<Attempt>, now: f64 },
    WorktreeMade { attempt: Attempt, result: Result<String, String>, now: f64 },
    /// One of the folder's MCP toggles ended.
    McpSettled { directory: String, toggle: Attempt, ok: bool },
    Created,
    Failed,
    Tick { now: f64 },
}

/// Why a start rolled back, as its toast says (`TITLES`, `toast`, `toast.timed-out`).
#[derive(Debug, PartialEq)]
enum Why {
    /// The MCP controller already reported the failed toggle.
    Quiet,
    Failed { title: &'static str, message: String },
    TimedOut { title: &'static str },
}

enum Effect {
    /// `worktree.create`.
    CreateWorktree { attempt: Attempt, project: String, from: String, branch: Option<String> },
    /// `session.create`: the outbox takes the prompt for `directory`.
    Create { directory: String, held: Held },
    /// The server has it: read what opening it would.
    Promote,
    /// Back to the draft tab it replaced, in `directory`, with the words (`held`, when the
    /// outbox doesn't hold them). A worktree this start made becomes the draft's folder.
    Rollback { directory: String, draft: Option<String>, held: Option<Held>, worktree: bool, why: Why },
}

#[derive(Default)]
pub struct Starts {
    starts: HashMap<String, Start>,
    seq: Seq,
}

impl Starts {
    pub fn has(&self, session: &str) -> bool {
        self.starts.contains_key(session)
    }

    /// The pending session's status line (`viewNewSessionStart`).
    pub fn status(&self, session: &str) -> Option<&'static str> {
        match self.starts.get(session)? {
            Start::Worktree { .. } => Some(opencode_i18n::t!("session.new.worktree.creating")),
            Start::Mcp { .. } | Start::Creating { .. } => Some(opencode_i18n::t!("session.new.starting")),
        }
    }

    /// Where the pending session will run (`viewNewSessionStart(state).directory`, #65190):
    /// the project root stands in while a new worktree is created, then the target worktree.
    pub fn directory(&self, session: &str) -> Option<&str> {
        match self.starts.get(session)? {
            Start::Worktree { project_directory, .. } => Some(project_directory.as_str()),
            Start::Mcp { directory, .. } | Start::Creating { directory, .. } => Some(directory.as_str()),
        }
    }

    /// When the next phase's time runs out.
    fn due(&self) -> Option<f64> {
        self.starts.values().filter_map(|s| match s { Start::Worktree { due, .. } | Start::Mcp { due, .. } => Some(*due), Start::Creating { .. } => None }).reduce(f64::min)
    }

    fn step(&mut self, session: &str, event: Event) -> Vec<Effect> {
        match (self.starts.remove(session), event) {
            (None, Event::Begin { place: Place::New { project, from, branch }, draft, held, now, .. }) => {
                let attempt = self.seq.next();
                self.starts.insert(session.to_owned(), Start::Worktree { attempt, draft, project_directory: from.clone(), held, due: now + WORKTREE_SECS });
                vec![Effect::CreateWorktree { attempt, project, from, branch }]
            }
            (None, Event::Begin { place: Place::Existing(directory), draft, held, toggling, now }) => {
                let attempt = self.seq.next();
                self.prepare(session, attempt, draft, held, directory, false, toggling, now)
            }
            (Some(Start::Worktree { attempt, draft, held, .. }), Event::WorktreeMade { attempt: a, result, now }) if a == attempt => match result {
                // A new worktree has no MCP toggles in flight.
                Ok(directory) => self.prepare(session, attempt, draft, held, directory, true, Vec::new(), now),
                // No worktree: the draft comes back in the folder its session showed in.
                Err(message) => {
                    let why = Why::Failed { title: opencode_i18n::t!("prompt.toast.worktreeCreateFailed.title"), message };
                    vec![Effect::Rollback { directory: String::new(), draft, held: Some(held), worktree: false, why }]
                }
            },
            (Some(Start::Mcp { attempt, draft, held, directory, created, mut waiting, failed, due }), Event::McpSettled { directory: d, toggle, ok }) if d == directory => {
                let ours = waiting.contains(&toggle);
                waiting.retain(|t| *t != toggle);
                let failed = failed || (ours && !ok);
                if !waiting.is_empty() {
                    self.starts.insert(session.to_owned(), Start::Mcp { attempt, draft, held, directory, created, waiting, failed, due });
                    return Vec::new();
                }
                if failed {
                    return vec![Effect::Rollback { directory, draft, held: Some(held), worktree: created, why: Why::Quiet }];
                }
                self.create(session, draft, held, directory, created)
            }
            (Some(Start::Creating { .. }), Event::Created) => vec![Effect::Promote],
            (Some(Start::Creating { directory, draft, created }), Event::Failed) => vec![Effect::Rollback { directory, draft, held: None, worktree: created, why: Why::Quiet }],
            // A phase's time only ends that phase; a later phase has its own.
            (Some(Start::Worktree { draft, held, due, .. }), Event::Tick { now }) if now + 1e-9 >= due => {
                let why = Why::TimedOut { title: opencode_i18n::t!("prompt.toast.worktreeCreateFailed.title") };
                vec![Effect::Rollback { directory: String::new(), draft, held: Some(held), worktree: false, why }]
            }
            (Some(Start::Mcp { draft, held, directory, created, due, .. }), Event::Tick { now }) if now + 1e-9 >= due => {
                let why = Why::TimedOut { title: opencode_i18n::t!("session.summary.mcp.prepareFailed") };
                vec![Effect::Rollback { directory, draft, held: Some(held), worktree: created, why }]
            }
            (Some(start), _) => {
                self.starts.insert(session.to_owned(), start);
                Vec::new()
            }
            (None, _) => Vec::new(),
        }
    }

    /// Skips the MCP phase when no toggle in the folder is in flight (`prepare`).
    #[allow(clippy::too_many_arguments)]
    fn prepare(&mut self, session: &str, attempt: Attempt, draft: Option<String>, held: Held, directory: String, created: bool, toggling: Vec<Attempt>, now: f64) -> Vec<Effect> {
        if toggling.is_empty() {
            return self.create(session, draft, held, directory, created);
        }
        self.starts.insert(session.to_owned(), Start::Mcp { attempt, draft, held, directory, created, waiting: toggling, failed: false, due: now + MCP_SECS });
        Vec::new()
    }

    fn create(&mut self, session: &str, draft: Option<String>, held: Held, directory: String, created: bool) -> Vec<Effect> {
        self.starts.insert(session.to_owned(), Start::Creating { directory: directory.clone(), draft, created });
        vec![Effect::Create { directory, held }]
    }
}

pub const FAILED: &str = "Couldn't start the session";

impl App {
    /// Shows session `id`, which the server is about to create in the new-session page's
    /// folder with the toolbar's agent and model, as the open page. It switches at once, as any
    /// session opens; the caller adds the prompt. The draft's id, or None when no draft showed.
    fn show_start(&mut self, id: &str, directory: &str, tx: &mut Tx) -> Option<String> {
        let selection = self.selection();
        // The models the draft's agents were given go with the session, and the next draft
        // starts with none (`new-session/composer-adapter.ts` `choices: model.remembered()`).
        self.composer_ext.pickers.promote_draft(id);
        crate::pickers::saved::save(self, id);
        let mut info = SessionInfo {
            // Provisional until `started` takes the server's create response.
            created: *self.now_ms,
            updated: *self.now_ms,
            id: id.into(),
            directory: directory.to_owned(),
            agent: selection.agent,
            model: selection.model,
            ..Default::default()
        };
        info.project = self.project_for_session(&info).map(|p| p.id.clone()).unwrap_or_default();
        self.sessions.insert(0, info);
        // The draft's tab becomes the session's, where it stands.
        let draft = self.promote_draft(id, directory);
        self.open_now(id, tx);
        draft
    }

    /// A start that can't journal (a fixture or a read-only window): the session shows, and a
    /// live one waits for the server's answer to the write already sent.
    pub(crate) fn start(&mut self, id: &str, tx: &mut Tx) {
        let directory = self.draft_dir().to_owned();
        let draft = self.show_start(id, &directory, tx);
        if self.live() {
            self.queue.starts.starts.insert(id.to_owned(), Start::Creating { directory, draft, created: false });
        }
    }

    /// Send on the new-session page, live: the session shows and its start runs its phases,
    /// holding `sent` and `prompt` until the server is asked to create it. False (the draft
    /// stays) when the outbox couldn't take a start that needed no phase.
    pub(crate) fn begin_start(&mut self, id: &str, sent: Sent, prompt: Prompt, tx: &mut Tx) -> bool {
        let place = self.start_place();
        let directory = match &place {
            Place::Existing(d) => d.clone(),
            Place::New { from, .. } => from.clone(),
        };
        let toggling = match &place {
            Place::Existing(d) => self.mcp_toggling_in(d),
            Place::New { .. } => Vec::new(),
        };
        // A start with nothing to wait for journals at once, as before, so a write the outbox
        // refuses leaves the draft where it is.
        if !matches!(place, Place::New { .. }) && toggling.is_empty() {
            if !self.enqueue_prompt(sent, Write::StartAs { session: id.to_owned(), directory: directory.clone(), prompt }) {
                return false;
            }
            let draft = self.show_start(id, &directory, tx);
            self.queue.starts.starts.insert(id.to_owned(), Start::Creating { directory, draft, created: false });
            return true;
        }
        // Its row first: a start that has to wait keeps its words in the outbox from now on.
        let held = Held { id: sent.id.clone() };
        let write = match &place {
            Place::New { project, from, branch } => Write::StartInNewWorktree { session: id.to_owned(), project: project.clone(), from: from.clone(), branch: branch.clone(), prompt },
            Place::Existing(d) => Write::StartAs { session: id.to_owned(), directory: d.clone(), prompt },
        };
        if !self.enqueue_prepared(sent, write) {
            return false;
        }
        let draft = self.show_start(id, &directory, tx);
        let now = *self.clock;
        let effects = self.queue.starts.step(id, Event::Begin { place, draft, held, toggling, now });
        self.start_effects(id, effects, tx);
        true
    }

    /// One of the folder `directory`'s MCP toggles ended (`mcp::mcp_settled`).
    pub(crate) fn start_mcp_settled(&mut self, directory: &str, toggle: Attempt, ok: bool, tx: &mut Tx) {
        let waiting: Vec<String> = self.queue.starts.starts.iter().filter(|(_, s)| matches!(s, Start::Mcp { directory: d, .. } if d == directory)).map(|(id, _)| id.clone()).collect();
        for id in waiting {
            let effects = self.queue.starts.step(&id, Event::McpSettled { directory: directory.to_owned(), toggle, ok });
            self.start_effects(&id, effects, tx);
        }
    }

    /// Each frame: a phase past its time rolls its start back.
    pub(crate) fn start_timers(&mut self, tx: &mut Tx) {
        let now = *self.clock;
        if self.queue.starts.due().is_some_and(|d| now + 1e-9 >= d) {
            let ids: Vec<String> = self.queue.starts.starts.keys().cloned().collect();
            for id in ids {
                let effects = self.queue.starts.step(&id, Event::Tick { now });
                self.start_effects(&id, effects, tx);
            }
        }
        if let Some(due) = self.queue.starts.due().filter(|d| d.is_finite()) {
            tx.wake_after(std::time::Duration::from_secs_f64((due - now).max(0.0)));
        }
    }

    fn start_effects(&mut self, id: &str, effects: Vec<Effect>, tx: &mut Tx) {
        for effect in effects {
            match effect {
                // The request isn't cut off when the phase ends: that could stop the project's
                // setup script midway. A worktree it makes after that stays, and lists.
                Effect::CreateWorktree { attempt, project, from, branch } => {
                    let (session, key) = (id.to_owned(), crate::servers::runtime::active(self).clone());
                    self.job(move |srv| {
                        let result = srv.create_worktree(&project, &from, branch.as_deref()).map_err(|e| e.to_string());
                        Box::new(move |s: &mut App, tx: &mut Tx| {
                            crate::servers::runtime::with_server(s, &key, |s| {
                                if let Ok(dir) = &result {
                                    opencode_kit::model::add_project_sandbox(&project, dir.clone());
                                    s.worktree_updated(&project);
                                }
                                let now = *s.clock;
                                let effects = s.queue.starts.step(&session, Event::WorktreeMade { attempt, result, now });
                                if let Some(dir) = s.queue.starts.directory(&session).map(str::to_owned) {
                                    s.set_start_directory(&session, &dir);
                                }
                                s.start_effects(&session, effects, tx);
                            });
                        })
                    });
                }
                Effect::Create { directory, held } => {
                    self.set_start_directory(id, &directory);
                    // Its row goes from Preparing to Waiting, on to the folder the session runs in.
                    self.release_prepared(&held.id, Some(directory));
                }
                Effect::Promote => {}
                Effect::Rollback { directory, draft, held, worktree, why } => self.roll_back_start(id, directory, draft, held, worktree, why, tx),
            }
        }
    }

    fn set_start_directory(&mut self, id: &str, directory: &str) {
        if let Some(info) = self.sessions.iter_mut().find(|s| s.id == id) {
            info.directory.clear();
            info.directory.push_str(directory);
        }
        let server = crate::servers::runtime::active(self).clone();
        let mut updated = false;
        if let Some(tab) = self.tabs.iter_mut().find(|t| !t.is_draft() && t.server == server && t.id == id)
            && tab.directory != directory
        {
            tab.directory.clear();
            tab.directory.push_str(directory);
            updated = true;
        }
        if updated {
            self.save_tabs();
        }
    }

    /// Where the open draft's session runs: a new worktree of its project, or its folder.
    fn start_place(&self) -> Place {
        match self.draft_project().filter(|_| self.draft_creates()) {
            Some(project) => Place::New { project: project.id.clone(), from: project.directory.clone(), branch: self.draft_base().map(str::to_owned) },
            None => Place::Existing(self.draft_dir().to_owned()),
        }
    }

    /// `Incoming::Created` for a start: the server has the session and its prompt. It takes the
    /// server's details and reads what opening it would have.
    pub(crate) fn started(&mut self, mut info: SessionInfo, tx: &mut Tx) {
        if !matches!(self.queue.starts.step(&info.id, Event::Created).first(), Some(Effect::Promote)) {
            return;
        }
        let id = info.id.clone();
        if let Some(old) = self.sessions.iter_mut().find(|s| s.id == id) {
            info.viewed = info.viewed.max(old.viewed);
            *old = info;
        }
        if let Source::Live { requests: Some(r), .. } = &*self.source {
            if !self.pages.has_pool() { r.send(Request::Session(id.clone())).ok(); }
            r.send(Request::Inbox(id.clone())).ok();
        }
        self.read_requests(&id);
        self.open_page(&id, tx);
    }

    /// `Incoming::SendFailed` for a start: the session goes (the server has deleted whatever it
    /// made), and the new-session page comes back with the prompt in its composer. False when
    /// `session` isn't starting.
    pub(crate) fn start_failed(&mut self, session: &str, prompt: &str, error: &opencode_kit::wire_error::WireError, tx: &mut Tx) -> bool {
        let effects = self.queue.starts.step(session, Event::Failed);
        let Some(Effect::Rollback { directory, draft, worktree, .. }) = effects.into_iter().next() else { return false };
        let held = Some(Held { id: prompt.to_owned() });
        let why = Why::Failed { title: FAILED, message: crate::errors::said(self, "", error) };
        self.roll_back_start(session, directory, draft, held, worktree, why, tx);
        true
    }

    /// `tab.rollback`: the session goes and the draft tab comes back, with its words in the
    /// composer. A worktree this start made becomes the draft's folder.
    #[allow(clippy::too_many_arguments)]
    fn roll_back_start(&mut self, session: &str, directory: String, draft: Option<String>, held: Option<Held>, worktree: bool, why: Why, tx: &mut Tx) {
        let directory = if worktree { directory } else { self.session(session).map(|i| i.directory.clone()).unwrap_or(directory) };
        self.sessions.retain(|s| s.id != session);
        // The draft's agents get their models back.
        self.composer_ext.pickers.demote_session(session);
        crate::pickers::saved::save(self, session);
        self.transcripts.remove(session);
        self.inboxes.remove(session);
        self.running.remove(session);
        let draft = draft.unwrap_or_else(crate::tabs::draft_id);
        self.demote_session(session, &draft, &directory);
        self.forget_history(|_, id| id == session);
        if self.route == Route::here(session.into()) {
            *self.route = Route::Draft(draft);
            // The composer swaps to the draft's words now, so the prompt below lands in them.
            self.show_draft();
            if worktree {
                self.set_draft_dir(directory);
            }
            if crate::auto_focus_composer(tx) {
                self.composer.focused = true;
            }
            tx.settle();
        }
        if let Some(sent) = held.and_then(|h| self.unsent.take(&h.id)) {
            self.restore_draft(sent);
        }
        match why {
            Why::Quiet => {}
            Why::Failed { title, message } => crate::status_toast::fail(self, title, crate::status_toast::plain(&message)),
            Why::TimedOut { title } => crate::status_toast::fail(self, title, opencode_i18n::t!("session.new.timedOut")),
        }
    }
}

mod stories;
