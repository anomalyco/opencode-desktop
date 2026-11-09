//! View usage (`composer/commands.tsx:90-130`, `usage/dialog.tsx`, `usage/machine.ts`): the
//! location's OpenCode Go allowance and Console budgets, read from the server each time the
//! dialog opens. Limits read before show while the new read runs; a read that fails, or takes
//! longer than the client's 15 s, offers Try again; a server not signed in to Console says so.

use std::collections::HashMap;

use rune::Tx;
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
    attempt: u32,
}

/// `UsageDialogState`, less what the client's timeout and the app's thread already give.
#[derive(Clone, Debug, PartialEq)]
pub enum Load {
    Loading,
    /// Limits read before, while the dialog's own read runs.
    Refreshing(Info),
    Ready(Info),
    SignedOut,
    Failed,
}

impl App {
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
        let load = self.usage.cache.get(&directory).cloned().map_or(Load::Loading, Load::Refreshing);
        // Console refuses with 402 only when the workspace can't pay, which the dialog explains:
        // the open session's last reply, whatever came after it (`viewUsage`).
        let last = self.active_session_id().and_then(|id| self.transcripts.get(id)).and_then(|t| {
            t.messages.iter().rev().find_map(|m| match m {
                opencode_kit::model::Message::Assistant(a) => Some(a),
                _ => None,
            })
        });
        let credits = last.and_then(|a| a.failure.as_ref()).is_some_and(|e| e.status == Some(402));
        self.usage.open = Some(Open { directory, model, credits, load, attempt: 0 });
        self.load_usage();
    }

    pub fn close_usage(&mut self) {
        self.usage.open = None;
    }

    /// Reads the open dialog's limits again (`usage.limits` through `data.location.usage.sync`).
    fn load_usage(&mut self) {
        let Some(open) = self.usage.open.as_mut() else { return };
        open.attempt += 1;
        let (attempt, directory) = (open.attempt, open.directory.clone());
        if let Source::Fixture = self.source {
            // No server, no Console: a fixture's own `usage_limits`, else signed out.
            let answer = Ok(serde_json::json!({ "data": self.fixture.get("usage_limits").cloned().unwrap_or(Value::Null) }));
            return loaded(self, attempt, &directory, answer);
        }
        self.job(move |srv| {
            let path = format!("/api/experimental/usage/limits?location%5Bdirectory%5D={}", crate::switcher::encode(&directory));
            let answer = srv.get(&path);
            Box::new(move |s: &mut App, tx: &mut Tx| {
                tx.instant();
                loaded(s, attempt, &directory, answer);
            })
        });
    }

    /// Try again, after a failed read.
    fn retry_usage(&mut self) {
        let Some(open) = self.usage.open.as_mut().filter(|o| o.load == Load::Failed) else { return };
        open.load = Load::Loading;
        self.load_usage();
    }
}

/// A read's answer: limits, signed out (`null`), or a failure. One the dialog no longer waits
/// for is dropped.
fn loaded(s: &mut App, attempt: u32, directory: &str, answer: Result<Value, String>) {
    let info = answer.as_ref().ok().map(|v| v.get("data").unwrap_or(v));
    if let Some(info) = info.and_then(Info::parse) {
        s.usage.cache.insert(directory.to_owned(), info);
    }
    let Some(open) = s.usage.open.as_mut().filter(|o| o.attempt == attempt && o.directory == directory) else { return };
    open.load = match info {
        Some(Value::Null) => Load::SignedOut,
        Some(v) => Info::parse(v).map_or(Load::Failed, Load::Ready),
        None => Load::Failed,
    };
}

rune_register::register!(crate::commands::COMMANDS: crate::commands::Cmd = [
    crate::commands::Cmd::new("usage.view", opencode_i18n::key!("command.usage.view"), |s, tx| s.open_usage(tx))
        .describe(opencode_i18n::key!("command.usage.view.description"))
        .when(App::has_composer),
]);
