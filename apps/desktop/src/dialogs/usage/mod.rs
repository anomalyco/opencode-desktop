//! View usage (`composer/commands.tsx:90-130`, `usage/dialog.tsx`, `usage/machine.ts`): the
//! location's OpenCode Go allowance and Console budgets, read from the server each time the
//! dialog opens. Limits read before show while the new read runs; a read that fails, or takes
//! longer than 15 s, offers Try again; a server not signed in to Console says so.

use std::collections::HashMap;

use rune::Tx;
use rune::machine::{Attempt, Seq};
use serde_json::Value;

use crate::pickers::PickerHost as _;
use crate::{App, Source};

pub mod limits;
mod stories;
mod view;

pub use view::layer;

use limits::{CurrentModel, Info};

/// The dialog, when open, and the limits each location answered last.
#[derive(Default)]
pub struct Usage {
    pub open: Option<Open>,
    cache: HashMap<String, Info>,
}

pub struct Open {
    directory: String,
    /// The model in use, so a per-model budget shows its own counter.
    model: Option<CurrentModel>,
    /// The last request was refused because the workspace is out of credits.
    credits: bool,
    pub load: Load,
    /// The last load's attempt: a load whose attempt `load` no longer holds is stale.
    issued: Seq,
}

/// `UsageDialogState` (`usage/machine.ts:16-28`) for the open dialog's location. `Refreshing`
/// shows limits read before while the dialog's own load runs.
#[derive(Clone, Debug, PartialEq)]
pub enum Load {
    Loading { attempt: Attempt },
    Refreshing { attempt: Attempt, info: Info },
    Ready(Info),
    SignedOut,
    Failed,
}

/// `UsageDialogEvent`, less `location.changed` and `cache.synced`: the dialog keeps the location
/// it opened on, and nothing else refreshes the cache while it shows.
#[derive(Clone, Debug, PartialEq)]
enum Event {
    /// `None`: the server isn't signed in to Console.
    Loaded { attempt: Attempt, info: Option<Info> },
    Failed { attempt: Attempt },
    TimedOut { attempt: Attempt },
    Retry,
}

/// `UsageDialogEffect`: `sync` with its `timer.start`. The attempt fences the timer, so
/// `timer.stop` has nothing to do.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Sync {
    attempt: Attempt,
}

/// `USAGE_LOAD_TIMEOUT_MS`.
const TIMEOUT_S: f64 = 15.0;

/// `load`: a fresh attempt, showing `cached` while it runs.
fn load(open: &mut Open, cached: Option<Info>) -> Sync {
    let attempt = open.issued.next();
    open.load = match cached {
        Some(info) => Load::Refreshing { attempt, info },
        None => Load::Loading { attempt },
    };
    Sync { attempt }
}

/// `transitionUsageDialog`.
fn step(open: &mut Open, event: Event) -> Option<Sync> {
    let waiting = match &open.load {
        Load::Loading { attempt } | Load::Refreshing { attempt, .. } => Some(*attempt),
        _ => None,
    };
    match event {
        Event::Retry if open.load == Load::Failed => Some(load(open, None)),
        Event::Loaded { attempt, info } if waiting == Some(attempt) => {
            open.load = info.map_or(Load::SignedOut, Load::Ready);
            None
        }
        Event::Failed { attempt } | Event::TimedOut { attempt } if waiting == Some(attempt) => {
            open.load = Load::Failed;
            None
        }
        _ => None,
    }
}
impl App {
    pub(crate) fn refresh_picker_usage(&mut self) {
        let directory = self.composer_directory();
        if !self.live() {
            if let Some(info) = self.fixture.get("usage_limits").and_then(Info::parse) { self.usage.cache.insert(directory, info); }
            return;
        }
        self.job(move |srv| {
            let path = format!("/api/experimental/usage/limits?location%5Bdirectory%5D={}", crate::switcher::encode(&directory));
            let result = srv.get(&path);
            Box::new(move |s: &mut App, tx: &mut Tx| {
                if let Ok(value) = result {
                    if let Some(info) = Info::parse(value.get("data").unwrap_or(&value)) { s.usage.cache.insert(directory, info); }
                    else { s.usage.cache.remove(&directory); }
                    tx.instant();
                }
            })
        });
    }

    pub(crate) fn picker_budget_label(&self, key: &str) -> Option<(String, bool)> {
        let info = self.usage.cache.get(&self.composer_directory())?;
        let model = self.pickers.models.iter().find(|m| m.key == key && !m.free)?;
        let served = model.provider != "opencode-go" && (model.provider == "opencode" || self.provider_infos(None).unwrap_or_default().iter()
            .any(|p| p.id == model.provider && p.integration.as_deref() == Some("opencode")));
        let left = info.model_left(&model.provider, &model.id, served)?;
        let label = if left <= 0.0 { opencode_i18n::t!("usage.budget.reached").into() }
            else { opencode_i18n::t!("usage.budget.left", amount = view::money(left, false)) };
        Some((label, left <= 0.0))
    }
    /// `usage.view`: opens the dialog over the composer's location and reads its limits.
    pub fn open_usage(&mut self, tx: &mut Tx) {
        tx.instant();
        let directory = self.composer_directory();
        let selection = self.selection();
        let model = self.pickers().models.iter().find(|m| m.provider == selection.model.provider && m.id == selection.model.id).map(|m| CurrentModel {
            provider: m.provider.clone(),
            model: m.id.clone(),
            name: m.name.clone(),
        });
        let cached = self.usage.cache.get(&directory).cloned();
        // Console refuses with 402 only when the workspace can't pay, which the dialog explains:
        // the open session's last reply, whatever came after it (`viewUsage`).
        let last = self.active_session_id().and_then(|id| self.transcripts.get(id)).and_then(|t| {
            t.messages.iter().rev().find_map(|m| match m {
                opencode_kit::model::Message::Assistant(a) => Some(a),
                _ => None,
            })
        });
        let credits = last.and_then(|a| a.failure.as_ref()).is_some_and(|e| e.status == Some(402));
        let mut open = Open { directory, model, credits, load: Load::Failed, issued: Seq::default() };
        let sync = load(&mut open, cached);
        self.usage.open = Some(open);
        self.sync_usage(sync);
    }

    pub fn close_usage(&mut self) {
        self.usage.open = None;
    }

    /// Reads the open dialog's limits (`usage.limits` through `data.location.usage.sync`), giving
    /// up after the timeout.
    fn sync_usage(&mut self, sync: Sync) {
        let Some(open) = self.usage.open.as_ref() else { return };
        let (attempt, directory) = (sync.attempt, open.directory.clone());
        if let Source::Fixture = *self.source {
            // No server, no Console: a fixture's own `usage_limits`, else signed out.
            let answer = Ok(serde_json::json!({ "data": self.fixture.get("usage_limits").cloned().unwrap_or(Value::Null) }));
            return loaded(self, attempt, &directory, answer);
        }
        self.job_after(std::time::Duration::from_secs_f64(TIMEOUT_S), move |_| {
            Box::new(move |s: &mut App, tx: &mut Tx| usage_step(s, Event::TimedOut { attempt }, tx)) as crate::jobs::Apply
        });
        self.job(move |srv| {
            let path = format!("/api/experimental/usage/limits?location%5Bdirectory%5D={}", crate::switcher::encode(&directory));
            let answer = srv.get(&path).map_err(|e| e.to_string());
            Box::new(move |s: &mut App, tx: &mut Tx| {
                tx.instant();
                loaded(s, attempt, &directory, answer);
            })
        });
    }

    /// Try again, after a failed read.
    fn retry_usage(&mut self) {
        usage_step(self, Event::Retry, &mut Tx::default());
    }
}

fn usage_step(s: &mut App, event: Event, tx: &mut Tx) {
    let Some(open) = s.usage.open.as_mut() else { return };
    let before = open.load.clone();
    let sync = step(open, event);
    if open.load != before {
        tx.instant();
    }
    if let Some(sync) = sync {
        s.sync_usage(sync);
    }
}

/// A read's answer: limits, signed out (`null`), or a failure. It fills the location's cache
/// either way; the dialog takes it only while it waits for that attempt at that location.
fn loaded(s: &mut App, attempt: Attempt, directory: &str, answer: Result<Value, String>) {
    let info = answer.as_ref().ok().map(|v| v.get("data").unwrap_or(v));
    if let Some(info) = info.and_then(Info::parse) {
        s.usage.cache.insert(directory.to_owned(), info);
    }
    if s.usage.open.as_ref().is_none_or(|o| o.directory != directory) {
        return;
    }
    let event = match info {
        Some(Value::Null) => Event::Loaded { attempt, info: None },
        Some(v) => Info::parse(v).map_or(Event::Failed { attempt }, |info| Event::Loaded { attempt, info: Some(info) }),
        None => Event::Failed { attempt },
    };
    usage_step(s, event, &mut Tx::default());
}
rune_register::register!(crate::commands::COMMANDS: crate::commands::Cmd = [
    crate::commands::Cmd::new("usage.view", opencode_i18n::key!("command.usage.view"), |s, tx| s.open_usage(tx))
        .describe(opencode_i18n::key!("command.usage.view.description"))
        .when(App::has_composer),
]);
