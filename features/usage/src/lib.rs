//! View usage (`composer/commands.tsx:90-130`, `usage/dialog.tsx`, `usage/machine.ts`): the
//! location's OpenCode Go allowance and Console budgets, read from the server each time the
//! dialog opens. Limits read before show while the new read runs; a read that fails, or takes
//! longer than 15 s, offers Try again; a server not signed in to Console says so.

pub mod limits;
pub mod view;

use std::collections::HashMap;

use opencode_kit::model::{Message, Transcript};
use rune::Tx;
use rune::machine::{Attempt, Seq};
use serde_json::Value;

pub use limits::{Budget, BudgetModel, CurrentModel, Go, Info, Meter, Workspace};
pub use view::{dialog, layer, money};

/// Overlay stack identifier for the Usage dialog.
pub const OVERLAY_ID: &str = "dialog.usage";

/// `USAGE_LOAD_TIMEOUT_MS`: seconds before a pending limits read times out.
pub const TIMEOUT_S: f64 = 15.0;

/// Object-safe host boundary for the Usage feature.
pub trait UsageHost {
    fn usage(&self) -> &Usage;
    fn usage_mut(&mut self) -> &mut Usage;
    fn composer_directory(&self) -> String;
    fn selected_model(&self) -> Option<CurrentModel>;
    fn is_live(&self) -> bool;
    fn fixture_usage_limits(&self) -> Option<&Value>;
    fn fetch_usage_limits(&mut self, directory: String, seq: u32);
    fn fetch_picker_usage_limits(&mut self, directory: String) {
        let _ = directory;
    }
    fn out_of_credits(&self) -> bool {
        false
    }
    fn now_ms(&self) -> i64 {
        0
    }
    fn zoom(&self) -> f32 {
        1.0
    }
    fn open_external(&mut self, url: &str, tx: &mut Tx) {
        drop(tx.open_url(url));
    }
    fn open_model_picker(&mut self, tx: &mut Tx) {
        let _ = tx;
    }
    fn on_overlay_opened(&mut self, id: &'static str, return_focus: Option<String>);
    fn on_overlay_closed(&mut self, id: &'static str);
}

/// The dialog, when open, and the limits each location answered last.
#[derive(Default)]
pub struct Usage {
    pub open: Option<Open>,
    pub cache: HashMap<String, Info>,
}

pub struct Open {
    pub directory: String,
    /// The model in use, so a per-model budget shows its own counter.
    pub model: Option<CurrentModel>,
    /// The last request was refused because the workspace is out of credits.
    pub credits: bool,
    pub load: Load,
    /// The last load's attempt: a load whose attempt `load` no longer holds is stale.
    pub issued: Seq,
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

/// Server endpoint path for fetching usage limits at `directory`.
pub fn limits_endpoint(directory: &str) -> String {
    format!("/api/experimental/usage/limits?location%5Bdirectory%5D={}", encode_query(directory))
}

fn encode_query(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => (b as char).to_string(),
            _ => format!("%{b:02X}"),
        })
        .collect()
}

/// Console refuses with 402 only when the workspace can't pay: checks the open session's
/// last assistant reply, whatever came after it (`viewUsage`).
pub fn transcript_out_of_credits(transcript: &Transcript) -> bool {
    transcript
        .messages
        .iter()
        .rev()
        .find_map(|m| match m {
            Message::Assistant(a) => Some(a),
            _ => None,
        })
        .and_then(|a| a.failure.as_ref())
        .is_some_and(|e| e.status == Some(402))
}

/// `usage.view`: opens the dialog over the composer's location and reads its limits.
pub fn open(s: &mut dyn UsageHost, tx: &mut Tx) {
    tx.instant();
    let directory = s.composer_directory();
    let model = s.selected_model();
    let credits = s.out_of_credits();
    let cached = s.usage().cache.get(&directory).cloned();
    let mut open_state = Open { directory, model, credits, load: Load::Failed, issued: Seq::default() };
    let sync = load(&mut open_state, cached);
    s.usage_mut().open = Some(open_state);
    let return_focus = tx.focused().map(str::to_owned);
    s.on_overlay_opened(OVERLAY_ID, return_focus);
    sync_usage(s, sync, tx);
}

/// Closes the Usage dialog and notifies the host overlay stack.
pub fn close(s: &mut dyn UsageHost) {
    s.usage_mut().open = None;
    s.on_overlay_closed(OVERLAY_ID);
}

/// Try again, after a failed read.
pub fn retry(s: &mut dyn UsageHost, tx: &mut Tx) {
    tx.instant();
    usage_step(s, Event::Retry, tx);
}

/// Marks load attempt `seq` as timed out after [`TIMEOUT_S`].
pub fn timed_out(s: &mut dyn UsageHost, seq: u32, tx: &mut Tx) {
    usage_step(s, Event::TimedOut { attempt: Attempt(seq) }, tx);
}

/// A read's answer: limits, signed out (`null`), or a failure. It fills the location's cache
/// either way; the dialog takes it only while it waits for that attempt at that location.
pub fn apply_response(s: &mut dyn UsageHost, seq: u32, directory: &str, answer: Result<Value, String>, tx: &mut Tx) {
    let attempt = Attempt(seq);
    let info = answer.as_ref().ok().map(|v| v.get("data").unwrap_or(v));
    if let Some(parsed) = info.and_then(Info::parse) {
        s.usage_mut().cache.insert(directory.to_owned(), parsed);
    }
    if s.usage().open.as_ref().is_none_or(|o| o.directory != directory) {
        return;
    }
    let event = match info {
        Some(Value::Null) => Event::Loaded { attempt, info: None },
        Some(v) => Info::parse(v).map_or(Event::Failed { attempt }, |info| Event::Loaded { attempt, info: Some(info) }),
        None => Event::Failed { attempt },
    };
    usage_step(s, event, tx);
}

/// Refreshes the cached limits for the composer's location when the model picker opens.
pub fn refresh_picker(s: &mut dyn UsageHost) {
    let directory = s.composer_directory();
    if !s.is_live() {
        let parsed = s.fixture_usage_limits().and_then(Info::parse);
        if let Some(info) = parsed {
            s.usage_mut().cache.insert(directory, info);
        }
        return;
    }
    s.fetch_picker_usage_limits(directory);
}

/// Applies a background model-picker usage response to the location cache.
pub fn apply_picker_response(s: &mut dyn UsageHost, directory: &str, answer: Result<Value, String>, tx: &mut Tx) {
    if let Ok(value) = answer {
        if let Some(info) = Info::parse(value.get("data").unwrap_or(&value)) {
            s.usage_mut().cache.insert(directory.to_owned(), info);
        } else {
            s.usage_mut().cache.remove(directory);
        }
        tx.instant();
    }
}

/// Localized budget label for `(provider, model)` at the composer's location, paired with
/// whether the cap has been reached (`true` when `left <= 0.0`).
pub fn warning_for(s: &dyn UsageHost, provider: &str, model: &str, served: bool) -> Option<(String, bool)> {
    let info = s.usage().cache.get(&s.composer_directory())?;
    let left = info.model_left(provider, model, served)?;
    let label = if left <= 0.0 {
        opencode_i18n::t!("usage.budget.reached").into()
    } else {
        opencode_i18n::t!("usage.budget.left", amount = view::money(left, false))
    };
    Some((label, left <= 0.0))
}

/// Opens the workspace usage details URL through the host.
pub fn open_details(s: &mut dyn UsageHost, url: &str, tx: &mut Tx) {
    s.open_external(url, tx);
}

/// Closes the Usage dialog and opens the host's model picker.
pub fn switch_model(s: &mut dyn UsageHost, tx: &mut Tx) {
    tx.instant();
    close(s);
    s.open_model_picker(tx);
}

/// Reads the open dialog's limits (`usage.limits` through `data.location.usage.sync`), giving
/// up after the timeout.
fn sync_usage(s: &mut dyn UsageHost, sync: Sync, tx: &mut Tx) {
    let Some(directory) = s.usage().open.as_ref().map(|o| o.directory.clone()) else { return };
    let seq = sync.attempt.0;
    if !s.is_live() {
        // No server, no Console: a fixture's own `usage_limits`, else signed out.
        let fixture = s.fixture_usage_limits().cloned().unwrap_or(Value::Null);
        let answer = Ok(serde_json::json!({ "data": fixture }));
        apply_response(s, seq, &directory, answer, tx);
        return;
    }
    s.fetch_usage_limits(directory, seq);
}

fn usage_step(s: &mut dyn UsageHost, event: Event, tx: &mut Tx) {
    let sync = {
        let Some(open) = s.usage_mut().open.as_mut() else { return };
        let before = open.load.clone();
        let sync = step(open, event);
        if open.load != before {
            tx.instant();
        }
        sync
    };
    if let Some(sync) = sync {
        sync_usage(s, sync, tx);
    }
}
