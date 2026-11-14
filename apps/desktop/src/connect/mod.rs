//! Connect an integration (Desktop's `DialogConnectProvider`, `providers/connect/*`): the
//! searchable list of providers and services, then the picked one's sign-in (an API key, a
//! browser sign-in, a device code, a command or an external login, with the method's form), its
//! accounts once connected, and the models to start with. `+` beside the model search, `/connect`
//! and Settings › Providers open it. The steps are [`dialog`] and [`machine`]; this file runs
//! their effects against the server.

pub mod catalog;
pub(crate) mod picker_reads;
pub mod dialog;
pub mod form;
pub mod machine;
mod run;
mod scenes;
mod steps;
pub mod view;
pub mod welcome;

use rune::{Key, KeyEvent, TextEdit, Tx};

use crate::App;
use crate::pickers::PickerHost as _;
use dialog::{Accounts, Dialog, Event};

/// The open dialog and what its inputs hold.
pub struct Open {
    pub dialog: Dialog,
    /// The folder the sign-in belongs to; none for the server's default location.
    pub dir: Option<String>,
    /// The composer whose model Continue sets; none starts a new session with it.
    pub scope: Option<String>,
    pub search: TextEdit,
    /// The list's lit row, by integration id.
    pub active: Option<String>,
    /// The API key or the pasted code; a new step starts it empty.
    pub entry: TextEdit,
    /// A form step's text, or its own answer; a new step starts it on its default.
    pub field: TextEdit,
    /// The account whose options are open.
    pub menu: Option<String>,
    pub rename: TextEdit,
}

#[derive(Default)]
pub struct Connect {
    pub open: Option<Open>,
    /// The ChatGPT plan welcome shows.
    pub welcome: bool,
}

fn focused(text: &str) -> TextEdit {
    let mut t = TextEdit::new(text);
    t.focused = true;
    t
}

impl App {
    /// Opens the dialog on its list, or on `provider`'s sign-in. From Settings it signs in at the
    /// server's default location; from the composer, in the composer's folder.
    pub fn open_connect(&mut self, provider: Option<&str>, from_settings: bool, tx: &mut Tx) {
        tx.instant();
        self.close_picker(false);
        self.pickers.close_manage();
        self.composer.focused = false;
        let dir = (!from_settings).then(|| self.composer_dir()).filter(|d| !d.is_empty());
        let scope = (!from_settings).then(|| self.picker_scope());
        self.connect.open = Some(Open {
            dialog: Dialog::default(),
            dir: dir.clone(),
            scope,
            search: focused(""),
            active: None,
            entry: TextEdit::new(""),
            field: TextEdit::new(""),
            menu: None,
            rename: TextEdit::new(""),
        });
        self.fresh_inputs();
        self.sync_catalogs(dir, |_, _, _| {});
        if let Some(id) = provider {
            let accounts = self.accounts_of(id);
            self.connect_event(Event::Selected { provider: id.to_owned(), from_picker: false, accounts }, tx);
        }
    }

    pub fn close_connect(&mut self, tx: &mut Tx) {
        if self.connect.open.is_none() {
            return;
        }
        tx.instant();
        let welcome = self.connect.open.as_ref().is_some_and(|o| o.dialog.chatgpt_welcome);
        self.connect_event(Event::Disposed, tx);
        self.connect.open = None;
        self.composer.focused = self.has_composer() && !self.settings.open;
        if welcome {
            self.welcome_chatgpt_plan();
        }
    }

    /// The folder a sign-in from the composer belongs to: the open session's, or the draft's.
    fn composer_dir(&self) -> String {
        match &*self.route {
            crate::Route::Session { id, .. } => self.session(id).map(|s| s.directory.clone()).unwrap_or_default(),
            _ => self.draft_dir().to_owned(),
        }
    }

    pub(crate) fn connect_dir(&self) -> Option<&str> {
        self.connect.open.as_ref().and_then(|o| o.dir.as_deref())
    }

    /// The integration's accounts as the server lists them, the active one first.
    pub(crate) fn accounts_of(&self, id: &str) -> Accounts {
        let Some(i) = self.integration_of(self.connect_dir(), id) else { return Accounts::default() };
        let list = i.accounts();
        Accounts { count: list.len(), active: list.first().map(|a| a.id.clone()), problem: list.first().is_some_and(|a| a.status.is_some()) }
    }

    /// Runs one event through the dialog and its effects.
    pub(crate) fn connect_event(&mut self, event: Event, tx: &mut Tx) {
        let Some(open) = self.connect.open.as_mut() else { return };
        let (next, effects) = dialog::transition(&open.dialog, event);
        // A transition to another step (or method, or form) starts its inputs afresh.
        let moved = steps::inputs_id(&open.dialog) != steps::inputs_id(&next);
        open.dialog = next;
        if moved {
            self.fresh_inputs();
        }
        for effect in effects {
            self.run_connect(effect, tx);
        }
    }

    /// A new step's inputs start empty, its first one taking the keys.
    fn fresh_inputs(&mut self) {
        let Some(open) = self.connect.open.as_mut() else { return };
        let value = steps::field_default(&open.dialog);
        open.field = focused(&value);
        open.entry = focused("");
        open.search.focused = open.dialog.provider.is_none();
        open.menu = None;
    }

    /// The catalogs changed: the dialog follows the accounts and Claude Agent's login.
    pub(crate) fn catalogs_changed(&mut self, tx: &mut Tx) {
        let Some(p) = self.connect.open.as_ref().and_then(|o| o.dialog.provider.as_ref()) else { return };
        let id = p.id.clone();
        let claude = matches!(p.step, dialog::Step::Claude { .. });
        let accounts = self.accounts_of(&id);
        self.connect_event(Event::AccountsSynced(accounts.clone()), tx);
        if claude {
            let models = !self.connection_models(&id).is_empty();
            self.connect_event(Event::ClaudeStatus { problem: accounts.problem, models }, tx);
        }
    }

    /// The models a connection to `id` offers: its providers' (`id` or integrating as `id`).
    pub(crate) fn connection_models(&self, id: &str) -> Vec<crate::pickers::Model> {
        let infos = self.provider_infos(self.connect_dir()).unwrap_or_default();
        let of = |provider: &str| provider == id || infos.iter().any(|p| p.id == provider && p.integration.as_deref() == Some(id));
        self.pickers.models.iter().filter(|m| of(&m.provider)).cloned().collect()
    }
}

/// The dialog's keys: Escape closes it, and the list's arrows and Enter pick a provider.
pub fn key(s: &mut App, ev: &KeyEvent, tx: &mut Tx) -> bool {
    if welcome::key(s, ev, tx) {
        return true;
    }
    let Some(open) = &s.connect.open else { return false };
    if ev.key == Key::Escape {
        if open.menu.is_some() {
            tx.instant();
            s.connect.open.as_mut().expect("open").menu = None;
            return true;
        }
        s.close_connect(tx);
        return true;
    }
    if open.dialog.provider.is_none() {
        return view::picker_key(s, ev, tx);
    }
    false
}

rune_register::register!(crate::commands::COMMANDS: crate::commands::Cmd = [
    crate::commands::Cmd::new("provider.connect", opencode_i18n::key!("command.provider.connect"), |s, tx| s.open_connect(None, false, tx))
        .describe(opencode_i18n::key!("command.provider.connect.description"))
        .slash("connect")
        .when(App::has_composer),
]);
